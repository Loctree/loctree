//! Public composition API for ContextPack.
//!
//! MCP servers, IDE extensions, and other library consumers should import from
//! this module, not from `crate::cli::*` modules.
//!
//! The CLI uses the same API through compatibility re-exports.
//!
//! Original Cut 4 layout:
//!
//! Cut 4 layout:
//! - T0 (codex)        — command skeleton + ContextPack schema (empty slices).
//! - T1 (sibling)      — fills `structural` (in HEAD).
//! - T2 (this file)    — fills `runtime` from `SemanticFacts` + Tauri bridges.
//! - T3 (sibling)      — fills `risk` + `action` (concurrent).
//!
//! T2 owns the runtime slice composer plus runtime sub-types (idiom tags,
//! dispatch edges, reachability, env contracts, Tauri bridges, framework
//! hints). The `AuthorityLabel` enum was introduced by T1 and is reused here.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::aicx::{
    AicxClient, IntentAuthority, ScopeKeywords, SemanticReadiness, authority_for_intent,
    is_aicx_available, score_intent, summarize_entry,
};
use crate::aicx::redact::redact_secrets;
use crate::analyzer::classify::{ArtifactClass, artifact_class};
use crate::analyzer::env_truth::source_reads::collect_source_env_reads;
use crate::cli::command::GlobalOptions;
use crate::cli::dispatch::DispatchResult;
use crate::context_render::{aicx_read_chunk_footer, chunk_ref};
use crate::context_scope::{
    ResolvedScope, ScopeMode, ScopeReport, TaskReport, resolve_scope_with_mode,
};
use crate::context_stack::{
    PackageManager, ProjectStack, dedup_top_n, detect_project_stack, extract_ci_test_commands,
    read_makefile_test_targets,
};
use crate::metrics::{importer_counts_direct, top_hubs_by_importers_direct};
use crate::query::query_who_imports;
use crate::receipt::QueryReceipt;
use crate::semantic::{
    Classifier, DispatchEdge, DispatchKind, EnvContract, IdiomTag, ReachReason, RuntimeRole,
    SemanticFacts, TagSource,
};
use crate::slicer::{HolographicSlice, SliceConfig};
use crate::snapshot::{
    CommandBridge, EventBridge, Snapshot, normalize_roots_for_scope_compare, resolve_snapshot_root,
};
use crate::types::{ImportKind, ImportResolutionKind, OutputMode};

// 1.1: additive `receipt` field (loctree.receipt.v1 identity binding, W1-A).
pub const CONTEXT_SCHEMA_VERSION: &str = "1.1";
const MAKE_RUNTIME_TARGET_LIMIT: usize = 6;
/// Bare/default context keeps at most this many target files. The pack must
/// declare how many were kept vs the indexed universe so agents never read a
/// truncated slice as the whole repo (G-FULL-SCOPE).
const DEFAULT_SCOPE_TARGET_LIMIT: usize = 8;
/// `--task` ranker: a file named verbatim in the task outranks substring
/// token hits (path +5 / export +3) so `launch.sh` is not buried by recency
/// hotspots that merely share a word (G-TASK-RANKING).
const EXACT_NAME_TASK_BOOST: usize = 50;

/// Options for composing a ContextPack.
#[derive(Debug, Clone, Default)]
pub struct ContextOptions {
    /// Focus the context pack on a specific file.
    pub file: Option<PathBuf>,

    /// Limit the context pack to changed files.
    pub changed: bool,

    /// Natural-language task hint for context narrowing.
    pub task: Option<String>,

    /// Deterministic structural scope selectors. Repeatable; default combine is AND.
    /// Comma-joined selectors in one value (`path:a,path:b`) are a union.
    pub scopes: Vec<String>,

    /// How repeated `--scope` flags combine. Default [`ScopeMode::All`] (AND).
    /// `--scope-mode any` unions them. Comma-joined values stay a union
    /// regardless of this flag.
    pub scope_mode: ScopeMode,

    /// Include AICX memory overlay.
    pub with_aicx: bool,

    /// Disable AICX memory overlay, including bare-context auto overlay.
    pub no_aicx: bool,

    /// Project root to use for identity and snapshot loading.
    pub project: Option<PathBuf>,

    /// Operator override for the AICX project bucket (`aicx -p <bucket>`).
    ///
    /// Plan L04 / Finding #16 — the legacy resolver guesses the bucket
    /// from the cwd folder name (`PathBuf::file_name()`), which is wrong
    /// for monorepos, fixtures, worktrees, and any directory layout
    /// where the folder name does not match the AICX bucket. When this
    /// field is `Some(...)`, [`aicx_project_bucket`] returns the
    /// operator-supplied bucket verbatim, bypassing the heuristic.
    ///
    /// Wire format: `loct context --aicx-project <bucket>`.
    pub aicx_project_override: Option<String>,

    /// Emit JSON output.
    pub json: bool,

    /// Emit Markdown output.
    pub markdown: bool,

    /// Emit the full ContextPack JSON.
    pub full: bool,
    /// Explicit intent-window override in hours for the memory slice.
    /// `None` falls back to `LOCT_CONTEXT_MEMORY_HOURS`, then the 168 h
    /// default. Callers with a per-request value (the LSP `hours` field)
    /// set this instead of mutating the process environment.
    pub memory_hours: Option<u64>,
    /// Explicit cap on memory-slice entries. `None` falls back to
    /// `LOCT_CONTEXT_MEMORY_LIMIT`, then 50.
    pub memory_limit: Option<usize>,
}

/// Provenance label attached to every fact in the ContextPack.
///
/// Cut 4 T1 emits `RepoVerified` exclusively (everything in the structural slice
/// derives from the live snapshot which was built from real source). Other tracks
/// (T2 runtime, T3 risk/action, Cut 5 AICX overlay) reuse this enum.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityLabel {
    /// Fact comes from a verified repo artifact (snapshot, file content, git).
    RepoVerified,
    /// Derived structural inference loctree itself produced.
    LoctreeDerived,
    /// AICX-recorded operator decision.
    AicxOperator,
    /// AICX-recorded agent claim.
    AicxAgent,
    /// AICX-recorded failure or rollback.
    AicxFailure,
    /// Heuristic or semantic best-guess (Layer 3 idiom inference).
    SemanticGuess,
    /// Stale or unknown — caller should re-verify.
    #[default]
    StaleOrUnknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPack {
    pub schema_version: String,
    pub project: ProjectIdentity,
    /// How many targets this pack selected versus the indexed universe.
    /// Silent `truncate(8)` without this field reads as whole-repo truth.
    #[serde(default)]
    pub coverage: PackCoverage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<ScopeReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskReport>,
    pub structural: StructuralSlice,
    pub runtime: RuntimeSlice,
    pub risk: RiskSlice,
    pub action: ActionSlice,
    pub memory: MemorySlice,
    pub authority: AuthoritySlice,
    /// Identity receipt binding this pack to root, live HEAD, dirty state,
    /// snapshot fingerprint, and the answering binary (`loctree.receipt.v1`).
    #[serde(default)]
    pub receipt: QueryReceipt,
}

/// Declared target coverage for a ContextPack (bare and `--full`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackCoverage {
    /// Targets actually composed into this pack.
    pub selected: usize,
    /// Indexed files in the snapshot (the universe the selection was drawn from).
    pub total: usize,
    /// `true` when `selected < total` — the pack is a slice, not the repo.
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectIdentity {
    pub canonical_root: Option<String>,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub snapshot_id: Option<String>,
}

