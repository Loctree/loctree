//! Custom LSP request: `loctree/diff` (Plan 11).
//!
//! Session-local structural deltas for the routed workspace —
//! "what changed since I last asked?". The daemon keeps a per-
//! workspace previous snapshot (snapshotted by the watcher each time
//! it reloads) and, separately, a per-session `lastQuery` marker
//! advanced on every successful response.
//!
//! ## Modes (`since` field)
//!
//! - `epoch` → diff against an empty baseline (full inventory). Always
//!   safe; useful for boot.
//! - `lastScan` → diff against the snapshot the watcher reloaded
//!   *before* the most recent reload. Captures filesystem activity
//!   the daemon already absorbed.
//! - `lastQuery` → diff against whatever the same session asked for
//!   most recently. Advances on every successful response so calls
//!   chain.
//! - any other value (intended for git revs, e.g. `HEAD~1`) → typed
//!   `unsupported_since` error. v1 deliberately defers git-rev
//!   diffing to a follow-up cut so the wire shape doesn't ship a
//!   half-baked promise.
//!
//! ## Wire shape
//!
//! Files are reported as paths. Edges and symbols mirror the canonical
//! [`loctree::diff`] structures so an agent can deserialize them
//! against the same types it already knows. `since_marker` carries an
//! opaque label the caller can pass back as `since` to resume.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use loctree::snapshot::Snapshot;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::cursor::{CursorError, CursorState};
use crate::protocol::{DEFAULT_CHUNK_SIZE, Paginated, paginate, single_page};

const MAX_PINNED_SNAPSHOTS: usize = 32;
const SNAPSHOT_ID_FALLBACK: &str = "snapshot:unknown";
const EDGES_ADDED_CURSOR_KIND: &str = "loctree/diff.edges_added";
const EDGES_REMOVED_CURSOR_KIND: &str = "loctree/diff.edges_removed";
const SYMBOLS_ADDED_CURSOR_KIND: &str = "loctree/diff.symbols_added";
const SYMBOLS_REMOVED_CURSOR_KIND: &str = "loctree/diff.symbols_removed";

/// Request params for `loctree/diff`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DiffParams {
    /// `epoch` | `lastScan` | `lastQuery` | git-rev (rejected v1).
    pub since: String,
    /// Plan 13 multi-workspace routing override.
    #[serde(default)]
    pub project: Option<PathBuf>,
    /// Opaque Plan 12 cursor returned by one of the delta buckets.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Requested page size for paginated edge/symbol deltas.
    #[serde(default)]
    pub chunk_size: Option<usize>,
}

/// One added/removed import edge in the wire response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct DiffEdge {
    pub from: String,
    pub to: String,
    pub label: String,
}

/// One added/removed symbol in the wire response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct DiffSymbol {
    pub file: String,
    pub name: String,
    pub kind: String,
}

/// Wire shape for `loctree/diff`.
#[derive(Debug, Clone, Serialize)]
pub struct DiffResponse {
    /// `"ok"` (data populated) or `"unsupported_since"` (caller asked
    /// for a git rev or unknown marker — see module docs).
    pub status: String,
    /// Free-form hint — populated for non-`ok` statuses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Echo of the resolved baseline (`epoch`, `lastScan`,
    /// `lastQuery`, or the original input on error).
    pub since: String,
    pub files_added: Vec<String>,
    pub files_removed: Vec<String>,
    pub files_changed: Vec<String>,
    pub edges_added: Paginated<Vec<DiffEdge>>,
    pub edges_removed: Paginated<Vec<DiffEdge>>,
    pub symbols_added: Paginated<Vec<DiffSymbol>>,
    pub symbols_removed: Paginated<Vec<DiffSymbol>>,
    /// Opaque label callers can pass back as `since` to resume from
    /// the current snapshot. Populated when `status == "ok"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_marker: Option<String>,
}

/// Per-workspace history maintained by the LSP backend.
///
/// `last_scan` is rotated by the watcher each time it reloads; the
/// new snapshot becomes the "current" one and the previous "current"
/// shifts here. `last_query` is advanced by [`DiffSession::advance`]
/// every time a `loctree/diff` request resolves — that turns
/// `since: "lastQuery"` into a chainable cursor.
#[derive(Debug, Default)]
pub struct DiffSession {
    last_scan: Option<Arc<Snapshot>>,
    last_query: Option<Arc<Snapshot>>,
    pinned_snapshots: HashMap<String, Arc<Snapshot>>,
    pinned_order: VecDeque<String>,
}

