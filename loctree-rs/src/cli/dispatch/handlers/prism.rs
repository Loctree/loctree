//! Handler for `loct prism`.
//!
//! V0 gives `vc-polarize` a stable runtime contract: compare several
//! task-scoped ContextPacks, score conceptual smear on the canonical 0..15
//! rubric, and emit either markdown or JSON.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::super::{DispatchResult, GlobalOptions};
use crate::cli::command::PrismOptions;
use crate::pack::{
    AuthoritySlice, ContextOptions, ContextPack, RiskCacheScope,
    compose_context_pack as compose_dense_context_pack,
};

const PRISM_SCHEMA_VERSION: &str = "loctree.prism.v1.1";

#[derive(Debug, Serialize)]
pub struct PrismReport {
    pub schema_version: String,
    pub project_root: String,
    pub tasks: Vec<String>,
    pub total_score: u8,
    pub band: String,
    pub axes: Vec<PrismAxisScore>,
    pub task_summaries: Vec<PrismTaskSummary>,
    pub overlap: PrismOverlap,
    pub recommendation: String,
    pub band_action: String,
}

#[derive(Debug, Serialize)]
pub struct PrismAxisScore {
    pub axis: String,
    pub score: u8,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrismTaskSummary {
    pub task: String,
    pub file_count: usize,
    pub runtime_signal_count: usize,
    pub memory_entry_count: usize,
    pub low_lexical_memory_count: usize,
    pub surface_kinds: Vec<String>,
    pub authority_labels: Vec<String>,
    pub top_files: Vec<String>,
    pub verification_gates: Vec<String>,
    pub likely_tests: Vec<String>,
    pub cache_scope: String,
    pub stale_snapshot: bool,
    pub dirty_worktree: bool,
    /// Internal aggregation set used by `compute_overlap` to compute
    /// pairwise Jaccard distance. Marked `pub` so external callers (the
    /// MCP-facing `run_prism` exposure and the schema-regression test in
    /// `tests/prism_schema_golden.rs`) can construct a summary without
    /// going through `compose_dense_context_pack`. Skipped from JSON
    /// serialization — it is not part of the `loctree.prism.v1.1` contract.
    #[serde(skip_serializing)]
    pub file_set: BTreeSet<String>,
}

#[derive(Debug, Serialize)]
pub struct PrismOverlap {
    pub union_files: usize,
    pub shared_files_all_tasks: usize,
    pub average_pairwise_jaccard: f64,
}

pub fn handle_prism_command(opts: &PrismOptions, global: &GlobalOptions) -> DispatchResult {
    let report = match run_prism(opts) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("[loct][prism] {err}");
            return DispatchResult::Exit(1);
        }
    };

    if opts.json || global.json {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("[loct][prism] failed to serialize report: {err}");
                return DispatchResult::Exit(1);
            }
        }
    } else {
        println!("{}", format_markdown(&report));
    }

    DispatchResult::Exit(0)
}

/// Compose context packs for each task framing and assemble a `PrismReport`.
///
/// Pure-data entry point shared by the CLI handler (`handle_prism_command`)
/// and the `loctree-mcp` `prism` tool. Performs no printing; returns the
/// fully-scored report on success or a stable error string on failure.
///
/// Errors are reported as `String` (rather than `anyhow::Error`) so that
/// callers in either binary can surface them through their own error
/// channels without dragging extra dependencies. Schema is the canonical
/// `loctree.prism.v1.1` shape pinned by `prism_schema_golden`.
pub fn run_prism(opts: &PrismOptions) -> Result<PrismReport, String> {
    if opts.tasks.len() < 2 {
        return Err("prism requires at least two task framings to compare".to_string());
    }

    let project_root = opts.project.clone().unwrap_or_else(|| PathBuf::from("."));
    let project_root = project_root.canonicalize().unwrap_or(project_root);

    let mut packs = Vec::with_capacity(opts.tasks.len());
    for task in &opts.tasks {
        let context_opts = ContextOptions {
            task: Some(task.clone()),
            with_aicx: opts.with_aicx && !opts.no_aicx,
            no_aicx: opts.no_aicx,
            project: Some(project_root.clone()),
            aicx_project_override: opts.aicx_project_override.clone(),
            json: true,
            full: true,
            ..Default::default()
        };

        match compose_dense_context_pack(&context_opts, &project_root) {
            Ok(pack) => packs.push((task.clone(), pack)),
            Err(err) => {
                return Err(format!("failed to compose context for '{task}': {err}"));
            }
        }
    }

    Ok(build_report(
        &project_root,
        &opts.tasks,
        &packs,
        opts.limit.max(1),
    ))
}

