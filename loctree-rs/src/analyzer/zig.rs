//! Lightweight Zig (.zig/.zon) analyzer.
//!
//! Regex-based parser that extracts public declarations and `@import("...")`
//! references. Mirrors the minimal-viable shape of `analyzer/go.rs`.

use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;

use crate::types::{ExportSymbol, FileAnalysis, ImportEntry, ImportKind, ImportResolutionKind};

// Public declarations:   pub fn NAME / pub const NAME / pub var NAME
// We also catch `pub inline fn`, `pub extern fn`, `pub export fn`.
static RE_PUB_DECL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^\s*pub\s+(?:inline\s+|extern\s+|export\s+|threadlocal\s+)?(fn|const|var)\s+([A-Za-z_][A-Za-z0-9_]*)",
    )
    .expect("valid zig pub decl regex")
});

// `@import("some/path.zig")` — also matches `@import("std")`.
static RE_IMPORT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"@import\(\s*"([^"]+)"\s*\)"#).expect("valid zig import regex"));

// `test "name" { ... }` or `test name { ... }` (for metrics; we count only).
static RE_TEST: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"^\s*test\s+(?:"[^"]*"|[A-Za-z_][A-Za-z0-9_]*)\s*\{"#)
        .expect("valid zig test regex")
});

/// Analyze a Zig source file (.zig or .zon manifest).
pub fn analyze_zig_file(content: &str, relative: String) -> FileAnalysis {
    let mut analysis = FileAnalysis::new(relative);
    analysis.imports = parse_imports(content);
    analysis.exports = parse_exports(content);
    // Test-count tracking: we don't expose a dedicated field, but tests found
    // live as additional exports of kind `test` so dead-code passes don't flag
    // them as unused.
    push_tests(&mut analysis, content);
    analysis
}

fn parse_imports(content: &str) -> Vec<ImportEntry> {
    let mut imports: Vec<ImportEntry> = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        // Strip `//` comments (respecting string escapes minimally)
        let effective = strip_line_comment(line);
        for caps in RE_IMPORT.captures_iter(effective) {
            if let Some(m) = caps.get(1) {
                let path = m.as_str().trim();
                if path.is_empty() {
                    continue;
                }
                if imports.iter().any(|i| i.source == path) {
                    continue;
                }
                let mut entry = ImportEntry::new(path.to_string(), ImportKind::Static);
                entry.line = Some(idx + 1);
                entry.resolution =
                    if path == "std" || path == "builtin" || path == "root" || path == "@import" {
                        ImportResolutionKind::Stdlib
                    } else {
                        ImportResolutionKind::Unknown
                    };
                imports.push(entry);
            }
        }
    }
    imports
}

fn parse_exports(content: &str) -> Vec<ExportSymbol> {
    let mut out: Vec<ExportSymbol> = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let effective = strip_line_comment(line);
        if let Some(caps) = RE_PUB_DECL.captures(effective) {
            let keyword = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            let kind = match keyword {
                "fn" => "function",
                "const" => "const",
                "var" => "var",
                _ => "const",
            };
            if !out.iter().any(|e| e.name == name && e.kind == kind) {
                out.push(ExportSymbol::new(name, kind, "named", Some(idx + 1)));
            }
        }
    }
    out
}

fn push_tests(analysis: &mut FileAnalysis, content: &str) {
    let mut test_idx: usize = 0;
    for (idx, line) in content.lines().enumerate() {
        if RE_TEST.is_match(line) {
            // Synthetic unique name so dedup within `exports` doesn't collapse them.
            let name = format!("test#{}", test_idx);
            test_idx += 1;
            analysis
                .exports
                .push(ExportSymbol::new(name, "test", "named", Some(idx + 1)));
        }
    }
}

fn strip_line_comment(line: &str) -> &str {
    // Zig comments are `//` at line level; strings use `"..."`. Minimal guard.
    let mut in_str = false;
    let bytes = line.as_bytes();
    let mut idx = 0;
    while idx + 1 < bytes.len() {
        let ch = bytes[idx] as char;
        match ch {
            '\\' => {
                idx += 2;
                continue;
            }
            '"' => in_str = !in_str,
            '/' if !in_str && bytes[idx + 1] == b'/' => {
                return &line[..idx];
            }
            _ => {}
        }
        idx += 1;
    }
    line
}

/// Resolve a Zig `@import("path.zig")` relative to the file's directory.
/// `std`/`builtin`/`root` return None (stdlib).
pub fn resolve_zig_import(spec: &str, file_path: &Path, _root: &Path) -> Option<String> {
    if spec == "std" || spec == "builtin" || spec == "root" {
        return None;
    }
    let parent = file_path.parent()?;
    let candidate: PathBuf = if Path::new(spec).is_absolute() {
        PathBuf::from(spec)
    } else {
        parent.join(spec)
    };
    if candidate.exists() {
        let canon = candidate.canonicalize().unwrap_or(candidate);
        return Some(canon.to_string_lossy().to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pub_decls() {
        let src = r#"
const std = @import("std");

pub fn main() void {
    _ = std;
}

pub const VERSION = "1.0";
pub var global_counter: u32 = 0;
pub inline fn fast() u32 { return 1; }
pub extern fn c_api() c_int;

fn private_helper() void {}
"#;
        let analysis = analyze_zig_file(src, "main.zig".to_string());
        let fns: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "function")
            .map(|e| e.name.clone())
            .collect();
        assert!(fns.contains(&"main".to_string()));
        assert!(fns.contains(&"fast".to_string()));
        assert!(fns.contains(&"c_api".to_string()));
        assert!(!fns.contains(&"private_helper".to_string()));

        let consts: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "const")
            .map(|e| e.name.clone())
            .collect();
        assert!(consts.contains(&"VERSION".to_string()));

        let vars: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "var")
            .map(|e| e.name.clone())
            .collect();
        assert!(vars.contains(&"global_counter".to_string()));
    }

    #[test]
    fn parses_imports() {
        let src = r#"
const std = @import("std");
const mem = @import("std.mem");
const local = @import("./helpers.zig");
const tree = @import("../shared/tree.zig");
// Commented:  const skip = @import("skip.zig");
"#;
        let analysis = analyze_zig_file(src, "main.zig".to_string());
        let imports: Vec<_> = analysis.imports.iter().map(|i| i.source.clone()).collect();
        assert!(imports.contains(&"std".to_string()));
        assert!(imports.contains(&"std.mem".to_string()));
        assert!(imports.contains(&"./helpers.zig".to_string()));
        assert!(imports.contains(&"../shared/tree.zig".to_string()));
        assert!(!imports.contains(&"skip.zig".to_string()));
    }

    #[test]
    fn std_import_marked_stdlib() {
        let src = r#"const std = @import("std");"#;
        let analysis = analyze_zig_file(src, "main.zig".to_string());
        let std_entry = analysis
            .imports
            .iter()
            .find(|i| i.source == "std")
            .expect("std import present");
        assert_eq!(std_entry.resolution, ImportResolutionKind::Stdlib);
    }

    #[test]
    fn counts_tests() {
        let src = r#"
test "basic math" {
    try std.testing.expect(1 + 1 == 2);
}

test "another test" {
    try std.testing.expect(true);
}

pub fn main() void {}
"#;
        let analysis = analyze_zig_file(src, "main.zig".to_string());
        let tests: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "test")
            .collect();
        assert_eq!(tests.len(), 2);
    }
}
