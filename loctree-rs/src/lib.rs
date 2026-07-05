//! # loctree
//!
//! **AI-oriented Project Analyzer** - Static analysis tool designed for AI agents
//! and developers building production-ready software.
//!
//! loctree helps overcome the common AI tendency to generate excessive artifacts
//! that lead to re-export cascades, circular imports, and spaghetti dependencies.
//!
//! ## Features
//!
//! - **Holographic Slice** - Extract focused context (deps + consumers) for any file
//! - **Handler Trace** - Follow Tauri commands through the entire pipeline
//! - **Dead Export Detection** - Find unused exports and orphaned code
//! - **Circular Import Detection** - Catch runtime bombs before they explode
//! - **Auto-Detect Stack** - Automatically configure for Rust, TypeScript, Python, Tauri
//! - **HTML Reports** - Interactive reports with Cytoscape.js dependency graphs
//!
//! ## Quick Start (Library Usage)
//!
//! ```rust,no_run
//! use loctree::{detect, snapshot, slicer};
//! use std::path::PathBuf;
//!
//! // Detect project stack
//! let detected = detect::detect_stack(std::path::Path::new("."));
//! println!("Detected: {}", detected.description);
//! ```
//!
//! ## Running Import Analysis
//!
//! ```rust,no_run
//! use loctree::{analyzer, args};
//! use std::path::PathBuf;
//!
//! // Run the full import analyzer on a project
//! let mut parsed = args::ParsedArgs::default();
//! parsed.dead_exports = true;
//! parsed.circular = true;
//!
//! let roots = vec![PathBuf::from(".")];
//! analyzer::run_import_analyzer(&roots, &parsed).unwrap();
//! ```
//!
//! ## CLI Usage
//!
//! For command-line usage, install with `cargo install loctree` and run:
//!
//! ```bash
//! loct                       # Auto-scan with stack detection
//! loct slice src/App.tsx     # Extract holographic context
//! loct trace get_user        # Trace Tauri handler
//! loct cycles                # Find circular imports
//! loct context               # Brand-defining pill (markdown by default)
//! ```
//!
//! See the [README](https://github.com/Loctree/Loctree) for full documentation.
//!
//! # Public Library API
//!
//! MCP servers, IDE extensions, and other library consumers should import from
//! library-layer modules such as [`pack`], [`atlas`], [`analyzer`], [`query`],
//! and [`snapshot`].
//!
//! `cli::*` modules are CLI-internal and may change without notice. Re-exports
//! exist for backward compatibility, but new non-CLI code should not depend on
//! them.

#![doc(html_root_url = "https://docs.rs/loctree/0.13.0")]
#![doc(html_favicon_url = "https://loct.io/assets/loctree-logo.png")]
#![doc(html_logo_url = "https://loct.io/assets/loctree-logo.png")]

// ============================================================================
// Core Modules
// ============================================================================

/// Import/export analyzer supporting TypeScript, JavaScript, Python, Rust, and CSS.
///
/// # Submodules
///
/// - [`analyzer::js`] - TypeScript/JavaScript analysis
/// - [`analyzer::py`] - Python analysis
/// - [`analyzer::rust`] - Rust analysis (Tauri commands)
/// - [`analyzer::cycles`] - Circular import detection (Tarjan's SCC)
/// - [`analyzer::dead_parrots`] - Dead export detection
/// - [`analyzer::trace`] - Handler tracing for Tauri
/// - [`analyzer::coverage`] - Tauri command coverage
/// - [`analyzer::for_ai`] - AI-optimized output generation
/// - [`analyzer::html`] - HTML report generation
/// - [`analyzer::sarif`] - SARIF 2.1.0 output for CI
pub mod analyzer;
pub mod body;
pub(crate) mod context_render;
pub mod context_scope;
pub(crate) mod context_stack;

