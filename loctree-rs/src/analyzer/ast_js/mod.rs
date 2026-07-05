//! JavaScript/TypeScript AST analysis module.
//!
//! This module provides comprehensive analysis of JS/TS files using OXC AST parser,
//! including support for:
//! - Import/export detection (static, dynamic, re-exports)
//! - Svelte and Vue Single File Component parsing
//! - Tauri command detection
//! - Event emit/listen detection
//! - TypeScript type signature tracking
//! - Flow type annotation detection
//!
//! # Module Structure
//!
//! - `config`: Command detection configuration and exclusion lists
//! - `sfc`: Single File Component script/template extraction
//! - `template`: Svelte/Vue template usage parsing
//! - `visitor`: Core AST visitor and helper methods
//! - `imports`: Import declaration handling
//! - `exports`: Export declaration handling
//! - `calls`: Call expression and dynamic import handling
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

mod calls;
mod config;
mod exports;
mod imports;
mod sfc;
mod template;
mod visitor;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::{AstKind, ast::*};
use oxc_ast_visit::{Visit, walk::walk_expression};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::GetSpan;
use oxc_span::SourceType;

use crate::types::{ExportSymbol, FileAnalysis, LocalSymbol, ParamInfo, SymbolUsage};

use super::resolvers::TsPathResolver;

// Re-export public types
pub use config::CommandDetectionConfig;

// Use internal functions from submodules
use calls::is_flow_file;
use sfc::{
    SnippetDeclaration, extract_astro_frontmatter, extract_astro_scripts, extract_astro_styles,
    extract_svelte_script, extract_svelte_snippets, extract_svelte_template, extract_svelte5_runes,
    extract_vue_script, extract_vue_template,
};
use template::{parse_svelte_template_usages, parse_vue_template_usages};
use visitor::JsVisitor;

