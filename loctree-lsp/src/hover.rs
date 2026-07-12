//! Hover provider for loctree LSP
//!
//! Provides rich hover information for exports and imports using snapshot data.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::collections::HashMap;

use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};

use crate::navigation::get_word_at_position;
use crate::snapshot::SnapshotState;

/// Information about an export for hover display
#[derive(Debug, Clone)]
pub struct ExportHoverInfo {
    /// Symbol name
    pub symbol: String,
    /// File where the symbol is exported
    pub file: String,
    /// Number of files that import this export
    pub import_count: usize,
    /// List of consumer files (top N)
    pub top_consumers: Vec<String>,
    /// Export kind (function, class, const, type, etc.)
    pub kind: String,
    /// Line number of the export
    pub line: Option<usize>,
}

/// Information about an import for hover display
#[derive(Debug, Clone)]
pub struct ImportHoverInfo {
    /// Symbol name being imported
    pub symbol: String,
    /// Source file where the symbol is defined
    pub source_file: String,
    /// Line number where the symbol is defined
    pub source_line: Option<usize>,
    /// Kind of the imported symbol
    pub kind: String,
}

impl SnapshotState {
    /// Find export info at a given position in a file
    ///
    /// Returns hover info if the position is on an export symbol
    pub async fn find_export_at_position(
        &self,
        file_path: &str,
        position: Position,
        hover_symbol: Option<&str>,
    ) -> Option<ExportHoverInfo> {
        let guard = self.get().await?;
        let loaded = guard.as_ref()?;
        let snapshot = &loaded.snapshot;

        // Find the file in the snapshot
        let file = snapshot
            .files
            .iter()
            .find(|f| f.path.ends_with(file_path) || file_path.ends_with(&f.path))?;

        // Find export at the given line (1-based line numbers in LSP)
        let target_line = position.line as usize + 1;
        let export = file.exports.iter().find(|e| {
            e.line.map(|l| l == target_line).unwrap_or(false)
                && hover_symbol
                    .map(|symbol| symbol_matches_export(symbol, e))
                    .unwrap_or(true)
        })?;

        // Count imports for this export by analyzing edges
        let (import_count, top_consumers) = self.count_imports_for_file(snapshot, &file.path);

        Some(ExportHoverInfo {
            symbol: export.name.clone(),
            file: file.path.clone(),
            import_count,
            top_consumers,
            kind: export.kind.clone(),
            line: export.line,
        })
    }

    /// Find import info at a given position in a file
    ///
    /// Returns hover info if the position is on an import statement
    pub async fn find_import_at_position(
        &self,
        file_path: &str,
        position: Position,
        hover_symbol: Option<&str>,
    ) -> Option<ImportHoverInfo> {
        let guard = self.get().await?;
        let loaded = guard.as_ref()?;
        let snapshot = &loaded.snapshot;

        // Find the file in the snapshot
        let file = snapshot
            .files
            .iter()
            .find(|f| f.path.ends_with(file_path) || file_path.ends_with(&f.path))?;

        // Import lines in snapshot are 1-based, LSP lines are 0-based.
        let target_line = position.line as usize + 1;
        let imports_on_line: Vec<_> = file
            .imports
            .iter()
            .filter(|import| import.line == Some(target_line))
            .collect();
        let import = hover_symbol
            .and_then(|symbol| {
                imports_on_line
                    .iter()
                    .copied()
                    .find(|import| import_symbols_contain(import, symbol))
            })
            .or_else(|| imports_on_line.first().copied())?;

        let matching_edges: Vec<_> = snapshot
            .edges
            .iter()
            .filter(|edge| paths_match(&edge.from, &file.path))
            .filter(|edge| import_targets_edge(import, edge))
            .collect();

        let edge = matching_edges
            .iter()
            .copied()
            .find(|edge| edge_label_matches_import(import, &edge.label))
            .or_else(|| matching_edges.first().copied())?;

        // Find the target file to get export info
        let target_file = snapshot
            .files
            .iter()
            .find(|f| paths_match(&f.path, &edge.to))?;

        // Prefer export matching edge label; if unavailable, try imported symbol names.
        let export = target_file
            .exports
            .iter()
            .find(|e| e.name == edge.label)
            .or_else(|| {
                import.symbols.iter().find_map(|sym| {
                    target_file.exports.iter().find(|e| {
                        e.name == sym.name
                            || sym.alias.as_deref().is_some_and(|alias| e.name == alias)
                    })
                })
            });

        Some(ImportHoverInfo {
            symbol: export
                .map(|e| e.name.clone())
                .unwrap_or_else(|| edge.label.clone()),
            source_file: target_file.path.clone(),
            source_line: export.and_then(|e| e.line),
            kind: export.map(|e| e.kind.clone()).unwrap_or_default(),
        })
    }

