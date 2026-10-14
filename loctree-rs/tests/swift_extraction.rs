use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;
use tempfile::tempdir;

fn assert_all_loct_commands_are_cache_isolated() {
    let source = include_str!("swift_extraction.rs");
    let loct_command = concat!("Command::cargo_bin", "(\"loct\")");
    let cache_override = concat!(".env", "(\"LOCT_CACHE_DIR\"");
    let commands = source.matches(loct_command).count();
    let overrides = source.matches(cache_override).count();

    assert_eq!(
        overrides, commands,
        "each loct CLI invocation in swift_extraction.rs must set LOCT_CACHE_DIR to avoid writing fixture snapshots into the operator-global cache"
    );
}

#[test]
fn swift_cli_commands_are_cache_isolated() {
    assert_all_loct_commands_are_cache_isolated();
}

#[test]
fn swift_graph_correctness() {
    assert_all_loct_commands_are_cache_isolated();
    let cache = tempdir().unwrap();
    let mut cmd = Command::cargo_bin("loct").unwrap();
    let root = PathBuf::from("tests/fixtures/cfamily/Pensieve");

    // LCT-003 acceptance: query where-symbol WorkspaceCacheStore
    cmd.current_dir(&root)
        .env("LOCT_CACHE_DIR", cache.path())
        .arg("query")
        .arg("where-symbol")
        .arg("WorkspaceCacheStore")
        .assert()
        .success()
        .stdout(predicate::str::contains("WorkspaceCacheStore.swift"));

    let mut final_class_cmd = Command::cargo_bin("loct").unwrap();
    final_class_cmd
        .current_dir(&root)
        .env("LOCT_CACHE_DIR", cache.path())
        .arg("query")
        .arg("where-symbol")
        .arg("WorkspaceMetadataStore")
        .assert()
        .success()
        .stdout(predicate::str::contains("DocumentCommands.swift"));

    let mut folder_manager_cmd = Command::cargo_bin("loct").unwrap();
    folder_manager_cmd
        .current_dir(&root)
        .env("LOCT_CACHE_DIR", cache.path())
        .arg("query")
        .arg("where-symbol")
        .arg("FolderManager")
        .assert()
        .success()
        .stdout(predicate::str::contains("DocumentCommands.swift"));

    let mut method_cmd = Command::cargo_bin("loct").unwrap();
    method_cmd
        .current_dir(&root)
        .env("LOCT_CACHE_DIR", cache.path())
        .arg("query")
        .arg("where-symbol")
        .arg("closeActiveDocument")
        .assert()
        .success()
        .stdout(predicate::str::contains("DocumentCommands.swift"));

    let mut close_command_cmd = Command::cargo_bin("loct").unwrap();
    close_command_cmd
        .current_dir(&root)
        .env("LOCT_CACHE_DIR", cache.path())
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
        .env("LOCT_CACHE_DIR", cache.path())
        .arg("-A")
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::str::contains(r#"Foundation"#))
        .stdout(predicate::str::contains(r#"AppState.DocumentStore"#));
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

/// Regression: loctree-feedback.md 2026-07-25 (blinksh/blink). A nested
/// `enum Error` flattened into file exports used to give the defining file a
/// module-sized implicit fan-in: every Swift file using the STDLIB `Error`
/// became an "importer", and hub ranking mirrored module size. Implicit
/// edges are now created only for unambiguous, non-stdlib-shadowed type
/// names, and `who-imports` verifies real symbol usage.
#[test]
fn swift_implicit_edges_ignore_stdlib_shadowed_and_ambiguous_names() {
    let cache = tempdir().unwrap();
    let root = PathBuf::from("tests/fixtures/cfamily/swift-implicit");

    Command::cargo_bin("loct")
        .unwrap()
        .current_dir(&root)
        .env("LOCT_CACHE_DIR", cache.path())
        .arg("scan")
        .arg(".")
        .assert()
        .success();

    // Exactly one implicit edge survives: Consumer.swift -> Agent.swift.
    // `Error` (stdlib-shadowed) and `SharedThing` (two exporters) create none.
    Command::cargo_bin("loct")
        .unwrap()
        .current_dir(&root)
        .env("LOCT_CACHE_DIR", cache.path())
        .arg(r#"[.edges[] | select(.label == "implicit_symbol")] | length"#)
        .assert()
        .success()
        .stdout(predicate::str::contains("1"))
        .stdout(predicate::str::contains("10").not());

    // who-imports reports the real consumer with the real reference line and
    // never the stdlib-`Error` bystander.
    Command::cargo_bin("loct")
        .unwrap()
        .current_dir(&root)
        .env("LOCT_CACHE_DIR", cache.path())
        .args(["query", "who-imports", "DefaultAgent"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Consumer.swift"))
        .stdout(predicate::str::contains("Unrelated.swift").not())
        .stdout(predicate::str::contains(
            "references DefaultAgent (implicit module scope)",
        ));
}
