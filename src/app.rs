//! 用例编排（design.md §6.2）。
//!
//! `run(config)`：解析 query → 选引擎 → 驱动浏览器 → 抽取 → 组装 Outcome。
//! 硬超时包裹全流程，超时返回 `Error::Timeout`（exit 124）。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Serialize;
use tokio::time::timeout;

use crate::SearchResult;
use crate::domain::{
    BrowserKind, ExtractField, FetchedPage, Freshness, SafesearchLevel, SearchMeta, SearchQuery,
};
use crate::engines;
use crate::error::Error;
use crate::ports::{BrowserDriver, SearchProvider};

/// 低结果阈值：结果数低于该值时 `meta.low_yield = true`（design.md §10.4）。
pub const LOW_YIELD_THRESHOLD: usize = 3;
/// 结果元素等待预算上限：页面加载已消耗大部分 timeout 时，剩余时间不足以等待选择器
/// （design.md §6.2 二级超时）。
pub const WAIT_BUDGET: Duration = Duration::from_secs(10);
/// 同域名去重上限（P2，roadmap-result-quality.md）：未启用 `site:` 过滤时，同一域名
/// 最多保留该条数（防单一来源刷屏）；`site:` 过滤时用户意图为同域，不限制。
pub const DOMAIN_LIMIT: usize = 2;
/// fetch 页面加载等待轮询间隔（`wait_load`，ADR-009）。
const LOAD_POLL: Duration = Duration::from_millis(200);

/// 取目标页 HTTP 状态码的 JS 表达式（`PerformanceNavigationTiming.responseStatus`）：
/// Firefox ≥ 105 / Chrome 支持；data: URL、跨域受限或无 navigation 条目时返回 `null`。
/// 用 IIFE 保持"表达式"语义（Marionette 会包一层 `return (...)`，表达式内不能有顶层 return）。
const HTTP_STATUS_JS: &str = "(() => { const n = performance.getEntriesByType('navigation')[0]; return n ? (n.responseStatus || null) : null; })()";

/// 内容型（`ResultKind::Web`）结果数：质量降级信号核心（roadmap-result-quality.md）。
/// 词典/翻译等污染结果不计入——高产低质（如 Bing 对 `best`/`learn` 返回全词典释义）
/// 不再满足降级判定，自动尝试下一引擎。
fn content_count(results: &[SearchResult]) -> usize {
    results
        .iter()
        .filter(|r| r.result_kind == crate::domain::ResultKind::Web)
        .count()
}

/// 相关性命中占比阈值（P3 门禁细化，roadmap-result-quality.md）：命中至少一个显著词
/// 的结果占比 **低于 1/5（20%）** 判离题。零重叠是强信号但过于严格——部分重叠
/// （如 10 条中仅 1 条命中）同样是弱相关。整数比例（`hits * 5 >= len`）避免浮点比较。
const RELEVANT_HIT_RATIO: usize = 5;

/// 查询词重叠相关性门禁（roadmap-result-quality.md P3）：纯词面统计，不做语义判断。
///
/// 背景：Bing 对空格分隔的多词中文查询可能锚定首强实体（如「中国基金 数据 网站 天天
/// 基金网 蛋卷基金 净值查询」→ 恒返回中国维基/百科，9/10 结果离题），数量与类型均
/// 正常，现有数量/占比门禁无法识别——必须让降级链有机会切到下一引擎。
///
/// 规则：
/// - 仅当查询含 ≥2 个原始词且 ≥1 个显著词（长度 ≥3）时判定（单词查询多为导航性/
///   歧义查询，不做相关性判定）；
/// - 显著词过滤「数据/网站/查询」类短泛词，避免「中国政府网」里的「网站」误判命中；
/// - 结果集（标题+摘要+域名，小写）中命中任一显著词的结果占比 < 1/5（20%）
///   → 判离题（`false`）；空结果集不额外拦截（由内容型数量门禁兜底）。
///
/// 单次误判代价 = 多试一个引擎（沿用既有降级链），可接受。
fn relevant(results: &[SearchResult], query: &str) -> bool {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.len() < 2 {
        return true;
    }
    let significant: Vec<&str> = terms
        .iter()
        .copied()
        .filter(|t| t.chars().count() >= 3)
        .collect();
    if significant.is_empty() {
        return true;
    }
    let hits = results
        .iter()
        .filter(|r| {
            let hay = format!(
                "{} {} {}",
                r.title.to_lowercase(),
                r.snippet.to_lowercase(),
                r.domain.to_lowercase()
            );
            significant.iter().any(|t| hay.contains(&t.to_lowercase()))
        })
        .count();
    results.is_empty() || hits * RELEVANT_HIT_RATIO >= results.len()
}

/// 搜索配置（ADR-006 公开面；字段私有，构造与修改只经 [`Config::new`]/builder，保证不变量）。
pub struct Config {
    query: String,
    /// 引擎尝试顺序（首选在前；`Config::new` 按逗号拆分为链，`with_fallback_engines` 追加）。
    engines: Vec<String>,
    browser: BrowserKind,
    max_results: usize,
    timeout: Duration,
    screenshot: Option<PathBuf>,
    dump_html: Option<PathBuf>,
    /// 结果语言（`SearchQuery.lang`）；`None` = 引擎默认。
    lang: Option<String>,
    /// 结果地域/市场（`SearchQuery.region`）；`None` = 引擎默认。
    region: Option<String>,
    /// 翻页聚合页数（`SearchQuery.pages`）；1 = 仅首页。
    pages: usize,
    /// 时间过滤窗口（`SearchQuery.freshness`）；`None` = 不限时间（引擎默认）。
    freshness: Option<Freshness>,
    /// 安全搜索级别（`SearchQuery.safesearch`）；`None` = 引擎默认。
    safesearch: Option<SafesearchLevel>,
    /// 站点过滤（`SearchQuery.site`，query 级 `site:` 语法）；`None` = 不限站点。
    site: Option<String>,
    /// 文件类型过滤（`SearchQuery.filetype`，query 级 `filetype:` 语法）；`None` = 不限类型。
    filetype: Option<String>,
    /// 测试注入用；生产为 `None`，走 `drivers::resolve`。
    driver: Option<Box<dyn BrowserDriver>>,
    /// 外部引擎扩展点：注入自定义 `SearchProvider` 时优先于 `engine` 注册表；生产为 `None`。
    provider: Option<Box<dyn SearchProvider>>,
    /// 瞬时网络错误重试次数（`--retry <n>`；指数退避，封顶）。0 = 不重试（默认）。
    retry: usize,
}

impl Config {
    /// 典型搜索配置：默认 `max_results`/`timeout` 取 `domain::DEFAULT_*`，无调试产物。
    /// `engine` 支持逗号分隔（`"bing,duckduckgo"` = 降级尝试顺序，首选在前）。
    pub fn new(query: impl Into<String>, engine: impl Into<String>, browser: BrowserKind) -> Self {
        Self {
            query: query.into(),
            engines: parse_engine_chain(&engine.into()),
            browser,
            max_results: crate::domain::DEFAULT_MAX_RESULTS,
            timeout: Duration::from_secs(crate::domain::DEFAULT_TIMEOUT_SECS),
            screenshot: None,
            dump_html: None,
            lang: None,
            region: None,
            pages: 1,
            freshness: None,
            safesearch: None,
            site: None,
            filetype: None,
            driver: None,
            provider: None,
            retry: 0,
        }
    }

