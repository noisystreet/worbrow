//! 自研 Marionette 客户端后端（Firefox）。V1 待实现（docs/adr/0002-browser-driver-protocols.md / design.md §6.5）。
//!
//! 协议命令登记：
//! - `WebDriver:NewSession`：建会话
//! - `WebDriver:Navigate`：导航
//! - `WebDriver:ExecuteScript`：取 HTML、轮询 `document.readyState`（无原生 load 事件）
//! - `WebDriver:TakeScreenshot`：调试截图
//!
//! 启动/并发（design.md §10.1）：`firefox -marionette -headless`，每实例独立临时
//! profile + `user.js` 写入随机 `marionette.port` 以规避 2828 端口冲突。

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use url::Url;

use crate::error::Error;
use crate::ports::BrowserDriver;

/// Marionette 后端占位。V1 实现基于 `drivers::jsonrpc` 的 WebSocket 传输层。
#[derive(Debug, Default)]
pub struct MarionetteDriver;

impl MarionetteDriver {
    pub fn spawn() -> Result<Box<dyn BrowserDriver>, Error> {
        Err(Error::NotImplemented(
            "Marionette 后端待实现（V1，见 docs/adr/0002-browser-driver-protocols.md）".into(),
        ))
    }
}

#[async_trait]
impl BrowserDriver for MarionetteDriver {
    async fn navigate(&mut self, _url: Url) -> Result<(), Error> {
        Err(Error::NotImplemented("marionette: navigate".into()))
    }

    async fn wait_for(&mut self, _selector: &str, _timeout: Duration) -> Result<(), Error> {
        Err(Error::NotImplemented("marionette: wait_for".into()))
    }

    async fn html(&self) -> Result<String, Error> {
        Err(Error::NotImplemented("marionette: html".into()))
    }

    async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
        Err(Error::NotImplemented("marionette: eval".into()))
    }

    async fn screenshot(&mut self, _path: &Path) -> Result<(), Error> {
        Err(Error::NotImplemented("marionette: screenshot".into()))
    }
}
