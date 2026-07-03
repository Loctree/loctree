//! Call expression handling for JS/TS AST analysis.
//!
//! This module handles:
//! - Tauri invoke() command detection
//! - Event emit/listen detection
//! - Dynamic imports: `import('./module')`
//! - Variable declarator handling (for const event name resolution)
//! - Plugin command parsing
//! - Flow type annotation detection
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use oxc_ast::ast::*;

use crate::types::{CommandPayloadCasing, CommandRef, EventRef, ImportEntry, ImportKind};

use super::visitor::JsVisitor;

/// Parse Tauri plugin command pattern from invoke() argument.
///
/// Tauri plugins use the pattern: `invoke('plugin:name|command')`
/// - `plugin:window|set_icon` -> ("set_icon", Some("window"))
/// - `get_users` -> ("get_users", None)
///
/// Returns (command_name, optional_plugin_name)
pub(super) fn parse_plugin_command(cmd_name: &str) -> (String, Option<String>) {
    // Pattern: plugin:<plugin_name>|<command_name>
    if let Some(rest) = cmd_name.strip_prefix("plugin:")
        && let Some(pipe_pos) = rest.find('|')
    {
        let plugin_name = &rest[..pipe_pos];
        let command = &rest[pipe_pos + 1..];
        if !plugin_name.is_empty() && !command.is_empty() {
            return (command.to_string(), Some(plugin_name.to_string()));
        }
    }
    // Not a plugin command
    (cmd_name.to_string(), None)
}

/// Check if file uses Flow type annotations.
///
/// Flow files start with `// @flow` or `/* @flow */`
pub(super) fn is_flow_file(content: &str) -> bool {
    // Find safe UTF-8 boundary (stable alternative to floor_char_boundary)
    let end_idx = content.len().min(1000);
    let safe_end = (0..=end_idx)
        .rev()
        .find(|&i| content.is_char_boundary(i))
        .unwrap_or(0);
    let first_1000 = &content[..safe_end];
    first_1000.contains("@flow")
        || first_1000.contains("// @flow")
        || first_1000.contains("/* @flow */")
}

impl<'a> JsVisitor<'a> {
    /// Handle dynamic import expression: `import('./module')`
    pub(super) fn handle_import_expression(&mut self, expr: &ImportExpression<'a>) {
        if let Expression::StringLiteral(s) = &expr.source {
            let source = s.value.to_string();

            // Track in dynamic_imports for backward compatibility
            if !self.analysis.dynamic_imports.contains(&source) {
                self.analysis.dynamic_imports.push(source.clone());
            }

            // Also create an ImportEntry with Dynamic kind for graph edges
            let mut entry = ImportEntry::new(source.clone(), ImportKind::Dynamic);
            entry.resolved_path = self.resolve_path(&source);
            entry.is_bare = !source.starts_with('.') && !source.starts_with('/');
            // Dynamic imports don't have specific symbols - they import the whole module
            self.analysis.imports.push(entry);

            // Also track resolved path in dynamic_imports if available
            if let Some(resolved) = self.resolve_path(&source)
                && !self.analysis.dynamic_imports.contains(&resolved)
            {
                self.analysis.dynamic_imports.push(resolved);
            }
        }
    }

    /// Handle call expression for command and event detection.
    pub(super) fn handle_call_expression(&mut self, call: &CallExpression<'a>) {
        let callee_name = match &call.callee {
            Expression::Identifier(ident) => Some(ident.name.to_string()),
            Expression::StaticMemberExpression(member) => {
                // Handle obj.emit(...)
                Some(member.property.name.to_string())
            }
            _ => None,
        };

        if let Some(name) = callee_name {
            self.handle_command_detection(&name, call);
            self.handle_event_detection(&name, call);
        }
    }

