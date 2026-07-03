//! `.env*` and `.envrc` declaration sensor.
//!
//! Hand-rolled minimal parser — supports `KEY=value`, `export KEY=value`,
//! single/double-quoted values, line comments (`#`), and continuation
//! across multiple sources via `value_hash`. Reference: dotenv RFC at
//! https://hexdocs.pm/dotenvy/dotenv-file-format.html — we deliberately
//! ignore variable expansion because we never resolve runtime values.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::path::Path;

use super::io_helpers::{hash_value, mtime_info, relativize};
use super::precedence::refine_dotenv_rank;
use super::types::{EnvSource, EnvSourceKind, ValuePresence};

/// Parse a single `.env*` file into its declarations. Lines that fail to
/// parse are silently skipped (logging is the orchestrator's job).
pub fn parse_dotenv_file(
    path: &Path,
    root: &Path,
    base_rank: u8,
    is_envrc: bool,
) -> Vec<(String, EnvSource)> {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let rel = relativize(path, root);
    let (mtime, age) = mtime_info(path);
    let mtime_str = mtime.unwrap_or_default();
    let mut out = Vec::new();
    for (idx, raw_line) in raw.lines().enumerate() {
        let Some((name, value)) = parse_line(raw_line) else {
            continue;
        };
        let presence = if value.is_empty() {
            ValuePresence::Empty
        } else {
            ValuePresence::Plain {
                value_hash: hash_value(&value),
            }
        };
        let kind = if is_envrc {
            EnvSourceKind::EnvRc
        } else {
            EnvSourceKind::DotEnv
        };
        let rank = if is_envrc {
            base_rank
        } else {
            refine_dotenv_rank(base_rank, &rel)
        };
        out.push((
            name,
            EnvSource {
                kind,
                path: rel.clone(),
                line: Some((idx + 1) as u32),
                mtime: mtime_str.clone(),
                mtime_age_days: age,
                git_age_days: None,
                value_present: presence,
                precedence_rank: rank,
            },
        ));
    }
    out
}

/// Parse one `.env`-style line. Returns `Some((name, value))` for valid
/// declarations, `None` for blank/comment lines or malformed input.
fn parse_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let mut rest = trimmed;
    // direnv `export KEY=value`, dotenv allows it too.
    if let Some(stripped) = rest.strip_prefix("export ") {
        rest = stripped.trim_start();
    }
    let eq_idx = rest.find('=')?;
    let name = rest[..eq_idx].trim().to_string();
    if name.is_empty() || !is_valid_env_name(&name) {
        return None;
    }
    let value_raw = rest[eq_idx + 1..].trim();
    let value = strip_quotes(value_raw);
    Some((name, value))
}

fn is_valid_env_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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
    // Strip trailing inline comment for unquoted values: `KEY=val # comment`.
    if let Some(idx) = value.find(" #") {
        return value[..idx].trim().to_string();
    }
    value.to_string()
}

/// `.envrc` support is a thin wrapper around `parse_dotenv_file` — direnv
/// shell scripts can be arbitrarily complex, but the common pattern
/// (`export KEY=value`) is the only one we audit.
pub fn parse_envrc_file(path: &Path, root: &Path, base_rank: u8) -> Vec<(String, EnvSource)> {
    parse_dotenv_file(path, root, base_rank, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parses_simple_keys() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let path = root.join(".env");
        fs::write(
            &path,
            "# header comment\nFOO=bar\nBAZ=\"quoted value\"\nexport BIN=binary\n\n",
        )
        .unwrap();
        let out = parse_dotenv_file(&path, root, 30, false);
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["FOO", "BAZ", "BIN"]);
        // BAZ value is hashed, not literal.
        let baz_value = match &out[1].1.value_present {
            ValuePresence::Plain { value_hash } => value_hash.clone(),
            other => panic!("expected Plain, got {:?}", other),
        };
        assert_eq!(baz_value, hash_value("quoted value"));
    }

    #[test]
    fn skips_invalid_names() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(".env");
        fs::write(&path, "1BAD=x\nGOOD=y\n=missing\n").unwrap();
        let out = parse_dotenv_file(&path, tmp.path(), 30, false);
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["GOOD"]);
    }

    #[test]
    fn empty_value_marked_as_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(".env");
        fs::write(&path, "EMPTY=\n").unwrap();
        let out = parse_dotenv_file(&path, tmp.path(), 30, false);
        assert!(matches!(out[0].1.value_present, ValuePresence::Empty));
    }

    #[test]
    fn example_files_get_lower_rank() {
        let tmp = TempDir::new().unwrap();
        let prod = tmp.path().join(".env.production");
        fs::write(&prod, "X=1\n").unwrap();
        let example = tmp.path().join(".env.example");
        fs::write(&example, "X=2\n").unwrap();
        let prod_out = parse_dotenv_file(&prod, tmp.path(), 30, false);
        let ex_out = parse_dotenv_file(&example, tmp.path(), 30, false);
        assert!(prod_out[0].1.precedence_rank > ex_out[0].1.precedence_rank);
    }
}
