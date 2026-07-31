//! 输出序列化：成功/失败 JSON 包（design.md §7.1）。

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

/// 成功包（stdout 唯一 JSON 输出）。
pub fn success(query: &str, results: &[SearchResult], meta: &SearchMeta) -> String {
    serde_json::to_string_pretty(&SuccessPayload {
        schema_version: SCHEMA_VERSION,
        query,
        results,
        meta,
    })
    .expect("序列化成功包不应失败")
}

/// 失败包：非 0 退出码时同样输出到 stdout，供 agent 结构化处理。
pub fn failure(err: &Error) -> String {
    serde_json::to_string_pretty(&ErrorPayload {
        schema_version: SCHEMA_VERSION,
        error: ErrorBody {
            code: err.code_str(),
            message: &err.to_string(),
            detail: None,
        },
    })
    .expect("序列化错误包不应失败")
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
            low_yield: true,
            captcha: false,
            engine_error: None,
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
    }
}
