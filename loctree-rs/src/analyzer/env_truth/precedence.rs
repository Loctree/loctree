//! Precedence-rank model for env declarations.
//!
//! Higher rank = more likely to win at deploy time. Operator can override
//! the default table via `.loctree/config.toml [env_truth] precedence = ...`.
//! See `docs/env-truth-precedence.md` for the full doctrine.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use super::types::EnvSourceKind;

/// Default precedence table — derived from observed deploy-time order in
/// k8s + docker-compose + dotenv stacks. Operator MUST treat as a heuristic;
/// override per repo via `.loctree/config.toml`.
pub fn default_table() -> BTreeMap<EnvSourceKind, u8> {
    use EnvSourceKind::*;
    let mut t = BTreeMap::new();
    t.insert(SealedSecret, 100);
    t.insert(ExternalSecret, 95);
    t.insert(K8sSecretStringData, 92);
    t.insert(K8sSecret, 90);
    t.insert(K8sDeploymentEnv, 85);
    t.insert(K8sDeploymentEnvFrom, 82);
    t.insert(K8sConfigMap, 80);
    t.insert(SopsFile, 78);
    t.insert(HelmValues, 65);
    t.insert(DockerCompose, 50);
    t.insert(DockerComposeEnvFile, 45);
    t.insert(Dockerfile, 40);
    t.insert(NpmScript, 35);
    t.insert(EnvSourceKind::DotEnv, 30);
    t.insert(GitHubActionsSecret, 20);
    t.insert(GitHubActionsEnv, 15);
    t.insert(TauriConf, 12);
    t.insert(EnvRc, 8);
    t
}

/// Is this dotenv-family path a TEMPLATE (shape, never a live source)?
///
/// W2-c: templates (`.env.example`, `*.sample`, `*.template`) are excluded
/// from the precedence ranking entirely and compared in template-drift mode
/// instead. Delegates to the shared W1-b artifact fence, widened with the
/// bare `example`/`sample`/`template` filename substrings that dotenv names
/// use without a dot separator (`.env.example` has one, `env.example.local`
/// might not).
pub fn is_template_path(path: &str) -> bool {
    if crate::analyzer::classify::artifact_class(path, None)
        == crate::analyzer::classify::ArtifactClass::Template
    {
        return true;
    }
    let name = Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_ascii_lowercase();
    name.contains("example") || name.contains("template") || name.contains("sample")
}

/// Refine the rank for a specific declaration based on file name. The base
/// table is by `EnvSourceKind`; for dotenv we further differentiate
/// `.env.production` (higher) from `.env.example` (lowest, intent-only).
pub fn refine_dotenv_rank(base: u8, path: &str) -> u8 {
    let name = Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path);
    if name.contains("example") || name.contains("template") || name.contains("sample") {
        // Intent only — lowest priority. Defense-in-depth: the orchestrator
        // routes template files away from ranking before this runs
        // (`is_template_path`), but a template that slips through any other
        // sensor path still lands at the bottom.
        return 5;
    }
    if name.contains("production") || name.contains(".prod") {
        return base.saturating_add(8);
    }
    if name.contains("staging") || name.contains(".stage") {
        return base.saturating_add(4);
    }
    if name.contains(".local") {
        return base.saturating_sub(5);
    }
    if name.contains(".test") || name.contains(".dev") {
        return base.saturating_sub(8);
    }
    base
}

/// On-disk override loaded from `.loctree/config.toml [env_truth] precedence`.
///
/// Keys are stringified `EnvSourceKind` variants in `snake_case` (matching
/// the JSON tag). Unknown keys are ignored (loose forward-compat).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EnvTruthConfig {
    #[serde(default)]
    pub precedence: BTreeMap<String, u8>,
    /// Optional override for the stale-overrides-fresh threshold in days.
    /// When no CLI `--stale-threshold-days` flag is given, this config value
    /// is used. If neither is set, the hardcoded default applies.
    #[serde(default)]
    pub stale_threshold_days: Option<u32>,
}

