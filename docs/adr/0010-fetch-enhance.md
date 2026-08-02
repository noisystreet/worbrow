# ADR-010：fetch 补强（`meta.http_status` + `wait_selector`）

- 状态：已接受
- 日期：2026-08-02

## 背景

ADR-009 fetch 的两个已知行为短板（README「已知行为」）：

1. **HTTP 状态码缺失**：4xx/5xx/404 页导航成功即成功包，agent 无法区分"正常正文"与
   "错误页/404"，只能从 `text` 内容猜测（不可靠）
2. **SPA/懒加载内容缺失**：`wait_load` 只等 `readyState == "complete"`，SPA 首屏在
   JS 渲染后才出现内容，complete 不代表内容就绪，正文可能为空

## 决策

### `meta.http_status`：尽力语义，零副作用

- 导航后经 `eval` 执行 JS
  `(() => { const n = performance.getEntriesByType('navigation')[0]; return n ? (n.responseStatus || null) : null; })()`
  读取 `PerformanceNavigationTiming.responseStatus`
- **零副作用**：不额外发请求（对比"导航后二次 HEAD/GET"方案，避免重复请求触发反爬/副作用）
- **跨协议统一**：CDP `Runtime.evaluate` 与 Marionette `ExecuteScript` 共用 `eval`
  （IIFE 保持"表达式"语义，兼容 Marionette 的 `return (...)` 包装）
- **尽力语义**：Firefox < 105 / data: URL / eval 失败 → `null`，不改变"导航成功即成功包"
  契约；4xx/5xx/404 的判定交给 agent（`meta.http_status >= 400`）
- 契约：fetch 成功包 `meta` 新增 `http_status: Option<u16>`（schema v1 只增不改）

### `wait_selector`：SPA 内容就绪等待（可选参数，向后兼容）

- CLI `--wait-selector <css>` / MCP `wait_selector` / 库 `FetchConfig::with_wait_selector`
- 导航后在 `wait_load` 之后调用 `driver.wait_for(selector, budget)`，选择器出现再取正文
- **尽力语义**：超时/失败仍返回成功包（正文可能为空）——不改变现有成功包语义，
  `wait_selector` 只是"多等一会儿"；调用方显式指定说明在意内容就绪
- 不传 = 保持现行为（仅等 `readyState=complete`），完全向后兼容

### 明确不做

- 状态码 ≥400 时改为失败包：破坏"导航成功即成功包"既有语义，agent 迁移成本高，收益小
- 自动网络空闲检测（无需参数）：启发式不稳，不如显式选择器可预期

## 影响

- `FetchedPage` 新增 `http_status`；`FetchConfig` 新增 `wait_selector`（公开面只增不改）
- CLI/MCP 参数只增（`--wait-selector` / `wait_selector`），既有客户端无感
- 测试：app 单测（status 读出 + wait_for 调用 + 超时尽力）；output 契约断言；真机冒烟
  （example.com 200、本地 404 服务 404）
