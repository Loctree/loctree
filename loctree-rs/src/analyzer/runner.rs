use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::args::{ParsedArgs, preset_ignore_symbols};
use crate::config::LoctreeConfig;
use crate::snapshot::{Snapshot, SnapshotMetadata};
use crate::types::OutputMode;

use super::ReportSection;
use super::coverage::{
    CommandUsage, compute_command_gaps_with_confidence, compute_unregistered_handlers,
};
use super::dead_parrots::{
    DeadFilterConfig, analyze_impact, find_dead_exports, find_similar, print_dead_exports,
    print_impact_results, print_similarity_results, print_symbol_results, search_symbol,
};
use super::open_server::{open_in_browser, start_open_server};
use super::output::{GlobalContext, RootArtifacts, process_root_context, write_report};
use super::pipelines::build_pipeline_summary;
use super::root_scan::{ScanConfig, ScanResults, scan_results_from_snapshot, scan_roots};
use super::scan::{opt_globset, python_stdlib};
use crate::analyzer::ast_js::CommandDetectionConfig;

const DEFAULT_EXCLUDE_REPORT_PATTERNS: &[&str] =
    &["**/__tests__/**", "scripts/semgrep-fixtures/**"];

const SCHEMA_NAME: &str = "loctree-json";
const SCHEMA_VERSION: &str = crate::snapshot::SNAPSHOT_SCHEMA_VERSION;

