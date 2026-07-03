//! Python file analysis module.
//!
//! Provides comprehensive Python code analysis including:
//! - Import/export detection (static and dynamic)
//! - Type hint usage extraction
//! - Framework decorator detection (pytest, FastAPI, Flask, Django, etc.)
//! - Concurrency pattern detection for race conditions
//! - Dynamic code generation detection (exec/eval/compile)
//! - Package metadata (typed packages, namespace packages)
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

mod concurrency;
mod decorators;
mod dynamic;
mod exports;
mod helpers;
mod imports;
mod metadata;
mod stdlib;
mod usages;

// Re-export the public API
pub(crate) use stdlib::python_stdlib_set;

// Private imports from submodules
use concurrency::detect_py_race_indicators;
use decorators::{extract_decorator_type_usages, is_framework_decorator, parse_route_decorator};
use dynamic::{detect_dynamic_exec_templates, detect_sys_modules_injection};
use exports::{parse_all_list, read_all_from_resolved};
use helpers::{is_valid_python_identifier, parse_module_const_target};
use imports::resolve_python_import;
use metadata::{check_namespace_package, check_typed_package, is_python_test_file};
use usages::{
    extract_bare_class_references, extract_class_from_containers, extract_python_function_calls,
    extract_type_hint_usages,
};

// External imports
use super::regexes::{regex_py_dynamic_dunder, regex_py_dynamic_importlib};
use crate::types::{
    ExportSymbol, FileAnalysis, ImportEntry, ImportKind, ImportSymbol, LocalSymbol, LogLevel,
    LogMessage, ParamInfo, ReexportEntry, ReexportKind, SymbolUsage,
};
use regex::Regex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Parse Python function parameters from the text between parentheses.
///
/// Handles formats like:
/// - `x: int`
/// - `y: str = 'default'`
/// - `*args`
/// - `**kwargs`
/// - `self`
fn parse_python_params(params_text: &str) -> Vec<ParamInfo> {
    let mut params = Vec::new();
    let trimmed = params_text.trim();
    if trimmed.is_empty() {
        return params;
    }

    // Split by commas, but respect brackets for generic types like List[int]
    let mut current = String::new();
    let mut bracket_depth: usize = 0;
    let mut paren_depth: usize = 0;

    for ch in trimmed.chars() {
        match ch {
            '[' => {
                bracket_depth += 1;
                current.push(ch);
            }
            ']' => {
                bracket_depth = bracket_depth.saturating_sub(1);
                current.push(ch);
            }
            '(' => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if bracket_depth == 0 && paren_depth == 0 => {
                let param = parse_single_param(current.trim());
                if let Some(p) = param {
                    params.push(p);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    // Don't forget the last parameter
    if !current.trim().is_empty()
        && let Some(p) = parse_single_param(current.trim())
    {
        params.push(p);
    }

    params
}

/// Parse a single Python parameter like `x: int = 5` or `*args` or `**kwargs`.
fn parse_single_param(param: &str) -> Option<ParamInfo> {
    let param = param.trim();
    if param.is_empty() {
        return None;
    }

    // Handle *args and **kwargs
    let (name_part, is_variadic) = if let Some(rest) = param.strip_prefix("**") {
        (rest, true)
    } else if let Some(rest) = param.strip_prefix('*') {
        (rest, true)
    } else {
        (param, false)
    };

    // Check for default value
    let (before_default, has_default) = if let Some(pos) = name_part.find('=') {
        (&name_part[..pos], true)
    } else {
        (name_part, false)
    };

    // Check for type annotation
    let (name, type_annotation) = if let Some(pos) = before_default.find(':') {
        let n = before_default[..pos].trim();
        let t = before_default[pos + 1..].trim();
        (
            n,
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            },
        )
    } else {
        (before_default.trim(), None)
    };

    // Reconstruct name with prefix for variadic params
    let final_name = if is_variadic {
        if param.starts_with("**") {
            format!("**{}", name)
        } else {
            format!("*{}", name)
        }
    } else {
        name.to_string()
    };

    if final_name.is_empty() || final_name == "*" || final_name == "**" {
        return None;
    }

    Some(ParamInfo {
        name: final_name,
        type_annotation,
        has_default,
    })
}

fn collect_symbol_usages_from_lines(
    lines: &[&str],
    names: &HashSet<String>,
    max: usize,
) -> Vec<SymbolUsage> {
    let mut usages = Vec::new();
    let mut seen: HashSet<(String, usize)> = HashSet::new();

    for (idx, line) in lines.iter().enumerate() {
        if usages.len() >= max {
            break;
        }
        let mut start: Option<usize> = None;
        for (i, ch) in line.char_indices() {
            let is_ident = ch.is_ascii_alphanumeric() || ch == '_';
            if is_ident {
                if start.is_none() {
                    start = Some(i);
                }
                continue;
            }
            if let Some(begin) = start.take() {
                let token = &line[begin..i];
                let Some(first) = token.chars().next() else {
                    continue;
                };
                if !(first.is_ascii_alphabetic() || first == '_') {
                    continue;
                }
                if !names.contains(token) {
                    continue;
                }
                let line_num = idx + 1;
                if seen.insert((token.to_string(), line_num)) {
                    usages.push(SymbolUsage {
                        name: token.to_string(),
                        line: line_num,
                        context: line.trim().to_string(),
                    });
                    if usages.len() >= max {
                        break;
                    }
                }
            }
        }
        if usages.len() >= max {
            break;
        }
        if let Some(begin) = start.take() {
            let token = &line[begin..];
            let Some(first) = token.chars().next() else {
                continue;
            };
            if !(first.is_ascii_alphabetic() || first == '_') {
                continue;
            }
            if !names.contains(token) {
                continue;
            }
            let line_num = idx + 1;
            if seen.insert((token.to_string(), line_num)) {
                usages.push(SymbolUsage {
                    name: token.to_string(),
                    line: line_num,
                    context: line.trim().to_string(),
                });
            }
        }
    }

    usages
}

/// Bundled context for [`process_from_import`] to stay under the 7-argument
/// clippy limit while keeping the call sites readable.
struct FromImportContext<'a> {
    module: &'a str,
    names_clean: &'a str,
    path: &'a Path,
    root: &'a Path,
    py_roots: &'a [PathBuf],
    extensions: Option<&'a HashSet<String>>,
    stdlib: &'a HashSet<String>,
    is_type_checking: bool,
    is_lazy: bool,
    line_num: usize,
    is_package_init: bool,
}

/// Process a `from X import Y, Z` statement and update analysis.
///
/// This is extracted to handle both single-line and multiline imports uniformly.
/// Normalize a `from <module> import ...` module string while PRESERVING the
/// leading relative-import dots (`.`, `..`) that encode the package level and
/// that `resolve_python_relative` counts. A naive `trim_end_matches('.')`
/// reduced a pure-dots module (`.`, `..`) to "" and dropped the whole import
/// edge, so `from . import (CONST, ...)` recorded nothing and intra-package
/// symbols looked dead (loctree-feedback.md, 2026-06-16). Only a stray trailing dot
/// on the NON-dot remainder is trimmed (e.g. `pkg.` → `pkg`).
fn normalize_relative_import_module(raw: &str) -> &str {
    let raw = raw.trim();
    let leading_dots = raw.len() - raw.trim_start_matches('.').len();
    let rest = raw[leading_dots..].trim_end_matches('.');
    if rest.is_empty() {
        &raw[..leading_dots]
    } else {
        // `rest` is a prefix of `raw[leading_dots..]` (trim_end only drops the
        // tail), so this byte range is contiguous and char-boundary safe.
        &raw[..leading_dots + rest.len()]
    }
}

fn process_from_import(ctx: &FromImportContext<'_>, analysis: &mut FileAnalysis) {
    let module = normalize_relative_import_module(ctx.module);
    if module.is_empty() {
        return;
    }

    let mut entry = ImportEntry::new(module.to_string(), ImportKind::Static);
    entry.line = Some(ctx.line_num);
    let (resolved, resolution) = resolve_python_import(
        module,
        ctx.path,
        ctx.root,
        ctx.py_roots,
        ctx.extensions,
        ctx.stdlib,
    );
    entry.resolution = resolution;
    entry.resolved_path = resolved.clone();
    entry.is_type_checking = ctx.is_type_checking;
    entry.is_lazy = ctx.is_lazy;
    entry.source_raw = format!("from {} import {}", module, ctx.names_clean);

    if ctx.names_clean != "*" {
        for sym in ctx.names_clean.split(',') {
            let sym = sym.trim();
            if sym.is_empty() {
                continue;
            }
            let (name, alias) = if let Some((lhs, rhs)) = sym.split_once(" as ") {
                (lhs.trim(), Some(rhs.trim().to_string()))
            } else {
                (sym, None)
            };
            entry.symbols.push(ImportSymbol {
                name: name.to_string(),
                alias,
                is_default: false,
            });
        }
    }
    analysis.imports.push(entry);

    // Python package API re-export pattern:
    // __init__.py often re-exports names via `from .mod import Foo as Bar`.
    // Treat these as re-exports (not fresh definitions) to reduce duplicate/dead noise.
    if ctx.is_package_init && ctx.names_clean != "*" {
        let mut name_pairs: Vec<(String, String)> = Vec::new();
        for sym in ctx.names_clean.split(',') {
            let sym = sym.trim();
            if sym.is_empty() {
                continue;
            }
            let (original, exported) = if let Some((lhs, rhs)) = sym.split_once(" as ") {
                (lhs.trim(), rhs.trim())
            } else {
                (sym, sym)
            };
            if exported.is_empty() || exported.starts_with('_') {
                continue;
            }
            name_pairs.push((original.to_string(), exported.to_string()));
            analysis.exports.push(ExportSymbol::new(
                exported.to_string(),
                "reexport",
                "named",
                Some(ctx.line_num),
            ));
        }

        if !name_pairs.is_empty() {
            analysis.reexports.push(ReexportEntry {
                source: module.to_string(),
                kind: ReexportKind::Named(name_pairs),
                resolved: resolved.clone(),
            });
        }
    }

    if ctx.names_clean == "*" {
        let mut entry = ReexportEntry {
            source: module.to_string(),
            kind: ReexportKind::Star,
            resolved: resolved.clone(),
        };
        if let Some(names) = read_all_from_resolved(&resolved, ctx.root) {
            for name in &names {
                analysis
                    .exports
                    .push(ExportSymbol::new(name.clone(), "reexport", "named", None));
            }
            // Star imports have no aliases - original and exported are the same
            let name_pairs: Vec<(String, String)> =
                names.into_iter().map(|n| (n.clone(), n)).collect();
            entry.kind = ReexportKind::Named(name_pairs);
        }
        analysis.reexports.push(entry);
    }
}

fn py_logging_call_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?m)(?P<recv>logging|logger|[A-Za-z_][A-Za-z0-9_]*logger[A-Za-z0-9_]*)\.(?P<level>debug|info|warning|warn|error|critical|exception)\s*\("#,
        )
        .expect("valid python logging regex")
    })
}

