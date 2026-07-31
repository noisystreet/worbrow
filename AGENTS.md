# AGENTS.md

面向在该仓库工作的 Agent（人也可读）的硬约束与验证入口。

## 项目身份

- Rust（edition 2024，MSRV 1.85）CLI 工具，二进制名 `search`，库名 `rplay_search`
- 为 AI agent 提供"搜索引擎搜索"能力：驱动本机 headless 浏览器，输出稳定 JSON 契约
- 架构权威来源：`docs/design.md`（§5 分层、§6 模块、§7 契约、ADR 章节）

## 硬约束（违反即回退）

1. **依赖方向**：`cli → app → domain/ports ← adapters(drivers/engines)`。`domain` 零框架依赖；
   `app` 只面向 `ports` 的 trait 编程；禁止反向/循环依赖。
2. **浏览器协议自研**：CDP（Chrome/Edge）与 Marionette（Firefox）必须手写实现，**禁止引入
   chromiumoxide / fantoccini / playwright**。协议命令集中在各自驱动文件 + `drivers/jsonrpc.rs`
   共用 JSON-RPC 框架。唯一例外：V2 深度控制时按 `docs/design.md` 重新评估。
3. **输出契约**：stdout 仅 JSON（成功包/失败包），`schema_version` 字段**只增不改**；
   日志一律 stderr。破坏契约需 bump schema 主版本并记 ADR。
4. **引擎适配器**：新增引擎 = 新文件 + `engines/mod.rs` 注册一行；解析失败走
   `EngineFailure`（`engine_error`/exit 4），禁止改 schema 兜底。
5. **退出码语义冻结**（0/2/3/4/124/1），见 `src/error.rs::exit_code`。
6. 安全红线：不写密钥、不绕过权限、不自动访问搜索结果中的第三方 URL（只输出）。

## 修改架构文档

- `docs/design.md` 的重大结构变更（分层、ADR、契约）需用户确认后再改；小修正（笔误、编号）可直接改。
- 新决策记录为 ADR：追加到 `docs/adr/NNNN-标题.md`（`docs/design.md` §4 为索引表，格式参照 docs-style 模板）。

## 必跑验证命令

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo deny check
cargo machete
```

CI 与本地使用同一套检查（见 `.github/workflows/ci.yml`、Makefile）。
