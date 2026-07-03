//! Library-facing analysis reports shared by MCP, CLI-adjacent callers, and tests.
//!
//! This module intentionally stays out of `cli::*` so non-CLI surfaces can
//! expose health, findings, audit, and coverage without importing parser or
//! terminal dispatch code.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::analyzer::audit_report::{AuditFindings, OrphanFile, ShadowExport};
use crate::analyzer::coverage_gaps::{CoverageGap, GapKind, Severity, find_coverage_gaps};
use crate::analyzer::crowd::detect_all_crowds;
use crate::analyzer::cycles::{CycleCompilability, find_cycles_classified_with_lazy};
use crate::analyzer::dead_parrots::{DeadFilterConfig, find_dead_exports};
use crate::analyzer::findings::{Findings, FindingsConfig, FindingsSummary};
use crate::analyzer::root_scan::scan_results_from_snapshot;
use crate::analyzer::test_coverage::{TestCoverageReport, analyze_test_coverage};
use crate::analyzer::twins::{build_symbol_registry, detect_exact_twins};
use crate::snapshot::Snapshot;

#[derive(Debug, Clone, Default)]
pub struct HealthReportOptions {
    pub include_tests: bool,
    pub library_mode: bool,
    pub python_library: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    pub cycles: HealthCycleSummary,
    pub dead_exports: HealthDeadSummary,
    pub twins: HealthTwinSummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthCycleSummary {
    pub total: usize,
    pub high_risk: usize,
    pub structural: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthDeadSummary {
    pub total: usize,
    pub high_confidence: usize,
    pub low_confidence: usize,
    pub top_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthTwinSummary {
    pub total: usize,
    pub top_groups: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FindingsReportOptions {
    pub high_confidence: bool,
    pub library_mode: bool,
    pub python_library: bool,
    pub example_globs: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AuditReportOptions {
    pub include_tests: bool,
    pub library_mode: bool,
    pub python_library: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CoverageReportOptions {
    pub include_gaps: bool,
    pub include_tests: bool,
    pub handlers_only: bool,
    pub events_only: bool,
    pub min_severity: Option<Severity>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoverageReport {
    pub gaps: Vec<CoverageGap>,
    pub tests: Option<TestCoverageReport>,
}

pub fn health_report(
    snapshot: &Snapshot,
    root: &Path,
    options: HealthReportOptions,
) -> HealthReport {
    let edges: Vec<(String, String, String)> = snapshot
        .edges
        .iter()
        .map(|edge| (edge.from.clone(), edge.to.clone(), edge.label.clone()))
        .collect();
    let (classified_cycles, _) = find_cycles_classified_with_lazy(&edges);

    let high_risk = classified_cycles
        .iter()
        .filter(|cycle| cycle.compilability == CycleCompilability::Breaking)
        .count();
    let structural = classified_cycles
        .iter()
        .filter(|cycle| cycle.compilability == CycleCompilability::Structural)
        .count();

    let dead_exports = find_dead_exports(
        &snapshot.files,
        false,
        None,
        DeadFilterConfig {
            include_tests: options.include_tests,
            include_helpers: false,
            library_mode: options.library_mode,
            example_globs: Vec::new(),
            python_library_mode: options.python_library,
            include_ambient: false,
            include_dynamic: false,
            dead_ok_globs: crate::fs_utils::load_loctignore_dead_ok_globs(root),
        },
    );
    let high_confidence = dead_exports
        .iter()
        .filter(|dead| dead.confidence == "high")
        .count();
    let low_confidence = dead_exports.len().saturating_sub(high_confidence);

    let mut dead_by_file: HashMap<String, usize> = HashMap::new();
    for dead in &dead_exports {
        *dead_by_file.entry(dead.file.clone()).or_insert(0) += 1;
    }
    let mut top_dead_files: Vec<(String, usize)> = dead_by_file.into_iter().collect();
    top_dead_files.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let top_files = top_dead_files
        .into_iter()
        .take(3)
        .map(|(path, count)| {
            let display_name = Path::new(&path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path.as_str());
            format!("{display_name} ({count} dead)")
        })
        .collect();

    let twins = detect_exact_twins(&snapshot.files, options.include_tests);
    let mut twin_examples: Vec<(String, usize)> = twins
        .iter()
        .map(|twin| {
            let file_count = twin
                .locations
                .iter()
                .map(|loc| loc.file_path.as_str())
                .collect::<HashSet<_>>()
                .len();
            (twin.name.clone(), file_count)
        })
        .collect();
    twin_examples.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let top_groups = twin_examples
        .into_iter()
        .take(3)
        .map(|(name, file_count)| format!("{name} ({file_count} files)"))
        .collect();

    HealthReport {
        cycles: HealthCycleSummary {
            total: classified_cycles.len(),
            high_risk,
            structural,
        },
        dead_exports: HealthDeadSummary {
            total: dead_exports.len(),
            high_confidence,
            low_confidence,
            top_files,
        },
        twins: HealthTwinSummary {
            total: twins.len(),
            top_groups,
        },
    }
}

pub fn findings_report(snapshot: &Snapshot, options: FindingsReportOptions) -> Findings {
    let scan_results = scan_results_from_snapshot(snapshot);
    Findings::produce(
        &scan_results,
        snapshot,
        FindingsConfig {
            high_confidence: options.high_confidence,
            library_mode: options.library_mode,
            python_library: options.python_library,
            example_globs: options.example_globs,
        },
        None,
    )
}

pub fn findings_summary_report(
    snapshot: &Snapshot,
    options: FindingsReportOptions,
) -> FindingsSummary {
    findings_report(snapshot, options).summary_only()
}

pub fn audit_findings(
    snapshot: &Snapshot,
    root: &Path,
    options: AuditReportOptions,
) -> AuditFindings {
    let edges: Vec<(String, String, String)> = snapshot
        .edges
        .iter()
        .map(|edge| (edge.from.clone(), edge.to.clone(), edge.label.clone()))
        .collect();
    let (classified_cycles, _) = find_cycles_classified_with_lazy(&edges);

    // Canonical dead pipeline — audit consumes the same candidates (with
    // cross-check evidence and entry-point fence) as every other surface.
    let dead_exports = crate::analyzer::dead_parrots::compute_dead_truth_with(
        snapshot,
        DeadFilterConfig {
            include_tests: options.include_tests,
            include_helpers: false,
            library_mode: options.library_mode,
            example_globs: Vec::new(),
            python_library_mode: options.python_library,
            include_ambient: false,
            include_dynamic: false,
            dead_ok_globs: crate::fs_utils::load_loctignore_dead_ok_globs(root),
        },
        false,
    )
    .dead;

    let twins = detect_exact_twins(&snapshot.files, options.include_tests);

    let mut in_degree: HashMap<String, usize> = HashMap::new();
    for file in &snapshot.files {
        in_degree.insert(file.path.clone(), 0);
    }
    for edge in &snapshot.edges {
        *in_degree.entry(edge.to.clone()).or_insert(0) += 1;
    }

    let mut orphan_files: Vec<(String, usize)> = in_degree
        .iter()
        .filter(|(path, count)| {
            **count == 0
                && !is_entry_point(path)
                && (options.include_tests || !is_test_file_path(path))
        })
        .map(|(path, _)| {
            let loc = snapshot
                .files
                .iter()
                .find(|file| &file.path == path)
                .map(|file| file.loc)
                .unwrap_or(0);
            (path.clone(), loc)
        })
        .collect();
    orphan_files.sort_by_key(|(_, loc)| std::cmp::Reverse(*loc));

    // Artifact fence: generated files, lockfiles, vendored code, fixtures and
    // docs are not actionable "orphans to review" — separate, don't drop.
    let (artifact_orphans, orphan_files): (Vec<_>, Vec<_>) =
        orphan_files.into_iter().partition(|(path, _)| {
            crate::analyzer::classify::artifact_class(path, None).is_artifact()
                || crate::analyzer::classify::resource_kind(path) == Some("doc")
        });

    // Entry-point fence: runtime entries (Cargo [[bin]], package.json
    // main/bin, shebang scripts, detected main markers) legitimately have no
    // importers — they are roots, not orphans to review.
    let runtime_entries =
        crate::analyzer::dead_parrots::filters::runtime_entrypoint_paths(snapshot);
    let (entrypoint_orphans, orphan_files): (Vec<_>, Vec<_>) =
        orphan_files.into_iter().partition(|(path, _)| {
            runtime_entries.contains(path.replace('\\', "/").trim_start_matches("./"))
        });

    let registry = build_symbol_registry(&snapshot.files, options.include_tests);
    let mut shadow_exports: Vec<(String, usize, usize)> = Vec::new();
    for twin in &twins {
        let mut total_locations = 0;
        let mut dead_locations = 0;
        for loc in &twin.locations {
            total_locations += 1;
            let key = (loc.file_path.clone(), twin.name.clone());
            if let Some(entry) = registry.get(&key)
                && entry.import_count == 0
            {
                dead_locations += 1;
            }
        }
        if dead_locations > 0 && dead_locations < total_locations {
            shadow_exports.push((twin.name.clone(), total_locations, dead_locations));
        }
    }

    AuditFindings {
        cycles: classified_cycles,
        dead_exports,
        twins,
        orphan_files: orphan_files
            .into_iter()
            .map(|(path, loc)| OrphanFile { path, loc })
            .collect(),
        artifact_orphans: artifact_orphans
            .into_iter()
            .map(|(path, loc)| OrphanFile { path, loc })
            .collect(),
        entrypoint_orphans: entrypoint_orphans
            .into_iter()
            .map(|(path, loc)| OrphanFile { path, loc })
            .collect(),
        shadow_exports: shadow_exports
            .into_iter()
            .map(|(name, total_locations, dead_locations)| ShadowExport {
                name,
                total_locations,
                dead_locations,
            })
            .collect(),
        crowds: detect_all_crowds(&snapshot.files),
        total_files: snapshot.files.len(),
        total_loc: snapshot.files.iter().map(|file| file.loc).sum(),
    }
}

pub fn audit_json_report(findings: &AuditFindings, limit: Option<usize>) -> Value {
    let high_confidence = findings
        .dead_exports
        .iter()
        .filter(|dead| dead.confidence == "high")
        .count();
    let low_confidence = findings.dead_exports.len().saturating_sub(high_confidence);
    let high_risk_cycles = findings
        .cycles
        .iter()
        .filter(|cycle| cycle.compilability == CycleCompilability::Breaking)
        .count();
    let structural_cycles = findings
        .cycles
        .iter()
        .filter(|cycle| cycle.compilability == CycleCompilability::Structural)
        .count();
    let orphan_loc: usize = findings.orphan_files.iter().map(|file| file.loc).sum();

    let mut cycles = Map::new();
    cycles.insert("total".to_string(), json!(findings.cycles.len()));
    cycles.insert("high_risk".to_string(), json!(high_risk_cycles));
    cycles.insert("structural".to_string(), json!(structural_cycles));
    insert_audit_collection(&mut cycles, "items", &findings.cycles, limit);

    let mut dead_exports = Map::new();
    dead_exports.insert("total".to_string(), json!(findings.dead_exports.len()));
    dead_exports.insert("high_confidence".to_string(), json!(high_confidence));
    dead_exports.insert("low_confidence".to_string(), json!(low_confidence));
    insert_audit_collection(&mut dead_exports, "items", &findings.dead_exports, limit);

    let mut twins = Map::new();
    twins.insert("total".to_string(), json!(findings.twins.len()));
    insert_audit_collection(&mut twins, "groups", &findings.twins, limit);

    let mut orphan_files = Map::new();
    orphan_files.insert("total".to_string(), json!(findings.orphan_files.len()));
    orphan_files.insert("total_loc".to_string(), json!(orphan_loc));
    insert_audit_collection(&mut orphan_files, "files", &findings.orphan_files, limit);

    // Artifact fence: non-actionable orphans (generated/lockfiles/vendored/
    // fixtures/docs) reported separately — extracted, never silently dropped.
    let mut artifact_orphans = Map::new();
    artifact_orphans.insert("total".to_string(), json!(findings.artifact_orphans.len()));
    insert_audit_collection(
        &mut artifact_orphans,
        "files",
        &findings.artifact_orphans,
        limit,
    );

    // Entry-point fence: runtime entries with no importers are roots, not
    // orphans to review — reported separately, never silently dropped.
    let mut entrypoint_orphans = Map::new();
    entrypoint_orphans.insert(
        "total".to_string(),
        json!(findings.entrypoint_orphans.len()),
    );
    insert_audit_collection(
        &mut entrypoint_orphans,
        "files",
        &findings.entrypoint_orphans,
        limit,
    );

    let mut shadow_exports = Map::new();
    shadow_exports.insert("total".to_string(), json!(findings.shadow_exports.len()));
    insert_audit_collection(
        &mut shadow_exports,
        "items",
        &findings.shadow_exports,
        limit,
    );

    let mut crowds = Map::new();
    crowds.insert("total".to_string(), json!(findings.crowds.len()));
    insert_audit_collection(&mut crowds, "clusters", &findings.crowds, limit);

    Value::Object(Map::from_iter([
        ("cycles".to_string(), Value::Object(cycles)),
        ("dead_exports".to_string(), Value::Object(dead_exports)),
        ("twins".to_string(), Value::Object(twins)),
        ("orphan_files".to_string(), Value::Object(orphan_files)),
        (
            "artifact_orphans".to_string(),
            Value::Object(artifact_orphans),
        ),
        (
            "entrypoint_orphans".to_string(),
            Value::Object(entrypoint_orphans),
        ),
        ("shadow_exports".to_string(), Value::Object(shadow_exports)),
        ("crowds".to_string(), Value::Object(crowds)),
        (
            "summary".to_string(),
            json!({
                "total_files": findings.total_files,
                "total_loc": findings.total_loc,
            }),
        ),
    ]))
}

pub fn coverage_report(snapshot: &Snapshot, options: CoverageReportOptions) -> CoverageReport {
    let gaps = if options.include_gaps {
        let mut gaps = find_coverage_gaps(snapshot);
        if options.handlers_only {
            gaps.retain(|gap| matches!(gap.kind, GapKind::HandlerWithoutTest));
        }
        if options.events_only {
            gaps.retain(|gap| matches!(gap.kind, GapKind::EventWithoutTest));
        }
        if let Some(min_severity) = options.min_severity {
            gaps.retain(|gap| gap.severity <= min_severity);
        }
        gaps
    } else {
        Vec::new()
    };

    let tests = options
        .include_tests
        .then(|| analyze_test_coverage(snapshot));

    CoverageReport { gaps, tests }
}

fn insert_audit_collection<T: Serialize>(
    section: &mut Map<String, Value>,
    key: &str,
    items: &[T],
    limit: Option<usize>,
) {
    let display_limit = limit.unwrap_or(usize::MAX);
    section.insert(
        key.to_string(),
        json!(items.iter().take(display_limit).collect::<Vec<_>>()),
    );

    if let Some(limit) = limit {
        let omitted = items.len().saturating_sub(limit);
        section.insert("limit".to_string(), json!(limit));
        section.insert("omitted".to_string(), json!(omitted));
        section.insert("truncated".to_string(), json!(omitted > 0));
    }
}

fn is_entry_point(path: &str) -> bool {
    path.ends_with("/main.rs")
        || path.ends_with("/lib.rs")
        || path.ends_with("/main.ts")
        || path.ends_with("/main.tsx")
        || path.ends_with("/main.js")
        || path.ends_with("/main.jsx")
        || path.ends_with("/index.ts")
        || path.ends_with("/index.tsx")
        || path.ends_with("/index.js")
        || path.ends_with("/index.jsx")
        || path.ends_with("/App.tsx")
        || path.ends_with("/App.jsx")
        || path.ends_with("/_app.tsx")
        || path.ends_with("/_app.jsx")
        || path.ends_with("/__init__.py")
        || path == "main.rs"
        || path == "lib.rs"
        || path == "main.ts"
        || path == "index.ts"
}

fn is_test_file_path(path: &str) -> bool {
    path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("/__tests__/")
        || path.contains("/spec/")
        || path.ends_with(".test.ts")
        || path.ends_with(".test.tsx")
        || path.ends_with(".test.js")
        || path.ends_with(".test.jsx")
        || path.ends_with(".spec.ts")
        || path.ends_with(".spec.tsx")
        || path.ends_with(".spec.js")
        || path.ends_with(".spec.jsx")
        || path.ends_with("_test.rs")
        || path.ends_with("_test.py")
        || path.starts_with("test_")
        || path.contains("/test_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{CommandBridge, GraphEdge};
    use crate::types::{ExportSymbol, FileAnalysis, ImportEntry, ImportKind, ImportSymbol};

    fn sample_snapshot() -> Snapshot {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);

        let mut lib = FileAnalysis::new("src/lib.rs".to_string());
        lib.loc = 12;
        lib.language = "rust".to_string();
        lib.exports.push(ExportSymbol::new(
            "used_api".to_string(),
            "function",
            "named",
            Some(1),
        ));
        lib.exports.push(ExportSymbol::new(
            "untested_api".to_string(),
            "function",
            "named",
            Some(5),
        ));

        let mut tests = FileAnalysis::new("tests/lib_test.rs".to_string());
        tests.kind = "test".to_string();
        tests.is_test = true;
        let mut test_import = ImportEntry::new("src/lib.rs".to_string(), ImportKind::Static);
        test_import.resolved_path = Some("src/lib.rs".to_string());
        test_import.symbols = vec![ImportSymbol {
            name: "used_api".to_string(),
            alias: None,
            is_default: false,
        }];
        tests.imports.push(test_import);

        snapshot.files = vec![lib, tests];
        snapshot.edges = vec![GraphEdge {
            from: "tests/lib_test.rs".to_string(),
            to: "src/lib.rs".to_string(),
            label: "named".to_string(),
        }];
        snapshot.command_bridges = vec![CommandBridge {
            name: "untested_api".to_string(),
            frontend_calls: vec![("src/app.ts".to_string(), 10)],
            backend_handler: Some(("src/lib.rs".to_string(), 5)),
            has_handler: true,
            is_called: true,
        }];

        snapshot
    }

    #[test]
    fn health_report_returns_cli_parity_shape() {
        let snapshot = sample_snapshot();
        let report = health_report(&snapshot, Path::new("."), HealthReportOptions::default());

        assert_eq!(report.cycles.total, 0);
        assert!(report.dead_exports.high_confidence <= report.dead_exports.total);
        assert_eq!(report.twins.total, 0);
    }

    #[test]
    fn findings_report_can_emit_summary() {
        let snapshot = sample_snapshot();
        let summary = findings_summary_report(&snapshot, FindingsReportOptions::default());

        assert_eq!(summary.files, 2);
        assert!(summary.health_score <= 100);
    }

    #[test]
    fn audit_report_contains_summary_and_orphans() {
        let snapshot = sample_snapshot();
        let findings = audit_findings(&snapshot, Path::new("."), AuditReportOptions::default());
        let json = audit_json_report(&findings, Some(1));

        assert_eq!(json["summary"]["total_files"], 2);
        assert!(json["orphan_files"]["total"].as_u64().is_some());
    }

    /// Artifact fence (w1-b): lockfiles, generated bundles and docs are not
    /// "orphans to review" — they are extracted to artifact_orphans.
    #[test]
    fn audit_orphans_extract_generated_and_docs() {
        let mut snapshot = sample_snapshot();
        let mut lockfile = FileAnalysis::new("package-lock.json".to_string());
        lockfile.loc = 5000;
        let mut dist = FileAnalysis::new("public_dist/index.html".to_string());
        dist.loc = 120;
        let mut doc = FileAnalysis::new("docs/guide.md".to_string());
        doc.loc = 80;
        let mut product_orphan = FileAnalysis::new("src/forgotten.rs".to_string());
        product_orphan.loc = 40;
        snapshot.files.extend([lockfile, dist, doc, product_orphan]);

        let findings = audit_findings(&snapshot, Path::new("."), AuditReportOptions::default());

        let orphan_paths: Vec<&str> = findings
            .orphan_files
            .iter()
            .map(|o| o.path.as_str())
            .collect();
        assert!(
            !orphan_paths.contains(&"package-lock.json"),
            "package-lock.json must not be an actionable orphan (was: {:?})",
            orphan_paths
        );
        assert!(
            !orphan_paths.contains(&"public_dist/index.html"),
            "generated dist files must not be actionable orphans"
        );
        assert!(
            !orphan_paths.contains(&"docs/guide.md"),
            "docs must not be actionable orphans"
        );
        assert!(
            orphan_paths.contains(&"src/forgotten.rs"),
            "real product orphan must stay actionable (was: {:?})",
            orphan_paths
        );

        let artifact_paths: Vec<&str> = findings
            .artifact_orphans
            .iter()
            .map(|o| o.path.as_str())
            .collect();
        assert!(artifact_paths.contains(&"package-lock.json"));
        assert!(artifact_paths.contains(&"public_dist/index.html"));
        assert!(artifact_paths.contains(&"docs/guide.md"));

        // Extracted, not silently dropped: JSON report carries them.
        let json = audit_json_report(&findings, None);
        assert!(json["artifact_orphans"]["total"].as_u64().unwrap() >= 3);
    }

    #[test]
    fn coverage_report_filters_handler_gaps() {
        let snapshot = sample_snapshot();
        let report = coverage_report(
            &snapshot,
            CoverageReportOptions {
                include_gaps: true,
                include_tests: true,
                handlers_only: true,
                events_only: false,
                min_severity: Some(Severity::Critical),
            },
        );

        assert!(report.tests.is_some());
        assert!(
            report
                .gaps
                .iter()
                .all(|gap| matches!(gap.kind, GapKind::HandlerWithoutTest))
        );
    }
}
