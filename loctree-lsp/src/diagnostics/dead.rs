//! Dead export diagnostics
//!
//! Converts dead export findings to LSP diagnostics.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};

use crate::diagnostic_codes::DiagnosticCode;
use crate::snapshot::SnapshotState;

/// Generate diagnostics for dead exports in a file
pub async fn dead_export_diagnostics(snapshot: &SnapshotState, file_path: &str) -> Vec<Diagnostic> {
    let dead_exports = snapshot.dead_exports_for_file(file_path).await;

    dead_exports
        .into_iter()
        .map(|dead| {
            let severity = match dead.confidence.as_str() {
                "high" | "very-high" => DiagnosticSeverity::WARNING,
                "normal" => DiagnosticSeverity::INFORMATION,
                _ => DiagnosticSeverity::HINT,
            };

            // Line numbers in LSP are 0-indexed
            let line = dead.line.saturating_sub(1) as u32;

            Diagnostic {
                range: Range {
                    start: Position { line, character: 0 },
                    end: Position {
                        line,
                        character: 100,
                    },
                },
                severity: Some(severity),
                code: Some(NumberOrString::String(
                    DiagnosticCode::DeadExport.as_str().to_string(),
                )),
                code_description: None,
                source: Some("loctree".to_string()),
                message: format!(
                    "Dead export '{}' [{} confidence]: {}",
                    dead.symbol, dead.confidence, dead.reason
                ),
                related_information: None,
                tags: None,
                data: None,
            }
        })
        .collect()
}
