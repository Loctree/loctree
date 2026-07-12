//! Custom LSP request: `loctree/semantic` (Plan 14).
//!
//! Surfaces Loctree's meaning layer — idiom tags, dispatch edges,
//! reachability, env contracts, Tauri command/event bridges, and
//! framework hints — over JSON-RPC. The LSP daemon gives an agent the
//! same semantic facts the CLI's `loct context --runtime` exposes,
//! without round-tripping through the binary.
//!
//! ## Contract
//!
//! - Params: `scope`, `target`, `kinds` (filter), `project`.
//! - `scope = "file"` is fully implemented: delegates to
//!   [`loctree::pack::compose_runtime_slice`] with
//!   `ContextOptions { file: Some(target), project, .. }`.
//! - `scope = "symbol"` is staged for v2 — returns
//!   `status: "symbol_scope_unimplemented"` with a hint to use file
//!   scope (the original plan flagged this as a known constraint).
//! - `scope = "project"` aggregates per-file runtime facts across the
//!   snapshot and applies the Plan 12 cursor pagination so the
//!   response stays inside the host JSON-RPC payload limit.
//! - Every entry preserves its `AuthorityLabel` so agents can weigh
//!   `RepoVerified` vs. `LoctreeDerived` vs. `SemanticGuess`.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::collections::HashSet;
use std::path::PathBuf;

use loctree::pack::{
    ContextOptions, RuntimeDispatchEdge, RuntimeEnvContract, RuntimeFrameworkHint, RuntimeIdiomTag,
    RuntimeReachability, RuntimeSlice, RuntimeTauriCommand, RuntimeTauriEvent,
    compose_runtime_slice,
};
use loctree::snapshot::Snapshot;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::cursor::CursorState;
use crate::protocol::{Paginated, paginate, single_page};

/// Default snapshot id used when the routed snapshot has no recorded
/// commit/scan id. The id is what the cursor pattern (Plan 12) keys
/// pagination off — even a stable placeholder gives chunked clients a
/// continuation token they can present back to the daemon.
const SNAPSHOT_ID_FALLBACK: &str = "loctree-lsp:semantic";

/// Request params for `loctree/semantic`.
#[derive(Debug, Clone, Deserialize, Default, JsonSchema)]
pub struct SemanticParams {
    /// `"file"`, `"symbol"`, or `"project"`. Required.
    pub scope: String,
    /// Repo-relative or absolute target path / symbol id. Required for
    /// `scope = "file"` and `scope = "symbol"`.
    #[serde(default)]
    pub target: Option<String>,
    /// Optional filter: subset of `idiom_tags`, `dispatch_edges`,
    /// `reachability`, `env_contracts`, `tauri_commands`,
    /// `tauri_events`, `framework_hints`. `None` = all kinds.
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
    /// Plan 13 multi-workspace routing override.
    #[serde(default)]
    pub project: Option<PathBuf>,
    /// Plan 12 cursor — pass back the `next_cursor` from a prior
    /// response to fetch the next chunk of `scope = "project"` data.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Plan 12 chunk size override (clamped server-side).
    #[serde(default)]
    pub chunk_size: Option<usize>,
}

/// Wire shape for `loctree/semantic`.
#[derive(Debug, Clone, Serialize)]
pub struct SemanticResponse {
    /// `"ok"` (data populated) or `"symbol_scope_unimplemented"`
    /// (v1 limitation — see module docs).
    pub status: String,
    /// Free-form hint — populated for non-`ok` statuses to tell the
    /// caller what to do next.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Echo of the resolved scope (`file` / `symbol` / `project`).
    pub scope: String,
    /// Echo of the routed target. `None` for `scope = "project"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Filtered runtime facts. Empty when `status != "ok"`.
    pub data: SemanticData,
    /// Plan 12 pagination envelope — populated for `scope = "project"`
    /// when the response was chunked. `chunk = 0`, `total_chunks = 1`,
    /// `next_cursor = None` for non-paginated responses.
    pub pagination: SemanticPagination,
}

/// Filtered runtime facts. Mirrors [`RuntimeSlice`] verbatim so an
/// agent can deserialize a single shape regardless of which kinds it
/// asked for; unrequested arrays are simply empty.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SemanticData {
    pub idiom_tags: Vec<RuntimeIdiomTag>,
    pub dispatch_edges: Vec<RuntimeDispatchEdge>,
    pub reachability: Vec<RuntimeReachability>,
    pub env_contracts: Vec<RuntimeEnvContract>,
    pub tauri_commands: Vec<RuntimeTauriCommand>,
    pub tauri_events: Vec<RuntimeTauriEvent>,
    pub framework_hints: Vec<RuntimeFrameworkHint>,
}

