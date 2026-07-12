//! Cut 11 — pill renderer (`loct context` brand surface).
//!
//! Produces a curated markdown briefing that adheres to the schema laid
//! out in the Cut 11 plan: six sections in fixed order with hard per-section
//! line caps, ranked content within each section, explicit truncation tail,
//! TL;DR generated last but rendered first, and authority labels on every
//! claim.
//!
//! The renderer never touches the wire — it returns a fully-formed string
//! that the caller writes to stdout in one shot. Streaming-order semantics
//! are preserved by ordering the string itself: TL;DR is the first
//! `## ...` section after the header.

use std::time::Instant;

use crate::aicx::summarize_entry;
use crate::context_render::chunk_ref;
use crate::pack::{
    AuthorityLabel, AuthoritySlice, ContextPack, MemoryEntry, MemorySlice, RiskCacheScope,
    RiskSlice, RuntimeIdiomTag, RuntimeSlice,
};

use super::budget::{Budget, count_lines, rank_and_render, truncate_with_tail};
use super::honesty::{DeadExportsStatus, MeasurementStatus};
use super::scope::{AutoScope, HubEntry};

/// Render the pill markdown. Output is bounded by [`Budget::TOTAL_CEILING`].
pub fn render_pill(input: PillInput<'_>) -> String {
    let mut out = String::with_capacity(8 * 1024);

    // ----- Sections (build bodies first; TL;DR generated after the body
    //       so it can synthesize from the collected metrics) -------------
    let where_section = render_where_you_are(&input);
    let live_section = render_whats_live(&input);
    let memory_section = render_memory(&input);
    let action_section = render_action(&input);
    let authority_section = render_authority_index(&input);

    let metrics = collect_metrics(&input, &where_section, &live_section, &memory_section);
    let tldr_section = render_tldr(&input, &metrics);

    // ----- Header (1-line repo identity + render timestamp) -------------
    out.push_str(&render_header(&input));

    // ----- TL;DR FIRST (Cut 11 streaming-order rule) --------------------
    out.push_str(&tldr_section.body);

    // ----- Body sections in fixed order ---------------------------------
    out.push_str(&where_section.body);
    out.push_str(&live_section.body);
    out.push_str(&memory_section.body);
    out.push_str(&action_section.body);
    out.push_str(&authority_section.body);

    // ----- Footer ------------------------------------------------------
    let footer = format!(
        "\n---\n\n*`loct context` rendered in {ms}ms · sections: where={w}L · live={l}L · memory={mem}L · `--full` for full ContextPack data · `--file <X>` to narrow scope*\n",
        ms = input.elapsed.as_millis(),
        w = metrics.where_lines,
        l = metrics.live_lines,
        mem = metrics.memory_lines,
    );
    out.push_str(&footer);

    enforce_total_ceiling(out)
}

/// Everything the renderer needs in one struct (cheaper than threading
/// individual references through every helper).
pub struct PillInput<'a> {
    pub pack: &'a ContextPack,
    pub scope: &'a AutoScope,
    pub project_name: String,
    pub timestamp_iso: String,
    pub snapshot_files: usize,
    pub snapshot_edges: usize,
    pub elapsed: std::time::Duration,
    pub aicx_status: AicxRenderStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AicxRenderStatus {
    /// AICX was engaged and produced rows.
    EnabledWithRows,
    /// AICX was engaged but the binary was unavailable.
    Unavailable,
    /// AICX was engaged but produced no rows.
    EnabledNoRows,
    /// AICX was engaged but the transport timed out / the auto-overlay
    /// wall-clock budget ran dry before the store answered.
    SkippedTimeout,
    /// AICX was explicitly disabled with `--no-aicx`.
    Disabled,
}

#[derive(Debug, Clone)]
struct Section {
    body: String,
    line_count: usize,
}

// All fields are consumed by the live render path: `top_three_warnings`
// reads cycles_status / dead_exports / twins_status / hub_count; the TL;DR
// "Snapshot facts" line reads the count fields; the pill footer reads the
// *_lines fields for per-section accounting.
#[derive(Debug, Default)]
struct Metrics {
    hub_count: usize,
    cycles_status: MeasurementStatus,
    dead_exports: DeadExportsStatus,
    twins_status: MeasurementStatus,
    idiom_tag_count: usize,
    dispatch_edge_count: usize,
    env_contract_count: usize,
    memory_entry_count: usize,
    authority_total: usize,
    where_lines: usize,
    live_lines: usize,
    memory_lines: usize,
}

impl Default for MeasurementStatus {
    fn default() -> Self {
        Self::NotMeasured("snapshot does not pre-compute this metric".to_string())
    }
}

// Custom default — `derive(Default)` would pick `Measured(0)` (the first
// variant), which is exactly the misleading-green metric Cut 11 forbids.
#[allow(clippy::derivable_impls)]
impl Default for DeadExportsStatus {
    fn default() -> Self {
        // Rust workspaces (the dominant case for loctree itself) hit the
        // `pub use` re-export hole. Default to honest skip.
        Self::SkippedDueToReExports
    }
}

// ----------------------------------------------------------------------------
// Header & repo-identity line
// ----------------------------------------------------------------------------

fn render_header(input: &PillInput<'_>) -> String {
    let branch = input
        .scope
        .branch
        .clone()
        .unwrap_or_else(|| "<no-branch>".to_string());
    let mut header = String::new();
    header.push_str(&format!(
        "# Loctree Context · {project} @ {branch} · {ts}\n\n",
        project = input.project_name,
        branch = branch,
        ts = input.timestamp_iso,
    ));
    // `AutoScope::dirty` is the canonical worktree-dirty signal captured at
    // scope discovery. Fall back to the risk slice if discovery never ran.
    let dirty = input.scope.dirty || input.pack.risk.dirty_worktree;
    let worktree = if dirty {
        "dirty worktree"
    } else {
        "clean worktree"
    };
    if snapshot_is_missing(input) {
        header.push_str(&format!("_no snapshot - run `loct scan` · {worktree}_\n\n"));
    } else {
        header.push_str(&format!(
            "_{files} files · {edges} import edges · snapshot {state} · {worktree}_\n\n",
            files = input.snapshot_files,
            edges = input.snapshot_edges,
            state = snapshot_state_label(input),
        ));
    }
    header
}

// ----------------------------------------------------------------------------
// TL;DR
// ----------------------------------------------------------------------------

