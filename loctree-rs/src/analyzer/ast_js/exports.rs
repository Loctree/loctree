//! Export declaration handling for JS/TS AST analysis.
//!
//! This module handles parsing of export declarations including:
//! - Named exports: `export const x = 1;`, `export function foo() {}`
//! - Default exports: `export default class MyClass {}`
//! - Re-exports: `export { foo } from './bar'`, `export * from './module'`
//! - TypeScript-specific: interfaces, type aliases, enums
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use oxc_ast::ast::*;
use oxc_ast_visit::Visit;

use crate::types::{ExportSymbol, ReexportEntry, ReexportKind};

// Note: Visit trait is imported for use in handle_ methods that need to call
// visitor methods like visit_variable_declaration, visit_function_body, visit_class

use super::visitor::JsVisitor;

impl<'a> JsVisitor<'a> {
    /// Handle named export declaration.
    ///
    /// Supports:
    /// - Re-exports: `export { foo } from 'bar'`
    /// - Variable exports: `export const x = 1;`
    /// - Function exports: `export function foo() {}`
    /// - Class exports: `export class MyClass {}`
    /// - TypeScript exports: interfaces, types, enums
    /// - Named specifier exports: `export { foo };`
    pub(super) fn handle_export_named_declaration(&mut self, decl: &ExportNamedDeclaration<'a>) {
        let line = self.get_line(decl.span);

        if let Some(src) = &decl.source {
            // Re-export: export { foo } from 'bar' or export { foo as bar } from 'baz'
            let source = src.value.to_string();
            let resolved = self.resolve_path(&source);
            let mut names: Vec<(String, String)> = Vec::new();

            for spec in &decl.specifiers {
                // local = name in source module
                let local_name = match &spec.local {
                    ModuleExportName::IdentifierName(id) => id.name.to_string(),
                    ModuleExportName::IdentifierReference(id) => id.name.to_string(),
                    ModuleExportName::StringLiteral(str) => str.value.to_string(),
                };
                // exported = name as exported (may be aliased)
                let exported_name = match &spec.exported {
                    ModuleExportName::IdentifierName(id) => id.name.to_string(),
                    ModuleExportName::IdentifierReference(id) => id.name.to_string(),
                    ModuleExportName::StringLiteral(str) => str.value.to_string(),
                };
                names.push((local_name, exported_name));
            }

            self.analysis.reexports.push(ReexportEntry {
                source,
                kind: ReexportKind::Named(names.clone()),
                resolved,
            });

            // Also track as exports (use exported name, not local)
            for (_, exported_name) in &names {
                self.analysis.exports.push(ExportSymbol::new(
                    exported_name.clone(),
                    "reexport",
                    "named",
                    Some(line),
                ));
            }
        } else {
            // Named export: export const x = 1;
            if let Some(declaration) = &decl.declaration {
                match declaration {
                    Declaration::VariableDeclaration(var) => {
                        for d in &var.declarations {
                            if let BindingPattern::BindingIdentifier(id) = &d.id {
                                let name = id.name.to_string();
                                // Check if it's a function expression or arrow function
                                if let Some(init) = &d.init {
                                    if let Expression::FunctionExpression(fun) = init {
                                        let params = self.extract_function_params(fun);
                                        self.analysis.exports.push(ExportSymbol::with_params(
                                            name,
                                            "function",
                                            "named",
                                            Some(line),
                                            params,
                                        ));
                                        self.record_function_signature(id.name.as_str(), fun);
                                    } else if let Expression::ArrowFunctionExpression(fun) = init {
                                        let params = self.extract_arrow_params(fun);
                                        self.analysis.exports.push(ExportSymbol::with_params(
                                            name,
                                            "function",
                                            "named",
                                            Some(line),
                                            params,
                                        ));
                                        self.record_arrow_signature(id.name.as_str(), fun);
                                    } else {
                                        self.analysis.exports.push(ExportSymbol::new(
                                            name,
                                            "var",
                                            "named",
                                            Some(line),
                                        ));
                                    }
                                } else {
                                    self.analysis.exports.push(ExportSymbol::new(
                                        name,
                                        "var",
                                        "named",
                                        Some(line),
                                    ));
                                }
                            }
                        }
                        // Continue traversal
                        self.visit_variable_declaration(var);
                    }
                    Declaration::FunctionDeclaration(f) => {
                        if let Some(id) = &f.id {
                            let name = id.name.to_string();
                            let params = self.extract_function_params(f);
                            self.analysis.exports.push(ExportSymbol::with_params(
                                name,
                                "function",
                                "named",
                                Some(line),
                                params,
                            ));
                            self.record_function_signature(id.name.as_str(), f);
                        }
                        // Continue traversal
                        if let Some(body) = &f.body {
                            self.visit_function_body(body);
                        }
                    }
                    Declaration::ClassDeclaration(c) => {
                        if let Some(id) = &c.id {
                            let name = id.name.to_string();
                            self.analysis.exports.push(ExportSymbol::new(
                                name,
                                "class",
                                "named",
                                Some(line),
                            ));
                        }
                        // Continue traversal
                        self.visit_class(c);
                    }
                    Declaration::TSInterfaceDeclaration(i) => {
                        let name = i.id.name.to_string();
                        self.analysis.exports.push(ExportSymbol::new(
                            name,
                            "interface",
                            "named",
                            Some(line),
                        ));
                    }
                    Declaration::TSTypeAliasDeclaration(t) => {
                        let name = t.id.name.to_string();
                        self.analysis.exports.push(ExportSymbol::new(
                            name,
                            "type",
                            "named",
                            Some(line),
                        ));
                    }
                    Declaration::TSEnumDeclaration(e) => {
                        let name = e.id.name.to_string();
                        self.analysis.exports.push(ExportSymbol::new(
                            name,
                            "enum",
                            "named",
                            Some(line),
                        ));
                    }
                    _ => {}
                }
            }

            // export { foo };
            for spec in &decl.specifiers {
                let name = match &spec.exported {
                    ModuleExportName::IdentifierName(id) => id.name.to_string(),
                    ModuleExportName::IdentifierReference(id) => id.name.to_string(),
                    ModuleExportName::StringLiteral(str) => str.value.to_string(),
                };
                self.analysis
                    .exports
                    .push(ExportSymbol::new(name, "named", "named", Some(line)));
            }
        }
    }

