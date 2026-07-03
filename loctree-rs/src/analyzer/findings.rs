//! Findings Module - Consolidated issue reporting for 0.7.0 artifact-first architecture
//!
//! This module produces `findings.json`, the single source of truth for all detected issues:
//! - Dead parrots (unused exports)
//! - Shadow exports
//! - Cycles (circular imports)
//! - Duplicates (twins)
//! - Barrel chaos
//! - Quick wins (actionable suggestions)
//!
//! Philosophy: One artifact, one query. No more hunting across multiple commands.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::analyzer::barrels::{BarrelAnalysis, analyze_barrel_chaos};
use crate::analyzer::cycles::{ClassifiedCycle, find_cycles_with_lazy};
use crate::analyzer::dead_parrots::{
    DeadExport, DeadFilterConfig, ShadowExport, canonical_dead_filter, compute_dead_truth_with,
};
use crate::analyzer::dist::DistResult;
use crate::analyzer::health_score::{HealthMetrics, calculate_health_score};
use crate::analyzer::memory_lint::{MemoryLintIssue, MemoryLintSummary, lint_memory_file};
use crate::analyzer::react_lint::{ReactLintIssue, ReactLintSummary, analyze_react_file};
use crate::analyzer::report::RankedDup;
use crate::analyzer::root_scan::ScanResults;
use crate::analyzer::ts_lint::{TsLintIssue, TsLintSummary, lint_ts_file};
use crate::analyzer::twins::{
    TwinCategory, categorize_twin, detect_exact_twins, filter_idiom_twins, find_dead_parrots,
};
use crate::semantic::{Classifier as SemClassifier, SemanticFacts};
use crate::snapshot::{EntrypointDriftSummary, Snapshot};
use crate::types::FileAnalysis;

const MAX_FINDINGS_QUICK_WINS: usize = 10;

/// Complete findings artifact (`findings.json` in the artifacts dir).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Findings {
    /// Loctree version that generated this
    pub loctree: String,
    /// ISO 8601 timestamp
    pub generated_at: String,
    /// Git commit hash (short)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,

    /// Summary counts for quick overview
    pub summary: FindingsSummary,

    /// Dead exports (unused code)
    pub dead_parrots: Vec<DeadExport>,

    /// Shadow exports (same symbol exported from multiple files, one unused)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shadow_exports: Vec<ShadowExport>,

    /// Circular import cycles
    pub cycles: Vec<CycleEntry>,

    /// Duplicate export groups (twins)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub duplicates: Vec<DuplicateGroup>,

    /// Barrel chaos issues
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub barrel_chaos: Vec<BarrelChaosEntry>,

    /// React-specific lint issues (race conditions, memory leaks)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub react_lint: Vec<ReactLintIssue>,

    /// TypeScript lint issues (any types, ts-ignore)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ts_lint: Vec<TsLintIssue>,

    /// Memory leak issues (outside React hooks)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory_lint: Vec<MemoryLintIssue>,

    /// Prioritized quick-win shortlist; full truth stays in the issue arrays above.
    pub quick_wins: Vec<QuickWin>,

    /// Drift between declared manifest roots and code entrypoints
    #[serde(default, skip_serializing_if = "EntrypointDriftSummary::is_empty")]
    pub entrypoint_drift: EntrypointDriftSummary,

    /// Bundle distribution analysis from source maps
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dist: Option<DistResult>,

    /// Number of dead-export candidates dropped because Layer 3 semantic
    /// analysis classified them as idioms (`usage`, `die`, sourced helpers,
    /// public entrypoints, env vars, …).
    #[serde(default)]
    pub idiom_suppressed: u32,

    /// Number of Makefile auxiliary surfaces collapsed out of target-level
    /// dead-export accounting.
    #[serde(default)]
    pub make_metadata_suppressed: u32,

    /// Number of dead-export candidates dropped because Layer 3 reachability
    /// marked them as reached via shell `case ... esac` dispatch handlers.
    #[serde(default)]
    pub shell_reachable_by_dispatch: u32,
}

/// Summary of all findings for quick health check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingsSummary {
    /// Total files analyzed
    pub files: usize,
    /// Total lines of code
    pub loc: usize,
    /// Health score 0-100 (higher is better)
    pub health_score: u8,
    /// Number of dead parrots
    pub dead_parrots: usize,
    /// Number of shadow exports
    pub shadow_exports: usize,
    /// Number of duplicate export groups
    pub duplicate_groups: usize,
    /// Cycle counts by type
    pub cycles: CycleCounts,
    /// Number of barrel chaos issues
    pub barrel_chaos: usize,
    /// React lint issues summary
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub react_lint: Option<ReactLintSummary>,
    /// TypeScript lint issues summary
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts_lint: Option<TsLintSummary>,
    /// Memory lint issues summary
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_lint: Option<MemoryLintSummary>,
    /// Bundle distribution summary
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dist: Option<FindingsDistSummary>,
    /// Artifact fence: files in the analyzed universe that the detectors
    /// excluded as non-product (vendored/fixture/generated/template).
    /// Always present — the fence never cuts silently.
    #[serde(default)]
    pub excluded: crate::analyzer::classify::ArtifactFenceStats,
}

