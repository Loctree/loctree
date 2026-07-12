//! Analysis-related command handlers
//!
//! Handles: dead, cycles, commands, events, pipelines, insights, manifests, routes, zombie

use super::super::super::command::{
    AuditOptions, CommandsOptions, CyclesOptions, DeadOptions, DoctorOptions, EventsOptions,
    FocusOptions, FollowOptions, HealthOptions, HotspotsOptions, InsightsOptions, LayoutmapOptions,
    ManifestsOptions, PipelinesOptions, PlanOptions, RoutesOptions, TraceOptions, ZombieOptions,
};
use super::super::{
    DispatchResult, GlobalOptions, load_or_create_snapshot, load_or_create_snapshot_for_roots,
    with_command_snapshot_cache,
};
use super::deprecation::warn_deprecated;
use crate::progress::Spinner;

/// Handle the follow command - unified wrapper over the existing analysis scopes.
pub fn handle_follow_command(opts: &FollowOptions, global: &GlobalOptions) -> DispatchResult {
    use std::path::PathBuf;

    let roots = if opts.roots.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        opts.roots.clone()
    };
    let first_root = roots.first().cloned().unwrap_or_else(|| PathBuf::from("."));

    match opts.scope.as_str() {
        "dead" => handle_dead_command(
            &DeadOptions {
                roots,
                top: opts.limit,
                ..Default::default()
            },
            global,
        ),
        "cycles" => handle_cycles_command(
            &CyclesOptions {
                roots,
                ..Default::default()
            },
            global,
        ),
        "twins" => handle_twins_follow(&roots, global),
        "hotspots" => handle_hotspots_command(
            &HotspotsOptions {
                root: Some(first_root),
                limit: opts.limit,
                ..Default::default()
            },
            global,
        ),
        "trace" => {
            let Some(handler) = opts.handler.clone() else {
                eprintln!("[loct][error] follow trace requires --handler <name>");
                return DispatchResult::Exit(1);
            };
            handle_trace_command(&TraceOptions { handler, roots }, global)
        }
        "commands" => handle_commands_command(
            &CommandsOptions {
                roots,
                limit: opts.limit,
                ..Default::default()
            },
            global,
        ),
        "events" => handle_events_command(
            &EventsOptions {
                roots,
                ..Default::default()
            },
            global,
        ),
        "pipelines" => handle_pipelines_command(&PipelinesOptions { roots }, global),
        "all" => with_command_snapshot_cache(|| {
            handle_follow_all(&roots, opts.limit.unwrap_or(3), global)
        }),
        other => {
            eprintln!(
                "[loct][error] unknown follow scope '{}'. Valid: all, dead, cycles, twins, hotspots, trace, commands, events, pipelines",
                other
            );
            DispatchResult::Exit(1)
        }
    }
}

fn handle_twins_follow(roots: &[std::path::PathBuf], global: &GlobalOptions) -> DispatchResult {
    let path = roots.first().cloned();
    super::ai::handle_twins_command(
        &super::super::super::command::TwinsOptions {
            path,
            ..Default::default()
        },
        global,
    )
}

/// Build the aggregated `follow all --json` report: ONE JSON object carrying
/// machine-readable counts across the dead / cycles / twins / hotspots scopes,
/// plus the top `limit` items for the list-shaped scopes. Per-scope full detail
/// stays in `loct follow <scope> --json`. Dead count uses the canonical dead
/// pipeline (`compute_dead_truth`), so it agrees with repo-view / health /
/// twins — the count never forks.
fn build_follow_all_report(
    snapshot: &crate::snapshot::Snapshot,
    limit: usize,
) -> serde_json::Value {
    use crate::analyzer::cycles::find_cycles_classified_with_lazy;
    use crate::analyzer::dead_parrots::compute_dead_truth;
    use crate::analyzer::twins::{detect_exact_twins_with_frameworks, find_dead_parrots};
    use std::collections::HashMap;

    // Dead exports (canonical pipeline — same number as repo-view/health/twins).
    let dead = compute_dead_truth(snapshot).dead;
    let dead_top: Vec<_> = dead.iter().take(limit).collect();

    // Import cycles.
    let edges: Vec<(String, String, String)> = snapshot
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
        .collect();
    let (cycles, _lazy) = find_cycles_classified_with_lazy(&edges);

    // Structural twin signals.
    let exact_twins = detect_exact_twins_with_frameworks(&snapshot.files, false, None);
    let dead_parrots = find_dead_parrots(&snapshot.files, false, false).dead_parrots;

    // Import hotspots (in/out degree), mirroring `handle_hotspots_command`.
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut out_degree: HashMap<&str, usize> = HashMap::new();
    for file in &snapshot.files {
        in_degree.entry(file.path.as_str()).or_insert(0);
        out_degree.entry(file.path.as_str()).or_insert(0);
    }
    for edge in &snapshot.edges {
        *in_degree.entry(edge.to.as_str()).or_insert(0) += 1;
        *out_degree.entry(edge.from.as_str()).or_insert(0) += 1;
    }
    let mut hotspots: Vec<(&str, usize, usize)> = in_degree
        .iter()
        .map(|(path, &in_deg)| (*path, in_deg, out_degree.get(path).copied().unwrap_or(0)))
        .filter(|(_, in_deg, _)| *in_deg > 0)
        .collect();
    hotspots.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let hotspot_count = hotspots.len();
    let hotspot_top: Vec<serde_json::Value> = hotspots
        .iter()
        .take(limit)
        .map(|(path, in_deg, out_deg)| {
            let category = match *in_deg {
                n if n >= 10 => "CORE",
                n if n >= 3 => "SHARED",
                _ => "PERIPHERAL",
            };
            serde_json::json!({
                "path": path,
                "in_degree": in_deg,
                "out_degree": out_deg,
                "category": category,
            })
        })
        .collect();

    serde_json::json!({
        "scope": "all",
        "limit": limit,
        "note": "Aggregated machine-readable counts across follow scopes. Per-scope full detail: `loct follow <scope> --json`.",
        "dead": { "count": dead.len(), "top": dead_top },
        "cycles": { "count": cycles.len(), "cycles": cycles },
        "twins": { "exact_twins": exact_twins.len(), "dead_parrots": dead_parrots.len() },
        "hotspots": { "count": hotspot_count, "top": hotspot_top },
    })
}

fn handle_follow_all(
    roots: &[std::path::PathBuf],
    limit: usize,
    global: &GlobalOptions,
) -> DispatchResult {
    if global.json {
        // Single aggregated JSON object (the `follow all --json` contract that
        // `--help` advertised but used to refuse). Per-scope `--json` keeps its
        // own full-detail output.
        let snapshot = match load_or_create_snapshot_for_roots(roots, global) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[loct][error] {}", e);
                return DispatchResult::Exit(1);
            }
        };
        let report = build_follow_all_report(&snapshot, limit);
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize follow report: {}", e);
                return DispatchResult::Exit(1);
            }
        }
        return DispatchResult::Exit(0);
    }

    println!("=== follow: dead ===");
    let dead_result = handle_dead_command(
        &DeadOptions {
            roots: roots.to_vec(),
            top: Some(limit),
            ..Default::default()
        },
        global,
    );
    if !dispatch_ok(&dead_result) {
        return dead_result;
    }

    println!("\n=== follow: cycles ===");
    let cycles_result = handle_cycles_command(
        &CyclesOptions {
            roots: roots.to_vec(),
            ..Default::default()
        },
        global,
    );
    if !dispatch_ok(&cycles_result) {
        return cycles_result;
    }

    println!("\n=== follow: twins ===");
    let twins_result = handle_twins_follow(roots, global);
    if !dispatch_ok(&twins_result) {
        return twins_result;
    }

    println!("\n=== follow: hotspots ===");
    let hotspots_result = handle_hotspots_command(
        &HotspotsOptions {
            root: roots.first().cloned(),
            limit: Some(limit),
            ..Default::default()
        },
        global,
    );
    if !dispatch_ok(&hotspots_result) {
        return hotspots_result;
    }

    DispatchResult::Exit(0)
}

fn dispatch_ok(result: &DispatchResult) -> bool {
    matches!(result, DispatchResult::Exit(0))
}

/// Handle the dead command - detect dead exports
pub fn handle_dead_command(opts: &DeadOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::analyzer::dead_parrots::{
        DeadFilterConfig, compute_dead_truth_with, print_dead_exports,
    };
    use std::path::Path;
    use std::path::PathBuf;

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Analyzing dead exports..."))
    } else {
        None
    };

    // Load snapshot (auto-scan if missing) using ALL provided roots.
    let roots: Vec<PathBuf> = if opts.roots.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        opts.roots.clone()
    };
    let root = roots.first().map(|p| p.as_path()).unwrap_or(Path::new("."));

    let snapshot = match load_or_create_snapshot_for_roots(&roots, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    // Determine confidence level
    let high_confidence = opts.confidence.as_deref() == Some("high");
    let dead_ok_globs = crate::fs_utils::load_loctignore_dead_ok_globs(root);

    // Canonical dead pipeline: same source as `loct twins`, `loct findings`
    // and repo-view — semantic suppression + literal/symbol-graph cross-check
    // + entry-point fence run on every surface, so the count never forks.
    let dead_truth = compute_dead_truth_with(
        &snapshot,
        DeadFilterConfig {
            include_tests: opts.with_tests,
            include_helpers: opts.with_helpers,
            library_mode: global.library_mode,
            example_globs: Vec::new(),
            python_library_mode: global.python_library,
            include_ambient: opts.with_ambient,
            include_dynamic: opts.with_dynamic,
            dead_ok_globs,
        },
        high_confidence,
    );
    let dead_exports = dead_truth.dead;

    if let Some(s) = spinner {
        s.finish_success(&format!("Found {} dead export(s)", dead_exports.len()));
    }

    // Output results
    let output_mode = if global.json {
        crate::types::OutputMode::Json
    } else {
        crate::types::OutputMode::Human
    };

    print_dead_exports(
        &dead_exports,
        output_mode,
        high_confidence,
        if opts.full {
            dead_exports.len()
        } else {
            opts.top.unwrap_or(20)
        },
    );

    DispatchResult::Exit(0)
}

