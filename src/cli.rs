//! CLI 参数定义（design.md §6.1）。
//!
//! 调用约定（硬契约）：stdout 仅输出 JSON（`--json` 时），日志走 stderr，无交互。

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// Agent search CLI: drives local headless browsers to search the web.
#[derive(Debug, Parser)]
#[command(
    name = "worbrow",
    version,
    about = "Drive local headless browsers (Chrome/Edge/Firefox) to search the web",
    long_about = None
)]
pub struct Cli {
    /// Search query (omit when using a subcommand)
    #[arg(value_name = "QUERY")]
    pub query: Option<String>,

    /// Search engine: comma-separated fallback order (available: duckduckgo, bing)
    // 默认引擎与 `domain::DEFAULT_ENGINE` 保持一致（clap default_value 需字面量）；
    // 变更默认引擎时两处必须同步
    #[arg(long, default_value = "bing,duckduckgo,baidu")]
    pub engine: String,

    /// Browser backend
    #[arg(long, global = true, value_enum, default_value_t = BrowserArg::Firefox)]
    pub browser: BrowserArg,

    /// Max number of results
    #[arg(long, default_value_t = worbrow::DEFAULT_MAX_RESULTS)]
    pub max_results: usize,

    /// Hard timeout in seconds for the whole run
    #[arg(long, global = true, default_value_t = worbrow::DEFAULT_TIMEOUT_SECS)]
    pub timeout: u64,

    /// Pages to aggregate (>1 merges results across pages)
    #[arg(long, default_value_t = 1)]
    pub pages: usize,

    /// Result language (e.g. zh-hans, Bing setlang)
    #[arg(long)]
    pub lang: Option<String>,

    /// Result region/market (e.g. zh-CN, Bing mkt / DDG kl)
    #[arg(long)]
    pub region: Option<String>,

    /// Freshness window (day|week|month|year; omit = any time)
    #[arg(long, value_enum)]
    pub freshness: Option<FreshnessArg>,

    /// Safe search level (off|moderate|strict; omit = engine default)
    #[arg(long, value_enum)]
    pub safesearch: Option<SafesearchArg>,

    /// Site filter (query-level site: syntax, e.g. doc.rust-lang.org)
    #[arg(long)]
    pub site: Option<String>,

    /// File type filter (query-level filetype: syntax, e.g. pdf)
    #[arg(long)]
    pub filetype: Option<String>,

    /// Retry count for transient network errors (exponential backoff, capped; only network errors trigger)
    #[arg(long, global = true, default_value_t = 0)]
    pub retry: usize,

    /// JSON output (required for agent use)
    #[arg(long, global = true)]
    pub json: bool,

    /// stderr log level
    #[arg(long, global = true, value_enum, default_value_t = LogLevelArg::Off)]
    pub log_level: LogLevelArg,

    /// Save a screenshot on failure or success (debugging)
    #[arg(long, global = true)]
    pub screenshot: Option<PathBuf>,

    /// Save raw HTML on failure or low_yield (debugging)
    #[arg(long, global = true)]
    pub dump_html: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check the environment: browser binaries, engine registry, driver availability
    Doctor,
    /// List available search engines
    List,
    /// Fetch an explicitly specified URL, returning cleaned body text and optional structured fields (ADR-009)
    Fetch {
        /// Target URL (http/https; missing scheme defaults to https://)
        url: String,
        /// Extract structured fields (comma-separated; supported: title/author/published_at/price/currency/rating/rating_max/reviews_count)
        #[arg(long, value_delimiter = ',', value_enum)]
        extract: Vec<ExtractFieldArg>,
        /// Max characters of body text
        #[arg(long, default_value_t = worbrow::DEFAULT_MAX_CHARS)]
        max_chars: usize,
        /// Do not return body text (only extracted structured fields, saves tokens)
        #[arg(long)]
        no_text: bool,
        /// Wait for this CSS selector to appear before extracting text (SPA content; best-effort, timeout still yields a success payload)
        #[arg(long)]
        wait_selector: Option<String>,
    },
    /// Run as an MCP stdio server
    #[cfg(feature = "mcp")]
    Mcp {
        /// Idle timeout (sec): exit after this long without any request; 0 = disabled (wait for client disconnect)
        #[arg(long, default_value_t = 0)]
        idle_timeout: u64,
        /// Session pool concurrency limit: concurrent searches reusing browser processes; 1 = serial reuse (saves memory)
        #[arg(long, default_value_t = 1)]
        max_sessions: usize,
        /// Idle session reclamation threshold (sec): browser processes unused longer than this are recycled
        #[arg(long, default_value_t = 60)]
        session_ttl: u64,
    },
}