/// Summary of bundle distribution analysis for quick checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingsDistSummary {
    /// Number of source maps used for analysis
    #[serde(rename = "sourceMaps")]
    pub source_maps: usize,
    /// Number of exports removed from the bundle surface
    #[serde(rename = "treeShakenExports")]
    pub tree_shaken_exports: usize,
    /// Bundle coverage percentage
    #[serde(rename = "coveragePct")]
    pub coverage_pct: usize,
    /// Files impacted by tree-shaking
    #[serde(rename = "impactedFiles")]
    pub impacted_files: usize,
    /// Analysis granularity: file, symbol, or mixed
    #[serde(rename = "analysisLevel")]
    pub analysis_level: String,
}

/// Cycle counts by classification
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CycleCounts {
    /// Hard bidirectional cycles (breaking)
    pub breaking: usize,
    /// Structural cycles (compilable but smelly)
    pub structural: usize,
    /// Diamond dependencies
    pub diamond: usize,
    /// Lazy/dynamic cycles (usually harmless)
    pub lazy: usize,
}

/// Single cycle entry in findings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleEntry {
    /// Type: "breaking", "structural", "diamond", "lazy"
    #[serde(rename = "type")]
    pub cycle_type: String,
    /// Files involved in the cycle
    pub files: Vec<String>,
    /// Suggestion for breaking the cycle
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// Duplicate export group (twins)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateGroup {
    /// Symbol name
    pub symbol: String,
    /// Files exporting this symbol
    pub files: Vec<DuplicateFile>,
    /// Canonical file (most imports)
    pub canonical: String,
    /// Severity: "low", "medium", "high"
    pub severity: String,
    /// Human-readable reason
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Shape-evidence class (additive, W2-b): "EXACT", "SHAPE_SIMILAR",
    /// "NAME_COLLISION", "IDIOM". `None` when the twin classifier produced
    /// no group for this symbol (no shape evidence available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
}

/// File in a duplicate group
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateFile {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import_count: Option<usize>,
}

/// Barrel chaos entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarrelChaosEntry {
    /// Type: "missing_barrel", "deep_chain", "inconsistent_path"
    #[serde(rename = "type")]
    pub chaos_type: String,
    /// Affected path(s)
    pub paths: Vec<String>,
    /// Human-readable description
    pub description: String,
}

/// Quick win: actionable suggestion with high impact
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickWin {
    /// Action: "delete_candidate", "consolidate", "move", "break_cycle".
    /// There is intentionally no "delete" — the detector proposes candidates,
    /// the operator decides.
    pub action: String,
    /// Target file
    pub file: String,
    /// Human-readable reason
    pub reason: String,
    /// Estimated LOC savings (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saves_loc: Option<usize>,
}

/// Configuration for findings production
#[derive(Debug, Clone, Default)]
pub struct FindingsConfig {
    /// High confidence only (skip "smell" level)
    pub high_confidence: bool,
    /// Library mode (ignore examples/demos)
    pub library_mode: bool,
    /// Python library mode
    pub python_library: bool,
    /// Example globs for library mode
    pub example_globs: Vec<String>,
}

impl Findings {
    /// Produce findings from scan results
    pub fn produce(
        scan_results: &ScanResults,
        snapshot: &Snapshot,
        config: FindingsConfig,
        dist: Option<DistResult>,
    ) -> Self {
        let version = env!("CARGO_PKG_VERSION").to_string();
        let generated_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Iso8601::DEFAULT)
            .unwrap_or_else(|_| "unknown".to_string());

        // Get git ref from snapshot metadata
        let git_ref = snapshot.metadata.git_commit.clone();

        // Collect all analyses
        let analyses: Vec<&FileAnalysis> = scan_results.global_analyses.iter().collect();

        // Dead exports — canonical pipeline (single source of the dead number
        // across `loct dead`, `loct twins`, `loct findings`, repo-view):
        // one config, semantic suppression, literal + symbol-graph
        // cross-check, entry-point fence.
        let dead_filter = DeadFilterConfig {
            library_mode: config.library_mode,
            example_globs: config.example_globs.clone(),
            python_library_mode: config.python_library,
            ..canonical_dead_filter(snapshot)
        };
        let dead_truth = compute_dead_truth_with(snapshot, dead_filter, config.high_confidence);
        let dead_parrots = dead_truth.dead;
        let mut suppression = dead_truth.suppression;

        // Top-up: dead_parrots short-circuits some categories before semantic
        // suppression runs:
        //   * Make files exit before the candidate stage entirely.
        //   * Shell symbols invoked by command-word name are dropped via
        //     `shell_called_by_name` before suppression sees them, hiding
        //     `case ... esac` dispatch handlers from the counter.
        // Layer 3 still absorbs those false positives — count them directly
        // from the facts so the audit signal remains truthful when the
        // candidate-driven counters under-report.
        if let Some(facts) = snapshot.semantic_facts.as_ref() {
            suppression.make_metadata_suppressed += count_make_metadata_facts(facts);
            if suppression.shell_reachable_by_dispatch == 0 {
                suppression.shell_reachable_by_dispatch += count_dispatch_handler_facts(facts);
            }
        }

        // Cycles
        let all_edges: Vec<_> = scan_results
            .contexts
            .iter()
            .flat_map(|ctx| ctx.graph_edges.clone())
            .collect();
        let (strict_cycles, lazy_cycles) = find_cycles_with_lazy(&all_edges);