    pub fn with_max_results(mut self, max_results: usize) -> Self {
        self.max_results = max_results.max(1);
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_screenshot(mut self, path: Option<PathBuf>) -> Self {
        self.screenshot = path;
        self
    }

    pub fn with_dump_html(mut self, path: Option<PathBuf>) -> Self {
        self.dump_html = path;
        self
    }

    /// 测试注入驱动（生产不要调用；优先级高于 `browser`）。
    pub fn with_driver(mut self, driver: Box<dyn BrowserDriver>) -> Self {
        self.driver = Some(driver);
        self
    }

    /// 注入自定义引擎（`SearchProvider` 实现；优先级高于 `engine` 注册表）。
    pub fn with_provider(mut self, provider: Box<dyn SearchProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// 结果语言（如 `zh-hans`，Bing `setlang`；`None` = 引擎默认）。
    pub fn with_lang(mut self, lang: Option<String>) -> Self {
        self.lang = lang;
        self
    }

    /// 结果地域/市场（如 `zh-CN`，Bing `mkt` / DDG `kl`；`None` = 引擎默认）。
    pub fn with_region(mut self, region: Option<String>) -> Self {
        self.region = region;
        self
    }

    /// 翻页聚合页数（≥1，clamp；1 = 仅首页）。
    pub fn with_pages(mut self, pages: usize) -> Self {
        self.pages = pages.max(1);
        self
    }

    /// 时间过滤窗口（如 `Freshness::Week`；`None` = 不限时间，引擎默认）。
    pub fn with_freshness(mut self, freshness: Option<Freshness>) -> Self {
        self.freshness = freshness;
        self
    }

    /// 安全搜索级别（如 `SafesearchLevel::Strict`；`None` = 引擎默认）。
    pub fn with_safesearch(mut self, safesearch: Option<SafesearchLevel>) -> Self {
        self.safesearch = safesearch;
        self
    }

    /// 站点过滤（如 `"doc.rust-lang.org"`，query 级 `site:` 语法；`None` = 不限站点）。
    pub fn with_site(mut self, site: Option<String>) -> Self {
        self.site = site;
        self
    }

    /// 文件类型过滤（如 `"pdf"`，query 级 `filetype:` 语法；`None` = 不限类型）。
    pub fn with_filetype(mut self, filetype: Option<String>) -> Self {
        self.filetype = filetype;
        self
    }

    /// 瞬时网络错误重试次数（指数退避，封顶；0 = 不重试）。
    /// 仅 `Error::Network` 触发重试；验证码/参数错误/超时不重试（避免无意义放大延迟）。
    pub fn with_retry(mut self, retry: usize) -> Self {
        self.retry = retry;
        self
    }

    /// 追加降级引擎（顺序在首选之后；验证码/解析失败/低产时自动尝试）。
    /// 与 `Config::new` 的逗号分隔一致：trim、去空、去重（保持首现）。
    pub fn with_fallback_engines(
        mut self,
        fallbacks: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let mut seen: std::collections::HashSet<String> = self.engines.iter().cloned().collect();
        for name in fallbacks {
            let name = name.into().trim().to_string();
            if !name.is_empty() && seen.insert(name.clone()) {
                self.engines.push(name);
            }
        }
        self
    }
}

/// 正文抓取配置（ADR-009；公开面，字段私有，构造与修改只经 [`FetchConfig::new`]/builder）。
pub struct FetchConfig {
    url: String,
    /// 正文截断上限（字符；`DEFAULT_MAX_CHARS`）。
    max_chars: usize,
    /// 是否返回清洗后正文（false = 只返回 `extracted`，省 token）。
    text: bool,
    /// 结构化字段提取 allowlist（空 = 不提取）。
    extract: Vec<ExtractField>,
    /// SPA 内容等待选择器（导航后轮询该选择器出现再取正文；`None` = 现行为）。
    wait_selector: Option<String>,
    timeout: Duration,
    browser: BrowserKind,
    /// 瞬时网络错误重试次数（`--retry`；指数退避，封顶）。
    retry: usize,
    screenshot: Option<PathBuf>,
    dump_html: Option<PathBuf>,
    /// 测试注入用；生产为 `None`，走 `drivers::resolve`。
    driver: Option<Box<dyn BrowserDriver>>,
}

impl FetchConfig {
    /// 典型抓取配置：默认 `max_chars` 取 `domain::DEFAULT_MAX_CHARS`、默认 `timeout`
    /// 取 `domain::DEFAULT_TIMEOUT_SECS`、返回正文、不提取字段。
    pub fn new(url: impl Into<String>, browser: BrowserKind) -> Self {
        Self {
            url: url.into(),
            max_chars: crate::domain::DEFAULT_MAX_CHARS,
            text: true,
            extract: Vec::new(),
            wait_selector: None,
            timeout: Duration::from_secs(crate::domain::DEFAULT_TIMEOUT_SECS),
            browser,
            retry: 0,
            screenshot: None,
            dump_html: None,
            driver: None,
        }
    }

    /// 正文截断上限（字符，≥1）。
    pub fn with_max_chars(mut self, max_chars: usize) -> Self {
        self.max_chars = max_chars.max(1);
        self
    }

    /// 是否返回清洗后正文（false = 只返回 `extracted`）。
    pub fn with_text(mut self, text: bool) -> Self {
        self.text = text;
        self
    }

    /// 结构化字段提取 allowlist（空 = 不提取）。
    pub fn with_extract(mut self, extract: Vec<ExtractField>) -> Self {
        self.extract = extract;
        self
    }

    /// SPA 内容等待选择器：导航后轮询该选择器出现（预算内）再取正文。
    /// 尽力语义：超时/失败仍返回成功包（正文可能为空，不改变"导航成功即成功包"）。
    /// `None`（默认）= 保持现行为（仅等 `readyState=complete`）。
    pub fn with_wait_selector(mut self, wait_selector: Option<String>) -> Self {
        self.wait_selector = wait_selector;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 瞬时网络错误重试次数（指数退避，封顶；0 = 不重试）。
    /// 仅 `Error::Network` 触发重试（与 [`Config::with_retry`] 同语义）。
    pub fn with_retry(mut self, retry: usize) -> Self {
        self.retry = retry;
        self
    }

    pub fn with_screenshot(mut self, path: Option<PathBuf>) -> Self {
        self.screenshot = path;
        self
    }

    pub fn with_dump_html(mut self, path: Option<PathBuf>) -> Self {
        self.dump_html = path;
        self
    }

    /// 测试注入驱动（生产不要调用；优先级高于 `browser`）。
    pub fn with_driver(mut self, driver: Box<dyn BrowserDriver>) -> Self {
        self.driver = Some(driver);
        self
    }
}

/// 逗号分隔引擎串 → 尝试顺序链（trim、去空、去重保持首现）。
fn parse_engine_chain(s: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    s.split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .filter(|e| seen.insert(e.to_string()))
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub meta: SearchMeta,
}

/// 环境自检结果（design.md §10）：引擎注册表 + 各浏览器后端状态。
/// `Serialize` 供 MCP `doctor` 工具输出（schemas/JSON；CLI `doctor` 为人读文本）。
#[derive(Debug, Serialize)]
pub struct DoctorReport {
    /// 可用引擎（注册表顺序）。
    pub engines: Vec<&'static str>,
    /// 浏览器后端状态（fake/chrome/firefox）。
    pub backends: Vec<BackendStatus>,
}

/// 单个浏览器后端状态。
#[derive(Debug, Serialize)]
pub struct BackendStatus {
    pub kind: BrowserKind,
    /// 找到的二进制路径；`None` 表示未找到。
    pub binary: Option<PathBuf>,
    /// 主版本号；读取失败为 `None`。
    pub major_version: Option<u32>,
    /// 二进制发现失败原因（`binary` 为 `None` 时）。
    pub error: Option<String>,
}

impl DoctorReport {
    /// 收集环境自检信息（同步、无网络；供 CLI `doctor` 与诊断工具复用）。
    pub fn collect() -> Self {
        let backends = [BrowserKind::Fake, BrowserKind::Chrome, BrowserKind::Firefox]
            .into_iter()
            .map(|kind| {
                let (binary, major_version, error) = match crate::drivers::find_browser(kind) {
                    Ok(p) => {
                        let version = crate::drivers::browser_major_version(&p);
                        (Some(p), version, None)
                    }
                    Err(e) => (None, None, Some(e.to_string())),
                };
                BackendStatus {
                    kind,
                    binary,
                    major_version,
                    error,
                }
            })
            .collect();
        Self {
            engines: engines::AVAILABLE.to_vec(),
            backends,
        }
    }
}

/// 同步入口：内部创建 tokio runtime 并阻塞执行一次搜索（CLI/脚本/测试便捷形态）。
///
/// **适用上下文**：无 tokio runtime 的线程——`main`、CLI/脚本、`spawn_blocking` 闭包、
/// 独立线程（blocking 线程内创建新 runtime 是安全的）。
///
/// **不要**在 async 上下文（tokio worker 线程）中调用：会触发「runtime within a runtime」
/// panic。异步场景请直接 [`run`]，或在 async 内同步等待时用
/// `tokio::task::block_in_place(|| handle.block_on(run(cfg)))`（需 multi-thread runtime）。
pub fn search(config: Config) -> Result<Outcome, Error> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| Error::Internal(format!("failed to initialize tokio runtime: {e}")))?;
    runtime.block_on(run(config))
}

/// 执行一次搜索（design.md §6.2 步骤 1-10）。**异步首选入口**。
///
/// 在已有 tokio runtime 的上下文中调用（MCP handler、`tokio::main`、`#[tokio::test]` 等）；
/// 无 runtime 的同步场景用 [`search`]。async 内需要同步阻塞等待时：
///
/// ```rust,no_run
/// # use worbrow::{BrowserKind, Config, run};
/// # let config = Config::new("q", "bing", BrowserKind::Fake);
/// let handle = tokio::runtime::Handle::current();
/// let outcome = tokio::task::block_in_place(|| handle.block_on(run(config))).unwrap();
/// ```
pub async fn run(mut config: Config) -> Result<Outcome, Error> {
    // 2. 选浏览器后端（测试可注入）；`run_with` 借用的 driver 随本函数返回 Drop 回收
    //（design.md §8：成功/错误/超时路径均回收浏览器子进程，防残留）
    let mut driver = match config.driver.take() {
        Some(d) => d,
        None => crate::drivers::resolve(config.browser).await?,
    };
    run_with(&mut *driver, config).await
}

/// 使用**外部注入**驱动的搜索编排（design.md §6.2 步骤 1-10）。
///
/// 与 [`run`] 共用同一编排；区别：driver 为借用（生命周期归调用方），供
/// MCP 会话池复用浏览器进程（[`crate::drivers::SessionPool`]，roadmap-session-pool.md）。
/// 调用方负责回收：失败/超时后按错误类型标记会话健康，Drop 时归还或丢弃。
pub(crate) async fn run_with(
    driver: &mut dyn BrowserDriver,
    config: Config,
) -> Result<Outcome, Error> {
    // 1. 解析并校验 query
    let text = config.query.trim();
    if text.is_empty() {
        return Err(Error::Cli("empty search query".into()));
    }
    if text.chars().count() > 512 {
        return Err(Error::Cli("search query too long (>512 characters)".into()));
    }
    // 空/全空引擎串（如 `--engine ""`）→ 参数错误（exit 2），而非内部错误
    if config.engines.is_empty() {
        return Err(Error::Cli("no engine specified".into()));
    }

    let started_at = Utc::now();
    let timer = Instant::now();

    let query = SearchQuery {
        text: text.to_string(),
        max_results: config.max_results.max(1),
        lang: config.lang.clone(),
        region: config.region.clone(),
        pages: config.pages.max(1),
        freshness: config.freshness,
        safesearch: config.safesearch,
        site: config.site.clone(),
        filetype: config.filetype.clone(),
    };

    // 4-8. 包整体硬超时 + 瞬时网络错误退避重试（`--retry <n>`；全局 timeout 兜底，
    // 退避计入预算内，避免重试无限放大延迟）。
    // 仅 `Error::Network` 触发重试；验证码/参数错误/解析失败（有降级链）不重试。
    let retries = config.retry;
    let outcome = timeout(config.timeout, async {
        let mut attempt = 0usize;
        loop {
            match search_attempt(driver, &config, &query, config.timeout).await {
                Ok(v) => break Ok((v, attempt)),
                Err(e @ Error::Network(_)) if attempt < retries => {
                    attempt += 1;
                    let delay = backoff_delay(attempt);
                    tracing::warn!(
                        attempt,
                        ?delay,
                        "transient network error, backing off and retrying: {e}"
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(e) => break Err(e),
            }
        }
    })
    .await;
    let (engine, html, results, captcha, fetched_pages, low_yield, engine_tried, retried) =
        match outcome {
            Ok(Ok((v, retried))) => (v.0, v.1, v.2, v.3, v.4, v.5, v.6, retried),
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(Error::Timeout("task timed out".into())),
        };

    // 9. 可选调试产物（失败仅告警，不影响主流程）
    if let Some(path) = config.screenshot.as_deref()
        && let Err(e) = driver.screenshot(path).await
    {
        tracing::warn!("failed to save screenshot {path:?}: {e}");
    }
    if let Some(path) = config.dump_html.as_deref()
        && let Err(e) = std::fs::write(path, &html)
    {
        tracing::warn!("failed to save HTML {path:?}: {e}");
    }

    // 10. 组装 Outcome
    let elapsed_ms = timer.elapsed().as_millis() as u64;
    let result_count = results.len();
    let meta = SearchMeta {
        engine,
        started_at,
        elapsed_ms,
        result_count,
        pages: fetched_pages,
        low_yield,
        captcha,
        engine_error: None,
        engine_tried,
        cached: false, // CLI/MCP 非缓存路径恒 false；MCP 缓存命中由缓存层构造
        retries: retried,
    };

    Ok(Outcome {
        query: text.to_string(),
        results,
        meta,
    })
}

/// 同步入口：内部创建 tokio runtime 并阻塞执行一次正文抓取（CLI/脚本便捷形态）。
///
/// **适用上下文**：无 tokio runtime 的线程（同 [`search`] 语义）；
/// 异步上下文请直接用 [`run_fetch`]。
pub fn fetch(config: FetchConfig) -> Result<FetchedPage, Error> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| Error::Internal(format!("failed to initialize tokio runtime: {e}")))?;
    runtime.block_on(run_fetch(config))
}

/// 执行一次正文抓取（ADR-009）。**异步首选入口**：自建 driver（CLI/独立调用）。
pub async fn run_fetch(mut config: FetchConfig) -> Result<FetchedPage, Error> {
    // URL 校验前置：非法 URL 直接参数错误，不启动浏览器（CLI 无浏览器环境也能自检）
    let _ = normalize_fetch_url(&config.url)?;
    let mut driver = match config.driver.take() {
        Some(d) => d,
        None => crate::drivers::resolve(config.browser).await?,
    };
    run_fetch_with(&mut *driver, config).await
}

/// 使用**外部注入**驱动的抓取编排（MCP 会话池复用浏览器进程，镜像 [`run_with`]）。
/// 调用方负责回收：网络/超时错误后按错误类型标记会话健康，Drop 时归还或丢弃。
pub(crate) async fn run_fetch_with(
    driver: &mut dyn BrowserDriver,
    config: FetchConfig,
) -> Result<FetchedPage, Error> {
    // 1. URL 校验与归一化（scheme 缺失自动补 https://；非 http/https → 参数错误）
    let url = normalize_fetch_url(&config.url)?;
    let started_at = Utc::now();
    let timer = Instant::now();

    // 2. 全局硬超时 + 瞬时网络错误退避重试（仅 Error::Network，同 search 语义）
    let retries = config.retry;
    let result = timeout(config.timeout, async {
        let mut attempt = 0usize;
        loop {
            match fetch_attempt(
                driver,
                &url,
                config.timeout,
                config.wait_selector.as_deref(),
            )
            .await
            {
                Ok(v) => break Ok((v, attempt)),
                Err(e @ Error::Network(_)) if attempt < retries => {
                    attempt += 1;
                    let delay = backoff_delay(attempt);
                    tracing::warn!(
                        attempt,
                        ?delay,
                        "transient network error, backing off and retrying: {e}"
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(e) => break Err(e),
            }
        }
    })
    .await;
    let (html, final_url, http_status) = match result {
        Ok(Ok((v, _))) => v,
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(Error::Timeout("task timed out".into())),
    };

    // 3. 提取正文/结构化字段（同一份 HTML 二次解析，不重复导航）
    let (text, truncated) = if config.text {
        crate::extract::extract_main_text(&html, config.max_chars)
    } else {
        (String::new(), false)
    };
    let extracted = crate::extract::extract_fields(&html, &config.extract);

    // 4. 可选调试产物（失败仅告警，不影响主流程）
    if let Some(path) = config.screenshot.as_deref()
        && let Err(e) = driver.screenshot(path).await
    {
        tracing::warn!("failed to save screenshot {path:?}: {e}");
    }
    if let Some(path) = config.dump_html.as_deref()
        && let Err(e) = std::fs::write(path, &html)
    {
        tracing::warn!("failed to save HTML {path:?}: {e}");
    }

    // 5. 组装 FetchedPage
    let chars = text.chars().count();
    Ok(FetchedPage {
        url: url.to_string(),
        fetched_at: started_at,
        text,
        extracted,
        elapsed_ms: timer.elapsed().as_millis() as u64,
        chars,
        truncated,
        final_url,
        http_status,
    })
}

/// 单次抓取尝试：导航 → 等待加载（尽力）→（可选 SPA 选择器等待）→ 取 HTML →
/// 读重定向落地页与 HTTP 状态码。
async fn fetch_attempt(
    driver: &mut dyn BrowserDriver,
    url: &url::Url,
    timeout_dur: Duration,
    wait_selector: Option<&str>,
) -> Result<(String, Option<String>, Option<u16>), Error> {
    driver.navigate(url.clone()).await?;
    // 等待加载：尽力语义（预算耗尽/eval 失败均不报错，导航成功即成功包）
    wait_load(driver, timeout_dur.min(WAIT_BUDGET)).await;
    // SPA 内容等待（可选）：显式选择器出现后再取正文；尽力语义——超时/失败
    // 仍继续抓取（正文可能为空），不改变"导航成功即成功包"契约（README 已知行为）
    if let Some(selector) = wait_selector {
        let _ = driver
            .wait_for(selector, timeout_dur.min(WAIT_BUDGET))
            .await;
    }
    let html = driver.html().await?;
    let final_url = driver
        .eval("location.href")
        .await
        .ok()
        .and_then(|v| v.as_str().map(str::to_string));
    let http_status = driver
        .eval(HTTP_STATUS_JS)
        .await
        .ok()
        .and_then(|v| v.as_u64())
        .map(|s| s as u16);
    Ok((html, final_url, http_status))
}

/// 等待页面加载完成（尽力语义）：轮询 `document.readyState == "complete"`；
/// 预算耗尽或 eval 失败（如 fake 驱动/受限页）即返回，不报错。
async fn wait_load(driver: &mut dyn BrowserDriver, budget: Duration) {
    let deadline = Instant::now() + budget;
    loop {
        match driver.eval("document.readyState").await {
            Ok(v) if v.as_str() == Some("complete") => return,
            Ok(_) => {}
            // 无法评估 → 视为已加载（不阻塞抓取）
            Err(_) => return,
        }
        if Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(LOAD_POLL).await;
    }
}

/// 归一化抓取 URL：缺 scheme 自动补 `https://`（与浏览器行为一致）；协议相对 URL
/// （`//example.com`）显式补 `https:`；去 fragment；仅放行 http/https。
/// 非法（空/无法解析/非 http/https）→ `Error::Cli`（参数错误，exit 2）。
/// `pub(crate)`：CLI 与 MCP 共用前置校验（MCP 在 acquire 会话前调用，非法 URL 不启动浏览器）。
pub(crate) fn normalize_fetch_url(raw: &str) -> Result<url::Url, Error> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(Error::Cli("empty URL".into()));
    }
    // 协议相对 URL（//example.com）→ https://example.com（与引擎 normalize_url 一致）
    let raw = if let Some(rest) = raw.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        raw.to_string()
    };
    // 无 scheme 时按浏览器行为补 https://（如 `example.com/foo` → `https://example.com/foo`）
    let candidate = if raw.contains("://") {
        raw
    } else {
        format!("https://{raw}")
    };
    let mut url =
        url::Url::parse(&candidate).map_err(|e| Error::Cli(format!("invalid URL: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::Cli(format!(
            "unsupported URL scheme: {} (only http/https supported)",
            url.scheme()
        )));
    }
    url.set_fragment(None);
    Ok(url)
}

