//! 浏览器后端：自研 CDP（Chrome/Edge）与 Marionette（Firefox）双协议（docs/adr/0002-browser-driver-protocols.md）。
//!
//! Marionette 后端（Firefox）V1 已实现（TCP 帧协议，见 marionette.rs）；CDP 后端（Chrome/Edge）
//! V1 已实现（WebSocket，复用 `jsonrpc` 消息类型，见 cdp.rs）。两个后端共用二进制发现
//! `discovery`；`fake` 供测试。
//!
//! 公开面仅 `resolve`（后端注册表）；具体驱动实现（cdp/marionette/fake）为内部细节，
//! 不属稳定 API（ADR-006）。

mod cdp;
mod discovery;
mod fake;
mod jsonrpc;
mod marionette;
mod pool;

use crate::domain::BrowserKind;
use crate::error::Error;
use crate::ports::BrowserDriver;
use fake::FakeDriver;

/// 二进制发现（包内自检使用，如 `DoctorReport`；不对外，ADR-006）。
pub(crate) use discovery::{browser_major_version, find_browser};

/// 会话池（MCP 长驻进程内复用浏览器；drivers 内部服务，供 mcp 与冒烟测试使用，
/// 不 re-export 到 crate 根，不属稳定 API（ADR-006））。
pub use pool::{SessionGuard, SessionPool};

/// 后端注册表：`--browser` 参数值 → `Box<dyn BrowserDriver>`（无代理直连/系统代理）。
///
/// Firefox（Marionette）与 Chrome（CDP）V1 均已实现（design.md §6.5 / §10.2）。
pub async fn resolve(kind: BrowserKind) -> Result<Box<dyn BrowserDriver>, Error> {
    resolve_with(kind, None).await
}

/// 后端注册表 + HTTP/HTTPS 代理（ADR-013）：`proxy` 传递到浏览器启动参数
/// （CDP `--proxy-server` / Marionette `network.proxy.*`）。非法代理 URL → `Error::Cli`。
pub async fn resolve_with(
    kind: BrowserKind,
    proxy: Option<&str>,
) -> Result<Box<dyn BrowserDriver>, Error> {
    if let Some(p) = proxy {
        validate_proxy(p)?;
    }
    match kind {
        // fake：冒烟/测试用，返回可解析的模拟结果页（SMOKE_HTML），非空页面
        BrowserKind::Fake => Ok(Box::new(FakeDriver::with_html(fake::SMOKE_HTML))),
        BrowserKind::Chrome => cdp::CdpDriver::spawn(proxy).await,
        BrowserKind::Firefox => marionette::MarionetteDriver::spawn(proxy).await,
    }
}

/// 校验 `--proxy` 参数：http/https scheme + 非空 host，且**不含凭据/路径**（`Error::Cli`，
/// exit 2）。凭据会泄漏到浏览器进程命令行且三处行为不一致（ADR-013 评审）；path/query
/// 会被 Chrome `--proxy-server` 误解析。MCP 启动期（`serve_stdio`）同样调用。
pub(crate) fn validate_proxy(proxy: &str) -> Result<(), Error> {
    let url = url::Url::parse(proxy).map_err(|e| {
        Error::Cli(format!(
            "invalid proxy URL '{proxy}': {e} (expected http://host:port or https://host:port)"
        ))
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::Cli(format!(
            "unsupported proxy scheme '{}' (expected http or https): {proxy}",
            url.scheme()
        )));
    }
    if url.host_str().is_none() {
        return Err(Error::Cli(format!(
            "proxy URL missing host: {proxy} (expected http://host:port)"
        )));
    }
    if !url.username().is_empty() {
        return Err(Error::Cli(format!(
            "proxy URL must not contain credentials 'user:pass@': {proxy} (configure auth in the proxy itself)"
        )));
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(Error::Cli(format!(
            "proxy URL must not contain a path/query/fragment: {proxy} (expected http://host:port)"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_proxy_accepts_http_and_https() {
        assert!(validate_proxy("http://127.0.0.1:7890").is_ok());
        assert!(validate_proxy("https://proxy.example.com:8443").is_ok());
        assert!(validate_proxy("http://localhost:8080").is_ok());
    }

    #[test]
    // 测试断言序列（assert! 宏展开）非控制流复杂度，豁免门禁；生产代码仍严格 ≤10
    #[allow(clippy::cognitive_complexity)]
    fn validate_proxy_rejects_bad_input() {
        assert!(matches!(
            validate_proxy("ftp://proxy.example.com"),
            Err(Error::Cli(_))
        ));
        assert!(matches!(validate_proxy("not a url"), Err(Error::Cli(_))));
        assert!(matches!(validate_proxy("http://"), Err(Error::Cli(_))));
        assert!(matches!(validate_proxy(""), Err(Error::Cli(_))));
        // 凭据/路径会泄漏到浏览器 argv 或被 Chrome 误解析 → 拒绝
        assert!(matches!(
            validate_proxy("http://user:pass@proxy.example.com:8080"),
            Err(Error::Cli(_))
        ));
        assert!(matches!(
            validate_proxy("http://proxy.example.com:8080/path"),
            Err(Error::Cli(_))
        ));
        assert!(matches!(
            validate_proxy("http://proxy.example.com:8080?x=1"),
            Err(Error::Cli(_))
        ));
    }

    #[tokio::test]
    async fn resolve_with_invalid_proxy_fails_before_browser() {
        // 非法代理 → Cli 错误（不启动浏览器；Fake 分支同样被前置校验拦截）
        // Box<dyn BrowserDriver> 非 Debug，用 match 断言而非 expect_err
        let err = match resolve_with(BrowserKind::Fake, Some("socks5://127.0.0.1:1080")).await {
            Err(e) => e,
            Ok(_) => panic!("socks scheme 应被拒绝"),
        };
        assert!(matches!(err, Error::Cli(_)));
    }
}
