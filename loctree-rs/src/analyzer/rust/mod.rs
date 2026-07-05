// Rust analyzer module structure
mod imports;
mod naming;
mod preprocess;
mod tauri;
mod usages;

// Re-export public items
pub use imports::CrateModuleMap;

// Imports from submodules
use imports::parse_rust_brace_names;
use naming::exposed_command_name;
use preprocess::{
    find_balanced_bracket, strip_cfg_attributes, strip_cfg_test_modules, strip_function_body_uses,
};
use tauri::{extract_plugin_identifier, extract_plugin_name};
use usages::{
    collect_identifier_mentions, collect_rust_signature_uses, extract_bare_function_calls,
    extract_function_arguments, extract_identifier_usages, extract_path_qualified_calls,
    extract_struct_field_types, extract_type_alias_qualified_paths,
};

// External imports
use super::offset_to_line;
use super::regexes::{
    regex_custom_command_fn, regex_event_const_rust, regex_event_emit_rust,
    regex_event_listen_rust, regex_rust_async_main_attr, regex_rust_fn_main, regex_rust_mod_decl,
    regex_rust_pub_use, regex_rust_use, regex_tauri_command_fn, regex_tauri_generate_handler,
    rust_pub_const_regexes, rust_pub_decl_regexes,
};
use crate::semantic::rust::{parse_impl_blocks, strip_comments};
use crate::types::{
    CommandRef, EventRef, ExportSymbol, FileAnalysis, ImplMethod, ImportEntry, ImportKind,
    LocalSymbol, LogLevel, LogMessage, ParamInfo, ReexportEntry, ReexportKind, SymbolUsage,
};
use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

/// Extract params from content starting at a given position after function name.
/// Looks for `(...)` and parses the params inside.
fn extract_rust_fn_params(content: &str, after_name_pos: usize) -> Vec<ParamInfo> {
    // Find opening paren
    let rest = &content[after_name_pos..];
    let Some(paren_start) = rest.find('(') else {
        return Vec::new();
    };

    // Find matching closing paren (handle nested generics)
    let params_start = after_name_pos + paren_start + 1;
    let mut depth = 1;
    let mut end_pos = params_start;
    for (i, ch) in content[params_start..].char_indices() {
        match ch {
            '(' | '<' | '[' | '{' => depth += 1,
            ')' | '>' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    end_pos = params_start + i;
                    break;
                }
            }
            _ => {}
        }
    }

    if depth != 0 {
        return Vec::new();
    }

    let params_text = &content[params_start..end_pos];
    parse_rust_params(params_text)
}

