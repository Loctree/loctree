# Loctree npm — Quick Start

One install. One runtime.

## Install

```bash
npm install -g @loctree/loctree
```

You get **one runtime**, with two CLI names and one MCP adapter:

- `loctree` — the Loctree runtime (scan, slice, impact, health, MCP, editors)
- `loct` — short alias for `loctree` (same binary)
- `loctree-mcp` — stdio MCP co-process for MCP clients

## Smoke test

```bash
loctree --version
loct --version
```

If a command is not found, your `npm bin -g` path is not on `PATH`.

## Install from a local checkout (developers)

```bash
make npm-install        # alias: make install-npm
```

Builds the four suite binaries, stages the wrapper plus your host platform
package, and runs a real `npm install -g` from local tarballs — the machine
ends up with exactly the registry layout. Handy overrides:
`NPM_INSTALL_WORK=/path` (staging area), `npm_config_prefix=/tmp/x` (throwaway
prefix). `distribution/npm/install-local.sh` is the reusable core; it takes
`--wrapper` / `--native-bin-dir` / `--platform` / `--legacy-names` and can be
dropped into other wrapper+platform-packages repos (e.g. AICX) as-is.

## Everyday CLI

```bash
loct                                  # scan current dir + write snapshot
loct slice src/lib.rs --consumers     # 3-layer slice around a file
loct impact src/core.rs               # blast radius if you refactor
loct find FileAnalysis                # symbol search
loct health                           # sanity check before commit
```

## MCP — stdio or `loct watch --http`

MCP is part of the same runtime package, not a separate npm product. Bring up
the streamable-HTTP server (the runtime co-spawns `loctree-mcp` for you):

```bash
loct watch --http              # MCP at http://127.0.0.1:5174/mcp (override with --port)
```

Point an HTTP-capable MCP client (e.g. an agent gateway) at
`http://127.0.0.1:5174/mcp`.

For a stdio MCP client without a global install:

```bash
npx -y --package=@loctree/loctree loctree-mcp
```

Do not shorten this to `npx -y @loctree/loctree`: that command intentionally runs
the `loct` CLI.

## Editors — `loct watch --lsp`

```bash
loct watch --lsp               # runs the watcher + co-spawned editor language server
```

Editor integrations talk to the language-server co-process; you do not run it as
a separate command.

## Supported platforms

- macOS Apple Silicon (`darwin-arm64`)
- macOS Intel (`darwin-x64`)
- Linux x64 glibc (`linux-x64-gnu`)
- Windows x64 MSVC (`win32-x64-msvc`)

We only claim the targets CI actually builds today.

## Troubleshooting

### "Platform package not found"

Wait 30–60 seconds for npm registry propagation after release, then try again.

### `--ignore-scripts`

Binaries ship inside the platform package, so `--ignore-scripts` does not break
the install; it only skips the post-install validation log.

## Next steps

- [README.md](./README.md) — overview of the single-wrapper layout
- [PUBLISHING.md](./PUBLISHING.md) — maintainer publish flow
- Source: https://github.com/Loctree/loctree-suite
- Issues: https://github.com/Loctree/loctree-suite/issues
