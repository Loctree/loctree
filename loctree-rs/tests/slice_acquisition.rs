//! Slice must validate its target before acquiring a snapshot and emit one JSON document.
use assert_cmd::cargo::cargo_bin_cmd;
use git2::{Repository, Signature};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use tempfile::TempDir;

fn fixture() -> (TempDir, TempDir) {
    let root = TempDir::new().expect("root");
    let cache = TempDir::new().expect("cache");
    let repo = Repository::init(root.path()).expect("git init");
    std::fs::write(root.path().join("entry.ts"), "export const entry = 1;\n").expect("source");
    let mut index = repo.index().expect("index");
    index.add_path(Path::new("entry.ts")).expect("add source");
    let tree = repo
        .find_tree(index.write_tree().expect("tree id"))
        .expect("tree");
    let signature = Signature::now("Fixture", "fixture@example.invalid").expect("signature");
    repo.commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[])
        .expect("commit");
    (root, cache)
}

fn snapshot_contents(cache: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut directories = vec![cache.to_path_buf()];
    let mut snapshots = BTreeMap::new();
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory).expect("cache entries") {
            let path = entry.expect("cache entry").path();
            if path.is_dir() {
                directories.push(path);
            } else if path.file_name().is_some_and(|name| name == "snapshot.json") {
                snapshots.insert(path.clone(), std::fs::read(path).expect("snapshot bytes"));
            }
        }
    }
    snapshots
}

#[test]
fn cold_no_scan_slice_refuses_without_acquisition() {
    for flags in [vec!["--no-scan"], vec!["--no-scan", "--fail-stale"]] {
        let (root, cache) = fixture();
        let output = cargo_bin_cmd!("loct")
            .current_dir(root.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .env("LOCT_NO_GITIGNORE", "1")
            .args(["slice", "entry.ts", "--json"])
            .args(flags)
            .output()
            .expect("slice");
        assert!(
            !output.status.success(),
            "cold no-scan must refuse acquisition"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("--no-scan"));
        assert!(snapshot_contents(cache.path()).is_empty());
        assert!(!root.path().join(".loctree").exists());
    }
}

#[test]
fn stale_no_scan_slice_refuses_without_refresh() {
    let (root, cache) = fixture();
    let run = |flags: &[&str]| {
        cargo_bin_cmd!("loct")
            .current_dir(root.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .env("LOCT_NO_GITIGNORE", "1")
            .args(["slice", "entry.ts", "--json"])
            .args(flags)
            .output()
            .expect("slice")
    };
    assert!(run(&[]).status.success());
    let before = snapshot_contents(cache.path());
    assert!(!before.is_empty());
    std::fs::write(root.path().join("entry.ts"), "export const changed = 2;\n")
        .expect("dirty source");
    for flags in [vec!["--no-scan"], vec!["--no-scan", "--fail-stale"]] {
        let output = run(&flags);
        assert!(
            !output.status.success(),
            "stale no-scan must refuse refresh"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("drift"));
        assert_eq!(snapshot_contents(cache.path()), before);
    }
    let fresh = run(&["--fresh"]);
    assert!(
        fresh.status.success(),
        "{}",
        String::from_utf8_lossy(&fresh.stderr)
    );
    let _: Value = serde_json::from_slice(&fresh.stdout).expect("fresh slice JSON");
    assert_ne!(snapshot_contents(cache.path()), before);
}

#[test]
fn slice_rejects_traversal_before_cold_acquisition() {
    let (root, cache) = fixture();
    let output = cargo_bin_cmd!("loct")
        .current_dir(root.path())
        .env("LOCT_CACHE_DIR", cache.path())
        .env("LOCT_NO_GITIGNORE", "1")
        .args(["slice", "nested/../entry.ts", "--json"])
        .output()
        .expect("slice");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("'..' component"));
    assert_eq!(
        std::fs::read_dir(cache.path())
            .expect("cache entries")
            .count(),
        0
    );
}

#[test]
fn missing_warm_slice_does_not_refresh_dirty_snapshot() {
    let (root, cache) = fixture();
    let run = |target: &str| {
        cargo_bin_cmd!("loct")
            .current_dir(root.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .env("LOCT_NO_GITIGNORE", "1")
            .args(["slice", target, "--json"])
            .output()
            .expect("slice")
    };
    let first = run("entry.ts");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let bucket = std::fs::read_dir(cache.path().join("projects"))
        .expect("cache entries")
        .filter_map(Result::ok)
        .flat_map(|entry| std::fs::read_dir(entry.path()).expect("project cache"))
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("snapshot.json"))
        .find(|path| path.is_file())
        .expect("snapshot");
    let before = std::fs::read(&bucket).expect("before");
    std::fs::write(root.path().join("entry.ts"), "export const changed = 2;\n")
        .expect("dirty source");
    let missing = run("missing.ts");
    assert!(!missing.status.success());
    assert_eq!(std::fs::read(bucket).expect("after"), before);
    assert!(!String::from_utf8_lossy(&missing.stderr).contains("[DRIFT]"));
}

#[test]
fn rescan_slice_stdout_is_one_json_document() {
    let (root, cache) = fixture();
    let output = cargo_bin_cmd!("loct")
        .current_dir(root.path())
        .env("LOCT_CACHE_DIR", cache.path())
        .env("LOCT_NO_GITIGNORE", "1")
        .args(["slice", "entry.ts", "--rescan", "--json"])
        .output()
        .expect("slice");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("one slice JSON document");
    assert_eq!(value["core"][0]["path"], "entry.ts");
}

#[test]
fn slice_preserves_subdirectory_relative_targets() {
    let (root, cache) = fixture();
    let subdir = root.path().join("nested");
    std::fs::create_dir(&subdir).expect("subdirectory");
    std::fs::write(subdir.join("nested.ts"), "export const nested = 1;\n").expect("source");
    let output = cargo_bin_cmd!("loct")
        .current_dir(&subdir)
        .env("LOCT_CACHE_DIR", cache.path())
        .env("LOCT_NO_GITIGNORE", "1")
        .args(["slice", "nested.ts", "--json"])
        .output()
        .expect("slice");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("slice JSON");
    assert_eq!(value["core"][0]["path"], "nested/nested.ts");
}
