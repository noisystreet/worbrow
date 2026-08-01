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

### P1：浏览器会话复用 / 池化 —— ✅ 已完成（2026-08，[ADR-007](adr/0007-mcp-session-pool.md)）

| 项 | 内容 |
|---|---|
| 现状 | 每次搜索 spawn 新 Firefox（启动 + NewSession 约 2-5s），MCP 高频调用开销显著 |
| 目标 | 长驻会话复用；空闲回收防残留；并发上限与排队；崩溃会话复活 |
| 改动点 | ① [drivers/pool.rs](../src/drivers/pool.rs) `SessionPool`（空闲 LIFO + Semaphore 限并发 + TTL reaper 后台回收）；② [app.rs](../src/app.rs) 拆出 `run_with`（`run` 与 MCP 共用编排）；③ MCP `SearchServer` 持池，`mcp` 子命令新增 `--max-sessions`/`--session-ttl`（默认 1/60s）；④ 健康判定 = 命令错误驱动（Network/Timeout → 重建）；⑤ 搜索间状态隔离（navigate 覆盖，无需额外清理） |
| 验证 | 池单测（复用/TTL/健康丢弃/排队）+ app 集成（run_with 与 run 等价）+ MCP 集成（fake 回归）+ 真机冒烟（`pool_reuses_same_firefox_process` 复用同一进程 pid） |
| 风险 | 浏览器进程泄漏（TTL + Drop 双保险）、崩溃会话复活（错误驱动健康判定 → 重建）、并发 profile 冲突（池内单一类型 + Semaphore 排队） |
| 参考 | [roadmap-session-pool.md](roadmap-session-pool.md)（专项设计）；[ADR-007](adr/0007-mcp-session-pool.md) |

### P1：搜索参数增强 —— ✅ 已完成（2026-08）

| 项 | 内容 |
|---|---|
| 现状 | `SearchQuery` 仅 `query`；Bing 单页、无地域/语言控制 |
| 目标 | `hl`（语言）、`mkt`/`cc`（地域）、翻页聚合（前 N 页去重合并）、`max_results` 精确截断 |
| 改动点 | ① [domain.rs](../src/domain.rs) `SearchQuery` 扩展 `lang`/`region`/`pages`（默认保持现行为）；② [bing.rs](../src/engines/bing.rs)：`setlang`/`mkt`/`first` 翻页，[duckduckgo.rs](../src/engines/duckduckgo.rs)：`kl`/`s` 翻页；③ [app.rs](../src/app.rs) 翻页聚合循环（按 URL 去重合并、rank 重排、集满 `max_results` 提前停止）；④ CLI/MCP 新增 `--lang/--region/--pages` |
| 契约影响 | `meta.pages` 新增字段（schema v1 **只增不改**，允许）；不做破坏性变更 |
| 验证 | URL 模板单测（bing/ddg）+ 翻页聚合集成测试（去重/重排/提前停止）+ lib_api 外部视角 |

### P1：agent 契约增强（结果字段 domain/https）

| 项 | 内容 |
|---|---|
| 现状 | `SearchResult` 仅 rank/title/url/snippet；agent 需自行解析 URL 判断来源可信度 |
| 目标 | 结果条目直接携带 `domain`（URL host）与 `https`（scheme 判定），供 agent 免解析判断来源 |
| 改动点 | ① [domain.rs](../src/domain.rs) `SearchResult` 新增 `domain`/`https`（构造时从 url 提取）；② [extract.rs](../src/extract.rs) 新增 `url_origin` 辅助；③ [bing.rs](../src/engines/bing.rs)/[duckduckgo.rs](../src/engines/duckduckgo.rs) 填充 |
| 契约影响 | 结果对象新增字段（schema v1 **只增不改**，允许）；0.x 破坏性面：struct literal 构造点需同步（记录于 CONTRIBUTING） |
| 验证 | 引擎 fixture 单测断言 domain/https；集成/输出测试同步 |

