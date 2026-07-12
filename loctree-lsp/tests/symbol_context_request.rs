//! Integration test for the `loctree/symbolContext` custom LSP request.
//!
//! Proves the keystone contract:
//! - `exported: true` for an exported symbol (resolved from symbol IDENTITY,
//!   NOT from `find` literal's `dead_status` stub).
//! - `internal: true` for a file-local, non-exported symbol.
//! - `body` resolves to the REQUESTED file (disambiguation), truncates.
//! - `occurrences` paginate with `total` / `same_file_total` / `has_more` /
//!   `next_offset`.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use loctree::analyzer::occurrences::scan_files;
use loctree::snapshot::Snapshot;
use loctree::types::{
    ExportSymbol, FileAnalysis, ImportEntry, ImportKind, ImportSymbol, LocalSymbol, SymbolUsage,
};
use loctree_lsp::symbol_context::{
    DEFAULT_BODY_MAX_LINES, DEFAULT_OCCURRENCE_LIMIT, SymbolContextParams, build_body,
    build_occurrences, line_range, resolve_identity,
};
use tower_lsp::lsp_types::Position;

fn export(name: &str, line: usize) -> ExportSymbol {
    ExportSymbol {
        name: name.to_string(),
        kind: "function".to_string(),
        export_type: "named".to_string(),
        line: Some(line),
        params: Vec::new(),
        symbol_id: loctree::types::SymbolIdV1::default(),
    }
}

fn local(name: &str, line: usize, exported: bool) -> LocalSymbol {
    LocalSymbol {
        name: name.to_string(),
        kind: "function".to_string(),
        line: Some(line),
        context: String::new(),
        is_exported: exported,
    }
}

/// An import bringing in `name` (optionally aliased, optionally default) from
/// `source`, resolved to `resolved_path` (None for bare/unresolved).
fn import_of(
    source: &str,
    resolved_path: Option<&str>,
    name: &str,
    alias: Option<&str>,
    is_default: bool,
) -> ImportEntry {
    let mut entry = ImportEntry::new(source.to_string(), ImportKind::Static);
    entry.resolved_path = resolved_path.map(|p| p.to_string());
    entry.symbols.push(ImportSymbol {
        name: name.to_string(),
        alias: alias.map(|a| a.to_string()),
        is_default,
    });
    entry
}

fn usage(name: &str, line: usize) -> SymbolUsage {
    SymbolUsage {
        name: name.to_string(),
        line,
        context: String::new(),
    }
}

fn pos(line: u32) -> Position {
    Position { line, character: 0 }
}

#[test]
fn params_deserialize_minimal_with_defaults() {
    let json = serde_json::json!({
        "file": "src/server.rs",
        "position": { "line": 1, "character": 0 }
    });
    let params: SymbolContextParams = serde_json::from_value(json).expect("minimal parse");
    assert_eq!(params.file, "src/server.rs");
    assert_eq!(params.position.line, 1);
    assert!(params.symbol.is_none());
    assert_eq!(params.body_max_lines_resolved(), DEFAULT_BODY_MAX_LINES);
    assert_eq!(params.occurrence_limit_resolved(), DEFAULT_OCCURRENCE_LIMIT);
    assert!(!params.same_file_only);
}

/// EXPORTED symbol → `exported: true`, `internal: false`, resolved from the
/// symbol identity at file+line — never from `find` literal `dead_status`.
#[test]
fn exported_symbol_reports_exported_true() {
    let mut file = FileAnalysis::new("src/server.rs".to_string());
    file.exports.push(export("resolveServerBinary", 2));
    let mut snapshot = Snapshot::new(vec!["/tmp/proj".to_string()]);
    snapshot.files.push(file);

    // LSP line 1 (0-based) maps to snapshot line 2 (1-based).
    let identity = resolve_identity(&snapshot, "src/server.rs", pos(1), None)
        .expect("exported symbol resolves");
    assert_eq!(identity.name, "resolveServerBinary");
    assert!(identity.exported, "exported symbol → exported=true");
    assert!(!identity.internal, "exported symbol → internal=false");

    let range = line_range(identity.line).expect("range");
    assert_eq!(range.start.line, 1, "0-based range line");
}

