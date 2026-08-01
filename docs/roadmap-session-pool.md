# worbrow 浏览器会话复用 / 池化规划

> 读者：项目维护者 / 贡献者。状态：**✅ 已实现（2026-08，[ADR-007](adr/0007-mcp-session-pool.md)）**。
> 本文为设计记录；架构权威见 [design.md](design.md)，主功能路线见 [roadmap.md](roadmap.md)。

## 1. 背景与现状

当前每次搜索（CLI 或 MCP `web_search`）都经历：

```
drivers::resolve(browser) → spawn 新浏览器进程（Firefox 启动 + NewSession / Chrome 启动 + WS 握手）
→ 搜索 → Drop → kill 子进程（design.md §8）
```

实测单次 spawn 开销约 **2-5s**（`meta.elapsed_ms` 只统计搜索阶段，不含 spawn）。CLI 单次调用
（进程即起即走）无优化空间；**MCP 长驻进程**中每次工具调用都重复 spawn，高频场景开销显著：

| 场景 | 每次 spawn 开销 | 100 次调用累计 |
|---|---|---|
| 现状（每次 spawn） | 2-5s | 200-500s |
| 池化后（复用会话） | ≈0（仅搜索本身） | 20-50s（视搜索耗时） |

已有基础（可直接复用）：
- [ports.rs](../src/ports.rs) `BrowserDriver` trait 已统一两协议（CDP/Marionette），内部
  `Arc<Mutex<Inner>>` 已保证并发安全；fake 后端供测试
- [app.rs](../src/app.rs) 搜索编排集中在 `run`，driver 作为 `Box<dyn BrowserDriver>` 参数传递
- [mcp.rs](../src/mcp.rs) 已有原子化空闲计时经验（`AtomicU64` + select 轮询），可复用于
  会话 TTL 回收
- design.md §8 已保证「超时/取消/Drop 三路径统一回收子进程」——池化后复用同一机制

## 2. 目标与非目标

### 目标

1. **MCP 长驻进程内复用浏览器会话**：多次搜索共用同一浏览器进程/连接，消除每次 spawn 2-5s 开销
2. **空闲 TTL 回收**：会话空闲超过阈值自动 kill，防长驻进程残留浏览器（design.md §8 语义延续）
3. **并发上限与排队**：会话池有界（Semaphore），超限请求排队而非无限 spawn
4. **崩溃会话复活**：健康检查（命令失败 → 标记不健康 → 丢弃重建），浏览器进程崩溃不影响后续搜索

### 非目标（明确不做）

- **CLI 单次调用池化**：进程即用即走，池无收益；CLI 保持每次 spawn（行为不变）
- **跨进程会话复用**（`--connect` 连常驻浏览器，design.md §13 V2 规划）：本次只做进程内池
- **多实例 / 分布式池**：单机 CLI 工具（roadmap.md 非目标）
- **搜索间浏览器状态持久化**（cookie/缓存跨搜索保留）：不承诺，navigate 前语义等价冷启动
- **会话级验证码/登录态处理**：不绕过反爬（AGENTS.md 硬约束 6）

## 3. 设计

### 3.1 位置与依赖方向

新增 `src/drivers/pool.rs`（drivers 模块内部，天然依赖 `drivers::resolve` 与 `ports::BrowserDriver`，
无循环依赖）：

```
app.rs（run / run_with）        —— 面向 ports::BrowserDriver 编程
mcp.rs（SearchServer 持池）     —— 面向 app + drivers::pool 编程
drivers/pool.rs（SessionPool）  —— 面向 ports::BrowserDriver + drivers::resolve
```

- `BrowserDriver` trait **不修改**（协议差异仍封在 cdp/marionette 内部，ADR-002 不变）
- `app::run` 拆出内部 `run_with(&mut dyn BrowserDriver, config)`：公开 `run` 内部
  `resolve → run_with → drop`；MCP 从池 `acquire → run_with → 归还`，两者共用同一编排
- `drivers::resolve` 保持原签名；池内部调用它 spawn 新会话

### 3.2 SessionPool 数据结构

```rust
/// 会话池：空闲会话 LIFO 复用 + Semaphore 限并发 + TTL 后台回收。
pub struct SessionPool {
    kind: BrowserKind,              // 池内会话的浏览器类型（单一类型，不混池）
    idle: Mutex<Vec<IdleSession>>,  // 空闲会话（最近归还优先复用）
    semaphore: Arc<Semaphore>,      // 并发上限（含借出中）
    idle_ttl: Duration,             // 空闲回收阈值
    max_idle: usize,                // 空闲保留上限（防峰值后堆积）
}

struct IdleSession {
    driver: Box<dyn BrowserDriver>,
    returned_at: Instant,
}
```

