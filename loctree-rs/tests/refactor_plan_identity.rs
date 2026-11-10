use std::collections::HashMap;

use loctree::refactor_plan::{
    Layer, RiskLevel, execution_blockers, generate_refactor_plan, output, parse_target_layout_spec,
};
use loctree::snapshot::{GraphEdge, Snapshot};
use loctree::types::{ExportSymbol, FileAnalysis};

fn fixture(paths: &[&str]) -> (tempfile::TempDir, Snapshot) {
    let root = tempfile::tempdir().expect("fixture root");
    std::fs::write(root.path().join("package.json"), "{\"type\":\"module\"}")
        .expect("package identity");
    write_fixture(
        root.path(),
        "tsconfig.json",
        "{\"compilerOptions\":{\"module\":\"NodeNext\",\"rootDir\":\".\",\"outDir\":\"dist\"},\"include\":[\"**/*.ts\"]}",
    );
    let mut snapshot = Snapshot::new(vec![root.path().to_string_lossy().into_owned()]);
    for path in paths {
        let full = root.path().join(path);
        std::fs::create_dir_all(full.parent().expect("parent")).expect("fixture directories");
        std::fs::write(&full, "export const value = 42;\n").expect("fixture source");
        let mut file = FileAnalysis::new((*path).to_string());
        file.exports.push(ExportSymbol {
            name: "value".into(),
            kind: "const".into(),
            export_type: "named".into(),
            line: Some(1),
            params: vec![],
            symbol_id: Default::default(),
        });
        snapshot.files.push(file);
    }
    (root, snapshot)
}

#[test]
fn default_plan_preserves_repository_identity() {
    let (_root, snapshot) = fixture(&[
        "Cargo.toml",
        "crates/one/Cargo.toml",
        "crates/one/src/lib.rs",
        "crates/two/Cargo.toml",
        "crates/two/src/lib.rs",
        "pyproject.toml",
        "one/__init__.py",
        "two/__init__.py",
        "one/__main__.py",
        "two/__main__.py",
        "package.json",
        "src/components/a/index.ts",
        "src/components/b/index.ts",
        "src/assets/icon.svg",
        ".github/FUNDING.yml",
        "fixtures/utils/sample.ts",
        "mystery.ts",
    ]);
    let plan = generate_refactor_plan(&snapshot, ".", None).expect("analysis, not silence");
    assert!(
        plan.moves.is_empty(),
        "path heuristics do not authorize relocation"
    );
    assert!(plan.phases.is_empty());
    assert_eq!(plan.stats.layer_before, plan.stats.layer_after);
}

#[test]
fn explicit_layout_does_not_authorize_rust_python_or_metadata_moves() {
    let (_root, snapshot) = fixture(&[
        "crates/a/src/utils/lib.rs",
        "crates/b/src/utils/lib.rs",
        "Cargo.toml",
        "packages/a/utils/__init__.py",
        "packages/b/utils/__init__.py",
        "packages/a/utils/__main__.py",
        "pyproject.toml",
        ".github/utils/config.yml",
        "src/utils/image.svg",
        "fixtures/utils/sample.ts",
    ]);
    let layout = HashMap::from([(Layer::Infra, "infra".into())]);
    let plan = generate_refactor_plan(&snapshot, ".", Some(&layout)).expect("analysis");
    assert!(
        plan.moves.is_empty(),
        "unsupported module/resource contracts must be kept"
    );
}

#[test]
fn explicit_move_preserves_relative_topology() {
    let (_root, snapshot) = fixture(&["src/components/a/value.ts", "src/components/b/value.ts"]);
    let layout = HashMap::from([(Layer::UI, "ui".into())]);
    let plan = generate_refactor_plan(&snapshot, "src", Some(&layout)).expect("analysis");
    let targets: Vec<_> = plan.moves.iter().map(|mv| mv.target.as_str()).collect();
    assert_eq!(
        targets,
        [
            "src/ui/components/a/value.ts",
            "src/ui/components/b/value.ts"
        ]
    );
    assert_eq!(plan.shims.len(), 2, "preserve even unrepresented consumers");
}

