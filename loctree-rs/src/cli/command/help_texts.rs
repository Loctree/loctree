//! Static help text constants for CLI commands.
//!
//! Each constant provides detailed usage documentation for a specific command.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

pub(super) const AUTO_HELP: &str =
    "loct auto - Full auto-scan with stack detection (default command)

USAGE:
    loct auto [OPTIONS] [PATHS...]
    loct [OPTIONS] [PATHS...]    # 'auto' is the default command

DESCRIPTION:
    Performs a comprehensive analysis of your codebase:
    - Detects project type and language stack automatically
    - Builds dependency graph and import relationships
    - Analyzes code structure and exports
    - Identifies potential issues (dead code, cycles, etc.)

OPTIONS:
    --full-scan          Force full rescan (ignore cache)
    --scan-all           Scan all files including hidden/ignored
    --no-duplicates      Hide duplicate export sections in CLI output
    --no-dynamic-imports Hide dynamic import sections in CLI output
    --help, -h           Show this help message

ARGUMENTS:
    [PATHS...]           Root directories to scan (default: current directory)

EXAMPLES:
    loct                         # Auto-scan current directory
    loct auto                    # Explicit auto command
    loct auto --full-scan        # Force full rescan
    loct auto src/ lib/          # Scan specific directories
    loct context                 # Agent-ready Markdown ContextPack (preferred)
    loct context --json          # Full ContextPack JSON for tooling

See `loct --help-legacy` for deprecated flag migration.";

pub(super) const AGENT_HELP: &str =
    "loct agent - Agent bundle JSON (shortcut for auto --agent-json)

USAGE:
    loct agent [PATHS...]

DESCRIPTION:
    Runs the auto scan and emits a single JSON tuned for AI agents:
    full handlers, duplicates, dead exports, dynamic imports, cycles, and lint findings,
    plus prioritized quick wins and top files for context anchoring.
    The bundle is also saved to the artifacts dir (cache by default; set LOCT_CACHE_DIR to override).

OPTIONS:
    --full-scan          Force full rescan (ignore cache)
    --scan-all           Scan all files including hidden/ignored
    --help, -h           Show this help message

ARGUMENTS:
    [PATHS...]           Root directories to scan (default: current directory)

EXAMPLES:
    loct agent                   # Agent bundle for current directory
    loct agent src/              # Agent bundle for src/";

pub(super) const SCAN_HELP: &str = "loct scan - Build/update snapshot for current HEAD

USAGE:
    loct scan [OPTIONS] [PATHS...]

DESCRIPTION:
    Scans the codebase and updates the internal snapshot database.
    Builds the dependency graph and prepares data for other commands.
    Unlike 'auto', it only builds the snapshot without extra analysis.

    With `--watch`, runs a long-lived loop and is gated by a per-repo
    single-instance lock (`.loctree/scan.lock`). A second `--watch` for
    the same repo refuses to start and exits with code 75
    (EX_TEMPFAIL) — pass `--replace` to recycle the holder or
    `--wait[=SECONDS]` to block until it exits. `loct watch` is the
    preferred surface for the same loop (foreground / background / LSP
    co-process).

OPTIONS:
    --full-scan       Force full rescan, ignore cached data
    --scan-all        Include hidden and ignored files
    --watch           Watch for changes and re-scan automatically
    --replace         (with --watch) SIGTERM the current --watch holder
                      and take over the lock
    --wait[=SECONDS]  (with --watch) Block until the lock is free;
                      no value = wait indefinitely
    --help, -h        Show this help message

ARGUMENTS:
    [PATHS...]        Root directories to scan (default: current directory)

EXIT CODES:
    0   Watcher exited cleanly.
    1   Watcher failed (IO / parse / runtime error).
    75  Lock held by another `--watch` process (EX_TEMPFAIL). Use
        --replace or --wait to recover.

EXAMPLES:
    loct scan                            # Scan current directory
    loct scan --full-scan                # Force complete rescan
    loct scan src/ lib/                  # Scan specific directories
    loct scan --scan-all                 # Include all files (even hidden)
    loct scan --watch                    # Watch mode with live refresh
    loct scan --watch --replace          # Take over from an existing watcher
    loct scan --watch --wait=30          # Block up to 30s, then fail";

pub(super) const WATCH_HELP: &str = "loct watch - Single-instance watch loop with co-processes

USAGE:
    loct watch [MODE] [OPTIONS] [PATHS...]

DESCRIPTION:
    The new shape of `loct scan --watch`. Same locked watch loop, but
    with mode flags that bring up the surface you actually want:

      --dev       Foreground watcher (default). Equivalent to today's
                  `loct scan --watch`, but with the single-instance
                  lock enforced.
      --bg        Daemonize: re-spawn detached and exit. Logs land in
                  `.loctree/watch.log`. Returns immediately.
      --lsp       Foreground watcher + co-spawned `loctree-lsp`. The
                  LSP child shares the watch lock and shuts down when
                  the parent exits.
      --http      Foreground watcher + co-spawned `loctree-mcp` over
                  streamable-http. The MCP child binds on
                  `127.0.0.1:<--port>` (default 5174) and exposes the
                  full MCP tool surface at `/mcp` for HTTP-capable
                  agent clients. Child is killed automatically on
                  parent exit.
      --report    Foreground watcher + in-process HTTP server on
                  `127.0.0.1:<--port>` (default 5075) serving
                  `.loctree/report.html`. The report is re-rendered
                  after every snapshot save; rapid file events are
                  naturally throttled by `loct report --output ...`
                  child concurrency.

    The single-instance lock lives at `.loctree/scan.lock` keyed by the
    canonical snapshot root. `.`, `./`, `/abs/path/to/repo`, and a path
    with a trailing slash all resolve to the same lock. The lock is
    kernel-level advisory (flock on Unix, LockFileEx on Windows) and
    self-heals across SIGKILL because it lives on a file descriptor.

OPTIONS:
    --dev | --bg | --lsp | --http | --report   Pick a mode (--dev default)
    --full-scan                                Force full initial rescan
    --scan-all                                 Include hidden / ignored files
    --replace                                  SIGTERM the current holder
    --wait[=SECONDS]                           Block until lock is free
    --port <N>                                 Port for --report (default 5075)
                                               or --http (default 5174)
    --help, -h                                 Show this help

ARGUMENTS:
    [PATHS...]   Root directories to watch (default: current directory)

EXIT CODES:
    0   Watcher exited cleanly (or `--bg` parent successfully forked).
    1   Watcher failed (IO / parse / runtime error).
    75  Lock held by another --watch process (EX_TEMPFAIL).

EXAMPLES:
    loct watch                           # Foreground watch on .
    loct watch --bg                      # Background daemon; log to .loctree/watch.log
    loct watch --lsp                     # Watch + co-spawned loctree-lsp
    loct watch --http                    # Watch + streamable-http MCP on :5174
    loct watch --report --port 5080      # Watch + live HTML report on :5080
    loct watch --replace                 # Take over an existing watcher
    loct watch --wait=60 src/            # Block up to 60s waiting for lock";

pub(super) const TREE_HELP: &str = "loct tree - Display LOC tree / structural overview

USAGE:
    loct tree [OPTIONS] [PATHS...]

DESCRIPTION:
    Hierarchical tree of the codebase with LOC metrics.
    Similar to 'tree' but with LOC and gitignore handling.

OPTIONS:
    --depth <N>, -L <N>    Maximum depth (default: unlimited)
    --summary [N]          Show top N largest items (default: 5)
    --top [N]              Only show top N largest items (default: 50)
    --loc <N>              Only show items with LOC >= N
    --min-loc <N>          Alias for --loc
    --show-hidden, -H      Include hidden files/directories
    --find-artifacts       Highlight build/generated artifacts
    --show-ignored         Show gitignored files
    --files                Print matching file paths only, one per line
    --match <REGEX>        Filter output paths by regex
    --help, -h             Show this help message

ARGUMENTS:
    [PATHS...]             Roots to analyze (default: current directory)

EXAMPLES:
    loct tree                       # Full tree
    loct tree --depth 3             # Limit depth
    loct tree --summary 10          # Top 10 largest
    loct tree --loc 100             # LOC threshold
    loct tree src/ --show-hidden    # Include dotfiles
    loct tree server --files --match 'test|route' # Exact file list for report/gate work";

pub(super) const SLICE_HELP: &str = "loct slice - Extract file + dependencies and consumers for AI context

USAGE:
    loct slice <TARGET_PATH> [OPTIONS]

