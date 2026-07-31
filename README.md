# rplay-search

Agent 搜索 CLI：驱动**本机 headless 浏览器**（Chrome/Edge 走 CDP，Firefox 走 Marionette，协议层自研）在通用搜索引擎上执行搜索，输出稳定 JSON 契约供 AI agent 以子进程方式调用。

架构设计与决策见 [design.md](design.md)。

## 快速开始

前置：系统装有 Chrome/Edge（≥ 109）或 Firefox（≥ 55）。

```bash
cargo run -- list                    # 列出可用引擎
cargo run -- doctor                  # 环境自检（骨架阶段：引擎/后端状态）
cargo run -- "rust 异步运行时" --json --engine duckduckgo
```

> 骨架阶段仅 `fake` 驱动可用（测试用）；`chrome`/`firefox` 后端为 V1 待实现桩，
> 运行时报 `not_implemented`（exit 1）。协议实现见 [ADR-002](docs/adr/0002-browser-driver-protocols.md)。

## 调用契约（agent 侧）

- **stdout** 仅输出 JSON（`--json`），日志全部走 stderr
- 退出码语义化：`0` 成功 / `2` 参数错 / `3` 环境错 / `4` 搜索失败 / `124` 超时 / `1` 内部错
- schema 版本化：顶层 `schema_version` 字段，字段只增不改
- 无交互、硬超时默认 20s

示例成功包：

```json
{
  "schema_version": 1,
  "query": "rust",
  "results": [{ "rank": 1, "title": "…", "url": "https://…", "snippet": "…" }],
  "meta": { "engine": "duckduckgo", "started_at": "…", "elapsed_ms": 1200,
            "result_count": 3, "low_yield": false, "captcha": false, "engine_error": null }
}
```

## 质量命令

未安装 `just`，统一入口为 `make`：

```bash
make check      # fmt + clippy(-D warnings) + test
make test       # cargo test（18 个单测/集成测试，CI 无需浏览器）
make deny       # cargo-deny 许可/漏洞检查
make machete    # 未使用依赖检查
make doctor     # 运行 search doctor
```

## 目录

```
src/
  cli.rs app.rs domain.rs error.rs ports.rs output.rs extract.rs
  drivers/   # jsonrpc(共用框架) · cdp(桩) · marionette(桩) · fake(测试)
  engines/   # duckduckgo（bing/baidu 见演进路线）
tests/       # 集成测试 + fixtures（离线 HTML golden）
```

依赖方向：`cli → app → domain/ports ← adapters(drivers/engines)`，禁止反向。

## License

MIT OR Apache-2.0（见 [LICENSE-MIT](LICENSE-MIT)；Apache-2.0 文本见 <https://www.apache.org/licenses/LICENSE-2.0>）。
