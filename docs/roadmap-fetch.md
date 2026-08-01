# worbrow 正文抓取与结构化提取（fetch_page）规划

> 读者：项目维护者 / 贡献者。状态：**已通过（2026-08，决策见 §6）**，实施中随进度更新；落地决策按 AGENTS.md 记 ADR（ADR-009）。
> 本文聚焦「搜索闭环能力」，架构权威见 [design.md](design.md)，主功能路线见 [roadmap.md](roadmap.md)。

## 1. 背景与现状

agent 用 worbrow 目前只能拿到搜索结果列表（title/url/snippet），两个闭环缺口：

1. **结果正文抓取**：拿到链接后无法直接读正文，必须自己再走一遍 HTTP 抓取，重复处理 UA/反爬
2. **结构化结果解析**：「找最便宜的 X」类比较/筛选任务需要价格/评分/作者等结构化字段，摘要里只有扁平 snippet

基建盘点（**均已有，无新依赖**）：

- [ports.rs](../src/ports.rs) `BrowserDriver` 已有 `navigate`/`html`/`eval`——取正文所需全部原语
- [pool.rs](../src/drivers/pool.rs) `SessionPool` + [app.rs](../src/app.rs) `run_with`——MCP 已复用浏览器进程（ADR-007）
- `scraper` 已是依赖（引擎解析在用），[extract.rs](../src/extract.rs) 已有 `clean_text`/`url_origin`

## 2. 目标与非目标

### 目标

1. `worbrow fetch <url>` CLI 子命令 + `fetch_page` MCP 工具：复用现有浏览器会话抓取 **agent 显式传入** 的 URL，返回清洗后正文文本
2. `fetch_page(url, extract: [...])` 结构化字段提取（allowlist）：JSON-LD → meta → DOM 启发式，缺失即 `null`，绝不编造
3. 对既有客户端**零破坏**：search 成功/失败包一字不动；fetch 为新增 sibling 契约（同 `list_engines` 先例）

### 非目标（明确不做）

- **不扩 `SearchResult` schema** 加 price/rating/author：SERP 摘要无跨引擎稳定选择器，多数时候恒 `null` → 全局 schema 噪音；结构化数据真正在目标页（见 §3 决策）
- **不做 Readability 级评分正文提取**：v1 为「尽力正文」（剥噪音容器 → article/main → body），不承诺质量
- **不隐式跟随搜索结果**：硬约束 6——fetch 只抓 agent 显式传的 URL，绝不自动访问搜索结果中的第三方链接
- **不做 fetch 的 MCP 短 TTL 缓存**（v1）：正文体积远大于搜索结果，缓存内存成本高；页新鲜度优先
- **不做批量抓取**：单次单 URL；频率纪律（≥2s/请求）延续 search 约定，防刷站

## 3. 方向与优先级

### P1：正文抓取（fetch）—— ✅ 已完成（2026-08）

