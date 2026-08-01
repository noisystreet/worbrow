# ADR-007：MCP 会话池化（浏览器进程复用 + 空闲 TTL 回收）

- 状态：已接受
- 日期：2026-08-01

## 背景

每次搜索都 `drivers::resolve` → spawn 新浏览器进程（Firefox 启动 + NewSession /
Chrome 启动 + WS 握手），实测单次 **2-5s**。CLI 单次进程即用即走无优化空间；
**MCP 长驻进程**每次 `web_search` 都重复 spawn，高频场景开销显著（100 次调用
累计 200-500s）。方案（roadmap P1，见 [roadmap-session-pool.md](../roadmap-session-pool.md)）：
MCP 进程内复用浏览器会话。

## 决策

- **会话池**：新增 `src/drivers/pool.rs`（drivers 内部服务，不属稳定公开面）——
  空闲会话 LIFO（最近归还优先复用）+ `Semaphore` 限并发 + TTL reaper 后台任务
- **借出/归还（RAII）**：`SessionGuard` 持 `Box<dyn BrowserDriver>` + semaphore
  permit；Drop 时健康 → 回池，不健康 → driver Drop（触发 kill 子进程，design.md §8）
- **健康判定 = 命令级错误驱动**：`Error::Network/Timeout` → 标记不健康丢弃重建
  （浏览器崩溃/连接断开场景）；`Error::Captcha/Engine/Cli` → 浏览器本身健康可复用。
  不选 acquire 时主动 ping（每次借出多一次命令往返，成本不划算）
- **app 拆分**：`app::run` 拆出 `pub(crate) run_with(&mut dyn BrowserDriver, config)`，
  CLI（`run`）与 MCP（池化）共用同一编排；`BrowserDriver` trait 与 `drivers::resolve`
  签名不变
- **默认值（roadmap §6 已定）**：`--max-sessions=1`（单用户串行省内存，超限排队）、
  `--session-ttl=60s`（与 MCP 空闲超时同量级）、`max_idle=4`（防峰值后进程堆积）
- **作用域**：仅 MCP 长驻启用（`worbrow mcp --max-sessions N --session-ttl S`）；
  CLI 单次不池化，行为不变。池按 `BrowserKind` 分建（fake/chrome/firefox 不混池）
- **契约**：输出 schema v1 / 退出码 / `BrowserDriver` trait / `resolve` 签名全部不变，
  会话复用对 agent 透明

## 后果

- **得到**：MCP 高频搜索从每次 spawn（2-5s）降为复用（≈0）；TTL 回收防残留；
  并发上限有界；崩溃会话自动重建
- **付出**：池自身约 200 行 + app 拆分；健康判定为启发式（命令错误驱动，
  无法提前发现"进程僵死但命令未超时"的罕见场景——由命令超时兜底）
- **拒绝**：跨进程会话复用（`--connect`，design.md §13 V2 再评估）；多实例/分布式池
  （单机工具）；CLI 池化（单次进程无收益）

## 验证

`cargo test`：pool 单测（复用/TTL/健康丢弃/排队/spawn 失败 permit 释放）+ app 集成
（`run_with` 与 `run` 结果等价）+ MCP 集成（fake 路径回归）。真机冒烟
（CI 外，`--ignored`）：`pool_reuses_same_firefox_process` 断言连续两次
acquire 复用同一进程（进程数不增长）。
