# Loctree Engine 0.13.0

This repository is the public release mirror for the Loctree engine.

It contains the `loctree` crate, the `loctree-ast` crate, and the internal report renderer required by the current engine build graph. This is a release mirror of a private integration monorepo; issues/PRs welcome here.

## Build

```bash
cargo check --workspace
```

## License

BUSL-1.1. See `LICENSE` and `NOTICE.md`.

## Snapshot Notes

- Target repo: `Loctree/loctree`
- Dependency mode: `local workspace snapshot`
- Engine staging vendors report-leptos as an internal build dependency because loctree 0.13.0 still depends on it and the 0.13.0 crate line is not yet published.
