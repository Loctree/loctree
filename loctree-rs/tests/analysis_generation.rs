//! Derived facts must cross analyzer upgrades through the shared acquisition authority.

use assert_cmd::cargo::cargo_bin_cmd;
use git2::{Repository, Signature};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;
use tempfile::TempDir;

fn fixture() -> (TempDir, TempDir) {
    let root = TempDir::new().expect("project");
    let cache = TempDir::new().expect("cache");
    for (name, source) in [
        (
            "a.ts",
            "import type { B } from './b.ts'; export interface A { b: B }",
        ),
        (
            "b.ts",
            "import type { A } from './a.ts'; export interface B { a: A }",
        ),
    ] {
        std::fs::write(root.path().join(name), source).expect("source");
    }
    let repo = Repository::init(root.path()).expect("git init");
    let mut index = repo.index().expect("index");
    for name in ["a.ts", "b.ts"] {
        index.add_path(Path::new(name)).expect("add");
    }
    let tree = repo
        .find_tree(index.write_tree().expect("tree id"))
        .expect("tree");
    let sig = Signature::now("Fixture", "fixture@example.invalid").expect("signature");
    repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree, &[])
        .expect("commit");
    (root, cache)
}

fn run(root: &Path, cache: &Path, args: &[&str]) -> Output {
    cargo_bin_cmd!("loct")
        .current_dir(root)
        .env("LOCT_CACHE_DIR", cache)
        .env("LOCT_NO_GITIGNORE", "1")
        .args(args)
        .output()
        .expect("CLI")
}

fn json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI JSON")
}

// Include every cache file, not only snapshot JSON: refusal must not refresh fences,
// migrate legacy data, create lock files, or touch report artifacts.
fn contents(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = BTreeMap::new();
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).expect("entries") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(path.clone(), std::fs::read(path).expect("bytes"));
            }
        }
    }
    files
}

fn rewrite_snapshots(cache: &Path, mut edit: impl FnMut(&mut Value)) {
    let mut count = 0;
    for path in contents(cache)
        .keys()
        .filter(|p| p.file_name().is_some_and(|n| n == "snapshot.json"))
    {
        let mut value: Value =
            serde_json::from_slice(&std::fs::read(path).expect("snapshot")).expect("JSON");
        edit(&mut value);
        std::fs::write(path, value.to_string()).expect("replace cache");
        count += 1;
    }
    assert!(count > 0);
}

#[test]
fn unknown_generation_rebuilds_raw_facts_before_relabeling_edges() {
    let (root, cache) = fixture();
    let edges = json(run(root.path(), cache.path(), &["[.edges[]]"]));
    assert_eq!(edges.as_array().expect("edges").len(), 2);
    rewrite_snapshots(cache.path(), |s| {
        s["metadata"]
            .as_object_mut()
            .expect("metadata")
            .remove("analysis_generation");
        for file in s["files"].as_array_mut().expect("files") {
            for import in file["imports"].as_array_mut().expect("imports") {
                import["type_only"] = false.into();
            }
            file["exports"] = serde_json::json!([]);
        }
        for edge in s["edges"].as_array_mut().expect("edges") {
            edge["label"] = "import".into();
        }
    });
    let cycles = json(run(root.path(), cache.path(), &["cycles", "--json"]));
    assert_eq!(
        cycles["classifiedCycles"].as_array().expect("cycles").len(),
        0
    );
    let rebuilt = json(run(root.path(), cache.path(), &["[.edges[]]"]));
    assert_eq!(rebuilt, edges);
    let symbols = json(run(
        root.path(),
        cache.path(),
        &["[.files[].exports[].name]"],
    ));
    assert!(
        symbols
            .as_array()
            .expect("exports")
            .contains(&Value::from("A"))
    );
    assert!(
        symbols
            .as_array()
            .expect("exports")
            .contains(&Value::from("B"))
    );
    let before = contents(cache.path());
    json(run(
        root.path(),
        cache.path(),
        &["--no-scan", "--fail-stale", "cycles", "--json"],
    ));
    assert_eq!(
        contents(cache.path()),
        before,
        "same-generation warm read must reuse"
    );
}

#[test]
fn incompatible_generations_refuse_strict_reads_without_writes() {
    for generation in [
        Value::Null,
        Value::from("older"),
        Value::from("loctree-structural-v1"),
        Value::from("future"),
        Value::from(""),
    ] {
        let (root, cache) = fixture();
        json(run(root.path(), cache.path(), &["cycles", "--json"]));
        rewrite_snapshots(cache.path(), |s| {
            if generation.is_null() {
                s["metadata"]
                    .as_object_mut()
                    .expect("metadata")
                    .remove("analysis_generation");
            } else {
                s["metadata"]["analysis_generation"] = generation.clone();
            }
        });
        let before = contents(cache.path());
        for flags in [
            vec!["--no-scan"],
            vec!["--fail-stale"],
            vec!["--no-scan", "--fail-stale"],
        ] {
            let args = [flags, vec!["cycles", "--json"]].concat();
            let out = run(root.path(), cache.path(), &args);
            assert!(!out.status.success(), "must refuse {generation}");
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("analysis generation"),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(contents(cache.path()), before);
            assert!(!root.path().join(".loctree").exists());
        }
    }
}

#[test]
fn compatible_schema_patch_and_producer_changes_do_not_invalidate_analysis() {
    let (root, cache) = fixture();
    json(run(root.path(), cache.path(), &["cycles", "--json"]));
    rewrite_snapshots(cache.path(), |s| {
        s["metadata"]["schema_version"] = "0.11.999".into();
        s["metadata"]["producer_version"] = "a-different-cli-or-mcp-build".into();
    });
    let before = contents(cache.path());
    for _ in 0..2 {
        json(run(root.path(), cache.path(), &["cycles", "--json"]));
    }
    assert_eq!(contents(cache.path()), before);
}

#[test]
fn no_scan_legacy_generation_refusal_does_not_migrate_or_create_locks() {
    let (root, cache) = fixture();
    let mut snapshot = loctree::snapshot::Snapshot::new(vec![root.path().display().to_string()]);
    let local = root.path().join(".loctree");
    std::fs::create_dir(&local).expect("legacy directory");
    // Serialize rather than save: this is a legacy local cache with no global bucket or lock.
    snapshot.metadata.git_scan_id = None;
    let mut value = serde_json::to_value(snapshot).expect("snapshot JSON");
    value["metadata"]
        .as_object_mut()
        .expect("metadata")
        .remove("analysis_generation");
    std::fs::write(local.join("snapshot.json"), value.to_string()).expect("legacy snapshot");
    let before = contents(&local);
    let out = run(
        root.path(),
        cache.path(),
        &["--no-scan", "--fail-stale", "cycles", "--json"],
    );
    assert!(!out.status.success());
    assert_eq!(contents(&local), before);
    assert!(contents(cache.path()).is_empty());
}

#[test]
fn strict_flags_cannot_be_overridden_by_fresh() {
    let (root, cache) = fixture();
    json(run(root.path(), cache.path(), &["cycles", "--json"]));
    rewrite_snapshots(cache.path(), |s| {
        s["metadata"]
            .as_object_mut()
            .expect("metadata")
            .remove("analysis_generation");
    });
    let before = contents(cache.path());
    for strict in ["--no-scan", "--fail-stale"] {
        let out = run(
            root.path(),
            cache.path(),
            &[strict, "--fresh", "cycles", "--json"],
        );
        assert!(
            !out.status.success(),
            "strict flag must prevent forced refresh"
        );
        assert_eq!(contents(cache.path()), before);
    }
}