DESCRIPTION:
    Extracts a 'holographic slice' - the target file, its dependencies, and
    the files that import it.
    Perfect for feeding focused context to AI assistants.

    Shows both what the file USES and what USES it by default. For the old
    dependency-only view, pass --no-consumers.

OPTIONS:
    --consumers, -c    Include reverse dependencies (default; compatibility no-op)
    --no-consumers     Hide reverse dependencies (old dependency-only behavior)
    --depth <N>        Maximum dependency depth to traverse
    --root <PATH>      Project root for resolving imports
    --rescan           Force snapshot update (includes new/uncommitted files)
    --help, -h         Show this help message

EXAMPLES:
    loct slice src/main.rs              # File + deps + consumers
    loct slice src/utils.ts --no-consumers # Dependency-only view
    loct slice lib/api.ts --depth 2     # Limit to 2 levels
    loct slice src/app.tsx --json       # JSON output for AI tools
    loct slice src/new-file.ts --rescan # Slice a newly created file

RELATED COMMANDS:
    loct query who-imports <file>    Find all importers
    loct context                     Agent-ready ContextPack (preferred over legacy --for-agent-feed)
    loct focus <dir>                 Slice for a directory";

pub(super) const CONTEXT_HELP: &str = "loct context - Emit an agent-ready ContextPack

USAGE:
    loct context [OPTIONS]

OPTIONS:
    --file <PATH>      Focus the context pack on a specific file
    --scope <SELECTOR> Deterministic structural filter (repeatable; multiple = AND)
    --changed          Limit to changed files (filled by later cut)
    --task <TEXT>      Semantic task hint; ranks within --scope when scope is present
    --with-aicx        Request AICX memory overlay (default; kept for scripts)
    --no-aicx          Disable the default AICX memory overlay
    --project <PATH>   Project root for identity and snapshot scope
    --full             Output the full ContextPack (JSON by default)
    --json             Output full ContextPack JSON
    --markdown         Output Markdown (pill by default; full Markdown with --full)
    --help, -h         Show this help message

EXAMPLES:
    loct context
    loct context --file Cargo.toml
    loct context --scope \"path:loctree-rs/src/cli/\"
    loct context --scope path:core --task \"hold-mods versus hands-off\"
    loct context --scope path:src/agent/ --task \"fix SSE retry behavior\" --full --markdown
    loct context --scope \"context-pipeline\"
    loct context --scope \"context-pipeline\" --task \"cache invalidation\"
    loct context --task \"fix dead exports\"
    loct context --full
    loct context --full --markdown";

pub(super) const REPO_VIEW_HELP: &str = "loct repo-view - Repository overview for AI agents

USAGE:
    loct repo-view [PROJECT]

DESCRIPTION:
    First-class CLI counterpart of the MCP repo-view tool, with an optional
    project path. For full agent-ready context prefer `loct context`.

ARGUMENTS:
    [PROJECT]       Project root to analyze (default: current directory)

EXAMPLES:
    loct repo-view
    loct repo-view /path/to/project";

pub(super) const PRISM_HELP: &str =
    "loct prism - Compare task framings and score conceptual smear

USAGE:
    loct prism --task <TEXT> --task <TEXT> [OPTIONS]

DESCRIPTION:
    Builds ContextPacks for multiple task framings, compares the surfaces they
    retrieve, and emits the 0..15 Prism Score used by vc-polarize.

OPTIONS:
    --task <TEXT>             Task framing to compare (repeat at least twice)
    --project <PATH>          Project root for snapshot and identity
    --aicx-project <BUCKET>   Override AICX project bucket
    --aicx-bucket <BUCKET>    Alias for --aicx-project
    --with-aicx               Include AICX memory overlay (default)
    --no-aicx                 Disable AICX memory overlay
    --limit <N>               Maximum examples per section (default: 8)
    --json                    Emit JSON report
    --help, -h                Show this help message

EXAMPLES:
    loct prism --task \"auth\" --task \"auth core\" --task \"auth core portal\"
    loct prism --project . --aicx-project loctree-suite --task \"marbles\" --task \"polarize\" --json";

pub(super) const FOLLOW_HELP: &str = "loct follow - Unified signal follower

USAGE:
    loct follow [SCOPE] [OPTIONS] [PATHS...]

SCOPES:
    all, dead, cycles, twins, hotspots, trace, commands, events, pipelines

    The events scope also covers C-family runtime bridges: NotificationCenter
    post/observe pairs and @selector targets (Heuristic provenance).

OPTIONS:
    --handler <NAME>     Handler name for trace scope
    --limit <N>          Limit results where the underlying scope supports it
    --help, -h           Show this help message

EXAMPLES:
    loct follow
    loct follow dead
    loct follow cycles --limit 20
    loct follow events
    loct follow trace --handler my_command";

pub(super) const FIND_HELP: &str = "loct find - Semantic search for symbols and parameters

USAGE:
    loct find [QUERY...] [OPTIONS]

DESCRIPTION:
    Semantic search for symbols (functions, classes, types) AND function parameters.
    Uses regex patterns to match names in your codebase.

    Query modes:
    - Split-mode (multiple args): `loct find Foo Bar Baz`
        Runs separate searches per term and prints a cross-match summary of files
        that match 2+ queries.
    - AND-mode (single arg with spaces): `loct find \"Foo Bar Baz\"`
        Treats whitespace as AND and prints only the intersection (files matching
        all terms). This avoids the legacy \"auto-OR\" behavior.
    - Regex OR (explicit `|`): `loct find \"Foo|Bar|Baz\"`
        Preserves regex OR and enables built-in cross-match output.
    - Legacy OR: `loct find --or Foo Bar Baz`
        Forces old behavior (combines terms with `|`).

    Returns three types of matches:
    - Symbol Matches: exported functions, classes, types
    - Parameter Matches: function parameter names (NEW in 0.8.4)
    - Semantic Matches: similar symbol names (fuzzy matching)

    NOT impact analysis - for dependency impact, use 'loct impact'.
    NOT dead code detection - use 'loct dead' or 'loct twins'.

DEFAULT vs --literal (exact identifier-boundary occurrences over the indexed universe; coverage stated per query):
    Default 'find' is AST/fuzzy-aware — it matches symbols, parameters, and
    similar names. Powerful for discovery, but it can miss local variables
    buried in a function body and it surfaces fuzzy 'did you mean' candidates.

    '--literal <IDENT>' switches to exact identifier-boundary occurrences over the indexed universe (coverage stated per query): it scans raw source bytes
    for exact identifier-boundary occurrences (the same substrate as
    'loct occurrences'). Primary results are LITERAL ONLY — any fuzzy
    suggestions are returned in a separate, explicitly-labeled section
    ('fuzzy_suggestions') and are NEVER mixed into the literal matches.
    With '--literal', 'not found' means not found.

OPTIONS:
    --literal            Literal exact-identifier truth scan (no fuzzy primaries)
    --where-symbol       Symbol definition lookup via symbol_graph (incl. Swift/ObjC/C/C++)
    --who-imports        Reverse dependency lookup (same graph path as query who-imports)
    --or                Force legacy OR for multi-arg queries (Foo|Bar|Baz)
    --symbol <PATTERN>   Search for symbols matching regex
    --file <PATTERN>     Search for files matching regex
    --similar <SYMBOL>   Find symbols with similar names (fuzzy)
    --dead               Only show dead/unused symbols
    --exported           Only show exported symbols
    --lang <LANG>        Filter by language (ts, rs, js, py, etc.)
    --limit <N>          Maximum results to show
    --help, -h           Show this help message

EXAMPLES:
    loct find request                   # Find 'request' in symbols AND params
    loct find Props Options ViewModel   # Split-mode + cross-match summary
    loct find \"Props Options\"          # AND-mode (intersection)
    loct find \"Props|Options|ViewModel\" # Regex OR (explicit)
    loct find --or Props Options        # Legacy OR (Props|Options)
    loct find --symbol \".*Config$\"      # Regex: symbols ending with Config
    loct find --file \"utils\"            # Files containing \"utils\" in path
    loct find --dead --exported         # Dead exported symbols
    loct find --literal utterance_id    # Literal truth: every exact occurrence
    loct find --literal utterance_id --json  # Literal matches as JSON
    loct find WorkspaceSubstrate --where-symbol  # Where is the symbol defined?
    loct find src/utils.ts --who-imports  # What imports this file?

OUTPUT (default):
    === Symbol Matches (10) ===
      src/auth.py:45 - export def login
    === Parameter Matches (34) ===
      src/auth.py:45 - request: Request in login()
    === Semantic Matches (5) ===
      loginUser (score: 0.85) in src/users.py

