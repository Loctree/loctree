//! Integration tests for the `loctree/contextPack` custom LSP request.

use std::fs;
use std::path::Path;

use loctree::args::ParsedArgs;
use loctree::snapshot::Snapshot;
use loctree_lsp::{ContextPackParams, context_pack};
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn params_deserialize_minimal() {
    let params: ContextPackParams =
        serde_json::from_value(serde_json::json!({})).expect("minimal contextPack params");
    assert!(params.project.is_none());
    assert!(params.cursor.is_none());
    assert!(params.cards.is_none());
    assert!(params.scope.is_none());
    assert!(params.task.is_none());
    assert_eq!(params.with_aicx, None);
    assert!(!params.no_aicx);
}

#[test]
fn params_deserialize_full() {
    let params: ContextPackParams = serde_json::from_value(serde_json::json!({
        "project": "/abs/repo",
        "cursor": "v1.cursor",
        "cards": ["core", "risk"],
        "scope": ["path:src"],
        "task": "explain parser",
        "with_aicx": true,
        "no_aicx": false
    }))
    .expect("full contextPack params");

    assert_eq!(
        params
            .project
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        Some("/abs/repo".into())
    );
    assert_eq!(params.cursor.as_deref(), Some("v1.cursor"));
    assert_eq!(
        params.cards.as_deref(),
        Some(&["core".into(), "risk".into()][..])
    );
    assert_eq!(params.scope.as_deref(), Some(&["path:src".into()][..]));
    assert_eq!(params.task.as_deref(), Some("explain parser"));
    assert_eq!(params.with_aicx, Some(true));
}

#[test]
fn context_pack_walks_all_six_cards() {
    let project = sample_project();
    let snapshot = scan(project.path());

    let mut cursor = None;
    let mut cards = Vec::new();
    loop {
        let response = context_pack::compute(
            project.path(),
            &snapshot,
            &ContextPackParams {
                cursor: cursor.clone(),
                ..ContextPackParams::default()
            },
        )
        .expect("contextPack page");
        cards.push(response.card);
        cursor = response.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    assert_eq!(
        cards,
        vec![
            "core",
            "structural",
            "runtime",
            "memory",
            "verification",
            "risk"
        ]
    );
}

#[test]
fn context_pack_cards_filter_skips_intermediate_cards() {
    let project = sample_project();
    let snapshot = scan(project.path());

    let first = context_pack::compute(
        project.path(),
        &snapshot,
        &ContextPackParams {
            cards: Some(vec!["core".into(), "risk".into()]),
            ..ContextPackParams::default()
        },
    )
    .expect("first page");
    assert_eq!(first.section, 0);
    assert_eq!(first.card, "core");
    assert_eq!(first.total_sections, 2);

    let second = context_pack::compute(
        project.path(),
        &snapshot,
        &ContextPackParams {
            cursor: first.next_cursor,
            ..ContextPackParams::default()
        },
    )
    .expect("second page");
    assert_eq!(second.section, 1);
    assert_eq!(second.card, "risk");
    assert!(second.next_cursor.is_none());
}

#[test]
fn context_pack_response_includes_routed_identity() {
    let project = sample_project();
    let snapshot = scan(project.path());
    let requested_project = project.path().join(".");

    let response = context_pack::compute(
        project.path(),
        &snapshot,
        &ContextPackParams {
            project: Some(requested_project.clone()),
            ..ContextPackParams::default()
        },
    )
    .expect("contextPack page");

    assert_eq!(
        response.identity.requested_project.as_deref(),
        Some(requested_project.to_string_lossy().as_ref())
    );
    assert_eq!(
        response.identity.resolved_project,
        project.path().canonicalize().unwrap().to_string_lossy()
    );
    assert!(
        !response.identity.snapshot_id.is_empty(),
        "contextPack identity should expose the atlas fingerprint as snapshot id"
    );
}

#[test]
fn context_pack_request_core_page_preserves_suggested_next_action_path() {
    let project = sample_project();
    let snapshot = scan(project.path());

    let response = context_pack::compute(
        project.path(),
        &snapshot,
        &ContextPackParams {
            cards: Some(vec!["core".into()]),
            scope: Some(vec!["path:src/lib.rs".into()]),
            ..ContextPackParams::default()
        },
    )
    .expect("contextPack core page");

    assert_eq!(response.card, "core");
    assert!(
        response.content.contains("\"power_path\""),
        "contextPack should preserve action/suggested-next guidance: {}",
        response.content
    );
    assert!(
        response.content.contains("loct slice src/lib.rs"),
        "contextPack suggested-next guidance should include executable loct commands: {}",
        response.content
    );
}

#[test]
fn context_pack_returns_gone_when_atlas_fingerprint_changes_mid_cursor() {
    let project = sample_project();
    let snapshot = scan(project.path());

    let first = context_pack::compute(project.path(), &snapshot, &ContextPackParams::default())
        .expect("first page");
    let cursor = first.next_cursor.expect("next cursor");

    let manifest_path = project.path().join(".loctree/context-atlas/manifest.json");
    let mut manifest: Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).expect("manifest"))
            .expect("manifest json");
    manifest["generated_at"] = Value::String("2099-01-01T00:00:00Z".to_string());
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).expect("manifest serialize"),
    )
    .expect("rewrite manifest");

    let err = context_pack::compute(
        project.path(),
        &snapshot,
        &ContextPackParams {
            cursor: Some(cursor),
            ..ContextPackParams::default()
        },
    )
    .expect_err("fingerprint mismatch must fail");
    assert_eq!(err.kind(), "gone");
}

fn sample_project() -> TempDir {
    let tmp = TempDir::new().expect("temp dir");
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"context-pack-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write Cargo.toml");
    fs::create_dir_all(tmp.path().join("src")).expect("src dir");
    fs::write(
        tmp.path().join("src/lib.rs"),
        "pub fn alpha() -> &'static str { beta() }\nfn beta() -> &'static str { \"beta\" }\n",
    )
    .expect("write lib.rs");
    tmp
}

fn scan(project: &Path) -> Snapshot {
    let parsed = ParsedArgs {
        ignore_patterns: loctree::fs_utils::load_loctreeignore(project),
        ..ParsedArgs::default()
    };
    loctree::snapshot::run_init_with_options(&[project.to_path_buf()], &parsed, true)
        .expect("scan");
    Snapshot::load(project).expect("snapshot")
}
