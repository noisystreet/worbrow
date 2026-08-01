//! 自研 CDP 客户端后端（Chrome/Edge）——V1 已实现（docs/adr/0002-browser-driver-protocols.md / design.md §6.5）。
//!
//! 协议：WebSocket + JSON-RPC。消息类型复用 `jsonrpc`（CDP 与 Marionette 共用）：
//! - 请求：`{id, method, params, sessionId?}`
//! - 响应：`{id, result|error}`；事件：`{method, params}`（无 id，跳过）
//! - `Target.attachToTarget`（flatten）后，页面级命令（`Page.*` / `Runtime.*`）在
//!   browser 连接上带 `sessionId` 发送
//!
//! 生命周期（design.md §10.1/§10.2）：
//! - 启动：`chrome --headless=new --remote-debugging-port=<动态端口> --user-data-dir=<临时目录>`
//! - 发现：`GET http://127.0.0.1:<port>/json/version` 取 `webSocketDebuggerUrl`（browser 级）
//! - 页面：`Target.createTarget` 新建 tab → `Target.attachToTarget` 取 sessionId
//!
//! 命令子集：`Target.createTarget` / `Target.attachToTarget` / `Page.navigate` /
//! `Runtime.evaluate`（取 HTML、轮询 readyState/选择器、验证码判定）/ `Page.captureScreenshot`

use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use url::Url;

use super::BrowserKind;
use super::discovery;
use super::jsonrpc::{IdAllocator, Incoming, RpcRequest, RpcResponse};
use crate::error::Error;
use crate::ports::BrowserDriver;

/// 等待 Chrome DevTools 端口就绪/建 WebSocket 的超时。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// 单条命令等待响应的超时（须大于页面加载轮询，见 PAGE_LOAD_TIMEOUT）。
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
/// 导航后等待 `readyState == "complete"` 的超时（对齐 Firefox 的 pageLoad 30s）。
const PAGE_LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// 选择器/就绪轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// CDP 后端。内部状态经 Mutex 共享（trait 同时有 `&self` 与 `&mut self` 方法）。
pub struct CdpDriver {
    inner: Arc<Mutex<CdpInner>>,
}

struct CdpInner {
    transport: CdpTransport,
    /// `Target.attachToTarget` 返回的页面会话 id（懒创建：首次 navigate 时建立）。
    session_id: Option<String>,
    child: Option<Child>,
    profile: Option<TempDir>,
}

/// 进程回收：Drop 时 kill Chrome 子进程并清理临时 user-data-dir（design.md §8）。
impl Drop for CdpInner {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }
        // 显式取走 TempDir：其 Drop 会删除临时 user-data-dir
        let _ = self.profile.take();
    }
}

