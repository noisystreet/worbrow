# 设计文档：Agent 搜索 CLI（headless-browser-search）

> 用 Rust 编写的命令行工具：启动 headless 浏览器，在通用搜索引擎上执行搜索，
> 以稳定的机器可读契约（JSON + 退出码）把结果返回给 **AI agent**。
> Agent 通过子进程方式调用，每次执行一次搜索任务。

---

## 1. 背景与目标

### 1.1 目标

- 提供一个 **CLI 工具**：`worbrow "rust async runtime" --engine bing --json`
- 驱动 **本机 headless 浏览器**（Chrome/Edge/Firefox，协议层自研）完成真实搜索，突破纯 HTTP 抓取被反爬拦截的局限
- 输出 **结构化的搜索结果**（标题 / URL / 摘要 / 排名），schema 版本化、跨版本稳定
- 输出契约面向 **agent 程序**：默认 `--json` 全量输出，日志绝不混入 stdout
- 可插拔的**搜索引擎适配器**（MVP：Bing、DuckDuckGo；后续：百度、Google 等）
- 内置**超时、验证码检测、失败定位**（调试截图），保证子进程调用不挂死

### 1.2 非目标

- 不是通用网页抓取/爬虫框架（不做深度爬取、调度、分布式）
- 不做站内搜索适配（v1 聚焦通用搜索引擎；站内搜索留给演进路线）
- 不做交互式 REPL、不做常驻服务（形态演进见 §13）
- 不做验证码自动破解 / 绕过（只检测并上报，由 agent 决策）
- 不保证绕过任何搜索引擎的反爬限制；反爬对抗不在本设计承诺范围内

---

## 2. 使用场景：agent 如何调用

```
$ worbrow "rust 异步运行时 对比" --engine bing --max-results 8 --timeout 60 --json
```

调用约定（对调用方是**硬契约**）：

| 约定 | 内容 |
|---|---|
| stdout | 仅输出 JSON（唯一例外：无 `--json` 时输出人读文本） |
| stderr | 所有日志 / 警告 / 调试信息（`--log-level`、`RUST_LOG`） |
| 退出码 | 语义化，见 §7.2；非 0 时 stdout 输出错误 JSON 包 |
| 无交互 | 不读 stdin、不提示输入、不等待按键 |
| 必达超时 | `--timeout` 默认 60s；超时返回 124，不留孤儿进程 |

Agent 侧典型用法：子进程执行 → 读 stdout → 按 `schema_version` 解析 → 检查退出码与
`meta` 中的 `captcha` / `engine_error` 字段 → 决定重试或换引擎。

---

## 3. 质量属性（按优先级排序）

| 优先级 | 属性 | 架构含义 |
|---|---|---|
| P0 | **契约稳定性** | JSON schema 版本化；退出码语义冻结；字段只增不删、新增可忽略 |
| P0 | **可靠性 / 不挂死** | 全流程硬超时；进程退出即回收浏览器（无常驻泄漏）；panic 兜底为错误码 |
| P0 | **可维护性** | 搜索引擎 HTML 变化频繁 → 解析逻辑集中在可替换的引擎适配器，配 golden 测试 |
| P1 | **可观测性** | stderr 结构化日志（tracing）；`--screenshot` 失败现场；`--log-level` 显示导航步骤 |
| P1 | **可测试性** | 浏览器驱动抽象出 trait，测试用 Fake Driver + 离线 HTML fixture，CI 无需浏览器 |
| P2 | **性能** | 单任务冷启动可接受（CLI 每次新建浏览器实例）；不做常驻池 |
| P2 | **合规安全** | 遵守 robots.txt 精神、控制频率、真实 UA；防 SSRF 不适用但限制目标域集合 |

冲突取舍说明：**契约稳定性与解析灵敏度**存在张力（引擎改版会破坏解析，但输出 schema 必须不变）——
解法是解析失败归为 `engine_error` 上报而非改 schema；**性能让位于可靠性**（宁可多 1s 等待页面
加载完成，不提前返回残缺结果）。

---

## 4. 关键架构决策（ADR）

