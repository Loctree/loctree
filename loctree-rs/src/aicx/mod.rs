//! Read-only consumer wrapper around the external `aicx` memory surfaces.
//!
//! Loctree never panics or fails when AICX is missing. By default the wrapper
//! tries `aicx-mcp --transport stdio` first and falls back to the installed
//! `aicx` CLI when MCP is unavailable. `LOCT_AICX_MODE=cli|mcp|auto` can force
//! the transport.
//!
//! Cache: each [`AicxClient`] holds its own per-instance cache, keyed by the
//! call signature (window/filters/query). The cache is shared across all
//! Loctree slice composers that operate inside a single `loct context`
//! invocation, so multiple consumers do not re-shell-out for the same window.
//!
//! See `LOCTREE_NEXT.md` Lane 3 ("Memory slice — AICX overlay") for the
//! product context: AICX is the third leg of the agent-pack, alongside the
//! structural and runtime/risk overlays.
//!
//! # Runtime discovery
//!
//! The CLI fallback resolves the `aicx` binary in this order:
//! 1. `LOCT_AICX_BINARY` env var (used by tests and operators with a
//!    custom install location).
//! 2. `aicx` on `PATH` (standard install path).
//!
//! The MCP transport resolves `aicx-mcp` with `AICX_MCP_BINARY`, then `PATH`.
//!
//! When neither transport can produce data, the wrapper returns an empty result.

mod intent_source;
pub mod intents;
mod mcp;
mod shell;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub use intent_source::{CliIntentSource, IntentSource};
pub use intents::{
    AuthorityResolution, IntentAuthority, ScopeKeywords, authority_for_intent, classify_phrase_hit,
    resolve_authority, score_intent,
};
pub use mcp::{AICX_MCP_BINARY_ENV, AICX_MODE_ENV};
pub use shell::{AICX_BINARY_ENV, AICX_TIMEOUT_ENV, DEFAULT_TIMEOUT};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySummary {
    pub text: String,
    pub structured: bool,
}

/// Convert an AICX memory payload into a one-line human summary.
///
/// AICX history can contain tool payloads or raw JSON blobs. The context pill
/// must point back to the source chunk for raw detail instead of pasting those
/// blobs into prose.
pub fn summarize_entry(raw: &str) -> MemorySummary {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return MemorySummary {
            text: "raw AICX entry".to_string(),
            structured: false,
        };
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Some(summary) = summarize_json_value(&value)
    {
        return summary;
    }

    summarize_plain_text(trimmed)
}

fn summarize_plain_text(raw: &str) -> MemorySummary {
    let one_line = collapse_ws(raw);
    if looks_like_raw_blob(raw) {
        return MemorySummary {
            text: "raw AICX entry".to_string(),
            structured: false,
        };
    }
    MemorySummary {
        text: truncate_summary(&one_line),
        structured: true,
    }
}

fn summarize_json_value(value: &serde_json::Value) -> Option<MemorySummary> {
    match value {
        serde_json::Value::Object(map) => {
            for key in ["summary", "text", "message", "title"] {
                if let Some(text) = map.get(key).and_then(serde_json::Value::as_str) {
                    let summary = summarize_plain_text(text);
                    if summary.structured {
                        return Some(summary);
                    }
                }
            }

            let mut files = Vec::new();
            collect_file_hints(value, &mut files);
            files.sort();
            files.dedup();
            let files = files.into_iter().take(3).collect::<Vec<_>>().join(", ");

            if let Some(diff) = map.get("unified_diff").and_then(serde_json::Value::as_str) {
                let (added, removed) = diff_line_counts(diff);
                let mut diff_files = files_from_diff(diff);
                if diff_files.is_empty() && !files.is_empty() {
                    diff_files.push(files);
                }
                diff_files.sort();
                diff_files.dedup();
                let scope = if diff_files.is_empty() {
                    "source chunk".to_string()
                } else {
                    diff_files
                        .into_iter()
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                return Some(MemorySummary {
                    text: truncate_summary(&format!(
                        "updated {scope} from AICX diff (+{added}/-{removed})"
                    )),
                    structured: true,
                });
            }

            if map.contains_key("call_id") || map.contains_key("tool") || map.contains_key("args") {
                let subject = if files.is_empty() {
                    "tool call".to_string()
                } else {
                    format!("tool call touching {files}")
                };
                return Some(MemorySummary {
                    text: truncate_summary(&subject),
                    structured: true,
                });
            }

            Some(MemorySummary {
                text: "raw AICX entry".to_string(),
                structured: false,
            })
        }
        serde_json::Value::Array(items) => Some(MemorySummary {
            text: format!("structured AICX entry list ({} items)", items.len()),
            structured: true,
        }),
        serde_json::Value::String(text) => Some(summarize_plain_text(text)),
        _ => Some(MemorySummary {
            text: "raw AICX entry".to_string(),
            structured: false,
        }),
    }
}

fn looks_like_raw_blob(raw: &str) -> bool {
    let lowered = raw.to_ascii_lowercase();
    lowered.contains("unified_diff")
        || lowered.contains("\"call_id\"")
        || lowered.contains("@@")
        || (raw.lines().count() > 2 && raw.len() > 280)
        || ((raw.starts_with('{') || raw.starts_with('[')) && raw.len() > 120)
}

fn collect_file_hints(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                let key = key.as_str();
                if matches!(
                    key,
                    "file" | "path" | "source_file" | "target_file" | "changed_file"
                ) && let Some(path) = value.as_str()
                {
                    push_file_hint(path, out);
                }
                if matches!(key, "files" | "paths" | "changed_files")
                    && let Some(items) = value.as_array()
                {
                    for item in items {
                        if let Some(path) = item.as_str() {
                            push_file_hint(path, out);
                        }
                    }
                }
                if key == "unified_diff"
                    && let Some(diff) = value.as_str()
                {
                    out.extend(files_from_diff(diff));
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_file_hints(item, out);
            }
        }
        _ => {}
    }
}

fn push_file_hint(path: &str, out: &mut Vec<String>) {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/dev/null" {
        return;
    }
    out.push(
        trimmed
            .trim_start_matches("a/")
            .trim_start_matches("b/")
            .to_string(),
    );
}

fn files_from_diff(diff: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ b/") {
            push_file_hint(rest, &mut files);
        } else if let Some(rest) = line.strip_prefix("--- a/") {
            push_file_hint(rest, &mut files);
        } else if let Some(rest) = line.strip_prefix("diff --git a/")
            && let Some((_, b_path)) = rest.split_once(" b/")
        {
            push_file_hint(b_path, &mut files);
        }
    }
    files.sort();
    files.dedup();
    files
}

fn diff_line_counts(diff: &str) -> (usize, usize) {
    let mut added = 0usize;
    let mut removed = 0usize;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

fn collapse_ws(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_summary(raw: &str) -> String {
    const MAX: usize = 220;
    let compact = collapse_ws(raw);
    if compact.chars().count() <= MAX {
        return compact;
    }
    let mut out = compact
        .chars()
        .take(MAX.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

/// Probe whether AICX can serve local memory.
///
/// This is intentionally a cheap CLI probe. The MCP transport can still be
/// healthy even when this returns `false`; callers that need real data should
/// construct [`AicxClient`] and let it try MCP first.
pub fn is_aicx_available() -> bool {
    shell::is_aicx_available()
}

/// Single intent/decision/outcome/task entry extracted by `aicx intents`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AicxIntent {
    /// One of `decision`, `intent`, `outcome`, `task`.
    pub kind: String,
    /// Short summary text of the intent.
    pub text: String,
    /// Authoring agent (`claude`, `codex`, `gemini`, ...).
    pub agent: String,
    /// ISO date of the source chunk, e.g. `2026-04-28`.
    pub date: String,
    /// Full timestamp when available.
    #[serde(default)]
    pub timestamp: Option<String>,
    /// Session id segment (truncated form used by AICX).
    pub session_id: String,
    /// AICX project bucket (e.g. `loctree-suite`).
    pub project: String,
    /// Absolute path to the source markdown chunk inside `~/.aicx/store/...`.
    pub source_chunk_path: String,
    /// Frame kind when available (`user_msg`, `agent_reply`, `tool_call`, ...).
    #[serde(default)]
    pub frame_kind: Option<String>,
    /// Retrieval-layer provenance from the AICX response envelope. Populated
    /// by the MCP / CLI parsers when AICX emits an oracle-aware envelope.
    /// `None` when the wire payload predates oracle envelopes. See
    /// [`OracleStatus`] for the mapping into a stable `retrieval_mode` string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oracle_status: Option<OracleStatus>,
}

/// Steering filters accepted by `aicx steer`.
///
/// All fields are optional. `Default::default()` returns an empty filter set
/// (caller-side limit/sort still apply via [`AicxClient::steer`]).
#[derive(Debug, Clone, Default, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerFilters {
    pub run_id: Option<String>,
    pub prompt_id: Option<String>,
    pub kind: Option<String>,
    pub agent: Option<String>,
    pub date: Option<String>,
    pub frame_kind: Option<String>,
    pub limit: Option<usize>,
}

/// One row from `aicx steer` (3-line text block parsed into a struct).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AicxSteerResult {
    /// Repo bucket, e.g. `Loctree/loctree-suite`.
    pub project: String,
    pub agent: String,
    pub date: String,
    /// Bucket kind: `conversations`, `plans`, `reports`, `other`.
    pub kind: String,
    pub run_id: Option<String>,
    pub prompt_id: Option<String>,
    pub model: Option<String>,
    pub source_chunk_path: String,
}

/// One row from `aicx search --json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AicxSearchResult {
    pub score: i64,
    pub label: Option<String>,
    pub project: String,
    pub agent: String,
    pub date: String,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub frame_kind: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    /// Matched evidence snippets (already truncated by aicx).
    #[serde(default)]
    pub matches: Vec<String>,
    /// Absolute path to the source chunk.
    pub path: String,
    /// Retrieval-layer provenance from the AICX response envelope. AICX
    /// places `oracle_status` at the top level of `aicx_search` output —
    /// the parsers in [`shell::parse_search`] copy it onto every row so
    /// downstream callers can interrogate per-result without re-modelling
    /// the envelope shape. `None` when the wire payload predates oracle
    /// envelopes (closes audit finding A8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oracle_status: Option<OracleStatus>,
}