/// Analyze JS/TS file using OXC AST parser.
///
/// This is the main entry point for JavaScript/TypeScript analysis. It:
/// 1. Parses the source code into an AST
/// 2. Extracts script content from SFC files (Svelte/Vue)
/// 3. Traverses the AST to collect imports, exports, commands, and events
/// 4. Parses templates for function/variable usage (Svelte/Vue)
/// 5. Uses semantic analysis to track local symbol references
///
/// # Arguments
///
/// * `content` - The source file content
/// * `path` - Path to the source file
/// * `root` - Root directory of the project
/// * `extensions` - Optional set of valid file extensions for resolution
/// * `ts_resolver` - Optional TypeScript path resolver
/// * `relative` - Relative path for the analysis result
/// * `command_cfg` - Command detection configuration
///
/// # Returns
///
/// A `FileAnalysis` containing all extracted information.
pub(crate) fn analyze_js_file_ast(
    content: &str,
    path: &Path,
    root: &Path,
    extensions: Option<&HashSet<String>>,
    ts_resolver: Option<&TsPathResolver>,
    relative: String,
    command_cfg: &CommandDetectionConfig,
) -> FileAnalysis {
    let allocator = Allocator::default();

    // Determine source type from file extension
    // Only enable JSX for .tsx/.jsx files to avoid conflicts with TypeScript generics
    // (e.g., `const fn = <T>(...) =>` would be parsed as JSX tag with JSX enabled)
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let is_jsx_file = ext == "tsx" || ext == "jsx";
    let is_svelte_file = ext == "svelte";
    let is_svelte_rune_module = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.ends_with(".svelte.js") || name.ends_with(".svelte.ts"));
    let is_vue_file = ext == "vue";
    let is_astro_file = ext == "astro";
    let is_sfc_file = is_svelte_file || is_vue_file || is_astro_file;

    // Detect Flow type annotations
    let is_flow = is_flow_file(content);

    // For SFC files (Svelte/Vue), extract script content first
    let parsed_content: String;
    let content_to_parse = if is_svelte_file {
        parsed_content = extract_svelte_script(content);
        parsed_content.as_str()
    } else if is_vue_file {
        parsed_content = extract_vue_script(content);
        parsed_content.as_str()
    } else if is_astro_file {
        let _astro_scripts = extract_astro_scripts(content);
        let _astro_styles = extract_astro_styles(content);
        parsed_content = extract_astro_frontmatter(content);
        parsed_content.as_str()
    } else {
        content
    };

    // For SFC files (Svelte/Vue/Astro), parse extracted script/frontmatter as TypeScript.
    let source_type = if is_sfc_file {
        SourceType::tsx().with_typescript(true)
    } else {
        SourceType::from_path(path)
            .unwrap_or_default()
            .with_typescript(true)
            .with_jsx(is_jsx_file)
    };

    let ret = Parser::new(&allocator, content_to_parse, source_type).parse();

    // Log parser errors for debugging (verbose mode only)
    if !ret.errors.is_empty() && std::env::var("LOCTREE_VERBOSE").is_ok() {
        eprintln!(
            "[loctree][debug] Parser errors in {}: {} errors",
            path.display(),
            ret.errors.len()
        );
        for (i, err) in ret.errors.iter().take(5).enumerate() {
            // Get line number from error span using the labels field
            let line_info = err
                .labels
                .as_ref()
                .and_then(|labels| labels.first())
                .map(|label| {
                    let offset = label.offset();
                    let line = content[..offset].bytes().filter(|b| *b == b'\n').count() + 1;
                    format!(" (line {}, col {})", line, label.offset())
                })
                .unwrap_or_default();
            eprintln!("  [{}]{} {}", i + 1, line_info, err);
        }
    }

    let mut visitor = JsVisitor {
        analysis: FileAnalysis::new(relative),
        path,
        root,
        extensions,
        ts_resolver,
        source_text: content_to_parse,
        source_lines: content_to_parse.lines().collect(),
        newline_offsets: content_to_parse
            .bytes()
            .enumerate()
            .filter_map(|(i, b)| (b == b'\n').then_some(i))
            .collect(),
        command_cfg,
        namespace_imports: HashMap::new(),
    };

    visitor.visit_program(&ret.program);

    if is_svelte_file || is_svelte_rune_module {
        add_svelte_rune_exports(&mut visitor.analysis, content_to_parse);
    }

    // For Single File Components (.svelte/.astro/.vue), the file IS the
    // component by convention — `import HeroSection from './HeroSection.svelte'`
    // resolves the default export to the filename. Inject a synthetic
    // ExportSymbol{ name: file_stem, kind: "default" } so symbol-based
    // lookups (`find(name="HeroSection", mode="who-imports")`,
    // `query_where_symbol`) can bridge from component-name to file-path.
    // Rune modules (.svelte.ts/.svelte.js) are excluded — they are
    // TypeScript modules, not component files.
    if is_sfc_file {
        add_sfc_default_export(&mut visitor.analysis, path);
    }

    // Mark file as Flow if detected
    visitor.analysis.is_flow_file = is_flow;

    // Use oxc_semantic to track local symbol references
    // This helps detect when exported symbols are used internally (not dead)
    let semantic_ret = SemanticBuilder::new().build(&ret.program);
    if semantic_ret.errors.is_empty() {
        let semantic = semantic_ret.semantic;

        // Build set of exported symbol names for quick lookup
        let exported_names: HashSet<&str> = visitor
            .analysis
            .exports
            .iter()
            .map(|e| e.name.as_str())
            .collect();

        let mut local_symbols = Vec::new();
        let mut symbol_usages = Vec::new();
        let mut seen_defs: HashSet<(String, usize)> = HashSet::new();
        let mut seen_uses: HashSet<(String, usize)> = HashSet::new();

        const MAX_USAGES_PER_FILE: usize = 1500;

        // Check each symbol - record local defs/usages and exported local uses
        for symbol_id in semantic.scoping().symbol_ids() {
            let name = semantic.scoping().symbol_name(symbol_id);
            if name.is_empty() {
                continue;
            }

            let decl = semantic.symbol_declaration(symbol_id);
            let kind = match decl.kind() {
                AstKind::Function(_) => "function",
                AstKind::Class(_) => "class",
                AstKind::VariableDeclarator(_) => "variable",
                AstKind::TSTypeAliasDeclaration(_) => "type",
                AstKind::TSInterfaceDeclaration(_) => "interface",
                AstKind::TSEnumDeclaration(_) => "enum",
                AstKind::ImportSpecifier(_)
                | AstKind::ImportDefaultSpecifier(_)
                | AstKind::ImportNamespaceSpecifier(_) => "import",
                AstKind::FormalParameter(_) | AstKind::BindingIdentifier(_) => "binding",
                _ => "symbol",
            };

            let span = decl.kind().span();
            let line = visitor.get_line(span);
            let is_exported = exported_names.contains(name);
            let context = visitor.line_context(line);

            // Track local definitions (non-exported; skip imports here)
            if !is_exported && kind != "import" && seen_defs.insert((name.to_string(), line)) {
                local_symbols.push(LocalSymbol {
                    name: name.to_string(),
                    kind: kind.to_string(),
                    line: Some(line),
                    context,
                    is_exported,
                });
            }

            // Exported symbol used locally?
            let ref_ids = semantic.scoping().get_resolved_reference_ids(symbol_id);
            if is_exported && !ref_ids.is_empty() {
                visitor.analysis.local_uses.push(name.to_string());
            }

            // Record usage sites (identifier references)
            if symbol_usages.len() < MAX_USAGES_PER_FILE {
                for reference in semantic.symbol_references(symbol_id) {
                    if symbol_usages.len() >= MAX_USAGES_PER_FILE {
                        break;
                    }
                    let ref_span = semantic.reference_span(reference);
                    let ref_line = visitor.get_line(ref_span);
                    if ref_line == 0 {
                        continue;
                    }
                    if seen_uses.insert((name.to_string(), ref_line)) {
                        let ref_context = visitor.line_context(ref_line);
                        symbol_usages.push(SymbolUsage {
                            name: name.to_string(),
                            line: ref_line,
                            context: ref_context,
                        });
                    }
                }
            }
        }

        if !local_symbols.is_empty() {
            visitor.analysis.local_symbols = local_symbols;
        }
        if !symbol_usages.is_empty() {
            visitor.analysis.symbol_usages = symbol_usages;
        }
    }

    // For Svelte files, also parse the template section to detect function calls
    // This prevents false positives where exported functions are used in the template
    // e.g., {badgeText(account)} or on:click={handleClick}
    if is_svelte_file {
        let template = extract_svelte_template(content);
        add_svelte_snippet_exports(&mut visitor.analysis, &template);
        let template_usages = parse_svelte_template_usages(&template);
        for usage in template_usages {
            if !visitor.analysis.local_uses.contains(&usage) {
                visitor.analysis.local_uses.push(usage);
            }
        }
    }

    // For Vue files, also parse the template section to detect function calls
    // This prevents false positives where exported functions are used in the template
    // e.g., {{ formatDate(value) }} or @click="handleClick"
    if is_vue_file {
        let template = extract_vue_template(content);
        let template_usages = parse_vue_template_usages(&template);
        for usage in template_usages {
            if !visitor.analysis.local_uses.contains(&usage) {
                visitor.analysis.local_uses.push(usage);
            }
        }
    }

    visitor.analysis
}

