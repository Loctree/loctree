//! Per-command option structs for all CLI commands.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::path::PathBuf;

/// Options for the `auto` command (default behavior).
#[derive(Debug, Clone, Default)]
pub struct AutoOptions {
    /// Root directories to scan (defaults to current directory)
    pub roots: Vec<PathBuf>,

    /// Force full rescan ignoring mtime cache
    pub full_scan: bool,

    /// Include normally-ignored directories (node_modules, target, .venv)
    pub scan_all: bool,

    /// Generate AI agent feed report (ForAi mode)
    pub for_agent_feed: bool,

    /// Emit single-shot agent JSON bundle (vs JSONL stream)
    pub agent_json: bool,

    /// Suppress duplicate export output (noise reduction)
    pub suppress_duplicates: bool,

    /// Suppress dynamic imports output (noise reduction)
    pub suppress_dynamic: bool,
}

/// Options for the `scan` command.
#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Root directories to scan
    pub roots: Vec<PathBuf>,

    /// Force full rescan ignoring mtime cache
    pub full_scan: bool,

    /// Include normally-ignored directories
    pub scan_all: bool,

    /// Watch for file changes and re-scan automatically
    pub watch: bool,

    /// SIGTERM the existing `--watch` holder (if any) and take over the lock.
    pub replace: bool,

    /// Block until the lock is free, with a deadline in seconds.
    pub wait_seconds: Option<u64>,

    /// Block indefinitely until the lock is free (`--wait` without a value).
    pub wait_indefinite: bool,
}

/// Which co-process surface to bring up alongside the watch loop.
///
/// `Dev` is the default, equivalent to today's `loct scan --watch` (foreground
/// watcher only) but with the single-instance lock enforced. The remaining
/// variants are the new shape requested in the Loctree scan-watch plan and
/// vary in maturity:
///
/// * `Bg`  — daemonize: re-spawn `loct watch --dev` detached with logs
///   redirected to `.loctree/watch.log`. **Shipping.**
/// * `Lsp` — co-spawn `loctree-lsp` alongside the watch loop. **Shipping.**
/// * `Http` / `Report` — deferred until the corresponding transports land
///   in `loctree-mcp` / `reports` respectively. Selecting them today emits
///   a clear "not yet implemented" notice and exits with code 2 rather
///   than fake-succeeding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WatchMode {
    /// Foreground watch loop (default). Equivalent to legacy `loct scan --watch`.
    #[default]
    Dev,
    /// Daemonize via detached child process. Returns immediately after spawn.
    Bg,
    /// Spawn `loctree-lsp` as a sibling child alongside the watch loop.
    Lsp,
    /// **Deferred**: streamable-http MCP co-process.
    Http,
    /// **Deferred**: SSR report server co-process.
    Report,
}

/// Options for the `watch` subcommand (the new shape of `scan --watch`).
#[derive(Debug, Clone, Default)]
pub struct WatchOptions {
    /// Root directories to watch.
    pub roots: Vec<PathBuf>,
    /// Which co-process surface to start (see [`WatchMode`]).
    pub mode: WatchMode,

    /// Force full rescan ignoring mtime cache.
    pub full_scan: bool,
    /// Include normally-ignored directories.
    pub scan_all: bool,

    /// SIGTERM the existing holder and take over the lock.
    pub replace: bool,
    /// Block until the lock is free, with deadline in seconds.
    pub wait_seconds: Option<u64>,
    /// Block indefinitely until the lock is free (`--wait` without value).
    pub wait_indefinite: bool,

    /// Port for `--report` (HTML report HTTP server, default 5075) and
    /// `--http` (streamable-http MCP server, default 5174). Ignored for
    /// other modes.
    pub port: Option<u16>,
}

/// Options for the `tree` command.
#[derive(Debug, Clone, Default)]
pub struct TreeOptions {
    /// Root directories to display
    pub roots: Vec<PathBuf>,

    /// Maximum depth of tree recursion
    pub depth: Option<usize>,

    /// Show summary with top N large files
    pub summary: Option<usize>,

    /// Suppress full tree output, show top list only
    pub summary_only: bool,

    /// LOC threshold for highlighting large files
    pub loc_threshold: Option<usize>,

    /// Include hidden files (dotfiles)
    pub show_hidden: bool,

    /// Find build artifacts (node_modules, target, .venv)
    pub find_artifacts: bool,

