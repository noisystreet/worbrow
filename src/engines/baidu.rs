//! 百度适配器：URL 直访 + HTML 解析。
//!
//! 百度 SERP 结构（2026-08 实测）：有机结果容器 `div.result`，标题 `h3 a`；真实目标
//! URL 在容器 `mu` 属性（href 为 `www.baidu.com/link?url=` 跳转链，目标不可本地
//! 解码，经 `mu` 拿真实地址）；摘要 `.c-abstract` / `span.content-right`（新版 cos
//! 布局为 `div[class*=content-space-between]`，正文由 JS 水合渲染，静态 HTML 可能
//! 为空）。解析尽力语义：`mu` 缺失时保留 `baidu.com/link` 链式 URL（url_resolved=false）。
//!
//! 引擎改版时更新本文件并同步 `tests/fixtures/baidu.html`。

use scraper::{Html, Selector};
use url::Url;

use crate::domain::{SearchQuery, SearchResult};
use crate::error::EngineFailure;
use crate::extract::{clean_text, normalize_url};
use crate::ports::SearchProvider;

/// 百度搜索 URL。
const RESULT_URL: &str = "https://www.baidu.com/s";

pub struct Baidu;

impl SearchProvider for Baidu {
    fn name(&self) -> &'static str {
        "baidu"
    }

    fn result_url(&self, q: &SearchQuery) -> Url {
        let mut url = Url::parse(RESULT_URL).expect("静态 URL 应合法");
        // q 用 engine_text：原 text 追加 site:/filetype:（输出契约的 query 字段仍保留原始 text）
        url.query_pairs_mut().append_pair("wd", &q.engine_text());
        url
    }

    fn page_url(&self, q: &SearchQuery, page: usize) -> Url {
        let mut url = self.result_url(q);
        if page > 1 {
            // 百度每页 10 条，`pn` 为起始偏移（10, 20, ...）
            url.query_pairs_mut()
                .append_pair("pn", &((page - 1) * 10).to_string());
        }
        url
    }

    fn result_selector(&self) -> &'static str {
        "div.result"
    }

    fn parse(&self, html: &str) -> Result<Vec<SearchResult>, EngineFailure> {
        let document = Html::parse_document(html);
        let container = selector("div.result");
        let title_link = selector("h3 a");
        let snippet =
            selector(".c-abstract, span.content-right, div[class*='content-space-between']");

        let mut results = Vec::new();
        for (i, node) in document.select(&container).enumerate() {
            let Some(link) = node.select(&title_link).next() else {
                continue; // 缺少标题的结果项跳过（部分成功）
            };
            let title_text = clean_text(&link.text().collect::<String>());
            // 真实目标 URL：优先容器 `mu` 属性（百度/DDG 同类跳转解开的真实地址）；
            // 缺失时回退 href（`www.baidu.com/link?url=` 链式 URL，无法本地解码）
            let (url, url_resolved) = node
                .value()
                .attr("mu")
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(|m| (m.to_string(), true))
                .or_else(|| {
                    link.value()
                        .attr("href")
                        .map(normalize_url)
                        .map(|(u, _)| (u, false))
                })
                .unwrap_or_default();
            let snippet_text = node
                .select(&snippet)
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
                is_ad: false, // 广告容器为 result-op 等其它 class，div.result 不含广告位
                url_resolved,
                result_kind,
            });
        }

        if results.is_empty() {
            return Err(EngineFailure::new(
                "no_results",
                "Baidu page yielded no results (engine layout changed or anti-bot)",
            ));
        }
        Ok(results)
    }

    fn captcha_heuristics(&self) -> &[&'static str] {
        &["安全验证", "百度安全验证", "captcha", "verify"]
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

    const FIXTURE: &str = include_str!("../../tests/fixtures/baidu.html");

    #[test]
    fn result_url_encodes_query() {
        let q = SearchQuery {
            text: "天天基金网 净值查询".into(),
            max_results: 10,
            lang: None,
            region: None,
            pages: 1,
            freshness: None,
            safesearch: None,
            site: Some("eastmoney.com".into()),
            filetype: None,
        };
        let url = Baidu.result_url(&q);
        assert_eq!(url.host_str(), Some("www.baidu.com"));
        // wd 保留原始 text + 追加 site:（engine_text 语义）
        assert!(
            url.as_str()
                .contains("wd=%E5%A4%A9%E5%A4%A9%E5%9F%BA%E9%87%91%E7%BD%91+%E5%87%80%E5%80%BC%E6%9F%A5%E8%AF%A2+site%3Aeastmoney.com")
        );
    }

    #[test]
    fn page_url_appends_pn_offset() {
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
        // 第 2 页：pn=10（每页 10 条）
        assert!(Baidu.page_url(&q, 2).as_str().contains("pn=10"));
        // 第 1 页：无 pn 参数
        assert!(!Baidu.page_url(&q, 1).as_str().contains("pn="));
    }

    #[test]
    fn parse_fixture_yields_expected_results() {
        let results = Baidu.parse(FIXTURE).expect("fixture 应可解析");
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].rank, 1);
        assert_eq!(
            results[0].title,
            "天天基金网(1234567.com.cn) --首批独立基金销售机构-- 东方财富网旗下基金平台!"
        );
        // mu 展开为真实目标 URL，domain/https 取自展开结果
        assert_eq!(results[0].url, "https://fund.eastmoney.com/");
        assert_eq!(results[0].domain, "fund.eastmoney.com");
        assert!(results[0].https);
        assert!(results[0].url_resolved, "mu 展开应标记已解跳转");
        assert!(
            results[0]
                .snippet
                .contains("东方财富旗下专业的基金交易平台")
        );
        assert!(!results[0].is_ad, "div.result 不含广告位");
        assert!(results[1].url_resolved, "第二条 mu 同样展开");
        // 缺 mu 的容器：回退 baidu.com/link 链式 URL，未解跳转
        assert_eq!(results[2].url, "http://www.baidu.com/link?url=ghi789");
        assert!(!results[2].url_resolved);
        assert_eq!(results[2].domain, "www.baidu.com");
        // fixture URL 均为正常内容页 → result_kind 恒 web
        assert!(
            results
                .iter()
                .all(|r| r.result_kind == crate::domain::ResultKind::Web)
        );
    }

    #[test]
    fn parse_missing_structure_returns_engine_failure() {
        let err = Baidu.parse("<html><body>空</body></html>").unwrap_err();
        assert_eq!(err.code, "no_results");
    }

    #[test]
    fn captcha_heuristics_covered() {
        let heuristics = Baidu.captcha_heuristics();
        assert!(heuristics.contains(&"安全验证"));
        assert!(heuristics.contains(&"captcha"));
    }

    #[test]
    fn name_is_baidu() {
        assert_eq!(Baidu.name(), "baidu");
    }
}
