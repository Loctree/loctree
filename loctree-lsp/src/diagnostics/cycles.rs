//! Cycle diagnostics
//!
//! Converts circular import findings to LSP diagnostics.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString,
    Position, Range, Url,
};

use crate::diagnostic_codes::DiagnosticCode;
use crate::snapshot::SnapshotState;

/// Generate diagnostics for circular imports in a file
pub async fn cycle_diagnostics(snapshot: &SnapshotState, file_path: &str) -> Vec<Diagnostic> {
    let cycles = snapshot.cycles_for_file(file_path).await;

    cycles
        .into_iter()
        .map(|cycle| {
            let severity = match cycle.cycle_type.as_str() {
                "breaking" | "bidirectional" => DiagnosticSeverity::WARNING,
                "structural" => DiagnosticSeverity::WARNING,
                _ => DiagnosticSeverity::INFORMATION,
            };
            let line = cycle.import_line.unwrap_or(1).saturating_sub(1) as u32;

            let cycle_str = cycle.files.join(" -> ");

            // Create related information for other files in cycle
            let related = cycle
                .files
                .iter()
                .filter(|f| !f.ends_with(file_path) && !file_path.ends_with(*f))
                .filter_map(|f| {
                    Url::from_file_path(f)
                        .ok()
                        .map(|uri| DiagnosticRelatedInformation {
                            location: Location {
                                uri,
                                range: Range {
                                    start: Position {
                                        line: 0,
                                        character: 0,
                                    },
                                    end: Position {
                                        line: 0,
                                        character: 0,
                                    },
                                },
                            },
                            message: "Part of import cycle".to_string(),
                        })
                })
                .collect::<Vec<_>>();

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
                    DiagnosticCode::CircularImport.as_str().to_string(),
                )),
                code_description: None,
                source: Some("loctree".to_string()),
                message: format!("Circular import: {}", cycle_str),
                related_information: if related.is_empty() {
                    None
                } else {
                    Some(related)
                },
                tags: None,
                data: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use loctree::snapshot::{GraphEdge, Snapshot, project_cache_dir};
    use loctree::types::{FileAnalysis, ImportEntry, ImportKind};
    use tempfile::TempDir;

    fn write_snapshot(root: &Path, snapshot: &Snapshot) {
        snapshot.save(root).expect("save snapshot");
    }

    fn cleanup_cache(root: &Path) {
        let cache_dir = project_cache_dir(root);
        let _ = std::fs::remove_dir_all(cache_dir);
    }

    #[tokio::test]
    async fn cycle_diagnostic_uses_import_line() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);

        let mut a = FileAnalysis::new("src/a.rs".to_string());
        let mut import = ImportEntry::new("./b".to_string(), ImportKind::Static);
        import.resolved_path = Some("src/b.rs".to_string());
        import.line = Some(9);
        a.imports.push(import);

        snapshot.files = vec![a, FileAnalysis::new("src/b.rs".to_string())];
        snapshot.edges = vec![
            GraphEdge {
                from: "src/a.rs".to_string(),
                to: "src/b.rs".to_string(),
                label: "b".to_string(),
            },
            GraphEdge {
                from: "src/b.rs".to_string(),
                to: "src/a.rs".to_string(),
                label: "a".to_string(),
            },
        ];

        write_snapshot(root, &snapshot);

        let state = SnapshotState::new();
        state.load(root).await.expect("load snapshot");

        let diagnostics = cycle_diagnostics(&state, "src/a.rs").await;
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].range.start.line, 8);

        cleanup_cache(root);
    }
}
