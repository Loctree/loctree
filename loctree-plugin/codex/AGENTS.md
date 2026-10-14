<!-- loctree-advise: v1 -->
## **LOCTREE + AICX + VIBECRAFTED - MAP BEFORE MAGNIFIER**

> **Loctree first when it helps. Skip the map and you lose the context.**

Loctree is the default structural map for repository work, because at its
current quality skipping it usually costs time: dependencies, blast radius,
symbols, runtime entry points, dead surfaces, duplicates and exact
occurrences are all visible faster than through manual rummaging. AICX adds
the history of intent, and Vibecrafted closes the loop with proof
discipline.

**Operating rule:**

- For structural questions (who imports X, where does symbol Y live, what
  breaks after editing Z, blast radius, the layout of directory A) start
  with `loctree-mcp` (`context` / `repo-view` / `focus` / `slice` /
  `impact` / `find` / `follow`), or with the `loct` CLI when MCP is not
  available.
- Before editing a file prefer `slice(file)`. Before a delete/rename/
  refactor prefer `impact(file)`. Before creating a new symbol prefer
  `find(name)`.
- For literal truth use `find --literal`, `loct occurrences <ident>` and
  `loct body <symbol>` before reaching for a broad text search.
- `rg`, grep, awk, sed, cat and direct file reads are fine as a magnifier
  for local detail once the map is in hand, or as a fallback when Loctree
  does not answer the question cleanly.
- If Loctree misses, is stale, too slow, awkward, blind to a language,
  fails to capture an important surface, or you have an improvement idea,
  append a short note to the central feedback log.

**Central Loctree feedback log:**

- Append at the end of `~/internal-artifacts/loctree/loctree-feedback.md`.
- Never create the file from scratch and never overwrite it.
- An entry can be a bug, a missing feature, a UX suggestion, or a note on
  the spot where an agent had to drop to a fallback. Repeats are a
  priority signal, not a problem.

**Why:** Loctree turns agent work from text rummaging into map-first
engineering. The goal is not obedience theatre — it is fewer wrong edits,
better blast radius, faster recovery and more honest runtime decisions.
<!-- /loctree-advise -->

# Loctree — Agent Instructions for Codex

This file is the canonical entry point for Codex (and Codex-compatible agents) to consume the `loctree` plugin. Claude Code reads `.claude-plugin/plugin.json` and `skills/` directly; Codex reads this file and delegates to the same skill bodies through markdown reference.

## Doctrine

