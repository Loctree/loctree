# Changelog

All notable changes to the `loctree` Claude Code plugin are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) ·
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Advisory Loctree-first guard** (`hooks/loctree-first-guard.py`, PreToolUse Bash, runs before `loct-smart-suggest.sh`) — never blocks a command and never mutates its result. A standalone repo-wide grep/rg (the one shape that usually stands in for a structural question) runs unchanged and gets a short loct recipe attached as `additionalContext` (symbols via `loct find`, raw text via `loct find --regex` with coverage accounting, `loct slice`/`impact`/`focus` for structure) plus the `loctree-feedback.md` feedback loop. File targets, subdirectory targets, file listing (`rg --files`), pipe filters, out-of-repo searches, and deliberate `command grep`/`command rg` draw no note. The command splitter is quote- and heredoc-aware, so commit messages and heredoc bodies never trigger it. The distributed default is advisory — a shipped hook smuggles knowledge, it does not enforce. Strict mode (`LOCTREE_FIRST_GUARD=strict`, the zero-fallback posture: denies in-repo grep/rg including `command grep`, with the recipe map + `loctree-feedback.md` contract) stays available for machines that build the product. Kill-switch: `LOCTREE_FIRST_GUARD=0`; parsing uncertainty stays silent.

## [0.1.0] — 2026-05-08

First ship. Plugin packages the canonical 27→8 polarized loctree MCP surface plus the operator's curated perception layer (5 hook scripts originally maintained in `~/.claude/hooks/`) into a redistributable Claude Code plugin with first-class Codex parity.

### Added

- **MCP wiring** (`.mcp.json`) — declares both `loctree-mcp` (stdio, polyglot snapshot via canonical 8 tools: `context`, `repo-view`, `slice`, `find`, `impact`, `tree`, `focus`, `follow`) and `loctree-lsp` (stdio, live tree-sitter for JS/TS/TSX) with `extensionToLanguage` mapping. First plugin in the cache shipping LSP-via-MCP — the field is honored by hosts that parse it (codex), silently ignored by Claude Code today.
- **9 skills** under `skills/` — orchestrator (`loctree`) plus 8 thin slash-command wrappers (`loctree-context`, `loctree-slice`, `loctree-impact`, `loctree-find`, `loctree-focus`, `loctree-follow`, `loctree-repo-view`, `loctree-tree`). Each skill is third-person triggered, written FOR the agent, with explicit perception-before-action discipline.
- **3 agents** under `agents/` — `structural-reviewer` (senior pre-merge audit), `pre-edit-context` (briefing-as-task before Edit/Write), `refactor-impact-scout` (phased migration planner). All `model: inherit`, scoped tool sets.
- **5 hooks** ported with `${CLAUDE_PLUGIN_ROOT}` portability: `loct-context-card.sh` (SessionStart, 30-min TTL cache), `loct-grep-augment.sh` (PostToolUse Grep|Bash, 9-strategy router), `loct-edit-warning.sh` (PostToolUse Edit, hub-warning at ≥10 importers), `loct-read-context.sh` (PostToolUse Read), `loct-smart-suggest.sh` (PreToolUse Bash, hint-only stderr).
- **Polyglot wrapper** (`hooks/run-hook.cmd`) reserved for Windows portability in v0.2.0; not yet wired in `hooks.json`.
- **Codex parity** (`codex/AGENTS.md` + `codex/skills/README.md` + `codex/hooks/README.md`) — agent-agnostic distribution with explicit "Claude Code-primary, codex-secondary" labeling for hooks.
- **MIT License** + `.gitignore` covering `.env*`, `.loctree/`, `.cache/`, `.claude/*.local.md`.

### Doctrine

Three axioms enforced end-to-end:

1. **Perception over memory** — read the snapshot, not training data.
2. **Intentions retrieval over RAG** — find the *why* of past decisions before re-deciding them.
3. **Ground truth over intuition** — run the gates; `cargo test` before claiming "tests pass".

### Known limitations

- Live tree-sitter (via `loctree-lsp`) covers JS/JSX/TS/TSX/CTS/MTS only. All other languages (Rust/Python/Shell/Make/Astro/Svelte/Vue/Zig/CSS/...) are served via snapshot through `loctree-mcp` — still polyglot, just not live-buffer-aware.
- `loctree-lsp` has no socket transport; each Claude Code session spawns its own daemon. Cold-start synchronously loads snapshot — first custom request after `initialize` may return `-32001` until ready.
- `extensionToLanguage` is parsed by codex hosts and ignored by Claude Code today. The field is forward-compat for Claude Code adopting LSP-via-MCP wiring.
- Windows host support is partial — bash-only hooks; `run-hook.cmd` polyglot wrapper exists but is not yet wired through `hooks.json`. WSL or Git Bash recommended for v0.1.0.

### Authorship

Plugin authored by `claude` (Opus 4.7) at the operator's direction. Underlying perception layer (5 hooks) authored by `claude` over multiple prior sessions and curated by the operator at `~/.claude/hooks/`. Canonical 27→8 MCP polarization authored by `claude/vc-agents`. LSP CLI flags (`--version`/`--help`/`--capabilities`) authored by `codex` in run `polr-183603-83092`.

`Authored-By: claude <agents@vetcoders.io>` for first commit on this surface.
