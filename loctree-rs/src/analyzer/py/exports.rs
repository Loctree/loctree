//! Python __all__ list parsing and re-export handling.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::path::{Path, PathBuf};

use super::super::regexes::regex_py_all;

/// Parse Python `__all__` list and extract exported names.
/// Handles inline comments, multi-line lists, and both single and double quoted strings.
pub(super) fn parse_all_list(content: &str) -> Vec<String> {
    fn strip_line_comment(line: &str) -> String {
        let mut out = String::new();
        let mut in_single = false;
        let mut in_double = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    out.push(c);
                    if let Some(next) = chars.next() {
                        out.push(next);
                    }
                }
                '\'' if !in_double => {
                    in_single = !in_single;
                    out.push(c);
                }
                '"' if !in_single => {
                    in_double = !in_double;
                    out.push(c);
                }
                '#' if !in_single && !in_double => {
                    break;
                }
                _ => out.push(c),
            }
        }
        out
    }

    let mut names = Vec::new();
    for caps in regex_py_all().captures_iter(content) {
        let body = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        for line in body.lines() {
            let cleaned = strip_line_comment(line);
            let cleaned = cleaned.trim();
            if cleaned.is_empty() || cleaned.starts_with('#') {
                continue;
            }
            for item in cleaned.split(',') {
                let trimmed = item.trim();
                let mut name = trimmed
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim_matches(|c| c == '\'' || c == '"')
                    .trim()
                    .replace('\n', "")
                    .to_string();
                if name.starts_with('#') {
                    name.clear();
                }
                if !name.is_empty() {
                    names.push(name);
                }
            }
        }
    }
    names
}

/// Read __all__ list from a resolved module path.
/// Returns None if the file cannot be read or has no __all__ list.
pub(super) fn read_all_from_resolved(
    resolved: &Option<String>,
    root: &Path,
) -> Option<Vec<String>> {
    let path_str = resolved.as_ref()?;
    let candidate = {
        let p = PathBuf::from(path_str);
        if p.is_absolute() { p } else { root.join(p) }
    };
    let content = std::fs::read_to_string(&candidate).ok()?;
    let names = parse_all_list(&content);
    if names.is_empty() { None } else { Some(names) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_all_list() {
        let content = r#"__all__ = ["foo", "bar"]"#;
        let names = parse_all_list(content);
        assert_eq!(names, vec!["foo", "bar"]);
    }

    #[test]
    fn parses_all_list_with_comments() {
        let content = r#"
__all__ = [
    "foo",  # inline comment
    "bar",
    # "baz" is intentionally excluded
]
"#;
        let names = parse_all_list(content);
        assert_eq!(names, vec!["foo", "bar"]);
        assert!(!names.iter().any(|n| n.contains('#')));
        assert!(!names.contains(&"baz".to_string()));
    }

    #[test]
    fn parses_single_quoted_all_list() {
        let content = r#"__all__ = ['alpha', 'beta']"#;
        let names = parse_all_list(content);
        assert_eq!(names, vec!["alpha", "beta"]);
    }
}
