//! 输出序列化：成功/失败 JSON 包（design.md §7.1）；无 `--json` 时为人读文本。

use std::fmt::Write as _;

use serde::Serialize;

use crate::domain::{SearchMeta, SearchResult};
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
        };
        let results = [SearchResult {
            rank: 1,
            title: "T".into(),
            url: "https://example.com".into(),
            snippet: "S".into(),
            domain: "example.com".into(),
            https: true,
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
}
