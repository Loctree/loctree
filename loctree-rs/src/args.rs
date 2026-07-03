use std::collections::HashSet;
use std::path::PathBuf;

use crate::types::{ColorMode, DEFAULT_LOC_THRESHOLD, GitSubcommand, Mode, OutputMode};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SearchQueryMode {
    /// Single query string (default behavior).
    #[default]
    Single,
    /// Split-mode: run multiple subqueries and show cross-match summary.
    Split,
    /// AND-mode: tokenize a single quoted query and require all terms.
    And,
}

#[derive(Clone)]
pub struct ParsedArgs {
    pub extensions: Option<HashSet<String>>,
    pub ignore_patterns: Vec<String>,
    pub ignore_symbols: Option<HashSet<String>>,
    pub ignore_symbols_preset: Option<String>,
    pub focus_patterns: Vec<String>,
    pub exclude_report_patterns: Vec<String>,
    pub graph: bool,
    pub use_gitignore: bool,
    pub max_depth: Option<usize>,
    pub color: ColorMode,
    pub output: OutputMode,
    pub json_output_path: Option<PathBuf>,
    pub summary: bool,
    pub summary_limit: usize,
    pub summary_only: bool,
    pub suppress_duplicates: bool,
    pub suppress_dynamic: bool,
    pub show_help: bool,
    pub show_help_full: bool,
    pub show_version: bool,
    pub root_list: Vec<PathBuf>,
    pub py_roots: Vec<PathBuf>,
    pub show_hidden: bool,
    pub loc_threshold: usize,
    pub mode: Mode,
    pub analyze_limit: usize,
    pub report_path: Option<PathBuf>,
    pub serve: bool,
    pub serve_once: bool,
    pub serve_port: Option<u16>,
    pub editor_cmd: Option<String>,
    pub editor_kind: Option<String>,
    pub max_graph_nodes: Option<usize>,
    pub max_graph_edges: Option<usize>,
    pub verbose: bool,
    pub tauri_preset: bool,
    pub styles_preset: bool,
    pub fail_on_missing_handlers: bool,
    pub fail_on_ghost_events: bool,
    pub fail_on_races: bool,
    /// Maximum allowed dead exports before failing (CI policy)
    pub max_dead: Option<usize>,
    /// Maximum allowed circular imports before failing (CI policy)
    pub max_cycles: Option<usize>,
    pub ai_mode: bool,
    pub top_dead_symbols: usize,
    pub skip_dead_symbols: bool,
    pub scan_all: bool,
    pub symbol: Option<String>,
    pub impact: Option<String>,
    pub check_sim: Option<String>,
    pub dead_exports: bool,
    pub dead_confidence: Option<String>,
    pub show_ignored: bool,
    pub find_artifacts: bool,
    /// Tree mode: emit matching file paths only, one per line.
    pub tree_files_only: bool,
    /// Tree mode: regex filter applied to emitted relative paths.
    pub tree_path_filter: Option<String>,
    pub circular: bool,
    pub entrypoints: bool,
    pub py_races: bool,
    pub sarif: bool,
    pub full_scan: bool,
    pub slice_target: Option<String>,
    pub slice_consumers: bool,
    /// Force rescan before slicing (for uncommitted files)
    pub slice_rescan: bool,
    pub trace_handler: Option<String>,
    /// Unified search query
    pub search_query: Option<String>,
    /// Multi-query terms (used when `search_query_mode != Single`).
    pub search_queries: Vec<String>,
    /// How to interpret multi-term queries.
    pub search_query_mode: SearchQueryMode,
    /// Filter search to symbol matches only
    pub search_symbol_only: bool,
    /// Filter search to dead code only
    pub search_dead_only: bool,
    /// Filter search to semantic matches only
    pub search_semantic_only: bool,
    /// Auto mode: eagerly emit HTML/JSON/cycle artifacts into the artifacts dir (cache by default; set LOCT_CACHE_DIR to override).
    pub auto_outputs: bool,
    /// Filter search to exported symbols only
    pub search_exported_only: bool,
    /// Language filter for search
    pub search_lang: Option<String>,
    /// Limit search results
    pub search_limit: Option<usize>,
    /// Command name regex filter (commands subcommand)
    pub commands_name_filter: Option<String>,
    /// Only commands missing backend handlers
    pub commands_missing_only: bool,
    /// Only commands unused on frontend
    pub commands_unused_only: bool,
    /// Include tests in dead-export analysis
    pub with_tests: bool,
    /// Include helper/docs/scripts in dead-export analysis
    pub with_helpers: bool,
    /// Agent feed / JSON output mode
    pub for_agent_feed: bool,
    /// Write agent.json to disk
    pub agent_json: bool,
    /// Enforce fresh scan (no snapshot reuse) for agent mode
    pub force_full_scan: bool,
    /// Library/framework mode (ignore examples/demos from dead-code noise)
    pub library_mode: bool,
    /// Additional example/demo globs to ignore in library mode
    pub library_example_globs: Vec<String>,
    /// Python library mode: treat __all__ exports as public API
    pub python_library: bool,
}

