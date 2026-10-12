# Loctree Suite - Architecture

Technical architecture of the loctree-suite monorepo.

> **Canonical architecture lives in [`../architecture.md`](../architecture.md)** (Layer 1–4 plus the snapshot-first
> "same snapshot, different doors" surface model) and **[`../integrations/lsp-server.md`](../integrations/lsp-server.md)**
> (LSP / live-AST doctrine). The **Workspace Overview** crate inventory below is current. Sections after it still
> describe the pre-0.13.0 layout (with `loctree-memex`, `rmcp-memex`, `rmcp-mux`, `landing`) and are retained as
> historical until a separate refresh sweep — do not treat them as current.

## Workspace Overview

```
loctree-suite/                    # Cargo workspace root
├── Cargo.toml                    # Workspace manifest (members listed there)
├── Makefile                      # Build automation
│
├── loctree-ast/                  # Narrow tree-sitter substrate (JS/TS/TSX live AST + structural query)
│   └── src/lib.rs                # Sensor only — does not generate the snapshot

│
├── loctree-rs/                   # Core library + CLI
│   ├── src/lib.rs                # Public API
│   ├── src/bin/loct.rs           # Compact operator CLI
│   └── src/bin/loctree.rs        # Full analyzer/reporting CLI
│
├── loctree-mcp/                  # MCP server crate (loctree-mcp binary)
│   └── src/main.rs               # stdio MCP transport
│
├── loctree-lsp/                  # LSP server crate (loctree-lsp binary)
│   └── src/main.rs
│
├── reports/                      # Leptos HTML reports
│   ├── src/lib.rs                # Report generation
│   └── wasm/                     # Browser-side hydration
│
├── distribution/                 # npm wrappers, codesigning, Homebrew formulas
└── editors/                      # VS Code extension, JetBrains plugin, Neovim plugin
```

Crates removed from the workspace (kept in their own repos): `rmcp-memex` (now on crates.io), `landing` (split into
`Loctree/loct-io`), `loctree-memex` and `rmcp-mux` (deprecated).

## Crate Dependency Graph

```
                       ┌──────────────┐
                       │ loctree-ast  │  (cross-language AST surface)
                       └──────┬───────┘
                              │
                              ▼
                       ┌──────────────┐
                       │   loctree    │  (core library)
                       └──────┬───────┘
                              │
              ┌───────────────┼───────────────┐
              │               │               │
              ▼               ▼               ▼
       ┌────────────┐ ┌──────────────┐ ┌──────────────┐
       │ loctree-mcp│ │ loctree-lsp  │ │ report-leptos│
       └────────────┘ └──────────────┘ └──────────────┘
```

## Crate Details

### loctree (loctree-rs)

**Version line**: 0.13.0 (workspace-pinned)
**Type**: Library + 2 binaries
**Dependencies**: Pure Rust (no external runtime deps)

Core static analysis library:

- Multi-language parsing (TS/JS, Python, Rust, Go, C/C++)
- Dependency graph construction
- Dead code detection
- Cycle detection
- Code duplication (twins) analysis

**Binaries**:

- `loct` — compact operator CLI (recommended for daily use)
- `loctree` — full analyzer/reporting CLI (legacy long name; superset of `loct`)

**Key modules** (snapshot-first; the snapshot is the structural authority every surface reads):

```
src/
├── snapshot.rs       # Snapshot authority: persistence, schema/fingerprint, staleness, git + project identity, cache
├── pack.rs           # Context Pack: compose_context_pack → structural/runtime/risk/action/memory/authority slices
├── atlas.rs          # Materializes the on-disk Context Atlas cards from the pack
├── types.rs          # FileAnalysis + symbol/edge model (highest fan-in hub)
├── analyzer/         # Scan, classify, dead/cycles/twins, occurrences, env-truth, manifests, C-family (tree-sitter)
├── semantic/         # Runtime idioms per language (shell, make, python, rust, tauri) — heuristics, not a snapshot parser
├── aicx/             # AICX intent/memory overlay
├── slicer.rs / impact.rs / focuser.rs / query.rs   # Read surfaces over the snapshot
└── cli/              # `loct` / `loctree` command dispatch (context, slice, impact, find, follow, ...)
```

Parser sensors: OXC for JS/TS cold-scan; regex parsers for shell/make/zig/dart/go/css; tree-sitter for C-family
(`analyzer/c_family_syntax/`) and, via the `loctree-ast` crate, JS/TS/TSX live AST. The parser is a sensor — the
snapshot is the product.

### loctree-mcp

**Version line**: 0.10.x (workspace-pinned)
**Type**: Binary only
**Dependencies**: rmcp, loctree

MCP server exposing the same snapshot-backed structural truth to AI agents.
**12 tools** (not a mirrored CLI; see [integrations/mcp-server.md](../integrations/mcp-server.md)):

- `context` - Complete Agent Context Pack + Context Atlas pointer
- `repo-view` - Overview: files, LOC, languages, health, top hubs
- `focus` - Module deep-dive
- `slice` - File context with deps + consumers (before edit)
- `body` - Bounded symbol body/range from the shared library layer
- `find` - Symbol / import / literal / raw-text regex search
- `impact` - Blast radius (before delete/refactor)
- `diff` - Git-ref to current live-snapshot comparison
- `tree` - Directory layout with LOC counts (unlimited depth by default)
- `follow` - Structural signals: dead, cycles, twins, hotspots, pipelines
- `suppressions` - Source-side silencer inventory
- `prism` - Conceptual-smear score for `vc-polarize` gating

