---
status: completed
agent: codex
branch: feat/lsp/codelens-live-analyzer-local
plan: docs/plans/lsp/22-context-scope-flag.md
completed: 2026-05-10T12:22:29Z
---

# Plan 22 — `loct context --scope`

## Summary

Plan 22 is complete. The implementation exposes deterministic structural
scoping for `loct context` across the CLI and MCP ContextPack path:

- repeatable `--scope` / `scope` selectors
- `path:`, `tag:`, `import:`, and `reach:` selector resolution
- named scopes from `.loctree/scopes.toml`
- top-level ContextPack `scope` metadata
- top-level `task` metadata, with scoped tasks reported as
  `ranker_within_scope`
- scoped risk cache marker as `RiskCacheScope::Scoped(<fingerprint>)`
- `--file` precedence over `--scope`

## Files

- `loctree-rs/src/context_scope.rs`
- `loctree-rs/src/cli/parser/context_commands.rs`
- `loctree-rs/src/cli/dispatch/handlers/context/mod.rs`
- `loctree-rs/src/cli/command/help_texts.rs`
- `loctree-mcp/src/main.rs`

## Finalization Delta

This close-out added regression coverage for the pieces most likely to drift:

- `test_parse_context_scope_flags_are_repeatable`
- `compose_context_pack_records_scope_task_and_scoped_cache_marker`

The tracker row is now marked `done`, and the plan file frontmatter is aligned
with that final state.

## Verification

Passed:

```bash
cargo fmt --all --check
rustfmt --edition 2024 --check loctree-rs/src/cli/parser/context_commands.rs loctree-rs/src/cli/dispatch/handlers/context/mod.rs
cargo test -p loctree context_scope --lib
cargo test -p loctree parse_context --lib
cargo test -p loctree test_parse_context_scope_flags_are_repeatable --lib
cargo test -p loctree compose_context_pack_records_scope_task_and_scoped_cache_marker --lib
cargo run -q -p loctree --bin loct -- context --scope path:loctree-rs/src/cli/ --no-aicx --full --json
cargo check -p loctree-mcp
cargo clippy -p loctree --lib -- -D warnings
make precheck
cargo test -p loctree
make test
```

The invalid probe `cargo test -p loctree-mcp context --lib` was attempted first
and rejected because `loctree-mcp` has no library target; `cargo check -p
loctree-mcp` is the correct validation for that crate in this close-out.