fn render_tldr(input: &PillInput<'_>, m: &Metrics) -> Section {
    let mut lines: Vec<String> = Vec::new();
    lines.push("## TL;DR — read this first".to_string());
    lines.push(String::new());

    // What this is (auto-derived from project name + scope branch hint)
    let what = if snapshot_is_missing(input) {
        format!(
            "**What this is.** `{}` repository — no snapshot. Run `loct scan` before trusting file/import metrics.",
            input.project_name
        )
    } else {
        match input.scope.branch_hint.as_deref() {
            Some(hint) if !hint.is_empty() => format!(
                "**What this is.** `{}` repository — current focus: _{}_.",
                input.project_name, hint
            ),
            _ => format!(
                "**What this is.** `{}` repository ({} files, {} import edges).",
                input.project_name, input.snapshot_files, input.snapshot_edges
            ),
        }
    };
    lines.push(what);
    lines.push(String::new());

    // Where you stand
    let worktree = if input.pack.risk.dirty_worktree {
        "dirty"
    } else {
        "clean"
    };
    let branch = input.scope.branch.as_deref().unwrap_or("<detached>");
    lines.push(format!(
        "**Where you stand.** Branch `{branch}`, {worktree} worktree, snapshot {}.",
        snapshot_state_label(input)
    ));
    if !input.scope.commit_hints.is_empty() {
        lines.push(format!(
            "Last {n} commits: {commits}",
            n = input.scope.commit_hints.len(),
            commits = input
                .scope
                .commit_hints
                .iter()
                .take(3)
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(" · "),
        ));
    }
    let active_intent = top_aicx_intent(&input.pack.memory);
    if let Some(intent) = active_intent {
        let summary = summarize_entry(&intent.text);
        lines.push(format!(
            "Active intent (AICX): \"{}\" — {} on {}.",
            summary.text, intent.agent, intent.date
        ));
    }
    if let Some(scope) = &input.pack.scope {
        let scope_name = scope.named_resolved_from.as_deref().unwrap_or_else(|| {
            scope
                .selectors
                .first()
                .map(String::as_str)
                .unwrap_or("<scope>")
        });
        let resolved = if scope.resolved_selectors.is_empty() {
            scope.selectors.join(", ")
        } else {
            scope.resolved_selectors.join(", ")
        };
        lines.push(format!(
            "**Scope.** `{scope_name}` -> {resolved} ({} files, fingerprint: {}).",
            scope.matched_files, scope.fingerprint
        ));
    }
    if let Some(task) = &input.pack.task {
        lines.push(format!(
            "**Task.** *{}* ({}, SemanticGuess).",
            task.text, task.mode
        ));
    }
    lines.push(String::new());

    // Top-3 things to know
    lines.push("**Top 3 things to know before editing.**".to_string());
    let top_three = top_three_warnings(input, m);
    for (i, item) in top_three.iter().enumerate() {
        lines.push(format!("{}. {}", i + 1, item));
    }
    lines.push(String::new());

    // Snapshot facts (compact one-liner sourced from collected metrics)
    if snapshot_is_missing(input) {
        lines.push(
            "**Snapshot facts.** no snapshot - run `loct scan`; derived file/import metrics are unavailable. (RepoVerified)"
                .to_string(),
        );
    } else {
        lines.push(format!(
            "**Snapshot facts.** {hubs} hubs · {idiom} idiom tag(s) · {dispatch} dispatch edge(s) · {env} env contract(s) · {mem} memory entr{plural} · {auth} authority claim(s).",
            hubs = m.hub_count,
            idiom = m.idiom_tag_count,
            dispatch = m.dispatch_edge_count,
            env = m.env_contract_count,
            mem = m.memory_entry_count,
            plural = if m.memory_entry_count == 1 { "y" } else { "ies" },
            auth = m.authority_total,
        ));
    }
    lines.push(String::new());

    // What's stale
    let stale = collect_stale(input);
    lines.push(format!(
        "**What's stale.** {}",
        if stale.is_empty() {
            "None.".to_string()
        } else {
            stale.join(" · ")
        }
    ));
    lines.push(String::new());

    let trimmed = truncate_with_tail(lines, Budget::TLDR_CAP);
    let body = render_lines(&trimmed);
    let line_count = count_lines(&body);
    Section { body, line_count }
}

fn top_aicx_intent(memory: &MemorySlice) -> Option<&MemoryEntry> {
    memory
        .entries
        .iter()
        .find(|e| matches!(e.authority, AuthorityLabel::AicxOperator))
        .or_else(|| memory.entries.first())
}

fn top_summarized_aicx_intent(memory: &MemorySlice) -> Option<&MemoryEntry> {
    memory
        .entries
        .iter()
        .filter(|entry| summarize_entry(&entry.text).structured)
        .find(|e| matches!(e.authority, AuthorityLabel::AicxOperator))
        .or_else(|| {
            memory
                .entries
                .iter()
                .find(|entry| summarize_entry(&entry.text).structured)
        })
}

fn top_three_warnings(input: &PillInput<'_>, m: &Metrics) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(top_hub) = input.pack.risk.hotspots.first() {
        out.push(format!(
            "Hub `{}` has {} importers — touching it = wide blast radius (of {} ranked hubs). (LoctreeDerived)",
            top_hub.file, top_hub.importers, m.hub_count
        ));
    } else if let Some(top_hub) = input.scope.top_hubs.first() {
        out.push(format!(
            "Hub `{}` has {} importers — touching it = wide blast radius (of {} ranked hubs). (LoctreeDerived)",
            top_hub.file, top_hub.importers, m.hub_count
        ));
    } else {
        out.push(
            "_no high-leverage hub detected in current snapshot scope._ (LoctreeDerived)"
                .to_string(),
        );
    }

    let risk_message = if snapshot_is_missing(input) {
        Some("No snapshot is loaded — run `loct scan` before trusting ContextPack metrics. (RepoVerified)".to_string())
    } else if input.pack.risk.stale_snapshot {
        Some("Snapshot is stale relative to current git HEAD — run `loct scan` before trusting derived facts. (RepoVerified)".to_string())
    } else if m.cycles_status.is_measured()
        && let MeasurementStatus::Measured(n) = &m.cycles_status
        && *n > 0
    {
        Some(format!(
            "{n} import cycle{plural} present — refactor risk. Run `loct cycles` for details. (LoctreeDerived)",
            plural = if *n == 1 { "" } else { "s" }
        ))
    } else if let MeasurementStatus::Measured(n) = &m.twins_status
        && *n > 0
    {
        Some(format!(
            "{n} twin duplicate(s) detected — review before reuse. (LoctreeDerived)"
        ))
    } else if matches!(m.dead_exports, DeadExportsStatus::Measured(n) if n > 0) {
        let DeadExportsStatus::Measured(n) = m.dead_exports else {
            unreachable!()
        };
        Some(format!(
            "{n} dead export(s) measured — run `loct dead` for the live list. (LoctreeDerived)"
        ))
    } else if !input.pack.risk.high_fan_in.is_empty() {
        Some(format!(
            "{} high-fan-in file(s) exceed threshold — review impact before edits. (LoctreeDerived)",
            input.pack.risk.high_fan_in.len()
        ))
    } else if input.pack.risk.dirty_worktree {
        Some("Worktree is dirty — auto-scope mirrors uncommitted edits, not committed state. (RepoVerified)".to_string())
    } else {
        None
    };
    out.push(risk_message.unwrap_or_else(|| {
        "_no significant risk spike detected in snapshot scope._ (LoctreeDerived)".to_string()
    }));

    if let Some(intent) = top_summarized_aicx_intent(&input.pack.memory) {
        let summary = summarize_entry(&intent.text);
        out.push(format!(
            "Active memory says `{}` — {} on {}. (AicxOperator)",
            summary.text, intent.agent, intent.date
        ));
    } else if let Some(env) = input.pack.runtime.env_contracts.first() {
        out.push(format!(
            "Env var `{}` is read by {} file(s) in scope — verify before refactoring config flow. (LoctreeDerived)",
            env.name,
            env.used_in_files.len()
        ));
    } else if !input.pack.runtime.dispatch_edges.is_empty() {
        out.push(format!(
            "{} dispatch edge(s) surfaced in scope — trace handlers before moving runtime bridge code. (LoctreeDerived)",
            input.pack.runtime.dispatch_edges.len()
        ));
    } else if let Some(latest) = input.scope.commit_hints.first() {
        // Git already told us what "recent activity" is — never claim
        // "no significant recent activity" while the same pill prints
        // fresh commits three sections up.
        out.push(format!(
            "Recent git activity: {n} commit(s) in scope, latest `{latest}`. (RepoVerified)",
            n = input.scope.commit_hints.len(),
        ));
    } else {
        // Honest empty-slot declaration, not a pseudo-insight: name what
        // was checked and came back empty.
        out.push(
            "_no data for this slot: AICX memory, env contracts, dispatch edges and commit history are all empty in scope._ (StaleOrUnknown)"
                .to_string(),
        );
    }
    out.truncate(3);
    out
}