/// Thread-safe handle to a [`DiffSession`].
pub type SharedDiffSession = Arc<RwLock<DiffSession>>;

impl DiffSession {
    /// Replace the `last_scan` snapshot. Called by the watcher right
    /// before it loads a fresh one — the previous current snapshot
    /// becomes the new `last_scan` baseline.
    pub fn set_last_scan(&mut self, snapshot: Arc<Snapshot>) {
        self.pin_snapshot(snapshot.clone());
        self.last_scan = Some(snapshot);
    }

    /// Replace the `last_query` snapshot. Called from the diff
    /// handler after each successful response — the snapshot that
    /// served the response becomes the next baseline for
    /// `since: "lastQuery"`.
    pub fn advance(&mut self, snapshot: Arc<Snapshot>) {
        self.pin_snapshot(snapshot.clone());
        self.last_query = Some(snapshot);
    }

    /// Reset both markers — used when a workspace's snapshot is
    /// re-discovered and the prior history would be a phantom.
    pub fn reset(&mut self) {
        self.last_scan = None;
        self.last_query = None;
        self.pinned_snapshots.clear();
        self.pinned_order.clear();
    }

    pub fn last_scan(&self) -> Option<Arc<Snapshot>> {
        self.last_scan.clone()
    }

    pub fn last_query(&self) -> Option<Arc<Snapshot>> {
        self.last_query.clone()
    }

    /// Resolve an opaque `snapshot:*` marker previously emitted by
    /// [`compute`]. The backend uses this to make `since_marker`
    /// round-trip instead of treating it like an unsupported git rev.
    pub fn snapshot_for_marker(&self, marker: &str) -> Option<Arc<Snapshot>> {
        self.pinned_snapshots.get(marker).cloned()
    }

    fn pin_snapshot(&mut self, snapshot: Arc<Snapshot>) {
        let marker = snapshot_marker(&snapshot);
        if !self.pinned_snapshots.contains_key(&marker) {
            self.pinned_order.push_back(marker.clone());
        }
        self.pinned_snapshots.insert(marker, snapshot);

        while self.pinned_order.len() > MAX_PINNED_SNAPSHOTS {
            if let Some(old_marker) = self.pinned_order.pop_front() {
                self.pinned_snapshots.remove(&old_marker);
            }
        }
    }
}

/// Compute the actual diff for a `(prev?, current)` pair.
///
/// `prev = None` means epoch — every entity in `current` shows up as
/// "added", nothing removed. The implementation deliberately avoids
/// `loctree::diff::SnapshotDiff::compare` because that helper expects
/// a `[ChangedFile]` slice from libgit2. For session-local diffs we
/// don't have one — we infer file-level deltas straight from the
/// snapshot file paths instead.
pub fn compute(prev: Option<&Snapshot>, current: &Snapshot, since_label: &str) -> DiffResponse {
    compute_paginated(
        prev,
        current,
        since_label,
        None,
        DEFAULT_CHUNK_SIZE,
        SNAPSHOT_ID_FALLBACK,
    )
    .expect("default diff pagination should not fail")
}

/// Compute the diff and wrap edge/symbol buckets in Plan 12 cursor pages.
pub fn compute_paginated(
    prev: Option<&Snapshot>,
    current: &Snapshot,
    since_label: &str,
    cursor: Option<&str>,
    chunk_size: usize,
    snapshot_id: &str,
) -> Result<DiffResponse, CursorError> {
    let (files_added, files_removed, files_changed) = files_diff(prev, current);
    let (edges_added, edges_removed) = edges_diff(prev, current);
    let (symbols_added, symbols_removed) = symbols_diff(prev, current);
    let cursor_state = match cursor {
        Some(token) => Some(CursorState::decode_raw(token)?),
        None => None,
    };
    let offsets = diff_offsets(cursor_state.as_ref(), snapshot_id)?;

    Ok(DiffResponse {
        status: "ok".to_string(),
        hint: None,
        since: since_label.to_string(),
        files_added,
        files_removed,
        files_changed,
        edges_added: paginate(
            &edges_added,
            offsets.edges_added,
            chunk_size,
            snapshot_id,
            EDGES_ADDED_CURSOR_KIND,
        )?,
        edges_removed: paginate(
            &edges_removed,
            offsets.edges_removed,
            chunk_size,
            snapshot_id,
            EDGES_REMOVED_CURSOR_KIND,
        )?,
        symbols_added: paginate(
            &symbols_added,
            offsets.symbols_added,
            chunk_size,
            snapshot_id,
            SYMBOLS_ADDED_CURSOR_KIND,
        )?,
        symbols_removed: paginate(
            &symbols_removed,
            offsets.symbols_removed,
            chunk_size,
            snapshot_id,
            SYMBOLS_REMOVED_CURSOR_KIND,
        )?,
        since_marker: Some(snapshot_marker(current)),
    })
}

