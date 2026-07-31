# ADR-002：浏览器驱动 = 自研双协议后端（CDP + Marionette）

- 状态：已接受
- 日期：2026-07-31

## 背景

两条浏览器协议（Chrome DevTools Protocol、Firefox Marionette）都是公开协议。
候选方案：现成库 chromiumoxide / fantoccini / playwright-rs，或直接手写协议客户端。

## 决策

**直接手写协议客户端**，不依赖 chromiumoxide / fantoccini：

| 后端 | 协议 | 驱动目标 | 命令子集 |
|---|---|---|---|
| **drivers/cdp.rs** | Chrome DevTools Protocol（HTTP 发现端点 + WebSocket JSON-RPC） | Chrome / Edge / Chromium | ~15-20 个 |
| **drivers/marionette.rs** | Firefox Marionette（WebSocket，`firefox -marionette`） | Firefox（监听 127.0.0.1:2828） | ~5-8 个 |
| drivers/fake.rs | 无（读 fixture HTML） | 测试 | — |

## 后果

- **得到**：依赖链最轻（仅 tokio + tokio-tungstenite + serde）；**原生同时支持 Chrome 系与
  Firefox**，无需任何中间驱动进程；无第三方库 API 变动绑架；对"搜索"这种窄功能面，所需
  协议命令很少，手写成本可控
- **付出**：协议细节自己维护（Chrome 每 4 周发版可能有 breaking change）；事件订阅时序、
  等待加载等细节要自行封装；比用现成库多约 1-2k 行代码
- **放弃现成库的原因**：chromiumoxide 只支持 Chromium 系（Firefox 需另配
  fantoccini/WebDriver，引入 driver 进程），且大版本 API 变动频繁、依赖链约 30 个 crate；
  自研后两个浏览器协议统一收敛在一个 trait 后，行为一致
- **风险与缓解**：协议命令表在 `drivers/` 内集中登记 + `worbrow doctor` 验证连通性；
  若后续需要网络拦截等深度控制，再回退引入 chromiumoxide 作为第二个 CDP 实现
  （trait 不变，可随时切换）