#[test]
fn existing_destination_is_not_a_recommended_move() {
    let (root, snapshot) = fixture(&["src/components/value.ts"]);
    let destination = root.path().join("src/ui/components/value.ts");
    std::fs::create_dir_all(destination.parent().expect("parent")).expect("directories");
    std::fs::write(destination, "do not overwrite").expect("existing file");
    let layout = HashMap::from([(Layer::UI, "ui".into())]);
    let plan = generate_refactor_plan(&snapshot, "src", Some(&layout)).expect("analysis");
    assert!(
        plan.moves.is_empty(),
        "even unindexed destinations must be protected"
    );
}

#[test]
fn duplicate_bundle_destinations_withhold_executable_script() {
    let (_root, snapshot) = fixture(&["src/components/value.ts"]);
    let layout = HashMap::from([(Layer::UI, "ui".into())]);
    let plan = generate_refactor_plan(&snapshot, "src", Some(&layout)).expect("analysis");
    let script = output::format_bundle_as_script(&[plan.clone(), plan]);
    assert!(
        !script.contains("git mv"),
        "conflicting bundle must not emit moves"
    );
    assert!(script.contains("exit 2"));
}

fn run(root: &std::path::Path, program: &str, args: &[&str]) -> std::process::Output {
    std::process::Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .expect("run fixture command")
}