/// Loctree-side mirror of `aicx::oracle::OracleStatus`.
///
/// AICX exposes this struct at the top of `aicx_search` and `aicx_intents`
/// envelopes so callers can tell whether the answer came from the embedded
/// semantic index, the canonical-corpus chunk scan, the steer-metadata
/// index, or the filesystem-fuzzy fallback. Loctree does not depend on
/// the upstream type directly — that crate marks it `Serialize`-only —
/// so we keep a `Deserialize`-capable mirror here. Forward-compat: every
/// field is `#[serde(default)]` and unknown enum variants fall through to
/// [`OracleBackend::Unknown`] / [`OracleIndexKind::Unknown`] instead of
/// failing the whole parse.
///
/// See `aicx/src/oracle.rs` for the canonical definition. Closes audit
/// finding A8.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OracleStatus {
    /// e.g. `layer_1_canonical_corpus`. Free-form layer label.
    #[serde(default)]
    pub source_layer: String,
    /// Backend kind that served the response.
    #[serde(default)]
    pub backend: OracleBackend,
    /// Index kind backing the backend (or `None` for fallback paths).
    #[serde(default)]
    pub index_kind: OracleIndexKind,
    /// Reason explaining a degraded path. `Some(...)` whenever the
    /// canonical retrieval target was unavailable and AICX dropped to a
    /// lower-quality layer (e.g. `fallback_filesystem_fuzzy: content
    /// index unavailable`). Always propagated unmodified.
    #[serde(default)]
    pub fallback_reason: Option<String>,
    /// Free-form description of the derived view AICX returned (e.g.
    /// `none_filesystem_scan`).
    #[serde(default)]
    pub derived_view: String,
    /// Number of items present in the index AICX consulted.
    #[serde(default)]
    pub indexed_count: usize,
    /// Number of items AICX walked while serving the query.
    #[serde(default)]
    pub scanned_count: usize,
    /// Number of candidate items returned to the caller.
    #[serde(default)]
    pub candidate_count: usize,
    /// `true` when AICX verified that every returned `path` exists on disk.
    #[serde(default)]
    pub source_paths_verified: bool,
    /// `true` when AICX considers the answer stale or otherwise not
    /// authoritative — typically set whenever `fallback_reason` is `Some`.
    #[serde(default)]
    pub stale_or_unknown: bool,
    /// `true` when AICX considers the answer safe to use for narrowing
    /// Loctree scope. `false` for fuzzy fallback (use as routing evidence
    /// only).
    #[serde(default)]
    pub loctree_scope_safe: bool,
}

/// Backend kind that served an AICX response (mirror of
/// `aicx::oracle::OracleBackend`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleBackend {
    /// Layer 1 canonical-corpus chunk scan (no semantic index).
    CanonicalCorpus,
    /// Filesystem fuzzy scan — fallback when no semantic / metadata index
    /// is available.
    FilesystemFuzzy,
    /// Steer metadata index (rebuildable from canonical chunks).
    SteerMetadata,
    /// Embedded content semantic index — the highest-quality retrieval path.
    ContentSemantic,
    /// Hybrid retrieval (multiple backends combined).
    Hybrid,
    /// Forward-compat catch-all so a new AICX backend variant does not
    /// fail loctree parsing — the consumer treats this as `unknown`.
    #[default]
    #[serde(other)]
    Unknown,
}

/// Index kind backing an AICX retrieval (mirror of
/// `aicx::oracle::OracleIndexKind`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleIndexKind {
    /// No index — typically paired with [`OracleBackend::FilesystemFuzzy`].
    None,
    MetadataSteer,
    CanonicalChunks,
    ContentChunks,
    OnionContent,
    /// Forward-compat catch-all (see [`OracleBackend::Unknown`]).
    #[default]
    #[serde(other)]
    Unknown,
}

impl OracleBackend {
    /// Stable string projection used by `MemoryEntry.retrieval_mode`.
    ///
    /// The mapping is intentionally human-readable (rather than just the
    /// snake-case discriminant) so reports/UI can surface the failure mode
    /// directly: `FilesystemFuzzy` becomes `filesystem_fuzzy_fallback`
    /// (the `_fallback` suffix mirrors AICX's own messaging in
    /// `aicx::search_engine`), and `ContentSemantic` becomes
    /// `embedded_semantic` (the canonical name operators use to refer to
    /// the embedded layer).
    pub fn as_retrieval_mode(self) -> &'static str {
        match self {
            Self::CanonicalCorpus => "canonical_corpus",
            Self::FilesystemFuzzy => "filesystem_fuzzy_fallback",
            Self::SteerMetadata => "steer_metadata",
            Self::ContentSemantic => "embedded_semantic",
            Self::Hybrid => "hybrid",
            Self::Unknown => "unknown",
        }
    }
}

impl OracleStatus {
    /// Stable retrieval-mode label suitable for `MemoryEntry.retrieval_mode`.
    /// Convenience wrapper over [`OracleBackend::as_retrieval_mode`].
    pub fn retrieval_mode(&self) -> &'static str {
        self.backend.as_retrieval_mode()
    }

    /// Classify this oracle response into one of three operator-meaningful
    /// readiness states: ready (semantic backend served the answer and the
    /// scope is safe), degraded (canonical / steer / hybrid backend or stale
    /// index — still usable, but not the semantic oracle), or unsafe (fuzzy
    /// fallback or explicit `loctree_scope_safe == false` — must not be used
    /// to narrow Loctree scope, only as routing evidence).
    ///
    /// AICX's CLI/MCP contract guarantees that when the semantic vector
    /// index can serve a query it returns `backend = ContentSemantic` with
    /// `loctree_scope_safe = true`. Any other shape means the canonical
    /// path was unavailable and AICX fell back to a lower-quality layer.
    pub fn readiness(&self) -> SemanticReadiness {
        if !self.loctree_scope_safe || matches!(self.backend, OracleBackend::FilesystemFuzzy) {
            return SemanticReadiness::Unsafe {
                reason: self
                    .fallback_reason
                    .clone()
                    .unwrap_or_else(|| self.backend.as_retrieval_mode().to_string()),
            };
        }
        if matches!(self.backend, OracleBackend::ContentSemantic) && !self.stale_or_unknown {
            return SemanticReadiness::Ready;
        }
        let reason = self
            .fallback_reason
            .clone()
            .unwrap_or_else(|| self.backend.as_retrieval_mode().to_string());
        SemanticReadiness::Degraded { reason }
    }
}

/// Three operator-meaningful readiness states derived from
/// [`OracleStatus`]. Use this on Loctree context/memory diagnostics so the
/// agent can tell whether the AICX answer came from the semantic oracle
/// (safe to act on), a degraded but still-canonical layer (use, but flag),
/// or an unsafe fuzzy fallback (route-only, do not scope on).
///
/// Mapping rules (mirror the AICX `aicx::oracle::OracleStatus` contract):
///
/// - [`SemanticReadiness::Ready`] — `backend == ContentSemantic` and
///   `loctree_scope_safe == true` and `stale_or_unknown == false`. The
///   embedded semantic index served the request.
/// - [`SemanticReadiness::Degraded`] — canonical / steer / hybrid backend,
///   or any answer flagged `stale_or_unknown == true`. The retrieval is
///   trustworthy enough to inform scope, but is not a semantic oracle.
/// - [`SemanticReadiness::Unsafe`] — `backend == FilesystemFuzzy` or
///   `loctree_scope_safe == false`. Loctree must treat the rows as routing
///   evidence only; never as authoritative scope narrowing.
/// - [`SemanticReadiness::Unknown`] — no `OracleStatus` arrived (legacy
///   AICX build or no calls completed yet).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SemanticReadiness {
    Ready,
    Degraded {
        reason: String,
    },
    Unsafe {
        reason: String,
    },
    #[default]
    Unknown,
}

impl SemanticReadiness {
    /// Stable short-string projection for telemetry / wire shapes that
    /// prefer a flat tag over a tagged enum.
    pub fn as_tag(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Degraded { .. } => "degraded",
            Self::Unsafe { .. } => "unsafe",
            Self::Unknown => "unknown",
        }
    }

    /// True when the answer is the semantic oracle. False for every other
    /// state — operators should NOT scope Loctree on degraded/unsafe rows.
    pub fn is_semantic_oracle(&self) -> bool {
        matches!(self, Self::Ready)
    }

    /// Roll up two readiness observations into the lower-trust one. Used
    /// when a memory slice aggregates rows from multiple AICX calls (e.g.
    /// intents + search): the slice as a whole is only as ready as its
    /// least-ready component.
    pub fn min(self, other: Self) -> Self {
        let rank = |state: &Self| match state {
            Self::Ready => 3,
            Self::Degraded { .. } => 2,
            Self::Unknown => 1,
            Self::Unsafe { .. } => 0,
        };
        if rank(&self) <= rank(&other) {
            self
        } else {
            other
        }
    }
}

/// Project scope passed to [`AicxClient`] at construction.
///
/// AICX's library/MCP/CLI contract distinguishes three retrieval scopes:
/// search and index in a single project, search across an explicit list
/// of projects, or search the whole canonical store. Loctree threads this
/// distinction through the wrapper so callers can use AICX as a real
/// library boundary instead of building string-CLI arguments by hand.
///
/// Notes on partial support across AICX surfaces:
///
/// - `aicx search` (CLI + MCP): full multi-project / no-project support.
///   `-p` accepts `-p repo-a -p repo-b`, `-p repo-a,repo-b`, or omission
///   (meaning all projects). The MCP `aicx_search` tool exposes the same
///   contract through `projects: Vec<String>` plus the legacy
///   `project: Option<String>`.
/// - `aicx index` (CLI): full multi-project / no-project support
///   (materializes the persistent index by default; `--dry-run` previews).
/// - `aicx intents` (CLI): requires a single `-p <project>`. When the
///   wrapper holds [`ProjectScope::Multi`] it falls back to the first
///   project; when it holds [`ProjectScope::All`] it routes through MCP
///   (which accepts `project: Option<String>` = null = all). If MCP is
///   unavailable in `All` mode the wrapper returns an empty slice rather
///   than synthesising a fake project name.
/// - `aicx steer` (CLI): same constraint as intents.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProjectScope {
    /// Bind retrieval to a single AICX project bucket (e.g.
    /// `loctree-suite`). The wrapper passes the bucket name to every
    /// surface unchanged.
    Single(String),
    /// Bind retrieval to an explicit list of AICX project buckets. For
    /// search/index this fans out via AICX's native multi-project args.
    /// For surfaces that only support a single bucket (intents/steer)
    /// the wrapper uses the first entry and emits a debug log.
    Multi(Vec<String>),
    /// Span every project the canonical AICX store knows about. Search /
    /// index omit the `-p` flag (or send `null`/empty `projects`) so the
    /// upstream contract's "no project = all projects" semantics apply.
    All,
}