fn py_logger_binding_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?m)^\s*(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*=\s*logging\.getLogger\s*\("#)
            .expect("valid python logger binding regex")
    })
}

fn py_function_context_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?m)^\s*(?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)\s*\("#)
            .expect("valid python function context regex")
    })
}

fn py_offset_to_line(content: &str, offset: usize) -> usize {
    content[..offset.min(content.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
        + 1
}

fn py_log_level(level: &str) -> LogLevel {
    match level {
        "debug" => LogLevel::Debug,
        "warning" | "warn" => LogLevel::Warn,
        "error" | "critical" | "exception" => LogLevel::Error,
        _ => LogLevel::Info,
    }
}

fn py_nearest_function_context(functions: &[(usize, String)], line: usize) -> Option<String> {
    functions
        .iter()
        .take_while(|(fn_line, _)| *fn_line <= line)
        .last()
        .map(|(_, name)| name.clone())
}

fn py_extract_first_string_literal(input: &str) -> Option<String> {
    let mut quote = None;
    let mut start = 0usize;
    let mut escaped = false;
    for (idx, ch) in input.char_indices() {
        if quote.is_none() {
            if ch == '"' || ch == '\'' {
                quote = Some(ch);
                start = idx + ch.len_utf8();
            }
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if Some(ch) == quote {
            return Some(input[start..idx].to_string());
        }
    }
    None
}

fn collect_python_log_messages(content: &str) -> Vec<LogMessage> {
    let logger_names: HashSet<String> = py_logger_binding_regex()
        .captures_iter(content)
        .filter_map(|caps| caps.name("name").map(|m| m.as_str().to_string()))
        .collect();
    let functions: Vec<(usize, String)> = py_function_context_regex()
        .captures_iter(content)
        .filter_map(|caps| {
            let name = caps.get(1)?;
            Some((
                py_offset_to_line(content, name.start()),
                name.as_str().to_string(),
            ))
        })
        .collect();
    let mut messages = Vec::new();
    for caps in py_logging_call_regex().captures_iter(content) {
        let Some(receiver) = caps.name("recv").map(|m| m.as_str()) else {
            continue;
        };
        if receiver != "logging" && !logger_names.contains(receiver) {
            continue;
        }
        let Some(level) = caps.name("level") else {
            continue;
        };
        let Some(full_match) = caps.get(0) else {
            continue;
        };
        let format_string =
            py_extract_first_string_literal(&content[full_match.end()..]).unwrap_or_default();
        if format_string.is_empty() {
            continue;
        }
        let line = py_offset_to_line(content, full_match.start());
        messages.push(LogMessage {
            level: py_log_level(level.as_str()),
            macro_or_fn: format!("{}.{}", receiver, level.as_str()),
            format_string,
            line,
            function_context: py_nearest_function_context(&functions, line),
        });
    }
    messages
}

/// Main entry point for Python file analysis.
///
/// Analyzes a Python file and extracts:
/// - Import statements (static and dynamic)
/// - Export symbols (functions, classes, __all__ list)
/// - Re-exports from __init__.py files
/// - Type hint usages
/// - Framework decorators
/// - Concurrency patterns
/// - Entry points
pub(crate) fn analyze_py_file(
    content: &str,
    path: &Path,
    root: &Path,
    extensions: Option<&HashSet<String>>,
    relative: String,
    py_roots: &[PathBuf],
    stdlib: &HashSet<String>,
) -> FileAnalysis {
    let mut analysis = FileAnalysis::new(relative);
    let mut local_symbols: Vec<LocalSymbol> = Vec::new();
    let mut type_check_stack: Vec<usize> = Vec::new();
    let mut pending_callback_decorator = false;
    let mut pending_framework_decorator = false;
    let mut pending_fixture_decorator = false;
    let mut pending_routes: Vec<crate::types::RouteInfo> = Vec::new();
    let mut pending_fixture_name: Option<String> = None;
    let mut in_docstring = false;
    // State for multiline imports: (module, accumulated_symbols, start_line, is_type_checking, is_lazy)
    let mut pending_multiline_from: Option<(String, Vec<String>, usize, bool, bool)> = None;
    let is_package_init = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "__init__.py");

    // Set Python-specific metadata
    analysis.is_test = is_python_test_file(path, content);
    analysis.is_typed_package = check_typed_package(path, root);
    analysis.is_namespace_package = check_namespace_package(path, root);

    for (idx, line) in content.lines().enumerate() {
        let line_num = idx + 1;
        let trimmed_leading = line.trim_start();

        if in_docstring {
            // End docstring on closing triple quotes
            if trimmed_leading.contains("\"\"\"") || trimmed_leading.contains("'''") {
                in_docstring = false;
            }
            continue;
        }

        // Skip docstring/comment blocks at the start of a line
        if trimmed_leading.starts_with("\"\"\"") || trimmed_leading.starts_with("'''") {
            // If closing appears on the same line, exit docstring immediately
            let mut occurrences = 0;
            for token in ["\"\"\"", "'''"] {
                occurrences += trimmed_leading.matches(token).count();
            }
            if occurrences < 2 {
                in_docstring = true;
            }
            continue;
        }

        let without_comment = line.split('#').next().unwrap_or("").trim_end();
        let indent = without_comment
            .chars()
            .take_while(|c| c.is_whitespace())
            .count();
        if !without_comment.trim().is_empty() {
            while let Some(level) = type_check_stack.last() {
                if indent < *level {
                    type_check_stack.pop();
                } else {
                    break;
                }
            }
        }

        let trimmed = without_comment.trim_start();
        if let Some(body) = trimmed
            .strip_prefix("if ")
            .and_then(|rest| rest.strip_suffix(':'))
        {
            if body.contains("TYPE_CHECKING") {
                type_check_stack.push(indent + 1);
            }
            continue;
        }

        let in_type_checking = !type_check_stack.is_empty();
        if trimmed.starts_with('@') {
            // Track decorators that register callbacks (e.g., @rumps.clicked)
            if trimmed.contains("clicked") || trimmed.contains("rumps.") {
                pending_callback_decorator = true;
            }
            // Track framework decorators that mark functions as "used"
            if is_framework_decorator(trimmed) {
                pending_framework_decorator = true;
            }
            if let Some(route) = parse_route_decorator(trimmed, line_num) {
                pending_routes.push(route);
            }
            // pytest fixtures: treat next def as used
            if trimmed.contains("pytest.fixture") {
                pending_fixture_decorator = true;
                pending_fixture_name = None;
            }
            // Extract type usages from decorator parameters (response_model=X, Depends(X))
            extract_decorator_type_usages(trimmed, &mut analysis.local_uses);
            continue;
        }

        // Handle multiline import continuation
        if let Some((ref module, ref mut symbols, start_line, is_tc, is_lz)) =
            pending_multiline_from
        {
            // Accumulate symbols from continuation lines
            let line_content = trimmed.trim_end_matches(')').trim_end_matches(',');
            let line_content = line_content.split('#').next().unwrap_or("").trim();

            // Parse symbols from this line
            for sym in line_content.split(',') {
                let sym = sym.trim();
                if sym.is_empty() {
                    continue;
                }
                symbols.push(sym.to_string());
            }

            // Check if this line closes the import (contains closing paren)
            if trimmed.contains(')') {
                // Process the complete multiline import
                let module_clone = module.clone();
                let symbols_clone = symbols.clone();
                let start_line_val = start_line;
                let is_type_checking = is_tc;
                let is_lazy = is_lz;

                // Clear pending state before processing
                pending_multiline_from = None;

                // Process the import
                let joined = symbols_clone.join(", ");
                process_from_import(
                    &FromImportContext {
                        module: &module_clone,
                        names_clean: &joined,
                        path,
                        root,
                        py_roots,
                        extensions,
                        stdlib,
                        is_type_checking,
                        is_lazy,
                        line_num: start_line_val,
                        is_package_init,
                    },
                    &mut analysis,
                );
            }
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("import ") {
            for part in rest.split(',') {
                let mut name = part.trim();
                if let Some((lhs, _)) = name.split_once(" as ") {
                    name = lhs.trim();
                }
                if !name.is_empty() {
                    let mut entry = ImportEntry::new(name.to_string(), ImportKind::Static);
                    entry.line = Some(line_num);
                    let (resolved, resolution) =
                        resolve_python_import(name, path, root, py_roots, extensions, stdlib);
                    entry.resolution = resolution;
                    entry.resolved_path = resolved;
                    entry.is_type_checking = in_type_checking;
                    entry.is_lazy = indent > 0;
                    analysis.imports.push(entry);
                }
            }
        } else if let Some(rest) = trimmed.strip_prefix("from ")
            && let Some((module, names_raw)) = rest.split_once(" import ")
        {
            let module = normalize_relative_import_module(module);
            let names_raw_trimmed = names_raw.trim();

            // Check if this is a multiline import: starts with ( but doesn't end with )
            let is_multiline_start =
                names_raw_trimmed.starts_with('(') && !names_raw_trimmed.contains(')');

            if is_multiline_start {
                // Start accumulating multiline import
                // Extract any symbols on the first line after the opening paren
                let first_line_symbols = names_raw_trimmed
                    .trim_start_matches('(')
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim();
                let mut initial_symbols: Vec<String> = Vec::new();
                for sym in first_line_symbols.split(',') {
                    let sym = sym.trim();
                    if !sym.is_empty() {
                        initial_symbols.push(sym.to_string());
                    }
                }
                pending_multiline_from = Some((
                    module.to_string(),
                    initial_symbols,
                    line_num,
                    in_type_checking,
                    indent > 0,
                ));
            } else {
                // Single-line import - process immediately
                let names_clean = names_raw_trimmed.trim_matches('(').trim_matches(')');
                let names_clean = names_clean.split('#').next().unwrap_or("").trim();
                process_from_import(
                    &FromImportContext {
                        module,
                        names_clean,
                        path,
                        root,
                        py_roots,
                        extensions,
                        stdlib,
                        is_type_checking: in_type_checking,
                        is_lazy: indent > 0,
                        line_num,
                        is_package_init,
                    },
                    &mut analysis,
                );
            }
        } else {
            // Detect callback assignment patterns (callback=self.refresh or callback=refresh)
            if let Some(pos) = trimmed.find("callback")
                && let Some(eq_pos) = trimmed[pos..].find('=')
            {
                let after_eq = trimmed[pos + eq_pos + 1..].trim();
                let target = after_eq
                    .trim_start_matches("self.")
                    .trim_start_matches("cls.")
                    .trim_start_matches('&')
                    .trim_start_matches('*');
                let ident = target
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next()
                    .unwrap_or("")
                    .trim();
                if !ident.is_empty() {
                    analysis.local_uses.push(ident.to_string());
                }
            }

            // Track class bases and top-level exports
            if let Some(rest) = trimmed.strip_prefix("class ") {
                let (name_part, _) = rest.split_once(':').unwrap_or((rest, ""));
                let (name, bases_part) = if let Some((n, bases)) = name_part.split_once('(') {
                    (n.trim(), Some(bases.trim_end_matches(')').trim()))
                } else {
                    (name_part.trim(), None)
                };

                // Reject anything that isn't a valid Python identifier. Catches
                // JS `class Foo {` embedded in Python f-strings (escape `{{`),
                // template strings, and other non-Python text that the
                // line-based scanner cannot otherwise distinguish.
                let valid_name = is_valid_python_identifier(name);

                if valid_name {
                    local_symbols.push(LocalSymbol {
                        name: name.to_string(),
                        kind: "class".to_string(),
                        line: Some(line_num),
                        context: line.trim().to_string(),
                        is_exported: false,
                    });
                }

                if valid_name && indent == 0 && !name.starts_with('_') {
                    analysis.exports.push(ExportSymbol::new(
                        name.to_string(),
                        "class",
                        "named",
                        Some(line_num),
                    ));
                }

                if let Some(bases) = bases_part {
                    for base in bases.split(',') {
                        let base = base
                            .trim_start_matches("self.")
                            .trim_start_matches("cls.")
                            .trim();
                        if !base.is_empty() {
                            // Extract the last component for dotted names (e.g., wagtail.models.Page -> Page)
                            // But also keep the full dotted name in case it's a relative import
                            let simple_name = base.rsplit('.').next().unwrap_or(base);
                            if simple_name != base {
                                // If it's a dotted name, add both the full name and the simple name
                                analysis.local_uses.push(base.to_string());
                            }
                            if !simple_name.is_empty() {
                                analysis.local_uses.push(simple_name.to_string());
                            }
                        }
                    }
                }
            } else if let Some(rest) = trimmed
                .strip_prefix("async def ")
                .or_else(|| trimmed.strip_prefix("def "))
            {
                // Handle both "def foo" and "async def foo"
                // Extract function name and parameters
                let (name, params_text) = if let Some(paren_pos) = rest.find('(') {
                    let fn_name = rest[..paren_pos].trim().trim_matches(':');
                    // Find matching closing paren
                    let after_open = &rest[paren_pos + 1..];
                    let close_pos = after_open.find(')').unwrap_or(after_open.len());
                    let params = &after_open[..close_pos];
                    (fn_name, params)
                } else {
                    (rest.trim().trim_matches(':'), "")
                };

                let valid_name = is_valid_python_identifier(name);

                if valid_name {
                    local_symbols.push(LocalSymbol {
                        name: name.to_string(),
                        kind: "function".to_string(),
                        line: Some(line_num),
                        context: line.trim().to_string(),
                        is_exported: false,
                    });
                }

                if valid_name && indent == 0 && !name.starts_with('_') {
                    let params = parse_python_params(params_text);
                    analysis.exports.push(ExportSymbol::with_params(
                        name.to_string(),
                        "def",
                        "named",
                        Some(line_num),
                        params,
                    ));
                }

                // Mark function as used if decorated with callback/framework decorator
                if (pending_callback_decorator || pending_framework_decorator) && valid_name {
                    analysis.local_uses.push(name.to_string());
                }
                if pending_fixture_decorator && valid_name {
                    analysis.local_uses.push(name.to_string());
                    pending_fixture_name = Some(name.to_string());
                }
                if valid_name && !pending_routes.is_empty() {
                    for mut r in pending_routes.drain(..) {
                        if r.name.is_none() {
                            r.name = Some(name.to_string());
                        }
                        analysis.routes.push(r);
                    }
                } else {
                    pending_routes.clear();
                }
                pending_callback_decorator = false;
                pending_framework_decorator = false;
                pending_fixture_decorator = false;
                if let Some(fix) = pending_fixture_name.take() {
                    analysis.pytest_fixtures.push(fix);
                }
            } else if !trimmed.is_empty()
                && !trimmed.starts_with('#')
                && !trimmed.starts_with("class ")
            {
                // Module-level constant/variable bindings (e.g.
                // `FRAMEWORK_LAUNCHER_MARKERS = (...)`). Recorded as local
                // symbols ONLY (not exports) so `where-symbol`/`body` can resolve
                // them without seeding new dead-export false positives. Scoped to
                // indent == 0 so function-local assignments never leak in.
                if indent == 0
                    && let Some(name) = parse_module_const_target(trimmed)
                {
                    local_symbols.push(LocalSymbol {
                        name: name.to_string(),
                        kind: "const".to_string(),
                        line: Some(line_num),
                        context: line.trim().to_string(),
                        is_exported: false,
                    });
                }

                // Reset decorator flags if we hit a non-decorator, non-def, non-class line
                pending_framework_decorator = false;
                pending_routes.clear();
                pending_fixture_name = None;
            }
        }
    }

    for caps in regex_py_dynamic_importlib().captures_iter(content) {
        if let Some(m) = caps.get(1) {
            analysis.dynamic_imports.push(m.as_str().trim().to_string());
        }
    }
    for caps in regex_py_dynamic_dunder().captures_iter(content) {
        if let Some(m) = caps.get(1) {
            analysis.dynamic_imports.push(m.as_str().trim().to_string());
        }
    }

    // Process __all__ list: only add exports that aren't already defined
    // in this file (e.g., by class/def). This avoids "twins" false positives
    // where `__all__ = ["Foo"]` duplicates `class Foo:`.
    let existing_export_names: std::collections::HashSet<String> =
        analysis.exports.iter().map(|e| e.name.clone()).collect();
    for name in parse_all_list(content) {
        if !existing_export_names.contains(&name) {
            analysis
                .exports
                .push(ExportSymbol::new(name, "__all__", "named", None));
        }
    }

    if !local_symbols.is_empty() {
        let export_names: HashSet<String> =
            analysis.exports.iter().map(|e| e.name.clone()).collect();
        for symbol in &mut local_symbols {
            symbol.is_exported = export_names.contains(&symbol.name);
        }
        analysis.local_symbols = local_symbols;
    }

    // Detect Python entry points
    // 1. __main__.py files are package entry points
    if analysis.path.ends_with("__main__.py") {
        analysis.entry_points.push("__main__".to_string());
    }
    // 2. if __name__ == "__main__": is a script entry point
    if content.contains("if __name__")
        && (content.contains("__main__") || content.contains("'__main__'"))
        && !analysis.entry_points.contains(&"script".to_string())
    {
        analysis.entry_points.push("script".to_string());
        // Also mark 'main' as locally used if it's called in the __main__ block
        if content.contains("main()") && !analysis.local_uses.contains(&"main".to_string()) {
            analysis.local_uses.push("main".to_string());
        }
    }
    // 3. Web framework apps — any file that registers routes through a known
    //    framework decorator is, by definition, a server entrypoint. Surface
    //    one `<framework>_app` kind per framework, plus a transport hint
    //    (`asgi_target` for FastAPI/Starlette/Litestar, `wsgi_target` for Flask).
    {
        let mut seen_frameworks: HashSet<String> = HashSet::new();
        for route in &analysis.routes {
            if seen_frameworks.insert(route.framework.clone()) {
                let app_kind = format!("{}_app", route.framework);
                if !analysis.entry_points.contains(&app_kind) {
                    analysis.entry_points.push(app_kind);
                }
                let transport_kind: Option<&'static str> = match route.framework.as_str() {
                    "fastapi" | "starlette" | "litestar" => Some("asgi_target"),
                    "flask" => Some("wsgi_target"),
                    _ => None,
                };
                if let Some(kind) = transport_kind {
                    let kind_string = kind.to_string();
                    if !analysis.entry_points.contains(&kind_string) {
                        analysis.entry_points.push(kind_string);
                    }
                }
            }
        }
    }
    // 4. Bare `app = FastAPI(...)` / `Flask(__name__)` / `Starlette(...)` — files
    //    that construct an app instance at module scope (no routes registered
    //    in this file but still loaded by uvicorn/gunicorn). Detection is
    //    intentionally conservative: only `<ident> = <Class>(...)` at indent 0.
    if content.contains("FastAPI(") || content.contains("Starlette(") || content.contains("Flask(")
    {
        let mut detected: Vec<&'static str> = Vec::new();
        for line in content.lines() {
            let stripped = line.trim_start();
            if stripped.len() != line.len() {
                continue; // skip indented (function-local) constructions
            }
            if stripped.starts_with('#') {
                continue;
            }
            if stripped.contains("= FastAPI(") || stripped.contains("=FastAPI(") {
                detected.push("fastapi_app");
                detected.push("asgi_target");
            } else if stripped.contains("= Starlette(") || stripped.contains("=Starlette(") {
                detected.push("starlette_app");
                detected.push("asgi_target");
            } else if stripped.contains("= Flask(") || stripped.contains("=Flask(") {
                detected.push("flask_app");
                detected.push("wsgi_target");
            }
        }
        for kind in detected {
            let kind_string = kind.to_string();
            if !analysis.entry_points.contains(&kind_string) {
                analysis.entry_points.push(kind_string);
            }
        }
    }

    // Detect bare function calls in Python (similar to Rust detection)
    // This catches local function calls like `helper_func(...)` within the same file
    extract_python_function_calls(content, &mut analysis.local_uses);

    // Detect type hint usages (dict[str, MyClass], defaultdict(MyClass), etc.)
    extract_type_hint_usages(content, &mut analysis.local_uses);

    // Detect class references in tuple/list/dict literals (issue #2)
    // This catches patterns like: (ClassName, 'value'), [Foo, Bar], {'key': Baz}
    extract_class_from_containers(content, &mut analysis.local_uses);

    // Detect bare class name usage in function arguments and returns (issue #3)
    // This catches: return ClassName, issubclass(x, ClassName), isinstance(obj, MyClass)
    extract_bare_class_references(content, &mut analysis.local_uses);

    // Detect exec/eval/compile dynamic code generation patterns
    // This catches template strings like "def get%s" that generate symbols dynamically
    analysis.dynamic_exec_templates = detect_dynamic_exec_templates(content);

    // Detect sys.modules monkey-patching (e.g., sys.modules['compat'] = wrapper)
    // Files with these patterns have all exports accessible at runtime via the injected name
    analysis.sys_modules_injections = detect_sys_modules_injection(content);

    // Detect Python concurrency race indicators
    analysis.py_race_indicators = detect_py_race_indicators(content);
    analysis.log_messages = collect_python_log_messages(content);

    if !analysis.local_uses.is_empty() {
        let usage_names: HashSet<String> = analysis.local_uses.iter().cloned().collect();
        let lines: Vec<&str> = content.lines().collect();
        const MAX_USAGES_PER_FILE: usize = 1500;
        let usages = collect_symbol_usages_from_lines(&lines, &usage_names, MAX_USAGES_PER_FILE);
        if !usages.is_empty() {
            analysis.symbol_usages = usages;
        }
    }

    analysis
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ImportResolutionKind;
    use tempfile::tempdir;

    fn py_exts() -> HashSet<String> {
        ["py"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn python_log_function_extraction_records_logging_calls() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let path = root.join("app.py");
        let content = r#"
import logging

logger = logging.getLogger(__name__)

def handler():
    logging.info("started")
    logger.error("failed %s", "x")
"#;
        std::fs::write(&path, content).expect("write app");
        let exts = py_exts();
        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&exts),
            "app.py".to_string(),
            &[],
            &HashSet::new(),
        );
        assert!(
            analysis
                .log_messages
                .iter()
                .any(|msg| msg.format_string == "started"
                    && matches!(msg.level, crate::types::LogLevel::Info)
                    && msg.function_context.as_deref() == Some("handler"))
        );
        assert!(
            analysis
                .log_messages
                .iter()
                .any(|msg| msg.format_string == "failed %s"
                    && matches!(msg.level, crate::types::LogLevel::Error))
        );
    }

    #[test]
    fn marks_type_checking_imports_and_stdlib() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("foo.py"), "VALUE = 1").expect("write foo.py");
        let content = r#"
from typing import TYPE_CHECKING
if TYPE_CHECKING:
    import foo

import sys
"#;
        let path = root.join("main.py");
        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&py_exts()),
            "main.py".to_string(),
            &[root.to_path_buf()],
            python_stdlib_set(),
        );
        assert!(analysis.imports.len() >= 2);
        let foo = analysis
            .imports
            .iter()
            .find(|i| i.source == "foo")
            .expect("foo import");
        assert!(foo.is_type_checking);
        assert_eq!(foo.resolution, ImportResolutionKind::Local);
        assert!(foo.resolved_path.as_deref().unwrap().contains("foo.py"));

        let sys = analysis
            .imports
            .iter()
            .find(|i| i.source == "sys")
            .expect("sys import");
        assert!(!sys.is_type_checking);
        assert_eq!(sys.resolution, ImportResolutionKind::Stdlib);
        assert!(sys.resolved_path.is_none());
    }

    #[test]
    fn python_local_symbols_and_usages() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let path = root.join("sample.py");
        let content =
            "class Foo:\n    def method(self):\n        pass\n\ndef helper():\n    return Foo()\n";
        std::fs::write(&path, content).expect("write sample.py");

        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&py_exts()),
            "sample.py".to_string(),
            &[],
            python_stdlib_set(),
        );

        assert!(
            analysis.local_symbols.iter().any(|s| s.name == "Foo"),
            "Foo should be in local_symbols"
        );
        assert!(
            analysis.local_symbols.iter().any(|s| s.name == "helper"),
            "helper should be in local_symbols"
        );
        assert!(
            analysis.symbol_usages.iter().any(|u| u.name == "Foo"),
            "Foo should appear in symbol_usages"
        );
    }

    #[test]
    fn python_module_level_const_assignments_are_local_symbols() {
        // Hak (loctree-feedback.md, 2026-06-15): `loct body FRAMEWORK_LAUNCHER_MARKERS`
        // returned "(no source body found)" because module-level tuple/const
        // assignments were never recorded as symbols, so where-symbol/body found
        // nothing. They must land in local_symbols (NOT exports, to avoid new
        // dead-export false positives).
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let path = root.join("markers.py");
        let content = "FRAMEWORK_LAUNCHER_MARKERS = (\n    \"a\",\n    \"b\",\n)\n\nMAX_RETRIES: int = 3\n_private = 1\n\ndef helper():\n    local_var = 5\n    return local_var\n";
        std::fs::write(&path, content).expect("write markers.py");

        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&py_exts()),
            "markers.py".to_string(),
            &[],
            python_stdlib_set(),
        );

        let marker = analysis
            .local_symbols
            .iter()
            .find(|s| s.name == "FRAMEWORK_LAUNCHER_MARKERS")
            .expect("module-level tuple const should be a local symbol");
        assert_eq!(marker.kind, "const");
        assert_eq!(marker.line, Some(1));

        assert!(
            analysis
                .local_symbols
                .iter()
                .any(|s| s.name == "MAX_RETRIES" && s.kind == "const"),
            "annotated module const should be captured"
        );
        assert!(
            analysis.local_symbols.iter().any(|s| s.name == "_private"),
            "underscore-prefixed module const should still resolve for body"
        );
        // Function-local assignments must NOT pollute the module symbol index.
        assert!(
            !analysis.local_symbols.iter().any(|s| s.name == "local_var"),
            "indented (function-local) assignments must not be captured"
        );
        // Module consts stay out of exports so dead-export analysis is untouched.
        assert!(
            !analysis
                .exports
                .iter()
                .any(|e| e.name == "FRAMEWORK_LAUNCHER_MARKERS"),
            "module consts must not be added to exports (no new dead-export FPs)"
        );
    }

    #[test]
    fn python_assignment_lookalikes_are_not_captured_as_const() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let path = root.join("noise.py");
        // Comparisons, augmented assignment, subscript/attribute targets, and
        // tuple unpacking must not be misread as simple module consts.
        let content =
            "COUNTER += 1\nif x == 1:\n    pass\ncfg[\"k\"] = 2\nobj.attr = 3\na, b = 4, 5\n";
        std::fs::write(&path, content).expect("write noise.py");

        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&py_exts()),
            "noise.py".to_string(),
            &[],
            python_stdlib_set(),
        );

        for bad in ["COUNTER", "x", "cfg", "obj", "a", "b"] {
            assert!(
                !analysis
                    .local_symbols
                    .iter()
                    .any(|s| s.name == bad && s.kind == "const"),
                "{bad} must not be captured as a module const"
            );
        }
    }

    #[test]
    fn tracks_from_import_symbols_and_aliases() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("utils")).expect("mkdir utils");
        std::fs::write(
            root.join("utils/helpers.py"),
            "class Foo: pass\nclass Baz: pass",
        )
        .expect("write helpers");
        let content = "from utils.helpers import Foo as Bar, Baz";
        let path = root.join("main.py");
        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&py_exts()),
            "main.py".to_string(),
            &[root.to_path_buf()],
            python_stdlib_set(),
        );
        let imp = analysis.imports.first().expect("import entry");
        assert_eq!(imp.symbols.len(), 2);
        assert_eq!(imp.symbols[0].name, "Foo");
        assert_eq!(imp.symbols[0].alias.as_deref(), Some("Bar"));
        assert_eq!(imp.symbols[1].name, "Baz");
    }

    #[test]
    fn tracks_bare_dot_relative_import_edge() {
        // loctree-feedback.md (2026-06-16): `from . import (CONST, ...)` (bare-dot
        // relative import of package __init__ symbols) recorded NO import edge,
        // so intra-package consts were false dead. Root cause was
        // trim_end_matches('.') reducing module "." to "" → early return.
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("pkg")).expect("mkdir pkg");
        std::fs::write(
            root.join("pkg/__init__.py"),
            "FOO_CONST = \"x\"\nBAR_CONST = (1, 2)\n__all__ = [\"FOO_CONST\", \"BAR_CONST\"]\n",
        )
        .expect("write init");
        let content = "from . import (\n    FOO_CONST,\n    BAR_CONST,\n)\n\ndef use():\n    return FOO_CONST, BAR_CONST\n";
        let path = root.join("pkg/sibling.py");
        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&py_exts()),
            "pkg/sibling.py".to_string(),
            &[root.to_path_buf()],
            python_stdlib_set(),
        );

        let imp = analysis
            .imports
            .iter()
            .find(|i| i.source == ".")
            .expect("bare-dot relative import must be recorded with module \".\"");
        let names: Vec<&str> = imp.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"FOO_CONST"), "imported names: {names:?}");
        assert!(names.contains(&"BAR_CONST"), "imported names: {names:?}");
        assert!(
            imp.resolved_path
                .as_deref()
                .is_some_and(|p| p.ends_with("__init__.py")),
            "module \".\" must resolve to the package __init__.py, got {:?}",
            imp.resolved_path
        );
    }

    #[test]
    fn tracks_double_dot_relative_import_edge() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("pkg/sub")).expect("mkdir");
        std::fs::write(root.join("pkg/__init__.py"), "PARENT_CONST = 1\n").expect("write parent");
        let content = "from .. import PARENT_CONST\n";
        let path = root.join("pkg/sub/child.py");
        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&py_exts()),
            "pkg/sub/child.py".to_string(),
            &[root.to_path_buf()],
            python_stdlib_set(),
        );
        let imp = analysis
            .imports
            .iter()
            .find(|i| i.source == "..")
            .expect("double-dot relative import must be recorded with module \"..\"");
        assert!(imp.symbols.iter().any(|s| s.name == "PARENT_CONST"));
    }

    #[test]
    fn ignores_imports_inside_docstrings() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let content = "\"\"\"\nExample:\n    from app.middlewares.request_id import get_request_id\n\"\"\"\n\ndef real():\n    return 1\n";
        let path = root.join("main.py");
        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&py_exts()),
            "main.py".to_string(),
            &[root.to_path_buf()],
            python_stdlib_set(),
        );
        assert!(
            analysis.imports.is_empty(),
            "docstring-only import should be ignored"
        );
    }

    #[test]
    fn expands_all_for_star_import() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("pkg")).expect("mkdir pkg");
        std::fs::write(root.join("pkg/__init__.py"), "__all__ = ['Foo', 'Bar']")
            .expect("write __init__");
        let content = "from pkg import *";
        let path = root.join("main.py");
        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&py_exts()),
            "main.py".to_string(),
            &[root.to_path_buf()],
            python_stdlib_set(),
        );
        let reexports = analysis
            .reexports
            .iter()
            .find(|r| r.source == "pkg")
            .expect("pkg reexport");
        match &reexports.kind {
            ReexportKind::Named(names) => {
                assert_eq!(names.len(), 2);
                let exported_names: Vec<_> = names.iter().map(|(_, e)| e.as_str()).collect();
                assert!(exported_names.contains(&"Foo"));
                assert!(exported_names.contains(&"Bar"));
            }
            other => panic!("expected named reexport, got {:?}", other),
        }
        let exported: HashSet<_> = analysis.exports.iter().map(|e| e.name.clone()).collect();
        assert!(exported.contains("Foo"));
        assert!(exported.contains("Bar"));
    }

    #[test]
    fn treats_init_named_from_import_as_reexport() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("pkg")).expect("mkdir pkg");
        std::fs::write(
            root.join("pkg/foo.py"),
            "class Foo: pass\nclass Baz: pass\n",
        )
        .expect("write foo.py");

        let content = "from .foo import Foo as Bar, Baz";
        let path = root.join("pkg/__init__.py");
        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&py_exts()),
            "pkg/__init__.py".to_string(),
            &[root.to_path_buf()],
            python_stdlib_set(),
        );

        let reexport = analysis
            .reexports
            .iter()
            .find(|r| r.source == ".foo")
            .expect("expected .foo reexport");

        match &reexport.kind {
            ReexportKind::Named(names) => {
                assert!(names.contains(&(String::from("Foo"), String::from("Bar"))));
                assert!(names.contains(&(String::from("Baz"), String::from("Baz"))));
            }
            other => panic!("expected named reexport, got {:?}", other),
        }

        let exported: HashSet<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "reexport")
            .map(|e| e.name.as_str())
            .collect();
        assert!(exported.contains("Bar"));
        assert!(exported.contains("Baz"));
    }

    #[test]
    fn dynamic_imports_and_local_over_stdlib() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("json.py"), "LOCAL = True").expect("write json.py");
        let content = r#"