        // Classify strict cycles using ClassifiedCycle::new
        let classified_strict: Vec<ClassifiedCycle> = strict_cycles
            .into_iter()
            .map(|nodes| ClassifiedCycle::new(nodes, &all_edges))
            .collect();
        let cycles = classify_cycles(&classified_strict, &lazy_cycles);

        // Duplicates from scan contexts
        let mut duplicates = collect_duplicates(scan_results);

        // Barrel chaos
        let barrel_analysis = analyze_barrel_chaos(snapshot);
        let barrel_chaos = convert_barrel_chaos(&barrel_analysis);

        // Shadow exports (TODO: implement proper shadow detection)
        let shadow_exports: Vec<ShadowExport> = Vec::new();

        // React lint - analyze React/JSX files for race conditions
        // We need to read file content from disk since FileAnalysis doesn't store content
        let root_path = snapshot
            .metadata
            .roots
            .first()
            .map(std::path::PathBuf::from);

        let react_lint: Vec<ReactLintIssue> = if let Some(root) = &root_path {
            snapshot
                .files
                .iter()
                .filter(|f| {
                    matches!(
                        std::path::Path::new(&f.path)
                            .extension()
                            .and_then(std::ffi::OsStr::to_str),
                        Some("tsx") | Some("jsx") | Some("ts") | Some("js")
                    )
                })
                .flat_map(|f| {
                    let full_path = root.join(&f.path);
                    if let Ok(content) = std::fs::read_to_string(&full_path) {
                        analyze_react_file(&content, &full_path, f.path.clone())
                    } else {
                        Vec::new()
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        // TypeScript lint - analyze TS/TSX files for type safety issues
        let ts_lint: Vec<TsLintIssue> = if let Some(root) = &root_path {
            snapshot
                .files
                .iter()
                .filter(|f| {
                    matches!(
                        std::path::Path::new(&f.path)
                            .extension()
                            .and_then(std::ffi::OsStr::to_str),
                        Some("ts") | Some("tsx")
                    )
                })
                .flat_map(|f| {
                    let full_path = root.join(&f.path);
                    if let Ok(content) = std::fs::read_to_string(&full_path) {
                        lint_ts_file(&full_path, &content)
                    } else {
                        Vec::new()
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        // Memory lint - analyze JS/TS files for memory leak patterns (outside React hooks)
        let memory_lint: Vec<MemoryLintIssue> = if let Some(root) = &root_path {
            snapshot
                .files
                .iter()
                .filter(|f| {
                    matches!(
                        std::path::Path::new(&f.path)
                            .extension()
                            .and_then(std::ffi::OsStr::to_str),
                        Some("ts") | Some("tsx") | Some("js") | Some("jsx")
                    )
                })
                .flat_map(|f| {
                    let full_path = root.join(&f.path);
                    if let Ok(content) = std::fs::read_to_string(&full_path) {
                        lint_memory_file(&full_path, &content)
                    } else {
                        Vec::new()
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        // Quick wins
        // Detect exact twins for health score consistency with for_ai.rs
        // (and as the shape-evidence source for duplicate classification —
        // computed BEFORE quick wins so "consolidate" is gated on class EXACT).
        let analyses_vec: Vec<_> = analyses.iter().cloned().cloned().collect();
        let exact_twins_raw = detect_exact_twins(&analyses_vec, false);

        // Layer 3 — drop convention twins (e.g. two `usage` printers in two
        // scripts) before they pollute consolidation suggestions.
        let exact_twins = match snapshot.semantic_facts.as_ref() {
            Some(facts) => filter_idiom_twins(exact_twins_raw, facts),
            None => exact_twins_raw,
        };

        // Annotate duplicate groups with the twin shape-evidence class
        // (EXACT / SHAPE_SIMILAR / NAME_COLLISION / IDIOM). Symbols without a
        // twin group keep `class: None` — no shape evidence, never EXACT.
        let twin_class_by_name: HashMap<&str, &'static str> = exact_twins
            .iter()
            .map(|t| (t.name.as_str(), t.class.as_str()))
            .collect();
        for dup in &mut duplicates {
            dup.class = twin_class_by_name
                .get(dup.symbol.as_str())
                .map(|c| c.to_string());
        }

        let quick_wins = generate_quick_wins(
            &dead_parrots,
            &cycles,
            &duplicates,
            &barrel_chaos,
            &react_lint,
            &ts_lint,
            &memory_lint,
        );

        // Categorize twins: same-language vs cross-language
        let (twins_same_language, _twins_cross_language): (Vec<_>, Vec<_>) = exact_twins
            .iter()
            .partition(|twin| matches!(categorize_twin(twin), TwinCategory::SameLanguage(_)));

        // Use twins module's find_dead_parrots for consistency with for_ai.rs
        // (this is different from DeadExport dead_parrots - twins dead_parrots are symbols with 0 imports)
        let twins_result = find_dead_parrots(&analyses_vec, false, false);
        let twins_dead_parrots = twins_result.dead_parrots.len();
        let twins_same_lang_count = twins_same_language.len();

        // Count cascade imports for health score consistency with for_ai.rs
        let cascade_imports: usize = scan_results
            .contexts
            .iter()
            .map(|ctx| ctx.cascades.len())
            .sum();

        // Calculate summary
        let summary = calculate_summary(&SummaryInput {
            analyses: &analyses,
            dead_parrots: &dead_parrots,
            shadow_exports: &shadow_exports,
            duplicates: &duplicates,
            cycles: &cycles,
            barrel_chaos: &barrel_chaos,
            react_lint: &react_lint,
            ts_lint: &ts_lint,
            memory_lint: &memory_lint,
            twins_dead_parrots,
            twins_same_language: twins_same_lang_count,
            cascade_imports,
            dist: dist.as_ref(),
        });

        Findings {
            loctree: version,
            generated_at,
            git_ref,
            summary,
            dead_parrots,
            shadow_exports,
            cycles,
            duplicates,
            barrel_chaos,
            react_lint,
            ts_lint,
            memory_lint,
            quick_wins,
            entrypoint_drift: snapshot.metadata.entrypoint_drift.clone(),
            dist,
            idiom_suppressed: suppression.idiom_suppressed,
            make_metadata_suppressed: suppression.make_metadata_suppressed,
            shell_reachable_by_dispatch: suppression.shell_reachable_by_dispatch,
        }
    }

    /// Convert to JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Get summary only (for `loct findings --summary`)
    pub fn summary_only(&self) -> FindingsSummary {
        self.summary.clone()
    }
}

/// Count Make-side files whose semantic facts were collapsed out of findings.
///
/// Makefiles can contain dozens of `.PHONY` helper targets. Reporting every
/// target as a suppressed finding makes build glue look like product code, so
/// this intentionally counts one auxiliary Make surface per file.
fn count_make_metadata_facts(facts: &SemanticFacts) -> u32 {
    let mut files = HashSet::new();
    for (symbol_id, tags) in &facts.idiom_tags {
        let path = symbol_id.split("::").next().unwrap_or("");
        if !is_make_file_path(path) {
            continue;
        }
        let absorbs = tags.iter().any(|tag| {
            matches!(
                tag.classifier,
                SemClassifier::Metadata | SemClassifier::PublicEntrypoint
            )
        });
        if absorbs {
            files.insert(path.to_string());
        }
    }
    files.len() as u32
}

/// Count shell symbols whose Layer 3 reachability reason is a dispatch
/// handler. Used to top up `shell_reachable_by_dispatch` when dead_parrots
/// already filtered shell handlers out via name-based heuristics
/// (`shell_called_by_name`) before suppression saw them.
fn count_dispatch_handler_facts(facts: &SemanticFacts) -> u32 {
    use crate::semantic::ReachReason;
    let mut count: u32 = 0;
    for reason in facts.reachability.reasons.values() {
        if matches!(reason, ReachReason::DispatchHandler { .. }) {
            count += 1;
        }
    }
    count
}

fn is_make_file_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with("/makefile")
        || lower.ends_with("\\makefile")
        || lower == "makefile"
        || lower.ends_with(".mk")
        || lower.ends_with(".make")
}

/// Classify cycles into breaking/structural/diamond/lazy
fn classify_cycles(strict: &[ClassifiedCycle], lazy: &[Vec<String>]) -> Vec<CycleEntry> {
    use crate::analyzer::cycles::CycleClassification;

    let mut entries = Vec::new();

    for cycle in strict {
        let cycle_type = match cycle.classification {
            CycleClassification::HardBidirectional => "breaking",
            CycleClassification::ModuleSelfReference => "structural",
            CycleClassification::TraitBased => "structural",
            CycleClassification::CfgGated => "structural",
            CycleClassification::FanPattern => "diamond",
            CycleClassification::WildcardImport => "structural",
            CycleClassification::Unknown => "structural",
        };

        let suggestion = if cycle_type == "breaking" {
            suggest_cycle_break(&cycle.nodes)
        } else {
            None
        };

        entries.push(CycleEntry {
            cycle_type: cycle_type.to_string(),
            files: cycle.nodes.clone(),
            suggestion,
        });
    }

    // Add lazy cycles
    for nodes in lazy {
        entries.push(CycleEntry {
            cycle_type: "lazy".to_string(),
            files: nodes.clone(),
            suggestion: None,
        });
    }

    entries
}

/// Suggest where to break a cycle
fn suggest_cycle_break(nodes: &[String]) -> Option<String> {
    if nodes.len() < 2 {
        return None;
    }
    // Suggest breaking at the middle edge
    let mid = nodes.len() / 2;
    let from = &nodes[mid];
    let to = &nodes[(mid + 1) % nodes.len()];
    Some(format!("Break at: {} -> {}", from, to))
}

/// Collect duplicates from scan results
fn collect_duplicates(scan_results: &ScanResults) -> Vec<DuplicateGroup> {
    let all_ranked: Vec<&RankedDup> = scan_results
        .contexts
        .iter()
        .flat_map(|ctx| ctx.filtered_ranked.iter())
        .collect();

    all_ranked
        .into_iter()
        .map(|dup| {
            let files: Vec<DuplicateFile> = dup
                .locations
                .iter()
                .map(|loc| DuplicateFile {
                    file: loc.file.clone(),
                    line: loc.line,
                    import_count: None,
                })
                .collect();

            let severity = match dup.severity {
                crate::analyzer::report::DupSeverity::CrossLangExpected => "low",
                crate::analyzer::report::DupSeverity::ReExportOrGeneric => "low",
                crate::analyzer::report::DupSeverity::SamePackage => "medium",
                crate::analyzer::report::DupSeverity::CrossModule => "medium",
                crate::analyzer::report::DupSeverity::CrossCrate => "high",
            };

            DuplicateGroup {
                symbol: dup.name.clone(),
                files,
                canonical: dup.canonical.clone(),
                severity: severity.to_string(),
                reason: if dup.reason.is_empty() {
                    None
                } else {
                    Some(dup.reason.clone())
                },
                // Filled in by `produce` from the twin shape classifier.
                class: None,
            }
        })
        .collect()
}

/// Convert barrel analysis to chaos entries
fn convert_barrel_chaos(analysis: &BarrelAnalysis) -> Vec<BarrelChaosEntry> {
    let mut entries = Vec::new();

    for missing in &analysis.missing_barrels {
        entries.push(BarrelChaosEntry {
            chaos_type: "missing_barrel".to_string(),
            paths: vec![missing.directory.clone()],
            description: format!(
                "Directory with {} files has {} external imports but no barrel file",
                missing.file_count, missing.external_import_count
            ),
        });
    }

    for chain in &analysis.deep_chains {
        entries.push(BarrelChaosEntry {
            chaos_type: "deep_chain".to_string(),
            paths: chain.chain.clone(),
            description: format!(
                "Re-export chain for '{}' is {} levels deep",
                chain.symbol, chain.depth
            ),
        });
    }

    for inconsistent in &analysis.inconsistent_paths {
        let mut paths = vec![inconsistent.canonical_path.clone()];
        paths.extend(
            inconsistent
                .alternative_paths
                .iter()
                .map(|(p, _)| p.clone()),
        );
        entries.push(BarrelChaosEntry {
            chaos_type: "inconsistent_path".to_string(),
            paths,
            description: format!(
                "Symbol '{}' is imported via {} different paths",
                inconsistent.symbol,
                1 + inconsistent.alternative_paths.len()
            ),
        });
    }

    entries
}

/// Generate quick wins from all findings
fn generate_quick_wins(
    dead_parrots: &[DeadExport],
    cycles: &[CycleEntry],
    duplicates: &[DuplicateGroup],
    barrel_chaos: &[BarrelChaosEntry],
    react_lint: &[ReactLintIssue],
    ts_lint: &[TsLintIssue],
    memory_lint: &[MemoryLintIssue],
) -> Vec<QuickWin> {
    let mut wins = Vec::new();

    // Dead parrots are *candidates*, never verdicts: the action is always
    // `delete_candidate` and every candidate carries its cross-check reason.
    // Entry-point fence: runtime entrypoints (Cargo [[bin]], package.json
    // main/bin, shebang, @main) never become delete candidates. Candidates
    // degraded by the literal/symbol-graph cross-check are no longer "high"
    // confidence and stay out of quick wins by the same filter.
    let mut seen_files: HashSet<String> = HashSet::new();
    for dead in dead_parrots {
        if dead.confidence == "high" && !dead.entrypoint && !seen_files.contains(&dead.file) {
            seen_files.insert(dead.file.clone());
            wins.push(QuickWin {
                action: "delete_candidate".to_string(),
                file: dead.file.clone(),
                reason: dead.reason.clone(),
                saves_loc: None, // TODO: Add LOC info to DeadExport
            });
        }
    }

    // Breaking cycles
    for cycle in cycles {
        if cycle.cycle_type == "breaking"
            && let Some(suggestion) = &cycle.suggestion
        {
            wins.push(QuickWin {
                action: "break_cycle".to_string(),
                file: cycle.files.first().cloned().unwrap_or_default(),
                reason: suggestion.clone(),
                saves_loc: None,
            });
        }
    }

    // High severity duplicates — "Consolidate" is earned by shape evidence
    // (twin class EXACT: identical signature fingerprints at every location),
    // never by name equality alone. Name collisions, trait-impl registries
    // and idiomatic constructors stay informational.
    for dup in duplicates {
        if dup.severity == "high" && dup.class.as_deref() == Some("EXACT") {
            wins.push(QuickWin {
                action: "consolidate".to_string(),
                file: dup.canonical.clone(),
                reason: format!(
                    "Consolidate '{}' from {} files",
                    dup.symbol,
                    dup.files.len()
                ),
                saves_loc: None,
            });
        }
    }

    // Missing barrels
    for chaos in barrel_chaos {
        if chaos.chaos_type == "missing_barrel"
            && let Some(dir) = chaos.paths.first()
        {
            wins.push(QuickWin {
                action: "create_barrel".to_string(),
                file: format!("{}/index.ts", dir),
                reason: chaos.description.clone(),
                saves_loc: None,
            });
        }
    }

    // React lint issues (high severity = race conditions)
    let mut react_seen: HashSet<String> = HashSet::new();
    for issue in react_lint {
        if issue.severity == "high" && !react_seen.contains(&issue.file) {
            react_seen.insert(issue.file.clone());
            wins.push(QuickWin {
                action: "fix_race_condition".to_string(),
                file: issue.file.clone(),
                reason: format!("{} (line {})", issue.message, issue.line),
                saves_loc: None,
            });
        }
    }

    // TypeScript lint issues (high severity in prod = any types, ts-ignore)
    let mut ts_seen: HashSet<String> = HashSet::new();
    for issue in ts_lint {
        if issue.severity == "high" && !ts_seen.contains(&issue.file) {
            ts_seen.insert(issue.file.clone());
            wins.push(QuickWin {
                action: "fix_type_safety".to_string(),
                file: issue.file.clone(),
                reason: format!("{} (line {})", issue.message, issue.line),
                saves_loc: None,
            });
        }
    }

    // Memory lint issues (high severity = subscription leaks, global intervals)
    let mut mem_seen: HashSet<String> = HashSet::new();
    for issue in memory_lint {
        if issue.severity == "high" && !mem_seen.contains(&issue.file) {
            mem_seen.insert(issue.file.clone());
            wins.push(QuickWin {
                action: "fix_memory_leak".to_string(),
                file: issue.file.clone(),
                reason: format!("{} (line {})", issue.message, issue.line),
                saves_loc: None,
            });
        }
    }

    // Quick wins stay intentionally short; the full findings remain in the artifact.
    wins.truncate(MAX_FINDINGS_QUICK_WINS);
    wins
}

/// Bundled input for [`calculate_summary`] to stay under the 7-argument clippy limit.
struct SummaryInput<'a> {
    analyses: &'a [&'a FileAnalysis],
    dead_parrots: &'a [DeadExport],
    shadow_exports: &'a [ShadowExport],
    duplicates: &'a [DuplicateGroup],
    cycles: &'a [CycleEntry],
    barrel_chaos: &'a [BarrelChaosEntry],
    react_lint: &'a [ReactLintIssue],
    ts_lint: &'a [TsLintIssue],
    memory_lint: &'a [MemoryLintIssue],
    twins_dead_parrots: usize,
    twins_same_language: usize,
    cascade_imports: usize,
    dist: Option<&'a DistResult>,
}

/// Calculate summary statistics
fn calculate_summary(input: &SummaryInput) -> FindingsSummary {
    let files = input.analyses.len();
    let loc: usize = input.analyses.iter().map(|a| a.loc).sum();

    // Count cycles by type
    let mut cycle_counts = CycleCounts::default();
    for cycle in input.cycles {
        match cycle.cycle_type.as_str() {
            "breaking" => cycle_counts.breaking += 1,
            "structural" => cycle_counts.structural += 1,
            "diamond" => cycle_counts.diamond += 1,
            "lazy" => cycle_counts.lazy += 1,
            _ => {}
        }
    }

    // Vector-based health score with log-normalization (unified with for_ai.rs)
    // Now includes twins metrics for consistency with for_ai.rs health score
    let health_metrics = HealthMetrics {
        // CERTAIN: breaking cycles are critical
        breaking_cycles: cycle_counts.breaking,
        // HIGH: dead exports, twins_dead_parrots
        dead_exports: input.dead_parrots.len(),
        twins_dead_parrots: input.twins_dead_parrots,
        // SMELL: barrel chaos, structural cycles, duplicates, twins_same_language, cascades
        barrel_chaos_count: input.barrel_chaos.len(),
        structural_cycles: cycle_counts.structural,
        duplicate_exports: input.duplicates.len(),
        twins_same_language: input.twins_same_language,
        cascade_imports: input.cascade_imports,
        // Context
        files,
        loc,
        ..Default::default()
    };

    let health = calculate_health_score(&health_metrics);
    let health_score = health.health;

    // React lint summary
    let react_lint_summary = if input.react_lint.is_empty() {
        None
    } else {
        Some(ReactLintSummary::from_issues(input.react_lint))
    };

    // TypeScript lint summary
    let ts_lint_summary = if input.ts_lint.is_empty() {
        None
    } else {
        Some(TsLintSummary::from_issues(input.ts_lint))
    };

    // Memory lint summary
    let memory_lint_summary = if input.memory_lint.is_empty() {
        None
    } else {
        Some(crate::analyzer::memory_lint::calculate_summary(
            input.memory_lint,
        ))
    };

    let dist_summary = input.dist.map(|dist| FindingsDistSummary {
        source_maps: dist.source_maps,
        tree_shaken_exports: dist.tree_shaken_exports,
        coverage_pct: dist.coverage_pct,
        impacted_files: dist.impacted_files.len(),
        analysis_level: dist.analysis_level.as_str().to_string(),
    });

    // Artifact fence accounting: how many analyzed files are non-product
    // (vendored/fixture/generated/template). Detectors exclude these from
    // actionable findings; the summary reports the cut so it is never silent.
    let mut excluded = crate::analyzer::classify::ArtifactFenceStats::default();
    for analysis in input.analyses {
        excluded.record(crate::analyzer::classify::artifact_class(
            &analysis.path,
            None,
        ));
    }

    FindingsSummary {
        files,
        loc,
        health_score,
        dead_parrots: input.dead_parrots.len(),
        shadow_exports: input.shadow_exports.len(),
        duplicate_groups: input.duplicates.len(),
        cycles: cycle_counts,
        barrel_chaos: input.barrel_chaos.len(),
        react_lint: react_lint_summary,
        ts_lint: ts_lint_summary,
        memory_lint: memory_lint_summary,
        dist: dist_summary,
        excluded,
    }
}

// =============================================================================
// Manifest Producer - Index of all artifacts
// =============================================================================

/// Manifest artifact (`manifest.json` in the artifacts dir).
/// AI agents and tooling should read this FIRST to understand what's available.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Loctree version
    pub loctree: String,
    /// ISO 8601 timestamp
    pub generated_at: String,

    /// Project metadata
    pub project: ManifestProject,

    /// Available artifacts
    pub artifacts: ManifestArtifacts,

    /// Available commands
    pub commands: ManifestCommands,

    /// Example queries for quick start
    pub examples: Vec<ManifestExample>,

    /// Optional dist/report surface metadata
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dist: Option<ManifestDist>,
}

/// Project metadata in manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestProject {
    /// Project name (from git remote or directory name)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Detected languages
    pub languages: Vec<String>,
    /// Total files
    pub files: usize,
    /// Total LOC
    pub loc: usize,
}

/// Artifact descriptions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestArtifacts {
    #[serde(rename = "snapshot.json")]
    pub snapshot: ArtifactInfo,
    #[serde(rename = "findings.json")]
    pub findings: ArtifactInfo,
    #[serde(rename = "agent.json")]
    pub agent: ArtifactInfo,
}

/// Single artifact info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactInfo {
    /// Approximate size in KB
    pub size_kb: usize,
    /// Purpose description
    pub purpose: String,
    /// Commands to query this artifact
    pub query_with: Vec<String>,
}

/// Available commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestCommands {
    pub scan: String,
    pub slice: String,
    pub find: String,
    pub jq: String,
}

/// Example query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestExample {
    pub task: String,
    pub cmd: String,
}

/// Dist/report surface summary exposed in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestDist {
    /// Source directory analyzed for bundle coverage
    #[serde(rename = "srcDir")]
    pub src_dir: String,
    /// Analysis granularity: file, symbol, or mixed
    #[serde(rename = "analysisLevel")]
    pub analysis_level: String,
    /// Number of exports removed from the bundle surface
    #[serde(rename = "treeShakenExports")]
    pub tree_shaken_exports: usize,
    /// Bundle coverage percentage
    #[serde(rename = "coveragePct")]
    pub coverage_pct: usize,
}

impl Manifest {
    /// Produce manifest from snapshot metadata
    pub fn produce(
        snapshot: &Snapshot,
        findings_size_kb: usize,
        agent_size_kb: usize,
        dist: Option<&DistResult>,
    ) -> Self {
        let version = env!("CARGO_PKG_VERSION").to_string();
        let generated_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Iso8601::DEFAULT)
            .unwrap_or_else(|_| "unknown".to_string());

