//! Integration tests for Plan 18 v2 — `loctree/symbolChanged`
//! notification on live edits.
//!
//! These drive [`LiveAstStore::apply_change`] +
//! [`extract_live_symbols`] + [`diff_symbol_sets`] directly so we
//! exercise the same in-process surface the LSP backend's `did_change`
//! path calls. The tower-lsp integration test harness can't easily
//! round-trip notifications, so we stick to the same pattern Plan 17's
//! `tests/live_ast.rs` uses.
//!
//! Coverage:
//!   1. Function rename — single-event in-line edit; assert one
//!      `rewritten` change for the renamed symbol AND zero events for
//!      unaffected siblings.
//!   2. Function added — assert `added`, no other changes.
//!   3. Function removed — assert `removed`, no other changes.
//!   4. Function moved — same name, body shifted by an inserted line;
//!      assert `moved` with from/to byte ranges populated.
//!   5. Capability JSON flip — `loctree/symbolChanged.available: true`
//!      with the four kinds advertised.
//!   6. Class rename — verifies non-function kinds still classify.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::collections::HashMap;

use loctree::types::SymbolIdV1;
use loctree_lsp::live_ast::{
    self, LiveAstStore, SymbolChangeKind, SymbolMetadata, build_symbol_map, diff_symbol_sets,
    extract_live_symbols, symbol_changed_capability_json,
};
use tempfile::TempDir;
use tower_lsp::lsp_types::{Position, Range, TextDocumentContentChangeEvent, Url};

/// Two siblings — one we'll edit, one we leave alone. The fixture is
/// deliberately small so byte ranges are predictable.
const SEED_TS: &str = "\
export function greet(name: string): string {
    return `hello, ${name}`;
}

export function farewell(name: string): string {
    return `bye, ${name}`;
}
";

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

/// Run a one-shot extraction + diff cycle. Returns the change list plus
/// the next-baseline metadata so callers can chain a second edit.
fn extract_and_diff(
    store: &LiveAstStore,
    uri: &Url,
    file_path: &str,
    prev: Option<&HashMap<SymbolIdV1, SymbolMetadata>>,
) -> (
    Vec<live_ast::SymbolChange>,
    HashMap<SymbolIdV1, SymbolMetadata>,
) {
    let doc = store.get(uri).expect("live doc after parse");
    let symbols = extract_live_symbols(&doc.tree);
    let current = build_symbol_map(file_path, &symbols);
    diff_symbol_sets(file_path, prev, &current)
}

/// (1) Function rename — assert one `rewritten` event for the renamed
/// symbol AND zero events for the unchanged sibling.
#[test]
fn rename_function_emits_single_rewritten_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let uri = fixture_uri(&dir, "rename.ts");
    let store = LiveAstStore::new();
    open_seed(&store, &uri, SEED_TS);

    // Seed extraction — every symbol is `added`.
    let (initial, baseline) = extract_and_diff(&store, &uri, "rename.ts", None);
    assert_eq!(initial.len(), 2, "seed must emit two `added` changes");
    assert!(
        initial.iter().all(|c| c.kind == SymbolChangeKind::Added),
        "seed changes must all be `added`: {initial:?}"
    );

    // Replace `greet` with `welcome` — chars 16..21 on line 0.
    let event = change_replace(0, 16, 21, "welcome");
    let payload = store
        .apply_change(&uri, 2, std::slice::from_ref(&event))
        .expect("incremental apply");
    assert!(!payload.has_error, "rename must keep tree valid");

    let (changes, _next) = extract_and_diff(&store, &uri, "rename.ts", Some(&baseline));
    assert_eq!(
        changes.len(),
        1,
        "exactly one symbol change expected for a rename, got {changes:?}"
    );
    assert_eq!(changes[0].kind, SymbolChangeKind::Rewritten);
    assert_eq!(
        changes[0].id.as_str(),
        "rename.ts::welcome",
        "the new id must point at the renamed symbol"
    );
    assert!(
        changes[0].from.is_some() && changes[0].to.is_some(),
        "rewritten changes must populate both from and to: {:?}",
        changes[0]
    );

    // The sibling `farewell` must NOT appear in the change list.
    assert!(
        !changes.iter().any(|c| c.id.as_str().ends_with("farewell")),
        "sibling `farewell` must not appear in changes: {changes:?}"
    );
}