import json
mod = importlib.import_module(f"pkg.{name}")
dyn = __import__("x.y")
"#;
        let path = root.join("main.py");
        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&py_exts()),
            "main.py".to_string(),
            &[root.to_path_buf()],
            python_stdlib_set(),
        );
        let json_imp = analysis
            .imports
            .iter()
            .find(|i| i.source == "json")
            .expect("json import");
        assert_eq!(json_imp.resolution, ImportResolutionKind::Local);
        assert!(
            json_imp
                .resolved_path
                .as_deref()
                .unwrap_or("")
                .ends_with("json.py")
        );

        assert_eq!(analysis.dynamic_imports.len(), 2);
        assert!(analysis.dynamic_imports.iter().any(|s| s.contains("pkg.")));
        assert!(analysis.dynamic_imports.iter().any(|s| s.contains("x.y")));
    }

    #[test]
    fn parses_all_list_exports() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        let content = r#"
__all__ = ["foo", "bar"]

def foo():
    pass

def bar():
    pass

def _private():
    pass
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("module.py"),
            root,
            Some(&py_exts()),
            "module.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        let export_names: Vec<_> = analysis.exports.iter().map(|e| e.name.as_str()).collect();
        assert!(export_names.contains(&"foo"));
        assert!(export_names.contains(&"bar"));
        assert!(!export_names.contains(&"_private"));
    }

    #[test]
    fn parses_all_list_with_comments() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        let content = r#"
