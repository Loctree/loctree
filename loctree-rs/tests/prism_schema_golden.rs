//! Golden JSON schema regression for `loctree.prism.v1.1`.
//!
//! Pins the wire shape that `loct prism --json` and the `loctree-mcp`
//! `prism` tool emit. Any change here is a breaking change for the
//! polarize runner contract — bump the schema version and emit a
//! migration note in CHANGELOG before adjusting the fixture.

use std::collections::BTreeSet;

use loctree::{PrismAxisScore, PrismOverlap, PrismReport, PrismTaskSummary};
use serde_json::Value;

fn synthetic_summary(
    task: &str,
    files: &[&str],
    surface_kinds: &[&str],
    authority_labels: &[&str],
    runtime_signal_count: usize,
    verification_gates: &[&str],
    likely_tests: &[&str],
) -> PrismTaskSummary {
    let file_set: BTreeSet<String> = files.iter().map(|f| f.to_string()).collect();
    PrismTaskSummary {
        task: task.to_string(),
        file_count: file_set.len(),
        runtime_signal_count,
        memory_entry_count: 0,
        low_lexical_memory_count: 0,
        surface_kinds: surface_kinds.iter().map(|s| s.to_string()).collect(),
        authority_labels: authority_labels.iter().map(|s| s.to_string()).collect(),
        top_files: files.iter().map(|f| f.to_string()).collect(),
        verification_gates: verification_gates.iter().map(|s| s.to_string()).collect(),
        likely_tests: likely_tests.iter().map(|s| s.to_string()).collect(),
        cache_scope: "clean".to_string(),
        stale_snapshot: false,
        dirty_worktree: false,
        file_set,
    }
}

fn synthetic_minimal_report() -> PrismReport {
    PrismReport {
        schema_version: "loctree.prism.v1.1".to_string(),
        project_root: "/synthetic/repo".to_string(),
        tasks: vec!["auth flow".to_string(), "auth core".to_string()],
        total_score: 8,
        band: "5..8: local note or Loctree tag".to_string(),
        axes: vec![
            PrismAxisScore {
                axis: "spread".to_string(),
                score: 2,
                evidence: vec!["surface kinds: code, runtime, tests".to_string()],
            },
            PrismAxisScore {
                axis: "runtime_centrality".to_string(),
                score: 1,
                evidence: vec!["runtime signals: 3; central files: 0".to_string()],
            },
            PrismAxisScore {
                axis: "authority_diversity".to_string(),
                score: 1,
                evidence: vec!["authority labels: loctree_derived, repo_verified".to_string()],
            },
            PrismAxisScore {
                axis: "drift_risk".to_string(),
                score: 2,
                evidence: vec![
                    "average pairwise file overlap: 0.333".to_string(),
                    "low lexical memory entries: 0".to_string(),
                    "stale or dirty cache signal: false".to_string(),
                ],
            },
            PrismAxisScore {
                axis: "closure_evidence".to_string(),
                score: 2,
                evidence: vec![
                    "verification gates: 1".to_string(),
                    "likely tests: 2".to_string(),
                ],
            },
        ],
        task_summaries: vec![
            synthetic_summary(
                "auth flow",
                &["src/auth/login.rs", "src/auth/session.rs"],
                &["code", "runtime"],
                &["repo_verified"],
                2,
                &["cargo test"],
                &["tests/auth_flow.rs"],
            ),
            synthetic_summary(
                "auth core",
                &["src/auth/session.rs", "tests/auth_core.rs"],
                &["code", "tests"],
                &["loctree_derived"],
                1,
                &[],
                &["tests/auth_core.rs"],
            ),
        ],
        overlap: PrismOverlap {
            union_files: 3,
            shared_files_all_tasks: 1,
            average_pairwise_jaccard: 0.333,
        },
        recommendation: "Capture a local note or tag before implementation continues.".to_string(),
        band_action: "memo".to_string(),
    }
}

#[test]
fn prism_v1_1_schema_matches_golden_fixture() {
    let report = synthetic_minimal_report();
    let actual: Value =
        serde_json::to_value(&report).expect("PrismReport must serialize without error");

    let golden: Value = serde_json::from_str(include_str!("fixtures/prism_v1_minimal.json"))
        .expect("golden fixture is valid JSON");

    assert_eq!(
        actual, golden,
        "loctree.prism.v1.1 schema drift detected — bump schema_version + record migration before \
         refreshing the fixture (see CHANGELOG)."
    );
}

#[test]
fn prism_v1_1_top_level_field_set_is_pinned() {
    // Defensive contract check: independent of the deep golden, ensure the
    // top-level field set never silently drops or gains a key. Future schema
    // changes should be deliberate (and accompanied by a version bump).
    let report = synthetic_minimal_report();
    let value = serde_json::to_value(&report).expect("PrismReport must serialize");
    let object = value
        .as_object()
        .expect("PrismReport must serialize to a JSON object");

    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort();

    let expected = [
        "axes",
        "band",
        "band_action",
        "overlap",
        "project_root",
        "recommendation",
        "schema_version",
        "task_summaries",
        "tasks",
        "total_score",
    ];
    assert_eq!(keys, expected, "top-level PrismReport keys drifted");
}

#[test]
fn prism_v1_1_task_summary_excludes_internal_file_set() {
    let report = synthetic_minimal_report();
    let value = serde_json::to_value(&report).expect("PrismReport must serialize");
    let summaries = value["task_summaries"]
        .as_array()
        .expect("task_summaries is a JSON array");
    assert!(!summaries.is_empty());
    for summary in summaries {
        let object = summary
            .as_object()
            .expect("task_summaries entries must serialize as objects");
        assert!(
            !object.contains_key("file_set"),
            "internal `file_set` aggregation must not leak into the loctree.prism.v1.1 wire shape"
        );
    }
}

#[test]
fn prism_v1_1_axes_pin_the_five_canonical_axis_names() {
    // `vc-polarize` consumes axes by name; renaming or reordering breaks the
    // recommendation logic in handlers/prism.rs::prism_recommendation.
    let report = synthetic_minimal_report();
    let value = serde_json::to_value(&report).expect("PrismReport must serialize");
    let axes = value["axes"].as_array().expect("axes is an array");
    let names: Vec<&str> = axes
        .iter()
        .map(|axis| axis["axis"].as_str().expect("axis name is a string"))
        .collect();
    assert_eq!(
        names,
        vec![
            "spread",
            "runtime_centrality",
            "authority_diversity",
            "drift_risk",
            "closure_evidence",
        ],
        "axis names or ordering drifted from the loctree.prism.v1.1 contract"
    );
}
