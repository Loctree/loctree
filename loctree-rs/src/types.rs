//! Core types for loctree analysis.
//!
//! This module defines the fundamental data structures used throughout loctree:
//! - [`FileAnalysis`] - Per-file analysis result (imports, exports, commands)
//! - [`ImportEntry`] / [`ExportSymbol`] - Import/export representations
//! - [`CommandRef`] / [`EventRef`] - Tauri command and event tracking
//! - [`Mode`] - CLI operation modes
//! - [`Options`] - Analysis configuration

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Default LOC threshold for "large file" warnings.
pub const DEFAULT_LOC_THRESHOLD: usize = 1000;

/// ANSI escape code for red text.
pub const COLOR_RED: &str = "\u{001b}[31m";

/// ANSI escape code to reset text color.
pub const COLOR_RESET: &str = "\u{001b}[0m";

/// Terminal color mode.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ColorMode {
    /// Detect TTY and colorize if interactive.
    #[default]
    Auto,
    /// Always use ANSI colors.
    Always,
    /// Never use colors (for piping/CI).
    Never,
}

/// Output format for analysis results.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutputMode {
    /// Human-readable text with colors and formatting.
    Human,
    /// Pretty-printed JSON object.
    Json,
    /// Newline-delimited JSON (one object per line).
    Jsonl,
}

/// CLI operation mode - determines what loctree does.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Display directory tree with LOC counts (default without -A).
    Tree,
    /// Full import/export analysis (-A flag).
    AnalyzeImports,
    /// Initialize/update snapshot (scan once).
    Init,
    /// Holographic Slice - extract file + deps + consumers for AI context.
    Slice,
    /// Trace a handler - show full investigation path and WHY it's unused/missing.
    Trace,
    /// AI-optimized JSON output with quick wins and slice references.
    ForAi,
    /// Output the canonical findings artifact to stdout.
    Findings,
    /// Output findings summary only to stdout.
    Summary,
    /// Git awareness - temporal knowledge from repository history.
    Git(GitSubcommand),
    /// Unified search - returns symbol matches, semantic matches, dead status.
    Search,
}

/// Git subcommands for temporal awareness - semantic analysis only (no passthrough)
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GitSubcommand {
    /// Semantic diff between two commits (snapshot comparison)
    /// Shows: files changed, graph delta, exports delta, dead code delta, impact analysis
    Compare {
        /// Starting commit (e.g., "HEAD~1", "abc123")
        from: String,
        /// Ending commit, defaults to current working tree if None
        to: Option<String>,
    },
    /// Symbol-level blame: which commit introduced each symbol/import
    Blame {
        /// File to analyze
        file: String,
    },
    /// Track evolution of a symbol or file's structure over time
    History {
        /// Symbol name to track (e.g., "processUser")
        symbol: Option<String>,
        /// File path to track
        file: Option<String>,
        /// Maximum number of commits to show
        limit: usize,
    },
    /// Find when a pattern was introduced (circular import, dead code, etc.)
    WhenIntroduced {
        /// Circular import pattern (e.g., "src/a.rs <-> src/b.rs")
        circular: Option<String>,
        /// Dead code symbol (e.g., "src/utils.rs::unused_fn")
        dead: Option<String>,
        /// Import source (e.g., "lodash")
        import: Option<String>,
    },
}

/// Analysis configuration options.
///
/// Controls file filtering, output format, and analysis behavior.
#[derive(Clone)]
pub struct Options {
    /// File extensions to include (None = all supported).
    pub extensions: Option<HashSet<String>>,
    /// Paths to exclude from analysis.
    pub ignore_paths: Vec<std::path::PathBuf>,
    /// Optional glob-based ignore rules (compiled from ignore patterns).
    ///
    /// This complements `ignore_paths`:
    /// - `ignore_paths` is fast prefix matching (best for literal directories)
    /// - `ignore_globs` enables patterns like `**/index.*` or `*.log`
    pub ignore_globs: Option<std::sync::Arc<globset::GlobSet>>,
    /// Respect .gitignore rules.
    pub use_gitignore: bool,
    /// Maximum directory depth for tree view.
    pub max_depth: Option<usize>,
    /// Terminal color mode.
    pub color: ColorMode,
    /// Output format (Human, Json, Jsonl).
    pub output: OutputMode,
    /// Show summary statistics.
    pub summary: bool,
    /// Max items in summary lists.
    pub summary_limit: usize,
    /// If true, only show summary/top entries (suppress full tree dump).
    pub summary_only: bool,
    /// Include dotfiles/directories.
    pub show_hidden: bool,
    /// Include gitignored files.
    pub show_ignored: bool,
    /// LOC threshold for "large file" warnings.
    pub loc_threshold: usize,
    /// Max files to analyze (0 = unlimited).
    pub analyze_limit: usize,
    /// Path for HTML report output.
    pub report_path: Option<std::path::PathBuf>,
    /// Start local server for HTML report.
    pub serve: bool,
    /// Editor command for click-to-open (e.g., "code -g").
    pub editor_cmd: Option<String>,
    /// Max nodes in dependency graph.
    pub max_graph_nodes: Option<usize>,
    /// Max edges in dependency graph.
    pub max_graph_edges: Option<usize>,
    /// Enable verbose logging.
    pub verbose: bool,
    /// Scan all files (ignore incremental cache).
    pub scan_all: bool,
    /// Symbol to search for (--symbol flag).
    pub symbol: Option<String>,
    /// File for impact analysis (--impact flag).
    pub impact: Option<String>,
    /// Detect build artifacts (node_modules, target, etc.).
    pub find_artifacts: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            extensions: None,
            ignore_paths: Vec::new(),
            ignore_globs: None,
            use_gitignore: true,
            max_depth: None,
            color: ColorMode::Auto,
            output: OutputMode::Human,
            summary: false,
            summary_limit: 50,
            summary_only: false,
            show_hidden: false,
            show_ignored: false,
            loc_threshold: 500,
            analyze_limit: 100,
            report_path: None,
            serve: false,
            editor_cmd: None,
            max_graph_nodes: None,
            max_graph_edges: None,
            verbose: false,
            scan_all: false,
            symbol: None,
            impact: None,
            find_artifacts: false,
        }
    }
}

