# Loctree for JetBrains

Native JetBrains IDE integration for the [`loctree-lsp`](../../loctree-lsp)
language server: one-shot full repository context for AI agents, surfaced
directly inside IntelliJ-based IDEs. Agents go BIG when they see: Context Pack,
literal occurrences, symbol bodies, impact, slice, and findings signals all
flow through the LSP.

This module is a **sibling editor integration** alongside
[`editors/vscode`](../vscode). It is a Gradle/Kotlin project and is
intentionally **not** a member of the Rust Cargo workspace.

## Requirements

- A **paid JetBrains IDE** on the **2025.2.1+** line (build `252.1+`):
  IntelliJ IDEA Ultimate, WebStorm, PyCharm Professional, GoLand, RustRover,
  Rider, PhpStorm, RubyMine, or CLion. Community editions do not ship the
  native LSP module (`com.intellij.modules.lsp`) this plugin depends on.
  Pre-2025.2.1 native LSP support used the older Ultimate-module dependency
  and is not mixed into this ZIP.
- No separate `loctree-lsp` install is required for normal use. The plugin is
  **download-only**: the ZIP ships no binary, and the server is fetched once
  from GitHub Releases for the current OS/arch with a mandatory SHA256 check,
  then cached in the IDE. A local build can be pointed at via **Server path**.

## What it does

- Starts `loctree-lsp` automatically when the IDE project opens, launched with
  `--root <projectBasePath>`. Opening supported files (`.ts`, `.tsx`, `.js`,
  `.jsx`, `.mjs`, `.cjs`, `.rs`, `.py`, `.go`) still flows through the native
  `LspServerSupportProvider`, but the tool window and custom requests no longer
  wait for a file-open event.
- Standard LSP features (diagnostics, hover, go-to-definition,
  references, code actions, code lenses) are handled by the platform.
- Loctree-specific tooling goes through a typed gateway
  (`LoctreeLspGateway`) issuing custom `loctree/*` requests:
  `health`, `find`, `body`, `impact`, `slice`, `contextAtlas`, `contextPack`,
  `follow`, `workspaces`, `diff`, `semantic`, `aicx`, `astQuery`, and the
  `refresh` notification. The gateway also exposes a raw custom-request router
  so new read-side LSP surfaces can be consumed without bypassing the native
  LSP transport.
- Literal search is available in the IDE through **Find Literal
  Occurrences**. It uses `loctree/find` with `mode=literal`,
  `whole_token`, `group_by_file`, `limit`, and `offset`, so the IDE can
  consume the same classified, denoised, paginated occurrence surface as
  MCP `find(mode="literal")` and CLI `loct occurrences` / `loct find --literal`.
- The Loctree tool window includes an inline Context-King query row. Modes route
  to `contextPack`, literal `find`, `body`, `impact`, `slice`, `find`, `follow`,
  `aicx`, `semantic`, and `astQuery`; results render in the tool window and keep a
  **Load More** continuation when the response exposes `next_cursor` or literal
  `next_offset`.
- `loctree/contextPack` is the ACP-facing context bridge for IDE consumers. It
  streams Context Atlas cards one section at a time with `cards`, `scope`,
  `task`, `with_aicx`, and cursor continuation fields, aligned with MCP
  `/context_pack`.
- UI parity with the VS Code extension:
  - **Tool window** (right dock) centered on Context Pack / Literal / Body /
    Impact / Slice. Legacy **dead exports**, **circular imports**, and **twins**
    remain available as a secondary signal with loading/empty/error states and
    double-click navigation.
  - **Status bar widget** reflecting disconnected / downloading /
    running / healthy / error.
  - **Context actions** (editor + project view): Analyze Impact, Show
    Slice, Find Consumers/Importers, Check Dead Exports, Show/Analyze
    Cycles, Find Literal Occurrences, Refresh, Show Health, Open Report.
  - **Compact status bar icon** using a fixed-size IDE painting path rather
    than the marketing SVG, so SVG viewport metadata cannot widen the
    bottom status bar widget.

## ACP-facing LSP contract

Agents embedded in or attached to the IDE should call the LSP gateway, not
scrape Swing UI state. The stable read-side request set is:

```json
{
  "method": "loctree/contextPack",
  "params": {
    "task": "implement literal search UI",
    "cards": ["core", "structural", "risk"],
    "with_aicx": true
  }
}
```

```json
{
  "method": "loctree/find",
  "params": {
    "query": "LoctreeLspGateway",
    "mode": "single",
    "limit": 25
  }
}
```

Use `loctree/find`, `loctree/follow`, `loctree/semantic`,
`loctree/astQuery`, and read-only `loctree/aicx` for targeted follow-up
queries. Write-side AICX recording and CLI-only long-tail commands are explicit
future work unless they first gain an LSP capability.

## Runtime resolution (verified)

Resolution order mirrors the VS Code resolver. A manual path is an override only
when it points to a real executable file or directory containing the executable;
stale settings fall through instead of blocking startup.

1. valid user-configured **Server path** (Settings ▸ Tools ▸ Loctree),
2. bundled plugin binary — *dev-only opt-in builds; released ZIPs never carry
   one, so this step is a no-op for Marketplace installs*,