ADR 以独立文件维护在 `docs/adr/`，本节省略为索引；新决策追加
`docs/adr/NNNN-标题.md`（格式参照 docs-style 模板）。

| ADR | 标题 | 状态 |
|---|---|---|
| [ADR-001](adr/0001-program-shape.md) | 程序形态 = 单任务 CLI | 已接受 |
| [ADR-002](adr/0002-browser-driver-protocols.md) | 浏览器驱动 = 自研双协议后端（CDP + Marionette） | 已接受 |
| [ADR-003](adr/0003-search-url-direct.md) | 搜索方式 = URL 直访优先，交互原语备用 | 已接受 |
| [ADR-004](adr/0004-output-contract-json.md) | 输出契约 = JSON schema v1 + 语义化退出码 | 已接受 |
| [ADR-005](adr/0005-mcp-stdio-server.md) | MCP stdio server 支持（`worbrow mcp`，rmcp 2.2） | 已接受 |
| [ADR-006](adr/0006-lib-api-surface.md) | 库 API 公开面 = 类型级顶层 re-export（外部消费者） | 已接受 |

---

## 5. 架构风格与模块边界

风格：**分层 + 轻量端口-适配器（ports & adapters）**。

理由：核心领域（"执行一次搜索"用例 + 结果模型）小而稳定；易变点全部在外围——浏览器
驱动、搜索引擎 HTML 解析。这正好是 ports-adapters 的适用面：领域稳定、适配器多变。
刻意不做 use-case 对象、Repository 等完整 DDD 样板（该程序业务体量不支撑，属于"样板多于业务"）。

### 5.1 依赖方向（唯一允许的依赖方向，禁止反向）

```
┌────────────────────────────────────────────────────────┐
│ cli  (src/cli.rs, src/main.rs)                          │
│   参数解析 · 日志初始化 · 退出码映射 · stdout 输出        │
├────────────────────────────────────────────────────────┤
│ app  (src/app.rs)                                       │
│   用例编排：解析 query → 选引擎 → 驱动浏览器 → 抽取 → 输出 │
├────────────────────────────────────────────────────────┤
│ domain (src/domain.rs, src/error.rs)                    │
│   SearchQuery · SearchResult · Error · 纯数据，零依赖    │
│ ports  (src/ports.rs)                                   │
│   SearchProvider trait · BrowserDriver trait             │
├───────────────▲────────────────────────────────────────┤
│               │ 实现 ports（依赖方向指向内层）            │
│ adapters                                              │
│   engines/bing.rs · engines/duckduckgo.rs               │
│   engines/baidu.rs · engines/registry.rs                │
│   drivers/cdp.rs · drivers/marionette.rs · drivers/fake.rs │
│   extract.rs（scraper 解析公共逻辑）                     │
└────────────────────────────────────────────────────────┘
```

> 注：已实现 `engines/duckduckgo.rs`、`drivers/marionette.rs`（Firefox）与 `drivers/fake.rs`；
> `engines/bing.rs`、`engines/baidu.rs` 与 `drivers/cdp.rs`（Chrome/Edge）为 V1 后续目标（见 §13）。

规则：
1. `domain` 不依赖任何框架/IO 细节（无 serde 之外的依赖）；
2. `app` 只面向 `ports` 中的 trait 编程，不感知 CDP / Marionette / scraper；
3. `adapters` 之间的同名 crate 使用限制在自身模块内，不跨适配器共享可变内部结构；
4. 引擎适配器只通过 `SearchResult` 这个 DTO 向外传数据，不允许泄漏内部 DOM 结构；
5. 循环依赖视为设计失败。

### 5.2 crate / 目录结构

