//! 浏览器后端：自研 CDP（Chrome/Edge）与 Marionette（Firefox）双协议（docs/adr/0002-browser-driver-protocols.md）。
//!
//! 两个后端共用 `jsonrpc` 消息框架；`fake` 供测试与 CI。

pub mod cdp;
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
/// 骨架阶段仅 `Fake` 可用；CDP / Marionette 为 V1 待实现桩（design.md §6.5 / §10.2）。
pub fn resolve(kind: BrowserKind) -> Result<Box<dyn BrowserDriver>, Error> {
    match kind {
        BrowserKind::Fake => Ok(Box::<FakeDriver>::default()),
        BrowserKind::Chrome => cdp::CdpDriver::spawn(),
        BrowserKind::Firefox => marionette::MarionetteDriver::spawn(),
    }
}