fn collect_stale(input: &PillInput<'_>) -> Vec<String> {
    let mut out = Vec::new();
    if snapshot_is_missing(input) {
        out.push(
            "No snapshot loaded: run `loct scan` before relying on derived file/import metrics. (RepoVerified)"
                .to_string(),
        );
    }
    if input.pack.risk.stale_snapshot {
        out.push(format!(
            "Snapshot HEAD diverged from worktree HEAD ({}). (RepoVerified)",
            input.pack.project.commit.as_deref().unwrap_or("unknown")
        ));
    }
    // One honest line per distinct overlay outcome — "no rows" (store
    // answered: nothing there), "unavailable" (no transport) and
    // "skipped (timeout)" (store never got to answer) demand different
    // operator reactions and must not be blurred into one sentence.
    match input.aicx_status {
        AicxRenderStatus::Unavailable => {
            out.push(
                "AICX overlay unavailable: no transport reachable (see Memory section for setup hints). (StaleOrUnknown)"
                    .to_string(),
            );
        }
        AicxRenderStatus::EnabledNoRows => {
            out.push(
                "AICX overlay ran and returned no in-scope rows — no memory to apply, not an error. (StaleOrUnknown)"
                    .to_string(),
            );
        }
        AicxRenderStatus::SkippedTimeout => {
            out.push(
                "AICX overlay skipped (timeout): store did not answer within the session-start budget; use `--with-aicx` for patient recall. (StaleOrUnknown)"
                    .to_string(),
            );
        }
        AicxRenderStatus::EnabledWithRows | AicxRenderStatus::Disabled => {}
    }
    out
}

fn snapshot_is_missing(input: &PillInput<'_>) -> bool {
    matches!(input.pack.risk.cache_scope, RiskCacheScope::MissingSnapshot)
        || input.pack.risk.snapshot_health.as_deref() == Some("missing_snapshot")
}

fn snapshot_state_label(input: &PillInput<'_>) -> &'static str {
    if snapshot_is_missing(input) {
        "no snapshot - run `loct scan`"
    } else if input.pack.risk.stale_snapshot {
        "stale"
    } else {
        "fresh"
    }
}

// ----------------------------------------------------------------------------
// Section: Where You Are
// ----------------------------------------------------------------------------

fn render_where_you_are(input: &PillInput<'_>) -> Section {
    let mut buf = String::new();
    buf.push_str("## Where You Are\n\n");

    // Hubs
    buf.push_str("### Hubs — touch = blast radius\n\n");
    let hub_lines = render_hub_table(&input.pack.risk, &input.scope.top_hubs);
    let hubs_capped = truncate_with_tail(hub_lines, hubs_budget());
    buf.push_str(&render_lines(&hubs_capped));
    buf.push('\n');

    // Cycles
    buf.push_str("### Cycles\n\n");
    let cycle_status = derive_cycles_status();
    buf.push_str(&format!("- {} (LoctreeDerived)\n\n", cycle_status.label()));

    // Recently active surface
    buf.push_str("### Recently active surface (last 24h)\n\n");
    if input.scope.recent_files.is_empty() {
        buf.push_str("- No files modified in the last 24h. (RepoVerified)\n\n");
    } else {
        let recent_lines: Vec<String> = input
            .scope
            .recent_files
            .iter()
            .take(10)
            .map(|f| format!("- `{f}` (RepoVerified)"))
            .collect();
        let recent_capped = truncate_with_tail(recent_lines, 10);
        buf.push_str(&render_lines(&recent_capped));
        buf.push('\n');
    }

    // Risk metrics (honesty-forward)
    buf.push_str("### Risk metrics\n\n");
    buf.push_str(&format!(
        "- dead_exports: {} (RepoVerified)\n",
        DeadExportsStatus::default().label()
    ));
    let twins_status: MeasurementStatus = MeasurementStatus::NotMeasured(
        "run `loct twins` for a context-classified pass".to_string(),
    );
    buf.push_str(&format!(
        "- twins: {} (RepoVerified)\n",
        twins_status.label()
    ));
    buf.push_str(&format!(
        "- cache_scope: `{:?}` ({:?})\n",
        input.pack.risk.cache_scope, input.pack.risk.cache_scope_authority
    ));
    if input.pack.risk.high_fan_in.is_empty() {
        buf.push_str("- high_fan_in: 0 over threshold 10 (LoctreeDerived)\n");
    } else {
        buf.push_str(&format!(
            "- high_fan_in: {} files over threshold 10 (LoctreeDerived)\n",
            input.pack.risk.high_fan_in.len()
        ));
    }
    buf.push('\n');

    enforce_section_cap(buf, Budget::WHERE_CAP)
}

fn hubs_budget() -> usize {
    // Inside Where-You-Are, hubs may eat at most this many lines (header
    // + table fits inside 200-line section).
    50
}

fn derive_cycles_status() -> MeasurementStatus {
    // Pill v1: snapshots do not currently store a precomputed cycle count
    // alongside ContextPack. Be honest about it; operator can run
    // `loct cycles` for the current measurement.
    MeasurementStatus::NotMeasured("run `loct cycles` for current measurement".to_string())
}

fn render_hub_table(risk: &RiskSlice, fallback_hubs: &[HubEntry]) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    lines.push("| File | Importers | Authority |".to_string());
    lines.push("|---|---:|---|".to_string());

    // Ranked rendering with a tail-line cap, so noisy snapshots still respect
    // the section budget. Caps below leave headroom for the two header lines
    // already pushed above; `hubs_budget()` is the per-section ceiling.
    let cap = hubs_budget().saturating_sub(2).max(1);
    if !risk.hotspots.is_empty() {
        let ranked = rank_and_render(
            risk.hotspots.clone(),
            cap,
            |h| h.importers as i64,
            |h| format!("| `{}` | {} | LoctreeDerived |", h.file, h.importers),
        );
        lines.extend(ranked);
    } else if !fallback_hubs.is_empty() {
        let ranked = rank_and_render(
            fallback_hubs.to_vec(),
            cap.min(10),
            |h| h.importers as i64,
            |h| format!("| `{}` | {} | LoctreeDerived |", h.file, h.importers),
        );
        lines.extend(ranked);
    } else {
        lines.push("| _no import hubs in scope_ | — | LoctreeDerived |".to_string());
    }
    lines
}

// ----------------------------------------------------------------------------
// Section: What's Live
// ----------------------------------------------------------------------------

fn render_whats_live(input: &PillInput<'_>) -> Section {
    let mut buf = String::new();
    buf.push_str("## What's Live\n\n");

    // Idiom tag clusters
    buf.push_str("### Idiom tag clusters (top 10 by hit count)\n\n");
    let tags = top_idiom_tags(&input.pack.runtime, 10);
    if tags.is_empty() {
        buf.push_str("- _no named semantic-spine idioms in scope_ (LoctreeDerived)\n");
    } else {
        for cluster in &tags {
            buf.push_str(&format!(
                "- `{}` ({} hits) — {} ({})\n",
                cluster.label, cluster.count, cluster.summary, cluster.authority
            ));
        }
    }
    buf.push('\n');

    // Dispatch edges
    buf.push_str("### Dispatch edges (top 5 by reach)\n\n");
    if input.pack.runtime.dispatch_edges.is_empty() {
        buf.push_str("- _no dispatch edges in scope_ (LoctreeDerived)\n");
    } else {
        for edge in input.pack.runtime.dispatch_edges.iter().take(5) {
            buf.push_str(&format!(
                "- `{}`:{} → `{}` (`{}`) ({:?})\n",
                edge.from_file,
                edge.from_line,
                edge.handler_symbol,
                edge.dispatch_kind,
                edge.authority,
            ));
        }
    }
    buf.push('\n');

    // Env contracts
    buf.push_str("### Env contracts (vars used in scope)\n\n");
    if input.pack.runtime.env_contracts.is_empty() {
        buf.push_str("- _no env vars surfaced in scope_ (LoctreeDerived)\n");
    } else {
        for env in input.pack.runtime.env_contracts.iter().take(10) {
            buf.push_str(&format!(
                "- `{}` — used in {} file(s) ({:?})\n",
                env.name,
                env.used_in_files.len(),
                env.authority,
            ));
        }
    }
    buf.push('\n');

    // Reachability — surface widely-reached and unreached symbols separately
    buf.push_str("### Reachability (top 5 widely-reached symbols)\n\n");
    let mut reached: Vec<&crate::pack::RuntimeReachability> = input
        .pack
        .runtime
        .reachability
        .iter()
        .filter(|r| r.reached)
        .collect();
    reached.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    if reached.is_empty() {
        buf.push_str("- _no reachability claims in scope_ (LoctreeDerived)\n");
    } else {
        for r in reached.iter().take(5) {
            buf.push_str(&format!(
                "- `{}` — reason: `{}` ({:?})\n",
                r.symbol, r.reason, r.authority
            ));
        }
    }
    buf.push('\n');

    enforce_section_cap(buf, Budget::LIVE_CAP)
}