    /// Count how many files import a given file and return top consumers
    fn count_imports_for_file(
        &self,
        snapshot: &loctree::snapshot::Snapshot,
        file_path: &str,
    ) -> (usize, Vec<String>) {
        let mut consumers: HashMap<String, usize> = HashMap::new();

        for edge in &snapshot.edges {
            if edge.to == file_path || edge.to.ends_with(file_path) || file_path.ends_with(&edge.to)
            {
                *consumers.entry(edge.from.clone()).or_insert(0) += 1;
            }
        }

        let import_count = consumers.len();

        // Sort by import count and take top 5
        let mut sorted: Vec<_> = consumers.into_iter().collect();
        sorted.sort_by_key(|b| std::cmp::Reverse(b.1));
        let top_consumers: Vec<String> = sorted.into_iter().take(5).map(|(f, _)| f).collect();

        (import_count, top_consumers)
    }

    /// Get hover info for any symbol at position
    ///
    /// Checks exports first, then imports
    pub async fn get_hover_info(
        &self,
        file_path: &str,
        position: Position,
        document_text: Option<&str>,
    ) -> Option<Hover> {
        let hover_symbol = document_text.and_then(|text| get_word_at_position(text, position));
        let hover_symbol = hover_symbol.as_deref();

        // Try export first
        if let Some(export_info) = self
            .find_export_at_position(file_path, position, hover_symbol)
            .await
        {
            return Some(format_export_hover(&export_info));
        }

        // Try import
        if let Some(import_info) = self
            .find_import_at_position(file_path, position, hover_symbol)
            .await
        {
            return Some(format_import_hover(&import_info));
        }

        None
    }
}

fn symbol_matches_export(symbol: &str, export: &loctree::types::ExportSymbol) -> bool {
    export.name == symbol || (export.export_type == "default" && symbol != "default")
}

fn import_symbols_contain(import: &loctree::types::ImportEntry, symbol: &str) -> bool {
    import.symbols.iter().any(|candidate| {
        candidate.name == symbol
            || candidate.alias.as_deref() == Some(symbol)
            || (candidate.is_default && symbol != "default")
    })
}

fn normalize_path(path: &str) -> String {
    path.trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

fn paths_match(a: &str, b: &str) -> bool {
    let a_normalized = normalize_path(a);
    let b_normalized = normalize_path(b);
    a_normalized == b_normalized
        || a_normalized.ends_with(&b_normalized)
        || b_normalized.ends_with(&a_normalized)
}

fn import_targets_edge(
    import: &loctree::types::ImportEntry,
    edge: &loctree::snapshot::GraphEdge,
) -> bool {
    import
        .resolved_path
        .as_ref()
        .is_some_and(|resolved| paths_match(resolved, &edge.to))
        || paths_match(&import.source, &edge.to)
        || paths_match(&import.source_raw, &edge.to)
}

fn edge_label_matches_import(import: &loctree::types::ImportEntry, edge_label: &str) -> bool {
    if import.symbols.is_empty() {
        return true;
    }

    let labels: Vec<&str> = edge_label
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    if labels.contains(&"*") {
        return true;
    }

    import.symbols.iter().any(|symbol| {
        let base_name = if symbol.is_default {
            "default"
        } else {
            symbol.name.as_str()
        };
        labels.contains(&base_name)
            || labels.contains(&symbol.name.as_str())
            || symbol
                .alias
                .as_deref()
                .is_some_and(|alias| labels.contains(&alias))
    })
}

/// Format export info as Markdown hover content
fn format_export_hover(info: &ExportHoverInfo) -> Hover {
    let mut lines = vec![format!("**Export: `{}`**", info.symbol)];

    if !info.kind.is_empty() {
        lines.push(format!("- Kind: {}", info.kind));
    }

    if info.import_count > 0 {
        let file_word = if info.import_count == 1 {
            "file"
        } else {
            "files"
        };
        lines.push(format!(
            "- {} imports across {} {}",
            info.import_count, info.import_count, file_word
        ));

        if !info.top_consumers.is_empty() {
            let consumers: Vec<String> = info
                .top_consumers
                .iter()
                .map(|f| format!("`{}`", shorten_path(f)))
                .collect();
            lines.push(format!("- Top consumers: {}", consumers.join(", ")));
        }
    } else {
        lines.push("- No imports found (potentially dead code)".to_string());
    }

    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: lines.join("\n"),
        }),
        range: None,
    }
}

/// Format import info as Markdown hover content
fn format_import_hover(info: &ImportHoverInfo) -> Hover {
    let mut lines = vec![format!("**Import: `{}`**", info.symbol)];

    lines.push(format!(
        "- Defined in: `{}`",
        shorten_path(&info.source_file)
    ));

    if let Some(line) = info.source_line {
        lines.push(format!("- Line: {}", line));
    }

    if !info.kind.is_empty() {
        lines.push(format!("- Kind: {}", info.kind));
    }

    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: lines.join("\n"),
        }),
        range: None,
    }
}

