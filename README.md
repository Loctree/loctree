# Loctree Engine 0.13.1

This repository is the public release mirror for the Loctree engine.

It contains the `loctree` crate and the `loctree-ast` crate. The report renderer dependency is resolved from crates.io at the pinned release version. This is a release mirror of a private integration monorepo; issues/PRs welcome here.

## Build

```bash
cargo check --workspace
```

## License

BUSL-1.1. See `LICENSE` and `NOTICE.md`.

## Snapshot Notes

- Target repo: `Loctree/loctree`
- Dependency mode: `crates.io registry`
- Engine staging keeps loctree and loctree-ast as local mirror payloads, while report-leptos is consumed from crates.io at the pinned release version; no reports/ vendor payload in this mirror.
