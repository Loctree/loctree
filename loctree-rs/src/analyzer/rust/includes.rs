//! Parser for Rust include macros (`include_str!`, `include_bytes!`, `include!`)
//! and `build.rs` resource reads (`rerun-if-changed`, `fs::read*`, `File::open`).

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RustInclude {
    pub line: usize,
    pub raw: String,
    pub is_dynamic: bool,
    pub macro_name: String,
}

fn regex_include_macro() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?:\b(?:std|core)::)?\b(include_str|include_bytes|include)\s*!"#)
            .expect("valid regex")
    })
}

fn regex_rerun_if_changed() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"cargo(?:::|:)rerun-if-changed=([^\s"'\\]+)"#).expect("valid regex")
    })
}

fn regex_build_fs_read() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?:\b(?:std::)?fs::(?:read_to_string|read|copy)|\b(?:std::)?fs::File::open|\bFile::open)\s*\("#)
            .expect("valid regex")
    })
}

pub(crate) fn is_build_script(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized == "build.rs"
        || normalized.ends_with("/build.rs")
        || normalized.starts_with("build/")
        || normalized.contains("/build/")
}

pub(crate) fn parse_rust_string_literal(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Check for raw string: r"...", r#"..."#, r##"..."##
    if let Some(after_r) = s.strip_prefix('r') {
        let hash_count = after_r.chars().take_while(|&c| c == '#').count();
        let after_hashes = &after_r[hash_count..];
        if after_hashes.starts_with('"') {
            let closing = format!("\"{}", "#".repeat(hash_count));
            if after_hashes.ends_with(&closing) && after_hashes.len() > closing.len() {
                let inner = &after_hashes[1..after_hashes.len() - closing.len()];
                return Some(inner.to_string());
            }
        }
    }

    // Check for standard string: "..."
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        let mut unescaped = String::with_capacity(inner.len());
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(next) = chars.next() {
                    match next {
                        'n' => unescaped.push('\n'),
                        'r' => unescaped.push('\r'),
                        't' => unescaped.push('\t'),
                        '\\' => unescaped.push('\\'),
                        '"' => unescaped.push('"'),
                        '0' => unescaped.push('\0'),
                        other => {
                            unescaped.push('\\');
                            unescaped.push(other);
                        }
                    }
                } else {
                    unescaped.push('\\');
                }
            } else {
                unescaped.push(c);
            }
        }
        return Some(unescaped);
    }

    // Check for concat!(...)
    if s.starts_with("concat!(") && s.ends_with(')') {
        let inner = &s["concat!(".len()..s.len() - 1];
        return parse_concat_args(inner);
    }
    if s.starts_with("concat![") && s.ends_with(']') {
        let inner = &s["concat![".len()..s.len() - 1];
        return parse_concat_args(inner);
    }
    if s.starts_with("concat!{") && s.ends_with('}') {
        let inner = &s["concat!{".len()..s.len() - 1];
        return parse_concat_args(inner);
    }

    None
}

