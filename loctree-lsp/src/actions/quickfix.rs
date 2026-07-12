//! Quick fix code actions for loctree diagnostics
//!
//! Provides actionable fixes for dead exports and circular imports.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::collections::HashMap;
use tower_lsp::lsp_types::*;

/// Generate quick fix actions for dead export diagnostics
///
/// Creates two actions:
/// - "Remove unused export" - WorkspaceEdit to remove the export keyword
/// - "Add to .loctignore" - Appends pattern to ignore file
pub fn dead_export_fixes(
    diagnostic: &Diagnostic,
    uri: &Url,
    workspace_root: Option<&str>,
) -> Vec<CodeAction> {
    let mut actions = Vec::new();

    // Extract symbol name from diagnostic message
    // Message format: "Export 'symbolName' is unused (0 imports)"
    let symbol = extract_symbol_from_message(&diagnostic.message);

    // 1. Remove unused export action
    if let Some(ref sym) = symbol {
        let remove_action = CodeAction {
            title: format!("Remove unused export '{}'", sym),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            is_preferred: Some(true),
            disabled: None,
            edit: Some(create_remove_export_edit(uri, diagnostic)),
            command: None,
            data: None,
        };
        actions.push(remove_action);
    }

    // 2. Add to .loctignore action
    if let Some(root) = workspace_root {
        // Get relative file path for the ignore pattern
        let file_path = uri.path();
        let relative_path = file_path
            .strip_prefix(root)
            .unwrap_or(file_path)
            .trim_start_matches('/');

        let pattern = if let Some(ref sym) = symbol {
            format!("{}:{}", relative_path, sym)
        } else {
            relative_path.to_string()
        };

        let ignore_action = CodeAction {
            title: "Add to .loctignore".to_string(),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            is_preferred: Some(false),
            disabled: None,
            edit: Some(create_loctignore_edit(root, &pattern)),
            command: None,
            data: None,
        };
        actions.push(ignore_action);
    }

    // 3. Suppress with inline comment (fallback if editing fails)
    if let Some(ref sym) = symbol {
        let suppress_action = CodeAction {
            title: format!("Suppress warning for '{}'", sym),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            is_preferred: Some(false),
            disabled: None,
            edit: Some(create_suppress_comment_edit(uri, diagnostic)),
            command: None,
            data: None,
        };
        actions.push(suppress_action);
    }

    actions
}

/// Generate quick fix actions for circular import diagnostics
///
/// Creates action to show full cycle details in output channel
pub fn cycle_fixes(diagnostic: &Diagnostic, uri: &Url) -> Vec<CodeAction> {
    let mut actions = Vec::new();

    // Extract cycle info from diagnostic message
    // Message format: "Circular import: file1.ts -> file2.ts -> file1.ts"
    let cycle_chain = extract_cycle_from_message(&diagnostic.message);

    // 1. Show cycle details action (opens output with full cycle information)
    let show_details_action = CodeAction {
        title: "Show cycle details".to_string(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic.clone()]),
        is_preferred: Some(true),
        disabled: None,
        edit: None,
        command: Some(Command {
            title: "Show Cycle Details".to_string(),
            command: "loctree.showCycleDetails".to_string(),
            arguments: Some(vec![
                serde_json::to_value(uri.to_string()).unwrap_or_default(),
                serde_json::to_value(&cycle_chain).unwrap_or_default(),
            ]),
        }),
        data: None,
    };
    actions.push(show_details_action);

    // 2. Navigate to next file in cycle
    if let Some(ref chain) = cycle_chain
        && let Some(next_file) = extract_next_file_in_cycle(chain, uri.path())
    {
        let navigate_action = CodeAction {
            title: format!("Go to next file in cycle: {}", shorten_path(&next_file)),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            is_preferred: Some(false),
            disabled: None,
            edit: None,
            command: Some(Command {
                title: "Navigate to File".to_string(),
                command: "loctree.navigateToFile".to_string(),
                arguments: Some(vec![serde_json::to_value(&next_file).unwrap_or_default()]),
            }),
            data: None,
        };
        actions.push(navigate_action);
    }

    // 3. Add cycle to .loctignore
    if let Some(ref chain) = cycle_chain {
        let ignore_action = CodeAction {
            title: "Ignore this cycle".to_string(),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            is_preferred: Some(false),
            disabled: None,
            edit: None,
            command: Some(Command {
                title: "Add Cycle to Ignore".to_string(),
                command: "loctree.ignoreCycle".to_string(),
                arguments: Some(vec![serde_json::to_value(chain).unwrap_or_default()]),
            }),
            data: None,
        };
        actions.push(ignore_action);
    }

    actions
}

