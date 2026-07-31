//! 错误类型与退出码映射（design.md §7.2）。

use thiserror::Error;

/// 顶层错误，与退出码一一对应。
#[derive(Debug, Error)]
pub enum Error {
    /// 参数错误 → exit 2
    #[error("参数错误: {0}")]
    Cli(String),
    /// 环境错误（浏览器未安装/协议连通失败）→ exit 3
    #[error("环境错误: {0}")]
    Env(String),
    /// 网络错误 → exit 4
    #[error("网络错误: {0}")]
    Network(String),
    /// 引擎解析失败 → exit 4
    #[error("引擎错误: {0}")]
    Engine(#[from] EngineFailure),
    /// 验证码/反爬阻止 → exit 4
    #[error("验证码/反爬: {0}")]
    Captcha(String),
    /// 超时 → exit 124（对齐 GNU timeout 语义）
    #[error("超时: {0}")]
    Timeout(String),
    /// 功能未实现（骨架阶段占位）→ exit 1
    #[error("未实现: {0}")]
    NotImplemented(String),
    /// 内部错误 → exit 1
    #[error("内部错误: {0}")]
    Internal(String),
}

impl Error {
    /// 退出码映射（design.md §7.2）。
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Cli(_) => 2,
            Error::Env(_) => 3,
            Error::Network(_) | Error::Engine(_) | Error::Captcha(_) => 4,
            Error::Timeout(_) => 124,
            Error::NotImplemented(_) | Error::Internal(_) => 1,
        }
    }

    /// 错误 JSON 中的稳定 code（design.md §7.1 失败包）。
    pub fn code_str(&self) -> &'static str {
        match self {
            Error::Cli(_) => "cli",
            Error::Env(_) => "env",
            Error::Network(_) => "network",
            Error::Engine(_) => "parse",
            Error::Captcha(_) => "captcha",
            Error::Timeout(_) => "timeout",
            Error::NotImplemented(_) => "not_implemented",
            Error::Internal(_) => "internal",
        }
    }
}

/// 引擎解析/页面结构异常（适配器侧构造）。
#[derive(Debug, Error)]
#[error("{message} (code: {code})")]
pub struct EngineFailure {
    pub code: String,
    pub message: String,
}

impl EngineFailure {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl From<tokio::time::error::Elapsed> for Error {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        Error::Timeout("任务超时".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 退出码契约冻结（design.md §7.2）。
    #[test]
    fn exit_codes_match_contract() {
        let cases = [
            (Error::Cli("x".into()), 2),
            (Error::Env("x".into()), 3),
            (Error::Network("x".into()), 4),
            (Error::Engine(EngineFailure::new("no_results", "x")), 4),
            (Error::Captcha("x".into()), 4),
            (Error::Timeout("x".into()), 124),
            (Error::NotImplemented("x".into()), 1),
            (Error::Internal("x".into()), 1),
        ];
        for (err, expected) in cases {
            assert_eq!(err.exit_code(), expected, "错误: {err:?}");
        }
    }

    /// 错误 JSON 的稳定 code（design.md §7.1 失败包）。
    #[test]
    fn code_strs_are_stable() {
        let cases = [
            (Error::Cli("x".into()), "cli"),
            (Error::Env("x".into()), "env"),
            (Error::Network("x".into()), "network"),
            (
                Error::Engine(EngineFailure::new("no_results", "x")),
                "parse",
            ),
            (Error::Captcha("x".into()), "captcha"),
            (Error::Timeout("x".into()), "timeout"),
            (Error::NotImplemented("x".into()), "not_implemented"),
            (Error::Internal("x".into()), "internal"),
        ];
        for (err, expected) in cases {
            assert_eq!(err.code_str(), expected, "错误: {err:?}");
        }
    }
}
