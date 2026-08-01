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
    assert!(out.contains("引擎注册表"));
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
    let (code, out) = run(&["--engine", "google", "rust"]);
    assert_eq!(code, 2); // clap 参数解析失败
    assert!(out.is_empty() || out.contains("error")); // clap 错误走 stderr，stdout 为空
}
// 注：CDP（chrome）后端 V1 已实现——协议正确性由 `src/drivers/cdp.rs` 单测（mock WebSocket）
// 与 `tests/cdp_smoke.rs` 真机冒烟（#[ignore]）覆盖；`--browser chrome` 在有/无 Chrome 的
// 环境下退出码不同（真实搜索/未找到二进制），不做 CLI 级断言。
