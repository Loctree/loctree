//! CLI handler and renderers for `loct context`.
//!
//! ContextPack schema and composition live in [`crate::pack`]. This module owns
//! CLI argument handling, stdout rendering, and materialized artifact side
//! effects only.
//!
//! Cut 11 — the brand-defining pill is the default no-flag output. Pill
//! rendering, auto-scope discovery, budget enforcement and measurement-
//! honesty enums live in dedicated submodules:
//!
//! - [`pill`]    — markdown renderer (TL;DR-first, ranked, capped at 1000 lines)
//! - [`scope`]   — auto-scope discovery (zero-flag UX, deterministic)
//! - [`budget`]  — per-section budgets + truncation tail
//! - [`honesty`] — `DeadExportsStatus` / `MeasurementStatus` (no silent zeros)

pub mod atlas;
pub mod budget;
pub mod honesty;
pub mod pill;
pub mod scope;

use std::collections::{HashMap, HashSet};
use std::io::{self, IsTerminal};
use std::path::Path;
use std::time::Instant;

use crate::aicx::is_aicx_available;
use crate::analyzer::root_scan::{BarrelInfo, RootContext, ScanResults};
use crate::args::ParsedArgs;
use crate::cli::command::{ContextOptions, GlobalOptions};
use crate::context_render::current_iso_timestamp;
use crate::fs_utils::StaticAssetName;
use crate::pack::{
    ContextLoadError, ContextPack, aicx_project_bucket, compose_context_pack_with_global,
    context_snapshot_root, format_context_pack_markdown, missing_snapshot_context_pack,
};
use crate::snapshot::Snapshot;
use crate::types::{Options, OutputMode};

use super::super::DispatchResult;

pub fn run(opts: &ContextOptions, global: &GlobalOptions) -> DispatchResult {
    let render_start = Instant::now();
    let human_status = context_human_status_enabled(opts, global);
    let snapshot_root = context_snapshot_root(opts);

    if human_status {
        eprintln!("loct context");
        eprintln!("▸ composing ContextPack through pack.rs canon...");
    }

    let pack = match compose_context_pack_with_global(opts, &snapshot_root, global) {
        Ok(pack) => pack,
        Err(ContextLoadError::NoSnapshotNoScanMode { root }) => {
            eprintln!(
                "[loct][context] no snapshot found in {} and --no-scan in effect; using empty ContextPack",
                root.display()
            );
            missing_snapshot_context_pack(opts, &snapshot_root)
        }
        Err(ContextLoadError::StaleInCiMode { current, snapshot }) => {
            eprintln!(
                "[loct][context] snapshot is stale (current git {current} vs snapshot {snapshot}) and --fail-stale in effect"
            );
            return DispatchResult::Exit(3);
        }
        Err(ContextLoadError::Scope(err)) => {
            eprintln!("{err}");
            return DispatchResult::Exit(2);
        }
        Err(err) => {
            eprintln!("[loct][context] failed to compose ContextPack: {err}");
            return DispatchResult::Exit(1);
        }
    };

    let snapshot = Snapshot::load(&snapshot_root).ok();
    let snapshot_files = snapshot
        .as_ref()
        .map(|snapshot| snapshot.files.len())
        .unwrap_or(0);
    let snapshot_edges = snapshot
        .as_ref()
        .map(|snapshot| snapshot.edges.len())
        .unwrap_or(0);
    let auto_scope = snapshot
        .as_ref()
        .map(|snapshot| scope::discover(snapshot, &snapshot_root))
        .unwrap_or_default();

    if human_status {
        if snapshot.is_some() {
            eprintln!("✓ snapshot loaded: {snapshot_files} files, {snapshot_edges} edges");
        } else {
            eprintln!("⚠ no snapshot loaded for render metadata/artifacts");
        }
        if !auto_scope.changed_files.is_empty() {
            eprintln!(
                "✓ auto-scope detected {} changed file(s)",
                auto_scope.changed_files.len()
            );
        }
    }

    let write_human_html = should_write_context_html_artifacts(opts, human_status);
    materialize_context_artifacts(
        &pack,
        &snapshot_root,
        snapshot.as_ref(),
        human_status,
        write_human_html,
    );

    if opts.full && opts.markdown && !opts.json && !global.json {
        print!("{}", render_context_pack_markdown(&pack));
        if pack.risk.snapshot_health.as_deref() == Some("missing_snapshot") {
            println!("\n<!-- snapshot_health: missing_snapshot -->");
        }
        return DispatchResult::Exit(0);
    }

    if opts.full || opts.json || global.json {
        return emit_full_json(&pack);
    }

    let metadata = pill::PillRenderMetadata {
        project_name: aicx_project_bucket(opts),
        timestamp_iso: current_iso_timestamp(),
        snapshot_files,
        snapshot_edges,
        elapsed_start: render_start,
        aicx_status: pill_aicx_status(opts, &pack),
    };
    let pill_input = pill::build_input(&pack, &auto_scope, metadata);
    print!("{}", pill::render_pill(pill_input));
    if pack.risk.snapshot_health.as_deref() == Some("missing_snapshot") {
        println!("\n<!-- snapshot_health: missing_snapshot -->");
    }
    DispatchResult::Exit(0)
}

