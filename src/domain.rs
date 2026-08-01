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
    /// 结果语言（引擎相关，如 Bing `setlang`）；`None` = 引擎默认。
    pub lang: Option<String>,
    /// 结果地域/市场（如 Bing `mkt`、DDG `kl`）；`None` = 引擎默认。
    pub region: Option<String>,
    /// 翻页聚合页数（≥1）；1 = 仅首页。
    pub pages: usize,
    /// 时间过滤窗口（引擎相关：Bing `qft`、DDG `df`）；`None` = 不限时间（引擎默认）。
    pub freshness: Option<Freshness>,
    /// 安全搜索级别（Bing `adlt`、DDG `kp`）；`None` = 引擎默认。
    pub safesearch: Option<SafesearchLevel>,
    /// 站点过滤（query 级 `site:` 语法）；`None` = 不限站点。
    pub site: Option<String>,
    /// 文件类型过滤（query 级 `filetype:` 语法）；`None` = 不限类型。
    pub filetype: Option<String>,
}

impl SearchQuery {
    /// 引擎实际发送的查询文本：原 `text` 追加 `site:`/`filetype:`（如有）。
    /// 输出契约的 `query` 字段仍保留原始 text（过滤条件由请求参数表达，
    /// 见 roadmap-search-params.md §3 site/filetype）。
    pub fn engine_text(&self) -> String {
        let mut text = self.text.trim().to_string();
        if let Some(site) = &self.site {
            text.push_str(&format!(" site:{site}"));
        }
        if let Some(filetype) = &self.filetype {
            text.push_str(&format!(" filetype:{filetype}"));
        }
        text
    }
}

/// 单条搜索结果（DTO，跨边界唯一传递形态，禁止泄漏 DOM 结构）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchResult {
    pub rank: usize,
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// 来源域名（URL host，构造时从 url 提取；供 agent 免解析判断可信度）。
    pub domain: String,
    /// 链接是否为 HTTPS（scheme 判定）。
    pub https: bool,
    /// 发布日期（从摘要尽力提取的原始字符串；`None` = 引擎未提供/不可解析，
    /// 格式随引擎变化，不承诺统一）。
    pub published_at: Option<String>,
    /// 是否为广告位结果（Bing 广告容器已被选择器排除恒 false；DDG 广告位标记 true）。
    pub is_ad: bool,
    /// URL 是否已解跳转（uddg/ck-a 展开为真实目标）；`false` = 原样返回
    /// （含 ck/a 解码失败保持链式 URL），agent 据此判断 `url` 可信度。
    pub url_resolved: bool,
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
    /// 实际聚合的页数（schema v1 新增字段，`pages` 请求参数）。
    pub pages: usize,
    /// 结果数 < 3 时置 true，提示 agent 结果不可靠（design.md §10.4）。
    pub low_yield: bool,
    pub captcha: bool,
    pub engine_error: Option<EngineError>,
    /// 引擎降级尝试链（含最终采用者；单引擎时为 `[engine]`，schema v1 新增字段）。
    pub engine_tried: Vec<String>,
    /// 是否命中 MCP 短 TTL 缓存（schema v1 新增字段；CLI 恒 false——缓存仅 MCP 长驻生效）。
    pub cached: bool,
    /// 实际网络重试次数（`--retry` 触发的退避重试数；未配置或无重试为 0）。
    pub retries: usize,
}

/// 浏览器后端标识（配置概念，供 CLI/MCP/库调用方选择驱动后端；零依赖纯枚举）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// 时间过滤窗口（请求参数；`None` = 不限时间，引擎默认）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Freshness {
    Day,
    Week,
    Month,
    Year,
}

impl Freshness {
    /// Bing `qft` 时间窗（秒）：day=86400 / week=604800 / month≈2592000 / year≈31536000。
    pub fn bing_seconds(self) -> u64 {
        match self {
            Self::Day => 86_400,
            Self::Week => 604_800,
            Self::Month => 2_592_000,
            Self::Year => 31_536_000,
        }
    }

