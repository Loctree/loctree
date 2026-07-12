//! Refactoring code actions for loctree LSP
//!
//! Provides refactoring actions for dependency graph analysis:
//! - Show import graph - opens loctree HTML report
//! - Find all consumers - lists files that import the current file/symbol
//! - Analyze impact - shows what breaks if this file changes
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use tower_lsp::lsp_types::{CodeAction, CodeActionKind, Command, Url};

/// Generate refactoring actions for an export symbol
///
/// Provides actions to explore the dependency graph for a specific symbol.
///
/// # Arguments
/// * `symbol` - The symbol name (export) to analyze
/// * `uri` - The file URI containing the symbol
/// * `import_count` - Number of files that import this symbol
///
/// # Returns
/// Vector of CodeActions for refactoring operations
pub fn export_refactors(symbol: &str, uri: &Url, import_count: usize) -> Vec<CodeAction> {
    let mut actions = Vec::new();

    // "Show import graph" - opens HTML report focused on this symbol
    actions.push(CodeAction {
        title: format!("Show '{}' in dependency graph", symbol),
        kind: Some(CodeActionKind::REFACTOR),
        diagnostics: None,
        edit: None,
        command: Some(Command {
            title: "Open Loctree Report".to_string(),
            command: "loctree.openReport".to_string(),
            arguments: Some(vec![serde_json::json!({
                "symbol": symbol,
                "file": uri.path()
            })]),
        }),
        is_preferred: None,
        disabled: None,
        data: None,
    });

    // "Find all consumers" - only if there are importers
    if import_count > 0 {
        actions.push(CodeAction {
            title: format!("Find all {} consumers of '{}'", import_count, symbol),
            kind: Some(CodeActionKind::REFACTOR),
            diagnostics: None,
            edit: None,
            command: Some(Command {
                title: "Find References".to_string(),
                command: "editor.action.findReferences".to_string(),
                arguments: None,
            }),
            is_preferred: None,
            disabled: None,
            data: None,
        });
    } else {
        // Symbol has no consumers - might be dead code
        actions.push(CodeAction {
            title: format!("'{}' has no consumers (potential dead code)", symbol),
            kind: Some(CodeActionKind::REFACTOR),
            diagnostics: None,
            edit: None,
            command: Some(Command {
                title: "Check Dead Exports".to_string(),
                command: "loctree.checkDeadExports".to_string(),
                arguments: Some(vec![serde_json::json!({
                    "symbol": symbol,
                    "file": uri.path()
                })]),
            }),
            is_preferred: None,
            disabled: None,
            data: None,
        });
    }

    actions
}

/// Generate file-level refactoring actions
///
/// Provides actions for analyzing file-level dependencies and impact.
///
/// # Arguments
/// * `uri` - The file URI to analyze
/// * `file_path` - Relative file path within the project
/// * `consumer_count` - Number of files that import this file
///
/// # Returns
/// Vector of CodeActions for file-level refactoring operations
pub fn file_refactors(uri: &Url, file_path: &str, consumer_count: usize) -> Vec<CodeAction> {
    let mut actions = Vec::new();

    // "Analyze impact" - shows what breaks if this file changes
    actions.push(CodeAction {
        title: format!(
            "Analyze change impact ({} direct consumers)",
            consumer_count
        ),
        kind: Some(CodeActionKind::REFACTOR),
        diagnostics: None,
        edit: None,
        command: Some(Command {
            title: "Run Impact Analysis".to_string(),
            command: "loctree.analyzeImpact".to_string(),
            arguments: Some(vec![serde_json::json!({
                "file": file_path,
                "uri": uri.to_string()
            })]),
        }),
        is_preferred: None,
        disabled: None,
        data: None,
    });

    // "Show file in dependency graph"
    actions.push(CodeAction {
        title: "Show file in dependency graph".to_string(),
        kind: Some(CodeActionKind::REFACTOR),
        diagnostics: None,
        edit: None,
        command: Some(Command {
            title: "Open Loctree Report".to_string(),
            command: "loctree.openReport".to_string(),
            arguments: Some(vec![serde_json::json!({
                "file": file_path
            })]),
        }),
        is_preferred: None,
        disabled: None,
        data: None,
    });

    // "Find all files that import this"
    if consumer_count > 0 {
        actions.push(CodeAction {
            title: format!("Find all {} importers of this file", consumer_count),
            kind: Some(CodeActionKind::REFACTOR),
            diagnostics: None,
            edit: None,
            command: Some(Command {
                title: "Find File Importers".to_string(),
                command: "loctree.findImporters".to_string(),
                arguments: Some(vec![serde_json::json!({
                    "file": file_path
                })]),
            }),
            is_preferred: None,
            disabled: None,
            data: None,
        });
    }

    actions
}

