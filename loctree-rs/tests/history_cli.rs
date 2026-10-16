//! Real Git histories must lead to exact structural comparison endpoints.
use git2::{Repository, Signature};
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn commit(repo: &Repository, file: &str, body: &str) -> String {
    std::fs::write(repo.workdir().expect("workdir").join(file), body).expect("fixture file");
    let mut index = repo.index().expect("index");
    index.add_path(Path::new(file)).expect("stage");
    index.write().expect("index write");
    let tree = repo
        .find_tree(index.write_tree().expect("tree write"))
        .expect("tree");
    let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
    let timestamp = parent
        .as_ref()
        .map_or(1_700_000_000, |c| c.time().seconds() + 1);
    let signature = Signature::new(
        "Fixture",
        "fixture@example.invalid",
        &git2::Time::new(timestamp, 0),
    )
    .expect("signature");
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        file,
        &tree,
        &parent.iter().collect::<Vec<_>>(),
    )
    .expect("commit")
    .to_string()
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_loct"))
        .current_dir(root)
        .args(args)
        .env("LOCT_CACHE_DIR", root.join("fixture-cache"))
        .env("LOCT_NO_GITIGNORE", "1")
        .output()
        .expect("run compiled CLI")
}

#[test]
fn history_filters_files_and_pins_comparison_refs_without_scanning() {
    let temp = tempfile::TempDir::new().expect("fixture");
    let root = temp.path();
    let repo = Repository::init(root).expect("git init");
    let first = commit(&repo, "entry.ts", "export const old = 1;\n");
    let parent = commit(&repo, "other.ts", "export const other = 1;\n");
    let tip = commit(&repo, "entry.ts", "export const latest = 1;\n");
    std::fs::create_dir(root.join("subdir")).expect("subdir");
    let output = run(
        &root.join("subdir"),
        &["history", "../entry.ts", "--limit", "2", "--json"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("history JSON");
    assert_eq!(value["at"], tip);
    assert_eq!(value["file"], "entry.ts");
    assert_eq!(value["comparison_scope"], "repository");
    assert_comparison(&value["suggested_next"][0], &parent, &tip);
    assert_eq!(value["commits"].as_array().expect("commits").len(), 2);
    assert!(
        value["commits"]
            .as_array()
            .expect("commits")
            .iter()
            .all(|c| c["message"] == "entry.ts")
    );
    assert!(
        value["suggested_next"][0]["command"]
            .as_str()
            .expect("command")
            .contains(&tip)
    );
    let output = run(root, &["history", "--at", &first, "--json"]);
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).expect("at JSON");
    assert_eq!(value["commits"].as_array().expect("commits").len(), 1);
    for args in [
        vec!["history", "missing.ts"],
        vec!["history", "--at", "missing-ref"],
        vec!["history", "../outside.ts"],
        vec!["history", "--limit", "0"],
    ] {
        assert!(!run(root, &args).status.success(), "must reject {args:?}");
    }
    assert_eq!(
        repo.head()
            .expect("head")
            .target()
            .expect("oid")
            .to_string(),
        tip
    );
    assert_eq!(repo.worktrees().expect("worktrees").len(), 0);
    assert!(
        !root.join("fixture-cache").exists(),
        "timeline must not trigger a structural scan"
    );
}

fn history(root: &Path, args: &[&str]) -> Value {
    let output = run(root, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("history JSON")
}

fn assert_comparison(step: &Value, parent: &str, selected: &str) {
    assert_eq!(step["commit"], selected);
    assert_eq!(
        step["command"],
        format!("loct diff --since {parent} --to {selected}")
    );
    assert_ne!(parent, selected);
    assert_eq!(step["comparison"], "first_parent");
    assert_eq!(step["since"], parent);
    assert_eq!(step["to"], selected);
    assert_eq!(step["parents"][0], parent);
}

fn assert_structural_delta(root: &Path, step: &Value, added: &str, removed: &str) {
    let mut args: Vec<_> = step["command"]
        .as_str()
        .expect("executable recommendation")
        .split_whitespace()
        .skip(1)
        .collect();
    args.push("--json");
    let output = run(root, &args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let delta: Value = serde_json::from_slice(&output.stdout).expect("structural diff JSON");
    assert_eq!(delta["from_commit"]["hash"], step["since"]);
    assert_eq!(delta["to_commit"]["hash"], step["to"]);
    for (kind, expected) in [("added", added), ("removed", removed)] {
        let symbols = delta["exports"][kind].as_array().expect("export changes");
        assert_eq!(symbols.len(), 1, "{delta}");
        assert_eq!(symbols[0]["name"], expected, "{delta}");
        assert_eq!(symbols[0]["file"], "entry.ts");
    }
}

#[test]
fn history_tip_compares_the_change_in_the_commit_with_frozen_endpoints() {
    let temp = tempfile::TempDir::new().expect("fixture");
    let root = temp.path();
    let repo = Repository::init(root).expect("git init");
    let parent = commit(&repo, "entry.ts", "export const before = 1;\n");
    let tip = commit(&repo, "entry.ts", "export const after = 1;\n");
    let value = history(root, &["history", "--at", &tip, "--limit", "1", "--json"]);
    assert_eq!(value["at"], tip);
    assert_comparison(&value["suggested_next"][0], &parent, &tip);
    assert!(!root.join("fixture-cache").exists());
    commit(&repo, "entry.ts", "export const future = 1;\n");
    assert_structural_delta(root, &value["suggested_next"][0], "after", "before");
}

#[test]
fn history_older_file_commit_compares_to_its_parent_not_the_at_revision() {
    let temp = tempfile::TempDir::new().expect("fixture");
    let root = temp.path();
    let repo = Repository::init(root).expect("git init");
    let parent = commit(&repo, "entry.ts", "export const before = 1;\n");
    let selected = commit(&repo, "entry.ts", "export const after = 1;\n");
    let tip = commit(&repo, "other.ts", "export const unrelated = 1;\n");
    std::fs::create_dir(root.join("subdir")).expect("subdir");
    let value = history(
        &root.join("subdir"),
        &[
            "history",
            "../entry.ts",
            "--at",
            &tip,
            "--limit",
            "1",
            "--json",
        ],
    );
    assert_eq!(value["at"], tip);
    assert_eq!(value["file"], "entry.ts");
    assert_eq!(value["commits"].as_array().expect("commits").len(), 1);
    assert_eq!(value["commits"][0]["hash"], selected);
    assert_comparison(&value["suggested_next"][0], &parent, &selected);
    assert_structural_delta(root, &value["suggested_next"][0], "after", "before");
}

#[test]
fn history_initial_commit_reports_a_boundary_without_an_unrunnable_command() {
    let temp = tempfile::TempDir::new().expect("fixture");
    let root = temp.path();
    let repo = Repository::init(root).expect("git init");
    let initial = commit(&repo, "entry.ts", "export const initial = 1;\n");
    let value = history(root, &["history", "--at", &initial, "--json"]);
    let step = &value["suggested_next"][0];
    assert_eq!(step["commit"], initial);
    assert!(step["command"].is_null(), "{step}");
    assert!(step["since"].is_null());
    assert_eq!(step["to"], initial);
    assert_eq!(step["parents"], serde_json::json!([]));
    assert_eq!(step["boundary"], "initial_commit");
    assert!(
        step["reason"]
            .as_str()
            .expect("boundary reason")
            .contains("no parent")
    );
    let text = run(root, &["history"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).expect("text");
    assert!(text.contains("no parent"), "{text}");
    assert!(!text.contains("loct diff"), "{text}");
    assert!(!root.join("fixture-cache").exists());
}

#[test]
fn history_merge_compares_the_net_change_from_its_actual_first_parent() {
    let temp = tempfile::TempDir::new().expect("fixture");
    let root = temp.path();
    let repo = Repository::init(root).expect("git init");
    let base = commit(&repo, "entry.ts", "export const base = 1;\n");
    let main = commit(&repo, "entry.ts", "export const main = 1;\n");
    let base_commit = repo
        .find_commit(base.parse().expect("base oid"))
        .expect("base");
    let main_commit = repo
        .find_commit(main.parse().expect("main oid"))
        .expect("main");
    let mut builder = repo
        .treebuilder(Some(&base_commit.tree().expect("base tree")))
        .expect("builder");
    let blob = repo.blob(b"export const side = 1;\n").expect("side blob");
    builder
        .insert("entry.ts", blob, 0o100644)
        .expect("side entry");
    let side_tree = repo
        .find_tree(builder.write().expect("side tree write"))
        .expect("side tree");
    let signature = Signature::new(
        "Fixture",
        "fixture@example.invalid",
        &git2::Time::new(main_commit.time().seconds() + 1, 0),
    )
    .expect("signature");
    let side = repo
        .commit(
            None,
            &signature,
            &signature,
            "side",
            &side_tree,
            &[&base_commit],
        )
        .expect("side");
    let side_commit = repo.find_commit(side).expect("side commit");
    let merge = repo
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "merge",
            &side_tree,
            &[&main_commit, &side_commit],
        )
        .expect("merge")
        .to_string();
    let value = history(
        root,
        &[
            "history", "entry.ts", "--at", &merge, "--limit", "1", "--json",
        ],
    );
    let step = &value["suggested_next"][0];
    assert_comparison(step, &main, &merge);
    assert_eq!(step["parents"], serde_json::json!([main, side.to_string()]));
    assert_structural_delta(root, step, "side", "main");
}