__all__ = [
    "foo",  # inline comment
    "bar",
    # "baz" is intentionally excluded
]
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("module.py"),
            root,
            Some(&py_exts()),
            "module.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        let export_names: Vec<_> = analysis.exports.iter().map(|e| e.name.as_str()).collect();
        assert!(export_names.contains(&"foo"));
        assert!(export_names.contains(&"bar"));
        assert!(!export_names.iter().any(|n| n.contains('#')));
        assert!(!export_names.contains(&"baz"));
    }

    #[test]
    fn parses_class_exports() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        let content = r#"
class MyClass:
    pass

class _PrivateClass:
    pass
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("classes.py"),
            root,
            Some(&py_exts()),
            "classes.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        let class_exports: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "class")
            .collect();
        assert!(class_exports.iter().any(|e| e.name == "MyClass"));
        assert!(!class_exports.iter().any(|e| e.name == "_PrivateClass"));
    }

    #[test]
    fn detects_main_entry_point() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        let content = r#"
def main():
    print("Hello")

if __name__ == "__main__":
    main()
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("__main__.py"),
            root,
            Some(&py_exts()),
            "__main__.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert!(analysis.entry_points.contains(&"__main__".to_string()));
    }

    #[test]
    fn detects_script_entry_point() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        let content = r#"
