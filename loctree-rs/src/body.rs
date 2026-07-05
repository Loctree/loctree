//! Bounded symbol body / source-range retrieval.
//!
//! Closes the "Loctree got me to the door, grep opened it" gap: once
//! `where-symbol` locates a symbol's defining file + line, this module
//! returns the bounded source text of that symbol's body without the agent
//! ever shelling out to `grep`/`sed`/`awk`.
//!
//! Body extraction is brace-balanced (works for Rust, TS/JS, C-family) and
//! falls back to a fixed line window for languages without `{...}` bodies
//! (e.g. Python). Output is always bounded by a default line cap with
//! explicit truncation metadata.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use serde::{Deserialize, Serialize};

use crate::query::query_where_symbol;
use crate::snapshot::Snapshot;

/// Default maximum number of source lines returned for a single body.
pub const DEFAULT_BODY_LINE_CAP: usize = 200;

/// Fallback line window (lines after the definition line) for symbols whose
/// body is not delimited by braces (e.g. Python `def`).
const FALLBACK_WINDOW: usize = 40;

/// A bounded source body for a single symbol definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolBody {
    /// Symbol name that was queried.
    pub symbol: String,
    /// File the body was extracted from (repo-relative path).
    pub file: String,
    /// 1-based start line of the body.
    pub start_line: usize,
    /// 1-based end line of the body (inclusive) actually returned.
    pub end_line: usize,
    /// Detected language (file extension, lowercase) or "unknown".
    pub language: String,
    /// Bounded source text (already capped to `line_cap`).
    pub source: String,
    /// True if the body exceeded `line_cap` and was truncated.
    pub truncated: bool,
    /// Total lines the full body would have spanned (pre-cap).
    pub total_lines: usize,
    /// Line cap that was applied.
    pub line_cap: usize,
}

/// Aggregate result of a `loct body <symbol>` lookup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BodyResult {
    /// Symbol name queried.
    pub symbol: String,
    /// Bodies found (one per defining file/line).
    pub bodies: Vec<SymbolBody>,
}

/// Derive a lowercase language tag from a file path's extension.
fn language_of(path: &str) -> String {
    path.rsplit('.')
        .next()
        .filter(|ext| *ext != path)
        .map(|ext| ext.to_lowercase())
        .unwrap_or_else(|| "unknown".to_string())
}

/// If `line` is a plain assignment whose right-hand side opens a `(` tuple or
/// `[` list, return the byte offset just after the `=`. This is the signal that
/// the body is a bracket-delimited collection const (e.g.
/// `FRAMEWORK_LAUNCHER_MARKERS = (`) rather than a brace body or a `def`.
///
/// Returns `None` for `==`/`<=`/augmented/walrus operators, for `def f(...):`
/// (its paren is not preceded by a plain `=`), and for dict/object `{`
/// assignments (those keep the existing brace-balanced path). The returned
/// offset is where bracket balancing should begin counting on the first line.
fn assignment_collection_rhs(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' {
            let next = bytes.get(i + 1).copied().unwrap_or(b' ');
            if next == b'=' {
                i += 2;
                continue;
            }
            let prev = if i > 0 { bytes[i - 1] } else { b' ' };
            if matches!(
                prev,
                b'!' | b'<'
                    | b'>'
                    | b'+'
                    | b'-'
                    | b'*'
                    | b'/'
                    | b'%'
                    | b'&'
                    | b'|'
                    | b'^'
                    | b'@'
                    | b':'
                    | b'~'
                    | b'='
            ) {
                return None;
            }
            let rhs = line[i + 1..].trim_start();
            if rhs.starts_with('(') || rhs.starts_with('[') {
                return Some(i + 1);
            }
            return None;
        }
        i += 1;
    }
    None
}