fn successful(output: std::process::Output) -> std::process::Output {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git_fixture(root: &std::path::Path) {
    successful(run(root, "git", &["init", "--quiet"]));
    successful(run(root, "git", &["add", "."]));
}

fn script_fixture() -> (tempfile::TempDir, loctree::refactor_plan::RefactorPlan) {
    let (root, mut snapshot) =
        fixture(&["src/components/a/value.mjs", "src/components/b/value.mjs"]);
    snapshot.files[1].loc = 300; // A second phase exercises phase-filtered shims.
    snapshot
        .files
        .push(FileAnalysis::new("consumer.mjs".into()));
    for path in ["src/components/a/value.mjs", "src/components/b/value.mjs"] {
        snapshot.edges.push(GraphEdge {
            from: "consumer.mjs".into(),
            to: path.into(),
            label: "import".into(),
        });
    }
    std::fs::write(root.path().join("consumer.mjs"), "import { value as a } from './src/components/a/value.mjs';\nimport { value as b } from './src/components/b/value.mjs';\nif (a !== 42 || b !== 42) throw Error('consumer broken');\n").expect("consumer");
    git_fixture(root.path());
    let layout = HashMap::from([(Layer::UI, "ui".into())]);
    let plan = generate_refactor_plan(&snapshot, "src", Some(&layout)).expect("plan");
    assert_eq!(plan.moves.len(), 2);
    assert_eq!(plan.phases.len(), 2);
    assert!(execution_blockers(std::slice::from_ref(&plan)).is_empty());
    std::fs::write(
        root.path().join("refactor.sh"),
        output::format_as_script(&plan),
    )
    .expect("script");
    (root, plan)
}

#[test]
fn script_dry_run_and_phase_execution_preserve_actual_esm_consumers() {
    let (root, plan) = script_fixture();
    successful(run(root.path(), "node", &["consumer.mjs"]));
    let before = successful(run(root.path(), "git", &["status", "--porcelain"])).stdout;
    let dry = successful(run(root.path(), "bash", &["refactor.sh", "--dry"]));
    assert!(
        String::from_utf8_lossy(&dry.stdout).contains("[DRY] create exclusive compatibility shim")
    );
    assert_eq!(
        successful(run(root.path(), "git", &["status", "--porcelain"])).stdout,
        before
    );
    for mv in &plan.moves {
        assert!(!root.path().join(&mv.target).exists());
        assert_eq!(
            std::fs::read_to_string(root.path().join(&mv.source)).expect("source"),
            "export const value = 42;\n"
        );
    }
    successful(run(root.path(), "bash", &["refactor.sh", "1"]));
    successful(run(root.path(), "node", &["consumer.mjs"]));
    assert!(root.path().join(&plan.phases[0].moves[0].target).exists());
    assert!(!root.path().join(&plan.phases[1].moves[0].target).exists());
    successful(run(root.path(), "bash", &["refactor.sh", "2"]));
    successful(run(root.path(), "node", &["consumer.mjs"]));
    successful(run(root.path(), "bash", &["refactor.sh"])); // Verified completed phases are idempotent.
    for mv in &plan.moves {
        assert!(root.path().join(&mv.target).exists());
        assert!(
            std::fs::read_to_string(root.path().join(&mv.source))
                .expect("shim")
                .contains("export * from")
        );
    }
}

#[test]
fn runtime_destination_created_after_planning_is_never_overwritten() {
    let (root, plan) = script_fixture();
    let target = root.path().join(&plan.moves[1].target);
    std::fs::create_dir_all(target.parent().expect("parent")).expect("directories");
    std::fs::write(&target, "existing destination").expect("occupied destination");
    let before = successful(run(root.path(), "git", &["status", "--porcelain"])).stdout;
    for args in [&["refactor.sh", "--dry"][..], &["refactor.sh"][..]] {
        assert!(!run(root.path(), "bash", args).status.success());
        assert_eq!(
            std::fs::read_to_string(&target).expect("destination"),
            "existing destination"
        );
        assert_eq!(
            successful(run(root.path(), "git", &["status", "--porcelain"])).stdout,
            before
        );
        assert!(
            !root.path().join(&plan.moves[0].target).exists(),
            "whole-bundle preflight before first mutation"
        );
    }
}

#[test]
fn stale_source_and_wrong_root_reject_execution_without_mutation() {
    let (root, plan) = script_fixture();
    std::fs::write(
        root.path().join(&plan.moves[1].source),
        "export const value = 99;\n",
    )
    .expect("changed source");
    assert!(!run(root.path(), "bash", &["refactor.sh"]).status.success());
    assert!(!root.path().join(&plan.moves[0].target).exists());
    let script = root.path().join("refactor.sh");
    assert!(
        !run(
            root.path().join("src").as_path(),
            "bash",
            &[script.to_str().expect("path"), "--dry"]
        )
        .status
        .success()
    );
}

#[test]
fn corrupted_move_sets_are_rejected_consistently_across_renderers() {
    let (_root, plan) = script_fixture();
    for kind in [
        "duplicate",
        "case",
        "cycle",
        "ancestor",
        "phase",
        "extension",
        "shim",
    ] {
        let mut invalid = plan.clone();
        match kind {
            "duplicate" => invalid.moves[1].target = invalid.moves[0].target.clone(),
            "case" => invalid.moves[1].target = invalid.moves[0].target.to_ascii_uppercase(),
            "cycle" => {
                invalid.moves[0].target = invalid.moves[1].source.clone();
                invalid.moves[1].target = invalid.moves[0].source.clone();
            }
            "ancestor" => {
                invalid.moves[1].target = format!("{}/nested.mjs", invalid.moves[0].target)
            }
            "phase" => invalid.phases[0].moves.clear(),
            "extension" => invalid.moves[0].target.push_str(".rs"),
            "shim" => invalid.shims[0].code = "invalid module".into(),
            _ => unreachable!(),
        }
        let json: serde_json::Value =
            serde_json::from_str(&output::format_as_json(&invalid)).expect("JSON");
        assert_eq!(json["execution"]["supported"], false, "{kind}");
        assert!(
            output::format_as_markdown(&invalid).contains("Execution withheld"),
            "{kind}"
        );
        let script = output::format_as_script(&invalid);
        assert!(script.contains("exit 2"), "{kind}");
        assert!(!script.contains("git mv"), "{kind}");
    }
}

#[test]
fn dependencies_runtime_expressions_and_cross_package_moves_are_unresolved() {
    let (root, snapshot) = fixture(&["packages/a/src/components/value.ts"]);
    std::fs::write(
        root.path().join("packages/a/package.json"),
        "{\"type\":\"module\"}",
    )
    .expect("nested package");
    write_fixture(
        root.path(),
        "packages/a/tsconfig.json",
        "{\"compilerOptions\":{\"module\":\"NodeNext\",\"rootDir\":\".\",\"outDir\":\"dist\"},\"include\":[\"**/*.ts\"]}",
    );
    let layout = HashMap::from([(Layer::UI, "ui".into())]);
    let crossing = generate_refactor_plan(&snapshot, ".", Some(&layout)).expect("analysis");
    assert!(crossing.moves.is_empty());
    assert!(crossing.unresolved[0].reason.contains("Package boundary"));
    for source in [
        "import x from './other.js'; export const value = x;",
        "export const value = import.meta.url;",
        "export const value = new URL('./asset.svg', import.meta.url);",
        "export function value() { return 42; }",
        "'use client'; export const value = 42;",
        "#!/usr/bin/env node\nexport const value = 42;",
    ] {
        std::fs::write(root.path().join(&snapshot.files[0].path), source)
            .expect("unsupported source");
        let plan =
            generate_refactor_plan(&snapshot, "packages/a/src", Some(&layout)).expect("analysis");
        assert!(plan.moves.is_empty(), "{source}");
        assert!(
            plan.unresolved[0]
                .reason
                .contains("Unsupported relocation contract")
        );
    }
}

#[test]
fn layout_traversal_and_duplicate_layer_keys_are_rejected() {
    for spec in [
        "ui=../outside",
        "ui=/tmp/outside",
        "ui=a/../../b",
        "ui=a,ui=b",
        "ui=a//b",
        "ui=.github",
        "ui=node_modules",
        "ui=dist",
        "ui=foo\\bar",
    ] {
        let parsed = parse_target_layout_spec(spec);
        if matches!(spec, "ui=.github" | "ui=node_modules" | "ui=dist") {
            let (_root, snapshot) = fixture(&["src/components/value.ts"]);
            assert!(
                generate_refactor_plan(&snapshot, "src", parsed.as_ref().ok())
                    .expect("analysis")
                    .moves
                    .is_empty()
            );
        } else {
            assert!(parsed.is_err(), "{spec}");
        }
    }
}

#[test]
fn absolute_target_resolves_against_snapshot_root_and_unknown_stays_unknown() {
    let (root, snapshot) = fixture(&["mystery.ts", "src/components/value.ts"]);
    let plan = generate_refactor_plan(&snapshot, root.path().to_str().expect("root"), None)
        .expect("absolute target");
    assert_eq!(plan.target, ".");
    assert_eq!(plan.stats.layer_before["Unknown"], 1);
    assert_eq!(plan.stats.layer_after["Unknown"], 1);
    assert!(plan.unresolved.iter().any(|c| c.source == "mystery.ts"
        && c.layer == Layer::Unknown
        && c.reason.contains("Unknown layer")));
}

#[test]
fn literal_typescript_shims_work_with_actual_esm_emission() {
    let (root, mut snapshot) = fixture(&["src/components/a/value.ts", "src/components/b/value.ts"]);
    for file in &snapshot.files {
        std::fs::write(
            root.path().join(&file.path),
            "export const value: number = 42;\n",
        )
        .expect("TypeScript source");
    }
    std::fs::write(root.path().join("consumer.ts"), "import { value as a } from './src/components/a/value.js';\nimport { value as b } from './src/components/b/value.js';\nif (a !== 42 || b !== 42) throw Error('TypeScript consumers broken');\n").expect("consumer");
    snapshot.edges.push(GraphEdge {
        from: "consumer.ts".into(),
        to: snapshot.files[0].path.clone(),
        label: "import".into(),
    });
    git_fixture(root.path());
    let emitter = "import fs from 'node:fs'; import path from 'node:path'; import { stripTypeScriptTypes } from 'node:module'; for (const f of fs.readdirSync('.', {recursive:true})) { if (!f.endsWith('.ts') || f.startsWith('emitted/')) continue; const dest = 'emitted/' + f.slice(0,-3) + '.js'; fs.mkdirSync(path.dirname(dest), {recursive:true}); fs.writeFileSync(dest, stripTypeScriptTypes(fs.readFileSync(f,'utf8'))); }";
    successful(run(
        root.path(),
        "node",
        &["--input-type=module", "-e", emitter],
    ));
    successful(run(root.path(), "node", &["emitted/consumer.js"]));
    let layout = HashMap::from([(Layer::UI, "ui".into())]);
    let plan = generate_refactor_plan(&snapshot, "src", Some(&layout)).expect("plan");
    assert_eq!(plan.moves.len(), 2);
    std::fs::write(
        root.path().join("refactor.sh"),
        output::format_as_script(&plan),
    )
    .expect("script");
    successful(run(root.path(), "bash", &["refactor.sh", "--dry"]));
    successful(run(root.path(), "bash", &["refactor.sh"]));
    successful(run(
        root.path(),
        "node",
        &["--input-type=module", "-e", emitter],
    ));
    successful(run(root.path(), "node", &["emitted/consumer.js"]));
    assert!(plan.moves.iter().all(|mv| mv.risk == RiskLevel::Low));
}

#[test]
fn filesystem_case_and_symlink_ancestors_withhold_moves() {
    let (root, snapshot) = fixture(&["src/components/value.mjs"]);
    std::fs::create_dir(root.path().join("src/UI")).expect("case-colliding directory");
    let layout = HashMap::from([(Layer::UI, "ui".into())]);
    let plan = generate_refactor_plan(&snapshot, "src", Some(&layout)).expect("analysis");
    assert!(plan.moves.is_empty());
    assert!(plan.unresolved[0].reason.contains("case-colliding"));

    #[cfg(unix)]
    {
        let layout = HashMap::from([(Layer::UI, "linked".into())]);
        std::os::unix::fs::symlink(
            root.path().join("src/components"),
            root.path().join("src/linked"),
        )
        .expect("symlink fixture");
        let plan = generate_refactor_plan(&snapshot, "src", Some(&layout)).expect("analysis");
        assert!(plan.moves.is_empty());
        assert!(plan.unresolved[0].reason.contains("Symlink"));
    }
}

fn write_fixture(root: &std::path::Path, path: &str, content: &str) {
    let target = root.join(path);
    std::fs::create_dir_all(target.parent().expect("parent")).expect("directories");
    std::fs::write(target, content).expect("fixture file");
}

fn cli_plan(root: &std::path::Path, extra: &[&str]) -> serde_json::Value {
    git_fixture(root);
    let output = successful(
        std::process::Command::new(env!("CARGO_BIN_EXE_loct"))
            .args(["plan", ".", "--json", "--quiet"])
            .args(extra)
            .current_dir(root)
            .output()
            .expect("CLI plan"),
    );
    serde_json::from_slice(&output.stdout).expect("CLI JSON")
}

#[test]
fn cli_preserves_real_rust_workspace_and_python_package_identity() {
    let root = tempfile::tempdir().expect("CLI fixture");
    write_fixture(
        root.path(),
        "Cargo.toml",
        "[workspace]\nmembers = ['crates/one', 'crates/two']\nresolver = '2'\n",
    );
    for name in ["one", "two"] {
        write_fixture(
            root.path(),
            &format!("crates/{name}/Cargo.toml"),
            &format!("[package]\nname = '{name}'\nversion = '0.1.0'\nedition = '2024'\n"),
        );
        write_fixture(
            root.path(),
            &format!("crates/{name}/src/lib.rs"),
            "pub mod utils;\npub use utils::value;\n",
        );
        write_fixture(
            root.path(),
            &format!("crates/{name}/src/utils.rs"),
            "pub fn value() -> u8 { 42 }\n",
        );
        write_fixture(
            root.path(),
            &format!("{name}/__init__.py"),
            "from .utils import value\n",
        );
        write_fixture(
            root.path(),
            &format!("{name}/__main__.py"),
            "from . import value\nassert value() == 42\n",
        );
        write_fixture(
            root.path(),
            &format!("{name}/utils.py"),
            "def value():\n    return 42\n",
        );
    }
    write_fixture(
        root.path(),
        "pyproject.toml",
        "[project]\nname = 'identity-fixture'\nversion = '0.1.0'\n[project.scripts]\none = 'one.utils:value'\n",
    );
    write_fixture(root.path(), ".github/FUNDING.yml", "github: example\n");
    successful(run(root.path(), "python3", &["-m", "one"]));
    successful(run(root.path(), "python3", &["-m", "two"]));
    let before = std::fs::read(root.path().join("Cargo.toml")).expect("manifest");
    for extra in [
        &[][..],
        &["--target-layout", "infra=infra,app=app,kernel=kernel"][..],
    ] {
        let plan = cli_plan(root.path(), extra);
        assert!(plan["moves"].as_array().expect("moves").is_empty());
        let unresolved = plan["unresolved"].as_array().expect("unresolved");
        for path in [
            "Cargo.toml",
            "crates/one/Cargo.toml",
            "crates/two/src/lib.rs",
            "one/__init__.py",
            "two/__init__.py",
            "one/__main__.py",
            "two/__main__.py",
            "pyproject.toml",
        ] {
            assert!(
                unresolved.iter().any(|c| c["source"] == path),
                "missing {path}: {plan}"
            );
        }
        assert_eq!(plan["execution"]["mode"], "analysis_only");
    }
    assert_eq!(
        std::fs::read(root.path().join("Cargo.toml")).expect("manifest"),
        before
    );
    successful(run(root.path(), "python3", &["-m", "one"]));
}

#[test]
fn cli_explicit_typescript_layout_preserves_namespaces_and_keeps_assets() {
    let root = tempfile::tempdir().expect("CLI fixture");
    write_fixture(
        root.path(),
        "package.json",
        "{\"name\":\"identity-fixture\",\"type\":\"module\"}",
    );
    write_fixture(
        root.path(),
        "tsconfig.json",
        "{\"compilerOptions\":{\"module\":\"NodeNext\",\"rootDir\":\".\",\"outDir\":\"dist\"},\"include\":[\"**/*.ts\"]}",
    );
    for name in ["one", "two"] {
        write_fixture(
            root.path(),
            &format!("src/components/{name}/value.ts"),
            "export const value: number = 42;\n",
        );
    }
    write_fixture(root.path(), "src/assets/logo.svg", "<svg></svg>\n");
    write_fixture(
        root.path(),
        "src/consumer.ts",
        "import { value } from './components/one/value.js';\nexport const answer = value;\n",
    );
    let default = cli_plan(root.path(), &[]);
    assert!(default["moves"].as_array().expect("moves").is_empty());
    let explicit = cli_plan(root.path(), &["--target-layout", "ui=ui,infra=infra"]);
    let moves = explicit["moves"].as_array().expect("moves");
    assert_eq!(moves.len(), 2, "{explicit}");
    assert_eq!(explicit["shims"].as_array().expect("shims").len(), 2);
    let targets: std::collections::HashSet<_> = moves
        .iter()
        .map(|mv| mv["target"].as_str().expect("target"))
        .collect();
    assert_eq!(targets.len(), 2);
    assert!(targets.contains("ui/src/components/one/value.ts"));
    assert!(targets.contains("ui/src/components/two/value.ts"));
    assert!(
        explicit["unresolved"]
            .as_array()
            .expect("unresolved")
            .iter()
            .any(|c| c["source"] == "package.json")
    );
    assert!(root.path().join("src/assets/logo.svg").exists());
}

#[test]
fn typescript_compiler_contract_must_cover_the_destination() {
    let (root, snapshot) = fixture(&["src/components/value.ts"]);
    let layout = HashMap::from([(Layer::UI, "ui".into())]);
    for config in [
        "{\"compilerOptions\":{\"noEmit\":true,\"module\":\"NodeNext\",\"rootDir\":\".\",\"outDir\":\"dist\"},\"include\":[\"**/*.ts\"]}",
        "{\"compilerOptions\":{\"module\":\"NodeNext\",\"rootDir\":\"src/components\",\"outDir\":\"dist\"},\"include\":[\"src/components/**/*.ts\"]}",
        "{\"extends\":\"./base.json\",\"compilerOptions\":{\"module\":\"NodeNext\",\"rootDir\":\".\",\"outDir\":\"dist\"},\"include\":[\"**/*.ts\"]}",
        "{\"compilerOptions\":{\"module\":\"NodeNext\",\"rootDir\":\".\",\"outDir\":\"dist\"},\"include\":[\"**/*.ts\"],\"exclude\":[\"src/ui/**\"]}",
    ] {
        write_fixture(root.path(), "tsconfig.json", config);
        let plan = generate_refactor_plan(&snapshot, "src", Some(&layout)).expect("analysis");
        assert!(plan.moves.is_empty(), "{config}");
        assert!(
            plan.unresolved[0]
                .reason
                .contains("Unsupported TypeScript contract")
        );
    }
}

#[test]
fn changed_manifest_contract_rejects_script_before_mutation() {
    let (root, plan) = script_fixture();
    write_fixture(root.path(), "package.json", "{\"type\":\"commonjs\"}");
    let failed = run(root.path(), "bash", &["refactor.sh"]);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("contract changed"));
    assert!(!root.path().join(&plan.moves[0].target).exists());
}

#[test]
fn runtime_new_compiler_scope_is_rejected_before_any_move() {
    let (root, snapshot) = fixture(&["src/components/value.ts"]);
    git_fixture(root.path());
    let layout = HashMap::from([(Layer::UI, "ui".into())]);
    let plan = generate_refactor_plan(&snapshot, "src", Some(&layout)).expect("plan");
    assert_eq!(plan.moves.len(), 1);
    write_fixture(root.path(), "refactor.sh", &output::format_as_script(&plan));
    write_fixture(
        root.path(),
        "src/ui/tsconfig.json",
        "{\"compilerOptions\":{\"noEmit\":true}}",
    );
    let result = run(root.path(), "bash", &["refactor.sh"]);
    assert!(
        !result.status.success(),
        "new compiler scope must invalidate the old contract"
    );
    assert!(!root.path().join(&plan.moves[0].target).exists());
}
