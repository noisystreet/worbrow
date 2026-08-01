//! DuckDuckGo 适配器：URL 直访 html 端点 + HTML 解析。
//!
//! 选择器基线（DDG html 版）：结果容器 `div.result`，标题 `a.result__a`
//! （href 为 `uddg` 跳转参数，见 `extract::normalize_url`），摘要 `a.result__snippet`。
//! 引擎改版时更新本文件并同步 `tests/fixtures/duckduckgo.html`。

use scraper::{Html, Selector};
use url::Url;

use crate::domain::{SearchQuery, SearchResult};
use crate::error::EngineFailure;
use crate::extract::{clean_text, normalize_url};
use crate::ports::SearchProvider;

/// 骨架阶段默认使用 html 端点（解析更稳定，见 design.md 开放问题 3）。
const RESULT_URL: &str = "https://html.duckduckgo.com/html/";

pub struct DuckDuckGo;

impl SearchProvider for DuckDuckGo {
    fn name(&self) -> &'static str {
        "duckduckgo"
    }

    fn result_url(&self, q: &SearchQuery) -> Url {
        let mut url = Url::parse(RESULT_URL).expect("静态 URL 应合法");
        url.query_pairs_mut().append_pair("q", &q.text);
        // DDG 无独立语言参数，地域经 `kl`（如 zh-CN）
        if let Some(region) = &q.region {
            url.query_pairs_mut().append_pair("kl", region);
        }
        url
    }

    fn page_url(&self, q: &SearchQuery, page: usize) -> Url {
        let mut url = self.result_url(q);
        if page > 1 {
            // DDG html 端点每页 30 条，`s` 为起始偏移（30, 60, ...）
            url.query_pairs_mut()
                .append_pair("s", &((page - 1) * 30).to_string());
        }
        url
    }

    fn result_selector(&self) -> &'static str {
        "a.result__a"
    }

    fn parse(&self, html: &str) -> Result<Vec<SearchResult>, EngineFailure> {
        let document = Html::parse_document(html);
        let container = selector("div.result");
        let title = selector("a.result__a");
        let snippet = selector("a.result__snippet");

        let mut results = Vec::new();
        for (i, node) in document.select(&container).enumerate() {
            let Some(link) = node.select(&title).next() else {
                continue; // 缺少标题的结果项跳过（部分成功）
            };
            let title_text = clean_text(&link.text().collect::<String>());
            let raw_href = link.value().attr("href").unwrap_or_default().to_string();
            let snippet_text = node
                .select(&snippet)
                .next()
                .map(|e| clean_text(&e.text().collect::<String>()))
                .unwrap_or_default();

            results.push(SearchResult {
                rank: i + 1,
                title: title_text,
                url: normalize_url(&raw_href),
                snippet: snippet_text,
            });
        }

        if results.is_empty() {
            return Err(EngineFailure::new(
                "no_results",
                "页面结构未解析出任何结果（引擎改版或反爬）",
            ));
        }
        Ok(results)
    }

    fn captcha_heuristics(&self) -> &[&'static str] {
        &["anomaly", "challenge", "captcha"]
    }
}

/// 静态选择器编译失败视为编程错误。
fn selector(s: &str) -> Selector {
    Selector::parse(s).expect("静态选择器应合法")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/duckduckgo.html");

    #[test]
    fn result_url_encodes_query() {
        let q = SearchQuery {
            text: "rust 异步".into(),
            max_results: 10,
            lang: None,
            region: Some("zh-CN".into()),
            pages: 1,
        };
        let url = DuckDuckGo.result_url(&q);
        assert_eq!(url.host_str(), Some("html.duckduckgo.com"));
        assert!(url.as_str().contains("q=rust+%E5%BC%82%E6%AD%A5"));
        assert!(url.as_str().contains("kl=zh-CN"));
    }

    #[test]
    fn page_url_appends_offset() {
        let q = SearchQuery {
            text: "rust".into(),
            max_results: 10,
            lang: None,
            region: None,
            pages: 2,
        };
        // 第 2 页：s=30（DDG html 每页 30 条）
        assert!(DuckDuckGo.page_url(&q, 2).as_str().contains("s=30"));
        assert!(!DuckDuckGo.page_url(&q, 1).as_str().contains("s="));
    }

    #[test]
    fn parse_fixture_yields_expected_results() {
        let results = DuckDuckGo.parse(FIXTURE).expect("fixture 应可解析");
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].rank, 1);
        assert_eq!(results[0].title, "Rust 程序设计语言（示例标题一）");
        assert_eq!(results[0].url, "https://example.com/rust");
        assert_eq!(results[0].snippet, "这是摘要一");
        // uddg 跳转展开
        assert_eq!(results[2].url, "https://example.org/async");
    }

    #[test]
    fn parse_missing_structure_returns_engine_failure() {
        let err = DuckDuckGo
            .parse("<html><body>空</body></html>")
            .unwrap_err();
        assert_eq!(err.code, "no_results");
    }

    #[test]
    fn captcha_heuristics_covered() {
        let heuristics = DuckDuckGo.captcha_heuristics();
        assert!(heuristics.contains(&"captcha"));
    }
}