def main():
    print("Hello")

if __name__ == "__main__":
    main()
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("script.py"),
            root,
            Some(&py_exts()),
            "script.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert!(analysis.entry_points.contains(&"script".to_string()));
    }

    #[test]
    fn detects_test_file_by_path() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("tests")).expect("mkdir");
        std::fs::write(root.join("tests/test_utils.py"), "def test_foo(): pass")
            .expect("write test file");

        let content = "def test_foo(): pass";
        let analysis = analyze_py_file(
            content,
            &root.join("tests/test_utils.py"),
            root,
            Some(&py_exts()),
            "tests/test_utils.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert!(analysis.is_test);
    }

    #[test]
    fn detects_test_file_by_content() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        let content = r#"
import pytest

@pytest.fixture
def sample_fixture():
    return 42

def test_something(sample_fixture):
    assert sample_fixture == 42
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("my_tests.py"),
            root,
            Some(&py_exts()),
            "my_tests.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert!(analysis.is_test);
    }

    #[test]
    fn detects_typed_package() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("mypackage")).expect("mkdir");
        std::fs::write(root.join("mypackage/__init__.py"), "").expect("write __init__");
        std::fs::write(root.join("mypackage/py.typed"), "").expect("write py.typed");
        std::fs::write(root.join("mypackage/utils.py"), "def foo(): pass").expect("write utils");

        let content = "def foo(): pass";
        let analysis = analyze_py_file(
            content,
            &root.join("mypackage/utils.py"),
            root,
            Some(&py_exts()),
            "mypackage/utils.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert!(analysis.is_typed_package);
    }

    #[test]
    fn detects_non_typed_package() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("mypackage")).expect("mkdir");
        std::fs::write(root.join("mypackage/__init__.py"), "").expect("write __init__");
        std::fs::write(root.join("mypackage/utils.py"), "def foo(): pass").expect("write utils");

        let content = "def foo(): pass";
        let analysis = analyze_py_file(
            content,
            &root.join("mypackage/utils.py"),
            root,
            Some(&py_exts()),
            "mypackage/utils.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert!(!analysis.is_typed_package);
    }

    #[test]
    fn detects_namespace_package() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("namespace_pkg")).expect("mkdir");
        std::fs::write(root.join("namespace_pkg/module.py"), "VALUE = 1").expect("write module");

        let content = "VALUE = 1";
        let analysis = analyze_py_file(
            content,
            &root.join("namespace_pkg/module.py"),
            root,
            Some(&py_exts()),
            "namespace_pkg/module.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert!(analysis.is_namespace_package);
    }

    #[test]
    fn traditional_package_not_namespace() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("pkg")).expect("mkdir");
        std::fs::write(root.join("pkg/__init__.py"), "").expect("write __init__");
        std::fs::write(root.join("pkg/module.py"), "VALUE = 1").expect("write module");

        let content = "VALUE = 1";
        let analysis = analyze_py_file(
            content,
            &root.join("pkg/module.py"),
            root,
            Some(&py_exts()),
            "pkg/module.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert!(!analysis.is_namespace_package);
    }

    #[test]
    fn top_level_exports_have_lines_and_methods_not_exported() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let content = "\
class Base:\n    pass\n\nclass Child(Base):\n    def method(self):\n        pass\n\ndef top():\n    return True\n\nmenu = MenuItem(callback=top)\n";
        let analysis = analyze_py_file(
            content,
            &root.join("app.py"),
            root,
            Some(&py_exts()),
            "app.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        let names: Vec<_> = analysis.exports.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"Base"));
        assert!(names.contains(&"Child"));
        assert!(names.contains(&"top"));
        assert!(!names.contains(&"method"));

        let top_line = analysis
            .exports
            .iter()
            .find(|e| e.name == "top")
            .and_then(|e| e.line)
            .unwrap();
        assert_eq!(top_line, 8);

        assert!(analysis.local_uses.contains(&"top".to_string()));
        assert!(analysis.local_uses.contains(&"Base".to_string()));
    }

    #[test]
    fn detects_type_hint_usage() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        let content = r#"
