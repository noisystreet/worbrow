//! 领域模型：纯数据，不依赖框架/IO 细节（design.md §6.3）。

use chrono::{DateTime, Utc};
use serde::Serialize;

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