    /// Show gitignored files
    pub show_ignored: bool,

    /// Emit matching file paths only, one path per line
    pub files_only: bool,

    /// Regex filter applied to relative output paths
    pub path_filter: Option<String>,
}

/// Options for the `slice` command.
#[derive(Debug, Clone)]
pub struct SliceOptions {
    /// Target file path for the slice
    pub target: String,

    /// Root directory (defaults to current directory)
    pub root: Option<PathBuf>,

    /// Include consumer files (files that import the target)
    pub consumers: bool,

    /// Maximum depth for dependency traversal
    pub depth: Option<usize>,

    /// Force rescan before slicing (includes uncommitted files)
    pub rescan: bool,
}

impl Default for SliceOptions {
    fn default() -> Self {
        Self {
            target: String::new(),
            root: None,
            consumers: true,
            depth: None,
            rescan: false,
        }
    }
}

/// Options for the `find` command.
///
/// Supports regex filtering on metadata fields for AI agent queries.
#[derive(Debug, Clone, Default)]
pub struct FindOptions {
    /// Search query (can be regex pattern)
    pub query: Option<String>,

    /// Positional query args (preserved as provided; used for split/AND modes in CLI).
    ///
    /// Examples:
    /// - `loct find Foo Bar` => queries: ["Foo", "Bar"]
    /// - `loct find "Foo Bar"` => queries: ["Foo Bar"] (single arg containing whitespace)
    pub queries: Vec<String>,

    /// Force legacy OR behavior for multi-arg queries (combine with `|`).
    pub or_mode: bool,

    /// Filter by symbol name (regex supported)
    pub symbol: Option<String>,

    /// Filter by file path (regex supported in symbol mode; exact path/suffix
    /// scope in literal mode)
    pub file: Option<String>,

    /// Find files impacted by changes to this file
    pub impact: Option<String>,

    /// Find similar symbols (fuzzy matching)
    pub similar: Option<String>,

    /// Literal truth mode: scan raw source bytes for exact identifier-boundary
    /// occurrences (the W1-A occurrences substrate) instead of AST/fuzzy search.
    /// Primary results are literal only; any fuzzy suggestions stay in a
    /// separate, explicitly-labeled section and are never promoted as matches.
    pub literal: bool,

    /// Regex truth mode: scan raw file TEXT (not just identifier tokens) for a
    /// regex pattern, keeping loct's artifact-fence coverage accounting and
    /// per-hit context labels (comment / string_literal / code). For
    /// security/privacy audits where `--literal` (exact string) cannot evaluate
    /// a pattern and grep/sed give no coverage accounting. Mutually exclusive
    /// with `--literal`.
    pub regex: bool,

    /// Filter to dead code only
    pub dead_only: bool,

    /// Filter to exported symbols only
    pub exported_only: bool,

    /// Programming language filter
    pub lang: Option<String>,

    /// Maximum results to return (default: 200)
    pub limit: Option<usize>,

    /// Literal-mode: zero-based occurrence offset for paged output. Ignored
    /// outside `--literal`.
    pub offset: usize,

    /// Literal-mode: treat `-` as token-internal so `backdrop` does not match
    /// inside `overlay-backdrop` / `--vista-z-overlay-backdrop`. Opt-in; the
    /// default boundary is unchanged. Ignored outside `--literal`.
    pub whole_token: bool,

    /// Literal-mode: attach a per-file occurrence rollup (`by_file`).
    pub group_by_file: bool,

    /// Literal-mode: suppress the full occurrence list, keep only counters
    /// (`slim`). Ignored outside `--literal`.
    pub count_only: bool,

    /// Find where a symbol is defined/exported
    pub where_symbol: bool,
}

/// Options for the `occurrences` command — literal exact-identifier scan.
///
/// Unlike `find`, this never consults the AST/tagmap and never promotes a
/// fuzzy suggestion. It walks raw snapshot file bytes and reports every
/// identifier-boundary occurrence so that "not found" means not found.
#[derive(Debug, Clone, Default)]
pub struct OccurrencesOptions {
    /// The exact identifier to scan for.
    pub ident: String,

    /// Root directories to scan (default: current directory).
    pub roots: Vec<PathBuf>,

    /// Treat `-` as token-internal (tighter boundary) so `backdrop` does not
    /// match inside `overlay-backdrop` / `--vista-z-overlay-backdrop`. Opt-in;
    /// the default boundary is unchanged.
    pub whole_token: bool,

