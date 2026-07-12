//! Integration tests for Plan 17 v2 — INCREMENTAL textDocument sync via
//! tree-sitter `InputEdit` translation.
//!
//! These tests drive [`LiveAstStore::apply_change`] (the LSP backend's
//! `did_change` handler) directly with synthetic
//! `TextDocumentContentChangeEvent`s. That path is the single concentration
//! point of the v2 work: position→byte translation, edit accumulation, and
//! tree-sitter incremental reparse. The tower-lsp integration test harness
//! cannot easily round-trip notifications without spinning a real stdio
//! loop, so we exercise the same in-process surface the backend calls,
//! identical to the pattern used by `tests/ast_query.rs`.
//!
//! Coverage:
//!   1. Function rename — single-event in-line edit; verifies edit
//!      translation + parse_duration_ms freshness + tree validity.
//!   2. Multi-line insert — multi-event transaction; verifies edit
//!      composition over an accumulated content state.
//!   3. Range deletion — verifies old_end_byte > start_byte handling and
//!      truncated content propagation.
//!   4. Range-less full-text replacement — LSP spec fall-through; verifies
//!      the daemon still emits a coherent payload.
//!   5. 100-edit benchmark — total wall time gate (<100ms) plus a parse
//!      duration histogram so reports can carry p50/p99.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::path::Path;
use std::time::Instant;

use loctree_lsp::live_ast::LiveAstStore;
use tempfile::TempDir;
use tower_lsp::lsp_types::{Position, Range, TextDocumentContentChangeEvent, Url};

const SEED_TS: &str =
    "export function greet(name: string): string {\n    return `hello, ${name}`;\n}\n";

fn fixture_uri(dir: &TempDir, name: &str) -> Url {
    Url::from_file_path(dir.path().join(name)).expect("file URL")
}

fn open_seed(store: &LiveAstStore, uri: &Url, source: &str) {
    let payload = store.update(uri, 1, source).expect("seed parse");
    assert!(!payload.has_error, "seed must parse cleanly: {payload:?}");
}

fn change_replace(
    line: u32,
    start_char: u32,
    end_char: u32,
    text: &str,
) -> TextDocumentContentChangeEvent {
    TextDocumentContentChangeEvent {
        range: Some(Range {
            start: Position {
                line,
                character: start_char,
            },
            end: Position {
                line,
                character: end_char,
            },
        }),
        range_length: None,
        text: text.to_string(),
    }
}

fn change_full(text: &str) -> TextDocumentContentChangeEvent {
    TextDocumentContentChangeEvent {
        range: None,
        range_length: None,
        text: text.to_string(),
    }
}

/// (1) Function rename — a single-character edit at a known offset.
/// Verifies that:
///   - The InputEdit translation produces a well-formed tree.
///   - `parse_duration_ms` stays bounded (sub-5ms on this fixture).
///   - The post-edit content reflects the rename.
#[test]
fn function_rename_emits_incremental_payload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let uri = fixture_uri(&dir, "rename.ts");
    let store = LiveAstStore::new();
    open_seed(&store, &uri, SEED_TS);

    // Replace `greet` (chars 16..21 on line 0) with `welcome`.
    let event = change_replace(0, 16, 21, "welcome");
    let payload = store
        .apply_change(&uri, 2, std::slice::from_ref(&event))
        .expect("incremental apply");

    assert_eq!(payload.version, 2);
    assert_eq!(payload.lang, "typescript");
    assert!(!payload.has_error, "rename must keep tree valid");
    assert_eq!(payload.root_kind, "program");
    assert!(
        payload.parse_duration_ms < 5.0,
        "incremental parse must stay under 5ms; got {} ms",
        payload.parse_duration_ms
    );

    let doc = store.get(&uri).expect("document after rename");
    assert!(
        doc.content.contains("welcome("),
        "post-edit content must carry the new identifier; got: {}",
        doc.content
    );
    assert!(
        !doc.content.contains("greet("),
        "old identifier must be replaced; got: {}",
        doc.content
    );
}

/// (2) Multi-line insert composed over multiple events. The second event
/// must reason against the *post-event-1* content, so this exercises
/// `translate_change_events` accumulation.
#[test]
fn multi_event_transaction_composes_edits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let uri = fixture_uri(&dir, "multi.ts");
    let store = LiveAstStore::new();
    open_seed(&store, &uri, SEED_TS);

    // Event 1: insert a new line above the function body's return.
    // Event 2: append an import line at the very top.
    let line_insert = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: Position {
                line: 1,
                character: 0,
            },
            end: Position {
                line: 1,
                character: 0,
            },
        }),
        range_length: None,
        text: "    const trimmed = name.trim();\n".to_string(),
    };
    let import_insert = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        }),
        range_length: None,
        text: "import { z } from 'zod';\n".to_string(),
    };

    let payload = store
        .apply_change(&uri, 3, &[line_insert, import_insert])
        .expect("multi-event apply");

    assert!(!payload.has_error, "multi-event must keep tree valid");
    assert_eq!(payload.root_kind, "program");

    let doc = store.get(&uri).expect("document after multi-event");
    assert!(doc.content.starts_with("import { z } from 'zod';\n"));
    assert!(doc.content.contains("const trimmed = name.trim();"));
    assert!(doc.content.contains("export function greet"));
}

