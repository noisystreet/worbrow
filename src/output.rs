//! 输出序列化：成功/失败 JSON 包（design.md §7.1）；无 `--json` 时为人读文本。

use std::fmt::Write as _;

use serde::Serialize;

use crate::domain::{FetchedPage, SearchMeta, SearchResult};
use crate::error::Error;

/// 输出 schema 主版本。字段只增不改；破坏性变更 bump 主版本。
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize)]
pub struct SuccessPayload<'a> {
    pub schema_version: u32,
    pub query: &'a str,
    pub results: &'a [SearchResult],
    pub meta: &'a SearchMeta,
}

/// 精简模式成功包（MCP `compact=true`）：results 仅 rank/title/url，
/// 省 agent 上下文 token；schema v1 语义不变（紧凑只读视图，roadmap「MCP 体验完善」）。
#[derive(Serialize)]
pub struct CompactSuccessPayload<'a> {
    pub schema_version: u32,
    pub query: &'a str,
    pub results: &'a [CompactResult<'a>],
    pub meta: &'a SearchMeta,
}

/// 精简结果条目：仅保留 rank/title/url（MCP compact 模式）。
#[derive(Serialize)]
pub struct CompactResult<'a> {
    pub rank: usize,
    pub title: &'a str,
    pub url: &'a str,
}

#[derive(Serialize)]
pub struct ErrorPayload<'a> {
    pub schema_version: u32,
    pub error: ErrorBody<'a>,
}

#[derive(Serialize)]
pub struct ErrorBody<'a> {
    pub code: &'static str,
    pub message: &'a str,
    pub detail: Option<String>,
}

/// 成功包（`--json` 时 stdout）。
pub fn success(query: &str, results: &[SearchResult], meta: &SearchMeta) -> String {
    serde_json::to_string_pretty(&SuccessPayload {
        schema_version: SCHEMA_VERSION,
        query,
        results,
        meta,
    })
    .expect("序列化成功包不应失败")
}

/// 精简成功包（MCP `compact=true`）：results 仅 rank/title/url（schema v1 只读视图）。
pub fn success_compact(query: &str, results: &[SearchResult], meta: &SearchMeta) -> String {
    let compact: Vec<CompactResult<'_>> = results
        .iter()
        .map(|r| CompactResult {
            rank: r.rank,
            title: &r.title,
            url: &r.url,
        })
        .collect();
    serde_json::to_string_pretty(&CompactSuccessPayload {
        schema_version: SCHEMA_VERSION,
        query,
        results: &compact,
        meta,
    })
    .expect("序列化精简成功包不应失败")
}

/// 失败包：`--json` 且非 0 退出码时输出到 stdout，供 agent 结构化处理。
pub fn failure(err: &Error) -> String {
    serde_json::to_string_pretty(&ErrorPayload {
        schema_version: SCHEMA_VERSION,
        error: ErrorBody {
            code: err.code_str(),
            message: &err.to_string(),
            detail: err.detail(),
        },
    })
    .expect("序列化错误包不应失败")
}

/// 人读成功文本（无 `--json`）。
pub fn success_text(query: &str, results: &[SearchResult], meta: &SearchMeta) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "query: {query}\nengine: {}  results: {}  elapsed: {}ms",
        meta.engine, meta.result_count, meta.elapsed_ms
    );
    if meta.captcha {
        let _ = writeln!(out, "warning: captcha detected");
    }
    if meta.low_yield {
        let _ = writeln!(out, "warning: low yield (<3 results)");
    }
    for r in results {
        let _ = writeln!(
            out,
            "\n{}. {}\n   {}\n   {}",
            r.rank, r.title, r.url, r.snippet
        );
    }
    out
}

/// 人读失败文本（无 `--json`）。
pub fn failure_text(err: &Error) -> String {
    format!("错误 [{}]: {err}\n", err.code_str())
}

/// 抓取成功包（`worbrow fetch` / MCP `fetch_page`，ADR-009）：新 sibling 契约，
/// 与 search 成功包同 `schema_version`；对既有客户端无感。
#[derive(Serialize)]
pub struct FetchSuccessPayload<'a> {
    pub schema_version: u32,
    pub url: &'a str,
    pub fetched_at: chrono::DateTime<chrono::Utc>,
    /// 清洗后正文（`text=false` 时为空串）。
    pub text: &'a str,
    /// 结构化字段（键 = `ExtractField::as_str`；缺失字段不出现）。
    pub extracted: &'a serde_json::Map<String, serde_json::Value>,
    pub meta: FetchMeta<'a>,
}

