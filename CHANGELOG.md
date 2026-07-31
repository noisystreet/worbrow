# Changelog

本文件记录用户可见变更（Keep a Changelog 风格）。

## [Unreleased]

### Added

- 项目骨架（design.md §5.2 目录结构）：domain/ports/app/cli/output 分层
- 自研 JSON-RPC 消息框架（`drivers/jsonrpc.rs`，CDP 与 Marionette 后端共用消息类型）
- DuckDuckGo 引擎适配器（html 端点，URL 直访 + scraper 解析）
- **Firefox（Marionette）后端 V1**：自研 DebuggerTransport 客户端（`<长度>:` 文本帧 +
  四元素数组消息 `[0,id,command,params]` / `[1,id,error,result]`）；随机端口 + 独立
  临时 profile 并发隔离；Drop 回收子进程；命令子集 NewSession/Navigate/ExecuteScript/
  GetPageSource/TakeScreenshot
- 浏览器二进制发现（`FIREFOX_PATH`/`CHROME_PATH` → PATH → 平台默认位置）
- `fake` 浏览器后端 + 离线 HTML fixture，CI 无需真实浏览器
- 真机冒烟测试（`tests/firefox_smoke.rs`，`#[ignore]`，data: URL 全链路，无外网依赖）
- CLI：`search "<query>"`（`--engine/--browser/--max-results/--timeout/--json/--log-level/--screenshot/--dump-html`）、
  `search list`、`search doctor`（真实检测浏览器二进制）
- 输出契约 schema v1（成功包 + 错误包、语义化退出码）
- 质量基线：cargo fmt/clippy(-D warnings)/test/deny/machete、CI workflow、pre-commit