/// Generate refactoring actions for a cycle diagnostic
///
/// When a file is involved in a circular dependency, provide actions to analyze it.
///
/// # Arguments
/// * `file_path` - The file involved in the cycle
/// * `cycle_files` - All files in the cycle chain
///
/// # Returns
/// Vector of CodeActions for breaking cycles
pub fn cycle_refactors(file_path: &str, cycle_files: &[String]) -> Vec<CodeAction> {
    let mut actions = Vec::new();

    // "Show cycle in dependency graph"
    actions.push(CodeAction {
        title: format!("Show cycle ({} files) in graph", cycle_files.len()),
        kind: Some(CodeActionKind::REFACTOR),
        diagnostics: None,
        edit: None,
        command: Some(Command {
            title: "Open Loctree Cycles View".to_string(),
            command: "loctree.showCycles".to_string(),
            arguments: Some(vec![serde_json::json!({
                "file": file_path,
                "cycle": cycle_files
            })]),
        }),
        is_preferred: None,
        disabled: None,
        data: None,
    });

    // "Analyze cycle breaking options"
    actions.push(CodeAction {
        title: "Analyze cycle breaking options".to_string(),
        kind: Some(CodeActionKind::REFACTOR),
        diagnostics: None,
        edit: None,
        command: Some(Command {
            title: "Analyze Cycle".to_string(),
            command: "loctree.analyzeCycle".to_string(),
            arguments: Some(vec![serde_json::json!({
                "file": file_path,
                "cycle": cycle_files
            })]),
        }),
        is_preferred: None,
        disabled: None,
        data: None,
    });

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_refactors_with_consumers() {
        let uri = Url::parse("file:///src/utils.ts").unwrap();
        let actions = export_refactors("formatDate", &uri, 5);

        assert_eq!(actions.len(), 2);
        assert!(actions[0].title.contains("dependency graph"));
        assert!(actions[1].title.contains("5 consumers"));
        assert_eq!(actions[0].kind, Some(CodeActionKind::REFACTOR));
    }

    #[test]
    fn test_export_refactors_no_consumers() {
        let uri = Url::parse("file:///src/unused.ts").unwrap();
        let actions = export_refactors("deadFunction", &uri, 0);

        assert_eq!(actions.len(), 2);
        assert!(actions[1].title.contains("no consumers"));
    }

    #[test]
    fn test_file_refactors() {
        let uri = Url::parse("file:///src/index.ts").unwrap();
        let actions = file_refactors(&uri, "src/index.ts", 10);

        assert_eq!(actions.len(), 3);
        assert!(actions[0].title.contains("impact"));
        assert!(actions[1].title.contains("dependency graph"));
        assert!(actions[2].title.contains("10 importers"));
    }

    #[test]
    fn test_file_refactors_no_importers() {
        let uri = Url::parse("file:///src/leaf.ts").unwrap();
        let actions = file_refactors(&uri, "src/leaf.ts", 0);

        // No "find importers" action when count is 0
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn test_cycle_refactors() {
        let cycle_files = vec![
            "src/a.ts".to_string(),
            "src/b.ts".to_string(),
            "src/c.ts".to_string(),
        ];
        let actions = cycle_refactors("src/a.ts", &cycle_files);

        assert_eq!(actions.len(), 2);
        assert!(actions[0].title.contains("3 files"));
        assert!(actions[1].title.contains("breaking options"));
    }
}
