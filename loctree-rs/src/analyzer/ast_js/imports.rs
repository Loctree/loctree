//! Import declaration handling for JS/TS AST analysis.
//!
//! This module handles parsing of import declarations including:
//! - Static imports: `import { foo } from './bar'`
//! - Default imports: `import Default from './bar'`
//! - Namespace imports: `import * as NS from './bar'`
//! - Type imports: `import type { Foo } from './bar'`
//! - Side-effect imports: `import './styles.css'`
//!
//! Also handles namespace member access tracking for accurate symbol usage.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use oxc_ast::ast::*;

use crate::types::{ImportEntry, ImportKind, ImportSymbol};

use super::visitor::JsVisitor;

impl<'a> JsVisitor<'a> {
    /// Handle import declaration, extracting symbols and resolving paths.
    pub(super) fn handle_import_declaration(&mut self, decl: &ImportDeclaration<'a>) {
        let source = decl.source.value.to_string();
        let mut entry = ImportEntry::new(source.clone(), ImportKind::Static);
        entry.line = Some(self.get_line(decl.span));
        entry.resolved_path = self.resolve_path(&source);
        entry.is_bare = !source.starts_with('.') && !source.starts_with('/');
        if matches!(decl.import_kind, ImportOrExportKind::Type) {
            entry.kind = ImportKind::Type;
        }

        if let Some(specifiers) = &decl.specifiers {
            for spec in specifiers {
                match spec {
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                        entry.symbols.push(ImportSymbol {
                            name: s.local.name.to_string(),
                            alias: None,
                            is_default: true,
                        });
                    }
                    ImportDeclarationSpecifier::ImportSpecifier(s) => {
                        let name = match &s.imported {
                            ModuleExportName::IdentifierName(id) => id.name.to_string(),
                            ModuleExportName::IdentifierReference(id) => id.name.to_string(),
                            ModuleExportName::StringLiteral(str) => str.value.to_string(),
                        };

                        // Fix cmp_owned: compare &str directly
                        let alias = if *s.local.name != *name {
                            Some(s.local.name.to_string())
                        } else {
                            None
                        };

                        entry.symbols.push(ImportSymbol {
                            name,
                            alias,
                            is_default: false,
                        });
                    }
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                        let alias = s.local.name.to_string();
                        entry.symbols.push(ImportSymbol {
                            name: "*".to_string(),
                            alias: Some(alias.clone()),
                            is_default: false,
                        });
                        // Track namespace import for member expression resolution
                        self.namespace_imports
                            .insert(alias, (source.clone(), entry.resolved_path.clone()));
                    }
                }
            }
        } else {
            // Side-effect import
            entry.kind = ImportKind::SideEffect;
        }
        self.analysis.imports.push(entry);
    }

    /// Handle member expression to track namespace member access.
    ///
    /// This fixes Issue #5 - namespace member access not tracked.
    /// When using `NS.member` where NS is from `import * as NS`, we track
    /// which members are actually used.
    pub(super) fn handle_member_expression(&mut self, member: &MemberExpression<'a>) {
        if let MemberExpression::StaticMemberExpression(static_member) = member {
            // Check if the object is an identifier (e.g., `NS` in `NS.transform`)
            if let Expression::Identifier(obj_ident) = &static_member.object {
                let namespace_name = obj_ident.name.to_string();
                let member_name = static_member.property.name.to_string();

                // Check if this identifier is a namespace import
                if let Some((source, _resolved_path)) = self.namespace_imports.get(&namespace_name)
                {
                    // Find the import entry and add this member as a used symbol
                    for imp in &mut self.analysis.imports {
                        if &imp.source == source {
                            // Check if we already have this member symbol
                            let already_has_member =
                                imp.symbols.iter().any(|s| s.name == member_name);
                            if !already_has_member {
                                // Add the accessed member as an import symbol
                                imp.symbols.push(ImportSymbol {
                                    name: member_name.clone(),
                                    alias: None,
                                    is_default: false,
                                });
                            }
                            break;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use std::path::Path;

    /// Test import alias tracking - the original export name should be in `name`,
    /// and the local alias should be in `alias` field.
    #[test]
    fn test_import_alias_tracking() {
        let content = r#"
            // Test various import alias patterns
            import { Component as MyComponent } from 'react';
            import { useState as useStateHook, useEffect } from 'react';
            import DefaultExport from './module';
            import { originalName as renamedImport } from './utils';
            import { default as DefaultWithAlias } from './other';

            // Use the imports (to avoid them being marked as unused in other analyses)
            MyComponent();
            useStateHook();
            useEffect();
            DefaultExport();
            renamedImport();
            DefaultWithAlias();
        "#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/test.tsx"),
            Path::new("src"),
            None,
            None,
            "test.tsx".to_string(),
            &CommandDetectionConfig::default(),
        );

        // Find ALL react imports - they may be separate or merged depending on implementation
        let react_imports: Vec<_> = analysis
            .imports
            .iter()
            .filter(|i| i.source == "react")
            .collect();

        // Collect all react symbols across all import entries
        let all_react_symbols: Vec<_> = react_imports
            .iter()
            .flat_map(|i| i.symbols.iter())
            .collect();

        // We should have 3 symbols from react: Component, useState, useEffect
        assert_eq!(
            all_react_symbols.len(),
            3,
            "Should have 3 symbols from react total"
        );

        // Check Component as MyComponent
        let component_sym = all_react_symbols
            .iter()
            .find(|s| s.name == "Component")
            .expect("Should find Component symbol");
        assert_eq!(
            component_sym.alias.as_deref(),
            Some("MyComponent"),
            "Alias should be 'MyComponent'"
        );
        assert!(
            !component_sym.is_default,
            "Component is not a default export"
        );

        // Check useState as useStateHook
        let usestate_sym = all_react_symbols
            .iter()
            .find(|s| s.name == "useState")
            .expect("Should find useState symbol");

        assert_eq!(
            usestate_sym.name, "useState",
            "Original export name should be 'useState'"
        );
        assert_eq!(
            usestate_sym.alias.as_deref(),
            Some("useStateHook"),
            "Alias should be 'useStateHook'"
        );

        // Check useEffect (no alias)
        let useeffect_sym = all_react_symbols
            .iter()
            .find(|s| s.name == "useEffect")
            .expect("Should find useEffect symbol");

        assert_eq!(
            useeffect_sym.name, "useEffect",
            "Name should be 'useEffect'"
        );
        assert_eq!(useeffect_sym.alias, None, "Should have no alias");

        // Check utils import
        let utils_import = analysis
            .imports
            .iter()
            .find(|i| i.source == "./utils")
            .expect("Should find ./utils import");

        let original_sym = utils_import
            .symbols
            .iter()
            .find(|s| s.name == "originalName")
            .expect("Should find originalName symbol");

        assert_eq!(
            original_sym.name, "originalName",
            "Original name should be preserved"
        );
        assert_eq!(
            original_sym.alias.as_deref(),
            Some("renamedImport"),
            "Alias should be 'renamedImport'"
        );

        // Check { default as DefaultWithAlias } pattern
        let other_import = analysis
            .imports
            .iter()
            .find(|i| i.source == "./other")
            .expect("Should find ./other import");

        let default_alias_sym = &other_import.symbols[0];
        assert_eq!(
            default_alias_sym.name, "default",
            "Should track 'default' as the original export name"
        );
        assert_eq!(
            default_alias_sym.alias.as_deref(),
            Some("DefaultWithAlias"),
            "Alias should be 'DefaultWithAlias'"
        );
        assert!(
            !default_alias_sym.is_default,
            "This is NOT a default import (uses named import syntax)"
        );
    }

    /// Test for Issue #5: Namespace member access should be tracked
    /// When using `import * as namespace`, accessing `namespace.member` should be detected
    /// as usage of the `member` export from the imported module.
    #[test]
    fn test_namespace_member_access_tracking() {
        let content = r#"
            import * as amp from '@sveltejs/amp';
            import * as utils from './utils';

            // These member accesses should be tracked as using 'transform' and 'helper'
            const result = amp.transform(buffer);
            utils.helper();
            const value = utils.CONSTANT;
        "#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/app.ts"),
            Path::new("src"),
            None,
            None,
            "app.ts".to_string(),
            &CommandDetectionConfig::default(),
        );

        // Should have 2 imports
        assert_eq!(analysis.imports.len(), 2);

        // Check the amp import
        let amp_import = analysis
            .imports
            .iter()
            .find(|i| i.source == "@sveltejs/amp")
            .expect("Should find @sveltejs/amp import");

        // Should have both the namespace symbol (*) and the accessed member (transform)
        assert_eq!(
            amp_import.symbols.len(),
            2,
            "amp import should have 2 symbols: * and transform"
        );
        assert!(
            amp_import.symbols.iter().any(|s| s.name == "*"),
            "amp import should have namespace symbol"
        );
        assert!(
            amp_import.symbols.iter().any(|s| s.name == "transform"),
            "amp import should track 'transform' member access"
        );

        // Check the utils import
        let utils_import = analysis
            .imports
            .iter()
            .find(|i| i.source == "./utils")
            .expect("Should find ./utils import");

        // Should have namespace symbol (*), helper, and CONSTANT
        assert_eq!(
            utils_import.symbols.len(),
            3,
            "utils import should have 3 symbols: *, helper, and CONSTANT"
        );
        assert!(
            utils_import.symbols.iter().any(|s| s.name == "*"),
            "utils import should have namespace symbol"
        );
        assert!(
            utils_import.symbols.iter().any(|s| s.name == "helper"),
            "utils import should track 'helper' member access"
        );
        assert!(
            utils_import.symbols.iter().any(|s| s.name == "CONSTANT"),
            "utils import should track 'CONSTANT' member access"
        );
    }
}
