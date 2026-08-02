# CONTRIBUTING.md

## 环境要求

- Rust ≥ 1.97（edition 2024）
- 可选：`cargo-deny`、`cargo-machete`、`pre-commit`（未装则跳过对应步骤）

首次启用 pre-commit：

```bash
pre-commit install
pre-commit install --hook-type commit-msg   # Conventional Commits 校验
```

## 开发流程

1. 先读 `docs/design.md`（架构与契约）与 `AGENTS.md`（硬约束）
2. 改代码 → 本地验证（见下）→ 提交（Conventional Commits：`feat:` / `fix:` / `docs:` / `chore:`，
   **description 用英文**；历史提交不重写）
3. 变更契约（JSON schema / 退出码）必须 bump `schema_version` 并记 ADR（追加到 `docs/adr/`）
4. 变更**库公开面**（ADR-006：顶层 re-export 的类型/trait/常量）时：
   - 0.x 阶段破坏性变更（删/改名/字段私有化）须 bump minor 并记 CHANGELOG 迁移说明
   - 新公开类型/ trait 必须经顶层 re-export 评估（模块级新 `pub` 一律禁止，除非有
     bin/tests 需要并经 `resolve` 转发），并同步 `tests/lib_api.rs` 外部视角门禁

## 本地验证（提交前必跑）

```bash
pre-commit run --all-files   # 空白/YAML/TOML/密钥/冲突标记等（可选但推荐）
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity
cargo test
cargo deny check
cargo machete
```

`fmt`/`clippy`/`test`/`deny`/`machete` 不进 pre-commit（偏慢；由 Makefile 与 CI 承担）。

## 常见任务

- **新增搜索引擎**：复制 `src/engines/duckduckgo.rs` 模式 → 实现 `SearchProvider` → 注册到
  `engines/mod.rs` → 添加 `tests/fixtures/<engine>.html` 与解析单测
- **实现浏览器后端**：填充 `drivers/cdp.rs` 或 `drivers/marionette.rs`（复用 `drivers/jsonrpc.rs`），
  同步 `drivers/mod.rs::resolve` 与 `worbrow doctor` 输出
- **更新 fixture**：引擎 HTML 改版导致解析失败时，更新对应 fixture 并记录抓取日期

## 安全

安全问题不要走本流程，见 [SECURITY.md](SECURITY.md)。