/// Handle the pipelines command - pipeline summary (events/commands/risks)
pub fn handle_pipelines_command(opts: &PipelinesOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::analyzer::pipelines::build_pipeline_summary;
    use crate::analyzer::root_scan::scan_results_from_snapshot;
    use std::path::Path;

    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Building pipeline summary..."))
    } else {
        None
    };

    let root = opts
        .roots
        .first()
        .map(|p| p.as_path())
        .unwrap_or(Path::new("."));

    let snapshot = match load_or_create_snapshot(root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    let scan_results = scan_results_from_snapshot(&snapshot);
    let focus = None;
    let exclude = None;
    let pipeline_summary = build_pipeline_summary(
        &scan_results.global_analyses,
        &focus,
        &exclude,
        &scan_results.global_fe_commands,
        &scan_results.global_be_commands,
        &scan_results.global_fe_payloads,
        &scan_results.global_be_payloads,
    );

    if let Some(s) = spinner {
        s.finish_success("Pipeline summary ready");
    }

    if global.json {
        match serde_json::to_string_pretty(&pipeline_summary) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize pipeline summary: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else {
        let events = &pipeline_summary["events"];
        let stats = &events["stats"];
        let ghost = stats["ghostCount"].as_u64().unwrap_or(0);
        let orphan = stats["orphanCount"].as_u64().unwrap_or(0);
        let matched = stats["matched"].as_u64().unwrap_or(0);
        let emitted = stats["distinctEmitted"].as_u64().unwrap_or(0);
        let listened = stats["distinctListened"].as_u64().unwrap_or(0);

        let cmd_stats = &pipeline_summary["commands"]["stats"];
        let total_cmds = cmd_stats["total"].as_u64().unwrap_or(0);
        let calls = cmd_stats["withCalls"].as_u64().unwrap_or(0);
        let handlers = cmd_stats["withHandlers"].as_u64().unwrap_or(0);

        let risks = pipeline_summary["risks"]
            .as_array()
            .map(|v| v.len())
            .unwrap_or(0);

        println!("Pipeline Summary:");
        println!(
            "  Events: {} emitted, {} listened, {} matched",
            emitted, listened, matched
        );
        println!("  Ghost emits: {}", ghost);
        println!("  Orphan listeners: {}", orphan);
        println!(
            "  Commands: {} total ({} FE calls, {} handlers)",
            total_cmds, calls, handlers
        );
        println!("  Risks: {}", risks);
    }

    DispatchResult::Exit(0)
}

/// Handle the insights command - AI insights summary
pub fn handle_insights_command(opts: &InsightsOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::analyzer::coverage::compute_command_gaps_with_confidence;
    use crate::analyzer::insights::collect_ai_insights;
    use crate::analyzer::root_scan::scan_results_from_snapshot;
    use std::path::Path;

    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Collecting insights..."))
    } else {
        None
    };

    let root = opts
        .roots
        .first()
        .map(|p| p.as_path())
        .unwrap_or(Path::new("."));

    let snapshot = match load_or_create_snapshot(root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    let scan_results = scan_results_from_snapshot(&snapshot);
    let mut dups = Vec::new();
    let mut cascades = Vec::new();
    for ctx in &scan_results.contexts {
        dups.extend(ctx.filtered_ranked.clone());
        cascades.extend(ctx.cascades.clone());
    }

    let focus = None;
    let exclude = None;
    let (missing_handlers, unused_handlers) = compute_command_gaps_with_confidence(
        &scan_results.global_fe_commands,
        &scan_results.global_be_commands,
        &focus,
        &exclude,
        &scan_results.global_analyses,
    );

    let insights = collect_ai_insights(
        &scan_results.global_analyses,
        &dups,
        &cascades,
        &missing_handlers,
        &unused_handlers,
    );

    if let Some(s) = spinner {
        s.finish_success(&format!("Found {} insight(s)", insights.len()));
    }

    if global.json {
        match serde_json::to_string_pretty(&insights) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize insights: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else if insights.is_empty() {
        println!("[loct][insights] No insights found");
    } else {
        println!("Insights:");
        for insight in &insights {
            println!(
                "  - [{}] {}: {}",
                insight.severity.to_uppercase(),
                insight.title,
                insight.message
            );
        }
    }

    DispatchResult::Exit(0)
}

/// Handle the manifests command - show manifest summaries
pub fn handle_manifests_command(opts: &ManifestsOptions, global: &GlobalOptions) -> DispatchResult {
    use std::path::Path;

    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Loading manifest summaries..."))
    } else {
        None
    };

    let root = opts
        .roots
        .first()
        .map(|p| p.as_path())
        .unwrap_or(Path::new("."));

    let snapshot = match load_or_create_snapshot(root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    if let Some(s) = spinner {
        s.finish_success("Manifest summaries ready");
    }

    let manifests = &snapshot.metadata.manifest_summary;

    if global.json {
        match serde_json::to_string_pretty(&manifests) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!(
                    "[loct][error] Failed to serialize manifest summaries: {}",
                    e
                );
                return DispatchResult::Exit(1);
            }
        }
    } else if manifests.is_empty() {
        println!("[loct][manifests] No manifest summaries found");
    } else {
        println!("Manifest summaries:");
        for manifest in manifests {
            println!("  Root: {}", manifest.root);
            if let Some(pkg) = &manifest.package_json {
                println!(
                    "    package.json: {}",
                    pkg.name.clone().unwrap_or_else(|| "<unnamed>".to_string())
                );
            }
            if let Some(cargo) = &manifest.cargo_toml {
                println!(
                    "    Cargo.toml: {}",
                    cargo
                        .package_name
                        .clone()
                        .unwrap_or_else(|| "<unnamed>".to_string())
                );
            }
            if let Some(py) = &manifest.pyproject_toml {
                let name = py
                    .project_name
                    .clone()
                    .or_else(|| py.poetry_name.clone())
                    .unwrap_or_else(|| "<unnamed>".to_string());
                println!("    pyproject.toml: {}", name);
            }
        }
    }

    DispatchResult::Exit(0)
}

/// Handle the cycles command - detect circular imports
pub fn handle_cycles_command(opts: &CyclesOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::analyzer::cycles::{
        CycleCompilability, find_cycles_classified_with_lazy, print_cycles_classified,
        print_cycles_classified_legacy,
    };
    use std::path::PathBuf;

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Detecting circular imports..."))
    } else {
        None
    };

    // Load snapshot (auto-scan if missing) using ALL provided roots.
    let roots: Vec<PathBuf> = if opts.roots.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        opts.roots.clone()
    };

    let snapshot = match load_or_create_snapshot_for_roots(&roots, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    // Extract edges from snapshot
    let edges: Vec<(String, String, String)> = snapshot
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
        .collect();

    // Find and classify cycles
    let (mut classified_cycles, classified_lazy_cycles) = find_cycles_classified_with_lazy(&edges);

    // Filter to breaking-only if requested
    if opts.breaking_only {
        classified_cycles.retain(|c| c.compilability == CycleCompilability::Breaking);
    }

    // Artifact fence (default-on, human output): cycles living entirely in
    // fixtures/vendored/generated files are intentional test inputs, not
    // production regressions. They move to their own "Fixture cycles"
    // section instead of leading the main result. JSON keeps the full set —
    // each cycle already carries its `fixture` provenance flag.
    let mut fixture_cycles: Vec<crate::analyzer::cycles::ClassifiedCycle> = Vec::new();
    if !opts.include_artifacts && !global.json {
        use crate::analyzer::classify::artifact_class;
        let (fenced, product): (Vec<_>, Vec<_>) = classified_cycles.into_iter().partition(|c| {
            c.fixture
                || (!c.nodes.is_empty()
                    && c.nodes
                        .iter()
                        .all(|node| artifact_class(node, None).is_artifact()))
        });
        classified_cycles = product;
        fixture_cycles = fenced;
    }

    // Count by compilability for spinner message
    let bidirectional_count = classified_cycles
        .iter()
        .filter(|c| c.compilability == CycleCompilability::Breaking)
        .count();
    let structural_count = classified_cycles
        .iter()
        .filter(|c| c.compilability == CycleCompilability::Structural)
        .count();
    let diamond_count = classified_cycles
        .iter()
        .filter(|c| c.compilability == CycleCompilability::DiamondDependency)
        .count();

    if let Some(s) = spinner {
        if opts.breaking_only {
            s.finish_success(&format!(
                "Found {} high-risk cycle(s) (filtered from {} total)",
                bidirectional_count,
                bidirectional_count + structural_count + diamond_count
            ));
        } else {
            s.finish_success(&format!(
                "Found {} cycle(s) ({} breaking, {} structural, {} diamond)",
                classified_cycles.len(),
                bidirectional_count,
                structural_count,
                diamond_count
            ));
        }
    }

    // Output results
    let json_output = global.json;

    if opts.legacy_format {
        print_cycles_classified_legacy(&classified_cycles, json_output);
    } else {
        print_cycles_classified(&classified_cycles, json_output);
    }

    // Fenced fixture/artifact cycles: separate section, never silent.
    if !fixture_cycles.is_empty() && !json_output {
        println!("\nFixture cycles ({}):", fixture_cycles.len());
        println!(
            "  (Cycles living entirely in fixtures/vendored/generated files — intentional test inputs, not production regressions. Use --include-artifacts to merge them into the main result.)"
        );
        if opts.legacy_format {
            print_cycles_classified_legacy(&fixture_cycles, false);
        } else {
            print_cycles_classified(&fixture_cycles, false);
        }
    }

    if !classified_lazy_cycles.is_empty() && !json_output && !opts.breaking_only {
        println!("\nLazy circular imports (info):");
        println!(
            "  Detected via imports inside functions/methods; usually safe but review if init order matters."
        );
        if opts.legacy_format {
            print_cycles_classified_legacy(&classified_lazy_cycles, false);
        } else {
            print_cycles_classified(&classified_lazy_cycles, false);
        }

        // Show the lazy edges that participated (sample)
        let lazy_edges: Vec<_> = edges
            .iter()
            .filter(|(_, _, kind)| kind.contains("lazy"))
            .take(5)
            .collect();
        if !lazy_edges.is_empty() {
            println!("  Lazy edges (sample):");
            for (from, to, kind) in lazy_edges {
                println!("    {} -> {} [{}]", from, to, kind);
            }
        }
    }

    // Exit code: 1 if there are high-risk cycles (for CI use)
    if bidirectional_count > 0 && opts.breaking_only {
        DispatchResult::Exit(1)
    } else {
        DispatchResult::Exit(0)
    }
}

/// Handle the trace command - Tauri/IPC handler tracing
///
/// Uses snapshot's command_bridges for fast lookup (auto-creates snapshot if missing)
pub fn handle_trace_command(opts: &TraceOptions, global: &GlobalOptions) -> DispatchResult {
    use std::path::Path;

    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new(&format!(
            "Tracing handler '{}'...",
            opts.handler
        )))
    } else {
        None
    };

    // Load snapshot (auto-scan if missing)
    let root = opts
        .roots
        .first()
        .map(|p| p.as_path())
        .unwrap_or(Path::new("."));

    let snapshot = match load_or_create_snapshot(root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    // Find matching command bridges (case-insensitive partial match)
    let handler_lower = opts.handler.to_lowercase();
    let matching_bridges: Vec<_> = snapshot
        .command_bridges
        .iter()
        .filter(|b| b.name.to_lowercase().contains(&handler_lower))
        .collect();

    if let Some(s) = spinner {
        s.finish_success("Trace complete");
    }

    // Output results
    if global.json {
        let json_output = serde_json::json!({
            "query": opts.handler,
            "matches": matching_bridges.iter().map(|b| {
                serde_json::json!({
                    "name": b.name,
                    "has_handler": b.has_handler,
                    "is_called": b.is_called,
                    "backend_handler": b.backend_handler,
                    "frontend_calls": b.frontend_calls,
                    "verdict": if !b.has_handler && b.is_called {
                        "MISSING"
                    } else if b.has_handler && !b.is_called {
                        "UNUSED"
                    } else if b.has_handler && b.is_called {
                        "OK"
                    } else {
                        "UNKNOWN"
                    }
                })
            }).collect::<Vec<_>>()
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json_output).unwrap_or_default()
        );
    } else if matching_bridges.is_empty() {
        println!("\nNo command bridges found matching '{}'", opts.handler);
        println!("\nAvailable commands (sample):");
        for bridge in snapshot.command_bridges.iter().take(10) {
            println!("  - {}", bridge.name);
        }
        if snapshot.command_bridges.len() > 10 {
            println!("  ... and {} more", snapshot.command_bridges.len() - 10);
        }
    } else {
        println!(
            "\nTrace for '{}' ({} match(es)):\n",
            opts.handler,
            matching_bridges.len()
        );
        for bridge in &matching_bridges {
            let verdict = if !bridge.has_handler && bridge.is_called {
                "MISSING"
            } else if bridge.has_handler && !bridge.is_called {
                "UNUSED"
            } else if bridge.has_handler && bridge.is_called {
                "OK"
            } else {
                "?"
            };
            println!("  [{}] {}", verdict, bridge.name);
            if let Some((ref file, line)) = bridge.backend_handler {
                println!("    Backend: {}:{}", file, line);
            } else {
                println!("    Backend: (not found)");
            }
            if bridge.frontend_calls.is_empty() {
                println!("    Frontend: (no calls)");
            } else {
                println!("    Frontend calls ({}):", bridge.frontend_calls.len());
                for (file, line) in bridge.frontend_calls.iter().take(5) {
                    println!("      {}:{}", file, line);
                }
                if bridge.frontend_calls.len() > 5 {
                    println!("      ... and {} more", bridge.frontend_calls.len() - 5);
                }
            }
            if !bridge.has_handler && bridge.is_called {
                println!(
                    "    [!] Frontend calls invoke('{}') but no backend handler found.",
                    bridge.name
                );
                println!(
                    "    Fix: Add #[tauri::command] pub async fn {}(...) in src-tauri/",
                    bridge.name
                );
            } else if bridge.has_handler && !bridge.is_called {
                println!("    [i] Handler defined but not called from frontend.");
                println!(
                    "    Consider: Remove if unused, or add invoke('{}') call.",
                    bridge.name
                );
            }
            println!();
        }
    }

    DispatchResult::Exit(0)
}

