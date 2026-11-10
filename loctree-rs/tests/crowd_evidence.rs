use loctree::analyzer::crowd::{CrowdIssue, detect_crowd_with_edges, format_crowd};
use loctree::snapshot::GraphEdge;
use loctree::types::{ExportSymbol, FileAnalysis};

fn file(path: &str, exports: &[&str]) -> FileAnalysis {
    FileAnalysis {
        path: path.to_string(),
        exports: exports
            .iter()
            .map(|name| ExportSymbol {
                name: name.to_string(),
                kind: "function".to_string(),
                export_type: "named".to_string(),
                line: Some(1),
                params: vec![],
                symbol_id: Default::default(),
            })
            .collect(),
        ..Default::default()
    }
}

fn imports(target: &str, count: usize) -> Vec<GraphEdge> {
    (0..count)
        .map(|n| GraphEdge {
            from: format!("consumers/{n}.ts"),
            to: target.to_string(),
            label: "import".to_string(),
        })
        .collect()
}

#[test]
fn specialized_roles_keep_members_and_counts_without_inventing_issues() {
    // Three independent domains; most files match by export, not by filename.
    for (seed, paths, names) in [
        (
            "routes",
            ["src/options.rs", "src/analysis.rs", "src/rust_routes.rs"],
            ["routes_options", "analyze_routes", "rust_routes"],
        ),
        (
            "transcribe",
            ["src/cli.py", "src/transcribe_types.py", "src/xai.py"],
            ["transcribe_cli", "TranscribeResult", "transcribe_audio"],
        ),
        (
            "glyph",
            ["ui/Glyph.tsx", "fonts/catalog.ts", "tools/render.ts"],
            ["Glyph", "glyphCatalog", "renderGlyph"],
        ),
    ] {
        let files: Vec<_> = paths
            .iter()
            .zip(names)
            .map(|(p, n)| file(p, &[n]))
            .collect();
        let edges = imports(paths[0], 9);
        let crowd = detect_crowd_with_edges(&files, seed, &edges);
        assert_eq!(crowd.members.len(), 3, "domain recall: {seed}");
        assert_eq!(crowd.members[0].file, paths[0]);
        assert_eq!(crowd.members[0].importer_count, 9);
        assert_eq!(crowd.members[0].importer_count_direct, 9);
        assert!(crowd.issues.is_empty(), "{seed}: {:?}", crowd.issues);
        assert_eq!(crowd.score, 0.0, "membership is not a problem: {seed}");
    }
}

#[test]
fn shared_dependencies_are_not_shared_exports() {
    let mut files = vec![
        file("glyph/view.ts", &["GlyphView"]),
        file("glyph/measure.ts", &["measureGlyph"]),
    ];
    for file in &mut files {
        file.imports = serde_json::from_value(serde_json::json!([
            {"source": "react", "source_raw": "react", "kind": "Static", "symbols": [],
             "is_bare": true, "resolution": "Unknown", "is_type_checking": false},
            {"source": "./theme", "source_raw": "./theme", "kind": "Static", "symbols": [],
             "is_bare": false, "resolution": "Local", "is_type_checking": false}
        ]))
        .expect("import fixture");
    }
    let crowd = detect_crowd_with_edges(&files, "glyph", &[]);
    assert_eq!(crowd.members[0].similarity_scores[0].1, 1.0);
    assert!(crowd.issues.is_empty(), "{:?}", crowd.issues);
    assert_eq!(crowd.score, 0.0);
}