#[derive(Debug, Clone, Copy, Default)]
struct DiffOffsets {
    edges_added: usize,
    edges_removed: usize,
    symbols_added: usize,
    symbols_removed: usize,
}

fn diff_offsets(
    cursor: Option<&CursorState>,
    snapshot_id: &str,
) -> Result<DiffOffsets, CursorError> {
    let Some(cursor) = cursor else {
        return Ok(DiffOffsets::default());
    };
    if cursor.snapshot_id != snapshot_id {
        return Err(CursorError::SnapshotDrifted {
            expected: snapshot_id.into(),
            got: cursor.snapshot_id.clone(),
        });
    }
    let mut offsets = DiffOffsets::default();
    match cursor.kind.as_str() {
        EDGES_ADDED_CURSOR_KIND => offsets.edges_added = cursor.offset,
        EDGES_REMOVED_CURSOR_KIND => offsets.edges_removed = cursor.offset,
        SYMBOLS_ADDED_CURSOR_KIND => offsets.symbols_added = cursor.offset,
        SYMBOLS_REMOVED_CURSOR_KIND => offsets.symbols_removed = cursor.offset,
        other => {
            return Err(CursorError::KindMismatch {
                expected: format!(
                    "{EDGES_ADDED_CURSOR_KIND}|{EDGES_REMOVED_CURSOR_KIND}|{SYMBOLS_ADDED_CURSOR_KIND}|{SYMBOLS_REMOVED_CURSOR_KIND}"
                ),
                got: other.into(),
            });
        }
    }
    Ok(offsets)
}

/// Build the marker label clients pass back as `since`. Prefers the
/// snapshot's git scan id, falls back to commit, then to a stable
/// placeholder. The placeholder is intentionally distinct from
/// `epoch`/`lastScan`/`lastQuery` so it can never accidentally route
/// back into the special-baseline path.
pub fn snapshot_marker(snapshot: &Snapshot) -> String {
    if let Some(scan) = &snapshot.metadata.git_scan_id {
        format!("snapshot:{scan}")
    } else if let Some(commit) = &snapshot.metadata.git_commit {
        format!("snapshot:{commit}")
    } else {
        "snapshot:current".to_string()
    }
}

/// Build an `unsupported_since` response — used when the caller asked
/// for a git rev or any non-special marker.
pub fn unsupported_since(input: &str) -> DiffResponse {
    DiffResponse {
        status: "unsupported_since".to_string(),
        hint: Some(format!(
            "loctree/diff v1 supports `epoch`, `lastScan`, `lastQuery`, and cached `snapshot:*` markers. \
             Git-rev diffing or unknown markers (`{input}`) are staged for v2 — use `loct diff` from the CLI in the meantime."
        )),
        since: input.to_string(),
        files_added: Vec::new(),
        files_removed: Vec::new(),
        files_changed: Vec::new(),
        edges_added: single_page(Vec::new()),
        edges_removed: single_page(Vec::new()),
        symbols_added: single_page(Vec::new()),
        symbols_removed: single_page(Vec::new()),
        since_marker: None,
    }
}