/// Role a file plays in the structural slice relative to the focus target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuralRole {
    /// The file under focus.
    Target,
    /// File transitively imported by the target.
    Dependency,
    /// File that imports (directly or via re-export chain) the target.
    Consumer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralFile {
    pub path: String,
    pub role: StructuralRole,
    pub depth: usize,
    pub language: String,
    pub loc: usize,
    pub authority: AuthorityLabel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralSymbol {
    pub name: String,
    pub kind: String,
    pub export_type: String,
    pub file: String,
    pub line: Option<usize>,
    pub authority: AuthorityLabel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralImport {
    /// File that contains this import statement.
    pub file: String,
    /// Resolved/normalized source (canonical).
    pub source: String,
    /// Source as written in code (raw spelling).
    pub source_raw: String,
    /// Import kind (static / type / side_effect / dynamic).
    pub kind: String,
    /// Resolution outcome (local / stdlib / dynamic / unknown).
    pub resolution: String,
    pub resolved_path: Option<String>,
    pub line: Option<usize>,
    pub symbols: Vec<String>,
    pub is_bare: bool,
    pub authority: AuthorityLabel,
}

/// How a consumer reaches the target file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsumerKind {
    /// Consumer has a direct import edge to the target.
    Direct,
    /// Consumer imports a barrel/re-export module that points to the target.
    Reexport,
    /// Consumer reaches the target only through transitive paths (no direct edge).
    Transitive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralConsumer {
    pub file: String,
    pub import_kind: ConsumerKind,
    /// Symbol names from the target this consumer references (best-effort: only
    /// resolved when consumer's import row carries explicit symbol bindings).
    pub imports_used: Vec<String>,
    pub authority: AuthorityLabel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralEntrypoint {
    pub path: String,
    pub kinds: Vec<String>,
    pub authority: AuthorityLabel,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StructuralSlice {
    pub files: Vec<StructuralFile>,
    pub symbols: Vec<StructuralSymbol>,
    pub imports: Vec<StructuralImport>,
    pub consumers: Vec<StructuralConsumer>,
    pub entrypoints: Vec<StructuralEntrypoint>,
}

/// Cut 4 T2 — runtime semantic slice.
///
/// Surfaces Layer 3 [`SemanticFacts`] (idiom tags, dispatch edges, reachability,
/// env contracts) plus Layer 1 Tauri bridges (`CommandBridge` / `EventBridge`)
/// plus per-file framework metadata (FastAPI / Flask routes, pytest fixtures,
/// entrypoints, Rust trait-impl methods, Python decorator handlers).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeSlice {
    /// Idiom-tagged symbols whose `<file>::<symbol>` SymbolId falls in scope.
    pub idiom_tags: Vec<RuntimeIdiomTag>,
    /// Dispatch edges (case-statement, function-pointer, eval-string,
    /// recipe-shell-call, Tauri invoke/event) where target file participates.
    pub dispatch_edges: Vec<RuntimeDispatchEdge>,
    /// Reachability claims (reached / unreached) for symbols within scope.
    pub reachability: Vec<RuntimeReachability>,
    /// Env vars used by files in scope, with cross-file usage list.
    pub env_contracts: Vec<RuntimeEnvContract>,
    /// Tauri command bridges (`#[tauri::command]` handler + invoke sites).
    pub tauri_commands: Vec<RuntimeTauriCommand>,
    /// Tauri event bridges (emit / listen pairs).
    pub tauri_events: Vec<RuntimeTauriEvent>,
    /// Framework-specific reachability hints — FastAPI/Flask routes, pytest
    /// fixtures, Rust trait/impl methods, Python decorator handlers, entrypoints.
    pub framework_hints: Vec<RuntimeFrameworkHint>,
    /// What the repository-wide runtime inventory actually inspected. This is
    /// intentionally explicit so an omitted class can never masquerade as a
    /// proven zero in rendered cards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_coverage: Option<RuntimeInventoryCoverage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeInventoryCoverage {
    pub owner_classes: Vec<String>,
    pub env_read_classes: Vec<String>,
    pub source_files_scanned: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeIdiomTag {
    /// Stable `<file>::<symbol>` identifier.
    pub symbol: String,
    pub name: String,
    pub classifier: String,
    pub runtime_role: String,
    pub source: String,
    pub reasoning: String,
    pub authority: AuthorityLabel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeDispatchEdge {
    pub from_file: String,
    pub from_line: u32,
    /// Dispatch kind label (`case_statement`, `function_pointer`, `eval_string`,
    /// `recipe_shell_call`, `tauri_invoke`, `tauri_event`, `http_route`,
    /// `cli_command`, `event_handler`, `task_target`).
    pub dispatch_kind: String,
    pub handler_symbol: String,
    pub handler_file: Option<String>,
    /// Framework label (e.g. `fastapi`, `flask`, `typer`, `celery`) when the
    /// edge originated from a known framework decorator. Lifted from the
    /// matching `RouteInfo` (HTTP routes) or inferred from decorator pattern.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    /// HTTP method (`GET`, `POST`, …) — only populated for `http_route` edges
    /// when the matching route metadata is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_method: Option<String>,
    /// HTTP route path — only populated for `http_route` edges when the
    /// matching route metadata is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_path: Option<String>,
    pub authority: AuthorityLabel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeReachability {
    /// Stable `<file>::<symbol>` identifier.
    pub symbol: String,
    /// `true` for `reached_symbols`, `false` for `unreached_symbols`.
    pub reached: bool,
    /// Human-readable reason summary (`direct_import`,
    /// `dispatch_handler:case_statement:foo`,
    /// `idiom_runtime_role:user_facing`, …).
    pub reason: String,
    pub authority: AuthorityLabel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEnvContract {
    pub name: String,
    pub used_in_files: Vec<String>,
    pub required_for: Vec<String>,
    /// Per-call-site detail (file / line / access kind / default / required).
    /// Empty for env contracts surfaced from analyzers that only track
    /// aggregated facts (Make, Shell). Populated by the Python analyzer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub occurrences: Vec<RuntimeEnvOccurrence>,
    /// True when at least one occurrence is recorded as `required` (i.e. a
    /// read site that does not provide a default and will raise on missing).
    pub required: bool,
    pub authority: AuthorityLabel,
}

/// Per-read-site detail for an env contract. Mirrors
/// [`crate::semantic::EnvContractOccurrence`] with `AuthorityLabel` plumbed in
/// so the agent surface stays uniform with every other claim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEnvOccurrence {
    pub file: String,
    pub line: u32,
    pub access_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeTauriCommand {
    /// Command name (Rust handler `fn` name or invoke string).
    pub name: String,
    pub handler_file: Option<String>,
    pub handler_line: Option<usize>,
    pub invoke_site_count: usize,
    pub has_handler: bool,
    pub is_called: bool,
    pub authority: AuthorityLabel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeTauriEvent {
    pub name: String,
    pub emit_count: usize,
    pub listen_count: usize,
    pub is_fe_sync: bool,
    pub same_file_sync: bool,
    pub authority: AuthorityLabel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeFrameworkHint {
    /// Hint kind label (`fastapi_route`, `pytest_fixture`, `python_decorator`,
    /// `entrypoint`, `rust:trait_impl_method`, …).
    pub kind: String,
    pub symbol: String,
    pub file: String,
    pub line: Option<u32>,
    /// Optional sub-detail (HTTP method, decorator expression, …).
    pub detail: Option<String>,
    pub authority: AuthorityLabel,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RiskSlice {
    pub hotspots: Vec<HotspotFile>,
    pub high_fan_in: Vec<HighFanInFile>,
    pub snapshot_health: Option<String>,
    pub cache_scope: RiskCacheScope,
    pub cache_scope_authority: AuthorityLabel,
    pub stale_snapshot: bool,
    pub dirty_worktree: bool,
    /// Git status/HEAD could not be determined. Fail-closed: never treat
    /// `dirty_worktree == false` as a verified clean worktree (G-DIRTY-IDENTITY).
    #[serde(default)]
    pub git_unknown: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotspotFile {
    pub file: String,
    pub importers: usize,
    pub authority: AuthorityLabel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighFanInFile {
    pub file: String,
    pub importers: usize,
    pub threshold: usize,
    pub authority: AuthorityLabel,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskCacheScope {
    Clean,
    DirtyWorktree,
    StaleSnapshot,
    MissingSnapshot,
    Scoped(String),
    #[default]
    Unknown,
}

impl RiskSlice {
    fn missing_snapshot() -> Self {
        Self {
            cache_scope: RiskCacheScope::MissingSnapshot,
            cache_scope_authority: AuthorityLabel::RepoVerified,
            snapshot_health: Some("missing_snapshot".to_string()),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SuggestedCommand {
    pub command: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionSlice {
    pub next_safe_commands: Vec<String>,
    pub verification_gates: Vec<String>,
    pub likely_tests: Vec<String>,
    #[serde(skip)]
    pub next_safe_command_authorities: Vec<ActionAuthorityClaim>,
    #[serde(skip)]
    pub verification_gate_authorities: Vec<ActionAuthorityClaim>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub power_path: Vec<SuggestedCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionAuthorityClaim {
    pub item: String,
    pub authority: AuthorityLabel,
}

/// Cut 5 T1 — single AICX-derived memory entry.
///
/// Each entry carries enough provenance for an agent to grep the original
/// chunk on disk (`source_chunk`) and re-read the conversation context that
/// produced the decision.
///
/// **Relevance fields** (Plan L02 / Finding #11):
/// - `relevance` is the **local** keyword-overlap score against the
///   in-flight scope keywords. It is computed by the composer.
/// - `retrieval_score` / `retrieval_label` / `retrieval_mode` come from
///   AICX's own ranker (`aicx search --json` returns `score`, `label`).
///   They stay `None` until `compose_memory_slice` is wired to
///   `client.search()`. Future work — but the field is reserved now so
///   the future change is purely additive and the local heuristic score
///   never silently overwrites a stronger AICX signal.
/// - `low_lexical_match` is `true` for entries kept by the score==0
///   newest-first fallback (Plan L02 / Finding #12). Reports / UI
///   should present these with the caveat "no keyword overlap — falling
///   back to recency".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// AICX intent kind: `decision`, `intent`, `outcome`, `task`.
    pub kind: String,
    /// Short summary text from `aicx intents`.
    pub text: String,
    /// Provenance label per the AICX authority hierarchy.
    pub authority: AuthorityLabel,
    /// Absolute path to the source markdown chunk under `~/.aicx/store/...`.
    pub source_chunk: String,
    /// Authoring agent name (`claude`, `codex`, `gemini`, ...).
    pub agent: String,
    /// ISO date (YYYY-MM-DD) of the source chunk.
    pub date: String,
    /// Full ISO 8601 timestamp when AICX recorded one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// AICX session id segment (truncated form used by AICX itself).
    pub session_id: String,
    /// AICX project bucket the entry was retrieved from.
    pub project: String,
    /// Local token-overlap relevance score against the in-flight scope keywords.
    /// 0 indicates a recency-fallback entry; see `low_lexical_match`.
    pub relevance: u32,
    /// AICX-side retrieval score (from `AicxSearchResult.score`). `None`
    /// until `compose_memory_slice` is wired to `client.search()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval_score: Option<i64>,
    /// AICX-side retrieval label (e.g. `HIGH` / `MEDIUM` / `LOW`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval_label: Option<String>,
    /// Retrieval mode: `semantic`, `fuzzy_fallback`, `filesystem`. Reflects
    /// how AICX answered the query. `None` for the legacy `intents()` path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval_mode: Option<String>,
    /// `true` when this entry was promoted by the score==0 newest-first
    /// fallback (Plan L02 / Finding #12) — the keyword bag had no overlap
    /// with the intent's text so it was kept on recency alone.
    #[serde(default, skip_serializing_if = "is_false")]
    pub low_lexical_match: bool,
}

/// Serde helper for `#[serde(skip_serializing_if = "is_false")]`.
fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemorySlice {
    /// Ranked, scope-filtered memory entries (most relevant first).
    pub entries: Vec<MemoryEntry>,
    /// De-duplicated source chunk paths referenced by `entries`. Provided
    /// separately so an agent can quickly grep the underlying conversation
    /// without iterating `entries`.
    pub source_chunks: Vec<String>,
    /// Always populated when AICX overlay was attempted. Distinguishes between
    /// "no memory exists", "matcher returned nothing", and "AICX unreachable"
    /// — three states that previously all looked like an empty `entries` array.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<MemoryDiagnostic>,
    /// C0-01 intent layer for the bare path (I1-01): typed theses from the
    /// cached `aicx overlay` emission, full-key identity, and an explicit
    /// freshness verdict. The bare render never waits for the producer —
    /// this is always served from `.loctree/aicx-overlay.v1.json`, with the
    /// detached refresh converging the cache between invocations. `None` on
    /// non-bare compositions and under `--no-aicx` / `--with-aicx` (the
    /// patient `entries` path above stays the forensic-recall surface).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay: Option<crate::aicx::overlay::OverlayRenderState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDiagnostic {
    /// True when the overlay actually ran (`with_aicx=true`, `no_aicx=false`,
    /// AICX client constructed). False when overlay was disabled / unavailable.
    pub engaged: bool,
    /// AICX project bucket the overlay queried (the canonical project root's
    /// directory name).
    pub namespace: String,
    /// How the overlay seeded its keyword bag: `target_file`, `default_scope`,
    /// `task_keywords`, `fallback_branch_commit`, etc.
    pub seed_strategy: String,
    /// Number of raw intents fetched from AICX before relevance filtering.
    pub candidates_considered: usize,
    /// Number of intents that survived relevance scoring and made it into
    /// `entries`.
    pub candidates_returned: usize,
    /// Drawn from a closed enum so an agent can branch on the cause without
    /// regex-parsing free text. See [`MemorySkipReason`] for the catalog.
    pub skip_reason: MemorySkipReason,
    /// Retrieval-layer readiness derived from AICX's [`OracleStatus`].
    /// Lets a context consumer distinguish a semantic-oracle answer from
    /// a canonical-corpus scan, a fuzzy fallback, or an unknown wire.
    /// Defaults to [`SemanticReadiness::Unknown`] so older serialised
    /// diagnostics round-trip without breakage. See
    /// [`crate::aicx::SemanticReadiness`] for the mapping rules.
    #[serde(default)]
    pub semantic_readiness: SemanticReadiness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySkipReason {
    /// Overlay engaged and at least one entry returned.
    Ok,
    /// Caller passed `no_aicx=true`.
    DisabledByNoAicx,
    /// `with_aicx=false` and not in bare-context auto-overlay path.
    DisabledOptOut,
    /// AICX binary unavailable on PATH / not configured.
    AicxUnreachable,
    /// AICX transport timed out (or the auto-overlay wall-clock budget ran
    /// dry) before the store could answer. Distinct from [`Self::NamespaceEmpty`]:
    /// "store never got to answer" is not "store answered: nothing there".
    /// Raise `LOCT_CONTEXT_AICX_BUDGET_MS` / `LOCT_AICX_TIMEOUT_SECS` or use
    /// `--with-aicx` (patient, unbudgeted) for forensic recall.
    TimedOut,
    /// AICX returned no rows for this namespace (or with the chosen window).
    NamespaceEmpty,
    /// AICX returned candidates but the relevance filter dropped all of them
    /// (the matcher had no token overlap with the in-flight scope).
    NoTokenOverlap,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthoritySlice {
    pub repo_verified: Vec<String>,
    pub loctree_derived: Vec<String>,
    pub aicx_operator: Vec<String>,
    pub aicx_agent: Vec<String>,
    pub aicx_failure: Vec<String>,
    pub semantic_guess: Vec<String>,
    pub stale_or_unknown: Vec<String>,
}

impl ContextPack {
    pub fn empty(project: ProjectIdentity) -> Self {
        Self {
            schema_version: CONTEXT_SCHEMA_VERSION.to_string(),
            project,
            coverage: PackCoverage::default(),
            scope: None,
            task: None,
            structural: StructuralSlice::default(),
            runtime: RuntimeSlice::default(),
            risk: RiskSlice::default(),
            action: ActionSlice::default(),
            memory: MemorySlice::default(),
            authority: AuthoritySlice::default(),
            receipt: QueryReceipt::unbound(),
        }
    }
}

#[derive(Debug)]
pub enum ContextLoadError {
    NoSnapshotNoScanMode {
        root: PathBuf,
    },
    StaleInCiMode {
        current: String,
        snapshot: String,
    },
    /// Wrong-commit class (audit LCT-D/E/L): a snapshot loaded right after a
    /// scan — including `--fresh` — names a different commit than live git.
    /// Fail closed: no receipt may claim fresh authority over that answer.
    PostScanIdentityMismatch {
        live_head: String,
        snapshot_commit: String,
        root: PathBuf,
    },
    ScanFailed(io::Error),
    IncrementalRescanFailed(io::Error),
    PostScanLoadFailed(io::Error),
    LatestSnapshotLoadFailed(io::Error),
    Scope(crate::context_scope::ScopeError),
}

impl std::fmt::Display for ContextLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContextLoadError::NoSnapshotNoScanMode { root } => {
                write!(
                    f,
                    "no snapshot in {} and --no-scan in effect",
                    root.display()
                )
            }
            ContextLoadError::StaleInCiMode { current, snapshot } => write!(
                f,
                "snapshot is stale (current git {current}, snapshot {snapshot}) and --fail-stale in effect"
            ),
            ContextLoadError::PostScanIdentityMismatch {
                live_head,
                snapshot_commit,
                root,
            } => write!(
                f,
                "identity mismatch after scan in {}: live HEAD {} vs snapshot {} — authority refused",
                root.display(),
                short_sha(live_head),
                short_sha(snapshot_commit)
            ),
            ContextLoadError::ScanFailed(err) => write!(f, "scan failed: {err}"),
            ContextLoadError::IncrementalRescanFailed(err) => {
                write!(f, "incremental rescan failed: {err}")
            }
            ContextLoadError::PostScanLoadFailed(err) => {
                write!(f, "snapshot load failed after scan: {err}")
            }
            ContextLoadError::LatestSnapshotLoadFailed(err) => {
                write!(f, "latest snapshot load failed: {err}")
            }
            ContextLoadError::Scope(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ContextLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ContextLoadError::ScanFailed(err)
            | ContextLoadError::IncrementalRescanFailed(err)
            | ContextLoadError::PostScanLoadFailed(err)
            | ContextLoadError::LatestSnapshotLoadFailed(err) => Some(err),
            ContextLoadError::NoSnapshotNoScanMode { .. }
            | ContextLoadError::StaleInCiMode { .. }
            | ContextLoadError::PostScanIdentityMismatch { .. }
            | ContextLoadError::Scope(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanKind {
    Initial,
    Incremental,
    Full,
    Fresh,
}

pub fn compose_context_pack(
    opts: &ContextOptions,
    project_root: &Path,
) -> anyhow::Result<ContextPack> {
    compose_context_pack_with_global(opts, project_root, &GlobalOptions::default())
        .map_err(anyhow::Error::from)
}

pub(crate) fn compose_context_pack_with_global(
    opts: &ContextOptions,
    project_root: &Path,
    global: &GlobalOptions,
) -> Result<ContextPack, ContextLoadError> {
    let mut effective_opts = opts.clone();
    if effective_opts.project.is_none() {
        effective_opts.project = Some(project_root.to_path_buf());
    }

    let mut pack = ContextPack::empty(project_identity(&effective_opts));
    let bare_context = is_bare_context(&effective_opts);

    let aicx_client = build_context_aicx_client(&effective_opts);

    let snapshot = try_load_snapshot_with_auto_scan(&effective_opts, global)?;
    pack.receipt = QueryReceipt::bind(&context_snapshot_root(&effective_opts), Some(&snapshot));
    // I1-01: the bare path consumes the C0-01 `aicx overlay` cache instead
    // of the budgeted `aicx intents` shell-out (which timed out on 100% of
    // bare invocations against a live store). Pure read — never waits.
    let overlay_state =
        compose_bare_overlay_state(&effective_opts, bare_context, project_root, &snapshot);
    {
        let resolved_scope =
            resolve_context_scope_for_opts(&effective_opts, project_root, &snapshot)?;
        if effective_opts.file.is_some() && !effective_opts.scopes.is_empty() {
            eprintln!("warning: --file narrows to a single file; --scope ignored");
        }
        if let Some(scope) = &resolved_scope {
            pack.scope = Some(scope.report.clone());
        }
        pack.task = task_report_for_opts(&effective_opts, resolved_scope.is_some());

        let mut targets = if let Some(scope) = &resolved_scope {
            scoped_targets(&effective_opts, &snapshot, scope)
        } else {
            determine_targets(&effective_opts, &snapshot)
        };
        retain_context_targets(&mut targets);
        if targets.is_empty() && bare_context && resolved_scope.is_none() {
            targets = compose_default_scope(
                &snapshot,
                &effective_opts,
                aicx_client.as_ref(),
                overlay_state.as_ref(),
            );
            retain_context_targets(&mut targets);
        }
        bind_pack_coverage(&mut pack, &snapshot, targets.len());
        if targets.is_empty() && resolved_scope.is_some() {
            pack.risk = compose_risk_slice(&effective_opts, &snapshot);
        } else if targets.is_empty() && (effective_opts.changed || effective_opts.task.is_some()) {
            if effective_opts.changed {
                eprintln!("Not a git repository or no changed files found in snapshot.");
            } else {
                eprintln!("No files matched task description above threshold.");
            }
            if !effective_opts.json {
                pack.risk = RiskSlice::missing_snapshot();
                merge_risk_action_into_authority(&pack.risk, &pack.action, &mut pack.authority);
            }
        } else if !targets.is_empty() {
            for target in targets {
                let mut single_opts = effective_opts.clone();
                single_opts.file = Some(context_target_path(&target));

                let structural = compose_structural_slice(&single_opts, &snapshot);
                let runtime = compose_runtime_slice(&single_opts, &snapshot);
                let risk = compose_risk_slice(&single_opts, &snapshot);
                let action =
                    compose_action_slice(&single_opts, &snapshot, &structural, &runtime, &risk);
                let memory =
                    compose_memory_slice(&single_opts, &structural, &runtime, aicx_client.as_ref());

                merge_structural(&mut pack.structural, structural);
                merge_runtime(&mut pack.runtime, runtime);
                merge_risk(&mut pack.risk, risk);
                merge_action(&mut pack.action, action);
                merge_memory(&mut pack.memory, memory);
            }
            dedup_structural(&mut pack.structural);
            dedup_runtime(&mut pack.runtime);
            dedup_risk(&mut pack.risk);
            dedup_action(&mut pack.action);
            dedup_memory(&mut pack.memory);

            merge_runtime_into_authority(&pack.runtime, &mut pack.authority);
            merge_risk_action_into_authority(&pack.risk, &pack.action, &mut pack.authority);
            merge_memory_into_authority(&pack.memory, &mut pack.authority);
        } else {
            pack.structural = compose_structural_slice(&effective_opts, &snapshot);
            pack.runtime = compose_runtime_slice(&effective_opts, &snapshot);
            pack.risk = compose_risk_slice(&effective_opts, &snapshot);
            pack.action = compose_action_slice(
                &effective_opts,
                &snapshot,
                &pack.structural,
                &pack.runtime,
                &pack.risk,
            );
            pack.memory = compose_memory_slice(
                &effective_opts,
                &pack.structural,
                &pack.runtime,
                aicx_client.as_ref(),
            );
            merge_runtime_into_authority(&pack.runtime, &mut pack.authority);
            merge_risk_action_into_authority(&pack.risk, &pack.action, &mut pack.authority);
            merge_memory_into_authority(&pack.memory, &mut pack.authority);
        }
    }
    if bare_context && pack.scope.is_none() {
        enrich_repo_runtime_inventory(project_root, &snapshot, &mut pack.runtime);
        dedup_runtime(&mut pack.runtime);
        merge_runtime_into_authority(&pack.runtime, &mut pack.authority);
        dedup_authority(&mut pack.authority);
    }
    pack.memory.overlay = overlay_state;
    scrub_context_pack(&mut pack);
    apply_scope_cache_marker(&mut pack);

    // Spec: when --with-aicx is requested but the binary is missing, log the
    // gap on stderr instead of silently returning an empty memory slice.
    if effective_opts.with_aicx
        && !effective_opts.no_aicx
        && pack.memory.entries.is_empty()
        && !is_aicx_available()
    {
        eprintln!(
            "[loct][context] --with-aicx requested but `aicx` binary unavailable; \
             memory slice empty (install: cargo install aicx, or set LOCT_AICX_BINARY)"
        );
    }

    Ok(pack)
}

pub fn compose_context_pack_from_snapshot(
    opts: &ContextOptions,
    project_root: &Path,
    snapshot: &Snapshot,
) -> anyhow::Result<ContextPack> {
    let mut effective_opts = opts.clone();
    effective_opts.project = Some(project_root.to_path_buf());

    let mut pack = ContextPack::empty(project_identity(&effective_opts));
    pack.receipt = QueryReceipt::bind(&context_snapshot_root(&effective_opts), Some(snapshot));
    let bare_context = is_bare_context(&effective_opts);
    let aicx_client = build_context_aicx_client(&effective_opts);
    // I1-01 — same overlay-cache contract as
    // `compose_context_pack_with_global` above. Pure read, never spawns:
    // this entry point also serves the LSP.
    let overlay_state =
        compose_bare_overlay_state(&effective_opts, bare_context, project_root, snapshot);

    let resolved_scope = resolve_context_scope_for_opts(&effective_opts, project_root, snapshot)?;
    if effective_opts.file.is_some() && !effective_opts.scopes.is_empty() {
        eprintln!("warning: --file narrows to a single file; --scope ignored");
    }
    if let Some(scope) = &resolved_scope {
        pack.scope = Some(scope.report.clone());
    }
    pack.task = task_report_for_opts(&effective_opts, resolved_scope.is_some());

    let mut targets = if let Some(scope) = &resolved_scope {
        scoped_targets(&effective_opts, snapshot, scope)
    } else {
        determine_targets(&effective_opts, snapshot)
    };
    retain_context_targets(&mut targets);
    if targets.is_empty() && bare_context && resolved_scope.is_none() {
        targets = compose_default_scope(
            snapshot,
            &effective_opts,
            aicx_client.as_ref(),
            overlay_state.as_ref(),
        );
        retain_context_targets(&mut targets);
    }
    bind_pack_coverage(&mut pack, snapshot, targets.len());

    if targets.is_empty() && resolved_scope.is_some() {
        pack.risk = compose_risk_slice(&effective_opts, snapshot);
    } else if targets.is_empty() && (effective_opts.changed || effective_opts.task.is_some()) {
        if !effective_opts.json {
            pack.risk = RiskSlice::missing_snapshot();
            merge_risk_action_into_authority(&pack.risk, &pack.action, &mut pack.authority);
        }
    } else if !targets.is_empty() {
        for target in targets {
            let mut single_opts = effective_opts.clone();
            single_opts.file = Some(context_target_path(&target));

            let structural = compose_structural_slice(&single_opts, snapshot);
            let runtime = compose_runtime_slice(&single_opts, snapshot);
            let risk = compose_risk_slice(&single_opts, snapshot);
            let action = compose_action_slice(&single_opts, snapshot, &structural, &runtime, &risk);
            let memory =
                compose_memory_slice(&single_opts, &structural, &runtime, aicx_client.as_ref());

            merge_structural(&mut pack.structural, structural);
            merge_runtime(&mut pack.runtime, runtime);
            merge_risk(&mut pack.risk, risk);
            merge_action(&mut pack.action, action);
            merge_memory(&mut pack.memory, memory);
        }
        dedup_structural(&mut pack.structural);
        dedup_runtime(&mut pack.runtime);
        dedup_risk(&mut pack.risk);
        dedup_action(&mut pack.action);
        dedup_memory(&mut pack.memory);

        merge_runtime_into_authority(&pack.runtime, &mut pack.authority);
        merge_risk_action_into_authority(&pack.risk, &pack.action, &mut pack.authority);
        merge_memory_into_authority(&pack.memory, &mut pack.authority);
    } else {
        pack.structural = compose_structural_slice(&effective_opts, snapshot);
        pack.runtime = compose_runtime_slice(&effective_opts, snapshot);
        pack.risk = compose_risk_slice(&effective_opts, snapshot);
        pack.action = compose_action_slice(
            &effective_opts,
            snapshot,
            &pack.structural,
            &pack.runtime,
            &pack.risk,
        );
        pack.memory = compose_memory_slice(
            &effective_opts,
            &pack.structural,
            &pack.runtime,
            aicx_client.as_ref(),
        );
        merge_runtime_into_authority(&pack.runtime, &mut pack.authority);
        merge_risk_action_into_authority(&pack.risk, &pack.action, &mut pack.authority);
        merge_memory_into_authority(&pack.memory, &mut pack.authority);
    }

    if bare_context && pack.scope.is_none() {
        enrich_repo_runtime_inventory(project_root, snapshot, &mut pack.runtime);
        dedup_runtime(&mut pack.runtime);
        merge_runtime_into_authority(&pack.runtime, &mut pack.authority);
        dedup_authority(&mut pack.authority);
    }
    pack.memory.overlay = overlay_state;
    scrub_context_pack(&mut pack);
    apply_scope_cache_marker(&mut pack);

    Ok(pack)
}

pub(crate) fn missing_snapshot_context_pack(
    opts: &ContextOptions,
    project_root: &Path,
) -> ContextPack {
    let mut effective_opts = opts.clone();
    if effective_opts.project.is_none() {
        effective_opts.project = Some(project_root.to_path_buf());
    }
    let mut pack = ContextPack::empty(project_identity(&effective_opts));
    pack.receipt = QueryReceipt::bind(project_root, None);
    pack.risk = RiskSlice::missing_snapshot();
    merge_risk_action_into_authority(&pack.risk, &pack.action, &mut pack.authority);
    pack
}

pub fn run(opts: &ContextOptions, global: &GlobalOptions) -> DispatchResult {
    let project_root = context_snapshot_root(opts);
    let pack = match compose_context_pack_with_global(opts, &project_root, global) {
        Ok(pack) => pack,
        Err(ContextLoadError::NoSnapshotNoScanMode { root }) => {
            eprintln!(
                "[loct][context] no snapshot found in {} and --no-scan in effect; using empty ContextPack",
                root.display()
            );
            let mut effective_opts = opts.clone();
            if effective_opts.project.is_none() {
                effective_opts.project = Some(project_root.clone());
            }
            let mut pack = ContextPack::empty(project_identity(&effective_opts));
            pack.receipt = QueryReceipt::bind(&project_root, None);
            pack.risk = RiskSlice::missing_snapshot();
            merge_risk_action_into_authority(&pack.risk, &pack.action, &mut pack.authority);
            pack
        }
        Err(ContextLoadError::StaleInCiMode { current, snapshot }) => {
            eprintln!(
                "[loct][context] snapshot is stale (current git {} vs snapshot {}) and --fail-stale in effect",
                short_sha(&current),
                short_sha(&snapshot)
            );
            return DispatchResult::Exit(3);
        }
        Err(ContextLoadError::PostScanIdentityMismatch {
            live_head,
            snapshot_commit,
            root,
        }) => {
            eprintln!(
                "[loct][context] identity mismatch after scan in {}: live HEAD {} vs snapshot {} — authority: refused (wrong-commit class; not emitting a fresh receipt)",
                root.display(),
                short_sha(&live_head),
                short_sha(&snapshot_commit)
            );
            return DispatchResult::Exit(4);
        }
        Err(ContextLoadError::Scope(err)) => {
            eprintln!("{err}");
            return DispatchResult::Exit(2);
        }
        Err(err) => {
            eprintln!("[loct][context] failed to compose ContextPack: {err}");
            return DispatchResult::Exit(1);
        }
    };

    let want_json = context_wants_json(opts, global);
    if !want_json {
        println!("{}", format_context_pack_markdown(&pack));
        return DispatchResult::Exit(0);
    }

    match serde_json::to_string_pretty(&pack) {
        Ok(json) => {
            println!("{json}");
            DispatchResult::Exit(0)
        }
        Err(err) => {
            eprintln!("[loct][context] failed to serialize ContextPack: {err}");
            DispatchResult::Exit(1)
        }
    }
}

fn context_wants_json(opts: &ContextOptions, global: &GlobalOptions) -> bool {
    opts.json || opts.full || global.json
}

fn try_load_snapshot_with_auto_scan(
    opts: &ContextOptions,
    global: &GlobalOptions,
) -> Result<Snapshot, ContextLoadError> {
    let roots = context_roots(opts);
    let root = context_snapshot_root(opts);

    if global.fresh {
        eprintln!(
            "[loct][context] --fresh requested for {}, rescanning...",
            root.display()
        );
        run_context_scan(&root, &roots, true, ScanKind::Fresh, global.force_non_git)?;
        return load_snapshot_after_scan(&root);
    }

    match Snapshot::load(&root) {
        Ok(snapshot) => ensure_fresh_with_mtime_check(snapshot, &root, &roots, global),
        Err(err) if err.kind() == io::ErrorKind::NotFound => match load_latest_snapshot(&root) {
            Ok(Some(snapshot)) => ensure_fresh_with_mtime_check(snapshot, &root, &roots, global),
            Ok(None) => {
                if global.no_scan {
                    return Err(ContextLoadError::NoSnapshotNoScanMode { root });
                }
                eprintln!(
                    "[loct][context] no snapshot found in {}, scanning... (use --no-scan to skip)",
                    root.display()
                );
                run_context_scan(
                    &root,
                    &roots,
                    false,
                    ScanKind::Initial,
                    global.force_non_git,
                )?;
                load_snapshot_after_scan(&root)
            }
            Err(err) => Err(ContextLoadError::LatestSnapshotLoadFailed(err)),
        },
        Err(err) => Err(ContextLoadError::LatestSnapshotLoadFailed(err)),
    }
}

fn ensure_fresh_with_mtime_check(
    snapshot: Snapshot,
    root: &Path,
    roots: &[PathBuf],
    global: &GlobalOptions,
) -> Result<Snapshot, ContextLoadError> {
    let current_head = current_git_head(root).unwrap_or_default();
    let snapshot_head = snapshot.metadata.git_commit.clone().unwrap_or_default();
    let branch_stale = !(current_head.is_empty()
        || snapshot_head.is_empty()
        || current_head.starts_with(&snapshot_head)
        || snapshot_head.starts_with(&current_head));

    if branch_stale {
        if global.fail_stale {
            return Err(ContextLoadError::StaleInCiMode {
                current: current_head,
                snapshot: snapshot_head,
            });
        }
        if global.no_scan {
            eprintln!(
                "[loct][context] snapshot is stale (git HEAD {} vs snapshot {}), but --no-scan in effect - using stale data. Surfaces may be inaccurate.",
                short_sha(&current_head),
                short_sha(&snapshot_head)
            );
            return Ok(snapshot);
        }

        eprintln!(
            "[loct][context] snapshot stale (git HEAD {} -> {}), rescanning... (use --no-scan to keep stale)",
            short_sha(&snapshot_head),
            short_sha(&current_head)
        );
        run_context_scan(root, roots, true, ScanKind::Full, global.force_non_git)?;
        return load_snapshot_after_scan(root);
    }

    let changed_files = snapshot.files_changed_since_scan(root, 100).unwrap_or(0);
    let max_age = loct_cache_max_age();
    let age_stale = snapshot.is_older_than(max_age);
    if changed_files == 0 && !age_stale {
        return Ok(snapshot);
    }

    if global.fail_stale {
        return Err(ContextLoadError::StaleInCiMode {
            current: current_head,
            snapshot: snapshot_head,
        });
    }
    if global.no_scan {
        if changed_files > 0 {
            eprintln!(
                "[loct][context] snapshot has {changed_files} file(s) changed since last scan, but --no-scan in effect - using stale data. Surfaces may be inaccurate."
            );
        } else {
            eprintln!(
                "[loct][context] snapshot is older than {}, but --no-scan in effect - using stale data. Surfaces may be inaccurate.",
                format_duration(max_age)
            );
        }
        return Ok(snapshot);
    }

    if changed_files > 0 {
        eprintln!(
            "[loct][context] {changed_files} file(s) changed since last scan, incremental rescan..."
        );
    } else {
        eprintln!(
            "[loct][context] snapshot older than {}, incremental rescan...",
            format_duration(max_age)
        );
    }
    run_context_scan(
        root,
        roots,
        false,
        ScanKind::Incremental,
        global.force_non_git,
    )?;
    load_snapshot_after_scan(root)
}

fn load_snapshot_after_scan(root: &Path) -> Result<Snapshot, ContextLoadError> {
    let snapshot = match Snapshot::load(root) {
        Ok(snapshot) => snapshot,
        Err(err) if err.kind() == io::ErrorKind::NotFound => match load_latest_snapshot(root) {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => return Err(ContextLoadError::PostScanLoadFailed(err)),
            Err(latest_err) => return Err(ContextLoadError::PostScanLoadFailed(latest_err)),
        },
        Err(err) => return Err(ContextLoadError::PostScanLoadFailed(err)),
    };
    verify_post_scan_identity(&snapshot, root)?;
    Ok(snapshot)
}

/// Fail-closed identity gate for every snapshot loaded right after a scan
/// (`--fresh` included). The `load_latest_snapshot` fallback above can hand
/// back an artifact from a different commit than the tree we just scanned —
/// exactly the audit wrong-commit class. When both identities are known and
/// disagree, refuse instead of emitting fresh authority.
///
/// Non-git roots and snapshots without a recorded commit pass through: there
/// is no identity to contradict, and the receipt already reports `unknown`.
fn verify_post_scan_identity(snapshot: &Snapshot, root: &Path) -> Result<(), ContextLoadError> {
    let live_head = current_git_head(root).unwrap_or_default();
    let snapshot_commit = snapshot.metadata.git_commit.clone().unwrap_or_default();
    if live_head.is_empty() || snapshot_commit.is_empty() {
        return Ok(());
    }
    if !crate::receipt::commits_identity_compatible(&live_head, &snapshot_commit) {
        return Err(ContextLoadError::PostScanIdentityMismatch {
            live_head,
            snapshot_commit,
            root: root.to_path_buf(),
        });
    }
    Ok(())
}

fn run_context_scan(
    root: &Path,
    roots: &[PathBuf],
    full_scan: bool,
    kind: ScanKind,
    force_non_git: bool,
) -> Result<(), ContextLoadError> {
    let scan_roots = if roots.is_empty() {
        vec![root.to_path_buf()]
    } else {
        roots.to_vec()
    };
    // Unified file universe: internal rescans must cover the same file set as
    // the initial `loct` scan (detect-applied extensions + .loctignore).
    let mut parsed =
        crate::snapshot::unified_scan_args(scan_roots.first().map_or(root, |r| r.as_path()), false);
    parsed.root_list = scan_roots.clone();
    parsed.full_scan = full_scan;
    parsed.output = OutputMode::Human;
    parsed.force_non_git = force_non_git;

    let scan_start = Instant::now();
    crate::snapshot::run_init_with_options(&scan_roots, &parsed, true).map_err(
        |err| match kind {
            ScanKind::Incremental => ContextLoadError::IncrementalRescanFailed(err),
            ScanKind::Initial | ScanKind::Full | ScanKind::Fresh => {
                ContextLoadError::ScanFailed(err)
            }
        },
    )?;
    let scan_duration = scan_start.elapsed();
    eprintln!(
        "[loct][context] scan completed in {:.2}s",
        scan_duration.as_secs_f64()
    );
    if scan_duration > Duration::from_secs(30) {
        eprintln!(
            "[loct][context] scan exceeded 30s; consider `loct scan --watch` during long development sessions"
        );
    }
    Ok(())
}

fn load_latest_snapshot(root: &Path) -> io::Result<Option<Snapshot>> {
    let path = match Snapshot::find_latest_snapshot_in(root) {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str(&content).map(Some).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to parse latest snapshot: {err}"),
        )
    })
}

fn loct_cache_max_age() -> Duration {
    std::env::var("LOCT_CACHE_MAX_AGE")
        .ok()
        .and_then(|raw| parse_duration_env(&raw))
        .unwrap_or_else(|| Duration::from_secs(24 * 60 * 60))
}

fn parse_duration_env(raw: &str) -> Option<Duration> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (number, multiplier) = if let Some(value) = raw.strip_suffix('h') {
        (value, 60 * 60)
    } else if let Some(value) = raw.strip_suffix('m') {
        (value, 60)
    } else if let Some(value) = raw.strip_suffix('s') {
        (value, 1)
    } else {
        (raw, 60 * 60)
    };
    number
        .parse::<u64>()
        .ok()
        .map(|value| Duration::from_secs(value.saturating_mul(multiplier)))
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs.is_multiple_of(60 * 60) {
        format!("{}h", secs / (60 * 60))
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn short_sha(s: &str) -> &str {
    if s.len() > 8 { &s[..8] } else { s }
}

/// Cut 4 T1 — fill the structural slice from snapshot facts.
///
/// Reuses [`HolographicSlice::from_path`] for the 3-layer (target / deps /
/// consumers) graph, then enriches with per-symbol exports, per-import
/// resolution, consumer classification, and snapshot-declared entrypoints.
///
/// Returns the default empty slice when:
/// - `opts.file` is None (T4 will add `--changed` and `--task` selection),
/// - the target file isn't found in the snapshot.
///
/// Every populated fact carries `AuthorityLabel::RepoVerified` because the
/// snapshot is the source of truth for repo state.
pub fn compose_structural_slice(opts: &ContextOptions, snapshot: &Snapshot) -> StructuralSlice {
    let Some(file_path) = &opts.file else {
        return StructuralSlice::default();
    };

    let target_str_owned = file_path.to_string_lossy().into_owned();
    let target_str = target_str_owned.trim_start_matches("./");
    let normalized_target = snapshot.normalize_path(target_str);

    // Reuse the slicer's BFS — Cut 4 doesn't reimplement graph traversal.
    let slice_config = SliceConfig {
        include_consumers: true,
        max_depth: 2,
    };
    let slice = match HolographicSlice::from_path(snapshot, &normalized_target, &slice_config) {
        Some(s) => s,
        None => return StructuralSlice::default(),
    };

    let target_path = slice.target.clone();
    let target_analysis = snapshot.files.iter().find(|f| f.path == target_path);

    // Files in scope (target + deps + consumers, role-tagged).
    let mut files: Vec<StructuralFile> =
        Vec::with_capacity(slice.core.len() + slice.deps.len() + slice.consumers.len());
    for f in &slice.core {
        files.push(StructuralFile {
            path: f.path.clone(),
            role: StructuralRole::Target,
            depth: 0,
            language: f.language.clone(),
            loc: f.loc,
            authority: AuthorityLabel::RepoVerified,
        });
    }
    for f in &slice.deps {
        files.push(StructuralFile {
            path: f.path.clone(),
            role: StructuralRole::Dependency,
            depth: f.depth,
            language: f.language.clone(),
            loc: f.loc,
            authority: AuthorityLabel::RepoVerified,
        });
    }
    for f in &slice.consumers {
        files.push(StructuralFile {
            path: f.path.clone(),
            role: StructuralRole::Consumer,
            depth: f.depth,
            language: f.language.clone(),
            loc: f.loc,
            authority: AuthorityLabel::RepoVerified,
        });
    }

    // Symbols exported by the target file.
    let mut symbols: Vec<StructuralSymbol> = Vec::new();
    if let Some(target) = target_analysis {
        for exp in &target.exports {
            symbols.push(StructuralSymbol {
                name: exp.name.clone(),
                kind: exp.kind.clone(),
                export_type: exp.export_type.clone(),
                file: target.path.clone(),
                line: exp.line,
                authority: AuthorityLabel::RepoVerified,
            });
        }
    }

    // Imports declared by the target file (resolved + unresolved).
    let mut imports: Vec<StructuralImport> = Vec::new();
    if let Some(target) = target_analysis {
        for imp in &target.imports {
            imports.push(StructuralImport {
                file: target.path.clone(),
                source: imp.source.clone(),
                source_raw: imp.source_raw.clone(),
                kind: import_kind_label(&imp.kind).to_string(),
                resolution: import_resolution_label(&imp.resolution).to_string(),
                resolved_path: imp.resolved_path.clone(),
                line: imp.line,
                symbols: imp
                    .symbols
                    .iter()
                    .map(|s| s.alias.clone().unwrap_or_else(|| s.name.clone()))
                    .collect(),
                is_bare: imp.is_bare,
                authority: AuthorityLabel::RepoVerified,
            });
        }
    }

    // Consumers — classify direct/reexport/transitive + extract imports_used.
    let target_export_names: HashSet<&str> = target_analysis
        .map(|t| t.exports.iter().map(|e| e.name.as_str()).collect())
        .unwrap_or_default();
    let target_stripped = strip_path_extension(&target_path);

    let mut consumers: Vec<StructuralConsumer> = Vec::new();
    for c in &slice.consumers {
        let edge_to_target = snapshot.edges.iter().find(|e| {
            e.from == c.path
                && (e.to == target_path || strip_path_extension(&e.to) == target_stripped)
        });
        let import_kind = match edge_to_target {
            Some(edge) if edge.label == "reexport" => ConsumerKind::Reexport,
            Some(_) => ConsumerKind::Direct,
            None => ConsumerKind::Transitive,
        };

        let mut imports_used: Vec<String> = Vec::new();
        if let Some(consumer_analysis) = snapshot.files.iter().find(|f| f.path == c.path) {
            for imp in &consumer_analysis.imports {
                let resolves_to_target = imp
                    .resolved_path
                    .as_deref()
                    .map(|rp| rp == target_path || strip_path_extension(rp) == target_stripped)
                    .unwrap_or(false)
                    || imp.source == target_path
                    || strip_path_extension(&imp.source) == target_stripped;

                let candidate_names: Vec<String> = if resolves_to_target {
                    imp.symbols.iter().map(|s| s.name.clone()).collect()
                } else if !target_export_names.is_empty() {
                    // Re-export / barrel case: keep symbols whose name overlaps target's exports.
                    imp.symbols
                        .iter()
                        .filter(|s| target_export_names.contains(s.name.as_str()))
                        .map(|s| s.name.clone())
                        .collect()
                } else {
                    Vec::new()
                };

                for name in candidate_names {
                    if !imports_used.contains(&name) {
                        imports_used.push(name);
                    }
                }
            }
        }

        consumers.push(StructuralConsumer {
            file: c.path.clone(),
            import_kind,
            imports_used,
            authority: AuthorityLabel::RepoVerified,
        });
    }

    // Entrypoints — surface those whose path is in the structural scope.
    let scope_paths: HashSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let entrypoints: Vec<StructuralEntrypoint> = snapshot
        .metadata
        .entrypoints
        .iter()
        .filter(|e| scope_paths.contains(e.path.as_str()))
        .map(|e| StructuralEntrypoint {
            path: e.path.clone(),
            kinds: e.kinds.clone(),
            authority: AuthorityLabel::RepoVerified,
        })
        .collect();

    StructuralSlice {
        files,
        symbols,
        imports,
        consumers,
        entrypoints,
    }
}

/// Cut 4 T2 — fill the runtime slice from semantic facts + Tauri bridges.
///
/// Source layers:
/// - [`SemanticFacts`] — Layer 3 idioms, dispatch edges, reachability, env contracts.
/// - `snapshot.command_bridges` / `snapshot.event_bridges` — Layer 1 Tauri bridges.
/// - `FileAnalysis::routes` / `pytest_fixtures` / `entry_points` — per-file metadata.
/// - Idiom tags with `Classifier::Custom("rust:trait_impl_method")` etc. —
///   surfaced as framework hints so an agent gets cross-runtime reachability.
///
/// Scope: only filled when `opts.file` is set; otherwise the empty slice is
/// returned. T4 will widen this for `--changed` / `--task` modes.
///
/// Authority labels:
/// - `RepoVerified` for facts sourced from real code artefacts (Tauri bridges,
///   per-file route metadata, `DirectImport` reachability).
/// - `LoctreeDerived` for analyzer-computed facts (dispatch edges, env contracts,
///   most reachability proofs, idioms from the embedded catalog).
/// - `SemanticGuess` for inferred idiom roles (`InferredFromCode` source, or
///   `IdiomRuntimeRole` reachability).
/// - `StaleOrUnknown` for `ReachReason::Unknown`.
pub fn compose_runtime_slice(opts: &ContextOptions, snapshot: &Snapshot) -> RuntimeSlice {
    let Some(file_path) = &opts.file else {
        return RuntimeSlice::default();
    };

    let target = normalize_target_path(snapshot, file_path);
    let mut slice = RuntimeSlice::default();

    if let Some(facts) = snapshot.semantic_facts.as_ref() {
        collect_idiom_tags(facts, &target, &mut slice);
        collect_dispatch_edges(facts, &target, &mut slice);
        collect_reachability(facts, &target, &mut slice);
        collect_env_contracts(facts, &target, &mut slice);
    }

    collect_tauri_commands(&snapshot.command_bridges, &target, &mut slice);
    collect_tauri_events(&snapshot.event_bridges, &target, &mut slice);
    collect_framework_hints(snapshot, &target, &mut slice);
    enrich_http_dispatch_metadata(snapshot, &mut slice);
    cap_make_runtime_targets(&mut slice);

    slice
}

/// W5-A: widen only the bare repository card with executable ownership and
/// named environment reads. File/task/scoped packs retain their local slice.
/// The inventory never reads environment values and records the classes it
/// knows how to inspect so absence cannot be presented as exhaustive truth.
fn enrich_repo_runtime_inventory(
    project_root: &Path,
    snapshot: &Snapshot,
    slice: &mut RuntimeSlice,
) {
    if let Some(facts) = snapshot.semantic_facts.as_ref() {
        for contract in &facts.env_contracts {
            slice
                .env_contracts
                .push(runtime_env_contract_from_semantic(contract));
        }
    }

    let source_env = collect_source_env_reads(project_root, snapshot);
    for read in &source_env.reads {
        slice.env_contracts.push(RuntimeEnvContract {
            name: read.name.clone(),
            used_in_files: vec![read.file.clone()],
            required_for: Vec::new(),
            occurrences: vec![RuntimeEnvOccurrence {
                file: read.file.clone(),
                line: read.line,
                access_kind: read.access_kind.clone(),
                default: None,
                required: false,
            }],
            required: false,
            authority: AuthorityLabel::RepoVerified,
        });
    }

    for entrypoint in &snapshot.metadata.entrypoints {
        slice.framework_hints.push(RuntimeFrameworkHint {
            kind: "executable_owner".to_string(),
            symbol: if entrypoint.kinds.is_empty() {
                "code_entrypoint".to_string()
            } else {
                entrypoint.kinds.join("+")
            },
            file: entrypoint.path.clone(),
            line: None,
            detail: Some("snapshot entrypoint inventory".to_string()),
            authority: AuthorityLabel::RepoVerified,
        });
    }

    for file in &snapshot.files {
        collect_swift_application_hints(snapshot, file, slice);
        if file.path.ends_with("Cargo.toml") {
            collect_cargo_owner_hints(project_root, &file.path, slice);
        }
    }

    if let Some(facts) = snapshot.semantic_facts.as_ref() {
        for (symbol, tags) in &facts.idiom_tags {
            let Some((file, target)) = symbol.split_once("::") else {
                continue;
            };
            if !file.ends_with("Makefile") && !file.ends_with("makefile") {
                continue;
            }
            if tags.iter().any(|tag| {
                matches!(
                    tag.classifier,
                    Classifier::PrimaryEntrypoint
                        | Classifier::UserFacingEntrypoint
                        | Classifier::PublicEntrypoint
                )
            }) {
                slice.framework_hints.push(RuntimeFrameworkHint {
                    kind: "make_target_owner".to_string(),
                    symbol: target.to_string(),
                    file: file.to_string(),
                    line: None,
                    detail: Some("executable Make target".to_string()),
                    authority: AuthorityLabel::LoctreeDerived,
                });
            }
        }
    }

    slice.inventory_coverage = Some(RuntimeInventoryCoverage {
        owner_classes: vec![
            "cargo_bin".to_string(),
            "cargo_default_run".to_string(),
            "cargo_library_crate_type".to_string(),
            "code_entrypoint".to_string(),
            "make_public_target".to_string(),
            "swiftpm_executable_target".to_string(),
            "swiftui_main".to_string(),
        ],
        env_read_classes: source_env.classes,
        source_files_scanned: source_env.files_scanned,
    });
}

fn collect_cargo_owner_hints(project_root: &Path, manifest_path: &str, slice: &mut RuntimeSlice) {
    let Ok(raw) = std::fs::read_to_string(project_root.join(manifest_path)) else {
        return;
    };
    let Ok(manifest) = toml::from_str::<toml::Value>(&raw) else {
        return;
    };

    if let Some(default_run) = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("default-run"))
        .and_then(toml::Value::as_str)
    {
        push_owner_hint(
            slice,
            "cargo_default_run",
            default_run,
            manifest_path,
            "Cargo package default-run",
        );
    }
    if let Some(bins) = manifest.get("bin").and_then(toml::Value::as_array) {
        for bin in bins.iter().filter_map(toml::Value::as_table) {
            let Some(name) = bin.get("name").and_then(toml::Value::as_str) else {
                continue;
            };
            let detail = bin
                .get("path")
                .and_then(toml::Value::as_str)
                .map(|path| format!("Cargo [[bin]] path `{path}`"))
                .unwrap_or_else(|| "Cargo [[bin]] target".to_string());
            push_owner_hint(slice, "cargo_bin", name, manifest_path, &detail);
        }
    }
    if let Some(crate_types) = manifest
        .get("lib")
        .and_then(toml::Value::as_table)
        .and_then(|lib| lib.get("crate-type"))
        .and_then(toml::Value::as_array)
    {
        for crate_type in crate_types.iter().filter_map(toml::Value::as_str) {
            push_owner_hint(
                slice,
                "cargo_library_crate_type",
                crate_type,
                manifest_path,
                "Cargo [lib] crate-type",
            );
        }
    }
}

fn push_owner_hint(slice: &mut RuntimeSlice, kind: &str, symbol: &str, file: &str, detail: &str) {
    slice.framework_hints.push(RuntimeFrameworkHint {
        kind: kind.to_string(),
        symbol: symbol.to_string(),
        file: file.to_string(),
        line: None,
        detail: Some(detail.to_string()),
        authority: AuthorityLabel::RepoVerified,
    });
}

fn cap_make_runtime_targets(slice: &mut RuntimeSlice) {
    retain_with_cap(&mut slice.idiom_tags, |tag| tag.name == ".PHONY");
    retain_with_cap(&mut slice.reachability, |reach| {
        reach.reason == "phony_make_target"
    });
}

fn retain_with_cap<T>(items: &mut Vec<T>, mut matches_cap: impl FnMut(&T) -> bool) {
    let mut retained = 0usize;
    items.retain(|item| {
        if !matches_cap(item) {
            return true;
        }
        retained += 1;
        retained <= MAKE_RUNTIME_TARGET_LIMIT
    });
}

/// Lift HTTP method/path/framework metadata from `RouteInfo` onto the matching
/// `http_route` dispatch edge. Layer 3 emits the edge with the handler symbol
/// and decorator line, Layer 1 owns the route detail — joining them here lets
/// agents plan a cURL/integration test without grepping the source file.
fn enrich_http_dispatch_metadata(snapshot: &Snapshot, slice: &mut RuntimeSlice) {
    for edge in slice.dispatch_edges.iter_mut() {
        if edge.dispatch_kind != "http_route" {
            continue;
        }
        let Some(file) = snapshot.files.iter().find(|f| f.path == edge.from_file) else {
            continue;
        };
        let route = file
            .routes
            .iter()
            .find(|r| r.name.as_deref() == Some(edge.handler_symbol.as_str()))
            .or_else(|| {
                file.routes
                    .iter()
                    .find(|r| (r.line as u32) == edge.from_line)
            });
        if let Some(route) = route {
            edge.framework = Some(route.framework.clone());
            if !route.method.is_empty() {
                edge.http_method = Some(route.method.clone());
            }
            edge.http_path = route.path.clone();
        }
    }
}

fn normalize_target_path(snapshot: &Snapshot, file_path: &Path) -> String {
    let raw = file_path.to_string_lossy().into_owned();
    let trimmed = raw.trim_start_matches("./");
    snapshot.normalize_path(trimmed)
}

fn symbol_in_scope(symbol_id: &str, target: &str) -> bool {
    match symbol_id.split_once("::") {
        Some((file, _)) => file == target,
        None => false,
    }
}

fn collect_idiom_tags(facts: &SemanticFacts, target: &str, slice: &mut RuntimeSlice) {
    for (symbol_id, tags) in &facts.idiom_tags {
        if !symbol_in_scope(symbol_id, target) {
            continue;
        }
        for tag in tags {
            slice.idiom_tags.push(RuntimeIdiomTag {
                symbol: symbol_id.clone(),
                name: tag.name.clone(),
                classifier: classifier_label(&tag.classifier),
                runtime_role: runtime_role_label(&tag.runtime_role).to_string(),
                source: tag_source_label(&tag.source).to_string(),
                reasoning: tag.reasoning.clone(),
                authority: idiom_tag_authority(tag),
            });
        }
    }
    slice
        .idiom_tags
        .sort_by(|a, b| a.symbol.cmp(&b.symbol).then_with(|| a.name.cmp(&b.name)));
}

fn collect_dispatch_edges(facts: &SemanticFacts, target: &str, slice: &mut RuntimeSlice) {
    for edge in &facts.dispatch_edges {
        if !dispatch_edge_in_scope(edge, target) {
            continue;
        }
        slice.dispatch_edges.push(RuntimeDispatchEdge {
            from_file: edge.from_file.clone(),
            from_line: edge.from_line,
            dispatch_kind: dispatch_kind_label(&edge.dispatch_kind).to_string(),
            handler_symbol: edge.handler_symbol.clone(),
            handler_file: edge.handler_file.clone(),
            framework: None,
            http_method: None,
            http_path: None,
            authority: AuthorityLabel::LoctreeDerived,
        });
    }
    slice.dispatch_edges.sort_by(|a, b| {
        a.from_file
            .cmp(&b.from_file)
            .then_with(|| a.from_line.cmp(&b.from_line))
            .then_with(|| a.handler_symbol.cmp(&b.handler_symbol))
    });
}

fn dispatch_edge_in_scope(edge: &DispatchEdge, target: &str) -> bool {
    if edge.from_file == target {
        return true;
    }
    matches!(edge.handler_file.as_deref(), Some(p) if p == target)
}

fn collect_reachability(facts: &SemanticFacts, target: &str, slice: &mut RuntimeSlice) {
    let mut seen: HashSet<String> = HashSet::new();

    let mut reached: Vec<&str> = facts
        .reachability
        .reached_symbols
        .iter()
        .map(String::as_str)
        .collect();
    reached.sort();
    for symbol_id in reached {
        push_reachability(facts, symbol_id, true, target, &mut seen, slice);
    }

    let mut unreached: Vec<&str> = facts
        .reachability
        .unreached_symbols
        .iter()
        .map(String::as_str)
        .collect();
    unreached.sort();
    for symbol_id in unreached {
        push_reachability(facts, symbol_id, false, target, &mut seen, slice);
    }
}

fn push_reachability(
    facts: &SemanticFacts,
    symbol_id: &str,
    reached: bool,
    target: &str,
    seen: &mut HashSet<String>,
    slice: &mut RuntimeSlice,
) {
    if !symbol_in_scope(symbol_id, target) {
        return;
    }
    if !seen.insert(symbol_id.to_string()) {
        return;
    }
    let (reason, authority) = match facts.reachability.reasons.get(symbol_id) {
        Some(r) => (reach_reason_label(r), reach_reason_authority(r)),
        None => ("unknown".to_string(), AuthorityLabel::StaleOrUnknown),
    };
    slice.reachability.push(RuntimeReachability {
        symbol: symbol_id.to_string(),
        reached,
        reason,
        authority,
    });
}

fn collect_env_contracts(facts: &SemanticFacts, target: &str, slice: &mut RuntimeSlice) {
    for contract in &facts.env_contracts {
        if !env_contract_in_scope(contract, target) {
            continue;
        }
        slice
            .env_contracts
            .push(runtime_env_contract_from_semantic(contract));
    }
    slice.env_contracts.sort_by(|a, b| a.name.cmp(&b.name));
}

fn runtime_env_contract_from_semantic(contract: &EnvContract) -> RuntimeEnvContract {
    let occurrences: Vec<RuntimeEnvOccurrence> = contract
        .occurrences
        .iter()
        .map(|o| RuntimeEnvOccurrence {
            file: o.file.clone(),
            line: o.line,
            access_kind: o.access_kind.clone(),
            default: o.default.clone(),
            required: o.required,
        })
        .collect();
    let required = occurrences.iter().any(|o| o.required);
    RuntimeEnvContract {
        name: contract.name.clone(),
        used_in_files: contract.used_in_files.clone(),
        required_for: contract.required_for.clone(),
        occurrences,
        required,
        authority: AuthorityLabel::LoctreeDerived,
    }
}

fn env_contract_in_scope(contract: &EnvContract, target: &str) -> bool {
    contract.used_in_files.iter().any(|f| f == target)
        || contract.occurrences.iter().any(|occ| occ.file == target)
}

fn collect_tauri_commands(bridges: &[CommandBridge], target: &str, slice: &mut RuntimeSlice) {
    for bridge in bridges {
        if !tauri_command_in_scope(bridge, target) {
            continue;
        }
        let (handler_file, handler_line) = match &bridge.backend_handler {
            Some((file, line)) => (Some(file.clone()), Some(*line)),
            None => (None, None),
        };
        slice.tauri_commands.push(RuntimeTauriCommand {
            name: bridge.name.clone(),
            handler_file,
            handler_line,
            invoke_site_count: bridge.frontend_calls.len(),
            has_handler: bridge.has_handler,
            is_called: bridge.is_called,
            authority: AuthorityLabel::RepoVerified,
        });
    }
    slice.tauri_commands.sort_by(|a, b| a.name.cmp(&b.name));
}

fn tauri_command_in_scope(bridge: &CommandBridge, target: &str) -> bool {
    if let Some((file, _)) = &bridge.backend_handler
        && file == target
    {
        return true;
    }
    bridge.frontend_calls.iter().any(|(file, _)| file == target)
}

fn collect_tauri_events(bridges: &[EventBridge], target: &str, slice: &mut RuntimeSlice) {
    for bridge in bridges {
        if !tauri_event_in_scope(bridge, target) {
            continue;
        }
        slice.tauri_events.push(RuntimeTauriEvent {
            name: bridge.name.clone(),
            emit_count: bridge.emits.len(),
            listen_count: bridge.listens.len(),
            is_fe_sync: bridge.is_fe_sync,
            same_file_sync: bridge.same_file_sync,
            authority: AuthorityLabel::RepoVerified,
        });
    }
    slice.tauri_events.sort_by(|a, b| a.name.cmp(&b.name));
}

fn tauri_event_in_scope(bridge: &EventBridge, target: &str) -> bool {
    bridge.emits.iter().any(|(f, _, _)| f == target)
        || bridge.listens.iter().any(|(f, _)| f == target)
}

/// Surface framework-specific reachability hints from every layer that owns
/// one — Layer 1 sensors (entrypoints, RouteInfo, pytest fixtures) and Layer 3
/// semantics (Custom-classifier idiom tags such as `rust:trait_impl_method`,
/// plus Python-file decorator dispatch like FastAPI / Flask / pytest /
/// Pydantic / Click / Celery).
fn collect_framework_hints(snapshot: &Snapshot, target: &str, slice: &mut RuntimeSlice) {
    if let Some(file) = snapshot.files.iter().find(|f| f.path == target) {
        for entrypoint in &file.entry_points {
            slice.framework_hints.push(RuntimeFrameworkHint {
                kind: "entrypoint".to_string(),
                symbol: entrypoint.clone(),
                file: file.path.clone(),
                line: None,
                detail: None,
                authority: AuthorityLabel::RepoVerified,
            });
        }
        for route in &file.routes {
            slice.framework_hints.push(RuntimeFrameworkHint {
                kind: format!("{}_route", route.framework),
                symbol: route
                    .name
                    .clone()
                    .unwrap_or_else(|| "<anonymous>".to_string()),
                file: file.path.clone(),
                line: Some(route.line as u32),
                detail: Some(route_detail(route)),
                authority: AuthorityLabel::RepoVerified,
            });
        }
        for fixture in &file.pytest_fixtures {
            slice.framework_hints.push(RuntimeFrameworkHint {
                kind: "pytest_fixture".to_string(),
                symbol: fixture.clone(),
                file: file.path.clone(),
                line: None,
                detail: None,
                authority: AuthorityLabel::RepoVerified,
            });
        }
        collect_swift_application_hints(snapshot, file, slice);
    }

    if let Some(facts) = snapshot.semantic_facts.as_ref() {
        // Custom-classifier idiom tags carry framework labels (e.g.
        // `rust:trait_impl_method`) that the closed `Classifier` enum cannot.
        for (symbol_id, tags) in &facts.idiom_tags {
            if !symbol_in_scope(symbol_id, target) {
                continue;
            }
            for tag in tags {
                if let Classifier::Custom(label) = &tag.classifier {
                    slice.framework_hints.push(RuntimeFrameworkHint {
                        kind: label.clone(),
                        symbol: symbol_id.clone(),
                        file: target.to_string(),
                        line: None,
                        detail: Some(tag.reasoning.clone()),
                        authority: AuthorityLabel::LoctreeDerived,
                    });
                }
            }
        }

        // Python decorator-based dispatch — Layer 3 now classifies recognised
        // decorators into `HttpRoute` / `CliCommand` / `EventHandler` /
        // `TaskTarget`. Those are already surfaced as `<framework>_route`,
        // `pytest_fixture`, etc. hints from the Layer 1 sensor pass above —
        // emitting `python_decorator` mirrors would just inflate the payload.
        //
        // Only fall back to `python_decorator` for `FunctionPointer` edges,
        // which represent unrecognised callback decorators (custom `@retry`,
        // `@cache`, etc.). And even then, suppress it when a more specific
        // framework hint already exists for the same `(file, line, symbol)`.
        for edge in &facts.dispatch_edges {
            if !dispatch_edge_in_scope(edge, target) {
                continue;
            }
            if !matches!(edge.dispatch_kind, DispatchKind::FunctionPointer) {
                continue;
            }
            if !is_python_path(&edge.from_file) {
                continue;
            }
            let already_has_specific_hint = slice.framework_hints.iter().any(|h| {
                h.file == edge.from_file
                    && h.line == Some(edge.from_line)
                    && h.symbol == edge.handler_symbol
                    && h.kind != "python_decorator"
            });
            if already_has_specific_hint {
                continue;
            }
            slice.framework_hints.push(RuntimeFrameworkHint {
                kind: "python_decorator".to_string(),
                symbol: edge.handler_symbol.clone(),
                file: edge.from_file.clone(),
                line: Some(edge.from_line),
                detail: edge.handler_file.clone(),
                authority: AuthorityLabel::LoctreeDerived,
            });
        }
    }

    slice.framework_hints.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.symbol.cmp(&b.symbol))
    });
}

/// Lift Swift application ownership into the runtime surface from facts the
/// snapshot already carries. `@main` is emitted as `swift-main`; a SwiftUI
/// import identifies the framework; and the enclosing `Package.swift` must
/// explicitly list the matching target in an executable product before we
/// claim SwiftPM ownership.
fn collect_swift_application_hints(
    snapshot: &Snapshot,
    file: &crate::types::FileAnalysis,
    slice: &mut RuntimeSlice,
) {
    if file.language != "swift" || !file.entry_points.iter().any(|entry| entry == "swift-main") {
        return;
    }

    if file
        .imports
        .iter()
        .any(|import| import.source == "SwiftUI" || import.source_raw == "SwiftUI")
    {
        slice.framework_hints.push(RuntimeFrameworkHint {
            kind: "swiftui_app".to_string(),
            symbol: "swift-main".to_string(),
            file: file.path.clone(),
            line: None,
            detail: Some("SwiftUI application entrypoint (`@main` + `import SwiftUI`)".to_string()),
            authority: AuthorityLabel::RepoVerified,
        });
    }

    let Some((target_name, manifest_path, line)) = swiftpm_executable_owner(snapshot, &file.path)
    else {
        return;
    };
    slice.framework_hints.push(RuntimeFrameworkHint {
        kind: "swiftpm_executable_target".to_string(),
        symbol: target_name,
        file: manifest_path.clone(),
        line: Some(line as u32),
        detail: Some(format!(
            "declares the SwiftPM executable product target that owns `{}`",
            file.path
        )),
        authority: AuthorityLabel::RepoVerified,
    });
}

fn swiftpm_executable_owner(
    snapshot: &Snapshot,
    entrypoint_path: &str,
) -> Option<(String, String, usize)> {
    for manifest in &snapshot.files {
        let Some(package_dir) = manifest.path.strip_suffix("Package.swift") else {
            continue;
        };
        if !manifest.imports.iter().any(|import| {
            import.source == "PackageDescription" || import.source_raw == "PackageDescription"
        }) {
            continue;
        }
        let sources_prefix = format!("{package_dir}Sources/");
        let Some(relative) = entrypoint_path.strip_prefix(&sources_prefix) else {
            continue;
        };
        let Some(target_name) = relative.split('/').next().map(str::trim) else {
            continue;
        };
        if target_name.is_empty() {
            continue;
        }
        let quoted_target = format!("\"{target_name}\"");
        if let Some(usage) = manifest.symbol_usages.iter().find(|usage| {
            usage.name == "executable"
                && usage.context.contains("targets:")
                && usage.context.contains(&quoted_target)
        }) {
            return Some((target_name.to_string(), manifest.path.clone(), usage.line));
        }
    }
    None
}

fn route_detail(route: &crate::types::RouteInfo) -> String {
    match (&route.method, &route.path) {
        (m, Some(p)) if !m.is_empty() => format!("{m} {p}"),
        (m, None) if !m.is_empty() => m.clone(),
        (_, Some(p)) => p.clone(),
        _ => String::new(),
    }
}

fn is_python_path(path: &str) -> bool {
    path.ends_with(".py") || path.ends_with(".pyi")
}

// ---------------------------------------------------------------------------
// Label helpers — one block so the JSON wire format is documented in one place.
// ---------------------------------------------------------------------------

fn classifier_label(c: &Classifier) -> String {
    match c {
        Classifier::HelpPrinter => "help_printer".to_string(),
        Classifier::ErrorExit => "error_exit".to_string(),
        Classifier::PrimaryEntrypoint => "primary_entrypoint".to_string(),
        Classifier::UserFacingEntrypoint => "user_facing_entrypoint".to_string(),
        Classifier::PublicEntrypoint => "public_entrypoint".to_string(),
        Classifier::LibraryHelper => "library_helper".to_string(),
        Classifier::Metadata => "metadata".to_string(),
        Classifier::EnvVar => "env_var".to_string(),
        Classifier::EnvContract => "env_contract".to_string(),
        Classifier::SourceLibraryApi => "source_library_api".to_string(),
        Classifier::DispatchHandler => "dispatch_handler".to_string(),
        Classifier::Custom(s) => s.clone(),
    }
}

fn runtime_role_label(role: &RuntimeRole) -> &'static str {
    match role {
        RuntimeRole::UserFacing => "user_facing",
        RuntimeRole::PrimaryEntrypoint => "primary_entrypoint",
        RuntimeRole::PublicEntrypoint => "public_entrypoint",
        RuntimeRole::LibraryHelper => "library_helper",
        RuntimeRole::EnvInput => "env_input",
        RuntimeRole::Metadata => "metadata",
        RuntimeRole::Internal => "internal",
    }
}

fn tag_source_label(s: &TagSource) -> &'static str {
    match s {
        TagSource::EmbeddedDefault => "embedded_default",
        TagSource::UserOverride => "user_override",
        TagSource::InferredFromCode => "inferred_from_code",
    }
}

fn dispatch_kind_label(k: &DispatchKind) -> &'static str {
    match k {
        DispatchKind::CaseStatement => "case_statement",
        DispatchKind::FunctionPointer => "function_pointer",
        DispatchKind::EvalString => "eval_string",
        DispatchKind::RecipeShellCall => "recipe_shell_call",
        DispatchKind::TauriInvoke => "tauri_invoke",
        DispatchKind::TauriEvent => "tauri_event",
        DispatchKind::HttpRoute => "http_route",
        DispatchKind::CliCommand => "cli_command",
        DispatchKind::EventHandler => "event_handler",
        DispatchKind::TaskTarget => "task_target",
    }
}

fn reach_reason_label(r: &ReachReason) -> String {
    match r {
        ReachReason::DirectImport => "direct_import".to_string(),
        ReachReason::DispatchHandler {
            from_symbol,
            dispatch_kind,
        } => format!(
            "dispatch_handler:{}:{from_symbol}",
            dispatch_kind_label(dispatch_kind)
        ),
        ReachReason::SourceInclude { from_file } => format!("source_include:{from_file}"),
        ReachReason::PhonyMakeTarget => "phony_make_target".to_string(),
        ReachReason::RecipeShellCall { recipe_owner } => {
            format!("recipe_shell_call:{recipe_owner}")
        }
        ReachReason::IdiomRuntimeRole(role) => {
            format!("idiom_runtime_role:{}", runtime_role_label(role))
        }
        ReachReason::Unknown => "unknown".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Authority assignment — encodes "how trustworthy is this fact?".
// ---------------------------------------------------------------------------

/// Embedded-default and user-override idioms are deterministic catalog lookups
/// — those claims are fully derived from loctree's own ground truth. Idioms
/// inferred from code are best-guess heuristics; surface the difference so
/// downstream agents can weigh them accordingly.
fn idiom_tag_authority(tag: &IdiomTag) -> AuthorityLabel {
    match tag.source {
        TagSource::EmbeddedDefault | TagSource::UserOverride => AuthorityLabel::LoctreeDerived,
        TagSource::InferredFromCode => AuthorityLabel::SemanticGuess,
    }
}

/// `DirectImport` survives the entire static-analysis chain — that's the
/// strongest reachability proof we can produce without runtime instrumentation,
/// so it earns `RepoVerified`. Idiom-runtime-role reachability is purely
/// inferential, so it drops to `SemanticGuess`. `Unknown` reasons are stale.
fn reach_reason_authority(reason: &ReachReason) -> AuthorityLabel {
    match reason {
        ReachReason::DirectImport => AuthorityLabel::RepoVerified,
        ReachReason::DispatchHandler { .. }
        | ReachReason::SourceInclude { .. }
        | ReachReason::PhonyMakeTarget
        | ReachReason::RecipeShellCall { .. } => AuthorityLabel::LoctreeDerived,
        ReachReason::IdiomRuntimeRole(_) => AuthorityLabel::SemanticGuess,
        ReachReason::Unknown => AuthorityLabel::StaleOrUnknown,
    }
}

/// Append runtime-slice claim IDs to the top-level [`AuthoritySlice`].
///
/// Claim IDs are hierarchical (`runtime.idiom_tags.<symbol>::<name>` etc.) so
/// agents can grep one channel and find every fact bucketed by provenance.
fn merge_runtime_into_authority(runtime: &RuntimeSlice, authority: &mut AuthoritySlice) {
    for tag in &runtime.idiom_tags {
        push_authority_claim(
            &format!("runtime.idiom_tags.{}::{}", tag.symbol, tag.name),
            tag.authority,
            authority,
        );
    }
    for edge in &runtime.dispatch_edges {
        push_authority_claim(
            &format!(
                "runtime.dispatch_edges.{}:{}->{}",
                edge.from_file, edge.from_line, edge.handler_symbol
            ),
            edge.authority,
            authority,
        );
    }
    for reach in &runtime.reachability {
        let prefix = if reach.reached {
            "runtime.reachability.reached"
        } else {
            "runtime.reachability.unreached"
        };
        push_authority_claim(
            &format!("{prefix}.{}", reach.symbol),
            reach.authority,
            authority,
        );
    }
    for env in &runtime.env_contracts {
        push_authority_claim(
            &format!("runtime.env_contracts.{}", env.name),
            env.authority,
            authority,
        );
    }
    for cmd in &runtime.tauri_commands {
        push_authority_claim(
            &format!("runtime.tauri_commands.{}", cmd.name),
            cmd.authority,
            authority,
        );
    }
    for ev in &runtime.tauri_events {
        push_authority_claim(
            &format!("runtime.tauri_events.{}", ev.name),
            ev.authority,
            authority,
        );
    }
    for hint in &runtime.framework_hints {
        push_authority_claim(
            &format!("runtime.framework_hints.{}.{}", hint.kind, hint.symbol),
            hint.authority,
            authority,
        );
    }
}

fn merge_risk_action_into_authority(
    risk: &RiskSlice,
    action: &ActionSlice,
    authority: &mut AuthoritySlice,
) {
    for hotspot in &risk.hotspots {
        push_authority_claim(
            &format!("risk.hotspots.{}", hotspot.file),
            hotspot.authority,
            authority,
        );
    }
    for fan_in in &risk.high_fan_in {
        push_authority_claim(
            &format!("risk.high_fan_in.{}", fan_in.file),
            fan_in.authority,
            authority,
        );
    }
    push_authority_claim("risk.cache_scope", risk.cache_scope_authority, authority);
    for test in &action.likely_tests {
        push_authority_claim(
            &format!("action.likely_tests.{test}"),
            AuthorityLabel::LoctreeDerived,
            authority,
        );
    }
    for command in &action.next_safe_commands {
        let label = action_authority_for(command, &action.next_safe_command_authorities)
            .unwrap_or(AuthorityLabel::LoctreeDerived);
        push_authority_claim(
            &format!("action.next_safe_commands.{command}"),
            label,
            authority,
        );
    }
    if action.verification_gates.is_empty() {
        push_authority_claim(
            "action.verification_gates",
            AuthorityLabel::StaleOrUnknown,
            authority,
        );
    }
    for gate in &action.verification_gates {
        let label = action_authority_for(gate, &action.verification_gate_authorities)
            .unwrap_or(AuthorityLabel::SemanticGuess);
        push_authority_claim(
            &format!("action.verification_gates.{gate}"),
            label,
            authority,
        );
    }
}

fn action_authority_for(item: &str, claims: &[ActionAuthorityClaim]) -> Option<AuthorityLabel> {
    claims
        .iter()
        .find(|claim| claim.item == item)
        .map(|claim| claim.authority)
}

fn push_authority_claim(claim: &str, label: AuthorityLabel, authority: &mut AuthoritySlice) {
    let bucket = match label {
        AuthorityLabel::RepoVerified => &mut authority.repo_verified,
        AuthorityLabel::LoctreeDerived => &mut authority.loctree_derived,
        AuthorityLabel::AicxOperator => &mut authority.aicx_operator,
        AuthorityLabel::AicxAgent => &mut authority.aicx_agent,
        AuthorityLabel::AicxFailure => &mut authority.aicx_failure,
        AuthorityLabel::SemanticGuess => &mut authority.semantic_guess,
        AuthorityLabel::StaleOrUnknown => &mut authority.stale_or_unknown,
    };
    bucket.push(claim.to_string());
}

const STRIPPABLE_EXTENSIONS: &[&str] = &[
    ".tsx", ".ts", ".jsx", ".js", ".mjs", ".cjs", ".rs", ".py", ".css", ".scss", ".sass",
];

fn strip_path_extension(path: &str) -> &str {
    for ext in STRIPPABLE_EXTENSIONS {
        if let Some(stripped) = path.strip_suffix(ext) {
            return stripped;
        }
    }
    path
}

fn import_kind_label(kind: &ImportKind) -> &'static str {
    match kind {
        ImportKind::Static => "static",
        ImportKind::Type => "type",
        ImportKind::SideEffect => "side_effect",
        ImportKind::Dynamic => "dynamic",
    }
}

fn import_resolution_label(res: &ImportResolutionKind) -> &'static str {
    match res {
        ImportResolutionKind::Local => "local",
        ImportResolutionKind::Stdlib => "stdlib",
        ImportResolutionKind::Dynamic => "dynamic",
        ImportResolutionKind::Unknown => "unknown",
    }
}

fn project_identity(opts: &ContextOptions) -> ProjectIdentity {
    let roots = context_roots(opts);
    let root = resolve_snapshot_root(&roots);
    let canonical_root = root.canonicalize().unwrap_or(root);
    let git = Snapshot::git_context_for(&canonical_root);

    ProjectIdentity {
        canonical_root: Some(canonical_root.display().to_string()),
        branch: git.branch,
        commit: git.commit,
        snapshot_id: git.scan_id,
    }
}

fn context_roots(opts: &ContextOptions) -> Vec<PathBuf> {
    if let Some(project) = &opts.project {
        return vec![project.clone()];
    }

    if let Some(file) = &opts.file {
        return vec![file_root(file)];
    }

    vec![PathBuf::from(".")]
}

fn file_root(file: &Path) -> PathBuf {
    if file.is_dir() {
        return file.to_path_buf();
    }
    file.parent()
        .map(Path::to_path_buf)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Cut 4 T3 — fill the risk slice from snapshot and git facts.
///
/// Risk facts are intentionally compact: import fan-in identifies blast-radius
/// hubs, while cache scope reports whether the loaded snapshot matches current
/// git reality. Import fan-in is Loctree-derived; cache scope is repo-verified
/// because it comes from git and snapshot metadata comparisons.
pub fn compose_risk_slice(opts: &ContextOptions, snapshot: &Snapshot) -> RiskSlice {
    const HOTSPOT_LIMIT: usize = 5;
    const HIGH_FAN_IN_THRESHOLD: usize = 10;

    let snapshot_root = context_snapshot_root(opts);
    let current_head = current_git_head(&snapshot_root);
    let git_dirty = git_worktree_dirty(&snapshot_root);
    let git_unknown = git_dirty.is_none();
    let stale_snapshot = snapshot_commit_is_stale(snapshot, current_head.as_deref());
    let dirty_worktree = git_dirty.unwrap_or(false);
    let cache_scope = cache_scope_for(
        snapshot,
        &snapshot_root,
        stale_snapshot,
        dirty_worktree,
        git_unknown,
    );
    let importer_counts = importer_counts_direct(snapshot);

    let mut scoped_counts: Vec<(String, usize)> = risk_scope_files(opts, snapshot)
        .into_iter()
        .map(|file| {
            let importers = importer_counts.get(&file).copied().unwrap_or(0);
            (file, importers)
        })
        .collect();
    scoped_counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let hotspots = scoped_counts
        .iter()
        .filter(|(_, importers)| *importers > 0)
        .take(HOTSPOT_LIMIT)
        .map(|(file, importers)| HotspotFile {
            file: file.clone(),
            importers: *importers,
            authority: AuthorityLabel::LoctreeDerived,
        })
        .collect();

    let high_fan_in = scoped_counts
        .iter()
        .filter(|(_, importers)| *importers >= HIGH_FAN_IN_THRESHOLD)
        .map(|(file, importers)| HighFanInFile {
            file: file.clone(),
            importers: *importers,
            threshold: HIGH_FAN_IN_THRESHOLD,
            authority: AuthorityLabel::LoctreeDerived,
        })
        .collect();

    // Successful zero-file corpus is not `missing_snapshot` and not a normal
    // populated `fresh` tree — consumers need an explicit empty state
    // (loctree-fail 2026-08-11 empty-directory scan).
    let empty_corpus = snapshot.files.is_empty();

    RiskSlice {
        hotspots,
        high_fan_in,
        snapshot_health: Some(
            snapshot_health_label(stale_snapshot, dirty_worktree, empty_corpus, git_unknown)
                .to_string(),
        ),
        cache_scope: cache_scope.clone(),
        cache_scope_authority: cache_scope_authority(&cache_scope),
        stale_snapshot,
        dirty_worktree,
        git_unknown,
    }
}

/// Map a [`RiskCacheScope`] to the authority label the agent should trust.
///
/// `Unknown` means we genuinely could not determine snapshot/git alignment
/// (no `.loctree/` root resolved, git probe failed). Reporting that with a
/// `RepoVerified` label is a semantic contradiction — the agent is told
/// "this fact comes from a git+snapshot comparison" while the system
/// actually says "I never ran the comparison". Plan L02 / Finding #5 fix:
/// `Unknown` maps to `StaleOrUnknown`; every other variant did do the
/// comparison and is repo-verified.
fn cache_scope_authority(scope: &RiskCacheScope) -> AuthorityLabel {
    match scope {
        RiskCacheScope::Unknown => AuthorityLabel::StaleOrUnknown,
        RiskCacheScope::Clean
        | RiskCacheScope::DirtyWorktree
        | RiskCacheScope::StaleSnapshot
        | RiskCacheScope::MissingSnapshot
        | RiskCacheScope::Scoped(_) => AuthorityLabel::RepoVerified,
    }
}

pub fn compute_power_path(
    opts: &ContextOptions,
    structural: &StructuralSlice,
    _runtime: &RuntimeSlice,
    risk: &RiskSlice,
    _snapshot: &Snapshot,
) -> Vec<SuggestedCommand> {
    let mut path = Vec::new();

    if let Some(file_str) = opts
        .file
        .as_ref()
        .and_then(|path| non_empty_context_path(path.as_path()))
    {
        // 1. Slice is always recommended for file-focused tasks
        path.push(SuggestedCommand {
            command: format!("loct slice {file_str}"),
            reason: "Slice this file to inspect its direct dependencies and immediate consumers before editing.".to_string(),
        });

        // 2. Impact is relevant if there are consumers
        let has_consumers = !structural.consumers.is_empty()
            || risk.hotspots.iter().any(|h| h.file == file_str)
            || risk.high_fan_in.iter().any(|h| h.file == file_str);
        if has_consumers {
            path.push(SuggestedCommand {
                command: format!("loct impact {file_str}"),
                reason: "Assess the transitive blast radius and downstream consumers of this file before modifying.".to_string(),
            });
        }

        // 3. Body for named symbols
        let mut target_sym = None;
        if let Some(sym) = structural.symbols.first() {
            target_sym = Some(sym.name.clone());
            path.push(SuggestedCommand {
                command: format!("loct body {}", sym.name),
                reason: format!("View the definition body of symbol '{}' to understand its implementation structure.", sym.name),
            });
        }

        // 4. occurrences / find --literal for exact queries
        if let Some(ref sym) = target_sym {
            path.push(SuggestedCommand {
                command: format!("loct find --literal {sym}"),
                reason: format!("Execute a literal exact-identifier scan for '{}' to find exact matches across the codebase.", sym),
            });
            path.push(SuggestedCommand {
                command: format!("loct occurrences {sym}"),
                reason: format!("Perform a literal boundary-sensitive occurrences scan for '{}' to locate all references.", sym),
            });
        } else if let Some(ref task) = opts.task {
            // Extract the first word from task as a fallback literal search term
            let words: Vec<&str> = task.split_whitespace().collect();
            if let Some(word) = words.first() {
                let clean_word = word.trim_matches(|c: char| !c.is_alphanumeric());
                if !clean_word.is_empty() {
                    path.push(SuggestedCommand {
                        command: format!("loct find --literal {clean_word}"),
                        reason: format!(
                            "Perform a literal exact-identifier scan for task keyword '{}'.",
                            clean_word
                        ),
                    });
                }
            }
        }

        // 5. Follow for structural signal
        path.push(SuggestedCommand {
            command: "loct follow".to_string(),
            reason: "Pursue unified structural signals (dead exports, import cycles, twins) to ensure health.".to_string(),
        });
    } else {
        // Not file-focused (bare or task-only context)
        if let Some(hot) = risk.hotspots.first() {
            path.push(SuggestedCommand {
                command: format!("loct slice {}", hot.file),
                reason: "Slice the primary hotspot file to analyze its central role in the dependency graph.".to_string(),
            });
            path.push(SuggestedCommand {
                command: format!("loct impact {}", hot.file),
                reason: "Check the downstream impact of changing the primary workspace hotspot."
                    .to_string(),
            });
        }
        path.push(SuggestedCommand {
            command: "loct follow".to_string(),
            reason: "Trace structural signals across the codebase to uncover hidden anomalies."
                .to_string(),
        });
        path.push(SuggestedCommand {
            command: "loct repo-view".to_string(),
            reason: "Get a high-level overview of workspace metrics and languages.".to_string(),
        });
    }

    // Deduplicate commands in the path while preserving order
    let mut seen = HashSet::new();
    path.retain(|item| seen.insert(item.command.clone()));

    path
}

/// Cut 4 T3 — suggest grounded follow-up commands and likely verification.
pub fn compose_action_slice(
    opts: &ContextOptions,
    snapshot: &Snapshot,
    structural: &StructuralSlice,
    runtime: &RuntimeSlice,
    risk: &RiskSlice,
) -> ActionSlice {
    let scope = action_scope_files(opts, snapshot, structural);
    let project_root = action_project_root(opts, snapshot);
    let stack = detect_project_stack(&project_root);
    let gates = verification_gates_for(&scope, &stack, &project_root);
    let next_safe_commands = next_safe_commands_for(opts, structural, runtime, risk, snapshot);
    let likely_tests = likely_tests_for(&scope, snapshot, likely_tests_limit());
    let power_path = compute_power_path(opts, structural, runtime, risk, snapshot);

    ActionSlice {
        next_safe_command_authorities: next_safe_commands
            .iter()
            .map(|cmd| ActionAuthorityClaim {
                item: cmd.clone(),
                authority: AuthorityLabel::LoctreeDerived,
            })
            .collect(),
        verification_gate_authorities: gates
            .iter()
            .map(|gate| ActionAuthorityClaim {
                item: gate.command.clone(),
                authority: gate.authority,
            })
            .collect(),
        next_safe_commands,
        verification_gates: gates.into_iter().map(|gate| gate.command).collect(),
        likely_tests,
        power_path,
    }
}

/// Cut 5 T1 — fill the memory slice from AICX intent history.
///
/// Returns the empty slice when:
/// - `opts.with_aicx` is `false` (default — memory is opt-in),
/// - `aicx_client` is `None` (caller did not construct one),
/// - `aicx` is unavailable (the client transparently returns no rows).
///
/// When enabled, the composer builds a [`ScopeKeywords`] bag from the
/// structural slice (file paths, exported symbols) and the runtime slice
/// (idiom-tagged symbols), pulls a recent intent window from `aicx`, scores
/// every intent against the bag, drops zero-relevance rows, and ranks by
/// `(relevance desc, date desc, timestamp desc)`. The final list is capped
/// at `LOCT_CONTEXT_MEMORY_LIMIT` entries (default 50).
///
/// Authority labels follow the Cut 5 spec exactly:
/// - `decision` / `intent`             → [`AuthorityLabel::AicxOperator`]
/// - `outcome` (default)               → [`AuthorityLabel::AicxAgent`]
/// - `outcome` (operator-tagged)       → [`AuthorityLabel::AicxOperator`]
/// - `task`                            → [`AuthorityLabel::AicxAgent`]
/// - text matching failure markers     → [`AuthorityLabel::AicxFailure`]
///
/// Each entry carries the absolute path to its source markdown chunk
/// inside `~/.aicx/store/...` so a follow-up agent can read the original
/// conversation.
pub fn compose_memory_slice(
    opts: &ContextOptions,
    structural: &StructuralSlice,
    runtime: &RuntimeSlice,
    aicx_client: Option<&AicxClient>,
) -> MemorySlice {
    let namespace = aicx_project_bucket(opts);
    let seed_strategy = if opts.file.is_some() {
        "target_file".to_string()
    } else if opts.task.is_some() {
        "task_keywords".to_string()
    } else if opts.changed {
        "changed_files".to_string()
    } else {
        "default_scope".to_string()
    };

    if opts.no_aicx {
        return MemorySlice {
            diagnostic: Some(MemoryDiagnostic {
                engaged: false,
                namespace,
                seed_strategy,
                candidates_considered: 0,
                candidates_returned: 0,
                skip_reason: MemorySkipReason::DisabledByNoAicx,
                semantic_readiness: SemanticReadiness::Unknown,
            }),
            ..MemorySlice::default()
        };
    }
    if !opts.with_aicx {
        return MemorySlice {
            diagnostic: Some(MemoryDiagnostic {
                engaged: false,
                namespace,
                seed_strategy,
                candidates_considered: 0,
                candidates_returned: 0,
                skip_reason: MemorySkipReason::DisabledOptOut,
                semantic_readiness: SemanticReadiness::Unknown,
            }),
            ..MemorySlice::default()
        };
    }
    let Some(client) = aicx_client else {
        return MemorySlice {
            diagnostic: Some(MemoryDiagnostic {
                engaged: false,
                namespace,
                seed_strategy,
                candidates_considered: 0,
                candidates_returned: 0,
                skip_reason: MemorySkipReason::AicxUnreachable,
                semantic_readiness: SemanticReadiness::Unknown,
            }),
            ..MemorySlice::default()
        };
    };

    let keywords = build_scope_keywords(structural, runtime);
    let limit = opts.memory_limit.unwrap_or_else(memory_limit);
    let hours = opts.memory_hours.unwrap_or_else(memory_hours);
    // Plan L04 / Finding #17 — raw_limit now scales with the time
    // window so a wider `LOCT_CONTEXT_MEMORY_HOURS` actually reaches
    // older intents. Operators can override with
    // `LOCT_CONTEXT_MEMORY_RAW_LIMIT`. The default scales as `hours/2`
    // clamped to [50, 1000]: 7 d → 84 → 50 (close to legacy 100), 30 d
    // → 360, 90 d → 1000 (cap). Keeps aicx wrapper timeout in mind on
    // busy stores while no longer throwing away forensic context.
    let raw_limit = memory_raw_limit(hours, limit);

    let raw_intents = client.intents(hours, raw_limit);
    let candidates_considered = raw_intents.len();

    let mut scored: Vec<(u32, &crate::aicx::AicxIntent)> = raw_intents
        .iter()
        .filter_map(|intent| {
            let s = score_intent(intent, &keywords);
            if s == 0 { None } else { Some((s, intent)) }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.date.cmp(&a.1.date))
            .then_with(|| b.1.timestamp.cmp(&a.1.timestamp))
            .then_with(|| a.1.session_id.cmp(&b.1.session_id))
    });

    // Plan L02 / Finding #12 — score==0 fallback. When the keyword bag
    // produced zero overlaps across the entire raw intent window, fall
    // back to top-N newest with `low_lexical_match: true` so the slice
    // is not silently empty for active repos with narrow `--file` scope.
    let used_recency_fallback = scored.is_empty() && !raw_intents.is_empty();
    let ranked: Vec<(u32, &crate::aicx::AicxIntent, bool)> = if used_recency_fallback {
        let mut by_recency: Vec<&crate::aicx::AicxIntent> = raw_intents.iter().collect();
        by_recency.sort_by(|a, b| {
            b.date
                .cmp(&a.date)
                .then_with(|| b.timestamp.cmp(&a.timestamp))
                .then_with(|| a.session_id.cmp(&b.session_id))
        });
        by_recency
            .into_iter()
            .map(|intent| (0u32, intent, true))
            .collect()
    } else {
        scored
            .into_iter()
            .map(|(rel, intent)| (rel, intent, false))
            .collect()
    };

    // Plan L02 / Finding #1 — dedup `MemoryEntry` rows BEFORE the limit.
    // Same `(text, source_chunk_path)` pair can appear multiple times in
    // raw intents (multi-frame conversations re-quote the same line);
    // without this guard duplicates eat the limit and mask older but
    // distinct decisions.
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut entries: Vec<MemoryEntry> = Vec::with_capacity(ranked.len().min(limit));
    let mut chunks: HashSet<String> = HashSet::new();
    for (relevance, intent, low_lex) in ranked {
        if entries.len() >= limit {
            break;
        }
        let summary = summarize_entry(&intent.text);
        let key = (summary.text.clone(), intent.source_chunk_path.clone());
        if !seen.insert(key) {
            continue;
        }
        chunks.insert(intent.source_chunk_path.clone());
        // Authority gating — Wave 6b prep (loctree-side-needs.md §2).
        //
        // The kind-based label (`decision`/`task`/...) tells us what the
        // operator/agent claimed. The per-row `oracle_status` tells us how
        // AICX actually retrieved the row. When AICX explicitly marks the
        // row `loctree_scope_safe = false` (filesystem fuzzy fallback, or
        // any backend that cannot vouch for the canonical retrieval path)
        // we MUST NOT carry the row out at `AicxOperator` / `AicxAgent` /
        // `AicxFailure` authority — those tiers promise the row is the
        // semantic oracle's word. Demote to `SemanticGuess` so downstream
        // agents see "loctree-derived heuristic, verify before acting" and
        // not "operator decision, follow it".
        //
        // `oracle_status: None` means the wire predates the envelope (or
        // came from an in-process path that does not expose it yet); we
        // fall back to the kind-based label without demotion — preserves
        // legacy behaviour for the wire shapes that have not opted in to
        // the oracle contract.
        let kind_authority = intent_authority_label(authority_for_intent(intent));
        let authority = gate_authority_on_oracle(kind_authority, intent.oracle_status.as_ref());
        entries.push(MemoryEntry {
            kind: intent.kind.clone(),
            text: summary.text,
            authority,
            source_chunk: intent.source_chunk_path.clone(),
            agent: intent.agent.clone(),
            date: intent.date.clone(),
            timestamp: intent.timestamp.clone(),
            session_id: intent.session_id.clone(),
            project: intent.project.clone(),
            relevance,
            // `retrieval_score` / `retrieval_label` stay `None` until the
            // composer is wired to `client.search()` — only that surface
            // carries per-row scores. The intents path has none.
            retrieval_score: None,
            retrieval_label: None,
            // Closes audit finding A8: when AICX speaks the oracle envelope
            // (MCP / CLI transports), every intent row carries the
            // top-level `oracle_status`, so we can stamp the retrieval
            // mode on the memory entry without a second round-trip.
            // `None` means the row came back from a wire that predates the
            // oracle envelope (or the in-process library path, which does
            // not expose it yet).
            retrieval_mode: intent
                .oracle_status
                .as_ref()
                .map(|os| os.retrieval_mode().to_string()),
            low_lexical_match: low_lex,
        });
    }

    let mut source_chunks: Vec<String> = chunks.into_iter().collect();
    source_chunks.sort();

    let candidates_returned = entries.len();
    let skip_reason = if candidates_returned > 0 {
        MemorySkipReason::Ok
    } else if candidates_considered == 0 && client.transport_timed_out() {
        // Zero candidates because the transport never answered (per-call
        // timeout or exhausted overlay budget) — not because the store is
        // empty. Surface the difference instead of a fake "namespace empty".
        MemorySkipReason::TimedOut
    } else if candidates_considered == 0 {
        MemorySkipReason::NamespaceEmpty
    } else {
        MemorySkipReason::NoTokenOverlap
    };
    // Roll up oracle readiness across the raw intents we saw. Aggregate to
    // the LOWEST-trust observation so a slice that mixes a semantic-oracle
    // row with a fuzzy-fallback row presents as "unsafe" — that is the
    // honest summary for an agent deciding whether to act on the slice.
    // No row carrying an `OracleStatus` → readiness stays `Unknown` so the
    // caller can tell "AICX has not been reached for this kind of probe"
    // apart from "AICX answered but degraded".
    let semantic_readiness = raw_intents
        .iter()
        .filter_map(|intent| intent.oracle_status.as_ref())
        .map(crate::aicx::OracleStatus::readiness)
        .reduce(crate::aicx::SemanticReadiness::min)
        .unwrap_or(SemanticReadiness::Unknown);

    MemorySlice {
        entries,
        source_chunks,
        diagnostic: Some(MemoryDiagnostic {
            engaged: true,
            namespace,
            seed_strategy,
            candidates_considered,
            candidates_returned,
            skip_reason,
            semantic_readiness,
        }),
        overlay: None,
    }
}

fn build_scope_keywords(structural: &StructuralSlice, runtime: &RuntimeSlice) -> ScopeKeywords {
    let mut bag = ScopeKeywords::default();
    for f in &structural.files {
        bag.insert_path(&f.path);
    }
    for s in &structural.symbols {
        bag.insert_symbol(&s.name);
    }
    for tag in &runtime.idiom_tags {
        bag.insert_symbol(&tag.symbol);
        bag.insert_symbol(&tag.name);
    }
    for hint in &runtime.framework_hints {
        bag.insert_symbol(&hint.symbol);
    }
    bag
}

fn intent_authority_label(authority: IntentAuthority) -> AuthorityLabel {
    match authority {
        IntentAuthority::Operator => AuthorityLabel::AicxOperator,
        IntentAuthority::Agent => AuthorityLabel::AicxAgent,
        IntentAuthority::Failure => AuthorityLabel::AicxFailure,
    }
}

/// Demote AICX-tier authority to [`AuthorityLabel::SemanticGuess`] when
/// AICX itself says the retrieval was not scope-safe.
///
/// The kind-based [`intent_authority_label`] mapping promises the row
/// is the semantic oracle's word (Operator / Agent / Failure are all
/// "trust this enough to scope on it"). When AICX falls back to the
/// filesystem-fuzzy path it sets `loctree_scope_safe = false` on the
/// row's [`crate::aicx::OracleStatus`] envelope; carrying the row out
/// at AICX-tier authority would silently lie to downstream agents.
/// Demote to `SemanticGuess` — "loctree-derived heuristic, verify
/// before acting" — so the honesty surface in the agent pack matches
/// what AICX actually delivered.
///
/// Non-AICX kind authorities (`RepoVerified`, `LoctreeDerived`, etc.)
/// pass through unchanged: those tiers do not encode an AICX-oracle
/// claim, so there is nothing to demote.
///
/// `oracle_status: None` preserves the kind-based label — the wire
/// shape predates the oracle envelope, so we have nothing to gate on.
/// Once AICX universally stamps the envelope (post-P0), this branch
/// becomes dead code; the trait surface in
/// [`crate::aicx::IntentSource`] is the seam where we will tighten
/// the contract.
pub(crate) fn gate_authority_on_oracle(
    kind_authority: AuthorityLabel,
    oracle: Option<&crate::aicx::OracleStatus>,
) -> AuthorityLabel {
    let is_aicx_tier = matches!(
        kind_authority,
        AuthorityLabel::AicxOperator | AuthorityLabel::AicxAgent | AuthorityLabel::AicxFailure
    );
    if !is_aicx_tier {
        return kind_authority;
    }
    match oracle {
        Some(os) if !os.loctree_scope_safe => AuthorityLabel::SemanticGuess,
        _ => kind_authority,
    }
}

fn memory_limit() -> usize {
    std::env::var("LOCT_CONTEXT_MEMORY_LIMIT")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(50)
}

fn memory_hours() -> u64 {
    // Default to a 7-day window (168 h). The deeper history exists in AICX
    // but is rarely the most relevant context for an agent that is about
    // to touch code; longer windows also push aicx past the wrapper's
    // default 5 s timeout on busy stores. Operators can widen with
    // `LOCT_CONTEXT_MEMORY_HOURS=720` (or higher) when forensic context
    // is more important than freshness.
    std::env::var("LOCT_CONTEXT_MEMORY_HOURS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(168)
}

/// Resolve the raw fetch ceiling for `aicx intents` (Plan L04 / Finding #17).
///
/// Priority:
/// 1. `LOCT_CONTEXT_MEMORY_RAW_LIMIT` env override (operator-supplied).
/// 2. Window-scaled default: `hours / 2` clamped to `[50, 1000]`. So a
///    7-day window (168 h) yields 84 → 50, a 30-day window yields 360,
///    a 90-day window yields 1000 (the cap). Always at least
///    `limit.saturating_mul(2)` so the relevance filter still has
///    headroom over the final `--limit`.
fn memory_raw_limit(hours: u64, limit: usize) -> usize {
    if let Some(override_n) = std::env::var("LOCT_CONTEXT_MEMORY_RAW_LIMIT")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|n| *n > 0)
    {
        return override_n;
    }
    let scaled = (hours / 2).clamp(50, 1000) as usize;
    scaled.max(limit.saturating_mul(2))
}

/// Resolve the AICX project bucket for this context invocation.
///
/// Resolution priority (Plan L04 / Finding #16):
/// 1. `opts.aicx_project_override` — operator-supplied via
///    `--aicx-project <bucket>`. Highest priority, no further checks.
/// 2. Last-segment `file_name()` of the canonical snapshot root —
///    legacy heuristic. Wrong for monorepos / fixtures / worktrees,
///    but the right choice for top-level repos and the operator can
///    always override with `--aicx-project`.
/// 3. Hard-coded `"loctree"` when even `file_name()` fails — extreme
///    edge case (path is `/` or empty).
pub fn aicx_project_bucket(opts: &ContextOptions) -> String {
    if let Some(override_bucket) = opts.aicx_project_override.as_deref()
        && !override_bucket.trim().is_empty()
    {
        return override_bucket.trim().to_string();
    }
    let root = context_snapshot_root(opts);
    let canonical = root.canonicalize().unwrap_or(root);
    canonical
        .file_name()
        .and_then(|name| name.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "loctree".to_string())
}

pub(crate) fn context_snapshot_root(opts: &ContextOptions) -> PathBuf {
    let roots = context_roots(opts);
    let primary = roots.first().cloned().unwrap_or_else(|| PathBuf::from("."));
    Snapshot::find_loctree_root(&primary).unwrap_or_else(|| resolve_snapshot_root(&roots))
}

fn action_project_root(opts: &ContextOptions, snapshot: &Snapshot) -> PathBuf {
    if let Some(root) = snapshot.metadata.roots.first() {
        let root = PathBuf::from(root);
        if root.exists() {
            return root;
        }
    }
    context_snapshot_root(opts)
}

fn current_git_head(root: &Path) -> Option<String> {
    crate::git::GitRepo::discover(root)
        .ok()
        .and_then(|repo| repo.head_commit().ok())
        .filter(|head| !head.is_empty())
}

fn git_worktree_dirty(root: &Path) -> Option<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

fn snapshot_commit_is_stale(snapshot: &Snapshot, current_head: Option<&str>) -> bool {
    let Some(snapshot_commit) = snapshot.metadata.git_commit.as_deref() else {
        return false;
    };
    let Some(current_head) = current_head else {
        // Fail-closed: a named snapshot commit with no live HEAD is not "fresh".
        return true;
    };
    !(current_head.starts_with(snapshot_commit) || snapshot_commit.starts_with(current_head))
}

fn cache_scope_for(
    snapshot: &Snapshot,
    snapshot_root: &Path,
    stale_snapshot: bool,
    dirty_worktree: bool,
    git_unknown: bool,
) -> RiskCacheScope {
    if snapshot.metadata.roots.is_empty() {
        return RiskCacheScope::Unknown;
    }

    let expected = normalize_roots_for_scope_compare(std::iter::once(snapshot_root), snapshot_root);
    let actual = normalize_roots_for_scope_compare(
        snapshot.metadata.roots.iter().map(Path::new),
        snapshot_root,
    );
    if expected != actual {
        return RiskCacheScope::Unknown;
    }
    if git_unknown {
        return RiskCacheScope::Unknown;
    }
    if stale_snapshot {
        return RiskCacheScope::StaleSnapshot;
    }
    if dirty_worktree {
        return RiskCacheScope::DirtyWorktree;
    }
    RiskCacheScope::Clean
}

fn snapshot_health_label(
    stale_snapshot: bool,
    dirty_worktree: bool,
    empty_corpus: bool,
    git_unknown: bool,
) -> &'static str {
    // Empty successful snapshot wins the clean label so agents can separate
    // "verified zero files" from "no authority / missing snapshot".
    if empty_corpus && !stale_snapshot && !dirty_worktree {
        return "empty_snapshot";
    }
    if git_unknown {
        return "unknown";
    }
    match (stale_snapshot, dirty_worktree) {
        (true, true) => "stale_dirty",
        (true, false) => "stale",
        (false, true) => "dirty",
        (false, false) => "fresh",
    }
}

fn risk_scope_files(opts: &ContextOptions, snapshot: &Snapshot) -> Vec<String> {
    if let Some(file) = &opts.file {
        return resolve_context_file(snapshot, file)
            .map(|file| vec![file])
            .unwrap_or_else(|| vec![normalize_requested_path(snapshot, file)]);
    }

    snapshot
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect()
}

fn action_scope_files(
    opts: &ContextOptions,
    snapshot: &Snapshot,
    structural: &StructuralSlice,
) -> Vec<String> {
    if let Some(file) = &opts.file {
        return resolve_context_file(snapshot, file)
            .map(|file| vec![file])
            .unwrap_or_else(|| vec![normalize_requested_path(snapshot, file)]);
    }

    let mut scope: Vec<String> = structural
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    if scope.is_empty() {
        scope = risk_scope_files(opts, snapshot);
    }
    scope.sort();
    scope.dedup();
    scope
}

fn resolve_context_file(snapshot: &Snapshot, file: &Path) -> Option<String> {
    let normalized = normalize_requested_path(snapshot, file);
    if snapshot
        .files
        .iter()
        .any(|analysis| analysis.path == normalized)
    {
        return Some(normalized);
    }

    let suffix_match = suffixes_without_leading_components(&normalized).find(|suffix| {
        snapshot
            .files
            .iter()
            .any(|analysis| analysis.path == *suffix)
    });
    if let Some(suffix) = suffix_match {
        return Some(suffix);
    }

    let file_name = file.file_name()?.to_str()?;
    let importer_counts = importer_counts_direct(snapshot);
    let mut matches: Vec<String> = snapshot
        .files
        .iter()
        .filter(|analysis| {
            Path::new(&analysis.path)
                .file_name()
                .and_then(|name| name.to_str())
                == Some(file_name)
        })
        .map(|analysis| analysis.path.clone())
        .collect();
    matches.sort_by(|a, b| {
        importer_counts
            .get(b)
            .copied()
            .unwrap_or(0)
            .cmp(&importer_counts.get(a).copied().unwrap_or(0))
            .then_with(|| a.matches('/').count().cmp(&b.matches('/').count()))
            .then_with(|| a.len().cmp(&b.len()))
            .then_with(|| a.cmp(b))
    });
    matches.into_iter().next()
}

fn normalize_requested_path(snapshot: &Snapshot, file: &Path) -> String {
    let raw = file.to_string_lossy();
    snapshot.normalize_path(raw.trim_start_matches("./"))
}

fn context_target_path(target: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for component in Path::new(target).components() {
        match component {
            std::path::Component::Normal(part) => path.push(part),
            std::path::Component::CurDir => {}
            _ => {}
        }
    }
    path
}

fn suffixes_without_leading_components(path: &str) -> impl Iterator<Item = String> + '_ {
    let parts: Vec<&str> = path.split('/').collect();
    (1..parts.len()).map(move |start| parts[start..].join("/"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerificationGate {
    command: String,
    authority: AuthorityLabel,
}

fn verification_gates_for(
    _scope: &[String],
    stack: &ProjectStack,
    project_root: &Path,
) -> Vec<VerificationGate> {
    if !matches!(stack, ProjectStack::Rust { .. } | ProjectStack::Mixed(_)) {
        let make_targets = read_makefile_test_targets(project_root);
        if !make_targets.is_empty() {
            return make_targets
                .into_iter()
                .map(|target| VerificationGate {
                    command: format!("make {target}"),
                    authority: AuthorityLabel::RepoVerified,
                })
                .collect();
        }
    }

    let ci_commands = extract_ci_test_commands(project_root);
    if !ci_commands.is_empty() && !matches!(stack, ProjectStack::Rust { .. }) {
        return ci_commands
            .into_iter()
            .map(|command| VerificationGate {
                command,
                authority: AuthorityLabel::RepoVerified,
            })
            .collect();
    }

    stack_derived_verification_gates(stack, project_root)
}

fn stack_derived_verification_gates(
    stack: &ProjectStack,
    project_root: &Path,
) -> Vec<VerificationGate> {
    let commands = match stack {
        ProjectStack::Rust { workspace_members } => {
            if workspace_members.len() > 1 || has_workspace_manifest(project_root) {
                vec![
                    "cargo check --workspace".to_string(),
                    "cargo clippy --workspace --all-targets -- -D warnings".to_string(),
                    "cargo test --workspace".to_string(),
                ]
            } else {
                vec![
                    "cargo check".to_string(),
                    "cargo clippy --all-targets -- -D warnings".to_string(),
                    "cargo test".to_string(),
                ]
            }
        }
        ProjectStack::Python {
            has_pytest,
            has_ruff,
            has_mypy,
            ..
        } => {
            let mut gates = Vec::new();
            if *has_ruff {
                gates.push("ruff check .".to_string());
            }
            if *has_mypy {
                gates.push("mypy .".to_string());
            }
            if *has_pytest {
                gates.push("pytest".to_string());
            }
            if gates.is_empty() {
                gates.push("python -m pytest".to_string());
            }
            gates
        }
        ProjectStack::NodeJs {
            package_manager,
            has_lint,
            has_test,
            has_check,
            has_build,
        } => node_verification_commands(
            *package_manager,
            *has_lint,
            *has_test,
            *has_check,
            *has_build,
        ),
        ProjectStack::Mixed(stacks) => {
            let mut commands = Vec::new();
            for stack in stacks {
                commands.extend(
                    stack_derived_verification_gates(stack, project_root)
                        .into_iter()
                        .map(|gate| gate.command)
                        .take(2),
                );
                if commands.len() >= 5 {
                    break;
                }
            }
            dedup_top_n(commands, 5)
        }
        ProjectStack::Unknown => Vec::new(),
    };

    commands
        .into_iter()
        .map(|command| VerificationGate {
            command,
            authority: if matches!(stack, ProjectStack::Unknown) {
                AuthorityLabel::StaleOrUnknown
            } else {
                AuthorityLabel::LoctreeDerived
            },
        })
        .collect()
}

fn node_verification_commands(
    package_manager: PackageManager,
    has_lint: bool,
    has_test: bool,
    has_check: bool,
    has_build: bool,
) -> Vec<String> {
    let pm = package_manager.command();
    let mut gates = Vec::new();
    if has_lint {
        gates.push(format!("{pm} lint"));
    }
    if has_check {
        gates.push(format!("{pm} check"));
    }
    if has_test {
        gates.push(format!("{pm} test"));
    }
    if gates.is_empty() && has_build {
        gates.push(format!("{pm} build"));
    }
    gates
}

fn has_workspace_manifest(project_root: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(project_root.join("Cargo.toml")) else {
        return false;
    };
    content
        .parse::<toml::Table>()
        .ok()
        .and_then(|value| value.get("workspace").cloned())
        .is_some()
}

/// Build the three "what should I do next?" commands.
///
/// Two regimes:
///
/// - **Targeted call** (`opts.file` is set): emit `loct slice <target>`,
///   `loct impact <target>`, and a structurally-distinct third command —
///   either `loct follow trace --to <symbol>` for the most-exported symbol,
///   or `loct find <symbol>` when no symbol is in scope. The verb-on-noun
///   shape matches what an agent already chose to focus on.
///
/// - **Bare call** (no `opts.file`, no `task=`, no `--changed`): diversify
///   across the three highest-leverage facts in the pack — the top hotspot,
///   the most informative entrypoint, and the most-imported public symbol.
///   This avoids the previous failure mode where all three commands targeted
///   the same arbitrary file.
fn next_safe_commands_for(
    opts: &ContextOptions,
    structural: &StructuralSlice,
    runtime: &RuntimeSlice,
    risk: &RiskSlice,
    snapshot: &Snapshot,
) -> Vec<String> {
    let mut cmds: Vec<String> = Vec::new();
    let push_unique = |cmd: String, sink: &mut Vec<String>| {
        if !sink.contains(&cmd) {
            sink.push(cmd);
        }
    };

    let bare = is_bare_context(opts);

    if bare {
        // Move 1: deepest hotspot — the file most other code depends on.
        if let Some(hot) = risk.hotspots.first() {
            push_unique(format!("loct slice {}", hot.file), &mut cmds);
        }
        // Move 2: most informative entrypoint, preferring server / app-style
        // kinds. Falls back to whatever the structural slice surfaced.
        if let Some(entry) = pick_informative_entrypoint(structural) {
            push_unique(format!("loct context --file {entry}"), &mut cmds);
        }
        // Move 3: top public symbol the rest of the codebase actually uses.
        if let Some(sym) = top_imported_public_symbol(snapshot) {
            push_unique(format!("loct find {sym}"), &mut cmds);
        }
        // Backfill if heuristics could not produce three distinct commands.
        if let Some(hot) = risk.hotspots.first() {
            push_unique(format!("loct impact {}", hot.file), &mut cmds);
        }
        if let Some(file) = structural.files.first() {
            push_unique(format!("loct context --file {}", file.path), &mut cmds);
        }
    } else {
        // Targeted: stay anchored to the operator's chosen file/scope.
        if let Some(hot) = risk.hotspots.first() {
            push_unique(format!("loct slice {}", hot.file), &mut cmds);
            push_unique(format!("loct impact {}", hot.file), &mut cmds);
        }
        // Replace the third "context --file <same>" suggestion with a
        // structurally distinct probe (symbol search). The targeted call
        // already viewed `--file <target>`, so we want a different shape.
        if let Some(target_sym) = first_target_symbol(structural) {
            push_unique(format!("loct find {target_sym}"), &mut cmds);
        } else if let Some(file) = structural.files.first() {
            push_unique(format!("loct context --file {}", file.path), &mut cmds);
        }
        // Also surface the matching tauri command / FastAPI route for trace
        // when one exists in scope.
        if let Some(handler_sym) = first_runtime_handler_symbol(runtime) {
            push_unique(format!("loct follow trace --to {handler_sym}"), &mut cmds);
        }
    }

    if cmds.is_empty() {
        cmds.push("loct repo-view".to_string());
    }

    cmds.truncate(3);
    cmds
}

/// Pick a structural entrypoint that maximises information density. Prefers
/// "app" / "asgi" / "wsgi" / "tauri" kinds (servers and desktop hosts) over
/// plain `script` entries — the agent already understands what a CLI script
/// is, but a FastAPI app surfaces a whole route table.
fn pick_informative_entrypoint(structural: &StructuralSlice) -> Option<String> {
    const PREFERRED_KIND_SUBSTRINGS: &[&str] = &[
        "fastapi_app",
        "starlette_app",
        "flask_app",
        "asgi_target",
        "wsgi_target",
        "tauri_command_host",
        "_app",
    ];
    for prefix in PREFERRED_KIND_SUBSTRINGS {
        for entry in &structural.entrypoints {
            if entry.kinds.iter().any(|k| k.contains(prefix)) {
                return Some(entry.path.clone());
            }
        }
    }
    structural.entrypoints.first().map(|e| e.path.clone())
}

/// Most-imported public symbol across the snapshot, by count of `imports.symbols`
/// referencing it. Used to ground `loct find <symbol>` on something the rest of
/// the codebase actually depends on, rather than an arbitrary first export.
fn top_imported_public_symbol(snapshot: &Snapshot) -> Option<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for file in &snapshot.files {
        for imp in &file.imports {
            for sym in &imp.symbols {
                let name = sym.alias.clone().unwrap_or_else(|| sym.name.clone());
                if name.is_empty() || name.starts_with('_') {
                    continue;
                }
                *counts.entry(name).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(name, _)| name)
}

fn first_target_symbol(structural: &StructuralSlice) -> Option<String> {
    structural.symbols.first().map(|s| s.name.clone())
}

fn first_runtime_handler_symbol(runtime: &RuntimeSlice) -> Option<String> {
    if let Some(cmd) = runtime.tauri_commands.first() {
        return Some(cmd.name.clone());
    }
    runtime
        .framework_hints
        .iter()
        .find(|h| h.kind.ends_with("_route") || h.kind == "tauri_command")
        .map(|h| h.symbol.clone())
}

/// Return the `top_n` tests most likely to exercise the in-flight scope,
/// ranked by symbol-overlap relevance instead of alphabet.
///
/// Scoring (per (test, scope_file) pair):
/// - +N for each imported symbol that matches a `scope_file` export
/// - +1 fallback for plain "imports the scope file" with no symbol match
/// - +K for tests whose path is itself in scope (covers `--task` / changed
///   self-tests)
///
/// Each scope file gets a decreasing weight (first scope file is highest)
/// so calls with multiple targets favour tests of the primary one. Ties
/// break alphabetically. Falls back to the previous "tests of top hubs"
/// behaviour when scope is empty.
fn likely_tests_for(scope: &[String], snapshot: &Snapshot, top_n: usize) -> Vec<String> {
    if top_n == 0 {
        return Vec::new();
    }
    if scope.is_empty() {
        return relevant_tests_for_top_hubs(snapshot, top_n);
    }

    // scope_file → (path, exports). Keep `path` cloned for ergonomics later.
    let mut scope_lookup: HashMap<String, HashSet<String>> = HashMap::new();
    for path in scope {
        if let Some(file) = snapshot.files.iter().find(|f| f.path == *path) {
            let exports = file
                .exports
                .iter()
                .map(|e| e.name.clone())
                .collect::<HashSet<_>>();
            scope_lookup.insert(path.clone(), exports);
        }
    }

    let mut scores: HashMap<String, usize> = HashMap::new();

    // Score every test in the snapshot by how strongly it imports scope files.
    for file in &snapshot.files {
        if !is_likely_test_file(file) {
            continue;
        }
        let mut total: usize = 0;
        for (idx, scope_path) in scope.iter().enumerate() {
            let scope_weight = (scope.len() - idx).max(1);
            let scope_exports = scope_lookup.get(scope_path);
            for imp in &file.imports {
                let resolves_to_scope = imp.resolved_path.as_deref() == Some(scope_path)
                    || imp.source == *scope_path
                    || imp.source.ends_with(scope_path);
                if resolves_to_scope {
                    let symbols_count = imp.symbols.len().max(1);
                    total = total.saturating_add(symbols_count * scope_weight);
                    continue;
                }
                if let Some(exports) = scope_exports {
                    let mut local = 0usize;
                    for sym in &imp.symbols {
                        let name = sym.alias.clone().unwrap_or_else(|| sym.name.clone());
                        if exports.contains(&name) {
                            local += 1;
                        }
                    }
                    if local > 0 {
                        total = total.saturating_add(local * scope_weight);
                    }
                }
            }
        }
        if total > 0 {
            scores.insert(file.path.clone(), total);
        }
    }

    // Test files that are themselves in scope (e.g. `--task` zeroed in on a
    // test, or `--changed` includes one) get a strong boost.
    for path in scope {
        if let Some(file) = snapshot.files.iter().find(|f| f.path == *path)
            && is_likely_test_file(file)
        {
            *scores.entry(path.clone()).or_default() += 1_000;
        }
    }

    // Fallback: when the scope yielded no scoring tests, fall back to the
    // legacy "tests of top hubs" ranking — better than silently empty.
    if scores.is_empty() {
        return relevant_tests_for_top_hubs(snapshot, top_n);
    }

    let mut ranked: Vec<(String, usize)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .take(top_n)
        .map(|(path, _)| path)
        .collect()
}

fn relevant_tests_for_top_hubs(snapshot: &Snapshot, top_n: usize) -> Vec<String> {
    let top_hubs = top_hub_files(snapshot, 3);
    let mut seen = HashSet::new();
    let mut tests = Vec::new();
    for hub in &top_hubs {
        for importer in query_who_imports(snapshot, hub).results {
            let Some(file) = snapshot.files.iter().find(|f| f.path == importer.file) else {
                continue;
            };
            if is_likely_test_file(file) && seen.insert(importer.file.clone()) {
                tests.push(importer.file);
                if tests.len() >= top_n {
                    return tests;
                }
            }
        }
    }
    tests
}

fn likely_tests_limit() -> usize {
    std::env::var("LOCT_CONTEXT_LIKELY_TESTS_LIMIT")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|limit| *limit > 0)
        .unwrap_or(10)
}

fn is_test_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized.starts_with("tests/")
        || normalized.contains("/tests/")
        || normalized.contains("/__tests__/")
        || normalized.ends_with("_test.rs")
        || normalized.ends_with("_tests.rs")
        || normalized.ends_with(".test.ts")
        || normalized.ends_with(".test.tsx")
        || normalized.ends_with(".spec.ts")
        || normalized.ends_with(".spec.tsx")
        || normalized.ends_with("_test.py")
        || normalized.ends_with("_tests.py")
}

fn is_likely_test_file(file: &crate::types::FileAnalysis) -> bool {
    (file.is_test || is_test_path(&file.path))
        && is_test_language_or_extension(&file.language, &file.path)
        && artifact_class(&file.path, None) != ArtifactClass::Fixture
}

fn is_test_language_or_extension(language: &str, path: &str) -> bool {
    if !language.is_empty() {
        return matches!(
            language,
            "rust"
                | "rs"
                | "python"
                | "py"
                | "typescript"
                | "javascript"
                | "tsx"
                | "jsx"
                | "ts"
                | "js"
                | "go"
                | "java"
                | "kotlin"
                | "swift"
                | "ruby"
                | "php"
                | "c"
                | "cpp"
                | "cxx"
                | "csharp"
                | "shell"
                | "sh"
        );
    }
    let normalized = path.replace('\\', "/");
    matches!(
        Path::new(&normalized)
            .extension()
            .and_then(|ext| ext.to_str()),
        Some(
            "rs" | "py"
                | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "go"
                | "java"
                | "kt"
                | "swift"
                | "rb"
                | "php"
                | "c"
                | "cc"
                | "cpp"
                | "cxx"
                | "cs"
                | "sh"
        )
    )
}

fn determine_targets(opts: &ContextOptions, snapshot: &Snapshot) -> Vec<String> {
    if let Some(file) = &opts.file {
        if let Some(resolved) = resolve_context_file(snapshot, file) {
            return vec![resolved];
        }
        return vec![normalize_requested_path(snapshot, file)];
    }

    if opts.changed {
        return get_changed_targets(opts, snapshot);
    }

    if let Some(task) = &opts.task {
        return get_task_targets(task, snapshot);
    }

    Vec::new()
}

fn resolve_context_scope_for_opts(
    opts: &ContextOptions,
    project_root: &Path,
    snapshot: &Snapshot,
) -> Result<Option<ResolvedScope>, ContextLoadError> {
    if opts.file.is_some() || opts.scopes.is_empty() {
        return Ok(None);
    }
    resolve_scope_with_mode(&opts.scopes, project_root, snapshot, opts.scope_mode)
        .map(Some)
        .map_err(ContextLoadError::Scope)
}

fn scoped_targets(
    opts: &ContextOptions,
    snapshot: &Snapshot,
    scope: &ResolvedScope,
) -> Vec<String> {
    if let Some(task) = opts.task.as_deref() {
        let mut targets: Vec<String> = get_task_targets(task, snapshot)
            .into_iter()
            .filter(|target| scope.contains(target))
            .collect();
        if targets.is_empty() {
            targets = scope.matched_files();
        }
        return targets;
    }
    scope.matched_files()
}

fn task_report_for_opts(opts: &ContextOptions, has_scope: bool) -> Option<TaskReport> {
    opts.task.as_ref().map(|task| TaskReport {
        text: task.clone(),
        mode: if has_scope {
            "ranker_within_scope".to_string()
        } else {
            "ranker".to_string()
        },
        authority: "semantic_guess".to_string(),
    })
}

fn apply_scope_cache_marker(pack: &mut ContextPack) {
    if let Some(scope) = &pack.scope {
        pack.risk.cache_scope = RiskCacheScope::Scoped(scope.fingerprint.clone());
        pack.risk.cache_scope_authority = AuthorityLabel::RepoVerified;
        pack.risk.snapshot_health = Some(if scope.empty {
            "scoped_empty".to_string()
        } else {
            "scoped".to_string()
        });
    }
}

fn is_bare_context(opts: &ContextOptions) -> bool {
    opts.file.is_none() && !opts.changed && opts.task.is_none() && opts.scopes.is_empty()
}

/// Construct the AICX client for a `loct context` composition, honoring the
/// overlay opt-in/opt-out matrix:
/// - `--no-aicx` → no client,
/// - `--with-aicx` → patient client (operator explicitly asked for memory;
///   the per-call `LOCT_AICX_TIMEOUT_SECS` ceiling applies),
/// - anything else (including the bare session-start path) → no client.
///
/// I1-01 retired the bare-path budgeted client: the 300 ms budget timed out
/// on 100% of bare invocations against a live store (`aicx intents` needs
/// tens of seconds there), so the bare path now consumes the C0-01
/// `aicx overlay` cache instead — see [`compose_bare_overlay_state`]. The
/// budgeted [`AicxClient::new_budgeted`] machinery stays for callers that
/// genuinely want a bounded synchronous transport.
fn build_context_aicx_client(opts: &ContextOptions) -> Option<AicxClient> {
    if opts.no_aicx {
        return None;
    }
    if opts.with_aicx {
        return Some(AicxClient::new(aicx_project_bucket(opts)));
    }
    None
}

/// Compose the C0-01 overlay render state for the bare path.
///
/// Local truth is what loctree can prove without asking the producer:
/// the snapshot commit and the `loctree.anchors.v1` catalog revision (both
/// derived from the loaded snapshot — [`crate::anchors::build_anchor_catalog`]
/// is pure in-process work, ~tens of ms) plus the repo identity.
/// Producer-side revisions are trusted as signed and converged by the
/// detached refresh.
fn compose_bare_overlay_state(
    opts: &ContextOptions,
    bare_context: bool,
    project_root: &Path,
    snapshot: &Snapshot,
) -> Option<crate::aicx::overlay::OverlayRenderState> {
    if !bare_context || opts.no_aicx || opts.with_aicx {
        return None;
    }
    let mut local = crate::aicx::overlay::LocalTruth {
        repo_id: None,
        snapshot_commit: snapshot
            .metadata
            .git_commit
            .as_ref()
            .filter(|commit| !commit.is_empty())
            .cloned(),
        anchor_catalog_revision: None,
    };
    if let Ok(catalog) = crate::anchors::build_anchor_catalog(snapshot, project_root) {
        local.repo_id = Some(catalog.repo_id);
        local.anchor_catalog_revision = Some(catalog.anchor_catalog_revision);
    }
    Some(crate::aicx::overlay::compose_overlay_state(
        project_root,
        &local,
    ))
}

/// Fail-closed scrub of AICX overlay/memory before a pack is marked commitable.
/// Count is written once onto the receipt so compose+render never double-count.
fn scrub_context_pack(pack: &mut ContextPack) {
    let mut count = 0u32;
    for entry in &mut pack.memory.entries {
        scrub_field(&mut entry.text, &mut count);
        scrub_field(&mut entry.source_chunk, &mut count);
    }
    for chunk in &mut pack.memory.source_chunks {
        scrub_field(chunk, &mut count);
    }
    if let Some(overlay) = pack.memory.overlay.as_mut() {
        for thesis in &mut overlay.theses {
            scrub_field(thesis, &mut count);
        }
        for path in &mut overlay.scope_paths {
            scrub_field(path, &mut count);
        }
        scrub_field(&mut overlay.refresh_command, &mut count);
    }
    pack.receipt.redactions = count;
}

fn scrub_field(value: &mut String, count: &mut u32) {
    let result = redact_secrets(value);
    if result.count > 0 {
        *count += result.count;
        *value = result.text;
    }
}

pub fn compose_default_scope(
    snapshot: &Snapshot,
    opts: &ContextOptions,
    aicx: Option<&AicxClient>,
    overlay: Option<&crate::aicx::overlay::OverlayRenderState>,
) -> Vec<String> {
    let mut targets = Vec::new();

    // Runtime roots are the safest first anchors for a repo-wide context.
    // They must not disappear merely because eight high-fan-in or high-LOC
    // files outrank them.
    targets.extend(
        snapshot
            .metadata
            .entrypoints
            .iter()
            .map(|entrypoint| entrypoint.path.clone()),
    );
    targets.extend(top_hub_files(snapshot, 8));
    targets.extend(recently_changed_files(opts, snapshot, 48, 4));
    targets.extend(aicx_intent_scope_files(snapshot, aicx, 5));
    targets.extend(overlay_scope_files(snapshot, overlay));
    retain_context_targets(&mut targets);

    if targets.is_empty() {
        targets.extend(top_loc_files(snapshot, 10));
        retain_context_targets(&mut targets);
    }

    if targets.len() < 3 {
        targets.extend(top_loc_files(snapshot, 10));
        retain_context_targets(&mut targets);
    }

    stable_dedup_strings(&mut targets);
    targets.truncate(DEFAULT_SCOPE_TARGET_LIMIT);
    targets
}

fn non_empty_context_path(path: &Path) -> Option<String> {
    let target = path.to_string_lossy().to_string();
    if target.trim().is_empty() {
        None
    } else {
        Some(target)
    }
}

fn retain_context_targets(targets: &mut Vec<String>) {
    targets.retain(|target| !target.trim().is_empty());
}

fn bind_pack_coverage(pack: &mut ContextPack, snapshot: &Snapshot, selected: usize) {
    let total = snapshot.files.len();
    pack.coverage = PackCoverage {
        selected,
        total,
        truncated: selected < total,
    };
}

fn top_hub_files(snapshot: &Snapshot, limit: usize) -> Vec<String> {
    top_hubs_by_importers_direct(snapshot, limit)
        .into_iter()
        .map(|metric| metric.file)
        .collect()
}

fn top_loc_files(snapshot: &Snapshot, limit: usize) -> Vec<String> {
    let mut ranked: Vec<(String, usize)> = snapshot
        .files
        .iter()
        .map(|file| (file.path.clone(), file.loc))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .take(limit)
        .map(|(path, _)| path)
        .collect()
}

fn recently_changed_files(
    opts: &ContextOptions,
    snapshot: &Snapshot,
    hours: u64,
    limit: usize,
) -> Vec<String> {
    let root = context_snapshot_root(opts);
    let since = format!("{hours} hours ago");
    let output = Command::new("git")
        .args([
            "log",
            "--name-only",
            "--pretty=format:",
            "--since",
            since.as_str(),
            "--",
        ])
        .current_dir(&root)
        .output();

    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    let known: HashSet<&str> = snapshot
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    let mut changed = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        let normalized = snapshot.normalize_path(path);
        if known.contains(normalized.as_str()) {
            changed.push(normalized);
        }
    }

    changed.sort();
    changed.dedup();
    changed.truncate(limit);
    changed
}

/// Seed the default scope from the cached overlay's attributed targets.
/// Zero-cost replacement for the retired bare-path intents probe: the
/// producer already resolved intent→anchor attributions, so the target
/// paths are typed joins, not keyword guesses. Only paths that exist in
/// the current snapshot survive.
fn overlay_scope_files(
    snapshot: &Snapshot,
    overlay: Option<&crate::aicx::overlay::OverlayRenderState>,
) -> Vec<String> {
    let Some(overlay) = overlay else {
        return Vec::new();
    };
    overlay
        .scope_paths
        .iter()
        .filter(|path| snapshot.files.iter().any(|file| &&file.path == path))
        .cloned()
        .collect()
}

fn aicx_intent_scope_files(
    snapshot: &Snapshot,
    aicx: Option<&AicxClient>,
    limit: usize,
) -> Vec<String> {
    let Some(client) = aicx else {
        return Vec::new();
    };

    // Same (window, limit) as the memory-slice fetch in
    // `compose_memory_slice` — the client caches intents per
    // (scope, hours, limit), so unifying the key means ONE transport
    // round-trip serves both scope seeding and the memory overlay. The
    // seeding view only grows (168 h ⊇ 72 h), never shrinks.
    let hours = memory_hours();
    let tokens: HashSet<String> = client
        .intents(hours, memory_raw_limit(hours, memory_limit()))
        .into_iter()
        .flat_map(|intent| tokenize_scope_text(&intent.text))
        .collect();
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut scored = Vec::new();
    for file in &snapshot.files {
        let mut score = 0usize;
        let path_lower = file.path.to_lowercase();
        for token in &tokens {
            if path_lower.contains(token) {
                score += 3;
            }
            for export in &file.exports {
                if export.name.to_lowercase().contains(token) {
                    score += 2;
                }
            }
        }
        if score > 0 {
            scored.push((file.path.clone(), score));
        }
    }

    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored
        .into_iter()
        .take(limit)
        .map(|(path, _)| path)
        .collect()
}

fn tokenize_scope_text(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 4)
        .map(|token| token.to_string())
        .collect()
}

fn get_changed_targets(opts: &ContextOptions, snapshot: &Snapshot) -> Vec<String> {
    let root = context_snapshot_root(opts);
    let repo = match git2::Repository::discover(&root) {
        Ok(repo) => repo,
        Err(_) => return Vec::new(),
    };

    let mut changed_paths = HashSet::new();
    let mut status_opts = git2::StatusOptions::new();
    status_opts
        .include_untracked(true)
        .recurse_untracked_dirs(true);

    if let Ok(statuses) = repo.statuses(Some(&mut status_opts)) {
        for entry in statuses.iter() {
            if entry.status().is_ignored() {
                continue;
            }
            if let Some(path) = entry.path() {
                changed_paths.insert(path.to_string());
            }
        }
    }

    let mut targets = Vec::new();
    for analysis in &snapshot.files {
        if changed_paths
            .iter()
            .any(|cp| analysis.path.ends_with(cp) || cp.ends_with(&analysis.path))
        {
            targets.push(analysis.path.clone());
        }
    }

    targets.sort();
    targets.dedup();
    targets
}

fn get_task_targets(task: &str, snapshot: &Snapshot) -> Vec<String> {
    // Keep dots so `launch.sh` is one token, not `launch` + discarded `sh`.
    let tokens: Vec<String> = task
        .to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '.'))
        .map(|t| t.trim_matches('.'))
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_string())
        .collect();

    if tokens.is_empty() {
        return Vec::new();
    }

    let mut scores: HashMap<String, usize> = HashMap::new();

    for file in &snapshot.files {
        let mut score = 0;
        let file_lower = file.path.to_lowercase();
        let basename = Path::new(&file.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(file.path.as_str())
            .to_lowercase();

        for token in &tokens {
            if file_lower.contains(token) {
                score += 5;
            }
            for exp in &file.exports {
                if exp.name.to_lowercase().contains(token) {
                    score += 3;
                }
            }
            for imp in &file.imports {
                for sym in &imp.symbols {
                    if sym.name.to_lowercase().contains(token) {
                        score += 1;
                    }
                }
            }
        }
        if tokens.iter().any(|token| token == &basename) {
            score += EXACT_NAME_TASK_BOOST;
        }
        if score > 0 {
            scores.insert(file.path.clone(), score);
        }
    }

    if let Some(facts) = &snapshot.semantic_facts {
        for (symbol_id, tags) in &facts.idiom_tags {
            if let Some((file_path, _)) = symbol_id.split_once("::") {
                let mut score = 0;
                for tag in tags {
                    let name_lower = tag.name.to_lowercase();
                    let reason_lower = tag.reasoning.to_lowercase();
                    for token in &tokens {
                        if name_lower.contains(token) {
                            score += 2;
                        }
                        if reason_lower.contains(token) {
                            score += 1;
                        }
                    }
                }
                if score > 0 {
                    *scores.entry(file_path.to_string()).or_default() += score;
                }
            }
        }
    }

    let mut scored: Vec<(String, usize)> = scores.into_iter().collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    scored
        .into_iter()
        .filter(|(_, s)| *s >= 2)
        .take(5)
        .map(|(p, _)| p)
        .collect()
}

fn merge_structural(dest: &mut StructuralSlice, src: StructuralSlice) {
    dest.files.extend(src.files);
    dest.symbols.extend(src.symbols);
    dest.imports.extend(src.imports);
    dest.consumers.extend(src.consumers);
    dest.entrypoints.extend(src.entrypoints);
}

fn dedup_structural(dest: &mut StructuralSlice) {
    dest.files.sort_by(|a, b| a.path.cmp(&b.path));
    dest.files.dedup_by(|a, b| a.path == b.path);

    dest.symbols
        .sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.name.cmp(&b.name)));
    dest.symbols
        .dedup_by(|a, b| a.file == b.file && a.name == b.name);

    dest.imports.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.source_raw.cmp(&b.source_raw))
    });
    dest.imports
        .dedup_by(|a, b| a.file == b.file && a.source_raw == b.source_raw);

    dest.consumers.sort_by(|a, b| a.file.cmp(&b.file));
    dest.consumers.dedup_by(|a, b| a.file == b.file);

    dest.entrypoints.sort_by(|a, b| a.path.cmp(&b.path));
    dest.entrypoints.dedup_by(|a, b| a.path == b.path);
}

fn merge_runtime(dest: &mut RuntimeSlice, src: RuntimeSlice) {
    dest.idiom_tags.extend(src.idiom_tags);
    dest.dispatch_edges.extend(src.dispatch_edges);
    dest.reachability.extend(src.reachability);
    dest.env_contracts.extend(src.env_contracts);
    dest.tauri_commands.extend(src.tauri_commands);
    dest.tauri_events.extend(src.tauri_events);
    dest.framework_hints.extend(src.framework_hints);
    if dest.inventory_coverage.is_none() {
        dest.inventory_coverage = src.inventory_coverage;
    }
}

fn dedup_runtime(dest: &mut RuntimeSlice) {
    dest.idiom_tags
        .sort_by(|a, b| a.symbol.cmp(&b.symbol).then_with(|| a.name.cmp(&b.name)));
    dest.idiom_tags
        .dedup_by(|a, b| a.symbol == b.symbol && a.name == b.name);

    dest.dispatch_edges.sort_by(|a, b| {
        a.from_file
            .cmp(&b.from_file)
            .then_with(|| a.handler_symbol.cmp(&b.handler_symbol))
    });
    dest.dispatch_edges
        .dedup_by(|a, b| a.from_file == b.from_file && a.handler_symbol == b.handler_symbol);

    dest.reachability.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    dest.reachability.dedup_by(|a, b| a.symbol == b.symbol);

    dest.env_contracts.sort_by(|a, b| a.name.cmp(&b.name));
    let mut merged_env: Vec<RuntimeEnvContract> = Vec::with_capacity(dest.env_contracts.len());
    for mut contract in dest.env_contracts.drain(..) {
        if let Some(existing) = merged_env
            .last_mut()
            .filter(|item| item.name == contract.name)
        {
            existing.used_in_files.append(&mut contract.used_in_files);
            existing.required_for.append(&mut contract.required_for);
            existing.occurrences.append(&mut contract.occurrences);
            existing.required |= contract.required;
            if contract.authority == AuthorityLabel::RepoVerified {
                existing.authority = AuthorityLabel::RepoVerified;
            }
        } else {
            merged_env.push(contract);
        }
    }
    for contract in &mut merged_env {
        contract.used_in_files.sort();
        contract.used_in_files.dedup();
        contract.required_for.sort();
        contract.required_for.dedup();
        contract.occurrences.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.access_kind.cmp(&b.access_kind))
        });
        contract.occurrences.dedup_by(|a, b| {
            a.file == b.file && a.line == b.line && a.access_kind == b.access_kind
        });
    }
    dest.env_contracts = merged_env;

    dest.tauri_commands.sort_by(|a, b| a.name.cmp(&b.name));
    dest.tauri_commands.dedup_by(|a, b| a.name == b.name);

    dest.tauri_events.sort_by(|a, b| a.name.cmp(&b.name));
    dest.tauri_events.dedup_by(|a, b| a.name == b.name);

    dest.framework_hints
        .sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.symbol.cmp(&b.symbol)));
    dest.framework_hints
        .dedup_by(|a, b| a.file == b.file && a.symbol == b.symbol && a.kind == b.kind);
}

