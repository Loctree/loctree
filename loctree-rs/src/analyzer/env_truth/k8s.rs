//! Kubernetes manifest sensor.
//!
//! Handles `Deployment` / `StatefulSet` / `DaemonSet` (env + envFrom),
//! `ConfigMap`, `Secret` (data + stringData), `SealedSecret`, and
//! `ExternalSecret`. Multi-document YAML files (`---` separated) are
//! supported. Sealed/encrypted payloads are NEVER decoded — only their
//! presence and age surface.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_yaml::Value;

use super::io_helpers::{hash_value, mtime_info, relativize};
use super::types::{EnvSource, EnvSourceKind, ValuePresence};

/// Parse a k8s YAML file (possibly multi-document) into env declarations.
///
/// Returns a flat `Vec<(name, EnvSource)>` — one entry per declared env name
/// per document. Documents we don't recognize (CRDs we don't model) are
/// silently skipped.
pub fn parse_k8s_yaml(
    path: &Path,
    root: &Path,
    table: &BTreeMap<EnvSourceKind, u8>,
) -> Vec<(String, EnvSource)> {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let rel = relativize(path, root);
    let (mtime, age) = mtime_info(path);
    let mtime_str = mtime.unwrap_or_default();
    let mut out = Vec::new();

    for doc in serde_yaml::Deserializer::from_str(&raw) {
        let value: Value = match Value::deserialize(doc) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let kind_str = value
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match kind_str {
            "Deployment" | "StatefulSet" | "DaemonSet" | "Job" | "CronJob" | "ReplicaSet"
            | "Pod" => {
                collect_pod_spec_env(&value, &rel, &mtime_str, age, table, &mut out);
            }
            "ConfigMap" => {
                collect_configmap(&value, &rel, &mtime_str, age, table, &mut out);
            }
            "Secret" => {
                collect_secret(&value, &rel, &mtime_str, age, table, &mut out);
            }
            "SealedSecret" => {
                collect_sealed_secret(&value, &rel, &mtime_str, age, table, &mut out);
            }
            "ExternalSecret" => {
                collect_external_secret(&value, &rel, &mtime_str, age, table, &mut out);
            }
            _ => {}
        }
    }
    out
}

fn rank_for(kind: EnvSourceKind, table: &BTreeMap<EnvSourceKind, u8>) -> u8 {
    table.get(&kind).copied().unwrap_or(50)
}

fn collect_pod_spec_env(
    value: &Value,
    rel: &str,
    mtime: &str,
    age: Option<u32>,
    table: &BTreeMap<EnvSourceKind, u8>,
    out: &mut Vec<(String, EnvSource)>,
) {
    // Walk down to `spec.template.spec.containers[]` (workloads) or
    // `spec.containers[]` (Pod). Initialize cursor to the value.
    let containers = pod_containers(value);
    for container in containers {
        if let Some(env) = container.get("env").and_then(Value::as_sequence) {
            for entry in env {
                let Some(name) = entry.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let presence = if let Some(literal) = entry.get("value").and_then(Value::as_str) {
                    if literal.is_empty() {
                        ValuePresence::Empty
                    } else {
                        ValuePresence::Plain {
                            value_hash: hash_value(literal),
                        }
                    }
                } else if let Some(value_from) = entry.get("valueFrom") {
                    ValuePresence::EnvFrom {
                        reference: describe_value_from(value_from),
                    }
                } else {
                    ValuePresence::Empty
                };
                out.push((
                    name.to_string(),
                    EnvSource {
                        kind: EnvSourceKind::K8sDeploymentEnv,
                        path: rel.to_string(),
                        line: None,
                        mtime: mtime.to_string(),
                        mtime_age_days: age,
                        git_age_days: None,
                        value_present: presence,
                        precedence_rank: rank_for(EnvSourceKind::K8sDeploymentEnv, table),
                    },
                ));
            }
        }
        if let Some(env_from) = container.get("envFrom").and_then(Value::as_sequence) {
            for entry in env_from {
                let reference = describe_env_from(entry);
                out.push((
                    "__env_from__".to_string(),
                    EnvSource {
                        kind: EnvSourceKind::K8sDeploymentEnvFrom,
                        path: rel.to_string(),
                        line: None,
                        mtime: mtime.to_string(),
                        mtime_age_days: age,
                        git_age_days: None,
                        value_present: ValuePresence::EnvFrom { reference },
                        precedence_rank: rank_for(EnvSourceKind::K8sDeploymentEnvFrom, table),
                    },
                ));
            }
        }
    }
}