/// A single line in the tree output (file or directory).
pub struct LineEntry {
    /// Display label (filename with tree prefix).
    pub label: String,
    /// Lines of code (None for directories without aggregation).
    pub loc: Option<usize>,
    /// Path relative to scan root.
    pub relative_path: String,
    /// True if this is a directory.
    pub is_dir: bool,
    /// True if LOC exceeds threshold.
    pub is_large: bool,
}

/// A symbol match from search/grep operations.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymbolMatch {
    /// 1-based line number.
    pub line: usize,
    /// Line content with match highlighted.
    pub context: String,
}

/// Normalized language label for typed analyzer contracts.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    Python,
    Typescript,
    Javascript,
    Shell,
    Makefile,
    Css,
    Html,
    Go,
    Dart,
    Zig,
    Other(String),
}

/// A locally-defined symbol (non-exported or imported).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalSymbol {
    /// Symbol name as defined in the file.
    pub name: String,
    /// Symbol kind (function, class, variable, type, import, etc.).
    pub kind: String,
    /// 1-based line number of definition (if known).
    #[serde(default)]
    pub line: Option<usize>,
    /// Source line context for the definition (trimmed).
    #[serde(default)]
    pub context: String,
    /// True if this symbol is exported.
    #[serde(default)]
    pub is_exported: bool,
}

/// A usage site for a symbol within a file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymbolUsage {
    /// Symbol name as referenced in code.
    pub name: String,
    /// 1-based line number of usage.
    pub line: usize,
    /// Source line context for the usage (trimmed).
    #[serde(default)]
    pub context: String,
}

/// A file exceeding the LOC threshold.
pub struct LargeEntry {
    /// Relative path to file.
    pub path: String,
    /// Lines of code.
    pub loc: usize,
}

/// Aggregated scan statistics.
#[derive(Default)]
pub struct Stats {
    /// Total directories scanned.
    pub directories: usize,
    /// Total files scanned.
    pub files: usize,
    /// Files with countable LOC.
    pub files_with_loc: usize,
    /// Sum of all LOC.
    pub total_loc: usize,
}

/// Mutable collectors passed through tree traversal.
pub struct Collectors<'a> {
    /// Tree entries for display.
    pub entries: &'a mut Vec<LineEntry>,
    /// Files exceeding LOC threshold.
    pub large_entries: &'a mut Vec<LargeEntry>,
    /// Running statistics.
    pub stats: &'a mut Stats,
}

/// Plan 19 v1 callsite contract.
///
/// Re-exported from `loctree-ast` so the cold-scan dispatcher and downstream
/// analyzers share one shape for tree-sitter call extraction. Stage 1 emits
/// this from the TS/JS extractors; Stage 2 will swap it into `SymbolUsage`
/// once parity for member-call resolution lands.
pub use loctree_ast::CallEntry;