/// Load `.loctree/config.toml` and pull `[env_truth] precedence`.
/// Returns `None` if config is absent or unparseable. Errors are silent —
/// loctree never blows up the scan because of a bad config snippet.
pub fn load_config_override(snapshot_root: &Path) -> Option<EnvTruthConfig> {
    let config_path = snapshot_root.join(".loctree").join("config.toml");
    let raw = std::fs::read_to_string(&config_path).ok()?;
    let value: toml::Value = toml::from_str(&raw).ok()?;
    let env_truth = value.get("env_truth")?;
    let cfg: EnvTruthConfig = env_truth.clone().try_into().ok()?;
    Some(cfg)
}

/// Apply a config override on top of the default table. Unknown keys log a
/// warning to stderr but do not abort scan.
pub fn apply_override(table: &mut BTreeMap<EnvSourceKind, u8>, cfg: &EnvTruthConfig, quiet: bool) {
    for (raw_key, rank) in &cfg.precedence {
        match key_to_kind(raw_key) {
            Some(kind) => {
                table.insert(kind, *rank);
            }
            None if !quiet => {
                eprintln!("[loct][env-truth] config: unknown precedence key '{raw_key}' (ignored)");
            }
            None => {}
        }
    }
}

fn key_to_kind(raw: &str) -> Option<EnvSourceKind> {
    use EnvSourceKind::*;
    Some(match raw {
        "dot_env" | "dotenv" => DotEnv,
        "env_rc" | "envrc" => EnvRc,
        "dockerfile" => Dockerfile,
        "docker_compose" => DockerCompose,
        "docker_compose_env_file" => DockerComposeEnvFile,
        "k8s_deployment_env" => K8sDeploymentEnv,
        "k8s_deployment_env_from" => K8sDeploymentEnvFrom,
        "k8s_config_map" | "configmap" => K8sConfigMap,
        "k8s_secret" => K8sSecret,
        "k8s_secret_string_data" => K8sSecretStringData,
        "sealed_secret" => SealedSecret,
        "external_secret" => ExternalSecret,
        "sops_file" | "sops" => SopsFile,
        "helm_values" => HelmValues,
        "github_actions_env" | "gha_env" => GitHubActionsEnv,
        "github_actions_secret" | "gha_secret" => GitHubActionsSecret,
        "npm_script" => NpmScript,
        "tauri_conf" => TauriConf,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_table_populates_every_kind() {
        let table = default_table();
        assert!(table.contains_key(&EnvSourceKind::SealedSecret));
        assert!(table.contains_key(&EnvSourceKind::DotEnv));
        assert!(table.contains_key(&EnvSourceKind::GitHubActionsEnv));
        // SealedSecret is the highest in the default policy.
        let max = table.values().max().copied().unwrap();
        assert_eq!(table[&EnvSourceKind::SealedSecret], max);
    }

    #[test]
    fn template_paths_detected() {
        assert!(is_template_path(".env.example"));
        assert!(is_template_path("config/.env.sample"));
        assert!(is_template_path("deploy/.env.template"));
        assert!(is_template_path("env.example.local"));
        assert!(!is_template_path(".env"));
        assert!(!is_template_path(".env.production"));
        assert!(!is_template_path(".env.local"));
    }

    #[test]
    fn dotenv_refinement_demotes_example() {
        let base = 30;
        assert!(refine_dotenv_rank(base, ".env.example") < base);
        assert!(refine_dotenv_rank(base, ".env.production") > base);
        assert!(refine_dotenv_rank(base, ".env.local") < base);
    }

    #[test]
    fn config_override_replaces_known_keys() {
        let cfg = EnvTruthConfig {
            precedence: BTreeMap::from([("dot_env".into(), 99), ("unknown_key_xyz".into(), 50)]),
            stale_threshold_days: None,
        };
        let mut table = default_table();
        apply_override(&mut table, &cfg, true);
        assert_eq!(table[&EnvSourceKind::DotEnv], 99);
    }
}
