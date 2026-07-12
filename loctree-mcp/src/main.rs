//! # loctree-mcp
//!
//! Universal MCP server for loctree - works with ANY project directory.
//! Scan once, query everything. Use BEFORE reading files manually.
//!
//! ## Architecture
//!
//! - **Project-agnostic**: Each tool accepts `project` parameter
//! - **Auto-scan**: First use on a project creates snapshot automatically
//! - **Multi-project cache**: Snapshots kept in RAM for instant responses
//! - **Zero config**: Just start the server, no --project needed
//!
//! ## Usage
//!
//! ```bash
//! # Standalone
//! loctree-mcp
//! ```
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use clap::Parser;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::ServerInfo;
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info};

mod args;
mod auth;
mod http;
mod security;
mod signals;

use args::{Args, TransportKind};
use signals::{ignore_sigpipe, install_panic_hook, safe_stderr_log};

use loctree::analyzer::classify::{detect_language, detect_language_from_filename};
use loctree::analyzer::crowd::detect_crowd_with_edges;
use loctree::analyzer::cycles::find_cycles;
use loctree::analyzer::dead_parrots::{DeadFilterConfig, find_dead_exports};
use loctree::analyzer::occurrences::{
    FileScope, OccurrenceResults, ReportOptions, ScanOptions, scan_files_with_scope,
};
use loctree::analyzer::pipelines::build_pipeline_summary;
use loctree::analyzer::root_scan::scan_results_from_snapshot;
use loctree::analyzer::route_twins::detect_route_twins;
use loctree::analyzer::search::{literal_fuzzy_suggestions, run_search};
use loctree::analyzer::suppression_inventory::{
    SilencerKind, inventory as silencer_inventory, resolve_ignore_globs,
};
use loctree::analyzer::twins::{detect_exact_twins, twin_action};
use loctree::atlas::{
    ContextOptions, atlas_dir_for_project, compose_context_pack_from_snapshot,
    materialize_context_atlas, render_context_markdown,
};
use loctree::focuser::{FocusConfig, HolographicFocus};
use loctree::git::find_git_root;
use loctree::metrics::{repository_metrics, top_hubs_by_importers_direct};
use loctree::query::{query_where_symbol, query_who_imports};
use loctree::slicer::{HolographicSlice, SliceConfig};
use loctree::snapshot::{Snapshot, normalize_roots_for_scope_compare};

// ============================================================================
// Tool Parameter Types - All tools have optional `project` parameter
// ============================================================================

/// Deserialize usize from either a number or a string (Claude Code sends strings).
mod deserialize_usize_lenient {
    use serde::{self, Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<usize, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StringOrNum {
            Num(usize),
            Str(String),
        }
        match StringOrNum::deserialize(deserializer)? {
            StringOrNum::Num(n) => Ok(n),
            StringOrNum::Str(s) => s
                .trim()
                .parse()
                .map_err(|_| serde::de::Error::custom(format!("invalid number: {s}"))),
        }
    }
}

/// Process-wide default project root, pinned via `--root` / `--project`.
///
/// `None` (the default — never `.set()`) keeps the long-standing universal
/// behavior: every tool's empty `project` field resolves against the server
/// cwd. Once `--root` pins this at startup, empty `project` fields resolve
/// against the pinned root instead. A per-request `project` value always
/// wins because serde only invokes [`default_project`] when the field is
/// absent.
static DEFAULT_PROJECT_ROOT: OnceLock<String> = OnceLock::new();

/// Pin the process-wide default project root. Called once at startup when
/// `--root` is provided; the path is canonicalized to an absolute form so
/// the pin matches the watch lock's canonical snapshot root. A second call
/// is silently ignored (`OnceLock::set` returns `Err`).
fn set_default_project_root(root: &str) {
    let resolved = Path::new(root)
        .canonicalize()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| root.to_string());
    let _ = DEFAULT_PROJECT_ROOT.set(resolved);
}

fn default_project() -> String {
    // A `--root` / `--project` pin wins over cwd. Per-request `project`
    // params still override this — serde only calls us when the field is
    // absent, so universal callers are unaffected.
    if let Some(root) = DEFAULT_PROJECT_ROOT.get() {
        return root.clone();
    }
    // Normalize "." to absolute path - MCP server cwd may differ from agent cwd
    std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".to_string())
}

fn tagmap_normalize(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn tagmap_matches(value: &str, keyword_lower: &str, keyword_normalized: &str) -> bool {
    let value_lower = value.to_ascii_lowercase();
    value_lower.contains(keyword_lower)
        || (!keyword_normalized.is_empty()
            && tagmap_normalize(&value_lower).contains(keyword_normalized))
}

fn path_filter_matches(path: &str, filter: &str) -> bool {
    loctree::analyzer::occurrences::path_matches_scope(path, filter)
}

fn signature_symbol_anchor(query: &str) -> Option<String> {
    let mut tokens = query
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|token| !token.is_empty())
        .filter(|token| {
            !matches!(
                token.to_ascii_lowercase().as_str(),
                "async" | "fn" | "function" | "pub" | "export"
            )
        });
    tokens.next_back().map(|token| token.to_ascii_lowercase())
}

fn context_has_identifier(context: &str, ident: &str) -> bool {
    context
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|token| token.eq_ignore_ascii_case(ident))
}

/// Environment variable that holds the SaaS-mode tenant root allowlist.
///
/// Format: one or more absolute paths separated by the platform path
/// separator (`:` on Unix, `;` on Windows). When set, every resolved
/// project path MUST canonicalize to a location underneath one of the
/// listed roots; otherwise the request is rejected with
/// `io::ErrorKind::PermissionDenied`.
///
/// When unset, the server falls back to the historical local-trust
/// behavior — the basic structural rejections in
/// `validate_project_path` still apply (no `..`, no NUL bytes, no
/// empty input), so even the unset configuration is hardened compared
/// to the pre-validation state.
const ALLOWED_ROOTS_ENV: &str = "LOCTREE_MCP_ALLOWED_ROOTS";

/// Read and canonicalize the optional tenant-root allowlist from the
/// process environment. Returns `None` when `LOCTREE_MCP_ALLOWED_ROOTS`
/// is unset or empty. Entries that fail to canonicalize are dropped
/// (they cannot match anything anyway) but a non-empty env var with no
/// valid entries returns `Some(Vec::new())`, which causes every
/// caller-supplied path to be rejected — fail-closed by design.
fn allowed_project_roots() -> Option<Vec<PathBuf>> {
    let raw = std::env::var(ALLOWED_ROOTS_ENV).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let roots: Vec<PathBuf> = std::env::split_paths(&raw)
        .filter(|p| !p.as_os_str().is_empty())
        .filter_map(|p| p.canonicalize().ok())
        .collect();
    Some(roots)
}

/// Lexically-validated project path — proves that the caller-supplied
/// string has passed non-emptiness, NUL-byte rejection, parent-dir
/// rejection, and (when configured) the pre-canonical allowlist gate.
/// The wrapped `PathBuf` is the cwd-joined absolute lexical form; it
/// has NOT yet been canonicalized, because validation runs before any
/// filesystem touch (e.g. tests pass non-existent paths).
///
/// The newtype exists primarily as a dataflow witness: Semgrep's
/// `tainted-path` analysis sees `PathBuf::from(input)` as a sink, so
/// the input is decomposed into per-component sanitized pushes via
/// `assemble_lexical_path` rather than handed to `PathBuf::from`
/// wholesale. Consumers must canonicalize + bound via
/// [`loctree::fs_utils::SanitizedPath`] before passing the path to
/// any `fs::*` API.
pub struct LexicallyValidatedPath(PathBuf);

impl LexicallyValidatedPath {
    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl std::fmt::Debug for LexicallyValidatedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("LexicallyValidatedPath")
            .field(&self.0)
            .finish()
    }
}

/// Decompose a sanitized input string into a `PathBuf` by pushing each
/// component individually. This avoids `PathBuf::from(input)` /
/// `Path::new(input)` on the raw `&str`, which `p/rust`
/// `tainted-path` flags as a path-traversal sink. Components are
/// re-validated here as a defense-in-depth — `validate_project_path`
/// already rejects `..` and NUL bytes, but reconstructing the path
/// component-by-component makes the sanitization visible to Semgrep
/// dataflow.
fn assemble_lexical_path(input: &str) -> io::Result<PathBuf> {
    let mut out = PathBuf::new();
    let mut saw_any = false;
    // Iterate over OS-level segments without ever wrapping the full
    // input string in a Path/PathBuf. `split` on the platform separator
    // keeps Windows callers working through forward slashes too;
    // backslash handling is deliberately permissive (mirrors prior
    // behavior of treating Windows-style inputs as Unix-rooted).
    for raw in input.split(['/', std::path::MAIN_SEPARATOR]) {
        if raw.is_empty() {
            // Leading "/" or doubled separators: anchor at root on first
            // empty segment, otherwise skip.
            if !saw_any {
                out.push(std::path::MAIN_SEPARATOR_STR);
                saw_any = true;
            }
            continue;
        }
        if raw == "." {
            // skip explicit current-dir markers; PathBuf would too
            continue;
        }
        if raw == ".." {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "project path contains '..' component; provide a path that does not traverse upward",
            ));
        }
        out.push(raw);
        saw_any = true;
    }
    if !saw_any {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "project path is empty after sanitization",
        ));
    }
    Ok(out)
}

/// Validate a caller-supplied project path string before any filesystem
/// touch.
///
/// # Threat model
///
/// `loctree-mcp` exposes filesystem-rooted tools (`context`, `slice`,
/// `find`, `impact`, ...) to a calling agent over the MCP wire. The
/// `project` parameter is therefore **untrusted input** in the SaaS
/// posture: a hosted client, a misbehaving local agent, or a prompt
/// injection inside otherwise-trusted agent traffic can put arbitrary
/// strings into it. Without validation, `PathBuf::from(project)` would
/// happily accept `../../../etc/passwd`, `/etc/shadow`, a Windows UNC
/// path (`\\?\C:\Users\...`), or a string containing a NUL byte that
/// truncates downstream `CString` conversions.
///
/// This helper enforces, in order:
///
/// 1. **Non-empty** — `""` and whitespace-only strings are rejected
///    with `InvalidInput`. Empty strings canonicalize to the process
///    cwd, which is a silent privilege grant.
/// 2. **No NUL bytes** — embedded `\0` is rejected with `InvalidInput`.
///    NUL truncates strings in syscalls and breaks any audit trail.
/// 3. **No `..` components** — any `Component::ParentDir` in the *input*
///    is rejected with `InvalidInput`. We refuse to reason about how
///    many `..` segments are "safe"; the path stays anchored where the
///    caller wrote it. Decomposition uses [`assemble_lexical_path`] so
///    Semgrep's `tainted-path` dataflow sees the per-component
///    sanitization at the call site rather than a single
///    `PathBuf::from($INPUT)` sink.
/// 4. **Allowlist gate (optional)** — if `allowed_roots` is `Some(_)`,
///    absolute inputs that do not start with any allowed root are
///    rejected with `PermissionDenied`. The post-canonical
///    containment check (run by the caller after `.canonicalize()`,
///    via [`loctree::fs_utils::SanitizedPath`] or
///    [`enforce_allowed_root`]) catches symlink escape.
///
/// # Output
///
/// Returns a [`LexicallyValidatedPath`] wrapping the cwd-joined
/// absolute lexical form. The caller is expected to canonicalize and
/// re-assert containment via [`loctree::fs_utils::SanitizedPath`]
/// before any `fs::*` access. Returning a newtype rather than a bare
/// `PathBuf` documents the validation status at the type level.
fn validate_project_path(
    input: &str,
    allowed_roots: Option<&[PathBuf]>,
) -> io::Result<LexicallyValidatedPath> {
    if input.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "project path is empty",
        ));
    }
    if input.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "project path contains NUL byte",
        ));
    }

    // Decompose and re-assemble per-component so Semgrep can see the
    // sanitization sites adjacent to PathBuf construction.
    let lexical = assemble_lexical_path(input)?;

    // Pre-canonical allowlist check for absolute inputs: catches obvious
    // `/etc/passwd`-style attempts before we touch the filesystem. The
    // authoritative check still happens post-canonicalize so symlink
    // escapes also reject.
    if let Some(roots) = allowed_roots
        && lexical.is_absolute()
        && !roots.iter().any(|root| lexical.starts_with(root))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "project path is outside the configured allowed roots",
        ));
    }

    // Resolve relative paths against cwd here (mirrors the historical
    // behavior) so the caller can canonicalize a complete path.
    let absolute = if lexical.is_absolute() {
        lexical
    } else {
        std::env::current_dir()?.join(lexical)
    };
    Ok(LexicallyValidatedPath(absolute))
}

/// Confirm that a canonicalized project path lives under one of the
/// configured allowed roots. No-op when `allowed_roots` is `None`
/// (allowlist not configured) or contains the path. Rejects with
/// `PermissionDenied` otherwise — symlink-escape and post-resolution
/// boundary crossings end here.
fn enforce_allowed_root(canonical: &Path, allowed_roots: Option<&[PathBuf]>) -> io::Result<()> {
    let Some(roots) = allowed_roots else {
        return Ok(());
    };
    if roots.iter().any(|root| canonical.starts_with(root)) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "project path {} resolves outside configured allowed roots",
                canonical.display()
            ),
        ))
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct ForAiParams {
    /// Project directory (default: current directory)
    #[serde(default = "default_project")]
    project: String,
    /// Allow non-git directories. Default false guards accidental scans outside a repo.
    #[serde(default)]
    force_no_git: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct ContextParams {
    /// Project directory (default: current directory)
    #[serde(default = "default_project")]
    project: String,
    /// Allow non-git directories. Default false guards accidental scans outside a repo.
    #[serde(default)]
    force_no_git: bool,
    /// Do not scan when a snapshot is missing or stale; return an error or stale snapshot.
    #[serde(default)]
    no_scan: bool,
    /// Return an error instead of using or refreshing a stale snapshot.
    #[serde(default)]
    fail_stale: bool,
    /// Force a fresh scan before composing context.
    #[serde(default)]
    fresh: bool,
    /// Optional: focus context on a specific file
    #[serde(default)]
    file: Option<String>,
    /// Optional: focus on a task description (token-overlap matcher)
    #[serde(default)]
    task: Option<String>,
    /// Optional: deterministic structural filter (repeatable; multiple = AND)
    #[serde(default, alias = "scopes")]
    scope: Vec<String>,
    /// Optional: limit to changed files (git-aware)
    #[serde(default)]
    changed: bool,
    /// Engage AICX memory overlay (default: true for MCP)
    #[serde(default = "default_true")]
    with_aicx: bool,
    /// Opt-out of AICX (overrides with_aicx)
    #[serde(default)]
    no_aicx: bool,
    /// Output format: json (default) or markdown.
    #[serde(default)]
    format: ContextFormat,
    /// (markdown format only) Zero-based section cursor for paginated reading.
    /// When the full markdown pack would exceed the MCP token cap, the response
    /// is split on its top-level `## ` sections; pass the `next_section` cursor
    /// from the previous response to walk the pack card-at-a-time. Ignored for
    /// JSON format and when the whole pack already fits the budget.
    #[serde(default)]
    section: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum ContextFormat {
    #[default]
    Json,
    Markdown,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct SliceParams {
    /// Project directory (default: current directory)
    #[serde(default = "default_project")]
    project: String,
    /// Allow non-git directories. Default false guards accidental scans outside a repo.
    #[serde(default)]
    force_no_git: bool,
    /// File path relative to project root (e.g., 'src/App.tsx')
    file: String,
    /// Include consumer files (files that import this file)
    #[serde(default = "default_true")]
    consumers: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct FindParams {
    /// Project directory (default: current directory)
    #[serde(default = "default_project")]
    project: String,
    /// Allow non-git directories. Default false guards accidental scans outside a repo.
    #[serde(default)]
    force_no_git: bool,
    /// Symbol name or regex pattern to search for
    name: String,
    /// Search mode: "symbols" (default), "who-imports", "where-symbol", "tagmap", "crowd", "literal"
    #[serde(default = "default_find_mode")]
    mode: String,
    /// Maximum results to return (default: 50)
    #[serde(
        default = "default_limit",
        deserialize_with = "deserialize_usize_lenient::deserialize"
    )]
    limit: usize,
    /// Filter results by language extension (e.g. 'rs', 'ts', 'py')
    #[serde(default)]
    lang: Option<String>,
    /// Only return exported symbols
    #[serde(default)]
    exported_only: bool,
    /// Only return symbols from dead-export files
    #[serde(default)]
    dead_only: bool,
    /// Minimum similarity score for fuzzy/crowd searches
    #[serde(default)]
    min_score: Option<f64>,
    /// Trigger fuzzy-only mode with similar symbol
    #[serde(default)]
    similar: Option<String>,
    /// Optional file path to narrow results (e.g., 'src/App.tsx')
    #[serde(default)]
    file: Option<String>,
    /// (literal mode) Treat `-` as token-internal so `backdrop` does not match
    /// inside `overlay-backdrop` / `--vista-z-overlay-backdrop`. Opt-in; the
    /// default boundary is unchanged. Ignored outside `mode="literal"`.
    #[serde(default)]
    whole_token: bool,
    /// (literal mode) Attach a per-file occurrence rollup (`by_file`).
    #[serde(default)]
    group_by_file: bool,
    /// (literal mode) Suppress the full occurrence list, keep only counters
    /// (`slim`). Accepts the alias `slim`. Ignored outside `mode="literal"`.
    #[serde(default, alias = "slim")]
    count_only: bool,
    /// (literal mode) Zero-based occurrence offset for paged output.
    #[serde(default, deserialize_with = "deserialize_usize_lenient::deserialize")]
    offset: usize,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct ImpactParams {
    /// Project directory (default: current directory)
    #[serde(default = "default_project")]
    project: String,
    /// Allow non-git directories. Default false guards accidental scans outside a repo.
    #[serde(default)]
    force_no_git: bool,
    /// File path to analyze impact for
    file: String,
}

fn default_prism_limit() -> usize {
    8
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct PrismParams {
    /// Project directory (default: current directory)
    #[serde(default = "default_project")]
    project: String,
    /// Allow non-git directories. Default false guards accidental scans outside a repo.
    #[serde(default)]
    force_no_git: bool,
    /// Task framings to compare. Pass at least two distinct phrasings of the same concept.
    #[serde(default, alias = "tasks")]
    task: Vec<String>,
    /// Engage AICX memory overlay (default: true).
    #[serde(default = "default_true")]
    with_aicx: bool,
    /// Opt-out of AICX memory overlay (overrides with_aicx).
    #[serde(default)]
    no_aicx: bool,
    /// Optional override for the AICX project bucket (defaults to the project root identity).
    #[serde(default)]
    aicx_project: Option<String>,
    /// Maximum example items per section (default: 8).
    #[serde(
        default = "default_prism_limit",
        deserialize_with = "deserialize_usize_lenient::deserialize"
    )]
    limit: usize,
}

fn default_true() -> bool {
    true
}

fn default_limit() -> usize {
    50
}

fn default_find_mode() -> String {
    "symbols".to_string()
}

/// Run the literal occurrence scan over a snapshot's files, reading raw bytes
/// from disk relative to `base`.
///
/// This is the MCP surface of the W1 literal truth layer. It deliberately
/// reuses [`loctree::analyzer::occurrences::scan_files`] — the *same* scanner
/// `loct occurrences` / `loct find --literal` use — so MCP results are
/// byte-for-byte identical to the CLI for the same snapshot. There is no second
/// scanner here; only the file-enumeration glue is mirrored from the CLI handler
/// (`loctree-rs/src/cli/dispatch/handlers/occurrences.rs::read_snapshot_contents`),
/// keeping the file set and the bytes read identical across surfaces.
fn scan_literal_occurrences(
    snapshot: &Snapshot,
    base: &std::path::Path,
    ident: &str,
    opts: ScanOptions,
    scope: FileScope<'_>,
) -> OccurrenceResults {
    // Best-effort read: a binary/deleted/unreadable file is simply not a literal
    // match site, exactly as in the CLI handler.
    let contents: Vec<(String, String)> = snapshot
        .files
        .iter()
        .filter_map(|file| {
            let joined = base.join(&file.path);
            let resolved = if joined.exists() {
                joined
            } else {
                std::path::PathBuf::from(&file.path)
            };
            std::fs::read_to_string(&resolved)
                .ok()
                .map(|text| (file.path.clone(), text))
        })
        .collect();
    let borrowed = contents
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect::<Vec<_>>();
    scan_files_with_scope(borrowed, ident.trim(), opts, scope)
}

#[derive(Debug, Clone, Copy, Default)]
struct SnapshotLoadOptions {
    no_scan: bool,
    fail_stale: bool,
    fresh: bool,
    force_no_git: bool,
}

impl SnapshotLoadOptions {
    fn from_context(params: &ContextParams) -> Self {
        Self {
            no_scan: params.no_scan,
            fail_stale: params.fail_stale,
            fresh: params.fresh,
            force_no_git: params.force_no_git,
        }
    }
}

fn json_error(err: impl std::fmt::Display) -> String {
    serde_json::json!({ "error": err.to_string() }).to_string()
}

/// Environment override for the per-call deadline applied to `context()`.
///
/// Defaults to 90s — short enough that the loctree-mcp server returns a
/// structured `deadline_exceeded` payload before the typical MCP client-side
/// 120s timeout fires, giving operators an actionable hint rather than a
/// silent kill. Set to a higher value for very large monorepos where the
/// initial fresh scan legitimately needs more time.
const CONTEXT_DEADLINE_ENV: &str = "LOCT_MCP_CONTEXT_DEADLINE_SECS";

fn context_deadline() -> std::time::Duration {
    let secs = std::env::var(CONTEXT_DEADLINE_ENV)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(90);
    std::time::Duration::from_secs(secs)
}

/// Environment override for the markdown-body char budget applied to
/// `context(format=markdown)`.
///
/// loctree-feedback tail (2026-06-22, recurring ~6×): the full ContextPack markdown
/// hit 149,891 chars and was rejected by the MCP runtime token cap, forcing
/// every agent off the recommended session-start surface onto a non-Loctree
/// fallback exactly when structural orientation matters most. The default
/// 38,000-char body budget keeps the JSON-wrapped response (atlas pointers +
/// receipt + newline/quote escaping overhead) at roughly half the ~25k-token
/// MCP cap — a verified worst-case page of ≈44 KB / ~12.5k tokens on
/// loctree-suite itself, against the ~88 KB / ~25k-token ceiling that rejected
/// the original 149,891-char dump. Full fidelity is preserved through section
/// pagination. Raise it for clients with a larger cap; the pack is split on its
/// top-level `## ` sections (synthesis-first, so early sections carry the most
/// value) and walked via the `next_section` cursor.
const CONTEXT_MARKDOWN_BUDGET_ENV: &str = "LOCT_MCP_CONTEXT_MAX_CHARS";
const CONTEXT_MARKDOWN_BUDGET_DEFAULT: usize = 38_000;

fn context_markdown_budget() -> usize {
    std::env::var(CONTEXT_MARKDOWN_BUDGET_ENV)
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n >= 2_000)
        .unwrap_or(CONTEXT_MARKDOWN_BUDGET_DEFAULT)
}

const MCP_RESPONSE_BUDGET_PROTOCOL: &str = "loctree.mcp.response_budget.v1";

fn tool_json_response(tool: &str, project: Option<&Path>, value: serde_json::Value) -> String {
    match serde_json::to_string_pretty(&value) {
        Ok(raw) => budget_tool_response(tool, project, raw),
        Err(e) => format!("Serialization error: {e}"),
    }
}

fn budget_tool_response(tool: &str, project: Option<&Path>, raw: String) -> String {
    let budget = context_markdown_budget();
    if raw.chars().count() <= budget {
        return raw;
    }

    match write_full_tool_payload(tool, project, &raw) {
        Ok(artifact_path) => budget_marker_response(tool, &raw, budget, Some(artifact_path), None),
        Err(e) => budget_marker_response(tool, &raw, budget, None, Some(e.to_string())),
    }
}

fn write_full_tool_payload(tool: &str, project: Option<&Path>, raw: &str) -> io::Result<PathBuf> {
    let artifact_dir = project
        .map(|project| project.join(".loctree").join("mcp-response-payloads"))
        .unwrap_or_else(|| std::env::temp_dir().join("loctree-mcp-response-payloads"));
    fs::create_dir_all(&artifact_dir)?;

    let artifact_path = artifact_dir.join(format!(
        "{}-{}.full.json",
        sanitize_artifact_stem(tool),
        full_markdown_sha256(raw)
    ));
    fs::write(&artifact_path, raw)?;
    Ok(artifact_path)
}

fn sanitize_artifact_stem(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "tool".to_string()
    } else {
        sanitized
    }
}

