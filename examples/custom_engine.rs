//! 自定义引擎示例：实现 [`worbrow::SearchProvider`] 并经
//! [`worbrow::Config::with_provider`] 注入，无需复制 `run` 编排。
//!
//! 运行：`cargo run --example custom_engine`

use worbrow::error::EngineFailure;
use worbrow::{BrowserKind, Config, SearchProvider, SearchQuery, SearchResult, search};

/// 极简自定义引擎：固定返回一条结果（`result_url` 不会被访问，因 fake 驱动不导航）。
struct StaticEngine;

impl SearchProvider for StaticEngine {
    fn name(&self) -> &'static str {
        "static"
    }

    fn result_url(&self, _q: &SearchQuery) -> url::Url {
        url::Url::parse("data:text/html,<p>ok</p>").expect("data URL 合法")
    }

    fn result_selector(&self) -> &'static str {
        "p"
    }

    fn parse(&self, _html: &str) -> Result<Vec<SearchResult>, EngineFailure> {
        Ok(vec![SearchResult {
            rank: 1,
            title: "静态结果".into(),
            url: "https://example.com/".into(),
            snippet: "示例摘要".into(),
        }])
    }

    fn captcha_heuristics(&self) -> &[&'static str] {
        &[]
    }
}

fn main() -> Result<(), worbrow::Error> {
    // engine 名 "not-registered" 未在内置注册表，注入的 provider 优先生效
    let config =
        Config::new("q", "not-registered", BrowserKind::Fake).with_provider(Box::new(StaticEngine));
    let outcome = search(config)?;

    println!("engine: {}", outcome.meta.engine);
    for r in &outcome.results {
        println!("{}. {}", r.rank, r.title);
    }
    Ok(())
}
