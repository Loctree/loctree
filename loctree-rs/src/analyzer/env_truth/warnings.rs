//! Warning detection for env-truth.
//!
//! Computes drift / divergence warnings against the merged declaration map
//! using simple heuristics:
//!
//! - **stale-overrides-fresh**: highest-precedence source is materially older
//!   than the next-highest by a configurable day threshold.
//! - **multi-source-mismatch**: two or more `Plain` sources disagree on
//!   `value_hash`. Encrypted/secret sources are ignored — we cannot compare
//!   what we never see.
//! - **orphan-code-reference**: a name appears in `semantic_facts.env_contracts`
//!   but no declaration is present.
//! - **orphan-declaration**: a name has at least one declaration but zero
//!   read sites *and* the snapshot has any env_contracts at all (so we
//!   know the read-side scanner ran).
//! - **sealed-secret-suspected-stale**: specialization of stale-overrides-fresh
//!   that triggers specifically when the stale source is a SealedSecret/SOPS
//!   and the fresh source is a plain dotenv/configmap.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use super::types::{EnvDeclaration, EnvSource, EnvSourceKind, EnvWarning, ValuePresence};

/// Minimum age delta (in days) before a stale-overrides-fresh warning fires.
///
/// Tunable via `.loctree/config.toml [env_truth] stale_threshold_days`,
/// but the constant here serves as a safe default.
pub const DEFAULT_STALE_THRESHOLD_DAYS: u32 = 7;

/// Compute warnings for one env declaration in place.
pub fn compute_warnings(
    decl: &mut EnvDeclaration,
    has_env_contracts: bool,
    stale_threshold_days: u32,
) {
    decl.precedence_warnings.clear();

    // 0. orphan-code-reference: read sites exist but no declarations.
    //    Mirrors the top-level `orphan_reads` collection. Runtime-provided
    //    names (OS user env, shell/CI builtins) are exempt — `$HOME` or
    //    `$GITHUB_ENV` exist without any declaration file, so flagging them
    //    is pure noise (W2-c).
    if decl.sources.is_empty()
        && !decl.reads.is_empty()
        && !is_runtime_provided_env(&decl.name)
        && !is_single_letter_shell_shorthand(&decl.name, &decl.reads)
    {
        decl.precedence_warnings
            .push(EnvWarning::OrphanCodeReference {
                read_sites: decl.reads.iter().map(|r| r.file.clone()).collect(),
            });
    }

    // 1. multi-source value mismatch.
    let plain_sources: Vec<&EnvSource> = decl
        .sources
        .iter()
        .filter(|s| matches!(s.value_present, ValuePresence::Plain { .. }))
        .collect();
    if plain_sources.len() >= 2 {
        let first_hash = plain_value_hash(plain_sources[0]);
        let mismatch = plain_sources
            .iter()
            .any(|s| plain_value_hash(s) != first_hash);
        if mismatch {
            decl.precedence_warnings
                .push(EnvWarning::MultiSourceValueMismatch {
                    sources: plain_sources.iter().map(|s| s.path.clone()).collect(),
                });
        }
    }

    // 2. orphan-declaration: declared but never read.
    if has_env_contracts && decl.reads.is_empty() && !is_synthetic(&decl.name) {
        decl.precedence_warnings
            .push(EnvWarning::OrphanDeclaration {
                sources: decl.sources.iter().map(|s| s.path.clone()).collect(),
            });
    }

    // 3. stale-overrides-fresh: highest-precedence is older than the
    //    runner-up by `stale_threshold_days`.
    //
    //    Sources are sorted descending by precedence_rank in the orchestrator,
    //    so `decl.sources[0]` wins at deploy time. The runner-up gives the
    //    canonical "obvious neighbor" comparison, but the SealedSecret/
    //    SOPS/ExternalSecret specialization scans **all** lower-rank
    //    plain-bearing sources — example-app's case had a stale SealedSecret
    //    overriding a fresh ConfigMap several layers down, not just the
    //    immediate runner-up.
    if decl.sources.len() >= 2 {
        let high = &decl.sources[0];
        let low = &decl.sources[1];
        if let (Some(h_age), Some(l_age)) = (high.mtime_age_days, low.mtime_age_days)
            && h_age > l_age
            && h_age.saturating_sub(l_age) >= stale_threshold_days
        {
            let delta = h_age.saturating_sub(l_age);
            decl.precedence_warnings
                .push(EnvWarning::StaleOverridesFresh {
                    stale_source: high.path.clone(),
                    fresh_source: low.path.clone(),
                    age_delta_days: delta,
                });
        }
        // example-app specialization: highest is sealed/sops/external AND any
        // lower-rank plain-bearing source is materially fresher.
        if matches!(
            high.kind,
            EnvSourceKind::SealedSecret | EnvSourceKind::SopsFile | EnvSourceKind::ExternalSecret
        ) && let Some(h_age) = high.mtime_age_days
        {
            let plain_fresh_neighbor = decl.sources[1..].iter().find(|s| {
                matches!(s.value_present, ValuePresence::Plain { .. })
                    && s.mtime_age_days
                        .is_some_and(|a| h_age > a && h_age - a >= stale_threshold_days)
            });
            if let Some(neighbor) = plain_fresh_neighbor {
                decl.precedence_warnings
                    .push(EnvWarning::SealedSecretSuspectedStale {
                        sealed_path: high.path.clone(),
                        sealed_age_days: h_age,
                        plain_age_days: neighbor.mtime_age_days.unwrap_or(0),
                    });
            }
        }
    }

    // 4. encrypted-decode-blocked is informational — emit one per encrypted source.
    for src in &decl.sources {
        if matches!(src.value_present, ValuePresence::Encrypted { .. }) {
            decl.precedence_warnings
                .push(EnvWarning::EncryptedDecodeBlocked {
                    source: src.path.clone(),
                });
        }
    }
}

