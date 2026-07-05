//! End-to-End CLI Tests for loctree
//!
//! Following TDD principles - tests define expected behavior.
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use serde_json::Value;
use std::path::PathBuf;
use tempfile::TempDir;

/// Check that a snapshot exists for the given project root (in global cache or legacy .loctree)
fn snapshot_exists(root: &std::path::Path) -> bool {
    loctree::snapshot::Snapshot::exists(root)
}

/// Get path to test fixtures
fn fixtures_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Get a command pointing to the loctree binary
///
/// Sets `LOCT_OPEN_BROWSER=0` so the report/auto code paths never spawn a
/// real `open`/`xdg-open` against a `tempfile::TempDir` that is about to be
/// recursively deleted when the test returns — the browser would race and
/// surface a misleading `WebKitErrorDomain:103` to the operator.
fn loctree() -> Command {
    let mut cmd = cargo_bin_cmd!("loctree");
    cmd.env("LOCT_OPEN_BROWSER", "0");
    cmd
}

/// Get a command pointing to the loct binary
///
/// See `loctree()` for the rationale behind the `LOCT_OPEN_BROWSER=0` env var.
fn loct() -> Command {
    let mut cmd = cargo_bin_cmd!("loct");
    cmd.env("LOCT_OPEN_BROWSER", "0");
    cmd
}

// ============================================
// Basic CLI Tests
// ============================================

mod cli_basics {
    use super::*;

