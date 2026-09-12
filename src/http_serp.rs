//! 静态 SERP 的 HTTP GET（ADR-011/015）：供 `prefer_http_html` 引擎在浏览器之前直抓。
//!
//! 仅 http/https、有限重定向、响应体上限；失败由调用方回退浏览器。`domain` 不依赖本模块。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use url::Url;

use crate::error::Error;

/// 响应体上限（字节）：SERP HTML 远小于此；防止异常/恶意 Content-Length 撑爆内存。
pub(crate) const MAX_HTML_BYTES: u64 = 2 * 1024 * 1024;

/// 浏览器形态 UA：与 curl `-A Mozilla` 同类，避免被当成脚本探针。
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";

/// 可注入的 HTML GET（测试替身；生产用 [`UreqHtmlGet`]）。
#[async_trait]
pub(crate) trait HtmlGet: Send + Sync {
    async fn get(&self, url: &Url, timeout: Duration) -> Result<String, Error>;
}

/// HTTP GET 客户端（ureq 阻塞 I/O，ADR-015；连接池复用）。
///
/// 构造时按代理配置取用**缓存的** agent：`proxy = Some(..)` 时经代理直抓（ADR-013），
/// `None` 时直连并读取 `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` 系统代理环境变量
/// （`NO_PROXY` 按域豁免随之生效）。同一代理配置复用同一 agent
/// （连接池 + TLS 会话缓存跨请求生效）。
pub(crate) struct UreqHtmlGet(Arc<ureq::Agent>);

impl UreqHtmlGet {
    /// 取用按代理缓存的 agent；代理为 http/https URL（非法时告警并忽略，保持"尽力"语义）。
    pub fn new(proxy: Option<&str>) -> Self {
        Self(agent_for(proxy))
    }
}

#[async_trait]
impl HtmlGet for UreqHtmlGet {
    async fn get(&self, url: &Url, timeout: Duration) -> Result<String, Error> {
        // ureq 是阻塞 I/O（ADR-015）：整次请求（连接/重定向/读体）放入 tokio 阻塞线程池，
        // 不占用运行时工作线程；超时按请求注入（timeout_global），到点即失败。
        let agent = Arc::clone(&self.0);
        let url = url.clone();
        tokio::task::spawn_blocking(move || get_blocking(&agent, &url, timeout))
            .await
            .map_err(|e| Error::Network(format!("static HTML GET task failed: {e}")))?
    }
}

/// 按代理配置缓存的 agent 注册表：避免每次搜索重建（重建会丢连接池/TLS 会话）。
/// 代理是进程级配置（CLI/MCP 启动参数），数量有限，无容量淘汰需求。
fn agent_for(proxy: Option<&str>) -> Arc<ureq::Agent> {
    static AGENTS: OnceLock<Mutex<HashMap<Option<String>, Arc<ureq::Agent>>>> = OnceLock::new();
    let key = proxy.map(str::to_owned);
    match AGENTS.get_or_init(|| Mutex::new(HashMap::new())).lock() {
        Ok(mut map) => map
            .entry(key.clone())
            .or_insert_with(|| Arc::new(build_agent(key.as_deref())))
            .clone(),
        // 锁中毒兜底：退化为临时 agent（不阻塞搜索，仅丢失连接复用）
        Err(_) => Arc::new(build_agent(proxy)),
    }
}

fn build_agent(proxy: Option<&str>) -> ureq::Agent {
    // `None` 时读环境代理（对齐原 reqwest 默认行为，ADR-013）；显式代理非法时告警并
    // 退回环境代理，保持"尽力"语义。
    let proxy = match proxy {
        Some(p) => match ureq::Proxy::new(p) {
            Ok(proxy) => Some(proxy),
            Err(e) => {
                tracing::warn!(proxy = p, "ignoring invalid proxy for HTTP SERP: {e}");
                ureq::Proxy::try_from_env()
            }
        },
        None => ureq::Proxy::try_from_env(),
    };
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        // 重定向上限 10 与原 reqwest `Policy::limited(10)` 对齐（ADR-011）。
        .max_redirects(10)
        .proxy(proxy)
        .build()
        .new_agent()
}

