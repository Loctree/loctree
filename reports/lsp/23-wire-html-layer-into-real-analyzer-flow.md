---
plan: 23-wire-html-layer-into-real-analyzer-flow
status: done
date: 2026-05-15
agent: claude
project: Loctree/loctree-suite
branch: fix/the-truth-of-findings
commit_base: f4d339ac
---

# Plan 23 — Completion report

## Summary

`loct report` is now wired end-to-end against the real analyzer flow:

- The subcommand always emits an HTML artifact, defaulting to the canonical
  artifacts directory when `--output` is not supplied.
- The HTML renderer is fed by the same `ReportSection` payload used by the
  JSON/MCP pipeline; no `reports/examples/*` or demo data participates in
  the production path.
- Provenance (loctree binary version, git branch, git commit, generated_at,
  schema, project root) is rendered as chip badges in the report header.
- JS assets required by interactive graph views are emitted alongside the
  artifact so the report works when opened from disk.
- Round-trip e2e coverage asserts that fixture-derived facts land in the
  rendered HTML.

## Code evidence

- `loctree-rs/src/cli/dispatch/mod.rs` — `Command::Report` now defaults
  `parsed.report_path` to `Snapshot::artifacts_dir(<root>)/report.html`
  when `--output` is not provided, closing the silent-no-op gap where
  `loct report` previously refreshed cached JSON but skipped the HTML
  write.
- `loctree-rs/src/analyzer/output.rs` — `ReportSection` now carries
  `loctree_version` (`env!("CARGO_PKG_VERSION")`); `write_report` wraps
  io errors with the failing path for actionable diagnostics; parent
  directories are still created via `mkdir -p`.
- `loctree-rs/src/analyzer/report.rs` and `reports/src/types.rs` — new
  optional `loctree_version: Option<String>` field; `skip_serializing_if
  = "Option::is_none"` keeps every existing JSON/MCP consumer stable.
- `loctree-rs/src/analyzer/html.rs` — existing JSON-bridge adapter
  (`render_html_report`) updated; the literal struct sites in the
  module's tests pick up the new field. The adapter remains the
  single, explicit boundary between analyzer types and
  `report_leptos::types::ReportSection`.
- `reports/src/components/section.rs` — provenance row now exposes a
  `loctree` chip whose value cell carries the bare semver.
- `loctree-rs/src/analyzer/for_ai.rs` — test fixture updated for the
  new field.

## Test evidence

```
$ cargo fmt --all --check        # clean
$ cargo test -p report-leptos    # 19 passed, 0 failed (unit + doctests)
$ cargo test -p loctree --test e2e_cli html
running 3 tests
test management_commands::report_creates_html ... ok
test management_commands::report_without_output_writes_html_to_artifacts_dir ... ok
test management_commands::report_html_round_trip_against_fixture ... ok
$ cargo test -p loctree --test e2e_cli json_output
22 passed; 0 failed   # JSON / MCP surfaces stable
```

Two new e2e cases in `loctree-rs/tests/e2e_cli.rs`:

1. `report_html_round_trip_against_fixture` — explicit `--output` into a
   nested subdirectory, asserts `<!DOCTYPE html>`, "Loctree Report"
   title, loctree binary version chip, `generated` provenance label,
   fixture filenames (`alpha.ts`, `beta.ts`), and the
   `loctree-cytoscape.min.js` asset emitted next to the report.
2. `report_without_output_writes_html_to_artifacts_dir` — `loct report`
   with no flags must write the HTML to
   `Snapshot::artifacts_dir(root)/report.html` and the HTML must
   reference the fixture's own files.

## Smoke run against this repository

```
$ cargo run -q -p loctree --bin loct -- report --output target/loctree-smoke/report.html
[OK] Report → /home/polyversai/Library/Caches/loctree/projects/b16a071961699188/fix_the-truth-of-findings@f4d339ac/report.html
[OK] Report → target/loctree-smoke/report.html
[loctree] Summary: files 356, missing handlers 0, unused handlers 0, languages [ts,rs,make,shell,css,js], elapsed 19.81s
```

Generated artifact: `target/loctree-smoke/report.html` (1,245,680 bytes).

Asserted strings present in the artifact:

| Needle                              | Source                                   |
|-------------------------------------|------------------------------------------|
| `<!DOCTYPE html>`                   | Renderer prologue                        |
| `Loctree Report`                    | Vista document title                     |
| `loctree-suite`                     | Project root display (this repo)         |
| `fix/the-truth-of-findings`         | Git branch (this checkout)               |
| `f4d339ac`                          | Git commit (this checkout)               |
| `0.10.2`                            | `loctree_version` provenance (new)       |
| `reports/src/lib.rs`                | Fixture-derived fact (real source file)  |
| `loctree-rs/src/types.rs`           | Top hub from this repo (analyzer truth)  |
| `loctree-cytoscape.min.js`          | Cytoscape asset reference, file on disk  |

Text excerpt from the provenance row in the rendered DOM:

```
<span class="report-identity-badge" aria-label="Generated Loctree report">
  Generated Loctree Report
</span>
<p class="report-eyebrow">Project</p>
<h1 class="report-section-title">…loctree-suite</h1>
<div class="report-meta-row">
  <span class="report-meta" title="git branch @ commit">
    <span class="report-meta-label">git</span>
    <span class="report-meta-value">fix/the-truth-of-findings@f4d339ac</span>
  </span>
  <span class="report-meta" title="report generated at">
    <span class="report-meta-label">generated</span>
    <span class="report-meta-value">2026-05-15T03:19:47.280579Z</span>
  </span>
  …
  <span class="report-meta" title="loctree binary version">
    <span class="report-meta-label">loctree</span>
    <span class="report-meta-value">0.10.2</span>
  </span>
</div>
```

Graph assets when opened from disk: six JS files (`loctree-cytoscape.min.js`,
`loctree-dagre.min.js`, `loctree-cytoscape-dagre.js`, `loctree-layout-base.js`,
`loctree-cose-base.js`, `loctree-cytoscape-cose-bilkent.js`) are emitted next
to `report.html`. No broken local-asset links in the default path.

## Boundary notes

- No SaaS, billing, Polar, or upload workflow touched. `loctree-com` is
  unaffected.
- JSON, JSONL, SARIF, MCP, and AI-oriented outputs are unchanged in
  observable shape (`loctree_version` is `Option<String>` with
  `skip_serializing_if = "Option::is_none"`).
- No `reports/examples/*` data participates in the production CLI path.
- No re-implementation of analyzer logic inside `reports/`; the existing
  JSON-bridge adapter in `loctree-rs/src/analyzer/html.rs` remains the
  single boundary.

## Handoff to plan 24

The renderer surface is real, but the visual language still leans on
internal Leptos chrome. Plan 24 should:

- Align typography, spacing, palette, and chip styling with the
  `loctree-com` design system.
- Reconcile the duplicate `report-identity-badge` CSS rule and any other
  drift between `reports/src/styles.rs` and the marketing surface.
- Consider visual treatment for empty/sparse sections (currently they
  render as bare placeholders).

No visual redesign was attempted in this plan — that is plan 24's scope.

VibeCrafted with AI Agents (c)2024-2026 The LibraxisAI Team
