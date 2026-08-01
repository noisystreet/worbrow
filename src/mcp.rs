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
use crate::domain::{BrowserKind, Freshness, SafesearchLevel};
use crate::drivers::SessionPool;
use crate::error::Error;

/// MCP 会话池默认配置（roadmap-session-pool.md §6 已定决策）。
const DEFAULT_MAX_SESSIONS: usize = 1;
const DEFAULT_IDLE_TTL: Duration = Duration::from_secs(60);

/// MCP 短 TTL 结果缓存默认配置（roadmap.md「网络重试与结果缓存」已定决策）。
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(60);
/// 缓存容量上限（LRU 淘汰最久未用；防止长驻进程内存无限增长）。
const DEFAULT_CACHE_CAPACITY: usize = 128;

/// 结果缓存 key：覆盖影响搜索结果的**全部**请求参数（含浏览器后端）。
/// 相同 key = 相同请求 → 命中缓存返回同一结果（TTL 内）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    query: String,
    engine: String,
    browser: BrowserKind,
    max_results: usize,
    lang: Option<String>,
    region: Option<String>,
    pages: usize,
    freshness: Option<Freshness>,
    safesearch: Option<SafesearchLevel>,
    site: Option<String>,
    filetype: Option<String>,
}

/// 缓存条目：结果 + 写入时间（TTL 判定）+ 命中次序（LRU 淘汰）。
#[derive(Debug)]
struct CacheEntry {
    outcome: app::Outcome,
    /// 距进程启动的单调时间（复用 `monotonic_nanos` 语义；此处直接存 Instant 供 LRU 排序）。
    inserted: tokio::time::Instant,
}

/// MCP 短 TTL 结果缓存（LRU + TTL）：相同 query 短时间重复搜索直接命中，
/// 免去浏览器往返（roadmap.md「网络重试与结果缓存」；schema v1 只增不改）。
#[derive(Debug)]
struct SearchCache {
    inner: std::sync::Mutex<Vec<(CacheKey, CacheEntry)>>,
    ttl: Duration,
    capacity: usize,
}

impl SearchCache {
    fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            inner: std::sync::Mutex::new(Vec::new()),
            ttl,
            capacity: capacity.max(1),
        }
    }

    /// 命中：返回结果副本并刷新 LRU 次序；`inserted` 为命中时刻（TTL 重新计时）。
    /// `meta.cached=true`，`started_at`/`elapsed_ms` 刷新为本次调用（agent 感知命中时延）。
    fn get(&self, key: &CacheKey) -> Option<app::Outcome> {
        let mut entries = self.inner.lock().ok()?;
        let now = tokio::time::Instant::now();
        // 先清理过期条目
        entries.retain(|(_, e)| now.duration_since(e.inserted) < self.ttl);
        let pos = entries.iter().position(|(k, _)| k == key)?;
        let (_, entry) = entries.remove(pos); // 取出 → 放到末尾（LRU 最近使用）
        let mut outcome = entry.outcome;
        outcome.meta.cached = true;
        outcome.meta.started_at = chrono::Utc::now();
        // 缓存命中仅一次 Mutex 往返（微秒级），elapsed_ms 记 0 让 agent 明确感知"未走搜索"
        outcome.meta.elapsed_ms = 0;
        entries.push((
            key.clone(),
            CacheEntry {
                outcome: outcome.clone(),
                inserted: now,
            },
        ));
        Some(outcome)
    }

    /// 写入：TTL 内相同 key 覆盖；超容量淘汰最久未用（队首）。
    fn put(&self, key: CacheKey, outcome: app::Outcome) {
        let mut entries = self.inner.lock().ok();
        let Some(entries) = entries.as_mut() else {
            return;
        };
        let now = tokio::time::Instant::now();
        entries.retain(|(_, e)| now.duration_since(e.inserted) < self.ttl);
        if let Some(pos) = entries.iter().position(|(k, _)| *k == key) {
            entries.remove(pos);
        }
        entries.push((
            key,
            CacheEntry {
                outcome,
                inserted: now,
            },
        ));
        // LRU 淘汰：超出容量时移除队首（最久未使用）
        while entries.len() > self.capacity {
            entries.remove(0);
        }
    }
}

/// 会话池注册表：按浏览器后端各持一个池（fake/chrome/firefox 不混池，避免并发
/// profile 冲突；spawn 惰性——只有 acquire 时才真正启动浏览器进程）。
#[derive(Debug)]
struct PoolRegistry {
    fake: Arc<SessionPool>,
    chrome: Arc<SessionPool>,
    firefox: Arc<SessionPool>,
}

