# Loctree Architecture

## Doctrine
Loctree is the structural perception and context-compiler layer for agentic software work.
It tells agents what the code is, how it is connected, what runtime surface it participates in, and what context is safe to act on now.
The parser is treated as a sensor, not the product; ground truth and derived views are kept explicit; and the agent context pack is the core UX.

## Layer 1 — Sensors
- OXC (JS/TS) — primary cold-scan parser
- regex parsers (shell, makefile, zig, dart, go, css)
- tree-sitter substrate (`loctree-ast` crate): JS/TS/TSX live AST + structural query, consumed by the LSP editor layer
- tree-sitter C-family extraction (`loctree-rs` analyzer — `analyzer/swift.rs`, `semantic/c_family.rs`, deps in `loctree-rs/Cargo.toml`): Layer 1 symbol extraction for Swift/ObjC/C/C++ with heuristic provenance. Distinct from `loctree-ast`; these grammars do not live in that crate.
- both tree-sitter paths are sensors, not the product; neither generates the snapshot — the snapshot is composed from sensor facts
- artifact metadata: source maps, gitignore, repo identity

## Layer 2 — Symbol Model
- types.rs::FileAnalysis
- export/import edges
- node ranges + sensor tags

## Layer 3 — Runtime Semantics
See [docs/semantic-spec.md](semantic-spec.md) for idiom catalog.
- ShellSemantics (shipped)
- MakeSemantics (shipped)
- PythonRuntimeSemantics (shipped) — `loctree-rs/src/semantic/python.rs`
- RustRuntimeSemantics (shipped) — `loctree-rs/src/semantic/rust.rs`
- TauriSemantics (shipped) — `loctree-rs/src/semantic/tauri.rs`
- contract: trait `RuntimeSemanticAnalyzer`, types `SemanticFacts` / `IdiomTag` / etc.
- orchestration: `compute_semantic_facts` in `semantic/mod.rs` runs all five analyzers; each self-filters by `FileAnalysis::language`
- idiom catalogs (embedded defaults, overridable via `<workspace>/.loctree/idioms/*.toml`):
  - `shell.toml`, `make.toml` — catalogued in `docs/semantic-spec.md`
  - `python.toml`, `rust.toml`, `tauri.toml` — present in `semantic/idioms/`; Python/Rust/Tauri prose catalogs still to be written into the spec

> **Honesty note (2026-08-30):** this section previously read `queued: PythonRuntimeSemantics, RustRuntimeSemantics, TauriSemantics` with a last-reviewed stamp of 2026-06-28. That stamp was stale — the three analyzers, their idiom TOMLs, and the `compute_semantic_facts` wiring have been on `main` since the 0.14.x series. The code is the source of truth; this doc now matches it. Remaining honest gaps: the Python/Rust/Tauri *prose* catalogs in `semantic-spec.md` (only shell + make are documented there today), and the deferred items listed inside each analyzer's module docs (type-aware reachability, macro expansion, tree-sitter migration).

## Layer 4 — Agent Context Pack
- `loct context` (shipped): the agent-ready output of the snapshot, and the core UX
- composition: structural + runtime + risk + action + memory + authority slices
- `--with-aicx` overlay (shipped): attaches AICX memory continuity

## Surfaces — same snapshot, different doors
The snapshot is the single structural authority. CLI (`loct`/`loctree`), MCP
(`loctree-mcp`, 12 tools), and LSP (`loctree-lsp`, 15 `loctree/*` custom
requests) all expose it; `./editors` (VS Code, JetBrains, Neovim) surface it
where the user works. Live AST in the LSP is an editor-time freshness layer over
the same authority — see [integrations/lsp-server.md](integrations/lsp-server.md).

## Cross-cutting
- Snapshot identity & rebuildability (Cut 2)
- Doctor surfaces (Cut 2)
- Reports / Leptos cockpit (Lane 4)
- Note: AICX integration depends on stable interfaces; live `memex-sync` drift must be resolved before use.

## Last reviewed: 2026-08-30
