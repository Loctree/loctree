//! Integration tests for Plan 13 — multi-workspace context routing.
//!
//! These cover the discovery surface (`discover_loctree_dirs`,
//! `max_depth_from_options`) and the wire-shape contract for
//! `loctree/workspaces`. End-to-end Backend tests that drive the LSP
//! over JSON-RPC live in the daemon smoke harness; these ensure the
//! pure functions and the WorkspaceInfo serialization are stable.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use loctree::snapshot::Snapshot;
use loctree::types::FileAnalysis;
use loctree_lsp::{
    WorkspaceInfo, WorkspacesResponse, discover_loctree_dirs, max_depth_from_options,
};
use serde_json::json;
use tempfile::TempDir;

/// Vista monorepo shape: root + apps/web + apps/api/src + packages/ui.
///
/// Each `.loctree/` carries a `snapshot.json` marker so discovery
/// treats the parent as a real addressable sub-project. Empty
/// `.loctree/` markers are intentionally skipped — that contract is
/// covered by `discover_skips_empty_loctree_marker_without_snapshot`
/// in the workspaces module unit tests.
fn fake_monorepo() -> TempDir {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path();
    for parent in [
        root.to_path_buf(),
        root.join("apps/web"),
        root.join("apps/api/src-tauri"),
        root.join("packages/ui"),
        // Noise that should NOT be discovered (pruned by name):
        root.join("node_modules/foo"),
        root.join("target/release"),
        root.join(".git/refs"),
    ] {
        let dir = parent.join(".loctree");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("snapshot.json"), b"{}").unwrap();
    }
    temp
}

#[test]
fn discover_finds_three_subprojects_in_monorepo() {
    let temp = fake_monorepo();
    let found = discover_loctree_dirs(temp.path(), 4);
    assert_eq!(
        found.len(),
        3,
        "expected web, src-tauri, ui — got {:?}",
        found
    );
}

#[test]
fn discover_excludes_root_from_extras() {
    let temp = fake_monorepo();
    let canonical_root = temp
        .path()
        .canonicalize()
        .unwrap_or_else(|_| temp.path().to_path_buf());
    let found = discover_loctree_dirs(temp.path(), 4);
    assert!(
        !found.iter().any(|p| p == &canonical_root),
        "root must be addressed via the dedicated handle, not extras"
    );
}

#[test]
fn discover_prunes_node_modules_and_target_and_git() {
    let temp = fake_monorepo();
    let found = discover_loctree_dirs(temp.path(), 4);
    let labels: Vec<String> = found.iter().map(|p| p.display().to_string()).collect();
    for label in &labels {
        assert!(
            !label.contains("node_modules"),
            "node_modules subtree must be pruned: {label}"
        );
        assert!(
            !label.contains("target/release"),
            "target/release subtree must be pruned: {label}"
        );
        assert!(
            !label.contains(".git"),
            ".git subtree must be pruned: {label}"
        );
    }
}

#[test]
fn max_depth_overrides_via_init_options() {
    let nested = json!({"loctree": {"workspaces": {"maxDepth": 5}}});
    assert_eq!(max_depth_from_options(Some(&nested)), 5);

    let flat = json!({"loctree.workspaces.maxDepth": 3});
    assert_eq!(max_depth_from_options(Some(&flat)), 3);

    assert_eq!(max_depth_from_options(None), 4);
}

#[test]
fn workspace_info_serializes_to_expected_shape() {
    // The wire shape is what agents pin against — guard the JSON keys
    // explicitly so a future refactor doesn't silently rename them.
    let info = WorkspaceInfo {
        root: "/repo/apps/web".to_string(),
        is_root: false,
        has_snapshot: true,
        files: 42,
        languages: vec!["rust".into(), "typescript".into()],
        snapshot_age_seconds: Some(123),
    };
    let json = serde_json::to_value(&info).unwrap();
    assert_eq!(json["root"], "/repo/apps/web");
    assert_eq!(json["is_root"], false);
    assert_eq!(json["has_snapshot"], true);
    assert_eq!(json["files"], 42);
    assert_eq!(json["languages"], json!(["rust", "typescript"]));
    assert_eq!(json["snapshot_age_seconds"], 123);
}

#[test]
fn workspaces_response_serializes_root_first_then_subs() {
    let response = WorkspacesResponse {
        workspaces: vec![
            WorkspaceInfo {
                root: "/repo".into(),
                is_root: true,
                has_snapshot: true,
                files: 100,
                languages: vec!["rust".into()],
                snapshot_age_seconds: Some(60),
            },
            WorkspaceInfo {
                root: "/repo/apps/web".into(),
                is_root: false,
                has_snapshot: true,
                files: 30,
                languages: vec!["typescript".into()],
                snapshot_age_seconds: Some(120),
            },
        ],
    };
    let json = serde_json::to_value(&response).unwrap();
    let arr = json["workspaces"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["is_root"], true);
    assert_eq!(arr[1]["is_root"], false);
}

/// A real snapshot saved into a sub-project's `.loctree/` should be
/// loadable independently — proves Plan 13's per-workspace ownership.
#[tokio::test]
async fn subproject_snapshot_loads_independently() {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path();
    let sub = root.join("apps/web");
    std::fs::create_dir_all(sub.join(".loctree")).unwrap();

    // Save a sub-project snapshot through the canonical Save API so
    // it lands in the global cache keyed off `sub`.
    let mut snapshot = Snapshot::new(vec![sub.display().to_string()]);
    snapshot.files = vec![FileAnalysis {
        path: "src/lib.rs".into(),
        ..Default::default()
    }];
    snapshot.save(&sub).expect("save sub snapshot");

    let loaded = Snapshot::load(&sub).expect("load sub snapshot");
    assert_eq!(loaded.files.len(), 1);
    assert_eq!(loaded.files[0].path, "src/lib.rs");

    // Cleanup global cache for the sub root to keep CI tidy.
    let _ = std::fs::remove_dir_all(loctree::snapshot::project_cache_dir(&sub));
}
