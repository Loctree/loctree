//! Coverage gap detection - finds mismatches between production usage and test coverage.
//!
//! This module cross-references three data sources:
//! 1. **Production usage**: What FE actually calls (invoke(), emit())
//! 2. **Test imports**: What test files import
//! 3. **Handler definitions**: What exists in backend
//!
//! The result is actionable gaps like:
//! - Handlers used in production but not tested (HIGH RISK)
//! - Events emitted but no test coverage (MEDIUM RISK)
//! - Tested code that's not used in production (potential dead code)
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use super::classify::{ArtifactFenceStats, artifact_class, is_test_path};
use crate::snapshot::{CommandBridge, EventBridge, Snapshot};
use crate::types::{FileAnalysis, ImportEntry, ImportResolutionKind};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Decide whether an import resolves to a symbol that this repo could
/// realistically have a "test export" coverage gap for.
///
/// Stdlib (Python `typing.Annotated`, `os.path.join`, …) and bare-specifier
/// imports (npm packages, pip packages — `rich.progress.BarColumn`,
/// `pydantic.BaseModel`) are not exports of the repo; flagging them as
/// "production usage that lacks a test" drowns real coverage gaps in noise.
///
/// Source hak: 2026-05-18 Screenscribe HAK 3 (`loct coverage` mixes external
/// imports as "missing exports"). See `~/internal-artifacts/loctree/loctree-feedback.md`.
fn import_is_local_repo_symbol(import: &ImportEntry) -> bool {
    match import.resolution {
        ImportResolutionKind::Local => true,
        ImportResolutionKind::Stdlib
        | ImportResolutionKind::Dynamic
        | ImportResolutionKind::Unknown => false,
    }
}

/// A gap in test coverage
#[derive(Debug, Clone, Serialize)]
pub struct CoverageGap {
    pub kind: GapKind,
    pub target: String,
    pub location: String,
    pub severity: Severity,
    pub recommendation: String,
    /// Additional context about the gap
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// File paths involved
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    /// Handler used in production but not tested
    HandlerWithoutTest,
    /// Event emitted in production but not tested
    EventWithoutTest,
    /// Export used in production but not tested
    ExportWithoutTest,
    /// Tested but not used in production (suspicious)
    TestedButUnused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical, // Handler without test (can break runtime)
    High,     // Event without test (data flow issues)
    Medium,   // Export without test (integration gaps)
    Low,      // Tested but unused (cleanup candidate)
}

/// Find all coverage gaps in a snapshot (artifact fence on, stats discarded).
pub fn find_coverage_gaps(snapshot: &Snapshot) -> Vec<CoverageGap> {
    find_coverage_gaps_fenced(snapshot, false).0
}

/// Find all coverage gaps with explicit artifact-fence control.
///
/// With `include_artifacts == false` (default), gaps whose evidence lives
/// entirely in vendored/minified/fixture/generated/template files are cut and
/// counted in the returned [`ArtifactFenceStats`] so the caller can print the
/// `excluded: …` summary line — the fence never cuts silently.
pub fn find_coverage_gaps_fenced(
    snapshot: &Snapshot,
    include_artifacts: bool,
) -> (Vec<CoverageGap>, ArtifactFenceStats) {
    let mut gaps = Vec::new();
    let mut fence = ArtifactFenceStats::default();

    // Build test file detection
    let test_files = detect_test_files(&snapshot.files);

    // Build test import index: what symbols do test files import?
    let test_imports = build_test_import_index(&snapshot.files, &test_files);

    // Gap 1: Handlers without tests
    gaps.extend(find_handler_gaps(&snapshot.command_bridges, &test_imports));

    // Gap 2: Events without tests
    gaps.extend(find_event_gaps(&snapshot.event_bridges, &test_imports));

    // Gap 3: Exports without tests (from files with production usage)
    gaps.extend(find_export_gaps(
        &snapshot.files,
        &test_imports,
        &test_files,
    ));

    // Gap 4: Tested but unused (inverse analysis)
    gaps.extend(find_tested_but_unused(
        &snapshot.command_bridges,
        &snapshot.event_bridges,
        &test_imports,
    ));

    // Artifact fence: a gap whose evidence files are all artifacts is noise
    // from vendored/minified/generated inputs (e.g. event tokens parsed out
    // of cytoscape.min.js), not product risk.
    if !include_artifacts {
        gaps.retain(|gap| match gap_artifact_class(gap) {
            None => true,
            Some(class) => {
                fence.record(class);
                false
            }
        });
    }

    // Sort by severity (critical first)
    gaps.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.target.cmp(&b.target)));

    (gaps, fence)
}

