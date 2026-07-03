//! Global options shared across all CLI commands.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::path::PathBuf;

use crate::types::ColorMode;

/// Global options that apply to all commands.
///
/// These flags can be used with any command and control output format,
/// verbosity, and other cross-cutting concerns.
#[derive(Debug, Clone, Default)]
pub struct GlobalOptions {
    /// Output as JSON (stdout is JSON only, warnings go to stderr)
    pub json: bool,

    /// Suppress all non-essential output including deprecation warnings
    pub quiet: bool,

    /// Color mode for terminal output
    pub color: ColorMode,

    /// Verbose output with progress information
    pub verbose: bool,

    /// Library/framework mode (tunes dead-code heuristics, ignores examples)
    pub library_mode: bool,

    /// Python library mode (treat __all__ exports as public API, skip dunder methods)
    pub python_library: bool,

    /// Additional Python package roots for import resolution
    pub py_roots: Vec<PathBuf>,

    /// Force fresh scan even if snapshot exists (--fresh)
    pub fresh: bool,

    /// Fail if no snapshot exists instead of auto-scanning (--no-scan)
    pub no_scan: bool,

    /// Fail if snapshot is stale (different git HEAD) - for CI (--fail-stale)
    pub fail_stale: bool,
}
