# Loctree Engine

This repository is the public home of the Loctree engine — the full bundle in one workspace: the `loctree` CLI crate, the `loctree-ast` crate, the `loctree-mcp` server and the `loctree-lsp` server. The report renderer dependency is resolved from crates.io at the pinned release version. Issues and pull requests are welcome here.

## Install

These commands install the current release. They do not pin a version number.

```bash
curl -fsSL https://loct.io/install.sh | sh
```

The installer reads the current stable version from the release index, checks the SHA256 checksum and the GPG signature, and copies binaries into `~/.local/bin` (override with `INSTALL_DIR`). It needs `curl`, `tar`, `gpg`, and either `sha256sum` or `shasum`. Put `~/.local/bin` on `PATH` if it is not there already. On Apple silicon and Linux x64 it installs `loct`, `loctree`, `loctree-mcp`, `loctree-lsp`, `aicx`, and `aicx-mcp`. `https://loct.io/install.sh` and `https://loctree.com/install.sh` are the same script.

```bash
npm install -g @loctree/loctree
```

`@loctree/loctree`, `@loctree/loct`, and `loctree` publish the same runtime. That install provides `loct`, `loctree`, `loctree-mcp`, and `loctree-lsp` for macOS arm64, macOS Intel, Linux x64, and Windows x64.

```bash
cargo install --locked loctree
cargo install --locked loctree-mcp
```

`loctree` installs the `loct` and `loctree` binaries. `loctree-mcp` installs the MCP server. Both come from crates.io. The language server is in the installer and the npm package.

The Homebrew taps `loctree/cli` and `loctree/mcp`, and the Releases tab on this repository, currently lag the installer, npm, and crates.io.

```bash
loct --version
```

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