/// Pagination metadata mirrored from [`Paginated`] but flattened so
/// the response shape stays uniform whether or not chunking is active.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SemanticPagination {
    pub chunk: u32,
    pub total_chunks: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Catalog of `kinds` filter values. Order matches [`RuntimeSlice`]
/// fields so the response shape and the kinds list stay in lockstep.
pub const ALL_KINDS: &[&str] = &[
    "idiom_tags",
    "dispatch_edges",
    "reachability",
    "env_contracts",
    "tauri_commands",
    "tauri_events",
    "framework_hints",
];

/// Build a [`ContextOptions`] for the `scope = "file"` path. The LSP
/// composer only needs `file` and `project`; everything else stays at
/// default — JSON / markdown emission is for the CLI.
pub fn options_for_file(target: &str, project: Option<PathBuf>) -> ContextOptions {
    ContextOptions {
        file: Some(PathBuf::from(target)),
        project,
        ..ContextOptions::default()
    }
}

/// Apply the `kinds` filter to a [`RuntimeSlice`], producing a
/// [`SemanticData`] with everything else zeroed out.
pub fn filter_kinds(slice: RuntimeSlice, kinds: &Option<Vec<String>>) -> SemanticData {
    let allowed: HashSet<&str> = match kinds {
        Some(list) if !list.is_empty() => list.iter().map(String::as_str).collect(),
        _ => ALL_KINDS.iter().copied().collect(),
    };

    let RuntimeSlice {
        idiom_tags,
        dispatch_edges,
        reachability,
        env_contracts,
        tauri_commands,
        tauri_events,
        framework_hints,
    } = slice;

    SemanticData {
        idiom_tags: keep("idiom_tags", &allowed, idiom_tags),
        dispatch_edges: keep("dispatch_edges", &allowed, dispatch_edges),
        reachability: keep("reachability", &allowed, reachability),
        env_contracts: keep("env_contracts", &allowed, env_contracts),
        tauri_commands: keep("tauri_commands", &allowed, tauri_commands),
        tauri_events: keep("tauri_events", &allowed, tauri_events),
        framework_hints: keep("framework_hints", &allowed, framework_hints),
    }
}

fn keep<T>(label: &str, allowed: &HashSet<&str>, value: Vec<T>) -> Vec<T> {
    if allowed.contains(label) {
        value
    } else {
        Vec::new()
    }
}

/// Compute the per-file slice for `scope = "file"`. Pulled out of the
/// Backend handler so unit tests can drive the composer directly.
pub fn compute_file_scope(
    snapshot: &Snapshot,
    target: &str,
    project: Option<PathBuf>,
    kinds: &Option<Vec<String>>,
) -> SemanticData {
    let opts = options_for_file(target, project);
    let slice = compose_runtime_slice(&opts, snapshot);
    filter_kinds(slice, kinds)
}

/// Aggregate every per-file runtime slice across the snapshot for
/// `scope = "project"`. Used by the Backend handler before pagination.
pub fn compute_project_scope(
    snapshot: &Snapshot,
    project: Option<PathBuf>,
    kinds: &Option<Vec<String>>,
) -> SemanticData {
    let mut aggregate = SemanticData::default();
    for file in &snapshot.files {
        let opts = options_for_file(&file.path, project.clone());
        let slice = compose_runtime_slice(&opts, snapshot);
        let data = filter_kinds(slice, kinds);
        merge_into(&mut aggregate, data);
    }
    dedup_by_authority_aware(&mut aggregate);
    aggregate
}

fn merge_into(dst: &mut SemanticData, src: SemanticData) {
    dst.idiom_tags.extend(src.idiom_tags);
    dst.dispatch_edges.extend(src.dispatch_edges);
    dst.reachability.extend(src.reachability);
    dst.env_contracts.extend(src.env_contracts);
    dst.tauri_commands.extend(src.tauri_commands);
    dst.tauri_events.extend(src.tauri_events);
    dst.framework_hints.extend(src.framework_hints);
}