/// 单次完整搜索尝试的结果元组（engine, html, results, captcha, pages, low_yield, tried）。
type SearchAttempt = (
    &'static str,
    String,
    Vec<SearchResult>,
    bool,
    usize,
    bool,
    Vec<String>,
);

/// 单次完整搜索尝试（含引擎降级链）：注入引擎单引擎（无降级）；否则内置注册表
/// 降级循环（验证码阻止/解析失败/低产 → 尝试下一引擎）。
/// `Error::Network` 交由上层退避重试循环处理。
///
/// `timeout_dur` 用于等待预算（整体超时在调用方）。
async fn search_attempt(
    driver: &mut dyn BrowserDriver,
    config: &Config,
    query: &SearchQuery,
    timeout_dur: Duration,
) -> Result<SearchAttempt, Error> {
    match &config.provider {
        Some(provider) => {
            let name = provider.name();
            let (html, results, captcha, pages) =
                search_one(&**provider, query, driver, timeout_dur).await?;
            let low_yield = content_count(&results) < LOW_YIELD_THRESHOLD;
            Ok((
                name,
                html,
                results,
                captcha,
                pages,
                low_yield,
                vec![name.to_string()],
            ))
        }
        None => search_engine_chain(driver, config, query, timeout_dur).await,
    }
}

