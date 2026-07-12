//! Dispatcher for the new command interface.
//!
//! This module converts `Command` variants into `ParsedArgs` and dispatches
//! to the existing handlers. This provides a bridge between the new CLI
//! interface and the existing implementation.

pub(crate) mod handlers;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

pub use crate::pack::{
    ContextPack, ContextPack as DenseContextPack, StructuralRole, compose_context_pack,
    compose_context_pack as compose_context_pack_dense, compose_context_pack_from_snapshot,
};
pub use handlers::context::atlas::{
    ContextAtlasManifest, atlas_dir_for_project, materialize_context_atlas,
};
pub use handlers::context::render_context_pack_markdown;

use crate::args::{ParsedArgs, SearchQueryMode};
use crate::types::{DEFAULT_LOC_THRESHOLD, Mode, OutputMode};

use super::command::*;

thread_local! {
    static COMMAND_SNAPSHOT_CACHE: RefCell<Option<HashMap<String, crate::snapshot::Snapshot>>> =
        const { RefCell::new(None) };
}

struct CommandSnapshotCacheGuard {
    enabled: bool,
}

impl Drop for CommandSnapshotCacheGuard {
    fn drop(&mut self) {
        if self.enabled {
            COMMAND_SNAPSHOT_CACHE.with(|cache| {
                *cache.borrow_mut() = None;
            });
        }
    }
}

pub(crate) fn with_command_snapshot_cache<T>(f: impl FnOnce() -> T) -> T {
    let enabled = COMMAND_SNAPSHOT_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.is_some() {
            false
        } else {
            *cache = Some(HashMap::new());
            true
        }
    });
    let _guard = CommandSnapshotCacheGuard { enabled };
    f()
}

fn command_snapshot_cache_key(roots: &[PathBuf]) -> String {
    let snapshot_root = crate::snapshot::resolve_snapshot_root(roots);
    let requested_roots = crate::snapshot::normalize_roots_for_scope_compare(
        roots.iter().map(|p| p.as_path()),
        &snapshot_root,
    );
    format!(
        "{}\n{}",
        snapshot_root.to_string_lossy().replace('\\', "/"),
        requested_roots.join("\n")
    )
}

