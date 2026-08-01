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

    /// 搜索引擎
    #[arg(long, value_enum, default_value_t = EngineArg::Bing)]
    pub engine: EngineArg,

    /// 浏览器后端
    #[arg(long, value_enum, default_value_t = BrowserArg::Firefox)]
    pub browser: BrowserArg,

    /// 返回条数上限
    #[arg(long, default_value_t = 10)]
    pub max_results: usize,

    /// 全流程硬超时（秒）
    #[arg(long, default_value_t = 60)]
    pub timeout: u64,

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
    Mcp,
}

/// 搜索引擎（clap value_enum）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EngineArg {
    /// DuckDuckGo（html 端点）
    #[value(name = "duckduckgo")]
    DuckDuckGo,
    /// Bing（www.bing.com/search）
    #[value(name = "bing")]
    Bing,
}

impl EngineArg {
    pub fn name(self) -> &'static str {
        match self {
            EngineArg::DuckDuckGo => "duckduckgo",
            EngineArg::Bing => "bing",
        }
    }
}

/// 浏览器后端（clap value_enum）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BrowserArg {
    Chrome,
    Firefox,
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
