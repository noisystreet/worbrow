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
//!
//! # 快速开始
//!
//! ```rust
//! use worbrow::{BrowserKind, Config, search};
//!
//! // fake 浏览器后端无需真实浏览器，便于在测试/示例中快速验证全链路
//! let outcome = search(Config::new("rust async", "bing", BrowserKind::Fake)).unwrap();
//! assert!(!outcome.results.is_empty());
//! ```
//!
//! # 入口选择（tokio runtime）
//!
//! 两个入口，按调用方是否已处于 tokio runtime 选择，**避免嵌套 runtime panic**：
//!
//! - [`search`]（同步）：无 runtime 上下文时用——`main`/CLI/脚本/`spawn_blocking` 闭包。
//!   内部自建 runtime，**勿在 async 上下文调用**
//! - [`run`]（async）：已有 runtime 时用——MCP handler、`#[tokio::main]`、`#[tokio::test]`，
//!   直接 `await` 复用外部 runtime
//!
//! async 内需要同步阻塞等待结果时：
//!
//! ```rust,no_run
//! use worbrow::{BrowserKind, Config, run};
//! # let config = Config::new("q", "bing", BrowserKind::Fake);
//! let handle = tokio::runtime::Handle::current();
//! let outcome = tokio::task::block_in_place(|| handle.block_on(run(config))).unwrap();
//! ```
//!
//! 自定义引擎（无需复制 `run` 编排）：实现 [`SearchProvider`] 并经
//! [`Config::with_provider`] 注入。更多示例见 `examples/`。

pub mod app;
pub(crate) mod domain;
pub use app::{BackendStatus, Config, DoctorReport, Outcome, run, search};
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
pub use output::{ErrorBody, ErrorPayload, SCHEMA_VERSION, SuccessPayload};
pub mod ports;
pub use ports::{BrowserDriver, SearchProvider};