from collections import defaultdict
from typing import Dict, List

class UserRateLimit:
    pass

class Session:
    pass

rate_limits: dict[str, UserRateLimit] = {}
sessions: Dict[str, Session] = {}

user_limits = defaultdict(UserRateLimit)

def get_limit(user_id: str) -> UserRateLimit:
    return rate_limits[user_id]

def process(items: List[Session]) -> None:
    pass
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("session_security.py"),
            root,
            Some(&py_exts()),
            "session_security.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert!(
            analysis.local_uses.contains(&"UserRateLimit".to_string()),
            "UserRateLimit not found in local_uses: {:?}",
            analysis.local_uses
        );
        assert!(
            analysis.local_uses.contains(&"Session".to_string()),
            "Session not found in local_uses: {:?}",
            analysis.local_uses
        );
    }

    #[test]
    fn marks_pytest_fixture_as_used() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        let content = r#"
import pytest

@pytest.fixture
def client():
    return object()
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("conftest.py"),
            root,
            Some(&py_exts()),
            "conftest.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert!(
            analysis.local_uses.contains(&"client".to_string()),
            "pytest fixture should be marked as used"
        );
        assert!(
            analysis.pytest_fixtures.contains(&"client".to_string()),
            "pytest fixture list should capture fixture name"
        );
    }

    #[test]
    fn captures_fastapi_route_metadata() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        let content = r#"
from fastapi import APIRouter
router = APIRouter()

@router.get("/patients")
def list_patients():
    return []
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("api.py"),
            root,
            Some(&py_exts()),
            "api.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert_eq!(analysis.routes.len(), 1);
        let route = &analysis.routes[0];
        assert_eq!(route.framework, "fastapi");
        assert_eq!(route.method, "GET");
        assert_eq!(route.path.as_deref(), Some("/patients"));
        assert_eq!(route.name.as_deref(), Some("list_patients"));
    }

    #[test]
    fn captures_async_def_fastapi_route() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        // Test that async def functions are correctly associated with routes
        let content = r#"
from fastapi import APIRouter
router = APIRouter()

@router.post("/items")
async def create_item(data: dict):
    return {"created": True}

@router.get("/items/{item_id}")
async def get_item(item_id: int):
    return {"id": item_id}
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("api.py"),
            root,
            Some(&py_exts()),
            "api.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert_eq!(
            analysis.routes.len(),
            2,
            "Should detect both async def routes"
        );

        let post_route = &analysis.routes[0];
        assert_eq!(post_route.framework, "fastapi");
        assert_eq!(post_route.method, "POST");
        assert_eq!(post_route.path.as_deref(), Some("/items"));
        assert_eq!(post_route.name.as_deref(), Some("create_item"));

        let get_route = &analysis.routes[1];
        assert_eq!(get_route.framework, "fastapi");
        assert_eq!(get_route.method, "GET");
        assert_eq!(get_route.path.as_deref(), Some("/items/{item_id}"));
        assert_eq!(get_route.name.as_deref(), Some("get_item"));
    }

    #[test]
    fn captures_flask_route_methods_list() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        let content = r#"
from flask import Blueprint
bp = Blueprint("bp", __name__)

@bp.route("/ping", methods=["GET", "POST"])
def ping():
    return "ok"
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("flask_app.py"),
            root,
            Some(&py_exts()),
            "flask_app.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert_eq!(analysis.routes.len(), 1);
        let route = &analysis.routes[0];
        assert_eq!(route.framework, "flask");
        assert_eq!(route.method, "GET,POST");
        assert_eq!(route.path.as_deref(), Some("/ping"));
        assert_eq!(route.name.as_deref(), Some("ping"));
    }

    #[test]
    fn golang_gdb_pattern_full_integration() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        let content = r#"
class StringTypePrinter:
    pattern = re.compile(r'^struct string$')

class SliceTypePrinter:
    pattern = re.compile(r'^struct \[\]')

class MapTypePrinter:
    pattern = re.compile(r'^map\[')

class ChanTypePrinter:
    pattern = re.compile(r'^chan ')