/// Convert a Command and GlobalOptions into ParsedArgs for backward compatibility.
///
/// This allows us to reuse existing handlers while providing the new CLI interface.
pub fn command_to_parsed_args(cmd: &Command, global: &GlobalOptions) -> ParsedArgs {
    // Initialize with global options applied
    let mut parsed = ParsedArgs {
        output: if global.json {
            OutputMode::Json
        } else {
            OutputMode::Human
        },
        verbose: global.verbose,
        color: global.color,
        ..Default::default()
    };
    parsed.library_mode = global.library_mode;
    parsed.python_library = global.python_library;
    parsed.py_roots = global.py_roots.clone();
    // Carrier only: read commands that call `acquire_snapshot` directly (slice)
    // read this to request the ephemeral include-ignored snapshot. The scan-time
    // override flag proper is set by `unified_scan_args_with_ignore`, so an empty
    // `loctignore_override_patterns` here keeps normal scans unaffected.
    parsed.include_ignored = global.include_ignored;

    // Convert command-specific options
    match cmd {
        Command::Auto(opts) => {
            // Auto mode: full scan with stack detection, write cached artifacts (see LOCT_CACHE_DIR).
            // Maps to Mode::Init (which does scan + snapshot)
            // Unless --for-agent-feed is set, then use Mode::ForAi
            if opts.for_agent_feed {
                parsed.mode = Mode::ForAi;
                parsed.output = if opts.agent_json {
                    OutputMode::Json
                } else {
                    OutputMode::Jsonl
                };
                parsed.for_agent_feed = true;
                parsed.agent_json = opts.agent_json;
                parsed.force_full_scan = true; // don't reuse snapshot for agent feed
            } else {
                parsed.mode = Mode::Init;
                parsed.auto_outputs = true;
            }
            parsed.root_list = if opts.roots.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                opts.roots.clone()
            };
            parsed.suppress_duplicates = opts.suppress_duplicates;
            parsed.suppress_dynamic = opts.suppress_dynamic;
            parsed.full_scan = opts.full_scan;
            parsed.scan_all = opts.scan_all;
            parsed.use_gitignore = true; // Auto mode respects gitignore by default
        }

        Command::Scan(opts) => {
            parsed.mode = Mode::Init;
            parsed.root_list = if opts.roots.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                opts.roots.clone()
            };
            parsed.full_scan = opts.full_scan;
            parsed.scan_all = opts.scan_all;
            parsed.use_gitignore = true;
        }

        Command::Watch(opts) => {
            parsed.mode = Mode::Init;
            parsed.root_list = if opts.roots.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                opts.roots.clone()
            };
            parsed.full_scan = opts.full_scan;
            parsed.scan_all = opts.scan_all;
            parsed.use_gitignore = true;
        }

        Command::Tree(opts) => {
            parsed.mode = Mode::Tree;
            parsed.root_list = if opts.roots.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                opts.roots.clone()
            };
            parsed.max_depth = opts.depth;
            if let Some(limit) = opts.summary {
                parsed.summary = true;
                parsed.summary_limit = limit;
            }
            parsed.summary_only = opts.summary_only;
            parsed.loc_threshold = opts.loc_threshold.unwrap_or(DEFAULT_LOC_THRESHOLD);
            parsed.show_hidden = opts.show_hidden;
            parsed.find_artifacts = opts.find_artifacts;
            parsed.show_ignored = opts.show_ignored;
            parsed.tree_files_only = opts.files_only;
            parsed.tree_path_filter = opts.path_filter.clone();
            if opts.show_ignored {
                parsed.use_gitignore = true;
            }
        }

        Command::Slice(opts) => {
            parsed.mode = Mode::Slice;
            parsed.slice_target = Some(opts.target.clone());
            parsed.slice_consumers = opts.consumers;
            parsed.slice_rescan = opts.rescan;
            parsed.root_list = if let Some(ref root) = opts.root {
                vec![root.clone()]
            } else {
                vec![PathBuf::from(".")]
            };
        }

        Command::Context(_) => {
            // Context is handled specially in dispatch_command.
        }

        Command::RepoView(opts) => {
            parsed.mode = Mode::ForAi;
            parsed.output = OutputMode::Json;
            parsed.for_agent_feed = true;
            parsed.agent_json = true;
            parsed.force_full_scan = true;
            parsed.root_list = opts
                .project
                .as_ref()
                .map(|project| vec![project.clone()])
                .unwrap_or_else(|| vec![PathBuf::from(".")]);
            parsed.use_gitignore = true;
        }

        Command::Find(opts) => {
            parsed.mode = Mode::Search;
            parsed.search_query_mode = SearchQueryMode::Single;
            parsed.search_queries.clear();

            parsed.search_query = opts
                .query
                .clone()
                .or_else(|| opts.symbol.clone())
                .or_else(|| opts.similar.clone())
                .or_else(|| opts.impact.clone())
                .or_else(|| {
                    if opts.queries.is_empty() {
                        return None;
                    }

                    // Multi-arg positional query handling:
                    // - `loct find A B C` => split-mode (separate subqueries + cross-match)
                    // - `loct find "A B C"` => AND-mode (intersection)
                    // - `loct find --or A B C` => legacy OR (A|B|C)
                    if opts.queries.len() >= 2 {
                        if opts.or_mode {
                            Some(opts.queries.join("|"))
                        } else {
                            parsed.search_query_mode = SearchQueryMode::Split;
                            parsed.search_queries = opts.queries.clone();
                            None
                        }
                    } else {
                        // Single positional query: if it contains whitespace, treat as AND-mode.
                        let raw = opts.queries[0].trim().to_string();
                        if raw.chars().any(|c| c.is_whitespace()) && !raw.contains('|') {
                            let terms: Vec<String> =
                                raw.split_whitespace().map(|t| t.to_string()).collect();
                            if terms.len() >= 2 {
                                parsed.search_query_mode = SearchQueryMode::And;
                                parsed.search_queries = terms;
                                None
                            } else {
                                Some(raw)
                            }
                        } else {
                            Some(raw)
                        }
                    }
                });
            parsed.symbol = opts.symbol.clone();
            parsed.impact = opts.impact.clone();
            parsed.check_sim = opts.similar.clone();
            parsed.search_dead_only = opts.dead_only;
            parsed.search_exported_only = opts.exported_only;
            parsed.search_lang = opts.lang.clone();
            parsed.search_limit = opts.limit;
            parsed.root_list = vec![PathBuf::from(".")];
        }

        Command::Occurrences(_) => {
            // Occurrences is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs.
        }

        Command::Findings(opts) => {
            parsed.mode = if opts.summary {
                Mode::Summary
            } else {
                Mode::Findings
            };
            parsed.output = OutputMode::Json;
            parsed.root_list = if opts.roots.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                opts.roots.clone()
            };
            parsed.use_gitignore = true;
        }

        Command::Dead(opts) => {
            parsed.mode = Mode::AnalyzeImports;
            parsed.dead_exports = true;
            parsed.root_list = if opts.roots.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                opts.roots.clone()
            };
            parsed.dead_confidence = opts.confidence.clone();
            parsed.top_dead_symbols = if opts.full {
                usize::MAX
            } else if let Some(top) = opts.top {
                top
            } else {
                parsed.top_dead_symbols
            };
            parsed.use_gitignore = true;
            parsed.with_tests = opts.with_tests;
            parsed.with_helpers = opts.with_helpers;
        }

        Command::Cycles(opts) => {
            parsed.mode = Mode::AnalyzeImports;
            parsed.circular = true;
            parsed.root_list = if opts.roots.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                opts.roots.clone()
            };
            parsed.use_gitignore = true;
        }

        Command::Trace(_) => {
            // Trace is handled specially in dispatch_command
        }

        Command::Commands(opts) => {
            // Commands shows Tauri command bridges
            parsed.mode = Mode::AnalyzeImports;
            parsed.tauri_preset = true;
            parsed.root_list = if opts.roots.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                opts.roots.clone()
            };
            parsed.use_gitignore = true;
            parsed.commands_name_filter = opts.name_filter.clone();
            parsed.commands_missing_only = opts.missing_only;
            parsed.commands_unused_only = opts.unused_only;
            parsed.suppress_duplicates = opts.suppress_duplicates;
            parsed.suppress_dynamic = opts.suppress_dynamic;
        }

        Command::Events(opts) => {
            // Events analysis (ghost/orphan/races)
            parsed.mode = Mode::AnalyzeImports;
            parsed.root_list = if opts.roots.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                opts.roots.clone()
            };
            // Enable race detection if specified
            parsed.py_races = opts.races;
            parsed.use_gitignore = true;
            parsed.suppress_duplicates = opts.suppress_duplicates;
            parsed.suppress_dynamic = opts.suppress_dynamic;
        }
        Command::Pipelines(opts) => {
            parsed.mode = Mode::AnalyzeImports;
            parsed.root_list = if opts.roots.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                opts.roots.clone()
            };
            parsed.use_gitignore = true;
        }
        Command::Insights(opts) => {
            parsed.mode = Mode::AnalyzeImports;
            parsed.root_list = if opts.roots.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                opts.roots.clone()
            };
            parsed.use_gitignore = true;
        }
        Command::Manifests(opts) => {
            parsed.mode = Mode::AnalyzeImports;
            parsed.root_list = if opts.roots.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                opts.roots.clone()
            };
            parsed.use_gitignore = true;
        }

        Command::Info(_opts) => {
            // Info command - show snapshot metadata
            // For now, map to Init which will show info if snapshot exists
            parsed.mode = Mode::Init;
            parsed.root_list = vec![PathBuf::from(".")];
        }

        Command::Lint(opts) => {
            parsed.mode = Mode::AnalyzeImports;
            parsed.entrypoints = opts.entrypoints;
            parsed.sarif = opts.sarif;
            parsed.tauri_preset = opts.tauri;
            parsed.root_list = if opts.roots.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                opts.roots.clone()
            };
            if opts.fail {
                parsed.fail_on_missing_handlers = true;
                parsed.fail_on_ghost_events = true;
            }
            parsed.use_gitignore = true;
            parsed.suppress_duplicates = opts.suppress_duplicates;
            parsed.suppress_dynamic = opts.suppress_dynamic;
        }

        Command::Report(opts) => {
            parsed.mode = Mode::AnalyzeImports;
            parsed.auto_outputs = true;
            parsed.root_list = if opts.roots.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                opts.roots.clone()
            };
            // `loct report` is the human-facing HTML surface and must always
            // emit an HTML artifact, not just refresh cached JSON. When the
            // operator supplies `--output`, honour it verbatim; otherwise
            // default to the canonical artifacts directory beside the
            // snapshot so the help text ("writes the full HTML report") and
            // observable behaviour agree.
            let resolved_report_path = if let Some(ref output) = opts.output {
                output.clone()
            } else {
                let primary_root = parsed
                    .root_list
                    .first()
                    .cloned()
                    .unwrap_or_else(|| PathBuf::from("."));
                crate::snapshot::Snapshot::artifacts_dir(&primary_root).join("report.html")
            };
            parsed.report_path = Some(resolved_report_path);
            parsed.serve = opts.serve;
            parsed.serve_port = opts.port;
            if let Some(ref editor) = opts.editor {
                parsed.editor_kind = Some(editor.clone());
            }
            parsed.use_gitignore = true;
        }

        Command::Prism(_) => {
            // Prism is handled specially in dispatch_command.
        }

        Command::Help(opts) => {
            if opts.legacy {
                parsed.show_help_full = true; // Show legacy help
            } else {
                parsed.show_help = true;
            }
        }

        Command::Version => {
            parsed.show_version = true;
        }

        Command::Query(_) => {
            // Query is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Body(_) => {
            // Body is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Impact(_) => {
            // Impact is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Diff(_) => {
            // Diff is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Crowd(_) => {
            // Crowd is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Tagmap(_) => {
            // Tagmap is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Twins(_) => {
            // Twins is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Sniff(_) => {
            // Sniff is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Suppress(_) => {
            // Suppress is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Suppressions(_) => {
            // Suppressions (source-side silencer inventory) is handled
            // specially in dispatch_command — does not go through ParsedArgs.
        }

        Command::Routes(_) => {
            // Routes is handled specially in dispatch_command
        }

        Command::Dist(_) => {
            // Dist is handled specially in dispatch_command
        }

        Command::Coverage(_) => {
            // Coverage is handled specially in dispatch_command
        }

        Command::JqQuery(_) => {
            // JqQuery is handled specially in dispatch_command
            // It doesn't use ParsedArgs, will be handled by jaq executor
        }

        Command::Focus(_) => {
            // Focus is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Hotspots(_) => {
            // Hotspots is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Follow(_) => {
            // Follow is handled specially in dispatch_command
            // as it delegates to existing scope handlers.
        }

        Command::Layoutmap(_) => {
            // Layoutmap is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Zombie(_) => {
            // Zombie is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Health(_) => {
            // Health is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Audit(_) => {
            // Audit is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::EnvTruth(_) => {
            // EnvTruth is handled specially in dispatch_command (Cut 8)
        }

        Command::Doctor(_) => {
            // Doctor is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Plan(_) => {
            // Plan is handled specially in dispatch_command
            // as it doesn't go through ParsedArgs
        }

        Command::Cache(_) => {
            // Cache is handled specially in dispatch_command
        }

        Command::PruneOldArtifacts(_) => {
            // PruneOldArtifacts is handled specially in dispatch_command
        }
    }

    parsed
}

