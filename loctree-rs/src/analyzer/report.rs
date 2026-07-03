use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::barrels::BarrelAnalysis;
use super::crowd::types::Crowd;
use super::dead_parrots::DeadExport;
use super::dist::DistResult;
use crate::refactor_plan::{Move, PlanStats, RefactorPhase, RefactorPlan, Shim};

/// Confidence level for dead export and handler detection.
///
/// CERTAIN - Will definitely break/is definitely unused
///   - Unregistered handlers (has #[tauri::command] but NOT in invoke_handler![])
///   - Missing handlers (FE calls invoke() but no handler exists)
///
/// HIGH - Very likely unused, worth fixing
///   - Export with 0 imports across all scanned files
///   - Handler registered but 0 invoke() calls found
///
/// SMELL - Worth checking, might be intentional
///   - Twins (same name in multiple files)
///   - Low import count relative to codebase size
///   - String literal matches found (may be used dynamically)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Confidence {
    /// CERTAIN - Will definitely break/is definitely unused
    Certain,
    /// HIGH - Very likely unused, worth fixing
    High,
    /// SMELL - Worth checking, might be intentional
    Smell,
}

impl Confidence {
    /// Get indicator for this confidence level
    pub fn indicator(&self) -> &'static str {
        match self {
            Confidence::Certain => "[!!]",
            Confidence::High => "[!]",
            Confidence::Smell => "[?]",
        }
    }
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Confidence::Certain => write!(f, "CERTAIN"),
            Confidence::High => write!(f, "HIGH"),
            Confidence::Smell => write!(f, "SMELL"),
        }
    }
}

/// A string literal match in frontend code that might indicate dynamic usage.
#[derive(Clone, Debug, Serialize)]
pub struct StringLiteralMatch {
    pub file: String,
    pub line: usize,
    pub context: String, // "allowlist", "const", "object_key", "array_item"
}

#[derive(Clone, Serialize)]
pub struct CommandGap {
    pub name: String,
    pub implementation_name: Option<String>,
    pub locations: Vec<(String, usize)>,
    /// Confidence level (None for missing handlers, Some for unused handlers)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
    /// String literal matches that may indicate dynamic usage
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub string_literal_matches: Vec<StringLiteralMatch>,
}

#[derive(Clone, Serialize)]
pub struct AiInsight {
    pub title: String,
    pub severity: String,
    pub message: String,
}

// Re-export canonical graph types from report-leptos
// These are the same types used by the HTML report renderer
pub use report_leptos::types::{GraphComponent, GraphData, GraphNode};

