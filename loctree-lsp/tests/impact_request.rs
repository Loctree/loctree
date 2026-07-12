//! Integration test for the `loctree/impact` custom LSP request (Plan 06).
//!
//! Covers: params deserialization, severity classification, dynamic-import
//! warnings, and the response-shape contract (paths-only, plus blast metadata).
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use loctree::impact::{ImpactEntry, ImpactResult};
use loctree_lsp::{ImpactParams, ImpactResponse, impact};

fn entry(file: &str, depth: usize, import_type: &str) -> ImpactEntry {
    ImpactEntry {
        file: file.into(),
        depth,
        import_type: import_type.into(),
        chain: vec!["src/lib.rs".into(), file.into()],
    }
}

fn fixture_result(direct: Vec<ImpactEntry>, transitive: Vec<ImpactEntry>) -> ImpactResult {
    let max_depth = direct
        .iter()
        .chain(transitive.iter())
        .map(|e| e.depth)
        .max()
        .unwrap_or(0);
    let total = direct.len() + transitive.len();
    ImpactResult {
        target: "src/lib.rs".into(),
        direct_consumers: direct,
        transitive_consumers: transitive,
        total_affected: total,
        max_depth,
        target_ignored: false,
    }
}

#[test]
fn params_deserialize_minimal() {
    let json = serde_json::json!({ "target": "src/lib.rs" });
    let params: ImpactParams = serde_json::from_value(json).expect("minimal params parse");
    assert_eq!(params.target.to_string_lossy(), "src/lib.rs");
    assert!(!params.transitive, "transitive defaults to false");
    assert!(params.project.is_none(), "project defaults to None");
}

#[test]
fn params_deserialize_full() {
    let json = serde_json::json!({
        "target": "src/lib.rs",
        "transitive": true,
        "project": "/abs/repo"
    });
    let params: ImpactParams = serde_json::from_value(json).expect("full params parse");
    assert!(params.transitive);
    assert_eq!(
        params
            .project
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        Some("/abs/repo".into())
    );
}

#[test]
fn options_short_circuit_when_transitive_off() {
    let params = ImpactParams {
        target: "src/lib.rs".into(),
        transitive: false,
        project: None,
    };
    let opts = impact::options_from_params(&params);
    assert_eq!(
        opts.max_depth,
        Some(1),
        "transitive=false caps BFS at depth 1"
    );
}

#[test]
fn response_low_severity_under_five() {
    let result = fixture_result(
        vec![
            entry("a.rs", 1, "import"),
            entry("b.rs", 1, "import"),
            entry("c.rs", 1, "import"),
        ],
        vec![],
    );
    let response = ImpactResponse::from_impact(&result, false);
    assert_eq!(response.direct.len(), 3);
    assert!(response.transitive.is_empty());
    assert_eq!(response.total, 3);
    assert_eq!(response.blast_severity, "low");
    assert!(response.warnings.is_empty());
}

#[test]
fn response_medium_severity_when_in_band() {
    let direct: Vec<ImpactEntry> = (0..6)
        .map(|i| entry(&format!("a{i}.rs"), 1, "import"))
        .collect();
    let transitive: Vec<ImpactEntry> = vec![entry("t.rs", 2, "import")];
    let result = fixture_result(direct, transitive);
    let response = ImpactResponse::from_impact(&result, true);
    assert_eq!(response.total, 7);
    assert_eq!(response.blast_severity, "medium");
}

#[test]
fn response_high_severity_when_over_twenty() {
    let direct: Vec<ImpactEntry> = (0..21)
        .map(|i| entry(&format!("a{i}.rs"), 1, "import"))
        .collect();
    let result = fixture_result(direct, vec![]);
    let response = ImpactResponse::from_impact(&result, false);
    assert_eq!(response.total, 21);
    assert_eq!(response.blast_severity, "high");
}

#[test]
fn response_high_severity_when_depth_exceeds_three() {
    let result = fixture_result(
        vec![entry("a.rs", 1, "import")],
        vec![
            entry("b.rs", 2, "import"),
            entry("c.rs", 3, "import"),
            entry("d.rs", 4, "import"),
        ],
    );
    let response = ImpactResponse::from_impact(&result, true);
    assert_eq!(response.total, 4);
    assert_eq!(
        response.blast_severity, "high",
        "depth >3 forces high severity"
    );
}

#[test]
fn response_dynamic_import_warning_is_collected() {
    let result = fixture_result(
        vec![
            entry("a.rs", 1, "import"),
            entry("b.rs", 1, "dynamic"),
            entry("c.rs", 1, "import()"),
        ],
        vec![],
    );
    let response = ImpactResponse::from_impact(&result, false);
    assert_eq!(response.warnings.len(), 1);
    let warning = &response.warnings[0];
    assert!(warning.contains("dynamic imports"), "warning: {warning}");
    assert!(warning.starts_with("2 importer"), "warning: {warning}");
}

#[test]
fn response_drops_transitive_when_not_requested() {
    let result = fixture_result(
        vec![entry("a.rs", 1, "import")],
        vec![entry("b.rs", 2, "import")],
    );
    let response = ImpactResponse::from_impact(&result, false);
    assert_eq!(response.direct.len(), 1);
    assert!(
        response.transitive.is_empty(),
        "transitive must be empty when not requested"
    );
    assert_eq!(
        response.total, 1,
        "transitive consumers excluded from total"
    );
}

#[test]
fn response_serializes_to_paths_only_entries() {
    let result = fixture_result(
        vec![entry("src/util.rs", 1, "import")],
        vec![entry("src/inner.rs", 2, "import")],
    );
    let response = ImpactResponse::from_impact(&result, true);
    let json = serde_json::to_value(&response).expect("serializes");
    let obj = json.as_object().unwrap();

    let mut keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "blast_severity",
            "direct",
            "total",
            "transitive",
            "warnings"
        ]
    );

    for entry in obj["direct"]
        .as_array()
        .unwrap()
        .iter()
        .chain(obj["transitive"].as_array().unwrap())
    {
        let entry_obj = entry.as_object().unwrap();
        let mut entry_keys: Vec<&str> = entry_obj.keys().map(|s| s.as_str()).collect();
        entry_keys.sort();
        assert_eq!(entry_keys, ["depth", "path"], "got: {entry_keys:?}");
    }
}
