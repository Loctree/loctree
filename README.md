# Loctree Engine 0.15.1

This repository is the public home of the Loctree engine — the full bundle in one workspace: the `loctree` CLI crate, the `loctree-ast` crate, the `loctree-mcp` server, the `loctree-lsp` server and the `report-leptos` renderer in `reports/`. All five crates use local path dependencies, so this source snapshot builds standalone without waiting for crates.io publication. Issues and pull requests are welcome here.

## Install

```bash
npm install -g @loctree/loctree
```

The canonical npm package exposes `loct`, `loctree`, `loctree-mcp` and
`loctree-lsp`. The maintained names `@loctree/loctree` (canonical),
`@loctree/loct` and `loctree` are alternative owners of these same four
commands. Pick one. Installing another name over an existing wrapper can fail
with `EEXIST`; npm checks bin ownership before a package's `preinstall` hook runs.

If your existing global wrapper is the short name `loctree`, remove that exact
package first:

```bash
npm uninstall -g loctree
```

If the existing wrapper is `@loctree/loct`, use `npm uninstall -g @loctree/loct`
instead. Then install the canonical name:

```bash
npm install -g @loctree/loctree
```

If `@loctree/loctree` is already installed, reinstall that same name directly.
Keep `-g` explicit. Do not use `--force` or remove unrelated prefix files,
Cargo binaries or Homebrew links to resolve a wrapper collision. Contributor
builds can use [`install-local.sh`](distribution/npm/install-local.sh) with
`--native-bin-dir <built-bin-directory>` and, when switching from a verified
previous wrapper, `--legacy-names '<previous-wrapper-name>'`.

The install command selects the published npm package. This source candidate
does not publish or replace an existing registry version.

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
