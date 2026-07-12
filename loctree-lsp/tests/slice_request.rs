//! Integration test for the `loctree/slice` custom LSP request (Plan 05).
//!
//! Covers: params deserialization, `HolographicSlice` → `SliceResponse`
//! mapping, response serialization shape (paths-only contract — no
//! inline file content).
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use loctree::slicer::{HolographicSlice, SliceFile, SliceStats};
use loctree_lsp::{CursorState, ResponseIdentity, SliceParams, SliceResponse};

fn slice_file(
    path: impl Into<String>,
    layer: impl Into<String>,
    loc: usize,
    depth: usize,
) -> SliceFile {
    SliceFile {
        path: path.into(),
        layer: layer.into(),
        loc,
        language: "rust".into(),
        kind: "code".into(),
        resource_kind: None,
        depth,
        ignored: false,
    }
}

fn fixture_slice() -> HolographicSlice {
    HolographicSlice {
        target: "src/lib.rs".into(),
        core: vec![slice_file("src/lib.rs", "core", 42, 0)],
        deps: vec![
            slice_file("src/util.rs", "deps", 21, 1),
            slice_file("src/inner/mod.rs", "deps", 7, 2),
        ],
        consumers: vec![slice_file("tests/it.rs", "consumers", 13, 1)],
        symbol_consumers: vec![],
        core_symbols: vec![],
        authority_labels: vec![],
        suggested_next: vec![],
        command_bridges: vec![],
        event_bridges: vec![],
        stats: SliceStats {
            core_files: 1,
            core_loc: 42,
            deps_files: 2,
            deps_loc: 28,
            consumers_files: 1,
            consumers_loc: 13,
            total_files: 4,
            total_loc: 83,
        },
    }
}

#[test]
fn params_deserialize_minimal() {
    let json = serde_json::json!({ "target": "src/lib.rs" });
    let params: SliceParams = serde_json::from_value(json).expect("minimal params parse");
    assert_eq!(params.target.to_string_lossy(), "src/lib.rs");
    assert!(!params.consumers, "consumers defaults to false");
    assert!(params.depth.is_none(), "depth defaults to None");
    assert!(params.project.is_none(), "project defaults to None");
}

#[test]
fn params_deserialize_full() {
    let json = serde_json::json!({
        "target": "src/lib.rs",
        "consumers": true,
        "depth": 3,
        "project": "/abs/repo",
        "cursor": "opaque-token",
        "chunk_size": 30
    });
    let params: SliceParams = serde_json::from_value(json).expect("full params parse");
    assert_eq!(params.target.to_string_lossy(), "src/lib.rs");
    assert!(params.consumers);
    assert_eq!(params.depth, Some(3));
    assert_eq!(
        params
            .project
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        Some("/abs/repo".into())
    );
    assert_eq!(params.cursor.as_deref(), Some("opaque-token"));
    assert_eq!(params.chunk_size, Some(30));
}

#[test]
fn response_maps_holographic_slice() {
    let slice = fixture_slice();
    let response = SliceResponse::from_holographic(&slice);

    assert_eq!(response.core.len(), 1);
    assert_eq!(response.core[0].path, "src/lib.rs");
    assert_eq!(response.core[0].depth, 0);
    assert_eq!(response.core[0].lang, "rust");
    assert_eq!(response.core[0].loc, 42);

    assert_eq!(response.deps.data.len(), 2);
    assert_eq!(response.deps.data[0].path, "src/util.rs");
    assert_eq!(response.deps.data[1].path, "src/inner/mod.rs");
    assert_eq!(response.deps.data[1].depth, 2);
    assert!(response.deps.next_cursor.is_none());

    assert_eq!(response.consumers.data.len(), 1);
    assert_eq!(response.consumers.data[0].path, "tests/it.rs");
    assert!(response.consumers.next_cursor.is_none());

    assert_eq!(response.total_files, 4);
    assert_eq!(response.total_loc, 83);
}

#[test]
fn response_serializes_to_pointer_only_shape() {
    let slice = fixture_slice();
    let response = SliceResponse::from_holographic(&slice);
    let json = serde_json::to_value(&response).expect("response serializes");

    // Top-level shape: core, deps, consumers, total_files, total_loc.
    let obj = json.as_object().expect("response is a JSON object");
    let mut keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
    keys.sort();
    assert_eq!(
        keys,
        ["consumers", "core", "deps", "total_files", "total_loc"]
    );

    // No inline content key sneaks in (paths-only contract).
    let entries = obj["core"]
        .as_array()
        .expect("core is array")
        .iter()
        .chain(obj["deps"]["data"].as_array().expect("deps data is array"))
        .chain(
            obj["consumers"]["data"]
                .as_array()
                .expect("consumers data is array"),
        );
    for entry in entries {
        let entry_obj = entry.as_object().expect("entry is object");
        let mut entry_keys: Vec<&str> = entry_obj.keys().map(|s| s.as_str()).collect();
        entry_keys.sort();
        assert_eq!(
            entry_keys,
            ["depth", "lang", "loc", "path"],
            "entry must expose only the paths-only contract fields, got: {entry_keys:?}"
        );
        assert!(!entry_obj.contains_key("content"));
        assert!(!entry_obj.contains_key("body"));
    }
}

