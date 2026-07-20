//! Shared CLI entry point for both `loct` and `loctree` binaries.
//!
//! This module contains all the dispatch logic and mode handlers so that
//! both binaries can share a single implementation. The `loctree` binary
//! adds a deprecation warning and delegates here.

use std::fs;
use std::path::PathBuf;

use crate::args::{ParsedArgs, SearchQueryMode, parse_args};
use crate::cli::{self, Command, DispatchResult};
use crate::config::LoctreeConfig;
use crate::types::{GitSubcommand, Mode};
use crate::{OutputMode, analyzer, detect, diff, fs_utils, git, slicer, snapshot, tree};

/// Options controlling binary-specific behavior.
pub struct EntryOptions {
    /// Name shown in `--version` output (e.g. "loctree" or "loct").
    pub binary_name: &'static str,
    /// If true, show deprecation banner before dispatch.
    pub deprecated: bool,
    /// If true, show the animated startup banner on `Init` mode.
    pub show_banner: bool,
    /// Usage text for `--help`.
    pub usage: &'static str,
}

/// Run the CLI with the given options. This is the shared main() body.
pub fn run(opts: &EntryOptions) -> std::io::Result<()> {
    // SAFETY: This is the shared CLI trust boundary for both `loct` and `loctree` binaries.
    // `args()` returns raw process arguments straight from the OS — by definition the
    // entry point of user input. Args are routed to `cli::parse_command` / `parse_args`
    // for structured parsing; any path/identifier derived from them MUST be validated
    // before privileged use (see `semantic::io::validate_and_canonicalize`). The legacy
    // parser already collected via `args_os()` in `args.rs`; this lossy `args()` view is
    // only fed to flag detection and the new subcommand parser (string-only).
    // Rule suppression is enforced at file scope via `.semgrepignore` (CLI ENTRY POINTS).
    let raw_args: Vec<String> = std::env::args().skip(1).collect();

    // Preserve legacy full help output expected by CI/tests
    if raw_args.iter().any(|a| a == "--help-full") {
        println!("{}", Command::format_help_full());
        return Ok(());
    }

    // Try new subcommand parser first
    let mut parsed = match cli::parse_command(&raw_args) {
        Ok(Some(parsed_cmd)) => {
            // New syntax detected - dispatch through new system
            match cli::dispatch_command(&parsed_cmd) {
                DispatchResult::ShowHelp => {
                    println!("{}", Command::format_help());
                    return Ok(());
                }
                DispatchResult::ShowLegacyHelp => {
                    println!("{}", Command::format_legacy_help());
                    return Ok(());
                }
                DispatchResult::ShowVersion => {
                    println!("{} {}", opts.binary_name, crate::BUILD_VERSION);
                    return Ok(());
                }
                DispatchResult::Exit(code) => {
                    std::process::exit(code);
                }
                DispatchResult::Continue(args) => *args,
            }
        }
        Ok(None) => {
            // Legacy syntax - fall back to old parser
            match parse_args() {
                Ok(args) => args,
                Err(err) => {
                    eprintln!("{}", err);
                    std::process::exit(1);
                }
            }
        }
        Err(err) => {
            eprintln!("{}", err);
            std::process::exit(1);
        }
    };

    // Auto-detect stack if no explicit extensions provided
    if !parsed.root_list.is_empty() {
        let mut library_mode = parsed.library_mode;
        detect::apply_detected_stack(
            &parsed.root_list[0],
            &mut parsed.extensions,
            &mut parsed.ignore_patterns,
            &mut parsed.tauri_preset,
            &mut library_mode,
            &mut parsed.py_roots,
            parsed.verbose,
        );
        parsed.library_mode = library_mode;

        // Load .loctreeignore from root (if exists)
        let loctreeignore_patterns = fs_utils::load_loctreeignore(&parsed.root_list[0]);
        if !loctreeignore_patterns.is_empty() {
            if parsed.verbose {
                eprintln!(
                    "[loct] loaded {} patterns from .loctignore",
                    loctreeignore_patterns.len()
                );
            }
            parsed.ignore_patterns.extend(loctreeignore_patterns);
        }
    }

    // Handle help/version for legacy path (new path handles these above)
    if parsed.show_help {
        println!("{}", opts.usage);
        return Ok(());
    }

    if parsed.show_help_full {
        println!("{}", Command::format_help_full());
        return Ok(());
    }

    if parsed.show_version {
        println!("{} {}", opts.binary_name, crate::BUILD_VERSION);
        return Ok(());
    }

    if parsed.max_depth.is_some() && parsed.max_depth.unwrap_or(0) == usize::MAX {
        eprintln!("Invalid max depth");
        std::process::exit(1);
    }

    let mut root_list: Vec<PathBuf> = Vec::new();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for root in parsed.root_list.iter() {
        if !root.is_dir() {
            let raw = if root.as_os_str().is_empty() {
                "<empty>".to_string()
            } else {
                root.display().to_string()
            };
            eprintln!(
                "Root \"{}\" (cwd: {}) is not a directory",
                raw,
                cwd.display()
            );
            std::process::exit(1);
        }
        root_list.push(root.canonicalize().unwrap_or_else(|_| root.clone()));
    }

    match parsed.mode {
        Mode::AnalyzeImports => analyzer::run_import_analyzer(&root_list, &parsed)?,
        Mode::Tree => tree::run_tree(&root_list, &parsed)?,
        Mode::Init => {
            if opts.show_banner {
                print_animated_banner();
            }
            snapshot::run_init(&root_list, &parsed)?
        }
        Mode::Slice => {
            let target = parsed.slice_target.as_ref().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "slice requires a target file path, e.g.: loct slice src/foo.ts",
                )
            })?;
            let root = root_list
                .first()
                .cloned()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

            // Check if target is a directory
            let target_path = root.join(target);
            if target_path.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "'{}' is a directory. slice works on files.\nFor directories use: loct focus {}",
                        target, target
                    ),
                ));
            }

            let json_output = matches!(parsed.output, OutputMode::Json);
            slicer::run_slice(&root, target, parsed.slice_consumers, json_output, &parsed)?;
        }
        Mode::Trace => {
            let handler_name = parsed.trace_handler.as_ref().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "trace requires a handler name, e.g.: loct trace toggle_assistant",
                )
            })?;
            run_trace(&root_list, handler_name, &parsed)?;
        }
        Mode::ForAi => {
            run_for_ai(&root_list, &parsed)?;
        }
        Mode::Findings => {
            run_findings(&root_list, &parsed, false)?;
        }
        Mode::Summary => {
            run_findings(&root_list, &parsed, true)?;
        }
        Mode::Git(ref subcommand) => {
            run_git(subcommand, &parsed)?;
        }
        Mode::Search => {
            run_search(&root_list, &parsed)?;
        }
    }

    Ok(())
}