fn dedup_authority(authority: &mut AuthoritySlice) {
    for claims in [
        &mut authority.repo_verified,
        &mut authority.loctree_derived,
        &mut authority.aicx_operator,
        &mut authority.aicx_agent,
        &mut authority.aicx_failure,
        &mut authority.semantic_guess,
        &mut authority.stale_or_unknown,
    ] {
        claims.sort();
        claims.dedup();
    }
}

fn merge_risk(dest: &mut RiskSlice, src: RiskSlice) {
    dest.hotspots.extend(src.hotspots);
    dest.high_fan_in.extend(src.high_fan_in);
    if dest.snapshot_health.is_none() {
        dest.snapshot_health = src.snapshot_health;
    }
    dest.cache_scope = src.cache_scope;
    dest.cache_scope_authority = src.cache_scope_authority;
    dest.stale_snapshot = dest.stale_snapshot || src.stale_snapshot;
    dest.dirty_worktree = dest.dirty_worktree || src.dirty_worktree;
    dest.git_unknown = dest.git_unknown || src.git_unknown;
}

fn dedup_risk(dest: &mut RiskSlice) {
    dest.hotspots.sort_by(|a, b| {
        b.importers
            .cmp(&a.importers)
            .then_with(|| a.file.cmp(&b.file))
    });
    dest.hotspots.dedup_by(|a, b| a.file == b.file);

    dest.high_fan_in.sort_by(|a, b| {
        b.importers
            .cmp(&a.importers)
            .then_with(|| a.file.cmp(&b.file))
    });
    dest.high_fan_in.dedup_by(|a, b| a.file == b.file);
}