impl Default for ParsedArgs {
    fn default() -> Self {
        Self {
            extensions: None,
            ignore_patterns: Vec::new(),
            ignore_symbols: None,
            ignore_symbols_preset: None,
            focus_patterns: Vec::new(),
            exclude_report_patterns: Vec::new(),
            graph: false,
            use_gitignore: true,
            max_depth: None,
            color: ColorMode::Auto,
            output: OutputMode::Human,
            json_output_path: None,
            summary: false,
            summary_limit: 5,
            summary_only: false,
            suppress_duplicates: false,
            suppress_dynamic: false,
            show_help: false,
            show_help_full: false,
            show_version: false,
            root_list: Vec::new(),
            py_roots: Vec::new(),
            show_hidden: false,
            loc_threshold: DEFAULT_LOC_THRESHOLD,
            mode: Mode::Tree,
            analyze_limit: 8,
            report_path: None,
            serve: false,
            serve_once: false,
            serve_port: None,
            editor_cmd: None,
            editor_kind: None,
            max_graph_nodes: None,
            max_graph_edges: None,
            verbose: false,
            tauri_preset: false,
            styles_preset: false,
            fail_on_missing_handlers: false,
            fail_on_ghost_events: false,
            fail_on_races: false,
            max_dead: None,
            max_cycles: None,
            ai_mode: false,
            top_dead_symbols: 20,
            skip_dead_symbols: false,
            scan_all: false,
            symbol: None,
            impact: None,
            check_sim: None,
            dead_exports: false,
            dead_confidence: None,
            show_ignored: false,
            find_artifacts: false,
            tree_files_only: false,
            tree_path_filter: None,
            circular: false,
            entrypoints: false,
            py_races: false,
            sarif: false,
            full_scan: false,
            slice_target: None,
            slice_consumers: true,
            slice_rescan: false,
            trace_handler: None,
            search_query: None,
            search_queries: Vec::new(),
            search_query_mode: SearchQueryMode::Single,
            search_symbol_only: false,
            search_dead_only: false,
            search_semantic_only: false,
            auto_outputs: false,
            search_exported_only: false,
            search_lang: None,
            search_limit: None,
            commands_name_filter: None,
            commands_missing_only: false,
            commands_unused_only: false,
            with_tests: false,
            with_helpers: false,
            for_agent_feed: false,
            agent_json: false,
            force_full_scan: false,
            library_mode: false,
            library_example_globs: Vec::new(),
            python_library: false,
        }
    }
}

fn parse_color_mode(raw: &str) -> Result<ColorMode, String> {
    match raw {
        "auto" => Ok(ColorMode::Auto),
        "always" => Ok(ColorMode::Always),
        "never" => Ok(ColorMode::Never),
        _ => Err("--color expects auto|always|never".to_string()),
    }
}

fn parse_summary_limit(raw: &str) -> Result<usize, String> {
    let value = raw
        .parse::<usize>()
        .map_err(|_| "--summary expects a positive integer".to_string())?;
    if value == 0 {
        Err("--summary expects a positive integer".to_string())
    } else {
        Ok(value)
    }
}

pub fn parse_extensions(raw: &str) -> Option<HashSet<String>> {
    let set: HashSet<String> = raw
        .split(',')
        .filter_map(|segment| {
            let trimmed = segment.trim().trim_start_matches('.').to_lowercase();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        })
        .collect();
    if set.is_empty() { None } else { Some(set) }
}

