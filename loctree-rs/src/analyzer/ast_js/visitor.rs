//! JsVisitor struct and helper methods for AST traversal.
//!
//! This module contains the core visitor structure and utility methods
//! for path resolution, line number calculation, string literal tracking,
//! and type signature recording.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::collections::{HashMap, HashSet};
use std::path::Path;

use oxc_ast::ast::*;
use oxc_span::Span;

use crate::types::{FileAnalysis, ParamInfo, SignatureUse, SignatureUseKind, StringLiteral};

use super::config::CommandDetectionConfig;
use crate::analyzer::resolvers::{TsPathResolver, resolve_reexport_target};

/// Visitor for traversing JS/TS AST and collecting analysis data.
pub(super) struct JsVisitor<'a> {
    pub analysis: FileAnalysis,
    pub path: &'a Path,
    pub root: &'a Path,
    pub extensions: Option<&'a HashSet<String>>,
    pub ts_resolver: Option<&'a TsPathResolver>,
    pub source_text: &'a str,
    pub source_lines: Vec<&'a str>,
    /// Ascending byte offsets of every `\n` in `source_text`, precomputed once at
    /// construction. Lets `get_line` binary-search (O(log n)) instead of counting
    /// newlines from the start of the file on every call — the previous O(n)
    /// per-call cost was O(symbols · filesize) overall and hung `loct slice` on
    /// large inputs.
    pub newline_offsets: Vec<usize>,
    pub command_cfg: &'a CommandDetectionConfig,
    /// Map of namespace import aliases to their resolved paths: alias -> (source, resolved_path)
    pub namespace_imports: HashMap<String, (String, Option<String>)>,
}

impl<'a> JsVisitor<'a> {
    /// Resolve import source path to an absolute path.
    pub(super) fn resolve_path(&self, source: &str) -> Option<String> {
        let file_ext = self
            .path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase());

        // For TS/JS files, skip resolve_reexport_target (uses Python logic)
        // Go straight to TS resolver or JS relative resolution
        let skip_python = matches!(
            file_ext.as_deref(),
            Some("ts") | Some("tsx") | Some("js") | Some("jsx") | Some("mjs") | Some("cjs")
        );

        let initial = if skip_python {
            None
        } else {
            resolve_reexport_target(self.path, self.root, source, self.extensions)
        };