#[test]
fn two_namesakes_are_candidates_with_full_identity_not_deletion_advice() {
    let files = vec![
        file("platform/linux/route.rs", &["linux_route"]),
        file("platform/windows/route.rs", &["windows_route"]),
        file("platform/routes.rs", &["routes"]),
    ];
    let crowd = detect_crowd_with_edges(&files, "route", &imports(&files[0].path, 6));
    let json = serde_json::to_value(&crowd).expect("crowd JSON");
    let collisions: Vec<_> = json["issues"]
        .as_array()
        .expect("issues")
        .iter()
        .filter(|issue| issue["type"] == "name_collision")
        .collect();
    assert_eq!(collisions.len(), 1);
    assert_eq!(
        collisions[0]["files"],
        serde_json::json!(["platform/linux/route.rs", "platform/windows/route.rs"])
    );
    assert!(crowd.issues.iter().any(|issue| matches!(issue,
        CrowdIssue::UsageAsymmetry { primary, underused }
        if primary == &files[0].path && underused == &vec![files[1].path.clone()]
    )));
    let rendered = format_crowd(&crowd, false);
    assert!(rendered.contains("candidate"), "{rendered}");
    assert!(!rendered.contains("is primary"), "{rendered}");
    assert!(!rendered.contains("redundant"), "{rendered}");
    assert!(crowd.score > 0.0);

    let pair = detect_crowd_with_edges(&files[..2], "route", &[]);
    assert!(
        pair.issues
            .iter()
            .any(|i| matches!(i, CrowdIssue::NameCollision { .. }))
    );
    assert!(
        !pair
            .issues
            .iter()
            .any(|i| matches!(i, CrowdIssue::UsageAsymmetry { .. })),
        "zero/zero usage is not asymmetry"
    );
}

#[test]
fn actual_export_overlap_carries_only_exact_shared_names_and_paths() {
    let files = vec![
        file("glyph/a.ts", &["renderGlyph", "Glyph", "renderGlyph"]),
        file("glyph/b.ts", &["Glyph", "renderGlyph", "unrelated"]),
        file("glyph/c.ts", &["otherGlyph"]),
    ];
    let crowd = detect_crowd_with_edges(&files, "glyph", &imports(&files[0].path, 6));
    let overlaps: Vec<_> = crowd
        .issues
        .iter()
        .filter_map(|i| match i {
            CrowdIssue::ExportOverlap { files, overlap } => Some((files, overlap)),
            _ => None,
        })
        .collect();
    assert_eq!(
        overlaps,
        vec![(
            &vec!["glyph/a.ts".to_string(), "glyph/b.ts".to_string()],
            &vec!["Glyph".to_string(), "renderGlyph".to_string()]
        )]
    );
    assert!(crowd.issues.iter().any(|i| matches!(i,
        CrowdIssue::UsageAsymmetry { underused, .. } if underused == &vec![files[1].path.clone()]
    )));
    assert!(format_crowd(&crowd, false).contains("renderGlyph"));
}

#[test]
fn exact_case_sensitive_stems_do_not_collapse_variants_or_pseudo_exports() {
    let files = vec![
        file("glyph/Route.ts", &["default", "*"]),
        file("glyph/route.ts", &["default", "*"]),
        file("glyph/route.test.ts", &[]),
        file("glyph/route_linux.ts", &[]),
    ];
    let crowd = detect_crowd_with_edges(&files, "glyph", &imports(&files[0].path, 9));
    assert!(crowd.issues.is_empty(), "{:?}", crowd.issues);
}

#[test]
fn crowd_evidence_is_deterministic_under_input_permutation() {
    let mut files = vec![
        file("glyph/z/item.ts", &["Glyph"]),
        file("glyph/a/item.ts", &["Glyph"]),
        file("glyph/m/other.ts", &["OtherGlyph"]),
    ];
    let first = detect_crowd_with_edges(&files, "glyph", &[]);
    files.reverse();
    let second = detect_crowd_with_edges(&files, "glyph", &[]);
    assert_eq!(
        serde_json::to_value(first).expect("first"),
        serde_json::to_value(second).expect("second")
    );
}

#[test]
fn issue_wire_compatibility_and_mcp_debug_keep_qualified_evidence() {
    let legacy = serde_json::json!({
        "type": "export_overlap", "files": ["a/glyph.ts", "b/glyph.ts"],
        "overlap": ["Glyph"]
    });
    let issue: CrowdIssue = serde_json::from_value(legacy.clone()).expect("legacy issue");
    let mut encoded = serde_json::to_value(&issue).expect("qualified issue");
    assert!(
        encoded["qualification"]
            .as_str()
            .expect("qualification")
            .contains("candidate")
    );
    encoded
        .as_object_mut()
        .expect("object")
        .remove("qualification");
    assert_eq!(encoded, legacy, "existing fields and variant stay intact");
    let debug = format!("{issue:?}");
    assert!(debug.contains("candidate") && debug.contains("Glyph") && debug.contains("a/glyph.ts"));
}

