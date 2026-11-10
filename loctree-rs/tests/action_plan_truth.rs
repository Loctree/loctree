//! The public producer and HTML first page must agree on evidence, not name frequency.
use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

fn run(root: &Path, cache: &Path, args: &[&str]) -> String {
    let output = cargo_bin_cmd!("loct")
        .current_dir(root)
        .env("LOCT_OPEN_BROWSER", "0")
        .env("LOCT_CACHE_DIR", cache)
        .env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1")
        .args(args)
        .output()
        .expect("run isolated CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 CLI output")
}

fn surfaces(root: &Path, cache: &Path) -> (Value, String) {
    let stdout = run(root, cache, &["--for-ai"]);
    let start = stdout.find('{').expect("agent JSON");
    let agent = serde_json::from_str(&stdout[start..]).expect("parse agent JSON");
    let html_path = cache.join("report.html");
    run(
        root,
        cache,
        &["report", "--output", html_path.to_str().expect("path")],
    );
    (
        agent,
        std::fs::read_to_string(html_path).expect("rendered HTML"),
    )
}

#[test]
fn scoped_protocol_properties_and_typed_methods_are_not_action_items() {
    let root = TempDir::new().expect("source");
    let cache = TempDir::new().expect("cache");
    for i in 0..8 {
        std::fs::write(root.path().join(format!("Screen{i}.swift")), format!(
            "import SwiftUI\nstruct Screen{i}: View {{\n    var body: some View {{\n        Text(\"Screen {i}\")\n    }}\n}}\n"
        )).expect("Swift source");
        std::fs::write(root.path().join(format!("owner{i}.rs")), format!(
            "pub struct Owner{i};\nimpl Owner{i} {{ pub fn as_str(&self) -> &str {{ \"value\" }} pub fn compute(&self) -> usize {{ {i} }} }}\n"
        )).expect("Rust source");
    }
    let (agent, html) = surfaces(root.path(), cache.path());
    assert!(
        !agent["bundle"]["duplicates"]
            .as_array()
            .expect("raw observations")
            .is_empty()
    );
    let observations = agent["bundle"]["duplicates"]
        .as_array()
        .expect("raw observations");
    assert!(
        observations.iter().any(|d| d["name"] == "body"),
        "Swift property remains inspectable"
    );
    assert!(
        observations.iter().any(|d| d["name"] == "compute"),
        "typed method remains inspectable"
    );
    for field in ["quick_wins", "priority_tasks"] {
        let tasks = agent[field].as_array().expect("task list");
        assert!(
            !tasks
                .iter()
                .any(|task| matches!(task["target"].as_str(), Some("body" | "as_str" | "compute"))),
            "{field}: {tasks:?}"
        );
    }
    assert!(!html.contains("creating ambiguity for importers"));
    assert!(!html.contains("class=\"action-target\">body"));
    assert!(html.contains("repeated names"));
}

#[test]
fn genuine_duplicate_remains_reviewable_with_bounded_html_and_full_evidence() {
    let root = TempDir::new().expect("source");
    let cache = TempDir::new().expect("cache");
    for i in 0..12 {
        std::fs::write(root.path().join(format!("ledger_{i:02}.ts")),
            "interface Ledger { value: number }\nexport function normalizeLedger(input: Ledger): Ledger { return { value: Math.max(0, input.value) }; }\n"
        ).expect("duplicate source");
    }
    let (agent, html) = surfaces(root.path(), cache.path());
    for field in ["quick_wins", "priority_tasks"] {
        let task = agent[field]
            .as_array()
            .expect("task list")
            .iter()
            .find(|t| t["target"] == "normalizeLedger")
            .expect("positive survives");
        assert_eq!(task["kind"], "duplicate_candidate");
        assert!(task["why"].as_str().expect("why").contains("signature"));
        assert!(
            task["fix_hint"]
                .as_str()
                .expect("review hint")
                .contains("9 more")
        );
        assert!(!task["why"].as_str().expect("why").contains("ambiguity"));
    }
    let task = agent["priority_tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .find(|t| t["target"] == "normalizeLedger")
        .expect("priority");
    assert_eq!(task["risk"], "medium");
    assert_eq!(task["verify_cmd"], "loct twins --json");
    let raw = agent["bundle"]["duplicates"]
        .as_array()
        .expect("observations")
        .iter()
        .find(|t| t["name"] == "normalizeLedger")
        .expect("full duplicate");
    assert_eq!(raw["files"].as_u64().expect("all files"), 12);
    let start = html
        .find("<li class=\"action-item\">")
        .expect("action card");
    let card = &html[start..start + html[start..].find("</li>").expect("card end")];
    assert!(card.contains("duplicate_candidate"));
    assert!(card.contains("Review"));
    assert!(card.contains("9 more"));
    assert!(card.contains("loct twins --json"));
    assert!(
        !card.contains("ledger_11.ts"),
        "full path list must stay off the first card"
    );
    assert!(
        html.contains("ledger_11.ts"),
        "full inspection data remains available"
    );
}

#[test]
fn reexports_and_cross_language_namesakes_stay_out_of_action_plan() {
    let root = TempDir::new().expect("source");
    let cache = TempDir::new().expect("cache");
    std::fs::write(
        root.path().join("source.ts"),
        "export function transferLedger(input: string): string { return input.trim(); }\n",
    )
    .expect("source");
    for i in 0..8 {
        std::fs::write(
            root.path().join(format!("barrel{i}.ts")),
            "export { transferLedger } from './source';\nexport class SharedEnvelope {}\n",
        )
        .expect("barrel");
        std::fs::write(
            root.path().join(format!("crate{i}.rs")),
            "pub struct SharedEnvelope;\n",
        )
        .expect("Rust namesake");
        std::fs::write(
            root.path().join(format!("module{i}.py")),
            "class SharedEnvelope:\n    pass\n",
        )
        .expect("Python namesake");
    }
    let (agent, html) = surfaces(root.path(), cache.path());
    for field in ["quick_wins", "priority_tasks"] {
        assert!(
            !agent[field]
                .as_array()
                .expect("task list")
                .iter()
                .any(|t| matches!(
                    t["target"].as_str(),
                    Some("transferLedger" | "SharedEnvelope")
                ))
        );
    }
    assert!(!html.contains("creating ambiguity for importers"));
    assert!(
        !agent["bundle"]["duplicates"]
            .as_array()
            .expect("observations")
            .is_empty()
    );
}