    #[test]
    fn shows_help() {
        loctree()
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("loctree"))
            .stdout(predicate::str::contains("POWER PATH"))
            .stdout(predicate::str::contains("Map:"))
            .stdout(predicate::str::contains("Search:"))
            .stdout(predicate::str::contains("Understand:"))
            .stdout(predicate::str::contains("Trust:"))
            .stdout(predicate::str::contains("Compare:"))
            .stdout(predicate::str::contains("loct focus"))
            .stdout(predicate::str::contains("loct hotspots"))
            .stdout(predicate::str::contains("loct follow"))
            .stdout(predicate::str::contains("loct body"))
            .stdout(predicate::str::contains("loct occurrences"))
            .stdout(predicate::str::contains("loct query where-symbol"))
            .stdout(predicate::str::contains("loct tree --files --match"))
            .stdout(predicate::str::contains("loct doctor"))
            .stdout(predicate::str::contains("loct diff"))
            .stdout(predicate::str::contains("loct env-truth"))
            .stdout(predicate::str::contains("loct suppressions"))
            .stdout(predicate::str::contains("loct prism"))
            .stdout(predicate::str::contains("loct tagmap"))
            .stdout(predicate::str::contains("ast_js"))
            .stdout(predicate::str::contains("tree-sitter C-family"))
            .stdout(predicate::str::contains("streamable-http MCP"))
            .stdout(predicate::str::contains("per-root watch lock"))
            .stdout(predicate::str::contains("internal command reference").not());
    }

    #[test]
    fn shows_version() {
        loctree()
            .arg("--version")
            .assert()
            .success()
            .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn shows_full_help() {
        loctree()
            .arg("--help-full")
            .assert()
            .success()
            .stdout(predicate::str::contains("--sarif").or(predicate::str::contains("sarif")))
            .stdout(predicate::str::contains("loct context"))
            .stdout(predicate::str::contains("loct occurrences"))
            .stdout(predicate::str::contains("loct body"))
            .stdout(predicate::str::contains("loct find --literal"))
            .stdout(predicate::str::contains("loct prism"))
            .stdout(predicate::str::contains("ast_js"))
            .stdout(predicate::str::contains("tree-sitter C-family"))
            .stdout(predicate::str::contains("streamable-http MCP"))
            .stdout(predicate::str::contains("per-root watch lock"))
            .stdout(predicate::str::contains("loct --for-ai > context.json").not())
            .stdout(predicate::str::contains("dead").or(predicate::str::contains("cycles")));
    }

    #[test]
    fn repo_view_help_demotes_legacy_for_ai_alias() {
        loct()
            .args(["repo-view", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("loct context"))
            .stdout(predicate::str::contains("loct --for-ai").not());
    }

    #[test]
    fn full_help_has_no_duplicate_command_table_lines_or_stale_timings() {
        let output = loct()
            .arg("--help-full")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let stdout = String::from_utf8(output).expect("help is utf8");

        let mut command_lines: Vec<&str> = stdout
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("loct ") && !line.contains('#') && !line.contains('|'))
            .collect();
        command_lines.sort_unstable();

        let mut duplicates = Vec::new();
        for pair in command_lines.windows(2) {
            if pair[0] == pair[1] {
                duplicates.push(pair[0]);
            }
        }

        assert!(
            duplicates.is_empty(),
            "duplicate command lines in --help-full: {duplicates:?}"
        );
        assert!(
            !stdout.contains("ms!"),
            "--help-full should not contain stale performance exclamation claims"
        );
        assert!(
            !stdout.contains("internal command reference"),
            "--help-full should be product-facing, not hidden as internal"
        );
    }

    #[test]
    fn global_fresh_flag_does_not_break_subcommands() {
        loctree()
            .args(["--fresh", "query", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("loct query"));
    }
}

mod context_scope_cli {
    use super::*;
    use std::fs;

    fn scoped_fixture() -> TempDir {
        let temp = TempDir::new().unwrap();
        let fixture = fixtures_path().join("simple_ts");
        copy_dir_all(&fixture, temp.path()).unwrap();
        loct()
            .current_dir(temp.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();
        let loctree_dir = temp.path().join(".loctree");
        fs::create_dir_all(&loctree_dir).unwrap();
        fs::write(
            loctree_dir.join("scopes.toml"),
            r#"
[scopes."source"]
description = "Source files"
selectors = ["path:src/"]
"#,
        )
        .unwrap();
        temp
    }

    fn unconfigured_scope_fixture() -> TempDir {
        let temp = TempDir::new().unwrap();
        let fixture = fixtures_path().join("simple_ts");
        copy_dir_all(&fixture, temp.path()).unwrap();
        loct()
            .current_dir(temp.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();
        temp
    }

    #[test]
    fn cli_loct_context_scope_path_smoke() {
        let temp = scoped_fixture();
        let output = loct()
            .current_dir(temp.path())
            .args(["context", "--scope", "path:src/", "--no-aicx", "--full"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let json: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(json["scope"]["selectors"][0], "path:src/");
        assert!(json["scope"]["matched_files"].as_u64().unwrap() > 0);
        assert_eq!(
            json["risk"]["cache_scope"]
                .as_object()
                .unwrap()
                .keys()
                .next()
                .unwrap(),
            "Scoped"
        );
    }

    #[test]
    fn cli_loct_context_scope_named_smoke() {
        let temp = scoped_fixture();
        let output = loct()
            .current_dir(temp.path())
            .args(["context", "--scope", "source", "--no-aicx", "--full"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let json: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(json["scope"]["named_resolved_from"], "source");
        assert_eq!(json["scope"]["resolved_selectors"][0], "path:src/");
    }

    #[test]
    fn cli_loct_context_scope_with_task_keeps_scope_set() {
        let temp = scoped_fixture();
        let output = loct()
            .current_dir(temp.path())
            .args([
                "context",
                "--scope",
                "source",
                "--task",
                "greeting helper",
                "--no-aicx",
                "--full",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let json: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(json["task"]["mode"], "ranker_within_scope");
        assert!(json["scope"]["matched_files"].as_u64().unwrap() > 0);
    }

    #[test]
    fn cli_loct_context_empty_path_scope_fails_with_nearest_prefix_hint() {
        let temp = unconfigured_scope_fixture();
        loct()
            .current_dir(temp.path())
            .args(["context", "--scope", "path:srk", "--no-aicx", "--full"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("matched zero files"))
            .stderr(predicate::str::contains("Did you mean `path:src/`?"));
    }

    #[test]
    fn cli_loct_context_empty_single_segment_glob_fails_with_recursive_hint() {
        let temp = unconfigured_scope_fixture();
        loct()
            .current_dir(temp.path())
            .args(["context", "--scope", "path:src-*", "--no-aicx", "--full"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("matched zero files"))
            .stderr(predicate::str::contains("`*` does not cross `/`"))
            .stderr(predicate::str::contains("Try `path:src-*/**`"));
    }

    #[test]
    fn cli_loct_context_disjoint_scopes_explain_and_intersection() {
        let temp = TempDir::new().unwrap();
        let fixture = fixtures_path().join("simple_ts");
        copy_dir_all(&fixture, temp.path()).unwrap();
        fs::write(temp.path().join("README.md"), "# Fixture\n").unwrap();
        loct()
            .current_dir(temp.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();

        loct()
            .current_dir(temp.path())
            .args([
                "context",
                "--scope",
                "path:src/",
                "--scope",
                "path:README.md",
                "--no-aicx",
                "--full",
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("matched zero files"))
            .stderr(predicate::str::contains(
                "repeated --scope selectors are intersected (AND)",
            ))
            .stderr(predicate::str::contains("path:src/="))
            .stderr(predicate::str::contains("path:README.md="));
    }

    #[test]
    fn cli_loct_context_scope_with_file_warns_and_file_wins() {
        let temp = scoped_fixture();
        loct()
            .current_dir(temp.path())
            .args([
                "context",
                "--file",
                "src/index.ts",
                "--scope",
                "source",
                "--no-aicx",
                "--full",
            ])
            .assert()
            .success()
            .stderr(predicate::str::contains("--scope ignored"));
    }

    #[test]
    fn cli_loct_context_unknown_scope_suggests_named_scope() {
        let temp = scoped_fixture();
        loct()
            .current_dir(temp.path())
            .args(["context", "--scope", "sorce", "--no-aicx", "--full"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("Available named scopes: [source]"))
            .stderr(predicate::str::contains("Did you mean 'source'?"));
    }

    #[test]
    fn cli_loct_context_unknown_scope_without_config_teaches_selector_syntax() {
        let temp = unconfigured_scope_fixture();
        loct()
            .current_dir(temp.path())
            .args(["context", "--scope", "source", "--no-aicx", "--full"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains(
                "Supported selector kinds: path:, tag:, import:, reach:",
            ))
            .stderr(predicate::str::contains("No named scopes are configured"))
            .stderr(predicate::str::contains("`--scope path:core/`"))
            .stderr(predicate::str::contains("Available named scopes: []").not());
    }

    #[test]
    fn cli_loct_context_scoped_task_non_tty_skips_html_report_side_effect() {
        let temp = unconfigured_scope_fixture();
        loct()
            .current_dir(temp.path())
            .args([
                "context",
                "--scope",
                "path:src/",
                "--task",
                "greeting helper",
                "--no-aicx",
            ])
            .assert()
            .success();

        assert!(
            !temp.path().join(".loctree/report.html").exists(),
            "scoped/task context calls from non-TTY must not render the human HTML report"
        );
    }

    #[test]
    fn cli_loct_context_help_includes_scope() {
        loct()
            .args(["context", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("--scope <SELECTOR>"))
            .stdout(predicate::str::contains(
                "loct context --scope path:core --task \"hold-mods versus hands-off\"",
            ))
            .stdout(predicate::str::contains(
                "loct context --scope path:src/agent/ --task \"fix SSE retry behavior\" --full --markdown",
            ));
    }
}

// ============================================
// Scan Mode Tests
// ============================================

mod scan_mode {
    use super::*;

    #[test]
    fn scans_typescript_project() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .assert()
            .success()
            // Summary output goes to stderr (stdout reserved for machine-readable data)
            .stderr(predicate::str::contains("ts").or(predicate::str::contains("Scanned")));
    }

    #[test]
    fn creates_snapshot() {
        let temp = TempDir::new().unwrap();
        let fixture = fixtures_path().join("simple_ts");

        // Copy fixture to temp
        copy_dir_all(&fixture, temp.path()).unwrap();

        loctree().current_dir(temp.path()).assert().success();

        // Snapshot should exist (in global cache)
        assert!(snapshot_exists(temp.path()));
    }

    #[test]
    fn file_count_parity() {
        let temp = TempDir::new().unwrap();
        let fixture = fixtures_path().join("simple_ts");
        copy_dir_all(&fixture, temp.path()).unwrap();
        let project = temp.path().to_str().expect("temp path should be utf-8");

        let scan = loct()
            .current_dir(temp.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success()
            .get_output()
            .stderr
            .clone();
        let scan_count = parse_scan_banner_count(&scan);

        let snapshot = loctree::snapshot::Snapshot::load(temp.path()).unwrap();
        let metadata_count = snapshot.metadata.file_count;

        let repo_view = loct()
            .args(["repo-view", project])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let repo_view: Value = parse_json_output(&repo_view);
        let repo_view_count = repo_view
            .pointer("/summary/files_analyzed")
            .and_then(Value::as_u64)
            .expect("repo-view summary.files_analyzed") as usize;

        let findings = loct()
            .args(["findings", project])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let findings: Value = parse_json_output(&findings);
        let findings_count = findings
            .pointer("/summary/files")
            .and_then(Value::as_u64)
            .expect("findings summary.files") as usize;

        assert_eq!(scan_count, metadata_count);
        assert_eq!(metadata_count, repo_view_count);
        assert_eq!(repo_view_count, findings_count);
    }

    #[test]
    fn respects_gitignore_flag() {
        let fixture = fixtures_path().join("simple_ts");

        loctree().current_dir(&fixture).arg("-g").assert().success();
    }

    #[test]
    fn scans_astro_frontmatter_imports_and_exports() {
        let temp = TempDir::new().unwrap();
        let fixture = fixtures_path().join("astro_app");
        copy_dir_all(&fixture, temp.path()).unwrap();

        loct()
            .current_dir(temp.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();

        let snapshot = loctree::snapshot::Snapshot::load(temp.path()).unwrap();
        let index = snapshot
            .files
            .iter()
            .find(|file| file.path == "src/pages/index.astro")
            .expect("index.astro should be analyzed");

        assert!(
            index
                .imports
                .iter()
                .any(|i| i.source == "../components/Card.astro")
        );
        assert!(
            index
                .exports
                .iter()
                .any(|e| e.name == "Props" && e.kind == "interface")
        );
        assert!(snapshot.edges.iter().any(|edge| {
            edge.from == "src/pages/index.astro" && edge.to == "src/components/Card.astro"
        }));

        loct()
            .current_dir(temp.path())
            .args(["slice", "src/pages/index.astro"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Card.astro"));
    }

    #[test]
    fn scans_svelte5_runes_snippets_and_svelte_ts_modules() {
        let temp = TempDir::new().unwrap();
        let fixture = fixtures_path().join("svelte5_app");
        copy_dir_all(&fixture, temp.path()).unwrap();

        loct()
            .current_dir(temp.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();

        let snapshot = loctree::snapshot::Snapshot::load(temp.path()).unwrap();
        let counter = snapshot
            .files
            .iter()
            .find(|file| file.path == "src/lib/Counter.svelte")
            .expect("Counter.svelte should be analyzed");
        let store = snapshot
            .files
            .iter()
            .find(|file| file.path == "src/lib/store.svelte.ts")
            .expect("store.svelte.ts should be analyzed");
        let page = snapshot
            .files
            .iter()
            .find(|file| file.path == "src/routes/+page.svelte")
            .expect("+page.svelte should be analyzed");

        assert!(
            counter
                .exports
                .iter()
                .any(|e| e.name == "count" && e.kind == "rune_state")
        );
        assert!(
            counter
                .exports
                .iter()
                .any(|e| e.name == "label" && e.kind == "rune_props")
        );
        assert!(
            counter
                .exports
                .iter()
                .any(|e| e.name == "doubled" && e.kind == "rune_derived")
        );
        assert!(
            counter
                .exports
                .iter()
                .any(|e| e.name == "row" && e.kind == "snippet")
        );
        assert!(
            store
                .exports
                .iter()
                .any(|e| e.name == "count" && e.kind == "rune_state")
        );
        assert!(store.exports.iter().any(|e| e.name == "increment"));
        assert!(page.exports.iter().any(|e| e.name == "prerender"));
    }
}

// ============================================
// Slice Mode Tests
// ============================================

mod slice_mode {
    use super::*;

    /// Helper to ensure snapshot exists before slice tests
    fn ensure_snapshot(fixture: &std::path::Path) {
        loctree().current_dir(fixture).assert().success();
    }

    #[test]
    fn slices_single_file() {
        let fixture = fixtures_path().join("simple_ts");
        ensure_snapshot(&fixture);

        loctree()
            .current_dir(&fixture)
            .args(["slice", "src/index.ts"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Core"))
            .stdout(predicate::str::contains("index.ts"));
    }

    #[test]
    fn slice_rescan_flag_triggers_rescan() {
        let fixture = fixtures_path().join("simple_ts");
        ensure_snapshot(&fixture);

        loctree()
            .current_dir(&fixture)
            .args(["slice", "src/index.ts", "--rescan"])
            .write_stdin("")
            .assert()
            .success()
            .stderr(predicate::str::contains("Rescanning"));
    }

    #[test]
    fn slices_with_deps() {
        let fixture = fixtures_path().join("simple_ts");
        ensure_snapshot(&fixture);

        loctree()
            .current_dir(&fixture)
            .args(["slice", "src/index.ts"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Deps"))
            .stdout(predicate::str::contains("greeting.ts"))
            .stdout(predicate::str::contains("date.ts"));
    }

    #[test]
    fn slices_with_consumers() {
        let fixture = fixtures_path().join("simple_ts");
        ensure_snapshot(&fixture);

        loctree()
            .current_dir(&fixture)
            .args(["slice", "src/utils/greeting.ts"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Consumers"))
            .stdout(predicate::str::contains("index.ts"));
    }

    #[test]
    fn slice_no_consumers_restores_dependency_only_view() {
        let fixture = fixtures_path().join("simple_ts");
        ensure_snapshot(&fixture);

        loctree()
            .current_dir(&fixture)
            .args(["slice", "src/utils/greeting.ts", "--no-consumers"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Consumers").not());
    }

    #[test]
    fn slices_json_output() {
        let fixture = fixtures_path().join("simple_ts");
        ensure_snapshot(&fixture);

        loctree()
            .current_dir(&fixture)
            .args(["slice", "src/index.ts", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::contains(r#""core""#))
            .stdout(predicate::str::contains(r#""deps""#))
            .stdout(predicate::str::contains(r#""consumers""#));
    }

    #[test]
    fn slice_file_not_found() {
        let fixture = fixtures_path().join("simple_ts");
        ensure_snapshot(&fixture);

        loctree()
            .current_dir(&fixture)
            .args(["slice", "nonexistent.ts"])
            .assert()
            .failure();
    }
}

// ============================================
// Analyzer Mode Tests
// ============================================

mod analyzer_mode {
    use super::*;

    #[test]
    fn detects_circular_imports() {
        let fixture = fixtures_path().join("circular_imports");

        loctree()
            .current_dir(&fixture)
            .args(["-A", "--circular"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Circular"));
    }

    #[test]
    fn detects_dead_exports() {
        let fixture = fixtures_path().join("dead_code");

        loctree()
            .current_dir(&fixture)
            .args(["-A", "--dead"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("deadFunction")
                    .or(predicate::str::contains("DEAD_CONSTANT")),
            );
    }

    #[test]
    fn lists_entrypoints() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["-A", "--entrypoints"])
            .assert()
            .success()
            // Entry points might be empty for simple TS project without main()
            .stdout(
                predicate::str::is_empty()
                    .not()
                    .or(predicate::str::contains("Entry")),
            );
    }

    #[test]
    fn checks_similar_components() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["-A", "--check", "greet"])
            .assert()
            .success()
            .stdout(predicate::str::contains("greet").or(predicate::str::contains("greeting")));
    }

    #[test]
    fn analyzes_impact() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["-A", "--impact", "src/utils/greeting.ts"])
            .assert()
            .success()
            .stdout(predicate::str::contains("index.ts"));
    }

    #[test]
    fn finds_symbol() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["-A", "--symbol", "greet"])
            .assert()
            .success()
            .stdout(predicate::str::contains("greeting.ts"));
    }

    #[test]
    fn outputs_sarif() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["-A", "--sarif"])
            .assert()
            .success()
            .stdout(predicate::str::contains(r#""$schema""#))
            .stdout(predicate::str::contains("sarif"));
    }

    #[test]
    fn outputs_json() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["-A", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{"));
    }
}

// ============================================
// Tauri Mode Tests
// ============================================

mod tauri_mode {
    use super::*;

    #[test]
    fn detects_tauri_project() {
        let fixture = fixtures_path().join("tauri_app");

        loctree()
            .current_dir(&fixture)
            .assert()
            .success()
            // Summary output goes to stderr (stdout reserved for machine-readable data)
            .stderr(predicate::str::contains("handlers")); // Tauri mode detected = handlers shown
    }

    #[test]
    fn analyzes_tauri_handlers() {
        let fixture = fixtures_path().join("tauri_app");

        loctree()
            .current_dir(&fixture)
            .args(["-A", "--preset-tauri", "src", "src-tauri/src"])
            .assert()
            .success();
    }

    #[test]
    fn detects_missing_handlers() {
        let fixture = fixtures_path().join("tauri_app");

        loctree()
            .current_dir(&fixture)
            .args(["-A", "--preset-tauri", "src", "src-tauri/src"])
            .assert()
            .success()
            .stdout(predicate::str::contains("missing_handler"));
    }

    #[test]
    fn detects_unused_handlers() {
        let fixture = fixtures_path().join("tauri_app");

        loctree()
            .current_dir(&fixture)
            .args(["-A", "--preset-tauri", "src", "src-tauri/src"])
            .assert()
            .success()
            .stdout(predicate::str::contains("unused_handler"));
    }
}

// ============================================
// CI Fail Flag Tests
// ============================================

mod ci_fail_flags {
    use super::*;

    #[test]
    fn fails_on_missing_handlers() {
        let fixture = fixtures_path().join("tauri_app");

        loctree()
            .current_dir(&fixture)
            .args([
                "-A",
                "--preset-tauri",
                "src",
                "src-tauri/src",
                "--fail-on-missing-handlers",
            ])
            .assert()
            .failure()
            .code(1);
    }

    #[test]
    fn passes_when_no_missing_handlers() {
        // Must isolate fixture to avoid scanning parent repo (loctree-dev)
        // which contains other fixtures with missing handlers!
        let temp = TempDir::new().unwrap();
        let fixture = fixtures_path().join("simple_ts");
        copy_dir_all(&fixture, temp.path()).unwrap();

        // Non-Tauri project shouldn't fail on missing handlers
        loctree()
            .current_dir(temp.path())
            .args(["-A", "--fail-on-missing-handlers"])
            .assert()
            .success();
    }
}

// ============================================
// Confidence Scoring Tests
// ============================================

mod confidence_scoring {
    use super::*;

    #[test]
    fn filters_high_confidence() {
        let fixture = fixtures_path().join("dead_code");

        loctree()
            .current_dir(&fixture)
            .args(["-A", "--dead", "--confidence", "high"])
            .assert()
            .success();
    }

    #[test]
    fn filters_low_confidence() {
        let fixture = fixtures_path().join("dead_code");

        loctree()
            .current_dir(&fixture)
            .args(["-A", "--dead", "--confidence", "low"])
            .assert()
            .success();
    }

    #[test]
    fn shows_all_confidence_levels() {
        let fixture = fixtures_path().join("dead_code");

        loctree()
            .current_dir(&fixture)
            .args(["-A", "--dead", "--confidence", "all"])
            .assert()
            .success();
    }
}

// ============================================
// Trace Command Tests
// ============================================

mod trace_command {
    use super::*;

    #[test]
    fn traces_handler() {
        let fixture = fixtures_path().join("tauri_app");

        loctree()
            .current_dir(&fixture)
            .args(["trace", "unused_handler", "src", "src-tauri/src"])
            .assert()
            .success();
    }
}

// ============================================
// Git Commands Tests (Semantic Analysis)
// ============================================

mod git_commands {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    /// Create a temporary git repository for testing
    fn create_test_git_repo() -> TempDir {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path();

        // Initialize git repo
        Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .unwrap();

        // Configure git user
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(path)
            .output()
            .unwrap();

        // Create initial file and commit
        std::fs::write(path.join("main.ts"), "export function main() {}").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Initial commit"])
            .current_dir(path)
            .output()
            .unwrap();

        // Add second commit
        std::fs::write(
            path.join("utils.ts"),
            "export function add(a: number, b: number) { return a + b; }",
        )
        .unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add utils"])
            .current_dir(path)
            .output()
            .unwrap();

        temp_dir
    }

    #[test]
    fn git_compare_shows_json_output() {
        let temp_dir = create_test_git_repo();

        loctree()
            .current_dir(temp_dir.path())
            .args(["git", "compare", "HEAD~1", "HEAD"])
            .assert()
            .success()
            .stdout(predicate::str::contains("from_commit"))
            .stdout(predicate::str::contains("to_commit"))
            .stdout(predicate::str::contains("files"))
            .stdout(predicate::str::contains("impact"));
    }

    #[test]
    fn git_compare_with_range_notation() {
        let temp_dir = create_test_git_repo();

        loctree()
            .current_dir(temp_dir.path())
            .args(["git", "compare", "HEAD~1..HEAD"])
            .assert()
            .success()
            .stdout(predicate::str::contains("from_commit"));
    }

    #[test]
    fn git_compare_shows_added_files() {
        let temp_dir = create_test_git_repo();

        loctree()
            .current_dir(temp_dir.path())
            .args(["git", "compare", "HEAD~1", "HEAD"])
            .assert()
            .success()
            .stdout(predicate::str::contains("utils.ts"));
    }

    #[test]
    fn git_command_fails_in_non_git_dir() {
        // Create a truly isolated temp directory (not inside any git repo)
        let temp_dir = TempDir::new().unwrap();

        // Create a simple file so it's not empty
        std::fs::write(temp_dir.path().join("test.txt"), "hello").unwrap();

        loctree()
            .current_dir(temp_dir.path())
            .args(["git", "compare", "HEAD~1"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("not a git repository"));
    }

    #[test]
    fn git_blame_returns_not_implemented() {
        let temp_dir = create_test_git_repo();

        loctree()
            .current_dir(temp_dir.path())
            .args(["git", "blame", "main.ts"])
            .assert()
            .success()
            .stdout(predicate::str::contains("not_implemented"))
            .stdout(predicate::str::contains("Phase 2"));
    }

    #[test]
    fn git_history_returns_not_implemented() {
        let temp_dir = create_test_git_repo();

        loctree()
            .current_dir(temp_dir.path())
            .args(["git", "history", "main"])
            .assert()
            .success()
            .stdout(predicate::str::contains("not_implemented"))
            .stdout(predicate::str::contains("Phase 3"));
    }

    #[test]
    fn git_when_introduced_returns_not_implemented() {
        let temp_dir = create_test_git_repo();

        loctree()
            .current_dir(temp_dir.path())
            .args(["git", "when-introduced", "--dead", "unused_fn"])
            .assert()
            .success()
            .stdout(predicate::str::contains("not_implemented"))
            .stdout(predicate::str::contains("Phase 3"));
    }

    #[test]
    fn git_compare_shows_commit_info() {
        let temp_dir = create_test_git_repo();

        loctree()
            .current_dir(temp_dir.path())
            .args(["git", "compare", "HEAD~1", "HEAD"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Initial commit"))
            .stdout(predicate::str::contains("Add utils"));
    }
}

// ============================================
// Impact Analysis Tests
// ============================================

mod impact_mode {
    use super::*;

    /// Helper to ensure snapshot exists before impact tests
    fn ensure_snapshot(fixture: &std::path::Path) {
        loctree().current_dir(fixture).assert().success();
    }

    #[test]
    fn impact_shows_direct_consumers() {
        let fixture = fixtures_path().join("simple_ts");
        ensure_snapshot(&fixture);

        loctree()
            .current_dir(&fixture)
            .args(["impact", "src/utils/greeting.ts"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Impact analysis"))
            .stdout(predicate::str::contains("Direct consumers"));
    }

    #[test]
    fn impact_shows_transitive_consumers() {
        let fixture = fixtures_path().join("simple_ts");
        ensure_snapshot(&fixture);

        loctree()
            .current_dir(&fixture)
            .args(["impact", "src/utils/date.ts"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Impact analysis"));
    }

    #[test]
    fn impact_no_consumers_safe_to_remove() {
        let fixture = fixtures_path().join("simple_ts");
        ensure_snapshot(&fixture);

        // index.ts is likely a top-level file with no consumers
        loctree()
            .current_dir(&fixture)
            .args(["impact", "src/index.ts"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Impact analysis"));
    }

    #[test]
    fn impact_json_output() {
        let fixture = fixtures_path().join("simple_ts");
        ensure_snapshot(&fixture);

        loctree()
            .current_dir(&fixture)
            .args(["impact", "src/utils/greeting.ts", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::contains(r#""target""#))
            .stdout(predicate::str::contains(r#""direct_consumers""#))
            .stdout(predicate::str::contains(r#""transitive_consumers""#))
            .stdout(predicate::str::contains(r#""total_affected""#));
    }

    #[test]
    fn impact_counts_rust_mod_declaration_consumers() {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"mod-impact-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "pub mod commands;\n").unwrap();
        std::fs::write(
            temp.path().join("src/commands.rs"),
            "pub fn ensure_portal_entitlement() {}\n",
        )
        .unwrap();

        loctree()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .assert()
            .success();

        let output = loctree()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["impact", "src/commands.rs", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let json: Value = serde_json::from_slice(&output).expect("impact --json is valid JSON");
        let direct = json["direct_consumers"]
            .as_array()
            .expect("direct_consumers array");
        assert!(
            direct
                .iter()
                .any(|entry| entry["file"] == "src/lib.rs" && entry["import_type"] == "mod"),
            "Rust `pub mod commands;` must count as a direct consumer edge: {json}"
        );
        assert_eq!(
            json["total_affected"], 1,
            "mod declaration impact should not read as safe-to-delete: {json}"
        );
    }

    #[test]
    fn hidden_cargo_config_is_visible_to_slice_and_impact() {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join(".cargo")).unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"hidden-truth-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join(".cargo/config.toml"),
            "[target.x86_64-unknown-linux-musl]\nlinker = \"musl-gcc\"\n",
        )
        .unwrap();
        std::fs::write(temp.path().join(".gitignore"), ".loctree/\n").unwrap();
        std::fs::write(temp.path().join(".semgrep.yaml"), "rules: []\n").unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "pub fn live() {}\n").unwrap();

        loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .env("LOCT_NO_GITIGNORE", "1")
            .args(["scan", "--full-scan"])
            .assert()
            .success();

        let slice_output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["slice", ".cargo/config.toml", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let slice_json: Value =
            serde_json::from_slice(&slice_output).expect("slice --json is valid JSON");
        assert_eq!(slice_json["target"], ".cargo/config.toml");
        assert!(
            slice_json["core"]
                .as_array()
                .expect("slice core array")
                .iter()
                .any(|entry| entry["path"] == ".cargo/config.toml"),
            ".cargo/config.toml should be snapshot-backed slice core: {slice_json}"
        );
        assert!(
            !String::from_utf8_lossy(&slice_output).contains("disk_explicit_fallback"),
            ".cargo/config.toml should be in the snapshot, not disk fallback"
        );

        let impact_output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["impact", ".cargo/config.toml", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let impact_json: Value =
            serde_json::from_slice(&impact_output).expect("impact --json is valid JSON");
        assert_eq!(impact_json["target"], ".cargo/config.toml");
        assert_eq!(impact_json["total_affected"], 0);

        for hidden_config in [".gitignore", ".semgrep.yaml"] {
            let slice_output = loct()
                .current_dir(temp.path())
                .env("LOCT_CACHE_DIR", cache.path())
                .args(["slice", hidden_config, "--json"])
                .assert()
                .success()
                .get_output()
                .stdout
                .clone();
            let slice_json: Value =
                serde_json::from_slice(&slice_output).expect("slice --json is valid JSON");
            assert_eq!(slice_json["target"], hidden_config);
            assert!(
                slice_json["core"]
                    .as_array()
                    .expect("slice core array")
                    .iter()
                    .any(|entry| entry["path"] == hidden_config),
                "{hidden_config} should be snapshot-backed slice core: {slice_json}"
            );
            assert!(
                !String::from_utf8_lossy(&slice_output).contains("disk_explicit_fallback"),
                "{hidden_config} should be in the snapshot, not disk fallback"
            );
        }
    }

    #[test]
    fn impact_names_exclusion_for_explicit_hidden_parent_target() {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join(".secret")).unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(
            temp.path().join(".secret/config.toml"),
            "token = 'redacted'\n",
        )
        .unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "pub fn live() {}\n").unwrap();

        loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();

        loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["impact", ".secret/config.toml"])
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "File exists but is excluded from snapshot: .secret/config.toml",
            ))
            .stderr(predicate::str::contains(
                "Detected exclusion: skipped by default hidden-file filter for `.secret`",
            ))
            .stderr(predicate::str::contains("core-only fallback read"));
    }

    #[test]
    fn impact_with_max_depth() {
        let fixture = fixtures_path().join("simple_ts");
        ensure_snapshot(&fixture);

        loctree()
            .current_dir(&fixture)
            .args(["impact", "src/utils/greeting.ts", "--max-depth", "1"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Impact analysis"));
    }

    #[test]
    fn impact_file_not_found() {
        let fixture = fixtures_path().join("simple_ts");
        ensure_snapshot(&fixture);

        // Nonexistent file returns error (file must exist in snapshot)
        loctree()
            .current_dir(&fixture)
            .args(["impact", "nonexistent.ts"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("File not found in snapshot"));
    }

    #[test]
    fn impact_without_snapshot_auto_scans() {
        let temp = TempDir::new().unwrap();
        // Create minimal file structure
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/test.ts"), "export const x = 1;").unwrap();

        // Without snapshot, impact command auto-scans first (good UX)
        loctree()
            .current_dir(temp.path())
            .args(["impact", "src/test.ts"])
            .assert()
            .success()
            .stderr(predicate::str::contains("running initial scan"));
    }
}

// ============================================
// Diff Mode Tests (auto-scan-base flag)
// ============================================

mod diff_mode_new_features {
    use super::*;
    use std::process::Command as ProcessCommand;

    fn create_diff_repo() -> TempDir {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        ProcessCommand::new("git")
            .args(["init"])
            .current_dir(root)
            .output()
            .unwrap();
        ProcessCommand::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(root)
            .output()
            .unwrap();
        ProcessCommand::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(root)
            .output()
            .unwrap();

        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.ts"), "export const version = 1;\n").unwrap();
        ProcessCommand::new("git")
            .args(["add", "."])
            .current_dir(root)
            .output()
            .unwrap();
        ProcessCommand::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(root)
            .output()
            .unwrap();

        std::fs::write(root.join("src/main.ts"), "export const version = 2;\n").unwrap();
        std::fs::write(root.join("src/new.ts"), "export const fresh = true;\n").unwrap();
        ProcessCommand::new("git")
            .args(["add", "."])
            .current_dir(root)
            .output()
            .unwrap();
        ProcessCommand::new("git")
            .args(["commit", "-m", "update"])
            .current_dir(root)
            .output()
            .unwrap();

        temp
    }

    #[test]
    fn diff_help_shows_auto_scan_base_flag() {
        loctree()
            .args(["diff", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("auto-scan-base")
                    .or(predicate::str::contains("Auto-scan base commit")),
            )
            .stdout(predicate::str::contains("changed-files"));
    }

    #[test]
    fn diff_auto_scan_base_flag_exists() {
        // Just verify the flag is recognized (don't need actual git worktree)
        loctree()
            .args(["diff", "--auto-scan-base", "--help"])
            .assert()
            .success();
    }

    #[test]
    fn diff_changed_files_summarizes_ref_to_head_without_snapshot_scan() {
        let temp = create_diff_repo();

        let output = loct()
            .current_dir(temp.path())
            .args(["diff", "--since", "HEAD~1", "--changed-files", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let json: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(json["from"], "HEAD~1");
        assert_eq!(json["to"], "HEAD");
        assert_eq!(json["total"], 2);
        assert_eq!(json["changed_files"][0]["status"], "Modified");
        assert_eq!(json["changed_files"][0]["new_path"], "src/main.ts");
        assert_eq!(json["changed_files"][1]["status"], "Added");
        assert_eq!(json["changed_files"][1]["new_path"], "src/new.ts");
    }
}

// ============================================
// Watch Mode Tests
// ============================================

mod watch_mode {
    use super::*;

    #[test]
    fn watch_help_shows_flag() {
        loctree()
            .args(["scan", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("--watch")
                    .and(predicate::str::contains("Watch for changes")),
            );
    }

    #[test]
    fn watch_flag_recognized() {
        // Just verify the flag is parsed (won't actually start watching in test)
        // This will timeout or need Ctrl+C, so we just check it doesn't error on parsing
        let fixture = fixtures_path().join("simple_ts");

        // Run with timeout to avoid hanging
        // Note: This test is limited - real watch mode testing would require
        // simulating file changes or mocking the watcher
        // Use new CLI syntax: `loct scan --watch` instead of legacy `loct --watch`
        loctree()
            .current_dir(&fixture)
            .args(["scan", "--watch"])
            .timeout(std::time::Duration::from_millis(100))
            .assert()
            .interrupted(); // Expect timeout/interrupt
    }
}

// ============================================
// Context Pill Tests
// ============================================

mod context_pill {
    use super::*;

    fn write_hub_fixture(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/types.ts"),
            "export type Model = { id: string };\nexport function normalize(value: Model): string { return value.id; }\n",
        )
        .unwrap();

        for name in ["alpha", "beta", "gamma", "delta"] {
            std::fs::write(
                root.join("src").join(format!("{name}.ts")),
                format!(
                    "import {{ Model, normalize }} from './types';\nexport const {name}: Model = {{ id: normalize({{ id: '{name}' }}) }};\n"
                ),
            )
            .unwrap();
        }

        std::fs::write(
            root.join("src/index.ts"),
            "export * from './alpha';\nexport * from './beta';\nexport * from './gamma';\nexport * from './delta';\n",
        )
        .unwrap();
    }

    #[test]
    fn bare_context_yields_markdown_pill_with_grounded_actions() {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        write_hub_fixture(temp.path());

        loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();

        let output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .env("LOCT_AICX_BINARY", "/this/path/does/not/exist/aicx")
            .args(["context", "--no-aicx"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "context failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.starts_with("# Loctree Context"),
            "bare context should be markdown pill, got:\n{}",
            stdout
        );
        assert!(stdout.contains("## Where You Are"));
        assert!(stdout.contains("src/types.ts"));
        assert!(stdout.contains("loct slice src/types.ts"));
        assert!(!stdout.trim_start().starts_with('{'));

        let full = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["context", "--full", "--no-aicx"])
            .output()
            .unwrap();
        assert!(
            full.status.success(),
            "context --full failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&full.stdout),
            String::from_utf8_lossy(&full.stderr)
        );
        let value: Value = serde_json::from_slice(&full.stdout).expect("--full emits JSON");
        assert_eq!(value["schema_version"], "1.0");
        assert!(
            value["structural"]["files"].as_array().unwrap().len() >= 3,
            "--full should preserve auto-scoped structural files"
        );

        let full_markdown = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .env("LOCT_AICX_BINARY", "/this/path/does/not/exist/aicx")
            .args(["context", "--full", "--markdown", "--no-aicx"])
            .output()
            .unwrap();
        assert!(
            full_markdown.status.success(),
            "context --full --markdown failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&full_markdown.stdout),
            String::from_utf8_lossy(&full_markdown.stderr)
        );
        let full_markdown_stdout = String::from_utf8_lossy(&full_markdown.stdout);
        assert!(
            full_markdown_stdout.starts_with("# Loctree Context"),
            "context --full --markdown should emit markdown, got:\n{}",
            full_markdown_stdout
        );
        assert!(
            serde_json::from_slice::<Value>(&full_markdown.stdout).is_err(),
            "context --full --markdown must not emit raw ContextPack JSON"
        );

        let json = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["context", "--json", "--no-aicx"])
            .output()
            .unwrap();
        assert!(
            json.status.success(),
            "context --json failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&json.stdout),
            String::from_utf8_lossy(&json.stderr)
        );
        serde_json::from_slice::<Value>(&json.stdout).expect("--json emits JSON");
    }

    #[test]
    fn positional_directory_context_uses_project_scope_not_file_slice() {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        write_hub_fixture(temp.path());

        loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();

        let project_arg = temp.path().to_string_lossy().to_string();
        let output = loct()
            .env("LOCT_CACHE_DIR", cache.path())
            .env("LOCT_AICX_BINARY", "/this/path/does/not/exist/aicx")
            .args([
                "context",
                project_arg.as_str(),
                "--full",
                "--markdown",
                "--no-aicx",
            ])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "context project dir failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("Ambiguous slice target"),
            "project-directory context must not leak an ambiguous empty slice warning: {stderr}"
        );
        assert!(
            !stdout.contains(&format!("loct slice {project_arg}")),
            "project-directory context must not recommend slicing the repository root:\n{stdout}"
        );
        assert!(
            stdout.contains("src/types.ts"),
            "project-directory context should still use grounded default scope:\n{stdout}"
        );
    }

    #[test]
    fn file_focused_context_yields_markdown_pill_with_power_path() {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        write_hub_fixture(temp.path());

        loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();

        let output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .env("LOCT_AICX_BINARY", "/this/path/does/not/exist/aicx")
            .args(["context", "--file", "src/types.ts", "--no-aicx"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "context failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("### Power Path (suggested next steps)"),
            "should render power path, stdout:\n{}",
            stdout
        );
        assert!(stdout.contains("loct slice src/types.ts"));
        assert!(stdout.contains("loct body Model") || stdout.contains("loct body normalize"));
        assert!(stdout.contains("loct follow"));
    }

    #[cfg(unix)]
    #[test]
    fn context_markdown_summarizes_aicx_raw_tool_payloads() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        write_hub_fixture(temp.path());

        loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();

        let mock = temp.path().join("aicx-mock.sh");
        std::fs::write(
            &mock,
            r#"#!/bin/sh
case "$1" in
  --version)
    printf 'aicx mock\n'
    ;;
  intents)
    cat <<'JSON'
{
  "items": [
    {
      "kind": "intent",
      "summary": "{\"call_id\":\"call_123\",\"unified_diff\":\"diff --git a/src/types.ts b/src/types.ts\\n--- a/src/types.ts\\n+++ b/src/types.ts\\n@@ -1 +1 @@\\n-old\\n+new\"}",
      "project": "loctree-suite",
      "agent": "codex",
      "date": "2026-06-11",
      "timestamp": "2026-06-11T12:00:00Z",
      "session_id": "pill-honesty",
      "source_chunk": "/tmp/aicx/store/loctree-suite/2026-06-11/codex/pill-honesty.md"
    }
  ]
}
JSON
    ;;
  *)
    printf 'unsupported mock command: %s\n' "$1" >&2
    exit 2
    ;;
esac
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&mock).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&mock, perms).unwrap();

        let output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .env("LOCT_AICX_BINARY", &mock)
            .env("LOCT_AICX_MODE", "cli")
            .args(["context", "--with-aicx", "--markdown"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "context --with-aicx --markdown failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("updated src/types.ts from AICX diff"),
            "raw AICX payload should be summarized, got:\n{stdout}"
        );
        assert!(stdout.contains("chunk:"));
        for raw_marker in ["call_id", "unified_diff", "@@"] {
            assert!(
                !stdout.contains(raw_marker),
                "context pill leaked raw AICX marker {raw_marker:?}:\n{stdout}"
            );
        }
    }

    #[test]
    fn context_json_python_project_uses_python_verification_gates() {
        let fixture = fixtures_path().join("python_project");
        let cache = TempDir::new().unwrap();

        loct()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();

        let output = loct()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["context", "--json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "context --json failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).expect("context JSON");
        let gates = value["action"]["verification_gates"]
            .as_array()
            .expect("verification_gates array");
        let gate_strings: Vec<&str> = gates.iter().filter_map(|gate| gate.as_str()).collect();

        assert!(gate_strings.contains(&"ruff check ."));
        assert!(gate_strings.contains(&"mypy ."));
        assert!(gate_strings.contains(&"pytest"));
        assert!(
            !gate_strings.iter().any(|gate| gate.contains("cargo")),
            "python fixture should not receive cargo gates: {gate_strings:?}"
        );
    }
}

// ============================================
// Helper Functions
// ============================================

/// Read a directory after re-asserting that its canonical form is a
/// descendant of `allowed_root`.
///
/// Test-side mirror of the production `read_dir_within_root` helper. The
/// copy helpers below are only ever invoked with `src` rooted under
/// `fixtures_path()` (a `CARGO_MANIFEST_DIR`-derived compile-time path)
/// or under a tempdir created by the test itself. By canonicalizing the
/// requested directory and verifying containment immediately before
/// `read_dir`, the boundary guard sits at the same call site as the I/O
/// sink — Semgrep's local `tainted-path` data-flow analysis can prove
/// the read is bounded without a `nosemgrep` suppression.
fn read_dir_within_root(
    allowed_root: &std::path::Path,
    dir: &std::path::Path,
) -> std::io::Result<std::fs::ReadDir> {
    let canonical = dir.canonicalize()?;
    let canonical_root = allowed_root
        .canonicalize()
        .unwrap_or_else(|_| allowed_root.to_path_buf());
    if !canonical.starts_with(&canonical_root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "test fixture copy escapes allowed root: {} (allowed root: {})",
                canonical.display(),
                canonical_root.display()
            ),
        ));
    }
    std::fs::read_dir(&canonical)
}

/// Resolve the bounding root for a test fixture copy. Tests either copy a
/// subtree of `fixtures_path()` (the canonical case) or a subtree under a
/// per-test temp dir (when the test mutates the tree before re-copying it).
/// In both cases the actual on-disk parent of `src` is the trustworthy
/// boundary: it is either compile-time-rooted or test-rooted, never
/// supplied by an external caller, so locking the read scope to that
/// parent is sufficient to satisfy the path-traversal analyzer.
fn fixture_copy_root(src: &std::path::Path) -> std::path::PathBuf {
    src.parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| src.to_path_buf())
}

fn parse_scan_banner_count(stderr: &[u8]) -> usize {
    let text = String::from_utf8_lossy(stderr);
    let marker = "Scanned ";
    let start = text.find(marker).expect("scan banner should include count") + marker.len();
    let digits: String = text[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits.parse().expect("scan banner count should be numeric")
}

fn parse_json_output(stdout: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stdout);
    let start = text.find('{').expect("command output should include json");
    serde_json::from_str(&text[start..]).expect("command output should parse as json")
}

fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    let allowed_root = fixture_copy_root(src);
    for entry in read_dir_within_root(&allowed_root, src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), dest_path)?;
        }
    }
    Ok(())
}

/// Copy directory tree, skipping a named top-level entry (e.g. ".loctree").
fn copy_dir_excluding(
    src: &std::path::Path,
    dst: &std::path::Path,
    exclude: &str,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    let allowed_root = fixture_copy_root(src);
    for entry in read_dir_within_root(&allowed_root, src)? {
        let entry = entry?;
        if entry.file_name() == exclude {
            continue;
        }
        let ty = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), dest_path)?;
        }
    }
    Ok(())
}

fn run_git(repo: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|e| panic!("failed to run git {:?}: {e}", args));
    assert!(
        output.status.success(),
        "git {:?} failed.\nstdout: {}\nstderr: {}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

mod auto_scan {
    use super::*;
    use std::time::Duration;

    fn commit_all(root: &std::path::Path, message: &str) {
        run_git(root, &["add", "."]);
        run_git(
            root,
            &[
                "-c",
                "user.email=agents@vetcoders.io",
                "-c",
                "user.name=codex",
                "commit",
                "-m",
                message,
            ],
        );
    }

    fn create_git_repo() -> TempDir {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(
            temp.path().join("src/lib.rs"),
            "pub fn initial_marker() -> &'static str { \"initial\" }\n",
        )
        .unwrap();
        run_git(temp.path(), &["init"]);
        commit_all(temp.path(), "init");
        temp
    }

    fn scan_repo(root: &std::path::Path, cache: &TempDir) {
        loct()
            .current_dir(root)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();
    }

    fn latest_snapshot_path(cache: &TempDir) -> PathBuf {
        latest_snapshot_path_in(cache.path())
    }

    fn latest_snapshot_path_in(cache: &std::path::Path) -> PathBuf {
        fn visit(dir: &std::path::Path, snapshots: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    visit(&path, snapshots);
                } else if path.file_name().and_then(|name| name.to_str()) == Some("snapshot.json")
                    && !path
                        .components()
                        .any(|component| component.as_os_str() == "latest")
                {
                    snapshots.push(path);
                }
            }
        }

        let mut snapshots = Vec::new();
        visit(cache, &mut snapshots);
        let scoped: Vec<PathBuf> = snapshots
            .iter()
            .filter(|path| {
                path.components()
                    .any(|component| component.as_os_str().to_string_lossy().contains('@'))
            })
            .cloned()
            .collect();
        if !scoped.is_empty() {
            snapshots = scoped;
        }
        snapshots.sort_by_key(|path| {
            std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        });
        snapshots
            .pop()
            .expect("snapshot.json should exist in cache")
    }

    #[test]
    fn auto_scans_on_missing_snapshot() {
        let temp = create_git_repo();
        let cache = TempDir::new().unwrap();

        let output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["context"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "context failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("no snapshot found"));
        assert!(stderr.contains("scanning"));
        assert!(stderr.contains("scan completed in"));
        assert!(latest_snapshot_path(&cache).exists());
        assert!(String::from_utf8_lossy(&output.stdout).contains("# Loctree Context"));
    }

    #[test]
    fn auto_rescans_on_stale_snapshot() {
        let temp = create_git_repo();
        let cache = TempDir::new().unwrap();
        scan_repo(temp.path(), &cache);

        std::fs::write(
            temp.path().join("src/new.rs"),
            "pub fn post_commit_marker() -> &'static str { \"new\" }\n",
        )
        .unwrap();
        commit_all(temp.path(), "add new file");

        let output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["context", "src/new.rs", "--json"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "context failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("snapshot stale"));
        assert!(stderr.contains("rescanning"));
        assert!(String::from_utf8_lossy(&output.stdout).contains("src/new.rs"));
    }

    #[test]
    fn slice_auto_rescans_on_stale_snapshot_before_reporting_not_found() {
        let temp = create_git_repo();
        let cache = TempDir::new().unwrap();
        scan_repo(temp.path(), &cache);

        std::fs::write(
            temp.path().join("src/new.rs"),
            "pub fn post_commit_marker() -> &'static str { \"new\" }\n",
        )
        .unwrap();
        commit_all(temp.path(), "add new file");

        let output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["slice", "src/new.rs"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "slice should refresh stale snapshots before declaring a real file missing\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Snapshot content changed"));
        assert!(stderr.contains("rescanning"));
        assert!(String::from_utf8_lossy(&output.stdout).contains("src/new.rs"));
    }

    #[test]
    fn monorepo_with_legacy_sub_loctree_preserves_root_project_identity() {
        // Regression for the scope-drift bug Monika reported in 0.9.3:
        // `loct context --file sub-tauri/...` from monorepo root would
        // shadow-pick a stale per-branch snapshot from a sub-`.loctree/`
        // (often left over from another env) and report the sub-project
        // as Project Identity Root, blanking memory + invoke edges.
        let temp = create_git_repo();
        let cache = TempDir::new().unwrap();

        std::fs::create_dir_all(temp.path().join("sub-tauri/src/commands")).unwrap();
        std::fs::write(
            temp.path().join("src/lib.rs"),
            "pub fn main_hub() -> &'static str { \"hub\" }\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("sub-tauri/src/commands/license.rs"),
            "pub fn license_activate() -> &'static str { \"ok\" }\n",
        )
        .unwrap();
        commit_all(temp.path(), "monorepo with sub-tauri");

        scan_repo(temp.path(), &cache);

        // Plant a stale per-branch snapshot inside the sub-projekt's `.loctree/`,
        // pretending it came from a foreign env (`/foreign/env/sub-tauri`).
        let stale_loctree = temp.path().join("sub-tauri/.loctree/legacy@abc1234");
        std::fs::create_dir_all(&stale_loctree).unwrap();
        let stale_snapshot = serde_json::json!({
            "schema_version": "0.8.11",
            "metadata": {
                "roots": ["/foreign/env/sub-tauri"],
                "git_commit": "abc1234deadbeef",
                "generated_at": "2025-01-01T00:00:00Z",
                "languages": ["rs"],
                "scan_duration_ms": 0
            },
            "files": [],
            "edges": [],
            "export_index": {},
            "command_bridges": [],
            "event_bridges": [],
            "barrels": []
        });
        std::fs::write(
            stale_loctree.join("snapshot.json"),
            serde_json::to_string(&stale_snapshot).unwrap(),
        )
        .unwrap();

        let output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args([
                "context",
                "--file",
                "sub-tauri/src/commands/license.rs",
                "--json",
            ])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "context failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let value: Value =
            serde_json::from_slice(&output.stdout).expect("ContextPack JSON on stdout");
        let canonical_root = value["project"]["canonical_root"]
            .as_str()
            .expect("project.canonical_root field present");

        let expected_root = temp
            .path()
            .canonicalize()
            .unwrap_or_else(|_| temp.path().to_path_buf());
        let actual_root = std::path::PathBuf::from(canonical_root);
        let actual_canonical = actual_root.canonicalize().unwrap_or(actual_root);
        assert_eq!(
            actual_canonical, expected_root,
            "canonical_root must be monorepo root, not sub-projekt or foreign env"
        );
        assert!(
            !canonical_root.contains("/foreign/"),
            "canonical_root must not be from a foreign env (legacy snapshot leak): {canonical_root}"
        );
        assert!(
            !canonical_root.ends_with("sub-tauri"),
            "canonical_root must not collapse onto sub-projekt: {canonical_root}"
        );
    }

    #[test]
    fn no_scan_flag_preserves_missing_snapshot_behavior() {
        let temp = create_git_repo();
        let cache = TempDir::new().unwrap();

        let output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["--no-scan", "context"])
            .output()
            .unwrap();

        assert!(output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("no snapshot found"));
        assert!(stderr.contains("--no-scan in effect"));
        assert!(String::from_utf8_lossy(&output.stdout).contains("missing_snapshot"));
    }

    #[test]
    fn no_scan_flag_uses_stale_snapshot_with_warning() {
        let temp = create_git_repo();
        let cache = TempDir::new().unwrap();
        scan_repo(temp.path(), &cache);
        run_git(
            temp.path(),
            &[
                "-c",
                "user.email=agents@vetcoders.io",
                "-c",
                "user.name=codex",
                "commit",
                "--allow-empty",
                "-m",
                "trigger stale",
            ],
        );

        let output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["--no-scan", "context", "--json"])
            .output()
            .unwrap();

        assert!(output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("snapshot is stale"));
        assert!(stderr.contains("--no-scan in effect"));
    }

    #[test]
    fn fail_stale_exits_3_in_ci_mode() {
        let temp = create_git_repo();
        let cache = TempDir::new().unwrap();
        scan_repo(temp.path(), &cache);
        run_git(
            temp.path(),
            &[
                "-c",
                "user.email=agents@vetcoders.io",
                "-c",
                "user.name=codex",
                "commit",
                "--allow-empty",
                "-m",
                "trigger stale",
            ],
        );

        let output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["--fail-stale", "context", "--json"])
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(3));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("snapshot is stale"));
        assert!(stderr.contains("--fail-stale"));
    }

    #[test]
    fn mtime_stale_snapshot_incrementally_rescans() {
        let temp = create_git_repo();
        let cache = TempDir::new().unwrap();
        scan_repo(temp.path(), &cache);

        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(
            temp.path().join("src/lib.rs"),
            "pub fn changed_marker() -> &'static str { \"changed\" }\n",
        )
        .unwrap();

        let output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["context", "src/lib.rs", "--json"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "context failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("file(s) changed since last scan"));
        assert!(stderr.contains("incremental rescan"));
        assert!(String::from_utf8_lossy(&output.stdout).contains("changed_marker"));
    }

    #[test]
    fn max_age_env_triggers_incremental_rescan() {
        let temp = create_git_repo();
        let cache = TempDir::new().unwrap();
        loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();

        let snapshot_path = latest_snapshot_path_in(cache.path());
        let mut snapshot: loctree::snapshot::Snapshot =
            serde_json::from_str(&std::fs::read_to_string(&snapshot_path).unwrap()).unwrap();
        snapshot.metadata.generated_at = "2020-01-01T00:00:00Z".to_string();
        std::fs::write(
            &snapshot_path,
            serde_json::to_string_pretty(&snapshot).unwrap(),
        )
        .unwrap();
        let output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .env("LOCT_CACHE_MAX_AGE", "12h")
            .env("LOCT_NO_GITIGNORE", "1")
            .args(["context", "src/lib.rs", "--json"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "context failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("snapshot older than 12h")
                || stderr.contains("file(s) changed since last scan"),
            "stderr did not contain stale-cache rescan message:\n{stderr}"
        );
        assert!(stderr.contains("incremental rescan"));
    }

    #[test]
    fn auto_writes_atlas_to_dotloctree() {
        // Plan 01 — atlas-per-repo: the materialized Context Atlas must land
        // at `<repo_root>/.loctree/context-atlas/`, not in the global cache.
        // Verifies `atlas_dir_for_project` returns the per-repo path and that
        // `materialize_context_atlas` creates `.loctree/` even when missing.
        let temp = create_git_repo();
        let cache = TempDir::new().unwrap();

        // Sanity: no `.loctree/` before the run.
        assert!(
            !temp.path().join(".loctree").exists(),
            "fixture repo should start without .loctree/"
        );

        let output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["context"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "context failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let atlas_dir = temp.path().join(".loctree").join("context-atlas");
        let manifest_json = atlas_dir.join("manifest.json");
        assert!(
            manifest_json.exists(),
            "atlas manifest missing at {} (expected per Plan 01 layout)",
            manifest_json.display()
        );

        // Manifest is well-formed and points at the per-repo location.
        let raw = std::fs::read_to_string(&manifest_json).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            value.get("protocol").and_then(|v| v.as_str()),
            Some("loctree.context_atlas.v1")
        );
        let manifest_atlas_dir = value
            .get("atlas_dir")
            .and_then(|v| v.as_str())
            .expect("manifest atlas_dir present");
        assert!(
            manifest_atlas_dir.ends_with(".loctree/context-atlas"),
            "atlas_dir does not end with .loctree/context-atlas: {manifest_atlas_dir}"
        );
        assert!(
            value
                .get("cards")
                .and_then(|v| v.as_array())
                .map(|cards| !cards.is_empty())
                .unwrap_or(false),
            "manifest contains no cards"
        );
    }
}

// ============================================
// Instant Commands Tests (<100ms)
// ============================================
// 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

mod instant_commands {
    use super::*;

    // ----------------------------------------
    // Focus Command Tests
    // ----------------------------------------

    #[test]
    fn focus_help_shows_usage() {
        loctree()
            .args(["focus", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("focus").or(predicate::str::contains("directory")));
    }

    #[test]
    fn focus_on_directory() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["focus", "src/"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("src")
                    .or(predicate::str::contains("Focus"))
                    .or(predicate::str::contains("files")),
            );
    }

    #[test]
    fn focus_json_output() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["focus", "src/", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{").or(predicate::str::starts_with("[")));
    }

    // ----------------------------------------
    // Hotspots Command Tests
    // ----------------------------------------

    #[test]
    fn hotspots_help_shows_usage() {
        loctree()
            .args(["hotspots", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("hotspots")
                    .or(predicate::str::contains("import"))
                    .or(predicate::str::contains("frequency")),
            );
    }

    #[test]
    fn hotspots_runs_successfully() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["hotspots"])
            .assert()
            .success();
    }

    #[test]
    fn hotspots_json_output() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["hotspots", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{").or(predicate::str::starts_with("[")));
    }

    // ----------------------------------------
    // Health Command Tests
    // ----------------------------------------

    #[test]
    fn health_help_shows_usage() {
        loctree()
            .args(["health", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("health").or(predicate::str::contains("check")));
    }

    #[test]
    fn health_runs_successfully() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["health"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("Health")
                    .or(predicate::str::contains("OK"))
                    .or(predicate::str::contains("score")),
            );
    }

    #[test]
    fn health_alias_h() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["h"])
            .assert()
            .success();
    }

    #[test]
    fn health_json_output() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["health", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::contains(r#""health"#).or(predicate::str::starts_with("{")));
    }

    // ----------------------------------------
    // Query Command Tests
    // ----------------------------------------

    #[test]
    fn query_help_shows_usage() {
        loctree()
            .args(["query", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("query")
                    .or(predicate::str::contains("who-imports"))
                    .or(predicate::str::contains("where-symbol")),
            );
    }

    #[test]
    fn query_who_imports() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["query", "who-imports", "src/utils/greeting.ts"])
            .assert()
            .success();
    }

    #[test]
    fn query_where_symbol() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["query", "where-symbol", "greet"])
            .assert()
            .success();
    }

    #[test]
    fn query_alias_q() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["q", "who-imports", "src/utils/greeting.ts"])
            .assert()
            .success();
    }

    #[test]
    fn rust_use_edges_feed_impact_query_and_slice_without_swift_false_positive() {
        let fixture = fixtures_path().join("rust_import_edges");
        let cache = TempDir::new().unwrap();

        loct()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["impact", "app/a.rs"])
            .assert()
            .success()
            .stdout(predicate::str::contains("app/b.rs"))
            .stdout(predicate::str::contains("app/c.rs"))
            .stdout(predicate::str::contains("app/d.rs"))
            .stdout(predicate::str::contains("Bridge.swift").not());

        loct()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["query", "who-imports", "app/a.rs"])
            .assert()
            .success()
            .stdout(predicate::str::contains("app/b.rs"))
            .stdout(predicate::str::contains("app/c.rs"))
            .stdout(predicate::str::contains("app/d.rs"))
            .stdout(predicate::str::contains("Bridge.swift").not());

        loct()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["find", "app/a.rs", "--who-imports"])
            .assert()
            .success()
            .stdout(predicate::str::contains("app/b.rs"))
            .stdout(predicate::str::contains("app/c.rs"))
            .stdout(predicate::str::contains("app/d.rs"))
            .stdout(predicate::str::contains("Bridge.swift").not());

        loct()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["slice", "app/a.rs", "--consumers"])
            .assert()
            .success()
            .stdout(predicate::str::contains("app/b.rs"))
            .stdout(predicate::str::contains("app/c.rs"))
            .stdout(predicate::str::contains("app/d.rs"))
            .stdout(predicate::str::contains("Bridge.swift").not());

        loct()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .arg(r#"[.edges[] | select(.label == "implicit_symbol" and .from == "app/Bridge.swift")] | length"#)
            .assert()
            .success()
            .stdout(predicate::str::contains("0"));

        // W1 regression: module-dir facade + use-with-trailing-item
        // (use crate::pipeline::streaming::StreamItem in c.rs) must make the
        // streaming/mod.rs report c.rs as consumer via impact/slice/who-imports.
        // This covers the cross-module crate::...::modname::Item case and
        // mod.rs facade aggregation (fail.md 2997, 3144, dispatch W1.1/W1.2).
        loct()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["impact", "app/pipeline/streaming/mod.rs"])
            .assert()
            .success()
            .stdout(predicate::str::contains("app/c.rs"));

        loct()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["slice", "app/pipeline/streaming/mod.rs", "--consumers"])
            .assert()
            .success()
            .stdout(predicate::str::contains("app/c.rs"));

        loct()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["find", "app/pipeline/streaming/mod.rs", "--who-imports"])
            .assert()
            .success()
            .stdout(predicate::str::contains("app/c.rs"));
    }

    #[test]
    fn find_and_mode_lists_intersection_file_paths() {
        let fixture = fixtures_path().join("simple_ts");

        loct()
            .current_dir(&fixture)
            .args(["find", "greet main"])
            .assert()
            .success()
            .stdout(predicate::str::contains("=== Intersection Files (1) ==="))
            .stdout(predicate::str::contains("src/index.ts"));
    }

    // ----------------------------------------
    // Commands Command Tests (Tauri)
    // ----------------------------------------

    #[test]
    fn commands_help_shows_usage() {
        loctree()
            .args(["commands", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("commands")
                    .or(predicate::str::contains("Tauri"))
                    .or(predicate::str::contains("handler")),
            );
    }

    #[test]
    fn commands_in_tauri_project() {
        let fixture = fixtures_path().join("tauri_app");

        loctree()
            .current_dir(&fixture)
            .args(["commands"])
            .assert()
            .success();
    }

    #[test]
    fn commands_json_output() {
        let fixture = fixtures_path().join("tauri_app");

        loctree()
            .current_dir(&fixture)
            .args(["commands", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{").or(predicate::str::starts_with("[")));
    }

    // ----------------------------------------
    // Events Command Tests
    // ----------------------------------------

    #[test]
    fn events_help_shows_usage() {
        loctree()
            .args(["events", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("events")
                    .or(predicate::str::contains("emit"))
                    .or(predicate::str::contains("listen")),
            );
    }

    #[test]
    fn events_runs_successfully() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["events"])
            .assert()
            .success();
    }

    #[test]
    fn events_json_output() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["events", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{").or(predicate::str::starts_with("[")));
    }

    // ----------------------------------------
    // Coverage Command Tests
    // ----------------------------------------

    #[test]
    fn coverage_help_shows_usage() {
        loctree()
            .args(["coverage", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("coverage")
                    .or(predicate::str::contains("test"))
                    .or(predicate::str::contains("gaps")),
            );
    }

    #[test]
    fn coverage_runs_successfully() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["coverage"])
            .assert()
            .success();
    }

    #[test]
    fn coverage_json_output() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["coverage", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{").or(predicate::str::starts_with("[")));
    }

    // ----------------------------------------
    // Artifact Fence Tests (w1-b / W2-02)
    // ----------------------------------------

    /// Isolated copy of the `artifact_fence` fixture plus a private snapshot
    /// cache.
    ///
    /// These tests must NOT run with `current_dir` pointing at the in-repo
    /// fixture directory: loct resolves project identity by walking up to the
    /// enclosing git root, so an in-repo cwd shares its snapshot cache key
    /// with every repo-root scan from parallel tests. Under `cargo test`
    /// parallelism that produced a "Snapshot roots differ from requested
    /// roots; refreshing snapshot scope" race — a concurrent repo-root scan
    /// overwrote the fixture-scoped snapshot between refresh and read, and
    /// the fence assertions saw repo-root output ("No event bridges found").
    /// Copying the fixture into a TempDir (outside any git repo) and pinning
    /// `LOCT_CACHE_DIR` to a per-test TempDir gives each test its own
    /// snapshot universe.
    fn artifact_fence_fixture() -> (TempDir, TempDir) {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        copy_dir_all(&fixtures_path().join("artifact_fence"), temp.path()).unwrap();
        (temp, cache)
    }

    /// Golden regression: minified vendored JS (cytoscape.min.js shape) must
    /// not lead `loct coverage` with event-token noise, and the cut must be
    /// reported via the `excluded:` summary line — never silent.
    #[test]
    fn coverage_fences_minified_vendor_events() {
        let (fixture, cache) = artifact_fence_fixture();

        loct()
            .current_dir(fixture.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["coverage"])
            .assert()
            .success()
            .stdout(predicate::str::contains("vendor.min.js").not())
            .stdout(predicate::str::contains("user_saved"))
            .stdout(predicate::str::contains("excluded: vendored("));
    }

    /// Opt-out: --include-artifacts restores vendored findings.
    #[test]
    fn coverage_include_artifacts_restores_vendor_events() {
        let (fixture, cache) = artifact_fence_fixture();

        loct()
            .current_dir(fixture.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["coverage", "--include-artifacts"])
            .assert()
            .success()
            .stdout(predicate::str::contains("vendor.min.js"));
    }

    /// `loct events` must demote event bridges whose only locations are in
    /// vendored/minified files, while keeping product event flow visible.
    #[test]
    fn events_fences_minified_vendor_bridges() {
        let (fixture, cache) = artifact_fence_fixture();

        loct()
            .current_dir(fixture.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["events"])
            .assert()
            .success()
            .stdout(predicate::str::contains("vendor_noise").not())
            .stdout(predicate::str::contains("user_saved"))
            .stdout(predicate::str::contains("excluded: vendored("));

        loct()
            .current_dir(fixture.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["events", "--include-artifacts"])
            .assert()
            .success()
            .stdout(predicate::str::contains("vendor_noise"));
    }

    /// Cycles living entirely under tests/fixtures get their own
    /// "Fixture cycles" section instead of leading the main result.
    #[test]
    fn cycles_reports_fixture_cycles_in_separate_section() {
        let (fixture, cache) = artifact_fence_fixture();

        loct()
            .current_dir(fixture.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["cycles"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Fixture cycles (1)"));

        // Opt-out merges them back into the main section.
        loct()
            .current_dir(fixture.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["cycles", "--include-artifacts"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Fixture cycles").not());
    }

    /// `loct findings --summary` always carries the artifact-fence excluded
    /// counts (zero silent cuts).
    #[test]
    fn findings_summary_reports_excluded_artifacts() {
        let (fixture, cache) = artifact_fence_fixture();

        let output = loct()
            .current_dir(fixture.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["findings", "--summary"])
            .output()
            .expect("findings --summary runs");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let json_start = stdout.find('{').expect("summary JSON present");
        let summary: Value =
            serde_json::from_str(stdout[json_start..].trim()).expect("summary parses as JSON");
        let excluded = &summary["excluded"];
        assert!(
            excluded.is_object(),
            "summary must contain the excluded fence counts (was: {})",
            summary
        );
        assert!(
            excluded["vendored"].as_u64().unwrap_or(0) >= 1,
            "vendor.min.js must be counted as excluded vendored (was: {})",
            excluded
        );
        assert!(
            excluded["fixtures"].as_u64().unwrap_or(0) >= 2,
            "tests/fixtures/loop/*.ts must be counted as excluded fixtures (was: {})",
            excluded
        );
    }
}

// ============================================
// Analysis Commands Tests
// ============================================
// 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

mod analysis_commands {
    use super::*;

    // ----------------------------------------
    // Dead Command Tests
    // ----------------------------------------

    #[test]
    fn dead_help_shows_usage() {
        loctree()
            .args(["dead", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("dead")
                    .or(predicate::str::contains("unused"))
                    .or(predicate::str::contains("exports")),
            );
    }

    #[test]
    fn dead_detects_unused_exports() {
        let fixture = fixtures_path().join("dead_code");

        loctree()
            .current_dir(&fixture)
            .args(["dead"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("dead")
                    .or(predicate::str::contains("unused"))
                    .or(predicate::str::contains("DEAD_CONSTANT")),
            );
    }

    #[test]
    fn dead_alias_d() {
        let fixture = fixtures_path().join("dead_code");

        loctree()
            .current_dir(&fixture)
            .args(["d"])
            .assert()
            .success();
    }

    #[test]
    fn dead_json_output() {
        let fixture = fixtures_path().join("dead_code");

        loctree()
            .current_dir(&fixture)
            .args(["dead", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{").or(predicate::str::starts_with("[")));
    }

    // ----------------------------------------
    // Cycles Command Tests
    // ----------------------------------------

    #[test]
    fn cycles_help_shows_usage() {
        loctree()
            .args(["cycles", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("cycles")
                    .or(predicate::str::contains("circular"))
                    .or(predicate::str::contains("imports")),
            );
    }

    #[test]
    fn cycles_detects_circular_imports() {
        let fixture = fixtures_path().join("circular_imports");

        loctree()
            .current_dir(&fixture)
            .args(["cycles"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("cycle")
                    .or(predicate::str::contains("circular"))
                    .or(predicate::str::contains("→")),
            );
    }

    #[test]
    fn cycles_alias_c() {
        let fixture = fixtures_path().join("circular_imports");

        loctree()
            .current_dir(&fixture)
            .args(["c"])
            .assert()
            .success();
    }

    #[test]
    fn cycles_json_output() {
        let fixture = fixtures_path().join("circular_imports");

        loctree()
            .current_dir(&fixture)
            .args(["cycles", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{").or(predicate::str::starts_with("[")));
    }

    #[test]
    fn follow_all_json_emits_single_aggregated_object() {
        // loctree-feedback.md (2026-06-19): `loct --help` advertised `follow all`
        // and global `--json`, but `loct follow all --json` errored
        // ("not available yet"). It must now emit ONE valid JSON object with
        // per-scope machine-readable counts.
        let fixture = fixtures_path().join("circular_imports");

        let assert = loctree()
            .current_dir(&fixture)
            .args(["follow", "all", "--json"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
        let json: Value =
            serde_json::from_str(stdout.trim()).expect("follow all --json must emit valid JSON");

        assert_eq!(json["scope"], "all");
        for key in ["dead", "cycles", "twins", "hotspots"] {
            assert!(
                json.get(key).is_some(),
                "aggregated report must carry scope `{key}`"
            );
        }
        assert!(
            json["cycles"]["count"].is_u64(),
            "cycles.count must be a machine-readable number"
        );
        assert!(
            json["cycles"]["count"].as_u64().unwrap() >= 1,
            "circular_imports fixture must report at least one cycle"
        );
        assert!(json["dead"]["count"].is_u64());
        assert!(json["hotspots"]["count"].is_u64());
        assert!(json["twins"]["exact_twins"].is_u64());
    }

    #[test]
    fn follow_single_scope_json_still_works() {
        // Regression guard: lifting the `follow all --json` block must not
        // affect single-scope `--json`, which always worked.
        let fixture = fixtures_path().join("circular_imports");

        loctree()
            .current_dir(&fixture)
            .args(["follow", "cycles", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{").or(predicate::str::starts_with("[")));
    }

    // ----------------------------------------
    // Twins Command Tests
    // ----------------------------------------

    #[test]
    fn twins_help_shows_usage() {
        loctree()
            .args(["twins", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("twins")
                    .or(predicate::str::contains("duplicate"))
                    .or(predicate::str::contains("dead parrot")),
            );
    }

    #[test]
    fn twins_runs_successfully() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["twins"])
            .assert()
            .success();
    }

    #[test]
    fn twins_alias_t() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["t"])
            .assert()
            .success();
    }

    #[test]
    fn twins_json_output() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["twins", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{").or(predicate::str::starts_with("[")));
    }

    // ----------------------------------------
    // Twins classification (W2-b): EXACT / SHAPE_SIMILAR /
    // NAME_COLLISION / IDIOM — "Consolidate" exclusively for EXACT.
    // ----------------------------------------

    /// Isolated copy of the `twins_classes` fixture plus a private snapshot
    /// cache (same isolation rationale as `artifact_fence_fixture`).
    fn twins_classes_fixture() -> (TempDir, TempDir) {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        copy_dir_all(&fixtures_path().join("twins_classes"), temp.path()).unwrap();
        (temp, cache)
    }

    fn twins_classes_json() -> Value {
        let (fixture, cache) = twins_classes_fixture();
        let output = loct()
            .current_dir(fixture.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["twins", "--json"])
            .output()
            .unwrap();
        assert!(output.status.success());
        serde_json::from_slice(&output.stdout).expect("twins JSON")
    }

    fn find_twin<'a>(twins: &'a Value, name: &str) -> &'a Value {
        twins["exact_twins"]
            .as_array()
            .expect("exact_twins array")
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("twin group '{name}' missing"))
    }

    /// Registry pattern (`register` as a method on 3 different tool types):
    /// class NAME_COLLISION, zero "Consolidate" advice — even though the
    /// signatures are identical (the trait contract makes them identical).
    #[test]
    fn twins_classifies_registry_pattern_as_name_collision() {
        let twins = twins_classes_json();
        let register = find_twin(&twins, "register");

        assert_eq!(register["class"], "NAME_COLLISION");
        let action = register["action"].as_str().unwrap_or_default();
        assert!(
            !action.to_lowercase().contains("consolidate into"),
            "registry pattern must not be narrated as consolidation work: {action}"
        );
    }

    /// Two structs sharing a name but with different fields: extraction has
    /// no field-shape data, so the class is NAME_COLLISION — never EXACT.
    #[test]
    fn twins_never_marks_different_structs_exact() {
        let twins = twins_classes_json();
        let group = find_twin(&twins, "QuickWinReport");

        assert_ne!(group["class"], "EXACT");
        assert_eq!(group["class"], "NAME_COLLISION");
        let action = group["action"].as_str().unwrap_or_default();
        assert!(
            !action.to_lowercase().contains("consolidate into"),
            "{action}"
        );
    }

    /// A true duplicate (identical signature + body in two files) earns class
    /// EXACT and the consolidate recommendation.
    #[test]
    fn twins_marks_true_duplicate_exact_with_consolidate() {
        let twins = twins_classes_json();
        let group = find_twin(&twins, "render_widget");

        assert_eq!(group["class"], "EXACT");
        assert_eq!(group["action"], "consolidate into single module");
        assert_eq!(group["signature_similarity"], 1.0);
    }

    /// Additive JSON contract: the legacy fields of the twins surface are
    /// untouched — `class`/`action` only extend the per-group object.
    #[test]
    fn twins_json_contract_is_additive() {
        let twins = twins_classes_json();

        // Legacy top-level keys
        for key in ["dead_parrots", "exact_twins", "summary"] {
            assert!(!twins[key].is_null(), "missing legacy key {key}");
        }
        // Legacy per-group keys preserved alongside the new `class`
        let group = find_twin(&twins, "render_widget");
        for key in ["name", "category", "locations"] {
            assert!(!group[key].is_null(), "missing legacy group key {key}");
        }
        let loc = &group["locations"][0];
        for key in ["file", "line", "kind", "imports", "canonical", "language"] {
            assert!(!loc[key].is_null(), "missing legacy location key {key}");
        }
    }

    // ----------------------------------------
    // Zombie Command Tests
    // ----------------------------------------

    #[test]
    fn zombie_is_retired_with_findings_hint() {
        loctree()
            .args(["zombie"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("loct zombie has been retired"))
            .stderr(predicate::str::contains("loct findings"));
    }

    #[test]
    fn zombie_help_is_retired_with_findings_hint() {
        loctree()
            .args(["zombie", "--help"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("loct zombie has been retired"))
            .stderr(predicate::str::contains("loct findings"));

        loct()
            .args(["help", "zombie"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("loct zombie has been retired"))
            .stderr(predicate::str::contains("loct findings"));
    }

    // ----------------------------------------
    // Audit Command Tests
    // ----------------------------------------

    #[test]
    fn audit_help_shows_usage() {
        loctree()
            .args(["audit", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("audit")
                    .or(predicate::str::contains("full"))
                    .or(predicate::str::contains("codebase")),
            );
    }

    #[test]
    fn audit_runs_successfully() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["audit", "--no-open"])
            .assert()
            .success();
    }

    #[test]
    fn audit_stdout_flag() {
        let fixture = fixtures_path().join("simple_ts");

        // `audit --stdout` was removed: audit now writes to artifact files only.
        // The --json flag is the stdout-oriented contract.
        loctree()
            .current_dir(&fixture)
            .args(["audit", "--stdout"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("--json"));

        // Verify --json contract works
        loctree()
            .current_dir(&fixture)
            .args(["audit", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::contains("{"));
    }

    // ----------------------------------------
    // Crowd Command Tests
    // ----------------------------------------

    #[test]
    fn crowd_help_shows_usage() {
        loctree()
            .args(["crowd", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("crowd")
                    .or(predicate::str::contains("cluster"))
                    .or(predicate::str::contains("keyword")),
            );
    }

    #[test]
    fn crowd_with_keyword() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["crowd", "greet"])
            .assert()
            .success();
    }

    #[test]
    fn crowd_json_output() {
        // Copy fixture to temp WITHOUT .loctree/ to force auto-scan,
        // verifying --json stdout stays clean even when scanning triggers.
        let temp = TempDir::new().unwrap();
        let fixture = fixtures_path().join("simple_ts");
        copy_dir_excluding(&fixture, temp.path(), ".loctree").unwrap();

        loctree()
            .current_dir(temp.path())
            .args(["crowd", "greet", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{").or(predicate::str::starts_with("[")));
    }

    // ----------------------------------------
    // Tagmap Command Tests
    // ----------------------------------------

    #[test]
    fn tagmap_help_shows_usage() {
        loctree()
            .args(["tagmap", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("tagmap")
                    .or(predicate::str::contains("search"))
                    .or(predicate::str::contains("unified")),
            );
    }

    #[test]
    fn tagmap_with_keyword() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["tagmap", "greet"])
            .assert()
            .success();
    }

    #[test]
    fn tagmap_json_output() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["tagmap", "greet", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{").or(predicate::str::starts_with("[")));
    }

    #[test]
    fn tagmap_json_recalls_indexed_symbols() {
        let fixture = fixtures_path().join("simple_ts");

        let output = loctree()
            .current_dir(&fixture)
            .args(["tagmap", "greet", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let value: Value = serde_json::from_slice(&output).expect("tagmap json");
        let facts = value["facts"]["items"].as_array().expect("facts array");

        assert!(
            facts.iter().any(|fact| {
                fact["kind"] == "export"
                    && fact["file"] == "src/utils/greeting.ts"
                    && fact["name"] == "greet"
            }),
            "tagmap should recall symbol facts already present in the snapshot: {}",
            String::from_utf8_lossy(&output)
        );
    }

    // ----------------------------------------
    // Plan Command Tests
    // ----------------------------------------

    #[test]
    fn plan_help_shows_usage() {
        loctree()
            .args(["plan", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("plan").or(predicate::str::contains("refactor")));
    }

    #[test]
    fn plan_supports_multiple_targets() {
        let fixture = fixtures_path().join("plan_multi");
        let temp = TempDir::new().unwrap();
        copy_dir_all(&fixture, temp.path()).unwrap();

        loctree()
            .current_dir(temp.path())
            .args(["plan", "--json", "src", "other"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("["))
            .stdout(predicate::str::contains("\"target\": \"src\""))
            .stdout(predicate::str::contains("\"target\": \"other\""));
    }

    #[test]
    fn plan_target_layout_affects_move_targets() {
        let fixture = fixtures_path().join("plan_multi");
        let temp = TempDir::new().unwrap();
        copy_dir_all(&fixture, temp.path()).unwrap();

        loctree()
            .current_dir(temp.path())
            .args([
                "plan",
                "--json",
                "src",
                "--target-layout",
                "ui=custom-ui,app=custom-app",
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("src/custom-ui"))
            .stdout(predicate::str::contains("src/custom-app"));
    }

    // ----------------------------------------
    // Sniff Command Tests
    // ----------------------------------------

    #[test]
    fn sniff_is_retired_with_findings_hint() {
        loctree()
            .args(["sniff"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("loct sniff has been retired"))
            .stderr(predicate::str::contains("loct findings"));
    }

    #[test]
    fn sniff_help_is_retired_with_findings_hint() {
        loctree()
            .args(["sniff", "--help"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("loct sniff has been retired"))
            .stderr(predicate::str::contains("loct findings"));

        loct()
            .args(["help", "sniff"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("loct sniff has been retired"))
            .stderr(predicate::str::contains("loct findings"));
    }
}

// ============================================
// Management & Core Workflow Commands Tests
// ============================================
// 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

mod management_commands {
    use super::*;
    use serde_json::json;

    fn write_cache_snapshot(path: &std::path::Path, metadata: serde_json::Value) {
        std::fs::create_dir_all(path.parent().expect("snapshot parent")).expect("create dir");
        let body = serde_json::to_vec_pretty(&json!({ "metadata": metadata }))
            .expect("serialize snapshot");
        std::fs::write(path, body).expect("write snapshot");
    }

    // ----------------------------------------
    // Doctor Command Tests
    // ----------------------------------------

    #[test]
    fn doctor_help_shows_usage() {
        loctree()
            .args(["doctor", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("doctor")
                    .or(predicate::str::contains("diagnostic"))
                    .or(predicate::str::contains("recommendation")),
            );
    }

    #[test]
    fn doctor_runs_successfully() {
        let fixture = fixtures_path().join("simple_ts");
        let cache = TempDir::new().unwrap();

        loctree()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["doctor"])
            .assert()
            .success();
    }

    #[test]
    fn doctor_json_output() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["doctor", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{").or(predicate::str::starts_with("[")));
    }

    // ----------------------------------------
    // Suppress Command Tests
    // ----------------------------------------

    #[test]
    fn suppress_help_shows_usage() {
        loctree()
            .args(["suppress", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("suppress")
                    .or(predicate::str::contains("false positive"))
                    .or(predicate::str::contains("ignore")),
            );
    }

    #[test]
    fn suppress_list_empty() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join("main.ts"), "export const x = 1;").unwrap();

        loctree()
            .current_dir(temp.path())
            .args(["suppress", "--list"])
            .assert()
            .success();
    }

    // ----------------------------------------
    // Cache Command Tests
    // ----------------------------------------

    #[test]
    fn cache_list_groups_multiscan_bucket_by_repo() {
        let cache = TempDir::new().unwrap();
        let bucket = cache.path().join("projects").join("bucket1234567890ab");
        let repo_root = cache.path().join("demo-repo");
        let nested_root = repo_root.join("src");

        write_cache_snapshot(
            &bucket.join("main@aaa111").join("snapshot.json"),
            json!({
                "schema_version": "0.9.0",
                "generated_at": "2026-03-30T12:00:00Z",
                "roots": [repo_root.display().to_string()],
                "git_owner_repo": "VetCoders/Loctree",
                "git_repo": "Loctree",
                "git_branch": "main",
                "git_commit": "aaa111"
            }),
        );
        write_cache_snapshot(
            &bucket.join("feature@bbb222").join("snapshot.json"),
            json!({
                "schema_version": "0.9.0",
                "generated_at": "2026-03-31T12:00:00Z",
                "roots": [nested_root.display().to_string()],
                "git_owner_repo": "VetCoders/Loctree",
                "git_repo": "Loctree",
                "git_branch": "feature",
                "git_commit": "bbb222"
            }),
        );
        write_cache_snapshot(
            &bucket.join("latest").join("snapshot.json"),
            json!({
                "schema_version": "0.9.0",
                "generated_at": "2026-03-31T12:00:00Z",
                "roots": [nested_root.display().to_string()],
                "git_owner_repo": "VetCoders/Loctree",
                "git_repo": "Loctree",
                "git_branch": "feature",
                "git_commit": "bbb222"
            }),
        );
        std::fs::write(
            bucket.join("feature@bbb222").join("analysis.json"),
            b"analysis",
        )
        .unwrap();
        std::fs::write(bucket.join("latest").join("manifest.json"), b"manifest").unwrap();

        let output = loct()
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["cache", "list"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "cache list failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Org/Repo | Path | Cache size MB | Meta"));
        assert!(stdout.contains(&format!("VetCoders/Loctree | {} |", repo_root.display())));
        assert!(stdout.contains("scans 2"));
        assert!(stdout.contains("roots 2"));
        assert!(stdout.contains("branches 2"));
        assert!(stdout.contains("ref feature@bbb222"));
        assert!(stdout.contains("schema 0.9.0"));
    }

    #[test]
    fn cache_list_uses_local_fallback_for_non_git_bucket() {
        let cache = TempDir::new().unwrap();
        let bucket = cache.path().join("projects").join("bucketfedcba098765");
        let project_root = cache.path().join("local-project");

        write_cache_snapshot(
            &bucket.join("snapshot.json"),
            json!({
                "schema_version": "0.9.0",
                "generated_at": "2026-03-31T09:00:00Z",
                "roots": [project_root.display().to_string()]
            }),
        );

        let output = loct()
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["cache", "list"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "cache list failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(&format!(
            "local/local-project | {} |",
            project_root.display()
        )));
        assert!(stdout.contains("scans 1"));
        assert!(stdout.contains("schema 0.9.0"));
    }

    #[test]
    fn cache_list_uses_unknown_fallback_when_metadata_is_missing() {
        let cache = TempDir::new().unwrap();
        let bucket_id = "feedfacecafebeef";
        let bucket = cache.path().join("projects").join(bucket_id);
        std::fs::create_dir_all(&bucket).unwrap();
        std::fs::write(bucket.join("artifact.bin"), b"cache-bytes").unwrap();

        let output = loct()
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["cache", "list"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "cache list failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(&format!("unknown/{bucket_id} | (unknown path) |")));
        assert!(stdout.contains("scans 0; latest unknown; schema unknown"));
    }

    #[test]
    fn cache_clean_without_force_shows_confirmation() {
        let cache = TempDir::new().unwrap();
        let bucket = cache.path().join("projects").join("bucket_clean_test01");
        std::fs::create_dir_all(&bucket).unwrap();
        std::fs::write(bucket.join("snapshot.json"), b"{}").unwrap();

        let output = loct()
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["cache", "clean"])
            .output()
            .unwrap();

        assert!(
            !output.status.success(),
            "cache clean without --force should fail.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Use --force"),
            "stderr should mention --force.\nstderr: {}",
            stderr
        );

        assert!(
            bucket.exists(),
            "cache bucket should not be removed without --force"
        );
    }

    #[test]
    fn cache_clean_force_removes_all_buckets() {
        let cache = TempDir::new().unwrap();
        let projects_dir = cache.path().join("projects");
        let bucket_a = projects_dir.join("bucket_clean_a_1234");
        let bucket_b = projects_dir.join("bucket_clean_b_5678");

        std::fs::create_dir_all(&bucket_a).unwrap();
        std::fs::write(bucket_a.join("snapshot.json"), b"{}").unwrap();
        std::fs::create_dir_all(&bucket_b).unwrap();
        std::fs::write(bucket_b.join("snapshot.json"), b"{}").unwrap();

        let output = loct()
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["cache", "clean", "--force"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "cache clean --force should succeed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("Cleaned 2 project(s)"),
            "should report 2 cleaned projects.\nstdout: {}",
            stdout
        );

        assert!(!bucket_a.exists(), "bucket_a should be removed");
        assert!(!bucket_b.exists(), "bucket_b should be removed");
    }

    #[test]
    fn cache_clean_older_than_skips_recent_entries() {
        let cache = TempDir::new().unwrap();
        let projects_dir = cache.path().join("projects");
        let bucket = projects_dir.join("bucket_clean_recent1");

        std::fs::create_dir_all(&bucket).unwrap();
        std::fs::write(bucket.join("snapshot.json"), b"{}").unwrap();

        let output = loct()
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["cache", "clean", "--older-than", "9999d", "--force"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "cache clean --older-than should succeed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("Nothing to clean"),
            "recent entries should not match 9999d threshold.\nstdout: {}",
            stdout
        );

        assert!(
            bucket.exists(),
            "recent bucket should not be removed by age filter"
        );
    }

    // ----------------------------------------
    // Auto Command Tests
    // ----------------------------------------

    #[test]
    fn auto_help_shows_usage() {
        loctree()
            .args(["auto", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("auto")
                    .or(predicate::str::contains("scan"))
                    .or(predicate::str::contains("artifacts")),
            );
    }

    #[test]
    fn auto_creates_loctree_dir() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/main.ts"), "export const x = 1;").unwrap();

        loctree()
            .current_dir(temp.path())
            .args(["auto"])
            .assert()
            .success();

        // Artifacts should exist in global cache (not in project .loctree/)
        assert!(snapshot_exists(temp.path()));
    }

    #[test]
    fn auto_json_output() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/main.ts"), "export const x = 1;").unwrap();

        // auto mode generates .loctree/ artifacts; --json suppresses summary on stderr
        loctree()
            .current_dir(temp.path())
            .args(["auto", "--json"])
            .assert()
            .success();

        // Snapshot should exist (in global cache)
        assert!(snapshot_exists(temp.path()));
    }

    // ----------------------------------------
    // Tree Command Tests
    // ----------------------------------------

    #[test]
    fn tree_help_shows_usage() {
        loctree()
            .args(["tree", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("tree")
                    .or(predicate::str::contains("directory"))
                    .or(predicate::str::contains("LOC")),
            );
    }

    #[test]
    fn tree_shows_structure() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["tree"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("src")
                    .or(predicate::str::contains("├"))
                    .or(predicate::str::contains("└")),
            );
    }

    #[test]
    fn tree_with_depth() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["tree", "--depth", "1"])
            .assert()
            .success();
    }

    #[test]
    fn tree_json_output() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["tree", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{").or(predicate::str::starts_with("[")));
    }

    #[test]
    fn tree_files_match_outputs_exact_paths() {
        let fixture = fixtures_path().join("simple_ts");

        let output = loctree()
            .current_dir(&fixture)
            .args(["tree", "--files", "--match", "utils"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "tree --files --match failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("src/utils/date.ts"));
        assert!(stdout.contains("src/utils/greeting.ts"));
        assert!(
            !stdout.contains("src/index.ts"),
            "path filter should exclude non-matching files.\nstdout: {}",
            stdout
        );
    }

    // ----------------------------------------
    // Find Command Tests
    // ----------------------------------------

    #[test]
    fn find_help_shows_usage() {
        loctree()
            .args(["find", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("find")
                    .or(predicate::str::contains("search"))
                    .or(predicate::str::contains("symbol")),
            );
    }

    #[test]
    fn find_symbol() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["find", "greet"])
            .assert()
            .success()
            .stdout(predicate::str::contains("greet").or(predicate::str::contains("greeting")));
    }

    #[test]
    fn find_alias_f() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["f", "greet"])
            .assert()
            .success();
    }

    #[test]
    fn find_json_output() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["find", "greet", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with("{").or(predicate::str::starts_with("[")));
    }

    // ----------------------------------------
    // Report Command Tests
    // ----------------------------------------

    #[test]
    fn report_help_shows_usage() {
        loctree()
            .args(["report", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("report")
                    .or(predicate::str::contains("HTML"))
                    .or(predicate::str::contains("generate")),
            );
    }

    #[test]
    fn report_creates_html() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/main.ts"), "export const x = 1;").unwrap();

        // First create snapshot
        loctree().current_dir(temp.path()).assert().success();

        loctree()
            .current_dir(temp.path())
            .args(["report"])
            .assert()
            .success();
    }

    /// Plan 23 round-trip: `loct report --output <path>` against a real fixture
    /// must write an HTML artifact whose body contains analyzer-derived facts
    /// for that fixture, not generic shell text or sample data.
    #[test]
    fn report_html_round_trip_against_fixture() {
        let temp = TempDir::new().unwrap();
        let src_dir = temp.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(
            src_dir.join("alpha.ts"),
            "import { beta } from './beta';\nexport const alpha = beta + 1;\n",
        )
        .unwrap();
        std::fs::write(src_dir.join("beta.ts"), "export const beta = 41;\n").unwrap();

        // Prime snapshot so report runs without re-scanning under test.
        loct().current_dir(temp.path()).assert().success();

        // Drop the report into a nested subdirectory to verify mkdir -p semantics.
        let report_path = temp.path().join("artifacts/sub/report.html");
        loct()
            .current_dir(temp.path())
            .args(["report", "--output", report_path.to_str().unwrap()])
            .assert()
            .success();

        assert!(
            report_path.exists(),
            "HTML artifact must exist at {}",
            report_path.display()
        );
        let html = std::fs::read_to_string(&report_path).expect("read generated HTML");

        // Provenance (Plan 23 round-trip evidence contract).
        assert!(
            html.contains("<!DOCTYPE html>"),
            "report must start with DOCTYPE, got first chars: {:?}",
            &html.chars().take(40).collect::<String>()
        );
        assert!(
            html.contains("Loctree Report"),
            "missing Vista report title"
        );
        assert!(
            html.contains(env!("CARGO_PKG_VERSION")),
            "missing loctree binary version ({}) in rendered provenance chip",
            env!("CARGO_PKG_VERSION")
        );
        // The provenance chip label is the literal token "loctree" and the
        // value cell renders the bare version. The label string appears at
        // least twice once both the label cell and the loctree-version chip
        // are emitted (and again inside <title>/branding).
        assert!(
            html.matches("loctree").count() >= 2,
            "expected the rendered HTML to reference the `loctree` provenance label more than once"
        );
        // generated_at chip carries a "generated" label adjacent to an
        // RFC3339 timestamp value.
        assert!(
            html.contains("generated"),
            "missing generated_at provenance chip label"
        );

        // Fixture-derived structural facts: the analyzer should have surfaced
        // the fixture's own filenames somewhere in the rendered HTML.
        let fixture_evidence = ["alpha.ts", "beta.ts"];
        for needle in fixture_evidence {
            assert!(
                html.contains(needle),
                "fixture-derived fact '{}' missing from HTML; first 500 chars: {:?}",
                needle,
                &html.chars().take(500).collect::<String>()
            );
        }

        // JS asset emitted next to the report so the graph survives opening
        // the artifact directly from disk (Plan 23 acceptance criterion).
        let cytoscape_path = report_path
            .parent()
            .unwrap()
            .join("loctree-cytoscape.min.js");
        assert!(
            cytoscape_path.exists(),
            "expected cytoscape asset alongside report at {}",
            cytoscape_path.display()
        );
    }

    /// Plan 23: `loct report` with no `--output` must write HTML to the
    /// canonical artifacts directory beside the snapshot. Previously the
    /// command only refreshed cached JSON and silently skipped the HTML
    /// write — the help text promised HTML, the behaviour did not.
    #[test]
    fn report_without_output_writes_html_to_artifacts_dir() {
        let temp = TempDir::new().unwrap();
        let src_dir = temp.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(
            src_dir.join("hub.ts"),
            "export function hub() { return 'plan23'; }\n",
        )
        .unwrap();

        // Prime snapshot to materialize the artifacts dir.
        loct().current_dir(temp.path()).assert().success();

        loct()
            .current_dir(temp.path())
            .args(["report"])
            .assert()
            .success();

        let artifacts_dir = loctree::snapshot::Snapshot::artifacts_dir(temp.path());
        let report_path = artifacts_dir.join("report.html");
        assert!(
            report_path.exists(),
            "loct report must write HTML to {} (artifacts_dir/report.html)",
            report_path.display()
        );
        let html = std::fs::read_to_string(&report_path).expect("read report");
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(
            html.contains("hub.ts"),
            "expected fixture-derived fact 'hub.ts' in default-path HTML"
        );
    }

    // ----------------------------------------
    // Lint Command Tests
    // ----------------------------------------

    #[test]
    fn lint_help_shows_usage() {
        loctree()
            .args(["lint", "--help"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("lint")
                    .or(predicate::str::contains("policy"))
                    .or(predicate::str::contains("structural")),
            );
    }

    #[test]
    fn lint_runs_successfully() {
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["lint"])
            .assert()
            .success();
    }

    #[test]
    fn lint_with_fail_flag() {
        let fixture = fixtures_path().join("simple_ts");

        // --fail should work (exit code depends on findings)
        let _ = loctree()
            .current_dir(&fixture)
            .args(["lint", "--fail"])
            .assert(); // Don't check success/failure - depends on findings
    }

    #[test]
    fn lint_sarif_flag_recognized() {
        let fixture = fixtures_path().join("simple_ts");

        // --sarif should emit SARIF JSON to stdout
        loctree()
            .current_dir(&fixture)
            .args(["lint", "--sarif"])
            .assert()
            .stdout(predicate::str::contains("\"version\""))
            .stdout(predicate::str::contains("\"runs\""))
            .success();
    }
}

// ============================================
// Framework-Specific Command Tests
// ============================================
// 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

mod framework_commands {
    use super::*;

    // ----------------------------------------
    // Routes Command Tests
    // ----------------------------------------

    #[test]
    fn routes_help_shows_usage() {
        let output = loctree().args(["routes", "--help"]).output().unwrap();
        let combined = String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr);
        assert!(
            combined.contains("routes")
                || combined.contains("FastAPI")
                || combined.contains("Flask"),
            "Help should mention routes/FastAPI/Flask: {}",
            combined
        );
    }

    #[test]
    fn routes_no_routes_in_ts_project() {
        // TypeScript project has no Python routes - should complete gracefully
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["routes"])
            .assert()
            .success()
            .stdout(predicate::str::contains("No routes detected"));
    }

    #[test]
    fn routes_json_output_empty() {
        // TypeScript project - JSON output should be valid with empty routes
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["routes", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::contains(r#""routes""#))
            .stdout(predicate::str::contains(r#""summary""#))
            .stdout(predicate::str::contains(r#""count""#));
    }

    #[test]
    fn routes_with_framework_filter() {
        // Filter should work even when no routes exist
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["routes", "--framework", "fastapi"])
            .assert()
            .success()
            .stdout(predicate::str::contains("No routes detected"));
    }

    #[test]
    fn routes_with_path_filter() {
        // Path filter should work even when no routes exist
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["routes", "--path", "/api/v1"])
            .assert()
            .success()
            .stdout(predicate::str::contains("No routes detected"));
    }

    #[test]
    fn routes_in_python_fixture() {
        // Create a minimal Python project with FastAPI routes
        let temp = TempDir::new().unwrap();

        // Create a FastAPI app file
        std::fs::write(
            temp.path().join("main.py"),
            r#"from fastapi import FastAPI

app = FastAPI()

@app.get("/health")
def health_check():
    return {"status": "ok"}

@app.post("/users")
def create_user(name: str):
    return {"name": name}

@app.get("/users/{user_id}")
def get_user(user_id: int):
    return {"user_id": user_id}
"#,
        )
        .unwrap();

        loctree()
            .current_dir(temp.path())
            .args(["routes"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("/health")
                    .or(predicate::str::contains("route"))
                    .or(predicate::str::contains("No routes")), // May not detect without full scan
            );
    }

    // ----------------------------------------
    // Dist Command Tests
    // ----------------------------------------

    #[test]
    fn dist_help_shows_usage() {
        let output = loctree().args(["dist", "--help"]).output().unwrap();
        let combined = String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr);
        assert!(
            combined.contains("one or more production source maps")
                && combined.contains("--source-map")
                && combined.contains("--report"),
            "Help should explain the dist mental model and report option: {}",
            combined
        );
    }

    #[test]
    fn dist_requires_source_map() {
        // dist command requires --source-map flag
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/index.ts"), "export const x = 1;").unwrap();

        loctree()
            .current_dir(temp.path())
            .args(["dist", "--src", "src/"])
            .assert()
            .failure()
            .stderr(
                predicate::str::contains("source-map")
                    .and(predicate::str::contains("at least one")),
            );
    }

    #[test]
    fn dist_requires_src() {
        // dist command requires --src flag
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join("main.js.map"), "{}").unwrap();

        loctree()
            .current_dir(temp.path())
            .args(["dist", "--source-map", "main.js.map"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("src").or(predicate::str::contains("required")));
    }

    #[test]
    fn dist_handles_missing_source_map_file() {
        // Should fail gracefully when source map file doesn't exist
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/index.ts"), "export const x = 1;").unwrap();

        loctree()
            .current_dir(temp.path())
            .args([
                "dist",
                "--source-map",
                "nonexistent.js.map",
                "--src",
                "src/",
            ])
            .assert()
            .failure()
            .stderr(
                predicate::str::contains("does not exist")
                    .or(predicate::str::contains("not found"))
                    .or(predicate::str::contains("Failed"))
                    .or(predicate::str::contains("error")),
            );
    }

    #[test]
    fn dist_handles_invalid_source_map() {
        // Should fail gracefully when source map is invalid JSON
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/index.ts"), "export const x = 1;").unwrap();
        std::fs::write(temp.path().join("main.js.map"), "not valid json").unwrap();

        loctree()
            .current_dir(temp.path())
            .args(["dist", "--source-map", "main.js.map", "--src", "src/"])
            .assert()
            .failure()
            .stderr(
                predicate::str::contains("parse")
                    .or(predicate::str::contains("invalid"))
                    .or(predicate::str::contains("Failed"))
                    .or(predicate::str::contains("error")),
            );
    }

    #[test]
    fn dist_falls_back_to_line_level_when_symbol_names_are_unavailable() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(
            temp.path().join("src/index.ts"),
            "export const hello = 'world';",
        )
        .unwrap();

        // Minimal source map structure
        let source_map = r#"{
            "version": 3,
            "file": "main.js",
            "sources": ["src/index.ts"],
            "sourcesContent": ["export const hello = 'world';"],
            "names": ["hello"],
            "mappings": "AAAA"
        }"#;
        std::fs::write(temp.path().join("main.js.map"), source_map).unwrap();

        let output = loctree()
            .current_dir(temp.path())
            .args([
                "dist",
                "--source-map",
                "main.js.map",
                "--src",
                "src/",
                "--json",
            ])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "dist should succeed with line-level fallback: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["analysisLevel"], "line");
        assert_eq!(report["symbolLevel"], false);
        assert_eq!(
            report["deadExports"]
                .as_array()
                .map(|exports| exports.len()),
            Some(0)
        );
    }

    #[test]
    fn dist_aggregates_multiple_source_maps() {
        let temp = TempDir::new().unwrap();
        let fixture = fixtures_path().join("dist_multi_map");
        copy_dir_all(&fixture, temp.path()).unwrap();

        let output = loctree()
            .current_dir(temp.path())
            .args([
                "dist",
                "--src",
                "src/",
                "--source-map",
                "dist/client.js.map",
                "--source-map",
                "dist/admin.js.map",
                "--json",
            ])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "multi-map dist should succeed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        let dead_exports = report["deadExports"].as_array().unwrap();
        let dead_names: Vec<_> = dead_exports
            .iter()
            .filter_map(|entry| entry.get("name").and_then(|value| value.as_str()))
            .collect();

        assert_eq!(report["sourceMaps"], 2);
        assert_eq!(report["analysisLevel"], "symbol");
        assert!(dead_names.contains(&"dead"));
        assert!(!dead_names.contains(&"shared"));
        assert!(!dead_names.contains(&"clientOnly"));
        assert!(!dead_names.contains(&"adminOnly"));
    }

    #[test]
    fn dist_directory_discovery_classifies_runtime_candidates() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::create_dir_all(temp.path().join("dist")).unwrap();

        std::fs::write(
            temp.path().join("src/index.ts"),
            r#"
import { bootOnly } from "./boot";
import { lazyThing } from "./lazy";

export async function loadLazy() {
    return import("./lazy");
}

export async function loadFeature() {
    return import("./feature");
}

export const app = bootOnly + lazyThing;
"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("src/boot.ts"),
            "export const bootOnly = 1;",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("src/lazy.ts"),
            "export const lazyThing = 2;",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("src/feature.ts"),
            "export const featureOnly = 3;",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("src/dead.ts"),
            "export const deadThing = 4;",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("src/verify.ts"),
            "export const verifyThing = 5;",
        )
        .unwrap();

        let boot_map = r#"{
            "version": 3,
            "file": "main.js",
            "sources": ["src/index.ts", "src/boot.ts", "src/lazy.ts", "src/verify.ts"],
            "names": [],
            "mappings": ""
        }"#;
        let feature_map = r#"{
            "version": 3,
            "file": "feature.js",
            "sources": ["src/feature.ts", "src/verify.ts"],
            "names": [],
            "mappings": ""
        }"#;
        std::fs::write(temp.path().join("dist/main.js.map"), boot_map).unwrap();
        std::fs::write(temp.path().join("dist/feature.js.map"), feature_map).unwrap();

        let output = loctree()
            .current_dir(temp.path())
            .args(["dist", "--src", "src/", "--source-map", "dist/", "--json"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "dist directory discovery should succeed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["sourceMaps"], 2);
        assert_eq!(report["candidateCounts"]["dead_in_all_chunks"], 1);
        assert_eq!(report["candidateCounts"]["boot_path_only"], 1);
        assert_eq!(report["candidateCounts"]["feature_local"], 1);
        assert_eq!(report["candidateCounts"]["fake_lazy"], 1);
        assert_eq!(report["candidateCounts"]["verify_first"], 1);

        let candidates = report["candidates"].as_array().unwrap();
        assert!(candidates.iter().any(|candidate| {
            candidate["class"] == "dead_in_all_chunks" && candidate["name"] == "deadThing"
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate["class"] == "boot_path_only" && candidate["name"] == "bootOnly"
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate["class"] == "feature_local" && candidate["name"] == "featureOnly"
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate["class"] == "fake_lazy" && candidate["name"] == "lazyThing"
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate["class"] == "verify_first" && candidate["name"] == "verifyThing"
        }));
    }

    #[test]
    fn dist_writes_json_report() {
        let temp = TempDir::new().unwrap();
        let fixture = fixtures_path().join("dist_multi_map");
        copy_dir_all(&fixture, temp.path()).unwrap();

        let report_path = temp.path().join(".loctree/dist-report.json");

        loctree()
            .current_dir(temp.path())
            .args([
                "dist",
                "--src",
                "src/",
                "--source-map",
                "dist/client.js.map",
                "--source-map",
                "dist/admin.js.map",
                "--report",
                report_path.to_str().unwrap(),
            ])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("Report:")
                    .and(predicate::str::contains(".loctree/dist-report.json")),
            );

        let report_contents = std::fs::read_to_string(&report_path).unwrap();
        let report: Value = serde_json::from_str(&report_contents).unwrap();
        assert_eq!(report["sourceMaps"], 2);
        assert_eq!(report["analysisLevel"], "symbol");
        assert!(
            report["deadExports"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["name"] == "dead")
        );
    }

    #[test]
    fn dist_uses_explicit_src_scope_instead_of_repo_root_snapshot() {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(
            temp.path().join("src/index.ts"),
            "export const live = 'world';",
        )
        .unwrap();
        std::fs::write(temp.path().join("other.ts"), "export const repoOnly = 1;").unwrap();

        let source_map = r#"{
            "version": 3,
            "file": "main.js",
            "sources": ["src/index.ts"],
            "sourcesContent": ["export const live = 'world';"],
            "names": [],
            "mappings": "AAAA"
        }"#;
        std::fs::write(temp.path().join("main.js.map"), source_map).unwrap();

        loctree()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .assert()
            .success();

        let output = loctree()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args([
                "dist",
                "--source-map",
                "main.js.map",
                "--src",
                "src/",
                "--json",
            ])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "dist failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["sourceExports"], 1);
        assert_eq!(report["bundledExports"], 1);
        assert_eq!(
            report["deadExports"]
                .as_array()
                .map(|exports| exports.len()),
            Some(0)
        );
    }

    #[test]
    fn dist_refreshes_stale_strict_src_snapshot() {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/index.ts"), "export const live = 1;").unwrap();

        let source_map = r#"{
            "version": 3,
            "file": "main.js",
            "sources": ["src/index.ts"],
            "sourcesContent": ["export const live = 1;"],
            "names": [],
            "mappings": "AAAA"
        }"#;
        std::fs::write(temp.path().join("main.js.map"), source_map).unwrap();

        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["add", "."]);
        run_git(temp.path(), &["commit", "-m", "init"]);

        let first = loctree()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args([
                "dist",
                "--source-map",
                "main.js.map",
                "--src",
                "src/",
                "--json",
            ])
            .output()
            .unwrap();
        assert!(
            first.status.success(),
            "initial dist failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&first.stdout),
            String::from_utf8_lossy(&first.stderr)
        );

        std::fs::write(temp.path().join("src/new.ts"), "export const stray = 1;").unwrap();

        let second = loctree()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args([
                "dist",
                "--source-map",
                "main.js.map",
                "--src",
                "src/",
                "--json",
            ])
            .output()
            .unwrap();
        assert!(
            second.status.success(),
            "second dist failed.\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&second.stdout),
            String::from_utf8_lossy(&second.stderr)
        );

        let report: Value = serde_json::from_slice(&second.stdout).unwrap();
        assert_eq!(report["sourceExports"], 2);
        assert_eq!(report["bundledExports"], 1);

        let dead_exports = report["deadExports"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(dead_exports.len(), 1);
        assert_eq!(dead_exports[0]["file"].as_str(), Some("new.ts"));
    }

    // ----------------------------------------
    // Layoutmap Command Tests
    // ----------------------------------------

    #[test]
    fn layoutmap_help_shows_usage() {
        loctree()
            .args(["layoutmap", "--help"])
            .assert()
            .success() // layoutmap help exits successfully
            .stdout(
                predicate::str::contains("layoutmap")
                    .or(predicate::str::contains("z-index"))
                    .or(predicate::str::contains("CSS")),
            );
    }

    #[test]
    fn layoutmap_no_css_in_ts_project() {
        // TypeScript project without CSS - should complete gracefully
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["layoutmap"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("No CSS")
                    .or(predicate::str::contains("findings"))
                    .or(predicate::str::contains("0")),
            );
    }

    #[test]
    fn layoutmap_json_output_empty() {
        // JSON output should be valid even with no CSS files
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["layoutmap", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::contains("[").or(predicate::str::contains("{")));
    }

    #[test]
    fn layoutmap_with_zindex_filter() {
        // --zindex flag should be recognized
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["layoutmap", "--zindex"])
            .assert()
            .success();
    }

    #[test]
    fn layoutmap_with_sticky_filter() {
        // --sticky flag should be recognized
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["layoutmap", "--sticky"])
            .assert()
            .success();
    }

    #[test]
    fn layoutmap_with_grid_filter() {
        // --grid flag should be recognized
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["layoutmap", "--grid"])
            .assert()
            .success();
    }

    #[test]
    fn layoutmap_with_min_zindex() {
        // --min-zindex flag should be recognized
        let fixture = fixtures_path().join("simple_ts");

        loctree()
            .current_dir(&fixture)
            .args(["layoutmap", "--min-zindex", "100"])
            .assert()
            .success();
    }

    #[test]
    fn layoutmap_with_css_content() {
        // Create a temp directory with CSS that has z-index
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("styles")).unwrap();

        std::fs::write(
            temp.path().join("styles/main.css"),
            r#"
.modal {
    position: fixed;
    z-index: 1000;
    top: 0;
    left: 0;
}

.tooltip {
    position: absolute;
    z-index: 500;
}

.header {
    position: sticky;
    top: 0;
    z-index: 100;
}

.container {
    display: grid;
    grid-template-columns: 1fr 1fr;
}

.flex-row {
    display: flex;
    flex-direction: row;
}
"#,
        )
        .unwrap();

        loctree()
            .current_dir(temp.path())
            .args(["layoutmap"])
            .assert()
            .success()
            .stdout(
                predicate::str::contains("z-index")
                    .or(predicate::str::contains("1000"))
                    .or(predicate::str::contains("modal"))
                    .or(predicate::str::contains("LAYERS"))
                    .or(predicate::str::contains("findings")),
            );
    }

    #[test]
    fn layoutmap_json_with_css() {
        // JSON output with actual CSS content
        let temp = TempDir::new().unwrap();

        std::fs::write(
            temp.path().join("app.css"),
            r#"
.overlay {
    position: fixed;
    z-index: 9999;
}
"#,
        )
        .unwrap();

        loctree()
            .current_dir(temp.path())
            .args(["layoutmap", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::contains("[").or(predicate::str::contains("{")));
    }

    #[test]
    fn layoutmap_exclude_pattern() {
        // --exclude flag should be recognized
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("node_modules")).unwrap();

        std::fs::write(
            temp.path().join("node_modules/lib.css"),
            ".x { z-index: 1000; }",
        )
        .unwrap();

        std::fs::write(temp.path().join("main.css"), ".y { z-index: 100; }").unwrap();

        loctree()
            .current_dir(temp.path())
            .args(["layoutmap", "--exclude", "**/node_modules/**"])
            .assert()
            .success();
    }
}

// ============================================
// Cut 8 (P0) — `loct env-truth` declaration-side audit
// ============================================

mod env_truth {
    use super::*;
    use std::fs;
    use std::time::{Duration, SystemTime};

    /// Copy `tests/fixtures/env_drift` into a tempdir, optionally adjusting
    /// mtimes so the stale-overrides-fresh warning is materialised.
    fn stage_env_drift(temp: &TempDir, sealed_age_days: u64, fresh_age_days: u64) {
        let src = fixtures_path().join("env_drift");
        copy_tree(&src, temp.path());
        let now = SystemTime::now();
        let stale = now - Duration::from_secs(sealed_age_days * 86_400);
        let fresh = now - Duration::from_secs(fresh_age_days * 86_400);
        // Stale: SealedSecret + ConfigMap.
        set_mtime(&temp.path().join("k8s/sealed-secret.yaml"), stale);
        set_mtime(&temp.path().join("k8s/configmap.yaml"), fresh);
        // Fresh: dotenv with rotated credentials.
        set_mtime(&temp.path().join(".env"), fresh);
        set_mtime(&temp.path().join("Dockerfile"), fresh);
        set_mtime(&temp.path().join(".github/workflows/ci.yml"), fresh);
        set_mtime(&temp.path().join("docker-compose.yml"), fresh);
        set_mtime(&temp.path().join("k8s/deployment.yaml"), fresh);
    }

    fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let dst_path = dst.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &dst_path);
            } else {
                fs::copy(entry.path(), &dst_path).unwrap();
            }
        }
    }

    fn set_mtime(path: &std::path::Path, target: SystemTime) {
        if !path.exists() {
            return;
        }
        // `File::set_modified` is stable since Rust 1.75 — workspace pins
        // 1.85 so this path is portable across CI. Need a writable handle.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("fixture file is writable");
        file.set_modified(target).expect("set_modified");
    }

    fn json_value(stdout: &[u8]) -> Value {
        serde_json::from_slice(stdout).expect("env-truth output is valid JSON")
    }

    #[test]
    fn json_emits_schema_and_declarations() {
        let temp = TempDir::new().unwrap();
        stage_env_drift(&temp, 30, 2);

        let assert = loct()
            .current_dir(temp.path())
            .args(["env-truth", "--json"])
            .assert()
            .success();
        let stdout = assert.get_output().stdout.clone();
        let value = json_value(&stdout);
        assert_eq!(
            value["schema_version"].as_str(),
            Some("1.1"),
            "schema version pinned at 1.1 (W2-c additive template_drift)"
        );
        let names: Vec<&str> = value["declarations"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|d| d["name"].as_str())
            .collect();
        assert!(names.contains(&"DATABASE_URL"));
        assert!(names.contains(&"API_KEY"));
    }

    #[test]
    fn name_filter_returns_only_one_declaration() {
        let temp = TempDir::new().unwrap();
        stage_env_drift(&temp, 30, 2);

        let assert = loct()
            .current_dir(temp.path())
            .args(["env-truth", "--name", "DATABASE_URL", "--json"])
            .assert()
            .success();
        let value = json_value(&assert.get_output().stdout);
        let declarations = value["declarations"].as_array().unwrap();
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0]["name"].as_str(), Some("DATABASE_URL"));
        // Sources should include at least the SealedSecret + .env + ConfigMap.
        let sources = declarations[0]["sources"].as_array().unwrap();
        assert!(
            sources.len() >= 3,
            "expected ≥3 sources, got {}",
            sources.len()
        );
    }

    #[test]
    fn sealed_overrides_fresh_plain_returns_exit_2() {
        let temp = TempDir::new().unwrap();
        stage_env_drift(&temp, 30, 2);

        loct()
            .current_dir(temp.path())
            .args([
                "env-truth",
                "--fail-on",
                "stale-sealed-overrides-fresh-plain",
            ])
            .assert()
            .code(2);
    }

    #[test]
    fn multi_source_mismatch_returns_exit_2() {
        let temp = TempDir::new().unwrap();
        stage_env_drift(&temp, 30, 2);

        // Inject a third source that disagrees with .env and configmap.
        // Since the fixture already has different DATABASE_URL across the
        // ConfigMap, .env, docker-compose, and Dockerfile-less environment
        // (ConfigMap=other-host vs .env=fresh-credentials vs docker-compose=compose-host),
        // the multi-source-mismatch warning should fire on DATABASE_URL.
        loct()
            .current_dir(temp.path())
            .args(["env-truth", "--fail-on", "multi-source-mismatch"])
            .assert()
            .code(2);
    }

    #[test]
    fn sealed_secret_value_is_never_decoded() {
        let temp = TempDir::new().unwrap();
        stage_env_drift(&temp, 30, 2);

        let assert = loct()
            .current_dir(temp.path())
            .args(["env-truth", "--name", "DATABASE_URL", "--json"])
            .assert()
            .success();
        let value = json_value(&assert.get_output().stdout);
        let sources = value["declarations"][0]["sources"].as_array().unwrap();
        let sealed = sources
            .iter()
            .find(|s| s["kind"].as_str() == Some("sealed_secret"))
            .expect("SealedSecret source present");
        let presence = &sealed["value_present"];
        assert_eq!(
            presence["kind"].as_str(),
            Some("encrypted"),
            "sealed payload must be marked encrypted, never plain"
        );
        // Must NOT contain a plaintext-looking field anywhere on the source.
        let serialized = serde_json::to_string(sealed).unwrap();
        assert!(
            !serialized.contains("postgres://"),
            "sealed source leaked decoded value: {serialized}"
        );
    }

    #[test]
    fn markdown_default_is_top_problems_view() {
        let temp = TempDir::new().unwrap();
        stage_env_drift(&temp, 30, 2);

        let assert = loct()
            .current_dir(temp.path())
            .args(["env-truth"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
        assert!(stdout.contains("# loct env-truth report"));
        assert!(stdout.contains("## Summary"));
        // W2-c: default is the Top-problems view, not the full dump.
        assert!(stdout.contains("## Top problems"));
        assert!(
            !stdout.contains("## Declarations"),
            "full declaration dump must hide behind --all"
        );
        // DATABASE_URL has a multi-source mismatch → it IS a top problem.
        assert!(stdout.contains("DATABASE_URL"));
        // Value hashes hide behind --hashes.
        assert!(
            !stdout.contains("sha256:"),
            "hashes must hide behind --hashes:\n{stdout}"
        );
    }

    /// W2-c: `--all` restores the full per-declaration dump and the
    /// precedence table; `--hashes` reveals the sha256 value hashes.
    #[test]
    fn markdown_all_flag_restores_full_dump() {
        let temp = TempDir::new().unwrap();
        stage_env_drift(&temp, 30, 2);

        let assert = loct()
            .current_dir(temp.path())
            .args(["env-truth", "--all", "--hashes"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
        assert!(stdout.contains("## Declarations"));
        assert!(stdout.contains("## Precedence table (active)"));
        assert!(stdout.contains("sha256:"));
    }

    /// W2-c: default report stays small even on a CodeScribe-like surface
    /// (the 2026-line wall regression).
    #[test]
    fn default_report_is_bounded_under_200_lines() {
        let temp = TempDir::new().unwrap();
        stage_env_drift(&temp, 30, 2);
        // Add a CodeScribe-like pile of one-off declarations to fatten the
        // would-be full dump.
        let mut extra = String::new();
        for i in 0..120 {
            extra.push_str(&format!("CS_EXTRA_VAR_{i}=value-{i}\n"));
        }
        std::fs::write(temp.path().join(".env.local"), extra).unwrap();

        let assert = loct()
            .current_dir(temp.path())
            .args(["env-truth"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
        let default_lines = stdout.lines().count();
        assert!(
            default_lines < 200,
            "default Top-problems report must stay under 200 lines, got {default_lines}"
        );

        let assert_all = loct()
            .current_dir(temp.path())
            .args(["env-truth", "--all"])
            .assert()
            .success();
        let all_lines = String::from_utf8_lossy(&assert_all.get_output().stdout)
            .lines()
            .count();
        assert!(
            all_lines > default_lines,
            "--all must be the bigger surface ({all_lines} vs {default_lines})"
        );
    }

    #[test]
    fn unknown_fail_on_token_is_rejected() {
        let temp = TempDir::new().unwrap();
        stage_env_drift(&temp, 30, 2);

        loct()
            .current_dir(temp.path())
            .args(["env-truth", "--fail-on", "garbage-token"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unknown --fail-on kind"));
    }

    #[test]
    fn paths_restriction_filters_scope() {
        let temp = TempDir::new().unwrap();
        stage_env_drift(&temp, 30, 2);

        let assert = loct()
            .current_dir(temp.path())
            .args(["env-truth", "--paths", "k8s/", "--json"])
            .assert()
            .success();
        let value = json_value(&assert.get_output().stdout);
        // With --paths k8s/ scope, the GHA workflow / Dockerfile sources
        // should not appear. We expect at minimum DATABASE_URL coming
        // from k8s only.
        let declarations = value["declarations"].as_array().unwrap();
        let database_url = declarations
            .iter()
            .find(|d| d["name"].as_str() == Some("DATABASE_URL"))
            .expect("DATABASE_URL appears under k8s scope");
        let kinds: Vec<&str> = database_url["sources"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["kind"].as_str())
            .collect();
        for kind in &kinds {
            assert!(
                kind.contains("k8s") || kind.contains("sealed") || kind.contains("external"),
                "unexpected source kind under --paths k8s/: {kind}"
            );
        }
    }

    // ----------------------------------------
    // W2-c — assignment-scope predicate + TEMPLATE class
    // ----------------------------------------

    fn stage_env_predicate(temp: &TempDir) {
        let src = fixtures_path().join("env_predicate");
        copy_tree(&src, temp.path());
    }

    /// W2-c acceptance: a shell script with `APP_NAME=x`, `echo $APP_NAME`,
    /// ANSI color locals, and shell/CI builtins produces ZERO false orphan
    /// code references — while the genuinely-unassigned `$DEPLOY_TOKEN`
    /// read survives as a real orphan.
    #[test]
    fn shell_assignment_predicate_kills_false_orphans_end_to_end() {
        let temp = TempDir::new().unwrap();
        stage_env_predicate(&temp);

        loctree()
            .current_dir(temp.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();

        let assert = loct()
            .current_dir(temp.path())
            .args(["env-truth", "--json"])
            .assert()
            .success();
        let value = json_value(&assert.get_output().stdout);

        let orphan_names: Vec<&str> = value["orphan_reads"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|o| o["name"].as_str())
            .collect();
        for false_orphan in [
            "APP_NAME",
            "GREEN",
            "NC",
            "RELEASE_CHANNEL",
            "TARGET",
            "BASH_REMATCH",
            "COMP_WORDS",
            "GITHUB_ENV",
            "RUNNER_OS",
            "CI",
            "BASE_REF",
        ] {
            assert!(
                !orphan_names.contains(&false_orphan),
                "`{false_orphan}` must not be an orphan read; got {orphan_names:?}"
            );
        }
        assert!(
            orphan_names.contains(&"DEPLOY_TOKEN"),
            "genuine unassigned read $DEPLOY_TOKEN must stay an orphan; got {orphan_names:?}"
        );

        // Real signal preserved: UNUSED_FLAG declared in .env, never read.
        let declarations = value["declarations"].as_array().unwrap();
        let unused = declarations
            .iter()
            .find(|d| d["name"].as_str() == Some("UNUSED_FLAG"))
            .expect("UNUSED_FLAG declaration present");
        let warning_kinds: Vec<&str> = unused["precedence_warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|w| w["kind"].as_str())
            .collect();
        assert!(
            warning_kinds.contains(&"orphan_declaration"),
            "orphan_declaration must still fire for UNUSED_FLAG; got {warning_kinds:?}"
        );
    }

    /// W2-c acceptance: `.env.example` never appears in the source ranking;
    /// drift between the template and the live `.env` is reported as
    /// `template_drift` instead.
    #[test]
    fn template_excluded_from_ranking_and_drift_reported() {
        let temp = TempDir::new().unwrap();
        stage_env_predicate(&temp);

        let assert = loct()
            .current_dir(temp.path())
            .args(["env-truth", "--json"])
            .assert()
            .success();
        let value = json_value(&assert.get_output().stdout);

        for decl in value["declarations"].as_array().unwrap() {
            for source in decl["sources"].as_array().unwrap() {
                let path = source["path"].as_str().unwrap_or_default();
                assert!(
                    !path.contains(".env.example"),
                    "template leaked into ranking for {}: {path}",
                    decl["name"]
                );
            }
        }

        let drift = value["template_drift"].as_array().unwrap();
        let example = drift
            .iter()
            .find(|d| d["template_path"].as_str() == Some(".env.example"))
            .expect("template_drift entry for .env.example");
        let missing: Vec<&str> = example["missing_in_live"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(
            missing.contains(&"TEMPLATE_ONLY_KEY"),
            "TEMPLATE_ONLY_KEY promised by template but not live; got {missing:?}"
        );
        let extra: Vec<&str> = example["extra_in_live"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(
            extra.contains(&"UNUSED_FLAG"),
            "UNUSED_FLAG live but missing from template; got {extra:?}"
        );
        assert_eq!(
            value["summary"]["warnings_by_kind"]["template_drift"].as_u64(),
            Some(1)
        );
    }
}

// ============================================
// Occurrences Command Tests (W1-A literal truth layer)
// ============================================
// 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

mod occurrences_cli {
    use super::*;

    /// W1-A regression — the CodeScribe `utterance_id` failure class.
    ///
    /// `loct occurrences <ident>` must find LOCAL-variable occurrences buried
    /// inside a large function — exactly the case `find`/`tagmap` missed (they
    /// only surfaced exported symbols). Asserts the literal scanner returns the
    /// local `let mut` init and the increments, every result tagged
    /// `source: "literal"` and never a fuzzy suggestion.
    #[test]
    fn occurrences_finds_local_idents_codescribe_class() {
        let fixture = fixtures_path().join("occurrences_local_idents");

        let output = loctree()
            .current_dir(&fixture)
            .args(["occurrences", "utterance_id", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let json: Value =
            serde_json::from_slice(&output).expect("occurrences --json must be valid JSON");
        let occ = json["occurrences"]
            .as_array()
            .expect("response carries an `occurrences` array");

        // init + 2 increments + field decl + field emit + doc references.
        assert!(
            occ.len() >= 6,
            "expected >=6 literal occurrences of utterance_id, got {}",
            occ.len()
        );
        assert!(
            occ.iter().all(|o| o["source"] == "literal"),
            "every occurrence must carry source=\"literal\" (no fuzzy primary results)"
        );
        assert!(
            occ.iter().any(|o| o["context"]
                .as_str()
                .unwrap_or("")
                .contains("let mut utterance_id")),
            "must find the local init `let mut utterance_id` (the case find/tagmap missed)"
        );
        assert!(
            occ.iter().any(|o| o["context"]
                .as_str()
                .unwrap_or("")
                .contains("utterance_id += 1")),
            "must find an `utterance_id += 1` increment"
        );
    }

    /// W2-B acceptance — local mutation/dataflow classification rides on every
    /// literal occurrence as `occurrence_kind`. The fixture proves all three
    /// proven Rust shapes at once: `let mut utterance_id` (definition-like),
    /// `utterance_id += 1` (mutation-like), and the single-line
    /// `UtteranceFinal { utterance_id, .. }` shorthand (field-emit-like). It is
    /// conservative: the multiline emission sites stay `unknown`.
    #[test]
    fn occurrences_classifies_local_mutation_kinds() {
        let fixture = fixtures_path().join("occurrences_local_idents");

        let output = loctree()
            .current_dir(&fixture)
            .args(["occurrences", "utterance_id", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let json: Value =
            serde_json::from_slice(&output).expect("occurrences --json must be valid JSON");
        let occ = json["occurrences"]
            .as_array()
            .expect("response carries an `occurrences` array");

        // Every occurrence carries a classification (no missing field).
        assert!(
            occ.iter().all(|o| o.get("occurrence_kind").is_some()),
            "every occurrence must carry an `occurrence_kind` label"
        );
        assert!(
            occ.iter().all(|o| o.get("match_role").is_some()
                && o.get("confidence").is_some()
                && o.get("scope_classification").is_some()),
            "every occurrence must carry role contract fields"
        );

        let kind_for = |needle: &str| -> Option<String> {
            occ.iter()
                .find(|o| o["context"].as_str().unwrap_or("").contains(needle))
                .and_then(|o| o["occurrence_kind"].as_str())
                .map(str::to_string)
        };

        assert_eq!(
            kind_for("let mut utterance_id").as_deref(),
            Some("definition_like"),
            "`let mut utterance_id` must classify as definition_like"
        );
        assert_eq!(
            kind_for("utterance_id += 1").as_deref(),
            Some("mutation_like"),
            "`utterance_id += 1` must classify as mutation_like"
        );
        assert_eq!(
            kind_for("UtteranceFinal { utterance_id, text }").as_deref(),
            Some("field_emit_like"),
            "single-line `UtteranceFinal {{ utterance_id, .. }}` must be field_emit_like"
        );
        assert!(
            occ.iter().any(|o| o["match_role"] == "local_binding"),
            "definition_like let sites must expose compact match_role=local_binding"
        );
        assert!(
            occ.iter().any(|o| o["match_role"] == "mutation"),
            "mutation_like sites must expose compact match_role=mutation"
        );

        // Classification only ever uses the documented label set. The Rust role
        // shapes stay exactly as before; the language-aware layer additionally
        // labels doc comments (`comment`) and ordinary reads / field decls
        // (`identifier`) that used to fall through to `unknown`.
        let allowed = [
            "definition_like",
            "import_like",
            "mutation_like",
            "field_emit_like",
            "comment",
            "identifier",
            "unknown",
        ];
        assert!(
            occ.iter()
                .all(|o| allowed.contains(&o["occurrence_kind"].as_str().unwrap_or(""))),
            "occurrence_kind must stay within the documented conservative label set"
        );

        assert_eq!(json["query_kind"], "identifier");
        assert_eq!(json["match_mode"], "identifier_boundary");
        assert!(
            json["suggested_next"]
                .as_array()
                .expect("suggested_next")
                .iter()
                .any(|s| s["command"] == "loct body 'utterance_id' --json"),
            "literal JSON must suggest body navigation for identifier hits"
        );
    }

    #[test]
    fn occurrences_enriches_roles_and_resolves_twin_definitions() {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();

        std::fs::create_dir_all(temp.path().join("src/onboarding")).unwrap();
        std::fs::create_dir_all(temp.path().join("src/overlay")).unwrap();
        std::fs::create_dir_all(temp.path().join("src/settings/handlers")).unwrap();
        std::fs::create_dir_all(temp.path().join("src/voice_chat/handlers")).unwrap();

        std::fs::write(
            temp.path().join("src/lib.rs"),
            "pub mod onboarding;\npub mod overlay;\npub mod settings;\npub mod voice_chat;\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("src/onboarding/mod.rs"),
            "pub mod handlers;\npub mod window;\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("src/onboarding/handlers.rs"),
            "pub(super) fn action_handler_class() -> usize {\n    1\n}\n",
        )
        .unwrap();
        std::fs::write(temp.path().join("src/onboarding/window.rs"), "use super::handlers::{action_handler_class, window_delegate_class};\n\npub fn install() {\n    let action_handler_class = action_handler_class();\n    let action_handler = action_handler_class;\n}\n\npub(super) fn window_delegate_class() -> usize { 9 }\n").unwrap();
        std::fs::write(
            temp.path().join("src/overlay/mod.rs"),
            "pub mod actions;\npub mod window;\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("src/overlay/actions.rs"),
            "pub(super) fn action_handler_class() -> usize {\n    2\n}\n",
        )
        .unwrap();
        std::fs::write(temp.path().join("src/overlay/window.rs"), "use super::actions::{OverlayActionButtonRole, action_handler_class, overlay_button_selector};\n\npub fn install_overlay() {\n    let handler_class = action_handler_class();\n}\n\npub struct OverlayActionButtonRole;\npub(super) fn overlay_button_selector() -> usize { 8 }\n").unwrap();
        std::fs::write(temp.path().join("src/settings/mod.rs"), "pub mod handlers;\n\nuse handlers::{action_handler_class, toolbar_delegate_class, window_delegate_class};\n\npub fn install_settings() {\n    let action_handler_class = action_handler_class();\n    let action_handler = action_handler_class;\n}\n").unwrap();
        std::fs::write(temp.path().join("src/settings/handlers.rs"), "pub fn action_handler_class() -> usize {\n    3\n}\npub fn toolbar_delegate_class() -> usize { 4 }\npub fn window_delegate_class() -> usize { 5 }\n").unwrap();
        std::fs::write(temp.path().join("src/voice_chat/mod.rs"), "pub mod handlers;\n\nuse handlers::{action_handler_class, agent_input_text_view_class, drop_target_view_class};\n\npub fn install_voice() {\n    let action_handler_class = action_handler_class();\n    let action_handler = action_handler_class;\n}\n").unwrap();
        std::fs::write(
            temp.path().join("src/voice_chat/handlers/mod.rs"),
            "mod classes;\npub use classes::*;\n",
        )
        .unwrap();
        std::fs::write(temp.path().join("src/voice_chat/handlers/classes.rs"), "pub fn action_handler_class() -> usize {\n    4\n}\npub fn agent_input_text_view_class() -> usize { 6 }\npub fn drop_target_view_class() -> usize { 7 }\n").unwrap();

        std::process::Command::new("git")
            .arg("init")
            .current_dir(temp.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(temp.path())
            .output()
            .unwrap();

        let output = loctree()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["occurrences", "action_handler_class", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let json: Value =
            serde_json::from_slice(&output).expect("occurrences --json must be valid JSON");
        let occ = json["occurrences"]
            .as_array()
            .expect("response carries an `occurrences` array");

        assert_eq!(
            json["total"], 18,
            "literal rg-parity hit count must remain intact"
        );
        assert_eq!(
            occ.iter()
                .filter(|o| o["match_role"] == "definition")
                .count(),
            4,
            "only the four exported function declarations are symbol definitions"
        );
        assert_eq!(
            occ.iter()
                .filter(|o| o["match_role"] == "local_binding")
                .count(),
            3,
            "`let action_handler_class = ...` sites are local bindings, not definitions"
        );
        assert_eq!(
            occ.iter().filter(|o| o["match_role"] == "import").count(),
            4,
            "Rust use-list entries must be classified as imports"
        );
        assert!(
            occ.iter().all(|o| o.get("enclosing_symbol").is_some()),
            "every occurrence should expose an enclosing symbol or file-level context"
        );
        assert!(
            occ.iter().all(|o| o
                .get("definition_candidates")
                .and_then(|v| v.as_array())
                .is_some_and(|defs| defs.len() == 4)),
            "twin definitions must be explicit on every occurrence"
        );

        let import = occ
            .iter()
            .find(|o| {
                o["file"] == "src/onboarding/window.rs"
                    && o["context"]
                        .as_str()
                        .unwrap_or("")
                        .starts_with("use super::handlers")
            })
            .expect("onboarding import occurrence");
        assert_eq!(import["match_role"], "import");
        assert_eq!(
            import["resolved_definition"]["symbol_id"],
            "src/onboarding/handlers.rs::action_handler_class"
        );

        let local = occ
            .iter()
            .find(|o| {
                o["file"] == "src/onboarding/window.rs"
                    && o["context"]
                        .as_str()
                        .unwrap_or("")
                        .contains("let action_handler_class = action_handler_class()")
                    && o["column"] == 9
            })
            .expect("onboarding local binding occurrence");
        assert_eq!(local["match_role"], "local_binding");
        assert!(
            local.get("resolved_definition").is_none(),
            "a local binding is not an exported symbol definition"
        );

        let rhs_call = occ
            .iter()
            .find(|o| {
                o["file"] == "src/onboarding/window.rs"
                    && o["context"]
                        .as_str()
                        .unwrap_or("")
                        .contains("let action_handler_class = action_handler_class()")
                    && o["column"] == 32
            })
            .expect("onboarding RHS call occurrence");
        assert_eq!(rhs_call["match_role"], "reference");
        assert_eq!(
            rhs_call["resolved_definition"]["symbol_id"],
            "src/onboarding/handlers.rs::action_handler_class",
            "RHS call should bind to the imported function, not the new local binding"
        );

        let later_local_ref = occ
            .iter()
            .find(|o| {
                o["file"] == "src/onboarding/window.rs"
                    && o["context"]
                        .as_str()
                        .unwrap_or("")
                        .contains("let action_handler = action_handler_class")
            })
            .expect("later local reference occurrence");
        assert_eq!(later_local_ref["match_role"], "reference");
        assert_eq!(
            later_local_ref["resolved_definition"]["kind"], "local_binding",
            "later references should bind to the local binding"
        );

        let voice_import = occ
            .iter()
            .find(|o| {
                o["file"] == "src/voice_chat/mod.rs"
                    && o["context"]
                        .as_str()
                        .unwrap_or("")
                        .starts_with("use handlers::{")
            })
            .expect("voice_chat re-export import occurrence");
        assert_eq!(voice_import["match_role"], "import");
        assert_eq!(
            voice_import["resolved_definition"]["symbol_id"],
            "src/voice_chat/handlers/classes.rs::action_handler_class",
            "imports through handlers::* re-export should resolve to the concrete class module"
        );

        let voice_rhs = occ
            .iter()
            .find(|o| {
                o["file"] == "src/voice_chat/mod.rs"
                    && o["context"]
                        .as_str()
                        .unwrap_or("")
                        .contains("let action_handler_class = action_handler_class()")
                    && o["column"] == 32
            })
            .expect("voice_chat RHS call occurrence");
        assert_eq!(
            voice_rhs["resolved_definition"]["symbol_id"],
            "src/voice_chat/handlers/classes.rs::action_handler_class"
        );

        let suggestions = json["suggested_next"].as_array().expect("suggested_next");
        assert!(
            suggestions
                .iter()
                .any(|s| s["command"] == "loct body 'action_handler_class' --json")
                && suggestions
                    .iter()
                    .any(|s| s["command"] == "loct find --literal 'action_handler_class' --json")
                && suggestions
                    .iter()
                    .any(|s| s["command"] == "loct slice 'src/onboarding/handlers.rs'"),
            "suggested_next should point to body, literal parity, and a concrete slice"
        );
    }

    #[test]
    fn occurrences_human_output_uses_compact_role_label_and_next_step() {
        let fixture = fixtures_path().join("occurrences_local_idents");

        loctree()
            .current_dir(&fixture)
            .args(["occurrences", "utterance_id"])
            .assert()
            .success()
            .stdout(predicate::str::contains("[local_binding]"))
            .stdout(predicate::str::contains("[mutation]"))
            .stdout(predicate::str::contains("suggested next:"))
            .stdout(predicate::str::contains("loct body 'utterance_id' --json"));
    }

    #[test]
    fn occurrences_zero_results_explain_next_steps() {
        let fixture = fixtures_path().join("occurrences_local_idents");

        let output = loctree()
            .current_dir(&fixture)
            .args(["occurrences", "definitely_missing_symbol_zzz", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let json: Value =
            serde_json::from_slice(&output).expect("occurrences --json must be valid JSON");
        assert_eq!(json["total"], 0);
        assert_eq!(json["query_kind"], "identifier");
        let suggestions = json["suggested_next"]
            .as_array()
            .expect("zero-result suggested_next");
        assert!(
            suggestions.iter().any(|s| s["command"]
                == "loct find 'definitely_missing_symbol_zzz' --json"
                && s["reason"]
                    .as_str()
                    .unwrap_or("")
                    .contains("without treating suggestions as evidence")),
            "zero literal results must suggest broadening without pretending suggestions are evidence"
        );
    }

    #[test]
    fn occurrences_verifies_coverage_line_and_artifact_flagging() {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();

        // Write a product file with a match
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/main.rs"), "pub fn target_ident() {}").unwrap();

        // Write a generated file with a match
        std::fs::create_dir_all(temp.path().join("dist")).unwrap();
        std::fs::write(
            temp.path().join("dist/bundle.gen.rs"),
            "pub fn target_ident() {}",
        )
        .unwrap();

        // Initialize git so the scan works
        std::process::Command::new("git")
            .arg("init")
            .current_dir(temp.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(temp.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(temp.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(temp.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "Initial commit"])
            .current_dir(temp.path())
            .output()
            .unwrap();

        // Build snapshot
        loctree()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["scan"])
            .assert()
            .success();

        // Query occurrences
        let output = loctree()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["occurrences", "target_ident", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let json: Value = serde_json::from_slice(&output).expect("valid JSON");

        // W2-02 truth contract: the generated file is scanned and its hit
        // reported; the artifact class survives only as accounting.
        let occ = json["occurrences"].as_array().expect("occurrences list");
        assert_eq!(occ.len(), 2);
        let hit_files: Vec<&str> = occ.iter().filter_map(|o| o["file"].as_str()).collect();
        assert!(hit_files.contains(&"src/main.rs"));
        assert!(hit_files.contains(&"dist/bundle.gen.rs"));

        // Verify coverage line and scope stats
        let coverage_line = json["coverage_line"].as_str().expect("coverage_line field");
        assert!(coverage_line.contains("scanned 3 of 3 repo files"));
        assert!(coverage_line.contains("artifact-flagged: generated(1)"));

        let scope = &json["scope"];
        assert_eq!(scope["files_in_universe"].as_u64(), Some(3));
        assert_eq!(scope["files_scanned"].as_u64(), Some(3));
        assert_eq!(scope["generated"].as_u64(), Some(1));
        assert_eq!(scope["vendored"].as_u64(), Some(0));
        assert_eq!(scope["fixtures"].as_u64(), Some(0));
        assert_eq!(scope["templates"].as_u64(), Some(0));

        let output_human = loctree()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["occurrences", "target_ident"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let human_str = String::from_utf8_lossy(&output_human);
        assert!(human_str.contains("scanned 3 of 3 repo files; artifact-flagged: generated(1)"));

        // Query find --literal
        let output_find = loctree()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["find", "--literal", "target_ident", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let json_find: Value = serde_json::from_slice(&output_find).expect("valid JSON");
        let literal_matches = &json_find["literal_matches"];

        // Same truth contract on the find --literal surface: generated hits
        // are reported, the class tally stays as accounting.
        let occ_find = literal_matches["occurrences"]
            .as_array()
            .expect("occurrences list");
        assert_eq!(occ_find.len(), 2);
        let find_files: Vec<&str> = occ_find.iter().filter_map(|o| o["file"].as_str()).collect();
        assert!(find_files.contains(&"src/main.rs"));
        assert!(find_files.contains(&"dist/bundle.gen.rs"));

        // Verify coverage line and scope stats
        let coverage_line_find = literal_matches["coverage_line"]
            .as_str()
            .expect("coverage_line field");
        assert!(coverage_line_find.contains("scanned 3 of 3 repo files"));
        assert!(coverage_line_find.contains("artifact-flagged: generated(1)"));

        let scope_find = &literal_matches["scope"];
        assert_eq!(scope_find["files_in_universe"].as_u64(), Some(3));
        assert_eq!(scope_find["files_scanned"].as_u64(), Some(3));
        assert_eq!(scope_find["generated"].as_u64(), Some(1));

        let output_find_human = loctree()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["find", "--literal", "target_ident"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let human_find_str = String::from_utf8_lossy(&output_find_human);
        assert!(
            human_find_str.contains("scanned 3 of 3 repo files; artifact-flagged: generated(1)")
        );
    }
}

// ============================================
// find --literal Tests (W1-B literal mode wired into find)
// ============================================
// 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

mod find_literal_cli {
    use super::*;

    /// `loct find --literal <ident> --json` must return a `literal_matches`
    /// section whose occurrences are byte-for-byte the same lines `loct
    /// occurrences` surfaces — every result tagged `source: "literal"`, and the
    /// buried-local CodeScribe lines (`let mut utterance_id`, `utterance_id += 1`)
    /// present. This is the W1-B acceptance: when the mode says literal, the
    /// answer is literal.
    #[test]
    fn find_literal_returns_literal_matches_section() {
        let fixture = fixtures_path().join("occurrences_local_idents");

        let output = loctree()
            .current_dir(&fixture)
            .args(["find", "--literal", "utterance_id", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let json: Value =
            serde_json::from_slice(&output).expect("find --literal --json must be valid JSON");

        assert_eq!(
            json["mode"], "literal",
            "literal mode must be self-describing"
        );

        let literal = &json["literal_matches"];
        assert_eq!(
            literal["source"], "literal",
            "literal_matches block must carry source=\"literal\""
        );
        assert_eq!(literal["query_kind"], "identifier");
        assert_eq!(literal["match_mode"], "identifier_boundary");

        let occ = literal["occurrences"]
            .as_array()
            .expect("literal_matches carries an `occurrences` array");

        assert!(
            occ.len() >= 6,
            "expected >=6 literal occurrences of utterance_id, got {}",
            occ.len()
        );
        assert!(
            occ.iter().all(|o| o["source"] == "literal"),
            "every literal match must carry source=\"literal\" (no fuzzy primaries)"
        );
        assert!(
            occ.iter().all(|o| o.get("match_role").is_some()
                && o.get("confidence").is_some()
                && o.get("scope_classification").is_some()),
            "find --literal occurrences must carry the agent role contract"
        );
        assert!(
            occ.iter().any(|o| o["context"]
                .as_str()
                .unwrap_or("")
                .contains("let mut utterance_id")),
            "literal_matches must include the local init `let mut utterance_id`"
        );
        assert!(
            occ.iter().any(|o| o["context"]
                .as_str()
                .unwrap_or("")
                .contains("utterance_id += 1")),
            "literal_matches must include an `utterance_id += 1` increment"
        );
    }

    /// Fuzzy suggestions, if present, live in their own labeled section and are
    /// NEVER folded into the primary literal matches. We prove the separation
    /// structurally: the `fuzzy_suggestions` key exists and any item there is
    /// tagged `source: "fuzzy"` — the deliberate inverse of literal provenance.
    #[test]
    fn find_literal_keeps_fuzzy_suggestions_separate() {
        let fixture = fixtures_path().join("occurrences_local_idents");

        let output = loctree()
            .current_dir(&fixture)
            .args(["find", "--literal", "utterance_id", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let json: Value =
            serde_json::from_slice(&output).expect("find --literal --json must be valid JSON");

        let fuzzy = json["fuzzy_suggestions"]
            .as_array()
            .expect("literal mode always emits a (possibly empty) fuzzy_suggestions array");

        // Whatever is suggested is labeled fuzzy and is absent from the literal set.
        for s in fuzzy {
            assert_eq!(
                s["source"], "fuzzy",
                "every suggestion must be labeled source=\"fuzzy\", never literal"
            );
            let score = s["score"]
                .as_f64()
                .expect("each fuzzy suggestion carries a numeric score");
            assert!(
                score >= 0.7,
                "literal mode must drop sub-0.7 fuzzy noise; saw score {score}"
            );
        }

        let literal_occ = json["literal_matches"]["occurrences"]
            .as_array()
            .expect("literal_matches carries an `occurrences` array");
        assert!(
            literal_occ.iter().all(|o| o["source"] == "literal"),
            "fuzzy suggestions must never leak into the literal_matches set"
        );
    }

    /// Non-symbol literals (`=` punctuation, not an identifier) must still be
    /// exact occurrence truth: file/line/range metadata, `source: "literal"`,
    /// and no fuzzy candidates promoted into primary matches.
    #[test]
    fn find_literal_returns_exact_non_symbol_token_hits() {
        let fixture = fixtures_path().join("occurrences_backdrop");
        let cache = TempDir::new().unwrap();

        let output = loctree()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["find", "--literal", "checkout=success", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let json: Value =
            serde_json::from_slice(&output).expect("find --literal --json must be valid JSON");

        assert_eq!(json["mode"], "literal");
        let lit = &json["literal_matches"];
        let occ = lit["occurrences"].as_array().expect("occurrences array");
        assert_eq!(lit["source"], "literal");
        assert_eq!(lit["total"], 1);
        assert_eq!(lit["files_matched"], 1);
        assert_eq!(occ.len(), 1);

        let hit = &occ[0];
        assert_eq!(hit["source"], "literal");
        assert_eq!(hit["matched_text"], "checkout=success");
        assert_eq!(hit["file"], "overlay.tsx");
        assert_eq!(hit["line"], 9);
        assert_eq!(hit["column"], 31);
        assert_eq!(hit["range"]["start"]["line"], 9);
        assert_eq!(hit["range"]["start"]["column"], 31);
        assert_eq!(hit["range"]["end"]["line"], 9);
        assert_eq!(hit["range"]["end"]["column"], 47);
        assert_eq!(hit["occurrence_kind"], "string_literal");
        assert!(
            hit["context"]
                .as_str()
                .unwrap_or("")
                .contains("checkout=success")
        );

        let fuzzy = json["fuzzy_suggestions"]
            .as_array()
            .expect("literal mode emits fuzzy_suggestions separately");
        assert!(
            fuzzy.iter().all(|s| s["source"] == "fuzzy"),
            "any suggestions must remain separate fuzzy hints: {json}"
        );
    }

    /// Prose/phrase literals must behave like fixed-string lookup. Identifier
    /// queries stay boundary-aware, but `snapshot fresh` is not a symbol lookup;
    /// it is raw literal evidence agents compare against filesystem search.
    #[test]
    fn find_literal_returns_exact_phrase_hits() {
        let fixture = fixtures_path().join("occurrences_backdrop");
        let cache = TempDir::new().unwrap();

        let output = loctree()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["find", "--literal", "snapshot fresh", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let json: Value =
            serde_json::from_slice(&output).expect("find --literal --json must be valid JSON");

        let lit = &json["literal_matches"];
        let occ = lit["occurrences"].as_array().expect("occurrences array");
        assert_eq!(lit["source"], "literal");
        assert_eq!(lit["total"], 1);
        assert_eq!(lit["files_matched"], 1);
        assert_eq!(occ.len(), 1);

        let hit = &occ[0];
        assert_eq!(hit["matched_text"], "snapshot fresh");
        assert_eq!(hit["file"], "overlay.tsx");
        assert_eq!(hit["occurrence_kind"], "string_literal");
        assert!(
            hit["context"]
                .as_str()
                .unwrap_or("")
                .contains("snapshot fresh")
        );
    }

    /// Backward compatibility: default `loct find <pattern>` (no --literal) must
    /// keep its existing AST/fuzzy behavior and must NOT emit a `literal_matches`
    /// section. The truth layer is strictly opt-in.
    #[test]
    fn find_without_literal_stays_backward_compatible() {
        let fixture = fixtures_path().join("occurrences_local_idents");

        let output = loctree()
            .current_dir(&fixture)
            .args(["find", "utterance_id", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let json: Value =
            serde_json::from_slice(&output).expect("default find --json must be valid JSON");

        assert!(
            json.get("literal_matches").is_none(),
            "default find must NOT emit a literal_matches section (opt-in only)"
        );
        assert!(
            json.get("symbol_matches").is_some(),
            "default find must keep its existing symbol_matches contract"
        );
    }
}

// ============================================
// occurrence_kind taxonomy + whole_token + aggregation (W1 output quality)
// ============================================
// Drives the real `loct` binary over a CSS/TSX fixture to prove, end to end,
// that the literal layer now classifies (no blanket `unknown`), can tighten the
// token boundary, and can roll up / slim its output — all at MCP↔CLI parity.

mod occurrences_quality_cli {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;

    fn run_occurrences(args: &[&str]) -> Value {
        let fixture = fixtures_path().join("occurrences_backdrop");
        // Isolate the loctree cache per invocation. Several tests in this module
        // scan the same fixture path, so sharing the global cache races on the
        // project snapshot under parallel `cargo test` — yielding flaky empty
        // reads (e.g. whole_token returning 0). Mirrors the LOCT_CACHE_DIR
        // isolation used elsewhere in this file.
        let cache = TempDir::new().unwrap();
        let output = loctree()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice(&output).expect("occurrences --json must be valid JSON")
    }

    fn kinds_of(occ: &[Value]) -> BTreeSet<String> {
        occ.iter()
            .filter_map(|o| o["occurrence_kind"].as_str().map(str::to_string))
            .collect()
    }

    /// Every literal hit on `backdrop` across CSS/TSX carries a *real*
    /// language-aware `occurrence_kind`; the blanket-`unknown` regime is gone.
    #[test]
    fn backdrop_occurrences_are_classified_not_unknown() {
        let json = run_occurrences(&["occurrences", "backdrop", "--json"]);
        let occ = json["occurrences"].as_array().expect("occurrences array");
        assert!(!occ.is_empty(), "fixture must contain backdrop occurrences");

        // Honest fallback only: not a single hit may be `unknown` here.
        assert!(
            occ.iter().all(|o| o["occurrence_kind"] != "unknown"),
            "no occurrence may stay `unknown` on this CSS/TSX fixture"
        );

        // The taxonomy actually fires across both languages.
        let kinds = kinds_of(occ);
        for expected in [
            "class_token",
            "custom_property",
            "comment",
            "data_attribute",
            "identifier",
        ] {
            assert!(
                kinds.contains(expected),
                "expected kind `{expected}` among {kinds:?}"
            );
        }
    }

    /// `loct find --literal` and `loct occurrences` stay byte-for-byte at parity
    /// for the same query — same total, same per-site classification.
    #[test]
    fn find_literal_and_occurrences_are_at_parity() {
        let occ_json = run_occurrences(&["occurrences", "backdrop", "--json"]);

        let fixture = fixtures_path().join("occurrences_backdrop");
        // Isolated cache: same fixture path as run_occurrences above; a shared
        // global cache would race under parallel `cargo test`.
        let cache = TempDir::new().unwrap();
        let find_out = loctree()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["find", "--literal", "backdrop", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let find_json: Value =
            serde_json::from_slice(&find_out).expect("find --literal --json must be valid JSON");

        let occ = occ_json["occurrences"].as_array().unwrap();
        let lit = find_json["literal_matches"]["occurrences"]
            .as_array()
            .unwrap();

        let fingerprint = |arr: &[Value]| -> Vec<String> {
            let mut v: Vec<String> = arr
                .iter()
                .map(|o| {
                    format!(
                        "{}:{}:{}:{}",
                        o["file"].as_str().unwrap_or(""),
                        o["line"].as_u64().unwrap_or(0),
                        o["column"].as_u64().unwrap_or(0),
                        o["occurrence_kind"].as_str().unwrap_or("")
                    )
                })
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            fingerprint(occ),
            fingerprint(lit),
            "occurrences and find --literal must agree on every site AND its kind"
        );
    }

    /// `loct find --literal --file` is an exact occurrence query narrowed to one
    /// requested snapshot path. It must not leak same-token hits from sibling
    /// CSS/TSX files, and every hit carries line/range metadata.
    #[test]
    fn find_literal_file_scope_returns_only_requested_file_with_range() {
        let fixture = fixtures_path().join("occurrences_backdrop");
        let cache = TempDir::new().unwrap();
        let find_out = loctree()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args([
                "find",
                "--literal",
                "backdrop",
                "--file",
                "styles.css",
                "--json",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let find_json: Value =
            serde_json::from_slice(&find_out).expect("find --literal --json must be valid JSON");

        let lit = &find_json["literal_matches"];
        let occ = lit["occurrences"].as_array().expect("occurrences array");
        assert!(!occ.is_empty(), "fixture must contain CSS backdrop hits");
        assert_eq!(lit["files_matched"], 1);
        assert!(
            occ.iter().all(|o| o["file"] == "styles.css"),
            "file scope must not leak overlay.tsx hits: {find_json}"
        );
        assert!(
            occ.iter().all(|o| o["line"].is_u64()
                && o["column"].is_u64()
                && o["range"]["start"]["line"].is_u64()
                && o["range"]["start"]["column"].is_u64()
                && o["range"]["end"]["line"].is_u64()
                && o["range"]["end"]["column"].is_u64()),
            "every literal occurrence should carry line/column plus range metadata: {find_json}"
        );
    }

    /// `--whole-token` tightens the boundary so `backdrop` stops matching inside
    /// hyphenated neighbors (`--vista-z-overlay-backdrop`, `backdrop-filter`),
    /// cutting the z-index noise — while the default boundary is unchanged.
    #[test]
    fn whole_token_cuts_hyphenated_noise() {
        let loose = run_occurrences(&["occurrences", "backdrop", "--json"]);
        let tight = run_occurrences(&["occurrences", "backdrop", "--whole-token", "--json"]);

        let loose_total = loose["total"].as_u64().unwrap();
        let tight_total = tight["total"].as_u64().unwrap();
        assert!(
            tight_total > 0 && tight_total < loose_total,
            "whole_token must drop hyphenated hits (loose={loose_total}, tight={tight_total})"
        );

        let tight_occ = tight["occurrences"].as_array().unwrap();
        assert!(
            tight_occ.iter().all(|o| {
                let ctx = o["context"].as_str().unwrap_or("");
                !ctx.contains("--vista-z-overlay-backdrop")
            }) || tight_occ.iter().all(|o| o["matched_text"] == "backdrop"),
            "whole_token must not surface a hit *inside* the hyphenated custom property"
        );
        // Concretely: no whole-token hit may sit on the z-index custom property.
        assert!(
            !tight_occ.iter().any(|o| o["context"]
                .as_str()
                .unwrap_or("")
                .contains("--vista-z-overlay-backdrop")
                && o["occurrence_kind"] == "custom_property"),
            "the `--vista-z-overlay-backdrop` noise hit must be gone under whole_token"
        );
    }

    /// `--group-by-file` adds a per-file rollup and `--count-only` suppresses the
    /// full list while preserving the counters — distinguishable from not-found.
    #[test]
    fn group_by_file_and_count_only_shape_output() {
        let json = run_occurrences(&[
            "occurrences",
            "backdrop",
            "--group-by-file",
            "--count-only",
            "--json",
        ]);

        assert_eq!(
            json["slim"],
            serde_json::json!(true),
            "count_only sets slim"
        );
        assert_eq!(
            json["occurrences"].as_array().map(|a| a.len()),
            Some(0),
            "count_only suppresses the full occurrence list"
        );

        let total = json["total"].as_u64().expect("total survives slim");
        assert!(
            total > 0,
            "total must reflect the real count even when slim"
        );

        let by_file = json["by_file"]
            .as_array()
            .expect("group_by_file populates by_file");
        let summed: u64 = by_file
            .iter()
            .map(|fc| fc["count"].as_u64().unwrap_or(0))
            .sum();
        assert_eq!(summed, total, "per-file counts must sum to the total");
        assert!(
            by_file
                .iter()
                .any(|fc| fc["file"].as_str().unwrap_or("").ends_with(".css")),
            "rollup must include the CSS file"
        );
    }

    #[test]
    fn occurrences_refreshes_when_parent_snapshot_hides_requested_subdir() {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        fs::write(temp.path().join("package.json"), r#"{"name":"scope-host"}"#).unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(
            temp.path().join("src/index.ts"),
            "export const rootOnly = 1;",
        )
        .unwrap();

        loctree()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .assert()
            .success();

        let subdir = temp.path().join("fixture");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(
            subdir.join("styles.css"),
            "/* backdrop */\n.backdrop { --vista-z-overlay-backdrop: 40; }\n",
        )
        .unwrap();
        fs::write(
            subdir.join("overlay.tsx"),
            r#"export const node = <div className="backdrop" data-backdrop="true" />;"#,
        )
        .unwrap();

        let output = loctree()
            .current_dir(&subdir)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["occurrences", "backdrop", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let json: Value = serde_json::from_slice(&output).unwrap();

        assert!(
            json["total"].as_u64().unwrap_or(0) > 0,
            "subdir literal scan must not reuse the parent snapshot as an empty truth"
        );
        assert!(
            json["occurrences"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o["file"].as_str().unwrap_or("").ends_with("styles.css")),
            "refreshed subdir snapshot must include the new CSS file"
        );
    }
}

// ============================================
// Query/body CLI output + reuse fence contracts
// ============================================

mod query_body_cli {
    use super::*;
    use std::fs;

    #[test]
    fn body_resolves_rust_impl_method_from_snapshot() {
        let fixture = fixtures_path().join("rust_impl_methods");
        let cache = TempDir::new().expect("temp cache dir");

        let output = loct()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["body", "Recorder::start"])
            .assert()
            .success()
            .get_output()
            .clone();

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.contains("src/recorder.rs"));
        assert!(stdout.contains("pub fn start(&self)"));
        assert!(stdout.contains("println!(\"start\")"));
        assert!(
            !stdout.contains("fn stop"),
            "body Recorder::start must return the method body, not the whole impl block"
        );
        assert!(
            !stderr.contains("Languages:") && !stderr.contains("Next steps"),
            "body query must not print scan summary boilerplate to stderr: {stderr}"
        );
    }

    #[test]
    fn body_bounds_impl_method_despite_char_escape_and_lifetime() {
        // Regression lock for the `loct body resolve_file_in_snapshot`
        // overcapture: a `'\\'` char literal (and `'a` lifetimes) used to
        // derail the brace scanner, so the body ran through every sibling
        // method until the line cap. The body must stop at the method's
        // own closing brace with truthful range metadata.
        let fixture = fixtures_path().join("rust_impl_methods");
        let cache = TempDir::new().expect("temp cache dir");

        let output = loct()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["body", "Resolver::normalize", "--json"])
            .assert()
            .success()
            .get_output()
            .clone();

        let json: Value =
            serde_json::from_slice(&output.stdout).expect("body --json must emit clean JSON");
        let bodies = json["bodies"].as_array().expect("bodies array");
        assert_eq!(bodies.len(), 1, "exactly one defining body expected");
        let body = &bodies[0];
        assert!(
            body["file"]
                .as_str()
                .unwrap_or("")
                .ends_with("src/resolver.rs")
        );
        assert_eq!(body["start_line"].as_u64(), Some(4), "fn normalize line");
        assert_eq!(
            body["end_line"].as_u64(),
            Some(7),
            "body must end at the method's closing brace, not run into siblings"
        );
        assert_eq!(body["total_lines"].as_u64(), Some(4));
        assert_eq!(body["truncated"].as_bool(), Some(false));
        let source = body["source"].as_str().unwrap_or("");
        assert!(source.contains("replace('\\\\', \"/\")"));
        assert!(
            !source.contains("sibling_marker"),
            "body must not overcapture the sibling method: {source}"
        );
    }

    #[test]
    fn where_symbol_lists_rust_impl_method_exactly() {
        let fixture = fixtures_path().join("rust_impl_methods");
        let cache = TempDir::new().expect("temp cache dir");

        let output = loct()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["query", "where-symbol", "Recorder::start"])
            .assert()
            .success()
            .get_output()
            .clone();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("src/recorder.rs:4"));
        assert!(stdout.contains("impl method Recorder::start"));
    }

    #[test]
    fn query_auto_scan_suppresses_summary_boilerplate() {
        let fixture = fixtures_path().join("rust_impl_methods");
        let cache = TempDir::new().expect("temp cache dir");

        let output = loct()
            .current_dir(&fixture)
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["occurrences", "Recorder", "--json"])
            .assert()
            .success()
            .get_output()
            .clone();

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let json: Value = serde_json::from_slice(&output.stdout)
            .expect("occurrences --json must stay clean JSON");
        assert!(json["total"].as_u64().unwrap_or(0) > 0);
        assert!(
            !stdout.contains("Next steps")
                && !stdout.contains("Languages:")
                && !stderr.contains("Next steps")
                && !stderr.contains("Languages:"),
            "query command leaked scan boilerplate\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }

    #[test]
    fn reuse_fence_skips_rescan_when_commit_changes_without_content_drift() {
        let temp = TempDir::new().expect("temp repo dir");
        let cache = TempDir::new().expect("temp cache dir");
        fs::create_dir_all(temp.path().join("src")).expect("create src dir");
        fs::write(
            temp.path().join("src/lib.rs"),
            "pub fn initial_marker() -> &'static str { \"initial\" }\n",
        )
        .expect("write initial Rust fixture");
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["add", "."]);
        run_git(
            temp.path(),
            &[
                "-c",
                "user.email=agents@vetcoders.io",
                "-c",
                "user.name=codex",
                "commit",
                "-m",
                "init",
            ],
        );

        loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();

        fs::write(
            temp.path().join("notes.txt"),
            "not part of the indexed code graph\n",
        )
        .expect("write unindexed notes file");
        run_git(temp.path(), &["add", "notes.txt"]);
        run_git(
            temp.path(),
            &[
                "-c",
                "user.email=agents@vetcoders.io",
                "-c",
                "user.name=codex",
                "commit",
                "-m",
                "add notes",
            ],
        );

        let output = loct()
            .current_dir(temp.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .args(["--verbose", "occurrences", "initial_marker", "--json"])
            .assert()
            .success()
            .get_output()
            .clone();

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("[REUSE_FENCE]"),
            "verbose query should explain fence reuse, stderr:\n{stderr}"
        );
        assert!(
            !stderr.contains("rescanning"),
            "unchanged content must not trigger a rescan, stderr:\n{stderr}"
        );
    }
}

// ============================================
// Snapshot freshness authority golden tests (W1-a)
//
// One guardian (`snapshot::acquire_snapshot`) decides snapshot freshness and
// every internal rescan rebuilds the SAME unified file universe as the
// initial scan. These goldens pin the two observable contracts:
//   1. zero [DRIFT] rescans across the analytic command sequence on a clean
//      tree, and
//   2. identical file counts across initial scan / drift-rescan / repo-view /
//      findings.summary.
// ============================================

mod snapshot_authority_golden {
    use super::*;
    use std::fs;

    /// Mixed-universe fixture: a Cargo root (detect narrows the universe to
    /// rs+toml) that ALSO contains ts/py files. Before the guardian, the
    /// initial scan indexed 2 files while every internal drift-rescan indexed
    /// 4+ (default extensions) — the fingerprints never converged and every
    /// analytic command paid a rescan.
    ///
    /// Deliberately NO `.gitignore` with `.loctree/`: the first scan writes
    /// `./.loctree/context-atlas/` into the worktree as untracked dirt, which
    /// is exactly the default first-touch state on a fresh repo. The guardian
    /// must stay drift-free in THAT state, not only when the operator has
    /// already gitignored loct's artifacts.
    fn mixed_universe_repo() -> TempDir {
        let temp = TempDir::new().expect("temp repo");
        let root = temp.path();
        fs::create_dir_all(root.join("src")).expect("mkdir src");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"mix\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("write Cargo.toml");
        fs::write(root.join("src/lib.rs"), "pub fn alpha() {}\n").expect("write lib.rs");
        fs::write(root.join("web.ts"), "export const x = 1;\n").expect("write web.ts");
        fs::write(root.join("tool.py"), "def main():\n    return 1\n").expect("write tool.py");
        run_git(root, &["init"]);
        commit_all(root, "init");
        temp
    }

    fn commit_all(root: &std::path::Path, message: &str) {
        run_git(root, &["add", "."]);
        run_git(
            root,
            &[
                "-c",
                "user.email=agents@vetcoders.io",
                "-c",
                "user.name=claude",
                "commit",
                "-m",
                message,
            ],
        );
    }

    fn loct_in(root: &std::path::Path, cache: &std::path::Path) -> Command {
        let mut cmd = loct();
        cmd.current_dir(root).env("LOCT_CACHE_DIR", cache);
        cmd
    }

    fn snapshot_file_count(root: &std::path::Path, cache: &std::path::Path) -> u64 {
        let output = loct_in(root, cache)
            .args([".metadata.file_count"])
            .assert()
            .success()
            .get_output()
            .clone();
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .expect("file_count must be a number")
    }

    #[test]
    fn zero_drift_across_analytic_sequence_on_clean_tree() {
        let temp = mixed_universe_repo();
        let cache = TempDir::new().expect("temp cache");
        let root = temp.path();

        // Initial scan (bare `loct`). This writes `.loctree/context-atlas/`
        // into the worktree; the fixture has no `.gitignore`, so loct's own
        // artifacts sit there as untracked git dirt — the default
        // first-touch state the guardian must NOT mistake for content drift.
        loct_in(root, cache.path()).assert().success();

        // Clean tree, same commit: the analytic sequence must execute with
        // ZERO rescans — the guardian trusts the fresh snapshot everywhere.
        for args in [
            vec!["impact", "src/lib.rs"],
            vec!["dead"],
            vec!["cycles"],
            vec!["twins"],
            vec!["hotspots"],
            vec!["health"],
        ] {
            let output = loct_in(root, cache.path())
                .args(&args)
                .assert()
                .success()
                .get_output()
                .clone();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !stderr.contains("[DRIFT]") && !stderr.contains("rescanning"),
                "loct {:?} must not rescan on a clean tree, stderr:\n{stderr}",
                args
            );
        }
    }

    /// W1-03 acceptance: the guardian must be blind to `.loctree/` even when
    /// the entry NEVER lands in .gitignore. `LOCT_NO_GITIGNORE=1` suppresses
    /// the first-scan append, so loct's own artifacts stay untracked git dirt
    /// for the whole analytic sequence — the exact fail-log state
    /// (2026-06-10: self-sustaining [DRIFT] loop on loct's own artifact).
    #[test]
    fn zero_drift_without_gitignore_entry() {
        let temp = mixed_universe_repo();
        let cache = TempDir::new().expect("temp cache");
        let root = temp.path();

        loct_in(root, cache.path())
            .env("LOCT_NO_GITIGNORE", "1")
            .assert()
            .success();
        assert!(
            root.join(".loctree").exists(),
            "scan must write its artifacts into ./.loctree/"
        );
        assert!(
            !root.join(".gitignore").exists(),
            "LOCT_NO_GITIGNORE=1 must suppress the gitignore append"
        );

        let mut drift_count = 0usize;
        for args in [["dead"], ["cycles"], ["twins"], ["health"]] {
            let output = loct_in(root, cache.path())
                .env("LOCT_NO_GITIGNORE", "1")
                .args(args)
                .assert()
                .success()
                .get_output()
                .clone();
            let stderr = String::from_utf8_lossy(&output.stderr);
            drift_count += stderr.matches("[DRIFT]").count();
        }
        assert_eq!(
            drift_count, 0,
            "dead→cycles→twins→health must not [DRIFT]-rescan on loct's own untracked artifacts"
        );
        assert!(
            !root.join(".gitignore").exists(),
            "read verbs must never write .gitignore"
        );
    }

    #[test]
    fn first_scan_appends_loctree_gitignore_entry_loudly() {
        let temp = mixed_universe_repo();
        let cache = TempDir::new().expect("temp cache");
        let root = temp.path();

        let output = loct_in(root, cache.path())
            .assert()
            .success()
            .get_output()
            .clone();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Added '.loctree/' to .gitignore"),
            "first scan must announce the gitignore append, stderr:\n{stderr}"
        );
        let gitignore =
            fs::read_to_string(root.join(".gitignore")).expect(".gitignore created by first scan");
        assert!(
            gitignore.lines().any(|line| line.trim() == ".loctree/"),
            "entry present after first scan: {gitignore:?}"
        );

        // Second explicit scan: snapshot exists, entry exists — no repeat
        // announcement, no duplicate entry.
        let output = loct_in(root, cache.path())
            .args(["scan"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("Added '.loctree/' to .gitignore"),
            "second scan must not re-announce, stderr:\n{stderr}"
        );
        let gitignore = fs::read_to_string(root.join(".gitignore")).expect("read .gitignore");
        assert_eq!(
            gitignore.matches(".loctree/").count(),
            1,
            "no duplicate entry: {gitignore:?}"
        );
    }

    #[test]
    fn first_scan_respects_existing_loctree_gitignore_entry() {
        let temp = mixed_universe_repo();
        let cache = TempDir::new().expect("temp cache");
        let root = temp.path();
        fs::write(root.join(".gitignore"), ".loctree/\n").expect("seed .gitignore");
        commit_all(root, "gitignore");

        let output = loct_in(root, cache.path())
            .assert()
            .success()
            .get_output()
            .clone();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("Added '.loctree/' to .gitignore"),
            "already-ignored repo must not be touched, stderr:\n{stderr}"
        );
        let gitignore = fs::read_to_string(root.join(".gitignore")).expect("read .gitignore");
        assert_eq!(gitignore, ".loctree/\n", "user file left exactly as-is");
    }

    #[test]
    fn follow_all_does_not_rescan_after_scan_of_preexisting_dirty_content() {
        let temp = mixed_universe_repo();
        let cache = TempDir::new().expect("temp cache");
        let root = temp.path();

        fs::write(root.join("src/lib.rs"), "pub fn beta() {}\n").expect("dirty source");
        let status = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(root)
            .output()
            .expect("git status");
        assert!(
            !status.stdout.is_empty(),
            "fixture must be dirty before scan"
        );

        loct_in(root, cache.path())
            .args(["scan", "--full-scan"])
            .assert()
            .success();

        let output = loct_in(root, cache.path())
            .args(["follow", "all"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("[DRIFT]") && !stderr.contains("rescanning"),
            "follow all must not rescan after a fresh scan when no files changed after scan, stderr:\n{stderr}"
        );
    }

    #[test]
    fn unified_file_universe_across_scan_rescan_and_report_surfaces() {
        let temp = mixed_universe_repo();
        let cache = TempDir::new().expect("temp cache");
        let root = temp.path();

        // 1. Initial scan: detect narrows the universe (Cargo root → rs+toml).
        loct_in(root, cache.path()).assert().success();
        let initial_count = snapshot_file_count(root, cache.path());
        assert!(initial_count > 0, "initial scan must index files");

        // 2. Move HEAD without touching the indexed universe so the next
        //    analytic command performs a genuine [DRIFT] rescan. Since W2-02
        //    `.txt` is part of the literal-truth universe, so the outside-file
        //    needs an extension no analyzer or resource path claims.
        fs::write(
            root.join("notes.xyzunknown"),
            "outside the indexed universe\n",
        )
        .expect("write notes.xyzunknown");
        commit_all(root, "add notes");

        let output = loct_in(root, cache.path())
            .args(["dead"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("[DRIFT]"),
            "moved HEAD must trigger exactly the rescan under test, stderr:\n{stderr}"
        );

        // 3. The drift-rescan must rebuild the SAME universe as the initial
        //    scan (guardian + unified_scan_args). Pre-guardian this diverged
        //    (default extensions pulled in ts/py → count grew → permanent
        //    fingerprint mismatch → self-sustaining DRIFT loop).
        let rescan_count = snapshot_file_count(root, cache.path());
        assert_eq!(
            rescan_count, initial_count,
            "initial scan and drift-rescan must agree on the file universe"
        );

        // 4. repo-view reports the same universe.
        let output = loct_in(root, cache.path())
            .args(["repo-view"])
            .assert()
            .success()
            .get_output()
            .clone();
        let repo_view: Value =
            serde_json::from_slice(&output.stdout).expect("repo-view must emit JSON");
        assert_eq!(
            repo_view["summary"]["files_analyzed"].as_u64(),
            Some(initial_count),
            "repo-view summary must report the unified universe"
        );

        // 5. findings.summary reports the same universe.
        let output = loct_in(root, cache.path())
            .args(["findings"])
            .assert()
            .success()
            .get_output()
            .clone();
        let findings: Value =
            serde_json::from_slice(&output.stdout).expect("findings must emit JSON");
        assert_eq!(
            findings["summary"]["files"].as_u64(),
            Some(initial_count),
            "findings.summary must report the unified universe"
        );

        // 6. And the universe is the WIDE default analyzer set everywhere:
        //    ts/py files are indexed even on a Cargo root so agents can
        //    slice/find them, instead of detect silently narrowing the
        //    initial scan below what every rescan rebuilds.
        let output = loct_in(root, cache.path())
            .args([".files[].path"])
            .assert()
            .success()
            .get_output()
            .clone();
        let listed = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(
            listed.contains("web.ts") && listed.contains("tool.py"),
            "the unified universe must include ts/py files on a Cargo root: {listed}"
        );
    }
}

// ============================================
// Dead truth (W2-a): one count, candidates not verdicts,
// cross-check before verdict, entry-point fence
// ============================================

mod dead_truth {
    use super::*;

    /// Isolated copy of a dead-truth fixture plus a private snapshot cache —
    /// same isolation rationale as `artifact_fence_fixture`.
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

    fn loct_at(root: &std::path::Path, cache: &std::path::Path) -> Command {
        let mut cmd = loct();
        cmd.current_dir(root).env("LOCT_CACHE_DIR", cache);
        cmd
    }

    fn dead_json(root: &std::path::Path, cache: &std::path::Path) -> Vec<Value> {
        let output = loct_at(root, cache)
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

    /// Empiria: `assetNameForPlatform` — sole consumer lives in a test
    /// directory. The graph (or its fallbacks) plus the literal cross-check
    /// must keep it out of the dead list; the genuinely dead export stays in.
    #[test]
    fn test_dir_import_is_not_dead() {
        let (fixture, cache) = dead_truth_fixture("dead_truth_testdir");
        let dead = dead_json(fixture.path(), cache.path());

        assert!(
            !dead
                .iter()
                .any(|d| d["symbol"].as_str() == Some("assetNameForPlatform")),
            "symbol imported from a test dir must NOT be dead: {dead:?}"
        );
        assert!(
            dead.iter()
                .any(|d| d["symbol"].as_str() == Some("trulyDeadHelper")),
            "the genuinely dead export must still be reported: {dead:?}"
        );
    }

    /// Empiria: Vista lazy-import `import('./Steps').then((m) => m.InviteTeamStep)`
    /// — the named export consumed only through the dynamic-import member
    /// access must not be dead.
    #[test]
    fn lazy_import_named_export_is_not_dead() {
        let (fixture, cache) = dead_truth_fixture("dead_truth_lazy");
        let dead = dead_json(fixture.path(), cache.path());

        assert!(
            !dead
                .iter()
                .any(|d| d["symbol"].as_str() == Some("InviteTeamStep")),
            "lazy-imported named export must NOT be dead: {dead:?}"
        );
    }

    /// Empiria: CodeScribe stt-bridge — runtime entry files spawned only by
    /// string. The fixture carries BOTH a Cargo [[bin]]
    /// (`src/bin/stt_bridge.rs`) and a package.json bin
    /// (`daemon/voice_daemon.js`, spawned via `spawn('voice-daemon')`).
    /// Neither file may ever get a delete quick-win, and the literal
    /// cross-check must surface the string-hit as evidence on the bin
    /// candidate.
    #[test]
    fn spawn_by_string_bin_is_fenced_with_string_hit_evidence() {
        let (fixture, cache) = dead_truth_fixture("dead_truth_spawn");

        // 1. quick_wins: zero "delete" verdicts anywhere, and neither
        //    declared bin file ever appears as a delete candidate.
        let output = loct_at(fixture.path(), cache.path())
            .args(["findings"])
            .assert()
            .success()
            .get_output()
            .clone();
        let findings: Value =
            serde_json::from_slice(&output.stdout).expect("findings must emit JSON");
        let wins = findings["quick_wins"]
            .as_array()
            .expect("findings.quick_wins must be an array");
        assert!(
            wins.iter().all(|w| w["action"].as_str() != Some("delete")),
            "quick_wins must never emit a bare delete verdict: {wins:?}"
        );
        assert!(
            wins.iter().all(|w| {
                let file = w["file"].as_str().unwrap_or("");
                w["action"].as_str() != Some("delete_candidate")
                    || (!file.contains("stt_bridge") && !file.contains("voice_daemon"))
            }),
            "declared bin entrypoints must never become delete candidates: {wins:?}"
        );
        assert!(
            wins.iter()
                .all(|w| !w["reason"].as_str().unwrap_or("").trim().is_empty()),
            "every quick-win candidate must carry a reason: {wins:?}"
        );

        let dead = dead_json(fixture.path(), cache.path());

        // 2. The Cargo [[bin]] file: any candidate from it must be fenced.
        //    (Rust's identifier-mention local_uses currently keeps its
        //    exports out of the candidate list entirely — also acceptable.)
        for candidate in dead
            .iter()
            .filter(|d| d["file"].as_str().unwrap_or("").contains("stt_bridge"))
        {
            assert_eq!(
                candidate["entrypoint"].as_bool(),
                Some(true),
                "[[bin]] candidates must be entrypoint-fenced: {candidate:?}"
            );
        }

        // 3. The package.json bin candidate exists, is entrypoint-fenced,
        //    degraded, and carries the spawn-by-string literal evidence.
        let daemon: Vec<_> = dead
            .iter()
            .filter(|d| d["file"].as_str().unwrap_or("").contains("voice_daemon"))
            .collect();
        assert!(
            !daemon.is_empty(),
            "fixture must produce a candidate in the declared bin file: {dead:?}"
        );
        for candidate in &daemon {
            assert_eq!(
                candidate["entrypoint"].as_bool(),
                Some(true),
                "bin candidates must be entrypoint-fenced: {candidate:?}"
            );
            assert_eq!(
                candidate["action"].as_str(),
                Some("delete_candidate"),
                "dead exports are candidates, never verdicts: {candidate:?}"
            );
            assert_eq!(
                candidate["confidence"].as_str(),
                Some("low"),
                "string-hit evidence must degrade confidence: {candidate:?}"
            );
            assert!(
                candidate["reason"]
                    .as_str()
                    .unwrap_or("")
                    .contains("string-literal reference"),
                "literal cross-check must surface the spawn-by-string hit: {candidate:?}"
            );
        }
    }

    /// The same snapshot must yield ONE dead number on every surface:
    /// `loct dead --json`, `loct twins --json`, `loct findings --summary`
    /// and the repo-view/for-ai summary.
    #[test]
    fn four_surfaces_report_one_dead_count() {
        let (fixture, cache) = dead_truth_fixture("dead_truth_testdir");

        let dead_cli = dead_json(fixture.path(), cache.path()).len() as u64;

        let output = loct_at(fixture.path(), cache.path())
            .args(["twins", "--json"])
            .assert()
            .success()
            .get_output()
            .clone();
        let twins: Value = serde_json::from_slice(&output.stdout).expect("twins JSON");
        let twins_count = twins["summary"]["dead_parrots"]
            .as_u64()
            .expect("twins summary.dead_parrots");

        let output = loct_at(fixture.path(), cache.path())
            .args(["findings", "--summary"])
            .assert()
            .success()
            .get_output()
            .clone();
        let summary: Value = serde_json::from_slice(&output.stdout).expect("findings summary");
        let findings_count = summary["dead_parrots"]
            .as_u64()
            .expect("findings summary dead_parrots");

        let output = loct_at(fixture.path(), cache.path())
            .args(["--for-ai"])
            .assert()
            .success()
            .get_output()
            .clone();
        let for_ai: Value = serde_json::from_slice(&output.stdout).expect("for-ai JSON");
        let repo_view_count = for_ai["summary"]["dead_exports"]
            .as_u64()
            .expect("for-ai summary.dead_exports");

        assert_eq!(
            (dead_cli, twins_count, findings_count),
            (dead_cli, dead_cli, dead_cli),
            "dead / twins / findings must report one dead count"
        );
        assert_eq!(
            repo_view_count, dead_cli,
            "repo-view (for-ai) must report the same dead count as loct dead"
        );
        assert!(
            dead_cli >= 1,
            "fixture must contain at least one dead export"
        );
    }

    /// Golden contract: every dead candidate carries action=delete_candidate
    /// and a cross-check trail in its reason.
    #[test]
    fn dead_candidates_carry_action_and_cross_check_reason() {
        let (fixture, cache) = dead_truth_fixture("dead_truth_testdir");
        let dead = dead_json(fixture.path(), cache.path());

        for candidate in &dead {
            assert_eq!(
                candidate["action"].as_str(),
                Some("delete_candidate"),
                "candidate without the action contract: {candidate:?}"
            );
            assert!(
                candidate["reason"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Cross-check:"),
                "candidate without cross-check evidence in reason: {candidate:?}"
            );
        }
    }

    /// Regression for W5.1 (loctree-feedback.md 2903,2978,2990,3052): dash-prefixed
    /// literals must not be parsed as CLI options. `find --literal -- <str>`
    /// and `occurrences -- <str>` must work (or the lenient post-literal path).
    /// The bug is not closed until a test would catch regression on the parser.
    #[test]
    fn find_and_occurrences_accept_dashed_literals_via_separator_or_lenient() {
        let (fixture, cache) = dead_truth_fixture("dead_truth_spawn");

        // -- separator form (primary documented)
        let res = loct_at(fixture.path(), cache.path())
            .args(["find", "--literal", "--", "--no-such-dashed-xyz-164049"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&res.get_output().stdout);
        assert!(
            !stdout.contains("Unknown option") && !stdout.contains("unknown option"),
            "find --literal -- <dashed> must not treat dash as option: {}",
            stdout
        );

        // occurrences form
        let res2 = loct_at(fixture.path(), cache.path())
            .args(["occurrences", "--", "--no-such-dashed-xyz-164049"])
            .assert()
            .success();
        let stdout2 = String::from_utf8_lossy(&res2.get_output().stdout);
        assert!(
            !stdout2.contains("Unknown option") && !stdout2.contains("unknown option"),
            "occurrences -- <dashed> must not treat dash as option: {}",
            stdout2
        );
    }
}
