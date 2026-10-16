use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

fn slice_fixture(files: &[(&str, &str)], target: &str) -> Value {
    let root = TempDir::new().expect("fixture root");
    let cache = TempDir::new().expect("private cache");
    for (path, source) in files {
        let path = root.path().join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("fixture directory");
        std::fs::write(path, source).expect("fixture source");
    }
    let output = cargo_bin_cmd!("loct")
        .current_dir(root.path())
        .env("LOCT_CACHE_DIR", cache.path())
        .env("LOCT_NO_GITIGNORE", "1")
        .env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1")
        .args(["slice", target, "--depth", "1", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let slice: Value = serde_json::from_slice(&output.stdout).expect("slice JSON");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("[loctree][slice]"));
    if slice["consumerCoverage"]["diagnostics"]
        .as_array()
        .is_some_and(|rows| !rows.is_empty())
    {
        let human = cargo_bin_cmd!("loct")
            .current_dir(root.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .env("LOCT_NO_GITIGNORE", "1")
            .env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1")
            .args(["slice", target, "--depth", "1"])
            .assert()
            .success()
            .get_output()
            .clone();
        assert!(String::from_utf8_lossy(&human.stdout).contains("Consumer coverage ("));
    }
    slice
}

fn consumer_depth(slice: &Value, path: &str) -> Option<u64> {
    slice["consumers"]
        .as_array()
        .expect("consumers")
        .iter()
        .find(|row| Path::new(row["path"].as_str().expect("consumer path")) == Path::new(path))
        .and_then(|row| row["depth"].as_u64())
}

#[test]
fn typescript_unrelated_reexport_does_not_propagate_target_consumers() {
    let slice = slice_fixture(
        &[
            ("target.ts", "export const a = 1;"),
            ("other.ts", "export const c = 2;"),
            (
                "bridge.ts",
                "import { a } from './target'; export { c } from './other'; console.log(a);",
            ),
            ("user.ts", "import { c } from './bridge'; console.log(c);"),
        ],
        "target.ts",
    );
    assert_eq!(consumer_depth(&slice, "bridge.ts"), Some(1));
    assert_eq!(consumer_depth(&slice, "user.ts"), None, "{slice}");
}

#[test]
fn rust_unrelated_pub_use_does_not_propagate_target_consumers() {
    let slice = slice_fixture(
        &[
            (
                "Cargo.toml",
                "[package]\nname = 'slice-fixture'\nversion = '0.1.0'\nedition = '2024'\n",
            ),
            (
                "src/lib.rs",
                "pub mod target; pub mod other; pub mod bridge; pub mod user;",
            ),
            ("src/target.rs", "pub fn a() {}"),
            ("src/other.rs", "pub fn c() {}"),
            (
                "src/bridge.rs",
                "use crate::target::a;\npub use crate::other::c;\npub fn run() { a(); }",
            ),
            (
                "src/user.rs",
                "use crate::bridge::c;\npub fn run() { c(); }",
            ),
        ],
        "src/target.rs",
    );
    assert_eq!(consumer_depth(&slice, "src/bridge.rs"), Some(1));
    assert_eq!(consumer_depth(&slice, "src/user.rs"), None, "{slice}");
}

#[test]
fn python_unrelated_package_reexport_does_not_propagate_target_consumers() {
    let slice = slice_fixture(
        &[
            ("pkg/target.py", "def a():\n    return 1\n"),
            ("pkg/other.py", "def c():\n    return 2\n"),
            (
                "pkg/__init__.py",
                "from . import target\nfrom .other import c\nvalue = target.a()\n",
            ),
            ("user.py", "from pkg import c\nprint(c())\n"),
        ],
        "pkg/target.py",
    );
    assert_eq!(consumer_depth(&slice, "pkg/__init__.py"), Some(1));
    assert_eq!(consumer_depth(&slice, "user.py"), None, "{slice}");
}

#[test]
fn typescript_alias_star_chain_keeps_identity_depth_and_terminates_cycles() {
    let slice = slice_fixture(
        &[
            ("target.ts", "export const a = 1;"),
            ("other.ts", "export const c = 2;"),
            (
                "bridge.ts",
                "export { a as renamed } from './target'; export { c } from './other';",
            ),
            (
                "second.ts",
                "export * from './bridge'; export * from './third';",
            ),
            (
                "third.ts",
                "export { renamed as finalName } from './second';",
            ),
            (
                "user.ts",
                "import { finalName as local } from './third'; console.log(local);",
            ),
            (
                "unrelated.ts",
                "import { c } from './second'; console.log(c);",
            ),
            (
                "namespace.ts",
                "import * as api from './second'; console.log(api.renamed);",
            ),
        ],
        "target.ts",
    );
    for (path, depth) in [
        ("bridge.ts", 1),
        ("second.ts", 2),
        ("third.ts", 3),
        ("user.ts", 4),
        ("namespace.ts", 3),
    ] {
        assert_eq!(consumer_depth(&slice, path), Some(depth), "{path}: {slice}");
    }
    assert_eq!(consumer_depth(&slice, "unrelated.ts"), None, "{slice}");
    assert_eq!(consumer_depth(&slice, "target.ts"), None);
}

#[test]
fn rust_pub_use_alias_and_star_retain_true_consumers() {
    let slice = slice_fixture(
        &[
            (
                "Cargo.toml",
                "[package]\nname = 'slice-fixture'\nversion = '0.1.0'\nedition = '2024'\n",
            ),
            (
                "src/lib.rs",
                "pub mod target;\npub mod bridge;\npub mod second;\npub mod user;",
            ),
            ("src/target.rs", "pub struct A;"),
            ("src/bridge.rs", "pub use crate::target::A as Alias;"),
            ("src/second.rs", "pub use crate::bridge::*;"),
            (
                "src/user.rs",
                "use crate::second::Alias;\npub fn run() -> Alias { Alias }",
            ),
        ],
        "src/target.rs",
    );
    for (path, depth) in [
        ("src/bridge.rs", 1),
        ("src/second.rs", 2),
        ("src/user.rs", 3),
    ] {
        assert_eq!(consumer_depth(&slice, path), Some(depth), "{slice}");
    }
}

#[test]
fn python_package_alias_retain_true_consumers() {
    let slice = slice_fixture(
        &[
            ("pkg/target.py", "def a():\n    return 1\n"),
            ("pkg/other.py", "def c():\n    return 2\n"),
            (
                "pkg/__init__.py",
                "from .target import a as renamed\nfrom .other import c\n",
            ),
            (
                "user.py",
                "from pkg import renamed as local\nprint(local())\n",
            ),
            ("unrelated.py", "from pkg import c\nprint(c())\n"),
        ],
        "pkg/target.py",
    );
    assert_eq!(consumer_depth(&slice, "pkg/__init__.py"), Some(1));
    assert_eq!(consumer_depth(&slice, "user.py"), Some(2), "{slice}");
    assert_eq!(consumer_depth(&slice, "unrelated.py"), None, "{slice}");
}

#[test]
fn cli_coverage_qualifies_wildcard_and_namespace_evidence() {
    let slice = slice_fixture(
        &[
            ("target.ts", "export const a = 1;"),
            ("barrel.ts", "export * from './target';"),
            (
                "user.ts",
                "import * as api from './barrel'; console.log(api.a);",
            ),
        ],
        "target.ts",
    );
    let coverage = &slice["consumerCoverage"];
    assert_eq!(coverage["status"], "partial", "{slice}");
    for reason in ["wildcard_reexports", "namespace_imports"] {
        assert!(
            coverage["diagnostics"]
                .as_array()
                .expect("diagnostics")
                .iter()
                .any(|row| row["reason"] == reason
                    && row["scope"] == "target_reachable"
                    && row["count"] == 1),
            "{coverage}"
        );
    }
    assert_eq!(consumer_depth(&slice, "user.ts"), Some(2));
}

#[test]
fn cli_coverage_unrelated_unresolved_is_global_not_missing_target_consumers() {
    let slice = slice_fixture(
        &[
            ("target.ts", "export const a = 1;"),
            ("barrel.ts", "export { a as alias } from './target';"),
            (
                "user.ts",
                "import { alias } from './barrel'; console.log(alias);",
            ),
            ("unrelated.ts", "export { missing } from './absent';"),
        ],
        "target.ts",
    );
    let coverage = &slice["consumerCoverage"];
    assert_eq!(coverage["status"], "modeled", "{slice}");
    let diagnostics = coverage["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(diagnostics.len(), 1, "{coverage}");
    assert_eq!(diagnostics[0]["reason"], "unresolved_reexports");
    assert_eq!(diagnostics[0]["scope"], "snapshot_global");
    assert_eq!(diagnostics[0]["count"], 1);
    assert_eq!(consumer_depth(&slice, "user.ts"), Some(2));
}

#[test]
fn cli_coverage_reachable_unresolved_keeps_useful_consumers() {
    let slice = slice_fixture(
        &[
            ("target.ts", "export const a = 1;"),
            (
                "barrel.ts",
                "export { a as alias } from './target'; export { missing } from './absent';",
            ),
            (
                "user.ts",
                "import { alias } from './barrel'; console.log(alias);",
            ),
        ],
        "target.ts",
    );
    let coverage = &slice["consumerCoverage"];
    assert_eq!(coverage["status"], "partial", "{slice}");
    assert_eq!(coverage["snapshot_reexports"], 2);
    for scope in ["snapshot_global", "target_reachable"] {
        assert!(
            coverage["diagnostics"]
                .as_array()
                .expect("diagnostics")
                .iter()
                .any(|row| row["reason"] == "unresolved_reexports"
                    && row["scope"] == scope
                    && row["count"] == 1),
            "{coverage}"
        );
    }
    assert_eq!(consumer_depth(&slice, "user.ts"), Some(2));
}

#[test]
fn cli_coverage_complete_modeled_alias_control_has_no_diagnostics() {
    let slice = slice_fixture(
        &[
            ("target.ts", "export const a = 1;"),
            ("barrel.ts", "export { a as alias } from './target';"),
            (
                "user.ts",
                "import { alias } from './barrel'; console.log(alias);",
            ),
        ],
        "target.ts",
    );
    assert_eq!(slice["consumerCoverage"]["status"], "modeled", "{slice}");
    assert_eq!(slice["consumerCoverage"]["compiler_complete"], false);
    assert_eq!(
        slice["consumerCoverage"]["diagnostics"],
        serde_json::json!([])
    );
    assert_eq!(consumer_depth(&slice, "barrel.ts"), Some(1));
    assert_eq!(consumer_depth(&slice, "user.ts"), Some(2));
}

#[test]
fn typescript_default_and_shadowed_star_bindings_are_not_propagated() {
    let slice = slice_fixture(
        &[
            ("target.ts", "export const a = 1; export default 42;"),
            ("other.ts", "export const a = 2; export default 99;"),
            (
                "bridge.ts",
                "export * from './target'; export { a, default } from './other';",
            ),
            (
                "user.ts",
                "import other, { a } from './bridge'; console.log(other, a);",
            ),
            (
                "default.ts",
                "export { default as selected } from './target';",
            ),
            (
                "positive.ts",
                "import { selected } from './default'; console.log(selected);",
            ),
        ],
        "target.ts",
    );
    assert_eq!(consumer_depth(&slice, "bridge.ts"), Some(1));
    assert_eq!(consumer_depth(&slice, "user.ts"), None, "{slice}");
    assert_eq!(consumer_depth(&slice, "positive.ts"), Some(2), "{slice}");
}

#[test]
fn typescript_star_does_not_expose_named_default_declaration() {
    let slice = slice_fixture(
        &[
            ("target.ts", "export default function namedDefault() {}"),
            ("bridge.ts", "export * from './target';"),
            (
                "not-exported.ts",
                "import { namedDefault } from './bridge'; namedDefault();",
            ),
            (
                "selected.ts",
                "export { default as selected } from './target';",
            ),
            (
                "positive.ts",
                "import { selected } from './selected'; selected();",
            ),
        ],
        "target.ts",
    );
    assert_eq!(consumer_depth(&slice, "bridge.ts"), Some(1));
    assert_eq!(consumer_depth(&slice, "not-exported.ts"), None, "{slice}");
    assert_eq!(consumer_depth(&slice, "positive.ts"), Some(2), "{slice}");
}

#[test]
fn swift_same_module_symbol_consumers_remain_visible() {
    let slice = slice_fixture(
        &[
            ("Target.swift", "struct Target {}"),
            ("Consumer.swift", "func makeTarget() -> Target { Target() }"),
        ],
        "Target.swift",
    );
    // The scanner also models an implicit same-module file edge; the immutable
    // BEFORE binary reports this edge alongside the symbol consumer as well.
    assert_eq!(consumer_depth(&slice, "Consumer.swift"), Some(1), "{slice}");
    assert_eq!(slice["consumers"].as_array().expect("consumers").len(), 1);
    assert!(
        slice["symbolConsumers"]
            .as_array()
            .expect("symbol consumers")
            .iter()
            .any(|row| {
                row["path"] == "Consumer.swift"
                    && row["symbols"]
                        .as_array()
                        .expect("symbols")
                        .iter()
                        .any(|name| name == "Target")
            }),
        "{slice}"
    );
}
