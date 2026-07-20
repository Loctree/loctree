use std::collections::{HashMap, HashSet};
use std::io;

use serde_json::json;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::args::ParsedArgs;
use crate::snapshot::GitContext;
use crate::types::{FileAnalysis, ImportKind, ImportResolutionKind, OutputMode, ReexportKind};

use super::CommandGap;
use super::ReportSection;
use super::barrels::analyze_barrel_chaos;
use super::classify::language_from_path;
use super::coverage_gaps::find_coverage_gaps;
use super::crowd::detect_all_crowds_with_edges;
use super::cycles;
use super::dead_parrots::{DeadFilterConfig, find_dead_exports};
use super::dist::DistResult;
use super::for_ai::{
    HubFile as AiHubFile, PriorityTask as AiPriorityTask, build_priority_tasks, extract_quick_wins,
    find_hub_files,
};
use super::graph::{MAX_GRAPH_EDGES, MAX_GRAPH_NODES, build_graph_data};
use super::health_score::{HealthMetrics, calculate_health_score};
use super::html::render_html_report;
use super::insights::collect_ai_insights;
use super::open_server::current_open_base;
use super::report::{
    AiInsight, CommandBridge, HotspotFile, HubFile, PriorityTask, TreeNode, TwinsData,
};
use super::root_scan::{RootContext, normalize_module_id};
use super::scan::resolve_event_constants_across_files;
use super::twins::{detect_exact_twins, find_dead_parrots};
use super::{DupSeverity, RankedDup};

fn build_tree(analyses: &[FileAnalysis], root_path: &std::path::Path) -> Vec<TreeNode> {
    #[derive(Default)]
    struct TmpNode {
        loc: usize,
        children: std::collections::BTreeMap<String, TmpNode>,
    }

    let mut root = TmpNode::default();
    let mut paths: Vec<(Vec<String>, usize)> = analyses
        .iter()
        .map(|a| {
            let file_path = std::path::Path::new(&a.path);
            // Try to strip root_path prefix for both absolute and relative paths
            let rel = file_path.strip_prefix(root_path).unwrap_or(file_path);
            let parts: Vec<_> = rel
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            (parts, a.loc)
        })
        .collect();
    paths.sort_by(|a, b| a.0.cmp(&b.0));

    for (parts, loc) in paths {
        let mut cursor = &mut root;
        for part in parts {
            let entry = cursor.children.entry(part).or_default();
            cursor = entry;
        }
        cursor.loc = loc;
    }

    fn finalize(name: Option<String>, node: &TmpNode) -> TreeNode {
        let mut loc_sum = node.loc;
        let mut children: Vec<TreeNode> = node
            .children
            .iter()
            .map(|(k, v)| finalize(Some(k.clone()), v))
            .collect();
        for c in &children {
            loc_sum += c.loc;
        }
        children.sort_by(|a, b| a.path.cmp(&b.path));
        TreeNode {
            path: name.unwrap_or_default(),
            loc: loc_sum,
            children,
        }
    }

    root.children
        .iter()
        .map(|(k, v)| finalize(Some(k.clone()), v))
        .collect()
}

fn build_dist_insight(dist: &DistResult) -> AiInsight {
    let severity = if dist.tree_shaken_pct >= 40 {
        "high"
    } else if dist.tree_shaken_exports > 0 {
        "medium"
    } else {
        "low"
    };

    let message = if dist.tree_shaken_exports == 0 {
        format!(
            "Bundle coverage is {}% across {} source map(s); every exported symbol from this scope makes it into at least one bundle.",
            dist.coverage_pct, dist.source_maps
        )
    } else {
        format!(
            "Bundle coverage is {}% across {} source map(s). {} export(s) are fully tree-shaken out of the bundle surface across {} impacted file(s) using {}-level matching.",
            dist.coverage_pct,
            dist.source_maps,
            dist.tree_shaken_exports,
            dist.impacted_files.len(),
            dist.analysis_level.as_str()
        )
    };

    AiInsight {
        title: "Bundle Distribution".to_string(),
        severity: severity.to_string(),
        message,
    }
}

fn score_dist_section(section_root: &str, src_dir: &std::path::Path) -> Option<usize> {
    let candidate = std::fs::canonicalize(section_root)
        .unwrap_or_else(|_| std::path::PathBuf::from(section_root));
    let requested = std::fs::canonicalize(src_dir).unwrap_or_else(|_| src_dir.to_path_buf());

    if requested == candidate {
        Some(usize::MAX)
    } else if requested.starts_with(&candidate) {
        Some(candidate.components().count())
    } else if candidate.starts_with(&requested) {
        Some(requested.components().count())
    } else {
        None
    }
}

pub fn attach_dist_to_sections(
    sections: &mut Vec<ReportSection>,
    dist: DistResult,
    src_dir: &std::path::Path,
) {
    let target_idx = sections
        .iter()
        .enumerate()
        .filter_map(|(idx, section)| {
            score_dist_section(&section.root, src_dir).map(|score| (idx, score))
        })
        .max_by_key(|(_, score)| *score)
        .map(|(idx, _)| idx)
        .or_else(|| (!sections.is_empty()).then_some(0));

    let insight = build_dist_insight(&dist);

    if let Some(idx) = target_idx {
        let section = &mut sections[idx];
        if !section
            .insights
            .iter()
            .any(|existing| existing.title == insight.title)
        {
            section.insights.insert(0, insight);
        }
        section.dist = Some(dist);
        return;
    }

    sections.push(ReportSection {
        insights: vec![insight],
        root: src_dir.display().to_string(),
        files_analyzed: 0,
        total_loc: 0,
        reexport_files_count: 0,
        dynamic_imports_count: 0,
        ranked_dups: Vec::new(),
        cascades: Vec::new(),
        circular_imports: Vec::new(),
        lazy_circular_imports: Vec::new(),
        dynamic: Vec::new(),
        analyze_limit: 0,
        generated_at: None,
        schema_name: None,
        schema_version: None,
        loctree_version: None,
        missing_handlers: Vec::new(),
        unregistered_handlers: Vec::new(),
        unused_handlers: Vec::new(),
        command_counts: (0, 0),
        command_bridges: Vec::new(),
        open_base: None,
        tree: None,
        graph: None,
        graph_warning: None,
        git_branch: None,
        git_commit: None,
        priority_tasks: Vec::new(),
        hub_files: Vec::new(),
        hotspots: Vec::new(),
        crowds: Vec::new(),
        dead_exports: Vec::new(),
        dist: Some(dist),
        twins_data: None,
        coverage_gaps: Vec::new(),
        health_score: None,
        refactor_plan: None,
        context_atlas: None,
    });
}

/// Build edges for cycle detection even when graph collection is disabled.
/// Falls back to resolved imports/re-exports in analyses.
fn build_cycle_edges(
    graph_edges: &[(String, String, String)],
    analyses: &[FileAnalysis],
) -> Vec<(String, String, String)> {
    if !graph_edges.is_empty() {
        return graph_edges.to_vec();
    }

    let mut edges = Vec::new();
    for analysis in analyses {
        for imp in &analysis.imports {
            if let Some(target) = &imp.resolved_path {
                let kind = if imp.is_mod_declaration {
                    // Rust `mod foo;` declarations create parent->child module relationships
                    // These are NOT import edges and should not contribute to cycle detection
                    "mod"
                } else if imp.is_type_checking {
                    "type_import"
                } else if imp.is_lazy {
                    "lazy_import"
                } else {
                    match imp.kind {
                        ImportKind::Dynamic => "dynamic_import",
                        _ => "import",
                    }
                };
                if kind != "type_import" {
                    edges.push((analysis.path.clone(), target.clone(), kind.to_string()));
                }
            }
        }

        for reexport in &analysis.reexports {
            if let Some(target) = &reexport.resolved {
                edges.push((
                    analysis.path.clone(),
                    target.clone(),
                    "reexport".to_string(),
                ));
            }
        }
    }

    edges
}