3. IDE cache (binary plus SHA256 sidecar, re-verified before every use),
4. **verified download** from GitHub Releases,
5. preferred user install (`~/.local/bin/loctree-lsp`),
6. exact executable resolved from `PATH`.

The status widget tooltip and IDE log expose the exact executable path, its full
`--version` build identity, and the resolver source. If an older Cargo/Homebrew
copy appears earlier on `PATH` than the preferred user install, Loctree uses the
preferred runtime and emits a visible PATH-shadowing warning with both paths and
identities.

The asset matrix lists only the binaries actually published on
[`Loctree/loctree-release`](https://github.com/Loctree/loctree-release) — the
same release assets produced by the canonical bundle workflow:

| OS / arch        | asset                              |
|------------------|------------------------------------|
| macOS arm64      | `loctree-lsp-darwin-arm64`         |
| macOS x64        | `loctree-lsp-darwin-x64`           |
| Linux x64        | `loctree-lsp-linux-x64`            |
| Windows x64      | `loctree-lsp-windows-x64.exe`      |

On any other platform (Windows arm64, Linux arm64, …) there is no published
runtime: the plugin shows an IDE notification naming the unsupported
platform and pointing at the manual install path (build or install
`loctree-lsp` yourself, then set **Server path** in Settings ▸ Tools ▸
Loctree).

**Security — SHA256, fail-closed.** The downloader fetches the
`<asset>.sha256` checksum alongside the binary and verifies it **before**
marking the file executable. Successful downloads write a local
`<binary>.sha256` sidecar; cached binaries are re-hashed against that sidecar
before every use. A missing or mismatched checksum aborts the download or
ignores the cache entry — unverified bytes are never executed. This mirrors the
VS Code resolver's cache/download chain.

## Settings

Settings ▸ **Tools ▸ Loctree** (mirrors the VS Code configuration):

| Setting              | Default    | Meaning                                            |
|----------------------|------------|----------------------------------------------------|
| `serverPath`         | *(empty)*  | Optional override; empty = cache/verified download/PATH. |
| `autoRefresh`        | `false`    | Refresh analysis on file save.                     |
| `showStatusBar`      | `true`     | Show the status bar widget.                         |
| `autoDownload`       | `true`     | Download `loctree-lsp` when not found locally.      |
| `downloadBaseUrl`    | `https://github.com/Loctree/loctree-release` | Override the GitHub repo for downloads. |
| `downloadTag`        | *(empty)*  | Release tag (e.g. `v0.14.5-rc1`). Empty pins to the plugin's own version; `latest` is an explicit opt-in. |
| `diagnosticSeverity` | `WARNING`  | Severity for dead-export diagnostics.              |

## Build & test

```bash
# from the repository root
make editors-jetbrains         # daily: tests + distributable plugin ZIP
make editors-jetbrains-full    # release: daily build + Plugin Verifier
make editors-jetbrains-install # local reinstall into latest IntelliJIdea config

# override install target when needed
make editors-jetbrains-install \
  EDITORS_JETBRAINS_CONFIG="$HOME/Library/Application Support/JetBrains/IntelliJIdea2026.1"
```

> The first build downloads the IntelliJ Platform SDK and Plugin Verifier
> IDEs; this requires network access and several hundred MB of cache.

Default builds are **download-only** — identical to what CI publishes, with no
`loctree-lsp` inside the ZIP. To bake a locally built server into the plugin for
development (never for publishing), opt in explicitly:

```bash
cd editors/jetbrains
./gradlew buildPlugin -PbundleLsp=true                       # builds loctree-lsp via cargo
LOCTREE_LSP_PATH=~/.local/bin/loctree-lsp \
  ./gradlew buildPlugin -PbundleLsp=true                     # or reuse an existing binary
LOCTREE_BUNDLE_LSP=1 make editors-jetbrains-install          # env form, for make targets
```

## Release (operator stop-points)

Signing and publishing never happen as a consequence of a push. They are a
button: `.github/workflows/jetbrains-publish.yml` runs on `workflow_dispatch`
only, defaults to `dry_run=true`, and fails fast when a Marketplace secret is
missing rather than silently skipping.

```bash
gh workflow run jetbrains-publish.yml -f dry_run=true  -f channel=stable  # rehearsal
gh workflow run jetbrains-publish.yml -f dry_run=false -f channel=stable  # live
```

The same environment variables drive it locally:

- Signing: `CERTIFICATE_CHAIN`, `PRIVATE_KEY` (both **file paths**),
  `PRIVATE_KEY_PASSWORD` → `./gradlew signPlugin`.
- Publishing: `PUBLISH_TOKEN`, optional `PUBLISH_CHANNEL` (`default` = stable)
  → `./gradlew publishPlugin`.

`jetbrains-plugin.yml` builds, tests, and verifies on every push without any
Marketplace secrets. Read
[docs/release/jetbrains-marketplace.md](../../docs/release/jetbrains-marketplace.md)
— including its **Blockers** section — before the first real publish.

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Vetcoders
