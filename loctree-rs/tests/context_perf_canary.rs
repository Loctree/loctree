//! Perf canaries for the sub-2s `loct context` session-start contract
//! (W1-06).
//!
//! The 12-15 s warm `loct context` regression was a fork-storm: the tree
//! walk consulted gitignore state by spawning `git check-ignore -q` once
//! per walked path (1269 spawns ≈ 13 s on a ~900-file repo). The fix moved
//! [`loctree::fs_utils::GitIgnoreChecker`] onto in-process libgit2. These
//! tests keep that class of regression out without any CI-flaky wall-clock
//! assertion — they count *mechanisms* (subprocess call sites, rule
//! parity), not milliseconds.
//!
//! The companion runtime canaries live next to the code they guard:
//! `aicx::tests::exhausted_overlay_budget_latches_timeout_without_transport`
//! (no blocking overlay call once the budget is dry) and
//! `pack::tests::compose_memory_slice_reports_timed_out_when_budget_exhausted`
//! (honest `timed_out` skip reason).

use std::fs;
use std::path::Path;
use std::process::Command;

use loctree::fs_utils::GitIgnoreChecker;

/// The per-path walk helpers must never shell out for quiet ignore checks.
///
/// `fs_utils.rs` (GitIgnoreChecker + gather_files) and `tree.rs` (the
/// walker) are hot per-path code: one subprocess per path is the exact
/// shape of the 13 s regression. The guard targets the quiet probe
/// signature (`check-ignore` + `-q`) specifically — the verbose
/// `explain_ignored` variant (`-v`, cold diagnostic path, once per query)
/// and `snapshot.rs`'s single once-per-scan hygiene probe stay legal.
#[test]
fn per_path_walk_modules_do_not_spawn_git_check_ignore() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let hot_modules = ["fs_utils.rs", "tree.rs"];
    // Whitespace-insensitive signatures of the quiet per-path probe, built
    // via concat! so this test file does not match its own check.
    let forbidden = [
        concat!(".arg(\"check", "-ignore\").arg(\"-q\")"),
        concat!("[\"check", "-ignore\",\"-q\""),
    ];

    for module in hot_modules {
        let path = src_root.join(module);
        let content = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let squashed: String = content.split_whitespace().collect();
        for pattern in forbidden {
            assert!(
                !squashed.contains(pattern),
                "{module} spawns a quiet `git check-ignore -q` probe — per-path \
                 gitignore checks must stay in-process \
                 (git2::Repository::is_path_ignored); one subprocess per walked \
                 path is the 13s `loct context` fork-storm regression"
            );
        }
    }
}

/// The in-process checker must agree with `git check-ignore` ground truth
/// on the rule shapes the walk actually meets: plain file patterns,
/// directory patterns, negations, and nested .gitignore files.
#[test]
fn gitignore_checker_matches_git_check_ignore_semantics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "-q"]);

    fs::write(
        root.join(".gitignore"),
        "target/\n*.log\n!keep.log\n.loctree/\n",
    )
    .expect("write .gitignore");
    fs::create_dir_all(root.join("src/nested")).expect("mkdir src/nested");
    fs::write(root.join("src/nested/.gitignore"), "local-only.txt\n").expect("nested gitignore");
    fs::create_dir_all(root.join("target/debug")).expect("mkdir target");
    // Directory-only patterns (`.loctree/`) need the directory to exist:
    // libgit2 stats the path to learn it is a directory, and the real walk
    // only ever asks about paths that came out of read_dir anyway.
    fs::create_dir_all(root.join(".loctree")).expect("mkdir .loctree");
    fs::write(root.join("app.log"), "log").expect("write app.log");
    fs::write(root.join("keep.log"), "kept").expect("write keep.log");
    fs::write(root.join("src/main.rs"), "fn main() {}").expect("write main.rs");
    fs::write(root.join("src/nested/local-only.txt"), "x").expect("write local-only");

    let checker = GitIgnoreChecker::new(root).expect("temp dir is a git repo");

    let cases = [
        ("target", true),
        ("target/debug", true),
        ("app.log", true),
        ("keep.log", false),
        (".loctree", true),
        ("src/main.rs", false),
        ("src/nested/local-only.txt", true),
    ];
    for (rel, expected) in cases {
        let got = checker.is_ignored(&root.join(rel));
        assert_eq!(
            got, expected,
            "GitIgnoreChecker::is_ignored({rel}) = {got}, git semantics say {expected}"
        );
    }
}
