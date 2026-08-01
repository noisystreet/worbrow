//! 用例编排（design.md §6.2）。
//!
//! `run(config)`：解析 query → 选引擎 → 驱动浏览器 → 抽取 → 组装 Outcome。
//! 硬超时包裹全流程，超时返回 `Error::Timeout`（exit 124）。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::time::timeout;

use crate::SearchResult;
use crate::domain::{BrowserKind, Freshness, SafesearchLevel, SearchMeta, SearchQuery};
use crate::engines;
use crate::error::Error;
use crate::ports::{BrowserDriver, SearchProvider};

/// 低结果阈值：结果数低于该值时 `meta.low_yield = true`（design.md §10.4）。
pub const LOW_YIELD_THRESHOLD: usize = 3;
/// 结果元素等待预算上限：页面加载已消耗大部分 timeout 时，剩余时间不足以等待选择器
/// （design.md §6.2 二级超时）。
pub const WAIT_BUDGET: Duration = Duration::from_secs(10);

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

#[derive(Debug)]
pub struct Outcome {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub meta: SearchMeta,
}

/// 环境自检结果（design.md §10）：引擎注册表 + 各浏览器后端状态。
#[derive(Debug)]
pub struct DoctorReport {
    /// 可用引擎（注册表顺序）。
    pub engines: Vec<&'static str>,
    /// 浏览器后端状态（fake/chrome/firefox）。
    pub backends: Vec<BackendStatus>,
}

