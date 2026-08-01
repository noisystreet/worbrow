//! 外部视角 API 测试：仅使用 lib 公开面（顶层 re-export + app/error/output），
//! 模拟第三方消费者，作为公开面回归门禁（docs/roadmap-lib-api.md P0）。
//! CI 无需真实浏览器（fake 驱动 + fixture）。

use worbrow::app::{self, Config};
use worbrow::drivers::BrowserKind;
use worbrow::error::Error;
use worbrow::{
    DEFAULT_ENGINE, DEFAULT_MAX_RESULTS, DEFAULT_TIMEOUT_SECS, EngineError, SearchMeta,
    SearchResult,
};

/// 编译期可命名性（P0 补漏项）：`EngineError`/`SearchResult`/`SearchMeta`
/// 均从顶层 `worbrow::*` 引入，直接出现在变量类型注解中即可验证。
#[test]
fn public_types_are_namable() {
    let _: Option<EngineError> = None;
    let _: Option<SearchResult> = None;
    let _: Option<SearchMeta> = None;
}

#[test]
fn run_sync_works_from_external_crate() {
    let config = Config::new("rust", DEFAULT_ENGINE, BrowserKind::Fake);
    let outcome = app::run_sync(config).expect("fake 驱动 + bing fixture 应成功");
    assert!(!outcome.results.is_empty());
    assert_eq!(outcome.meta.engine, DEFAULT_ENGINE);
    assert_eq!(outcome.meta.result_count, outcome.results.len());
    assert!(outcome.meta.engine_error.is_none());
    // 排名从 1 开始
    assert_eq!(outcome.results[0].rank, 1);
}

#[test]
fn defaults_are_reachable_from_root() {
    assert_eq!(DEFAULT_ENGINE, "bing");
    assert_eq!(DEFAULT_MAX_RESULTS, 10);
    assert_eq!(DEFAULT_TIMEOUT_SECS, 60);
}

#[test]
fn output_contract_is_schema_v1() {
    let config = Config::new("rust", DEFAULT_ENGINE, BrowserKind::Fake);
    let outcome = app::run_sync(config).expect("fake 驱动应成功");
    let body = worbrow::output::success(&outcome.query, &outcome.results, &outcome.meta);
    let v: serde_json::Value = serde_json::from_str(&body).expect("契约 JSON 可解析");
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["query"], "rust");
    assert!(v["results"].is_array());
    assert!(v["meta"]["engine_error"].is_null());
}

#[test]
fn error_type_is_usable_externally() {
    let err = Error::Engine(worbrow::error::EngineFailure::new("no_results", "x"));
    assert_eq!(err.code_str(), "parse");
    assert_eq!(err.exit_code(), 4);
    assert_eq!(err.detail(), Some("no_results".into()));
}