/// (2) Function added — append a new export at end of file. Assert
/// exactly one `added` change for the new symbol and nothing else.
#[test]
fn append_function_emits_added_for_new_symbol_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let uri = fixture_uri(&dir, "add.ts");
    let store = LiveAstStore::new();
    open_seed(&store, &uri, SEED_TS);
    let (_seed, baseline) = extract_and_diff(&store, &uri, "add.ts", None);

    // Append a new function at end-of-document.
    let doc = store.get(&uri).expect("doc");
    let line_count = doc.content.matches('\n').count() as u32;
    let event = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: Position {
                line: line_count,
                character: 0,
            },
            end: Position {
                line: line_count,
                character: 0,
            },
        }),
        range_length: None,
        text: "\nexport function bonus(): number { return 0; }\n".to_string(),
    };
    store
        .apply_change(&uri, 2, std::slice::from_ref(&event))
        .expect("apply");

    let (changes, _) = extract_and_diff(&store, &uri, "add.ts", Some(&baseline));
    assert_eq!(changes.len(), 1, "expected single change, got {changes:?}");
    assert_eq!(changes[0].kind, SymbolChangeKind::Added);
    assert_eq!(changes[0].id.as_str(), "add.ts::bonus");
    assert!(changes[0].from.is_none());
    assert!(changes[0].to.is_some());
}

/// (3) Function removed — delete `farewell`'s declaration entirely.
/// Assert exactly one `removed` change.
#[test]
fn delete_function_emits_removed_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let uri = fixture_uri(&dir, "remove.ts");
    let store = LiveAstStore::new();
    open_seed(&store, &uri, SEED_TS);
    let (_seed, baseline) = extract_and_diff(&store, &uri, "remove.ts", None);

    // Locate `export function farewell` start in the seed and delete
    // through end-of-file. We compute it from the live content so the
    // test stays robust if the seed grows trailing whitespace.
    let doc = store.get(&uri).expect("doc");
    let content = doc.content.clone();
    let farewell_start = content.find("export function farewell").expect("farewell");
    // Delete from the start of `farewell` to end-of-file.
    // Convert byte offsets back to (line, char) for the LSP event. The
    // seed is ASCII so byte == char.
    let prefix = &content[..farewell_start];
    let line = prefix.matches('\n').count() as u32;
    let last_nl = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let start_char = (farewell_start - last_nl) as u32;
    let total_lines = content.matches('\n').count() as u32;

    let event = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: Position {
                line,
                character: start_char,
            },
            end: Position {
                line: total_lines,
                character: 0,
            },
        }),
        range_length: None,
        text: String::new(),
    };
    store
        .apply_change(&uri, 2, std::slice::from_ref(&event))
        .expect("apply");

    let (changes, _) = extract_and_diff(&store, &uri, "remove.ts", Some(&baseline));
    assert_eq!(changes.len(), 1, "expected single change, got {changes:?}");
    assert_eq!(changes[0].kind, SymbolChangeKind::Removed);
    assert_eq!(changes[0].id.as_str(), "remove.ts::farewell");
    assert!(changes[0].from.is_some());
    assert!(changes[0].to.is_none());
}

