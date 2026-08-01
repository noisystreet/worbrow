# Changelog

本文件记录用户可见变更（Keep a Changelog 风格）。

## [Unreleased]

### Changed

- **MSRV 提升至 1.97**（原 1.85）：依赖链真实要求 rustc ≥ 1.88（darling/ICU），
  1.85 无法编译；rust-version 与 CI MSRV 校验同步更新
- **MCP server 支持空闲超时**：`worbrow mcp --idle-timeout <secs>` 超过该时长无任何
  请求自动退出（默认 0 = 禁用），覆盖握手前与握手后阶段，防 agent 崩溃后残留进程
- **默认搜索引擎改为 `bing`**（CLI 与 MCP 工具一致，原 `duckduckgo`）
- **默认硬超时改为 60s**（原 20s）
- **默认启用 `mcp` feature**：普通 `cargo build` / `make build` 即含 `worbrow mcp`；
  可用 `--no-default-features` 精简构建
- **项目更名为 `worbrow`**：Cargo 包/库/二进制统一为 `worbrow`（原 `rplay-search` / `search`）；
  MCP 工具名为 `web_search`
- **MCP 工具更名为 `web_search`**（原 `search`）

### Added

- **Chrome/Edge（CDP）后端 V1**：自研 WebSocket 客户端（tokio-tungstenite + 复用
  `drivers::jsonrpc` 消息类型），`--browser chrome` / MCP `browser=chrome` 真实搜索；
  `--remote-debugging-port=0` 随机端口 + stderr 日志发现（消除端口竞态）；命令子集
  `Target.createTarget/attachToTarget`、`Page.navigate`、`Runtime.evaluate`、
  `Page.captureScreenshot`；含 mock WebSocket 单测 + 真机冒烟（`tests/cdp_smoke.rs`）
- **Bing 搜索引擎**：`worbrow --engine bing` 支持，复用 Bing 的 HTML 搜索结果页面
  （`www.bing.com/search?q=`），解析器覆盖 `li.b_algo`/`h2 a`/`.b_caption` 结构；
  含 6 个单测 + 独立 fixture
- **Debian 打包（cargo-deb）**：`make deb` 生成 `target/debian/worbrow_*.deb`
  （发布形态启用 mcp feature）；`Recommends: firefox | firefox-esr` 运行时弱依赖；
  CI 新增 deb 构建与内容校验 job
- **MCP stdio server（`worbrow mcp`）**：rmcp 2.2 官方
  SDK，stdio 传输；`web_search` 工具（query/engine/browser/max_results/timeout）复用
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
- CLI：`worbrow "<query>"`（`--engine/--browser/--max-results/--timeout/--json/--log-level/--screenshot/--dump-html`）、
  `worbrow list`、`worbrow doctor`（真实检测浏览器二进制）
- 输出契约 schema v1（成功包 + 错误包、语义化退出码）
- 质量基线：cargo fmt/clippy(-D warnings)/test/deny/machete、CI workflow、pre-commit
