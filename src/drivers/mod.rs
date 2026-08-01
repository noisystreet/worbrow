//! 浏览器后端：自研 CDP（Chrome/Edge）与 Marionette（Firefox）双协议（docs/adr/0002-browser-driver-protocols.md）。
//!
//! Marionette 后端（Firefox）V1 已实现（TCP 帧协议，见 marionette.rs）；CDP 后端（Chrome/Edge）
//! V1 已实现（WebSocket，复用 `jsonrpc` 消息类型，见 cdp.rs）。两个后端共用二进制发现
//! `discovery`；`fake` 供测试。

pub mod cdp;
pub mod discovery;
pub mod fake;
pub mod jsonrpc;
pub mod marionette;

use std::fmt;

use crate::error::Error;
use crate::ports::BrowserDriver;
use fake::FakeDriver;

/// 浏览器后端标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserKind {
    /// Chrome / Edge / Chromium（CDP）
    Chrome,
    /// Firefox（Marionette）
    Firefox,
    /// 测试用假驱动（CLI 不暴露）
    Fake,
}

impl fmt::Display for BrowserKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BrowserKind::Chrome => write!(f, "chrome"),
            BrowserKind::Firefox => write!(f, "firefox"),
            BrowserKind::Fake => write!(f, "fake"),
        }
    }
}

/// 后端注册表：`--browser` 参数值 → `Box<dyn BrowserDriver>`。
///
/// Firefox（Marionette）与 Chrome（CDP）V1 均已实现（design.md §6.5 / §10.2）。
pub async fn resolve(kind: BrowserKind) -> Result<Box<dyn BrowserDriver>, Error> {
    match kind {
        // fake：冒烟/测试用，返回可解析的模拟结果页（SMOKE_HTML），非空页面
        BrowserKind::Fake => Ok(Box::new(FakeDriver::with_html(fake::SMOKE_HTML))),
        BrowserKind::Chrome => cdp::CdpDriver::spawn().await,
        BrowserKind::Firefox => marionette::MarionetteDriver::spawn().await,
    }
}
