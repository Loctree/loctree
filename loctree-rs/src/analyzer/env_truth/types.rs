//! Schema types for `loct env-truth` — declaration-side env audit.
//!
//! Cut 8 (P0). See `docs/env-truth-precedence.md` for the precedence-rank
//! model and why we never decode sealed/SOPS payloads.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::pack::AuthorityLabel;

/// Schema version emitted in `EnvTruthReport.schema_version`.
///
/// Consumers (CI gates, agent context packs, external tooling) may rely on:
/// - top-level fields: `schema_version`, `generated_at`, `roots`,
///   `declarations`, `orphan_reads`, `summary`.
/// - per-declaration fields: `name`, `sources`, `reads`,
///   `precedence_warnings`, `authority`.
/// - `EnvSource.kind` discriminator (one of `EnvSourceKind`).
/// - `EnvWarning` discriminator (one of `EnvWarning::*`).
///
/// Schema bumps follow semver: bug fixes don't bump, additive fields are
/// minor (`"1.1"`), breaking changes are major (`"2.0"`).
///
/// `"1.1"` (W2-c): additive `template_drift` top-level field; template
/// files (`.env.example` & friends) no longer appear in `sources`.
pub const ENV_TRUTH_SCHEMA_VERSION: &str = "1.1";

/// Top-level `env-truth` report.
///
/// Stable JSON output schema: `"1.0"`. See [`ENV_TRUTH_SCHEMA_VERSION`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvTruthReport {
    pub schema_version: String,
    pub generated_at: String,
    /// Scan roots that produced this report (canonical, repo-relative).
    pub roots: Vec<String>,
    /// One entry per distinct env name discovered or referenced from code.
    pub declarations: Vec<EnvDeclaration>,
    /// Code references (from `semantic_facts.env_contracts`) that have zero
    /// matching declaration anywhere in scope.
    pub orphan_reads: Vec<OrphanRead>,
    /// Drift between template files (`.env.example`, `*.sample`,
    /// `*.template`) and the live sources. Templates are shapes, never
    /// live declarations — they are excluded from `sources` ranking and
    /// compared key-by-key instead. Additive in schema `"1.1"`.
    #[serde(default)]
    pub template_drift: Vec<TemplateDrift>,
    /// Roll-up counts and the precedence-rank table that was applied.
    pub summary: EnvTruthSummary,
}

/// Key-level drift between one template file and the live declaration set.
///
/// Emitted only when at least one side drifts; templates in perfect sync
/// produce no entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateDrift {
    /// Repo-relative path of the template file.
    pub template_path: String,
    /// Keys the template promises that no live source declares anywhere
    /// in scope — onboarding will ask for vars production never defined,
    /// or a required var is genuinely missing from the live env.
    pub missing_in_live: Vec<String>,
    /// Keys declared by live dotenv files in the template's directory that
    /// the template omits — the template is stale as documentation.
    pub extra_in_live: Vec<String>,
}

/// Manifest for one env variable: every declaration site, every read site,
/// and any precedence-resolution warnings derived by the orchestrator.
///
/// Sources are sorted by descending `precedence_rank` (highest-precedence
/// declaration first — i.e. the one that likely wins at deploy time).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvDeclaration {
    pub name: String,
    pub sources: Vec<EnvSource>,
    pub reads: Vec<EnvReadSite>,
    pub precedence_warnings: Vec<EnvWarning>,
    /// Authority of the *declaration set as a whole*. Per-source authority
    /// is implicit in the source kind (file-derived = `RepoVerified`).
    pub authority: AuthorityLabel,
}

