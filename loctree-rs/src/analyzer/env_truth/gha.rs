//! GitHub Actions workflow sensor — `.github/workflows/*.yml`.
//!
//! Walks workflow-level, job-level, and step-level `env:` blocks. Every
//! `${{ secrets.NAME }}` reference in any string value is also surfaced
//! as a GitHubActionsSecret declaration so an audit can spot which
//! workflow exposes which secret name (the secret value of course is
//! never available at scan time).
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::collections::HashSet;
use std::path::Path;

use regex::Regex;
use serde_yaml::Value;

use super::io_helpers::{hash_value, mtime_info, relativize};
use super::types::{EnvReadSite, EnvSource, EnvSourceKind, ValuePresence};

/// Parse a GitHub Actions workflow file. Emits two source kinds:
/// `GitHubActionsEnv` (literal `env:` declarations) and `GitHubActionsSecret`
/// (every distinct `secrets.NAME` reference seen anywhere).
pub fn parse_workflow_file(
    path: &Path,
    root: &Path,
    env_rank: u8,
    secret_rank: u8,
) -> Vec<(String, EnvSource)> {
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
    let mut secret_names: HashSet<String> = HashSet::new();

    // Top-level env.
    if let Some(env) = yaml.get("env").and_then(Value::as_mapping) {
        push_env_mapping(env, &rel, &mtime_str, age, env_rank, &mut out);
    }

    // jobs.<id>.env, steps[].env
    if let Some(jobs) = yaml.get("jobs").and_then(Value::as_mapping) {
        for (_id, job_value) in jobs {
            let Some(job) = job_value.as_mapping() else {
                continue;
            };
            if let Some(env) = job
                .get(Value::String("env".into()))
                .and_then(Value::as_mapping)
            {
                push_env_mapping(env, &rel, &mtime_str, age, env_rank, &mut out);
            }
            if let Some(steps) = job
                .get(Value::String("steps".into()))
                .and_then(Value::as_sequence)
            {
                for step in steps {
                    if let Some(env) = step.get("env").and_then(Value::as_mapping) {
                        push_env_mapping(env, &rel, &mtime_str, age, env_rank, &mut out);
                    }
                }
            }
        }
    }

    // Secret references — scan all string values recursively for `${{ secrets.X }}`.
    let secret_re =
        Regex::new(r"\$\{\{\s*secrets\.([A-Z_][A-Z0-9_]*)\s*\}\}").expect("static regex compiles");
    walk_strings(&yaml, &mut |s| {
        for cap in secret_re.captures_iter(s) {
            if let Some(name) = cap.get(1) {
                secret_names.insert(name.as_str().to_string());
            }
        }
    });
    for name in secret_names {
        out.push((
            name,
            EnvSource {
                kind: EnvSourceKind::GitHubActionsSecret,
                path: rel.clone(),
                line: None,
                mtime: mtime_str.clone(),
                mtime_age_days: age,
                git_age_days: None,
                value_present: ValuePresence::EnvFrom {
                    reference: "github_secret".into(),
                },
                precedence_rank: secret_rank,
            },
        ));
    }

    out
}

fn push_env_mapping(
    env: &serde_yaml::Mapping,
    rel: &str,
    mtime: &str,
    age: Option<u32>,
    rank: u8,
    out: &mut Vec<(String, EnvSource)>,
) {
    for (k, v) in env {
        let Some(name) = k.as_str() else { continue };
        let presence = match v {
            Value::String(s) if s.is_empty() => ValuePresence::Empty,
            Value::String(s) if s.contains("${{") => ValuePresence::EnvFrom {
                reference: s.to_string(),
            },
            Value::String(s) => ValuePresence::Plain {
                value_hash: hash_value(s),
            },
            Value::Null => ValuePresence::Empty,
            Value::Bool(b) => ValuePresence::Plain {
                value_hash: hash_value(&b.to_string()),
            },
            Value::Number(n) => ValuePresence::Plain {
                value_hash: hash_value(&n.to_string()),
            },
            _ => ValuePresence::EnvFrom {
                reference: "complex".into(),
            },
        };
        out.push((
            name.to_string(),
            EnvSource {
                kind: EnvSourceKind::GitHubActionsEnv,
                path: rel.to_string(),
                line: None,
                mtime: mtime.to_string(),
                mtime_age_days: age,
                git_age_days: None,
                value_present: presence,
                precedence_rank: rank,
            },
        ));
    }
}

fn walk_strings<F: FnMut(&str)>(value: &Value, f: &mut F) {
    match value {
        Value::String(s) => f(s),
        Value::Mapping(m) => {
            for (_, v) in m {
                walk_strings(v, f);
            }
        }
        Value::Sequence(seq) => {
            for entry in seq {
                walk_strings(entry, f);
            }
        }
        _ => {}
    }
}

