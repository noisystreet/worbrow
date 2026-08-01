//! # worbrow
//!
//! Agent 搜索 CLI 的库核心。驱动本机 headless 浏览器（Chrome/Edge/Firefox）在通用
//! 搜索引擎上执行搜索，输出稳定的 JSON 契约。
//!
//! 分层（design.md §5）：`cli → app → domain/ports ← adapters(drivers/engines)`。
//! 依赖方向只允许指向内层，禁止反向。
//!
//! 公开面（semver 稳定）：`app`/`cli`/`drivers`/`engines`/`error`/`mcp`/`output`/`ports`
//! 为包内公共接口（bin 与集成测试使用）；`domain`/`extract` 为内部实现
//! （`pub(crate)`），domain 类型经根 re-export 暴露，避免过早固化内部路径。

pub mod app;
pub mod cli;
pub(crate) mod domain;
pub use domain::{
    DEFAULT_BROWSER, DEFAULT_ENGINE, DEFAULT_MAX_RESULTS, DEFAULT_TIMEOUT_SECS, SearchMeta,
    SearchQuery, SearchResult,
};
pub mod drivers;
pub mod engines;
pub mod error;
pub(crate) mod extract;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod output;
pub mod ports;