/// Handle the commands command - show Tauri command bridges
pub fn handle_commands_command(opts: &CommandsOptions, global: &GlobalOptions) -> DispatchResult {
    use std::path::Path;

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Analyzing Tauri commands..."))
    } else {
        None
    };

    // Load snapshot (auto-scan if missing)
    let root = opts
        .roots
        .first()
        .map(|p| p.as_path())
        .unwrap_or(Path::new("."));

    let snapshot = match load_or_create_snapshot(root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    // Filter command bridges based on options
    let mut bridges: Vec<_> = snapshot.command_bridges.clone();

    // Apply name filter
    if let Some(ref filter) = opts.name_filter {
        bridges.retain(|b| b.name.contains(filter));
    }

    // Apply missing-only filter
    if opts.missing_only {
        bridges.retain(|b| !b.has_handler && b.is_called);
    }

    // Apply unused-only filter
    if opts.unused_only {
        bridges.retain(|b| b.has_handler && !b.is_called);
    }

    // Apply limit if specified
    let total_before_limit = bridges.len();
    if let Some(limit) = opts.limit {
        bridges.truncate(limit);
    }

    if let Some(s) = spinner {
        if opts.limit.is_some() && total_before_limit > bridges.len() {
            s.finish_success(&format!(
                "Showing {} of {} command bridge(s)",
                bridges.len(),
                total_before_limit
            ));
        } else {
            s.finish_success(&format!("Found {} command bridge(s)", bridges.len()));
        }
    }

    // Output results
    if global.json {
        match serde_json::to_string_pretty(&bridges) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize command bridges: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else {
        // Human-readable output
        if bridges.is_empty() {
            println!("No command bridges found matching criteria");
        } else {
            println!("Tauri Command Bridges ({} total):\n", bridges.len());

            for bridge in &bridges {
                let status = if !bridge.has_handler && bridge.is_called {
                    "MISSING"
                } else if bridge.has_handler && !bridge.is_called {
                    "UNUSED"
                } else if bridge.has_handler && bridge.is_called {
                    "OK"
                } else {
                    "?"
                };

                println!("  [{}] {}", status, bridge.name);

                if !bridge.frontend_calls.is_empty() {
                    println!("    Frontend calls ({}):", bridge.frontend_calls.len());
                    for (file, line) in bridge.frontend_calls.iter().take(3) {
                        println!("      {}:{}", file, line);
                    }
                    if bridge.frontend_calls.len() > 3 {
                        println!("      ... and {} more", bridge.frontend_calls.len() - 3);
                    }
                }

                if let Some((ref backend_file, backend_line)) = bridge.backend_handler {
                    println!("    Backend: {}:{}", backend_file, backend_line);
                }

                if !bridge.has_handler && bridge.is_called {
                    println!(
                        "    [!] Why: Frontend calls invoke('{}') but no #[tauri::command] found in Rust.",
                        bridge.name
                    );
                    println!(
                        "    Impact: This command will fail at runtime with 'command not found' error."
                    );
                    if let Some((file, line)) = bridge.frontend_calls.first() {
                        println!("    First callsite: {}:{}", file, line);
                    }
                    println!(
                        "    Suggested fix: Add handler to src-tauri/src/commands/ and register in invoke_handler![]"
                    );
                    println!(
                        "    Stub: #[tauri::command] pub async fn {}(...) -> Result<(), String> {{ todo!() }}",
                        bridge.name
                    );
                } else if bridge.has_handler && !bridge.is_called {
                    println!(
                        "    [i] Why: #[tauri::command] defined but no invoke('{}') calls found in frontend.",
                        bridge.name
                    );
                    println!(
                        "    Consider: If intentionally unused, remove handler. If needed, add frontend call."
                    );
                }

                println!();
            }
        }
    }

    DispatchResult::Exit(0)
}

/// Handle the events command - analyze event flow
pub fn handle_events_command(opts: &EventsOptions, global: &GlobalOptions) -> DispatchResult {
    use std::path::Path;

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Analyzing event flow..."))
    } else {
        None
    };

    // Load snapshot (auto-scan if missing)
    let root = opts
        .roots
        .first()
        .map(|p| p.as_path())
        .unwrap_or(Path::new("."));

    let mut snapshot = match load_or_create_snapshot(root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    // Artifact fence (default-on): event bridges whose every emit/listen
    // location sits in vendored/minified/fixture/generated files are parser
    // noise (e.g. `.emit(` tokens inside cytoscape.min.js), not product
    // event flow. `--include-artifacts` restores them.
    let mut fence = crate::analyzer::classify::ArtifactFenceStats::default();
    if !opts.include_artifacts {
        use crate::analyzer::classify::artifact_class;
        snapshot.event_bridges.retain(|bridge| {
            let mut first_artifact = None;
            for path in bridge
                .emits
                .iter()
                .map(|(f, _, _)| f.as_str())
                .chain(bridge.listens.iter().map(|(f, _)| f.as_str()))
            {
                let class = artifact_class(path, None);
                if !class.is_artifact() {
                    return true;
                }
                first_artifact.get_or_insert(class);
            }
            match first_artifact {
                Some(class) => {
                    fence.record(class);
                    false
                }
                // No locations at all — keep, nothing to judge by.
                None => true,
            }
        });
    }

    let symbol_events = collect_symbol_runtime_events(&snapshot);

    if let Some(s) = spinner {
        s.finish_success(&format!(
            "Found {} event bridge(s)",
            snapshot.event_bridges.len() + symbol_events.len()
        ));
    }

    // Output results
    if global.json {
        let payload = if symbol_events.is_empty() {
            serde_json::json!(snapshot.event_bridges)
        } else {
            serde_json::json!({
                "event_bridges": snapshot.event_bridges,
                "symbol_events": symbol_events,
            })
        };
        match serde_json::to_string_pretty(&payload) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize events: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else {
        // Group events by pattern
        let fe_sync_events: Vec<_> = snapshot
            .event_bridges
            .iter()
            .filter(|e| e.is_fe_sync)
            .collect();
        let other_events: Vec<_> = snapshot
            .event_bridges
            .iter()
            .filter(|e| !e.is_fe_sync)
            .collect();

        // If --fe-sync flag, only show FE↔FE events
        if opts.fe_sync {
            if fe_sync_events.is_empty() {
                println!("No FE↔FE sync events found");
            } else {
                println!("FE↔FE Sync Events ({}):", fe_sync_events.len());
                println!("  (Window sync pattern: emit and listen both in frontend)\n");

                for event in &fe_sync_events {
                    println!("  Event: {}", event.name);

                    if event.same_file_sync {
                        println!("    Pattern: Same-file sync (emit+listen in same file)");
                    }

                    if !event.emits.is_empty() {
                        println!("    Emit locations ({}):", event.emits.len());
                        for (file, line, kind) in event.emits.iter().take(3) {
                            println!("      {}:{} [{}]", file, line, kind);
                        }
                        if event.emits.len() > 3 {
                            println!("      ... and {} more", event.emits.len() - 3);
                        }
                    }

                    if !event.listens.is_empty() {
                        println!("    Listen locations ({}):", event.listens.len());
                        for (file, line) in event.listens.iter().take(3) {
                            println!("      {}:{}", file, line);
                        }
                        if event.listens.len() > 3 {
                            println!("      ... and {} more", event.listens.len() - 3);
                        }
                    }

                    println!();
                }
            }
        } else {
            // Show all events, with FE↔FE sync clearly marked
            if snapshot.event_bridges.is_empty() {
                if symbol_events.is_empty() {
                    println!("No event bridges found");
                } else {
                    print_symbol_runtime_events(&symbol_events);
                }
            } else {
                println!("Event Bridges Analysis:\n");

                // Show FE↔FE sync events first if any exist
                if !fe_sync_events.is_empty() {
                    println!("FE↔FE Sync Events ({}):", fe_sync_events.len());
                    println!("  (Window sync: emit+listen both in frontend, not orphans)\n");

                    for event in &fe_sync_events {
                        println!(
                            "  {} {}",
                            event.name,
                            if event.same_file_sync {
                                "(same file)"
                            } else {
                                ""
                            }
                        );

                        if !event.emits.is_empty() {
                            println!(
                                "    Emit: {}",
                                event
                                    .emits
                                    .iter()
                                    .map(|(f, l, _)| format!("{}:{}", f, l))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                        }

                        if !event.listens.is_empty() {
                            println!(
                                "    Listen: {}",
                                event
                                    .listens
                                    .iter()
                                    .map(|(f, l)| format!("{}:{}", f, l))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                        }

                        println!();
                    }
                }

                // Show other events
                if !other_events.is_empty() {
                    if !fe_sync_events.is_empty() {
                        println!("Other Events ({}):\n", other_events.len());
                    } else {
                        println!("Found {} event bridge(s):\n", other_events.len());
                    }

                    for event in &other_events {
                        println!("  Event: {}", event.name);

                        if !event.emits.is_empty() {
                            println!("    Emit locations ({}):", event.emits.len());
                            for (file, line, kind) in event.emits.iter().take(3) {
                                println!("      {}:{} [{}]", file, line, kind);
                            }
                            if event.emits.len() > 3 {
                                println!("      ... and {} more", event.emits.len() - 3);
                            }
                        }

                        if !event.listens.is_empty() {
                            println!("    Listen locations ({}):", event.listens.len());
                            for (file, line) in event.listens.iter().take(3) {
                                println!("      {}:{}", file, line);
                            }
                            if event.listens.len() > 3 {
                                println!("      ... and {} more", event.listens.len() - 3);
                            }
                        }

                        // Highlight potential issues (not FE↔FE sync)
                        if event.emits.is_empty() {
                            println!("    [!] No emitters found (orphan listener?)");
                        }
                        if event.listens.is_empty() {
                            println!("    [!] No listeners found (orphan emitter?)");
                        }

                        println!();
                    }
                }

                if !symbol_events.is_empty() {
                    print_symbol_runtime_events(&symbol_events);
                }
            }
        }
    }

    // Zero silent cuts: always surface what the artifact fence removed.
    if !fence.is_empty() {
        if global.json {
            eprintln!("[loct] {}", fence.summary_line());
        } else {
            println!(
                "{} (use --include-artifacts to inspect)",
                fence.summary_line()
            );
        }
    }

    DispatchResult::Exit(0)
}

#[derive(Debug, Clone, serde::Serialize)]
struct SymbolRuntimeEvent {
    name: String,
    emits: Vec<SymbolRuntimeLocation>,
    observes: Vec<SymbolRuntimeLocation>,
    selectors: Vec<SymbolRuntimeLocation>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SymbolRuntimeLocation {
    file: String,
    line: usize,
    kind: String,
    confidence: String,
}

fn collect_symbol_runtime_events(snapshot: &crate::snapshot::Snapshot) -> Vec<SymbolRuntimeEvent> {
    use crate::symbols::SymbolEdgeKind;
    use std::collections::HashMap;

    let Some(graph) = snapshot.symbol_graph.as_ref() else {
        return Vec::new();
    };
    let nodes: HashMap<_, _> = graph
        .symbols
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut events: HashMap<String, SymbolRuntimeEvent> = HashMap::new();

    for edge in &graph.edges {
        let is_runtime_event = matches!(
            edge.kind,
            SymbolEdgeKind::NotificationEmit
                | SymbolEdgeKind::NotificationObserve
                | SymbolEdgeKind::SelectorMessage
        );
        if !is_runtime_event {
            continue;
        }

        let Some(target) = nodes.get(edge.to.as_str()) else {
            continue;
        };
        let Some(source) = nodes.get(edge.from.as_str()) else {
            continue;
        };
        let location = SymbolRuntimeLocation {
            file: source
                .file
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_string()),
            line: source.range.map(|range| range.start_line).unwrap_or(0),
            kind: format!("{:?}", edge.kind),
            confidence: format!("{:?}", edge.confidence),
        };
        let event = events
            .entry(target.name.clone())
            .or_insert_with(|| SymbolRuntimeEvent {
                name: target.name.clone(),
                emits: Vec::new(),
                observes: Vec::new(),
                selectors: Vec::new(),
            });
        match edge.kind {
            SymbolEdgeKind::NotificationEmit => event.emits.push(location),
            SymbolEdgeKind::NotificationObserve => event.observes.push(location),
            SymbolEdgeKind::SelectorMessage => event.selectors.push(location),
            _ => {}
        }
    }

    let mut events: Vec<_> = events.into_values().collect();
    events.sort_by(|a, b| a.name.cmp(&b.name));
    events
}

fn print_symbol_runtime_events(events: &[SymbolRuntimeEvent]) {
    println!("Symbol Graph Runtime Events ({}):\n", events.len());
    for event in events {
        println!("  Event: {}", event.name);
        if !event.emits.is_empty() {
            println!("    Emit locations ({}):", event.emits.len());
            for loc in event.emits.iter().take(3) {
                println!(
                    "      {}:{} [{}; {}]",
                    loc.file, loc.line, loc.kind, loc.confidence
                );
            }
            if event.emits.len() > 3 {
                println!("      ... and {} more", event.emits.len() - 3);
            }
        }
        if !event.observes.is_empty() {
            println!("    Observe locations ({}):", event.observes.len());
            for loc in event.observes.iter().take(3) {
                println!(
                    "      {}:{} [{}; {}]",
                    loc.file, loc.line, loc.kind, loc.confidence
                );
            }
            if event.observes.len() > 3 {
                println!("      ... and {} more", event.observes.len() - 3);
            }
        }
        if !event.selectors.is_empty() {
            println!("    Selector messages ({}):", event.selectors.len());
            for loc in event.selectors.iter().take(3) {
                println!(
                    "      {}:{} [{}; {}]",
                    loc.file, loc.line, loc.kind, loc.confidence
                );
            }
            if event.selectors.len() > 3 {
                println!("      ... and {} more", event.selectors.len() - 3);
            }
        }
        println!();
    }
}

/// Handle the routes command - list backend/web routes (FastAPI/Flask)
pub fn handle_routes_command(opts: &RoutesOptions, global: &GlobalOptions) -> DispatchResult {
    use std::path::Path;

    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Detecting backend routes..."))
    } else {
        None
    };

    let root = opts
        .roots
        .first()
        .map(|p| p.as_path())
        .unwrap_or(Path::new("."));

    let snapshot = match load_or_create_snapshot(root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    let framework_filter = opts.framework.as_ref().map(|f| f.to_lowercase());
    let path_filter = opts.path_filter.as_ref().map(|p| p.to_lowercase());

    let mut routes: Vec<serde_json::Value> = Vec::new();

    for file in &snapshot.files {
        for r in &file.routes {
            if let Some(ff) = &framework_filter
                && r.framework.to_lowercase() != *ff
            {
                continue;
            }
            if let Some(pf) = &path_filter {
                let path_match = r
                    .path
                    .as_ref()
                    .map(|p| p.to_lowercase().contains(pf))
                    .unwrap_or(false);
                if !path_match && !file.path.to_lowercase().contains(pf) {
                    continue;
                }
            }

            routes.push(serde_json::json!({
                "framework": r.framework,
                "method": r.method,
                "path": r.path,
                "handler": r.name,
                "file": file.path,
                "line": r.line,
            }));
        }
    }

    routes.sort_by(|a, b| {
        let af = a.get("framework").and_then(|v| v.as_str()).unwrap_or("");
        let bf = b.get("framework").and_then(|v| v.as_str()).unwrap_or("");
        let ap = a.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let bp = b.get("path").and_then(|v| v.as_str()).unwrap_or("");
        af.cmp(bf).then_with(|| ap.cmp(bp))
    });

    if let Some(s) = spinner {
        s.finish_success(&format!("Found {} route(s)", routes.len()));
    }

    if global.json {
        let output = serde_json::json!({
            "routes": routes,
            "summary": { "count": routes.len() }
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    } else if routes.is_empty() {
        println!("No routes detected.");
    } else {
        println!("Detected routes ({}):", routes.len());
        for r in &routes {
            let framework = r.get("framework").and_then(|v| v.as_str()).unwrap_or("-");
            let method = r.get("method").and_then(|v| v.as_str()).unwrap_or("-");
            let path = r
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("(no path)");
            let file = r.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let line = r.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
            let handler = r
                .get("handler")
                .and_then(|v| v.as_str())
                .unwrap_or("(anon)");
            println!(
                "  [{}] {} {} -> {}:{} ({})",
                framework, method, path, file, line, handler
            );
        }
        println!("\nTip: use --framework fastapi or --path <substr> to filter.");
    }

    DispatchResult::Exit(0)
}

/// Handle the focus command - extract holographic context for a directory
pub fn handle_focus_command(opts: &FocusOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::focuser::{FocusConfig, HolographicFocus};
    use std::path::Path;

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Analyzing directory..."))
    } else {
        None
    };

    // Load snapshot (auto-scan if missing)
    let root = opts.root.as_deref().unwrap_or(Path::new("."));

    let snapshot = match load_or_create_snapshot(root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    let config = FocusConfig {
        include_consumers: opts.consumers,
        max_depth: opts.depth.unwrap_or(2),
    };

    let focus = match HolographicFocus::from_path(&snapshot, &opts.target, &config) {
        Some(f) => f,
        None => {
            // Distinguish a genuine wrong path from a correct path that is simply
            // parked outside the snapshot by .loctignore (loctree-feedback.md: vista
            // docs/). When the latter, lead with the precise cause instead of
            // telling the user to "check the path".
            let ignore_hint = crate::fs_utils::loctignore_exclusion_hint(root, &opts.target);
            if let Some(s) = spinner {
                s.finish_error(&format!("No files found in directory '{}'", opts.target));
            }
            eprintln!();
            eprintln!("No files found in directory '{}'.", opts.target);
            eprintln!();
            if let Some(hint) = &ignore_hint {
                eprintln!("   {hint}");
                eprintln!(
                    "   (use `--full-scan` after editing .loctignore, or inspect the files directly)"
                );
            } else {
                eprintln!("   Possible causes:");
                eprintln!("   - Directory path is incorrect");
                eprintln!("   - Directory was added after last snapshot (run `loctree` to update)");
                eprintln!("   - All files in directory are excluded by .gitignore or .loctignore");
            }
            return DispatchResult::Exit(1);
        }
    };

    if let Some(s) = spinner {
        s.finish_success(&format!(
            "Found {} files in {}",
            focus.stats.core_files, opts.target
        ));
    }

    // Output results
    if global.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&focus.to_json()).unwrap_or_default()
        );
    } else {
        focus.print();
    }

    DispatchResult::Exit(0)
}

/// Handle the hotspots command - show import frequency heatmap
pub fn handle_hotspots_command(opts: &HotspotsOptions, global: &GlobalOptions) -> DispatchResult {
    use std::collections::HashMap;
    use std::path::Path;

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Analyzing import hotspots..."))
    } else {
        None
    };

    // Load snapshot (auto-scan if missing)
    let root = opts.root.as_deref().unwrap_or(Path::new("."));

    let snapshot = match load_or_create_snapshot(root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    // Calculate in-degree (how many files import this file) and out-degree (how many files this imports)
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut out_degree: HashMap<String, usize> = HashMap::new();

    // Initialize all files with 0
    for file in &snapshot.files {
        in_degree.insert(file.path.clone(), 0);
        out_degree.insert(file.path.clone(), 0);
    }

    // Count edges
    for edge in &snapshot.edges {
        *in_degree.entry(edge.to.clone()).or_insert(0) += 1;
        *out_degree.entry(edge.from.clone()).or_insert(0) += 1;
    }

    // Build list of (path, in_degree, out_degree)
    let mut hotspots: Vec<(String, usize, usize)> = in_degree
        .iter()
        .map(|(path, &in_deg)| {
            let out_deg = out_degree.get(path).copied().unwrap_or(0);
            (path.clone(), in_deg, out_deg)
        })
        .collect();

    // Filter
    let min_imports = opts.min_imports.unwrap_or(0);
    if opts.leaves_only {
        hotspots.retain(|(_, in_deg, _)| *in_deg == 0);
    } else if min_imports > 0 {
        hotspots.retain(|(_, in_deg, _)| *in_deg >= min_imports);
    }

    // Sort by in-degree (descending)
    hotspots.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    // Apply limit
    let limit = opts.limit.unwrap_or(50);
    if hotspots.len() > limit {
        hotspots.truncate(limit);
    }

    if let Some(s) = spinner {
        s.finish_success(&format!("Analyzed {} files", snapshot.files.len()));
    }

    // Output
    if global.json {
        let json_output: Vec<serde_json::Value> = hotspots
            .iter()
            .map(|(path, in_deg, out_deg)| {
                let category = match *in_deg {
                    n if n >= 10 => "CORE",
                    n if n >= 3 => "SHARED",
                    n if n >= 1 => "PERIPHERAL",
                    _ => "LEAF",
                };
                serde_json::json!({
                    "path": path,
                    "in_degree": in_deg,
                    "out_degree": out_deg,
                    "category": category
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json_output).unwrap_or_default()
        );
    } else {
        println!();
        println!("Import Hotspots ({} files analyzed)", snapshot.files.len());
        println!();

        // Group by category
        let core: Vec<_> = hotspots
            .iter()
            .filter(|(_, in_deg, _)| *in_deg >= 10)
            .collect();
        let shared: Vec<_> = hotspots
            .iter()
            .filter(|(_, in_deg, _)| *in_deg >= 3 && *in_deg < 10)
            .collect();
        let peripheral: Vec<_> = hotspots
            .iter()
            .filter(|(_, in_deg, _)| *in_deg >= 1 && *in_deg < 3)
            .collect();
        let leaves: Vec<_> = hotspots
            .iter()
            .filter(|(_, in_deg, _)| *in_deg == 0)
            .collect();

        if !core.is_empty() {
            println!("CORE (10+ importers):");
            for (path, in_deg, out_deg) in &core {
                if opts.coupling {
                    println!("  [in:{:<3} out:{:<3}] {}", in_deg, out_deg, path);
                } else {
                    println!("  [{:>3}] {}", in_deg, path);
                }
            }
            println!();
        }

        if !shared.is_empty() {
            println!("SHARED (3-9 importers):");
            for (path, in_deg, out_deg) in &shared {
                if opts.coupling {
                    println!("  [in:{:<3} out:{:<3}] {}", in_deg, out_deg, path);
                } else {
                    println!("  [{:>3}] {}", in_deg, path);
                }
            }
            println!();
        }

        if !peripheral.is_empty() {
            println!("PERIPHERAL (1-2 importers):");
            for (path, in_deg, out_deg) in &peripheral {
                if opts.coupling {
                    println!("  [in:{:<3} out:{:<3}] {}", in_deg, out_deg, path);
                } else {
                    println!("  [{:>3}] {}", in_deg, path);
                }
            }
            println!();
        }

        if !leaves.is_empty() {
            println!("LEAF (0 importers):");
            for (path, _, out_deg) in &leaves {
                if opts.coupling {
                    println!("  [in:0   out:{:<3}] {}", out_deg, path);
                } else {
                    println!("        {}", path);
                }
            }
            println!();
        }

        if hotspots.is_empty() {
            println!("  No files match the filter criteria.");
            println!();
        }

        // Summary
        println!(
            "Showing {} of {} files (--limit {})",
            hotspots.len(),
            snapshot.files.len(),
            limit
        );
        if opts.leaves_only {
            println!("Filtered to leaf nodes only (--leaves)");
        } else if min_imports > 0 {
            println!("Filtered to files with {} + importers (--min)", min_imports);
        }
    }

    DispatchResult::Exit(0)
}

/// Handle the layoutmap command - CSS z-index/sticky/grid analysis
pub fn handle_layoutmap_command(opts: &LayoutmapOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::layoutmap::scan_css_layout;
    use std::path::Path;

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Analyzing CSS layout properties..."))
    } else {
        None
    };

    let root = opts.root.as_deref().unwrap_or(Path::new("."));

    // Scan CSS files
    let findings = match scan_css_layout(root, opts) {
        Ok(f) => f,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to scan CSS: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    if let Some(s) = spinner {
        s.finish_success(&format!("Found {} layout findings", findings.len()));
    }

    // Output
    if global.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&findings).unwrap_or_default()
        );
    } else {
        print_layoutmap_human(&findings, opts);
    }

    DispatchResult::Exit(0)
}

fn print_layoutmap_human(findings: &[crate::layoutmap::LayoutFinding], opts: &LayoutmapOptions) {
    use crate::layoutmap::LayoutFinding;

    if findings.is_empty() {
        println!("\nNo CSS layout findings detected.\n");
        return;
    }

    // Group by type
    let zindex: Vec<_> = findings
        .iter()
        .filter(|f| matches!(f, LayoutFinding::ZIndex { .. }))
        .collect();
    let sticky: Vec<_> = findings
        .iter()
        .filter(|f| matches!(f, LayoutFinding::Sticky { .. }))
        .collect();
    let grid: Vec<_> = findings
        .iter()
        .filter(|f| matches!(f, LayoutFinding::Grid { .. }))
        .collect();
    let flex: Vec<_> = findings
        .iter()
        .filter(|f| matches!(f, LayoutFinding::Flex { .. }))
        .collect();

    println!();

    // Z-Index section
    if !opts.sticky_only && !opts.grid_only && !zindex.is_empty() {
        println!("Z-INDEX LAYERS (sorted by z-index):");
        let mut zindex_sorted: Vec<_> = zindex.iter().collect();
        zindex_sorted.sort_by(|a, b| {
            let za = match a {
                LayoutFinding::ZIndex { z_index, .. } => *z_index,
                _ => 0,
            };
            let zb = match b {
                LayoutFinding::ZIndex { z_index, .. } => *z_index,
                _ => 0,
            };
            zb.cmp(&za)
        });

        for finding in zindex_sorted {
            if let LayoutFinding::ZIndex {
                file,
                line,
                selector,
                z_index,
            } = finding
            {
                println!(
                    "  z-index: {:>6}  {}  ({}:{})",
                    z_index, selector, file, line
                );
            }
        }
        println!();
    }

    // Sticky section
    if !opts.zindex_only && !opts.grid_only && !sticky.is_empty() {
        println!("STICKY/FIXED ELEMENTS:");
        for finding in &sticky {
            if let LayoutFinding::Sticky {
                file,
                line,
                selector,
                position,
            } = finding
            {
                println!("  {} {:>6}  ({}:{})", selector, position, file, line);
            }
        }
        println!();
    }

    // Grid section
    if !opts.zindex_only && !opts.sticky_only && !grid.is_empty() {
        println!("CSS GRID CONTAINERS:");
        for finding in &grid {
            if let LayoutFinding::Grid {
                file,
                line,
                selector,
            } = finding
            {
                println!("  {}  ({}:{})", selector, file, line);
            }
        }
        println!();
    }

    // Flex section (only if not filtering)
    if !opts.zindex_only && !opts.sticky_only && !opts.grid_only && !flex.is_empty() {
        println!("FLEX CONTAINERS:");
        for finding in &flex {
            if let LayoutFinding::Flex {
                file,
                line,
                selector,
            } = finding
            {
                println!("  {}  ({}:{})", selector, file, line);
            }
        }
        println!();
    }

    // Summary
    println!(
        "Total: {} z-index, {} sticky/fixed, {} grid, {} flex",
        zindex.len(),
        sticky.len(),
        grid.len(),
        flex.len()
    );
}
/// Handle the zombie command - find all zombie code
pub fn handle_zombie_command(opts: &ZombieOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::analyzer::dead_parrots::DeadFilterConfig;
    use crate::analyzer::twins::{build_symbol_registry, detect_exact_twins};
    use std::collections::HashMap;
    use std::path::Path;

    // Deprecation warning (goes to stderr, won't break piped output)
    warn_deprecated("zombie", "loct '.dead_parrots'");

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Hunting for zombie code..."))
    } else {
        None
    };

    // Load snapshot (auto-scan if missing)
    let root = opts
        .roots
        .first()
        .map(|p| p.as_path())
        .unwrap_or(Path::new("."));

    let snapshot = match load_or_create_snapshot(root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    // 1. Find dead exports
    let dead_ok_globs = crate::fs_utils::load_loctignore_dead_ok_globs(root);
    let dead_exports = crate::analyzer::dead_parrots::compute_dead_truth_with(
        &snapshot,
        DeadFilterConfig {
            include_tests: opts.include_tests,
            include_helpers: false,
            library_mode: global.library_mode,
            example_globs: Vec::new(),
            python_library_mode: global.python_library,
            include_ambient: false,
            include_dynamic: false,
            dead_ok_globs,
        },
        false,
    )
    .dead;

    // 2. Find orphan files (files with 0 importers)
    let mut in_degree: HashMap<String, usize> = HashMap::new();

    // Initialize all files with 0
    for file in &snapshot.files {
        in_degree.insert(file.path.clone(), 0);
    }

    // Count edges
    for edge in &snapshot.edges {
        *in_degree.entry(edge.to.clone()).or_insert(0) += 1;
    }

    // Filter to orphan files (0 importers, non-entry-points, non-tests unless requested)
    let mut orphan_files: Vec<(String, usize)> = in_degree
        .iter()
        .filter(|(path, count)| {
            if **count > 0 {
                return false;
            }
            // Skip entry points
            if is_entry_point(path.as_str()) {
                return false;
            }
            // Skip tests unless --include-tests
            if !opts.include_tests && is_test_file_path(path.as_str()) {
                return false;
            }
            true
        })
        .map(|(path, _)| {
            let loc = snapshot
                .files
                .iter()
                .find(|f| &f.path == path)
                .map(|f| f.loc)
                .unwrap_or(0);
            (path.clone(), loc)
        })
        .collect();

    // Sort by LOC descending (biggest files first - most impact)
    orphan_files.sort_by_key(|b| std::cmp::Reverse(b.1));

    // 3. Find shadow exports (same symbol exported by multiple files where some have 0 imports)
    let twins = detect_exact_twins(&snapshot.files, opts.include_tests);
    let registry = build_symbol_registry(&snapshot.files, opts.include_tests);

    // Shadow exports: twins where at least one location has 0 imports but not all
    let mut shadow_exports: Vec<(String, usize, usize)> = Vec::new(); // (symbol, total_locations, dead_locations)

    for twin in &twins {
        let mut total_locations = 0;
        let mut dead_count = 0;

        for loc in &twin.locations {
            total_locations += 1;
            let key = (loc.file_path.clone(), twin.name.clone());
            if let Some(entry) = registry.get(&key)
                && entry.import_count == 0
            {
                dead_count += 1;
            }
        }

        // Shadow if: multiple locations, at least one dead, not all dead
        if total_locations >= 2 && dead_count > 0 && dead_count < total_locations {
            shadow_exports.push((twin.name.clone(), total_locations, dead_count));
        }
    }

    // Calculate total LOC for orphan files
    let orphan_loc: usize = orphan_files.iter().map(|(_, loc)| loc).sum();

    if let Some(s) = spinner {
        s.finish_success(&format!(
            "Found {} dead exports, {} orphan files, {} shadow exports",
            dead_exports.len(),
            orphan_files.len(),
            shadow_exports.len()
        ));
    }

    // Output results
    if global.json {
        let json = serde_json::json!({
            "dead_exports": dead_exports.iter().map(|d| {
                serde_json::json!({
                    "file": d.file,
                    "line": d.line,
                    "symbol": d.symbol,
                    "confidence": d.confidence
                })
            }).collect::<Vec<_>>(),
            "orphan_files": orphan_files.iter().map(|(path, loc)| {
                serde_json::json!({
                    "path": path,
                    "loc": loc
                })
            }).collect::<Vec<_>>(),
            "shadow_exports": shadow_exports.iter().map(|(symbol, total, dead)| {
                serde_json::json!({
                    "symbol": symbol,
                    "total_locations": total,
                    "dead_locations": dead
                })
            }).collect::<Vec<_>>(),
            "summary": {
                "dead_exports_count": dead_exports.len(),
                "orphan_files_count": orphan_files.len(),
                "orphan_files_loc": orphan_loc,
                "shadow_exports_count": shadow_exports.len(),
                "total_zombie_items": dead_exports.len() + orphan_files.len() + shadow_exports.len()
            }
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).unwrap_or_default()
        );
    } else {
        // Human-readable output
        println!();
        println!("=== Zombie Code Report ===");
        println!();

        // Dead Exports section
        println!("Dead Exports ({}):", dead_exports.len());
        if dead_exports.is_empty() {
            println!("  (none)");
        } else {
            for (i, dead) in dead_exports.iter().take(10).enumerate() {
                let line_str = dead
                    .line
                    .map(|l| l.to_string())
                    .unwrap_or_else(|| "?".to_string());
                println!(
                    "  {}:{}  {} [{}]",
                    dead.file, line_str, dead.symbol, dead.confidence
                );
                if i == 9 && dead_exports.len() > 10 {
                    println!("  ... and {} more", dead_exports.len() - 10);
                }
            }
        }
        println!();

        // Orphan Files section
        println!(
            "Orphan Files (0 importers, {} files, {} LOC):",
            orphan_files.len(),
            orphan_loc
        );
        if orphan_files.is_empty() {
            println!("  (none)");
        } else {
            for (i, (path, loc)) in orphan_files.iter().take(10).enumerate() {
                println!("  {} ({} LOC)", path, loc);
                if i == 9 && orphan_files.len() > 10 {
                    println!("  ... and {} more", orphan_files.len() - 10);
                }
            }
        }
        println!();

        // Shadow Exports section
        println!("Shadow Exports ({}):", shadow_exports.len());
        if shadow_exports.is_empty() {
            println!("  (none)");
        } else {
            for (symbol, total, dead) in &shadow_exports {
                println!("  {} exported by {} files, {} dead", symbol, total, dead);
            }
        }
        println!();

        // Summary
        let total_items = dead_exports.len() + orphan_files.len() + shadow_exports.len();
        println!(
            "Total: {} zombie items, ~{} LOC to review",
            total_items, orphan_loc
        );
        println!();
    }

    DispatchResult::Exit(0)
}

/// Check if a file is an entry point
fn is_entry_point(path: &str) -> bool {
    path.ends_with("/main.rs")
        || path.ends_with("/lib.rs")
        || path.ends_with("/main.ts")
        || path.ends_with("/main.tsx")
        || path.ends_with("/main.js")
        || path.ends_with("/main.jsx")
        || path.ends_with("/index.ts")
        || path.ends_with("/index.tsx")
        || path.ends_with("/index.js")
        || path.ends_with("/index.jsx")
        || path.ends_with("/App.tsx")
        || path.ends_with("/App.jsx")
        || path.ends_with("/_app.tsx")
        || path.ends_with("/_app.jsx")
        || path.ends_with("/__init__.py")
        || path == "main.rs"
        || path == "lib.rs"
        || path == "main.ts"
        || path == "index.ts"
}

/// Check if a file path looks like a test file
fn is_test_file_path(path: &str) -> bool {
    path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("/__tests__/")
        || path.contains("/spec/")
        || path.ends_with(".test.ts")
        || path.ends_with(".test.tsx")
        || path.ends_with(".test.js")
        || path.ends_with(".test.jsx")
        || path.ends_with(".spec.ts")
        || path.ends_with(".spec.tsx")
        || path.ends_with(".spec.js")
        || path.ends_with(".spec.jsx")
        || path.ends_with("_test.rs")
        || path.ends_with("_test.py")
        || path.starts_with("test_")
        || path.contains("/test_")
}

/// Handle the health command - quick summary of cycles + dead + twins
pub fn handle_health_command(opts: &HealthOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::analyzer::cycles::{CycleCompilability, find_cycles_classified_with_lazy};
    use crate::analyzer::dead_parrots::DeadFilterConfig;
    use crate::analyzer::twins::detect_exact_twins;
    use crate::colors::Painter;
    use std::collections::{HashMap, HashSet};
    use std::path::Path;

    let p = Painter::new(global.color);

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Running health check..."))
    } else {
        None
    };

    // Load snapshot (auto-scan if missing)
    let root = opts
        .roots
        .first()
        .map(|p| p.as_path())
        .unwrap_or(Path::new("."));

    let snapshot = match load_or_create_snapshot(root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    // 1. Cycles analysis
    let edges: Vec<(String, String, String)> = snapshot
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
        .collect();

    let (classified_cycles, _) = find_cycles_classified_with_lazy(&edges);

    let high_risk_cycles = classified_cycles
        .iter()
        .filter(|c| c.compilability == CycleCompilability::Breaking)
        .count();
    let structural_cycles = classified_cycles
        .iter()
        .filter(|c| c.compilability == CycleCompilability::Structural)
        .count();
    let total_cycles = classified_cycles.len();

    // 2. Dead exports analysis
    let dead_ok_globs = crate::fs_utils::load_loctignore_dead_ok_globs(root);
    let dead_exports = crate::analyzer::dead_parrots::compute_dead_truth_with(
        &snapshot,
        DeadFilterConfig {
            include_tests: opts.include_tests,
            include_helpers: false,
            library_mode: global.library_mode,
            example_globs: Vec::new(),
            python_library_mode: global.python_library,
            include_ambient: false,
            include_dynamic: false,
            dead_ok_globs,
        },
        false,
    )
    .dead;

    // Count by confidence
    let high_confidence = dead_exports
        .iter()
        .filter(|d| d.confidence == "high")
        .count();
    let low_confidence = dead_exports.len() - high_confidence;
    let mut dead_by_file: HashMap<String, usize> = HashMap::new();
    for dead in &dead_exports {
        *dead_by_file.entry(dead.file.clone()).or_insert(0) += 1;
    }
    let mut top_dead_files: Vec<(String, usize)> = dead_by_file.into_iter().collect();
    top_dead_files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let top_dead_files: Vec<String> = top_dead_files
        .into_iter()
        .take(3)
        .map(|(path, count)| {
            let display_name = Path::new(&path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path.as_str());
            format!("{display_name} ({count} dead)")
        })
        .collect();

    // 3. Twins analysis
    let twins = detect_exact_twins(&snapshot.files, opts.include_tests);
    let twin_count = twins.len();
    let mut twin_examples: Vec<(String, usize)> = twins
        .iter()
        .map(|twin| {
            let file_count = twin
                .locations
                .iter()
                .map(|loc| loc.file_path.as_str())
                .collect::<HashSet<_>>()
                .len();
            (twin.name.clone(), file_count)
        })
        .collect();
    twin_examples.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let top_twin_groups: Vec<String> = twin_examples
        .into_iter()
        .take(3)
        .map(|(name, file_count)| format!("{name} ({file_count} files)"))
        .collect();

    if let Some(s) = spinner {
        s.finish_success("Health check complete");
    }

    // Output results
    if global.json {
        // loctree-feedback hak 2026-05-18 Screenscribe #1: `loct health` must
        // not return silent `OK` while `loct findings`, `loct insights`,
        // and broad `loct twins` (barrel chaos + missing index.ts +
        // inconsistent paths) report real issues in the same snapshot.
        // JSON consumers get an explicit `scope` block enumerating what
        // is and is not measured by this command.
        let json = serde_json::json!({
            "cycles": {
                "total": total_cycles,
                "high_risk": high_risk_cycles,
                "structural": structural_cycles
            },
            "dead_exports": {
                "total": dead_exports.len(),
                "high_confidence": high_confidence,
                "low_confidence": low_confidence
            },
            "twins": {
                "total": twin_count
            },
            "scope": {
                "covered": ["cycles", "dead_exports", "exact_twin_groups"],
                "not_covered": [
                    "duplicate_export_groups (see `loct findings`)",
                    "insights / huge files / missing handlers (see `loct insights`)",
                    "barrel chaos / missing index.ts / inconsistent paths (see `loct twins`)",
                    "coverage gaps (see `loct coverage`)",
                    "route twins (see `loct routes`)"
                ],
                "doctrine_note": "narrow OK on this surface does not imply broad OK across the repo"
            }
        });
        println!("{}", serde_json::to_string_pretty(&json).unwrap());
    } else {
        println!("\n{}\n", p.header("Health Check Summary"));

        // Cycles
        if total_cycles == 0 {
            println!("Cycles:      {} (none detected)", p.ok("[OK]"));
        } else {
            let status = if high_risk_cycles > 0 {
                p.error(&format!("{} total", total_cycles))
            } else {
                p.warn(&format!("{} total", total_cycles))
            };
            println!(
                "Cycles:      {} ({} high-risk, {} structural)",
                status,
                p.error(&high_risk_cycles.to_string()),
                p.warn(&structural_cycles.to_string())
            );
        }

        // Dead exports
        if dead_exports.is_empty() {
            println!("Dead:        {} (none detected)", p.ok("[OK]"));
        } else {
            println!(
                "Dead:        {} high confidence, {} low",
                p.ok(&high_confidence.to_string()),
                p.warn(&low_confidence.to_string())
            );
            if !top_dead_files.is_empty() {
                println!("             top files: {}", top_dead_files.join(", "));
            }
        }

        // Twins
        if twin_count == 0 {
            println!("Twins:       {} (none detected)", p.ok("[OK]"));
        } else {
            println!(
                "Twins:       {} duplicate symbol groups",
                p.warn(&twin_count.to_string())
            );
            if !top_twin_groups.is_empty() {
                println!("             top: {}", top_twin_groups.join(", "));
            }
        }

        println!();
        // loctree-feedback hak 2026-05-18 Screenscribe #1: `health` must
        // tell the operator what it did NOT check, so a green narrow-OK
        // never reads as a green broad-OK. Footer enumerates the broader
        // surfaces explicitly.
        println!(
            "{}",
            p.dim("Scope: this summary measures cycles, dead exports, and exact-twin groups only.")
        );
        println!(
            "{}",
            p.dim("Not covered here: duplicate exports, insights, barrel chaos, coverage gaps, route twins.")
        );
        println!(
            "Run {}, {}, {}, {}, {} for the broader surface.",
            p.dim("`loct findings`"),
            p.dim("`loct insights`"),
            p.dim("`loct twins`"),
            p.dim("`loct coverage`"),
            p.dim("`loct routes`")
        );
        println!(
            "Drill into this summary with {}, {}, {}.",
            p.dim("`loct cycles`"),
            p.dim("`loct dead`"),
            p.dim("`loct twins`")
        );
        println!();
    }

    DispatchResult::Exit(0)
}

fn insert_audit_collection<T: serde::Serialize>(
    section: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    items: &[T],
    limit: Option<usize>,
) {
    let display_limit = limit.unwrap_or(usize::MAX);
    section.insert(
        key.to_string(),
        serde_json::json!(items.iter().take(display_limit).collect::<Vec<_>>()),
    );

    if let Some(limit) = limit {
        let omitted = items.len().saturating_sub(limit);
        section.insert("limit".to_string(), serde_json::json!(limit));
        section.insert("omitted".to_string(), serde_json::json!(omitted));
        section.insert("truncated".to_string(), serde_json::json!(omitted > 0));
    }
}

fn build_audit_json(
    findings: &crate::analyzer::audit_report::AuditFindings,
    limit: Option<usize>,
) -> serde_json::Value {
    use crate::analyzer::cycles::CycleCompilability;
    use serde_json::{Map, Value, json};

    let high_confidence = findings
        .dead_exports
        .iter()
        .filter(|d| d.confidence == "high")
        .count();
    let low_confidence = findings.dead_exports.len() - high_confidence;
    let high_risk_cycles = findings
        .cycles
        .iter()
        .filter(|c| c.compilability == CycleCompilability::Breaking)
        .count();
    let structural_cycles = findings
        .cycles
        .iter()
        .filter(|c| c.compilability == CycleCompilability::Structural)
        .count();
    let orphan_loc: usize = findings.orphan_files.iter().map(|f| f.loc).sum();

    let mut cycles = Map::new();
    cycles.insert("total".to_string(), json!(findings.cycles.len()));
    cycles.insert("high_risk".to_string(), json!(high_risk_cycles));
    cycles.insert("structural".to_string(), json!(structural_cycles));
    insert_audit_collection(&mut cycles, "items", &findings.cycles, limit);

    let mut dead_exports = Map::new();
    dead_exports.insert("total".to_string(), json!(findings.dead_exports.len()));
    dead_exports.insert("high_confidence".to_string(), json!(high_confidence));
    dead_exports.insert("low_confidence".to_string(), json!(low_confidence));
    insert_audit_collection(&mut dead_exports, "items", &findings.dead_exports, limit);

    let mut twins = Map::new();
    twins.insert("total".to_string(), json!(findings.twins.len()));
    insert_audit_collection(&mut twins, "groups", &findings.twins, limit);

    let mut orphan_files = Map::new();
    orphan_files.insert("total".to_string(), json!(findings.orphan_files.len()));
    orphan_files.insert("total_loc".to_string(), json!(orphan_loc));
    insert_audit_collection(&mut orphan_files, "files", &findings.orphan_files, limit);

    let mut shadow_exports = Map::new();
    shadow_exports.insert("total".to_string(), json!(findings.shadow_exports.len()));
    insert_audit_collection(
        &mut shadow_exports,
        "items",
        &findings.shadow_exports,
        limit,
    );

    let mut crowds = Map::new();
    crowds.insert("total".to_string(), json!(findings.crowds.len()));
    insert_audit_collection(&mut crowds, "clusters", &findings.crowds, limit);

    Value::Object(Map::from_iter([
        ("cycles".to_string(), Value::Object(cycles)),
        ("dead_exports".to_string(), Value::Object(dead_exports)),
        ("twins".to_string(), Value::Object(twins)),
        ("orphan_files".to_string(), Value::Object(orphan_files)),
        ("shadow_exports".to_string(), Value::Object(shadow_exports)),
        ("crowds".to_string(), Value::Object(crowds)),
        (
            "summary".to_string(),
            json!({
                "total_files": findings.total_files,
                "total_loc": findings.total_loc,
            }),
        ),
    ]))
}

/// Handle the audit command - full codebase audit with actionable markdown report
pub fn handle_audit_command(opts: &AuditOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::analyzer::audit_report::{
        AuditFindings, OrphanFile, ShadowExport, generate_markdown_report, generate_todos,
    };
    use crate::analyzer::crowd::detect_all_crowds;
    use crate::analyzer::cycles::{CycleCompilability, find_cycles_classified_with_lazy};
    use crate::analyzer::dead_parrots::DeadFilterConfig;
    use crate::analyzer::twins::{build_symbol_registry, detect_exact_twins};
    use std::collections::HashMap;
    use std::path::Path;

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Running full audit..."))
    } else {
        None
    };

    // Load snapshot (auto-scan if missing)
    let root = opts
        .roots
        .first()
        .map(|p| p.as_path())
        .unwrap_or(Path::new("."));

    let snapshot = match load_or_create_snapshot(root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    // 1. Cycles analysis
    let edges: Vec<(String, String, String)> = snapshot
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
        .collect();

    let (classified_cycles, _) = find_cycles_classified_with_lazy(&edges);

    let _high_risk_cycles = classified_cycles
        .iter()
        .filter(|c| c.compilability == CycleCompilability::Breaking)
        .count();
    let _structural_cycles = classified_cycles
        .iter()
        .filter(|c| c.compilability == CycleCompilability::Structural)
        .count();
    let _total_cycles = classified_cycles.len();

    // 2. Dead exports analysis
    let dead_ok_globs = crate::fs_utils::load_loctignore_dead_ok_globs(root);
    let dead_exports = crate::analyzer::dead_parrots::compute_dead_truth_with(
        &snapshot,
        DeadFilterConfig {
            include_tests: opts.include_tests,
            include_helpers: false,
            library_mode: global.library_mode,
            example_globs: Vec::new(),
            python_library_mode: global.python_library,
            include_ambient: false,
            include_dynamic: false,
            dead_ok_globs,
        },
        false,
    )
    .dead;

    let high_confidence = dead_exports
        .iter()
        .filter(|d| d.confidence == "high")
        .count();
    let _low_confidence = dead_exports.len() - high_confidence;

    // 3. Twins analysis
    let twins = detect_exact_twins(&snapshot.files, opts.include_tests);
    let _twin_count = twins.len();

    // 4. Orphan files (files with 0 importers)
    let mut in_degree: HashMap<String, usize> = HashMap::new();

    for file in &snapshot.files {
        in_degree.insert(file.path.clone(), 0);
    }

    for edge in &snapshot.edges {
        *in_degree.entry(edge.to.clone()).or_insert(0) += 1;
    }

    let mut orphan_files: Vec<(String, usize)> = in_degree
        .iter()
        .filter(|(path, count)| {
            if **count > 0 {
                return false;
            }
            if is_entry_point(path.as_str()) {
                return false;
            }
            if !opts.include_tests && is_test_file_path(path.as_str()) {
                return false;
            }
            true
        })
        .map(|(path, _)| {
            let loc = snapshot
                .files
                .iter()
                .find(|f| &f.path == path)
                .map(|f| f.loc)
                .unwrap_or(0);
            (path.clone(), loc)
        })
        .collect();

    orphan_files.sort_by_key(|b| std::cmp::Reverse(b.1));
    let _orphan_loc: usize = orphan_files.iter().map(|(_, loc)| loc).sum();

    // Artifact fence: generated files, lockfiles, vendored code, fixtures and
    // docs are not actionable "orphans to review" — separate, don't drop.
    let (artifact_orphan_files, orphan_files): (Vec<_>, Vec<_>) =
        orphan_files.into_iter().partition(|(path, _)| {
            crate::analyzer::classify::artifact_class(path, None).is_artifact()
                || crate::analyzer::classify::resource_kind(path) == Some("doc")
        });

    // Entry-point fence: runtime entries (Cargo [[bin]], package.json
    // main/bin, shebang, detected main markers) have no importers by design —
    // they are roots, not orphans to review.
    let runtime_entries =
        crate::analyzer::dead_parrots::filters::runtime_entrypoint_paths(&snapshot);
    let (entrypoint_orphan_files, orphan_files): (Vec<_>, Vec<_>) =
        orphan_files.into_iter().partition(|(path, _)| {
            runtime_entries.contains(path.replace('\\', "/").trim_start_matches("./"))
        });

    // 5. Shadow exports
    let registry = build_symbol_registry(&snapshot.files, opts.include_tests);
    let mut shadow_exports: Vec<(String, usize, usize)> = Vec::new();

    for twin in &twins {
        let mut total_locations = 0;
        let mut dead_count = 0;

        for loc in &twin.locations {
            total_locations += 1;
            let key = (loc.file_path.clone(), twin.name.clone());
            if let Some(entry) = registry.get(&key)
                && entry.import_count == 0
            {
                dead_count += 1;
            }
        }

        if dead_count > 0 && dead_count < total_locations {
            shadow_exports.push((twin.name.clone(), total_locations, dead_count));
        }
    }

    // 6. Crowds analysis
    let crowds = detect_all_crowds(&snapshot.files);

    if let Some(s) = spinner {
        s.finish_success("Audit complete");
    }

    // Build AuditFindings struct
    let total_loc: usize = snapshot.files.iter().map(|f| f.loc).sum();

    let findings = AuditFindings {
        cycles: classified_cycles,
        dead_exports,
        twins,
        orphan_files: orphan_files
            .into_iter()
            .map(|(path, loc)| OrphanFile { path, loc })
            .collect(),
        artifact_orphans: artifact_orphan_files
            .into_iter()
            .map(|(path, loc)| OrphanFile { path, loc })
            .collect(),
        entrypoint_orphans: entrypoint_orphan_files
            .into_iter()
            .map(|(path, loc)| OrphanFile { path, loc })
            .collect(),
        shadow_exports: shadow_exports
            .into_iter()
            .map(|(name, total_locations, dead_locations)| ShadowExport {
                name,
                total_locations,
                dead_locations,
            })
            .collect(),
        crowds,
        total_files: snapshot.files.len(),
        total_loc,
    };

    // Calculate summary stats for terminal output
    use crate::colors::Painter;
    let p = Painter::new(global.color);

    let high_confidence = findings
        .dead_exports
        .iter()
        .filter(|d| d.confidence == "high")
        .count();
    let high_risk_cycles = findings
        .cycles
        .iter()
        .filter(|c| c.compilability == CycleCompilability::Breaking)
        .count();

    // Output results
    if global.json {
        let json = build_audit_json(&findings, opts.limit);
        println!("{}", serde_json::to_string_pretty(&json).unwrap());
    } else {
        // Audit markdown is artifact-only to avoid truncation in terminal/agent pipelines.
        let loctree_dir = crate::snapshot::Snapshot::artifacts_dir(root);
        if !loctree_dir.exists() {
            std::fs::create_dir_all(&loctree_dir).ok();
        }

        let (filename, output) = if opts.todos {
            ("audit_todos.md", generate_todos(&findings, opts.limit))
        } else {
            (
                "audit_report.md",
                generate_markdown_report(&findings, opts.limit),
            )
        };

        let report_path = loctree_dir.join(filename);

        // Write report to file
        if let Err(e) = std::fs::write(&report_path, &output) {
            eprintln!("{}", p.error(&format!("Failed to write report: {}", e)));
            return DispatchResult::Exit(1);
        }

        // Print colored summary to terminal
        let total_issues = findings.cycles.len()
            + findings.dead_exports.len()
            + findings.twins.len()
            + findings.shadow_exports.len();

        println!("\n{}\n", p.header("Audit Summary"));
        println!(
            "  Files: {}  |  LOC: {}  |  Issues: {}",
            p.number(findings.total_files),
            p.number(findings.total_loc),
            if total_issues > 0 {
                p.warn(&total_issues.to_string())
            } else {
                p.ok(&total_issues.to_string())
            }
        );

        if high_risk_cycles > 0 {
            println!(
                "  {} {} breaking cycles",
                p.error("[!]"),
                p.error(&high_risk_cycles.to_string())
            );
        }
        if high_confidence > 0 {
            println!(
                "  {} {} high-confidence dead exports",
                p.warn("[~]"),
                p.warn(&high_confidence.to_string())
            );
        }
        if !findings.twins.is_empty() {
            println!(
                "  {} {} duplicate symbol groups",
                p.info("[i]"),
                p.info(&findings.twins.len().to_string())
            );
        }

        println!(
            "\n{} {}\n",
            p.ok("Report saved:"),
            p.path(&report_path.display().to_string())
        );

        // Open the file (unless --no-open)
        if !opts.no_open {
            #[cfg(target_os = "macos")]
            {
                std::process::Command::new("open")
                    .arg(&report_path)
                    .spawn()
                    .ok();
            }
            #[cfg(target_os = "linux")]
            {
                std::process::Command::new("xdg-open")
                    .arg(&report_path)
                    .spawn()
                    .ok();
            }
            #[cfg(target_os = "windows")]
            {
                std::process::Command::new("cmd")
                    .args(["/C", "start", &report_path.display().to_string()])
                    .spawn()
                    .ok();
            }
        }
    }

    DispatchResult::Exit(0)
}

/// Handle the doctor command - interactive diagnostics with actionable recommendations
pub fn handle_doctor_command(opts: &DoctorOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::analyzer::cycles::{CycleCompilability, find_cycles_classified_with_lazy};
    use crate::analyzer::dead_parrots::DeadFilterConfig;
    use crate::analyzer::twins::detect_exact_twins;
    use crate::colors::Painter;
    use std::path::Path;

    let p = Painter::new(global.color);

    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Running diagnostics..."))
    } else {
        None
    };

    let root = opts
        .roots
        .first()
        .map(|p| p.as_path())
        .unwrap_or(Path::new("."));

    let snapshot = match load_or_create_snapshot(root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    // 1. Cycles analysis
    let edges: Vec<(String, String, String)> = snapshot
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
        .collect();

    let (classified_cycles, _) = find_cycles_classified_with_lazy(&edges);

    let high_risk_cycles: Vec<_> = classified_cycles
        .iter()
        .filter(|c| c.compilability == CycleCompilability::Breaking)
        .collect();
    let structural_cycles: Vec<_> = classified_cycles
        .iter()
        .filter(|c| c.compilability == CycleCompilability::Structural)
        .collect();

    // 2. Dead exports analysis
    let dead_ok_globs = crate::fs_utils::load_loctignore_dead_ok_globs(root);
    let dead_exports = crate::analyzer::dead_parrots::compute_dead_truth_with(
        &snapshot,
        DeadFilterConfig {
            include_tests: opts.include_tests,
            include_helpers: false,
            library_mode: global.library_mode,
            example_globs: Vec::new(),
            python_library_mode: global.python_library,
            include_ambient: false,
            include_dynamic: false,
            dead_ok_globs,
        },
        false,
    )
    .dead;

    let high_confidence_dead: Vec<_> = dead_exports
        .iter()
        .filter(|d| d.confidence == "high")
        .collect();
    let low_confidence_dead: Vec<_> = dead_exports
        .iter()
        .filter(|d| d.confidence != "high")
        .collect();

    // 3. Twins analysis
    let twins = detect_exact_twins(&snapshot.files, opts.include_tests);

    // 4. Categorize findings
    let mut auto_fixable = high_confidence_dead.len();
    let mut needs_review = low_confidence_dead.len() + high_risk_cycles.len();
    let mut false_positive_patterns: Vec<String> = Vec::new();

    for twin in &twins {
        let has_index = twin
            .locations
            .iter()
            .any(|loc| loc.file_path.contains("index."));
        let has_test = twin
            .locations
            .iter()
            .any(|loc| loc.file_path.contains("test") || loc.file_path.contains("spec"));
        if has_index || has_test {
            let pattern = if has_index {
                "**/index.*".to_string()
            } else {
                "**/*test*".to_string()
            };
            if !false_positive_patterns.contains(&pattern) {
                false_positive_patterns.push(pattern);
            }
            needs_review += 1;
        } else {
            auto_fixable += 1;
        }
    }

    if let Some(s) = spinner {
        s.finish_success("Diagnostics complete");
    }

    if global.json {
        let json = serde_json::json!({
            "summary": {
                "auto_fixable": auto_fixable,
                "needs_review": needs_review,
                "total_issues": auto_fixable + needs_review
            },
            "cycles": {
                "high_risk": high_risk_cycles.len(),
                "structural": structural_cycles.len(),
                "total": classified_cycles.len()
            },
            "dead_exports": {
                "high_confidence": high_confidence_dead.len(),
                "low_confidence": low_confidence_dead.len(),
                "total": dead_exports.len()
            },
            "twins": { "total": twins.len() },
            "suggested_suppressions": false_positive_patterns
        });
        println!("{}", serde_json::to_string_pretty(&json).unwrap());
    } else {
        println!("\n{}\n", p.header("=== Doctor Diagnostics ==="));
        println!(
            "Found {} issues: {} auto-fixable, {} need review\n",
            p.number(auto_fixable + needs_review),
            p.ok(&auto_fixable.to_string()),
            p.warn(&needs_review.to_string())
        );

        // Cycles
        if !classified_cycles.is_empty() {
            println!(
                "{} ({} total):",
                p.header("Circular Imports"),
                p.number(classified_cycles.len())
            );
            if !high_risk_cycles.is_empty() {
                println!(
                    "  {} {} (breaking)",
                    p.error(&high_risk_cycles.len().to_string()),
                    p.error("high-risk cycles")
                );
                for (i, cycle) in high_risk_cycles.iter().take(3).enumerate() {
                    println!(
                        "    {}. {} -> {} files",
                        i + 1,
                        p.path(&cycle.nodes[0]),
                        cycle.nodes.len()
                    );
                }
                if high_risk_cycles.len() > 3 {
                    println!(
                        "    {} {} more",
                        p.dim("...and"),
                        high_risk_cycles.len() - 3
                    );
                }
            }
            if !structural_cycles.is_empty() {
                println!(
                    "  {} {} (warnings)",
                    p.warn(&structural_cycles.len().to_string()),
                    p.warn("structural cycles")
                );
            }
            println!("  Run {} for details\n", p.dim("`loct cycles`"));
        }

        // Dead exports
        if !dead_exports.is_empty() {
            println!(
                "{} ({} total):",
                p.header("Dead Exports"),
                p.number(dead_exports.len())
            );
            println!(
                "  {} {} (safe to remove)",
                p.ok(&high_confidence_dead.len().to_string()),
                p.ok("high confidence")
            );
            for (i, dead) in high_confidence_dead.iter().take(5).enumerate() {
                let line_str = dead
                    .line
                    .map(|l| l.to_string())
                    .unwrap_or_else(|| "?".to_string());
                println!(
                    "    {}. {}:{} - {}",
                    i + 1,
                    p.path(&dead.file),
                    line_str,
                    p.symbol(&dead.symbol)
                );
            }
            if high_confidence_dead.len() > 5 {
                println!(
                    "    {} {} more",
                    p.dim("...and"),
                    high_confidence_dead.len() - 5
                );
            }
            if !low_confidence_dead.is_empty() {
                println!(
                    "  {} {} (needs review)",
                    p.warn(&low_confidence_dead.len().to_string()),
                    p.warn("low confidence")
                );
            }
            println!("  Run {} for full list\n", p.dim("`loct dead`"));
        }

        // Twins
        if !twins.is_empty() {
            println!(
                "{} ({} groups):",
                p.header("Duplicate Symbols"),
                p.number(twins.len())
            );
            for (i, twin) in twins.iter().take(3).enumerate() {
                println!(
                    "    {}. {} appears in {} files",
                    i + 1,
                    p.symbol(&format!("'{}'", twin.name)),
                    p.number(twin.locations.len())
                );
            }
            if twins.len() > 3 {
                println!("    {} {} more groups", p.dim("...and"), twins.len() - 3);
            }
            println!("  Run {} for details\n", p.dim("`loct twins`"));
        }

        // Suppressions
        if !false_positive_patterns.is_empty() {
            println!(
                "{} for false positives:",
                p.header("Suggested .loctignore entries")
            );
            for pattern in &false_positive_patterns {
                println!("  {}", p.path(pattern));
            }

            if opts.apply_suppressions {
                println!("\n{}...", p.info("Applying suppressions to .loctignore"));
                let loctignore_path = root.join(".loctignore");
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(&loctignore_path)
                {
                    use std::io::Write;
                    writeln!(file, "\n# Auto-generated by loct doctor").ok();
                    for pattern in &false_positive_patterns {
                        writeln!(file, "{}", pattern).ok();
                    }
                    println!(
                        "{} {}",
                        p.ok("Suppressions written to"),
                        p.path(&loctignore_path.display().to_string())
                    );
                } else {
                    eprintln!("{}", p.error("Failed to write .loctignore"));
                }
            } else {
                println!(
                    "\nRun with {} to automatically add these",
                    p.info("--apply-suppressions")
                );
            }
            println!();
        }

        // Next steps
        println!("{}:", p.header("Next steps"));
        if auto_fixable > 0 {
            println!(
                "  1. Review {} dead exports and remove if safe",
                p.ok("high-confidence")
            );
            println!("  2. Run tests after each removal to ensure nothing breaks");
        }
        if needs_review > 0 {
            println!(
                "  3. Investigate {} findings manually",
                p.warn("low-confidence")
            );
        }
        if !high_risk_cycles.is_empty() {
            println!(
                "  4. Break {} using dependency injection or interfaces",
                p.error("circular imports")
            );
        }
        println!();
    }

    DispatchResult::Exit(0)
}

/// Handle the plan command - generate refactoring plan
pub fn handle_plan_command(opts: &PlanOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::refactor_plan::{generate_refactor_plan, output, parse_target_layout_spec};
    use crate::snapshot::resolve_snapshot_root;
    use std::path::PathBuf;

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Generating refactor plan..."))
    } else {
        None
    };

    let roots: Vec<PathBuf> = if opts.roots.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        opts.roots.clone()
    };

    let snapshot_root = resolve_snapshot_root(&roots);
    let snapshot = match load_or_create_snapshot(&snapshot_root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    let layout = match opts.target_layout.as_deref() {
        Some(spec) => match parse_target_layout_spec(spec) {
            Ok(map) => Some(map),
            Err(e) => {
                if let Some(s) = spinner {
                    s.finish_error(&e);
                } else {
                    eprintln!("[loct][error] {}", e);
                }
                return DispatchResult::Exit(2);
            }
        },
        None => None,
    };

    // Generate one plan per root (multi-target output)
    let mut plans = Vec::new();
    let mut skipped = Vec::new();
    for root in &roots {
        let target_dir = root.as_path();
        let target_str = target_dir.to_str().unwrap_or(".");
        match generate_refactor_plan(&snapshot, target_str, layout.as_ref()) {
            Some(plan) => plans.push(plan),
            None => skipped.push(target_str.to_string()),
        }
    }

    if plans.is_empty() {
        if let Some(s) = spinner {
            s.finish_success("No files need reorganization");
        } else if !global.quiet {
            if skipped.len() == 1 {
                println!("No files need reorganization in {}", skipped[0]);
            } else {
                println!("No files need reorganization in any target");
            }
        }
        return DispatchResult::Exit(0);
    }

    if let Some(s) = spinner {
        let total_moves: usize = plans.iter().map(|p| p.stats.files_to_move).sum();
        let total_shims: usize = plans.iter().map(|p| p.stats.shims_needed).sum();
        s.finish_success(&format!(
            "Generated plan(s): {} target(s), {} moves, {} shims",
            plans.len(),
            total_moves,
            total_shims
        ));
    }

    // Handle output based on flags
    if opts.all {
        // Generate all formats
        let base_path = opts
            .output
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("refactor-plan"));

        let md_path = base_path.with_extension("md");
        let json_path = base_path.with_extension("json");
        let script_path = base_path.with_extension("sh");

        if plans.len() == 1 {
            if let Err(e) = output::output_as_markdown(&plans[0], &md_path) {
                eprintln!("[loct][error] Failed to write markdown: {}", e);
                return DispatchResult::Exit(1);
            }
        } else if let Err(e) = output::output_bundle_as_markdown(&plans, &md_path) {
            eprintln!("[loct][error] Failed to write markdown: {}", e);
            return DispatchResult::Exit(1);
        }

        if plans.len() == 1 {
            if let Err(e) = output::output_as_json(&plans[0], &json_path) {
                eprintln!("[loct][error] Failed to write JSON: {}", e);
                return DispatchResult::Exit(1);
            }
        } else if let Err(e) = output::output_bundle_as_json(&plans, &json_path) {
            eprintln!("[loct][error] Failed to write JSON: {}", e);
            return DispatchResult::Exit(1);
        }

        if plans.len() == 1 {
            if let Err(e) = output::output_as_script(&plans[0], &script_path) {
                eprintln!("[loct][error] Failed to write script: {}", e);
                return DispatchResult::Exit(1);
            }
        } else if let Err(e) = output::output_bundle_as_script(&plans, &script_path) {
            eprintln!("[loct][error] Failed to write script: {}", e);
            return DispatchResult::Exit(1);
        }

        if !global.quiet {
            println!("Generated:");
            println!("  {} (markdown)", md_path.display());
            println!("  {} (json)", json_path.display());
            println!("  {} (script)", script_path.display());
        }

        // Auto-open markdown if not suppressed
        if !opts.no_open {
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("open").arg(&md_path).spawn();
            }
        }
    } else if opts.json || global.json {
        // Output JSON to stdout
        if plans.len() == 1 {
            println!("{}", output::format_as_json(&plans[0]));
        } else {
            println!("{}", output::format_bundle_as_json(&plans));
        }
    } else if opts.script {
        // Output script to stdout (or file)
        if let Some(ref path) = opts.output {
            let result = if plans.len() == 1 {
                output::output_as_script(&plans[0], path)
            } else {
                output::output_bundle_as_script(&plans, path)
            };
            if let Err(e) = result {
                eprintln!("[loct][error] Failed to write script: {}", e);
                return DispatchResult::Exit(1);
            }
            if !global.quiet {
                println!("Script written to: {}", path.display());
            }
        } else if plans.len() == 1 {
            print!("{}", output::format_as_script(&plans[0]));
        } else {
            print!("{}", output::format_bundle_as_script(&plans));
        }
    } else {
        // Default: markdown
        if let Some(ref path) = opts.output {
            let result = if plans.len() == 1 {
                output::output_as_markdown(&plans[0], path)
            } else {
                output::output_bundle_as_markdown(&plans, path)
            };
            if let Err(e) = result {
                eprintln!("[loct][error] Failed to write markdown: {}", e);
                return DispatchResult::Exit(1);
            }
            if !global.quiet {
                println!("Report written to: {}", path.display());
            }

            // Auto-open if not suppressed
            if !opts.no_open {
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("open").arg(path).spawn();
                }
            }
        } else {
            // Print to stdout
            if plans.len() == 1 {
                println!("{}", output::format_as_markdown(&plans[0]));
            } else {
                println!("{}", output::format_bundle_as_markdown(&plans));
            }
        }
    }

    DispatchResult::Exit(0)
}