fn build_report(
    project_root: &Path,
    tasks: &[String],
    packs: &[(String, ContextPack)],
    limit: usize,
) -> PrismReport {
    let task_summaries: Vec<PrismTaskSummary> = packs
        .iter()
        .map(|(task, pack)| summarize_task(task, pack, limit))
        .collect();
    let overlap = compute_overlap(&task_summaries);
    let axes = score_axes(&task_summaries, &overlap);
    let total_score = axes.iter().map(|axis| axis.score).sum();
    let band = prism_band(total_score).to_string();
    let recommendation = prism_recommendation(total_score).to_string();
    let band_action = prism_band_action(total_score).to_string();

    PrismReport {
        schema_version: PRISM_SCHEMA_VERSION.to_string(),
        project_root: project_root.display().to_string(),
        tasks: tasks.to_vec(),
        total_score,
        band,
        axes,
        task_summaries,
        overlap,
        recommendation,
        band_action,
    }
}

fn summarize_task(task: &str, pack: &ContextPack, limit: usize) -> PrismTaskSummary {
    let mut files: BTreeSet<String> = pack
        .structural
        .files
        .iter()
        .map(|f| f.path.clone())
        .collect();
    for edge in &pack.runtime.dispatch_edges {
        files.insert(edge.from_file.clone());
        if let Some(handler) = &edge.handler_file {
            files.insert(handler.clone());
        }
    }
    for contract in &pack.runtime.env_contracts {
        for file in &contract.used_in_files {
            files.insert(file.clone());
        }
    }
    for hint in &pack.runtime.framework_hints {
        files.insert(hint.file.clone());
    }

    let runtime_signal_count = pack.runtime.idiom_tags.len()
        + pack.runtime.dispatch_edges.len()
        + pack.runtime.reachability.len()
        + pack.runtime.env_contracts.len()
        + pack.runtime.tauri_commands.len()
        + pack.runtime.tauri_events.len()
        + pack.runtime.framework_hints.len();
    let memory_entry_count = pack.memory.entries.len();
    let low_lexical_memory_count = pack
        .memory
        .entries
        .iter()
        .filter(|entry| entry.low_lexical_match)
        .count();

    let surface_kinds = surface_kinds(
        &files,
        runtime_signal_count,
        memory_entry_count,
        pack.action.verification_gates.len() + pack.action.likely_tests.len(),
    );
    let authority_labels = authority_labels(&pack.authority);
    let top_files = files.iter().take(limit).cloned().collect();

    PrismTaskSummary {
        task: task.to_string(),
        file_count: files.len(),
        runtime_signal_count,
        memory_entry_count,
        low_lexical_memory_count,
        surface_kinds,
        authority_labels,
        top_files,
        verification_gates: pack
            .action
            .verification_gates
            .iter()
            .take(limit)
            .cloned()
            .collect(),
        likely_tests: pack
            .action
            .likely_tests
            .iter()
            .take(limit)
            .cloned()
            .collect(),
        cache_scope: risk_cache_scope_label(&pack.risk.cache_scope).to_string(),
        stale_snapshot: pack.risk.stale_snapshot,
        dirty_worktree: pack.risk.dirty_worktree,
        file_set: files,
    }
}