impl ProjectScope {
    /// Build a scope from a free-form iterator of project names. Empty
    /// iter → [`Self::All`]; one element → [`Self::Single`]; more than
    /// one → [`Self::Multi`]. Use this when the caller does not yet know
    /// the cardinality of its inputs.
    ///
    /// Not named `from_iter` so it does not shadow
    /// `std::iter::FromIterator::from_iter`; the wrapper keeps the
    /// classification logic colocated with the rest of the scope helpers
    /// rather than spreading it across a free-function trait impl.
    pub fn from_projects<I, S>(projects: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let collected: Vec<String> = projects
            .into_iter()
            .map(|p| p.into())
            .filter(|p| !p.trim().is_empty())
            .collect();
        match collected.len() {
            0 => Self::All,
            1 => Self::Single(collected.into_iter().next().expect("len==1")),
            _ => Self::Multi(collected),
        }
    }

    /// Convenience: every project (the AICX "no scope" path).
    pub fn all() -> Self {
        Self::All
    }

    /// Slice view of the bound projects. Empty for [`Self::All`].
    pub fn projects(&self) -> &[String] {
        match self {
            Self::Single(p) => std::slice::from_ref(p),
            Self::Multi(ps) => ps.as_slice(),
            Self::All => &[],
        }
    }

    /// Primary project name — first explicit project for
    /// [`Self::Single`] / [`Self::Multi`], or `""` for [`Self::All`].
    /// Used by surfaces that only accept a single bucket (intents CLI
    /// fallback, telemetry labels, LSP namespace echo).
    pub fn primary(&self) -> &str {
        self.projects().first().map(String::as_str).unwrap_or("")
    }

    /// True when this scope binds to exactly one explicit project.
    pub fn is_single(&self) -> bool {
        matches!(self, Self::Single(_))
    }

    /// True when this scope explicitly opts into every project.
    pub fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    /// Stable cache-key projection. Mirrors the wire shape so two
    /// equivalent scopes share cache entries.
    fn cache_key(&self) -> String {
        match self {
            Self::Single(p) => format!("s:{p}"),
            Self::Multi(ps) => {
                let mut sorted = ps.clone();
                sorted.sort();
                format!("m:{}", sorted.join(","))
            }
            Self::All => "a:*".to_string(),
        }
    }
}

/// Cache key for the per-client search cache: `(scope_key, query, hours,
/// limit)`. scope_key sits at the front so multi-project clients do not
/// collide on `query + hours + limit` alone.
type SearchCacheKey = (String, String, u64, usize);

/// Cache key for the per-client intents cache. `(scope_key, hours, limit)`.
type IntentsCacheKey = (String, u64, usize);

/// Cache key for the per-client steer cache. `(scope_key, filters)`.
type SteerCacheKey = (String, SteerFilters);

/// Read-only client for AICX memory surfaces.
///
/// Each client holds its own caches. Construct one per `loct context`
/// invocation and pass the same reference to every slice composer that
/// needs memory data.
///
/// # Resilience contract (Plan L03 / Findings #6 #7 #8)
///
/// - **Mode contract**: `LOCT_AICX_MODE=mcp` is honored as **hard MCP**.
///   On timeout (or any MCP transport error) the client logs the
///   violation and returns an empty result; it does **not** silently
///   fall back to the CLI. Operators who set `mcp` know they are
///   testing the MCP path and should see the failure surface.
/// - **Per-operation failure budget**: Replaces the old global
///   `mcp_failed: bool` flag. One transient timeout no longer poisons
///   every subsequent call. The breaker trips only after `N`
///   consecutive failures within a rolling time window
///   (default 3 / 60 s) and auto-resets when the window passes.
/// - **Cache hygiene**: Successful empty results ARE cached
///   (legitimate "no rows for this query"). Failure results are
///   **not** cached — the next call retries. A transient `aicx`
///   crash no longer becomes a permanent gap for the lifetime of
///   the client.
#[derive(Debug)]
pub struct AicxClient {
    scope: ProjectScope,
    mode: AicxMode,
    mcp: Option<mcp::AicxMcpClient>,
    /// `true` when the cfg(test) kill switch refused to set up an
    /// AICX transport (see [`test_mode_blocks_spawn`]). Forces
    /// [`Self::cli_fallback`] to a no-op as well, so a disabled
    /// client never spawns the `aicx` CLI binary even if a caller
    /// later flips `LOCT_AICX_MODE` / `LOCT_AICX_BINARY` to point
    /// at a real path. Always `false` in production builds.
    test_blocked: bool,
    mcp_budget: FailureBudget,
    mcp_fallback_logged: Mutex<bool>,
    cli_budget: FailureBudget,
    cli_budget_logged: Mutex<bool>,
    /// Hard wall-clock deadline across ALL transport work on this client
    /// (connect + every MCP/CLI call). Set by the bare-context auto-overlay
    /// so `loct context` session start is never hostage to a slow AICX
    /// store; `None` = patient client (explicit `--with-aicx`, LSP, MCP).
    overlay_deadline: Option<Instant>,
    /// Latched when any transport attempt timed out or the overlay budget
    /// ran dry. The context memory composer reads this to report
    /// "skipped (timeout)" instead of presenting a timed-out store as an
    /// empty one.
    transport_timed_out: std::sync::atomic::AtomicBool,
    /// Cache key includes the scope cache projection so a client that
    /// flips between projects (uncommon, but possible through
    /// [`AicxClient::with_scope`]) does not collide rows.
    intents_cache: Mutex<HashMap<IntentsCacheKey, Vec<AicxIntent>>>,
    steer_cache: Mutex<HashMap<SteerCacheKey, Vec<AicxSteerResult>>>,
    search_cache: Mutex<HashMap<SearchCacheKey, Vec<AicxSearchResult>>>,
}

/// Per-operation failure tracker for the MCP transport.
///
/// Replaces the legacy global `mcp_failed: Mutex<bool>` flag (Plan L03 /
/// Finding #7). One timeout no longer trips the breaker — the budget
/// counts consecutive failures within a rolling time window, trips only
/// after the threshold is exceeded, and auto-resets when the window
/// passes.
#[derive(Debug)]
struct FailureBudget {
    state: Mutex<FailureBudgetState>,
    threshold: usize,
    reset_after: Duration,
}

#[derive(Debug, Default)]
struct FailureBudgetState {
    failures: usize,
    last_failure_at: Option<Instant>,
}

impl FailureBudget {
    fn new(threshold: usize, reset_after: Duration) -> Self {
        Self {
            state: Mutex::new(FailureBudgetState::default()),
            threshold,
            reset_after,
        }
    }

    /// Bump the failure counter. Resets to 1 if the previous failure was
    /// older than the reset window — a rolling counter, not a sticky one.
    fn record_failure(&self) {
        if let Ok(mut s) = self.state.lock() {
            let now = Instant::now();
            let stale = s
                .last_failure_at
                .map(|prev| now.duration_since(prev) >= self.reset_after)
                .unwrap_or(true);
            s.failures = if stale {
                1
            } else {
                s.failures.saturating_add(1)
            };
            s.last_failure_at = Some(now);
        }
    }