/// 浏览器后端（clap value_enum）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BrowserArg {
    Chrome,
    Firefox,
}

impl BrowserArg {
    /// 映射到库层 `BrowserKind`：委托 `BrowserKind::from_arg`（clap 变体名与
    /// MCP 侧参数共享同一解析源，杜绝两处映射漂移）。
    pub fn to_kind(self) -> worbrow::BrowserKind {
        worbrow::BrowserKind::from_arg(match self {
            BrowserArg::Chrome => "chrome",
            BrowserArg::Firefox => "firefox",
        })
        .expect("clap 变体名必然可被 BrowserKind::from_arg 解析")
    }
}

/// 时间过滤窗口（clap value_enum，与 `domain::Freshness` 一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FreshnessArg {
    Day,
    Week,
    Month,
    Year,
}

impl FreshnessArg {
    pub fn to_domain(self) -> worbrow::Freshness {
        match self {
            FreshnessArg::Day => worbrow::Freshness::Day,
            FreshnessArg::Week => worbrow::Freshness::Week,
            FreshnessArg::Month => worbrow::Freshness::Month,
            FreshnessArg::Year => worbrow::Freshness::Year,
        }
    }
}

/// 安全搜索级别（clap value_enum，与 `domain::SafesearchLevel` 一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SafesearchArg {
    Off,
    Moderate,
    Strict,
}

impl SafesearchArg {
    pub fn to_domain(self) -> worbrow::SafesearchLevel {
        match self {
            SafesearchArg::Off => worbrow::SafesearchLevel::Off,
            SafesearchArg::Moderate => worbrow::SafesearchLevel::Moderate,
            SafesearchArg::Strict => worbrow::SafesearchLevel::Strict,
        }
    }
}

/// 结构化字段提取（clap value_enum，与 `domain::ExtractField` 一一对应）。
/// 多词变体同时接受 kebab（`published-at`）与 snake（`published_at`）两种写法，
/// 与 MCP 侧 snake_case 参数（经 `ExtractField::from_arg` 解析）对齐，均为稳定输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExtractFieldArg {
    Title,
    Author,
    #[value(name = "published-at", alias = "published_at")]
    PublishedAt,
    Price,
    Currency,
    Rating,
    #[value(name = "rating-max", alias = "rating_max")]
    RatingMax,
    #[value(name = "reviews-count", alias = "reviews_count")]
    ReviewsCount,
}

impl ExtractFieldArg {
    pub fn to_domain(self) -> worbrow::ExtractField {
        match self {
            ExtractFieldArg::Title => worbrow::ExtractField::Title,
            ExtractFieldArg::Author => worbrow::ExtractField::Author,
            ExtractFieldArg::PublishedAt => worbrow::ExtractField::PublishedAt,
            ExtractFieldArg::Price => worbrow::ExtractField::Price,
            ExtractFieldArg::Currency => worbrow::ExtractField::Currency,
            ExtractFieldArg::Rating => worbrow::ExtractField::Rating,
            ExtractFieldArg::RatingMax => worbrow::ExtractField::RatingMax,
            ExtractFieldArg::ReviewsCount => worbrow::ExtractField::ReviewsCount,
        }
    }
}

/// stderr 日志级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum LogLevelArg {
    #[default]
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
