# Changelog

本文件记录用户可见变更（Keep a Changelog 风格）。

## [Unreleased]

## [0.2.0] - 2026-08-01

### Added

- **正文抓取与结构化提取（P1，ADR-009）**：新增 `worbrow fetch <url>` 子命令与 MCP
  `fetch_page` 工具——抓取 **agent 显式传入** 的 URL（scheme 白名单 http/https，缺
  scheme 自动补 `https://`，非法 URL → exit 2），复用浏览器会话返回清洗后正文
  （`max-chars` 截断默认 20000，`meta.truncated`/`final_url` 标记）与可选结构化字段
  （`extract` allowlist：title/author/published_at/price/currency/rating/rating_max/
  reviews_count，JSON-LD → meta → DOM 提取，缺失缺省不编造，值保留原生类型）；
  `--no-text` 只要字段省 token；新 sibling fetch 成功包（schema v1 同版本），search
  成功/失败包零变化，新工具对既有客户端无感
- **质量信号扩展（P2）**：降级判定叠加 **web 占比阈值**（结果集中内容型占比 < 50%
  视为同质化低质，多义混入自动尝试下一引擎）；翻页聚合新增**同域名去重**（同一
  域名最多保留 2 条，rank 靠前优先，防单一来源刷屏；`site:` 过滤时用户意图为同域，
  豁免去重）；schema v1 无字段变化（内部信号）
- **结果质量信号与降级链增强（P1）**：`SearchResult` 新增 `result_kind`
  （web/dictionary/translation，URL 路径/主机特征识别，识别失败回退 `web`）；
  引擎降级判定升级为按**内容型结果数**（≥3 或集满 max_results），Bing 对
  `best`/`learn` 等查询返回全词典释义的高产低质结果不再误判满意，自动尝试
  下一引擎；`meta.low_yield` 语义扩展为"内容型结果不足"（字段与退出码不变，
  schema v1 只增不改）；agent 可自行过滤词典/翻译噪声
- **MCP 体验完善（P1）**：`web_search` 新增 `compact` 精简模式（结果仅 rank/title/url，
  省 agent 上下文 token，meta 完整；缓存命中路径同样生效）；新增 `list_engines` 工具
  （列出可用引擎）与 `doctor` 工具（环境自检：浏览器二进制/版本/引擎注册表）——
  输出 schema v1 不变，新工具对既有客户端无感
- **网络重试与结果缓存（P1，ADR-008）**：`--retry <n>`（CLI）/`retry`（MCP，封顶 5）
  对瞬时网络错误指数退避重试（2^(n-1) 秒封顶 8s，计入全局超时预算；验证码/参数错/
  超时不重试）；MCP 长驻进程内相同请求参数 60s TTL 缓存命中直接返回（`meta.cached`），
  `no_cache` 参数绕过；`meta.cached`/`meta.retries` 新增（schema v1 只增不改）；
  CLI 无状态不缓存
- **MCP 会话池化（P1，ADR-007）**：MCP 长驻进程内复用浏览器会话，消除每次搜索
  spawn 2-5s 开销；`worbrow mcp --max-sessions <n> --session-ttl <sec>`（默认
  1/60s）；空闲会话 TTL 回收、并发上限排队、崩溃会话错误驱动重建；CLI 单次行为
  不变；schema v1 / 退出码不变（对 agent 透明）
- **结果契约增强（P1）**：`SearchResult` 新增 `published_at`（发布日期，摘要尽力提取）、
  `is_ad`（广告标记，DDG 广告位标记、Bing 选择器天然排除）、`url_resolved`（是否已解
  跳转，uddg/ck-a 展开标记，agent 判断 `url` 可信度）；schema v1 只增不改
- **搜索参数补全（P1）**：`SearchQuery`/`Config` 新增 `freshness`（时间过滤，Bing `qft`
  尽力尝试/DDG `df`）、`safesearch`（安全搜索，Bing `adlt`/DDG `kp`）、`site` 与
  `filetype`（query 级 `site:`/`filetype:` 语法，零引擎适配）；CLI
  `--freshness/--safesearch/--site/--filetype` 与 MCP 参数同步（非法枚举值 →
  工具级错误）；输出 `query` 字段保留原始 text（过滤条件由请求参数表达）；schema v1
  无变化（Bing `qft` 实网验证不过滤，详见 roadmap-search-params.md §3 风险）

