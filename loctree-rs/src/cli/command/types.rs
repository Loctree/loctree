//! Command enum definition for the CLI interface.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use super::options::*;
#[allow(unused_imports)]
use super::options::{WatchMode, WatchOptions};
use crate::pack::ContextOptions;

/// The canonical command enum for the `loct <command>` interface.
///
/// Each variant maps to a handler module. This enum is the single source
/// of truth for CLI commands and backs both parser and help output.
#[derive(Debug, Clone)]
pub enum Command {
    /// Automatic full scan with stack detection (default when no command given).
    Auto(AutoOptions),

    /// Build/update snapshot for current HEAD.
    Scan(ScanOptions),

    /// Single-instance watch loop with optional co-processes.
    ///
    /// The new shape of `loct scan --watch`. Same locked watch loop,
    /// plus `--dev` / `--bg` / `--lsp` / (deferred) `--http` / `--report`.
    Watch(WatchOptions),

    /// Display LOC tree / structural overview.
    Tree(TreeOptions),

    /// Produce 3-layer holographic context for a path.
    Slice(SliceOptions),

    /// Produce an agent-ready context pack.
    Context(ContextOptions),

    /// Repository overview for AI agents.
    RepoView(RepoViewOptions),

    /// Search symbols/files/impact/similar.
    Find(FindOptions),

    /// Literal exact-identifier occurrence scan over snapshot files.
    ///
    /// The truth layer beneath `find`: walks raw source bytes with
    /// identifier-boundary matching, never promotes fuzzy suggestions.
    Occurrences(OccurrencesOptions),

    /// Emit the canonical findings artifact to stdout.
    Findings(FindingsOptions),

    /// Detect unused exports / dead code.
    Dead(DeadOptions),

    /// Detect circular imports / structural cycles.
    Cycles(CyclesOptions),

    /// Trace a Tauri/IPC handler end-to-end.
    Trace(TraceOptions),

    /// Show Tauri command bridges (FE <-> BE mappings).
    Commands(CommandsOptions),

    /// Show backend/web routes (FastAPI/Flask/etc.)
    Routes(RoutesOptions),

    /// Show event flow (ghost events, orphan handlers, races).
    Events(EventsOptions),

    /// Show pipeline summary (events, commands, risks).
    Pipelines(PipelinesOptions),

    /// Show AI insights summary.
    Insights(InsightsOptions),

    /// Show manifest summaries (package.json, Cargo.toml, pyproject).
    Manifests(ManifestsOptions),

    /// Snapshot metadata and project info.
    Info(InfoOptions),

    /// Structural lint/policy checks.
    Lint(LintOptions),

    /// Generate HTML/JSON reports.
    Report(ReportOptions),

    /// Compare context packs across task framings and score conceptual smear.
    Prism(PrismOptions),

    /// Show help for commands.
    Help(HelpOptions),

    /// Show version.
    Version,

    /// Query snapshot data (who-imports, where-symbol, component-of).
    Query(QueryOptions),

    /// Retrieve bounded source body/range for an exported symbol.
    Body(BodyOptions),

    /// Compare two snapshots and show delta.
    Diff(DiffOptions),

    /// Detect functional crowds (similar files clustering).
    Crowd(CrowdOptions),

    /// Unified search around a keyword - files, crowds, and dead exports.
    Tagmap(TagmapOptions),

    /// Show symbol registry and dead parrots (semantic duplicate detection).
    Twins(TwinsOptions),

    /// Manage false positive suppressions (loctree's own finding-suppression file).
    Suppress(SuppressOptions),

    /// Source-side silencer inventory (`#[allow(...)]`, `@ts-ignore`,
    /// `# noqa`, `// nosemgrep`, `unsafe { ... }`, etc.). Literal regex
    /// detection — semantic enrichment is paid-tier Wave 7+.
    Suppressions(SuppressionsOptions),

    /// Analyze bundle distribution using source maps.
    Dist(DistOptions),

    /// Analyze test coverage gaps.
    Coverage(CoverageOptions),

    /// Sniff for code smells (twins + dead parrots + crowds).
    Sniff(SniffOptions),

    /// Query snapshot with jq-style filters (loct '.metadata').
    JqQuery(JqQueryOptions),

    /// Analyze impact of modifying/removing a file.
    Impact(ImpactCommandOptions),

    /// Focus on a directory - extract holographic context for all files.
    Focus(FocusOptions),

    /// Show import frequency heatmap - which files are core vs peripheral.
    Hotspots(HotspotsOptions),

    /// Unified signal follower matching the MCP follow tool.
    Follow(FollowOptions),

    /// Analyze CSS layout properties (z-index, position, display).
    Layoutmap(LayoutmapOptions),

    /// Find zombie code (dead exports + orphan files + shadow exports).
    Zombie(ZombieOptions),

    /// Quick health check summary (cycles + dead + twins).
    Health(HealthOptions),

    /// Full audit - comprehensive analysis with actionable findings.
    Audit(AuditOptions),

    /// Interactive diagnostics with actionable recommendations.
    Doctor(DoctorOptions),

    /// Generate architectural refactoring plan based on module analysis.
    Plan(PlanOptions),

    /// Manage snapshot cache (list, clean).
    Cache(CacheOptions),

    /// Audit env-variable declaration sources for resolution-order drift
    /// (Cut 8 / Lane 4). Surfaces dotenv, dockerfile, docker-compose, k8s,
    /// helm, GitHub Actions, npm, and sops-marker declarations and
    /// cross-references the read side from `semantic_facts.env_contracts`.
    EnvTruth(EnvTruthOptions),

    /// Prune old per-branch snapshot artifacts from local `.loctree/` dirs.
    ///
    /// Living-tree projects accumulate `<branch>@<commit>/snapshot.json`
    /// directories per agent run; without periodic pruning they shadow real
    /// project roots (causing scope-drift bugs) and bloat the repo.
    PruneOldArtifacts(PruneOldArtifactsOptions),
}

impl Default for Command {
    fn default() -> Self {
        Command::Auto(AutoOptions::default())
    }
}