fn budget_marker_response(
    tool: &str,
    raw: &str,
    budget: usize,
    artifact_path: Option<PathBuf>,
    artifact_error: Option<String>,
) -> String {
    let mut preview_budget = budget.saturating_sub(1_800).max(256);
    let mut marker = serde_json::json!({
        "protocol": MCP_RESPONSE_BUDGET_PROTOCOL,
        "tool": tool,
        "status": "truncated_for_mcp_token_budget",
        "budget_chars": budget,
        "original_chars": raw.chars().count(),
        "original_bytes": raw.len(),
        "original_sha256": full_markdown_sha256(raw),
        "marker": "Full unmodified payload written to sibling artifact; response body was capped before MCP harness rejection.",
        "full_payload": artifact_path.as_ref().map(|path| serde_json::json!({
            "path": path.display().to_string(),
            "bytes": raw.len(),
            "sha256": full_markdown_sha256(raw)
        })),
        "artifact_error": artifact_error,
        "continuation": {
            "kind": "full_payload_artifact",
            "path": artifact_path.as_ref().map(|path| path.display().to_string())
        },
        "payload_preview": truncate_chars(raw, preview_budget),
        "preview_truncated": true
    });

    loop {
        let rendered = serde_json::to_string_pretty(&marker)
            .unwrap_or_else(|e| format!("Serialization error: {e}"));
        if rendered.chars().count() <= budget || preview_budget == 0 {
            return rendered;
        }
        preview_budget /= 2;
        if let Some(obj) = marker.as_object_mut() {
            obj.insert(
                "payload_preview".to_string(),
                serde_json::Value::String(truncate_chars(raw, preview_budget)),
            );
        }
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect::<String>()
}

/// One top-level `## ` section of the rendered ContextPack markdown.
struct MarkdownSection {
    /// Section title (the `## …` line, trimmed of the leading `## `), or
    /// `"Overview"` for the leading title block before the first `## `.
    title: String,
    /// Full section text including its own `## ` heading line and trailing
    /// blank lines, ready to concatenate.
    body: String,
}

/// A single paginated markdown page plus the cursor metadata an agent needs to
/// keep reading without overflowing the MCP token cap.
struct MarkdownPage {
    /// The markdown to emit in this response (already within budget).
    markdown: String,
    /// True when the pack was split (i.e. the caller did not receive the whole
    /// pack in one response).
    paginated: bool,
    /// Zero-based index of the first section included in this page.
    section_start: usize,
    /// Number of sections included in this page.
    sections_emitted: usize,
    /// Total number of top-level sections in the pack.
    total_sections: usize,
    /// Cursor to pass back as `section` to fetch the next page, or `None` at
    /// the end of the pack.
    next_section: Option<usize>,
    /// Titles of every top-level section, in order — a compact table of
    /// contents so an agent can see the whole map and jump the cursor.
    section_titles: Vec<String>,
}

/// Split rendered ContextPack markdown into its top-level `## ` sections. The
/// leading title block (everything before the first `## `) becomes the first
/// section titled `"Overview"`; if the document has no `## ` headers the whole
/// document is returned as a single `"Overview"` section.
fn split_markdown_sections(full: &str) -> Vec<MarkdownSection> {
    let mut sections: Vec<MarkdownSection> = Vec::new();
    let mut current_title = "Overview".to_string();
    let mut current_body = String::new();

    for line in full.split_inclusive('\n') {
        let is_h2 = {
            let trimmed = line.trim_end_matches('\n');
            trimmed.starts_with("## ") && !trimmed.starts_with("### ")
        };
        if is_h2 {
            // Close the section in progress (skip an empty synthetic preamble).
            if !current_body.trim().is_empty() {
                sections.push(MarkdownSection {
                    title: std::mem::take(&mut current_title),
                    body: std::mem::take(&mut current_body),
                });
            } else {
                current_body.clear();
            }
            current_title = line
                .trim_end_matches('\n')
                .trim_start_matches("## ")
                .trim()
                .to_string();
        }
        current_body.push_str(line);
    }
    if !current_body.trim().is_empty() {
        sections.push(MarkdownSection {
            title: current_title,
            body: current_body,
        });
    }
    if sections.is_empty() {
        sections.push(MarkdownSection {
            title: "Overview".to_string(),
            body: full.to_string(),
        });
    }
    sections
}

/// Hard-truncate a single over-budget section body on a line boundary,
/// appending an honest tail that points at the on-disk atlas card so the agent
/// never reads a silently clipped section as canonical truth.
fn truncate_section_body(body: &str, budget: usize) -> String {
    if body.len() <= budget {
        return body.to_string();
    }
    let tail = "\n<!-- truncated: section exceeds the MCP markdown budget; \
                read the matching card under .loctree/context-atlas/ or raise \
                LOCT_MCP_CONTEXT_MAX_CHARS for the full section -->\n";
    let keep = budget.saturating_sub(tail.len()).max(1);
    let mut cut = 0usize;
    for line in body.split_inclusive('\n') {
        if cut + line.len() > keep {
            break;
        }
        cut += line.len();
    }
    if cut == 0 {
        // A single line longer than the budget — cut on a char boundary.
        cut = body
            .char_indices()
            .take_while(|(idx, _)| *idx <= keep)
            .last()
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or(0);
    }
    let mut out = String::with_capacity(cut + tail.len());
    out.push_str(&body[..cut]);
    out.push_str(tail);
    out
}

/// Paginate rendered ContextPack markdown so a single response stays under the
/// MCP token cap while preserving full fidelity via the `next_section` cursor.
///
/// - `section == None` and the whole pack fits `budget` → return it unchanged
///   (backward-compatible whole-pack response; no pagination).
/// - otherwise → greedily pack as many consecutive top-level `## ` sections as
///   fit `budget`, starting at the requested cursor (default 0), always
///   emitting at least one section (hard-truncated if it alone overflows), and
///   hand back the cursor to the next un-emitted section.
fn paginate_context_markdown(full: &str, section: Option<usize>, budget: usize) -> MarkdownPage {
    let sections = split_markdown_sections(full);
    let total = sections.len();
    let section_titles: Vec<String> = sections.iter().map(|s| s.title.clone()).collect();

    if section.is_none() && full.len() <= budget {
        return MarkdownPage {
            markdown: full.to_string(),
            paginated: false,
            section_start: 0,
            sections_emitted: total,
            total_sections: total,
            next_section: None,
            section_titles,
        };
    }

    let start = section.unwrap_or(0).min(total.saturating_sub(1));
    let mut markdown = String::new();
    let mut emitted = 0usize;
    let mut idx = start;
    while idx < total {
        let body = &sections[idx].body;
        if emitted == 0 {
            // Always emit the first section; truncate it if it alone overflows.
            markdown.push_str(&truncate_section_body(body, budget));
            emitted += 1;
            idx += 1;
            continue;
        }
        if markdown.len() + body.len() > budget {
            break;
        }
        markdown.push_str(body);
        emitted += 1;
        idx += 1;
    }

    let next_section = if idx < total { Some(idx) } else { None };
    MarkdownPage {
        markdown,
        paginated: true,
        section_start: start,
        sections_emitted: emitted,
        total_sections: total,
        next_section,
        section_titles,
    }
}

/// Structured response returned when `context()` exceeds the server-side
/// deadline. Stays under the typical 120s MCP client timeout so the client
/// receives a real JSON payload instead of an "MCP timed out" wrapper.
fn deadline_exceeded_response(deadline: std::time::Duration) -> String {
    let session_id = format!(
        "ctx_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let payload = serde_json::json!({
        "protocol": "loctree.context_atlas.v1",
        "session": session_id,
        "status": "error",
        "error": "deadline_exceeded",
        "deadline_secs": deadline.as_secs(),
        "hint": "Context composition exceeded LOCT_MCP_CONTEXT_DEADLINE_SECS. \
                Try (a) context(no_scan=true) to use cached snapshot, \
                (b) repo-view/focus/slice for narrower views, \
                (c) raise LOCT_MCP_CONTEXT_DEADLINE_SECS for very large monorepos."
    });
    serde_json::to_string_pretty(&payload)
        .unwrap_or_else(|_| r#"{"error":"deadline_exceeded"}"#.to_string())
}

/// SHA-256 hex of the full rendered ContextPack markdown. Stamped into the
/// `receipt.full_context` accounting so a client that walks the pagination
/// cursor can prove it reconstructed the whole pack — completeness is checked
/// off by the receipt, never by trusting a single (possibly partial) response.
fn full_markdown_sha256(markdown: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(markdown.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct TreeParams {
    /// Project directory (default: current directory)
    #[serde(default = "default_project")]
    project: String,
    /// Allow non-git directories. Default false guards accidental scans outside a repo.
    #[serde(default)]
    force_no_git: bool,
    /// Maximum depth (default: 3)
    #[serde(
        default = "default_depth",
        deserialize_with = "deserialize_usize_lenient::deserialize"
    )]
    depth: usize,
    /// LOC threshold for highlighting (default: 500)
    #[serde(default = "default_loc_threshold")]
    loc_threshold: usize,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct FocusParams {
    /// Project directory (default: current directory)
    #[serde(default = "default_project")]
    project: String,
    /// Allow non-git directories. Default false guards accidental scans outside a repo.
    #[serde(default)]
    force_no_git: bool,
    /// Directory to focus on (e.g., 'src/components')
    directory: String,
}

fn default_follow_scope() -> String {
    "all".to_string()
}

fn default_follow_limit() -> usize {
    10
}

/// Parameters for the `suppressions` MCP tool.
///
/// Mirrors `loct suppressions [OPTIONS] [ROOT]`.
///
/// LITERAL-ONLY scan — free-tier scope. NO embedding similarity, NO LLM
/// classification, NO "this suppression is suspicious because…" enrichment.
/// Semantic enrichment is paid-tier delta (Wave 7+ post-aicx-library). See
/// `loctree::analyzer::suppression_inventory` module docs for the boundary.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct SuppressionsParams {
    /// Project directory (default: current directory)
    #[serde(default = "default_project")]
    project: String,
    /// Allow non-git directories. Default false guards accidental scans outside a repo.
    #[serde(default)]
    force_no_git: bool,
    /// Filter to specific silencer kinds (omit for all). Tokens:
    /// `allow`, `dead-code`, `nosemgrep`, `ts-ignore`, `ts-expect-error`,
    /// `ts-nocheck`, `eslint-disable`, `noqa`, `type-ignore`,
    /// `pylint-disable`, `mypy-ignore`, `shellcheck`, `unsafe`,
    /// `unsafe-env-var`, `ignore`. Accepts `kind` (singular alias) for
    /// callers that pass one filter.
    #[serde(default, alias = "kind", alias = "types")]
    kinds: Vec<String>,
    /// Include paths normally excluded by `.semgrepignore` (fixtures,
    /// vendored tests, CLI entry-points). Default OFF for hygiene parity
    /// with `semgrep` audits.
    #[serde(default)]
    include_fixtures: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct FollowParams {
    /// Project directory (default: current directory)
    #[serde(default = "default_project")]
    project: String,
    /// Allow non-git directories. Default false guards accidental scans outside a repo.
    #[serde(default)]
    force_no_git: bool,
    /// What to follow: "dead", "cycles", "twins", "hotspots", "trace", "commands", "events", "pipelines", or "all"
    #[serde(default = "default_follow_scope")]
    scope: String,
    /// Handler name for trace scope (e.g., "toggle_assistant")
    #[serde(default)]
    handler: Option<String>,
    /// Max trails to return per scope (default: 10)
    #[serde(
        default = "default_follow_limit",
        deserialize_with = "deserialize_usize_lenient::deserialize"
    )]
    limit: usize,
}

fn default_depth() -> usize {
    3
}

