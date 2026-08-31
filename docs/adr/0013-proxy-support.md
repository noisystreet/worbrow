# ADR-013：HTTP/HTTPS 代理支持（`--proxy`）

- 状态：已接受
- 日期：2026-08-31

## 背景

worbrow 落地了静态 SERP HTTP 直抓（ADR-011）、CDP（Chrome/Edge）与 Marionette（Firefox）
双后端。部分使用环境（CN 网络、企业网络、本地代理工具如 Clash/v2ray）需要把浏览器与
HTTP 直抓都经由显式代理访问引擎。此前这是 design.md §14 的开放问题 #2（"是否提供
`--proxy`"），未实现——浏览器走系统代理、HTTP 客户端（reqwest）读 `HTTP_PROXY` 等
环境变量，但无法显式指定且行为不可控。

## 决策

- 新增 **`--proxy <url>`**（CLI 全局参数）+ `Config::with_proxy` / `FetchConfig::with_proxy`
  （lib）+ `worbrow mcp --proxy <url>`（server 级，经 `PoolConfig.proxy`）
- **代理 URL 白名单**：`http://host:port` 与 `https://host:port`（含显式端口或缺省端口，
  https 缺省 443）；非法 URL / 非 http(s) scheme（如 `socks5://`）/ **含凭据**（`user:pass@`，
  凭据会泄漏到浏览器进程命令行且三处行为不一致）/ **含 path/query**（Chrome `--proxy-server`
  误解析）→ `Error::Cli`（exit 2），在**启动浏览器之前**校验（`drivers::resolve_with` 前置；
  MCP 在 `serve_stdio` 启动期）
- **三处生效**：
  1. CDP（Chrome/Edge）：启动参数 `--proxy-server=<url>`
  2. Marionette（Firefox）：profile `user.js` 写 `network.proxy.type=1`（手动）+
     `network.proxy.http/http_port/ssl/ssl_port`（http/https 同址同端口）
  3. 静态 SERP HTTP 直抓（`http_serp`）：reqwest `Proxy::all`（HTTP GET 路径）
- **作用域**：MCP 为 server 级配置（会话池按固定代理 spawn，复用语义不变），
  工具参数**不**提供 per-request proxy；CLI 每次调用独立指定
- `None`（缺省）= 保持现行为：浏览器走系统代理，reqwest 读 `HTTP_PROXY`/`HTTPS_PROXY`/
  `ALL_PROXY`/`NO_PROXY` 环境变量

### 明确不做

- `socks5://` 代理：reqwest 需额外 `socks` feature，Chrome/Firefox 也各自有坑；
  收益不匹配成本，留作后续（需求驱动再加）
- per-request proxy（MCP）：与会话池"固定参数复用"语义冲突，server 级已够用
- 把代理写进 JSON schema / 退出码（请求参数，输出契约 v1 零变化）
- 在 `domain` 引入 URL 解析（`url` 为解析框架，domain 只允许 serde/chrono；代理解析
  收敛在 adapters 层）

## 后果

- **得到**：CN/企业网络可用；浏览器与 HTTP 直抓路径行为一致；非法参数在启动前报
  `exit 2`（与 fetch URL 校验同语义）
- **付出**：每后端多一个启动参数面；Marionette 的 profile 生成逻辑增加代理偏好
  （防御性二次解析，解析失败即 `Error::Env`，不静默直连）
- **测试**：代理 URL 校验单测（drivers）+ Marionette user.js 偏好单测 + app 集成
  （合法代理 Fake 成功 / 非法代理 Cli 错误）+ CLI 集成（`--proxy socks5://…` → exit 2）
- 真机验证：`worbrow --proxy http://127.0.0.1:<port>` 在本地代理下跑真实搜索（CI 外）

## 备选

- 仅浏览器支持、HTTP 直抓不跟进：DDG 走浏览器代理但 HTTP 路径直连，行为分裂，否决
- 只靠环境变量：不可显式指定、CLI 不可移植，否决
- 让 `Config` 直接持有 `reqwest::Proxy`：domain 层引入 reqwest 依赖，违反分层，否决
