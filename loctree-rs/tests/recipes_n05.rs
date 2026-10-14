//! N05 (audit audi-260927-162748-80858) — entity-first default `find`.
//!
//! The recipe-C rollup existed but never reached the default `find` surface,
//! printed every flat row again below itself, skipped single-identity groups,
//! and counted definition sites among "production caller(s)". These tests pin
//! the repaired contract on a purpose-built fixture, and pin that the explicit
//! `--literal` spelling keeps the flat per-hit output.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use std::path::PathBuf;
use tempfile::TempDir;

fn fixtures_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn loct() -> Command {
    let mut cmd = cargo_bin_cmd!("loct");
    cmd.env("LOCT_OPEN_BROWSER", "0");
    cmd.env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1");
    cmd
}

fn find_stdout(args: &[&str]) -> String {
    let fixture = fixtures_path().join("recipes_n05_entity_find");
    let cache = TempDir::new().expect("temp cache");
    let output = loct()
        .current_dir(&fixture)
        .env("LOCT_CACHE_DIR", cache.path())
        .args(args)
        .assert()
        .success()
        .get_output()
        .clone();
    String::from_utf8(output.stdout).expect("stdout is utf-8")
}

#[test]
fn default_find_groups_by_entity_and_collapses_test_rows() {
    let out = find_stdout(&["find", "widget"]);
    assert!(
        out.contains("by entity:"),
        "default find must print the entity rollup, got:\n{out}"
    );
    assert!(
        out.contains("widget (src/widget.rs)  [1 definition(s), 2 production caller(s)]  (1 test)"),
        "the definition is not a caller: one definition, the two same-line call-site hits as callers, the test hit collapsed, got:\n{out}"
    );
    assert!(
        out.contains("collapsed into the entity rollup above"),
        "test/docs rows must actually collapse, got:\n{out}"
    );
    assert!(
        !out.contains("tests/widget_test.rs:3"),
        "the test-scope flat row must not print again below the rollup, got:\n{out}"
    );
}

#[test]
fn explicit_literal_keeps_the_flat_per_hit_output() {
    let out = find_stdout(&["find", "--literal", "widget"]);
    assert!(
        !out.contains("by entity:"),
        "--literal is the explicit flat spelling and must not roll up, got:\n{out}"
    );
    assert!(
        out.contains("tests/widget_test.rs:3"),
        "--literal must keep printing every row, got:\n{out}"
    );
}

/// N10 (audit): a quote-include is a `Local` edge only when the header
/// exists in the scanned universe; `missing.h` must not become a graph node.
#[test]
fn c_include_edges_require_an_existing_target() {
    let fixture = fixtures_path().join("recipes_n10_c_includes");
    let cache = TempDir::new().expect("temp cache");
    let output = loct()
        .current_dir(&fixture)
        .env("LOCT_CACHE_DIR", cache.path())
        .arg("[.edges[]]")
        .assert()
        .success()
        .get_output()
        .clone();
    let edges: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("[.edges[]] must be JSON");
    let edges = edges.as_array().cloned().unwrap_or_default();

    let to_util = edges
        .iter()
        .any(|e| e.get("to").and_then(serde_json::Value::as_str) == Some("src/util.h"));
    let to_missing = edges.iter().any(|e| {
        e.get("to")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|to| to.ends_with("missing.h"))
    });
    assert!(
        to_util,
        "existing header keeps its include edge, got: {edges:#?}"
    );
    assert!(
        !to_missing,
        "a missing header must not appear as an edge target, got: {edges:#?}"
    );
}

/// H8 regression (post-audit): the pre-audit `focus tools/` consumers came
/// from the bare-name call synthesis that N02 removed as fabrication. The
/// honest replacement: a Makefile recipe line naming an explicit script path
/// is a dispatch fact — but only when that script exists in the scanned
/// universe (N10 parity).
#[test]
fn makefile_recipe_script_paths_become_call_edges_only_when_the_script_exists() {
    let fixture = fixtures_path().join("recipes_h8_makefile_dispatch");
    let cache = TempDir::new().expect("temp cache");
    let output = loct()
        .current_dir(&fixture)
        .env("LOCT_CACHE_DIR", cache.path())
        .arg("[.edges[]]")
        .assert()
        .success()
        .get_output()
        .clone();
    let edges: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("[.edges[]] must be JSON");
    let edges = edges.as_array().cloned().unwrap_or_default();

    let edge_to = |target: &str| {
        edges.iter().any(|e| {
            e.get("from").and_then(serde_json::Value::as_str) == Some("Makefile")
                && e.get("to").and_then(serde_json::Value::as_str) == Some(target)
        })
    };
    assert!(
        edge_to("tools/probe_runner.py"),
        "recipe `$(PY) tools/probe_runner.py` dispatches an existing script, got: {edges:#?}"
    );
    assert!(
        edge_to("scripts/lint.sh"),
        "recipe `./scripts/lint.sh` dispatches an existing script, got: {edges:#?}"
    );
    let to_ghost = edges.iter().any(|e| {
        e.get("to")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|to| to.ends_with("not_there.py"))
    });
    assert!(
        !to_ghost,
        "a missing script must not become an edge target, got: {edges:#?}"
    );
}
