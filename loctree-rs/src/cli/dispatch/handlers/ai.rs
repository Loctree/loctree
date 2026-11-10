//! AI and code analysis command handlers
//!
//! Handles: crowd, twins, sniff, suppress

use super::super::super::command::{
    CrowdOptions, SniffOptions, SuppressOptions, TagmapOptions, TwinsOptions,
};
use super::super::{DispatchResult, GlobalOptions, is_test_file, load_or_create_snapshot};
use super::deprecation::warn_deprecated;
use crate::progress::Spinner;
use crate::suppressions::{SuppressionType, Suppressions};
use crate::tagmap::{collect_tagmap_facts, tagmap_matches, tagmap_normalize};

/// Handle the crowd command - detect functional crowds
pub fn handle_crowd_command(opts: &CrowdOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::analyzer::crowd::{
        detect_all_crowds_with_edges, detect_crowd_with_edges, format_crowd, format_crowds_summary,
    };
    use std::path::Path;

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Detecting functional crowds..."))
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

    // Filter out test files unless --include-tests is specified
    let files: Vec<_> = if opts.include_tests {
        snapshot.files.clone()
    } else {
        snapshot
            .files
            .iter()
            .filter(|f| !is_test_file(&f.path))
            .cloned()
            .collect()
    };

    // Detect crowds (using edges for accurate transitive importer counting)
    let crowds = if let Some(ref pattern) = opts.pattern {
        // Single pattern mode
        vec![detect_crowd_with_edges(&files, pattern, &snapshot.edges)]
    } else {
        // Auto-detect mode
        let mut all_crowds = detect_all_crowds_with_edges(&files, &snapshot.edges);

        // Apply min_size filter
        if let Some(min_size) = opts.min_size {
            all_crowds.retain(|c| c.members.len() >= min_size);
        }

        // Apply limit
        if let Some(limit) = opts.limit {
            all_crowds.truncate(limit);
        }

        all_crowds
    };

    // Filter out empty crowds
    let crowds: Vec<_> = crowds
        .into_iter()
        .filter(|c| !c.members.is_empty())
        .collect();

    if crowds.is_empty() {
        if let Some(s) = spinner {
            if let Some(ref pattern) = opts.pattern {
                s.finish_warning(&format!("No files found matching pattern '{}'", pattern));
            } else {
                s.finish_warning("No crowds detected in codebase");
            }
        } else if !global.quiet {
            if let Some(ref pattern) = opts.pattern {
                eprintln!(
                    "[loct][crowd] No files found matching pattern '{}'",
                    pattern
                );
            } else {
                eprintln!("[loct][crowd] No crowds detected in codebase");
            }
        }
        return DispatchResult::Exit(0);
    }

    // Finish spinner with success message
    if let Some(s) = spinner {
        let total_members: usize = crowds.iter().map(|c| c.members.len()).sum();
        s.finish_success(&format!(
            "Found {} crowd(s) with {} total members",
            crowds.len(),
            total_members
        ));
    }

    // Output results
    if global.json {
        match serde_json::to_string_pretty(&crowds) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize crowds: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else {
        // Human-readable output
        if crowds.len() == 1 {
            // Single crowd - detailed view
            println!("{}", format_crowd(&crowds[0], global.verbose));
        } else {
            // Multiple crowds - summary view
            println!("{}", format_crowds_summary(&crowds));
        }
    }

    DispatchResult::Exit(0)
}

/// Handle the tagmap command - unified search around a keyword
pub fn handle_tagmap_command(opts: &TagmapOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::analyzer::crowd::detect_crowd_with_edges;
    use crate::analyzer::dead_parrots::{DeadFilterConfig, find_dead_exports};
    use std::path::Path;

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new(&format!(
            "Analyzing tagmap '{}'...",
            opts.keyword
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

    // Filter test files unless --include-tests
    let files: Vec<_> = if opts.include_tests {
        snapshot.files.clone()
    } else {
        snapshot
            .files
            .iter()
            .filter(|f| !is_test_file(&f.path))
            .cloned()
            .collect()
    };

    let keyword_lower = opts.keyword.to_ascii_lowercase();
    let keyword_normalized = tagmap_normalize(&opts.keyword);

    // 1. Find files matching keyword (in path or filename)
    let matching_files: Vec<_> = files
        .iter()
        .filter(|f| tagmap_matches(&f.path, &keyword_lower, &keyword_normalized))
        .collect();

    // 2. Rank the complete shared evidence universe before applying --limit.
    let fact_matches = collect_tagmap_facts(&files, &opts.keyword);

    // 3. Detect crowd for this keyword
    let crowd = detect_crowd_with_edges(&files, &opts.keyword, &snapshot.edges);

    // 4. Find dead exports related to keyword
    let dead_ok_globs = crate::fs_utils::load_loctignore_dead_ok_globs(root);
    let all_dead = find_dead_exports(
        &files,
        false,
        None,
        DeadFilterConfig {
            include_tests: opts.include_tests,
            dead_ok_globs,
            ..Default::default()
        },
    );
    let dead_for_keyword: Vec<_> = all_dead
        .iter()
        .filter(|d| {
            tagmap_matches(&d.symbol, &keyword_lower, &keyword_normalized)
                || tagmap_matches(&d.file, &keyword_lower, &keyword_normalized)
        })
        .collect();

    // Finish spinner
    if let Some(s) = spinner {
        s.finish_success(&format!(
            "Found {} files, {} facts, {} crowd members, {} dead exports",
            matching_files.len(),
            fact_matches.len(),
            crowd.members.len(),
            dead_for_keyword.len()
        ));
    }

    let limit = opts.limit.unwrap_or(usize::MAX);
    let files_omitted = matching_files.len().saturating_sub(limit);
    let facts_omitted = fact_matches.len().saturating_sub(limit);
    let crowd_omitted = crowd.members.len().saturating_sub(limit);
    let dead_omitted = dead_for_keyword.len().saturating_sub(limit);

    // Output results
    if global.json {
        let json = serde_json::json!({
            "keyword": opts.keyword,
            "files": {
                "count": matching_files.len(),
                "omitted_by_limit": if opts.limit.is_some() && files_omitted > 0 {
                    Some(files_omitted)
                } else {
                    None
                },
                "items": matching_files.iter().take(limit).map(|f| {
                    serde_json::json!({
                        "path": f.path,
                        "loc": f.loc,
                        "language": f.language
                    })
                }).collect::<Vec<_>>()
            },
            "facts": {
                "count": fact_matches.len(),
                "omitted_by_limit": if opts.limit.is_some() && facts_omitted > 0 {
                    Some(facts_omitted)
                } else {
                    None
                },
                "items": fact_matches.iter().take(limit).collect::<Vec<_>>()
            },
            "crowd": {
                "pattern": crowd.pattern,
                "members": crowd.members.len(),
                "omitted_by_limit": if opts.limit.is_some() && crowd_omitted > 0 {
                    Some(crowd_omitted)
                } else {
                    None
                },
                "score": crowd.score,
                "issues": crowd.issues.len()
            },
            "dead_exports": {
                "count": dead_for_keyword.len(),
                "omitted_by_limit": if opts.limit.is_some() && dead_omitted > 0 {
                    Some(dead_omitted)
                } else {
                    None
                },
                "items": dead_for_keyword.iter().take(limit).map(|d| {
                    serde_json::json!({
                        "file": d.file,
                        "symbol": d.symbol,
                        "confidence": d.confidence
                    })
                }).collect::<Vec<_>>()
            }
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).unwrap_or_default()
        );
    } else {
        // Human-readable output
        println!();
        println!("=== TAGMAP: '{}' ===", opts.keyword);

        println!("\nFILES MATCHING KEYWORD ({}):", matching_files.len());
        if matching_files.is_empty() {
            println!("  (none)");
        } else {
            for file in matching_files.iter().take(limit) {
                println!("  {} ({} LOC, {})", file.path, file.loc, file.language);
            }
            if opts.limit.is_some() && files_omitted > 0 {
                println!("  {} additional file(s) omitted by --limit", files_omitted);
            }
        }

        println!("\nINDEXED FACTS MATCHING KEYWORD ({}):", fact_matches.len());
        if fact_matches.is_empty() {
            println!("  (none)");
        } else {
            for fact in fact_matches.iter().take(limit) {
                let kind = fact["kind"].as_str().unwrap_or("fact");
                let file = fact["file"].as_str().unwrap_or("<unknown>");
                let label = fact["name"]
                    .as_str()
                    .or_else(|| fact["source"].as_str())
                    .or_else(|| fact["value"].as_str())
                    .unwrap_or("<unknown>");
                let line = fact["line"].as_u64().unwrap_or(0);
                println!("  {} {} in {}:{}", kind, label, file, line);
            }
            if opts.limit.is_some() && facts_omitted > 0 {
                println!("  {} additional fact(s) omitted by --limit", facts_omitted);
            }
        }

        if !crowd.members.is_empty() {
            println!(
                "\nCROWD ANALYSIS ({} files, score {:.1}/10):",
                crowd.members.len(),
                crowd.score
            );
            for member in crowd.members.iter().take(limit) {
                println!("  {} ({} importers)", member.file, member.importer_count);
            }
            if opts.limit.is_some() && crowd_omitted > 0 {
                println!(
                    "  {} additional crowd member(s) omitted by --limit",
                    crowd_omitted
                );
            }
            if !crowd.issues.is_empty() {
                println!(
                    "  Issues: {} detected (use 'loct crowd {}' for details)",
                    crowd.issues.len(),
                    opts.keyword
                );
            }
        } else {
            println!("\nCROWD ANALYSIS: (no cluster found)");
        }

        if !dead_for_keyword.is_empty() {
            println!("\nDEAD EXPORTS ({}):", dead_for_keyword.len());
            for dead in dead_for_keyword.iter().take(limit) {
                println!("  {} in {} [{}]", dead.symbol, dead.file, dead.confidence);
            }
            if opts.limit.is_some() && dead_omitted > 0 {
                println!(
                    "  {} additional dead export(s) omitted by --limit",
                    dead_omitted
                );
            }
        } else {
            println!("\nDEAD EXPORTS: (none)");
        }

        println!();
    }

    DispatchResult::Exit(0)
}

pub fn handle_twins_command(opts: &TwinsOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::analyzer::barrels::{analyze_barrel_chaos, format_barrel_analysis};
    use crate::analyzer::frameworks::detect_frameworks;
    use crate::analyzer::route_twins::{detect_route_twins, print_route_twins_human};
    use crate::analyzer::twins::{
        TwinCategory, categorize_twin, detect_exact_twins_with_sources, detect_language,
        order_twins_for_page, page_family_windows, page_has_more, parse_twin_class_filter,
        print_exact_twins_human, print_twins_human, retain_twin_classes,
    };
    use std::path::Path;

    let class_filter = match opts.class.as_deref() {
        Some(spec) => match parse_twin_class_filter(spec) {
            Ok(classes) => classes,
            Err(err) => {
                eprintln!("[loct][error] {err}");
                return DispatchResult::Exit(1);
            }
        },
        None => Vec::new(),
    };

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Analyzing semantic duplicates..."))
    } else {
        None
    };

    // Load snapshot (auto-scan if missing)
    let root = opts.path.as_deref().unwrap_or(Path::new("."));

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

    // Detect frameworks for convention filtering (unless --ignore-conventions)
    let detected_frameworks = if opts.ignore_conventions {
        Vec::new()
    } else {
        detect_frameworks(root)
    };

    // Log detected frameworks if verbose
    if global.verbose && !detected_frameworks.is_empty() {
        let fw_names: Vec<_> = detected_frameworks.iter().map(|f| f.to_string()).collect();
        eprintln!(
            "[loct][twins] Detected frameworks: {} (use --ignore-conventions to show all)",
            fw_names.join(", ")
        );
    }

    // Load suppressions (unless --include-suppressed)
    let suppressions = if opts.include_suppressed {
        Suppressions::default()
    } else {
        Suppressions::load(root)
    };

    // Dead parrots — canonical pipeline, same source as `loct dead`,
    // `loct findings` and repo-view: one config, semantic suppression,
    // literal/symbol-graph cross-check, entry-point fence. The twins surface
    // previously ran its own registry-based detector and reported a different
    // dead count on the same snapshot; that fork is gone.
    let mut dead_result = {
        use crate::analyzer::dead_parrots::{
            DeadFilterConfig, canonical_dead_filter, compute_dead_truth_with,
        };
        use crate::analyzer::twins::{SymbolEntry, TwinsResult, build_symbol_registry};

        let dead_truth = compute_dead_truth_with(
            &snapshot,
            DeadFilterConfig {
                include_tests: opts.include_tests,
                ..canonical_dead_filter(&snapshot)
            },
            false,
        );
        TwinsResult {
            dead_parrots: dead_truth
                .dead
                .iter()
                .map(|dead| SymbolEntry {
                    name: dead.symbol.clone(),
                    kind: "export".to_string(),
                    file_path: dead.file.clone(),
                    line: dead.line.unwrap_or(0),
                    import_count: 0,
                })
                .collect(),
            total_symbols: build_symbol_registry(&snapshot.files, opts.include_tests).len(),
            total_files: snapshot.files.len(),
        }
    };

    // Filter out suppressed dead parrots
    if !opts.include_suppressed {
        dead_result.dead_parrots.retain(|dp| {
            !suppressions.is_suppressed(&SuppressionType::DeadParrot, &dp.name, Some(&dp.file_path))
                && !suppressions.is_suppressed(
                    &SuppressionType::DeadExport,
                    &dp.name,
                    Some(&dp.file_path),
                )
        });
    }

    // Run exact twins detection with framework awareness (unless dead_only)
    let frameworks_opt = if detected_frameworks.is_empty() {
        None
    } else {
        Some(detected_frameworks.as_slice())
    };

    let mut twins = if !opts.dead_only {
        detect_exact_twins_with_sources(
            &snapshot.files,
            opts.include_tests,
            frameworks_opt,
            &mut crate::analyzer::twins::TwinSources::for_request(&snapshot, root),
        )
    } else {
        Vec::new()
    };

    // Filter out suppressed twins
    if !opts.include_suppressed {
        twins.retain(|twin| !suppressions.is_suppressed(&SuppressionType::Twins, &twin.name, None));
    }

    // Run barrel chaos analysis (unless dead_only)
    let mut barrel_analysis = if !opts.dead_only {
        Some(analyze_barrel_chaos(&snapshot))
    } else {
        None
    };

    // Detect route-level twins — runtime contract drift surface.
    // Source hak: 2026-05-18 Screenscribe HAK 6 (two FastAPI handlers
    // registered `POST /api/stt`, twin detector reported 0 groups).
    let mut route_twins = if !opts.dead_only {
        detect_route_twins(&snapshot.files)
    } else {
        Vec::new()
    };

    let dead_count = dead_result.dead_parrots.len();
    let twin_count = twins.len();
    let route_twin_count = route_twins.len();
    let same_language_count = twins
        .iter()
        .filter(|twin| matches!(categorize_twin(twin), TwinCategory::SameLanguage(_)))
        .count();
    let cross_language_count = twins
        .iter()
        .filter(|twin| matches!(categorize_twin(twin), TwinCategory::CrossLanguage))
        .count();
    let high_similarity_count = twins
        .iter()
        .filter(|twin| {
            twin.signature_similarity
                .map(|score| score >= 0.8)
                .unwrap_or(false)
        })
        .count();
    let barrel_count = barrel_analysis.as_ref().map_or(0, |analysis| {
        analysis.missing_barrels.len()
            + analysis.deep_chains.len()
            + analysis.inconsistent_paths.len()
    });
    let total_findings = dead_count + twin_count + route_twin_count + barrel_count;

    // One filtered census, walked in priority order: exact twins (class-ranked),
    // routes, dead parrots, then missing barrels, deep chains and inconsistent
    // paths. Every top-level finding spends the same offset/limit exactly once.
    order_twins_for_page(&mut twins);
    let twins_before_class = twins.len();
    retain_twin_classes(&mut twins, &class_filter);
    let class_dropped_twins = twins_before_class - twins.len();
    let class_dropped_routes = if class_filter.is_empty() {
        0
    } else {
        let dropped = route_twins.len();
        route_twins.clear();
        dropped
    };
    if opts.omit_dead_parrots {
        dead_result.dead_parrots.clear();
    }
    for twin in &mut route_twins {
        twin.locations.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then(a.line.cmp(&b.line))
                .then(a.handler.cmp(&b.handler))
        });
    }
    dead_result.dead_parrots.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then(a.line.cmp(&b.line))
            .then(a.name.cmp(&b.name))
            .then(a.kind.cmp(&b.kind))
    });
    // The detectors rank severity, but some use HashMap iteration for ties.
    // Pin those ties here so separate CLI processes reconstruct one census.
    if let Some(analysis) = barrel_analysis.as_mut() {
        analysis.missing_barrels.sort_by(|a, b| {
            b.external_import_count
                .cmp(&a.external_import_count)
                .then(a.directory.cmp(&b.directory))
        });
        analysis.deep_chains.sort_by(|a, b| {
            b.depth
                .cmp(&a.depth)
                .then(a.symbol.cmp(&b.symbol))
                .then(a.chain.cmp(&b.chain))
        });
        analysis.inconsistent_paths.sort_by(|a, b| {
            b.alternative_paths
                .len()
                .cmp(&a.alternative_paths.len())
                .then(a.symbol.cmp(&b.symbol))
                .then(a.canonical_path.cmp(&b.canonical_path))
                .then(a.alternative_paths.cmp(&b.alternative_paths))
        });
    }
    let barrel_lengths = barrel_analysis.as_ref().map_or([0; 3], |analysis| {
        [
            analysis.missing_barrels.len(),
            analysis.deep_chains.len(),
            analysis.inconsistent_paths.len(),
        ]
    });
    let windows = page_family_windows(
        &[
            twins.len(),
            route_twins.len(),
            dead_result.dead_parrots.len(),
            barrel_lengths[0],
            barrel_lengths[1],
            barrel_lengths[2],
        ],
        opts.offset,
        opts.limit,
    );
    twins = twins
        .into_iter()
        .skip(windows[0].0)
        .take(windows[0].1)
        .collect();
    route_twins = route_twins
        .into_iter()
        .skip(windows[1].0)
        .take(windows[1].1)
        .collect();
    dead_result.dead_parrots = dead_result
        .dead_parrots
        .into_iter()
        .skip(windows[2].0)
        .take(windows[2].1)
        .collect();
    if let Some(analysis) = barrel_analysis.as_mut() {
        analysis.missing_barrels.drain(..windows[3].0);
        analysis.missing_barrels.truncate(windows[3].1);
        analysis.deep_chains.drain(..windows[4].0);
        analysis.deep_chains.truncate(windows[4].1);
        analysis.inconsistent_paths.drain(..windows[5].0);
        analysis.inconsistent_paths.truncate(windows[5].1);
    }
    let returned_findings = dead_result.dead_parrots.len()
        + twins.len()
        + route_twins.len()
        + barrel_analysis.as_ref().map_or(0, |analysis| {
            analysis.missing_barrels.len()
                + analysis.deep_chains.len()
                + analysis.inconsistent_paths.len()
        });
    let page_total = total_findings
        .saturating_sub(if opts.omit_dead_parrots {
            dead_count
        } else {
            0
        })
        .saturating_sub(class_dropped_twins)
        .saturating_sub(class_dropped_routes);

    // Finish spinner before printing results
    if let Some(s) = spinner {
        s.finish_success(&format!(
            "Found {} dead parrot(s), {} twin group(s), {} route twin(s)",
            dead_count, twin_count, route_twin_count,
        ));
    }

    // JSON mode: emit single combined JSON object
    if global.json {
        let twin_to_json = |twin: &crate::analyzer::twins::ExactTwin| {
            let category = categorize_twin(twin);
            let mut json = serde_json::json!({
                "name": twin.name,
                "category": match category {
                    TwinCategory::SameLanguage(ref lang) => format!("same_language:{:?}", lang).to_lowercase(),
                    TwinCategory::CrossLanguage => "cross_language".to_string(),
                    TwinCategory::Namesake => "namesake".to_string(),
                },
                "class": twin.class,
                "shape_match": twin.shape_match,
                "single_module_target": twin.single_module_target,
                "exclude_from_score": twin.exclude_from_score,
                "action": crate::analyzer::twins::twin_action(twin),
                "locations": twin.locations.iter().map(|loc| {
                    let mut loc_json = serde_json::json!({
                        "file": loc.file_path,
                        "line": loc.line,
                        "kind": loc.kind,
                        "imports": loc.import_count,
                        "canonical": loc.is_canonical,
                        "language": format!("{:?}", detect_language(&loc.file_path)).to_lowercase(),
                    });
                    if let Some(ref fp) = loc.signature_fingerprint {
                        loc_json["signature_fingerprint"] = serde_json::json!(fp);
                    }
                    loc_json
                }).collect::<Vec<_>>(),
            });
            if let Some(sim) = twin.signature_similarity {
                json["signature_similarity"] = serde_json::json!(sim);
            }
            json
        };

        // Build barrel chaos JSON if available
        let barrel_json = barrel_analysis.as_ref().map(|ba| {
            serde_json::json!({
                "missing_barrels": ba.missing_barrels,
                "deep_chains": ba.deep_chains.iter().map(|c| {
                    serde_json::json!({
                        "symbol": c.symbol,
                        "depth": c.depth,
                        "chain": c.chain,
                    })
                }).collect::<Vec<_>>(),
                "inconsistent_paths": ba.inconsistent_paths,
            })
        });

        // Combined output
        let mut output = serde_json::json!({
            "dead_parrots": dead_result.dead_parrots.iter().map(|e| {
                serde_json::json!({
                    "name": e.name,
                    "file": e.file_path,
                    "line": e.line,
                    "kind": e.kind,
                    "import_count": e.import_count,
                })
            }).collect::<Vec<_>>(),
            "exact_twins": twins.iter().map(twin_to_json).collect::<Vec<_>>(),
            "route_twins": route_twins,
            "barrel_chaos": barrel_json,
            "summary": {
                "total_symbols": dead_result.total_symbols,
                "total_files": dead_result.total_files,
                "dead_parrots": dead_count,
                "twin_groups": twin_count,
                "route_twin_groups": route_twin_count,
                "barrel_findings": barrel_count,
                "same_language_groups": same_language_count,
                "cross_language_groups": cross_language_count,
                "high_similarity_groups": high_similarity_count,
            }
        });
        // Bound serialized detail, after classification and top-level paging.
        // These counts describe nested arrays on emitted findings only; they
        // neither spend the global finding budget nor change detection metadata.
        let bound_nested = |item: &mut serde_json::Value, field: &str| -> usize {
            let Some(items) = item[field].as_array_mut() else {
                return 0;
            };
            let total = items.len();
            if let Some(limit) = opts.limit {
                items.truncate(limit);
            }
            let omitted = total - items.len();
            item[format!("{field}_total")] = serde_json::json!(total);
            item[format!("{field}_omitted")] = serde_json::json!(omitted);
            item[format!("{field}_truncated")] = serde_json::json!(omitted > 0);
            omitted
        };
        let mut nested_items_omitted = 0;
        for (pointer, field) in [
            ("/exact_twins", "locations"),
            ("/route_twins", "locations"),
            ("/barrel_chaos/deep_chains", "chain"),
            ("/barrel_chaos/inconsistent_paths", "alternative_paths"),
        ] {
            if let Some(items) = output
                .pointer_mut(pointer)
                .and_then(serde_json::Value::as_array_mut)
            {
                for item in items {
                    nested_items_omitted += bound_nested(item, field);
                }
            }
        }
        output["page"] = serde_json::json!({
            "semantics": "global",
            "limit": opts.limit,
            "offset": opts.offset,
            "returned": returned_findings,
            "total": page_total,
            "has_more": page_has_more(page_total, opts.offset, returned_findings),
            "nested_item_limit": opts.limit,
            "nested_truncated": nested_items_omitted > 0,
            "nested_items_omitted": nested_items_omitted,
        });

        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    } else {
        // Human-readable mode: separate sections
        print_twins_human(&dead_result);

        if !twins.is_empty() {
            print_exact_twins_human(&twins);
        }

        // Route twins surfaced inline with the same human banner shape so
        // operators don't have to chase another command for the runtime
        // contract drift signal.
        print_route_twins_human(&route_twins);

        if let Some(ref ba) = barrel_analysis {
            let has_issues = !ba.missing_barrels.is_empty()
                || !ba.deep_chains.is_empty()
                || !ba.inconsistent_paths.is_empty();

            if has_issues {
                println!("{}", format_barrel_analysis(ba));
            }
        }
        if returned_findings < page_total {
            println!(
                "{} finding(s) outside this --limit/--offset page ({} total eligible)",
                page_total - returned_findings,
                page_total,
            );
        }
    }

    DispatchResult::Exit(0)
}