### Fixed

- **浏览器进程泄漏**：spawn 取消路径丢失 `tokio::process::Child`（`ChildGuard` 兜底
  kill）+ 会话池 reaper 自持强引用导致池永不 Drop（改 Weak 弱引用）
- **Marionette eval 表达式语义**：`ExecuteScript` 需显式 `return` 才返回值，裸表达式
  （`document.readyState`/`location.href`）在 Firefox 下返回 undefined，导致 fetch
  的 `wait_load` 空转满预算、`meta.final_url` 为 null；包装为 `return (expr);` 对齐
  CDP `Runtime.evaluate` 语义

## [0.1.1] - 2026-08-01

### Added

- **搜索参数增强（P1）**：`SearchQuery`/`Config` 新增 `lang`/`region`/`pages`；
  CLI `--lang/--region/--pages` 与 MCP 参数同步；Bing `setlang/mkt/first`、DDG
  `kl/s` URL 模板；app 翻页聚合（按 URL 去重合并、rank 重排、集满 `max_results`
  提前停止）；`meta.pages` 记录实际聚合页数（schema v1 只增不改）
- **agent 契约增强（P1）**：`SearchResult` 新增 `domain`（URL host）与 `https`
  （scheme 判定），agent 免解析 URL 即可判断来源可信度（schema v1 只增不改）；
  README 新增「Agent 集成」章节（Claude Code / Cursor MCP 配置 + CLI 子进程要点）
- **Bing ck/a 跳转链展开**：`www.bing.com/ck/a` 点击追踪链的 `u` 参数（base64url
  编码）解码为真实目标 URL；解码失败（无 `u`/非法 base64/非 http(s)）保持原样，
  模型侧拿到干净 URL 无需跟跳转
- **引擎可配且可降级（P1）**：`--engine bing,duckduckgo`（逗号分隔 = 尝试顺序）；
  验证码阻止/解析失败/低产时自动尝试下一引擎，全低产用最高产候选兜底；
  `meta.engine_tried` 记录尝试链（schema v1 只增不改）；最终失败错误码保持稳定
  （`captcha`/`parse`，exit 4）
- **进程回收强化**：driver 移入超时闭包，超时/取消/Drop 三路径统一回收浏览器
  子进程；CDP kill 后 `wait()` 收割防 zombie（MCP 长驻不积累残留进程）

## [0.1.0] - 2026-08-01

### Added

- **库 API 体验完善（P2，ADR-006）**：`Config` 字段私有化（builder 唯一入口，不可绕过
  clamp 不变量）；`run_sync` 移除、同步入口统一为 `search`；trait（`SearchProvider`/
  `BrowserDriver`）与契约包类型（`SuccessPayload`/`ErrorPayload`/`SCHEMA_VERSION`）顶层
  re-export；lib.rs quickstart doc-test + `examples/` 可运行示例；`EngineError` 与
  `EngineFailure` rustdoc 互链说明语义；README"作为库使用"（`default-features = false`
  去 MCP 依赖）；CONTRIBUTING 明确 0.x 公开面冻结与变更流程
- **库 API 公开面收敛为类型级顶层 API（ADR-006）**：`BrowserKind` 上移为顶层类型
  （`worbrow::BrowserKind`）；顶层 re-export `Config`/`Outcome`/`DoctorReport`/`Error`/
  `search`/`DEFAULT_*`；`Config::with_provider` 支持注入自定义引擎（外部无需复制
  `run` 编排）；CLI 参数解析（clap）移入二进制，lib 不再暴露 `cli` 模块；适配器
  实现（cdp/marionette/fake/bing 等）内部化，公开面仅 `resolve`/`AVAILABLE`；
  `Error` 底层错误经 `#[source]` 可下钻
- **库 API 完善**：`app::search`（同步搜索入口，内部管理 tokio runtime）、
  `app::DoctorReport`/`BackendStatus`（结构化环境自检）、`Config::new` + builder
  （`with_max_results`/`with_timeout`/`with_screenshot`/`with_dump_html`/`with_driver`）、
  `BrowserKind::from_arg`（浏览器参数单一解析源）、`domain::DEFAULT_*` 默认值常量
  （CLI/MCP 单源）、失败包 `detail` 填充引擎错误码；CLI 改为薄封装
  （参数解析 + 渲染，业务逻辑下沉库层）

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