/// Parse Rust function params like `x: i32, y: &str, z: Option<T>`.
/// Skips `self`, `&self`, `&mut self`.
fn parse_rust_params(params_text: &str) -> Vec<ParamInfo> {
    let mut params = Vec::new();
    let mut current = String::new();
    let mut depth: usize = 0;

    for ch in params_text.chars() {
        match ch {
            '<' | '(' | '[' | '{' => {
                depth += 1;
                current.push(ch);
            }
            '>' | ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                if let Some(p) = parse_single_rust_param(current.trim()) {
                    params.push(p);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    // Last param
    if !current.trim().is_empty()
        && let Some(p) = parse_single_rust_param(current.trim())
    {
        params.push(p);
    }

    params
}

/// Parse a single Rust param like `name: Type`.
fn parse_single_rust_param(param: &str) -> Option<ParamInfo> {
    let param = param.trim();
    if param.is_empty() {
        return None;
    }

    // Skip self variants
    if param == "self"
        || param == "&self"
        || param == "&mut self"
        || param == "mut self"
        || param.starts_with("self:")
    {
        return None;
    }

    // Parse `name: Type` or `mut name: Type`
    let param = param.strip_prefix("mut ").unwrap_or(param);

    if let Some((name, type_ann)) = param.split_once(':') {
        Some(ParamInfo {
            name: name.trim().to_string(),
            type_annotation: Some(type_ann.trim().to_string()),
            has_default: false, // Rust doesn't have default params
        })
    } else {
        // Just a name without type annotation (rare in Rust)
        Some(ParamInfo {
            name: param.to_string(),
            type_annotation: None,
            has_default: false,
        })
    }
}

fn rust_local_decl_regexes() -> &'static Vec<(&'static str, Regex)> {
    static RE: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    RE.get_or_init(|| {
        vec![
            (
                "function",
                Regex::new(
                    r#"(?m)^\s*(?:pub\s*(?:\([^)]*\)\s*)?)?(?:async\s+|const\s+|unsafe\s+|extern(?:\s+"[^"]+")?\s+)*fn\s+([A-Za-z_][A-Za-z0-9_]*)"#,
                )
                .expect("valid rust fn regex"),
            ),
            (
                "struct",
                Regex::new(
                    r#"(?m)^\s*(?:pub\s*(?:\([^)]*\)\s*)?)?struct\s+([A-Za-z_][A-Za-z0-9_]*)"#,
                )
                .expect("valid rust struct regex"),
            ),
            (
                "enum",
                Regex::new(
                    r#"(?m)^\s*(?:pub\s*(?:\([^)]*\)\s*)?)?enum\s+([A-Za-z_][A-Za-z0-9_]*)"#,
                )
                .expect("valid rust enum regex"),
            ),
            (
                "trait",
                Regex::new(
                    r#"(?m)^\s*(?:pub\s*(?:\([^)]*\)\s*)?)?trait\s+([A-Za-z_][A-Za-z0-9_]*)"#,
                )
                .expect("valid rust trait regex"),
            ),
            (
                "type",
                Regex::new(
                    r#"(?m)^\s*(?:pub\s*(?:\([^)]*\)\s*)?)?type\s+([A-Za-z_][A-Za-z0-9_]*)"#,
                )
                .expect("valid rust type regex"),
            ),
            (
                "union",
                Regex::new(
                    r#"(?m)^\s*(?:pub\s*(?:\([^)]*\)\s*)?)?union\s+([A-Za-z_][A-Za-z0-9_]*)"#,
                )
                .expect("valid rust union regex"),
            ),
            (
                "const",
                Regex::new(
                    r#"(?m)^\s*(?:pub\s*(?:\([^)]*\)\s*)?)?const\s+([A-Za-z_][A-Za-z0-9_]*)"#,
                )
                .expect("valid rust const regex"),
            ),
            (
                "static",
                Regex::new(
                    r#"(?m)^\s*(?:pub\s*(?:\([^)]*\)\s*)?)?static\s+(?:mut\s+)?(?:ref\s+)?([A-Za-z_][A-Za-z0-9_]*)"#,
                )
                .expect("valid rust static regex"),
            ),
        ]
    })
}

fn rust_line_context(lines: &[&str], line: usize) -> String {
    if line == 0 {
        return String::new();
    }
    lines
        .get(line.saturating_sub(1))
        .map(|l| l.trim().to_string())
        .unwrap_or_default()
}

fn rust_fn_context_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?m)^\s*(?:pub\s*(?:\([^)]*\)\s*)?)?(?:async\s+|const\s+|unsafe\s+|extern(?:\s+"[^"]+")?\s+)*fn\s+([A-Za-z_][A-Za-z0-9_]*)"#,
        )
        .expect("valid rust function context regex")
    })
}

fn rust_log_macro_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?m)(?P<macro>(?:(?:tracing|log)::)?(?:trace|debug|info|warn|error)!|(?:println|eprintln|print|eprint|panic|unimplemented|todo|unreachable)!)\s*\("#,
        )
        .expect("valid rust log macro regex")
    })
}

fn nearest_function_context(functions: &[(usize, String)], line: usize) -> Option<String> {
    functions
        .iter()
        .take_while(|(fn_line, _)| *fn_line <= line)
        .last()
        .map(|(_, name)| name.clone())
}

