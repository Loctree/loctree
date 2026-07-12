//! Custom LSP request: `loctree/health`
//!
//! Repo readiness gate for daemon-mode agents. Returns a 0-100 score,
//! a green/yellow/red status, the cycle / dead-export / twin / hotspot
//! counts that drive that score, snapshot freshness, top risks, and a
//! short list of recommended actions.
//!
//! Plan 09 of the LSP roadmap.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::path::{Path, PathBuf};

use loctree::analysis_reports::{HealthReport, HealthReportOptions, health_report};
use loctree::analyzer::health_score::{HealthMetrics, calculate_health_score};
use loctree::metrics::{importer_counts_direct, top_hubs_by_importers_direct};
use loctree::snapshot::Snapshot;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const HOTSPOT_IMPORTERS_THRESHOLD: usize = 10;
const TOP_RISKS_LIMIT: usize = 8;

/// Parameters for `loctree/health`.
#[derive(Debug, Deserialize, Default, JsonSchema)]
pub struct HealthParams {
    /// Workspace project root override. Reserved for Plan 13
    /// (multi-workspace context); ignored in single-workspace mode.
    #[serde(default)]
    pub project: Option<PathBuf>,
    /// When true, include the `top_risks` array in the response.
    /// Off by default — saves serialization on the hot path.
    #[serde(default)]
    pub include_top_risks: bool,
}

/// One entry in `top_risks`.
#[derive(Debug, Clone, Serialize)]
pub struct RiskItem {
    /// Risk family: `"cycle"` | `"dead_export"` | `"twin"` | `"hotspot"`.
    pub kind: String,
    /// File path most associated with the risk.
    pub file: String,
    /// Severity tag: `"high"` | `"medium"` | `"low"`.
    pub severity: String,
    /// Human-readable summary.
    pub message: String,
}

/// `loctree/health` response payload.
#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    /// Overall health score, 0-100 (higher is better).
    pub health_score: u8,
    /// `green` (≥80), `yellow` (50-79), `red` (<50).
    pub status: String,
    /// Cycle count from `analysis_reports::HealthReport`.
    pub cycles: usize,
    /// Dead-export count.
    pub dead_exports: usize,
    /// Exact-twin count.
    pub twins: usize,
    /// Files with importer count above the hotspot threshold.
    pub hotspots: usize,
    /// True when the snapshot's git commit no longer matches HEAD on disk.
    pub snapshot_stale: bool,
    /// Seconds since `snapshot.metadata.generated_at`. 0 when unparseable.
    pub snapshot_age_seconds: u64,
    /// Optional drill-down. Empty unless `include_top_risks: true`.
    pub top_risks: Vec<RiskItem>,
    /// Suggested next actions a planning agent can show to the operator.
    pub recommended_actions: Vec<String>,
}

/// Map an integer score 0-100 to the green/yellow/red status string.
pub fn status_label(score: u8) -> &'static str {
    if score >= 80 {
        "green"
    } else if score >= 50 {
        "yellow"
    } else {
        "red"
    }
}

/// Build a `HealthMetrics` snapshot from the analyzer's `HealthReport`
/// plus the underlying `Snapshot` for project size context.
fn metrics_from_report(report: &HealthReport, snapshot: &Snapshot) -> HealthMetrics {
    let twin_count = report.twins.total;
    let dead_count = report.dead_exports.total;
    let dead_high = report.dead_exports.high_confidence;
    let cycles_breaking = report.cycles.high_risk;
    let cycles_structural = report.cycles.structural;

    let total_loc: usize = snapshot.files.iter().map(|file| file.loc).sum();
    let files = snapshot.files.len();

    HealthMetrics {
        missing_handlers: 0,
        unregistered_handlers: 0,
        breaking_cycles: cycles_breaking,
        unused_high_confidence: dead_high,
        dead_exports: dead_count,
        twins_dead_parrots: 0,
        twins_same_language: twin_count,
        barrel_chaos_count: 0,
        structural_cycles: cycles_structural,
        cascade_imports: 0,
        duplicate_exports: 0,
        files,
        loc: total_loc,
        certain_items: Vec::new(),
        high_items: Vec::new(),
        smell_items: Vec::new(),
    }
}

/// Count files whose `importers_count >= threshold`. The atlas card uses
/// the same heuristic to flag fan-in hotspots in the Risk Register.
pub fn count_hotspots(snapshot: &Snapshot) -> usize {
    importer_counts_direct(snapshot)
        .values()
        .filter(|&&count| count >= HOTSPOT_IMPORTERS_THRESHOLD)
        .count()
}

/// Seconds since `snapshot.metadata.generated_at`. Returns 0 when the
/// timestamp is missing or unparseable so a stale read doesn't show as
/// negative-time.
pub fn snapshot_age_seconds(snapshot: &Snapshot) -> u64 {
    use chrono::{DateTime, Utc};
    let generated = &snapshot.metadata.generated_at;
    DateTime::parse_from_rfc3339(generated)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
        .map(|dt| Utc::now().signed_duration_since(dt).num_seconds())
        .and_then(|secs| u64::try_from(secs).ok())
        .unwrap_or(0)
}