    /// Attach a per-file occurrence rollup (`by_file`).
    pub group_by_file: bool,

    /// Suppress the full occurrence list, keep only counters (`slim`).
    pub count_only: bool,

    /// Emit terse human output: `path:line context` per occurrence.
    pub compact: bool,

    /// Zero-based occurrence offset for paged output.
    pub offset: usize,

    /// Maximum number of occurrences to return in the current page.
    pub limit: Option<usize>,
}

/// Options for the `findings` command.
#[derive(Debug, Clone, Default)]
pub struct FindingsOptions {
    /// Root directories to analyze
    pub roots: Vec<PathBuf>,

    /// Emit summary-only JSON instead of the full findings artifact
    pub summary: bool,
}

/// Options for the `dead` command.
#[derive(Debug, Clone, Default)]
pub struct DeadOptions {
    /// Root directories to analyze
    pub roots: Vec<PathBuf>,

    /// Confidence level filter (high, medium, low)
    pub confidence: Option<String>,

    /// Maximum number of dead symbols to report
    pub top: Option<usize>,

    /// Show full list (no top limit)
    pub full: bool,

    /// Filter by file path pattern (regex)
    pub path_filter: Option<String>,

    /// Include tests in dead-export detection (default: false)
    pub with_tests: bool,

    /// Include helper/scripts/docs files (default: false)
    pub with_helpers: bool,

    /// Detect shadow exports (same symbol exported by multiple files, only one used)
    pub with_shadows: bool,

    /// Include ambient declarations (declare global/module/namespace) in analysis.
    /// By default these are excluded as they're consumed by TypeScript compiler, not imports.
    pub with_ambient: bool,

    /// Include dynamically generated symbols (exec/eval/compile templates) in analysis.
    /// By default these are excluded as they're generated at runtime, not actual dead code.
    pub with_dynamic: bool,
}

/// Options for the `cycles` command.
#[derive(Debug, Clone, Default)]
pub struct CyclesOptions {
    /// Root directories to analyze
    pub roots: Vec<PathBuf>,

    /// Filter by file path pattern (regex)
    pub path_filter: Option<String>,

    /// Only show cycles that would break compilation
    pub breaking_only: bool,

    /// Show detailed explanation for each cycle
    pub explain: bool,

    /// Use legacy output format (for backwards compatibility)
    pub legacy_format: bool,

    /// Disable the artifact fence (report fixture/vendored cycles in the main section)
    pub include_artifacts: bool,
}

/// Options for the `trace` command (Tauri/IPC handler tracing).
#[derive(Debug, Clone, Default)]
pub struct TraceOptions {
    /// Handler name to trace
    pub handler: String,

    /// Root directories to analyze
    pub roots: Vec<PathBuf>,
}

/// Options for the `commands` command (Tauri command bridges).
#[derive(Debug, Clone, Default)]
pub struct CommandsOptions {
    /// Root directories to analyze
    pub roots: Vec<PathBuf>,

    /// Filter by command name (regex)
    pub name_filter: Option<String>,

    /// Show only commands with missing handlers
    pub missing_only: bool,

    /// Show only commands with missing frontend invocations
    pub unused_only: bool,

    /// Suppress duplicate sections (noise reduction)
    pub suppress_duplicates: bool,

    /// Suppress dynamic import sections (noise reduction)
    pub suppress_dynamic: bool,

    /// Maximum number of results to show (for limiting large outputs)
    pub limit: Option<usize>,
}

/// Options for the `coverage` command (test coverage analysis).
#[derive(Debug, Clone, Default)]
pub struct CoverageOptions {
    /// Root directories to analyze
    pub roots: Vec<PathBuf>,

    /// Show only handler coverage gaps
    pub handlers_only: bool,

    /// Show only event coverage gaps
    pub events_only: bool,

    /// Filter by minimum severity (critical/high/medium/low)
    pub min_severity: Option<String>,

    /// Include structural test coverage report
    pub tests: bool,

    /// Include gap analysis (handlers/events without tests)
    pub gaps: bool,

    /// Disable the artifact fence (include vendored/fixture/generated/template findings)
    pub include_artifacts: bool,
}

/// Options for the `repo-view` command.
#[derive(Debug, Clone, Default)]
pub struct RepoViewOptions {
    /// Project root to analyze (defaults to current directory).
    pub project: Option<PathBuf>,
}

