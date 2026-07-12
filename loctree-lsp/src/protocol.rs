//! Streaming + cursor pagination wire shapes for LSP responses (Plan 12).
//!
//! Heavy responses (slice on a hub file, find with hundreds of hits,
//! semantic facts on a 3000-file workspace) trip the host-truncation
//! ceiling on JSON-RPC. The Codex Manifest Protocol pattern:
//! return one chunk at a time with an opaque `next_cursor` token; the
//! client round-trips the token to fetch the next chunk.
//!
//! `Paginated<T>` is the wire envelope. `paginate` is the slicing
//! helper any handler can call when it has a `Vec<Item>` ready.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::path::Path;

use loctree::snapshot::Snapshot;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cursor::CursorState;

/// Default page size when init options don't override it.
pub const DEFAULT_CHUNK_SIZE: usize = 50;

/// Hard ceiling — even when an init option requests more, we cap to
/// keep individual responses inside the host JSON-RPC payload limit.
pub const MAX_CHUNK_SIZE: usize = 500;

/// Hard floor — a chunk size of 0 would never make progress.
pub const MIN_CHUNK_SIZE: usize = 1;

/// Wire envelope for paginated responses.
///
/// Single-shot responses (item count ≤ chunk_size) carry
/// `chunk = 0`, `total_chunks = 1`, and `next_cursor = None`.
/// Clients can therefore treat the same wire shape for both
/// fits-in-one and chunked cases.
#[derive(Debug, Clone, Serialize)]
pub struct Paginated<T> {
    pub chunk: u32,
    pub total_chunks: u32,
    /// Opaque token to pass back as `cursor` in the next request.
    /// `None` on the final chunk.
    pub next_cursor: Option<String>,
    pub data: T,
    /// Human-readable hint surfaced to clients (e.g. truncation
    /// notes). Skipped from JSON when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advisory: Option<String>,
}

/// Identity envelope attached to responses that depend on a routed snapshot.
///
/// This makes the requested project, resolved workspace, and snapshot/git
/// authority visible at the response boundary instead of requiring clients to
/// infer it from cache paths or cursor ids.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResponseIdentity {
    pub requested_project: Option<String>,
    pub resolved_project: String,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub snapshot_id: String,
    pub scan_id: Option<String>,
    pub repo: Option<String>,
    pub owner_repo: Option<String>,
}

impl ResponseIdentity {
    pub fn from_snapshot(
        requested_project: Option<&Path>,
        resolved_project: &Path,
        snapshot: &Snapshot,
        snapshot_id: impl Into<String>,
    ) -> Self {
        Self {
            requested_project: requested_project.map(|path| path.to_string_lossy().into_owned()),
            resolved_project: resolved_project.to_string_lossy().into_owned(),
            branch: snapshot.metadata.git_branch.clone(),
            commit: snapshot.metadata.git_commit.clone(),
            snapshot_id: snapshot_id.into(),
            scan_id: snapshot.metadata.git_scan_id.clone(),
            repo: snapshot.metadata.git_repo.clone(),
            owner_repo: snapshot.metadata.git_owner_repo.clone(),
        }
    }
}

/// Build a single page of `items` starting at `offset`.
///
/// The current chunk is returned alongside a freshly minted cursor
/// for the *next* chunk. When `offset + chunk_size >= items.len()`,
/// `next_cursor` is `None` (the page is the final one).
pub fn paginate<T: Clone>(
    items: &[T],
    offset: usize,
    chunk_size: usize,
    snapshot_id: &str,
    kind: &str,
) -> Result<Paginated<Vec<T>>, crate::cursor::CursorError> {
    let chunk_size = clamp_chunk_size(chunk_size);
    let total = items.len();
    let end = offset.saturating_add(chunk_size).min(total);
    let chunk_index = (offset / chunk_size) as u32;
    let total_chunks = if total == 0 {
        1
    } else {
        total.div_ceil(chunk_size) as u32
    };

    let data = if offset >= total {
        Vec::new()
    } else {
        items[offset..end].to_vec()
    };

    let next_cursor = if end >= total {
        None
    } else {
        let state = CursorState {
            snapshot_id: snapshot_id.to_string(),
            offset: end,
            kind: kind.to_string(),
        };
        Some(state.encode()?)
    };

    Ok(Paginated {
        chunk: chunk_index,
        total_chunks,
        next_cursor,
        data,
        advisory: None,
    })
}

