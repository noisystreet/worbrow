# ADR-015：静态 SERP HTTP 客户端 reqwest → ureq

- 状态：已接受
- 日期：2026-09-12

## 背景

静态 SERP HTTP 直抓（ADR-011）只需要一条**单发、阻塞式**的 HTTPS GET：无流式、无
连接池调优、无 HTTP/2 诉求。但 ADR-011 选型时引入的 reqwest 把整个 async HTTP 栈
（hyper、h2、hyper-rustls、tower、tokio 全套…）拖进了依赖闭包——`cargo tree` 统计
normal 边约 135 个 crate，是本项目最大的编译依赖来源，直接拖慢编译并撑大二进制
（musl 静态产物尤甚）。

`HtmlGet` 端口是隔离缝（`app` 只依赖 trait，`domain` 零 IO），替换 HTTP 客户端不
触碰分层与输出契约。

## 决策

- HTTP SERP 客户端换用 **ureq 3**（阻塞客户端）：
  `ureq = { version = "3", default-features = false, features = ["rustls", "gzip"] }`
  （rustls 走 ring + webpki-roots，与 musl 静态链接诉求一致，避免 aws-lc-rs；
  gzip 对齐原 reqwest 的 gzip feature）
- 端口不变：`http_serp::HtmlGet`（async trait）仍是 `app` 依赖的唯一端口；
  `UreqHtmlGet` 用 `tokio::task::spawn_blocking` 桥接阻塞调用，不占用 async 运行时
  线程；agent 按 `Option<proxy>` 缓存（`OnceLock<Mutex<HashMap<…>>>`），复用语义不变
- **语义逐项对齐 ADR-011/013**：
  - 仅 http/https（`Url` scheme 校验不变）
  - 重定向 ≤10（`Agent::config_builder().max_redirects(10)`，对齐 reqwest
    `Policy::limited(10)`）
  - 响应体上限 2MB（`Body::with_config().limit(MAX_HTML_BYTES)`，超限即 Err）
  - 超时取页面等待预算（每请求 `.config().timeout_global(Some(timeout))`，端到端）
  - 代理：`--proxy` 显式指定走 `ureq::Proxy::new`；`None` 时 `Proxy::try_from_env()` 读
    `ALL_PROXY`/`HTTPS_PROXY`/`HTTP_PROXY`（大小写），`NO_PROXY` 自动生效；显式代理
    非法时告警并降级 env 代理
  - 与 reqwest 的唯一可观察差异：ureq 对 http/https 代理统一先发 **CONNECT 隧道**
    （reqwest 对 http 目标直接发 absolute-form GET）。CONNECT 到任意端口是代理的
    标准能力，对真实代理（Clash/v2ray/企业代理）无影响；假代理单测按两阶段实现
    （先应答 CONNECT，再在隧道内应答 origin-form GET）
- **裁剪**：不启用 ureq 的 `charset` feature——原实现即 `bytes + from_utf8_lossy`，
  从不按响应头 charset 解码，行为不变且省掉 encoding_rs

### 明确不做

- socks5 代理（ureq `socks-proxy` feature）：与 ADR-013 同口径，需求驱动再加
- 把 ureq 引入 `domain` / 改动 `HtmlGet` 端口：分层不变
- 换 async 模型或直接手写 hyper：手写浏览器协议（CDP/Marionette）是刚需，HTTP
  客户端不是，自研不成比例
- ureq 连接池调优：单发 GET 场景收益可忽略

## 后果

- **得到**：HTTP 客户端依赖闭包 ~135 → ~30 个 crate（`cargo tree -p ureq` 实测 27）；
  编译时间与二进制体积显著下降；
  重定向/代理/体积上限/超时/UA 语义全部保持，JSON schema / 退出码零变化
- **付出**：阻塞调用需 `spawn_blocking` 桥接（多一层任务切换，开销可忽略）；
  失去 reqwest 未被本项目用到的能力面（HTTP/2、流式、middleware 等）
- **测试**：`http_serp` 5 个单测等价迁移（读 body / 拒绝非 2xx / 拒绝 file:// /
  代理 CONNECT 隧道断言 / 拒绝超大 Content-Length）；app 注入式集成测试不变，
  不依赖外网

## 备选

- 保留 reqwest、裁 features（`default-features = false` + rustls + gzip）：
  hyper/tokio 全套仍在，闭包仍远大于 ureq，收益有限，否决
- 直接手写 hyper / 自研 HTTP 客户端：维护成本不成比例，否决
- 换其他轻量阻塞客户端（attohttpc、minreq 等）：ureq 3 维护活跃，代理/重定向/
  超时/体积限制语义最完整，选 ureq
