//! Coverage must classify the exports actually emitted by the canonical parsers.
use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

struct Repo {
    root: TempDir,
    cache: TempDir,
}

impl Repo {
    fn new(files: &[(&str, &str)]) -> Self {
        let repo = Self {
            root: TempDir::new().expect("repo"),
            cache: TempDir::new().expect("cache"),
        };
        for (path, source) in files {
            let path = repo.root.path().join(path);
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            fs::write(path, source).expect("write source");
        }
        repo
    }

    fn coverage(&self, options: &[&str]) -> Value {
        let output = cargo_bin_cmd!("loct")
            .current_dir(self.root.path())
            .env("LOCT_CACHE_DIR", self.cache.path())
            .env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1")
            .args(["--quiet", "coverage", "--json"])
            .args(options)
            .output()
            .expect("coverage CLI");
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout).expect("JSON")
    }

    fn assert_half(&self) {
        let report = self.coverage(&["--tests"]);
        assert_eq!(report["coverage_percent"], 50.0, "{report}");
        let uncovered = report["exports_without_tests"]
            .as_array()
            .expect("inventory");
        assert_eq!(uncovered.len(), 1, "{report}");
        assert_eq!(uncovered[0]["symbol"], "untested");
        assert_eq!(report["test_file_count"], 1);
        // Default mode remains a list; combined mode retains named containers.
        let gaps = self.coverage(&[]);
        let gaps = gaps.as_array().expect("default gap list");
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        assert_eq!(gaps[0]["kind"], "export_without_test");
        assert_eq!(gaps[0]["target"], "untested");
        let combined = self.coverage(&["--tests", "--gaps"]);
        assert_eq!(combined["tests"], report);
        assert_eq!(combined["gaps"], Value::Array(gaps.clone()));
        assert!(combined["excluded_artifacts"].is_object());
    }
}

fn python_repo(import: &str) -> Repo {
    Repo::new(&[
        (
            "service.py",
            "def covered():\n    return 1\n\nasync def untested():\n    return 2\n\nclass Shape:\n    pass\nVALUE = 3\n",
        ),
        (
            "consumer_a.py",
            "from service import covered, untested, Shape, VALUE\n",
        ),
        (
            "consumer_b.py",
            "from service import covered, untested, Shape, VALUE\n",
        ),
        ("test_service.py", import),
    ])
}

#[test]
fn python_named_import_counts_def_exports_in_both_surfaces() {
    python_repo("from service import covered\n").assert_half();
}

#[test]
fn python_alias_credits_original_export_in_both_surfaces() {
    python_repo("from service import covered as subject\n").assert_half();
}

#[test]
fn python_without_tests_lists_functions_but_excludes_classes_and_values() {
    let repo = Repo::new(&[(
        "service.py",
        "def covered():\n    return 1\nasync def untested():\n    return 2\nclass Shape:\n    pass\nVALUE = 3\n",
    )]);
    let report = repo.coverage(&["--tests"]);
    assert_eq!(report["coverage_percent"], 0.0);
    assert_eq!(report["test_file_count"], 0);
    let names: Vec<_> = report["exports_without_tests"]
        .as_array()
        .expect("inventory")
        .iter()
        .map(|item| item["symbol"].as_str().expect("name"))
        .collect();
    assert_eq!(names.len(), 2, "{report}");
    assert!(names.contains(&"covered") && names.contains(&"untested"));
}

#[test]
fn typescript_functions_keep_the_same_coverage_and_gap_contract() {
    Repo::new(&[
        ("service.ts", "export function covered() { return 1; }\nexport function untested() { return 2; }\nexport interface Shape {}\nexport const VALUE = 3;\n"),
        ("consumer_a.ts", "import { covered, untested, Shape, VALUE } from './service';\n"),
        ("consumer_b.ts", "import { covered, untested, Shape, VALUE } from './service';\n"),
        ("service.test.ts", "import { covered as subject } from './service';\n"),
    ]).assert_half();
}

#[test]
fn rust_functions_keep_the_same_coverage_and_gap_contract() {
    Repo::new(&[
        ("Cargo.toml", "[package]\nname = \"coverage-contract\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
        ("src/lib.rs", "pub mod service;\nmod consumer_a;\nmod consumer_b;\n#[cfg(test)] mod service_test;\n"),
        ("src/service.rs", "pub fn covered() {}\npub fn untested() {}\npub struct Shape;\npub const VALUE: u8 = 3;\npub use std::io::Error;\nunsafe extern \"C\" { pub fn foreign(); }\n"),
        ("src/consumer_a.rs", "use crate::service::{covered, untested, Shape, VALUE, Error, foreign};\n"),
        ("src/consumer_b.rs", "use crate::service::{covered, untested, Shape, VALUE, Error, foreign};\n"),
        ("src/service_test.rs", "use crate::service::covered as subject;\n"),
    ]).assert_half();
}

#[test]
fn func_spelling_from_go_and_swift_parsers_is_testable() {
    for (path, source) in [
        (
            "service.go",
            "package service\nfunc Covered() {}\nfunc Untested() {}\ntype Shape struct {}\nconst VALUE = 3\n",
        ),
        (
            "Service.swift",
            "public func covered() {}\npublic func untested() {}\npublic struct Shape {}\npublic let value = 3\n",
        ),
    ] {
        let report = Repo::new(&[(path, source)]).coverage(&["--tests"]);
        assert_eq!(
            report["exports_without_tests"]
                .as_array()
                .expect("inventory")
                .len(),
            2,
            "{path}: {report}"
        );
    }
}

#[test]
fn no_eligible_behavior_preserves_zero_and_empty_inventory() {
    let report = Repo::new(&[(
        "service.ts",
        "export interface Shape {}\nexport const VALUE = 3;\n",
    )])
    .coverage(&["--tests"]);
    assert_eq!(report["coverage_percent"], 0.0);
    assert_eq!(report["exports_without_tests"], serde_json::json!([]));
}