/// An import statement (JS/TS/Python).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportEntry {
    /// 1-based line number of the import declaration (if known).
    #[serde(default)]
    pub line: Option<usize>,
    /// Resolved/normalized source path.
    pub source: String,
    /// Original source as written in code.
    pub source_raw: String,
    /// Import type (static, side-effect, dynamic).
    pub kind: ImportKind,
    /// Absolute resolved path (if local file).
    pub resolved_path: Option<String>,
    /// True if bare specifier (npm package, not relative).
    pub is_bare: bool,
    /// Imported symbols (named, default, namespace).
    pub symbols: Vec<ImportSymbol>,
    /// Resolution result (local, stdlib, dynamic, unknown).
    pub resolution: ImportResolutionKind,
    /// True if inside TYPE_CHECKING block (Python).
    pub is_type_checking: bool,
    /// True if placed inside a function/method (lazy import to break cycles).
    #[serde(default)]
    pub is_lazy: bool,
    /// True if import starts with `crate::` (Rust only).
    #[serde(default)]
    pub is_crate_relative: bool,
    /// True if import starts with `super::` (Rust only).
    #[serde(default)]
    pub is_super_relative: bool,
    /// True if import starts with `self::` (Rust only).
    #[serde(default)]
    pub is_self_relative: bool,
    /// True if this is a Rust `mod foo;` declaration (not a true import).
    #[serde(default)]
    pub is_mod_declaration: bool,
    /// Original raw path before resolution (Rust only).
    #[serde(default)]
    pub raw_path: String,
}

/// Type of import statement.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ImportKind {
    /// `import X from 'y'` or `from x import y`
    Static,
    /// `import type { X } from 'y'` (TypeScript-only, still a real dependency)
    Type,
    /// `import 'styles.css'` (no bindings)
    SideEffect,
    /// `import('module')` or `React.lazy(() => import(...))`
    Dynamic,
}

/// How an import source was resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportResolutionKind {
    /// Local file (relative or absolute path).
    Local,
    /// Standard library module.
    Stdlib,
    /// Dynamic import (path unknown at parse time).
    Dynamic,
    /// Could not resolve.
    Unknown,
}

/// A single symbol from an import statement.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportSymbol {
    /// Original name in source module.
    pub name: String,
    /// Local alias (e.g., `import { foo as bar }`).
    pub alias: Option<String>,
    /// True if default import (`import Foo from './bar'`).
    #[serde(default)]
    pub is_default: bool,
}

/// A re-export statement (`export { x } from './y'` or `export * from './z'`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReexportEntry {
    /// Source module path.
    pub source: String,
    /// Star or named re-export.
    pub kind: ReexportKind,
    /// Resolved absolute path (if local).
    pub resolved: Option<String>,
}

/// Type of re-export.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ReexportKind {
    /// `export * from './module'`
    Star,
    /// `export { a, b as c } from './module'`
    /// Each tuple is (original_name, exported_name) - same if no alias
    Named(Vec<(String, String)>),
}

/// Parameter information for function exports.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ParamInfo {
    /// Parameter name.
    pub name: String,
    /// Type annotation if present (e.g., "string", "int", "&str").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_annotation: Option<String>,
    /// Whether parameter has a default value.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_default: bool,
}

/// An exported symbol from a module.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportSymbol {
    /// Exported name (may differ from internal name).
    pub name: String,
    /// Symbol kind: "function", "class", "const", "type", etc.
    pub kind: String,
    /// Export type: "named", "default", "reexport".
    pub export_type: String,
    /// 1-based line number of declaration.
    pub line: Option<usize>,
    /// Function parameters (empty for non-functions).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<ParamInfo>,
    /// Stable identifier for this symbol (Plan 18 v2). Default-empty
    /// when populated by callers that don't yet carry a v2-quality id
    /// (most cold-scan extractors prior to Plan 19). The LSP live path
    /// fills this in from `SymbolIdV1::from_export` so per-symbol
    /// metadata maps and `loctree/symbolChanged` diff classification
    /// have a typed key without breaking the v1 wire contract.
    #[serde(default, skip_serializing_if = "SymbolIdV1::is_empty")]
    pub symbol_id: SymbolIdV1,
}

/// Stable identifier for a symbol within a snapshot — **v1 contract**.
///
/// Format: `<file_path>::<symbol_name>`, mirroring the string alias
/// already used by every Layer 3 semantic analyzer
/// ([`crate::semantic::SymbolId`]).
///
/// This newtype exists so wire boundaries (LSP custom requests, AICX
/// intent overlays, MCP tools) can carry a typed identity instead of a
/// bare `String`, while the underlying format stays compatible with
/// every fact already keyed by `<file>::<name>` in the snapshot.
///
/// **v2 contract — deferred to Plan 18.** The full per-symbol tracking
/// described in `docs/plans/lsp/18-symbol-level-granularity.md` requires
/// a stable byte-range / node-id hash so the id survives line moves and
/// rolls when a body is rewritten. That depends on the Plan 16
/// tree-sitter substrate, which is **not** integrated yet (see
/// `LOCTREE_NEXT.md` P0 #1). Until the substrate lands, callers must
/// treat v1 ids as file-coarse: they survive renames within a file only
/// when the symbol name is unchanged, and they cannot tell apart "moved
/// to line N" from "rewritten in place".
///
/// The `version()` method makes the contract observable at runtime so
/// agents can refuse to act on body-sensitive operations (per-symbol
/// AICX overlay, semantic-equality dedup) when only v1 is available.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct SymbolIdV1(pub String);

impl SymbolIdV1 {
    /// Wire-version label. Bumped to `"v2-tree-sitter"` once Plan 18
    /// lands the byte-range hash variant.
    pub const VERSION: &'static str = "v1-string";