        initial
            .or_else(|| {
                self.ts_resolver
                    .and_then(|r| r.resolve(source, self.extensions))
            })
            .or_else(|| {
                crate::analyzer::resolvers::resolve_js_relative(
                    self.path,
                    self.root,
                    source,
                    self.extensions,
                )
            })
    }

    /// Get line number from span position.
    ///
    /// Binary-search over the precomputed `newline_offsets`: the line is one more
    /// than the count of newline bytes strictly before `capped`. Equivalent to the
    /// old `source_text[..capped].bytes().filter(== '\n').count() + 1`, but O(log n)
    /// per call instead of O(n).
    pub(super) fn get_line(&self, span: Span) -> usize {
        let start = span.start as usize;
        let capped = std::cmp::min(start, self.source_text.len());
        self.newline_offsets.partition_point(|&p| p < capped) + 1
    }

    /// Get trimmed source line for a 1-based line number.
    pub(super) fn line_context(&self, line: usize) -> String {
        if line == 0 {
            return String::new();
        }
        self.source_lines
            .get(line.saturating_sub(1))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    /// Record a string literal in the analysis.
    pub(super) fn push_string_literal(&mut self, value: &str, span: Span) {
        let line = self.get_line(span);
        self.analysis.string_literals.push(StringLiteral {
            value: value.to_string(),
            line,
        });
    }

    /// Extract basic type representation from TSType.
    pub(super) fn type_to_string(ty: &TSType<'a>) -> String {
        match ty {
            TSType::TSTypeReference(r) => JsVisitor::type_name_to_string(&r.type_name),
            // When the type is a complex union/inline construct, return a neutral label
            // so we don't bloat command payloads with full type ASTs.
            _ => "Type".to_string(),
        }
    }

    fn type_name_to_string(name: &TSTypeName<'a>) -> String {
        match name {
            TSTypeName::IdentifierReference(id) => id.name.to_string(),
            TSTypeName::QualifiedName(q) => {
                format!(
                    "{}.{}",
                    JsVisitor::type_name_to_string(&q.left),
                    q.right.name
                )
            }
            TSTypeName::ThisExpression(_) => "This".to_string(),
        }
    }

    /// Record a type usage in function signature.
    pub(super) fn record_type_use(
        &mut self,
        fn_name: &str,
        usage: SignatureUseKind,
        ty: &TSType<'a>,
        span: Span,
    ) {
        let type_name = JsVisitor::type_to_string(ty);
        if type_name.is_empty() || type_name == "Type" {
            return;
        }
        let line = self.get_line(span);
        if !self.analysis.local_uses.contains(&type_name) {
            self.analysis.local_uses.push(type_name.clone());
        }
        self.analysis.signature_uses.push(SignatureUse {
            function: fn_name.to_string(),
            usage,
            type_name,
            line: Some(line),
        });
    }

    /// Record parameter types from function parameters.
    pub(super) fn record_param_types(&mut self, fn_name: &str, params: &FormalParameters<'a>) {
        for param in params.items.iter() {
            if let Some(ann) = &param.type_annotation {
                self.record_type_use(
                    fn_name,
                    SignatureUseKind::Parameter,
                    &ann.type_annotation,
                    ann.span,
                );
            }
        }
        // NOTE: OXC wraps rest params as `FormalParameterRest { rest: BindingRestElement { ... } }`.
        if let Some(rest_param) = &params.rest
            && let Some(ann) = &rest_param.type_annotation
        {
            self.record_type_use(
                fn_name,
                SignatureUseKind::Parameter,
                &ann.type_annotation,
                ann.span,
            );
        }
    }

    /// Record function signature types (return type and parameters).
    pub(super) fn record_function_signature(&mut self, fn_name: &str, func: &Function<'a>) {
        if let Some(ret) = &func.return_type {
            self.record_type_use(
                fn_name,
                SignatureUseKind::Return,
                &ret.type_annotation,
                ret.span,
            );
        }
        self.record_param_types(fn_name, &func.params);
    }

    /// Record arrow function signature types.
    pub(super) fn record_arrow_signature(
        &mut self,
        fn_name: &str,
        func: &ArrowFunctionExpression<'a>,
    ) {
        if let Some(ret) = &func.return_type {
            self.record_type_use(
                fn_name,
                SignatureUseKind::Return,
                &ret.type_annotation,
                ret.span,
            );
        }
        self.record_param_types(fn_name, &func.params);
    }

    /// Extract parameter info from FormalParameters for export tracking.
    pub(super) fn extract_params(&self, params: &FormalParameters<'a>) -> Vec<ParamInfo> {
        let mut result = Vec::new();
        for param in params.items.iter() {
            let name = match &param.pattern {
                BindingPattern::BindingIdentifier(id) => id.name.to_string(),
                BindingPattern::ObjectPattern(_) => "{...}".to_string(),
                BindingPattern::ArrayPattern(_) => "[...]".to_string(),
                BindingPattern::AssignmentPattern(ap) => match &ap.left {
                    BindingPattern::BindingIdentifier(id) => id.name.to_string(),
                    _ => "_".to_string(),
                },
            };
            let type_annotation = param
                .type_annotation
                .as_ref()
                .map(|ann| JsVisitor::type_to_string(&ann.type_annotation));
            let has_default = matches!(&param.pattern, BindingPattern::AssignmentPattern(_));
            result.push(ParamInfo {
                name,
                type_annotation,
                has_default,
            });
        }
        // Handle rest parameter
        if let Some(rest_param) = &params.rest {
            let name = match &rest_param.rest.argument {
                BindingPattern::BindingIdentifier(id) => format!("...{}", id.name),
                _ => "...rest".to_string(),
            };
            let type_annotation = rest_param
                .type_annotation
                .as_ref()
                .map(|ann| JsVisitor::type_to_string(&ann.type_annotation));
            result.push(ParamInfo {
                name,
                type_annotation,
                has_default: false,
            });
        }
        result
    }

    /// Extract params from Function.
    pub(super) fn extract_function_params(&self, func: &Function<'a>) -> Vec<ParamInfo> {
        self.extract_params(&func.params)
    }

    /// Extract params from ArrowFunctionExpression.
    pub(super) fn extract_arrow_params(
        &self,
        func: &ArrowFunctionExpression<'a>,
    ) -> Vec<ParamInfo> {
        self.extract_params(&func.params)
    }
}
