# Changelog

本文件记录用户可见变更（Keep a Changelog 风格）。

## [Unreleased]

### Added

- **Debian 打包（cargo-deb）**：`make deb` 生成 `target/debian/rplay-search_*.deb`
  （发布形态启用 mcp feature）；`Recommends: firefox | firefox-esr` 运行时弱依赖；
  CI 新增 deb 构建与内容校验 job
- **MCP stdio server（`search mcp`，需 `--features mcp` 编译）**：rmcp 2.2 官方
  SDK，stdio 传输；`search` 工具（query/engine/browser/max_results/timeout）复用
  `app::run`，成功/失败包 JSON 经 `tools/call` 返回；`browser=fake` 冒烟免浏览器；
  集成测试覆盖握手/tools/list/tools/call（见 docs/adr/0005-mcp-stdio-server.md）
- 项目骨架（design.md §5.2 目录结构）：domain/ports/app/cli/output 分层
- 自研 JSON-RPC 消息框架（`drivers/jsonrpc.rs`，CDP 与 Marionette 后端共用消息类型）
- DuckDuckGo 引擎适配器（html 端点，URL 直访 + scraper 解析）
- **Firefox（Marionette）后端 V1**：自研 DebuggerTransport 客户端（`<长度>:` 文本帧 +
  四元素数组消息 `[0,id,command,params]` / `[1,id,error,result]`）；随机端口 + 独立
  临时 profile 并发隔离；Drop 回收子进程；命令子集 NewSession/Navigate/ExecuteScript/
  GetPageSource/TakeScreenshot
- Firefox 后端可靠性完善：NewSession/单命令级超时（不挂死）；`WebDriver:SetTimeouts`
  收紧 pageLoad（30s）/script（10s）；spawn 时校验 Firefox ≥ 55；命令耗时 tracing 日志
- 浏览器二进制发现（`FIREFOX_PATH`/`CHROME_PATH` → PATH → 平台默认位置）+ 版本解析
- `fake` 浏览器后端 + 离线 HTML fixture，CI 无需真实浏览器
- 真机冒烟测试（`tests/firefox_smoke.rs`，`#[ignore]`，data: URL 全链路、并发端口隔离、
  无效 URL 导航不挂死，均无外网依赖）
- app 层：wait_for 二级超时（≤10s 预算）+ navigate/wait_for/html 步骤耗时日志
- CLI：`search "<query>"`（`--engine/--browser/--max-results/--timeout/--json/--log-level/--screenshot/--dump-html`）、
  `search list`、`search doctor`（真实检测浏览器二进制）
- 输出契约 schema v1（成功包 + 错误包、语义化退出码）
- 质量基线：cargo fmt/clippy(-D warnings)/test/deny/machete、CI workflow、pre-commit