fn files_diff(
    prev: Option<&Snapshot>,
    current: &Snapshot,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let prev_paths: HashSet<&str> = match prev {
        Some(p) => p.files.iter().map(|f| f.path.as_str()).collect(),
        None => HashSet::new(),
    };
    let current_paths: HashSet<&str> = current.files.iter().map(|f| f.path.as_str()).collect();

    let mut added: Vec<String> = current_paths
        .difference(&prev_paths)
        .map(|s| (*s).to_string())
        .collect();
    let mut removed: Vec<String> = prev_paths
        .difference(&current_paths)
        .map(|s| (*s).to_string())
        .collect();

    // Files that exist in both — a real change is when the file's
    // export set or import set differs. Pure LOC drift would not be
    // visible from the snapshot anyway.
    let mut changed: Vec<String> = Vec::new();
    if let Some(prev_snapshot) = prev {
        let prev_files: std::collections::HashMap<&str, &loctree::types::FileAnalysis> =
            prev_snapshot
                .files
                .iter()
                .map(|f| (f.path.as_str(), f))
                .collect();
        for current_file in &current.files {
            if let Some(prev_file) = prev_files.get(current_file.path.as_str())
                && file_analysis_changed(prev_file, current_file)
            {
                changed.push(current_file.path.clone());
            }
        }
    }

    added.sort();
    removed.sort();
    changed.sort();
    (added, removed, changed)
}

fn file_analysis_changed(
    prev: &loctree::types::FileAnalysis,
    current: &loctree::types::FileAnalysis,
) -> bool {
    if prev.exports.len() != current.exports.len() || prev.imports.len() != current.imports.len() {
        return true;
    }
    let prev_exports: HashSet<(&str, &str)> = prev
        .exports
        .iter()
        .map(|e| (e.name.as_str(), e.kind.as_str()))
        .collect();
    let current_exports: HashSet<(&str, &str)> = current
        .exports
        .iter()
        .map(|e| (e.name.as_str(), e.kind.as_str()))
        .collect();
    if prev_exports != current_exports {
        return true;
    }
    let prev_imports: HashSet<(&str, &str)> = prev
        .imports
        .iter()
        .map(|i| (i.source.as_str(), i.resolved_path.as_deref().unwrap_or("")))
        .collect();
    let current_imports: HashSet<(&str, &str)> = current
        .imports
        .iter()
        .map(|i| (i.source.as_str(), i.resolved_path.as_deref().unwrap_or("")))
        .collect();
    prev_imports != current_imports
}

fn edges_diff(prev: Option<&Snapshot>, current: &Snapshot) -> (Vec<DiffEdge>, Vec<DiffEdge>) {
    let to_set = |edges: &[loctree::snapshot::GraphEdge]| -> HashSet<DiffEdge> {
        edges
            .iter()
            .map(|e| DiffEdge {
                from: e.from.clone(),
                to: e.to.clone(),
                label: e.label.clone(),
            })
            .collect()
    };
    let prev_edges: HashSet<DiffEdge> = match prev {
        Some(p) => to_set(&p.edges),
        None => HashSet::new(),
    };
    let current_edges: HashSet<DiffEdge> = to_set(&current.edges);

    let mut added: Vec<DiffEdge> = current_edges.difference(&prev_edges).cloned().collect();
    let mut removed: Vec<DiffEdge> = prev_edges.difference(&current_edges).cloned().collect();
    added.sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.to.cmp(&b.to)));
    removed.sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.to.cmp(&b.to)));
    (added, removed)
}