/// Micro-animation for loct startup.
/// Subtle flex - blink and you'll miss it, but those who see it know.
fn print_animated_banner() {
    use std::io::Write;
    use std::thread;
    use std::time::Duration;

    // Skip in non-TTY (CI, pipes)
    if !console::Term::stderr().is_term() {
        return;
    }

    const RESET: &str = "\x1b[0m";
    const BOLD: &str = "\x1b[1m";
    const DIM: &str = "\x1b[2m";
    const CYAN: &str = "\x1b[96m";
    const WHITE: &str = "\x1b[97m";

    // Phase 1: Letters materialize one by one with glow
    let letters = ['l', 'o', 'c', 't'];
    eprint!("\r");

    for (i, ch) in letters.iter().enumerate() {
        eprint!("{}{}{}", BOLD, WHITE, ch);
        let _ = std::io::stderr().flush();
        thread::sleep(Duration::from_millis(35));

        eprint!("\r{}{}", BOLD, CYAN);
        for c in &letters[..=i] {
            eprint!("{}", c);
        }
        let _ = std::io::stderr().flush();
        thread::sleep(Duration::from_millis(25));
    }

    // Phase 2: Dot pulse
    let dots = ["", ".", "..", "...", "..", ".", ""];
    for dot in dots {
        eprint!("\r{}{}loct{}{}", BOLD, CYAN, DIM, dot);
        eprint!("   ");
        eprint!("\r{}{}loct{}{}", BOLD, CYAN, DIM, dot);
        let _ = std::io::stderr().flush();
        thread::sleep(Duration::from_millis(40));
    }

    // Phase 3: Final form
    eprint!("\r{}{}loct{} ▸{}", BOLD, CYAN, RESET, RESET);
    eprintln!();
}