/// Result type for command dispatch.
pub enum DispatchResult {
    /// Command was handled, return this exit code
    Exit(i32),
    /// Show main help
    ShowHelp,
    /// Show legacy help
    ShowLegacyHelp,
    /// Show version
    ShowVersion,
    /// Continue with normal execution using ParsedArgs (boxed to reduce enum size)
    Continue(Box<ParsedArgs>),
}

/// Dispatch a parsed command.
///
/// Returns a DispatchResult indicating what action to take.
pub fn dispatch_command(parsed_cmd: &ParsedCommand) -> DispatchResult {
    // Emit deprecation warning if this was from legacy syntax
    parsed_cmd.emit_deprecation_warning();

    // Handle special cases first
    match &parsed_cmd.command {
        Command::Help(opts) if opts.legacy => {
            return DispatchResult::ShowLegacyHelp;
        }
        Command::Help(opts) if opts.full => {
            return DispatchResult::ShowLegacyHelp; // Full help shows legacy too
        }
        Command::Help(opts) if opts.command.is_some() => {
            let cmd_name = opts.command.clone().unwrap();
            if let Some(message) = Command::retired_command_message(&cmd_name) {
                eprintln!("{}", message.trim_end());
                return DispatchResult::Exit(1);
            }
            if let Some(text) = Command::format_command_help(&cmd_name) {
                println!("{}", text);
                return DispatchResult::Exit(0);
            } else {
                eprintln!(
                    "Unknown command '{}'. Run 'loct --help' for available commands.",
                    cmd_name
                );
                return DispatchResult::Exit(1);
            }
        }
        Command::Help(_) => {
            return DispatchResult::ShowHelp;
        }
        Command::Version => {
            return DispatchResult::ShowVersion;
        }
        Command::Query(opts) => {
            // Execute query and return result
            return handlers::query::handle_query_command(opts, &parsed_cmd.global);
        }
        Command::Body(opts) => {
            return handlers::query::handle_body_command(opts, &parsed_cmd.global);
        }
        Command::Occurrences(opts) => {
            return handlers::occurrences::handle_occurrences_command(opts, &parsed_cmd.global);
        }
        // `find --literal` is the truth-layer mode of find: intercept it BEFORE
        // the ParsedArgs/Mode::Search path so the fuzzy/semantic search engine
        // never touches the literal result set. Default `find` (no --literal)
        // falls through unchanged.
        Command::Find(opts) if opts.where_symbol => {
            return handlers::query::handle_find_where_symbol_command(opts, &parsed_cmd.global);
        }
        Command::Find(opts) if opts.literal => {
            return handlers::occurrences::handle_find_literal_command(opts, &parsed_cmd.global);
        }
        // `find --regex` is the pattern-truth mode: regex over raw file text with
        // coverage accounting, intercepted before the fuzzy search path just like
        // `--literal`.
        Command::Find(opts) if opts.regex => {
            return handlers::occurrences::handle_find_regex_command(opts, &parsed_cmd.global);
        }
        Command::Impact(opts) => {
            // Execute impact analysis and return result
            return handlers::diff::handle_impact_command(opts, &parsed_cmd.global);
        }
        Command::Diff(opts) => {
            // Execute diff and return result
            return handlers::diff::handle_diff_command(opts, &parsed_cmd.global);
        }
        Command::Crowd(opts) => {
            return handlers::ai::handle_crowd_command(opts, &parsed_cmd.global);
        }
        Command::Tagmap(opts) => {
            return handlers::ai::handle_tagmap_command(opts, &parsed_cmd.global);
        }
        Command::Twins(opts) => {
            return handlers::ai::handle_twins_command(opts, &parsed_cmd.global);
        }
        Command::Sniff(opts) => {
            return handlers::ai::handle_sniff_command(opts, &parsed_cmd.global);
        }
        Command::Suppress(opts) => {
            return handlers::ai::handle_suppress_command(opts, &parsed_cmd.global);
        }
        Command::Suppressions(opts) => {
            return handlers::suppressions::handle_suppressions_command(opts, &parsed_cmd.global);
        }
        Command::Dead(opts) => {
            return handlers::analysis::handle_dead_command(opts, &parsed_cmd.global);
        }
        Command::Cycles(opts) => {
            return handlers::analysis::handle_cycles_command(opts, &parsed_cmd.global);
        }
        Command::Trace(opts) => {
            return handlers::analysis::handle_trace_command(opts, &parsed_cmd.global);
        }
        Command::Commands(opts) => {
            return handlers::analysis::handle_commands_command(opts, &parsed_cmd.global);
        }
        Command::Routes(opts) => {
            return handlers::analysis::handle_routes_command(opts, &parsed_cmd.global);
        }
        Command::Events(opts) => {
            return handlers::analysis::handle_events_command(opts, &parsed_cmd.global);
        }
        Command::Pipelines(opts) => {
            return handlers::analysis::handle_pipelines_command(opts, &parsed_cmd.global);
        }
        Command::Insights(opts) => {
            return handlers::analysis::handle_insights_command(opts, &parsed_cmd.global);
        }
        Command::Manifests(opts) => {
            return handlers::analysis::handle_manifests_command(opts, &parsed_cmd.global);
        }
        Command::Lint(opts) => {
            return handlers::output::handle_lint_command(opts, &parsed_cmd.global);
        }
        Command::Dist(opts) => {
            return handlers::output::handle_dist_command(opts, &parsed_cmd.global);
        }
        Command::Coverage(opts) => {
            return handlers::watch::handle_coverage_command(opts, &parsed_cmd.global);
        }
        Command::JqQuery(opts) => {
            return handlers::query::handle_jq_query_command(opts, &parsed_cmd.global);
        }
        Command::Focus(opts) => {
            return handlers::analysis::handle_focus_command(opts, &parsed_cmd.global);
        }
        Command::Hotspots(opts) => {
            return handlers::analysis::handle_hotspots_command(opts, &parsed_cmd.global);
        }
        Command::Follow(opts) => {
            return handlers::analysis::handle_follow_command(opts, &parsed_cmd.global);
        }
        Command::Layoutmap(opts) => {
            return handlers::analysis::handle_layoutmap_command(opts, &parsed_cmd.global);
        }
        Command::Zombie(opts) => {
            return handlers::analysis::handle_zombie_command(opts, &parsed_cmd.global);
        }
        Command::Health(opts) => {
            return handlers::analysis::handle_health_command(opts, &parsed_cmd.global);
        }
        Command::Audit(opts) => {
            return handlers::analysis::handle_audit_command(opts, &parsed_cmd.global);
        }
        Command::Doctor(opts) => {
            return handlers::doctor::run(opts, &parsed_cmd.global);
        }
        Command::EnvTruth(opts) => {
            return handlers::env_truth::run(opts, &parsed_cmd.global);
        }
        Command::Context(opts) => {
            return handlers::context::run(opts, &parsed_cmd.global);
        }
        Command::Prism(opts) => {
            return handlers::prism::handle_prism_command(opts, &parsed_cmd.global);
        }
        Command::Plan(opts) => {
            return handlers::analysis::handle_plan_command(opts, &parsed_cmd.global);
        }
        Command::Cache(opts) => {
            return handlers::cache::handle_cache_command(opts);
        }
        Command::PruneOldArtifacts(opts) => {
            return handlers::prune::handle_prune_old_artifacts(opts);
        }
        Command::Scan(opts) if opts.watch => {
            return handlers::watch::handle_scan_watch_command(opts, &parsed_cmd.global);
        }
        Command::Watch(opts) => {
            return handlers::watch::handle_watch_command(opts, &parsed_cmd.global);
        }
        // Note: Command::Report falls through to ParsedArgs flow to use full analysis pipeline
        // which includes twins data, graph visualization, and proper Leptos SSR rendering
        _ => {}
    }

    // Convert to ParsedArgs for the existing handlers
    let parsed_args = command_to_parsed_args(&parsed_cmd.command, &parsed_cmd.global);
    DispatchResult::Continue(Box::new(parsed_args))
}

