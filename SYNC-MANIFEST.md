# Component Sync Manifest

- Component: `engine`
- Version: `0.15.0`
- Target repo: `Loctree/loctree`
- Source commit: `abbb3d48a840`
- Generated at: `2026-10-14T19:26:12Z`
- Push mode: `enabled`
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

- `.github/workflows/publish.yml`
- `.github/workflows/release-bundles.yml`

## Dependency Note

The engine repo carries the full source workspace - loctree, loctree-ast, loctree-mcp, loctree-lsp and the report-leptos renderer - as path dependencies. It builds standalone, with no wait on crates.io publication.