fn surface_kinds(
    files: &BTreeSet<String>,
    runtime_count: usize,
    memory_count: usize,
    closure_count: usize,
) -> Vec<String> {
    let mut kinds: BTreeSet<String> = BTreeSet::new();
    for file in files {
        let lower = file.to_lowercase();
        if lower.contains("/test")
            || lower.contains("tests/")
            || lower.ends_with("_test.rs")
            || lower.ends_with(".test.ts")
            || lower.ends_with(".spec.ts")
        {
            kinds.insert("tests".to_string());
        } else if lower.ends_with(".md")
            || lower.contains("/docs/")
            || lower.contains("readme")
            || lower.contains("changelog")
        {
            kinds.insert("docs".to_string());
        } else if lower.contains("public")
            || lower.contains("landing")
            || lower.contains("distribution")
            || lower.contains("release")
        {
            kinds.insert("product_surface".to_string());
        } else {
            kinds.insert("code".to_string());
        }
    }
    if runtime_count > 0 {
        kinds.insert("runtime".to_string());
    }
    if memory_count > 0 {
        kinds.insert("memory".to_string());
    }
    if closure_count > 0 {
        kinds.insert("closure_evidence".to_string());
    }
    kinds.into_iter().collect()
}

fn authority_labels(authority: &AuthoritySlice) -> Vec<String> {
    let mut labels = Vec::new();
    if !authority.repo_verified.is_empty() {
        labels.push("repo_verified".to_string());
    }
    if !authority.loctree_derived.is_empty() {
        labels.push("loctree_derived".to_string());
    }
    if !authority.aicx_operator.is_empty() {
        labels.push("aicx_operator".to_string());
    }
    if !authority.aicx_agent.is_empty() {
        labels.push("aicx_agent".to_string());
    }
    if !authority.aicx_failure.is_empty() {
        labels.push("aicx_failure".to_string());
    }
    if !authority.semantic_guess.is_empty() {
        labels.push("semantic_guess".to_string());
    }
    if !authority.stale_or_unknown.is_empty() {
        labels.push("stale_or_unknown".to_string());
    }
    labels
}

fn compute_overlap(summaries: &[PrismTaskSummary]) -> PrismOverlap {
    let mut union: BTreeSet<String> = BTreeSet::new();
    for summary in summaries {
        union.extend(summary.file_set.iter().cloned());
    }

    let shared_files_all_tasks = summaries
        .first()
        .map(|first| {
            first
                .file_set
                .iter()
                .filter(|file| summaries.iter().all(|s| s.file_set.contains(*file)))
                .count()
        })
        .unwrap_or(0);

    let mut pair_count = 0usize;
    let mut pair_total = 0.0f64;
    for i in 0..summaries.len() {
        for j in (i + 1)..summaries.len() {
            let a = &summaries[i].file_set;
            let b = &summaries[j].file_set;
            let intersection = a.intersection(b).count();
            let union = a.union(b).count();
            let jaccard = if union == 0 {
                1.0
            } else {
                intersection as f64 / union as f64
            };
            pair_total += jaccard;
            pair_count += 1;
        }
    }

    PrismOverlap {
        union_files: union.len(),
        shared_files_all_tasks,
        average_pairwise_jaccard: if pair_count == 0 {
            1.0
        } else {
            (pair_total / pair_count as f64 * 1000.0).round() / 1000.0
        },
    }
}

fn score_axes(summaries: &[PrismTaskSummary], overlap: &PrismOverlap) -> Vec<PrismAxisScore> {
    vec![
        score_spread(summaries),
        score_runtime_centrality(summaries),
        score_authority_diversity(summaries),
        score_drift_risk(summaries, overlap),
        score_closure_evidence(summaries),
    ]
}

fn score_spread(summaries: &[PrismTaskSummary]) -> PrismAxisScore {
    let surfaces = union_strings(summaries.iter().flat_map(|s| s.surface_kinds.iter()));
    let score = match surfaces.len() {
        0 => 0,
        1 | 2 => 1,
        3 | 4 => 2,
        _ => 3,
    };
    PrismAxisScore {
        axis: "spread".to_string(),
        score,
        evidence: vec![format!("surface kinds: {}", join_or_none(&surfaces))],
    }
}

fn score_runtime_centrality(summaries: &[PrismTaskSummary]) -> PrismAxisScore {
    let runtime_total: usize = summaries.iter().map(|s| s.runtime_signal_count).sum();
    let central_files = summaries
        .iter()
        .flat_map(|s| s.file_set.iter())
        .filter(|file| is_central_runtime_file(file))
        .count();
    let score = if runtime_total == 0 && central_files == 0 {
        0
    } else if central_files == 0 {
        1
    } else if central_files < 4 {
        2
    } else {
        3
    };
    PrismAxisScore {
        axis: "runtime_centrality".to_string(),
        score,
        evidence: vec![format!(
            "runtime signals: {runtime_total}; central files: {central_files}"
        )],
    }
}

