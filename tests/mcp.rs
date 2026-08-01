//! MCP stdio server 集成测试（默认 feature 含 mcp）。
//!
//! 真实子进程 + 真实 stdio 管道：验证协议握手（initialize）、tools/list、
//! tools/call（browser=fake 路径，无外网/无浏览器依赖）。
//!
//! ```bash
//! cargo test --test mcp
//! ```

#![cfg(feature = "mcp")]

use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

/// 单个请求-响应交互的总预算（fake 路径瞬时，给进程启动/协议握手留足余量）。
const RPC_TIMEOUT: Duration = Duration::from_secs(20);

struct McpClient {
    child: Child,
    writer: tokio::process::ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
    next_id: u64,
}

impl McpClient {
    /// 默认（禁用空闲超时）。
    async fn spawn() -> McpClient {
        Self::spawn_with_idle(0).await
    }

    /// 以指定空闲超时（秒，0 = 禁用）启动 server。
    async fn spawn_with_idle(idle_secs: u64) -> McpClient {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_worbrow"));
        cmd.arg("mcp");
        if idle_secs > 0 {
            cmd.arg("--idle-timeout").arg(idle_secs.to_string());
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn worbrow mcp");
        let writer = child.stdin.take().expect("server stdin");
        let stdout = child.stdout.take().expect("server stdout");
        McpClient {
            child,
            writer,
            reader: BufReader::new(stdout),
            next_id: 1,
        }
    }

    /// 发送 JSON-RPC 请求并等待匹配 id 的响应（跳过通知/事件帧）。
    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&request).expect("序列化请求");
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .await
            .expect("写入请求");
        self.writer.flush().await.expect("flush 请求");

        let deadline = tokio::time::Instant::now() + RPC_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!remaining.is_zero(), "等待响应 {method} 超时（id={id}）");
            let mut line = String::new();
            let read = tokio::time::timeout(remaining, self.reader.read_line(&mut line))
                .await
                .expect("读取响应超时")
                .expect("读取响应 IO 失败");
            assert!(
                read > 0,
                "server 提前关闭 stdin（id={id}，method={method}）"
            );
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(trimmed).expect("响应 JSON 解析失败");
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return value;
            }
            // 非本 id 的帧（乱序响应）跳过，继续等待
        }
    }

    /// 完成 MCP 生命周期握手（initialize + notifications/initialized）。
    async fn initialize(&mut self) {
        let resp = self
            .call(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "mcp-test-client", "version": "0.0.0" }
                }),
            )
            .await;
        assert!(
            resp.get("result").is_some(),
            "initialize 应返回 result（实际: {resp}）"
        );
        // notifications/initialized：无响应，仅写入
        let mut line = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .expect("序列化通知");
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .await
            .expect("写入 initialized 通知");
        self.writer.flush().await.expect("flush 通知");
    }

    async fn kill(mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await;
        if self.child.id().is_some() {
            let _ = self.child.kill().await;
        }
    }
}

#[tokio::test]
async fn tools_list_exposes_web_search_tool() {
    let mut client = McpClient::spawn().await;
    client.initialize().await;

    let resp = client.call("tools/list", json!({})).await;
    let tools = resp["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list 应返回 tools 数组（实际: {resp}）"));
    assert!(
        tools.iter().any(|t| t["name"] == "web_search"),
        "tools 应包含 web_search 工具（实际: {tools:?}）"
    );
    // 输入 schema 应带 query 必填字段
    let web_search = tools
        .iter()
        .find(|t| t["name"] == "web_search")
        .expect("web_search 工具存在");
    let required = web_search["inputSchema"]["required"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(required.contains(&Value::String("query".into())));
    client.kill().await;
}