    /// Build a v1 id from a `(file, symbol)` pair. Idempotent and pure.
    pub fn from_parts(file: &str, symbol: &str) -> Self {
        Self(format!("{}::{}", file, symbol))
    }

    /// Build a v1 id from an `ExportSymbol` with its containing file.
    /// Convenience for the LSP/MCP path that already has both halves.
    pub fn from_export(file: &str, export: &ExportSymbol) -> Self {
        Self::from_parts(file, &export.name)
    }

    /// Return `(file, symbol)` view of a well-formed v1 id, or `None`
    /// when the input does not contain the `::` separator.
    pub fn split_parts(&self) -> Option<(&str, &str)> {
        self.0.split_once("::")
    }

    /// Borrow the underlying `<file>::<symbol>` string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when this id carries no string content. Used by serde's
    /// `skip_serializing_if` so back-compatible callers that did not
    /// populate `symbol_id` keep emitting the same wire shape they did
    /// before Plan 18 v2.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Wire-version label for capability/handshake discovery.
    pub fn version() -> &'static str {
        Self::VERSION
    }
}

/// Stable identifier for a symbol — **v2 contract** (Plan 18 v2).
///
/// Format: `<file>::<kind>::<name>::<hash16>` where `<hash16>` is the
/// hex-encoded `u64` from a [`std::hash::DefaultHasher`] over the
/// captured byte range of the symbol's declaration node. Survives line
/// moves (the byte range hash is computed against the source span, not
/// the line offset) and rolls when the body is rewritten.
///
/// **Heuristic, not semantic.** Two symbols with different bodies that
/// "do the same thing" still get different ids; two symbols with
/// identical bodies in different files still get different ids because
/// `<file>` is part of the key. Cross-file moves (Plan 18 v3 candidate)
/// are out of scope.
///
/// **Intentional cohabitation with [`SymbolIdV1`].** v2 is opt-in and
/// only populated by call sites that have a tree-sitter node available.
/// The v1 string form (`<file>::<symbol>`) stays the default wire
/// contract for `loctree/find` and `loctree/aicx`; v2 is keyed by the
/// LSP live tracker for `loctree/symbolChanged` diff classification.
/// The `to_v1()` accessor keeps the bridge cheap.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct SymbolIdV2(pub String);

impl SymbolIdV2 {
    /// Wire-version label.
    pub const VERSION: &'static str = "v2-byte-range";

    /// Build a v2 id from `(file, kind, name, byte_range)`. Hashes the
    /// byte range tuple via `DefaultHasher` and truncates the resulting
    /// `u64` to 16 hex chars — collision-resistant within a single
    /// process for the small per-file cardinality we care about
    /// (typical TS file: <100 exports).
    pub fn from_parts(file: &str, kind: &str, name: &str, byte_range: (usize, usize)) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        byte_range.0.hash(&mut hasher);
        byte_range.1.hash(&mut hasher);
        let hash = hasher.finish();
        Self(format!("{}::{}::{}::{:016x}", file, kind, name, hash))
    }

    /// Project the v2 id down to a v1 string id (`<file>::<name>`).
    /// Used so v2-aware callers can still feed handlers wired only for
    /// v1 (find/aicx) without re-hashing.
    pub fn to_v1(&self) -> SymbolIdV1 {
        let parts: Vec<&str> = self.0.splitn(4, "::").collect();
        if parts.len() == 4 {
            SymbolIdV1::from_parts(parts[0], parts[2])
        } else {
            SymbolIdV1(self.0.clone())
        }
    }

    /// Borrow the underlying `<file>::<kind>::<name>::<hash>` string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when this id carries no string content.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Wire-version label for capability/handshake discovery.
    pub fn version() -> &'static str {
        Self::VERSION
    }
}

impl std::fmt::Display for SymbolIdV2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Display for SymbolIdV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SymbolIdV1 {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SymbolIdV1 {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// A Tauri command reference (handler or invocation).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandRef {
    /// Command function name (Rust side).
    pub name: String,
    /// Exposed name if different (e.g., via `#[tauri::command(rename_all = ...)]`).
    pub exposed_name: Option<String>,
    /// 1-based line number.
    pub line: usize,
    /// Generic type parameter (e.g., `State<AppState>`).
    pub generic_type: Option<String>,
    /// Payload type/shape if detected.
    pub payload: Option<String>,
    /// Plugin name for Tauri plugin commands (e.g., "window" from `invoke('plugin:window|set_icon')`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
}

/// Casing inconsistency in command payload keys.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandPayloadCasing {
    /// Command name.
    pub command: String,
    /// Key with inconsistent casing.
    pub key: String,
    /// File path.
    pub path: String,
    /// 1-based line number.
    pub line: usize,
}

/// JS/TS string literal captured for dynamic/registry awareness
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StringLiteral {
    pub value: String,
    pub line: usize,
}