fn score_authority_diversity(summaries: &[PrismTaskSummary]) -> PrismAxisScore {
    let labels = union_strings(summaries.iter().flat_map(|s| s.authority_labels.iter()));
    let score = match labels.len() {
        0 | 1 => 0,
        2 => 1,
        3 | 4 => 2,
        _ => 3,
    };
    PrismAxisScore {
        axis: "authority_diversity".to_string(),
        score,
        evidence: vec![format!("authority labels: {}", join_or_none(&labels))],
    }
}

fn score_drift_risk(summaries: &[PrismTaskSummary], overlap: &PrismOverlap) -> PrismAxisScore {
    let mut score = if overlap.union_files == 0 {
        0
    } else if overlap.average_pairwise_jaccard < 0.25 {
        3
    } else if overlap.average_pairwise_jaccard < 0.55 {
        2
    } else if overlap.average_pairwise_jaccard < 0.85 {
        1
    } else {
        0
    };
    let low_lexical: usize = summaries.iter().map(|s| s.low_lexical_memory_count).sum();
    let stale_or_dirty = summaries
        .iter()
        .any(|s| s.stale_snapshot || s.dirty_worktree || s.cache_scope != "clean");
    if low_lexical > 0 || stale_or_dirty {
        score = (score + 1).min(3);
    }

    PrismAxisScore {
        axis: "drift_risk".to_string(),
        score,
        evidence: vec![
            format!(
                "average pairwise file overlap: {:.3}",
                overlap.average_pairwise_jaccard
            ),
            format!("low lexical memory entries: {low_lexical}"),
            format!("stale or dirty cache signal: {stale_or_dirty}"),
        ],
    }
}

fn score_closure_evidence(summaries: &[PrismTaskSummary]) -> PrismAxisScore {
    let gates = union_strings(summaries.iter().flat_map(|s| s.verification_gates.iter()));
    let tests = union_strings(summaries.iter().flat_map(|s| s.likely_tests.iter()));
    let total = gates.len() + tests.len();
    let score = match total {
        0 => 0,
        1 | 2 => 1,
        3..=5 => 2,
        _ => 3,
    };
    PrismAxisScore {
        axis: "closure_evidence".to_string(),
        score,
        evidence: vec![
            format!("verification gates: {}", gates.len()),
            format!("likely tests: {}", tests.len()),
        ],
    }
}

fn union_strings<'a>(items: impl Iterator<Item = &'a String>) -> Vec<String> {
    items
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn is_central_runtime_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.contains("src/bin/")
        || lower.ends_with("main.rs")
        || lower.contains("/cli/")
        || lower.contains("/dispatch/")
        || lower.contains("install")
        || lower.contains("release")
        || lower.contains("workflow")
        || lower.contains("skills/")
        || lower.contains("landing")
}

fn risk_cache_scope_label(scope: &RiskCacheScope) -> &'static str {
    match scope {
        RiskCacheScope::Clean => "clean",
        RiskCacheScope::DirtyWorktree => "dirty_worktree",
        RiskCacheScope::StaleSnapshot => "stale_snapshot",
        RiskCacheScope::MissingSnapshot => "missing_snapshot",
        RiskCacheScope::Scoped(_) => "scoped",
        RiskCacheScope::Unknown => "unknown",
    }
}

fn prism_band(score: u8) -> &'static str {
    match score {
        0..=4 => "0..4: no corpus entry",
        5..=8 => "5..8: local note or Loctree tag",
        9..=12 => "9..12: context-corpus entry",
        _ => "13..15: canonical doctrine entry plus regression contract",
    }
}

