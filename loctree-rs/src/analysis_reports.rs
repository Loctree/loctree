//! Library-facing analysis reports shared by MCP, CLI-adjacent callers, and tests.
//!
//! This module intentionally stays out of `cli::*` so non-CLI surfaces can
//! expose health, findings, audit, and coverage without importing parser or
//! terminal dispatch code.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::analyzer::audit_report::{
    AuditFindings, OrphanFile, ReportProvenance, ShadowExport, audit_health,
};
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
            workspace_closed: false,
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
            workspace_closed: false,
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

    // Zero-importer files enter the list; role/artifact/entrypoint fences
    // then extract them into named buckets. Tests are no longer silently
    // dropped here — they are graph roots and must stay visible.
    let mut orphan_files: Vec<(String, usize)> = in_degree
        .iter()
        .filter(|(path, count)| **count == 0 && !is_entry_point(path))
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

    // Role fence: test / script / doc / manifest are graph roots by role.
    // Extracted, never silently dropped — same shape as artifact_orphans.
    let (test_orphans, orphan_files): (Vec<_>, Vec<_>) = orphan_files
        .into_iter()
        .partition(|(path, _)| is_test_file_path(path));
    let (script_orphans, orphan_files): (Vec<_>, Vec<_>) = orphan_files
        .into_iter()
        .partition(|(path, _)| is_script_role_path(path));
    let (doc_orphans, orphan_files): (Vec<_>, Vec<_>) = orphan_files
        .into_iter()
        .partition(|(path, _)| is_doc_role_path(path));
    let (manifest_orphans, orphan_files): (Vec<_>, Vec<_>) = orphan_files
        .into_iter()
        .partition(|(path, _)| is_manifest_role_path(path));

    // Artifact fence: generated files, lockfiles, vendored code and fixtures
    // are not actionable "orphans to review" — separate, don't drop.
    // Docs are a role (above), not an artifact.
    let (artifact_orphans, orphan_files): (Vec<_>, Vec<_>) = orphan_files
        .into_iter()
        .partition(|(path, _)| crate::analyzer::classify::artifact_class(path, None).is_artifact());

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
        test_orphans: to_orphan_files(test_orphans),
        script_orphans: to_orphan_files(script_orphans),
        doc_orphans: to_orphan_files(doc_orphans),
        manifest_orphans: to_orphan_files(manifest_orphans),
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
        provenance: Some(ReportProvenance {
            version: crate::BUILD_VERSION.to_string(),
            generated_at: if snapshot.metadata.generated_at.is_empty() {
                time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Iso8601::DEFAULT)
                    .unwrap_or_else(|_| "unknown".to_string())
            } else {
                snapshot.metadata.generated_at.clone()
            },
            root: root.display().to_string(),
            git_commit: snapshot
                .metadata
                .git_commit
                .clone()
                .filter(|commit| !commit.is_empty()),
        }),
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

    let mut test_orphans = Map::new();
    test_orphans.insert("total".to_string(), json!(findings.test_orphans.len()));
    insert_audit_collection(&mut test_orphans, "files", &findings.test_orphans, limit);

    let mut script_orphans = Map::new();
    script_orphans.insert("total".to_string(), json!(findings.script_orphans.len()));
    insert_audit_collection(
        &mut script_orphans,
        "files",
        &findings.script_orphans,
        limit,
    );

    let mut doc_orphans = Map::new();
    doc_orphans.insert("total".to_string(), json!(findings.doc_orphans.len()));
    insert_audit_collection(&mut doc_orphans, "files", &findings.doc_orphans, limit);

    let mut manifest_orphans = Map::new();
    manifest_orphans.insert("total".to_string(), json!(findings.manifest_orphans.len()));
    insert_audit_collection(
        &mut manifest_orphans,
        "files",
        &findings.manifest_orphans,
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

    let health = audit_health(findings);

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
        ("test_orphans".to_string(), Value::Object(test_orphans)),
        ("script_orphans".to_string(), Value::Object(script_orphans)),
        ("doc_orphans".to_string(), Value::Object(doc_orphans)),
        (
            "manifest_orphans".to_string(),
            Value::Object(manifest_orphans),
        ),
        ("shadow_exports".to_string(), Value::Object(shadow_exports)),
        ("crowds".to_string(), Value::Object(crowds)),
        (
            "summary".to_string(),
            json!({
                "total_files": findings.total_files,
                "total_loc": findings.total_loc,
                "health_score": health.health,
                "health_reason": health.reason,
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

fn to_orphan_files(files: Vec<(String, usize)>) -> Vec<OrphanFile> {
    files
        .into_iter()
        .map(|(path, loc)| OrphanFile { path, loc })
        .collect()
}

/// Path looks like a test file by role. Graph root, not an orphan to review.
///
/// Swift/SPM uses capitalised `Tests/`, `*Tests/` (e.g. `PensieveTests/`),
/// `*Tests.swift` and `*Spec.swift`. Directory matching is case-insensitive
/// on whole segments; filename matching is suffix-bound so
/// `Sources/App/Contest.swift` is not a test.
pub fn is_test_file_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();

    if has_test_directory_segment(&lower) {
        return true;
    }

    lower.contains("/__tests__/")
        || lower.starts_with("__tests__/")
        || lower.contains("/spec/")
        || lower.starts_with("spec/")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".test.tsx")
        || lower.ends_with(".test.js")
        || lower.ends_with(".test.jsx")
        || lower.ends_with(".spec.ts")
        || lower.ends_with(".spec.tsx")
        || lower.ends_with(".spec.js")
        || lower.ends_with(".spec.jsx")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_test.py")
        || lower.starts_with("test_")
        || lower.contains("/test_")
        || swift_test_filename(&lower)
}

fn has_test_directory_segment(lower_path: &str) -> bool {
    lower_path.split('/').any(|segment| {
        !segment.is_empty()
            && (segment == "test"
                || segment == "tests"
                || segment == "__tests__"
                || segment == "spec"
                || (segment.len() > 5 && segment.ends_with("tests")))
    })
}

fn swift_test_filename(lower_path: &str) -> bool {
    let file = lower_path.rsplit('/').next().unwrap_or(lower_path);
    let Some(stem) = file.strip_suffix(".swift") else {
        return false;
    };
    stem.ends_with("tests") || stem.ends_with("spec")
}

fn is_script_role_path(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    if lower.contains("/scripts/") || lower.starts_with("scripts/") {
        return true;
    }
    let file = lower.rsplit('/').next().unwrap_or(lower.as_str());
    file.ends_with(".sh") || file.ends_with(".bash") || file.ends_with(".zsh")
}

fn is_doc_role_path(path: &str) -> bool {
    crate::analyzer::classify::resource_kind(path) == Some("doc")
}

fn is_manifest_role_path(path: &str) -> bool {
    let file = path
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    matches!(
        file.as_str(),
        "package.swift"
            | "package.json"
            | "cargo.toml"
            | "pyproject.toml"
            | "podfile"
            | "package.resolved"
    )
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
        assert!(summary.health_score.is_some_and(|h| h <= 100));
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
        assert!(
            !artifact_paths.contains(&"docs/guide.md"),
            "docs are a role root, not an artifact"
        );
        let doc_paths: Vec<&str> = findings
            .doc_orphans
            .iter()
            .map(|o| o.path.as_str())
            .collect();
        assert!(
            doc_paths.contains(&"docs/guide.md"),
            "docs must land in the named doc-role bucket (was: {:?})",
            doc_paths
        );

        // Extracted, not silently dropped: JSON report carries them.
        let json = audit_json_report(&findings, None);
        assert!(json["artifact_orphans"]["total"].as_u64().unwrap() >= 2);
        assert!(json["doc_orphans"]["total"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn role_root_conventions_are_tests() {
        assert!(
            is_test_file_path("Tests/AppTests/AppTests.swift"),
            "Tests/ is the SPM test root"
        );
        assert!(
            is_test_file_path("PensieveTests/IndexTests.swift"),
            "PensieveTests/ is a Swift test target directory"
        );
        assert!(
            is_test_file_path("Sources/App/RoleRootsHelperTests.swift"),
            "*Tests.swift is a test file even outside Tests/"
        );
        assert!(
            is_test_file_path("Sources/App/RoleRootsSpec.swift"),
            "*Spec.swift is a test file"
        );
        assert!(
            is_test_file_path("AppTests/RoleRootsHelperTests.swift"),
            "AppTests/ is a *Tests directory"
        );
        assert!(
            is_test_file_path("src/tests/lib_test.rs"),
            "lowercase /tests/ stays a test path"
        );
    }

    #[test]
    fn role_root_contest_is_not_a_test() {
        assert!(
            !is_test_file_path("Sources/App/Contest.swift"),
            "Contest.swift must not match a test suffix — the 'test' letters are mid-stem"
        );
        assert!(!is_test_file_path("Sources/App/RoleRootsHelper.swift"));
        assert!(!is_test_file_path("Sources/App/RoleRootsApp.swift"));
    }

    #[test]
    fn role_root_buckets_appear_in_audit_json() {
        let mut snapshot = sample_snapshot();
        let mut xctest = FileAnalysis::new("PensieveTests/IndexTests.swift".to_string());
        xctest.loc = 20;
        xctest.language = "swift".to_string();
        let mut spec = FileAnalysis::new("Tests/AppSpec.swift".to_string());
        spec.loc = 8;
        spec.language = "swift".to_string();
        let mut script = FileAnalysis::new("scripts/build-role-roots.sh".to_string());
        script.loc = 6;
        script.language = "shell".to_string();
        let mut manifest = FileAnalysis::new("Package.swift".to_string());
        manifest.loc = 12;
        let mut product = FileAnalysis::new("src/forgotten.rs".to_string());
        product.loc = 40;
        snapshot
            .files
            .extend([xctest, spec, script, manifest, product]);

        let findings = audit_findings(&snapshot, Path::new("."), AuditReportOptions::default());
        let json = audit_json_report(&findings, None);

        assert_eq!(
            json["orphan_files"]["total"].as_u64().unwrap(),
            1,
            "only the real product orphan stays on the review list: {:?}",
            findings
                .orphan_files
                .iter()
                .map(|o| o.path.as_str())
                .collect::<Vec<_>>()
        );
        assert!(json["test_orphans"]["total"].as_u64().unwrap() >= 2);
        assert!(json["script_orphans"]["total"].as_u64().unwrap() >= 1);
        assert_eq!(json["manifest_orphans"]["total"].as_u64().unwrap(), 1);
        assert!(json.get("doc_orphans").is_some());

        let test_paths: Vec<&str> = findings
            .test_orphans
            .iter()
            .map(|o| o.path.as_str())
            .collect();
        assert!(test_paths.contains(&"PensieveTests/IndexTests.swift"));
        assert!(test_paths.contains(&"Tests/AppSpec.swift"));
        assert_eq!(
            findings.script_orphans[0].path,
            "scripts/build-role-roots.sh"
        );
        assert_eq!(findings.manifest_orphans[0].path, "Package.swift");
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