impl PoolRegistry {
    fn new(max_sessions: usize, idle_ttl: Duration) -> Self {
        Self {
            fake: SessionPool::new(BrowserKind::Fake, max_sessions, idle_ttl, 4),
            chrome: SessionPool::new(BrowserKind::Chrome, max_sessions, idle_ttl, 4),
            firefox: SessionPool::new(BrowserKind::Firefox, max_sessions, idle_ttl, 4),
        }
    }

    fn pool_for(&self, kind: BrowserKind) -> &Arc<SessionPool> {
        match kind {
            BrowserKind::Fake => &self.fake,
            BrowserKind::Chrome => &self.chrome,
            BrowserKind::Firefox => &self.firefox,
        }
    }
}

/// MCP server：`web_search` 工具的唯一宿主；持会话池复用浏览器进程
/// （MCP 长驻场景消除每次搜索 spawn 2-5s 开销，roadmap-session-pool.md）。
#[derive(Debug, Clone)]
pub struct SearchServer {
    pools: Arc<PoolRegistry>,
    cache: Arc<SearchCache>,
}

impl SearchServer {
    /// 以指定池配置创建 server（MCP 长驻进程内共享）。
    fn with_pools(max_sessions: usize, idle_ttl: Duration) -> Self {
        Self {
            pools: Arc::new(PoolRegistry::new(max_sessions, idle_ttl)),
            cache: Arc::new(SearchCache::new(DEFAULT_CACHE_TTL, DEFAULT_CACHE_CAPACITY)),
        }
    }
}

/// `web_search` 工具输入参数（schemars 自动生成 JSON Schema）。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    /// 搜索关键词（必填，1-512 字符）
    #[schemars(description = "要搜索的关键词（1-512 字符）")]
    pub query: String,
    /// 搜索引擎（逗号分隔 = 降级尝试顺序，如 "bing,duckduckgo"）
    #[schemars(
        description = "搜索引擎（当前支持: duckduckgo/bing；逗号分隔为降级尝试顺序，如 bing,duckduckgo）",
        default = "default_engine"
    )]
    #[serde(default = "default_engine")]
    pub engine: String,
    /// 浏览器后端
    #[schemars(
        description = "浏览器后端（fake=测试/无需浏览器，firefox=本机 Firefox，chrome=Chrome/Edge）",
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
    /// 结果语言（如 zh-hans，Bing setlang）
    #[schemars(description = "结果语言（可选，如 zh-hans）", default = "default_lang")]
    #[serde(default = "default_lang")]
    pub lang: Option<String>,
    /// 结果地域/市场（如 zh-CN，Bing mkt / DDG kl）
    #[schemars(
        description = "结果地域/市场（可选，如 zh-CN）",
        default = "default_region"
    )]
    #[serde(default = "default_region")]
    pub region: Option<String>,
    /// 翻页聚合页数（>1 时跨页去重合并）
    #[schemars(
        description = "翻页聚合页数（默认 1 = 仅首页）",
        default = "default_pages"
    )]
    #[serde(default = "default_pages")]
    pub pages: usize,
    /// 时间过滤窗口（day|week|month|year）
    #[schemars(
        description = "时间过滤窗口（可选: day/week/month/year；缺省 = 不限时间）",
        default = "default_none"
    )]
    #[serde(default = "default_none")]
    pub freshness: Option<String>,
    /// 安全搜索级别（off|moderate|strict）
    #[schemars(
        description = "安全搜索级别（可选: off/moderate/strict；缺省 = 引擎默认）",
        default = "default_none"
    )]
    #[serde(default = "default_none")]
    pub safesearch: Option<String>,
    /// 站点过滤（query 级 site: 语法）
    #[schemars(
        description = "站点过滤（可选，如 doc.rust-lang.org；query 级 site: 语法）",
        default = "default_none"
    )]
    #[serde(default = "default_none")]
    pub site: Option<String>,
    /// 文件类型过滤（query 级 filetype: 语法）
    #[schemars(
        description = "文件类型过滤（可选，如 pdf；query 级 filetype: 语法）",
        default = "default_none"
    )]
    #[serde(default = "default_none")]
    pub filetype: Option<String>,
    /// 全流程硬超时（秒）
    #[schemars(
        description = "全流程硬超时秒数（默认 60）",
        default = "default_timeout_secs"
    )]
    #[serde(default = "default_timeout_secs")]
    pub timeout: u64,
    /// 瞬时网络错误重试次数（指数退避，封顶）
    #[schemars(
        description = "瞬时网络错误重试次数（默认 0 = 不重试；仅网络错误触发）",
        default = "default_retry"
    )]
    #[serde(default = "default_retry")]
    pub retry: usize,
    /// 是否绕过短 TTL 结果缓存（需要新鲜结果时置 true）
    #[schemars(
        description = "是否绕过短 TTL 结果缓存（默认 false = 命中缓存直接返回）",
        default = "default_no_cache"
    )]
    #[serde(default = "default_no_cache")]
    pub no_cache: bool,
    /// 精简输出模式（仅 rank/title/url，省上下文 token）
    #[schemars(
        description = "精简输出模式（默认 false；true = 结果仅含 rank/title/url）",
        default = "default_compact"
    )]
    #[serde(default = "default_compact")]
    pub compact: bool,
}

