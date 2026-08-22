# ADR-011：静态 SERP 优先 HTTP GET，失败再回浏览器

- 状态：已接受
- 日期：2026-08-22

## 背景

默认搜索走 headless 浏览器直访引擎 URL（ADR-003）。DuckDuckGo 的
`html.duckduckgo.com/html/` 是为无 JS 客户端准备的静态页：`curl` 即可得到稳定有机
结果。用 Firefox/Chrome 打开同一 URL 更慢，且可能碰到同意墙或与 curl 不同的 HTML。

同时，Bing 在 CN IP + headless 下经常给出词典/百科锚定等劣化 SERP，却仍能通过数量/
类型门禁被采用，观感差于 curl。默认引擎链因此改为 DDG 优先（见 `DEFAULT_ENGINE`）。

## 决策

- **默认引擎链** = `duckduckgo,baidu,bing`（CLI 与 MCP 共用 `DEFAULT_ENGINE`）
- **静态 HTML 引擎**（当前仅 DuckDuckGo）声明 `SearchProvider::prefer_http_html()`：
  先对 `result_url` / `page_url` 做 HTTP GET，解析成功即采用；GET 失败或 `parse`
  失败则回退既有浏览器路径（navigate → wait_for → html）
- HTTP 客户端是适配器（`http_serp.rs`），`domain` 无 IO；`app` 只通过端口编排
- **真实浏览器**（CDP / Marionette）`allows_http_serp() = true`；**FakeDriver 为 false**，
  保证 CI/fixture 测试不打外网、不被 HTTP 结果替换夹具
- 安全边界：仅 http/https；跟随有限次重定向；响应体上限（防内存撑爆）；超时取自
  页面等待预算；4xx/5xx 视为 GET 失败并回退浏览器

### 明确不做

- 用 HTTP 抓 Bing/百度 SERP（JS 水合 / 反爬，不是静态 HTML）
- 把 reqwest 引入 `domain`
- 改变 JSON schema / 退出码

## 后果

- **得到**：DDG 结果与 curl 同源、冷启动更快；浏览器仍覆盖 HTTP 失败与非静态引擎
- **付出**：新增 `reqwest`（rustls）；真实浏览器路径上 DDG 多一次失败才会回退
- **测试**：Fake 路径行为不变；HTTP 成功/失败回退用注入的 `HtmlGet` 单测，不依赖外网

## 备选

- 全部引擎都 HTTP：Bing/百度静态 HTML 质量差，否决
- 仅改默认引擎、仍用浏览器打 DDG html：能改善排序，但延迟与页面形态仍不如 curl，否决