#[derive(Debug, Clone)]
struct IdiomCluster {
    label: String,
    count: usize,
    summary: String,
    authority: &'static str,
}

fn top_idiom_tags(runtime: &RuntimeSlice, limit: usize) -> Vec<IdiomCluster> {
    use std::collections::HashMap;
    let mut counts: HashMap<String, (usize, AuthorityLabel, Vec<String>)> = HashMap::new();
    for tag in &runtime.idiom_tags {
        let Some((label, sample)) = named_idiom_cluster(tag) else {
            continue;
        };
        let entry = counts
            .entry(label)
            .or_insert((0, tag.authority, Vec::new()));
        entry.0 += 1;
        if entry.2.len() < 4 && !entry.2.contains(&sample) {
            entry.2.push(sample);
        }
        // Promote to strongest authority encountered (RepoVerified > LoctreeDerived > SemanticGuess > others)
        if authority_rank(tag.authority) > authority_rank(entry.1) {
            entry.1 = tag.authority;
        }
    }
    let mut ranked: Vec<(String, usize, AuthorityLabel, Vec<String>)> = counts
        .into_iter()
        .map(|(name, (count, auth, samples))| (name, count, auth, samples))
        .collect();
    ranked.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| authority_rank(b.2).cmp(&authority_rank(a.2)))
            .then_with(|| a.0.cmp(&b.0))
    });
    ranked
        .into_iter()
        .take(limit)
        .map(|(label, count, auth, samples)| IdiomCluster {
            summary: idiom_cluster_summary(&label, &samples),
            label,
            count,
            authority: authority_label_str(auth),
        })
        .collect()
}

fn named_idiom_cluster(tag: &RuntimeIdiomTag) -> Option<(String, String)> {
    let symbol_path = tag
        .symbol
        .split_once("::")
        .map(|(path, _)| path)
        .unwrap_or(tag.symbol.as_str());
    let name = tag.name.as_str();
    let classifier = tag.classifier.as_str();

    if classifier == "rust:trait_impl_method" {
        return Some((
            "idiom:rust-trait-impl".to_string(),
            symbol_leaf(&tag.symbol),
        ));
    }
    if classifier == "rust:inherent_impl_method" {
        return Some((
            "idiom:rust-inherent-impl".to_string(),
            symbol_leaf(&tag.symbol),
        ));
    }
    if classifier == "rust:derive_emission" {
        return Some((
            "idiom:rust-derive-emission".to_string(),
            symbol_leaf(&tag.symbol),
        ));
    }
    if classifier == "rust:pub_use_reexport" {
        return Some(("idiom:rust-reexport".to_string(), symbol_leaf(&tag.symbol)));
    }
    if symbol_path.ends_with(".rs") && classifier == "primary_entrypoint" {
        return Some(("idiom:rust-entrypoint".to_string(), name.to_string()));
    }
    if classifier == "dispatch_handler"
        && (name.contains("tauri") || name == "invoke" || name == "generate_handler!")
    {
        return Some(("idiom:tauri-command".to_string(), name.to_string()));
    }
    if name == "frontend-orphan" || name == "dead-likely-tauri" {
        return Some(("idiom:tauri-command".to_string(), name.to_string()));
    }
    if symbol_path.ends_with(".sh")
        || symbol_path.ends_with(".bash")
        || symbol_path.ends_with(".zsh")
        || matches!(
            name,
            "usage"
                | "die"
                | "main"
                | "info"
                | "warn"
                | "error"
                | "log"
                | "cleanup"
                | "trap_handler"
                | "have"
                | "exists"
                | "require"
                | "banner"
                | "success"
                | "confirm"
        )
    {
        return Some(("idiom:shell-helper".to_string(), name.to_string()));
    }
    if symbol_path.ends_with("Makefile") || symbol_path.ends_with(".mk") {
        return Some(("idiom:make-target".to_string(), name.to_string()));
    }
    if symbol_path.ends_with(".py")
        && matches!(
            classifier,
            "fastapi_route"
                | "flask_route"
                | "click_command"
                | "python_decorator"
                | "dispatch_handler"
        )
    {
        return Some(("idiom:python-decorator-route".to_string(), name.to_string()));
    }

    None
}

fn symbol_leaf(symbol: &str) -> String {
    symbol
        .rsplit_once("::")
        .map(|(_, leaf)| leaf)
        .unwrap_or(symbol)
        .to_string()
}

fn idiom_cluster_summary(label: &str, samples: &[String]) -> String {
    let sample = if samples.is_empty() {
        "scope matches".to_string()
    } else {
        samples.join("/")
    };
    match label {
        "idiom:shell-helper" => format!("{sample} shell helpers"),
        "idiom:rust-trait-impl" => "trait reachability via impls".to_string(),
        "idiom:rust-inherent-impl" => "method-call reachability via inherent impls".to_string(),
        "idiom:rust-derive-emission" => "derive-emitted method reachability".to_string(),
        "idiom:rust-reexport" => "public re-export surface".to_string(),
        "idiom:rust-entrypoint" => "Rust executable entrypoints".to_string(),
        "idiom:tauri-command" => format!("{sample} Tauri command/event bridge patterns"),
        "idiom:make-target" => format!("{sample} Make workflow targets"),
        "idiom:python-decorator-route" => format!("{sample} decorator-routed call sites"),
        _ => sample,
    }
}

fn authority_rank(label: AuthorityLabel) -> u8 {
    match label {
        AuthorityLabel::RepoVerified => 6,
        AuthorityLabel::LoctreeDerived => 5,
        AuthorityLabel::AicxOperator => 4,
        AuthorityLabel::AicxAgent => 3,
        AuthorityLabel::AicxFailure => 2,
        AuthorityLabel::SemanticGuess => 1,
        AuthorityLabel::StaleOrUnknown => 0,
    }
}

fn authority_label_str(label: AuthorityLabel) -> &'static str {
    match label {
        AuthorityLabel::RepoVerified => "RepoVerified",
        AuthorityLabel::LoctreeDerived => "LoctreeDerived",
        AuthorityLabel::AicxOperator => "AicxOperator",
        AuthorityLabel::AicxAgent => "AicxAgent",
        AuthorityLabel::AicxFailure => "AicxFailure",
        AuthorityLabel::SemanticGuess => "SemanticGuess",
        AuthorityLabel::StaleOrUnknown => "StaleOrUnknown",
    }
}

// ----------------------------------------------------------------------------
// Section: Memory
// ----------------------------------------------------------------------------