fn context_human_status_enabled(opts: &ContextOptions, global: &GlobalOptions) -> bool {
    std::io::stderr().is_terminal() && !opts.json && !global.json
}

fn should_write_context_html_artifacts(opts: &ContextOptions, human_status: bool) -> bool {
    if !human_status {
        return false;
    }

    opts.full || context_request_is_bare(opts)
}

fn context_request_is_bare(opts: &ContextOptions) -> bool {
    opts.file.is_none() && !opts.changed && opts.scopes.is_empty() && opts.task.is_none()
}

fn emit_full_json(pack: &ContextPack) -> DispatchResult {
    match serde_json::to_string_pretty(pack) {
        Ok(json) => {
            println!("{json}");
            DispatchResult::Exit(0)
        }
        Err(err) => {
            eprintln!("[loct][context] failed to serialize ContextPack: {err}");
            DispatchResult::Exit(1)
        }
    }
}

fn materialize_context_artifacts(
    pack: &ContextPack,
    snapshot_root: &Path,
    snapshot: Option<&Snapshot>,
    human_status: bool,
    write_human_html: bool,
) {
    if human_status {
        eprintln!("▸ materializing Context Atlas cards…");
    }

    match atlas::materialize_context_atlas(pack, snapshot_root, None) {
        Ok(manifest) if human_status => {
            eprintln!("✓ atlas ready: {} cards", manifest.cards.len());
        }
        Ok(_) => {}
        Err(err) => {
            eprintln!("[loct][context] failed to write Context Atlas: {err}");
        }
    }

    if !write_human_html {
        return;
    }

    let Some(snapshot) = snapshot else {
        return;
    };

    if human_status {
        eprintln!("▸ rendering full HTML report…");
    }

    match write_context_html_artifacts(snapshot_root, snapshot, !human_status) {
        Ok(paths) if human_status && !paths.is_empty() => {
            eprintln!(
                "✓ human artifacts ready under {}:",
                Snapshot::artifacts_dir(snapshot_root).display()
            );
            for path in paths {
                eprintln!("  - {path}");
            }
            eprintln!();
        }
        Ok(_) => {}
        Err(err) => {
            eprintln!("[loct][context] failed to write human HTML report: {err}");
        }
    }
}

