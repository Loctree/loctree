//! Integration test for the `loctree/body` custom LSP request.
//!
//! Covers params parsing, end-to-end body retrieval through the shared
//! `loctree::body` engine, the optional `file` disambiguation filter, and
//! the response-shape contract (byte-for-byte the `loct body --json` shape).
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use loctree::body::query_symbol_body;
use loctree::snapshot::Snapshot;
use loctree::types::FileAnalysis;
use loctree_lsp::{BodyParams, BodyResponse};

const SOURCE: &str = "// header\npub fn resolveServerBinary() -> String {\n    let mut path = String::new();\n    path.push_str(\"loctree-lsp\");\n    path\n}\n\nfn other() {}\n";

/// Build a snapshot over a real on-disk source file whose `resolveServerBinary`
/// export is recorded at its true definition line, so the engine can read the
/// bounded body straight from disk. Absolute paths keep the test cwd-agnostic.
fn snapshot_with_symbol(dir: &std::path::Path) -> Snapshot {
    let abs = dir.join("server.rs");
    std::fs::write(&abs, SOURCE).expect("write server.rs");

    let mut snapshot = Snapshot::new(vec![dir.display().to_string()]);
    let mut file = FileAnalysis::new(abs.display().to_string());
    file.exports.push(loctree::types::ExportSymbol {
        name: "resolveServerBinary".to_string(),
        kind: "function".to_string(),
        export_type: "named".to_string(),
        // `resolveServerBinary` is defined on line 2 of SOURCE.
        line: Some(2),
        params: Vec::new(),
        symbol_id: loctree::types::SymbolIdV1::default(),
    });
    snapshot.files.push(file);
    snapshot
}

#[test]
fn params_deserialize_minimal() {
    let json = serde_json::json!({ "symbol": "resolveServerBinary" });
    let params: BodyParams = serde_json::from_value(json).expect("minimal params parse");
    assert_eq!(params.symbol, "resolveServerBinary");
    assert!(params.max_lines.is_none());
    assert!(params.file.is_none());
    assert!(params.project.is_none());
}

#[test]
fn params_deserialize_full() {
    let json = serde_json::json!({
        "symbol": "resolveServerBinary",
        "max_lines": 40,
        "file": "server.rs",
        "project": "/abs/repo"
    });
    let params: BodyParams = serde_json::from_value(json).expect("full params parse");
    assert_eq!(params.max_lines, Some(40));
    assert_eq!(params.file.as_deref(), Some("server.rs"));
    assert_eq!(
        params
            .project
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        Some("/abs/repo".into())
    );
}

#[test]
fn body_request_returns_bounded_body_for_known_symbol() {
    let dir = tempfile::tempdir().expect("temp project");
    let snapshot = snapshot_with_symbol(dir.path());

    let result = query_symbol_body(&snapshot, "resolveServerBinary", None);
    let response = BodyResponse::from_result(result, None);

    assert_eq!(response.symbol, "resolveServerBinary");
    assert_eq!(response.bodies.len(), 1, "exactly one definition body");
    let body = &response.bodies[0];
    assert!(
        body.file.ends_with("server.rs"),
        "file should point at the defining file; got {}",
        body.file
    );
    assert_eq!(body.start_line, 2, "body anchored at the def line");
    assert_eq!(body.symbol, "resolveServerBinary");
    assert_eq!(body.language, "rs");
    assert!(
        body.source.contains("pub fn resolveServerBinary"),
        "source must contain the symbol signature; got:\n{}",
        body.source
    );
    assert!(
        body.source.trim_end().ends_with('}'),
        "brace-balanced body should end on the closing brace; got:\n{}",
        body.source
    );
    assert!(!body.truncated, "small body is not truncated");
}

#[test]
fn body_request_file_filter_drops_non_matching_bodies() {
    let dir = tempfile::tempdir().expect("temp project");
    let snapshot = snapshot_with_symbol(dir.path());

    let result = query_symbol_body(&snapshot, "resolveServerBinary", None);
    // Filter to a path that does not match — the body must be dropped.
    let response = BodyResponse::from_result(result, Some("does/not/match.rs"));
    assert!(
        response.bodies.is_empty(),
        "file filter must drop non-matching bodies: {response:?}"
    );
}

#[test]
fn body_request_missing_symbol_returns_empty_bodies() {
    let dir = tempfile::tempdir().expect("temp project");
    let snapshot = snapshot_with_symbol(dir.path());

    let result = query_symbol_body(&snapshot, "definitelyNotPresent", None);
    let response = BodyResponse::from_result(result, None);
    assert_eq!(response.symbol, "definitelyNotPresent");
    assert!(response.bodies.is_empty());
}

#[test]
fn response_serializes_to_loct_body_json_shape() {
    let dir = tempfile::tempdir().expect("temp project");
    let snapshot = snapshot_with_symbol(dir.path());

    let result = query_symbol_body(&snapshot, "resolveServerBinary", None);
    let response = BodyResponse::from_result(result, None);
    let json = serde_json::to_value(&response).expect("serialize body response");

    let obj = json.as_object().expect("top-level object");
    let mut top_keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
    top_keys.sort();
    assert_eq!(top_keys, ["bodies", "symbol"]);

    let entry = json["bodies"][0].as_object().expect("body object");
    let mut entry_keys: Vec<&str> = entry.keys().map(|s| s.as_str()).collect();
    entry_keys.sort();
    assert_eq!(
        entry_keys,
        [
            "end_line",
            "file",
            "language",
            "line_cap",
            "source",
            "start_line",
            "symbol",
            "total_lines",
            "truncated",
        ],
        "body must serialize the exact 9-field `loct body --json` shape"
    );
}
