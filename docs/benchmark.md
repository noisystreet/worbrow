# Search result dump (external evaluation)

This repo does **not** score search quality. The example only **runs queries** and **writes schema v1 JSON** (the same packets as `worbrow --json`). A separate model or tool reads those files and judges relevance, diversity, and usefulness.

## Run

Needs network. Default engine chain is `duckduckgo,baidu,bing`; default browser is Firefox (used when HTTP SERP fetch fails).

```bash
make bench-search
# or
cargo run --example search_benchmark -- --out target/benchmark/run
```

Flags: `--out DIR`, `--engine CHAIN`, `--browser firefox|chrome|fake`, `--max-results N`, `--timeout SECS`, `--cases PATH`.

Query list: [tests/fixtures/benchmark_queries.json](../tests/fixtures/benchmark_queries.json).

Not part of `cargo test` / CI.

## Output layout

```
<out>/
  manifest.json          # run metadata + per-case file names (no scores)
  cases/<id>.json        # success or failure packet (schema_version 1)
```

Each `cases/*.json` is either a search success payload (`query` / `results` / `meta`) or a failure payload (`error.code`). See [design.md](design.md) §7.1.

`manifest.json` fields: `kind=search_benchmark_manifest`, `started_at`, `engine`, `browser`, `max_results`, `timeout_secs`, `cases[]` with `id`, `query`, `ok`, `file`, `elapsed_ms`.

## Evaluating with another model

Point the judge at a run directory. Suggested prompt (not executed here):

- For each case, read `cases/<id>.json`.
- If `error` is present, treat as a failed run, not as bad ranking.
- If `results` is present, judge whether titles/URLs/snippets answer `query`, note ads, dictionary hits, hub homepages vs concrete pages, and empty snippets.
- Compare runs by `manifest.json` (`engine`, `started_at`).

Do not commit live dumps under `target/` (gitignored). Optional: copy a dated run elsewhere if you want a snapshot in review.