/// 抓取元信息。
#[derive(Serialize)]
pub struct FetchMeta<'a> {
    pub elapsed_ms: u64,
    pub chars: usize,
    pub truncated: bool,
    /// 重定向落地页（导航后 `location.href`）。
    pub final_url: Option<&'a str>,
}

/// 抓取成功包（`--json` 时 stdout）。
pub fn fetch_success(page: &FetchedPage) -> String {
    serde_json::to_string_pretty(&FetchSuccessPayload {
        schema_version: SCHEMA_VERSION,
        url: &page.url,
        fetched_at: page.fetched_at,
        text: &page.text,
        extracted: &page.extracted,
        meta: FetchMeta {
            elapsed_ms: page.elapsed_ms,
            chars: page.chars,
            truncated: page.truncated,
            final_url: page.final_url.as_deref(),
        },
    })
    .expect("序列化抓取成功包不应失败")
}

/// 人读抓取成功文本（无 `--json`）。
pub fn fetch_success_text(page: &FetchedPage) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "url: {}\nchars: {}  elapsed: {}ms{}",
        page.url,
        page.chars,
        page.elapsed_ms,
        if page.truncated { "  (truncated)" } else { "" }
    );
    if let Some(final_url) = &page.final_url
        && final_url != &page.url
    {
        let _ = writeln!(out, "redirected: {final_url}");
    }
    if !page.extracted.is_empty() {
        let _ = writeln!(out, "extracted:");
        for (k, v) in &page.extracted {
            let _ = writeln!(out, "  {k}: {v}");
        }
    }
    if !page.text.is_empty() {
        let _ = writeln!(out, "\n{}", page.text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn success_payload_contains_schema_version_and_low_yield() {
        let meta = SearchMeta {
            engine: "duckduckgo",
            started_at: Utc::now(),
            elapsed_ms: 10,
            result_count: 0,
            pages: 1,
            low_yield: true,
            captcha: false,
            engine_error: None,
            engine_tried: vec!["duckduckgo".to_string()],
            cached: false,
            retries: 0,
        };
        let json = success("q", &[], &meta);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["meta"]["low_yield"], true);
        assert_eq!(parsed["results"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn failure_payload_matches_exit_code_semantics() {
        let err = Error::Timeout("x".into());
        let parsed: serde_json::Value = serde_json::from_str(&failure(&err)).unwrap();
        assert_eq!(parsed["error"]["code"], "timeout");
        assert_eq!(parsed["error"]["detail"], serde_json::Value::Null);
    }

    #[test]
    fn failure_detail_contains_engine_code() {
        let err = Error::Engine(crate::error::EngineFailure::new("no_results", "页面无结果"));
        let parsed: serde_json::Value = serde_json::from_str(&failure(&err)).unwrap();
        assert_eq!(parsed["error"]["code"], "parse");
        assert_eq!(parsed["error"]["detail"], "no_results");
    }

    #[test]
    fn success_text_lists_results() {
        let meta = SearchMeta {
            engine: "duckduckgo",
            started_at: Utc::now(),
            elapsed_ms: 42,
            result_count: 1,
            pages: 1,
            low_yield: true,
            captcha: false,
            engine_error: None,
            engine_tried: vec!["duckduckgo".to_string()],
            cached: false,
            retries: 0,
        };
        let results = [SearchResult {
            rank: 1,
            title: "T".into(),
            url: "https://example.com".into(),
            snippet: "S".into(),
            domain: "example.com".into(),
            https: true,
            published_at: None,
            is_ad: false,
            url_resolved: false,
            result_kind: crate::domain::ResultKind::Web,
        }];
        let text = success_text("q", &results, &meta);
        assert!(text.contains("query: q"));
        assert!(text.contains("engine: duckduckgo"));
        assert!(text.contains("1. T"));
        assert!(text.contains("https://example.com"));
        assert!(text.contains("warning: low yield"));
        assert!(!text.contains("schema_version"));
    }

    #[test]
    fn failure_text_includes_code() {
        let text = failure_text(&Error::Cli("缺参".into()));
        assert!(text.contains("[cli]"));
        assert!(text.contains("缺参"));
        assert!(!text.contains("schema_version"));
    }

    /// compact 精简包：results 仅 rank/title/url，无 snippet/domain 等冗余字段。
    #[test]
    fn compact_payload_only_contains_rank_title_url() {
        let meta = SearchMeta {
            engine: "bing",
            started_at: Utc::now(),
            elapsed_ms: 10,
            result_count: 1,
            pages: 1,
            low_yield: false,
            captcha: false,
            engine_error: None,
            engine_tried: vec!["bing".to_string()],
            cached: false,
            retries: 0,
        };
        let results = [SearchResult {
            rank: 1,
            title: "T".into(),
            url: "https://example.com".into(),
            snippet: "SNIPPET".into(),
            domain: "example.com".into(),
            https: true,
            published_at: Some("2026-08-01".into()),
            is_ad: false,
            url_resolved: true,
            result_kind: crate::domain::ResultKind::Web,
        }];
        let json = success_compact("q", &results, &meta);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["query"], "q");
        // 精简条目只含 rank/title/url
        assert_eq!(parsed["results"][0]["rank"], 1);
        assert_eq!(parsed["results"][0]["title"], "T");
        assert_eq!(parsed["results"][0]["url"], "https://example.com");
        assert!(
            parsed["results"][0].get("snippet").is_none(),
            "compact 不含 snippet"
        );
        assert!(
            parsed["results"][0].get("domain").is_none(),
            "compact 不含 domain"
        );
        // meta 完整保留
        assert_eq!(parsed["meta"]["engine"], "bing");
    }

    /// fetch 成功包：schema_version/url/text/extracted/meta 形状完整。
    #[test]
    fn fetch_success_payload_has_contract_shape() {
        use crate::domain::ExtractField as F;
        let page = FetchedPage {
            url: "https://example.com/a".into(),
            fetched_at: Utc::now(),
            text: "正文内容".into(),
            extracted: serde_json::Map::from_iter([(
                F::Price.as_str().to_string(),
                serde_json::json!("1299.00"),
            )]),
            elapsed_ms: 42,
            chars: 4,
            truncated: false,
            final_url: Some("https://example.com/b".into()),
        };
        let json = fetch_success(&page);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["url"], "https://example.com/a");
        assert_eq!(parsed["text"], "正文内容");
        assert_eq!(parsed["extracted"]["price"], "1299.00");
        assert_eq!(parsed["meta"]["chars"], 4);
        assert_eq!(parsed["meta"]["truncated"], false);
        assert_eq!(parsed["meta"]["final_url"], "https://example.com/b");
        // 失败包复用统一信封
        let err = Error::Cli("bad url".into());
        let failed = failure(&err);
        let parsed: serde_json::Value = serde_json::from_str(&failed).unwrap();
        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["error"]["code"], "cli");
    }

    /// fetch 人读文本：含 url/chars/extracted/正文，无 schema_version。
    #[test]
    fn fetch_success_text_is_human_readable() {
        let page = FetchedPage {
            url: "https://example.com/a".into(),
            fetched_at: Utc::now(),
            text: "正文内容".into(),
            extracted: serde_json::Map::new(),
            elapsed_ms: 10,
            chars: 4,
            truncated: true,
            final_url: None,
        };
        let text = fetch_success_text(&page);
        assert!(text.contains("url: https://example.com/a"));
        assert!(text.contains("(truncated)"));
        assert!(text.contains("正文内容"));
        assert!(!text.contains("schema_version"));
    }

    /// fetch 人读文本：重定向落地页与请求 URL 不同 → 输出 redirected 行。
    #[test]
    fn fetch_success_text_shows_redirect() {
        let page = FetchedPage {
            url: "https://example.com/a".into(),
            fetched_at: Utc::now(),
            text: String::new(),
            extracted: serde_json::Map::new(),
            elapsed_ms: 10,
            chars: 0,
            truncated: false,
            final_url: Some("https://example.com/b".into()),
        };
        let text = fetch_success_text(&page);
        assert!(
            text.contains("redirected: https://example.com/b"),
            "final_url 与 url 不同时输出 redirected 行（实际: {text}）"
        );
        // final_url == url 时不输出 redirected
        let page_same = FetchedPage {
            final_url: Some("https://example.com/a".into()),
            ..page
        };
        assert!(!fetch_success_text(&page_same).contains("redirected"));
    }
}
