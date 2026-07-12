//! Twin/duplicate diagnostics
//!
//! Converts twin findings to LSP diagnostics.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use loctree::analyzer::twins::{TwinCategory, categorize_twin};
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString,
    Position, Range, Url,
};

use crate::diagnostic_codes::DiagnosticCode;
use crate::snapshot::SnapshotState;

/// Generate diagnostics for duplicate exports in a file
pub async fn twin_diagnostics(snapshot: &SnapshotState, file_path: &str) -> Vec<Diagnostic> {
    let twins = snapshot.twins_for_file(file_path).await;
    let workspace_root = snapshot.workspace_root().await;

    twins
        .into_iter()
        .map(|twin| {
            let severity = match categorize_twin(&twin) {
                TwinCategory::CrossLanguage => DiagnosticSeverity::INFORMATION,
                TwinCategory::SameLanguage(_) => DiagnosticSeverity::WARNING,
                TwinCategory::Namesake => DiagnosticSeverity::HINT,
            };

            let current_location = twin
                .locations
                .iter()
                .find(|loc| paths_match(&loc.file_path, file_path));
            let current_line = current_location
                .map(|loc| loc.line)
                .filter(|line| *line > 0)
                .unwrap_or(1);

            let other_locations: Vec<_> = twin
                .locations
                .iter()
                .filter(|loc| !paths_match(&loc.file_path, file_path))
                .collect();

            let also_found = other_locations
                .iter()
                .map(|loc| format!("{}:{}", loc.file_path, loc.line))
                .collect::<Vec<_>>()
                .join(", ");

            let message = if also_found.is_empty() {
                format!("Twin export '{}' detected", twin.name)
            } else {
                format!("Twin export '{}' also found in: {}", twin.name, also_found)
            };

            let related_information = other_locations
                .iter()
                .filter_map(|loc| {
                    twin_location_url(&loc.file_path, workspace_root.as_deref()).map(|uri| {
                        DiagnosticRelatedInformation {
                            location: Location {
                                uri,
                                range: Range {
                                    start: Position {
                                        line: loc.line.saturating_sub(1) as u32,
                                        character: 0,
                                    },
                                    end: Position {
                                        line: loc.line.saturating_sub(1) as u32,
                                        character: 100,
                                    },
                                },
                            },
                            message: format!("Twin export '{}' is also defined here", twin.name),
                        }
                    })
                })
                .collect::<Vec<_>>();

            Diagnostic {
                range: Range {
                    start: Position {
                        line: current_line.saturating_sub(1) as u32,
                        character: 0,
                    },
                    end: Position {
                        line: current_line.saturating_sub(1) as u32,
                        character: 100,
                    },
                },
                severity: Some(severity),
                code: Some(NumberOrString::String(
                    DiagnosticCode::TwinExport.as_str().to_string(),
                )),
                code_description: None,
                source: Some("loctree".to_string()),
                message,
                related_information: if related_information.is_empty() {
                    None
                } else {
                    Some(related_information)
                },
                tags: None,
                data: None,
            }
        })
        .collect()
}

fn paths_match(a: &str, b: &str) -> bool {
    a.ends_with(b) || b.ends_with(a)
}

fn twin_location_url(path: &str, workspace_root: Option<&std::path::Path>) -> Option<Url> {
    Url::from_file_path(path)
        .ok()
        .or_else(|| workspace_root.and_then(|root| Url::from_file_path(root.join(path)).ok()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use loctree::snapshot::{Snapshot, project_cache_dir};
    use loctree::types::{ExportSymbol, FileAnalysis};
    use tempfile::TempDir;

    fn build_export(name: &str, line: usize) -> ExportSymbol {
        ExportSymbol {
            name: name.to_string(),
            kind: "function".to_string(),
            export_type: "named".to_string(),
            line: Some(line),
            params: Vec::new(),

            symbol_id: ::loctree::types::SymbolIdV1::default(),
        }
    }

    fn write_snapshot(root: &Path, snapshot: &Snapshot) {
        snapshot.save(root).expect("save snapshot");
    }

    fn cleanup_cache(root: &Path) {
        let cache_dir = project_cache_dir(root);
        let _ = std::fs::remove_dir_all(cache_dir);
    }

    #[tokio::test]
    async fn emits_warning_for_same_language_twins() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
        snapshot.files = vec![
            FileAnalysis {
                path: "src/a.rs".to_string(),
                exports: vec![build_export("Shared", 10)],
                ..Default::default()
            },
            FileAnalysis {
                path: "src/b.rs".to_string(),
                exports: vec![build_export("Shared", 20)],
                ..Default::default()
            },
        ];
        write_snapshot(root, &snapshot);

        let state = SnapshotState::new();
        state.load(root).await.expect("load snapshot");

        let diagnostics = twin_diagnostics(&state, "src/a.rs").await;
        assert_eq!(diagnostics.len(), 1);
        let diag = &diagnostics[0];
        assert_eq!(diag.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            diag.code,
            Some(NumberOrString::String("twin-export".to_string()))
        );
        assert!(diag.message.contains("Twin export 'Shared'"));
        assert!(diag.message.contains("src/b.rs:20"));
        assert!(
            diag.related_information
                .as_ref()
                .is_some_and(|related| !related.is_empty())
        );

        cleanup_cache(root);
    }

    #[tokio::test]
    async fn emits_info_for_cross_language_twins() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
        snapshot.files = vec![
            FileAnalysis {
                path: "src/a.rs".to_string(),
                exports: vec![build_export("Bridge", 10)],
                ..Default::default()
            },
            FileAnalysis {
                path: "src/bridge.ts".to_string(),
                exports: vec![build_export("Bridge", 5)],
                ..Default::default()
            },
        ];
        write_snapshot(root, &snapshot);

        let state = SnapshotState::new();
        state.load(root).await.expect("load snapshot");

        let diagnostics = twin_diagnostics(&state, "src/a.rs").await;
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].severity,
            Some(DiagnosticSeverity::INFORMATION)
        );

        cleanup_cache(root);
    }
}