    fn clear(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.failures = 0;
            s.last_failure_at = None;
        }
    }

    /// `true` when the breaker should be considered tripped — too many
    /// failures within `reset_after`. Auto-resets when the window passes.
    fn is_tripped(&self) -> bool {
        let Ok(s) = self.state.lock() else {
            return true;
        };
        let Some(last) = s.last_failure_at else {
            return false;
        };
        if last.elapsed() >= self.reset_after {
            return false;
        }
        s.failures >= self.threshold
    }

    #[cfg(test)]
    fn failure_count(&self) -> usize {
        self.state.lock().map(|s| s.failures).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AicxMode {
    Auto,
    Cli,
    Mcp,
}

impl AicxMode {
    fn from_env() -> Self {
        match std::env::var(AICX_MODE_ENV)
            .ok()
            .map(|raw| raw.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("cli") => Self::Cli,
            Some("mcp") => Self::Mcp,
            Some("auto") | None | Some("") => Self::Auto,
            Some(other) => {
                shell::debug_log(format!("unknown {AICX_MODE_ENV}={other:?}; using auto"));
                Self::Auto
            }
        }
    }

    fn may_try_mcp(self) -> bool {
        matches!(self, Self::Auto | Self::Mcp)
    }

    fn may_fallback_to_cli(self) -> bool {
        matches!(self, Self::Auto)
    }
}

impl AicxClient {
    /// Create a new client bound to a single AICX project bucket.
    ///
    /// `project` is forwarded to `aicx -p <project>` and is the same bucket
    /// name AICX uses internally (case-insensitive substring match in `aicx`).
    pub fn new(project: impl Into<String>) -> Self {
        Self::with_scope(ProjectScope::Single(project.into()))
    }

    /// Like [`AicxClient::new`], with a hard wall-clock budget across ALL
    /// transport work (connect + every call). Used by the bare-context
    /// auto-overlay so session start stays bounded on slow AICX stores.
    /// See [`AicxClient::with_scope_budgeted`].
    pub fn new_budgeted(project: impl Into<String>, budget: Option<Duration>) -> Self {
        Self::with_scope_budgeted(ProjectScope::Single(project.into()), budget)
    }

    /// Create a client bound to an explicit list of AICX project buckets.
    ///
    /// Empty iter is treated as [`ProjectScope::All`] (every project).
    /// One element collapses to [`ProjectScope::Single`]; two or more to
    /// [`ProjectScope::Multi`]. See [`ProjectScope`] for per-surface
    /// support notes (`aicx intents` and `aicx steer` only honour the
    /// first project; `aicx search` and `aicx index` honour all of them).
    pub fn with_projects<I, S>(projects: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::with_scope(ProjectScope::from_projects(projects))
    }

    /// Create a client that spans every project in the canonical AICX
    /// store. The wrapper omits the project flag (or sends `null`) on
    /// surfaces that interpret the absence as "all projects". Surfaces
    /// that require a single bucket (intents/steer CLI) return an empty
    /// slice in this mode — operators set up MCP transport for the
    /// "all projects" intents path.
    pub fn all_projects() -> Self {
        Self::with_scope(ProjectScope::All)
    }

    /// Create a client from an explicit [`ProjectScope`]. Use this when
    /// the caller already classified its scope (e.g. routed from typed
    /// LSP params or CLI flags).
    pub fn with_scope(scope: ProjectScope) -> Self {
        Self::with_scope_budgeted(scope, None)
    }

    /// Like [`AicxClient::with_scope`], with a hard wall-clock budget
    /// spanning ALL transport work on this client — MCP connect, every
    /// MCP call, every CLI invocation. When the budget runs dry the
    /// client stops issuing transport calls, returns empty results fast,
    /// and latches [`AicxClient::transport_timed_out`] so callers can
    /// render an explicit "skipped (timeout)" instead of a silent gap.
    pub fn with_scope_budgeted(scope: ProjectScope, budget: Option<Duration>) -> Self {
        // Test-mode kill switch — see [`test_mode_blocks_spawn`] for
        // the rationale. In production builds the check resolves to
        // `false` at compile time and disappears.
        let test_blocked = test_mode_blocks_spawn();
        let overlay_deadline = budget.map(|b| Instant::now() + b);
        let mut connect_timed_out = false;

        let mode = AicxMode::from_env();
        let mcp = if mode.may_try_mcp() && !test_blocked {
            let connect_cap = overlay_deadline.map(|d| d.saturating_duration_since(Instant::now()));
            match mcp::AicxMcpClient::connect_and_check(connect_cap) {
                Ok(client) => Some(client),
                Err(error) => {
                    connect_timed_out = error.is_timeout();
                    if mode.may_fallback_to_cli() {
                        shell::debug_log(format!(
                            "MCP unavailable; falling back to AICX CLI: {error}"
                        ));
                    } else {
                        shell::debug_log(format!("MCP unavailable: {error}"));
                    }
                    None
                }
            }
        } else {
            None
        };

        Self {
            scope,
            mode,
            mcp,
            test_blocked,
            // Three failures within 60 seconds trip the breaker. Generous
            // enough that one timeout on a slow MCP store does not derail
            // the whole `loct context` invocation; tight enough that a
            // genuinely broken upstream stops getting hammered. Auto-resets.
            mcp_budget: FailureBudget::new(3, Duration::from_secs(60)),
            mcp_fallback_logged: Mutex::new(false),
            // For CLI fallback, we trip on 3 failures (15s timeout each = 45s total).
            // A broken CLI typically hangs on every invocation, so we fail fast after 3.
            cli_budget: FailureBudget::new(3, Duration::from_secs(60)),
            cli_budget_logged: Mutex::new(false),
            intents_cache: Mutex::new(HashMap::new()),
            steer_cache: Mutex::new(HashMap::new()),
            search_cache: Mutex::new(HashMap::new()),
            overlay_deadline,
            transport_timed_out: std::sync::atomic::AtomicBool::new(connect_timed_out),
        }
    }

    /// Remaining transport budget. `Ok(None)` = no deadline set;
    /// `Ok(Some(d))` = `d` left; `Err(())` = budget exhausted (callers
    /// must skip the transport call and latch the timeout flag).
    fn transport_cap(&self) -> Result<Option<Duration>, ()> {
        match self.overlay_deadline {
            None => Ok(None),
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    Err(())
                } else {
                    Ok(Some(remaining))
                }
            }
        }
    }

    fn note_timeout(&self) {
        self.transport_timed_out
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// True when any transport attempt on this client timed out or the
    /// overlay budget ran dry. Distinguishes "store answered: nothing
    /// there" from "store never got to answer" — the two demand different
    /// operator reactions (nothing vs. raise the budget / use
    /// `--with-aicx`).
    pub fn transport_timed_out(&self) -> bool {
        self.transport_timed_out
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Project bucket bound to this client. For [`ProjectScope::Single`]
    /// returns the bucket name verbatim; for [`ProjectScope::Multi`]
    /// returns the first project (the "primary" used by surfaces that
    /// only accept one); for [`ProjectScope::All`] returns `""` — callers
    /// that need a stable label should branch on [`AicxClient::scope`]
    /// instead.
    pub fn project(&self) -> &str {
        self.scope.primary()
    }

    /// Full [`ProjectScope`] backing this client.
    pub fn scope(&self) -> &ProjectScope {
        &self.scope
    }

    /// Fetch structured intents for the given window.
    ///
    /// `window_hours` bounds the extraction window. `limit` caps the number of
    /// rows. Returns an empty `Vec` when AICX is missing or fails.
    ///
    /// Multi-project caveat: `aicx intents` (CLI) requires a single
    /// `-p <project>`. When the wrapper holds [`ProjectScope::Multi`] the
    /// CLI path queries the first project only; the MCP path forwards
    /// the same single bucket. When the wrapper holds
    /// [`ProjectScope::All`] the CLI path is skipped (no synthetic
    /// project name) and the wrapper relies on MCP's
    /// `project: Option<String>` semantics where `null` means all
    /// projects; if MCP is not configured the call returns an empty
    /// slice rather than guessing.
    pub fn intents(&self, window_hours: u64, limit: usize) -> Vec<AicxIntent> {
        let key = (self.scope.cache_key(), window_hours, limit);
        if let Some(cached) = cache_get(&self.intents_cache, &key) {
            return cached;
        }

        let parsed = match self.mcp_intents(window_hours, limit) {
            Some(parsed) => parsed,
            None => {
                let primary = self.scope.primary();
                if primary.is_empty() {
                    // ProjectScope::All — `aicx intents` CLI cannot serve
                    // "all projects" without a bucket name, and we refuse
                    // to fabricate one. Operator should configure MCP for
                    // the all-projects intents path.
                    shell::debug_log(
                        "intents: ProjectScope::All has no CLI shape; configure aicx-mcp \
                         (LOCT_AICX_MODE=auto|mcp) for cross-project intents",
                    );
                    return Vec::new();
                }
                let limit_str = limit.to_string();
                let hours_str = window_hours.to_string();
                let args = [
                    "intents", "-p", primary, "-H", &hours_str, "--limit", &limit_str, "--emit",
                    "json",
                ];
                let stdout = match self.cli_fallback(&args) {
                    Some(s) => s,
                    None => {
                        // Plan L03 / Finding #8 — do NOT cache failures.
                        // A transient AICX failure returning None must not
                        // poison the cache with `Vec::new()` for the
                        // lifetime of the client. Next call retries.
                        return Vec::new();
                    }
                };
                shell::parse_intents(&stdout)
            }
        };
        cache_put(&self.intents_cache, key, parsed.clone());
        parsed
    }

    /// Steer-filter chunks by frontmatter metadata.
    ///
    /// Falls back to text parsing because `aicx steer` does not currently
    /// emit JSON. Returns an empty `Vec` on any failure.
    ///
    /// Multi-project caveat: same as [`AicxClient::intents`] — `aicx steer`
    /// CLI takes one `--project`. [`ProjectScope::Multi`] uses the first
    /// project; [`ProjectScope::All`] requires MCP transport.
    pub fn steer(&self, filters: SteerFilters) -> Vec<AicxSteerResult> {
        let scope_key = self.scope.cache_key();
        let cache_key = (scope_key, filters.clone());
        if let Some(cached) = cache_get(&self.steer_cache, &cache_key) {
            return cached;
        }

        let parsed = match self.mcp_steer(&filters) {
            Some(parsed) => parsed,
            None => {
                let primary = self.scope.primary();
                if primary.is_empty() {
                    shell::debug_log(
                        "steer: ProjectScope::All has no CLI shape; configure aicx-mcp \
                         (LOCT_AICX_MODE=auto|mcp) for cross-project steer",
                    );
                    return Vec::new();
                }
                self.steer_via_cli(primary, &filters)
            }
        };
        cache_put(&self.steer_cache, cache_key, parsed.clone());
        parsed
    }

    fn steer_via_cli(&self, primary: &str, filters: &SteerFilters) -> Vec<AicxSteerResult> {
        let limit_str;
        let mut args: Vec<&str> = vec!["steer", "--project", primary];
        if let Some(run_id) = filters.run_id.as_deref() {
            args.push("--run-id");
            args.push(run_id);
        }
        if let Some(prompt_id) = filters.prompt_id.as_deref() {
            args.push("--prompt-id");
            args.push(prompt_id);
        }
        if let Some(kind) = filters.kind.as_deref() {
            args.push("-k");
            args.push(kind);
        }
        if let Some(agent) = filters.agent.as_deref() {
            args.push("--agent");
            args.push(agent);
        }
        if let Some(date) = filters.date.as_deref() {
            args.push("-d");
            args.push(date);
        }
        if let Some(frame_kind) = filters.frame_kind.as_deref() {
            args.push("--frame-kind");
            args.push(frame_kind);
        }
        if let Some(limit) = filters.limit {
            limit_str = limit.to_string();
            args.push("--limit");
            args.push(&limit_str);
        }

        // cfg(test) kill switch — see [`test_mode_blocks_spawn`].
        // `steer_via_cli` bypasses `cli_fallback`, so the guard needs
        // to be repeated here.
        if self.test_blocked {
            return Vec::new();
        }
        match shell::run_aicx(&args) {
            Some(stdout) => shell::parse_steer(&stdout),
            // Plan L03 / Finding #8 — do NOT cache failures. Caller side
            // (`steer`) handles the empty-on-fail contract.
            None => Vec::new(),
        }
    }

    /// Search the canonical AICX corpus.
    ///
    /// Honors the wrapper's [`ProjectScope`]:
    ///
    /// - [`ProjectScope::Single`] → `aicx search query -p <project>`
    ///   (MCP: `project: <name>`).
    /// - [`ProjectScope::Multi`] → `aicx search query -p p1,p2,p3`
    ///   (MCP: `projects: [...]`). AICX fans the query out across each
    ///   bucket and merges the result set.
    /// - [`ProjectScope::All`] → `aicx search query` with no project flag
    ///   (MCP: `project` omitted). AICX searches the entire canonical store.
    ///
    /// `hours` bounds the caller contract where the active transport supports
    /// it (`0` = all time). Returns ranked results with `matches` snippets
    /// pre-truncated by AICX. Per the AICX contract, when the semantic
    /// vector index is unavailable the row-level [`OracleStatus`] carries a
    /// `fallback_reason` and `loctree_scope_safe == false` — callers should
    /// consult [`OracleStatus::readiness`] instead of treating any
    /// non-empty result as semantic oracle output.
    pub fn search(&self, query: &str, hours: u64, limit: usize) -> Vec<AicxSearchResult> {
        let key = (self.scope.cache_key(), query.to_string(), hours, limit);
        if let Some(cached) = cache_get(&self.search_cache, &key) {
            return cached;
        }

        let hours_str = hours.to_string();
        let limit_str = limit.to_string();
        let project_csv = self.scope.projects().join(",");
        let mut args: Vec<&str> = vec!["search", query];
        if !project_csv.is_empty() {
            args.push("-p");
            args.push(&project_csv);
        }
        args.push("-H");
        args.push(&hours_str);
        args.push("--limit");
        args.push(&limit_str);
        args.push("-j");

        let parsed = match self.mcp_search(query, hours, limit) {
            Some(parsed) => parsed,
            None => {
                let stdout = match self.cli_fallback(&args) {
                    Some(s) => s,
                    None => {
                        // Plan L03 / Finding #8 — do NOT cache failures.
                        return Vec::new();
                    }
                };
                shell::parse_search(&stdout)
            }
        };
        cache_put(&self.search_cache, key, parsed.clone());
        parsed
    }

    fn cli_fallback(&self, args: &[&str]) -> Option<String> {
        // Overlay wall-clock budget exhausted — skip the spawn entirely.
        // Checked FIRST (latching spawns nothing) so the exhausted path
        // always reports as a timeout, even under the cfg(test) kill
        // switch or after earlier failures tripped the breaker.
        let timeout_cap = match self.transport_cap() {
            Ok(cap) => cap,
            Err(()) => {
                self.note_timeout();
                shell::debug_log("overlay budget exhausted; skipping CLI call");
                return None;
            }
        };

        // cfg(test) kill switch — see [`test_mode_blocks_spawn`].
        // A disabled client never spawns subprocesses regardless of
        // mode, so production-path tests that transitively trigger
        // `AicxClient::new` cannot race the `aicx_env` serial group
        // by reading another test's `LOCT_AICX_BINARY` mock script.
        if self.test_blocked {
            return None;
        }

        if self.cli_budget.is_tripped() {
            return None;
        }

        if self.mode == AicxMode::Cli || self.mode.may_fallback_to_cli() {
            match shell::run_aicx_outcome(args, timeout_cap) {
                Ok(out) => {
                    self.cli_budget.clear();
                    Some(out)
                }
                Err(failure) => {
                    if failure == shell::AicxRunFailure::Timeout {
                        self.note_timeout();
                    }
                    self.cli_budget.record_failure();
                    if self.cli_budget.is_tripped() {
                        let mut logged = self.cli_budget_logged.lock().unwrap();
                        if !*logged {
                            eprintln!(
                                "[loctree::aicx] aicx unavailable, atlas without overlay: AICX CLI timed out/failed. Disabling for this session."
                            );
                            *logged = true;
                        }
                    }
                    None
                }
            }
        } else {
            None
        }
    }

    fn mcp_intents(&self, window_hours: u64, limit: usize) -> Option<Vec<AicxIntent>> {
        // Intents MCP tool takes a single optional project. Empty = null
        // (= all projects). Multi-project: pick the primary; the AICX
        // surface does not yet model multi-project intents.
        let primary = self.scope.primary();
        let project_arg = if primary.is_empty() {
            None
        } else {
            Some(primary)
        };
        self.try_mcp(|timeout_cap| {
            self.mcp.as_ref().expect("checked in try_mcp").intents(
                project_arg,
                window_hours,
                limit,
                timeout_cap,
            )
        })
    }

    fn mcp_steer(&self, filters: &SteerFilters) -> Option<Vec<AicxSteerResult>> {
        let primary = self.scope.primary();
        let project_arg = if primary.is_empty() {
            None
        } else {
            Some(primary)
        };
        self.try_mcp(|timeout_cap| {
            self.mcp
                .as_ref()
                .expect("checked in try_mcp")
                .steer(project_arg, filters, timeout_cap)
        })
    }

    fn mcp_search(&self, query: &str, hours: u64, limit: usize) -> Option<Vec<AicxSearchResult>> {
        let projects = self.scope.projects();
        self.try_mcp(|timeout_cap| {
            self.mcp.as_ref().expect("checked in try_mcp").search(
                projects,
                query,
                hours,
                limit,
                timeout_cap,
            )
        })
    }

    fn try_mcp<T>(
        &self,
        call: impl FnOnce(Option<Duration>) -> Result<T, mcp::AicxMcpError>,
    ) -> Option<T>
    where
        T: Default,
    {
        // Overlay wall-clock budget exhausted — skip the transport call
        // entirely and latch the timeout flag. Checked FIRST, before mode
        // and transport-state gates: latching spawns nothing, and an
        // exhausted budget must read as a timeout on EVERY path (including
        // CLI-only mode and the cfg(test) kill switch — otherwise the skip
        // reason degrades to a fake "namespace empty" depending on
        // process-global env state). Hard MCP surfaces empty; other modes
        // return None so cli_fallback runs its own check and skips as fast.
        let timeout_cap = match self.transport_cap() {
            Ok(cap) => cap,
            Err(()) => {
                self.note_timeout();
                shell::debug_log("overlay budget exhausted; skipping MCP call");
                return if self.mode == AicxMode::Mcp {
                    Some(default_empty())
                } else {
                    None
                };
            }
        };
        // CLI mode never tries MCP.
        if self.mode == AicxMode::Cli {
            return None;
        }
        // No MCP transport — for hard MCP we surface as empty; otherwise
        // fall back to CLI.
        if self.mcp.is_none() {
            return if self.mode == AicxMode::Mcp {
                Some(default_empty())
            } else {
                None
            };
        }
        // Budget tripped — short-circuit. Auto mode falls back to CLI;
        // hard MCP surfaces empty so the operator sees the consequence.
        if self.mcp_budget.is_tripped() {
            return if self.mode.may_fallback_to_cli() {
                None
            } else {
                Some(default_empty())
            };
        }

        match call(timeout_cap) {
            Ok(value) => Some(value),
            Err(error) => {
                self.mcp_budget.record_failure();
                let timeout = error.is_timeout();
                if timeout {
                    self.note_timeout();
                }
                if self.mode.may_fallback_to_cli() {
                    self.log_mcp_fallback_once(format!(
                        "MCP call failed; falling back to CLI for this session: {error}"
                    ));
                    None
                } else {
                    // Plan L03 / Finding #6 — hard MCP mode does NOT
                    // silently fall back to CLI on timeout. Operator who
                    // explicitly chose `LOCT_AICX_MODE=mcp` must see the
                    // failure surface, not get a CLI substitute disguised
                    // as MCP.
                    if timeout {
                        eprintln!(
                            "loctree: AICX MCP timeout in hard MCP mode \
                             (LOCT_AICX_MODE=mcp). Returning empty result. \
                             Unset LOCT_AICX_MODE or set it to 'auto' to \
                             allow CLI fallback."
                        );
                    } else {
                        shell::debug_log(format!("MCP call failed in hard mode: {error}"));
                    }
                    Some(default_empty())
                }
            }
        }
    }

    fn log_mcp_fallback_once(&self, msg: String) {
        if let Ok(mut guard) = self.mcp_fallback_logged.lock()
            && !*guard
        {
            shell::debug_log(msg);
            *guard = true;
        }
    }
}

impl mcp::AicxMcpError {
    fn is_timeout(&self) -> bool {
        // `Init("timeout")` is the connect-phase handshake timeout — same
        // operator-facing meaning as a tool-call timeout.
        matches!(self, Self::Timeout(_)) || matches!(self, Self::Init(msg) if msg == "timeout")
    }
}

fn default_empty<T>() -> T
where
    T: Default,
{
    T::default()
}

fn cache_get<K, V>(cache: &Mutex<HashMap<K, V>>, key: &K) -> Option<V>
where
    K: std::hash::Hash + Eq,
    V: Clone,
{
    cache.lock().ok()?.get(key).cloned()
}

fn cache_put<K, V>(cache: &Mutex<HashMap<K, V>>, key: K, value: V)
where
    K: std::hash::Hash + Eq,
{
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, value);
    }
}

