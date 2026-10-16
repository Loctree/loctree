//! Help-examples-as-tests: every jq example `--help-full` advertises must be
//! EXECUTABLE against a real snapshot — no nulls, no filter errors.
//!
//! Born from a live dead-end (2026-08-19): help promised `.dead_parrots[]`
//! and `.cycles[]`, README promised `.summary.health_score`, while the
//! snapshot's top-level keys were barrels/edges/files/metadata/... — agents
//! got "cannot use null as iterable" from the product's own front door.
//! A help surface that advertises a contract the engine refuses is a layer
//! lying about the layer below; this gate makes that class unrepresentable
//! for the JQ QUERIES section.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

fn fixtures_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn loct(cache: &std::path::Path) -> Command {
    let mut cmd = cargo_bin_cmd!("loct");
    cmd.env("LOCT_OPEN_BROWSER", "0");
    cmd.env("LOCT_CACHE_DIR", cache);
    cmd.env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1");
    cmd
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Parse one advertised `loct '<filter>' [flags]   <description>` help line
/// into the argv it promises.
///
/// The WHOLE argv is recovered, not just the quoted filter: an example that
/// needs `--artifact findings` makes a different promise than the bare filter,
/// and silently dropping the flag would let the help advertise a contract this
/// gate never actually runs. The help renders its description in a column
/// separated by two-or-more spaces — that column boundary ends the argv.
fn parse_example(line: &str) -> Option<Vec<String>> {
    let rest = line.trim().strip_prefix("loct '")?;
    let end = rest.find('\'')?;
    let mut args = vec![rest[..end].to_string()];
    let tail = &rest[end + 1..];
    let argv_tail = match tail.find("  ") {
        Some(i) => &tail[..i],
        None => tail,
    };
    args.extend(argv_tail.split_whitespace().map(str::to_string));
    Some(args)
}

/// Extract `loct '<query>' ...` examples from the JQ QUERIES help section.
fn advertised_jq_examples(help: &str) -> Vec<Vec<String>> {
    let section = help
        .split("=== JQ QUERIES")
        .nth(1)
        .expect("--help-full must carry a JQ QUERIES section")
        .split("\n=== ")
        .next()
        .unwrap();
    section.lines().filter_map(parse_example).collect()
}

#[test]
fn occurrences_help_separates_literal_inclusion_from_role_metadata() {
    let cache = TempDir::new().expect("help cache");
    for args in [vec!["occurrences", "--help"], vec!["--help-full"]] {
        let output = loct(cache.path())
            .args(args)
            .assert()
            .success()
            .get_output()
            .clone();
        let help = String::from_utf8_lossy(&output.stdout);
        assert!(help.contains("Literal inclusion does not depend on symbol extraction"));
        assert!(help.contains("Role metadata may use source AST and snapshot anchors"));
        assert!(help.contains("No fuzzy suggestions are promoted as primary"));
        assert!(!help.contains("it does not consult the AST/tagmap"));
    }
}

#[test]
fn every_advertised_jq_example_executes_against_a_real_snapshot() {
    let help = loct(&TempDir::new().unwrap().path().join("cache"))
        .arg("--help-full")
        .output()
        .expect("run --help-full");
    let help_text = String::from_utf8_lossy(&help.stdout).to_string();
    let examples = advertised_jq_examples(&help_text);
    assert!(
        examples.len() >= 3,
        "JQ QUERIES section should advertise at least 3 runnable examples, found {examples:?}"
    );
    assert!(
        examples.iter().any(|args| args.len() > 1),
        "JQ QUERIES advertises no flagged example — the --artifact surface is unproven: {examples:?}"
    );

    let cache = TempDir::new().unwrap();
    // Work on a copy: the full pass writes a context atlas into the working
    // directory, and the checked-in fixture is not a scratch space.
    let work = TempDir::new().unwrap();
    let fixture = work.path().join("simple_ts");
    copy_tree(&fixtures_path().join("simple_ts"), &fixture);
    // Full pass, not `scan`: the advertised --artifact examples read the
    // sibling artifacts, which only a full run materializes.
    loct(cache.path()).current_dir(&fixture).assert().success();

    for args in &examples {
        let out = loct(cache.path())
            .current_dir(&fixture)
            .args(args)
            .output()
            .expect("run advertised jq example");
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            out.status.success(),
            "advertised example `loct {args:?}` failed: {stderr}"
        );
        assert!(
            !stdout.is_empty() && stdout != "null",
            "advertised example `loct {args:?}` answered {stdout:?} — \
             the help promises a contract the snapshot does not carry"
        );
    }
}

fn file_contents(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(root).expect("read help sandbox") {
        let entry = entry.expect("read sandbox entry");
        if entry.file_type().expect("inspect sandbox entry").is_dir() {
            for (path, contents) in file_contents(&entry.path()) {
                files.insert(PathBuf::from(entry.file_name()).join(path), contents);
            }
        } else {
            files.insert(
                PathBuf::from(entry.file_name()),
                std::fs::read(entry.path()).expect("read sandbox file"),
            );
        }
    }
    files
}

