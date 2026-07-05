# GitHub Actions Workflows

This directory contains the release, CI, and automation workflows for the
Loctree monorepo.

## Active Workflows

### Release & Distribution

| Workflow | Trigger | Purpose | Status |
|----------|---------|---------|--------|
| **publish.yml** | Tag push (`v*`, `loctree-v*`) | Publish crates, build CLI + MCP binaries, push assets into thin repos, publish npm, then create the monorepo release | ✅ Active |
| **homebrew-release.yml** | Monorepo release published / manual dispatch | Render formulas and sync custom taps `Loctree/homebrew-cli` + `Loctree/homebrew-mcp` | ✅ Active |

### CI & Quality

| Workflow | Trigger | Purpose | Status |
|----------|---------|---------|--------|
| **ci.yml** | Push, PR | Workspace fmt, clippy, tests on self-hosted Linux + macOS | ✅ Active |
| **loctree-ci.yml** | Push, PR | Self-analysis dogfooding on self-hosted Linux + macOS | ✅ Active |
| **semgrep.yml** | Push, PR | Security scanning on self-hosted Linux | ✅ Active |

### AI Assistants

| Workflow | Trigger | Purpose | Status |
|----------|---------|---------|--------|
| **claude.yml** | Manual dispatch | Claude AI assistance | ✅ Active |
| **gemini-*.yml** | Issues, PR comments | Gemini AI triage and review | ✅ Active |
| **codex-auto-fix.yml** | PR comments | Automated code fixes | ✅ Active |

## Release Shape

The monorepo is the build and orchestration source of truth.

User-facing binary distribution is split into thin repos:

- CLI assets: `Loctree/loct`
- MCP assets: `Loctree/loctree-mcp`
- Homebrew tap for CLI: `Loctree/homebrew-cli`
- Homebrew tap for MCP: `Loctree/homebrew-mcp`

This keeps `Loctree/loctree-ast` focused on code, CI, and release choreography while
the thin repos stay narrowly scoped to distribution.

## Required Secrets

The release pipeline expects these secrets in `Loctree/loctree-ast`:

- `CARGO_REGISTRY_TOKEN`
- `NPM_TOKEN`
- `HOMEBREW_GITHUB_API_TOKEN`
- `MACOS_CERT_P12_BASE64`
- `MACOS_CERT_PASSWORD`
- `MACOS_KEYCHAIN_PASSWORD`
- `MACOS_DEVELOPER_ID_APPLICATION`
- `APPLE_API_KEY_BASE64`
- `APPLE_API_KEY_ID`
- `APPLE_API_ISSUER_ID`

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

That tag push triggers the publish pipeline. The workflow then:

1. Verifies workspace and npm versions match the tag.
2. Publishes `report-leptos`, `loctree`, and `loctree-mcp`.
3. Builds signed binaries for CLI and MCP.
4. Uploads assets into `Loctree/loct` and `Loctree/loctree-mcp`.
5. Publishes npm packages from `distribution/npm`.
6. Creates the monorepo release.
7. Triggers tap sync into `Loctree/homebrew-cli` and `Loctree/homebrew-mcp`.

## Monitoring

- Monorepo actions: https://github.com/Loctree/loctree-ast/actions
- CLI releases: https://github.com/Loctree/loct/releases
- MCP releases: https://github.com/Loctree/loctree-mcp/releases
- CLI tap: https://github.com/Loctree/homebrew-cli
- MCP tap: https://github.com/Loctree/homebrew-mcp

## Bootstrap Note

Before the first release on this architecture, create the four thin repos above.
The workflows assume they already exist and will fail fast if they do not.