> 不做：`published_date`（引擎 HTML 日期无稳定选择器、中英文格式不统一，解析脆弱）；
> `meta.cached/retries`（依赖缓存/重试功能，落地时再增，schema 只增不改允许）。

### P1：引擎可配且可降级（fallback 链）

| 项 | 内容 |
|---|---|
| 现状 | `--engine` 单引擎，验证码/解析失败/低产即终止（exit 4 或 low_yield 标志），agent 需自行换引擎重试 |
| 目标 | `--engine bing,duckduckgo`（逗号分隔 = 尝试顺序）；验证码阻止/解析失败/低产时自动尝试下一引擎；`meta.engine_tried` 记录尝试链（schema v1 只增不改） |
| 改动点 | ① [app.rs](../src/app.rs)：单引擎流程抽 `search_one`，外层降级循环（captcha 无结果/`EngineFailure` → 换下一个；低产保留候选继续，全低产用最高产候选）；② [cli.rs](../src/cli.rs)/[mcp.rs](../src/mcp.rs)：`engine` 支持逗号分隔；③ [domain.rs](../src/domain.rs) `SearchMeta` 新增 `engine_tried` |
| 契约影响 | `meta.engine_tried` 新增字段（只增不改）；错误码保持稳定（`captcha`/`parse`，exit 4 冻结）；`low_yield` 保持成功包标志语义（结果可用即不降级为失败） |
| 验证 | 集成测试：ddg 解析失败→降级 bing 成功（engine_tried 断言）、首引擎成功不降级、全失败返回稳定错误码、低产候选兜底 |
| 风险 | 降级放大总耗时（全局 timeout 兜底）；同页面多引擎解析差异（fixture 按引擎分开） |

### P1：结果质量信号与降级链增强（`result_kind` + 质量降级）—— ✅ 已完成（2026-08，[roadmap-result-quality.md](roadmap-result-quality.md)）

| 项 | 内容 |
|---|---|
| 现状 | 降级判定只看数量（`results.len() >= LOW_YIELD_THRESHOLD`）；Bing 对含常见英文词查询（如 `best`/`learn`）返回"词典释义"结果时，10 条高产低质不触发降级（真实案例见专项文档） |
| 目标 | 引擎自检结果质量：URL 特征标记 `result_kind`（web/dictionary/translation）；降级判定按"**内容型**结果数"≥ 阈值，低质自动尝试下一引擎，不依赖 agent 输入（对比 `site:` 需事先知道答案站点，不通用） |
| 改动点 | ① [extract.rs](../src/extract.rs) 新增 `result_kind(url)` 类型识别（URL 路径/主机特征，跨引擎共享，识别失败回退 `web`）；② [domain.rs](../src/domain.rs) `SearchResult` 新增 `result_kind`；③ [app.rs](../src/app.rs) 降级判定改用内容型结果数（`satisfied` 条件升级，候选兜底按内容型择优） |
| 契约影响 | 结果对象新增 `result_kind` 字段（schema v1 **只增不改**，允许）；`low_yield` 语义扩展（数量低 → 内容型结果不足），字段与错误码不变 |
| 验证 | 特征库单测（真实污染 URL 样本：iciba/剑桥/eudic/fanyi）+ 集成测试（全词典结果触发降级 engine_tried、首引擎内容型不降级回归） |
| 风险 | 特征误判（如 wordpress 路径含 word）→ 路径段精确匹配 + host 前缀匹配 + 回退 `web` 兜底；误判代价仅"多试一个引擎" |
| 参考 | 专项规划 [roadmap-result-quality.md](roadmap-result-quality.md)（真实案例、目标/非目标、开放决策） |

### P1：网络重试与结果缓存（`--retry` / TTL 缓存）—— ✅ 已完成（2026-08，[ADR-008](adr/0008-retry-and-cache.md)）

