//! Integration tests for the codeLens importer-counts provider (Plan 03).
//!
//! Title-only tests live as unit tests next to the module. These cover
//! the end-to-end emission path: build a snapshot, ask
//! [`code_lens_for_file`] for lenses, assert the wire shape v1
//! advertises (title rendered, command-string empty, position zero-
//! based, count formatting matching the closure contract).
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use loctree::snapshot::{GraphEdge, Snapshot, project_cache_dir};
use loctree::types::{ExportSymbol, FileAnalysis};
use loctree_lsp::{SnapshotState, code_lens_for_file, format_title};
use std::path::Path;
use tempfile::TempDir;

#[test]
fn zero_count_says_unused() {
    assert_eq!(format_title(0), "unused (0 importers)");
}

#[test]
fn one_count_uses_singular() {
    assert_eq!(format_title(1), "1 importer");
}

#[test]
fn many_count_uses_plural() {
    assert_eq!(format_title(2), "2 importers");
    assert_eq!(format_title(57), "57 importers");
    assert_eq!(format_title(1000), "1000 importers");
}

#[test]
fn large_count_does_not_panic() {
    // No grouping separator — keeps the lens compact in narrow editors.
    let title = format_title(123_456_789);
    assert!(title.contains("123456789"));
    assert!(title.contains("importers"));
}

fn build_export(name: &str, line: usize) -> ExportSymbol {
    ExportSymbol {
        name: name.to_string(),
        kind: "function".to_string(),
        export_type: "named".to_string(),
        line: Some(line),
        params: Vec::new(),

        symbol_id: ::loctree::types::SymbolIdV1::default(),
    }
}

fn cleanup_cache(root: &Path) {
    let cache_dir = project_cache_dir(root);
    let _ = std::fs::remove_dir_all(cache_dir);
}

/// End-to-end: snapshot with an exporter and an importer → lens emits
/// `"1 importer"` at the correct zero-based line, with title-carrier
/// command shape (empty `command.command`).
#[tokio::test]
async fn emits_lens_for_exporter_with_one_importer() {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path();

    let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
    snapshot.files = vec![
        FileAnalysis {
            path: "src/util.rs".to_string(),
            exports: vec![build_export("helper", 12)],
            ..Default::default()
        },
        FileAnalysis {
            path: "src/main.rs".to_string(),
            ..Default::default()
        },
    ];
    snapshot.edges = vec![GraphEdge {
        from: "src/main.rs".to_string(),
        to: "src/util.rs".to_string(),
        label: "helper".to_string(),
    }];
    snapshot.save(root).expect("save snapshot");

    let state = SnapshotState::new();
    state.load(root).await.expect("load snapshot");

    let lenses = code_lens_for_file(&state, "src/util.rs").await;
    assert_eq!(lenses.len(), 1, "expected one lens");
    let lens = &lenses[0];

    // Plan 03 contract: line is zero-based (export.line was 12 → 11).
    assert_eq!(lens.range.start.line, 11);
    assert_eq!(lens.range.end.line, 11);

    // v1 contract: title-carrier command (closure decision — see code_lens.rs).
    let cmd = lens.command.as_ref().expect("title carrier present");
    assert_eq!(cmd.title, "1 importer");
    assert_eq!(cmd.command, "");
    assert!(cmd.arguments.is_none());

    cleanup_cache(root);
}

/// File with multiple exports, varied importer counts → lens count
/// matches `format_title()` for each.
#[tokio::test]
async fn emits_lenses_for_each_top_level_export() {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path();

    let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
    snapshot.files = vec![
        FileAnalysis {
            path: "src/api.rs".to_string(),
            exports: vec![
                build_export("widely_used", 10),
                build_export("rarely_used", 25),
                build_export("dead", 40),
            ],
            ..Default::default()
        },
        FileAnalysis {
            path: "src/a.rs".to_string(),
            ..Default::default()
        },
        FileAnalysis {
            path: "src/b.rs".to_string(),
            ..Default::default()
        },
        FileAnalysis {
            path: "src/c.rs".to_string(),
            ..Default::default()
        },
    ];
    snapshot.edges = vec![
        GraphEdge {
            from: "src/a.rs".to_string(),
            to: "src/api.rs".to_string(),
            label: "widely_used".to_string(),
        },
        GraphEdge {
            from: "src/b.rs".to_string(),
            to: "src/api.rs".to_string(),
            label: "widely_used".to_string(),
        },
        GraphEdge {
            from: "src/c.rs".to_string(),
            to: "src/api.rs".to_string(),
            label: "rarely_used".to_string(),
        },
    ];
    snapshot.save(root).expect("save snapshot");

    let state = SnapshotState::new();
    state.load(root).await.expect("load snapshot");

    let lenses = code_lens_for_file(&state, "src/api.rs").await;
    assert_eq!(lenses.len(), 3, "one lens per export");

    // Lenses are emitted in snapshot-export order. Match by position.
    let titles: Vec<String> = lenses
        .iter()
        .map(|l| l.command.as_ref().unwrap().title.clone())
        .collect();
    assert!(titles.contains(&"2 importers".to_string()));
    assert!(titles.contains(&"1 importer".to_string()));
    assert!(titles.contains(&"unused (0 importers)".to_string()));

    cleanup_cache(root);
}

/// Snapshot without the requested file → empty vector, no panic.
#[tokio::test]
async fn missing_file_yields_empty_lenses() {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path();

    let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
    snapshot.files = vec![FileAnalysis {
        path: "src/known.rs".to_string(),
        exports: vec![build_export("foo", 1)],
        ..Default::default()
    }];
    snapshot.save(root).expect("save snapshot");

    let state = SnapshotState::new();
    state.load(root).await.expect("load snapshot");

    let lenses = code_lens_for_file(&state, "src/never_seen.rs").await;
    assert!(lenses.is_empty());

    cleanup_cache(root);
}

/// SnapshotState that has never been loaded → empty vector, no panic.
#[tokio::test]
async fn empty_snapshot_state_is_safe() {
    let state = SnapshotState::new();
    let lenses = code_lens_for_file(&state, "src/anything.rs").await;
    assert!(lenses.is_empty());
}

/// Export reported with `line: None` is skipped — there is nothing to
/// pin the lens to.
#[tokio::test]
async fn export_without_line_is_skipped() {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path();

    let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
    snapshot.files = vec![FileAnalysis {
        path: "src/anon.rs".to_string(),
        exports: vec![ExportSymbol {
            name: "headless".to_string(),
            kind: "function".to_string(),
            export_type: "named".to_string(),
            line: None,
            params: Vec::new(),

            symbol_id: ::loctree::types::SymbolIdV1::default(),
        }],
        ..Default::default()
    }];
    snapshot.save(root).expect("save snapshot");

    let state = SnapshotState::new();
    state.load(root).await.expect("load snapshot");

    let lenses = code_lens_for_file(&state, "src/anon.rs").await;
    assert!(lenses.is_empty());

    cleanup_cache(root);
}
