//! 自研 Marionette 客户端后端（Firefox）——V1 已实现（docs/adr/0002-browser-driver-protocols.md / design.md §6.5）。
//!
//! 协议：Firefox DebuggerTransport（TCP + `<ASCII长度>:` 文本帧前缀 + JSON，实测/源码确认）。
//! 消息（四元素数组，参照 marionette crate 与 Firefox message.sys.mjs）：
//! - 命令：`[0, id, "WebDriver:...", params]`
//! - 响应：`[1, id, error|null, result]`（error 为 `{error, message, stacktrace}` 对象）
//! - 握手：连接后 Firefox 主动发 `{"applicationType":"gecko","marionetteProtocol":3}`
//! - 命令子集：`WebDriver:NewSession` / `WebDriver:Navigate` / `WebDriver:ExecuteScript`
//!   （等待与取 HTML 均走此命令）/ `WebDriver:GetPageSource` / `WebDriver:TakeScreenshot`
//!
//! 启动/并发（design.md §10.1/§10.2）：`firefox -marionette -headless -profile <temp>`，
//! 独立临时 profile + `user.js` 写入随机 `marionette.port` 以规避 2828 端口冲突。

use std::net::SocketAddr;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Child;
use tokio::sync::Mutex;
use tokio::time::timeout;
use url::Url;

use super::BrowserKind;
use super::discovery;
use super::jsonrpc::IdAllocator;
use crate::error::Error;
use crate::ports::BrowserDriver;

/// 等待 Firefox Marionette 端口就绪的超时（geckodriver 用 60s）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// 选择器轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Marionette 后端。内部状态经 Mutex 共享（trait 同时有 `&self` 与 `&mut self` 方法）。
pub struct MarionetteDriver {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    transport: MarionetteTransport,
    child: Option<Child>,
    profile: Option<TempDir>,
}

/// 进程回收：Drop 时 kill Firefox 子进程并清理临时 profile（design.md §8）。
impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
        // 显式取走 TempDir：其 Drop 会删除临时 profile
        let _ = self.profile.take();
    }
}

impl MarionetteDriver {
    /// 启动 Firefox 并完成握手：find → spawn → connect → NewSession（design.md §6.2 步骤 3）。
    pub async fn spawn() -> Result<Box<dyn BrowserDriver>, Error> {
        let binary = discovery::find_browser(BrowserKind::Firefox)?;
        let port = pick_free_port()?;
        let profile = create_profile(port)?;

        let mut cmd = tokio::process::Command::new(&binary);
        cmd.arg("-marionette")
            .arg("-headless")
            .arg("-no-remote")
            .arg("-foreground")
            .arg("-profile")
            .arg(profile.path());
        // Firefox 自身的日志（Marionette/webrender 等）重定向丢弃：headless 下会走
        // stdout，污染输出契约管道（design.md §2）；诊断走截图/--dump-html 与 tracing
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Env(format!("启动 Firefox（{}）失败: {e}", binary.display())))?;

        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let transport = match timeout(CONNECT_TIMEOUT, connect_retry(addr)).await {
            Ok(Ok(t)) => t,
            // child 尚未进入 Inner：必须在此主动 kill，否则连接失败会残留进程（design.md §8）
            Ok(Err(e)) => {
                let _ = child.start_kill();
                return Err(e);
            }
            Err(_) => {
                let _ = child.start_kill();
                return Err(Error::Timeout(
                    "等待 Firefox Marionette 端口超时（firefox 启动失败或端口被占用）".into(),
                ));
            }
        };

        let mut inner = Inner {
            transport,
            child: Some(child),
            profile: Some(profile),
        };
        // NewSession 失败时 inner 的 Drop 会 kill 子进程，无泄漏
        inner
            .transport
            .send(
                "WebDriver:NewSession",
                json!({"capabilities": {"alwaysMatch": {}}}),
            )
            .await?;

        Ok(Box::new(MarionetteDriver {
            inner: Arc::new(Mutex::new(inner)),
        }))
    }
}