/// Deduplicate aggregated facts by their natural identity — a fact
/// surfaced by both an importer and an exporter scope shows up twice
/// in the project aggregate. The composer already sorts each per-file
/// slice; we only need to drop the duplicates here.
fn dedup_by_authority_aware(data: &mut SemanticData) {
    let mut seen = HashSet::new();
    data.idiom_tags
        .retain(|tag| seen.insert(format!("{}::{}", tag.symbol, tag.name)));

    let mut seen = HashSet::new();
    data.dispatch_edges.retain(|edge| {
        seen.insert(format!(
            "{}:{}->{}",
            edge.from_file, edge.from_line, edge.handler_symbol
        ))
    });

    let mut seen = HashSet::new();
    data.reachability
        .retain(|reach| seen.insert(format!("{}::{}", reach.symbol, reach.reached)));

    let mut seen = HashSet::new();
    data.env_contracts
        .retain(|env| seen.insert(env.name.clone()));

    let mut seen = HashSet::new();
    data.tauri_commands
        .retain(|cmd| seen.insert(cmd.name.clone()));

    let mut seen = HashSet::new();
    data.tauri_events.retain(|ev| seen.insert(ev.name.clone()));

    let mut seen = HashSet::new();
    data.framework_hints
        .retain(|h| seen.insert(format!("{}|{}|{}", h.kind, h.symbol, h.file)));
}

/// Build a `symbol_scope_unimplemented` response.
pub fn symbol_scope_response(target: Option<String>) -> SemanticResponse {
    SemanticResponse {
        status: "symbol_scope_unimplemented".to_string(),
        hint: Some(
            "loctree/semantic v1 supports scope=file and scope=project. \
             Pass scope=file with target=<symbol's containing file> instead."
                .to_string(),
        ),
        scope: "symbol".to_string(),
        target,
        data: SemanticData::default(),
        pagination: SemanticPagination::default(),
    }
}

/// Snapshot id used by [`paginate`] when a `scope = "project"` response
/// needs chunking. Falls back to a static label when the snapshot
/// metadata has no commit/scan_id (test fixtures, fresh scans).
pub fn snapshot_pagination_id(snapshot: &Snapshot) -> String {
    snapshot
        .metadata
        .git_scan_id
        .clone()
        .or_else(|| snapshot.metadata.git_commit.clone())
        .unwrap_or_else(|| SNAPSHOT_ID_FALLBACK.to_string())
}

/// For project-scope responses, chunk the dispatch_edges array (the
/// largest-by-far in real repos) and treat the rest as side data on
/// chunk 0. Subsequent chunks carry only edges. This keeps the wire
/// shape predictable while still respecting the host JSON-RPC limit.
pub fn paginate_project(
    mut data: SemanticData,
    cursor: Option<&str>,
    chunk_size: usize,
    snapshot_id: &str,
) -> Result<(SemanticData, SemanticPagination), crate::cursor::CursorError> {
    let kind = "loctree/semantic.project";
    let edges = std::mem::take(&mut data.dispatch_edges);
    let offset = match cursor {
        Some(token) => CursorState::decode(token, snapshot_id, kind)?.offset,
        None => 0,
    };
    let page: Paginated<Vec<RuntimeDispatchEdge>> =
        paginate(&edges, offset, chunk_size, snapshot_id, kind)?;

    // Side data only on the first chunk — subsequent chunks carry
    // dispatch edges only so the host stays under the response cap.
    if page.chunk > 0 {
        data = SemanticData {
            dispatch_edges: page.data.clone(),
            ..SemanticData::default()
        };
    } else {
        data.dispatch_edges = page.data.clone();
    }

    let pagination = SemanticPagination {
        chunk: page.chunk,
        total_chunks: page.total_chunks,
        next_cursor: page.next_cursor.clone(),
    };
    Ok((data, pagination))
}

