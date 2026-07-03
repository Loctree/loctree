//! CLI command definitions and help text.
//!
//! This module provides the core Command enum and associated types for the
//! `loct <command>` interface. It is modularized for maintainability:
//!
//! - `global`: GlobalOptions struct shared across all commands
//! - `options`: Per-command option structs
//! - `types`: Command enum definition
//! - `help`: Help text generation (impl on Command)
//! - `help_texts`: Static help text constants
//! - `parsed`: ParsedCommand result type
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

mod global;
mod help;
mod help_texts;
pub mod options;
mod parsed;
mod types;

// Re-export the main types at the module level
pub use crate::pack::ContextOptions;
pub use global::GlobalOptions;
pub use options::{
    AuditOptions, AutoOptions, BodyOptions, CacheAction, CacheOptions, CommandsOptions,
    CoverageOptions, CrowdOptions, CyclesOptions, DeadOptions, DiffOptions, DistOptions,
    DoctorOptions, EnvTruthOptions, EventsOptions, FindOptions, FindingsOptions, FocusOptions,
    FollowOptions, HealthOptions, HelpOptions, HotspotsOptions, ImpactCommandOptions, InfoOptions,
    InsightsOptions, JqQueryOptions, LayoutmapOptions, LintOptions, ManifestsOptions,
    OccurrencesOptions, PipelinesOptions, PlanOptions, PrismOptions, PruneOldArtifactsOptions,
    QueryKind, QueryOptions, RepoViewOptions, ReportOptions, RoutesOptions, ScanOptions,
    SliceOptions, SniffOptions, SuppressOptions, SuppressionsOptions, TagmapOptions, TraceOptions,
    TreeOptions, TwinsOptions, WatchMode, WatchOptions, ZombieOptions,
};
pub use parsed::ParsedCommand;
pub use types::Command;
