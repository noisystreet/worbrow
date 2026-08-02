# AGENTS.md

Hard constraints and verification entry points for agents (and humans) working in this repository.

## Project identity

- Rust (edition 2024, MSRV 1.97) CLI tool, binary name `worbrow`, library name `worbrow`
- Provides "web search" capability for AI agents: drives local headless browsers, outputs a stable JSON contract
- Architecture authority: `docs/design.md` (§5 layering, §6 modules, §7 contract, ADR section)

## Hard constraints (revert on violation)

1. **Dependency direction**: `cli → app → domain/ports ← adapters(drivers/engines)`. `domain` has zero framework
   dependencies; `app` programs only against the `ports` traits; reverse/cyclic dependencies are forbidden.
2. **Hand-written browser protocols**: CDP (Chrome/Edge) and Marionette (Firefox) must be implemented by hand;
   **introducing chromiumoxide / fantoccini / playwright is forbidden**. Protocol commands live in their own driver
   files plus the shared JSON-RPC framework in `drivers/jsonrpc.rs`. The only exception: revisit per `docs/design.md`
   when V2 deep control is needed.
3. **Output contract**: stdout only carries JSON (success/failure payloads); the `schema_version` field only grows,
   never changes; logs always go to stderr. Breaking the contract requires bumping the schema major version and
   recording an ADR.
4. **Engine adapters**: adding an engine = a new file + one registration line in `engines/mod.rs`; parse failures go
   through `EngineFailure` (`engine_error`/exit 4); never patch by changing the schema.
5. **Exit-code semantics frozen** (0/2/3/4/124/1), see `src/error.rs::exit_code`.
6. Safety red lines: no secrets written, no permission bypass, no automatic visits to third-party URLs in search
   results (output only).

## Modifying architecture docs

- Major structural changes to `docs/design.md` (layering, ADRs, contract) require user confirmation before changing;
  small fixes (typos, numbering) can be made directly.
- New decisions are recorded as ADRs: append to `docs/adr/NNNN-title.md` (`docs/design.md` §4 is the index table;
  format follows the docs-style template).

## Commit conventions

- Commit messages follow Conventional Commits; the type is in English (`feat`/`fix`/`docs`/`chore`/`refactor`/..., enforced by pre-commit).
- The description part is written in English. Applies to new commits only; history is never rewritten.

## Mandatory verification commands

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity
cargo test
cargo deny check
cargo machete
```

CI and local use the same checks (see `.github/workflows/ci.yml`, Makefile).