/// Shorten a file path for display (show last 2-3 segments)
fn shorten_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 3 {
        path.to_string()
    } else {
        parts[parts.len() - 3..].join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use loctree::snapshot::{GraphEdge, Snapshot, project_cache_dir};
    use loctree::types::{ExportSymbol, FileAnalysis, ImportEntry, ImportKind, ImportSymbol};
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

    #[test]
    fn test_shorten_path_short() {
        assert_eq!(shorten_path("src/main.ts"), "src/main.ts");
        assert_eq!(shorten_path("a/b/c"), "a/b/c");
    }

    #[test]
    fn test_shorten_path_long() {
        assert_eq!(
            shorten_path("project/src/components/Button.tsx"),
            "src/components/Button.tsx"
        );
    }

    #[test]
    fn test_format_export_hover_with_imports() {
        let info = ExportHoverInfo {
            symbol: "Button".to_string(),
            file: "src/components/Button.tsx".to_string(),
            import_count: 5,
            top_consumers: vec!["App.tsx".to_string(), "Page.tsx".to_string()],
            kind: "function".to_string(),
            line: Some(10),
        };

        let hover = format_export_hover(&info);
        if let HoverContents::Markup(content) = hover.contents {
            assert!(content.value.contains("**Export: `Button`**"));
            assert!(content.value.contains("5 imports"));
            assert!(content.value.contains("`App.tsx`"));
        } else {
            panic!("Expected Markup content");
        }
    }

    #[test]
    fn test_format_export_hover_no_imports() {
        let info = ExportHoverInfo {
            symbol: "unused".to_string(),
            file: "src/unused.ts".to_string(),
            import_count: 0,
            top_consumers: vec![],
            kind: "const".to_string(),
            line: Some(1),
        };

        let hover = format_export_hover(&info);
        if let HoverContents::Markup(content) = hover.contents {
            assert!(content.value.contains("No imports found"));
            assert!(content.value.contains("dead code"));
        } else {
            panic!("Expected Markup content");
        }
    }

    #[test]
    fn test_format_import_hover() {
        let info = ImportHoverInfo {
            symbol: "useState".to_string(),
            source_file: "node_modules/react/index.js".to_string(),
            source_line: Some(42),
            kind: "function".to_string(),
        };

        let hover = format_import_hover(&info);
        if let HoverContents::Markup(content) = hover.contents {
            assert!(content.value.contains("**Import: `useState`**"));
            assert!(content.value.contains("react/index.js"));
            assert!(content.value.contains("Line: 42"));
        } else {
            panic!("Expected Markup content");
        }
    }

    #[tokio::test]
    async fn finds_import_hover_for_exact_line() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);

        let mut app = FileAnalysis::new("src/app.ts".to_string());

        let mut foo_import = ImportEntry::new("./foo".to_string(), ImportKind::Static);
        foo_import.resolved_path = Some("src/foo.ts".to_string());
        foo_import.line = Some(3);
        foo_import.symbols.push(ImportSymbol {
            name: "Foo".to_string(),
            alias: None,
            is_default: false,
        });

        let mut bar_import = ImportEntry::new("./bar".to_string(), ImportKind::Static);
        bar_import.resolved_path = Some("src/bar.ts".to_string());
        bar_import.line = Some(10);
        bar_import.symbols.push(ImportSymbol {
            name: "Bar".to_string(),
            alias: None,
            is_default: false,
        });

        app.imports = vec![foo_import, bar_import];

        snapshot.files = vec![
            app,
            FileAnalysis {
                path: "src/foo.ts".to_string(),
                exports: vec![build_export("Foo", 2)],
                ..Default::default()
            },
            FileAnalysis {
                path: "src/bar.ts".to_string(),
                exports: vec![build_export("Bar", 7)],
                ..Default::default()
            },
        ];

        snapshot.edges = vec![
            GraphEdge {
                from: "src/app.ts".to_string(),
                to: "src/foo.ts".to_string(),
                label: "Foo".to_string(),
            },
            GraphEdge {
                from: "src/app.ts".to_string(),
                to: "src/bar.ts".to_string(),
                label: "Bar".to_string(),
            },
        ];

        write_snapshot(root, &snapshot);

        let state = SnapshotState::new();
        state.load(root).await.expect("load snapshot");

        let hover = state
            .find_import_at_position(
                "src/app.ts",
                Position {
                    line: 9,
                    character: 0,
                },
                None,
            )
            .await
            .expect("hover info");
        assert_eq!(hover.symbol, "Bar");
        assert_eq!(hover.source_file, "src/bar.ts");
        assert_eq!(hover.source_line, Some(7));

        cleanup_cache(root);
    }

    #[tokio::test]
    async fn hover_uses_cursor_symbol_when_document_text_is_available() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
        snapshot.files = vec![FileAnalysis {
            path: "src/foo.ts".to_string(),
            exports: vec![build_export("Foo", 1)],
            ..Default::default()
        }];

        write_snapshot(root, &snapshot);

        let state = SnapshotState::new();
        state.load(root).await.expect("load snapshot");

        let text = "export const Foo = 1;";
        assert!(
            state
                .get_hover_info(
                    "src/foo.ts",
                    Position {
                        line: 0,
                        character: 13,
                    },
                    Some(text),
                )
                .await
                .is_some()
        );
        assert!(
            state
                .get_hover_info(
                    "src/foo.ts",
                    Position {
                        line: 0,
                        character: 7,
                    },
                    Some(text),
                )
                .await
                .is_none()
        );

        cleanup_cache(root);
    }
}
