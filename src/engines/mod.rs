//! 搜索引擎适配器（design.md §6.5 / ADR-003）。
//!
//! 每个引擎两个关注点：URL 直访模板 + 解析选择器。新增引擎 = 新增一个文件 + 在
//! `resolve` 注册一行。引擎 HTML 改版是常态：解析失败经 `EngineFailure` 上报，
//! 不破坏输出 schema。
//!
//! 公开面仅 `resolve`/`AVAILABLE`；具体引擎实现（bing/duckduckgo）为内部细节，
//! 不属稳定 API（ADR-006）。

mod bing;
mod duckduckgo;

use crate::error::Error;
use crate::ports::SearchProvider;
use bing::Bing;
use duckduckgo::DuckDuckGo;

/// 可用引擎列表（`worbrow list` 输出）。
pub const AVAILABLE: &[&str] = &["duckduckgo", "bing"];

/// 引擎注册表：名称 → `Box<dyn SearchProvider>`。
pub fn resolve(name: &str) -> Result<Box<dyn SearchProvider>, Error> {
    match name {
        "duckduckgo" => Ok(Box::new(DuckDuckGo)),
        "bing" => Ok(Box::new(Bing)),
        other => Err(Error::Cli(format!(
            "unknown engine: {other} (available: {})",
            AVAILABLE.join(", ")
        ))),
    }
}
