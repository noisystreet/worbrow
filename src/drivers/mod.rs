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