fn parse_glob_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .filter_map(|segment| {
            let trimmed = segment.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect()
}

fn parse_positive_usize(raw: &str, flag: &str) -> Result<usize, String> {
    let value = raw
        .parse::<usize>()
        .map_err(|_| format!("{flag} requires a positive integer"))?;
    if value == 0 {
        Err(format!("{flag} requires a positive integer"))
    } else {
        Ok(value)
    }
}

fn parse_port(raw: &str, flag: &str) -> Result<u16, String> {
    let value = raw
        .parse::<u16>()
        .map_err(|_| format!("{flag} requires a port number (0-65535)"))?;
    Ok(value)
}

fn validate_globs(patterns: &[String], flag: &str) -> Result<(), String> {
    for pat in patterns {
        if pat.trim().is_empty() {
            continue;
        }
        globset::Glob::new(pat).map_err(|e| format!("{flag}: invalid glob '{pat}': {e}"))?;
    }
    Ok(())
}

fn detect_glob_conflicts(focus: &[String], exclude: &[String]) -> Result<(), String> {
    if focus.is_empty() || exclude.is_empty() {
        return Ok(());
    }
    let focus_set: std::collections::HashSet<_> = focus.iter().collect();
    let exclude_set: std::collections::HashSet<_> = exclude.iter().collect();
    let duplicates: Vec<_> = focus_set
        .intersection(&exclude_set)
        .map(|s| s.to_string())
        .collect();
    if !duplicates.is_empty() {
        return Err(format!(
            "Conflicting globs between --focus and --exclude-report: {}",
            duplicates.join(", ")
        ));
    }
    Ok(())
}

pub fn parse_ignore_symbols(raw: &str) -> Option<HashSet<String>> {
    let set: HashSet<String> = raw
        .split(',')
        .filter_map(|segment| {
            let trimmed = segment.trim().to_lowercase();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        })
        .collect();
    if set.is_empty() { None } else { Some(set) }
}

pub fn preset_ignore_symbols(name: &str) -> Option<HashSet<String>> {
    match name.to_lowercase().as_str() {
        "common" => Some(
            ["main", "run", "setup", "test_*", "tests_*"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        ),
        "tauri" => Some(
            [
                // language-level boilerplate often duplicated across crates/configs
                "default", "new", "from", "try_from", "from_str", "into", "build", "init", "config",
                "main", "run", "setup", // python/ts interop noise
                "__all__", "__init__", "test_*", "tests_*",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        ),
        _ => None,
    }
}

pub fn parse_args() -> Result<ParsedArgs, String> {
    // SAFETY: This is the CLI trust boundary. `args_os()` returns raw process arguments
    // straight from the OS — by definition the entry point of user input. Downstream code
    // MUST validate any path/identifier derived from these args before privileged use
    // (see `semantic::io::validate_and_canonicalize` for the canonical sanitizer).
    // Rule suppression is enforced at file scope via `.semgrepignore` (CLI ENTRY POINTS).
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    let mut parsed = ParsedArgs {
        ..ParsedArgs::default()
    };

    let mut roots: Vec<PathBuf> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "tauri" | "--preset-tauri" => {
                parsed.tauri_preset = true;
                i += 1;
            }
            "styles" | "--preset-styles" => {
                parsed.styles_preset = true;
                parsed.mode = Mode::AnalyzeImports;
                i += 1;
            }
            "init" | "--init" => {
                parsed.mode = Mode::Init;
                i += 1;
            }
            "--ai" => {
                parsed.ai_mode = true;
                parsed.output = OutputMode::Json;
                parsed.summary = true;
                parsed.mode = Mode::AnalyzeImports;
                i += 1;
            }
            "--help" | "-h" => {
                parsed.show_help = true;
                i += 1;
            }
            "--help-full" => {
                parsed.show_help_full = true;
                i += 1;
            }
            "--tree" | "tree" => {
                parsed.mode = Mode::Tree;
                i += 1;
            }
            "--version" | "-V" => {
                parsed.show_version = true;
                i += 1;
            }
            "--color" | "-c" => {
                if let Some(next) = args.get(i + 1)
                    && !next.starts_with('-')
                {
                    parsed.color = parse_color_mode(next)?;
                    i += 2;
                    continue;
                }
                parsed.color = ColorMode::Always;
                i += 1;
            }
            _ if arg.starts_with("--color=") => {
                let value = arg.trim_start_matches("--color=");
                parsed.color = parse_color_mode(value)?;
                i += 1;
            }
            "--gitignore" | "-g" => {
                parsed.use_gitignore = true;
                i += 1;
            }
            "--no-gitignore" => {
                parsed.use_gitignore = false;
                i += 1;
            }
            "--graph" => {
                parsed.graph = true;
                i += 1;
            }
            "--library-mode" => {
                parsed.library_mode = true;
                i += 1;
            }
            "--verbose" | "-v" => {
                parsed.verbose = true;
                i += 1;
            }
            "--quiet" | "-q" => {
                // Recognized but not used in legacy path (legacy mode doesn't emit progress)
                // This prevents the "Ignoring unknown flag" warning
                i += 1;
            }
            "--fail-on-missing-handlers" => {
                parsed.fail_on_missing_handlers = true;
                i += 1;
            }
            "--fail-on-ghost-events" => {
                parsed.fail_on_ghost_events = true;
                i += 1;
            }
            "--fail-on-races" => {
                parsed.fail_on_races = true;
                i += 1;
            }
            "--max-dead" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--max-dead requires a non-negative integer".to_string())?;
                let value = next
                    .parse::<usize>()
                    .map_err(|_| "--max-dead requires a non-negative integer".to_string())?;
                parsed.max_dead = Some(value);
                i += 2;
            }
            _ if arg.starts_with("--max-dead=") => {
                let value = arg
                    .trim_start_matches("--max-dead=")
                    .parse::<usize>()
                    .map_err(|_| "--max-dead requires a non-negative integer".to_string())?;
                parsed.max_dead = Some(value);
                i += 1;
            }
            "--max-cycles" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--max-cycles requires a non-negative integer".to_string())?;
                let value = next
                    .parse::<usize>()
                    .map_err(|_| "--max-cycles requires a non-negative integer".to_string())?;
                parsed.max_cycles = Some(value);
                i += 2;
            }
            _ if arg.starts_with("--max-cycles=") => {
                let value = arg
                    .trim_start_matches("--max-cycles=")
                    .parse::<usize>()
                    .map_err(|_| "--max-cycles requires a non-negative integer".to_string())?;
                parsed.max_cycles = Some(value);
                i += 1;
            }
            "--show-hidden" | "-H" => {
                parsed.show_hidden = true;
                i += 1;
            }
            "--show-ignored" => {
                parsed.show_ignored = true;
                parsed.use_gitignore = true; // Required to know what's ignored
                parsed.mode = Mode::Tree; // Show-ignored works in tree mode
                i += 1;
            }
            "--find-artifacts" => {
                parsed.find_artifacts = true;
                parsed.mode = Mode::Tree; // Find-artifacts works in tree mode
                i += 1;
            }
            "--json" => {
                parsed.output = OutputMode::Json;
                if let Some(next) = args.get(i + 1)
                    && !next.starts_with('-')
                {
                    parsed.json_output_path = Some(PathBuf::from(next));
                    i += 2;
                    continue;
                }
                i += 1;
            }
            "--json-out" | "--json-output" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--json-out requires a file path".to_string())?;
                parsed.output = OutputMode::Json;
                parsed.json_output_path = Some(PathBuf::from(next));
                i += 2;
            }
            "--jsonl" => {
                parsed.output = OutputMode::Jsonl;
                i += 1;
            }
            "--html-report" | "--report" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--html-report requires a file path".to_string())?;
                parsed.report_path = Some(PathBuf::from(next));
                i += 2;
            }
            "--serve" | "--serve-keepalive" | "--serve-wait" => {
                parsed.serve = true;
                i += 1;
            }
            "--serve-once" => {
                parsed.serve = true;
                parsed.serve_once = true;
                i += 1;
            }
            "--port" | "--serve-port" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--port requires a value".to_string())?;
                parsed.serve_port = Some(parse_port(next, "--port")?);
                i += 2;
            }
            "--editor-cmd" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--editor-cmd requires a command template".to_string())?;
                parsed.editor_cmd = Some(next.clone());
                i += 2;
            }
            "--editor" => {
                let next = args.get(i + 1).ok_or_else(|| {
                    "--editor requires a value (code|cursor|windsurf|jetbrains|none)".to_string()
                })?;
                parsed.editor_kind = Some(next.clone());
                i += 2;
            }
            _ if arg.starts_with("--editor=") => {
                let value = arg.trim_start_matches("--editor=");
                parsed.editor_kind = Some(value.to_string());
                i += 1;
            }
            "--summary" => {
                parsed.summary = true;
                if let Some(next) = args.get(i + 1)
                    && !next.starts_with('-')
                {
                    parsed.summary_limit = parse_summary_limit(next)?;
                    i += 2;
                    continue;
                }
                i += 1;
            }
            _ if arg.starts_with("--summary=") => {
                let value = arg.trim_start_matches("--summary=");
                parsed.summary = true;
                parsed.summary_limit = parse_summary_limit(value)?;
                i += 1;
            }
            "--loc" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--loc requires a positive integer".to_string())?;
                parsed.loc_threshold = parse_positive_usize(next, "--loc")?;
                i += 2;
            }
            "--limit" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--limit requires a positive integer".to_string())?;
                parsed.analyze_limit = parse_positive_usize(next, "--limit")?;
                i += 2;
            }
            "--top-dead-symbols" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--top-dead-symbols requires a positive integer".to_string())?;
                parsed.top_dead_symbols = parse_positive_usize(next, "--top-dead-symbols")?;
                i += 2;
            }
            "--skip-dead-symbols" => {
                parsed.skip_dead_symbols = true;
                i += 1;
            }
            "--scan-all" => {
                parsed.scan_all = true;
                i += 1;
            }
            "--symbol" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--symbol requires a value".to_string())?;
                parsed.symbol = Some(next.clone());
                i += 2;
            }
            _ if arg.starts_with("--symbol=") => {
                let value = arg.trim_start_matches("--symbol=");
                parsed.symbol = Some(value.to_string());
                i += 1;
            }
            "--impact" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--impact requires a file path or glob".to_string())?;
                parsed.impact = Some(next.clone());
                i += 2;
            }
            _ if arg.starts_with("--impact=") => {
                let value = arg.trim_start_matches("--impact=");
                parsed.impact = Some(value.to_string());
                i += 1;
            }
            "--check" | "--sim" | "--find-similar" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--check/--sim requires a query string".to_string())?;
                parsed.check_sim = Some(next.clone());
                parsed.mode = Mode::AnalyzeImports;
                i += 2;
            }
            _ if arg.starts_with("--check=") => {
                let value = arg.trim_start_matches("--check=");
                parsed.check_sim = Some(value.to_string());
                parsed.mode = Mode::AnalyzeImports;
                i += 1;
            }
            _ if arg.starts_with("--sim=") => {
                let value = arg.trim_start_matches("--sim=");
                parsed.check_sim = Some(value.to_string());
                parsed.mode = Mode::AnalyzeImports;
                i += 1;
            }
            "--dead" | "--unused" => {
                parsed.dead_exports = true;
                parsed.mode = Mode::AnalyzeImports;
                i += 1;
            }
            "--circular" => {
                parsed.circular = true;
                parsed.mode = Mode::AnalyzeImports;
                i += 1;
            }
            "--entrypoints" => {
                parsed.entrypoints = true;
                parsed.mode = Mode::AnalyzeImports;
                i += 1;
            }
            "--py-races" => {
                parsed.py_races = true;
                parsed.mode = Mode::AnalyzeImports;
                i += 1;
            }
            "--sarif" => {
                parsed.sarif = true;
                parsed.output = OutputMode::Json;
                parsed.mode = Mode::AnalyzeImports;
                i += 1;
            }
            "--full-scan" => {
                parsed.full_scan = true;
                i += 1;
            }
            "--consumers" => {
                parsed.slice_consumers = true;
                i += 1;
            }
            "--no-consumers" => {
                parsed.slice_consumers = false;
                i += 1;
            }
            "slice" | "--slice" => {
                parsed.mode = Mode::Slice;
                if let Some(next) = args.get(i + 1)
                    && !next.starts_with('-')
                {
                    parsed.slice_target = Some(next.clone());
                    i += 2;
                    continue;
                }
                i += 1;
            }
            "trace" | "--trace" => {
                parsed.mode = Mode::Trace;
                if let Some(next) = args.get(i + 1)
                    && !next.starts_with('-')
                {
                    parsed.trace_handler = Some(next.clone());
                    i += 2;
                    continue;
                }
                i += 1;
            }
            "search" => {
                parsed.mode = Mode::Search;
                if let Some(next) = args.get(i + 1)
                    && !next.starts_with('-')
                {
                    parsed.search_query = Some(next.clone());
                    i += 2;
                    continue;
                }
                i += 1;
            }
            "--symbol-only" => {
                parsed.search_symbol_only = true;
                i += 1;
            }
            "--dead-only" => {
                parsed.search_dead_only = true;
                i += 1;
            }
            "--semantic-only" | "--sem-only" => {
                parsed.search_semantic_only = true;
                i += 1;
            }
            "git" => {
                // Parse git subcommand: compare, blame, history, when-introduced
                // NO passthrough commands - agents can call git directly
                let subcommand = args.get(i + 1).ok_or_else(|| {
                    "git requires a subcommand: compare, blame, history, or when-introduced"
                        .to_string()
                })?;
                match subcommand.as_str() {
                    "compare" => {
                        // loctree git compare <from> [to]
                        // loctree git compare HEAD~1..HEAD
                        // loctree git compare HEAD~1 (compares to working tree)
                        let from_arg = args.get(i + 2).ok_or_else(|| {
                            "git compare requires at least one commit reference (e.g., HEAD~1 or abc123..def456)".to_string()
                        })?;

                        // Check if it's range notation (e.g., HEAD~1..HEAD)
                        if from_arg.contains("..") {
                            let parts: Vec<&str> = from_arg.split("..").collect();
                            if parts.len() != 2 {
                                return Err(
                                    "Invalid range format. Use: commit1..commit2".to_string()
                                );
                            }
                            parsed.mode = Mode::Git(GitSubcommand::Compare {
                                from: parts[0].to_string(),
                                to: Some(parts[1].to_string()),
                            });
                            i += 3;
                        } else {
                            // Check for optional second argument
                            let to = args.get(i + 3).and_then(|t| {
                                if t.starts_with('-') {
                                    None
                                } else {
                                    Some(t.clone())
                                }
                            });
                            parsed.mode = Mode::Git(GitSubcommand::Compare {
                                from: from_arg.clone(),
                                to: to.clone(),
                            });
                            i += if to.is_some() { 4 } else { 3 };
                        }
                    }
                    "blame" => {
                        // loctree git blame <file>
                        let file = args
                            .get(i + 2)
                            .ok_or_else(|| "git blame requires a file path".to_string())?;
                        parsed.mode = Mode::Git(GitSubcommand::Blame { file: file.clone() });
                        i += 3;
                    }
                    "history" => {
                        // loctree git history [--symbol <name>] [--file <path>] [--limit <n>]
                        let mut symbol = None;
                        let mut file = None;
                        let mut limit = 10usize; // default
                        let mut j = i + 2;

                        while j < args.len() {
                            match args[j].as_str() {
                                "--symbol" => {
                                    symbol = args.get(j + 1).cloned();
                                    j += 2;
                                }
                                "--file" => {
                                    file = args.get(j + 1).cloned();
                                    j += 2;
                                }
                                "--limit" => {
                                    if let Some(l) = args.get(j + 1) {
                                        limit = l.parse().unwrap_or(10);
                                    }
                                    j += 2;
                                }
                                _ if !args[j].starts_with('-')
                                    && symbol.is_none()
                                    && file.is_none() =>
                                {
                                    // First positional arg is symbol
                                    symbol = Some(args[j].clone());
                                    j += 1;
                                }
                                _ => break,
                            }
                        }

                        if symbol.is_none() && file.is_none() {
                            return Err(
                                "git history requires --symbol <name> or --file <path>".to_string()
                            );
                        }

                        parsed.mode = Mode::Git(GitSubcommand::History {
                            symbol,
                            file,
                            limit,
                        });
                        i = j;
                    }
                    "when-introduced" => {
                        // loctree git when-introduced --circular "src/a.rs <-> src/b.rs"
                        // loctree git when-introduced --dead "src/utils.rs::unused_fn"
                        // loctree git when-introduced --import "lodash"
                        let mut circular = None;
                        let mut dead = None;
                        let mut import = None;
                        let mut j = i + 2;

                        while j < args.len() {
                            match args[j].as_str() {
                                "--circular" => {
                                    circular = args.get(j + 1).cloned();
                                    j += 2;
                                }
                                "--dead" => {
                                    dead = args.get(j + 1).cloned();
                                    j += 2;
                                }
                                "--import" => {
                                    import = args.get(j + 1).cloned();
                                    j += 2;
                                }
                                _ => break,
                            }
                        }

                        if circular.is_none() && dead.is_none() && import.is_none() {
                            return Err(
                                "git when-introduced requires --circular, --dead, or --import"
                                    .to_string(),
                            );
                        }

                        parsed.mode = Mode::Git(GitSubcommand::WhenIntroduced {
                            circular,
                            dead,
                            import,
                        });
                        i = j;
                    }
                    _ => {
                        return Err(format!(
                            "Unknown git subcommand '{}'. Use: compare, blame, history, or when-introduced",
                            subcommand
                        ));
                    }
                }
            }
            "--for-ai" | "for-ai" => {
                parsed.mode = Mode::ForAi;
                parsed.output = OutputMode::Json;
                parsed.for_agent_feed = true; // Needed for twins_data in sections
                i += 1;
            }
            "--for-agent-feed" => {
                parsed.mode = Mode::ForAi;
                parsed.output = OutputMode::Jsonl;
                parsed.for_agent_feed = true; // Needed for twins_data in sections
                i += 1;
            }
            "--confidence" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--confidence requires a value".to_string())?;
                parsed.dead_confidence = Some(next.clone());
                i += 2;
            }
            _ if arg.starts_with("--confidence=") => {
                let value = arg.trim_start_matches("--confidence=");
                parsed.dead_confidence = Some(value.to_string());
                i += 1;
            }
            "--analyze-imports" | "-A" => {
                parsed.mode = Mode::AnalyzeImports;
                i += 1;
            }
            "-L" | "--max-depth" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "-L/--max-depth requires a non-negative integer".to_string())?;
                let depth = next
                    .parse::<usize>()
                    .map_err(|_| "-L/--max-depth requires a non-negative integer".to_string())?;
                parsed.max_depth = Some(depth);
                i += 2;
            }
            "--ext" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--ext requires a comma-separated value".to_string())?;
                parsed.extensions = parse_extensions(next);
                i += 2;
            }
            _ if arg.starts_with("--ext=") => {
                let value = arg.trim_start_matches("--ext=");
                parsed.extensions = parse_extensions(value);
                i += 1;
            }
            "--ignore-symbols" => {
                let next = args.get(i + 1).ok_or_else(|| {
                    "--ignore-symbols requires a comma-separated list".to_string()
                })?;
                parsed.ignore_symbols = parse_ignore_symbols(next);
                i += 2;
            }
            _ if arg.starts_with("--ignore-symbols=") => {
                let value = arg.trim_start_matches("--ignore-symbols=");
                parsed.ignore_symbols = parse_ignore_symbols(value);
                i += 1;
            }
            "--ignore-symbols-preset" => {
                let next = args.get(i + 1).ok_or_else(|| {
                    "--ignore-symbols-preset requires a name (e.g. common)".to_string()
                })?;
                parsed.ignore_symbols_preset = Some(next.clone());
                i += 2;
            }
            _ if arg.starts_with("--ignore-symbols-preset=") => {
                let value = arg.trim_start_matches("--ignore-symbols-preset=");
                parsed.ignore_symbols_preset = Some(value.to_string());
                i += 1;
            }
            "--focus" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--focus requires a glob or comma list".to_string())?;
                parsed.focus_patterns.extend(parse_glob_list(next));
                i += 2;
            }
            _ if arg.starts_with("--focus=") => {
                let value = arg.trim_start_matches("--focus=");
                parsed.focus_patterns.extend(parse_glob_list(value));
                i += 1;
            }
            "--exclude-report" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--exclude-report requires a glob or comma list".to_string())?;
                parsed.exclude_report_patterns.extend(parse_glob_list(next));
                i += 2;
            }
            _ if arg.starts_with("--exclude-report=") => {
                let value = arg.trim_start_matches("--exclude-report=");
                parsed
                    .exclude_report_patterns
                    .extend(parse_glob_list(value));
                i += 1;
            }
            "-I" | "--ignore" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "-I/--ignore requires a path argument".to_string())?;
                parsed.ignore_patterns.push(next.clone());
                i += 2;
            }
            "--max-nodes" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--max-nodes requires a positive integer".to_string())?;
                parsed.max_graph_nodes = Some(parse_positive_usize(next, "--max-nodes")?);
                i += 2;
            }
            "--max-edges" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--max-edges requires a positive integer".to_string())?;
                parsed.max_graph_edges = Some(parse_positive_usize(next, "--max-edges")?);
                i += 2;
            }
            "--py-root" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "--py-root requires a path".to_string())?;
                parsed.py_roots.push(PathBuf::from(next));
                i += 2;
            }
            _ if arg.starts_with("--py-root=") => {
                let value = arg.trim_start_matches("--py-root=");
                parsed.py_roots.push(PathBuf::from(value));
                i += 1;
            }
            _ if arg.starts_with('-') => {
                eprintln!("Ignoring unknown flag {}", arg);
                i += 1;
            }
            _ => {
                let trimmed = arg.trim();
                if !trimmed.is_empty() {
                    roots.push(PathBuf::from(trimmed));
                }
                i += 1;
            }
        }
    }

    if parsed.tauri_preset {
        if parsed.extensions.is_none() {
            parsed.extensions = Some(
                ["ts", "tsx", "js", "jsx", "mjs", "cjs", "rs", "css"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            );
        }
        if roots.is_empty() {
            roots.push(PathBuf::from("."));
        }
        parsed.mode = Mode::AnalyzeImports;
        parsed.graph = true;
        parsed.use_gitignore = true;
        if parsed.ignore_patterns.is_empty() {
            parsed.ignore_patterns.extend(
                [
                    "node_modules",
                    "dist",
                    "target",
                    "build",
                    "coverage",
                    "docs/*.json",
                ]
                .iter()
                .map(|s| s.to_string()),
            );
        }
        if parsed.ignore_symbols.is_none() && parsed.ignore_symbols_preset.is_none() {
            parsed.ignore_symbols_preset = Some("tauri".to_string());
        }
        // Sanity check: warn if Tauri structure not detected
        let check_root = roots.first().cloned().unwrap_or_else(|| PathBuf::from("."));
        let has_tauri_backend = check_root.join("src-tauri/Cargo.toml").exists()
            || check_root.join("src-tauri").exists();
        if !has_tauri_backend {
            // Detect what the project actually is
            let has_python =
                check_root.join("pyproject.toml").exists() || check_root.join("setup.py").exists();
            let has_rust = check_root.join("Cargo.toml").exists();
            let has_ts = check_root.join("tsconfig.json").exists()
                || check_root.join("package.json").exists();

            let suggestion = if has_python {
                "Try: loctree init  (Python auto-detected)"
            } else if has_rust {
                "Try: loctree init  (Rust auto-detected)"
            } else if has_ts {
                "Try: loctree init  (TypeScript auto-detected)"
            } else {
                "Try: loctree init --ext py,rs,ts  (specify extensions)"
            };

            eprintln!(
                "[loctree][warn] --preset-tauri: No src-tauri/ found. {}",
                suggestion
            );
        }
    }

    // Default to Init mode when running bare `loctree` without any mode-setting flags
    // This implements "scan once" - bare loctree creates/updates the snapshot
    if roots.is_empty()
        && matches!(parsed.mode, Mode::Tree)
        && !parsed.summary
        && parsed.extensions.is_none()
    {
        parsed.mode = Mode::Init;
        parsed.use_gitignore = true;
    }

    if roots.is_empty() {
        roots.push(PathBuf::from("."));
    }
    for root in &roots {
        if !root.exists() {
            return Err(format!(
                "Path '{}' does not exist. Provide a valid file or directory.",
                root.display()
            ));
        }
        if root.is_file() && matches!(parsed.mode, Mode::AnalyzeImports) {
            return Err(format!(
                "Path '{}' is a file; import analyzer expects a directory.",
                root.display()
            ));
        }
    }
    parsed.root_list = roots;

    validate_globs(&parsed.focus_patterns, "--focus")?;
    validate_globs(&parsed.exclude_report_patterns, "--exclude-report")?;
    detect_glob_conflicts(&parsed.focus_patterns, &parsed.exclude_report_patterns)?;

    if parsed.serve {
        // --serve implies full analysis + HTML report generation
        parsed.mode = Mode::AnalyzeImports;
    }

    for extra in &parsed.py_roots {
        if !extra.exists() {
            return Err(format!(
                "--py-root '{}' does not exist. Provide a valid directory.",
                extra.display()
            ));
        }
        if !extra.is_dir() {
            return Err(format!(
                "--py-root '{}' is not a directory.",
                extra.display()
            ));
        }
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_extensions() {
        let res = parse_extensions("rs,ts").expect("parse extensions");
        assert!(res.contains("rs"));
        assert!(res.contains("ts"));
        assert_eq!(res.len(), 2);
    }

    #[test]
    fn test_parse_extensions_empty() {
        assert!(parse_extensions("").is_none());
    }

    #[test]
    fn test_parse_extensions_with_dots() {
        // Extensions with leading dots should be trimmed
        let res = parse_extensions(".rs, .ts, .js").expect("parse extensions");
        assert!(res.contains("rs"));
        assert!(res.contains("ts"));
        assert!(res.contains("js"));
        assert_eq!(res.len(), 3);
    }

    #[test]
    fn test_parse_extensions_uppercase() {
        // Extensions should be lowercased
        let res = parse_extensions("RS,TS").expect("parse extensions");
        assert!(res.contains("rs"));
        assert!(res.contains("ts"));
    }

    #[test]
    fn test_parse_extensions_empty_segments() {
        // Empty segments should be ignored
        let res = parse_extensions("rs,,ts, ,js").expect("parse extensions");
        assert_eq!(res.len(), 3);
    }

    #[test]
    fn test_parse_color_mode() {
        assert_eq!(
            parse_color_mode("always").expect("color always"),
            ColorMode::Always
        );
        assert_eq!(
            parse_color_mode("never").expect("color never"),
            ColorMode::Never
        );
        assert!(parse_color_mode("invalid").is_err());
    }

    #[test]
    fn test_parse_color_mode_auto() {
        assert_eq!(
            parse_color_mode("auto").expect("color auto"),
            ColorMode::Auto
        );
    }

    #[test]
    fn test_parse_summary_limit() {
        assert_eq!(parse_summary_limit("5").expect("summary"), 5);
        assert!(parse_summary_limit("0").is_err());
        assert!(parse_summary_limit("abc").is_err());
    }

    #[test]
    fn test_parse_summary_limit_large() {
        assert_eq!(parse_summary_limit("100").expect("summary"), 100);
    }

    #[test]
    fn test_parse_glob_list() {
        let list = parse_glob_list("src/**,tests/**,lib/*");
        assert_eq!(list.len(), 3);
        assert!(list.contains(&"src/**".to_string()));
        assert!(list.contains(&"tests/**".to_string()));
        assert!(list.contains(&"lib/*".to_string()));
    }

    #[test]
    fn test_parse_glob_list_with_spaces() {
        let list = parse_glob_list(" src/** , tests/** ");
        assert_eq!(list.len(), 2);
        assert!(list.contains(&"src/**".to_string()));
        assert!(list.contains(&"tests/**".to_string()));
    }

    #[test]
    fn test_parse_glob_list_empty() {
        let list = parse_glob_list("");
        assert!(list.is_empty());

        let list = parse_glob_list("  ,  ,  ");
        assert!(list.is_empty());
    }

    #[test]
    fn test_parse_positive_usize() {
        assert_eq!(parse_positive_usize("10", "--limit").expect("parse"), 10);
        assert_eq!(parse_positive_usize("1", "--limit").expect("parse"), 1);
    }

    #[test]
    fn test_parse_positive_usize_zero_error() {
        let result = parse_positive_usize("0", "--limit");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires a positive integer"));
    }

    #[test]
    fn test_parse_positive_usize_non_numeric() {
        let result = parse_positive_usize("abc", "--limit");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_port() {
        assert_eq!(parse_port("8080", "--port").expect("port"), 8080);
        assert_eq!(parse_port("0", "--port").expect("port"), 0);
        assert_eq!(parse_port("65535", "--port").expect("port"), 65535);
    }

    #[test]
    fn test_parse_port_invalid() {
        let result = parse_port("abc", "--port");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("port number"));
    }

    #[test]
    fn test_validate_globs_valid() {
        let patterns = vec!["src/**".to_string(), "*.rs".to_string()];
        assert!(validate_globs(&patterns, "--focus").is_ok());
    }

    #[test]
    fn test_validate_globs_invalid() {
        let patterns = vec!["[invalid".to_string()]; // Unclosed bracket
        assert!(validate_globs(&patterns, "--focus").is_err());
    }

    #[test]
    fn test_validate_globs_empty() {
        let patterns: Vec<String> = vec![];
        assert!(validate_globs(&patterns, "--focus").is_ok());

        let patterns = vec!["".to_string(), "  ".to_string()];
        assert!(validate_globs(&patterns, "--focus").is_ok()); // Empty patterns are skipped
    }

    #[test]
    fn test_parse_ignore_symbols() {
        let result = parse_ignore_symbols("main,setup,test").expect("parse");
        assert!(result.contains("main"));
        assert!(result.contains("setup"));
        assert!(result.contains("test"));
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_parse_ignore_symbols_empty() {
        assert!(parse_ignore_symbols("").is_none());
        assert!(parse_ignore_symbols("  ,  ,  ").is_none());
    }

    #[test]
    fn test_parse_ignore_symbols_case_insensitive() {
        let result = parse_ignore_symbols("Main,SETUP").expect("parse");
        assert!(result.contains("main"));
        assert!(result.contains("setup"));
    }

    #[test]
    fn test_preset_ignore_symbols_common() {
        let symbols = preset_ignore_symbols("common").expect("preset common");
        assert!(symbols.contains("main"));
        assert!(symbols.contains("run"));
        assert!(symbols.contains("setup"));
    }

    #[test]
    fn test_preset_ignore_symbols_tauri() {
        let symbols = preset_ignore_symbols("tauri").expect("preset tauri");
        assert!(symbols.contains("main"));
        assert!(symbols.contains("default"));
        assert!(symbols.contains("new"));
        assert!(symbols.contains("from"));
    }

    #[test]
    fn test_preset_ignore_symbols_unknown() {
        assert!(preset_ignore_symbols("unknown").is_none());
    }

    #[test]
    fn test_preset_ignore_symbols_case_insensitive() {
        assert!(preset_ignore_symbols("COMMON").is_some());
        assert!(preset_ignore_symbols("Tauri").is_some());
    }

    #[test]
    fn detects_glob_conflicts() {
        let focus = vec!["src/**".to_string(), "pkg/**".to_string()];
        let exclude = vec!["pkg/**".to_string()];
        assert!(detect_glob_conflicts(&focus, &exclude).is_err());
    }

    #[test]
    fn allows_distinct_globs() {
        let focus = vec!["src/**".to_string()];
        let exclude = vec!["tests/**".to_string()];
        assert!(detect_glob_conflicts(&focus, &exclude).is_ok());
    }

    #[test]
    fn detect_glob_conflicts_empty_lists() {
        // Empty lists should not conflict
        assert!(detect_glob_conflicts(&[], &[]).is_ok());
        assert!(detect_glob_conflicts(&["src/**".to_string()], &[]).is_ok());
        assert!(detect_glob_conflicts(&[], &["tests/**".to_string()]).is_ok());
    }

    #[test]
    fn test_parsed_args_default() {
        let args = ParsedArgs::default();
        assert!(args.extensions.is_none());
        assert!(args.ignore_patterns.is_empty());
        assert!(!args.graph);
        assert!(args.use_gitignore); // Default: respect gitignore
        assert!(args.max_depth.is_none());
        assert_eq!(args.color, ColorMode::Auto);
        assert_eq!(args.output, OutputMode::Human);
        assert!(!args.summary);
        assert_eq!(args.summary_limit, 5);
        assert!(!args.show_help);
        assert!(!args.show_version);
        assert!(args.root_list.is_empty());
        assert_eq!(args.loc_threshold, DEFAULT_LOC_THRESHOLD);
        assert!(matches!(args.mode, Mode::Tree));
        assert_eq!(args.analyze_limit, 8);
        assert!(!args.verbose);
        assert!(!args.tauri_preset);
        assert!(!args.fail_on_missing_handlers);
        assert!(args.max_dead.is_none());
        assert!(args.max_cycles.is_none());
        assert!(!args.ai_mode);
        assert_eq!(args.top_dead_symbols, 20);
        assert!(!args.dead_exports);
        assert!(!args.circular);
        assert!(!args.entrypoints);
        assert!(!args.sarif);
    }
}
