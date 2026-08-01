# worbrow 库 API 规划（面向外部消费者）

> 读者：项目维护者 / 贡献者。状态：**规划草案**，随实施更新；落地决策按 AGENTS.md 记 ADR。
> 本文聚焦「作为库提供给外部使用」，架构权威见 [design.md](design.md)，公开面决策见 [ADR-006](adr/0006-lib-api-surface.md)。

## 1. 背景与现状

worbrow 目前是"CLI 为主、库为辅"的形态：`lib.rs` 公开 `app`/`cli`/`drivers`/`engines`/
`error`/`mcp`/`output`/`ports` 八个模块，**公开面是按模块粒度**为 bin 与集成测试服务的，
并非为外部库消费者设计。

作为库消费已具备的骨架（P0–P2 重构产物）：

- `app::Config::new` + builder（`with_max_results`/`with_timeout`/`with_screenshot`/`with_dump_html`/`with_driver`）
- `app::search`（同步入口，内部管 runtime）/ `app::run`（async，复用外部 runtime）
- `app::DoctorReport::collect()` 环境自检
- `output::success/failure` 契约序列化、`error::Error`（含 `exit_code`/`code_str`/`detail`）
- `domain::DEFAULT_*` 常量、`BrowserKind::from_arg`

### 现状障碍（详见 [ADR-006](adr/0006-lib-api-surface.md)）

1. `EngineError` 未 re-export：`SearchMeta.engine_error` 字段类型外部**无法命名**
2. `Outcome.results` 用 `crate::domain::SearchResult` 内部路径
3. 公开面过宽：`cli`（clap 参数）、`drivers::{cdp, marionette, jsonrpc, discovery, fake}`、
   `engines::{bing, duckduckgo}` 等实现细节全部 pub，把"多变点"固化为稳定契约
4. `BrowserKind` 挂在 `drivers` 下（它是配置参数，不是驱动实现）
5. 外部自定义引擎无法注册（`engines::resolve` 固定 match，无扩展点）
6. `default = ["mcp"]` 使库消费者被强制拉入 rmcp 依赖树
7. `Error` 无 `#[source]`，底层错误被字符串化
8. 输出契约为函数拼 String，无类型化契约包（`schema_version` 不是一等公民）

## 2. 目标与非目标

### 目标

1. 外部 Rust 消费者 **3 行内**完成一次搜索（顶层 re-export，无需跨模块拼装）
2. 公开面**类型级稳定**：稳定的是 `Config`/`Outcome`/`SearchResult`/`SearchMeta`/
   `Error`/trait 与 `DEFAULT_*`，不是模块树
3. 适配器多变点对外不可见：新增驱动/引擎不破坏公开面
4. 外部可插拔：自定义引擎/驱动能接入 `app` 编排，不必复制 `run` 逻辑
5. 库消费可选功能按需启用（`default-features = false` 即最小依赖）

### 非目标（明确不做）

- **拆多 crate workspace**（lib + bin 分 crate）：单 crate 模块化单体仍合理，拆分增加
  发布与依赖编排复杂度（见 [ADR-006](adr/0006-lib-api-surface.md) 备选）
- **暴露驱动/引擎实现细节**给外部扩展（jsonrpc 协议框架、具体引擎解析器保持内部）
- **HTTP 常驻服务**等新形态（留待 design.md §13 V3，按需求单独评估）
- **为 0.x 提供完整 semver 承诺**：公开面冻结后 0.1 正式发布，0.x 破坏变更须 bump
  minor + CHANGELOG

## 3. 方向与优先级

### P0：公开面补漏（小改动，先行）

| 项 | 内容 |
|---|---|
| `EngineError` re-export | `lib.rs` 补 `pub use domain::EngineError`，解决 `SearchMeta.engine_error` 类型不可命名 |
| `Outcome` 字段路径 | `results: Vec<crate::domain::SearchResult>` → re-export 路径 |
| 验证 | `cargo doc` 无警告；新增"外部视角"集成测试（`tests/lib_api.rs`，仅用顶层/公开类型编译） |

### P1：公开面收敛 + 顶层 re-export（核心）