fn extract_rust_format_string(input: &str) -> Option<String> {
    let mut literals = Vec::new();
    let mut escaped = false;
    let mut in_string = false;
    let mut start = 0usize;
    for (idx, ch) in input.char_indices() {
        if !in_string {
            if ch == '"' {
                in_string = true;
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
        if ch == '"' {
            literals.push(input[start..idx].to_string());
            if literals.len() >= 2 {
                break;
            }
            in_string = false;
        }
    }
    if input.trim_start().starts_with("target:") && literals.len() > 1 {
        literals.get(1).cloned()
    } else {
        literals.into_iter().next()
    }
}

fn rust_log_level(macro_name: &str) -> LogLevel {
    let base = macro_name
        .trim_end_matches('!')
        .rsplit("::")
        .next()
        .unwrap_or(macro_name);
    match base {
        "trace" => LogLevel::Trace,
        "debug" => LogLevel::Debug,
        "warn" => LogLevel::Warn,
        "error" | "eprintln" | "eprint" => LogLevel::Error,
        "panic" | "unimplemented" | "todo" | "unreachable" => LogLevel::Panic,
        _ => LogLevel::Info,
    }
}

fn collect_impl_methods(content: &str) -> Vec<ImplMethod> {
    let stripped = strip_comments(content);
    let mut out = Vec::new();
    for block in parse_impl_blocks(&stripped) {
        for method in block.methods {
            out.push(ImplMethod {
                name: method.name,
                qualifier: block.type_name.clone(),
                trait_qualifier: block.trait_name.clone(),
                line: Some(offset_to_line(&stripped, method.byte_offset)),
                is_async: method.is_async,
                visibility: method.visibility,
                is_definition: true,
            });
        }
    }
    out
}

fn collect_rust_log_messages(content: &str) -> Vec<LogMessage> {
    let functions: Vec<(usize, String)> = rust_fn_context_regex()
        .captures_iter(content)
        .filter_map(|caps| {
            let name = caps.get(1)?;
            Some((
                offset_to_line(content, name.start()),
                name.as_str().to_string(),
            ))
        })
        .collect();
    let mut messages = Vec::new();
    for caps in rust_log_macro_regex().captures_iter(content) {
        let Some(macro_match) = caps.name("macro") else {
            continue;
        };
        let Some(full_match) = caps.get(0) else {
            continue;
        };
        let line = offset_to_line(content, macro_match.start());
        let args_start = full_match.end();
        let args_tail = &content[args_start..];
        let format_string = extract_rust_format_string(args_tail).unwrap_or_default();
        if format_string.is_empty() {
            continue;
        }
        messages.push(LogMessage {
            level: rust_log_level(macro_match.as_str()),
            macro_or_fn: macro_match.as_str().to_string(),
            format_string,
            line,
            function_context: nearest_function_context(&functions, line),
        });
    }
    messages
}

fn collect_rust_local_symbols(content: &str, exported_names: &HashSet<String>) -> Vec<LocalSymbol> {
    let lines: Vec<&str> = content.lines().collect();
    let mut locals = Vec::new();
    const SKIP_NAMES: &[&str] = &["fn"];

    for (kind, re) in rust_local_decl_regexes() {
        for caps in re.captures_iter(content) {
            let Some(name) = caps.get(1) else { continue };
            let name_str = name.as_str().to_string();
            if exported_names.contains(&name_str) {
                continue;
            }
            if SKIP_NAMES.contains(&name_str.as_str()) {
                continue;
            }
            let line = offset_to_line(content, name.start());
            locals.push(LocalSymbol {
                name: name_str,
                kind: (*kind).to_string(),
                line: Some(line),
                context: rust_line_context(&lines, line),
                is_exported: false,
            });
        }
    }

    locals
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

pub(crate) fn analyze_rust_file(
    content: &str,
    relative: String,
    custom_command_macros: &[String],
) -> FileAnalysis {
    let mut analysis = FileAnalysis::new(relative.clone());
    let mut event_emits = Vec::new();
    let mut event_listens = Vec::new();

    // Extract plugin identifier for Tauri plugins
    // Tries: 1) #![plugin(identifier = "...")] attribute
    //        2) tauri-plugin-XXX in path
    //        3) plugins/XXX/ in path
    let plugin_identifier = extract_plugin_identifier(content, &relative);

    // Strip #[cfg(test)] modules and inline function-body imports to avoid false positive cycles
    let production_content = strip_function_body_uses(&strip_cfg_test_modules(content));

    for caps in regex_rust_use().captures_iter(&production_content) {
        let source = caps.get(1).map(|m| m.as_str()).unwrap_or("").trim();
        if source.is_empty() {
            continue;
        }

        let mut imp = ImportEntry::new(source.to_string(), ImportKind::Static);
        imp.line = Some(offset_to_line(
            &production_content,
            caps.get(0).map(|m| m.start()).unwrap_or(0),
        ));

        // Track crate-internal import patterns for dead code detection
        imp.raw_path = source.to_string();
        imp.is_crate_relative = source.starts_with("crate::");
        imp.is_super_relative = source.starts_with("super::");
        imp.is_self_relative = source.starts_with("self::");

        // Parse symbols from use statements like `use foo::{Bar, Baz}`
        if source.contains('{') && source.contains('}') {
            let mut parts = source.splitn(2, '{');
            let prefix = parts.next().unwrap_or("").trim().trim_end_matches("::");
            let braces = parts.next().unwrap_or("").trim_end_matches('}').trim();
            let names = parse_rust_brace_names(braces);
            for (original, exported) in names {
                imp.symbols.push(crate::types::ImportSymbol {
                    name: original.clone(),
                    alias: if original != exported {
                        Some(exported)
                    } else {
                        None
                    },
                    is_default: false,
                });
            }
            // Set source to the prefix for better matching
            imp.source = prefix.to_string();
        } else {
            // Single import like `use foo::Bar` or `use foo::*`
            if let Some(last_segment) = source.rsplit("::").next() {
                let last = last_segment.trim();
                if last == "*" {
                    // Star import - add "*" as symbol to trigger star_used check
                    imp.symbols.push(crate::types::ImportSymbol {
                        name: "*".to_string(),
                        alias: None,
                        is_default: false,
                    });
                    // Also set source to the prefix path
                    if let Some(prefix) = source.rsplit_once("::") {
                        imp.source = prefix.0.to_string();
                    }
                } else if !last.is_empty() && last != "self" {
                    imp.symbols.push(crate::types::ImportSymbol {
                        name: last.to_string(),
                        alias: None,
                        is_default: false,
                    });
                }
            }
        }

        analysis.imports.push(imp);
    }

    for caps in regex_rust_pub_use().captures_iter(content) {
        let raw = caps.get(1).map(|m| m.as_str()).unwrap_or("").trim();
        if raw.is_empty() {
            continue;
        }

        if raw.contains('{') && raw.contains('}') {
            let mut parts = raw.splitn(2, '{');
            let _prefix = parts.next().unwrap_or("").trim().trim_end_matches("::");
            let braces = parts.next().unwrap_or("").trim_end_matches('}').trim();
            let names = parse_rust_brace_names(braces);
            analysis.reexports.push(ReexportEntry {
                source: raw.to_string(),
                kind: ReexportKind::Named(names.clone()),
                resolved: None,
            });
            for (_, exported) in names {
                analysis
                    .exports
                    .push(ExportSymbol::new(exported, "reexport", "named", None));
            }
        } else if raw.ends_with("::*") {
            analysis.reexports.push(ReexportEntry {
                source: raw.to_string(),
                kind: ReexportKind::Star,
                resolved: None,
            });
        } else {
            // pub use foo::bar as Baz;
            let (path_part, original_name, export_name) =
                if let Some((path, alias)) = raw.split_once(" as ") {
                    // Extract original name from path (last segment)
                    let orig = path.trim().rsplit("::").next().unwrap_or(path.trim());
                    (path.trim(), orig, alias.trim())
                } else {
                    let mut segments = raw.rsplitn(2, "::");
                    let name = segments.next().unwrap_or(raw).trim();
                    let _ = segments.next();
                    (raw, name, name) // No alias - same name
                };

            analysis.reexports.push(ReexportEntry {
                source: path_part.to_string(),
                kind: ReexportKind::Named(vec![(
                    original_name.to_string(),
                    export_name.to_string(),
                )]),
                resolved: None,
            });
            analysis.exports.push(ExportSymbol::new(
                export_name.to_string(),
                "reexport",
                "named",
                None,
            ));
        }
    }

    // W1.1 fix (cross-module `use crate::...` + inside-fn local uses): explicitly
    // harvest `use` statements from the *full original* content (not production_content).
    // This guarantees fn-body uses like `use crate::watch::{WatchConfig, watch_and_rescan};`
    // inside run_watch_with_lock create an ImportEntry. Combined with 0ab00158
    // CrateModuleMap resolution + our strip pass-through, this emits the true
    // consumer edges so impact/slice/who-imports report the real importer set
    // (e.g. the CLI handler for watch.rs). Dupe guard prevents double-count.
    // See loctree-feedback.md:2900,3144 and AGENTS "fix the ENGINE".
    for caps in regex_rust_use().captures_iter(content) {
        let source = caps
            .get(1)
            .map(|m| m.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if source.is_empty() {
            continue;
        }
        if analysis
            .imports
            .iter()
            .any(|e| e.source == source || e.raw_path == source)
        {
            continue;
        }
        let mut imp = ImportEntry::new(source.clone(), ImportKind::Static);
        imp.line = Some(offset_to_line(
            content,
            caps.get(0).map(|m| m.start()).unwrap_or(0),
        ));
        imp.raw_path = source.clone();
        imp.is_crate_relative = source.starts_with("crate::");
        imp.is_super_relative = source.starts_with("super::");
        imp.is_self_relative = source.starts_with("self::");
        // (symbols parsing elided in harvest; sufficient for consumer wiring)
        analysis.imports.push(imp);
    }

    // Parse `mod foo;` declarations as imports
    // This creates a dependency edge from the declaring file to the module file
    for caps in regex_rust_mod_decl().captures_iter(&production_content) {
        if let Some(mod_name) = caps.get(2) {
            let mod_name = mod_name.as_str();

            // Check for #[path = "..."] attribute (group 1)
            let custom_path = caps.get(1).map(|m| m.as_str().to_string());

            // Create import source in format: mod::name or mod::path::name for #[path]
            let source = if let Some(path) = &custom_path {
                // #[path = "foo.rs"] mod bar; -> mod::path:foo.rs
                format!("mod::path:{}", path)
            } else {
                // Regular mod foo; -> mod::foo
                format!("mod::{}", mod_name)
            };

            let mut imp = ImportEntry::new(source.clone(), ImportKind::Static);
            imp.raw_path = source;
            imp.is_crate_relative = false;
            imp.is_super_relative = false;
            imp.is_self_relative = false;
            // Mark as mod declaration - this is NOT an import edge for cycle detection
            imp.is_mod_declaration = true;

            // Add the module name as an imported symbol
            imp.symbols.push(crate::types::ImportSymbol {
                name: mod_name.to_string(),
                alias: None,
                is_default: false,
            });

            analysis.imports.push(imp);
        }
    }

    // public items - process with proper kind detection
    // rust_pub_decl_regexes() returns [fn, struct, enum, trait, type, union] in order
    let kinds = ["function", "struct", "enum", "trait", "type", "union"];
    for (regex, kind) in rust_pub_decl_regexes().iter().zip(kinds.iter()) {
        for caps in regex.captures_iter(content) {
            if let Some(name) = caps.get(1) {
                let line = offset_to_line(content, name.start());
                let name_str = name.as_str().to_string();

                // Extract params only for functions
                let params = if *kind == "function" {
                    extract_rust_fn_params(content, name.end())
                } else {
                    Vec::new()
                };

                analysis.exports.push(ExportSymbol::with_params(
                    name_str,
                    kind,
                    "named",
                    Some(line),
                    params,
                ));
            }
        }
    }

    for regex in rust_pub_const_regexes() {
        for caps in regex.captures_iter(content) {
            if let Some(name) = caps.get(1) {
                let line = offset_to_line(content, name.start());
                analysis.exports.push(ExportSymbol::new(
                    name.as_str().to_string(),
                    "decl",
                    "named",
                    Some(line),
                ));
            }
        }
    }

    let exported_names: HashSet<String> = analysis.exports.iter().map(|e| e.name.clone()).collect();
    let locals = collect_rust_local_symbols(content, &exported_names);
    if !locals.is_empty() {
        analysis.local_symbols = locals;
    }

    analysis.impl_methods = collect_impl_methods(content);
    analysis.log_messages = collect_rust_log_messages(content);

    collect_rust_signature_uses(&production_content, &mut analysis);

    for caps in regex_event_const_rust().captures_iter(content) {
        if let (Some(name), Some(val)) = (caps.get(1), caps.get(2)) {
            analysis
                .event_consts
                .insert(name.as_str().to_string(), val.as_str().to_string());
        }
    }

    // Check if a token looks like a valid Tauri event name (not a Rust literal/keyword).
    // Returns true if the token should be filtered out (is NOT a valid event name).
    let is_invalid_event_identifier = |token: &str| -> bool {
        // Filter out Rust keywords and common literals
        const RUST_KEYWORDS: &[&str] = &[
            "true", "false", "None", "Some", "Ok", "Err", "self", "Self", "super", "crate",
        ];

        if RUST_KEYWORDS.contains(&token) {
            return true;
        }

        // Filter out tokens that look like module paths (contain ::)
        // These are likely enum variants or associated items, not event names
        if token.contains("::") {
            return true;
        }

        // Filter out single lowercase words that look like crate/module names
        // Valid event names typically use kebab-case, snake_case with underscores,
        // or have mixed case. Single lowercase words without separators are
        // more likely to be crate names (e.g., "gix", "tokio", "serde")
        if token.chars().all(|c| c.is_ascii_lowercase()) && token.len() <= 8 {
            return true;
        }

        // Filter out PascalCase identifiers without underscores or hyphens
        // These are likely type names (Mode, AppState, etc.) not event names.
        // Event names typically use kebab-case, snake_case, or SCREAMING_SNAKE_CASE.
        // A single PascalCase word is almost never an event name.
        if let Some(first) = token.chars().next()
            && first.is_ascii_uppercase()
        {
            // Check if it's a simple PascalCase identifier (no underscores/hyphens)
            let has_separator = token.contains('_') || token.contains('-');
            let is_all_caps = token.chars().all(|c| !c.is_ascii_lowercase());

            // Filter out if it's PascalCase without separators and not all caps
            if !has_separator && !is_all_caps {
                return true;
            }
        }

        false
    };

    let resolve_event = |token: &str| -> Option<(String, Option<String>, String, bool)> {
        let trimmed = token.trim();

        // Detect format! pattern - e.g., format!("event:{}", var) or &format!(...)
        if trimmed.contains("format!") {
            // Extract the format string pattern
            if let Some(start) = trimmed.find("format!(\"") {
                let after_paren = &trimmed[start + 9..]; // Skip 'format!("'
                if let Some(end) = after_paren.find('"') {
                    let pattern = &after_paren[..end];
                    // Replace {} placeholders with * for pattern matching
                    let normalized = pattern.replace("{}", "*").replace("{:?}", "*");
                    return Some((
                        normalized.clone(),
                        Some(format!("format!(\"{}\")", pattern)),
                        "dynamic".to_string(),
                        true, // is_dynamic
                    ));
                }
            }
            // Fallback for complex format patterns
            return Some((
                "dynamic-event:*".to_string(),
                Some(trimmed.to_string()),
                "dynamic".to_string(),
                true,
            ));
        }

        // String literals are always valid event names
        if (trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        {
            let name = trimmed
                .trim_start_matches(['"', '\''])
                .trim_end_matches(['"', '\''])
                .to_string();
            return Some((
                name,
                Some(trimmed.to_string()),
                "literal".to_string(),
                false,
            ));
        }

        // Check if it's a known const
        if let Some(val) = analysis.event_consts.get(trimmed) {
            return Some((
                val.clone(),
                Some(trimmed.to_string()),
                "const".to_string(),
                false,
            ));
        }

        // For identifiers, apply filtering
        if is_invalid_event_identifier(trimmed) {
            return None;
        }

        Some((
            trimmed.to_string(),
            Some(trimmed.to_string()),
            "ident".to_string(),
            false,
        ))
    };

    for caps in regex_event_emit_rust().captures_iter(content) {
        if let Some(target) = caps.name("target") {
            // Skip if resolve_event filters out this identifier
            if let Some((name, raw_name, source_kind, is_dynamic)) = resolve_event(target.as_str())
            {
                let payload = caps
                    .name("payload")
                    .map(|p| p.as_str().trim().trim_end_matches(')').trim().to_string())
                    .filter(|s| !s.is_empty());
                let line = offset_to_line(content, caps.get(0).map(|m| m.start()).unwrap_or(0));
                event_emits.push(EventRef {
                    raw_name,
                    name,
                    line,
                    kind: format!("emit_{}", source_kind),
                    awaited: false,
                    payload,
                    is_dynamic,
                });
            }
        }
    }
    for caps in regex_event_listen_rust().captures_iter(content) {
        if let Some(target) = caps.name("target") {
            // Skip if resolve_event filters out this identifier
            if let Some((name, raw_name, source_kind, is_dynamic)) = resolve_event(target.as_str())
            {
                let line = offset_to_line(content, caps.get(0).map(|m| m.start()).unwrap_or(0));
                event_listens.push(EventRef {
                    raw_name,
                    name,
                    line,
                    kind: format!("listen_{}", source_kind),
                    awaited: false,
                    payload: None,
                    is_dynamic,
                });
            }
        }
    }

    for caps in regex_tauri_command_fn().captures_iter(content) {
        let attr_raw = caps.get(1).map(|m| m.as_str()).unwrap_or("").trim();
        let name_match = caps.get(2);
        let params = caps
            .name("params")
            .map(|p| p.as_str().trim().to_string())
            .filter(|s| !s.is_empty());

        if let Some(name) = name_match {
            let fn_name = name.as_str().to_string();
            let base_exposed_name = exposed_command_name(attr_raw, &fn_name);
            // Check if this is a plugin command (has root = "crate" attribute)
            let is_plugin_command = extract_plugin_name(attr_raw).is_some();

            // exposed_name is just the command name (without plugin prefix)
            // The plugin namespace is stored separately in plugin_name field
            // This matches frontend behavior: invoke('plugin:window|cmd') parses to name="cmd", plugin_name="window"
            let exposed_name = base_exposed_name;

            let line = offset_to_line(content, name.start());
            analysis.command_handlers.push(CommandRef {
                name: fn_name,
                exposed_name: Some(exposed_name),
                line,
                generic_type: None,
                payload: params,
                plugin_name: if is_plugin_command {
                    plugin_identifier.clone()
                } else {
                    None
                },
            });
        }
    }

    // Custom command macros (from .loctree/config.toml)
    if let Some(custom_regex) = regex_custom_command_fn(custom_command_macros) {
        for caps in custom_regex.captures_iter(content) {
            let attr_raw = caps.get(1).map(|m| m.as_str()).unwrap_or("").trim();
            let name_match = caps.get(2);
            let params = caps
                .name("params")
                .map(|p| p.as_str().trim().to_string())
                .filter(|s| !s.is_empty());

            if let Some(name) = name_match {
                let fn_name = name.as_str().to_string();
                // Avoid duplicates if both #[tauri::command] and custom macro are used
                if analysis.command_handlers.iter().any(|c| c.name == fn_name) {
                    continue;
                }
                let base_exposed_name = exposed_command_name(attr_raw, &fn_name);
                let is_plugin_command = extract_plugin_name(attr_raw).is_some();

                // exposed_name is just the command name (without plugin prefix)
                // The plugin namespace is stored separately in plugin_name field
                let exposed_name = base_exposed_name;

                let line = offset_to_line(content, name.start());
                analysis.command_handlers.push(CommandRef {
                    name: fn_name,
                    exposed_name: Some(exposed_name),
                    line,
                    generic_type: None,
                    payload: params,
                    plugin_name: if is_plugin_command {
                        plugin_identifier.clone()
                    } else {
                        None
                    },
                });
            }
        }
    }

    // Tauri generate_handler! registrations
    // The generate_handler! macro may span multiple lines and contain #[cfg(...)] attributes.
    // We need to handle nested brackets by finding balanced pairs.
    for caps in regex_tauri_generate_handler().captures_iter(content) {
        if let Some(list_match) = caps.get(1) {
            let start_pos = list_match.start();
            // Find the actual end by matching balanced brackets from the start
            let remaining = &content[start_pos..];
            let balanced_end = find_balanced_bracket(remaining);
            let raw = if balanced_end > 0 {
                &remaining[..balanced_end]
            } else {
                list_match.as_str()
            };
            // Strip #[...] attributes from the handler list
            let cleaned = strip_cfg_attributes(raw);
            for part in cleaned.split(',') {
                let ident = part.trim();
                if ident.is_empty() {
                    continue;
                }
                // Strip potential trailing generics or module qualifiers (foo::<T>, module::foo)
                // Use .last() to get the function name from paths like commands::foo::bar
                let base = ident
                    .split(|c: char| c == ':' || c.is_whitespace() || c == '<')
                    .rfind(|s| !s.is_empty())
                    .unwrap_or("")
                    .trim();
                if base.is_empty() {
                    continue;
                }
                // Basic Rust identifier check: starts with letter or '_', rest alphanumeric or '_'
                let mut chars = base.chars();
                if let Some(first) = chars.next() {
                    if !(first.is_ascii_alphabetic() || first == '_') {
                        continue;
                    }
                    if chars.any(|ch| !(ch.is_ascii_alphanumeric() || ch == '_')) {
                        continue;
                    }
                    if !analysis
                        .tauri_registered_handlers
                        .contains(&base.to_string())
                    {
                        analysis.tauri_registered_handlers.push(base.to_string());
                    }
                }
            }
        }
    }

    analysis.event_emits = event_emits;
    analysis.event_listens = event_listens;

    // Detect Rust entry points using proper regex (not contains - avoids false positives in comments/strings)
    if regex_rust_fn_main().is_match(content) {
        analysis.entry_points.push("main".to_string());
    }
    if regex_rust_async_main_attr().is_match(content)
        && !analysis.entry_points.contains(&"async_main".to_string())
    {
        analysis.entry_points.push("async_main".to_string());
    }

    // Detect path-qualified calls like `module::function()` or `Type::method()`
    // These are function calls via module path without explicit `use` import.
    // Pattern: `::<identifier>(` or `::<Identifier>{` or `::<Identifier><`
    // This catches: command::branch::handle(), OutputChannel::new(), etc.
    extract_path_qualified_calls(&production_content, &mut analysis.local_uses);

    // Detect type alias qualified paths like `io::Result`, `fs::File`, etc.
    // This handles cases where a module is imported but types from that module
    // are used via qualified paths (e.g., `use std::io; fn foo() -> io::Result<()>`)
    // This reduces false positives by ~15% for Rust codebases
    extract_type_alias_qualified_paths(content, &analysis.imports, &mut analysis.local_uses);

    // Detect bare function calls like `func_name(...)` in the same file
    // This catches local function calls without path qualification
    extract_bare_function_calls(&production_content, &mut analysis.local_uses);

    // Detect type names used in struct/enum field definitions
    // This catches types like Vec<DiffEdge>, Option<HubFile>, etc. that are used
    // as field types within the same file - they count as "local uses" of those types
    extract_struct_field_types(content, &mut analysis.local_uses);

    // Detect identifiers used in expressions and variable declarations
    // This catches const/static usage like `create_buffer::<BUFFER_SIZE>()`
    // and type usage in let bindings like `let x: SomeType = ...`
    // NOTE: Use full `content` here, not `production_content`, because we need to
    // scan function bodies for usages of exported symbols (constants, types, etc.)
    extract_identifier_usages(content, &mut analysis.local_uses);

    // Detect identifiers used as function arguments like `func(CONST_NAME)`
    // This catches const/static usage passed as arguments to functions
    extract_function_arguments(content, &mut analysis.local_uses);

    // Fallback: treat any identifier mention (excluding keywords) as a local use.
    // This plugs gaps where complex patterns (const tables, enum variants, nested types)
    // might not be caught by the structured extractors above.
    collect_identifier_mentions(content, &mut analysis.local_uses);

    // Remove standard library/common types from local uses to avoid false positives
    // in same-file usage checks.
    const SKIP_STD_TYPES: &[&str] = &[
        "Vec", "Option", "Result", "String", "HashMap", "Box", "Arc", "Rc",
    ];
    analysis
        .local_uses
        .retain(|u| !SKIP_STD_TYPES.contains(&u.as_str()));

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

    #[test]
    fn impl_method_extraction_records_inherent_trait_async_and_visibility() {
        let content = r#"
pub struct Worker;
impl Worker {
    pub async fn run(&self) {}
    pub(crate) fn helper(&self) {}
    fn private(&self) {}
}

impl std::fmt::Display for Worker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}
"#;
        let analysis = analyze_rust_file(content, "src/lib.rs".to_string(), &[]);
        let run = analysis
            .impl_methods
            .iter()
            .find(|method| method.name == "run" && method.qualifier == "Worker")
            .expect("run method");
        assert!(run.is_async);
        assert!(matches!(run.visibility, crate::types::Visibility::Public));
        assert_eq!(run.line, Some(4));
        assert!(
            analysis
                .impl_methods
                .iter()
                .any(|method| method.name == "helper"
                    && matches!(method.visibility, crate::types::Visibility::Crate))
        );
        assert!(
            analysis
                .impl_methods
                .iter()
                .any(|method| method.name == "fmt"
                    && method.line == Some(10)
                    && method.trait_qualifier.as_deref() == Some("std::fmt::Display"))
        );
    }

    #[test]
    fn log_macro_extraction_records_levels_and_function_context() {
        let content = r#"
fn run() {
    tracing::info!("starting {}", 1);
    warn!(target: "loctree", "careful");
    eprintln!("bad");
    panic!("boom");
}
"#;
        let analysis = analyze_rust_file(content, "src/lib.rs".to_string(), &[]);
        assert!(
            analysis
                .log_messages
                .iter()
                .any(|msg| msg.format_string == "starting {}"
                    && matches!(msg.level, crate::types::LogLevel::Info)
                    && msg.function_context.as_deref() == Some("run"))
        );
        assert!(
            analysis
                .log_messages
                .iter()
                .any(|msg| msg.format_string == "careful"
                    && matches!(msg.level, crate::types::LogLevel::Warn))
        );
        assert!(
            analysis
                .log_messages
                .iter()
                .any(|msg| msg.format_string == "boom"
                    && matches!(msg.level, crate::types::LogLevel::Panic))
        );
    }

    #[test]
    fn rust_local_symbols_and_usages() {
        let content = r#"
fn helper() {}

struct LocalType;

pub fn public_fn() {
    helper();
}

fn call_local() {
    helper();
    let _x = LocalType;
}
"#;

        let analysis = analyze_rust_file(content, "sample.rs".to_string(), &[]);
        assert!(
            analysis.local_symbols.iter().any(|s| s.name == "helper"),
            "helper should be in local_symbols"
        );
        assert!(
            analysis.local_symbols.iter().any(|s| s.name == "LocalType"),
            "LocalType should be in local_symbols"
        );
        assert!(
            !analysis.local_symbols.iter().any(|s| s.name == "public_fn"),
            "public_fn should be exported, not local"
        );
        assert!(
            analysis.symbol_usages.iter().any(|u| u.name == "helper"),
            "helper should appear in symbol_usages"
        );
    }

    #[test]
    fn rust_local_fn_modifiers_resolve_to_local_symbols() {
        // Regression: private fns carrying leading modifiers must land in
        // local_symbols so `where-symbol`/`body` can resolve them. The buggy
        // prefix `(?:async|const|unsafe\s+)*` only consumed trailing whitespace
        // after `unsafe`, so `async fn`/`const fn` (and combos) fell through and
        // became invisible to the structural resolver while staying visible to
        // the literal scanner. `extern "C" fn` had no branch at all.
        let content = r#"
async fn run_agent_send_with_fallback() {}

const fn const_helper() -> u8 { 0 }

extern "C" fn extern_helper() {}

async unsafe fn async_unsafe_helper() {}

unsafe fn unsafe_helper() {}
"#;

        let analysis = analyze_rust_file(content, "sample.rs".to_string(), &[]);
        for name in [
            "run_agent_send_with_fallback",
            "const_helper",
            "extern_helper",
            "async_unsafe_helper",
            "unsafe_helper",
        ] {
            assert!(
                analysis.local_symbols.iter().any(|s| s.name == name),
                "{name} should be in local_symbols"
            );
        }
    }

    #[test]
    fn rust_pub_fn_modifiers_resolve_to_exports() {
        // Regression (export-side twin of bc31b072): a `pub` fn carrying leading
        // modifiers must land in `exports`, not leak into `local_symbols` where it
        // would be mislabeled private. The `pub` fn regex's modifier group
        // `(?:(?:async|const|unsafe)\s+)*` had no `extern` branch, so
        // `pub extern "C" fn` (an FFI export whose consumers loctree can never see)
        // fell through to local_symbols and became a prime false-positive dead-code
        // candidate. `async`/`const`/`unsafe` were already covered; this locks the
        // full matrix end-to-end.
        let content = r#"
pub async fn pub_async_export() {}

pub const fn pub_const_export() -> u8 { 0 }

pub unsafe fn pub_unsafe_export() {}

pub extern "C" fn pub_extern_export() {}

pub extern fn pub_extern_default_abi() {}

pub fn pub_plain_export() {}
"#;

        let analysis = analyze_rust_file(content, "pubmods.rs".to_string(), &[]);
        for name in [
            "pub_async_export",
            "pub_const_export",
            "pub_unsafe_export",
            "pub_extern_export",
            "pub_extern_default_abi",
            "pub_plain_export",
        ] {
            assert!(
                analysis.exports.iter().any(|e| e.name == name),
                "{name} should be in exports, not leaked to local_symbols"
            );
            assert!(
                !analysis.local_symbols.iter().any(|s| s.name == name),
                "{name} is a pub export and must not appear in local_symbols"
            );
        }
    }
}