| 项 | 内容 |
|---|---|
| 现状 | 瞬时网络/引擎错误直接失败（exit 4）；MCP 长驻进程内相同 query 重复搜索每次都重跑 |
| 目标 | `--retry <n>` 瞬时错误退避重试；MCP 进程内相同 query 短 TTL 缓存（去重） |
| 改动点 | ① [app.rs](../src/app.rs) 提取 `search_attempt`/`search_engine_chain`/`handle_engine_result`，仅 `Error::Network` 指数退避重试（2^(n-1) 秒封顶 8s，计入全局 timeout）；② [mcp.rs](../src/mcp.rs) `SearchCache`（LRU + TTL 60s + 容量 128 + `no_cache` 逃生阀）；③ `SearchMeta` 新增 `cached`/`retries`（schema v1 只增不改）；④ CLI `--retry`、MCP `retry`（封顶 5） |
| 契约影响 | `meta.cached`/`meta.retries` 新增字段（只增不改）；`--retry`/`retry`/`no_cache` 为请求参数（CLI/MCP schema 只增） |
| 验证 | app 单测（退避序列/瞬时失败重试成功/耗尽返回/验证码不重试）+ MCP 缓存单测（命中/key 区分/TTL/LRU）+ MCP 集成（相同 query 二次 cached=true、no_cache 绕过） |
| 风险 | 缓存时效性（TTL 60s，命中刷新；`no_cache` 逃生阀）；重试放大延迟（退避封顶 8s + 全局 timeout 兜底） |

### P1：MCP 体验完善（compact 精简模式 + 工具面）—— ✅ 已完成（2026-08）

| 项 | 内容 |
|---|---|
| 现状 | 单 `web_search` 工具；结果全量返回（完整 snippet），agent 上下文预算敏感时浪费 token |
| 目标 | `compact` 精简模式（title+url）；`list_engines`/`doctor` 工具（agent 自查环境，无需读错误码） |
| 改动点 | ① [output.rs](../src/output.rs) 新增 `success_compact`（results 仅 rank/title/url，meta 完整）；② [mcp.rs](../src/mcp.rs) `SearchParams` 新增 `compact: bool`（缓存命中路径同样生效）；③ 新增 `list_engines`（复用 `engines::AVAILABLE`）/`doctor`（复用 `DoctorReport`，后者加 Serialize）工具 |
| 契约影响 | 输出 schema v1 不变（compact 为请求参数，精简只读视图；结果字段减少但语义等同 max_results 截断）；新工具对既有客户端无感 |
| 验证 | output 单测（compact 仅含 rank/title/url）+ MCP 集成（compact 输出无 snippet、list_engines/doctor 出现在 tools/list 且调用成功） |
| 风险 | 低 |

### P1：正文抓取与结构化提取（fetch_page / `worbrow fetch`）—— ✅ 已完成（2026-08，[ADR-009](adr/0009-fetch-page.md)，专项规划见 [roadmap-fetch.md](roadmap-fetch.md)）