class GoLenFunc(gdb.Function):
    how = ((StringTypePrinter, 'len'),
           (SliceTypePrinter, 'len'),
           (MapTypePrinter, 'used'),
           (ChanTypePrinter, 'qcount'))

    def invoke(self, obj):
        typename = str(obj.type)
        for klass, fld in self.how:
            if klass.pattern.match(typename):
                return obj[fld]
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("gdb_golang.py"),
            root,
            Some(&py_exts()),
            "gdb_golang.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert!(
            analysis
                .local_uses
                .contains(&"StringTypePrinter".to_string()),
            "StringTypePrinter not found in local_uses: {:?}",
            analysis.local_uses
        );
        assert!(
            analysis
                .local_uses
                .contains(&"SliceTypePrinter".to_string()),
            "SliceTypePrinter not found in local_uses"
        );
        assert!(
            analysis.local_uses.contains(&"MapTypePrinter".to_string()),
            "MapTypePrinter not found in local_uses"
        );
        assert!(
            analysis.local_uses.contains(&"ChanTypePrinter".to_string()),
            "ChanTypePrinter not found in local_uses"
        );

        let export_names: Vec<_> = analysis.exports.iter().map(|e| e.name.as_str()).collect();
        assert!(export_names.contains(&"StringTypePrinter"));
        assert!(export_names.contains(&"SliceTypePrinter"));
        assert!(export_names.contains(&"MapTypePrinter"));
        assert!(export_names.contains(&"ChanTypePrinter"));
        assert!(export_names.contains(&"GoLenFunc"));
    }

    #[test]
    fn detects_mixin_class_usage() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        let content = r#"
class ButtonsColumnMixin:
    """Mixin for button column functionality"""
    pass

class WagtailAdminDraftStateFormMixin:
    pass

class IndexViewOptionalFeaturesMixin:
    pass

class NullAdminURLFinder:
    """Class used in same-file reference"""
    pass

class MyView(IndexViewOptionalFeaturesMixin, ButtonsColumnMixin):
    pass

def get_finder():
    return NullAdminURLFinder

def check_column(column_class):
    if issubclass(column_class, ButtonsColumnMixin):
        return True
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("views.py"),
            root,
            Some(&py_exts()),
            "views.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert!(
            analysis
                .local_uses
                .contains(&"ButtonsColumnMixin".to_string()),
            "ButtonsColumnMixin should be marked as used (inheritance): {:?}",
            analysis.local_uses
        );
        assert!(
            analysis
                .local_uses
                .contains(&"IndexViewOptionalFeaturesMixin".to_string()),
            "IndexViewOptionalFeaturesMixin should be marked as used (inheritance): {:?}",
            analysis.local_uses
        );
        assert!(
            analysis
                .local_uses
                .contains(&"NullAdminURLFinder".to_string()),
            "NullAdminURLFinder should be marked as used (function return): {:?}",
            analysis.local_uses
        );
    }

    #[test]
    fn handles_utf8_emoji_in_python_code() {
        let code = r#"
"""
This docstring has emoji and ellipsis and bullet points
"""

class MyClass:
    """Another docstring with emoji"""

    def method(self):
        return MyHelper  # Class reference after emoji content

class MyHelper:
    pass
"#;

        let temp = tempdir().unwrap();
        let py_file = temp.path().join("test_emoji.py");
        std::fs::write(&py_file, code).unwrap();

        let relative = py_file
            .strip_prefix(temp.path())
            .unwrap()
            .to_string_lossy()
            .to_string();
        let analysis = analyze_py_file(
            code,
            &py_file,
            temp.path(),
            Some(&py_exts()),
            relative,
            &[],
            &HashSet::new(),
        );

        assert!(analysis.exports.iter().any(|e| e.name == "MyClass"));
        assert!(analysis.exports.iter().any(|e| e.name == "MyHelper"));
    }

    #[test]
    fn parses_multiline_from_import() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("pkg/sub")).expect("mkdir pkg/sub");
        std::fs::write(
            root.join("pkg/sub/adapter.py"),
            "class AnthropicMessagesAdapter:\n    pass\n\nclass OtherClass:\n    pass\n",
        )
        .expect("write adapter.py");
        std::fs::write(root.join("pkg/__init__.py"), "").expect("write pkg init");
        std::fs::write(root.join("pkg/sub/__init__.py"), "").expect("write sub init");

        let content = r#"
from pkg.sub.adapter import (
    AnthropicMessagesAdapter,
)

def use_adapter():
    return AnthropicMessagesAdapter()
"#;

        let path = root.join("router.py");
        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&py_exts()),
            "router.py".to_string(),
            &[root.to_path_buf()],
            python_stdlib_set(),
        );

        // Should have found the import
        assert!(
            !analysis.imports.is_empty(),
            "multiline import should be parsed"
        );

        let imp = analysis
            .imports
            .iter()
            .find(|i| i.source == "pkg.sub.adapter")
            .expect("pkg.sub.adapter import should exist");

        // Should have extracted the symbol
        assert_eq!(imp.symbols.len(), 1, "should have one imported symbol");
        assert_eq!(
            imp.symbols[0].name, "AnthropicMessagesAdapter",
            "symbol name should match"
        );

        // Should have resolved to local file
        assert_eq!(
            imp.resolution,
            ImportResolutionKind::Local,
            "should resolve as local"
        );
        assert!(
            imp.resolved_path
                .as_ref()
                .is_some_and(|p| p.contains("adapter.py")),
            "should resolve to adapter.py"
        );
    }

    #[test]
    fn parses_multiline_from_import_multiple_symbols() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("mypackage")).expect("mkdir mypackage");
        std::fs::write(
            root.join("mypackage/models.py"),
            "class Foo:\n    pass\n\nclass Bar:\n    pass\n\nclass Baz:\n    pass\n",
        )
        .expect("write models.py");
        std::fs::write(root.join("mypackage/__init__.py"), "").expect("write init");

        let content = r#"
from mypackage.models import (
    Foo,
    Bar as AliasedBar,
    Baz,  # trailing comma
)

x = Foo()
"#;

        let path = root.join("main.py");
        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&py_exts()),
            "main.py".to_string(),
            &[root.to_path_buf()],
            python_stdlib_set(),
        );

        let imp = analysis
            .imports
            .iter()
            .find(|i| i.source == "mypackage.models")
            .expect("mypackage.models import should exist");

        assert_eq!(imp.symbols.len(), 3, "should have three imported symbols");

        let foo = imp.symbols.iter().find(|s| s.name == "Foo");
        assert!(foo.is_some(), "Foo should be imported");
        assert!(foo.unwrap().alias.is_none(), "Foo should have no alias");

        let bar = imp.symbols.iter().find(|s| s.name == "Bar");
        assert!(bar.is_some(), "Bar should be imported");
        assert_eq!(
            bar.unwrap().alias.as_deref(),
            Some("AliasedBar"),
            "Bar should have alias AliasedBar"
        );

        let baz = imp.symbols.iter().find(|s| s.name == "Baz");
        assert!(baz.is_some(), "Baz should be imported");
    }

    #[test]
    fn parses_multiline_from_import_in_init_as_reexport() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("chat/anthropic")).expect("mkdir");
        std::fs::write(
            root.join("chat/anthropic/anthropic_messages_adapter.py"),
            "class AnthropicMessagesAdapter:\n    pass\n",
        )
        .expect("write adapter");
        std::fs::write(root.join("chat/__init__.py"), "").expect("write chat init");
        std::fs::write(root.join("chat/anthropic/__init__.py"), "").expect("write anthropic init");

        // This is what __init__.py often looks like - multiline import for re-export
        let content = r#"
from .anthropic_messages_adapter import (
    AnthropicMessagesAdapter,
)
"#;

        let path = root.join("chat/anthropic/__init__.py");
        let analysis = analyze_py_file(
            content,
            &path,
            root,
            Some(&py_exts()),
            "chat/anthropic/__init__.py".to_string(),
            &[root.to_path_buf()],
            python_stdlib_set(),
        );

        // Should recognize this as a re-export
        assert!(
            !analysis.reexports.is_empty(),
            "should have re-exports from __init__.py"
        );

        let reexport = analysis
            .reexports
            .iter()
            .find(|r| r.source == ".anthropic_messages_adapter")
            .expect("should have reexport from .anthropic_messages_adapter");

        match &reexport.kind {
            ReexportKind::Named(names) => {
                assert!(
                    names.iter().any(|(_, e)| e == "AnthropicMessagesAdapter"),
                    "should re-export AnthropicMessagesAdapter"
                );
            }
            other => panic!("expected named reexport, got {:?}", other),
        }

        // Should also appear in exports
        assert!(
            analysis
                .exports
                .iter()
                .any(|e| e.name == "AnthropicMessagesAdapter" && e.kind == "reexport"),
            "AnthropicMessagesAdapter should be in exports as reexport"
        );
    }

    #[test]
    fn all_list_does_not_duplicate_class_exports() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        // Common Python pattern: class definition + __all__ list
        let content = r#"
__all__ = ["Foo", "Bar"]

class Foo:
    pass