    /// Detect Tauri invoke patterns and record command calls.
    fn handle_command_detection(&mut self, name: &str, call: &CallExpression<'a>) {
        let name_lower = name.to_lowercase();
        if matches!(
            name_lower.as_str(),
            "registercommand" | "registertexteditorcommand"
        ) {
            // VSCode command registration is not a Tauri invoke.
            return;
        }
        let is_potential_command = name_lower.contains("invoke") || name.contains("Command");

        if is_potential_command
            && !self.command_cfg.dom_exclusions.contains(name)
            && !self.command_cfg.non_invoke_exclusions.contains(name)
            && let Some(arg) = call.arguments.first()
        {
            // Extract command name from first argument (string literal or template literal)
            let cmd_name = match arg {
                Argument::StringLiteral(s) => Some(s.value.to_string()),
                Argument::TemplateLiteral(t) => {
                    // Only extract if it's a simple template without expressions
                    if t.quasis.len() == 1 && t.expressions.is_empty() {
                        t.quasis.first().map(|q| q.value.raw.to_string())
                    } else {
                        None
                    }
                }
                _ => None,
            };

            // Only record command if we have an actual command name (from the argument).
            // Skip if cmd_name is None - that means we couldn't extract the command name
            // (e.g., dynamic command name or wrapper function definition).
            if let Some(cmd_name) = cmd_name {
                // VSCode-style commands are dotted (e.g., loctree.analyzeImpact). Those are not Tauri.
                if !name_lower.contains("invoke") && cmd_name.contains('.') {
                    return;
                }

                // Filter out command names that are clearly not Tauri commands
                // (e.g., CLI tools, shell commands found in scripts/config files)
                if self.command_cfg.invalid_command_names.contains(&cmd_name) {
                    // Skip - not a real Tauri command
                    return;
                }

                // Parse plugin command pattern: plugin:name|command
                // e.g., "plugin:window|set_icon" -> plugin_name="window", actual_cmd="set_icon"
                let (actual_cmd, plugin_name) = parse_plugin_command(&cmd_name);

                // Payload casing drift: if command name looks snake_case and payload keys are camelCase
                let mut casing_issues: Vec<CommandPayloadCasing> = Vec::new();
                if actual_cmd.contains('_')
                    && let Some(Argument::ObjectExpression(obj)) = call.arguments.first()
                {
                    for prop in &obj.properties {
                        if let ObjectPropertyKind::ObjectProperty(p) = prop
                            && let PropertyKey::Identifier(id) = &p.key
                        {
                            let key = id.name.to_string();
                            if key.chars().any(|c| c.is_uppercase()) {
                                casing_issues.push(CommandPayloadCasing {
                                    command: actual_cmd.clone(),
                                    key,
                                    path: self.path.to_string_lossy().to_string(),
                                    line: self.get_line(p.span),
                                });
                            }
                        }
                    }
                }
                self.analysis.command_payload_casing.extend(casing_issues);

                let generic = call
                    .type_arguments
                    .as_ref()
                    .and_then(|params| params.params.first().map(JsVisitor::type_to_string));

                let line = self.get_line(call.span);

                self.analysis.command_calls.push(CommandRef {
                    name: actual_cmd,
                    exposed_name: None,
                    line,
                    generic_type: generic,
                    payload: None,
                    plugin_name,
                });
            }
        }
    }

    /// Detect event emit/listen patterns and record event references.
    fn handle_event_detection(&mut self, name: &str, call: &CallExpression<'a>) {
        // Events: emit / listen
        // Heuristic: function name contains "emit" or "listen"
        let is_emit = name == "emit" || name.ends_with("emit"); // e.g. window.emit, appWindow.emit
        let is_listen = name == "listen" || name.contains("listen"); // e.g. appWindow.listen, listenTo

        if (is_emit || is_listen)
            && let Some(arg) = call.arguments.first()
        {
            // Resolve event name from argument (literal or constant)
            let (event_name, raw_name, kind, is_dynamic) = match arg {
                Argument::StringLiteral(s) => (
                    s.value.to_string(),
                    Some(s.value.to_string()),
                    "literal",
                    false,
                ),
                Argument::TemplateLiteral(t) => {
                    if t.quasis.len() == 1 && t.expressions.is_empty() {
                        // Static template literal: `event-name` with no expressions
                        if let Some(q) = t.quasis.first() {
                            (
                                q.value.raw.to_string(),
                                Some(q.value.raw.to_string()),
                                "literal",
                                false,
                            )
                        } else {
                            ("?".to_string(), None, "unknown", false)
                        }
                    } else {
                        // Dynamic template literal: `event:${id}` with expressions
                        // Build pattern by replacing ${...} with *
                        let mut pattern = String::new();
                        let mut raw_pattern = String::new();
                        for (i, quasi) in t.quasis.iter().enumerate() {
                            pattern.push_str(&quasi.value.raw);
                            raw_pattern.push_str(&quasi.value.raw);
                            if i < t.expressions.len() {
                                pattern.push('*');
                                raw_pattern.push_str("${...}");
                            }
                        }
                        (pattern, Some(format!("`{}`", raw_pattern)), "dynamic", true)
                    }
                }
                Argument::Identifier(id) => {
                    let id_name = id.name.to_string();
                    if let Some(val) = self.analysis.event_consts.get(&id_name) {
                        (val.clone(), Some(id_name), "const", false)
                    } else {
                        (id_name.clone(), Some(id_name), "ident", false)
                    }
                }
                _ => ("?".to_string(), None, "unknown", false),
            };

            let line = self.get_line(call.span);

            if is_emit {
                let payload = call.arguments.get(1).map(|_| "payload".to_string()); // Simplified payload detection
                self.analysis.event_emits.push(EventRef {
                    raw_name,
                    name: event_name,
                    line,
                    kind: format!("emit_{}", kind),
                    awaited: false, // Todo: check await parent
                    payload,
                    is_dynamic,
                });
            } else {
                // listen
                // Todo: check await parent
                self.analysis.event_listens.push(EventRef {
                    raw_name,
                    name: event_name,
                    line,
                    kind: format!("listen_{}", kind),
                    awaited: false,
                    payload: None,
                    is_dynamic,
                });
            }
        }
    }