/// New CLI module for the subcommand-based interface.
///
/// Provides the canonical `loct <command> [options]` interface with:
/// - [`Command`](cli::Command) enum as the source of truth for all commands
/// - [`GlobalOptions`](cli::GlobalOptions) for shared flags
/// - Per-command option structs
/// - Legacy adapter for backward compatibility (until v1.0)
///
/// # Key Commands (Human Interface)
///
/// - `loct` / `loct auto` - Full auto-scan with stack detection (default)
/// - `loct scan` - Build/update snapshot
/// - `loct dead` - Detect unused exports
/// - `loct commands` - Show Tauri command bridges
/// - `loct events` - Show event flow
/// - `loct slice <path>` - Extract holographic context
///
/// # Agent Interface
///
/// Agents should use `--json` output with regex filters on metadata:
/// - `loct find --symbol '.*patient.*' --lang ts --json`
/// - `loct dead --confidence high --json`
pub mod cli;

/// Public ContextPack composition API shared by CLI, MCP, and library users.
pub mod pack;

/// Public Context Atlas API for non-CLI library consumers.
///
/// MCP servers, LSP integrations, and editor extensions use this module to
/// materialize and consume the navigable Context Atlas without depending on
/// CLI-internal layout. Complementary to [`pack`] (dense in-memory pack) —
/// agents pick the shape that fits.
pub mod atlas;

/// Library-facing report builders for health, findings, audit, and coverage.
pub mod analysis_reports;

/// Command-line argument parsing.
///
/// Contains [`ParsedArgs`](args::ParsedArgs) struct and [`parse_args`](args::parse_args) function.
pub mod args;

/// Configuration file support.
///
/// Loads `.loctree/config.toml` for project-specific settings like custom Tauri command macros.
pub mod config;

/// Suppression system for false positives.
///
/// Allows marking findings as "reviewed and OK" so they don't appear in subsequent runs.
/// Stored in `.loctree/suppressions.toml`.
pub mod suppressions;

/// Auto-detection of project stacks.
///
/// Detects Rust, TypeScript, Python, Tauri, Vite, and more based on marker files.
///
/// # Example
///
/// ```rust,no_run
/// use loctree::detect;
/// use std::path::Path;
///
/// let detected = detect::detect_stack(Path::new("."));
/// if !detected.is_empty() {
///     println!("Stack: {}", detected.description);
///     println!("Extensions: {:?}", detected.extensions);
/// }
/// ```
pub mod detect;

/// Filesystem utilities.
///
/// - Gitignore handling with [`GitIgnoreChecker`](fs_utils::GitIgnoreChecker)
/// - File gathering with extension/depth filters
/// - Line counting
/// - Pattern normalization
pub mod fs_utils;

/// String similarity using Levenshtein distance.
///
/// Used for fuzzy matching in `--check` mode to find similar component names.
pub mod similarity;

/// Holographic slice extraction.
///
/// Extracts a file's context in three layers:
/// - **Core** - The target file itself
/// - **Deps** - Files the target imports (transitive)
/// - **Consumers** - Files that import the target
///
/// # Example
///
/// ```rust,no_run
/// use loctree::slicer;
/// use loctree::args::ParsedArgs;
/// use std::path::Path;
///
/// let parsed = ParsedArgs::default();
/// let root = Path::new(".");
///
/// // Extract slice for src/App.tsx with consumers, as JSON
/// slicer::run_slice(root, "src/App.tsx", true, true, &parsed).unwrap();
/// ```
pub mod slicer;

/// Directory-level holographic focus.
///
/// Like slicer but for directories instead of single files.
pub mod focuser;

/// CSS Layout Analysis.
///
/// Scans CSS/SCSS files for layout-related properties:
/// z-index, position: sticky/fixed, display: grid/flex.
pub mod layoutmap;

/// Incremental snapshot persistence.
///
/// Saves analysis results to cached artifacts for faster subsequent runs.
/// By default, artifacts live in the OS user cache dir (override via `LOCT_CACHE_DIR`).
/// Uses file modification times to skip unchanged files.
///
/// # Key Types
///
/// - [`Snapshot`](snapshot::Snapshot) - The persisted analysis state
/// - [`SnapshotMetadata`](snapshot::SnapshotMetadata) - Version and timestamp info
/// - [`GraphEdge`](snapshot::GraphEdge) - Import relationship
/// - [`CommandBridge`](snapshot::CommandBridge) - FE→BE command mapping
pub mod snapshot;

