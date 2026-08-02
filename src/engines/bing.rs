//! Bing 适配器：URL 直访 + HTML 解析。
//!
//! Bing 搜索结果 HTML 结构相对稳定：结果容器 `li.b_algo`，标题 `h2 a`，摘要
//! `.b_caption`（或 `p.b_lineclamp2`）。Bing 结果链接可能是直接 URL，也可能是
//! `www.bing.com/ck/a` 点击追踪链（`u` 参数 base64 编码目标），经
//! `extract::normalize_url` 尽力展开（解码失败保持原样）。
//!
//! 引擎改版时更新本文件并同步 `tests/fixtures/bing.html`。

use scraper::{Html, Selector};
use url::Url;

use crate::domain::{SearchQuery, SearchResult};
use crate::error::EngineFailure;
use crate::extract::{clean_text, normalize_url};
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
        // q 用 engine_text：原 text 追加 site:/filetype:（输出契约的 query 字段仍保留原始 text）
        url.query_pairs_mut().append_pair("q", &q.engine_text());
        if let Some(lang) = &q.lang {
            url.query_pairs_mut().append_pair("setlang", lang);
        }
        if let Some(region) = &q.region {
            url.query_pairs_mut().append_pair("mkt", region);
        }
        if let Some(freshness) = q.freshness {
            // `qft=+filterui:age-lt<sec>`：Bing 时间过滤参数
            url.query_pairs_mut().append_pair(
                "qft",
                &format!("+filterui:age-lt{}", freshness.bing_seconds()),
            );
        }
        if let Some(safesearch) = q.safesearch {
            url.query_pairs_mut()
                .append_pair("adlt", safesearch.bing_param());
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
            let (url, url_resolved) = link
                .value()
                .attr("href")
                .map(normalize_url)
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
            let published_at = crate::extract::extract_date(&snippet_text);
            let result_kind = crate::extract::result_kind(&url);
            results.push(SearchResult {
                rank: i + 1,
                title: title_text,
                url,
                snippet: snippet_text,
                domain,
                https,
                published_at,
                is_ad: false, // b_algo 选择器不含广告容器（li.b_ad）
                url_resolved,
                result_kind,
            });
        }

        if results.is_empty() {
            return Err(EngineFailure::new(
                "no_results",
                "Bing page yielded no results (engine layout changed or anti-bot)",
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
// 测试断言序列（assert_eq 宏展开）非控制流复杂度，豁免门禁；生产代码仍严格 ≤10
#[allow(clippy::cognitive_complexity)]
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
            freshness: Some(crate::domain::Freshness::Week),
            safesearch: Some(crate::domain::SafesearchLevel::Strict),
            site: Some("doc.rust-lang.org".into()),
            filetype: None,
        };
        let url = Bing.result_url(&q);
        assert_eq!(url.host_str(), Some("www.bing.com"));
        // q 保留原始 text + 追加 site:（engine_text 语义）
        assert!(
            url.as_str()
                .contains("q=rust+%E5%BC%82%E6%AD%A5+site%3Adoc.rust-lang.org")
        );
        assert!(url.as_str().contains("setlang=zh-hans"));
        assert!(url.as_str().contains("mkt=zh-CN"));
        // 时间过滤 qft + 安全搜索 adlt（strict → Bing strict）
        assert!(url.as_str().contains("qft=%2Bfilterui%3Aage-lt604800"));
        assert!(url.as_str().contains("adlt=strict"));
    }

    #[test]
    fn result_url_appends_filetype() {
        let q = SearchQuery {
            text: "rust".into(),
            max_results: 10,
            lang: None,
            region: None,
            pages: 1,
            freshness: None,
            safesearch: Some(crate::domain::SafesearchLevel::Moderate),
            site: None,
            filetype: Some("pdf".into()),
        };
        let url = Bing.result_url(&q);
        assert!(url.as_str().contains("q=rust+filetype%3Apdf"));
        // moderate 映射 Bing strict（Bing 仅 off/strict 两级）
        assert!(url.as_str().contains("adlt=strict"));
    }

    #[test]
    fn page_url_appends_first_offset() {
        let q = SearchQuery {
            text: "rust".into(),
            max_results: 10,
            lang: None,
            region: None,
            pages: 2,
            freshness: None,
            safesearch: None,
            site: None,
            filetype: None,
        };
        // 第 2 页：first=10（每页 10 条）
        assert!(Bing.page_url(&q, 2).as_str().contains("first=10"));
        // 第 1 页：无 first 参数（与 result_url 一致）
        assert!(!Bing.page_url(&q, 1).as_str().contains("first="));
    }

    #[test]
    fn parse_expands_ck_redirect_url() {
        let html = r#"<html><body><ol id="b_results">
          <li class="b_algo">
            <h2><a href="https://www.bing.com/ck/a?!&p=abc&u=a1aHR0cHM6Ly9sb3BlemNhc3Ryb21pbC5jb20v">目标站</a></h2>
            <div class="b_caption"><p>摘要</p></div>
          </li>
        </ol></body></html>"#;
        let results = Bing.parse(html).expect("应可解析");
        assert_eq!(results.len(), 1);
        // ck/a 追踪链展开为真实目标 URL，domain/https 取自展开结果
        assert_eq!(results[0].url, "https://lopezcastromil.com/");
        assert_eq!(results[0].domain, "lopezcastromil.com");
        assert!(results[0].https);
        assert!(results[0].url_resolved, "ck/a 解码应标记已解跳转");
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
        // 发布日期：fixture 仅第 2/3 条摘要含日期（Bing 中文格式 `YYYY年M月D日 ·`）
        assert_eq!(results[0].published_at, None, "第一条摘要无日期");
        assert_eq!(results[1].published_at.as_deref(), Some("2025年5月25日"));
        assert_eq!(results[2].published_at.as_deref(), Some("2025年3月23日"));
        assert!(!results[0].is_ad, "b_algo 不含广告");
        assert!(!results[0].url_resolved, "fixture 为直接 URL，无需解跳转");
        // fixture URL 均为正常内容页 → result_kind 恒 web
        assert!(
            results
                .iter()
                .all(|r| r.result_kind == crate::domain::ResultKind::Web)
        );
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