/// Options for the `follow` command.
#[derive(Debug, Clone)]
pub struct FollowOptions {
    /// Scope to follow: dead/cycles/twins/hotspots/trace/commands/events/pipelines/all.
    pub scope: String,

    /// Optional handler name for trace scope.
    pub handler: Option<String>,

    /// Optional result limit.
    pub limit: Option<usize>,

    /// Root directories to analyze.
    pub roots: Vec<PathBuf>,
}

impl Default for FollowOptions {
    fn default() -> Self {
        Self {
            scope: "all".to_string(),
            handler: None,
            limit: None,
            roots: Vec::new(),
        }
    }
}

/// Options for the `routes` command.
#[derive(Debug, Clone, Default)]
pub struct RoutesOptions {
    /// Root directories to analyze
    pub roots: Vec<PathBuf>,

    /// Filter by framework label (fastapi/flask)
    pub framework: Option<String>,

    /// Filter by route path substring
    pub path_filter: Option<String>,
}

/// Options for the `events` command.
#[derive(Debug, Clone, Default)]
pub struct EventsOptions {
    /// Root directories to analyze
    pub roots: Vec<PathBuf>,

    /// Show ghost events (emitted but not handled)
    pub ghost: bool,

    /// Show orphan handlers (handlers without emitters)
    pub orphan: bool,

    /// Show potential race conditions
    pub races: bool,

    /// Show only FE<->FE sync events (window sync pattern)
    pub fe_sync: bool,

    /// Suppress duplicate sections (noise reduction)
    pub suppress_duplicates: bool,

    /// Suppress dynamic import sections (noise reduction)
    pub suppress_dynamic: bool,

    /// Disable the artifact fence (include event bridges from vendored/generated files)
    pub include_artifacts: bool,
}

/// Options for the `pipelines` command.
#[derive(Debug, Clone, Default)]
pub struct PipelinesOptions {
    /// Root directories to analyze
    pub roots: Vec<PathBuf>,
}

/// Options for the `insights` command.
#[derive(Debug, Clone, Default)]
pub struct InsightsOptions {
    /// Root directories to analyze
    pub roots: Vec<PathBuf>,
}

/// Options for the `manifests` command.
#[derive(Debug, Clone, Default)]
pub struct ManifestsOptions {
    /// Root directories to analyze
    pub roots: Vec<PathBuf>,
}

/// Options for the `info` command.
#[derive(Debug, Clone, Default)]
pub struct InfoOptions {
    /// Root directory to check
    pub root: Option<PathBuf>,
}

/// Options for the `lint` command.
#[derive(Debug, Clone, Default)]
pub struct LintOptions {
    /// Root directories to lint
    pub roots: Vec<PathBuf>,

    /// Check entrypoint coverage
    pub entrypoints: bool,

    /// Fail with non-zero exit code on issues
    pub fail: bool,

    /// Output in SARIF format for CI integration
    pub sarif: bool,

    /// Enable Tauri-specific checks
    pub tauri: bool,

    /// Enable deep lint checks (ts/react/memory)
    pub deep: bool,

    /// Include TypeScript lint checks
    pub ts: bool,

    /// Include React lint checks
    pub react: bool,

    /// Include memory leak lint checks
    pub memory: bool,

    /// Suppress duplicate sections (noise reduction)
    pub suppress_duplicates: bool,

    /// Suppress dynamic import sections (noise reduction)
    pub suppress_dynamic: bool,
}

/// Options for the `report` command.
#[derive(Debug, Clone, Default)]
pub struct ReportOptions {
    /// Root directories to report on
    pub roots: Vec<PathBuf>,

    /// Output file path
    pub output: Option<PathBuf>,

    /// Start a local server to view the report
    pub serve: bool,

    /// Server port
    pub port: Option<u16>,

    /// Editor integration (code, cursor, windsurf, jetbrains)
    pub editor: Option<String>,
}

/// Options for the `prism` command.
/// Compares context packs across task framings and scores conceptual smear.
#[derive(Debug, Clone)]
pub struct PrismOptions {
    /// Task framings to compare. Pass at least two.
    pub tasks: Vec<String>,

    /// Project root for identity and snapshot scope.
    pub project: Option<PathBuf>,

    /// Operator override for the AICX project bucket.
    pub aicx_project_override: Option<String>,

    /// Include AICX memory overlay.
    pub with_aicx: bool,

