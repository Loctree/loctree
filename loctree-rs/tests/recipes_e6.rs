//! E6 recipe-round RED tests (F1, F2b, F2c) for the dead-parrots engine.
//!
//! F2c (Zig synthetic `test#N` exports never dead) and F3/F4/F9 are covered
//! by inline/unit tests in their own modules (twins.rs, barrels.rs,
//! cli/parser/analysis_commands.rs) per the COMPILE_EMBARGO file scope for
//! this pass. This file covers the two findings that need a real CLI-driven
//! fixture (F1, F2b) plus a direct-call regression for F2c that does not
//! need a fixture at all.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn fixtures_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
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
            std::fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

/// Get a command pointing to the `loct` binary, isolated from the
/// operator's real global cache and from a real browser-open side effect.
fn loct() -> Command {
    let mut cmd = cargo_bin_cmd!("loct");
    cmd.env("LOCT_OPEN_BROWSER", "0");
    cmd.env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1");
    cmd
}

/// Copy a fixture into an isolated temp dir + private cache, then run a
/// full scan. Mirrors `e2e_cli.rs`'s `dead_truth_fixture` helper (that one
/// is private to its own file, so this is a self-contained equivalent for
/// this test binary).
fn dead_truth_fixture(name: &str) -> (TempDir, TempDir) {
    let temp = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    copy_dir_all(&fixtures_path().join(name), temp.path()).unwrap();
    loct()
        .current_dir(temp.path())
        .env("LOCT_CACHE_DIR", cache.path())
        .args(["scan", "--full-scan"])
        .assert()
        .success();
    (temp, cache)
}

fn dead_json(root: &Path, cache: &Path) -> Vec<Value> {
    let output = loct()
        .current_dir(root)
        .env("LOCT_CACHE_DIR", cache)
        .args(["dead", "--json", "--full"])
        .assert()
        .success()
        .get_output()
        .clone();
    serde_json::from_slice::<Value>(&output.stdout)
        .expect("loct dead --json must emit JSON")
        .as_array()
        .expect("loct dead --json must emit an array")
        .clone()
}

/// F1 RED: `hf_cache.rs` has one genuinely dead fn. An UNRELATED string
/// literal elsewhere (a log message) merely contains the file's stem
/// ("hf_cache"), with no spawn/exec call anywhere near it. Before the fix,
/// ANY string literal matching a candidate's file-stem — anywhere in the
/// repo, regardless of context — degraded the candidate to "low" with a
/// "possible spawn-by-string" note. That is why `--confidence high`
/// returned 0 where `--full` found genuinely dead fns: the noise degraded
/// every export sharing that file's stem, not just real spawn targets.
#[test]
fn f1_stem_noise_without_spawn_context_does_not_degrade_confidence() {
    let (fixture, cache) = dead_truth_fixture("recipes_e6_stem_noise");
    let dead = dead_json(fixture.path(), cache.path());

    let candidate = dead
        .iter()
        .find(|d| d["symbol"].as_str() == Some("download_model_cache"))
        .unwrap_or_else(|| panic!("dead fn in hf_cache.rs must still be a candidate: {dead:?}"));

    assert_eq!(
        candidate["confidence"].as_str(),
        Some("high"),
        "an unrelated string literal containing the file's stem, with no \
         spawn/exec context on that line, must not degrade confidence: {candidate:?}"
    );
    assert!(
        !candidate["reason"]
            .as_str()
            .unwrap_or("")
            .contains("string-literal reference"),
        "no spawn/exec context near the stem hit -> no string-literal cross-check note: {candidate:?}"
    );

    // The candidate must also survive `--confidence high` filtering — the
    // whole point of F1 is that this class of false-positive degrade is why
    // `--confidence high` under-reported real dead code.
    let output = loct()
        .current_dir(fixture.path())
        .env("LOCT_CACHE_DIR", cache.path())
        .args(["dead", "--json", "--confidence", "high"])
        .assert()
        .success()
        .get_output()
        .clone();
    let high_confidence: Vec<Value> = serde_json::from_slice::<Value>(&output.stdout)
        .expect("loct dead --json must emit JSON")
        .as_array()
        .expect("loct dead --json must emit an array")
        .clone();
    assert!(
        high_confidence
            .iter()
            .any(|d| d["symbol"].as_str() == Some("download_model_cache")),
        "--confidence high must not lose this candidate to stem noise: {high_confidence:?}"
    );
}

/// F2b RED: `export default { findByRunId(){}, getJob(){} }` — nothing in
/// the fixture imports this module at all, so every member is genuinely
/// dead. Before the fix, `handle_export_default_declaration`'s fallback arm
/// pushed one opaque "default" export and every object-literal member
/// vanished: `loct dead` reported 0 where every member was actually unused
/// (false negative).
#[test]
fn f2b_default_object_members_become_individual_dead_candidates() {
    let (fixture, cache) = dead_truth_fixture("recipes_e6_default_object_members");
    let dead = dead_json(fixture.path(), cache.path());

    for name in ["findByRunId", "getJob"] {
        assert!(
            dead.iter().any(|d| d["symbol"].as_str() == Some(name)),
            "default-exported object member '{name}' must be its own dead \
             candidate, not hidden behind the opaque \"default\" export: {dead:?}"
        );
    }
}

/// F2c RED: Zig `test { ... }` blocks are pushed as synthetic exports
/// (`test#N`, by `zig.rs`'s `push_tests` — not part of this fix) so they
/// show up on symbol/tagmap surfaces. They are guaranteed 0-referenced by
/// design (nothing calls a test block), so before this fix every one of
/// them was a false "high confidence dead" candidate. This is a direct
/// unit-style call — no fixture/CLI round-trip needed since the bug and fix
/// are both in the candidate-loop's kind-based skip list.
#[test]
fn f2c_synthetic_test_kind_export_is_never_a_dead_candidate() {
    use loctree::analyzer::dead_parrots::{DeadFilterConfig, find_dead_exports};
    use loctree::types::{ExportSymbol, FileAnalysis};

    let analysis = FileAnalysis {
        path: "src/synthetic.zig".to_string(),
        language: "zig".to_string(),
        exports: vec![ExportSymbol::new(
            "test#1".to_string(),
            "test",
            "named",
            Some(1),
        )],
        ..Default::default()
    };

    let dead = find_dead_exports(&[analysis], false, None, DeadFilterConfig::default());
    assert!(
        dead.iter().all(|d| d.symbol != "test#1"),
        "synthetic Zig test-block exports (kind == \"test\") must never be \
         dead candidates: {dead:?}"
    );
}
