//! Diagnostics generation for loctree LSP
//!
//! Converts loctree analysis results into LSP Diagnostic objects.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

mod cycles;
mod dead;
mod twins;

pub use cycles::cycle_diagnostics;
pub use dead::dead_export_diagnostics;
pub use twins::twin_diagnostics;

use tower_lsp::lsp_types::Diagnostic;

use crate::snapshot::SnapshotState;

/// Collect all diagnostics for a file
pub async fn collect_diagnostics(snapshot: &SnapshotState, file_path: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    // Dead exports
    diagnostics.extend(dead_export_diagnostics(snapshot, file_path).await);

    // Cycles
    diagnostics.extend(cycle_diagnostics(snapshot, file_path).await);

    // Twins
    diagnostics.extend(twin_diagnostics(snapshot, file_path).await);

    diagnostics
}