### 3.3 借出/归还（SessionGuard）

```rust
pub struct SessionGuard {
    pool: Arc<SessionPool>,
    driver: Option<Box<dyn BrowserDriver>>,  // take 后为 None（已借出）
    healthy: bool,                            // 使用后健康标记
}
```

- **acquire()**：`semaphore.acquire().await`（排队）→ 弹空闲会话（LIFO）；无空闲 → `drivers::resolve` 新建
- **归还**：`SessionGuard` Drop 时——`healthy` 则 push 回 `idle`（记录 `returned_at`），
  否则直接 Drop `driver`（触发 kill 子进程）；同时释放 semaphore permit
- **健康检查**：`mark_unhealthy()` 由调用方在使用出错时调用。判定规则（app 层 `run_with`
  错误返回后由 mcp 判定）：
  - `Error::Network / Timeout` → 会话可能已损坏 → 标记不健康，重建
  - `Error::Captcha / Engine / Cli` → 浏览器本身健康（页面/参数问题）→ 可复用
- **崩溃复活**：浏览器进程被外部 kill 后，下一次命令返回 `Network` 错误 → 标记不健康 →
  Drop 重建，池自动恢复

### 3.4 空闲回收（TTL reaper）

后台任务（`tokio::spawn`，随池生命周期）：

```
每 min(ttl/2, 1s) 周期：
  锁定 idle → 移除 returned_at.elapsed() > idle_ttl 的会话（Drop 触发 kill）
```

- MCP 进程退出时：池 Drop → 所有借出/空闲会话 Drop → 浏览器进程全回收（design.md §8 兜底）
- reaper 与 mcp.rs 空闲超时（进程级退出）职责不同：reaper 管**会话级**空闲回收（进程保活），
  mcp 空闲超时管**进程级**退出（客户端断开），二者互补

### 3.5 搜索间状态隔离

每次搜索第一步 `driver.navigate(url)` 即覆盖页面状态（navigate 到新 URL 语义等价新页面），
**无需额外清理**；池只复用「进程 + 连接」，不承诺跨搜索保留页面数据。

### 3.6 接入点

| 入口 | 是否池化 | 说明 |
|---|---|---|
| CLI `worbrow search` | ❌ | 单次进程，保持现状 |
| MCP `web_search` | ✅ | `SearchServer` 持 `Arc<SessionPool>`，每次工具调用 acquire/归还 |
| MCP `mcp` 子命令 | — | 新增 `--max-sessions` / `--session-ttl` 参数（默认见 §6） |

## 4. 契约影响

- **输出 schema v1 无变化**（会话复用对 agent 透明；`meta` 不加字段，保持最小）
- **退出码语义冻结**（0/2/3/4/124/1）不变
- `BrowserDriver` trait 与 `drivers::resolve` 签名不变（池为新增内部模块，不属公开面，ADR-006）

## 5. 验证

| 层 | 手段 |
|---|---|
| 池单测 | fake 驱动注入：连续 acquire 复用同一实例（spawn 次数断言）；TTL 过期重建；`mark_unhealthy` 丢弃重建；Semaphore 排队（并发 acquire 总数 ≤ 上限）；空闲上限回收 |
| app 集成 | `run_with` 与 `run` 结果等价（同 config 同 fixture）；错误类型健康判定映射单测 |
| MCP 集成 | 两次 `web_search` 断言 spawn 次数 = 1（fake）；并发调用排队不崩 |
| 真机冒烟（CI 外） | 连续多次真实搜索断言复用同一进程（进程 pid 不变）；TTL 后 pid 变化 |

## 6. 开放决策

1. **默认 `--max-sessions`**：✅ 已定 **1**（MCP 单用户串行为主，省内存；并发需求再调）
2. **默认 `--session-ttl`**：✅ 已定 **60s**（与 mcp 默认空闲超时同量级；过长残留浏览器，过短复用失效）
3. **`max_idle` 默认**：✅ 已定 **4**（峰值并发后最多保留 4 个空闲会话，防堆积；实现为池内常量）
4. **健康判定粒度**：✅ 已定 **命令级错误驱动**（Network/Timeout 错误 → 标记不健康丢弃重建；
   不选 acquire 时主动 ping——每次借出多一次命令往返，成本不划算）
5. **CLI 是否暴露池参数**：✅ 已定 **不暴露**（CLI 单次不池化，行为不变）；如需
   `--keep-session` 类实验参数再评估
6. **ADR-007**：✅ 已记录（池化策略：默认值、回收语义、健康判定，见
   [adr/0007-mcp-session-pool.md](adr/0007-mcp-session-pool.md)）