OUTPUT (--literal --json):
    {
      \"mode\": \"literal\",
      \"query\": \"utterance_id\",
      \"literal_matches\": { \"source\": \"literal\", \"files_matched\": 1,
        \"occurrences\": [ { \"file\": \"src/lib.rs\", \"line\": 12, ... } ] },
      \"fuzzy_suggestions\": [ { \"symbol\": \"utterance\", \"source\": \"fuzzy\", ... } ]
    }

RELATED COMMANDS:
    loct occurrences <ident>  Literal-only scan (same substrate as --literal)
    loct dead              Find unused exports / dead code
    loct twins             Find duplicate exports and dead parrots
    loct slice <file>      Extract file dependencies
    loct query where-symbol  Find where a symbol is defined (same lookup as --where-symbol)";

pub(super) const OCCURRENCES_HELP: &str =
    "loct occurrences - Literal exact-identifier scan (exact identifier-boundary occurrences over the indexed universe; coverage stated per query)

USAGE:
    loct occurrences <IDENT> [OPTIONS]

DESCRIPTION:
    Walks raw source bytes of every snapshot file and reports EVERY
    identifier-boundary occurrence of <IDENT>. This provides exact
    identifier-boundary occurrences over the indexed universe (coverage stated per query)
    beneath 'find': it does not consult the AST/tagmap, so it sees local
    variables buried inside large function bodies (e.g. 'let mut utterance_id'
    plus later 'utterance_id += 1') that symbol search can miss.

    Matching is token/identifier-boundary aware, not naive substring:
    searching 'id' will NOT match inside 'utterance_id' or 'valid'.

    Results are literal only. No fuzzy suggestions are promoted as primary
    results — a suggestion is not evidence. 'Not found' means not found.

OPTIONS:
    --json               Emit JSON (file, line, column, matched_text, context, source)
    --help, -h           Show this help message

EXAMPLES:
    loct occurrences utterance_id            # All literal occurrences
    loct occurrences utterance_id --json     # JSON for tooling/agents

OUTPUT (JSON):
    {
      \"query\": \"utterance_id\",
      \"source\": \"literal\",
      \"files_matched\": 1,
      \"occurrences\": [
        { \"file\": \"src/lib.rs\", \"line\": 12, \"column\": 13,
          \"matched_text\": \"utterance_id\", \"context\": \"...\",
          \"source\": \"literal\" }
      ]
    }

RELATED COMMANDS:
    loct find              Semantic/symbol search (AST-backed, fuzzy-aware)
    loct query where-symbol  Find where a symbol is defined";

pub(super) const DEAD_HELP: &str = "loct dead - Detect unused exports / dead code

USAGE:
    loct dead [OPTIONS] [PATHS...]

DESCRIPTION:
    Detects unused exports with confidence levels and optional
    inclusion of tests/helpers. Integrates with quick wins.

OPTIONS:
    --confidence <lvl>   normal|high (default: normal)
    --top <N>            Limit results to top N (default: 20)
    --full, --all        Show all results (ignore --top limit)
    --with-tests         Include test files
    --with-helpers       Include helper files
    --with-shadows       Detect shadow exports (same symbol, multiple files)
    --help, -h           Show this help message

EXAMPLES:
    loct dead
    loct dead --confidence high
    loct dead --with-tests
    loct dead --with-shadows";

pub(super) const CYCLES_HELP: &str = "loct cycles - Detect circular import chains

USAGE:
    loct cycles [OPTIONS] [PATHS...]

DESCRIPTION:
    Detects circular dependencies in your import graph.
    Example: A -> B -> C -> A

    Circular imports cause:
    - Runtime initialization errors
    - Build/bundling failures
    - Flaky test behavior

OPTIONS:
    --path <PATTERN>     Filter to files matching pattern
    --help, -h           Show this help message

EXAMPLES:
    loct cycles                # Detect all cycles
    loct cycles src/           # Only analyze src/
    loct cycles --json         # JSON output for CI

RELATED COMMANDS:
    loct slice <file>       See what a file depends on
    loct query who-imports  Find reverse dependencies
    loct lint --fail        Run as CI check";

pub(super) const TRACE_HELP: &str = "loct trace - Trace a Tauri/IPC handler end-to-end

USAGE:
    loct trace <handler> [ROOTS...]
    loct trace --handler <handler> [ROOTS...]

DESCRIPTION:
    Investigates why a handler is missing/unused:
    - Backend definition (file, line, exposed name)
    - Frontend invoke() calls and plain mentions
    - Registration status in generate_handler![]
    - Verdict + suggestion to fix

OPTIONS:
    --help, -h           Show this help message

ARGUMENTS:
    <handler>            Handler name to trace (exposed or internal)
    [ROOTS...]           Root directories to scan (default: current directory)

EXAMPLES:
    loct trace toggle_assistant
    loct trace --handler toggle_assistant
    loct trace standard_command apps/desktop";

pub(super) const JQ_HELP: &str = "loct jq - Query snapshot with jq-style filters

USAGE:
    loct '<filter>' [OPTIONS]