/// One declaration site for an env variable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSource {
    pub kind: EnvSourceKind,
    /// Repo-relative path to the file containing the declaration.
    pub path: String,
    /// Optional line number (1-based). Some YAML / structured sources only
    /// give a coarse "in this file" signal — those omit `line`.
    pub line: Option<u32>,
    /// File mtime as RFC 3339 (UTC). Cheap proxy for "freshness" without
    /// touching git.
    pub mtime: String,
    /// Optional file age in days, derived from `mtime` at scan time.
    pub mtime_age_days: Option<u32>,
    /// Optional git blame age in days for the line carrying this declaration
    /// (best-effort, missing on shallow clones / new files).
    pub git_age_days: Option<u32>,
    /// Value presence summary. Plain values are hashed (first 12 hex chars
    /// of SHA-256) — never the literal value.
    pub value_present: ValuePresence,
    /// Heuristic precedence score — higher means "more likely to win at
    /// deploy time". See `precedence.rs` for the default table.
    pub precedence_rank: u8,
}

/// Source-format discriminator.
///
/// Stable in JSON via `serde(tag = "format")` on the parent enum. Variants
/// are kept stable across schema versions; new formats append.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvSourceKind {
    /// `.env`, `.env.local`, `.env.production`, `.env.example`, ...
    DotEnv,
    /// `.envrc` (direnv).
    EnvRc,
    /// `Dockerfile` `ENV` directive.
    Dockerfile,
    /// `docker-compose*.yml` `environment:` (literal map / list).
    DockerCompose,
    /// `docker-compose*.yml` `env_file:` reference (delegates to dotenv files).
    DockerComposeEnvFile,
    /// k8s `Deployment`/`StatefulSet`/`DaemonSet` container `env:` literal.
    K8sDeploymentEnv,
    /// k8s `Deployment` container `envFrom:` reference (configmap/secret).
    K8sDeploymentEnvFrom,
    /// k8s `ConfigMap` `data` entry.
    K8sConfigMap,
    /// k8s `Secret` `data` (base64) — never decoded.
    K8sSecret,
    /// k8s `Secret` `stringData` (plain) — value hashed.
    K8sSecretStringData,
    /// `bitnami.com/v1alpha1` `SealedSecret` — never decoded.
    SealedSecret,
    /// External Secrets Operator `ExternalSecret`.
    ExternalSecret,
    /// SOPS-encrypted file (presence + age only).
    SopsFile,
    /// Helm `values*.yaml` env-block entry.
    HelmValues,
    /// `.github/workflows/*.yml` workflow-level / job-level / step-level `env:`.
    GitHubActionsEnv,
    /// `.github/workflows/*.yml` `${{ secrets.X }}` reference.
    GitHubActionsSecret,
    /// `package.json` script with `KEY=value` prefix.
    NpmScript,
    /// `tauri.conf.json` env-related field (best-effort, presence only).
    TauriConf,
}

/// Whether the source carries a literal value, an opaque/encrypted blob, or a
/// reference to another container.
///
/// Decoded values **never** appear here. Plain values are hashed (first 12
/// hex chars of SHA-256) so multi-source-mismatch warnings can compare
/// without leaking secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValuePresence {
    /// Plain literal value. `value_hash` is `sha256(value)[..12]` hex.
    Plain { value_hash: String },
    /// Encrypted blob (SealedSecret / SOPS / `ENC[AES256_GCM,...]` markers).
    /// `marker` is the format hint, not the payload.
    Encrypted { marker: String },
    /// `envFrom:` / `valueFrom:` style reference. The literal value lives
    /// elsewhere (resolved to its own EnvSource if discovered).
    EnvFrom { reference: String },
    /// k8s `Secret.data` — base64-encoded, treated as opaque even though
    /// reversible. Never hashed, never decoded.
    Secret,
    /// Declaration is present but value is empty (`KEY=` or `KEY:`).
    Empty,
}

/// Code site that reads this env variable (from
/// `snapshot.semantic_facts.env_contracts`). Authoritative provenance is the
/// Cut 3B semantic analyzer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvReadSite {
    pub file: String,
    pub line: Option<u32>,
    pub symbol: Option<String>,
    pub required_for: Vec<String>,
}