/// Load existing snapshot or create one if missing (used by handler submodules)
///
/// Thin shim over the snapshot freshness authority
/// (`crate::snapshot::acquire_snapshot`) — all DRIFT / REUSE_FENCE /
/// watch-fast-path / `--fresh` / `--no-scan` / `--fail-stale` decisions live
/// there, not here.
pub(crate) fn load_or_create_snapshot_for_roots(
    roots: &[std::path::PathBuf],
    global: &GlobalOptions,
) -> std::io::Result<crate::snapshot::Snapshot> {
    let cache_key = command_snapshot_cache_key(roots);
    if let Some(snapshot) = COMMAND_SNAPSHOT_CACHE.with(|cache| {
        cache
            .borrow()
            .as_ref()
            .and_then(|snapshots| snapshots.get(&cache_key).cloned())
    }) {
        return Ok(snapshot);
    }

    let snapshot = crate::snapshot::acquire_snapshot(
        roots,
        crate::snapshot::SnapshotReusePolicy::Strict,
        &acquire_options_from_global(global),
    )?;

    COMMAND_SNAPSHOT_CACHE.with(|cache| {
        if let Some(snapshots) = cache.borrow_mut().as_mut() {
            snapshots.insert(cache_key, snapshot.clone());
        }
    });

    Ok(snapshot)
}

