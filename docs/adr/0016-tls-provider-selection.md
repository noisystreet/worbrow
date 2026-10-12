# ADR-016：rustls TLS provider 由下游 feature 选择（issue #62）

- 状态：已接受
- 日期：2026-10-12

## 背景

ADR-015 把静态 SERP HTTP 客户端从 reqwest 换成 ureq，并枚举 ureq 的 `rustls` feature
（`features = ["rustls", "gzip"]`）。当时未察觉：**ureq 3 的 `rustls` feature 不是"只接入
rustls"，而是内含 `_ring` → 强制 `rustls/ring`**（`ureq-3.4.2/Cargo.toml`：
`rustls = ["rustls-no-provider", "_ring", "rustls-webpki-roots"]`，`_ring = ["rustls?/ring"]`）。

问题在下游集成时暴露（issue #62）：当消费者同时依赖 `reqwest`（其 rustls 后端用默认的
**aws-lc-rs**）时，Cargo 的 **feature 统一（只增不减）** 会把同一份 `rustls` 的 feature
合并——`ring` 与 `aws_lc_rs` **同时启用**，于是：

- 双重编译两套 TLS backend（`ring` + `aws-lc-sys`），编译时间与产物体积翻倍；
- 拖入 `aws-lc-sys` 的 `cmake` 构建依赖（部分环境无 cmake 直接编译失败）；
- 下游**无法削减**：`ring` 由 worbrow 自己的 feature 图强加，消费者改不动。

根因是 worbrow 把"选哪个 provider"的决策**写死在上游**，而 rustls 的 provider feature
恰恰只能加不能减——上游一旦选了，下游就再也去不掉。

## 决策

把 provider 选择权**交还下游**，worbrow 自身不写死：

- ureq 改用 **`rustls-no-provider`**（只接入 rustls、不绑定 provider）：
  `ureq = { version = "3", default-features = false, features = ["rustls-no-provider", "rustls-webpki-roots", "gzip"] }`
  ——不再使用 ureq 的 `rustls` feature（它强制 `rustls/ring`，正是 issue #62 根因）
- 新增常驻 `rustls = { version = "0.23", default-features = false }`：仅用于 provider
  选择与向 ureq 显式注入；ureq 已以 `rustls-no-provider` 拉入同一 `rustls`，**不增依赖体积**
- 新增三个**互斥语义**的 feature，默认保持现状（ring，musl 静态友好）：

  | feature | 效果 | 适用 |
  |---|---|---|
  | `tls-ring`（默认） | 捆绑 `rustls/ring` | 默认 / musl 静态链接 |
  | `tls-aws-lc` | 捆绑 `rustls/aws-lc-rs` | 与下游 reqwest(aws-lc-rs) 对齐 |
  | `tls-none` | 不捆绑 provider | 用进程 `CryptoProvider::install_default` 的默认值 |

- `http_serp::build_agent` **显式注入** provider：`TlsConfig::builder()
  .provider(Rustls).unversioned_rustls_crypto_provider(provider)`。**这是必须的**——ureq
  以 `rustls-no-provider` 编译后，其 `tls/rustls.rs::build_config` 的 provider 解析链为
  `注入值 → CryptoProvider::get_default() → ring_if_enabled()`，而 `ring_if_enabled` 仅在
  ureq **自身** `_ring` feature 开启时返回 ring，否则 **`panic!`**。不注入的 https 请求会 panic。
- **无 provider 时优雅回退**：`tls-none` 且进程未 `install_default` 时，`tls_provider()`
  返回 `None`，`UreqHtmlGet` 内层为 `None`，`get()` 返回 `Error::Network`；`app.rs` 的
  `try_static_http` 既有逻辑把 HTTP 失败视为"HTTP 直抓不可用"并**回退浏览器**——不 panic、
  不改变输出契约。
- **确定性优先而非互斥守卫**：`--all-features`（CI 的 `cargo clippy --all-targets
  --all-features`）会同时开启 `tls-ring` + `tls-aws-lc`，故**不引入 `compile_error!` 互斥
  守卫**；`bundled_provider()` 用 cfg 分级，二者同时开启时**确定性取 ring**。

### 明确不做

- 引入 `chromiumoxide`/`fantoccini`/`playwright` 或换 HTTP 客户端：与 ADR-015 同口径，否决
- 在 worbrow 内 `CryptoProvider::install_default()`：进程级单例会污染宿主程序、且多次调用
  panic，绝不代消费者决定
- `compile_error!` 守卫 `tls-ring` + `tls-aws-lc` 互斥：会挂掉 `--all-features` 构建
- 改动 `HtmlGet` 端口 / 分层 / 输出 schema：与 ADR-015 一致，契约零变化

## 后果

- **得到**：下游可 `default-features = false, features = ["tls-aws-lc"]` 与 reqwest 共享
  单一 `aws-lc-rs`（实测 `cargo tree -i ring` 输出 "nothing to print"，ring 完全不在树）；
  默认（`tls-ring`）行为与 ADR-015 一致，`aws-lc-sys` 不在默认树；issue #62 解决
- **付出**：ureq 不再自选 provider，**所有 agent 构造必须显式注入**——这是最易踩的坑
  （遗漏注入 → https panic），故在 `build_agent` 注释与 ADR 中双重说明
- **取代**：本 ADR 取代 ADR-015 中"rustls 走 ring + webpki-roots"的选型细节；`webpki-roots`
  保留，ring 从"写死"改为"`tls-ring` 默认提供"
- **测试**：经 `UreqHtmlGet::new`（即 `agent_for → tls_provider`）的用例需 provider 可用——该判定在
  **构造期**发生，与请求走 http/https 无关；故 `get_html_uses_proxy_when_configured` 以
  `cfg(any(tls-ring, tls-aws-lc))` 门控（`tls-none` 下无 agent 可构造）。仅 `direct_agent()` 系用例
  走 http、不经 provider 选择，不受影响。JSON schema / 退出码零变化

## 备选

- 保留 ureq 的 `rustls` feature：即 issue #62，下游无法削减 `ring`，否决
- 只暴露 `tls-aws-lc` 并默认启用：改变默认产物特性（musl 静态需 cmake），且多数消费者
  并不依赖 reqwest，故仍以 `tls-ring` 为默认，按需切换，否决
- 运行时读取环境变量选 provider：provider 是编译期链接产物，运行时选择不可行，否决