fn symbols_diff(prev: Option<&Snapshot>, current: &Snapshot) -> (Vec<DiffSymbol>, Vec<DiffSymbol>) {
    let to_set = |files: &[loctree::types::FileAnalysis]| -> HashSet<DiffSymbol> {
        let mut set = HashSet::new();
        for file in files {
            for export in &file.exports {
                set.insert(DiffSymbol {
                    file: file.path.clone(),
                    name: export.name.clone(),
                    kind: export.kind.clone(),
                });
            }
        }
        set
    };
    let prev_symbols: HashSet<DiffSymbol> = match prev {
        Some(p) => to_set(&p.files),
        None => HashSet::new(),
    };
    let current_symbols: HashSet<DiffSymbol> = to_set(&current.files);

    let mut added: Vec<DiffSymbol> = current_symbols.difference(&prev_symbols).cloned().collect();
    let mut removed: Vec<DiffSymbol> = prev_symbols.difference(&current_symbols).cloned().collect();
    added.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.kind.cmp(&b.kind))
    });
    removed.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.kind.cmp(&b.kind))
    });
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use loctree::snapshot::{GraphEdge, Snapshot};
    use loctree::types::{ExportSymbol, FileAnalysis};

    fn export(name: &str) -> ExportSymbol {
        ExportSymbol {
            name: name.to_string(),
            kind: "function".to_string(),
            export_type: "named".to_string(),
            line: Some(1),
            params: Vec::new(),

            symbol_id: ::loctree::types::SymbolIdV1::default(),
        }
    }

    fn snapshot_with(files: Vec<FileAnalysis>, edges: Vec<GraphEdge>) -> Snapshot {
        let mut s = Snapshot::new(vec![".".to_string()]);
        s.files = files;
        s.edges = edges;
        s
    }

    #[test]
    fn epoch_returns_full_inventory_as_added() {
        let current = snapshot_with(
            vec![FileAnalysis {
                path: "src/a.rs".into(),
                exports: vec![export("foo")],
                ..Default::default()
            }],
            vec![],
        );
        let resp = compute(None, &current, "epoch");
        assert_eq!(resp.status, "ok");
        assert_eq!(resp.files_added, vec!["src/a.rs"]);
        assert!(resp.files_removed.is_empty());
        assert_eq!(resp.symbols_added.data.len(), 1);
        assert!(resp.since_marker.is_some());
    }

    #[test]
    fn detects_added_and_removed_files() {
        let prev = snapshot_with(
            vec![FileAnalysis {
                path: "src/old.rs".into(),
                exports: vec![export("gone")],
                ..Default::default()
            }],
            vec![],
        );
        let current = snapshot_with(
            vec![FileAnalysis {
                path: "src/new.rs".into(),
                exports: vec![export("fresh")],
                ..Default::default()
            }],
            vec![],
        );
        let resp = compute(Some(&prev), &current, "lastScan");
        assert_eq!(resp.files_added, vec!["src/new.rs"]);
        assert_eq!(resp.files_removed, vec!["src/old.rs"]);
        assert_eq!(resp.symbols_added.data.len(), 1);
        assert_eq!(resp.symbols_removed.data.len(), 1);
    }

    #[test]
    fn detects_changed_files_via_export_drift() {
        let prev = snapshot_with(
            vec![FileAnalysis {
                path: "src/util.rs".into(),
                exports: vec![export("foo")],
                ..Default::default()
            }],
            vec![],
        );
        let current = snapshot_with(
            vec![FileAnalysis {
                path: "src/util.rs".into(),
                exports: vec![export("foo"), export("bar")],
                ..Default::default()
            }],
            vec![],
        );
        let resp = compute(Some(&prev), &current, "lastQuery");
        assert!(resp.files_added.is_empty());
        assert!(resp.files_removed.is_empty());
        assert_eq!(resp.files_changed, vec!["src/util.rs"]);
        assert_eq!(resp.symbols_added.data.len(), 1);
        assert_eq!(resp.symbols_added.data[0].name, "bar");
    }

    #[test]
    fn detects_added_and_removed_edges() {
        let prev = snapshot_with(
            vec![],
            vec![GraphEdge {
                from: "a.rs".into(),
                to: "b.rs".into(),
                label: "foo".into(),
            }],
        );
        let current = snapshot_with(
            vec![],
            vec![GraphEdge {
                from: "c.rs".into(),
                to: "d.rs".into(),
                label: "bar".into(),
            }],
        );
        let resp = compute(Some(&prev), &current, "lastScan");
        assert_eq!(resp.edges_added.data.len(), 1);
        assert_eq!(resp.edges_removed.data.len(), 1);
        assert_eq!(resp.edges_added.data[0].from, "c.rs");
        assert_eq!(resp.edges_removed.data[0].from, "a.rs");
    }

    #[test]
    fn unsupported_since_carries_hint() {
        let resp = unsupported_since("HEAD~1");
        assert_eq!(resp.status, "unsupported_since");
        assert!(resp.hint.is_some());
        assert_eq!(resp.since, "HEAD~1");
    }

    #[test]
    fn diff_session_advance_updates_last_query() {
        let mut session = DiffSession::default();
        let snapshot = Arc::new(snapshot_with(vec![], vec![]));
        session.advance(snapshot.clone());
        assert!(session.last_query().is_some());
    }
}