/// Scan a GitHub Actions workflow's shell `run:` blocks for `$VAR` /
/// `${VAR}` references. Returns `(name, EnvReadSite)` pairs that the
/// orchestrator merges into the declaration read index so an env var
/// declared in the same workflow's `env:` block does not trigger a false
/// `orphan-declaration` warning.
///
/// Past incident (Screenscribe 2026-05-18): `HEALTH_THRESHOLD` declared at
/// `env:` line 16 of `loctree-ci.yml` and read three times in the same
/// file's shell `run:` block (lines 72/73/76). `env-truth` reported
/// `orphan-declaration: declared but never read` because read-detection
/// only walked `semantic_facts.env_contracts` (code-side reads). Cross-block
/// same-file references in shell `run:` steps were invisible.
///
/// Stays literal: only matches `$NAME` and `${NAME}` shell expansion. Does
/// not attempt to track `GITHUB_ENV` / `$GITHUB_OUTPUT` propagation across
/// steps — that is out of scope and architectural.
///
/// W2-c assignment-scope predicate (example-app CI-vars regression): a `$VAR`
/// reference counts as a read ONLY when no `run:` block in the same workflow
/// assigns `VAR=` itself and VAR is not a runner-provided builtin
/// (`GITHUB_*`, `RUNNER_*`, `CI`, ...). `BASE_REF="${1:-main}"` followed by
/// `$BASE_REF` is shell-local data flow, and `$GITHUB_ENV` exists on every
/// runner regardless of any declaration file.
pub fn parse_workflow_shell_reads(path: &Path, root: &Path) -> Vec<(String, EnvReadSite)> {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let yaml: Value = match serde_yaml::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let rel = relativize(path, root);

    let mut out: Vec<(String, EnvReadSite)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // Two-form regex: bare `$NAME` and bracketed `${NAME}`. Exclude leading
    // `${{` (GHA expression syntax) so we do not pick up `${{ env.X }}`
    // here — those are GHA template expressions, not POSIX shell reads.
    let shell_var = Regex::new(r"\$\{([A-Z_][A-Z0-9_]*)\}|\$([A-Z_][A-Z0-9_]*)")
        .expect("static regex compiles");

    let Some(jobs) = yaml.get("jobs").and_then(Value::as_mapping) else {
        return out;
    };

    // Pass 1: collect shell-local assignments across every `run:` block in
    // the file (steps share a workflow file even if not a process — file
    // scope keeps the predicate simple and kills the example-app false positives).
    let mut assigned: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for_each_run_block(jobs, &mut |run| {
        let cleaned = strip_gha_expressions(run);
        assigned.extend(crate::semantic::shell::collect_assigned_shell_vars(
            &cleaned,
        ));
    });

    for (_id, job_value) in jobs {
        let Some(job) = job_value.as_mapping() else {
            continue;
        };
        let Some(steps) = job
            .get(Value::String("steps".into()))
            .and_then(Value::as_sequence)
        else {
            continue;
        };
        for step in steps {
            let Some(run) = step.get("run").and_then(Value::as_str) else {
                continue;
            };
            // Skip `${{ ... }}` GHA expressions before regex scan so a
            // bracketed name inside that pattern is not double-counted.
            let cleaned = strip_gha_expressions(run);
            for cap in shell_var.captures_iter(&cleaned) {
                let name = cap
                    .get(1)
                    .or_else(|| cap.get(2))
                    .map(|m| m.as_str().to_string());
                if let Some(name) = name {
                    if assigned.contains(&name)
                        || crate::semantic::shell::is_shell_runtime_var(&name)
                    {
                        continue;
                    }
                    let key = format!("{name}::{rel}");
                    if seen.insert(key) {
                        out.push((
                            name,
                            EnvReadSite {
                                file: rel.clone(),
                                line: None,
                                symbol: None,
                                required_for: vec!["github_actions_workflow".into()],
                            },
                        ));
                    }
                }
            }
        }
    }

    out
}

/// Visit every `jobs.<id>.steps[].run` string in the workflow.
fn for_each_run_block<F: FnMut(&str)>(jobs: &serde_yaml::Mapping, f: &mut F) {
    for (_id, job_value) in jobs {
        let Some(job) = job_value.as_mapping() else {
            continue;
        };
        let Some(steps) = job
            .get(Value::String("steps".into()))
            .and_then(Value::as_sequence)
        else {
            continue;
        };
        for step in steps {
            if let Some(run) = step.get("run").and_then(Value::as_str) {
                f(run);
            }
        }
    }
}

