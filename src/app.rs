//! 用例编排（design.md §6.2）。
//!
//! `run(config)`：解析 query → 选引擎 → 驱动浏览器 → 抽取 → 组装 Outcome。
//! 硬超时包裹全流程，超时返回 `Error::Timeout`（exit 124）。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::time::timeout;

use crate::SearchResult;
use crate::domain::{BrowserKind, SearchMeta, SearchQuery};
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
    engine: String,
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
    /// 测试注入用；生产为 `None`，走 `drivers::resolve`。
    driver: Option<Box<dyn BrowserDriver>>,
    /// 外部引擎扩展点：注入自定义 `SearchProvider` 时优先于 `engine` 注册表；生产为 `None`。
    provider: Option<Box<dyn SearchProvider>>,
}

impl Config {
    /// 典型搜索配置：默认 `max_results`/`timeout` 取 `domain::DEFAULT_*`，无调试产物。
    pub fn new(query: impl Into<String>, engine: impl Into<String>, browser: BrowserKind) -> Self {
        Self {
            query: query.into(),
            engine: engine.into(),
            browser,
            max_results: crate::domain::DEFAULT_MAX_RESULTS,
            timeout: Duration::from_secs(crate::domain::DEFAULT_TIMEOUT_SECS),
            screenshot: None,
            dump_html: None,
            lang: None,
            region: None,
            pages: 1,
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

/// 同步入口：内部创建 tokio runtime 并阻塞执行一次搜索。
///
/// CLI/脚本等非 async 调用方无需自行管理 runtime；异步场景（如 MCP）直接使用 [`run`]。
pub fn search(config: Config) -> Result<Outcome, Error> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| Error::Internal(format!("tokio runtime 初始化失败: {e}")))?;
    runtime.block_on(run(config))
}

/// 执行一次搜索（design.md §6.2 步骤 1-10）。
pub async fn run(config: Config) -> Result<Outcome, Error> {
    // 1. 解析并校验 query
    let text = config.query.trim();
    if text.is_empty() {
        return Err(Error::Cli("搜索词为空".into()));
    }
    if text.chars().count() > 512 {
        return Err(Error::Cli("搜索词过长（>512 字符）".into()));
    }

    let started_at = Utc::now();
    let timer = Instant::now();

    // 2. 选引擎：外部注入优先（自定义引擎扩展点），否则走内置注册表
    let provider = match config.provider {
        Some(p) => p,
        None => engines::resolve(&config.engine)?,
    };
    // 3. 选浏览器后端（测试可注入）
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
    };

    // 4-8. 包整体硬超时（翻页聚合：逐页 navigate → 解析 → 按 URL 去重合并）
    let (html, results, captcha, fetched_pages) = timeout(config.timeout, async {
        let wait_budget = config.timeout.min(WAIT_BUDGET);
        let mut seen = std::collections::HashSet::new();
        let mut all = Vec::new();
        let mut captcha = false;
        let mut fetched_pages = 0usize;
        let mut last_html = String::new();

        for page in 1..=query.pages {
            fetched_pages += 1;
            let step = Instant::now();
            let url = if page == 1 {
                provider.result_url(&query)
            } else {
                provider.page_url(&query, page)
            };
            driver.navigate(url).await?;
            tracing::info!(
                elapsed_ms = step.elapsed().as_millis() as u64,
                page,
                "navigate 完成"
            );

            // 6. 等待结果容器出现：二级超时（页面加载预算内截断，design.md §6.2）
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
            last_html = driver.html().await?;
            tracing::info!(
                elapsed_ms = step.elapsed().as_millis() as u64,
                page,
                "html 完成"
            );

            // 7. 验证码启发式检测（不中止）
            let lower = last_html.to_lowercase();
            captcha |= provider
                .captcha_heuristics()
                .iter()
                .any(|h| lower.contains(h));

            // 8. 抽取并去重合并（按 URL）
            for r in provider.parse(&last_html)? {
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

        Ok::<_, Error>((last_html, all, captcha, fetched_pages))
    })
    .await??;

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
        engine: provider.name(),
        started_at,
        elapsed_ms,
        result_count,
        pages: fetched_pages,
        low_yield: result_count < LOW_YIELD_THRESHOLD,
        captcha,
        engine_error: None,
    };

    Ok(Outcome {
        query: text.to_string(),
        results,
        meta,
    })
}

#[cfg(test)]
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
}