fn write_context_html_artifacts(
    snapshot_root: &Path,
    snapshot: &Snapshot,
    quiet: bool,
) -> io::Result<Vec<String>> {
    let mut loc_map = HashMap::new();
    let mut languages = HashSet::new();
    let mut dynamic_summary = Vec::new();
    let mut global_fe_commands = HashMap::new();
    let mut global_be_commands = HashMap::new();

    for file in &snapshot.files {
        loc_map.insert(file.path.clone(), file.loc);
        if !file.language.is_empty() {
            languages.insert(file.language.clone());
        }
        if !file.dynamic_imports.is_empty() {
            dynamic_summary.push((file.path.clone(), file.dynamic_imports.clone()));
        }
        for call in &file.command_calls {
            global_fe_commands
                .entry(call.name.clone())
                .or_insert_with(Vec::new)
                .push((file.path.clone(), call.line, String::new()));
        }
        for handler in &file.command_handlers {
            global_be_commands
                .entry(handler.name.clone())
                .or_insert_with(Vec::new)
                .push((file.path.clone(), handler.line, handler.name.clone()));
        }
    }

    for bridge in &snapshot.command_bridges {
        for (file, line) in &bridge.frontend_calls {
            global_fe_commands
                .entry(bridge.name.clone())
                .or_insert_with(Vec::new)
                .push((file.clone(), *line, String::new()));
        }
        if let Some((file, line)) = &bridge.backend_handler {
            global_be_commands
                .entry(bridge.name.clone())
                .or_insert_with(Vec::new)
                .push((file.clone(), *line, bridge.name.clone()));
        }
    }

    let root_context = RootContext {
        root_path: snapshot_root.to_path_buf(),
        options: Options::default(),
        analyses: snapshot.files.clone(),
        export_index: snapshot.export_index.clone(),
        dynamic_summary,
        cascades: Vec::new(),
        filtered_ranked: Vec::new(),
        graph_edges: snapshot
            .edges
            .iter()
            .map(|edge| (edge.from.clone(), edge.to.clone(), edge.label.clone()))
            .collect(),
        loc_map,
        languages,
        tsconfig_summary: serde_json::json!({}),
        calls_with_generics: Vec::new(),
        renamed_handlers: Vec::new(),
        barrels: snapshot
            .barrels
            .iter()
            .map(|barrel| BarrelInfo {
                path: barrel.path.clone(),
                module_id: barrel.module_id.clone(),
                reexport_count: barrel.reexport_count,
                target_count: barrel.targets.len(),
                mixed: false,
                targets: barrel.targets.clone(),
            })
            .collect(),
    };

    let scan_results = ScanResults {
        contexts: vec![root_context],
        global_fe_commands,
        global_be_commands,
        global_fe_payloads: HashMap::new(),
        global_be_payloads: HashMap::new(),
        global_analyses: snapshot.files.clone(),
        ts_resolver_config: None,
        py_roots: Vec::new(),
    };

    let parsed = ParsedArgs {
        graph: true,
        output: OutputMode::Json,
        summary: true,
        root_list: vec![snapshot_root.to_path_buf()],
        verbose: quiet,
        ..ParsedArgs::default()
    };

    let roots = vec![snapshot_root.to_path_buf()];
    let mut created = crate::snapshot::write_auto_artifacts(
        snapshot_root,
        &roots,
        &scan_results,
        &parsed,
        Some(&snapshot.metadata),
        None,
    )?;
    if let Some(local_report) = mirror_context_report_to_local_atlas_dir(snapshot_root)? {
        created.push(local_report);
    }
    Ok(created)
}

