//! Source code preprocessing for Rust analysis.
//!
//! Provides functions to strip test code, comments, and function-body imports
//! to avoid false positives in dependency and dead code analysis.

/// Find the position of the closing `]` that balances the opening one.
/// Returns the index of that `]` in the input, or 0 if not found.
pub(super) fn find_balanced_bracket(s: &str) -> usize {
    let mut depth = 0i32;
    for (i, ch) in s.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                if depth == 0 {
                    return i;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    0
}

/// Pass-through for function bodies (previously stripped inner `use` stmts).
/// Inner `use crate::...` (even inside fn) ARE real module consumers for impact/slice/
/// who-imports. Stripping them produced false "safe to remove" and invisible importers
/// (see loctree-feedback.md:2900, 3144). Local uses still participate in the import graph;
/// cycles pass can tolerate or filter length-1/self if needed.
/// KNOWN-GAP: if future cycle FP appears from this, filter in cycles.rs not by
/// stripping declarations here.
pub(super) fn strip_function_body_uses(content: &str) -> String {
    // Do not strip `use` statements from fn bodies: they establish real consumer
    // edges (e.g. CLI handler `use crate::watch::{...};` inside fn must make the
    // handler a consumer of watch.rs for impact/slice). The previous strip was
    // a local-optimum for "avoid false cycles" that violated the core promise.
    content.to_string()
}

/// Strip `#[cfg(test)]` annotated modules from content to avoid false positive cycles.
/// This removes test-only imports from dependency analysis.
pub(super) fn strip_cfg_test_modules(content: &str) -> String {
    let mut result = String::new();
    let mut chars = content.chars().peekable();
    let mut in_cfg_test_attr = false;

    while let Some(ch) = chars.next() {
        // Look for #[cfg(test)]
        if ch == '#' && chars.peek() == Some(&'[') {
            let pos = result.len();
            result.push(ch);

            // Collect the attribute
            let mut attr = String::from("#");
            for next in chars.by_ref() {
                attr.push(next);
                if next == ']' {
                    break;
                }
            }
            result.push_str(&attr[1..]); // Skip the '#' we already added

            // Check if it's #[cfg(test)] or #[cfg(all(..., test, ...))]
            let attr_inner = attr.trim();
            if attr_inner.starts_with("#[cfg(test)")
                || attr_inner.starts_with("#[cfg(all(") && attr_inner.contains("test")
            {
                in_cfg_test_attr = true;
                // Remove the attribute we just added
                result.truncate(pos);
            }
            continue;
        }

        // If we're after #[cfg(test)], look for `mod` keyword and skip the block
        if in_cfg_test_attr {
            result.push(ch);

            // Skip whitespace and look for `mod`
            if ch.is_whitespace() {
                continue;
            }

            // Check for 'mod' keyword
            if ch == 'm' {
                let mut keyword = String::from("m");
                while let Some(&next) = chars.peek() {
                    if next.is_alphabetic() || next == '_' {
                        keyword.push(chars.next().unwrap());
                    } else {
                        break;
                    }
                }

                if keyword == "mod" {
                    // Skip the module name and look for either `;` (external) or `{` (inline)
                    // Skip whitespace and module name first
                    let mut found_end = false;
                    for next in chars.by_ref() {
                        if next == ';' {
                            // External module: #[cfg(test)] mod env_tests;
                            // Just skip to the semicolon
                            found_end = true;
                            break;
                        }
                        if next == '{' {
                            // Inline module: #[cfg(test)] mod tests { ... }
                            // Skip the entire block (handle nested braces)
                            let mut depth = 1;
                            for inner in chars.by_ref() {
                                match inner {
                                    '{' => depth += 1,
                                    '}' => {
                                        depth -= 1;
                                        if depth == 0 {
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            found_end = true;
                            break;
                        }
                    }

                    if found_end {
                        // Remove 'mod' we just added to result
                        result.truncate(result.len() - 1); // Remove the 'm'
                    }
                    in_cfg_test_attr = false;
                    continue;
                } else {
                    // Not a mod, push the keyword
                    result.push_str(&keyword[1..]); // Skip 'm' we already added
                    in_cfg_test_attr = false;
                }
            } else {
                in_cfg_test_attr = false;
            }
            continue;
        }

        result.push(ch);
    }
    result
}

/// Strip `#[...]` attributes from a string (handles nested brackets).
pub(super) fn strip_cfg_attributes(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '#' {
            // Check if next char is '['
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                let mut depth = 1;
                // Skip until we find the matching ']'
                for inner in chars.by_ref() {
                    match inner {
                        '[' => depth += 1,
                        ']' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                continue;
            }
        }
        result.push(ch);
    }
    result
}

/// Strip both line comments (//) and block comments (/* */) from Rust source code.
/// This prevents false positives where type names are mentioned in comments.
pub(super) fn strip_comments(content: &str) -> String {
    let mut result = String::new();
    let mut chars = content.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '/' {
            match chars.peek() {
                Some('/') => {
                    // Line comment - skip until newline
                    chars.next(); // consume second '/'
                    while let Some(&next) = chars.peek() {
                        if next == '\n' {
                            result.push('\n'); // preserve newline for line counting
                            chars.next();
                            break;
                        }
                        chars.next();
                    }
                    continue;
                }
                Some('*') => {
                    // Block comment - skip until */
                    chars.next(); // consume '*'
                    let mut prev_star = false;
                    for next in chars.by_ref() {
                        if prev_star && next == '/' {
                            break;
                        }
                        prev_star = next == '*';
                        // Preserve newlines for line counting
                        if next == '\n' {
                            result.push('\n');
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }
        result.push(ch);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_function_body_uses() {
        let content = r#"
use crate::types::Foo;

fn bar() {
    use std::io;
    let x = 1;
}

use crate::types::Bar;
"#;
        let stripped = strip_function_body_uses(content);
        assert!(stripped.contains("use crate::types::Foo"));
        assert!(stripped.contains("use crate::types::Bar"));
        // Inner fn-body `use` must survive — they are real consumer edges (fail.md:2900).
        assert!(stripped.contains("use std::io"));
    }

    #[test]
    fn test_strip_cfg_test_modules_inline() {
        let content = r#"
use crate::types::Foo;

#[cfg(test)]
mod tests {
    use super::*;
    fn test_foo() {}
}

fn main() {}
"#;
        let stripped = strip_cfg_test_modules(content);
        assert!(stripped.contains("use crate::types::Foo"));
        assert!(stripped.contains("fn main()"));
        assert!(!stripped.contains("mod tests"));
        assert!(!stripped.contains("test_foo"));
    }

    #[test]
    fn test_strip_cfg_test_modules_external() {
        let content = r#"
use crate::types::Foo;

#[cfg(test)]
mod env_tests;

fn main() {}
"#;
        let stripped = strip_cfg_test_modules(content);
        assert!(stripped.contains("use crate::types::Foo"));
        assert!(stripped.contains("fn main()"));
        assert!(!stripped.contains("env_tests"));
    }

    #[test]
    fn test_strip_comments_line() {
        let content = "let x = 1; // this is a comment\nlet y = 2;";
        let stripped = strip_comments(content);
        assert_eq!(stripped, "let x = 1; \nlet y = 2;");
    }

    #[test]
    fn test_strip_comments_block() {
        let content = "let x = 1; /* block */ let y = 2;";
        let stripped = strip_comments(content);
        assert_eq!(stripped, "let x = 1;  let y = 2;");
    }

    #[test]
    fn test_strip_cfg_attributes() {
        let content = "#[derive(Debug)] struct Foo;";
        let stripped = strip_cfg_attributes(content);
        assert_eq!(stripped, " struct Foo;");
    }

    #[test]
    fn test_find_balanced_bracket() {
        assert_eq!(find_balanced_bracket("foo]"), 3);
        assert_eq!(find_balanced_bracket("foo[bar]]"), 8);
        assert_eq!(find_balanced_bracket(""), 0);
    }
}
