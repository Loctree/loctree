use assert_cmd::prelude::*;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

#[derive(Debug)]
enum LoctQuery<'a> {
    Literal(&'a str),
    Regex(&'a str),
    WhereSymbol(&'a str),
    WhoImports {
        target: &'a str,
        rg_fixed_query: &'a str,
    },
}

#[derive(Debug)]
struct Probe<'a> {
    class: &'a str,
    query: LoctQuery<'a>,
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scorecard_rg_parity")
}

fn loct() -> Command {
    Command::cargo_bin("loct").expect("loct binary should be built for integration tests")
}

fn normalize_rg_path(raw: &str) -> String {
    raw.trim_start_matches("./").to_string()
}

fn rg_counts(root: &Path, query: &str, regex: bool) -> BTreeMap<String, u64> {
    let mut command = Command::new("rg");
    command
        .current_dir(root)
        .args(["-o", "--hidden", "--glob", "!.git/**"]);
    if regex {
        command.args(["-e", query, "."]);
    } else {
        command.args(["--fixed-strings", query, "."]);
    }

    let output = command
        .output()
        .expect("scorecard gate requires ripgrep (`rg`) on PATH");

    if !output.status.success() {
        assert_eq!(
            output.status.code(),
            Some(1),
            "rg failed unexpectedly: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return BTreeMap::new();
    }

    let stdout = String::from_utf8(output.stdout).expect("rg output should be valid UTF-8");
    let mut counts = BTreeMap::new();
    for line in stdout.lines() {
        let (path, _) = line
            .split_once(':')
            .unwrap_or_else(|| panic!("rg -o output should be path:match, got `{line}`"));
        *counts.entry(normalize_rg_path(path)).or_insert(0) += 1;
    }
    counts
}

fn loct_json(root: &Path, args: &[&str]) -> Value {
    let cache = TempDir::new().expect("temp cache dir");
    let output = loct()
        .current_dir(root)
        .env("LOCT_CACHE_DIR", cache.path())
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    serde_json::from_slice(&output)
        .unwrap_or_else(|err| panic!("loct JSON parse failed for args {args:?}: {err}"))
}

fn literal_counts(root: &Path, query: &str) -> BTreeMap<String, u64> {
    let json = loct_json(
        root,
        &[
            "find",
            "--literal",
            query,
            "--group-by-file",
            "--count-only",
            "--json",
        ],
    );
    by_file_counts(&json["literal_matches"]["by_file"])
}

fn regex_counts(root: &Path, query: &str) -> BTreeMap<String, u64> {
    let json = loct_json(root, &["find", "--regex", query, "--json"]);
    occurrence_counts(&json["regex_matches"]["occurrences"])
}

fn where_symbol_counts(root: &Path, query: &str) -> BTreeMap<String, u64> {
    let json = loct_json(root, &["find", query, "--where-symbol", "--json"]);
    result_counts(&json["results"])
}

fn who_imports_counts(root: &Path, target: &str) -> BTreeMap<String, u64> {
    let json = loct_json(root, &["query", "who-imports", target, "--json"]);
    result_counts(&json["results"])
}

fn by_file_counts(value: &Value) -> BTreeMap<String, u64> {
    value
        .as_array()
        .expect("by_file should be an array")
        .iter()
        .map(|entry| {
            (
                entry["file"]
                    .as_str()
                    .expect("by_file entry file")
                    .to_string(),
                entry["count"].as_u64().expect("by_file entry count"),
            )
        })
        .collect()
}

fn occurrence_counts(value: &Value) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for occurrence in value.as_array().expect("occurrences should be an array") {
        let file = occurrence["file"]
            .as_str()
            .expect("occurrence file")
            .to_string();
        *counts.entry(file).or_insert(0) += 1;
    }
    counts
}

fn result_counts(value: &Value) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for result in value.as_array().expect("results should be an array") {
        let file = result["file"].as_str().expect("result file").to_string();
        *counts.entry(file).or_insert(0) += 1;
    }
    counts
}

fn assert_superset(class: &str, rg: &BTreeMap<String, u64>, loct: &BTreeMap<String, u64>) {
    assert!(
        !rg.is_empty(),
        "scorecard probe `{class}` must have rg ground truth"
    );
    let missing: Vec<String> = rg
        .iter()
        .filter_map(|(file, rg_count)| {
            let loct_count = loct.get(file).copied().unwrap_or(0);
            (loct_count < *rg_count).then(|| format!("{file}: loct={loct_count}, rg={rg_count}"))
        })
        .collect();

    assert!(
        missing.is_empty(),
        "loct lost rg hits for `{class}`: {missing:?}\nrg={rg:?}\nloct={loct:?}"
    );
}

fn file_presence(counts: BTreeMap<String, u64>) -> BTreeMap<String, u64> {
    counts.into_keys().map(|file| (file, 1)).collect()
}

#[test]
fn scorecard_rg_parity_fixture_matrix() {
    let root = fixture_root();
    let probes = [
        Probe {
            class: "identifier",
            query: LoctQuery::Literal("scorecard_worker_token"),
        },
        Probe {
            class: "prose",
            query: LoctQuery::Literal("scorecard prose phrase literal parity stays honest"),
        },
        Probe {
            class: "regex",
            query: LoctQuery::Regex("Scorecard[A-Za-z]+"),
        },
        Probe {
            class: "symbol-definition",
            query: LoctQuery::WhereSymbol("ScorecardWorker"),
        },
        Probe {
            class: "who-imports",
            query: LoctQuery::WhoImports {
                target: "src/alpha.rs",
                rg_fixed_query: "crate::alpha",
            },
        },
    ];

    for probe in probes {
        match probe.query {
            LoctQuery::Literal(query) => {
                let rg = rg_counts(&root, query, false);
                let loct = literal_counts(&root, query);
                assert_superset(probe.class, &rg, &loct);
            }
            LoctQuery::Regex(query) => {
                let rg = rg_counts(&root, query, true);
                let loct = regex_counts(&root, query);
                assert_superset(probe.class, &rg, &loct);
            }
            LoctQuery::WhereSymbol(query) => {
                let rg = file_presence(rg_counts(&root, query, false));
                let loct = where_symbol_counts(&root, query);
                assert_superset(probe.class, &rg, &loct);
            }
            LoctQuery::WhoImports {
                target,
                rg_fixed_query,
            } => {
                let rg = rg_counts(&root, rg_fixed_query, false);
                let loct = who_imports_counts(&root, target);
                assert_superset(probe.class, &rg, &loct);
            }
        }
    }
}