fn mirror_context_report_to_local_atlas_dir(snapshot_root: &Path) -> io::Result<Option<String>> {
    let artifacts_dir = Snapshot::artifacts_dir(snapshot_root);
    let local_dir = snapshot_root.join(".loctree");
    let same_dir = artifacts_dir.canonicalize().ok() == local_dir.canonicalize().ok();
    if same_dir {
        return Ok(None);
    }

    let source_report = artifacts_dir.join("report.html");
    if !source_report.exists() {
        return Ok(None);
    }

    std::fs::create_dir_all(&local_dir)?;

    // SaaS-safety: each asset is mirrored through `mirror_static_asset`,
    // which takes a `&'static str` and only joins it onto trusted roots.
    // Listing assets as separate literal arguments (rather than a `&[&str]`
    // iterated by reference) keeps Semgrep's taint analysis able to see
    // that the filename component is a compile-time constant — no need
    // for `nosemgrep` suppressions.
    mirror_static_asset(
        &artifacts_dir,
        &local_dir,
        StaticAssetName::new("report.html"),
    )?;
    mirror_static_asset(
        &artifacts_dir,
        &local_dir,
        StaticAssetName::new("loctree-cytoscape.min.js"),
    )?;
    mirror_static_asset(
        &artifacts_dir,
        &local_dir,
        StaticAssetName::new("loctree-dagre.min.js"),
    )?;
    mirror_static_asset(
        &artifacts_dir,
        &local_dir,
        StaticAssetName::new("loctree-cytoscape-dagre.js"),
    )?;
    mirror_static_asset(
        &artifacts_dir,
        &local_dir,
        StaticAssetName::new("loctree-layout-base.js"),
    )?;
    mirror_static_asset(
        &artifacts_dir,
        &local_dir,
        StaticAssetName::new("loctree-cose-base.js"),
    )?;
    mirror_static_asset(
        &artifacts_dir,
        &local_dir,
        StaticAssetName::new("loctree-cytoscape-cose-bilkent.js"),
    )?;

    Ok(Some(".loctree/report.html".to_string()))
}

/// Copy one report asset from the (potentially cache-scoped) artifacts
/// directory into the per-repo `.loctree/` mirror.
///
/// `name` is a [`StaticAssetName`] by design: callers must pass a
/// compile-time string literal so Semgrep's `tainted-path` analysis can
/// prove the file name is not derived from operator/MCP input. The actual
/// copy goes through [`crate::fs_utils::copy_static_asset_within`], which
/// routes the source read through a `SanitizedPath` anchored at
/// `src_dir` so the boundary guard sits at the same call site as the
/// `fs::copy` sink.
fn mirror_static_asset(src_dir: &Path, dst_dir: &Path, name: StaticAssetName) -> io::Result<()> {
    crate::fs_utils::copy_static_asset_within(src_dir, dst_dir, name)
}

fn pill_aicx_status(opts: &ContextOptions, pack: &ContextPack) -> pill::AicxRenderStatus {
    if !context_aicx_enabled(opts) {
        return pill::AicxRenderStatus::Disabled;
    }
    // The composer already recorded the overlay outcome — read it instead
    // of re-probing `aicx --version` (an extra subprocess per render that
    // could also disagree with what the composer actually saw).
    if let Some(diagnostic) = &pack.memory.diagnostic {
        use crate::pack::MemorySkipReason;
        return match diagnostic.skip_reason {
            MemorySkipReason::Ok => pill::AicxRenderStatus::EnabledWithRows,
            MemorySkipReason::DisabledByNoAicx => pill::AicxRenderStatus::Disabled,
            // The overlay was wanted here (context_aicx_enabled passed), yet
            // the composer reports opt-out — that means the budgeted client
            // was never built, i.e. no AICX transport exists on this host.
            MemorySkipReason::DisabledOptOut | MemorySkipReason::AicxUnreachable => {
                pill::AicxRenderStatus::Unavailable
            }
            MemorySkipReason::TimedOut => pill::AicxRenderStatus::SkippedTimeout,
            MemorySkipReason::NamespaceEmpty | MemorySkipReason::NoTokenOverlap => {
                pill::AicxRenderStatus::EnabledNoRows
            }
        };
    }
    // Legacy packs without a diagnostic (e.g. deserialized older JSON).
    if pack.memory.entries.is_empty() && !is_aicx_available() {
        pill::AicxRenderStatus::Unavailable
    } else if pack.memory.entries.is_empty() {
        pill::AicxRenderStatus::EnabledNoRows
    } else {
        pill::AicxRenderStatus::EnabledWithRows
    }
}

fn context_aicx_enabled(opts: &ContextOptions) -> bool {
    !opts.no_aicx && (opts.with_aicx || context_request_is_bare(opts))
}

/// Render a composed ContextPack as full Markdown, without invoking CLI dispatch.
pub fn render_context_pack_markdown(pack: &ContextPack) -> String {
    format_context_pack_markdown(pack)
}