fn render_memory(input: &PillInput<'_>) -> Section {
    let mut buf = String::new();
    buf.push_str("## Memory\n\n");

    match input.aicx_status {
        AicxRenderStatus::Disabled => {
            buf.push_str(
                "_AICX overlay disabled by `--no-aicx`; memory overlay intentionally omitted._ (RepoVerified)\n\n",
            );
        }
        AicxRenderStatus::Unavailable => {
            buf.push_str(
                "_AICX unavailable (no transport reachable: install `aicx` CLI or run `aicx-mcp` server, or set `LOCT_AICX_BINARY` / `AICX_MCP_BINARY` overrides). Memory overlay is empty by design until a transport responds; treat any prior memory claims for this scope as unverified continuity hints, not task-scoped truth._ (StaleOrUnknown)\n\n",
            );
        }
        AicxRenderStatus::EnabledNoRows => {
            buf.push_str(
                "_AICX overlay engaged but produced no in-scope rows in the last 168h._ (StaleOrUnknown)\n\n",
            );
        }
        AicxRenderStatus::SkippedTimeout => {
            buf.push_str(
                "_AICX overlay skipped (timeout): the store did not answer within the session-start budget. No data, not an empty store — rerun with `loct context --with-aicx` (patient) or raise `LOCT_CONTEXT_AICX_BUDGET_MS`._ (StaleOrUnknown)\n\n",
            );
        }
        AicxRenderStatus::EnabledWithRows => {
            buf.push_str(
                "### Recent decisions / intents / outcomes (last 168h, top by relevance)\n\n",
            );
            let entries = &input.pack.memory.entries;
            let mut lines: Vec<String> = Vec::new();
            for (idx, entry) in entries.iter().enumerate().take(15) {
                let auth = authority_label_str(entry.authority);
                let date = if entry.date.is_empty() {
                    "<no-date>"
                } else {
                    entry.date.as_str()
                };
                let summary = summarize_entry(&entry.text);
                lines.push(format!(
                    "{n}. `{kind}` · {agent} · {date} · _{text}_ ({auth}) — chunk: `{chunk}`",
                    n = idx + 1,
                    kind = entry.kind,
                    agent = entry.agent,
                    date = date,
                    text = summary.text.replace('\n', " ").replace('|', "\\|"),
                    auth = auth,
                    chunk = chunk_ref(&entry.source_chunk),
                ));
            }
            if entries.is_empty() {
                buf.push_str("_No in-scope intents._ (StaleOrUnknown)\n\n");
            } else {
                let capped = truncate_with_tail(lines, 80);
                buf.push_str(&render_lines(&capped));
                buf.push('\n');
            }
            if !input.pack.memory.source_chunks.is_empty() {
                buf.push_str("### Source chunk pointers\n\n");
                buf.push_str(&format!(
                    "_{n} unique chunk(s) reachable via `aicx open <chunk:ref>` (resolved against the operator's local aicx store; absolute paths intentionally redacted to keep this context-pack commitable)._\n\n",
                    n = input.pack.memory.source_chunks.len()
                ));
            }
        }
    }

    enforce_section_cap(buf, Budget::MEMORY_CAP)
}

// ----------------------------------------------------------------------------
// Section: Action
// ----------------------------------------------------------------------------

fn render_action(input: &PillInput<'_>) -> Section {
    let mut buf = String::new();
    buf.push_str("## Action\n\n");

    if !input.pack.action.power_path.is_empty() {
        buf.push_str("### Power Path (suggested next steps)\n\n");
        for sug in &input.pack.action.power_path {
            buf.push_str(&format!("- `{}`: {}\n", sug.command, sug.reason));
        }
        buf.push('\n');
    }

    buf.push_str("### Next safe commands (grounded, ≤3)\n\n");
    if input.pack.action.next_safe_commands.is_empty() {
        buf.push_str("_No specific suggestion for the current scope._ (SemanticGuess)\n\n");
    } else {
        buf.push_str("```bash\n");
        for cmd in input.pack.action.next_safe_commands.iter().take(3) {
            buf.push_str(cmd);
            buf.push('\n');
        }
        buf.push_str("```\n\n");
    }

    buf.push_str("### Verification gates (run before any commit)\n\n");
    if input.pack.action.verification_gates.is_empty() {
        buf.push_str("- `cargo check -p loctree` (SemanticGuess)\n\n");
    } else {
        for gate in input.pack.action.verification_gates.iter().take(8) {
            buf.push_str(&format!("- `{gate}` (SemanticGuess)\n"));
        }
        buf.push('\n');
    }

    buf.push_str("### Likely tests for current scope\n\n");
    if input.pack.action.likely_tests.is_empty() {
        buf.push_str("- _no tests directly cover the current scope._ (LoctreeDerived)\n\n");
    } else {
        for test in input.pack.action.likely_tests.iter().take(10) {
            buf.push_str(&format!("- `{test}` (LoctreeDerived)\n"));
        }
        buf.push('\n');
    }

    enforce_section_cap(buf, Budget::ACTION_CAP)
}

// ----------------------------------------------------------------------------
// Section: Authority Index
// ----------------------------------------------------------------------------

fn render_authority_index(input: &PillInput<'_>) -> Section {
    let mut buf = String::new();
    buf.push_str("## Authority Index\n\n");

    let a = &input.pack.authority;
    let total = total_authority_claims(a);
    buf.push_str(&format!(
        "_{total} total claims tagged with provenance._\n\n"
    ));

    buf.push_str(&format!(
        "- **RepoVerified** ({n}) — AST + git state derivation\n",
        n = a.repo_verified.len()
    ));
    buf.push_str(&format!(
        "- **LoctreeDerived** ({n}) — snapshot edges + idiom catalogs\n",
        n = a.loctree_derived.len()
    ));
    buf.push_str(&format!(
        "- **AicxOperator** ({n}) — operator decisions / intents from AICX\n",
        n = a.aicx_operator.len()
    ));
    buf.push_str(&format!(
        "- **AicxAgent** ({n}) — agent outcomes (verify before propagating)\n",
        n = a.aicx_agent.len()
    ));
    buf.push_str(&format!(
        "- **AicxFailure** ({n}) — anti-recommendations (avoid repeating)\n",
        n = a.aicx_failure.len()
    ));
    buf.push_str(&format!(
        "- **SemanticGuess** ({n}) — heuristic only, low trust\n",
        n = a.semantic_guess.len()
    ));
    buf.push_str(&format!(
        "- **StaleOrUnknown** ({n}) — explicit \"we don't know\"\n",
        n = a.stale_or_unknown.len()
    ));
    buf.push('\n');

    enforce_section_cap(buf, Budget::AUTHORITY_CAP)
}

fn total_authority_claims(a: &AuthoritySlice) -> usize {
    a.repo_verified.len()
        + a.loctree_derived.len()
        + a.aicx_operator.len()
        + a.aicx_agent.len()
        + a.aicx_failure.len()
        + a.semantic_guess.len()
        + a.stale_or_unknown.len()
}

// ----------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------

fn render_lines(lines: &[String]) -> String {
    let mut buf = String::new();
    for line in lines {
        buf.push_str(line);
        buf.push('\n');
    }
    buf
}

fn enforce_section_cap(body: String, cap: usize) -> Section {
    let line_count = count_lines(&body);
    if line_count <= cap {
        return Section { body, line_count };
    }
    // Split, keep first `cap - 1` lines, append tail.
    let mut lines: Vec<&str> = body.lines().collect();
    lines.truncate(cap.saturating_sub(1));
    let mut new_body = lines.join("\n");
    new_body.push('\n');
    new_body.push_str("+ section truncated, run `loct context --full` for full data\n");
    let new_count = count_lines(&new_body);
    Section {
        body: new_body,
        line_count: new_count,
    }
}

fn enforce_total_ceiling(body: String) -> String {
    let lc = count_lines(&body);
    if lc <= Budget::TOTAL_CEILING {
        return body;
    }
    let mut lines: Vec<&str> = body.lines().collect();
    lines.truncate(Budget::TOTAL_CEILING.saturating_sub(1));
    let mut shrunk = lines.join("\n");
    shrunk.push('\n');
    shrunk.push_str("+ pill exceeded global ceiling, run `loct context --full` for full data\n");
    shrunk
}

fn collect_metrics(
    input: &PillInput<'_>,
    where_section: &Section,
    live_section: &Section,
    memory_section: &Section,
) -> Metrics {
    Metrics {
        hub_count: input
            .pack
            .risk
            .hotspots
            .len()
            .max(input.scope.top_hubs.len()),
        cycles_status: derive_cycles_status(),
        dead_exports: DeadExportsStatus::default(),
        twins_status: MeasurementStatus::NotMeasured(
            "run `loct twins` for current measurement".to_string(),
        ),
        idiom_tag_count: input.pack.runtime.idiom_tags.len(),
        dispatch_edge_count: input.pack.runtime.dispatch_edges.len(),
        env_contract_count: input.pack.runtime.env_contracts.len(),
        memory_entry_count: input.pack.memory.entries.len(),
        authority_total: total_authority_claims(&input.pack.authority),
        where_lines: where_section.line_count,
        live_lines: live_section.line_count,
        memory_lines: memory_section.line_count,
    }
}