/// (4) Function moved — insert a blank line above `farewell` AND
/// rewrite its body so both its start byte and its body hash change.
///
/// Plan 18 v2 heuristic: a pure offset shift with identical body
/// bytes is *suppressed* (so renames don't fire `moved` cascades on
/// every sibling below them). A real `moved` requires the body to
/// also have changed. This test covers the moved-with-edit path,
/// while [`rename_function_emits_single_rewritten_change`] above
/// covers the sibling-shift suppression case implicitly.
#[test]
fn move_function_emits_moved_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let uri = fixture_uri(&dir, "move.ts");
    let store = LiveAstStore::new();
    open_seed(&store, &uri, SEED_TS);
    let (_seed, baseline) = extract_and_diff(&store, &uri, "move.ts", None);

    // Two-event transaction: insert a blank line above `farewell` so
    // its offset shifts AND replace its body so its body_hash also
    // changes. Both pre-conditions for `moved` per the v2 heuristic.
    let line_insert = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: Position {
                line: 3,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 0,
            },
        }),
        range_length: None,
        text: "\n".to_string(),
    };
    // After the line insert, `farewell`'s body lives at line 6 chars
    // 28..43 (the template literal `bye, ${name}`). Replace it with a
    // distinct return value so the body_hash rolls.
    let body_edit = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: Position {
                line: 6,
                character: 12,
            },
            end: Position {
                line: 6,
                character: 28,
            },
        }),
        range_length: None,
        text: "`adios, ${name}`".to_string(),
    };
    store
        .apply_change(&uri, 2, &[line_insert, body_edit])
        .expect("apply");

    let (changes, _) = extract_and_diff(&store, &uri, "move.ts", Some(&baseline));
    let move_changes: Vec<_> = changes
        .iter()
        .filter(|c| c.kind == SymbolChangeKind::Moved)
        .collect();
    assert_eq!(
        move_changes.len(),
        1,
        "exactly one moved expected, got changes={changes:?}"
    );
    assert_eq!(move_changes[0].id.as_str(), "move.ts::farewell");
    let from = move_changes[0]
        .from
        .as_ref()
        .expect("moved must carry from");
    let to = move_changes[0].to.as_ref().expect("moved must carry to");
    assert!(
        to.byte_range.0 > from.byte_range.0,
        "moved symbol must have a later start; from={from:?} to={to:?}"
    );

    // `greet` must not appear — it sits before the inserted line and
    // its byte range is unchanged.
    assert!(
        !changes.iter().any(|c| c.id.as_str().ends_with("greet")),
        "unaffected sibling must stay silent: {changes:?}"
    );
}

/// Sibling-shift suppression — verify the heuristic that a pure
/// offset shift with identical body bytes does NOT fire any change
/// event. This is the core "no noise on sibling rename" guarantee.
#[test]
fn pure_sibling_shift_emits_no_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let uri = fixture_uri(&dir, "shift.ts");
    let store = LiveAstStore::new();
    open_seed(&store, &uri, SEED_TS);
    let (_seed, baseline) = extract_and_diff(&store, &uri, "shift.ts", None);

    // Insert a blank line at line 3 (the empty separator between
    // greet and farewell) so farewell's byte range shifts down by one
    // line without rewriting its name or body.
    let event = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: Position {
                line: 3,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 0,
            },
        }),
        range_length: None,
        text: "\n".to_string(),
    };
    store
        .apply_change(&uri, 2, std::slice::from_ref(&event))
        .expect("apply");

    let (changes, _) = extract_and_diff(&store, &uri, "shift.ts", Some(&baseline));
    assert!(
        changes.is_empty(),
        "pure offset shift with identical body bytes must be silent; got {changes:?}"
    );
}

/// (5) Capability JSON flip — Plan 18 v2 contract: `available: true`
/// plus the four diff kinds.
#[test]
fn capability_advertises_all_four_kinds() {
    let cap = symbol_changed_capability_json();
    assert_eq!(cap["available"], serde_json::json!(true));
    let kinds = cap["kinds"]
        .as_array()
        .expect("kinds array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"added"));
    assert!(kinds.contains(&"removed"));
    assert!(kinds.contains(&"moved"));
    assert!(kinds.contains(&"rewritten"));
    assert_eq!(cap["version"], serde_json::json!("v1-string"));
}

/// (6) Class rename — non-function kind path. Assert `rewritten` for
/// the renamed class and zero events for an unchanged sibling.
#[test]
fn class_rename_classifies_as_rewritten() {
    const SEED: &str = "\
export class Foo {
    bar(): number { return 1; }
}

export class Sibling {
    keep(): void {}
}
";
    let dir = tempfile::tempdir().expect("tempdir");
    let uri = fixture_uri(&dir, "klass.ts");
    let store = LiveAstStore::new();
    open_seed(&store, &uri, SEED);
    let (_seed, baseline) = extract_and_diff(&store, &uri, "klass.ts", None);

    // Rename `Foo` to `Bar` — chars 13..16 on line 0.
    let event = change_replace(0, 13, 16, "Bar");
    store
        .apply_change(&uri, 2, std::slice::from_ref(&event))
        .expect("apply");

    let (changes, _) = extract_and_diff(&store, &uri, "klass.ts", Some(&baseline));
    assert_eq!(changes.len(), 1, "expected single change, got {changes:?}");
    assert_eq!(changes[0].kind, SymbolChangeKind::Rewritten);
    assert_eq!(changes[0].id.as_str(), "klass.ts::Bar");
}
