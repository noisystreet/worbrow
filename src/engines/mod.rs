//! 搜索引擎适配器（design.md §6.5 / ADR-003）。
//!
//! 每个引擎两个关注点：URL 直访模板 + 解析选择器。新增引擎 = 新增一个文件 + 在
//! `resolve` 注册一行。引擎 HTML 改版是常态：解析失败经 `EngineFailure` 上报，
//! 不破坏输出 schema。

pub mod duckduckgo;

use crate::error::Error;
use crate::ports::SearchProvider;
use duckduckgo::DuckDuckGo;

/// 可用引擎列表（`search list` 输出）。
pub const AVAILABLE: &[&str] = &["duckduckgo"];

/// 引擎注册表：名称 → `Box<dyn SearchProvider>`。
pub fn resolve(name: &str) -> Result<Box<dyn SearchProvider>, Error> {
    match name {
        "duckduckgo" => Ok(Box::new(DuckDuckGo)),
        other => Err(Error::Cli(format!(
            "未知引擎: {other}（可用: {}）",
            AVAILABLE.join(", ")
        ))),
    }
}