#[test]
fn response_serializes_identity_when_attached() {
    let slice = fixture_slice();
    let response = SliceResponse::from_holographic(&slice).with_identity(ResponseIdentity {
        requested_project: Some("/requested/repo".into()),
        resolved_project: "/resolved/repo".into(),
        branch: Some("main".into()),
        commit: Some("abc1234".into()),
        snapshot_id: "main@abc1234".into(),
        scan_id: Some("main@abc1234".into()),
        repo: Some("loctree-suite".into()),
        owner_repo: Some("Loctree/loctree-suite".into()),
    });
    let json = serde_json::to_value(&response).expect("response serializes");
    let identity = json
        .get("identity")
        .and_then(|value| value.as_object())
        .expect("identity is attached");

    assert_eq!(identity["requested_project"], "/requested/repo");
    assert_eq!(identity["resolved_project"], "/resolved/repo");
    assert_eq!(identity["branch"], "main");
    assert_eq!(identity["commit"], "abc1234");
    assert_eq!(identity["snapshot_id"], "main@abc1234");
    assert_eq!(identity["scan_id"], "main@abc1234");
    assert_eq!(identity["repo"], "loctree-suite");
    assert_eq!(identity["owner_repo"], "Loctree/loctree-suite");
}

#[test]
fn empty_slice_yields_zero_totals() {
    let slice = HolographicSlice {
        target: "src/empty.rs".into(),
        core: vec![],
        deps: vec![],
        consumers: vec![],
        symbol_consumers: vec![],
        core_symbols: vec![],
        authority_labels: vec![],
        suggested_next: vec![],
        command_bridges: vec![],
        event_bridges: vec![],
        stats: SliceStats {
            core_files: 0,
            core_loc: 0,
            deps_files: 0,
            deps_loc: 0,
            consumers_files: 0,
            consumers_loc: 0,
            total_files: 0,
            total_loc: 0,
        },
    };
    let response = SliceResponse::from_holographic(&slice);
    assert!(response.core.is_empty());
    assert!(response.deps.data.is_empty());
    assert!(response.consumers.data.is_empty());
    assert!(response.deps.next_cursor.is_none());
    assert!(response.consumers.next_cursor.is_none());
    assert_eq!(response.total_files, 0);
    assert_eq!(response.total_loc, 0);
}

#[test]
fn large_consumer_layer_round_trips_through_cursor_pages() {
    let mut slice = fixture_slice();
    slice.deps.clear();
    slice.consumers = (0..80)
        .map(|i| slice_file(format!("tests/consumer_{i:02}.rs"), "consumers", 3, 1))
        .collect();
    slice.stats.deps_files = 0;
    slice.stats.deps_loc = 0;
    slice.stats.consumers_files = 80;
    slice.stats.consumers_loc = 240;
    slice.stats.total_files = 81;
    slice.stats.total_loc = 282;

    let snapshot_id = "main@slice-pagination";
    let first = SliceResponse::from_holographic_paginated(&slice, None, 30, snapshot_id)
        .expect("first consumer page");
    assert_eq!(first.consumers.chunk, 0);
    assert_eq!(first.consumers.total_chunks, 3);
    assert_eq!(first.consumers.data.len(), 30);
    assert!(first.deps.data.is_empty());
    assert!(first.deps.next_cursor.is_none());

    let first_cursor = first
        .consumers
        .next_cursor
        .as_deref()
        .expect("80 consumers should emit a cursor");
    assert_url_safe_cursor(first_cursor);
    let decoded = CursorState::decode(first_cursor, snapshot_id, "loctree/slice.consumers")
        .expect("consumer cursor decodes");
    assert_eq!(decoded.offset, 30);

    let second =
        SliceResponse::from_holographic_paginated(&slice, Some(first_cursor), 30, snapshot_id)
            .expect("second consumer page");
    assert_eq!(second.consumers.chunk, 1);
    assert_eq!(second.consumers.data[0].path, "tests/consumer_30.rs");
    assert_eq!(second.consumers.data.len(), 30);

    let second_cursor = second
        .consumers
        .next_cursor
        .as_deref()
        .expect("second page should emit final cursor");
    assert_url_safe_cursor(second_cursor);
    let final_page =
        SliceResponse::from_holographic_paginated(&slice, Some(second_cursor), 30, snapshot_id)
            .expect("final consumer page");
    assert_eq!(final_page.consumers.chunk, 2);
    assert_eq!(final_page.consumers.data.len(), 20);
    assert_eq!(final_page.consumers.data[19].path, "tests/consumer_79.rs");
    assert!(final_page.consumers.next_cursor.is_none());

    let all_paths: Vec<_> = first
        .consumers
        .data
        .into_iter()
        .chain(second.consumers.data)
        .chain(final_page.consumers.data)
        .map(|entry| entry.path)
        .collect();
    assert_eq!(all_paths.len(), 80);
    assert_eq!(all_paths[0], "tests/consumer_00.rs");
    assert_eq!(all_paths[79], "tests/consumer_79.rs");
}

#[test]
fn small_slice_layers_are_single_page_envelopes() {
    let response =
        SliceResponse::from_holographic_paginated(&fixture_slice(), None, 30, "snapshot")
            .expect("small fixture page");

    assert_eq!(response.deps.chunk, 0);
    assert_eq!(response.deps.total_chunks, 1);
    assert_eq!(response.deps.data.len(), 2);
    assert!(response.deps.next_cursor.is_none());
    assert_eq!(response.consumers.chunk, 0);
    assert_eq!(response.consumers.total_chunks, 1);
    assert_eq!(response.consumers.data.len(), 1);
    assert!(response.consumers.next_cursor.is_none());
}

fn assert_url_safe_cursor(token: &str) {
    assert!(!token.contains('+'), "cursor should be URL-safe: {token}");
    assert!(!token.contains('/'), "cursor should be URL-safe: {token}");
    assert!(!token.contains('='), "cursor should not be padded: {token}");
}
