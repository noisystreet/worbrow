//! # rplay-search
//!
//! Agent 搜索 CLI 的库核心。驱动本机 headless 浏览器（Chrome/Edge/Firefox）在通用
//! 搜索引擎上执行搜索，输出稳定的 JSON 契约。
//!
//! 分层（design.md §5）：`cli → app → domain/ports ← adapters(drivers/engines)`。
//! 依赖方向只允许指向内层，禁止反向。

pub mod app;
pub mod cli;
pub mod domain;
pub mod drivers;
pub mod engines;
pub mod error;
pub mod extract;
pub mod output;
pub mod ports;