/// Classify a gap against the artifact fence.
///
/// Returns `Some(class)` when *every* evidence location (gap.location +
/// gap.files) sits in artifact files — the class of the primary location is
/// reported. Gaps with at least one product-code location stay visible.
fn gap_artifact_class(gap: &CoverageGap) -> Option<super::classify::ArtifactClass> {
    let location_path =
        gap.location
            .rsplit_once(':')
            .map_or(gap.location.as_str(), |(path, line)| {
                if line.chars().all(|c| c.is_ascii_digit()) {
                    path
                } else {
                    gap.location.as_str()
                }
            });

    let mut paths: Vec<&str> = Vec::with_capacity(gap.files.len() + 1);
    if !location_path.is_empty() && location_path != "unknown" {
        paths.push(location_path);
    }
    paths.extend(gap.files.iter().map(String::as_str));

    if paths.is_empty() {
        return None;
    }

    let mut primary_class = None;
    for path in paths {
        let class = artifact_class(path, None);
        if !class.is_artifact() {
            return None;
        }
        primary_class.get_or_insert(class);
    }
    primary_class
}

/// Detect which files are test files
fn detect_test_files(files: &[FileAnalysis]) -> HashSet<String> {
    files
        .iter()
        .filter(|f| is_test_path(&f.path))
        .map(|f| f.path.clone())
        .collect()
}

/// Build index of what test files import
/// Returns: Map<symbol_name, Vec<test_file_that_imports_it>>
fn build_test_import_index(
    files: &[FileAnalysis],
    test_files: &HashSet<String>,
) -> HashMap<String, Vec<String>> {
    let mut index: HashMap<String, Vec<String>> = HashMap::new();

    for file in files {
        if !test_files.contains(&file.path) {
            continue;
        }

        // Collect all imported symbols from this test file
        for import in &file.imports {
            for symbol in &import.symbols {
                let name = if symbol.is_default {
                    "default".to_string()
                } else {
                    symbol.name.clone()
                };

                index.entry(name).or_default().push(file.path.clone());
            }
        }

        // Also track command handlers if test files define mocks/fixtures
        for handler in &file.command_handlers {
            index
                .entry(handler.name.clone())
                .or_default()
                .push(file.path.clone());
        }
    }

    index
}

/// Find handlers used in production but not tested
fn find_handler_gaps(
    command_bridges: &[CommandBridge],
    test_imports: &HashMap<String, Vec<String>>,
) -> Vec<CoverageGap> {
    let mut gaps = Vec::new();

    for bridge in command_bridges {
        // Only care about handlers that:
        // 1. Have a backend implementation
        // 2. Are called from frontend (production usage)
        if !bridge.has_handler || !bridge.is_called {
            continue;
        }

        // Check if this handler is imported by any test file
        let is_tested = test_imports.contains_key(&bridge.name);

        if !is_tested {
            let location = bridge
                .backend_handler
                .as_ref()
                .map(|(path, line)| format!("{}:{}", path, line))
                .unwrap_or_else(|| "unknown".to_string());

            let frontend_files: Vec<String> = bridge
                .frontend_calls
                .iter()
                .map(|(path, _)| path.clone())
                .collect();

            gaps.push(CoverageGap {
                kind: GapKind::HandlerWithoutTest,
                target: bridge.name.clone(),
                location,
                severity: Severity::Critical,
                recommendation: format!(
                    "Add test coverage for handler '{}' - it's called from {} production location(s) but has no tests",
                    bridge.name,
                    bridge.frontend_calls.len()
                ),
                context: Some(format!(
                    "Called from: {}",
                    frontend_files.join(", ")
                )),
                files: frontend_files,
            });
        }
    }

    gaps
}

