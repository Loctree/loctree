# Component Sync Manifest

- Component: `engine`
- Version: `0.15.1`
- Target repo: `Loctree/loctree`
- Source commit: `d69d50802e0a`
- Generated at: `2026-10-16T16:12:47Z`
- Push mode: `disabled`
- Dependency mode: `local workspace snapshot`

## Included Payload

- `loctree-rs` -> `loctree-rs`
- `loctree-ast` -> `loctree-ast`
- `loctree-mcp` -> `loctree-mcp`
- `loctree-lsp` -> `loctree-lsp`
- `reports` -> `reports`
- `editors` -> `editors`
- `loctree-plugin` -> `loctree-plugin`
- `distribution/npm` -> `distribution/npm`
- `distribution/macos` -> `distribution/macos`

## Paths Removed From The Mirror

- `.github/workflows/release-bundles.yml`

## Dependency Note

The engine repo carries the full source workspace - loctree, loctree-ast, loctree-mcp, loctree-lsp and the report-leptos renderer - as path dependencies. It builds standalone, with no wait on crates.io publication.
