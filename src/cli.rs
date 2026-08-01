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
    #[arg(long, value_enum, default_value_t = BrowserArg::Firefox)]
    pub browser: BrowserArg,

    /// 返回条数上限
    #[arg(long, default_value_t = worbrow::DEFAULT_MAX_RESULTS)]
    pub max_results: usize,

    /// 全流程硬超时（秒）
    #[arg(long, default_value_t = worbrow::DEFAULT_TIMEOUT_SECS)]
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

    /// JSON 输出（agent 调用必带）
    #[arg(long)]
    pub json: bool,

    /// stderr 日志级别
    #[arg(long, value_enum, default_value_t = LogLevelArg::Off)]
    pub log_level: LogLevelArg,

    /// 失败或成功时保存页面截图（调试）
    #[arg(long)]
    pub screenshot: Option<PathBuf>,

    /// 失败或 low_yield 时保存原始 HTML（调试）
    #[arg(long)]
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
    /// 以 MCP stdio server 形态运行
    #[cfg(feature = "mcp")]
    Mcp {
        /// 空闲超时（秒）：超过该时长无任何请求则自动退出；0 = 禁用（等客户端断开）
        #[arg(long, default_value_t = 0)]
        idle_timeout: u64,
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