/// Find events emitted in production but not tested
fn find_event_gaps(
    event_bridges: &[EventBridge],
    test_imports: &HashMap<String, Vec<String>>,
) -> Vec<CoverageGap> {
    let mut gaps = Vec::new();

    for bridge in event_bridges {
        // Only care about events that are actually emitted
        if bridge.emits.is_empty() {
            continue;
        }

        // Check if event name appears in test imports (rough heuristic)
        let is_tested = test_imports.contains_key(&bridge.name);

        if !is_tested {
            let location = bridge
                .emits
                .first()
                .map(|(path, line, _)| format!("{}:{}", path, line))
                .unwrap_or_else(|| "unknown".to_string());

            let emit_files: Vec<String> = bridge
                .emits
                .iter()
                .map(|(path, _, _)| path.clone())
                .collect();

            gaps.push(CoverageGap {
                kind: GapKind::EventWithoutTest,
                target: bridge.name.clone(),
                location,
                severity: Severity::High,
                recommendation: format!(
                    "Add test coverage for event '{}' - emitted from {} location(s) but not tested",
                    bridge.name,
                    bridge.emits.len()
                ),
                context: Some(format!(
                    "Emitted from: {}",
                    emit_files
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
                files: emit_files,
            });
        }
    }

    gaps
}

/// Find exports used in production but not tested
fn find_export_gaps(
    files: &[FileAnalysis],
    test_imports: &HashMap<String, Vec<String>>,
    test_files: &HashSet<String>,
) -> Vec<CoverageGap> {
    let mut gaps = Vec::new();

    // Build usage map: which exports are actually imported in production code?
    let mut production_usage: HashMap<String, Vec<String>> = HashMap::new();

    for file in files {
        if test_files.contains(&file.path) {
            continue; // Skip test files
        }

        for import in &file.imports {
            // External symbols (stdlib, npm/pip packages, dynamic imports,
            // unresolved bare specifiers) are not our exports. Flagging them
            // as production-used-but-untested generates ~30 false coverage
            // gaps for `from typing import Annotated`, `from rich.progress
            // import BarColumn`, etc. — drowning real gaps in noise.
            // See loctree-feedback.md 2026-05-18 Screenscribe HAK 3.
            if !import_is_local_repo_symbol(import) {
                continue;
            }
            for symbol in &import.symbols {
                let name = if symbol.is_default {
                    "default".to_string()
                } else {
                    symbol.name.clone()
                };

                production_usage
                    .entry(name)
                    .or_default()
                    .push(file.path.clone());
            }
        }
    }

    // Find exports that are used in production but not tested
    for (symbol, usage_locations) in production_usage {
        if symbol == "*" {
            continue; // Skip wildcard imports
        }

        let is_tested = test_imports.contains_key(&symbol);

        if !is_tested && usage_locations.len() >= 2 {
            // Only flag if used in multiple places (more important)
            gaps.push(CoverageGap {
                kind: GapKind::ExportWithoutTest,
                target: symbol.clone(),
                location: usage_locations.first().cloned().unwrap_or_default(),
                severity: Severity::Medium,
                recommendation: format!(
                    "Add test for export '{}' - used in {} production files but not tested",
                    symbol,
                    usage_locations.len()
                ),
                context: Some(format!(
                    "Used in: {}",
                    usage_locations
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
                files: usage_locations,
            });
        }
    }

    gaps
}

/// Find handlers/events that are tested but not used in production
fn find_tested_but_unused(
    command_bridges: &[CommandBridge],
    event_bridges: &[EventBridge],
    test_imports: &HashMap<String, Vec<String>>,
) -> Vec<CoverageGap> {
    let mut gaps = Vec::new();

    // Build set of production-used handlers and events
    let mut production_handlers: HashSet<String> = HashSet::new();
    let mut production_events: HashSet<String> = HashSet::new();

    for bridge in command_bridges {
        if bridge.is_called {
            production_handlers.insert(bridge.name.clone());
        }
    }

    for bridge in event_bridges {
        if !bridge.emits.is_empty() {
            production_events.insert(bridge.name.clone());
        }
    }

    // Check test imports for handlers/events not in production
    for (symbol, test_files) in test_imports {
        // Check if it looks like a handler (common naming patterns)
        let looks_like_handler = symbol.contains("Handler")
            || symbol.contains("Command")
            || symbol.starts_with("handle_")
            || symbol.starts_with("cmd_");

        let looks_like_event =
            symbol.contains("Event") || symbol.contains("event") || symbol.ends_with("_event");

        if looks_like_handler && !production_handlers.contains(symbol) {
            gaps.push(CoverageGap {
                kind: GapKind::TestedButUnused,
                target: symbol.clone(),
                location: test_files.first().cloned().unwrap_or_default(),
                severity: Severity::Low,
                recommendation: format!(
                    "Handler '{}' has tests but is not called in production - consider removing if truly unused",
                    symbol
                ),
                context: Some(format!(
                    "Tested in: {}",
                    test_files.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
                )),
                files: test_files.clone(),
            });
        } else if looks_like_event && !production_events.contains(symbol) {
            gaps.push(CoverageGap {
                kind: GapKind::TestedButUnused,
                target: symbol.clone(),
                location: test_files.first().cloned().unwrap_or_default(),
                severity: Severity::Low,
                recommendation: format!(
                    "Event '{}' has tests but is not emitted in production - consider removing if truly unused",
                    symbol
                ),
                context: Some(format!(
                    "Tested in: {}",
                    test_files.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
                )),
                files: test_files.clone(),
            });
        }
    }

    gaps
}

/// Generate actionable quick wins from gaps
pub fn gaps_to_quick_wins(gaps: &[CoverageGap]) -> Vec<crate::analyzer::for_ai::QuickWin> {
    gaps.iter()
        .enumerate()
        .map(|(idx, gap)| {
            let (action, why, fix_hint, complexity) = match gap.kind {
                GapKind::HandlerWithoutTest => (
                    "Add test coverage for production handler",
                    format!("Handler '{}' is called in production but has no tests - runtime failures won't be caught", gap.target),
                    format!("Create test file that imports and tests '{}' handler", gap.target),
                    "medium",
                ),
                GapKind::EventWithoutTest => (
                    "Add test coverage for event emission",
                    format!("Event '{}' is emitted in production but has no tests - event handlers may break silently", gap.target),
                    format!("Add test that verifies '{}' event is emitted with correct payload", gap.target),
                    "medium",
                ),
                GapKind::ExportWithoutTest => (
                    "Add test for production export",
                    format!("Export '{}' is used across multiple production files but has no tests", gap.target),
                    format!("Create unit tests for '{}' to ensure behavior is documented", gap.target),
                    "easy",
                ),
                GapKind::TestedButUnused => (
                    "Remove unused test or restore production usage",
                    format!("'{}' has tests but is not used in production - likely dead code", gap.target),
                    format!("Either remove tests for '{}' or restore production usage if intentional", gap.target),
                    "easy",
                ),
            };

            let priority = match gap.severity {
                Severity::Critical => 5 + idx as u8,
                Severity::High => 15 + idx as u8,
                Severity::Medium => 25 + idx as u8,
                Severity::Low => 35 + idx as u8,
            };

            crate::analyzer::for_ai::QuickWin {
                priority: priority.min(100),
                kind: format!("{:?}", gap.kind).to_lowercase(),
                action: action.to_string(),
                target: gap.target.clone(),
                location: gap.location.clone(),
                impact: gap.recommendation.clone(),
                why,
                fix_hint: fix_hint.to_string(),
                complexity: complexity.to_string(),
                suggested_next: crate::analyzer::for_ai::literal_truth_suggested_next(
                    &gap.target,
                    Some(&gap.location),
                ),
                trace_cmd: Some(format!("loct trace {}", gap.target)),
                open_url: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ImportEntry, ImportKind, ImportSymbol};

    fn local_import(source: &str, symbol: &str) -> ImportEntry {
        let mut imp = ImportEntry::new(source.to_string(), ImportKind::Static);
        imp.resolution = ImportResolutionKind::Local;
        imp.symbols.push(ImportSymbol {
            name: symbol.to_string(),
            alias: None,
            is_default: false,
        });
        imp
    }

    fn stdlib_import(source: &str, symbol: &str) -> ImportEntry {
        let mut imp = ImportEntry::new(source.to_string(), ImportKind::Static);
        imp.resolution = ImportResolutionKind::Stdlib;
        imp.symbols.push(ImportSymbol {
            name: symbol.to_string(),
            alias: None,
            is_default: false,
        });
        imp
    }

    fn unknown_bare_import(source: &str, symbol: &str) -> ImportEntry {
        let mut imp = ImportEntry::new(source.to_string(), ImportKind::Static);
        imp.resolution = ImportResolutionKind::Unknown;
        imp.symbols.push(ImportSymbol {
            name: symbol.to_string(),
            alias: None,
            is_default: false,
        });
        imp
    }

    fn file_with_imports(path: &str, imports: Vec<ImportEntry>) -> FileAnalysis {
        FileAnalysis {
            path: path.to_string(),
            imports,
            ..FileAnalysis::default()
        }
    }

    /// Source hak: 2026-05-18 Screenscribe HAK 3 — `from typing import
    /// Annotated` triggered a coverage gap for stdlib symbol. Filter must
    /// drop stdlib resolution before computing production_usage.
    #[test]
    fn find_export_gaps_skips_stdlib_imports() {
        let files = vec![
            file_with_imports(
                "screenscribe/analyze_server.py",
                vec![
                    stdlib_import("typing", "Annotated"),
                    stdlib_import("typing", "Callable"),
                ],
            ),
            file_with_imports(
                "screenscribe/cli.py",
                vec![
                    stdlib_import("typing", "Annotated"),
                    stdlib_import("typing", "Callable"),
                ],
            ),
        ];
        let test_imports = HashMap::new();
        let test_files = HashSet::new();
        let gaps = find_export_gaps(&files, &test_imports, &test_files);
        assert!(
            !gaps.iter().any(|g| g.target == "Annotated"),
            "stdlib `Annotated` must not show up as a coverage gap (was: {:?})",
            gaps
        );
        assert!(
            !gaps.iter().any(|g| g.target == "Callable"),
            "stdlib `Callable` must not show up as a coverage gap (was: {:?})",
            gaps
        );
    }

    /// Source hak: 2026-05-18 Screenscribe HAK 3 — `from rich.progress import
    /// BarColumn` (bare specifier, resolution Unknown) flagged as gap. Same
    /// noise reduction applies to npm/pip package imports.
    #[test]
    fn find_export_gaps_skips_bare_specifier_imports() {
        let files = vec![
            file_with_imports(
                "screenscribe/api_utils.py",
                vec![
                    unknown_bare_import("rich.progress", "BarColumn"),
                    unknown_bare_import("rich.console", "Console"),
                ],
            ),
            file_with_imports(
                "screenscribe/cli.py",
                vec![
                    unknown_bare_import("rich.progress", "BarColumn"),
                    unknown_bare_import("rich.console", "Console"),
                ],
            ),
        ];
        let test_imports = HashMap::new();
        let test_files = HashSet::new();
        let gaps = find_export_gaps(&files, &test_imports, &test_files);
        assert!(
            !gaps.iter().any(|g| g.target == "BarColumn"),
            "bare-specifier `BarColumn` must not show up as a coverage gap (was: {:?})",
            gaps
        );
        assert!(
            !gaps.iter().any(|g| g.target == "Console"),
            "bare-specifier `Console` must not show up as a coverage gap (was: {:?})",
            gaps
        );
    }

    /// Artifact fence (w1-b / W2-02): event tokens parsed out of minified
    /// vendored JS (cytoscape.min.js) must not surface as coverage gaps.
    #[test]
    fn coverage_fence_cuts_minified_vendor_event_gaps() {
        use crate::snapshot::{EventBridge, Snapshot};

        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        snapshot.event_bridges = vec![
            EventBridge {
                name: "?".to_string(),
                emits: vec![(
                    "loctree-rs/src/analyzer/assets/cytoscape.min.js".to_string(),
                    29,
                    "emit_const".to_string(),
                )],
                listens: vec![],
                is_fe_sync: false,
                same_file_sync: false,
            },
            EventBridge {
                name: "user_saved".to_string(),
                emits: vec![("src/app.ts".to_string(), 10, "emit_const".to_string())],
                listens: vec![],
                is_fe_sync: false,
                same_file_sync: false,
            },
        ];

        let (gaps, fence) = find_coverage_gaps_fenced(&snapshot, false);
        assert!(
            !gaps.iter().any(|g| g.location.contains("min.js")),
            "minified vendor events must be fenced out (was: {:?})",
            gaps
        );
        assert!(
            gaps.iter().any(|g| g.target == "user_saved"),
            "product event gap must survive the fence (was: {:?})",
            gaps
        );
        assert_eq!(fence.vendored, 1, "fence must count the cut, not hide it");
        assert_eq!(fence.summary_line(), "excluded: vendored(1)");

        // Opt-out: --include-artifacts restores full truth
        let (gaps_all, fence_all) = find_coverage_gaps_fenced(&snapshot, true);
        assert!(
            gaps_all.iter().any(|g| g.location.contains("min.js")),
            "include_artifacts must restore vendored findings"
        );
        assert!(fence_all.is_empty());
    }

    /// Filter must not over-fire: locally-resolved exports (relative imports
    /// or absolute paths inside the repo) still trigger gaps when untested.
    #[test]
    fn find_export_gaps_keeps_local_imports() {
        let files = vec![
            file_with_imports(
                "screenscribe/cli.py",
                vec![local_import("./internal_util", "process_video")],
            ),
            file_with_imports(
                "screenscribe/report.py",
                vec![local_import("./internal_util", "process_video")],
            ),
        ];
        let test_imports = HashMap::new();
        let test_files = HashSet::new();
        let gaps = find_export_gaps(&files, &test_imports, &test_files);
        assert!(
            gaps.iter().any(|g| g.target == "process_video"),
            "locally-resolved untested export must still trigger a gap (was: {:?})",
            gaps
        );
    }
}
