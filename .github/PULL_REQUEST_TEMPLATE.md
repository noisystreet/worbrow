## Summary

<!-- Describe what this change does and why (1-2 paragraphs). -->

## Related issues

<!-- Reference `#123` if any; leave empty otherwise. -->

## Checklist

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity` passes
- [ ] `cargo test` passes
- [ ] `cargo machete` passes (`cargo deny check` may be skipped when network is restricted locally; CI runs it)
- [ ] Contract changes (JSON schema / exit codes) bump `schema_version` and record an ADR (see `docs/adr/`)
- [ ] User-visible changes are reflected in `README.md` / `CHANGELOG.md`