fn default_engine() -> String {
    crate::domain::DEFAULT_ENGINE.to_string()
}

fn default_browser() -> String {
    crate::domain::DEFAULT_BROWSER.to_string()
}

fn default_max_results() -> usize {
    crate::domain::DEFAULT_MAX_RESULTS
}

fn default_lang() -> Option<String> {
    None
}

fn default_region() -> Option<String> {
    None
}

fn default_pages() -> usize {
    1
}

fn default_none() -> Option<String> {
    None
}

fn default_timeout_secs() -> u64 {
    crate::domain::DEFAULT_TIMEOUT_SECS
}

fn default_retry() -> usize {
    0
}

fn default_no_cache() -> bool {
    false
}

fn default_compact() -> bool {
    false
}

impl SearchServer {
    /// 解析浏览器后端参数（CLI `--browser` 与 MCP 共用 `BrowserKind::from_arg` 单一映射）。
    fn parse_browser(s: &str) -> Result<BrowserKind, Error> {
        BrowserKind::from_arg(s).ok_or_else(|| {
            Error::Cli(format!(
                "不支持的浏览器后端: {s}（支持 fake/firefox/chrome/edge/chromium）"
            ))
        })
    }

    /// 解析时间过滤窗口（缺省 = None；非法值 → 参数错误，工具级 error）。
    fn parse_freshness(s: Option<&str>) -> Result<Option<Freshness>, Error> {
        match s {
            None => Ok(None),
            Some(v) => Freshness::from_arg(v).map(Some).ok_or_else(|| {
                Error::Cli(format!(
                    "不支持的 freshness: {v}（支持 day/week/month/year）"
                ))
            }),
        }
    }

    /// 解析安全搜索级别（缺省 = None；非法值 → 参数错误，工具级 error）。
    fn parse_safesearch(s: Option<&str>) -> Result<Option<SafesearchLevel>, Error> {
        match s {
            None => Ok(None),
            Some(v) => SafesearchLevel::from_arg(v).map(Some).ok_or_else(|| {
                Error::Cli(format!(
                    "不支持的 safesearch: {v}（支持 off/moderate/strict）"
                ))
            }),
        }
    }
}

#[tool_router]
impl SearchServer {
    /// 执行一次搜索引擎搜索（MCP 工具）。
    ///
    /// 池化路径：`acquire` 借出会话（复用浏览器进程）→ `app::run_with` 执行 →
    /// 按错误类型判定健康（网络/超时错误 → 标记不健康丢弃重建；其余错误浏览器
    /// 本身健康可复用）→ Drop 归还（roadmap-session-pool.md）。
    ///
    /// 缓存路径：相同请求参数（CacheKey）在短 TTL 内二次调用直接命中缓存返回
    /// （`meta.cached=true`）；`no_cache=true` 绕过缓存（roadmap.md「网络重试与缓存」）。
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
        let freshness = match Self::parse_freshness(params.freshness.as_deref()) {
            Ok(v) => v,
            Err(e) => return CallToolResult::error(vec![ContentBlock::text(e.to_string())]),
        };
        let safesearch = match Self::parse_safesearch(params.safesearch.as_deref()) {
            Ok(v) => v,
            Err(e) => return CallToolResult::error(vec![ContentBlock::text(e.to_string())]),
        };

        let config = app::Config::new(params.query.clone(), params.engine.clone(), browser)
            .with_max_results(params.max_results)
            // 防呆：clamp 到 1-300s，避免 agent 传 0 或极端值
            .with_timeout(Duration::from_secs(params.timeout.clamp(1, 300)))
            .with_lang(params.lang.clone())
            .with_region(params.region.clone())
            .with_pages(params.pages)
            .with_freshness(freshness)
            .with_safesearch(safesearch)
            .with_site(params.site.clone())
            .with_filetype(params.filetype.clone())
            .with_retry(params.retry.min(5)); // 防呆：重试次数封顶 5