/// A code reference whose env name matches no declaration in scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrphanRead {
    pub name: String,
    pub read_sites: Vec<EnvReadSite>,
}

/// Detected drift / divergence in the precedence chain for a single env var.
///
/// Variants are stable in JSON via `serde(tag = "kind")`. CI gates select
/// failure modes by `kind`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EnvWarning {
    /// A higher-precedence declaration is materially older than a
    /// lower-precedence one. The classic Vista incident: stale SealedSecret
    /// (highest precedence, slow update cadence) overrides a freshly-edited
    /// `.env` / ConfigMap.
    StaleOverridesFresh {
        stale_source: String,
        fresh_source: String,
        age_delta_days: u32,
    },
    /// Two or more sources declare the same name with **different** plain
    /// value hashes. Encrypted/secret sources are skipped (cannot compare).
    MultiSourceValueMismatch { sources: Vec<String> },
    /// Code reads this env var but no declaration source is present.
    /// Mirrored at the report level as `orphan_reads`; per-declaration
    /// instances appear here when the code reference matches a source by
    /// name but with zero declarations in the *active* scope.
    OrphanCodeReference { read_sites: Vec<String> },
    /// Declaration exists but no code reads it. Frequently false-positive
    /// for runtime-injected vars (PaaS) — emitted only when
    /// `semantic_facts.env_contracts` is non-empty (i.e. semantic layer
    /// could have detected the read).
    OrphanDeclaration { sources: Vec<String> },
    /// Specialization of `StaleOverridesFresh` for the SealedSecret/plain
    /// pair — emitted as a separate kind so CI gates can target the
    /// Vista-style failure surface specifically.
    SealedSecretSuspectedStale {
        sealed_path: String,
        sealed_age_days: u32,
        plain_age_days: u32,
    },
    /// Informational — operator may want to know that a SealedSecret /
    /// SOPS file was discovered and intentionally left undecoded.
    EncryptedDecodeBlocked { source: String },
}

/// Roll-up counts plus the precedence table that was applied during scan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnvTruthSummary {
    pub total_declarations: usize,
    pub total_sources: usize,
    pub orphan_reads: usize,
    pub warnings_by_kind: std::collections::BTreeMap<String, usize>,
    /// The active precedence table: `EnvSourceKind` (snake_case) -> rank.
    pub precedence_table: std::collections::BTreeMap<String, u8>,
}

/// CI-gate failure modes. `loct env-truth --fail-on <kind>` exits 2 on
/// first matching warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailOnKind {
    StaleSealedOverridesFreshPlain,
    StaleOverridesFresh,
    MultiSourceMismatch,
    OrphanCodeReference,
    OrphanDeclaration,
    AnyWarning,
}

impl FailOnKind {
    /// Parse a CLI `--fail-on <token>` value.
    pub fn from_cli(token: &str) -> Option<FailOnKind> {
        match token {
            "stale-sealed-overrides-fresh-plain" => {
                Some(FailOnKind::StaleSealedOverridesFreshPlain)
            }
            "stale-overrides-fresh" => Some(FailOnKind::StaleOverridesFresh),
            "multi-source-mismatch" => Some(FailOnKind::MultiSourceMismatch),
            "orphan-code-reference" => Some(FailOnKind::OrphanCodeReference),
            "orphan-declaration" => Some(FailOnKind::OrphanDeclaration),
            "any" | "any-warning" => Some(FailOnKind::AnyWarning),
            _ => None,
        }
    }

