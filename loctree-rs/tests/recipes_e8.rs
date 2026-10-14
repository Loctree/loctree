//! Integration tests for the recipes-round E8 findings (H1-H11, minus
//! python.rs), exercised through the crate's PUBLIC API surface.
//!
//! These complement the in-file unit tests added alongside each fix (in
//! `src/analyzer/ast_js/calls.rs`, `src/analyzer/ast_js/mod.rs`,
//! `src/analyzer/html_analyzer.rs`, `src/analyzer/regexes.rs`,
//! `src/analyzer/coverage_gaps.rs`, `src/analyzer/test_coverage.rs`,
//! `src/analyzer/env_truth/discovery.rs`, `src/analyzer/env_truth/
//! warnings.rs`, `src/layoutmap.rs`, `src/cli/dispatch/handlers/prism.rs`,
//! `src/semantic/make.rs`, `src/semantic/rust_routes.rs`, and
//! `src/analyzer/insights.rs`).
//!
//! Findings H1, H2, and H3(b) live in `analyzer::ast_js`, whose
//! `analyze_js_file_ast` entry point is `pub(crate)` — not reachable from
//! here. Finding H3(c) lives in `analyzer::html_analyzer`, which is a
//! `pub(crate) mod` — the whole module is unreachable from an external
//! integration test, even though its individual functions are `pub`. Those
//! three findings are covered exclusively by their in-file unit tests.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use std::fs;
use std::path::{Path, PathBuf};

use loctree::analyzer::coverage_gaps::find_coverage_gaps_fenced;
use loctree::analyzer::env_truth::{ComputeConfig, compute_env_truth};
use loctree::analyzer::insights::collect_ai_insights;
use loctree::analyzer::makefile::analyze_makefile;
use loctree::analyzer::test_coverage::analyze_test_coverage;
use loctree::cli::command::LayoutmapOptions;
use loctree::layoutmap::{LayoutFinding, scan_css_layout};
use loctree::semantic::{IdiomRegistry, MakeSemantics, RuntimeSemanticAnalyzer, SemanticFacts};
use loctree::snapshot::{EventBridge, Snapshot};
use loctree::types::{ExportSymbol, FileAnalysis, ReexportEntry, ReexportKind};

fn fixture_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

/// H4(d): a vendored/minified evidence location riding along a real
/// product-code emit under the same (bare-name-merged) event bridge must
/// not survive as evidence once the artifact fence runs. Exercises the
/// full `find_coverage_gaps_fenced` pipeline through the public `Snapshot`
/// API (the isolated `find_event_gaps` unit test lives in
/// `analyzer/coverage_gaps.rs`).
#[test]
fn h4d_event_gap_fence_strips_vendor_evidence_end_to_end() {
    let mut snapshot = Snapshot::new(vec![".".to_string()]);
    snapshot.event_bridges = vec![EventBridge {
        name: "update".to_string(),
        emits: vec![
            (
                "loctree-rs/src/analyzer/assets/jszip.min.js".to_string(),
                5,
                "emit_const".to_string(),
            ),
            ("src/app.ts".to_string(), 12, "emit_const".to_string()),
        ],
        listens: vec![],
        is_fe_sync: false,
        same_file_sync: false,
    }];

    let (gaps, _fence) = find_coverage_gaps_fenced(&snapshot, false);
    let gap = gaps
        .iter()
        .find(|g| g.target == "update")
        .expect("product-code event gap must survive the fence");
    assert!(
        !gap.files.iter().any(|f| f.contains("min.js")),
        "vendored evidence must not ride along, got: {:?}",
        gap.files
    );
    assert!(gap.files.iter().any(|f| f == "src/app.ts"));
}

/// H4(b) + H4(c): a stdlib `pub use` reexport must not count as an
/// untested export, and a file with an inline `#[cfg(test)]` module must
/// credit its own file's coverage - both exercised together through the
/// public `analyze_test_coverage` entry point.
#[test]
fn h4bc_test_coverage_handles_stdlib_reexport_and_inline_tests() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_path = tmp.path().join("lib.rs");
    fs::write(
        &lib_path,
        r#"
pub fn add(a: i32, b: i32) -> i32 { a + b }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds() { assert_eq!(add(1, 2), 3); }
}
"#,
    )
    .unwrap();

    let mut inline_tested_file = FileAnalysis::new(lib_path.to_string_lossy().to_string());
    inline_tested_file.kind = "code".to_string();
    inline_tested_file.language = "rs".to_string();
    inline_tested_file.exports = vec![ExportSymbol::new(
        "add".to_string(),
        "function",
        "named",
        Some(2),
    )];

    let mut reexport_file = FileAnalysis::new("src/util.rs".to_string());
    reexport_file.kind = "code".to_string();
    reexport_file.language = "rs".to_string();
    reexport_file.exports = vec![ExportSymbol::new(
        "Command".to_string(),
        "reexport",
        "named",
        None,
    )];
    reexport_file.reexports.push(ReexportEntry {
        source: "std::process::Command".to_string(),
        kind: ReexportKind::Named(vec![("Command".to_string(), "Command".to_string())]),
        resolved: None,
    });

    let mut snapshot = Snapshot::new(vec![tmp.path().to_string_lossy().to_string()]);
    snapshot.files = vec![inline_tested_file, reexport_file];

    let report = analyze_test_coverage(&snapshot);
    assert_eq!(
        report.exports_without_tests.len(),
        0,
        "neither the inline-tested export nor the stdlib reexport should be flagged, got: {:?}",
        report.exports_without_tests
    );
}

