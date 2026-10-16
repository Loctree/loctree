# distribution/npm — Loctree npm release surface

This directory is the canonical npm distribution surface for Loctree. The source
of truth for the code lives in
[Loctree/loctree-suite](https://github.com/Loctree/loctree-suite); this folder
contains the thin JS wrapper and platform package manifests that ship to the
`@loctree` npm scope.

## One wrapper, three maintained names, one runtime

| Package | Commands exposed | Notes |
| --- | --- | --- |
| `@loctree/loctree` | `loctree`, `loct`, `loctree-mcp`, `loctree-lsp` | canonical identity |
| `@loctree/loct` | same | maintained scoped alias |
| `loctree` | same | maintained short form |

`loctree` and `loct` are the **same binary** (`loct` is a short alias). MCP and
LSP are not separate packages — they are modes/co-processes the runtime spawns:

- `loct watch --http` → streamable-HTTP MCP at `http://127.0.0.1:5174/mcp`
- `loct watch --lsp` → editor language server

The wrapper declares 4 platform sub-packages as `optionalDependencies`
(esbuild/swc pattern). Platform matrix: `darwin-arm64`, `darwin-x64`,
`linux-x64-gnu`, `win32-x64-msvc`.

**Embed model:** each `@loctree/loctree-<platform>` package carries four release
binaries side by side — `loct`, `loctree`, `loctree-mcp`, `loctree-lsp` — with no
postinstall download. The runtime resolves `loctree-mcp` / `loctree-lsp` as
**siblings** of its own executable, so they must live in the same directory.

Total: **one wrapper source under 3 names + 4 platform packages = 7 npm identities.**

> There are **no separate** `@loctree/loctree-mcp` or `@loctree/loctree-lsp` npm
> packages. The bundled MCP binary is exposed as `loctree-mcp` so stdio clients
> can use `npx -y --package=@loctree/loctree loctree-mcp`; LSP stays internal.

## Install (end users)

```bash
npm install -g @loctree/loctree
```

The three maintained names are alternatives, not packages to install together:
they claim identical command links. To change wrapper names, remove the exact
previous global wrapper first, then install the canonical name above. Follow
the [named migration steps](../../docs/installation.md#switching-npm-wrapper-names);
do not force-overwrite unrelated command owners. Contributor builds can reuse
`install-local.sh --legacy-names '<previous-wrapper-name>'` (`make npm-install`
already selects `loctree` as its legacy name).

Smoke-test:

```bash
loctree --version
loct --version
```

> AICX (`aicx` / `aicx-mcp`) is owned by the sibling
> [`Loctree/aicx`](https://github.com/Loctree/aicx) repo and publishes separately
> as `@loctree/aicx`. It is not part of the `@loctree/loctree` runtime.

## Layout

```
distribution/npm/
├── README.md            (this file)
├── PUBLISHING.md        (local bootstrap + exact-tag OIDC contract for 7 identities)
├── QUICKSTART.md        (end-user install recipe)
├── LICENSE              (BUSL-1.1)
├── sync-version.mjs     (bump the 5 package.json files in lockstep)
└── loct/                (the @loctree/loctree wrapper + 4 platform packages)
```

> The legacy Gen2 wrappers (`loctree-mcp/`, `loctree-lsp/`) were removed from
> this tree — they were never published and the runtime ships MCP/LSP as
> co-process binaries inside `@loctree/loctree`'s platform packages. None of the
> published packages declare lifecycle scripts (npm 11+ `allowScripts` blocks
> unapproved postinstalls; delivery is pure `optionalDependencies`).

## Repo maintenance workflow

```bash
# Bump the 5 package.json files to the workspace Cargo version.
node distribution/npm/sync-version.mjs 0.13.0

# Verify all versions match.
node distribution/npm/sync-version.mjs --check 0.13.0
```

See [PUBLISHING.md](./PUBLISHING.md) for the full publish flow.

## License

All packages ship under [BUSL-1.1](./LICENSE).

## Links

- Source: https://github.com/Loctree/loctree-suite
- Website: https://loct.io
- Issues: https://github.com/Loctree/loctree-suite/issues