/// FILE-LOCAL non-exported symbol → `internal: true`, `exported: false`.
#[test]
fn file_local_symbol_reports_internal_true() {
    let mut file = FileAnalysis::new("src/server.rs".to_string());
    file.local_symbols.push(local("handleInternal", 12, false));
    let mut snapshot = Snapshot::new(vec!["/tmp/proj".to_string()]);
    snapshot.files.push(file);

    let identity =
        resolve_identity(&snapshot, "src/server.rs", pos(11), None).expect("local symbol resolves");
    assert_eq!(identity.name, "handleInternal");
    assert!(identity.internal, "non-exported local → internal=true");
    assert!(!identity.exported, "non-exported local → exported=false");
}

/// `symbol` hint disambiguates when the cursor line doesn't sit on the decl.
#[test]
fn symbol_hint_resolves_when_line_does_not_match() {
    let mut file = FileAnalysis::new("src/server.rs".to_string());
    file.exports.push(export("run", 5));
    let mut snapshot = Snapshot::new(vec!["/tmp/proj".to_string()]);
    snapshot.files.push(file);

    // Cursor on an unrelated line (0-based 99) but with a symbol hint.
    let identity = resolve_identity(&snapshot, "src/server.rs", pos(99), Some("run"))
        .expect("hint resolves identity");
    assert_eq!(identity.name, "run");
    assert!(identity.exported);
}

/// CROSS-FILE via the import graph: file A imports `{ foo }` from B, B exports
/// `foo`. Hovering a USAGE of `foo` in A resolves `exported=true` and
/// `defined_in=B` (the declaring file), NOT unresolved.
#[test]
fn imported_symbol_resolves_cross_file_via_import_graph() {
    // B declares + exports `foo`.
    let mut b = FileAnalysis::new("src/b.ts".to_string());
    b.exports.push(export("foo", 1));

    // A imports `{ foo }` from B (resolved to src/b.ts) and uses it on line 5.
    let mut a = FileAnalysis::new("src/a.ts".to_string());
    a.imports
        .push(import_of("./b", Some("src/b.ts"), "foo", None, false));
    a.symbol_usages.push(usage("foo", 5));

    let mut snapshot = Snapshot::new(vec!["/tmp/proj".to_string()]);
    snapshot.files.push(a);
    snapshot.files.push(b);

    // Cursor on the usage line in A (0-based 4 == snapshot line 5), with hint.
    let identity = resolve_identity(&snapshot, "src/a.ts", pos(4), Some("foo"))
        .expect("imported symbol resolves cross-file");
    assert_eq!(identity.name, "foo");
    assert!(identity.exported, "B exports foo → exported=true");
    assert!(!identity.internal);
    assert_eq!(
        identity.defined_in.as_deref(),
        Some("src/b.ts"),
        "declaring file from the import graph"
    );
}

/// The cursor symbol can come from `symbol_usages` (no caller hint) and still
/// resolve cross-file through the import graph.
#[test]
fn imported_symbol_resolves_without_hint_from_usage_line() {
    let mut b = FileAnalysis::new("src/b.ts".to_string());
    b.exports.push(export("foo", 1));

    let mut a = FileAnalysis::new("src/a.ts".to_string());
    a.imports
        .push(import_of("./b", Some("src/b.ts"), "foo", None, false));
    a.symbol_usages.push(usage("foo", 5));

    let mut snapshot = Snapshot::new(vec!["/tmp/proj".to_string()]);
    snapshot.files.push(a);
    snapshot.files.push(b);

    let identity = resolve_identity(&snapshot, "src/a.ts", pos(4), None)
        .expect("usage-line symbol resolves cross-file without a hint");
    assert_eq!(identity.name, "foo");
    assert_eq!(identity.defined_in.as_deref(), Some("src/b.ts"));
}

