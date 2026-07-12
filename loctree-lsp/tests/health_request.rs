//! Integration test for the `loctree/health` custom LSP request (Plan 09).
//!
//! Covers params parsing, status thresholds, hotspot detection, and the
//! response shape contract.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use loctree_lsp::{HealthParams, health};

#[test]
fn params_deserialize_minimal() {
    let json = serde_json::json!({});
    let params: HealthParams = serde_json::from_value(json).expect("minimal params parse");
    assert!(params.project.is_none());
    assert!(!params.include_top_risks);
}

#[test]
fn params_deserialize_full() {
    let json = serde_json::json!({
        "project": "/abs/repo",
        "include_top_risks": true
    });
    let params: HealthParams = serde_json::from_value(json).expect("full params parse");
    assert_eq!(
        params
            .project
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        Some("/abs/repo".into())
    );
    assert!(params.include_top_risks);
}

#[test]
fn status_label_matches_plan_thresholds() {
    assert_eq!(health::status_label(100), "green");
    assert_eq!(health::status_label(80), "green");
    assert_eq!(health::status_label(79), "yellow");
    assert_eq!(health::status_label(50), "yellow");
    assert_eq!(health::status_label(49), "red");
    assert_eq!(health::status_label(0), "red");
}

#[test]
fn count_hotspots_thresholds_at_ten_importers() {
    use loctree::snapshot::{GraphEdge, Snapshot};

    let mut snapshot = Snapshot::new(vec![]);
    // 9 edges into "low.rs" — under threshold.
    for i in 0..9 {
        snapshot.edges.push(GraphEdge {
            from: format!("src/caller_{i}.rs"),
            to: "src/low.rs".into(),
            label: "import".into(),
        });
    }
    // 12 edges into "hot.rs" — above threshold.
    for i in 0..12 {
        snapshot.edges.push(GraphEdge {
            from: format!("src/caller_{i}.rs"),
            to: "src/hot.rs".into(),
            label: "import".into(),
        });
    }

    let hot = health::count_hotspots(&snapshot);
    assert_eq!(hot, 1, "only files with ≥10 importers count as hotspots");
}

#[test]
fn snapshot_age_returns_zero_for_unparseable_timestamp() {
    use loctree::snapshot::Snapshot;

    let mut snapshot = Snapshot::new(vec![]);
    snapshot.metadata.generated_at = String::new();
    assert_eq!(health::snapshot_age_seconds(&snapshot), 0);

    snapshot.metadata.generated_at = "not-a-date".into();
    assert_eq!(health::snapshot_age_seconds(&snapshot), 0);
}