/// OS user-environment names every POSIX process inherits — reading them
/// without a repo-side declaration is normal, not drift.
const OS_STANDARD_ENV: &[&str] = &[
    "HOME",
    "PATH",
    "LANG",
    "LANGUAGE",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "PWD",
    "OLDPWD",
    "TMPDIR",
    "TMP",
    "TEMP",
    "HOSTNAME",
    "EDITOR",
    "VISUAL",
    "PAGER",
    "DISPLAY",
    "COLORTERM",
];

/// H5: common shell terminal-color / ANSI escape constants
/// (`RED='\033[0;31m'`, `NC='\033[0m'`). These are self-contained shell
/// script presentation locals, declared and read in the SAME script for
/// output formatting - not environment variables any repo-wide declaration
/// sensor tracks. `source_reads` (generic `$VAR`/`${VAR}` scanning) can't
/// tell them apart from a real env var read, so without this exemption
/// every color-coded shell script prints a false `orphan-code-reference`
/// for its own palette.
///
/// This is a name-based exemption, not a true same-file declare-and-read
/// correlation (no sensor exists yet for plain shell script `VAR=value`
/// assignments as declarations - that would be a real new sensor in
/// `discovery.rs`/a new parser, out of scope here and flagged for the
/// integrator). It only ever suppresses noise for this specific, narrow,
/// well-known name set - it can't hide a real gap for any other name.
const SHELL_ANSI_COLOR_NAMES: &[&str] = &[
    "RED",
    "GREEN",
    "YELLOW",
    "BLUE",
    "MAGENTA",
    "CYAN",
    "WHITE",
    "BLACK",
    "NC",
    "RESET",
    "BOLD",
    "DIM",
    "UNDERLINE",
    "BLINK",
    "REVERSE",
];

/// Names provided by the runtime (OS, shell, CI runner) rather than by any
/// repo declaration. Exempt from orphan-code-reference warnings.
///
/// Shell/CI builtins (`BASH_*`, `COMP_*`, `GITHUB_*`, `RUNNER_*`, `CI`) are
/// already filtered out of shell *contracts* at the semantic layer; this
/// check additionally covers reads coming from other languages (e.g. Rust
/// `std::env::var("HOME")`) that legitimately stay contracts.
pub(super) fn is_runtime_provided_env(name: &str) -> bool {
    OS_STANDARD_ENV.contains(&name)
        || SHELL_ANSI_COLOR_NAMES.contains(&name)
        || name.starts_with("LC_")
        || name.starts_with("XDG_")
        || crate::semantic::shell::is_shell_runtime_var(name)
}

