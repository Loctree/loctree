//! Product contracts for default find: source before accounting, real private
//! definitions, bounded previews, and literal/page/scope preservation.

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use tempfile::TempDir;

const SOURCE: &str = "struct StopCanvasRequest {\n    stopped_at: u64,\n    target: String,\n}\nfn begin_stop_paste() -> StopCanvasRequest {\n    StopCanvasRequest { stopped_at: 0, target: String::new() }\n}\nfn deliver_frozen_canvas_at_stop(stop: StopCanvasRequest) {}\nfn process_recording(stop: StopCanvasRequest) {}\n";

struct Project {
    root: TempDir,
    cache: TempDir,
}

impl Project {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/lib.rs"), SOURCE).unwrap();
        Self {
            root,
            cache: tempfile::tempdir().unwrap(),
        }
    }

    fn run(&self, args: &[&str]) -> String {
        let mut cmd = cargo_bin_cmd!("loct");
        let output = cmd
            .current_dir(self.root.path())
            .env("LOCT_CACHE_DIR", self.cache.path())
            .env("LOCT_OPEN_BROWSER", "0")
            .env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1")
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(output).unwrap()
    }
}

#[test]
fn private_stop_request_shows_fields_and_usage_purposes_before_coverage() {
    let project = Project::new();
    let out = project.run(&["find", "StopCanvasRequest"]);
    assert!(out.contains("definitions (live source): 1"), "{out}");
    assert!(out.contains("struct StopCanvasRequest {"), "{out}");
    assert!(out.contains("target: String"), "{out}");
    assert!(out.contains("[return_type] in begin_stop_paste"), "{out}");
    assert!(
        out.contains("[construction_expression] in begin_stop_paste"),
        "{out}"
    );
    assert!(
        out.contains("[type_annotation] in deliver_frozen_canvas_at_stop"),
        "{out}"
    );
    assert!(out.contains("roles: 1 definition, 4 reference"), "{out}");
    assert!(!out.contains("5 callsite"));
    assert!(out.find("struct StopCanvasRequest").unwrap() < out.find("coverage:").unwrap());
    assert!(!out.contains("loct follow all"));
}

#[test]
fn structured_context_preserves_literal_coordinates_and_pagination() {
    let project = Project::new();
    let default: Value =
        serde_json::from_str(&project.run(&["find", "StopCanvasRequest", "--json"])).unwrap();
    let literal: Value =
        serde_json::from_str(&project.run(&["find", "--literal", "StopCanvasRequest", "--json"]))
            .unwrap();
    assert_eq!(default["literal_matches"], literal["literal_matches"]);
    assert_eq!(default["literal_matches"]["role_summary"]["definitions"], 1);
    assert_eq!(default["symbol_context"]["definitions"][0]["start_line"], 1);
    assert!(literal.get("symbol_context").is_none());
    let page: Value = serde_json::from_str(&project.run(&[
        "find",
        "StopCanvasRequest",
        "--limit",
        "1",
        "--offset",
        "1",
        "--json",
    ]))
    .unwrap();
    assert_eq!(page["literal_matches"]["total"], 5);
    assert_eq!(page["symbol_context"]["uses"].as_array().unwrap().len(), 1);
}

#[test]
fn same_name_definitions_stay_distinct_and_file_scope_excludes_other_bodies() {
    let project = Project::new();
    std::fs::write(
        project.root.path().join("src/other.rs"),
        "struct StopCanvasRequest {\n    unrelated: bool,\n}\n",
    )
    .unwrap();
    let all: Value =
        serde_json::from_str(&project.run(&["find", "StopCanvasRequest", "--json"])).unwrap();
    assert_eq!(all["symbol_context"]["total_definitions"], 2);
    let scoped: Value = serde_json::from_str(&project.run(&[
        "find",
        "StopCanvasRequest",
        "--path",
        "src/lib.rs",
        "--json",
    ]))
    .unwrap();
    assert_eq!(scoped["symbol_context"]["total_definitions"], 1);
    assert_eq!(
        scoped["symbol_context"]["definitions"][0]["file"],
        "src/lib.rs"
    );
}

#[test]
fn previews_are_bounded_and_do_not_replace_count_only_contract() {
    let project = Project::new();
    let fields = (0..30)
        .map(|i| format!("    field_{i}: u64,\n"))
        .collect::<String>();
    std::fs::write(
        project.root.path().join("src/lib.rs"),
        format!("struct StopCanvasRequest {{\n{fields}}}\n"),
    )
    .unwrap();
    let out = project.run(&["find", "StopCanvasRequest"]);
    assert!(out.contains("preview; truncated"), "{out}");
    assert!(!out.contains("field_29"));
    let counts: Value = serde_json::from_str(&project.run(&[
        "find",
        "StopCanvasRequest",
        "--count-only",
        "--json",
    ]))
    .unwrap();
    assert_eq!(counts["literal_matches"]["total"], 1);
    assert!(counts.get("symbol_context").is_none());
}

#[test]
fn many_definitions_are_counted_without_dumping_every_body() {
    let project = Project::new();
    for i in 0..5 {
        std::fs::write(
            project.root.path().join(format!("src/other_{i}.rs")),
            format!("struct StopCanvasRequest {{ unique_{i}: u64 }}\n"),
        )
        .unwrap();
    }
    let result: Value =
        serde_json::from_str(&project.run(&["find", "StopCanvasRequest", "--json"])).unwrap();
    assert_eq!(result["symbol_context"]["total_definitions"], 6);
    assert_eq!(
        result["symbol_context"]["definitions"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(result["symbol_context"]["definitions_truncated"], true);
    let miss: Value = serde_json::from_str(&project.run(&[
        "find",
        "StopCanvasRequest",
        "--path",
        "no/such/file.rs",
        "--json",
    ]))
    .unwrap();
    assert_eq!(miss["literal_matches"]["total"], 0);
    assert!(miss.get("symbol_context").is_none());
}

#[test]
fn no_generated_does_not_reintroduce_a_skipped_definition_as_a_preview() {
    let project = Project::new();
    std::fs::create_dir(project.root.path().join("src/generated")).unwrap();
    std::fs::write(
        project.root.path().join("src/generated/request.rs"),
        "// @generated\nstruct StopCanvasRequest { secret_generated_field: u64 }\n",
    )
    .unwrap();
    let result: Value = serde_json::from_str(&project.run(&[
        "find",
        "StopCanvasRequest",
        "--no-generated",
        "--json",
    ]))
    .unwrap();
    assert_eq!(result["symbol_context"]["total_definitions"], 1);
    assert_eq!(
        result["symbol_context"]["definitions"][0]["file"],
        "src/lib.rs"
    );
}