#[cfg(test)]
mod tests {
    use super::build_audit_json;
    use crate::analyzer::audit_report::AuditFindings;
    use crate::analyzer::dead_parrots::DeadExport;

    fn dead_export(symbol: &str, line: usize) -> DeadExport {
        DeadExport {
            file: "src/lib.rs".into(),
            symbol: symbol.into(),
            line: Some(line),
            confidence: "high".into(),
            reason: "unused export".into(),
            open_url: None,
            is_test: false,
            action: "delete_candidate".to_string(),
            entrypoint: false,
        }
    }

    #[test]
    fn test_audit_json_is_full_by_default() {
        let findings = AuditFindings {
            dead_exports: (0..3)
                .map(|idx| dead_export(&format!("dead_{idx}"), idx + 1))
                .collect(),
            ..AuditFindings::default()
        };

        let json = build_audit_json(&findings, None);
        let items = json["dead_exports"]["items"]
            .as_array()
            .expect("dead export items");

        assert_eq!(items.len(), 3);
        assert!(json["dead_exports"].get("truncated").is_none());
    }

    #[test]
    fn test_audit_json_calls_out_explicit_limit() {
        let findings = AuditFindings {
            dead_exports: (0..3)
                .map(|idx| dead_export(&format!("dead_{idx}"), idx + 1))
                .collect(),
            ..AuditFindings::default()
        };

        let json = build_audit_json(&findings, Some(2));
        let items = json["dead_exports"]["items"]
            .as_array()
            .expect("dead export items");

        assert_eq!(items.len(), 2);
        assert_eq!(json["dead_exports"]["omitted"].as_u64(), Some(1));
        assert_eq!(json["dead_exports"]["truncated"].as_bool(), Some(true));
    }
}
