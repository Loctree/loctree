//! New command parser for the subcommand-based CLI interface.
//!
//! This module parses `loct <command> [options]` style invocations.
//! It detects whether the input uses new subcommands or legacy flags
//! and routes accordingly.
//!
//! # Module Structure
//!
//! The parser is organized into focused submodules:
//!
//! - [`core`] - Main entry point, syntax detection, global options
//! - [`helpers`] - Utility functions (color parsing, jq detection, suggestions)
//! - [`scan_commands`] - auto, scan, tree command parsers
//! - [`analysis_commands`] - dead, cycles, find, query, impact, twins parsers
//! - [`context_commands`] - slice, trace, focus, coverage, hotspots parsers
//! - [`tauri_commands`] - commands, events, routes parsers
//! - [`output_commands`] - report, findings, info, lint, diff, jq_query parsers
//! - [`misc_commands`] - crowd, tagmap, suppress, dist, layoutmap, health, audit, doctor, help parsers
//!
//! # Usage
//!
//! ```ignore
//! use loctree::cli::parser::{parse_command, is_subcommand, uses_new_syntax};
//!
//! let args: Vec<String> = std::env::args().skip(1).collect();
//!
//! if let Some(parsed) = parse_command(&args)? {
//!     // Handle new-style command
//!     dispatch_command(parsed);
//! } else {
//!     // Fall back to legacy parser
//!     legacy_parse(&args);
//! }
//! ```

mod analysis_commands;
mod context_commands;
mod core;
mod helpers;
mod misc_commands;
mod output_commands;
mod scan_commands;
mod tauri_commands;

// Re-export public API
pub use core::{parse_command, uses_new_syntax};
pub use helpers::is_subcommand;