    /// Disable AICX memory overlay.
    pub no_aicx: bool,

    /// Emit JSON report.
    pub json: bool,

    /// Maximum example items per section.
    pub limit: usize,
}

impl Default for PrismOptions {
    fn default() -> Self {
        Self {
            tasks: Vec::new(),
            project: None,
            aicx_project_override: None,
            with_aicx: true,
            no_aicx: false,
            json: false,
            limit: 8,
        }
    }
}

/// Options for the `diff` command.
#[derive(Debug, Clone, Default)]
pub struct DiffOptions {
    /// Snapshot ID or path to compare against (from)
    pub since: Option<String>,

    /// Second snapshot ID or path (to). If omitted, compare against current state
    pub to: Option<String>,

    /// Output as JSONL (one line per change)
    pub jsonl: bool,

    /// Show only the changed-file summary between --since and HEAD
    pub changed_files: bool,

    /// Show only new problems (added dead exports, new cycles, new missing handlers)
    pub problems_only: bool,

    /// Automatically scan target branch using git worktree (zero-friction diff)
    pub auto_scan_base: bool,

    /// Disable the artifact fence (include exports from generated/vendored files)
    pub include_artifacts: bool,
}

/// Options for the `crowd` command.
/// Detects functional crowds - multiple files clustering around same functionality.
#[derive(Debug, Clone, Default)]
pub struct CrowdOptions {
    /// Pattern to detect crowd around (e.g., "message", "patient", "auth")
    pub pattern: Option<String>,

    /// Root directories to analyze
    pub roots: Vec<PathBuf>,

    /// Detect all crowds automatically (if no pattern specified)
    pub auto_detect: bool,

    /// Minimum crowd size to report (default: 2)
    pub min_size: Option<usize>,

    /// Maximum crowds to show in auto-detect mode (default: 10)
    pub limit: Option<usize>,

    /// Include test files in crowd detection (default: false)
    /// Tests are entry points by design - they have 0 importers and create noise
    pub include_tests: bool,
}

/// Options for the `tagmap` command.
/// Unified search aggregating files, crowds, and dead code around a keyword.
#[derive(Debug, Clone, Default)]
pub struct TagmapOptions {
    /// Keyword to search for (in paths and names)
    pub keyword: String,

    /// Root directories to analyze
    pub roots: Vec<PathBuf>,

    /// Include test files in analysis (default: false)
    pub include_tests: bool,

    /// Maximum results to show per section
    pub limit: Option<usize>,
}

/// Options for the `twins` command.
/// Shows symbol registry and dead parrots (0 import count).
#[derive(Debug, Clone, Default)]
pub struct TwinsOptions {
    /// Root directory to analyze (defaults to current directory)
    pub path: Option<PathBuf>,

    /// Show only dead parrots (symbols with 0 imports)
    pub dead_only: bool,

    /// Include suppressed findings in output
    pub include_suppressed: bool,

    /// Include test files in analysis (default: false)
    pub include_tests: bool,

    /// Ignore framework conventions when detecting twins.
    /// By default, framework-specific patterns (e.g., Django mixins) are filtered.
    pub ignore_conventions: bool,
}

/// Options for the `suppressions` command — source-side silencer inventory.
///
/// Surfaces every `#[allow(...)]`, `#[ignore]`, `unsafe { ... }`,
/// `// nosemgrep`, `@ts-ignore`, `eslint-disable`, `# noqa`, `# type: ignore`,
/// `# shellcheck disable`, etc. detected literally in the working tree.
///
/// **Distinct** from `SuppressOptions` (which manages loctree's own
/// finding-suppression file `.loctree/suppressions.toml`). The name collision
/// is intentional for ergonomics but the surfaces are unrelated.
///
/// LITERAL-ONLY (free-tier scope). Semantic enrichment (suspicious/stale
/// classification) is paid-tier Wave 7+ — see
/// `analyzer::suppression_inventory` module docs for the tier boundary.
#[derive(Debug, Clone, Default)]
pub struct SuppressionsOptions {
    /// Root directory to scan (defaults to current directory).
    pub root: Option<PathBuf>,
    /// Filter to specific kinds (repeatable). Empty = all kinds.
    /// Tokens: `allow`, `dead-code`, `nosemgrep`, `ts-ignore`,
    /// `ts-expect-error`, `ts-nocheck`, `eslint-disable`, `noqa`,
    /// `type-ignore`, `pylint-disable`, `mypy-ignore`, `shellcheck`,
    /// `unsafe`, `unsafe-env-var`, `ignore`.
    pub kinds: Vec<String>,
    /// Print summary-only table (default if no other output mode).
    pub summary: bool,
    /// Emit full structured JSON output.
    pub json: bool,
    /// Include paths normally excluded by `.semgrepignore`
    /// (e.g. fixtures, vendored tests). Default OFF.
    pub include_fixtures: bool,
}