pub(crate) fn load_or_create_query_snapshot_for_roots(
    roots: &[std::path::PathBuf],
    global: &GlobalOptions,
) -> std::io::Result<crate::snapshot::Snapshot> {
    crate::snapshot::acquire_snapshot(
        roots,
        crate::snapshot::SnapshotReusePolicy::ReuseFence,
        &acquire_options_from_global(global),
    )
}

fn acquire_options_from_global(global: &GlobalOptions) -> crate::snapshot::AcquireOptions {
    crate::snapshot::AcquireOptions {
        fresh: global.fresh,
        no_scan: global.no_scan,
        fail_stale: global.fail_stale,
        quiet: global.quiet,
        verbose: global.verbose,
        json: global.json,
        // Suppress the scan summary in json/quiet mode to keep stdout clean.
        print_scan_summary: !(global.json || global.quiet),
        include_ignored: global.include_ignored,
        ..Default::default()
    }
}

pub(crate) fn load_or_create_snapshot(
    root: &std::path::Path,
    global: &GlobalOptions,
) -> std::io::Result<crate::snapshot::Snapshot> {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let root_list = vec![canonical];
    load_or_create_snapshot_for_roots(&root_list, global)
}

/// Check if a file is a test file (used by handler submodules).
/// Single canonical definition lives in `analyzer::classify::is_test_file`
/// (deduplicated from the former analyzer/mod.rs + cli/dispatch copies).
pub(crate) fn is_test_file(path: &str) -> bool {
    crate::analyzer::classify::is_test_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::normalize_roots_for_scope_compare;
    use tempfile::TempDir;

    #[test]
    fn test_auto_command_to_parsed_args() {
        let cmd = Command::Auto(AutoOptions {
            roots: vec![PathBuf::from(".")],
            full_scan: true,
            scan_all: false,
            for_agent_feed: false,
            agent_json: false,
            suppress_duplicates: false,
            suppress_dynamic: false,
        });
        let global = GlobalOptions::default();
        let parsed = command_to_parsed_args(&cmd, &global);

        assert!(matches!(parsed.mode, Mode::Init));
        assert!(parsed.full_scan);
        assert!(!parsed.scan_all);
    }

    #[test]
    fn test_dead_command_to_parsed_args() {
        let cmd = Command::Dead(DeadOptions {
            roots: vec![],
            confidence: Some("high".into()),
            top: Some(10),
            full: false,
            path_filter: None,
            with_tests: false,
            with_helpers: false,
            with_shadows: false,
            with_ambient: false,
            with_dynamic: false,
        });
        let global = GlobalOptions {
            json: true,
            ..Default::default()
        };
        let parsed = command_to_parsed_args(&cmd, &global);

        assert!(matches!(parsed.mode, Mode::AnalyzeImports));
        assert!(parsed.dead_exports);
        assert_eq!(parsed.dead_confidence, Some("high".into()));
        assert_eq!(parsed.top_dead_symbols, 10);
        assert!(!parsed.with_tests);
        assert!(!parsed.with_helpers);
        assert!(matches!(parsed.output, OutputMode::Json));
    }

    #[test]
    fn test_tree_command_to_parsed_args() {
        let cmd = Command::Tree(TreeOptions {
            roots: vec![PathBuf::from("src")],
            depth: Some(3),
            summary: Some(5),
            summary_only: false,
            loc_threshold: Some(500),
            show_hidden: true,
            find_artifacts: false,
            show_ignored: false,
            files_only: true,
            path_filter: Some("src/.*\\.rs".to_string()),
        });
        let global = GlobalOptions::default();
        let parsed = command_to_parsed_args(&cmd, &global);

        assert!(matches!(parsed.mode, Mode::Tree));
        assert_eq!(parsed.max_depth, Some(3));
        assert!(parsed.summary);
        assert_eq!(parsed.summary_limit, 5);
        assert!(parsed.tree_files_only);
        assert_eq!(parsed.tree_path_filter, Some("src/.*\\.rs".to_string()));
        assert_eq!(parsed.loc_threshold, 500);
        assert!(parsed.show_hidden);
    }

    #[test]
    fn test_parse_findings_command_to_parsed_args() {
        let cmd = Command::Findings(FindingsOptions {
            roots: vec![PathBuf::from("src")],
            summary: true,
        });
        let global = GlobalOptions::default();
        let parsed = command_to_parsed_args(&cmd, &global);

        assert!(matches!(parsed.mode, Mode::Summary));
        assert!(matches!(parsed.output, OutputMode::Json));
        assert_eq!(parsed.root_list, vec![PathBuf::from("src")]);
    }

    #[test]
    fn test_repo_view_command_to_parsed_args_matches_for_ai_shape() {
        let cmd = Command::RepoView(RepoViewOptions {
            project: Some(PathBuf::from("src")),
        });
        let global = GlobalOptions::default();
        let parsed = command_to_parsed_args(&cmd, &global);

        assert!(matches!(parsed.mode, Mode::ForAi));
        assert!(matches!(parsed.output, OutputMode::Json));
        assert!(parsed.for_agent_feed);
        assert!(parsed.agent_json);
        assert!(parsed.force_full_scan);
        assert_eq!(parsed.root_list, vec![PathBuf::from("src")]);
    }

    #[test]
    fn test_slice_command_to_parsed_args() {
        let cmd = Command::Slice(SliceOptions {
            target: "src/main.rs".into(),
            root: None,
            consumers: true,
            depth: None,
            rescan: false,
        });
        let global = GlobalOptions {
            json: true,
            ..Default::default()
        };
        let parsed = command_to_parsed_args(&cmd, &global);

        assert!(matches!(parsed.mode, Mode::Slice));
        assert_eq!(parsed.slice_target, Some("src/main.rs".into()));
        assert!(parsed.slice_consumers);
        assert!(matches!(parsed.output, OutputMode::Json));
    }

    #[test]
    fn test_normalize_roots_for_scope_compare_relative_and_absolute_match() {
        let tmp = TempDir::new().expect("temp dir");
        std::fs::create_dir_all(tmp.path().join("src")).expect("create src");

        let relative = normalize_roots_for_scope_compare(
            [std::path::Path::new("src")].into_iter(),
            tmp.path(),
        );
        let absolute = normalize_roots_for_scope_compare(
            [tmp.path().join("src")].iter().map(|p| p.as_path()),
            tmp.path(),
        );

        assert_eq!(relative, absolute);
        assert_eq!(relative.len(), 1);
    }

    #[test]
    fn test_normalize_roots_for_scope_compare_sorted_and_deduped() {
        let tmp = TempDir::new().expect("temp dir");
        std::fs::create_dir_all(tmp.path().join("a")).expect("create a");
        std::fs::create_dir_all(tmp.path().join("b")).expect("create b");

        let normalized = normalize_roots_for_scope_compare(
            [
                std::path::Path::new("b"),
                std::path::Path::new("./a"),
                std::path::Path::new("b"),
            ]
            .into_iter(),
            tmp.path(),
        );

        assert_eq!(normalized.len(), 2);
        assert!(normalized[0] <= normalized[1]);
    }

    #[test]
    fn test_find_multi_arg_defaults_to_split_mode() {
        let cmd = Command::Find(FindOptions {
            queries: vec!["Props".into(), "Options".into(), "ViewModel".into()],
            ..Default::default()
        });
        let global = GlobalOptions::default();
        let parsed = command_to_parsed_args(&cmd, &global);

        assert!(matches!(parsed.mode, Mode::Search));
        assert!(matches!(parsed.search_query_mode, SearchQueryMode::Split));
        assert_eq!(parsed.search_queries, vec!["Props", "Options", "ViewModel"]);
        assert!(parsed.search_query.is_none());
    }

    #[test]
    fn test_find_single_arg_with_spaces_defaults_to_and_mode() {
        let cmd = Command::Find(FindOptions {
            queries: vec!["Props Options ViewModel".into()],
            ..Default::default()
        });
        let global = GlobalOptions::default();
        let parsed = command_to_parsed_args(&cmd, &global);

        assert!(matches!(parsed.mode, Mode::Search));
        assert!(matches!(parsed.search_query_mode, SearchQueryMode::And));
        assert_eq!(parsed.search_queries, vec!["Props", "Options", "ViewModel"]);
        assert!(parsed.search_query.is_none());
    }

    #[test]
    fn test_find_multi_arg_can_force_legacy_or_mode() {
        let cmd = Command::Find(FindOptions {
            queries: vec!["Props".into(), "Options".into()],
            or_mode: true,
            ..Default::default()
        });
        let global = GlobalOptions::default();
        let parsed = command_to_parsed_args(&cmd, &global);

        assert!(matches!(parsed.mode, Mode::Search));
        assert!(matches!(parsed.search_query_mode, SearchQueryMode::Single));
        assert_eq!(parsed.search_query.as_deref(), Some("Props|Options"));
        assert!(parsed.search_queries.is_empty());
    }

    #[test]
    fn test_find_single_arg_with_pipe_stays_single_mode() {
        let cmd = Command::Find(FindOptions {
            queries: vec!["Props|Options|ViewModel".into()],
            ..Default::default()
        });
        let global = GlobalOptions::default();
        let parsed = command_to_parsed_args(&cmd, &global);

        assert!(matches!(parsed.mode, Mode::Search));
        assert!(matches!(parsed.search_query_mode, SearchQueryMode::Single));
        assert_eq!(
            parsed.search_query.as_deref(),
            Some("Props|Options|ViewModel")
        );
        assert!(parsed.search_queries.is_empty());
    }

    #[test]
    fn test_dispatch_help_command() {
        let parsed_cmd = ParsedCommand::new(
            Command::Help(HelpOptions::default()),
            GlobalOptions::default(),
        );
        let result = dispatch_command(&parsed_cmd);
        assert!(matches!(result, DispatchResult::ShowHelp));
    }

    #[test]
    fn test_dispatch_legacy_help_command() {
        let parsed_cmd = ParsedCommand::new(
            Command::Help(HelpOptions {
                legacy: true,
                ..Default::default()
            }),
            GlobalOptions::default(),
        );
        let result = dispatch_command(&parsed_cmd);
        assert!(matches!(result, DispatchResult::ShowLegacyHelp));
    }

    #[test]
    fn test_dispatch_version_command() {
        let parsed_cmd = ParsedCommand::new(Command::Version, GlobalOptions::default());
        let result = dispatch_command(&parsed_cmd);
        assert!(matches!(result, DispatchResult::ShowVersion));
    }

    #[test]
    fn test_crowd_command_to_dispatch() {
        let tmp = TempDir::new().expect("temp dir");
        let parsed_cmd = ParsedCommand::new(
            Command::Crowd(CrowdOptions {
                pattern: Some("message".into()),
                roots: vec![tmp.path().to_path_buf()],
                ..Default::default()
            }),
            GlobalOptions::default(),
        );
        // Verify dispatch completes without scanning the live repo under the current working directory.
        let result = dispatch_command(&parsed_cmd);
        assert!(matches!(result, DispatchResult::Exit(_)));
    }
}
