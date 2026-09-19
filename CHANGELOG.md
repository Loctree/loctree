# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to Semantic Versioning.

## [Unreleased]

## [0.14.5-dev0] - 2026-09-19

### Fixed
- pin npm trusted-publishing toolchain (#76)
- install ripgrep for publish gate (#74)
- let exact-tag recovery publish (#73)

## [0.14.4] - 2026-08-23

### Changed

- Combined bundles now default to the corrective AICX 0.12.5 release across
  local build tooling, tag workflows, tests, and operator documentation.

### Fixed

- Windows combined bundles now build, verify, smoke all six binaries against a
  committed Git fixture, stage the editor LSP executable, and upload through
  one Git Bash-safe path contract. Checksum generation and archive creation no
  longer pass drive-qualified filenames through GNU tools.
- The 0.14.4 npm identity bootstrap has an explicit operator-owned local lane
  over checksum-verified release assets. It publishes all seven package
  identities idempotently; later releases use exact-tag trusted publishing
  without a long-lived workflow token.
- `@loctree/loct` remains a maintained wrapper alias alongside canonical
  `@loctree/loctree` and short-form `loctree`.
- `loctree-mcp` keeps shared CLI/logging dependencies cross-platform while
  retaining `libc` as Unix-only, restoring the Windows MSVC build.

## [0.14.3] - 2026-08-23

### Added
- add canonical Windows combined bundles (#68)

### Fixed
- initialize musl smoke as a git fixture (#67)

## [0.14.2] - 2026-08-13

> Supersedes the untagged `0.14.1`. That version was bumped but never tagged or
> published, so `v0.13.1` is still the newest git tag and crates.io release —
> this release carries both its content and 0.14.1's.

### Added

- `loctree-mcp` now exposes `body` and `diff`, and `find(mode="regex")` scans
  raw file text through the same coverage-accounted occurrence engine as
  `loct find --regex`.

### Changed

- npm identity: the wrapper's canonical name is now `@loctree/loctree` (scope =
  org, name = repo, mirroring `github.com/Loctree/loctree`), published together
  with the plain short form `loctree` from the same tarball. The four platform
  packages follow as `@loctree/loctree-<platform>`. The established scoped name
  `@loctree/loct` remains a maintained alias over the same platform packages.
- npm wrapper now exposes `loctree-lsp` on PATH (bin shim), alongside
  `loctree`, `loct` and `loctree-mcp` — the platform packages always carried
  the binary, but `npm install -g` never linked it, so editors could not find
  it without going through `loct watch --lsp`.
- npm wrapper `author` metadata corrected to LibraxisAI <support@loctree.com>
  (was the stale "Loctree <loct@loct.io>").
- Git hooks are now an explicit installation step. `make git-hooks` installs a
  source-commit-addressed snapshot of the lightweight `pre-commit` and `commit-msg`
  hooks outside branch checkouts, while `make install` and `make install-all`
  leave repository configuration unchanged.
- Full validation moved from the automatic `pre-push` hook to the explicit
  `make preflight` command, which includes hook and Git-environment regressions.
- Both CI lanes now run on ephemeral GitHub-hosted runners. The `ops-linux`
  security hold is resolved by retirement rather than by a host rebuild: no
  self-hosted machine is trusted with repository code, and
  `tests/preflight_contract.sh` fails if any `self-hosted` label reappears in
  `ci.yml`. Internal stacked pull requests are covered regardless of their base
  branch.
- MCP traversal controls now match their CLI counterparts: `tree` defaults to
  unlimited depth and accepts path/files/match/summary/top/hidden/ignored/artifact
  controls; `slice` accepts `depth` and `rescan`; `impact` accepts `depth`; and
  `focus` accepts `depth` plus consumer opt-out.

### Fixed

- An explicit project marker (`package.json`, `pyproject.toml`, …) again
  outranks the enclosing git root when resolving a snapshot root. A monorepo
  subpackage was being re-homed onto the whole repository, so every subpackage
  wrote its atlas into one repo-wide cache slot and they silently overwrote each
  other. Requested roots nested inside the current directory now resolve to that
  directory instead of collapsing onto the first one, which also restores
  `loct plan <a> <b>` across sibling targets.
- `loctree-mcp`'s `force_no_git` parameter reaches the scan guard instead of
  only selecting the snapshot-root strategy. An MCP client has no command line
  on which to pass `--force-non-git`, so this was the only opt-in available for
  scratch directories and it was half-wired.
- Cache cleanup now fails closed: global deletion requires `--all --force`,
  invalid or incompatible selectors are rejected, `--max-size` retains only
  the newest bucket prefix that fits the requested budget, and incomplete
  bucket inventories, recency timestamps, or size measurements cannot drive
  quota eviction. Destructive cleanup acquires the same per-bucket snapshot
  cache locks used by `Snapshot::save`/`load` (non-blocking) before measuring
  recency/size and holds them through preview or deletion, so a concurrent
  writer cannot invalidate a successful quota report. Cache inventories stream
  within their wall-clock and output limits, render filesystem and metadata
  omissions as lower bounds, doctor diagnostics name incomplete-walk causes
  accurately, and freed-space totals include only successful removals.
- `loctree-mcp` now bounds its per-session in-memory project snapshot cache to
  one most-recently-used entry by default, preventing temporary worktrees from
  accumulating snapshots for the lifetime of a client process. Operators can
  tune the limit with `--snapshot-cache-capacity`, or set it to `0` to disable
  in-memory snapshot retention.
- Temporary Git fixtures can no longer inherit repository-local `GIT_*`
  variables and mutate the caller's shared Git directory, index or worktree.
- Older linked worktrees can no longer reactivate a tracked heavy `pre-push`
  hook after the common hook snapshot is installed.
- The test suites carry the non-git scan guard's documented escape hatch, so
  fixtures living in `TMPDIR` are scannable again; git fixtures supply their own
  identity instead of depending on an ambient `~/.gitconfig`; and the one
  `isolated_cache()` holder that was not `#[serial]` no longer races another
  test for the process-global cache directory.
- Bounded body reads resolve relative source paths against the requested
  snapshot roots before the server cwd, preventing a same-named `src/lib.rs`
  in the MCP/LSP checkout from returning valid-looking source from the wrong
  repository.
- The HTML report's Atlas cards are openable in place instead of copy-only:
  card markdown is embedded at generation time and expands via `<details>`
  (Overview reading-path entries expand fully, Atlas tab cards get an "Open
  card" toggle); per-file line counts are read from the card file at render
  time instead of a manifest value that drifts once cards are regenerated;
  and the Tools tab's descriptions and section tags are curated operator copy
  instead of leaking the MCP server's tiering language ("free-tier",
  "paid-tier Wave 7+") into a report humans read.
- The file walk now stops at nested git repository boundaries: a directory
  carrying its own `.git` (checkout or worktree/submodule file form) is
  another project's checkout, not content of the scanned repo, and is pruned
  at both the plain-directory and symlink recursion sites (the scan root
  itself is never pruned; `--scan-all` still re-enables it). Closes a blowup
  where an untracked build directory holding nested dependency checkouts drove
  ~2.9 GB of per-process memory and pinned a core in single-threaded allocator
  churn; also fixes silent double-indexing of `.claude/worktrees/*` gitdir-file
  entries.
- `loct audit`'s markdown report now polarizes onto a single scorer: the
  truncated italic header closes with build version, snapshot `generated_at`,
  root and commit; health renders from the canonical `health_score.rs` instead
  of a private duplicate that disagreed with it; `NAME_COLLISION` no longer
  counts as a defect; INFO-level role-fenced roots collapse to counts plus a
  `findings.json` pointer; `[QUICK WIN]` always renders, including an explicit
  "nothing cheap to do"; WARNING dead-export entries keep their reason instead
  of a bare `:0`; crowds render only when they carry issues; and
  `loct findings` accepts `--root`/`--project`.
- **Card `00-core-map` reports the intent-layer revisions it always had.**
  The Identity section carried three hardcoded rows — `store_revision:
  unavailable — ContextPack v1 does not carry an AICX store revision` and
  two siblings — printed on every card, in every repository. The claim was
  false: `pack.memory.overlay` has carried `store_revision`,
  `overlay_revision`, and `anchor_catalog_revision` since the overlay seam
  landed, and card 03 has been rendering two of them all along. Card 00 now
  reads the same layer (pack state first, the cached producer document as
  fallback) and names the refresh command only when the layer is genuinely
  cold. The overlay source is resolved once per atlas and shared by both
  cards instead of being loaded twice.
- The env-truth catalogue's read side now includes Rust. `compute_env_truth`
  never called `source_reads`, so read evidence came only from
  `semantic_facts.env_contracts` (shell, Python, Make); a repo whose live
  contract was consumed by Rust looked like a repo with no consumers, which
  invites an agent to delete a working flag. `source_reads::collect_source_env_reads`
  is now wired in, with a three-tier evidence ladder (the env API itself, an
  `env`-named accessor, and `const`/`static` key registries) and explicit
  mutation exclusion (`set`/`remove`/`save`/`restore`/`persist`/`seed`/
  `inject`/`reset` accessors, `Command::env`). The scan surface is now stated
  in a `source_reads` field and a Read-side coverage block (schema 1.1 →
  1.2), so absence of a read says where the scan looked instead of asserting
  silence.

## [0.14.1] - 2026-07-24

> Version bumped in `6bff88ce`. **Not tagged and not published yet** — `v0.13.1`
> is still the newest git tag and the newest crates.io release, so everything
> under 0.14.0 and 0.14.1 reaches users for the first time when 0.14.1 ships.

### Added

- Multi-literal OR search: `loct find A B` matches any of several literals in a
  single pass, and the same engine now backs the MCP `find` tool and the LSP
  `loctree/find` request. This closes the last query shape that pushed agents
  back to `grep`.
- Every `find`/query result declares its **indexed universe** (scanned vs.
  indexed vs. ignored vs. generated vs. fixture counts), so an empty result is
  trustworthy evidence of absence rather than silence.
- Query receipts carry a fail-closed identity: a receipt that cannot prove which
  snapshot and bundle produced it is rejected instead of being served.
- Bundle identity surface across CLI and MCP, plus checkout-identity provenance
  stamped into builds.
- Immutable scoped atlases, and context Runtime cards derived from executable
  ownership.
- Project-local cache and `doctor` output, with RSS ceilings and an opt-in
  `cache list` parsing path.
- Dead-code framework reachability: `@main` entrypoint credit, AppKit/UIKit
  delegate-protocol credit, and folded multi-line Swift inheritance clauses.
- LSP mega-root guard with a bounded residency LRU, so opening a very large
  workspace no longer grows unbounded.
- JetBrains tool-window dashboard (editorial layout, project-scoped LSP results,
  health ring) replacing raw JSON tree dumps.
- `loct anchors`-backed AI hook installer that wires Loctree-first and AICX
  continuity hooks, plus a `commit-msg` gate that compares the message against
  the actual diff.
- `loctree-plugin` is now the canonical agent-plugin surface.

### Changed

- Literal truth is the **default** search mode.
- `--limit` is applied globally or via a continuation token, instead of being
  silently per-category.
- High-confidence dead findings now require framework reachability to have been
  computed; without it the finding is downgraded rather than asserted.
- The pre-push Semgrep scan was advisory by default; the automatic pre-push
  validation path is superseded by explicit `make semgrep` / `make preflight`
  gates in the unreleased line above.
- The in-suite `./plugin` directory is deprecated in favour of `loctree-plugin`.
- Context atlas cards render English-only.

### Fixed

- `impact` no longer prints "Safe to remove" — the claim is withheld until graph
  coverage is proven, and MCP removal assessment is fail-closed.
- `context` ignores a legacy-only nested `.loctree` directory when picking the
  project root.
- Context markdown emits Hotspots / High Fan-In exactly once.
- Atlas recognizes Swift application runtimes and diagnoses empty overlays
  instead of rendering a blank card.
- LSP releases scan allocator arenas after a scan completes.
- `loct watch` reaps supervised MCP companion processes.
- MCP version override hardened against mismatched clients.
- Test suites no longer read or write the operator's real cache.

### Security

- VS Code extension lockfile: `fast-uri` 3.1.4, `linkify-it` 5.0.2.

## [0.14.0] - 2026-07-15

> Version bumped in `b3a4fc36`; never tagged or published on its own.

### Added

- I1-01 cross-repo integration: bare `loct context` now consumes the C0-01
  `aicx overlay` emission (`loctree.overlay.intent.v1`) instead of the
  budgeted `aicx intents` shell-out (which timed out on 100% of bare
  invocations against a live store). The render is served from a validated
  cache at `.loctree/aicx-overlay.v1.json` keyed by the FULL revision tuple
  `(repo_id, store_revision, overlay_revision, snapshot_commit,
  anchor_catalog_revision, schema_version)`; typed theses appear in the
  mandatory read in the spec grammar (`✓[U] 2026-07-13 · <thesis> → <target>
  (<ref>)`). Degradation is explicit — `intent layer stale (<reason>) —
  refresh: <command>` — and a DETACHED background refresh converges the
  cache between invocations (plan B: the render never waits for the
  producer). Schema-mismatched emissions fail with an explicit version
  error, never a silent drop. New module: `loctree::aicx::overlay`.
- Core `loctree::anchors` module: the deterministic `loctree.anchors.v1`
  catalog builder extracted from the `loct anchors` CLI handler so library
  consumers (ContextPack composer, LSP) can compute the current
  `anchor_catalog_revision` without importing CLI handler modules.
- `loct anchors` command emitting the deterministic `loctree.anchors.v1`
  catalog.
- Frozen `loctree.overlay.intent.v1` contract: three-level identity with
  evidence-backed relations, tracked as a checked-in contract JSON.
- Typed atlas `CardBody`: canonical payload plus a coverage receipt, with
  `.full.json` always written; dense markdown synthesis that is
  machine-provably decision-complete (FactSet parser + mutation test); and the
  `03-intent-map` card.
- Atlas data domains have a single owner, declared via `domain_owners` in the
  manifest.
- `--include-ignored` override for files excluded by `.loctignore`.

### Changed

- Engine distribution mirror ships the full bundle (engine + MCP + LSP).

### Fixed

- Re-landed the 27065f7e TTY regression contract on this branch: a bare
  `loct context` on a real TTY no longer renders the full HTML report
  synchronously (tens of seconds on large repos); the artifact stays behind
  the explicit `--full` request.
- `loct find --mode <x>` is accepted as an alias, and unknown options produce a
  sharper error.
- macOS installs ad-hoc re-sign the installed binaries.
- VS Code Context webview no longer breaks on a bad source/origin guard; host
  messages for the context pill are validated.
- Staged distribution workspace follows the suite MSRV and ships the `aicx`
  dependency; `component-sync` extras are tracked in git.
- LSP editor-parity guard is skipped outside the monorepo shape.

### Security

- Refreshed the audited Rust lockfile; VS Code dev-dependency bumps
  (`undici` 7.28.0, `form-data` 4.0.6, `js-yaml` 4.3.0, `esbuild` 0.28.1).

## [0.13.1] - 2026-07-12

### Added

- `glama.json` maintainer manifest at repo root (mirrored to the public repos)
  so the auto-indexed Glama.ai MCP server page can be claimed by the maintainer
  GitHub account.

- Literal layer (W1) output quality: every literal occurrence now carries a
  **language-aware `occurrence_kind`** instead of a blanket `unknown`. New token-type
  classes — `css_property`, `class_token` (CSS selectors + Tailwind/utility classes
  in `class=`/`className=`), `custom_property` (`--var`), `comment`, `string_literal`,
  `data_attribute`, and `identifier` — sit alongside the existing Rust role shapes
  (`definition_like`/`mutation_like`/`field_emit_like`). `unknown` is now an honest
  fallback only. Surfaced identically across `loct occurrences`, `loct find --literal`,
  and MCP `find(mode="literal")`.
- Literal layer boundary control: `--whole-token` (CLI `occurrences` + `find --literal`)
  and `whole_token` (MCP `find`) treat `-` as token-internal, so `backdrop` no longer
  matches inside `overlay-backdrop` / `--vista-z-overlay-backdrop`. Opt-in; the default
  boundary is unchanged.
- Literal layer aggregation + token economy: `--group-by-file` / `group_by_file` adds a
  per-file `by_file` count rollup, and `--count-only` (alias `--slim`) / `count_only`
  (alias `slim`) suppresses the full occurrence list while preserving `total`,
  `files_matched`, and `by_file`. Additive JSON fields (`total`, `by_file`, `slim`) keep
  existing `loctree.*` consumers unaffected; MCP↔CLI parity preserved.
- Literal layer pagination: `--limit <N>` / `--offset <N>` on `loct occurrences` and
  `loct find --literal`, plus `limit` / `offset` on MCP `find(mode="literal")`, return a
  deterministic occurrence page with additive `page` metadata (`returned`, `has_more`,
  `next_offset`). Large repos can now stream through all literal hits without relying on
  transport truncation, while `total` still reflects the full result set.
- JetBrains plugin now exposes the literal truth layer through a user-facing
  `Find Literal Occurrences` action. It sends `mode=literal` with `whole_token`,
  `group_by_file`, `limit`, and `offset` over `loctree/find`, matching the MCP/CLI
  literal capability surface from inside the IDE.
- `loctree-lsp` now exposes `loctree/contextPack`, a cursor-paginated LSP
  request that materializes Context Atlas cards from the routed workspace
  snapshot and streams one card per response. The capability is advertised with
  a request schema and mirrors MCP `/context_pack` card filtering and cursor
  invalidation semantics.
- JetBrains plugin LSP parity expanded beyond the original narrow custom
  surface. `LoctreeLspGateway` can now call `workspaces`, `diff`, `semantic`,
  `aicx`, `astQuery`, and `contextPack`, while keeping raw JSON forwarding for
  less-common tool-window and agent-facing flows.
- JetBrains Loctree tool window now includes an inline LSP query row with modes
  for Health, Find, Literal, Follow, Slice, Impact, Context Atlas, Context Pack,
  Workspaces, Diff, AICX, Semantic, and AST Query. Results render inside the
  tool window with generic file/line navigation where payloads provide locations
  and a Load More continuation for cursored or offset-paged responses. The
  literal-search action now opens the same tool-window result pipeline instead
  of hiding rich output behind a notification summary.
- `loct context --scope <SELECTOR>` for deterministic structural narrowing.
  Selectors: `path:`, `tag:`, `import:`, `reach:`, plus named scopes from
  `.loctree/scopes.toml`. Repeatable scopes are AND-combined and can be paired
  with `--task`.
- Expose `prism` as an MCP tool in `loctree-mcp` (parameters: `task[]`, `project`, `with_aicx`, `no_aicx`,
  `aicx_project`, `limit`). Returns the same `loctree.prism.v1` JSON contract emitted by `loct prism --json`, so
  vc-polarize gating no longer requires shelling out to the CLI from agents.
- Pin the `loctree.prism.v1` wire shape with a golden JSON regression test (`loctree-rs/tests/prism_schema_golden.rs` +
  `tests/fixtures/prism_v1_minimal.json`). Any future schema drift now requires an explicit fixture refresh and a
  schema-version bump.
- Publish the `loctree.prism.v1` JSON Schema (draft-07) as a standalone artifact at
  `schemas/loctree.prism.v1.schema.json`. External integrators (vc-polarize runner, vc-operator, third-party gating
  tools) can now validate the wire shape without reading source code or test fixtures.
- Document `prism` in the `loctree` Claude Code plugin: new `loctree-plugin/skills/loctree-prism/SKILL.md` mirrors the
  sibling shape (loctree-impact / loctree-find), the orchestrator skill adds `mcp__loctree-mcp__prism` to
  `allowed-tools` and an intent-table row for "should I run vc-polarize?". Plugin manifest bumped to 0.2.0.
- Literal layer scope classification is now **multi-language**: `scope_classification` returns an honest
  `production` bucket for Python, Go, Ruby, JVM, native, shell, and other source kinds instead of collapsing
  every non-Rust/TS/CSS source into `unknown`. Path heuristics gained Python/Go/Ruby conventions
  (`test_*.py`/`*_test.py`/`conftest.py`/`*_test.go`/`*_spec.rb` → test; `setup.py`/`setup.cfg`/`tox.ini`/
  `requirements*.txt`/`Pipfile` → config; `__pycache__`/`*.pyc`/`*_pb2.py`/`.egg-info` → generated), and
  repo-root segments (`tests/`, `build/`, `dist/`, `docs/`, `.github/`) are now matched identically to nested
  ones. `unknown` stays strictly for genuinely unrecognized file kinds. Surfaced across `loct occurrences`,
  `loct find --literal`, and MCP `find(mode="literal")`.
- MCP `context(format="markdown")` no longer chokes large-repo clients on the response cap. When the rendered
  pack exceeds `LOCT_MCP_CONTEXT_MARKDOWN_BUDGET_BYTES` (default 48000) **and** the Context Atlas cards are
  materialized on disk, the tool returns a budget-bound TL;DR head plus the atlas pointer (`atlas.atlas_dir` +
  `cards[].path`) and a `read_cards_hint`, alongside additive `truncated` / `markdown_bytes` /
  `markdown_budget_bytes` fields — instead of a 60–70KB inline blob. Small repos keep the one-shot inline pack;
  if atlas materialization fails the full inline pack still rides through so no data is lost.

### Changed

- `loct context --task` is ranker-only when `--scope` is present. Standalone
  `--task` keeps the existing set-cutting behavior for backward compatibility.
- Editor Make targets now split daily development from release packaging:
  `editors-vscode` installs/updates deps and compiles, `editors-jetbrains`
  runs tests and builds the plugin ZIP, while `editors-vscode-package`,
  `editors-jetbrains-full`, and `editors-full` keep VSIX packaging and Plugin
  Verifier on explicit release lanes. `editors-jetbrains-install` reinstalls
  the built plugin into the selected IntelliJ config.
- Extract `loctree::run_prism(&PrismOptions) -> Result<PrismReport, String>` and re-export `PrismReport`,
  `PrismAxisScore`, `PrismOverlap`, `PrismTaskSummary` from the `loctree` library crate. The `loct prism` CLI handler is
  now a thin wrapper that delegates report assembly to the shared helper consumed by the MCP `prism` tool.
- JetBrains status bar no longer paints the marketing SVG in the status slot;
  it uses a fixed-size 16x16 icon so SVG viewport metadata cannot widen or crop
  the IntelliJ status bar widget.

### Fixed

- JetBrains plugin: `loctree/body` is now wired through the Kotlin client
  (protocol `BodyParams`, lsp4j interface method, gateway wrapper + router
  branch). The tool-window Body mode previously fell into the gateway's
  unsupported-request arm and surfaced as a generic "Loctree query failed"
  although the Rust LSP fully implements the method.
- Shipped surfaces no longer reference the private `loctree-suite` repo:
  JetBrains `DEFAULT_REPO_URL` (auto-download 404'd for every external user)
  and settings example now point at `Loctree/loctree-release` like the VS Code
  client; npm `@loctree/loct` package metadata, shipped README links, and the
  LICENSE Licensed-Work wording now name the public repos.
- npm `@loctree/loct`: dropped `postinstall`/`install.js` (npm 11+
  allowScripts blocks unapproved install scripts); delivery is pure
  `optionalDependencies`. Legacy never-published Gen2 wrappers
  (`loctree-mcp/`, `loctree-lsp/` staging dirs) removed with their
  postinstall scripts; PUBLISHING.md documents the no-lifecycle-scripts
  policy.
- Report sidebar renders its version from `CARGO_PKG_VERSION` instead of a
  hardcoded literal that desynced on every bump.
- Severe performance regression fixed: Pensieve scan time reduced from 1495s to 4.05s.
- JetBrains tool-window query rendering no longer silently truncates raw JSON
  responses, arrays, object fields, or `contextPack` card content; multiline
  card content is split into readable tree rows while preserving the full text.

## [0.13.0] - 2026-06-19

### Fixed
- update platformVersion to 2026.1.3 to fix verifyPlugin CI failure

## [0.13.0-dev] - 2026-06-10

### Fixed
- update platformVersion to 2026.1.3 to fix verifyPlugin CI failure

## [0.13.0-dev] - 2026-06-10

### Fixed
- update platformVersion to 2026.1.3 to fix verifyPlugin CI failure

## [0.13.0-dev] - 2026-06-10

### Fixed
- update platformVersion to 2026.1.3 to fix verifyPlugin CI failure

## [0.12.3] - 2026-06-08

### Added
- summarize changed files for diff refs
- support explicit bounded non-git context
- Context panel pagination polish
- precise import-graph cross-file resolution
- Context-King UI — Hover B + Loctree Context panel
- full literal contract, paginated occurrences, body disambiguation
- add loctree/symbolContext request (Context-King keystone)

### Fixed
- close ItemPayload when branch in LoctreeCellRenderer
- close PR32 pre-release audit gaps
- harden loctree cache and lsp healing paths
- classify inspectable resource files
- tighten literal occurrence truth
- label fixture cycles with provenance (LCT-004)
- link Swift implicit symbol consumers
- add lightweight Swift file analysis for LCT-003
- index extensionless shebang source files
- expose loctree snapshot identity
- resolve snapshot-relative paths against root, not cwd
- Workflow 2 UI corrections (operator review)
- symbolContext body never lies cross-file + real stdio smoke
- release hardening (package fail, cache rehash, startup scan)

## [0.12.2] - 2026-06-06

### Added
- summarize changed files for diff refs
- support explicit bounded non-git context
- Context panel pagination polish
- precise import-graph cross-file resolution
- Context-King UI — Hover B + Loctree Context panel
- full literal contract, paginated occurrences, body disambiguation
- add loctree/symbolContext request (Context-King keystone)

### Fixed
- close ItemPayload when branch in LoctreeCellRenderer
- close PR32 pre-release audit gaps
- harden loctree cache and lsp healing paths
- classify inspectable resource files
- tighten literal occurrence truth
- label fixture cycles with provenance (LCT-004)
- link Swift implicit symbol consumers
- add lightweight Swift file analysis for LCT-003
- index extensionless shebang source files
- expose loctree snapshot identity
- resolve snapshot-relative paths against root, not cwd
- Workflow 2 UI corrections (operator review)
- symbolContext body never lies cross-file + real stdio smoke
- release hardening (package fail, cache rehash, startup scan)

## [0.12.1] - 2026-06-05

### Added
- route Loctree queries through tool window
- add context pack IDE query surface
- rich health status bar (score + status color + actions)
- source views from LSP, add occurrences/body, fix placebo commands
- add loctree/body custom request
- expose literal whole_token/group_by_file/count_only on find tool
- wire whole_token/group_by_file/count_only into occurrences & find --literal
- language-aware occurrence_kind + whole_token + aggregation
- introduce icon theming, checksum verification, and enhanced diagnostic visuals
- upgrade `rmcp` to 1.7.0, introduce new design/UX assets, and enhance Loctree plugin functionality
- implement Loctree IDE plugin and CI workflow

### Changed
- remove dead pre-pivot status bar count path
- remove dead pull-diagnostic handler

### Fixed
- preserve full query result payloads
- point public listing links to public repo (was private loctree-suite)
- stop advertising pull diagnostics (tower-lsp doesn't route them)
- don't advertise openAtlasCard executeCommand (client registration collision)
- accept --stdio flag for LSP client compatibility
- ship runtime deps in VSIX (was: Cannot find module vscode-languageclient)
- remove invalid contributes.icons, use built-in codicon
- address adversarial review — refresh lifecycle + hardening
- restrict verifyPlugin to IntelliJ IDEA Ultimate IDEs

## [0.12.0] - 2026-06-03

### Added

- expose literal whole_token/group_by_file/count_only on find tool
- wire whole_token/group_by_file/count_only into occurrences & find --literal
- language-aware occurrence_kind + whole_token + aggregation
- introduce icon theming, checksum verification, and enhanced diagnostic visuals
- upgrade `rmcp` to 1.7.0, introduce new design/UX assets, and enhance Loctree plugin functionality
- implement Loctree IDE plugin and CI workflow

### Fixed

- restrict verifyPlugin to IntelliJ IDEA Ultimate IDEs

## [0.11.3] - 2026-06-01

### Added

- spike Gen3 single @loctree/loct suite wrapper (4 binaries)

### Changed

- single Loctree runtime — loctree+loct, mcp/lsp as co-processes

### Fixed

- close recent loctree fail hooks
- make --http/--lsp companions project-agnostic (drop launch-time root)
- converge repo on single @loctree/loct suite wrapper (Phase B1 draft)
- keep Makefile vars out of env reads

## [0.11.2] - 2026-05-31

### Added

- spike Gen3 single @loctree/loct suite wrapper (4 binaries)

### Changed

- single Loctree runtime — loctree+loct, mcp/lsp as co-processes

### Fixed

- make --http/--lsp companions project-agnostic (drop launch-time root)
- converge repo on single @loctree/loct suite wrapper (Phase B1 draft)
- keep Makefile vars out of env reads

## [0.11.1] - 2026-05-30

### Fixed

- keep Makefile vars out of env reads

## [0.11.0] - 2026-05-27

### Release Notes

- Supersedes `0.10.5`, which is yanked because its release proof did not make
  context-scope freshness and MCP reinstall state explicit enough for agent use.

## [0.10.5] - 2026-05-21 [YANKED]

### Yanked

- Yanked on 2026-05-27 in favor of `0.11.0`.
- Reason: release validation could allow agents to trust a stale or
  falsely-narrow context view after MCP reinstall/restart churn. Fresh
  `0.11.0` evidence restores the expected suite-wide snapshot and MCP command
  deck contract.
- Operators should use `0.11.0` or newer.

## [0.10.4] - 2026-05-19

## [0.10.3] - 2026-05-16

## [0.10.2] - 2026-05-13

### Added

- add deterministic scope selectors
- bump schema to v1.1 add canonical band_action field + update golden fixture
- add loctree prism skill
- expose CLI help and capabilities
- add context markdown format
- add tree-sitter substrate and astQuery MVP

## [0.10.1] - 2026-05-13

### Added

- add deterministic scope selectors
- bump schema to v1.1 add canonical band_action field + update golden fixture
- add loctree prism skill
- expose CLI help and capabilities
- add context markdown format
- add tree-sitter substrate and astQuery MVP

## [0.10.0] - 2026-05-10

### Added

- add deterministic scope selectors
- bump schema to v1.1 add canonical band_action field + update golden fixture
- add loctree prism skill
- expose CLI help and capabilities
- add context markdown format
- add tree-sitter substrate and astQuery MVP

## [0.9.7] - 2026-05-10

### Added

- add deterministic scope selectors
- bump schema to v1.1 add canonical band_action field + update golden fixture
- add loctree prism skill
- expose CLI help and capabilities
- add context markdown format
- add tree-sitter substrate and astQuery MVP

## [0.9.6] - 2026-05-09

### Added

- expose CLI help and capabilities
- add context markdown format
- add tree-sitter substrate and astQuery MVP

## [0.9.5] - 2026-05-07

### Added

- materialize context atlas
- implement Codex Manifest Protocol (manifest/cursor/receipt) for context delivery
- slim default + paginate + resource pattern (token-budget fix)
- cut4-T4 changed + task modes and markdown formatter

### Changed

- decouple loctree-mcp from cli::* namespace (extract pack module)

### Fixed

- is_git_dirty returns None for non-git dirs
- honor timeout for mcp calls
- clarify stale_threshold_days doc in EnvTruthConfig
- address PR review feedback (6 issues)
- tighten authority heuristics
- resolve test suite compilation errors after ContextPack updates

## [0.9.4] - 2026-05-05

### Added

- materialize context atlas
- add full context pack panel
- implement Codex Manifest Protocol (manifest/cursor/receipt) for context delivery
- slim default + paginate + resource pattern (token-budget fix)
- cut4-T4 changed + task modes and markdown formatter
- add response cache, repo-root detection, optional logging
- Add MCP-direct AICX integration with CLI and auto-fallback paths for agent context
  retrieval ([41eadbfc](https://github.com/Loctree/loctree-suite/commit/41eadbfc)).

### Changed

- Refresh security hygiene by clearing Dependabot alerts through dependency lockfile updates and removal of orphaned
  lockfiles ([2d7e2953](https://github.com/Loctree/loctree-suite/commit/2d7e2953)).

### Fixed

- tighten authority heuristics
- resolve test suite compilation errors after ContextPack updates
- add jq dependency check and payload size limit
- handle comments between #[tauri::command] and fn
- use libgit2 for upward git root discovery
- Address PR #14 review feedback across the semantic spine and Cut 5
  surfaces ([1eef10b6](https://github.com/Loctree/loctree-suite/commit/1eef10b6)).
- Incorporate the Cut 9 security-hygiene merge from the default
  branch ([213e9f02](https://github.com/Loctree/loctree-suite/commit/213e9f02)).

## [0.9.3] - 2026-04-28

### Added

- cut4-T4 changed + task modes and markdown formatter
- add response cache, repo-root detection, optional logging
- add sys.modules monkey-patch detection

### Fixed

- add jq dependency check and payload size limit
- handle comments between #[tauri::command] and fn
- use libgit2 for upward git root discovery
- clippy warnings in query.rs

## [0.9.2] - 2026-04-25

### Added

- `loct doctor --cache --scope` operator-facing trust diagnostics. Cache
  identity inspection (Cut 2 T0), snapshot scope validation including the
  scope-mismatch class caught by the MCP guard (Cut 2 T1), and an opt-in
  `--fix` mode that purges only scope-mismatched flat fallbacks while
  preserving per-scan history (Cut 2 T2). JSON output with stable
  `schema_version: "1.0"` for agent consumption; non-interactive `--fix`
  without `--yes` exits with code 2 instead of silently mutating cache.
- add response cache, repo-root detection, optional logging
- add sys.modules monkey-patch detection
- Add loctree-lsp crate and VSCode extension scaffold
- Add health score gauge, audit tab & style unification
- add clickable file links to Pipelines tab
- v0.8.0 - auto-snapshot + dual license MIT/Apache-2.0

### Fixed

- add jq dependency check and payload size limit
- handle comments between #[tauri::command] and fn
- use libgit2 for upward git root discovery
- clippy warnings in query.rs
- respect suppressions in loct twins command
- handle async def for route detection
- multiline imports and __all__ duplicate detection
- refactor Pipelines to vanilla JS for expand/filter/search
- critical path rebasing + file existence validation
- add Pipelines nav tab + remove memex from scripts and hooks
- Add SLED_PATH env for multi-instance support

## [0.9.1] - 2026-04-25

### Added

- add response cache, repo-root detection, optional logging
- add sys.modules monkey-patch detection
- Add loctree-lsp crate and VSCode extension scaffold
- Add health score gauge, audit tab & style unification
- add clickable file links to Pipelines tab
- v0.8.0 - auto-snapshot + dual license MIT/Apache-2.0
- Add rmcp-mux launcher, configs, and documentation

### Fixed

- add jq dependency check and payload size limit
- handle comments between #[tauri::command] and fn
- use libgit2 for upward git root discovery
- clippy warnings in query.rs
- respect suppressions in loct twins command
- handle async def for route detection
- multiline imports and __all__ duplicate detection
- refactor Pipelines to vanilla JS for expand/filter/search
- critical path rebasing + file existence validation
- add Pipelines nav tab + remove memex from scripts and hooks
- Add SLED_PATH env for multi-instance support

## [0.9.0] - 2026-04-13

### Added

- new analyzers: shell (bash/sh/zsh/fish), Makefile (GNU make family), Zig
- extensionless shell detection via shebang fallback (`#!/.../{sh,bash,zsh,fish}`)
- extensionless Makefile detection (`Makefile`, `GNUmakefile`, `BSDmakefile`)
- response cache, repo-root detection, optional logging in pre-commit hooks
- sys.modules monkey-patch detection (Python analyzer)
- loctree-lsp crate and VSCode extension scaffold
- health score gauge, audit tab and style unification in reports
- clickable file links in Pipelines tab
- pipelines, insights, and manifests CLI commands
- SymbolMatchKind in symbol search output
- lint suppression and cross-match support in `loct find`
- NPM distribution split into 3 scoped packages: `@loctree/loct`, `@loctree/loctree-mcp`, `@loctree/loctree-lsp`, each
  with 4 platform packages (darwin-arm64, darwin-x64, linux-x64-gnu, win32-x64-msvc)

### Changed

- **License: workspace flipped from `MIT OR Apache-2.0` to `BUSL-1.1`** (Business Source License 1.1). Change Date
  `2030-04-13`, Change License `Apache-2.0`. Additional Use Grant restricts hosted-service redistribution that competes
  with Loctree-provided services. Full parameters in `LICENSE`. All per-crate manifests inherit via
  `license.workspace = true`. NPM packages and platform-packages aligned. Old `LICENSE-MIT` and `LICENSE-APACHE` files
  removed.
- **Publish lockdown:** every workspace crate has `publish = false`. Crates.io publish is gated; thin releases, npm, and
  Homebrew are the active distribution paths.

### Removed

- memex feature: removed entirely. Never built in any shipped distribution. Use standalone rmcp-memex CLI + MCP server
  if AI memory indexing is needed.
- legacy `LICENSE-MIT` and `LICENSE-APACHE` files (replaced by single `LICENSE` carrying BUSL-1.1).
- dead `documentation = "https://docs.rs/..."` entries from per-crate manifests (docs.rs does not build crates with
  `publish = false`).

### Fixed

- Makefile `landing` / `landing-dev` targets pin `TRUNK_TOOLS_WASM_BINDGEN=0.2.118` to match `wasm-bindgen` in
  `Cargo.lock` (was stuck on 0.2.114, broke `make landing`).
- OG social card, homebrew-core formula, test sync
- fall back to available python for landing
- restore deploy and version contract
- restore landing mobile nav and build parity
- jq dependency check and payload size limit in hooks
- handle comments between `#[tauri::command]` and `fn`
- libgit2-based upward git root discovery
- clippy warnings in `query.rs`
- respect suppressions in `loct twins`
- handle `async def` for route detection
- multiline imports and `__all__` duplicate detection
- refactor Pipelines tab to vanilla JS for expand/filter/search
- critical path rebasing and file existence validation
- add Pipelines nav tab; remove memex from scripts and hooks

## [0.8.16] - 2026-03-07

### Added

- add response cache, repo-root detection, optional logging
- add sys.modules monkey-patch detection
- Add loctree-lsp crate and VSCode extension scaffold
- Add health score gauge, audit tab & style unification
- add clickable file links to Pipelines tab
- v0.8.0 - auto-snapshot + dual license MIT/Apache-2.0
- Add rmcp-mux launcher, configs, and documentation
- Embedded blog system with markdown rendering
- Add micro-animation to loct startup + rebrand to loct
- add batch index CLI command

### Fixed

- add jq dependency check and payload size limit
- handle comments between #[tauri::command] and fn
- use libgit2 for upward git root discovery
- clippy warnings in query.rs
- respect suppressions in loct twins command
- handle async def for route detection
- multiline imports and __all__ duplicate detection
- refactor Pipelines to vanilla JS for expand/filter/search
- critical path rebasing + file existence validation
- add Pipelines nav tab + remove memex from scripts and hooks
- Add SLED_PATH env for multi-instance support
- Use ASCII characters in deprecation warning

## [0.8.11] - 2026-02-07

### Added

- **Cross-match search** (`loct find`): Multi-term queries (`loct find Snapshot FileAnalysis`) now show WHERE terms
  MEET — files and functions where 2+ different terms co-occur. Replaces flat OR bloat with semantic cross-matching.
  Single-term queries unchanged.
- **`make publish` target**: Cascading publish to crates.io in correct dependency order (report-leptos -> loctree ->
  loctree-mcp) with pre-publish validation. Supports `BUMP=true` for version bump + publish in one step.

### Fixed

- **Snapshot schema compatibility**: Patch bumps no longer invalidate user snapshots. Schema version comparison now uses
  `major.minor` only — a 0.8.10 snapshot works with 0.8.11 binary without warnings.
- **Version management scripts**: Fixed `version-bump.sh` and `sync-version.sh` for workspace versioning (
  `version.workspace = true`). Both scripts failed silently when crates inherit version from workspace root.
- **Publish dependency order**: Fixed cascade from wrong order (loctree -> report-leptos) to correct (report-leptos ->
  loctree -> loctree-mcp) matching actual Cargo dependency chain. Increased crates.io index wait from 10s to 15s.

### Changed

- Multi-term search splits display into compact cross-match summary + filtered symbol matches (files with cross-matches
  listed first, symbols filtered to those files only)
- `search_params()` uses function-level cross-match for multi-term: only params from functions where 2+ different terms
  match in the signature

## [0.8.9] - 2026-01-21

### Added

- add sys.modules monkey-patch detection
- Add loctree-lsp crate and VSCode extension scaffold
- Add health score gauge, audit tab & style unification
- add clickable file links to Pipelines tab
- v0.8.0 - auto-snapshot + dual license MIT/Apache-2.0
- Add rmcp-mux launcher, configs, and documentation
- Embedded blog system with markdown rendering
- Add micro-animation to loct startup + rebrand to loct
- add batch index CLI command

### Fixed

- handle comments between #[tauri::command] and fn
- use libgit2 for upward git root discovery
- clippy warnings in query.rs
- respect suppressions in loct twins command
- handle async def for route detection
- multiline imports and __all__ duplicate detection
- refactor Pipelines to vanilla JS for expand/filter/search
- critical path rebasing + file existence validation
- add Pipelines nav tab + remove memex from scripts and hooks
- Add SLED_PATH env for multi-instance support
- Use ASCII characters in deprecation warning

## [0.8.9-dev] - 2026-01-20

### Added

- add sys.modules monkey-patch detection
- Add loctree-lsp crate and VSCode extension scaffold
- Add health score gauge, audit tab & style unification
- add clickable file links to Pipelines tab
- v0.8.0 - auto-snapshot + dual license MIT/Apache-2.0
- Add rmcp-mux launcher, configs, and documentation
- Embedded blog system with markdown rendering
- Add micro-animation to loct startup + rebrand to loct
- add batch index CLI command

### Fixed

- clippy warnings in query.rs
- respect suppressions in loct twins command
- handle async def for route detection
- multiline imports and __all__ duplicate detection
- refactor Pipelines to vanilla JS for expand/filter/search
- critical path rebasing + file existence validation
- add Pipelines nav tab + remove memex from scripts and hooks
- Add SLED_PATH env for multi-instance support
- Use ASCII characters in deprecation warning

## [0.8.8] - 2026-01-18

### Added

- add sys.modules monkey-patch detection
- Add loctree-lsp crate and VSCode extension scaffold
- Add health score gauge, audit tab & style unification
- add clickable file links to Pipelines tab
- v0.8.0 - auto-snapshot + dual license MIT/Apache-2.0
- Add rmcp-mux launcher, configs, and documentation
- Embedded blog system with markdown rendering
- Add micro-animation to loct startup + rebrand to loct
- add batch index CLI command

### Fixed

- clippy warnings in query.rs
- respect suppressions in loct twins command
- handle async def for route detection
- multiline imports and __all__ duplicate detection
- refactor Pipelines to vanilla JS for expand/filter/search
- critical path rebasing + file existence validation
- add Pipelines nav tab + remove memex from scripts and hooks
- Add SLED_PATH env for multi-instance support
- Use ASCII characters in deprecation warning

## [0.8.7] - 2026-01-15

### Added

- add sys.modules monkey-patch detection
- Add loctree-lsp crate and VSCode extension scaffold
- Add health score gauge, audit tab & style unification
- add clickable file links to Pipelines tab
- v0.8.0 - auto-snapshot + dual license MIT/Apache-2.0
- Add rmcp-mux launcher, configs, and documentation
- Embedded blog system with markdown rendering
- Add micro-animation to loct startup + rebrand to loct
- add batch index CLI command

### Fixed

- clippy warnings in query.rs
- respect suppressions in loct twins command
- handle async def for route detection
- multiline imports and __all__ duplicate detection
- refactor Pipelines to vanilla JS for expand/filter/search
- critical path rebasing + file existence validation
- add Pipelines nav tab + remove memex from scripts and hooks
- Add SLED_PATH env for multi-instance support
- Use ASCII characters in deprecation warning

## [0.8.6] - 2026-01-15

### Added

- add sys.modules monkey-patch detection
- Add loctree-lsp crate and VSCode extension scaffold
- Add health score gauge, audit tab & style unification
- add clickable file links to Pipelines tab
- v0.8.0 - auto-snapshot + dual license MIT/Apache-2.0
- Add rmcp-mux launcher, configs, and documentation
- Embedded blog system with markdown rendering
- Add micro-animation to loct startup + rebrand to loct
- add batch index CLI command

### Fixed

- respect suppressions in loct twins command
- handle async def for route detection
- multiline imports and __all__ duplicate detection
- refactor Pipelines to vanilla JS for expand/filter/search
- critical path rebasing + file existence validation
- add Pipelines nav tab + remove memex from scripts and hooks
- Add SLED_PATH env for multi-instance support
- Use ASCII characters in deprecation warning

## [0.8.5-dev] - 2026-01-12

### Added

- add sys.modules monkey-patch detection
- Add loctree-lsp crate and VSCode extension scaffold
- Add health score gauge, audit tab & style unification
- add clickable file links to Pipelines tab
- v0.8.0 - auto-snapshot + dual license MIT/Apache-2.0
- Add rmcp-mux launcher, configs, and documentation
- Embedded blog system with markdown rendering
- Add micro-animation to loct startup + rebrand to loct
- add batch index CLI command

### Fixed

- respect suppressions in loct twins command
- handle async def for route detection
- multiline imports and __all__ duplicate detection
- refactor Pipelines to vanilla JS for expand/filter/search
- critical path rebasing + file existence validation
- add Pipelines nav tab + remove memex from scripts and hooks
- Add SLED_PATH env for multi-instance support
- Use ASCII characters in deprecation warning

## [0.8.4] - 2026-01-10

### Added

- add sys.modules monkey-patch detection
- Add loctree-lsp crate and VSCode extension scaffold
- Add health score gauge, audit tab & style unification
- add clickable file links to Pipelines tab
- v0.8.0 - auto-snapshot + dual license MIT/Apache-2.0
- Add rmcp-mux launcher, configs, and documentation
- Embedded blog system with markdown rendering
- Add micro-animation to loct startup + rebrand to loct
- add batch index CLI command

### Fixed

- respect suppressions in loct twins command
- handle async def for route detection
- multiline imports and __all__ duplicate detection
- refactor Pipelines to vanilla JS for expand/filter/search
- critical path rebasing + file existence validation
- add Pipelines nav tab + remove memex from scripts and hooks
- Add SLED_PATH env for multi-instance support
- Use ASCII characters in deprecation warning

## [0.8.4] - 2026-01-09

### Added

- **Function parameter indexing**: `ExportSymbol` now includes `params: Vec<ParamInfo>` with name, type annotation, and
  default value info
- **Parameter search in `loct find`**: New `=== Parameter Matches ===` section shows matches in function parameters
    - JS/TS: Full type extraction via OXC parser (e.g., `props: ExportDialogProps`)
    - Python: Regex-based with support for `*args`, `**kwargs`, type hints, defaults
    - Rust: Generic param extraction, filters out `self`/`&self`/`&mut self`
- **`ParamInfo` struct**: `{name, type_annotation, has_default}` stored in snapshot
- **Backwards-compatible snapshots**: Old snapshots work via `#[serde(default)]`
- **Multi-query support**: `loct find foo bar baz` combines terms with OR regex (`foo|bar|baz`)
- **Zero-latency search**: `loct find` now loads snapshot first instead of rescanning (15x speedup: 13.7s → 0.9s)
- **Cross-match results**: Multi-query shows `=== Cross-Match Files ===` for files containing 2+ different terms
- **loctree-mcp find improvements**:
    - Case-insensitive regex matching (consistent with CLI)
    - Parameter search in MCP tool output
    - Multi-query support via `foo|bar` syntax
- **Fuzzy fallback for `loct query where-symbol`**: When no exact match found, returns semantic matches via
  `find_similar`
    - Enables hook augmentation for typos/partial names (e.g., `analyze_python` → `analyze_py_file`)
    - Top 5 matches with score > 0.5 returned with similarity scores

### Changed

- Rust analyzer now correctly sets `kind` field (was hardcoded to "const" for all items)

### Example

```bash
# Single query with param search
$ loct find request
=== Symbol Matches (10) ===
  ...
=== Parameter Matches (34) ===
  api-router/app/main.py:342 - request: Request in playground_gated()
  api-router/app/routers/auth.py:51 - request: LoginRequest in login()

# Multi-query with cross-match
$ loct find recording_finalize_session transcription_finalize_stream session_not_found
=== Symbol Matches (3) ===
  src/routers/recordings.rs:120 - recording_finalize_session
  src/routers/transcription.rs:88 - transcription_finalize_stream
  src/errors.rs:45 - session_not_found
=== Cross-Match Files (1) ===
  Files containing 2+ different query terms:
  src/handlers/session.rs (2 terms)
    ├─ recording_finalize_session (line 234) - function recording_finalize_session
    ├─ session_not_found (line 45) - function session_not_found
```

### Metrics (validated on 4 repos)

| Repo            | Indexed Params |
|-----------------|----------------|
| mlx-omni-server | 225            |
| lbrx-services   | 899            |
| loctree-suite   | 862            |
| vista           | 2362           |

## [0.8.3] - 2026-01-05

### Added

- Add loctree-lsp crate and VSCode extension scaffold
- Add health score gauge, audit tab & style unification
- add clickable file links to Pipelines tab
- v0.8.0 - auto-snapshot + dual license MIT/Apache-2.0
- Add rmcp-mux launcher, configs, and documentation
- Embedded blog system with markdown rendering
- Add micro-animation to loct startup + rebrand to loct
- add batch index CLI command

### Fixed

- multiline imports and __all__ duplicate detection
- refactor Pipelines to vanilla JS for expand/filter/search
- critical path rebasing + file existence validation
- add Pipelines nav tab + remove memex from scripts and hooks
- Add SLED_PATH env for multi-instance support
- Use ASCII characters in deprecation warning

## [0.8.0] - 2026-01-03

### Added

- **Dual licensing**: MIT OR Apache-2.0 (standard for Rust ecosystem)
- **Auto-snapshot for all commands**: CLI commands now auto-scan if no snapshot exists
    - Fixes agent adoption gap where commands failed without explicit `loct` first
    - `loct i <file>`, `loct jq`, `loct trace` now work immediately in any repo

### Fixed

- **Critical path rebasing bug**: Running `loct` from directory A targeting files in directory B now uses B's git
  context
    - Previously: `loct i /other/repo/file.tsx` scanned CWD instead of target's repo
    - Now: Correctly finds and uses target file's repository root
- **File existence validation**: Non-existent files now return error instead of misleading "Safe to remove"
- **Git context per-root**: `get_git_info()` now accepts root parameter and uses `.current_dir(root)` for all git
  commands

### Changed

- `candidate_snapshot_paths()` uses root-specific git context via `git_context_for(root)`
- Tests updated to verify new path rebasing behavior

## [0.7.4] - 2025-12-18

### Added

- Universal loctree-mcp, rmcp-mux half-close fix, python exec fixtures
- Deprecation warning for loctree binary, make version targets, .loctignore
- add doctor command with --help and short aliases
- add short aliases and doctor command
- add deprecation warnings for 0.7.0 command migration
- Phase 1+2 - findings.json and manifest.json artifacts
- Phase 4 - query improvements

### Changed

- Split rust.rs monolith into modular structure

### Fixed

- version-bump.sh changelog generation - fix awk pattern and add git log limit
- remove unused AutoOptions import in help.rs
- Apply rustfmt to extract_plugin_identifier
- replace unstable floor_char_boundary with stable impl
- add serial_test + bump(0.7.1): release version
- resolve collapsed if statement warnings

## [0.7.3] - 2025-12-17

### Changed

- **Refactor**: Split `rust.rs` monolith into modular structure (`imports.rs`, `naming.rs`, `preprocess.rs`, `tauri.rs`,
  `usages.rs`)
- **Refactor**: Split analyzer and CLI modules into submodules for maintainability

### Fixed

- Improved plugin identifier inference for Tauri plugins
- Removed unused `AutoOptions` import in help.rs

## [0.7.2] - 2025-12-17

### Added

- Framework-aware twins detection (distinguishes intentional patterns from actual duplicates)
- Dynamic exec tracking for Node.js runtime API detection
- Rust module fixtures and analyzer options

### Fixed

- `floor_char_boundary` compatibility - replaced unstable API with stable implementation
- Rust module detection improvements
- `rustfmt` applied to `extract_plugin_identifier`

## [0.7.1] - 2025-12-17

### Fixed

- Added `serial_test` for test isolation (fixes flaky parallel tests)

### Documentation

- Added v0.7.0 comparative benchmarks analysis

## [0.7.0] - 2025-12-16

### Added

- **Artifact-first architecture** - Major refactor introducing persistent artifacts:
    - `findings.json` - All analysis results in structured format
    - `manifest.json` - Codebase metadata and configuration
- **`loct doctor` command** - Diagnose loctree setup and environment issues
- **Short aliases** - `loct a` for analyze, `loct s` for slice, etc.
- Deprecation warnings for commands migrating in 0.7.0

### Changed

- CLI help output updated for artifact-first workflow
- Query improvements (Phase 4)

### Documentation

- Updated all docs for v0.7.0 artifact-first architecture
- Enhanced AI/CLI guidance

## [0.6.23] - 2025-12-15

### Added

- **`loct audit` command** — Full codebase audit with actionable findings in one command
    - Combines ALL structural analyses: cycles + dead + twins + orphans + shadows + crowds
    - Perfect for getting a complete picture of codebase health on day one
    - Shows summary with counts and top findings for each category
    - Supports `--json` for CI integration and `--include-tests` for test files
    - Usage: `loct audit`, `loct audit --json`, `loct audit src/`
    - Designed for agents and developers who want actionable feedback immediately

### Technical

- Added `Audit` command variant and `AuditOptions` struct
- Implemented `handle_audit_command` in analysis handlers
- Aggregates: cycles, dead exports, twins, orphan files, shadow exports, crowds

## [0.6.22] - 2025-12-15

### Added

- **`loct health` command** — Quick health check summary in one command
    - Combines cycles + dead exports + twins analysis into a single summary
    - Shows: cycle count (hard/structural), dead exports (high/low confidence), twins count
    - Supports `--json` for CI integration
    - Usage: `loct health`, `loct health --json`, `loct health src/`
    - Designed for quick sanity checks before commits or in CI pipelines

### Technical

- Added `Health` command variant and `HealthOptions` struct
- Implemented `handle_health_command` in analysis handlers

## [0.6.15] - 2025-12-13

### Added

- **jq-style query mode** — Query snapshot data directly with jq syntax
    - `loct '.metadata'` — Extract metadata from snapshot
    - `loct '.files | length'` — Count files in codebase
    - `loct '.edges[] | select(.from | contains("api"))'` — Filter edges
    - Uses jaq (Rust-native jq implementation) for zero external dependencies
    - Flags: `-r` (raw), `-c` (compact), `-e` (exit status), `--arg`, `--argjson`
    - Auto-discovers latest snapshot from `.loctree/*/snapshot.json`
    - Explicit snapshot: `loct '.files' --snapshot path/to/snapshot.json`
    - Usage: `loct '<filter>' [flags]` — filter must come before flags

- **Progress spinners** — Visual feedback during `loct auto`:
    - "Building snapshot..." spinner after scanning completes
    - "Generating artifacts..." spinner before writing reports
    - Eliminates "dead time" between scan and output

- **`find_latest_snapshot_in(root)` API** — Thread-safe snapshot discovery
    - Allows passing explicit root directory instead of relying on `cwd`
    - Fixes flaky tests in parallel execution environments

### Changed

- **Dirty worktree now allows fresh scans** — Previously, dirty worktree would skip scanning even when files changed.
  Now only clean worktree + same commit skips (actual no-change scenario). Users can scan during refactoring without
  committing first.

- **Updated "Next steps" output** — Now shows modern commands:
    - `loct --for-ai` (project overview for AI agents)
    - `loct slice <file> --json` (context extraction)
    - `loct twins` (dead parrots + duplicates)
    - `loct '.files | length'` (jq queries)
    - `loct query who-imports <f>` (quick graph queries)

### Fixed

- **Thread-safe snapshot tests** — Removed `set_current_dir` calls that caused race conditions in parallel test
  execution

### Technical

- Added version requirement (`0.6.15`) to `loctree_server` dependency for crates.io publishing

## [0.6.9] - 2025-12-11

### Added

- **Test coverage analysis (`loct coverage`)**: Structural test coverage for Tauri apps
    - Cross-references production usage (FE invoke/emit) with test imports
    - CRITICAL: Handlers called from production but not tested
    - HIGH: Events emitted but not tested
    - MEDIUM: Exports without test imports
    - LOW: Tested but unused (potential dead test code)
    - Usage: `loct coverage`, `loct coverage --handlers`, `loct coverage --json`

- **`--limit` option for commands**: Control output size for large codebases
    - `loct commands --limit 10 --json` - First 10 command bridges

### Improved

- **Duplicate detection accuracy**: Dramatically reduced false positives
    - Re-exports (`pub use`) no longer counted as duplicates (was ~80% noise)
    - Generic method names (`new`, `from`, `clone`, etc.) filtered out
    - Test fixtures excluded from analysis reports
    - Cross-crate duplicates now surfaced first (real issues)

- **Agent bundle (`loct agent`)**: Fixed `files_analyzed: 0` bug
    - ReportSection now built for `--for-agent-feed` mode
    - Summary stats correctly populated

### Fixed

- **JS assets pollution**: Fixed cytoscape.min.js files being written to repo root
    - Now only writes to actual report directory, not current directory

## [0.6.7] - 2025-12-10

### Added

- **Bundle distribution analysis (`loct dist`)**: Analyze production bundles using source maps to find truly dead
  exports
    - Symbol-level detection via VLQ Base64 decoding of source map mappings
    - File-level fallback when source maps lack `names` array
    - Compare source exports vs bundled symbols to verify tree-shaking effectiveness
    - Usage: `loct dist dist/bundle.js.map src/`

### Technical

- Full VLQ (Variable Length Quantity) Base64 decoder for source map v3 format
- Delta-encoded position parsing for accurate symbol mapping
- 11 new unit tests for dist module

## [0.6.3-dev] - 2025-12-08

### Added

- **Python stdlib/library mode**: Exports in `__all__` are now recognized as public API
    - `is_python_stdlib_export()` heuristic for CPython stdlib detection
    - Reduces false positives from 100% → <20% on python/cpython repository
- **WeakMap/WeakSet registry pattern detection**: React-style codebases using weak collections now tracked
    - Added `has_weak_collections` flag in FileAnalysis
    - Improves React dead export detection accuracy
- **TypeScript .d.ts re-export tracking**: When .d.ts files re-export from .js/.ts, implementation exports are marked as
  used
    - Prevents false positives for type definition files
- **Dart/Flutter language support**: Full support for Dart projects
    - Detects pubspec.yaml for project identification
    - Analyzes Dart-specific import/export patterns
- **Go language support**: Full support for Go projects
    - Analyzes Go import/export patterns
    - Detects Go modules and packages

### Improved

- **Python library mode**: False positive rate reduced from 100% → <20% on cpython stdlib
- **React dead export detection**: False positive rate reduced from 40% → ~20%
    - Better handling of component registries and dynamic patterns
- **Svelte dead export detection**: False positive rate reduced from 70% → <15%
    - Enhanced template analysis and component resolution
- **Flow type annotation handling**: Better parsing of Flow types in .js files
    - Removed unused `is_flow_file` field from JsVisitor

### Fixed

- **Python UTF-8 crash**: Fixed crashes on emoji characters in Python files (py.rs)
- **Python UTF-8 crash**: Fixed crashes on Devanagari numerals and other non-ASCII characters
- **Binary file detection**: Improved detection to prevent UTF-8 parsing crashes on binary files

### Performance

Verified against major open-source repositories:

- **rust-lang/rust**: 35,387 files, 0% FP (EXCEPTIONAL)
- **golang/go**: 17,182 files, ~0% FP (PERFECT)
- **facebook/react**: 3,951 files, ~20% FP (improved from 40%)
- **sveltejs/svelte**: 405 files, <15% FP (improved from 70%)
- **python/cpython**: 842 files, <20% FP (improved from 100%)

---

## [Released]

## [0.5.18] - 2025-12-06

### 🎯 Major: Twins Analysis (Semantic Duplicate Detection)

New `loct twins` command for semantic duplicate analysis — a comprehensive tool for detecting code organization issues
inspired by Monty Python's Dead Parrot sketch.

### Added

**Dead Parrots Detection**

- Finds exports with 0 imports across the entire codebase
- Filters out Tauri registered handlers (not false positives)
- Filters out locally used symbols within the same file
- Smart detection reduces false positives by ~75% compared to naive analysis

**Exact Twins Detection**

- Identifies same symbol name exported from multiple files
- Highlights potential naming conflicts and duplicate implementations
- Groups twins by symbol name for easy review

**Barrel Chaos Analysis**

- **Missing barrels**: Directories with multiple files imported externally but no `index.ts`
- **Deep re-export chains**: Detects `index.ts → sub/index.ts → sub/sub/index.ts` (depth > 2)
- **Inconsistent import paths**: Same symbol imported via different paths

### Usage

```bash
loct twins              # Full analysis: dead parrots + exact twins + barrel chaos
loct twins --dead-only  # Only exports with 0 imports
loct twins --path src/  # Analyze specific path
```

### Technical Details

- 638 tests passing (added 12 new tests for twins detection)
- ~800 lines of new analyzer code in `twins.rs` and `barrels.rs`
- Zero breaking changes to public API

---

## [0.5.17] - 2025-12-06

### 🎯 Major: False Positive Massacre

This release dramatically reduces false positives in dead export detection across all major frameworks. Based on smoke
tests against 11 real-world repositories (loctree-dev, SvelteKit, FastAPI, Vue Core, GitButler, etc.), we identified and
fixed 6 critical FP sources.

### Added

**Rust Same-File Usage Detection** (Agent 1)

- Detects types used in struct/enum field definitions within the same file
- Handles generic parameters: `Vec<T>`, `Option<T>`, `HashMap<K,V>`, `Result<T,E>`
- Parses tuple structs, enum variants with data, and associated types
- **NEW**: Detects const usage in generics: `fn foo::<BUFFER_SIZE>()`, `create_buffer::<SIZE, _>()`
- Fixes 100% FP rate in Rust projects where types were only used as field types

**Rust Crate-Internal Import Resolution** (Agent 7, 8, 9)

- Resolves `use crate::foo::Bar` imports to actual file paths
- Handles `use super::Bar` and `use self::foo::Bar` relative imports
- Supports nested brace imports: `use crate::{foo::{A, B}, bar::C}`
- Fuzzy symbol matching for complex multi-line imports
- `CrateModuleMap` for module path → file path resolution
- Fixes MENU_GAP-style false positives in Zed and similar large Rust codebases

**SvelteKit Virtual Module Resolution** (Agent 2)

- Recognizes SvelteKit virtual modules: `$app/*`, `$lib/*`, `$env/*`, `$service-worker`
- Parses `vite.config.js/ts` for custom path aliases
- Resolves tsconfig `paths` with wildcard patterns
- Virtual modules now resolve to `__virtual__/$app/forms` style paths
- Fixes 83% FP rate in SvelteKit projects

**Python FastAPI Decorator Tracking** (Agent 3)

- Extracts type references from decorator parameters:
    - `response_model=User` → marks `User` as used
    - `Depends(get_db)` → marks `get_db` as used
    - `List[Schema]`, `Optional[Model]` → extracts inner types
- Recognizes FastAPI/Pydantic factories: `Query`, `Body`, `Path`, `Header`, `Cookie`, `Form`, `File`
- Fixes 100% FP rate in FastAPI projects

**Svelte Template Function Call Detection** (Agent 4)

- Parses Svelte template expressions: `{formatDate(value)}`
- Detects event handlers: `on:click={handleClick}`, `on:submit|preventDefault={submit}`
- Recognizes bind directives: `bind:value={store}`, `bind:this={element}`
- Handles transitions and actions: `transition:fade`, `use:tooltip`
- Extracts component usage: `<MyComponent />`, `<svelte:component this={comp}/>`
- Fixes 40-50% FP rate in Svelte projects (GitButler-level)

**Generated Code Detection** (Agent 5)

- Path-based detection: `**/generated/**`, `**/*.generated.*`, `**/*.g.dart`
- Content-based markers: `@generated`, `DO NOT EDIT`, `auto-generated`, `THIS FILE IS GENERATED`
- Skips generated files in dead export analysis
- Integrated into `FileAnalysis.is_generated` flag

**Vue SFC Script Parsing** (Agent 6)

- Extracts `<script>` and `<script setup>` blocks from `.vue` files
- Supports both Composition API and Options API
- Routes extracted scripts through standard JS/TS analyzer
- Fixes 86% FP rate in Vue projects

### Changed

- `FileAnalysis` now includes `is_generated` flag from both path and content analysis
- Virtual module resolution integrated into standard import resolution pipeline
- Svelte files now analyzed in two passes: script block + template expressions

### Fixed

- Clippy warnings in `resolvers.rs` (collapsible if/else, regex in loop)

### Technical Details

- 667 tests passing (added 27 new tests for new features)
- ~800 lines of new analyzer code across 6 modules
- Zero breaking changes to public API

---

## [0.5.16] - 2025-12-05

### Added

- Progress UI for long-running symbol searches
- Improved `find_symbol` with regex pattern matching and path filters

### Changed

- Crowd match reasons refactored for clearer similarity explanations
- Report UI polish for crowd detection display

---

## [0.5.15] - 2025-12-04

### Added

- **Crowd Detection**: Identifies functional duplicate files using:
    - Structural similarity (import/export patterns)
    - Content fingerprinting
    - Clustering with configurable thresholds
- Router-based subpages in landing for better navigation
- Dead code analysis integrated into HTML reports

### Changed

- Test file handling improved for Python and TypeScript fixtures

---

## [0.5.14] - 2025-12-03

### Added

- Initial crowd detection module with similarity scoring
- Logo assets updated with thicker stem and adjusted positions

### Changed

- Branding assets refresh

---

## [0.5.13] - 2025-12-09

### Added

- Circular-import quick wins now use real cycle data from `cycles::find_cycles`, surfaced in QuickWin output and tests.
- Atomic snapshot writes (via `write_atomic`) for report/SARIF/analysis/dead/handlers/circular/races artifacts to avoid
  partial files and corruption.
- Alias-aware dynamic import reachability (`@core/*`, Windows case-insensitive) with new fixtures/tests.

### Changed

- Unified CLI help paths: `--help-full` is handled consistently in both binaries; `search` hints now use `loctree …`
  wording everywhere.
- Install/docs/CI instructions standardized on `cargo install loctree`; removed `curl | sh` mentions.
- Tooltip layer helper (`.tooltip-floating` z-index 9999) and scrollbar CSS now ship with fallbacks.
- Landing/AI README/changelog bumped to 0.5.12+ alignment for release metadata.

### Fixed

- Removed panics in QuickWin/SARIF/snapshot paths; errors now bubble as `Result` or log warnings instead of crashing.
- Mutex poison recovery in `root_scan` avoids thread panics; dead export matching handles Windows casing correctly.
- Ignored and removed generated `**/.loctree/**/report.html` fixture artifacts from the repo.

## [0.5.12] - 2025-12-08

### Added

- Atomic writes for snapshot artifacts (report/SARIF/JSON) to prevent partial files on crash or interrupt.
- Alias-aware dynamic import reachability (handles `@core/*` prefixes and Windows casing) with new tests.

### Changed

- Unified install docs and prompts to `cargo install loctree`.
- Quick-win JSONL and SARIF generation now return/log errors instead of panicking.
- Tooltip layer helper to avoid z-index clashes in reports.

## [0.5.10] - 2025-12-03

### Added

- Snapshot artifacts now live under `.loctree/<branch@commit>/` and `save()` skips rewrites for the same commit/branch (
  with a hint when the worktree is dirty).
- Base scans print a concise human summary (files, handlers, languages, elapsed); `--serve` binds 0.0.0.0:5075 with
  loopback/random fallback and warns about the upcoming `loct report --serve` migration.

### Fixed

- Python analyzer: dead-export FP reduction (imported symbols, mixin inheritance, callbacks), line numbers in dead
  output, faster scan path; `who-imports` query now reports Python imports correctly.
- Report WASM: removed deprecated-init console warning from `report_wasm.js`.
- HTML/auto-artifacts no longer auto-open during tests/builds; analysis artifacts key off parsed output mode (JSON/SARIF
  now written reliably).

### Changed

- HTML reports are generated only when explicitly requested (`loct report --serve` or `--report`); global `--serve`
  remains as a backwards-compatible alias with a warning.
- Snapshot writing warns and reuses the existing snapshot when nothing changed for the current commit/branch.

## [0.5.7] - 2025-12-01

### Added

- **One-shot artifact bundle**: Bare `loct`/`loctree` now saves the full analyzer output to `.loctree/` alongside
  `snapshot.json` — `report.html` (with graph), `analysis.json`, `circular.json`, and `py_races.json`, so you don't need
  to run extra commands after a scan.

### Changed

- **Rebrand alignment**: Updated repository/org references to `Loctree/Loctree` and refreshed version strings to v0.5.7
  across crates and docs.
- **Release hygiene**: Rust formatting/clippy cleanups applied for the 0.5.7 publish pipeline.

## [0.5.6] - 2025-12-01

### Fixed

- **AST Parser JSX Fix**: Disabled JSX parsing for `.ts` files (only enabled for `.tsx`/`.jsx`). Previously, TypeScript
  generics like `<T>` were incorrectly parsed as JSX tags, causing entire files like `api.ts` to fail parsing.
- **Template Literal Support**: Added detection of Tauri `invoke` calls using backticks (`` `cmd` ``). Commands like
  `` safeInvoke(`create_user`) `` are now correctly identified.
- **False Positive Reduction**: Added exclusion lists to prevent non-Tauri functions from being detected as commands:
    - `NON_INVOKE_EXCLUSIONS`: ~35 patterns like `useVoiceCommands`, `runGitCommand`, `executeCommand`
    - `INVALID_COMMAND_NAMES`: CLI tools like `node`, `cargo`, `pnpm`, `git`
- **Payload Requirement**: `CommandRef` is now only created when a valid command name payload exists, eliminating false
  positives where function names were mistaken for commands.

### Added

- **Git Context in Reports**: Added `git_branch` and `git_commit` fields to `ReportSection` for future Scan ID system
  integration.
- **Parser Debug Logging**: Added error logging when OXC parser encounters issues (visible with `--verbose`).

### Changed

- **Vista Project Results**: Improved detection accuracy:
    - Frontend commands: 170 → 254 (+49%)
    - Missing handlers: 18 → 5 (72% reduction in false positives)
    - Unused handlers: 137 → 57 (58% reduction in false positives)

## [0.5.5] - 2025-11-30

### Fixed

- **AI Context Safety**: Limited verbosity of `slice` and `circular` commands to prevent context flooding in LLMs:
    - `slice`: Truncates Deps/Consumers lists > 25 items (showing "... and N more").
    - `circular`: Compresses dependency cycles longer than 12 steps into `head -> ... (N intermediate) ... -> tail`
      format.

## [0.5.4] - 2025-11-30

### Added

- **Loctree CI workflow**: Separate GitHub Actions workflow that runs loctree self-analysis on all inner crates (
  loctree-rs, reports, landing) with HTML report artifacts.
- **Version sync script**: `scripts/sync-version.sh` automatically synchronizes version across all crates and hardcoded
  strings during releases.
- **`loct` CLI alias**: Short alias for `loctree` command for faster typing.

### Changed

- **Binary structure refactored**: Moved CLI entry points from `src/main.rs` to `src/bin/loctree.rs` and
  `src/bin/loct.rs` to eliminate "multiple build targets" warning.
- **CI matrix**: Loctree CI now runs on both Ubuntu and macOS.

### Fixed

- **Version sync**: All version references (reports footer, lib.rs doc URL, landing easter eggs) now properly synced to
  0.5.4.

## [0.5.3] - 2025-11-29

### Added

- **COSE-Bilkent graph layout**: Added force-directed layout algorithm for better dependency graph visualization in HTML
  reports.
- **`report-leptos` library crate**: Extracted HTML report generation into a standalone crate (v0.1.1) for reuse and
  cleaner architecture.

### Changed

- **Report UI redesign**: New dark/light theme with improved visual hierarchy and accessibility.
- **Shared JS assets**: Moved graph visualization libraries (Cytoscape, Dagre, COSE-Bilkent) to the library crate.

### Fixed

- **Nested conditions refactored**: Improved `root_scan` and `detect` modules using Rust 2024 if-let chains.

## [0.5.2] - 2025-11-28

### Changed

- Updated Semgrep policy configuration to the latest defaults.

### Fixed

- Synced version references (landing assets and metadata) with the v0.5.2 release to resolve develop/main divergence.

## [0.5.1] - 2025-11-28

### Added

- **Entry point detection**: Proper regex-based detection for Python and Rust entry points:
    - Python: `__main__.py` files and `if __name__ == "__main__":` blocks
    - Rust: `fn main(` and async runtime attributes (`#[tokio::main]`, `#[async_std::main]`)
    - Uses regex with line-start anchors to avoid false positives in comments/strings
- **Lazy import detection**: React.lazy() patterns now properly tracked as dynamic imports:
    - Detects `import('./Foo').then(m => ({ default: m.Bar }))` syntax
    - Prevents false positives for lazy-loaded components in dead export detection

### Changed

- **Python stack detection**: Extended default ignores for Python projects:
    - Added: `packaging`, `logs`, `.fastembed_cache`, `.cache`, `.uv`
    - Covers common ML/data caches and uv package manager artifacts
- **Git hooks restructured**:
    - `pre-commit`: Fast checks on staged files only (fmt auto-fix, cargo check, unit tests)
    - `pre-push`: Comprehensive validation (clippy -D warnings, full tests, integration tests, dogfooding, semgrep)

### Fixed

- **Slice file matching**: Now prioritizes exact path match over `ends_with` match; warns when multiple files match the
  same target to avoid selecting wrong file (e.g., `backend.py` picking monorepo's root instead of src).
- **Tauri generate_handler! parsing**: Fixed extraction of function names from module-qualified paths (e.g.,
  `commands::foo::bar` now correctly registers `bar` instead of `commands`). Also handles `#[cfg(...)]` attributes
  inside the macro block without breaking the parser.

## [0.5.0-rc] - 2025-11-28

### Added

- **Holographic Slice** (`slice` command): Extract 3-layer context for AI agents from any file:
    - **Core**: Target file itself (full content)
    - **Deps**: Files imported by target (BFS traversal up to depth 2)
    - **Consumers**: Files that import target (with `--consumers` flag)
    - JSON output for piping directly to AI: `loctree slice src/App.tsx --json | claude`
- **Auto-detect stack**: Automatically detects project type from:
    - `Cargo.toml` → Rust (adds `target/` to ignores)
    - `tsconfig.json` / `vite.config.*` → TypeScript (adds `node_modules/` to ignores)
    - `pyproject.toml` → Python (adds `.venv/`, `__pycache__/` to ignores)
    - `src-tauri/` → Tauri hybrid (sets `--preset-tauri` automatically)
- **Incremental scanning**: Uses file mtime to skip unchanged files. Typical re-scans now show "32 cached, 1 fresh"
  instead of re-parsing everything.
- **`--full-scan` flag**: Forces re-analysis of all files, bypassing mtime cache.
- **`--consumers` flag**: Include consumer layer in slice output.
- Wired existing modules to CLI:
    - `--circular`: Find circular imports using SCC algorithm
    - `--entrypoints`: List entry points (main, __main__, index)
    - `--sarif`: SARIF 2.1.0 output for CI integration

### Changed

- Rebranded as "AI-oriented Project Analyzer" to reflect the primary use case.
- Help text completely rewritten with slice examples: `loctree slice src/main.rs --consumers`
- Snapshot now stores file mtime for incremental scanning.
- Snapshot edges are always collected (previously only with `--graph`).

### Fixed

- Slice now correctly matches files when edges store paths without extensions.
- Removed unused `SliceConfig` fields (`target`, `json_output`, `deep`).
- Removed unused `Snapshot::file_mtimes()` method.
- Changed all test `unwrap()` to `expect()` with context for cleaner error messages.

## [0.4.7] - 2025-11-28

### Added

- **Snapshot system** ("scan once, slice many"): Running bare `loctree` (no arguments) now scans the project and saves a
  complete graph snapshot to `.loctree/snapshot.json`.
- New `init` command/mode: `loctree init [path]` explicitly creates or updates the snapshot.
- Snapshot contains: file analyses (imports, exports, commands, events), graph edges, export index, command bridges (
  FE↔BE mappings), event bridges (emit↔listen), and barrel file detection.
- Snapshot metadata includes: schema version, generation timestamp, detected languages, file count, total LOC, and scan
  duration.
- Foundation for upcoming "holographic slice" feature (Vertical Slice 2) – context slicing from snapshot.
- **Janitor: circular imports** – new `--circular` flag walks the import graph and reports strongly connected
  components (including self-loops) as cycles in CLI/JSON.
- **Janitor: entry points** – new `--entrypoints` flag detects Python and Rust entry points (e.g.
  `if __name__ == "__main__"`, `fn main`, `#[tokio::main]`) to separate startup scripts from dead code.
- **SARIF output for CI** – new `--sarif` flag emits findings (duplicate exports, missing/unused handlers, dead exports,
  ghost/orphan events) in SARIF 2.1.0 format for GitHub/GitLab integration.
- **Find build artifacts** – new `--find-artifacts` flag finds common build artifact directories (`node_modules`,
  `.venv`, `target`, `dist`, `build`, `.cache`, `Pods`, `DerivedData`, etc.) and outputs their absolute paths one per
  line. Useful for cleaning up disk space or excluding from Spotlight indexing. Does not recurse into found
  directories (prune behavior).

### Changed

- Default behavior: bare `loctree` without arguments now runs in Init mode (creates snapshot) instead of Tree mode.
- Added `Serialize`/`Deserialize` derives to core analysis types for snapshot persistence.
- Made `root_scan` and `coverage` modules public for snapshot building.
- Snapshot summary now shows actionable next steps (`loctree . -A --json`, `loctree . -A --preset-tauri`) instead of
  not-yet-implemented slice command.

### Fixed

- `--fail-on-missing-handlers`, `--fail-on-ghost-events`, `--fail-on-races` flags now actually work: they return
  non-zero exit code when issues are detected (previously flags were parsed but had no effect).
- Python analyzer: fixed resolution of relative imports like `from . import mod` and `from .mod import name` so that
  star re-exports and `__all__` expansion are reflected correctly in the graph and dead-code analysis.

## [0.4.6] - 2025-11-27

### Added

- **Janitor Mode tools**:
    - `--check <query>`: Finds existing components/symbols similar to the query (Levenshtein distance) to prevent
      duplication before writing new code.
    - `--dead` (alias `--unused`): Lists potentially unused exports (defined but never imported).
    - `--confidence <level>`: Filters dead exports (use `high` to hide implicit uses like `default` exports or
      re-exports).
    - `--symbol <name>`: Quickly finds all occurrences of a symbol (definitions and usages) across the project.
    - `--impact <file>`: Analyzes dependency graph to show what would break if the target file changed.
    - `--scan-all`: Option to include `node_modules`, `target`, `.venv` in analysis (normally ignored by default).
- **Pipeline Confidence**: "Ghost events" now include confidence scores and recommendations (`safe_to_remove` vs
  `verify_dynamic_value`).
- **Graph UX**: Sticky tooltips on nodes (persist on hover/click) for easier reading and copying paths.

### Changed

- Default behavior: `loctree` (no args) now ignores `node_modules`, `target`, `.venv` by default to prevent massive
  snapshots. Use `--scan-all` to override.
- CLI output: Removed emojis from standard output for cleaner, grep-friendly text.

### Fixed

- Fixed false positives in "dead exports" where re-exports were not counted as usage.
- Fixed double-counting of named re-exports in parser.
- Fixed tooltip flickering in HTML report graph.

## [0.4.4] - 2025-11-27

### Security

- Replaced unmaintained `json5` crate (RUSTSEC-2025-0120) with actively maintained `json-five` for tsconfig.json
  parsing. No API or behavior changes.

### Fixed

- Added Semgrep suppression comments with safety justifications for `innerHTML` usage in `graph_bootstrap.js` (all user
  data is escaped via `escapeHtml()`; other values are numbers from analyzer).
- Replaced bare `unwrap()` with `expect()` providing context in snapshot module tests to comply with project linting
  rules.

## [0.4.3] - 2025-11-26

### Fixed

- HTML report no longer renders duplicate graph toolbars; inline graph panels are hidden so the drawer is the single
  source of controls (no double scrollbars).

### Changed

- Documentation updated for the streamlined graph UI.

## [0.4.2] - 2025-11-26

### Fixed

- Multi-root analyzer now merges frontend calls and backend handlers across roots, so Tauri coverage/commands summaries
  stop flagging cross-root missing/unused pairs.
- Duplicate export detection skips re-exports and `default` exports from declaration files, reducing barrel/index.ts
  false positives.
- Event names declared as constants (TS/JS/Rust, including imported consts) are resolved for emit/listen analysis,
  cutting ghost/orphan noise.

### Changed

- Analyzer scanning logic was extracted into dedicated modules (`scan.rs`/`root_scan.rs`), shrinking `runner.rs` and
  preparing the upcoming subcommands without changing CLI behavior.

## [0.4.1] - 2025-11-25

### Fixed

- TS path resolver now walks parent dirs to find `tsconfig.json` (or a base in parent), merges `extends`, parses
  JSONC/JSON5 (tsconfig with comments/trailing commas), and canonicalizes `baseUrl/paths`, so aliases like `@/*` resolve
  instead of returning null even when tsconfig lives above the scanned root.

## [0.4.0] - 2025-11-25

### Added

- `--ai` concise output mode that emits a compact JSON summary with top issues instead of full per-file payloads.
- Dead-symbol controls: `--top-dead-symbols` (default 20) to cap lists and `--skip-dead-symbols` to omit them entirely.
- AI/bridge payload now keeps a compact list of FE↔BE Tauri command mappings (`bridges`) so agents can jump to
  handlers/call-sites.

### Changed

- AI/summary views respect the new limits to reduce noisy output; help/README refreshed to mention the AI flags and
  limits.
- `--preset-tauri` now auto-ignores common build artifacts (`node_modules`, `dist`, `target`, `build`, `coverage`,
  `docs/*.json`) to cut report noise without extra flags.

### Fixed

- Resolved clippy warning in the open-server editor launcher (mutable closure), no functional change.

## [0.3.8] - 2025-11-24

### Added

- Report UI reorganized into tabs (Overview / Duplicates / Dynamic imports / Tauri coverage / Graph anchor) with a
  dedicated bottom drawer for the graph and controls.
- Help text split per mode (Tree / Analyzer / Common) and expanded examples; graph/drawer behavior documented.
- Python analyzer refinements: `--py-root` (repeatable) for extra roots, `resolutionKind` + `isTypeChecking` on imports,
  dynamic import tagging, `__all__` expansion for star imports.

### Fixed

- Dark-mode toggle in the graph drawer no longer panics when Cytoscape style is not ready.
- Resolved stray brace/formatting issues in CLI help output.

## [0.3.6] - 2025-11-23

### Added

- Python analyzer: TYPE_CHECKING-aware imports (`isTypeChecking`), dynamic import tagging (`importlib.import_module`,
  `__import__`), `__all__` expansion for star imports, and stdlib vs local disambiguation (`resolutionKind`).
- New flag `--py-root <path>` (repeatable) to add extra Python package roots for resolution.

### Changed

- JSON schema bumped to `1.2.0`; per-import records now include `resolutionKind` and `isTypeChecking`. Fixtures count as
  dev noise in duplicate scoring.

## [0.3.5] - 2025-11-24

### Added

- TS/JS resolver now honors `tsconfig.json` (`baseUrl` + `paths` with `*` patterns) for imports and re-exports,
  improving FE↔BE linkage and graph accuracy when aliasing is heavy.

### Changed

- Graph/import resolution for non-relative specs prefers tsconfig aliases before falling back to relative heuristics;
  reduces “unresolved” noise in JSON/HTML/CLI reports.

## [0.3.4] - 2025-11-24

### Added

- FE↔BE coverage view now captures generic `invoke`/`safeInvoke` call sites and renamed Tauri handlers; surfaced in
  `aiViews.coverage`.
- `aiViews.tsconfig` summarizes `baseUrl`/aliases and highlights unresolved aliases plus `include`/`exclude` drift.
- Public-surface exports (barrels/index/mod.rs) are flagged in `symbols`/`clusters`/`deadSymbols` to prioritize cleanup.

### Changed

- Patch release bump for the above analyzer JSON improvements; no CLI-breaking changes.

## [0.3.3] - 2025-11-24

### Added

- JSON schema metadata (`schema`, `schemaVersion`, `generatedAt`, `rootDir`, `languages`) plus deterministic ordering
  for easier machine use.
- Richer per-file records: stable `id`, `language`, `kind` (code/test/story/config/generated), `isTest`, `isGenerated`,
  import symbol lists with `resolvedPath`, export `exportType` + `line`.
- Derived AI views in JSON: `commands2` (canonical handler + call-sites + status), `symbols`/`clusters`, and `aiViews` (
  default export chains, suspicious barrels, dead symbols, CI summary, coverage stats with renamed handlers + generic
  call sites, tsconfig summary with aliases/include|exclude drift).
- `--verbose` flag and auto-creation of parent directories for `--html-report` (matching `--json-out`).

### Changed

- JSON output remains backward-compatible while exposing the new fields for agents/LLMs; dynamic imports, duplicate
  metadata, and commands are now sorted deterministically.

## [0.3.2] - 2025-11-23

### Added

- Component graph metadata in reports (component id/size, isolates, LOC sum, Tauri FE/BE counts) with UI controls for
  highlighting disconnected components.
- Import graph data builder extracted to `graph.rs` with safer node/edge caps and deterministic layout; HTML graph
  bootstrap served from a dedicated asset.
- AI insights collected from analyzer output (dup/export cascades, missing handlers, huge files) and shown in reports.

### Changed

- Analyzer runner split into focused modules (`graph.rs`, `coverage.rs`, `insights.rs`, `graph_bootstrap.js`), shrinking
  `runner.rs` and `html.rs`.
- Tauri command matching now normalizes names via `heck::ToSnakeCase` and respects focus/exclude globs.
- Generic invoke regexes hardened to handle type parameters without excessive backtracking risk.

## [0.3.1] - 2025-11-22

### Added

- Tauri command coverage view (missing vs unused handlers) grouped by module with linkable locations.
- Import graph drawer and safety limits; buttons for fit/reset/fullscreen/dark-mode and JSON/PNG export.
- Self-hosted Cytoscape asset for CSP/offline friendliness.

### Changed

- Duplicate export filtering honors `--focus` / `--exclude-report` globs; canonical picks non-dev files first.
- `--serve` links url-encoded and open-server startup made more robust.

## [0.3.0] - 2025-11-22

### Added

- **Import Graph Drawer**: When analyzing a single root, the graph is pinned to a collapsible bottom drawer, keeping
  tables readable.
- **Easier-to-hit Tooltips**: Nodes now have a larger hitbox, and tooltips appear near the cursor within the viewport
  boundaries.

### Changed

- The import graph is now attached to a collapsible drawer when analyzing a single root to improve table visibility.

## [0.2.9] - 2025-11-22

### Added

- Graph toolbar upgrades: fit, reset, graph-only fullscreen, dark mode toggle, and tooltips with full path + LOC; node
  size now scales with LOC and uses stable preset layout computed in Rust.
- Graph safety/perf guards: caps at 8k nodes / 12k edges, skips overflow with warnings, and prevents rendering when
  filters empty; legend/hints updated.
- Graph assets self-hosted (CSP-friendly) + buttons to export PNG/JSON snapshots.
- Tauri coverage: FE↔BE matching normalizes camelCase/snake_case aliases; coverage respects `--focus/--exclude-report`
  globs and groups rows by module for readability.

### Changed

- Cleaner import graph (edge labels removed, deduped CSS, more defensive `buildElements`/filter handling).
- Tauri command coverage table restyled for readability (pill rows, clearer columns).
- FE↔BE Tauri matching now normalizes camelCase/snake_case aliases (e.g., `loginWithPin` ↔ `login_with_pin`) to trim
  false missing/unused reports.

## [0.2.8] - 2025-11-22

### Added

- `--focus <glob>` filters the report to show only duplicates where at least one file matches the glob patterns (
  analysis still covers the entire tree).
- `--exclude-report <glob>` allows filtering out noise (e.g., `**/__tests__/**`, `**/*.stories.tsx`) only from the
  duplicate report.

### Changed

- The number of duplicates in CLI/JSON/HTML reflects the above filters; canonical file and score are calculated after
  filtering paths.

## [0.2.7] - 2025-11-22

### Added

- `--graph` optionally appends an interactive import/re-export graph to the HTML report (Cytoscape.js from CDN).
- `--ignore-symbols-preset <name>` (currently `common` → `main,run,setup,test_*`) and support for `foo*` prefixes in
  `--ignore-symbols`.

### Changed

- Help/README/Monika guide updated with new flags; duplicate analysis now considers prefix patterns.

## [0.2.6] - 2025-11-22

### Added

- The `--ignore-symbols` flag for the analyzer – allows omitting specified symbols (e.g., `main,run`) when detecting
  duplicate exports.

### Changed

- Documentation and help updated with the new flag.

## [0.2.5] - 2025-11-22

### Added

- The import/export analyzer now covers Python: `import`/`from`/`__all__`, detects dynamic `importlib.import_module` and
  `__import__`, and reports re-exports via `from x import *`.
- Default analyzer extensions now include `py`.

### Changed

- README and Monika's guide updated with Python support.

## [0.2.4] - 2025-11-22

### Added

- Optional `--serve` mini HTTP server: HTML reports contain clickable `file:line` links that open in an editor/OS (
  `code -g` by default, configurable with `--editor-cmd`). Safe: paths are canonicalized and restricted to provided
  roots.
- Reports and JSON now include locations of Tauri command calls/handlers, which speeds up FE↔BE diagnosis.

### Changed

- `--serve`/`--editor-cmd` described in help/README; auto-opening the report in the browser remains.

## [0.2.3] - 2025-11-22

### Added

- The analyzer reports Tauri command coverage: FE calls (`safeInvoke`/`invokeSnake`) vs. handlers with
  `#[tauri::command]` in Rust; also shows missing and unused handlers in HTML/JSON/CLI reports.

### Changed

- Hardening auto-open HTML (path canonicalization, no control character checks).
- Unified dependencies: `regex = 1.12` in manifest.
- Hidden files recognized solely by a leading dot (no special-case `.DS_Store`).

## [0.2.2] - 2025-11-22

### Added

- The analyzer now understands CSS `@import` and Rust `use`/`pub use`/public items; default analyzer extensions expanded
  to include `rs` and `css`.
- HTML report auto-open remains; help/README updated to note new language coverage.

### Changed

- Hidden-file detection no longer special-cases `.DS_Store`; relies on leading dot + `--show-hidden`.

## [0.2.0] - 2025-11-21

### Added

- Unified CLI features and JSON output across all runtimes (Node.js, Python, Rust): extension filters, ignore patterns,
  gitignore support, max depth, color modes, JSON output, and summary reporting (commit [
  `8962e39`](https://github.com/Loctree/Loctree/commit/8962e39)).
- Installation scripts for fast setup: `install.sh`, `install_node.sh`, and `install_py.sh` (commit [
  `b6824f4`](https://github.com/Loctree/Loctree/commit/b6824f4)).
- `--show-hidden` (`-H`) option to include dotfiles and other hidden entries in output in Rust and Python CLIs (commit [
  `12310b4`](https://github.com/Loctree/Loctree/commit/12310b4)).

### Changed

- Standardized the project name from `loc-tree` to `loctree` across runtimes, binaries, installers, and documentation;
  improved CLI UX and argument parsing, and enhanced error messages (commit [
  `e31d3a4`](https://github.com/Loctree/Loctree/commit/e31d3a4)).
- Usage/help output refined and examples clarified across Rust, Node, and Python CLIs (commit [
  `b6824f4`](https://github.com/Loctree/Loctree/commit/b6824f4) and [
  `8962e39`](https://github.com/Loctree/Loctree/commit/8962e39)).

### Documentation

- Expanded and clarified README with installation instructions, usage details, examples, and project structure
  overview (commits [`e31d3a4`](https://github.com/Loctree/Loctree/commit/e31d3a4), [
  `b6824f4`](https://github.com/Loctree/Loctree/commit/b6824f4), [
  `8962e39`](https://github.com/Loctree/Loctree/commit/8962e39)).

### Other

- Initial project setup (commit [`2031f80`](https://github.com/Loctree/Loctree/commit/2031f80)).

---

Release notes are generated from the last 5 commits on the default branch (`main`).
3a4`](https://github.com/Loctree/Loctree/commit/e31d3a4), [
  `b6824f4`](https://github.com/Loctree/Loctree/commit/b6824f4), [
  `8962e39`](https://github.com/Loctree/Loctree/commit/8962e39)).

### Other

- Initial project setup (commit [`2031f80`](https://github.com/Loctree/Loctree/commit/2031f80)).

---

Release notes are generated from the last 5 commits on the default branch (`main`).
