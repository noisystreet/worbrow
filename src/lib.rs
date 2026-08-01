//! # worbrow
//!
//! Agent 搜索 CLI 的库核心。驱动本机 headless 浏览器（Chrome/Edge/Firefox）在通用
//! 搜索引擎上执行搜索，输出稳定的 JSON 契约。
//!
//! 分层（design.md §5）：`cli → app → domain/ports ← adapters(drivers/engines)`。
//! 依赖方向只允许指向内层，禁止反向。
//!
//! 公开面（semver 稳定，ADR-006）：顶层 re-export（`Config`/`BrowserKind`/`Outcome`/
//! `DoctorReport`/`Error`/`DEFAULT_*`/`SearchQuery`/`SearchResult`/`SearchMeta`/
//! `EngineError`）为库消费者入口；`app`/`drivers`/`engines`/`error`/`mcp`/`output`/
//! `ports` 为包内服务模块（`resolve` 等为内部服务入口，bin 与集成测试使用）；
//! `domain`/`extract` 与适配器实现（cdp/marionette/fake/bing 等）为内部细节，
//! 不属稳定 API。

pub mod app;
pub(crate) mod domain;
/// 同步便捷入口（等价于 `run_sync`，主用例动词化）。
pub use app::run_sync as search;
pub use app::{BackendStatus, Config, DoctorReport, Outcome, run, run_sync};
pub use domain::{
    BrowserKind, DEFAULT_BROWSER, DEFAULT_ENGINE, DEFAULT_MAX_RESULTS, DEFAULT_TIMEOUT_SECS,
    EngineError, SearchMeta, SearchQuery, SearchResult,
};
pub mod drivers;
pub mod engines;
pub mod error;
pub use error::Error;
pub(crate) mod extract;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod output;
pub mod ports;
