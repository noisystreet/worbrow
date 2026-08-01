## 变更说明

<!-- 描述这次改动的内容与动机（1-2 段）。 -->

## 关联 issue

<!-- 如有关联 issue 请填写 `#123`；无则留空。 -->

## 检查清单

- [ ] `cargo fmt --check` 通过
- [ ] `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity` 通过
- [ ] `cargo test` 通过
- [ ] `cargo machete` 通过（`cargo deny check` 受本机网络影响时可跳过，CI 会跑）
- [ ] 契约变更（JSON schema / 退出码）已 bump `schema_version` 并记 ADR（见 `docs/adr/`）
- [ ] 用户可见变更已更新 `README.md` / `CHANGELOG.md`