#[test]
fn advertised_commands_have_successful_help_without_side_effects() {
    let work = TempDir::new().expect("create help sandbox");
    let root = work.path();
    let cache = root.join("cache");
    std::fs::create_dir(root.join(".loctree")).expect("create configuration directory");
    std::fs::write(
        root.join(".loctree/config.toml"),
        "invalid configuration sentinel",
    )
    .expect("write invalid configuration sentinel");
    std::fs::write(root.join(".loctree/snapshot.old@00000000.json"), "keep me")
        .expect("write prune sentinel");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let bin = root.join("bin");
        std::fs::create_dir(&bin).expect("create browser sentinel directory");
        for name in ["open", "xdg-open"] {
            let path = bin.join(name);
            std::fs::write(
                &path,
                "#!/bin/sh\nprintf browser-invoked > browser-was-opened\nexit 99\n",
            )
            .expect("write browser invocation sentinel");
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                .expect("make browser sentinel executable");
        }
    }
    let before = file_contents(root);
    let full = loct(&cache)
        .current_dir(root)
        .arg("--help-full")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let full = String::from_utf8(full).expect("full help is UTF-8");
    let names: std::collections::BTreeSet<_> = full
        .lines()
        .filter_map(|line| line.trim().strip_prefix("loct "))
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| loctree::cli::parser::is_subcommand(name))
        .collect();
    assert!(
        names.len() >= 50,
        "full reference hides live commands: {names:?}"
    );
    for name in names {
        for args in [
            vec![name, "--help"],
            vec!["help", name],
            vec![name, "-h"],
            vec!["--help", name],
        ] {
            let expected = loctree::cli::command::Command::format_command_help(name)
                .expect("advertised command has topic help");
            let output = loct(&cache)
                .current_dir(root)
                .env("PATH", root.join("bin"))
                .env("LOCT_OPEN_BROWSER", "1")
                .timeout(Duration::from_secs(5))
                .args(&args)
                .assert()
                .success()
                .get_output()
                .clone();
            assert_eq!(
                String::from_utf8(output.stdout)
                    .expect("topic help is UTF-8")
                    .trim(),
                expected.trim(),
                "{args:?}"
            );
            assert!(output.stderr.is_empty(), "{args:?}: {:?}", output.stderr);
            assert_eq!(file_contents(root), before, "help changed files: {args:?}");
        }
    }
    for args in [
        vec!["prune-old-artifacts", "--apply", "--keep", "0", "--help"],
        vec!["cache", "clean", "--all", "--force", "--help"],
        vec!["report", "--serve", "--html", "generated.html", "--help"],
        vec!["watch", "--background", "--http", "--help"],
        vec!["plan", "--all", "--output", "migration", "--help"],
    ] {
        loct(&cache)
            .current_dir(root)
            .env("PATH", root.join("bin"))
            .env("LOCT_OPEN_BROWSER", "1")
            .timeout(Duration::from_secs(5))
            .args(&args)
            .assert()
            .success();
        assert_eq!(
            file_contents(root),
            before,
            "help executed an action: {args:?}"
        );
    }
}

#[test]
fn discovery_help_names_actual_limits() {
    let full = loctree::cli::command::Command::format_help_full();
    for name in ["info", "plan", "prune-old-artifacts"] {
        assert!(
            full.contains(&format!("loct {name} ")),
            "full reference hides {name}"
        );
    }
    let routes = loctree::cli::command::Command::format_command_help("routes")
        .expect("routes has topic help");
    for boundary in ["Axum", "method-chain", "TypeScript", "not proof"] {
        assert!(routes.contains(boundary), "routes help omits {boundary}");
    }
    let plan =
        loctree::cli::command::Command::format_command_help("plan").expect("plan has topic help");
    assert!(plan.contains("path heuristics"));
    assert!(plan.contains("not proof"));
    assert!(!plan.contains("safe refactoring plan"));
}

// Exercise the advertised degree/filter contract through the actual parser and CLI.
mod hotspots_contract {
    use super::*;
    use serde_json::{Value, json};
    use std::fs;

    fn mixed_project(root: &Path) {
        for (name, source) in [
            (
                "entry.ts",
                "import { shared } from './shared';\nimport { shared as again } from './shared';\nimport { forwarded } from './barrel';\nexport function entry() { return shared() + again() + forwarded(); }\n",
            ),
            (
                "barrel.ts",
                "export { shared as forwarded } from './shared';\n",
            ),
            ("shared.ts", "export function shared() { return 1; }\n"),
            ("main.rs", "mod helper;\nfn main() { helper::run(); }\n"),
            ("helper.rs", "pub fn run() {}\n"),
            (
                "external.py",
                "if __name__ == '__main__':\n    print('external entry')\n",
            ),
        ] {
            fs::write(root.join(name), source).expect("write mixed graph fixture");
        }
    }

    fn rows(root: &Path, cache: &Path, flags: &[&str]) -> Value {
        let output = loct(cache)
            .current_dir(root)
            .args(["hotspots", "--json"])
            .args(flags)
            .assert()
            .success()
            .get_output()
            .clone();
        serde_json::from_slice(&output.stdout).expect("hotspot rows JSON")
    }