    /// DDG `df` 参数（d/w/m/y）。
    pub fn ddg_param(self) -> &'static str {
        match self {
            Self::Day => "d",
            Self::Week => "w",
            Self::Month => "m",
            Self::Year => "y",
        }
    }

    /// 从 CLI/MCP 参数值解析（大小写不敏感，接受全称与单字母缩写）。
    pub fn from_arg(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "day" | "d" => Some(Self::Day),
            "week" | "w" => Some(Self::Week),
            "month" | "m" => Some(Self::Month),
            "year" | "y" => Some(Self::Year),
            _ => None,
        }
    }
}

/// 安全搜索级别（请求参数；`None` = 引擎默认）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SafesearchLevel {
    Off,
    Moderate,
    Strict,
}

impl SafesearchLevel {
    /// Bing `adlt` 参数（仅 off/strict 两级；moderate 映射 strict）。
    pub fn bing_param(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Moderate | Self::Strict => "strict",
        }
    }

    /// DDG `kp` 参数（-1=off / 1=moderate / 2=strict）。
    pub fn ddg_param(self) -> &'static str {
        match self {
            Self::Off => "-1",
            Self::Moderate => "1",
            Self::Strict => "2",
        }
    }

    /// 从 CLI/MCP 参数值解析（大小写不敏感）。
    pub fn from_arg(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "moderate" | "mod" => Some(Self::Moderate),
            "strict" => Some(Self::Strict),
            _ => None,
        }
    }
}

#[cfg(test)]
// 测试断言序列（assert_eq 宏展开）非控制流复杂度，豁免门禁；生产代码仍严格 ≤10
#[allow(clippy::cognitive_complexity)]
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

    #[test]
    fn freshness_from_arg_and_mappings() {
        assert_eq!(Freshness::from_arg("day"), Some(Freshness::Day));
        assert_eq!(Freshness::from_arg("WEEK"), Some(Freshness::Week));
        assert_eq!(Freshness::from_arg("m"), Some(Freshness::Month));
        assert_eq!(Freshness::from_arg("year"), Some(Freshness::Year));
        assert_eq!(Freshness::from_arg("decade"), None);
        // Bing qft 秒数
        assert_eq!(Freshness::Day.bing_seconds(), 86_400);
        assert_eq!(Freshness::Week.bing_seconds(), 604_800);
        assert_eq!(Freshness::Month.bing_seconds(), 2_592_000);
        assert_eq!(Freshness::Year.bing_seconds(), 31_536_000);
        // DDG df 参数
        assert_eq!(Freshness::Day.ddg_param(), "d");
        assert_eq!(Freshness::Week.ddg_param(), "w");
        assert_eq!(Freshness::Month.ddg_param(), "m");
        assert_eq!(Freshness::Year.ddg_param(), "y");
    }

    #[test]
    fn safesearch_from_arg_and_mappings() {
        assert_eq!(SafesearchLevel::from_arg("off"), Some(SafesearchLevel::Off));
        assert_eq!(
            SafesearchLevel::from_arg("MODERATE"),
            Some(SafesearchLevel::Moderate)
        );
        assert_eq!(
            SafesearchLevel::from_arg("strict"),
            Some(SafesearchLevel::Strict)
        );
        assert_eq!(SafesearchLevel::from_arg("max"), None);
        // Bing adlt 仅 off/strict 两级（moderate 映射 strict）
        assert_eq!(SafesearchLevel::Off.bing_param(), "off");
        assert_eq!(SafesearchLevel::Moderate.bing_param(), "strict");
        assert_eq!(SafesearchLevel::Strict.bing_param(), "strict");
        // DDG kp 三级
        assert_eq!(SafesearchLevel::Off.ddg_param(), "-1");
        assert_eq!(SafesearchLevel::Moderate.ddg_param(), "1");
        assert_eq!(SafesearchLevel::Strict.ddg_param(), "2");
    }

    #[test]
    fn engine_text_appends_site_and_filetype() {
        let q = SearchQuery {
            text: "rust async".into(),
            max_results: 10,
            lang: None,
            region: None,
            pages: 1,
            freshness: None,
            safesearch: None,
            site: Some("doc.rust-lang.org".into()),
            filetype: Some("pdf".into()),
        };
        assert_eq!(
            q.engine_text(),
            "rust async site:doc.rust-lang.org filetype:pdf"
        );
        // 原始 text 保留（site/filetype 只在引擎发送时追加）
        assert_eq!(q.text, "rust async");
    }
}