fn run_trace(
    root_list: &[PathBuf],
    handler_name: &str,
    parsed: &ParsedArgs,
) -> std::io::Result<()> {
    use analyzer::root_scan::{ScanConfig, ScanResults, scan_roots};
    use analyzer::trace::{print_trace_human, print_trace_json, trace_handler};
    use std::collections::HashSet;

    let extensions = parsed.extensions.clone().or_else(|| {
        Some(
            ["ts", "tsx", "js", "jsx", "mjs", "cjs", "rs", "css", "py"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        )
    });

    let py_stdlib = analyzer::scan::python_stdlib();

    let loctree_config = root_list
        .first()
        .map(|root| LoctreeConfig::load(root))
        .unwrap_or_default();
    let command_detection = analyzer::ast_js::CommandDetectionConfig::new(
        &loctree_config.tauri.dom_exclusions,
        &loctree_config.tauri.non_invoke_exclusions,
        &loctree_config.tauri.invalid_command_names,
    )
    .with_event_wrappers(&loctree_config.event_wrappers);
    let custom_command_macros = loctree_config.tauri.command_macros;

    let scan_results = scan_roots(ScanConfig {
        roots: root_list,
        parsed,
        extensions,
        focus_set: &None,
        exclude_set: &None,
        ignore_exact: HashSet::new(),
        ignore_prefixes: Vec::new(),
        py_stdlib: &py_stdlib,
        cached_analyses: None,
        collect_edges: false,
        custom_command_macros: &custom_command_macros,
        command_detection,
    })?;

    let ScanResults {
        global_fe_commands,
        global_be_commands,
        global_analyses,
        ..
    } = scan_results;

    let registered_impls: HashSet<String> = global_analyses
        .iter()
        .flat_map(|a| a.tauri_registered_handlers.iter().cloned())
        .collect();

    let result = trace_handler(
        handler_name,
        &global_analyses,
        &global_fe_commands,
        &global_be_commands,
        &registered_impls,
    );

    if matches!(parsed.output, OutputMode::Json) {
        print_trace_json(&result);
    } else {
        print_trace_human(&result);
    }

    Ok(())
}

fn run_for_ai(root_list: &[PathBuf], parsed: &ParsedArgs) -> std::io::Result<()> {
    use analyzer::coverage::{compute_command_gaps_with_confidence, compute_unregistered_handlers};
    use analyzer::for_ai::{generate_for_ai_report, print_for_ai_json};
    use analyzer::output::{GlobalContext, process_root_context};
    use analyzer::root_scan::{ScanConfig, ScanResults, scan_roots};
    use analyzer::runner::default_analyzer_exts;
    use analyzer::scan::{opt_globset, python_stdlib};
    use std::collections::HashSet;

    let extensions = parsed
        .extensions
        .clone()
        .or_else(|| Some(default_analyzer_exts()));

    let py_stdlib = python_stdlib();
    let focus_set = opt_globset(&parsed.focus_patterns);
    let exclude_set = opt_globset(&parsed.exclude_report_patterns);

    let loctree_config = root_list
        .first()
        .map(|root| LoctreeConfig::load(root))
        .unwrap_or_default();
    let command_detection = analyzer::ast_js::CommandDetectionConfig::new(
        &loctree_config.tauri.dom_exclusions,
        &loctree_config.tauri.non_invoke_exclusions,
        &loctree_config.tauri.invalid_command_names,
    )
    .with_event_wrappers(&loctree_config.event_wrappers);
    let custom_command_macros = loctree_config.tauri.command_macros;

    let scan_results = scan_roots(ScanConfig {
        roots: root_list,
        parsed,
        extensions,
        focus_set: &focus_set,
        exclude_set: &exclude_set,
        ignore_exact: HashSet::new(),
        ignore_prefixes: Vec::new(),
        py_stdlib: &py_stdlib,
        cached_analyses: None,
        collect_edges: true,
        custom_command_macros: &custom_command_macros,
        command_detection,
    })?;

    let ScanResults {
        contexts,
        global_fe_commands,
        global_be_commands,
        global_analyses,
        ..
    } = scan_results;

    let registered_impls: HashSet<String> = global_analyses
        .iter()
        .flat_map(|a| a.tauri_registered_handlers.iter().cloned())
        .collect();

    let mut global_be_registered: analyzer::coverage::CommandUsage =
        std::collections::HashMap::new();
    for (name, locs) in &global_be_commands {
        for (path, line, impl_name) in locs {
            if registered_impls.is_empty() || registered_impls.contains(impl_name) {
                global_be_registered.entry(name.clone()).or_default().push((
                    path.clone(),
                    *line,
                    impl_name.clone(),
                ));
            }
        }
    }

    let (global_missing, global_unused) = compute_command_gaps_with_confidence(
        &global_fe_commands,
        &global_be_registered,
        &focus_set,
        &exclude_set,
        &global_analyses,
    );

    let global_unregistered = compute_unregistered_handlers(
        &global_be_commands,
        &registered_impls,
        &focus_set,
        &exclude_set,
    );

    let pipeline_summary = analyzer::pipelines::build_pipeline_summary(
        &global_analyses,
        &focus_set,
        &exclude_set,
        &global_fe_commands,
        &global_be_commands,
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
    );
    let git_ctx = root_list
        .first()
        .map(|root| snapshot::Snapshot::git_context_for(root))
        .unwrap_or_else(snapshot::Snapshot::current_git_context);

    // Build snapshot with edges for accurate barrel_chaos calculation in agent.json
    let ai_snapshot = {
        let mut snap =
            snapshot::Snapshot::new(root_list.iter().map(|p| p.display().to_string()).collect());
        for ctx in &contexts {
            snap.files.extend(ctx.analyses.clone());
            for (from, to, label) in &ctx.graph_edges {
                snap.edges.push(snapshot::GraphEdge {
                    from: from.clone(),
                    to: to.clone(),
                    label: label.clone(),
                });
            }
        }
        let semantic_root = root_list
            .first()
            .cloned()
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        snap.semantic_facts = Some(crate::semantic::compute_semantic_facts(
            &snap.files,
            &semantic_root,
        ));
        snap.finalize_metadata(0);
        snap
    };

    let mut report_sections = Vec::new();
    for (idx, ctx) in contexts.into_iter().enumerate() {
        let artifacts = process_root_context(
            idx,
            ctx,
            parsed,
            &GlobalContext {
                fe_commands: &global_fe_commands,
                be_commands: &global_be_commands,
                missing_handlers: &global_missing,
                unregistered_handlers: &global_unregistered,
                unused_handlers: &global_unused,
                pipeline_summary: &pipeline_summary,
                git: Some(&git_ctx),
                schema_name: "loctree-json",
                schema_version: "1.2.0",
                analyses: &global_analyses,
            },
        );
        if let Some(section) = artifacts.report_section {
            report_sections.push(section);
        }
    }

    let project_root = root_list
        .first()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".to_string());

    let report = generate_for_ai_report(
        &project_root,
        &report_sections,
        &global_analyses,
        Some(&ai_snapshot),
    );

    // Persist agent bundle to disk for single-file consumption
    if parsed.output == OutputMode::Json
        && let Some(root) = root_list.first()
    {
        let agent_path = crate::snapshot::Snapshot::artifacts_dir(root).join("agent.json");
        if let Some(dir) = agent_path.parent() {
            if let Err(e) = fs::create_dir_all(dir) {
                eprintln!("[loct][agent] Failed to create {}: {}", dir.display(), e);
            } else {
                match serde_json::to_vec_pretty(&report) {
                    Ok(data) => {
                        if let Err(e) = fs::write(&agent_path, data) {
                            eprintln!(
                                "[loct][agent] Failed to write {}: {}",
                                agent_path.display(),
                                e
                            );
                        } else {
                            eprintln!("[loct][agent] Bundle saved to {}", agent_path.display());
                        }
                    }
                    Err(e) => eprintln!("[loct][agent] Failed to serialize agent bundle: {e}"),
                }
            }
        }
    }

    // JSONL mode outputs one QuickWin per line for streaming agent consumption
    if parsed.output == OutputMode::Jsonl {
        analyzer::for_ai::print_agent_feed_jsonl(&report);
    } else {
        print_for_ai_json(&report);
    }

    Ok(())
}

/// Output findings.json or summary to stdout
fn run_findings(
    root_list: &[PathBuf],
    parsed: &ParsedArgs,
    summary_only: bool,
) -> std::io::Result<()> {
    use analyzer::findings::{Findings, FindingsConfig};
    use analyzer::root_scan::{ScanConfig, scan_roots};
    use analyzer::runner::default_analyzer_exts;
    use analyzer::scan::{opt_globset, python_stdlib};
    use std::collections::HashSet;

    let extensions = parsed
        .extensions
        .clone()
        .or_else(|| Some(default_analyzer_exts()));

    let py_stdlib = python_stdlib();
    let focus_set = opt_globset(&parsed.focus_patterns);
    let exclude_set = opt_globset(&parsed.exclude_report_patterns);

    let loctree_config = root_list
        .first()
        .map(|root| LoctreeConfig::load(root))
        .unwrap_or_default();
    let command_detection = analyzer::ast_js::CommandDetectionConfig::new(
        &loctree_config.tauri.dom_exclusions,
        &loctree_config.tauri.non_invoke_exclusions,
        &loctree_config.tauri.invalid_command_names,
    )
    .with_event_wrappers(&loctree_config.event_wrappers);
    let custom_command_macros = loctree_config.tauri.command_macros;

    let scan_results = scan_roots(ScanConfig {
        roots: root_list,
        parsed,
        extensions,
        focus_set: &focus_set,
        exclude_set: &exclude_set,
        ignore_exact: HashSet::new(),
        ignore_prefixes: Vec::new(),
        py_stdlib: &py_stdlib,
        cached_analyses: None,
        collect_edges: true,
        custom_command_macros: &custom_command_macros,
        command_detection,
    })?;

    let mut snap =
        snapshot::Snapshot::new(root_list.iter().map(|p| p.display().to_string()).collect());
    for ctx in &scan_results.contexts {
        snap.files.extend(ctx.analyses.clone());
        for (from, to, label) in &ctx.graph_edges {
            snap.edges.push(snapshot::GraphEdge {
                from: from.clone(),
                to: to.clone(),
                label: label.clone(),
            });
        }
    }
    let semantic_root = root_list
        .first()
        .cloned()
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    snap.semantic_facts = Some(crate::semantic::compute_semantic_facts(
        &snap.files,
        &semantic_root,
    ));
    snap.finalize_metadata(0);

    let config = FindingsConfig {
        high_confidence: parsed.dead_confidence.as_deref() == Some("high"),
        library_mode: parsed.library_mode,
        python_library: parsed.python_library,
        example_globs: parsed.library_example_globs.clone(),
    };

    let findings = Findings::produce(&scan_results, &snap, config, None);

    if summary_only {
        let summary = findings.summary_only();
        let json = serde_json::to_string_pretty(&summary)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        println!("{}", json);
    } else {
        let json = findings
            .to_json()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        println!("{}", json);
    }

    Ok(())
}

/// Unified search - aggregates symbol, semantic, and dead code results
fn run_search(root_list: &[PathBuf], parsed: &ParsedArgs) -> std::io::Result<()> {
    use analyzer::search::{SearchResults, print_search_results, run_search as do_search};
    use serde_json::{Value, json};
    use std::collections::{HashMap, HashSet};

    fn search_results_to_json_value(
        results: &SearchResults,
        symbol_only: bool,
        dead_only: bool,
        semantic_only: bool,
    ) -> Value {
        if symbol_only {
            json!({
                "query": results.query,
                "symbol_matches": results.symbol_matches,
                "param_matches": results.param_matches,
            })
        } else if dead_only {
            json!({
                "query": results.query,
                "dead_status": results.dead_status,
            })
        } else if semantic_only {
            json!({
                "query": results.query,
                "semantic_matches": results.semantic_matches,
            })
        } else {
            json!({
                "query": results.query,
                "symbol_matches": results.symbol_matches,
                "param_matches": results.param_matches,
                "semantic_matches": results.semantic_matches,
                "suppression_matches": results.suppression_matches,
                "cross_matches": results.cross_matches,
                "dead_status": results.dead_status,
            })
        }
    }

    fn file_set_for_search_results(
        results: &SearchResults,
        symbol_only: bool,
        dead_only: bool,
        semantic_only: bool,
    ) -> HashSet<String> {
        let mut files = HashSet::new();

        if !dead_only && !semantic_only {
            for file_match in &results.symbol_matches.files {
                files.insert(file_match.file.clone());
            }
            for m in &results.param_matches {
                files.insert(m.file.clone());
            }
            for m in &results.suppression_matches {
                files.insert(m.file.clone());
            }
            for m in &results.cross_matches {
                files.insert(m.file.clone());
            }
        }

        if !dead_only && !symbol_only {
            for m in &results.semantic_matches {
                files.insert(m.file.clone());
            }
        }

        if !symbol_only && !semantic_only {
            for file in &results.dead_status.dead_in_files {
                files.insert(file.clone());
            }
        }

        files
    }

    fn compute_cross_files(
        per_query_files: &[(String, HashSet<String>)],
    ) -> Vec<(String, Vec<String>)> {
        let mut file_to_queries: HashMap<String, Vec<String>> = HashMap::new();
        for (query, files) in per_query_files {
            for file in files {
                file_to_queries
                    .entry(file.clone())
                    .or_default()
                    .push(query.clone());
            }
        }
        let mut out: Vec<(String, Vec<String>)> = file_to_queries
            .into_iter()
            .filter_map(|(file, mut queries)| {
                queries.sort();
                queries.dedup();
                if queries.len() >= 2 {
                    Some((file, queries))
                } else {
                    None
                }
            })
            .collect();

        out.sort_by(|(file_a, qs_a), (file_b, qs_b)| {
            qs_b.len().cmp(&qs_a.len()).then_with(|| file_a.cmp(file_b))
        });

        out
    }

    // Snapshot freshness is delegated to the single authority
    // (`snapshot::acquire_snapshot`): commit-only stale snapshots are reused
    // when the content fence proves indexed source bytes did not change, a
    // live `loct watch` short-circuits re-hashing entirely, and any internal
    // rescan rebuilds the same unified file universe as the initial scan.
    let analyses = snapshot::acquire_snapshot(
        root_list,
        snapshot::SnapshotReusePolicy::ReuseFence,
        &snapshot::AcquireOptions {
            verbose: parsed.verbose,
            ..Default::default()
        },
    )?
    .files;

    let symbol_only = parsed.search_symbol_only;
    let dead_only = parsed.search_dead_only;
    let semantic_only = parsed.search_semantic_only;

    match parsed.search_query_mode {
        SearchQueryMode::Single => {
            let query = parsed.search_query.as_ref().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "find requires a query, e.g.: loct find MySymbol",
                )
            })?;

            let results = do_search(query, &analyses);
            print_search_results(
                &results,
                parsed.output,
                symbol_only,
                dead_only,
                semantic_only,
                parsed.color,
            );
        }
        SearchQueryMode::Split => {
            if parsed.search_queries.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "find (split-mode) requires 2+ query terms, e.g.: loct find Foo Bar",
                ));
            }

            let mut results: Vec<(String, SearchResults)> = Vec::new();
            let mut per_query_files: Vec<(String, HashSet<String>)> = Vec::new();
            for q in &parsed.search_queries {
                let r = do_search(q, &analyses);
                per_query_files.push((
                    q.clone(),
                    file_set_for_search_results(&r, symbol_only, dead_only, semantic_only),
                ));
                results.push((q.clone(), r));
            }

            let cross_files = compute_cross_files(&per_query_files);

            match parsed.output {
                OutputMode::Human => {
                    println!(
                        "Search mode: split ({} queries)\n",
                        parsed.search_queries.len()
                    );

                    if cross_files.is_empty() {
                        println!("=== Cross-Match Files (0) ===");
                        println!("  No files matched 2+ queries.\n");
                    } else {
                        println!("=== Cross-Match Files ({}) ===", cross_files.len());
                        println!("  Files matching 2+ queries:\n");
                        for (file, qs) in &cross_files {
                            println!("  {} ({}): {}", file, qs.len(), qs.join(", "));
                        }
                        println!();
                    }

                    for (_q, r) in &results {
                        print_search_results(
                            r,
                            OutputMode::Human,
                            symbol_only,
                            dead_only,
                            semantic_only,
                            parsed.color,
                        );
                        println!();
                    }
                }
                OutputMode::Json => {
                    let out = json!({
                        "type": "search_multi",
                        "mode": "split",
                        "queries": parsed.search_queries,
                        "cross_files": cross_files.iter().map(|(file, qs)| json!({"file": file, "matched_queries": qs})).collect::<Vec<_>>(),
                        "results": results.iter().map(|(_q, r)| search_results_to_json_value(r, symbol_only, dead_only, semantic_only)).collect::<Vec<_>>(),
                    });
                    println!("{}", serde_json::to_string_pretty(&out).unwrap());
                }
                OutputMode::Jsonl => {
                    println!(
                        "{}",
                        json!({
                            "type": "search.summary",
                            "mode": "split",
                            "queries": parsed.search_queries,
                            "cross_files": cross_files.iter().map(|(file, qs)| json!({"file": file, "matched_queries": qs})).collect::<Vec<_>>(),
                        })
                    );
                    for (q, r) in &results {
                        println!(
                            "{}",
                            json!({
                                "type": "search.results",
                                "query": q,
                                "data": search_results_to_json_value(r, symbol_only, dead_only, semantic_only),
                            })
                        );
                    }
                }
            }
        }
        SearchQueryMode::And => {
            if parsed.search_queries.len() < 2 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "find (AND-mode) requires 2+ terms, e.g.: loct find \"Foo Bar\"",
                ));
            }

            let mut term_results: Vec<(String, SearchResults)> = Vec::new();
            let mut per_term_files: Vec<(String, HashSet<String>)> = Vec::new();

            for term in &parsed.search_queries {
                let r = do_search(term, &analyses);
                per_term_files.push((
                    term.clone(),
                    file_set_for_search_results(&r, symbol_only, dead_only, semantic_only),
                ));
                term_results.push((term.clone(), r));
            }

            let mut intersect: HashSet<String> = per_term_files
                .first()
                .map(|(_term, files)| files.clone())
                .unwrap_or_default();
            for (_term, files) in per_term_files.iter().skip(1) {
                intersect.retain(|f| files.contains(f));
            }
            let mut intersect_files: Vec<String> = intersect.into_iter().collect();
            intersect_files.sort();

            match parsed.output {
                OutputMode::Human => {
                    println!("Search mode: AND ({} terms)\n", parsed.search_queries.len());
                    println!("Terms: {}\n", parsed.search_queries.join(" & "));
                    println!("=== Intersection Files ({}) ===", intersect_files.len());
                    if intersect_files.is_empty() {
                        println!("  No files matched all terms.\n");
                    } else {
                        for f in &intersect_files {
                            println!("  - {}", f);
                        }
                        println!();
                    }
                }
                OutputMode::Json => {
                    let out = json!({
                        "type": "search_multi",
                        "mode": "and",
                        "terms": parsed.search_queries,
                        "intersection_files": intersect_files,
                        "results": term_results.iter().map(|(_t, r)| search_results_to_json_value(r, symbol_only, dead_only, semantic_only)).collect::<Vec<_>>(),
                    });
                    println!("{}", serde_json::to_string_pretty(&out).unwrap());
                }
                OutputMode::Jsonl => {
                    println!(
                        "{}",
                        json!({
                            "type": "search.summary",
                            "mode": "and",
                            "terms": parsed.search_queries,
                            "intersection_files": intersect_files,
                        })
                    );
                    for (term, r) in &term_results {
                        println!(
                            "{}",
                            json!({
                                "type": "search.results",
                                "query": term,
                                "data": search_results_to_json_value(r, symbol_only, dead_only, semantic_only),
                            })
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

/// Handle git subcommands for temporal awareness
fn run_git(subcommand: &GitSubcommand, parsed: &ParsedArgs) -> std::io::Result<()> {
    let cwd = std::env::current_dir()?;
    let repo = git::GitRepo::discover(&cwd)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))?;

    match subcommand {
        GitSubcommand::Compare { from, to } => run_git_compare(&repo, from, to.as_deref(), parsed),
        GitSubcommand::Blame { file } => run_git_blame(&repo, file, parsed),
        GitSubcommand::History {
            symbol,
            file,
            limit,
        } => run_git_history(&repo, symbol.as_deref(), file.as_deref(), *limit, parsed),
        GitSubcommand::WhenIntroduced {
            circular,
            dead,
            import,
        } => run_git_when_introduced(
            &repo,
            circular.as_deref(),
            dead.as_deref(),
            import.as_deref(),
            parsed,
        ),
    }
}

fn run_git_compare(
    repo: &git::GitRepo,
    from: &str,
    to: Option<&str>,
    _parsed: &ParsedArgs,
) -> std::io::Result<()> {
    let from_commit = repo
        .get_commit_info(from)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))?;

    let to_commit = if let Some(to_ref) = to {
        Some(
            repo.get_commit_info(to_ref)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))?,
        )
    } else {
        None
    };

    let to_ref = to.unwrap_or("HEAD");
    let changed_files = repo
        .changed_files(from, to_ref)
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let repo_path = repo.path().to_path_buf();
    let current_snapshot = match snapshot::Snapshot::load(&repo_path) {
        Ok(snap) => snap,
        Err(_) => {
            eprintln!("Warning: No snapshot found. Run 'loct' first for full analysis.");
            eprintln!("Showing file-level changes only.");
            snapshot::Snapshot::new(vec![repo_path.display().to_string()])
        }
    };

    let from_snapshot = current_snapshot.clone();
    let to_snapshot = current_snapshot;

    let snapshot_diff = diff::SnapshotDiff::compare(
        &from_snapshot,
        &to_snapshot,
        Some(from_commit),
        to_commit,
        &changed_files,
    );

    let json = serde_json::to_string_pretty(&snapshot_diff)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    println!("{}", json);

    Ok(())
}