/// 阻塞执行一次 GET（仅在 `spawn_blocking` 内调用）。
fn get_blocking(agent: &ureq::Agent, url: &Url, timeout: Duration) -> Result<String, Error> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::Network(format!(
            "unsupported SERP URL scheme: {}",
            url.scheme()
        )));
    }
    let mut resp = agent
        .get(url.as_str())
        // 每次请求覆盖超时：端到端（连接 + 重定向 + 读体）与原 reqwest 单请求超时对齐
        // （超时取自页面等待预算，ADR-011）。
        .config()
        .timeout_global(Some(timeout))
        .build()
        .call()
        .map_err(|e| Error::Network(format!("static HTML GET failed: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Network(format!("static HTML GET HTTP {status}")));
    }
    if let Some(len) = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        && len > MAX_HTML_BYTES
    {
        return Err(Error::Network(format!(
            "static HTML GET body too large ({len} bytes)"
        )));
    }
    // limit 超限即报错：覆盖 Content-Length 缺失/不符与 chunked 响应的体积上限（ADR-011）；
    // lossy_utf8 与原 String::from_utf8_lossy 语义对齐（非法 UTF-8 不致命）。
    resp.body_mut()
        .with_config()
        .limit(MAX_HTML_BYTES)
        .lossy_utf8(true)
        .read_to_string()
        .map_err(|e| Error::Network(format!("static HTML GET body failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// 直连（不读环境代理）的测试 agent。
    fn direct_agent() -> ureq::Agent {
        ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .max_redirects(10)
            .proxy(None)
            .build()
            .new_agent()
    }

    async fn get_direct(url: &Url) -> Result<String, Error> {
        let agent = direct_agent();
        let url = url.clone();
        tokio::task::spawn_blocking(move || get_blocking(&agent, &url, Duration::from_secs(2)))
            .await
            .expect("blocking task panicked")
    }

    async fn serve_once(status: &str, body: &str) -> Url {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let header = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let payload = format!("{header}{body}");
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut buf = vec![0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let _ = sock.write_all(payload.as_bytes()).await;
        });
        Url::parse(&format!("http://{addr}/html/?q=rust")).expect("url")
    }

    #[tokio::test]
    async fn get_html_reads_http_body() {
        let url = serve_once("200 OK", "<html>ok</html>").await;
        let html = get_direct(&url).await.expect("GET 应成功");
        assert!(html.contains("ok"));
    }

    #[tokio::test]
    async fn get_html_rejects_http_error_status() {
        let url = serve_once("503 Service Unavailable", "nope").await;
        let err = get_direct(&url).await.expect_err("非 2xx 应失败");
        assert!(matches!(err, Error::Network(_)));
    }

    #[tokio::test]
    async fn get_html_rejects_non_http_scheme() {
        let url = Url::parse("file:///tmp/serp.html").expect("url");
        let err = UreqHtmlGet::new(None)
            .get(&url, Duration::from_secs(1))
            .await
            .expect_err("file 应拒绝");
        assert!(matches!(err, Error::Network(_)));
    }

    /// 配置代理后请求应**经代理**发出：假代理按 ureq 的 CONNECT 隧道语义工作
    /// （对 http/https 代理一律先 CONNECT，reqwest 的 absolute-form 直发不存在于 ureq）：
    /// 先应答 `CONNECT host:port`，再在隧道内应答 origin-form 的 `GET /path`。
    #[tokio::test]
    async fn get_html_uses_proxy_when_configured() {
        use std::sync::Arc as StdArc;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let seen = StdArc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen_task = StdArc::clone(&seen);
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            // 第一阶段：应答 CONNECT，建立隧道（CONNECT 响应无 body）
            let connect_req = read_request_head(&mut sock).await;
            seen_task.lock().expect("lock").push(connect_req);
            let _ = sock
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await;
            // 第二阶段：隧道内的 origin-form GET
            let get_req = read_request_head(&mut sock).await;
            seen_task.lock().expect("lock").push(get_req);
            let body = "<html>via proxy</html>";
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(format!("{header}{body}").as_bytes()).await;
        });

        let proxy = format!("http://{addr}");
        let html = UreqHtmlGet::new(Some(&proxy))
            .get(
                &Url::parse("http://example.com/html/").expect("url"),
                Duration::from_secs(2),
            )
            .await
            .expect("经代理 GET 应成功");
        assert!(html.contains("via proxy"));
        let reqs = seen.lock().expect("lock").clone();
        let [connect_req, get_req] = reqs.try_into().expect("假代理应恰收到两个请求");
        assert!(
            connect_req.starts_with("CONNECT example.com:80 "),
            "http 代理应先建立 CONNECT 隧道: {connect_req}"
        );
        assert!(
            get_req.starts_with("GET /html/"),
            "隧道内应为 origin-form GET: {get_req}"
        );
        assert!(
            get_req.to_ascii_lowercase().contains("host: example.com"),
            "隧道内 GET 应保留目标 Host: {get_req}"
        );
    }

    /// 读到请求头结束（`\r\n\r\n`）：请求可能分多个 TCP 段到达，单次 read 可能只拿到半包，
    /// 未读完就关闭连接会向对端回 RST。
    async fn read_request_head(sock: &mut tokio::net::TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = sock.read(&mut chunk).await.expect("read");
            assert!(n > 0, "对端在请求头读取完成前关闭了连接");
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                return String::from_utf8_lossy(&buf).into_owned();
            }
        }
    }

    #[tokio::test]
    async fn get_html_rejects_oversize_content_length() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut buf = vec![0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_HTML_BYTES + 1
            );
            let _ = sock.write_all(hdr.as_bytes()).await;
        });
        let url = Url::parse(&format!("http://{addr}/")).expect("url");
        let err = get_direct(&url)
            .await
            .expect_err("过大 Content-Length 应拒绝");
        assert!(matches!(err, Error::Network(_)));
    }
}