    /// Match a single warning against the gate's filter.
    pub fn matches(self, w: &EnvWarning) -> bool {
        matches!(
            (self, w),
            (FailOnKind::AnyWarning, _)
                | (
                    FailOnKind::StaleSealedOverridesFreshPlain,
                    EnvWarning::SealedSecretSuspectedStale { .. },
                )
                | (
                    FailOnKind::StaleOverridesFresh,
                    EnvWarning::StaleOverridesFresh { .. }
                        | EnvWarning::SealedSecretSuspectedStale { .. },
                )
                | (
                    FailOnKind::MultiSourceMismatch,
                    EnvWarning::MultiSourceValueMismatch { .. },
                )
                | (
                    FailOnKind::OrphanCodeReference,
                    EnvWarning::OrphanCodeReference { .. },
                )
                | (
                    FailOnKind::OrphanDeclaration,
                    EnvWarning::OrphanDeclaration { .. },
                )
        )
    }
}

/// Materialized scan target: an absolute path the orchestrator will walk.
#[derive(Debug, Clone)]
pub struct ScanRoot {
    pub absolute: PathBuf,
    pub relative: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fail_on_kind_parses_known_tokens() {
        assert_eq!(
            FailOnKind::from_cli("stale-sealed-overrides-fresh-plain"),
            Some(FailOnKind::StaleSealedOverridesFreshPlain)
        );
        assert_eq!(
            FailOnKind::from_cli("multi-source-mismatch"),
            Some(FailOnKind::MultiSourceMismatch)
        );
        assert_eq!(
            FailOnKind::from_cli("orphan-code-reference"),
            Some(FailOnKind::OrphanCodeReference)
        );
        assert_eq!(FailOnKind::from_cli("any"), Some(FailOnKind::AnyWarning));
        assert_eq!(FailOnKind::from_cli("garbage"), None);
    }

    #[test]
    fn fail_on_stale_includes_sealed_specialization() {
        let warning = EnvWarning::SealedSecretSuspectedStale {
            sealed_path: "k8s/sealed.yaml".into(),
            sealed_age_days: 30,
            plain_age_days: 2,
        };
        assert!(FailOnKind::StaleOverridesFresh.matches(&warning));
        assert!(FailOnKind::StaleSealedOverridesFreshPlain.matches(&warning));
        assert!(!FailOnKind::MultiSourceMismatch.matches(&warning));
    }

    #[test]
    fn report_roundtrips_through_serde() {
        let report = EnvTruthReport {
            schema_version: ENV_TRUTH_SCHEMA_VERSION.into(),
            generated_at: "2026-04-28T00:00:00Z".into(),
            roots: vec![".".into()],
            declarations: vec![EnvDeclaration {
                name: "DATABASE_URL".into(),
                sources: vec![EnvSource {
                    kind: EnvSourceKind::DotEnv,
                    path: ".env".into(),
                    line: Some(1),
                    mtime: "2026-04-26T00:00:00Z".into(),
                    mtime_age_days: Some(2),
                    git_age_days: None,
                    value_present: ValuePresence::Plain {
                        value_hash: "abc123def456".into(),
                    },
                    precedence_rank: 30,
                }],
                reads: vec![],
                precedence_warnings: vec![EnvWarning::SealedSecretSuspectedStale {
                    sealed_path: "k8s/sealed.yaml".into(),
                    sealed_age_days: 30,
                    plain_age_days: 2,
                }],
                authority: AuthorityLabel::SemanticGuess,
            }],
            orphan_reads: vec![],
            template_drift: vec![TemplateDrift {
                template_path: ".env.example".into(),
                missing_in_live: vec!["TEMPLATE_ONLY".into()],
                extra_in_live: vec![],
            }],
            summary: EnvTruthSummary::default(),
        };
        let json = serde_json::to_string(&report).expect("serialize");
        let _back: EnvTruthReport = serde_json::from_str(&json).expect("deserialize");
        // Schema 1.0 payloads (no template_drift key) still deserialize.
        let legacy = json.replace(
            "\"template_drift\":[{\"template_path\":\".env.example\",\"missing_in_live\":[\"TEMPLATE_ONLY\"],\"extra_in_live\":[]}],",
            "",
        );
        let back: EnvTruthReport = serde_json::from_str(&legacy).expect("legacy deserialize");
        assert!(back.template_drift.is_empty());
    }
}