/// 引擎注册表降级循环（design.md §6.2 步骤 4a-4e）：按 `config.engines` 顺序尝试，
/// 验证码阻止/解析失败/低产 → 下一引擎；全低产用最高产候选兜底。
/// `Error::Network`/超时不降级，直接返回（交由重试循环，避免放大总耗时）。
async fn search_engine_chain(
    driver: &mut dyn BrowserDriver,
    config: &Config,
    query: &SearchQuery,
    timeout_dur: Duration,
) -> Result<SearchAttempt, Error> {
    let mut tried: Vec<String> = Vec::new();
    // 低产候选兜底（取最高产）；引擎名恒为 `&'static str`（trait 签名）
    let mut candidate: Option<(&'static str, Vec<SearchResult>, bool, usize, String)> = None;
    let mut last_error: Option<Error> = None;

    for name in &config.engines {
        let provider = engines::resolve(name)?;
        tried.push(provider.name().to_string());
        tracing::info!(engine = provider.name(), "trying engine");

        if let Some((engine, html, results, captcha, pages, low_yield)) = handle_engine_result(
            search_one(&*provider, query, driver, timeout_dur).await,
            &*provider,
            query,
            &mut candidate,
            &mut last_error,
        )
        .await?
        {
            return Ok((engine, html, results, captcha, pages, low_yield, tried));
        }
    }

    // 全部尝试完：有低产候选则兜底成功；否则返回最后错误（captcha 优先）
    if let Some((engine, results, captcha, pages, html)) = candidate {
        return Ok((engine, html, results, captcha, pages, true, tried));
    }
    match last_error {
        Some(err) => Err(err),
        None => Err(Error::Internal("engine list is empty".into())),
    }
}

/// 处理单引擎结果：满意 → `Some((engine, html, results, captcha, pages, low_yield))` 采用；
/// 低产/验证码/解析失败 → `None`（记录候选或错误，继续降级）；网络/超时 → `Err`。
#[allow(clippy::type_complexity)]
async fn handle_engine_result(
    result: Result<(String, Vec<SearchResult>, bool, usize), Error>,
    provider: &dyn SearchProvider,
    query: &SearchQuery,
    candidate: &mut Option<(&'static str, Vec<SearchResult>, bool, usize, String)>,
    last_error: &mut Option<Error>,
) -> Result<Option<(&'static str, String, Vec<SearchResult>, bool, usize, bool)>, Error> {
    match result {
        Ok((html, results, captcha, pages)) => {
            // 满意：内容型（web）结果集满请求量或非低产（≥ 阈值），web 占比 ≥ 50%，
            // 且与查询词有重叠（P3 相关性门禁，roadmap-result-quality.md：离题但
            // 类型正常的 Web 结果同样降级）
            let content = content_count(&results);
            let web_ratio_ok = content * 2 >= results.len();
            let relevance_ok = relevant(&results, &query.text);
            let satisfied = web_ratio_ok
                && relevance_ok
                && (content >= query.max_results || content >= LOW_YIELD_THRESHOLD);
            if satisfied {
                tracing::info!(
                    engine = provider.name(),
                    count = results.len(),
                    "adopting engine"
                );
                let low_yield = content < LOW_YIELD_THRESHOLD;
                return Ok(Some((
                    provider.name(),
                    html,
                    results,
                    captcha,
                    pages,
                    low_yield,
                )));
            }
            // 低产/低质：保留最高产（按内容型数）候选，继续尝试下一引擎
            let better = match candidate {
                Some((_, cur, ..)) => content > content_count(cur),
                None => true,
            };
            if better {
                *candidate = Some((provider.name(), results, captcha, pages, html));
            }
            tracing::warn!(
                engine = provider.name(),
                "low yield/low quality, keep candidate and try more engines"
            );
            Ok(None)
        }
        Err(Error::Captcha(e)) => {
            *last_error = Some(Error::Captcha(e));
            tracing::warn!(engine = provider.name(), "captcha blocked, degrading");
            Ok(None)
        }
        Err(Error::Engine(e)) => {
            tracing::warn!(engine = provider.name(), code = %e.code, "parse failed, degrading");
            *last_error = Some(Error::Engine(e));
            Ok(None)
        }
        // 网络/超时等错误不降级，直接返回（避免放大总耗时，交由重试循环）
        Err(e) => Err(e),
    }
}

/// 指数退避：第 `attempt`（从 1 起）次重试延迟 = 2^(attempt-1) 秒（1s/2s/4s/8s 封顶）。
fn backoff_delay(attempt: usize) -> Duration {
    Duration::from_secs(1 << attempt.saturating_sub(1).min(3))
}

/// 单引擎搜索（design.md §6.2 步骤 5-8）：翻页聚合、captcha 检测、按 URL 去重合并。
///
/// 验证码阻止且无结果 → `Error::Captcha`；页面解析失败 → `Error::Engine`
/// （两者由上层降级循环处理）。`timeout_dur` 仅用于等待预算（整体超时在调用方）。
async fn search_one(
    provider: &dyn SearchProvider,
    query: &SearchQuery,
    driver: &mut dyn BrowserDriver,
    timeout_dur: Duration,
) -> Result<(String, Vec<SearchResult>, bool, usize), Error> {
    let wait_budget = timeout_dur.min(WAIT_BUDGET);
    let mut seen = std::collections::HashSet::new();
    // 同域名去重计数（P2）：未启用 site: 过滤时，同一域名最多保留 DOMAIN_LIMIT 条
    let mut domain_count: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut all = Vec::new();
    let mut captcha = false;
    let mut fetched_pages = 0usize;
    let mut last_html = String::new();

    for page in 1..=query.pages {
        fetched_pages += 1;
        let (html, page_captcha, results) =
            fetch_page(provider, query, driver, page, wait_budget).await?;
        captcha |= page_captcha;
        last_html = html;
        // 抽取并去重合并：先按 URL，再按域名截断（防单一来源刷屏）
        for r in results {
            if !seen.insert(r.url.clone()) {
                continue;
            }
            if query.site.is_none() && !r.domain.is_empty() {
                let n = domain_count.entry(r.domain.clone()).or_insert(0);
                if *n >= DOMAIN_LIMIT {
                    continue; // 该域名已达上限，丢弃（rank 靠前已保留）
                }
                *n += 1;
            }
            all.push(r);
        }
        // 已集满 max_results 可提前停止翻页
        if all.len() >= query.max_results {
            break;
        }
    }

    if captcha && all.is_empty() {
        return Err(Error::Captcha(
            "captcha detected and no results obtained".into(),
        ));
    }

    // 去重后重排 rank 并截断
    for (i, r) in all.iter_mut().enumerate() {
        r.rank = i + 1;
    }
    all.truncate(query.max_results);

    Ok((last_html, all, captcha, fetched_pages))
}

/// 抓取单页：navigate → 等待结果容器 → html → 验证码检测 → 解析。
/// 返回 `(html, 是否检测到验证码, 本页结果)`。
async fn fetch_page(
    provider: &dyn SearchProvider,
    query: &SearchQuery,
    driver: &mut dyn BrowserDriver,
    page: usize,
    wait_budget: Duration,
) -> Result<(String, bool, Vec<SearchResult>), Error> {
    let url = if page == 1 {
        provider.result_url(query)
    } else {
        provider.page_url(query, page)
    };
    let step = Instant::now();
    driver.navigate(url).await?;
    tracing::info!(
        elapsed_ms = step.elapsed().as_millis() as u64,
        page,
        "navigate done"
    );

    // 等待结果容器出现：二级超时（页面加载预算内截断，design.md §6.2）
    let step = Instant::now();
    driver
        .wait_for(provider.result_selector(), wait_budget)
        .await?;
    tracing::info!(
        elapsed_ms = step.elapsed().as_millis() as u64,
        page,
        "wait_for done"
    );

    let step = Instant::now();
    let html = driver.html().await?;
    tracing::info!(
        elapsed_ms = step.elapsed().as_millis() as u64,
        page,
        "html done"
    );

    // 验证码启发式检测（不中止）
    let lower = html.to_lowercase();
    let captcha = provider
        .captcha_heuristics()
        .iter()
        .any(|h| lower.contains(h));

    let results = provider.parse(&html)?;
    Ok((html, captcha, results))
}

#[cfg(test)]
// 测试断言序列（assert_eq 宏展开）非控制流复杂度，豁免门禁；生产代码仍严格 ≤10
#[allow(clippy::cognitive_complexity)]
mod tests {
    use super::*;

    #[test]
    fn config_new_applies_defaults() {
        let c = Config::new("q", "bing", BrowserKind::Fake);
        assert_eq!(c.max_results, crate::domain::DEFAULT_MAX_RESULTS);
        assert_eq!(
            c.timeout,
            Duration::from_secs(crate::domain::DEFAULT_TIMEOUT_SECS)
        );
        assert!(c.screenshot.is_none());
        assert!(c.dump_html.is_none());
        assert!(c.lang.is_none());
        assert!(c.region.is_none());
        assert_eq!(c.pages, 1);
        assert_eq!(c.engines, vec!["bing".to_string()]);
        assert!(c.driver.is_none());
        assert!(c.provider.is_none());
    }

    #[test]
    fn config_builder_overrides_and_clamps() {
        let c = Config::new("q", "bing", BrowserKind::Fake)
            .with_max_results(0)
            .with_timeout(Duration::from_secs(5));
        assert_eq!(c.max_results, 1, "max_results 至少为 1");
        assert_eq!(c.timeout, Duration::from_secs(5));
    }

    #[test]
    fn engine_chain_parses_comma_separated_order() {
        assert_eq!(
            parse_engine_chain("bing,duckduckgo"),
            vec!["bing", "duckduckgo"]
        );
        // trim / 去空 / 去重（保持首现）
        assert_eq!(
            parse_engine_chain(" bing ,,duckduckgo, bing "),
            vec!["bing", "duckduckgo"]
        );
        assert!(parse_engine_chain("").is_empty());
    }

    #[test]
    fn with_fallback_engines_appends_deduplicated() {
        let c = Config::new("q", "bing", BrowserKind::Fake).with_fallback_engines([
            "duckduckgo",
            "bing",
            "duckduckgo",
        ]);
        assert_eq!(c.engines, vec!["bing", "duckduckgo"]);
    }

    /// `run_with`（外部注入驱动）与 `run`（自建驱动）编排等价：同 config 同 fixture
    /// 结果一致（roadmap-session-pool.md §5 app 集成验证）。
    #[tokio::test]
    async fn run_with_matches_run_on_same_fixture() {
        // run：内部 resolve FakeDriver（SMOKE_HTML）
        let cfg_run = Config::new("rust async", "bing", BrowserKind::Fake)
            .with_max_results(5)
            .with_timeout(Duration::from_secs(5));
        let via_run = run(cfg_run).await.expect("run 应成功");

        // run_with：注入同 fixture 的 FakeDriver（drivers::resolve(Fake) = SMOKE_HTML）
        let mut driver = crate::drivers::resolve(BrowserKind::Fake)
            .await
            .expect("resolve Fake 应成功");
        let cfg_run_with = Config::new("rust async", "bing", BrowserKind::Fake)
            .with_max_results(5)
            .with_timeout(Duration::from_secs(5));
        let via_run_with = run_with(&mut *driver, cfg_run_with)
            .await
            .expect("run_with 应成功");

        assert_eq!(via_run.query, via_run_with.query);
        assert_eq!(via_run.results, via_run_with.results);
        assert_eq!(via_run.meta.engine, via_run_with.meta.engine);
        assert_eq!(via_run.meta.result_count, via_run_with.meta.result_count);
    }

    /// `run_with` 错误路径不泄漏驱动状态：网络错误返回后驱动仍可再用于下一次搜索
    /// （会话池健康判定的前提——错误只标记会话，不破坏驱动对象）。
    #[tokio::test]
    async fn run_with_network_error_leaves_driver_usable() {
        /// 首次 navigate 返回网络错误、之后正常的驱动（模拟浏览器连接抖动）。
        struct FlakyDriver {
            fail_first: bool,
        }
        #[async_trait::async_trait]
        impl crate::ports::BrowserDriver for FlakyDriver {
            async fn navigate(&mut self, _url: url::Url) -> Result<(), Error> {
                if self.fail_first {
                    self.fail_first = false;
                    return Err(Error::Network("模拟连接断开".into()));
                }
                Ok(())
            }
            async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
                Ok(())
            }
            async fn html(&self) -> Result<String, Error> {
                Ok(include_str!("../tests/fixtures/bing.html").to_string())
            }
            async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
                Ok(serde_json::Value::Null)
            }
            async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
                Ok(())
            }
        }

        let mut driver = FlakyDriver { fail_first: true };
        // 第一次：navigate 网络错误 → 引擎降级循环直接返回（不吞错误）
        let cfg = Config::new("q", "bing", BrowserKind::Fake).with_timeout(Duration::from_secs(5));
        let err = run_with(&mut driver, cfg)
            .await
            .expect_err("navigate 错误应返回");
        assert!(matches!(err, Error::Network(_)));
        // 第二次正常搜索仍可用（驱动对象未被 run_with 消耗/破坏）
        let cfg2 = Config::new("rust", "bing", BrowserKind::Fake)
            .with_max_results(3)
            .with_timeout(Duration::from_secs(5));
        let outcome = run_with(&mut driver, cfg2).await.expect("复用驱动应成功");
        assert!(!outcome.results.is_empty());
    }

    /// 退避延迟：指数序列 1s/2s/4s/8s 封顶。
    #[test]
    fn backoff_delay_is_exponential_capped() {
        assert_eq!(backoff_delay(1), Duration::from_secs(1));
        assert_eq!(backoff_delay(2), Duration::from_secs(2));
        assert_eq!(backoff_delay(3), Duration::from_secs(4));
        assert_eq!(backoff_delay(4), Duration::from_secs(8));
        assert_eq!(backoff_delay(10), Duration::from_secs(8), "封顶 8s");
    }

    /// `--retry`：瞬时网络错误退避重试后成功，`meta.retries` 记录实际重试次数。
    #[tokio::test]
    async fn retry_recovers_from_transient_network_error() {
        /// 前 `failures` 次 navigate 返回网络错误，之后正常（模拟瞬时网络抖动）。
        struct FlakyDriver {
            failures: usize,
        }
        #[async_trait::async_trait]
        impl crate::ports::BrowserDriver for FlakyDriver {
            async fn navigate(&mut self, _url: url::Url) -> Result<(), Error> {
                if self.failures > 0 {
                    self.failures -= 1;
                    return Err(Error::Network("模拟瞬时网络错误".into()));
                }
                Ok(())
            }
            async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
                Ok(())
            }
            async fn html(&self) -> Result<String, Error> {
                Ok(include_str!("../tests/fixtures/bing.html").to_string())
            }
            async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
                Ok(serde_json::Value::Null)
            }
            async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
                Ok(())
            }
        }

        // 失败 1 次 + retry=2 → 第 2 次尝试成功；meta.retries=1
        let mut driver = FlakyDriver { failures: 1 };
        let cfg = Config::new("rust", "bing", BrowserKind::Fake)
            .with_max_results(3)
            .with_timeout(Duration::from_secs(30))
            .with_retry(2);
        let outcome = run_with(&mut driver, cfg).await.expect("重试后应成功");
        assert_eq!(outcome.meta.retries, 1, "失败 1 次 → 实际重试 1 次");
        assert!(!outcome.results.is_empty());
    }

    /// 重试耗尽：网络错误持续 → 返回 Network 错误（不无限重试）。
    #[tokio::test]
    async fn retry_exhausted_returns_network_error() {
        struct AlwaysFail;
        #[async_trait::async_trait]
        impl crate::ports::BrowserDriver for AlwaysFail {
            async fn navigate(&mut self, _url: url::Url) -> Result<(), Error> {
                Err(Error::Network("模拟持续网络错误".into()))
            }
            async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
                Ok(())
            }
            async fn html(&self) -> Result<String, Error> {
                Ok(String::new())
            }
            async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
                Ok(serde_json::Value::Null)
            }
            async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
                Ok(())
            }
        }

        let mut driver = AlwaysFail;
        let cfg = Config::new("q", "bing", BrowserKind::Fake)
            .with_timeout(Duration::from_secs(30))
            .with_retry(1);
        // 首次失败 + 退避 1s + 重试失败 = 总计约 1s+，断言错误类型即可
        let err = run_with(&mut driver, cfg)
            .await
            .expect_err("持续网络错误应返回");
        assert!(matches!(err, Error::Network(_)));
    }

    /// 非网络错误不触发重试：验证码阻止直接返回（不放大延迟）。
    #[tokio::test]
    async fn captcha_error_is_not_retried() {
        struct CaptchaDriver;
        #[async_trait::async_trait]
        impl crate::ports::BrowserDriver for CaptchaDriver {
            async fn navigate(&mut self, _url: url::Url) -> Result<(), Error> {
                Err(Error::Captcha("模拟验证码".into()))
            }
            async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
                Ok(())
            }
            async fn html(&self) -> Result<String, Error> {
                Ok(String::new())
            }
            async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
                Ok(serde_json::Value::Null)
            }
            async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
                Ok(())
            }
        }

        let mut driver = CaptchaDriver;
        let cfg = Config::new("q", "bing", BrowserKind::Fake)
            .with_timeout(Duration::from_secs(30))
            .with_retry(3);
        let err = run_with(&mut driver, cfg)
            .await
            .expect_err("验证码错误应直接返回");
        assert!(matches!(err, Error::Captcha(_)));
    }

    /// 质量降级（roadmap-result-quality.md 核心）：Bing 返回全词典污染（高产低质，
    /// 旧判定 `results.len() >= 3` 会误判满意）→ 内容型 0 → 自动降级 DuckDuckGo。
    #[tokio::test]
    async fn dictionary_pollution_triggers_engine_fallback() {
        /// Bing 风格页面：全部结果指向 iciba 词典释义（真实污染形态）。
        const DICT_POLLUTION_HTML: &str = r#"<html><body><ol id="b_results">
          <li class="b_algo"><h2><a href="https://www.iciba.com/word?w=best">best 的翻译</a></h2><div class="b_caption"><p>词典释义一</p></div></li>
          <li class="b_algo"><h2><a href="https://www.iciba.com/word?w=learn">learn 的翻译</a></h2><div class="b_caption"><p>词典释义二</p></div></li>
          <li class="b_algo"><h2><a href="https://www.iciba.com/word?w=rust">rust 的翻译</a></h2><div class="b_caption"><p>词典释义三</p></div></li>
          <li class="b_algo"><h2><a href="https://www.iciba.com/word?w=tutorial">tutorial 的翻译</a></h2><div class="b_caption"><p>词典释义四</p></div></li>
        </ol></body></html>"#;

        /// 按导航 URL 返回不同页面：bing → 全词典污染；ddg → 正常内容 fixture。
        struct PollutingDriver {
            current: Option<String>,
        }
        #[async_trait::async_trait]
        impl crate::ports::BrowserDriver for PollutingDriver {
            async fn navigate(&mut self, url: url::Url) -> Result<(), Error> {
                self.current = Some(url.to_string());
                Ok(())
            }
            async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
                Ok(())
            }
            async fn html(&self) -> Result<String, Error> {
                let url = self.current.as_deref().unwrap_or_default();
                if url.contains("bing.com") {
                    Ok(DICT_POLLUTION_HTML.to_string())
                } else {
                    Ok(include_str!("../tests/fixtures/duckduckgo.html").to_string())
                }
            }
            async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
                Ok(serde_json::Value::Null)
            }
            async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
                Ok(())
            }
        }

        let mut driver = PollutingDriver { current: None };
        let cfg = Config::new("best rust tutorial", "bing,duckduckgo", BrowserKind::Fake)
            .with_max_results(5)
            .with_timeout(Duration::from_secs(10));
        let outcome = run_with(&mut driver, cfg).await.expect("降级后应成功");
        // 词典污染不满足 → 降级到 ddg 并采用其内容型结果
        assert_eq!(outcome.meta.engine, "duckduckgo");
        assert_eq!(outcome.meta.engine_tried, vec!["bing", "duckduckgo"]);
        assert!(!outcome.meta.low_yield, "ddg 3 条内容型 ≥ 阈值");
        assert!(
            outcome
                .results
                .iter()
                .all(|r| r.result_kind == crate::domain::ResultKind::Web),
            "采用的结果应全部为内容型"
        );
    }

    /// 回归：首引擎内容型结果足够（≥ 阈值）→ 不降级、不误报低产。
    #[tokio::test]
    async fn content_results_do_not_trigger_fallback() {
        let mut driver = crate::drivers::resolve(BrowserKind::Fake)
            .await
            .expect("resolve Fake 应成功");
        let cfg = Config::new("rust", "bing", BrowserKind::Fake)
            .with_max_results(5)
            .with_timeout(Duration::from_secs(5));
        let outcome = run_with(&mut *driver, cfg).await.expect("应成功");
        assert_eq!(outcome.meta.engine, "bing");
        assert_eq!(outcome.meta.engine_tried, vec!["bing"], "不应降级");
        assert!(!outcome.meta.low_yield, "3 条内容型 ≥ 阈值，不应低产");
    }

    /// P2 同质化检测（roadmap-result-quality.md）：内容型数量达标（3 ≥ 阈值）但
    /// web 占比 < 50%（多义混入 7 条词典/翻译）→ 仍视为低质，自动降级下一引擎。
    #[tokio::test]
    async fn homogeneous_pollution_triggers_fallback() {
        /// Bing 风格页面：3 条不同域内容页 + 7 条不同域词典/翻译页（占比 30%）。
        const MIXED_POLLUTION_HTML: &str = r#"<html><body><ol id="b_results">
          <li class="b_algo"><h2><a href="https://example.com/a">内容一</a></h2><div class="b_caption"><p>摘要</p></div></li>
          <li class="b_algo"><h2><a href="https://example.org/b">内容二</a></h2><div class="b_caption"><p>摘要</p></div></li>
          <li class="b_algo"><h2><a href="https://example.net/c">内容三</a></h2><div class="b_caption"><p>摘要</p></div></li>
          <li class="b_algo"><h2><a href="https://www.iciba.com/word?w=best">词典一</a></h2><div class="b_caption"><p>词典</p></div></li>
          <li class="b_algo"><h2><a href="https://dictionary.cambridge.org/dictionary/english/best">词典二</a></h2><div class="b_caption"><p>词典</p></div></li>
          <li class="b_algo"><h2><a href="https://dict.eudic.net/dicts/en/best">词典三</a></h2><div class="b_caption"><p>词典</p></div></li>
          <li class="b_algo"><h2><a href="https://fanyi.baidu.com/#en/zh/best">词典四</a></h2><div class="b_caption"><p>词典</p></div></li>
          <li class="b_algo"><h2><a href="https://fanyi.so/dict/?q=best">词典五</a></h2><div class="b_caption"><p>词典</p></div></li>
          <li class="b_algo"><h2><a href="https://translate.yandex.com/">词典六</a></h2><div class="b_caption"><p>词典</p></div></li>
          <li class="b_algo"><h2><a href="https://www.ichacha.net/dict?w=best">词典七</a></h2><div class="b_caption"><p>词典</p></div></li>
        </ol></body></html>"#;

        /// 按导航 URL 返回不同页面：bing → 混合污染；ddg → 正常内容 fixture。
        struct MixedDriver {
            current: Option<String>,
        }
        #[async_trait::async_trait]
        impl crate::ports::BrowserDriver for MixedDriver {
            async fn navigate(&mut self, url: url::Url) -> Result<(), Error> {
                self.current = Some(url.to_string());
                Ok(())
            }
            async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
                Ok(())
            }
            async fn html(&self) -> Result<String, Error> {
                let url = self.current.as_deref().unwrap_or_default();
                if url.contains("bing.com") {
                    Ok(MIXED_POLLUTION_HTML.to_string())
                } else {
                    Ok(include_str!("../tests/fixtures/duckduckgo.html").to_string())
                }
            }
            async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
                Ok(serde_json::Value::Null)
            }
            async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
                Ok(())
            }
        }

        let mut driver = MixedDriver { current: None };
        let cfg = Config::new("best rust", "bing,duckduckgo", BrowserKind::Fake)
            .with_timeout(Duration::from_secs(10));
        let outcome = run_with(&mut driver, cfg).await.expect("占比低质应降级");
        // 3 条内容型数量达标但占比 30% < 50% → 降级 ddg
        assert_eq!(outcome.meta.engine, "duckduckgo");
        assert_eq!(outcome.meta.engine_tried, vec!["bing", "duckduckgo"]);
    }

    /// P2 同域名去重：同一域名最多保留 DOMAIN_LIMIT 条（rank 靠前优先），防刷屏。
    #[tokio::test]
    async fn domain_flooding_is_capped() {
        /// Bing 风格页面：4 条 example.com + 1 条 example.org。
        const FLOOD_HTML: &str = r#"<html><body><ol id="b_results">
          <li class="b_algo"><h2><a href="https://example.com/1">同域一</a></h2><div class="b_caption"><p>摘要</p></div></li>
          <li class="b_algo"><h2><a href="https://example.com/2">同域二</a></h2><div class="b_caption"><p>摘要</p></div></li>
          <li class="b_algo"><h2><a href="https://example.com/3">同域三</a></h2><div class="b_caption"><p>摘要</p></div></li>
          <li class="b_algo"><h2><a href="https://example.com/4">同域四</a></h2><div class="b_caption"><p>摘要</p></div></li>
          <li class="b_algo"><h2><a href="https://example.org/a">异域</a></h2><div class="b_caption"><p>摘要</p></div></li>
        </ol></body></html>"#;

        struct FloodDriver;
        #[async_trait::async_trait]
        impl crate::ports::BrowserDriver for FloodDriver {
            async fn navigate(&mut self, _url: url::Url) -> Result<(), Error> {
                Ok(())
            }
            async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
                Ok(())
            }
            async fn html(&self) -> Result<String, Error> {
                Ok(FLOOD_HTML.to_string())
            }
            async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
                Ok(serde_json::Value::Null)
            }
            async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
                Ok(())
            }
        }

        let mut driver = FloodDriver;
        let cfg = Config::new("rust", "bing", BrowserKind::Fake)
            .with_max_results(5)
            .with_timeout(Duration::from_secs(5));
        let outcome = run_with(&mut driver, cfg).await.expect("应成功");
        // example.com 4 条被截断到 2 条；example.org 保留 → 共 3 条
        assert_eq!(outcome.results.len(), 3, "同域名去重后保留 2+1");
        assert_eq!(
            outcome
                .results
                .iter()
                .filter(|r| r.domain == "example.com")
                .count(),
            DOMAIN_LIMIT,
            "同域名最多保留 DOMAIN_LIMIT 条"
        );
        // rank 连续重排
        assert_eq!(outcome.results[0].rank, 1);
        assert_eq!(outcome.results[2].rank, 3);
    }

    /// 回归：`site:` 过滤时用户意图为同域 → 同域名去重禁用，结果全保留。
    #[tokio::test]
    async fn site_filter_disables_domain_dedup() {
        struct FloodDriver;
        #[async_trait::async_trait]
        impl crate::ports::BrowserDriver for FloodDriver {
            async fn navigate(&mut self, _url: url::Url) -> Result<(), Error> {
                Ok(())
            }
            async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
                Ok(())
            }
            async fn html(&self) -> Result<String, Error> {
                // 与 domain_flooding_is_capped 相同的同域刷屏页面
                Ok(r#"<html><body><ol id="b_results">
                  <li class="b_algo"><h2><a href="https://example.com/1">一</a></h2><div class="b_caption"><p>摘要</p></div></li>
                  <li class="b_algo"><h2><a href="https://example.com/2">二</a></h2><div class="b_caption"><p>摘要</p></div></li>
                  <li class="b_algo"><h2><a href="https://example.com/3">三</a></h2><div class="b_caption"><p>摘要</p></div></li>
                  <li class="b_algo"><h2><a href="https://example.com/4">四</a></h2><div class="b_caption"><p>摘要</p></div></li>
                </ol></body></html>"#
                    .to_string())
            }
            async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
                Ok(serde_json::Value::Null)
            }
            async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
                Ok(())
            }
        }

        let mut driver = FloodDriver;
        let cfg = Config::new("rust", "bing", BrowserKind::Fake)
            .with_max_results(5)
            .with_timeout(Duration::from_secs(5))
            .with_site(Some("example.com".into()));
        let outcome = run_with(&mut driver, cfg).await.expect("应成功");
        assert_eq!(outcome.results.len(), 4, "site: 过滤时同域名不截断");
    }

    // ==== P3 相关性门禁（roadmap-result-quality.md）====

    /// 构造一条 web 结果（测试用）。
    fn result(title: &str, snippet: &str, domain: &str) -> SearchResult {
        SearchResult {
            rank: 1,
            title: title.into(),
            url: format!("https://{domain}/"),
            snippet: snippet.into(),
            domain: domain.into(),
            https: true,
            published_at: None,
            is_ad: false,
            url_resolved: false,
            result_kind: crate::domain::ResultKind::Web,
        }
    }

    /// 真实故障样本（CrabMate 会话导出）：Bing 对「中国基金 数据 网站 天天基金网
    /// 蛋卷基金 净值查询」锚定「中国」，返回中国维基/百科——数量/类型正常但零词面重叠。
    #[test]
    fn relevant_detects_topic_mismatch_for_multi_term_queries() {
        let q = "中国基金 数据 网站 天天基金网 蛋卷基金 净值查询";
        let china_results = vec![
            result(
                "中國 - 维基百科，自由的百科全书",
                "中国最早成型于新石器时期与青铜时期之间的过渡期…",
                "zh.m.wikipedia.org",
            ),
            result(
                "中华人民共和国 - 维基百科，自由的百科全书",
                "1949年9月21日，中国人民政治协商会议第一次全体会议…",
                "zh.m.wikipedia.org",
            ),
            result(
                "中华人民共和国_百度百科",
                "中华人民共和国简称中国，位于亚洲东部…",
                "baike.baidu.com",
            ),
            result("中国政府网_中央人民政府门户网站", "政策解读…", "www.gov.cn"),
        ];
        assert!(
            !relevant(&china_results, q),
            "零词面重叠应判离题（「网站」为短泛词不计显著词）"
        );

        // 同一查询命中基金站点 → 相关（命中显著词「天天基金网」）
        let fund_results = vec![
            result(
                "天天基金网 净值查询",
                "天天基金网是东方财富旗下基金平台…",
                "fund.eastmoney.com",
            ),
            result(
                "蛋卷基金官网",
                "蛋卷基金净值查询与定投…",
                "danjuanfunds.com",
            ),
        ];
        assert!(relevant(&fund_results, q), "命中显著词应判相关");
    }

    /// 单词查询（导航性/歧义）与全短泛词查询不做相关性判定（防误伤）。
    #[test]
    fn relevant_skips_single_term_and_short_term_queries() {
        let any = vec![result("同域一", "摘要", "example.com")];
        assert!(relevant(&any, "rust"), "单词查询跳过判定");
        let any2 = vec![result("中国政府网", "政策解读…", "www.gov.cn")];
        assert!(
            relevant(&any2, "数据 网站"),
            "全为短泛词（<3 字符）→ 无可判显著词，跳过"
        );
    }

    /// 词面重叠大小写不敏感。
    #[test]
    fn relevant_is_case_insensitive() {
        let r = vec![result("Learn Rust async/await", "…", "doc.rust-lang.org")];
        assert!(relevant(&r, "rust async"), "Rust 应命中小写 rust");
    }

    /// 短泛词（「网站」）不构成命中：中国政府网含「网站」但不含任何显著词。
    #[test]
    fn relevant_short_generic_word_does_not_count_as_hit() {
        let q = "中国基金 数据 网站 天天基金网 蛋卷基金 净值查询";
        let r = vec![result(
            "中国政府网_中央人民政府门户网站",
            "政策解读…网站…",
            "www.gov.cn",
        )];
        assert!(!relevant(&r, q), "仅命中短泛词不应判相关");
    }

    /// P3 门禁细化（C1）：部分重叠但占比低于 1/5 → 仍判离题；恰好 ≥1/5 → 判相关。
    #[test]
    fn relevant_requires_hit_ratio_threshold() {
        let q = "中国基金 数据 网站 天天基金网 蛋卷基金 净值查询";
        // 10 条中仅 1 条命中显著词（10% < 20%）→ 离题
        let mut off_topic = vec![result("中国政府网", "政策解读…", "www.gov.cn",); 9];
        off_topic.push(result(
            "天天基金网 净值查询",
            "天天基金网是东方财富旗下基金平台…",
            "fund.eastmoney.com",
        ));
        assert_eq!(off_topic.len(), 10);
        assert!(
            !relevant(&off_topic, q),
            "1/10 命中（10%）低于 1/5 阈值应判离题"
        );
        // 10 条中 2 条命中（20% = 阈值）→ 相关
        let mut on_topic = vec![result("中国政府网", "政策解读…", "www.gov.cn",); 8];
        on_topic.push(result(
            "天天基金网 净值查询",
            "天天基金网是东方财富旗下基金平台…",
            "fund.eastmoney.com",
        ));
        on_topic.push(result(
            "蛋卷基金官网",
            "蛋卷基金净值查询与定投…",
            "danjuanfunds.com",
        ));
        assert_eq!(on_topic.len(), 10);
        assert!(relevant(&on_topic, q), "2/10 命中（20%）达阈值应判相关");
    }

    /// 空结果集：不额外拦截（由内容型数量门禁兜底），避免与低产判定叠加误伤。
    #[test]
    fn relevant_empty_results_is_not_off_topic() {
        assert!(relevant(&[], "中国基金 数据 网站"), "空集不判离题");
    }

    /// P3 核心：Bing 返回离题但类型正常的 Web 结果（数量达标、占比达标）→ 相关性
    /// 门禁触发 → 降级 DuckDuckGo，采用其相关内容结果。
    #[tokio::test]
    async fn irrelevant_web_results_trigger_engine_fallback() {
        /// Bing 风格：5 条中国维基/百科/政府站（全部 web，占比 100%，但零词面重叠）。
        const IRRELEVANT_BING_HTML: &str = r#"<html><body><ol id="b_results">
          <li class="b_algo"><h2><a href="https://zh.m.wikipedia.org/wiki/%E4%B8%AD%E5%9C%8B">中國 - 维基百科</a></h2><div class="b_caption"><p>中国最早成型于新石器时期…</p></div></li>
          <li class="b_algo"><h2><a href="https://zh.m.wikipedia.org/wiki/%E4%B8%AD%E5%8D%8E%E4%BA%BA%E6%B0%91%E5%85%B1%E5%92%8C%E5%9B%BD">中华人民共和国 - 维基百科</a></h2><div class="b_caption"><p>1949年…</p></div></li>
          <li class="b_algo"><h2><a href="https://baike.baidu.com/item/%E4%B8%AD%E5%8D%8E%E4%BA%BA%E6%B0%91%E5%85%B1%E5%92%8C%E5%9B%BD">中华人民共和国_百度百科</a></h2><div class="b_caption"><p>简称中国…</p></div></li>
          <li class="b_algo"><h2><a href="https://www.gov.cn/">中国政府网</a></h2><div class="b_caption"><p>政策解读…</p></div></li>
          <li class="b_algo"><h2><a href="https://www.bbc.com/zhongwen/topics/ckr7mn6r003t/simp">中国 - BBC News 中文</a></h2><div class="b_caption"><p>BBC中文网关于中国的最新新闻…</p></div></li>
        </ol></body></html>"#;
        /// DDG 风格：3 条基金站点（命中查询显著词）。
        const RELEVANT_DDG_HTML: &str = r#"<html><body>
          <div class="result"><a class="result__a" href="https://fund.eastmoney.com/">天天基金网 (1234567.com.cn) 基金数据</a><a class="result__snippet">东方财富旗下基金平台，提供净值查询。</a></div>
          <div class="result"><a class="result__a" href="https://danjuanfunds.com/">蛋卷基金官网</a><a class="result__snippet">蛋卷基金净值查询与定投。</a></div>
          <div class="result"><a class="result__a" href="https://www.howbuy.com/">好买基金网</a><a class="result__snippet">基金数据与净值查询。</a></div>
        </body></html>"#;

        /// 按导航 URL 返回不同页面：bing → 离题集群；ddg → 相关结果。
        struct TopicDriver {
            current: Option<String>,
        }
        #[async_trait::async_trait]
        impl crate::ports::BrowserDriver for TopicDriver {
            async fn navigate(&mut self, url: url::Url) -> Result<(), Error> {
                self.current = Some(url.to_string());
                Ok(())
            }
            async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
                Ok(())
            }
            async fn html(&self) -> Result<String, Error> {
                let url = self.current.as_deref().unwrap_or_default();
                if url.contains("bing.com") {
                    Ok(IRRELEVANT_BING_HTML.to_string())
                } else {
                    Ok(RELEVANT_DDG_HTML.to_string())
                }
            }
            async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
                Ok(serde_json::Value::Null)
            }
            async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
                Ok(())
            }
        }

        let mut driver = TopicDriver { current: None };
        let cfg = Config::new(
            "中国基金 数据 网站 天天基金网 蛋卷基金 净值查询",
            "bing,duckduckgo",
            BrowserKind::Fake,
        )
        .with_max_results(5)
        .with_timeout(Duration::from_secs(10));
        let outcome = run_with(&mut driver, cfg).await.expect("降级后应成功");
        // 离题集群不满足 → 降级 ddg 并采用其相关内容结果
        assert_eq!(outcome.meta.engine, "duckduckgo");
        assert_eq!(outcome.meta.engine_tried, vec!["bing", "duckduckgo"]);
        assert!(!outcome.meta.low_yield, "ddg 3 条内容型 ≥ 阈值");
    }

    /// 单引擎（无降级链）时离题结果不崩溃：候选兜底成功并标记 low_yield。
    #[tokio::test]
    async fn irrelevant_results_single_engine_marks_low_yield() {
        /// Bing 风格：2 条中国维基（web，零词面重叠）。
        const IRRELEVANT_BING_HTML: &str = r#"<html><body><ol id="b_results">
          <li class="b_algo"><h2><a href="https://zh.m.wikipedia.org/wiki/%E4%B8%AD%E5%9C%8B">中國 - 维基百科</a></h2><div class="b_caption"><p>中国最早成型于新石器时期…</p></div></li>
          <li class="b_algo"><h2><a href="https://zh.m.wikipedia.org/wiki/%E4%B8%AD%E5%8D%8E%E4%BA%BA%E6%B0%91%E5%85%B1%E5%92%8C%E5%9B%BD">中华人民共和国 - 维基百科</a></h2><div class="b_caption"><p>1949年…</p></div></li>
        </ol></body></html>"#;

        struct FixedDriver;
        #[async_trait::async_trait]
        impl crate::ports::BrowserDriver for FixedDriver {
            async fn navigate(&mut self, _url: url::Url) -> Result<(), Error> {
                Ok(())
            }
            async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
                Ok(())
            }
            async fn html(&self) -> Result<String, Error> {
                Ok(IRRELEVANT_BING_HTML.to_string())
            }
            async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
                Ok(serde_json::Value::Null)
            }
            async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
                Ok(())
            }
        }

        let mut driver = FixedDriver;
        let cfg = Config::new(
            "中国基金 数据 网站 天天基金网 蛋卷基金 净值查询",
            "bing",
            BrowserKind::Fake,
        )
        .with_max_results(5)
        .with_timeout(Duration::from_secs(10));
        let outcome = run_with(&mut driver, cfg)
            .await
            .expect("单引擎离题应兜底成功");
        assert_eq!(outcome.meta.engine, "bing");
        assert_eq!(outcome.meta.engine_tried, vec!["bing"]);
        assert!(outcome.meta.low_yield, "离题结果应标记 low_yield");
    }

    /// 回归：多词查询 + 相关结果 → 不触发相关性降级（首引擎直接采用）。
    #[tokio::test]
    async fn relevant_multi_term_results_do_not_trigger_fallback() {
        let mut driver = crate::drivers::resolve(BrowserKind::Fake)
            .await
            .expect("resolve Fake 应成功");
        let cfg = Config::new("rust 异步", "bing", BrowserKind::Fake)
            .with_max_results(5)
            .with_timeout(Duration::from_secs(5));
        let outcome = run_with(&mut *driver, cfg).await.expect("应成功");
        assert_eq!(outcome.meta.engine, "bing", "相关结果不应降级");
        assert_eq!(outcome.meta.engine_tried, vec!["bing"]);
        assert!(!outcome.meta.low_yield);
    }

    // ==== fetch（ADR-009）====

    /// 抓取驱动：返回文章页 fixture，eval 模拟 readyState/location.href。
    struct FetchDriver {
        html: String,
        current: Option<String>,
    }

    #[async_trait::async_trait]
    impl crate::ports::BrowserDriver for FetchDriver {
        async fn navigate(&mut self, url: url::Url) -> Result<(), Error> {
            self.current = Some(url.to_string());
            Ok(())
        }
        async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
            Ok(())
        }
        async fn html(&self) -> Result<String, Error> {
            Ok(self.html.clone())
        }
        async fn eval(&mut self, js: &str) -> Result<serde_json::Value, Error> {
            if js.contains("readyState") {
                return Ok(serde_json::Value::String("complete".into()));
            }
            if js.contains("location.href") {
                return Ok(serde_json::Value::String(
                    self.current.clone().unwrap_or_default(),
                ));
            }
            Ok(serde_json::Value::Null)
        }
        async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
            Ok(())
        }
    }

    /// URL 归一化：缺 scheme 自动补 https；去 fragment；非 http/https → 参数错误。
    #[test]
    fn normalize_fetch_url_auto_prefixes_and_validates() {
        assert_eq!(
            normalize_fetch_url("example.com/a").unwrap().to_string(),
            "https://example.com/a"
        );
        assert_eq!(
            normalize_fetch_url("http://example.com")
                .unwrap()
                .to_string(),
            "http://example.com/",
            "url crate 对无路径 URL 归一化补尾斜杠"
        );
        assert_eq!(
            normalize_fetch_url("https://example.com/a#sec")
                .unwrap()
                .to_string(),
            "https://example.com/a",
            "去 fragment"
        );
        assert_eq!(
            normalize_fetch_url("//example.com/x").unwrap().to_string(),
            "https://example.com/x",
            "协议相对 URL 显式补 https:"
        );
        assert!(
            matches!(normalize_fetch_url(""), Err(Error::Cli(_))),
            "空 URL → 参数错误"
        );
        assert!(
            matches!(
                normalize_fetch_url("file:///etc/passwd"),
                Err(Error::Cli(_))
            ),
            "file scheme → 参数错误"
        );
        assert!(
            matches!(
                normalize_fetch_url("javascript:alert(1)"),
                Err(Error::Cli(_))
            ),
            "javascript scheme → 参数错误"
        );
    }

    /// run_fetch_with：正文 + 结构化字段 + final_url（fake 驱动返回文章页 fixture）。
    #[tokio::test]
    async fn fetch_with_returns_text_fields_and_final_url() {
        use crate::domain::ExtractField as F;
        let mut driver = FetchDriver {
            html: include_str!("../tests/fixtures/article.html").to_string(),
            current: None,
        };
        let cfg = FetchConfig::new("https://example.com/a", BrowserKind::Fake)
            .with_max_chars(20_000)
            .with_extract(F::ALL.to_vec())
            .with_timeout(Duration::from_secs(10));
        let page = run_fetch_with(&mut driver, cfg).await.expect("抓取应成功");
        assert_eq!(page.url, "https://example.com/a");
        assert_eq!(
            page.final_url.as_deref(),
            Some("https://example.com/a"),
            "fake 无重定向 → final_url = 请求 URL"
        );
        assert!(page.text.contains("这是第一段正文内容。"));
        assert!(!page.text.contains("导航链接"));
        assert_eq!(page.extracted["price"], "1299.00");
        assert_eq!(page.extracted["rating"], 4.6);
        assert_eq!(page.chars, page.text.chars().count());
        assert!(!page.truncated);
    }

    /// text=false：正文为空（chars=0），字段照常提取。
    #[tokio::test]
    async fn fetch_with_text_false_skips_body() {
        use crate::domain::ExtractField as F;
        let mut driver = FetchDriver {
            html: include_str!("../tests/fixtures/article.html").to_string(),
            current: None,
        };
        let cfg = FetchConfig::new("https://example.com/a", BrowserKind::Fake)
            .with_text(false)
            .with_extract(vec![F::Price])
            .with_timeout(Duration::from_secs(10));
        let page = run_fetch_with(&mut driver, cfg).await.expect("抓取应成功");
        assert!(page.text.is_empty(), "text=false 时正文为空");
        assert_eq!(page.chars, 0);
        assert_eq!(page.extracted["price"], "1299.00", "字段仍提取");
    }

    /// 非法 URL：参数错误直接返回（不触网）。
    #[tokio::test]
    async fn fetch_with_rejects_invalid_url() {
        let mut driver = FetchDriver {
            html: String::new(),
            current: None,
        };
        let cfg = FetchConfig::new("file:///etc/passwd", BrowserKind::Fake)
            .with_timeout(Duration::from_secs(10));
        let err = run_fetch_with(&mut driver, cfg)
            .await
            .expect_err("非法 URL 应报错");
        assert!(matches!(err, Error::Cli(_)));
    }

    /// `run_fetch`（resolve FakeDriver = SMOKE_HTML）：成功包 + 截断 + title 提取。
    #[tokio::test]
    async fn fetch_via_fake_driver_truncates_and_returns_url() {
        use crate::domain::ExtractField as F;
        let cfg = FetchConfig::new("https://example.com", BrowserKind::Fake)
            .with_max_chars(5)
            .with_extract(vec![F::Title])
            .with_timeout(Duration::from_secs(10));
        let page = run_fetch(cfg).await.expect("fetch 应成功");
        assert_eq!(page.url, "https://example.com/");
        assert_eq!(page.text.chars().count(), 5, "截断到 max_chars");
        assert!(page.truncated);
        assert_eq!(
            page.final_url.as_deref(),
            Some("https://example.com/"),
            "fake eval location.href = 导航 URL"
        );
        assert_eq!(
            page.extracted["title"], "rust async - 搜索",
            "SMOKE_HTML 的 <title> 可提取"
        );
    }

    /// `run_fetch` text=false：正文为空（省 token），URL 缺 scheme 自动补 https。
    #[tokio::test]
    async fn fetch_via_fake_without_text() {
        let cfg = FetchConfig::new("example.com", BrowserKind::Fake)
            .with_text(false)
            .with_timeout(Duration::from_secs(10));
        let page = run_fetch(cfg).await.expect("fetch 应成功");
        assert!(page.text.is_empty(), "text=false 时正文为空");
        assert_eq!(page.chars, 0);
        assert_eq!(page.url, "https://example.com/", "缺 scheme 自动补 https");
    }

    /// `run_fetch` 非法 URL：校验前置，不 resolve 浏览器（无浏览器环境也可测）。
    #[tokio::test]
    async fn fetch_via_fake_rejects_bad_url_before_resolve() {
        let cfg = FetchConfig::new("file:///etc/passwd", BrowserKind::Fake)
            .with_timeout(Duration::from_secs(10));
        let err = run_fetch(cfg).await.expect_err("非法 URL 应报错");
        assert!(matches!(err, Error::Cli(_)));
    }

    /// wait_load：eval 持续返回非 complete（SPA 慢加载）→ 预算耗尽后返回（尽力语义）。
    #[tokio::test]
    async fn wait_load_gives_up_after_budget() {
        struct SlowDriver;
        #[async_trait::async_trait]
        impl crate::ports::BrowserDriver for SlowDriver {
            async fn navigate(&mut self, _url: url::Url) -> Result<(), Error> {
                Ok(())
            }
            async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
                Ok(())
            }
            async fn html(&self) -> Result<String, Error> {
                Ok(String::new())
            }
            async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
                Ok(serde_json::Value::String("loading".into()))
            }
            async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
                Ok(())
            }
        }
        let mut driver = SlowDriver;
        let start = Instant::now();
        wait_load(&mut driver, Duration::from_millis(150)).await;
        assert!(
            start.elapsed() >= Duration::from_millis(150),
            "预算耗尽才返回（尽力语义，不报错）"
        );
    }

    /// wait_load：eval 失败（受限页/驱动异常）→ 立即返回，不阻塞抓取。
    #[tokio::test]
    async fn wait_load_returns_on_eval_error() {
        struct BrokenDriver;
        #[async_trait::async_trait]
        impl crate::ports::BrowserDriver for BrokenDriver {
            async fn navigate(&mut self, _url: url::Url) -> Result<(), Error> {
                Ok(())
            }
            async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
                Ok(())
            }
            async fn html(&self) -> Result<String, Error> {
                Ok(String::new())
            }
            async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
                Err(Error::Network("eval 失败".into()))
            }
            async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
                Ok(())
            }
        }
        let mut driver = BrokenDriver;
        let start = Instant::now();
        wait_load(&mut driver, Duration::from_secs(5)).await;
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "eval 失败应立即返回（视为已加载）"
        );
    }

    /// fetch：`wait_selector` 触发 SPA 等待（wait_for 被调用）+ `http_status` 从
    /// PerformanceNavigationTiming eval 读出（fetch 补强，ADR-010）。
    #[tokio::test]
    async fn fetch_reports_http_status_and_waits_for_selector() {
        struct StatusDriver {
            waited: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl crate::ports::BrowserDriver for StatusDriver {
            async fn navigate(&mut self, _url: url::Url) -> Result<(), Error> {
                Ok(())
            }
            async fn wait_for(&mut self, selector: &str, _t: Duration) -> Result<(), Error> {
                assert_eq!(selector, "#content", "wait_selector 应透传给 wait_for");
                self.waited
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
            async fn html(&self) -> Result<String, Error> {
                Ok("<html><body><div id=\"content\">正文</div></body></html>".into())
            }
            async fn eval(&mut self, js: &str) -> Result<serde_json::Value, Error> {
                if js.contains("responseStatus") {
                    return Ok(serde_json::json!(200));
                }
                if js.contains("location.href") {
                    return Ok(serde_json::Value::String("https://example.com/a".into()));
                }
                if js.contains("readyState") {
                    return Ok(serde_json::Value::String("complete".into()));
                }
                Ok(serde_json::Value::Null)
            }
            async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
                Ok(())
            }
        }
        let waited = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut driver = StatusDriver {
            waited: waited.clone(),
        };
        let cfg = FetchConfig::new("https://example.com/a", BrowserKind::Fake)
            .with_wait_selector(Some("#content".into()))
            .with_timeout(Duration::from_secs(5));
        let page = run_fetch_with(&mut driver, cfg).await.expect("抓取应成功");
        assert_eq!(page.http_status, Some(200), "http_status 应从 eval 读出");
        assert_eq!(
            waited.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "wait_selector 应触发 wait_for"
        );
    }

    /// fetch：`wait_selector` 超时（选择器未出现）→ 尽力语义，仍返回成功包。
    #[tokio::test]
    async fn fetch_wait_selector_timeout_is_best_effort() {
        struct TimeoutWaitDriver;
        #[async_trait::async_trait]
        impl crate::ports::BrowserDriver for TimeoutWaitDriver {
            async fn navigate(&mut self, _url: url::Url) -> Result<(), Error> {
                Ok(())
            }
            async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
                Err(Error::Timeout("selector not found".into()))
            }
            async fn html(&self) -> Result<String, Error> {
                Ok(String::new())
            }
            async fn eval(&mut self, js: &str) -> Result<serde_json::Value, Error> {
                if js.contains("readyState") {
                    return Ok(serde_json::Value::String("complete".into()));
                }
                Ok(serde_json::Value::Null)
            }
            async fn screenshot(&mut self, _p: &std::path::Path) -> Result<(), Error> {
                Ok(())
            }
        }
        let mut driver = TimeoutWaitDriver;
        let cfg = FetchConfig::new("https://example.com/a", BrowserKind::Fake)
            .with_wait_selector(Some("#never".into()))
            .with_timeout(Duration::from_secs(5));
        let page = run_fetch_with(&mut driver, cfg)
            .await
            .expect("超时应尽力返回成功包");
        assert_eq!(page.http_status, None);
    }
}