/// Directory tree with LOC counts.
///
/// Fast tree view similar to Unix `tree` command but with:
/// - Line counts per file
/// - Large file highlighting
/// - Gitignore support
/// - Build artifact detection (`--find-artifacts`)
pub mod tree;

/// Common types used throughout the crate.
///
/// # Key Types
///
/// - [`Mode`] - CLI mode (Tree, Slice, Trace, AnalyzeImports, ForAi, Git)
/// - [`Options`] - Analysis configuration
/// - [`FileAnalysis`] - Per-file analysis result
/// - `ImportEntry` - Import statement representation
/// - `ExportSymbol` - Export declaration
/// - `CommandRef` - Tauri command reference
pub mod types;

/// Terminal color utilities for CLI output.
///
/// Provides ANSI color codes and semantic helpers for consistent
/// colorized output across all loctree commands.
///
/// # Key Types
///
/// - [`Painter`](colors::Painter) - Color-aware string formatter
/// - [`is_enabled`](colors::is_enabled) - Color mode detection
pub mod colors;

/// Git operations for temporal awareness.
///
/// Native git operations using libgit2 for analyzing repository history.
///
/// # Key Types
///
/// - [`GitRepo`](git::GitRepo) - Git repository wrapper
/// - [`CommitInfo`](git::CommitInfo) - Commit metadata
/// - [`ChangedFile`](git::ChangedFile) - File change between commits
///
/// # Example
///
/// ```rust,no_run
/// use loctree::git::GitRepo;
/// use std::path::Path;
///
/// let repo = GitRepo::discover(Path::new(".")).unwrap();
/// let head = repo.head_commit().unwrap();
/// println!("HEAD: {}", head);
/// ```
pub mod git;

/// Snapshot comparison engine for temporal analysis.
///
/// Compares loctree snapshots between commits to show semantic changes.
///
/// # Key Types
///
/// - [`SnapshotDiff`](diff::SnapshotDiff) - Result of comparing two snapshots
/// - [`GraphDiff`](diff::GraphDiff) - Import graph changes
/// - [`ExportsDiff`](diff::ExportsDiff) - Export changes
/// - [`DeadCodeDiff`](diff::DeadCodeDiff) - Dead code changes
/// - [`ImpactAnalysis`](diff::ImpactAnalysis) - Change impact assessment
pub mod diff;

/// Query API for fast lookups against the cached snapshot.
///
/// Provides interactive queries without re-scanning:
/// - `who-imports <file>` - Find all files that import a given file
/// - `where-symbol <symbol>` - Find where a symbol is defined
/// - `component-of <file>` - Show what component/module a file belongs to
///
/// # Example
///
/// ```rust,no_run
/// use loctree::{query, snapshot};
/// use std::path::Path;
///
/// let snapshot = snapshot::Snapshot::load(Path::new(".")).unwrap();
/// let result = query::query_who_imports(&snapshot, "src/utils.ts");
/// println!("Found {} importers", result.results.len());
/// ```
pub mod query;

/// Progress UI utilities (spinners, status messages).
///
/// Provides Black-style visual feedback for CLI operations.
pub mod progress;

/// jaq query execution for filtering snapshot data.
///
/// Provides jq-compatible filtering using the jaq library.
pub mod jaq_query;

/// Canonical repository metrics derived once from snapshot graph authority.
pub mod metrics;

/// Impact analysis module for understanding file dependencies.
///
/// Analyzes "what breaks if you modify/remove this file" by traversing
/// the reverse dependency graph to find all direct and transitive consumers.
pub mod impact;

/// Watch mode for live snapshot refresh during iterative development.
///
/// Provides file system watching with debouncing and incremental re-scanning.
pub mod watch;

/// Single-instance lock guarding long-lived watch loops.
///
/// Kernel advisory file lock (`flock` / `LockFileEx`) keyed on the canonical
/// snapshot root. Self-healing on `SIGKILL` because the kernel releases the
/// lock when the holder's fd closes — no stale-PID-file dance required.
pub mod watch_lock;