fn add_svelte_rune_exports(analysis: &mut FileAnalysis, script: &str) {
    for rune in extract_svelte5_runes(script) {
        push_export_if_missing(
            analysis,
            rune.name,
            rune.kind.export_kind(),
            "svelte5_rune",
            Some(rune.line),
            Vec::new(),
        );
    }
}

/// Push a synthetic default-export for SFC files where the component name
/// equals the file stem by convention. Skipped when the file already has
/// an export with that stem name (avoids duplicating an explicit Vue
/// `export default class HeroSection` plus the synthetic one, while still
/// allowing the canonical `name: "default"` entry from a classic Vue
/// `<script>` block to coexist).
fn add_sfc_default_export(analysis: &mut FileAnalysis, path: &Path) {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return;
    };
    if stem.is_empty() {
        return;
    }
    // file_stem on "Counter.svelte.ts" returns "Counter.svelte" — the SFC
    // gate already excludes rune modules, but belt-and-braces: skip stems
    // containing additional dots so we never push "Counter.svelte" as the
    // synthetic component name.
    if stem.contains('.') {
        return;
    }
    push_export_if_missing(
        analysis,
        stem.to_string(),
        "default",
        "sfc_component",
        Some(1),
        Vec::new(),
    );
}

fn add_svelte_snippet_exports(analysis: &mut FileAnalysis, template: &str) {
    for SnippetDeclaration { name, line, params } in extract_svelte_snippets(template) {
        let params = params
            .into_iter()
            .map(|name| ParamInfo {
                name,
                type_annotation: None,
                has_default: false,
            })
            .collect();
        push_export_if_missing(
            analysis,
            name,
            "snippet",
            "svelte_template",
            Some(line),
            params,
        );
    }
}