// cfg(test) kill switch for the AICX transport.
//
// # Why this exists
//
// The chronic `serial_test::serial(aicx_env)` flake had its root cause
// in production code paths constructing an [`AicxClient`] every time
// the snapshot/init pipeline runs. Specifically:
// `Snapshot::save_full_artifacts` →
// `compose_context_pack_from_snapshot(with_aicx: true)` →
// `AicxClient::new(bucket)`. This unconditional spawn happens during
// `run_init`, which many lib tests trigger transitively
// (`load_or_scan_src`, `ensure_snapshot`, `try_load_snapshot_with_auto_scan`).
//
// Those production-path tests are **not** in the `aicx_env` serial
// group. When they ran concurrently with an `aicx_env` test that had
// just set `LOCT_AICX_BINARY` / `AICX_MCP_BINARY` to a tempdir mock
// script, the production-path `AicxClient` read those env vars at
// spawn time and invoked the OTHER test's mock binary. Each stray
// invocation appended a `"cli\n"` or counter increment to the
// aicx_env test's `transport.log` / counter file, producing
// intermittent failures like
// `left: "cli\ncli", right: "cli"` in `cli_mode_skips_mcp_check` and
// counter drift (`"3" != "2"`) in `client_caches_per_invocation`.
//
// # Why a thread-local opt-in, not an env var
//
// The opt-in signal is a **thread-local** (`AICX_TEST_OPT_IN`) rather
// than an env var because env vars are process-global. If the opt-in
// signal lived in `std::env`, an `aicx_env` test setting it would
// also let any concurrent non-aicx test pass the kill switch — which
// is exactly the original race we are trying to fix. The thread-local
// is set inside the test function body (after the serial mutex is
// acquired), so it is visible only to code running on that test's
// thread; concurrent tests on other threads see the default `false`
// and get a no-op `AicxClient`.
//
// # Behaviour
//
// In production builds (`cfg(test)` off) this function always
// returns `false` and the compiler folds the constant — there is
// zero runtime cost.
//
// In test builds (`cfg(test)` on) this function returns `true`
// **unless** the calling thread has set `AICX_TEST_OPT_IN` to
// `true` via [`set_aicx_test_opt_in`]. The `aicx::tests`,
// `pack::tests`, and `cli::dispatch::handlers::context::tests`
// memory-slice tests (which legitimately need the AICX shell-out)
// opt in via that helper. Every other test gets a no-op
// `AicxClient` that returns empty rows without touching env vars
// or spawning subprocesses — eliminating the race deterministically.
#[cfg(test)]
thread_local! {
    pub(crate) static AICX_TEST_OPT_IN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[inline]
fn test_mode_blocks_spawn() -> bool {
    #[cfg(test)]
    {
        !AICX_TEST_OPT_IN.with(|cell| cell.get())
    }
    #[cfg(not(test))]
    {
        false
    }
}

/// Opt the current thread in to real AICX transport setup during
/// tests. Pair with [`clear_aicx_test_opt_in`] for cleanup. See
/// [`test_mode_blocks_spawn`] for the rationale.
///
/// `pub(crate)` so `pack::tests` and
/// `cli::dispatch::handlers::context::tests` memory-slice tests can
/// opt in without going through the `aicx::tests`-private helper.
#[cfg(test)]
pub(crate) fn set_aicx_test_opt_in() {
    AICX_TEST_OPT_IN.with(|cell| cell.set(true));
}

/// Reset the per-thread AICX test opt-in flag. Pair with
/// [`set_aicx_test_opt_in`].
#[cfg(test)]
pub(crate) fn clear_aicx_test_opt_in() {
    AICX_TEST_OPT_IN.with(|cell| cell.set(false));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::path::Path;

    /// Write a Unix shell script and chmod it executable.
    #[cfg(unix)]
    fn write_script(path: &Path, body: &str) {
        std::fs::write(path, body).expect("write mock script");
        let mut perms = std::fs::metadata(path)
            .expect("stat mock script")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).expect("chmod mock script");
    }

    unsafe fn clear_aicx_env() {
        // SAFETY: tests in this module are serialised on the `aicx_env` group
        // because they mutate process-global env vars.
        unsafe {
            std::env::remove_var(AICX_BINARY_ENV);
            std::env::remove_var(AICX_MODE_ENV);
            std::env::remove_var(AICX_MCP_BINARY_ENV);
        }
        // Also reset the per-thread AICX kill-switch opt-in. Not
        // strictly required for correctness (the opt-in is
        // thread-local and dies with the test thread) but keeps test
        // cleanup hygienic — if a test thread is reused for a
        // subsequent test that doesn't want AICX, the flag won't
        // leak.
        super::clear_aicx_test_opt_in();
    }

    /// Opt-in to the AICX transport for the current test thread.
    /// Pairs with [`clear_aicx_env`] for cleanup. See
    /// [`super::test_mode_blocks_spawn`] for the rationale.
    fn enable_aicx_for_test() {
        super::set_aicx_test_opt_in();
    }

    #[test]
    fn exhausted_overlay_budget_latches_timeout_without_transport() {
        // Perf canary for the sub-2s session-start contract: a client whose
        // overlay budget is already spent must (a) never attempt a blocking
        // transport call and (b) latch the explicit timeout flag so the
        // memory composer reports "skipped (timeout)" instead of a fake
        // "namespace empty". Deterministic — no wall-clock assertions, no
        // subprocess spawns (cfg(test) kill switch stays engaged).
        let client =
            AicxClient::with_scope_budgeted(ProjectScope::Single("x".into()), Some(Duration::ZERO));

        let rows = client.intents(168, 50);

        assert!(rows.is_empty(), "exhausted budget must yield no rows");
        assert!(
            client.transport_timed_out(),
            "exhausted budget must latch the explicit timeout flag"
        );
    }

    #[test]
    fn unbudgeted_client_reports_no_timeout_when_transport_disabled() {
        // Control for the canary above: with no deadline set, an empty
        // result from a disabled transport must NOT read as a timeout —
        // "no answer configured" and "no time to answer" are different
        // truths.
        let client = AicxClient::with_scope(ProjectScope::Single("x".into()));

        let rows = client.intents(168, 50);

        assert!(rows.is_empty());
        assert!(!client.transport_timed_out());
    }

    #[test]
    fn summarize_entry_collapses_raw_unified_diff_json() {
        let raw = r#"{
            "call_id":"call_123",
            "unified_diff":"diff --git a/loctree-rs/src/pack.rs b/loctree-rs/src/pack.rs\n--- a/loctree-rs/src/pack.rs\n+++ b/loctree-rs/src/pack.rs\n@@ -1 +1 @@\n-old\n+new"
        }"#;

        let summary = summarize_entry(raw);

        assert!(summary.structured);
        assert!(summary.text.contains("loctree-rs/src/pack.rs"));
        assert!(!summary.text.contains("call_id"));
        assert!(!summary.text.contains("unified_diff"));
        assert!(!summary.text.contains("@@"));
    }

    #[test]
    fn summarize_entry_marks_unknown_blob_raw_without_pasting_it() {
        let raw = "{ \"unknown\": \"shape\", \"payload\": \"@@ -1,2 +1,2\" }";

        let summary = summarize_entry(raw);

        assert!(!summary.structured);
        assert_eq!(summary.text, "raw AICX entry");
    }

    #[cfg(unix)]
    fn write_cli_mock(path: &Path, log: &Path, text: &str) {
        let body = format!(
            r#"#!/bin/sh
printf 'cli\n' >> '{log}'
case "$1" in
  intents)
    printf '{text}'
    ;;
  steer)
    printf 'Loctree/loctree-suite | codex | 2026-04-28 | reports
  run_id: -  prompt_id: -  model: -
  /tmp/cli.md
