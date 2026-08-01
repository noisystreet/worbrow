# worbrow 功能路线规划

> 读者：项目维护者 / 贡献者。状态：**规划草案**，随实施更新；落地决策按 AGENTS.md 记 ADR。
> 本文只描述「往哪走」，架构权威见 [design.md](design.md)，协议决策见 [ADR 目录](adr/)。

## 1. 背景与现状

worbrow 是驱动本机 headless 浏览器执行搜索引擎搜索的 agent CLI，输出稳定 JSON 契约。

当前已稳定：

- **引擎**：duckduckgo、bing（默认）；解析失败走 `EngineFailure`（exit 4）
- **浏览器后端**：Firefox（Marionette，自研协议，含超时/版本校验/并发隔离）与 Chrome/Edge（CDP，自研协议）均已实现；`fake` 供 CI 冒烟
- **MCP**：`web_search` 工具 + 空闲超时（覆盖握手前/后）
- **契约**：schema v1、退出码语义冻结、stdout 仅 JSON

规划聚焦两个短板：**浏览器覆盖**（CDP 缺口）与**搜索体验**（单页、无地域/语言控制、每次搜索重新起浏览器）。

## 2. 目标与非目标

### 目标

1. Chrome/Edge 后端可用（`--browser chrome` / MCP `browser=chrome`）
2. 多次搜索复用长驻浏览器会话，显著降低单次搜索开销（当前每次 spawn 约 2-5s）
3. 搜索结果更可控：语言、地域、翻页聚合、精确条数

### 非目标（明确不做）

- **Google 引擎**：对 headless 反爬强、稳定性差，收益不匹配成本
- **绕过验证码/反爬**：遵守目标站 ToS，只检测上报（现有 `captcha_heuristics`）
- **图片/视频等垂直内容搜索**：与「通用搜索 JSON 契约」定位不符
- **分布式/多实例**：单机 CLI 工具

## 3. 方向与优先级

### P0：CDP 后端（Chrome/Edge）—— ✅ 已完成（2026-08）

> 实施见提交 `feat: Chrome/Edge（CDP）后端`：`drivers/cdp.rs` WebSocket 实现、
> mock 单测 + `tests/cdp_smoke.rs` 真机冒烟。

| 项 | 内容 |
|---|---|
| 现状 | [cdp.rs](../src/drivers/cdp.rs) 全部 `NotImplemented`（[ADR-002](adr/0002-browser-driver-protocols.md) 规划 V1/V2） |
| 目标 | `browser=chrome/edge/chromium` 走真实搜索 |
| 改动点 | ① [cdp.rs](../src/drivers/cdp.rs)：基于 [jsonrpc.rs](../src/drivers/jsonrpc.rs) + `tokio-tungstenite`（依赖已预留）实现 WebSocket 传输；② 命令子集 `Target.attachToTarget` / `Page.navigate` / `Runtime.evaluate`（取 HTML、`document.readyState`、验证码判定）/ `Page.captureScreenshot`；③ 生命周期：`chrome --headless=new --remote-debugging-port=<动态端口>`，`GET /json/version` 取 `webSocketDebuggerUrl`（复用 [marionette.rs](../src/drivers/marionette.rs) 的随机端口/profile/版本校验模式）；④ [drivers/mod.rs](../src/drivers/mod.rs) `resolve` 接 `BrowserKind::Chrome`，`doctor` 输出同步 |
| 验证 | jsonrpc 单测；data: URL 真机冒烟（参照 [firefox_smoke.rs](../tests/firefox_smoke.rs)）；CI 外真搜索 |
| 风险 | `attachToTarget` 的 sessionId 透传（消息需带 sessionId）；Chrome 版本差异（≥109） |
| 参考 | [ADR-002](adr/0002-browser-driver-protocols.md)；Chrome DevTools Protocol 官方文档 |

### P1：浏览器会话复用 / 池化

| 项 | 内容 |
|---|---|
| 现状 | 每次搜索 spawn 新 Firefox（启动 + NewSession 约 2-5s），MCP 高频调用开销显著 |
| 目标 | 长驻会话复用；空闲回收防残留 |
| 改动点 | ① drivers 层抽出 `Session` 抽象（连接/命令/超时/健康检查），与 `BrowserDriver` 解耦；② app 层持会话池（LRU + 上限 + 空闲 TTL 回收，可复用 [mcp.rs](../src/mcp.rs) 的原子化空闲计时经验）；③ 并发上限与排队；搜索间状态隔离（navigate 前重置） |
| 验证 | 集成测试（fake 时序模拟复用/回收）；真机冒烟扩展「连续多次搜索复用同一进程」 |
| 风险 | 浏览器进程泄漏、并发 profile 冲突、崩溃会话复活（需健康检查/重连） |
| 参考 | [marionette.rs](../src/drivers/marionette.rs) 的 Drop 回收；[mcp.rs](../src/mcp.rs) 空闲超时 |

### P1：搜索参数增强

| 项 | 内容 |
|---|---|
| 现状 | `SearchQuery` 仅 `query`；Bing 单页、无地域/语言控制 |
| 目标 | `hl`（语言）、`mkt`/`cc`（地域）、翻页聚合（前 N 页去重合并）、`max_results` 精确截断 |
| 改动点 | ① [domain.rs](../src/domain.rs) `SearchQuery` 扩展可选字段（默认保持现行为）；② [bing.rs](../src/engines/bing.rs) 先行：URL 模板 + 翻页参数，[duckduckgo.rs](../src/engines/duckduckgo.rs) 保持一致；③ [app.rs](../src/app.rs) 翻页循环 + 结果去重重排 rank；④ CLI/MCP 新增 `--lang/--region/--pages` |
| 契约影响 | `meta` 可能新增字段（schema v1 **只增不改**，允许）；不做破坏性变更 |
| 验证 | URL 模板单测 + fixture 扩展 |

### P2：新引擎（baidu）

复用 `SearchProvider` 模板 + fixture（见 [CONTRIBUTING.md](../CONTRIBUTING.md) 常见任务）。前置评估：反爬强度、headless 可达性；失败走 `EngineFailure`（exit 4）。若收益不足可推迟或不做。

## 4. 实施顺序

CDP 后端 → 会话复用 → 搜索参数增强 →（P2 baidu 视评估）

- 每步独立可验证、可回退；完成后同步 `doctor`、README、CHANGELOG
- 不承诺时间；重要取舍（如 CDP 传输层、会话池默认策略）以 ADR 记录

## 5. 契约与约束（贯穿全程）

- `schema_version` 只增不改；破坏契约需 bump 主版本并记 ADR（[AGENTS.md](../AGENTS.md) 硬约束 3）
- 退出码语义冻结（0/2/3/4/124/1）
- 安全红线：不自动访问搜索结果中的第三方 URL（只输出）、不绕过验证码
- 新增引擎 = 新文件 + `engines/mod.rs` 注册一行（硬约束 4）

## 6. 开放决策

1. **CDP 传输层**：沿用已预留的 `tokio-tungstenite`（与 ADR-002 一致），还是评估更轻替代 → 倾向沿用
2. **会话池默认策略**：CLI 单次搜索（用完即回收）与 MCP 长驻（TTL 空闲回收）是否分策略 → 倾向 MCP 场景默认启用
