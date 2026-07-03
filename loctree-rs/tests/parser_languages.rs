//! E2E tests for v0.9.0 lightweight parsers (shell / make / zig).
//!
//! Each test copies the fixture tree into a TempDir so that
//! `Snapshot::find_loctree_root` doesn't walk up into the parent loctree
//! repository's own snapshot. This mirrors `e2e_cli.rs::creates_snapshot`.

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use loctree::slicer::{HolographicSlice, SliceConfig};
use loctree::snapshot::Snapshot;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn fixtures_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn loct() -> Command {
    cargo_bin_cmd!("loct")
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), dest_path)?;
        }
    }
    Ok(())
}

/// Copy fixture to a tempdir, scan it via `loct`, and load the snapshot.
fn scan_fixture_in_tempdir(fixture_name: &str) -> (TempDir, Snapshot) {
    let temp = TempDir::new().expect("tempdir");
    let fixture = fixtures_path().join(fixture_name);
    copy_dir_all(&fixture, temp.path()).expect("copy fixture");

    let output = loct().current_dir(temp.path()).output().expect("loct runs");
    assert!(
        output.status.success(),
        "loct failed in {}: stderr={}",
        temp.path().display(),
        String::from_utf8_lossy(&output.stderr)
    );

    let loctree_root = Snapshot::find_loctree_root(temp.path()).expect("loctree root after scan");
    let snapshot = Snapshot::load(&loctree_root).expect("load snapshot");
    (temp, snapshot)
}

fn languages_in(snapshot: &Snapshot) -> Vec<String> {
    let mut langs: Vec<String> = snapshot
        .files
        .iter()
        .map(|f| f.language.clone())
        .filter(|l| !l.is_empty())
        .collect();
    langs.sort();
    langs.dedup();
    langs
}

#[test]
fn shell_project_is_analyzed() {
    let (_temp, snapshot) = scan_fixture_in_tempdir("shell_project");

    assert!(
        snapshot.files.len() >= 3,
        "expected >= 3 shell files, got {}",
        snapshot.files.len()
    );

    let langs = languages_in(&snapshot);
    assert!(
        langs.iter().any(|l| l == "shell"),
        "expected shell language; got {:?}",
        langs
    );
}

