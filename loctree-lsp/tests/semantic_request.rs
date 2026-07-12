//! Integration tests for Plan 14 — `loctree/semantic` request module.
//!
//! Drives the pure handlers in [`loctree_lsp::semantic`]: file-scope
//! delegation to `compose_runtime_slice`, kinds filter, the staged
//! `symbol_scope_unimplemented` response, and Plan 12 cursor pagination
//! for the project-scope path.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use loctree::snapshot::Snapshot;
use loctree::types::FileAnalysis;
use loctree_lsp::semantic::{
    SemanticData, compute_file_scope, compute_project_scope, paginate_project,
    snapshot_pagination_id, symbol_scope_response,
};

fn empty_snapshot() -> Snapshot {
    let mut s = Snapshot::new(vec![".".to_string()]);
    s.files = vec![FileAnalysis {
        path: "src/lib.rs".into(),
        ..Default::default()
    }];
    s
}

#[test]
fn file_scope_returns_runtime_slice_shape() {
    let snapshot = empty_snapshot();
    let data = compute_file_scope(&snapshot, "src/lib.rs", None, &None);
    // No semantic facts in the fixture → empty arrays. The shape
    // contract is what matters: we exercise the dispatch path and
    // confirm it does not panic on an unannotated snapshot.
    assert_eq!(data.idiom_tags.len(), 0);
    assert_eq!(data.dispatch_edges.len(), 0);
    assert_eq!(data.reachability.len(), 0);
}

#[test]
fn kinds_filter_drops_unrequested_buckets() {
    let snapshot = empty_snapshot();
    let data = compute_file_scope(
        &snapshot,
        "src/lib.rs",
        None,
        &Some(vec!["env_contracts".to_string()]),
    );
    // Even on an empty snapshot, the contract is "everything else
    // empty when the filter excludes it" — guarding against a future
    // change that would inflate the shape.
    assert_eq!(data.idiom_tags.len(), 0);
    assert_eq!(data.dispatch_edges.len(), 0);
    assert_eq!(data.reachability.len(), 0);
    assert_eq!(data.tauri_commands.len(), 0);
    assert_eq!(data.tauri_events.len(), 0);
    assert_eq!(data.framework_hints.len(), 0);
}

#[test]
fn project_scope_aggregates_across_files() {
    let mut snapshot = empty_snapshot();
    snapshot.files.push(FileAnalysis {
        path: "src/main.rs".into(),
        ..Default::default()
    });
    let data = compute_project_scope(&snapshot, None, &None);
    // No semantic facts in the fixture → still empty. The exercise is
    // that the aggregator visits every file without panicking.
    assert!(data.idiom_tags.is_empty());
}

#[test]
fn symbol_scope_returns_unimplemented_with_hint() {
    let resp = symbol_scope_response(Some("src/foo.rs::Foo".into()));
    assert_eq!(resp.status, "symbol_scope_unimplemented");
    assert_eq!(resp.scope, "symbol");
    assert!(resp.hint.is_some());
    assert!(resp.data.idiom_tags.is_empty());
}

#[test]
fn snapshot_pagination_id_falls_back_to_placeholder() {
    let snapshot = empty_snapshot();
    let id = snapshot_pagination_id(&snapshot);
    assert!(!id.is_empty());
}

#[test]
fn paginate_project_first_chunk_has_no_cursor_when_small() {
    let snapshot = empty_snapshot();
    let id = snapshot_pagination_id(&snapshot);
    let data = compute_project_scope(&snapshot, None, &None);
    let (paged, pagination) = paginate_project(data, None, 50, &id).expect("paginate");
    assert_eq!(pagination.chunk, 0);
    assert!(pagination.next_cursor.is_none());
    assert!(paged.dispatch_edges.is_empty());
}

#[test]
fn semantic_data_serializes_to_stable_keys() {
    let data = SemanticData::default();
    let value = serde_json::to_value(&data).unwrap();
    for key in [
        "idiom_tags",
        "dispatch_edges",
        "reachability",
        "env_contracts",
        "tauri_commands",
        "tauri_events",
        "framework_hints",
    ] {
        assert!(value.get(key).is_some(), "wire key `{key}` missing");
    }
}
