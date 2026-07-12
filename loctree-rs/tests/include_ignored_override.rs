//! Integration tests for the `--include-ignored` override.
//!
//! Contract: files excluded by `.loctignore` (tests, scripts, docs) are
//! load-bearing review surfaces that agents sometimes need to inspect. By
//! default read commands (`find`/`slice`/`impact`) never see them; with
//! `--include-ignored` they are surfaced for a single ephemeral read and
//! explicitly marked `ignored`, without polluting the persisted snapshot.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use tempfile::TempDir;

/// Command pointing at the `loct` binary with browser side effects disabled.
fn loct() -> Command {
    let mut cmd = cargo_bin_cmd!("loct");
    cmd.env("LOCT_OPEN_BROWSER", "0");
    cmd
}

fn git(args: &[&str], dir: &std::path::Path) {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git command runs");
}

/// A repo where `.loctignore` excludes `tests/`. `tests/helper.ts` defines
/// `MAGIC_TOKEN` and is imported by `src/main.ts`, so it is genuinely
/// load-bearing yet normally invisible to loctree.
fn ignored_fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    let path = temp.path();

    git(&["init"], path);
    git(&["config", "user.email", "test@test.com"], path);
    git(&["config", "user.name", "Test User"], path);

    std::fs::create_dir_all(path.join("src")).unwrap();
    std::fs::create_dir_all(path.join("tests")).unwrap();
    std::fs::write(
        path.join("src/main.ts"),
        "import { helper } from \"../tests/helper\";\nexport function core() { return helper(); }\n",
    )
    .unwrap();
    std::fs::write(
        path.join("tests/helper.ts"),
        "export function helper() { return 42; }\nexport const MAGIC_TOKEN = 1;\n",
    )
    .unwrap();
    std::fs::write(path.join(".loctignore"), "tests/\n").unwrap();

    git(&["add", "-A"], path);
    git(&["commit", "-m", "init"], path);

    temp
}

/// Default `find` must NOT surface a symbol that lives only in a
/// `.loctignore`-excluded file.
#[test]
fn default_find_hides_ignored_file() {
    let repo = ignored_fixture();

    let out = loct()
        .current_dir(repo.path())
        .args(["--json", "find", "--literal", "MAGIC_TOKEN"])
        .assert()
        .success();
    let json: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(
        json["literal_matches"]["total"].as_u64(),
        Some(0),
        "default find must not see .loctignore-excluded files"
    );
}

/// `--include-ignored find` surfaces the hidden hit AND marks it `ignored`.
#[test]
fn include_ignored_find_surfaces_and_marks() {
    let repo = ignored_fixture();

    let out = loct()
        .current_dir(repo.path())
        .args([
            "--include-ignored",
            "--json",
            "find",
            "--literal",
            "MAGIC_TOKEN",
        ])
        .assert()
        .success();
    let json: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();

    let matches = &json["literal_matches"];
    assert_eq!(
        matches["total"].as_u64(),
        Some(1),
        "override must surface the hidden hit"
    );
    let occ = &matches["occurrences"][0];
    assert_eq!(occ["file"].as_str(), Some("tests/helper.ts"));
    assert_eq!(
        occ["ignored"].as_bool(),
        Some(true),
        "surfaced hit must be marked ignored so the agent knows it is normally hidden"
    );
}

/// Default `slice` on a `.loctignore`-excluded file fails with an exclusion
/// note; `--include-ignored slice` returns the slice with `ignored=true`.
#[test]
fn slice_override_shows_marked_target() {
    let repo = ignored_fixture();

    // Establish a clean snapshot first (mirrors normal agent usage).
    loct()
        .current_dir(repo.path())
        .args(["--json", "find", "--literal", "core"])
        .assert()
        .success();

    // Default slice cannot see the excluded file.
    loct()
        .current_dir(repo.path())
        .args(["slice", "tests/helper.ts"])
        .assert()
        .failure();

    // Override slice sees it and marks the core file ignored.
    let out = loct()
        .current_dir(repo.path())
        .args(["--include-ignored", "--json", "slice", "tests/helper.ts"])
        .assert()
        .success();
    let json: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    let core0 = &json["core"][0];
    assert_eq!(core0["path"].as_str(), Some("tests/helper.ts"));
    assert_eq!(
        core0["ignored"].as_bool(),
        Some(true),
        "sliced ignored file must be marked ignored"
    );
}

/// The override is ephemeral: running it must NOT persist ignored files into
/// the cached snapshot, so a later default read stays clean.
#[test]
fn override_does_not_pollute_persisted_snapshot() {
    let repo = ignored_fixture();

    // Prime the persisted snapshot with a normal read.
    loct()
        .current_dir(repo.path())
        .args(["--json", "find", "--literal", "core"])
        .assert()
        .success();

    // Run several override reads that scan the superset.
    for args in [
        ["--include-ignored", "find", "--literal", "helper"].as_slice(),
        ["--include-ignored", "slice", "tests/helper.ts"].as_slice(),
        ["--include-ignored", "impact", "tests/helper.ts"].as_slice(),
    ] {
        loct()
            .current_dir(repo.path())
            .args(args)
            .assert()
            .success();
    }

    // A default read afterwards must still not see the excluded file.
    let out = loct()
        .current_dir(repo.path())
        .args(["--json", "find", "--literal", "MAGIC_TOKEN"])
        .assert()
        .success();
    let json: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(
        json["literal_matches"]["total"].as_u64(),
        Some(0),
        "persisted snapshot must remain clean after override reads"
    );
}
