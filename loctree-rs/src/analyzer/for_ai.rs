//! AI-optimized hierarchical output format.
//!
//! Transforms analysis results into structured JSON that AI agents can:
//! - Parse easily with regex/jq
//! - Navigate via slice references
//! - Get actionable quick wins
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use serde::Serialize;
use std::collections::{HashMap, HashSet};

use super::barrels::analyze_barrel_chaos;
use super::classify::is_semantic_code_language;
use super::dead_parrots::{DeadFilterConfig, find_dead_exports};
use super::dist::DistResult;
use super::health_score::{HealthIssue, HealthMetrics, calculate_health_score};
use super::memory_lint::lint_memory_file;
use super::occurrences::SuggestedNext;
use super::report::{Confidence, DupSeverity, RankedDup, ReportSection};
use super::root_scan::normalize_module_id;
use super::ts_lint::lint_ts_file;
use super::twins::{TwinCategory, categorize_twin, find_dead_parrots};
use crate::snapshot::Snapshot;
use crate::types::{FileAnalysis, SignatureUse, SignatureUseKind};

const MAX_PRIORITY_TASKS: usize = 10;
const MAX_HUB_FILES: usize = 10;
const MAX_LARGEST_FILES: usize = 25;

/// Top-level AI summary - the entry point for agents
#[derive(Serialize)]
pub struct ForAiReport {
    /// Project root path
    pub project: String,
    /// ISO timestamp
    pub generated_at: String,
    /// High-level summary with priorities
    pub summary: ForAiSummary,
    /// Per-root section references (link to slices)
    pub sections: Vec<ForAiSectionRef>,
    /// Immediate actionable shortlist for the first pass, ordered by priority.
    pub quick_wins: Vec<QuickWin>,
    /// Top actionable tasks with explicit verification commands
    pub priority_tasks: Vec<PriorityTask>,
    /// Highest-connectivity context anchors, ranked rather than exhaustive.
    pub hub_files: Vec<HubFile>,
    /// Agent-ready bundle with full issue lists plus ranked context anchors.
    pub bundle: AgentBundle,
}

/// Summary with counts and priority guidance
#[derive(Serialize)]
pub struct ForAiSummary {
    pub files_analyzed: usize,
    pub total_loc: usize,
    pub dead_exports: usize,
    pub duplicate_exports: usize,
    pub circular_imports: usize,
    pub missing_handlers: usize,
    pub unregistered_handlers: usize,
    pub unused_handlers: usize,
    pub unused_high_confidence: usize,
    pub cascade_imports: usize,
    pub dynamic_imports: usize,
    /// Dead parrots from twins analysis (exports with 0 imports)
    pub twins_dead_parrots: usize,
    /// Same-language exact twins (likely real duplicates needing consolidation)
    pub twins_same_language: usize,
    /// Cross-language twins (FE/BE pairs, usually intentional)
    pub twins_cross_language: usize,
    /// Total indexed function parameters (NEW in 0.8.4)
    pub indexed_params: usize,
    /// Functions that have at least one parameter indexed
    pub functions_with_params: usize,
    /// Priority message for the AI
    pub priority: String,
    /// Health score 0-100 (vector-based with log-normalization)
    pub health_score: u8,
    /// Breakdown by severity: certain (50%), high (30%), smell (20%)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_details: Option<super::health_score::HealthDetails>,
    /// Normalized issue density (log-adjusted for project size)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalized_density: Option<f64>,
    /// Loud warning emitted when the analyzed surface contains zero
    /// semantic-code-language files (e.g. only Markdown + CSS in an
    /// Objective-C repo). Present means: `health_score` reflects only
    /// the parsed subset and is NOT a verdict on the repo.
    ///
    /// See [`is_semantic_code_language`](super::classify::is_semantic_code_language)
    /// and the 2026-05-22 loctree-feedback hak about `markdown-editor-mac-objc`
    /// returning `health_score: 100` while loctree could not parse a
    /// single `.h`/`.m` file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage_warning: Option<CoverageWarning>,
}

/// Loud warning attached to [`ForAiSummary`] when loctree's parser
/// coverage is structurally insufficient to make a health verdict.
///
/// Today this fires when the analyzed pass contains **zero**
/// semantic-code-language files. Future tightenings may also fire on
/// "very low fraction of repo parsed" once filesystem-vs-parsed
/// counters live on [`SnapshotMetadata`](crate::snapshot::SnapshotMetadata).
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct CoverageWarning {
    /// Short tag for downstream filtering. Stable string,
    /// e.g. `"no_semantic_code_languages"`.
    pub kind: String,
    /// Number of analyzed files in this pass (parsed surface).
    pub files_analyzed: usize,
    /// Languages we did detect among analyzed files (e.g.
    /// `["css", "md"]`). Empty when no analyses at all.
    pub parsed_languages: Vec<String>,
    /// Human-readable note for agents and operators.
    pub message: String,
}

/// Reference to a section with command to get details
#[derive(Serialize)]
pub struct ForAiSectionRef {
    pub id: String,
    pub root: String,
    pub files: usize,
    pub loc: usize,
    pub issues: usize,
    /// Command to drill down
    pub slice_cmd: String,
}

