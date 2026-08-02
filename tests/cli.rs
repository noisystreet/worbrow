//! CLI 级测试：真实二进制进程的退出码、stdout JSON 契约与子命令（design.md §6.1/§7）。
//! 需要已构建的 binary（`cargo test` 会自动构建）。

use assert_cmd::Command;
use serde_json::Value;

/// 运行 `worbrow` 并返回 (exit_code, stdout_str)。
fn run(args: &[&str]) -> (i32, String) {
    let output = Command::cargo_bin("worbrow")
        .expect("binary 应存在")
        .args(args)
        .output()
        .expect("进程应运行成功");
    (
        output.status.code().expect("应捕获退出码"),
        String::from_utf8(output.stdout).expect("stdout 应为 UTF-8"),
    )
}

/// 解析 stdout 为 JSON。
fn parse_json(s: &str) -> Value {
    serde_json::from_str(s).expect("stdout 应为合法 JSON")
}

#[test]
fn list_subcommand_lists_engines() {
    let (code, out) = run(&["list"]);
    assert_eq!(code, 0);
    assert!(out.lines().any(|l| l == "duckduckgo"));
}

#[test]
fn doctor_subcommand_exits_zero() {
    let (code, out) = run(&["doctor"]);
    assert_eq!(code, 0);
    assert!(out.contains("engines:"));
    assert!(out.contains("chrome (CDP)"));
}

#[test]
fn missing_query_without_json_is_human_error() {
    let (code, out) = run(&[]);
    assert_eq!(code, 2);
    assert!(out.contains("[cli]"));
    assert!(!out.trim_start().starts_with('{'));
}

#[test]
fn missing_query_with_json_is_error_payload() {
    let (code, out) = run(&["--json"]);
    assert_eq!(code, 2);
    let json = parse_json(&out);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["error"]["code"], "cli");
}

#[test]
fn unknown_engine_is_cli_error() {
    // engine 参数校验在运行时（app::engines::resolve）：未知引擎 → 参数错误 exit 2
    let (code, out) = run(&["--engine", "google", "rust", "--json"]);
    assert_eq!(code, 2);
    let json = parse_json(&out);
    assert_eq!(json["error"]["code"], "cli");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown engine")
    );
}

/// `worbrow fetch`：非法 URL（file scheme）→ 参数错误 exit 2 + 统一失败包。
/// URL 校验在 app 层前置（不启动浏览器），CLI 无浏览器环境也稳定（CI 安全）。
#[test]
fn fetch_subcommand_invalid_url_is_cli_error() {
    let (code, out) = run(&["fetch", "file:///etc/passwd", "--json"]);
    assert_eq!(code, 2);
    let json = parse_json(&out);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["error"]["code"], "cli");
}

/// `worbrow fetch` 成功路径由 app 层 FakeDriver 测试覆盖（CLI 不暴露 `fake` 后端，
/// 与 search 一致）；此处仅断言 fetch 子命令的稳定错误面。
#[test]
fn fetch_subcommand_missing_url_is_cli_error() {
    let (code, out) = run(&["fetch", "--json"]);
    assert_eq!(code, 2);
    assert!(out.trim().is_empty(), "clap 缺参错误不写 stdout");
}

/// `worbrow fetch`：非法 extract 字段 → 参数错误 exit 2（clap value_enum 在解析期拒绝，
/// 错误走 stderr，stdout 为空——与既有 clap 参数错误一致）。
#[test]
fn fetch_subcommand_invalid_extract_is_cli_error() {
    let (code, out) = run(&[
        "fetch",
        "https://example.com",
        "--extract",
        "lynx",
        "--json",
    ]);
    assert_eq!(code, 2);
    assert!(out.trim().is_empty(), "clap 解析错误不写 stdout");
}
// 注：CDP（chrome）后端 V1 已实现——协议正确性由 `src/drivers/cdp.rs` 单测（mock WebSocket）
// 与 `tests/cdp_smoke.rs` 真机冒烟（#[ignore]）覆盖；`--browser chrome` 在有/无 Chrome 的
// 环境下退出码不同（真实搜索/未找到二进制），不做 CLI 级断言。
