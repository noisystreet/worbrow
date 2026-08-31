//! 静态 SERP 的 HTTP GET（ADR-011）：供 `prefer_http_html` 引擎在浏览器之前直抓。
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

/// 可注入的 HTML GET（测试替身；生产用 [`ReqwestHtmlGet`]）。
#[async_trait]
pub(crate) trait HtmlGet: Send + Sync {
    async fn get(&self, url: &Url, timeout: Duration) -> Result<String, Error>;
}

/// HTTP GET 客户端（连接池复用；单次请求仍带超时）。
///
/// 构造时按代理配置取用**缓存的** reqwest 客户端：`proxy = Some(..)` 时经代理直抓
/// （ADR-013），`None` 时直连（reqwest 默认仍读取 `HTTP_PROXY`/`HTTPS_PROXY` 等系统代理
/// 环境变量）。同一代理配置复用同一客户端（连接池 + TLS 会话缓存跨请求生效）。
pub(crate) struct ReqwestHtmlGet(Arc<reqwest::Client>);

impl ReqwestHtmlGet {
    /// 取用按代理缓存的客户端；代理为 http/https URL（非法时告警并忽略，保持"尽力"语义）。
    pub fn new(proxy: Option<&str>) -> Self {
        Self(client_for(proxy))
    }
}

#[async_trait]
impl HtmlGet for ReqwestHtmlGet {
    async fn get(&self, url: &Url, timeout: Duration) -> Result<String, Error> {
        get_with(&self.0, url, timeout).await
    }
}

/// 按代理配置缓存的客户端注册表：避免每次搜索重建 Client（重建会丢连接池/TLS 会话）。
/// 代理是进程级配置（CLI/MCP 启动参数），数量有限，无容量淘汰需求。
fn client_for(proxy: Option<&str>) -> Arc<reqwest::Client> {
    static CLIENTS: OnceLock<Mutex<HashMap<Option<String>, Arc<reqwest::Client>>>> =
        OnceLock::new();
    let key = proxy.map(str::to_owned);
    match CLIENTS.get_or_init(|| Mutex::new(HashMap::new())).lock() {
        Ok(mut map) => map
            .entry(key.clone())
            .or_insert_with(|| Arc::new(build_client(key.as_deref())))
            .clone(),
        // 锁中毒兜底：退化为临时客户端（不阻塞搜索，仅丢失连接复用）
        Err(_) => Arc::new(build_client(proxy)),
    }
}

fn build_client(proxy: Option<&str>) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(10));
    if let Some(p) = proxy {
        match reqwest::Proxy::all(p) {
            Ok(proxy) => builder = builder.proxy(proxy),
            Err(e) => {
                tracing::warn!(proxy = p, "ignoring invalid proxy for HTTP SERP: {e}")
            }
        }
    }
    builder
        .build()
        .expect("reqwest Client builder should succeed")
}

async fn get_with(http: &reqwest::Client, url: &Url, timeout: Duration) -> Result<String, Error> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::Network(format!(
            "unsupported SERP URL scheme: {}",
            url.scheme()
        )));
    }
    let resp = http
        .get(url.as_str())
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| Error::Network(format!("static HTML GET failed: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Network(format!("static HTML GET HTTP {status}")));
    }
    if let Some(len) = resp.content_length()
        && len > MAX_HTML_BYTES
    {
        return Err(Error::Network(format!(
            "static HTML GET body too large ({len} bytes)"
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Network(format!("static HTML GET body failed: {e}")))?;
    if bytes.len() as u64 > MAX_HTML_BYTES {
        return Err(Error::Network(format!(
            "static HTML GET body too large ({} bytes)",
            bytes.len()
        )));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn direct_client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test client")
    }

    async fn get_direct(url: &Url) -> Result<String, Error> {
        get_with(&direct_client(), url, Duration::from_secs(2)).await
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
        let err = ReqwestHtmlGet::new(None)
            .get(&url, Duration::from_secs(1))
            .await
            .expect_err("file 应拒绝");
        assert!(matches!(err, Error::Network(_)));
    }

    /// 配置代理后请求应**经代理**发出：假代理监听本机端口并记录请求行——
    /// reqwest 对 http 代理发 absolute-form（`GET http://…`）而非 origin-form。
    #[tokio::test]
    async fn get_html_uses_proxy_when_configured() {
        use std::sync::Arc as StdArc;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let seen = StdArc::new(std::sync::Mutex::new(None::<String>));
        let seen_task = StdArc::clone(&seen);
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut buf = vec![0u8; 1024];
            let n = sock.read(&mut buf).await.expect("read");
            *seen_task.lock().expect("lock") =
                Some(String::from_utf8_lossy(&buf[..n]).into_owned());
            let body = "<html>via proxy</html>";
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(format!("{header}{body}").as_bytes()).await;
        });

        let proxy = format!("http://{addr}");
        let html = ReqwestHtmlGet::new(Some(&proxy))
            .get(
                &Url::parse("http://example.com/html/").expect("url"),
                Duration::from_secs(2),
            )
            .await
            .expect("经代理 GET 应成功");
        assert!(html.contains("via proxy"));
        let req = seen.lock().expect("lock").clone().expect("代理应收到请求");
        assert!(
            req.starts_with("GET http://"),
            "http 代理请求应为 absolute-form: {req}"
        );
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
