//! Integration tests for Plan 11 — `loctree/diff` request module.
//!
//! Drives the pure functions in [`loctree_lsp::diff`]: full-from-
//! current epoch baseline, file/edge/symbol delta detection, the
//! `unsupported_since` typed error, and `DiffSession` rotation.
//! End-to-end Backend tests live in the daemon smoke harness.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use loctree::snapshot::{GraphEdge, Snapshot};
use loctree::types::{ExportSymbol, FileAnalysis};
use loctree_lsp::cursor::CursorState;
use loctree_lsp::diff::{
    DiffSession, compute, compute_paginated, snapshot_marker, unsupported_since,
};
use std::sync::Arc;

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

fn snap(files: Vec<FileAnalysis>, edges: Vec<GraphEdge>) -> Snapshot {
    let mut s = Snapshot::new(vec![".".to_string()]);
    s.files = files;
    s.edges = edges;
    s
}

#[test]
fn epoch_baseline_returns_full_inventory_as_added() {
    let current = snap(
        vec![FileAnalysis {
            path: "src/a.rs".into(),
            exports: vec![export("foo")],
            ..Default::default()
        }],
        vec![],
    );
    let resp = compute(None, &current, "epoch");
    assert_eq!(resp.status, "ok");
    assert_eq!(resp.files_added.len(), 1);
    assert_eq!(resp.files_removed.len(), 0);
    assert_eq!(resp.symbols_added.data.len(), 1);
    assert!(resp.since_marker.is_some());
}

