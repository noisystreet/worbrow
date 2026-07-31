# ADR-005：MCP stdio server 支持（`search mcp`）

- 状态：已接受
- 日期：2026-07-31

## 背景

主流 AI agent 运行时（Claude Code、Cursor、通用 MCP 客户端等）通过 **MCP
（Model Context Protocol）** 接入外部工具。stdio 传输是 MCP 客户端默认且覆盖面最广
的接入方式（免端口、免认证、进程生命周期由客户端管理）。此前本项目只有单任务 CLI，
agent 只能以子进程方式调用；接入 MCP 可将 `search` 作为一等工具暴露，复用同一内核。

## 决策

- 新增 `search mcp` 子命令：以 **MCP stdio server** 形态运行，通过 stdio 暴露
  `search` 工具（参数：`query`/`engine`/`browser`/`max_results`/`timeout`）
- SDK 采用 **rmcp 2.2.0**（官方 Rust SDK）。钉 2.x：v3 需要 rustc 1.88，超过本项目
  MSRV 1.85；2.2 提供 `#[tool]`/`#[tool_router]` 宏、`ServerHandler` trait 与
  `ServiceExt::serve` + `transport::stdio`，满足全部需求
- 依赖 **feature-gating**：`mcp = ["dep:rmcp"]`，默认不启用，普通构建零额外依赖
- 工具实现复用 `app::run`（design.md §6.2），成功/失败包 JSON 作为
  `CallToolResult` 的 text content 返回，**不打印 stdout**（stdout 是 MCP
  JSON-RPC 通道，日志仍走 stderr）
- 失败语义遵循 rmcp 约定：参数校验/搜索失败 → `Ok(CallToolResult::error(...))`
  （`isError=true`，内容用户可见）；未知工具 → JSON-RPC 协议错误（rmcp 2.2 为
  invalid_params，tool not found）
- `browser=fake` 供冒烟：`drivers::resolve(Fake)` 返回模拟结果页（SMOKE_HTML），
  无需真实浏览器即可验证全链路

## 后果

- **得到**：MCP 客户端零配置接入；工具与 CLI 共享同一内核与输出契约（schema v1），
  行为一致；stdio 传输无端口/认证负担
- **付出**：`search mcp` 需 `--features mcp` 编译（CI 与发布脚本同步）；rmcp
  依赖树仅在 mcp feature 下引入；MCP 模式的错误不再有退出码语义（由 MCP 客户端
  呈现 `isError`，契约 JSON 中的 `error.code` 仍可机器区分）
- **拒绝**：无 SDK 手写 JSON-RPC（协议栈非核心价值，rmcp 为官方 SDK 且成熟）；
  HTTP/SSE 传输（stdio 覆盖默认场景，HTTP 常驻服务留待 §13 V3 按需求引入）；
  CLI 子进程包装（每次调用冷启动、无会话复用，收益低）

## 验证

`cargo test --features mcp --test mcp`：真实子进程 + stdio 管道，覆盖
initialize 握手、tools/list、tools/call（fake 路径成功包）、未知浏览器（isError）、
未知工具（协议错误）。
