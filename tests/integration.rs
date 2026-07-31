//! 端到端集成测试：FakeDriver + fixture → app::run → 校验 JSON/退出码（design.md §11）。
//! CI 无需真实浏览器。

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use url::Url;
use worbrow::app::{self, Config};
use worbrow::drivers::{BrowserKind, fake::FakeDriver};
use worbrow::error::Error;
use worbrow::ports::BrowserDriver;

const FIXTURE: &str = include_str!("fixtures/duckduckgo.html");

fn config(query: &str) -> Config {
    Config {
        query: query.to_string(),
        engine: "duckduckgo".to_string(),
        browser: BrowserKind::Fake,
        max_results: 5,
        timeout: Duration::from_secs(5),
        screenshot: None,
        dump_html: None,
        driver: Some(Box::new(FakeDriver::with_html(FIXTURE))),
    }
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
    let mut cfg = config("rust");
    cfg.max_results = 2;
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
    let mut cfg = config("rust");
    cfg.engine = "google".into();
    let err = app::run(cfg).await.unwrap_err();
    assert!(matches!(err, Error::Cli(_)));
}

#[tokio::test]
async fn captcha_html_yields_captcha_flag() {
    // 验证码特征词存在但仍有结果：标记 captcha=true，不中止
    let html = format!("<html>anomaly<body>{FIXTURE}</body></html>");
    let mut cfg = config("rust");
    cfg.driver = Some(Box::new(FakeDriver::with_html(html)));
    let outcome = app::run(cfg).await.expect("应成功");
    assert!(outcome.meta.captcha);
    assert!(!outcome.results.is_empty());
}

#[tokio::test]
async fn timeout_returns_timeout_error() {
    let mut cfg = config("rust");
    cfg.timeout = Duration::from_millis(1);
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
    let mut cfg = config("rust");
    cfg.timeout = Duration::from_millis(100);
    cfg.driver = Some(Box::new(SlowDriver));
    let err = app::run(cfg).await.unwrap_err();
    assert!(matches!(err, Error::Timeout(_)));
    // 退出码契约：超时 = 124（design.md §7.2）
    assert_eq!(err.exit_code(), 124);
}