| 项 | 内容 |
|---|---|
| 现状 | 无；agent 拿到链接后需自行 HTTP 抓取 |
| 目标 | CLI/MCP 双侧抓取显式 URL → 清洗正文文本，复用浏览器会话（CLI 单次 spawn、MCP 走会话池） |
| 改动点 | ① [domain.rs](../src/domain.rs)：`FetchQuery`（url/max_chars/text 开关）+ `FetchedPage` DTO；② [extract.rs](../src/extract.rs) 新增 `extract_main_text(html, max_chars)`（剥 script/style/nav/footer/header/aside + 注释 → 优先 `article`/`main` 回退 `body` → 复用 `clean_text`）；③ [app.rs](../src/app.rs)：`fetch`（同步）/`fetch_with`（async，镜像 `run`/`run_with`，MCP 复用池）+ `wait_load`（`eval` 轮询 `document.readyState == "complete"`，浏览器无关）+ `eval("location.href")` 取重定向落地页 `meta.final_url`；④ [output.rs](../src/output.rs)：fetch 成功包/人读文本（失败包复用统一信封）；⑤ [cli.rs](../src/cli.rs)/[main.rs](../src/main.rs)：`Fetch` 子命令（`--extract`/`--max-chars`/`--no-text` 局部；`--browser`/`--timeout`/`--json`/`--retry`/`--log-level`/`--screenshot` 标 `global = true`，非破坏——`worbrow mcp --json` 会被接受但忽略，mcp 不走 stdout 契约）；⑥ [mcp.rs](../src/mcp.rs)：`fetch_page` 工具 + `FetchParams`，走既有 `pool_for(browser).acquire()` + 健康判定；⑦ [lib.rs](../src/lib.rs)：re-export 新类型（ADR-006 公开面） |
| 契约影响 | 新 sibling 成功包：`{schema_version: 1, url, fetched_at, text, extracted, meta{elapsed_ms, chars, truncated, final_url}}`；失败包复用 `{schema_version, error}`；退出码复用冻结语义（非法 URL → `cli`/2，网络 → `network`/4，超时 → `timeout`/124） |
| 验证 | 正文提取单测（新增文章页 fixture：噪音容器剥离/截断）；FakeDriver 集成（fetch 成功/非法 URL；fake.rs 需支持按 URL 返回 fixture）；MCP tools/list 断言；CI 外真机冒烟（真实 URL 如 example.com/项目 README 验证提取） |
| 风险 | 页面等待无选择器（readyState 轮询兜底）；SPA/懒加载/付费墙/反爬页提取质量差（尽力语义 + 文档明示）；正文体积失控（`max-chars` 默认 20k + `truncated` 标志；`--no-text` 时零正文开销） |

### 已知行为与局限（fetch 契约语义，实现前定死）

- **HTTP 4xx/5xx/验证码/404 页**：`BrowserDriver` 不暴露 HTTP 状态码，导航成功即返回成功包（正文为空或错误页文本均合法），v1 不检测 HTTP 状态；agent 从 `text` 内容自行判断
- **SPA/懒加载**：`document.readyState == "complete"` 即返回，异步渲染内容可能缺失（真实浏览器 DOM 已缓解一部分，v1 明示局限）
- **重定向**：导航后 `final_url` 记录真实落地页，正文以落地页为准
- **`extracted` 值类型**：保留 JSON 原生类型（price 字符串 / rating 数字），不统一转字符串；缺失字段缺省，不编造

### P1：结构化字段提取（fetch_page 的 `extract` 参数）—— ✅ 已完成（2026-08）

| 项 | 内容 |
|---|---|
| 现状 | 无；agent 需靠正则猜 snippet |
| 目标 | allowlist 字段：`title`/`author`/`published_at`/`price`/`currency`/`rating`/`rating_max`/`reviews_count`；提取优先级 JSON-LD（`application/ld+json`）→ meta（`og:`/`twitter:`/`article:`/`product:`）→ 微数据/DOM 启发式；缺失即缺省 |
| 改动点 | ① [domain.rs](../src/domain.rs)：`ExtractField` 枚举（allowlist）；② [extract.rs](../src/extract.rs) 新增 `extract_fields(html, &[ExtractField])`；③ CLI `--extract price,rating` / MCP `extract: [...]`；与正文同一次导航、同 HTML 二次解析 |
| 契约影响 | fetch 包新增 `extracted` 对象（fetch 包本身为新增，无既有破坏）；search 包零变化 |
| 验证 | 提取单测（JSON-LD/meta fixture 各字段命中/缺失）；集成断言 `extracted` 形状 |
| 风险 | 站点结构差异（尽力语义 + 文档明示不承诺全站覆盖）；JS 渲染页靠真实浏览器 DOM 已缓解 |

### 决策：为什么不扩 `SearchResult`

