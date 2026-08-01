//! 领域模型：纯数据，不依赖框架/IO 细节（design.md §6.3）。

use std::fmt;

use chrono::{DateTime, Utc};
use serde::Serialize;

/// 默认引擎（CLI 与 MCP 共用，design.md §6.1）。
pub const DEFAULT_ENGINE: &str = "bing";
/// 默认浏览器后端（CLI 与 MCP 共用）。
pub const DEFAULT_BROWSER: &str = "firefox";
/// 默认返回条数上限。
pub const DEFAULT_MAX_RESULTS: usize = 10;
/// 默认全流程硬超时（秒）。
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// 一次搜索请求。
#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub text: String,
    pub max_results: usize,
}

/// 单条搜索结果（DTO，跨边界唯一传递形态，禁止泄漏 DOM 结构）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchResult {
    pub rank: usize,
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// 引擎侧可上报的结构化异常：不为空即结果不可信（design.md §7.1）。
///
/// 与 [`crate::error::EngineFailure`] 语义不同：本类型是搜索结果元信息
/// （`meta.engine_error`，随成功包返回）；`EngineFailure` 是适配器解析失败时
/// 构造的错误（经 [`crate::error::Error::Engine`] 映射为 exit 4）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EngineError {
    pub code: String,
    pub message: String,
}

/// 搜索元信息。
#[derive(Debug, Clone, Serialize)]
pub struct SearchMeta {
    pub engine: &'static str,
    pub started_at: DateTime<Utc>,
    pub elapsed_ms: u64,
    pub result_count: usize,
    /// 结果数 < 3 时置 true，提示 agent 结果不可靠（design.md §10.4）。
    pub low_yield: bool,
    pub captcha: bool,
    pub engine_error: Option<EngineError>,
}

/// 浏览器后端标识（配置概念，供 CLI/MCP/库调用方选择驱动后端；零依赖纯枚举）。
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

impl BrowserKind {
    /// 从 CLI/MCP 参数值解析（大小写不敏感；`chrome`/`edge`/`chromium` 统一为 Chrome）。
    /// 单一映射源：`cli::BrowserArg` 与 `mcp::parse_browser` 均委托此实现。
    pub fn from_arg(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "chrome" | "edge" | "chromium" => Some(Self::Chrome),
            "firefox" => Some(Self::Firefox),
            "fake" => Some(Self::Fake),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_kind_from_arg_normalizes_and_aliases() {
        assert_eq!(BrowserKind::from_arg("chrome"), Some(BrowserKind::Chrome));
        assert_eq!(BrowserKind::from_arg("Edge"), Some(BrowserKind::Chrome));
        assert_eq!(
            BrowserKind::from_arg(" chromium "),
            Some(BrowserKind::Chrome)
        );
        assert_eq!(BrowserKind::from_arg("firefox"), Some(BrowserKind::Firefox));
        assert_eq!(BrowserKind::from_arg("fake"), Some(BrowserKind::Fake));
        assert_eq!(BrowserKind::from_arg("lynx"), None);
        assert_eq!(BrowserKind::from_arg(""), None);
    }
}
