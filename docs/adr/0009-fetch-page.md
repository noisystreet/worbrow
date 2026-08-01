# ADR-009：正文抓取与结构化提取（`fetch_page` / `worbrow fetch`）

- 状态：已接受
- 日期：2026-08-01

## 背景

agent 用 worbrow 只能拿到搜索结果列表（title/url/snippet），两个闭环缺口
（roadmap-fetch.md，决策见 §6）：

1. **结果正文抓取**：拿到链接后无法直接读正文，必须自行 HTTP 抓取，重复处理 UA/反爬
2. **结构化结果解析**：「找最便宜的 X」类比较/筛选任务需要价格/评分/作者等结构化字段，
   摘要里只有扁平 snippet

现有基建已齐（`BrowserDriver::navigate/html/eval`、MCP 会话池 ADR-007、`scraper` 依赖、
`extract::clean_text`），无需新协议/新依赖。

## 决策

### 新 sibling 契约（对既有客户端零破坏）

- **CLI**：新增 `worbrow fetch <url>` 子命令（`--extract`/`--max-chars`/`--no-text` 局部；
  `--browser`/`--timeout`/`--json`/`--retry`/`--log-level`/`--screenshot` 标 `global = true`，
  非破坏——`worbrow mcp --json` 会被接受但忽略，MCP 不走 stdout 契约）
- **MCP**：新增 `fetch_page` 工具（`FetchParams`），复用会话池 + 健康判定（ADR-007 路径）
- **输出**：新 sibling 成功包
  `{schema_version: 1, url, fetched_at, text, extracted, meta{elapsed_ms, chars, truncated, final_url}}`；
  失败包复用统一信封 `{schema_version, error}`；search 成功/失败包一字不动，schema v1 不 bump
- **退出码**：复用冻结语义（非法 URL → `cli`/2；网络 → `network`/4；超时 → `timeout`/124）

### 正文提取（尽力语义，非 Readability 级）

- 优先 `article`/`main` 容器回退 `body`；剥 `script/style/noscript/nav/footer/header/aside/form/iframe/template`
  噪音容器文本（含祖先链判断）；复用 `clean_text`；按 `max_chars` 截断（默认 20,000 字符，
  `truncated` 标志）
- 页面等待无结果选择器 → `wait_load`：`eval` 轮询 `document.readyState == "complete"`，
  **尽力语义**（预算耗尽/eval 失败不报错，导航成功即成功包）
- **已知行为**：HTTP 4xx/5xx/验证码/404 页导航成功即成功包（v1 不检测 HTTP 状态码，
  agent 从 `text` 内容判断）；SPA/懒加载内容可能缺失（真实浏览器 DOM 已缓解一部分）；
  `meta.final_url` 记录重定向落地页（`eval("location.href")`）

### 结构化提取（`fetch_page` 的 `extract` 参数，不扩 SearchResult）

- **拒绝扩 `SearchResult`**：SERP 摘要无跨引擎稳定选择器，price/rating/author 多数恒 `null` →
  全局 schema 噪音；结构化数据真正在目标页（JSON-LD/meta/DOM），正好是 fetch 的返回面。
  SERP 层唯一稳定字段是日期（已有 `published_at`）
- **allowlist 枚举** `ExtractField`：`title/author/published_at/price/currency/rating/rating_max/reviews_count`
  （枚举只增不改）；CLI kebab-case（`published-at`）与 MCP snake_case（`published_at`）并存，
  均经 `ExtractField::from_arg` 单一解析源
- **提取优先级**：JSON-LD（`application/ld+json`，递归搜索首个命中 key）→ meta（og:/twitter:/
  article:/product:）→ DOM 启发式（title/h1）；**缺失字段缺省，绝不编造**；值保留 JSON 原生
  类型（price 字符串 / rating 数字）
- **`text=false` 开关**（`--no-text`）：只要 `extracted` 时省 agent token

### 安全与合规（硬约束 6 边界）

- **只抓 agent 显式传入的 URL**：`url` 为必填参数，绝不隐式跟随搜索结果（闭环用法 =
  `web_search` 的 `results[i].url` 经 agent 显式挑选后传入）
- **scheme 白名单**：仅 `http/https`；缺 scheme 自动补 `https://`（与浏览器行为一致）；
  拒绝 `file:`/`javascript:`/`data:`；去 fragment；URL 校验在 app 层**前置**
  （非法 URL 不启动浏览器）
- **可达性明示**：fetch 用真实浏览器导航，可访问 `localhost`/内网/云 metadata 端点
  （等价本机浏览器行为），v1 不做 IP 过滤；`fetch_page` 工具描述与 README 写明
  「仅显式 URL + 可达本机/内网」，防 agent 被 prompt injection 诱导抓内网
- **页面 JS 执行**：目标页 JS 在浏览器进程内真实执行（与用户点链接风险等价，headless
  沙箱化），只读回 HTML/正文/字段，不执行页内动作
- **合规重划界**：design.md §9「仅抓取摘要，不盗用整页正文」与 fetch 整页抓取划界为
  「fetch = 用户显式发起的导航，等价用户点开链接，与搜索爬虫 snippet-only 政策是两条
  独立路径」；频率纪律（≥2s/请求）仍适用；不做批量抓取

### 明确不做（v1）

- fetch 结果的 MCP 短 TTL 缓存（页新鲜度优先，正文内存成本高）
- Readability 级评分正文提取 / HTTP 状态码检测 / 批量抓取 / IP 过滤

## 后果

- **得到**：agent「搜到链接 → 读内容 → 比较筛选」一步到位；结构化字段免正则猜测；
  search 契约零变化、对既有客户端无感
- **付出**：新 sibling 契约（agent 端需知悉 fetch 包形状）；正文提取质量依赖页面结构
  （尽力语义已明示）；整页抓取打开第三方站点访问面（用户显式授权 + 工具描述明示边界）
- **拒绝**：扩 `SearchResult` schema（噪音 + 引擎维护成本）、fetch 缓存（新鲜度/内存）、
  批量抓取（频率纪律）

## 验证

- extract 单测：正文噪音剥离/截断/空页；字段 meta 优先 + JSON-LD 原生类型 + 缺失缺省
- app 单测：URL 归一化（补 https/去 fragment/拒绝 file:）；`run_fetch_with` 正文+字段+
  final_url；`text=false`；非法 URL 校验前置不 resolve 浏览器
- FakeDriver 集成：`run_fetch`（resolve fake = SMOKE_HTML）截断/title 提取/自动补 scheme
- MCP 集成：tools/list 含 `fetch_page` 且 url 必填；fake 成功包；非法 extract/非法 URL →
  工具级错误（isError，失败包 code=cli）
- CLI 集成：非法 URL → exit 2 失败包；缺 url/非法 extract → clap exit 2（成功路径由
  app/MCP fake 覆盖，CLI 不暴露 fake 后端）
- **真机冒烟（CI 外，2026-08）**：Firefox（Marionette）与 Chrome（CDP）实网抓取
  example.com/github.com 均通过——正文提取、`meta.final_url`（http→https 重定向
  正确记录）、og:title 字段提取正常；冒烟暴露 Marionette `ExecuteScript` 需显式
  `return` 才返回值（裸表达式 `document.readyState` 返回 undefined）→ eval 包装为
  `return (expr);` 对齐 CDP `Runtime.evaluate` 语义（`ports::BrowserDriver::eval`
  契约 = "JS 表达式 → 求值结果"）