#[async_trait]
impl BrowserDriver for MarionetteDriver {
    async fn navigate(&mut self, url: Url) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        inner
            .transport
            .send("WebDriver:Navigate", json!({"url": url.to_string()}))
            .await?;
        Ok(())
    }

    async fn wait_for(&mut self, selector: &str, timeout: Duration) -> Result<(), Error> {
        let script = "return !!document.querySelector(arguments[0]);";
        let deadline = Instant::now() + timeout;
        loop {
            let found = {
                let mut inner = self.inner.lock().await;
                let r = inner
                    .transport
                    .send(
                        "WebDriver:ExecuteScript",
                        json!({"script": script, "args": [selector], "newSandbox": true}),
                    )
                    .await?;
                r.get("value").and_then(|v| v.as_bool()).unwrap_or(false)
            };
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
        let r = inner
            .transport
            .send("WebDriver:GetPageSource", json!({}))
            .await?;
        r.get("value")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| Error::Network("GetPageSource 响应缺少 value".into()))
    }

    async fn eval(&mut self, js: &str) -> Result<serde_json::Value, Error> {
        let mut inner = self.inner.lock().await;
        let r = inner
            .transport
            .send(
                "WebDriver:ExecuteScript",
                json!({"script": js, "args": [], "newSandbox": false}),
            )
            .await?;
        Ok(r.get("value").cloned().unwrap_or(Value::Null))
    }

    async fn screenshot(&mut self, path: &Path) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        let r = inner
            .transport
            .send("WebDriver:TakeScreenshot", json!({}))
            .await?;
        let b64 = r
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Network("TakeScreenshot 响应缺少 value".into()))?;
        let png = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| Error::Network(format!("截图 base64 解码失败: {e}")))?;
        std::fs::write(path, png)
            .map_err(|e| Error::Internal(format!("写入截图失败（{}）: {e}", path.display())))
    }
}

/// Marionette 传输层：TCP 帧协议 + id 匹配（忽略 hello/事件帧）。
#[derive(Debug)]
struct MarionetteTransport {
    stream: TcpStream,
    ids: IdAllocator,
}

impl MarionetteTransport {
    /// 连接并消费握手 hello 帧。
    async fn connect(mut stream: TcpStream) -> Result<Self, Error> {
        let frame = read_frame(&mut stream).await?;
        if frame.get("applicationType").is_none() {
            return Err(Error::Network(format!(
                "Marionette 握手失败（非 hello 帧）: {frame}"
            )));
        }
        Ok(Self {
            stream,
            ids: IdAllocator::default(),
        })
    }

    /// 发送命令并等待 id 匹配的响应；hello/事件等无关帧跳过。
    async fn send(&mut self, command: &str, params: Value) -> Result<Value, Error> {
        let id = self.ids.allocate_id();
        // Marionette 命令：四元素数组 [0, id, command, params]
        let req = json!([0, id, command, params]);
        write_frame(&mut self.stream, &req).await?;
        loop {
            let frame = read_frame(&mut self.stream).await?;
            // 响应：数组 [1, id, error|null, result]
            let is_response = frame
                .as_array()
                .map(|a| a.len() >= 4 && a[0].as_u64() == Some(1) && a[1].as_u64() == Some(id))
                .unwrap_or(false);
            if !is_response {
                continue;
            }
            let arr = frame.as_array().expect("已校验为数组");
            let error = &arr[2];
            if !error.is_null() {
                return Err(marionette_error(error));
            }
            return Ok(arr[3].clone());
        }
    }
}

/// 读取一帧：`<ASCII十进制长度>:<JSON>`（Firefox DebuggerTransport 文本帧格式，实测确认）。
async fn read_frame(stream: &mut TcpStream) -> Result<Value, Error> {
    let mut len_bytes = Vec::with_capacity(8);
    loop {
        let b = stream
            .read_u8()
            .await
            .map_err(|e| Error::Network(format!("读取帧长度失败: {e}")))?;
        if b == b':' {
            break;
        }
        if !b.is_ascii_digit() || len_bytes.len() > 10 {
            return Err(Error::Network(format!("帧长度前缀非法: {:?}", len_bytes)));
        }
        len_bytes.push(b);
    }
    let len: usize = std::str::from_utf8(&len_bytes)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            Error::Network(format!(
                "帧长度解析失败: {:?}",
                String::from_utf8_lossy(&len_bytes)
            ))
        })?;
    let mut buf = vec![0u8; len];
    stream
        .read_exact(&mut buf)
        .await
        .map_err(|e| Error::Network(format!("读取帧失败: {e}")))?;
    serde_json::from_slice(&buf).map_err(|e| Error::Network(format!("帧 JSON 解析失败: {e}")))
}

/// 写入一帧：`<ASCII十进制长度>:<JSON>`。
async fn write_frame(stream: &mut TcpStream, v: &Value) -> Result<(), Error> {
    let bytes = serde_json::to_vec(v).map_err(|e| Error::Network(format!("帧序列化失败: {e}")))?;
    stream
        .write_all(format!("{}:", bytes.len()).as_bytes())
        .await
        .map_err(|e| Error::Network(format!("写入帧长度失败: {e}")))?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|e| Error::Network(format!("写入帧失败: {e}")))
}

/// 轮询 TCP 连接直到 Marionette 端口就绪。
async fn connect_retry(addr: SocketAddr) -> Result<MarionetteTransport, Error> {
    loop {
        match TcpStream::connect(addr).await {
            Ok(stream) => return MarionetteTransport::connect(stream).await,
            Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
}

/// 分配一个随机空闲端口（监听后立即释放，供 user.js 写入）。
fn pick_free_port() -> Result<u16, Error> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| Error::Env(format!("分配随机端口失败: {e}")))?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| Error::Env(format!("读取随机端口失败: {e}")))
}

