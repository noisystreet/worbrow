# worbrow

[![CI](https://github.com/noisystreet/worbrow/actions/workflows/ci.yml/badge.svg)](https://github.com/noisystreet/worbrow/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/worbrow.svg)](https://crates.io/crates/worbrow)
[![Rust](https://img.shields.io/badge/rust-1.97+-orange.svg)](https://github.com/noisystreet/worbrow)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![MSRV](https://img.shields.io/badge/MSRV-1.97-blue)](https://github.com/noisystreet/worbrow)

Agent 搜索 CLI：驱动**本机 headless 浏览器**（Chrome/Edge 走 CDP，Firefox 走 Marionette，协议层自研）在通用搜索引擎上执行搜索，输出稳定 JSON 契约供 AI agent 以子进程方式调用。

架构设计与决策见 [docs/design.md](docs/design.md)；功能路线见 [docs/roadmap.md](docs/roadmap.md)。

## 快速开始

前置：系统装有 Chrome/Edge（≥ 109）或 Firefox（≥ 55）。

```bash
cargo run -- list                    # 列出可用引擎
cargo run -- doctor                  # 环境自检（浏览器二进制/引擎/后端状态）
cargo run -- "rust 异步运行时" --json   # 默认引擎 bing、默认超时 60s
cargo run -- "rust" --engine duckduckgo --timeout 30 --max-results 5
cargo run -- "rust" --pages 2 --max-results 15 --lang zh-hans --region zh-CN   # 翻页聚合 + 语言/地域
cargo run -- "rust" --freshness week --safesearch strict                       # 时间过滤 + 安全搜索
cargo run -- "rust" --site doc.rust-lang.org --filetype pdf                    # 站点/文件类型过滤
cargo run -- "rust" --engine bing,duckduckgo   # 引擎降级链（验证码/低产时自动尝试下一个）
```

当前后端状态：`firefox`（Marionette，自研协议）与 `chrome`（CDP，自研协议）均已实现；
`fake` 供测试/冒烟。协议实现见 [ADR-002](docs/adr/0002-browser-driver-protocols.md)。

### 安装（Debian/Ubuntu）

发布形态的 `.deb` 含 MCP 支持（`worbrow mcp`）：

```bash
make deb                       # 生成 target/debian/worbrow_*.deb
sudo apt install ./target/debian/worbrow_*.deb
```

或直接在 CI 产物/发布页安装；运行时弱依赖 Firefox（Recommends: firefox | firefox-esr）。

### MCP（Model Context Protocol）

```bash
cargo build --release
```

以 MCP stdio server 运行 `worbrow mcp`，向 MCP 客户端暴露 `web_search` 工具
（query/engine/browser/max_results/timeout/lang/region/pages/freshness/safesearch/site/filetype），
工具结果复用输出契约（schema v1）。
设计见 [ADR-005](docs/adr/0005-mcp-stdio-server.md)。
（若不需要 MCP：`cargo build --no-default-features`）

`worbrow mcp --idle-timeout <secs>`：超过该时长无任何请求自动退出（防 agent 崩溃后
残留进程；0 = 禁用，默认）。

**会话池化（MCP 长驻）**：MCP 进程内复用浏览器进程，消除每次搜索 spawn 2-5s 开销。
`--max-sessions <n>` 并发上限（默认 1 = 串行复用，超限排队）、`--session-ttl <sec>`
空闲会话回收阈值（默认 60s）；空闲超 TTL 自动回收、崩溃会话自动重建，对 agent 透明
（schema v1 不变）。设计见 [ADR-007](docs/adr/0007-mcp-session-pool.md)。

## Agent 集成

worbrow 提供两种 agent 接入方式：**MCP**（推荐，长驻进程 + 工具语义）与 **CLI 子进程**
（零依赖、单次调用）。两条路径共享同一内核与输出契约（schema v1）。

### Claude Code

在 `claude_desktop_config.json`（或 `.claude.json` 的 `mcpServers`）注册：

```json
{
  "mcpServers": {
    "worbrow": {
      "command": "worbrow",
      "args": ["mcp", "--idle-timeout", "300"]
    }
  }
}
```

> 提示：`--idle-timeout 300` 让长驻进程在 agent 会话空闲 5 分钟后自动退出，避免残留。
> 需保证 `worbrow` 在 PATH（`make deb` 安装后自动满足）。

### Cursor / 通用 MCP 客户端

项目级 `.mcp.json`（Cursor）或客户端全局配置等价结构：

```json
{
  "mcpServers": {
    "worbrow": {
      "command": "worbrow",
      "args": ["mcp", "--idle-timeout", "300"]
    }
  }
}
```

工具暴露：`web_search`（参数含 engine/browser/max_results/timeout/lang/region/pages/
freshness/safesearch/site/filetype）。

### CLI 子进程（无 MCP 客户端时）

```bash
worbrow "rust 异步" --engine bing --max-results 8 --timeout 60 --json
```

- 读 **stdout** JSON（`schema_version` 校验），日志在 **stderr**
- 非 0 退出码时 stdout 仍输出错误 JSON 包（code/message/detail）
- 结果条目自带 `domain`/`https`，无需自行解析 URL 判断来源

## 调用契约（agent 侧）

- **stdout** 仅输出 JSON（`--json`），日志全部走 stderr
- 退出码语义化：`0` 成功 / `2` 参数错 / `3` 环境错 / `4` 搜索失败 / `124` 超时 / `1` 内部错
- schema 版本化：顶层 `schema_version` 字段，字段只增不改
- 无交互、硬超时默认 60s

示例成功包：

```json
{
  "schema_version": 1,
  "query": "rust",
  "results": [{ "rank": 1, "title": "…", "url": "https://…", "snippet": "…",
                "domain": "example.com", "https": true,
                "published_at": "2025年5月25日", "is_ad": false,
                "url_resolved": true }],
  "meta": { "engine": "bing", "started_at": "…", "elapsed_ms": 1200,
            "result_count": 3, "pages": 1, "low_yield": false,
            "captcha": false, "engine_error": null,
            "engine_tried": ["bing"] }
}
```

## 作为库使用

worbrow 的库公开面是**类型级顶层 API**（ADR-006）：消费者一行 `use worbrow::...`
完成拼装，无需感知内部模块树。

```rust
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

两个入口按调用方是否已处于 tokio runtime 选择，**避免嵌套 runtime panic**：

- `search`（同步）：无 runtime 上下文时用（`main`/CLI/脚本/`spawn_blocking` 闭包）；
  内部自建 runtime，**勿在 async 上下文调用**
- `run`（async）：已有 runtime 时用（MCP handler / `#[tokio::main]` / `#[tokio::test]`），
  直接 `await` 复用外部 runtime；async 内需同步阻塞等待时可
  `tokio::task::block_in_place(|| handle.block_on(run(cfg)))`（需 multi-thread runtime）

- **依赖面**：库消费可用 `default-features = false` 去掉 MCP 依赖（`rmcp`）
  ——`mcp` feature 默认启用仅为服务 CLI 二进制
- **扩展**：自定义引擎实现 [`SearchProvider`](https://docs.rs/worbrow/latest/worbrow/trait.SearchProvider.html)
  并经 `Config::with_provider` 注入；自定义浏览器后端实现 `BrowserDriver` 并经
  `Config::with_driver` 注入
- **契约序列化**：`SuccessPayload`/`ErrorPayload`（含 `schema_version`）可直接
  `serde_json::to_string`；可运行示例见 `examples/`（`cargo run --example basic_search`）

## 质量命令

未安装 `just`，统一入口为 `make`：

```bash
make check      # fmt + clippy(-D warnings，认知复杂度 ≤10) + test
make test       # cargo test（默认含 mcp，CI 无需浏览器）
make deny       # cargo-deny 许可/漏洞检查
make machete    # 未使用依赖检查
make doctor     # 运行 worbrow doctor
```

## 目录

```
src/
  main.rs    # 薄入口 + CLI 参数解析（clap，bin 私有）
  lib.rs     # 库公开面：顶层 re-export（Config/BrowserKind/...，ADR-006）
  app.rs domain.rs error.rs ports.rs output.rs extract.rs
  drivers/   # resolve · jsonrpc(共用框架) · cdp · marionette · fake
  engines/   # resolve/AVAILABLE · duckduckgo · bing
tests/       # 集成测试 + fixtures（离线 HTML golden）
```

依赖方向：`cli → app → domain/ports ← adapters(drivers/engines)`，禁止反向。

## License

MIT OR Apache-2.0（见 [LICENSE-MIT](LICENSE-MIT)；Apache-2.0 文本见 <https://www.apache.org/licenses/LICENSE-2.0>）。