'
    ;;
  search)
    printf '{{"results":1,"items":[{{"score":88,"label":"HIGH","project":"Loctree/loctree-suite","agent":"codex","date":"2026-04-28","matches":["cli"],"path":"/tmp/cli.md"}}]}}'
    ;;
  *)
    exit 0
    ;;
esac
"#,
            log = log.display(),
            text = text,
        );
        write_script(path, &body);
    }

    #[cfg(unix)]
    fn write_mcp_mock(path: &Path, log: &Path, text: &str) {
        let escaped_text = text.replace('\\', "\\\\").replace('"', "\\\"");
        let body = format!(
            r#"#!/bin/sh
INTENTS_TEXT='{text}'
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2024-11-05","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"aicx-mock","version":"0.0.0"}}}}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"aicx_intents","description":"","inputSchema":{{"type":"object"}}}},{{"name":"aicx_search","description":"","inputSchema":{{"type":"object"}}}},{{"name":"aicx_steer","description":"","inputSchema":{{"type":"object"}}}}]}}}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      printf 'mcp\n' >> '{log}'
      case "$line" in
        *aicx_intents*)
          out="$INTENTS_TEXT"
          ;;
        *aicx_search*)
          out='{{"results":1,"items":[{{"score":88,"label":"HIGH","project":"Loctree/loctree-suite","agent":"codex","date":"2026-04-28","matches":["mcp"],"path":"/tmp/mcp.md"}}]}}'
          ;;
        *aicx_steer*)
          out='{{"results":1,"items":[{{"project":"Loctree/loctree-suite","agent":"codex","date":"2026-04-28","kind":"reports","path":"/tmp/mcp.md"}}]}}'
          ;;
        *)
          out='[]'
          ;;
      esac
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"%s"}}],"isError":false}}}}\n' "$id" "$out"
      ;;
  esac