/// H5: `<prefix>.env.example` naming conventions (e.g. `s4.env.example`,
/// see `tests/fixtures/recipes_e8_env/`) must be discovered as a dotenv
/// TEMPLATE, not silently invisible to env-truth. Proven via template_drift:
/// `S4_API_KEY` is declared only in the template, never in the fixture's
/// live `.env` - if the template file were never discovered at all, no
/// template_drift entry would exist for it either.
#[test]
fn h5_prefixed_env_example_fixture_is_discovered_as_template() {
    let root = fixture_path("recipes_e8_env");
    let cfg = ComputeConfig {
        roots: vec![root],
        ..ComputeConfig::default()
    };
    let report = compute_env_truth(&cfg, None);

    let drift = report
        .template_drift
        .iter()
        .find(|d| d.template_path.ends_with("s4.env.example"))
        .unwrap_or_else(|| {
            panic!(
                "s4.env.example must be discovered and tracked as a template, drift = {:?}",
                report.template_drift
            )
        });
    assert!(
        drift.missing_in_live.contains(&"S4_API_KEY".to_string()),
        "S4_API_KEY (template-only) must show as missing-in-live, got: {drift:?}"
    );
}

/// H6: a Svelte component's `<style>` block (see
/// `tests/fixtures/recipes_e8_svelte/Hero.svelte`) must be scanned for
/// layout properties - the old css/js-only extension lists made
/// Svelte/Astro-only design systems invisible to `loct layoutmap`.
#[test]
fn h6_svelte_style_block_layout_findings_via_fixture() {
    let root = fixture_path("recipes_e8_svelte");
    let opts = LayoutmapOptions::default();
    let findings = scan_css_layout(&root, &opts).expect("scan_css_layout must succeed");

    assert!(
        findings
            .iter()
            .any(|f| matches!(f, LayoutFinding::ZIndex { z_index: 42, .. })),
        "z-index inside the fixture's .svelte <style> block must be recorded, found: {findings:?}"
    );
    assert!(
        findings
            .iter()
            .any(|f| matches!(f, LayoutFinding::Grid { .. })),
        "display: grid inside the fixture's .svelte <style> block must be recorded, found: {findings:?}"
    );
}

/// H8: a Makefile recipe calling `tools/gen.py` (see
/// `tests/fixtures/recipes_e8_make/`, where the script actually exists)
/// must resolve `handler_file` via the public `MakeSemantics` /
/// `analyze_makefile` API, instead of the old hardcoded `None`.
#[test]
fn h8_make_recipe_resolves_handler_file_via_fixture_script() {
    let root = fixture_path("recipes_e8_make");
    let makefile_path = root.join("Makefile");
    let content = fs::read_to_string(&makefile_path).expect("read fixture Makefile");

    let mut makefile = analyze_makefile(&content, makefile_path.to_string_lossy().to_string());
    makefile.language = "make".to_string();

    let gen_py = FileAnalysis::new("tools/gen.py".to_string());

    let registry = IdiomRegistry::load_defaults().expect("idiom registry defaults");
    let mut facts = SemanticFacts::default();
    MakeSemantics
        .analyze(&[makefile, gen_py], &registry, &mut facts)
        .expect("MakeSemantics::analyze must succeed");

    let edge = facts
        .dispatch_edges
        .iter()
        .find(|e| e.handler_symbol == "gen.py")
        .expect("gen.py dispatch edge must be present");
    assert_eq!(
        edge.handler_file.as_deref(),
        Some("tools/gen.py"),
        "handler_file must resolve to the fixture's tools/gen.py, got: {:?}",
        edge.handler_file
    );
}

/// H11(a) + H11(b): a pure-Rust snapshot with 25 re-export cascades must
/// not get a JS/TS-only "tree-shaking/bundling" insight, and generic
/// cross-language stems (`version`, `config`, `cli`, `app`, `settings`)
/// must not surface as false binding-pair insights - exercised together
/// through the public `collect_ai_insights` entry point.
#[test]
fn h11ab_insights_gate_tree_shaking_and_ignore_generic_stems() {
    let files = vec![
        FileAnalysis {
            path: "src/lib.rs".to_string(),
            language: "rs".to_string(),
            ..Default::default()
        },
        FileAnalysis {
            path: "src/version.rs".to_string(),
            language: "rs".to_string(),
            ..Default::default()
        },
        FileAnalysis {
            path: "scripts/version.py".to_string(),
            language: "py".to_string(),
            ..Default::default()
        },
    ];
    let cascades: Vec<(String, String)> = (0..25)
        .map(|i| (format!("mod_{i}"), format!("mod_{i}_target")))
        .collect();

    let insights = collect_ai_insights(&files, &[], &cascades, &[], &[]);
    let titles: Vec<&str> = insights.iter().map(|i| i.title.as_str()).collect();

    assert!(
        !insights
            .iter()
            .any(|i| i.title.contains("re-export chains")),
        "pure-Rust snapshot must not get a tree-shaking/bundling insight, got: {titles:?}"
    );
    assert!(
        !insights
            .iter()
            .any(|i| i.title.contains("cross-language binding")),
        "generic `version` stem must not surface a cross-language binding insight, got: {titles:?}"
    );
}