The shared `body` extractor searches for a body opener within the requested
line cap, including Rust signatures longer than ten lines. The opener scan
ignores comments and quoted strings and tracks signature delimiters and
const-generic expressions. A prototype or following function declaration
cannot supply a missing opener. An unproved boundary retains `extent=window`
and `truncated=true`; a larger cap reports a complete body only when the opener
and closing boundary are proved. The existing body balancer and output cap
remain authoritative.

**Version**: 0.1.11
**Type**: Library + binary
**Dependencies**: rmcp, lancedb (embeddings via external providers)

RAG/memory MCP server:

- Vector storage with LanceDB
- Document indexing (PDF, text, markdown)
- Semantic search with reranking
- Namespace-based memory isolation

**CLI commands**:

```bash
```

**MCP tools**:

- `rag_index` - Index document
- `rag_index_text` - Index raw text
- `rag_search` - Semantic search
- `memory_upsert` - Store memory chunk
- `memory_get` - Retrieve by ID
- `memory_search` - Semantic memory search
- `memory_delete` - Delete chunk
- `memory_purge_namespace` - Clear namespace

**Version**: 0.3.3
**Type**: Library + binary
**Dependencies**: rmcp, tokio, ratatui

MCP server multiplexer - **single process manages ALL servers**:

- One daemon process for all configured MCP servers
- Unix socket communication per server
- Automatic server lifecycle management
- TUI dashboard for monitoring
- Lazy loading support (spawn on first request)
- Heartbeat monitoring with auto-restart

**Binary**:

**CLI flags**:

- `--config` - Path to mux.toml
- `--only` - Start only specific servers
- `--except` - Exclude specific servers
- `--show-status` - Show status and exit
- `--restart-service` - Restart single service
- `proxy --socket` - Bridge STDIO to socket

### reports

**Version**: 0.1.9
**Type**: Library (WASM)
**Dependencies**: leptos

Leptos-based HTML report generation:

- Interactive dependency graphs
- Health dashboards
- Export to standalone HTML

### landing

**Type**: WASM application
**Dependencies**: leptos, trunk

Marketing landing page built with Leptos.

## Data Flow

### Loctree Analysis

```
Project Files
     │
     ▼
┌─────────────┐
│   Parser    │  (OXC, tree-sitter, syn)
└──────┬──────┘
       │
       ▼
┌─────────────┐
│    Graph    │  (nodes: files, edges: imports)
└──────┬──────┘
       │
       ▼
┌─────────────┐
│  Analysis   │  (dead code, cycles, twins)
└──────┬──────┘
       │
       ▼
┌─────────────┐
│  Artifacts  │  (.loctree/*.json)
└─────────────┘
```

```
Document
    │
    ▼
┌──────────────┐
│ Text Extract │  (PDF, markdown, code)
└──────┬───────┘
       │
       ▼
┌──────────────┐
│   Chunker    │  (512 chars, 128 overlap)
└──────┬───────┘
       │
       ▼
┌──────────────┐
│  Embedder    │  (FastEmbed / MLX)
└──────┬───────┘
       │
       ▼
┌──────────────┐
│   LanceDB    │  (vector storage)
└──────────────┘
```

## Storage Locations

```
~/.rmcp_servers/
├── config/
├── logs/
│   └── mux.log              # Unified mux log
├── pids/
│   └── mux.pid              # Single PID for mux daemon
├── sockets/
│   ├── loctree.sock         # Per-server Unix sockets
│   └── ...
    └── lancedb/             # Vector storage

~/.config/
├── claude/
│   └── claude_desktop_config.json
└── cursor/
    └── mcp.json

/tmp/
└── loctree-make.lock        # Build lock
```

## Configuration Files

### Loctree

```
project/
├── .loctignore              # Files to ignore (gitignore syntax)
└── .loctree/
    ├── manifest.json        # Artifact index
    ├── snapshot.json        # Full graph data
    ├── findings.json        # Issues (dead, cycles, twins)
    └── agent.json           # AI-optimized bundle
```

```toml
mode = "full"
cache_mb = 4096
```

## Build Profiles

```toml
# Cargo.toml [profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
```

## Testing

```bash
# All tests
cargo test --workspace

# Specific crate
cargo test -p loctree

# With output
cargo test -- --nocapture
```

## Feature Flags

```toml
[features]
default = ["cli", "tray"]
cli = ["clap", "ratatui", "crossterm"]
tray = ["tray-icon"]
```

## External Dependencies

| Dependency  | Used By             | Purpose                       |
|-------------|---------------------|-------------------------------|
| OXC         | loctree             | TypeScript/JavaScript cold-scan parsing |
| tree-sitter | loctree, loctree-ast | C-family (Swift/ObjC/C/C++) Layer 1; JS/TS/TSX live AST substrate |
| syn         | loctree             | Rust parsing                  |
| Leptos      | reports, landing    | WASM UI                       |
| rmcp        | loctree-mcp, rmcp-* | MCP protocol                  |

---

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