        // 缓存命中（仅未显式绕过时）：相同请求参数 TTL 内直接返回，免浏览器往返。
        // key 与 config 对齐：max_results clamp 到 ≥1（同 with_max_results 语义）
        let cache_key = CacheKey {
            query: params.query.clone(),
            engine: params.engine.clone(),
            browser,
            max_results: params.max_results.max(1),
            lang: params.lang.clone(),
            region: params.region.clone(),
            pages: params.pages,
            freshness,
            safesearch,
            site: params.site.clone(),
            filetype: params.filetype.clone(),
        };
        if !params.no_cache
            && let Some(outcome) = self.cache.get(&cache_key)
        {
            return CallToolResult::success(vec![ContentBlock::text(format_outcome(
                &outcome,
                params.compact,
            ))]);
        }

        // 借出会话（复用浏览器进程）；spawn 失败 → 工具级错误
        let mut guard = match self.pools.pool_for(browser).acquire().await {
            Ok(g) => g,
            Err(err) => {
                return CallToolResult::error(vec![ContentBlock::text(crate::output::failure(
                    &err,
                ))]);
            }
        };

        let outcome = app::run_with(guard.driver(), config).await;
        // 健康判定：网络/超时 → 会话可能已损坏（浏览器崩溃/连接断开），丢弃重建；
        // 其余（验证码/解析/参数）→ 浏览器本身健康，可复用
        match outcome {
            Ok(outcome) => {
                // 写入缓存（未绕过时）：后续相同请求 TTL 内直接命中
                if !params.no_cache {
                    self.cache.put(cache_key, outcome.clone());
                }
                CallToolResult::success(vec![ContentBlock::text(format_outcome(
                    &outcome,
                    params.compact,
                ))])
            }
            Err(err) => {
                if matches!(err, Error::Network(_) | Error::Timeout(_)) {
                    guard.mark_unhealthy();
                }
                CallToolResult::error(vec![ContentBlock::text(crate::output::failure(&err))])
            }
        }
        // guard 在此 Drop：健康 → 归还池；不健康 → 丢弃（重建）
    }

    /// 列出可用引擎（agent 自查引擎名/降级顺序用，无需读错误码）。
    #[tool(
        name = "list_engines",
        description = "列出当前可用的搜索引擎（供拼接 engine 参数与降级链，如 bing,duckduckgo）"
    )]
    async fn list_engines(&self) -> CallToolResult {
        let body = serde_json::to_string_pretty(&crate::engines::AVAILABLE)
            .expect("引擎列表序列化不应失败");
        CallToolResult::success(vec![ContentBlock::text(body)])
    }

    /// 环境自检（浏览器二进制/引擎注册表），agent 排查环境问题无需读错误码。
    #[tool(
        name = "doctor",
        description = "检查环境：可用引擎、浏览器后端二进制与版本（Chrome ≥109 / Firefox ≥55）"
    )]
    async fn doctor(&self) -> CallToolResult {
        let report = app::DoctorReport::collect();
        let body = serde_json::to_string_pretty(&report).expect("doctor 报告序列化不应失败");
        CallToolResult::success(vec![ContentBlock::text(body)])
    }
}

/// 按 `compact` 选择成功包序列化（完整 vs 精简 rank/title/url）。
fn format_outcome(outcome: &app::Outcome, compact: bool) -> String {
    if compact {
        crate::output::success_compact(&outcome.query, &outcome.results, &outcome.meta)
    } else {
        crate::output::success(&outcome.query, &outcome.results, &outcome.meta)
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

/// 会话池配置（MCP 长驻场景启用；roadmap-session-pool.md §6 已定决策默认值）。
#[derive(Debug, Clone, Copy)]
pub struct PoolConfig {
    /// 并发上限（默认 1：单用户串行省内存；超限请求排队）。
    pub max_sessions: usize,
    /// 空闲会话回收阈值（默认 60s）。
    pub idle_ttl: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_sessions: DEFAULT_MAX_SESSIONS,
            idle_ttl: DEFAULT_IDLE_TTL,
        }
    }
}