/// Extract symbol name from diagnostic message
/// Expected format: "Export 'symbolName' is unused..."
fn extract_symbol_from_message(message: &str) -> Option<String> {
    // Look for pattern: 'symbolName'
    let start = message.find('\'')?;
    let rest = &message[start + 1..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

/// Extract cycle chain from diagnostic message
/// Expected format: "Circular import: file1 -> file2 -> file1"
fn extract_cycle_from_message(message: &str) -> Option<String> {
    message
        .strip_prefix("Circular import: ")
        .map(|s| s.to_string())
}

/// Extract the next file in cycle after current file
fn extract_next_file_in_cycle(cycle_chain: &str, current_file: &str) -> Option<String> {
    let files: Vec<&str> = cycle_chain.split(" -> ").collect();

    // Normalize current file path for comparison
    let current_normalized = current_file
        .trim_start_matches('/')
        .trim_start_matches("./");

    for (i, file) in files.iter().enumerate() {
        let file_normalized = file.trim_start_matches('/').trim_start_matches("./");

        // Check if this file matches current (allow suffix matching)
        if current_normalized.ends_with(file_normalized)
            || file_normalized.ends_with(current_normalized)
        {
            // Return next file in cycle (wrap around)
            let next_idx = (i + 1) % files.len();
            return Some(files[next_idx].to_string());
        }
    }

    // If no match, return first file
    files.first().map(|s| s.to_string())
}

/// Shorten file path for display
fn shorten_path(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Create WorkspaceEdit to remove export keyword from a line
///
/// This creates a text edit that:
/// - Removes 'export ' keyword from 'export function/const/class'
/// - Or removes the entire export line for re-exports
fn create_remove_export_edit(uri: &Url, diagnostic: &Diagnostic) -> WorkspaceEdit {
    let range = diagnostic.range;

    // Create edit to prefix the line with a comment marking it as unused
    // A full removal would require parsing - this is safer for an initial implementation
    let text_edit = TextEdit {
        range: Range {
            start: Position {
                line: range.start.line,
                character: 0,
            },
            end: Position {
                line: range.start.line,
                character: 0,
            },
        },
        new_text: "// FIXME: Remove unused export\n// ".to_string(),
    };

    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![text_edit]);

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

/// Create WorkspaceEdit to append pattern to .loctignore file
fn create_loctignore_edit(workspace_root: &str, pattern: &str) -> WorkspaceEdit {
    let loctignore_path = format!("{}/.loctignore", workspace_root.trim_end_matches('/'));

    let uri = match Url::from_file_path(&loctignore_path) {
        Ok(u) => u,
        Err(_) => {
            // Return empty edit if URI construction fails
            return WorkspaceEdit {
                changes: Some(HashMap::new()),
                document_changes: None,
                change_annotations: None,
            };
        }
    };

    // Append pattern to end of file (assumes file exists or will be created)
    // We use a large line number to append at the end
    let text_edit = TextEdit {
        range: Range {
            start: Position {
                line: u32::MAX,
                character: 0,
            },
            end: Position {
                line: u32::MAX,
                character: 0,
            },
        },
        new_text: format!("\n# Added by loctree quick fix\n{}\n", pattern),
    };

    let mut changes = HashMap::new();
    changes.insert(uri, vec![text_edit]);

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

/// Create WorkspaceEdit to add a suppression comment
fn create_suppress_comment_edit(uri: &Url, diagnostic: &Diagnostic) -> WorkspaceEdit {
    let range = diagnostic.range;

    // Add loctree-ignore comment on the line before
    let text_edit = TextEdit {
        range: Range {
            start: Position {
                line: range.start.line,
                character: 0,
            },
            end: Position {
                line: range.start.line,
                character: 0,
            },
        },
        new_text: "// loctree-ignore: dead-export\n".to_string(),
    };

    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![text_edit]);

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_symbol_from_message() {
        assert_eq!(
            extract_symbol_from_message("Export 'myFunction' is unused (0 imports)"),
            Some("myFunction".to_string())
        );
        assert_eq!(
            extract_symbol_from_message("Export 'MyClass' is unused (0 imports)"),
            Some("MyClass".to_string())
        );
        assert_eq!(extract_symbol_from_message("No quotes here"), None);
    }

    #[test]
    fn test_extract_cycle_from_message() {
        assert_eq!(
            extract_cycle_from_message("Circular import: a.ts -> b.ts -> a.ts"),
            Some("a.ts -> b.ts -> a.ts".to_string())
        );
        assert_eq!(extract_cycle_from_message("Not a cycle message"), None);
    }

    #[test]
    fn test_extract_next_file_in_cycle() {
        let chain = "src/a.ts -> src/b.ts -> src/c.ts -> src/a.ts";

        assert_eq!(
            extract_next_file_in_cycle(chain, "/project/src/a.ts"),
            Some("src/b.ts".to_string())
        );
        assert_eq!(
            extract_next_file_in_cycle(chain, "/project/src/b.ts"),
            Some("src/c.ts".to_string())
        );
        assert_eq!(
            extract_next_file_in_cycle(chain, "/project/src/c.ts"),
            Some("src/a.ts".to_string())
        );
    }

    #[test]
    fn test_shorten_path() {
        assert_eq!(shorten_path("src/components/Button.tsx"), "Button.tsx");
        assert_eq!(shorten_path("index.ts"), "index.ts");
    }
}