```
Cargo.toml            # 单 crate：lib + bin
src/
  main.rs             # 薄入口 + CLI 参数解析（clap，bin 私有，ADR-006）
  lib.rs              # 公开面：顶层 re-export（Config/BrowserKind/...，ADR-006）
  app.rs              # 用例编排（见 §6.2）
  domain.rs           # SearchQuery / SearchResult
  error.rs            # Error 枚举 → 退出码映射
  ports.rs            # SearchProvider / BrowserDriver trait 定义
  output.rs           # 结果 JSON 序列化（schema v1）
  drivers/
    mod.rs            # 后端注册表：--browser chrome|firefox → Box<dyn BrowserDriver>
    jsonrpc.rs        # 共用的 JSON-RPC 消息框架（编解码、id↔响应匹配、事件路由）
    cdp.rs            # 自研 CDP 客户端（HTTP 发现端点 + WebSocket JSON-RPC）→ Chrome/Edge
    marionette.rs     # 自研 Marionette 客户端（WebSocket JSON-RPC）→ Firefox
    fake.rs           # 测试用 FakeDriver（返回 fixture HTML）
  engines/
    mod.rs            # 引擎注册表：name → Box<dyn SearchProvider>
    bing.rs
    duckduckgo.rs
    baidu.rs
tests/
  fixtures/           # 各引擎离线 HTML golden 文件
  integration.rs      # 走 FakeDriver 的端到端测试
```

单 crate、内部强模块边界（模块化单体）。不拆多 crate workspace：无独立编译/发布需求，
拆分只会增加认知负荷。

---

## 6. 模块设计

### 6.1 CLI 层（`cli.rs` / `main.rs`）

clap derive 定义参数（示意）：

| 参数 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `<query>` | string | 有子命令时省略 | 搜索词 |
| `--engine` | string | `bing` | 引擎名；逗号分隔 = 降级尝试顺序（如 `bing,duckduckgo`；可用：`worbrow list` 查看） |
| `--browser` | enum | `firefox` | 浏览器后端：`firefox`（Marionette，已实现）或 `chrome`（CDP，已实现） |
| `--max-results` | usize | 10 | 返回条数上限 |
| `--timeout` | secs | 60 | 全流程硬超时 |
| `--json` | flag | 否 | JSON 输出（agent 调用必带） |
| `--log-level` | enum | off | stderr 日志级别（error/warn/info/debug/trace） |
| `--screenshot <path>` | path | 无 | 失败或成功时保存页面截图（调试） |
| `--dump-html <path>` | path | 无 | 失败或 low_yield 时保存原始 HTML（调试） |
| `--connect <cdp-url>` | url | 无 | 连接已运行浏览器（V2 性能演进） |

子命令：`worbrow doctor`（环境自检，§10）、`worbrow list`（列出引擎）。

`main.rs` 职责：初始化 tracing（仅 stderr）→ 子命令分发 → `app::run` →
输出 JSON 包并映射退出码。任何 panic 由顶层 `catch_unwind` 兜底转成 `exit(1)` 并输出错误 JSON。

### 6.2 用例编排层（`app.rs`）

```
run(config) -> Outcome
 1. 解析并校验 query（长度、URL 注入防护）
 2. 引擎顺序解析：config.engine 逗号分隔 = 尝试链（如 "bing,duckduckgo"）
 3. driver_registry.resolve(config.browser) → Box<dyn BrowserDriver>（cdp 或 marionette）
 4. 包整体 timeout(→ 124)，内部为引擎降级循环：
    a. 按序 resolve 引擎 → search_one（5-8 步）
    b. 成功且非低产（≥3 条）→ 采用，停止
    c. 低产（<3 条）→ 保留为候选（最高产），继续下一引擎
    d. 验证码阻止（captcha 且无结果）或解析失败（EngineFailure）→ 继续下一引擎
    e. 全部尝试完：有候选 → 成功包（low_yield=true）；否则返回最后错误（captcha 优先）
 5. search_one：驱动 navigate(provider.result_url(query)) + 翻页聚合
 6. 轮询 wait_for_load（网络 idle 或结果选择器出现，带二级超时）
 7. provider.detect_captcha(html)? → 标记 captcha=true（不中止，见 §9）
 8. provider.parse(html) → Vec<SearchResult>（跨页去重合并、截断到 max_results）
 9. 可选 screenshot；关闭浏览器（Drop 保证：浏览器进程随本进程退出回收）
10. 组装 Outcome{results, meta（engine=最终引擎，engine_tried=尝试链）} → output 序列化
```

