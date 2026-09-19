# GitHub Actions Workflows

This directory contains the release, CI, and automation workflows for the
Loctree monorepo.

## Active Workflows

### Release & Distribution

| Workflow | Trigger | Purpose | Status |
|----------|---------|---------|--------|
| **release-bundles.yml** | Manual dispatch / tag push (`v*`) | Build combined Loctree + AICX tarballs for macOS ARM64, Linux X64 GNU, and Windows X64 MSVC, plus core tarballs for macOS X64 and Linux musl | ✅ Active |
| **publish.yml** | **Manual dispatch** (`workflow_dispatch`, input `tag`) | Publish crates, build binaries, push active-line assets into thin repos, publish one runtime wrapper as `@loctree/loctree`, `@loctree/loct`, and `loctree` via npm trusted publishing, then create the monorepo release | ✅ Active |
| **homebrew-release.yml** | Monorepo release published / manual dispatch | Render formulas and sync active-line formulas into `Loctree/homebrew-cli` + `Loctree/homebrew-mcp` | ✅ Active |

### CI & Quality

| Workflow | Trigger | Purpose | Status |
|----------|---------|---------|--------|
| **ci.yml** | Selected long-lived pushes; all internal PR bases | Workspace fmt, clippy, hook contracts and tests on GitHub-hosted Linux and macOS | ✅ Active |
| **codeql.yml** | Push, PR, manual dispatch | CodeQL scanning with Rust autobuild plus explicit Gradle build for the JetBrains Kotlin plugin | ✅ Active |
| **loctree-ci.yml** | Push, PR | Self-analysis dogfooding on GitHub-hosted Linux + macOS | ✅ Active |
| **semgrep.yml** | Push, PR | Security scanning on GitHub-hosted Linux | ✅ Active |

### AI Assistants

| Workflow | Trigger | Purpose | Status |
|----------|---------|---------|--------|
| **claude.yml** | Manual dispatch | Claude AI assistance | ✅ Active |
| **gemini-*.yml** | Issues, PR comments | Gemini AI triage and review | ✅ Active |

## Release Shape

The monorepo is the build and orchestration source of truth.

User-facing binary distribution is split into thin repos:

- Combined suite tarballs: `dist/release-bundles/<version>/loctree-<version>-<target>.tar.gz`
- Musl core tarball: `dist/release-bundles/<version>/loctree-<version>-x86_64-unknown-linux-musl-core.tar.gz`
- Windows full tarball: `dist/release-bundles/<version>/loctree-<version>-x86_64-pc-windows-msvc.tar.gz`
- Windows raw LSP: `dist/release-bundles/<version>/loctree-lsp-windows-x64.exe`
- CLI assets: `Loctree/loct`
- MCP assets: `Loctree/loctree-mcp`
- Homebrew tap for CLI: `Loctree/homebrew-cli`
- Homebrew tap for MCP: `Loctree/homebrew-mcp`

This keeps `Loctree/loctree-suite` focused on code, CI, and release choreography while
the thin repos stay narrowly scoped to distribution.

## Active Naming Matrix

| Surface | Active name | Current source-of-truth in this repo | Notes |
|---------|-------------|--------------------------------------|-------|
| CLI | `loct` | thin repo `Loctree/loct`, npm package `loct`, Homebrew formula `loct` | Crates.io package remains `loctree` for now |
| MCP | `loct-mcp` | thin repo `Loctree/loctree-mcp`, Homebrew formula `loct-mcp` | Crates.io package remains `loctree-mcp` for now |
| LSP | `loct-lsp` | release asset naming in `vscode-extension.yml` | Thin repo/npm/Homebrew sync not wired yet |
| Report | `loct-report` | not in active release path yet | `reports/` still publishes as legacy `report-leptos` |

The public `loctree` / `loctree-mcp` / `report-leptos` naming only remains where
the current crates.io line still depends on it.

## Required Secrets

The release pipeline expects these secrets in `Loctree/loctree-suite`:

- `CARGO_REGISTRY_TOKEN`
- `HOMEBREW_GITHUB_API_TOKEN`
- `MACOS_CERT_P12_BASE64`
- `MACOS_CERT_PASSWORD`
- `MACOS_KEYCHAIN_PASSWORD`
- `MACOS_DEVELOPER_ID_APPLICATION`
- `APPLE_API_KEY_BASE64`
- `APPLE_API_KEY_ID`
- `APPLE_API_ISSUER_ID`
- `LOCTREE_GPG_KEY_ID` (optional; creates `.sig` sidecars when the runner has the key)

`HOMEBREW_GITHUB_API_TOKEN` must be able to write releases to:

- `Loctree/loct`
- `Loctree/loctree-mcp`
- `Loctree/homebrew-cli`
- `Loctree/homebrew-mcp`

## Release Entry Point

The canonical human entry point stays the same:

```bash
make version TYPE=minor TAG=1 PUSH=1
```

`publish.yml` is triggered **manually** via `workflow_dispatch` (with a `tag`
input), not automatically by the tag push. Once dispatched, the workflow:

1. Verifies workspace and npm versions match the tag.
2. Publishes `report-leptos`, `loctree`, and `loctree-mcp` as the current legacy crate line.
3. Builds signed binaries for CLI and MCP.
4. Uploads active-line assets into `Loctree/loct` and `Loctree/loctree-mcp`.
5. Publishes four platform packages, then one suite wrapper under canonical
   `@loctree/loctree` plus maintained aliases `@loctree/loct` and `loctree`.
6. Creates the monorepo release.
7. Triggers tap sync for formulas `loct` and `loct-mcp` into `Loctree/homebrew-cli` and `Loctree/homebrew-mcp`.

For the binary-first suite bundle, run `Build Combined Release Bundles` manually
or from a version tag. It produces:

- `loctree-<version>-aarch64-apple-darwin.tar.gz`
- `loctree-<version>-x86_64-unknown-linux-gnu.tar.gz`
- `loctree-<version>-x86_64-unknown-linux-musl-core.tar.gz`

Full bundles contain `components.json`, `CHECKSUMS.sha256`, `README.md`, and
exactly these binaries: `loct`, `loctree`, `loctree-mcp`, `loctree-lsp`,
`aicx`, `aicx-mcp`.

The musl artifact is a `-core` bundle: static Loctree binaries only, with
`components.json` and `README.md` explicitly marking AICX as an optional runtime
dependency because no static musl AICX release asset exists.

## Monitoring

- Monorepo actions: https://github.com/Loctree/loctree-suite/actions
- CLI releases: https://github.com/Loctree/loct/releases
- MCP releases: https://github.com/Loctree/loctree-mcp/releases
- CLI tap: https://github.com/Loctree/homebrew-cli
- MCP tap: https://github.com/Loctree/homebrew-mcp

## Bootstrap Note

The four release/tap repositories above exist. The workflows still verify them
and fail fast when the configured token cannot write to the target.