#[tokio::test]
async fn tools_call_runs_web_search_via_fake_driver() {
    let mut client = McpClient::spawn().await;
    client.initialize().await;

    let resp = client
        .call(
            "tools/call",
            json!({
                "name": "web_search",
                "arguments": {
                    "query": "rust async",
                    "browser": "fake",
                    "max_results": 5,
                    "timeout": 10
                }
            }),
        )
        .await;

    // 协议层成功：无 error 字段，且 content 为文本
    assert!(
        resp.get("error").is_none(),
        "tools/call 不应有协议错误（实际: {resp}）"
    );
    let content = resp["result"]["content"]
        .as_array()
        .unwrap_or_else(|| panic!("result 应含 content（实际: {resp}）"));
    let text = content
        .iter()
        .find(|c| c["type"] == "text")
        .and_then(|c| c["text"].as_str())
        .unwrap_or_else(|| panic!("content 应含 text（实际: {content:?}）"));

    // 输出契约：成功包 JSON（schema v1）
    let payload: Value = serde_json::from_str(text).expect("工具输出应是 JSON");
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["query"], "rust async");
    assert_eq!(payload["meta"]["engine"], "bing"); // 默认引擎 = bing
    // fake 后端返回模拟结果页（SMOKE_HTML = bing 结构，3 条 ≥ 低产量阈值）
    assert_eq!(payload["results"].as_array().map(Vec::len), Some(3));
    // 结果条目携带 domain/https（agent 免解析判断来源）
    assert_eq!(payload["results"][0]["domain"], "www.runoob.com");
    assert_eq!(payload["results"][0]["https"], true);
    // 结果契约增强：日期/广告/解跳转标记（SMOKE_HTML 首条摘要无日期、直接 URL）
    assert_eq!(payload["results"][0]["published_at"], Value::Null);
    assert_eq!(payload["results"][0]["is_ad"], false);
    assert_eq!(payload["results"][0]["url_resolved"], false);
    assert_eq!(payload["meta"]["low_yield"], false);
    client.kill().await;
}

#[tokio::test]
async fn tools_call_rejects_unknown_browser() {
    let mut client = McpClient::spawn().await;
    client.initialize().await;

    let resp = client
        .call(
            "tools/call",
            json!({
                "name": "web_search",
                "arguments": { "query": "x", "browser": "lynx" }
            }),
        )
        .await;

    // 参数错误 → CallToolResult.isError=true（用户可见 error content，而非协议错误）
    assert!(
        resp.get("error").is_none(),
        "不支持的浏览器是工具级错误（实际: {resp}）"
    );
    assert_eq!(
        resp["result"]["isError"], true,
        "应标记 isError（实际: {resp}）"
    );
    client.kill().await;
}

#[tokio::test]
async fn tools_call_unknown_tool_is_protocol_error() {
    let mut client = McpClient::spawn().await;
    client.initialize().await;

    let resp = client
        .call("tools/call", json!({ "name": "nope", "arguments": {} }))
        .await;

    // 未知工具 → JSON-RPC 协议错误；rmcp v2.2.0 用 invalid_params(-32602) 表示 tool not found
    assert!(
        resp.get("error").is_some(),
        "未知工具应返回协议错误（实际: {resp}）"
    );
    assert_eq!(resp["error"]["code"], -32602);
    client.kill().await;
}

/// 空闲超时：无任何请求时 server 应在超时后自动退出（exit 0），不留残留进程。
/// 关键：stdin 保持打开（writer 移出后不 drop），验证触发因素是"空闲"而非"EOF"。
#[tokio::test]
async fn idle_timeout_exits_when_no_requests() {
    let client = McpClient::spawn_with_idle(1).await;
    // 移出 child，writer/reader 保持打开直到 server 退出
    let mut child = client.child;
    let status = tokio::time::timeout(Duration::from_secs(8), child.wait())
        .await
        .expect("空闲超时后 server 应自动退出")
        .expect("wait 不应失败");
    assert_eq!(status.code(), Some(0), "空闲超时应正常退出（exit 0）");
}

/// 空闲超时不误杀活跃连接：请求活动应重置空闲计时。
#[tokio::test]
async fn idle_timeout_resets_on_requests() {
    let mut client = McpClient::spawn_with_idle(2).await;
    client.initialize().await;
    // 3 次 tools/list，间隔 1.2s：累计 3.6s > 2s 空闲窗口，但每次调用都有活动
    for _ in 0..3 {
        client.call("tools/list", json!({})).await;
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    assert!(
        client.child.try_wait().unwrap().is_none(),
        "有请求活动时 server 不应空闲退出"
    );
    client.kill().await;
}