### 6.3 领域模型（`domain.rs`）

```rust
pub struct SearchQuery {
    pub text: String,
    pub max_results: usize,
    pub lang: Option<String>,   // 结果语言（如 zh-hans，Bing setlang）
    pub region: Option<String>, // 结果地域/市场（如 zh-CN，Bing mkt / DDG kl）
    pub pages: usize,           // 翻页聚合页数（≥1；1 = 仅首页）
}
```
#[derive(Serialize)]
pub struct SearchResult {
    pub rank: usize,
    pub title: String,
    pub url: String,          // 已解析为最终跳转目标（尽力去重/归一化）
    pub snippet: String,      // 摘要，可为空
    pub domain: String,       // URL host（来源域，供 agent 免解析判断可信度）
    pub https: bool,          // scheme 是否为 https
}

#[derive(Serialize)]
pub struct SearchMeta {
    pub engine: &'static str,
    pub started_at: DateTime<Utc>,
    pub elapsed_ms: u64,
    pub result_count: usize,
    pub low_yield: bool,                    // 结果数低于阈值（<3），提示 agent 结果不可靠
    pub captcha: bool,
    pub engine_error: Option<EngineError>,  // 解析/页结构异常时上报，不为空即结果不可信
    pub engine_tried: Vec<String>,          // 引擎降级尝试链（含最终采用者）
}
```

### 6.4 端口（`ports.rs`）

```rust
#[async_trait]
pub trait BrowserDriver: Send + Sync {
    async fn navigate(&mut self, url: Url) -> Result<()>;            // 导航 + 等待首屏
    async fn wait_for(&mut self, selector: &str, timeout: Duration)
        -> Result<()>;                                               // 结果元素出现
    async fn html(&self) -> Result<String>;
    async fn eval(&mut self, js: &str) -> Result<serde_json::Value>; // 备用：读结构化数据
    async fn screenshot(&mut self, path: &Path) -> Result<()>;
}

pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn result_url(&self, q: &SearchQuery) -> Url;      // URL 直访模板
    fn result_selector(&self) -> &'static str;         // 结果容器选择器（wait_for 用）
    fn parse(&self, html: &str) -> Result<Vec<SearchResult>>;   // HTML → DTO
    fn captcha_heuristics(&self) -> &[&'static str];   // 验证码特征词/选择器
}
```

- `SearchProvider::parse` 内部用 `scraper` 的 CSS 选择器抽取标题/链接/摘要；
- 链接归一化（去重、去跳转参数、协议补齐）收敛在 `extract.rs` 公共工具，供各引擎复用；
- `BrowserDriver` 是**唯一**的浏览器接触面，测试/CI 换 `FakeDriver`，生产按 `--browser`
  换 CDP 或 Marionette 实现；两个后端的协议差异全部封在各自文件内。

### 6.5 适配器

**drivers/cdp.rs**（Chrome/Edge，自研 CDP 客户端）：
- 启动：`chrome --headless=new --remote-debugging-port=<动态端口> --no-sandbox [--proxy-server=…]`
- 发现端点：`GET http://127.0.0.1:<port>/json/version` → `webSocketDebuggerUrl`
- 消息框架：tokio-tungstenite + JSON-RPC（`{id,method,params}` / 事件通道），tokio 超时轮询
- 命令子集：`Target.attachToTarget` / `Page.navigate` / `Runtime.evaluate`（取 HTML、轮询
  `document.readyState`、验证码判定）/ `Page.captureScreenshot`
- 协议命令与版本在模块内集中登记，`worbrow doctor` 做连通性验证

**drivers/marionette.rs**（Firefox）：
- 启动：`firefox -marionette -headless`（监听 127.0.0.1:2828）
- 消息框架：与 cdp.rs **共用**同一套 JSON-RPC 客户端
- 命令子集：`WebDriver:NewSession` / `WebDriver:Navigate` / `WebDriver:ExecuteScript`
  （等待与取 HTML 均走此命令）/ `WebDriver:TakeScreenshot`