/// Trivial single-page envelope for `scope = "file"` responses.
/// Mirrors the `Paginated::single` shape from `protocol.rs` so the
/// wire envelope stays uniform across paginated and non-paginated
/// endpoints. Concrete typing avoids an unsafe zeroing dance.
pub fn singleton_pagination() -> SemanticPagination {
    let single: Paginated<()> = single_page(());
    SemanticPagination {
        chunk: single.chunk,
        total_chunks: single.total_chunks,
        next_cursor: single.next_cursor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loctree::pack::AuthorityLabel;

    #[test]
    fn kinds_filter_accepts_all_when_none() {
        let slice = RuntimeSlice {
            idiom_tags: vec![RuntimeIdiomTag {
                symbol: "f::g".into(),
                name: "n".into(),
                classifier: "c".into(),
                runtime_role: "r".into(),
                source: "s".into(),
                reasoning: "x".into(),
                authority: AuthorityLabel::LoctreeDerived,
            }],
            ..RuntimeSlice::default()
        };
        let data = filter_kinds(slice, &None);
        assert_eq!(data.idiom_tags.len(), 1);
    }

    #[test]
    fn kinds_filter_drops_unrequested_kinds() {
        let slice = RuntimeSlice {
            idiom_tags: vec![RuntimeIdiomTag {
                symbol: "f::g".into(),
                name: "n".into(),
                classifier: "c".into(),
                runtime_role: "r".into(),
                source: "s".into(),
                reasoning: "x".into(),
                authority: AuthorityLabel::LoctreeDerived,
            }],
            env_contracts: vec![RuntimeEnvContract {
                name: "API_KEY".into(),
                used_in_files: vec![],
                required_for: vec![],
                occurrences: vec![],
                required: true,
                authority: AuthorityLabel::LoctreeDerived,
            }],
            ..RuntimeSlice::default()
        };
        let data = filter_kinds(slice, &Some(vec!["env_contracts".to_string()]));
        assert!(data.idiom_tags.is_empty());
        assert_eq!(data.env_contracts.len(), 1);
    }

    #[test]
    fn idiom_tag_authority_repo_verified_round_trips() {
        let slice = RuntimeSlice {
            idiom_tags: vec![RuntimeIdiomTag {
                symbol: "src/lib.rs::main".into(),
                name: "entrypoint".into(),
                classifier: "runtime".into(),
                runtime_role: "entrypoint".into(),
                source: "snapshot".into(),
                reasoning: "repo-visible entrypoint".into(),
                authority: AuthorityLabel::RepoVerified,
            }],
            ..RuntimeSlice::default()
        };

        let data = filter_kinds(slice, &Some(vec!["idiom_tags".to_string()]));
        assert_eq!(data.idiom_tags[0].authority, AuthorityLabel::RepoVerified);
        let json = serde_json::to_value(&data).expect("semantic data serializes");
        assert_eq!(json["idiom_tags"][0]["authority"], "repo_verified");
    }

    #[test]
    fn dispatch_edge_authority_aicx_agent_round_trips() {
        let slice = RuntimeSlice {
            dispatch_edges: vec![RuntimeDispatchEdge {
                from_file: "Makefile".into(),
                from_line: 42,
                dispatch_kind: "recipe_shell_call".into(),
                handler_symbol: "cargo".into(),
                handler_file: None,
                framework: None,
                http_method: None,
                http_path: None,
                authority: AuthorityLabel::AicxAgent,
            }],
            ..RuntimeSlice::default()
        };

        let data = filter_kinds(slice, &Some(vec!["dispatch_edges".to_string()]));
        assert_eq!(data.dispatch_edges[0].authority, AuthorityLabel::AicxAgent);
        let json = serde_json::to_value(&data).expect("semantic data serializes");
        assert_eq!(json["dispatch_edges"][0]["authority"], "aicx_agent");
    }

    #[test]
    fn env_contract_authority_semantic_guess_round_trips() {
        let slice = RuntimeSlice {
            env_contracts: vec![RuntimeEnvContract {
                name: "AICX_BASE_URL".into(),
                used_in_files: vec!["src/aicx.rs".into()],
                required_for: vec!["memory lookup".into()],
                occurrences: vec![],
                required: true,
                authority: AuthorityLabel::SemanticGuess,
            }],
            ..RuntimeSlice::default()
        };

        let data = filter_kinds(slice, &Some(vec!["env_contracts".to_string()]));
        assert_eq!(
            data.env_contracts[0].authority,
            AuthorityLabel::SemanticGuess
        );
        let json = serde_json::to_value(&data).expect("semantic data serializes");
        assert_eq!(json["env_contracts"][0]["authority"], "semantic_guess");
    }

    #[test]
    fn tauri_command_kinds_filter_drops_when_unrequested() {
        let slice = RuntimeSlice {
            tauri_commands: vec![RuntimeTauriCommand {
                name: "open_report".into(),
                handler_file: Some("src-tauri/src/main.rs".into()),
                handler_line: Some(17),
                invoke_site_count: 1,
                has_handler: true,
                is_called: true,
                authority: AuthorityLabel::RepoVerified,
            }],
            ..RuntimeSlice::default()
        };

        let data = filter_kinds(slice, &Some(vec!["env_contracts".to_string()]));
        assert!(data.tauri_commands.is_empty());
    }

    #[test]
    fn symbol_scope_response_has_hint() {
        let resp = symbol_scope_response(Some("Foo".into()));
        assert_eq!(resp.status, "symbol_scope_unimplemented");
        assert!(resp.hint.is_some());
        assert_eq!(resp.scope, "symbol");
    }
}