/// (3) Range deletion. Verifies that `old_end_byte > start_byte` cases
/// produce a tree with the deleted range removed.
#[test]
fn range_deletion_shrinks_content() {
    let dir = tempfile::tempdir().expect("tempdir");
    let uri = fixture_uri(&dir, "delete.ts");
    let store = LiveAstStore::new();
    open_seed(&store, &uri, SEED_TS);

    // Delete `: string` — that's the return-type annotation on line 0.
    // Layout: `export function greet(name: string): string {` — the
    // return-type colon starts at byte 35 and runs through 43.
    let prev = store.get(&uri).expect("seed doc");
    let prev_len = prev.content.len();
    assert!(
        &prev.content[35..43] == ": string",
        "fixture layout drifted: expected ': string' at 35..43, got: {:?}",
        &prev.content[35..43]
    );

    let event = change_replace(0, 35, 43, "");
    let payload = store
        .apply_change(&uri, 2, std::slice::from_ref(&event))
        .expect("delete apply");

    assert!(!payload.has_error);
    let doc = store.get(&uri).expect("document after delete");
    assert!(
        doc.content.len() < prev_len,
        "delete must shrink content; before={} after={}",
        prev_len,
        doc.content.len()
    );
    // After deleting the return type annotation the parameter `: string`
    // remains; the function signature now ends with `) {` — that is the
    // observable post-edit state we assert on.
    assert!(
        doc.content.contains("function greet(name: string) {"),
        "return-type deletion must leave the parameter annotation intact; got: {}",
        doc.content
    );
}

/// (4) Range-less event — LSP spec fall-through. Tree-sitter still gets
/// a fresh tree but the store routes through `update`.
#[test]
fn range_less_event_falls_back_to_full_parse() {
    let dir = tempfile::tempdir().expect("tempdir");
    let uri = fixture_uri(&dir, "fallback.ts");
    let store = LiveAstStore::new();
    open_seed(&store, &uri, SEED_TS);

    let replacement = "export const answer = 42;\n";
    let event = change_full(replacement);
    let payload = store
        .apply_change(&uri, 2, std::slice::from_ref(&event))
        .expect("full-replace apply");

    assert_eq!(payload.version, 2);
    assert!(!payload.has_error);
    let doc = store.get(&uri).expect("document after replace");
    assert_eq!(doc.content, replacement);
}

/// (5) 100-edit benchmark + histogram. Each iteration appends a single
/// character at end-of-document; the test asserts a total wall-time gate
/// of <100ms and prints a p50/p99 line so the report can carry the
/// histogram without re-running the bench.
#[test]
fn hundred_edits_complete_under_hundred_ms() {
    let dir = tempfile::tempdir().expect("tempdir");
    let uri = fixture_uri(&dir, "bench.ts");
    let store = LiveAstStore::new();
    open_seed(&store, &uri, SEED_TS);

    // Each edit appends a single space at end-of-document. We compute
    // the post-edit position by reading the live document's content and
    // splitting it into lines.
    let mut samples: Vec<f64> = Vec::with_capacity(100);
    let total_started = Instant::now();
    for i in 0i32..100 {
        let doc = store.get(&uri).expect("doc per iter");
        let content = doc.content.clone();
        // Compute end-of-document position in (line, character) UTF-16
        // units. We use the raw line iterator since the seed contains
        // only ASCII so UTF-16 == bytes.
        let mut line_count: u32 = 0;
        let mut last_line_chars: u32 = 0;
        for (idx, line) in content.split('\n').enumerate() {
            line_count = idx as u32;
            last_line_chars = line.chars().map(|c| c.len_utf16() as u32).sum();
        }

        let event = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: line_count,
                    character: last_line_chars,
                },
                end: Position {
                    line: line_count,
                    character: last_line_chars,
                },
            }),
            range_length: None,
            text: " ".to_string(),
        };
        let payload = store
            .apply_change(&uri, 2 + i, std::slice::from_ref(&event))
            .unwrap_or_else(|| panic!("iter {i} produced no payload"));
        samples.push(payload.parse_duration_ms);
    }
    let total_ms = total_started.elapsed().as_secs_f64() * 1000.0;

    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() * 99) / 100];
    let mean: f64 = samples.iter().sum::<f64>() / samples.len() as f64;
    let max = samples.last().copied().unwrap_or(0.0);

    eprintln!(
        "live_ast 100-edit bench: total={:.3}ms samples=100 p50={:.3}ms p99={:.3}ms max={:.3}ms mean={:.3}ms",
        total_ms, p50, p99, max, mean
    );

    assert!(
        total_ms < 100.0,
        "100 edits must complete under 100ms; total={total_ms:.3}ms (p50={p50:.3} p99={p99:.3})"
    );
    // Tree must still be valid after 100 edits — no error nodes.
    let final_doc = store.get(&uri).expect("final doc");
    assert!(
        !final_doc.tree.has_error(),
        "tree-sitter must keep producing a valid tree after 100 incremental edits"
    );
    assert_eq!(final_doc.version, 101);
}

/// (6) Parity check — the `documents` cache observed via `get_for_path`
/// must reflect the post-edit buffer (this is the contract `ast_query`
/// relies on so it sees what the editor is showing rather than disk).
#[test]
fn live_cache_observable_via_workspace_relative_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = dir.path();
    let src_dir = workspace.join("src");
    std::fs::create_dir_all(&src_dir).expect("mkdir src");
    let file = src_dir.join("live.ts");
    let uri = Url::from_file_path(&file).expect("file URL");

    let store = LiveAstStore::new();
    open_seed(&store, &uri, SEED_TS);

    let event = change_replace(0, 16, 21, "salute");
    store
        .apply_change(&uri, 2, std::slice::from_ref(&event))
        .expect("apply");

    let lookup = store
        .get_for_path(workspace as &Path, "src/live.ts")
        .expect("live doc by relative path");
    assert!(lookup.content.contains("salute("));
    assert!(!lookup.content.contains("greet("));
}
