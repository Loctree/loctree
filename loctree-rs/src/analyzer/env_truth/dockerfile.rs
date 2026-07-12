//! `Dockerfile` `ENV` directive sensor.
//!
//! Supports both forms:
//! - `ENV KEY=value` (single-line, multiple `KEY=value` pairs allowed)
//! - `ENV KEY value` (legacy single-pair form)
//!
//! Also handles backslash line-continuation.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::path::Path;

use super::io_helpers::{hash_value, mtime_info, relativize};
use super::types::{EnvSource, EnvSourceKind, ValuePresence};

/// Parse a Dockerfile and return every `(name, EnvSource)` pair found in
/// `ENV` directives.
pub fn parse_dockerfile(path: &Path, root: &Path, base_rank: u8) -> Vec<(String, EnvSource)> {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let rel = relativize(path, root);
    let (mtime, age) = mtime_info(path);
    let mtime_str = mtime.unwrap_or_default();

    // Collapse line-continuations so `ENV X=1 \\` joins with the next line.
    let collapsed = collapse_continuations(&raw);
    let mut out = Vec::new();
    for (idx, line) in collapsed.iter().enumerate() {
        let trimmed = line.trim();
        if !trimmed.starts_with("ENV ") && !trimmed.starts_with("ENV\t") {
            continue;
        }
        let body = trimmed[3..].trim_start();
        let pairs = parse_env_directive(body);
        let line_no = (idx + 1) as u32;
        for (name, value) in pairs {
            let presence = if value.is_empty() {
                ValuePresence::Empty
            } else {
                ValuePresence::Plain {
                    value_hash: hash_value(&value),
                }
            };
            out.push((
                name,
                EnvSource {
                    kind: EnvSourceKind::Dockerfile,
                    path: rel.clone(),
                    line: Some(line_no),
                    mtime: mtime_str.clone(),
                    mtime_age_days: age,
                    git_age_days: None,
                    value_present: presence,
                    precedence_rank: base_rank,
                },
            ));
        }
    }
    out
}

/// Join lines that end with a backslash with the next line, preserving
/// original line numbers (the joined line keeps the index of the *first*
/// physical line so reported line numbers are predictable).
fn collapse_continuations(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut continuing = false;
    for line in raw.lines() {
        if continuing {
            // Append to last logical line buffer.
            buf.push(' ');
            buf.push_str(line.trim_end_matches('\\').trim());
            continuing = line.trim_end().ends_with('\\');
            if !continuing {
                if let Some(last) = out.last_mut() {
                    last.push_str(&buf);
                }
                buf.clear();
            }
            continue;
        }
        let mut s = line.to_string();
        if s.trim_end().ends_with('\\') {
            s = s.trim_end().trim_end_matches('\\').to_string();
            buf.clear();
            continuing = true;
        }
        out.push(s);
    }
    // Flush any in-progress continuation at end of file.
    if continuing && let Some(last) = out.last_mut() {
        last.push_str(&buf);
    }
    out
}

/// Parse the body of an `ENV` directive into `(key, value)` pairs.
///
/// Accepts:
/// - `KEY=value` (multiple allowed, separated by whitespace)
/// - `KEY value` (legacy: rest-of-line is the value)
fn parse_env_directive(body: &str) -> Vec<(String, String)> {
    let body = body.trim();
    if body.is_empty() {
        return Vec::new();
    }
    if body.contains('=') && !body.starts_with(|c: char| c.is_whitespace()) {
        return parse_kv_form(body);
    }
    // Legacy form: first whitespace splits key from value.
    let mut parts = body.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("").trim();
    let value = parts.next().unwrap_or("").trim().to_string();
    if name.is_empty() {
        return Vec::new();
    }
    vec![(name.to_string(), strip_quotes(&value))]
}

fn parse_kv_form(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = body.char_indices().peekable();
    let mut start = 0usize;
    while let Some((i, c)) = chars.next() {
        if c == '=' {
            let key = body[start..i].trim().to_string();
            // Find value: quoted or whitespace-terminated.
            let value_start = i + 1;
            let (value, end) = read_value(body, value_start);
            if !key.is_empty() {
                out.push((key, value));
            }
            // Skip whitespace before next pair.
            start = end;
            while let Some(&(j, ch)) = chars.peek() {
                if ch.is_whitespace() {
                    chars.next();
                    start = j + ch.len_utf8();
                } else {
                    break;
                }
            }
        }
    }
    out
}

fn read_value(s: &str, start: usize) -> (String, usize) {
    let bytes = s.as_bytes();
    if start >= bytes.len() {
        return (String::new(), bytes.len());
    }
    let first = bytes[start];
    if first == b'"' || first == b'\'' {
        // Quoted value: read until matching unescaped quote.
        let quote = first;
        let mut i = start + 1;
        while i < bytes.len() {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if bytes[i] == quote {
                return (s[start + 1..i].to_string(), i + 1);
            }
            i += 1;
        }
        return (s[start + 1..].to_string(), bytes.len());
    }
    let mut i = start;
    while i < bytes.len() && !(bytes[i] as char).is_whitespace() {
        i += 1;
    }
    (s[start..i].to_string(), i)
}

fn strip_quotes(value: &str) -> String {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parses_kv_form_with_multiple_pairs() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("Dockerfile");
        fs::write(
            &path,
            "FROM debian\nENV NODE_ENV=production API_PORT=3000\nENV LANG en_US.UTF-8\n",
        )
        .unwrap();
        let out = parse_dockerfile(&path, tmp.path(), 40);
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"NODE_ENV"));
        assert!(names.contains(&"API_PORT"));
        assert!(names.contains(&"LANG"));
    }

    #[test]
    fn handles_quoted_values() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("Dockerfile");
        fs::write(&path, "ENV PROMPT=\"hello world\" SECRET='do not leak'\n").unwrap();
        let out = parse_dockerfile(&path, tmp.path(), 40);
        let prompt = out.iter().find(|(n, _)| n == "PROMPT").unwrap();
        let secret = out.iter().find(|(n, _)| n == "SECRET").unwrap();
        match &prompt.1.value_present {
            ValuePresence::Plain { value_hash } => {
                assert_eq!(*value_hash, hash_value("hello world"))
            }
            other => panic!("unexpected: {:?}", other),
        }
        assert!(matches!(
            &secret.1.value_present,
            ValuePresence::Plain { .. }
        ));
    }

    #[test]
    fn continuation_at_eof_does_not_drop_last_line() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("Dockerfile");
        // File ends with backslash continuation — no trailing newline after the last segment.
        fs::write(&path, "FROM debian\nENV FOO=bar \\\n    BAZ=qux").unwrap();
        let out = parse_dockerfile(&path, tmp.path(), 40);
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"FOO"),
            "FOO should be present; got: {:?}",
            names
        );
        assert!(
            names.contains(&"BAZ"),
            "BAZ should be present; got: {:?}",
            names
        );
    }
}