/// ALIASED import (`import { foo as bar }`): the cursor sees the local binding
/// `bar`, but the declaration in B is looked up by the ORIGINAL name `foo`.
#[test]
fn aliased_import_resolves_by_original_name() {
    let mut b = FileAnalysis::new("src/b.ts".to_string());
    b.exports.push(export("foo", 1));

    let mut a = FileAnalysis::new("src/a.ts".to_string());
    a.imports.push(import_of(
        "./b",
        Some("src/b.ts"),
        "foo",
        Some("bar"),
        false,
    ));
    a.symbol_usages.push(usage("bar", 5));

    let mut snapshot = Snapshot::new(vec!["/tmp/proj".to_string()]);
    snapshot.files.push(a);
    snapshot.files.push(b);

    // Hover the local alias `bar`.
    let identity = resolve_identity(&snapshot, "src/a.ts", pos(4), Some("bar"))
        .expect("aliased import resolves by original name");
    // Resolved identity carries the DECLARED name (`foo`), not the alias.
    assert_eq!(identity.name, "foo", "looked up by original name in B");
    assert!(identity.exported);
    assert_eq!(identity.defined_in.as_deref(), Some("src/b.ts"));
}

/// DEFAULT import (`import Foo from './b'`): the binding is matched and the
/// declaration is looked up by the original `name` in B.
#[test]
fn default_import_resolves_cross_file() {
    let mut b = FileAnalysis::new("src/b.ts".to_string());
    b.exports.push(export("Foo", 1));

    let mut a = FileAnalysis::new("src/a.ts".to_string());
    a.imports
        .push(import_of("./b", Some("src/b.ts"), "Foo", None, true));
    a.symbol_usages.push(usage("Foo", 5));

    let mut snapshot = Snapshot::new(vec!["/tmp/proj".to_string()]);
    snapshot.files.push(a);
    snapshot.files.push(b);

    let identity = resolve_identity(&snapshot, "src/a.ts", pos(4), Some("Foo"))
        .expect("default import resolves cross-file");
    assert_eq!(identity.name, "Foo");
    assert_eq!(identity.defined_in.as_deref(), Some("src/b.ts"));
}

/// BARE / unresolved import (`resolved_path: None`, e.g. an npm package or
/// stdlib): we STAY unresolved — no false cross-file resolution, no `defined_in`.
#[test]
fn bare_unresolved_import_stays_unresolved() {
    let mut a = FileAnalysis::new("src/a.ts".to_string());
    // `useState` imported from the bare `react` package — no resolved_path.
    a.imports
        .push(import_of("react", None, "useState", None, false));
    a.symbol_usages.push(usage("useState", 5));

    let mut snapshot = Snapshot::new(vec!["/tmp/proj".to_string()]);
    snapshot.files.push(a);

    let identity = resolve_identity(&snapshot, "src/a.ts", pos(4), Some("useState"));
    assert!(
        identity.is_none(),
        "bare/unresolved import must not resolve cross-file (no name-guessing)"
    );
}

/// NO import edge for the symbol: even if another file happens to declare a
/// same-named symbol, we do NOT scan-and-guess. Honesty over magic.
#[test]
fn no_import_edge_does_not_scan_other_files() {
    // Unrelated file B declares `foo`, but A does NOT import it.
    let mut b = FileAnalysis::new("src/b.ts".to_string());
    b.exports.push(export("foo", 1));

    let mut a = FileAnalysis::new("src/a.ts".to_string());
    a.symbol_usages.push(usage("foo", 5)); // usage, but no import bringing it in

    let mut snapshot = Snapshot::new(vec!["/tmp/proj".to_string()]);
    snapshot.files.push(a);
    snapshot.files.push(b);

    let identity = resolve_identity(&snapshot, "src/a.ts", pos(4), Some("foo"));
    assert!(
        identity.is_none(),
        "without an import edge we never conflate unrelated same-named symbols"
    );
}

