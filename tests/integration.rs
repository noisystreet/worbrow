//! 端到端集成测试：FakeDriver + fixture → app::run → 校验 JSON/退出码（design.md §11）。
//! CI 无需真实浏览器。

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use url::Url;
use worbrow::BrowserKind;
use worbrow::app::{self, Config};
use worbrow::error::Error;
use worbrow::ports::BrowserDriver;

const FIXTURE: &str = include_str!("fixtures/duckduckgo.html");

/// 本地假驱动：返回注入的 fixture HTML（`drivers::fake` 已内部化，外部测试自实现；
/// 行为与内部 FakeDriver 一致）。
#[derive(Debug)]
struct FixtureDriver {
    html: String,
}

impl FixtureDriver {
    fn new(html: impl Into<String>) -> Self {
        Self { html: html.into() }
    }
}

#[async_trait]
impl BrowserDriver for FixtureDriver {
    async fn navigate(&mut self, _url: Url) -> Result<(), Error> {
        Ok(())
    }

    async fn wait_for(&mut self, _selector: &str, _timeout: Duration) -> Result<(), Error> {
        Ok(())
    }

    async fn html(&self) -> Result<String, Error> {
        Ok(self.html.clone())
    }

    async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
        Ok(serde_json::Value::Null)
    }

    async fn screenshot(&mut self, _path: &Path) -> Result<(), Error> {
        Ok(())
    }
}

fn config(query: &str) -> Config {
    config_with(query, "duckduckgo")
}

/// 指定引擎的配置（字段已私有化，仅经 builder 构造）。
fn config_with(query: &str, engine: &str) -> Config {
    Config::new(query, engine, BrowserKind::Fake)
        .with_max_results(5)
        .with_timeout(Duration::from_secs(5))
        .with_driver(Box::new(FixtureDriver::new(FIXTURE)))
}

#[tokio::test]
async fn end_to_end_with_fake_driver_yields_results() {
    let outcome = app::run(config("rust")).await.expect("应成功");
    assert_eq!(outcome.results.len(), 3);
    assert_eq!(outcome.meta.engine, "duckduckgo");
    assert_eq!(outcome.meta.result_count, 3);
    assert!(!outcome.meta.low_yield); // 3 条 ≥ 阈值
    assert!(!outcome.meta.captcha);
    assert!(outcome.meta.engine_error.is_none());
    // 排名从 1 开始
    assert_eq!(outcome.results[0].rank, 1);
}

#[tokio::test]
async fn max_results_truncates() {
    let cfg = config("rust").with_max_results(2);
    let outcome = app::run(cfg).await.expect("应成功");
    assert_eq!(outcome.results.len(), 2);
    assert!(outcome.meta.low_yield);
}

#[tokio::test]
async fn empty_query_is_cli_error() {
    let err = app::run(config("   ")).await.unwrap_err();
    assert!(matches!(err, Error::Cli(_)));
}

#[tokio::test]
async fn unknown_engine_is_cli_error() {
    let cfg = config_with("rust", "google");
    let err = app::run(cfg).await.unwrap_err();
    assert!(matches!(err, Error::Cli(_)));
}

#[tokio::test]
async fn empty_engine_is_cli_error() {
    // 空引擎串 → 参数错误（exit 2），而非内部错误
    let cfg = config_with("rust", "");
    let err = app::run(cfg).await.unwrap_err();
    assert!(matches!(err, Error::Cli(_)));
    assert_eq!(err.exit_code(), 2);
}

#[tokio::test]
async fn captcha_html_yields_captcha_flag() {
    // 验证码特征词存在但仍有结果：标记 captcha=true，不中止
    let html = format!("<html>anomaly<body>{FIXTURE}</body></html>");
    let cfg = config("rust").with_driver(Box::new(FixtureDriver::new(html)));
    let outcome = app::run(cfg).await.expect("应成功");
    assert!(outcome.meta.captcha);
    assert!(!outcome.results.is_empty());
}

#[tokio::test]
async fn timeout_returns_timeout_error() {
    let cfg = config("rust").with_timeout(Duration::from_millis(1));
    // FakeDriver 不阻塞，但超时语义应正确映射
    let outcome = app::run(cfg).await;
    // FakeDriver 瞬时完成，超时未必触发；断言无 panic 即可
    assert!(outcome.is_ok() || matches!(outcome, Err(Error::Timeout(_))));
}

