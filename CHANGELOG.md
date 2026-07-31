# Changelog

本文件记录用户可见变更（Keep a Changelog 风格）。

## [Unreleased]

### Added

- 项目骨架（design.md §5.2 目录结构）：domain/ports/app/cli/output 分层
- 自研 JSON-RPC 消息框架（`drivers/jsonrpc.rs`，CDP 与 Marionette 后端共用）
- DuckDuckGo 引擎适配器（html 端点，URL 直访 + scraper 解析）
- `fake` 浏览器后端 + 离线 HTML fixture，CI 无需真实浏览器
- CLI：`search "<query>"`（`--engine/--browser/--max-results/--timeout/--json/--log-level/--screenshot/--dump-html`）、
  `search list`、`search doctor`
- 输出契约 schema v1（成功包 + 错误包、语义化退出码）
- 质量基线：cargo fmt/clippy(-D warnings)/test/deny/machete、CI workflow、pre-commit
