//! 自研 CDP 客户端后端（Chrome/Edge）。V1 待实现（docs/adr/0002-browser-driver-protocols.md / design.md §6.5）。
//!
//! 协议命令登记：
//! - `Target.attachToTarget`：绑定页面会话
//! - `Page.navigate`：导航
//! - `Runtime.evaluate`：取 HTML、轮询 `document.readyState`、验证码判定
//! - `Page.captureScreenshot`：调试截图
//!
//! 启动/发现（design.md §10.1/§10.2）：
//! `chrome --headless=new --remote-debugging-port=<动态端口> --no-sandbox`，再
//! `GET /json/version` 取 `webSocketDebuggerUrl`。

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use url::Url;

use crate::error::Error;
use crate::ports::BrowserDriver;

/// CDP 后端占位。V1 实现基于 `drivers::jsonrpc` 的 WebSocket 传输层。
#[derive(Debug, Default)]
pub struct CdpDriver;

impl CdpDriver {
    pub async fn spawn() -> Result<Box<dyn BrowserDriver>, Error> {
        Err(Error::NotImplemented(
            "CDP 后端待实现（V1，见 docs/adr/0002-browser-driver-protocols.md）".into(),
        ))
    }
}

#[async_trait]
impl BrowserDriver for CdpDriver {
    async fn navigate(&mut self, _url: Url) -> Result<(), Error> {
        Err(Error::NotImplemented("cdp: navigate".into()))
    }

    async fn wait_for(&mut self, _selector: &str, _timeout: Duration) -> Result<(), Error> {
        Err(Error::NotImplemented("cdp: wait_for".into()))
    }

    async fn html(&self) -> Result<String, Error> {
        Err(Error::NotImplemented("cdp: html".into()))
    }

    async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
        Err(Error::NotImplemented("cdp: eval".into()))
    }

    async fn screenshot(&mut self, _path: &Path) -> Result<(), Error> {
        Err(Error::NotImplemented("cdp: screenshot".into()))
    }
}