/// 单个浏览器后端状态。
#[derive(Debug)]
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
        .map_err(|e| Error::Internal(format!("tokio runtime 初始化失败: {e}")))?;
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
pub async fn run(config: Config) -> Result<Outcome, Error> {
    // 1. 解析并校验 query
    let text = config.query.trim();
    if text.is_empty() {
        return Err(Error::Cli("搜索词为空".into()));
    }
    if text.chars().count() > 512 {
        return Err(Error::Cli("搜索词过长（>512 字符）".into()));
    }
    // 空/全空引擎串（如 `--engine ""`）→ 参数错误（exit 2），而非内部错误
    if config.engines.is_empty() {
        return Err(Error::Cli("未指定引擎".into()));
    }

    let started_at = Utc::now();
    let timer = Instant::now();

    // 2. 选浏览器后端（测试可注入）
    let mut driver = match config.driver {
        Some(d) => d,
        None => crate::drivers::resolve(config.browser).await?,
    };

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

    // 4-8. 包整体硬超时：注入引擎单引擎（无降级）；否则内置注册表降级循环
    // （验证码阻止/解析失败/低产 → 尝试下一引擎，见 roadmap「引擎可配且可降级」）
    let (engine, html, results, captcha, fetched_pages, low_yield, engine_tried, mut driver) =
        match config.provider {
            Some(provider) => {
                let name = provider.name();
                // driver 移入闭包：超时/失败时闭包 drop → driver drop → 杀浏览器进程
                //（design.md §8：超时/取消/失败均回收子进程，防残留）
                let outcome = timeout(config.timeout, async {
                    let (html, results, captcha, pages) =
                        search_one(&*provider, &query, &mut *driver, config.timeout).await?;
                    let low_yield = results.len() < LOW_YIELD_THRESHOLD;
                    Ok::<_, Error>((
                        name,
                        html,
                        results,
                        captcha,
                        pages,
                        low_yield,
                        vec![name.to_string()],
                        driver,
                    ))
                })
                .await;
                match outcome {
                    Ok(Ok(v)) => v,
                    Ok(Err(e)) => return Err(e),
                    Err(_) => return Err(Error::Timeout("任务超时".into())),
                }
            }
            None => {
                let outcome = timeout(config.timeout, async {
                    let mut tried: Vec<String> = Vec::new();
                    // 低产候选兜底（取最高产）；引擎名恒为 `&'static str`（trait 签名）
                    let mut candidate: Option<(&'static str, Vec<SearchResult>, bool, usize, String)> =
                        None;
                    let mut last_error: Option<Error> = None;

                    for name in &config.engines {
                        let provider = engines::resolve(name)?;
                        tried.push(provider.name().to_string());
                        tracing::info!(engine = provider.name(), "尝试引擎");

                        match search_one(&*provider, &query, &mut *driver, config.timeout).await {
                            Ok((html, results, captcha, pages)) => {
                                // 满意：集满请求量或非低产（≥ 阈值）；否则保留候选继续降级
                                let satisfied = results.len() >= query.max_results
                                    || results.len() >= LOW_YIELD_THRESHOLD;
                                if satisfied {
                                    tracing::info!(
                                        engine = provider.name(),
                                        count = results.len(),
                                        "采用引擎"
                                    );
                                    let low_yield = results.len() < LOW_YIELD_THRESHOLD;
                                    return Ok::<_, Error>((
                                        provider.name(),
                                        html,
                                        results,
                                        captcha,
                                        pages,
                                        low_yield,
                                        tried,
                                        driver,
                                    ));
                                }
                                // 低产：保留最高产候选，继续尝试下一引擎
                                let better = match &candidate {
                                    Some((_, cur, ..)) => results.len() > cur.len(),
                                    None => true,
                                };
                                if better {
                                    candidate =
                                        Some((provider.name(), results, captcha, pages, html));
                                }
                                tracing::warn!(engine = provider.name(), "低产，保留候选继续尝试");
                            }
                            Err(Error::Captcha(e)) => {
                                last_error = Some(Error::Captcha(e));
                                tracing::warn!(engine = provider.name(), "验证码阻止，降级");
                            }
                            Err(Error::Engine(e)) => {
                                tracing::warn!(engine = provider.name(), code = %e.code, "解析失败，降级");
                                last_error = Some(Error::Engine(e));
                            }
                            // 网络/超时等错误不降级，直接返回（避免放大总耗时）
                            Err(e) => return Err(e),
                        }
                    }

                    // 全部尝试完：有低产候选则兜底成功；否则返回最后错误（captcha 优先）
                    if let Some((engine, results, captcha, pages, html)) = candidate {
                        return Ok((engine, html, results, captcha, pages, true, tried, driver));
                    }
                    match last_error {
                        Some(err) => Err(err),
                        None => Err(Error::Internal("引擎列表为空".into())),
                    }
                })
                .await;
                match outcome {
                    Ok(Ok(v)) => v,
                    Ok(Err(e)) => return Err(e),
                    Err(_) => return Err(Error::Timeout("任务超时".into())),
                }
            }
        };

    // 9. 可选调试产物（失败仅告警，不影响主流程）
    if let Some(path) = config.screenshot.as_deref()
        && let Err(e) = driver.screenshot(path).await
    {
        tracing::warn!("截图保存失败 {path:?}: {e}");
    }
    if let Some(path) = config.dump_html.as_deref()
        && let Err(e) = std::fs::write(path, &html)
    {
        tracing::warn!("HTML 保存失败 {path:?}: {e}");
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
    };

    Ok(Outcome {
        query: text.to_string(),
        results,
        meta,
    })
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
        // 抽取并去重合并（按 URL）
        for r in results {
            if seen.insert(r.url.clone()) {
                all.push(r);
            }
        }
        // 已集满 max_results 可提前停止翻页
        if all.len() >= query.max_results {
            break;
        }
    }

    if captcha && all.is_empty() {
        return Err(Error::Captcha("检测到验证码且未取得任何结果".into()));
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
        "navigate 完成"
    );

    // 等待结果容器出现：二级超时（页面加载预算内截断，design.md §6.2）
    let step = Instant::now();
    driver
        .wait_for(provider.result_selector(), wait_budget)
        .await?;
    tracing::info!(
        elapsed_ms = step.elapsed().as_millis() as u64,
        page,
        "wait_for 完成"
    );

    let step = Instant::now();
    let html = driver.html().await?;
    tracing::info!(
        elapsed_ms = step.elapsed().as_millis() as u64,
        page,
        "html 完成"
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
}
