use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn swift_graph_correctness() {
    let mut cmd = Command::cargo_bin("loct").unwrap();
    let root = PathBuf::from("tests/fixtures/cfamily/Pensieve");

    // LCT-003 acceptance: query where-symbol WorkspaceCacheStore
    cmd.current_dir(&root)
        .arg("query")
        .arg("where-symbol")
        .arg("WorkspaceCacheStore")
        .assert()
        .success()
        .stdout(predicate::str::contains("WorkspaceCacheStore.swift"));

    let mut final_class_cmd = Command::cargo_bin("loct").unwrap();
    final_class_cmd
        .current_dir(&root)
        .arg("query")
        .arg("where-symbol")
        .arg("WorkspaceMetadataStore")
        .assert()
        .success()
        .stdout(predicate::str::contains("DocumentCommands.swift"));

    let mut folder_manager_cmd = Command::cargo_bin("loct").unwrap();
    folder_manager_cmd
        .current_dir(&root)
        .arg("query")
        .arg("where-symbol")
        .arg("FolderManager")
        .assert()
        .success()
        .stdout(predicate::str::contains("DocumentCommands.swift"));

    let mut method_cmd = Command::cargo_bin("loct").unwrap();
    method_cmd
        .current_dir(&root)
        .arg("query")
        .arg("where-symbol")
        .arg("closeActiveDocument")
        .assert()
        .success()
        .stdout(predicate::str::contains("DocumentCommands.swift"));

    let mut close_command_cmd = Command::cargo_bin("loct").unwrap();
    close_command_cmd
        .current_dir(&root)
        .arg("query")
        .arg("where-symbol")
        .arg("Close")
        .assert()
        .success()
        .stdout(predicate::str::contains("DocumentCommands.swift"));

    let mut analyze_cmd = Command::cargo_bin("loct").unwrap();
    // -A --json
    analyze_cmd
        .current_dir(&root)
        .arg("-A")
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::str::contains(r#"Foundation"#))
        .stdout(predicate::str::contains(r#"AppState.DocumentStore"#));

    let cache = tempdir().unwrap();
    let mut impact_cmd = Command::cargo_bin("loct").unwrap();
    impact_cmd
        .current_dir(&root)
        .env("LOCT_CACHE_DIR", cache.path())
        .arg("--impact")
        .arg("Sources/Pensieve/Workspace/IndexDatabase.swift")
        .assert()
        .success();

    let mut edges_cmd = Command::cargo_bin("loct").unwrap();
    edges_cmd
        .current_dir(&root)
        .env("LOCT_CACHE_DIR", cache.path())
        .arg(r#"[.edges[] | select(.label == "implicit_symbol")] | length"#)
        .assert()
        .success()
        .stdout(predicate::str::contains("1"));
}

/// Wave C-1 acceptance: a same-module Swift consumer shows up as a *symbol*
/// consumer of the file it uses, even though no `import` connects the two
/// files (the import graph is structurally silent intra-module).
#[test]
fn swift_slice_reports_symbol_consumers_without_imports() {
    let cache = tempdir().unwrap();
    let root = PathBuf::from("tests/fixtures/cfamily/swift");

    Command::cargo_bin("loct")
        .unwrap()
        .current_dir(&root)
        .env("LOCT_CACHE_DIR", cache.path())
        .arg("scan")
        .arg(".")
        .assert()
        .success();

    Command::cargo_bin("loct")
        .unwrap()
        .current_dir(&root)
        .env("LOCT_CACHE_DIR", cache.path())
        .arg("slice")
        .arg("DocumentStore.swift")
        .assert()
        .success()
        .stdout(predicate::str::contains("Symbol consumers"))
        .stdout(predicate::str::contains("DocumentStoreTests.swift"))
        .stdout(predicate::str::contains("DocumentStore"));
}