fn push_export_if_missing(
    analysis: &mut FileAnalysis,
    name: String,
    kind: &str,
    export_type: &str,
    line: Option<usize>,
    params: Vec<ParamInfo>,
) {
    if analysis
        .exports
        .iter()
        .any(|existing| existing.name == name && existing.kind == kind)
    {
        return;
    }

    analysis.exports.push(ExportSymbol::with_params(
        name,
        kind,
        export_type,
        line,
        params,
    ));
}

/// Custom Visit implementation for JsVisitor that delegates to submodule handlers.
///
/// This implementation wires together all the visitor methods from the submodules
/// (imports, exports, calls) while also handling expression visiting for string
/// literal and WeakMap/WeakSet detection.
impl<'a> Visit<'a> for JsVisitor<'a> {
    fn visit_expression(&mut self, expr: &Expression<'a>) {
        match expr {
            Expression::StringLiteral(lit) => {
                self.push_string_literal(&lit.value, lit.span);
            }
            Expression::TemplateLiteral(tpl) => {
                if tpl.expressions.is_empty()
                    && tpl.quasis.len() == 1
                    && let Some(cooked) = &tpl.quasis[0].value.cooked
                {
                    self.push_string_literal(cooked, tpl.span);
                } else if tpl.expressions.is_empty() && tpl.quasis.len() == 1 {
                    self.push_string_literal(&tpl.quasis[0].value.raw, tpl.span);
                }
            }
            Expression::NewExpression(new_expr) => {
                // Detect WeakMap/WeakSet constructor calls to identify global registry patterns
                // Common in React DevTools and other libraries for storing metadata
                if let Expression::Identifier(ident) = &new_expr.callee {
                    let name = ident.name.to_string();
                    if name == "WeakMap" || name == "WeakSet" {
                        self.analysis.has_weak_collections = true;
                    }
                }
            }
            _ => {}
        }
        walk_expression(self, expr);
    }

    // Import handling - delegates to imports.rs
    fn visit_import_declaration(&mut self, decl: &ImportDeclaration<'a>) {
        self.handle_import_declaration(decl);
    }

    fn visit_member_expression(&mut self, member: &MemberExpression<'a>) {
        self.handle_member_expression(member);
        oxc_ast_visit::walk::walk_member_expression(self, member);
    }

    // Export handling - delegates to exports.rs
    fn visit_export_named_declaration(&mut self, decl: &ExportNamedDeclaration<'a>) {
        self.handle_export_named_declaration(decl);
    }

    fn visit_export_default_declaration(&mut self, decl: &ExportDefaultDeclaration<'a>) {
        self.handle_export_default_declaration(decl);
    }

    fn visit_export_all_declaration(&mut self, decl: &ExportAllDeclaration<'a>) {
        self.handle_export_all_declaration(decl);
    }

    // Call expression handling - delegates to calls.rs
    fn visit_import_expression(&mut self, expr: &ImportExpression<'a>) {
        self.handle_import_expression(expr);

        // Continue visiting children
        self.visit_expression(&expr.source);
        if let Some(opts) = &expr.options {
            self.visit_expression(opts);
        }
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        // Continue visiting children (callee/args may contain nested invocations)
        self.visit_arguments(&call.arguments);
        self.visit_expression(&call.callee);

        self.handle_call_expression(call);
    }

    fn visit_variable_declarator(&mut self, decl: &VariableDeclarator<'a>) {
        self.handle_variable_declarator(decl);

        // IMPORTANT: Continue visiting children (e.g. init expression might contain dynamic imports)
        self.visit_binding_pattern(&decl.id);
        if let Some(init) = &decl.init {
            self.visit_expression(init);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_ast_parsing_basic() {
        let content = r#"
            import { Foo } from "./bar";
            import Default, { Named } from "./baz";
            import * as NS from "./ns";

            export const myVar = 1;
            export function myFunc() {}
            export default class MyClass {}
            export { reexported } from "./other";

            invoke("my_command");
            safeInvoke("another_command");
        "#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/test.ts"),
            Path::new("src"),
            None,
            None,
            "test.ts".to_string(),
            &CommandDetectionConfig::default(),
        );

        // Imports
        assert_eq!(analysis.imports.len(), 3);

        let bar = analysis
            .imports
            .iter()
            .find(|i| i.source == "./bar")
            .unwrap();
        assert_eq!(bar.symbols[0].name, "Foo");
        assert!(!bar.symbols[0].is_default);

        let baz = analysis
            .imports
            .iter()
            .find(|i| i.source == "./baz")
            .unwrap();
        assert_eq!(baz.symbols.len(), 2);
        assert!(
            baz.symbols
                .iter()
                .any(|s| s.name == "Default" && s.is_default)
        );
        assert!(
            baz.symbols
                .iter()
                .any(|s| s.name == "Named" && !s.is_default)
        );

        let ns = analysis
            .imports
            .iter()
            .find(|i| i.source == "./ns")
            .unwrap();
        assert_eq!(ns.symbols[0].name, "*");
        assert_eq!(ns.symbols[0].alias.as_deref(), Some("NS"));

        // Exports
        let exports: Vec<_> = analysis.exports.iter().map(|e| e.name.as_str()).collect();
        assert!(exports.contains(&"myVar"));
        assert!(exports.contains(&"myFunc"));
        // Default exports are now named "default" for proper import matching
        assert!(exports.contains(&"default"));
        // The original class name is preserved in export_type
        assert!(
            analysis
                .exports
                .iter()
                .any(|e| e.name == "default" && e.export_type == "MyClass")
        );
        assert!(exports.contains(&"reexported"));

        // Commands
        let commands: Vec<_> = analysis
            .command_calls
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(commands.contains(&"my_command"));
        assert!(commands.contains(&"another_command"));
    }

    #[test]
    fn test_register_command_not_tauri_invoke() {
        let content = r#"
            import * as vscode from "vscode";
            vscode.commands.registerCommand("loctree.analyzeImpact", () => {});
        "#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("editors/vscode/src/commands.ts"),
            Path::new("editors/vscode/src"),
            None,
            None,
            "commands.ts".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(
            analysis.command_calls.is_empty(),
            "VSCode registerCommand should not be treated as a Tauri invoke"
        );
    }

    #[test]
    fn test_vue_sfc_script_extraction() {
        // Vue SFC with script setup (Composition API)
        let content = r#"
<script setup lang="ts">
import { ref, computed } from 'vue'

const count = ref(0)

export function increment() {
    count.value++
}
</script>

<template>
  <div>{{ count }}</div>
</template>
        "#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/Counter.vue"),
            Path::new("src"),
            None,
            None,
            "Counter.vue".to_string(),
            &CommandDetectionConfig::default(),
        );

        // Verify imports are detected
        assert!(
            analysis.imports.iter().any(|i| i.source == "vue"),
            "Should detect vue import"
        );

        // Verify exports are detected
        assert!(
            analysis
                .exports
                .iter()
                .any(|e| e.name == "increment" && e.kind == "function"),
            "Should detect increment export"
        );
    }

    #[test]
    fn test_vue_sfc_options_api() {
        // Vue SFC with Options API
        let content = r#"
<script lang="ts">
import { defineComponent } from 'vue'

export default defineComponent({
    data() {
        return { count: 0 }
    }
})
</script>

<template>
  <div>{{ count }}</div>
</template>
        "#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/Counter.vue"),
            Path::new("src"),
            None,
            None,
            "Counter.vue".to_string(),
            &CommandDetectionConfig::default(),
        );

        // Verify import is detected
        assert!(
            analysis.imports.iter().any(|i| i.source == "vue"),
            "Should detect vue import"
        );

        // Verify default export is detected
        assert!(
            analysis.exports.iter().any(|e| e.export_type == "default"),
            "Should detect default export"
        );
    }

    #[test]
    fn test_svelte_file_full_analysis() {
        let content = r#"
<script lang="ts">
    import type { Account } from './types';

    export function badgeText(account: Account): string {
        return account.name;
    }

    export let account: Account;
</script>

<div class="badge">
    <span>{badgeText(account)}</span>
</div>

<style>
    .badge { color: blue; }
</style>
        "#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/GitHubAccountBadge.svelte"),
            Path::new("src"),
            None,
            None,
            "GitHubAccountBadge.svelte".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(
            analysis.local_uses.contains(&"badgeText".to_string()),
            "badgeText should be in local_uses, found: {:?}",
            analysis.local_uses
        );

        assert!(
            analysis.local_uses.contains(&"account".to_string()),
            "account should be in local_uses, found: {:?}",
            analysis.local_uses
        );
    }

    #[test]
    fn test_astro_frontmatter_analysis() {
        let content = r#"---
import Card from "../components/Card.astro";
export interface Props { title: string; }
const { title } = Astro.props;
---
<Card title={title} />
"#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/pages/index.astro"),
            Path::new("src"),
            Some(&HashSet::from(["astro".to_string(), "ts".to_string()])),
            None,
            "pages/index.astro".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(
            analysis
                .imports
                .iter()
                .any(|i| i.source == "../components/Card.astro")
        );
        assert!(
            analysis
                .exports
                .iter()
                .any(|e| e.name == "Props" && e.kind == "interface")
        );
    }

    #[test]
    fn test_svelte5_runes_surface_as_exports() {
        let content = r#"
<script lang="ts">
let { title, count = 0 } = $props();
let clicks = $state(count);
const doubled = $derived(clicks * 2);
let label = $bindable(title);
</script>

<button>{label}: {doubled}</button>
"#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/Counter.svelte"),
            Path::new("src"),
            None,
            None,
            "Counter.svelte".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(
            analysis
                .exports
                .iter()
                .any(|e| e.name == "clicks" && e.kind == "rune_state")
        );
        assert!(
            analysis
                .exports
                .iter()
                .any(|e| e.name == "doubled" && e.kind == "rune_derived")
        );
        assert!(
            analysis
                .exports
                .iter()
                .any(|e| e.name == "title" && e.kind == "rune_props")
        );
        assert!(
            analysis
                .exports
                .iter()
                .any(|e| e.name == "count" && e.kind == "rune_props")
        );
        assert!(
            analysis
                .exports
                .iter()
                .any(|e| e.name == "label" && e.kind == "rune_bindable")
        );
    }

    #[test]
    fn test_svelte5_runes_in_svelte_ts_module_surface_as_exports() {
        let content = r#"
export const storeName = "counter";
let count = $state(0);
export function increment() {
    count += 1;
}
"#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/lib/store.svelte.ts"),
            Path::new("src"),
            None,
            None,
            "lib/store.svelte.ts".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(analysis.exports.iter().any(|e| e.name == "storeName"));
        assert!(analysis.exports.iter().any(|e| e.name == "increment"));
        assert!(
            analysis
                .exports
                .iter()
                .any(|e| e.name == "count" && e.kind == "rune_state")
        );
    }

    #[test]
    fn test_svelte5_script_module_and_snippet_exports() {
        let content = r#"
<script module lang="ts">
export const prerender = true;
</script>

<script lang="ts">
let count = $state(0);
</script>

{#snippet item(label)}
  <span>{label}</span>
{/snippet}
"#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/routes/+page.svelte"),
            Path::new("src"),
            None,
            None,
            "routes/+page.svelte".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(analysis.exports.iter().any(|e| e.name == "prerender"));
        assert!(
            analysis
                .exports
                .iter()
                .any(|e| e.name == "count" && e.kind == "rune_state")
        );
        assert!(analysis.exports.iter().any(|e| {
            e.name == "item"
                && e.kind == "snippet"
                && e.export_type == "svelte_template"
                && e.params.iter().any(|param| param.name == "label")
        }));
    }

    #[test]
    fn test_vue_file_full_analysis() {
        let content = r#"
<script setup lang="ts">
    import type { Product } from './types';

    export function formatPrice(price: number): string {
        return `$${price.toFixed(2)}`;
    }

    export const product: Product = { name: 'Widget', price: 29.99 };
</script>

<template>
    <div class="product">
        <h3>{{ product.name }}</h3>
        <p>{{ formatPrice(product.price) }}</p>
    </div>
</template>

<style scoped>
    .product { border: 1px solid #ccc; }
</style>
        "#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/ProductCard.vue"),
            Path::new("src"),
            None,
            None,
            "ProductCard.vue".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(
            analysis.local_uses.contains(&"formatPrice".to_string()),
            "formatPrice should be in local_uses, found: {:?}",
            analysis.local_uses
        );

        assert!(
            analysis.local_uses.contains(&"product".to_string()),
            "product should be in local_uses, found: {:?}",
            analysis.local_uses
        );
    }

    /// Test WeakMap/WeakSet detection for registry pattern (React DevTools, etc.)
    #[test]
    fn test_weakmap_detection() {
        let content = r#"
            // React DevTools pattern: store component metadata in WeakMap
            const componentMap = new WeakMap();
            const stateMap = new WeakSet();

            export function registerComponent(component) {
                componentMap.set(component, { name: component.name });
            }

            export const MyComponent = () => <div>Hello</div>;
        "#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/devtools.tsx"), // Use .tsx for JSX support
            Path::new("src"),
            None,
            None,
            "devtools.tsx".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(
            analysis.has_weak_collections,
            "Should detect WeakMap/WeakSet usage"
        );

        // Should export 2 symbols
        assert_eq!(analysis.exports.len(), 2);
        assert!(
            analysis
                .exports
                .iter()
                .any(|e| e.name == "registerComponent")
        );
        assert!(analysis.exports.iter().any(|e| e.name == "MyComponent"));
    }

    /// Test that files without WeakMap/WeakSet don't get flagged
    #[test]
    fn test_no_weakmap_detection() {
        let content = r#"
            const cache = new Map();
            export function getCached(key) {
                return cache.get(key);
            }
        "#;

        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/cache.ts"),
            Path::new("src"),
            None,
            None,
            "cache.ts".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(
            !analysis.has_weak_collections,
            "Should NOT flag regular Map as WeakMap"
        );
    }

    #[test]
    fn test_local_symbols_and_usages() {
        let content = r#"
            import { Component as MyComponent } from 'react';

            const taskFilter = 'all';
            function applyFilter() {
                return taskFilter;
            }
            const onClick = () => MyComponent;
            MyComponent();
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

        assert!(
            analysis
                .local_symbols
                .iter()
                .any(|s| s.name == "taskFilter"),
            "taskFilter should be in local_symbols"
        );
        assert!(
            analysis
                .local_symbols
                .iter()
                .any(|s| s.name == "applyFilter"),
            "applyFilter should be in local_symbols"
        );
        assert!(
            analysis
                .symbol_usages
                .iter()
                .any(|u| u.name == "taskFilter"),
            "taskFilter should be in symbol_usages"
        );
        assert!(
            analysis
                .symbol_usages
                .iter()
                .any(|u| u.name == "MyComponent"),
            "MyComponent usage should be tracked"
        );
    }

    #[test]
    fn test_sfc_default_export_synthesized_for_svelte() {
        // Svelte component without explicit `export default` — file IS the component.
        // Synthetic default export should land with name = file_stem so
        // `find(name="HeroSectionV2", mode="who-imports")` can resolve it.
        let content = r#"
<script lang="ts">
    let count = $state(0);
</script>

<button on:click={() => count++}>{count}</button>
"#;
        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/components/HeroSectionV2.svelte"),
            Path::new("src"),
            None,
            None,
            "components/HeroSectionV2.svelte".to_string(),
            &CommandDetectionConfig::default(),
        );

        let default_match = analysis
            .exports
            .iter()
            .find(|e| e.name == "HeroSectionV2" && e.kind == "default");
        assert!(
            default_match.is_some(),
            "Svelte file should synthesize a default export named after its stem; got exports = {:?}",
            analysis
                .exports
                .iter()
                .map(|e| (&e.name, &e.kind))
                .collect::<Vec<_>>()
        );
        assert_eq!(default_match.unwrap().export_type, "sfc_component");
    }

    #[test]
    fn test_sfc_default_export_synthesized_for_astro() {
        let content = r#"---
import Card from "../components/Card.astro";
const title = "Hello";
---
<Card title={title} />
"#;
        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/pages/HomePage.astro"),
            Path::new("src"),
            None,
            None,
            "pages/HomePage.astro".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(
            analysis.exports.iter().any(|e| e.name == "HomePage"
                && e.kind == "default"
                && e.export_type == "sfc_component"),
            "Astro file should synthesize a default export named after its stem; got exports = {:?}",
            analysis
                .exports
                .iter()
                .map(|e| (&e.name, &e.kind))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_sfc_default_export_synthesized_for_vue() {
        // Vue SFC without classic `export default` (e.g. `<script setup>`)
        // — synthetic default export should still let symbol lookups bridge
        // the gap between component name and file path.
        let content = r#"
<script setup lang="ts">
const count = ref(0);
</script>

<template>
  <div>{{ count }}</div>
</template>
"#;
        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/widgets/Counter.vue"),
            Path::new("src"),
            None,
            None,
            "widgets/Counter.vue".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(
            analysis.exports.iter().any(|e| e.name == "Counter"
                && e.kind == "default"
                && e.export_type == "sfc_component"),
            "Vue file should synthesize a default export named after its stem; got exports = {:?}",
            analysis
                .exports
                .iter()
                .map(|e| (&e.name, &e.kind))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_sfc_synthetic_export_does_not_clobber_explicit() {
        // Vue file already has `export default class HeroSection {}` from
        // the AST visitor → ExportSymbol { name: "default", kind: "default" }.
        // The synthetic helper pushes a DIFFERENT name (file stem), so both
        // entries coexist. `name: "default"` is the JS-side default-import
        // anchor; the file-stem entry is the symbol-search anchor.
        let content = r#"
<script lang="ts">
export default class HeroSection {}
</script>
"#;
        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/HeroSection.vue"),
            Path::new("src"),
            None,
            None,
            "HeroSection.vue".to_string(),
            &CommandDetectionConfig::default(),
        );

        let default_anchor = analysis
            .exports
            .iter()
            .filter(|e| e.name == "default" && e.kind == "default")
            .count();
        let stem_anchor = analysis
            .exports
            .iter()
            .filter(|e| e.name == "HeroSection" && e.kind == "default")
            .count();
        assert_eq!(
            default_anchor, 1,
            "Original `name: \"default\"` export should remain untouched"
        );
        assert_eq!(
            stem_anchor, 1,
            "Synthetic stem-named export should be pushed exactly once"
        );
    }

    #[test]
    fn test_sfc_synthetic_export_not_emitted_for_rune_module() {
        // `.svelte.ts` rune modules are TypeScript files using the svelte
        // co-located convention — they are NOT component files. No synthetic
        // default export should be added (the file stem would be
        // "Counter.svelte" which would mis-resolve as a component name).
        let content = r#"
let count = $state(0);
export function getCount() { return count; }
"#;
        let analysis = analyze_js_file_ast(
            content,
            Path::new("src/Counter.svelte.ts"),
            Path::new("src"),
            None,
            None,
            "Counter.svelte.ts".to_string(),
            &CommandDetectionConfig::default(),
        );

        assert!(
            !analysis
                .exports
                .iter()
                .any(|e| e.export_type == "sfc_component"),
            "Rune modules (.svelte.ts) must not get a synthetic sfc_component export"
        );
    }
}
