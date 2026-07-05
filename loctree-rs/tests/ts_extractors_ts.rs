//! Plan 19 Stage 1 — integration coverage for the tree-sitter TS/JS extractor
//! pipeline reachable through `analyzer::scan::ts_dispatch_js`.
//!
//! These tests exercise the public-facing surface (FileAnalysis fields) so we
//! catch shape regressions even if the upstream extractors stay stable.

use loctree::analyzer::scan::ts_dispatch_js;
use std::path::PathBuf;

fn fixture_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/simple_ts");
    path
}

fn analyze(rel: &str) -> loctree::types::FileAnalysis {
    let root = fixture_root();
    let abs = root.join(rel);
    let content = std::fs::read_to_string(&abs).expect("fixture readable");
    ts_dispatch_js(&content, &abs, rel.to_string())
}

#[test]
fn ts_dispatch_extracts_function_exports_from_index() {
    let analysis = analyze("src/index.ts");
    assert_eq!(analysis.exports.len(), 1, "{:?}", analysis.exports);
    let main = &analysis.exports[0];
    assert_eq!(main.name, "main");
    assert_eq!(main.kind, "function");
    assert_eq!(main.export_type, "named");
    assert_eq!(main.line, Some(5));
}

#[test]
fn ts_dispatch_extracts_named_imports_with_local_resolution_hint() {
    let analysis = analyze("src/index.ts");
    let sources: Vec<&str> = analysis.imports.iter().map(|i| i.source.as_str()).collect();
    assert!(
        sources.contains(&"./utils/greeting"),
        "greeting import missing: {sources:?}"
    );
    assert!(
        sources.contains(&"./utils/date"),
        "date import missing: {sources:?}"
    );
    let greeting = analysis
        .imports
        .iter()
        .find(|i| i.source == "./utils/greeting")
        .unwrap();
    assert!(!greeting.is_bare, "relative import must not be bare");
    assert!(greeting.symbols.iter().any(|s| s.name == "greet"));
}

#[test]
fn ts_dispatch_extracts_multiple_exports_per_file() {
    let analysis = analyze("src/utils/date.ts");
    let names: Vec<&str> = analysis.exports.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"formatDate"), "{names:?}");
    assert!(names.contains(&"parseDate"), "{names:?}");
    assert_eq!(analysis.exports.len(), 2);
}

#[test]
fn ts_dispatch_records_call_sites_as_symbol_usages() {
    let analysis = analyze("src/index.ts");
    let names: Vec<&str> = analysis
        .symbol_usages
        .iter()
        .map(|u| u.name.as_str())
        .collect();
    // Stage 1 records every call_expression: greet(...), formatDate(...),
    // console.log(...) (twice), main(). `name` is the trailing identifier
    // for member expressions.
    assert!(names.contains(&"greet"), "greet call missing: {names:?}");
    assert!(
        names.contains(&"formatDate"),
        "formatDate call missing: {names:?}"
    );
    assert!(
        names.contains(&"log"),
        "console.log call missing: {names:?}"
    );
    assert!(
        names.contains(&"main"),
        "main() invocation missing: {names:?}"
    );
}
