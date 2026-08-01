//! CLI 参数定义（design.md §6.1）。
//!
//! 调用约定（硬契约）：stdout 仅输出 JSON（`--json` 时），日志走 stderr，无交互。

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// Agent 搜索 CLI：驱动本机 headless 浏览器执行搜索引擎搜索。
#[derive(Debug, Parser)]
#[command(
    name = "worbrow",
    version,
    about = "驱动本机 headless 浏览器（Chrome/Edge/Firefox）执行搜索引擎搜索",
    long_about = None
)]
pub struct Cli {
    /// 搜索词（有子命令时省略）
    #[arg(value_name = "QUERY")]
    pub query: Option<String>,

    /// 搜索引擎：逗号分隔 = 降级尝试顺序（可用: duckduckgo, bing）
    // 默认引擎与 `domain::DEFAULT_ENGINE` 保持一致（clap default_value 需字面量）；
    // 变更默认引擎时两处必须同步
    #[arg(long, default_value = "bing")]
    pub engine: String,

    /// 浏览器后端
    #[arg(long, global = true, value_enum, default_value_t = BrowserArg::Firefox)]
    pub browser: BrowserArg,

    /// 返回条数上限
    #[arg(long, default_value_t = worbrow::DEFAULT_MAX_RESULTS)]
    pub max_results: usize,

    /// 全流程硬超时（秒）
    #[arg(long, global = true, default_value_t = worbrow::DEFAULT_TIMEOUT_SECS)]
    pub timeout: u64,

    /// 翻页聚合页数（>1 时跨页去重合并）
    #[arg(long, default_value_t = 1)]
    pub pages: usize,

    /// 结果语言（如 zh-hans，Bing setlang）
    #[arg(long)]
    pub lang: Option<String>,

    /// 结果地域/市场（如 zh-CN，Bing mkt / DDG kl）
    #[arg(long)]
    pub region: Option<String>,

    /// 时间过滤窗口（day|week|month|year；不指定 = 不限时间）
    #[arg(long, value_enum)]
    pub freshness: Option<FreshnessArg>,

    /// 安全搜索级别（off|moderate|strict；不指定 = 引擎默认）
    #[arg(long, value_enum)]
    pub safesearch: Option<SafesearchArg>,

    /// 站点过滤（query 级 site: 语法，如 doc.rust-lang.org）
    #[arg(long)]
    pub site: Option<String>,

    /// 文件类型过滤（query 级 filetype: 语法，如 pdf）
    #[arg(long)]
    pub filetype: Option<String>,

    /// 瞬时网络错误重试次数（指数退避，封顶；仅网络错误触发）
    #[arg(long, global = true, default_value_t = 0)]
    pub retry: usize,

    /// JSON 输出（agent 调用必带）
    #[arg(long, global = true)]
    pub json: bool,

    /// stderr 日志级别
    #[arg(long, global = true, value_enum, default_value_t = LogLevelArg::Off)]
    pub log_level: LogLevelArg,

    /// 失败或成功时保存页面截图（调试）
    #[arg(long, global = true)]
    pub screenshot: Option<PathBuf>,

    /// 失败或 low_yield 时保存原始 HTML（调试）
    #[arg(long, global = true)]
    pub dump_html: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 自检环境：浏览器二进制、引擎注册表、驱动可用性
    Doctor,
    /// 列出可用搜索引擎
    List,
    /// 抓取显式指定的 URL，返回清洗后的正文与可选结构化字段（ADR-009）
    Fetch {
        /// 目标 URL（http/https；缺 scheme 自动补 https://）
        url: String,
        /// 结构化字段提取（逗号分隔，可用: title/author/published_at/price/currency/rating/rating_max/reviews_count）
        #[arg(long, value_delimiter = ',', value_enum)]
        extract: Vec<ExtractFieldArg>,
        /// 正文截断上限（字符）
        #[arg(long, default_value_t = worbrow::DEFAULT_MAX_CHARS)]
        max_chars: usize,
        /// 不返回正文文本（只返回 extracted 结构化字段，省 token）
        #[arg(long)]
        no_text: bool,
    },
    /// 以 MCP stdio server 形态运行
    #[cfg(feature = "mcp")]
    Mcp {
        /// 空闲超时（秒）：超过该时长无任何请求则自动退出；0 = 禁用（等客户端断开）
        #[arg(long, default_value_t = 0)]
        idle_timeout: u64,
        /// 会话池并发上限：复用浏览器进程的并发搜索数；1 = 串行复用（省内存）
        #[arg(long, default_value_t = 1)]
        max_sessions: usize,
        /// 空闲会话回收阈值（秒）：超过该时长未使用的浏览器进程被回收
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