#[test]
fn makefile_project_is_analyzed() {
    let (_temp, snapshot) = scan_fixture_in_tempdir("makefile_project");

    assert!(
        snapshot.files.len() >= 2,
        "expected >= 2 make files, got {}",
        snapshot.files.len()
    );

    let langs = languages_in(&snapshot);
    assert!(
        langs.iter().any(|l| l == "make"),
        "expected make language; got {:?}",
        langs
    );

    // Makefile (no extension) must be classified as make via filename-based
    // fallback — this is the primary user-visible signal.
    let has_makefile = snapshot
        .files
        .iter()
        .any(|f| f.path.ends_with("Makefile") && f.language == "make");
    assert!(
        has_makefile,
        "Makefile not classified as make; files={:?}",
        snapshot
            .files
            .iter()
            .map(|f| (f.path.clone(), f.language.clone()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn zig_project_is_analyzed() {
    let (_temp, snapshot) = scan_fixture_in_tempdir("zig_project");

    assert!(
        snapshot.files.len() >= 2,
        "expected >= 2 zig files, got {}",
        snapshot.files.len()
    );

    let langs = languages_in(&snapshot);
    assert!(
        langs.iter().any(|l| l == "zig"),
        "expected zig language; got {:?}",
        langs
    );
}

#[test]
fn kotlin_project_is_visible_scan_only() {
    let (_temp, snapshot) = scan_fixture_in_tempdir("kotlin_project");

    let kotlin_files: Vec<_> = snapshot
        .files
        .iter()
        .filter(|f| f.language == "kotlin")
        .collect();
    assert!(
        kotlin_files.len() >= 2,
        "expected Kotlin .kt/.kts files in snapshot, got {:?}",
        snapshot
            .files
            .iter()
            .map(|f| (f.path.clone(), f.language.clone()))
            .collect::<Vec<_>>()
    );

    assert!(
        kotlin_files
            .iter()
            .all(|f| f.imports.is_empty() && f.exports.is_empty()),
        "Kotlin is scan-only until a parser lands; got {:?}",
        kotlin_files
    );
}

#[test]
fn resource_files_have_first_class_membership() {
    let (_temp, snapshot) = scan_fixture_in_tempdir("resource_membership");

    let file = |path: &str| {
        snapshot
            .files
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| {
                panic!(
                    "expected {path} in snapshot; got {:?}",
                    snapshot
                        .files
                        .iter()
                        .map(|f| (f.path.clone(), f.language.clone(), f.kind.clone()))
                        .collect::<Vec<_>>()
                )
            })
    };

    let readme = file("README.md");
    assert_eq!(readme.language, "md");
    assert_eq!(readme.kind, "doc");
    assert_eq!(readme.resource_kind.as_deref(), Some("doc"));

    let cargo = file("Cargo.toml");
    assert_eq!(cargo.language, "toml");
    assert_eq!(cargo.kind, "config");
    assert_eq!(cargo.resource_kind.as_deref(), Some("config"));

    let workflow = file(".github/workflows/ci.yml");
    assert_eq!(workflow.language, "yml");
    assert_eq!(workflow.kind, "workflow");
    assert_eq!(workflow.resource_kind.as_deref(), Some("workflow"));

    let locale = file("locales/en.json");
    assert_eq!(locale.language, "json");
    assert_eq!(locale.kind, "locale");
    assert_eq!(locale.resource_kind.as_deref(), Some("locale"));

    let resource = file("data/schema.json");
    assert_eq!(resource.language, "json");
    assert_eq!(resource.kind, "resource");
    assert_eq!(resource.resource_kind.as_deref(), Some("resource"));

    let storyboard = file("Base.lproj/Main.storyboard");
    assert_eq!(storyboard.language, "storyboard");
    assert_eq!(storyboard.kind, "resource");
    assert_eq!(storyboard.resource_kind.as_deref(), Some("resource"));

    let slice = HolographicSlice::from_path(
        &snapshot,
        ".github/workflows/ci.yml",
        &SliceConfig::default(),
    )
    .expect("workflow resource is sliceable");
    let core = slice.core.first().expect("slice core");
    assert_eq!(core.path, ".github/workflows/ci.yml");
    assert_eq!(core.kind, "workflow");
    assert_eq!(core.resource_kind.as_deref(), Some("workflow"));

    let json = slice.to_json();
    assert_eq!(json["core"][0]["kind"], "workflow");
    assert_eq!(json["core"][0]["resource_kind"], "workflow");

    let storyboard_slice = HolographicSlice::from_path(
        &snapshot,
        "Base.lproj/Main.storyboard",
        &SliceConfig::default(),
    )
    .expect("storyboard resource is sliceable");
    let storyboard_core = storyboard_slice
        .core
        .first()
        .expect("storyboard slice core");
    assert_eq!(storyboard_core.path, "Base.lproj/Main.storyboard");
    assert_eq!(storyboard_core.kind, "resource");
    assert_eq!(storyboard_core.resource_kind.as_deref(), Some("resource"));
}

/// Shell fixture has `source ./common.sh` edges from both install.sh and
/// utils.sh — at least one must resolve to common.sh.
#[test]
fn shell_source_edges_resolved() {
    let (_temp, snapshot) = scan_fixture_in_tempdir("shell_project");

    let has_common_import = snapshot.files.iter().any(|f| {
        f.language == "shell"
            && f.imports.iter().any(|i| {
                i.resolved_path
                    .as_ref()
                    .map(|p| p.to_ascii_lowercase().ends_with("common.sh"))
                    .unwrap_or(false)
            })
    });
    assert!(
        has_common_import,
        "expected resolved import to common.sh; imports={:?}",
        snapshot
            .files
            .iter()
            .map(|f| (f.path.clone(), f.imports.clone()))
            .collect::<Vec<_>>()
    );
}

/// Makefile `include common.mk` should produce at least one resolved edge.
#[test]
fn makefile_include_edges_resolved() {
    let (_temp, snapshot) = scan_fixture_in_tempdir("makefile_project");

    let has_common_mk_import = snapshot.files.iter().any(|f| {
        f.language == "make"
            && f.imports.iter().any(|i| {
                i.resolved_path
                    .as_ref()
                    .map(|p| p.to_ascii_lowercase().ends_with("common.mk"))
                    .unwrap_or(false)
            })
    });
    assert!(
        has_common_mk_import,
        "expected resolved include of common.mk; imports={:?}",
        snapshot
            .files
            .iter()
            .map(|f| (f.path.clone(), f.imports.clone()))
            .collect::<Vec<_>>()
    );
}

/// Zig `@import("helpers.zig")` should resolve to a local path.
#[test]
fn zig_import_edges_resolved() {
    let (_temp, snapshot) = scan_fixture_in_tempdir("zig_project");

    let has_helpers_import = snapshot.files.iter().any(|f| {
        f.language == "zig"
            && f.imports.iter().any(|i| {
                i.resolved_path
                    .as_ref()
                    .map(|p| p.to_ascii_lowercase().ends_with("helpers.zig"))
                    .unwrap_or(false)
            })
    });
    assert!(
        has_helpers_import,
        "expected resolved @import(helpers.zig); imports={:?}",
        snapshot
            .files
            .iter()
            .map(|f| (f.path.clone(), f.imports.clone()))
            .collect::<Vec<_>>()
    );
}
