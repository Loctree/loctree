---
name: loctree
description: Structural and literal repository perception before edits, deletes, refactors, or unfamiliar-code work. Use context/repo-view/focus/slice/impact/find/follow to establish scope, dependencies, blast radius, exact occurrences, and analyzer coverage before acting.
argument-hint: "[task, file, directory, or symbol]"
allowed-tools:
  - Read
  - Grep
  - Glob
  - Bash
  - mcp__loctree-mcp__context
  - mcp__loctree-mcp__repo-view
  - mcp__loctree-mcp__slice
  - mcp__loctree-mcp__impact
  - mcp__loctree-mcp__find
  - mcp__loctree-mcp__focus
  - mcp__loctree-mcp__follow
  - mcp__loctree-mcp__tree
  - mcp__loctree-mcp__prism
---

# Loctree: map before the cut

Loctree is a structural map and an indexed literal truth surface. It is strongest
when it answers questions that plain text alone cannot: what a file imports, who
consumes it, where an identifier is defined or re-exported, which runtime bridge
connects two layers, and what scope the answer actually covers.

It is evidence, not an oracle. Snapshot scope, analyzer coverage, generated code,
manifests, reflection, and dynamic loading can all bound what it sees. Use direct
file reads, tests, manifests, and runtime probes as independent witnesses when a
decision is destructive or user-visible.

## Start from the question

| Need | MCP route | CLI route |
|---|---|---|
| Broad task/repo orientation | `context` | `loct context --task "..."` |
| Quick repository overview | `repo-view` | `loct repo-view` |
| Directory wiring | `focus` | `loct focus path/` |
| File dependencies and consumers | `slice` | `loct slice path/file` |
| Rename/delete blast radius | `impact` | `loct impact path/file` |
| Exact identifier occurrences | `find` with `mode: literal` | `loct find Identifier` |
| Definition/re-export sites | `find` with `mode: where-symbol` | `loct find Identifier --where-symbol` |
| Broad symbol/parameter candidates | `find` with discovery mode | `loct find --discover Terms` |
| Bounded definition source | use body surface if exposed | `loct body Symbol` |
| Dead/cycles/twins/hotspots/runtime flow | `follow` | `loct follow <scope>` |

Plain `loct find QUERY` is exact identifier-boundary literal search. `--literal`
is an explicit alias. Broad AST, parameter, regex, and fuzzy discovery is opt-in
through `--discover`; candidates from discovery are not literal proof.

For large literal result sets use `loct occurrences` with `--compact`,
`--count-only`, `--group-by-file`, or `--limit/--offset`. Read the returned scope,
counts, truncation, and freshness metadata before interpreting absence.

## Working sequence

1. Establish repo identity, branch/HEAD, dirty state, snapshot scope, and freshness.
2. Use `context` for a broad task or route directly to `focus`, `slice`, `find`, or
   `impact` when the question is already bounded.
3. Before editing a file, inspect its slice. Before delete/rename, inspect impact
   and then verify manifests, runtime entrypoints, generated wiring, and tests.
4. Before creating a symbol, run literal find plus where-symbol to detect existing
   definitions and re-exports.
5. Read the exact files and run the nearest real product gate before claiming.

Do not interpret empty runtime/structural cards as automatically healthy or as
automatically broken. Treat them as a result that needs a coverage check: confirm
the requested scope, language support, snapshot freshness, and known entrypoints.

## Proven strengths

- Identifier boundaries avoid substring noise: `LOCT_OPEN_BROWSER` stays distinct
  from `LOCT_OPEN_BROWSER_ENV`; `hotspot` stays distinct from `hotspots`.
- Indexed tracked files can remain visible even when default ignore rules hide them
  from a recursive text search; the response still states its indexed universe.
- In live AICX and Vibecrafted checks, plain `find` matched independent exact-word
  counts (38/38 and 22/22), while `where-symbol` reduced those occurrences to the
  two meaningful definition/re-export sites.
- On ScreenScribe, `slice` separated dependencies from consumers, `impact` exposed
  5 direct and 12 transitive dependents, and `follow trace` connected the
  frontend/backend handler path.

These are examples of the contract, not universal performance promises.

## Honest boundaries

- A zero-consumer or dead-code result is a candidate, never deletion permission.
- `impact` covers edges represented in the current snapshot graph.
- Literal absence applies to the stated indexed universe, not every byte on disk.
- AICX memory is historical intent, not code truth.
- Warm-cache speed is not a promise about cold scans after tree drift.
- `--discover --limit` and JSON truncation must be checked in the installed version;
  do not advertise a globally bounded discovery response unless the receipt proves it.

If Loctree is stale, wrong, noisy, unsupported, or forces a fallback, record a
reproducible note in the repository's `.loctree/loctree-feedback.md` (contributors) or
append to `~/internal-artifacts/loctree/loctree-feedback.md` (external/operator workflow).

Finish orientation with: snapshot/HEAD, scope and coverage, decisive evidence,
remaining uncertainty, and the next safe command.
