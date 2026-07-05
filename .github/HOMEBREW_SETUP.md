# Homebrew Tap Setup

Loctree now ships through two custom taps, not through `homebrew-core`:

- `Loctree/homebrew-cli` → `brew install loctree/cli/loct`
- `Loctree/homebrew-mcp` → `brew install loctree/mcp/loctree-mcp`

## One-Time Bootstrap

1. Create the target repositories on GitHub:
   - `Loctree/homebrew-cli`
   - `Loctree/homebrew-mcp`

2. Create a GitHub token with write access to:
   - `Loctree/loct`
   - `Loctree/loctree-mcp`
   - `Loctree/homebrew-cli`
   - `Loctree/homebrew-mcp`

3. Store it in `Loctree/loctree-ast` as:
   - `HOMEBREW_GITHUB_API_TOKEN`

## Runtime Contract

`homebrew-release.yml` runs after the monorepo release is published.

It:

1. Downloads the published tarballs from `Loctree/loct` and `Loctree/loctree-mcp`.
2. Calculates SHA256 checksums for each supported Homebrew target.
3. Renders the tap formulas via `scripts/render-homebrew-formula.sh`.
4. Commits the updated formulas into the tap repos.

## Supported Homebrew Targets

- macOS Apple Silicon
- macOS Intel
- Linux x86_64

## Local Smoke Test

After a tap sync lands, install from the tap:

```bash
brew install loctree/cli/loct
brew install loctree/mcp/loctree-mcp
```

## Important

Do not maintain formula versions manually in the tap repos.
The monorepo release workflow is the only source of truth.
