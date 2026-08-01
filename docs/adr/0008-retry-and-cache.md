# ADR-008：网络重试与结果缓存（`--retry` / MCP 短 TTL 缓存）

- 状态：已接受
- 日期：2026-08-01

## 背景

两个独立痛点（roadmap P1「网络重试与结果缓存」）：

1. **瞬时网络错误直接失败**：引擎/网络偶发抖动（连接重置、瞬时超时）时搜索直接
   `exit 4`，agent 需自行重试，CLI/MCP 均无内置重试
2. **MCP 长驻进程重复搜索**：相同 query 在 TTL 窗口内重复调用每次都重新驱动浏览器
   搜索，浪费时延与资源

## 决策

### 重试（app 层，CLI + MCP 共用）

- **触发范围 = 仅 `Error::Network`**（瞬时网络抖动）。验证码/参数错误/超时不重试
  （避免无意义放大延迟）；引擎解析失败（`Error::Engine`）已有降级链处理，不重试
- **指数退避封顶**：第 n 次重试延迟 = 2^(n-1) 秒（1s/2s/4s/8s 封顶），计入全局
  timeout 预算内（`tokio::time::timeout` 包裹整个重试循环，总耗时不超过 `--timeout`）
- **入口**：CLI `--retry <n>`（默认 0）；MCP `retry` 请求参数（默认 0，封顶 5）
- **`meta.retries`**：实际重试次数（schema v1 只增不改）

### 缓存（仅 MCP 长驻）

- **作用域**：仅 MCP 进程内生效（`SearchServer` 持 `SearchCache`）；CLI 单次无状态
  不缓存
- **LRU + TTL**：key 覆盖全部请求参数（query/engine/browser/max_results/lang/region/
  pages/freshness/safesearch/site/filetype）；TTL 60s（命中后刷新）；容量 128（LRU 淘汰）
- **逃生阀**：MCP `no_cache` 请求参数（默认 false）——需要新鲜结果时绕过（不读不写）
- **命中语义**：`meta.cached=true`，`started_at`/`elapsed_ms` 刷新为本次调用
  （elapsed_ms=0，agent 明确感知"未走搜索"）
- **`meta.cached`**：新增字段（schema v1 只增不改）；CLI 恒 false

## 后果

- **得到**：瞬时网络抖动自动恢复（无需 agent 重试）；MCP 高频重复 query 免搜索直接
  返回（省时延/省浏览器资源）；失败/命中语义对 agent 透明（meta 字段）
- **付出**：`SearchMeta` 新增 `cached`/`retries` 两字段（schema 只增不改）；重试放大
  延迟（指数退避封顶 8s + 全局 timeout 兜底）；缓存时效性（TTL 60s，短于实时搜索）
- **拒绝**：重试网络+解析错误（解析失败已有降级链，重试同引擎意义有限）；缓存扩展
  到 CLI/跨进程（CLI 无状态、跨进程需 HTTP 常驻，归 V3）；`--retry` 无限重试
  （封顶 + timeout 兜底）

## 验证

- app 单测：`backoff_delay` 指数序列封顶；瞬时失败重试成功（`meta.retries=1`）；
  重试耗尽返回 Network；验证码错误不重试
- MCP 缓存单测：命中（cached=true）/key 区分/TTL 过期/LRU 容量淘汰
- MCP 集成：相同 query 二次调用 `cached=true` 且 `elapsed_ms=0`；`no_cache` 绕过