pub fn default_analyzer_exts() -> HashSet<String> {
    [
        "ts",
        "tsx",
        "js",
        "jsx",
        "mjs",
        "cjs",
        "rs",
        "css",
        "scss",
        "sass",
        "less",
        "html",
        "htm",
        "py",
        "svelte",
        "vue",
        "astro",
        "toml",
        "md",
        "markdown",
        "yaml",
        "yml",
        "dart",
        "go", // Lightweight parsers (v0.9.0): shell, make, zig
        "sh",
        "bash",
        "zsh",
        "fish",
        "mk",
        "make",
        "zig",
        "zon",
        // Kotlin/JVM is scan-only for now: include files/LOC/language in
        // structural maps without pretending import/export analysis exists.
        "kt",
        "kts",
        // 2026-05-19 vllm-swift D1 + mlx-swift A1 haki: Swift files were
        // listed as source-code (binary check) but excluded from snapshot
        // scan — slice/find returned "not in snapshot" for `*.swift` files.
        "swift",
        "m",
        "mm",
        "c",
        "cc",
        "cpp",
        "cxx",
        "h",
        "hpp",
        // Apple UI resources are scan-only today, but they must remain
        // slice/focus-visible alongside ObjC legacy sources.
        "storyboard",
        "xib",
        // Ruby is scan-only today, but extensionless `#!/usr/bin/env ruby`
        // entrypoints should still participate in snapshot file/LOC counts.
        "rb",
        // W2-02 scorecard correctness: JVM resource bundles (`.properties`),
        // XML manifests/descriptors, and plain-text notes are literal truth rg
        // sees — they must live in the snapshot universe as scan-only
        // resources or `find --literal/--regex` under-reports versus rg.
        "properties",
        "xml",
        "svg",
        "txt",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

pub fn styles_preset_exts() -> HashSet<String> {
    [
        "css", "scss", "sass", "less", "ts", "tsx", "js", "jsx", "mjs", "cjs",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Print Python race condition indicators
fn print_py_race_indicators(analyses: &[crate::types::FileAnalysis], json: bool) {
    let mut all_indicators: Vec<(&str, &crate::types::PyRaceIndicator)> = Vec::new();

    for analysis in analyses {
        for indicator in &analysis.py_race_indicators {
            all_indicators.push((&analysis.path, indicator));
        }
    }

    if all_indicators.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No Python concurrency race indicators found.");
        }
        return;
    }

    if json {
        let items: Vec<_> = all_indicators
            .iter()
            .map(|(path, ind)| {
                serde_json::json!({
                    "path": path,
                    "line": ind.line,
                    "type": ind.concurrency_type,
                    "pattern": ind.pattern,
                    "risk": ind.risk,
                    "message": ind.message
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&items).unwrap_or_default()
        );
    } else {
        // Group by risk level
        let warnings: Vec<_> = all_indicators
            .iter()
            .filter(|(_, i)| i.risk == "warning")
            .collect();
        let infos: Vec<_> = all_indicators
            .iter()
            .filter(|(_, i)| i.risk == "info")
            .collect();

        println!("Python Concurrency Race Indicators");
        println!("===================================\n");

        if !warnings.is_empty() {
            println!("[!] WARNINGS ({}):", warnings.len());
            for (path, ind) in &warnings {
                println!("  {}:{}", path, ind.line);
                println!("    [{}] {}", ind.pattern, ind.message);
                println!();
            }
        }

        if !infos.is_empty() {
            println!("[i] INFO ({}):", infos.len());
            for (path, ind) in &infos {
                println!("  {}:{}", path, ind.line);
                println!("    [{}] {}", ind.pattern, ind.message);
                println!();
            }
        }

        println!(
            "Total: {} indicators ({} warnings, {} info)",
            all_indicators.len(),
            warnings.len(),
            infos.len()
        );
    }
}

pub fn run_import_analyzer(root_list: &[PathBuf], parsed: &ParsedArgs) -> io::Result<()> {
    use std::time::Instant;

    let mut parsed = parsed.clone();
    let scan_started = Instant::now();
    let mut json_results = Vec::new();
    let mut report_sections: Vec<ReportSection> = Vec::new();
    let mut server_handle = None;
    let mut ignore_exact: HashSet<String> = HashSet::new();
    let mut ignore_prefixes: Vec<String> = Vec::new();

    if let Some(preset_name) = parsed.ignore_symbols_preset.as_deref() {
        if let Some(set) = preset_ignore_symbols(preset_name) {
            for s in set {
                if s.ends_with('*') {
                    ignore_prefixes.push(s.trim_end_matches('*').to_string());
                } else {
                    ignore_exact.insert(s);
                }
            }
        } else {
            eprintln!(
                "[loctree][warn] unknown --ignore-symbols-preset '{}', ignoring",
                preset_name
            );
        }
    }

    if let Some(user_syms) = parsed.ignore_symbols.clone() {
        for s in user_syms {
            let lc = s.to_lowercase();
            if lc.ends_with('*') {
                ignore_prefixes.push(lc.trim_end_matches('*').to_string());
            } else {
                ignore_exact.insert(lc);
            }
        }
    }

    let focus_set = opt_globset(&parsed.focus_patterns);

    // Load custom Tauri command macros from .loctree/config.toml
    let loctree_config = root_list
        .first()
        .map(|root| LoctreeConfig::load(root))
        .unwrap_or_default();
    parsed.library_mode = parsed.library_mode || loctree_config.library_mode;
    if parsed.library_mode && parsed.library_example_globs.is_empty() {
        parsed.library_example_globs = loctree_config.library_example_globs.clone();
    }
    let library_mode = parsed.library_mode;
    let command_detection = CommandDetectionConfig::new(
        &loctree_config.tauri.dom_exclusions,
        &loctree_config.tauri.non_invoke_exclusions,
        &loctree_config.tauri.invalid_command_names,
    )
    .with_event_wrappers(&loctree_config.event_wrappers);
    let custom_command_macros = loctree_config.tauri.command_macros;

    let mut exclude_patterns = parsed.exclude_report_patterns.clone();
    exclude_patterns.extend(
        DEFAULT_EXCLUDE_REPORT_PATTERNS
            .iter()
            .map(|p| p.to_string()),
    );
    let exclude_set = opt_globset(&exclude_patterns);

    let editor_cfg = super::open_server::EditorConfig::from_args(
        parsed.editor_kind.clone(),
        parsed.editor_cmd.clone(),
    );

    // Only generate HTML when explicitly requested or serving; avoid auto-opening during tests/builds.
    let auto_report_path = if parsed.serve || parsed.report_path.is_some() {
        parsed.report_path.clone().or_else(|| {
            root_list
                .first()
                .map(|root| Snapshot::artifacts_dir(root).join("report.html"))
        })
    } else {
        None
    };

    if parsed.serve {
        eprintln!(
            "[loctree][warn] `--serve` will move to `loct report --serve`; please prefer the report subcommand (backwards compatible for now)"
        );
        if let Some((base, handle)) = start_open_server(
            root_list.to_vec(),
            editor_cfg.clone(),
            auto_report_path.clone(),
            parsed.serve_port,
        ) {
            server_handle = Some(handle);
            eprintln!("[loctree] local open server at {}", base);
        } else {
            eprintln!("[loctree][warn] could not start open server; continue without --serve");
        }
    }

    let py_stdlib = python_stdlib();

    let base_extensions = parsed.extensions.clone().or_else(|| {
        if parsed.styles_preset {
            Some(styles_preset_exts())
        } else {
            Some(default_analyzer_exts())
        }
    });

    // Try to use snapshot if available (scan once, analyze many)
    // Skip snapshot for modes that need fresh data or special handling:
    // - --symbol: requires reading file contents for text search
    // - --circular: needs complete edges for cycle detection
    // - --graph: needs complete edges for visualization
    // - --dead: needs imports/exports from scanned directory, not parent snapshot
    // - --json/--jsonl/--sarif: output mode not preserved in snapshot Options
    let needs_fresh_scan = parsed.symbol.is_some()
        || parsed.circular
        || parsed.graph
        || parsed.dead_exports
        || matches!(parsed.output, OutputMode::Json | OutputMode::Jsonl)
        || parsed.sarif;
    let use_snapshot = !needs_fresh_scan;
    let scan_results = if use_snapshot {
        if let Some(root) = root_list.first() {
            if let Some(loctree_root) = Snapshot::find_loctree_root(root) {
                match Snapshot::load(&loctree_root) {
                    Ok(snapshot) => {
                        if parsed.verbose {
                            eprintln!(
                                "[loctree] Using snapshot from {} ({} files)",
                                loctree_root.display(),
                                snapshot.files.len()
                            );
                        }
                        scan_results_from_snapshot(&snapshot)
                    }
                    Err(e) => {
                        if parsed.verbose {
                            eprintln!("[loctree] Could not load snapshot: {}, scanning fresh", e);
                        }
                        scan_roots(ScanConfig {
                            roots: root_list,
                            parsed: &parsed,
                            extensions: base_extensions.clone(),
                            focus_set: &focus_set,
                            exclude_set: &exclude_set,
                            ignore_exact: ignore_exact.clone(),
                            ignore_prefixes: ignore_prefixes.clone(),
                            py_stdlib: &py_stdlib,
                            cached_analyses: None,
                            collect_edges: parsed.graph
                                || parsed.impact.is_some()
                                || parsed.circular,
                            custom_command_macros: &custom_command_macros,
                            command_detection: command_detection.clone(),
                        })?
                    }
                }
            } else {
                // No .loctree directory found, scan fresh
                scan_roots(ScanConfig {
                    roots: root_list,
                    parsed: &parsed,
                    extensions: base_extensions.clone(),
                    focus_set: &focus_set,
                    exclude_set: &exclude_set,
                    ignore_exact: ignore_exact.clone(),
                    ignore_prefixes: ignore_prefixes.clone(),
                    py_stdlib: &py_stdlib,
                    cached_analyses: None,
                    collect_edges: parsed.graph || parsed.impact.is_some() || parsed.circular,
                    custom_command_macros: &custom_command_macros,
                    command_detection: command_detection.clone(),
                })?
            }
        } else {
            // No roots provided
            scan_roots(ScanConfig {
                roots: root_list,
                parsed: &parsed,
                extensions: base_extensions.clone(),
                focus_set: &focus_set,
                exclude_set: &exclude_set,
                ignore_exact: ignore_exact.clone(),
                ignore_prefixes: ignore_prefixes.clone(),
                py_stdlib: &py_stdlib,
                cached_analyses: None,
                collect_edges: parsed.graph || parsed.impact.is_some() || parsed.circular,
                custom_command_macros: &custom_command_macros,
                command_detection: command_detection.clone(),
            })?
        }
    } else {
        // --symbol requires reading files, skip snapshot
        scan_roots(ScanConfig {
            roots: root_list,
            parsed: &parsed,
            extensions: base_extensions,
            focus_set: &focus_set,
            exclude_set: &exclude_set,
            ignore_exact,
            ignore_prefixes,
            py_stdlib: &py_stdlib,
            cached_analyses: None,
            collect_edges: parsed.graph || parsed.impact.is_some() || parsed.circular,
            custom_command_macros: &custom_command_macros,
            command_detection,
        })?
    };
    if parsed.auto_outputs {
        let snapshot_root = crate::snapshot::resolve_snapshot_root(root_list);

        match crate::snapshot::write_auto_artifacts(
            &snapshot_root,
            root_list,
            &scan_results,
            &parsed,
            None,
            None,
        ) {
            Ok(paths) => {
                if !paths.is_empty() {
                    println!(
                        "Artifacts saved under {}:",
                        crate::snapshot::Snapshot::artifacts_dir(&snapshot_root).display()
                    );
                    for p in paths {
                        println!("  - {}", p);
                    }
                }
            }
            Err(err) => {
                eprintln!("[loctree][warn] failed to write auto artifacts: {}", err);
            }
        }
    }

    let ScanResults {
        contexts,
        global_fe_commands,
        global_be_commands,
        global_fe_payloads,
        global_be_payloads,
        global_analyses,
        ..
    } = scan_results;
    #[cfg(any(
        feature = "deep-index",
        all(target_os = "macos", feature = "deep-index-macos")
    ))]
    let deep_symbol_graph = {
        let mut graph = crate::symbols::SymbolGraph::new();
        #[cfg(feature = "deep-index")]
        if let Some(scip_graph) = crate::analyzer::scip::import_indexes(root_list) {
            crate::analyzer::scip::merge_graphs(&mut graph, scip_graph);
        }
        #[cfg(all(target_os = "macos", feature = "deep-index-macos"))]
        match crate::analyzer::indexstore::ingest_roots(root_list) {
            Ok(Some(ingest)) => {
                if parsed.verbose {
                    eprintln!(
                        "[loctree][indexstore] detected and imported {} store(s) for analysis snapshot context",
                        ingest.stores.len()
                    );
                }
                crate::analyzer::indexstore::merge_into_graph(&mut graph, ingest.graph);
            }
            Ok(None) => {
                if parsed.verbose {
                    let stores = crate::analyzer::indexstore::discover_stores(root_list);
                    if stores.is_empty() {
                        eprintln!(
                            "[loctree][indexstore] no existing index store detected for this analysis run"
                        );
                    } else {
                        eprintln!(
                            "[loctree][indexstore] detected {} store(s) but no dump helper; skipping precise import",
                            stores.len()
                        );
                    }
                }
            }
            Err(err) => eprintln!("[loctree][warn] IndexStore import skipped: {err}"),
        }
        if graph.is_empty() { None } else { Some(graph) }
    };

    let mut dead_ok_globs: Vec<String> = parsed
        .root_list
        .iter()
        .flat_map(|root| crate::fs_utils::load_loctignore_dead_ok_globs(root))
        .collect();
    dead_ok_globs.sort();
    dead_ok_globs.dedup();

    if let Some(sym) = &parsed.symbol {
        let result = search_symbol(sym, &global_analyses);
        print_symbol_results(sym, &result, matches!(parsed.output, OutputMode::Json));
        return Ok(());
    }

    if let Some(target_path) = &parsed.impact {
        if let Some(result) = analyze_impact(target_path, &global_analyses, &contexts) {
            print_impact_results(
                target_path,
                &result,
                matches!(parsed.output, OutputMode::Json),
            );
        } else {
            eprintln!("Target file not found in scan results: {}", target_path);
        }
        return Ok(());
    }

    if let Some(query) = &parsed.check_sim {
        let candidates = find_similar(query, &global_analyses);
        print_similarity_results(
            query,
            &candidates,
            matches!(parsed.output, OutputMode::Json),
        );
        return Ok(());
    }

    if parsed.dead_exports {
        let high_confidence = parsed.dead_confidence.as_deref() == Some("high");
        let dead_exports = find_dead_exports(
            &global_analyses,
            high_confidence,
            None,
            DeadFilterConfig {
                include_tests: parsed.with_tests,
                include_helpers: parsed.with_helpers,
                library_mode,
                example_globs: parsed.library_example_globs.clone(),
                python_library_mode: parsed.python_library,
                include_ambient: false,
                include_dynamic: false,
                dead_ok_globs: dead_ok_globs.clone(),
            },
        );
        // Apply --focus and --exclude-report filters to dead exports
        let filtered_dead: Vec<_> = dead_exports
            .into_iter()
            .filter(|d| {
                let path = std::path::PathBuf::from(&d.file);
                // Check focus_set: if set, file must match
                let passes_focus = focus_set
                    .as_ref()
                    .map(|set| set.is_match(&path))
                    .unwrap_or(true);
                // Check exclude_set: if set, file must NOT match
                let passes_exclude = exclude_set
                    .as_ref()
                    .map(|set| !set.is_match(&path))
                    .unwrap_or(true);
                passes_focus && passes_exclude
            })
            .collect();
        print_dead_exports(
            &filtered_dead,
            parsed.output,
            high_confidence,
            parsed.top_dead_symbols,
        );
        return Ok(());
    }

    // Collect all graph edges for cycle detection
    let all_graph_edges: Vec<(String, String, String)> = contexts
        .iter()
        .flat_map(|ctx| ctx.graph_edges.clone())
        .collect();

    if parsed.circular {
        let (cycles, lazy_cycles) = super::cycles::find_cycles_with_lazy(&all_graph_edges);
        super::cycles::print_cycles(&cycles, matches!(parsed.output, OutputMode::Json));
        if !lazy_cycles.is_empty() && !matches!(parsed.output, OutputMode::Json) {
            println!("\nLazy circular imports (info):");
            println!(
                "  These come from imports inside functions/methods; usually safe, but check init order if relevant."
            );
            super::cycles::print_cycles(&lazy_cycles, false);
            let lazy_edges: Vec<_> = all_graph_edges
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
        return Ok(());
    }

    if parsed.entrypoints {
        let eps = super::entrypoints::find_entrypoints(&global_analyses);
        super::entrypoints::print_entrypoints(&eps, matches!(parsed.output, OutputMode::Json));
        return Ok(());
    }

    if parsed.py_races {
        print_py_race_indicators(&global_analyses, matches!(parsed.output, OutputMode::Json));
        return Ok(());
    }

    // Build a set of registered Tauri handler function names from all analyzed files.
    let registered_impls: std::collections::HashSet<String> = global_analyses
        .iter()
        .flat_map(|a| a.tauri_registered_handlers.iter().cloned())
        .collect();

    // Filter backend commands down to those whose implementation symbol is actually
    // registered via tauri::generate_handler![...]. This prevents unregistered
    // handlers from counting as "available" when computing FE→BE gaps.
    let mut global_be_registered_commands: CommandUsage = std::collections::HashMap::new();
    for (name, locs) in &global_be_commands {
        for (path, line, impl_name) in locs {
            if registered_impls.is_empty() || registered_impls.contains(impl_name) {
                global_be_registered_commands
                    .entry(name.clone())
                    .or_default()
                    .push((path.clone(), *line, impl_name.clone()));
            }
        }
    }
    // Cross-root command gaps (fixes multi-root FP for missing/unused handlers)
    // Pass analyses for confidence scoring on unused handlers
    let (global_missing_handlers, global_unused_handlers) = compute_command_gaps_with_confidence(
        &global_fe_commands,
        &global_be_registered_commands,
        &focus_set,
        &exclude_set,
        &global_analyses,
    );

    // Handlers that have #[tauri::command] but are never registered via generate_handler!.
    let global_unregistered_handlers = compute_unregistered_handlers(
        &global_be_commands,
        &registered_impls,
        &focus_set,
        &exclude_set,
    );

    let pipeline_summary = build_pipeline_summary(
        &global_analyses,
        &focus_set,
        &exclude_set,
        &global_fe_commands,
        &global_be_commands,
        &global_fe_payloads,
        &global_be_payloads,
    );
    let git_ctx = Snapshot::current_git_context();

    // Handle SARIF output
    if parsed.sarif {
        // Collect duplicate exports from all contexts
        let all_ranked_dups: Vec<_> = contexts
            .iter()
            .flat_map(|ctx| ctx.filtered_ranked.clone())
            .collect();

        // Get dead exports
        let high_confidence = parsed.dead_confidence.as_deref() == Some("high");
        let dead_exports = find_dead_exports(
            &global_analyses,
            high_confidence,
            None,
            DeadFilterConfig {
                include_tests: parsed.with_tests,
                include_helpers: parsed.with_helpers,
                library_mode,
                example_globs: parsed.library_example_globs.clone(),
                python_library_mode: parsed.python_library,
                include_ambient: false,
                include_dynamic: false,
                dead_ok_globs: dead_ok_globs.clone(),
            },
        );

        // Get circular imports
        let (circular_imports, _lazy) = super::cycles::find_cycles_with_lazy(&all_graph_edges);

        // Build minimal snapshot for SARIF enrichment (blast radius, consumer count)
        use crate::snapshot::GraphEdge;
        let minimal_snapshot = Snapshot {
            metadata: SnapshotMetadata::default(),
            files: vec![],
            edges: all_graph_edges
                .iter()
                .map(|(from, to, label)| GraphEdge {
                    from: from.clone(),
                    to: to.clone(),
                    label: label.clone(),
                })
                .collect(),
            export_index: Default::default(),
            command_bridges: vec![],
            event_bridges: vec![],
            barrels: vec![],
            semantic_facts: None,
            symbol_graph: {
                #[cfg(any(
                    feature = "deep-index",
                    all(target_os = "macos", feature = "deep-index-macos")
                ))]
                {
                    deep_symbol_graph.clone()
                }
                #[cfg(not(any(
                    feature = "deep-index",
                    all(target_os = "macos", feature = "deep-index-macos")
                )))]
                {
                    None
                }
            },
        };

        super::sarif::print_sarif(super::sarif::SarifInputs {
            duplicate_exports: &all_ranked_dups,
            missing_handlers: &global_missing_handlers,
            unused_handlers: &global_unused_handlers,
            dead_exports: &dead_exports,
            circular_imports: &circular_imports,
            pipeline_summary: &pipeline_summary,
            snapshot: Some(&minimal_snapshot),
        })
        .map_err(|err| io::Error::other(format!("Failed to serialize SARIF: {err}")))?;
        return Ok(());
    }

    for (idx, ctx) in contexts.into_iter().enumerate() {
        let RootArtifacts {
            json_items,
            report_section,
        } = process_root_context(
            idx,
            ctx,
            &parsed,
            &GlobalContext {
                fe_commands: &global_fe_commands,
                be_commands: &global_be_commands,
                missing_handlers: &global_missing_handlers,
                unregistered_handlers: &global_unregistered_handlers,
                unused_handlers: &global_unused_handlers,
                pipeline_summary: &pipeline_summary,
                git: Some(&git_ctx),
                schema_name: SCHEMA_NAME,
                schema_version: SCHEMA_VERSION,
                analyses: &global_analyses,
            },
        );
        json_results.extend(json_items);
        if let Some(section) = report_section {
            report_sections.push(section);
        }
    }

    if matches!(parsed.output, OutputMode::Json) {
        let payload = if json_results.len() == 1 {
            serde_json::to_string_pretty(&json_results[0])
        } else {
            serde_json::to_string_pretty(&json_results)
        }
        .map_err(io::Error::other)?;
        if let Some(path) = parsed.json_output_path.as_ref() {
            if path.exists() && path.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("--json-out points to a directory: {}", path.display()),
                ));
            }
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            if path.exists() {
                eprintln!(
                    "[loctree][warn] JSON output will overwrite existing file: {}",
                    path.display()
                );
            }
            fs::write(path, payload.as_bytes()).map_err(|err| {
                io::Error::other(format!(
                    "failed to write JSON to {}: {}",
                    path.display(),
                    err
                ))
            })?;
            if parsed.verbose {
                eprintln!("[loctree][debug] wrote JSON to {}", path.display());
            } else {
                eprintln!("[loctree] JSON written to {}", path.display());
            }
        } else {
            println!("{}", payload);
        }
    }

    if (parsed.serve || parsed.report_path.is_some())
        && let Some(report_path) = auto_report_path.as_ref()
    {
        write_report(report_path, &report_sections, parsed.verbose)?;
        open_in_browser(report_path);
    }

    if parsed.serve && !parsed.serve_once {
        use std::io::Read;
        eprintln!("[loctree] --serve: Press Enter (Ctrl+C to interrupt) to stop the server");
        let _ = std::io::stdin().read(&mut [0u8]).ok();
    }
    drop(server_handle);

    // Check --fail-on-* flags and return appropriate exit code
    let mut fail_reasons: Vec<String> = Vec::new();

    if parsed.fail_on_missing_handlers && !global_missing_handlers.is_empty() {
        let examples: Vec<String> = global_missing_handlers
            .iter()
            .take(3)
            .map(|h| {
                let loc = h
                    .locations
                    .first()
                    .map(|(path, line)| format!(" ({}:{})", path, line))
                    .unwrap_or_default();
                format!("{}{}", h.name, loc)
            })
            .collect();
        let more = if global_missing_handlers.len() > 3 {
            format!(" (+{} more)", global_missing_handlers.len() - 3)
        } else {
            String::new()
        };
        fail_reasons.push(format!(
            "{} missing handler(s): {}{}",
            global_missing_handlers.len(),
            examples.join(", "),
            more
        ));
    }

    if parsed.fail_on_ghost_events {
        let ghost_count = pipeline_summary
            .get("events")
            .and_then(|e| e.get("ghostCount"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let orphan_count = pipeline_summary
            .get("events")
            .and_then(|e| e.get("orphanCount"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if ghost_count > 0 {
            fail_reasons.push(format!(
                "{} ghost event(s) (emitted but no listener)",
                ghost_count
            ));
        }
        if orphan_count > 0 {
            fail_reasons.push(format!("{} orphan listener(s) (no emitter)", orphan_count));
        }
    }

    if parsed.fail_on_races {
        let race_count = pipeline_summary
            .get("events")
            .and_then(|e| e.get("races"))
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        if race_count > 0 {
            fail_reasons.push(format!("{} potential race(s) detected", race_count));
        }
    }

    // Threshold-based CI policy checks
    if let Some(max_dead) = parsed.max_dead {
        let high_confidence = parsed.dead_confidence.as_deref() == Some("high");
        let dead_exports = super::dead_parrots::find_dead_exports(
            &global_analyses,
            high_confidence,
            None,
            DeadFilterConfig {
                include_tests: parsed.with_tests,
                include_helpers: parsed.with_helpers,
                library_mode,
                example_globs: parsed.library_example_globs.clone(),
                python_library_mode: parsed.python_library,
                include_ambient: false,
                include_dynamic: false,
                dead_ok_globs,
            },
        );
        let dead_count = dead_exports.len();
        if dead_count > max_dead {
            fail_reasons.push(format!(
                "{} dead export(s) exceed threshold of {} (--max-dead)",
                dead_count, max_dead
            ));
        }
    }

    if let Some(max_cycles) = parsed.max_cycles {
        let (cycles, _) = super::cycles::find_cycles_with_lazy(&all_graph_edges);
        let cycle_count = cycles.len();
        if cycle_count > max_cycles {
            fail_reasons.push(format!(
                "{} circular import(s) exceed threshold of {} (--max-cycles)",
                cycle_count, max_cycles
            ));
        }
    }

    if !fail_reasons.is_empty() {
        eprintln!("[loctree][fail] {}", fail_reasons.join("; "));
        return Err(io::Error::other(format!(
            "Pipeline check failed: {}",
            fail_reasons.join("; ")
        )));
    }

    // Human-friendly summary for the default scan (avoid empty output).
    if matches!(parsed.output, OutputMode::Human) && !parsed.sarif {
        let elapsed = scan_started.elapsed();
        let mut langs: HashSet<String> = HashSet::new();
        for fa in &global_analyses {
            if !fa.language.is_empty() {
                langs.insert(fa.language.clone());
            }
        }
        eprintln!(
            "[loctree] Summary: files {}, missing handlers {}, unused handlers {}, languages [{}], elapsed {:.2?}",
            global_analyses.len(),
            global_missing_handlers.len(),
            global_unused_handlers.len(),
            langs.iter().cloned().collect::<Vec<_>>().join(","),
            elapsed
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::default_analyzer_exts;

    /// Regression for the 2026-05-19 vllm-swift D1 + mlx-swift A1 haki:
    /// `*.swift` files were treated as source-code by the binary-detection
    /// list but excluded from the snapshot-scan extension set, so `slice`
    /// and `find` returned "not in snapshot" for every Swift file.
    #[test]
    fn default_analyzer_exts_includes_c_family() {
        let exts = default_analyzer_exts();
        for ext in ["swift", "m", "mm", "c", "cc", "cpp", "cxx", "h", "hpp"] {
            assert!(
                exts.contains(ext),
                "{} must be in the default analyzer extension set; got {:?}",
                ext,
                exts
            );
        }
    }

    #[test]
    fn default_analyzer_exts_includes_apple_ui_resources() {
        let exts = default_analyzer_exts();
        for ext in ["storyboard", "xib"] {
            assert!(
                exts.contains(ext),
                "{ext} must be in default analyzer extensions so legacy Apple UI resources land in tree/focus/slice; got {:?}",
                exts
            );
        }
    }

    #[test]
    fn default_analyzer_exts_includes_kotlin_scan_only_surface() {
        let exts = default_analyzer_exts();
        for ext in ["kt", "kts"] {
            assert!(
                exts.contains(ext),
                "{ext} must be in default analyzer extensions so JetBrains/Kotlin plugin sources land in tree/focus/slice; got {:?}",
                exts
            );
        }
    }

    /// Regression for recurring loctree-feedback entries where `Cargo.toml`,
    /// workflow YAML, and repo-local markdown plans were missing from the
    /// snapshot, forcing shell fallback for docs/manifest truth.
    #[test]
    fn default_analyzer_exts_includes_manifest_docs_and_workflows() {
        let exts = default_analyzer_exts();
        for ext in ["toml", "md", "markdown", "yaml", "yml"] {
            assert!(
                exts.contains(ext),
                "{ext} must be in default analyzer extensions so manifests/docs/workflows land in the snapshot; got {:?}",
                exts
            );
        }
    }

    /// Regression for the W2-02 scorecard correctness loss: rg saw
    /// `Occurrence[A-Za-z]+` hits in a JetBrains message bundle
    /// (`LoctreeBundle.properties`) and plugin manifest (`loctree-lsp.xml`)
    /// that never entered the snapshot universe, so `find --literal/--regex`
    /// under-reported versus rg.
    #[test]
    fn default_analyzer_exts_includes_scan_only_text_resources() {
        let exts = default_analyzer_exts();
        for ext in ["properties", "xml", "svg", "txt"] {
            assert!(
                exts.contains(ext),
                "{ext} must be in default analyzer extensions so text resources land in the literal/regex truth universe; got {:?}",
                exts
            );
        }
    }

    /// Regression for the 2026-05-31 design-system hook: Rust/Leptos repos
    /// can carry product-critical styling in CSS-family assets even when Rust
    /// is the dominant language.
    #[test]
    fn default_analyzer_exts_includes_design_assets() {
        let exts = default_analyzer_exts();
        for ext in ["css", "scss", "sass", "less", "html", "htm"] {
            assert!(
                exts.contains(ext),
                "{ext} must be in default analyzer extensions so design assets land in the snapshot; got {:?}",
                exts
            );
        }
    }
}