fn prism_recommendation(score: u8) -> &'static str {
    match score {
        0..=4 => "Keep local. No polarization pass required from prism evidence alone.",
        5..=8 => "Capture a local note or tag before implementation continues.",
        9..=12 => "Create a context-corpus entry and run vc-polarize if product truth is split.",
        _ => {
            "Run vc-polarize now: choose one axis, reject competing truths, and emit DoU/release handoff."
        }
    }
}

fn prism_band_action(score: u8) -> &'static str {
    match score {
        0..=4 => "abort",
        5..=8 => "memo",
        9..=12 => "pass",
        _ => "doctrine",
    }
}

fn join_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

fn format_markdown(report: &PrismReport) -> String {
    let mut out = String::new();
    out.push_str("# Prism Score\n\n");
    out.push_str(&format!(
        "- Project: `{}`\n- Score: **{} / 15**\n- Band: {}\n- Action: `{}`\n- Recommendation: {}\n\n",
        report.project_root,
        report.total_score,
        report.band,
        report.band_action,
        report.recommendation
    ));

    out.push_str("## Axes\n\n");
    for axis in &report.axes {
        out.push_str(&format!("- `{}`: **{} / 3**", axis.axis, axis.score));
        if !axis.evidence.is_empty() {
            out.push_str(&format!(" - {}", axis.evidence.join("; ")));
        }
        out.push('\n');
    }

    out.push_str("\n## Overlap\n\n");
    out.push_str(&format!(
        "- Union files: {}\n- Shared by all tasks: {}\n- Average pairwise Jaccard: {:.3}\n\n",
        report.overlap.union_files,
        report.overlap.shared_files_all_tasks,
        report.overlap.average_pairwise_jaccard
    ));

    out.push_str("## Task Framings\n\n");
    for summary in &report.task_summaries {
        out.push_str(&format!(
            "### `{}`\n\n- Files: {}\n- Runtime signals: {}\n- Memory entries: {}",
            summary.task,
            summary.file_count,
            summary.runtime_signal_count,
            summary.memory_entry_count
        ));
        if summary.low_lexical_memory_count > 0 {
            out.push_str(&format!(
                " ({} low lexical fallback)",
                summary.low_lexical_memory_count
            ));
        }
        out.push_str(&format!(
            "\n- Surfaces: {}\n- Authority: {}\n- Cache: `{}`\n",
            join_or_none(&summary.surface_kinds),
            join_or_none(&summary.authority_labels),
            summary.cache_scope
        ));
        if !summary.top_files.is_empty() {
            out.push_str("- Top files:\n");
            for file in &summary.top_files {
                out.push_str(&format!("  - `{file}`\n"));
            }
        }
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(
        task: &str,
        files: &[&str],
        surfaces: &[&str],
        authorities: &[&str],
    ) -> PrismTaskSummary {
        PrismTaskSummary {
            task: task.to_string(),
            file_count: files.len(),
            runtime_signal_count: 0,
            memory_entry_count: 0,
            low_lexical_memory_count: 0,
            surface_kinds: surfaces.iter().map(|s| s.to_string()).collect(),
            authority_labels: authorities.iter().map(|s| s.to_string()).collect(),
            top_files: files.iter().map(|s| s.to_string()).collect(),
            verification_gates: Vec::new(),
            likely_tests: Vec::new(),
            cache_scope: "clean".to_string(),
            stale_snapshot: false,
            dirty_worktree: false,
            file_set: files.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn overlap_reports_pairwise_jaccard() {
        let summaries = vec![
            summary("auth", &["a.rs", "b.rs"], &["code"], &["repo_verified"]),
            summary(
                "auth core",
                &["b.rs", "c.rs"],
                &["code"],
                &["repo_verified"],
            ),
        ];
        let overlap = compute_overlap(&summaries);
        assert_eq!(overlap.union_files, 3);
        assert_eq!(overlap.shared_files_all_tasks, 1);
        assert_eq!(overlap.average_pairwise_jaccard, 0.333);
    }

    #[test]
    fn high_spread_scores_three() {
        let summaries = vec![summary(
            "release",
            &["src/bin/loct.rs"],
            &["code", "runtime", "docs", "memory", "closure_evidence"],
            &["repo_verified", "loctree_derived"],
        )];
        let axis = score_spread(&summaries);
        assert_eq!(axis.score, 3);
    }
}
