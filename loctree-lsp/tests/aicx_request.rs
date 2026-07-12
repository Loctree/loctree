//! Integration tests for Plan 08 — `loctree/aicx` request module.
//!
//! Pure-function coverage: kinds filter, default/clamp limits, the
//! `aicx_unavailable` graceful response, the entry projection. The
//! end-to-end AICX path requires a live `aicx` binary on PATH and is
//! exercised by the daemon smoke harness rather than this unit test.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use loctree::pack::AuthorityLabel;
use loctree::pack::MemoryEntry;
use loctree::snapshot::Snapshot;
use loctree::types::SymbolIdV1;
use loctree_lsp::aicx::{
    AicxParams, DEFAULT_LIMIT, MAX_LIMIT, clamp_limit, compute, filter_and_project, project_entry,
    unavailable_response,
};

fn entry(kind: &str, authority: AuthorityLabel) -> MemoryEntry {
    MemoryEntry {
        kind: kind.to_string(),
        text: "demo".to_string(),
        authority,
        source_chunk: format!("/tmp/{kind}.md"),
        agent: "claude".to_string(),
        date: "2026-05-07".to_string(),
        timestamp: None,
        session_id: "abc".to_string(),
        project: "demo".to_string(),
        relevance: 1,
        retrieval_score: None,
        retrieval_label: None,
        retrieval_mode: None,
        low_lexical_match: false,
    }
}

#[test]
fn clamp_limit_default_and_cap() {
    assert_eq!(clamp_limit(None), DEFAULT_LIMIT);
    assert_eq!(clamp_limit(Some(0)), 1);
    assert_eq!(clamp_limit(Some(usize::MAX)), MAX_LIMIT);
    assert_eq!(clamp_limit(Some(75)), 75);
}

#[test]
fn unavailable_response_carries_hint_and_status() {
    let resp = unavailable_response("loctree-suite".into(), "file".into());
    assert_eq!(resp.status, "aicx_unavailable");
    assert_eq!(resp.namespace, "loctree-suite");
    assert_eq!(resp.scope, "file");
    assert!(resp.hint.is_some());
    assert!(resp.entries.is_empty());
    assert!(resp.source_chunks.is_empty());
}

#[test]
fn filter_keeps_all_when_kinds_unset() {
    let entries = vec![
        entry("decision", AuthorityLabel::AicxOperator),
        entry("outcome", AuthorityLabel::AicxAgent),
    ];
    let (wire, chunks) = filter_and_project(&entries, &None);
    assert_eq!(wire.len(), 2);
    assert_eq!(chunks.len(), 2);
}

#[test]
fn filter_drops_unmatched_kinds() {
    let entries = vec![
        entry("decision", AuthorityLabel::AicxOperator),
        entry("outcome", AuthorityLabel::AicxAgent),
    ];
    let (wire, _) = filter_and_project(&entries, &Some(vec!["decision".into()]));
    assert_eq!(wire.len(), 1);
    assert_eq!(wire[0].kind, "decision");
}

#[test]
fn filter_failure_alias_matches_aicx_failure_authority() {
    let mut e = entry("outcome", AuthorityLabel::AicxFailure);
    e.text = "rolled back".to_string();
    let (wire, _) = filter_and_project(&[e], &Some(vec!["failure".into()]));
    assert_eq!(wire.len(), 1);
    assert_eq!(wire[0].authority, AuthorityLabel::AicxFailure);
}

#[test]
fn projection_preserves_provenance_fields() {
    let mut e = entry("intent", AuthorityLabel::AicxOperator);
    e.timestamp = Some("2026-05-07T12:00:00Z".into());
    e.retrieval_score = Some(57);
    e.retrieval_label = Some("HIGH".into());
    e.retrieval_mode = Some("embedded_semantic".into());
    e.low_lexical_match = true;

    let projected = project_entry(&e);
    assert_eq!(projected.timestamp.as_deref(), Some("2026-05-07T12:00:00Z"));
    assert_eq!(projected.retrieval_score, Some(57));
    assert_eq!(projected.retrieval_label.as_deref(), Some("HIGH"));
    assert_eq!(
        projected.retrieval_mode.as_deref(),
        Some("embedded_semantic")
    );
    assert!(projected.low_lexical_match);
}

#[test]
fn unavailable_response_serializes_to_stable_keys() {
    let resp = unavailable_response("ns".into(), "project".into());
    let value = serde_json::to_value(&resp).unwrap();
    for key in [
        "status",
        "namespace",
        "scope",
        "entries",
        "source_chunks",
        "symbol_id_version",
    ] {
        assert!(value.get(key).is_some(), "wire key `{key}` missing");
    }
}

#[test]
fn params_schema_exposes_typed_symbol_id() {
    let schema = schemars::schema_for!(AicxParams);
    let value = serde_json::to_value(schema).unwrap();
    let schema_text = serde_json::to_string(&value).unwrap();
    assert!(
        schema_text.contains("\"symbol_id\""),
        "AicxParams schema must expose typed symbol_id: {schema_text}"
    );
}

#[test]
fn symbol_id_round_trips_through_compute_response() {
    let symbol_id = SymbolIdV1::from_parts("src/lib.rs", "some_fn");
    let params = AicxParams {
        scope: "symbol".to_string(),
        target: Some("some_fn".to_string()),
        symbol_id: Some(symbol_id.clone()),
        ..AicxParams::default()
    };
    let snapshot = Snapshot::new(vec![".".to_string()]);

    let resp = compute(&snapshot, &params, None);

    assert_eq!(resp.symbol_id.as_ref(), Some(&symbol_id));
    assert_eq!(resp.symbol_id_version, SymbolIdV1::VERSION);
}

#[test]
fn dedups_source_chunks_across_entries() {
    let entries = vec![
        entry("decision", AuthorityLabel::AicxOperator),
        entry("decision", AuthorityLabel::AicxOperator), // same source_chunk
        entry("outcome", AuthorityLabel::AicxAgent),
    ];
    let (wire, chunks) = filter_and_project(&entries, &None);
    assert_eq!(wire.len(), 3);
    assert_eq!(chunks.len(), 2, "duplicate source chunks should dedup");
}

#[test]
fn symbol_scope_echoes_typed_symbol_id() {
    let symbol_id = SymbolIdV1::from_parts("src/lib.rs", "some_fn");
    let params = AicxParams {
        scope: "symbol".to_string(),
        target: Some("some_fn".to_string()),
        symbol_id: Some(symbol_id.clone()),
        ..AicxParams::default()
    };

    let snapshot = Snapshot::new(vec![".".to_string()]);
    let response = compute(&snapshot, &params, None);

    assert_eq!(response.symbol_id, Some(symbol_id));
}
