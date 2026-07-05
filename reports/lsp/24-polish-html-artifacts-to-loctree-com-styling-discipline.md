---
plan: 24-polish-html-artifacts-to-loctree-com-styling-discipline
status: done
date: 2026-05-15
agent: claude
project: Loctree/loctree-suite
---

# Plan 24 — Completion report

Polish pass aligning generated Loctree HTML artifacts to the loctree-com
`/cloud` styling discipline. The artifact still uses the existing analyzer
flow shipped in plan 23 (`loct report --output …`); this round only touches
the renderer (`reports/` crate, the Leptos SSR layer).

## Summary of changes

- **Editorial token layer** — `reports/src/styles.rs` now ships `--report-*`
  primitive + semantic tokens that mirror
  `loctree-com/styles/tokens.css` (warm `--report-ink/-bone` surfaces, the
  two-color `--report-amber` / `--report-teal` accent system, semantic
  `--report-status-{success,warning,info,danger}` tokens). The legacy
  `--theme-*` aliases are remapped onto the new palette so every existing
  component inherits the editorial dark surface without a per-component
  refactor.
- **Default theme is now warm editorial dark** — the `:root` block defaults
  to ink/bone instead of the cool blue-grey "Vista Galaxy Black Steel"
  palette. Light mode is preserved (and re-keyed to a warm off-white that
  matches loctree-com's `data-theme="light"` mode) but only kicks in when
  the user explicitly opts in via `.light`.
- **Identity hero** — every section now opens with a `report-identity-badge`
  ("GENERATED LOCTREE REPORT" with an amber dot), an eyebrow / display-serif
  title pattern (`report-eyebrow` + `report-section-title`), and a
  `report-meta-row` carrying git, generated time, schema, and loctree binary
  version. The header height is now `min-height` instead of fixed so the
  pattern fits cleanly without truncation.
- **Share/evidence footer** — new `ReportEvidenceFooter` Leptos component
  renders at the bottom of the document with renderer version (sourced from
  `CARGO_PKG_VERSION` rather than a hardcoded string), source project,
  generated-at, git ref, schema, and a verbatim reproduction command. Copy
  is editorial fineprint, not a SaaS CTA. The renderer footer also points
  reviewers to `https://loct.io/cloud` for hosted Loctree without
  pretending to be the hosted page itself.
- **Severity tokens** — every severity badge / risk indicator
  (action-plan risk, dist status, count badges, confidence chips, etc.)
  was migrated from raw hex (`#27ae60`, `#e67e22`, `#c0392b`, `#3182ce`,
  `#22c55e`, `#ef4444`, `#d64545`, `#b9770e`) to the semantic
  `--report-status-*` tokens. Severity text labels remain present, so no
  encoding is color-only.
- **Accessibility / reduced-motion** — added a global `:focus-visible`
  outline using `--report-teal`, scoped reduced-motion overrides for
  navigation/button transitions, and `aria-label` on the identity badge,
  meta-row, header stats, and evidence footer.
- **Long-path/empty/narrow viewport** — new `report-path-wrap`,
  `report-fallback-empty`, and a `@media (max-width: 900px)` block keep the
  hero and footer credible on long file paths and narrow viewports;
  the pre-existing 900px responsive collapse already moves the sidebar
  under the main content.
- **Crate version honesty** — the sidebar footer now interpolates
  `env!("CARGO_PKG_VERSION")` instead of the literal string `"loctree v0.10.2"`
  and labels the snapshot text as `Generated artifact`.

## Files touched

- `reports/src/styles.rs`
  - Replaced the `:root` block with the loctree-com aligned token layer.
  - Re-declared `.dark` and added `.light` so the JS toggle still has
    selectors to land on; both themes now derive from `--report-*`.
  - Loosened `.app-header` to `min-height` and gave it editorial padding;
    added `.header-title { flex: 1 1 auto; min-width: 0 }`.
  - Migrated 30+ raw severity hexes to `--report-status-*` tokens.
  - Appended an editorial polish layer at the bottom defining
    `.report-eyebrow`, `.report-title-display`, `.report-section-title`,
    `.report-section-header`, `.report-sticky-hero`, `.report-meta-row`,
    `.report-meta`, `.report-meta-label`, `.report-meta-value`,
    `.report-path-wrap`, `.report-status-*`, `.report-severity-badge`,
    `.report-evidence-footer`, `.report-identity-badge`,
    `.report-fallback-empty`, `:focus-visible`, reduced-motion overrides,
    and a narrow-viewport block for the new surfaces.
- `reports/src/components/document.rs`
  - Added `pub(crate) const REPORT_RENDERER_VERSION = env!(...)`.
  - Replaced the hardcoded sidebar footer string.
  - Switched the sections iterator to `iter().cloned()` so the new
    `<ReportEvidenceFooter sections=sections.clone() />` can render with the
    primary section's provenance.
  - Added the new `ReportEvidenceFooter` Leptos component and a small
    `shorten_for_repro` helper.
- `reports/src/components/section.rs`
  - Replaced the plain `<h1> + <p>` header with the editorial pattern:
    identity badge → eyebrow → display title → wrapping path → meta-row.
  - Added `aria-label` attributes on the identity badge, meta-row, and
    stats group.
- `reports/src/lib.rs`
  - Added 7 new unit tests covering: identity badge presence + absence of
    SaaS copy; eyebrow/title/sticky-hero pattern; evidence footer with
    full provenance; evidence footer graceful empty-state; presence of the
    editorial token layer in the CSS string; secrets-free sweep
    (no `polar_*`, `price_*`, `/api/checkout`, `Add Cloud Sync`); long-path
    rendering with the `report-path-wrap` class.

## Verification

```
cargo fmt --all --check                                # clean
cargo test -p report-leptos --lib                      # 26 passed (was 19)
cargo test -p report-leptos --doc                      # 14 passed
cargo test -p loctree --test e2e_cli html              # 3 passed
cargo test -p loctree --lib analyzer::html             # 8 passed
loct report --output target/loctree-smoke/polished-report.html
ls target/loctree-smoke/                               # report.html + 6 JS bundles
```

Smoke artifact: `target/loctree-smoke/polished-report.html` — 1.24 MB,
containing 37 hits across the new editorial classes / copy
(`Generated Loctree Report`, `report-evidence-footer`,
`report-identity-badge`, `report-eyebrow`, `report-section-title`,
`loct.io/cloud`, `evidence-repro`, `--report-amber`,
`--report-status-success`, …).

Baseline: `target/loctree-smoke/baseline-report.html` (1.20 MB, generated
before the polish pass; same fixtures, cool blue-grey palette, hardcoded
`loctree v0.10.2` sidebar string, no evidence footer, no identity badge).

## Round-trip evidence (excerpt)

```
$ grep -onE "Generated Loctree Report — provenance|Reproduce this artifact|loct.io/cloud" \
        target/loctree-smoke/polished-report.html
3959:Generated Loctree Report — provenance
3959:Reproduce this artifact
3959:loct.io/cloud
3959:loct.io/cloud
```

```
$ grep -nE "report-evidence-footer\b" target/loctree-smoke/polished-report.html | head -1
3686:.report-evidence-footer {
```

The provenance footer renders the renderer version
(`loctree-suite v0.10.2`), the source project root, the live git ref
(`fix/the-truth-of-findings@f4d339ac`), the timestamp, and a copy-paste
reproduction command. The cross-link to `https://loct.io/cloud` is
fineprint, not a CTA; no checkout, no purchase copy, no Polar IDs (the
`no_loctree_com_secrets_in_artifact` test guards this).

## Acceptance criteria mapping

- [x] **Coherent IA**: identity badge → executive summary (existing health
      gauge + analysis summary) → evidence sections → graphs → reproduction
      footer.
- [x] **Visual language matches loctree-com**: warm dark surface, editorial
      typography (Instrument Serif display, Inter body, JetBrains Mono
      meta), restrained accent palette, consistent radii/spacing tokens.
- [x] **Tokens, not ad-hoc CSS**: severity colors and report classes route
      through `--report-*` tokens; new shared classes
      (`report-eyebrow`, `report-section-title`, `report-evidence-footer`,
      `report-identity-badge`, `report-fallback-empty`) replace the need
      for component-specific re-implementations.
- [x] **Header says "generated Loctree report"**: literal
      `report-identity-badge` chip plus the evidence-footer eyebrow
      (`Generated Loctree Report — provenance`); no "buy now" / "checkout"
      copy in the artifact.
- [x] **Critical readability paths**: long paths use `report-path-wrap`;
      empty provenance handled by the missing-provenance test;
      narrow-viewport CSS preserves both header and footer; graph fallback
      text already exists upstream and remains routed through the
      `--theme-text-secondary` token.
- [x] **Accessibility**: semantic `<header>` / `<footer>`, `aria-label`s on
      identity badge, meta-row, header stats and evidence footer, global
      `:focus-visible` ring at `--report-teal`. Severity badges always carry
      a text label (`high`, `medium`, `low`, `fully-shaken`, etc.) so
      severity is never color-only.
- [x] **Graph affordances explained / static fallback**: `.report-fallback-empty`
      class shipped for empty graph/large-surface fallback; existing
      graph_warning copy continues to surface when the dataset is missing.
      JavaScript-disabled callers still see the full provenance footer,
      identity badge, and evidence (the footer is plain HTML).
- [x] **Provenance footer**: version, timestamp, source project, repro
      command — all present in the new `report-evidence-footer`.
- [x] **No loctree-com secrets**: enforced by
      `no_loctree_com_secrets_in_artifact` (rejects `polar_*`, `price_*`,
      `/api/checkout`, `Add Cloud Sync`).
- [x] **Visual regression evidence**: 7 new deterministic HTML assertion
      tests guarding the editorial discipline plus a real-repo smoke
      artifact (`target/loctree-smoke/polished-report.html`).

## Residual UX gaps

- **Browser/Playwright smoke harness**: not introduced in this round. The
  existing test surface is deterministic HTML assertions (cargo unit + doc
  tests + the e2e_cli round-trip tests). Adding a Playwright sweep that
  takes desktop + narrow viewport screenshots is a clean follow-up — the
  fixture and the assertion patterns are now in place.
- **Plan 23 status note**: plan 23 has no completion report file under
  `reports/lsp/`, but the analyzer→HTML wiring it specifies is already
  shipping (`loctree-rs/src/analyzer/html.rs:render_html_report` →
  `report_leptos::render_report`, exposed via `loct report --output`). The
  three `e2e_cli html` tests already cover the round-trip. If a formal
  plan 23 report is desired, that is a separate paperwork pass.
- **Light theme polish**: the new `.light` mode rebalances tokens but the
  full coverage of every existing component in light mode has not been
  re-audited beyond compile-time checks. Most existing CSS only references
  `--theme-*` tokens, so it inherits cleanly, but a focused light-mode
  visual sweep would be a worthwhile follow-up.
- **Snapshot schema warning**: `loct report` still warns "Snapshot schema
  version mismatch: found 0.10.2, expected 0.11.0". Unrelated to plan 24
  but worth flagging for the broader stabilization queue.

## Verification commands

```bash
cargo fmt --all --check
cargo test -p report-leptos
cargo test -p loctree --test e2e_cli html
cargo run -q -p loctree --bin loct -- report --output target/loctree-smoke/polished-report.html
```

VibeCrafted with AI Agents (c)2024-2026 The LibraxisAI Team
