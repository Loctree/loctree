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

/// Replace string and raw-string literal BODIES with spaces, line-preserving.
///
/// Newlines pass through unchanged and every delimiter (the quotes, and a raw
/// string's `r`/`b`/`#` prefix) is kept verbatim — only the content between
/// the opening and closing quote becomes spaces, one space per character, so
/// byte offsets per line stay plausible for downstream line-based tools.
/// Mirrors [`strip_comments`]'s style (structure kept, content blanked).
///
/// Handles:
/// - Regular strings: `"..."`, honoring `\"` / `\\` escapes so an escaped
///   quote never terminates the literal early.
/// - Raw strings: `r"..."` and `r#"..."#` / `r##"..."##` / … — the closing
///   delimiter is `"` followed by exactly as many `#` as the opener.
/// - Byte and raw-byte strings: `b"..."`, `br"..."`, `br#"..."#`, same rules.
/// - Char literals (`'x'`, `'\''`, `'\n'`, `'"'`) are recognized and passed
///   through UNCHANGED — a `"` inside one (`'"'`) must never be mistaken for
///   a string opener. A lifetime (`'a`, `'static`) is not a char literal (no
///   closing `'` follows a single token) and is left untouched either way.
pub(crate) fn strip_string_spans(content: &str) -> String {
    let chars: Vec<char> = content.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(content.len());
    let mut i = 0usize;

    while i < n {
        let ch = chars[i];

        // Comments pass through verbatim: a quote inside `//` or `/* */`
        // must never flip the lexer into string state (audit N03). Their
        // content is strip_comments' job downstream, not this pass's.
        if ch == '/' && chars.get(i + 1) == Some(&'/') {
            while i < n && chars[i] != '\n' {
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        if ch == '/' && chars.get(i + 1) == Some(&'*') {
            out.push('/');
            out.push('*');
            i += 2;
            // Rust block comments nest.
            let mut depth = 1usize;
            while i < n && depth > 0 {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push('/');
                    out.push('*');
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push('*');
                    out.push('/');
                    i += 2;
                } else {
                    out.push(chars[i]);
                    i += 1;
                }
            }
            continue;
        }

        // Raw / byte-raw string prefix: optional `b`, then `r`, then zero or
        // more `#`, then the opening `"`. Only consumed when this exact
        // shape is present — an ordinary identifier starting `r`/`b` falls
        // through untouched (no `#`+`"` follows it).
        let mut j = i;
        if chars.get(j) == Some(&'b') {
            j += 1;
        }
        if chars.get(j) == Some(&'r') {
            let mut k = j + 1;
            let mut hashes = 0usize;
            while chars.get(k) == Some(&'#') {
                hashes += 1;
                k += 1;
            }
            if chars.get(k) == Some(&'"') {
                for &c in &chars[i..=k] {
                    out.push(c);
                }
                i = k + 1;
                loop {
                    if i >= n {
                        break;
                    }
                    if chars[i] == '"' {
                        let mut m = i + 1;
                        let mut seen = 0usize;
                        while seen < hashes && chars.get(m) == Some(&'#') {
                            seen += 1;
                            m += 1;
                        }
                        if seen == hashes {
                            out.push('"');
                            for _ in 0..hashes {
                                out.push('#');
                            }
                            i = m;
                            break;
                        }
                    }
                    if chars[i] == '\n' {
                        out.push('\n');
                    } else {
                        out.push(' ');
                    }
                    i += 1;
                }
                continue;
            }
        }

        if ch == '"' {
            out.push('"');
            i += 1;
            let mut escaped = false;
            while i < n {
                let c = chars[i];
                if escaped {
                    escaped = false;
                    // A `\` + newline continuation must keep the newline —
                    // the whole pass is line-preserving (audit N03).
                    if c == '\n' {
                        out.push('\n');
                    } else {
                        out.push(' ');
                    }
                    i += 1;
                    continue;
                }
                if c == '\\' {
                    escaped = true;
                    out.push(' ');
                    i += 1;
                    continue;
                }
                if c == '"' {
                    out.push('"');
                    i += 1;
                    break;
                }
                if c == '\n' {
                    out.push('\n');
                } else {
                    out.push(' ');
                }
                i += 1;
            }
            continue;
        }

        if ch == '\''
            && let Some(end) = char_literal_end(&chars, i)
        {
            for &c in &chars[i..=end] {
                out.push(c);
            }
            i = end + 1;
            continue;
        }

        out.push(ch);
        i += 1;
    }

    out
}

/// If `chars[start]` (a `'`) opens a char literal, return the index of its
/// closing `'`. Handles a plain char (`'x'`), a backslash escape (`'\n'`,
/// `'\''`, `'\\'`), and a unicode escape (`'\u{1F600}'`). Returns `None` for
/// anything that does not close this way — in particular a lifetime
/// (`'a`, `'static`) never has a bare closing `'` and is left alone.
pub(super) fn char_literal_end(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start + 1;
    if chars.get(i) == Some(&'\\') {
        i += 1;
        match chars.get(i) {
            Some('u') if chars.get(i + 1) == Some(&'{') => {
                i += 2;
                while chars.get(i).is_some_and(|c| *c != '}') {
                    i += 1;
                }
                if chars.get(i) == Some(&'}') {
                    i += 1;
                } else {
                    return None;
                }
            }
            Some(_) => i += 1,
            None => return None,
        }
    } else {
        i += 1;
    }
    if chars.get(i) == Some(&'\'') {
        Some(i)
    } else {
        None
    }
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

    #[test]
    fn strip_string_spans_blanks_plain_string_body_only() {
        let content = r#"let s = "hello";"#;
        assert_eq!(strip_string_spans(content), r#"let s = "     ";"#);
    }

    #[test]
    fn strip_string_spans_honors_escaped_quotes_and_backslashes() {
        // Body is `a\"b` (4 source characters: a, \, ", b) — the escaped
        // quote must NOT terminate the literal early.
        let content = r#"let s = "a\"b";"#;
        assert_eq!(strip_string_spans(content), r#"let s = "    ";"#);
    }

    #[test]
    fn strip_string_spans_blanks_raw_string_body_with_hashes() {
        // `r##"..."##`: the body contains an embedded `"` that is NOT
        // followed by two `#`, so it must not close the literal early; only
        // the trailing `"##` closes it.
        let content = r###"let s = r##"a "b" c"##;"###;
        assert_eq!(
            strip_string_spans(content),
            r###"let s = r##"       "##;"###
        );
    }

    #[test]
    fn strip_string_spans_blanks_raw_byte_string_body() {
        let content = r##"let s = br#"raw"#;"##;
        assert_eq!(strip_string_spans(content), r##"let s = br#"   "#;"##);
    }

    #[test]
    fn strip_string_spans_preserves_newlines_inside_string_body() {
        let content = "let s = \"a\nb\";";
        let stripped = strip_string_spans(content);
        assert_eq!(stripped, "let s = \" \n \";");
        assert_eq!(
            stripped.lines().count(),
            content.lines().count(),
            "line count must be preserved so line-based offsets stay plausible"
        );
    }

    #[test]
    fn strip_string_spans_leaves_char_literals_untouched() {
        // A `"` embedded in a char literal (`'"'`) must never be mistaken for
        // a string opener — the char literal passes through byte-for-byte,
        // and the real string right after it is still blanked correctly.
        let content = r#"let c = '"'; let s = "ok";"#;
        assert_eq!(strip_string_spans(content), r#"let c = '"'; let s = "  ";"#);
    }

    #[test]
    fn strip_string_spans_leaves_lifetimes_and_escaped_char_literals_alone() {
        let content = r"fn f<'a>(x: &'a str) { let c = '\''; let n = '\n'; }";
        let stripped = strip_string_spans(content);
        // No string literals at all in this fixture — nothing should change.
        assert_eq!(stripped, content);
    }
    #[test]
    fn quote_in_line_comment_does_not_open_a_string() {
        // Audit N03: a lone `"` inside a comment used to flip the lexer into
        // string state and blank every following declaration.
        let src = "// a lone \" in a comment\npub fn real() {}\n";
        let out = strip_string_spans(src);
        assert!(
            out.contains("pub fn real() {}"),
            "real code after a commented quote must survive, got: {out:?}"
        );
    }

    #[test]
    fn quote_in_block_comment_does_not_open_a_string() {
        let src = "/* say \" here */\npub fn real() {}\n";
        let out = strip_string_spans(src);
        assert!(
            out.contains("pub fn real() {}"),
            "real code after a block-commented quote must survive, got: {out:?}"
        );
    }

    #[test]
    fn quote_in_nested_block_comment_does_not_open_a_string() {
        // Rust block comments nest.
        let src = "/* outer /* \" */ still comment */\npub fn real() {}\n";
        let out = strip_string_spans(src);
        assert!(
            out.contains("pub fn real() {}"),
            "nested block comments must be honored, got: {out:?}"
        );
    }

    #[test]
    fn escaped_newline_in_string_keeps_the_line_count() {
        // Audit N03: the escape branch replaced `\\` + newline with a space,
        // breaking the documented line-preserving contract.
        let src = "let s = \"a\\\n b\";\npub fn real() {}\n";
        let out = strip_string_spans(src);
        assert_eq!(
            out.lines().count(),
            src.lines().count(),
            "line count must be preserved through a string continuation, got: {out:?}"
        );
        assert!(out.contains("pub fn real() {}"));
    }
}