/// Bundle of metadata threaded into [`build_input`].
///
/// Bundling these together keeps the public API small and avoids the
/// "too many arguments" clippy lint while still letting the caller
/// produce them at different times (project/timestamp at startup, file
/// counts after snapshot load, AICX status after memory composition).
pub struct PillRenderMetadata {
    pub project_name: String,
    pub timestamp_iso: String,
    pub snapshot_files: usize,
    pub snapshot_edges: usize,
    pub elapsed_start: Instant,
    pub aicx_status: AicxRenderStatus,
}

/// Build a `PillInput` for the renderer given the raw pack + auto-scope
/// plus a [`PillRenderMetadata`] bundle of timing / project metadata.
pub fn build_input<'a>(
    pack: &'a ContextPack,
    scope: &'a AutoScope,
    metadata: PillRenderMetadata,
) -> PillInput<'a> {
    PillInput {
        pack,
        scope,
        project_name: metadata.project_name,
        timestamp_iso: metadata.timestamp_iso,
        snapshot_files: metadata.snapshot_files,
        snapshot_edges: metadata.snapshot_edges,
        elapsed: metadata.elapsed_start.elapsed(),
        aicx_status: metadata.aicx_status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dispatch::handlers::context::scope::{AutoScope, HubEntry};
    use crate::pack::{
        AuthorityLabel, ContextPack, MemoryEntry, MemorySlice, ProjectIdentity,
        RuntimeDispatchEdge, RuntimeEnvContract, RuntimeIdiomTag,
    };

    fn fixture_pack() -> ContextPack {
        let mut pack = ContextPack::empty(ProjectIdentity {
            canonical_root: Some("/tmp/loctree".to_string()),
            branch: Some("feat/cut11-context-pill".to_string()),
            commit: Some("abc1234".to_string()),
            snapshot_id: None,
        });
        pack.authority.repo_verified = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        pack.authority.loctree_derived = vec!["d".to_string(), "e".to_string()];
        pack.authority.aicx_operator = vec!["f".to_string()];
        pack.authority.aicx_agent = vec!["g".to_string(), "h".to_string()];
        pack.authority.aicx_failure = vec!["i".to_string()];
        pack.authority.semantic_guess = vec!["j".to_string()];
        pack.authority.stale_or_unknown = vec!["k".to_string()];
        pack.action.next_safe_commands = vec!["loct slice src/foo.rs".to_string()];
        pack.action.verification_gates = vec!["cargo check -p loctree".to_string()];
        pack
    }

    fn fixture_scope() -> AutoScope {
        AutoScope {
            branch: Some("feat/cut11-context-pill".to_string()),
            branch_hint: Some("Cut11 Context Pill".to_string()),
            commit_hints: vec![
                "abc Hello world".to_string(),
                "def Another".to_string(),
                "ghi Third".to_string(),
            ],
            top_hubs: vec![HubEntry {
                file: "loctree-rs/src/types.rs".to_string(),
                importers: 63,
            }],
            recent_files: vec!["loctree-rs/src/cli/dispatch/handlers/context/pill.rs".to_string()],
            dirty: true,
            changed_files: vec!["src/foo.rs".to_string()],
        }
    }

    fn input_with<'a>(
        pack: &'a ContextPack,
        scope: &'a AutoScope,
        aicx: AicxRenderStatus,
    ) -> PillInput<'a> {
        PillInput {
            pack,
            scope,
            project_name: "loctree-suite".to_string(),
            timestamp_iso: "2026-04-28T16:58:02Z".to_string(),
            snapshot_files: 231,
            snapshot_edges: 421,
            elapsed: std::time::Duration::from_millis(120),
            aicx_status: aicx,
        }
    }

    #[test]
    fn pill_renders_six_top_level_sections_in_fixed_order() {
        let pack = fixture_pack();
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::Disabled);
        let md = render_pill(input);

        // Verify section ordering.
        let tldr_pos = md
            .find("## TL;DR — read this first")
            .expect("TL;DR present");
        let where_pos = md.find("## Where You Are").expect("Where You Are present");
        let live_pos = md.find("## What's Live").expect("What's Live present");
        let memory_pos = md.find("## Memory").expect("Memory present");
        let action_pos = md.find("## Action").expect("Action present");
        let authority_pos = md
            .find("## Authority Index")
            .expect("Authority Index present");

        assert!(tldr_pos < where_pos, "TL;DR must precede Where You Are");
        assert!(where_pos < live_pos);
        assert!(live_pos < memory_pos);
        assert!(memory_pos < action_pos);
        assert!(action_pos < authority_pos);
    }

    #[test]
    fn pill_total_line_count_under_ceiling() {
        let pack = fixture_pack();
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::Disabled);
        let md = render_pill(input);
        let lines = count_lines(&md);
        assert!(
            lines <= Budget::TOTAL_CEILING,
            "pill exceeded {} lines (got {})",
            Budget::TOTAL_CEILING,
            lines
        );
    }

    #[test]
    fn pill_surfaces_authority_labels_for_every_metric() {
        let pack = fixture_pack();
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::Disabled);
        let md = render_pill(input);
        let label_count = [
            "RepoVerified",
            "LoctreeDerived",
            "SemanticGuess",
            "StaleOrUnknown",
        ]
        .iter()
        .map(|label| md.matches(label).count())
        .sum::<usize>();
        assert!(
            label_count >= 10,
            "expected ≥10 authority label mentions, got {label_count}"
        );
    }

    #[test]
    fn pill_dead_exports_never_silent_zero() {
        let pack = fixture_pack();
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::Disabled);
        let md = render_pill(input);
        // The pill must NEVER render a bare `dead_exports: 0`.
        assert!(
            !md.contains("dead_exports: 0\n") && !md.contains("dead_exports: 0\r"),
            "pill emitted a silent dead_exports zero — broken honesty contract"
        );
        assert!(
            md.contains("not_measured") && (md.contains("re-exports") || md.contains("loct dead")),
            "expected an explanation of why dead_exports is unmeasured"
        );
    }

    #[test]
    fn pill_no_semantic_matches_in_output() {
        let pack = fixture_pack();
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::Disabled);
        let md = render_pill(input);
        assert!(
            !md.contains("semantic_matches"),
            "pill must not surface placebo semantic_matches scores"
        );
    }

    #[test]
    fn pill_section_truncation_kicks_in_when_body_overflows_cap() {
        // Stress the per-section cap directly: enforce_section_cap is
        // called by every render_*_section helper. A 600-line synthetic
        // body trips the LIVE_CAP=200 ceiling and must surface a tail
        // line pointing the operator to `loct context --full`.
        let body = (0..600)
            .map(|i| format!("- row {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let section = enforce_section_cap(format!("{body}\n"), Budget::LIVE_CAP);
        assert!(
            section.line_count <= Budget::LIVE_CAP,
            "enforce_section_cap must cap line count at LIVE_CAP, got {}",
            section.line_count
        );
        assert!(
            section.body.contains("loct context --full"),
            "truncated section must mention `loct context --full` for power users"
        );
    }

    #[test]
    fn pill_footer_always_mentions_full_for_full_contextpack() {
        let pack = fixture_pack();
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::Disabled);
        let md = render_pill(input);
        assert!(
            md.contains("`--full`"),
            "footer must point operators at `loct context --full` for full ContextPack data"
        );
    }

    #[test]
    fn pill_aicx_disabled_message_explains_opt_out() {
        let pack = fixture_pack();
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::Disabled);
        let md = render_pill(input);
        assert!(
            md.contains("--no-aicx") && md.contains("intentionally omitted"),
            "Memory section must explain explicit AICX opt-out"
        );
    }

    #[test]
    fn pill_aicx_enabled_no_rows_marks_stale() {
        let pack = fixture_pack();
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::EnabledNoRows);
        let md = render_pill(input);
        assert!(
            md.contains("StaleOrUnknown"),
            "empty AICX result must surface as StaleOrUnknown, not a silent gap"
        );
    }

    #[test]
    fn pill_aicx_unavailable_names_binary_contract() {
        let pack = fixture_pack();
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::Unavailable);
        let md = render_pill(input);
        assert!(md.contains("AICX unavailable"));
        assert!(md.contains("LOCT_AICX_BINARY"));
    }

    #[test]
    fn pill_tldr_always_renders_three_curated_lines() {
        let mut pack = fixture_pack();
        pack.risk.hotspots.clear();
        pack.risk.high_fan_in.clear();
        let mut scope = fixture_scope();
        scope.top_hubs.clear();
        let input = input_with(&pack, &scope, AicxRenderStatus::Disabled);
        let md = render_pill(input);
        assert!(md.contains("1. _no high-leverage hub detected"));
        assert!(md.contains("2. _no significant risk spike detected"));
        // Zero-filler contract: the fixture scope carries commit hints, so
        // slot 3 must quote real git activity instead of pretending there
        // is "no significant recent activity" next to fresh commits.
        assert!(
            md.contains("3. Recent git activity: 3 commit(s)"),
            "slot 3 must surface commit hints when git has them: {md}"
        );
        assert!(
            !md.contains("no significant recent activity"),
            "filler card leaked while commit hints exist"
        );
    }

    #[test]
    fn pill_tldr_empty_slot_declares_no_data_honestly() {
        let mut pack = fixture_pack();
        pack.risk.hotspots.clear();
        pack.risk.high_fan_in.clear();
        let mut scope = fixture_scope();
        scope.top_hubs.clear();
        scope.commit_hints.clear();
        let input = input_with(&pack, &scope, AicxRenderStatus::Disabled);
        let md = render_pill(input);
        // With every source empty, the slot declares the absence and names
        // what was checked — one honest line, not a pseudo-conclusion.
        assert!(
            md.contains("3. _no data for this slot"),
            "empty slot must declare absence explicitly: {md}"
        );
        assert!(md.contains("commit history are all empty in scope"));
    }

    #[test]
    fn pill_aicx_skipped_timeout_renders_explicit_status() {
        let pack = fixture_pack();
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::SkippedTimeout);
        let md = render_pill(input);
        assert!(
            md.contains("AICX overlay skipped (timeout)"),
            "timeout must be an explicit status, not a silent gap: {md}"
        );
        assert!(
            md.contains("--with-aicx") && md.contains("LOCT_CONTEXT_AICX_BUDGET_MS"),
            "timeout message must name the patient-recall escape hatches"
        );
        assert!(
            !md.contains("produced no rows or is unavailable"),
            "old blurred no-rows/unavailable phrasing must not come back"
        );
    }

    #[test]
    fn pill_whats_stale_distinguishes_no_rows_from_unavailable() {
        let pack = fixture_pack();
        let scope = fixture_scope();

        let no_rows = render_pill(input_with(&pack, &scope, AicxRenderStatus::EnabledNoRows));
        assert!(
            no_rows.contains("returned no in-scope rows — no memory to apply, not an error"),
            "no-rows must read as an answer, not a failure: {no_rows}"
        );

        let unavailable = render_pill(input_with(&pack, &scope, AicxRenderStatus::Unavailable));
        assert!(
            unavailable.contains("AICX overlay unavailable: no transport reachable"),
            "unavailable must name the transport gap: {unavailable}"
        );
    }

    #[test]
    fn pill_top_three_skips_unsummarizable_aicx_blob() {
        let mut pack = fixture_pack();
        pack.risk.hotspots.clear();
        pack.risk.high_fan_in.clear();
        pack.memory = MemorySlice {
            entries: vec![MemoryEntry {
                kind: "outcome".to_string(),
                text: "{ \"unknown\": \"shape\", \"payload\": \"@@ -1,2 +1,2\" }".to_string(),
                authority: AuthorityLabel::AicxOperator,
                source_chunk: "/tmp/aicx/store/raw.md".to_string(),
                agent: "codex".to_string(),
                date: "2026-06-11".to_string(),
                timestamp: None,
                session_id: "raw".to_string(),
                project: "loctree-suite".to_string(),
                relevance: 9,
                retrieval_score: None,
                retrieval_label: None,
                retrieval_mode: None,
                low_lexical_match: false,
            }],
            source_chunks: vec!["/tmp/aicx/store/raw.md".to_string()],
            diagnostic: None,
        };
        pack.runtime.env_contracts.push(RuntimeEnvContract {
            name: "LOCT_AICX_BINARY".to_string(),
            used_in_files: vec!["loctree-rs/src/aicx/mod.rs".to_string()],
            required_for: vec!["AICX CLI override".to_string()],
            occurrences: Vec::new(),
            required: false,
            authority: AuthorityLabel::LoctreeDerived,
        });
        let mut scope = fixture_scope();
        scope.top_hubs.clear();

        let input = input_with(&pack, &scope, AicxRenderStatus::EnabledWithRows);
        let md = render_pill(input);
        let top_three_start = md.find("**Top 3 things to know").expect("top3 present");
        let snapshot_facts = md[top_three_start..]
            .find("**Snapshot facts.")
            .expect("snapshot facts present")
            + top_three_start;
        let top_three = &md[top_three_start..snapshot_facts];

        assert!(
            !top_three.contains("raw AICX entry"),
            "Top 3 must not promote unsummarizable raw AICX entries: {top_three}"
        );
        assert!(
            top_three.contains("LOCT_AICX_BINARY"),
            "Top 3 should fall back to structural/runtime facts when AICX is raw: {top_three}"
        );
    }

    #[test]
    fn pill_memory_raw_blob_renders_one_line_with_chunk_ref() {
        let mut pack = fixture_pack();
        pack.memory = MemorySlice {
            entries: vec![MemoryEntry {
                kind: "outcome".to_string(),
                text: "{ \"unknown\": \"shape\", \"payload\": \"@@ -1,2 +1,2\" }".to_string(),
                authority: AuthorityLabel::AicxAgent,
                source_chunk: "/tmp/aicx/store/raw.md".to_string(),
                agent: "codex".to_string(),
                date: "2026-06-11".to_string(),
                timestamp: None,
                session_id: "raw".to_string(),
                project: "loctree-suite".to_string(),
                relevance: 9,
                retrieval_score: None,
                retrieval_label: None,
                retrieval_mode: None,
                low_lexical_match: false,
            }],
            source_chunks: vec!["/tmp/aicx/store/raw.md".to_string()],
            diagnostic: None,
        };

        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::EnabledWithRows);
        let md = render_pill(input);
        let memory_start = md.find("## Memory").expect("memory section present");
        let action_start = md[memory_start..]
            .find("## Action")
            .expect("action section present")
            + memory_start;
        let memory = &md[memory_start..action_start];
        let memory_line = memory
            .lines()
            .find(|line| line.contains("raw AICX entry"))
            .expect("raw entry summary line present");

        assert!(memory_line.contains("chunk:"));
        assert!(!memory_line.contains("@@"));
        assert_eq!(memory_line.matches("raw AICX entry").count(), 1);
    }

    #[test]
    fn pill_filters_raw_idioms_to_named_spine_clusters() {
        let mut pack = fixture_pack();
        pack.runtime.idiom_tags = vec![
            RuntimeIdiomTag {
                symbol: "loctree-rs/src/types.rs::new".to_string(),
                name: "new".to_string(),
                classifier: "library_helper".to_string(),
                runtime_role: "library_helper".to_string(),
                source: "embedded_default".to_string(),
                reasoning: "raw Rust constructor".to_string(),
                authority: AuthorityLabel::LoctreeDerived,
            },
            RuntimeIdiomTag {
                symbol: "scripts/install.sh::usage".to_string(),
                name: "usage".to_string(),
                classifier: "help_printer".to_string(),
                runtime_role: "user_facing".to_string(),
                source: "embedded_default".to_string(),
                reasoning: "shell helper".to_string(),
                authority: AuthorityLabel::LoctreeDerived,
            },
            RuntimeIdiomTag {
                symbol: "src/lib.rs::fmt".to_string(),
                name: "fmt".to_string(),
                classifier: "rust:trait_impl_method".to_string(),
                runtime_role: "library_helper".to_string(),
                source: "inferred_from_code".to_string(),
                reasoning: "trait impl".to_string(),
                authority: AuthorityLabel::SemanticGuess,
            },
            RuntimeIdiomTag {
                symbol: "src-tauri/src/main.rs::greet".to_string(),
                name: "#[tauri::command]".to_string(),
                classifier: "dispatch_handler".to_string(),
                runtime_role: "public_entrypoint".to_string(),
                source: "embedded_default".to_string(),
                reasoning: "tauri command".to_string(),
                authority: AuthorityLabel::LoctreeDerived,
            },
        ];
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::Disabled);
        let md = render_pill(input);
        assert!(md.contains("`idiom:shell-helper`"));
        assert!(md.contains("`idiom:rust-trait-impl`"));
        assert!(md.contains("`idiom:tauri-command`"));
        assert!(!md.contains("`new`"));
    }

    #[test]
    fn pill_renders_dispatch_edges_and_env_contracts() {
        let mut pack = fixture_pack();
        pack.runtime.dispatch_edges.push(RuntimeDispatchEdge {
            from_file: "src/api.ts".to_string(),
            from_line: 12,
            dispatch_kind: "tauri_invoke".to_string(),
            handler_symbol: "greet_user".to_string(),
            handler_file: Some("src-tauri/src/commands.rs".to_string()),
            framework: None,
            http_method: None,
            http_path: None,
            authority: AuthorityLabel::LoctreeDerived,
        });
        pack.runtime.env_contracts.push(RuntimeEnvContract {
            name: "LOCT_CACHE_DIR".to_string(),
            used_in_files: vec!["loctree-rs/src/cache.rs".to_string()],
            required_for: vec!["cache override".to_string()],
            occurrences: Vec::new(),
            required: false,
            authority: AuthorityLabel::LoctreeDerived,
        });
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::Disabled);
        let md = render_pill(input);
        assert!(md.contains("greet_user"));
        assert!(md.contains("LOCT_CACHE_DIR"));
    }

    #[test]
    fn pill_aicx_enabled_with_rows_lists_entries() {
        let mut pack = fixture_pack();
        pack.memory = MemorySlice {
            entries: vec![MemoryEntry {
                kind: "decision".to_string(),
                text: "Adopt pill renderer".to_string(),
                authority: AuthorityLabel::AicxOperator,
                source_chunk: "/tmp/aicx/store/s1.md".to_string(),
                agent: "claude".to_string(),
                date: "2026-04-28".to_string(),
                timestamp: None,
                session_id: "s1".to_string(),
                project: "loctree-suite".to_string(),
                relevance: 10,
                retrieval_score: None,
                retrieval_label: None,
                retrieval_mode: None,
                low_lexical_match: false,
            }],
            source_chunks: vec!["/tmp/aicx/store/s1.md".to_string()],
            diagnostic: None,
        };
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::EnabledWithRows);
        let md = render_pill(input);
        assert!(
            md.contains("Adopt pill renderer"),
            "memory entry must surface"
        );
        assert!(md.contains("AicxOperator"));
    }

    /// Regression for loctree-feedback hak 2026-05-23 #2: aicx-store absolute
    /// paths must never leak into rendered context output, including the
    /// per-entry chunk hint and the Source-chunk-pointers footer.
    #[test]
    fn pill_aicx_memory_does_not_leak_absolute_aicx_store_paths() {
        let mut pack = fixture_pack();
        let leaky_path = "/home/polyversai/.aicx/store/Loctree/loctree-suite/2026_0525/conversations/claude/s1.md";
        pack.memory = MemorySlice {
            entries: vec![MemoryEntry {
                kind: "decision".to_string(),
                text: "Adopt pill renderer".to_string(),
                authority: AuthorityLabel::AicxOperator,
                source_chunk: leaky_path.to_string(),
                agent: "claude".to_string(),
                date: "2026-04-28".to_string(),
                timestamp: None,
                session_id: "s1".to_string(),
                project: "loctree-suite".to_string(),
                relevance: 10,
                retrieval_score: None,
                retrieval_label: None,
                retrieval_mode: None,
                low_lexical_match: false,
            }],
            source_chunks: vec![leaky_path.to_string()],
            diagnostic: None,
        };
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::EnabledWithRows);
        let md = render_pill(input);
        assert!(
            !md.contains("/home/polyversai/.aicx/store/"),
            "absolute aicx-store path must NOT leak into rendered context: {md}"
        );
        assert!(
            !md.contains("/tmp/aicx/store/"),
            "absolute tmp aicx-store path must NOT leak either: {md}"
        );
        assert!(
            md.contains("chunk:"),
            "opaque chunk reference must replace raw path in memory section"
        );
        assert!(
            md.contains("intentionally redacted"),
            "Source-chunk-pointers footer must explain the redaction explicitly"
        );
    }

    #[test]
    fn chunk_ref_is_stable_short_hash() {
        let a = chunk_ref("/home/op/.aicx/store/proj/2026/01/01/sess.md");
        let b = chunk_ref("/home/op/.aicx/store/proj/2026/01/01/sess.md");
        assert_eq!(a, b, "same path must hash to same chunk ref");
        assert!(
            a.starts_with("chunk:") && a.len() == "chunk:".len() + 8,
            "chunk ref must be 8-char hex prefix: {a}"
        );
        assert_eq!(chunk_ref(""), "chunk:none");
        let c = chunk_ref("/home/op/.aicx/store/proj/2026/01/02/sess.md");
        assert_ne!(a, c, "different paths must hash to different refs");
    }

    #[test]
    fn pill_tldr_is_first_section_after_header() {
        let pack = fixture_pack();
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::Disabled);
        let md = render_pill(input);
        let first_h2 = md.find("## ").expect("at least one h2 expected");
        let tldr = md
            .find("## TL;DR — read this first")
            .expect("TL;DR present");
        assert_eq!(
            first_h2, tldr,
            "TL;DR must be the FIRST h2 in the pill output"
        );
    }

    #[test]
    fn pill_header_includes_repo_identity_and_timestamp() {
        let pack = fixture_pack();
        let scope = fixture_scope();
        let input = input_with(&pack, &scope, AicxRenderStatus::Disabled);
        let md = render_pill(input);
        assert!(md.starts_with("# Loctree Context · "));
        assert!(md.contains("loctree-suite"));
        assert!(md.contains("2026-04-28T16:58:02Z"));
        assert!(md.contains("231 files"));
        assert!(md.contains("421 import edges"));
    }

    #[test]
    fn pill_missing_snapshot_never_reports_zero_files_fresh() {
        let mut pack = fixture_pack();
        pack.risk.cache_scope = RiskCacheScope::MissingSnapshot;
        pack.risk.cache_scope_authority = AuthorityLabel::RepoVerified;
        pack.risk.snapshot_health = Some("missing_snapshot".to_string());
        pack.risk.stale_snapshot = false;
        pack.risk.hotspots.clear();
        pack.risk.high_fan_in.clear();

        let mut scope = fixture_scope();
        scope.branch_hint = None;
        scope.top_hubs.clear();
        scope.recent_files.clear();
        let input = PillInput {
            pack: &pack,
            scope: &scope,
            project_name: "loctree-suite".to_string(),
            timestamp_iso: "2026-07-06T09:00:00Z".to_string(),
            snapshot_files: 0,
            snapshot_edges: 0,
            elapsed: std::time::Duration::from_millis(12),
            aicx_status: AicxRenderStatus::Disabled,
        };

        let md = render_pill(input);
        assert!(
            md.contains("no snapshot - run `loct scan`"),
            "missing snapshot must be explicit: {md}"
        );
        assert!(
            !md.contains("0 files") && !md.contains("0 import edges"),
            "missing snapshot must not masquerade as zero metrics: {md}"
        );
        assert!(
            !md.contains("snapshot fresh"),
            "missing snapshot must not render as fresh: {md}"
        );
    }
}