fn pod_containers(value: &Value) -> Vec<&Value> {
    let mut out = Vec::new();
    let direct = value
        .get("spec")
        .and_then(|s| s.get("containers"))
        .and_then(Value::as_sequence);
    if let Some(seq) = direct {
        out.extend(seq.iter());
        return out;
    }
    let templated = value
        .get("spec")
        .and_then(|s| s.get("template"))
        .and_then(|t| t.get("spec"))
        .and_then(|s| s.get("containers"))
        .and_then(Value::as_sequence);
    if let Some(seq) = templated {
        out.extend(seq.iter());
    }
    // CronJob nests one extra level: spec.jobTemplate.spec.template.spec.containers
    let cron = value
        .get("spec")
        .and_then(|s| s.get("jobTemplate"))
        .and_then(|jt| jt.get("spec"))
        .and_then(|s| s.get("template"))
        .and_then(|t| t.get("spec"))
        .and_then(|s| s.get("containers"))
        .and_then(Value::as_sequence);
    if let Some(seq) = cron {
        out.extend(seq.iter());
    }
    out
}

fn describe_value_from(value_from: &Value) -> String {
    if let Some(cm) = value_from.get("configMapKeyRef") {
        let name = cm.get("name").and_then(Value::as_str).unwrap_or("?");
        let key = cm.get("key").and_then(Value::as_str).unwrap_or("?");
        return format!("configMapKeyRef:{name}.{key}");
    }
    if let Some(sec) = value_from.get("secretKeyRef") {
        let name = sec.get("name").and_then(Value::as_str).unwrap_or("?");
        let key = sec.get("key").and_then(Value::as_str).unwrap_or("?");
        return format!("secretKeyRef:{name}.{key}");
    }
    if let Some(field) = value_from.get("fieldRef") {
        let path = field
            .get("fieldPath")
            .and_then(Value::as_str)
            .unwrap_or("?");
        return format!("fieldRef:{path}");
    }
    "valueFrom:?".into()
}

fn describe_env_from(entry: &Value) -> String {
    if let Some(cm) = entry.get("configMapRef") {
        let name = cm.get("name").and_then(Value::as_str).unwrap_or("?");
        return format!("configMapRef:{name}");
    }
    if let Some(sec) = entry.get("secretRef") {
        let name = sec.get("name").and_then(Value::as_str).unwrap_or("?");
        return format!("secretRef:{name}");
    }
    "envFrom:?".into()
}

fn collect_configmap(
    value: &Value,
    rel: &str,
    mtime: &str,
    age: Option<u32>,
    table: &BTreeMap<EnvSourceKind, u8>,
    out: &mut Vec<(String, EnvSource)>,
) {
    let Some(data) = value.get("data").and_then(Value::as_mapping) else {
        return;
    };
    for (k, v) in data {
        let Some(name) = k.as_str() else { continue };
        if !looks_like_env_name(name) {
            continue;
        }
        let presence = match v {
            Value::String(s) if s.is_empty() => ValuePresence::Empty,
            Value::String(s) => ValuePresence::Plain {
                value_hash: hash_value(s),
            },
            Value::Null => ValuePresence::Empty,
            _ => ValuePresence::Plain {
                value_hash: hash_value(&serde_yaml::to_string(v).unwrap_or_default()),
            },
        };
        out.push((
            name.to_string(),
            EnvSource {
                kind: EnvSourceKind::K8sConfigMap,
                path: rel.to_string(),
                line: None,
                mtime: mtime.to_string(),
                mtime_age_days: age,
                git_age_days: None,
                value_present: presence,
                precedence_rank: rank_for(EnvSourceKind::K8sConfigMap, table),
            },
        ));
    }
}

