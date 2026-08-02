# worbrow

[![CI](https://github.com/noisystreet/worbrow/actions/workflows/ci.yml/badge.svg)](https://github.com/noisystreet/worbrow/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/worbrow.svg)](https://crates.io/crates/worbrow)
[![Rust](https://img.shields.io/badge/rust-1.97+-orange.svg)](https://github.com/noisystreet/worbrow)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![MSRV](https://img.shields.io/badge/MSRV-1.97-blue)](https://github.com/noisystreet/worbrow)

Agent search CLI: drives **local headless browsers** (Chrome/Edge via hand-written CDP, Firefox via hand-written Marionette) to search general web search engines, emitting a stable JSON contract for AI agents to invoke as a subprocess.

Architecture & decisions: [docs/design.md](docs/design.md); feature roadmap: [docs/roadmap.md](docs/roadmap.md). 简体中文版见 [README.zh-CN.md](README.zh-CN.md).

## Quick Start

Prerequisites: Chrome/Edge (>= 109) or Firefox (>= 55) installed on the system.

```bash
cargo run -- list                    # list available engines
cargo run -- doctor                  # environment self-check (browser binaries/engines/backend status)
cargo run -- "rust async runtime" --json   # default engine bing→duckduckgo→baidu fallback, default timeout 60s
cargo run -- "rust" --engine duckduckgo --timeout 30 --max-results 5
cargo run -- "rust" --pages 2 --max-results 15 --lang zh-hans --region zh-CN   # multi-page aggregation + language/region
cargo run -- "rust" --freshness week --safesearch strict                       # freshness filter + safe search
cargo run -- "rust" --site doc.rust-lang.org --filetype pdf                    # site/file-type filter
cargo run -- "rust" --engine bing,duckduckgo   # engine fallback chain (auto-tries the next on captcha/low-quality/low-yield)
cargo run -- "rust" --retry 2                  # backoff retry on transient network errors (exponential backoff capped at 8s)
# body fetch + structured extraction (ADR-009): pass an explicit URL, get cleaned text and optional fields
cargo run -- fetch https://example.com/rust --json
cargo run -- fetch https://example.com --extract price,rating --json    # field extraction (allowlist)
cargo run -- fetch https://example.com --no-text --extract price        # fields only, saves tokens
```

Backend status: `firefox` (Marionette, hand-written protocol) and `chrome` (CDP, hand-written protocol) are both implemented; `fake` is for tests/smoke. Protocol implementation: [ADR-002](docs/adr/0002-browser-driver-protocols.md).

### Installation

**Prebuilt binaries** are attached to every [GitHub Release](https://github.com/noisystreet/worbrow/releases) (`v*` tags): `worbrow-linux-x86_64.tar.gz`, `worbrow-linux-x86_64-musl.tar.gz` (statically linked, runs on any Linux), `worbrow-macos-arm64.tar.gz`, `worbrow-windows-x86_64.zip`, each with a `SHA256SUMS` checksum. Debian/Ubuntu users can also install the release `.deb` (includes MCP support, `worbrow mcp`):

```bash
sudo apt install ./worbrow_*.deb
```

Or build from source (`cargo install worbrow`, or `make deb` for a local `.deb`). Runtime soft dependency on Firefox (Recommends: firefox | firefox-esr).

### MCP (Model Context Protocol)

```bash
cargo build --release
```

Run `worbrow mcp` as an MCP stdio server, exposing tools to MCP clients:
- `web_search` (query/engine/browser/max_results/timeout/lang/region/pages/freshness/safesearch/site/filetype/retry/no_cache/compact)
- `fetch_page` (url/browser/timeout/max_chars/extract/text/wait_selector/retry: fetches an **explicitly passed** URL, returns cleaned body text and optional structured fields)
- `list_engines` (list available engines), `doctor` (environment self-check: browser binaries/versions/engine registry)

Tool results reuse the output contract (schema v1). With `compact=true`, `web_search` results contain only rank/title/url (saves agent context tokens, meta complete).
Design: [ADR-005](docs/adr/0005-mcp-stdio-server.md) and [ADR-009](docs/adr/0009-fetch-page.md).
(If MCP is not needed: `cargo build --no-default-features`)

`worbrow mcp --idle-timeout <secs>`: exits automatically after this long without any request (prevents orphan processes after an agent crashes; 0 = disabled, the default).

**Session pooling (MCP long-running)**: browser processes are reused inside the MCP process, removing the 2-5s spawn overhead per search.
`--max-sessions <n>` concurrency cap (default 1 = serial reuse, excess queued), `--session-ttl <sec>` idle-session reclamation threshold (default 60s); idle sessions past TTL are recycled, crashed sessions auto-rebuilt, transparent to agents (schema v1 unchanged). Design: [ADR-007](docs/adr/0007-mcp-session-pool.md).

**Network retry & result cache (ADR-008)**: the `retry` parameter (default 0) applies exponential backoff to transient network errors (capped at 8s, counted in the timeout budget); `meta.retries` records the actual retry count. Inside the long-running MCP process, repeated calls with identical parameters within a 60s TTL hit the cache directly (`meta.cached=true`, `elapsed_ms=0`); `no_cache` bypasses it. The CLI `--retry <n>` supports retry as well (no cache).

### Body fetch & structured extraction (ADR-009)

`fetch_page` / `worbrow fetch <url>` fetches an **explicitly passed** URL and returns cleaned body text, optionally extracting structured fields via the `extract` allowlist (title/author/published_at/price/currency/rating/rating_max/reviews_count; extraction priority JSON-LD → meta → DOM; missing fields are omitted, never fabricated):

```bash
worbrow fetch https://example.com --json --extract price,rating
```

- **Closing the loop**: pass `results[i].url` from `web_search` explicitly to `fetch_page` — "search links → read content → compare fields" in one step; **never auto-follows search results** (fetches explicit URLs only)
- **Parameters**: `--max-chars <n>` (body truncation, default 20000, flagged by `meta.truncated`), `--no-text` (extracted fields only, saves tokens), `--extract a,b` (allowlist, invalid values exit 2), `--wait-selector <css>` (SPA: wait for this selector before extracting text, best-effort)
- **Known behavior**: navigation success always yields a success payload (body may be empty); `meta.http_status` reports the page HTTP status (best-effort via `PerformanceNavigationTiming.responseStatus`; `null` on Firefox < 105 / data: URLs), so 4xx/5xx/404 no longer go unnoticed; `meta.final_url` records the redirect landing page; SPA/lazy-loaded content may be missing — pass `--wait-selector <css>` to wait for the content to render
- **Safety boundary**: `http/https` only (missing scheme defaults to `https://`); navigates with a real browser, page JS runs inside the browser (equivalent to clicking the link yourself); **can reach localhost/intranet** (equivalent to your local browser — do not feed untrusted input if you want to avoid being induced to fetch intranet content); no bulk fetching; rate discipline still applies
- **Compliance**: fetch is an explicit full-page fetch by the user, a separate path from search engines' snippet-only crawling policy

## Agent Integration

worbrow offers two agent integration paths: **MCP** (recommended; long-running process + tool semantics) and **CLI subprocess** (zero-dependency, one-shot). Both share the same core and output contract (schema v1).

### Claude Code

Register in `claude_desktop_config.json` (or `.claude.json` under `mcpServers`):

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

> Tip: `--idle-timeout 300` makes the long-running process exit 5 minutes after the agent session goes idle, avoiding orphans.
> Ensure `worbrow` is on PATH (automatically satisfied after `make deb`).

### Cursor / generic MCP clients

Project-level `.mcp.json` (Cursor) or an equivalent global client config:

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

Exposed tools: `web_search` (parameters include engine/browser/max_results/timeout/lang/region/pages/freshness/safesearch/site/filetype).

### CLI subprocess (without an MCP client)

```bash
worbrow "rust async" --engine bing --max-results 8 --timeout 60 --json
```

- Read **stdout** JSON (validate `schema_version`), logs on **stderr**
- On non-zero exit, stdout still carries the error JSON payload (code/message/detail)
- Each result carries `domain`/`https`, no need to parse URLs yourself to judge the source

## Output contract (agent side)

- **stdout** only emits JSON (`--json`), all logs go to stderr
- Semantic exit codes: `0` success / `2` argument error / `3` environment error / `4` search failure / `124` timeout / `1` internal error
- Versioned schema: top-level `schema_version` field; fields only grow, never change
- No interaction; hard timeout defaults to 60s

Example success payload:

```json
{
  "schema_version": 1,
  "query": "rust",
  "results": [{ "rank": 1, "title": "…", "url": "https://…", "snippet": "…",
                "domain": "example.com", "https": true,
                "published_at": "2025年5月25日", "is_ad": false,
                "url_resolved": true, "result_kind": "web" }],
  "meta": { "engine": "bing", "started_at": "…", "elapsed_ms": 1200,
            "result_count": 3, "pages": 1, "low_yield": false,
            "captcha": false, "engine_error": null,
            "engine_tried": ["bing"], "cached": false, "retries": 0 }
}
```

## Use as a library

worbrow's public library surface is a **type-level top-level API** (ADR-006): consumers assemble everything with one `use worbrow::...`, no need to know the internal module tree.

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

Body fetch (ADR-009) is a first-class library API too:

```rust
use worbrow::{BrowserKind, ExtractField, FetchConfig, fetch};

fn main() -> Result<(), worbrow::Error> {
    let page = fetch(FetchConfig::new("https://example.com/rust", BrowserKind::Firefox)
        .with_extract(vec![ExtractField::Title, ExtractField::PublishedAt]))?;
    println!("{}: {} chars", page.url, page.chars);
    println!("title: {:?}", page.extracted.get("title"));
    Ok(())
}
```

Pick the entry point by whether you are already inside a tokio runtime, to **avoid nested-runtime panics**:

- `search` (sync): use when there is no runtime context (`main`/CLI/scripts/`spawn_blocking` closures); builds its own runtime internally, **do not call in async contexts**
- `run` (async): use when a runtime already exists (MCP handlers / `#[tokio::main]` / `#[tokio::test]`), awaiting reuses the external runtime; to block-synchronously wait inside async, use `tokio::task::block_in_place(|| handle.block_on(run(cfg)))` (needs a multi-thread runtime)

- **Dependency surface**: library consumers can use `default-features = false` to drop the MCP dependency (`rmcp`) — the `mcp` feature is enabled by default only to serve the CLI binary
- **Extension**: implement a custom engine via [`SearchProvider`](https://docs.rs/worbrow/latest/worbrow/trait.SearchProvider.html) and inject it with `Config::with_provider`; implement a custom browser backend via `BrowserDriver` and inject it with `Config::with_driver`
- **Contract serialization**: `SuccessPayload`/`ErrorPayload` (including `schema_version`) can be `serde_json::to_string`'d directly; runnable examples live in `examples/` (`cargo run --example basic_search`)

## Quality commands

No `just`; the unified entry point is `make`:

```bash
make check      # fmt + clippy(-D warnings, cognitive complexity <= 10) + test
make test       # cargo test (mcp enabled by default, CI needs no browser)
make deny       # cargo-deny license/vulnerability check
make machete    # unused-dependency check
make doctor     # run worbrow doctor
```

## Search tips

- **Avoid stacking nouns separated by spaces in Chinese queries**: Bing may anchor
  the whole query to the first strong entity (e.g. a fund-data query returns China
  wiki/baike pages) — results are plentiful and well-formed but completely
  off-topic. The relevance gate falls back to DuckDuckGo/Baidu automatically, but
  it is more reliable to cut noisy terms; use double quotes for exact phrases
  (e.g. `"天天基金网 净值查询"`)
- **Engine choice**: `--engine duckduckgo` (most reliable for Chinese queries in
  practice), `--engine baidu` (Chinese long-tail; reachable from CN networks with
  no CAPTCHA wall), `--engine bing` (English queries); the default chain
  `bing,duckduckgo,baidu` tries the next engine when the quality gates fail
- **Query quality**: avoid piling up high-noise words such as `best`/`learn`
  (Bing tends to misclassify them as dictionary intent; see
  [docs/roadmap-result-quality.md](docs/roadmap-result-quality.md)); rephrase with
  synonyms, quotes, or `site:` constraints when results are poor

## Layout

```
src/
  main.rs    # thin entry + CLI parsing (clap, bin-private)
  lib.rs     # public library surface: top-level re-exports (Config/BrowserKind/..., ADR-006)
  app.rs domain.rs error.rs ports.rs output.rs extract.rs
  drivers/   # resolve · jsonrpc(shared framework) · cdp · marionette · fake
  engines/   # resolve/AVAILABLE · duckduckgo · bing · baidu
tests/       # integration tests + fixtures (offline HTML golden)
```

Dependency direction: `cli → app → domain/ports ← adapters(drivers/engines)`, reverse is forbidden.

## License

MIT OR Apache-2.0 (see [LICENSE-MIT](LICENSE-MIT); Apache-2.0 text at <https://www.apache.org/licenses/LICENSE-2.0>).