/// Refactor plan generation for architectural reorganization.
///
/// Analyzes module coupling and suggests safe file reorganization:
/// - Layer detection (UI, App, Kernel, Infra)
/// - Risk scoring based on consumer count and file size
/// - Topological ordering for safe incremental moves
/// - Shim generation for backward compatibility
pub mod refactor_plan;

/// Runtime semantic contracts and idiom catalogs.
pub mod semantic;

/// Symbol graph schema — semantic-topology layer beside `import_graph`.
///
/// Wave-A foundation for C-family (Swift / ObjC / ObjC++ / C / C++) awareness.
/// Holds [`SymbolGraph`](symbols::SymbolGraph) and its node/edge/occurrence
/// types with per-node provenance and per-occurrence/-edge confidence. Attached
/// to [`Snapshot`](snapshot::Snapshot) as an optional section. Leaf module: it
/// imports nothing from the `types.rs` hub.
pub mod symbols;

/// Read-only consumer wrapper around the external `aicx` CLI.
///
/// Provides typed access to `aicx intents`, `aicx steer`, and `aicx search`
/// for the agent-context pipeline (Cut 5 memory slice). Degrades gracefully
/// to empty results when the binary is missing — see [`aicx::AicxClient`].
pub mod aicx;

#[used]
static EMBEDDED_IDIOM_CATALOG_SMOKE: &str = concat!(
    include_str!("semantic/idioms/shell.toml"),
    "\n",
    include_str!("semantic/idioms/make.toml")
);

// ============================================================================
// Re-exports for convenience
// ============================================================================

/// CLI modes.
pub use types::Mode;

/// Analysis options.
pub use types::Options;

/// Output format (Text, Json, Jsonl).
pub use types::OutputMode;

/// Color mode (Auto, Always, Never).
pub use types::ColorMode;

/// Per-file analysis result with imports, exports, commands, etc.
pub use types::FileAnalysis;

/// Detected project stack with extensions and ignores.
pub use detect::DetectedStack;

/// Main stack detection function.
pub use detect::detect_stack;

/// Holographic slice result.
pub use slicer::HolographicSlice;

/// Slice configuration.
pub use slicer::SliceConfig;

/// Persisted analysis state.
pub use snapshot::Snapshot;

/// Symbol graph — semantic-topology layer (optional snapshot section).
pub use symbols::SymbolGraph;

/// Run the import analyzer.
pub use analyzer::run_import_analyzer;

/// Report section for HTML output.
pub use analyzer::ReportSection;

/// Command gap (missing/unused handler).
pub use analyzer::CommandGap;

/// Ranked duplicate export.
pub use analyzer::RankedDup;

/// Refactor plan result.
pub use refactor_plan::RefactorPlan;

/// Architectural layer classification.
pub use refactor_plan::Layer;

/// Risk level for refactor operations.
pub use refactor_plan::RiskLevel;

/// ContextPack composition options.
pub use pack::ContextOptions;

/// Agent-ready context package.
pub use pack::ContextPack;

/// Compose an agent-ready ContextPack for a project.
pub use pack::compose_context_pack;

/// Compose a `PrismReport` for a multi-task framing comparison.
///
/// Pure-data entry point shared by the `loct prism` CLI handler and the
/// `loctree-mcp` `prism` tool. Schema is `loctree.prism.v1.1`, pinned by
/// `loctree-rs/tests/prism_schema_golden.rs`.
pub use cli::dispatch::handlers::prism::{
    PrismAxisScore, PrismOverlap, PrismReport, PrismTaskSummary, run_prism,
};

// ============================================================================
// CLI types (new subcommand interface)
// ============================================================================

/// CLI command enum (source of truth for `loct <command>`).
pub use cli::Command;

/// Global CLI options (--json, --quiet, --verbose, --color).
pub use cli::GlobalOptions;

/// Parsed command result with deprecation warning support.
pub use cli::ParsedCommand;