        // Project info
        let project = ManifestProject {
            name: snapshot.metadata.git_repo.clone(),
            languages: snapshot.metadata.languages.iter().cloned().collect(),
            files: snapshot.canonical_file_count(),
            loc: snapshot.metadata.total_loc,
        };

        // Estimate snapshot size (rough calculation)
        let snapshot_size_kb = snapshot.files.len() * 2; // ~2KB per file on average

        // Artifacts
        let artifacts = ManifestArtifacts {
            snapshot: ArtifactInfo {
                size_kb: snapshot_size_kb,
                purpose: "Complete analysis graph - imports, exports, LOC per file".to_string(),
                query_with: vec![
                    "loct slice".to_string(),
                    "loct find".to_string(),
                    "loct '<jq>'".to_string(),
                ],
            },
            findings: ArtifactInfo {
                size_kb: findings_size_kb,
                purpose: if dist.is_some() {
                    "All detected issues plus bundle distribution insights".to_string()
                } else {
                    "All detected issues - dead code, cycles, duplicates".to_string()
                },
                query_with: {
                    let mut queries = vec![
                        "loct findings".to_string(),
                        "loct '.dead_parrots'".to_string(),
                    ];
                    if dist.is_some() {
                        queries.push("loct '.dist'".to_string());
                    }
                    queries
                },
            },
            agent: ArtifactInfo {
                size_kb: agent_size_kb,
                purpose: if dist.is_some() {
                    "AI-optimized context bundle with bundle distribution".to_string()
                } else {
                    "AI-optimized context bundle".to_string()
                },
                query_with: {
                    let mut queries = vec!["loct --for-ai".to_string()];
                    if dist.is_some() {
                        queries.push("loct '.bundle.dist'".to_string());
                    }
                    queries
                },
            },
        };

