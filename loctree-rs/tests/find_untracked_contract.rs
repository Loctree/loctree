//! Snapshot-only modes must not silently ignore an untracked-source request.
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::str::contains;

#[test]
fn unsupported_find_modes_reject_untracked_before_scanning() {
    let root = tempfile::TempDir::new().unwrap();
    for args in [
        vec!["find", "--discover", "FreshSymbol", "--include-untracked"],
        vec!["find", "--file", "fresh.rs", "--include-untracked"],
        vec![
            "find",
            "--where-symbol",
            "FreshSymbol",
            "--include-untracked",
        ],
    ] {
        cargo_bin_cmd!("loct")
            .current_dir(root.path())
            .args(args)
            .assert()
            .code(2)
            .stderr(contains(
                "--include-untracked requires find --literal or --regex",
            ))
            .stderr(contains("loct scan --fresh"));
    }
}