fn merge_action(dest: &mut ActionSlice, src: ActionSlice) {
    dest.next_safe_commands.extend(src.next_safe_commands);
    dest.verification_gates.extend(src.verification_gates);
    dest.likely_tests.extend(src.likely_tests);
    dest.next_safe_command_authorities
        .extend(src.next_safe_command_authorities);
    dest.verification_gate_authorities
        .extend(src.verification_gate_authorities);
    dest.power_path.extend(src.power_path);
}

fn dedup_action(dest: &mut ActionSlice) {
    stable_dedup_strings(&mut dest.next_safe_commands);
    dest.next_safe_commands.truncate(3);
    dest.verification_gates.sort();
    dest.verification_gates.dedup();
    dest.likely_tests.sort();
    dest.likely_tests.dedup();
    dest.likely_tests.truncate(likely_tests_limit());
    dedup_action_authority_claims(&mut dest.next_safe_command_authorities);
    dedup_action_authority_claims(&mut dest.verification_gate_authorities);
    let mut seen = HashSet::new();
    dest.power_path
        .retain(|cmd| seen.insert(cmd.command.clone()));
}

fn dedup_action_authority_claims(claims: &mut Vec<ActionAuthorityClaim>) {
    let mut seen = HashSet::new();
    claims.retain(|claim| seen.insert(claim.item.clone()));
}