fn parse_concat_args(args_str: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut chars = args_str.chars().peekable();

    while let Some(c) = chars.next() {
        if in_string {
            current.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }

        match c {
            '"' => {
                in_string = true;
                current.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                current.push(c);
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            ',' if depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        parts.push(trimmed.to_string());
    }

    let mut result = String::new();
    for part in parts {
        let part_trimmed = part.trim();
        if part_trimmed.starts_with("env!(") && part_trimmed.ends_with(')') {
            let env_var = part_trimmed["env!(".len()..part_trimmed.len() - 1].trim();
            let parsed_env =
                parse_rust_string_literal(env_var).unwrap_or_else(|| env_var.to_string());
            if parsed_env == "CARGO_MANIFEST_DIR" {
                result.push_str("env:CARGO_MANIFEST_DIR");
            } else {
                return None;
            }
        } else {
            let lit = parse_rust_string_literal(part_trimmed)?;
            result.push_str(&lit);
        }
    }
    Some(result)
}

fn extract_macro_call_arg(s: &str, after_bang_idx: usize) -> Option<(&str, char)> {
    let remaining = &s[after_bang_idx..];
    let mut char_indices = remaining.char_indices();

    // Find opening delimiter
    let (open_offset, open_delim) = loop {
        match char_indices.next() {
            Some((_, ch)) if ch.is_whitespace() => continue,
            Some((idx, ch @ ('(' | '[' | '{'))) => break (idx, ch),
            _ => return None,
        }
    };

    let close_delim = match open_delim {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => return None,
    };

    let start_idx = after_bang_idx + open_offset + 1;
    let mut depth = 1usize;
    let mut in_string = false;
    let mut chars = s[start_idx..].char_indices().peekable();

    while let Some((idx, c)) = chars.next() {
        if in_string {
            if c == '\\' {
                chars.next();
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }

        if c == '"' {
            in_string = true;
            continue;
        }

        if c == open_delim {
            depth += 1;
        } else if c == close_delim {
            depth -= 1;
            if depth == 0 {
                let end_idx = start_idx + idx;
                return Some((&s[start_idx..end_idx], open_delim));
            }
        }
    }

    None
}

fn extract_first_comma_arg(s: &str) -> &str {
    let mut in_string = false;
    let mut depth: usize = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((idx, c)) = chars.next() {
        if in_string {
            if c == '\\' {
                chars.next();
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return &s[..idx],
            _ => {}
        }
    }
    s
}

pub(crate) fn extract_rust_includes(content: &str, relative: &str) -> Vec<RustInclude> {
    let mut includes = Vec::new();
    let mut seen = HashSet::new();

    // 1. Extract include_str!, include_bytes!, include!
    for caps in regex_include_macro().captures_iter(content) {
        let full_match = caps.get(0).unwrap();
        let macro_name = caps.get(1).unwrap().as_str();
        let after_bang = full_match.end();

        if let Some((inner, _delim)) = extract_macro_call_arg(content, after_bang) {
            let line = content[..full_match.start()]
                .chars()
                .filter(|&c| c == '\n')
                .count()
                + 1;
            let trimmed = inner.trim().trim_end_matches(',').trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some(lit) = parse_rust_string_literal(trimmed) {
                if seen.insert((lit.clone(), false)) {
                    includes.push(RustInclude {
                        line,
                        raw: lit,
                        is_dynamic: false,
                        macro_name: macro_name.to_string(),
                    });
                }
            } else {
                // Dynamic / variable argument
                if seen.insert((trimmed.to_string(), true)) {
                    includes.push(RustInclude {
                        line,
                        raw: trimmed.to_string(),
                        is_dynamic: true,
                        macro_name: macro_name.to_string(),
                    });
                }
            }
        }
    }

    // 2. If this is a build script (build.rs or build/*), extract rerun-if-changed and fs reads
    if is_build_script(relative) {
        // 2a. rerun-if-changed
        for caps in regex_rerun_if_changed().captures_iter(content) {
            let m = caps.get(1).unwrap();
            let path_str = m.as_str().trim();
            if path_str == "build.rs" || path_str == "./build.rs" {
                continue;
            }
            let line = content[..m.start()].chars().filter(|&c| c == '\n').count() + 1;
            let is_dynamic = path_str.contains('{') || path_str.contains('}');
            if seen.insert((path_str.to_string(), is_dynamic)) {
                includes.push(RustInclude {
                    line,
                    raw: path_str.to_string(),
                    is_dynamic,
                    macro_name: "rerun-if-changed".to_string(),
                });
            }
        }

        // 2b. fs::read*, File::open
        for caps in regex_build_fs_read().captures_iter(content) {
            let m = caps.get(0).unwrap();
            let after_paren = m.end() - 1; // start of '('
            if let Some((inner, _delim)) = extract_macro_call_arg(content, after_paren) {
                let line = content[..m.start()].chars().filter(|&c| c == '\n').count() + 1;
                let first_arg = extract_first_comma_arg(inner).trim();
                if first_arg.is_empty() {
                    continue;
                }
                if let Some(lit) = parse_rust_string_literal(first_arg) {
                    if lit == "build.rs" || lit == "./build.rs" {
                        continue;
                    }
                    if seen.insert((lit.clone(), false)) {
                        includes.push(RustInclude {
                            line,
                            raw: lit,
                            is_dynamic: false,
                            macro_name: "fs_read".to_string(),
                        });
                    }
                } else if seen.insert((first_arg.to_string(), true)) {
                    includes.push(RustInclude {
                        line,
                        raw: first_arg.to_string(),
                        is_dynamic: true,
                        macro_name: "fs_read".to_string(),
                    });
                }
            }
        }
    }

    includes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_string_literals() {
        assert_eq!(
            parse_rust_string_literal(r#""hello/world.json""#),
            Some("hello/world.json".to_string())
        );
        assert_eq!(
            parse_rust_string_literal(r#"r"raw/path.sql""#),
            Some("raw/path.sql".to_string())
        );
        assert_eq!(
            parse_rust_string_literal(r##"r#"hash/path.wasm"#"##),
            Some("hash/path.wasm".to_string())
        );
        assert_eq!(
            parse_rust_string_literal(r#"concat!(env!("CARGO_MANIFEST_DIR"), "/data/x.json")"#),
            Some("env:CARGO_MANIFEST_DIR/data/x.json".to_string())
        );
        assert_eq!(parse_rust_string_literal("VARIABLE_PATH"), None);
    }

    #[test]
    fn test_extract_includes() {
        let code = r#"
            const S: &str = include_str!("../data/x.json");
            let bytes = include_bytes!["assets/icon.png"];
            include!{"nested.rs"};
            let dyn_data = include_str!(DYNAMIC_PATH);
        "#;
        let incs = extract_rust_includes(code, "src/lib.rs");
        assert_eq!(incs.len(), 4);
        assert_eq!(incs[0].raw, "../data/x.json");
        assert!(!incs[0].is_dynamic);
        assert_eq!(incs[1].raw, "assets/icon.png");
        assert!(!incs[1].is_dynamic);
        assert_eq!(incs[2].raw, "nested.rs");
        assert!(!incs[2].is_dynamic);
        assert_eq!(incs[3].raw, "DYNAMIC_PATH");
        assert!(incs[3].is_dynamic);
    }

    #[test]
    fn test_extract_build_script_reads() {
        let build_rs = r#"
            fn main() {
                println!("cargo:rerun-if-changed=plugins/session-manager.wasm");
                println!("cargo:rerun-if-changed=build.rs");
                let wasm = std::fs::read("plugins/other.wasm");
                let schema = fs::read_to_string("schema.sql");
            }
        "#;
        let incs = extract_rust_includes(build_rs, "build.rs");
        let paths: Vec<&str> = incs.iter().map(|i| i.raw.as_str()).collect();
        assert!(paths.contains(&"plugins/session-manager.wasm"));
        assert!(paths.contains(&"plugins/other.wasm"));
        assert!(paths.contains(&"schema.sql"));
        // build.rs self-dependency must be skipped
        assert!(!paths.contains(&"build.rs"));
    }
}
