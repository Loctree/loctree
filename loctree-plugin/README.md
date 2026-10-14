# loctree — structural perception for AI agents

> **Canonical in-suite agent plugin.** Do not use the deprecated stub at
> `loctree-suite/plugin/` (redirect only). Sibling remote
> `github.com/Loctree/loctree-plugin` is marketplace packaging history —
> suite truth is this directory.

> **No edit without orientation. No delete without impact. No create without search.**

`loctree` is a Claude Code plugin that wraps the [loctree](https://github.com/Loctree/loctree) toolchain (`loct` CLI, `loctree-mcp` MCP server, `loctree-lsp` LSP daemon) and turns it into the agent's default perception layer. The model is rust-analyzer for AI agents: live tree-sitter intelligence for JS/TS/TSX, snapshot-based polyglot intelligence for everything else (Rust, Python, Shell, Make, Astro, Svelte, Vue, Zig, …).

## Why this exists

Vibe-coded codebases overgrew. Agents grep, agents create files that already exist, agents delete files with 30 transitive consumers. Each round of guesswork costs context, costs trust, costs time. `loctree` fixes the ground truth: every edit is preceded by `slice`; every delete is preceded by `impact`; every create is preceded by `find`. Hooks make it automatic. Slash commands make it ergonomic. Agents make it autonomous.

## Components

| Layer | What | When fires |
|---|---|---|
| **MCP** | `loctree-mcp` (canonical 12 tools, polyglot snapshot) + `loctree-lsp` (live tree-sitter, JS/TS/TSX) | continuous |
| **Skills (slash commands)** | `/loctree`, `/loctree:context`, `:slice`, `:impact`, `:find`, `:focus`, `:follow`, `:repo-view`, `:tree` | user-invoked |
| **Agents** | `structural-reviewer`, `pre-edit-context`, `refactor-impact-scout` | task-dispatched |
| **Hooks** | SessionStart context card · PostToolUse Grep/rg augment · PostToolUse Edit warning · PostToolUse Read context · PreToolUse Bash smart-suggest | event-driven, automatic |
| **Codex parity** | `codex/AGENTS.md` + agent mirrors + hook stubs | cross-tool consumption |

## Doctrine

The plugin enforces the perception triad from the [Vetcoders Charter](https://github.com/vetcoders/vibecrafted):

1. **Perception over memory.** Read the snapshot, not the training data. Files have changed since the LLM saw them.
2. **Intentions retrieval over RAG.** Find the *why* of past decisions before re-deciding them.
3. **Ground truth over intuition.** Run the gates; don't claim "tests pass" without `cargo test`.

`/loctree` is the orchestrator. Read its skill before any major operation.

## Requirements

- `loct` CLI v0.13.0+ on `PATH`
- `loctree-mcp` v0.13.0+ on `PATH` (declared in `.mcp.json`)
- `loctree-lsp` v0.13.0+ on `PATH` for live AST features (JS/TS/TSX only); fallback is snapshot-based
- `jq` (used by hooks for JSON parsing)
- Claude Code or Codex compatible host

Verify:

```bash
loct --version          # 0.13.0+
loctree-mcp --version   # 0.13.0+
loctree-lsp --version   # 0.13.0+
```

If any binary is missing, install **prebuilt binaries** (no Rust toolchain required):

```bash
# Recommended — signed bundle (full target bundle: loct + loctree + loctree-mcp + loctree-lsp + aicx + aicx-mcp)
curl -fsSL https://loct.io/install.sh | bash

# Or npm runtime package (loctree/loct plus sibling MCP/LSP binaries)
npm install -g @loctree/loctree

# Or via Homebrew for the core CLI; MCP/LSP formulae follow the thin-repo release tracks
brew install loctree/cli/loct
```

> **Bundle note (0.13.0):** full target bundles ship six binaries; the `x86_64-unknown-linux-musl-core` bundle carries static Loctree binaries and marks AICX as an optional runtime dependency.

Contributors building from source (cargo / `make install` from the parent `loctree-suite` workspace) — see [docs/dev/01_installation.md](https://github.com/Loctree/loctree-suite/blob/main/docs/dev/01_installation.md).

## Installation

### Claude Code (host)

From inside an interactive Claude Code session:

```text
/plugin install /path/to/loctree-suite/loctree-plugin
```

Or symlink directly into the user-level plugin dir:

```bash
ln -s /path/to/loctree-suite/loctree-plugin ~/.claude/plugins/loctree
```

(Exact install path depends on your Claude Code version — see your host's plugin docs.)

### Codex (host)

The plugin emits a `codex/` directory with agent definitions and skill mirrors that codex consumes natively. Point your codex config at `loctree-plugin/codex/`:

```bash
ln -s /path/to/loctree-suite/loctree-plugin/codex/AGENTS.md ~/.codex/loctree-AGENTS.md
ln -s /path/to/loctree-suite/loctree-plugin/codex/skills    ~/.codex/loctree-skills
```

## First run

1. Open any source file in a repo that has `.loctree/` (run `loct repo-view` first if not).
2. The SessionStart hook injects `loct context` Agent Context Pack into the conversation.
3. The PostToolUse hooks fire on every Read/Edit/Grep/rg, surfacing structural context as `additionalContext`.
4. Try the orchestrator: `/loctree` and let the agent walk you through perception-before-action.

## Configuration

Create `.claude/loctree.local.md` in your project to tune behavior:

```yaml
---
hooks:
  context_card: true       # SessionStart on startup|clear|compact
  grep_augment: true       # PostToolUse on Grep, Bash(rg ...)
  edit_warning: true       # PostToolUse on Edit, with critical-threshold
  read_context: true       # PostToolUse on Read
  smart_suggest: true      # PreToolUse on Bash (hint-only stderr; complementary to grep_augment, not exclusive)

thresholds:
  edit_warning_critical: 10   # direct consumers triggering systemMessage
  context_card_ttl_seconds: 1800

paths:
  loct_log: ~/.claude/logs/loct.log
---
```

`grep_augment` and `smart_suggest` are **complementary**, not exclusive: `smart_suggest` fires *before* a Bash command runs (printing a `loct find` hint to stderr for the operator), while `grep_augment` fires *after* a `rg`/`grep` result lands (injecting structural context into the agent's reasoning). They serve different surfaces and can both be on. Disable either independently if you find one too noisy.

The file is committed-by-default-ignored (`.gitignore` has `.claude/*.local.md`); only the hook scripts read it.

## Troubleshooting

- **No structural context after Read/Edit** — Verify `.loctree/` exists at repo root: `loct repo-view`. If missing, snapshot needs first scan.
- **`loctree-lsp --version` works but no live AST** — Live tree-sitter is JS/TS/TSX only. Other languages use snapshot.
- **Cold-start LSP returns `-32001`** — `initialize` is non-blocking; the snapshot loads in background. Retry the request after a few seconds, or trust the `loct` CLI fallback in agents.
- **Hooks fire but no output** — Check `~/.claude/logs/loct-*.log`. Most hooks log invocations; missing log = hook never matched.
- **Multiple plugin instances** — `loctree-lsp` is stdio-only with no socket sharing. Each Claude Code session spawns its own daemon. Use `--debug` flag in `.mcp.json` for visibility.
- **`extensionToLanguage` in `.mcp.json` looks like dead config** — Claude Code does not yet parse this field; codex hosts that wire LSP-via-MCP do. The field is forward-compatible scaffolding for hosts that adopt the convention. Until Claude Code parses it, `loctree-lsp` is invoked with default args and routes by working directory.
- **Windows host** — v0.1.0 hooks invoke `bash` directly. Windows users need Git Bash or WSL on PATH. The `run-hook.cmd` polyglot wrapper is shipped for v0.2.0 wiring; not yet active in `hooks.json`.

## Authoring

This plugin lives at [loctree/loctree-plugin](https://github.com/Loctree/loctree/tree/main/loctree-plugin). Issues and PRs welcome.

## License

This plugin is MIT-licensed (see [LICENSE](LICENSE)); the Loctree runtime it
drives (`loct`, `loctree-mcp`, `loctree-lsp`) is licensed under BUSL-1.1.

## Author

Vetcoders · `agents@vetcoders.io` · https://vetcoders.io

## Version

0.1.0 — first plugin ship. It drives the current stable Loctree runtime (see
https://loctree.com/releases/index.json) and its canonical 12-tool MCP surface.