/// 创建独立临时 profile，写入随机 `marionette.port`（design.md §10.1）。
fn create_profile(port: u16) -> Result<TempDir, Error> {
    let dir = tempfile::Builder::new()
        .prefix("search-firefox-profile-")
        .tempdir()
        .map_err(|e| Error::Env(format!("创建 Firefox profile 失败: {e}")))?;
    std::fs::write(
        dir.path().join("user.js"),
        format!("user_pref(\"marionette.port\", {port});\n"),
    )
    .map_err(|e| Error::Env(format!("写入 profile user.js 失败: {e}")))?;
    Ok(dir)
}

/// Marionette 协议错误 → 领域错误映射（error 对象：`{error, message, stacktrace}`）。
fn marionette_error(err: &Value) -> Error {
    let code = err
        .get("error")
        .and_then(|c| c.as_str())
        .unwrap_or("unknown");
    let message = err
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or_default();
    match code {
        "timeout" | "script timeout" => Error::Timeout(format!("Marionette {code}: {message}")),
        _ => Error::Network(format!("Marionette {code}: {message}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    async fn write_frame_raw(stream: &mut TcpStream, v: &Value) {
        let bytes = serde_json::to_vec(v).unwrap();
        stream
            .write_all(format!("{}:", bytes.len()).as_bytes())
            .await
            .unwrap();
        stream.write_all(&bytes).await.unwrap();
    }

    async fn read_frame_raw(stream: &mut TcpStream) -> Value {
        let mut len_bytes = Vec::new();
        loop {
            let b = stream.read_u8().await.unwrap();
            if b == b':' {
                break;
            }
            len_bytes.push(b);
        }
        let len: usize = String::from_utf8(len_bytes).unwrap().parse().unwrap();
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await.unwrap();
        serde_json::from_slice(&buf).unwrap()
    }

    /// 握手 + 命令/响应 + 事件帧穿插 + id 匹配。
    #[tokio::test]
    async fn transport_handles_hello_and_id_matching() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // 握手 hello
            write_frame_raw(
                &mut sock,
                &json!({"applicationType": "gecko", "marionetteProtocol": 3}),
            )
            .await;
            // 命令 1：NewSession（数组格式 [0, id, command, params]）
            let cmd = read_frame_raw(&mut sock).await;
            assert_eq!(cmd[2], "WebDriver:NewSession");
            write_frame_raw(
                &mut sock,
                &json!([1, cmd[1].clone(), Value::Null, {"sessionId": "s1"}]),
            )
            .await;
            // 命令 2：GetPageSource，响应前插入事件帧干扰
            let cmd2 = read_frame_raw(&mut sock).await;
            assert_eq!(cmd2[2], "WebDriver:GetPageSource");
            write_frame_raw(&mut sock, &json!([2, 0, "Marionette:Quit", {}])).await;
            write_frame_raw(
                &mut sock,
                &json!([1, cmd2[1].clone(), Value::Null, {"value": "<html>hi</html>"}]),
            )
            .await;
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        let mut t = MarionetteTransport::connect(stream).await.unwrap();
        let r = t
            .send("WebDriver:NewSession", json!({"capabilities": {}}))
            .await
            .unwrap();
        assert_eq!(r["sessionId"], "s1");
        let r2 = t.send("WebDriver:GetPageSource", json!({})).await.unwrap();
        assert_eq!(r2["value"], "<html>hi</html>");
    }

    /// 协议错误 → Error::Network。
    #[tokio::test]
    async fn transport_surfaces_marionette_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            write_frame_raw(
                &mut sock,
                &json!({"applicationType": "gecko", "marionetteProtocol": 3}),
            )
            .await;
            let cmd = read_frame_raw(&mut sock).await;
            write_frame_raw(
                &mut sock,
                &json!([1, cmd[1].clone(), {"error": "invalid argument", "message": "bad url", "stacktrace": ""}, Value::Null]),
            )
            .await;
        });
        let stream = TcpStream::connect(addr).await.unwrap();
        let mut t = MarionetteTransport::connect(stream).await.unwrap();
        let err = t
            .send("WebDriver:Navigate", json!({"url": "::"}))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Network(_)));
    }

    /// 非 hello 首帧 → 握手失败。
    #[tokio::test]
    async fn connect_rejects_non_hello_frame() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            write_frame_raw(&mut sock, &json!([1, 0, Value::Null, Value::Null])).await;
        });
        let stream = TcpStream::connect(addr).await.unwrap();
        let err = MarionetteTransport::connect(stream).await.unwrap_err();
        assert!(matches!(err, Error::Network(_)));
    }

    /// 帧编解码 roundtrip。
    #[tokio::test]
    async fn frame_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            write_frame_raw(&mut sock, &json!([1, 1, Value::Null, Value::Null])).await;
        });
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let v = read_frame(&mut stream).await.unwrap();
        assert_eq!(v[0], 1);
    }
}