#[test]
fn file_added_and_removed_detected() {
    let prev = snap(
        vec![FileAnalysis {
            path: "src/old.rs".into(),
            exports: vec![export("gone")],
            ..Default::default()
        }],
        vec![],
    );
    let current = snap(
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
    assert_eq!(resp.symbols_added.data[0].name, "fresh");
    assert_eq!(resp.symbols_removed.data[0].name, "gone");
}

#[test]
fn changed_file_when_exports_drift() {
    let prev = snap(
        vec![FileAnalysis {
            path: "src/util.rs".into(),
            exports: vec![export("foo")],
            ..Default::default()
        }],
        vec![],
    );
    let current = snap(
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
fn edge_diff_added_and_removed() {
    let prev = snap(
        vec![],
        vec![GraphEdge {
            from: "a.rs".into(),
            to: "b.rs".into(),
            label: "foo".into(),
        }],
    );
    let current = snap(
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
    assert_eq!(resp.edges_added.data[0].to, "d.rs");
    assert_eq!(resp.edges_removed.data[0].from, "a.rs");
}

#[test]
fn unsupported_since_response_is_typed() {
    let resp = unsupported_since("HEAD~3");
    assert_eq!(resp.status, "unsupported_since");
    assert_eq!(resp.since, "HEAD~3");
    assert!(
        resp.hint.as_deref().unwrap_or("").contains("v1"),
        "hint should mention v1 limitation"
    );
    assert!(resp.since_marker.is_none());
}

#[test]
fn snapshot_marker_falls_back_when_metadata_missing() {
    let snapshot = snap(vec![], vec![]);
    let marker = snapshot_marker(&snapshot);
    assert!(marker.starts_with("snapshot:"));
}

#[test]
fn diff_session_advance_and_set_last_scan() {
    let mut session = DiffSession::default();
    assert!(session.last_scan().is_none());
    assert!(session.last_query().is_none());

    let snapshot = Arc::new(snap(vec![], vec![]));
    session.set_last_scan(snapshot.clone());
    assert!(session.last_scan().is_some());

    let later = Arc::new(snap(vec![], vec![]));
    session.advance(later);
    assert!(session.last_query().is_some());
    // Reset clears both markers.
    session.reset();
    assert!(session.last_scan().is_none());
    assert!(session.last_query().is_none());
}

#[test]
fn since_marker_round_trips_as_cached_snapshot_baseline() {
    let mut session = DiffSession::default();
    let initial = Arc::new(snap(
        vec![FileAnalysis {
            path: "src/a.rs".into(),
            exports: vec![export("existing")],
            ..Default::default()
        }],
        vec![],
    ));

    let first = compute(None, &initial, "epoch");
    let marker = first.since_marker.expect("ok diff emits since_marker");
    assert!(
        session.snapshot_for_marker(&marker).is_none(),
        "unadvanced sessions must not resolve marker by accident"
    );
    session.advance(initial);

    let current = snap(
        vec![
            FileAnalysis {
                path: "src/a.rs".into(),
                exports: vec![export("existing")],
                ..Default::default()
            },
            FileAnalysis {
                path: "src/added.rs".into(),
                exports: vec![export("fresh")],
                ..Default::default()
            },
        ],
        vec![],
    );
    let baseline = session
        .snapshot_for_marker(&marker)
        .expect("emitted snapshot marker resolves after session advance");
    let second = compute(Some(&baseline), &current, &marker);

    assert_eq!(second.status, "ok");
    assert_eq!(second.since, marker);
    assert_eq!(second.files_added, vec!["src/added.rs"]);
    assert!(second.files_removed.is_empty());
    assert_eq!(second.symbols_added.data.len(), 1);
    assert_eq!(second.symbols_added.data[0].name, "fresh");
}

#[test]
fn edge_delta_pages_round_trip_through_cursor() {
    let prev = snap(vec![], vec![]);
    let current = snap(
        vec![],
        (0..120)
            .map(|i| GraphEdge {
                from: format!("src/from_{i:03}.rs"),
                to: format!("src/to_{i:03}.rs"),
                label: "import".into(),
            })
            .collect(),
    );
    let snapshot_id = "main@diff-pagination";

    let first = compute_paginated(Some(&prev), &current, "lastScan", None, 50, snapshot_id)
        .expect("first edge page");
    assert_eq!(first.edges_added.chunk, 0);
    assert_eq!(first.edges_added.total_chunks, 3);
    assert_eq!(first.edges_added.data.len(), 50);
    assert!(first.edges_removed.next_cursor.is_none());
    assert!(first.symbols_added.next_cursor.is_none());
    assert!(first.symbols_removed.next_cursor.is_none());

    let first_cursor = first
        .edges_added
        .next_cursor
        .as_deref()
        .expect("120 edge deltas should emit a cursor");
    assert_url_safe_cursor(first_cursor);
    let decoded = CursorState::decode(first_cursor, snapshot_id, "loctree/diff.edges_added")
        .expect("edge cursor decodes");
    assert_eq!(decoded.offset, 50);

    let second = compute_paginated(
        Some(&prev),
        &current,
        "lastScan",
        Some(first_cursor),
        50,
        snapshot_id,
    )
    .expect("second edge page");
    assert_eq!(second.edges_added.chunk, 1);
    assert_eq!(second.edges_added.data[0].from, "src/from_050.rs");
    assert_eq!(second.edges_added.data.len(), 50);

    let second_cursor = second
        .edges_added
        .next_cursor
        .as_deref()
        .expect("second page should emit final cursor");
    assert_url_safe_cursor(second_cursor);

    let final_page = compute_paginated(
        Some(&prev),
        &current,
        "lastScan",
        Some(second_cursor),
        50,
        snapshot_id,
    )
    .expect("final edge page");
    assert_eq!(final_page.edges_added.chunk, 2);
    assert_eq!(final_page.edges_added.data.len(), 20);
    assert_eq!(final_page.edges_added.data[19].from, "src/from_119.rs");
    assert!(final_page.edges_added.next_cursor.is_none());

    let all_edges: Vec<_> = first
        .edges_added
        .data
        .into_iter()
        .chain(second.edges_added.data)
        .chain(final_page.edges_added.data)
        .map(|edge| edge.from)
        .collect();
    assert_eq!(all_edges.len(), 120);
    assert_eq!(all_edges[0], "src/from_000.rs");
    assert_eq!(all_edges[119], "src/from_119.rs");
}

#[test]
fn small_diff_delta_buckets_are_single_page_envelopes() {
    let current = snap(
        vec![FileAnalysis {
            path: "src/a.rs".into(),
            exports: vec![export("foo")],
            ..Default::default()
        }],
        vec![GraphEdge {
            from: "src/a.rs".into(),
            to: "src/b.rs".into(),
            label: "import".into(),
        }],
    );
    let response =
        compute_paginated(None, &current, "epoch", None, 30, "snapshot").expect("small diff page");

    assert_eq!(response.edges_added.chunk, 0);
    assert_eq!(response.edges_added.total_chunks, 1);
    assert!(response.edges_added.next_cursor.is_none());
    assert_eq!(response.symbols_added.chunk, 0);
    assert_eq!(response.symbols_added.total_chunks, 1);
    assert!(response.symbols_added.next_cursor.is_none());
}

fn assert_url_safe_cursor(token: &str) {
    assert!(!token.contains('+'), "cursor should be URL-safe: {token}");
    assert!(!token.contains('/'), "cursor should be URL-safe: {token}");
    assert!(!token.contains('='), "cursor should not be padded: {token}");
}

#[test]
fn epoch_response_serializes_to_stable_keys() {
    let current = snap(
        vec![FileAnalysis {
            path: "x.rs".into(),
            exports: vec![export("foo")],
            ..Default::default()
        }],
        vec![],
    );
    let resp = compute(None, &current, "epoch");
    let value = serde_json::to_value(&resp).unwrap();
    for key in [
        "status",
        "since",
        "files_added",
        "files_removed",
        "files_changed",
        "edges_added",
        "edges_removed",
        "symbols_added",
        "symbols_removed",
        "since_marker",
    ] {
        assert!(value.get(key).is_some(), "wire key `{key}` missing");
    }
}