/// The one-character shell colour-shorthand idiom (G/N/R/Y in
/// vc-workspace's scripts): exempt from orphan evidence only when every
/// read site is a shell script. A single-letter env var read from Rust or
/// Python is a real contract — the old blanket `len() == 1` exemption
/// erased genuine evidence by name alone (audit N08).
pub(super) fn is_single_letter_shell_shorthand(
    name: &str,
    reads: &[crate::analyzer::env_truth::types::EnvReadSite],
) -> bool {
    name.chars().count() == 1
        && !reads.is_empty()
        && reads.iter().all(|read| {
            let file = read.file.to_lowercase();
            file.ends_with(".sh") || file.ends_with(".bash") || file.ends_with(".zsh")
        })
}

fn plain_value_hash(s: &EnvSource) -> Option<&str> {
    match &s.value_present {
        ValuePresence::Plain { value_hash } => Some(value_hash.as_str()),
        _ => None,
    }
}

fn is_synthetic(name: &str) -> bool {
    name.starts_with("__") && name.ends_with("__")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::AuthorityLabel;

    fn mk_source(
        kind: EnvSourceKind,
        path: &str,
        rank: u8,
        age_days: u32,
        value: ValuePresence,
    ) -> EnvSource {
        EnvSource {
            kind,
            path: path.into(),
            line: None,
            mtime: "2026-01-01T00:00:00Z".into(),
            mtime_age_days: Some(age_days),
            git_age_days: None,
            value_present: value,
            precedence_rank: rank,
        }
    }

    fn decl_from_sources(sources: Vec<EnvSource>) -> EnvDeclaration {
        EnvDeclaration {
            name: "TEST".into(),
            sources,
            reads: vec![],
            precedence_warnings: vec![],
            authority: AuthorityLabel::SemanticGuess,
        }
    }

    #[test]
    fn detects_multi_source_mismatch_via_hashes() {
        let mut decl = decl_from_sources(vec![
            mk_source(
                EnvSourceKind::DotEnv,
                ".env",
                30,
                1,
                ValuePresence::Plain {
                    value_hash: "aaaaaaaaaaaa".into(),
                },
            ),
            mk_source(
                EnvSourceKind::DockerCompose,
                "docker-compose.yml",
                50,
                1,
                ValuePresence::Plain {
                    value_hash: "bbbbbbbbbbbb".into(),
                },
            ),
        ]);
        compute_warnings(&mut decl, false, 7);
        assert!(
            decl.precedence_warnings
                .iter()
                .any(|w| matches!(w, EnvWarning::MultiSourceValueMismatch { .. }))
        );
    }

    #[test]
    fn detects_sealed_overrides_fresh_plain() {
        let mut decl = decl_from_sources(vec![
            // High-precedence stale SealedSecret (30 days old)
            mk_source(
                EnvSourceKind::SealedSecret,
                "k8s/sealed.yaml",
                100,
                30,
                ValuePresence::Encrypted {
                    marker: "SealedSecret".into(),
                },
            ),
            // Low-precedence fresh .env (2 days old)
            mk_source(
                EnvSourceKind::DotEnv,
                ".env",
                30,
                2,
                ValuePresence::Plain {
                    value_hash: "cafe".repeat(3),
                },
            ),
        ]);
        compute_warnings(&mut decl, false, 7);
        assert!(
            decl.precedence_warnings
                .iter()
                .any(|w| matches!(w, EnvWarning::SealedSecretSuspectedStale { .. }))
        );
        assert!(
            decl.precedence_warnings
                .iter()
                .any(|w| matches!(w, EnvWarning::StaleOverridesFresh { .. }))
        );
    }

    #[test]
    fn no_stale_warning_when_within_threshold() {
        let mut decl = decl_from_sources(vec![
            mk_source(
                EnvSourceKind::SealedSecret,
                "k8s/sealed.yaml",
                100,
                3,
                ValuePresence::Encrypted {
                    marker: "SealedSecret".into(),
                },
            ),
            mk_source(
                EnvSourceKind::DotEnv,
                ".env",
                30,
                1,
                ValuePresence::Plain {
                    value_hash: "1234".repeat(3),
                },
            ),
        ]);
        compute_warnings(&mut decl, false, 7);
        assert!(
            !decl
                .precedence_warnings
                .iter()
                .any(|w| matches!(w, EnvWarning::StaleOverridesFresh { .. }))
        );
    }

    #[test]
    fn runtime_provided_reads_are_not_orphan_code_references() {
        use crate::analyzer::env_truth::types::EnvReadSite;
        for name in ["HOME", "PATH", "LC_ALL", "XDG_CONFIG_HOME", "GITHUB_ENV"] {
            let mut decl = decl_from_sources(vec![]);
            decl.name = name.into();
            decl.reads = vec![EnvReadSite {
                file: "src/lib.rs".into(),
                line: None,
                symbol: None,
                required_for: vec![],
            }];
            compute_warnings(&mut decl, true, 7);
            assert!(
                !decl
                    .precedence_warnings
                    .iter()
                    .any(|w| matches!(w, EnvWarning::OrphanCodeReference { .. })),
                "runtime-provided `{name}` must not be an orphan code reference"
            );
        }
        // A project var with no declaration still fires.
        let mut decl = decl_from_sources(vec![]);
        decl.name = "MY_API_KEY".into();
        decl.reads = vec![EnvReadSite {
            file: "src/lib.rs".into(),
            line: None,
            symbol: None,
            required_for: vec![],
        }];
        compute_warnings(&mut decl, true, 7);
        assert!(
            decl.precedence_warnings
                .iter()
                .any(|w| matches!(w, EnvWarning::OrphanCodeReference { .. }))
        );
    }

    /// H5 RED: a same-file shell ANSI color read (`RED`, `NC`, `BLUE`, ...)
    /// must not register as an orphan-code-reference - it's a shell script's
    /// own presentation constant, not a repo environment variable.
    #[test]
    fn ansi_color_reads_are_not_orphan_code_references() {
        use crate::analyzer::env_truth::types::EnvReadSite;
        for name in ["RED", "NC", "BLUE", "GREEN", "RESET", "G", "Y"] {
            let mut decl = decl_from_sources(vec![]);
            decl.name = name.into();
            decl.reads = vec![EnvReadSite {
                file: "scripts/deploy.sh".into(),
                line: Some(3),
                symbol: None,
                required_for: vec![],
            }];
            compute_warnings(&mut decl, true, 7);
            assert!(
                !decl
                    .precedence_warnings
                    .iter()
                    .any(|w| matches!(w, EnvWarning::OrphanCodeReference { .. })),
                "shell ANSI color var `{name}` must not be an orphan code reference"
            );
        }
    }

    #[test]
    fn orphan_declaration_only_when_env_contracts_present() {
        let mut decl = decl_from_sources(vec![mk_source(
            EnvSourceKind::DotEnv,
            ".env",
            30,
            1,
            ValuePresence::Plain {
                value_hash: "abcd".repeat(3),
            },
        )]);
        compute_warnings(&mut decl, false, 7);
        assert!(
            !decl
                .precedence_warnings
                .iter()
                .any(|w| matches!(w, EnvWarning::OrphanDeclaration { .. }))
        );
        let mut decl2 = decl.clone();
        decl2.precedence_warnings.clear();
        compute_warnings(&mut decl2, true, 7);
        assert!(
            decl2
                .precedence_warnings
                .iter()
                .any(|w| matches!(w, EnvWarning::OrphanDeclaration { .. }))
        );
    }
    /// N08 (audit): the one-letter exemption needs shell-read facts. `R`
    /// read only from shell scripts is the colour shorthand; `R` read from
    /// Rust is a real contract and must stay orphan evidence.
    #[test]
    fn single_letter_exemption_requires_shell_reads() {
        let shell_reads = vec![crate::analyzer::env_truth::types::EnvReadSite {
            file: "scripts/colors.sh".to_string(),
            line: Some(3),
            symbol: None,
            required_for: vec![],
        }];
        assert!(is_single_letter_shell_shorthand("R", &shell_reads));

        let rust_reads = vec![crate::analyzer::env_truth::types::EnvReadSite {
            file: "src/main.rs".to_string(),
            line: Some(10),
            symbol: None,
            required_for: vec![],
        }];
        assert!(
            !is_single_letter_shell_shorthand("R", &rust_reads),
            "a single-letter env read from Rust is a real contract, not a colour"
        );
        assert!(!is_single_letter_shell_shorthand("RED_LONG", &shell_reads));
    }
}