impl CdpDriver {
    /// 启动 Chrome 并完成连接：find → 校验版本 → spawn → 发现 ws url → 握手
    /// （design.md §6.2 步骤 3）。
    pub async fn spawn() -> Result<Box<dyn BrowserDriver>, Error> {
        let binary = discovery::find_browser(BrowserKind::Chrome)?;
        // 版本矩阵校验（design.md §10.2）：Chrome/Edge ≥ 109 才支持 --headless=new
        if let Some(version) = discovery::browser_major_version(&binary)
            && version < 109
        {
            return Err(Error::Env(format!(
                "Chrome 版本过低（{version} < 109），不支持 --headless=new（二进制: {}）",
                binary.display()
            )));
        }
        let profile = create_profile()?;
        // Chrome 打 stderr 的 "DevTools listening on ws://..." 用于发现实际端口
        // （--remote-debugging-port=0 = 随机端口，消除 pick_free_port 释放竞态导致
        //  Chrome bind 失败后静默 fallback 到随机端口的问题）
        let devtools_log = profile.path().join("devtools.log");

        let mut cmd = std::process::Command::new(&binary);
        cmd.arg("--headless=new")
            .arg("--remote-debugging-port=0")
            .arg("--no-sandbox")
            .arg("--disable-gpu")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            // 必须用等号形式：空格形式会把路径当第二个 URL target（headless 不支持多 target）
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg("about:blank");
        // Chrome 自身日志重定向丢弃（stdout 防污染输出契约管道，design.md §2）；
        // stderr 写入 devtools.log 供端口发现（随 profile 一起清理）
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::from(std::fs::File::create(&devtools_log).map_err(
            |e| Error::Env(format!("创建 DevTools 日志文件失败: {e}")),
        )?));
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Env(format!("启动 Chrome（{}）失败: {e}", binary.display())))?;

        // 轮询 devtools.log 直到出现 DevTools WebSocket URL（等价 marionette 的 connect_retry）
        let ws_url = match timeout(CONNECT_TIMEOUT, discover_ws_url(&devtools_log)).await {
            Ok(Ok(u)) => u,
            // child 尚未进入 Inner：必须在此主动 kill，否则连接失败会残留进程（design.md §8）
            Ok(Err(e)) => {
                let _ = child.kill();
                return Err(e);
            }
            Err(_) => {
                let _ = child.kill();
                return Err(Error::Timeout(
                    "等待 Chrome DevTools 就绪超时（chrome 启动失败，详见 devtools.log）".into(),
                ));
            }
        };

        let transport = match CdpTransport::connect(&ws_url).await {
            Ok(t) => t,
            Err(e) => {
                let _ = child.kill();
                return Err(e);
            }
        };

        Ok(Box::new(CdpDriver {
            inner: Arc::new(Mutex::new(CdpInner {
                transport,
                session_id: None,
                child: Some(child),
                profile: Some(profile),
            })),
        }))
    }
}