/// Options for the `suppress` command.
/// Manage false positive suppressions.
#[derive(Debug, Clone, Default)]
pub struct SuppressOptions {
    /// Root directory (defaults to current directory)
    pub path: Option<PathBuf>,

    /// Type of finding to suppress: twins, dead_parrot, dead_export, circular
    pub suppression_type: Option<String>,

    /// Symbol name to suppress
    pub symbol: Option<String>,

    /// Optional: specific file path
    pub file: Option<String>,

    /// Reason for suppression
    pub reason: Option<String>,

    /// List all current suppressions
    pub list: bool,

    /// Clear all suppressions
    pub clear: bool,

    /// Remove a specific suppression
    pub remove: bool,
}

/// Options for the `dist` command.
/// Analyzes bundle distribution using source maps.
#[derive(Debug, Clone, Default)]
pub struct DistOptions {
    /// Source map inputs (.map files or directories to auto-discover under)
    pub source_maps: Vec<PathBuf>,

    /// Source directory to scan for exports
    pub src: Option<PathBuf>,

    /// Optional path to write the JSON report
    pub report_path: Option<PathBuf>,
}

/// Options for the `sniff` command.
/// Aggregates code smell findings (twins, dead parrots, crowds).
#[derive(Debug, Clone, Default)]
pub struct SniffOptions {
    /// Root directory to analyze (defaults to current directory)
    pub path: Option<PathBuf>,

    /// Show only dead parrots (skip twins and crowds)
    pub dead_only: bool,

    /// Show only twins (skip dead parrots and crowds)
    pub twins_only: bool,

    /// Show only crowds (skip twins and dead parrots)
    pub crowds_only: bool,

    /// Include test files in analysis (default: false)
    pub include_tests: bool,

    /// Minimum crowd size to report (default: 2)
    pub min_crowd_size: Option<usize>,
}

/// Options for jq-style query mode (loct '.filter')
#[derive(Debug, Clone, Default)]
pub struct JqQueryOptions {
    /// The jq filter expression
    pub filter: String,
    /// Raw string output (-r)
    pub raw_output: bool,
    /// Compact JSON output (-c)
    pub compact_output: bool,
    /// Exit status mode (-e)
    pub exit_status: bool,
    /// String variable bindings: (name, value)
    pub string_args: Vec<(String, String)>,
    /// JSON variable bindings: (name, json_string)
    pub json_args: Vec<(String, String)>,
    /// Explicit snapshot path
    pub snapshot_path: Option<PathBuf>,
}

/// Options for the `impact` command.
#[derive(Debug, Clone, Default)]
pub struct ImpactCommandOptions {
    /// Target file path to analyze
    pub target: String,

    /// Maximum traversal depth (None = unlimited)
    pub depth: Option<usize>,

    /// Root directory (defaults to current directory)
    pub root: Option<PathBuf>,
}

/// Options for the `focus` command.
/// Focus on a directory - like slice but for directories.
#[derive(Debug, Clone)]
pub struct FocusOptions {
    /// Target directory path
    pub target: String,

    /// Root directory (defaults to current directory)
    pub root: Option<PathBuf>,

    /// Include consumer files (files outside the directory that import it)
    pub consumers: bool,

    /// Maximum depth for external dependency traversal
    pub depth: Option<usize>,

    /// Emit JSON output (parity with MCP --format and other commands).
    pub json: bool,

    /// Emit Markdown output (parity with MCP --format and other commands).
    pub markdown: bool,
}

impl Default for FocusOptions {
    fn default() -> Self {
        Self {
            target: String::new(),
            root: None,
            consumers: true,
            depth: None,
            json: false,
            markdown: false,
        }
    }
}

/// Options for the `hotspots` command.
/// Shows import frequency heatmap - which files are core vs peripheral.
#[derive(Debug, Clone, Default)]
pub struct HotspotsOptions {
    /// Root directory (defaults to current directory)
    pub root: Option<PathBuf>,