fn collect_secret(
    value: &Value,
    rel: &str,
    mtime: &str,
    age: Option<u32>,
    table: &BTreeMap<EnvSourceKind, u8>,
    out: &mut Vec<(String, EnvSource)>,
) {
    if let Some(string_data) = value.get("stringData").and_then(Value::as_mapping) {
        for (k, v) in string_data {
            let Some(name) = k.as_str() else { continue };
            if !looks_like_env_name(name) {
                continue;
            }
            let presence = match v.as_str() {
                Some(s) if !s.is_empty() => ValuePresence::Plain {
                    value_hash: hash_value(s),
                },
                _ => ValuePresence::Empty,
            };
            out.push((
                name.to_string(),
                EnvSource {
                    kind: EnvSourceKind::K8sSecretStringData,
                    path: rel.to_string(),
                    line: None,
                    mtime: mtime.to_string(),
                    mtime_age_days: age,
                    git_age_days: None,
                    value_present: presence,
                    precedence_rank: rank_for(EnvSourceKind::K8sSecretStringData, table),
                },
            ));
        }
    }
    if let Some(data) = value.get("data").and_then(Value::as_mapping) {
        for (k, _v) in data {
            let Some(name) = k.as_str() else { continue };
            if !looks_like_env_name(name) {
                continue;
            }
            // NEVER decode base64. Mark as Secret presence.
            out.push((
                name.to_string(),
                EnvSource {
                    kind: EnvSourceKind::K8sSecret,
                    path: rel.to_string(),
                    line: None,
                    mtime: mtime.to_string(),
                    mtime_age_days: age,
                    git_age_days: None,
                    value_present: ValuePresence::Secret,
                    precedence_rank: rank_for(EnvSourceKind::K8sSecret, table),
                },
            ));
        }
    }
}

fn collect_sealed_secret(
    value: &Value,
    rel: &str,
    mtime: &str,
    age: Option<u32>,
    table: &BTreeMap<EnvSourceKind, u8>,
    out: &mut Vec<(String, EnvSource)>,
) {
    let encrypted = value
        .get("spec")
        .and_then(|s| s.get("encryptedData"))
        .and_then(Value::as_mapping);
    let Some(encrypted) = encrypted else {
        return;
    };
    for (k, _v) in encrypted {
        let Some(name) = k.as_str() else { continue };
        if !looks_like_env_name(name) {
            continue;
        }
        out.push((
            name.to_string(),
            EnvSource {
                kind: EnvSourceKind::SealedSecret,
                path: rel.to_string(),
                line: None,
                mtime: mtime.to_string(),
                mtime_age_days: age,
                git_age_days: None,
                value_present: ValuePresence::Encrypted {
                    marker: "SealedSecret".into(),
                },
                precedence_rank: rank_for(EnvSourceKind::SealedSecret, table),
            },
        ));
    }
}