    /// Handle default export declaration.
    ///
    /// Default exports are always named "default" for matching with `import X from './file'`.
    /// The actual function/class name is stored in export_type for debugging.
    pub(super) fn handle_export_default_declaration(
        &mut self,
        decl: &ExportDefaultDeclaration<'a>,
    ) {
        let line = self.get_line(decl.span);
        match &decl.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                let original_name = f.id.as_ref().map(|i| i.name.to_string());
                let params = self.extract_function_params(f);
                self.analysis.exports.push(ExportSymbol::with_params(
                    "default".to_string(),
                    "default",
                    original_name.as_deref().unwrap_or("default"),
                    Some(line),
                    params,
                ));
                if let Some(name) = &original_name {
                    self.record_function_signature(name, f);
                }

                // Continue traversal
                if let Some(body) = &f.body {
                    self.visit_function_body(body);
                }
            }
            ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                let original_name = c.id.as_ref().map(|i| i.name.to_string());
                self.analysis.exports.push(ExportSymbol::new(
                    "default".to_string(),
                    "default",
                    original_name.as_deref().unwrap_or("default"),
                    Some(line),
                ));

                // Continue traversal
                self.visit_class(c);
            }
            ExportDefaultDeclarationKind::TSInterfaceDeclaration(i) => {
                self.analysis.exports.push(ExportSymbol::new(
                    "default".to_string(),
                    "default",
                    &i.id.name,
                    Some(line),
                ));
                // Interfaces don't have executable code bodies (calls), so no need to traverse deep for commands
            }
            _ => {
                self.analysis.exports.push(ExportSymbol::new(
                    "default".to_string(),
                    "default",
                    "default",
                    Some(line),
                ));
            }
        };
    }

    /// Handle star re-export: `export * from './module'`
    pub(super) fn handle_export_all_declaration(&mut self, decl: &ExportAllDeclaration<'a>) {
        let source = decl.source.value.to_string();
        let resolved = self.resolve_path(&source);
        self.analysis.reexports.push(ReexportEntry {
            source,
            kind: ReexportKind::Star,
            resolved,
        });
    }
}
