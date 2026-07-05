use regex::Regex;

use crate::types::{ExportSymbol, FileAnalysis, ImportEntry, ImportKind};

/// Analyze a Go source file for imports/exports with lightweight heuristics.
/// This is intentionally simple (no full parser) but good enough for dead-export
/// and dependency tracking without skipping Go projects entirely.
pub fn analyze_go_file(content: &str, relative: String) -> FileAnalysis {
    let mut analysis = FileAnalysis::new(relative);

    analysis.imports = parse_imports(content);
    analysis.exports = parse_exports(content);
    analysis.local_uses = collect_local_uses(content);

    // Go entrypoints are discovered at runtime; treat main as a special entry so
    // we do not flag it as unused in package main.
    if content.contains("\nfunc main(") {
        analysis.local_uses.push("main".to_string());
    }

    analysis
}

fn parse_imports(content: &str) -> Vec<ImportEntry> {
    let mut imports: Vec<ImportEntry> = Vec::new();
    let mut in_block = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("import (") || trimmed == "import(" || trimmed == "import (" {
            in_block = true;
            continue;
        }

        if in_block {
            if trimmed.starts_with(')') {
                in_block = false;
                continue;
            }
            if let Some(path) = extract_import_path(trimmed) {
                push_import(&mut imports, path);
            }
            continue;
        }

        if trimmed.starts_with("import ")
            && let Some(path) = extract_import_path(trimmed.trim_start_matches("import").trim())
        {
            push_import(&mut imports, path);
        }
    }

    imports
}

fn extract_import_path(segment: &str) -> Option<String> {
    // Accept both single and double quoted imports: "pkg/path" or `pkg/path`
    let quote_start = segment.find(&['"', '`'][..])?;
    let quote = segment.as_bytes()[quote_start];
    let tail = &segment[quote_start + 1..];
    let path_end = tail.find(quote as char)?;
    let path = tail[..path_end].trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

fn push_import(imports: &mut Vec<ImportEntry>, path: String) {
    if imports.iter().any(|i| i.source == path) {
        return;
    }
    let mut entry = ImportEntry::new(path, ImportKind::Static);
    entry.resolution = crate::types::ImportResolutionKind::Unknown;
    imports.push(entry);
}

fn parse_exports(content: &str) -> Vec<ExportSymbol> {
    let mut exports = Vec::new();
    let mut const_block = false;
    let mut var_block = false;

    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();

        // func (receiver) Name(...)
        if let Some(name) = parse_func_name(trimmed) {
            if is_exported(&name) {
                exports.push(ExportSymbol::new(name, "function", "named", Some(idx + 1)));
            }
            continue;
        }

        // type Name struct/interface/alias
        if trimmed.starts_with("type ")
            && let Some(name) = trimmed
                .strip_prefix("type ")
                .and_then(|rest| rest.split_whitespace().next())
                .map(str::to_string)
            && is_exported(&name)
        {
            exports.push(ExportSymbol::new(name, "type", "named", Some(idx + 1)));
            continue;
        }

        if trimmed.starts_with("const (") {
            const_block = true;
            continue;
        }
        if trimmed.starts_with("var (") {
            var_block = true;
            continue;
        }
        if const_block && trimmed.starts_with(')') {
            const_block = false;
            continue;
        }
        if var_block && trimmed.starts_with(')') {
            var_block = false;
            continue;
        }

        if const_block {
            for name in parse_const_var_names(trimmed) {
                if is_exported(&name) {
                    exports.push(ExportSymbol::new(name, "const", "named", Some(idx + 1)));
                }
            }
            continue;
        }

        if var_block {
            for name in parse_const_var_names(trimmed) {
                if is_exported(&name) {
                    exports.push(ExportSymbol::new(name, "var", "named", Some(idx + 1)));
                }
            }
            continue;
        }

        if trimmed.starts_with("const ") {
            for name in parse_const_var_names(trimmed.trim_start_matches("const ").trim()) {
                if is_exported(&name) {
                    exports.push(ExportSymbol::new(name, "const", "named", Some(idx + 1)));
                }
            }
            continue;
        }

        if trimmed.starts_with("var ") {
            for name in parse_const_var_names(trimmed.trim_start_matches("var ").trim()) {
                if is_exported(&name) {
                    exports.push(ExportSymbol::new(name, "var", "named", Some(idx + 1)));
                }
            }
        }
    }

    exports
}

fn parse_func_name(line: &str) -> Option<String> {
    if !line.starts_with("func ") {
        return None;
    }
    let after = line.trim_start_matches("func ").trim_start();
    let without_receiver = if after.starts_with('(') {
        after.split_once(')')?.1.trim_start()
    } else {
        after
    };
    without_receiver
        .split(|c: char| c.is_whitespace() || c == '(')
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

fn parse_const_var_names(segment: &str) -> Vec<String> {
    // Handle "Foo = 1" or "Foo, Bar = ..." inside const/var blocks
    let lhs = segment.split('=').next().unwrap_or(segment);
    lhs.split(',')
        .map(|part| part.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn is_exported(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

fn collect_local_uses(content: &str) -> Vec<String> {
    const KEYWORDS: &[&str] = &[
        "break",
        "case",
        "chan",
        "const",
        "continue",
        "default",
        "defer",
        "else",
        "fallthrough",
        "for",
        "func",
        "go",
        "goto",
        "if",
        "import",
        "interface",
        "map",
        "package",
        "range",
        "return",
        "select",
        "struct",
        "switch",
        "type",
        "var",
        "true",
        "false",
        "nil",
        "iota",
    ];

    let ident_re = Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").expect("valid go ident regex");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_go_imports_exports_and_uses() {
        let src = r#"
package main

import (
    "fmt"
    util "example.com/util"
    _ "net/http/pprof"
)

const (
    Version = "1.0"
    localConst = "x"
)

var (
    Exported = 1
    internal = 0
)

type Server struct{}
func (s *Server) Serve() {}
func helper() {}
func Public() {}

func main() {
    Server{}
    Public()
    fmt.Println(util.Foo())
}
"#;

        let analysis = analyze_go_file(src, "app/main.go".to_string());
        let imports: Vec<_> = analysis.imports.iter().map(|i| i.source.clone()).collect();
        assert!(imports.contains(&"fmt".to_string()));
        assert!(imports.contains(&"example.com/util".to_string()));
        assert!(imports.contains(&"net/http/pprof".to_string()));

        let export_names: Vec<_> = analysis.exports.iter().map(|e| e.name.clone()).collect();
        assert!(export_names.contains(&"Version".to_string()));
        assert!(export_names.contains(&"Exported".to_string()));
        assert!(export_names.contains(&"Server".to_string()));
        assert!(export_names.contains(&"Serve".to_string()));
        assert!(export_names.contains(&"Public".to_string()));
        assert!(!export_names.contains(&"helper".to_string()));

        // local uses should capture identifiers referenced in the file
        assert!(analysis.local_uses.contains(&"Public".to_string()));
        assert!(analysis.local_uses.contains(&"Server".to_string()));
        // entrypoint should be treated as used
        assert!(analysis.local_uses.contains(&"main".to_string()));
    }
}