| 项 | 内容 |
|---|---|
| 顶层 re-export | `worbrow::{Config, BrowserKind, Outcome, DoctorReport, Error, run, search, DEFAULT_*, SearchQuery, SearchResult, SearchMeta, EngineError}` |
| `cli` 摘出到 bin | `main.rs` 声明 `mod cli;`（bin 侧私有），lib 删除 `pub mod cli`——bin 是独立 crate，`pub(crate)` 对 bin 不可见（实施中验证）；clap 依赖留 bin |
| 适配器内部化 | `drivers::{cdp, marionette, jsonrpc, discovery, fake}`、`engines::{bing, duckduckgo}` 内部化；真机冒烟测试改经 `drivers::resolve`（trait 面断言）；保留 `drivers::resolve`、`engines::resolve/AVAILABLE` 为内部服务公开面 |
| `BrowserKind` 归位 | 从 `drivers` 上移为 `domain` 纯配置枚举（零依赖），顶层 re-export |
| 引擎扩展点 | `Config::with_provider(Box<dyn SearchProvider>)`（对齐 `with_driver` 风格），`app::run` 注入优先、注册表兜底 |
| `Error` source chain | 复核确认（thiserror `#[from]` 已自动 source）；rustdoc 说明 |

### P2：库消费体验完善 —— ✅ 已完成（2026-08）

| 项 | 内容 |
|---|---|
| 类型化契约包 | ✅ `output::SuccessPayload`/`ErrorPayload`（`Serialize`，含 `schema_version`）顶层 re-export，消费者直接 `serde_json` |
| `Config` 字段私有化 | ✅ 字段私有，builder 唯一入口（`with_max_results` clamp 等不变量不可绕过；integration.rs 改用 builder） |
| 入口收敛 | ✅ `run_sync` 已移除，同步入口统一为顶层 `search`；async 用 `run` |
| trait 顶层 re-export | ✅ `worbrow::{SearchProvider, BrowserDriver}` 进顶层 |
| rustdoc 示例 | ✅ lib.rs quickstart doc-test（fake 可执行）+ `examples/basic_search.rs`、`examples/custom_engine.rs`（根目录） |
| feature 文档化 | ✅ README"作为库使用"：`default-features = false` 去 MCP 依赖说明 |
| 文档互链 | ✅ `EngineError`（meta 上报）与 `EngineFailure`（CLI 错误）rustdoc 互链 |
| 版本语义 | ✅ CONTRIBUTING 明确 0.x 公开面冻结：破坏性变更 bump minor + CHANGELOG，新公开项须经顶层 re-export 评估 |

## 4. 目标 API 形态（已达成）

```rust
// 外部消费者（Cargo.toml: worbrow = { version = "0.1", default-features = false }）
use worbrow::{BrowserKind, Config, search};

fn main() -> Result<(), worbrow::Error> {
    let outcome = search(Config::new("rust async", "bing", BrowserKind::Firefox)
        .with_max_results(5))?;
    for r in &outcome.results {
        println!("{} - {}", r.rank, r.title);
    }
    Ok(())
}
```

自定义引擎（无需复制 `run` 编排）：

```rust
struct MyEngine; // impl worbrow::SearchProvider
let config = Config::new("q", "myengine", BrowserKind::Firefox)
    .with_provider(Box::new(MyEngine));
let outcome = worbrow::search(config)?;
```

## 5. 实施顺序

P0 补漏 → P1 收敛 + re-export + 引擎扩展点 → P2 体验完善

- 每步独立可验证、可回退；同步 `cargo doc` 检查、README、CHANGELOG
- `tests/lib_api.rs`（外部视角集成测试）作为公开面回归门禁，随 P0 建立
- 不承诺时间；公开面取舍以 [ADR-006](adr/0006-lib-api-surface.md) 为准，变更须记新 ADR

## 6. 契约与约束（贯穿全程）

- 输出契约 `schema_version` 只增不改（[AGENTS.md](../AGENTS.md) 硬约束 3），本规划不触碰
- 退出码语义冻结、stdout 仅 JSON（CLI 形态不变）
- 依赖方向不变：`cli → app → domain/ports ← adapters`；`pub(crate)` 化不改变分层
- 安全红线：不写密钥、不自动访问第三方 URL

## 7. 开放决策

1. ~~`BrowserKind` 上移后 `drivers::BrowserKind` 保留 pub 别名还是彻底迁移~~ → ✅ P1 已实施：彻底迁移，顶层 re-export
2. `engines::resolve` 是否同时保留（内置引擎走注册表）与 `with_provider` 并存 →
   倾向并存：内置走注册表，外部走注入，二者在 `run` 内合并解析（P1 已实施并存）
3. `cli` 模块 `pub(crate)` 后，`clap` 依赖是否 feature 化（`cli = ["dep:clap"]`）→
   倾向 P2 做，P1 仅收敛可见性（P1 已实施：cli 摘出 bin，clap 留 bin）
