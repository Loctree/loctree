//! CLI module for the new subcommand-based interface.
//!
//! This module provides the canonical `loct <command> [options]` interface
//! while maintaining backward compatibility with legacy flags through an adapter.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                        User Input (argv)                        │
//! └─────────────────────────────────────────────────────────────────┘
//!                                  │
//!                                  ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                         CLI Parser                              │
//! │  ┌─────────────────┐    ┌─────────────────────────────────┐    │
//! │  │ New Subcommands │    │ Legacy Adapter (args::legacy)   │    │
//! │  │ loct scan       │    │ -A --dead → loct dead           │    │
//! │  │ loct tree       │    │ --tree → loct tree              │    │
//! │  │ loct slice      │    │ --for-ai → loct slice --json    │    │
//! │  └────────┬────────┘    └────────────────┬────────────────┘    │
//! │           │                              │                      │
//! │           └──────────────┬───────────────┘                      │
//! │                          ▼                                      │
//! │              ┌───────────────────────┐                          │
//! │              │  Command + Options    │                          │
//! │              │  (Unified internal    │                          │
//! │              │   representation)     │                          │
//! │              └───────────┬───────────┘                          │
//! └──────────────────────────┼──────────────────────────────────────┘
//!                            │
//!                            ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     Command Handlers                            │
//! │  analyzer::run_import_analyzer, tree::run_tree, slicer::...,   │
//! │  snapshot::..., etc.                                            │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Design Principles
//!
//! 1. **Agent-first**: The tool is primarily for AI agents. Humans get a
//!    minimal interface (5 core commands).
//!
//! 2. **Minimal commands, exclusive flags**: Flags modify/exclude default
//!    behavior, they don't add functionality.
//!
//! 3. **Regex on metadata**: Agents can filter using regex on symbol names,
//!    paths, namespaces - but never on raw source code.
//!
//! 4. **Legacy compatibility**: Old flags work with deprecation warnings
//!    until v1.0 when the adapter is removed.
//!
//! # Module Structure
//!
//! - [`command`] - Command enum and option types (source of truth)
//! - [`parser`] - New subcommand parser
//! - [`dispatch`] - Command dispatcher and ParsedArgs converter
//! - `legacy` (future) - Legacy flag adapter

pub mod command;
pub mod dispatch;
pub mod entrypoint;
pub mod parser;

// Re-export main types for convenience
pub use command::{
    // Per-command options
    AutoOptions,
    // Command enum
    Command,
    CommandsOptions,
    CyclesOptions,
    DeadOptions,
    EventsOptions,
    FindOptions,
    FindingsOptions,
    // Global options
    GlobalOptions,
    HelpOptions,
    InfoOptions,
    LintOptions,
    // Parsing result
    ParsedCommand,
    ReportOptions,
    ScanOptions,
    SliceOptions,
    TreeOptions,
};

// Re-export parser functions
pub use parser::{is_subcommand, parse_command, uses_new_syntax};

// Re-export dispatch functions
pub use dispatch::{DispatchResult, command_to_parsed_args, dispatch_command};
