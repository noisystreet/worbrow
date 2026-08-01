//! Bing 适配器：URL 直访 + HTML 解析。
//!
//! Bing 搜索结果 HTML 结构相对稳定：结果容器 `li.b_algo`，标题 `h2 a`，摘要
//! `.b_caption`（或 `p.b_lineclamp2`）。Bing 的搜索结果链接为直接 URL（无跳转
//! 参数），无需像 DDG 那样展开 uddg 重定向。
//!
//! 引擎改版时更新本文件并同步 `tests/fixtures/bing.html`。

use scraper::{Html, Selector};
use url::Url;

use crate::domain::{SearchQuery, SearchResult};
use crate::error::EngineFailure;
use crate::extract::clean_text;
use crate::ports::SearchProvider;

/// Bing 搜索 URL（html 端点）。
const RESULT_URL: &str = "https://www.bing.com/search";

pub struct Bing;

impl SearchProvider for Bing {
    fn name(&self) -> &'static str {
        "bing"
    }

    fn result_url(&self, q: &SearchQuery) -> Url {
        let mut url = Url::parse(RESULT_URL).expect("静态 URL 应合法");
        url.query_pairs_mut().append_pair("q", &q.text);
        if let Some(lang) = &q.lang {
            url.query_pairs_mut().append_pair("setlang", lang);
        }
        if let Some(region) = &q.region {
            url.query_pairs_mut().append_pair("mkt", region);
        }
        url
    }

    fn page_url(&self, q: &SearchQuery, page: usize) -> Url {
        let mut url = self.result_url(q);
        if page > 1 {
            // Bing 每页 10 条，`first` 为起始偏移（0, 10, 20, ...）
            url.query_pairs_mut()
                .append_pair("first", &((page - 1) * 10).to_string());
        }
        url
    }

    fn result_selector(&self) -> &'static str {
        "li.b_algo"
    }

    fn parse(&self, html: &str) -> Result<Vec<SearchResult>, EngineFailure> {
        let document = Html::parse_document(html);
        let container = selector("li.b_algo");
        let title_link = selector("h2 a");
        let caption = selector(".b_caption");

        let mut results = Vec::new();
        for (i, node) in document.select(&container).enumerate() {
            let Some(link) = node.select(&title_link).next() else {
                continue; // 缺少标题的结果项跳过（部分成功）
            };
            let title_text = clean_text(&link.text().collect::<String>());
            let url = link
                .value()
                .attr("href")
                .map(|s| s.to_string())
                .unwrap_or_default();
            let snippet_text = node
                .select(&caption)
                .next()
                .map(|e| clean_text(&e.text().collect::<String>()))
                .unwrap_or_default();

            if title_text.is_empty() && url.is_empty() {
                continue;
            }

            let (domain, https) = crate::extract::url_origin(&url);
            results.push(SearchResult {
                rank: i + 1,
                title: title_text,
                url,
                snippet: snippet_text,
                domain,
                https,
            });
        }

        if results.is_empty() {
            return Err(EngineFailure::new(
                "no_results",
                "Bing 页面结构未解析出任何结果（引擎改版或反爬）",
            ));
        }
        Ok(results)
    }

    fn captcha_heuristics(&self) -> &[&'static str] {
        &["captcha", "verify", "sorry", "are you human"]
    }
}

/// 静态选择器编译失败视为编程错误。
fn selector(s: &str) -> Selector {
    Selector::parse(s).expect("静态选择器应合法")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/bing.html");

    #[test]
    fn result_url_encodes_query() {
        let q = SearchQuery {
            text: "rust 异步".into(),
            max_results: 10,
            lang: Some("zh-hans".into()),
            region: Some("zh-CN".into()),
            pages: 1,
        };
        let url = Bing.result_url(&q);
        assert_eq!(url.host_str(), Some("www.bing.com"));
        assert!(url.as_str().contains("q=rust+%E5%BC%82%E6%AD%A5"));
        assert!(url.as_str().contains("setlang=zh-hans"));
        assert!(url.as_str().contains("mkt=zh-CN"));
    }

    #[test]
    fn page_url_appends_first_offset() {
        let q = SearchQuery {
            text: "rust".into(),
            max_results: 10,
            lang: None,
            region: None,
            pages: 2,
        };
        // 第 2 页：first=10（每页 10 条）
        assert!(Bing.page_url(&q, 2).as_str().contains("first=10"));
        // 第 1 页：无 first 参数（与 result_url 一致）
        assert!(!Bing.page_url(&q, 1).as_str().contains("first="));
    }

    #[test]
    fn parse_fixture_yields_expected_results() {
        let results = Bing.parse(FIXTURE).expect("fixture 应可解析");
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].rank, 1);
        assert_eq!(results[0].title, "Rust 异步编程 async/await | 菜鸟教程");
        assert_eq!(
            results[0].url,
            "https://www.runoob.com/rust/rust-async-await.html"
        );
        assert_eq!(results[0].domain, "www.runoob.com");
        assert!(results[0].https);
        assert!(results[0].snippet.contains("异步编程"));
        // 第二条：URL 来自 href 直接提取，无跳转参数
        assert_eq!(
            results[1].url,
            "https://doc.rust-lang.net.cn/book/ch17-00-async-await.html"
        );
        assert_eq!(results[2].title, "深入异步 | Tokio - 一个异步 Rust 运行时");
    }

    #[test]
    fn parse_missing_structure_returns_engine_failure() {
        let err = Bing.parse("<html><body>空</body></html>").unwrap_err();
        assert_eq!(err.code, "no_results");
    }

    #[test]
    fn captcha_heuristics_covered() {
        let heuristics = Bing.captcha_heuristics();
        assert!(heuristics.contains(&"captcha"));
    }

    #[test]
    fn result_selector_is_li_b_algo() {
        assert_eq!(Bing.result_selector(), "li.b_algo");
    }

    #[test]
    fn name_is_bing() {
        assert_eq!(Bing.name(), "bing");
    }
}