/// 慢驱动：navigate 阻塞，用于**真实触发**超时路径（FakeDriver 瞬时完成覆盖不到）。
struct SlowDriver;

#[async_trait]
impl BrowserDriver for SlowDriver {
    async fn navigate(&mut self, _url: Url) -> Result<(), Error> {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    }

    async fn wait_for(&mut self, _selector: &str, _timeout: Duration) -> Result<(), Error> {
        Ok(())
    }

    async fn html(&self) -> Result<String, Error> {
        Ok(String::new())
    }

    async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
        Ok(serde_json::Value::Null)
    }

    async fn screenshot(&mut self, _path: &Path) -> Result<(), Error> {
        Ok(())
    }
}

#[tokio::test]
async fn slow_driver_triggers_timeout_error() {
    let cfg = config("rust")
        .with_timeout(Duration::from_millis(100))
        .with_driver(Box::new(SlowDriver));
    let err = app::run(cfg).await.unwrap_err();
    assert!(matches!(err, Error::Timeout(_)));
    // 退出码契约：超时 = 124（design.md §7.2）
    assert_eq!(err.exit_code(), 124);
}

/// 翻页驱动：按 navigate 顺序依次返回各页 HTML（验证翻页聚合，无需真实浏览器）。
#[derive(Debug)]
struct PageDriver {
    pages: Vec<String>,
    next: usize,
    current: usize,
}

impl PageDriver {
    fn new(pages: Vec<String>) -> Self {
        Self {
            pages,
            next: 0,
            current: 0,
        }
    }
}

#[async_trait]
impl BrowserDriver for PageDriver {
    async fn navigate(&mut self, _url: Url) -> Result<(), Error> {
        self.current = self.next.min(self.pages.len().saturating_sub(1));
        self.next += 1;
        Ok(())
    }

    async fn wait_for(&mut self, _selector: &str, _timeout: Duration) -> Result<(), Error> {
        Ok(())
    }

    async fn html(&self) -> Result<String, Error> {
        Ok(self.pages[self.current].clone())
    }

    async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
        Ok(serde_json::Value::Null)
    }

    async fn screenshot(&mut self, _path: &Path) -> Result<(), Error> {
        Ok(())
    }
}

/// 第 2 页 HTML：DDG 结构，2 条新结果 + 1 条与首页重复（验证去重）。
fn page2_html() -> String {
    r#"<html><body>
        <div class="result"><a class="result__a" href="https://example.com/p2a">P2A</a><a class="result__snippet">p2a snippet</a></div>
        <div class="result"><a class="result__a" href="https://example.com/rust">重复</a><a class="result__snippet">dup</a></div>
        <div class="result"><a class="result__a" href="https://example.com/p2b">P2B</a><a class="result__snippet">p2b snippet</a></div>
        </body></html>"#
    .to_string()
}

#[tokio::test]
async fn pages_aggregate_deduplicate_and_rerank() {
    // 首页 fixture 3 条 + 第 2 页 3 条（1 条与首页 URL 重复）→ 合并后 5 条
    let cfg = config("rust")
        .with_pages(2)
        .with_max_results(10)
        .with_driver(Box::new(PageDriver::new(vec![
            FIXTURE.to_string(),
            page2_html(),
        ])));
    let outcome = app::run(cfg).await.expect("翻页聚合应成功");
    assert_eq!(outcome.results.len(), 5, "去重后应合并 5 条");
    // rank 重排 1..=5
    for (i, r) in outcome.results.iter().enumerate() {
        assert_eq!(r.rank, i + 1, "rank 应重排");
    }
    // meta 记录实际聚合页数
    assert_eq!(outcome.meta.pages, 2);
}

#[tokio::test]
async fn pages_stop_early_when_max_results_reached() {
    // max_results=2：第 1 页即满 → 提前停止翻页，meta.pages=1
    let cfg = config("rust")
        .with_pages(3)
        .with_max_results(2)
        .with_driver(Box::new(PageDriver::new(vec![
            FIXTURE.to_string(),
            page2_html(),
        ])));
    let outcome = app::run(cfg).await.expect("提前停止应成功");
    assert_eq!(outcome.results.len(), 2);
    assert_eq!(outcome.meta.pages, 1, "集满 max_results 后应停止翻页");
}