/// 以 MCP stdio server 形态运行。
///
/// - `idle: None`：一直等待客户端断开（stdin EOF）后退出（原行为）。
/// - `idle: Some(d)`：超过 `d` 时长无任何请求则自动退出（防 agent 崩溃后残留进程）。
///   检测覆盖整个生命周期：握手前（等 initialize）与握手后（等工具调用）同样生效。
pub async fn serve_stdio(idle: Option<Duration>, pool: Option<PoolConfig>) -> Result<(), Error> {
    let server = SearchServer::with_pools(
        pool.map(|p| p.max_sessions).unwrap_or(DEFAULT_MAX_SESSIONS),
        pool.map(|p| p.idle_ttl).unwrap_or(DEFAULT_IDLE_TTL),
    );
    let Some(idle) = idle else {
        return serve_until_eof(server).await;
    };

    let (stdin, stdout) = rmcp::transport::stdio();
    let last_activity = Arc::new(AtomicU64::new(monotonic_nanos()));
    let reader = ActivityReader {
        inner: stdin,
        last_activity: last_activity.clone(),
    };

    // 阶段一：等待握手（initialize）。rmcp 在收到 initialize 前不返回 RunningService，
    // 因此空闲检测必须覆盖此阶段，否则"未握手即崩溃"的 agent 仍会残留进程。
    let mut serve_fut = Box::pin(server.serve((reader, stdout)));
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
async fn serve_until_eof(server: SearchServer) -> Result<(), Error> {
    let running = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| Error::Internal(format!("MCP server 初始化失败: {e}")))?;
    running
        .waiting()
        .await
        .map_err(|e| Error::Internal(format!("MCP server 运行失败: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app;
    use chrono::Utc;

    fn sample_outcome(query: &str) -> app::Outcome {
        app::Outcome {
            query: query.to_string(),
            results: vec![],
            meta: crate::domain::SearchMeta {
                engine: "bing",
                started_at: Utc::now(),
                elapsed_ms: 10,
                result_count: 0,
                pages: 1,
                low_yield: false,
                captcha: false,
                engine_error: None,
                engine_tried: vec!["bing".to_string()],
                cached: false,
                retries: 0,
            },
        }
    }

    fn sample_key(query: &str) -> CacheKey {
        CacheKey {
            query: query.to_string(),
            engine: "bing".to_string(),
            browser: BrowserKind::Fake,
            max_results: 10,
            lang: None,
            region: None,
            pages: 1,
            freshness: None,
            safesearch: None,
            site: None,
            filetype: None,
        }
    }

    /// 相同 key put → get 命中（cached=true），且不重复执行。
    #[test]
    fn cache_hit_returns_cached_outcome() {
        let cache = SearchCache::new(Duration::from_secs(60), 16);
        let key = sample_key("rust");
        cache.put(key.clone(), sample_outcome("rust"));

        let hit = cache.get(&key).expect("应命中缓存");
        assert!(hit.meta.cached, "命中后 meta.cached 应为 true");
        assert_eq!(hit.query, "rust");
        // 再次 get 仍命中（命中后 LRU 刷新，TTL 重新计时）
        assert!(cache.get(&key).is_some());
    }

    /// 不同 key 不互相命中。
    #[test]
    fn cache_distinguishes_keys() {
        let cache = SearchCache::new(Duration::from_secs(60), 16);
        cache.put(sample_key("rust"), sample_outcome("rust"));
        assert!(cache.get(&sample_key("rust")).is_some());
        assert!(
            cache.get(&sample_key("async")).is_none(),
            "不同 query 不应命中"
        );
    }

    /// TTL 过期：超时后 get 返回 None（LRU 清理）。
    #[test]
    fn cache_expires_after_ttl() {
        let cache = SearchCache::new(Duration::from_millis(100), 16);
        cache.put(sample_key("rust"), sample_outcome("rust"));
        // TTL 内命中
        assert!(cache.get(&sample_key("rust")).is_some());
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            cache.get(&sample_key("rust")).is_none(),
            "TTL 过期后不应命中"
        );
    }

    /// LRU 容量淘汰：超出 capacity 时淘汰最久未用。
    #[test]
    fn cache_evicts_lru_on_capacity() {
        let cache = SearchCache::new(Duration::from_secs(60), 2);
        cache.put(sample_key("a"), sample_outcome("a"));
        cache.put(sample_key("b"), sample_outcome("b"));
        cache.put(sample_key("c"), sample_outcome("c")); // 超容量 → 淘汰 "a"
        assert!(cache.get(&sample_key("a")).is_none(), "最久未用应被淘汰");
        assert!(cache.get(&sample_key("b")).is_some());
        assert!(cache.get(&sample_key("c")).is_some());
    }
}
