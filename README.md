# Loctree Engine 0.15.0

This repository is the public home of the Loctree engine — the full bundle in one workspace: the `loctree` CLI crate, the `loctree-ast` crate, the `loctree-mcp` server and the `loctree-lsp` server. The report renderer dependency is resolved from crates.io at the pinned release version. Issues and pull requests are welcome here.

## Build

```bash
cargo check --workspace
cargo build --release -p loctree -p loctree-mcp -p loctree-lsp
```

## License

BUSL-1.1. See `LICENSE` and `NOTICE.md`.

## Snapshot Notes

- Target repo: `Loctree/loctree`
- Dependency mode: `local workspace snapshot`
- The engine repo carries the full source workspace - loctree, loctree-ast, loctree-mcp, loctree-lsp and the report-leptos renderer - as path dependencies. It builds standalone, with no wait on crates.io publication.