/// Wrap a single response value as a one-chunk page (no pagination).
/// Useful for handlers that don't yet stream — keeps the wire shape
/// uniform across paginated and non-paginated endpoints.
pub fn single_page<T>(data: T) -> Paginated<T> {
    Paginated {
        chunk: 0,
        total_chunks: 1,
        next_cursor: None,
        data,
        advisory: None,
    }
}

/// Clamp a requested chunk size to the supported range.
pub fn clamp_chunk_size(requested: usize) -> usize {
    requested.clamp(MIN_CHUNK_SIZE, MAX_CHUNK_SIZE)
}

/// Read `loctree.protocol.defaultChunkSize` from `initializationOptions`.
/// Honors both nested (`{"loctree":{"protocol":{...}}}`) and flat
/// (`{"loctree.protocol.defaultChunkSize": 100}`) shapes for parity with
/// the watcher config (Plan 10).
pub fn chunk_size_from_options(options: Option<&Value>) -> usize {
    let Some(value) = options else {
        return DEFAULT_CHUNK_SIZE;
    };
    let nested = value
        .pointer("/loctree/protocol/defaultChunkSize")
        .and_then(|v| v.as_u64());
    let flat = value
        .get("loctree.protocol.defaultChunkSize")
        .and_then(|v| v.as_u64());
    nested
        .or(flat)
        .map(|n| clamp_chunk_size(n as usize))
        .unwrap_or(DEFAULT_CHUNK_SIZE)
}