/// Python/Backend route declaration (FastAPI/Flask/etc.)
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RouteInfo {
    /// Framework label (e.g., "fastapi", "flask")
    pub framework: String,
    /// HTTP method or decorator kind (GET/POST/route/etc.)
    pub method: String,
    /// Route path if extracted from decorator
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Handler name (set when attached to a def)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 1-based line number of the decorator
    pub line: usize,
}

/// Python exec/eval/compile dynamic code generation pattern.
/// Tracks template strings (e.g., "get%s", "set%s") passed to exec() that generate symbols dynamically.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DynamicExecTemplate {
    /// Template pattern containing placeholders (%s, %d, {name}, etc.)
    pub template: String,
    /// Generated symbol names extracted from the template (e.g., ["get", "set"] from "get%s", "set%s")
    pub generated_prefixes: Vec<String>,
    /// 1-based line number where exec/eval/compile is called
    pub line: usize,
    /// Type of dynamic call: "exec", "eval", or "compile"
    pub call_type: String,
}

/// Python sys.modules monkey-patching pattern.
/// Detects when a module injects itself into sys.modules under a different name,
/// making all its exports accessible at runtime (should not be flagged as dead).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SysModulesInjection {
    /// The module name being injected (e.g., "compat", "shim", or "__name__")
    pub module_name: String,
    /// 1-based line number where injection occurs
    pub line: usize,
    /// The value being assigned (variable name or expression)
    pub value: String,
}

/// A Tauri event reference (emit or listen).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventRef {
    /// Original event name as written (may be const reference).
    pub raw_name: Option<String>,
    /// Resolved event name.
    pub name: String,
    /// 1-based line number.
    pub line: usize,
    /// "emit" or "listen".
    pub kind: String,
    /// True if awaited (`await emit(...)`).
    pub awaited: bool,
    /// Payload type/shape if detected.
    pub payload: Option<String>,
    /// True if this event uses a dynamic pattern (format!/template literal).
    #[serde(default)]
    pub is_dynamic: bool,
}

/// Python concurrency race indicator
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PyRaceIndicator {
    /// Line number where the pattern was found
    pub line: usize,
    /// Type of concurrency pattern: "threading", "asyncio", "multiprocessing"
    pub concurrency_type: String,
    /// Specific pattern: "Thread", "Lock", "gather", "create_task", "Pool", etc.
    pub pattern: String,
    /// Risk level: "info", "warning", "high"
    pub risk: String,
    /// Description of the potential issue
    pub message: String,
}

/// Rust visibility for definition-level facts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    Crate,
    Restricted(String),
    #[default]
    Private,
}

/// Method defined inside a Rust impl block.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ImplMethod {
    /// Method name.
    pub name: String,
    /// Implemented type or trait target.
    pub qualifier: String,
    /// Trait qualifier for `impl Trait for Type`.
    #[serde(default)]
    pub trait_qualifier: Option<String>,
    /// 1-based line number, if known.
    #[serde(default)]
    pub line: Option<usize>,
    /// True for `async fn`.
    #[serde(default)]
    pub is_async: bool,
    /// Rust visibility.
    #[serde(default)]
    pub visibility: Visibility,
    /// True when this entry is a source definition.
    #[serde(default = "default_true")]
    pub is_definition: bool,
}

fn default_true() -> bool {
    true
}

/// Cargo target kind declared by a manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Bin,
    Lib,
    Example,
    Bench,
    Test,
}

/// Cargo target declared by a crate manifest.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CargoTarget {
    pub name: String,
    pub kind: TargetKind,
    pub path: PathBuf,
    pub crate_root: PathBuf,
}

/// Observability/log emission level.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Panic,
}

/// Logging macro/function call extracted from source.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogMessage {
    pub level: LogLevel,
    pub macro_or_fn: String,
    pub format_string: String,
    pub line: usize,
    #[serde(default)]
    pub function_context: Option<String>,
}

