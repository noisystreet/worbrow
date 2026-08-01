# ADR-006：库 API 公开面（面向外部消费者）

- 状态：已接受
- 日期：2026-08-01

## 背景

worbrow 目前是"CLI 为主、库为辅"的形态：`lib.rs` 公开 `app`/`cli`/`drivers`/`engines`/
`error`/`mcp`/`output`/`ports` 八个模块。该公开面**按模块粒度**服务 bin 与集成测试
（同 crate 内 pub 即全暴露），并非为外部库消费者设计。已有外部消费骨架
（`app::Config` builder、`search`/`run`、`DoctorReport`、`output::*`），但存在
类型级缺陷与设计缺口：

1. `EngineError` 未 re-export：`SearchMeta.engine_error: Option<EngineError>` 的字段
   类型位于 `pub(crate) domain`，外部消费者能读字段但**无法命名该类型**
2. `Outcome.results` 使用 `crate::domain::SearchResult` 内部路径
3. 公开面过宽：`cli`（clap 参数结构）、`drivers::{cdp, marionette, jsonrpc,
   discovery, fake}`、`engines::{bing, duckduckgo}` 等适配器实现细节全部 pub。
   ports-adapters 的初衷是"领域稳定、适配器多变"——把多变点 pub 出去等于把它们
   固化为稳定契约，违背 ADR-002 的架构意图
4. `BrowserKind`（配置参数）挂在 `drivers`（实现层）下，概念归属错误
5. 外部自定义引擎无法接入 `app` 编排：`engines::resolve` 是固定 match，消费者实现
   `SearchProvider` 后注册不进去，只能复制 `run` 的编排逻辑
6. `default = ["mcp"]` 使库消费者被强制拉入 rmcp（含 schemars）依赖树
7. `Error` 无 `#[source]`，底层错误（如 `EngineFailure`）被字符串化，无法下钻

## 决策

将库公开面从**模块级**收敛为**类型级**顶层 API，作为 0.x 冻结前的目标形态：

- **顶层 re-export**：`lib.rs` 提供 `worbrow::{Config, BrowserKind, Outcome,
  DoctorReport, Error, search, run, DEFAULT_*, SearchQuery, SearchResult,
  SearchMeta, EngineError}`；消费者一行 `use worbrow::...` 完成拼装，无需感知模块树
- **`cli` 摘出到 bin**：CLI 参数解析（clap）从 lib 移除，`src/main.rs` 声明
  `mod cli;`（bin 侧私有）；lib 删除 `pub mod cli`。理由：bin 是独立 crate，
  `pub(crate)` 对 bin 不可见（实施中验证），CLI 解析本属 CLI 关注点；lib 只暴露
  `Config`/`search` 等库入口
- **适配器内部化**：`drivers::{cdp, marionette, jsonrpc, discovery, fake}`、
  `engines::{bing, duckduckgo}`、`extract` 改为内部；保留 `drivers::resolve`、
  `engines::resolve`/`AVAILABLE` 作为内部服务公开面（bin/tests 使用），**不进**
  对外稳定承诺；真机冒烟测试改经 `drivers::resolve`（trait 面行为断言，具体
  构造器细节由各驱动模块内部单测覆盖）
- **`BrowserKind` 归位**：从 `drivers` 上移为 `domain` 纯配置枚举（零依赖），
  顶层 re-export；`drivers` 仅保留驱动实现（`resolve` 内部引用）
- **引擎扩展点**：`Config::with_provider(Box<dyn SearchProvider>)`（对齐 `with_driver`
  风格）；`app::run` 内：注入 provider 优先，未注入走 `engines::resolve` 注册表，
  二者并存
- **`Error` source chain（复核确认）**：thiserror `#[from]` 已为 `Error::Engine`
  自动提供 source（`EngineFailure` 实现 `Error`）；实施为验证该行为 + rustdoc
  说明，除非复核发现缺口
- **类型化契约包**：`output::SuccessPacket`/`FailurePacket`（`Serialize`，含
  `schema_version`）；CLI 仅渲染，库消费者直接 `serde_json::to_string`
- **feature 语义**：`default = ["mcp"]` 保留（服务 CLI 二进制）；文档明示库消费用
  `default-features = false`；`clap`/`tempfile` 等 CLI 专属依赖按需 feature 化
  （P2 渐进，不在本 ADR 一步完成）

## 后果

- **得到**：稳定的是"类型面"而非"模块树"——新增驱动/引擎不改公开面；外部引擎/驱动
  可插拔；消费方依赖面小（`default-features = false` 不拉 rmcp）；`EngineError` 等
  缺陷消除，公开类型完备可命名
- **付出**：`tests/` 与文档需同步模块可见性变化（集成测试改经顶层或 `resolve`
  入口，真机冒烟改经 `resolve`）；`cli` 摘出后 clap 依赖留在 bin crate（feature
  化推迟到 P2）；`Config` 新增 `provider` 字段需同步 tests 字面量构造改为 builder
- **后续约束**：任何新公开类型/ trait 必须先经顶层 re-export 评估；模块级新 pub
  一律禁止（除非有 bin/tests 需要并经 `resolve` 转发）；公开面变更须记 ADR + CHANGELOG
- **迁移**：`tests/lib_api.rs`（外部视角集成测试）建立为公开面回归门禁；README 增加
  "库用法"章节（quickstart + `default-features = false` 说明）

## 备选方案

- **维持模块级公开（现状）**：零改动，但适配器多变点被固化、外部引擎不可插拔、
  类型缺陷遗留；否决——与 ports-adapters 意图矛盾
- **拆多 crate workspace（`worbrow` lib + `worbrow-cli` bin）**：可见性天然收敛，
  但增加发布编排、feature 传播、本地开发认知负荷；单 crate 模块化单体（design.md
  §5.2）仍合理；否决——成本大于收益，若未来 bin 形态膨胀再评估
- **只补漏不收敛**（re-export `EngineError` + 修字段路径）：修复类型缺陷但公开面
  仍过宽、引擎仍不可插拔；否决——未解决本 ADR 的核心问题，P1 收敛价值最大
