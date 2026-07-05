//! `docker-compose*.yml` / `compose.yaml` declaration sensor.
//!
//! Walks each service's `environment:` (literal map or list form) and
//! `env_file:` (one or many .env paths). For env_file references we record a
//! `EnvFrom` source so the orchestrator can chain into the dotenv parser.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::path::Path;

use serde_yaml::Value;

use super::io_helpers::{hash_value, mtime_info, relativize};
use super::types::{EnvSource, EnvSourceKind, ValuePresence};

/// Parse a docker-compose YAML file. Returns:
/// - inline declarations (`environment:`)
/// - referenced env_file paths (resolved relative to the compose file dir)
pub fn parse_compose_file(
    path: &Path,
    root: &Path,
    base_rank: u8,
) -> (Vec<(String, EnvSource)>, Vec<std::path::PathBuf>) {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(_) => return (Vec::new(), Vec::new()),
    };
    let yaml: Value = match serde_yaml::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return (Vec::new(), Vec::new()),
    };
    let rel = relativize(path, root);
    let (mtime, age) = mtime_info(path);
    let mtime_str = mtime.unwrap_or_default();
    let mut inline = Vec::new();
    let mut env_file_refs = Vec::new();
    let dir = path.parent().unwrap_or(root);

    let services = yaml.get("services").and_then(Value::as_mapping);
    if let Some(services) = services {
        for (_svc_name, svc_value) in services {
            let Some(svc) = svc_value.as_mapping() else {
                continue;
            };
            // environment: literal
            if let Some(env) = svc.get(Value::String("environment".into())) {
                collect_environment(
                    env,
                    &rel,
                    &mtime_str,
                    age,
                    base_rank,
                    EnvSourceKind::DockerCompose,
                    &mut inline,
                );
            }
            // env_file: single string or list
            if let Some(env_file) = svc.get(Value::String("env_file".into())) {
                match env_file {
                    Value::String(s) => {
                        let resolved = dir.join(s);
                        env_file_refs.push(resolved);
                        // Record the REFERENCE itself as a declaration source
                        // for transparency (without an env name we can only
                        // attach it later, so we add a sentinel only when
                        // chained — here we just expose the path).
                        push_env_file_reference(s, &rel, &mtime_str, age, base_rank, &mut inline);
                    }
                    Value::Sequence(seq) => {
                        for entry in seq {
                            if let Some(s) = entry.as_str() {
                                let resolved = dir.join(s);
                                env_file_refs.push(resolved);
                                push_env_file_reference(
                                    s,
                                    &rel,
                                    &mtime_str,
                                    age,
                                    base_rank,
                                    &mut inline,
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    (inline, env_file_refs)
}

fn collect_environment(
    value: &Value,
    rel_path: &str,
    mtime: &str,
    age: Option<u32>,
    base_rank: u8,
    kind: EnvSourceKind,
    out: &mut Vec<(String, EnvSource)>,
) {
    match value {
        Value::Mapping(m) => {
            for (k, v) in m {
                let Some(key) = k.as_str() else {
                    continue;
                };
                let presence = scalar_to_presence(v);
                out.push((
                    key.to_string(),
                    EnvSource {
                        kind,
                        path: rel_path.to_string(),
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
            for entry in seq {
                if let Some(s) = entry.as_str() {
                    if let Some((name, val)) = s.split_once('=') {
                        let presence = if val.is_empty() {
                            ValuePresence::Empty
                        } else {
                            ValuePresence::Plain {
                                value_hash: hash_value(val),
                            }
                        };
                        out.push((
                            name.trim().to_string(),
                            EnvSource {
                                kind,
                                path: rel_path.to_string(),
                                line: None,
                                mtime: mtime.to_string(),
                                mtime_age_days: age,
                                git_age_days: None,
                                value_present: presence,
                                precedence_rank: base_rank,
                            },
                        ));
                    } else {
                        // `KEY` only — value comes from host env at runtime.
                        out.push((
                            s.to_string(),
                            EnvSource {
                                kind,
                                path: rel_path.to_string(),
                                line: None,
                                mtime: mtime.to_string(),
                                mtime_age_days: age,
                                git_age_days: None,
                                value_present: ValuePresence::EnvFrom {
                                    reference: "host".into(),
                                },
                                precedence_rank: base_rank,
                            },
                        ));
                    }
                }
            }
        }
        _ => {}
    }
}

fn scalar_to_presence(v: &Value) -> ValuePresence {
    match v {
        Value::Null => ValuePresence::Empty,
        Value::String(s) if s.is_empty() => ValuePresence::Empty,
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

fn push_env_file_reference(
    file_ref: &str,
    rel_path: &str,
    mtime: &str,
    age: Option<u32>,
    base_rank: u8,
    out: &mut Vec<(String, EnvSource)>,
) {
    // We cannot know which keys this env_file references without reading it.
    // We push a synthetic `__env_file__` declaration with a reference value
    // so the orchestrator can re-issue dotenv parsing at the resolved path.
    // (The synthetic name is filtered out before the report is emitted.)
    out.push((
        "__env_file__".to_string(),
        EnvSource {
            kind: EnvSourceKind::DockerComposeEnvFile,
            path: rel_path.to_string(),
            line: None,
            mtime: mtime.to_string(),
            mtime_age_days: age,
            git_age_days: None,
            value_present: ValuePresence::EnvFrom {
                reference: file_ref.to_string(),
            },
            precedence_rank: base_rank,
        },
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parses_environment_mapping() {
        let tmp = TempDir::new().unwrap();
        let compose = tmp.path().join("docker-compose.yml");
        fs::write(
            &compose,
            "
services:
  api:
    image: foo
    environment:
      DATABASE_URL: postgres://localhost/x
      LOG_LEVEL: debug
",
        )
        .unwrap();
        let (decls, refs) = parse_compose_file(&compose, tmp.path(), 50);
        assert!(refs.is_empty());
        let names: Vec<&str> = decls.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"DATABASE_URL"));
        assert!(names.contains(&"LOG_LEVEL"));
    }

    #[test]
    fn parses_environment_list_form() {
        let tmp = TempDir::new().unwrap();
        let compose = tmp.path().join("docker-compose.yml");
        fs::write(
            &compose,
            "
services:
  worker:
    image: x
    environment:
      - REDIS_URL=redis://r:6379
      - HOST_INHERITED
",
        )
        .unwrap();
        let (decls, _refs) = parse_compose_file(&compose, tmp.path(), 50);
        let redis = decls.iter().find(|(n, _)| n == "REDIS_URL").unwrap();
        assert!(matches!(redis.1.value_present, ValuePresence::Plain { .. }));
        let host = decls.iter().find(|(n, _)| n == "HOST_INHERITED").unwrap();
        assert!(matches!(
            host.1.value_present,
            ValuePresence::EnvFrom { .. }
        ));
    }

    #[test]
    fn extracts_env_file_references() {
        let tmp = TempDir::new().unwrap();
        let compose = tmp.path().join("docker-compose.yml");
        fs::write(
            &compose,
            "
services:
  api:
    image: foo
    env_file:
      - ./.env
      - ./.env.production
",
        )
        .unwrap();
        let (decls, refs) = parse_compose_file(&compose, tmp.path(), 50);
        assert_eq!(refs.len(), 2);
        // Synthetic markers for the env_file references appear in decls.
        let synth: Vec<&str> = decls.iter().map(|(n, _)| n.as_str()).collect();
        assert!(synth.contains(&"__env_file__"));
    }
}