done
"#,
            log = log.display(),
            text = escaped_text,
        );
        write_script(path, &body);
    }

    #[test]
    #[serial_test::serial(aicx_env)]
    fn client_returns_empty_when_aicx_not_installed() {
        // Point the wrapper at a path that cannot be spawned.
        // SAFETY: tests in this module are serialised on the `aicx_env` group
        // because they mutate process-global env vars.
        unsafe {
            clear_aicx_env();
            enable_aicx_for_test();
            std::env::set_var(AICX_MODE_ENV, "cli");
            std::env::set_var(AICX_BINARY_ENV, "/this/path/does/not/exist/aicx-12345");
        }

        let client = AicxClient::new("loctree-suite");
        let intents = client.intents(720, 100);
        let steer = client.steer(SteerFilters::default());
        let search = client.search("foo", 168, 20);

        unsafe {
            clear_aicx_env();
        }

        assert!(
            intents.is_empty(),
            "intents should be empty when aicx missing"
        );
        assert!(steer.is_empty(), "steer should be empty when aicx missing");
        assert!(
            search.is_empty(),
            "search should be empty when aicx missing"
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn client_handles_invalid_json_gracefully() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("aicx-mock.sh");
        write_script(
            &script,
            "#!/bin/sh\nprintf 'not json at all { [ \\n'\nexit 0\n",
        );

        unsafe {
            clear_aicx_env();
            enable_aicx_for_test();
            std::env::set_var(AICX_MODE_ENV, "cli");
            std::env::set_var(AICX_BINARY_ENV, &script);
        }
        let client = AicxClient::new("loctree-suite");
        let intents = client.intents(720, 100);
        let search = client.search("foo", 168, 20);
        unsafe {
            clear_aicx_env();
        }

        assert!(intents.is_empty(), "intents should be empty on bad JSON");
        assert!(search.is_empty(), "search should be empty on bad JSON");
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn client_caches_per_invocation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let counter = dir.path().join("counter");
        std::fs::write(&counter, "0").unwrap();
        let script = dir.path().join("aicx-mock.sh");
        let body = format!(
            "#!/bin/sh\nCOUNTER_FILE='{}'\nN=$(cat \"$COUNTER_FILE\")\nNEXT=$((N + 1))\necho \"$NEXT\" > \"$COUNTER_FILE\"\nif [ \"$N\" = \"0\" ]; then\n  printf '[{{\"kind\":\"intent\",\"summary\":\"first\",\"project\":\"x\",\"agent\":\"claude\",\"date\":\"2026-04-28\",\"timestamp\":\"2026-04-28T00:00:00Z\",\"session_id\":\"s1\",\"source_chunk\":\"/tmp/a.md\"}}]'\nelse\n  printf '[{{\"kind\":\"intent\",\"summary\":\"second\",\"project\":\"x\",\"agent\":\"claude\",\"date\":\"2026-04-28\",\"timestamp\":\"2026-04-28T00:00:00Z\",\"session_id\":\"s2\",\"source_chunk\":\"/tmp/b.md\"}}]'\nfi\n",
            counter.display()
        );
        write_script(&script, &body);

        unsafe {
            clear_aicx_env();
            enable_aicx_for_test();
            std::env::set_var(AICX_MODE_ENV, "cli");
            std::env::set_var(AICX_BINARY_ENV, &script);
        }
        let client = AicxClient::new("x");
        let first = client.intents(720, 100);
        let second = client.intents(720, 100);

        // Different cache key → must invoke binary again and see the second
        // payload, proving the mock script does change output between calls.
        let third = client.intents(168, 100);
        unsafe {
            clear_aicx_env();
        }

        assert_eq!(first.len(), 1, "first call must return one intent");
        assert_eq!(first[0].text, "first", "first call returns 'first'");
        assert_eq!(second.len(), 1, "second call must return one intent");
        assert_eq!(
            second[0].text, "first",
            "second call must hit cache and reuse first call's data"
        );
        assert_eq!(third.len(), 1, "third call must return one intent");
        assert_eq!(
            third[0].text, "second",
            "different window key must miss the cache and shell out again"
        );

        // Counter must show only two underlying invocations: once for (720,
        // 100) and once for (168, 100); the duplicate (720, 100) call hit the
        // cache.
        let counter_value = std::fs::read_to_string(&counter).unwrap();
        assert_eq!(
            counter_value.trim(),
            "2",
            "cache should suppress the duplicate (720, 100) shell-out"
        );
    }

    #[test]
    fn intent_field_aliases_round_trip() {
        // Public type re-serialises with our canonical names; consumers do not
        // see the wire-level `summary`/`source_chunk` fields.
        let intent = AicxIntent {
            kind: "intent".to_string(),
            text: "hello".to_string(),
            agent: "claude".to_string(),
            date: "2026-04-28".to_string(),
            timestamp: Some("2026-04-28T00:00:00Z".to_string()),
            session_id: "s1".to_string(),
            project: "loctree-suite".to_string(),
            source_chunk_path: "/tmp/a.md".to_string(),
            frame_kind: None,
            oracle_status: None,
        };
        let serialized = serde_json::to_string(&intent).unwrap();
        assert!(serialized.contains("\"text\":\"hello\""));
        assert!(serialized.contains("\"source_chunk_path\":\"/tmp/a.md\""));
        // `oracle_status: None` is suppressed by `skip_serializing_if`,
        // so the wire never grows when callers do not have provenance.
        assert!(
            !serialized.contains("oracle_status"),
            "None oracle_status must not leak into the wire: {serialized}"
        );
        let round: AicxIntent = serde_json::from_str(&serialized).unwrap();
        assert_eq!(round, intent);
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn mcp_mode_uses_stdio_client() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("transport.log");
        let mcp = dir.path().join("aicx-mcp-mock.sh");
        let cli = dir.path().join("aicx-cli-mock.sh");
        let mcp_text = r#"[{"kind":"intent","summary":"mcp-intent","project":"x","agent":"codex","date":"2026-04-28","session_id":"m1","source_chunk":"/tmp/mcp.md"}]"#;
        write_mcp_mock(&mcp, &log, mcp_text);
        write_cli_mock(&cli, &log, r#"[]"#);

        unsafe {
            clear_aicx_env();
            enable_aicx_for_test();
            std::env::set_var(AICX_MODE_ENV, "mcp");
            std::env::set_var(AICX_MCP_BINARY_ENV, &mcp);
            std::env::set_var(AICX_BINARY_ENV, &cli);
        }
        let client = AicxClient::new("x");
        let intents = client.intents(720, 100);
        unsafe {
            clear_aicx_env();
        }

        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].text, "mcp-intent");
        let log_text = std::fs::read_to_string(&log).expect("transport log");
        assert!(log_text.contains("mcp"));
        assert!(!log_text.contains("cli"));
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn cli_mode_skips_mcp_check() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("transport.log");
        let cli = dir.path().join("aicx-cli-mock.sh");
        let cli_text = r#"[{"kind":"intent","summary":"cli-intent","project":"x","agent":"codex","date":"2026-04-28","session_id":"c1","source_chunk":"/tmp/cli.md"}]"#;
        write_cli_mock(&cli, &log, cli_text);

        unsafe {
            clear_aicx_env();
            enable_aicx_for_test();
            std::env::set_var(AICX_MODE_ENV, "cli");
            std::env::set_var(AICX_MCP_BINARY_ENV, "/this/path/must/not/be/probed");
            std::env::set_var(AICX_BINARY_ENV, &cli);
        }
        let client = AicxClient::new("x");
        let intents = client.intents(720, 100);
        unsafe {
            clear_aicx_env();
        }

        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].text, "cli-intent");
        let log_text = std::fs::read_to_string(&log).expect("transport log");
        assert_eq!(log_text.trim(), "cli");
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn auto_mode_falls_back_to_cli_when_mcp_unreachable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("transport.log");
        let cli = dir.path().join("aicx-cli-mock.sh");
        let cli_text = r#"[{"kind":"intent","summary":"fallback-intent","project":"x","agent":"codex","date":"2026-04-28","session_id":"c1","source_chunk":"/tmp/cli.md"}]"#;
        write_cli_mock(&cli, &log, cli_text);

        // Do NOT mutate `HOME` here. It is process-global and this test only
        // holds the `aicx_env` serial lock — snapshot tests in the DEFAULT
        // serial group run concurrently and resolve the loctree cache dir
        // through `HOME`, so a fake home from this test intermittently leaked
        // into their `Snapshot::load` allowed-roots check (observed as a
        // PermissionDenied flake in `strict_acquire_trusts_fresh_snapshot_…`).
        // Nothing in this test reads `HOME`: both transports are explicit
        // mock paths via `AICX_BINARY_ENV` / `AICX_MCP_BINARY_ENV`.
        unsafe {
            clear_aicx_env();
            enable_aicx_for_test();
            std::env::set_var(AICX_MODE_ENV, "auto");
            std::env::set_var(AICX_MCP_BINARY_ENV, "/this/path/does/not/exist/aicx-mcp");
            std::env::set_var(AICX_BINARY_ENV, &cli);
        }
        let client = AicxClient::new("x");
        let intents = client.intents(720, 100);
        unsafe {
            clear_aicx_env();
        }

        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].text, "fallback-intent");
        let log_text = std::fs::read_to_string(&log).expect("transport log");
        assert_eq!(
            log_text.trim(),
            "cli",
            "auto mode should use installed aicx CLI when MCP is unavailable"
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn aicx_mcp_handles_ambient_runtime() {
        // Regression: AicxClient::new instantiated from inside an existing
        // Tokio runtime used to panic with "Cannot start a runtime from
        // within a runtime". After Fix 1+3 the wrapper detects the ambient
        // runtime via Handle::try_current() and uses block_in_place +
        // shared OnceLock runtime to drive blocking calls without panicking.
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("transport.log");
        let mcp = dir.path().join("aicx-mcp-mock.sh");
        let cli = dir.path().join("aicx-cli-mock.sh");
        let mcp_text = r#"[{"kind":"intent","summary":"ambient-mcp","project":"x","agent":"codex","date":"2026-04-28","session_id":"a1","source_chunk":"/tmp/a.md"}]"#;
        write_mcp_mock(&mcp, &log, mcp_text);
        write_cli_mock(&cli, &log, "[]");

        unsafe {
            clear_aicx_env();
            enable_aicx_for_test();
            std::env::set_var(AICX_MODE_ENV, "mcp");
            std::env::set_var(AICX_MCP_BINARY_ENV, &mcp);
            std::env::set_var(AICX_BINARY_ENV, &cli);
        }

        // Build an explicit multi-thread runtime as the AMBIENT one.
        // block_in_place inside connect_and_check / call_json_tool requires
        // multi-thread; current_thread would panic and is documented as the
        // known limitation of this fix.
        let ambient = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .expect("build ambient runtime");
        let intents = ambient.block_on(async {
            // AicxClient::new is sync but internally drives the MCP transport
            // via run_blocking_on; this would panic without the ambient
            // detection branch.
            let client = AicxClient::new("x");
            client.intents(720, 100)
        });

        unsafe {
            clear_aicx_env();
        }

        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].text, "ambient-mcp");
        let log_text = std::fs::read_to_string(&log).expect("transport log");
        assert!(log_text.contains("mcp"));
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn mcp_returns_same_intent_shape_as_cli() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("transport.log");
        let mcp = dir.path().join("aicx-mcp-mock.sh");
        let cli = dir.path().join("aicx-cli-mock.sh");
        let wire_text = r#"[{"kind":"intent","summary":"same-shape","project":"x","agent":"codex","date":"2026-04-28","timestamp":"2026-04-28T00:00:00Z","session_id":"s1","source_chunk":"/tmp/same.md","frame_kind":"agent_reply"}]"#;
        write_mcp_mock(&mcp, &log, wire_text);
        write_cli_mock(&cli, &log, wire_text);

        unsafe {
            clear_aicx_env();
            enable_aicx_for_test();
            std::env::set_var(AICX_MODE_ENV, "cli");
            std::env::set_var(AICX_BINARY_ENV, &cli);
        }
        let cli_intents = AicxClient::new("x").intents(720, 100);

        unsafe {
            clear_aicx_env();
            enable_aicx_for_test();
            std::env::set_var(AICX_MODE_ENV, "mcp");
            std::env::set_var(AICX_MCP_BINARY_ENV, &mcp);
            std::env::set_var(AICX_BINARY_ENV, &cli);
        }
        let mcp_intents = AicxClient::new("x").intents(720, 100);
        unsafe {
            clear_aicx_env();
        }

        assert_eq!(mcp_intents, cli_intents);
        assert_eq!(mcp_intents.len(), 1);
        assert_eq!(mcp_intents[0].frame_kind.as_deref(), Some("agent_reply"));
    }

    // -----------------------------------------------------------------
    // AICX lib contract integration — ProjectScope + SemanticReadiness
    // -----------------------------------------------------------------
    //
    // AICX's CLI/MCP contract supports searching across no project (all),
    // one project, or many projects. Loctree must consume that contract
    // through a typed wrapper instead of fabricating CLI strings, and it
    // must distinguish three retrieval-readiness states so context
    // composers can decide whether the slice is safe to scope on.

    #[test]
    fn project_scope_from_iter_classifies_cardinality() {
        assert!(matches!(
            ProjectScope::from_projects(Vec::<String>::new()),
            ProjectScope::All
        ));
        assert!(matches!(
            ProjectScope::from_projects(["repo-a".to_string()]),
            ProjectScope::Single(ref name) if name == "repo-a"
        ));
        let multi = ProjectScope::from_projects(["repo-a".to_string(), "repo-b".to_string()]);
        assert!(matches!(multi, ProjectScope::Multi(_)));
        assert_eq!(multi.projects(), &["repo-a", "repo-b"]);
    }

    #[test]
    fn project_scope_from_iter_filters_blank_entries() {
        // Empty strings from sloppy callers must not synthesise fake
        // projects. The wrapper trims and drops them — an iter of all
        // blanks collapses to `All`, not `Multi(vec!["", ""])`.
        let scope = ProjectScope::from_projects(["".to_string(), "  ".to_string()]);
        assert!(scope.is_all());
        let with_blanks = ProjectScope::from_projects([
            "repo-a".to_string(),
            "".to_string(),
            "repo-b".to_string(),
        ]);
        assert_eq!(with_blanks.projects(), &["repo-a", "repo-b"]);
    }

    #[test]
    fn project_scope_primary_returns_first_or_empty() {
        assert_eq!(ProjectScope::Single("solo".into()).primary(), "solo");
        assert_eq!(
            ProjectScope::Multi(vec!["a".into(), "b".into()]).primary(),
            "a"
        );
        assert_eq!(ProjectScope::All.primary(), "");
    }

    #[test]
    fn project_scope_cache_key_is_order_independent_for_multi() {
        let a = ProjectScope::from_projects(["x".to_string(), "y".to_string()]);
        let b = ProjectScope::from_projects(["y".to_string(), "x".to_string()]);
        assert_eq!(a.cache_key(), b.cache_key());
    }

    #[test]
    fn aicx_client_all_projects_omits_primary() {
        // `all_projects` constructor reports an empty primary so callers
        // that need a stable label branch on `scope()`, not `project()`.
        let client = AicxClient::all_projects();
        assert!(client.scope().is_all());
        assert_eq!(client.project(), "");
    }

    #[test]
    fn aicx_client_with_projects_keeps_back_compat_single() {
        let client = AicxClient::with_projects(["loctree-suite".to_string()]);
        assert!(matches!(client.scope(), ProjectScope::Single(s) if s == "loctree-suite"));
        assert_eq!(client.project(), "loctree-suite");
    }

    #[test]
    fn semantic_readiness_classifies_oracle_status() {
        // Embedded semantic backend + scope-safe + not stale → Ready.
        let ready = OracleStatus {
            backend: OracleBackend::ContentSemantic,
            loctree_scope_safe: true,
            stale_or_unknown: false,
            ..OracleStatus::default()
        };
        assert!(matches!(ready.readiness(), SemanticReadiness::Ready));

        // Canonical-corpus scan with scope-safe = true → Degraded (not
        // the semantic oracle, but trustworthy).
        let canonical = OracleStatus {
            backend: OracleBackend::CanonicalCorpus,
            loctree_scope_safe: true,
            stale_or_unknown: false,
            ..OracleStatus::default()
        };
        match canonical.readiness() {
            SemanticReadiness::Degraded { reason } => {
                assert!(
                    reason.contains("canonical_corpus"),
                    "degraded reason must reflect backend: {reason}"
                );
            }
            other => panic!("expected Degraded for canonical_corpus, got {other:?}"),
        }

        // Filesystem fuzzy → always Unsafe, even when scope-safe is
        // missing/false. AICX explicitly marks this layer as routing-only.
        let fuzzy = OracleStatus {
            backend: OracleBackend::FilesystemFuzzy,
            loctree_scope_safe: false,
            fallback_reason: Some("content index unavailable".to_string()),
            ..OracleStatus::default()
        };
        match fuzzy.readiness() {
            SemanticReadiness::Unsafe { reason } => {
                assert_eq!(reason, "content index unavailable");
            }
            other => panic!("expected Unsafe for filesystem_fuzzy, got {other:?}"),
        }

        // ContentSemantic but explicitly marked unsafe (e.g. stale index
        // detected) → Unsafe takes precedence over backend identity.
        let stale_unsafe = OracleStatus {
            backend: OracleBackend::ContentSemantic,
            loctree_scope_safe: false,
            ..OracleStatus::default()
        };
        assert!(matches!(
            stale_unsafe.readiness(),
            SemanticReadiness::Unsafe { .. }
        ));
    }

    #[test]
    fn semantic_readiness_min_aggregates_lowest_trust() {
        // The slice aggregate is honest: mixing a Ready row with an
        // Unsafe row yields Unsafe, not Ready. Otherwise the composer
        // would let one good row hide many bad ones.
        let ready = SemanticReadiness::Ready;
        let degraded = SemanticReadiness::Degraded {
            reason: "canonical_corpus".into(),
        };
        let unsafe_state = SemanticReadiness::Unsafe {
            reason: "fuzzy".into(),
        };
        assert!(matches!(
            ready.clone().min(unsafe_state.clone()),
            SemanticReadiness::Unsafe { .. }
        ));
        assert!(matches!(
            degraded.clone().min(unsafe_state.clone()),
            SemanticReadiness::Unsafe { .. }
        ));
        assert!(matches!(
            ready.clone().min(degraded.clone()),
            SemanticReadiness::Degraded { .. }
        ));
        // Unknown collapses to whichever side is concrete.
        let unknown = SemanticReadiness::Unknown;
        assert!(matches!(
            unknown.clone().min(ready.clone()),
            SemanticReadiness::Unknown
        ));
    }

    // -----------------------------------------------------------------
    // Plan L03 / Findings #6 #7 #8 — FailureBudget unit + cache hygiene
    // + hard MCP mode contract. Unit-level coverage; AicxClient-level
    // contract validated through the existing mcp/cli mock harnesses.
    // -----------------------------------------------------------------

    #[test]
    fn failure_budget_starts_untripped() {
        let budget = FailureBudget::new(3, Duration::from_secs(60));
        assert!(!budget.is_tripped());
        assert_eq!(budget.failure_count(), 0);
    }

    #[test]
    fn failure_budget_does_not_trip_below_threshold() {
        let budget = FailureBudget::new(3, Duration::from_secs(60));
        budget.record_failure();
        budget.record_failure();
        assert!(
            !budget.is_tripped(),
            "two failures must stay below threshold"
        );
        assert_eq!(budget.failure_count(), 2);
    }

    #[test]
    fn failure_budget_trips_at_threshold() {
        let budget = FailureBudget::new(3, Duration::from_secs(60));
        budget.record_failure();
        budget.record_failure();
        budget.record_failure();
        assert!(budget.is_tripped(), "third failure must trip the breaker");
    }

    #[test]
    fn failure_budget_resets_after_window() {
        // Tiny window so the test runs fast — semantics, not duration.
        let budget = FailureBudget::new(2, Duration::from_millis(50));
        budget.record_failure();
        budget.record_failure();
        assert!(budget.is_tripped(), "tripped immediately after threshold");
        std::thread::sleep(Duration::from_millis(75));
        assert!(
            !budget.is_tripped(),
            "breaker must auto-reset once window elapses"
        );
        // Next failure starts a fresh window — counter resets to 1.
        budget.record_failure();
        assert!(
            !budget.is_tripped(),
            "single failure in fresh window must not trip"
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(aicx_env)]
    fn cache_does_not_serve_failures_as_empty_success() {
        // Plan L03 / Finding #8 — when `aicx` cannot run on call N,
        // call N+1 must retry and pick up a successful response. The
        // toxic-empty-cache bug would have memoised `Vec::new()` on
        // call N and then served it on call N+1.
        //
        // Strategy: counter file controls behaviour. First invocation
        // exits non-zero (no payload). Second invocation prints a real
        // intent. Without the fix the second call returned empty.
        let dir = tempfile::tempdir().expect("tempdir");
        let counter = dir.path().join("counter");
        std::fs::write(&counter, "0").unwrap();
        let script = dir.path().join("aicx-mock.sh");
        let body = format!(
            "#!/bin/sh\nN=$(cat '{counter}')\nNEXT=$((N + 1))\necho \"$NEXT\" > '{counter}'\nif [ \"$N\" = \"0\" ]; then\n  exit 1\nfi\nprintf '[{{\"kind\":\"intent\",\"summary\":\"recovered\",\"project\":\"x\",\"agent\":\"claude\",\"date\":\"2026-04-28\",\"session_id\":\"r1\",\"source_chunk\":\"/tmp/r.md\"}}]'\n",
            counter = counter.display()
        );
        write_script(&script, &body);

        unsafe {
            clear_aicx_env();
            enable_aicx_for_test();
            std::env::set_var(AICX_MODE_ENV, "cli");
            std::env::set_var(AICX_BINARY_ENV, &script);
        }
        let client = AicxClient::new("x");
        let first = client.intents(720, 100);
        let second = client.intents(720, 100);
        unsafe {
            clear_aicx_env();
        }

        assert!(
            first.is_empty(),
            "first call returns empty (script exits 1)"
        );
        assert_eq!(
            second.len(),
            1,
            "second call must retry and recover, not be poisoned by cache"
        );
        assert_eq!(second[0].text, "recovered");
    }
}