fn stable_dedup_strings(items: &mut Vec<String>) {
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert(item.clone()));
}

fn merge_memory(dest: &mut MemorySlice, src: MemorySlice) {
    dest.entries.extend(src.entries);
    dest.source_chunks.extend(src.source_chunks);
    // Carry the most informative diagnostic forward — multi-target overlays
    // produce one diagnostic per target, but the operator only needs the
    // outcome at the slice level. Prefer:
    //   - any `Ok` diagnostic (overlay actually returned something for a target)
    //   - then any `engaged=true` diagnostic over a disabled one
    //   - then the most recently merged one
    if let Some(src_diag) = src.diagnostic {
        let take = match (&dest.diagnostic, &src_diag.skip_reason) {
            (None, _) => true,
            (Some(_), MemorySkipReason::Ok) => true,
            (Some(d), _) if !d.engaged && src_diag.engaged => true,
            _ => false,
        };
        if take {
            dest.diagnostic = Some(src_diag);
        }
    }
    if dest.overlay.is_none() {
        dest.overlay = src.overlay;
    }
}

fn dedup_memory(dest: &mut MemorySlice) {
    // Sort by relevance desc, then date desc, then session_id asc for a stable
    // tie-break. De-dup on (session_id, kind, text) so the same intent emitted
    // twice (once per target file) collapses to a single row.
    dest.entries.sort_by(|a, b| {
        b.relevance
            .cmp(&a.relevance)
            .then_with(|| b.date.cmp(&a.date))
            .then_with(|| b.timestamp.cmp(&a.timestamp))
            .then_with(|| a.session_id.cmp(&b.session_id))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.text.cmp(&b.text))
    });
    dest.entries
        .dedup_by(|a, b| a.session_id == b.session_id && a.kind == b.kind && a.text == b.text);

    // Multi-target runs (`--changed`, `--task`) compose per-target memory
    // slices and merge them. Each per-target slice is already capped at
    // `LOCT_CONTEXT_MEMORY_LIMIT`, but the union after dedup may still
    // exceed the operator's preferred ceiling — re-truncate here and
    // rebuild `source_chunks` from the surviving entries so the two stay
    // consistent.
    let limit = memory_limit();
    if dest.entries.len() > limit {
        dest.entries.truncate(limit);
    }
    let mut chunks: HashSet<String> = HashSet::new();
    for entry in &dest.entries {
        chunks.insert(entry.source_chunk.clone());
    }
    let mut sorted: Vec<String> = chunks.into_iter().collect();
    sorted.sort();
    dest.source_chunks = sorted;
}

/// Append memory-slice claim IDs to the top-level [`AuthoritySlice`].
///
/// Claim IDs are hierarchical (`memory.entries.<session_id>::<kind>`) so an
/// agent can grep one provenance channel and see every memory-derived fact
/// bucketed by its AICX authority bucket.
fn merge_memory_into_authority(memory: &MemorySlice, authority: &mut AuthoritySlice) {
    for entry in &memory.entries {
        push_authority_claim(
            &format!("memory.entries.{}::{}", entry.session_id, entry.kind),
            entry.authority,
            authority,
        );
    }
}