/// Bracket-balanced scan over `(`/`[`/`{` (and their closers), starting at
/// `start_idx` from byte offset `first_byte_offset` on that line. Returns the
/// 0-based line index where the outermost bracket closes, or `None` if it never
/// balances. Shares the quote/escape/comment shielding of the brace scanner so
/// brackets inside strings or comments do not derail the count.
fn extract_bracket_balanced(
    lines: &[&str],
    start_idx: usize,
    first_byte_offset: usize,
    language: &str,
) -> Option<usize> {
    let rust_quotes = language == "rs";
    let mut depth: i32 = 0;
    let mut started = false;
    let mut in_string: Option<char> = None;
    for (i, line) in lines.iter().enumerate().skip(start_idx) {
        let chars: Vec<char> = line.chars().collect();
        let mut escaped = false;
        let mut idx = if i == start_idx {
            line[..first_byte_offset.min(line.len())].chars().count()
        } else {
            0
        };
        while idx < chars.len() {
            let ch = chars[idx];
            if let Some(q) = in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == q {
                    in_string = None;
                }
                idx += 1;
                continue;
            }
            match ch {
                '"' | '`' => in_string = Some(ch),
                '\'' => {
                    if rust_quotes {
                        let opens_char_literal =
                            chars.get(idx + 1) == Some(&'\\') || chars.get(idx + 2) == Some(&'\'');
                        if opens_char_literal {
                            in_string = Some('\'');
                        }
                    } else {
                        in_string = Some('\'');
                    }
                }
                // Python `#` and C-family `//` comments run to end-of-line.
                '#' => break,
                '/' if chars.get(idx + 1) == Some(&'/') => break,
                '(' | '[' | '{' => {
                    depth += 1;
                    started = true;
                }
                ')' | ']' | '}' => {
                    depth -= 1;
                    if started && depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
            idx += 1;
        }
    }
    None
}

/// Extract a bounded body starting at `start_line` (1-based) from `lines`.
///
/// Tries, in order: bracket balancing for an assignment-opened tuple/list const
/// (`NAME = (`/`[`), then brace balancing when the definition region contains a
/// `{`, then a fixed line window. Always returns at most `line_cap` lines.
///
/// `language` is the lowercase extension tag from [`language_of`]; it selects
/// quote semantics (Rust `'` is a lifetime/label/char-literal, not a general
/// string quote).
fn extract_body(
    lines: &[&str],
    start_idx: usize,
    line_cap: usize,
    language: &str,
) -> (usize, String, bool, usize) {
    // Assignment-opened tuple/list collection (`NAME = (`/`[`): balance that
    // bracket so a multi-line const returns exactly its own body instead of a
    // fixed window that overshoots into trailing code. Dict/object `{`
    // assignments fall through to the brace path below.
    let assign_collection_end = assignment_collection_rhs(lines[start_idx])
        .and_then(|off| extract_bracket_balanced(lines, start_idx, off, language));

    // Look for the opening brace within a small lookahead from the definition.
    let mut brace_open_idx: Option<usize> = None;
    let lookahead_end = (start_idx + 10).min(lines.len());
    'outer: for (offset, line) in lines[start_idx..lookahead_end].iter().enumerate() {
        if line.contains('{') {
            brace_open_idx = Some(start_idx + offset);
            break 'outer;
        }
    }

    let end_idx = if let Some(close_idx) = assign_collection_end {
        close_idx
    } else if let Some(open_idx) = brace_open_idx {
        // Brace-balanced scan from the opening brace line.
        let rust_quotes = language == "rs";
        let mut depth: i32 = 0;
        let mut found_end = open_idx;
        let mut in_string: Option<char> = None;
        let mut closed = false;
        'scan: for (i, line) in lines.iter().enumerate().skip(open_idx) {
            let chars: Vec<char> = line.chars().collect();
            let mut escaped = false;
            let mut idx = 0;
            while idx < chars.len() {
                let ch = chars[idx];
                if let Some(q) = in_string {
                    // Inside a string/char literal: consume escapes so that
                    // `'\\'` and `"\""` close where they actually close.
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == q {
                        in_string = None;
                    }
                    idx += 1;
                    continue;
                }
                match ch {
                    '"' | '`' => in_string = Some(ch),
                    '\'' => {
                        if rust_quotes {
                            // Rust: `'` opens a char literal only as `'x'` or
                            // `'\...'`. Lifetimes (`&'a`) and loop labels
                            // (`'scan:`) never close, so treating them as
                            // string openers derails brace balancing.
                            let opens_char_literal = chars.get(idx + 1) == Some(&'\\')
                                || chars.get(idx + 2) == Some(&'\'');
                            if opens_char_literal {
                                in_string = Some('\'');
                            }
                        } else {
                            in_string = Some('\'');
                        }
                    }
                    // Line comment: braces/quotes after `//` are not code.
                    '/' if chars.get(idx + 1) == Some(&'/') => break,
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            found_end = i;
                            closed = true;
                            break 'scan;
                        }
                    }
                    _ => {}
                }
                idx += 1;
            }
        }
        if closed {
            found_end
        } else {
            (start_idx + FALLBACK_WINDOW).min(lines.len().saturating_sub(1))
        }
    } else {
        // No brace body (e.g. Python): fixed window.
        (start_idx + FALLBACK_WINDOW).min(lines.len().saturating_sub(1))
    };

    let total_lines = end_idx - start_idx + 1;
    let capped_end_idx = (start_idx + line_cap - 1).min(end_idx);
    let truncated = capped_end_idx < end_idx;

    let source = lines[start_idx..=capped_end_idx].join("\n");
    (capped_end_idx + 1, source, truncated, total_lines)
}