`loctree` enforces three axioms from the [Vetcoders Charter](https://github.com/vetcoders/vibecrafted):

1. **Perception over memory.** Read the snapshot, not the training data.
2. **Intentions retrieval over RAG.** Find the *why* before re-deciding it.
3. **Ground truth over intuition.** Run the gates; don't claim "tests pass" without `cargo test`.

## Available skills (slash commands)

When the user types one of these, read the corresponding SKILL.md file and follow it. Skill bodies are written FOR you, in imperative form.

| Slash | Skill body | Purpose |
|---|---|---|
| `/loctree` | `../skills/loctree/SKILL.md` | Orchestrator — perception-before-action |
| `/loctree:context` | `../skills/loctree-context/SKILL.md` | Materialize Context Atlas |
| `/loctree:slice <file>` | `../skills/loctree-slice/SKILL.md` | File deps + consumers + exports |
| `/loctree:impact <file>` | `../skills/loctree-impact/SKILL.md` | Direct + transitive blast radius |
| `/loctree:find <pattern>` | `../skills/loctree-find/SKILL.md` | Literal truth, definitions/importers, or opt-in discovery |
| `/loctree:focus <dir>` | `../skills/loctree-focus/SKILL.md` | Module deep-dive |
| `/loctree:follow <scope>` | `../skills/loctree-follow/SKILL.md` | Pursue dead/cycles/twins/hotspots/trace |
| `/loctree:repo-view` | `../skills/loctree-repo-view/SKILL.md` | One-shot repo overview |
| `/loctree:tree [path]` | `../skills/loctree-tree/SKILL.md` | Directory structure with LOC |

## Available agents

For autonomous task dispatch, three agents are defined in `../agents/`:

### structural-reviewer (`../agents/structural-reviewer.md`)
Senior-level structural review. Use proactively after a hub-file modification or before merge to a protected branch. Walks `repo-view` → `follow` (cycles, twins, dead, hotspots) → `impact` per finding, returns a P0/P1/P2/P3 verdict with concrete recommendations.

### pre-edit-context (`../agents/pre-edit-context.md`)
Briefing-as-task pattern. Dispatch right before an Edit/Write to absorb the perception cost — agent runs `slice` + `impact` (when destructive intent) + `find` (when creating/renaming) and returns a 250-word briefing. Parent stays clean for the actual edit.

### refactor-impact-scout (`../agents/refactor-impact-scout.md`)
Migration planner. Use BEFORE planning any module split, merge, public-API rename, or framework migration. Returns a phased PR-by-PR proposal with verification gates per phase.

## MCP tools available (via host runtime)

Codex hosts that wire `loctree-mcp` get these eight tools:

```
context, repo-view, slice, find, impact, tree, focus, follow
```

Hosts that also wire `loctree-lsp` get live tree-sitter awareness for JS/TS/TSX through the same tool surface (cold-start sync; first call after `initialize` may return `-32001` until snapshot loads — retry with backoff).

For Codex, the MCP wiring goes in your `~/.codex/config.toml` or equivalent:

```toml
[mcp_servers.loctree-mcp]
command = "loctree-mcp"

[mcp_servers.loctree-lsp]
command = "loctree-lsp"
```

## Hooks

Codex does not have a native hook system equivalent to Claude Code's `PreToolUse` / `PostToolUse` / `SessionStart`. The hook scripts under `../hooks/` are bash-portable and can be invoked manually by the operator or wired into Codex via session-prelude scripts:

```bash
# Suggested codex session prelude (~/.codex/prelude.sh)
bash /path/to/loctree-plugin/hooks/loct-context-card.sh
```

The hook scripts read JSON on stdin (Claude Code's contract) — for codex consumption you'd need a shim that synthesizes the expected input shape. That shim is **not** included in v0.1.0; treat hooks as Claude Code-primary, codex-secondary.

## When to use

- **Cold start in unfamiliar repo:** dispatch `/loctree` (orchestrator).
- **Before any non-trivial Edit/Write:** dispatch `pre-edit-context` agent OR call `/loctree:slice <file>` directly.
- **Before any deletion / rename / breaking-API change:** call `/loctree:impact <file>`.
- **Before creating a new symbol:** call `/loctree:find <name> mode:where-symbol` to check it doesn't exist.
- **Before any refactor:** dispatch `refactor-impact-scout` agent.
- **Before merge to protected branch:** dispatch `structural-reviewer` agent.
- **During cleanup / audit:** call `/loctree:follow scope:<dead|cycles|twins|hotspots>`.

## When NOT to use

- Single-line typo fixes in non-source files (READMEs, comments, docs)
- Operator says "skip orientation" or "I know this code"
- Inside an active debugging loop where you've got recent perception in context

## Authority discipline

Every fact derived from loctree carries an authority label — surface it inline:

- `repo_verified` (top trust, AST + git state)
- `loctree_derived` (analyzer inference)
- `aicx_operator` (sticky operator intent)
- `aicx_agent` (prior agent outcome — verify before propagating)
- `aicx_failure` (anti-recommendation — don't repeat)
- `semantic_guess` (heuristic, low trust — verify)
- `stale_or_unknown` (re-check)

The user's trust scales with how cleanly you separate fact from heuristic.

## Reference

- Plugin manifest: `../.claude-plugin/plugin.json`
- MCP wiring: `../.mcp.json`
- Vetcoders Charter: https://github.com/vetcoders/vibecrafted
- Loctree suite: https://github.com/Loctree/loctree-suite

## Author

Vetcoders · `agents@vetcoders.io` · 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