/// Same-file resolution wins over the import graph: a symbol re-declared in the
/// current file is local (`defined_in=None`), even if an import of the same name
/// also exists.
#[test]
fn same_file_declaration_wins_over_import() {
    let mut b = FileAnalysis::new("src/b.ts".to_string());
    b.exports.push(export("foo", 1));

    let mut a = FileAnalysis::new("src/a.ts".to_string());
    a.imports
        .push(import_of("./b", Some("src/b.ts"), "foo", None, false));
    // A also declares its own local `foo` on the cursor line.
    a.local_symbols.push(local("foo", 5, false));

    let mut snapshot = Snapshot::new(vec!["/tmp/proj".to_string()]);
    snapshot.files.push(a);
    snapshot.files.push(b);

    let identity = resolve_identity(&snapshot, "src/a.ts", pos(4), Some("foo"))
        .expect("same-file local resolves");
    assert!(identity.internal, "local foo is internal");
    assert!(
        identity.defined_in.is_none(),
        "same-file resolution → defined_in stays None"
    );
}

/// CROSS-FILE BODY: when resolved through the import graph, `build_body` must
/// build the body from the DECLARING file `defined_in`, not the using file.
#[test]
fn cross_file_body_comes_from_declaring_file() {
    let dir = tempfile::tempdir().expect("temp project");
    let a_path = dir.path().join("a.ts");
    let b_path = dir.path().join("b.ts");
    // A only USES foo; B DECLARES it.
    std::fs::write(&a_path, "import { foo } from './b';\nfoo();\n").expect("write a.ts");
    std::fs::write(&b_path, "export function foo() {\n  return 1;\n}\n").expect("write b.ts");

    let mut snapshot = Snapshot::new(vec![dir.path().display().to_string()]);
    let mut a_file = FileAnalysis::new(a_path.display().to_string());
    a_file.symbol_usages.push(usage("foo", 2));
    let mut b_file = FileAnalysis::new(b_path.display().to_string());
    b_file.exports.push(export("foo", 1));
    snapshot.files.push(a_file);
    snapshot.files.push(b_file);

    // Build body disambiguated to the DECLARING file b.ts (what the handler
    // passes as `defined_in`), even though the request anchored on a.ts.
    let resolved = build_body(&snapshot, "foo", "b.ts", 80);
    let body = resolved
        .body
        .expect("body for the imported symbol comes from b.ts");
    assert!(resolved.error.is_none(), "declaring-file body → no error");
    assert!(
        body.source.contains("export function foo"),
        "body is B's declaration: {}",
        body.source
    );
}

/// BODY resolves to the REQUESTED file even when a common name exists in two
/// files (disambiguation), and respects the line cap (truncated/total_lines).
#[test]
fn body_resolves_to_requested_file_and_truncates() {
    let dir = tempfile::tempdir().expect("temp project");

    // Two files both define `run`; we must get the one in server.rs.
    let server = dir.path().join("server.rs");
    let other = dir.path().join("other.rs");
    std::fs::write(
        &server,
        "// header\npub fn run() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    let d = 4;\n}\n",
    )
    .expect("write server.rs");
    std::fs::write(&other, "pub fn run() {\n    other_body();\n}\n").expect("write other.rs");

    let mut snapshot = Snapshot::new(vec![dir.path().display().to_string()]);
    let mut server_file = FileAnalysis::new(server.display().to_string());
    server_file.exports.push(export("run", 2));
    let mut other_file = FileAnalysis::new(other.display().to_string());
    other_file.exports.push(export("run", 1));
    snapshot.files.push(server_file);
    snapshot.files.push(other_file);

    // Disambiguate to server.rs with a low cap → truncated.
    let resolved = build_body(&snapshot, "run", "server.rs", 2);
    let body = resolved.body.expect("body for server.rs");
    assert!(resolved.error.is_none(), "file match → no body_error");
    assert!(
        body.source.contains("pub fn run()"),
        "body should be the server.rs definition: {}",
        body.source
    );
    assert!(
        !body.source.contains("other_body"),
        "body must NOT pull other.rs's run(): {}",
        body.source
    );
    assert_eq!(body.start_line, 2, "anchored at server.rs def line");
    assert!(body.truncated, "low cap should truncate the 6-line body");
    assert!(body.total_lines > 2, "total_lines reflects the full body");
    assert_eq!(body.source.lines().count(), 2, "capped to body_max_lines");
}