fn collect_external_secret(
    value: &Value,
    rel: &str,
    mtime: &str,
    age: Option<u32>,
    table: &BTreeMap<EnvSourceKind, u8>,
    out: &mut Vec<(String, EnvSource)>,
) {
    let data = value
        .get("spec")
        .and_then(|s| s.get("data"))
        .and_then(Value::as_sequence);
    if let Some(data) = data {
        for entry in data {
            if let Some(secret_key) = entry.get("secretKey").and_then(Value::as_str) {
                if !looks_like_env_name(secret_key) {
                    continue;
                }
                let remote = entry
                    .get("remoteRef")
                    .and_then(|r| r.get("key"))
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                out.push((
                    secret_key.to_string(),
                    EnvSource {
                        kind: EnvSourceKind::ExternalSecret,
                        path: rel.to_string(),
                        line: None,
                        mtime: mtime.to_string(),
                        mtime_age_days: age,
                        git_age_days: None,
                        value_present: ValuePresence::EnvFrom {
                            reference: format!("externalSecret:{remote}"),
                        },
                        precedence_rank: rank_for(EnvSourceKind::ExternalSecret, table),
                    },
                ));
            }
        }
    }
}

/// Heuristic: env names are `[A-Z_][A-Z0-9_]*`. ConfigMaps store many
/// non-env keys (e.g. `config.json`) — filter those out so the report
/// stays signal-rich.
fn looks_like_env_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_uppercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn default_table() -> BTreeMap<EnvSourceKind, u8> {
        super::super::precedence::default_table()
    }

    #[test]
    fn parses_deployment_env() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("deploy.yaml");
        fs::write(
            &path,
            "
apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
spec:
  template:
    spec:
      containers:
        - name: api
          env:
            - name: DATABASE_URL
              value: postgres://x
            - name: SECRET_REF
              valueFrom:
                secretKeyRef:
                  name: api-secrets
                  key: token
",
        )
        .unwrap();
        let out = parse_k8s_yaml(&path, tmp.path(), &default_table());
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"DATABASE_URL"));
        assert!(names.contains(&"SECRET_REF"));
        let secret = out.iter().find(|(n, _)| n == "SECRET_REF").unwrap();
        assert!(matches!(
            secret.1.value_present,
            ValuePresence::EnvFrom { .. }
        ));
    }

    #[test]
    fn parses_configmap_filters_non_env_keys() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("cm.yaml");
        fs::write(
            &path,
            "
apiVersion: v1
kind: ConfigMap
metadata:
  name: api-config
data:
  DATABASE_URL: postgres://x
  config.json: '{}'
  log-level: debug
",
        )
        .unwrap();
        let out = parse_k8s_yaml(&path, tmp.path(), &default_table());
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"DATABASE_URL"));
        assert!(!names.contains(&"config.json"));
        assert!(!names.contains(&"log-level"));
    }

    #[test]
    fn sealed_secret_never_decodes() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("sealed.yaml");
        fs::write(
            &path,
            "
apiVersion: bitnami.com/v1alpha1
kind: SealedSecret
metadata:
  name: api-creds
spec:
  encryptedData:
    DATABASE_URL: AgBxxx...verylongciphertext...
    API_KEY: AgByyy...verylongciphertext...
",
        )
        .unwrap();
        let out = parse_k8s_yaml(&path, tmp.path(), &default_table());
        let db = out.iter().find(|(n, _)| n == "DATABASE_URL").unwrap();
        match &db.1.value_present {
            ValuePresence::Encrypted { marker } => assert_eq!(marker, "SealedSecret"),
            other => panic!("expected Encrypted, got {:?}", other),
        }
    }

    #[test]
    fn secret_data_marked_as_secret_not_decoded() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("secret.yaml");
        fs::write(
            &path,
            "
apiVersion: v1
kind: Secret
metadata:
  name: creds
type: Opaque
data:
  TOKEN: cGxhaW4tdGV4dA==
stringData:
  PLAIN_KEY: visible
",
        )
        .unwrap();
        let out = parse_k8s_yaml(&path, tmp.path(), &default_table());
        let token = out.iter().find(|(n, _)| n == "TOKEN").unwrap();
        assert!(matches!(token.1.value_present, ValuePresence::Secret));
        let plain = out.iter().find(|(n, _)| n == "PLAIN_KEY").unwrap();
        assert!(matches!(plain.1.value_present, ValuePresence::Plain { .. }));
    }
}