fn run_git_blame(_repo: &git::GitRepo, file: &str, _parsed: &ParsedArgs) -> std::io::Result<()> {
    let response = serde_json::json!({
        "status": "not_implemented",
        "message": "git blame is planned for Phase 2",
        "file": file,
        "hint": "Use 'loct git compare' for snapshot comparison"
    });
    println!("{}", serde_json::to_string_pretty(&response).unwrap());
    Ok(())
}

fn run_git_history(
    _repo: &git::GitRepo,
    symbol: Option<&str>,
    file: Option<&str>,
    limit: usize,
    _parsed: &ParsedArgs,
) -> std::io::Result<()> {
    let response = serde_json::json!({
        "status": "not_implemented",
        "message": "git history is planned for Phase 3",
        "symbol": symbol,
        "file": file,
        "limit": limit,
        "hint": "Use 'loct git compare' for snapshot comparison"
    });
    println!("{}", serde_json::to_string_pretty(&response).unwrap());
    Ok(())
}

fn run_git_when_introduced(
    _repo: &git::GitRepo,
    circular: Option<&str>,
    dead: Option<&str>,
    import: Option<&str>,
    _parsed: &ParsedArgs,
) -> std::io::Result<()> {
    let response = serde_json::json!({
        "status": "not_implemented",
        "message": "git when-introduced is planned for Phase 3",
        "circular": circular,
        "dead": dead,
        "import": import,
        "hint": "Use 'loct git compare' for snapshot comparison"
    });
    println!("{}", serde_json::to_string_pretty(&response).unwrap());
    Ok(())
}
