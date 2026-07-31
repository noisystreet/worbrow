//! 用例编排（design.md §6.2）。
//!
//! `run(config)`：解析 query → 选引擎 → 驱动浏览器 → 抽取 → 组装 Outcome。
//! 硬超时包裹全流程，超时返回 `Error::Timeout`（exit 124）。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::time::timeout;

use crate::domain::{SearchMeta, SearchQuery};
use crate::drivers::BrowserKind;
use crate::engines;
use crate::error::Error;
use crate::ports::BrowserDriver;

/// 低结果阈值：结果数低于该值时 `meta.low_yield = true`（design.md §10.4）。
pub const LOW_YIELD_THRESHOLD: usize = 3;
/// 结果元素等待预算上限：页面加载已消耗大部分 timeout 时，剩余时间不足以等待选择器
/// （design.md §6.2 二级超时）。
pub const WAIT_BUDGET: Duration = Duration::from_secs(10);

pub struct Config {
    pub query: String,
    pub engine: String,
    pub browser: BrowserKind,
    pub max_results: usize,
    pub timeout: Duration,
    pub screenshot: Option<PathBuf>,
    pub dump_html: Option<PathBuf>,
    /// 测试注入用；生产为 `None`，走 `drivers::resolve`。
    pub driver: Option<Box<dyn BrowserDriver>>,
}

#[derive(Debug)]
pub struct Outcome {
    pub query: String,
    pub results: Vec<crate::domain::SearchResult>,
    pub meta: SearchMeta,
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

    // 2. 选引擎
    let provider = engines::resolve(&config.engine)?;
    // 3. 选浏览器后端（测试可注入）
    let mut driver = match config.driver {
        Some(d) => d,
        None => crate::drivers::resolve(config.browser).await?,
    };

    let query = SearchQuery {
        text: text.to_string(),
        max_results: config.max_results.max(1),
    };

    // 4-8. 包整体硬超时
    let (html, results, captcha) = timeout(config.timeout, async {
        let step = Instant::now();
        driver.navigate(provider.result_url(&query)).await?;
        tracing::info!(
            elapsed_ms = step.elapsed().as_millis() as u64,
            "navigate 完成"
        );

        // 6. 等待结果容器出现：二级超时（页面加载预算内截断，design.md §6.2）
        let wait_budget = config.timeout.min(WAIT_BUDGET);
        let step = Instant::now();
        driver
            .wait_for(provider.result_selector(), wait_budget)
            .await?;
        tracing::info!(
            elapsed_ms = step.elapsed().as_millis() as u64,
            "wait_for 完成"
        );

        let step = Instant::now();
        let html = driver.html().await?;
        tracing::info!(elapsed_ms = step.elapsed().as_millis() as u64, "html 完成");

        // 7. 验证码启发式检测（不中止）
        let lower = html.to_lowercase();
        let captcha = provider
            .captcha_heuristics()
            .iter()
            .any(|h| lower.contains(h));

        // 8. 抽取结果并截断
        let mut results = provider.parse(&html)?;
        results.truncate(query.max_results);

        if captcha && results.is_empty() {
            return Err(Error::Captcha("检测到验证码且未取得任何结果".into()));
        }

        Ok::<_, Error>((html, results, captcha))
    })
    .await??;

    // 9. 可选调试产物（失败仅告警，不影响主流程）
    if let Some(path) = config.screenshot.as_deref() {
        if let Err(e) = driver.screenshot(path).await {
            tracing::warn!("截图保存失败 {path:?}: {e}");
        }
    }
    if let Some(path) = config.dump_html.as_deref() {
        if let Err(e) = std::fs::write(path, &html) {
            tracing::warn!("HTML 保存失败 {path:?}: {e}");
        }
    }

    // 10. 组装 Outcome
    let elapsed_ms = timer.elapsed().as_millis() as u64;
    let result_count = results.len();
    let meta = SearchMeta {
        engine: provider.name(),
        started_at,
        elapsed_ms,
        result_count,
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