/// BODY never lies across files: a symbol that exists only in OTHER files must
/// yield `body: None` + `body_error: "not_found_in_file"`, NOT a cross-file
/// definition (the showBody "demo works, product lies" trap).
#[test]
fn body_does_not_fall_back_to_other_file() {
    let dir = tempfile::tempdir().expect("temp project");
    let other = dir.path().join("other.rs");
    std::fs::write(&other, "pub fn run() {\n    other_body();\n}\n").expect("write other.rs");

    let mut snapshot = Snapshot::new(vec![dir.path().display().to_string()]);
    let mut other_file = FileAnalysis::new(other.display().to_string());
    other_file.exports.push(export("run", 1));
    snapshot.files.push(other_file);

    // Request `run` for a file that has no such body.
    let resolved = build_body(&snapshot, "run", "server.rs", 80);
    assert!(
        resolved.body.is_none(),
        "must not surface other.rs's run() for a server.rs request"
    );
    assert_eq!(
        resolved.error,
        Some("not_found_in_file"),
        "a body exists elsewhere → honest not_found_in_file signal"
    );

    // A symbol with no body anywhere → None body, no error.
    let missing = build_body(&snapshot, "does_not_exist", "server.rs", 80);
    assert!(missing.body.is_none());
    assert!(
        missing.error.is_none(),
        "no body anywhere → no not_found_in_file"
    );
}

/// OCCURRENCES paginate: total / same_file_total / has_more / next_offset.
#[test]
fn occurrences_paginate_across_pages() {
    let results = scan_files(
        [
            ("src/server.rs", "run();\nrun();\nrun();\n"),
            ("src/other.rs", "run();\n"),
        ],
        "run",
    );
    assert_eq!(results.total, 4);

    // Page 1: limit 2 from offset 0 over the whole scope.
    let page1 = build_occurrences(&results, "src/server.rs", false, 0, 2);
    assert_eq!(page1.total, 4, "total counts all files");
    assert_eq!(page1.same_file_total, 3, "same-file total counts server.rs");
    assert_eq!(page1.returned.len(), 2);
    assert!(page1.has_more);
    assert_eq!(page1.next_offset, Some(2));

    // Page 2: the remainder, no next page.
    let page2 = build_occurrences(&results, "src/server.rs", false, 2, 2);
    assert_eq!(page2.returned.len(), 2);
    assert!(!page2.has_more);
    assert_eq!(page2.next_offset, None);
}

/// `same_file_only` restricts the paginated scope to the requested file while
/// `total` keeps reporting the whole-scope count.
#[test]
fn same_file_only_scopes_pagination() {
    let results = scan_files(
        [
            ("src/server.rs", "run();\nrun();\n"),
            ("src/other.rs", "run();\nrun();\nrun();\n"),
        ],
        "run",
    );
    let ctx = build_occurrences(&results, "src/server.rs", true, 0, 50);
    assert_eq!(ctx.total, 5, "total stays whole-scope");
    assert_eq!(ctx.same_file_total, 2);
    assert_eq!(ctx.returned.len(), 2, "only same-file occurrences returned");
    assert!(ctx.returned.iter().all(|o| o.file == "src/server.rs"));
}