#[test]
fn unrelated_members_do_not_inflate_an_evidenced_concern_score() {
    let mut files = vec![
        file("glyph/a.ts", &["Glyph"]),
        file("glyph/b.ts", &["Glyph"]),
    ];
    let edges = imports("glyph/a.ts", 6);
    let pair = detect_crowd_with_edges(&files, "glyph", &edges);
    for n in 0..10 {
        files.push(file(
            &format!("glyph/role{n}.ts"),
            &[&format!("glyphRole{n}")],
        ));
    }
    let expanded = detect_crowd_with_edges(&files, "glyph", &edges);
    assert_eq!(expanded.members.len(), 12);
    assert_eq!(expanded.score, pair.score);
    assert_eq!(
        serde_json::to_value(expanded.issues).expect("expanded"),
        serde_json::to_value(pair.issues).expect("pair")
    );
}

#[test]
fn cli_parses_distinct_role_fixtures_across_three_languages() {
    for (extension, names, sources, consumer) in [
        (
            "rs",
            ["glyph_view", "glyph_measure", "glyph_cli"],
            [
                "pub fn glyph_view() {}",
                "pub fn glyph_measure() {}",
                "pub fn glyph_cli() {}",
            ],
            "use crate::glyph_view::glyph_view; pub fn run() { glyph_view(); }",
        ),
        (
            "py",
            ["glyph_view", "glyph_measure", "glyph_cli"],
            [
                "def glyph_view(): pass",
                "def glyph_measure(): pass",
                "def glyph_cli(): pass",
            ],
            "from glyph_view import glyph_view\nglyph_view()",
        ),
        (
            "ts",
            ["glyph_view", "glyph_measure", "glyph_cli"],
            [
                "import { version } from './dependency'; export const glyphView = version;",
                "import { version } from './dependency'; export const glyphMeasure = version;",
                "import { version } from './dependency'; export const glyphCli = version;",
            ],
            "import { glyphView } from './glyph_view'; console.log(glyphView);",
        ),
    ] {
        let root = tempfile::tempdir().expect("fixture root");
        let cache = tempfile::tempdir().expect("private cache");
        git2::Repository::init(root.path()).expect("git fixture");
        for (name, source) in names.into_iter().zip(sources) {
            std::fs::write(root.path().join(format!("{name}.{extension}")), source)
                .expect("source");
        }
        if extension == "ts" {
            std::fs::write(
                root.path().join("dependency.ts"),
                "export const version = 1;",
            )
            .expect("dependency");
        }
        for n in 0..6 {
            std::fs::write(
                root.path().join(format!("consumer{n}.{extension}")),
                consumer,
            )
            .expect("consumer");
        }
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_loct"))
            .current_dir(root.path())
            .env("LOCT_CACHE_DIR", cache.path())
            .env("LOCTREE_NO_PROFILE_UPDATE", "1")
            .env("LOCT_NO_GITIGNORE", "1")
            .args(["crowd", "glyph", "--json"])
            .output()
            .expect("CLI");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("crowd JSON");
        let crowd = &value[0];
        assert_eq!(
            crowd["members"].as_array().expect("members").len(),
            3,
            "{extension}: {value}"
        );
        assert_eq!(
            crowd["issues"],
            serde_json::json!([]),
            "{extension}: {value}"
        );
        assert_eq!(crowd["score"], 0.0, "{extension}: {value}");
        assert!(
            crowd["members"]
                .as_array()
                .expect("members")
                .iter()
                .any(|member| member["importer_count"].as_u64().expect("count") >= 3),
            "fan-in is preserved: {extension}: {value}"
        );
    }
}