/// Handle the sniff command - aggregate code smells
pub fn handle_sniff_command(opts: &SniffOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::analyzer::crowd::detect_all_crowds_with_edges;
    use crate::analyzer::health_inputs::canonical_exact_twins_with_sources;
    use crate::analyzer::twins::find_dead_parrots;
    use std::path::Path;

    // Deprecation warning (goes to stderr, won't break piped output)
    warn_deprecated("sniff", "loct findings");

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        Some(Spinner::new("Sniffing for code smells..."))
    } else {
        None
    };

    // Load snapshot (auto-scan if missing)
    let root = opts.path.as_deref().unwrap_or(Path::new("."));

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

    // Filter files based on include_tests
    let files: Vec<_> = if opts.include_tests {
        snapshot.files.clone()
    } else {
        snapshot
            .files
            .iter()
            .filter(|f| !is_test_file(&f.path))
            .cloned()
            .collect()
    };

    // Collect all findings
    let mut twins_count = 0;
    let mut dead_count = 0;
    let mut crowds_count = 0;

    let twins = if !opts.crowds_only && !opts.dead_only {
        // N13 (audit): the sniff surface was the one remaining direct
        // `detect_exact_twins` caller — every twins-reporting surface goes
        // through the canonical helper over the same files/include_tests
        // input, with the snapshot's semantic facts applied when present.
        let t = canonical_exact_twins_with_sources(
            &files,
            snapshot.semantic_facts.as_ref(),
            opts.include_tests,
            &mut crate::analyzer::twins::TwinSources::for_request(&snapshot, root),
        );
        twins_count = t.len();
        Some(t)
    } else {
        None
    };

    let dead_parrots = if !opts.crowds_only && !opts.twins_only {
        let result = find_dead_parrots(&files, false, opts.include_tests);
        dead_count = result.dead_parrots.len();
        Some(result)
    } else {
        None
    };

    let crowds = if !opts.twins_only && !opts.dead_only {
        let mut c = detect_all_crowds_with_edges(&files, &snapshot.edges);
        if let Some(min_size) = opts.min_crowd_size {
            c.retain(|crowd| crowd.members.len() >= min_size);
        }
        crowds_count = c.len();
        Some(c)
    } else {
        None
    };

    let total_smells = twins_count + dead_count + crowds_count;

    if let Some(s) = spinner {
        s.finish_success(&format!("Found {} code smell(s)", total_smells));
    }

    // Output results
    if global.json {
        // JSON output
        let output = serde_json::json!({
            "twins": twins.as_ref().map(|t| t.iter().map(|twin| {
                serde_json::json!({
                    "name": twin.name,
                    "locations": twin.locations.iter().map(|loc| {
                        serde_json::json!({
                            "file": loc.file_path,
                            "line": loc.line,
                            "kind": loc.kind,
                        })
                    }).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>()),
            "dead_parrots": dead_parrots.as_ref().map(|dp| dp.dead_parrots.iter().map(|e| {
                serde_json::json!({
                    "name": e.name,
                    "file": e.file_path,
                    "line": e.line,
                    "kind": e.kind,
                })
            }).collect::<Vec<_>>()),
            "crowds": crowds.as_ref().map(|c| c.iter().map(|crowd| {
                serde_json::json!({
                    "pattern": crowd.pattern,
                    "size": crowd.members.len(),
                    "members": crowd.members.iter().map(|m| m.file.clone()).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>()),
            "summary": {
                "twins": twins_count,
                "dead_parrots": dead_count,
                "crowds": crowds_count,
                "total": total_smells,
            }
        });

        match serde_json::to_string_pretty(&output) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize results: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else {
        // Human-readable output with friendly tone
        println!("🐕 SNIFFING FOR CODE SMELLS...\n");

        // Twins section
        if let Some(ref twins_list) = twins
            && !twins_list.is_empty()
        {
            println!(
                "📍 TWINS (same name, different files) - {} found",
                twins_count
            );
            println!("   Consider: consolidate or rename to avoid import confusion\n");

            for twin in twins_list.iter().take(20) {
                println!("   {} ({} locations)", twin.name, twin.locations.len());
                for loc in &twin.locations {
                    println!("   ├─ {}:{}", loc.file_path, loc.line);
                }
                println!();
            }

            if twins_list.len() > 20 {
                println!(
                    "   {} additional twin group(s) omitted in sniff summary; use `loct findings` for full truth\n",
                    twins_list.len() - 20
                );
            }
        }

        // Dead parrots section
        if let Some(ref dp_result) = dead_parrots
            && !dp_result.dead_parrots.is_empty()
        {
            println!("📍 DEAD PARROTS (unused exports) - {} found", dead_count);
            println!("   Consider: remove if truly unused, or document if external API\n");

            // Group by file
            let mut by_file: std::collections::HashMap<
                String,
                Vec<&crate::analyzer::twins::SymbolEntry>,
            > = std::collections::HashMap::new();
            for entry in &dp_result.dead_parrots {
                by_file
                    .entry(entry.file_path.clone())
                    .or_default()
                    .push(entry);
            }

            let mut files: Vec<_> = by_file.keys().collect();
            files.sort();

            for file in files.iter().take(10) {
                let entries = &by_file[*file];
                for entry in entries.iter().take(3) {
                    println!("   {} in {}:{}", entry.name, entry.file_path, entry.line);
                }
                if entries.len() > 3 {
                    println!(
                        "   {} additional dead export(s) omitted for {} in sniff summary",
                        entries.len() - 3,
                        file
                    );
                }
            }

            if files.len() > 10 {
                println!(
                    "   {} additional file(s) with dead exports omitted in sniff summary",
                    files.len() - 10
                );
            }
            println!();
        }

        // Crowds section
        if let Some(ref crowds_list) = crowds
            && !crowds_list.is_empty()
        {
            println!("📍 CROWDS (similar files) - {} groups", crowds_count);
            println!("   Consider: these files share many dependencies, possible duplication\n");

            for (idx, crowd) in crowds_list.iter().take(5).enumerate() {
                println!("   Group {}: {} pattern", idx + 1, crowd.pattern);
                for member in crowd.members.iter().take(5) {
                    println!("   ├─ {}", member.file);
                }
                if crowd.members.len() > 5 {
                    println!(
                        "   └─ {} additional member(s) omitted in sniff summary",
                        crowd.members.len() - 5
                    );
                }
                println!();
            }

            if crowds_list.len() > 5 {
                println!(
                    "   {} additional crowd group(s) omitted in sniff summary; use `loct findings` for full truth\n",
                    crowds_list.len() - 5
                );
            }
        }

        // Summary
        println!(
            "Summary: {} smells found. These are hints, not verdicts - you decide what matters.",
            total_smells
        );

        if total_smells == 0 {
            println!("\n[OK] No code smells detected - codebase looks clean!");
        }
    }

    DispatchResult::Exit(0)
}

/// Handle the suppress command - manage false positive suppressions
pub fn handle_suppress_command(opts: &SuppressOptions, _global: &GlobalOptions) -> DispatchResult {
    use std::path::Path;

    let root = Path::new(".");

    // Handle --clear
    if opts.clear {
        let mut suppressions = Suppressions::load(root);
        suppressions.clear();
        if let Err(e) = suppressions.save(root) {
            eprintln!("[loct][error] Failed to save suppressions: {}", e);
            return DispatchResult::Exit(1);
        }
        println!("All suppressions cleared.");
        return DispatchResult::Exit(0);
    }

    // Handle --list
    if opts.list {
        let suppressions = Suppressions::load(root);
        if suppressions.items.is_empty() {
            println!("No suppressions configured.");
            println!("Tip: Use `loct suppress twins <symbol>` to suppress false positives.");
        } else {
            println!("Current suppressions ({}):\n", suppressions.len());
            for s in &suppressions.items {
                let file_info = s
                    .file
                    .as_ref()
                    .map(|f| format!(" @ {}", f))
                    .unwrap_or_default();
                let reason_info = s
                    .reason
                    .as_ref()
                    .map(|r| format!(" - {}", r))
                    .unwrap_or_default();
                println!(
                    "  {} {}{}{}  ({})",
                    s.suppression_type, s.symbol, file_info, reason_info, s.added
                );
            }
        }
        return DispatchResult::Exit(0);
    }

    // Handle --remove
    if opts.remove {
        let suppression_type = match opts.suppression_type.as_deref() {
            Some("twins") => SuppressionType::Twins,
            Some("dead_parrot") => SuppressionType::DeadParrot,
            Some("dead_export") => SuppressionType::DeadExport,
            Some("circular") => SuppressionType::Circular,
            Some(other) => {
                eprintln!(
                    "[loct][error] Unknown suppression type '{}'. Valid: twins, dead_parrot, dead_export, circular",
                    other
                );
                return DispatchResult::Exit(1);
            }
            None => {
                eprintln!("[loct][error] --remove requires a type and symbol");
                return DispatchResult::Exit(1);
            }
        };

        let symbol = match &opts.symbol {
            Some(s) => s.clone(),
            None => {
                eprintln!("[loct][error] --remove requires a symbol name");
                return DispatchResult::Exit(1);
            }
        };

        let mut suppressions = Suppressions::load(root);
        if suppressions.remove(&suppression_type, &symbol) {
            if let Err(e) = suppressions.save(root) {
                eprintln!("[loct][error] Failed to save suppressions: {}", e);
                return DispatchResult::Exit(1);
            }
            println!("Removed suppression for {} '{}'", suppression_type, symbol);
        } else {
            println!("No matching suppression found.");
        }
        return DispatchResult::Exit(0);
    }

    // Handle adding a suppression
    let suppression_type = match opts.suppression_type.as_deref() {
        Some("twins") => SuppressionType::Twins,
        Some("dead_parrot") => SuppressionType::DeadParrot,
        Some("dead_export") => SuppressionType::DeadExport,
        Some("circular") => SuppressionType::Circular,
        Some(other) => {
            eprintln!(
                "[loct][error] Unknown suppression type '{}'. Valid: twins, dead_parrot, dead_export, circular",
                other
            );
            return DispatchResult::Exit(1);
        }
        None => {
            eprintln!("[loct][error] Usage: loct suppress <type> <symbol>");
            eprintln!("       loct suppress --list");
            eprintln!("       loct suppress --clear");
            return DispatchResult::Exit(1);
        }
    };

    let symbol = match &opts.symbol {
        Some(s) => s.clone(),
        None => {
            eprintln!(
                "[loct][error] Symbol name required. Usage: loct suppress {} <symbol>",
                suppression_type
            );
            return DispatchResult::Exit(1);
        }
    };

    let mut suppressions = Suppressions::load(root);
    suppressions.add(
        suppression_type.clone(),
        symbol.clone(),
        opts.file.clone(),
        opts.reason.clone(),
    );

    if let Err(e) = suppressions.save(root) {
        eprintln!("[loct][error] Failed to save suppressions: {}", e);
        return DispatchResult::Exit(1);
    }

    let file_info = opts
        .file
        .as_ref()
        .map(|f| format!(" in {}", f))
        .unwrap_or_default();
    println!(
        "Added suppression: {} '{}'{}",
        suppression_type, symbol, file_info
    );
    println!("This finding will be hidden from future runs.");
    println!("Use --include-suppressed to show suppressed items.");

    DispatchResult::Exit(0)
}

#[cfg(test)]
mod tagmap_tests {
    use super::*;
    use crate::tagmap::tagmap_symbol_usage_kind;

    #[test]
    fn g9_context_only_match_is_not_a_primary_hit() {
        // Two usages share one source line whose *context* mentions "pdu"
        // only in a trailing comment; neither usage's *name* has anything to
        // do with "pdu". Before the fix both surfaced as primary
        // "symbol-usage" hits for keyword "pdu" purely because of the shared
        // line text.
        let keyword_lower = "pdu".to_ascii_lowercase();
        let keyword_normalized = tagmap_normalize(&keyword_lower);

        let hit = tagmap_symbol_usage_kind(
            "length",
            "let length = of(pdu); // pdu framing",
            &keyword_lower,
            &keyword_normalized,
        );
        assert_eq!(
            hit,
            Some(("symbol-usage-context", false)),
            "context-only match must be downgraded, not a primary hit: {hit:?}"
        );

        let hit2 = tagmap_symbol_usage_kind(
            "of",
            "let length = of(pdu); // pdu framing",
            &keyword_lower,
            &keyword_normalized,
        );
        assert_eq!(hit2, Some(("symbol-usage-context", false)));
    }

    #[test]
    fn g9_name_match_is_a_primary_hit() {
        let keyword_lower = "pdu".to_ascii_lowercase();
        let keyword_normalized = tagmap_normalize(&keyword_lower);
        let hit = tagmap_symbol_usage_kind(
            "PduHeader",
            "let h = PduHeader::parse(buf);",
            &keyword_lower,
            &keyword_normalized,
        );
        assert_eq!(hit, Some(("symbol-usage", true)));
    }

    #[test]
    fn g9_no_match_returns_none() {
        let keyword_lower = "pdu".to_ascii_lowercase();
        let keyword_normalized = tagmap_normalize(&keyword_lower);
        assert_eq!(
            tagmap_symbol_usage_kind(
                "length",
                "let length = of(x);",
                &keyword_lower,
                &keyword_normalized
            ),
            None
        );
    }
}
