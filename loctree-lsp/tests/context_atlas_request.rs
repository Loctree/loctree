//! Integration test for the `loctree/contextAtlas` custom LSP request (Plan 02).
//!
//! Covers params parsing, manifest-on-disk → response mapping, the
//! `missing` zero-state, and the response-shape contract.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::path::PathBuf;

use loctree_lsp::{ContextAtlasParams, ContextAtlasResponse, context_atlas};
use serde_json::json;

#[test]
fn params_deserialize_minimal() {
    let value = json!({});
    let params: ContextAtlasParams = serde_json::from_value(value).expect("minimal parse");
    assert!(params.project.is_none());
}

#[test]
fn params_deserialize_with_project_override() {
    let value = json!({ "project": "/abs/repo" });
    let params: ContextAtlasParams = serde_json::from_value(value).expect("full parse");
    assert_eq!(
        params
            .project
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        Some("/abs/repo".into())
    );
}

#[test]
fn missing_response_carries_next_action() {
    let temp = tempfile::tempdir().unwrap();
    let response = context_atlas::compute(temp.path(), &ContextAtlasParams::default());
    assert_eq!(response.status, "missing");
    assert_eq!(response.next_action.as_deref(), Some("loct auto"));
    assert!(response.cards.is_empty());
}

#[test]
fn ready_response_lists_cards_in_reading_order() {
    let temp = tempfile::tempdir().unwrap();
    let manifest_dir = temp.path().join(".loctree/context-atlas");
    std::fs::create_dir_all(&manifest_dir).unwrap();
    let manifest = json!({
        "atlas_dir": "/abs/.loctree/context-atlas",
        "manifest": "/abs/.loctree/context-atlas/manifest.md",
        "manifest_json": "/abs/.loctree/context-atlas/manifest.json",
        "recommended_start": "/abs/.loctree/context-atlas/00-core-map.md",
        "cards": [
            { "id": "core",       "title": "Core Map",       "path": "00-core-map.md",       "lines": 226, "why": "Repo identity" },
            { "id": "structural", "title": "Structural Map", "path": "01-structural-map.md", "lines": 20,  "why": "Files + symbols" },
            { "id": "runtime",    "title": "Runtime Map",    "path": "02-runtime-map.md",    "lines": 22,  "why": "Runtime hints" }
        ],
        "message": "Atlas ready — read core, structural, runtime first."
    });
    std::fs::write(
        manifest_dir.join("manifest.json"),
        serde_json::to_string(&manifest).unwrap(),
    )
    .unwrap();

    let response = context_atlas::compute(temp.path(), &ContextAtlasParams::default());
    assert_eq!(response.status, "ready");
    assert!(response.next_action.is_none());
    assert_eq!(response.cards.len(), 3);
    let order: Vec<&str> = response.cards.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(order, ["core", "structural", "runtime"]);
}

#[test]
fn project_override_wins_over_workspace_root() {
    let workspace = tempfile::tempdir().unwrap();
    let other_project = tempfile::tempdir().unwrap();

    // workspace has an atlas; other_project does not.
    let manifest_dir = workspace.path().join(".loctree/context-atlas");
    std::fs::create_dir_all(&manifest_dir).unwrap();
    std::fs::write(
        manifest_dir.join("manifest.json"),
        serde_json::to_string(&json!({
            "atlas_dir": "/x", "manifest": "/x/m.md", "manifest_json": "/x/m.json",
            "recommended_start": "/x/00.md", "cards": [], "message": "ok"
        }))
        .unwrap(),
    )
    .unwrap();

    // Override project → other_project (no atlas) → response should be missing.
    let params = ContextAtlasParams {
        project: Some(other_project.path().to_path_buf()),
    };
    let response = context_atlas::compute(workspace.path(), &params);
    assert_eq!(response.status, "missing");
}

#[test]
fn manifest_path_layout_matches_plan_01() {
    let path = context_atlas::manifest_path_for(&PathBuf::from("/repo"));
    assert_eq!(
        path,
        PathBuf::from("/repo/.loctree/context-atlas/manifest.json")
    );
}

#[test]
fn response_serializes_with_only_populated_fields() {
    let response = ContextAtlasResponse {
        status: "ready".into(),
        atlas_dir: Some("/x".into()),
        manifest: Some("/x/m.md".into()),
        manifest_json: Some("/x/m.json".into()),
        recommended_start: Some("/x/00.md".into()),
        cards: vec![],
        message: "ready".into(),
        next_action: None,
    };
    let json = serde_json::to_value(&response).unwrap();
    let obj = json.as_object().unwrap();
    assert_eq!(obj["status"], serde_json::json!("ready"));
    assert!(obj.contains_key("atlas_dir"));
    assert!(obj.contains_key("manifest"));
    // next_action is None on `ready` → must NOT appear.
    assert!(!obj.contains_key("next_action"));
}