| | A: SearchResult 加可选字段 | B: `fetch_page(extract)`（采用） |
|---|---|---|
| 数据源 | SERP 摘要 | 目标页本身（JSON-LD/meta/DOM） |
| 可靠性 | 跨引擎无稳定选择器，多数恒 `null` | 结构化数据真正存在的地方 |
| 维护成本 | N 字段 × M 引擎解析 | 一套跨站提取器 |
| 契约影响 | 每个搜索结果包变大 | 仅 fetch 包，search 零变化 |

SERP 层唯一稳定可行的结构化字段是日期——**已有**（`published_at`，[extract.rs](../src/extract.rs) `extract_date`）。价格/评分/作者留给目标页提取。

## 4. 实施顺序

正文抓取（P1）→ 结构化提取（P1）→ 测试/文档收口（design.md、README、CHANGELOG、
[roadmap.md](roadmap.md) 主文档标记 ✅、[ADR-009](adr/0009-fetch-page.md)）—— **全部 ✅ 已完成（2026-08）**

- 每步独立可验证、可回退（提取单测 + FakeDriver 集成）
- ADR-009 记录 fetch 契约形态与合规划界（[已记录](adr/0009-fetch-page.md)）

## 5. 契约与约束（贯穿全程）

- `schema_version` 只增不改；fetch 为新增 sibling 契约，search 包零变化（硬约束 3 不触发 bump）
- 退出码语义冻结（0/2/3/4/124/1，[error.rs](../src/error.rs) `exit_code`）
- 安全红线（硬约束 6）：fetch 只抓 **agent 显式传入** 的 URL；scheme 白名单 `http/https`（拒绝 `file:`/`javascript:`/`data:`）
- **可达性明示**：fetch 用真实浏览器导航，可访问 `localhost`/内网/云 metadata 端点（等价本机浏览器行为），v1 不做 IP 过滤；`fetch_page` 工具描述与 README 写明「仅显式 URL + 可达本机/内网」，防 agent 被 prompt injection 诱导抓内网
- **页面 JS 执行**：目标页 JS 在浏览器进程内真实执行（与用户自己点链接风险等价，headless 沙箱化），非纯 HTTP 抓取；只读回 HTML/正文/字段，不执行页内动作
- **闭环用法**：`web_search` 返回的 `results[i].url` 可作 `fetch_page` 输入，但须 agent 显式挑选——工具描述明示「不自动跟随搜索结果」
- **合规重划界（需用户确认后写入 design.md §9/README）**：design.md §9「仅抓取摘要，不盗用整页正文」与 fetch 的整页抓取直接矛盾——划界为「fetch = 用户显式发起的导航，等价用户自己点开链接，与搜索爬虫的 snippet-only 政策是两条独立路径」；频率纪律仍适用

## 6. 决策（2026-08 已定）

1. **合规边界**：✅ 采纳——fetch = 用户显式发起的导航，等价用户点开链接；与搜索爬虫 snippet-only 政策两条独立路径；写入 design.md §9/README
2. **MCP 缓存**：✅ v1 不做（页新鲜度优先，正文内存成本高）
3. **`max-chars` 默认值**：✅ 20,000 字符
4. **extract 字段集合**：✅ 初始 8 字段（`title/author/published_at/price/currency/rating/rating_max/reviews_count`），枚举只增不改
5. **scheme 缺失行为**：✅ 自动补 `https://`（与浏览器行为一致）；非法 scheme（`file:`/`javascript:`/`data:`）→ 参数错误
6. **`text` 开关**：✅ 加入——`--no-text`/MCP `text: false` 只回 `extracted`
7. **`meta.final_url`**：✅ 加入（`eval("location.href")` 取重定向落地页）
8. **`extracted` 值类型**：✅ 保留 JSON 原生类型（price 字符串 / rating 数字），缺失缺省不编造

## 7. 参考

- [design.md](design.md) §6.1（CLI）/§6.2（用例编排）/§7（输出契约）/§9（合规）
- [ADR-007](adr/0007-mcp-session-pool.md)（会话池）、[ADR-008](adr/0008-retry-and-cache.md)（重试/缓存）
- [roadmap-result-quality.md](roadmap-result-quality.md)（专项规划文档模板）