    /// Minimum import count to show (default: 1)
    pub min_imports: Option<usize>,

    /// Maximum files to show (default: 50)
    pub limit: Option<usize>,

    /// Show only files with zero importers (leaf nodes)
    pub leaves_only: bool,

    /// Show coupling score (out-degree / files that import many others)
    pub coupling: bool,
}

/// Options for the `layoutmap` command.
/// Analyze CSS layout properties (z-index, position, display).
#[derive(Debug, Clone, Default)]
pub struct LayoutmapOptions {
    /// Root directory (defaults to current directory)
    pub root: Option<PathBuf>,

    /// Show only z-index values
    pub zindex_only: bool,

    /// Show only sticky/fixed position elements
    pub sticky_only: bool,

    /// Show only grid/flex layouts
    pub grid_only: bool,

    /// Minimum z-index threshold to report (default: 1)
    pub min_zindex: Option<i32>,

    /// Glob patterns to exclude (e.g., "**/prototype/**", "**/.obsidian/**")
    pub exclude: Vec<String>,
}

/// Options for the `zombie` command.
/// Find all zombie code (dead exports + orphan files + shadows).
#[derive(Debug, Clone, Default)]
pub struct ZombieOptions {
    /// Root directories to analyze
    pub roots: Vec<PathBuf>,

    /// Include test files in zombie detection (default: false)
    pub include_tests: bool,
}

/// Options for the `health` command.
/// Quick health check summary (cycles + dead + twins).
#[derive(Debug, Clone, Default)]
pub struct HealthOptions {
    /// Root directories to analyze
    pub roots: Vec<PathBuf>,

    /// Include test files in analysis (default: false)
    pub include_tests: bool,
}

/// Options for the `audit` command.
/// Full audit combining all structural analyses into one actionable markdown report.
#[derive(Debug, Clone, Default)]
pub struct AuditOptions {
    /// Root directories to analyze
    pub roots: Vec<PathBuf>,

    /// Include test files in analysis (default: false)
    pub include_tests: bool,

    /// Output as actionable todo checklist (default: false)
    pub todos: bool,

    /// Optional maximum items per category; unset means full report
    pub limit: Option<usize>,

    /// Don't auto-open the report file (default: false)
    pub no_open: bool,
}
/// Options for the `doctor` command.
/// Operator-facing diagnostics for cache identity and snapshot scope.
#[derive(Debug, Clone, Default)]
pub struct DoctorOptions {
    /// Inspect cache identity and latest scan metadata
    pub cache: bool,

    /// Validate cache/snapshot scope (filled in by Cut 2 T1)
    pub scope: bool,

    /// List cached projects (default if no other mode is selected)
    pub list: bool,

    /// Emit a JSON doctor report
    pub json: bool,

    /// Fix cache/scope problems (filled in by Cut 2 T2)
    pub fix: bool,

    /// Skip interactive confirmation for fix mode
    pub yes: bool,

    /// Limit diagnostics to one project path
    pub project: Option<PathBuf>,

    /// Root directories to analyze
    pub roots: Vec<PathBuf>,

    /// Include test files in analysis (default: false)
    pub include_tests: bool,

    /// Automatically apply suggested suppressions to .loctignore
    pub apply_suppressions: bool,
}

/// Options for the `plan` command.
/// Generate architectural refactoring plan based on module analysis.
#[derive(Debug, Clone, Default)]
pub struct PlanOptions {
    /// Root directories/files to analyze
    pub roots: Vec<PathBuf>,

    /// Custom target layout mapping (e.g., "core=src/kernel,ui=src/components")
    pub target_layout: Option<String>,

    /// Output as markdown (default)
    pub markdown: bool,

    /// Output as JSON
    pub json: bool,

    /// Output as executable shell script
    pub script: bool,

    /// Generate all formats (.md, .json, .sh)
    pub all: bool,

    /// Minimum coupling score to include (0.0-1.0)
    pub min_coupling: Option<f64>,

    /// Maximum module size in LOC before suggesting split
    pub max_module_size: Option<usize>,

    /// Include test files in analysis (default: false)
    pub include_tests: bool,

    /// Output file path (without extension for --all)
    pub output: Option<PathBuf>,

    /// Don't auto-open the generated report
    pub no_open: bool,
}

/// Options for the `help` command.
#[derive(Debug, Clone, Default)]
pub struct HelpOptions {
    /// Show help for a specific command
    pub command: Option<String>,