/// Per-file analysis result.
///
/// Contains all extracted information from a single source file:
/// imports, exports, Tauri commands/events, and metadata.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileAnalysis {
    /// Relative path from project root.
    #[serde(default)]
    pub path: String,
    /// Lines of code (excluding blanks/comments).
    #[serde(default)]
    pub loc: usize,
    /// Detected language: "typescript", "javascript", "python", "rust", "css",
    /// or a resource/config label such as "md", "json", "yaml", "toml".
    #[serde(default)]
    pub language: String,
    /// File kind: "code", "test", "config", "doc", "workflow", "locale", "resource", "style".
    #[serde(default)]
    pub kind: String,
    /// Non-code resource membership when the file is an inspectable agent surface.
    ///
    /// Examples: "doc", "config", "workflow", "locale", "resource".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_kind: Option<String>,
    /// True if test file (based on path/name patterns).
    #[serde(default)]
    pub is_test: bool,
    /// True if generated file (has generation marker).
    #[serde(default)]
    pub is_generated: bool,
    /// True if file uses Flow type annotations (@flow).
    #[serde(default)]
    pub is_flow_file: bool,
    /// Import statements found in file.
    #[serde(default)]
    pub imports: Vec<ImportEntry>,
    /// Re-export statements (`export { x } from './y'`).
    #[serde(default)]
    pub reexports: Vec<ReexportEntry>,
    /// Dynamic import paths (resolved where possible).
    #[serde(default)]
    pub dynamic_imports: Vec<String>,
    /// Exported symbols (functions, classes, consts, types).
    #[serde(default)]
    pub exports: Vec<ExportSymbol>,
    /// Locally-defined symbols (non-exported or imported).
    #[serde(default)]
    pub local_symbols: Vec<LocalSymbol>,
    /// Local usage sites for symbols in this file.
    #[serde(default)]
    pub symbol_usages: Vec<SymbolUsage>,
    /// Tauri command invocations (frontend `invoke()`).
    #[serde(default)]
    pub command_calls: Vec<CommandRef>,
    /// Tauri command handlers (backend `#[tauri::command]`).
    #[serde(default)]
    pub command_handlers: Vec<CommandRef>,
    /// Detected casing inconsistencies in command payloads.
    #[serde(default)]
    pub command_payload_casing: Vec<CommandPayloadCasing>,
    /// String literals for dynamic/registry awareness.
    #[serde(default)]
    pub string_literals: Vec<StringLiteral>,
    /// Tauri event emissions.
    #[serde(default)]
    pub event_emits: Vec<EventRef>,
    /// Tauri event listeners.
    #[serde(default)]
    pub event_listens: Vec<EventRef>,
    /// Event name constants (`const EVENT_X = "event-x"`).
    #[serde(default)]
    pub event_consts: HashMap<String, String>,
    /// Symbol search matches.
    #[serde(default)]
    pub matches: Vec<SymbolMatch>,
    /// Detected entry points (main, index, App).
    #[serde(default)]
    pub entry_points: Vec<String>,
    /// Rust handlers registered via `tauri::generate_handler![...]`.
    #[serde(default)]
    pub tauri_registered_handlers: Vec<String>,
    /// File mtime (Unix timestamp) for incremental scanning.
    #[serde(default)]
    pub mtime: u64,
    /// File size in bytes for incremental cache validation.
    #[serde(default)]
    pub size: u64,
    /// Python concurrency race indicators.
    #[serde(default)]
    pub py_race_indicators: Vec<PyRaceIndicator>,
    /// Python: True if package has py.typed marker (PEP 561).
    #[serde(default)]
    pub is_typed_package: bool,
    /// Python: True if namespace package (PEP 420).
    #[serde(default)]
    pub is_namespace_package: bool,
    /// Locally-referenced symbols (for dead-code suppression).
    #[serde(default)]
    pub local_uses: Vec<String>,
    /// Type usages that appear in function signatures (parameters/returns).
    #[serde(default)]
    pub signature_uses: Vec<SignatureUse>,

    /// Web route handlers detected in Python/other backends
    #[serde(default)]
    pub routes: Vec<RouteInfo>,

    /// Pytest fixtures defined in this file
    #[serde(default)]
    pub pytest_fixtures: Vec<String>,

    /// True if file uses WeakMap or WeakSet (global registry pattern in React/libs)
    #[serde(default)]
    pub has_weak_collections: bool,

    /// Python exec/eval/compile dynamic code generation templates.
    #[serde(default)]
    pub dynamic_exec_templates: Vec<DynamicExecTemplate>,

    /// Python sys.modules monkey-patching injections.
    /// Files with these injections have all exports accessible at runtime.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sys_modules_injections: Vec<SysModulesInjection>,

    /// Layer 3 semantic facts attached to this file. None when no semantic
    /// analyzer ran (e.g. unsupported language or scan-only mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_facts_ref: Option<SemanticFactsRef>,

    /// Rust impl-block methods defined in this file.
    #[serde(default)]
    pub impl_methods: Vec<ImplMethod>,

    /// Cargo targets declared by this file when it is a Cargo manifest.
    #[serde(default)]
    pub cargo_targets: Vec<CargoTarget>,

    /// Logging calls/macros emitted by this file.
    #[serde(default)]
    pub log_messages: Vec<LogMessage>,

    /// Cargo crate/package that owns this file, if known.
    #[serde(default)]
    pub crate_membership: Option<String>,

    /// Per-file symbol-graph fragment from the C-family tree-sitter extractor
    /// (Wave B). Merged into `Snapshot::symbol_graph` at snapshot build; kept
    /// on the file so incremental scans preserve symbols for unchanged files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_fragment: Option<crate::symbols::SymbolGraph>,
}

/// Lightweight reference into the per-snapshot SemanticFacts table.
///
/// Full SemanticFacts live alongside Snapshot, not embedded in every
/// FileAnalysis (avoids snapshot size blow-up).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticFactsRef {
    pub idiom_tag_count: u32,
    pub has_dispatch_edges: bool,
    pub has_env_contracts: bool,
}