// ---- 引擎降级链（roadmap「引擎可配且可降级」）----

fn low_yield_bing_html() -> String {
    r#"<html><body><ol id="b_results">
        <li class="b_algo"><h2><a href="https://example.com/a">A</a></h2><div class="b_caption"><p>a</p></div></li>
        <li class="b_algo"><h2><a href="https://example.com/b">B</a></h2><div class="b_caption"><p>b</p></div></li>
        </ol></body></html>"#
        .to_string()
}

fn ddg_one_result_html() -> String {
    r#"<html><body>
        <div class="result"><a class="result__a" href="https://example.com/only">Only</a><a class="result__snippet">only snippet</a></div>
        </body></html>"#
        .to_string()
}

#[tokio::test]
async fn engine_falls_back_on_parse_failure() {
    // 首引擎 bing 解析 DDG 结构失败（no_results）→ 自动降级 duckduckgo 成功
    let cfg = Config::new("rust", "bing,duckduckgo", BrowserKind::Fake)
        .with_max_results(5)
        .with_timeout(Duration::from_secs(5))
        .with_driver(Box::new(FixtureDriver::new(FIXTURE)));
    let outcome = app::run(cfg).await.expect("降级应成功");
    assert_eq!(outcome.meta.engine, "duckduckgo");
    assert_eq!(outcome.meta.engine_tried, vec!["bing", "duckduckgo"]);
    assert_eq!(outcome.results.len(), 3);
    assert!(!outcome.meta.low_yield);
}

#[tokio::test]
async fn first_engine_success_skips_fallback() {
    let cfg = Config::new("rust", "duckduckgo,bing", BrowserKind::Fake)
        .with_max_results(5)
        .with_timeout(Duration::from_secs(5))
        .with_driver(Box::new(FixtureDriver::new(FIXTURE)));
    let outcome = app::run(cfg).await.expect("应成功");
    assert_eq!(outcome.meta.engine, "duckduckgo");
    assert_eq!(outcome.meta.engine_tried, vec!["duckduckgo"]);
}

#[tokio::test]
async fn all_engines_fail_returns_stable_error_code() {
    // 全部引擎解析失败 → 稳定错误码 parse（exit 4），agent 可据此重试/换引擎
    let cfg = Config::new("rust", "bing,duckduckgo", BrowserKind::Fake)
        .with_max_results(5)
        .with_timeout(Duration::from_secs(5))
        .with_driver(Box::new(FixtureDriver::new("<html><body>空</body></html>")));
    let err = app::run(cfg).await.unwrap_err();
    assert_eq!(err.code_str(), "parse");
    assert_eq!(err.exit_code(), 4);
}

#[tokio::test]
async fn low_yield_engine_uses_best_candidate() {
    // bing 低产（2 条）→ 保留候选 → ddg 解析同页失败 → 候选兜底成功
    let cfg = Config::new("rust", "bing,duckduckgo", BrowserKind::Fake)
        .with_max_results(5)
        .with_timeout(Duration::from_secs(5))
        .with_driver(Box::new(FixtureDriver::new(low_yield_bing_html())));
    let outcome = app::run(cfg).await.expect("低产候选应兜底成功");
    assert_eq!(outcome.meta.engine, "bing");
    assert_eq!(outcome.meta.engine_tried, vec!["bing", "duckduckgo"]);
    assert_eq!(outcome.results.len(), 2);
    assert!(outcome.meta.low_yield);
}

#[tokio::test]
async fn multiple_low_yield_engines_picks_highest_yield() {
    // bing 2 条 + ddg 1 条（都低产，PageDriver 按 navigate 顺序分页）→ 选最高产候选
    let cfg = Config::new("rust", "bing,duckduckgo", BrowserKind::Fake)
        .with_max_results(5)
        .with_timeout(Duration::from_secs(5))
        .with_driver(Box::new(PageDriver::new(vec![
            low_yield_bing_html(),
            ddg_one_result_html(),
        ])));
    let outcome = app::run(cfg).await.expect("多低产应选最高产");
    assert_eq!(outcome.meta.engine, "bing");
    assert_eq!(outcome.meta.engine_tried, vec!["bing", "duckduckgo"]);
    assert_eq!(outcome.results.len(), 2);
    assert!(outcome.meta.low_yield);
}
