# loctree CLI Command Reference

Complete reference for all loctree commands. For global options and environment variables, see [options.md](./options.md).

## Philosophy

**Scan once, query everything.** Run `loct` to create artifacts in the user cache dir, then use subcommands to query them.

---

## Table of Contents

- [Quick Start](#quick-start)
- [Instant Commands (<100ms)](#instant-commands-100ms)
- [Analysis Commands](#analysis-commands)
- [Framework-Specific Commands](#framework-specific-commands)
- [Management Commands](#management-commands)
- [Core Workflow Commands](#core-workflow-commands)
- [JQ Query Mode](#jq-query-mode)
- [Aliases](#aliases)
- [Artifacts](#artifacts)

---

## Quick Start

| Command | Description | Speed |
|---------|-------------|-------|
| `loct` | Auto-scan and analyze current directory | ~2s |
| `loct health` | Quick health check (cycles + dead + twins) | <100ms |
| `loct slice <file>` | Extract file + dependencies for AI context | <100ms |
| `loct dead` | Find unused exports / dead code | <100ms |

---

## Instant Commands (<100ms)

These commands query pre-built artifacts and return results in milliseconds.

### `focus` - Directory Context

Extract holographic context for a directory (like `slice` but for directories).

```bash
loct focus <DIRECTORY> [OPTIONS]
```

**Options:**
- `--consumers`, `-c` - Include files that import from this directory
- `--depth <N>` - Maximum depth for external dependency traversal
- `--root <PATH>` - Project root (default: current directory)

**Examples:**
```bash
loct focus src/features/patients/           # Focus on patients feature
loct focus src/components/ --consumers      # Include external consumers
loct focus lib/utils/ --depth 1             # Limit external dep depth
loct focus src/api/ --json                  # JSON output for AI tools
```

**Output:**
- Core files (all files in the directory)
- Internal edges (imports within the directory)
- External deps (files outside the directory that are imported)
- Consumers (files outside that import from the directory)

---

### `hotspots` - Import Frequency Heatmap

Show import frequency heatmap - which files are core vs peripheral.

```bash
loct hotspots [OPTIONS]
```

**Options:**
- `--min <N>` - Minimum import count to show (default: 1)
- `--limit <N>` - Maximum files to show (default: 50)
- `--leaves` - Show only leaf nodes (0 importers)
- `--coupling` - Include out-degree (files that import many others)
- `--root <PATH>` - Project root

**Examples:**
```bash
loct hotspots                    # Show top 50 most-imported files
loct hotspots --limit 20         # Top 20 only
loct hotspots --leaves           # Find leaf nodes (entry points / dead)
loct hotspots --coupling         # Show both in-degree and out-degree
loct hotspots --min 5            # Only files with 5+ importers
```

**Output Categories:**
- **CORE** (10+ importers) - Critical infrastructure
- **SHARED** (3-9 importers) - Shared utilities
- **PERIPHERAL** (1-2 importers) - Feature-specific
- **LEAF** (0 importers) - Entry points or potentially dead code

---

### `commands` - Tauri FE↔BE Handler Bridges

Show Tauri command bridges between frontend and backend.

```bash
loct commands [OPTIONS] [PATHS...]
```

**Options:**
- `--name <PATTERN>` - Filter to commands matching pattern
- `--missing-only` - Show only missing handlers
- `--unused-only` - Show only unused handlers
- `--limit <N>` - Maximum results to show

**Examples:**
```bash
loct commands                    # Full coverage report
loct commands --missing-only     # Only missing handlers
loct commands --json --missing   # JSON for CI
```

**Detects:**
- Missing handlers: FE calls `invoke('cmd')` but no BE `#[tauri::command]`
- Unused handlers: BE has `#[tauri::command]` but FE never calls it
- Matched handlers: Both FE and BE exist (healthy)

---

### `events` - Event Flow Analysis

Show event emit/listen flow and issues.

```bash
loct events [OPTIONS] [PATHS...]
```

**Options:**
- `--ghost` - Show ghost events (emitted but not handled)
- `--orphan` - Show orphan handlers (handlers without emitters)
- `--races` - Show potential race conditions
- `--fe-sync` - Show only FE<->FE sync events (window sync pattern)

**Examples:**
```bash
loct events                     # Full event analysis
loct events --ghost             # Only ghost events
loct events --races             # Race condition detection
```

---

### `coverage` - Test Coverage Gaps

Analyze test coverage gaps (structural coverage, not line coverage).

```bash
loct coverage [OPTIONS] [PATHS...]
```

**Options:**
- `--handlers-only` - Only show handler gaps (skip events/exports)
- `--events-only` - Only show event gaps (skip handlers/exports)
- `--min-severity <LVL>` - Filter by minimum severity: critical, high, medium, low

**Examples:**
```bash
loct coverage                          # Show all coverage gaps
loct coverage --handlers-only          # Focus on untested handlers
loct coverage --min-severity high      # Only critical/high issues
```

**Severity Levels:**
- **CRITICAL:** Handlers without any test (used in production!)
- **HIGH:** Events emitted but never tested
- **MEDIUM:** Exports without test imports
- **LOW:** Tests that import unused code

---

### `health` - Quick Health Check

One-shot summary of all structural issues.

```bash
loct health [OPTIONS] [PATHS...]
```

**Options:**
- `--include-tests` - Include test files in analysis (default: false)

**Examples:**
```bash
loct health                    # Quick health summary
loct health --include-tests    # Include test files
loct health src/               # Analyze specific directory
loct health --json             # Machine-readable output
```

**Output:**
- Cycles: Circular import count (hard vs structural)
- Dead: Unused exports (high confidence count)
- Twins: Duplicate symbol names across files

---

### `slice` - File Context Extraction

Extract holographic context for a file (file + all its dependencies).

```bash
loct slice <TARGET_PATH> [OPTIONS]
```

**Options:**
- `--consumers`, `-c` - Include reverse dependencies (files that import this)
- `--depth <N>` - Maximum dependency depth to traverse
- `--root <PATH>` - Project root for resolving imports
- `--rescan` - Force snapshot update (includes new/uncommitted files)

**Examples:**
```bash
loct slice src/main.rs              # File + its dependencies
loct slice src/utils.ts --consumers # Include reverse deps
loct slice lib/api.ts --depth 2     # Limit to 2 levels
loct slice src/app.tsx --json       # JSON output for AI tools
loct slice src/new-file.ts --rescan # Slice a newly created file
```

**Note:** Shows what the file USES, not what USES it. For reverse dependencies, use `--consumers` or `loct query who-imports`.

---

### `impact` - Change Impact Analysis

Analyze impact of modifying or removing a file.

```bash
loct impact <FILE> [OPTIONS]
```

**Options:**
- `--depth <N>` - Limit traversal depth (default: unlimited)
- `--root <PATH>` - Project root

**Examples:**
```bash
loct impact src/utils.ts                # Full impact analysis
loct impact src/api.ts --depth 2        # Limit to 2 levels deep
loct impact lib/helpers.ts --json       # JSON output
```

**Difference from `query who-imports`:**
- `who-imports`: Finds direct importers only
- `impact`: Finds ALL affected files (direct + transitive)

---

### `query` - Graph Queries

Query the import graph and symbol index for specific relationships.

```bash
loct query <KIND> <TARGET>
```

**Query Kinds:**

| Kind | Description | Example |
|------|-------------|---------|
| `who-imports <FILE>` | Find all files that import the file | `loct query who-imports src/utils.ts` |
| `component-of <FILE>` | Show which component/module contains the file | `loct query component-of src/ui/Button.tsx` |

*(Note: `query where-symbol` is legacy. Use `loct find --where-symbol <SYMBOL>` instead. `find --where-symbol` leverages the full `symbol_graph` and supports C-family languages like Swift and Objective-C.)*

**Examples:**
```bash
loct query who-imports src/utils.ts       # What imports utils.ts?
loct find --where-symbol PatientRecord    # Where is it defined? (Modern syntax)
loct query component-of src/ui/Button.tsx # What owns Button?
```

---

### `follow` - Follow Dispatch Events

Trace late-binding event dispatch graphs and event lifecycles.

```bash
loct follow events [OPTIONS]
```

**Examples:**
```bash
loct follow events      # Traces event networks across the codebase
```
*(Supports `NotificationCenter` posts/observers and `@selector`/`IBAction` bridges for C-family Apple-stack languages.)*

---

## Analysis Commands

Commands that perform deeper analysis on the codebase.

### `dead` - Unused Export Detection

Detect unused exports / dead code.

```bash
loct dead [OPTIONS] [PATHS...]
```

**Options:**
- `--confidence <lvl>` - `normal` or `high` (default: normal)
- `--top <N>` - Limit results to top N (default: 20)
- `--full`, `--all` - Show all results (ignore --top limit)
- `--path <PATTERN>` - Filter by file path pattern (regex)
- `--with-tests` - Include test files
- `--with-helpers` - Include helper files
- `--with-shadows` - Detect shadow exports (same symbol, multiple files)
- `--with-ambient` - Include ambient declarations (declare global/module/namespace)
- `--with-dynamic` - Include dynamically generated symbols (exec/eval/compile)

**Examples:**
```bash
loct dead                        # Standard dead code analysis
loct dead --confidence high      # Only high-confidence findings
loct dead --with-tests           # Include test files
loct dead --with-shadows         # Detect shadow exports
loct dead --full                 # Show all results
```

---

### `cycles` - Circular Import Detection

Detect circular import chains.

```bash
loct cycles [OPTIONS] [PATHS...]
```

**Options:**
- `--path <PATTERN>` - Filter to files matching pattern
- `--breaking-only` - Only show cycles that would break compilation
- `--explain` - Show detailed explanation for each cycle
- `--legacy-format` - Use legacy output format

**Examples:**
```bash
loct cycles                # Detect all cycles
loct cycles src/           # Only analyze src/
loct cycles --json         # JSON output for CI
loct cycles --explain      # Detailed explanations
```

**Why Cycles Matter:**
- Runtime initialization errors
- Build/bundling failures
- Flaky test behavior

---

### `twins` - Duplicate Symbol Detection

Find dead parrots (0 imports) and duplicate exports.

```bash
loct twins [OPTIONS] [PATH]
```

**Options:**
- `--path <DIR>` - Root directory to analyze
- `--dead-only` - Show only dead parrots (0 imports)
- `--include-tests` - Include test files (excluded by default)
- `--include-suppressed` - Show suppressed findings too
- `--ignore-conventions` - Ignore framework conventions when detecting twins

**Examples:**
```bash
loct twins                    # Full analysis
loct twins --dead-only        # Only exports with 0 imports
loct twins src/               # Analyze specific directory
loct twins --include-tests    # Include test files
```

**Detects:**
- **Dead Parrots:** Exports with 0 imports anywhere in the codebase
- **Exact Twins:** Same symbol name exported from multiple files
- **Barrel Chaos:** Re-export anti-patterns (missing index.ts, deep re-export chains)

---

### `zombie` - Comprehensive Dead Code Analysis

Find all zombie code (dead exports + orphan files + shadows).

```bash
loct zombie [OPTIONS] [PATHS...]
```

**Options:**
- `--include-tests` - Include test files in zombie detection (default: false)

**Examples:**
```bash
loct zombie                    # Find all zombie code
loct zombie --include-tests    # Include test files
loct zombie src/               # Analyze specific directory
```

**Combines:**
- **Dead Exports:** Symbols with 0 imports
- **Orphan Files:** Files with 0 importers (not entry points)
- **Shadow Exports:** Same symbol exported by multiple files where some have 0 imports

---

### `audit` - Full Codebase Audit

Comprehensive analysis combining all structural checks.

```bash
loct audit [OPTIONS] [PATHS...]
```

**Options:**
- `--include-tests` - Include test files in analysis (default: false)
- `--todos` - Save an actionable todo checklist instead of the full audit report
- `--limit <N>` - Intentionally truncate each section to `N` items
- `--no-open` - Save the report without opening it automatically

**Examples:**
```bash
loct audit                     # Full audit of current directory
loct audit --include-tests     # Include test files
loct audit src/                # Audit specific directory
loct audit --todos             # Actionable checklist format
loct audit --limit 25          # Intentionally trim each section
```

**Output:**
- Markdown audit artifacts are always written to the loct cache for the project.
- Terminal output prints a short summary and the saved report path.

**Includes:**
- Cycles (circular imports)
- Dead exports (unused code)
- Twins (duplicate symbols)
- Orphan files (0 importers)
- Shadow exports (consolidation candidates)
- Crowds (similar dependency patterns)

---

### `crowd` - Functional Clustering

Detect functional crowds (similar files clustering).

```bash
loct crowd [PATTERN] [OPTIONS]
```

**Options:**
- `--pattern <PATTERN>` - Pattern to detect crowd around
- `--auto-detect` - Detect all crowds automatically
- `--min-size <N>` - Minimum crowd size to report (default: 2)
- `--limit <N>` - Maximum crowds to show in auto-detect mode (default: 10)
- `--include-tests` - Include test files in crowd detection (default: false)

**Examples:**
```bash
loct crowd cache              # Find cache-related clusters
loct crowd session            # Find session-related clusters
loct crowd --auto-detect      # Auto-detect all crowds
```

---

### `tagmap` - Unified Keyword Search

Unified search around a keyword - aggregates files, crowds, and dead code.

```bash
loct tagmap <KEYWORD> [OPTIONS]
```

**Options:**
- `--include-tests` - Include test files in analysis
- `--limit <N>` - Maximum results per section (default: 20)

**Examples:**
```bash
loct tagmap patient           # Everything about 'patient' feature
loct tagmap auth              # Auth-related files, crowds, dead code
loct tagmap message --json    # JSON output for AI processing
loct tagmap api --limit 10    # Limit results
```

**Aggregates:**
1. **FILES:** All files with keyword in path or name
2. **CROWD:** Functional clustering around the keyword
3. **DEAD:** Dead exports related to the keyword

---

### `sniff` - Code Smell Aggregate

Sniff for code smells (twins + dead parrots + crowds).

```bash
loct sniff [OPTIONS]
```

**Options:**
- `--path <DIR>` - Root directory to analyze
- `--dead-only` - Show only dead parrots
- `--twins-only` - Show only twins
- `--crowds-only` - Show only crowds
- `--include-tests` - Include test files in analysis (default: false)
- `--min-crowd-size <N>` - Minimum crowd size to report (default: 2)

**Examples:**
```bash
loct sniff                    # Full code smell analysis
loct sniff --dead-only        # Only dead parrots
loct sniff --twins-only       # Only duplicate names
loct sniff --crowds-only      # Only similar file clusters
```

**Aggregates:**
- **TWINS:** Same symbol exported from multiple files
- **DEAD PARROTS:** Exports with 0 imports
- **CROWDS:** Files clustering around similar functionality

---

### `doctor` - Interactive Diagnostics

Interactive diagnostics with actionable recommendations.

```bash
loct doctor [OPTIONS] [PATHS...]
```

**Options:**
- `--include-tests` - Include test files in analysis (default: false)
- `--apply-suppressions` - Auto-add suggested patterns to .loctignore

**Examples:**
```bash
loct doctor                        # Interactive diagnostics
loct doctor --apply-suppressions   # Auto-add .loctignore patterns
loct doctor src/                   # Analyze specific directory
```

**Categorizes Findings:**
1. **Auto-fixable:** Issues with clear automated solutions
2. **Needs Review:** Potential issues requiring human judgment
3. **Suggested Suppressions:** Patterns to add to .loctignore

---

## Framework-Specific Commands

Commands designed for specific frameworks and tools.

### `trace` - Tauri Handler Tracing

Trace a Tauri/IPC handler end-to-end.

```bash
loct trace <HANDLER> [ROOTS...]
```

**Examples:**
```bash
loct trace toggle_assistant
loct trace standard_command apps/desktop
```

**Investigates:**
- Backend definition (file, line, exposed name)
- Frontend `invoke()` calls and plain mentions
- Registration status in `generate_handler![]`
- Verdict + suggestion to fix

---

### `routes` - Backend Route Listing

List backend/web routes (FastAPI/Flask).

```bash
loct routes [OPTIONS] [PATHS...]
```

**Options:**
- `--framework <NAME>` - Filter by framework label (fastapi, flask)
- `--path <PATTERN>` - Filter by route path substring

**Examples:**
```bash
loct routes                       # List all routes
loct routes --framework fastapi   # Only FastAPI routes
loct routes --path /patients      # Filter by path
```

**Detects:**
- **FastAPI:** `@app.get/post/put/delete/patch`, `@router.*`, `@api_router.*`
- **Flask:** `@app.route`, `@blueprint.route`, `.route(...)`

---

### `dist` - Bundle Analysis

Verify tree-shaking by comparing your source exports against one or more
production source maps.

```bash
loct dist --src <DIR> --source-map <PATH> [--source-map <PATH> ...] [--report <PATH>]
```

**Options:**
- `--src <DIR>` - Source directory to scan once (e.g., `src/`)
- `--source-map <PATH>` - Production source map to compare against; repeat for multi-entry builds
- `--report <PATH>` - Write the JSON result to a file
- `--json` - Print the JSON result to stdout

**Examples:**
```bash
loct dist --src src/ --source-map dist/main.js.map
loct dist --src src/ --source-map dist/app.js.map --source-map dist/admin.js.map
loct dist --src src/ --source-map dist/main.js.map --report .loctree/dist-report.json
```

**Mental model:**
- Point `loct dist` at the source directory you own
- Point it at the production source maps your build emitted
- `loct` reports exports that never make it into any bundle

If a source map contains symbol names, `loct dist` checks exports at symbol
level. If not, it falls back to line-level coverage for that map.

**Output:**
- Default stdout: human summary
- `--json`: machine-readable stdout
- `--report`: saved JSON artifact for CI or review

---

### `layoutmap` - CSS Layout Analysis

Analyze CSS layout properties (z-index, position, display).

```bash
loct layoutmap [OPTIONS]
```

**Options:**
- `--zindex-only` - Show only z-index values
- `--sticky-only` - Show only sticky/fixed position elements
- `--grid-only` - Show only grid/flex layouts
- `--min-zindex <N>` - Filter z-index values >= N
- `--exclude <PATTERN>` - Exclude paths matching glob (can be repeated)
- `--root <PATH>` - Project root

**Examples:**
```bash
loct layoutmap                  # Full CSS layout analysis
loct layoutmap --zindex-only    # Only z-index hierarchy
loct layoutmap --sticky-only    # Only sticky/fixed elements
loct layoutmap --min-zindex 100 # High z-index values (likely overlays)
loct layoutmap --exclude .obsidian --exclude prototype  # Skip dirs
```

**Extracts:**
- **Z-INDEX:** All z-index values sorted by value (identifies layering conflicts)
- **POSITION:** Sticky/fixed positioned elements
- **DISPLAY:** Grid/flex layouts and their locations

---

## Management Commands

Commands for managing loctree analysis and configuration.

### `doctor` - Interactive Diagnostics

See [Analysis Commands](#doctor---interactive-diagnostics) section.

---

### `suppress` - False Positive Management

Manage false positive suppressions.

```bash
loct suppress <TYPE> <SYMBOL> [OPTIONS]
loct suppress --list
loct suppress --clear
```

**Types:**
- `twins` - Exact twin (same symbol in multiple files)
- `dead_parrot` - Dead parrot (export with 0 imports)
- `dead_export` - Dead export (unused export)
- `circular` - Circular import

**Options:**
- `--file <PATH>` - Only suppress in specific file (default: all files)
- `--reason <TEXT>` - Document why this is a false positive
- `--list` - Show all current suppressions
- `--clear` - Remove all suppressions
- `--remove` - Remove a specific suppression

**Examples:**
```bash
loct suppress twins Message              # Suppress 'Message' twin everywhere
loct suppress twins Message --file src/types.ts  # Only in specific file
loct suppress dead_parrot unusedHelper --reason 'Used via dynamic import'
loct suppress --list                     # View all suppressions
loct suppress --clear                    # Reset suppressions
```

**Storage:**
- Suppressions are stored in `.loctree/suppressions.toml`
- Commit this file to share suppressions with your team

---

### `diff` - Snapshot Comparison

Compare snapshots between branches/commits.

```bash
loct diff --since <SNAPSHOT> [--to <SNAPSHOT>] [OPTIONS]
```

**Options:**
- `--since <SNAPSHOT>` - Base snapshot to compare from (required)
- `--to <SNAPSHOT>` - Target snapshot (default: current working tree)
- `--auto-scan-base` - Export the `--since` ref tree and scan it (no git worktree)
- `--problems-only` - Show only regressions (new dead code, new cycles)
- `--jsonl` - Output as JSONL (one line per change)

**Examples:**
```bash
loct diff --since main                    # Compare main to working tree
loct diff --since HEAD~1                  # Compare to previous commit
loct diff --since main --auto-scan-base   # Auto-scan main branch
loct diff --since v1.0.0 --to v2.0.0      # Compare two tags
```

**Shows:**
- New/removed files and symbols
- Import graph changes
- New dead code introduced (regressions)
- New circular dependencies

---

### `memex` - AI Memory Indexing

Index analysis into AI memory (vector database).

```bash
loct memex [OPTIONS]
```

**Options:**
- `--report-path <PATH>` - Path to .loctree directory or analysis.json file
- `--project-id <ID>` - Unique project identifier (e.g., "github.com/org/repo")
- `--namespace <NS>` - Namespace for the memory index (default: "loctree")
- `--db-path <PATH>` - Path to LanceDB storage directory

**Examples:**
```bash
loct memex                                    # Index current project
loct memex --project-id github.com/org/repo   # Set project ID
loct memex --namespace myproject              # Custom namespace
```

---

## Core Workflow Commands

Commands for basic analysis and scanning.

### `auto` - Full Auto-Scan (Default)

Full auto-scan with stack detection (default command).

```bash
loct auto [OPTIONS] [PATHS...]
loct [OPTIONS] [PATHS...]    # 'auto' is the default command
```

**Options:**
- `--full-scan` - Force full rescan (ignore cache)
- `--scan-all` - Scan all files including hidden/ignored
- `--for-agent-feed` - Output optimized format for AI agents (JSONL stream)
- `--agent-json` - Emit a single agent bundle JSON

**Examples:**
```bash
loct                         # Auto-scan current directory
loct auto                    # Explicit auto command
loct auto --full-scan        # Force full rescan
loct auto src/ lib/          # Scan specific directories
loct --for-agent-feed        # AI-optimized output (JSONL stream)
loct --agent-json            # One-shot agent bundle JSON
```

**Performs:**
- Detects project type and language stack automatically
- Builds dependency graph and import relationships
- Analyzes code structure and exports
- Identifies potential issues (dead code, cycles, etc.)

---

### `scan` - Build Snapshot

Build/update snapshot for current HEAD.

```bash
loct scan [OPTIONS] [PATHS...]
```

**Options:**
- `--full-scan` - Force full rescan, ignore cached data
- `--scan-all` - Include hidden and ignored files
- `--watch` - Watch for changes and re-scan automatically

**Examples:**
```bash
loct scan                    # Scan current directory
loct scan --full-scan        # Force complete rescan
loct scan src/ lib/          # Scan specific directories
loct scan --scan-all         # Include all files (even hidden)
loct scan --watch            # Watch mode with live refresh
```

**Note:** Unlike `auto`, it only builds the snapshot without extra analysis.

---

### `tree` - Directory Tree with LOC

Display LOC tree / structural overview.

```bash
loct tree [OPTIONS] [PATHS...]
```

**Options:**
- `--depth <N>`, `-L <N>` - Maximum depth (default: unlimited)
- `--summary [N]` - Show top N largest items (default: 5)
- `--top [N]` - Only show top N largest items (default: 50)
- `--loc <N>` - Only show items with LOC >= N
- `--min-loc <N>` - Alias for --loc
- `--show-hidden`, `-H` - Include hidden files/directories
- `--find-artifacts` - Highlight build/generated artifacts
- `--show-ignored` - Show gitignored files

**Examples:**
```bash
loct tree                       # Full tree
loct tree --depth 3             # Limit depth
loct tree --summary 10          # Top 10 largest
loct tree --loc 100             # LOC threshold
loct tree src/ --show-hidden    # Include dotfiles
```

---

### `find` - Literal Truth and Explicit Discovery

Plain `loct find QUERY` returns exact identifier-boundary occurrences. Broad
symbol/parameter/fuzzy discovery is available only through `--discover`.

```bash
loct find [QUERY] [OPTIONS]
```

**Options:**
- `--literal` - Explicit alias for the default literal mode
- `--discover` - Opt into broad AST/parameter/fuzzy discovery
- `--symbol <PATTERN>` - Search for symbols matching regex
- `--file <PATTERN>` - Search for files matching regex
- `--similar <SYMBOL>` - Find symbols with similar names (fuzzy)
- `--dead` - Only show dead/unused symbols
- `--exported` - Only show exported symbols
- `--lang <LANG>` - Filter by language (ts, rs, js, py, etc.)
- `--limit <N>` - Maximum results to show

**Examples:**
```bash
loct find Patient              # Every exact identifier occurrence
loct find --discover Patient   # Broad symbol/parameter/fuzzy discovery
loct find --symbol ".*Config$" # Regex: symbols ending with Config
loct find --file "utils"       # Files containing "utils" in path
loct find --dead --exported    # Dead exported symbols
```

**Note:** NOT impact analysis - for dependency impact, use `loct impact`. NOT dead code detection - use `loct dead` or `loct twins`.

---

### `lint` - Structural Linting

Structural lint and policy checks.

```bash
loct lint [OPTIONS] [PATHS...]
```

**Options:**
- `--entrypoints` - Check entrypoint coverage
- `--sarif` - Output in SARIF format for CI integration
- `--tauri` - Enable Tauri-specific checks
- `--fail` - Exit non-zero on issues (CI mode)

**Examples:**
```bash
loct lint                   # Standard linting
loct lint --fail --sarif    # CI mode with SARIF output
loct lint --tauri           # Tauri-specific checks
```

---

### `report` - Generate HTML Report + Cached Artifacts

Generate the full HTML report and refresh cached machine-readable artifacts.

```bash
loct report [OPTIONS] [PATHS...]
```

**Options:**
- `--output <FILE>` - Output HTML file path
- `--serve` - Start a local server to view the report
- `--port <N>` - Server port (default: varies)
- `--editor <NAME>` - Editor integration (code, cursor, windsurf, jetbrains)

**Examples:**
```bash
loct report                              # Refresh report.html + cached artifacts
loct report --output report.html         # Generate HTML report
loct report --serve --port 4173          # Serve report locally
loct report --editor code                # VSCode integration
```

**Output:**
- Writes the full HTML report.
- Refreshes cached artifacts such as `findings.json`, `agent.json`, `analysis.json`, and `report.sarif`.

---

### `findings` - Canonical Findings JSON

Emit the canonical findings payload directly to stdout.

```bash
loct findings [OPTIONS] [PATHS...]
```

**Options:**
- `--summary` - Emit only the compact summary payload (health score + counts)

**Examples:**
```bash
loct findings                              # Full findings payload
loct findings --summary                    # Compact summary JSON
loct findings src/ | jq '.dead_parrots'    # Scope to a directory
```

**Output:**
- Full mode prints the same machine-readable payload stored in `findings.json`.
- `--summary` prints the compact JSON summary used for CI/status checks.

---

### `info` - Snapshot Metadata

Show snapshot metadata and project info.

```bash
loct info [OPTIONS]
```

**Options:**
- `--root <PATH>` - Root directory to check

**Examples:**
```bash
loct info                   # Show snapshot metadata
loct info --root src/       # Check specific directory
```

**Displays:**
- Snapshot metadata
- Detected stack
- Analysis summary

---

### `help` - Command Help

Show help for commands.

```bash
loct help [COMMAND]
loct --help
loct --help-full
loct --help-legacy
```

**Examples:**
```bash
loct help                   # Main help
loct help slice             # Help for slice command
loct --help-full            # All 28 commands
loct --help-legacy          # Legacy flag migration guide
```

---

### `version` - Version Info

Show version information.

```bash
loct version
loct --version
```

---

## JQ Query Mode

Query snapshot with jq-style filters. Requires jaq dependencies (enabled by default in CLI build).

```bash
loct '<FILTER>' [OPTIONS]
```

**Options:**
- `-r`, `--raw-output` - Output raw strings, not JSON
- `-c`, `--compact-output` - Compact JSON output (no pretty-printing)
- `-e`, `--exit-status` - Set exit code based on output (0 if truthy)
- `--arg <name> <value>` - Pass string variable to filter
- `--argjson <name> <json>` - Pass JSON variable to filter
- `--snapshot <path>` - Use specific snapshot file instead of latest

**Filter Syntax:**
- `.metadata` - Extract metadata field
- `.files[]` - Iterate over files array
- `.files[0]` - Get first file
- `.[\"key\"]` - Access key with special characters

**Examples:**
```bash
loct '.metadata'                         # Extract metadata
loct '.files | length'                   # Count files
loct '.files[] | .path'                  # List file paths
loct '.metadata.total_loc' -r            # Raw number output
loct '.files[] | select(.lang == "ts")' -c
loct '.files[] | select(.loc > 500)' -c
loct '.dead_parrots[]' --artifact findings                   # List dead exports
loct '.cycles[]' --artifact findings                         # List circular imports
```

---

## Aliases

Quick shortcuts for common commands:

| Alias | Equivalent | Description |
|-------|------------|-------------|
| `loct s <file>` | `loct slice <file>` | Extract file context |
| `loct f <pattern>` | `loct find <pattern>` | Search symbols/files |
| `loct h` | `loct health` | Health summary |

---

## Artifacts

loctree creates these artifacts in the user cache dir by default (override with `LOCT_CACHE_DIR`):

| Artifact | Description | Use Case |
|----------|-------------|----------|
| `snapshot.json` | Full dependency graph | jq-queryable, complete project state |
| `findings.json` | All issues (dead, cycles, twins...) | CI integration, automated checks |
| `agent.json` | AI-optimized bundle with health_score | AI agent consumption |
| `manifest.json` | Index for tooling | Tooling integration |

**Query artifacts:**
```bash
loct findings                           # View canonical findings payload
loct '.metadata'                        # Extract metadata
loct '.files | length'                  # Count files using jq mode
```

---

## Examples

### Quick Analysis
```bash
loct                       # Scan repo, create artifacts
loct health                # Quick health check
loct hotspots              # Find hub files (47ms!)
```

### Deep Analysis
```bash
loct focus src/features/   # Directory context (67ms!)
loct coverage              # Test gaps (49ms!)
loct audit                 # Full audit
```

### AI Integration
```bash
loct slice src/main.rs --json | claude
loct --for-ai > context.json
```

### CI Integration
```bash
loct lint --fail --sarif > loctree.sarif
loct health --json | jq '.summary.health_score'
```

---

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