| 项 | 内容 |
|---|---|
| 现状 | agent 只能拿搜索结果列表（title/url/snippet）；"比较/筛选"类任务（找最便宜的 X）缺正文与结构化字段 |
| 目标 | ① `worbrow fetch <url>` 子命令 + MCP `fetch_page` 工具：复用浏览器会话抓取 agent **显式传入** 的 URL，返回清洗后正文；② `fetch_page(url, extract: [...])` 结构化字段提取（allowlist：title/author/published_at/price/currency/rating/rating_max/reviews_count，JSON-LD → meta → DOM，缺失缺省不编造） |
| 改动点 | ① [domain.rs](../src/domain.rs)：`DEFAULT_MAX_CHARS`/`ExtractField`/`FetchedPage`；② [extract.rs](../src/extract.rs)：`extract_main_text`/`extract_fields`；③ [app.rs](../src/app.rs)：`fetch`/`run_fetch`/`run_fetch_with`（镜像 run 三入口）+ `wait_load` + URL 校验前置；④ [output.rs](../src/output.rs)：fetch 成功包；⑤ [cli.rs](../src/cli.rs)/[main.rs](../src/main.rs)：`Fetch` 子命令（`--extract`/`--max-chars`/`--no-text`；共享 flag 标 global）；⑥ [mcp.rs](../src/mcp.rs)：`fetch_page` 工具（会话池复用 + 健康判定）；⑦ [lib.rs](../src/lib.rs)：re-export 新类型 |
| 契约影响 | 新 sibling fetch 成功包（schema v1 同版本）；search 成功/失败包零变化；新工具对既有客户端无感；退出码复用冻结语义 |
| 验证 | extract 单测（噪音剥离/截断/字段类型）；app 单测（URL 归一化/正文+字段+final_url/text=false）；FakeDriver 集成；MCP/CLI 集成（成功包/非法 URL/非法 extract → isError 或 exit 2） |
| 风险 | 正文提取质量依赖页面结构（尽力语义明示）；SSRF/内网可达（工具描述 + README 明示，prompt injection 缓解）；合规划界（fetch = 用户显式导航，与 snippet-only 政策分离） |

### P2：新引擎（baidu）

复用 `SearchProvider` 模板 + fixture（见 [CONTRIBUTING.md](../CONTRIBUTING.md) 常见任务）。前置评估：反爬强度、headless 可达性；失败走 `EngineFailure`（exit 4）。若收益不足可推迟或不做。

## 4. 实施顺序

CDP 后端 → 会话复用 → 搜索参数增强 → agent 契约增强 → 引擎降级 →
结果质量信号（[roadmap-result-quality.md](roadmap-result-quality.md)）→
搜索参数补全（[roadmap-search-params.md](roadmap-search-params.md)）→
网络重试与缓存 → MCP 体验完善 → 正文抓取与结构化提取
（[roadmap-fetch.md](roadmap-fetch.md)，ADR-009）→（P2 baidu 视评估）

> 会话复用：已落地（ADR-007，见 §3 P1 章节）。
> 网络重试与缓存：已落地（ADR-008，见 §3 P1 章节）。
> MCP 体验完善：已落地（见 §3 P1 章节）。
> 正文抓取与结构化提取：已落地（ADR-009，见 §3 P1 章节）。

- 每步独立可验证、可回退；完成后同步 `doctor`、README、CHANGELOG
- 不承诺时间；重要取舍（如 CDP 传输层、会话池默认策略）以 ADR 记录

## 5. 契约与约束（贯穿全程）

- `schema_version` 只增不改；破坏契约需 bump 主版本并记 ADR（[AGENTS.md](../AGENTS.md) 硬约束 3）
- 退出码语义冻结（0/2/3/4/124/1）
- 安全红线：不自动访问搜索结果中的第三方 URL（只输出）、不绕过验证码
- 新增引擎 = 新文件 + `engines/mod.rs` 注册一行（硬约束 4）

## 6. 开放决策

1. **CDP 传输层**：沿用已预留的 `tokio-tungstenite`（与 ADR-002 一致），还是评估更轻替代 → 倾向沿用
2. **会话池默认策略**：CLI 单次搜索（用完即回收）与 MCP 长驻（TTL 空闲回收）是否分策略 →
   倾向 MCP 场景默认启用；默认 `--max-sessions`/`--session-ttl` 取值见
   [roadmap-session-pool.md](roadmap-session-pool.md) §6
3. **缓存作用域与 TTL**：✅ 已定——仅 MCP 长驻场景生效（CLI 单次无状态不缓存）；
   TTL 60s（命中刷新），`no_cache` 逃生阀（ADR-008）
4. **重试触发范围**：✅ 已定——仅 `Error::Network` 重试（指数退避封顶 8s，计入全局
   timeout）；验证码/参数错误/超时/引擎解析失败（有降级链）不重试（ADR-008）