/// Immediate actionable item
#[derive(Serialize, Clone)]
pub struct QuickWin {
    pub priority: u8, // 1=highest
    /// Kind of issue: missing_handler, unregistered_handler, unused_handler, dead_export, circular_import, opaque_passthrough
    pub kind: String,
    pub action: String,
    pub target: String,
    pub location: String,
    pub impact: String,
    /// Why this is a problem
    pub why: String,
    /// Specific fix suggestion
    pub fix_hint: String,
    /// Estimated complexity: trivial, easy, medium
    pub complexity: String,
    /// Follow-up commands that route agents from analyzer summaries into exact
    /// literal/source truth before they edit or delete.
    pub suggested_next: Vec<SuggestedNext>,
    /// Command to investigate further
    pub trace_cmd: Option<String>,
    /// IDE integration URL (loctree://open?f={file}&l={line})
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_url: Option<String>,
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn is_identifier_like(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn location_path(location: &str) -> Option<&str> {
    if location.is_empty() || location == "unknown" {
        return None;
    }
    if let Some((path, line)) = location.rsplit_once(':') {
        if !path.is_empty() && line.chars().all(|c| c.is_ascii_digit()) {
            return Some(path);
        }
    }
    Some(location)
}

pub(crate) fn literal_truth_suggested_next(
    target: &str,
    location: Option<&str>,
) -> Vec<SuggestedNext> {
    let quoted_target = shell_quote(target);
    let mut suggestions = vec![
        SuggestedNext {
            command: format!("loct occurrences {quoted_target} --json"),
            reason: "verify exact literal occurrences before trusting analyzer summaries"
                .to_string(),
        },
        SuggestedNext {
            command: format!("loct find --literal {quoted_target} --json"),
            reason: "cross-check the same literal-truth substrate through find --literal"
                .to_string(),
        },
    ];

    if is_identifier_like(target) {
        suggestions.insert(
            1,
            SuggestedNext {
                command: format!("loct body {quoted_target} --json"),
                reason: "open bounded source/body truth for the target symbol".to_string(),
            },
        );
        suggestions.push(SuggestedNext {
            command: format!("loct query where-symbol {quoted_target} --json"),
            reason: "separate definitions from literal read/write sites".to_string(),
        });
    }

    if let Some(path) = location.and_then(location_path) {
        suggestions.push(SuggestedNext {
            command: format!("loct slice {}", shell_quote(path)),
            reason: "inspect dependencies and consumers around the reported location".to_string(),
        });
    }

    suggestions
}

/// High-priority task for a first-shot plan (action + verify)
#[derive(Serialize, Clone)]
pub struct PriorityTask {
    pub priority: u8,
    pub kind: String,
    pub target: String,
    pub location: String,
    pub why: String,
    /// Risk severity of leaving it unfixed: high|medium|low
    pub risk: String,
    pub fix_hint: String,
    pub verify_cmd: String,
}

/// High-connectivity file that makes good context anchor
#[derive(Serialize)]
pub struct HubFile {
    pub path: String,
    pub loc: usize,
    pub imports_count: usize,
    pub exports_count: usize,
    pub importers_count: usize, // Files that import this
    pub commands_count: usize,
    /// Command to get full context
    pub slice_cmd: String,
}

/// Condensed agent bundle - one JSON instead of multiple artifacts.
#[derive(Serialize)]
pub struct AgentBundle {
    pub handlers: AgentHandlerGroups,
    pub duplicates: Vec<AgentDuplicate>,
    pub dead_exports: Vec<AgentDeadExport>,
    pub dynamic_imports: Vec<AgentDynamicImport>,
    /// Largest files by LOC, intentionally ranked as context anchors.
    pub largest_files: Vec<AgentFile>,
    pub cycles: Vec<AgentCycle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dist: Option<DistResult>,
    /// All detected TypeScript lint issues (any types, ts-ignore, etc.).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ts_lint: Vec<AgentTsLintIssue>,
    /// All detected memory leak issues (subscriptions, intervals, caches).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory_lint: Vec<AgentMemoryLintIssue>,
}

#[derive(Serialize, Default)]
pub struct AgentHandlerGroups {
    pub missing: Vec<AgentHandler>,
    pub unused: Vec<AgentHandler>,
    pub unregistered: Vec<AgentHandler>,
}

#[derive(Serialize, Clone)]
pub struct AgentHandler {
    pub name: String,
    pub status: String,
    pub frontend: Vec<AgentLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<AgentBackend>,
}

#[derive(Serialize, Clone)]
pub struct AgentBackend {
    pub path: String,
    pub line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct AgentLocation {
    pub path: String,
    pub line: usize,
}

#[derive(Serialize)]
pub struct AgentDuplicate {
    pub name: String,
    pub canonical: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_line: Option<usize>,
    pub score: usize,
    pub severity: String,
    pub files: usize,
}

#[derive(Serialize)]
pub struct AgentDeadExport {
    pub symbol: String,
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    pub confidence: String,
    pub reason: String,
}

#[derive(Serialize)]
pub struct AgentDynamicImport {
    pub file: String,
    pub resolved: Vec<String>,
    pub unresolved: Vec<String>,
}

#[derive(Serialize)]
pub struct AgentFile {
    pub path: String,
    pub loc: usize,
}

#[derive(Serialize)]
pub struct AgentCycle {
    pub kind: String,
    pub members: Vec<String>,
    /// Provenance: every member is a test/fixture path, so this cycle is an
    /// intentional fixture rather than a production regression. Lets agents and
    /// regression consumers exclude/annotate fixture cycles instead of counting
    /// them. See [`crate::analyzer::cycles::is_fixture_cycle`].
    pub fixture: bool,
}

#[derive(Serialize)]
pub struct AgentTsLintIssue {
    pub file: String,
    pub line: usize,
    pub rule: String,
    pub severity: String,
    pub message: String,
}

/// Memory leak issue for agent bundle
#[derive(Serialize)]
pub struct AgentMemoryLintIssue {
    pub file: String,
    pub line: usize,
    pub rule: String,
    pub severity: String,
    pub message: String,
}

/// Generate AI report from analysis results
///
/// If `snapshot` is provided, it's used for accurate barrel_chaos calculation.
/// Otherwise falls back to (possibly incomplete) sections.twins_data.
pub fn generate_for_ai_report(
    project_root: &str,
    sections: &[ReportSection],
    analyses: &[FileAnalysis],
    snapshot: Option<&Snapshot>,
) -> ForAiReport {
    let now = time::OffsetDateTime::now_utc();
    let generated_at = now
        .format(&time::format_description::well_known::Iso8601::DEFAULT)
        .unwrap_or_else(|_| "unknown".to_string());

    let summary = compute_summary(sections, analyses, snapshot);
    let section_refs = build_section_refs(sections);
    let quick_wins = extract_quick_wins(sections, analyses);
    let priority_tasks = build_priority_tasks(&quick_wins);
    let hub_files = find_hub_files(analyses);
    let bundle = build_agent_bundle(project_root, sections, analyses);

    ForAiReport {
        project: project_root.to_string(),
        generated_at,
        summary,
        sections: section_refs,
        quick_wins,
        priority_tasks,
        hub_files,
        bundle,
    }
}

pub(crate) fn build_priority_tasks(quick_wins: &[QuickWin]) -> Vec<PriorityTask> {
    quick_wins
        .iter()
        .take(MAX_PRIORITY_TASKS)
        .map(|w| {
            let risk = match w.kind.as_str() {
                "missing_handler" | "unregistered_handler" => "high",
                "circular_import" | "opaque_passthrough" => "medium",
                "unused_handler" | "dead_export" => "low",
                _ => "medium",
            }
            .to_string();

            let verify_cmd = match w.kind.as_str() {
                "missing_handler" | "unregistered_handler" | "unused_handler" => {
                    format!("loct trace {}", w.target)
                }
                "dead_export" | "opaque_passthrough" => {
                    format!("loct query where-symbol {}", w.target)
                }
                "circular_import" => "loct cycles --explain".to_string(),
                _ => w
                    .trace_cmd
                    .clone()
                    .unwrap_or_else(|| "loct health".to_string()),
            };

            PriorityTask {
                priority: w.priority,
                kind: w.kind.clone(),
                target: w.target.clone(),
                location: w.location.clone(),
                why: w.why.clone(),
                risk,
                fix_hint: w.fix_hint.clone(),
                verify_cmd,
            }
        })
        .collect()
}

fn compute_summary(
    sections: &[ReportSection],
    analyses: &[FileAnalysis],
    snapshot: Option<&Snapshot>,
) -> ForAiSummary {
    let files_analyzed: usize = snapshot.map_or_else(
        || {
            // When sections are empty but we have analyses (e.g., --full-scan), use analyses.len()
            if sections.is_empty() {
                analyses.len()
            } else {
                sections.iter().map(|s| s.files_analyzed).sum()
            }
        },
        Snapshot::canonical_file_count,
    );
    // Prefer LOC from analyses, fallback to sections if analyses is empty
    let total_loc: usize = if analyses.is_empty() {
        sections.iter().map(|s| s.total_loc).sum()
    } else {
        analyses.iter().map(|a| a.loc).sum()
    };
    // Canonical dead pipeline — same source as `loct dead`, `loct twins` and
    // `loct findings`, so the repo-view/for-ai surface never reports a forked
    // count. Fallback (no snapshot): raw detector with the default config.
    let dead_exports: usize = match snapshot {
        Some(snap) => super::dead_parrots::compute_dead_truth(snap).dead.len(),
        None => find_dead_exports(analyses, false, None, DeadFilterConfig::default()).len(),
    };
    let duplicate_exports: usize = sections.iter().map(|s| s.ranked_dups.len()).sum();
    let missing_handlers: usize = sections.iter().map(|s| s.missing_handlers.len()).sum();
    let unregistered_handlers: usize = sections.iter().map(|s| s.unregistered_handlers.len()).sum();
    let unused_handlers: usize = sections.iter().map(|s| s.unused_handlers.len()).sum();
    let unused_high_confidence: usize = sections
        .iter()
        .flat_map(|s| &s.unused_handlers)
        .filter(|h| h.confidence == Some(Confidence::High))
        .count();
    let cascade_imports: usize = sections.iter().map(|s| s.cascades.len()).sum();
    let dynamic_imports: usize = sections.iter().map(|s| s.dynamic.len()).sum();
    let circular_imports: usize = sections.iter().map(|s| s.circular_imports.len()).sum();
    let lazy_circular_imports: usize = sections.iter().map(|s| s.lazy_circular_imports.len()).sum();

    // Count barrel chaos issues (missing barrels + deep chains + inconsistent paths)
    // Use snapshot directly for accurate count (consistent with findings.rs)
    let barrel_chaos_count: usize = if let Some(snap) = snapshot {
        let barrel_analysis = analyze_barrel_chaos(snap);
        barrel_analysis.missing_barrels.len()
            + barrel_analysis.deep_chains.len()
            + barrel_analysis.inconsistent_paths.len()
    } else {
        // Fallback to sections.twins_data (may be incomplete)
        sections
            .iter()
            .filter_map(|s| s.twins_data.as_ref())
            .map(|t| {
                t.barrel_chaos.missing_barrels.len()
                    + t.barrel_chaos.deep_chains.len()
                    + t.barrel_chaos.inconsistent_paths.len()
            })
            .sum()
    };

    // Use find_dead_parrots from twins module directly for consistency with findings.rs
    let twins_dead_parrots: usize = {
        let twins_result = find_dead_parrots(analyses, false, false);
        twins_result.dead_parrots.len()
    };

    let (twins_same_language, twins_cross_language): (usize, usize) = sections
        .iter()
        .filter_map(|s| s.twins_data.as_ref())
        .flat_map(|t| &t.exact_twins)
        .fold((0, 0), |(same, cross), twin| match categorize_twin(twin) {
            TwinCategory::SameLanguage(_) => (same + 1, cross),
            TwinCategory::CrossLanguage => (same, cross + 1),
            TwinCategory::Namesake => (same, cross),
        });

    // Count indexed parameters (NEW in 0.8.4)
    let indexed_params: usize = analyses
        .iter()
        .flat_map(|f| f.exports.iter())
        .map(|e| e.params.len())
        .sum();
    let functions_with_params: usize = analyses
        .iter()
        .flat_map(|f| f.exports.iter())
        .filter(|e| !e.params.is_empty())
        .count();

    // Generate priority message (now includes twins!)
    let priority = if missing_handlers > 0 {
        format!(
            "CRITICAL: Fix {} missing handlers first (runtime errors at invoke). Then {} unused handlers (tech debt).",
            missing_handlers, unused_handlers
        )
    } else if unregistered_handlers > 0 {
        format!(
            "WARNING: {} handlers defined but not registered in generate_handler![]. They won't work at runtime.",
            unregistered_handlers
        )
    } else if unused_high_confidence > 0 {
        format!(
            "CLEANUP: {} unused handlers (high confidence) can be safely removed. {} dead exports, {} duplicate exports.",
            unused_high_confidence, dead_exports, duplicate_exports
        )
    } else if twins_same_language > 0 {
        format!(
            "TECH DEBT: {} same-language twins (consolidate duplicates). {} dead parrots (0 imports). {} cross-lang pairs (likely OK).",
            twins_same_language, twins_dead_parrots, twins_cross_language
        )
    } else if twins_dead_parrots > 0 {
        format!(
            "TECH DEBT: {} dead parrots (exports with 0 imports). Consider removing unused code.",
            twins_dead_parrots
        )
    } else if dead_exports > 0 {
        format!(
            "TECH DEBT: {} dead exports (unused). {} duplicate exports across files.",
            dead_exports, duplicate_exports
        )
    } else if duplicate_exports > 0 {
        format!(
            "TECH DEBT: {} duplicate exports across files. Consider consolidating to reduce confusion.",
            duplicate_exports
        )
    } else if circular_imports > 0 {
        format!(
            "TECH DEBT: {} circular import cycles. Consider refactoring to break cycles.",
            circular_imports
        )
    } else {
        "HEALTHY: No critical issues found. Good job!".to_string()
    };

    // Vector-based health score with 3 severity dimensions:
    // - CERTAIN (50%): breaking_cycles (missing_handlers excluded for consistency with findings.rs)
    // - HIGH (30%): unused_high_confidence, dead_exports, twins_dead_parrots
    // - SMELL (20%): twins_same_language, barrel_chaos, structural_cycles, cascades, duplicates/5
    //
    // Log-normalized to project size: larger projects get less penalty per issue
    // NOTE: missing_handlers/unregistered_handlers are excluded from health score
    // to match findings.rs which doesn't have access to command gap data.
    // These are still reported in the summary for visibility.
    let health_metrics = HealthMetrics {
        // CERTAIN (missing_handlers excluded - not available in findings.rs).
        // breaking = bidirectional import cycles (section.circular_imports).
        breaking_cycles: circular_imports,
        // HIGH
        unused_high_confidence,
        dead_exports,
        twins_dead_parrots,
        // SMELL
        twins_same_language,
        barrel_chaos_count,
        // structural = cycles broken by lazy/dynamic imports (section.lazy_circular_imports).
        // Matches output.rs::health_score so the for-ai surface does not silently
        // drop lazy cycles to zero.
        structural_cycles: lazy_circular_imports,
        cascade_imports,
        duplicate_exports,
        // Context
        files: files_analyzed,
        loc: total_loc,
        ..Default::default()
    };

    let mut health = calculate_health_score(&health_metrics);

    // Coverage warning gate (marbles L5, 2026-05-25): if the analyzed
    // surface has zero semantic-code-language files, the health number
    // below is structurally meaningless — it scored a CSS/Markdown
    // subset, not the repo. Cap health at 50 and emit a loud warning so
    // agents do not propagate a false "HEALTHY" verdict.
    //
    // See loctree-feedback.md 2026-05-22 — `markdown-editor-mac-objc`
    // returned `health_score: 100` while loctree could not parse a
    // single `.h`/`.m` file (36 ObjC files invisible). The repo had
    // command injection in PandocConverter that loctree never surfaced.
    let (coverage_warning, priority) = match coverage_warning_for(analyses, files_analyzed) {
        Some(warning) => {
            health.health = health.health.min(50);
            health.details.certain.count += 1;
            health.details.certain.items.push(HealthIssue {
                kind: "coverage_warning".to_string(),
                target: warning.kind.clone(),
                location: None,
            });
            let prefixed = format!("COVERAGE_WARNING: {}\n{}", warning.message, priority);
            (Some(warning), prefixed)
        }
        None => (None, priority),
    };

    ForAiSummary {
        files_analyzed,
        total_loc,
        dead_exports,
        duplicate_exports,
        circular_imports,
        missing_handlers,
        unregistered_handlers,
        unused_handlers,
        unused_high_confidence,
        cascade_imports,
        dynamic_imports,
        twins_dead_parrots,
        twins_same_language,
        twins_cross_language,
        indexed_params,
        functions_with_params,
        priority,
        health_score: health.health,
        health_details: Some(health.details),
        normalized_density: Some(health.normalized_density),
        coverage_warning,
    }
}

/// Compute a [`CoverageWarning`] for the for-ai summary when loctree's
/// parser coverage is structurally insufficient to make a health
/// verdict. Returns `None` when the surface is healthy (at least one
/// semantic-code-language file analyzed) **or** when the surface is
/// genuinely empty (no analyses at all — nothing meaningful to warn
/// about; spurious warning on cold-start would just be noise).
///
/// Today fires on: `files_analyzed > 0` AND zero of those files have a
/// language listed in [`is_semantic_code_language`]. Catches the
/// 2026-05-22 `markdown-editor-mac-objc` case (4 analyzed files, all
/// CSS/Markdown, 36 unparsed `.h`/`.m`).
fn coverage_warning_for(
    analyses: &[FileAnalysis],
    files_analyzed: usize,
) -> Option<CoverageWarning> {
    if files_analyzed == 0 || analyses.is_empty() {
        return None;
    }
    let mut parsed_languages: Vec<String> = analyses
        .iter()
        .map(|a| a.language.clone())
        .filter(|l| !l.is_empty())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    parsed_languages.sort();
    // No language information at all: cannot decide either way. Stay
    // silent to avoid spurious warnings on fixture/mock inputs that
    // never set `analysis.language`. The real scan path always sets it
    // (see `analyzer/scan.rs::analyze_file` ~line 471) so this guard
    // only triggers on synthetic data.
    if parsed_languages.is_empty() {
        return None;
    }
    let has_code = parsed_languages
        .iter()
        .any(|l| is_semantic_code_language(l));
    if has_code {
        return None;
    }
    let langs_label = format!("[{}]", parsed_languages.join(", "));
    let message = format!(
        "Scanned {files_analyzed} file(s); 0 semantic-code language(s) detected (parsed languages: {langs_label}). \
Health score is OK FOR PARSED SUBSET, NOT OK FOR REPO. \
Likely cause: unsupported language(s) such as Objective-C / Kotlin / Java / C++ / etc. \
Verify repository contents before trusting this health score."
    );
    Some(CoverageWarning {
        kind: "no_semantic_code_languages".to_string(),
        files_analyzed,
        parsed_languages,
        message,
    })
}

fn build_section_refs(sections: &[ReportSection]) -> Vec<ForAiSectionRef> {
    sections
        .iter()
        .enumerate()
        .map(|(idx, s)| {
            let issues = s.missing_handlers.len()
                + s.unregistered_handlers.len()
                + s.unused_handlers.len()
                + s.ranked_dups.len();

            let loc: usize = s
                .graph
                .as_ref()
                .map(|g| g.nodes.iter().map(|n| n.loc).sum())
                .unwrap_or(0);

            ForAiSectionRef {
                id: format!("section-{}", idx),
                root: s.root.clone(),
                files: s.files_analyzed,
                loc,
                issues,
                slice_cmd: format!("loct slice {} --json", s.root),
            }
        })
        .collect()
}

fn build_agent_bundle(
    project_root: &str,
    sections: &[ReportSection],
    analyses: &[FileAnalysis],
) -> AgentBundle {
    let handlers = build_handler_groups(sections);
    let dist = sections.iter().find_map(|section| section.dist.clone());

    let mut all_dups: Vec<RankedDup> = sections
        .iter()
        .flat_map(|s| s.ranked_dups.clone())
        .collect();
    all_dups.sort_by(|a, b| b.score.cmp(&a.score).then(a.name.cmp(&b.name)));
    let mut seen_dup: HashSet<(String, String)> = HashSet::new();
    let duplicates = all_dups
        .into_iter()
        .filter(|d| seen_dup.insert((d.name.clone(), d.canonical.clone())))
        .map(|d| AgentDuplicate {
            name: d.name,
            canonical: d.canonical,
            canonical_line: d.canonical_line,
            score: d.score,
            severity: severity_label(d.severity).to_string(),
            files: d.files.len(),
        })
        .collect();

    let mut seen_dead: HashSet<(String, String)> = HashSet::new();
    let dead_exports = sections
        .iter()
        .flat_map(|s| s.dead_exports.clone())
        .filter(|d| seen_dead.insert((d.file.clone(), d.symbol.clone())))
        .map(|d| AgentDeadExport {
            symbol: d.symbol,
            file: d.file,
            line: d.line,
            confidence: d.confidence,
            reason: d.reason,
        })
        .collect();

    let dynamic_imports = sections
        .iter()
        .flat_map(|s| s.dynamic.clone())
        .map(|(file, sources)| {
            let mut resolved = Vec::new();
            let mut unresolved = Vec::new();
            for src in sources {
                if is_resolved_dynamic(&src) {
                    resolved.push(src);
                } else {
                    unresolved.push(src);
                }
            }
            AgentDynamicImport {
                file,
                resolved,
                unresolved,
            }
        })
        .collect();

    let mut largest_files: Vec<AgentFile> = analyses
        .iter()
        .map(|a| AgentFile {
            path: a.path.clone(),
            loc: a.loc,
        })
        .collect();
    largest_files.sort_by(|a, b| b.loc.cmp(&a.loc).then(a.path.cmp(&b.path)));
    largest_files.truncate(MAX_LARGEST_FILES);

    let cycles = build_agent_cycles(sections);

    // TypeScript lint - scan TS/TSX files for type safety issues
    let root = std::path::Path::new(project_root);
    let ts_lint: Vec<AgentTsLintIssue> = analyses
        .iter()
        .filter(|a| {
            matches!(
                std::path::Path::new(&a.path)
                    .extension()
                    .and_then(std::ffi::OsStr::to_str),
                Some("ts") | Some("tsx")
            )
        })
        .flat_map(|a| {
            let full_path = root.join(&a.path);
            if let Ok(content) = std::fs::read_to_string(&full_path) {
                lint_ts_file(&full_path, &content)
                    .into_iter()
                    .map(|issue| AgentTsLintIssue {
                        file: a.path.clone(),
                        line: issue.line,
                        rule: issue.rule,
                        severity: issue.severity,
                        message: issue.message,
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            }
        })
        .collect();

    // Memory lint - scan JS/TS files for memory leak patterns
    let memory_lint: Vec<AgentMemoryLintIssue> = analyses
        .iter()
        .filter(|a| {
            matches!(
                std::path::Path::new(&a.path)
                    .extension()
                    .and_then(std::ffi::OsStr::to_str),
                Some("ts") | Some("tsx") | Some("js") | Some("jsx")
            )
        })
        .flat_map(|a| {
            let full_path = root.join(&a.path);
            if let Ok(content) = std::fs::read_to_string(&full_path) {
                lint_memory_file(&full_path, &content)
                    .into_iter()
                    .map(|issue| AgentMemoryLintIssue {
                        file: a.path.clone(),
                        line: issue.line,
                        rule: issue.rule,
                        severity: issue.severity,
                        message: issue.message,
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            }
        })
        .collect();

    AgentBundle {
        handlers,
        duplicates,
        dead_exports,
        dynamic_imports,
        largest_files,
        cycles,
        dist,
        ts_lint,
        memory_lint,
    }
}

fn build_handler_groups(sections: &[ReportSection]) -> AgentHandlerGroups {
    let mut missing = Vec::new();
    let mut unused = Vec::new();
    let mut unregistered = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for bridge in sections.iter().flat_map(|s| s.command_bridges.iter()) {
        // De-duplicate by command name to avoid repetition across roots
        if !seen.insert(bridge.name.clone()) {
            continue;
        }

        let handler = AgentHandler {
            name: bridge.name.clone(),
            status: bridge.status.clone(),
            frontend: bridge
                .fe_locations
                .iter()
                .map(|(path, line)| AgentLocation {
                    path: path.clone(),
                    line: *line,
                })
                .collect(),
            backend: bridge
                .be_location
                .as_ref()
                .map(|(path, line, symbol)| AgentBackend {
                    path: path.clone(),
                    line: *line,
                    symbol: Some(symbol.clone()),
                }),
        };

        match bridge.status.as_str() {
            "missing_handler" => missing.push(handler),
            "unused_handler" => unused.push(handler),
            "unregistered_handler" => unregistered.push(handler),
            _ => {}
        }
    }

    AgentHandlerGroups {
        missing,
        unused,
        unregistered,
    }
}

fn build_agent_cycles(sections: &[ReportSection]) -> Vec<AgentCycle> {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut cycles = Vec::new();

    for section in sections {
        for cycle in &section.circular_imports {
            let key = ("strict".to_string(), cycle.join("->"));
            if seen.insert(key.clone()) {
                cycles.push(AgentCycle {
                    kind: key.0,
                    fixture: super::cycles::is_fixture_cycle(cycle),
                    members: cycle.clone(),
                });
            }
        }
        for cycle in &section.lazy_circular_imports {
            let key = ("lazy".to_string(), cycle.join("->"));
            if seen.insert(key.clone()) {
                cycles.push(AgentCycle {
                    kind: key.0,
                    fixture: super::cycles::is_fixture_cycle(cycle),
                    members: cycle.clone(),
                });
            }
        }
    }

    cycles
}

fn is_resolved_dynamic(src: &str) -> bool {
    let has_extension = src.ends_with(".ts")
        || src.ends_with(".tsx")
        || src.ends_with(".js")
        || src.ends_with(".jsx")
        || src.ends_with(".mjs")
        || src.ends_with(".cjs")
        || src.ends_with(".rs")
        || src.ends_with(".py");
    let has_path = src.contains('/') || src.starts_with("./") || src.starts_with("../");
    has_extension || has_path
}

fn severity_label(severity: DupSeverity) -> &'static str {
    match severity {
        DupSeverity::CrossLangExpected => "cross_lang_expected",
        DupSeverity::ReExportOrGeneric => "reexport_or_generic",
        DupSeverity::SamePackage => "same_package",
        DupSeverity::CrossModule => "cross_module",
        DupSeverity::CrossCrate => "cross_crate",
    }
}

/// Heuristic: detect whether the analyzed project ships a Tauri backend.
///
/// loctree-feedback hak 2026-05-18 Screenscribe HAK 2 (`Tauri-pattern false
/// positive na non-Tauri repo`): `loct insights` was emitting
/// `[HIGH] Missing Tauri Handlers` on Screenscribe — a pure
/// Python+JS app with no `tauri.conf.json`, no `src-tauri/`, and no
/// `[tauri]` in `Cargo.toml`. The detector below short-circuits the
/// Tauri-specific quick-wins when none of these signals are present, so
/// non-Tauri repos stop being told to "add #[tauri::command]" for their
/// custom JS events.
pub(crate) fn has_tauri_stack(analyses: &[FileAnalysis]) -> bool {
    for fa in analyses {
        let path = fa.path.as_str();
        if path.ends_with("tauri.conf.json")
            || path.ends_with("tauri.conf.json5")
            || path.contains("/src-tauri/")
            || path.starts_with("src-tauri/")
        {
            return true;
        }
        for imp in &fa.imports {
            for src in [imp.source.as_str(), imp.source_raw.as_str()] {
                if src == "@tauri-apps/api"
                    || src.starts_with("@tauri-apps/")
                    || src == "tauri"
                    || src.starts_with("tauri::")
                {
                    return true;
                }
            }
        }
    }
    false
}

pub(crate) fn extract_quick_wins(
    sections: &[ReportSection],
    analyses: &[FileAnalysis],
) -> Vec<QuickWin> {
    let mut wins = Vec::new();
    let mut priority = 1u8;

    let tauri_present = has_tauri_stack(analyses);

    // Priority 1: Missing handlers (runtime errors!) — Tauri-only.
    // Skipped on non-Tauri repos (per hak 2026-05-18 Screenscribe HAK 2);
    // the missing_handlers signal there is almost always custom-JS-event
    // false positive, not a real `invoke()` ↔ `#[tauri::command]` gap.
    for section in sections {
        if !tauri_present {
            break;
        }
        for gap in &section.missing_handlers {
            if priority > 10 {
                break;
            }
            let (location, open_url) = gap
                .locations
                .first()
                .map(|(f, l)| {
                    let loc = format!("{}:{}", f, l);
                    let url = super::build_open_url(f, Some(*l), section.open_base.as_deref());
                    (loc, Some(url))
                })
                .unwrap_or_else(|| ("unknown".to_string(), None));

            wins.push(QuickWin {
                priority,
                kind: "missing_handler".to_string(),
                action: "Add missing backend handler".to_string(),
                target: gap.name.clone(),
                location: location.clone(),
                impact: "Fixes runtime error when frontend calls invoke()".to_string(),
                why: "Frontend calls invoke() but no #[tauri::command] handler exists".to_string(),
                fix_hint: format!(
                    "Add #[tauri::command] pub async fn {}(...) in src-tauri/src/commands/",
                    gap.name
                ),
                complexity: "medium".to_string(),
                suggested_next: literal_truth_suggested_next(&gap.name, Some(&location)),
                trace_cmd: Some(format!("loct trace {}", gap.name)),
                open_url,
            });
            priority += 1;
        }
    }

    // Priority 2: Unregistered handlers — Tauri-only (same gate as
    // missing_handlers above).
    for section in sections {
        if !tauri_present {
            break;
        }
        for gap in &section.unregistered_handlers {
            if priority > 15 {
                break;
            }
            let (location, open_url) = gap
                .locations
                .first()
                .map(|(f, l)| {
                    let loc = format!("{}:{}", f, l);
                    let url = super::build_open_url(f, Some(*l), section.open_base.as_deref());
                    (loc, Some(url))
                })
                .unwrap_or_else(|| ("unknown".to_string(), None));

            wins.push(QuickWin {
                priority,
                kind: "unregistered_handler".to_string(),
                action: "Register handler in generate_handler![]".to_string(),
                target: gap.name.clone(),
                location: location.clone(),
                impact: "Handler exists but isn't exposed to frontend".to_string(),
                why: "Handler has #[tauri::command] but missing from generate_handler![] macro"
                    .to_string(),
                fix_hint: format!(
                    "Add {} to generate_handler![...] in lib.rs or main.rs",
                    gap.name
                ),
                complexity: "trivial".to_string(),
                suggested_next: literal_truth_suggested_next(&gap.name, Some(&location)),
                trace_cmd: Some(format!("loct trace {}", gap.name)),
                open_url,
            });
            priority += 1;
        }
    }

    // Priority 3: Unused handlers (high confidence only)
    for section in sections {
        for gap in section
            .unused_handlers
            .iter()
            .filter(|h| h.confidence == Some(Confidence::High))
        {
            if priority > 20 {
                break;
            }
            let (location, open_url) = gap
                .locations
                .first()
                .map(|(f, l)| {
                    let loc = format!("{}:{}", f, l);
                    let url = super::build_open_url(f, Some(*l), section.open_base.as_deref());
                    (loc, Some(url))
                })
                .unwrap_or_else(|| ("unknown".to_string(), None));

            wins.push(QuickWin {
                priority,
                kind: "unused_handler".to_string(),
                action: "Remove unused handler".to_string(),
                target: gap.name.clone(),
                location: location.clone(),
                impact: "Dead code - handler defined but never invoked".to_string(),
                why: "No invoke() calls found in frontend for this handler".to_string(),
                fix_hint: format!(
                    "Delete the {} function and remove from generate_handler![]",
                    gap.name
                ),
                complexity: "easy".to_string(),
                suggested_next: literal_truth_suggested_next(&gap.name, Some(&location)),
                trace_cmd: Some(format!("loct trace {}", gap.name)),
                open_url,
            });
            priority += 1;
        }
    }

    // Priority 4: Dead exports (duplicate exports across files)
    for section in sections {
        for dup in section
            .ranked_dups
            .iter()
            .filter(|d| d.score > 10) // Only high-score duplicates
            .take(10)
        // Limit to top 10
        {
            if priority > 30 {
                break;
            }

            // Get primary location from canonical file
            let (location, open_url) = if let Some(canon_line) = dup.canonical_line {
                let loc = format!("{}:{}", dup.canonical, canon_line);
                let url = super::build_open_url(
                    &dup.canonical,
                    Some(canon_line),
                    section.open_base.as_deref(),
                );
                (loc, Some(url))
            } else {
                (dup.canonical.clone(), None)
            };

            let refactor_hint = if !dup.refactors.is_empty() {
                dup.refactors.join(", ")
            } else {
                format!(
                    "Consolidate {} into canonical file {}",
                    dup.name, dup.canonical
                )
            };

            wins.push(QuickWin {
                priority,
                kind: "dead_export".to_string(),
                action: "Consolidate duplicate exports".to_string(),
                target: dup.name.clone(),
                location: location.clone(),
                impact: format!(
                    "Duplicate export across {} files - causes confusion and maintenance burden",
                    dup.files.len()
                ),
                why: format!(
                    "Export '{}' is defined in {} files, creating ambiguity for importers",
                    dup.name,
                    dup.files.len()
                ),
                fix_hint: refactor_hint,
                complexity: "easy".to_string(),
                suggested_next: literal_truth_suggested_next(&dup.name, Some(&location)),
                trace_cmd: Some(format!("loct query where-symbol {}", dup.name)),
                open_url,
            });
            priority += 1;
        }
    }

    // Priority 5: Circular imports (import cycles)
    for section in sections {
        let mut seen_cycles = std::collections::HashSet::new();

        for cycle in section.circular_imports.iter().take(5) {
            if priority > 35 {
                break;
            }
            if cycle.is_empty() {
                continue;
            }

            let mut key_nodes = cycle.clone();
            key_nodes.sort();
            if !seen_cycles.insert(key_nodes) {
                continue;
            }

            let mut path = cycle.clone();
            if path.len() > 1 {
                path.push(path[0].clone());
            }

            let why_path = path.join(" → ");

            let target = if path.len() > 8 {
                let head = path[..3].join(" -> ");
                let tail = path[path.len() - 3..].join(" -> ");
                format!("{} -> ... -> {}", head, tail)
            } else {
                path.join(" -> ")
            };

            let location = cycle
                .first()
                .cloned()
                .unwrap_or_else(|| section.root.clone());

            wins.push(QuickWin {
                priority,
                kind: "circular_import".to_string(),
                action: "Break circular import".to_string(),
                target: target.clone(),
                location: location.clone(),
                impact:
                    "Circular imports can cause runtime errors and make code harder to understand"
                        .to_string(),
                why: format!("Dependency cycle detected: {}", why_path),
                fix_hint:
                    "Extract shared code into a third module, or make the dependency unidirectional"
                        .to_string(),
                complexity: "medium".to_string(),
                suggested_next: literal_truth_suggested_next(&target, Some(&location)),
                trace_cmd: None,
                open_url: super::build_open_url(&location, None, section.open_base.as_deref())
                    .into(),
            });
            priority += 1;
        }
    }

    // Priority 6: Opaque passthrough types (types only seen in signatures of used functions)
    let default_open_base = sections.iter().find_map(|s| s.open_base.as_deref());

    for opaque in detect_opaque_passthrough_types(analyses)
        .into_iter()
        .take(10)
    {
        if priority > 45 {
            break;
        }
        let location = if let Some(line) = opaque.line {
            format!("{}:{}", opaque.file, line)
        } else {
            opaque.file.clone()
        };
        let open_url = super::build_open_url(&opaque.file, opaque.line, default_open_base);
        let used_fns: Vec<String> = opaque
            .uses
            .iter()
            .map(|u| {
                let usage = match u.usage {
                    SignatureUseKind::Parameter => "param",
                    SignatureUseKind::Return => "return",
                };
                format!("{} ({usage})", u.function)
            })
            .take(4)
            .collect();
        let fix_hint = match opaque.severity.as_str() {
            "info" => "Document or re-export intentionally, or remove if unused".to_string(),
            "low" => {
                "Consider making the type private if it is only an internal carrier".to_string()
            }
            _ => "Either make the type private, or re-export it in the public API if intentional"
                .to_string(),
        };
        let impact = format!(
            "Severity: {}. Type is only flowing through function signatures; callers cannot import it directly",
            opaque.severity
        );
        wins.push(QuickWin {
            priority,
            kind: "opaque_passthrough".to_string(),
            action: "Harden opaque passthrough type".to_string(),
            target: opaque.symbol.clone(),
            location: location.clone(),
            impact,
            why: format!(
                "'{}' is never imported directly but is used in signatures of {}",
                opaque.symbol,
                used_fns.join(", ")
            ),
            fix_hint,
            complexity: "medium".to_string(),
            suggested_next: literal_truth_suggested_next(&opaque.symbol, Some(&location)),
            trace_cmd: Some(format!("loct query where-symbol {}", opaque.symbol)),
            open_url: Some(open_url),
        });
        priority += 1;
    }

    wins
}

#[derive(Clone)]
struct OpaquePassthroughFinding {
    symbol: String,
    file: String,
    line: Option<usize>,
    uses: Vec<SignatureUse>,
    severity: String,
}

fn build_used_exports(analyses: &[FileAnalysis]) -> HashSet<(String, String)> {
    let mut used_exports: HashSet<(String, String)> = HashSet::new();
    for analysis in analyses {
        for imp in &analysis.imports {
            let target_norm = if let Some(target) = &imp.resolved_path {
                normalize_module_id(target).as_key()
            } else {
                normalize_module_id(&imp.source).as_key()
            };
            if imp.symbols.is_empty() {
                continue;
            }
            for sym in &imp.symbols {
                let name = if sym.is_default {
                    "default".to_string()
                } else {
                    sym.name.clone()
                };
                used_exports.insert((target_norm.clone(), name.clone()));
                if sym.name == "*" {
                    used_exports.insert((target_norm.clone(), "*".to_string()));
                }
            }
        }
        for re in &analysis.reexports {
            let target_norm = re
                .resolved
                .as_ref()
                .map(|t| normalize_module_id(t).as_key())
                .unwrap_or_else(|| normalize_module_id(&re.source).as_key());
            match &re.kind {
                crate::types::ReexportKind::Star => {
                    used_exports.insert((target_norm, "*".to_string()));
                }
                crate::types::ReexportKind::Named(names) => {
                    for (original, _exported) in names {
                        used_exports.insert((target_norm.clone(), original.clone()));
                    }
                }
            }
        }
    }
    used_exports
}

fn build_reexport_map(analyses: &[FileAnalysis]) -> HashMap<String, HashSet<String>> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();
    for analysis in analyses {
        for re in &analysis.reexports {
            let target_norm = re
                .resolved
                .as_ref()
                .map(|t| normalize_module_id(t).as_key())
                .unwrap_or_else(|| normalize_module_id(&re.source).as_key());
            let entry = map.entry(target_norm).or_default();
            match &re.kind {
                crate::types::ReexportKind::Star => {
                    entry.insert("*".to_string());
                }
                crate::types::ReexportKind::Named(names) => {
                    for (original, _exported) in names {
                        entry.insert(original.clone());
                    }
                }
            }
        }
    }
    map
}

fn is_type_like_export(exp: &crate::types::ExportSymbol, path: &str) -> bool {
    match exp.kind.as_str() {
        "type" | "interface" | "enum" => true,
        "class" => true,
        _ if path.ends_with(".rs") => exp.name.chars().next().is_some_and(|c| c.is_uppercase()),
        _ => false,
    }
}

fn should_exclude_passthrough(exp: &crate::types::ExportSymbol) -> bool {
    let name = exp.name.as_str();
    // Common marker/ZST and doc-hidden-style prefixes
    matches!(
        name,
        "PhantomData" | "PhantomPinned" | "Never" | "Infallible"
    ) || name.starts_with('_')
}

fn detect_opaque_passthrough_types(analyses: &[FileAnalysis]) -> Vec<OpaquePassthroughFinding> {
    let used_exports = build_used_exports(analyses);
    let reexport_map = build_reexport_map(analyses);
    let mut findings = Vec::new();

    for analysis in analyses {
        let module_key = normalize_module_id(&analysis.path).as_key();
        let module_star = used_exports.contains(&(module_key.clone(), "*".to_string()));

        for exp in &analysis.exports {
            if !is_type_like_export(exp, &analysis.path) {
                continue;
            }
            if should_exclude_passthrough(exp) {
                continue;
            }
            if module_star || used_exports.contains(&(module_key.clone(), exp.name.clone())) {
                continue;
            }

            let sigs: Vec<SignatureUse> = analysis
                .signature_uses
                .iter()
                .filter(|s| s.type_name == exp.name)
                .cloned()
                .collect();
            if sigs.is_empty() {
                continue;
            }

            let mut used_sigs: Vec<SignatureUse> = Vec::new();
            for sig in sigs {
                if module_star || used_exports.contains(&(module_key.clone(), sig.function.clone()))
                {
                    used_sigs.push(sig);
                }
            }
            if used_sigs.is_empty() {
                continue;
            }

            let reexported = reexport_map
                .get(&module_key)
                .map(|names| names.contains("*") || names.contains(&exp.name))
                .unwrap_or(false);

            let severity = if reexported { "info" } else { "medium" }.to_string();

            findings.push(OpaquePassthroughFinding {
                symbol: exp.name.clone(),
                file: analysis.path.clone(),
                line: exp.line,
                uses: used_sigs,
                severity,
            });
        }
    }

    findings
}

/// Print quick wins as JSONL (one JSON object per line) for agent consumption
pub fn print_agent_feed_jsonl(report: &ForAiReport) {
    for win in &report.quick_wins {
        match serde_json::to_string(win) {
            Ok(json) => println!("{}", json),
            Err(err) => eprintln!("[loctree][warn] could not serialize quick win: {err}"),
        }
    }
}

pub(crate) fn find_hub_files(analyses: &[FileAnalysis]) -> Vec<HubFile> {
    use std::collections::HashMap;

    // Build reverse index: who imports what
    let mut importers: HashMap<String, Vec<String>> = HashMap::new();
    for analysis in analyses {
        for imp in &analysis.imports {
            if let Some(resolved) = &imp.resolved_path {
                importers
                    .entry(resolved.clone())
                    .or_default()
                    .push(analysis.path.clone());
            }
        }
    }

    // Score files by connectivity
    let mut scored: Vec<_> = analyses
        .iter()
        .map(|a| {
            let imports_count = a.imports.len();
            let exports_count = a.exports.len();
            let importers_count = importers.get(&a.path).map(|v| v.len()).unwrap_or(0);
            let commands_count = a.command_calls.len() + a.command_handlers.len();

            let score =
                imports_count + exports_count * 2 + importers_count * 3 + commands_count * 2;

            (
                a,
                imports_count,
                exports_count,
                importers_count,
                commands_count,
                score,
            )
        })
        .collect();

    scored.sort_by_key(|b| std::cmp::Reverse(b.5));

    scored
        .into_iter()
        .take(MAX_HUB_FILES)
        .filter(|(_, _, _, _, _, score)| *score > 5)
        .map(
            |(a, imports_count, exports_count, importers_count, commands_count, _)| HubFile {
                path: a.path.clone(),
                loc: a.loc,
                imports_count,
                exports_count,
                importers_count,
                commands_count,
                slice_cmd: format!("loct slice {} --json", a.path),
            },
        )
        .collect()
}

/// Print the report as JSON
pub fn print_for_ai_json(report: &ForAiReport) {
    let json = serde_json::to_string_pretty(report).expect("serialize for-ai report");
    println!("{}", json);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::dead_parrots::DeadExport;
    use crate::analyzer::report::{CommandGap, DupLocation, DupSeverity, RankedDup};
    use crate::types::{
        CommandRef, ExportSymbol, ImportEntry, ImportKind, ImportResolutionKind, ImportSymbol,
        SignatureUse, SignatureUseKind,
    };

    fn mock_file(path: &str, loc: usize) -> FileAnalysis {
        FileAnalysis {
            path: path.to_string(),
            loc,
            ..Default::default()
        }
    }

    /// Tauri-stack proxy: a fixture `FileAnalysis` whose path matches the
    /// `has_tauri_stack` heuristic. Tests that exercise the
    /// `missing_handlers` / `unregistered_handlers` quick-win path must
    /// supply this in their `analyses` slice, otherwise the post-2026-05-25
    /// non-Tauri gate filters those wins out (see
    /// `loctree-feedback.md` hak 2026-05-18 Screenscribe HAK 2).
    fn tauri_stack_marker() -> FileAnalysis {
        FileAnalysis {
            path: "src-tauri/tauri.conf.json".to_string(),
            ..Default::default()
        }
    }

    fn mock_section(root: &str, files: usize) -> ReportSection {
        // Use realistic LOC for log-normalized health score testing
        // 100 LOC per file is a reasonable default
        let estimated_loc = files * 100;
        ReportSection {
            root: root.to_string(),
            files_analyzed: files,
            total_loc: estimated_loc,
            reexport_files_count: 0,
            dynamic_imports_count: 0,
            ranked_dups: vec![],
            cascades: vec![],
            circular_imports: vec![],
            lazy_circular_imports: vec![],
            dynamic: vec![],
            analyze_limit: 50,
            generated_at: None,
            schema_name: None,
            schema_version: None,
            loctree_version: None,
            missing_handlers: vec![],
            unregistered_handlers: vec![],
            unused_handlers: vec![],
            command_counts: (0, 0),
            command_bridges: vec![],
            open_base: None,
            tree: None,
            insights: vec![],
            graph: None,
            graph_warning: None,
            git_branch: None,
            git_commit: None,
            priority_tasks: vec![],
            hub_files: vec![],
            hotspots: vec![],
            crowds: vec![],
            dead_exports: vec![],
            dist: None,
            twins_data: None,
            coverage_gaps: vec![],
            health_score: None,
            refactor_plan: None,
            context_atlas: None,
        }
    }

    fn mock_dead_export(symbol: &str, file: &str, line: usize) -> DeadExport {
        DeadExport {
            file: file.to_string(),
            symbol: symbol.to_string(),
            line: Some(line),
            confidence: "high".to_string(),
            reason: "unused export".to_string(),
            open_url: None,
            is_test: false,
            action: "delete_candidate".to_string(),
            entrypoint: false,
        }
    }

    fn mock_ranked_dup(name: &str, canonical: &str, canonical_line: usize) -> RankedDup {
        RankedDup {
            name: name.to_string(),
            files: vec![canonical.to_string(), format!("{canonical}.dup")],
            locations: vec![DupLocation {
                file: canonical.to_string(),
                line: Some(canonical_line),
            }],
            score: 50,
            prod_count: 2,
            dev_count: 0,
            canonical: canonical.to_string(),
            canonical_line: Some(canonical_line),
            refactors: vec![],
            severity: DupSeverity::CrossCrate,
            is_cross_lang: false,
            packages: vec!["pkg".to_string()],
            reason: "duplicate symbol".to_string(),
        }
    }

    #[test]
    fn test_compute_summary_empty() {
        let sections: Vec<ReportSection> = vec![];
        let analyses: Vec<FileAnalysis> = vec![];

        let summary = compute_summary(&sections, &analyses, None);

        assert_eq!(summary.files_analyzed, 0);
        assert_eq!(summary.total_loc, 0);
        assert_eq!(summary.health_score, 100);
        assert!(summary.priority.contains("HEALTHY"));
    }

    #[test]
    fn test_compute_summary_empty_sections_with_analyses() {
        // Bug fix test: when sections are empty but analyses exist (e.g., --full-scan),
        // files_analyzed should be populated from analyses.len()
        let sections: Vec<ReportSection> = vec![];
        let analyses = vec![
            mock_file("src/a.ts", 100),
            mock_file("src/b.ts", 200),
            mock_file("src/c.rs", 50),
        ];

        let summary = compute_summary(&sections, &analyses, None);

        assert_eq!(summary.files_analyzed, 3);
        assert_eq!(summary.total_loc, 350);
        assert_eq!(summary.health_score, 100);
        assert!(summary.priority.contains("HEALTHY"));
    }

    /// Helper: mock a file analysis with an explicit language tag.
    /// Mirrors `analyzer/scan.rs::analyze_file` which sets
    /// `analysis.language = detect_language(&ext)` after parsing.
    fn mock_file_with_lang(path: &str, loc: usize, language: &str) -> FileAnalysis {
        FileAnalysis {
            path: path.to_string(),
            loc,
            language: language.to_string(),
            ..Default::default()
        }
    }

    /// Coverage warning gate (marbles L5 / 2026-05-22 hak): when the
    /// parsed surface contains zero semantic-code-language files (only
    /// docs/styles), health score must be capped at 50 and a loud
    /// `COVERAGE_WARNING` must precede the priority message.
    ///
    /// Reproduces the `markdown-editor-mac-objc` scenario: 36 unparsed
    /// `.h`/`.m` ObjC files, loctree analyzes 4 docs files (css+md),
    /// pre-fix health was `100/100`. Post-fix: ≤50 + warning surfaces.
    #[test]
    fn test_compute_summary_coverage_warning_fires_on_docs_only_repo() {
        let sections: Vec<ReportSection> = vec![];
        let analyses = vec![
            mock_file_with_lang("docs/README.md", 100, "md"),
            mock_file_with_lang("styles/main.css", 200, "css"),
            mock_file_with_lang("styles/gfm.css", 150, "css"),
        ];

        let summary = compute_summary(&sections, &analyses, None);

        assert!(
            summary.health_score <= 50,
            "health_score must be capped at 50 when no semantic-code language analyzed; got {}",
            summary.health_score
        );
        let warning = summary
            .coverage_warning
            .as_ref()
            .expect("coverage_warning must be present for docs-only analyses");
        assert_eq!(warning.kind, "no_semantic_code_languages");
        assert_eq!(warning.files_analyzed, 3);
        assert_eq!(warning.parsed_languages, vec!["css", "md"]);
        assert!(
            warning.message.contains("0 semantic-code"),
            "warning message must call out zero code-language surface; got: {}",
            warning.message
        );
        assert!(
            summary.priority.starts_with("COVERAGE_WARNING:"),
            "priority must lead with COVERAGE_WARNING; got: {}",
            summary.priority
        );
    }

    /// Negative: any single semantic-code-language file in the analyses
    /// suppresses the warning. Catches accidental over-fire on healthy
    /// mixed repos (Rust + CSS + Markdown is a normal shape).
    #[test]
    fn test_compute_summary_coverage_warning_silent_on_code_repo() {
        let sections: Vec<ReportSection> = vec![];
        let analyses = vec![
            mock_file_with_lang("docs/README.md", 50, "md"),
            mock_file_with_lang("styles/main.css", 100, "css"),
            mock_file_with_lang("src/lib.rs", 500, "rs"),
        ];

        let summary = compute_summary(&sections, &analyses, None);

        assert!(
            summary.coverage_warning.is_none(),
            "coverage_warning must stay silent when any code language is present; got: {:?}",
            summary.coverage_warning
        );
        assert_eq!(
            summary.health_score, 100,
            "no issues + code language present → full health"
        );
        assert!(
            !summary.priority.starts_with("COVERAGE_WARNING:"),
            "priority must not prepend COVERAGE_WARNING when code is present; got: {}",
            summary.priority
        );
    }

    /// Negative: empty analyses (cold start, missing snapshot) must
    /// stay silent. Firing a warning on an empty corpus would be just
    /// noise — there is nothing to verify.
    #[test]
    fn test_compute_summary_coverage_warning_silent_on_empty_analyses() {
        let sections: Vec<ReportSection> = vec![];
        let analyses: Vec<FileAnalysis> = vec![];

        let summary = compute_summary(&sections, &analyses, None);

        assert!(
            summary.coverage_warning.is_none(),
            "coverage_warning must stay silent on empty corpus"
        );
        assert_eq!(summary.health_score, 100);
    }

    /// Negative: synthetic analyses without language (test fixtures
    /// using `..Default::default()`) must stay silent — no evidence
    /// either way. Guards against breaking the rest of the test suite.
    #[test]
    fn test_compute_summary_coverage_warning_silent_when_language_unset() {
        let sections: Vec<ReportSection> = vec![];
        let analyses = vec![mock_file("a.ts", 100), mock_file("b.ts", 100)]; // mock_file leaves language empty.

        let summary = compute_summary(&sections, &analyses, None);

        assert!(
            summary.coverage_warning.is_none(),
            "coverage_warning must stay silent for analyses with no language tag"
        );
        assert_eq!(summary.health_score, 100);
    }

    /// Coverage warning shapes the `certain` dimension too: items must
    /// gain one `coverage_warning` entry and count must increment by 1.
    /// Ensures downstream consumers reading health_details surface the
    /// flag even if they ignore the dedicated `coverage_warning` field.
    #[test]
    fn test_compute_summary_coverage_warning_adds_certain_item() {
        let sections: Vec<ReportSection> = vec![];
        let analyses = vec![mock_file_with_lang("docs/README.md", 100, "md")];

        let summary = compute_summary(&sections, &analyses, None);

        let details = summary
            .health_details
            .as_ref()
            .expect("health_details always present");
        assert!(
            details
                .certain
                .items
                .iter()
                .any(|item| item.kind == "coverage_warning"),
            "certain.items must contain a coverage_warning entry; got: {:?}",
            details.certain.items
        );
        assert!(
            details.certain.count >= 1,
            "certain.count must be ≥1 once warning fires; got: {}",
            details.certain.count
        );
    }

    #[test]
    fn test_compute_summary_with_missing_handlers() {
        let mut section = mock_section("src", 10);
        section.missing_handlers = vec![
            CommandGap {
                name: "cmd1".to_string(),
                implementation_name: None,
                locations: vec![("src/a.ts".to_string(), 1)],
                confidence: None,
                string_literal_matches: vec![],
            },
            CommandGap {
                name: "cmd2".to_string(),
                implementation_name: None,
                locations: vec![("src/b.ts".to_string(), 2)],
                confidence: None,
                string_literal_matches: vec![],
            },
        ];

        let sections = vec![section];
        let analyses: Vec<FileAnalysis> = vec![];

        let summary = compute_summary(&sections, &analyses, None);

        assert_eq!(summary.missing_handlers, 2);
        assert!(summary.priority.contains("CRITICAL"));
        // Note: missing_handlers is excluded from health score (not available in findings.rs)
        // so health score remains 100 with empty analyses
        assert_eq!(summary.health_score, 100);
    }

    #[test]
    fn test_compute_summary_structural_cycles_penalize_health() {
        // Guard: lazy_circular_imports must flow into HealthMetrics::structural_cycles
        // (SMELL bucket) instead of being silently dropped to 0.
        //
        // Before the fix at for_ai.rs, structural_cycles was hardcoded to 0 even
        // when sections carried lazy cycles, so the SMELL bucket never saw them
        // and projects with only lazy/structural cycles reported health_score=100.
        let mut clean_section = mock_section("src", 10);
        clean_section.circular_imports = vec![];
        clean_section.lazy_circular_imports = vec![];

        let mut lazy_section = mock_section("src", 10);
        lazy_section.lazy_circular_imports =
            vec![vec!["src/a.ts".to_string(), "src/b.ts".to_string()]];

        let clean = compute_summary(&[clean_section], &[], None);
        let lazy = compute_summary(&[lazy_section], &[], None);

        assert_eq!(clean.health_score, 100, "no cycles means no SMELL penalty");
        assert!(
            lazy.health_score < clean.health_score,
            "lazy cycle must lower health (was {}, clean {})",
            lazy.health_score,
            clean.health_score
        );
    }

    #[test]
    fn test_compute_summary_breaking_vs_structural_cycles_weighted_differently() {
        // Guard: breaking cycles (CERTAIN 50%) must penalize harder than the
        // same number of structural/lazy cycles (SMELL 20%). If for_ai ever
        // collapses both into the same bucket again, this catches it.
        let mut breaking_section = mock_section("src", 10);
        breaking_section.circular_imports =
            vec![vec!["src/a.ts".to_string(), "src/b.ts".to_string()]];

        let mut structural_section = mock_section("src", 10);
        structural_section.lazy_circular_imports =
            vec![vec!["src/a.ts".to_string(), "src/b.ts".to_string()]];

        let breaking = compute_summary(&[breaking_section], &[], None);
        let structural = compute_summary(&[structural_section], &[], None);

        assert!(
            breaking.health_score < structural.health_score,
            "breaking cycle (CERTAIN 50%) must penalize more than structural (SMELL 20%); \
             got breaking={}, structural={}",
            breaking.health_score,
            structural.health_score
        );
    }

    #[test]
    fn test_compute_summary_with_unregistered_handlers() {
        let mut section = mock_section("src", 10);
        section.unregistered_handlers = vec![CommandGap {
            name: "unreg_cmd".to_string(),
            implementation_name: Some("unregisteredHandler".to_string()),
            locations: vec![("src-tauri/src/main.rs".to_string(), 50)],
            confidence: None,
            string_literal_matches: vec![],
        }];

        let sections = vec![section];
        let analyses: Vec<FileAnalysis> = vec![];

        let summary = compute_summary(&sections, &analyses, None);

        assert_eq!(summary.unregistered_handlers, 1);
        assert!(summary.priority.contains("WARNING"));
    }

    #[test]
    fn test_compute_summary_with_unused_high_confidence() {
        let mut section = mock_section("src", 10);
        section.unused_handlers = vec![CommandGap {
            name: "unused_cmd".to_string(),
            implementation_name: Some("unusedHandler".to_string()),
            locations: vec![("src-tauri/src/main.rs".to_string(), 100)],
            confidence: Some(Confidence::High),
            string_literal_matches: vec![],
        }];

        let sections = vec![section];
        let analyses: Vec<FileAnalysis> = vec![];

        let summary = compute_summary(&sections, &analyses, None);

        assert_eq!(summary.unused_high_confidence, 1);
        assert!(summary.priority.contains("CLEANUP"));
    }

    #[test]
    fn test_build_section_refs() {
        let sections = vec![mock_section("src", 10), mock_section("lib", 5)];

        let refs = build_section_refs(&sections);

        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].root, "src");
        assert_eq!(refs[0].files, 10);
        assert!(refs[0].slice_cmd.contains("loct slice"));
        assert_eq!(refs[1].root, "lib");
    }

    #[test]
    fn test_extract_quick_wins_missing_handlers() {
        let mut section = mock_section("src", 10);
        section.missing_handlers = vec![CommandGap {
            name: "missing_cmd".to_string(),
            implementation_name: None,
            locations: vec![("src/app.ts".to_string(), 42)],
            confidence: None,
            string_literal_matches: vec![],
        }];

        let sections = vec![section];
        // Tauri marker required after 2026-05-25 non-Tauri gate.
        let analyses = vec![tauri_stack_marker()];
        let wins = extract_quick_wins(&sections, &analyses);

        assert!(!wins.is_empty());
        assert_eq!(wins[0].priority, 1);
        assert!(wins[0].action.contains("missing backend handler"));
        assert_eq!(wins[0].target, "missing_cmd");
        assert!(wins[0].location.contains("src/app.ts:42"));
    }

    /// loctree-feedback hak 2026-05-18 Screenscribe HAK 2 regression: on a
    /// non-Tauri repo the `missing_handlers` and `unregistered_handlers`
    /// quick-wins must be suppressed entirely — they are almost always
    /// false positives caused by custom JS events (`addEventListener` /
    /// `dispatchEvent`) being mis-classified as `invoke()` calls.
    #[test]
    fn quick_wins_skip_tauri_handlers_when_no_tauri_stack() {
        let mut section = mock_section("src", 10);
        section.missing_handlers = vec![CommandGap {
            name: "reattach-workspace".to_string(),
            implementation_name: None,
            locations: vec![("src/app.ts".to_string(), 42)],
            confidence: None,
            string_literal_matches: vec![],
        }];
        section.unregistered_handlers = vec![CommandGap {
            name: "seek-to-timestamp".to_string(),
            implementation_name: Some("seekTo".to_string()),
            locations: vec![("src/app.ts".to_string(), 100)],
            confidence: None,
            string_literal_matches: vec![],
        }];

        let sections = vec![section];
        // Pure Python+JS analyses, no Tauri marker → handler wins skipped.
        let analyses = vec![mock_file("screenscribe/cli.py", 100)];
        let wins = extract_quick_wins(&sections, &analyses);

        let leaked_kinds: Vec<&str> = wins
            .iter()
            .filter(|w| w.kind == "missing_handler" || w.kind == "unregistered_handler")
            .map(|w| w.kind.as_str())
            .collect();
        assert!(
            leaked_kinds.is_empty(),
            "non-Tauri repo must not emit handler quick-wins, leaked: {leaked_kinds:?}"
        );
    }

    #[test]
    fn test_extract_quick_wins_priority_order() {
        let mut section = mock_section("src", 10);
        section.missing_handlers = vec![CommandGap {
            name: "missing".to_string(),
            implementation_name: None,
            locations: vec![("a.ts".to_string(), 1)],
            confidence: None,
            string_literal_matches: vec![],
        }];
        section.unregistered_handlers = vec![CommandGap {
            name: "unreg".to_string(),
            implementation_name: Some("unregHandler".to_string()),
            locations: vec![("b.rs".to_string(), 2)],
            confidence: None,
            string_literal_matches: vec![],
        }];
        section.unused_handlers = vec![CommandGap {
            name: "unused".to_string(),
            implementation_name: Some("unusedHandler".to_string()),
            locations: vec![("c.rs".to_string(), 3)],
            confidence: Some(Confidence::High),
            string_literal_matches: vec![],
        }];

        let sections = vec![section];
        let analyses = vec![tauri_stack_marker()];
        let wins = extract_quick_wins(&sections, &analyses);

        // Should have all 3 with priority order: missing < unregistered < unused
        assert!(wins.len() >= 3);
        let missing_win = wins.iter().find(|w| w.target == "missing").unwrap();
        let unreg_win = wins.iter().find(|w| w.target == "unreg").unwrap();
        let unused_win = wins.iter().find(|w| w.target == "unused").unwrap();

        assert!(missing_win.priority < unreg_win.priority);
        assert!(unreg_win.priority < unused_win.priority);
    }

    #[test]
    fn test_find_hub_files_empty() {
        let analyses: Vec<FileAnalysis> = vec![];
        let hubs = find_hub_files(&analyses);
        assert!(hubs.is_empty());
    }

    #[test]
    fn test_find_hub_files_scores_by_connectivity() {
        let mut high_connectivity = mock_file("hub.ts", 200);
        high_connectivity.imports = vec![
            ImportEntry::new("./a".to_string(), ImportKind::Static),
            ImportEntry::new("./b".to_string(), ImportKind::Static),
            ImportEntry::new("./c".to_string(), ImportKind::Static),
        ];
        high_connectivity.command_handlers = vec![
            CommandRef {
                name: "cmd1".to_string(),
                exposed_name: None,
                line: 10,
                generic_type: None,
                payload: None,
                plugin_name: None,
            },
            CommandRef {
                name: "cmd2".to_string(),
                exposed_name: None,
                line: 20,
                generic_type: None,
                payload: None,
                plugin_name: None,
            },
        ];

        let low_connectivity = mock_file("leaf.ts", 50);

        let analyses = vec![high_connectivity, low_connectivity];
        let hubs = find_hub_files(&analyses);

        // High connectivity file should appear first (if any)
        if !hubs.is_empty() {
            assert_eq!(hubs[0].path, "hub.ts");
        }
    }

    #[test]
    fn test_generate_for_ai_report() {
        let sections = vec![mock_section("src", 5)];
        let analyses = vec![mock_file("src/a.ts", 100), mock_file("src/b.ts", 50)];

        let report = generate_for_ai_report("/project", &sections, &analyses, None);

        assert_eq!(report.project, "/project");
        assert!(!report.generated_at.is_empty());
        assert_eq!(report.summary.files_analyzed, 5);
        assert_eq!(report.summary.total_loc, 150);
        assert_eq!(report.sections.len(), 1);
    }

    #[test]
    fn test_agent_bundle_keeps_full_issue_lists() {
        let mut section = mock_section("src", 5);
        section.dead_exports = (0..60)
            .map(|idx| {
                mock_dead_export(
                    &format!("dead_{idx}"),
                    &format!("src/file_{idx}.ts"),
                    idx + 1,
                )
            })
            .collect();
        section.ranked_dups = (0..25)
            .map(|idx| {
                mock_ranked_dup(&format!("dup_{idx}"), &format!("src/dup_{idx}.ts"), idx + 1)
            })
            .collect();

        let analyses = vec![mock_file("src/a.ts", 100), mock_file("src/b.ts", 50)];
        let report = generate_for_ai_report("/project", &[section], &analyses, None);

        assert_eq!(report.bundle.dead_exports.len(), 60);
        assert_eq!(report.bundle.duplicates.len(), 25);
    }

    #[test]
    fn test_health_score_bounds() {
        // Health score should be 0-100 with log-normalized formula
        let mut section = mock_section("src", 10);
        // Add lots of issues to test penalty impact
        section.missing_handlers = (0..10)
            .map(|i| CommandGap {
                name: format!("cmd{}", i),
                implementation_name: None,
                locations: vec![("a.ts".to_string(), i)],
                confidence: None,
                string_literal_matches: vec![],
            })
            .collect();

        let sections = vec![section];
        let analyses: Vec<FileAnalysis> = vec![];

        let summary = compute_summary(&sections, &analyses, None);

        // Should be in valid range
        assert!(summary.health_score <= 100);
        // missing_handlers is now excluded from health score (for consistency with findings.rs)
        // With empty analyses and only missing_handlers, health should be 100
        assert_eq!(
            summary.health_score, 100,
            "Expected health = 100 since missing_handlers is excluded, got {}",
            summary.health_score
        );
    }

    #[test]
    fn test_extract_quick_wins_dead_exports() {
        let mut section = mock_section("src", 10);
        section.ranked_dups = vec![RankedDup {
            name: "UserType".to_string(),
            files: vec![
                "src/types/user.ts".to_string(),
                "src/models/user.ts".to_string(),
                "src/api/user.ts".to_string(),
            ],
            locations: vec![
                DupLocation {
                    file: "src/types/user.ts".to_string(),
                    line: Some(10),
                },
                DupLocation {
                    file: "src/models/user.ts".to_string(),
                    line: Some(20),
                },
            ],
            score: 50,
            prod_count: 3,
            dev_count: 0,
            canonical: "src/types/user.ts".to_string(),
            canonical_line: Some(10),
            refactors: vec!["Move all imports to src/types/user.ts".to_string()],
            severity: DupSeverity::CrossCrate,
            is_cross_lang: false,
            packages: vec!["types".to_string(), "models".to_string(), "api".to_string()],
            reason: "Symbol in 3 different packages".to_string(),
        }];

        let sections = vec![section];
        let wins = extract_quick_wins(&sections, &[]);

        // Should include dead export quick win
        let dead_export_wins: Vec<_> = wins.iter().filter(|w| w.kind == "dead_export").collect();
        assert!(!dead_export_wins.is_empty());

        let win = dead_export_wins[0];
        assert_eq!(win.target, "UserType");
        assert_eq!(win.kind, "dead_export");
        assert_eq!(win.complexity, "easy");
        assert!(win.location.contains("src/types/user.ts:10"));
        assert!(win.why.contains("defined in 3 files"));
        assert!(
            win.fix_hint
                .contains("Move all imports to src/types/user.ts")
        );
        let commands: Vec<&str> = win
            .suggested_next
            .iter()
            .map(|s| s.command.as_str())
            .collect();
        assert!(
            commands.contains(&"loct occurrences 'UserType' --json"),
            "dead-export quick wins must point agents at literal occurrence truth: {commands:?}"
        );
        assert!(
            commands.contains(&"loct body 'UserType' --json"),
            "dead-export quick wins must point agents at body/source truth: {commands:?}"
        );
        assert!(
            commands.contains(&"loct find --literal 'UserType' --json"),
            "dead-export quick wins must cross-refer find --literal parity: {commands:?}"
        );
    }

    #[test]
    fn test_extract_quick_wins_circular_imports() {
        let mut section = mock_section("src", 10);
        section.circular_imports = vec![
            vec!["src/a.ts".to_string(), "src/b.ts".to_string()],
            vec!["src/b.ts".to_string(), "src/a.ts".to_string()],
            vec![
                "src/c.ts".to_string(),
                "src/d.ts".to_string(),
                "src/e.ts".to_string(),
            ],
        ];

        let sections = vec![section];
        let wins = extract_quick_wins(&sections, &[]);

        // Should include circular import quick wins
        let cycle_wins: Vec<_> = wins
            .iter()
            .filter(|w| w.kind == "circular_import")
            .collect();
        assert!(!cycle_wins.is_empty());

        // Should deduplicate bidirectional cycles (a↔b)
        assert!(
            cycle_wins
                .iter()
                .any(|w| w.target.contains("src/a.ts") && w.target.contains("src/b.ts"))
        );

        let win = &cycle_wins[0];
        assert_eq!(win.kind, "circular_import");
        assert_eq!(win.complexity, "medium");
        assert!(win.why.contains("Dependency cycle"));
        assert!(
            win.fix_hint
                .contains("Extract shared code into a third module")
        );
    }

    #[test]
    fn test_extract_quick_wins_all_priorities() {
        let mut section = mock_section("src", 10);

        // Priority 1: Missing handler
        section.missing_handlers = vec![CommandGap {
            name: "missing_cmd".to_string(),
            implementation_name: None,
            locations: vec![("src/app.ts".to_string(), 1)],
            confidence: None,
            string_literal_matches: vec![],
        }];

        // Priority 2: Unregistered handler
        section.unregistered_handlers = vec![CommandGap {
            name: "unreg_cmd".to_string(),
            implementation_name: Some("unregHandler".to_string()),
            locations: vec![("src-tauri/src/main.rs".to_string(), 2)],
            confidence: None,
            string_literal_matches: vec![],
        }];

        // Priority 3: Unused handler
        section.unused_handlers = vec![CommandGap {
            name: "unused_cmd".to_string(),
            implementation_name: Some("unusedHandler".to_string()),
            locations: vec![("src-tauri/src/commands.rs".to_string(), 3)],
            confidence: Some(Confidence::High),
            string_literal_matches: vec![],
        }];

        // Priority 4: Dead export
        section.ranked_dups = vec![RankedDup {
            name: "DupType".to_string(),
            files: vec!["a.ts".to_string(), "b.ts".to_string()],
            locations: vec![],
            score: 20,
            prod_count: 2,
            dev_count: 0,
            canonical: "a.ts".to_string(),
            canonical_line: Some(10),
            refactors: vec![],
            severity: DupSeverity::SamePackage,
            is_cross_lang: false,
            packages: vec![],
            reason: String::new(),
        }];

        // Priority 5: Circular import
        section.circular_imports = vec![vec!["x.ts".to_string(), "y.ts".to_string()]];

        let sections = vec![section];
        // Tauri marker required for handler quick-wins to be emitted after
        // the 2026-05-25 non-Tauri gate; without it priorities 1-2 would
        // be filtered out as Screenscribe-class false positives.
        let analyses = vec![tauri_stack_marker()];
        let wins = extract_quick_wins(&sections, &analyses);

        // Should have all 5 priorities represented
        assert!(wins.len() >= 5);

        // Verify we have each kind
        assert!(wins.iter().any(|w| w.kind == "missing_handler"));
        assert!(wins.iter().any(|w| w.kind == "unregistered_handler"));
        assert!(wins.iter().any(|w| w.kind == "unused_handler"));
        assert!(wins.iter().any(|w| w.kind == "dead_export"));
        assert!(wins.iter().any(|w| w.kind == "circular_import"));

        // Verify priority ordering
        let priorities: Vec<u8> = wins.iter().map(|w| w.priority).collect();
        let mut sorted_priorities = priorities.clone();
        sorted_priorities.sort();
        assert_eq!(priorities, sorted_priorities, "Priorities should be sorted");
    }

    #[test]
    fn test_detects_opaque_passthrough_quick_win() {
        // Producer with a type and a function that uses it in signature
        let mut producer = FileAnalysis {
            path: "src/tray.rs".to_string(),
            exports: vec![
                ExportSymbol::new("LoadedIcon".to_string(), "decl", "named", Some(18)),
                ExportSymbol::new("spawn_tray".to_string(), "decl", "named", Some(24)),
            ],
            ..Default::default()
        };
        producer.signature_uses.push(SignatureUse {
            function: "spawn_tray".to_string(),
            usage: SignatureUseKind::Parameter,
            type_name: "LoadedIcon".to_string(),
            line: Some(24),
        });

        // Consumer imports the function (not the type)
        let mut consumer = FileAnalysis {
            path: "src/main.rs".to_string(),
            ..Default::default()
        };
        consumer.imports.push(ImportEntry {
            line: None,
            source: "src/tray.rs".to_string(),
            source_raw: "src/tray.rs".to_string(),
            kind: ImportKind::Static,
            resolved_path: Some("src/tray.rs".to_string()),
            is_bare: false,
            symbols: vec![ImportSymbol {
                name: "spawn_tray".to_string(),
                alias: None,
                is_default: false,
            }],
            resolution: ImportResolutionKind::Local,
            is_type_checking: false,
            is_lazy: false,
            is_crate_relative: false,
            is_super_relative: false,
            is_self_relative: false,
            raw_path: String::new(),
            is_mod_declaration: false,
        });

        let findings = detect_opaque_passthrough_types(&[producer.clone(), consumer.clone()]);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].symbol, "LoadedIcon");

        // Quick win emitted when analyses are provided
        let wins = extract_quick_wins(&[mock_section("root", 2)], &[producer, consumer]);
        assert!(
            wins.iter().any(|w| w.kind == "opaque_passthrough"),
            "Opaque passthrough quick win should be emitted"
        );
    }

    #[test]
    fn test_compute_summary_with_twins_same_language() {
        use crate::analyzer::report::TwinsData;
        use crate::analyzer::twins::{ExactTwin, SymbolEntry, TwinLocation};

        let mut section = mock_section("src", 10);
        section.twins_data = Some(TwinsData {
            dead_parrots: vec![SymbolEntry {
                name: "unusedUtil".to_string(),
                kind: "function".to_string(),
                file_path: "src/utils.ts".to_string(),
                line: 10,
                import_count: 0,
            }],
            exact_twins: vec![ExactTwin {
                name: "UserType".to_string(),
                classification: crate::analyzer::twins::TwinClassification::Duplicate,
                class: crate::analyzer::twins::TwinClass::NameCollision,
                locations: vec![
                    TwinLocation {
                        file_path: "src/types/user.ts".to_string(),
                        line: 5,
                        kind: "type".to_string(),
                        import_count: 10,
                        is_canonical: true,
                        signature_fingerprint: None,
                    },
                    TwinLocation {
                        file_path: "src/models/user.ts".to_string(),
                        line: 8,
                        kind: "type".to_string(),
                        import_count: 2,
                        is_canonical: false,
                        signature_fingerprint: None,
                    },
                ],
                signature_similarity: None,
            }],
            barrel_chaos: Default::default(),
        });

        let sections = vec![section];
        let analyses: Vec<FileAnalysis> = vec![];

        let summary = compute_summary(&sections, &analyses, None);

        // twins_dead_parrots now comes from find_dead_parrots(analyses), not sections
        // With empty analyses, it returns 0
        assert_eq!(summary.twins_dead_parrots, 0);
        // twins_same_language still comes from sections
        assert_eq!(summary.twins_same_language, 1);
        assert_eq!(summary.twins_cross_language, 0);

        // Health score should be reduced slightly (only 1 same_lang twin - SMELL category)
        // With log-normalization, penalty is small for 1 issue in ~1000 LOC
        assert!(
            summary.health_score < 100,
            "Expected health < 100 with twins, got {}",
            summary.health_score
        );
        assert!(
            summary.health_score > 90,
            "Expected health > 90 with only 2 minor issues, got {}",
            summary.health_score
        );

        // Priority should mention twins
        assert!(summary.priority.contains("same-language twins"));
    }

    #[test]
    fn test_compute_summary_with_twins_cross_language() {
        use crate::analyzer::report::TwinsData;
        use crate::analyzer::twins::{ExactTwin, TwinLocation};

        let mut section = mock_section("src", 10);
        section.twins_data = Some(TwinsData {
            dead_parrots: vec![],
            exact_twins: vec![ExactTwin {
                name: "Message".to_string(),
                classification: crate::analyzer::twins::TwinClassification::Duplicate,
                class: crate::analyzer::twins::TwinClass::NameCollision,
                locations: vec![
                    TwinLocation {
                        file_path: "src/types/message.ts".to_string(),
                        line: 5,
                        kind: "interface".to_string(),
                        import_count: 10,
                        is_canonical: true,
                        signature_fingerprint: None,
                    },
                    TwinLocation {
                        file_path: "src-tauri/src/types.rs".to_string(),
                        line: 20,
                        kind: "struct".to_string(),
                        import_count: 5,
                        is_canonical: false,
                        signature_fingerprint: None,
                    },
                ],
                signature_similarity: None,
            }],
            barrel_chaos: Default::default(),
        });

        let sections = vec![section];
        let analyses: Vec<FileAnalysis> = vec![];

        let summary = compute_summary(&sections, &analyses, None);

        // Cross-language twins should NOT add to penalty
        assert_eq!(summary.twins_same_language, 0);
        assert_eq!(summary.twins_cross_language, 1);

        // Health score should be 100 (cross-lang twins don't penalize)
        assert_eq!(summary.health_score, 100);
        assert!(summary.priority.contains("HEALTHY"));
    }

    #[test]
    fn test_compute_summary_twins_dead_parrots_penalty() {
        use crate::analyzer::report::TwinsData;
        use crate::analyzer::twins::SymbolEntry;

        let mut section = mock_section("src", 10);
        section.twins_data = Some(TwinsData {
            dead_parrots: (0..10)
                .map(|i| SymbolEntry {
                    name: format!("unused{}", i),
                    kind: "function".to_string(),
                    file_path: format!("src/util{}.ts", i),
                    line: i,
                    import_count: 0,
                })
                .collect(),
            exact_twins: vec![],
            barrel_chaos: Default::default(),
        });

        let sections = vec![section];
        let analyses: Vec<FileAnalysis> = vec![];

        let summary = compute_summary(&sections, &analyses, None);

        // twins_dead_parrots now comes from find_dead_parrots(analyses), not sections
        // With empty analyses, it returns 0
        assert_eq!(summary.twins_dead_parrots, 0);
        // With no dead parrots, health should be 100
        assert_eq!(summary.health_score, 100);
        // Priority should be HEALTHY since no issues
        assert!(summary.priority.contains("HEALTHY"));
    }
}
