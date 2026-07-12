//! Integration test for the cursor-pagination wire shape (Plan 12).
//!
//! Walks a synthetic result set through first / middle / final
//! chunks, then verifies the snapshot-drift error path (cursor was
//! issued for an old snapshot, current one is different).
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use loctree_lsp::{CursorState, DEFAULT_CHUNK_SIZE, MAX_CHUNK_SIZE, paginate, single_page};

const KIND: &str = "loctree/find";
const SNAPSHOT_A: &str = "release_1.5.0@7a64f2cb";
const SNAPSHOT_B: &str = "release_1.5.0@9a7ae9d3";

fn ids(n: usize) -> Vec<u32> {
    (0..n as u32).collect()
}

#[test]
fn full_walk_first_to_final() {
    let items = ids(120);

    // First chunk: offset 0, items 0..50.
    let p1 = paginate(&items, 0, 50, SNAPSHOT_A, KIND).unwrap();
    assert_eq!(p1.chunk, 0);
    assert_eq!(p1.total_chunks, 3);
    assert_eq!(p1.data.len(), 50);
    let cursor1 = p1.next_cursor.expect("cursor on first chunk");

    // Decode + use cursor for second chunk.
    let s1 = CursorState::decode(&cursor1, SNAPSHOT_A, KIND).unwrap();
    assert_eq!(s1.offset, 50);
    let p2 = paginate(&items, s1.offset, 50, SNAPSHOT_A, KIND).unwrap();
    assert_eq!(p2.chunk, 1);
    assert_eq!(p2.data.len(), 50);
    let cursor2 = p2.next_cursor.expect("cursor on middle chunk");

    // Decode + use cursor for final chunk.
    let s2 = CursorState::decode(&cursor2, SNAPSHOT_A, KIND).unwrap();
    assert_eq!(s2.offset, 100);
    let p3 = paginate(&items, s2.offset, 50, SNAPSHOT_A, KIND).unwrap();
    assert_eq!(p3.chunk, 2);
    assert_eq!(p3.total_chunks, 3);
    assert_eq!(p3.data.len(), 20);
    assert!(
        p3.next_cursor.is_none(),
        "final chunk must drop next_cursor"
    );

    // Concatenated chunks reconstruct the original.
    let mut walked: Vec<u32> = Vec::new();
    walked.extend(p1.data);
    walked.extend(p2.data);
    walked.extend(p3.data);
    assert_eq!(walked, items);
}

#[test]
fn snapshot_drift_mid_pagination_is_detected() {
    let items = ids(120);
    let p1 = paginate(&items, 0, 50, SNAPSHOT_A, KIND).unwrap();
    let cursor = p1.next_cursor.expect("cursor on first chunk");

    // Snapshot churned to B in the meantime — decode against B fails.
    let err = CursorState::decode(&cursor, SNAPSHOT_B, KIND).unwrap_err();
    assert_eq!(err.code(), "snapshot_drifted");
    assert!(err.retry(), "client must retry from offset 0");
}

#[test]
fn cursor_kind_mismatch_is_rejected() {
    let items = ids(120);
    let p1 = paginate(&items, 0, 50, SNAPSHOT_A, KIND).unwrap();
    let cursor = p1.next_cursor.unwrap();

    let err = CursorState::decode(&cursor, SNAPSHOT_A, "loctree/slice").unwrap_err();
    assert_eq!(err.code(), "cursor_kind_mismatch");
    assert!(!err.retry(), "kind mismatch is a programming bug, no retry");
}

#[test]
fn small_result_returns_single_page() {
    let items = ids(5);
    let page = paginate(&items, 0, DEFAULT_CHUNK_SIZE, SNAPSHOT_A, KIND).unwrap();
    assert_eq!(page.chunk, 0);
    assert_eq!(page.total_chunks, 1);
    assert!(page.next_cursor.is_none());
    assert_eq!(page.data, items);
}

#[test]
fn single_page_helper_matches_paginate_for_fits_in_one() {
    let items = ids(3);
    let helper = single_page(items.clone());
    let pag = paginate(&items, 0, DEFAULT_CHUNK_SIZE, SNAPSHOT_A, KIND).unwrap();

    assert_eq!(helper.chunk, pag.chunk);
    assert_eq!(helper.total_chunks, pag.total_chunks);
    assert_eq!(helper.next_cursor, pag.next_cursor);
    assert_eq!(helper.data, pag.data);
}

#[test]
fn chunk_size_clamps_oversized_request() {
    // 100k requested → MAX_CHUNK_SIZE applied.
    let items = ids(MAX_CHUNK_SIZE + 200);
    let page = paginate(&items, 0, 100_000, SNAPSHOT_A, KIND).unwrap();
    assert_eq!(page.data.len(), MAX_CHUNK_SIZE);
    assert!(page.next_cursor.is_some(), "should still need another page");
}

#[test]
fn cursor_token_is_url_safe_no_pad() {
    let items = ids(120);
    let page = paginate(&items, 0, 50, SNAPSHOT_A, KIND).unwrap();
    let token = page.next_cursor.unwrap();
    assert!(!token.contains('+'));
    assert!(!token.contains('/'));
    assert!(!token.contains('='));
}

#[test]
fn empty_result_yields_one_chunk_with_no_data() {
    let items: Vec<u32> = Vec::new();
    let page = paginate(&items, 0, 50, SNAPSHOT_A, KIND).unwrap();
    assert!(page.data.is_empty());
    assert_eq!(page.total_chunks, 1);
    assert!(page.next_cursor.is_none());
}