- 等待加载：Marionette 无原生 load 事件，统一用 ExecuteScript 轮询 `document.readyState`，
  与 cdp.rs 在 trait 内对齐为同一语义

**drivers/fake.rs**：读 `tests/fixtures/<engine>.html` 返回固定 HTML，CI 无浏览器也能跑端到端。

两个后端共用的 JSON-RPC 消息框架（约 100 行：消息编解码、id↔响应匹配、事件路由）收敛在
`drivers/jsonrpc.rs`，协议差异仅剩命令参数与端点发现。

**engines/**：每个引擎两个关注点——URL 模板 + 解析选择器。引擎 HTML 改版是常态，
所以：解析失败 → `engine_error` 上报（不改 schema），并作为 P1 告警被观测；选择器版本
信息（如 `data-selector-rev`）随代码注释记录。

**engines/registry.rs**：`fn resolve(name: &str) -> Result<Box<dyn SearchProvider>>`，
`--engine list` 复用同一注册表。新增引擎 = 新增一个文件 + 注册一行。

---

## 7. 输出契约细节

### 7.1 JSON schema v1（`--json`，stdout）

成功：

```json
{
  "schema_version": 1,
  "query": "rust 异步运行时 对比",
  "results": [
    { "rank": 1, "title": "…", "url": "https://…", "snippet": "…",
      "domain": "example.com", "https": true }
  ],
  "meta": {
    "engine": "bing",
    "started_at": "2026-07-31T08:00:00Z",
    "elapsed_ms": 1842,
    "result_count": 8,
    "pages": 2,
    "low_yield": false,
    "captcha": false,
    "engine_error": null,
    "engine_tried": ["bing"]
  }
}
```

失败（非 0 退出码时同样输出到 stdout，供 agent 结构化处理）：

```json
{ "schema_version": 1, "error": { "code": "timeout", "message": "…", "detail": "…" } }
```

版本策略：`schema_version` 主版本号，字段**只增不改**；破坏性变更 bump 主版本（agent 端
显式校验并告警）。新增字段对旧 agent 无感。

### 7.2 退出码

| 码 | 含义 | agent 端建议动作 |
|---|---|---|
| 0 | 成功（含 captcha=true 但出了部分结果） | 正常解析 |
| 2 | 参数错误（未知引擎 / query 为空等） | 修正调用，不重试 |
| 3 | 环境错误（浏览器未安装 / CDP 启动失败） | 检查环境，`worbrow doctor` 自检 |
| 4 | 搜索失败（网络 / 解析 / 验证码阻止） | 可换引擎重试（读 `error.detail`） |
| 124 | 超时（对齐 GNU timeout 语义） | 可选重试 |
| 1 | 未知内部错误 / panic | 上报，不重试 |

---

## 8. 错误处理、超时与重试

- **硬超时**：`--timeout` 用 `tokio::time::timeout` 包整个用例；超时 → 关闭浏览器 → exit 124。
  进程级兜底：调用方（agent）可再套一层 OS 级 timeout kill，两者语义一致（124）。
- **资源回收（超时/取消/Drop 三路径统一）**：浏览器为子进程；`app::run` 将 driver
  **移入超时闭包**——超时取消、错误提前返回或显式 abort 时闭包 drop → driver drop →
  杀浏览器子进程（CDP 另加 `wait()` 收割防 zombie；Marionette 由 tokio reaper 收割）；
  成功路径 driver 归还，随函数返回 Drop。无常驻句柄、无残留进程。
- **重试策略**：工具自身不做静默重试（CLI 无状态，重试应由 agent 决策）；`--retry <n>`
  （瞬时网络错误重试）整体归 V2（见 §13），V1 不含；V1 已支持引擎降级链（§6.2，验证码/
  解析失败/低产自动尝试下一引擎）。
- **错误分类**：`Error` 枚举区分 `Cli / Env / Network / Parse(engine) / Captcha / Timeout`，
  与退出码一一映射（§7.2）。

---

## 9. 反爬、验证码与合规

- **验证码检测**：启发式（页面含常见验证码特征词/选择器）。检测到 → `meta.captcha=true`，
  结果若为空 → exit 4 + `error.code="captcha"`。**不做自动破解**，由 agent 换引擎/降频/人工介入。
- **频率纪律**：工具默认单次任务；多任务频率由调用方自律。文档明示建议间隔（≥2s/请求）。
- **UA 与指纹**：设置与真实 Chrome/Firefox 一致的用户代理；自研客户端**不注入** automation 标记
  （CDP 侧由我们控制启动参数，Firefox 侧同理），尽量贴近真人，同时 README 说明这是灰色地带，
  目标站点风控可能仍拦截。
- **目标域白名单**：仅允许访问已注册引擎的域名（防 SSRF 面）；重定向链中若离开引擎域，
  记录并截断（v1 行为：记录 + 保留重定向目标 URL 本身）。
- **合规提醒**：遵守目标引擎 ToS 与 robots.txt；仅抓取摘要（snippet），不盗用整页正文；
  本工具定位是"搜索辅助"，不是规避风控的爬虫。此提醒写入 README 与 `--help` 中 `--engine list` 说明。

---

## 10. 运行约束与安全细节

### 10.1 并发与资源

- 每个实例 = 1 个浏览器进程（Chrome 约 200MB、Firefox 略低）；agent 并发调用时内存线性增长，
  文档明示建议并发上限（如 ≤4），并建议调用方自行限流
- **CDP 端口**：用动态端口（`--remote-debugging-port=0`，从 `/json/version` 读回实际端口），
  规避并发实例端口冲突
- **Marionette 端口**：默认固定 2828，多实例会冲突 → 每实例使用独立临时 profile
  （`-profile <temp>` + `user.js` 写入随机 `marionette.port`）；若实现受阻则退化为调用方串行化
- 进程回收：Drop + 进程退出即回收（见 §8）；并发下由 `worbrow doctor` 检查无残留进程

### 10.2 浏览器版本矩阵与发现

- **兼容矩阵**（登记在 `drivers/` 内，`worbrow doctor` 对照检查）：
  Chrome/Edge ≥ 109（`--headless=new`）；Firefox ≥ 55（`-marionette`）
- **发现顺序**：`CHROME_PATH` / `FIREFOX_PATH` 环境变量 → PATH 搜索（`google-chrome` /
  `chromium` / `firefox` 等）→ 平台默认位置（Windows 注册表 / macOS `/Applications` / Linux 常见路径）
- 版本不符时：`worbrow doctor` 给出明确安装指引；运行时报 `error.code="env"`（exit 3）

### 10.3 安全细节

- 搜索结果 URL 来自第三方，工具**只输出不访问**；README 提醒 agent 侧勿自动 follow 结果链接
  （防钓鱼/恶意跳转）
- 标题/摘要清洗：HTML 实体反转义 + 剥离控制字符，防注入与乱码（收敛在 `extract.rs`）
- 临时目录：profile 与截图写入 `temp_dir()/worbrow-<pid>/`，退出清理（见 §8）
- 目标域白名单与合规边界见 §9

### 10.4 输出信号增强

- `meta.low_yield`：结果数 < 3 时置 `true`（schema v1 新增字段，遵守"只增不改"），
  agent 可据此判断结果不可靠
- `--dump-html <path>`：失败或 low_yield 时保存原始 HTML，供离线诊断与更新 fixture
- `--log-format json`（可选）：stderr 输出结构化日志，便于 agent 采集

---

## 11. 可观测性与调试

- tracing 输出到 **stderr**：`--log-level` 时打印 `navigate → wait_for → parse` 各步骤耗时；
- 失败现场：`--screenshot <path>` 保存捕获时的页面截图（验证码、空白结果页均有用）；
- `worbrow doctor` 子命令：检查浏览器二进制、CDP 连通性、引擎注册表健康（各引擎跑一次
  离线 fixture 解析），环境类问题定位从"试一次"变成"查一次"。

---

## 12. 测试策略

| 层 | 手段 | 环境要求 |
|---|---|---|
| 解析单元测试 | 引擎 `parse(fixture_html)` 断言结果字段 | 无浏览器 |
| 端到端（集成） | FakeDriver + fixture → app::run → 校验 JSON/退出码 | 无浏览器 |
| golden 回归 | `tests/fixtures/<engine>.html` 提交入库，解析输出快照对比 | 无浏览器 |
| 真机冒烟（可选，CI 外） | 真实 Chrome 与 Firefox 各跑一次 Bing/DDG，人工/脚本核对 | 本机浏览器 |

fixture 更新纪律：引擎改版导致解析失败时，`engine_error` 上报 + 人更新 fixture（记录抓取日期）。
CI 不依赖真实浏览器，保证可复现。

---

## 13. 演进路线

- **V1（MVP，已完成，v0.1.0）**：DuckDuckGo/Bing 引擎 + Marionette 后端（Firefox）
  与 CDP 后端（Chrome/Edge）均已完成；`--json`/超时/验证码检测/截图/`worbrow doctor`
  已就绪；MCP stdio server（`worbrow mcp`，rmcp 2.2，见 ADR-005）已完成；库公开面
  收敛为类型级顶层 API（ADR-006），可作为库供外部消费
- **V2**：百度、Google（预期高拦截，降级为"尽力"）；`--connect` 连接常驻浏览器复用会话；
  结果去重归一化加强；新增 `--retry`（瞬时网络错误重试）；若需网络拦截等深度控制，
  引入 chromiumoxide 作第二 CDP 实现
- **V3（待需求驱动）**：站内搜索适配（届时扩展 `BrowserDriver` trait 增加交互原语，
  见 ADR-003）；若出现多语言/非 Rust 消费者且会话复用成为刚需 → 包 HTTP 常驻服务
  （届时新记 ADR，扩展而非重写：内核是同一 lib）

---

## 14. 风险与开放问题

| 风险 | 影响 | 缓解 |
|---|---|---|
| 搜索引擎 HTML 频繁改版 | 解析失败、结果空洞 | 适配器集中 + golden 测试 + `engine_error` 上报不破坏契约 |
| 风控升级（验证码/封禁 IP） | 搜索不可用 | 多引擎冗余、频率纪律、诚实上报 captcha |
| 浏览器协议演进（CDP/Marionette 改版） | 驱动失效 | 命令子集集中登记 + `worbrow doctor` 连通性自检 + fixture 冒烟 |
| 自研维护成本 | 开发/排障时间上升 | 功能面窄（仅搜索），协议命令少；两个后端共用 JSON-RPC 框架 |
| 头less 指纹被识别 | 引擎返回异常结果 | 真实 UA、文档化限制、保留 headful 调试模式 |
| 合规争议 | 目标站 ToS 纠纷 | 只取摘要、限目标域、README 明示边界 |

开放问题（实现前确认）：
1. 二进制命名（已定为 `worbrow`）；
2. 是否需要在 V1 就提供 `--proxy` 支持（影响 ADR-002 两个后端的启动参数面，成本低，倾向纳入）；
3. DuckDuckGo 的 lite/html 版（HTML-only 端点，解析更稳定）是否作为默认端点。

---

## 15. 参考

- [Chrome DevTools Protocol](https://chromedevtools.github.io/devtools-protocol/) — CDP 协议参考
- [Marionette Protocol (MDN)](https://firefox-source-docs.mozilla.org/testing/marionette/) — Firefox 自动化协议参考
- [chromiumoxide (crates.io)](https://crates.io/crates/chromiumoxide) — 备用 CDP 实现（深度控制时引入）
- [fantoccini (crates.io)](https://crates.io/crates/fantoccini) — WebDriver 客户端（Firefox 对比参照）
- [rpage (docs.rs)](https://docs.rs/crate/rpage/1.0.0) — 2026 新项目，DrissionPage 风格（观望）
- [oxibrowser (crates.io)](https://crates.io/crates/oxibrowser-cdp/0.17.0) — 纯 Rust 浏览器引擎，AI-agent 向（观望）