    #[test]
    fn hotspots_default_includes_external_entrypoint_and_mixed_edge_neighbors() {
        let project = TempDir::new().expect("project");
        let cache = TempDir::new().expect("cache");
        mixed_project(project.path());
        // Imports, re-exports and Rust module/call edges all contribute; repeated
        // edges from one file never inflate the number of known neighbors.
        let actual = rows(project.path(), cache.path(), &[]);
        assert_eq!(
            actual,
            json!([
                {"path":"shared.ts", "in_degree":2, "out_degree":0, "loc":1, "category":"PERIPHERAL"},
                {"path":"barrel.ts", "in_degree":1, "out_degree":1, "loc":1, "category":"PERIPHERAL"},
                {"path":"helper.rs", "in_degree":1, "out_degree":0, "loc":1, "category":"PERIPHERAL"},
                {"path":"entry.ts", "in_degree":0, "out_degree":2, "loc":4, "category":"LEAF"},
                {"path":"external.py", "in_degree":0, "out_degree":0, "loc":2, "category":"LEAF"},
                {"path":"main.rs", "in_degree":0, "out_degree":1, "loc":2, "category":"LEAF"}
            ])
        );
        assert_eq!(actual, rows(project.path(), cache.path(), &["--min", "0"]));
    }

    #[test]
    fn hotspots_min_and_leaves_select_documented_rows() {
        let project = TempDir::new().expect("project");
        let cache = TempDir::new().expect("cache");
        mixed_project(project.path());
        let filtered = rows(project.path(), cache.path(), &["--min", "2"]);
        assert_eq!(filtered.as_array().expect("array").len(), 1);
        assert_eq!(filtered[0]["path"], "shared.ts");
        let leaves = rows(project.path(), cache.path(), &["--leaves"]);
        let paths: Vec<_> = leaves
            .as_array()
            .expect("array")
            .iter()
            .map(|row| row["path"].as_str().expect("path"))
            .collect();
        assert_eq!(paths, ["entry.ts", "external.py", "main.rs"]);
        assert_eq!(
            leaves,
            rows(project.path(), cache.path(), &["--min", "2", "--leaves"])
        );
        let text = loct(cache.path())
            .current_dir(project.path())
            .args(["hotspots", "--leaves", "--min", "2", "--coupling"])
            .assert()
            .success()
            .get_output()
            .clone();
        let text = String::from_utf8_lossy(&text.stdout);
        assert!(text.contains("[in:0   out:2  ] entry.ts"));
        assert!(text.contains("LEAF (0 incoming neighbors)"));
        assert!(text.contains("overrides --min"));
        assert!(text.contains("low degree can include important entry points"));
        assert!(!text.contains("importers"));
    }

    #[test]
    fn hotspots_default_limit_is_fifty_including_zero_degree_files() {
        let project = TempDir::new().expect("project");
        let cache = TempDir::new().expect("cache");
        for i in 0..55 {
            fs::write(
                project.path().join(format!("file{i:02}.ts")),
                "export const value = 1;\n",
            )
            .expect("write leaf");
        }
        let default = rows(project.path(), cache.path(), &[]);
        assert_eq!(default.as_array().expect("array").len(), 50);
        assert_eq!(default[49]["path"], "file49.ts");
        assert_eq!(
            rows(project.path(), cache.path(), &["--limit", "60"])
                .as_array()
                .expect("array")
                .len(),
            55
        );
    }

    #[test]
    fn hotspots_topic_and_full_help_qualify_static_degree() {
        let cache = TempDir::new().expect("cache");
        for args in [vec!["hotspots", "--help"], vec!["--help-full"]] {
            let full = args == ["--help-full"];
            let output = loct(cache.path())
                .args(args)
                .assert()
                .success()
                .get_output()
                .clone();
            let help = String::from_utf8_lossy(&output.stdout);
            for promised in [
                "source neighbors",
                "target neighbors",
                "not architectural roles",
                "runtime entry point",
                "performance score",
                "safety certificate",
                "default: 0",
                "default: 50",
                "overrides --min",
            ] {
                assert!(help.contains(promised), "missing promise: {promised}");
            }
            if full {
                assert!(help.contains("Static indexed snapshot degree; all edge kinds, distinct known-file (from,to) pairs"));
                assert!(help.contains("High degree suggests impact/follow inspection"));
                assert!(help.contains("No runtime call counts"));
            } else {
                assert!(help.contains("distinct known-file source neighbors"));
                assert!(help.contains("distinct known-file target neighbors"));
                assert!(help.contains("All snapshot edge kinds"));
                assert!(help.contains("static indexed snapshot"));
                assert!(help.contains("High degree suggests inspecting impact/follow"));
                assert!(help.contains("not a runtime call count"));
            }
            for misleading in [
                "safe to refactor",
                "critical infrastructure",
                "possibly dead",
                "Import frequency heatmap",
            ] {
                assert!(
                    !help.contains(misleading),
                    "misleading hotspot promise: {misleading}"
                );
            }
        }
    }
}
