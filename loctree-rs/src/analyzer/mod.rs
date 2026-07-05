//! Static analysis engine for loctree.
//!
//! This module contains language-specific analyzers and cross-cutting analysis features:
//!
//! ## Language Analyzers
//! - [`ast_js`] / [`js`] - TypeScript/JavaScript AST analysis
//! - [`py`] - Python import/export analysis
//! - [`rust`] - Rust analysis (Tauri commands, mod statements)
//! - [`css`] - CSS/SCSS dependency tracking
//! - [`dart`] - Dart/Flutter import/export analysis (lightweight)
//!
//! ## Analysis Features
//! - [`cycles`] - Circular import detection (Tarjan's SCC algorithm)
//! - [`dead_parrots`] - Dead/unused export detection
//! - [`coverage`] - Tauri command coverage (FE→BE matching)
//! - [`trace`] - Handler tracing through the call graph
//! - [`pipelines`] - Data flow pipeline analysis
//!
//! ## Output Formats
//! - [`for_ai`] - AI-optimized JSON with quick wins
//! - [`html`] - Interactive HTML reports
//! - [`sarif`] - SARIF 2.1.0 for CI integration
//! - [`report`] - Report data structures

pub mod assets;
pub mod ast_js;
pub mod audit_report;
pub mod barrels;
pub mod c_family_syntax;
pub mod cargo_manifest;
pub mod classify;
pub mod coverage;
pub mod coverage_gaps;
pub mod crowd;
mod css;
pub mod cycles;
pub mod dart;
pub mod dead_parrots;
pub mod dist;
pub mod dist_vlq;
pub mod entrypoints;
pub mod env_truth;
pub mod findings;
pub mod for_ai;
pub mod frameworks;
pub mod go;
mod graph;
pub mod health_score;
pub mod html;
pub(crate) mod html_analyzer;
#[cfg(all(target_os = "macos", feature = "deep-index-macos"))]
pub mod indexstore;
pub mod insights;
pub mod js;
pub mod makefile;
pub mod manifests;
pub mod memory_lint;
pub mod occurrences;
pub mod open_server;
pub mod output;
pub mod pipelines;
pub mod py;
pub mod react_lint;
pub mod regexes;
pub mod report;
pub mod resolvers;
pub mod root_scan;
pub mod route_twins;
pub mod runner;
pub mod rust;
pub mod sarif;
pub mod scan;
#[cfg(feature = "deep-index")]
pub mod scip;
pub mod search;
pub mod shell;
pub mod suppression_inventory;
pub mod swift;
pub mod test_coverage;
pub mod trace;
pub mod ts_lint;
mod tsconfig;
pub mod twins;
pub mod zig;

pub(super) fn offset_to_line(content: &str, offset: usize) -> usize {
    content[..offset].bytes().filter(|b| *b == b'\n').count() + 1
}

/// Check if a file path looks like a test file.
/// Used by ts_lint and memory_lint to adjust severity.
/// Single canonical definition lives in [`classify::is_test_file`].
pub use self::classify::is_test_file;

/// Build an open URL for IDE integration
/// Format: loctree://open?f={file}&l={line}
/// Or if open_base is provided (e.g., "http://127.0.0.1:7777"):
/// Format: {open_base}/open?f={file}&l={line}
pub fn build_open_url(file: &str, line: Option<usize>, open_base: Option<&str>) -> String {
    let base = open_base.unwrap_or("loctree://");
    let path = if base.ends_with('/') {
        format!("{}open", base)
    } else if base.contains("://") {
        format!("{}/open", base)
    } else {
        format!("{}://open", base)
    };

    match line {
        Some(l) => format!("{}?f={}&l={}", path, urlencoding::encode(file), l),
        None => format!("{}?f={}", path, urlencoding::encode(file)),
    }
}

pub use cycles::{ClassifiedCycle, CycleClassification};
pub use report::{
    AiInsight, CommandGap, DupLocation, DupSeverity, GraphComponent, GraphData, GraphNode,
    RankedDup, ReportSection,
};
pub use runner::run_import_analyzer;