/// Location of a duplicate export with line number
#[derive(Clone, Serialize)]
pub struct DupLocation {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

/// Severity levels for duplicate exports
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DupSeverity {
    /// Cross-language expected (Rust↔TS DTOs) - noise
    CrossLangExpected = 0,
    /// Re-exports and generic names (new, from, clone) - usually OK
    ReExportOrGeneric = 1,
    /// Same-package duplicate - potential issue
    #[default]
    SamePackage = 2,
    /// Same symbol in different modules/packages - worth reviewing
    CrossModule = 3,
    /// Same symbol in different crates/packages - REAL issue
    CrossCrate = 4,
}

#[derive(Clone, Serialize)]
pub struct RankedDup {
    pub name: String,
    pub files: Vec<String>,
    /// Locations with line numbers (file, line)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub locations: Vec<DupLocation>,
    pub score: usize,
    pub prod_count: usize,
    pub dev_count: usize,
    pub canonical: String,
    /// Line number in canonical file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_line: Option<usize>,
    pub refactors: Vec<String>,
    /// Severity level: 0=cross-lang expected, 1=same-package, 2=semantic conflict
    #[serde(default)]
    pub severity: DupSeverity,
    /// True if duplicate spans multiple languages (Rust↔TS)
    #[serde(default)]
    pub is_cross_lang: bool,
    /// Distinct packages/directories containing this symbol
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<String>,
    /// Explanation for the severity classification
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// Full command bridge for FE↔BE comparison table.
/// Represents a single command with all its frontend calls and backend handler.
#[derive(Clone, Serialize)]
pub struct CommandBridge {
    /// Command name (exposed_name from Tauri)
    pub name: String,
    /// Frontend call locations (file, line)
    pub fe_locations: Vec<(String, usize)>,
    /// Backend handler location (file, line, impl_symbol) - None if missing
    pub be_location: Option<(String, usize, String)>,
    /// Status: "ok", "missing_handler", "unused_handler", "unregistered_handler"
    pub status: String,
    /// Language (ts, rs, etc.)
    pub language: String,
    /// Communication pattern: "invoke" | "invoke+emit" | "emit-only"
    #[serde(default)]
    pub comm_type: String,
    /// Events emitted by this command's handler
    #[serde(default)]
    pub emits_events: Vec<String>,
}

/// High-priority task for a first-shot plan (action + verify).
#[derive(Clone, Serialize)]
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

/// High-connectivity file that makes a good context anchor.
#[derive(Clone, Serialize)]
pub struct HubFile {
    pub path: String,
    pub loc: usize,
    pub imports_count: usize,
    pub exports_count: usize,
    pub importers_count: usize,
    pub commands_count: usize,
    pub slice_cmd: String,
}

#[derive(Clone, Serialize)]
pub struct HotspotFile {
    pub file: String,
    pub importers: usize,
    pub category: String,
    pub slice_cmd: String,
}

#[derive(Clone, Default, Serialize)]
pub struct TreeNode {
    pub path: String,
    pub loc: usize,
    #[serde(default)]
    pub children: Vec<TreeNode>,
}

#[derive(Serialize)]
pub struct ReportSection {
    pub root: String,
    pub files_analyzed: usize,
    pub total_loc: usize,
    pub reexport_files_count: usize,
    pub dynamic_imports_count: usize,
    pub ranked_dups: Vec<RankedDup>,
    pub cascades: Vec<(String, String)>,
    /// Actual circular import components (normalized)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub circular_imports: Vec<Vec<String>>,
    /// Lazy circular imports (broken by lazy imports inside functions)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lazy_circular_imports: Vec<Vec<String>>,
    pub dynamic: Vec<(String, Vec<String>)>,
    pub analyze_limit: usize,
    /// Report generation time (RFC3339)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
    /// Schema name for artifact payload
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    /// Schema version for artifact payload
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
    /// Loctree CLI/library version that produced this report (provenance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loctree_version: Option<String>,
    pub missing_handlers: Vec<CommandGap>,
    /// Backend handlers that exist (`#[tauri::command]`) but are never
    /// registered via `tauri::generate_handler![...]`.
    pub unregistered_handlers: Vec<CommandGap>,
    pub unused_handlers: Vec<CommandGap>,
    pub command_counts: (usize, usize),
    /// Full command bridges for FE↔BE comparison table
    pub command_bridges: Vec<CommandBridge>,
    pub open_base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<Vec<TreeNode>>,
    pub graph: Option<GraphData>,
    pub graph_warning: Option<String>,
    pub insights: Vec<AiInsight>,
    pub git_branch: Option<String>,
    pub git_commit: Option<String>,
    /// Top actionable tasks (why + fix + verify)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub priority_tasks: Vec<PriorityTask>,
    /// High-connectivity context anchors
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hub_files: Vec<HubFile>,
    /// Import fan-in hotspots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hotspots: Vec<HotspotFile>,
    /// Crowd analysis results (naming collision detection)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crowds: Vec<Crowd>,
    /// Dead exports (exported but never imported)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dead_exports: Vec<DeadExport>,
    /// Bundle distribution analysis (source-map-backed tree-shaking view)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dist: Option<DistResult>,
    /// Twins analysis data (dead parrots, exact twins, barrel chaos)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twins_data: Option<TwinsData>,
    /// Test coverage gaps (handlers/events without tests)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coverage_gaps: Vec<super::coverage_gaps::CoverageGap>,
    /// Overall health score 0-100 (higher is better)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_score: Option<u8>,
    /// Refactor plan data (architectural reorganization suggestions)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refactor_plan: Option<RefactorPlanForReport>,
    /// Context Atlas pointer (when `loct auto` materialized navigable cards
    /// under `<artifacts_dir>/context-atlas/`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_atlas: Option<ContextAtlasInfo>,
}

/// Lightweight pointer to a materialized Context Atlas surface.
///
/// Mirrors a subset of `ContextAtlasManifest` for HTML report rendering.
/// Full manifest stays on disk; the report only needs enough to render a
/// "start here" panel.
#[derive(Clone, Serialize)]
pub struct ContextAtlasInfo {
    /// Absolute path to the materialized atlas directory.
    pub atlas_dir: String,
    /// Absolute path to the human-readable manifest (`manifest.md`).
    pub manifest: String,
    /// Absolute path to the machine-readable manifest (`manifest.json`).
    pub manifest_json: String,
    /// Absolute path to the recommended first card (`00-core-map.md`).
    pub recommended_start: String,
    /// One-line summary suitable for top-of-report rendering.
    pub message: String,
    /// Atlas card pointers for the recommended reading path.
    #[serde(default)]
    pub cards: Vec<ContextAtlasCardInfo>,
}

/// Metadata for a single Context Atlas card (mirrors atlas.rs ContextAtlasCard).
#[derive(Clone, Serialize)]
pub struct ContextAtlasCardInfo {
    pub id: String,
    pub title: String,
    pub path: String,
    pub lines: usize,
    pub why: String,
}

/// Twins analysis data for the HTML report
#[derive(Clone, Serialize)]
pub struct TwinsData {
    /// Dead parrots (0 imports) - uses SymbolEntry from twins module
    pub dead_parrots: Vec<super::twins::SymbolEntry>,
    /// Exact twins (same symbol exported from multiple files)
    pub exact_twins: Vec<super::twins::ExactTwin>,
    /// Barrel analysis (missing barrels, deep chains, inconsistent paths)
    pub barrel_chaos: BarrelAnalysis,
}

// ============================================================================
// Refactor Plan Report Types
// 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team
// ============================================================================

/// A single file move formatted for the HTML report.
#[derive(Clone, Default, Serialize)]
pub struct RefactorMoveForReport {
    /// Source file path
    pub source: String,
    /// Target file path
    pub target: String,
    /// Current architectural layer
    pub current_layer: String,
    /// Target architectural layer
    pub target_layer: String,
    /// Risk level (low, medium, high)
    pub risk: String,
    /// Lines of code in file
    pub loc: usize,
    /// Number of direct consumers (importers)
    pub direct_consumers: usize,
    /// Reason for move suggestion
    pub reason: String,
    /// Verification command
    pub verify_cmd: String,
}

/// A shim suggestion formatted for the HTML report.
#[derive(Clone, Default, Serialize)]
pub struct RefactorShimForReport {
    /// Original file path (where shim will be created)
    pub old_path: String,
    /// New file path (where code was moved)
    pub new_path: String,
    /// Number of importers that would need updating
    pub importer_count: usize,
    /// Generated shim code (pub use statement)
    pub code: String,
}

/// A phase in the refactor execution plan formatted for HTML report.
#[derive(Clone, Default, Serialize)]
pub struct RefactorPhaseForReport {
    /// Phase name (e.g., "Phase 1: LOW Risk")
    pub name: String,
    /// Risk level for this phase
    pub risk: String,
    /// Moves in this phase
    pub moves: Vec<RefactorMoveForReport>,
    /// Git commands for this phase
    pub git_script: String,
}

/// Statistics about the refactor plan formatted for HTML report.
#[derive(Clone, Default, Serialize)]
pub struct RefactorStatsForReport {
    /// Total files analyzed
    pub total_files: usize,
    /// Files that need to move
    pub files_to_move: usize,
    /// Shims that should be created
    pub shims_needed: usize,
    /// Layer distribution before refactoring (layer -> count)
    pub layer_before: HashMap<String, usize>,
    /// Layer distribution after refactoring (layer -> count)
    pub layer_after: HashMap<String, usize>,
    /// Risk breakdown (risk level -> count)
    pub by_risk: HashMap<String, usize>,
}

/// Complete refactor plan data formatted for the HTML report.
#[derive(Clone, Default, Serialize)]
pub struct RefactorPlanForReport {
    /// Target directory analyzed
    pub target: String,
    /// Execution phases ordered by risk (LOW -> MEDIUM -> HIGH)
    pub phases: Vec<RefactorPhaseForReport>,
    /// Suggested shims for backward compatibility
    pub shims: Vec<RefactorShimForReport>,
    /// Groups of files with cyclic dependencies
    pub cyclic_groups: Vec<Vec<String>>,
    /// Statistics summary
    pub stats: RefactorStatsForReport,
}

impl From<&RefactorPlan> for RefactorPlanForReport {
    fn from(plan: &RefactorPlan) -> Self {
        Self {
            target: plan.target.clone(),
            phases: plan
                .phases
                .iter()
                .map(RefactorPhaseForReport::from)
                .collect(),
            shims: plan.shims.iter().map(RefactorShimForReport::from).collect(),
            cyclic_groups: plan.cyclic_groups.clone(),
            stats: RefactorStatsForReport::from(&plan.stats),
        }
    }
}

impl From<&RefactorPhase> for RefactorPhaseForReport {
    fn from(phase: &RefactorPhase) -> Self {
        Self {
            name: phase.name.clone(),
            risk: phase.risk.label().to_lowercase(),
            moves: phase
                .moves
                .iter()
                .map(RefactorMoveForReport::from)
                .collect(),
            git_script: phase.git_script.clone(),
        }
    }
}

impl From<&Move> for RefactorMoveForReport {
    fn from(mv: &Move) -> Self {
        Self {
            source: mv.source.clone(),
            target: mv.target.clone(),
            current_layer: mv.current_layer.display_name().to_string(),
            target_layer: mv.target_layer.display_name().to_string(),
            risk: mv.risk.label().to_lowercase(),
            loc: mv.loc,
            direct_consumers: mv.direct_consumers,
            reason: mv.reason.clone(),
            verify_cmd: mv.verify_cmd.clone(),
        }
    }
}

impl From<&Shim> for RefactorShimForReport {
    fn from(shim: &Shim) -> Self {
        Self {
            old_path: shim.old_path.clone(),
            new_path: shim.new_path.clone(),
            importer_count: shim.importer_count,
            code: shim.code.clone(),
        }
    }
}

impl From<&PlanStats> for RefactorStatsForReport {
    fn from(stats: &PlanStats) -> Self {
        Self {
            total_files: stats.total_files,
            files_to_move: stats.files_to_move,
            shims_needed: stats.shims_needed,
            layer_before: stats.layer_before.clone(),
            layer_after: stats.layer_after.clone(),
            by_risk: stats.by_risk.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::CommandBridge;

    #[test]
    fn confidence_display_certain() {
        assert_eq!(format!("{}", Confidence::Certain), "CERTAIN");
    }

    #[test]
    fn confidence_display_high() {
        assert_eq!(format!("{}", Confidence::High), "HIGH");
    }

    #[test]
    fn confidence_display_smell() {
        assert_eq!(format!("{}", Confidence::Smell), "SMELL");
    }

    #[test]
    fn confidence_equality() {
        assert_eq!(Confidence::Certain, Confidence::Certain);
        assert_eq!(Confidence::High, Confidence::High);
        assert_eq!(Confidence::Smell, Confidence::Smell);
        assert_ne!(Confidence::High, Confidence::Smell);
    }

    #[test]
    fn confidence_indicator() {
        assert_eq!(Confidence::Certain.indicator(), "[!!]");
        assert_eq!(Confidence::High.indicator(), "[!]");
        assert_eq!(Confidence::Smell.indicator(), "[?]");
    }

    #[test]
    fn string_literal_match_creation() {
        let m = StringLiteralMatch {
            file: "test.ts".to_string(),
            line: 42,
            context: "allowlist".to_string(),
        };
        assert_eq!(m.file, "test.ts");
        assert_eq!(m.line, 42);
        assert_eq!(m.context, "allowlist");
    }

    #[test]
    fn command_gap_creation() {
        let gap = CommandGap {
            name: "test_cmd".to_string(),
            implementation_name: Some("testCmd".to_string()),
            locations: vec![("test.ts".to_string(), 10)],
            confidence: Some(Confidence::High),
            string_literal_matches: vec![],
        };
        assert_eq!(gap.name, "test_cmd");
        assert_eq!(gap.implementation_name, Some("testCmd".to_string()));
        assert_eq!(gap.locations.len(), 1);
        assert_eq!(gap.confidence, Some(Confidence::High));
    }

    #[test]
    fn ai_insight_creation() {
        let insight = AiInsight {
            title: "Test Insight".to_string(),
            severity: "warning".to_string(),
            message: "Some message".to_string(),
        };
        assert_eq!(insight.title, "Test Insight");
        assert_eq!(insight.severity, "warning");
    }

    #[test]
    fn graph_node_creation() {
        let node = GraphNode {
            id: "src/main.ts".to_string(),
            label: "main.ts".to_string(),
            loc: 100,
            x: 0.5,
            y: 0.5,
            component: 0,
            degree: 3,
            detached: false,
        };
        assert_eq!(node.id, "src/main.ts");
        assert_eq!(node.loc, 100);
        assert!(!node.detached);
    }

    #[test]
    fn command_bridge_creation() {
        let bridge = CommandBridge {
            name: "get_user".to_string(),
            frontend_calls: vec![("src/app.ts".to_string(), 10)],
            backend_handler: Some(("src-tauri/src/lib.rs".to_string(), 20)),
            has_handler: true,
            is_called: true,
        };
        assert_eq!(bridge.name, "get_user");
        assert!(bridge.has_handler);
        assert!(bridge.backend_handler.is_some());
    }
}
