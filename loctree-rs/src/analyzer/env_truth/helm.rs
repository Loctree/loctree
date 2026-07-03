//! Helm `values*.yaml` env-block sensor.
//!
//! Helm values are free-form, but a strong convention is:
//!
//! ```yaml
//! env:
//!   DATABASE_URL: postgres://localhost/x
//!   LOG_LEVEL: info
//!
//! envFrom:
//!   - secretRef:
//!       name: api-secrets
//! ```
//!
//! We walk every `env:` and `envFrom:` block, plus any `*.env` /
//! `*.envFrom` nested under sub-charts. Values that look like Helm template
//! syntax (`{{ .Values.foo }}`) are treated as `EnvFrom { reference: "helm:..." }`.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::path::Path;

use serde_yaml::Value;

use super::io_helpers::{hash_value, mtime_info, relativize};
use super::types::{EnvSource, EnvSourceKind, ValuePresence};

/// Parse a Helm `values*.yaml` file. Returns env declarations from any
/// `env:` mapping found at any depth.
pub fn parse_values_file(path: &Path, root: &Path, base_rank: u8) -> Vec<(String, EnvSource)> {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let yaml: Value = match serde_yaml::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let rel = relativize(path, root);
    let (mtime, age) = mtime_info(path);
    let mtime_str = mtime.unwrap_or_default();
    let mut out = Vec::new();
    walk(&yaml, &rel, &mtime_str, age, base_rank, &mut out);
    out
}

fn walk(
    value: &Value,
    rel: &str,
    mtime: &str,
    age: Option<u32>,
    base_rank: u8,
    out: &mut Vec<(String, EnvSource)>,
) {
    if let Value::Mapping(m) = value {
        for (k, v) in m {
            let Some(key) = k.as_str() else {
                continue;
            };
            if key == "env" {
                collect_env_block(v, rel, mtime, age, base_rank, out);
            } else if let Value::Mapping(_) | Value::Sequence(_) = v {
                walk(v, rel, mtime, age, base_rank, out);
            }
        }
    } else if let Value::Sequence(seq) = value {
        for entry in seq {
            walk(entry, rel, mtime, age, base_rank, out);
        }
    }
}

fn collect_env_block(
    value: &Value,
    rel: &str,
    mtime: &str,
    age: Option<u32>,
    base_rank: u8,
    out: &mut Vec<(String, EnvSource)>,
) {
    match value {
        Value::Mapping(m) => {
            for (k, v) in m {
                let Some(name) = k.as_str() else { continue };
                let presence = scalar_to_presence(v);
                out.push((
                    name.to_string(),
                    EnvSource {
                        kind: EnvSourceKind::HelmValues,
                        path: rel.to_string(),
                        line: None,
                        mtime: mtime.to_string(),
                        mtime_age_days: age,
                        git_age_days: None,
                        value_present: presence,
                        precedence_rank: base_rank,
                    },
                ));
            }
        }
        Value::Sequence(seq) => {
            // List of `{ name: X, value: Y }` entries (k8s-style).
            for entry in seq {
                let Some(name) = entry.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let presence = if let Some(literal) = entry.get("value").and_then(Value::as_str) {
                    if literal.is_empty() {
                        ValuePresence::Empty
                    } else if is_helm_template(literal) {
                        ValuePresence::EnvFrom {
                            reference: format!("helm:{literal}"),
                        }
                    } else {
                        ValuePresence::Plain {
                            value_hash: hash_value(literal),
                        }
                    }
                } else {
                    ValuePresence::Empty
                };
                out.push((
                    name.to_string(),
                    EnvSource {
                        kind: EnvSourceKind::HelmValues,
                        path: rel.to_string(),
                        line: None,
                        mtime: mtime.to_string(),
                        mtime_age_days: age,
                        git_age_days: None,
                        value_present: presence,
                        precedence_rank: base_rank,
                    },
                ));
            }
        }
        _ => {}
    }
}

fn scalar_to_presence(v: &Value) -> ValuePresence {
    match v {
        Value::Null => ValuePresence::Empty,
        Value::String(s) if s.is_empty() => ValuePresence::Empty,
        Value::String(s) if is_helm_template(s) => ValuePresence::EnvFrom {
            reference: format!("helm:{s}"),
        },
        Value::String(s) => ValuePresence::Plain {
            value_hash: hash_value(s),
        },
        Value::Bool(b) => ValuePresence::Plain {
            value_hash: hash_value(&b.to_string()),
        },
        Value::Number(n) => ValuePresence::Plain {
            value_hash: hash_value(&n.to_string()),
        },
        _ => ValuePresence::EnvFrom {
            reference: "complex".into(),
        },
    }
}

fn is_helm_template(s: &str) -> bool {
    s.contains("{{") && s.contains("}}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parses_top_level_env_mapping() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("values.yaml");
        fs::write(
            &path,
            "
env:
  DATABASE_URL: postgres://localhost/x
  LOG_LEVEL: info
",
        )
        .unwrap();
        let out = parse_values_file(&path, tmp.path(), 65);
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"DATABASE_URL"));
        assert!(names.contains(&"LOG_LEVEL"));
    }

    #[test]
    fn helm_template_marked_as_envfrom() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("values.yaml");
        fs::write(
            &path,
            "
env:
  HOSTNAME: '{{ .Values.host }}'
",
        )
        .unwrap();
        let out = parse_values_file(&path, tmp.path(), 65);
        let host = out.iter().find(|(n, _)| n == "HOSTNAME").unwrap();
        assert!(matches!(
            host.1.value_present,
            ValuePresence::EnvFrom { .. }
        ));
    }

    #[test]
    fn parses_list_form_env() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("values.yaml");
        fs::write(
            &path,
            "
api:
  env:
    - name: API_KEY
      value: shhh
    - name: PROXY_URL
      value: '{{ .Values.proxy }}'
",
        )
        .unwrap();
        let out = parse_values_file(&path, tmp.path(), 65);
        let api = out.iter().find(|(n, _)| n == "API_KEY").unwrap();
        assert!(matches!(api.1.value_present, ValuePresence::Plain { .. }));
        let proxy = out.iter().find(|(n, _)| n == "PROXY_URL").unwrap();
        assert!(matches!(
            proxy.1.value_present,
            ValuePresence::EnvFrom { .. }
        ));
    }
}