    /// Handle variable declarator for const event name resolution.
    ///
    /// Captures constants like `const MY_EVENT = "event-name";` for event resolution.
    pub(super) fn handle_variable_declarator(&mut self, decl: &VariableDeclarator<'a>) {
        if let BindingPattern::BindingIdentifier(id) = &decl.id
            && let Some(init) = &decl.init
            && let Expression::StringLiteral(s) = init
        {
            // Store const name -> value mapping
            self.analysis
                .event_consts
                .insert(id.name.to_string(), s.value.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::types::ImportKind;
    use std::path::Path;

    #[test]
    fn test_ast_events_and_consts() {
        let content = r#"
            const MY_EVENT = "user-login";
            const ANOTHER_EVENT = "data-update";

            // Literal emit
            emit("literal-event", { id: 1 });

            // Constant emit
            emit(MY_EVENT, "payload");

            // Listen
            listen(ANOTHER_EVENT, () => {});
            appWindow.listen("window-event", handler);
        "#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/events.ts"),
            Path::new("src"),
            None,
            None,
            "events.ts".to_string(),
            &CommandDetectionConfig::default(),
        );

        // Constants
        assert_eq!(
            analysis.event_consts.get("MY_EVENT").map(|s| s.as_str()),
            Some("user-login")
        );
        assert_eq!(
            analysis
                .event_consts
                .get("ANOTHER_EVENT")
                .map(|s| s.as_str()),
            Some("data-update")
        );

        // Emits
        let emits: Vec<_> = analysis
            .event_emits
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert!(emits.contains(&"literal-event"));
        assert!(emits.contains(&"user-login")); // Resolved from const

        // Listens
        let listens: Vec<_> = analysis
            .event_listens
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert!(listens.contains(&"data-update")); // Resolved from const
        assert!(listens.contains(&"window-event"));
    }

    #[test]
    fn test_dynamic_imports_added_to_imports_list() {
        let content = r#"
            // Regular static import
            import { Button } from './Button';

            // Dynamic import with import()
            const LazyComponent = import('./LazyComponent');

            // React.lazy with dynamic import
            const LazyPage = React.lazy(() => import('./pages/Home'));
        "#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/App.tsx"),
            Path::new("src"),
            None,
            None,
            "App.tsx".to_string(),
            &CommandDetectionConfig::default(),
        );

        // Check that static import is captured
        assert!(analysis.imports.iter().any(|i| i.source == "./Button"));

        // Check that dynamic imports are captured in imports list (not just dynamic_imports)
        assert!(
            analysis
                .imports
                .iter()
                .any(|i| i.source == "./LazyComponent")
        );
        assert!(analysis.imports.iter().any(|i| i.source == "./pages/Home"));

        // Verify they're marked as Dynamic kind
        let lazy_import = analysis
            .imports
            .iter()
            .find(|i| i.source == "./LazyComponent")
            .unwrap();
        assert!(matches!(lazy_import.kind, ImportKind::Dynamic));
    }

    #[test]
    fn test_flow_file_detection() {
        // Test Flow annotation detection at start of file
        let flow_content = r#"
// @flow
export type MyType = {
    name: string,
    age: number,
};

export const myValue = 42;
        "#;

        let analysis = analyze_js_file_ast(
            flow_content,
            Path::new("src/types.js"),
            Path::new("src"),
            None,
            None,
            "types.js".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(
            analysis.is_flow_file,
            "File with @flow annotation should be detected as Flow file"
        );
    }

    #[test]
    fn test_non_flow_file() {
        // Test that files without @flow annotation are not marked as Flow
        let regular_content = r#"
export type MyType = {
    name: string,
    age: number,
};

export const myValue = 42;
        "#;

        let analysis = analyze_js_file_ast(
            regular_content,
            Path::new("src/types.ts"),
            Path::new("src"),
            None,
            None,
            "types.ts".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(
            !analysis.is_flow_file,
            "File without @flow annotation should NOT be detected as Flow file"
        );
    }
}