/// Retrieve bounded source bodies for `symbol` using the cached snapshot to
/// locate definitions, then reading source files directly from disk.
///
/// `line_cap` of `None` uses [`DEFAULT_BODY_LINE_CAP`].
/// Read a source file referenced by a snapshot match.
///
/// Snapshot file paths are project-root-relative, so a bare `read_to_string`
/// only succeeds when the process cwd happens to be the project root — which is
/// NOT guaranteed for the LSP server (it can be spawned with any cwd). Try the
/// path as-is first (absolute paths and cwd==root keep working), then resolve it
/// against each snapshot root, so an imported symbol's body (and any body) reads
/// correctly regardless of cwd.
fn read_source(snapshot: &Snapshot, file: &str) -> Option<String> {
    if let Ok(content) = std::fs::read_to_string(file) {
        return Some(content);
    }
    let path = std::path::Path::new(file);
    if path.is_relative() {
        for root in &snapshot.metadata.roots {
            if let Ok(content) = std::fs::read_to_string(std::path::Path::new(root).join(path)) {
                return Some(content);
            }
        }
    }
    None
}

pub fn query_symbol_body(snapshot: &Snapshot, symbol: &str, line_cap: Option<usize>) -> BodyResult {
    let cap = line_cap.unwrap_or(DEFAULT_BODY_LINE_CAP).max(1);
    let where_result = query_where_symbol(snapshot, symbol);

    let mut bodies = Vec::new();
    let mut seen: std::collections::HashSet<(String, usize)> = std::collections::HashSet::new();

    for m in &where_result.results {
        // We need a concrete line to anchor body extraction.
        let Some(line) = m.line else { continue };
        if line == 0 {
            continue;
        }
        let key = (m.file.clone(), line);
        if !seen.insert(key) {
            continue;
        }

        let Some(content) = read_source(snapshot, &m.file) else {
            continue;
        };
        let lines: Vec<&str> = content.lines().collect();
        let start_idx = line - 1;
        if start_idx >= lines.len() {
            continue;
        }

        let language = language_of(&m.file);
        let (end_line, source, truncated, total_lines) =
            extract_body(&lines, start_idx, cap, &language);

        bodies.push(SymbolBody {
            symbol: symbol.to_string(),
            file: m.file.clone(),
            start_line: line,
            end_line,
            language,
            source,
            truncated,
            total_lines,
            line_cap: cap,
        });
    }

    BodyResult {
        symbol: symbol.to_string(),
        bodies,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_of() {
        assert_eq!(language_of("src/foo.rs"), "rs");
        assert_eq!(language_of("a/b/Thing.TSX"), "tsx");
        assert_eq!(language_of("Makefile"), "unknown");
    }

    #[test]
    fn test_extract_brace_balanced() {
        let src =
            "fn outer() {\n    let x = 1;\n    if x > 0 {\n        return;\n    }\n}\ntrailing";
        let lines: Vec<&str> = src.lines().collect();
        let (end_line, body, truncated, total) = extract_body(&lines, 0, 200, "rs");
        assert_eq!(end_line, 6, "closing brace is on line 6");
        assert!(body.contains("fn outer()"));
        assert!(body.ends_with("}"));
        assert!(!body.contains("trailing"));
        assert!(!truncated);
        assert_eq!(total, 6);
    }

    #[test]
    fn test_extract_respects_cap() {
        let src = "fn big() {\n  a;\n  b;\n  c;\n  d;\n}";
        let lines: Vec<&str> = src.lines().collect();
        let (_end, body, truncated, total) = extract_body(&lines, 0, 3, "rs");
        assert!(truncated);
        assert_eq!(total, 6);
        assert_eq!(body.lines().count(), 3);
    }

    #[test]
    fn test_extract_fallback_window_no_brace() {
        let src = "def thing():\n    return 1\n    # more";
        let lines: Vec<&str> = src.lines().collect();
        let (_end, body, _truncated, _total) = extract_body(&lines, 0, 200, "py");
        assert!(body.contains("def thing():"));
    }

    #[test]
    fn test_extract_stops_at_boundary_despite_char_escape_literal() {
        // Regression: `'\\'` used to leave the scanner permanently
        // "in string", swallowing every brace after it and overshooting
        // into sibling methods (loct body resolve_file_in_snapshot bug).
        let src = "    fn normalize(&self, raw: &str) -> String {\n        raw.replace('\\\\', \"/\").to_string()\n    }\n\n    fn sibling(&self) {\n        println!(\"sibling\");\n    }";
        let lines: Vec<&str> = src.lines().collect();
        let (end_line, body, truncated, total) = extract_body(&lines, 0, 200, "rs");
        assert_eq!(end_line, 3, "body must close at the method's own brace");
        assert_eq!(total, 3);
        assert!(!truncated);
        assert!(!body.contains("sibling"), "must not overshoot into sibling");
    }

    #[test]
    fn test_extract_rust_lifetime_and_label_not_string_openers() {
        let src = "fn pick<'a>(&'a self, raw: &'a str) -> &'a str {\n    'outer: loop {\n        break 'outer;\n    }\n    raw\n}\nfn after() {}";
        let lines: Vec<&str> = src.lines().collect();
        let (end_line, body, truncated, total) = extract_body(&lines, 0, 200, "rs");
        assert_eq!(end_line, 6, "lifetimes/labels must not derail brace scan");
        assert_eq!(total, 6);
        assert!(!truncated);
        assert!(!body.contains("fn after"));
    }

    #[test]
    fn test_extract_ignores_braces_in_line_comments() {
        let src = "fn doc() {\n    // unmatched { in a comment\n    let x = 1;\n}\nfn next() {}";
        let lines: Vec<&str> = src.lines().collect();
        let (end_line, body, _truncated, _total) = extract_body(&lines, 0, 200, "rs");
        assert_eq!(end_line, 4);
        assert!(!body.contains("fn next"));
    }

    #[test]
    fn test_extract_js_single_quote_string_still_shields_braces() {
        let src = "function f() {\n  const s = '}';\n  return s;\n}\nconst after = 1;";
        let lines: Vec<&str> = src.lines().collect();
        let (end_line, body, _truncated, _total) = extract_body(&lines, 0, 200, "js");
        assert_eq!(end_line, 4, "JS '}}' string literal must not close the fn");
        assert!(!body.contains("after"));
    }

    #[test]
    fn test_extract_balances_assignment_collection_not_fixed_window() {
        // Hak (loctree-feedback.md, 2026-06-15): a module-level tuple/list/dict const
        // has no `{` fn-body brace, so it fell into the fixed 40-line window and
        // over-captured trailing code. An assignment that opens `(`/`[`/`{` should
        // balance that bracket and stop at its close.
        let src =
            "FRAMEWORK_LAUNCHER_MARKERS = (\n    \"a\",\n    \"b\",\n)\n\nOTHER = 1\nmore = 2";
        let lines: Vec<&str> = src.lines().collect();
        let (end_line, body, truncated, total) = extract_body(&lines, 0, 200, "py");
        assert_eq!(end_line, 4, "tuple closes on line 4 (the `)`)");
        assert_eq!(total, 4);
        assert!(!truncated);
        assert!(body.contains("FRAMEWORK_LAUNCHER_MARKERS"));
        assert!(body.trim_end().ends_with(')'));
        assert!(!body.contains("OTHER"), "must not overshoot past the tuple");
    }

    #[test]
    fn test_extract_def_paren_is_not_treated_as_assignment_collection() {
        // Guard: a `def f(...):` signature paren must NOT trigger bracket
        // balancing (it is not an assignment), keeping the Python def fallback.
        let src = "def thing(a, b):\n    return a + b\n    # trailing";
        let lines: Vec<&str> = src.lines().collect();
        let (_end, body, _truncated, _total) = extract_body(&lines, 0, 200, "py");
        assert!(body.contains("def thing(a, b):"));
        assert!(
            body.contains("return a + b"),
            "def body must still be captured, not just the signature"
        );
    }

    #[test]
    fn test_query_symbol_body_resolves_python_module_const() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let source_path = tmp.path().join("markers.py");
        std::fs::write(
            &source_path,
            "FRAMEWORK_LAUNCHER_MARKERS = (\n    \"vibecrafted\",\n    \"loctree\",\n)\n",
        )
        .expect("write source");

        let mut snapshot = Snapshot::new(vec![tmp.path().to_string_lossy().to_string()]);
        let mut file = crate::types::FileAnalysis::new(source_path.to_string_lossy().to_string());
        file.local_symbols.push(crate::types::LocalSymbol {
            name: "FRAMEWORK_LAUNCHER_MARKERS".to_string(),
            kind: "const".to_string(),
            line: Some(1),
            context: "FRAMEWORK_LAUNCHER_MARKERS = (".to_string(),
            is_exported: false,
        });
        snapshot.files.push(file);

        let result = query_symbol_body(&snapshot, "FRAMEWORK_LAUNCHER_MARKERS", None);
        assert_eq!(result.bodies.len(), 1, "module const must resolve a body");
        assert!(
            result.bodies[0]
                .source
                .contains("FRAMEWORK_LAUNCHER_MARKERS")
        );
        assert!(result.bodies[0].source.contains("loctree"));
        assert_eq!(
            result.bodies[0].end_line, 4,
            "body bounded to the tuple close"
        );
    }

    #[test]
    fn test_query_symbol_body_resolves_rust_impl_method() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let source_path = tmp.path().join("recorder.rs");
        std::fs::write(
            &source_path,
            "struct Recorder;\n\nimpl Recorder {\n    pub fn start(&self) {\n        println!(\"start\");\n    }\n}\n",
        )
        .expect("write source");

        let mut snapshot = Snapshot::new(vec![tmp.path().to_string_lossy().to_string()]);
        let mut file = crate::types::FileAnalysis::new(source_path.to_string_lossy().to_string());
        file.impl_methods.push(crate::types::ImplMethod {
            name: "start".to_string(),
            qualifier: "Recorder".to_string(),
            line: Some(4),
            visibility: crate::types::Visibility::Public,
            ..Default::default()
        });
        snapshot.files.push(file);

        let result = query_symbol_body(&snapshot, "Recorder::start", None);
        assert_eq!(result.bodies.len(), 1);
        assert!(result.bodies[0].source.contains("pub fn start(&self)"));
        assert!(result.bodies[0].source.contains("println!"));
    }
}
