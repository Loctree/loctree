# Loctree Engine 0.13.1

This repository is the public release mirror for the Loctree engine — the full bundle in one workspace: the `loctree` CLI crate, the `loctree-ast` crate, the `loctree-mcp` server and the `loctree-lsp` server. The report renderer dependency is resolved from crates.io at the pinned release version. This is a release mirror of a private integration monorepo; issues/PRs welcome here.

## Build

```bash
cargo check --workspace
cargo build --release -p loctree -p loctree-mcp -p loctree-lsp
```

## License

BUSL-1.1. See `LICENSE` and `NOTICE.md`.

## Snapshot Notes

- Target repo: `Loctree/loctree`
- Dependency mode: `crates.io registry`
- Engine staging keeps loctree, loctree-ast, loctree-mcp and loctree-lsp as local mirror payloads, while report-leptos is consumed from crates.io at the pinned release version; no reports/ vendor payload in this mirror.