/// Build the LSP response from the analyzer report + snapshot freshness.
pub fn build_response(
    report: &HealthReport,
    snapshot: &Snapshot,
    snapshot_stale: bool,
    include_top_risks: bool,
) -> HealthResponse {
    let metrics = metrics_from_report(report, snapshot);
    let score = calculate_health_score(&metrics);
    let hotspots = count_hotspots(snapshot);
    let top_risks = if include_top_risks {
        collect_top_risks(report, snapshot)
    } else {
        Vec::new()
    };
    let recommended_actions = recommend_actions(report, snapshot_stale, score.health);

    HealthResponse {
        health_score: score.health,
        status: status_label(score.health).to_string(),
        cycles: report.cycles.total,
        dead_exports: report.dead_exports.total,
        twins: report.twins.total,
        hotspots,
        snapshot_stale,
        snapshot_age_seconds: snapshot_age_seconds(snapshot),
        top_risks,
        recommended_actions,
    }
}

fn collect_top_risks(report: &HealthReport, snapshot: &Snapshot) -> Vec<RiskItem> {
    let mut risks: Vec<RiskItem> = Vec::new();

    if report.cycles.high_risk > 0 {
        risks.push(RiskItem {
            kind: "cycle".into(),
            file: String::new(),
            severity: "high".into(),
            message: format!(
                "{} breaking cycle(s) — fix before refactor",
                report.cycles.high_risk
            ),
        });
    }
    if report.cycles.structural > 0 {
        risks.push(RiskItem {
            kind: "cycle".into(),
            file: String::new(),
            severity: "medium".into(),
            message: format!("{} structural cycle(s)", report.cycles.structural),
        });
    }
    for top_file in report.dead_exports.top_files.iter().take(3) {
        risks.push(RiskItem {
            kind: "dead_export".into(),
            file: top_file.clone(),
            severity: "medium".into(),
            message: format!("dead exports concentrated: {top_file}"),
        });
    }
    for top_group in report.twins.top_groups.iter().take(3) {
        risks.push(RiskItem {
            kind: "twin".into(),
            file: String::new(),
            severity: "low".into(),
            message: format!("twin group: {top_group}"),
        });
    }

    let hotspot_files = top_hotspot_files(snapshot, 3);
    for (file, count) in hotspot_files {
        risks.push(RiskItem {
            kind: "hotspot".into(),
            file,
            severity: "low".into(),
            message: format!("{count} importers — high fan-in"),
        });
    }

    risks.truncate(TOP_RISKS_LIMIT);
    risks
}

fn top_hotspot_files(snapshot: &Snapshot, n: usize) -> Vec<(String, usize)> {
    top_hubs_by_importers_direct(snapshot, usize::MAX)
        .into_iter()
        .filter(|metric| metric.importers_direct >= HOTSPOT_IMPORTERS_THRESHOLD)
        .take(n)
        .map(|metric| (metric.file, metric.importers_direct))
        .collect()
}

fn recommend_actions(report: &HealthReport, stale: bool, score: u8) -> Vec<String> {
    let mut actions: Vec<String> = Vec::new();
    if stale {
        actions.push("Run `loct scan --full-scan` — snapshot is stale".into());
    }
    if report.cycles.high_risk > 0 {
        actions.push("Resolve breaking cycles (`loct cycles --json`)".into());
    }
    if report.dead_exports.high_confidence > 5 {
        actions.push("Prune high-confidence dead exports (`loct dead --confidence high`)".into());
    }
    if report.twins.total > 10 {
        actions.push("Audit twin groups (`loct twins --json`)".into());
    }
    if score < 50 {
        actions.push("Block destructive refactors until score ≥ 50".into());
    }
    actions
}

/// Top-level entry: load report + score against the loaded snapshot.
pub fn compute_health(
    snapshot: &Snapshot,
    root: &Path,
    snapshot_stale: bool,
    params: &HealthParams,
) -> HealthResponse {
    let report = health_report(snapshot, root, HealthReportOptions::default());
    build_response(&report, snapshot, snapshot_stale, params.include_top_risks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_thresholds_match_plan() {
        assert_eq!(status_label(100), "green");
        assert_eq!(status_label(80), "green");
        assert_eq!(status_label(79), "yellow");
        assert_eq!(status_label(50), "yellow");
        assert_eq!(status_label(49), "red");
        assert_eq!(status_label(0), "red");
    }

    fn empty_report() -> HealthReport {
        use loctree::analysis_reports::{HealthCycleSummary, HealthDeadSummary, HealthTwinSummary};
        HealthReport {
            cycles: HealthCycleSummary {
                total: 0,
                high_risk: 0,
                structural: 0,
            },
            dead_exports: HealthDeadSummary {
                total: 0,
                high_confidence: 0,
                low_confidence: 0,
                top_files: Vec::new(),
            },
            twins: HealthTwinSummary {
                total: 0,
                top_groups: Vec::new(),
            },
        }
    }

    #[test]
    fn recommend_actions_silent_on_clean_repo() {
        let actions = recommend_actions(&empty_report(), false, 100);
        assert!(actions.is_empty(), "clean repo should yield no actions");
    }

    #[test]
    fn recommend_actions_flags_stale_snapshot() {
        let actions = recommend_actions(&empty_report(), true, 90);
        assert!(actions.iter().any(|a| a.contains("snapshot is stale")));
    }

    #[test]
    fn recommend_actions_hard_block_under_fifty() {
        let actions = recommend_actions(&empty_report(), false, 30);
        assert!(
            actions
                .iter()
                .any(|a| a.contains("Block destructive refactors"))
        );
    }
}