/// Read the `codeLens` opt-in from `initializationOptions`.
///
/// Loctree code lenses are OFF by default: advertising them
/// unconditionally clutters the editor gutter alongside the real
/// language server's lenses (rust-analyzer / tsserver). The feature is
/// loctree-additive, not a masquerade, so it stays available — clients
/// opt in by passing `{"codeLens": true}`. Returns `true` only for that
/// exact boolean; absent options, `{}`, or `false` all yield `false`.
pub fn code_lens_from_options(options: Option<&Value>) -> bool {
    options
        .and_then(|value| value.get("codeLens"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ids(n: usize) -> Vec<u32> {
        (0..n as u32).collect()
    }

    #[test]
    fn fits_in_one_chunk_yields_no_cursor() {
        let items = ids(10);
        let page = paginate(&items, 0, 50, "snap@abc", "loctree/find").unwrap();
        assert_eq!(page.chunk, 0);
        assert_eq!(page.total_chunks, 1);
        assert!(page.next_cursor.is_none());
        assert_eq!(page.data, ids(10));
    }

    #[test]
    fn first_chunk_emits_cursor_for_next() {
        let items = ids(120);
        let page = paginate(&items, 0, 50, "snap@abc", "loctree/find").unwrap();
        assert_eq!(page.chunk, 0);
        assert_eq!(page.total_chunks, 3);
        assert_eq!(page.data.len(), 50);
        let cursor = page.next_cursor.expect("cursor present");
        let decoded = CursorState::decode(&cursor, "snap@abc", "loctree/find").unwrap();
        assert_eq!(decoded.offset, 50);
    }

    #[test]
    fn middle_chunk_advances_offset() {
        let items = ids(120);
        let page = paginate(&items, 50, 50, "snap@abc", "loctree/find").unwrap();
        assert_eq!(page.chunk, 1);
        assert_eq!(page.data.len(), 50);
        let next = page.next_cursor.expect("cursor present");
        let decoded = CursorState::decode(&next, "snap@abc", "loctree/find").unwrap();
        assert_eq!(decoded.offset, 100);
    }

    #[test]
    fn final_chunk_drops_cursor() {
        let items = ids(120);
        let page = paginate(&items, 100, 50, "snap@abc", "loctree/find").unwrap();
        assert_eq!(page.chunk, 2);
        assert_eq!(page.data.len(), 20);
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn empty_result_returns_empty_page() {
        let items: Vec<u32> = Vec::new();
        let page = paginate(&items, 0, 50, "snap@abc", "loctree/find").unwrap();
        assert!(page.data.is_empty());
        assert_eq!(page.total_chunks, 1);
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn offset_past_end_returns_empty_data() {
        let items = ids(10);
        let page = paginate(&items, 999, 50, "snap@abc", "loctree/find").unwrap();
        assert!(page.data.is_empty());
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn requested_chunk_size_is_clamped() {
        assert_eq!(clamp_chunk_size(0), MIN_CHUNK_SIZE);
        assert_eq!(clamp_chunk_size(1), MIN_CHUNK_SIZE);
        assert_eq!(clamp_chunk_size(50), 50);
        assert_eq!(clamp_chunk_size(usize::MAX), MAX_CHUNK_SIZE);
    }

    #[test]
    fn single_page_envelope_shape() {
        let resp = single_page(vec!["a", "b", "c"]);
        assert_eq!(resp.chunk, 0);
        assert_eq!(resp.total_chunks, 1);
        assert!(resp.next_cursor.is_none());
        assert_eq!(resp.data, vec!["a", "b", "c"]);
    }

    #[test]
    fn paginated_envelope_omits_advisory_when_none() {
        let page = paginate(&ids(5), 0, 50, "snap@abc", "loctree/find").unwrap();
        let json = serde_json::to_value(&page).unwrap();
        assert!(json.get("advisory").is_none());
        // Required fields are always present.
        assert!(json.get("chunk").is_some());
        assert!(json.get("total_chunks").is_some());
        assert!(json.get("data").is_some());
    }

    #[test]
    fn chunk_size_default_when_options_absent() {
        assert_eq!(chunk_size_from_options(None), DEFAULT_CHUNK_SIZE);
    }

    #[test]
    fn chunk_size_reads_nested_option() {
        let opts = json!({
            "loctree": { "protocol": { "defaultChunkSize": 100 } }
        });
        assert_eq!(chunk_size_from_options(Some(&opts)), 100);
    }

    #[test]
    fn chunk_size_reads_flat_option() {
        let opts = json!({ "loctree.protocol.defaultChunkSize": 25 });
        assert_eq!(chunk_size_from_options(Some(&opts)), 25);
    }

    #[test]
    fn chunk_size_clamps_overflow_value() {
        let opts = json!({ "loctree.protocol.defaultChunkSize": 100_000 });
        assert_eq!(chunk_size_from_options(Some(&opts)), MAX_CHUNK_SIZE);
    }

    #[test]
    fn chunk_size_falls_through_unrelated_options() {
        let opts = json!({ "other": "thing" });
        assert_eq!(chunk_size_from_options(Some(&opts)), DEFAULT_CHUNK_SIZE);
    }

    #[test]
    fn code_lens_default_when_options_absent() {
        assert!(!code_lens_from_options(None));
    }

    #[test]
    fn code_lens_default_when_options_empty() {
        let opts = json!({});
        assert!(!code_lens_from_options(Some(&opts)));
    }

    #[test]
    fn code_lens_opt_in_true() {
        let opts = json!({ "codeLens": true });
        assert!(code_lens_from_options(Some(&opts)));
    }

    #[test]
    fn code_lens_opt_in_false() {
        let opts = json!({ "codeLens": false });
        assert!(!code_lens_from_options(Some(&opts)));
    }
}