    /// Show legacy flag documentation
    pub legacy: bool,

    /// Show full help (new + legacy)
    pub full: bool,
}

/// Cache subcommand action.
#[derive(Debug, Clone)]
pub enum CacheAction {
    /// List all cached projects with sizes and ages
    List,
    /// Clean cache: all projects, or a specific one, or stale entries
    Clean {
        /// Only clean cache for a specific project directory
        project: Option<PathBuf>,
        /// Only clean entries older than this duration (e.g., "7d", "30d")
        older_than: Option<String>,
        /// Cap total cache size; evict oldest buckets until the remainder
        /// fits the budget (e.g., "1GB", "500MB", or plain bytes).
        /// Source hak: 2026-05-23 div0 system-cleanup (16.6 GB cache).
        max_size: Option<String>,
        /// Skip confirmation prompt
        force: bool,
    },
}

/// Options for the `cache` command.
#[derive(Debug, Clone)]
pub struct CacheOptions {
    pub action: CacheAction,
}

/// Options for the `env-truth` command (Cut 8 / Lane 4).
///
/// Surfaces every env-var declaration site with precedence and freshness
/// metadata, cross-references reads from Cut 3B `semantic_facts`, and
/// emits drift warnings (stale-overrides-fresh, multi-source-mismatch,
/// orphan-code-reference, sealed-suspected-stale).
#[derive(Debug, Clone, Default)]
pub struct EnvTruthOptions {
    /// Scan roots (defaults to current dir).
    pub roots: Vec<PathBuf>,

    /// Optional path-restriction set (relative to root): `--paths k8s/,deploy/`.
    pub restricted_paths: Vec<PathBuf>,

    /// Emit JSON instead of Markdown.
    pub json: bool,

    /// Emit Markdown explicitly (default human output is Markdown anyway,
    /// kept for symmetry and to allow `loct env-truth --md > ENV.md`).
    pub markdown: bool,

    /// Filter to one env name (deep-dive view).
    pub name: Option<String>,

    /// Include orphan code references in output (default: true).
    pub include_orphans: bool,

    /// Suppress orphan code references entirely.
    pub no_orphans: bool,

    /// Days threshold for stale-overrides-fresh warning (default 7).
    pub stale_threshold_days: Option<u32>,

    /// CI gate: exit 2 on the first warning matching this kind. Multiple
    /// invocations OR together.
    pub fail_on: Vec<String>,

    /// Full per-declaration Markdown dump. Default is the "Top problems"
    /// view (real conflicts + template drift + capped orphan lists).
    pub all: bool,

    /// Show `sha256:` value hashes in Markdown output (hidden by default).
    pub show_hashes: bool,
}

/// Query kind for the `query` command.
#[derive(Debug, Clone)]
pub enum QueryKind {
    /// Find files that import a given file
    WhoImports,
    /// Find where a symbol is defined
    WhereSymbol,
    /// Show what component a file belongs to
    ComponentOf,
    /// Classify Swift type-position references in a source file.
    SwiftTypes,
}

/// Options for the `query` command.
#[derive(Debug, Clone)]
pub struct QueryOptions {
    /// Query kind
    pub kind: QueryKind,

    /// Target (file path or symbol name)
    pub target: String,
}

/// Options for the `body` command — bounded symbol source retrieval.
#[derive(Debug, Clone)]
pub struct BodyOptions {
    /// Symbol name to retrieve the body for.
    pub symbol: String,

    /// Maximum source lines to return per body (None = default cap).
    pub line_cap: Option<usize>,
}

/// Options for `loct prune-old-artifacts` — local `.loctree/` housekeeping.
#[derive(Debug, Clone)]
pub struct PruneOldArtifactsOptions {
    /// Project root to scan (defaults to current directory).
    pub root: Option<PathBuf>,

    /// How many newest per-branch snapshots to keep per `.loctree/` dir.
    pub keep: usize,

    /// Also walk into sub-`.loctree/` directories (e.g. `src-tauri/.loctree/`).
    /// Off by default — sub-loctree pruning is more aggressive.
    pub include_sub: bool,

    /// Actually delete files. Without this flag the command runs as dry-run.
    pub apply: bool,
}

impl Default for PruneOldArtifactsOptions {
    fn default() -> Self {
        Self {
            root: None,
            keep: 3,
            include_sub: false,
            apply: false,
        }
    }
}