impl ImportEntry {
    pub fn new(source: String, kind: ImportKind) -> Self {
        let is_bare = !source.starts_with('.') && !source.starts_with('/');
        Self {
            line: None,
            source_raw: source.clone(),
            source,
            kind,
            resolved_path: None,
            is_bare,
            symbols: Vec::new(),
            resolution: ImportResolutionKind::Unknown,
            is_type_checking: false,
            is_lazy: false,
            is_crate_relative: false,
            is_super_relative: false,
            is_self_relative: false,
            raw_path: String::new(),
            is_mod_declaration: false,
        }
    }
}

impl ExportSymbol {
    /// Create export symbol without params (backwards compatible).
    ///
    /// `symbol_id` defaults to empty — Plan 18 v2 callers that have a
    /// containing file path should call [`Self::with_symbol_id`] (or
    /// set the field directly) so per-symbol tracking can key on a
    /// non-empty id.
    pub fn new(name: String, kind: &str, export_type: &str, line: Option<usize>) -> Self {
        Self {
            name,
            kind: kind.to_string(),
            export_type: export_type.to_string(),
            line,
            params: Vec::new(),
            symbol_id: SymbolIdV1::default(),
        }
    }

    /// Create export symbol with params (for functions).
    pub fn with_params(
        name: String,
        kind: &str,
        export_type: &str,
        line: Option<usize>,
        params: Vec<ParamInfo>,
    ) -> Self {
        Self {
            name,
            kind: kind.to_string(),
            export_type: export_type.to_string(),
            line,
            params,
            symbol_id: SymbolIdV1::default(),
        }
    }

    /// Populate [`Self::symbol_id`] from `(file, name)` and return
    /// the modified export. Plan 18 v2 entry point used by the LSP
    /// live extractor and snapshot pipelines that know which file
    /// they're attaching the export to.
    pub fn with_symbol_id(mut self, file: &str) -> Self {
        self.symbol_id = SymbolIdV1::from_parts(file, &self.name);
        self
    }
}

impl FileAnalysis {
    pub fn new(path: String) -> Self {
        Self {
            path,
            loc: 0,
            language: String::new(),
            kind: "code".to_string(),
            resource_kind: None,
            is_test: false,
            is_generated: false,
            is_flow_file: false,
            imports: Vec::new(),
            reexports: Vec::new(),
            dynamic_imports: Vec::new(),
            exports: Vec::new(),
            local_symbols: Vec::new(),
            symbol_usages: Vec::new(),
            command_calls: Vec::new(),
            command_handlers: Vec::new(),
            command_payload_casing: Vec::new(),
            string_literals: Vec::new(),
            event_emits: Vec::new(),
            event_listens: Vec::new(),
            event_consts: HashMap::new(),
            matches: Vec::new(),
            entry_points: Vec::new(),
            tauri_registered_handlers: Vec::new(),
            py_race_indicators: Vec::new(),
            mtime: 0,
            size: 0,
            is_typed_package: false,
            is_namespace_package: false,
            local_uses: Vec::new(),
            signature_uses: Vec::new(),
            routes: Vec::new(),
            pytest_fixtures: Vec::new(),
            has_weak_collections: false,
            dynamic_exec_templates: Vec::new(),
            sys_modules_injections: Vec::new(),
            semantic_facts_ref: None,
            impl_methods: Vec::new(),
            cargo_targets: Vec::new(),
            log_messages: Vec::new(),
            crate_membership: None,
            symbol_fragment: None,
        }
    }
}

/// How a type is used in a function signature.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignatureUseKind {
    Parameter,
    Return,
}

/// A single mention of a type in a function signature.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignatureUse {
    /// Function or method name where the type appears.
    pub function: String,
    /// Kind of usage: parameter or return type.
    pub usage: SignatureUseKind,
    /// The referenced type name (as parsed).
    pub type_name: String,
    /// Line number for traceability.
    #[serde(default)]
    pub line: Option<usize>,
}

// Convenience type aliases reused across modules
pub type ExportIndex = HashMap<String, Vec<String>>;
pub type PayloadEntry = (String, usize, Option<String>);
pub type PayloadMap = HashMap<String, Vec<PayloadEntry>>;

#[cfg(test)]
mod symbol_id_v1_tests {
    use super::{ExportSymbol, SymbolIdV1};

    fn export(name: &str) -> ExportSymbol {
        ExportSymbol {
            name: name.to_string(),
            kind: "function".to_string(),
            export_type: "named".to_string(),
            line: Some(42),
            params: Vec::new(),
            symbol_id: SymbolIdV1::default(),
        }
    }

    #[test]
    fn from_parts_uses_double_colon_separator() {
        let id = SymbolIdV1::from_parts("src/lib.rs", "compose_runtime_slice");
        assert_eq!(id.as_str(), "src/lib.rs::compose_runtime_slice");
    }

