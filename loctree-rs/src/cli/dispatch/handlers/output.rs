//! Output-related command handlers
//!
//! Handles: lint, dist

use super::super::super::command::{DistOptions, LintOptions};
use super::super::{DispatchResult, GlobalOptions, load_or_create_snapshot_for_roots};
use crate::progress::Spinner;

/// Handle the lint command - run linting checks
pub fn handle_lint_command(opts: &LintOptions, global: &GlobalOptions) -> DispatchResult {
    use std::path::PathBuf;

    use crate::analyzer::report::CommandGap;

    let sarif_mode = opts.sarif;
    let show_output = !global.quiet && !sarif_mode;

    // Show spinner unless in quiet/json mode
    let spinner = if show_output && !global.json {
        Some(Spinner::new("Running lint checks..."))
    } else {
        None
    };

    // Load snapshot (auto-scan if missing) using ALL provided roots.
    // This is critical for Tauri projects where FE and BE roots are passed separately
    // (e.g. `src` + `src-tauri/src`).
    let roots: Vec<PathBuf> = if opts.roots.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        opts.roots.clone()
    };

    // Fail fast on invalid roots to avoid misleading "missing handlers" noise.
    let missing_roots: Vec<&PathBuf> = roots.iter().filter(|p| !p.exists()).collect();
    if !missing_roots.is_empty() {
        if show_output {
            eprintln!("[loct][lint] Invalid root path(s):");
            for root in missing_roots {
                eprintln!("  - {}", root.display());
            }
        }
        return DispatchResult::Exit(2);
    }

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

    let mut issues_found = 0;
    let mut missing_gaps: Vec<CommandGap> = Vec::new();
    let mut pipeline_summary = serde_json::json!({});

    // Check for missing handlers if requested
    let run_handler_checks = opts.fail || opts.tauri || sarif_mode;
    if run_handler_checks {
        for bridge in &snapshot.command_bridges {
            if !bridge.has_handler && bridge.is_called {
                missing_gaps.push(CommandGap {
                    name: bridge.name.clone(),
                    confidence: None,
                    locations: bridge.frontend_calls.clone(),
                    implementation_name: None,
                    string_literal_matches: Vec::new(),
                });
            }
        }

        if !missing_gaps.is_empty() {
            issues_found += missing_gaps.len();

            if show_output {
                eprintln!(
                    "[loct][lint] {} missing Tauri handlers:",
                    missing_gaps.len()
                );
                for gap in &missing_gaps {
                    eprintln!("  - {}", gap.name);
                }
            }
        }
    }

    // Check for problematic event bridges if requested
    let run_event_checks = opts.fail || sarif_mode;
    if run_event_checks {
        // Count events with no emitters or no listeners
        let orphan_events = snapshot
            .event_bridges
            .iter()
            .filter(|e| e.emits.is_empty() || e.listens.is_empty())
            .collect::<Vec<_>>();

        if opts.fail && !orphan_events.is_empty() {
            issues_found += orphan_events.len();

            if show_output {
                eprintln!("[loct][lint] {} orphan events:", orphan_events.len());
                for event in orphan_events.iter().take(5) {
                    if event.emits.is_empty() {
                        eprintln!("  - {} (no emitters)", event.name);
                    } else {
                        eprintln!("  - {} (no listeners)", event.name);
                    }
                }
                if orphan_events.len() > 5 {
                    eprintln!("  ... and {} more", orphan_events.len() - 5);
                }
            }
        }

        if sarif_mode {
            let mut ghost_emits = Vec::new();
            let mut orphan_listeners = Vec::new();

            for event in &snapshot.event_bridges {
                if event.listens.is_empty() {
                    for (file, line, _kind) in &event.emits {
                        ghost_emits.push(serde_json::json!({
                            "name": event.name,
                            "path": file,
                            "line": line,
                            "confidence": "high",
                        }));
                    }
                }
                if event.emits.is_empty() {
                    for (file, line) in &event.listens {
                        orphan_listeners.push(serde_json::json!({
                            "name": event.name,
                            "path": file,
                            "line": line,
                        }));
                    }
                }
            }

            pipeline_summary = serde_json::json!({
                "events": {
                    "ghostEmits": ghost_emits,
                    "orphanListeners": orphan_listeners,
                }
            });
        }
    }

    if opts.entrypoints {
        let drift = &snapshot.metadata.entrypoint_drift;
        let entrypoints = &snapshot.metadata.entrypoints;
        let drift_count = drift.declared_missing.len()
            + drift.declared_without_marker.len()
            + drift.code_only_entrypoints.len()
            + drift.declared_unresolved.len();
        if show_output {
            if entrypoints.is_empty() {
                eprintln!("[loct][lint] No entrypoints detected");
            } else {
                eprintln!("[loct][lint] Entrypoints ({}):", entrypoints.len());
                for entry in entrypoints.iter().take(10) {
                    let kinds = if entry.kinds.is_empty() {
                        "unknown".to_string()
                    } else {
                        entry.kinds.join(", ")
                    };
                    eprintln!("  - {} ({})", entry.path, kinds);
                }
                if entrypoints.len() > 10 {
                    eprintln!("  ... and {} more", entrypoints.len() - 10);
                }
            }
        }
        if drift_count > 0 {
            issues_found += drift_count;
            if show_output {
                eprintln!("[loct][lint] Entrypoint drift detected:");
                for item in &drift.declared_missing {
                    eprintln!("  - missing: {} ({})", item.path, item.source);
                }
                for item in &drift.declared_without_marker {
                    eprintln!("  - no marker: {} ({})", item.path, item.source);
                }
                for item in &drift.declared_unresolved {
                    eprintln!("  - unresolved: {} ({})", item.path, item.source);
                }
                for item in &drift.code_only_entrypoints {
                    eprintln!("  - code-only: {}", item.path);
                }
            }
        } else if show_output {
            eprintln!("[loct][lint] No entrypoint drift detected");
        }
    }

    let run_ts = opts.deep || opts.ts;
    let run_react = opts.deep || opts.react;
    let run_memory = opts.deep || opts.memory;

    if run_ts || run_react || run_memory {
        let roots: Vec<PathBuf> = if snapshot.metadata.roots.is_empty() {
            vec![PathBuf::from(".")]
        } else {
            snapshot.metadata.roots.iter().map(PathBuf::from).collect()
        };

        let resolve_path = |rel: &str| -> Option<PathBuf> {
            roots
                .iter()
                .map(|root| root.join(rel))
                .find(|candidate| candidate.exists())
        };

        let mut ts_issues = Vec::new();
        let mut react_issues = Vec::new();
        let mut memory_issues = Vec::new();

        for file in &snapshot.files {
            let ext = std::path::Path::new(&file.path)
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("");
            let full_path = match resolve_path(&file.path) {
                Some(path) => path,
                None => continue,
            };
            let content = match std::fs::read_to_string(&full_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if run_ts && matches!(ext, "ts" | "tsx") {
                ts_issues.extend(crate::analyzer::ts_lint::lint_ts_file(&full_path, &content));
            }

            if run_react && matches!(ext, "ts" | "tsx" | "js" | "jsx") {
                react_issues.extend(crate::analyzer::react_lint::analyze_react_file(
                    &content,
                    &full_path,
                    file.path.clone(),
                ));
            }

            if run_memory && matches!(ext, "ts" | "tsx" | "js" | "jsx") {
                memory_issues.extend(crate::analyzer::memory_lint::lint_memory_file(
                    &full_path, &content,
                ));
            }
        }

        if !ts_issues.is_empty() {
            issues_found += ts_issues.len();
            if show_output {
                eprintln!("[loct][lint] {} TypeScript lint issue(s)", ts_issues.len());
                for issue in ts_issues.iter().take(10) {
                    eprintln!(
                        "  - {}:{}:{} {}",
                        issue.file, issue.line, issue.column, issue.message
                    );
                }
                if ts_issues.len() > 10 {
                    eprintln!("  ... and {} more", ts_issues.len() - 10);
                }
            }
        }

        if !react_issues.is_empty() {
            issues_found += react_issues.len();
            if show_output {
                eprintln!("[loct][lint] {} React lint issue(s)", react_issues.len());
                for issue in react_issues.iter().take(10) {
                    eprintln!("  - {}:{} {}", issue.file, issue.line, issue.message);
                }
                if react_issues.len() > 10 {
                    eprintln!("  ... and {} more", react_issues.len() - 10);
                }
            }
        }

        if !memory_issues.is_empty() {
            issues_found += memory_issues.len();
            if show_output {
                eprintln!("[loct][lint] {} Memory lint issue(s)", memory_issues.len());
                for issue in memory_issues.iter().take(10) {
                    eprintln!("  - {}:{} {}", issue.file, issue.line, issue.message);
                }
                if memory_issues.len() > 10 {
                    eprintln!("  ... and {} more", memory_issues.len() - 10);
                }
            }
        }
    }

    // Determine exit code based on findings and --fail flag
    let exit_code = if opts.fail && issues_found > 0 { 1 } else { 0 };

    if let Some(s) = spinner {
        if issues_found > 0 {
            s.finish_warning(&format!("Found {} issue(s)", issues_found));
        } else {
            s.finish_success("No issues found");
        }
    } else if show_output {
        if issues_found == 0 {
            println!("[loct][lint] No issues found");
        } else {
            println!("[loct][lint] Found {} issue(s)", issues_found);
        }
    }

    // Output SARIF format if requested
    if opts.sarif {
        let inputs = crate::analyzer::sarif::SarifInputs {
            duplicate_exports: &[],
            missing_handlers: &missing_gaps,
            unused_handlers: &[],
            dead_exports: &[],
            circular_imports: &[],
            pipeline_summary: &pipeline_summary,
            snapshot: Some(&snapshot),
        };
        if let Err(err) = crate::analyzer::sarif::print_sarif(inputs) {
            eprintln!("[loct][error] Failed to emit SARIF: {}", err);
            return DispatchResult::Exit(1);
        }
    }

    DispatchResult::Exit(exit_code)
}