/// Strip `${{ ... }}` GHA template expressions so a literal POSIX-style
/// regex pass over the `run:` block does not pick up names from inside
/// those expressions (e.g. `${{ env.NAME }}` should not count as a
/// shell read of `NAME`).
fn strip_gha_expressions(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 2 < bytes.len() && &bytes[i..i + 3] == b"${{" {
            // Skip until matching `}}`.
            let mut j = i + 3;
            while j + 1 < bytes.len() && &bytes[j..j + 2] != b"}}" {
                j += 1;
            }
            // Move past the closing `}}` if found; otherwise to end.
            i = if j + 1 < bytes.len() {
                j + 2
            } else {
                bytes.len()
            };
            // Replace skipped span with a single space to keep token boundaries.
            out.push(' ');
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parses_top_level_and_step_env() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".github/workflows");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ci.yml");
        fs::write(
            &path,
            "
name: CI
on: [push]
env:
  CI_LEVEL: production
jobs:
  build:
    env:
      JOB_TOKEN: '${{ secrets.JOB_TOKEN }}'
    steps:
      - name: build
        env:
          STEP_FLAG: '1'
",
        )
        .unwrap();
        let out = parse_workflow_file(&path, tmp.path(), 15, 20);
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"CI_LEVEL"));
        assert!(names.contains(&"JOB_TOKEN"));
        assert!(names.contains(&"STEP_FLAG"));
    }

    /// Regression for the 2026-05-18 Screenscribe hak: shell `$VAR` /
    /// `${VAR}` references in workflow `run:` blocks count as legitimate
    /// reads, while GHA template expressions `${{ env.VAR }}` are
    /// excluded (those are evaluated by Actions, not POSIX shell).
    #[test]
    fn shell_reads_include_dollar_and_brace_forms_skip_gha_expr() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".github/workflows");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ci.yml");
        fs::write(
            &path,
            "
name: CI
on: [push]
env:
  HEALTH_THRESHOLD: 50
jobs:
  gate:
    runs-on: ubuntu-latest
    steps:
      - run: |
          if [ \"$HEALTH\" -lt \"$HEALTH_THRESHOLD\" ]; then
            echo \"below ${THRESHOLD_LABEL}\"
            echo \"${{ env.UNRELATED }}\"
          fi
",
        )
        .unwrap();
        let reads = parse_workflow_shell_reads(&path, tmp.path());
        let names: HashSet<&str> = reads.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains("HEALTH"),
            "bare `$HEALTH` should be picked up: {names:?}"
        );
        assert!(
            names.contains("HEALTH_THRESHOLD"),
            "bare `$HEALTH_THRESHOLD` should be picked up: {names:?}"
        );
        assert!(
            names.contains("THRESHOLD_LABEL"),
            "bracketed `${{THRESHOLD_LABEL}}` should be picked up: {names:?}"
        );
        assert!(
            !names.contains("UNRELATED"),
            "GHA template expression `${{ env.UNRELATED }}` must NOT count as a shell read: {names:?}"
        );
        // Same-file dedupe: HEALTH_THRESHOLD referenced multiple times
        // collapses to one (name, file) entry per workflow.
        let count_threshold = reads
            .iter()
            .filter(|(n, _)| n == "HEALTH_THRESHOLD")
            .count();
        assert_eq!(
            count_threshold, 1,
            "duplicate references within one workflow should dedupe"
        );
    }

    /// W2-c regression (example-app CI-vars): `BASE_REF` assigned inside the same
    /// workflow's `run:` block must not count as an env read, and runner
    /// builtins (`GITHUB_ENV`, `GITHUB_OUTPUT`, `RUNNER_OS`, `CI`) are
    /// never reads regardless of declarations.
    #[test]
    fn shell_reads_skip_run_block_assignments_and_runner_builtins() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".github/workflows");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ci.yml");
        fs::write(
            &path,
            "
name: CI
on: [push]
jobs:
  vars:
    runs-on: ubuntu-latest
    steps:
      - run: |
          BASE_REF=\"main\"
          echo \"ref=$BASE_REF\" >> \"$GITHUB_ENV\"
          echo \"out=$BASE_REF\" >> \"$GITHUB_OUTPUT\"
          echo \"on $RUNNER_OS ci=$CI\"
          echo \"genuine read: $DEPLOY_TOKEN\"
",
        )
        .unwrap();
        let reads = parse_workflow_shell_reads(&path, tmp.path());
        let names: HashSet<&str> = reads.iter().map(|(n, _)| n.as_str()).collect();
        for skipped in ["BASE_REF", "GITHUB_ENV", "GITHUB_OUTPUT", "RUNNER_OS", "CI"] {
            assert!(
                !names.contains(skipped),
                "`{skipped}` must not be a shell read: {names:?}"
            );
        }
        assert!(
            names.contains("DEPLOY_TOKEN"),
            "unassigned `$DEPLOY_TOKEN` must remain a read: {names:?}"
        );
    }

    #[test]
    fn extracts_secrets_references() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".github/workflows");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("deploy.yml");
        fs::write(
            &path,
            "
name: Deploy
on: [push]
jobs:
  ship:
    runs-on: ubuntu-latest
    steps:
      - run: deploy
        env:
          AWS_ACCESS_KEY_ID: ${{ secrets.AWS_KEY_ID }}
          AWS_SECRET: ${{ secrets.AWS_SECRET }}
",
        )
        .unwrap();
        let out = parse_workflow_file(&path, tmp.path(), 15, 20);
        let secret_names: Vec<&str> = out
            .iter()
            .filter(|(_, s)| matches!(s.kind, EnvSourceKind::GitHubActionsSecret))
            .map(|(n, _)| n.as_str())
            .collect();
        assert!(secret_names.contains(&"AWS_KEY_ID"));
        assert!(secret_names.contains(&"AWS_SECRET"));
    }
}
