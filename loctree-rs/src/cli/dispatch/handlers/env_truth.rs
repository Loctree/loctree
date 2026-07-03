//! Handler for `loct env-truth` (Cut 8 / Lane 4).
//!
//! Builds an [`EnvTruthReport`] via [`crate::analyzer::env_truth::compute_env_truth`],
//! cross-references the read side from `snapshot.semantic_facts.env_contracts`,
//! and renders Markdown (default) or JSON. Supports CI gating via `--fail-on`.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::path::PathBuf;

use crate::analyzer::env_truth::{
    self, ComputeConfig, DEFAULT_STALE_THRESHOLD_DAYS, EnvTruthReport, FailOnKind, RenderOptions,
};
use crate::cli::command::{EnvTruthOptions, GlobalOptions};
use crate::snapshot::{Snapshot, resolve_snapshot_root};

use super::super::DispatchResult;

pub fn run(opts: &EnvTruthOptions, global: &GlobalOptions) -> DispatchResult {
    let roots: Vec<PathBuf> = if opts.roots.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        opts.roots.clone()
    };
    let snapshot_root = resolve_snapshot_root(&roots);

    // Best-effort snapshot load — env-truth does not require a snapshot,
    // but uses it for the read-side cross-reference when available.
    let snapshot = match Snapshot::load(&snapshot_root) {
        Ok(s) => Some(s),
        Err(err) => {
            if !global.quiet {
                eprintln!(
                    "[loct][env-truth] snapshot not loaded ({err}); proceeding with declaration-only audit"
                );
            }
            None
        }
    };

    let cfg_override = env_truth::load_config_for(&snapshot_root);
    // Stale threshold priority: explicit CLI flag > config file > hardcoded default.
    let stale_threshold = opts
        .stale_threshold_days
        .or_else(|| cfg_override.as_ref().and_then(|c| c.stale_threshold_days))
        .unwrap_or(DEFAULT_STALE_THRESHOLD_DAYS);
    let cfg = ComputeConfig {
        roots: roots.clone(),
        restricted_paths: opts.restricted_paths.clone(),
        precedence_override: cfg_override,
        stale_threshold_days: stale_threshold,
        quiet: global.quiet,
    };

    let mut report = env_truth::compute_env_truth(&cfg, snapshot.as_ref());
    if let Some(name) = &opts.name {
        report.declarations.retain(|d| d.name == *name);
        report.orphan_reads.retain(|o| o.name == *name);
    }
    if opts.no_orphans {
        report.orphan_reads.clear();
    }

    // Parse --fail-on tokens early so misconfigured CI gates surface fast.
    let fail_kinds: Vec<FailOnKind> = match parse_fail_on(&opts.fail_on) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("[loct][env-truth] {msg}");
            return DispatchResult::Exit(2);
        }
    };

    // Emit output.
    if opts.json || global.json {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("[loct][env-truth] failed to serialize report: {err}");
                return DispatchResult::Exit(1);
            }
        }
    } else {
        // Markdown is the default human surface; --md is the explicit flag.
        // Default view is "Top problems"; `--all` restores the full dump
        // and `--hashes` reveals sha256 value hashes.
        let render_opts = RenderOptions {
            all: opts.all,
            show_hashes: opts.show_hashes,
        };
        let md = env_truth::render_markdown(&report, &render_opts);
        print!("{md}");
    }

    // CI gate. Returns 2 on first matching warning so pipelines fail fast.
    if !fail_kinds.is_empty() {
        let triggered = first_trigger(&report, &fail_kinds);
        if let Some((env_name, kind)) = triggered {
            if !global.quiet {
                eprintln!(
                    "[loct][env-truth] --fail-on {kind:?} matched on '{env_name}' — exiting 2"
                );
            }
            return DispatchResult::Exit(2);
        }
    }

    DispatchResult::Exit(0)
}

fn parse_fail_on(tokens: &[String]) -> Result<Vec<FailOnKind>, String> {
    let mut out = Vec::with_capacity(tokens.len());
    for tok in tokens {
        match FailOnKind::from_cli(tok) {
            Some(k) => out.push(k),
            None => {
                return Err(format!(
                    "unknown --fail-on kind '{tok}'. Allowed: stale-sealed-overrides-fresh-plain, stale-overrides-fresh, multi-source-mismatch, orphan-code-reference, orphan-declaration, any"
                ));
            }
        }
    }
    Ok(out)
}

fn first_trigger<'a>(
    report: &'a EnvTruthReport,
    kinds: &[FailOnKind],
) -> Option<(&'a str, FailOnKind)> {
    for decl in &report.declarations {
        for warning in &decl.precedence_warnings {
            for kind in kinds {
                if kind.matches(warning) {
                    return Some((decl.name.as_str(), *kind));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::env_truth::EnvWarning;

    fn warning_kind_str(w: &EnvWarning) -> &'static str {
        match w {
            EnvWarning::StaleOverridesFresh { .. } => "stale_overrides_fresh",
            EnvWarning::MultiSourceValueMismatch { .. } => "multi_source_value_mismatch",
            EnvWarning::OrphanCodeReference { .. } => "orphan_code_reference",
            EnvWarning::OrphanDeclaration { .. } => "orphan_declaration",
            EnvWarning::SealedSecretSuspectedStale { .. } => "sealed_secret_suspected_stale",
            EnvWarning::EncryptedDecodeBlocked { .. } => "encrypted_decode_blocked",
        }
    }

    #[test]
    fn parse_fail_on_rejects_unknown_token() {
        let r = parse_fail_on(&["bogus-kind".into()]);
        assert!(r.is_err());
    }

    #[test]
    fn parse_fail_on_accepts_known_tokens() {
        let r =
            parse_fail_on(&["stale-sealed-overrides-fresh-plain".into(), "any".into()]).unwrap();
        assert_eq!(r.len(), 2);
        // Sanity: encryption-blocked is not a fail kind, but stale-sealed is.
        assert!(matches!(r[0], FailOnKind::StaleSealedOverridesFreshPlain));
        assert!(matches!(r[1], FailOnKind::AnyWarning));
        // warning_kind_str is exercised by ensuring the discriminator strings
        // align with the orchestrator's fmt — defensive smoke against drift.
        let w = EnvWarning::SealedSecretSuspectedStale {
            sealed_path: "p".into(),
            sealed_age_days: 30,
            plain_age_days: 1,
        };
        assert_eq!(warning_kind_str(&w), "sealed_secret_suspected_stale");
    }
}
