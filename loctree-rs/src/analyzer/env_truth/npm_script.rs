//! `package.json` script env-prefix sensor.
//!
//! Detects the common `KEY=value KEY2=value2 npm run x` pattern in
//! `scripts.*` entries. Only literal prefixes are recognized — shell
//! constructs (`$(...)`, `${VAR}`) are recorded as `EnvFrom`.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::path::Path;

use serde_json::Value;

use super::io_helpers::{hash_value, mtime_info, relativize};
use super::types::{EnvSource, EnvSourceKind, ValuePresence};

pub fn parse_package_json(path: &Path, root: &Path, base_rank: u8) -> Vec<(String, EnvSource)> {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let pkg: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let scripts = match pkg.get("scripts").and_then(Value::as_object) {
        Some(s) => s,
        None => return Vec::new(),
    };
    let rel = relativize(path, root);
    let (mtime, age) = mtime_info(path);
    let mtime_str = mtime.unwrap_or_default();
    let mut out = Vec::new();
    for (_name, body) in scripts {
        let Some(text) = body.as_str() else { continue };
        for (key, value) in extract_prefix_assignments(text) {
            let presence = if value.starts_with('$') {
                ValuePresence::EnvFrom {
                    reference: value.clone(),
                }
            } else if value.is_empty() {
                ValuePresence::Empty
            } else {
                ValuePresence::Plain {
                    value_hash: hash_value(&value),
                }
            };
            out.push((
                key,
                EnvSource {
                    kind: EnvSourceKind::NpmScript,
                    path: rel.clone(),
                    line: None,
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

/// Collect leading `KEY=value` pairs from a shell-ish script body. Stops at
/// the first non-assignment token.
fn extract_prefix_assignments(script: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for token in script.split_whitespace() {
        let Some(eq) = token.find('=') else {
            break;
        };
        let key = &token[..eq];
        if !is_valid_env_name(key) {
            break;
        }
        let value = strip_quotes(&token[eq + 1..]);
        out.push((key.to_string(), value));
    }
    out
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
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn extracts_kv_prefix_from_scripts() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("package.json");
        fs::write(
            &path,
            r#"{
  "name": "x",
  "scripts": {
    "build": "NODE_ENV=production API_PORT=3000 next build",
    "dev": "NODE_ENV=development next dev"
  }
}"#,
        )
        .unwrap();
        let out = parse_package_json(&path, tmp.path(), 35);
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"NODE_ENV"));
        assert!(names.contains(&"API_PORT"));
    }

    #[test]
    fn shell_substitution_marked_as_envfrom() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("package.json");
        fs::write(
            &path,
            r#"{ "scripts": { "run": "TOKEN=$REMOTE_TOKEN node app.js" } }"#,
        )
        .unwrap();
        let out = parse_package_json(&path, tmp.path(), 35);
        let token = out.iter().find(|(n, _)| n == "TOKEN").unwrap();
        assert!(matches!(
            token.1.value_present,
            ValuePresence::EnvFrom { .. }
        ));
    }
}
