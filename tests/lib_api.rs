//! 外部视角 API 测试：仅使用 lib 公开面（顶层 re-export + app/error/output），
//! 模拟第三方消费者，作为公开面回归门禁（docs/roadmap-lib-api.md P0）。
//! CI 无需真实浏览器（fake 驱动 + fixture）。

use worbrow::app::Config;
use worbrow::error::{EngineFailure, Error};
use worbrow::{
    BrowserKind, DEFAULT_ENGINE, DEFAULT_MAX_RESULTS, DEFAULT_TIMEOUT_SECS, EngineError,
    ResultKind, SearchMeta, SearchProvider, SearchQuery, SearchResult,
};

/// 编译期可命名性（P0 补漏项）：`EngineError`/`SearchResult`/`SearchMeta`
/// 均从顶层 `worbrow::*` 引入，直接出现在变量类型注解中即可验证。
#[test]
fn public_types_are_namable() {
    let _: Option<EngineError> = None;
    let _: Option<SearchResult> = None;
    let _: Option<SearchMeta> = None;
    let _: Option<BrowserKind> = None;
}

/// 自定义引擎：验证 `Config::with_provider` 扩展点（P1，外部无需复制 run 编排）。
/// `result_url` 不真正被访问（fake 驱动），parse 返回固定结果即可。
struct DummyEngine;

impl SearchProvider for DummyEngine {
    fn name(&self) -> &'static str {
        "dummy"
    }
    fn result_url(&self, _q: &SearchQuery) -> url::Url {
        url::Url::parse("data:text/html,<p>dummy</p>").expect("data URL 合法")
    }
    fn result_selector(&self) -> &'static str {
        "p"
    }
    fn parse(&self, _html: &str) -> Result<Vec<SearchResult>, EngineFailure> {
        Ok(vec![SearchResult {
            rank: 1,
            title: "dummy title".into(),
            url: "https://example.com/".into(),
            snippet: "dummy snippet".into(),
            domain: "example.com".into(),
            https: true,
            published_at: None,
            is_ad: false,
            url_resolved: false,
            result_kind: ResultKind::Web,
        }])
    }
    fn captcha_heuristics(&self) -> &[&'static str] {
        &[]
    }
}

#[tokio::test]
async fn custom_provider_is_injected_over_registry() {
    // engine 名未注册（"not-registered"），注入的 provider 优先于注册表
    let config = Config::new("rust", "not-registered", BrowserKind::Fake)
        .with_provider(Box::new(DummyEngine));
    let outcome = worbrow::run(config).await.expect("自定义引擎应可用");
    assert_eq!(outcome.meta.engine, "dummy");
    assert_eq!(outcome.results.len(), 1);
    assert_eq!(outcome.results[0].rank, 1);
}

#[test]
fn search_works_from_external_crate() {
    let config = Config::new("rust", DEFAULT_ENGINE, BrowserKind::Fake);
    // 顶层 `search` 同步入口
    let outcome = worbrow::search(config).expect("fake 驱动 + bing fixture 应成功");
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
fn pages_param_is_exposed_to_library_users() {
    // fake 驱动每页返回相同 HTML（去重后仍 3 条），验证翻页参数外部可达
    let config = Config::new("rust", DEFAULT_ENGINE, BrowserKind::Fake).with_pages(2);
    let outcome = worbrow::search(config).expect("fake 驱动应成功");
    assert_eq!(outcome.meta.pages, 2, "实际聚合页数应透传");
    assert_eq!(outcome.results.len(), 3, "相同 HTML 去重后保持 3 条");
}

#[test]
fn output_contract_is_schema_v1() {
    let config = Config::new("rust", DEFAULT_ENGINE, BrowserKind::Fake);
    let outcome = worbrow::search(config).expect("fake 驱动应成功");
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
    // source chain：底层 EngineFailure 可下钻（P1 复核项）
    let source = std::error::Error::source(&err).expect("Engine 变体应有 source");
    assert!(source.to_string().contains("no_results"));
}