    #[test]
    fn from_export_matches_legacy_string_alias_format() {
        let id = SymbolIdV1::from_export("src/lib.rs", &export("foo"));
        assert_eq!(id.as_str(), "src/lib.rs::foo");
    }

    #[test]
    fn split_parts_recovers_file_and_symbol() {
        let id = SymbolIdV1::from_parts("a/b.rs", "Bar");
        assert_eq!(id.split_parts(), Some(("a/b.rs", "Bar")));
    }

    #[test]
    fn split_parts_returns_none_for_malformed_id() {
        let id = SymbolIdV1("bare-symbol".into());
        assert!(id.split_parts().is_none());
    }

    #[test]
    fn version_is_v1_string() {
        assert_eq!(SymbolIdV1::version(), "v1-string");
    }

    #[test]
    fn serializes_as_plain_string_for_wire_compat() {
        let id = SymbolIdV1::from_parts("f.rs", "g");
        let json = serde_json::to_value(&id).unwrap();
        assert_eq!(json, serde_json::json!("f.rs::g"));
    }

    #[test]
    fn round_trips_through_serde() {
        let id = SymbolIdV1::from_parts("f.rs", "g");
        let json = serde_json::to_string(&id).unwrap();
        let back: SymbolIdV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn default_is_empty_so_skip_serializing_kicks_in() {
        let id = SymbolIdV1::default();
        assert!(id.is_empty());
        assert_eq!(id.as_str(), "");
    }

    #[test]
    fn export_with_symbol_id_populates_v1_form() {
        let exp = ExportSymbol::new("foo".to_string(), "function", "named", Some(1))
            .with_symbol_id("src/lib.rs");
        assert_eq!(exp.symbol_id.as_str(), "src/lib.rs::foo");
    }

    #[test]
    fn export_default_symbol_id_serializes_without_field_for_back_compat() {
        // Plan 18 v2: legacy ExportSymbol consumers (cold-scan analyzers)
        // use `ExportSymbol::new` which leaves symbol_id empty. The
        // serialized JSON must not carry `symbol_id` so existing
        // snapshot consumers stay byte-compatible.
        let exp = ExportSymbol::new("foo".to_string(), "function", "named", Some(1));
        let json = serde_json::to_value(&exp).unwrap();
        assert!(json.get("symbol_id").is_none(), "got: {json}");
    }

    #[test]
    fn export_with_symbol_id_serializes_field() {
        let exp = ExportSymbol::new("foo".to_string(), "function", "named", Some(1))
            .with_symbol_id("src/lib.rs");
        let json = serde_json::to_value(&exp).unwrap();
        assert_eq!(json["symbol_id"], serde_json::json!("src/lib.rs::foo"));
    }
}

#[cfg(test)]
mod symbol_id_v2_tests {
    use super::SymbolIdV2;

    #[test]
    fn from_parts_includes_file_kind_name_and_hash() {
        let id = SymbolIdV2::from_parts("src/lib.rs", "function", "greet", (16, 21));
        // Layout: <file>::<kind>::<name>::<hash16>
        let parts: Vec<&str> = id.as_str().splitn(4, "::").collect();
        assert_eq!(parts[0], "src/lib.rs");
        assert_eq!(parts[1], "function");
        assert_eq!(parts[2], "greet");
        assert_eq!(parts[3].len(), 16, "hash component must be 16 hex chars");
    }

    #[test]
    fn equal_ranges_produce_equal_hash() {
        // Same byte_range + same prefix tuple = same hash, deterministic
        // within a single process (DefaultHasher seeds per-process).
        let a = SymbolIdV2::from_parts("a.ts", "function", "f", (10, 20));
        let b = SymbolIdV2::from_parts("a.ts", "function", "f", (10, 20));
        assert_eq!(a, b);
    }

    #[test]
    fn different_ranges_produce_different_hash() {
        let a = SymbolIdV2::from_parts("a.ts", "function", "f", (10, 20));
        let b = SymbolIdV2::from_parts("a.ts", "function", "f", (10, 21));
        assert_ne!(a, b, "hash must roll when byte range changes");
    }

    #[test]
    fn to_v1_recovers_file_and_name() {
        let id = SymbolIdV2::from_parts("src/lib.rs", "function", "greet", (16, 21));
        let v1 = id.to_v1();
        assert_eq!(v1.as_str(), "src/lib.rs::greet");
    }

    #[test]
    fn version_is_v2_byte_range() {
        assert_eq!(SymbolIdV2::version(), "v2-byte-range");
    }

    #[test]
    fn serializes_as_plain_string_for_wire_compat() {
        let id = SymbolIdV2::from_parts("f.rs", "class", "C", (0, 100));
        let json = serde_json::to_value(&id).unwrap();
        assert!(json.is_string(), "v2 ids serialize as bare strings");
    }
}