DESCRIPTION:
    Execute jq-style filter expressions on the latest snapshot JSON.
    Automatically finds the most recent snapshot in the cache (override with LOCT_CACHE_DIR).

    The filter syntax follows jq conventions:
    - .metadata          Extract metadata field
    - .files[]           Iterate over files array
    - .files[0]          Get first file
    - .[\"key\"]         Access key with special characters

OPTIONS:
    -r, --raw-output         Output raw strings, not JSON
    -c, --compact-output     Compact JSON output (no pretty-printing)
    -e, --exit-status        Set exit code based on output (0 if truthy)
    --arg <name> <value>     Pass string variable to filter
    --argjson <name> <json>  Pass JSON variable to filter
    --snapshot <path>        Use specific snapshot file instead of latest
    --help, -h               Show this help message

EXAMPLES:
    loct '.metadata'                         # Extract metadata
    loct '.files | length'                   # Count files
    loct '.files[] | .path'                  # List file paths
    loct '.metadata.total_loc' -r            # Raw number output
    loct '.files[] | select(.lang == \"ts\")' -c
    loct '.files[] | select(.loc > 500)' -c

NOTE:
    This command requires jaq dependencies (enabled by default in the CLI build).";

pub(super) const COMMANDS_HELP: &str = "loct commands - Tauri FE<->BE handler coverage analysis

USAGE:
    loct commands [OPTIONS] [PATHS...]

DESCRIPTION:
    Analyzes Tauri command bridge contracts between frontend and backend.

    Detects:
    - Missing handlers: FE calls invoke('cmd') but no BE #[tauri::command]
    - Unused handlers: BE has #[tauri::command] but FE never calls it
    - Matched handlers: Both FE and BE exist (healthy)

OPTIONS:
    --name <PATTERN>     Filter to commands matching pattern
    --missing-only       Show only missing handlers
    --unused-only        Show only unused handlers
    --limit <N>          Maximum results to show
    --help, -h           Show this help message

EXAMPLES:
    loct commands                    # Full coverage report
    loct commands --missing-only     # Only missing handlers
    loct commands --json --missing   # JSON for CI

RELATED COMMANDS:
    loct events        Analyze Tauri event flow
    loct lint --tauri  Full Tauri contract validation";

pub(super) const EVENTS_HELP: &str = "loct events - Show event flow and issues

USAGE:
    loct events [OPTIONS] [PATHS...]

DESCRIPTION:
    Analyzes event emit/listen pairs, ghost events, and race conditions.

OPTIONS:
    --races             Enable race detection (async/await gaps)
    --no-duplicates     Hide duplicate sections in CLI output
    --no-dynamic-imports Hide dynamic import sections in CLI output
    --help, -h          Show this help message

EXAMPLES:
    loct events
    loct events --races";

pub(super) const ROUTES_HELP: &str = "loct routes - List backend/web routes (FastAPI/Flask)

USAGE:
    loct routes [OPTIONS] [PATHS...]

DESCRIPTION:
    Detects Python web routes based on common decorators:
    - FastAPI: @app.get/post/put/delete/patch, @router.*, @api_router.*
    - Flask:   @app.route, @blueprint.route, .route(...)

OPTIONS:
    --framework <NAME>   Filter by framework label (fastapi, flask)
    --path <PATTERN>     Filter by route path substring
    --help, -h           Show this help message

EXAMPLES:
    loct routes
    loct routes --framework fastapi
    loct routes --path /patients";

pub(super) const INFO_HELP: &str = "loct info - Show snapshot metadata and project info

USAGE:
    loct info

DESCRIPTION:
    Prints snapshot metadata, detected stack, and analysis summary.

OPTIONS:
    --help, -h          Show this help message";

pub(super) const LINT_HELP: &str = "loct lint - Structural lint and policy checks

USAGE:
    loct lint [OPTIONS] [PATHS...]

DESCRIPTION:
    Runs structural linting: entrypoints, handlers, ghost events, races.

OPTIONS:
    --entrypoints        List entrypoints
    --sarif              Emit SARIF
    --tauri              Apply Tauri presets
    --fail               Exit non-zero on findings
    --deep               Include ts/react/memory lint checks
    --ts                 Include TypeScript lint checks
    --react              Include React lint checks
    --memory             Include memory leak lint checks
    --no-duplicates      Hide duplicate sections in CLI output
    --no-dynamic-imports Hide dynamic import sections in CLI output
    --help, -h           Show this help message

EXAMPLES:
    loct lint
    loct lint --deep
    loct lint --fail --sarif";

pub(super) const PIPELINES_HELP: &str = "loct pipelines - Pipeline summary (events/commands/risks)

USAGE:
    loct pipelines [PATHS...]

DESCRIPTION:
    Prints a concise pipeline summary (events, commands, risk buckets)
    using the current snapshot.

OPTIONS:
    --help, -h        Show this help message

EXAMPLES:
    loct pipelines
    loct pipelines .";

pub(super) const INSIGHTS_HELP: &str = "loct insights - AI insights summary

USAGE:
    loct insights [PATHS...]

DESCRIPTION:
    Emits the AI insight hints (huge files, cross-language stems, missing handlers).

OPTIONS:
    --help, -h        Show this help message

EXAMPLES:
    loct insights
    loct insights .";

pub(super) const MANIFESTS_HELP: &str = "loct manifests - Manifest summaries

USAGE:
    loct manifests [PATHS...]

DESCRIPTION:
    Prints manifest summaries from snapshot metadata (package.json, Cargo.toml, pyproject).

OPTIONS:
    --help, -h        Show this help message

EXAMPLES:
    loct manifests
    loct manifests .";

pub(super) const REPORT_HELP: &str = "loct report - Generate HTML report + cached artifacts

USAGE:
    loct report [OPTIONS] [PATHS...]

DESCRIPTION:
    Runs full analysis and writes the full HTML report plus cached artifacts
    such as findings.json, agent.json, analysis.json, and report.sarif.

OPTIONS:
    --output <FILE>      Output HTML path
    --serve              Serve report locally
    --port <N>           Port for --serve
    --editor <NAME>      Editor integration (code/cursor/windsurf/jetbrains)
    --help, -h           Show this help message

ENV:
    LOCT_OPEN_BROWSER    Set to 0/false/no to suppress browser auto-open
                         (handy for CI, scripts, e2e tests). Default: open.

EXAMPLES:
    loct report
    loct report --output report.html
    loct report --serve --port 4173";

pub(super) const FINDINGS_HELP: &str = "loct findings - Emit findings JSON to stdout

USAGE:
    loct findings [OPTIONS] [PATHS...]

DESCRIPTION:
    Runs the full findings pipeline and prints the same issue payload that
    powers findings.json, but directly to stdout for piping and automation.

    Use --summary for the compact health/counts payload when you only need
    top-line status in CI or scripts.

OPTIONS:
    --summary         Emit only the compact summary payload
    --json            Output as JSON (default for this command)
    --help, -h        Show this help message

ARGUMENTS:
    [PATHS...]        Root directories to scan (default: current directory)

EXAMPLES:
    loct findings
    loct findings --summary
    loct findings src/ > findings.json
    loct findings --summary | jq '.health_score'";

pub(super) const QUERY_HELP: &str = "loct query - Graph queries (who-imports, who-exports, etc.)

USAGE:
    loct query <KIND> <TARGET>

DESCRIPTION:
    Query the import graph and symbol index for specific relationships.
    Targeted queries against the dependency graph built by 'loct scan'.

QUERY KINDS:
    who-imports <FILE>      Find all files that import the file (reverse deps)
    where-symbol <SYMBOL>   Find where a symbol is defined/exported
    component-of <FILE>     Show which component/module contains the file
    swift-types <FILE>      Classify Swift type-position refs as resolved/external/unresolved

OPTIONS:
    --help, -h              Show this help message

EXAMPLES:
    loct query who-imports src/utils.ts       # What imports utils.ts?
    loct query where-symbol PatientRecord     # Where is it defined?
    loct query component-of src/ui/Button.tsx # What owns Button?
    loct query swift-types Sources/App/AppController.swift --json

RELATED COMMANDS:
    loct slice <file>           Show what a file depends on
    loct find --symbol <name>   Search for symbols by pattern
    loct dead                   Find symbols with 0 imports";

pub(super) const BODY_HELP: &str = "loct body - Show the bounded source body/range of a symbol

USAGE:
    loct body <SYMBOL> [OPTIONS]

DESCRIPTION:
    Once `where-symbol` locates a symbol, `body` shows the actual source
    lines of its definition without ever shelling out to grep/sed.
    Body extraction is brace-balanced (Rust, TS/JS, C-family) with a
    fixed-window fallback for brace-less languages (e.g. Python). Output
    is always bounded by a line cap with explicit truncation metadata.

OPTIONS:
    --max-lines <N>   Cap source lines returned per body (default: 200)
    --json            Emit JSON (file, start/end line, language, source)
    --help, -h        Show this help message

EXAMPLES:
    loct body transcription_session
    loct body handle_query_command --max-lines 80
    loct body query_where_symbol --json

RELATED COMMANDS:
    loct query where-symbol <name>   Locate the symbol first
    loct slice <file>                Show what a file depends on";

pub(super) const IMPACT_HELP: &str = "loct impact - Analyze impact of modifying/removing a file

USAGE:
    loct impact <FILE> [OPTIONS]

DESCRIPTION:
    Shows \"what breaks if you modify or remove this file\" by traversing
    the reverse dependency graph. Finds all direct and transitive consumers.

    This is different from 'query who-imports':
    - who-imports: Finds direct importers only
    - impact: Finds ALL affected files (direct + transitive)

    Useful for:
    - Understanding change impact before refactoring
    - Identifying critical files (high fan-out)
    - Safe deletion analysis

OPTIONS:
    --depth <N>          Limit traversal depth (default: unlimited)
    --root <PATH>        Project root (default: current directory)
    --json               Output as JSON for agent consumption
    --help, -h           Show this help message

ARGUMENTS:
    <FILE>               Path to the file to analyze (required)

EXAMPLES:
    loct impact src/utils.ts                # Full impact analysis
    loct impact src/api.ts --depth 2        # Limit to 2 levels deep
    loct impact lib/helpers.ts --json       # JSON output
    loct impact src/core.ts --root ./       # Specify project root

OUTPUT FORMAT:
    Direct consumers (5 files):
      src/app.ts (import)
      src/lib.ts (import)
      ...

    Transitive impact (23 files total):
      [depth 2] src/page.tsx (import)
      ...

    Warning: Removing this file would break 28 files (max depth: 3)";

pub(super) const DIFF_HELP: &str = "loct diff - Compare snapshots between branches/commits

USAGE:
    loct diff --since <SNAPSHOT> [--to <SNAPSHOT>] [OPTIONS]

DESCRIPTION:
    Compares two code snapshots and shows semantic differences.

    Unlike git diff (line changes), this shows structural changes:
    - New/removed files and symbols
    - Import graph changes
    - New dead code introduced (regressions)
    - New circular dependencies

OPTIONS:
    --since <SNAPSHOT>   Base snapshot to compare from (required)
    --to <SNAPSHOT>      Target snapshot (default: current working tree)
    --auto-scan-base     Auto-create git worktree and scan target branch
    --changed-files      Show only the changed-file summary for <ref>..HEAD
    --problems-only      Show only regressions (new dead code, new cycles)
    --help, -h           Show this help message

EXAMPLES:
    loct diff --since main                    # Compare main to working tree
    loct diff --since HEAD~1                  # Compare to previous commit
    loct diff --since main --changed-files    # Summarize changed files only
    loct diff --since main --auto-scan-base   # Auto-scan main branch
    loct diff --since v1.0.0 --to v2.0.0      # Compare two tags

RELATED COMMANDS:
    loct scan             Run scan to create snapshot
    loct auto --full-scan Force full rescan";

pub(super) const CROWD_HELP: &str =
    "loct crowd - Detect functional crowds (similar files clustering)

USAGE:
    loct crowd [PATTERN]

DESCRIPTION:
    Groups related files around a seed pattern (name or path fragment).

OPTIONS:
    --help, -h          Show this help message

EXAMPLES:
    loct crowd cache
    loct crowd session";

pub(super) const TAGMAP_HELP: &str = "loct tagmap - Unified search around a keyword

USAGE:
    loct tagmap <KEYWORD> [OPTIONS]

DESCRIPTION:
    Aggregates three analyses into one view:
    1. FILES:  All files with keyword in path or name
    2. CROWD:  Functional clustering around the keyword
    3. DEAD:   Dead exports related to the keyword

    Perfect for understanding everything about a domain/feature at once.

OPTIONS:
    --include-tests    Include test files in analysis
    --limit <N>        Maximum results per section (default: 20)
    --json             Output as JSON for AI tools
    --help, -h         Show this help message

ARGUMENTS:
    <KEYWORD>          Keyword to search for (required)

EXAMPLES:
    loct tagmap patient           # Everything about 'patient' feature
    loct tagmap auth              # Auth-related files, crowds, dead code
    loct tagmap message --json    # JSON output for AI processing
    loct tagmap api --limit 10    # Limit results

OUTPUT FORMAT:
    === TAGMAP: 'patient' ===

    FILES MATCHING KEYWORD (12):
      src/features/patients/PatientsList.tsx
      src/hooks/usePatient.ts
      ...

    CROWD ANALYSIS (8 files):
      Score: 7.2/10
      Members: PatientsList, PatientDetail, PatientForm...
      Issues: Consider consolidating similar files

    DEAD EXPORTS (3):
      oldPatientHandler in src/api/patients.ts
      PatientV1 in src/types/patient.ts
      ...

RELATED COMMANDS:
    loct crowd <pattern>    Detailed crowd analysis
    loct dead               All dead exports
    loct find <query>       Symbol/file search
    loct focus <dir>        Directory-level context";

pub(super) const TWINS_HELP: &str =
    "loct twins - Find dead parrots (0 imports) and duplicate exports

USAGE:
    loct twins [OPTIONS] [PATH]

DESCRIPTION:
    Detects semantic issues in your export/import graph:

    Dead Parrots:   Exports with 0 imports anywhere in the codebase
                    (Monty Python reference - code that looks alive but isn't used)

    Exact Twins:    Same symbol name exported from multiple files
                    (can cause import confusion)

    Barrel Chaos:   Re-export anti-patterns
                    (missing index.ts, deep re-export chains)

    This is a code smell detector - findings are hints, not verdicts.

OPTIONS:
    --path <DIR>           Root directory to analyze
    --dead-only            Show only dead parrots (0 imports)
    --include-tests        Include test files (excluded by default)
    --include-suppressed   Show suppressed findings too
    --help, -h             Show this help message

EXAMPLES:
    loct twins                    # Full analysis
    loct twins --dead-only        # Only exports with 0 imports
    loct twins src/               # Analyze specific directory
    loct twins --include-tests    # Include test files
    loct twins --include-suppressed  # Include suppressed items

SUPPRESSION:
    Mark findings as false positives (they won't show in subsequent runs):
    loct suppress twins <symbol>              # Suppress a twin
    loct suppress twins <symbol> --file <f>   # Suppress only in specific file
    loct suppress --list                      # Show all suppressions
    loct suppress --clear                     # Clear all suppressions

RELATED COMMANDS:
    loct dead              Detailed dead code analysis
    loct suppress          Manage false positive suppressions
    loct find --dead       Search for specific dead symbols";

pub(super) const DIST_HELP: &str = "loct dist - Verify tree-shaking from production source maps

USAGE:
    loct dist --src <DIR> --source-map <PATH> [--source-map <PATH> ...] [--report <PATH>]

DESCRIPTION:
    Simple mental model:
    - Point loct at the source directory you own
    - Point it at one or more production source maps or a dist/ directory
    - loct builds a chunk matrix and ranks suspicious runtime classes

    Candidate classes:
    - dead_in_all_chunks
    - boot_path_only
    - feature_local
    - fake_lazy
    - verify_first

    loct uses symbol or line evidence when a source map exposes it.
    If a map does not, it falls back to file-level chunk coverage for that map.
    The standard artifact set under `.loctree/` is refreshed as part of the run,
    so bundle intelligence also lands in report.html, findings.json, agent.json,
    and manifest.json.

OPTIONS:
    --src <DIR>              Source directory to scan once (e.g., src/)
    --source-map <PATH>      Production source map or directory to compare against
                             Repeat for multi-entry or multi-bundle builds
    --report <PATH>          Write the raw dist JSON result to a file
    --help, -h               Show this help message

EXAMPLES:
    loct dist --src src/ --source-map dist/main.js.map
    loct dist --src src/ --source-map dist/
    loct dist --src src/ --source-map dist/app.js.map --source-map dist/admin.js.map
    loct dist --src src/ --source-map dist/main.js.map --report .loctree/dist-report.json

OUTPUT:
    Default stdout: human summary
    Add --json: machine-readable stdout
    Add --report: save the JSON result to disk

JSON adds chunk summaries, candidate counts, and ranked candidates while keeping
the existing dead export fields stable.";

pub(super) const COVERAGE_HELP: &str =
    "loct coverage - Analyze test coverage gaps (structural coverage)

USAGE:
    loct coverage [OPTIONS] [PATHS...]

DESCRIPTION:
    Performs structural test coverage analysis by cross-referencing:
    - Frontend invoke/emit calls (what the app uses)
    - Backend handlers and events (what the app exposes)
    - Test file imports (what tests actually cover)

    Unlike line coverage tools, this shows:
    - Which handlers have no corresponding tests
    - Which events are emitted but never tested
    - Which exports are tested but never used in production

    This is semantic coverage - not 'how many lines' but 'what functionality'.

OPTIONS:
    --handlers            Only show handler gaps (skip events/exports)
    --events              Only show event gaps (skip handlers/exports)
    --tests               Show structural test coverage report
    --gaps                Show coverage gap analysis (default)
    --min-severity <LVL>  Filter by minimum severity: critical, high, medium, low
    --json                Output as JSON for programmatic use
    --help, -h            Show this help message

ARGUMENTS:
    [PATHS...]            Root directories to scan (default: current directory)

EXAMPLES:
    loct coverage                          # Show all coverage gaps
    loct coverage --handlers              # Focus on untested handlers
    loct coverage --tests                 # Structural test coverage
    loct coverage --min-severity high      # Only critical/high issues
    loct coverage --json                   # Machine-readable output

OUTPUT:
    Groups findings by severity:
    - CRITICAL: Handlers without any test (used in production!)
    - HIGH: Events emitted but never tested
    - MEDIUM: Exports without test imports
    - LOW: Tests that import unused code

    Each gap shows the source location and usage context.";

pub(super) const SNIFF_HELP: &str = "loct sniff - Sniff for code smells (aggregate analysis)

USAGE:
    loct sniff [OPTIONS]

DESCRIPTION:
    Aggregates all smell-level findings worth checking:

    Twins:        Same symbol name in multiple files
                  - Can cause import confusion

    Dead Parrots: Exports with 0 imports
                  - Potentially unused code

    Crowds:       Files with similar dependency patterns
                  - Possible duplication or fragmentation

    Output is friendly and non-judgmental. These are hints, not verdicts.

OPTIONS:
    --path <DIR>           Root directory to analyze (default: current directory)
    --dead-only            Show only dead parrots (skip twins and crowds)
    --twins-only           Show only twins (skip dead parrots and crowds)
    --crowds-only          Show only crowds (skip twins and dead parrots)
    --include-tests        Include test files in analysis (default: false)
    --min-crowd-size <N>   Minimum crowd size to report (default: 2)
    --json                 Output as JSON for programmatic use
    --help, -h             Show this help message

EXAMPLES:
    loct sniff                    # Full code smell analysis
    loct sniff --dead-only        # Only dead parrots
    loct sniff --twins-only       # Only duplicate names
    loct sniff --crowds-only      # Only similar file clusters
    loct sniff --include-tests    # Include test files
    loct sniff --json             # Machine-readable output

OUTPUT:
    Aggregates three types of code smells:
    - TWINS: Same symbol exported from multiple files
    - DEAD PARROTS: Exports with 0 imports
    - CROWDS: Files clustering around similar functionality

    Each section provides actionable suggestions for consolidation or cleanup.";

pub(super) const SUPPRESS_HELP: &str = "loct suppress - Mark findings as false positives

USAGE:
    loct suppress <type> <symbol> [OPTIONS]
    loct suppress --list
    loct suppress --clear

DESCRIPTION:
    Manages false positive suppressions so reviewed findings don't appear
    in subsequent runs. Suppressions are stored in .loctree/suppressions.toml.

    Use this when you've reviewed a finding and determined it's intentional:
    - FE/BE type mirrors (same type defined in TypeScript and Rust)
    - Intentional re-exports for public APIs
    - Entry points that appear 'dead' but are used externally

TYPES:
    twins          Exact twin (same symbol in multiple files)
    dead_parrot    Dead parrot (export with 0 imports)
    dead_export    Dead export (unused export)
    circular       Circular import

OPTIONS:
    --file <PATH>      Only suppress in specific file (default: all files)
    --reason <TEXT>    Document why this is a false positive
    --list             Show all current suppressions
    --clear            Remove all suppressions
    --help, -h         Show this help message

EXAMPLES:
    loct suppress twins Message              # Suppress 'Message' twin everywhere
    loct suppress twins Message --file src/types.ts  # Only in specific file
    loct suppress dead_parrot unusedHelper --reason 'Used via dynamic import'
    loct suppress --list                     # View all suppressions
    loct suppress --clear                    # Reset suppressions

STORAGE:
    Suppressions are stored in .loctree/suppressions.toml
    Commit this file to share suppressions with your team.

RELATED COMMANDS:
    loct twins         Find twins and dead parrots (--include-suppressed to show all)
    loct dead          Find unused exports
    loct findings      Canonical findings artifact";

pub(super) const SUPPRESSIONS_HELP: &str = "loct suppressions - Source-side silencer inventory

USAGE:
    loct suppressions [OPTIONS] [ROOT]

DESCRIPTION:
    Surfaces every source-side silencer in the repo as a structured report:
    #[allow(...)] / #[expect(...)] (Rust), #[ignore] test attrs, unsafe { }
    blocks (with Rust 2024 env-var boilerplate triaged separately),
    // nosemgrep, @ts-ignore / @ts-expect-error / @ts-nocheck,
    eslint-disable, # noqa, # type: ignore, # pylint: disable, # mypy:,
    # shellcheck disable.

    Distinct from `loct suppress` (which manages loctree's OWN finding-
    suppression file at .loctree/suppressions.toml). This command detects
    silencers engineers left in code to mute external linters/checkers.

    Literal-only detection (regex matching). Semantic enrichment
    (suspicious/stale/similar-to-fixed classification) is a paid-tier
    delta (Wave 7+ post-aicx-library integration). Free-tier scope is
    locked to this literal surface.

OPTIONS:
    --type <KIND>        Filter to one kind (repeatable). Tokens:
                           allow, dead-code, nosemgrep, ts-ignore,
                           ts-expect-error, ts-nocheck, eslint-disable,
                           noqa, type-ignore, pylint-disable,
                           mypy-ignore, shellcheck, unsafe,
                           unsafe-env-var, ignore
    --summary            Count-per-kind table (default if no other mode)
    --json               Emit structured JSON (one record per occurrence)
    --include-fixtures   Include paths normally excluded by .semgrepignore
    --help, -h           Show this help message

ARGUMENTS:
    [ROOT]               Project root to scan (default: current directory)

EXAMPLES:
    loct suppressions                          # Summary table
    loct suppressions --type nosemgrep         # Only nosemgrep matches
    loct suppressions --type unsafe-env-var    # Rust 2024 env-var boilerplate
    loct suppressions --type unsafe            # Real unsafe blocks only
    loct suppressions --type dead-code --json  # Forgotten gems, JSON
    loct suppressions --type allow --type ignore  # Multiple filters (OR)

OUTPUT (--summary, default):
    Suppression inventory — <repo> (after .semgrepignore)
      nosemgrep            : 8 (3 files)
      dead-code            : 9 (5 files)   <- forgotten gems
      allow                : 6 (4 files)
      ignore               : 2 (2 files)
      unsafe-env-var       : 46 (Rust 2024 boilerplate)
      unsafe               : 5 (3 files)
      ...
    Total: 76 silencers across 17 files.

OUTPUT (--json):
    [
      { \"kind\": \"nosemgrep\", \"file\": \"src/lib.rs\", \"line\": 42,
        \"snippet\": \"// nosemgrep: rust.actix.path-traversal\",
        \"rule_id\": \"rust.actix.path-traversal\" },
      ...
    ]

RELATED COMMANDS:
    loct suppress       Manage loctree's own finding-suppression file
                          (.loctree/suppressions.toml). Different concept,
                          similar name — see `loct suppressions --help`.";

pub(super) const FOCUS_HELP: &str = "loct focus - Extract holographic context for a directory

USAGE:
    loct focus <DIRECTORY> [OPTIONS]

DESCRIPTION:
    Like 'slice' but for directories. Extracts a holographic view of a directory:

    Core:       All files within the target directory
    Internal:   Import edges between files inside the directory
    Deps:       External files imported by core (outside the directory)
    Consumers:  Files outside the directory that import core files

    Perfect for understanding feature modules like 'src/features/settings/'.

OPTIONS:
    --consumers, -c    Include files that import from this directory (default)
    --no-consumers     Hide files that import from this directory
    --depth <N>        Maximum depth for external dependency traversal
    --root <PATH>      Project root (default: current directory)
    --json             Output as JSON for agent consumption
    --help, -h         Show this help message

ARGUMENTS:
    <DIRECTORY>        Path to the directory to analyze (required)

EXAMPLES:
    loct focus src/features/settings/           # Focus + deps + consumers
    loct focus src/components/ --no-consumers   # Dependency-only view
    loct focus lib/utils/ --depth 1             # Limit external dep depth
    loct focus src/api/ --json                  # JSON output for AI tools

OUTPUT FORMAT:
    Focus: src/features/settings/

    Core (12 files, 2,340 LOC):
      src/features/settings/index.ts
      src/features/settings/SettingsList.tsx
      ...

    Internal edges: 18 imports within directory

    External Deps (8 files, 890 LOC):
      [d1] src/components/Button.tsx
      ...

    Consumers (3 files, 450 LOC):
      src/App.tsx
      ...

    Total: 23 files, 3,680 LOC

RELATED COMMANDS:
    loct slice <file>       Extract context for a single file
    loct impact <file>      Show what breaks if you change a file
    loct crowd <pattern>    Find files clustering around a pattern";

pub(super) const HOTSPOTS_HELP: &str =
    "loct hotspots - Import frequency heatmap (core vs peripheral)

USAGE:
    loct hotspots [OPTIONS]

DESCRIPTION:
    Ranks files by how often they are imported (in-degree) to identify:

    CORE:       Files imported by 10+ others (critical infrastructure)
    SHARED:     Files imported by 3-9 others (shared utilities)
    PERIPHERAL: Files imported by 1-2 others (feature-specific)
    LEAF:       Files with 0 importers (entry points or dead code)

    This helps AI agents understand which files are risky to modify
    (high in-degree = many dependents) vs safe to refactor (low in-degree).

OPTIONS:
    --min <N>              Minimum import count to show (default: 1)
    --limit <N>            Maximum files to show (default: 50)
    --leaves               Show only leaf nodes (0 importers)
    --coupling             Include out-degree (files that import many others)
    --root <PATH>          Project root (default: current directory)
    --json                 Output as JSON for agent consumption
    --help, -h             Show this help message

EXAMPLES:
    loct hotspots                    # Show top 50 most-imported files
    loct hotspots --limit 20         # Top 20 only
    loct hotspots --leaves           # Find leaf nodes (entry points / dead)
    loct hotspots --coupling         # Show both in-degree and out-degree
    loct hotspots --min 5            # Only files with 5+ importers
    loct hotspots --json             # JSON output for AI tools

OUTPUT FORMAT:
    Import Hotspots (42 files analyzed)

    CORE (10+ importers):
      [32] src/utils/helpers.ts           # hub module
      [18] src/components/Button.tsx

    SHARED (3-9 importers):
      [7]  src/hooks/useAuth.ts
      [5]  src/api/client.ts

    PERIPHERAL (1-2 importers):
      [2]  src/features/login/form.tsx
      [1]  src/features/login/types.ts

    LEAF (0 importers):
      src/pages/index.tsx               # entry point
      src/features/old/legacy.ts        # possibly dead

    With --coupling:
      [in:32 out:3]  src/utils/helpers.ts    # hub, low coupling
      [in:2  out:15] src/features/main.tsx   # feature root, high coupling

RELATED COMMANDS:
    loct dead               Find unused exports
    loct impact <file>      Show what breaks if you modify a file
    loct focus <dir>        Extract context for a directory";

pub(super) const LAYOUTMAP_HELP: &str = "loct layoutmap - Analyze CSS layout properties

USAGE:
    loct layoutmap [OPTIONS]

DESCRIPTION:
    Extracts and analyzes layout-related CSS properties from your codebase:

    Z-INDEX:    Shows all z-index values across CSS/SCSS files, sorted by value.
                Helps identify layering conflicts and understand UI stacking.

    POSITION:   Lists sticky/fixed positioned elements.
                Useful for understanding what elements persist during scroll.

    DISPLAY:    Identifies grid/flex layouts and their locations.
                Maps out the layout architecture of your components.

OPTIONS:
    --zindex-only          Show only z-index values
    --sticky-only          Show only sticky/fixed position elements
    --grid-only            Show only grid/flex layouts
    --min-zindex <N>       Filter z-index values >= N (default: show all)
    --exclude <PATTERN>    Exclude paths matching glob (can be repeated)
    --root <PATH>          Project root (default: current directory)
    --json                 Output as JSON for agent consumption
    --help, -h             Show this help message

EXAMPLES:
    loct layoutmap                  # Full CSS layout analysis
    loct layoutmap --zindex-only    # Only z-index hierarchy
    loct layoutmap --sticky-only    # Only sticky/fixed elements
    loct layoutmap --min-zindex 100 # High z-index values (likely overlays)
    loct layoutmap --exclude .obsidian --exclude prototype  # Skip dirs
    loct layoutmap --json           # JSON output for AI tools

OUTPUT FORMAT:
    Z-INDEX HIERARCHY:
      [9999] src/components/Modal.css:15       .modal-overlay
      [1000] src/components/Toast.css:8        .toast-container
      [ 100] src/components/Dropdown.css:23    .dropdown-menu
      [  10] src/components/Header.css:5       .header

    STICKY/FIXED ELEMENTS:
      [fixed]  src/components/Header.css:12    .header
      [sticky] src/components/Sidebar.css:5    .sidebar-nav

    GRID/FLEX LAYOUTS:
      [grid]   src/layouts/Dashboard.css:8     .dashboard-grid
      [flex]   src/components/Card.css:3       .card-content

RELATED COMMANDS:
    loct crowd              Find functionally similar components
    loct find <pattern>     Search for CSS selectors or properties";

pub(super) const ZOMBIE_HELP: &str = "loct zombie - Find all zombie code (combined analysis)

USAGE:
    loct zombie [OPTIONS] [PATHS...]

DESCRIPTION:
    Combines three sources of dead code into one actionable report:

    DEAD EXPORTS:     Unused exports detected by dead code analysis
                      (symbols with 0 imports)

    ORPHAN FILES:     Files with 0 importers (not imported by any other file)
                      Entry points are OK, but others might be dead

    SHADOW EXPORTS:   Same symbol exported by multiple files where some
                      have 0 imports (likely consolidation candidates)

    This is a comprehensive zombie hunter - finds all forms of potentially
    dead code in a single scan.

OPTIONS:
    --include-tests    Include test files in analysis (default: false)
    --json             Output as JSON for programmatic use
    --help, -h         Show this help message

ARGUMENTS:
    [PATHS...]         Root directories to scan (default: current directory)

EXAMPLES:
    loct zombie                    # Find all zombie code
    loct zombie --include-tests    # Include test files
    loct zombie src/               # Analyze specific directory
    loct zombie --json             # Machine-readable output

OUTPUT FORMAT:
    Zombie Code Report

    Dead Exports (3):
      src/utils/old.ts:15 - unusedFunction
      src/hooks/legacy.ts:8 - useLegacyHook
      ...

    Orphan Files (0 importers, 2):
      src/features/settings/SettingsList.tsx (504 LOC)
      src/components/deprecated/OldButton.tsx (89 LOC)

    Shadow Exports (1):
      conversationHostStore exported by 2 files, 1 dead

    Total: 6 zombie items, ~950 LOC to review

RELATED COMMANDS:
    loct dead               Detailed dead export analysis
    loct twins              Dead parrots and semantic duplicates
    loct hotspots --leaves  Find leaf nodes (0 importers)
    loct sniff              Code smell analysis";

pub(super) const HEALTH_HELP: &str = "loct health - Quick health check summary

USAGE:
    loct health [OPTIONS] [PATHS...]

DESCRIPTION:
    One-shot summary that aggregates dead/twins/cycles — no additional detectors.

    Sources:
    - Cycles: Circular import count (hard vs structural)
    - Dead: Unused exports (high confidence count)
    - Twins: Duplicate symbol names across files

    Use this as a quick sanity check before commits or in CI.
    Run individual commands for detailed analysis.

OPTIONS:
    --include-tests    Include test files in analysis (default: false)
    --json             Output as JSON for programmatic use
    --help, -h         Show this help message

ARGUMENTS:
    [PATHS...]         Root directories to scan (default: current directory)

EXAMPLES:
    loct health                    # Quick health summary
    loct health --include-tests    # Include test files
    loct health src/               # Analyze specific directory
    loct health --json             # Machine-readable output

OUTPUT FORMAT:
    Health Check Summary

    Cycles:      3 total (2 hard, 1 structural)
    Dead:        6 high confidence, 24 low
    Twins:       2 duplicate symbol groups

    Run `loct cycles`, `loct dead`, `loct twins` for details.

RELATED COMMANDS:
    loct cycles    Detailed circular import analysis
    loct dead      Detailed dead export analysis
    loct twins     Duplicate export analysis
    loct findings  Canonical findings artifact";

pub(super) const AUDIT_HELP: &str = "loct audit - Full codebase audit with actionable findings

USAGE:
    loct audit [OPTIONS] [PATHS...]

DESCRIPTION:
    Report that aggregates dead/twins/cycles — no additional detectors.
    Comprehensive analysis combining existing structural checks into one report.
    Perfect for getting a complete picture of codebase health on day one.

    Includes:
    - Cycles: Circular imports (hard + structural)
    - Dead exports: Unused code with 0 imports
    - Twins: Same symbol exported from multiple files
    - Orphan files: Files with 0 importers (not entry points)
    - Shadow exports: Consolidation candidates
    - Crowds: Files with similar dependency patterns

    Each finding includes actionable suggestions for cleanup.

OPTIONS:
    --include-tests    Include test files in analysis (default: false)
    --todos            Save an actionable todo checklist instead of the full audit report
    --limit <N>        Intentionally truncate each section to N items (JSON marks omissions)
    --no-open          Save report without opening it automatically
    --json             Output as JSON for programmatic use
    --help, -h         Show this help message

ARGUMENTS:
    [PATHS...]         Root directories to scan (default: current directory)

EXAMPLES:
    loct audit                     # Full audit of current directory
    loct audit --include-tests     # Include test files
    loct audit src/                # Audit specific directory
    loct audit --todos             # Save a focused cleanup checklist
    loct audit --limit 25          # Intentionally trim each section
    loct audit --json              # Machine-readable output for CI

OUTPUT FORMAT:
    Markdown report saved to the loct cache artifacts directory.
    Terminal output shows a short summary plus the saved report path.
    JSON output is full by default; when --limit is set, each section includes
    explicit truncated/omitted metadata.

    Full Codebase Audit

    CYCLES (3 total)
      2 hard cycles (breaking)
      1 structural cycle

    DEAD EXPORTS (12 total)
      6 high confidence
      6 low confidence

    TWINS (2 groups)
      useAuth exported from 2 files
      formatDate exported from 3 files

    ORPHAN FILES (4 files, 1,200 LOC)
      src/legacy/old-utils.ts (450 LOC)
      src/deprecated/helper.ts (320 LOC)
      ...

    SHADOW EXPORTS (1)
      store exported by 2 files, 1 dead

    CROWDS (2 clusters)
      API handlers: 5 similar files
      Form components: 3 similar files

    ----------------------
    Total: 22 findings to review
    Run individual commands for details.

RELATED COMMANDS:
    loct health    Quick summary (cycles + dead + twins only)
    loct findings  Canonical findings artifact
    loct cycles    Detailed cycle analysis
    loct dead      Detailed dead export analysis";

pub(super) const DOCTOR_HELP: &str = "loct doctor - Operator-facing trust diagnostics

USAGE:
    loct doctor [OPTIONS]

DESCRIPTION:
    Inspect cached project identities and provide the stable command surface
    for snapshot scope validation and cache cleanup.

    Cut 2 T0 owns cache identity listing. Cut 2 T1 fills scope validation.
    Cut 2 T2 fills fix mode.

OPTIONS:
    --list                 List all cached projects (default if no mode is selected)
    --cache                Inspect cache identity and latest scan metadata
    --scope                Validate snapshot scope (T1)
    --fix                  Purge scope-mismatched cache entries (T2; interactive)
    --yes                  Skip interactive confirmation for --fix
    --json                 Emit JSON report with schema_version = \"1.0\"
    --project PATH         Limit diagnostics to one project path
    --help, -h             Show this help message

EXAMPLES:
    loct doctor
    loct doctor --cache
    loct doctor --scope --project /path/to/repo
    loct doctor --cache --scope --json

OUTPUT FORMAT:
    Cached projects (N total)
    project_id | canonical_root | branch@commit | last_scan

RELATED COMMANDS:
    loct cache list      Deprecated; prefer loct doctor --list
    loct info            Show snapshot metadata for the current project";

pub(super) const PLAN_HELP: &str = "loct plan - Generate architectural refactoring plan

USAGE:
    loct plan [OPTIONS] [PATHS...]
    loct p [OPTIONS] [PATHS...]

DESCRIPTION:
    Analyzes module coupling and generates a safe refactoring plan.
    Detects architectural layers (UI, App, Kernel, Infra) and suggests
    file moves ordered by risk level (LOW first).

    The plan includes:
    - Layer detection via path heuristics
    - Risk scoring based on consumer count and file size
    - Cyclic dependency warnings
    - Re-export shim generation for backward compatibility

OPTIONS:
    --target-layout <SPEC>   Custom layer mapping (e.g., \"core=src/kernel,ui=src/views\")
    --markdown               Output as markdown (default)
    --json                   Output as JSON
    --script                 Output as executable shell script
    --all                    Generate all formats (.md, .json, .sh)
    --output, -o <PATH>      Output file path (without extension for --all)
    --no-open                Don't auto-open the generated report
    --include-tests          Include test files in analysis
    --min-coupling <N>       Minimum coupling score to include (0.0-1.0)
    --max-module-size <N>    Maximum module LOC before suggesting split
    --help, -h               Show this help message

ARGUMENTS:
    [PATHS...]               Directory/directories to analyze (default: current directory)

EXAMPLES:
    loct plan                          # Analyze current directory
    loct plan src/features             # Analyze specific directory
    loct plan src app                  # Analyze multiple targets
    loct plan --all -o refactor-2026   # Generate all formats
    loct plan --json                   # Output JSON to stdout
    loct plan --script > migrate.sh    # Generate executable script

OUTPUT FORMATS:

    Markdown (default):
    - Summary with file counts and risk breakdown
    - Layer distribution before/after
    - Phased execution plan (LOW -> MEDIUM -> HIGH risk)
    - Git commands for each phase
    - Shim generation instructions

    Shell Script (--script):
    - Executable bash script with phases
    - Dry-run support: ./migrate.sh --dry
    - Phase selection: ./migrate.sh 1

    JSON (--json):
    - Full RefactorPlan structure
    - Moves, shims, cyclic groups, stats

LAYER DETECTION:
    UI       components/, views/, pages/, ui/, widgets/
    App      hooks/, services/, stores/, state/, providers/
    Kernel   core/, domain/, models/, entities/, business/
    Infra    utils/, helpers/, lib/, adapters/, api/
    Test     tests/, __tests__/, .test., .spec.

RISK LEVELS:
    LOW      Few consumers (<5), small file (<200 LOC), not in cycle
    MEDIUM   Moderate consumers (5-10), medium file (200-500 LOC)
    HIGH     Many consumers (>10), large file (>500 LOC), or in cycle

RELATED COMMANDS:
    loct impact <file>   What breaks if you modify this file
    loct focus <dir>     Holographic context for a directory
    loct cycles          Detect circular imports
    loct audit           Full codebase audit";

pub(super) const CACHE_HELP: &str = "loct cache - Manage snapshot cache

USAGE:
    loct cache <SUBCOMMAND> [OPTIONS]

DESCRIPTION:
    List and clean snapshot caches. Each project gets a cached snapshot
    in the global cache directory (~/Library/Caches/loctree/ on macOS,
    $XDG_CACHE_HOME/loctree/ on Linux).

SUBCOMMANDS:
    list                   List cached buckets grouped by repo, path, size, and scan metadata
    clean                  Remove cached snapshots
    prune|gc|clear-stale   Alias for clean, intended for quota/ENOSPC recovery

CLEAN OPTIONS:
    --project <DIR>        Only clean cache for a specific project
    --older-than <DAYS>d   Only remove entries older than N days (e.g., 7d, 30d)
    --max-size <SIZE>      Cap total cache size; evict oldest buckets first
                           (e.g., 1GB, 500MB, 250M, or plain bytes)
    --force, -f            Skip confirmation prompt

EXAMPLES:
    loct cache list                        # Show grouped cached buckets
    loct cache clean                       # Remove all (with confirmation)
    loct cache clean --force               # Remove all without asking
    loct cache clean --project .           # Clean cache for current project
    loct cache clean --older-than 30d      # Remove entries older than 30 days
    loct cache prune --max-size 1GB --force # Agent-safe ENOSPC recovery";

pub(super) const ENV_TRUTH_HELP: &str =
    "loct env-truth - Audit env declaration drift (Cut 8 / Lane 4)

USAGE:
    loct env-truth [OPTIONS] [PATHS...]

DESCRIPTION:
    Surfaces every env-variable declaration site (dotenv, dockerfile,
    docker-compose, k8s manifests, helm values, GitHub Actions, npm
    scripts, sops-encrypted markers) and cross-references the read side
    already produced by Cut 3B (`semantic_facts.env_contracts`). Emits
    drift warnings (stale-overrides-fresh, multi-source-mismatch,
    orphan-code-reference, sealed-suspected-stale) and blocks pipelines
    via `--fail-on`.

    Encrypted/sealed payloads are surfaced by **format markers only** —
    SealedSecret / SOPS / `ENC[AES256_GCM,...]` blobs are NEVER decoded,
    even when local keys could in principle do it.

OPTIONS:
    --json                       Emit JSON manifest (default is Markdown)
    --md, --markdown             Emit Markdown explicitly (default)
    --all                        Full per-declaration dump (default: Top-problems view)
    --hashes, --show-hashes      Show sha256 value hashes (hidden by default)
    --name <NAME>                Deep-dive on a single env var
    --no-orphans                 Suppress orphan code references
    --include-orphans            Include orphan code references (default)
    --stale-threshold-days <N>   Days before stale-overrides-fresh fires (default: 7)
    --paths <P1,P2,...>          Restrict scan to listed paths (e.g. k8s/,deploy/)
    --fail-on <KIND>             CI gate; exit 2 on first matching warning
    --help, -h                   Show this help message

FAIL-ON KINDS:
    stale-sealed-overrides-fresh-plain   The example-app pattern: stale SealedSecret/
                                         SOPS overrides a fresh dotenv/configmap.
    stale-overrides-fresh                Generic: highest-precedence is older
                                         than runner-up by stale-threshold-days.
    multi-source-mismatch                Two or more `Plain` sources disagree
                                         on value hash.
    orphan-code-reference                Code reads an env var that has zero
                                         declarations.
    orphan-declaration                   Declaration exists but no code reads it
                                         (only fires when Cut 3B env_contracts
                                         is non-empty).
    any                                  Trip on any warning.

EXAMPLES:
    loct env-truth                                     # Top-problems markdown report
    loct env-truth --all                               # Full per-declaration dump
    loct env-truth --json | jq '.declarations | length'
    loct env-truth --name DATABASE_URL --json
    loct env-truth --paths k8s/,deploy/
    loct env-truth --fail-on stale-sealed-overrides-fresh-plain  # CI gate
    loct env-truth --all --md > ENV.md                 # Operator runbook

OUTPUT FORMAT:
    JSON: { schema_version, generated_at, roots, declarations[],
            orphan_reads[], template_drift[], summary }
    Markdown (default): H1 report → summary → Top problems (real conflicts)
              → template drift → capped orphan lists.
    Markdown (--all): full per-declaration H3 with sources table and
              warning bullets, plus the active precedence table.

PRECEDENCE OVERRIDE:
    Default ranks live in `docs/env-truth-precedence.md`. Override per repo
    with:
        # .loctree/config.toml
        [env_truth]
        precedence = { dot_env = 99, sealed_secret = 50 }

RELATED COMMANDS:
    loct findings    Full structural findings; env-truth is a separate channel
    loct doctor      Cache/scope diagnostics (similar UX/exit-code shape)";