        // Commands
        let commands = ManifestCommands {
            scan: "loct".to_string(),
            slice: "loct slice <file>".to_string(),
            find: "loct find <pattern>".to_string(),
            jq: "loct '<jq-query>'".to_string(),
        };

        // Examples
        let examples = vec![
            ManifestExample {
                task: "Get health score".to_string(),
                cmd: "loct '.summary.health_score'".to_string(),
            },
            ManifestExample {
                task: "List dead exports".to_string(),
                cmd: "loct '.dead_parrots'".to_string(),
            },
            ManifestExample {
                task: "Context for file".to_string(),
                cmd: "loct slice src/App.tsx --json".to_string(),
            },
            ManifestExample {
                task: "Find symbol".to_string(),
                cmd: "loct find UserPreferences".to_string(),
            },
            ManifestExample {
                task: "Count cycles".to_string(),
                cmd: "loct '.cycles | length'".to_string(),
            },
        ];
        let mut examples = examples;
        if dist.is_some() {
            examples.push(ManifestExample {
                task: "Show bundle coverage".to_string(),
                cmd: "loct '.dist.coveragePct'".to_string(),
            });
            examples.push(ManifestExample {
                task: "List tree-shaken exports".to_string(),
                cmd: "loct '.dist.deadExports'".to_string(),
            });
        }

