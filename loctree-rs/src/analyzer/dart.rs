use regex::Regex;

use crate::types::{
    ExportSymbol, FileAnalysis, ImportEntry, ImportKind, ReexportEntry, ReexportKind,
};

fn extract_string_literal(line: &str) -> Option<String> {
    let mut quote = None;
    for ch in ['\'', '"'] {
        if let Some(pos) = line.find(ch) {
            quote = Some((ch, pos));
            break;
        }
    }
    if let Some((delim, start)) = quote
        && let Some(end) = line[start + 1..].find(delim)
    {
        return Some(line[start + 1..start + 1 + end].to_string());
    }
    None
}

fn is_ident(token: &str) -> bool {
    let mut chars = token.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_top_level(line: &str) -> bool {
    !line.starts_with(char::is_whitespace)
}

fn parse_named_after_keyword(trimmed: &str, keyword: &str) -> Option<String> {
    let rest = trimmed.strip_prefix(keyword)?.trim_start();
    rest.split_whitespace().next().map(str::to_string)
}

fn parse_const_like_name(trimmed: &str) -> Option<String> {
    // Handles: const foo = 1;  | const int foo = 1; | final _token = ...;
    let tokens: Vec<&str> = trimmed
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.len() < 2 {
        return None;
    }
    let eq_idx = tokens
        .iter()
        .position(|t| *t == "=" || t.ends_with('='))
        .unwrap_or(tokens.len().saturating_sub(1));
    if eq_idx == 0 {
        return None;
    }
    let candidate = tokens[eq_idx.saturating_sub(1)]
        .trim_end_matches(';')
        .trim_end_matches(',');
    if is_ident(candidate) {
        Some(candidate.to_string())
    } else {
        None
    }
}

fn parse_function_name(line: &str) -> Option<String> {
    if !is_top_level(line) {
        return None;
    }
    if !line.contains('(') {
        return None;
    }
    let trimmed = line.trim_start();
    // Skip common declaration starters
    if trimmed.starts_with("if ")
        || trimmed.starts_with("for ")
        || trimmed.starts_with("while ")
        || trimmed.starts_with("switch ")
        || trimmed.starts_with("class ")
    {
        return None;
    }

    let before_paren = trimmed.split('(').next().unwrap_or("").trim_end();
    let tokens: Vec<&str> = before_paren.split_whitespace().collect();
    let name = tokens.last().unwrap_or(&"").trim_end_matches(':'); // named constructors use :
    if is_ident(name) {
        Some(name.to_string())
    } else {
        None
    }
}

fn collect_local_uses(content: &str) -> Vec<String> {
    const KEYWORDS: &[&str] = &[
        "abstract",
        "as",
        "assert",
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "covariant",
        "default",
        "deferred",
        "do",
        "dynamic",
        "else",
        "enum",
        "export",
        "extends",
        "extension",
        "external",
        "factory",
        "false",
        "final",
        "finally",
        "for",
        "Function",
        "get",
        "hide",
        "if",
        "implements",
        "import",
        "in",
        "interface",
        "is",
        "late",
        "library",
        "mixin",
        "new",
        "null",
        "on",
        "operator",
        "part",
        "rethrow",
        "return",
        "set",
        "show",
        "static",
        "super",
        "switch",
        "sync",
        "this",
        "throw",
        "true",
        "try",
        "typedef",
        "var",
        "void",
        "while",
        "with",
        "yield",
    ];
    let ident_re = Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").expect("valid dart ident regex");
    let mut uses: Vec<String> = Vec::new();
    for cap in ident_re.captures_iter(content) {
        let ident = cap.get(0).map(|m| m.as_str()).unwrap_or_default();
        if KEYWORDS.contains(&ident) {
            continue;
        }
        if !uses.contains(&ident.to_string()) {
            uses.push(ident.to_string());
        }
    }
    uses
}

fn parse_exports(content: &str) -> Vec<ExportSymbol> {
    let mut exports = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }

        if let Some(name) = parse_named_after_keyword(trimmed, "class ").filter(|n| is_ident(n)) {
            exports.push(ExportSymbol::new(name, "class", "named", Some(idx + 1)));
            continue;
        }

        if let Some(name) = parse_named_after_keyword(trimmed, "enum ").filter(|n| is_ident(n)) {
            exports.push(ExportSymbol::new(name, "enum", "named", Some(idx + 1)));
            continue;
        }

        if let Some(name) = parse_named_after_keyword(trimmed, "mixin ").filter(|n| is_ident(n)) {
            exports.push(ExportSymbol::new(name, "mixin", "named", Some(idx + 1)));
            continue;
        }

        if let Some(name) = parse_named_after_keyword(trimmed, "typedef ").filter(|n| is_ident(n)) {
            exports.push(ExportSymbol::new(name, "typedef", "named", Some(idx + 1)));
            continue;
        }

        if let Some(name) = parse_named_after_keyword(trimmed, "extension ").filter(|n| is_ident(n))
        {
            exports.push(ExportSymbol::new(name, "extension", "named", Some(idx + 1)));
            continue;
        }

        if is_top_level(line) && trimmed.starts_with("const ") {
            if let Some(name) = parse_const_like_name(trimmed) {
                exports.push(ExportSymbol::new(name, "const", "named", Some(idx + 1)));
            }
            continue;
        }

        if is_top_level(line) && trimmed.starts_with("final ") {
            if let Some(name) = parse_const_like_name(trimmed) {
                exports.push(ExportSymbol::new(name, "var", "named", Some(idx + 1)));
            }
            continue;
        }

        if let Some(name) = parse_function_name(line) {
            exports.push(ExportSymbol::new(name, "function", "named", Some(idx + 1)));
        }
    }

    exports
}

/// Dart analyzer: collects imports/re-exports and top-level declarations for dead-export and graph usage.
pub(crate) fn analyze_dart_file(content: &str, relative: String) -> FileAnalysis {
    let mut analysis = FileAnalysis::new(relative);

    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }

        if trimmed.starts_with("import ") || trimmed.starts_with("part ") {
            if let Some(source) = extract_string_literal(trimmed) {
                analysis
                    .imports
                    .push(ImportEntry::new(source, ImportKind::Static));
            }
            continue;
        }

        if trimmed.starts_with("export ")
            && let Some(source) = extract_string_literal(trimmed)
        {
            analysis.reexports.push(ReexportEntry {
                source,
                kind: ReexportKind::Star,
                resolved: None,
            });
        }
    }

    analysis.exports = parse_exports(content);
    analysis.local_uses = collect_local_uses(content);

    analysis
}

#[cfg(test)]
mod tests {
    use super::analyze_dart_file;

    #[test]
    fn parses_imports_and_exports() {
        let content = r#"
import 'package:flutter/material.dart';
import './widgets/button.dart';
export 'src/api.dart';
part 'src/state.dart';
// comment import 'ignored.dart';
        "#;
        let analysis = analyze_dart_file(content, "lib/main.dart".to_string());

        let sources: Vec<_> = analysis.imports.iter().map(|i| i.source.clone()).collect();
        assert!(sources.contains(&"package:flutter/material.dart".to_string()));
        assert!(sources.contains(&"./widgets/button.dart".to_string()));
        assert!(sources.contains(&"src/state.dart".to_string()));

        let exports: Vec<_> = analysis
            .reexports
            .iter()
            .map(|e| e.source.clone())
            .collect();
        assert_eq!(exports, vec!["src/api.dart".to_string()]);
    }
}