/// Handle the dist command - analyze bundle using source maps
pub fn handle_dist_command(opts: &DistOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::analyzer::dist::analyze_distribution_with_snapshot;
    use crate::analyzer::root_scan::scan_results_from_snapshot;

    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Analyzing bundle distribution..."))
    } else {
        None
    };

    let source_map_paths = if opts.source_maps.is_empty() {
        if let Some(s) = spinner {
            s.finish_error("at least one --source-map is required");
        } else {
            eprintln!("[loct][error] at least one --source-map is required");
        }
        return DispatchResult::Exit(1);
    } else {
        opts.source_maps.clone()
    };

    let src_path = match &opts.src {
        Some(p) => p.clone(),
        None => {
            if let Some(s) = spinner {
                s.finish_error("--src is required");
            } else {
                eprintln!("[loct][error] --src is required");
            }
            return DispatchResult::Exit(1);
        }
    };

    match analyze_distribution_with_snapshot(&source_map_paths, &src_path) {
        Ok((result, snapshot)) => {
            let serialized = if global.json || opts.report_path.is_some() {
                match serde_json::to_string_pretty(&result) {
                    Ok(json) => Some(json),
                    Err(e) => {
                        eprintln!("[loct][error] Failed to serialize results: {}", e);
                        return DispatchResult::Exit(1);
                    }
                }
            } else {
                None
            };

            let snapshot_root =
                crate::snapshot::resolve_snapshot_root(std::slice::from_ref(&src_path));
            let scan_results = scan_results_from_snapshot(&snapshot);
            let artifact_args = crate::args::ParsedArgs {
                verbose: global.verbose,
                library_mode: global.library_mode,
                python_library: global.python_library,
                ..Default::default()
            };

            if let Err(err) = crate::snapshot::write_auto_artifacts(
                &snapshot_root,
                std::slice::from_ref(&src_path),
                &scan_results,
                &artifact_args,
                Some(&snapshot.metadata),
                Some(result.clone()),
            ) {
                eprintln!(
                    "[loct][warn] dist artifacts were not refreshed under {}: {}",
                    crate::snapshot::Snapshot::artifacts_dir(&snapshot_root).display(),
                    err
                );
            } else if !global.quiet {
                eprintln!(
                    "[loct][dist] refreshed report artifacts under {}",
                    crate::snapshot::Snapshot::artifacts_dir(&snapshot_root).display()
                );
            }

            if let Some(report_path) = &opts.report_path {
                if let Some(parent) = report_path.parent()
                    && let Err(e) = std::fs::create_dir_all(parent)
                {
                    eprintln!(
                        "[loct][error] Failed to create report directory {}: {}",
                        parent.display(),
                        e
                    );
                    return DispatchResult::Exit(1);
                }

                if let Err(e) =
                    std::fs::write(report_path, serialized.as_deref().unwrap_or_default())
                {
                    eprintln!(
                        "[loct][error] Failed to write dist report {}: {}",
                        report_path.display(),
                        e
                    );
                    return DispatchResult::Exit(1);
                }
            }

            if let Some(s) = spinner {
                s.finish_success(&format!(
                    "Ranked {} runtime candidate(s), {} dead export(s) across {} source map(s) ({})",
                    result.candidates.len(),
                    result.dead_exports.len(),
                    result.source_maps,
                    result.reduction
                ));
            }

            if global.json {
                println!("{}", serialized.as_deref().unwrap_or_default());
            } else {
                let boot_chunks = result
                    .chunks
                    .iter()
                    .filter(|chunk| {
                        matches!(chunk.role, crate::analyzer::dist::DistChunkRole::Boot)
                    })
                    .count();
                let feature_chunks = result.chunks.len().saturating_sub(boot_chunks);

                println!("Bundle Analysis:");
                println!("  Source maps:      {}", result.source_maps);
                println!("  Source exports:   {}", result.source_exports);
                println!("  Bundled exports: {}", result.bundled_exports);
                println!("  Dead exports:    {}", result.dead_exports.len());
                println!("  Runtime candidates: {}", result.candidates.len());
                println!("  Reduction:       {}", result.reduction);
                println!("  Analysis level:  {}", result.analysis_level.as_str());
                println!("  Bundle coverage: {}%", result.coverage_pct);
                if !result.chunks.is_empty() {
                    println!("  Boot chunks:     {}", boot_chunks);
                    println!("  Feature chunks:  {}", feature_chunks);
                }
                if let Some(report_path) = &opts.report_path {
                    println!("  Report:          {}", report_path.display());
                }
                println!();

                if !result.candidate_counts.is_empty() {
                    println!("Candidate classes:");
                    for (class_name, count) in &result.candidate_counts {
                        println!("  {:<20} {}", class_name, count);
                    }
                    println!();
                }

                if !result.candidates.is_empty() {
                    println!("Top runtime candidates:");
                    for candidate in result.candidates.iter().take(20) {
                        println!(
                            "  [{}][{}] {} ({}) in {}:{}",
                            candidate.class_name.as_str(),
                            candidate.confidence.as_str(),
                            candidate.name,
                            candidate.kind,
                            candidate.file,
                            candidate.line
                        );
                        if let Some(note) = candidate.notes.first() {
                            println!("      {}", note);
                        }
                    }
                    if result.candidates.len() > 20 {
                        println!("  ... and {} more", result.candidates.len() - 20);
                    }
                } else {
                    println!("No runtime-local candidates found across analyzed chunks.");
                }
            }

            DispatchResult::Exit(0)
        }
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Analysis failed: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            DispatchResult::Exit(1)
        }
    }
}