/// Render a ContextPack as the operator-friendly markdown pill.
///
/// This is the library renderer used by the CLI and non-CLI consumers such as
/// MCP. Keep CLI-only status lines and stderr advice outside this function.
pub fn format_context_pack_markdown(pack: &ContextPack) -> String {
    let mut md = String::new();
    md.push_str(
        "# Loctree Context Pack

",
    );

    md.push_str(
        "## Project Identity

",
    );
    if let Some(root) = &pack.project.canonical_root {
        md.push_str(&format!(
            "- **Root**: `{}`
",
            root
        ));
    }
    if let Some(branch) = &pack.project.branch {
        md.push_str(&format!(
            "- **Branch**: `{}`
",
            branch
        ));
    }
    if let Some(commit) = &pack.project.commit {
        md.push_str(&format!(
            "- **Commit**: `{}`
",
            commit
        ));
    }
    if let Some(snapshot_id) = &pack.project.snapshot_id {
        md.push_str(&format!(
            "- **Snapshot**: `{}`
",
            snapshot_id
        ));
    }
    md.push_str(&format!(
        "- **Coverage**: {} of {} targets selected{}\n",
        pack.coverage.selected,
        pack.coverage.total,
        if pack.coverage.truncated {
            " (truncated — not the whole repo)"
        } else {
            ""
        }
    ));
    md.push('\n');

    md.push_str(
        "## Receipt (loctree.receipt.v1)

",
    );
    md.push_str(&format!(
        "- **Authority**: `{:?}`
",
        pack.receipt.authority
    ));
    if let Some(head) = &pack.receipt.head_full {
        md.push_str(&format!(
            "- **Live HEAD**: `{}`
",
            head
        ));
    }
    md.push_str(&format!(
        "- **Dirty**: `{}`
",
        pack.receipt
            .dirty_fingerprint
            .as_deref()
            .unwrap_or("unknown")
    ));
    if let Some(fingerprint) = &pack.receipt.snapshot_fingerprint {
        md.push_str(&format!(
            "- **Snapshot FP**: `{}`
",
            fingerprint
        ));
    }
    md.push_str(&format!(
        "- **Binary**: `{}`
",
        pack.receipt.binary_id
    ));
    md.push_str(&format!(
        "- **Redactions**: {}
",
        pack.receipt.redactions
    ));
    for diagnostic in &pack.receipt.diagnostics {
        md.push_str(&format!(
            "- **Diagnostic**: {}
",
            diagnostic
        ));
    }
    md.push('\n');

    // W6.4 / loctree-fail.md 3352: synthesis-first for --full so Risk/Action survive
    // read-truncation (Read tool ~450 lines, MCP token caps). Pill is already
    // synthesis-first; --full was enumeration wall first. Move Risk+Action here
    // (after identity, before bulk Files/Symbols/Imports/Consumers tables).
    // Full enumeration follows as drill-down.
    md.push_str(
        "## Risk Slice (synthesis-first — survives truncation in --full)

",
    );
    md.push_str(&format!(
        "- Cache Scope: `{:?}` *{:?}*
",
        pack.risk.cache_scope, pack.risk.cache_scope_authority
    ));
    if let Some(health) = &pack.risk.snapshot_health {
        md.push_str(&format!(
            "- Snapshot Health: `{}`
",
            health
        ));
    }
    md.push_str(&format!(
        "- Stale Snapshot: {}
",
        pack.risk.stale_snapshot
    ));
    md.push_str(&format!(
        "- Dirty Worktree: {}

",
        pack.risk.dirty_worktree
    ));

    if !pack.risk.hotspots.is_empty() {
        md.push_str(
            "### Hotspots

",
        );
        md.push_str(
            "| File | Importers | Authority |
",
        );
        md.push_str(
            "|---|---|---|
",
        );
        for hotspot in &pack.risk.hotspots {
            md.push_str(&format!(
                "| `{}` | {} | *{:?}* |
",
                hotspot.file, hotspot.importers, hotspot.authority
            ));
        }
        md.push('\n');
    }

    if !pack.risk.high_fan_in.is_empty() {
        md.push_str(
            "### High Fan-In

",
        );
        md.push_str(
            "| File | Importers | Authority |
",
        );
        md.push_str(
            "|---|---|---|
",
        );
        for hfi in &pack.risk.high_fan_in {
            md.push_str(&format!(
                "| `{}` | {} | *{:?}* |
",
                hfi.file, hfi.importers, hfi.authority
            ));
        }
        md.push('\n');
    }

    md.push_str(
        "## Action Slice (synthesis-first — survives truncation in --full)

",
    );
    if !pack.action.power_path.is_empty() {
        md.push_str(
            "### Power Path

",
        );
        for sug in &pack.action.power_path {
            md.push_str(&format!(
                "- `{}`: {}
",
                sug.command, sug.reason
            ));
        }
        md.push('\n');
    }
    if !pack.action.next_safe_commands.is_empty() {
        md.push_str(
            "### Next Safe Commands

```bash
",
        );
        for cmd in &pack.action.next_safe_commands {
            md.push_str(&format!(
                "{}
",
                cmd
            ));
        }
        md.push_str(
            "```

",
        );
    }
    if !pack.action.verification_gates.is_empty() {
        md.push_str(
            "### Verification Gates

```bash
",
        );
        for gate in &pack.action.verification_gates {
            md.push_str(&format!(
                "{}
",
                gate
            ));
        }
        md.push_str(
            "```

",
        );
    }
    if !pack.action.likely_tests.is_empty() {
        md.push_str(
            "### Likely Tests

",
        );
        for t in &pack.action.likely_tests {
            md.push_str(&format!("- `{}`\n", t));
        }
        md.push('\n');
    }

    md.push_str(
        "## Where You Are (detailed enumeration — after synthesis for truncation safety)

",
    );
    if !pack.structural.files.is_empty() {
        md.push_str(
            "### Files

",
        );
        md.push_str(
            "| Path | Role | Language | LOC | Authority |
",
        );
        md.push_str(
            "|---|---|---|---|---|
",
        );
        for file in &pack.structural.files {
            md.push_str(&format!(
                "| `{}` | `{:?}` | {} | {} | *{:?}* |
",
                file.path, file.role, file.language, file.loc, file.authority
            ));
        }
        md.push('\n');
    }

    if !pack.structural.symbols.is_empty() {
        md.push_str(
            "### Symbols

",
        );
        md.push_str(
            "| File | Name | Kind | Export Type | Line | Authority |
",
        );
        md.push_str(
            "|---|---|---|---|---|---|
",
        );
        for sym in &pack.structural.symbols {
            let line = sym.line.map(|l| l.to_string()).unwrap_or_default();
            md.push_str(&format!(
                "| `{}` | `{}` | {} | {} | {} | *{:?}* |
",
                sym.file, sym.name, sym.kind, sym.export_type, line, sym.authority
            ));
        }
        md.push('\n');
    }

    if !pack.structural.imports.is_empty() {
        md.push_str(
            "### Imports

",
        );
        md.push_str(
            "| File | Source | Kind | Resolution | Authority |
",
        );
        md.push_str(
            "|---|---|---|---|---|
",
        );
        for imp in &pack.structural.imports {
            md.push_str(&format!(
                "| `{}` | `{}` | {} | {} | *{:?}* |
",
                imp.file, imp.source, imp.kind, imp.resolution, imp.authority
            ));
        }
        md.push('\n');
    }

    if !pack.structural.consumers.is_empty() {
        md.push_str(
            "### Consumers

",
        );
        md.push_str(
            "| File | Import Kind | Imports Used | Authority |
",
        );
        md.push_str(
            "|---|---|---|---|
",
        );
        for cons in &pack.structural.consumers {
            md.push_str(&format!(
                "| `{}` | `{:?}` | {} | *{:?}* |
",
                cons.file,
                cons.import_kind,
                cons.imports_used.join(", "),
                cons.authority
            ));
        }
        md.push('\n');
    }

    if !pack.structural.entrypoints.is_empty() {
        md.push_str(
            "### Entrypoints

",
        );
        md.push_str(
            "| Path | Kinds | Authority |
",
        );
        md.push_str(
            "|---|---|---|
",
        );
        for ep in &pack.structural.entrypoints {
            md.push_str(&format!(
                "| `{}` | {} | *{:?}* |
",
                ep.path,
                ep.kinds.join(", "),
                ep.authority
            ));
        }
        md.push('\n');
    }

    md.push_str(
        "## Runtime Slice

",
    );
    if !pack.runtime.idiom_tags.is_empty() {
        md.push_str(
            "### Idiom Tags

",
        );
        for tag in &pack.runtime.idiom_tags {
            md.push_str(&format!(
                "- **{}** (`{}`): {} (Role: {}, Source: {}) *{:?}*
",
                tag.name, tag.symbol, tag.reasoning, tag.runtime_role, tag.source, tag.authority
            ));
        }
        md.push('\n');
    }
    if !pack.runtime.dispatch_edges.is_empty() {
        md.push_str(
            "### Dispatch Edges

",
        );
        for edge in &pack.runtime.dispatch_edges {
            let hfile = edge.handler_file.as_deref().unwrap_or("unknown");
            md.push_str(&format!(
                "- `{}`:{} -> `{}` (`{}`) *{:?}*
",
                edge.from_file, edge.from_line, edge.handler_symbol, hfile, edge.authority
            ));
        }
        md.push('\n');
    }
    if !pack.runtime.reachability.is_empty() {
        md.push_str(
            "### Reachability

",
        );
        for reach in &pack.runtime.reachability {
            let status = if reach.reached {
                "Reached"
            } else {
                "Unreached"
            };
            md.push_str(&format!(
                "- `{}`: {} ({}) *{:?}*
",
                reach.symbol, status, reach.reason, reach.authority
            ));
        }
        md.push('\n');
    }
    if !pack.runtime.env_contracts.is_empty() {
        md.push_str(
            "### Env Contracts

",
        );
        for env in &pack.runtime.env_contracts {
            md.push_str(&format!(
                "- `{}` used in {} files *{:?}*
",
                env.name,
                env.used_in_files.len(),
                env.authority
            ));
        }
        md.push('\n');
    }
    if !pack.runtime.tauri_commands.is_empty() {
        md.push_str(
            "### Tauri Commands

",
        );
        for cmd in &pack.runtime.tauri_commands {
            md.push_str(&format!(
                "- `{}` (Handler: {:?}:{:?}) *{:?}*
",
                cmd.name, cmd.handler_file, cmd.handler_line, cmd.authority
            ));
        }
        md.push('\n');
    }
    if !pack.runtime.tauri_events.is_empty() {
        md.push_str(
            "### Tauri Events

",
        );
        for ev in &pack.runtime.tauri_events {
            md.push_str(&format!(
                "- `{}` (Emits: {}, Listens: {}) *{:?}*
",
                ev.name, ev.emit_count, ev.listen_count, ev.authority
            ));
        }
        md.push('\n');
    }
    if !pack.runtime.framework_hints.is_empty() {
        md.push_str(
            "### Framework Hints

",
        );
        for hint in &pack.runtime.framework_hints {
            md.push_str(&format!(
                "- `{}` at `{}`: {} *{:?}*
",
                hint.kind, hint.file, hint.symbol, hint.authority
            ));
        }
        md.push('\n');
    }

    // Risk Hotspots / High Fan-In tables are emitted once in the early Risk
    // Slice (synthesis-first, above). Do not re-emit them here — a prior
    // "neutralized" late block still duplicated those tables and a comment
    // falsely claimed "No dupe" (audit ad28ded / pack trust review 2026-07-23).
    md.push_str(
        "## Memory Slice

",
    );
    if pack.memory.entries.is_empty() && pack.memory.source_chunks.is_empty() {
        md.push_str(
            "_Empty_

",
        );
    } else {
        if !pack.memory.entries.is_empty() {
            md.push_str(
                "| Kind | Agent | Date | Relevance | Text | Authority | Source Chunk |
",
            );
            md.push_str(
                "|---|---|---|---|---|---|---|
",
            );
            for entry in &pack.memory.entries {
                let text = summarize_entry(&entry.text).text.replace('|', "\\|");
                // loctree-fail hak 2026-05-23 #2: never leak raw
                // `~/.aicx/store/...` absolute paths into commitable markdown
                // output. Use the same opaque `chunk:<hash>` reference the
                // pill renderer emits.
                let chunk = chunk_ref(&entry.source_chunk);
                md.push_str(&format!(
                    "| `{}` | `{}` | {} | {} | {} | *{:?}* | `{}` |
",
                    entry.kind,
                    entry.agent,
                    entry.date,
                    entry.relevance,
                    text,
                    entry.authority,
                    chunk,
                ));
            }
            md.push('\n');
        }
        if !pack.memory.source_chunks.is_empty() {
            md.push_str(
                "### Source Chunks

",
            );
            md.push_str(&aicx_read_chunk_footer(pack.memory.source_chunks.len()));
            md.push_str("\n\n");
            for chunk in &pack.memory.source_chunks {
                let opaque = chunk_ref(chunk);
                md.push_str(&format!(
                    "- `{}`
",
                    opaque
                ));
            }
            md.push('\n');
        }
    }

    md.push_str(
        "## Authority Slice

",
    );
    md.push_str(&format!(
        "- Repo Verified: {}
",
        pack.authority.repo_verified.len()
    ));
    md.push_str(&format!(
        "- Loctree Derived: {}
",
        pack.authority.loctree_derived.len()
    ));
    md.push_str(&format!(
        "- AICX Operator: {}
",
        pack.authority.aicx_operator.len()
    ));
    md.push_str(&format!(
        "- AICX Agent: {}
",
        pack.authority.aicx_agent.len()
    ));
    md.push_str(&format!(
        "- AICX Failure: {}
",
        pack.authority.aicx_failure.len()
    ));
    md.push_str(&format!(
        "- Semantic Guess: {}
",
        pack.authority.semantic_guess.len()
    ));
    md.push_str(&format!(
        "- Stale Or Unknown: {}
",
        pack.authority.stale_or_unknown.len()
    ));
    md.push('\n');

    md
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{EntrypointSummary, GraphEdge};
    use crate::types::{
        ExportSymbol, FileAnalysis, ImportEntry, ImportSymbol, RouteInfo, SymbolUsage,
    };
    use std::collections::BTreeSet;

    #[test]
    fn context_pack_serde_roundtrip() {
        let pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some("/tmp/loctree".to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc123".to_string()),
            snapshot_id: Some("main@abc123".to_string()),
        });

        let json = serde_json::to_string(&pack).expect("serialize ContextPack");
        let roundtrip: ContextPack = serde_json::from_str(&json).expect("deserialize ContextPack");

        assert_eq!(roundtrip.schema_version, CONTEXT_SCHEMA_VERSION);
        assert_eq!(
            roundtrip.project.canonical_root.as_deref(),
            Some("/tmp/loctree")
        );
        assert!(roundtrip.structural.files.is_empty());
        assert!(roundtrip.runtime.idiom_tags.is_empty());
        assert!(roundtrip.runtime.tauri_commands.is_empty());
        assert!(roundtrip.runtime.framework_hints.is_empty());
        assert!(roundtrip.risk.hotspots.is_empty());
        assert!(roundtrip.action.next_safe_commands.is_empty());
        assert!(roundtrip.memory.entries.is_empty());
        assert!(roundtrip.memory.source_chunks.is_empty());
        assert!(roundtrip.authority.repo_verified.is_empty());
    }

    fn target_centric_snapshot() -> Snapshot {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);

        let mut target = FileAnalysis::new("src/lib.rs".to_string());
        target.loc = 120;
        target.language = "rust".to_string();
        target.exports.push(ExportSymbol::new(
            "foo".to_string(),
            "function",
            "named",
            Some(10),
        ));
        target.exports.push(ExportSymbol::new(
            "Bar".to_string(),
            "struct",
            "named",
            Some(42),
        ));

        let mut target_import = ImportEntry::new("src/utils.rs".to_string(), ImportKind::Static);
        target_import.line = Some(3);
        target_import.resolved_path = Some("src/utils.rs".to_string());
        target_import.resolution = ImportResolutionKind::Local;
        target_import.symbols.push(ImportSymbol {
            name: "helper".to_string(),
            alias: None,
            is_default: false,
        });
        target.imports.push(target_import);

        let mut utils = FileAnalysis::new("src/utils.rs".to_string());
        utils.loc = 40;
        utils.language = "rust".to_string();
        utils.exports.push(ExportSymbol::new(
            "helper".to_string(),
            "function",
            "named",
            Some(5),
        ));

        let mut consumer = FileAnalysis::new("src/main.rs".to_string());
        consumer.loc = 60;
        consumer.language = "rust".to_string();
        let mut consumer_import = ImportEntry::new("src/lib.rs".to_string(), ImportKind::Static);
        consumer_import.line = Some(1);
        consumer_import.resolved_path = Some("src/lib.rs".to_string());
        consumer_import.resolution = ImportResolutionKind::Local;
        consumer_import.symbols.push(ImportSymbol {
            name: "foo".to_string(),
            alias: None,
            is_default: false,
        });
        consumer.imports.push(consumer_import);

        snapshot.files.push(target);
        snapshot.files.push(utils);
        snapshot.files.push(consumer);

        snapshot.edges.push(GraphEdge {
            from: "src/lib.rs".to_string(),
            to: "src/utils.rs".to_string(),
            label: "import".to_string(),
        });
        snapshot.edges.push(GraphEdge {
            from: "src/main.rs".to_string(),
            to: "src/lib.rs".to_string(),
            label: "import".to_string(),
        });

        snapshot.metadata.entrypoints.push(EntrypointSummary {
            path: "src/main.rs".to_string(),
            kinds: vec!["bin".to_string()],
        });

        snapshot
    }

    #[test]
    fn compose_structural_slice_with_file_scope() {
        let snapshot = target_centric_snapshot();
        let opts = ContextOptions {
            file: Some(PathBuf::from("src/lib.rs")),
            ..ContextOptions::default()
        };

        let slice = compose_structural_slice(&opts, &snapshot);

        assert_eq!(slice.files.len(), 3);
        let target_role = slice
            .files
            .iter()
            .find(|f| f.path == "src/lib.rs")
            .expect("target file present");
        assert_eq!(target_role.role, StructuralRole::Target);
        assert_eq!(target_role.authority, AuthorityLabel::RepoVerified);
        assert!(
            slice
                .files
                .iter()
                .any(|f| f.path == "src/utils.rs" && f.role == StructuralRole::Dependency)
        );
        assert!(
            slice
                .files
                .iter()
                .any(|f| f.path == "src/main.rs" && f.role == StructuralRole::Consumer)
        );

        let symbol_names: HashSet<&str> = slice.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(symbol_names.contains("foo"));
        assert!(symbol_names.contains("Bar"));
        for sym in &slice.symbols {
            assert_eq!(sym.file, "src/lib.rs");
            assert!(!sym.kind.is_empty());
            assert!(sym.line.is_some());
            assert_eq!(sym.authority, AuthorityLabel::RepoVerified);
        }

        assert_eq!(slice.entrypoints.len(), 1);
        assert_eq!(slice.entrypoints[0].path, "src/main.rs");
        assert_eq!(slice.entrypoints[0].kinds, vec!["bin".to_string()]);
    }

    #[test]
    fn compose_structural_slice_empty_when_no_scope() {
        let snapshot = target_centric_snapshot();
        let opts = ContextOptions::default();

        let slice = compose_structural_slice(&opts, &snapshot);

        assert!(slice.files.is_empty());
        assert!(slice.symbols.is_empty());
        assert!(slice.imports.is_empty());
        assert!(slice.consumers.is_empty());
        assert!(slice.entrypoints.is_empty());
    }

    #[test]
    fn compose_structural_slice_includes_consumers_and_imports() {
        let snapshot = target_centric_snapshot();
        let opts = ContextOptions {
            file: Some(PathBuf::from("src/lib.rs")),
            ..ContextOptions::default()
        };

        let slice = compose_structural_slice(&opts, &snapshot);

        assert_eq!(slice.imports.len(), 1, "target's static import surfaces");
        let only_import = &slice.imports[0];
        assert_eq!(only_import.file, "src/lib.rs");
        assert_eq!(only_import.source, "src/utils.rs");
        assert_eq!(only_import.kind, "static");
        assert_eq!(only_import.resolution, "local");
        assert_eq!(only_import.symbols, vec!["helper".to_string()]);
        assert_eq!(only_import.authority, AuthorityLabel::RepoVerified);

        assert_eq!(slice.consumers.len(), 1);
        let consumer = &slice.consumers[0];
        assert_eq!(consumer.file, "src/main.rs");
        assert_eq!(consumer.import_kind, ConsumerKind::Direct);
        assert_eq!(consumer.imports_used, vec!["foo".to_string()]);
        assert_eq!(consumer.authority, AuthorityLabel::RepoVerified);
    }

    /// A2 regression fixture — a Python barrel package (`pkg/__init__.py`
    /// re-exporting `pkg/core.py`) consumed by three importer modules, so the
    /// barrel is a hub the risk hotspots can prove from the edge graph alone.
    ///
    /// Deliberately seeds a leaked empty-path `FileAnalysis` with a large LOC:
    /// that is the historical trigger of the empty-structural-card asymmetry
    /// (an empty default target reaching the slicer, whose `ends_with("")`
    /// suffix matcher then matched every file → "Ambiguous slice target" → a
    /// `None` slice → `StructuralSlice::default()`, while risk kept reading the
    /// edge graph directly). The empty file is NOT a node in the edge graph, so
    /// the `⊆ edge-graph nodes` assertion below also proves it never leaks into
    /// the structural slice.
    fn python_hub_snapshot() -> Snapshot {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);

        let mut core = FileAnalysis::new("pkg/core.py".to_string());
        core.loc = 24;
        core.language = "py".to_string();
        core.exports.push(ExportSymbol::new(
            "Engine".to_string(),
            "class",
            "named",
            Some(1),
        ));
        core.exports.push(ExportSymbol::new(
            "run".to_string(),
            "def",
            "named",
            Some(8),
        ));

        let mut barrel = FileAnalysis::new("pkg/__init__.py".to_string());
        barrel.loc = 3;
        barrel.language = "py".to_string();
        barrel.exports.push(ExportSymbol::new(
            "Engine".to_string(),
            "reexport",
            "named",
            Some(1),
        ));
        barrel.exports.push(ExportSymbol::new(
            "run".to_string(),
            "reexport",
            "named",
            Some(1),
        ));
        let mut barrel_import = ImportEntry::new(".core".to_string(), ImportKind::Static);
        barrel_import.line = Some(1);
        barrel_import.resolved_path = Some("pkg/core.py".to_string());
        barrel_import.resolution = ImportResolutionKind::Local;
        barrel_import.symbols.push(ImportSymbol {
            name: "Engine".to_string(),
            alias: None,
            is_default: false,
        });
        barrel.imports.push(barrel_import);

        snapshot.files.push(core);
        snapshot.files.push(barrel);

        // Three importer modules → `pkg/__init__.py` is a 3-importer hub.
        for tag in ["a", "b", "c"] {
            let path = format!("app_{tag}.py");
            let mut module = FileAnalysis::new(path.clone());
            module.loc = 12;
            module.language = "py".to_string();
            let mut module_import = ImportEntry::new("pkg".to_string(), ImportKind::Static);
            module_import.line = Some(1);
            module_import.resolved_path = Some("pkg/__init__.py".to_string());
            module_import.resolution = ImportResolutionKind::Local;
            module_import.symbols.push(ImportSymbol {
                name: "run".to_string(),
                alias: None,
                is_default: false,
            });
            module.imports.push(module_import);
            snapshot.files.push(module);

            snapshot.edges.push(GraphEdge {
                from: path,
                to: "pkg/__init__.py".to_string(),
                label: "import".to_string(),
            });
        }

        snapshot.edges.push(GraphEdge {
            from: "pkg/__init__.py".to_string(),
            to: "pkg/core.py".to_string(),
            label: "import".to_string(),
        });

        // Historical A2 trigger: a high-LOC file with an empty path. Before the
        // 2026-06-26 `retain_context_targets` fix it would rank into the default
        // scope and blank the whole structural slice.
        let mut leaked = FileAnalysis::new(String::new());
        leaked.loc = 999;
        snapshot.files.push(leaked);

        snapshot
    }

    /// A2 acceptance — for a Python project whose hub data is non-empty, the
    /// bare default-scope context pack must emit a `structural` slice whose
    /// `files` and `symbols` are non-empty AND drawn only from the same edge
    /// graph the risk hotspots are proven from. Locks the empty-card asymmetry
    /// closed: structural must quote the graph the hotspots already prove.
    #[test]
    fn python_structural_emits_from_edge_graph() {
        let snapshot = python_hub_snapshot();
        let opts = ContextOptions {
            project: Some(PathBuf::from(".")),
            no_aicx: true,
            ..ContextOptions::default()
        };

        let pack = compose_context_pack_from_snapshot(&opts, Path::new("."), &snapshot)
            .expect("compose context pack");

        // The hub data risk derives straight from the edge graph is populated…
        assert!(
            !pack.risk.hotspots.is_empty(),
            "risk hotspots must be populated for a Python hub snapshot"
        );
        assert!(
            pack.risk
                .hotspots
                .iter()
                .any(|h| h.file == "pkg/__init__.py"),
            "the barrel hub must surface as a hotspot: {:?}",
            pack.risk.hotspots
        );

        // …and the structural slice quotes that same graph instead of going empty.
        assert!(
            !pack.structural.files.is_empty(),
            "structural files must be non-empty when hub/hotspot data is present"
        );
        assert!(
            !pack.structural.symbols.is_empty(),
            "structural symbols must be non-empty when hub/hotspot data is present"
        );

        // Every structural file must be a real node in the edge graph — this
        // both proves consistency with the hotspots and proves the leaked
        // empty-path file never leaks into the slice.
        let edge_nodes: HashSet<&str> = snapshot
            .edges
            .iter()
            .flat_map(|e| [e.from.as_str(), e.to.as_str()])
            .collect();
        for file in &pack.structural.files {
            assert!(
                !file.path.trim().is_empty(),
                "structural slice must never carry an empty-path file"
            );
            assert!(
                edge_nodes.contains(file.path.as_str()),
                "structural file {:?} is not a node in the edge graph {:?}",
                file.path,
                edge_nodes
            );
        }

        // Symbols must be anchored on files the structural slice actually carries.
        let structural_files: HashSet<&str> = pack
            .structural
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        for sym in &pack.structural.symbols {
            assert!(
                structural_files.contains(sym.file.as_str()),
                "structural symbol {:?} anchored on out-of-scope file {:?}",
                sym.name,
                sym.file
            );
        }
    }

    // ---------------------------------------------------------------------
    // Cut 4 T2 — runtime slice tests
    // ---------------------------------------------------------------------

    fn shell_idiom_tag(name: &str, classifier: Classifier, role: RuntimeRole) -> IdiomTag {
        IdiomTag {
            name: name.to_string(),
            classifier,
            runtime_role: role,
            source: TagSource::EmbeddedDefault,
            reasoning: format!("shell embedded idiom: {name}"),
        }
    }

    /// Acceptance #1 — shell idiom tags surface for usage / die / main.
    #[test]
    fn compose_runtime_slice_with_shell_idioms() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        let target_path = "scripts/synthetic_dispatch.sh".to_string();

        let mut shell_file = FileAnalysis::new(target_path.clone());
        shell_file.language = "shell".to_string();
        snapshot.files.push(shell_file);

        let mut facts = SemanticFacts::default();
        for (name, classifier, role) in [
            ("usage", Classifier::HelpPrinter, RuntimeRole::UserFacing),
            ("die", Classifier::ErrorExit, RuntimeRole::LibraryHelper),
            (
                "main",
                Classifier::PrimaryEntrypoint,
                RuntimeRole::PrimaryEntrypoint,
            ),
        ] {
            facts.idiom_tags.insert(
                format!("{target_path}::{name}"),
                vec![shell_idiom_tag(name, classifier, role)],
            );
        }
        facts.dispatch_edges.push(DispatchEdge {
            from_file: target_path.clone(),
            from_line: 42,
            dispatch_kind: DispatchKind::CaseStatement,
            handler_symbol: "deploy_impl".to_string(),
            handler_file: Some(target_path.clone()),
        });
        snapshot.semantic_facts = Some(facts);

        let opts = ContextOptions {
            file: Some(PathBuf::from(&target_path)),
            ..ContextOptions::default()
        };
        let slice = compose_runtime_slice(&opts, &snapshot);

        assert!(
            slice.idiom_tags.len() >= 3,
            "expected ≥3 idiom tags, got {}",
            slice.idiom_tags.len()
        );
        let names: HashSet<&str> = slice.idiom_tags.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains("usage"));
        assert!(names.contains("die"));
        assert!(names.contains("main"));

        for tag in &slice.idiom_tags {
            assert_eq!(tag.authority, AuthorityLabel::LoctreeDerived);
            assert_eq!(tag.source, "embedded_default");
        }

        assert_eq!(slice.dispatch_edges.len(), 1);
        assert_eq!(slice.dispatch_edges[0].dispatch_kind, "case_statement");
        assert_eq!(
            slice.dispatch_edges[0].authority,
            AuthorityLabel::LoctreeDerived
        );

        assert!(slice.tauri_commands.is_empty());
        assert!(slice.tauri_events.is_empty());
        assert!(slice.env_contracts.is_empty());
    }

    /// Acceptance #2 — Python decorator dispatch surfaces as framework_hints.
    ///
    /// Updated for the dedup-on-specific-kind rule: when a `<framework>_route`
    /// hint already exists for the same `(file, line, symbol)`, the generic
    /// `python_decorator` mirror is suppressed. This trims the framework_hints
    /// payload by ~50% on real FastAPI/Flask projects without losing any
    /// information.
    #[test]
    fn compose_runtime_slice_with_python_decorators() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        let target_path = "app/main.py".to_string();

        let mut py_file = FileAnalysis::new(target_path.clone());
        py_file.language = "python".to_string();
        py_file.routes.push(RouteInfo {
            framework: "fastapi".to_string(),
            method: "GET".to_string(),
            path: Some("/users".to_string()),
            name: Some("list_users".to_string()),
            line: 17,
        });
        snapshot.files.push(py_file);

        let mut facts = SemanticFacts::default();
        facts.dispatch_edges.push(DispatchEdge {
            from_file: target_path.clone(),
            from_line: 17,
            dispatch_kind: DispatchKind::HttpRoute,
            handler_symbol: "list_users".to_string(),
            handler_file: Some(target_path.clone()),
        });
        // Also include an unrecognised callback edge — that one *should* still
        // surface as `python_decorator` because no specific hint exists.
        facts.dispatch_edges.push(DispatchEdge {
            from_file: target_path.clone(),
            from_line: 99,
            dispatch_kind: DispatchKind::FunctionPointer,
            handler_symbol: "_my_callback".to_string(),
            handler_file: Some(target_path.clone()),
        });
        let symbol_id = format!("{target_path}::list_users");
        facts.reachability.reached_symbols.insert(symbol_id.clone());
        facts
            .reachability
            .reasons
            .insert(symbol_id.clone(), ReachReason::DirectImport);
        snapshot.semantic_facts = Some(facts);

        let opts = ContextOptions {
            file: Some(PathBuf::from(&target_path)),
            ..ContextOptions::default()
        };
        let slice = compose_runtime_slice(&opts, &snapshot);

        assert!(
            !slice.framework_hints.is_empty(),
            "framework_hints empty: {slice:#?}"
        );
        let kinds: HashSet<&str> = slice
            .framework_hints
            .iter()
            .map(|h| h.kind.as_str())
            .collect();
        assert!(
            kinds.contains("fastapi_route"),
            "missing fastapi_route: {kinds:?}"
        );
        assert!(
            kinds.contains("python_decorator"),
            "missing python_decorator (for unrecognised callback): {kinds:?}"
        );

        let route_hint = slice
            .framework_hints
            .iter()
            .find(|h| h.kind == "fastapi_route")
            .expect("route hint");
        assert_eq!(route_hint.authority, AuthorityLabel::RepoVerified);
        assert_eq!(route_hint.detail.as_deref(), Some("GET /users"));

        // The route handler must NOT also carry a `python_decorator` mirror —
        // the dedup rule suppresses generic taxons when a specific one exists
        // at the same (file, line, symbol).
        let dup_decorator = slice
            .framework_hints
            .iter()
            .find(|h| h.kind == "python_decorator" && h.symbol == "list_users");
        assert!(
            dup_decorator.is_none(),
            "fastapi_route handler should not also have python_decorator mirror: {slice:#?}"
        );

        // The unrecognised callback edge IS surfaced as `python_decorator`.
        let decorator_hint = slice
            .framework_hints
            .iter()
            .find(|h| h.kind == "python_decorator")
            .expect("decorator hint for callback");
        assert_eq!(decorator_hint.authority, AuthorityLabel::LoctreeDerived);
        assert_eq!(decorator_hint.symbol, "_my_callback");

        // The HttpRoute edge should expose its method/path/framework metadata.
        let route_edge = slice
            .dispatch_edges
            .iter()
            .find(|e| e.dispatch_kind == "http_route")
            .expect("http_route dispatch edge");
        assert_eq!(route_edge.framework.as_deref(), Some("fastapi"));
        assert_eq!(route_edge.http_method.as_deref(), Some("GET"));
        assert_eq!(route_edge.http_path.as_deref(), Some("/users"));

        let reach = slice
            .reachability
            .iter()
            .find(|r| r.symbol == symbol_id)
            .expect("reach claim");
        assert!(reach.reached);
        assert_eq!(reach.reason, "direct_import");
        assert_eq!(reach.authority, AuthorityLabel::RepoVerified);
    }

    #[test]
    fn compose_runtime_slice_recognizes_swiftui_app_and_swiftpm_executable() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        let app_path = "Pensieve/Sources/Pensieve/App/PensieveApp.swift";

        let mut app = FileAnalysis::new(app_path.to_string());
        app.language = "swift".to_string();
        app.entry_points.push("swift-main".to_string());
        app.imports
            .push(ImportEntry::new("SwiftUI".to_string(), ImportKind::Static));
        snapshot.files.push(app);

        let mut manifest = FileAnalysis::new("Pensieve/Package.swift".to_string());
        manifest.language = "swift".to_string();
        manifest.imports.push(ImportEntry::new(
            "PackageDescription".to_string(),
            ImportKind::Static,
        ));
        manifest.symbol_usages.push(SymbolUsage {
            name: "executable".to_string(),
            line: 18,
            context: ".executable(name: \"Pensieve\", targets: [\"Pensieve\"])".to_string(),
        });
        snapshot.files.push(manifest);

        let opts = ContextOptions {
            file: Some(PathBuf::from(app_path)),
            ..ContextOptions::default()
        };
        let slice = compose_runtime_slice(&opts, &snapshot);

        let kinds: HashSet<&str> = slice
            .framework_hints
            .iter()
            .map(|hint| hint.kind.as_str())
            .collect();
        assert!(kinds.contains("entrypoint"), "{slice:#?}");
        assert!(kinds.contains("swiftui_app"), "{slice:#?}");
        assert!(kinds.contains("swiftpm_executable_target"), "{slice:#?}");

        let swiftpm = slice
            .framework_hints
            .iter()
            .find(|hint| hint.kind == "swiftpm_executable_target")
            .expect("SwiftPM executable hint");
        assert_eq!(swiftpm.symbol, "Pensieve");
        assert_eq!(swiftpm.file, "Pensieve/Package.swift");
        assert_eq!(swiftpm.line, Some(18));
        assert!(
            swiftpm
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains(app_path)),
            "{swiftpm:#?}"
        );
    }

    #[test]
    fn repo_runtime_inventory_surfaces_env_names_and_executable_owners() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/runtime_inventory");
        let mut snapshot = Snapshot::new(vec![".".to_string()]);

        let mut rust_main = FileAnalysis::new("src/main.rs".to_string());
        rust_main.language = "rust".to_string();
        rust_main.entry_points.push("rust-main".to_string());
        snapshot.files.push(rust_main);

        let mut cargo = FileAnalysis::new("Cargo.toml".to_string());
        cargo.language = "toml".to_string();
        snapshot.files.push(cargo);

        let mut swift_app = FileAnalysis::new("Sources/RuntimeFixture/App.swift".to_string());
        swift_app.language = "swift".to_string();
        swift_app.entry_points.push("swift-main".to_string());
        swift_app
            .imports
            .push(ImportEntry::new("SwiftUI".to_string(), ImportKind::Static));
        snapshot.files.push(swift_app);

        let mut package = FileAnalysis::new("Package.swift".to_string());
        package.language = "swift".to_string();
        package.imports.push(ImportEntry::new(
            "PackageDescription".to_string(),
            ImportKind::Static,
        ));
        package.symbol_usages.push(SymbolUsage {
            name: "executable".to_string(),
            line: 5,
            context: ".executable(name: \"RuntimeFixture\", targets: [\"RuntimeFixture\"])"
                .to_string(),
        });
        snapshot.files.push(package);
        snapshot.metadata.entrypoints = vec![
            EntrypointSummary {
                path: "src/main.rs".to_string(),
                kinds: vec!["rust-main".to_string()],
            },
            EntrypointSummary {
                path: "Sources/RuntimeFixture/App.swift".to_string(),
                kinds: vec!["swift-main".to_string()],
            },
        ];

        let mut runtime = RuntimeSlice::default();
        enrich_repo_runtime_inventory(&fixture, &snapshot, &mut runtime);
        dedup_runtime(&mut runtime);

        let env_names: BTreeSet<&str> = runtime
            .env_contracts
            .iter()
            .map(|contract| contract.name.as_str())
            .collect();
        assert_eq!(
            env_names,
            BTreeSet::from(["FIXTURE_CACHE_ROOT", "FIXTURE_RUNTIME_ROOT"])
        );
        assert!(runtime.env_contracts.iter().all(|contract| {
            contract
                .occurrences
                .iter()
                .all(|occurrence| occurrence.default.is_none())
        }));

        let owner_kinds: BTreeSet<&str> = runtime
            .framework_hints
            .iter()
            .map(|hint| hint.kind.as_str())
            .collect();
        for expected in [
            "cargo_bin",
            "cargo_default_run",
            "cargo_library_crate_type",
            "executable_owner",
            "swiftpm_executable_target",
            "swiftui_app",
        ] {
            assert!(
                owner_kinds.contains(expected),
                "missing {expected}: {runtime:#?}"
            );
        }
        let coverage = runtime.inventory_coverage.expect("inventory coverage");
        assert_eq!(coverage.source_files_scanned, 3);
    }

    /// Acceptance #3 — Tauri command bridges surface in `tauri_commands`.
    #[test]
    fn compose_runtime_slice_with_tauri_command_bridge() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        let backend_path = "src-tauri/src/commands.rs".to_string();
        let frontend_path = "src/api.ts".to_string();

        let mut backend = FileAnalysis::new(backend_path.clone());
        backend.language = "rust".to_string();
        let mut frontend = FileAnalysis::new(frontend_path.clone());
        frontend.language = "typescript".to_string();
        snapshot.files.push(backend);
        snapshot.files.push(frontend);

        snapshot.command_bridges.push(CommandBridge {
            name: "greet_user".to_string(),
            frontend_calls: vec![(frontend_path.clone(), 14)],
            backend_handler: Some((backend_path.clone(), 22)),
            has_handler: true,
            is_called: true,
        });
        snapshot.event_bridges.push(EventBridge {
            name: "user_updated".to_string(),
            emits: vec![(backend_path.clone(), 30, "emit".to_string())],
            listens: vec![(frontend_path.clone(), 8)],
            is_fe_sync: false,
            same_file_sync: false,
        });

        let opts = ContextOptions {
            file: Some(PathBuf::from(&backend_path)),
            ..ContextOptions::default()
        };
        let slice = compose_runtime_slice(&opts, &snapshot);

        assert_eq!(slice.tauri_commands.len(), 1, "{:#?}", slice.tauri_commands);
        let bridge = &slice.tauri_commands[0];
        assert_eq!(bridge.name, "greet_user");
        assert_eq!(bridge.handler_file.as_deref(), Some(backend_path.as_str()));
        assert_eq!(bridge.handler_line, Some(22));
        assert_eq!(bridge.invoke_site_count, 1);
        assert!(bridge.has_handler);
        assert!(bridge.is_called);
        assert_eq!(bridge.authority, AuthorityLabel::RepoVerified);

        assert_eq!(slice.tauri_events.len(), 1);
        assert_eq!(slice.tauri_events[0].name, "user_updated");
        assert_eq!(slice.tauri_events[0].emit_count, 1);
        assert_eq!(slice.tauri_events[0].listen_count, 1);
        assert_eq!(
            slice.tauri_events[0].authority,
            AuthorityLabel::RepoVerified
        );
    }

    /// Acceptance #4 — empty slice when snapshot has no semantic facts AND
    /// the file is bare (no entrypoints / routes / fixtures).
    #[test]
    fn compose_runtime_slice_empty_when_no_semantic_facts() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        let target = "src/lib.rs".to_string();
        let mut file = FileAnalysis::new(target.clone());
        file.language = "rust".to_string();
        snapshot.files.push(file);
        snapshot.semantic_facts = None;

        let opts = ContextOptions {
            file: Some(PathBuf::from(&target)),
            ..ContextOptions::default()
        };
        let slice = compose_runtime_slice(&opts, &snapshot);

        assert!(slice.idiom_tags.is_empty());
        assert!(slice.dispatch_edges.is_empty());
        assert!(slice.reachability.is_empty());
        assert!(slice.env_contracts.is_empty());
        assert!(slice.tauri_commands.is_empty());
        assert!(slice.tauri_events.is_empty());
        assert!(slice.framework_hints.is_empty());

        let json = serde_json::to_string(&slice).expect("serialize empty runtime slice");
        let back: RuntimeSlice = serde_json::from_str(&json).expect("deserialize");
        assert!(back.idiom_tags.is_empty());
    }

    /// Authority refinement spot-check: idiom inferred-from-code → SemanticGuess;
    /// reach IdiomRuntimeRole → SemanticGuess; reach Unknown → StaleOrUnknown;
    /// env contracts → LoctreeDerived.
    #[test]
    fn compose_runtime_slice_refines_authority_labels() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        let target = "src/inferred.sh".to_string();
        let mut file = FileAnalysis::new(target.clone());
        file.language = "shell".to_string();
        snapshot.files.push(file);

        let mut facts = SemanticFacts::default();
        facts.idiom_tags.insert(
            format!("{target}::heuristic_helper"),
            vec![IdiomTag {
                name: "heuristic_helper".to_string(),
                classifier: Classifier::LibraryHelper,
                runtime_role: RuntimeRole::LibraryHelper,
                source: TagSource::InferredFromCode,
                reasoning: "inferred".to_string(),
            }],
        );
        let inferred_id = format!("{target}::heuristic_helper");
        facts
            .reachability
            .reached_symbols
            .insert(inferred_id.clone());
        facts.reachability.reasons.insert(
            inferred_id.clone(),
            ReachReason::IdiomRuntimeRole(RuntimeRole::LibraryHelper),
        );
        let unknown_id = format!("{target}::orphan");
        facts
            .reachability
            .unreached_symbols
            .insert(unknown_id.clone());
        facts
            .reachability
            .reasons
            .insert(unknown_id.clone(), ReachReason::Unknown);
        facts.env_contracts.push(EnvContract {
            name: "LOCT_CACHE_DIR".to_string(),
            used_in_files: vec![target.clone()],
            required_for: vec!["snapshot location override".to_string()],
            occurrences: Vec::new(),
        });
        snapshot.semantic_facts = Some(facts);

        let opts = ContextOptions {
            file: Some(PathBuf::from(&target)),
            ..ContextOptions::default()
        };
        let slice = compose_runtime_slice(&opts, &snapshot);

        assert_eq!(slice.idiom_tags.len(), 1);
        assert_eq!(slice.idiom_tags[0].authority, AuthorityLabel::SemanticGuess);
        assert_eq!(slice.idiom_tags[0].source, "inferred_from_code");

        let inferred_reach = slice
            .reachability
            .iter()
            .find(|r| r.symbol == inferred_id)
            .expect("inferred reach");
        assert_eq!(inferred_reach.authority, AuthorityLabel::SemanticGuess);
        assert!(inferred_reach.reason.starts_with("idiom_runtime_role:"));

        let unknown_reach = slice
            .reachability
            .iter()
            .find(|r| r.symbol == unknown_id)
            .expect("unknown reach");
        assert_eq!(unknown_reach.authority, AuthorityLabel::StaleOrUnknown);
        assert_eq!(unknown_reach.reason, "unknown");

        assert_eq!(slice.env_contracts.len(), 1);
        assert_eq!(
            slice.env_contracts[0].authority,
            AuthorityLabel::LoctreeDerived
        );
    }

    #[test]
    fn compose_runtime_slice_caps_make_phony_targets() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        let target = "Makefile".to_string();
        let mut file = FileAnalysis::new(target.clone());
        file.language = "make".to_string();
        snapshot.files.push(file);

        let mut facts = SemanticFacts::default();
        for index in 0..12 {
            let name = format!("target-{index:02}");
            let symbol_id = format!("{target}::{name}");
            facts.idiom_tags.insert(
                symbol_id.clone(),
                vec![IdiomTag {
                    name: ".PHONY".to_string(),
                    classifier: Classifier::PublicEntrypoint,
                    runtime_role: RuntimeRole::PublicEntrypoint,
                    source: TagSource::InferredFromCode,
                    reasoning: format!("Target '{name}' is listed in a .PHONY directive."),
                }],
            );
            facts.reachability.reached_symbols.insert(symbol_id.clone());
            facts
                .reachability
                .reasons
                .insert(symbol_id, ReachReason::PhonyMakeTarget);
        }
        snapshot.semantic_facts = Some(facts);

        let opts = ContextOptions {
            file: Some(PathBuf::from(&target)),
            ..ContextOptions::default()
        };
        let slice = compose_runtime_slice(&opts, &snapshot);

        let phony_tags = slice
            .idiom_tags
            .iter()
            .filter(|tag| tag.name == ".PHONY")
            .count();
        let phony_reachability = slice
            .reachability
            .iter()
            .filter(|reach| reach.reason == "phony_make_target")
            .count();

        assert_eq!(phony_tags, MAKE_RUNTIME_TARGET_LIMIT);
        assert_eq!(phony_reachability, MAKE_RUNTIME_TARGET_LIMIT);
        assert!(
            slice
                .reachability
                .iter()
                .any(|reach| reach.symbol == "Makefile::target-00")
        );
        assert!(
            !slice
                .reachability
                .iter()
                .any(|reach| reach.symbol == "Makefile::target-11")
        );
    }

    fn hub_snapshot(importer_count: usize) -> Snapshot {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);

        let mut hub = FileAnalysis::new("loctree-rs/src/types.rs".to_string());
        hub.language = "rust".to_string();
        hub.loc = 800;
        snapshot.files.push(hub);

        for index in 0..importer_count {
            let path = format!("loctree-rs/src/consumer_{index}.rs");
            let mut consumer = FileAnalysis::new(path.clone());
            consumer.language = "rust".to_string();
            snapshot.files.push(consumer);
            snapshot.edges.push(GraphEdge {
                from: path,
                to: "loctree-rs/src/types.rs".to_string(),
                label: "import".to_string(),
            });
        }

        snapshot
    }

    #[test]
    fn compose_risk_slice_with_hotspots() {
        let snapshot = hub_snapshot(12);
        let opts = ContextOptions {
            file: Some(PathBuf::from("types.rs")),
            ..ContextOptions::default()
        };

        let risk = compose_risk_slice(&opts, &snapshot);

        assert_eq!(risk.hotspots.len(), 1);
        assert_eq!(risk.hotspots[0].file, "loctree-rs/src/types.rs");
        assert_eq!(risk.hotspots[0].importers, 12);
        assert_eq!(risk.hotspots[0].authority, AuthorityLabel::LoctreeDerived);
        assert_eq!(risk.high_fan_in.len(), 1);
        assert_eq!(risk.high_fan_in[0].threshold, 10);
        assert_eq!(
            risk.high_fan_in[0].authority,
            AuthorityLabel::LoctreeDerived
        );
    }

    #[test]
    fn compose_risk_slice_dirty_worktree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .expect("git init");
        std::fs::write(tmp.path().join("dirty.rs"), "fn main() {}\n").expect("dirty file");

        let mut snapshot = Snapshot::new(vec![tmp.path().display().to_string()]);
        snapshot.metadata.roots = vec![tmp.path().display().to_string()];
        let opts = ContextOptions {
            project: Some(tmp.path().to_path_buf()),
            ..ContextOptions::default()
        };

        let risk = compose_risk_slice(&opts, &snapshot);

        assert!(risk.dirty_worktree);
        assert!(!risk.stale_snapshot);
        assert_eq!(risk.cache_scope, RiskCacheScope::DirtyWorktree);
        assert_eq!(risk.cache_scope_authority, AuthorityLabel::RepoVerified);
        assert_eq!(risk.snapshot_health.as_deref(), Some("dirty"));
    }

    #[test]
    fn compose_risk_slice_empty_corpus_is_empty_snapshot_not_missing() {
        // loctree-fail 2026-08-11: successful zero-file scan must not read as
        // missing_snapshot — emit empty_snapshot so verified absence is explicit.
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut snapshot = Snapshot::new(vec![tmp.path().display().to_string()]);
        snapshot.metadata.roots = vec![tmp.path().display().to_string()];
        assert!(snapshot.files.is_empty());

        let opts = ContextOptions {
            project: Some(tmp.path().to_path_buf()),
            ..ContextOptions::default()
        };
        let risk = compose_risk_slice(&opts, &snapshot);

        assert_eq!(risk.snapshot_health.as_deref(), Some("empty_snapshot"));
        assert_ne!(risk.snapshot_health.as_deref(), Some("missing_snapshot"));
        assert!(!risk.stale_snapshot);
    }

    fn twelve_file_universe() -> Snapshot {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);
        for i in 0..12 {
            let mut file = FileAnalysis::new(format!("src/file_{i:02}.rs"));
            file.loc = 80 + i;
            file.language = "rust".to_string();
            snapshot.files.push(file);
        }
        snapshot
    }

    #[test]
    fn w2_04_full_pack_declares_coverage() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let snapshot = twelve_file_universe();
        let bare_opts = ContextOptions {
            project: Some(tmp.path().to_path_buf()),
            no_aicx: true,
            ..ContextOptions::default()
        };
        let full_opts = ContextOptions {
            full: true,
            ..bare_opts.clone()
        };

        let bare = compose_context_pack_from_snapshot(&bare_opts, tmp.path(), &snapshot)
            .expect("bare pack");
        let full = compose_context_pack_from_snapshot(&full_opts, tmp.path(), &snapshot)
            .expect("full pack");

        for (label, pack) in [("bare", &bare), ("full", &full)] {
            assert_eq!(
                pack.coverage.selected, DEFAULT_SCOPE_TARGET_LIMIT,
                "{label} selected"
            );
            assert_eq!(pack.coverage.total, 12, "{label} total");
            assert!(pack.coverage.truncated, "{label} must declare truncation");
            let json = serde_json::to_value(pack).expect("serialize");
            assert_eq!(json["coverage"]["selected"], 8, "{label} json selected");
            assert_eq!(json["coverage"]["total"], 12, "{label} json total");
            assert_eq!(
                json["coverage"]["truncated"].as_bool(),
                Some(true),
                "{label} json truncated"
            );
            eprintln!("W2-04 {label} coverage json: {}", json["coverage"]);
            let md = format_context_pack_markdown(pack);
            assert!(
                md.contains("**Coverage**: 8 of 12 targets selected (truncated — not the whole repo)"),
                "{label} markdown must declare coverage, got:\n{md}"
            );
        }
    }

    #[test]
    fn w2_04_task_exact_name_outranks_recency() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        let mut named = FileAnalysis::new("launch.sh".to_string());
        named.loc = 4;
        snapshot.files.push(named);
        for i in 0..6 {
            let mut hotspot = FileAnalysis::new(format!("src/fix_permissions_{i}.rs"));
            hotspot.loc = 900;
            hotspot.exports.push(ExportSymbol::new(
                "fix_permissions".to_string(),
                "function",
                "named",
                Some(1),
            ));
            snapshot.files.push(hotspot);
        }

        let ranked = get_task_targets("fix launch.sh permissions", &snapshot);
        eprintln!("W2-04 task ranking: {ranked:?}");
        assert_eq!(
            ranked.first().map(String::as_str),
            Some("launch.sh"),
            "exact-name file must outrank substring recency stand-ins: {ranked:?}"
        );

        let opts = ContextOptions {
            project: Some(tmp.path().to_path_buf()),
            no_aicx: true,
            task: Some("fix launch.sh permissions".to_string()),
            ..ContextOptions::default()
        };
        let pack = compose_context_pack_from_snapshot(&opts, tmp.path(), &snapshot)
            .expect("task pack");
        assert!(
            pack.structural
                .files
                .iter()
                .any(|file| file.path == "launch.sh"),
            "pack targets must include launch.sh, got {:?}",
            pack.structural
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>()
        );
    }

    /// W1-A negative control (audit wrong-commit class LCT-D/E/L): a snapshot
    /// that names a different commit than live HEAD must be refused right
    /// after a scan — it can never back a fresh receipt.
    #[test]
    fn identity_receipt_post_scan_mismatch_refuses() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?} failed");
        };
        git(&["init"]);
        git(&[
            "-c",
            "user.email=trust@loctree.test",
            "-c",
            "user.name=trust",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ]);

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
        snapshot.metadata.git_commit = Some("1cb669d4".to_string());
        let err = verify_post_scan_identity(&snapshot, root).expect_err("mismatch must refuse");
        assert!(
            matches!(err, ContextLoadError::PostScanIdentityMismatch { .. }),
            "expected PostScanIdentityMismatch, got: {err}"
        );

        let live_head = current_git_head(root).expect("live HEAD");
        snapshot.metadata.git_commit = Some(live_head[..8].to_string());
        verify_post_scan_identity(&snapshot, root).expect("matching identity passes");
    }

    /// Non-git roots have no identity to contradict: the gate must pass and
    /// leave honesty to the receipt's `unknown` authority.
    #[test]
    fn identity_receipt_post_scan_gate_passes_without_git_identity() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut snapshot = Snapshot::new(vec![tmp.path().display().to_string()]);
        snapshot.metadata.git_commit = Some("1cb669d4".to_string());
        verify_post_scan_identity(&snapshot, tmp.path()).expect("no live git — nothing to refuse");
    }

    /// A pack emitted without any snapshot must carry a refused receipt, not
    /// a silently-default one.
    #[test]
    fn identity_receipt_missing_snapshot_pack_refuses_authority() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let opts = ContextOptions {
            project: Some(tmp.path().to_path_buf()),
            ..ContextOptions::default()
        };

        let pack = missing_snapshot_context_pack(&opts, tmp.path());

        assert_eq!(
            pack.receipt.authority,
            crate::receipt::ReceiptAuthority::Refused
        );
        assert_eq!(pack.receipt.binary_id, crate::BUILD_VERSION);
        assert!(pack.receipt.snapshot_fingerprint.is_none());
        assert_eq!(
            pack.receipt.schema_version,
            crate::receipt::RECEIPT_SCHEMA_VERSION
        );
    }

    /// Every composed pack binds a receipt: root + snapshot fingerprint +
    /// binary identity travel with the answer (MCP inherits this path).
    #[test]
    fn identity_receipt_bound_on_compose_from_snapshot() {
        let snapshot = hub_snapshot(12);
        let opts = ContextOptions {
            file: Some(PathBuf::from("types.rs")),
            no_aicx: true,
            ..ContextOptions::default()
        };

        let pack = compose_context_pack_from_snapshot(&opts, Path::new("."), &snapshot)
            .expect("compose context pack");

        assert_eq!(
            pack.receipt.schema_version,
            crate::receipt::RECEIPT_SCHEMA_VERSION
        );
        assert_eq!(pack.receipt.binary_id, crate::BUILD_VERSION);
        assert!(pack.receipt.root.is_some(), "receipt must bind the root");
        let fingerprint = pack
            .receipt
            .snapshot_fingerprint
            .as_deref()
            .expect("receipt must bind the snapshot fingerprint");
        assert!(
            fingerprint.starts_with("sha256:loctree-snapshot-authority-v1:"),
            "fingerprint must carry its algorithm: {fingerprint}"
        );
    }

    #[test]
    fn compose_action_slice_suggests_loct_slice_for_hub() {
        let snapshot = hub_snapshot(12);
        let opts = ContextOptions {
            file: Some(PathBuf::from("types.rs")),
            ..ContextOptions::default()
        };
        let risk = compose_risk_slice(&opts, &snapshot);

        let action = compose_action_slice(
            &opts,
            &snapshot,
            &StructuralSlice::default(),
            &RuntimeSlice::default(),
            &risk,
        );

        assert!(
            action
                .next_safe_commands
                .contains(&"loct slice loctree-rs/src/types.rs".to_string())
        );
        assert!(
            action
                .next_safe_commands
                .contains(&"loct impact loctree-rs/src/types.rs".to_string())
        );
        assert!((1..=3).contains(&action.next_safe_commands.len()));
    }

    #[test]
    fn test_power_path_for_file_focused_task() {
        let snapshot = hub_snapshot(12);

        let structural = StructuralSlice {
            files: vec![],
            symbols: vec![StructuralSymbol {
                name: "my_cool_fn".to_string(),
                kind: "function".to_string(),
                export_type: "default".to_string(),
                file: "types.rs".to_string(),
                line: Some(10),
                authority: AuthorityLabel::RepoVerified,
            }],
            imports: vec![],
            consumers: vec![StructuralConsumer {
                file: "consumer.rs".to_string(),
                import_kind: ConsumerKind::Direct,
                imports_used: vec!["my_cool_fn".to_string()],
                authority: AuthorityLabel::RepoVerified,
            }],
            entrypoints: vec![],
        };

        let opts = ContextOptions {
            file: Some(PathBuf::from("types.rs")),
            task: Some("fix auth in types.rs".to_string()),
            ..ContextOptions::default()
        };

        let risk = compose_risk_slice(&opts, &snapshot);
        let action = compose_action_slice(
            &opts,
            &snapshot,
            &structural,
            &RuntimeSlice::default(),
            &risk,
        );

        assert!(
            !action.power_path.is_empty(),
            "power_path should not be empty"
        );

        assert!(
            action
                .power_path
                .iter()
                .any(|c| c.command == "loct slice types.rs")
        );
        assert!(
            action
                .power_path
                .iter()
                .any(|c| c.command == "loct impact types.rs")
        );
        assert!(
            action
                .power_path
                .iter()
                .any(|c| c.command == "loct body my_cool_fn")
        );
        assert!(
            action
                .power_path
                .iter()
                .any(|c| c.command == "loct find --literal my_cool_fn")
        );
        assert!(
            action
                .power_path
                .iter()
                .any(|c| c.command == "loct occurrences my_cool_fn")
        );
        assert!(action.power_path.iter().any(|c| c.command == "loct follow"));
    }

    #[test]
    fn context_default_routing_prefers_markdown_unless_json_requested() {
        let opts = ContextOptions::default();
        let global = GlobalOptions::default();
        assert!(
            !context_wants_json(&opts, &global),
            "bare loct context should render markdown"
        );

        let opts = ContextOptions {
            full: true,
            ..ContextOptions::default()
        };
        assert!(context_wants_json(&opts, &global), "--full opts into JSON");

        let opts = ContextOptions {
            json: true,
            ..ContextOptions::default()
        };
        assert!(context_wants_json(&opts, &global), "--json opts into JSON");

        let global = GlobalOptions {
            json: true,
            ..GlobalOptions::default()
        };
        assert!(
            context_wants_json(&ContextOptions::default(), &global),
            "global --json opts into JSON"
        );
    }

    #[test]
    fn compose_default_scope_uses_top_hubs_for_bare_context() {
        let snapshot = hub_snapshot(12);
        let opts = ContextOptions::default();

        let scope = compose_default_scope(&snapshot, &opts, None, None);

        assert!(
            scope.contains(&"loctree-rs/src/types.rs".to_string()),
            "default scope should include highest fan-in hub: {scope:?}"
        );
        assert!(
            scope.len() >= 3,
            "fallback should avoid an empty or tiny default scope: {scope:?}"
        );
    }

    #[test]
    fn verification_gates_rust_workspace_returns_workspace_commands() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("crates/app")).expect("crate dir");
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[workspace]
members = ["crates/app"]
"#,
        )
        .expect("workspace manifest");
        std::fs::write(
            tmp.path().join("crates/app/Cargo.toml"),
            r#"[package]
name = "app"
version = "0.1.0"
edition = "2024"
"#,
        )
        .expect("member manifest");

        let stack = detect_project_stack(tmp.path());
        let gates = verification_gates_for(&[], &stack, tmp.path());
        let commands: Vec<String> = gates.iter().map(|gate| gate.command.clone()).collect();

        assert_eq!(
            commands,
            vec![
                "cargo check --workspace".to_string(),
                "cargo clippy --workspace --all-targets -- -D warnings".to_string(),
                "cargo test --workspace".to_string(),
            ]
        );
        assert!(
            gates
                .iter()
                .all(|gate| gate.authority == AuthorityLabel::LoctreeDerived)
        );
    }

    #[test]
    fn verification_gates_python_returns_pytest_ruff() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("pyproject.toml"),
            r#"[project]
