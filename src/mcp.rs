//! MCP（Model Context Protocol）stdio server（docs/adr/0005-mcp-stdio-server.md）。
//!
//! `worbrow mcp` 子命令：以 MCP server 形态运行，通过 stdio 暴露 `web_search` 工具。
//! 工具参数 → `app::run` → 成功/失败包 JSON 作为 text content 返回。
//!
//! 通道约定：stdout 是 MCP JSON-RPC 通道（禁止任何 println 污染），日志走 stderr。

use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use tokio::io::{AsyncRead, ReadBuf};

use crate::app;
use crate::drivers::BrowserKind;
use crate::error::Error;

/// MCP server：`web_search` 工具的唯一宿主。
#[derive(Debug, Clone, Default)]
pub struct SearchServer;

/// `web_search` 工具输入参数（schemars 自动生成 JSON Schema）。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    /// 搜索关键词（必填，1-512 字符）
    #[schemars(description = "要搜索的关键词（1-512 字符）")]
    pub query: String,
    /// 搜索引擎
    #[schemars(
        description = "搜索引擎（当前支持: duckduckgo/bing）",
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
        description = "全流程硬超时秒数（默认 60）",
        default = "default_timeout_secs"
    )]
    #[serde(default = "default_timeout_secs")]
    pub timeout: u64,
}

fn default_engine() -> String {
    "bing".to_string()
}

fn default_browser() -> String {
    "firefox".to_string()
}

fn default_max_results() -> usize {
    10
}

fn default_timeout_secs() -> u64 {
    60
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
        name = "web_search",
        description = "驱动本机 headless 浏览器执行搜索引擎搜索，返回稳定 JSON 契约（schema_version=1 的成功/失败包）"
    )]
    async fn web_search(&self, Parameters(params): Parameters<SearchParams>) -> CallToolResult {
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

/// 空闲超时轮询间隔（秒级粒度即可，开销可忽略）。
const IDLE_POLL: Duration = Duration::from_secs(1);

/// 距进程启动的单调纳秒数。`Instant` 无法原子化，故存其到启动点的差值（u64），
/// 使空闲时间戳可用 `AtomicU64` 无锁共享（读多写少、Relaxed 序即足够）。
fn monotonic_nanos() -> u64 {
    static START: OnceLock<tokio::time::Instant> = OnceLock::new();
    let start = *START.get_or_init(tokio::time::Instant::now);
    start.elapsed().as_nanos() as u64
}

/// 包装 stdin 的读端：每次读到数据时刷新 `last_activity`，供空闲超时判定。
/// 客户端（agent）的任何请求都会经过 stdin 读取，因此"最近读取时间"即"最近活动时间"。
struct ActivityReader<R> {
    inner: R,
    last_activity: Arc<AtomicU64>,
}

impl<R: AsyncRead + Unpin> AsyncRead for ActivityReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let filled_before = buf.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if poll.is_ready() && buf.filled().len() > filled_before {
            self.last_activity
                .store(monotonic_nanos(), Ordering::Relaxed);
        }
        poll
    }
}

/// 以 MCP stdio server 形态运行。
///
/// - `idle: None`：一直等待客户端断开（stdin EOF）后退出（原行为）。
/// - `idle: Some(d)`：超过 `d` 时长无任何请求则自动退出（防 agent 崩溃后残留进程）。
///   检测覆盖整个生命周期：握手前（等 initialize）与握手后（等工具调用）同样生效。
pub async fn serve_stdio(idle: Option<Duration>) -> Result<(), Error> {
    let Some(idle) = idle else {
        return serve_until_eof().await;
    };

    let (stdin, stdout) = rmcp::transport::stdio();
    let last_activity = Arc::new(AtomicU64::new(monotonic_nanos()));
    let reader = ActivityReader {
        inner: stdin,
        last_activity: last_activity.clone(),
    };

    // 阶段一：等待握手（initialize）。rmcp 在收到 initialize 前不返回 RunningService，
    // 因此空闲检测必须覆盖此阶段，否则"未握手即崩溃"的 agent 仍会残留进程。
    let mut serve_fut = Box::pin(SearchServer.serve((reader, stdout)));
    let running = loop {
        tokio::select! {
            r = &mut serve_fut => {
                break r.map_err(|e| Error::Internal(format!("MCP server 初始化失败: {e}")))?;
            }
            _ = tokio::time::sleep(IDLE_POLL) => {
                if idle_expired(&last_activity, idle) {
                    return Ok(());
                }
            }
        }
    };

    // 阶段二：握手完成，等待后续请求直到 EOF 或空闲超时。
    let mut waiting = Box::pin(running.waiting());
    loop {
        tokio::select! {
            r = &mut waiting => {
                r.map_err(|e| Error::Internal(format!("MCP server 运行失败: {e}")))?;
                return Ok(());
            }
            _ = tokio::time::sleep(IDLE_POLL) => {
                if idle_expired(&last_activity, idle) {
                    tracing::info!(
                        idle_secs = idle.as_secs(),
                        "MCP server 空闲超时，自动退出"
                    );
                    return Ok(());
                }
            }
        }
    }
}

/// 距最后一次读取（请求活动）是否已超过空闲窗口。
fn idle_expired(last_activity: &AtomicU64, idle: Duration) -> bool {
    let idle_nanos = idle.as_nanos() as u64;
    // saturating_sub 防时钟回退/首次加载时 last > now 的假阴性
    monotonic_nanos().saturating_sub(last_activity.load(Ordering::Relaxed)) >= idle_nanos
}

/// 一直运行到 stdin EOF（客户端关闭连接）。
async fn serve_until_eof() -> Result<(), Error> {
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
