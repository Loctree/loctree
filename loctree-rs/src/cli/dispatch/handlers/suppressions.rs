//! Handler for `loct suppressions` — source-side silencer inventory surface.
//!
//! Wraps [`crate::analyzer::suppression_inventory`] (literal-only detection)
//! and renders one of three output shapes:
//!
//! 1. Default / `--summary` — count-per-kind table mirroring the visual
//!    style of `loct findings --summary` so operators get visual parity.
//! 2. `--json` — full structured array (one record per occurrence).
//! 3. Filtered human listing when `--type` is provided without `--summary`
//!    or `--json` (each match printed `file:line  kind  snippet`).
//!
//! # Tier boundary (read once, internalize forever)
//!
//! This handler is **free-tier scope**. It does NOT call semantic similarity
//! search, embedding-based suggestion, or LLM classification — and it MUST
//! NOT in any future revision. Adding "this suppression looks suspicious"
//! enrichment crosses into paid-tier Wave 7+ territory (see
//! `analyzer::suppression_inventory` module docs). Future agents touching
//! this file: keep the path literal-only or add an explicit
//! `feature = "semantic"` flag boundary.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::collections::HashSet;
use std::path::PathBuf;

use super::super::super::command::SuppressionsOptions;
use super::super::{DispatchResult, GlobalOptions};
use crate::analyzer::suppression_inventory::{
    SilencerKind, SilencerMatch, inventory, resolve_ignore_globs,
};

/// Entry point dispatched from `Command::Suppressions`.
pub fn handle_suppressions_command(
    opts: &SuppressionsOptions,
    global: &GlobalOptions,
) -> DispatchResult {
    let root = opts.root.clone().unwrap_or_else(|| PathBuf::from("."));

    if !root.exists() {
        eprintln!(
            "[loct][suppressions] root '{}' does not exist",
            root.display()
        );
        return DispatchResult::Exit(2);
    }

    // Parse --type filter tokens. Unknown tokens fail fast so the operator
    // doesn't get a silent empty report from a typo.
    let mut filter: HashSet<SilencerKind> = HashSet::new();
    for raw in &opts.kinds {
        for token in raw.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            match SilencerKind::from_filter(token) {
                Some(k) => {
                    filter.insert(k);
                }
                None => {
                    eprintln!(
                        "[loct][suppressions] unknown --type token '{}'. Valid: {}",
                        token,
                        valid_token_list()
                    );
                    return DispatchResult::Exit(2);
                }
            }
        }
    }

    // .semgrepignore filtering is ON by default (drop fixtures, vendored test
    // material, CLI-entry-points that engineers exclude from semgrep audits).
    // Operators opt back in with --include-fixtures.
    let extra_globs = resolve_ignore_globs(&root, !opts.include_fixtures);

    let inv = inventory(&root, &filter, &extra_globs);

    // Output mode resolution: JSON wins over summary, summary is default.
    let want_json = opts.json || global.json;

    if want_json {
        match serde_json::to_string_pretty(&inv.matches) {
            Ok(json) => {
                println!("{}", json);
                DispatchResult::Exit(0)
            }
            Err(e) => {
                eprintln!(
                    "[loct][suppressions][error] JSON serialization failed: {}",
                    e
                );
                DispatchResult::Exit(1)
            }
        }
    } else if opts.summary || filter.is_empty() {
        // Default to --summary when no other output mode is selected.
        print_summary(&inv, &root, !opts.include_fixtures);
        DispatchResult::Exit(0)
    } else {
        print_list(&inv.matches);
        DispatchResult::Exit(0)
    }
}

fn valid_token_list() -> String {
    SilencerKind::all()
        .iter()
        .map(|k| k.label())
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_summary(
    inv: &crate::analyzer::suppression_inventory::SilencerInventory,
    root: &std::path::Path,
    semgrepignore_applied: bool,
) {
    let suffix = if semgrepignore_applied {
        " (after .semgrepignore)"
    } else {
        " (including fixtures)"
    };
    println!("Suppression inventory — {}{}", root.display(), suffix);
    println!();

    if inv.matches.is_empty() {
        println!("  (no silencers detected)");
        return;
    }

    // Render in canonical order (matches the order in SilencerKind::all()).
    for kind in SilencerKind::all() {
        let label = kind.label();
        let Some(count) = inv.counts.get(label) else {
            continue;
        };
        let files = inv.files_per_kind.get(label).copied().unwrap_or(0);
        let note = annotation_for(*kind);
        let file_word = if files == 1 { "file" } else { "files" };
        if let Some(n) = note {
            println!(
                "  {:<22} : {:>4} ({} {}){}",
                label, count, files, file_word, n
            );
        } else {
            println!("  {:<22} : {:>4} ({} {})", label, count, files, file_word);
        }
    }

    println!();
    println!(
        "Total: {} silencers across {} files.",
        inv.total, inv.total_files
    );
    println!(
        "Tip: `loct suppressions --type <kind>` to list one bucket; \
         `--json` for full machine output. Literal detection only — \
         semantic enrichment is paid-tier (Wave 7+)."
    );
}

fn annotation_for(kind: SilencerKind) -> Option<&'static str> {
    match kind {
        SilencerKind::DeadCode => Some("  <- forgotten gems"),
        SilencerKind::UnsafeEnvVar => Some("  (Rust 2024 boilerplate)"),
        _ => None,
    }
}

fn print_list(matches: &[SilencerMatch]) {
    if matches.is_empty() {
        println!("(no matches)");
        return;
    }
    for m in matches {
        let rule = m
            .rule_id
            .as_ref()
            .map(|r| format!("  [{}]", r))
            .unwrap_or_default();
        println!(
            "  {}:{}  {}{}  {}",
            m.file,
            m.line,
            m.kind.label(),
            rule,
            m.snippet
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &std::path::Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn handler_returns_exit_0_on_clean_repo() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        write(tmp.path(), "src/lib.rs", "fn x() {}\n");
        let opts = SuppressionsOptions {
            root: Some(tmp.path().to_path_buf()),
            summary: true,
            ..Default::default()
        };
        let global = GlobalOptions::default();
        match handle_suppressions_command(&opts, &global) {
            DispatchResult::Exit(0) => {}
            other => panic!("expected Exit(0), got {:?}", debug_dispatch(&other)),
        }
    }

    #[test]
    fn handler_rejects_unknown_type_filter() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        let opts = SuppressionsOptions {
            root: Some(tmp.path().to_path_buf()),
            kinds: vec!["bogus-kind".to_string()],
            ..Default::default()
        };
        let global = GlobalOptions::default();
        match handle_suppressions_command(&opts, &global) {
            DispatchResult::Exit(2) => {}
            other => panic!("expected Exit(2), got {:?}", debug_dispatch(&other)),
        }
    }

    fn debug_dispatch(r: &DispatchResult) -> String {
        match r {
            DispatchResult::Exit(c) => format!("Exit({})", c),
            DispatchResult::ShowHelp => "ShowHelp".to_string(),
            DispatchResult::ShowLegacyHelp => "ShowLegacyHelp".to_string(),
            DispatchResult::ShowVersion => "ShowVersion".to_string(),
            DispatchResult::Continue(_) => "Continue".to_string(),
        }
    }
}
