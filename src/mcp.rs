//! MCP（Model Context Protocol）stdio server（docs/adr/0005-mcp-stdio-server.md）。
//!
//! `search mcp` 子命令：以 MCP server 形态运行，通过 stdio 暴露 `search` 工具。
//! 工具参数 → `app::run` → 成功/失败包 JSON 作为 text content 返回。
//!
//! 通道约定：stdout 是 MCP JSON-RPC 通道（禁止任何 println 污染），日志走 stderr。

use std::time::Duration;

use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};

use crate::app;
use crate::drivers::BrowserKind;
use crate::error::Error;

/// MCP server：`search` 工具的唯一宿主。
#[derive(Debug, Clone, Default)]
pub struct SearchServer;

/// `search` 工具输入参数（schemars 自动生成 JSON Schema）。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    /// 搜索关键词（必填，1-512 字符）
    #[schemars(description = "要搜索的关键词（1-512 字符）")]
    pub query: String,
    /// 搜索引擎
    #[schemars(
        description = "搜索引擎（当前支持: duckduckgo）",
        default = "default_engine"
    )]
    #[serde(default = "default_engine")]
    pub engine: String,
    /// 浏览器后端
    #[schemars(
        description = "浏览器后端（fake=测试/无需浏览器，firefox=本机 Firefox，chrome=Chrome/Edge 待实现）",
        default = "default_browser"
    )]
    #[serde(default = "default_browser")]
    pub browser: String,
    /// 返回条数上限
    #[schemars(
        description = "返回结果条数上限（默认 10）",
        default = "default_max_results"
    )]
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    /// 全流程硬超时（秒）
    #[schemars(
        description = "全流程硬超时秒数（默认 20）",
        default = "default_timeout_secs"
    )]
    #[serde(default = "default_timeout_secs")]
    pub timeout: u64,
}

fn default_engine() -> String {
    "duckduckgo".to_string()
}

fn default_browser() -> String {
    "firefox".to_string()
}

fn default_max_results() -> usize {
    10
}

fn default_timeout_secs() -> u64 {
    20
}

impl SearchServer {
    /// 解析浏览器后端参数（与 CLI `--browser` 对齐，另接受 `fake` 供测试/冒烟）。
    fn parse_browser(s: &str) -> Result<BrowserKind, Error> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fake" => Ok(BrowserKind::Fake),
            "firefox" => Ok(BrowserKind::Firefox),
            "chrome" | "edge" | "chromium" => Ok(BrowserKind::Chrome),
            other => Err(Error::Cli(format!(
                "不支持的浏览器后端: {other}（支持 fake/firefox/chrome）"
            ))),
        }
    }
}

#[tool_router]
impl SearchServer {
    /// 执行一次搜索引擎搜索（MCP 工具）。
    #[tool(
        description = "驱动本机 headless 浏览器执行搜索引擎搜索，返回稳定 JSON 契约（schema_version=1 的成功/失败包）"
    )]
    async fn search(&self, Parameters(params): Parameters<SearchParams>) -> CallToolResult {
        // 参数解析失败 → 用户可见的 error content（而非协议错误）
        let browser = match Self::parse_browser(&params.browser) {
            Ok(b) => b,
            Err(e) => return CallToolResult::error(vec![ContentBlock::text(e.to_string())]),
        };

        let config = app::Config {
            query: params.query,
            engine: params.engine,
            browser,
            max_results: params.max_results.max(1),
            // 防呆：clamp 到 1-300s，避免 agent 传 0 或极端值
            timeout: Duration::from_secs(params.timeout.clamp(1, 300)),
            screenshot: None,
            dump_html: None,
            driver: None,
        };

        match app::run(config).await {
            Ok(outcome) => CallToolResult::success(vec![ContentBlock::text(
                crate::output::success(&outcome.query, &outcome.results, &outcome.meta),
            )]),
            Err(err) => {
                CallToolResult::error(vec![ContentBlock::text(crate::output::failure(&err))])
            }
        }
    }
}

#[tool_handler(router = Self::tool_router())]
impl ServerHandler for SearchServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "通过驱动本机 headless 浏览器执行搜索引擎搜索。工具返回 JSON 文本：\
                 成功包含 results/meta，失败包含 error（code/message），schema_version=1。",
        )
    }
}

/// 以 MCP stdio server 形态运行，直到客户端关闭连接（stdin EOF）。
pub async fn serve_stdio() -> Result<(), Error> {
    let running = SearchServer
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| Error::Internal(format!("MCP server 初始化失败: {e}")))?;
    running
        .waiting()
        .await
        .map_err(|e| Error::Internal(format!("MCP server 运行失败: {e}")))?;
    Ok(())
}