pub struct GlobalContext<'a> {
    pub fe_commands: &'a HashMap<String, Vec<(String, usize, String)>>,
    pub be_commands: &'a HashMap<String, Vec<(String, usize, String)>>,
    pub missing_handlers: &'a [CommandGap],
    pub unregistered_handlers: &'a [CommandGap],
    pub unused_handlers: &'a [CommandGap],
    pub pipeline_summary: &'a serde_json::Value,
    pub git: Option<&'a GitContext>,
    pub schema_name: &'a str,
    pub schema_version: &'a str,
    pub analyses: &'a [FileAnalysis],
}

pub struct RootArtifacts {
    pub json_items: Vec<serde_json::Value>,
    pub report_section: Option<ReportSection>,
}

pub fn process_root_context(
    idx: usize,
    ctx: RootContext,
    parsed: &ParsedArgs,
    global: &GlobalContext,
) -> RootArtifacts {
    let mut json_items = Vec::new();
    let RootContext {
        root_path,
        options: _options,
        mut analyses,
        export_index,
        dynamic_summary,
        cascades,
        filtered_ranked,
        graph_edges,
        loc_map,
        languages,
        tsconfig_summary,
        calls_with_generics,
        renamed_handlers,
        barrels,
        ..
    } = ctx;

    let pipeline_summary = global.pipeline_summary.clone();

    resolve_event_constants_across_files(&mut analyses);

    let analysis_by_path: HashMap<String, FileAnalysis> = analyses
        .iter()
        .map(|a| (a.path.clone(), a.clone()))
        .collect();

    let _duplicate_exports: Vec<_> = export_index
        .into_iter()
        .filter(|(_, files)| files.len() > 1)
        .collect();

    let reexport_files: HashSet<String> = analyses
        .iter()
        .filter(|a| !a.reexports.is_empty())
        .map(|a| a.path.clone())
        .collect();

    let missing_handlers = global.missing_handlers.to_vec();
    let unregistered_handlers = global.unregistered_handlers.to_vec();
    let unused_handlers = global.unused_handlers.to_vec();

    let (graph_data, graph_warning) = if parsed.graph && parsed.report_path.is_some() {
        build_graph_data(
            &analyses,
            &graph_edges,
            &loc_map,
            global.fe_commands,
            global.be_commands,
            parsed.max_graph_nodes.unwrap_or(MAX_GRAPH_NODES),
            parsed.max_graph_edges.unwrap_or(MAX_GRAPH_EDGES),
        )
    } else {
        (None, None)
    };

    let cycle_edges = build_cycle_edges(&graph_edges, &analyses);
    let (circular_imports, lazy_circular_imports) = cycles::find_cycles_with_lazy(&cycle_edges);

    // Detect crowds (naming collision patterns)
    // Convert cycle_edges to GraphEdge format for crowd detection
    let graph_edges_for_crowd: Vec<crate::snapshot::GraphEdge> = cycle_edges
        .iter()
        .map(|(from, to, label)| crate::snapshot::GraphEdge {
            from: from.clone(),
            to: to.clone(),
            label: label.clone(),
        })
        .collect();
    let crowds = detect_all_crowds_with_edges(&analyses, &graph_edges_for_crowd);

    let mut sorted_paths: Vec<String> = analyses.iter().map(|a| a.path.clone()).collect();
    sorted_paths.sort();
    let file_id_map: HashMap<String, usize> = sorted_paths
        .iter()
        .enumerate()
        .map(|(idx, p)| (p.clone(), idx + 1))
        .collect();

    let mut imports_targeted: HashSet<String> = HashSet::new();
    let mut files_json: Vec<_> = Vec::new();
    let mut casing_issues: Vec<serde_json::Value> = Vec::new();
    let mut dead_symbols_total = 0usize;
    for path in &sorted_paths {
        if let Some(a) = analysis_by_path.get(path) {
            let mut imports = a.imports.clone();
            imports.sort_by(|x, y| x.source.cmp(&y.source));

            let mut reexports = a.reexports.clone();
            reexports.sort_by(|x, y| x.source.cmp(&y.source));

            let mut exports = a.exports.clone();
            exports.sort_by(|x, y| x.name.cmp(&y.name));

            let mut command_calls = a.command_calls.clone();
            command_calls.sort_by(|x, y| x.line.cmp(&y.line).then(x.name.cmp(&y.name)));

            let mut command_handlers = a.command_handlers.clone();
            command_handlers.sort_by(|x, y| x.line.cmp(&y.line).then(x.name.cmp(&y.name)));

            let mut event_emits = a.event_emits.clone();
            event_emits.sort_by(|x, y| x.line.cmp(&y.line).then(x.name.cmp(&y.name)));

            let mut event_listens = a.event_listens.clone();
            event_listens.sort_by(|x, y| x.line.cmp(&y.line).then(x.name.cmp(&y.name)));

            for imp in &imports {
                if let Some(resolved) = &imp.resolved_path {
                    imports_targeted.insert(resolved.clone());
                    imports_targeted.insert(normalize_module_id(resolved).as_key());
                }
            }
            for re in &reexports {
                if let Some(resolved) = &re.resolved {
                    imports_targeted.insert(resolved.clone());
                    imports_targeted.insert(normalize_module_id(resolved).as_key());
                } else {
                    imports_targeted.insert(normalize_module_id(&re.source).as_key());
                    imports_targeted.insert(re.source.clone());
                }
            }
            for dyn_imp in &a.dynamic_imports {
                imports_targeted.insert(dyn_imp.clone());
                imports_targeted.insert(normalize_module_id(dyn_imp).as_key());
            }

            for issue in &a.command_payload_casing {
                casing_issues.push(json!({
                    "command": issue.command,
                    "key": issue.key,
                    "path": issue.path,
                    "line": issue.line,
                }));
            }

            files_json.push(json!({
                "id": file_id_map.get(&a.path).cloned().unwrap_or(0),
                "path": a.path,
                "loc": a.loc,
                "language": a.language,
                "kind": a.kind,
                "isTest": a.is_test,
                "isGenerated": a.is_generated,
                "imports": imports.iter().map(|i| json!({
                    "source": i.source,
                    "sourceRaw": i.source_raw,
                    "kind": match i.kind {
                        ImportKind::Static => "static",
                        ImportKind::Type => "type",
                        ImportKind::SideEffect => "side-effect",
                        ImportKind::Dynamic => "dynamic",
                    },
                    "resolvedPath": i.resolved_path,
                    "isBareModule": i.is_bare,
                    "resolutionKind": match i.resolution {
                        ImportResolutionKind::Local => "local",
                        ImportResolutionKind::Stdlib => "stdlib",
                        ImportResolutionKind::Dynamic => "dynamic",
                        ImportResolutionKind::Unknown => "unknown",
                    },
                    "isTypeChecking": i.is_type_checking,
                    "symbols": i.symbols.iter().map(|s| json!({"name": s.name, "alias": s.alias})).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "reexports": reexports.iter().map(|r| {
                    match &r.kind {
                        ReexportKind::Star => json!({"source": r.source, "kind": "star", "resolved": r.resolved}),
                        ReexportKind::Named(names) => json!({"source": r.source, "kind": "named", "names": names, "resolved": r.resolved})
                    }
                }).collect::<Vec<_>>(),
                "dynamicImports": a.dynamic_imports,
                "exports": exports.iter().map(|e| json!({
                    "name": e.name,
                    "kind": e.kind,
                    "exportType": e.export_type,
                    "line": e.line,
                })).collect::<Vec<_>>(),
                "commandCalls": command_calls.iter().map(|c| json!({
                    "name": c.name,
                    "line": c.line,
                    "genericType": c.generic_type,
                    "payload": c.payload,
                })).collect::<Vec<_>>(),
                "commandHandlers": command_handlers.iter().map(|c| json!({
                    "name": c.name,
                    "line": c.line,
                    "exposedName": c.exposed_name,
                    "payload": c.payload,
                })).collect::<Vec<_>>(),
                "events": {
                    "emit": event_emits.iter().map(|e| json!({
                        "name": e.name,
                        "rawName": e.raw_name,
                        "line": e.line,
                        "kind": e.kind,
                        "payload": e.payload,
                        "awaited": e.awaited,
                    })).collect::<Vec<_>>(),
                    "listen": event_listens.iter().map(|e| json!({
                        "name": e.name,
                        "rawName": e.raw_name,
                        "line": e.line,
                        "kind": e.kind,
                        "payload": e.payload,
                        "awaited": e.awaited,
                    })).collect::<Vec<_>>(),
                },
            }));
        }
    }

    let mut languages_vec: Vec<_> = languages.iter().cloned().collect();
    languages_vec.sort();

    let mut all_command_names: Vec<String> = global
        .fe_commands
        .keys()
        .chain(global.be_commands.keys())
        .cloned()
        .collect();
    all_command_names.sort();
    all_command_names.dedup();

    let missing_set: HashSet<String> = missing_handlers.iter().map(|g| g.name.clone()).collect();
    let unregistered_set: HashSet<String> = unregistered_handlers
        .iter()
        .map(|g| g.name.clone())
        .collect();
    let unused_set: HashSet<String> = unused_handlers.iter().map(|g| g.name.clone()).collect();

    let mut commands2 = Vec::new();
    for name in &all_command_names {
        let mut handlers = global.be_commands.get(name).cloned().unwrap_or_default();
        handlers.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let canonical = handlers.first().map(|(path, line, symbol)| {
            json!({
                "path": path,
                "line": line,
                "symbol": symbol,
                "language": language_from_path(path),
            })
        });

        let mut call_sites = global.fe_commands.get(name).cloned().unwrap_or_default();
        call_sites.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

        let language = canonical
            .as_ref()
            .and_then(|c| c.get("language").and_then(|v| v.as_str()))
            .map(|s| s.to_string())
            .or_else(|| {
                call_sites
                    .first()
                    .map(|(path, _, _)| language_from_path(path))
            })
            .unwrap_or_default();

        let status = if missing_set.contains(name) {
            "missing_handler"
        } else if unused_set.contains(name) {
            "unused_handler"
        } else if unregistered_set.contains(name) {
            "unregistered_handler"
        } else {
            "ok"
        };

        commands2.push(json!({
            "name": name,
            "kind": if canonical.is_some() { "tauri_command" } else { "custom" },
            "language": language,
            "canonicalLocation": canonical,
            "callSites": call_sites.iter().map(|(path, line, symbol)| json!({
                "path": path,
                "line": line,
                "symbol": symbol,
                "language": language_from_path(path),
            })).collect::<Vec<_>>(),
            "status": status,
        }));
    }

    // Build command_bridges for full FE↔BE comparison table
    let mut command_bridges: Vec<CommandBridge> = Vec::new();
    for name in &all_command_names {
        let mut handlers = global.be_commands.get(name).cloned().unwrap_or_default();
        handlers.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let be_location = handlers
            .first()
            .map(|(path, line, symbol)| (path.clone(), *line, symbol.clone()));

        let mut call_sites = global.fe_commands.get(name).cloned().unwrap_or_default();
        call_sites.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let fe_locations: Vec<(String, usize)> = call_sites
            .iter()
            .map(|(path, line, _)| (path.clone(), *line))
            .collect();

        let language = be_location
            .as_ref()
            .map(|(path, _, _)| language_from_path(path))
            .or_else(|| {
                call_sites
                    .first()
                    .map(|(path, _, _)| language_from_path(path))
            })
            .unwrap_or_default();

        let status = if missing_set.contains(name) {
            "missing_handler"
        } else if unused_set.contains(name) {
            "unused_handler"
        } else if unregistered_set.contains(name) {
            "unregistered_handler"
        } else {
            "ok"
        };

        // Check if the handler file emits any events
        let (comm_type, emits_events) = if let Some((handler_path, _, _)) = &be_location {
            if let Some(handler_analysis) = analysis_by_path.get(handler_path) {
                let events: Vec<String> = handler_analysis
                    .event_emits
                    .iter()
                    .map(|e| e.name.clone())
                    .collect();
                if events.is_empty() {
                    ("invoke".to_string(), vec![])
                } else {
                    ("invoke+emit".to_string(), events)
                }
            } else {
                ("invoke".to_string(), vec![])
            }
        } else {
            ("invoke".to_string(), vec![])
        };

        command_bridges.push(CommandBridge {
            name: name.clone(),
            fe_locations,
            be_location,
            status: status.to_string(),
            language,
            comm_type,
            emits_events,
        });
    }

    let dup_score_map: HashMap<String, &RankedDup> = filtered_ranked
        .iter()
        .map(|d| (d.name.clone(), d))
        .collect();

    type SymbolOccurrence = (String, String, String, Option<usize>, String);
    let mut symbol_occurrences: HashMap<String, Vec<SymbolOccurrence>> = HashMap::new();
    for analysis in &analyses {
        for exp in &analysis.exports {
            if exp.kind == "reexport" {
                continue;
            }
            if analysis.is_test {
                continue;
            }
            // Exclude test fixtures from duplicate reports
            if super::classify::should_exclude_from_reports(&analysis.path) {
                continue;
            }
            if exp.export_type == "default" {
                continue;
            }
            let norm_path = normalize_module_id(&analysis.path).as_key();
            let entry = symbol_occurrences.entry(exp.name.clone()).or_default();
            let already_present = entry.iter().any(|(_, _, _, _, norm)| norm == &norm_path);
            if already_present {
                continue;
            }
            entry.push((
                analysis.path.clone(),
                exp.export_type.clone(),
                exp.kind.clone(),
                exp.line,
                norm_path,
            ));
        }
    }

    let mut symbols_json = Vec::new();
    let mut clusters_json = Vec::new();
    let mut sorted_symbol_names: Vec<_> = symbol_occurrences.keys().cloned().collect();
    sorted_symbol_names.sort();

    for name in &sorted_symbol_names {
        if let Some(occ_list) = symbol_occurrences.get(name) {
            let canonical_idx = occ_list
                .iter()
                .enumerate()
                .find(|(_, (path, _, _, _, _))| {
                    analysis_by_path
                        .get(path)
                        .map(|a| !a.is_test && !a.is_generated)
                        .unwrap_or(false)
                })
                .map(|(idx, _)| idx)
                .unwrap_or(0);

            let mut occurrences_json = Vec::new();
            let mut occurrence_ids = Vec::new();
            for (idx, (path, export_type, kind, line, norm_path)) in occ_list.iter().enumerate() {
                let analysis_meta = analysis_by_path.get(path);
                let id = format!("symbol:{}#{}", name, idx + 1);
                occurrence_ids.push(id.clone());
                occurrences_json.push(json!({
                    "id": id,
                    "fileId": file_id_map.get(path).cloned().unwrap_or(0),
                    "path": path,
                    "exportType": export_type,
                    "kind": kind,
                    "line": line,
                    "isCanonical": idx == canonical_idx,
                    "viaReexport": kind == "reexport",
                    "isTestFile": analysis_meta.map(|a| a.is_test).unwrap_or(false),
                    "isGenerated": analysis_meta.map(|a| a.is_generated).unwrap_or(false),
                    "normalizedPath": norm_path,
                }));
            }

            let canonical_path = occ_list
                .get(canonical_idx)
                .map(|(p, _, _, _, _)| p.clone())
                .unwrap_or_default();
            let public_surface = canonical_path.ends_with("index.ts")
                || canonical_path.ends_with("index.tsx")
                || canonical_path.ends_with("mod.rs")
                || canonical_path.ends_with("lib.rs");

            let score = dup_score_map
                .get(name)
                .map(|d| d.score)
                .unwrap_or(occ_list.len());
            let mut severity = if occ_list.len() > 5 {
                "high"
            } else if occ_list.len() > 2 {
                "medium"
            } else {
                "low"
            };
            if public_surface && occ_list.len() > 1 {
                severity = "high";
            }
            let reason = if occ_list.len() == 1 {
                "single_export"
            } else if occ_list.iter().any(|(_, _, kind, _, _)| kind == "reexport") {
                "reexport_chain"
            } else {
                "multiple_exports"
            };

            symbols_json.push(json!({
                "id": format!("symbol:{}", name),
                "name": name,
                "occurrences": occurrences_json,
                "duplicateScore": score,
                "severity": severity,
                "reason": reason,
                "publicSurface": public_surface,
            }));

            if occ_list.len() > 1 {
                clusters_json.push(json!({
                    "symbolName": name,
                    "symbolId": format!("symbol:{}", name),
                    "occurrenceIds": occurrence_ids,
                    "canonicalOccurrenceId": format!("symbol:{}#{}", name, canonical_idx + 1),
                    "size": occ_list.len(),
                    "severity": severity,
                    "reason": reason,
                    "publicSurface": public_surface,
                }));
            }
        }
    }

    let mut default_export_chains: Vec<_> = cascades
        .iter()
        .map(|(from, to)| json!({"chain": [from, to], "length": 2}))
        .collect();
    default_export_chains.sort_by(|a, b| a["chain"].to_string().cmp(&b["chain"].to_string()));

    let barrels_json: Vec<_> = barrels
        .iter()
        .map(|b| {
            json!({
                "path": b.path,
                "module": b.module_id,
                "reexportCount": b.reexport_count,
                "targetCount": b.target_count,
                "mixed": b.mixed,
                "targets": b.targets,
            })
        })
        .collect();

    let mut suspicious_barrels = Vec::new();
    for b in &barrels {
        if b.mixed || b.reexport_count >= 20 || b.target_count >= 12 {
            let dup_in_cluster = symbol_occurrences
                .iter()
                .filter(|(_, occs)| {
                    occs.len() > 1 && occs.iter().any(|(path, _, _, _, _)| path == &b.path)
                })
                .count();
            suspicious_barrels.push(json!({
                "path": b.path,
                "module": b.module_id,
                "reexportCount": b.reexport_count,
                "targetCount": b.target_count,
                "mixed": b.mixed,
                "duplicatesInClusterCount": dup_in_cluster,
            }));
        }
    }
    suspicious_barrels.sort_by(|a, b| {
        let a_path = a["path"].as_str().unwrap_or("");
        let b_path = b["path"].as_str().unwrap_or("");
        a_path.cmp(b_path)
    });

    // Use the canonical find_dead_exports() which includes:
    // - Transitive reachability from dynamic imports (React.lazy, Next.js dynamic)
    // - imported_by_name fallback for $lib/, @scope/ aliases
    // - Skip patterns for framework entry points, .d.ts, configs, tests
    let mut dead_symbols = Vec::new();
    let dead_exports_for_report = if !parsed.skip_dead_symbols {
        let open_base = current_open_base();
        let mut dead_ok_globs: Vec<String> = parsed
            .root_list
            .iter()
            .flat_map(|root| crate::fs_utils::load_loctignore_dead_ok_globs(root))
            .collect();
        dead_ok_globs.sort();
        dead_ok_globs.dedup();
        let dead_exports = find_dead_exports(
            global.analyses,
            true,
            open_base.as_deref(),
            DeadFilterConfig {
                include_tests: parsed.with_tests,
                include_helpers: parsed.with_helpers,
                library_mode: parsed.library_mode,
                example_globs: parsed.library_example_globs.clone(),
                python_library_mode: parsed.python_library,
                include_ambient: false,
                include_dynamic: false,
                dead_ok_globs,
            },
        );

        // Convert DeadExport results to the JSON format expected by -A mode
        // Group by symbol name since old algorithm grouped by name
        let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
        for de in &dead_exports {
            by_name
                .entry(de.symbol.clone())
                .or_default()
                .push(de.file.clone());
        }

        for (name, mut paths) in by_name {
            paths.sort();
            paths.dedup();
            let public_surface = paths.iter().any(|p| {
                p.ends_with("index.ts")
                    || p.ends_with("index.tsx")
                    || p.ends_with("mod.rs")
                    || p.ends_with("lib.rs")
            });
            dead_symbols
                .push(json!({"name": name, "paths": paths, "publicSurface": public_surface}));
        }

        dead_symbols.sort_by(|a, b| {
            let a_name = a["name"].as_str().unwrap_or("");
            let b_name = b["name"].as_str().unwrap_or("");
            a_name.cmp(b_name)
        });
        dead_symbols_total = dead_symbols.len();
        dead_symbols.truncate(parsed.top_dead_symbols);

        // Keep the original dead_exports for the report
        dead_exports
    } else {
        Vec::new()
    };

    // Run twins analysis (dead parrots, exact twins, barrel chaos)
    let twins_data = if !parsed.skip_dead_symbols {
        // Find dead parrots (0 imports)
        // Note: include_tests=false for production reports
        let twins_result = find_dead_parrots(&analyses, true, false);

        // Detect exact twins (same symbol exported from multiple files)
        let exact_twins = detect_exact_twins(&analyses, false);

        // Analyze barrel chaos (missing barrels, deep chains, inconsistent paths)
        // Build snapshot from analyses for barrel analysis
        let snapshot_barrels: Vec<crate::snapshot::BarrelFile> = barrels
            .iter()
            .map(|b| crate::snapshot::BarrelFile {
                path: b.path.clone(),
                module_id: b.module_id.clone(),
                reexport_count: b.reexport_count,
                targets: b.targets.clone(),
            })
            .collect();

        let snapshot = crate::snapshot::Snapshot {
            metadata: crate::snapshot::SnapshotMetadata {
                roots: vec![root_path.display().to_string()],
                languages: languages.clone(),
                file_count: analyses.len(),
                total_loc: analyses.iter().map(|a| a.loc).sum(),
                ..Default::default()
            },
            files: analyses.clone(),
            edges: graph_edges
                .iter()
                .map(|(from, to, label)| crate::snapshot::GraphEdge {
                    from: from.clone(),
                    to: to.clone(),
                    label: label.clone(),
                })
                .collect(),
            export_index: std::collections::HashMap::new(),
            command_bridges: Vec::new(),
            event_bridges: Vec::new(),
            barrels: snapshot_barrels,
            semantic_facts: None,
            symbol_graph: None,
        };
        let barrel_analysis = analyze_barrel_chaos(&snapshot);

        // Use the types directly from twins and barrels modules
        Some(TwinsData {
            dead_parrots: twins_result.dead_parrots,
            exact_twins,
            barrel_chaos: barrel_analysis,
        })
    } else {
        None
    };

    let duplicate_clusters_count = clusters_json.len();
    let max_cluster_size = symbol_occurrences
        .values()
        .map(|v| v.len())
        .max()
        .unwrap_or(0);
    let mut top_clusters = Vec::new();
    let mut sorted_by_size: Vec<_> = symbol_occurrences
        .iter()
        .filter(|(_, v)| v.len() > 1)
        .collect();
    sorted_by_size.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    for (name, occs) in sorted_by_size.into_iter().take(5) {
        let severity = if occs.len() > 5 {
            "high"
        } else if occs.len() > 2 {
            "medium"
        } else {
            "low"
        };
        top_clusters.push(json!({
            "symbolName": name,
            "size": occs.len(),
            "severity": severity,
        }));
    }

    let mut dynamic_imports_json = Vec::new();
    for (file, sources) in &dynamic_summary {
        let unique: HashSet<_> = sources.iter().collect();
        dynamic_imports_json.push(json!({
            "file": file,
            "sources": sources,
            "manySources": sources.len() > 5,
            "selfImport": unique.len() < sources.len(),
        }));
    }

    // Visibility toggles (noise reduction)
    let duplicates_hidden = parsed.suppress_duplicates;
    let dynamic_hidden = parsed.suppress_dynamic;
    let filtered_ranked_for_output = if duplicates_hidden {
        Vec::new()
    } else {
        filtered_ranked.clone()
    };
    let clusters_json_for_output = if duplicates_hidden {
        Vec::new()
    } else {
        clusters_json.clone()
    };
    let top_clusters_for_output = if duplicates_hidden {
        Vec::new()
    } else {
        top_clusters.clone()
    };
    let duplicate_clusters_count_for_output = if duplicates_hidden {
        0
    } else {
        duplicate_clusters_count
    };
    let max_cluster_size_for_output = if duplicates_hidden {
        0
    } else {
        max_cluster_size
    };

    let dynamic_imports_json_for_output = if dynamic_hidden {
        Vec::new()
    } else {
        dynamic_imports_json.clone()
    };
    let dynamic_summary_for_output = if dynamic_hidden {
        Vec::new()
    } else {
        dynamic_summary.clone()
    };

    let generated_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| String::new());

    let ghost_events: Vec<_> = pipeline_summary["events"]["ghostEmits"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let orphan_listeners: Vec<_> = pipeline_summary["events"]["orphanListeners"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let pipeline_risks: Vec<_> = pipeline_summary["risks"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let bridge_limit = parsed.summary_limit.max(50);
    let barrel_limit = parsed.summary_limit.max(50);
    let mut bridges_for_ai = Vec::new();
    for cmd in commands2.iter().take(bridge_limit) {
        bridges_for_ai.push(cmd.clone());
    }

    if matches!(parsed.output, OutputMode::Json | OutputMode::Jsonl) {
        if parsed.ai_mode {
            let top_limit = parsed.summary_limit;
            let mut event_alerts = Vec::new();
            for item in ghost_events.iter().take(top_limit) {
                event_alerts.push(json!({
                    "type": "ghost_event",
                    "name": item.get("name"),
                    "path": item.get("path"),
                    "line": item.get("line"),
                }));
            }
            for item in orphan_listeners.iter().take(top_limit) {
                event_alerts.push(json!({
                    "type": "orphan_listener",
                    "name": item.get("name"),
                    "path": item.get("path"),
                    "line": item.get("line"),
                    "awaited": item.get("awaited"),
                }));
            }

            let ai_payload = json!({
                "schema": global.schema_name,
                "schemaVersion": global.schema_version,
                "generatedAt": generated_at,
                "rootDir": root_path,
                "git": {
                    "repo": global.git.and_then(|g| g.repo.clone()),
                    "branch": global.git.and_then(|g| g.branch.clone()),
                    "commit": global.git.and_then(|g| g.commit.clone()),
                    "scanId": global.git.and_then(|g| g.scan_id.clone()),
                },
                "languages": languages_vec,
                "filesAnalyzed": analyses.len(),
                "summary": {
                    "duplicateExports": filtered_ranked_for_output.len(),
                    "reexportFiles": reexport_files.len(),
                    "dynamicImports": dynamic_summary_for_output.len(),
                "commands": {
                    "frontendCalls": global.fe_commands.len(),
                    "backendHandlers": global.be_commands.len(),
                    "missingHandlers": missing_handlers.len(),
                    "unusedHandlers": unused_handlers.len(),
                },
                    "events": {
                        "ghost": ghost_events.len(),
                        "orphan": orphan_listeners.len(),
                        "risks": pipeline_risks.len(),
                    },
                    "clusters": {
                        "duplicateCount": duplicate_clusters_count_for_output,
                        "maxClusterSize": max_cluster_size_for_output,
                    },
                    "barrels": {
                        "count": barrels.len(),
                        "mixed": barrels.iter().filter(|b| b.mixed).count(),
                    },
                },
                "topIssues": {
                    "duplicateExports": filtered_ranked_for_output.iter().take(top_limit).map(|dup| json!({
                        "name": dup.name,
                        "canonical": dup.canonical,
                        "canonicalLine": dup.canonical_line,
                        "locations": dup.locations.iter().map(|loc| json!({
                            "file": loc.file,
                            "line": loc.line,
                        })).collect::<Vec<_>>(),
                        "refactorTargets": dup.refactors,
                        "score": dup.score,
                    })).collect::<Vec<_>>(),
                    "missingHandlers": missing_handlers.iter().take(top_limit).map(|g| json!({
                        "name": g.name,
                        "locations": g.locations,
                    })).collect::<Vec<_>>(),
                    "unusedHandlers": unused_handlers.iter().take(top_limit).map(|g| {
                        let mut obj = json!({
                            "name": g.name,
                            "locations": g.locations,
                        });
                        if let Some(conf) = &g.confidence {
                            obj["confidence"] = json!(conf.to_string());
                        }
                        if !g.string_literal_matches.is_empty() {
                            obj["stringLiteralMatches"] = json!(g.string_literal_matches.len());
                        }
                        obj
                    }).collect::<Vec<_>>(),
                    "events": event_alerts,
                    "pipelineRisks": pipeline_risks.iter().take(top_limit).cloned().collect::<Vec<_>>(),
                    "deadSymbols": dead_symbols.iter().take(parsed.top_dead_symbols).cloned().collect::<Vec<_>>(),
                    "duplicateClusters": top_clusters_for_output,
                    "bridges": bridges_for_ai,
                    "barrels": barrels_json.iter().take(barrel_limit).cloned().collect::<Vec<_>>(),
                },
                "limits": {
                    "topItems": top_limit,
                    "topDeadSymbols": parsed.top_dead_symbols,
                    "bridges": bridge_limit,
                    "barrels": barrel_limit,
                }
            });

            if matches!(parsed.output, OutputMode::Jsonl) {
                if let Ok(line) = serde_json::to_string(&ai_payload) {
                    println!("{}", line);
                } else {
                    eprintln!("[loctree][warn] failed to serialize JSONL line for AI payload");
                }
            } else {
                json_items.push(ai_payload);
            }
        } else {
            let payload = json!({
                "schema": global.schema_name,
                "schemaVersion": global.schema_version,
                "generatedAt": generated_at,
                "rootDir": root_path,
                "root": root_path,
                "git": {
                    "repo": global.git.and_then(|g| g.repo.clone()),
                    "branch": global.git.and_then(|g| g.branch.clone()),
                    "commit": global.git.and_then(|g| g.commit.clone()),
                    "scanId": global.git.and_then(|g| g.scan_id.clone()),
                },
                "languages": languages_vec,
                "filesAnalyzed": analyses.len(),
                "duplicateExports": filtered_ranked_for_output
                    .iter()
                    .map(|dup| json!({
                        "name": dup.name,
                        "files": dup.files,
                        "locations": dup.locations,
                    }))
                    .collect::<Vec<_>>(),
                "duplicateExportsRanked": filtered_ranked_for_output
                    .iter()
                    .map(|dup| json!({
                        "name": dup.name,
                        "files": dup.files,
                        "locations": dup.locations,
                        "score": dup.score,
                        "nonDevCount": dup.prod_count,
                        "devCount": dup.dev_count,
                        "canonical": dup.canonical,
                        "canonicalLine": dup.canonical_line,
                        "refactorTargets": dup.refactors,
                        "severity": dup.severity,
                        "isCrossLang": dup.is_cross_lang,
                        "packages": dup.packages,
                        "reason": dup.reason,
                    }))
                    .collect::<Vec<_>>(),
                "reexportCascades": cascades
                    .iter()
                    .map(|(from, to)| json!({"from": from, "to": to}))
                    .collect::<Vec<_>>(),
                "barrels": barrels_json,
                "dynamicImports": dynamic_imports_json_for_output,
                "commands": {
                    "frontend": global.fe_commands.iter().map(|(k,v)| json!({"name": k, "locations": v})).collect::<Vec<_>>(),
                    "backend": global.be_commands.iter().map(|(k,v)| json!({"name": k, "locations": v})).collect::<Vec<_>>(),
                    "missingHandlers": missing_handlers.iter().map(|g| json!({"name": g.name, "locations": g.locations})).collect::<Vec<_>>(),
                    "unusedHandlers": unused_handlers.iter().map(|g| {
                        let mut obj = json!({"name": g.name, "locations": g.locations});
                        if let Some(conf) = &g.confidence {
                            obj["confidence"] = json!(conf.to_string());
                        }
                        if !g.string_literal_matches.is_empty() {
                            obj["stringLiteralMatches"] = json!(g.string_literal_matches.iter().map(|m| {
                                json!({"file": m.file, "line": m.line, "context": m.context})
                            }).collect::<Vec<_>>());
                        }
                        obj
                    }).collect::<Vec<_>>(),
                    "payloadCasing": casing_issues,
                },
                "commands2": commands2,
                "tauri_analysis": {
                    "total_handlers": global.be_commands.len(),
                    "total_calls": global.fe_commands.len(),
                    "registered": global.be_commands.len().saturating_sub(unregistered_handlers.len()),
                    "coverage": {
                        "ok": all_command_names.len().saturating_sub(
                            missing_handlers.len() + unused_handlers.len() + unregistered_handlers.len()
                        ),
                        "missing_handler": missing_handlers.len(),
                        "unused_handler": unused_handlers.len(),
                        "unregistered_handler": unregistered_handlers.len(),
                    },
                    "missing_handlers": missing_handlers.iter().map(|g| &g.name).collect::<Vec<_>>(),
                    "unused_handlers": unused_handlers.iter().map(|g| &g.name).collect::<Vec<_>>(),
                    "unregistered_handlers": unregistered_handlers.iter().map(|g| &g.name).collect::<Vec<_>>(),
                },
                "symbols": symbols_json,
                "clusters": clusters_json_for_output,
                "pipeline": pipeline_summary,
                "aiViews": {
                    "defaultExportChains": default_export_chains,
                    "suspiciousBarrels": suspicious_barrels,
                    "deadSymbols": dead_symbols,
                    "coverage": {
                        "frontendCommandCount": global.fe_commands.len(),
                        "backendHandlerCount": global.be_commands.len(),
                        "missingCount": missing_handlers.len(),
                        "unusedCount": unused_handlers.len(),
                        "renamedHandlers": renamed_handlers,
                        "callsWithGenerics": calls_with_generics,
                        "ghostEventCount": ghost_events.len(),
                        "orphanListenerCount": orphan_listeners.len(),
                    },
                    "tsconfig": tsconfig_summary,
                    "barrels": {
                        "count": barrels.len(),
                        "mixed": barrels.iter().filter(|b| b.mixed).count(),
                        "items": barrels_json,
                    },
                    "ciSummary": {
                        "duplicateClustersCount": duplicate_clusters_count_for_output,
                        "maxClusterSize": max_cluster_size_for_output,
                        "topClusters": top_clusters_for_output,
                    }
                },
                "files": files_json,
            });

            if matches!(parsed.output, OutputMode::Jsonl) {
                if let Ok(line) = serde_json::to_string(&payload) {
                    println!("{}", line);
                } else {
                    eprintln!("[loctree][warn] failed to serialize JSONL line");
                }
            } else {
                json_items.push(payload);
            }
        }
    } else {
        if idx > 0 {
            println!();
        }

        println!("Import/export analysis for {}/", root_path.display());
        println!("  Files analyzed: {}", analyses.len());
        if duplicates_hidden {
            println!("  Duplicate exports: (hidden by --no-duplicates)");
        } else {
            println!("  Duplicate exports: {}", filtered_ranked_for_output.len());
        }
        println!("  Files with re-exports: {}", reexport_files.len());
        if dynamic_hidden {
            println!("  Dynamic imports: (hidden by --no-dynamic-imports)");
        } else {
            println!("  Dynamic imports: {}", dynamic_summary_for_output.len());
        }
        if dead_symbols_total > 0 {
            println!(
                "  Dead exports (high confidence): {}{}",
                dead_symbols_total,
                if dead_symbols_total > parsed.top_dead_symbols {
                    format!(" (showing top {})", parsed.top_dead_symbols)
                } else {
                    String::new()
                }
            );
        }

        if !duplicates_hidden && !filtered_ranked_for_output.is_empty() {
            // Count silenced (cross-lang) duplicates
            let cross_lang_count = filtered_ranked
                .iter()
                .filter(|d| d.severity == DupSeverity::CrossLangExpected)
                .count();
            let actionable: Vec<_> = filtered_ranked
                .iter()
                .filter(|d| d.severity != DupSeverity::CrossLangExpected)
                .collect();

            println!(
                "
Top duplicate exports (showing {} actionable, {} cross-lang silenced):",
                actionable.len().min(parsed.analyze_limit),
                cross_lang_count
            );
            for dup in actionable.iter().take(parsed.analyze_limit) {
                // Format canonical with line number if available
                let canonical_str = match dup.canonical_line {
                    Some(line) => format!("{}:{}", dup.canonical, line),
                    None => dup.canonical.clone(),
                };
                // Format refs with line numbers
                let refs_str: Vec<String> = dup
                    .locations
                    .iter()
                    .filter(|loc| loc.file != dup.canonical)
                    .map(|loc| match loc.line {
                        Some(line) => format!("{}:{}", loc.file, line),
                        None => loc.file.clone(),
                    })
                    .collect();
                // Severity label
                let severity_label = match dup.severity {
                    DupSeverity::CrossCrate => "[CROSS_CRATE]",
                    DupSeverity::CrossModule => "[CROSS_MODULE]",
                    DupSeverity::SamePackage => "[SAME_PKG]",
                    DupSeverity::ReExportOrGeneric => "[REEXPORT]",
                    DupSeverity::CrossLangExpected => "[CROSS_LANG]",
                };
                // Cross-lang indicator
                let cross_lang_str = if dup.is_cross_lang { " cross-lang" } else { "" };
                println!(
                    "  - {} {} (score {},{} {} files) canonical: {} | import from: {}",
                    severity_label,
                    dup.name,
                    dup.score,
                    cross_lang_str,
                    dup.files.len(),
                    canonical_str,
                    refs_str.join(", ")
                );
            }
        }

        if !cascades.is_empty() {
            println!("\nRe-export cascades:");
            for (from, to) in &cascades {
                println!("  - {} -> {}", from, to);
            }
        }

        if !dynamic_hidden && !dynamic_summary_for_output.is_empty() {
            println!(
                "\nDynamic imports (showing up to {}):",
                parsed.analyze_limit
            );
            let mut sorted_dyn = dynamic_summary_for_output.clone();
            sorted_dyn.sort_by_key(|b| std::cmp::Reverse(b.1.len()));
            for (file, sources) in sorted_dyn.iter().take(parsed.analyze_limit) {
                println!(
                    "  - {}: {}{}",
                    file,
                    sources.join(", "),
                    if sources.len() > 5 {
                        "  [many sources]"
                    } else {
                        ""
                    }
                );
            }
        }

        if !missing_handlers.is_empty() || !unused_handlers.is_empty() {
            println!("\nTauri command coverage:");
            if !missing_handlers.is_empty() {
                println!(
                    "  Missing handlers (frontend calls without backend): {}",
                    missing_handlers
                        .iter()
                        .map(|g| g.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            if !unused_handlers.is_empty() {
                use crate::analyzer::report::Confidence;
                let high_conf: Vec<_> = unused_handlers
                    .iter()
                    .filter(|g| g.confidence == Some(Confidence::High))
                    .map(|g| g.name.clone())
                    .collect();
                let smell_conf: Vec<_> = unused_handlers
                    .iter()
                    .filter(|g| g.confidence == Some(Confidence::Smell))
                    .collect();

                if !high_conf.is_empty() {
                    println!(
                        "  Unused handlers (HIGH confidence): {}",
                        high_conf.join(", ")
                    );
                }
                if !smell_conf.is_empty() {
                    println!("  Unused handlers (SMELL confidence - possible dynamic usage):");
                    for g in &smell_conf {
                        let matches_note = if !g.string_literal_matches.is_empty() {
                            format!(
                                " ({} string literal matches)",
                                g.string_literal_matches.len()
                            )
                        } else {
                            String::new()
                        };
                        println!("    - {}{}", g.name, matches_note);
                    }
                }
                // Fallback for handlers without confidence (shouldn't happen but be safe)
                let no_conf: Vec<_> = unused_handlers
                    .iter()
                    .filter(|g| g.confidence.is_none())
                    .map(|g| g.name.clone())
                    .collect();
                if !no_conf.is_empty() {
                    println!("  Unused handlers: {}", no_conf.join(", "));
                }
            }
        }

        println!("\nTip: rerun with --json for machine-readable output.");
    }

    let mut report_section = None;
    // Build ReportSection for HTML reports OR for agent feed (--for-agent-feed/--agent-json)
    if parsed.report_path.is_some() || parsed.for_agent_feed {
        let mut sorted_dyn = dynamic_summary.clone();
        sorted_dyn.sort_by_key(|b| std::cmp::Reverse(b.1.len()));
        let insights = collect_ai_insights(
            &analyses,
            &filtered_ranked,
            &cascades,
            &missing_handlers,
            &unused_handlers,
        );
        let mut missing_sorted = missing_handlers.clone();
        missing_sorted.sort_by(|a, b| a.name.cmp(&b.name));
        let mut unused_sorted = unused_handlers.clone();
        unused_sorted.sort_by(|a, b| a.name.cmp(&b.name));
        let mut unregistered_sorted = unregistered_handlers.clone();
        unregistered_sorted.sort_by(|a, b| a.name.cmp(&b.name));

        // Calculate total LOC
        let total_loc: usize = analyses.iter().map(|a| a.loc).sum();

        let tree = build_tree(&analyses, &root_path);

        // Compute coverage gaps by building a minimal snapshot
        let coverage_gaps = {
            use crate::snapshot::{
                CommandBridge as SnapshotCommandBridge, EventBridge, Snapshot, SnapshotMetadata,
            };

            // Convert command_bridges to snapshot format
            let snapshot_command_bridges: Vec<SnapshotCommandBridge> = command_bridges
                .iter()
                .map(|cb| {
                    let has_handler = cb.be_location.is_some();
                    let is_called = !cb.fe_locations.is_empty();
                    let backend_handler = cb
                        .be_location
                        .as_ref()
                        .map(|(path, line, _)| (path.clone(), *line));
                    let frontend_calls = cb.fe_locations.clone();

                    SnapshotCommandBridge {
                        name: cb.name.clone(),
                        has_handler,
                        is_called,
                        backend_handler,
                        frontend_calls,
                    }
                })
                .collect();

            // Build event bridges from analyses (emit/listen patterns)
            let event_bridges: Vec<EventBridge> = Vec::new(); // TODO: extract from analyses if available

            // Create minimal snapshot for coverage analysis
            let snapshot = Snapshot {
                metadata: SnapshotMetadata::default(),
                files: analyses.clone(),
                edges: Vec::new(),
                export_index: HashMap::new(),
                command_bridges: snapshot_command_bridges,
                event_bridges,
                barrels: Vec::new(),
                semantic_facts: None,
                symbol_graph: None,
            };

            find_coverage_gaps(&snapshot)
        };

        // Calculate health score from available metrics
        let health_score = {
            // Count breaking cycles (hard bidirectional cycles)
            let breaking_cycles = circular_imports.len();
            // Count structural cycles (lazy/dynamic cycles)
            let structural_cycles = lazy_circular_imports.len();

            // Count dead exports (HIGH confidence)
            let dead_exports_count = dead_exports_for_report.len();

            // Count twins data if available
            let (twins_dead_parrots, twins_same_language, barrel_chaos_count) =
                if let Some(ref twins) = twins_data {
                    (
                        twins.dead_parrots.len(),
                        twins.exact_twins.len(),
                        twins.barrel_chaos.missing_barrels.len()
                            + twins.barrel_chaos.deep_chains.len()
                            + twins.barrel_chaos.inconsistent_paths.len(),
                    )
                } else {
                    (0, 0, 0)
                };

            // Count duplicate exports
            let duplicate_exports = filtered_ranked.len();

            // Count cascade imports
            let cascade_imports = cascades.len();

            let metrics = HealthMetrics {
                // CERTAIN severity
                missing_handlers: missing_sorted.len(),
                unregistered_handlers: unregistered_sorted.len(),
                breaking_cycles,

                // HIGH severity
                unused_high_confidence: unused_sorted.len(),
                dead_exports: dead_exports_count,
                twins_dead_parrots,

                // SMELL severity
                twins_same_language,
                barrel_chaos_count,
                structural_cycles,
                cascade_imports,
                duplicate_exports,

                // Project context
                files: analyses.len(),
                loc: total_loc,

                // Optional: issue details (not populated for now)
                certain_items: Vec::new(),
                high_items: Vec::new(),
                smell_items: Vec::new(),
            };

            let score = calculate_health_score(&metrics);
            Some(score.health)
        };

        let mut section = ReportSection {
            insights,
            root: root_path.display().to_string(),
            files_analyzed: analyses.len(),
            total_loc,
            reexport_files_count: reexport_files.len(),
            dynamic_imports_count: dynamic_summary.len(),
            ranked_dups: filtered_ranked.clone(),
            cascades: cascades.clone(),
            circular_imports: circular_imports.clone(),
            lazy_circular_imports: lazy_circular_imports.clone(),
            dynamic: sorted_dyn,
            analyze_limit: parsed.analyze_limit,
            generated_at: Some(generated_at.clone()),
            schema_name: Some(global.schema_name.to_string()),
            schema_version: Some(global.schema_version.to_string()),
            loctree_version: Some(crate::BUILD_VERSION.to_string()),
            missing_handlers: missing_sorted,
            unregistered_handlers: unregistered_sorted,
            unused_handlers: unused_sorted,
            command_counts: (global.fe_commands.len(), global.be_commands.len()),
            command_bridges: command_bridges.clone(),
            open_base: if parsed.report_path.is_some() && parsed.serve {
                current_open_base()
            } else {
                None
            },
            tree: Some(tree),
            graph: graph_data.clone(),
            graph_warning: graph_warning.clone(),
            git_branch: global.git.and_then(|g| g.branch.clone()),
            git_commit: global.git.and_then(|g| g.commit.clone()),
            priority_tasks: Vec::new(),
            hub_files: Vec::new(),
            hotspots: Vec::new(),
            crowds: crowds.clone(),
            dead_exports: dead_exports_for_report.clone(),
            dist: None,
            twins_data: twins_data.clone(),
            coverage_gaps,
            health_score,
            refactor_plan: None,
            context_atlas: None,
        };

        let quick_wins = extract_quick_wins(std::slice::from_ref(&section), &analyses);
        let priority_tasks: Vec<PriorityTask> = build_priority_tasks(&quick_wins)
            .into_iter()
            .map(|task: AiPriorityTask| PriorityTask {
                priority: task.priority,
                kind: task.kind,
                target: task.target,
                location: task.location,
                why: task.why,
                risk: task.risk,
                fix_hint: task.fix_hint,
                verify_cmd: task.verify_cmd,
            })
            .collect();
        let hub_files: Vec<HubFile> = find_hub_files(&analyses)
            .into_iter()
            .map(|hub: AiHubFile| HubFile {
                path: hub.path,
                loc: hub.loc,
                imports_count: hub.imports_count,
                exports_count: hub.exports_count,
                importers_count: hub.importers_count,
                commands_count: hub.commands_count,
                slice_cmd: hub.slice_cmd,
            })
            .collect();
        let hotspots: Vec<HotspotFile> = hub_files
            .iter()
            .filter(|hub| hub.importers_count > 0)
            .map(|hub| HotspotFile {
                file: hub.path.clone(),
                importers: hub.importers_count,
                category: hotspot_category(hub.importers_count).to_string(),
                slice_cmd: hub.slice_cmd.clone(),
            })
            .collect();

        section.priority_tasks = priority_tasks;
        section.hotspots = hotspots;
        section.hub_files = hub_files;
        report_section = Some(section);
    }

    RootArtifacts {
        json_items,
        report_section,
    }
}

fn hotspot_category(importers: usize) -> &'static str {
    match importers {
        n if n >= 10 => "CORE",
        n if n >= 3 => "SHARED",
        _ => "PERIPHERAL",
    }
}

pub fn write_report(
    report_path: &std::path::Path,
    sections: &[ReportSection],
    verbose: bool,
) -> io::Result<()> {
    if let Some(dir) = report_path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "Failed to create HTML report parent directory {}: {}",
                    dir.display(),
                    e
                ),
            )
        })?;
    }
    // Show spinner during HTML generation (can be slow for large codebases)
    let spinner = if !verbose {
        Some(crate::progress::Spinner::new("Generating HTML report..."))
    } else {
        None
    };
    let result = render_html_report(report_path, sections);
    if let Some(s) = spinner {
        s.finish_clear();
    }
    result.map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "Failed to write HTML report to {}: {}",
                report_path.display(),
                e
            ),
        )
    })?;
    // Show relative path for cleaner output (with ./ prefix for consistency)
    let display_path = std::env::current_dir()
        .ok()
        .and_then(|cwd| report_path.strip_prefix(&cwd).ok())
        .map(|p| format!("./{}", p.display()))
        .unwrap_or_else(|| report_path.display().to_string());
    if verbose {
        eprintln!("[loctree][debug] wrote HTML to {}", display_path);
    } else {
        crate::progress::success(&format!("Report → {}", display_path));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::build_tree;
    use crate::types::FileAnalysis;
    use std::path::Path;

    #[test]
    fn build_tree_aggregates_loc_and_hierarchy() {
        let analyses = vec![
            FileAnalysis {
                path: "src/a.ts".into(),
                loc: 10,
                ..Default::default()
            },
            FileAnalysis {
                path: "src/nested/b.ts".into(),
                loc: 20,
                ..Default::default()
            },
            FileAnalysis {
                path: "src/nested/deeper/c.ts".into(),
                loc: 30,
                ..Default::default()
            },
        ];
        let tree = build_tree(&analyses, Path::new("src"));
        // Expect top-level nodes include a.ts and nested/
        let a = tree
            .iter()
            .find(|n| n.path == "a.ts")
            .expect("top-level file node");
        assert_eq!(a.loc, 10);
        assert!(a.children.is_empty());

        let nested = tree
            .iter()
            .find(|n| n.path == "nested")
            .expect("nested directory node");
        assert_eq!(nested.loc, 50); // 20 + 30
        let b = nested
            .children
            .iter()
            .find(|c| c.path == "b.ts")
            .expect("nested file node");
        assert_eq!(b.loc, 20);
        let deeper = nested
            .children
            .iter()
            .find(|c| c.path == "deeper")
            .expect("deeper directory node");
        assert_eq!(deeper.path, "deeper");
        assert_eq!(deeper.loc, 30);
        assert_eq!(deeper.children.len(), 1);
        let leaf = &deeper.children[0];
        assert_eq!(leaf.path, "c.ts");
        assert_eq!(leaf.loc, 30);
    }

    #[test]
    fn build_tree_handles_root_prefix_mismatch() {
        let analyses = vec![FileAnalysis {
            path: "other/file.ts".into(),
            loc: 5,
            ..Default::default()
        }];
        // If strip_prefix fails, it should fall back to the full path parts.
        let tree = build_tree(&analyses, Path::new("src"));
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].path, "other");
        assert_eq!(tree[0].loc, 5);
    }
}