#[async_trait]
impl BrowserDriver for CdpDriver {
    async fn navigate(&mut self, url: Url) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        // 首次导航：新建 target 并 attach，取页面会话 id（懒初始化）
        if inner.session_id.is_none() {
            let tid = inner
                .transport
                .send("Target.createTarget", json!({"url": url.to_string()}), None)
                .await?
                .get("targetId")
                .and_then(|v| v.as_str())
                .map(String::from)
                .ok_or_else(|| Error::Network("createTarget 响应缺少 targetId".into()))?;
            let sid = inner
                .transport
                .send(
                    "Target.attachToTarget",
                    json!({"targetId": tid, "flatten": true}),
                    None,
                )
                .await?
                .get("sessionId")
                .and_then(|v| v.as_str())
                .map(String::from)
                .ok_or_else(|| Error::Network("attachToTarget 响应缺少 sessionId".into()))?;
            inner.session_id = Some(sid);
        }
        // clone 到局部变量，避免对 inner 的双重借用（transport 需 &mut）
        let sid = inner
            .session_id
            .clone()
            .ok_or_else(|| Error::Network("未先 navigate".into()))?;
        inner
            .transport
            .send("Page.navigate", json!({"url": url.to_string()}), Some(&sid))
            .await?;
        // Page.navigate 不等页面加载，主动轮询 readyState（对齐 WebDriver 语义）
        wait_ready(&mut inner, PAGE_LOAD_TIMEOUT).await
    }

    async fn wait_for(&mut self, selector: &str, timeout: Duration) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        let sid = inner
            .session_id
            .clone()
            .ok_or_else(|| Error::Network("未先 navigate 即 wait_for".into()))?;
        // selector 经 {:?} 转义为合法 JS 字符串字面量（含引号/反斜杠安全）
        let expr = format!("!!document.querySelector({selector:?})");
        let deadline = Instant::now() + timeout;
        loop {
            let r = inner
                .transport
                .send("Runtime.evaluate", json!({"expression": expr}), Some(&sid))
                .await?;
            let found = r
                .get("result")
                .and_then(|v| v.get("value"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if found {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(format!("等待选择器超时: {selector}")));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    async fn html(&self) -> Result<String, Error> {
        let mut inner = self.inner.lock().await;
        let sid = inner
            .session_id
            .clone()
            .ok_or_else(|| Error::Network("未先 navigate 即取 HTML".into()))?;
        let r = inner
            .transport
            .send(
                "Runtime.evaluate",
                json!({"expression": "document.documentElement.outerHTML"}),
                Some(&sid),
            )
            .await?;
        r.get("result")
            .and_then(|v| v.get("value"))
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| Error::Network("Runtime.evaluate 响应缺少 outerHTML".into()))
    }

    async fn eval(&mut self, js: &str) -> Result<serde_json::Value, Error> {
        let mut inner = self.inner.lock().await;
        let sid = inner
            .session_id
            .clone()
            .ok_or_else(|| Error::Network("未先 navigate 即 eval".into()))?;
        let r = inner
            .transport
            .send("Runtime.evaluate", json!({"expression": js}), Some(&sid))
            .await?;
        Ok(r.get("result")
            .and_then(|v| v.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    async fn screenshot(&mut self, path: &Path) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        let sid = inner
            .session_id
            .clone()
            .ok_or_else(|| Error::Network("未先 navigate 即截图".into()))?;
        let r = inner
            .transport
            .send(
                "Page.captureScreenshot",
                json!({"format": "png"}),
                Some(&sid),
            )
            .await?;
        let b64 = r
            .get("data")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Network("captureScreenshot 响应缺少 data".into()))?;
        let png = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| Error::Network(format!("截图 base64 解码失败: {e}")))?;
        std::fs::write(path, png)
            .map_err(|e| Error::Internal(format!("写入截图失败（{}）: {e}", path.display())))
    }
}

/// 导航后等待 `document.readyState == "complete"`（对齐 Firefox pageLoad 语义）。
async fn wait_ready(inner: &mut CdpInner, timeout: Duration) -> Result<(), Error> {
    let sid = inner
        .session_id
        .clone()
        .ok_or_else(|| Error::Network("未先 navigate".into()))?;
    let deadline = Instant::now() + timeout;
    loop {
        let r = inner
            .transport
            .send(
                "Runtime.evaluate",
                json!({"expression": "document.readyState"}),
                Some(&sid),
            )
            .await?;
        let state = r
            .get("result")
            .and_then(|v| v.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if state == "complete" {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout(
                "等待页面加载完成（readyState=complete）超时".into(),
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// CDP 传输层：WebSocket + id 匹配（跳过事件帧；tungstenite 自动应答 ping）。
#[derive(Debug)]
struct CdpTransport {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    ids: IdAllocator,
}

impl CdpTransport {
    async fn connect(ws_url: &str) -> Result<Self, Error> {
        let (ws, _) = connect_async(ws_url)
            .await
            .map_err(|e| Error::Network(format!("CDP WebSocket 连接失败（{ws_url}）: {e}")))?;
        Ok(Self {
            ws,
            ids: IdAllocator::default(),
        })
    }

    /// 发送命令并等待 id 匹配的响应（默认命令级超时）；事件/无关帧跳过。
    async fn send(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value, Error> {
        self.send_with_timeout(method, params, session_id, COMMAND_TIMEOUT)
            .await
    }

    /// 带可配置超时的命令发送（测试注入短超时用）。
    async fn send_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        cmd_timeout: Duration,
    ) -> Result<Value, Error> {
        let started = Instant::now();
        let id = self.ids.allocate_id();
        let mut req = RpcRequest::new(id, method, Some(params));
        if let Some(sid) = session_id {
            req = req.with_session(sid);
        }
        let text = serde_json::to_string(&req)
            .map_err(|e| Error::Network(format!("CDP 请求序列化失败: {e}")))?;
        self.ws
            .send(Message::Text(text.into()))
            .await
            .map_err(|e| Error::Network(format!("CDP 发送失败: {e}")))?;

        let result = loop {
            // 命令级超时：Chrome 挂起（JS 死循环/网络异常）时不永久阻塞（design.md §8）
            let msg = timeout(cmd_timeout, self.ws.next()).await.map_err(|_| {
                Error::Timeout(format!("CDP 命令超时（{cmd_timeout:?}）: {method}"))
            })?;
            let msg = msg
                .ok_or_else(|| Error::Network("CDP 连接已关闭".into()))?
                .map_err(|e| Error::Network(format!("CDP WebSocket 错误: {e}")))?;
            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                _ => continue, // ping/pong/close 等由库处理，跳过
            };
            let incoming: Incoming = serde_json::from_str(&text)
                .map_err(|e| Error::Network(format!("CDP 消息解析失败: {e}")))?;
            match incoming {
                Incoming::Response(RpcResponse {
                    id: resp_id,
                    result,
                    error,
                }) if resp_id == id => {
                    if let Some(err) = error {
                        break Err(cdp_error(method, &err));
                    }
                    break Ok(result.unwrap_or(Value::Null));
                }
                _ => continue, // 事件帧或非本 id 响应
            }
        };
        tracing::debug!(
            method,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "cdp 命令完成"
        );
        result
    }
}

/// CDP 协议错误 → 领域错误映射（`{code, message}`）。
fn cdp_error(method: &str, err: &super::jsonrpc::RpcError) -> Error {
    match err.code {
        // -32000：协议层「Server error」，导航到非法 URL / 页面加载失败等
        -32000 => Error::Timeout(format!("CDP {method} 失败（-32000）: {}", err.message)),
        _ => Error::Network(format!(
            "CDP {method} 错误（{}）: {}",
            err.code, err.message
        )),
    }
}

/// 轮询 `devtools.log` 直到 Chrome 打印 DevTools WebSocket URL。
async fn discover_ws_url(log_path: &std::path::Path) -> Result<String, Error> {
    loop {
        let text = std::fs::read_to_string(log_path).unwrap_or_default();
        if let Some(url) = parse_devtools_ws_url(&text) {
            return Ok(url);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// 从 Chrome stderr 输出解析 DevTools WebSocket URL：
/// `DevTools listening on ws://127.0.0.1:<port>/devtools/browser/<id>`（纯函数，便于测试）。
fn parse_devtools_ws_url(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.split_once("DevTools listening on ")
            .map(|(_, url)| url.trim().to_string())
            .filter(|url| url.starts_with("ws://"))
    })
}

/// 创建独立临时 user-data-dir（避免与用户 Chrome 会话/profile 冲突）。
fn create_profile() -> Result<TempDir, Error> {
    tempfile::Builder::new()
        .prefix("worbrow-chrome-profile-")
        .tempdir()
        .map_err(|e| Error::Env(format!("创建 Chrome user-data-dir 失败: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    /// 起一个本地 WebSocket mock server：接收命令并回显响应/事件。
    async fn spawn_ws_mock() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(WsMessage::Text(t))) = ws.next().await {
                let req: Value = serde_json::from_str(&t).unwrap();
                let id = req["id"].as_u64().unwrap();
                let method = req["method"].as_str().unwrap_or_default().to_string();
                let sid = req
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                match method.as_str() {
                    "Target.createTarget" => {
                        ws.send(WsMessage::Text(
                            json!({"id": id, "result": {"targetId": "t1"}})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap();
                    }
                    "Target.attachToTarget" => {
                        ws.send(WsMessage::Text(
                            json!({"id": id, "result": {"sessionId": "s1"}})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap();
                    }
                    "Page.navigate" => {
                        ws.send(WsMessage::Text(
                            json!({"id": id, "result": {}}).to_string().into(),
                        ))
                        .await
                        .unwrap();
                    }
                    "Runtime.evaluate" => {
                        let expr = req["params"]["expression"].as_str().unwrap_or_default();
                        let value = if expr.contains("readyState") {
                            json!("complete")
                        } else if expr.contains("querySelector") {
                            json!(true)
                        } else if expr.contains("outerHTML") {
                            json!("<html><body><h1>mock</h1></body></html>")
                        } else {
                            json!(42)
                        };
                        ws.send(WsMessage::Text(
                            json!({"id": id, "result": {"result": {"type": "string", "value": value}}}).to_string().into(),
                        ))
                        .await
                        .unwrap();
                    }
                    _ => {
                        // 先插入事件帧干扰，再回响应（sessionId 原样透传，验证请求带 session）
                        ws.send(WsMessage::Text(
                            json!({"method": "Runtime.consoleAPICalled", "params": {}})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap();
                        ws.send(WsMessage::Text(
                            json!({"id": id, "result": {"ok": true, "sessionId": sid}})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap();
                    }
                }
            }
        });
        format!("ws://{addr}")
    }

    /// 握手 + id 匹配 + 事件帧穿插 + sessionId 透传。
    #[tokio::test]
    async fn transport_handles_id_matching_and_events() {
        let url = spawn_ws_mock().await;
        let mut t = CdpTransport::connect(&url).await.unwrap();
        let r = t
            .send("Target.createTarget", json!({"url": "about:blank"}), None)
            .await
            .unwrap();
        assert_eq!(r["targetId"], "t1");
        let r = t.send("Unknown.cmd", json!({}), Some("s1")).await.unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["sessionId"], "s1");
    }

    /// 完整驱动链路（mock 协议）：navigate → wait_for → html。
    #[tokio::test]
    async fn driver_end_to_end_with_mock_protocol() {
        let url = spawn_ws_mock().await;
        // 绕过 spawn（不启动真实 Chrome），直接注入 mock 传输
        let driver = CdpDriver {
            inner: Arc::new(Mutex::new(CdpInner {
                transport: CdpTransport::connect(&url).await.unwrap(),
                session_id: None,
                child: None,
                profile: None,
            })),
        };
        let mut driver: Box<dyn BrowserDriver> = Box::new(driver);
        driver
            .navigate(Url::parse("https://example.invalid/").unwrap())
            .await
            .unwrap();
        driver
            .wait_for(".b_algo", Duration::from_secs(2))
            .await
            .unwrap();
        let html = driver.html().await.unwrap();
        assert!(html.contains("<h1>mock</h1>"));
    }

    /// 协议错误 → Error::Timeout（-32000）或 Error::Network（其他）。
    #[tokio::test]
    async fn send_surfaces_protocol_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            if let Some(Ok(WsMessage::Text(t))) = ws.next().await {
                let req: Value = serde_json::from_str(&t).unwrap();
                let id = req["id"].as_u64().unwrap();
                ws.send(WsMessage::Text(
                    json!({"id": id, "error": {"code": -32601, "message": "Method not found"}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            }
        });
        let mut t = CdpTransport::connect(&format!("ws://{addr}"))
            .await
            .unwrap();
        let err = t.send("Nope.cmd", json!({}), None).await.unwrap_err();
        assert!(matches!(err, Error::Network(_)));
    }

    /// 命令级超时：服务端收命令后不响应 → Error::Timeout（design.md §8 不挂死）。
    #[tokio::test]
    async fn send_times_out_when_no_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let _ = ws.next().await; // 收命令但不响应，保持连接
            tokio::time::sleep(Duration::from_secs(3)).await;
        });
        let mut t = CdpTransport::connect(&format!("ws://{addr}"))
            .await
            .unwrap();
        let err = t
            .send_with_timeout(
                "Runtime.evaluate",
                json!({}),
                None,
                Duration::from_millis(300),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Timeout(_)));
    }

    /// 从 Chrome stderr 解析 DevTools WebSocket URL。
    #[test]
    fn parses_devtools_ws_url() {
        let text = "DevTools listening on ws://127.0.0.1:34169/devtools/browser/abc\n\
                    [error] nss: something";
        assert_eq!(
            parse_devtools_ws_url(text),
            Some("ws://127.0.0.1:34169/devtools/browser/abc".to_string())
        );
        assert_eq!(parse_devtools_ws_url("no devtools line yet"), None);
        assert_eq!(parse_devtools_ws_url(""), None);
    }
}