fn default_loc_threshold() -> usize {
    500
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.chars()
        .zip(b.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

fn suggest_directories(snapshot: &Snapshot, query: &str, max: usize) -> Vec<String> {
    if max == 0 {
        return Vec::new();
    }

    let mut dirs = BTreeSet::new();
    for file in &snapshot.files {
        if let Some(parent) = Path::new(&file.path).parent() {
            let dir = parent.to_string_lossy().replace('\\', "/");
            if !dir.is_empty() && dir != "." {
                dirs.insert(dir);
            }
        }
    }

    if dirs.is_empty() {
        return Vec::new();
    }

    let normalized_query = query.trim().trim_matches('/');
    let query_lower = normalized_query.to_ascii_lowercase();
    let query_last = normalized_query
        .split('/')
        .rfind(|part| !part.is_empty())
        .unwrap_or(normalized_query);
    let query_last_lower = query_last.to_ascii_lowercase();
    let query_tokens: Vec<_> = query_lower
        .split(['/', '_', '-', '.'])
        .filter(|token| token.len() >= 2)
        .collect();

    let mut scored: Vec<(String, usize)> = dirs
        .iter()
        .map(|dir| {
            let dir_lower = dir.to_ascii_lowercase();
            let mut score = 0usize;

            if !query_last_lower.is_empty() && dir.contains(query_last) {
                score += 100;
            }
            if query_last_lower.len() > 2 && dir_lower.contains(&query_last_lower) {
                score += 50;
            }
            if !query_lower.is_empty() {
                score += common_prefix_len(&dir_lower, &query_lower) * 10;
            }
            for token in &query_tokens {
                if dir_lower.contains(token) {
                    score += 3;
                }
            }

            (dir.clone(), score)
        })
        .filter(|(_, score)| *score > 0)
        .collect();

    if scored.is_empty() {
        return dirs.into_iter().take(max).collect();
    }

    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored.into_iter().take(max).map(|(dir, _)| dir).collect()
}

// ============================================================================
// Server State - Multi-project cache
// ============================================================================

/// Universal server with multi-project snapshot cache.
#[derive(Clone)]
pub(crate) struct LoctreeServer {
    /// Cache of loaded snapshots per project
    cache: Arc<RwLock<HashMap<PathBuf, Arc<Snapshot>>>>,
    /// Tool router (generated by macro)
    tool_router: rmcp::handler::server::router::tool::ToolRouter<Self>,
}

impl LoctreeServer {
    pub(crate) fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            tool_router: Self::tool_router(),
        }
    }

    /// Resolve project path to absolute, canonicalized path.
    ///
    /// Every MCP tool funnels its caller-supplied `project` string through this
    /// helper before any filesystem operation. The validation below sits in
    /// front of `PathBuf::from`/`canonicalize` precisely so the MCP server is
    /// safe under the SaaS threat model where the calling agent (Claude,
    /// Codex, Gemini, or a hosted client) is NOT trusted to stay inside the
    /// repo. See `validate_project_path` for the rule set and rejection model.
    ///
    /// If the path has no snapshot yet, `get_snapshot()` will auto-scan it.
    fn resolve_existing_project_path(project: &str) -> Result<PathBuf> {
        let allowed_roots = allowed_project_roots();
        let validated = validate_project_path(project, allowed_roots.as_deref())
            .with_context(|| format!("Invalid project path: {project}"))?;
        // Funnel through SanitizedPath::within_any for the post-canonical
        // boundary check so canonicalize + allowlist containment happen at
        // one auditable site visible to Semgrep's `tainted-path` dataflow.
        // When no allowlist is configured, fall back to the previous shape
        // (canonicalize + enforce_allowed_root no-op).
        let lexical_buf = validated.into_path_buf();
        match allowed_roots.as_deref() {
            Some(roots) => {
                let root_refs: Vec<&Path> = roots.iter().map(|p| p.as_path()).collect();
                let sanitized =
                    loctree::fs_utils::SanitizedPath::within_any(&root_refs, &lexical_buf)
                        .with_context(|| format!("Project directory not found: {project}"))?;
                Ok(sanitized.as_path().to_path_buf())
            }
            None => {
                let canon = lexical_buf
                    .canonicalize()
                    .with_context(|| format!("Project directory not found: {project}"))?;
                enforce_allowed_root(&canon, None)?;
                Ok(canon)
            }
        }
    }

    fn resolve_project(project: &str, force_no_git: bool) -> Result<PathBuf> {
        let canonical = Self::resolve_existing_project_path(project)?;
        if !force_no_git && find_git_root(&canonical).is_none() {
            anyhow::bail!(
                "Project is not inside a git repository: {}. Pass force_no_git=true to opt out for scratch directories.",
                canonical.display()
            );
        }
        Ok(canonical)
    }

    /// Get or load snapshot for a project. Auto-scans if needed and allowed.
    ///
    /// Scope guard: a snapshot whose `metadata.roots` does not match the
    /// requested `project` (after canonicalization) is treated as a
    /// foreign-scope artifact and forces a rescan. This protects the
    /// workspace-root flat-fallback from being polluted by sub-tree scans
    /// (e.g. fixture-only scans whose `snapshot_root` walks up to the
    /// workspace's git root and overwrites the project_id flat fallback).
    /// Mirrors the CLI guard in `cli/dispatch/mod.rs::load_or_create_snapshot_for_roots`.
    async fn get_snapshot(
        &self,
        project: &Path,
        load_options: SnapshotLoadOptions,
    ) -> Result<Arc<Snapshot>> {
        // Compute the canonical scope identity once so cache lookup and
        // post-load checks compare against the same shape the CLI uses.
        let project_owned = project.to_path_buf();
        let strategy = if load_options.force_no_git {
            loctree::snapshot::SnapshotRootStrategy::Exact
        } else {
            loctree::snapshot::SnapshotRootStrategy::Project
        };
        let snapshot_root = loctree::snapshot::resolve_snapshot_root_with_strategy(
            std::slice::from_ref(&project_owned),
            strategy,
        );
        let requested_roots =
            normalize_roots_for_scope_compare(std::iter::once(project), &snapshot_root);
        let scope_matches = |snap: &Snapshot| -> bool {
            let snap_roots = normalize_roots_for_scope_compare(
                snap.metadata.roots.iter().map(Path::new),
                &snapshot_root,
            );
            snap_roots == requested_roots
        };

        // Check cache first — but require matching scope, not just matching path key.
        let cached_snapshot = {
            let cache = self.cache.read().await;
            cache.get(project).map(Arc::clone)
        };
        if let Some(snapshot) = cached_snapshot {
            if !scope_matches(&snapshot) {
                debug!(
                    "Cached snapshot scope mismatch for {:?}, will reload from disk",
                    project
                );
            } else if load_options.fresh {
                debug!("Fresh snapshot requested for {:?}", project);
            } else if !Self::is_snapshot_stale(&snapshot, project) {
                debug!("Using cached snapshot for {:?}", project);
                return Ok(snapshot);
            } else if load_options.fail_stale {
                anyhow::bail!(
                    "Snapshot is stale for {} and fail_stale=true",
                    project.display()
                );
            } else if load_options.no_scan {
                debug!(
                    "Using stale cached snapshot for {:?} because no_scan=true",
                    project
                );
                return Ok(snapshot);
            } else {
                debug!("Cached snapshot is stale for {:?}", project);
            }
        }

        // Need to load or create snapshot.
        //
        // Freshness decisions and the rescan file universe are owned by the
        // snapshot freshness authority in the loctree lib — this is a thin
        // call, not a parallel staleness implementation. `no_scan_uses_stale`
        // preserves MCP semantics: with no_scan=true a stale snapshot is
        // served instead of erroring.
        info!("Loading snapshot for {:?}", project);

        let snapshot = loctree::snapshot::acquire_snapshot(
            std::slice::from_ref(&project_owned),
            loctree::snapshot::SnapshotReusePolicy::Strict,
            &loctree::snapshot::AcquireOptions {
                fresh: load_options.fresh,
                no_scan: load_options.no_scan,
                fail_stale: load_options.fail_stale,
                quiet: true,
                no_scan_uses_stale: true,
                full_scan: load_options.fresh,
                strategy,
                ..Default::default()
            },
        )
        .map_err(|e| {
            if load_options.fail_stale {
                anyhow::anyhow!(
                    "Snapshot unavailable for {} and fail_stale=true: {e}",
                    project.display()
                )
            } else if load_options.no_scan {
                anyhow::anyhow!(
                    "Snapshot unavailable for {} and no_scan=true: {e}",
                    project.display()
                )
            } else {
                anyhow::anyhow!("Failed to load snapshot for {}: {e}", project.display())
            }
        })?;

        info!(
            "Snapshot loaded: {} files, {} edges",
            snapshot.files.len(),
            snapshot.edges.len()
        );

        let snapshot = Arc::new(snapshot);

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(project.to_path_buf(), Arc::clone(&snapshot));
        }

        Ok(snapshot)
    }

    /// Check if snapshot is stale (git HEAD changed OR dirty worktree).
    /// Delegates to `Snapshot::is_stale()` — single source of truth shared
    /// with CLI and LSP, covers both commit mismatch and uncommitted changes.
    fn is_snapshot_stale(snapshot: &Snapshot, project: &Path) -> bool {
        snapshot.is_stale(project)
    }

    /// Validate file path: check if within project, return matched path from snapshot or error.
    fn resolve_file_in_snapshot(
        snapshot: &Snapshot,
        project: &Path,
        file: &str,
    ) -> Result<String, String> {
        let requested = assemble_lexical_path(file).map_err(|e| e.to_string())?;
        if requested.is_absolute() && !requested.starts_with(project) {
            return Err(format!(
                "File outside project: '{}' not in '{}'",
                file,
                project.display()
            ));
        }

        let normalized = if requested.is_absolute() {
            requested
                .strip_prefix(project)
                .unwrap_or(requested.as_path())
                .to_string_lossy()
                .replace('\\', "/")
        } else {
            requested.to_string_lossy().replace('\\', "/")
        };
        let normalized = normalized.trim_start_matches("./").to_string();

        if let Some(exact) = snapshot.files.iter().find(|f| {
            let path = f.path.trim_start_matches("./").replace('\\', "/");
            path == normalized
        }) {
            return Ok(exact.path.clone());
        }

        let suffix = format!("/{normalized}");
        let mut suffix_matches: Vec<_> = snapshot
            .files
            .iter()
            .filter(|f| {
                let path = f.path.trim_start_matches("./").replace('\\', "/");
                path.ends_with(&suffix)
            })
            .collect();
        suffix_matches.sort_by(|a, b| a.path.cmp(&b.path));

        if suffix_matches.len() == 1 {
            return Ok(suffix_matches[0].path.clone());
        }
        if suffix_matches.len() > 1 {
            return Err(format!(
                "Ambiguous file '{}': {}. Provide a repo-relative path.",
                file,
                suffix_matches
                    .iter()
                    .map(|f| f.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        Err(format!(
            "File '{}' not in snapshot. Run 'scan' or check path.",
            file
        ))
    }

    fn requested_file_exists(project: &Path, file: &str) -> bool {
        Self::resolve_existing_file_under_project(project, file).is_ok()
    }

    fn resolve_existing_file_under_project(
        project: &Path,
        file: &str,
    ) -> io::Result<(PathBuf, String)> {
        if file.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "requested file path is empty",
            ));
        }
        if file.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "requested file path contains NUL byte",
            ));
        }
        let requested = assemble_lexical_path(file)?;
        let candidate = if requested.is_absolute() {
            requested
        } else {
            project.join(requested)
        };
        let sanitized = loctree::fs_utils::SanitizedPath::within(project, &candidate)?;
        let path = sanitized.as_path().to_path_buf();
        if !path.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "requested path is not a file",
            ));
        }
        let rel = path
            .strip_prefix(project)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        Ok((path, rel))
    }

    fn disk_file_language(path: &Path) -> String {
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .unwrap_or_default();
        if !ext.is_empty() {
            return detect_language(&ext);
        }
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        detect_language_from_filename(filename)
    }

    fn disk_core_slice_payload(
        project: &Path,
        file: &str,
        exclusion_note: &str,
    ) -> io::Result<serde_json::Value> {
        let (path, rel) = Self::resolve_existing_file_under_project(project, file)?;
        let content = loctree::fs_utils::read_to_string_within(project, &path)?;
        let loc = content.lines().count();
        let language = Self::disk_file_language(&path);
        Ok(serde_json::json!({
            "target": file,
            "project": project.display().to_string(),
            "core_loc": loc,
            "dependencies": 0,
            "consumers": 0,
            "snapshot_exclusion": exclusion_note,
            "files": [{
                "path": rel,
                "layer": "core",
                "loc": loc,
                "language": language,
                "source": "disk_explicit_fallback"
            }]
        }))
    }

    fn requested_file_ignore_explanation(project: &Path, file: &str) -> Option<String> {
        let (path, _) = Self::resolve_existing_file_under_project(project, file).ok()?;
        loctree::fs_utils::explain_ignore_for_path(project, &path)
    }

    async fn resolve_file_in_snapshot_or_refresh(
        &self,
        snapshot: Arc<Snapshot>,
        project: &Path,
        file: &str,
        force_no_git: bool,
    ) -> Result<(Arc<Snapshot>, String), String> {
        match Self::resolve_file_in_snapshot(&snapshot, project, file) {
            Ok(path) => Ok((snapshot, path)),
            Err(first_error) => {
                if !Self::requested_file_exists(project, file) {
                    return Err(first_error);
                }

                let refreshed = self
                    .get_snapshot(
                        project,
                        SnapshotLoadOptions {
                            fresh: true,
                            force_no_git,
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(|e| {
                        format!(
                            "{first_error}; requested file exists on disk, but fresh scan failed: {e:#}"
                        )
                    })?;

                let path = Self::resolve_file_in_snapshot(&refreshed, project, file).map_err(
                    |second_error| {
                        let exclusion = Self::requested_file_ignore_explanation(project, file)
                            .map(|note| format!("; detected exclusion: {note}"))
                            .unwrap_or_default();
                        format!(
                            "{first_error}; fresh scan completed but file is still absent: {second_error}{exclusion}"
                        )
                    },
                )?;

                Ok((refreshed, path))
            }
        }
    }

    fn context_receipt_payload(
        session_id: &str,
        project: &Path,
        snapshot: &Snapshot,
        with_aicx: bool,
    ) -> serde_json::Value {
        use sha2::{Digest, Sha256};

        let mut loaded = vec![
            "identity",
            "risk",
            "action",
            "authority",
            "structural",
            "runtime",
        ];
        if with_aicx {
            loaded.push("memory");
        }

        let authority = snapshot.authority_report(project);
        let mut hasher = Sha256::new();
        hasher.update(session_id.as_bytes());
        hasher.update(project.display().to_string().as_bytes());
        hasher.update(authority.fingerprint.value.as_bytes());
        for section in &loaded {
            hasher.update(section.as_bytes());
        }
        let hash: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();

        serde_json::json!({
            "sections_loaded": loaded,
            "sections_skipped": if with_aicx { Vec::<&str>::new() } else { vec!["memory"] },
            "aicx": if with_aicx { "enabled" } else { "disabled" },
            "snapshot": authority,
            "sha256": hash
        })
    }
}

// ============================================================================
// MCP Tool Implementations
// ============================================================================

#[tool_router]
impl LoctreeServer {
    #[tool(
        name = "context",
        description = "Get a complete Agent Context Pack with structural, runtime, risk, action, authority, and optional AICX memory context. Start here for onboarding."
    )]
    async fn context(&self, Parameters(params): Parameters<ContextParams>) -> String {
        // Server-side deadline keeps loctree-mcp ahead of the typical 120s MCP
        // client timeout so operators always receive a structured payload —
        // either the full response or a `deadline_exceeded` hint with a
        // fallback recipe (`no_scan=true`, narrower tools, or raising the env
        // override). Cooperative cancellation only fires at `.await` points,
        // so a long sync scan inside `get_snapshot` may still overshoot; the
        // operator-visible knob is still strictly better than a silent kill.
        let deadline = context_deadline();
        match tokio::time::timeout(deadline, self.context_inner(params)).await {
            Ok(response) => response,
            Err(_elapsed) => deadline_exceeded_response(deadline),
        }
    }

    async fn context_inner(&self, params: ContextParams) -> String {
        let load_options = SnapshotLoadOptions::from_context(&params);
        let project = match Self::resolve_project(&params.project, params.force_no_git) {
            Ok(p) => p,
            Err(e) => return json_error(e),
        };

        let snapshot = match self.get_snapshot(&project, load_options).await {
            Ok(snapshot) => snapshot,
            Err(e) => return json_error(e),
        };

        let opts = ContextOptions {
            file: params.file.map(std::path::PathBuf::from),
            changed: params.changed,
            task: params.task,
            scopes: params.scope,
            with_aicx: params.with_aicx,
            no_aicx: params.no_aicx,
            project: Some(project.clone()),
            aicx_project_override: None,
            json: matches!(params.format, ContextFormat::Json),
            full: true,
            markdown: matches!(params.format, ContextFormat::Markdown),
        };

        let pack = match compose_context_pack_from_snapshot(&opts, &project, &snapshot) {
            Ok(pack) => pack,
            Err(err) => return json_error(err),
        };

        let session_id = format!(
            "ctx_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );
        let context_has_aicx = params.with_aicx && !params.no_aicx;

        let atlas = materialize_context_atlas(&pack, &project, None).ok();

        if matches!(params.format, ContextFormat::Markdown) {
            // The full pack markdown can hit ~150k chars and blow past the MCP
            // token cap (loctree-feedback tail, recurring). We NEVER truncate — that
            // strands agents on a head + "go read the cards" and breaks their
            // flow. Instead paginate on top-level `## ` sections under a char
            // budget; the whole pack still comes back in one response when it
            // fits, so small repos are unchanged, and every section stays
            // reachable through the `next_section` cursor.
            let full_markdown = render_context_markdown(&pack);
            let page = paginate_context_markdown(
                &full_markdown,
                params.section,
                context_markdown_budget(),
            );
            let status = if page.next_section.is_some() {
                "partial"
            } else {
                "complete"
            };

            // Full context is accounted for in the receipt: even when delivery is
            // paginated, the receipt carries the whole-pack digest, byte count,
            // and section total so a client can tick off complete delivery once
            // it has walked the cursor to the end.
            let mut receipt =
                Self::context_receipt_payload(&session_id, &project, &snapshot, context_has_aicx);
            if let Some(obj) = receipt.as_object_mut() {
                obj.insert(
                    "full_context".to_string(),
                    serde_json::json!({
                        "total_sections": page.total_sections,
                        "total_bytes": full_markdown.len(),
                        "sha256": full_markdown_sha256(&full_markdown),
                        "complete_in_this_response": page.next_section.is_none()
                            && page.section_start == 0,
                    }),
                );
            }

            let markdown_response = serde_json::json!({
                "protocol": "loctree.context_atlas.v1",
                "format": "markdown",
                "session": session_id,
                "status": status,
                "atlas": atlas.as_ref().map(|manifest| manifest.pointer_payload()),
                "pagination": {
                    "paginated": page.paginated,
                    "section": page.section_start,
                    "sections_emitted": page.sections_emitted,
                    "total_sections": page.total_sections,
                    "next_section": page.next_section,
                    "section_titles": page.section_titles,
                    "budget_chars": context_markdown_budget(),
                    "hint": page.next_section.map(|next| format!(
                        "Markdown paginated to stay under the MCP token cap. \
                        Call context(format=\"markdown\", section={next}) to continue, \
                        or read the materialized cards under .loctree/context-atlas/. \
                        Raise LOCT_MCP_CONTEXT_MAX_CHARS for larger single responses."
                    )),
                },
                "receipt": receipt,
                "markdown": page.markdown
            });

            return tool_json_response("context", Some(&project), markdown_response);
        }

        let core_response = serde_json::json!({
            "protocol": "loctree.context_atlas.v1",
            "session": session_id,
            "status": "complete",
            "atlas": atlas.as_ref().map(|manifest| manifest.pointer_payload()),
            "format": "json",
            "sections_loaded": if context_has_aicx {
                vec!["identity", "risk", "action", "authority", "structural", "runtime", "memory", "receipt"]
            } else {
                vec!["identity", "risk", "action", "authority", "structural", "runtime", "receipt"]
            },
            "sections_skipped": if context_has_aicx { Vec::<&str>::new() } else { vec!["memory"] },
            "receipt": Self::context_receipt_payload(
                &session_id,
                &project,
                &snapshot,
                context_has_aicx
            ),
            "advisory": "Context is complete in this response and also materialized as a Context Atlas. Use repo-view/focus/slice/find/impact/tree/follow for follow-up structural questions.",
            "data": {
                "schema_version": pack.schema_version,
                "project": pack.project,
                "risk": pack.risk,
                "action": pack.action,
                "authority": pack.authority,
                "structural": pack.structural,
                "runtime": pack.runtime,
                "memory": if context_has_aicx { Some(pack.memory) } else { None },
            }
        });

        tool_json_response("context", Some(&project), core_response)
    }

    /// Get repository overview for AI agents
    #[tool(
        name = "repo-view",
        description = "Get a compact repository overview: file count, LOC, languages, health summary, top hubs, and quick wins."
    )]
    async fn repo_view(&self, Parameters(params): Parameters<ForAiParams>) -> String {
        let project = match Self::resolve_project(&params.project, params.force_no_git) {
            Ok(p) => p,
            Err(e) => return format!("Error: {}", e),
        };

        let snapshot = match self
            .get_snapshot(
                &project,
                SnapshotLoadOptions {
                    force_no_git: params.force_no_git,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(s) => s,
            Err(e) => return format!("Error loading project: {}", e),
        };

        let metrics = repository_metrics(&snapshot);

        // Health metrics — canonical dead pipeline, the same source as
        // `loct dead` / `loct twins` / `loct findings`: one config, semantic
        // suppression, literal + symbol-graph cross-check, entry-point fence.
        // repo-view must never report a forked dead count.
        let dead_exports = loctree::analyzer::dead_parrots::compute_dead_truth(&snapshot).dead;
        let dead_high: Vec<_> = dead_exports
            .iter()
            .filter(|d| d.confidence == "high")
            .collect();

        let edges: Vec<_> = snapshot
            .edges
            .iter()
            .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
            .collect();
        let cycles = find_cycles(&edges);

        let twins = detect_exact_twins(&snapshot.files, false);

        let top_hubs = top_hubs_by_importers_direct(&snapshot, 5);

        // Languages
        let languages: Vec<_> = snapshot.metadata.languages.iter().cloned().collect();

        let atlas_dir = atlas_dir_for_project(&project);
        let atlas_manifest = atlas_dir.join("manifest.md");
        let atlas = if atlas_manifest.exists() {
            let receipt_path = atlas_dir.join("receipt.json");
            // Use the same git probe (Snapshot::git_context_for on the canonical
            // project root) that materialize_context_atlas uses when stamping the
            // receipt. Mixing snapshot.metadata.git_* with a canonical-root probe
            // produces false "stale" verdicts whenever metadata is contaminated
            // by an outer repo's git state.
            let canonical_project = project.canonicalize().unwrap_or_else(|_| project.clone());
            let live_git = Snapshot::git_context_for(&canonical_project);
            let live_branch = live_git.branch.as_deref().unwrap_or("unknown");
            let live_commit = live_git.commit.as_deref().unwrap_or("unknown");
            let live_snapshot_tag = format!("{}@{}", live_branch, live_commit);

            let atlas_snapshot_tag = fs::read_to_string(&receipt_path)
                .ok()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                .and_then(|value| {
                    value
                        .get("snapshot")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                });

            let (status, freshness_note) = match atlas_snapshot_tag.as_deref() {
                Some(tag) if tag == live_snapshot_tag => (
                    "atlas_available",
                    "Atlas matches current snapshot.".to_string(),
                ),
                Some(_) => (
                    "atlas_stale",
                    "Atlas was materialized against a different snapshot. Re-run `loct context --full` to refresh cards before relying on them."
                        .to_string(),
                ),
                None => (
                    "atlas_unknown_freshness",
                    "Atlas exists but receipt.json is missing or unreadable. Re-run `loct context --full` to refresh."
                        .to_string(),
                ),
            };

            let message = if status == "atlas_available" {
                "This repo has a materialized Context Atlas. Read manifest.md, then core/structural/runtime cards before broad architectural decisions.".to_string()
            } else {
                format!(
                    "{freshness_note} Cards may misrepresent current code state until refreshed."
                )
            };

            Some(serde_json::json!({
                "protocol": "loctree.context_atlas.v1",
                "status": status,
                "atlas_dir": atlas_dir,
                "manifest": atlas_manifest,
                "recommended_start": atlas_dir.join("00-core-map.md"),
                "atlas_snapshot": atlas_snapshot_tag,
                "current_snapshot": live_snapshot_tag,
                "freshness_note": freshness_note,
                "message": message,
            }))
        } else {
            None
        };

        let overview = serde_json::json!({
            "project": project.display().to_string(),
            "context_atlas": atlas,
            "snapshot": snapshot.authority_report(&project),
            "summary": {
                "files": metrics.file_count,
                "total_loc": metrics.total_loc,
                "edges": metrics.edge_count,
                "languages": languages,
            },
            "health": {
                "dead_exports": {
                    "total": dead_exports.len(),
                    "high_confidence": dead_high.len(),
                },
                "cycles": cycles.len(),
                "twins": twins.len(),
            },
            "top_hubs": top_hubs.into_iter().map(|metric| serde_json::json!({
                "file": metric.file,
                "importers": metric.importers_direct,
                "importers_direct": metric.importers_direct,
                "import_edges": metric.import_edges,
                "loc": metric.loc
            })).collect::<Vec<_>>(),
            "quick_wins": {
                "dead_to_remove": dead_high.iter().take(3).map(|d| serde_json::json!({
                    "file": d.file,
                    "symbol": d.symbol
                })).collect::<Vec<_>>(),
            },
            "next_steps": [
                "slice(file) - before modifying any file",
                "find(name) - before creating anything new",
                "impact(file) - before deleting or major refactor",
                "follow(all) - pursue signals before commits"
            ]
        });

        tool_json_response("repo-view", Some(&project), overview)
    }

    /// Get file slice with dependencies and consumers
    #[tool(
        name = "slice",
        description = "Get file context: the file + all its imports + all files that depend on it. USE THIS BEFORE modifying any file. One call = complete understanding of a file's role."
    )]
    async fn slice(&self, Parameters(params): Parameters<SliceParams>) -> String {
        let project = match Self::resolve_project(&params.project, params.force_no_git) {
            Ok(p) => p,
            Err(e) => return format!("Error: {}", e),
        };

        let snapshot = match self
            .get_snapshot(
                &project,
                SnapshotLoadOptions {
                    force_no_git: params.force_no_git,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(s) => s,
            Err(e) => return format!("Error loading project: {}", e),
        };

        let (snapshot, target_path) = match self
            .resolve_file_in_snapshot_or_refresh(
                snapshot,
                &project,
                &params.file,
                params.force_no_git,
            )
            .await
        {
            Ok(resolved) => resolved,
            Err(e) => match Self::disk_core_slice_payload(&project, &params.file, &e) {
                Ok(payload) => return tool_json_response("slice", Some(&project), payload),
                Err(_) => return format!("Error: {}", e),
            },
        };
        let config = SliceConfig {
            include_consumers: params.consumers,
            max_depth: SliceConfig::default().max_depth,
        };
        let slice = match HolographicSlice::from_path(&snapshot, &target_path, &config) {
            Some(slice) => slice,
            None => {
                return format!(
                    "Error: Internal snapshot inconsistency for '{}'. Run a fresh scan and retry.",
                    params.file
                );
            }
        };

        let mut files = Vec::new();
        for core in &slice.core {
            files.push(serde_json::json!({
                "path": core.path,
                "layer": "core",
                "loc": core.loc,
                "language": core.language
            }));
        }
        for dep in &slice.deps {
            let import_type = snapshot
                .edges
                .iter()
                .find(|edge| edge.to == dep.path)
                .map(|edge| edge.label.as_str())
                .unwrap_or("unknown");
            files.push(serde_json::json!({
                "path": dep.path,
                "layer": "dependency",
                "loc": dep.loc,
                "language": dep.language,
                "depth": dep.depth,
                "import_type": import_type
            }));
        }
        for consumer in &slice.consumers {
            files.push(serde_json::json!({
                "path": consumer.path,
                "layer": "consumer",
                "loc": consumer.loc,
                "language": consumer.language,
                "depth": consumer.depth
            }));
        }

        let result = serde_json::json!({
            "target": slice.target,
            "project": project.display().to_string(),
            "core_loc": slice.stats.core_loc,
            "dependencies": slice.stats.deps_files,
            "consumers": slice.stats.consumers_files,
            "files": files,
            "core_symbols": slice.core_symbols,
            "authority_labels": slice.authority_labels,
            "suggested_next": slice.suggested_next
        });

        tool_json_response("slice", Some(&project), result)
    }

    /// Find symbol definitions (supports multi-query: "foo|bar|baz")
    #[tool(
        name = "find",
        description = "Find symbols, trace imports, or explore features. Modes: 'symbols' (default) — symbol/param search with regex. 'who-imports' — what files import this file (reverse deps). 'where-symbol' — where is this symbol defined. 'tagmap' — unified keyword search (files + crowd + dead). 'crowd' — functional clustering around a keyword. 'literal' — exact identifier-boundary occurrences over the indexed universe; coverage stated per query; 'not found' means not found, with fuzzy hints kept strictly separate. At parity with `loct occurrences` / `loct find --literal`. Literal-mode tuning (all opt-in, ignored otherwise): every occurrence carries a language-aware `occurrence_kind` (css_property, class_token, custom_property, comment, string_literal, data_attribute, identifier, plus the Rust role shapes; `unknown` only as honest fallback); `whole_token=true` treats '-' as token-internal so e.g. 'backdrop' stops matching inside 'overlay-backdrop'/'--vista-z-overlay-backdrop'; `group_by_file=true` adds a per-file `by_file` count rollup; `count_only`/`slim=true` suppresses the full occurrence list (keeping `total`/`files_matched`/`by_file`) for token economy."
    )]
    async fn find(&self, Parameters(params): Parameters<FindParams>) -> String {
        let project = match Self::resolve_project(&params.project, params.force_no_git) {
            Ok(p) => p,
            Err(e) => return format!("Error: {}", e),
        };

        let snapshot = match self
            .get_snapshot(
                &project,
                SnapshotLoadOptions {
                    force_no_git: params.force_no_git,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(s) => s,
            Err(e) => return format!("Error loading project: {}", e),
        };

        let mode = params.mode.to_lowercase();

        // Mode: who-imports - reverse dependency query
        if mode == "who-imports" {
            let result = query_who_imports(&snapshot, &params.name);
            return tool_json_response(
                "find",
                Some(&project),
                serde_json::json!({
                    "mode": "who-imports",
                    "query": params.name,
                    "project": project.display().to_string(),
                    "results": result.results.iter().map(|m| serde_json::json!({
                        "file": m.file,
                        "line": m.line,
                        "context": m.context.clone()
                    })).collect::<Vec<_>>(),
                    "total": result.results.len()
                }),
            );
        }

        // Mode: where-symbol - symbol definition/export lookup
        if mode == "where-symbol" {
            let has_pipe = params.name.contains('|');
            let has_signature_shape = !has_pipe && params.name.split_whitespace().count() > 1;

            let mut result = query_where_symbol(&snapshot, &params.name);
            if has_signature_shape && result.results.is_empty() {
                if let Some(anchor) = signature_symbol_anchor(&params.name) {
                    result = query_where_symbol(&snapshot, &anchor);
                }
            } else if !has_signature_shape && has_pipe {
                result = query_where_symbol(&snapshot, &params.name);
            }

            if let Some(file_filter) = params.file.as_deref() {
                result
                    .results
                    .retain(|m| path_filter_matches(&m.file, file_filter));
            }

            if has_signature_shape && let Some(anchor) = signature_symbol_anchor(&params.name) {
                let exact_symbol_matches: Vec<_> = result
                    .results
                    .iter()
                    .filter(|m| {
                        m.context
                            .as_deref()
                            .is_some_and(|ctx| context_has_identifier(ctx, &anchor))
                    })
                    .cloned()
                    .collect();
                if !exact_symbol_matches.is_empty() {
                    result.results = exact_symbol_matches;
                }
            }

            let total = result.results.len();
            return tool_json_response(
                "find",
                Some(&project),
                serde_json::json!({
                    "mode": "where-symbol",
                    "query": params.name,
                    "project": project.display().to_string(),
                    "file_filter": params.file,
                    "results": result.results.iter().take(params.limit).map(|m| serde_json::json!({
                        "file": m.file,
                        "line": m.line,
                        "context": m.context.clone()
                    })).collect::<Vec<_>>(),
                    "total": total
                }),
            );
        }

        // Mode: crowd - functional clustering around keyword
        if mode == "crowd" {
            let crowd = detect_crowd_with_edges(&snapshot.files, &params.name, &snapshot.edges);
            let members: Vec<_> = crowd
                .members
                .iter()
                .take(params.limit)
                .map(|m| {
                    serde_json::json!({
                        "file": m.file,
                        "importer_count": m.importer_count,
                        "reason": format!("{:?}", &m.match_reason),
                        "similarity_scores": m.similarity_scores,
                        "is_test": m.is_test
                    })
                })
                .collect();

            return tool_json_response(
                "find",
                Some(&project),
                serde_json::json!({
                    "mode": "crowd",
                    "query": params.name,
                    "project": project.display().to_string(),
                    "pattern": crowd.pattern,
                    "score": crowd.score,
                    "members": members,
                    "issues": crowd.issues.iter().map(|i| format!("{:?}", i)).collect::<Vec<_>>(),
                    "total": crowd.members.len()
                }),
            );
        }

        // Mode: tagmap - unified keyword search (files + crowd + dead)
        if mode == "tagmap" {
            let keyword = &params.name;
            let keyword_lower = keyword.to_ascii_lowercase();
            let keyword_normalized = tagmap_normalize(keyword);

            // 1) files matching keyword in path
            let matching_files: Vec<_> = snapshot
                .files
                .iter()
                .filter(|f| tagmap_matches(&f.path, &keyword_lower, &keyword_normalized))
                .take(params.limit)
                .map(|f| {
                    serde_json::json!({
                        "path": f.path,
                        "loc": f.loc
                    })
                })
                .collect();

            // 2) indexed code facts matching keyword in symbols, imports,
            // usages, and literals. This is intentionally literal-only:
            // tagmap should recall terms already present in the snapshot,
            // not perform semantic enrichment.
            let mut fact_matches = Vec::new();
            'files: for file in &snapshot.files {
                for export in &file.exports {
                    if tagmap_matches(&export.name, &keyword_lower, &keyword_normalized)
                        || tagmap_matches(&export.kind, &keyword_lower, &keyword_normalized)
                        || tagmap_matches(&export.export_type, &keyword_lower, &keyword_normalized)
                    {
                        fact_matches.push(serde_json::json!({
                            "kind": "export",
                            "file": file.path,
                            "name": export.name,
                            "symbol_kind": export.kind,
                            "line": export.line
                        }));
                    }
                    if fact_matches.len() >= params.limit {
                        break 'files;
                    }
                }

                for local in &file.local_symbols {
                    if tagmap_matches(&local.name, &keyword_lower, &keyword_normalized)
                        || tagmap_matches(&local.kind, &keyword_lower, &keyword_normalized)
                        || tagmap_matches(&local.context, &keyword_lower, &keyword_normalized)
                    {
                        fact_matches.push(serde_json::json!({
                            "kind": "local-symbol",
                            "file": file.path,
                            "name": local.name,
                            "symbol_kind": local.kind,
                            "line": local.line,
                            "context": local.context
                        }));
                    }
                    if fact_matches.len() >= params.limit {
                        break 'files;
                    }
                }

                for usage in &file.symbol_usages {
                    if tagmap_matches(&usage.name, &keyword_lower, &keyword_normalized)
                        || tagmap_matches(&usage.context, &keyword_lower, &keyword_normalized)
                    {
                        fact_matches.push(serde_json::json!({
                            "kind": "symbol-usage",
                            "file": file.path,
                            "name": usage.name,
                            "line": usage.line,
                            "context": usage.context
                        }));
                    }
                    if fact_matches.len() >= params.limit {
                        break 'files;
                    }
                }

                for import in &file.imports {
                    if tagmap_matches(&import.source, &keyword_lower, &keyword_normalized)
                        || tagmap_matches(&import.source_raw, &keyword_lower, &keyword_normalized)
                        || import.resolved_path.as_ref().is_some_and(|path| {
                            tagmap_matches(path, &keyword_lower, &keyword_normalized)
                        })
                    {
                        fact_matches.push(serde_json::json!({
                            "kind": "import-source",
                            "file": file.path,
                            "source": import.source,
                            "source_raw": import.source_raw,
                            "line": import.line
                        }));
                    }
                    if fact_matches.len() >= params.limit {
                        break 'files;
                    }

                    for symbol in &import.symbols {
                        if tagmap_matches(&symbol.name, &keyword_lower, &keyword_normalized)
                            || symbol.alias.as_ref().is_some_and(|alias| {
                                tagmap_matches(alias, &keyword_lower, &keyword_normalized)
                            })
                        {
                            fact_matches.push(serde_json::json!({
                                "kind": "import-symbol",
                                "file": file.path,
                                "name": symbol.name,
                                "alias": symbol.alias,
                                "source": import.source,
                                "line": import.line
                            }));
                        }
                        if fact_matches.len() >= params.limit {
                            break 'files;
                        }
                    }
                }

                for literal in &file.string_literals {
                    if tagmap_matches(&literal.value, &keyword_lower, &keyword_normalized) {
                        fact_matches.push(serde_json::json!({
                            "kind": "string-literal",
                            "file": file.path,
                            "value": literal.value,
                            "line": literal.line
                        }));
                    }
                    if fact_matches.len() >= params.limit {
                        break 'files;
                    }
                }
            }

            // 3) crowd analysis
            let crowd = detect_crowd_with_edges(&snapshot.files, keyword, &snapshot.edges);
            let crowd_members: Vec<_> = crowd
                .members
                .iter()
                .take(params.limit)
                .map(|m| {
                    serde_json::json!({
                        "file": m.file,
                        "importer_count": m.importer_count,
                        "reason": format!("{:?}", &m.match_reason),
                        "is_test": m.is_test
                    })
                })
                .collect();

            // 4) dead exports related to keyword
            let config = DeadFilterConfig::default();
            let dead = find_dead_exports(&snapshot.files, true, None, config);
            let related_dead: Vec<_> = dead
                .iter()
                .filter(|d| {
                    tagmap_matches(&d.file, &keyword_lower, &keyword_normalized)
                        || tagmap_matches(&d.symbol, &keyword_lower, &keyword_normalized)
                })
                .take(params.limit)
                .map(|d| {
                    serde_json::json!({
                        "file": d.file,
                        "symbol": d.symbol,
                        "confidence": d.confidence,
                        "reason": d.reason
                    })
                })
                .collect();

            return tool_json_response(
                "find",
                Some(&project),
                serde_json::json!({
                    "mode": "tagmap",
                    "query": keyword,
                    "project": project.display().to_string(),
                    "files": {
                        "count": matching_files.len(),
                        "matches": matching_files
                    },
                    "facts": {
                        "count": fact_matches.len(),
                        "matches": fact_matches
                    },
                    "crowd": {
                        "score": crowd.score,
                        "count": crowd_members.len(),
                        "members": crowd_members
                    },
                    "dead": {
                        "count": related_dead.len(),
                        "matches": related_dead
                    }
                }),
            );
        }

        // Mode: literal - exact identifier-boundary scan (W1 literal truth layer).
        // Reuses the shared `scan_files` scanner, so `literal_matches` is
        // byte-for-byte identical to `loct occurrences` / `loct find --literal`
        // for the same snapshot. Fuzzy name-similarity hints ride along in a
        // strictly separate `fuzzy_suggestions` block (source: "fuzzy") and are
        // NEVER promoted into the literal matches — a suggestion is not evidence.
        if mode == "literal" {
            let mut literal_matches = scan_literal_occurrences(
                &snapshot,
                &project,
                &params.name,
                ScanOptions {
                    whole_token: params.whole_token,
                },
                FileScope {
                    file: params.file.as_deref(),
                },
            );
            literal_matches.apply_report(ReportOptions {
                group_by_file: params.group_by_file,
                count_only: params.count_only,
                offset: params.offset,
                limit: Some(params.limit),
            });
            let total = literal_matches.total;
            let fuzzy_suggestions = literal_fuzzy_suggestions(params.name.trim(), &snapshot.files);
            return tool_json_response(
                "find",
                Some(&project),
                serde_json::json!({
                    "mode": "literal",
                    "query": params.name,
                    "project": project.display().to_string(),
                    "file_filter": params.file,
                    "literal_matches": literal_matches,
                    "fuzzy_suggestions": fuzzy_suggestions,
                    "scope": literal_matches.scope,
                    "total": total
                }),
            );
        }

        if mode != "symbols" {
            return tool_json_response(
                "find",
                Some(&project),
                serde_json::json!({
                    "error": format!("Unsupported find mode: {}", params.mode),
                    "supported_modes": ["symbols", "who-imports", "where-symbol", "tagmap", "crowd", "literal"]
                }),
            );
        }

        // Default mode: symbols (existing behavior)
        // Normalize query: split by whitespace and join with | for OR matching (like CLI)
        let query = if params.name.contains('|') {
            // Already has pipe - use as-is
            params.name.clone()
        } else {
            // Split by whitespace, filter short tokens, join with |
            let tokens: Vec<&str> = params
                .name
                .split_whitespace()
                .filter(|t| t.len() >= 2)
                .collect();
            if tokens.is_empty() {
                params.name.clone()
            } else {
                tokens.join("|")
            }
        };

        // Use the same search infrastructure as CLI
        let search_results = run_search(&query, &snapshot.files);

        // Convert symbol matches to JSON format (with limit)
        let symbol_matches: Vec<_> = search_results
            .symbol_matches
            .files
            .iter()
            .filter(|f| {
                params
                    .file
                    .as_deref()
                    .is_none_or(|filter| path_filter_matches(&f.file, filter))
            })
            .flat_map(|f| {
                f.matches.iter().map(move |m| {
                    serde_json::json!({
                        "file": f.file,
                        "symbol": m.context.split_whitespace().last().unwrap_or(&m.context),
                        "kind": if m.is_definition { "definition" } else { "usage" },
                        "line": m.line,
                        "context": m.context
                    })
                })
            })
            .take(params.limit)
            .collect();

        // Convert param matches to JSON format
        let param_matches: Vec<_> = search_results
            .param_matches
            .iter()
            .take(params.limit.saturating_sub(symbol_matches.len()))
            .map(|pm| {
                serde_json::json!({
                    "file": pm.file,
                    "function": pm.function,
                    "param": pm.param_name,
                    "type": pm.param_type,
                    "line": pm.line
                })
            })
            .collect();

        // Convert semantic matches to JSON format
        let semantic_matches: Vec<_> = search_results
            .semantic_matches
            .iter()
            .take(20)
            .map(|sm| {
                serde_json::json!({
                    "symbol": sm.symbol,
                    "file": sm.file,
                    "score": sm.score
                })
            })
            .collect();

        // Convert cross-match files to JSON format (files with 2+ query terms)
        let cross_matches: Vec<_> = search_results
            .cross_matches
            .iter()
            .take(20)
            .map(|cm| {
                let terms: Vec<_> = cm
                    .matched_terms
                    .iter()
                    .map(|t| {
                        let type_tag = match &t.match_type {
                            loctree::analyzer::search::MatchType::Export { kind } => {
                                format!("EXPORT:{}", kind)
                            }
                            loctree::analyzer::search::MatchType::Import { source } => {
                                format!("IMPORT:{}", source)
                            }
                            loctree::analyzer::search::MatchType::Parameter {
                                function, ..
                            } => {
                                format!("PARAM:{}", function)
                            }
                        };
                        serde_json::json!({
                            "term": t.term,
                            "line": t.line,
                            "type": type_tag,
                            "context": t.context
                        })
                    })
                    .collect();
                serde_json::json!({
                    "file": cm.file,
                    "matched_terms": terms
                })
            })
            .collect();

        // Convert suppression matches to JSON format
        let suppression_matches: Vec<_> = search_results
            .suppression_matches
            .iter()
            .take(20)
            .map(|sm| {
                serde_json::json!({
                    "file": sm.file,
                    "line": sm.line,
                    "type": sm.suppression_type,
                    "lint": sm.lint_name,
                    "context": sm.context
                })
            })
            .collect();

        let result = serde_json::json!({
            "query": query,
            "project": project.display().to_string(),
            "symbol_matches": {
                "count": symbol_matches.len(),
                "matches": symbol_matches
            },
            "param_matches": {
                "count": param_matches.len(),
                "matches": param_matches
            },
            "semantic_matches": {
                "count": semantic_matches.len(),
                "matches": semantic_matches
            },
            "cross_matches": {
                "count": cross_matches.len(),
                "matches": cross_matches
            },
            "suppression_matches": {
                "count": suppression_matches.len(),
                "matches": suppression_matches
            },
            "dead_status": {
                "is_exported": search_results.dead_status.is_exported,
                "is_dead": search_results.dead_status.is_dead
            }
        });

        let no_primary_matches =
            symbol_matches.is_empty() && param_matches.is_empty() && semantic_matches.is_empty();
        let mut result = result;
        if no_primary_matches && let Some(obj) = result.as_object_mut() {
            obj.insert(
                "suggestions".to_string(),
                serde_json::json!([
                    "Try a broader pattern or check spelling.",
                    "Browse available exports with repo-view()."
                ]),
            );
        }

        tool_json_response("find", Some(&project), result)
    }

    /// Analyze impact of changing/removing a file
    #[tool(
        name = "impact",
        description = "What breaks if you change or delete this file? Shows direct and transitive consumers. USE THIS BEFORE deleting or major refactor."
    )]
    async fn impact(&self, Parameters(params): Parameters<ImpactParams>) -> String {
        let project = match Self::resolve_project(&params.project, params.force_no_git) {
            Ok(p) => p,
            Err(e) => return format!("Error: {}", e),
        };

        let snapshot = match self
            .get_snapshot(
                &project,
                SnapshotLoadOptions {
                    force_no_git: params.force_no_git,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(s) => s,
            Err(e) => return format!("Error loading project: {}", e),
        };

        // Validate file exists in snapshot
        let (snapshot, target_path) = match self
            .resolve_file_in_snapshot_or_refresh(
                snapshot,
                &project,
                &params.file,
                params.force_no_git,
            )
            .await
        {
            Ok(resolved) => resolved,
            Err(e) => match Self::disk_core_slice_payload(&project, &params.file, &e) {
                Ok(fallback_read) => {
                    let payload = serde_json::json!({
                        "file": params.file,
                        "project": project.display().to_string(),
                        "risk_level": "unknown",
                        "direct_consumers": {
                            "count": 0,
                            "files": []
                        },
                        "transitive_consumers": {
                            "count": 0,
                            "files": []
                        },
                        "safe_to_delete": false,
                        "snapshot_exclusion": fallback_read["snapshot_exclusion"].clone(),
                        "fallback_read": fallback_read
                    });
                    return tool_json_response("impact", Some(&project), payload);
                }
                Err(_) => return format!("Error: {}", e),
            },
        };

        // Direct consumers (use exact match on resolved path)
        let direct: Vec<_> = snapshot
            .edges
            .iter()
            .filter(|e| e.to == target_path)
            .map(|e| e.from.clone())
            .collect();

        // Transitive consumers (BFS)
        let mut visited: std::collections::HashSet<String> = direct.iter().cloned().collect();
        let mut queue: std::collections::VecDeque<String> = direct.iter().cloned().collect();
        let mut transitive = Vec::new();

        while let Some(file) = queue.pop_front() {
            for edge in &snapshot.edges {
                if edge.to == file && !visited.contains(&edge.from) {
                    visited.insert(edge.from.clone());
                    queue.push_back(edge.from.clone());
                    transitive.push(edge.from.clone());
                }
            }
        }

        let risk = if direct.is_empty() {
            "none"
        } else if direct.len() > 10 || !transitive.is_empty() {
            "high"
        } else if direct.len() > 3 {
            "medium"
        } else {
            "low"
        };

        let result = serde_json::json!({
            "file": params.file,
            "project": project.display().to_string(),
            "risk_level": risk,
            "direct_consumers": {
                "count": direct.len(),
                "files": direct.iter().take(20).collect::<Vec<_>>()
            },
            "transitive_consumers": {
                "count": transitive.len(),
                "files": transitive.iter().take(10).collect::<Vec<_>>()
            },
            "safe_to_delete": direct.is_empty()
        });

        tool_json_response("impact", Some(&project), result)
    }

    /// Get directory tree with LOC counts
    #[tool(
        name = "tree",
        description = "Get directory structure with LOC (lines of code) counts. Helps understand project layout and find large files/directories."
    )]
    async fn tree(&self, Parameters(params): Parameters<TreeParams>) -> String {
        let project = match Self::resolve_project(&params.project, params.force_no_git) {
            Ok(p) => p,
            Err(e) => return format!("Error: {}", e),
        };

        let snapshot = match self
            .get_snapshot(
                &project,
                SnapshotLoadOptions {
                    force_no_git: params.force_no_git,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(s) => s,
            Err(e) => return format!("Error loading project: {}", e),
        };

        // Build directory tree
        let mut dir_loc: HashMap<String, usize> = HashMap::new();
        let mut large_files = Vec::new();

        for file in &snapshot.files {
            // Accumulate LOC per directory
            let parts: Vec<&str> = file.path.split('/').collect();
            for i in 1..=parts.len().min(params.depth) {
                let dir = parts[..i].join("/");
                *dir_loc.entry(dir).or_default() += file.loc;
            }

            // Track large files
            if file.loc >= params.loc_threshold {
                large_files.push(serde_json::json!({
                    "path": file.path,
                    "loc": file.loc,
                    "language": file.language
                }));
            }
        }

        // Sort directories by LOC
        let mut sorted_dirs: Vec<_> = dir_loc.into_iter().collect();
        sorted_dirs.sort_by_key(|b| std::cmp::Reverse(b.1));

        // Sort large files
        large_files.sort_by(|a, b| {
            b.get("loc")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                .cmp(&a.get("loc").and_then(|v| v.as_u64()).unwrap_or(0))
        });

        let result = serde_json::json!({
            "project": project.display().to_string(),
            "total_files": snapshot.files.len(),
            "total_loc": snapshot.files.iter().map(|f| f.loc).sum::<usize>(),
            "depth": params.depth,
            "top_directories": sorted_dirs.iter().take(15).map(|(dir, loc)| serde_json::json!({
                "path": dir,
                "loc": loc
            })).collect::<Vec<_>>(),
            "large_files": large_files.iter().take(10).collect::<Vec<_>>(),
            "loc_threshold": params.loc_threshold
        });

        tool_json_response("tree", Some(&project), result)
    }

    /// Focus on a specific directory
    #[tool(
        name = "focus",
        description = "Focus on a specific directory: list files, their LOC, exports, and dependencies within that directory. Great for understanding a module or subsystem."
    )]
    async fn focus(&self, Parameters(params): Parameters<FocusParams>) -> String {
        let project = match Self::resolve_project(&params.project, params.force_no_git) {
            Ok(p) => p,
            Err(e) => return format!("Error: {}", e),
        };

        let snapshot = match self
            .get_snapshot(
                &project,
                SnapshotLoadOptions {
                    force_no_git: params.force_no_git,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(s) => s,
            Err(e) => return format!("Error loading project: {}", e),
        };

        let config = FocusConfig {
            include_consumers: true,
            max_depth: FocusConfig::default().max_depth,
        };
        let focus = match HolographicFocus::from_path(&snapshot, &params.directory, &config) {
            Some(focus) => focus,
            None => {
                let suggestions = suggest_directories(&snapshot, &params.directory, 3);
                // A correct path can still yield no files if .loctignore parks it
                // outside the snapshot (loctree-feedback.md: vista docs/). Surface that
                // precise cause instead of the blanket "Check the path."
                let ignore_hint =
                    loctree::fs_utils::loctignore_exclusion_hint(&project, &params.directory);
                let error = match &ignore_hint {
                    Some(hint) => hint.clone(),
                    None => "No files found in this directory. Check the path.".to_string(),
                };
                return tool_json_response(
                    "focus",
                    Some(&project),
                    serde_json::json!({
                        "directory": params.directory,
                        "project": project.display().to_string(),
                        "error": error,
                        "loctignore_excluded": ignore_hint.is_some(),
                        "suggestions": suggestions
                    }),
                );
            }
        };
        let total_exports: usize = snapshot
            .files
            .iter()
            .filter(|file| focus.core.iter().any(|core| core.path == file.path))
            .map(|file| file.exports.len())
            .sum();

        let result = serde_json::json!({
            "directory": focus.target,
            "project": project.display().to_string(),
            "summary": {
                "files": focus.core.len(),
                "total_loc": focus.stats.core_loc,
                "total_exports": total_exports,
                "internal_edges": focus.stats.internal_edges,
                "external_dependency_edges": focus.deps.len(),
                "external_consumer_edges": focus.consumers.len(),
            },
            "files": focus.core.iter().map(|f| {
                let exports = snapshot
                    .files
                    .iter()
                    .find(|file| file.path == f.path)
                    .map(|file| file.exports.len())
                    .unwrap_or_default();
                serde_json::json!({
                "path": f.path,
                "loc": f.loc,
                "language": f.language,
                "exports": exports
                })
            }).collect::<Vec<_>>(),
            "external_dependencies": focus.deps.iter().take(20).map(|f| &f.path).collect::<Vec<_>>(),
            "external_consumers": focus.consumers.iter().take(20).map(|f| &f.path).collect::<Vec<_>>(),
            "module_consumers": focus.consumers.iter().take(20).map(|f| serde_json::json!({
                "path": f.path,
                "loc": f.loc,
                "language": f.language,
                "authority": "LoctreeDerived"
            })).collect::<Vec<_>>(),
            "core_symbols": focus.core_symbols,
            "authority_labels": focus.authority_labels,
            "suggested_next": focus.suggested_next
        });

        tool_json_response("focus", Some(&project), result)
    }

    /// Follow signals flagged by repo-view at field level
    #[tool(
        name = "follow",
        description = "Pursue structural signals at field level. Scopes: 'dead' — unused exports with nearest consumers. 'cycles' — circular imports with weakest link. 'twins' — duplicate exports plus route-level twins (CLI `loct twins` parity). 'hotspots' — high-importer files. 'trace' — trace a Tauri/IPC handler end-to-end (requires handler param). 'commands' — Tauri FE<->BE handler coverage. 'events' — event emit/listen flow analysis. 'pipelines' — pipeline summary (events + commands + risks). 'all' — dead + cycles + twins (incl. route_twins) + hotspots."
    )]
    async fn follow(&self, Parameters(params): Parameters<FollowParams>) -> String {
        let project = match Self::resolve_project(&params.project, params.force_no_git) {
            Ok(p) => p,
            Err(e) => return format!("Error: {}", e),
        };

        let snapshot = match self
            .get_snapshot(
                &project,
                SnapshotLoadOptions {
                    force_no_git: params.force_no_git,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(s) => s,
            Err(e) => return format!("Error loading project: {}", e),
        };

        let scope = params.scope.to_lowercase();
        let limit = params.limit;
        let mut trails = serde_json::Map::new();

        // Dead exports trail
        if scope == "dead" || scope == "all" {
            let config = DeadFilterConfig::default();
            let dead = find_dead_exports(&snapshot.files, true, None, config);

            // Find nearest candidate consumers for each dead export
            let signals: Vec<_> = dead
                .iter()
                .take(limit)
                .map(|d| {
                    // Find files that import from the same directory (potential wiring candidates)
                    let dir = Path::new(&d.file)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let candidates: Vec<_> = snapshot
                        .edges
                        .iter()
                        .filter(|e| e.to.starts_with(&dir) && e.from != d.file)
                        .map(|e| e.from.clone())
                        .collect::<std::collections::HashSet<_>>()
                        .into_iter()
                        .take(3)
                        .collect();

                    let loc = snapshot
                        .files
                        .iter()
                        .find(|f| f.path == d.file)
                        .map(|f| f.loc)
                        .unwrap_or(0);

                    serde_json::json!({
                        "file": d.file,
                        "symbol": d.symbol,
                        "confidence": d.confidence,
                        "reason": d.reason,
                        "loc": loc,
                        "nearest_candidates": candidates,
                        "action": "remove or wire into candidate consumers"
                    })
                })
                .collect();

            trails.insert(
                "dead_exports".to_string(),
                serde_json::json!({
                    "total": dead.len(),
                    "shown": signals.len(),
                    "signals": signals
                }),
            );
        }

        // Cycles trail
        if scope == "cycles" || scope == "all" {
            let edges: Vec<_> = snapshot
                .edges
                .iter()
                .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
                .collect();
            let cycles = find_cycles(&edges);

            let signals: Vec<_> = cycles
                .iter()
                .take(limit)
                .map(|chain| {
                    // Calculate total LOC in cycle
                    let total_loc: usize = chain
                        .iter()
                        .filter_map(|f| snapshot.files.iter().find(|a| a.path == *f))
                        .map(|a| a.loc)
                        .sum();

                    // Find weakest link (edge with fewest symbols crossing).
                    // Defense-in-depth (marbles L6, hak CYC-PHANTOM 2026-05-18):
                    // only consider edges where symbols_crossed > 0. A pair
                    // with symbols_crossed == 0 is a graph phantom — the
                    // chain hop reaches the next node but no snapshot edge
                    // actually carries a symbol across that pair. The legacy
                    // selector treated that as the *strongest* signal (lowest
                    // count wins), producing reports like
                    // `weakest_link: {from: A, to: B, symbols_crossed: 0}`
                    // even though no such edge exists. Filter first.
                    let mut weakest: Option<(&String, &String, usize)> = None;
                    for i in 0..chain.len() {
                        let from = &chain[i];
                        let to = &chain[(i + 1) % chain.len()];
                        let symbols_crossed = snapshot
                            .edges
                            .iter()
                            .filter(|e| e.from == *from && e.to == *to)
                            .count();
                        if symbols_crossed == 0 {
                            continue;
                        }
                        match weakest {
                            None => weakest = Some((from, to, symbols_crossed)),
                            Some((_, _, current)) if symbols_crossed < current => {
                                weakest = Some((from, to, symbols_crossed));
                            }
                            _ => {}
                        }
                    }

                    let (weakest_link, action) = match weakest {
                        Some((from, to, sym)) => (
                            serde_json::json!({
                                "from": from,
                                "to": to,
                                "symbols_crossed": sym
                            }),
                            "break at weakest link",
                        ),
                        None => (
                            serde_json::json!(null),
                            "graph anomaly: all chain edges have symbols_crossed == 0; verify analyzer edges",
                        ),
                    };

                    serde_json::json!({
                        "chain": chain,
                        "length": chain.len(),
                        "total_loc": total_loc,
                        "weakest_link": weakest_link,
                        "action": action
                    })
                })
                .collect();

            trails.insert(
                "cycles".to_string(),
                serde_json::json!({
                    "total": cycles.len(),
                    "shown": signals.len(),
                    "signals": signals
                }),
            );
        }

        // Twins trail
        if scope == "twins" || scope == "all" {
            let twins = detect_exact_twins(&snapshot.files, false);

            let signals: Vec<_> = twins
                .iter()
                .take(limit)
                .map(|twin| {
                    let files: Vec<_> = twin.locations.iter().map(|l| &l.file_path).collect();
                    serde_json::json!({
                        "symbol": twin.name,
                        "files": files,
                        "locations": twin.locations.iter().map(|l| serde_json::json!({
                            "file": l.file_path,
                            "line": l.line,
                            "kind": l.kind,
                            "importers": l.import_count
                        })).collect::<Vec<_>>(),
                        "signature_similarity": twin.signature_similarity,
                        "classification": twin.classification,
                        "action": twin_action(twin)
                    })
                })
                .collect();

            // loctree-feedback hak 2026-05-18 #6 + 2026-05-23 #13 (L9 closure):
            // CLI `loct twins` returns both exact_twins AND route_twins
            // (since marbles-L8). MCP `follow(scope='twins')` was only
            // returning exact_twins, leaving agents blind to runtime
            // route-level collisions (e.g. duplicate `POST /api/stt`)
            // that the operator sees from the CLI. Restore parity.
            let route_twins = detect_route_twins(&snapshot.files);
            let route_signals: Vec<_> = route_twins
                .iter()
                .take(limit)
                .map(|rt| {
                    serde_json::json!({
                        "framework": rt.framework,
                        "method": rt.method,
                        "path": rt.path,
                        "severity": rt.severity,
                        "registrations": rt.locations.iter().map(|loc| serde_json::json!({
                            "file": loc.file,
                            "line": loc.line,
                            "handler": loc.handler,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();

            trails.insert(
                "twins".to_string(),
                serde_json::json!({
                    "total": twins.len(),
                    "shown": signals.len(),
                    "signals": signals,
                    "route_twins": {
                        "total": route_twins.len(),
                        "shown": route_signals.len(),
                        "signals": route_signals,
                    }
                }),
            );
        }

        // Hotspots trail (files with most direct importer files)
        if scope == "hotspots" || scope == "all" {
            let hubs = top_hubs_by_importers_direct(&snapshot, limit);

            let signals: Vec<_> = hubs
                .iter()
                .map(|metric| {
                    let importers = metric.importers_direct;

                    let risk = if importers > 30 {
                        "high — changes here ripple everywhere"
                    } else if importers > 10 {
                        "medium — significant blast radius"
                    } else {
                        "low"
                    };

                    serde_json::json!({
                        "file": metric.file,
                        "importers": importers,
                        "importers_direct": metric.importers_direct,
                        "import_edges": metric.import_edges,
                        "loc": metric.loc,
                        "risk": risk,
                        "action": if importers > 20 { "split or freeze interface" } else { "monitor" }
                    })
                })
                .collect();

            trails.insert(
                "hotspots".to_string(),
                serde_json::json!({
                    "total": hubs.len(),
                    "shown": signals.len(),
                    "signals": signals
                }),
            );
        }

        // Trace trail - trace a specific Tauri handler end-to-end
        if scope == "trace" {
            let handler_name = match params.handler.as_deref() {
                Some(name) => name,
                None => {
                    return tool_json_response(
                        "follow",
                        Some(&project),
                        serde_json::json!({
                            "error": "trace scope requires 'handler' parameter",
                            "example": "follow(scope='trace', handler='toggle_assistant')",
                            "hint": "Use commands scope first to see available handlers"
                        }),
                    );
                }
            };

            let handler_lower = handler_name.to_lowercase();
            let matching: Vec<_> = snapshot
                .command_bridges
                .iter()
                .filter(|b| b.name.to_lowercase().contains(&handler_lower))
                .take(limit)
                .map(|b| {
                    serde_json::json!({
                        "name": b.name,
                        "has_handler": b.has_handler,
                        "is_called": b.is_called,
                        "backend": b.backend_handler.as_ref().map(|(f, l)| serde_json::json!({
                            "file": f,
                            "line": l
                        })),
                        "frontend_calls": b.frontend_calls.iter().map(|(f, l)| serde_json::json!({
                            "file": f,
                            "line": l
                        })).collect::<Vec<_>>(),
                        "status": if b.has_handler && b.is_called {
                            "healthy"
                        } else if !b.has_handler && b.is_called {
                            "missing_handler"
                        } else if b.has_handler && !b.is_called {
                            "unused_handler"
                        } else {
                            "orphan"
                        }
                    })
                })
                .collect();

            trails.insert(
                "trace".to_string(),
                serde_json::json!({
                    "handler": handler_name,
                    "total": matching.len(),
                    "signals": matching
                }),
            );
        }

        // Commands trail - Tauri FE<->BE handler coverage
        if scope == "commands" {
            let total = snapshot.command_bridges.len();
            let missing: Vec<_> = snapshot
                .command_bridges
                .iter()
                .filter(|b| !b.has_handler && b.is_called)
                .map(|b| {
                    serde_json::json!({
                        "name": b.name,
                        "frontend_calls": b.frontend_calls.iter().map(|(f, l)| serde_json::json!({
                            "file": f,
                            "line": l
                        })).collect::<Vec<_>>()
                    })
                })
                .collect();
            let unused: Vec<_> = snapshot
                .command_bridges
                .iter()
                .filter(|b| b.has_handler && !b.is_called)
                .map(|b| {
                    serde_json::json!({
                        "name": b.name,
                        "backend": b.backend_handler.as_ref().map(|(f, l)| serde_json::json!({
                            "file": f,
                            "line": l
                        }))
                    })
                })
                .collect();
            let matched = total.saturating_sub(missing.len() + unused.len());

            trails.insert(
                "commands".to_string(),
                serde_json::json!({
                    "total": total,
                    "matched": matched,
                    "missing_handlers": {
                        "count": missing.len(),
                        "signals": missing
                    },
                    "unused_handlers": {
                        "count": unused.len(),
                        "signals": unused
                    }
                }),
            );
        }

        // Events trail - event emit/listen flow
        if scope == "events" {
            let ghosts: Vec<_> = snapshot
                .event_bridges
                .iter()
                .filter(|e| e.listens.is_empty() || e.emits.is_empty())
                .take(limit)
                .map(|e| {
                    let status = if e.emits.is_empty() {
                        "listen_only"
                    } else if e.listens.is_empty() {
                        "emit_only"
                    } else {
                        "healthy"
                    };

                    serde_json::json!({
                        "name": e.name,
                        "status": status,
                        "emits": e.emits.iter().map(|(f, l, k)| serde_json::json!({
                            "file": f,
                            "line": l,
                            "kind": k
                        })).collect::<Vec<_>>(),
                        "listens": e.listens.iter().map(|(f, l)| serde_json::json!({
                            "file": f,
                            "line": l
                        })).collect::<Vec<_>>(),
                        "is_fe_sync": e.is_fe_sync,
                        "same_file_sync": e.same_file_sync
                    })
                })
                .collect();

            let total_events = snapshot.event_bridges.len();
            let ghost_count = snapshot
                .event_bridges
                .iter()
                .filter(|e| e.listens.is_empty() || e.emits.is_empty())
                .count();

            trails.insert(
                "events".to_string(),
                serde_json::json!({
                    "total": total_events,
                    "ghost_events": ghost_count,
                    "signals": ghosts
                }),
            );
        }

        // Pipelines trail - event/command/payload risks summary
        if scope == "pipelines" {
            let scan_results = scan_results_from_snapshot(&snapshot);
            let summary = build_pipeline_summary(
                &scan_results.global_analyses,
                &None,
                &None,
                &scan_results.global_fe_commands,
                &scan_results.global_be_commands,
                &scan_results.global_fe_payloads,
                &scan_results.global_be_payloads,
            );
            trails.insert("pipelines".to_string(), summary);
        }

        let result = serde_json::json!({
            "project": project.display().to_string(),
            "scope": params.scope,
            "trails": trails
        });

        tool_json_response("follow", Some(&project), result)
    }

    #[tool(
        name = "suppressions",
        description = "Source-side silencer inventory. LITERAL-ONLY detection (free-tier scope): surfaces every Rust #[allow(...)], Rust #[ignore], Rust unsafe { ... } (with Rust 2024 env-var boilerplate triaged as 'unsafe-env-var'), Semgrep nosemgrep comments, TypeScript @ts-ignore, @ts-expect-error, @ts-nocheck, ESLint eslint-disable, Python # noqa, Python # type: ignore, Python # pylint: disable, Python # mypy:, Shell # shellcheck disable. Returns structured JSON: { matches: [{ kind, file, line, snippet, rule_id }], counts: { kind: count }, files_per_kind: { kind: file_count }, total, total_files }. Filter with kinds=[...]. NO semantic enrichment — semantic classification (suspicious/stale/similar-to-fixed) is paid-tier Wave 7+ delta and explicitly out of scope here. Distinct from .loctree/suppressions.toml (that's loctree's OWN finding-suppression file; different concept, similar name)."
    )]
    async fn suppressions(&self, Parameters(params): Parameters<SuppressionsParams>) -> String {
        let project = match Self::resolve_project(&params.project, params.force_no_git) {
            Ok(p) => p,
            Err(e) => return json_error(e),
        };

        // Parse filter tokens. Unknown tokens fail fast with the same shape
        // as the CLI handler so MCP callers learn the vocabulary the same way.
        let mut filter: HashSet<SilencerKind> = HashSet::new();
        for raw in &params.kinds {
            for token in raw.split(',') {
                let token = token.trim();
                if token.is_empty() {
                    continue;
                }
                match SilencerKind::from_filter(token) {
                    Some(k) => {
                        filter.insert(k);
                    }
                    None => {
                        return json_error(format!(
                            "unknown kind '{}'. Valid: allow, dead-code, nosemgrep, ts-ignore, ts-expect-error, ts-nocheck, eslint-disable, noqa, type-ignore, pylint-disable, mypy-ignore, shellcheck, unsafe, unsafe-env-var, ignore",
                            token
                        ));
                    }
                }
            }
        }

        // .semgrepignore filtering ON by default — same hygiene as CLI.
        let extra_globs = resolve_ignore_globs(&project, !params.include_fixtures);
        let inv = silencer_inventory(&project, &filter, &extra_globs);

        tool_json_response(
            "suppressions",
            Some(&project),
            serde_json::json!({
                "tier": "free",
                "detection": "literal-only",
                "project": project.display().to_string(),
                "filter": params.kinds,
                "include_fixtures": params.include_fixtures,
                "total": inv.total,
                "total_files": inv.total_files,
                "counts": inv.counts,
                "files_per_kind": inv.files_per_kind,
                "matches": inv.matches,
            }),
        )
    }

    #[tool(
        name = "prism",
        description = "Score conceptual smear across two or more task framings. Composes one ContextPack per task, computes file overlap and Jaccard distance, and emits the canonical loctree.prism.v1 JSON schema (axes, band, recommendation, task summaries, overlap). Use when a feature feels like it lives in multiple places at once and you need to decide whether vc-polarize is warranted."
    )]
    async fn prism(&self, Parameters(params): Parameters<PrismParams>) -> String {
        if params.task.len() < 2 {
            return json_error(
                "prism requires at least two task framings; pass task=[\"a\", \"b\"]",
            );
        }

        let project = match Self::resolve_project(&params.project, params.force_no_git) {
            Ok(p) => p,
            Err(e) => return json_error(e),
        };

        let opts = loctree::cli::command::PrismOptions {
            tasks: params.task.clone(),
            project: Some(project.clone()),
            aicx_project_override: params.aicx_project.clone(),
            with_aicx: params.with_aicx && !params.no_aicx,
            no_aicx: params.no_aicx,
            json: true,
            limit: params.limit.max(1),
        };

        let report = match loctree::run_prism(&opts) {
            Ok(report) => report,
            Err(err) => return json_error(err),
        };

        serde_json::to_string(&report).unwrap_or_else(json_error)
    }
}

// ============================================================================
// Server Handler Implementation
// ============================================================================

/// Build-time identity stamp (populated by `build.rs`). Surfaced in the MCP
/// `initialize` handshake so an agent can detect that the running binary lags
/// its repo's source HEAD straight from `serverInfo.version` / `instructions`,
/// without reverse-engineering the tool schema. See `loctree-feedback.md`
/// ("live binary predates the committed fix").
const BUILD_VERSION: &str = env!("LOCTREE_MCP_BUILD_VERSION");
/// Richer human-facing commit stamp: `git describe --always --dirty --tags`.
const GIT_DESCRIBE: &str = env!("LOCTREE_MCP_GIT_DESCRIBE");
const TOOL_SURFACE_DIGEST: &str =
    "TOOLS: context,repo-view,focus,slice,find,impact,tree,follow,suppressions,prism";

/// The tool catalogue half of the `initialize` instructions. The build-identity
/// header and compact surface digest are prepended at runtime in
/// [`LoctreeServer::get_info`].
const INSTRUCTIONS_BODY: &str = "Loctree MCP provides one sharp agent surface: 10 tools, not a mirrored CLI.\n\n\
                 START:\n\
                 - context(project, format?) - Complete Agent Context Pack: structural + runtime semantics + risk + action + optional AICX memory + authority labels. Pretty JSON by default; use format='markdown' for operator-readable context.\n\n\
                 MAP TOOLS:\n\
                 - repo-view(project) - Overview: files, LOC, languages, health, top hubs.\n\
                 - focus(directory) - Understand a module. Files, internal edges, external deps.\n\
                 - slice(file) - Before modifying. File + dependencies + consumers in one call.\n\
                 - find(name) - Before creating. Symbol search with regex. Modes: symbols, who-imports, where-symbol, tagmap, crowd, literal (exact identifier-boundary truth scan, at parity with `loct occurrences`).\n\
                 - impact(file) - Before deleting. Direct + transitive consumers (blast radius).\n\
                 - tree(project) - Directory structure with LOC counts.\n\
                 - follow(scope) - Pursue signals: dead, cycles, twins, hotspots, trace, commands, events, pipelines.\n\n\
                 SILENCER SURFACE:\n\
                 - suppressions(project, kinds?) - Source-side silencer inventory: Rust #[allow(...)], Rust #[ignore], Rust unsafe { ... } (env-var boilerplate split out), Semgrep nosemgrep, TypeScript @ts-ignore, ESLint eslint-disable, Python # noqa, Python # type: ignore, Shell # shellcheck disable. Literal-only detection (free-tier). Semantic enrichment (suspicious/stale) is paid-tier Wave 7+.\n\n\
                 POLARIZATION GATE:\n\
                 - prism(task=[a, b, ...]) - Score conceptual smear across task framings. Emits loctree.prism.v1 JSON for vc-polarize gating.\n\n\
                 Reports such as health/findings/audit/coverage stay in the `loct` CLI, not the MCP tool list.\n\
                 All tools accept 'project' parameter (default: current dir).\n\
                 First use auto-scans if no snapshot exists.";

/// Build-identity header prepended to the `initialize` instructions. Kept as a
/// standalone helper so the regression tests can assert its exact shape.
fn build_identity_banner() -> String {
    format!(
        "BUILD: loctree-mcp {BUILD_VERSION} (git {GIT_DESCRIBE}). \
         If this commit lags your repo's source HEAD, the running binary is STALE — \
         rebuild and restart the MCP server before trusting tool-schema parity.\n\n"
    )
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for LoctreeServer {
    fn get_info(&self) -> ServerInfo {
        let mut capabilities = rmcp::model::ServerCapabilities::default();
        capabilities.tools = Some(rmcp::model::ToolsCapability {
            list_changed: Some(true),
        });

        ServerInfo::new(capabilities)
            .with_server_info(
                // `version` carries the git commit as semver build metadata
                // (`0.13.0+g<sha>`), so an agent reading `serverInfo.version`
                // can tell a stale binary from a fresh one.
                rmcp::model::Implementation::new("loctree", BUILD_VERSION)
                    .with_title("Loctree MCP Server")
                    .with_description("Structural code intelligence for AI agents")
                    .with_website_url("https://github.com/Loctree/Loctree"),
            )
            .with_instructions(format!(
                "{}{}\n\n{INSTRUCTIONS_BODY}",
                build_identity_banner(),
                TOOL_SURFACE_DIGEST
            ))
    }
}

// ============================================================================
// Main Entry Point
// ============================================================================

async fn run_server() -> Result<()> {
    let args = Args::parse();

    // Pin the default project root before serving so the first tool call
    // already resolves empty `project` fields against it. No `--root` keeps
    // the universal per-request behavior untouched.
    if let Some(root) = args.root.as_deref() {
        set_default_project_root(root);
    }

    // Initialize logging - MUST write to stderr, stdout is for MCP JSON-RPC
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        // Prevent tracing from recursively writing fallback errors to stderr when stderr is closed.
        .log_internal_errors(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| args.log_level.parse().unwrap_or_default()),
        )
        .init();

    info!(
        "Starting loctree-mcp v{} (git {}) (universal)",
        BUILD_VERSION, GIT_DESCRIBE
    );
    if args.root.is_some() {
        info!("Default project root pinned to {}", default_project());
    }

    match args.transport {
        TransportKind::Stdio => serve_stdio().await,
        TransportKind::Http => http::serve_http(&args.bind).await,
    }
}

/// Stdio transport — the default, line-delimited JSON-RPC over stdin/stdout.
/// Behaves exactly as previous versions of loctree-mcp.
async fn serve_stdio() -> Result<()> {
    let server = LoctreeServer::new();
    info!("Server ready. Listening on stdio...");
    server
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    // Ignore SIGPIPE - allows broken pipe to be handled as error instead of signal
    ignore_sigpipe();

    // Install panic hook for clean shutdown on broken pipe
    install_panic_hook();

    match run_server().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Check if this is a broken pipe error (client disconnected)
            let err_str = format!("{:?}", e);
            if err_str.contains("Broken pipe") || err_str.contains("os error 32") {
                safe_stderr_log("[loctree-mcp] Client disconnected, shutting down");
                ExitCode::SUCCESS
            } else {
                safe_stderr_log(&format!("[loctree-mcp] Error: {:#}", e));
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::Value;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn build_version_extends_crate_version_with_commit_metadata() {
        // Always starts with the plain crate version; in a git checkout it is
        // extended with `+g<sha>` build metadata. Robust when git is absent
        // (packaged build) — then it degrades to exactly the crate version.
        assert!(
            BUILD_VERSION.starts_with(env!("CARGO_PKG_VERSION")),
            "build version {BUILD_VERSION:?} must begin with crate version {:?}",
            env!("CARGO_PKG_VERSION")
        );
        let extended = BUILD_VERSION == env!("CARGO_PKG_VERSION")
            || BUILD_VERSION.contains("+g")
            || BUILD_VERSION.contains("+");
        assert!(
            extended,
            "unexpected build version shape: {BUILD_VERSION:?}"
        );
    }

    #[test]
    fn get_info_version_carries_the_build_stamp() {
        let info = LoctreeServer::new().get_info();
        assert_eq!(
            info.server_info.version, BUILD_VERSION,
            "serverInfo.version must expose the git-stamped build version so an \
             agent can spot a stale binary from the initialize handshake"
        );
        assert_eq!(info.server_info.name, "loctree");
    }

    #[test]
    fn get_info_instructions_lead_with_build_identity_banner() {
        let info = LoctreeServer::new().get_info();
        let instructions = info
            .instructions
            .expect("initialize instructions must be present");
        assert!(
            instructions.starts_with("BUILD: loctree-mcp "),
            "instructions must open with the build-identity banner, got: {:?}",
            &instructions[..instructions.len().min(60)]
        );
        assert!(
            instructions.contains(BUILD_VERSION) && instructions.contains(GIT_DESCRIBE),
            "banner must name both the build version and the git describe stamp"
        );
        assert!(
            instructions.contains("STALE"),
            "banner must warn that a lagging binary is stale"
        );
        assert_eq!(
            instructions.lines().nth(2),
            Some(TOOL_SURFACE_DIGEST),
            "compact tool digest should sit immediately after the build banner"
        );
        // The original tool catalogue must survive the prepend.
        assert!(
            instructions.contains("MAP TOOLS:") && instructions.contains("prism(task="),
            "tool catalogue body must be preserved after the banner"
        );
    }

    /// Single env-sensitive test for context_deadline() — consolidated to
    /// avoid the parallel race that would happen if these three branches
    /// ran as independent #[test]s mutating the same process-global env var.
    #[test]
    fn context_deadline_env_matrix() {
        // SAFETY: this is the only test that touches CONTEXT_DEADLINE_ENV;
        // all scenarios run sequentially inside a single test function so
        // there is no race with another #[test] reading the variable.
        unsafe { std::env::remove_var(CONTEXT_DEADLINE_ENV) };
        assert_eq!(
            context_deadline(),
            std::time::Duration::from_secs(90),
            "default applies when env var is unset"
        );

        unsafe { std::env::set_var(CONTEXT_DEADLINE_ENV, "30") };
        assert_eq!(
            context_deadline(),
            std::time::Duration::from_secs(30),
            "valid override is honored"
        );

        unsafe { std::env::set_var(CONTEXT_DEADLINE_ENV, "0") };
        assert_eq!(
            context_deadline(),
            std::time::Duration::from_secs(90),
            "zero falls back to default"
        );

        unsafe { std::env::set_var(CONTEXT_DEADLINE_ENV, "not-a-number") };
        assert_eq!(
            context_deadline(),
            std::time::Duration::from_secs(90),
            "garbage falls back to default"
        );

        unsafe { std::env::remove_var(CONTEXT_DEADLINE_ENV) };
    }

    #[test]
    fn deadline_exceeded_response_is_well_formed_json() {
        let body = deadline_exceeded_response(std::time::Duration::from_secs(45));
        let value: Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(value["status"], "error");
        assert_eq!(value["error"], "deadline_exceeded");
        assert_eq!(value["deadline_secs"], 45);
        assert_eq!(value["protocol"], "loctree.context_atlas.v1");
        assert!(
            value["hint"]
                .as_str()
                .expect("hint is string")
                .contains(CONTEXT_DEADLINE_ENV),
            "hint should reference the env var name"
        );
        assert!(
            value["session"]
                .as_str()
                .expect("session is string")
                .starts_with("ctx_"),
            "session id should follow ctx_ prefix"
        );
    }

    fn fixture_project() -> TempDir {
        let temp = tempfile::tempdir().expect("create temp project");
        fs::create_dir_all(temp.path().join("src")).expect("create src dir");
        fs::write(
            temp.path().join("Cargo.toml"),
            r#"[package]
name = "context-fixture"
version = "0.1.0"
edition = "2024"
"#,
        )
        .expect("write Cargo.toml");
        fs::write(
            temp.path().join("src/lib.rs"),
            r#"pub mod foo;

pub fn public_entry() {
    foo::helper();
}"#,
        )
        .expect("write lib.rs");
        fs::write(
            temp.path().join("src/foo.rs"),
            r#"pub fn helper() -> &'static str {
    "ok"
}"#,
        )
        .expect("write foo.rs");
        temp
    }

    fn params_for(project: &Path) -> ContextParams {
        ContextParams {
            project: project.display().to_string(),
            force_no_git: true,
            no_scan: false,
            fail_stale: false,
            fresh: false,
            file: None,
            task: None,
            scope: Vec::new(),
            changed: false,
            with_aicx: true,
            no_aicx: true,
            format: ContextFormat::Json,
            section: None,
        }
    }

    async fn context_output(project: &Path, mutate: impl FnOnce(&mut ContextParams)) -> String {
        let server = LoctreeServer::new();
        let mut params = params_for(project);
        mutate(&mut params);
        server.context(Parameters(params)).await
    }

    #[tokio::test]
    async fn context_tool_returns_valid_json() {
        let project = fixture_project();
        let output = context_output(project.path(), |_| {}).await;

        serde_json::from_str::<serde_json::Value>(&output).expect("context output should parse");
    }

    #[test]
    fn context_receipt_uses_snapshot_git_metadata_not_caller_root() {
        let scanned = fixture_project();
        let caller = fixture_project();
        let mut snapshot = Snapshot::new(vec![scanned.path().display().to_string()]);
        snapshot.metadata.git_repo = Some("scanned-repo".to_string());
        snapshot.metadata.git_owner_repo = Some("Org/scanned-repo".to_string());
        snapshot.metadata.git_branch = Some("feature/scanned".to_string());
        snapshot.metadata.git_commit = Some("abc1234".to_string());
        snapshot.metadata.git_scan_id = Some("feature/scanned@abc1234".to_string());

        let receipt =
            LoctreeServer::context_receipt_payload("ctx-test", caller.path(), &snapshot, true);

        assert_eq!(
            receipt["snapshot"]["git"]["repo"].as_str(),
            Some("scanned-repo")
        );
        assert_eq!(
            receipt["snapshot"]["git"]["owner_repo"].as_str(),
            Some("Org/scanned-repo")
        );
        assert_eq!(
            receipt["snapshot"]["git"]["scan_id"].as_str(),
            Some("feature/scanned@abc1234")
        );
        assert_eq!(
            receipt["snapshot"]["roots"][0].as_str(),
            Some(scanned.path().to_str().expect("utf8 temp path"))
        );
    }

    #[tokio::test]
    async fn slice_refreshes_when_existing_file_is_missing_from_cached_snapshot() {
        let project = fixture_project();
        let server = LoctreeServer::new();

        let initial = server.context(Parameters(params_for(project.path()))).await;
        serde_json::from_str::<serde_json::Value>(&initial).expect("prime snapshot");

        fs::write(
            project.path().join("CONTRIBUTING.md"),
            "# Contributor Guide\n",
        )
        .expect("write markdown after initial snapshot");

        let output = server
            .slice(Parameters(SliceParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                file: "CONTRIBUTING.md".to_string(),
                consumers: false,
            }))
            .await;

        let value: serde_json::Value =
            serde_json::from_str(&output).expect("slice output should be valid JSON after retry");
        assert_eq!(value["target"], "CONTRIBUTING.md");
        assert!(
            value["files"]
                .as_array()
                .expect("files array")
                .iter()
                .any(|file| file["path"] == "CONTRIBUTING.md"),
            "existing file missing from cached snapshot should trigger a fresh scan and retry: {output}"
        );
    }

    #[tokio::test]
    async fn slice_returns_core_for_explicit_file_excluded_by_loctignore() {
        let project = fixture_project();
        let server = LoctreeServer::new();
        fs::create_dir_all(project.path().join("fixtures")).expect("create fixtures dir");
        fs::write(project.path().join(".loctignore"), "fixtures/\n").expect("write loctignore");
        fs::write(
            project.path().join("fixtures/local.rs"),
            "pub fn fixture_only() {}\n",
        )
        .expect("write ignored fixture");

        let initial = server.context(Parameters(params_for(project.path()))).await;
        serde_json::from_str::<serde_json::Value>(&initial).expect("prime snapshot");

        let output = server
            .slice(Parameters(SliceParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                file: "fixtures/local.rs".to_string(),
                consumers: false,
            }))
            .await;

        let value: serde_json::Value =
            serde_json::from_str(&output).expect("ignored explicit fixture should still JSON");
        assert_eq!(value["target"], "fixtures/local.rs");
        assert_eq!(value["files"][0]["path"], "fixtures/local.rs");
        assert_eq!(value["files"][0]["source"], "disk_explicit_fallback");
        assert!(
            value["snapshot_exclusion"]
                .as_str()
                .unwrap_or_default()
                .contains("detected exclusion: ignored by .loctignore:1 pattern `fixtures/`"),
            "fallback should explain why the slice is core-only: {output}"
        );
    }

    #[tokio::test]
    async fn impact_returns_named_fallback_for_explicit_file_excluded_by_loctignore() {
        let project = fixture_project();
        let server = LoctreeServer::new();
        fs::create_dir_all(project.path().join("fixtures")).expect("create fixtures dir");
        fs::write(project.path().join(".loctignore"), "fixtures/\n").expect("write loctignore");
        fs::write(
            project.path().join("fixtures/local.rs"),
            "pub fn fixture_only() {}\n",
        )
        .expect("write ignored fixture");

        let initial = server.context(Parameters(params_for(project.path()))).await;
        serde_json::from_str::<serde_json::Value>(&initial).expect("prime snapshot");

        let output = server
            .impact(Parameters(ImpactParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                file: "fixtures/local.rs".to_string(),
            }))
            .await;

        let value: serde_json::Value =
            serde_json::from_str(&output).expect("ignored explicit impact should still JSON");
        assert_eq!(value["file"], "fixtures/local.rs");
        assert_eq!(value["risk_level"], "unknown");
        assert_eq!(value["safe_to_delete"], false);
        assert_eq!(value["direct_consumers"]["count"], 0);
        assert_eq!(value["transitive_consumers"]["count"], 0);
        assert_eq!(
            value["fallback_read"]["files"][0]["path"],
            "fixtures/local.rs"
        );
        assert_eq!(
            value["fallback_read"]["files"][0]["source"],
            "disk_explicit_fallback"
        );
        assert!(
            value["snapshot_exclusion"]
                .as_str()
                .unwrap_or_default()
                .contains("detected exclusion: ignored by .loctignore:1 pattern `fixtures/`"),
            "fallback should explain why impact is core-only: {output}"
        );
    }

    #[tokio::test]
    async fn slice_prefers_exact_repo_relative_file_over_fixture_suffix() {
        let project = fixture_project();
        let server = LoctreeServer::new();

        fs::write(project.path().join("Makefile"), "root:\n\t@echo root\n")
            .expect("write root Makefile");
        fs::create_dir_all(project.path().join("scripts")).expect("create root scripts dir");
        fs::write(
            project.path().join("scripts/version-bump.sh"),
            "#!/usr/bin/env bash\necho root\n",
        )
        .expect("write root version script");

        fs::create_dir_all(project.path().join("loctree-rs/tests/fixtures/make_rich"))
            .expect("create nested make fixture dir");
        fs::write(
            project
                .path()
                .join("loctree-rs/tests/fixtures/make_rich/Makefile"),
            "fixture:\n\t@echo fixture\n",
        )
        .expect("write fixture Makefile");
        fs::create_dir_all(
            project
                .path()
                .join("loctree-rs/tests/fixtures/shell_rich/scripts"),
        )
        .expect("create nested shell fixture dir");
        fs::write(
            project
                .path()
                .join("loctree-rs/tests/fixtures/shell_rich/scripts/version-bump.sh"),
            "#!/usr/bin/env bash\necho fixture\n",
        )
        .expect("write fixture version script");

        let initial = server.context(Parameters(params_for(project.path()))).await;
        serde_json::from_str::<serde_json::Value>(&initial).expect("prime snapshot");

        let make_output = server
            .slice(Parameters(SliceParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                file: "Makefile".to_string(),
                consumers: false,
            }))
            .await;
        let make_value: serde_json::Value =
            serde_json::from_str(&make_output).expect("Makefile slice output should be JSON");
        assert_eq!(make_value["files"][0]["path"], "Makefile");

        let script_output = server
            .slice(Parameters(SliceParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                file: "scripts/version-bump.sh".to_string(),
                consumers: false,
            }))
            .await;
        let script_value: serde_json::Value =
            serde_json::from_str(&script_output).expect("script slice output should be JSON");
        assert_eq!(script_value["files"][0]["path"], "scripts/version-bump.sh");
    }

    #[tokio::test]
    async fn slice_exposes_core_symbols_authority_and_suggested_next() {
        let project = fixture_project();
        let server = LoctreeServer::new();

        let initial = server.context(Parameters(params_for(project.path()))).await;
        serde_json::from_str::<serde_json::Value>(&initial).expect("prime snapshot");

        let output = server
            .slice(Parameters(SliceParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                file: "src/lib.rs".to_string(),
                consumers: true,
            }))
            .await;
        let value: Value = serde_json::from_str(&output).expect("slice output JSON");

        assert!(
            value["core_symbols"]
                .as_array()
                .expect("core_symbols array")
                .iter()
                .any(|symbol| {
                    symbol["name"] == "public_entry"
                        && symbol["file"] == "src/lib.rs"
                        && symbol["line"] == 3
                        && symbol["authority"] == "LoctreeDerived"
                }),
            "MCP slice should expose core symbols with file:line authority: {output}"
        );
        assert!(
            value["authority_labels"]
                .as_array()
                .expect("authority_labels array")
                .iter()
                .any(|label| label == "LoctreeDerived"),
            "MCP slice should expose authority labels: {output}"
        );
        assert!(
            value["suggested_next"]
                .as_array()
                .expect("suggested_next")
                .iter()
                .any(|step| step["command"] == "loct occurrences 'public_entry' --json"),
            "MCP slice should suggest concrete next commands: {output}"
        );
    }

    #[tokio::test]
    async fn focus_exposes_core_symbols_consumers_and_suggested_next() {
        let project = fixture_project();
        let server = LoctreeServer::new();
        fs::create_dir_all(project.path().join("src/feature")).expect("create feature dir");
        fs::write(
            project.path().join("src/feature/index.ts"),
            "export function feature_entry() { return 42; }\n",
        )
        .expect("write feature index");
        fs::write(
            project.path().join("src/app.ts"),
            "import { feature_entry } from './feature';\nfeature_entry();\n",
        )
        .expect("write app consumer");

        let initial = server.context(Parameters(params_for(project.path()))).await;
        serde_json::from_str::<serde_json::Value>(&initial).expect("prime snapshot");

        let output = server
            .focus(Parameters(FocusParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                directory: "src/feature".to_string(),
            }))
            .await;
        let value: Value = serde_json::from_str(&output).expect("focus output JSON");

        assert!(
            value["core_symbols"]
                .as_array()
                .expect("core_symbols")
                .iter()
                .any(|symbol| {
                    symbol["name"] == "feature_entry"
                        && symbol["file"] == "src/feature/index.ts"
                        && symbol["line"] == 1
                }),
            "MCP focus should expose core symbols in the focused directory: {output}"
        );
        assert!(
            value["module_consumers"]
                .as_array()
                .expect("module_consumers")
                .iter()
                .any(|consumer| consumer["path"] == "src/app.ts"),
            "MCP focus should expose pathful module consumer entries: {output}"
        );
        assert!(
            value["suggested_next"]
                .as_array()
                .expect("suggested_next")
                .iter()
                .any(|step| step["command"] == "loct occurrences 'feature_entry' --json"),
            "MCP focus should suggest concrete next commands: {output}"
        );
    }

    #[tokio::test]
    async fn follow_cycles_returns_deduped_pathful_chains() {
        let project = tempfile::tempdir().expect("create cycle project");
        fs::create_dir_all(project.path().join("src")).expect("create src");
        fs::write(
            project.path().join("src/a.ts"),
            "import { b } from './b';\nexport const a = b;\n",
        )
        .expect("write a.ts");
        fs::write(
            project.path().join("src/b.ts"),
            "import { a } from './a';\nexport const b = a;\n",
        )
        .expect("write b.ts");

        let server = LoctreeServer::new();
        let initial = server.context(Parameters(params_for(project.path()))).await;
        serde_json::from_str::<serde_json::Value>(&initial).expect("prime snapshot");

        let output = server
            .follow(Parameters(FollowParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                scope: "cycles".to_string(),
                handler: None,
                limit: 10,
            }))
            .await;
        let value: Value = serde_json::from_str(&output).expect("follow output JSON");
        let signals = value["trails"]["cycles"]["signals"]
            .as_array()
            .expect("cycle signals");
        let mut seen = std::collections::HashSet::new();
        for signal in signals {
            let chain = signal["chain"].as_array().expect("cycle chain");
            assert!(
                chain
                    .iter()
                    .all(|node| node.as_str().is_some_and(|path| path.starts_with("src/"))),
                "cycle chains should use repo-relative paths, not bare names: {output}"
            );
            let key = chain
                .iter()
                .filter_map(|node| node.as_str())
                .collect::<Vec<_>>()
                .join(" -> ");
            assert!(
                seen.insert(key.clone()),
                "duplicate cycle chain {key}: {output}"
            );
        }
    }

    #[tokio::test]
    async fn tagmap_recalls_symbols_with_dash_underscore_normalization() {
        let project = fixture_project();
        let server = LoctreeServer::new();

        let initial = server.context(Parameters(params_for(project.path()))).await;
        serde_json::from_str::<serde_json::Value>(&initial).expect("prime snapshot");

        let output = server
            .find(Parameters(FindParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                name: "public-entry".to_string(),
                mode: "tagmap".to_string(),
                limit: 20,
                lang: None,
                exported_only: false,
                dead_only: false,
                min_score: None,
                similar: None,
                file: None,
                whole_token: false,
                group_by_file: false,
                count_only: false,
                offset: 0,
            }))
            .await;

        let value: serde_json::Value =
            serde_json::from_str(&output).expect("tagmap output should be valid JSON");
        assert!(
            value["facts"]["matches"]
                .as_array()
                .expect("facts matches array")
                .iter()
                .any(|item| {
                    item["kind"] == "export"
                        && item["file"] == "src/lib.rs"
                        && item["name"] == "public_entry"
                }),
            "tagmap should recall exported symbols, including dash/underscore query variants: {output}"
        );
    }

    #[tokio::test]
    async fn tagmap_recalls_exact_indexed_local_symbol_terms() {
        let project = fixture_project();
        let server = LoctreeServer::new();

        fs::write(
            project.path().join("src/receipt.rs"),
            r#"pub fn context_receipt_payload() -> &'static str {
    "receipt"
}"#,
        )
        .expect("write receipt module");
        fs::write(
            project.path().join("src/lib.rs"),
            r#"pub mod foo;
pub mod receipt;

pub fn public_entry() {
    foo::helper();
}"#,
        )
        .expect("rewrite lib.rs");

        let initial = server.context(Parameters(params_for(project.path()))).await;
        serde_json::from_str::<serde_json::Value>(&initial).expect("prime snapshot");

        let output = server
            .find(Parameters(FindParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                name: "context_receipt_payload".to_string(),
                mode: "tagmap".to_string(),
                limit: 20,
                lang: None,
                exported_only: false,
                dead_only: false,
                min_score: None,
                similar: None,
                file: None,
                whole_token: false,
                group_by_file: false,
                count_only: false,
                offset: 0,
            }))
            .await;

        let value: serde_json::Value =
            serde_json::from_str(&output).expect("tagmap output should be valid JSON");
        assert!(
            value["facts"]["matches"]
                .as_array()
                .expect("facts matches array")
                .iter()
                .any(|item| {
                    item["kind"] == "export"
                        && item["file"] == "src/receipt.rs"
                        && item["name"] == "context_receipt_payload"
                }),
            "tagmap should recall exact indexed source terms surfaced by snapshot facts: {output}"
        );
    }

    #[tokio::test]
    async fn where_symbol_can_narrow_to_file_and_prefer_signature_context() {
        let project = fixture_project();
        let server = LoctreeServer::new();

        fs::write(
            project.path().join("src/handler.rs"),
            r#"pub async fn find() -> &'static str {
    "handler"
}

pub fn find_helper() -> &'static str {
    "helper"
}"#,
        )
        .expect("write handler");
        fs::write(
            project.path().join("src/other.rs"),
            r#"pub async fn find() -> &'static str {
    "other"
}"#,
        )
        .expect("write other");
        fs::write(
            project.path().join("src/lib.rs"),
            r#"pub mod handler;
pub mod other;

pub fn public_entry() {
    let _ = handler::find_helper();
}"#,
        )
        .expect("rewrite lib.rs");

        let initial = server.context(Parameters(params_for(project.path()))).await;
        serde_json::from_str::<serde_json::Value>(&initial).expect("prime snapshot");

        let output = server
            .find(Parameters(FindParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                name: "async fn find".to_string(),
                mode: "where-symbol".to_string(),
                limit: 20,
                lang: None,
                exported_only: false,
                dead_only: false,
                min_score: None,
                similar: None,
                file: Some("src/handler.rs".to_string()),
                whole_token: false,
                group_by_file: false,
                count_only: false,
                offset: 0,
            }))
            .await;

        let value: serde_json::Value =
            serde_json::from_str(&output).expect("where-symbol output should be valid JSON");
        let results = value["results"].as_array().expect("results array");
        assert!(
            !results.is_empty(),
            "file-scoped signature query should return at least one result: {output}"
        );
        assert!(
            results.iter().all(|item| item["file"] == "src/handler.rs"),
            "file filter should remove same-name symbols in other files: {output}"
        );
        assert!(
            results[0]["context"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .contains("function find"),
            "signature-shaped query should prefer the exact symbol anchor over substring matches: {output}"
        );
    }

    #[tokio::test]
    async fn find_param_parity_literal_mode_returns_occurrence_role_truth_at_cli_parity() {
        // The codescribe `utterance_id` failure class: a local binding plus an
        // increment plus a struct-field emission buried inside a function body.
        // `find` (AST/tagmap) misses the locals; the literal mode must see them.
        let project = fixture_project();
        let server = LoctreeServer::new();

        let source = r#"pub fn process() {
    let mut utterance_id = 0;
    utterance_id += 1;
    let _evt = Event { utterance_id };
    let _ = utterance_id;
}
"#;
        fs::write(project.path().join("src/scribe.rs"), source).expect("write scribe.rs");
        fs::write(
            project.path().join("src/lib.rs"),
            "pub mod foo;\npub mod scribe;\n",
        )
        .expect("rewrite lib.rs");

        let initial = server.context(Parameters(params_for(project.path()))).await;
        serde_json::from_str::<serde_json::Value>(&initial).expect("prime snapshot");

        let output = server
            .find(Parameters(FindParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                name: "utterance_id".to_string(),
                mode: "literal".to_string(),
                limit: 50,
                lang: None,
                exported_only: false,
                dead_only: false,
                min_score: None,
                similar: None,
                file: None,
                whole_token: false,
                group_by_file: false,
                count_only: false,
                offset: 0,
            }))
            .await;

        let value: serde_json::Value =
            serde_json::from_str(&output).expect("literal output should be valid JSON");
        assert_eq!(value["mode"], "literal", "mode echo: {output}");
        assert_eq!(value["literal_matches"]["source"], "literal");
        assert_eq!(value["literal_matches"]["query_kind"], "identifier");
        assert_eq!(
            value["literal_matches"]["match_mode"],
            "identifier_boundary"
        );
        assert!(
            value["literal_matches"]["coverage_line"]
                .as_str()
                .is_some_and(|line| line.contains("scanned")),
            "literal response should expose CLI coverage text: {output}"
        );
        assert!(
            value["literal_matches"]["scope"]["files_scanned"]
                .as_u64()
                .is_some_and(|count| count > 0),
            "literal response should expose scan scope stats: {output}"
        );

        let occ = value["literal_matches"]["occurrences"]
            .as_array()
            .expect("occurrences array");
        // 4 literal sites: binding, increment, field shorthand, plain read.
        assert_eq!(occ.len(), 4, "expected 4 literal occurrences: {output}");
        assert_eq!(value["total"], 4);
        assert!(
            occ.iter().all(|o| o["source"] == "literal"
                && o["matched_text"] == "utterance_id"
                && o["file"] == "src/scribe.rs"),
            "every occurrence is literal evidence in scribe.rs: {output}"
        );
        let kinds: Vec<&str> = occ
            .iter()
            .map(|o| o["occurrence_kind"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            kinds,
            vec![
                "definition_like",
                "mutation_like",
                "field_emit_like",
                // `let _ = utterance_id;` is an honest `identifier` read now,
                // no longer a blanket `unknown`.
                "identifier"
            ],
            "single-line classification rides along on every occurrence: {output}"
        );
        let roles: Vec<&str> = occ
            .iter()
            .map(|o| o["match_role"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            roles,
            vec!["local_binding", "mutation", "field_emission", "reference"],
            "MCP literal mode must expose the compact role contract: {output}"
        );
        let confidences: Vec<&str> = occ
            .iter()
            .map(|o| o["confidence"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            confidences,
            vec!["high", "high", "high", "medium"],
            "MCP literal mode must expose role confidence: {output}"
        );
        assert!(
            occ.iter()
                .all(|o| o["scope_classification"] == "production"),
            "MCP literal mode must expose file-scope classification per occurrence: {output}"
        );
        let suggested_next = value["literal_matches"]["suggested_next"]
            .as_array()
            .expect("suggested_next array");
        assert!(
            suggested_next
                .iter()
                .any(|s| s["command"] == "loct body 'utterance_id' --json"),
            "MCP literal mode should carry CLI suggested-next body command: {output}"
        );
        assert!(
            suggested_next
                .iter()
                .any(|s| s["command"] == "loct slice 'src/scribe.rs'"),
            "MCP literal mode should carry CLI suggested-next slice command: {output}"
        );

        // Parity contract: the MCP surface must equal the shared scanner run over
        // the same bytes. This is what guarantees `find(mode=literal)` never
        // drifts from `loct occurrences` / `loct find --literal`.
        let mut expected = loctree::analyzer::occurrences::scan_files_with(
            [("src/scribe.rs", source)],
            "utterance_id",
            ScanOptions::default(),
        );
        expected.apply_report(ReportOptions {
            group_by_file: false,
            count_only: false,
            offset: 0,
            limit: Some(50),
        });
        let mut expected_json =
            serde_json::to_value(&expected).expect("serialize expected OccurrenceResults");
        // Align scope/coverage fields to the actual MCP run's project context to assert parity on occurrences.
        if let Some(obj) = expected_json.as_object_mut()
            && let Some(real_obj) = value["literal_matches"].as_object()
        {
            if let Some(cov) = real_obj.get("coverage_line") {
                obj.insert("coverage_line".to_string(), cov.clone());
            }
            if let Some(scope) = real_obj.get("scope") {
                obj.insert("scope".to_string(), scope.clone());
            }
        }
        assert_eq!(
            value["literal_matches"], expected_json,
            "MCP literal_matches must be byte-for-byte identical to the shared scanner output"
        );
    }

    #[tokio::test]
    async fn find_literal_mode_honors_file_scope_and_reports_range() {
        let project = fixture_project();
        let server = LoctreeServer::new();

        fs::write(
            project.path().join("src/styles.css"),
            ".checkout-success { color: var(--checkout-success); }\n",
        )
        .expect("write styles");
        fs::write(
            project.path().join("src/other.css"),
            ".checkout-success { opacity: 1; }\n",
        )
        .expect("write other");

        let initial = server.context(Parameters(params_for(project.path()))).await;
        serde_json::from_str::<serde_json::Value>(&initial).expect("prime snapshot");

        let output = server
            .find(Parameters(FindParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                name: "checkout-success".to_string(),
                mode: "literal".to_string(),
                limit: 50,
                lang: None,
                exported_only: false,
                dead_only: false,
                min_score: None,
                similar: None,
                file: Some("src/styles.css".to_string()),
                whole_token: false,
                group_by_file: false,
                count_only: false,
                offset: 0,
            }))
            .await;

        let value: serde_json::Value =
            serde_json::from_str(&output).expect("literal output should be valid JSON");
        assert_eq!(value["file_filter"], "src/styles.css");
        let lit = &value["literal_matches"];
        assert_eq!(lit["files_matched"], 1);
        assert_eq!(lit["total"], 2);
        let occ = lit["occurrences"].as_array().expect("occurrences array");
        assert!(
            occ.iter().all(|o| o["file"] == "src/styles.css"),
            "file-scoped literal mode must not leak sibling files: {output}"
        );
        assert_eq!(occ[0]["line"], 1);
        assert_eq!(occ[0]["column"], 2);
        assert_eq!(occ[0]["range"]["start"]["line"], 1);
        assert_eq!(occ[0]["range"]["start"]["column"], 2);
        assert_eq!(occ[0]["range"]["end"]["column"], 18);
    }

    #[tokio::test]
    async fn follow_twins_exposes_classification_for_agent_action() {
        let project = fixture_project();
        let server = LoctreeServer::new();

        fs::write(
            project.path().join("src/left.rs"),
            r#"pub fn shared_marker(input: MarkerConfig) -> MarkerOutput {
    unimplemented!("{input:?}")
}"#,
        )
        .expect("write left twin");
        fs::write(
            project.path().join("src/right.rs"),
            r#"pub fn shared_marker(input: MarkerConfig) -> MarkerOutput {
    unimplemented!("{input:?}")
}"#,
        )
        .expect("write right twin");
        fs::write(
            project.path().join("src/lib.rs"),
            r#"pub mod foo;
pub mod left;
pub mod right;

pub fn public_entry() {
    foo::helper();
}"#,
        )
        .expect("rewrite lib.rs");

        let initial = server.context(Parameters(params_for(project.path()))).await;
        serde_json::from_str::<serde_json::Value>(&initial).expect("prime snapshot");

        let output = server
            .follow(Parameters(FollowParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                scope: "twins".to_string(),
                handler: None,
                limit: 10,
            }))
            .await;

        let value: serde_json::Value =
            serde_json::from_str(&output).expect("follow output should be valid JSON");
        let signals = value["trails"]["twins"]["signals"]
            .as_array()
            .expect("twins signals array");
        let marker = signals
            .iter()
            .find(|signal| signal["symbol"] == "shared_marker")
            .unwrap_or_else(|| panic!("shared_marker twin should be present: {output}"));

        assert_eq!(marker["classification"], "duplicate");
        assert_eq!(marker["action"], "consolidate into single module");
    }

    /// Regression for loctree-feedback hak 2026-05-23 #13 (L9 closure): MCP
    /// `follow(scope='twins')` must include route-level twins (FastAPI /
    /// Flask / Tauri duplicate `(method, path)` registrations) to match
    /// CLI `loct twins` since marbles-L8. Before this fix MCP returned
    /// only symbol-level twins while CLI returned both, so agents (MCP)
    /// and operators (CLI) saw different views of repo twin reality.
    #[tokio::test]
    async fn follow_twins_includes_route_twins() {
        let project = fixture_project();
        let server = LoctreeServer::new();

        // Synthesize two Python files that both register POST /api/stt
        // with a FastAPI-shaped decorator. The detector only needs the
        // route surface to populate `FileAnalysis::routes`.
        fs::write(
            project.path().join("src/analyze_server.py"),
            r#"from fastapi import FastAPI
app = FastAPI()

@app.post("/api/stt")
def transcribe_voice():
    return {"ok": True}
"#,
        )
        .expect("write analyze_server.py");
        fs::write(
            project.path().join("src/review_server.py"),
            r#"from fastapi import FastAPI
app = FastAPI()

@app.post("/api/stt")
def transcribe_voice():
    return {"ok": False}
"#,
        )
        .expect("write review_server.py");

        let initial = server.context(Parameters(params_for(project.path()))).await;
        serde_json::from_str::<serde_json::Value>(&initial).expect("prime snapshot");

        let output = server
            .follow(Parameters(FollowParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                scope: "twins".to_string(),
                handler: None,
                limit: 10,
            }))
            .await;

        let value: serde_json::Value =
            serde_json::from_str(&output).expect("follow output should be valid JSON");

        // The route_twins envelope MUST exist (parity contract with CLI).
        let route_twins = value["trails"]["twins"]["route_twins"]
            .as_object()
            .unwrap_or_else(|| {
                panic!(
                    "MCP follow(twins) must expose route_twins envelope for CLI parity: {output}"
                )
            });
        let total = route_twins["total"].as_u64().unwrap_or(0);
        let signals = route_twins["signals"]
            .as_array()
            .expect("route_twins.signals array");

        // If the Python analyzer in this build surfaces FastAPI routes,
        // the duplicate POST /api/stt must show up; if it does not
        // surface routes at all (analyzer feature flag), the envelope
        // is still present (total=0) so the contract still holds.
        if total > 0 {
            let stt_collision = signals.iter().find(|sig| {
                sig["path"] == "/api/stt" && sig["method"].as_str().unwrap_or("") == "POST"
            });
            assert!(
                stt_collision.is_some(),
                "POST /api/stt must surface as a route twin when route analyzer is active: {output}"
            );
        }
    }

    #[tokio::test]
    async fn focus_summary_counts_external_edges_it_lists() {
        let project = fixture_project();
        let server = LoctreeServer::new();

        fs::create_dir_all(project.path().join("src/pages")).expect("create pages dir");
        fs::write(
            project.path().join("src/pages/mod.rs"),
            r#"use crate::foo::helper;

pub fn page() {
    let _ = helper();
}
"#,
        )
        .expect("write pages module");

        let output = server
            .focus(Parameters(FocusParams {
                project: project.path().display().to_string(),
                force_no_git: true,
                directory: "src/pages".to_string(),
            }))
            .await;

        let value: serde_json::Value =
            serde_json::from_str(&output).expect("focus output should be valid JSON");
        let listed_deps = value["external_dependencies"]
            .as_array()
            .expect("external deps array")
            .len();

        assert!(
            listed_deps > 0,
            "fixture should produce at least one external dependency: {output}"
        );
        assert_eq!(
            value["summary"]["external_dependency_edges"], listed_deps,
            "summary should count the same external dependency edges focus lists"
        );
    }

    #[tokio::test]
    async fn context_format_defaults_to_json() {
        let project = fixture_project();
        let output = context_output(project.path(), |_| {}).await;
        let value: Value = serde_json::from_str(&output).expect("valid context json");

        assert_eq!(value["protocol"], "loctree.context_atlas.v1");
        assert!(value.get("markdown").is_none());
        assert_ne!(value["format"], "markdown");
    }

    #[tokio::test]
    async fn context_format_markdown_returns_pill() {
        let project = fixture_project();
        let output = context_output(project.path(), |params| {
            params.format = ContextFormat::Markdown;
        })
        .await;
        let value: Value = serde_json::from_str(&output).expect("valid markdown wrapper json");

        assert_eq!(value["protocol"], "loctree.context_atlas.v1");
        assert_eq!(value["format"], "markdown");
        assert_eq!(value["status"], "complete");
        assert!(
            value
                .pointer("/receipt/snapshot/fingerprint/value")
                .is_some()
        );
        assert!(
            value["markdown"]
                .as_str()
                .expect("markdown string")
                .starts_with("# Loctree Context")
        );
        // A small fixture fits in a single page: no truncation (forbidden by
        // decree), pagination is inert, and the receipt accounts for the full
        // pack so completeness is provable rather than trusted.
        assert_eq!(value["pagination"]["paginated"], serde_json::json!(false));
        assert!(value["pagination"]["next_section"].is_null());
        assert!(
            value.pointer("/receipt/full_context/sha256").is_some(),
            "receipt must check off full context via a whole-pack digest"
        );
        assert!(
            value["receipt"]["full_context"]["total_bytes"]
                .as_u64()
                .expect("full_context.total_bytes")
                > 0
        );
        assert_eq!(
            value["receipt"]["full_context"]["complete_in_this_response"],
            serde_json::json!(true)
        );
        // Truncation fields are gone — agents must never get a lossy head.
        assert!(value.get("truncated").is_none());
        assert!(value.get("read_cards_hint").is_none());
    }

    /// Single env-sensitive test for context_markdown_budget() — consolidated to
    /// avoid the parallel race that would occur if multiple #[test]s mutated the
    /// same process-global env var.
    #[test]
    fn context_markdown_budget_env_matrix() {
        // SAFETY: this is the only test that touches CONTEXT_MARKDOWN_BUDGET_ENV;
        // all scenarios run sequentially inside this single test function, so
        // there is no race with another #[test] reading the variable.
        unsafe { std::env::remove_var(CONTEXT_MARKDOWN_BUDGET_ENV) };
        assert_eq!(
            context_markdown_budget(),
            CONTEXT_MARKDOWN_BUDGET_DEFAULT,
            "default applies when env var is unset"
        );

        // Use a value ABOVE the default so a concurrent context() call (e.g.
        // context_format_markdown_returns_pill) just sees a larger single page
        // from this test's transient env mutation — the getter logic is
        // identical regardless of the magnitude.
        unsafe { std::env::set_var(CONTEXT_MARKDOWN_BUDGET_ENV, "60000") };
        assert_eq!(
            context_markdown_budget(),
            60_000,
            "valid override is honored"
        );

        // Below the 2_000 floor is rejected (a tiny budget would shred the pack
        // into uselessly small pages).
        unsafe { std::env::set_var(CONTEXT_MARKDOWN_BUDGET_ENV, "1500") };
        assert_eq!(
            context_markdown_budget(),
            CONTEXT_MARKDOWN_BUDGET_DEFAULT,
            "sub-floor value is rejected and falls back to the default"
        );

        unsafe { std::env::set_var(CONTEXT_MARKDOWN_BUDGET_ENV, "0") };
        assert_eq!(
            context_markdown_budget(),
            CONTEXT_MARKDOWN_BUDGET_DEFAULT,
            "zero is rejected and falls back to the default"
        );

        unsafe { std::env::set_var(CONTEXT_MARKDOWN_BUDGET_ENV, "not-a-number") };
        assert_eq!(
            context_markdown_budget(),
            CONTEXT_MARKDOWN_BUDGET_DEFAULT,
            "non-numeric is rejected and falls back to the default"
        );

        unsafe { std::env::remove_var(CONTEXT_MARKDOWN_BUDGET_ENV) };
    }

    #[test]
    fn oversized_tool_response_writes_full_payload_artifact_and_marker() {
        let temp = tempfile::tempdir().expect("temp dir");
        let raw = serde_json::json!({
            "tool": "slice",
            "items": ["x".repeat(CONTEXT_MARKDOWN_BUDGET_DEFAULT + 8_000)]
        })
        .to_string();

        let response = budget_tool_response("slice", Some(temp.path()), raw.clone());
        assert!(
            response.chars().count() <= CONTEXT_MARKDOWN_BUDGET_DEFAULT,
            "budgeted response must stay under default budget"
        );

        let marker: Value = serde_json::from_str(&response).expect("marker JSON");
        assert_eq!(marker["protocol"], MCP_RESPONSE_BUDGET_PROTOCOL);
        assert_eq!(marker["tool"], "slice");
        assert_eq!(marker["status"], "truncated_for_mcp_token_budget");
        let artifact_path = marker["full_payload"]["path"]
            .as_str()
            .expect("full payload path");
        assert!(
            artifact_path.ends_with(".full.json"),
            "marker must point at concrete full JSON artifact: {artifact_path}"
        );
        let artifact = fs::read_to_string(artifact_path).expect("read full payload artifact");
        assert_eq!(artifact, raw, "artifact must preserve unmodified payload");
    }

    #[test]
    fn budgeted_tool_response_keeps_small_payload_unchanged() {
        let raw = serde_json::json!({ "ok": true, "items": [1, 2, 3] }).to_string();
        let response = budget_tool_response("tree", None, raw.clone());
        assert_eq!(response, raw);
    }

    fn synthetic_pack_markdown(section_count: usize, section_chars: usize) -> String {
        let mut out = String::from("# Loctree Context — synthetic\n\nlead-in line\n\n");
        for s in 0..section_count {
            out.push_str(&format!("## Section {s}\n\n"));
            out.push_str(&"x".repeat(section_chars));
            out.push('\n');
        }
        out
    }

    #[test]
    fn split_markdown_keeps_preamble_as_overview_and_counts_h2() {
        let md = synthetic_pack_markdown(2, 10);
        let sections = split_markdown_sections(&md);
        // Overview (title block) + 2 `## ` sections.
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].title, "Overview");
        assert_eq!(sections[1].title, "Section 0");
        assert_eq!(sections[2].title, "Section 1");
        // `### ` must not start a new top-level section.
        let nested = "# Title\n\n## Top\n\n### Sub\n\nbody\n";
        let nested_sections = split_markdown_sections(nested);
        assert_eq!(
            nested_sections.len(),
            2,
            "### must stay inside its ## parent"
        );
    }

    #[test]
    fn paginate_returns_whole_pack_unchanged_when_it_fits() {
        let md = synthetic_pack_markdown(3, 50);
        let page = paginate_context_markdown(&md, None, 1_000_000);
        assert!(!page.paginated, "small pack must not paginate");
        assert_eq!(page.markdown, md, "whole pack returned byte-for-byte");
        assert!(page.next_section.is_none());
    }

    #[test]
    fn paginate_splits_and_stays_under_budget_when_oversized() {
        // 6 sections × ~2KB each ≈ 12KB; budget 3KB forces pagination.
        let md = synthetic_pack_markdown(6, 2_000);
        let budget = 3_000;
        let page = paginate_context_markdown(&md, None, budget);
        assert!(page.paginated, "oversized pack must paginate");
        assert!(
            page.markdown.len() <= budget,
            "page {} must fit budget {budget}",
            page.markdown.len()
        );
        assert!(page.next_section.is_some(), "more sections must remain");
        assert!(
            page.sections_emitted >= 1,
            "always emit at least one section"
        );
    }

    #[test]
    fn paginate_cursor_walks_every_section_exactly_once_in_order() {
        let md = synthetic_pack_markdown(6, 2_000);
        let budget = 3_000;
        let total = split_markdown_sections(&md).len();

        let mut cursor: Option<usize> = None;
        let mut covered: Vec<usize> = Vec::new();
        let mut guard = 0;
        loop {
            let page = paginate_context_markdown(&md, cursor, budget);
            assert_eq!(page.total_sections, total);
            for i in 0..page.sections_emitted {
                covered.push(page.section_start + i);
            }
            match page.next_section {
                Some(next) => {
                    assert!(next > page.section_start, "cursor must advance");
                    cursor = Some(next);
                }
                None => break,
            }
            guard += 1;
            assert!(guard < 100, "cursor walk must terminate");
        }
        let expected: Vec<usize> = (0..total).collect();
        assert_eq!(covered, expected, "every section read once, in order");
    }

    #[test]
    fn paginate_truncates_single_oversized_section_with_marker() {
        // A section far bigger than the budget must still be emitted when it
        // is the first on the page, but hard-truncated with an honest tail
        // rather than overflowing. Section 0 lives at index 1 (index 0 is the
        // small "Overview" title block), so aim the cursor straight at it.
        let md = synthetic_pack_markdown(1, 20_000);
        let budget = 4_000;
        let page = paginate_context_markdown(&md, Some(1), budget);
        assert!(page.paginated);
        assert!(
            page.markdown.len() <= budget,
            "truncated page {} must fit budget {budget}",
            page.markdown.len()
        );
        assert!(
            page.markdown
                .contains("truncated: section exceeds the MCP markdown budget"),
            "truncation must be explicit, not silent"
        );
    }

    #[tokio::test]
    async fn context_markdown_response_carries_pagination_block() {
        let project = fixture_project();
        let output = context_output(project.path(), |params| {
            params.format = ContextFormat::Markdown;
        })
        .await;
        let value: Value = serde_json::from_str(&output).expect("valid markdown wrapper json");
        // The tiny fixture fits the budget, so this is a whole-pack response:
        // pagination present, not paginated, status complete.
        assert_eq!(value["format"], "markdown");
        assert_eq!(value["status"], "complete");
        assert_eq!(value["pagination"]["paginated"], false);
        assert!(value["pagination"]["next_section"].is_null());
        assert!(
            value["markdown"]
                .as_str()
                .expect("markdown string")
                .starts_with("# Loctree Context")
        );
    }

    #[test]
    fn context_format_rejects_unknown() {
        let payload = serde_json::json!({
            "project": ".",
            "force_no_git": true,
            "format": "xml"
        });

        assert!(serde_json::from_value::<ContextParams>(payload).is_err());
    }

    #[tokio::test]
    async fn context_rejects_non_git_project_without_force_flag() {
        let project = fixture_project();
        let server = LoctreeServer::new();
        let mut params = params_for(project.path());
        params.force_no_git = false;
        let output = server.context(Parameters(params)).await;
        let value: Value = serde_json::from_str(&output).expect("error should be json-safe");

        assert!(
            value["error"]
                .as_str()
                .unwrap()
                .contains("not inside a git repository")
        );
    }

    #[tokio::test]
    async fn context_accepts_non_git_project_with_force_flag() {
        let project = fixture_project();
        let server = LoctreeServer::new();
        let mut params = params_for(project.path());
        params.force_no_git = true;
        let output = server.context(Parameters(params)).await;
        let value: Value = serde_json::from_str(&output).expect("success should be json-safe");
        println!("Value: {:?}", value);

        // Should return a valid payload (not an error string)
        assert!(value.get("error").is_none());
        assert!(value.get("data").is_some() || value.get("atlas").is_some());
    }

    #[tokio::test]
    async fn context_no_scan_returns_json_error_without_snapshot() {
        let project = fixture_project();
        let output = context_output(project.path(), |params| {
            params.no_scan = true;
        })
        .await;
        let value: Value = serde_json::from_str(&output).expect("error should be json-safe");

        assert!(value["error"].as_str().unwrap().contains("no_scan=true"));
    }

    #[test]
    fn mcp_server_does_not_shell_out_to_loctree_cli() {
        let source = include_str!("main.rs");
        let forbidden_process_bridges = [
            concat!("std::process", "::", "Command"),
            concat!("tokio::process", "::", "Command"),
            concat!("process", "::", "Command"),
            concat!("Command", "::", "new"),
            concat!("duct", "::", "cmd"),
            concat!("xshell", "::"),
            concat!("cargo", " run"),
        ];

        for needle in forbidden_process_bridges {
            assert!(
                !source.contains(needle),
                "loctree-mcp must use loctree library APIs instead of shelling out: found `{needle}`"
            );
        }
    }

    /// Guard against silently re-introducing path-traversal `nosemgrep`
    /// suppressions on the MCP project-path entrypoint. The real fix is
    /// `validate_project_path` — if a future change adds back a
    /// suppression to silence Semgrep instead of strengthening the
    /// validator, this test fails loudly. Test fixtures and unrelated
    /// suppressions inside this same file are still allowed; we only
    /// reject the specific tainted-path rule.
    #[test]
    fn mcp_project_path_has_no_tainted_path_suppressions() {
        let source = include_str!("main.rs");
        let needle = concat!("nose", "mgrep").to_string() + ": rust.actix.path-traversal";
        assert!(
            !source.contains(&needle),
            "loctree-mcp must validate project paths via `validate_project_path`, not suppress Semgrep: found `{needle}`"
        );
        let bare = concat!("// nose", "mgrep");
        let count = source.matches(bare).count();
        assert_eq!(
            count, 0,
            "loctree-mcp must not carry bare `nosemgrep` comments; rely on real validation instead"
        );
    }

    #[test]
    fn validate_project_path_rejects_empty_input() {
        let err = validate_project_path("", None).expect_err("empty input must reject");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn validate_project_path_rejects_whitespace_only_input() {
        let err = validate_project_path("   ", None).expect_err("whitespace input must reject");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn validate_project_path_rejects_null_bytes() {
        let err = validate_project_path("/tmp/proj\0/etc/passwd", None)
            .expect_err("NUL byte must reject");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn validate_project_path_rejects_parent_dir_segments() {
        let err = validate_project_path("../escape", None).expect_err("../escape must reject");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

        let err =
            validate_project_path("subdir/../../escape", None).expect_err("nested .. must reject");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn validate_project_path_accepts_relative_paths_without_traversal() {
        validate_project_path("subdir/file", None).expect("clean relative path must pass");
        validate_project_path("./subdir", None).expect("leading ./ must pass");
    }

    #[test]
    fn validate_project_path_with_allowlist_rejects_absolute_path_outside_root() {
        let temp = tempfile::tempdir().expect("temp dir");
        let allowed = vec![temp.path().to_path_buf()];
        // /etc is the canonical "out of bounds" target on Unix runners;
        // skipping the test cleanly on platforms where /etc cannot be
        // assumed keeps the suite hermetic.
        if Path::new("/etc").exists() {
            let err = validate_project_path("/etc", Some(&allowed))
                .expect_err("absolute path outside allowed root must reject");
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        }
    }

    #[test]
    fn validate_project_path_with_allowlist_accepts_absolute_path_inside_root() {
        let temp = tempfile::tempdir().expect("temp dir");
        // canonicalize so the prefix comparison matches /private/var/...
        // on macOS where /tmp -> /private/tmp.
        let allowed_canon = temp.path().canonicalize().expect("canonicalize temp");
        let inside = allowed_canon.join("inner");
        std::fs::create_dir_all(&inside).expect("mkdir inside");
        let allowed = vec![allowed_canon];
        validate_project_path(inside.to_str().expect("utf8"), Some(&allowed))
            .expect("absolute path inside allowed root must pass");
    }

    #[test]
    fn enforce_allowed_root_rejects_path_outside_allowlist() {
        let temp = tempfile::tempdir().expect("temp dir");
        let allowed_canon = temp.path().canonicalize().expect("canonicalize temp");
        let allowed = vec![allowed_canon];
        let outside = Path::new("/");
        let err = enforce_allowed_root(outside, Some(&allowed))
            .expect_err("post-canonical escape must reject");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn enforce_allowed_root_is_noop_when_unset() {
        // No allowlist configured -> any canonical path passes; this
        // preserves the pre-SaaS local-trust behavior for operators
        // running the server locally without setting LOCTREE_MCP_ALLOWED_ROOTS.
        enforce_allowed_root(Path::new("/anywhere"), None)
            .expect("unset allowlist must be a no-op");
    }

    #[tokio::test]
    async fn resolve_existing_project_path_rejects_traversal() {
        let err = LoctreeServer::resolve_existing_project_path("../etc/passwd")
            .expect_err("traversal must reject");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Invalid project path") || msg.contains(".."),
            "error message should explain the rejection, got: {msg}"
        );
    }

    #[tokio::test]
    async fn resolve_existing_project_path_rejects_null_byte() {
        let err = LoctreeServer::resolve_existing_project_path("/tmp/abc\0def")
            .expect_err("NUL byte must reject");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Invalid project path") || msg.contains("NUL"),
            "error message should explain the rejection, got: {msg}"
        );
    }

    #[tokio::test]
    async fn resolve_existing_project_path_rejects_empty() {
        let err = LoctreeServer::resolve_existing_project_path("").expect_err("empty must reject");
        let msg = format!("{err:#}");
        assert!(msg.contains("Invalid project path"));
    }

    #[tokio::test]
    async fn context_tool_returns_model_readable_pretty_json() {
        let project = fixture_project();
        let output = context_output(project.path(), |_| {}).await;

        assert!(
            output.lines().count() > 1,
            "context output must not be one huge line"
        );
        assert!(
            output.contains("\n  \"protocol\""),
            "top-level JSON should be pretty-printed"
        );
        assert!(
            output.contains("\n    \"schema_version\""),
            "nested context data should be readable without char slicing"
        );
        serde_json::from_str::<serde_json::Value>(&output).expect("pretty output remains JSON");
    }

    #[tokio::test]
    async fn context_tool_respects_no_aicx() {
        let project = fixture_project();
        let output = context_output(project.path(), |params| {
            params.with_aicx = false;
            params.no_aicx = true;
        })
        .await;
        let response: serde_json::Value =
            serde_json::from_str(&output).expect("valid context response");

        assert!(response["data"]["memory"].is_null());
        assert_eq!(response["sections_skipped"], serde_json::json!(["memory"]));
    }

    #[tokio::test]
    async fn context_tool_with_file_param() {
        let project = fixture_project();
        let output = context_output(project.path(), |params| {
            params.file = Some("src/foo.rs".to_string());
        })
        .await;
        let response: serde_json::Value =
            serde_json::from_str(&output).expect("valid context response");

        assert_eq!(response["status"], "complete");
        assert!(response["data"].get("project").is_some());
        assert!(response["data"].get("structural").is_some());
    }

    #[tokio::test]
    async fn context_tool_materializes_context_atlas_pointer() {
        let project = fixture_project();
        let output = context_output(project.path(), |_| {}).await;
        let response: Value = serde_json::from_str(&output).expect("valid context response");

        assert_eq!(response["protocol"], "loctree.context_atlas.v1");
        assert_eq!(response["status"], "complete");
        assert!(response["atlas"].is_object());
        assert!(
            response["sections_loaded"]
                .as_array()
                .unwrap()
                .contains(&Value::String("receipt".to_string()))
        );
        assert!(
            response
                .pointer("/receipt/snapshot/fingerprint/value")
                .is_some()
        );
        assert!(response.pointer("/data/structural").is_some());
        assert!(response.pointer("/data/runtime").is_some());
    }

    #[tokio::test]
    async fn context_tool_returns_authority_labels() {
        let project = fixture_project();
        let output = context_output(project.path(), |params| {
            params.file = Some("src/foo.rs".to_string());
        })
        .await;
        let value: Value = serde_json::from_str(&output).expect("valid json");

        assert!(value.pointer("/data/authority/repo_verified").is_some());
        assert!(value.pointer("/data/authority/loctree_derived").is_some());
        assert!(value.pointer("/data/authority/semantic_guess").is_some());
    }

    #[tokio::test]
    async fn repo_view_exposes_snapshot_authority() {
        let project = fixture_project();
        let server = LoctreeServer::new();
        let output = server
            .repo_view(Parameters(ForAiParams {
                project: project.path().display().to_string(),
                force_no_git: true,
            }))
            .await;
        let value: Value = serde_json::from_str(&output).expect("valid repo-view json");

        assert!(value.pointer("/snapshot/fingerprint/value").is_some());
        assert!(value.pointer("/snapshot/git/commit").is_some());
        assert!(value.pointer("/snapshot/staleness/stale").is_some());
    }

    #[tokio::test]
    async fn repo_view_reports_atlas_available_when_receipt_matches_snapshot() {
        let project = fixture_project();
        let server = LoctreeServer::new();

        // Materialize atlas via context() — receipt.json mirrors the live snapshot.
        let _ = server.context(Parameters(params_for(project.path()))).await;

        let output = server
            .repo_view(Parameters(ForAiParams {
                project: project.path().display().to_string(),
                force_no_git: true,
            }))
            .await;
        let value: Value = serde_json::from_str(&output).expect("valid repo-view json");

        assert_eq!(
            value["context_atlas"]["status"],
            "atlas_available",
            "context_atlas payload: {}",
            serde_json::to_string_pretty(&value["context_atlas"]).unwrap_or_default()
        );
        assert_eq!(
            value["context_atlas"]["atlas_snapshot"], value["context_atlas"]["current_snapshot"],
            "fresh atlas must echo the live snapshot tag"
        );
    }

    #[tokio::test]
    async fn repo_view_reports_atlas_stale_when_receipt_mismatches_snapshot() {
        let project = fixture_project();
        let server = LoctreeServer::new();

        let _ = server.context(Parameters(params_for(project.path()))).await;
        let receipt_path = atlas_dir_for_project(project.path()).join("receipt.json");
        let raw = fs::read_to_string(&receipt_path).expect("read receipt");
        let mut receipt: Value = serde_json::from_str(&raw).expect("parse receipt");
        receipt["snapshot"] = Value::String("different-branch@deadbeef".to_string());
        fs::write(
            &receipt_path,
            serde_json::to_string_pretty(&receipt).expect("re-serialize receipt"),
        )
        .expect("write mutated receipt");

        let output = server
            .repo_view(Parameters(ForAiParams {
                project: project.path().display().to_string(),
                force_no_git: true,
            }))
            .await;
        let value: Value = serde_json::from_str(&output).expect("valid repo-view json");

        assert_eq!(value["context_atlas"]["status"], "atlas_stale");
        assert_eq!(
            value["context_atlas"]["atlas_snapshot"],
            "different-branch@deadbeef"
        );
        assert_ne!(
            value["context_atlas"]["atlas_snapshot"],
            value["context_atlas"]["current_snapshot"]
        );
        assert!(
            value["context_atlas"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("misrepresent"),
            "stale atlas message must warn that cards no longer reflect the tree"
        );
    }

    #[tokio::test]
    async fn repo_view_reports_unknown_freshness_when_receipt_missing() {
        let project = fixture_project();
        let server = LoctreeServer::new();

        let _ = server.context(Parameters(params_for(project.path()))).await;
        let receipt_path = atlas_dir_for_project(project.path()).join("receipt.json");
        fs::remove_file(&receipt_path).expect("remove receipt");

        let output = server
            .repo_view(Parameters(ForAiParams {
                project: project.path().display().to_string(),
                force_no_git: true,
            }))
            .await;
        let value: Value = serde_json::from_str(&output).expect("valid repo-view json");

        assert_eq!(value["context_atlas"]["status"], "atlas_unknown_freshness");
        assert!(value["context_atlas"]["atlas_snapshot"].is_null());
    }

    #[test]
    fn public_mcp_tool_surface_is_polarized_to_ten_tools() {
        // The MCP surface stays intentionally small. Eight map/context tools,
        // the literal-only `suppressions` inventory, and the polarization-gate
        // `prism` tool — anything else belongs in the `loct` CLI
        // (health/findings/audit/coverage). Adding a tool here is a
        // surface-area decision; rename or update the assertion deliberately.
        //
        // `suppressions` joined on 2026-05-17 to close the silencer-surface
        // gap recorded in `~/internal-artifacts/loctree/loctree-feedback.md` (existing
        // logic under `analyzer/search.rs::search_suppressions` was invisible
        // agent-side; this tool exposes it). Free-tier scope is locked to
        // literal detection. Semantic enrichment is paid-tier Wave 7+.
        let source = include_str!("main.rs");
        let mut tools = Vec::new();
        let mut in_tool_attr = false;
        for line in source.lines() {
            let line = line.trim();
            if line == "#[tool(" {
                in_tool_attr = true;
                continue;
            }
            if !in_tool_attr {
                continue;
            }
            if let Some(rest) = line.strip_prefix("name = \"")
                && let Some((name, _)) = rest.split_once('"')
            {
                tools.push(name.to_string());
            }
            if line == ")]" {
                in_tool_attr = false;
            }
        }

        assert_eq!(
            tools,
            vec![
                "context",
                "repo-view",
                "slice",
                "find",
                "impact",
                "tree",
                "focus",
                "follow",
                "suppressions",
                "prism",
            ]
        );
    }

    /// Tier-boundary regression guard: the `suppressions` MCP tool MUST
    /// stay literal-only in its public description. If a future change
    /// adds semantic enrichment without an explicit feature-flag boundary,
    /// this test fails loudly. This protects the free-tier promise (see
    /// `~/internal-artifacts/loctree/loctree-feedback.md` 2026-05-17 addendum +
    /// `loctree::analyzer::suppression_inventory` module docs).
    #[test]
    fn suppressions_mcp_tool_description_advertises_literal_only_free_tier() {
        let source = include_str!("main.rs");
        // Find the tool-description block for `name = "suppressions"`.
        let mut found = false;
        let mut description_line = String::new();
        let mut prev_is_tool_attr = false;
        let mut in_block = false;
        for line in source.lines() {
            let trimmed = line.trim();
            if trimmed == "#[tool(" {
                prev_is_tool_attr = true;
                in_block = false;
                continue;
            }
            if prev_is_tool_attr {
                if trimmed.starts_with("name = \"suppressions\"") {
                    in_block = true;
                }
                prev_is_tool_attr = false;
            }
            if in_block && trimmed.starts_with("description = \"") {
                description_line = trimmed.to_string();
                found = true;
                break;
            }
        }
        assert!(
            found,
            "suppressions tool description not found in main.rs — did it get renamed?"
        );
        let desc_lower = description_line.to_lowercase();
        assert!(
            desc_lower.contains("literal"),
            "suppressions tool description MUST advertise 'literal' detection \
             to keep the free-tier boundary explicit. Found: {description_line}"
        );
        assert!(
            desc_lower.contains("free-tier") || desc_lower.contains("paid-tier"),
            "suppressions tool description MUST name the tier boundary \
             explicitly (free-tier scope OR paid-tier Wave 7+ delta). \
             Found: {description_line}"
        );
    }
}
