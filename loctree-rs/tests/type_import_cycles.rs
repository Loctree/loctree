//! Type dependencies survive snapshot projection without becoming runtime defects.

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use loctree::analyzer::health_inputs::structural_defects;
use loctree::snapshot::{GraphEdge, Snapshot};
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

fn run(root: &Path, cache: &Path, args: &[&str]) -> Value {
    let mut cmd: Command = cargo_bin_cmd!("loct");
    let output = cmd
        .current_dir(root)
        .env("LOCT_CACHE_DIR", cache)
        .env("LOCT_OPEN_BROWSER", "0")
        .env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1")
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&output).expect("CLI JSON")
}

fn fixture(files: &[(&str, &str)]) -> TempDir {
    let root = TempDir::new().expect("fixture");
    for (name, content) in files {
        std::fs::write(root.path().join(name), content).expect("fixture source");
    }
    root
}

fn assert_surfaces(
    root: &Path,
    expected_cycles: usize,
    expected_breaking: usize,
) -> Vec<GraphEdge> {
    let cache = TempDir::new().expect("cache");
    // Cold canonical snapshot construction, then repeated reads of that snapshot.
    let edges: Vec<GraphEdge> =
        serde_json::from_value(run(root, cache.path(), &["[.edges[]]"])).expect("edges");
    let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
    snapshot.edges = edges.clone();
    let defects = structural_defects(&snapshot, &[], &[], 0);
    assert_eq!(defects.breaking_cycles, expected_breaking);
    assert_eq!(defects.structural_cycles, expected_cycles);
    // MCP repo-view/follow and LSP use the raw walker, not the strict/lazy
    // wrapper. Preserved type dependencies must not create defects there.
    if edges.iter().any(|edge| edge.label == "type_import") {
        let raw_edges = edges
            .iter()
            .map(|edge| (edge.from.clone(), edge.to.clone(), edge.label.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            loctree::analyzer::cycles::find_cycles(&raw_edges).len(),
            expected_cycles
        );
    }

    for _ in 0..2 {
        let cycles = run(root, cache.path(), &["cycles", "--json"]);
        assert_eq!(
            cycles["classifiedCycles"].as_array().expect("cycles").len(),
            expected_cycles,
            "{cycles}"
        );
        let health = run(root, cache.path(), &["health", "--json"]);
        assert_eq!(health["cycles"]["total"], expected_cycles);
        assert_eq!(health["cycles"]["high_risk"], expected_breaking);
        let findings = run(root, cache.path(), &["findings", "--summary", "--json"]);
        assert_eq!(findings["cycles"]["breaking"], expected_breaking);
        assert_eq!(findings["cycles"]["structural"], expected_cycles);
        let warm = run(root, cache.path(), &["[.edges[]]"]);
        assert_eq!(serde_json::to_value(&edges).expect("edges JSON"), warm);
    }
    edges
}

#[test]
fn type_only_pair_keeps_navigation_edges_without_runtime_defects() {
    let root = fixture(&[
        (
            "a.ts",
            "import type { B } from './b.ts'; export interface A { b: B }",
        ),
        (
            "b.ts",
            "import type { A } from './a.ts'; export interface B { a: A }",
        ),
    ]);
    let edges = assert_surfaces(root.path(), 0, 0);
    assert_eq!(edges.len(), 2);
    assert!(edges.iter().all(|edge| edge.label == "type_import"));
}

#[test]
fn mixed_type_value_ring_is_not_a_runtime_cycle() {
    let root = fixture(&[
        (
            "a.ts",
            "import type { B } from './b.ts'; export interface A { b: B } export const a = 1;",
        ),
        (
            "b.ts",
            "import { c } from './c.ts'; export const b = c; export interface B {}",
        ),
        ("c.ts", "import { a } from './a.ts'; export const c = a;"),
    ]);
    let edges = assert_surfaces(root.path(), 0, 0);
    assert_eq!(edges.len(), 3);
    assert_eq!(edges.iter().filter(|e| e.label == "type_import").count(), 1);
    assert_eq!(edges.iter().filter(|e| e.label == "import").count(), 2);
}

#[test]
fn value_cycle_survives_parallel_type_dependencies() {
    let root = fixture(&[
        (
            "a.ts",
            "import type { B } from './b.ts'; import { b } from './b.ts'; export const a = b + 1; export interface A {}",
        ),
        (
            "b.ts",
            "import type { A } from './a.ts'; import { a } from './a.ts'; export const b = a + 1; export interface B {}",
        ),
    ]);
    let edges = assert_surfaces(root.path(), 1, 0);
    assert_eq!(edges.iter().filter(|e| e.label == "type_import").count(), 2);
    assert_eq!(edges.iter().filter(|e| e.label == "import").count(), 2);
}

#[test]
fn python_type_checking_edges_remain_non_runtime() {
    let root = fixture(&[
        (
            "a.py",
            "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from b import B\nclass A: pass\n",
        ),
        (
            "b.py",
            "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from a import A\nclass B: pass\n",
        ),
    ]);
    let edges = assert_surfaces(root.path(), 0, 0);
    assert_eq!(edges.len(), 2);
    assert!(edges.iter().all(|edge| edge.label == "type_import"));
}

#[test]
fn python_value_cycle_retains_unverified_review_evidence() {
    let root = fixture(&[
        ("a.py", "from b import b\ndef a(): return b()\n"),
        ("b.py", "from a import a\ndef b(): return a()\n"),
    ]);
    let edges = assert_surfaces(root.path(), 1, 0);
    assert!(edges.iter().all(|edge| edge.label == "import"));
}

#[test]
fn python_lazy_cycle_remains_unscored() {
    let root = fixture(&[
        ("a.py", "def a():\n    from b import b\n    return b()\n"),
        ("b.py", "def b():\n    from a import a\n    return a()\n"),
    ]);
    let edges = assert_surfaces(root.path(), 0, 0);
    assert_eq!(edges.len(), 2);
    assert!(edges.iter().all(|edge| edge.label == "lazy_import"));
}

#[test]
fn rust_module_declaration_is_not_a_value_import() {
    let root = fixture(&[
        (
            "Cargo.toml",
            "[package]\nname = 'type-edge-control'\nversion = '0.1.0'\nedition = '2024'\n[lib]\npath = 'lib.rs'\n",
        ),
        ("lib.rs", "mod child; mod parent;"),
        ("parent.rs", "pub fn parent() {}"),
        (
            "child.rs",
            "use crate::parent::parent; pub fn child() { parent(); }",
        ),
    ]);
    let edges = assert_surfaces(root.path(), 0, 0);
    assert!(edges.iter().any(|edge| edge.label == "mod"));
    assert!(edges.iter().any(|edge| edge.label == "import"));
}
