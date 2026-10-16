//! Graph cycles are evidence of coupling, not compiler/runtime verdicts.

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

fn cli(root: &Path, cache: &Path, args: &[&str]) -> Value {
    let output = cargo_bin_cmd!("loct")
        .current_dir(root)
        .env("LOCT_CACHE_DIR", cache)
        .env("LOCT_OPEN_BROWSER", "0")
        .env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1")
        .args(args)
        .output()
        .expect("CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let start = output
        .stdout
        .iter()
        .position(|b| *b == b'{')
        .expect("JSON object");
    serde_json::from_slice(&output.stdout[start..]).expect("JSON")
}

fn rust_counterexample(a: &str, b: &str) {
    let root = TempDir::new().expect("owned fixture");
    let cache = TempDir::new().expect("owned fixture");
    fs::write(root.path().join("lib.rs"), "pub mod a; pub mod b;").expect("owned fixture");
    fs::write(root.path().join("a.rs"), a).expect("owned fixture");
    fs::write(root.path().join("b.rs"), b).expect("owned fixture");
    let compile = Command::new("rustc")
        .current_dir(root.path())
        .args(["--edition=2024", "--crate-type=lib", "lib.rs", "-o"])
        .arg(cache.path().join("fixture.rlib"))
        .output()
        .expect("owned fixture");
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let cycles = cli(root.path(), cache.path(), &["cycles", "--json"]);
    let rows = cycles["classifiedCycles"]
        .as_array()
        .expect("owned fixture");
    assert_eq!(
        rows.len(),
        1,
        "graph coupling must remain visible: {cycles}"
    );
    assert_eq!(rows[0]["classification"], "hard_bidirectional");
    assert_eq!(
        rows[0]["compilability"], "structural",
        "rustc succeeded: {cycles}"
    );
    assert_eq!(rows[0]["risk"], "low");
    let health = cli(root.path(), cache.path(), &["health", "--json"]);
    assert_eq!(health["cycles"]["high_risk"], 0);
    let findings = cli(
        root.path(),
        cache.path(),
        &["findings", "--summary", "--json"],
    );
    assert_eq!(findings["cycles"]["breaking"], 0);
    assert_eq!(findings["cycles"]["structural"], 1);
    let agent = cli(root.path(), cache.path(), &["--for-ai"]);
    assert_eq!(agent["summary"]["health_score"], findings["health_score"]);
    assert_eq!(agent["summary"]["health_details"]["certain"]["count"], 0);
    let html_path = cache.path().join("report.html");
    cargo_bin_cmd!("loct")
        .current_dir(root.path())
        .env("LOCT_CACHE_DIR", cache.path())
        .env("LOCT_OPEN_BROWSER", "0")
        .env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1")
        .args(["report", "--output"])
        .arg(&html_path)
        .assert()
        .success();
    let html = fs::read_to_string(html_path).expect("owned fixture");
    assert!(html.contains("compiler/runtime outcome unverified"));
    assert!(!html.contains("These cycles break immediately"));
}

#[test]
fn rust_mutual_functions_are_structural() {
    rust_counterexample(
        "use crate::b::b; pub fn a(n: u8) -> u8 { if n == 0 { 0 } else { b(n-1) } }",
        "use crate::a::a; pub fn b(n: u8) -> u8 { if n == 0 { 0 } else { a(n-1) } }",
    );
}

#[test]
fn rust_boxed_recursive_types_are_structural() {
    rust_counterexample(
        "use crate::b::B; pub struct A { pub next: Option<Box<B>> }",
        "use crate::a::A; pub struct B { pub next: Option<Box<A>> }",
    );
}

#[test]
fn rust_structural_is_not_a_certificate_of_compilation() {
    let root = TempDir::new().expect("fixture");
    let cache = TempDir::new().expect("cache");
    for (name, source) in [
        ("lib.rs", "pub mod a; pub mod b;"),
        ("a.rs", "use crate::b::B; pub struct A { pub next: B }"),
        ("b.rs", "use crate::a::A; pub struct B { pub next: A }"),
    ] {
        fs::write(root.path().join(name), source).expect("source");
    }
    let result = Command::new("rustc")
        .current_dir(root.path())
        .args(["--crate-type=lib", "lib.rs", "-o"])
        .arg(cache.path().join("invalid.rlib"))
        .output()
        .expect("rustc");
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("infinite size"));
    let cycles = cli(root.path(), cache.path(), &["cycles", "--json"]);
    assert_eq!(
        cycles["classifiedCycles"].as_array().expect("cycles").len(),
        1
    );
    assert_eq!(cycles["classifiedCycles"][0]["compilability"], "structural");
    assert!(
        cycles["classifiedCycles"][0]["pattern"]
            .as_str()
            .expect("pattern")
            .contains("unverified")
    );
}

#[test]
fn safe_and_failing_initialization_share_topology_not_a_verdict() {
    for (ext, a, b, runtime, entry, succeeds) in [
        (
            "mjs",
            "import { b } from './b.mjs'; export function a() { return b(); }",
            "import { a } from './a.mjs'; export function b() { return typeof a; }",
            "node",
            "a.mjs",
            true,
        ),
        (
            "mjs",
            "import { b } from './b.mjs'; export const a = b + 1;",
            "import { a } from './a.mjs'; export const b = a + 1;",
            "node",
            "a.mjs",
            false,
        ),
        (
            "py",
            "import b\ndef a(): return b.b()\n",
            "import a\ndef b(): return a.a\n",
            "python3",
            "a.py",
            true,
        ),
        (
            "py",
            "from b import b\ndef a(): return b()\n",
            "from a import a\ndef b(): return a()\n",
            "python3",
            "a.py",
            false,
        ),
    ] {
        let root = TempDir::new().expect("owned fixture");
        let cache = TempDir::new().expect("owned fixture");
        fs::write(root.path().join(format!("a.{ext}")), a).expect("owned fixture");
        fs::write(root.path().join(format!("b.{ext}")), b).expect("owned fixture");
        let result = Command::new(runtime)
            .current_dir(root.path())
            .arg(entry)
            .output()
            .expect("owned fixture");
        assert_eq!(
            result.status.success(),
            succeeds,
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        if !succeeds {
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(stderr.contains(if ext == "mjs" {
                "before initialization"
            } else {
                "ImportError"
            }));
        }
        let cycles = cli(root.path(), cache.path(), &["cycles", "--json"]);
        let rows = cycles["classifiedCycles"]
            .as_array()
            .expect("owned fixture");
        assert_eq!(rows.len(), 1, "{cycles}");
        assert_eq!(rows[0]["compilability"], "structural");
        assert_eq!(rows[0]["risk"], "medium");
        assert!(
            rows[0]["suggestion"]
                .as_str()
                .expect("owned fixture")
                .contains("initialization order")
        );
        assert!(
            rows[0]["pattern"]
                .as_str()
                .expect("owned fixture")
                .contains("unverified")
        );
        let findings = cli(
            root.path(),
            cache.path(),
            &["findings", "--summary", "--json"],
        );
        assert_eq!(findings["cycles"]["breaking"], 0);
        assert_eq!(findings["cycles"]["structural"], 1);
    }
}