name = "py-demo"

[tool.pytest.ini_options]
testpaths = ["tests"]

[tool.ruff]
line-length = 100

[tool.mypy]
python_version = "3.12"
"#,
        )
        .expect("pyproject");

        let stack = detect_project_stack(tmp.path());
        let commands: Vec<String> = verification_gates_for(&[], &stack, tmp.path())
            .into_iter()
            .map(|gate| gate.command)
            .collect();

        assert_eq!(
            commands,
            vec![
                "ruff check .".to_string(),
                "mypy .".to_string(),
                "pytest".to_string(),
            ]
        );
        assert!(!commands.iter().any(|command| command.contains("cargo")));
    }

    #[test]
    fn verification_gates_makefile_wins() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("Makefile"), "test:\n\tpytest\n").expect("Makefile");

        let stack = detect_project_stack(tmp.path());
        let gates = verification_gates_for(&[], &stack, tmp.path());

        assert_eq!(gates.len(), 1);
        assert_eq!(gates[0].command, "make test");
        assert_eq!(gates[0].authority, AuthorityLabel::RepoVerified);
    }

    #[test]
    fn verification_gates_node_returns_package_manager_scripts() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"lint":"eslint .","test":"vitest","check":"tsc --noEmit"}}"#,
        )
        .expect("package.json");
        std::fs::write(
            tmp.path().join("pnpm-lock.yaml"),
            "lockfileVersion: '9.0'\n",
        )
        .expect("pnpm lockfile");

        let stack = detect_project_stack(tmp.path());
        let commands: Vec<String> = verification_gates_for(&[], &stack, tmp.path())
            .into_iter()
            .map(|gate| gate.command)
            .collect();

        assert_eq!(
            commands,
            vec![
                "pnpm lint".to_string(),
                "pnpm check".to_string(),
                "pnpm test".to_string(),
            ]
        );
    }

    #[test]
    fn verification_gates_unknown_stack_is_honestly_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let stack = detect_project_stack(tmp.path());
        let gates = verification_gates_for(&[], &stack, tmp.path());

        assert!(matches!(stack, ProjectStack::Unknown));
        assert!(gates.is_empty());

        let pack = ContextPack {
            action: ActionSlice::default(),
            ..ContextPack::empty(ProjectIdentity::default())
        };
        let md = format_context_pack_markdown(&pack);
        // Honest emptiness: an unknown stack fabricates no gates (the conditional
        // "### Verification Gates" section is never emitted) and the risk authority
        // is labelled StaleOrUnknown. The old literal "_no project stack detected"
        // line was dropped when --full went synthesis-first (W6.4); the contract is
        // now carried by these two signals.
        assert!(!md.contains("### Verification Gates"));
        assert!(md.contains("StaleOrUnknown"));
    }

    #[test]
    fn likely_tests_capped_at_top_n() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        snapshot
            .files
            .push(FileAnalysis::new("src/hub.py".to_string()));
        for index in 0..200 {
            let test_path = format!("tests/test_{index:03}.py");
            snapshot.files.push(FileAnalysis::new(test_path.clone()));
            snapshot.edges.push(GraphEdge {
                from: test_path,
                to: "src/hub.py".to_string(),
                label: "import".to_string(),
            });
        }

        let tests = likely_tests_for(&[], &snapshot, 10);

        assert_eq!(tests.len(), 10);
    }

    #[test]
    fn likely_tests_relevance_via_hubs() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        snapshot
            .files
            .push(FileAnalysis::new("src/hub_a.py".to_string()));
        snapshot
            .files
            .push(FileAnalysis::new("src/leaf_b.py".to_string()));
        snapshot
            .files
            .push(FileAnalysis::new("tests/test_hub_a.py".to_string()));
        snapshot
            .files
            .push(FileAnalysis::new("tests/test_leaf_b.py".to_string()));

        for index in 0..5 {
            let consumer = format!("src/consumer_{index}.py");
            snapshot.files.push(FileAnalysis::new(consumer.clone()));
            snapshot.edges.push(GraphEdge {
                from: consumer,
                to: "src/hub_a.py".to_string(),
                label: "import".to_string(),
            });
        }
        snapshot.edges.push(GraphEdge {
            from: "tests/test_hub_a.py".to_string(),
            to: "src/hub_a.py".to_string(),
            label: "import".to_string(),
        });
        snapshot.edges.push(GraphEdge {
            from: "tests/test_leaf_b.py".to_string(),
            to: "src/leaf_b.py".to_string(),
            label: "import".to_string(),
        });

        let tests = likely_tests_for(&[], &snapshot, 1);

        assert_eq!(tests, vec!["tests/test_hub_a.py".to_string()]);
    }

    #[test]
    fn likely_tests_excludes_fixture_and_non_test_language_files() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        let mut app = FileAnalysis::new("src/app.rs".to_string());
        app.language = "rust".to_string();
        snapshot.files.push(app);

        let mut real_test = FileAnalysis::new("tests/app_test.rs".to_string());
        real_test.language = "rust".to_string();
        real_test.is_test = true;
        let mut make_fixture = FileAnalysis::new("tests/fixtures/Makefile".to_string());
        make_fixture.language = "make".to_string();
        make_fixture.is_test = true;
        let mut css_fixture = FileAnalysis::new("tests/fixtures/styles.css".to_string());
        css_fixture.language = "css".to_string();
        css_fixture.is_test = true;
        snapshot
            .files
            .extend([real_test, make_fixture, css_fixture]);

        for from in [
            "tests/app_test.rs",
            "tests/fixtures/Makefile",
            "tests/fixtures/styles.css",
        ] {
            snapshot.edges.push(GraphEdge {
                from: from.to_string(),
                to: "src/app.rs".to_string(),
                label: "import".to_string(),
            });
        }

        let tests = likely_tests_for(&["src/app.rs".to_string()], &snapshot, 10);

        assert_eq!(tests, vec!["tests/app_test.rs".to_string()]);
    }

    #[test]
    fn next_safe_commands_references_top_hub() {
        let structural = StructuralSlice {
            files: vec![StructuralFile {
                path: "Makefile".to_string(),
                role: StructuralRole::Target,
                depth: 0,
                language: "make".to_string(),
                loc: 12,
                authority: AuthorityLabel::RepoVerified,
            }],
            ..StructuralSlice::default()
        };
        let risk = RiskSlice {
            hotspots: vec![HotspotFile {
                file: "src/hub.py".to_string(),
                importers: 42,
                authority: AuthorityLabel::LoctreeDerived,
            }],
            ..RiskSlice::default()
        };

        let opts = ContextOptions {
            file: Some(PathBuf::from("Makefile")),
            ..ContextOptions::default()
        };
        let runtime = RuntimeSlice::default();
        let snapshot = Snapshot::new(vec![".".to_string()]);
        let commands = next_safe_commands_for(&opts, &structural, &runtime, &risk, &snapshot);

        assert_eq!(commands[0], "loct slice src/hub.py");
        assert_eq!(commands[1], "loct impact src/hub.py");
        assert!(
            !commands
                .iter()
                .any(|command| command == "loct slice Makefile")
        );
    }

    fn test_import_snapshot() -> Snapshot {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);

        let mut lib = FileAnalysis::new("loctree-rs/src/lib.rs".to_string());
        lib.language = "rust".to_string();
        let mut test = FileAnalysis::new("loctree-rs/tests/e2e_cli.rs".to_string());
        test.language = "rust".to_string();
        test.is_test = true;

        snapshot.files.push(lib);
        snapshot.files.push(test);
        snapshot.edges.push(GraphEdge {
            from: "loctree-rs/tests/e2e_cli.rs".to_string(),
            to: "loctree-rs/src/lib.rs".to_string(),
            label: "import".to_string(),
        });

        snapshot
    }

    #[test]
    fn compose_action_slice_likely_tests() {
        let snapshot = test_import_snapshot();
        let opts = ContextOptions {
            file: Some(PathBuf::from("../src/lib.rs")),
            ..ContextOptions::default()
        };

        let action = compose_action_slice(
            &opts,
            &snapshot,
            &StructuralSlice::default(),
            &RuntimeSlice::default(),
            &RiskSlice::default(),
        );

        assert_eq!(
            action.likely_tests,
            vec!["loctree-rs/tests/e2e_cli.rs".to_string()]
        );

        let opts = ContextOptions {
            file: Some(PathBuf::from("e2e_cli.rs")),
            ..ContextOptions::default()
        };
        let action = compose_action_slice(
            &opts,
            &snapshot,
            &StructuralSlice::default(),
            &RuntimeSlice::default(),
            &RiskSlice::default(),
        );

        assert_eq!(
            action.likely_tests,
            vec!["loctree-rs/tests/e2e_cli.rs".to_string()]
        );
    }

    #[test]
    fn changed_mode_reads_git_status() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .expect("git init");
        std::fs::write(tmp.path().join("changed.rs"), "fn main() {}\n").expect("write file");

        let mut snapshot = Snapshot::new(vec![tmp.path().display().to_string()]);
        snapshot
            .files
            .push(FileAnalysis::new("changed.rs".to_string()));

        let opts = ContextOptions {
            changed: true,
            project: Some(tmp.path().to_path_buf()),
            ..ContextOptions::default()
        };

        let targets = super::get_changed_targets(&opts, &snapshot);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0], "changed.rs");
    }

    #[test]
    fn changed_mode_handles_detached_head_gracefully() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .expect("git init");

        // Setup initial commit to detach from
        std::fs::write(tmp.path().join("initial.rs"), "fn main() {}\n").expect("write file");
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(tmp.path())
            .output()
            .unwrap();

        // Detach HEAD
        std::process::Command::new("git")
            .args(["checkout", "--detach"])
            .current_dir(tmp.path())
            .output()
            .unwrap();

        std::fs::write(tmp.path().join("changed.rs"), "fn main() {}\n").expect("write file");

        let mut snapshot = Snapshot::new(vec![tmp.path().display().to_string()]);
        snapshot
            .files
            .push(FileAnalysis::new("changed.rs".to_string()));

        let opts = ContextOptions {
            changed: true,
            project: Some(tmp.path().to_path_buf()),
            ..ContextOptions::default()
        };

        let targets = super::get_changed_targets(&opts, &snapshot);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0], "changed.rs");
    }

    #[test]
    fn task_matcher_scores_by_token_overlap() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        let mut file1 = FileAnalysis::new("src/server.rs".to_string());
        file1.exports.push(ExportSymbol::new(
            "start_server".to_string(),
            "function",
            "named",
            None,
        ));

        let mut file2 = FileAnalysis::new("src/client.rs".to_string());
        file2.exports.push(ExportSymbol::new(
            "connect".to_string(),
            "function",
            "named",
            None,
        ));

        snapshot.files.push(file1);
        snapshot.files.push(file2);

        let targets = super::get_task_targets("start the server", &snapshot);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0], "src/server.rs");
    }

    #[test]
    fn task_matcher_returns_empty_with_explanation_below_threshold() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        snapshot
            .files
            .push(FileAnalysis::new("src/server.rs".to_string()));

        let targets = super::get_task_targets("unrelated jibberish", &snapshot);
        assert!(targets.is_empty());
    }

    #[test]
    fn markdown_formatter_includes_all_slice_headings() {
        let pack = ContextPack::empty(ProjectIdentity::default());
        let md = super::format_context_pack_markdown(&pack);

        assert!(md.contains("## Project Identity"));
        assert!(md.contains("## Where You Are"));
        assert!(md.contains("## Runtime Slice"));
        assert!(md.contains("## Risk Slice"));
        assert!(md.contains("## Action Slice"));
        assert!(md.contains("## Memory Slice"));
        assert!(md.contains("## Authority Slice"));
    }

    #[test]
    fn markdown_full_is_synthesis_first_w6_4() {
        // W6.4 regression: Risk/Action (synthesis) must appear before the bulk
        // Files/Symbols/Consumers enumeration in full md, so truncation (MCP
        // caps, Read ~25k tokens / ~450 lines per fail.md 3352) still delivers
        // the decision-useful Risk+Action+PowerPath. This is the fixture.
        let mut pack = ContextPack::empty(ProjectIdentity::default());
        // populate a bit of structural so enumeration exists
        pack.structural.files.push(StructuralFile {
            path: "src/foo.rs".to_string(),
            role: StructuralRole::Target,
            depth: 0,
            language: "rust".to_string(),
            loc: 10,
            authority: AuthorityLabel::RepoVerified,
        });
        let md = super::format_context_pack_markdown(&pack);
        let risk_pos = md
            .find("## Risk Slice (synthesis-first")
            .unwrap_or(usize::MAX);
        let files_pos = md.find("### Files").unwrap_or(0);
        // synthesis (early Risk) before the detailed enumeration tables
        assert!(
            risk_pos < files_pos,
            "W6.4: synthesis must precede bulk tables in --full md; risk@{} files@{}",
            risk_pos,
            files_pos
        );
    }

    #[test]
    fn markdown_hotspots_and_high_fan_in_emit_once() {
        // Trust audit ad28ded: late Risk tables duplicated early synthesis
        // tables while a comment claimed "No dupe". Count headers, not rows.
        let mut pack = ContextPack::empty(ProjectIdentity::default());
        pack.risk.hotspots.push(HotspotFile {
            file: "src/hub.rs".to_string(),
            importers: 12,
            authority: AuthorityLabel::LoctreeDerived,
        });
        pack.risk.high_fan_in.push(HighFanInFile {
            file: "src/hub.rs".to_string(),
            importers: 12,
            threshold: 10,
            authority: AuthorityLabel::LoctreeDerived,
        });
        let md = super::format_context_pack_markdown(&pack);
        let hotspots = md.matches("### Hotspots\n").count();
        let fan_in = md.matches("### High Fan-In\n").count();
        assert_eq!(
            hotspots, 1,
            "Hotspots table must appear exactly once (early Risk Slice only):\n{md}"
        );
        assert_eq!(
            fan_in, 1,
            "High Fan-In table must appear exactly once (early Risk Slice only):\n{md}"
        );
        assert!(
            !md.contains("No dupe, no late emission"),
            "false-confidence comment must not reappear"
        );
    }

    #[test]
    fn markdown_formatter_jsonpacks_round_trip() {
        let mut pack = ContextPack::empty(ProjectIdentity::default());
        pack.structural.files.push(StructuralFile {
            path: "src/main.rs".to_string(),
            role: StructuralRole::Target,
            depth: 0,
            language: "rust".to_string(),
            loc: 100,
            authority: AuthorityLabel::RepoVerified,
        });

        let md = super::format_context_pack_markdown(&pack);
        assert!(md.contains("src/main.rs"));
        assert!(md.contains("Target"));
        assert!(md.contains("rust"));
        assert!(md.contains("RepoVerified"));
    }

    // -----------------------------------------------------------------------
    // Cut 5 T1 — memory slice composer tests
    //
    // The composer integrates with the external `aicx` CLI through the
    // [`AicxClient`] wrapper. Tests fake the binary by writing a small shell
    // script to a tempdir and pointing `LOCT_AICX_BINARY` at it. They are
    // serialised via `serial_test::serial(aicx_env)` because they mutate the
    // process-global env, which would otherwise interleave with the wrapper
    // tests in `crate::aicx`.
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    fn write_mock_aicx(dir: &Path, payload: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("aicx-mock.sh");
        let escaped = payload.replace('\'', "'\\''");
        let body = format!("#!/bin/sh\nprintf '%s' '{escaped}'\nexit 0\n");
        std::fs::write(&script, body).expect("write mock script");
        let mut perms = std::fs::metadata(&script)
            .expect("stat mock script")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).expect("chmod mock script");
        script
    }

    fn structural_with_target(path: &str, symbols: &[&str]) -> StructuralSlice {
        let files = vec![StructuralFile {
            path: path.to_string(),
            role: StructuralRole::Target,
            depth: 0,
            language: "rust".to_string(),
            loc: 200,
            authority: AuthorityLabel::RepoVerified,
        }];
        let symbols = symbols
            .iter()
            .map(|name| StructuralSymbol {
                name: (*name).to_string(),
                kind: "function".to_string(),
                export_type: "named".to_string(),
                file: path.to_string(),
                line: Some(1),
                authority: AuthorityLabel::RepoVerified,
            })
            .collect();
        StructuralSlice {
            files,
            symbols,
            imports: Vec::new(),
            consumers: Vec::new(),
            entrypoints: Vec::new(),
        }
    }

    #[test]
    fn compose_memory_slice_empty_without_with_aicx_flag() {
        // No --with-aicx → composer must skip every shell-out and return
        // the empty slice, even when the structural slice has plenty of
        // candidate keywords.
        let opts = ContextOptions {
            with_aicx: false,
            ..ContextOptions::default()
        };
        let structural = structural_with_target(
            "loctree-rs/src/cli/dispatch/handlers/context.rs",
            &["compose_memory_slice"],
        );
        let runtime = RuntimeSlice::default();

        // Even when a client is present, the flag is the gate.
        let client = AicxClient::new("loctree-suite");
        let memory = compose_memory_slice(&opts, &structural, &runtime, Some(&client));

        assert!(
            memory.entries.is_empty(),
            "memory entries must be empty without --with-aicx"
        );
        assert!(
            memory.source_chunks.is_empty(),
            "source_chunks must be empty without --with-aicx"
        );
    }

    #[test]
    fn compose_memory_slice_reports_timed_out_when_budget_exhausted() {
        // Perf/honesty canary (W1-06): a budgeted overlay client whose
        // wall-clock budget ran dry must surface `skip_reason: timed_out`,
        // never `namespace_empty` — "store never got to answer" is not
        // "store answered: nothing there". Deterministic: zero budget, no
        // transport spawns (cfg(test) kill switch), no wall-clock asserts.
        let opts = ContextOptions {
            with_aicx: true,
            ..ContextOptions::default()
        };
        let structural = structural_with_target(
            "loctree-rs/src/cli/dispatch/handlers/context.rs",
            &["compose_memory_slice"],
        );
        let client = AicxClient::new_budgeted("loctree-suite", Some(Duration::ZERO));

        let memory =
            compose_memory_slice(&opts, &structural, &RuntimeSlice::default(), Some(&client));

        assert!(memory.entries.is_empty());
        let diagnostic = memory.diagnostic.expect("diagnostic always populated");
        assert_eq!(
            diagnostic.skip_reason,
            MemorySkipReason::TimedOut,
            "exhausted budget must be reported as timed_out, not namespace_empty"
        );
    }

    #[test]
    fn compose_memory_slice_empty_when_client_missing() {
        // Belt-and-suspenders: even with --with-aicx, if the caller passes
        // no client (e.g. flag was set but constructor was skipped), we
        // must not panic and must return an empty slice.
        let opts = ContextOptions {
            with_aicx: true,
            ..ContextOptions::default()
        };
        let memory = compose_memory_slice(
            &opts,
            &StructuralSlice::default(),
            &RuntimeSlice::default(),
            None,
        );
        assert!(memory.entries.is_empty());
        assert!(memory.source_chunks.is_empty());
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn compose_memory_slice_filters_by_scope_keywords() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Three intents: two mention compose_memory_slice / context.rs (in scope),
        // one is about an unrelated payments refactor (out of scope).
        let payload = r#"[
            {"kind":"decision","summary":"Adopt compose_memory_slice composer for context handler","project":"loctree-suite","agent":"claude","date":"2026-04-28","timestamp":"2026-04-28T01:00:00Z","session_id":"s1","source_chunk":"/tmp/aicx/store/s1.md"},
            {"kind":"task","summary":"Wire context.rs run() to memory slice","project":"loctree-suite","agent":"codex","date":"2026-04-27","timestamp":"2026-04-27T15:00:00Z","session_id":"s2","source_chunk":"/tmp/aicx/store/s2.md"},
            {"kind":"decision","summary":"Refactor stripe payment webhook handler","project":"loctree-suite","agent":"gemini","date":"2026-04-26","timestamp":"2026-04-26T10:00:00Z","session_id":"s3","source_chunk":"/tmp/aicx/store/s3.md"}
        ]"#;
        let script = write_mock_aicx(dir.path(), payload);

        {
            crate::aicx::set_aicx_test_opt_in();
            crate::test_env::set_var(crate::aicx::AICX_MODE_ENV, "cli");
            crate::test_env::set_var(crate::aicx::AICX_BINARY_ENV, &script);
        }
        let client = AicxClient::new("loctree-suite");
        let opts = ContextOptions {
            with_aicx: true,
            ..ContextOptions::default()
        };
        let structural = structural_with_target(
            "loctree-rs/src/cli/dispatch/handlers/context.rs",
            &["compose_memory_slice"],
        );
        let memory =
            compose_memory_slice(&opts, &structural, &RuntimeSlice::default(), Some(&client));
        {
            crate::test_env::remove_var(crate::aicx::AICX_BINARY_ENV);
            crate::test_env::remove_var(crate::aicx::AICX_MODE_ENV);
            crate::aicx::clear_aicx_test_opt_in();
        }

        assert_eq!(
            memory.entries.len(),
            2,
            "only in-scope intents should pass the filter, got: {:#?}",
            memory.entries
        );
        let kinds: Vec<&str> = memory.entries.iter().map(|e| e.kind.as_str()).collect();
        assert!(kinds.contains(&"decision"));
        assert!(kinds.contains(&"task"));
        assert!(
            !memory
                .entries
                .iter()
                .any(|e| e.text.contains("stripe payment")),
            "out-of-scope intent must not appear"
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn compose_memory_slice_authority_labels_by_kind() {
        let dir = tempfile::tempdir().expect("tempdir");
        // One intent of every kind plus one explicit failure outcome.
        // All four touch the in-scope token "context" so every row stays
        // after relevance filtering.
        let payload = r#"[
            {"kind":"decision","summary":"context handler keeps ContextPack composers","project":"loctree-suite","agent":"claude","date":"2026-04-28","timestamp":"2026-04-28T01:00:00Z","session_id":"sd","source_chunk":"/tmp/aicx/store/sd.md"},
            {"kind":"intent","summary":"add memory slice composer for context","project":"loctree-suite","agent":"claude","date":"2026-04-28","timestamp":"2026-04-28T01:01:00Z","session_id":"si","source_chunk":"/tmp/aicx/store/si.md"},
            {"kind":"outcome","summary":"context tests pass on macOS arm64","project":"loctree-suite","agent":"codex","date":"2026-04-28","timestamp":"2026-04-28T01:02:00Z","session_id":"so","source_chunk":"/tmp/aicx/store/so.md"},
            {"kind":"task","summary":"document context flag --with-aicx","project":"loctree-suite","agent":"gemini","date":"2026-04-28","timestamp":"2026-04-28T01:03:00Z","session_id":"st","source_chunk":"/tmp/aicx/store/st.md"},
            {"kind":"outcome","summary":"context build failed on Windows after rollback","project":"loctree-suite","agent":"codex","date":"2026-04-28","timestamp":"2026-04-28T01:04:00Z","session_id":"sf","source_chunk":"/tmp/aicx/store/sf.md"}
        ]"#;
        let script = write_mock_aicx(dir.path(), payload);

        {
            crate::aicx::set_aicx_test_opt_in();
            crate::test_env::set_var(crate::aicx::AICX_MODE_ENV, "cli");
            crate::test_env::set_var(crate::aicx::AICX_BINARY_ENV, &script);
        }
        let client = AicxClient::new("loctree-suite");
        let opts = ContextOptions {
            with_aicx: true,
            ..ContextOptions::default()
        };
        let structural = structural_with_target(
            "loctree-rs/src/cli/dispatch/handlers/context.rs",
            &["compose_memory_slice"],
        );
        let memory =
            compose_memory_slice(&opts, &structural, &RuntimeSlice::default(), Some(&client));
        {
            crate::test_env::remove_var(crate::aicx::AICX_BINARY_ENV);
            crate::test_env::remove_var(crate::aicx::AICX_MODE_ENV);
            crate::aicx::clear_aicx_test_opt_in();
        }

        let by_session: HashMap<&str, &MemoryEntry> = memory
            .entries
            .iter()
            .map(|e| (e.session_id.as_str(), e))
            .collect();

        assert_eq!(memory.entries.len(), 5, "all five intents must survive");
        assert_eq!(by_session["sd"].authority, AuthorityLabel::AicxOperator);
        assert_eq!(by_session["si"].authority, AuthorityLabel::AicxOperator);
        assert_eq!(by_session["so"].authority, AuthorityLabel::AicxAgent);
        assert_eq!(by_session["st"].authority, AuthorityLabel::AicxAgent);
        assert_eq!(by_session["sf"].authority, AuthorityLabel::AicxFailure);
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn compose_memory_slice_caps_at_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        // 10 in-scope intents; cap at 3 via env override.
        let mut rows = Vec::new();
        for i in 0..10 {
            rows.push(format!(
                "{{\"kind\":\"task\",\"summary\":\"work on context handler #{i}\",\"project\":\"loctree-suite\",\"agent\":\"claude\",\"date\":\"2026-04-28\",\"timestamp\":\"2026-04-28T01:00:{i:02}Z\",\"session_id\":\"sess{i}\",\"source_chunk\":\"/tmp/aicx/store/sess{i}.md\"}}"
            ));
        }
        let payload = format!("[{}]", rows.join(","));
        let script = write_mock_aicx(dir.path(), &payload);

        {
            crate::aicx::set_aicx_test_opt_in();
            crate::test_env::set_var(crate::aicx::AICX_MODE_ENV, "cli");
            crate::test_env::set_var(crate::aicx::AICX_BINARY_ENV, &script);
            crate::test_env::set_var("LOCT_CONTEXT_MEMORY_LIMIT", "3");
        }
        let client = AicxClient::new("loctree-suite");
        let opts = ContextOptions {
            with_aicx: true,
            ..ContextOptions::default()
        };
        let structural = structural_with_target(
            "loctree-rs/src/cli/dispatch/handlers/context.rs",
            &["compose_memory_slice"],
        );
        let memory =
            compose_memory_slice(&opts, &structural, &RuntimeSlice::default(), Some(&client));
        {
            crate::test_env::remove_var(crate::aicx::AICX_BINARY_ENV);
            crate::test_env::remove_var(crate::aicx::AICX_MODE_ENV);
            crate::aicx::clear_aicx_test_opt_in();
            crate::test_env::remove_var("LOCT_CONTEXT_MEMORY_LIMIT");
        }

        assert_eq!(
            memory.entries.len(),
            3,
            "memory slice must respect LOCT_CONTEXT_MEMORY_LIMIT"
        );
        assert!(
            memory.source_chunks.len() <= 3,
            "source_chunks bounded by entry count, got {}",
            memory.source_chunks.len()
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn compose_memory_slice_includes_source_chunk_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let payload = r#"[
            {"kind":"decision","summary":"context composer chooses absolute paths","project":"loctree-suite","agent":"claude","date":"2026-04-28","timestamp":"2026-04-28T01:00:00Z","session_id":"sa","source_chunk":"/Users/op/.aicx/store/loctree-suite/2026/04/28/sa.md"},
            {"kind":"task","summary":"context source chunk wiring test","project":"loctree-suite","agent":"codex","date":"2026-04-28","timestamp":"2026-04-28T01:01:00Z","session_id":"sb","source_chunk":"/Users/op/.aicx/store/loctree-suite/2026/04/28/sb.md"},
            {"kind":"task","summary":"another context follow-up referencing same chunk","project":"loctree-suite","agent":"codex","date":"2026-04-28","timestamp":"2026-04-28T01:02:00Z","session_id":"sc","source_chunk":"/Users/op/.aicx/store/loctree-suite/2026/04/28/sb.md"}
        ]"#;
        let script = write_mock_aicx(dir.path(), payload);

        {
            crate::aicx::set_aicx_test_opt_in();
            crate::test_env::set_var(crate::aicx::AICX_MODE_ENV, "cli");
            crate::test_env::set_var(crate::aicx::AICX_BINARY_ENV, &script);
        }
        let client = AicxClient::new("loctree-suite");
        let opts = ContextOptions {
            with_aicx: true,
            ..ContextOptions::default()
        };
        let structural = structural_with_target(
            "loctree-rs/src/cli/dispatch/handlers/context.rs",
            &["compose_memory_slice"],
        );
        let memory =
            compose_memory_slice(&opts, &structural, &RuntimeSlice::default(), Some(&client));
        {
            crate::test_env::remove_var(crate::aicx::AICX_BINARY_ENV);
            crate::test_env::remove_var(crate::aicx::AICX_MODE_ENV);
            crate::aicx::clear_aicx_test_opt_in();
        }

        assert_eq!(memory.entries.len(), 3, "all three intents are in scope");
        for entry in &memory.entries {
            assert!(
                entry.source_chunk.starts_with('/'),
                "source_chunk must be absolute: {}",
                entry.source_chunk
            );
            assert!(
                entry.source_chunk.ends_with(".md"),
                "source_chunk must point at a markdown file: {}",
                entry.source_chunk
            );
        }
        // De-duplicated: sb.md is referenced twice but appears once.
        assert_eq!(
            memory.source_chunks.len(),
            2,
            "duplicate chunk references collapse, got: {:?}",
            memory.source_chunks
        );
        assert!(
            memory
                .source_chunks
                .contains(&"/Users/op/.aicx/store/loctree-suite/2026/04/28/sa.md".to_string())
        );
        assert!(
            memory
                .source_chunks
                .contains(&"/Users/op/.aicx/store/loctree-suite/2026/04/28/sb.md".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn compose_memory_slice_populates_retrieval_mode_from_oracle_status() {
        // Closes audit finding A8 end-to-end: when AICX speaks the oracle
        // envelope (filesystem_fuzzy fallback in this scenario — the most
        // common operator-visible state today), `compose_memory_slice`
        // must propagate `retrieval_mode = "filesystem_fuzzy_fallback"`
        // onto every emitted `MemoryEntry`. Previously the wrapper threw
        // away the envelope and `retrieval_mode` was always `None`.
        let dir = tempfile::tempdir().expect("tempdir");
        let payload = r#"{
            "oracle_status": {
                "source_layer": "layer_1_canonical_corpus",
                "backend": "filesystem_fuzzy",
                "index_kind": "none",
                "fallback_reason": "fallback_filesystem_fuzzy: content index unavailable",
                "derived_view": "none_filesystem_scan",
                "store_root": "/tmp/aicx",
                "indexed_count": 0,
                "scanned_count": 42,
                "candidate_count": 2,
                "source_paths_verified": true,
                "stale_or_unknown": true,
                "loctree_scope_safe": false,
                "loctree_scope_note": "unsafe_for_scope_narrowing"
            },
            "results": 2,
            "items": [
                {"kind":"decision","summary":"Adopt compose_memory_slice composer for context handler","project":"loctree-suite","agent":"claude","date":"2026-04-28","timestamp":"2026-04-28T01:00:00Z","session_id":"sa","source_chunk":"/tmp/aicx/store/sa.md"},
                {"kind":"task","summary":"Wire context.rs run() to memory slice","project":"loctree-suite","agent":"codex","date":"2026-04-27","timestamp":"2026-04-27T15:00:00Z","session_id":"sb","source_chunk":"/tmp/aicx/store/sb.md"}
            ]
        }"#;
        let script = write_mock_aicx(dir.path(), payload);

        {
            crate::aicx::set_aicx_test_opt_in();
            crate::test_env::set_var(crate::aicx::AICX_MODE_ENV, "cli");
            crate::test_env::set_var(crate::aicx::AICX_BINARY_ENV, &script);
        }
        let client = AicxClient::new("loctree-suite");
        let opts = ContextOptions {
            with_aicx: true,
            ..ContextOptions::default()
        };
        let structural = structural_with_target(
            "loctree-rs/src/cli/dispatch/handlers/context.rs",
            &["compose_memory_slice"],
        );
        let memory =
            compose_memory_slice(&opts, &structural, &RuntimeSlice::default(), Some(&client));
        {
            crate::test_env::remove_var(crate::aicx::AICX_BINARY_ENV);
            crate::test_env::remove_var(crate::aicx::AICX_MODE_ENV);
            crate::aicx::clear_aicx_test_opt_in();
        }

        assert_eq!(memory.entries.len(), 2, "both intents should be in scope");
        for entry in &memory.entries {
            assert_eq!(
                entry.retrieval_mode.as_deref(),
                Some("filesystem_fuzzy_fallback"),
                "every memory entry must inherit the envelope's filesystem_fuzzy provenance: {entry:?}"
            );
            // Score / label remain None — the composer reads intents, not
            // search rows. Forward-compat: the day the composer is wired
            // to `client.search()` these will gain values without
            // overwriting `retrieval_mode`.
            assert!(entry.retrieval_score.is_none());
            assert!(entry.retrieval_label.is_none());
        }
        // AICX lib integration: the diagnostic must also surface the
        // semantic readiness. filesystem_fuzzy with loctree_scope_safe=false
        // is the unambiguous "Unsafe" state — the composer must propagate
        // that so the context consumer can decide not to scope on it.
        let diag = memory.diagnostic.as_ref().expect("diagnostic must exist");
        match &diag.semantic_readiness {
            SemanticReadiness::Unsafe { reason } => assert!(
                reason.contains("filesystem_fuzzy") || reason.contains("content index"),
                "unsafe reason must reflect the fuzzy fallback: {reason}"
            ),
            other => panic!(
                "expected Unsafe semantic_readiness for filesystem_fuzzy envelope, got {other:?}"
            ),
        }
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn compose_memory_slice_propagates_ready_state_for_semantic_oracle() {
        // Mirror of the filesystem_fuzzy regression but for the happy path:
        // when AICX serves the request from the embedded semantic index
        // (loctree_scope_safe = true, backend = content_semantic) the
        // memory diagnostic must publish `SemanticReadiness::Ready` so the
        // context consumer knows the slice is safe to scope on.
        let dir = tempfile::tempdir().expect("tempdir");
        let payload = r#"{
            "oracle_status": {
                "source_layer": "layer_2_embedded_semantic",
                "backend": "content_semantic",
                "index_kind": "content_chunks",
                "derived_view": "embedded_semantic_top_k",
                "indexed_count": 5000,
                "scanned_count": 5000,
                "candidate_count": 2,
                "source_paths_verified": true,
                "stale_or_unknown": false,
                "loctree_scope_safe": true,
                "loctree_scope_note": "safe_as_semantic_oracle"
            },
            "results": 2,
            "items": [
                {"kind":"decision","summary":"Adopt compose_memory_slice composer for context handler","project":"loctree-suite","agent":"claude","date":"2026-04-28","timestamp":"2026-04-28T01:00:00Z","session_id":"sa","source_chunk":"/tmp/aicx/store/sa.md"},
                {"kind":"task","summary":"Wire context.rs run() to memory slice","project":"loctree-suite","agent":"codex","date":"2026-04-27","timestamp":"2026-04-27T15:00:00Z","session_id":"sb","source_chunk":"/tmp/aicx/store/sb.md"}
            ]
        }"#;
        let script = write_mock_aicx(dir.path(), payload);

        {
            crate::aicx::set_aicx_test_opt_in();
            crate::test_env::set_var(crate::aicx::AICX_MODE_ENV, "cli");
            crate::test_env::set_var(crate::aicx::AICX_BINARY_ENV, &script);
        }
        let client = AicxClient::new("loctree-suite");
        let opts = ContextOptions {
            with_aicx: true,
            ..ContextOptions::default()
        };
        let structural = structural_with_target(
            "loctree-rs/src/cli/dispatch/handlers/context.rs",
            &["compose_memory_slice"],
        );
        let memory =
            compose_memory_slice(&opts, &structural, &RuntimeSlice::default(), Some(&client));
        {
            crate::test_env::remove_var(crate::aicx::AICX_BINARY_ENV);
            crate::test_env::remove_var(crate::aicx::AICX_MODE_ENV);
            crate::aicx::clear_aicx_test_opt_in();
        }

        let diag = memory.diagnostic.as_ref().expect("diagnostic must exist");
        assert!(
            matches!(diag.semantic_readiness, SemanticReadiness::Ready),
            "embedded semantic envelope must yield Ready, got {:?}",
            diag.semantic_readiness
        );
        for entry in &memory.entries {
            assert_eq!(entry.retrieval_mode.as_deref(), Some("embedded_semantic"));
        }
    }

    /// Wave 6b prep — authority gating on `loctree_scope_safe`.
    ///
    /// Unit-level guard on [`super::gate_authority_on_oracle`]: an AICX-tier
    /// kind authority (`AicxOperator` / `AicxAgent` / `AicxFailure`) MUST
    /// demote to [`AuthorityLabel::SemanticGuess`] when the row's oracle
    /// envelope says the retrieval was not scope-safe. Non-AICX-tier
    /// labels pass through unchanged regardless of envelope shape; a
    /// missing envelope (legacy wire) preserves the kind-based label so
    /// older AICX builds keep working.
    #[test]
    fn gate_authority_on_oracle_demotes_unsafe_aicx_rows() {
        use crate::aicx::{OracleBackend, OracleIndexKind, OracleStatus};

        let unsafe_env = OracleStatus {
            source_layer: "layer_1_canonical_corpus".to_string(),
            backend: OracleBackend::FilesystemFuzzy,
            index_kind: OracleIndexKind::None,
            fallback_reason: Some("content index unavailable".to_string()),
            loctree_scope_safe: false,
            ..OracleStatus::default()
        };
        let safe_env = OracleStatus {
            source_layer: "layer_2_embedded_semantic".to_string(),
            backend: OracleBackend::ContentSemantic,
            index_kind: OracleIndexKind::ContentChunks,
            loctree_scope_safe: true,
            ..OracleStatus::default()
        };

        // AICX-tier kinds demote when the envelope says unsafe.
        for tier in [
            AuthorityLabel::AicxOperator,
            AuthorityLabel::AicxAgent,
            AuthorityLabel::AicxFailure,
        ] {
            assert_eq!(
                super::gate_authority_on_oracle(tier, Some(&unsafe_env)),
                AuthorityLabel::SemanticGuess,
                "{tier:?} must demote to SemanticGuess for fuzzy fallback"
            );
            // Same AICX-tier passes through unchanged on a safe envelope.
            assert_eq!(
                super::gate_authority_on_oracle(tier, Some(&safe_env)),
                tier,
                "{tier:?} must survive a safe envelope"
            );
            // Legacy wire (no envelope) preserves the kind-based tier so
            // pre-oracle AICX builds keep their old labels.
            assert_eq!(
                super::gate_authority_on_oracle(tier, None),
                tier,
                "{tier:?} must survive a missing envelope (legacy wire)"
            );
        }

        // Non-AICX-tier labels never demote — they do not encode an AICX
        // oracle claim in the first place, so the gate is a no-op.
        for label in [
            AuthorityLabel::RepoVerified,
            AuthorityLabel::LoctreeDerived,
            AuthorityLabel::SemanticGuess,
            AuthorityLabel::StaleOrUnknown,
        ] {
            assert_eq!(
                super::gate_authority_on_oracle(label, Some(&unsafe_env)),
                label,
                "non-AICX label {label:?} must pass through unsafe envelope"
            );
        }
    }

    /// Wave 6b prep — end-to-end authority gating through `compose_memory_slice`.
    ///
    /// The kind-based mapping in pack.rs has long promoted `decision` /
    /// `task` rows to `AicxOperator` / `AicxAgent` regardless of how
    /// AICX actually retrieved them. When the wrapper falls back to the
    /// filesystem-fuzzy path the rows are NOT the semantic oracle's
    /// word — they are best-effort literal matches. The authority label
    /// MUST reflect that or downstream agents silently treat fuzzy hits
    /// as canonical operator decisions.
    ///
    /// This test mirrors `compose_memory_slice_populates_retrieval_mode_from_oracle_status`
    /// (same envelope shape) but asserts on the per-entry `authority`
    /// field rather than the per-entry `retrieval_mode`. Closes the
    /// audit finding documented at
    /// `~/.vibecrafted/inbox/Loctree/aicx/blockers/loctree-side-needs.md`.
    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn compose_memory_slice_demotes_authority_when_loctree_scope_unsafe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let payload = r#"{
            "oracle_status": {
                "source_layer": "layer_1_canonical_corpus",
                "backend": "filesystem_fuzzy",
                "index_kind": "none",
                "fallback_reason": "fallback_filesystem_fuzzy: content index unavailable",
                "derived_view": "none_filesystem_scan",
                "store_root": "/tmp/aicx",
                "indexed_count": 0,
                "scanned_count": 42,
                "candidate_count": 2,
                "source_paths_verified": true,
                "stale_or_unknown": true,
                "loctree_scope_safe": false,
                "loctree_scope_note": "unsafe_for_scope_narrowing"
            },
            "results": 2,
            "items": [
                {"kind":"decision","summary":"context composer keeps absolute source_chunk paths","project":"loctree-suite","agent":"claude","date":"2026-04-28","timestamp":"2026-04-28T01:00:00Z","session_id":"sa","source_chunk":"/tmp/aicx/store/sa.md"},
                {"kind":"task","summary":"wire compose_memory_slice authority gating","project":"loctree-suite","agent":"codex","date":"2026-04-27","timestamp":"2026-04-27T15:00:00Z","session_id":"sb","source_chunk":"/tmp/aicx/store/sb.md"}
            ]
        }"#;
        let script = write_mock_aicx(dir.path(), payload);

        {
            crate::aicx::set_aicx_test_opt_in();
            crate::test_env::set_var(crate::aicx::AICX_MODE_ENV, "cli");
            crate::test_env::set_var(crate::aicx::AICX_BINARY_ENV, &script);
        }
        let client = AicxClient::new("loctree-suite");
        let opts = ContextOptions {
            with_aicx: true,
            ..ContextOptions::default()
        };
        let structural = structural_with_target(
            "loctree-rs/src/cli/dispatch/handlers/context.rs",
            &["compose_memory_slice"],
        );
        let memory =
            compose_memory_slice(&opts, &structural, &RuntimeSlice::default(), Some(&client));
        {
            crate::test_env::remove_var(crate::aicx::AICX_BINARY_ENV);
            crate::test_env::remove_var(crate::aicx::AICX_MODE_ENV);
            crate::aicx::clear_aicx_test_opt_in();
        }

        assert_eq!(memory.entries.len(), 2, "both intents should be in scope");
        for entry in &memory.entries {
            assert_eq!(
                entry.authority,
                AuthorityLabel::SemanticGuess,
                "fuzzy-fallback row of kind={} must NOT be carried at \
                 AICX-tier authority — got {:?}, expected SemanticGuess",
                entry.kind,
                entry.authority,
            );
            // Retrieval mode also reflects the fuzzy fallback (no
            // regression on the existing audit-A8 contract).
            assert_eq!(
                entry.retrieval_mode.as_deref(),
                Some("filesystem_fuzzy_fallback"),
            );
        }
    }

    /// Regression test for Issues/context-tool-aicx-overlay-empty-by-default.md
    ///
    /// `compose_memory_slice` must always emit a `diagnostic` so an agent can
    /// distinguish "AICX disabled" from "AICX returned nothing".
    #[test]
    fn memory_diagnostic_disabled_opt_out_when_with_aicx_false() {
        let opts = ContextOptions {
            project: Some(PathBuf::from(".")),
            with_aicx: false,
            no_aicx: false,
            ..ContextOptions::default()
        };
        let structural = StructuralSlice::default();
        let runtime = RuntimeSlice::default();

        let memory = compose_memory_slice(&opts, &structural, &runtime, None);

        let diag = memory.diagnostic.expect("diagnostic populated");
        assert!(!diag.engaged);
        assert_eq!(diag.skip_reason, MemorySkipReason::DisabledOptOut);
        assert!(memory.entries.is_empty());
    }

    /// `no_aicx=true` overrides `with_aicx=true` and reports the explicit
    /// opt-out skip_reason.
    #[test]
    fn memory_diagnostic_disabled_by_no_aicx_takes_precedence() {
        let opts = ContextOptions {
            project: Some(PathBuf::from(".")),
            with_aicx: true,
            no_aicx: true,
            ..ContextOptions::default()
        };
        let memory = compose_memory_slice(
            &opts,
            &StructuralSlice::default(),
            &RuntimeSlice::default(),
            None,
        );
        let diag = memory.diagnostic.expect("diagnostic populated");
        assert!(!diag.engaged);
        assert_eq!(diag.skip_reason, MemorySkipReason::DisabledByNoAicx);
    }

    /// AICX requested but client unavailable — diagnostic must say so loudly.
    #[test]
    fn memory_diagnostic_aicx_unreachable_when_client_missing() {
        let opts = ContextOptions {
            project: Some(PathBuf::from(".")),
            with_aicx: true,
            no_aicx: false,
            ..ContextOptions::default()
        };
        let memory = compose_memory_slice(
            &opts,
            &StructuralSlice::default(),
            &RuntimeSlice::default(),
            None,
        );
        let diag = memory.diagnostic.expect("diagnostic populated");
        assert!(!diag.engaged);
        assert_eq!(diag.skip_reason, MemorySkipReason::AicxUnreachable);
    }

    /// Regression test for Issues/context-tool-action-suggestions-too-narrow.md
    ///
    /// On a bare context call (no `file=`, no `task=`, no `--changed`) the
    /// three commands MUST diversify across structurally distinct verbs and
    /// nouns instead of collapsing onto the same file.
    #[test]
    fn next_safe_commands_on_bare_context_diversifies_verbs_and_nouns() {
        let opts = ContextOptions {
            project: Some(PathBuf::from(".")),
            ..ContextOptions::default()
        };
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        // Build a tiny snapshot with one importer pointing at a "popular"
        // exported symbol so top_imported_public_symbol has something to find.
        let mut importer = FileAnalysis::new("src/cli.py".to_string());
        importer.language = "python".to_string();
        let mut imp = crate::types::ImportEntry::new("src.config".to_string(), ImportKind::Static);
        imp.line = Some(1);
        imp.resolution = crate::types::ImportResolutionKind::Local;
        imp.resolved_path = Some("src/config.py".to_string());
        imp.symbols.push(crate::types::ImportSymbol {
            name: "ScreenScribeConfig".to_string(),
            alias: None,
            is_default: false,
        });
        importer.imports.push(imp);
        snapshot.files.push(importer);

        let mut hub = FileAnalysis::new("src/config.py".to_string());
        hub.language = "python".to_string();
        snapshot.files.push(hub);

        let structural = StructuralSlice {
            entrypoints: vec![StructuralEntrypoint {
                path: "src/server.py".to_string(),
                kinds: vec!["fastapi_app".to_string(), "asgi_target".to_string()],
                authority: AuthorityLabel::RepoVerified,
            }],
            ..StructuralSlice::default()
        };
        let risk = RiskSlice {
            hotspots: vec![HotspotFile {
                file: "src/config.py".to_string(),
                importers: 21,
                authority: AuthorityLabel::LoctreeDerived,
            }],
            ..RiskSlice::default()
        };
        let runtime = RuntimeSlice::default();

        let cmds = next_safe_commands_for(&opts, &structural, &runtime, &risk, &snapshot);
        assert_eq!(cmds.len(), 3, "bare context should produce three commands");

        // Verb diversity: at most one duplicate verb across the triple.
        let verbs: HashSet<&str> = cmds
            .iter()
            .filter_map(|c| c.split_whitespace().nth(1))
            .collect();
        assert!(verbs.len() >= 2, "expected diverse verbs, got: {cmds:?}");

        // Noun diversity: hotspot, entrypoint, and find target should not
        // collapse onto a single file path.
        assert!(
            cmds.iter().any(|c| c.contains("src/server.py")),
            "expected entrypoint surfaced: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| c.contains("src/config.py")),
            "expected hotspot surfaced: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| c.contains("ScreenScribeConfig")),
            "expected top public symbol surfaced: {cmds:?}"
        );
    }

    #[test]
    fn bare_context_filters_empty_default_targets() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        let mut empty = FileAnalysis::new(String::new());
        empty.loc = 500;
        snapshot.files.push(empty);

        let mut real = FileAnalysis::new("src/lib.rs".to_string());
        real.loc = 10;
        snapshot.files.push(real);

        let opts = ContextOptions {
            project: Some(PathBuf::from(".")),
            no_aicx: true,
            ..ContextOptions::default()
        };

        let targets = compose_default_scope(&snapshot, &opts, None, None);
        assert_eq!(targets, vec!["src/lib.rs".to_string()]);

        let pack = compose_context_pack_from_snapshot(&opts, Path::new("."), &snapshot)
            .expect("compose context pack");
        assert!(
            pack.action
                .power_path
                .iter()
                .all(|cmd| cmd.command.trim_end() != "loct slice"),
            "empty file target must not leak into power path: {:?}",
            pack.action.power_path
        );
        assert!(
            pack.structural
                .files
                .iter()
                .all(|file| !file.path.is_empty()),
            "empty file target must not reach structural slice: {:?}",
            pack.structural.files
        );
    }

    #[test]
    fn bare_context_prioritizes_runtime_entrypoints_over_large_noise_files() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        for index in 0..12 {
            let mut noise = FileAnalysis::new(format!("Sources/Noise{index}.swift"));
            noise.language = "swift".to_string();
            noise.loc = 10_000 - index;
            snapshot.files.push(noise);
        }

        let app_path = "Pensieve/Sources/Pensieve/App/PensieveApp.swift";
        let mut app = FileAnalysis::new(app_path.to_string());
        app.language = "swift".to_string();
        app.loc = 1;
        app.entry_points.push("swift-main".to_string());
        snapshot.files.push(app);
        snapshot.metadata.entrypoints.push(EntrypointSummary {
            path: app_path.to_string(),
            kinds: vec!["swift-main".to_string()],
        });

        let opts = ContextOptions {
            project: Some(PathBuf::from(".")),
            no_aicx: true,
            ..ContextOptions::default()
        };
        let targets = compose_default_scope(&snapshot, &opts, None, None);
        assert!(
            targets.iter().any(|target| target == app_path),
            "runtime entrypoint lost behind LOC ranking: {targets:?}"
        );
    }

    /// `likely_tests_for` must rank tests by symbol-overlap relevance, not
    /// alphabet.
    #[test]
    fn likely_tests_ranks_by_symbol_overlap_not_alphabet() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);

        let mut config = FileAnalysis::new("src/config.py".to_string());
        config.language = "python".to_string();
        config.exports.push(crate::types::ExportSymbol::new(
            "ScreenScribeConfig".to_string(),
            "class",
            "named",
            Some(10),
        ));
        snapshot.files.push(config);

        // High-overlap test: imports the actual exported symbol AND resolves
        // to the scope file.
        let mut t_high = FileAnalysis::new("tests/test_config_env.py".to_string());
        t_high.language = "python".to_string();
        t_high.is_test = true;
        let mut imp_high =
            crate::types::ImportEntry::new("src.config".to_string(), ImportKind::Static);
        imp_high.line = Some(1);
        imp_high.resolution = crate::types::ImportResolutionKind::Local;
        imp_high.resolved_path = Some("src/config.py".to_string());
        imp_high.symbols.push(crate::types::ImportSymbol {
            name: "ScreenScribeConfig".to_string(),
            alias: None,
            is_default: false,
        });
        t_high.imports.push(imp_high);
        snapshot.files.push(t_high);

        // Low-overlap test: alphabetically earlier but unrelated to scope.
        let mut t_low = FileAnalysis::new("tests/test_audio.py".to_string());
        t_low.language = "python".to_string();
        t_low.is_test = true;
        snapshot.files.push(t_low);

        let scope = vec!["src/config.py".to_string()];
        let tests = likely_tests_for(&scope, &snapshot, 5);

        // Must surface the high-overlap test even though it's alphabetically
        // *later* than test_audio.
        assert!(!tests.is_empty(), "should produce at least one ranked test");
        assert_eq!(
            tests[0], "tests/test_config_env.py",
            "highest-overlap test should rank first, got: {tests:?}"
        );
    }

    // -----------------------------------------------------------------
    // L02 / Findings #1 #5 #11 #12 — dedup, cache_scope_authority match,
    // memory entry quality fields, score==0 newest-fallback.
    // -----------------------------------------------------------------

    #[test]
    fn cache_scope_authority_unknown_maps_to_stale_or_unknown() {
        assert_eq!(
            cache_scope_authority(&RiskCacheScope::Unknown),
            AuthorityLabel::StaleOrUnknown
        );
    }

    #[test]
    fn cache_scope_authority_clean_maps_to_repo_verified() {
        for scope in [
            RiskCacheScope::Clean,
            RiskCacheScope::DirtyWorktree,
            RiskCacheScope::StaleSnapshot,
            RiskCacheScope::MissingSnapshot,
        ] {
            assert_eq!(
                cache_scope_authority(&scope),
                AuthorityLabel::RepoVerified,
                "{scope:?}"
            );
        }
    }

    #[test]
    fn memory_entry_serde_back_compat_with_old_payload() {
        // v0.9 wire format had only the legacy fields (no `retrieval_*`,
        // no `low_lexical_match`). The struct must still deserialize.
        let legacy_json = r#"{
            "kind": "decision",
            "text": "Adopt the pill renderer",
            "authority": "aicx_operator",
            "source_chunk": "/tmp/aicx/store/legacy.md",
            "agent": "claude",
            "date": "2026-04-01",
            "session_id": "leg",
            "project": "loctree-suite",
            "relevance": 4
        }"#;
        let entry: MemoryEntry =
            serde_json::from_str(legacy_json).expect("legacy payload deserializes");
        assert_eq!(entry.relevance, 4);
        assert!(entry.retrieval_score.is_none());
        assert!(entry.retrieval_label.is_none());
        assert!(entry.retrieval_mode.is_none());
        assert!(!entry.low_lexical_match);
    }

    #[test]
    fn memory_entry_serde_skips_default_quality_fields_in_output() {
        // Backward compat at the producer side: when the new fields are
        // unset, they must be omitted from JSON output so existing
        // consumers (memory_lint, reports, MCP) see no schema drift.
        let entry = MemoryEntry {
            kind: "decision".to_string(),
            text: "test".to_string(),
            authority: AuthorityLabel::AicxOperator,
            source_chunk: "/tmp/aicx/store/x.md".to_string(),
            agent: "claude".to_string(),
            date: "2026-04-28".to_string(),
            timestamp: None,
            session_id: "x".to_string(),
            project: "loctree-suite".to_string(),
            relevance: 1,
            retrieval_score: None,
            retrieval_label: None,
            retrieval_mode: None,
            low_lexical_match: false,
        };
        let json = serde_json::to_string(&entry).expect("serializes");
        assert!(!json.contains("retrieval_score"), "{json}");
        assert!(!json.contains("retrieval_label"), "{json}");
        assert!(!json.contains("retrieval_mode"), "{json}");
        assert!(!json.contains("low_lexical_match"), "{json}");
        assert!(json.contains("\"relevance\":1"));
    }

    fn w5_01_leaky_overlay() -> crate::aicx::overlay::OverlayRenderState {
        crate::aicx::overlay::OverlayRenderState {
            schema_version: "loctree.overlay.intent.v1".to_string(),
            repo_id: "loctree-suite".to_string(),
            store_revision: format!("sr1:{}", "a".repeat(64)),
            overlay_revision: format!("ov1:{}", "b".repeat(64)),
            snapshot_commit: "abc1234".to_string(),
            anchor_catalog_revision: format!("acr1:{}", "c".repeat(64)),
            producer_version: "aicx 0.12.2-test".to_string(),
            freshness: crate::aicx::overlay::OverlayFreshness::Fresh,
            key_transition: None,
            theses: vec![
                "token sk-test_abcdefghijklmnopqrstuvwxyz12".to_string(),
                "-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAK8=\n-----END RSA PRIVATE KEY-----"
                    .to_string(),
                "wrote /Users/alice/.ssh/id_rsa".to_string(),
            ],
            scope_paths: Vec::new(),
            refresh_command: "aicx overlay --repo /tmp/loctree --format json".to_string(),
            refresh_recommended: false,
        }
    }

    #[test]
    fn w5_01_aicx_overlay_redacts_secrets() {
        let mut pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some("/tmp/proj".to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: None,
        });
        pack.memory.entries.push(MemoryEntry {
            kind: "decision".to_string(),
            text: "token sk-test_abcdefghijklmnopqrstuvwxyz12 pem -----BEGIN RSA PRIVATE KEY----- MIIBOgIBAAJBAK8= -----END RSA PRIVATE KEY----- path /Users/alice/.ssh/id_rsa".to_string(),
            authority: AuthorityLabel::AicxOperator,
            source_chunk: "/tmp/aicx/store/s1.md".to_string(),
            agent: "claude".to_string(),
            date: "2026-09-19".to_string(),
            timestamp: None,
            session_id: "s1".to_string(),
            project: "loctree-suite".to_string(),
            relevance: 9,
            retrieval_score: None,
            retrieval_label: None,
            retrieval_mode: None,
            low_lexical_match: false,
        });
        pack.memory.overlay = Some(w5_01_leaky_overlay());
        scrub_context_pack(&mut pack);

        let md = format_context_pack_markdown(&pack);
        eprintln!(
            "W5-01 runtime proof: redactions={} markdown has [redacted]={}",
            pack.receipt.redactions,
            md.contains("[redacted]")
        );
        assert!(
            pack.receipt.redactions >= 3,
            "receipt must count overlay+entry redactions, got {}",
            pack.receipt.redactions
        );
        assert!(
            md.contains("[redacted]"),
            "pack markdown must show [redacted]: {md}"
        );
        assert!(
            md.contains("**Redactions**:"),
            "receipt markdown must list the redaction count: {md}"
        );
        assert!(
            !md.contains("sk-test_"),
            "fake token must not survive pack markdown: {md}"
        );
        assert!(
            !md.contains("BEGIN RSA"),
            "PEM material must not survive pack markdown: {md}"
        );
        assert!(
            !md.contains("/Users/alice"),
            "abs home path must not survive pack markdown: {md}"
        );
        let overlay = pack.memory.overlay.as_ref().expect("overlay");
        let joined = overlay.theses.join("\n");
        assert!(joined.contains("[redacted]"), "{joined}");
        assert!(!joined.contains("sk-test_"), "{joined}");
        assert!(!joined.contains("BEGIN RSA"), "{joined}");
        assert!(!joined.contains("/Users/alice"), "{joined}");
        assert_eq!(
            overlay.store_revision,
            format!("sr1:{}", "a".repeat(64)),
            "hex overlay revision must not be treated as base64"
        );
    }

    #[test]
    fn w5_01_pack_advertises_aicx_read_only() {
        let mut pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some("/tmp/proj".to_string()),
            branch: Some("main".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: None,
        });
        pack.memory.source_chunks = vec!["/tmp/aicx/store/s1.md".to_string()];
        let md = format_context_pack_markdown(&pack);
        assert!(
            md.contains("`aicx read <chunk:ref>`"),
            "pack must advertise aicx read: {md}"
        );
        assert!(
            !md.contains("aicx open"),
            "pack must not advertise aicx open: {md}"
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn compose_memory_slice_dedups_identical_text_from_same_chunk() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Two intents with identical text from the same chunk plus one
        // distinct intent. Without dedup the slice would surface the dup
        // pair twice and crowd out distinct decisions when the limit is small.
        let payload = r#"[
            {"kind":"decision","summary":"compose context handler memory slice","project":"loctree-suite","agent":"claude","date":"2026-04-28","timestamp":"2026-04-28T01:00:00Z","session_id":"s1","source_chunk":"/tmp/aicx/store/dup.md"},
            {"kind":"task","summary":"compose context handler memory slice","project":"loctree-suite","agent":"claude","date":"2026-04-28","timestamp":"2026-04-28T01:00:30Z","session_id":"s1b","source_chunk":"/tmp/aicx/store/dup.md"},
            {"kind":"intent","summary":"document context flag --with-aicx","project":"loctree-suite","agent":"codex","date":"2026-04-28","timestamp":"2026-04-28T01:01:00Z","session_id":"s2","source_chunk":"/tmp/aicx/store/distinct.md"}
        ]"#;
        let script = write_mock_aicx(dir.path(), payload);

        {
            crate::aicx::set_aicx_test_opt_in();
            crate::test_env::set_var(crate::aicx::AICX_MODE_ENV, "cli");
            crate::test_env::set_var(crate::aicx::AICX_BINARY_ENV, &script);
        }
        let client = AicxClient::new("loctree-suite");
        let opts = ContextOptions {
            with_aicx: true,
            ..ContextOptions::default()
        };
        let structural = structural_with_target(
            "loctree-rs/src/cli/dispatch/handlers/context.rs",
            &["compose_memory_slice"],
        );
        let memory =
            compose_memory_slice(&opts, &structural, &RuntimeSlice::default(), Some(&client));
        {
            crate::test_env::remove_var(crate::aicx::AICX_BINARY_ENV);
            crate::test_env::remove_var(crate::aicx::AICX_MODE_ENV);
            crate::aicx::clear_aicx_test_opt_in();
        }

        let texts: Vec<&str> = memory.entries.iter().map(|e| e.text.as_str()).collect();
        let dup_count = texts
            .iter()
            .filter(|t| t.contains("compose context handler memory slice"))
            .count();
        assert_eq!(
            dup_count, 1,
            "dedup must collapse identical (text, source_chunk) to one entry, got: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("document context flag")),
            "distinct intent must remain after dedup, got: {texts:?}"
        );
    }

    #[test]
    #[serial_test::serial(memory_raw_limit_env)]
    fn raw_limit_scales_with_hours_when_env_unset() {
        {
            crate::test_env::remove_var("LOCT_CONTEXT_MEMORY_RAW_LIMIT");
        }
        // 7-day window (168 h) → 84 → bumped up by limit*2 floor (50*2=100)
        // → final 100. Stays close to legacy default.
        assert_eq!(memory_raw_limit(168, 50), 100);
        // 30-day window → 360 (above limit*2 floor of 100) → 360.
        assert_eq!(memory_raw_limit(720, 50), 360);
        // 90-day window → clamp ceiling at 1000.
        assert_eq!(memory_raw_limit(2160, 50), 1000);
        // 1-day window — clamp floor at 50, but limit*2 = 100 wins.
        assert_eq!(memory_raw_limit(24, 50), 100);
        // Tiny limit, big window — still scaled.
        assert_eq!(memory_raw_limit(720, 5), 360);
    }

    #[test]
    #[serial_test::serial(memory_raw_limit_env)]
    fn raw_limit_env_override_takes_precedence() {
        {
            crate::test_env::set_var("LOCT_CONTEXT_MEMORY_RAW_LIMIT", "777");
        }
        let v = memory_raw_limit(168, 50);
        {
            crate::test_env::remove_var("LOCT_CONTEXT_MEMORY_RAW_LIMIT");
        }
        assert_eq!(v, 777, "env override must win over scaling default");
    }

    #[test]
    #[serial_test::serial(memory_raw_limit_env)]
    fn raw_limit_env_zero_falls_back_to_scaling() {
        {
            crate::test_env::set_var("LOCT_CONTEXT_MEMORY_RAW_LIMIT", "0");
        }
        let v = memory_raw_limit(720, 50);
        {
            crate::test_env::remove_var("LOCT_CONTEXT_MEMORY_RAW_LIMIT");
        }
        assert_eq!(
            v, 360,
            "zero env value must be ignored — scaling default kicks in"
        );
    }

    #[test]
    fn aicx_project_bucket_override_wins_over_file_name_guess() {
        // Override is highest-priority — even when the snapshot root
        // exists and would normally produce a different bucket name.
        let opts = ContextOptions {
            project: Some(PathBuf::from("/tmp/some-monorepo-fixture")),
            aicx_project_override: Some("loctree-suite".to_string()),
            ..ContextOptions::default()
        };
        assert_eq!(aicx_project_bucket(&opts), "loctree-suite");
    }

    #[test]
    fn aicx_project_bucket_blank_override_is_ignored() {
        // Whitespace-only override is treated as absent — the resolver
        // falls through to the snapshot-root heuristic. We assert only
        // that the override does NOT short-circuit (bucket is not the
        // blank string).
        let opts = ContextOptions {
            aicx_project_override: Some("   ".to_string()),
            ..ContextOptions::default()
        };
        let bucket = aicx_project_bucket(&opts);
        assert_ne!(bucket.trim(), "");
        assert_ne!(bucket, "   ");
    }

    #[test]
    fn aicx_project_bucket_trims_override() {
        let opts = ContextOptions {
            aicx_project_override: Some("  loctree-suite  ".to_string()),
            ..ContextOptions::default()
        };
        assert_eq!(aicx_project_bucket(&opts), "loctree-suite");
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn compose_memory_slice_falls_back_to_newest_when_score_all_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Intent payload whose tokens have ZERO overlap with the
        // structural scope keywords. Without the fallback, the slice
        // would be empty even though AICX has a bag of recent rows.
        let payload = r#"[
            {"kind":"decision","summary":"Refactor stripe payment webhook handler","project":"loctree-suite","agent":"claude","date":"2026-04-28","timestamp":"2026-04-28T01:00:00Z","session_id":"sx","source_chunk":"/tmp/aicx/store/sx.md"},
            {"kind":"task","summary":"Update billing pricing copy","project":"loctree-suite","agent":"codex","date":"2026-04-27","timestamp":"2026-04-27T01:00:00Z","session_id":"sy","source_chunk":"/tmp/aicx/store/sy.md"}
        ]"#;
        let script = write_mock_aicx(dir.path(), payload);

        {
            crate::aicx::set_aicx_test_opt_in();
            crate::test_env::set_var(crate::aicx::AICX_MODE_ENV, "cli");
            crate::test_env::set_var(crate::aicx::AICX_BINARY_ENV, &script);
        }
        let client = AicxClient::new("loctree-suite");
        let opts = ContextOptions {
            with_aicx: true,
            ..ContextOptions::default()
        };
        // Tight scope unrelated to stripe / billing keywords so every
        // intent scores 0.
        let structural = structural_with_target(
            "loctree-rs/src/aicx/intents.rs",
            &["score_intent", "authority_for_intent"],
        );
        let memory =
            compose_memory_slice(&opts, &structural, &RuntimeSlice::default(), Some(&client));
        {
            crate::test_env::remove_var(crate::aicx::AICX_BINARY_ENV);
            crate::test_env::remove_var(crate::aicx::AICX_MODE_ENV);
            crate::aicx::clear_aicx_test_opt_in();
        }

        assert!(
            !memory.entries.is_empty(),
            "score==0 fallback must surface newest entries, got empty slice"
        );
        assert!(
            memory.entries.iter().all(|e| e.low_lexical_match),
            "every fallback entry must carry low_lexical_match=true"
        );
        assert!(
            memory.entries.iter().all(|e| e.relevance == 0),
            "fallback entries keep relevance=0"
        );
        // Newest first.
        assert_eq!(
            memory.entries.first().map(|e| e.session_id.as_str()),
            Some("sx"),
            "newest session_id must be first under recency fallback"
        );
    }
}