class Bar:
    pass
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("module.py"),
            root,
            Some(&py_exts()),
            "module.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        // Should have exactly 2 exports (Foo and Bar from class definitions)
        // NOT 4 (which would happen if __all__ duplicated them)
        let export_names: Vec<_> = analysis.exports.iter().map(|e| &e.name).collect();
        assert_eq!(
            export_names.len(),
            2,
            "should have 2 exports, not duplicates: {:?}",
            export_names
        );

        // Both should be class exports, not __all__ exports
        let foo_export = analysis.exports.iter().find(|e| e.name == "Foo");
        assert!(foo_export.is_some());
        assert_eq!(
            foo_export.unwrap().kind,
            "class",
            "Foo should be class export, not __all__"
        );
    }

    #[test]
    fn all_list_adds_exports_not_defined_in_file() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        // __all__ can export names imported from elsewhere (re-export pattern)
        let content = r#"
from .submodule import ExternalClass

__all__ = ["ExternalClass", "LocalClass"]

class LocalClass:
    pass
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("module.py"),
            root,
            Some(&py_exts()),
            "module.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        let export_names: Vec<_> = analysis.exports.iter().map(|e| &e.name).collect();

        // LocalClass should be class export
        let local = analysis.exports.iter().find(|e| e.name == "LocalClass");
        assert!(local.is_some());
        assert_eq!(local.unwrap().kind, "class");

        // ExternalClass should be __all__ export (not defined locally)
        let external = analysis.exports.iter().find(|e| e.name == "ExternalClass");
        assert!(
            external.is_some(),
            "ExternalClass should be in exports: {:?}",
            export_names
        );
        assert_eq!(
            external.unwrap().kind,
            "__all__",
            "ExternalClass should be __all__ export since not defined locally"
        );
    }

    #[test]
    fn parses_function_params() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        let content = r#"
def simple(x, y):
    pass

def typed(x: int, y: str):
    pass

def with_defaults(x: int = 5, y: str = 'hello'):
    pass

def variadic(*args, **kwargs):
    pass

def typed_variadic(*args: tuple, **kwargs: dict):
    pass

def mixed(self, x: int, y: str = 'default', *args, **kwargs):
    pass

async def async_typed(request: Request, db: Database) -> Response:
    pass
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("module.py"),
            root,
            Some(&py_exts()),
            "module.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        // Test simple function
        let simple = analysis.exports.iter().find(|e| e.name == "simple");
        assert!(simple.is_some(), "simple function should be exported");
        let simple_params = &simple.unwrap().params;
        assert_eq!(simple_params.len(), 2);
        assert_eq!(simple_params[0].name, "x");
        assert!(simple_params[0].type_annotation.is_none());
        assert!(!simple_params[0].has_default);

        // Test typed function
        let typed = analysis.exports.iter().find(|e| e.name == "typed");
        assert!(typed.is_some());
        let typed_params = &typed.unwrap().params;
        assert_eq!(typed_params.len(), 2);
        assert_eq!(typed_params[0].name, "x");
        assert_eq!(typed_params[0].type_annotation.as_deref(), Some("int"));
        assert_eq!(typed_params[1].type_annotation.as_deref(), Some("str"));

        // Test function with defaults
        let with_defaults = analysis.exports.iter().find(|e| e.name == "with_defaults");
        assert!(with_defaults.is_some());
        let wd_params = &with_defaults.unwrap().params;
        assert!(wd_params[0].has_default);
        assert!(wd_params[1].has_default);

        // Test variadic function
        let variadic = analysis.exports.iter().find(|e| e.name == "variadic");
        assert!(variadic.is_some());
        let var_params = &variadic.unwrap().params;
        assert_eq!(var_params.len(), 2);
        assert_eq!(var_params[0].name, "*args");
        assert_eq!(var_params[1].name, "**kwargs");

        // Test mixed function
        let mixed = analysis.exports.iter().find(|e| e.name == "mixed");
        assert!(mixed.is_some());
        let mixed_params = &mixed.unwrap().params;
        assert_eq!(mixed_params.len(), 5);
        assert_eq!(mixed_params[0].name, "self");
        assert_eq!(mixed_params[1].name, "x");
        assert_eq!(mixed_params[1].type_annotation.as_deref(), Some("int"));
        assert!(!mixed_params[1].has_default);
        assert_eq!(mixed_params[2].name, "y");
        assert!(mixed_params[2].has_default);
        assert_eq!(mixed_params[3].name, "*args");
        assert_eq!(mixed_params[4].name, "**kwargs");

        // Test async function
        let async_typed = analysis.exports.iter().find(|e| e.name == "async_typed");
        assert!(async_typed.is_some());
        let async_params = &async_typed.unwrap().params;
        assert_eq!(async_params.len(), 2);
        assert_eq!(async_params[0].name, "request");
        assert_eq!(async_params[0].type_annotation.as_deref(), Some("Request"));
        assert_eq!(async_params[1].type_annotation.as_deref(), Some("Database"));
    }

    #[test]
    fn parses_complex_type_annotations() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();

        let content = r#"
def generic(items: List[Dict[str, Any]], callback: Callable[[int], bool]) -> Optional[str]:
    pass
"#;

        let analysis = analyze_py_file(
            content,
            &root.join("module.py"),
            root,
            Some(&py_exts()),
            "module.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        let generic = analysis.exports.iter().find(|e| e.name == "generic");
        assert!(generic.is_some());
        let params = &generic.unwrap().params;
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "items");
        assert_eq!(
            params[0].type_annotation.as_deref(),
            Some("List[Dict[str, Any]]")
        );
        assert_eq!(params[1].name, "callback");
        assert_eq!(
            params[1].type_annotation.as_deref(),
            Some("Callable[[int], bool]")
        );
    }

    /// Regression test for Issues/context-tool-fstring-symbols-false-positive.md
    ///
    /// Python files that render HTML/JS via f-strings used to produce
    /// `class FrameMarker {{` symbols labelled `kind: class, authority:
    /// repo_verified`. The fix gates symbol promotion on a valid Python
    /// identifier check, so the polluted names never escape the analyzer
    /// even when the line-based scanner can't otherwise distinguish text
    /// inside a Python f-string from real Python code.
    #[test]
    fn fstring_embedded_js_class_does_not_become_python_export() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        // The real class lives BEFORE the polluted f-string so the line-based
        // parser still picks it up. The bug we're fixing is whether the
        // f-string-leaked `class FrameMarker {{` becomes a fake export — not
        // whether the parser handles every f-string termination edge case.
        let content = r#"
class RealPythonClass:
    pass

def render_html() -> str:
    return f"""
    <script>
    class FrameMarker {{
        constructor() {{ }}
    }}
    class VoiceRecorder {{
        record() {{ }}
    }}
    </script>
    """
"#;
        let analysis = analyze_py_file(
            content,
            &root.join("server.py"),
            root,
            Some(&py_exts()),
            "server.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        let names: Vec<&str> = analysis.exports.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"RealPythonClass"),
            "real top-level class should still export: {names:?}"
        );
        for export in &analysis.exports {
            assert!(
                !export.name.contains('{') && !export.name.contains(' '),
                "f-string-leaked symbol must not survive: {export:?}"
            );
            assert_ne!(export.name, "FrameMarker {{");
            assert_ne!(export.name, "VoiceRecorder {{");
        }
        for local in &analysis.local_symbols {
            assert!(
                !local.name.contains('{') && !local.name.contains(' '),
                "f-string-leaked local symbol must not survive: {local:?}"
            );
        }
    }

    /// Regression test for Issues/context-tool-fastapi-app-factories-not-detected.md
    ///
    /// Files declaring FastAPI routes must surface `fastapi_app` and
    /// `asgi_target` in their entry_points so the structural slice can
    /// promote them as server entrypoints.
    #[test]
    fn fastapi_routes_promote_file_to_fastapi_app_entrypoint() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let content = r#"
from fastapi import FastAPI

app = FastAPI()

@app.get("/users")
def list_users():
    return []

@app.post("/users")
def create_user(payload: dict):
    return payload
"#;
        let analysis = analyze_py_file(
            content,
            &root.join("server.py"),
            root,
            Some(&py_exts()),
            "server.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert!(
            analysis.entry_points.contains(&"fastapi_app".to_string()),
            "expected fastapi_app entrypoint, got: {:?}",
            analysis.entry_points
        );
        assert!(
            analysis.entry_points.contains(&"asgi_target".to_string()),
            "expected asgi_target entrypoint, got: {:?}",
            analysis.entry_points
        );
    }

    /// Same promotion path for Flask — different transport (`wsgi_target`).
    #[test]
    fn flask_app_promotes_file_to_flask_app_wsgi_entrypoint() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let content = r#"
from flask import Flask

app = Flask(__name__)

@app.route("/health")
def health():
    return "ok"
"#;
        let analysis = analyze_py_file(
            content,
            &root.join("flask_app.py"),
            root,
            Some(&py_exts()),
            "flask_app.py".to_string(),
            &[root.to_path_buf()],
            &HashSet::new(),
        );

        assert!(
            analysis.entry_points.contains(&"flask_app".to_string()),
            "expected flask_app entrypoint, got: {:?}",
            analysis.entry_points
        );
        assert!(
            analysis.entry_points.contains(&"wsgi_target".to_string()),
            "expected wsgi_target entrypoint, got: {:?}",
            analysis.entry_points
        );
    }
}