        let dist_summary = dist.map(|dist| ManifestDist {
            src_dir: dist.src_dir.clone(),
            analysis_level: dist.analysis_level.as_str().to_string(),
            tree_shaken_exports: dist.tree_shaken_exports,
            coverage_pct: dist.coverage_pct,
        });

        Manifest {
            loctree: version,
            generated_at,
            project,
            artifacts,
            commands,
            examples,
            dist: dist_summary,
        }
    }

    /// Convert to JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cycle_counts_default() {
        let counts = CycleCounts::default();
        assert_eq!(counts.breaking, 0);
        assert_eq!(counts.structural, 0);
        assert_eq!(counts.diamond, 0);
        assert_eq!(counts.lazy, 0);
    }

    #[test]
    fn test_quick_win_serialization() {
        let win = QuickWin {
            action: "delete_candidate".to_string(),
            file: "src/dead.ts".to_string(),
            reason: "Unused export".to_string(),
            saves_loc: Some(100),
        };
        let json = serde_json::to_string(&win).expect("serialize quick win");
        assert!(json.contains("delete_candidate"));
        assert!(json.contains("src/dead.ts"));
        assert!(json.contains("100"));
    }

    #[test]
    fn test_quick_wins_never_emit_bare_delete_and_always_carry_reason() {
        use crate::analyzer::dead_parrots::DeadExport;

        let dead = vec![
            DeadExport {
                file: "src/dead.ts".to_string(),
                symbol: "unused".to_string(),
                line: Some(3),
                confidence: "high".to_string(),
                reason: "no imports; literal: 0 hits outside def; not an entrypoint".to_string(),
                open_url: None,
                is_test: false,
                action: "delete_candidate".to_string(),
                entrypoint: false,
            },
            // Entry-point fence: must never become a delete candidate.
            DeadExport {
                file: "src/bin/tool.rs".to_string(),
                symbol: "helper".to_string(),
                line: Some(10),
                confidence: "high".to_string(),
                reason: "entry-point fence".to_string(),
                open_url: None,
                is_test: false,
                action: "delete_candidate".to_string(),
                entrypoint: true,
            },
            // Cross-check degraded: stays out of quick wins.
            DeadExport {
                file: "src/spawned.rs".to_string(),
                symbol: "main_loop".to_string(),
                line: Some(1),
                confidence: "low".to_string(),
                reason: "string-literal reference outside def".to_string(),
                open_url: None,
                is_test: false,
                action: "delete_candidate".to_string(),
                entrypoint: false,
            },
        ];

        let wins = generate_quick_wins(&dead, &[], &[], &[], &[], &[], &[]);

        assert!(
            wins.iter().all(|w| w.action != "delete"),
            "quick wins must never emit a bare delete verdict: {:?}",
            wins
        );
        assert!(
            wins.iter().all(|w| !w.reason.trim().is_empty()),
            "every quick-win candidate must carry a reason: {:?}",
            wins
        );
        assert_eq!(wins.len(), 1, "only the clean high-confidence candidate");
        assert_eq!(wins[0].action, "delete_candidate");
        assert_eq!(wins[0].file, "src/dead.ts");
        assert!(
            !wins.iter().any(|w| w.file == "src/bin/tool.rs"),
            "entry-point files must never get delete_candidate"
        );
    }

    #[test]
    fn test_findings_summary_serialization() {
        let summary = FindingsSummary {
            files: 100,
            loc: 10000,
            health_score: 85,
            dead_parrots: 5,
            shadow_exports: 2,
            duplicate_groups: 10,
            cycles: CycleCounts {
                breaking: 0,
                structural: 2,
                diamond: 1,
                lazy: 3,
            },
            barrel_chaos: 3,
            react_lint: None,
            ts_lint: None,
            memory_lint: None,
            dist: None,
            excluded: crate::analyzer::classify::ArtifactFenceStats::default(),
        };
        let json = serde_json::to_string_pretty(&summary).expect("serialize summary");
        assert!(json.contains("\"health_score\": 85"));
        assert!(json.contains("\"breaking\": 0"));
        assert!(
            json.contains("\"excluded\""),
            "summary must always carry the artifact-fence excluded counts"
        );
    }

    #[test]
    fn test_make_metadata_count_collapses_targets_per_file() {
        let mut facts = SemanticFacts::default();
        for target in ["build", "test", "precheck"] {
            facts.idiom_tags.insert(
                format!("Makefile::{target}"),
                vec![crate::semantic::IdiomTag {
                    name: ".PHONY".to_string(),
                    classifier: SemClassifier::PublicEntrypoint,
                    runtime_role: crate::semantic::RuntimeRole::PublicEntrypoint,
                    source: crate::semantic::TagSource::InferredFromCode,
                    reasoning: "phony target".to_string(),
                }],
            );
        }
        facts.idiom_tags.insert(
            "rules.mk::install".to_string(),
            vec![crate::semantic::IdiomTag {
                name: ".PHONY".to_string(),
                classifier: SemClassifier::PublicEntrypoint,
                runtime_role: crate::semantic::RuntimeRole::PublicEntrypoint,
                source: crate::semantic::TagSource::InferredFromCode,
                reasoning: "phony target".to_string(),
            }],
        );

        assert_eq!(count_make_metadata_facts(&facts), 2);
    }

    #[test]
    fn test_suggest_cycle_break() {
        let nodes = vec!["a.ts".to_string(), "b.ts".to_string(), "c.ts".to_string()];
        let suggestion =
            suggest_cycle_break(&nodes).expect("non-empty cycle should suggest a break");
        assert!(suggestion.contains("Break at:"));
    }

    #[test]
    fn test_suggest_cycle_break_empty() {
        let nodes: Vec<String> = vec![];
        assert!(suggest_cycle_break(&nodes).is_none());
    }
}
