use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;

use globset::GlobSet;
use serde_json::json;

use crate::args::ParsedArgs;
use crate::fs_utils::{GitIgnoreChecker, gather_files};
use crate::snapshot::Snapshot;
use crate::types::{
    ColorMode, ExportIndex, FileAnalysis, ImportKind, Options, OutputMode, PayloadMap,
};

use super::classify::is_dev_file;
use super::resolvers::{
    ExtractedResolverConfig, TsPathResolver, resolve_js_relative, resolve_python_absolute,
    resolve_python_relative, resolve_rust_import,
};
use super::scan::{
    AnalyzeContext, analyze_file, matches_focus, resolve_event_constants_across_files,
    strip_excluded,
};
use super::{DupLocation, DupSeverity, RankedDup, coverage::CommandUsage, twins};

pub struct ScanConfig<'a> {
    pub roots: &'a [PathBuf],
    pub parsed: &'a ParsedArgs,
    pub extensions: Option<HashSet<String>>,
    pub focus_set: &'a Option<GlobSet>,
    pub exclude_set: &'a Option<GlobSet>,
    pub ignore_exact: HashSet<String>,
    pub ignore_prefixes: Vec<String>,
    pub py_stdlib: &'a HashSet<String>,
    /// Cached file analyses from previous snapshot for incremental scanning
    pub cached_analyses: Option<&'a HashMap<String, crate::types::FileAnalysis>>,
    /// Force collection of graph edges (for Init mode snapshots)
    pub collect_edges: bool,
    /// Custom Tauri command macros from .loctree/config.toml
    pub custom_command_macros: &'a [String],
    /// Command detection exclusions (DOM/invoke/invalid) from config
    pub command_detection: crate::analyzer::ast_js::CommandDetectionConfig,
}

#[derive(Clone)]
pub struct RootContext {
    pub root_path: PathBuf,
    pub options: Options,
    pub analyses: Vec<FileAnalysis>,
    pub export_index: ExportIndex,
    pub dynamic_summary: Vec<(String, Vec<String>)>,
    pub cascades: Vec<(String, String)>,
    pub filtered_ranked: Vec<RankedDup>,
    pub graph_edges: Vec<(String, String, String)>,
    pub loc_map: HashMap<String, usize>,
    pub languages: HashSet<String>,
    pub tsconfig_summary: serde_json::Value,
    pub calls_with_generics: Vec<serde_json::Value>,
    pub renamed_handlers: Vec<serde_json::Value>,
    pub barrels: Vec<BarrelInfo>,
}

impl Default for RootContext {
    fn default() -> Self {
        Self {
            root_path: PathBuf::new(),
            options: Options::default(),
            analyses: Vec::new(),
            export_index: HashMap::new(),
            dynamic_summary: Vec::new(),
            cascades: Vec::new(),
            filtered_ranked: Vec::new(),
            graph_edges: Vec::new(),
            loc_map: HashMap::new(),
            languages: HashSet::new(),
            tsconfig_summary: serde_json::json!({}),
            calls_with_generics: Vec::new(),
            renamed_handlers: Vec::new(),
            barrels: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct BarrelInfo {
    pub path: String,
    pub module_id: String,
    pub reexport_count: usize,
    pub target_count: usize,
    pub mixed: bool,
    pub targets: Vec<String>,
}

#[derive(Clone)]
pub struct ScanResults {
    pub contexts: Vec<RootContext>,
    pub global_fe_commands: CommandUsage,
    pub global_be_commands: CommandUsage,
    pub global_fe_payloads: PayloadMap,
    pub global_be_payloads: PayloadMap,
    pub global_analyses: Vec<FileAnalysis>,
    /// Extracted TypeScript resolver configuration for caching in snapshot
    pub ts_resolver_config: Option<ExtractedResolverConfig>,
    /// Python root paths for caching in snapshot
    pub py_roots: Vec<String>,
}

/// Result from scanning a single root (used for parallel processing)
struct SingleRootResult {
    context: RootContext,
    fe_commands: CommandUsage,
    be_commands: CommandUsage,
    fe_payloads: PayloadMap,
    be_payloads: PayloadMap,
    analyses: Vec<FileAnalysis>,
    /// Extracted TS resolver config (if tsconfig found in this root)
    ts_resolver_config: Option<ExtractedResolverConfig>,
    /// Python roots used for this root scan
    py_roots: Vec<String>,
}

/// Maximum number of roots to scan in parallel (bounded parallelism)
const MAX_PARALLEL_ROOTS: usize = 4;

pub fn scan_roots(cfg: ScanConfig<'_>) -> io::Result<ScanResults> {
    // For single root, skip parallelization overhead
    if cfg.roots.len() <= 1 {
        return scan_roots_sequential(cfg);
    }

    // Parallel scanning with bounded concurrency
    let results: Mutex<Vec<SingleRootResult>> = Mutex::new(Vec::new());
    let errors: Mutex<Vec<(PathBuf, io::Error)>> = Mutex::new(Vec::new());

    thread::scope(|s| {
        // Process roots in chunks for bounded parallelism
        for chunk in cfg.roots.chunks(MAX_PARALLEL_ROOTS) {
            let handles: Vec<_> = chunk
                .iter()
                .map(|root_path| s.spawn(|| scan_single_root(root_path, &cfg)))
                .collect();

            for (handle, root_path) in handles.into_iter().zip(chunk.iter()) {
                match handle.join() {
                    Ok(Ok(result)) => {
                        let mut guard = match results.lock() {
                            Ok(g) => g,
                            Err(poisoned) => {
                                eprintln!(
                                    "[loctree][warn] result mutex poisoned; salvaging partial data"
                                );
                                poisoned.into_inner()
                            }
                        };
                        guard.push(result);
                    }
                    Ok(Err(e)) => {
                        let mut guard = match errors.lock() {
                            Ok(g) => g,
                            Err(poisoned) => {
                                eprintln!(
                                    "[loctree][warn] error mutex poisoned; salvaging partial errors"
                                );
                                poisoned.into_inner()
                            }
                        };
                        guard.push((root_path.clone(), e));
                    }
                    Err(_) => {
                        let mut guard = match errors.lock() {
                            Ok(g) => g,
                            Err(poisoned) => {
                                eprintln!(
                                    "[loctree][warn] error mutex poisoned; salvaging partial errors"
                                );
                                poisoned.into_inner()
                            }
                        };
                        guard.push((root_path.clone(), io::Error::other("thread panic")));
                    }
                }
            }
        }
    });

    // Check for errors
    let errors = match errors.into_inner() {
        Ok(v) => v,
        Err(poisoned) => {
            eprintln!(
                "[loctree][warn] error mutex poisoned during unwrap; returning partial errors"
            );
            poisoned.into_inner()
        }
    };
    if !errors.is_empty() {
        let first_err = errors.into_iter().next().unwrap();
        return Err(io::Error::new(
            first_err.1.kind(),
            format!("Error scanning {}: {}", first_err.0.display(), first_err.1),
        ));
    }

    // Merge results from all roots
    let all_results = match results.into_inner() {
        Ok(v) => v,
        Err(poisoned) => {
            eprintln!(
                "[loctree][warn] result mutex poisoned during unwrap; returning partial results"
            );
            poisoned.into_inner()
        }
    };
    let mut contexts: Vec<RootContext> = Vec::new();
    let mut global_fe_commands: CommandUsage = HashMap::new();
    let mut global_be_commands: CommandUsage = HashMap::new();
    let mut global_fe_payloads: PayloadMap = HashMap::new();
    let mut global_be_payloads: PayloadMap = HashMap::new();
    let mut global_analyses: Vec<FileAnalysis> = Vec::new();
    let mut ts_resolver_config: Option<ExtractedResolverConfig> = None;
    let mut all_py_roots: Vec<String> = Vec::new();

    for result in all_results {
        contexts.push(result.context);
        for (name, entries) in result.fe_commands {
            global_fe_commands.entry(name).or_default().extend(entries);
        }
        for (name, entries) in result.be_commands {
            global_be_commands.entry(name).or_default().extend(entries);
        }
        for (name, entries) in result.fe_payloads {
            global_fe_payloads.entry(name).or_default().extend(entries);
        }
        for (name, entries) in result.be_payloads {
            global_be_payloads.entry(name).or_default().extend(entries);
        }
        global_analyses.extend(result.analyses);

        // Use first non-None ts_resolver_config (typically from project root)
        if ts_resolver_config.is_none() && result.ts_resolver_config.is_some() {
            ts_resolver_config = result.ts_resolver_config;
        }
        all_py_roots.extend(result.py_roots);
    }

    // Deduplicate py_roots
    all_py_roots.sort();
    all_py_roots.dedup();

    Ok(ScanResults {
        contexts,
        global_fe_commands,
        global_be_commands,
        global_fe_payloads,
        global_be_payloads,
        global_analyses,
        ts_resolver_config,
        py_roots: all_py_roots,
    })
}

/// Sequential scanning (original implementation, used for single root)
fn scan_roots_sequential(cfg: ScanConfig<'_>) -> io::Result<ScanResults> {
    let mut contexts: Vec<RootContext> = Vec::new();
    let mut global_fe_commands: CommandUsage = HashMap::new();
    let mut global_be_commands: CommandUsage = HashMap::new();
    let mut global_fe_payloads: PayloadMap = HashMap::new();
    let mut global_be_payloads: PayloadMap = HashMap::new();
    let mut global_analyses: Vec<FileAnalysis> = Vec::new();
    let mut ts_resolver_config: Option<ExtractedResolverConfig> = None;
    let mut all_py_roots: Vec<String> = Vec::new();

    for root_path in cfg.roots.iter() {
        let result = scan_single_root(root_path, &cfg)?;

        contexts.push(result.context);
        for (name, entries) in result.fe_commands {
            global_fe_commands.entry(name).or_default().extend(entries);
        }
        for (name, entries) in result.be_commands {
            global_be_commands.entry(name).or_default().extend(entries);
        }
        for (name, entries) in result.fe_payloads {
            global_fe_payloads.entry(name).or_default().extend(entries);
        }
        for (name, entries) in result.be_payloads {
            global_be_payloads.entry(name).or_default().extend(entries);
        }
        global_analyses.extend(result.analyses);

        // Use first non-None ts_resolver_config (typically from project root)
        if ts_resolver_config.is_none() && result.ts_resolver_config.is_some() {
            ts_resolver_config = result.ts_resolver_config;
        }
        all_py_roots.extend(result.py_roots);
    }

    // Deduplicate py_roots
    all_py_roots.sort();
    all_py_roots.dedup();

    Ok(ScanResults {
        contexts,
        global_fe_commands,
        global_be_commands,
        global_fe_payloads,
        global_be_payloads,
        global_analyses,
        ts_resolver_config,
        py_roots: all_py_roots,
    })
}

/// True when `path` is matched by an [`IgnoreMatchers`] set (literal-prefix
/// path OR glob). Mirrors the `.loctignore` half of `fs_utils::should_ignore`,
/// used only to TAG files in `--include-ignored` override mode.
fn path_matches_ignore(matchers: &crate::fs_utils::IgnoreMatchers, path: &std::path::Path) -> bool {
    if matchers
        .ignore_paths
        .iter()
        .any(|ignored| path.starts_with(ignored))
    {
        return true;
    }
    if let Some(globs) = &matchers.ignore_globs
        && globs.is_match(path)
    {
        return true;
    }
    false
}

/// Scan a single root directory and return results
fn scan_single_root(
    root_path: &std::path::Path,
    cfg: &ScanConfig<'_>,
) -> io::Result<SingleRootResult> {
    let ignore_matchers =
        crate::fs_utils::build_ignore_matchers(&cfg.parsed.ignore_patterns, root_path);
    // In `--include-ignored` override mode, `.loctignore` patterns are kept out
    // of `ignore_patterns` (so those files are gathered). We build them into a
    // separate matcher here purely to TAG each gathered file as `ignored`.
    let loctignore_override = if cfg.parsed.include_ignored {
        Some(crate::fs_utils::build_ignore_matchers(
            &cfg.parsed.loctignore_override_patterns,
            root_path,
        ))
    } else {
        None
    };
    let root_canon = root_path
        .canonicalize()
        .unwrap_or_else(|_| root_path.to_path_buf());

    // Ensure core language extensions are always included even if user provided a custom list.
    // The trailing text-resource group (properties/xml/svg/txt) is the W2-02
    // literal-truth guarantee: detect-narrowed universes (e.g. Cargo root →
    // rs+toml) must still index the resources rg matches, or
    // `find --literal/--regex` under-reports on auto-detected repos.
    let extensions = cfg.extensions.clone().map(|mut set| {
        for lang_ext in [
            "go",
            "dart",
            "sh",
            "bash",
            "zsh",
            "fish",
            "mk",
            "make",
            "zig",
            "zon",
            "toml",
            "md",
            "markdown",
            "mdx",
            "json",
            "jsonc",
            "yaml",
            "yml",
            "swift",
            "kt",
            "kts",
            "css",
            "scss",
            "sass",
            "less",
            "html",
            "htm",
            "m",
            "mm",
            "c",
            "cc",
            "cpp",
            "cxx",
            "h",
            "hpp",
            "properties",
            "xml",
            "svg",
            "txt",
        ] {
            set.insert(lang_ext.to_string());
        }
        set
    });

    let options = Options {
        extensions,
        ignore_paths: ignore_matchers.ignore_paths,
        ignore_globs: ignore_matchers.ignore_globs,
        // --scan-all should ignore gitignore and include everything
        use_gitignore: cfg.parsed.use_gitignore && !cfg.parsed.scan_all,
        max_depth: cfg.parsed.max_depth,
        color: cfg.parsed.color,
        output: cfg.parsed.output,
        summary: cfg.parsed.summary,
        summary_limit: cfg.parsed.summary_limit,
        summary_only: cfg.parsed.summary_only,
        show_hidden: cfg.parsed.show_hidden,
        show_ignored: false, // Only used in tree mode
        loc_threshold: cfg.parsed.loc_threshold,
        analyze_limit: cfg.parsed.analyze_limit,
        report_path: cfg.parsed.report_path.clone(),
        serve: cfg.parsed.serve,
        editor_cmd: cfg.parsed.editor_cmd.clone(),
        max_graph_nodes: cfg.parsed.max_graph_nodes,
        max_graph_edges: cfg.parsed.max_graph_edges,
        verbose: cfg.parsed.verbose,
        scan_all: cfg.parsed.scan_all,
        symbol: cfg.parsed.symbol.clone(),
        impact: cfg.parsed.impact.clone(),
        find_artifacts: false, // Only used in tree mode
    };

    if options.verbose {
        eprintln!("[loctree][debug] analyzing root {}", root_path.display());
    }

    let git_checker = if options.use_gitignore {
        GitIgnoreChecker::new(root_path)
    } else {
        None
    };

    let ts_resolver = TsPathResolver::from_tsconfig(&root_canon);
    let py_roots: Vec<PathBuf> = build_py_roots(&root_canon, &cfg.parsed.py_roots);

    // Extract resolver config for caching in snapshot
    let extracted_ts_config = ts_resolver.as_ref().map(|r| r.extract_config());
    let py_roots_strings: Vec<String> = py_roots
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    let mut files = Vec::new();
    let mut visited = HashSet::new();
    gather_files(
        root_path,
        &options,
        0,
        git_checker.as_ref(),
        &mut visited,
        &mut files,
    )?;

    if let (Some(focus), Some(exclude)) = (cfg.focus_set, cfg.exclude_set) {
        let mut overlapping = Vec::new();
        for path in &files {
            let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
            if focus.is_match(&canon) && exclude.is_match(&canon) {
                overlapping.push(canon.display().to_string());
                if overlapping.len() >= 5 {
                    break;
                }
            }
        }
        if !overlapping.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "--focus and --exclude-report overlap on: {}",
                    overlapping.join(", ")
                ),
            ));
        }
    }

    let mut analyses = Vec::new();
    let mut export_index: ExportIndex = HashMap::new();
    let mut reexport_edges: Vec<(String, Option<String>)> = Vec::new();
    let mut dynamic_summary: Vec<(String, Vec<String>)> = Vec::new();
    let mut fe_commands: CommandUsage = HashMap::new();
    let mut be_commands: CommandUsage = HashMap::new();
    let mut fe_payloads: PayloadMap = HashMap::new();
    let mut be_payloads: PayloadMap = HashMap::new();
    let mut graph_edges: Vec<(String, String, String)> = Vec::new();
    let mut loc_map: HashMap<String, usize> = HashMap::new();
    let mut languages: HashSet<String> = HashSet::new();
    let mut barrels: Vec<BarrelInfo> = Vec::new();

    let mut cached_hits = 0usize;
    let mut fresh_scans = 0usize;

    let analyze_ctx = AnalyzeContext {
        root_canon: &root_canon,
        extensions: options.extensions.as_ref(),
        ts_resolver: ts_resolver.as_ref(),
        py_roots: &py_roots,
        py_stdlib: cfg.py_stdlib,
        symbol: options.symbol.as_deref(),
        custom_command_macros: cfg.custom_command_macros,
        command_cfg: &cfg.command_detection,
    };

    for file in files {
        // Get current file mtime and size for incremental scanning
        // Using both mtime + size avoids FP on fast edits (sub-second granularity)
        let metadata = std::fs::metadata(&file).ok();
        let current_mtime = metadata
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let current_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

        // Compute relative path for cache lookup
        let rel_path = file
            .strip_prefix(&root_canon)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| file.to_string_lossy().to_string())
            .replace('\\', "/");

        // Check if we can use cached analysis (mtime + size must both match)
        let mut analysis = if let Some(cache) = cfg.cached_analyses {
            if let Some(cached) = cache.get(&rel_path) {
                let mtime_matches = cached.mtime > 0 && cached.mtime == current_mtime;
                let size_matches = cached.size == current_size;
                if mtime_matches && size_matches {
                    // File unchanged - reuse cached analysis
                    cached_hits += 1;
                    cached.clone()
                } else {
                    // File changed - re-analyze
                    fresh_scans += 1;
                    match analyze_file(&file, &analyze_ctx) {
                        Ok(mut a) => {
                            a.mtime = current_mtime;
                            a.size = current_size;
                            a
                        }
                        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                            // Skip binary files or invalid UTF-8
                            continue;
                        }
                        Err(e) => return Err(e),
                    }
                }
            } else {
                // New file - analyze
                fresh_scans += 1;
                match analyze_file(&file, &analyze_ctx) {
                    Ok(mut a) => {
                        a.mtime = current_mtime;
                        a.size = current_size;
                        a
                    }
                    Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                        // Skip binary files or invalid UTF-8
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
        } else {
            // No cache - fresh scan
            fresh_scans += 1;
            match analyze_file(&file, &analyze_ctx) {
                Ok(mut a) => {
                    a.mtime = current_mtime;
                    a.size = current_size;
                    a
                }
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    // Skip binary files or invalid UTF-8
                    continue;
                }
                Err(e) => return Err(e),
            }
        };
        let abs_for_match = root_canon.join(&analysis.path);
        // Tag `.loctignore`-excluded files surfaced by `--include-ignored`.
        // Always reset (cache reuse must not carry a stale flag): only the
        // override matcher can set it true.
        analysis.ignored = loctignore_override
            .as_ref()
            .map(|m| path_matches_ignore(m, &abs_for_match))
            .unwrap_or(false);
        let is_excluded_for_commands = cfg
            .exclude_set
            .as_ref()
            .map(|set| {
                let canon = abs_for_match
                    .canonicalize()
                    .unwrap_or_else(|_| abs_for_match.clone());
                set.is_match(&canon)
            })
            .unwrap_or(false);

        loc_map.insert(analysis.path.clone(), analysis.loc);
        // Exclude test fixtures from export index to avoid false positive duplicates
        let is_test_fixture = super::classify::should_exclude_from_reports(&analysis.path);
        if !is_test_fixture {
            for exp in &analysis.exports {
                if analysis.language == "make" {
                    continue;
                }
                if analysis.language == "shell" && exp.kind != "function" {
                    continue;
                }
                if exp.kind == "reexport" {
                    continue;
                }
                if exp.export_type == "default" {
                    continue;
                }
                let name_lc = exp.name.to_lowercase();
                let is_decl = [".d.ts", ".d.tsx", ".d.mts", ".d.cts"]
                    .iter()
                    .any(|ext| analysis.path.ends_with(ext));
                if is_decl && name_lc == "default" {
                    continue;
                }
                let ignored = cfg.ignore_exact.contains(&name_lc)
                    || cfg.ignore_prefixes.iter().any(|p| name_lc.starts_with(p));
                if ignored {
                    continue;
                }
                export_index
                    .entry(exp.name.clone())
                    .or_default()
                    .push(analysis.path.clone());
            }
        }
        for re in &analysis.reexports {
            // Re-export edges are useful for cycle detection and barrel awareness.
            // If the parser didn't resolve the target, try to resolve it here using the same
            // resolution logic as imports (relative or tsconfig alias).
            let mut resolved_target = re.resolved.clone();
            if resolved_target.is_none() {
                let spec = &re.source;
                let ext = file
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_lowercase())
                    .unwrap_or_else(|| analysis.language.clone());
                resolved_target = if spec.starts_with('.') {
                    resolve_js_relative(&file, root_path, spec, options.extensions.as_ref())
                } else if matches!(
                    ext.as_str(),
                    "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "svelte" | "vue" | "astro"
                ) {
                    ts_resolver
                        .as_ref()
                        .and_then(|r| r.resolve(spec, options.extensions.as_ref()))
                } else {
                    None
                };
            }

            reexport_edges.push((analysis.path.clone(), resolved_target.clone()));
            let collect_edges = cfg.collect_edges
                || (cfg.parsed.graph && options.report_path.is_some())
                || options.impact.is_some();
            if collect_edges && let Some(target) = &resolved_target {
                graph_edges.push((
                    analysis.path.clone(),
                    target.clone(),
                    "reexport".to_string(),
                ));
            }
        }
        if !analysis.dynamic_imports.is_empty() {
            dynamic_summary.push((analysis.path.clone(), analysis.dynamic_imports.clone()));
        }
        let should_collect_edges = cfg.collect_edges
            || (cfg.parsed.graph && options.report_path.is_some())
            || options.impact.is_some();
        if should_collect_edges {
            let ext = file
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_else(|| analysis.language.clone());
            for imp in &analysis.imports {
                let resolved = imp.resolved_path.clone().or_else(|| match ext.as_str() {
                    "py" => {
                        if imp.source.starts_with('.') {
                            resolve_python_relative(
                                &imp.source,
                                &file,
                                root_path,
                                options.extensions.as_ref(),
                            )
                        } else {
                            resolve_python_absolute(
                                &imp.source,
                                &py_roots,
                                root_path,
                                options.extensions.as_ref(),
                            )
                        }
                    }
                    "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "css" | "svelte" | "vue"
                    | "astro" => {
                        if imp.source.starts_with('.') {
                            resolve_js_relative(
                                &file,
                                root_path,
                                &imp.source,
                                options.extensions.as_ref(),
                            )
                        } else {
                            ts_resolver
                                .as_ref()
                                .and_then(|r| r.resolve(&imp.source, options.extensions.as_ref()))
                        }
                    }
                    "rs" if imp.is_mod_declaration => {
                        resolve_rust_import(&imp.source, &file, root_path, root_path)
                    }
                    _ => None,
                });
                if let Some(target) = resolved {
                    let label = if imp.is_mod_declaration {
                        // Rust `mod foo;` declarations create parent->child module relationships
                        // These are NOT import edges and should not contribute to cycle detection
                        "mod"
                    } else if imp.is_type_checking {
                        "type_import"
                    } else if imp.is_lazy {
                        "lazy_import"
                    } else {
                        match imp.kind {
                            ImportKind::Static | ImportKind::Type | ImportKind::SideEffect => {
                                "import"
                            }
                            ImportKind::Dynamic => "dynamic_import",
                        }
                    };
                    if label == "type_import" {
                        continue;
                    }
                    // Use full paths for edges (slice module needs exact paths)
                    // Note: normalize_module_id strips /index suffix which breaks slice lookups
                    graph_edges.push((analysis.path.clone(), target, label.to_string()));
                }
            }
        }
        // Exclude test fixtures from command analysis to avoid false positives
        let is_test_fixture = super::classify::should_exclude_from_reports(&analysis.path);
        if !is_excluded_for_commands && !is_test_fixture {
            for call in &analysis.command_calls {
                if call.name.contains('.') {
                    // VSCode-style command IDs (e.g. "loctree.analyzeImpact") are not Tauri invokes.
                    continue;
                }
                let mut key = call.name.clone();
                if let Some(stripped) = key.strip_suffix("_command") {
                    key = stripped.to_string();
                } else if let Some(stripped) = key.strip_suffix("_cmd") {
                    key = stripped.to_string();
                }
                fe_commands.entry(key.clone()).or_default().push((
                    analysis.path.clone(),
                    call.line,
                    call.name.clone(),
                ));
                fe_payloads.entry(key).or_default().push((
                    analysis.path.clone(),
                    call.line,
                    call.payload.clone(),
                ));
            }
            for handler in &analysis.command_handlers {
                let mut key = handler
                    .exposed_name
                    .as_ref()
                    .unwrap_or(&handler.name)
                    .clone();
                if let Some(stripped) = key.strip_suffix("_command") {
                    key = stripped.to_string();
                } else if let Some(stripped) = key.strip_suffix("_cmd") {
                    key = stripped.to_string();
                }
                be_payloads.entry(key.clone()).or_default().push((
                    analysis.path.clone(),
                    handler.line,
                    handler.payload.clone(),
                ));
                be_commands.entry(key).or_default().push((
                    analysis.path.clone(),
                    handler.line,
                    handler.name.clone(),
                ));
            }
        }
        languages.insert(analysis.language.clone());
        analyses.push(analysis);
    }

    let mut barrel_map: HashMap<String, BarrelInfo> = HashMap::new();
    for analysis in &analyses {
        if analysis.reexports.is_empty() {
            continue;
        }
        if !is_index_like(&analysis.path) {
            continue;
        }

        let mut targets: Vec<String> = analysis
            .reexports
            .iter()
            .filter_map(|r| r.resolved.clone().or_else(|| Some(r.source.clone())))
            .map(|t| normalize_module_id(&t).as_key())
            .collect();
        targets.sort();
        targets.dedup();

        let module_id = normalize_module_id(&analysis.path).as_key();
        let entry = barrel_map.entry(module_id.clone()).or_insert(BarrelInfo {
            path: analysis.path.clone(),
            module_id,
            reexport_count: 0,
            target_count: 0,
            mixed: false,
            targets: Vec::new(),
        });

        entry.reexport_count += analysis.reexports.len();
        let has_own_defs = analysis.exports.iter().any(|e| e.kind != "reexport");
        entry.mixed |= has_own_defs;
        entry.targets.extend(targets);
    }
    for (_, mut info) in barrel_map {
        info.targets.sort();
        info.targets.dedup();
        info.target_count = info.targets.len();
        barrels.push(info);
    }
    barrels.sort_by(|a, b| a.path.cmp(&b.path));

    let duplicate_exports: Vec<_> = export_index
        .iter()
        .filter(|(_, files)| files.len() > 1)
        .map(|(name, files)| (name.clone(), files.clone()))
        .collect();

    let reexport_files: HashSet<String> = analyses
        .iter()
        .filter(|a| !a.reexports.is_empty())
        .map(|a| a.path.clone())
        .collect();

    resolve_event_constants_across_files(&mut analyses);

    // Establish implicit graph edges for languages like Swift that rely on global namespace symbols.
    if cfg.collect_edges
        || (cfg.parsed.graph && options.report_path.is_some())
        || options.impact.is_some()
    {
        let implicit_symbol_exports: HashSet<(String, String)> = analyses
            .iter()
            .flat_map(|analysis| {
                analysis
                    .exports
                    .iter()
                    .filter(|export| is_implicit_symbol_export(export))
                    .map(|export| (export.name.clone(), analysis.path.clone()))
            })
            .collect();
        let mut implicit_edges = HashSet::new();
        for analysis in &analyses {
            if analysis.language == "swift" {
                for usage in &analysis.symbol_usages {
                    if let Some(target_files) = export_index.get(&usage.name) {
                        for target in target_files {
                            if target != &analysis.path
                                && implicit_symbol_exports
                                    .contains(&(usage.name.clone(), target.clone()))
                                && implicit_edges.insert((analysis.path.clone(), target.clone()))
                            {
                                graph_edges.push((
                                    analysis.path.clone(),
                                    target.clone(),
                                    "implicit_symbol".to_string(),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    let mut cascades = Vec::new();
    for (from, resolved) in &reexport_edges {
        if let Some(target) = resolved
            && reexport_files.contains(target)
        {
            cascades.push((from.clone(), target.clone()));
        }
    }

    // Build lookup map for export line numbers: (file_path, export_name) -> line
    let export_lines: std::collections::HashMap<(String, String), Option<usize>> = analyses
        .iter()
        .flat_map(|a| {
            a.exports
                .iter()
                .map(|e| ((a.path.clone(), e.name.clone()), e.line))
        })
        .collect();

    let mut ranked_dups: Vec<RankedDup> = Vec::new();
    for (name, files) in &duplicate_exports {
        let files = dedupe_file_list(files);
        if files.len() <= 1 {
            continue;
        }

        // Skip generic method names that are expected to be duplicated across different types
        if is_generic_method(name) {
            continue;
        }

        let all_rs = files.iter().all(|f| f.ends_with(".rs"));
        let all_d_ts = files.iter().all(|f| {
            f.ends_with(".d.ts")
                || f.ends_with(".d.tsx")
                || f.ends_with(".d.mts")
                || f.ends_with(".d.cts")
        });
        // Keep the old filter for backward compatibility (though now redundant)
        if (name == "new" && all_rs) || (name == "default" && all_d_ts) {
            continue;
        }
        let dev_count = files.iter().filter(|f| is_dev_file(f)).count();
        let prod_count = files.len().saturating_sub(dev_count);
        let score = prod_count * 2 + dev_count;
        let canonical = files
            .iter()
            .find(|f| !is_dev_file(f))
            .cloned()
            .unwrap_or_else(|| files[0].clone());
        let canonical_line = export_lines
            .get(&(canonical.clone(), name.clone()))
            .copied()
            .flatten();
        let locations: Vec<DupLocation> = files
            .iter()
            .map(|f| DupLocation {
                file: f.clone(),
                line: export_lines
                    .get(&(f.clone(), name.clone()))
                    .copied()
                    .flatten(),
            })
            .collect();
        let mut refactors: Vec<String> =
            files.iter().filter(|f| *f != &canonical).cloned().collect();
        refactors.sort();

        // Extract packages for severity
        let packages: Vec<String> = files
            .iter()
            .map(|f| extract_package(f))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let cross_lang = is_cross_lang(&files);
        let (severity, reason) = compute_severity(name, &files, &packages);

        ranked_dups.push(RankedDup {
            name: name.clone(),
            files: files.clone(),
            locations,
            score,
            prod_count,
            dev_count,
            canonical,
            canonical_line,
            refactors,
            severity,
            is_cross_lang: cross_lang,
            packages,
            reason,
        });
    }
    ranked_dups.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(b.files.len().cmp(&a.files.len()))
    });

    let mut filtered_ranked: Vec<RankedDup> = Vec::new();
    for dup in ranked_dups.into_iter() {
        let kept_files = strip_excluded(&dup.files, cfg.exclude_set);
        if kept_files.len() <= 1 {
            continue;
        }
        if !matches_focus(&kept_files, cfg.focus_set) {
            continue;
        }
        let canonical = if kept_files.contains(&dup.canonical) {
            dup.canonical.clone()
        } else {
            kept_files
                .iter()
                .find(|f| !is_dev_file(f))
                .cloned()
                .unwrap_or_else(|| kept_files[0].clone())
        };
        let dev_count = kept_files.iter().filter(|f| is_dev_file(f)).count();
        let prod_count = kept_files.len().saturating_sub(dev_count);
        let score = prod_count * 2 + dev_count;
        // Filter locations to only kept files
        let locations: Vec<DupLocation> = dup
            .locations
            .into_iter()
            .filter(|loc| kept_files.contains(&loc.file))
            .collect();
        let canonical_line = locations
            .iter()
            .find(|loc| loc.file == canonical)
            .and_then(|loc| loc.line);
        let mut refactors: Vec<String> = kept_files
            .iter()
            .filter(|f| *f != &canonical)
            .cloned()
            .collect();
        refactors.sort();

        // Recompute severity for filtered files
        let packages: Vec<String> = kept_files
            .iter()
            .map(|f| extract_package(f))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let cross_lang = is_cross_lang(&kept_files);
        let (severity, reason) = compute_severity(&dup.name, &kept_files, &packages);

        filtered_ranked.push(RankedDup {
            name: dup.name,
            files: kept_files,
            locations,
            score,
            prod_count,
            dev_count,
            canonical,
            canonical_line,
            refactors,
            severity,
            is_cross_lang: cross_lang,
            packages,
            reason,
        });
    }
    // Sort by severity (descending), then by score
    filtered_ranked.sort_by(|a, b| {
        let sev_cmp = (b.severity as u8).cmp(&(a.severity as u8));
        if sev_cmp != std::cmp::Ordering::Equal {
            sev_cmp
        } else {
            b.score
                .cmp(&a.score)
                .then(b.files.len().cmp(&a.files.len()))
        }
    });

    let tsconfig_summary = super::tsconfig::summarize_tsconfig(root_path, &analyses);

    // Log incremental scan stats if verbose
    if options.verbose && cfg.cached_analyses.is_some() {
        let total = cached_hits + fresh_scans;
        eprintln!(
            "[loctree][incremental] {} cached, {} fresh ({} total files)",
            cached_hits, fresh_scans, total
        );
    }

    let mut calls_with_generics = Vec::new();
    for analysis in &analyses {
        for call in &analysis.command_calls {
            if let Some(gt) = &call.generic_type {
                calls_with_generics.push(json!({
                    "name": call.name,
                    "path": analysis.path,
                    "line": call.line,
                    "genericType": gt,
                }));
            }
        }
    }

    let mut renamed_handlers = Vec::new();
    for analysis in &analyses {
        for handler in &analysis.command_handlers {
            if let Some(exposed) = &handler.exposed_name
                && exposed != &handler.name
            {
                renamed_handlers.push(json!({
                    "path": analysis.path,
                    "line": handler.line,
                    "name": handler.name,
                    "exposedName": exposed,
                }));
            }
        }
    }

    let context = RootContext {
        root_path: root_path.to_path_buf(),
        options,
        analyses: analyses.clone(),
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
    };

    Ok(SingleRootResult {
        context,
        fe_commands,
        be_commands,
        fe_payloads,
        be_payloads,
        analyses,
        ts_resolver_config: extracted_ts_config,
        py_roots: py_roots_strings,
    })
}

fn is_index_like(path: &str) -> bool {
    let lowered = path.to_lowercase();
    lowered.ends_with("/index.ts")
        || lowered.ends_with("/index.tsx")
        || lowered.ends_with("/index.js")
        || lowered.ends_with("/index.jsx")
        || lowered.ends_with("/index.mjs")
        || lowered.ends_with("/index.cjs")
        || lowered.ends_with("/index.rs")
        || lowered.ends_with("/index.svelte")
        || lowered.ends_with("/index.vue")
        || lowered.ends_with("/index.astro")
}

/// Normalized module identifier with language context
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NormalizedModule {
    /// Path without extension and /index suffix
    pub path: String,
    /// Language/extension identifier (ts, tsx, js, jsx, mjs, cjs, rs, py, css)
    pub lang: String,
}

impl NormalizedModule {
    /// Format as string for use as map key: "{path}:{lang}"
    pub fn as_key(&self) -> String {
        format!("{}:{}", self.path, self.lang)
    }

    /// Parse from key string created by as_key()
    pub fn from_key(key: &str) -> Option<Self> {
        let parts: Vec<&str> = key.rsplitn(2, ':').collect();
        if parts.len() == 2 {
            Some(NormalizedModule {
                path: parts[1].to_string(),
                lang: parts[0].to_string(),
            })
        } else {
            None
        }
    }
}

/// Normalize module identifier preserving language context
///
/// - TS/JS family (`ts`, `tsx`, `js`, `jsx`, `mjs`, `cjs`) collapses to `ts`
///   to reduce FP noise across extensions and barrels.
/// - Cross-language collisions (e.g., `.rs` vs `.ts`) are still prevented.
/// - `/index` suffix is stripped so `foo/index.ts` -> `foo:ts`.
///
/// Returns a NormalizedModule with separate path and language fields.
pub(crate) fn normalize_module_id(path: &str) -> NormalizedModule {
    let mut p = path.replace('\\', "/");
    let mut lang = String::new();

    // Extract language family from extension (collapse TS/JS variants)
    for ext in [
        ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".rs", ".py", ".css", ".svelte", ".vue",
        ".astro",
    ] {
        if let Some(stripped) = p.strip_suffix(ext) {
            p = stripped.to_string();
            lang = match ext {
                ".ts" | ".tsx" | ".js" | ".jsx" | ".mjs" | ".cjs" | ".svelte" | ".vue"
                | ".astro" => "ts".to_string(),
                ".rs" => "rs".to_string(),
                ".py" => "py".to_string(),
                ".css" => "css".to_string(),
                _ => ext.trim_start_matches('.').to_string(),
            };
            break;
        }
    }

    // Strip /index suffix
    if let Some(stripped) = p.strip_suffix("/index") {
        p = stripped.to_string();
    }

    NormalizedModule { path: p, lang }
}

/// Build ScanResults from a loaded Snapshot (scan once, analyze many)
pub fn scan_results_from_snapshot(snapshot: &Snapshot) -> ScanResults {
    let mut global_fe_commands: CommandUsage = HashMap::new();
    let mut global_be_commands: CommandUsage = HashMap::new();
    let mut global_fe_payloads: PayloadMap = HashMap::new();
    let mut global_be_payloads: PayloadMap = HashMap::new();

    // Build command maps from FileAnalysis
    for analysis in &snapshot.files {
        // Frontend commands (invoke calls)
        for call in &analysis.command_calls {
            let mut key = call.name.clone();
            if let Some(stripped) = key.strip_suffix("_command") {
                key = stripped.to_string();
            }
            global_fe_commands.entry(key.clone()).or_default().push((
                analysis.path.clone(),
                call.line,
                call.name.clone(),
            ));

            // Track payload types if present
            if let Some(payload) = &call.payload {
                global_fe_payloads.entry(key).or_default().push((
                    analysis.path.clone(),
                    call.line,
                    Some(payload.clone()),
                ));
            }
        }

        // Backend handlers (#[tauri::command])
        for handler in &analysis.command_handlers {
            let mut key = handler
                .exposed_name
                .as_ref()
                .unwrap_or(&handler.name)
                .clone();
            if let Some(stripped) = key.strip_suffix("_command") {
                key = stripped.to_string();
            }
            global_be_commands.entry(key.clone()).or_default().push((
                analysis.path.clone(),
                handler.line,
                handler.name.clone(),
            ));

            // Track return types if present
            if let Some(ret) = &handler.payload {
                global_be_payloads.entry(key).or_default().push((
                    analysis.path.clone(),
                    handler.line,
                    Some(ret.clone()),
                ));
            }
        }
    }

    // Build minimal RootContext for each root in snapshot
    let mut contexts = Vec::new();

    // If only one root, all files belong to it (paths are relative in snapshot)
    let single_root = snapshot.metadata.roots.len() == 1;

    for root_str in &snapshot.metadata.roots {
        let root_path = PathBuf::from(root_str);

        // Filter analyses for this root
        // Note: snapshot paths are relative, so for single root we take all files
        let root_analyses: Vec<FileAnalysis> = if single_root {
            snapshot.files.clone()
        } else {
            snapshot
                .files
                .iter()
                .filter(|a| a.path.starts_with(root_str) || root_str == ".")
                .cloned()
                .collect()
        };

        // Build export index
        let mut export_index: ExportIndex = HashMap::new();
        for analysis in &root_analyses {
            for export in &analysis.exports {
                // Skip re-exports - they're not duplicate definitions
                if export.kind == "reexport" {
                    continue;
                }
                export_index
                    .entry(export.name.clone())
                    .or_default()
                    .push(analysis.path.clone());
            }
        }

        // Build LOC map
        let loc_map: HashMap<String, usize> = root_analyses
            .iter()
            .map(|a| (a.path.clone(), a.loc))
            .collect();

        // Build dynamic summary
        let dynamic_summary: Vec<(String, Vec<String>)> = root_analyses
            .iter()
            .filter(|a| !a.dynamic_imports.is_empty())
            .map(|a| (a.path.clone(), a.dynamic_imports.clone()))
            .collect();

        // Build graph edges from snapshot
        // Note: For single root, all edges belong to it (paths in snapshot are relative)
        let graph_edges: Vec<(String, String, String)> = if single_root {
            snapshot
                .edges
                .iter()
                .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
                .collect()
        } else {
            snapshot
                .edges
                .iter()
                .filter(|e| e.from.starts_with(root_str) || root_str == ".")
                .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
                .collect()
        };

        // Detect languages
        let languages: HashSet<String> = root_analyses.iter().map(|a| a.language.clone()).collect();

        // Calculate ranked duplicates
        let ignore_exact: HashSet<String> = twins::GENERIC_METHOD_NAMES
            .iter()
            .map(|s| s.to_string())
            .collect();
        let filtered_ranked = rank_duplicates(&export_index, &root_analyses, &ignore_exact, &[]);

        // Build cascades (re-export chains)
        let cascades = build_cascades(&root_analyses);

        // Build barrel info
        let barrels: Vec<BarrelInfo> = snapshot
            .barrels
            .iter()
            .filter(|b| b.path.starts_with(root_str) || root_str == ".")
            .map(|b| BarrelInfo {
                path: b.path.clone(),
                module_id: b.module_id.clone(),
                reexport_count: b.reexport_count,
                target_count: b.targets.len(),
                mixed: false, // Can't determine from snapshot
                targets: b.targets.clone(),
            })
            .collect();

        contexts.push(RootContext {
            root_path,
            options: Options {
                extensions: None,
                ignore_paths: vec![],
                ignore_globs: None,
                use_gitignore: false,
                max_depth: None,
                color: ColorMode::Auto,
                output: OutputMode::Human,
                summary: false,
                summary_limit: 5,
                summary_only: false,
                show_hidden: false,
                show_ignored: false,
                loc_threshold: 1000,
                analyze_limit: 0,
                report_path: None,
                serve: false,
                editor_cmd: None,
                max_graph_nodes: None,
                max_graph_edges: None,
                verbose: false,
                scan_all: false,
                symbol: None,
                impact: None,
                find_artifacts: false,
            },
            analyses: root_analyses,
            export_index,
            dynamic_summary,
            cascades,
            filtered_ranked,
            graph_edges,
            loc_map,
            languages,
            tsconfig_summary: json!({}),
            calls_with_generics: vec![],
            renamed_handlers: vec![],
            barrels,
        });
    }

    // Extract cached resolver config from snapshot metadata
    let ts_resolver_config =
        snapshot
            .metadata
            .resolver_config
            .as_ref()
            .map(|rc| ExtractedResolverConfig {
                ts_paths: rc.ts_paths.clone(),
                ts_base_url: rc.ts_base_url.clone(),
            });

    let py_roots = snapshot
        .metadata
        .resolver_config
        .as_ref()
        .map(|rc| rc.py_roots.clone())
        .unwrap_or_default();

    ScanResults {
        contexts,
        global_fe_commands,
        global_be_commands,
        global_fe_payloads,
        global_be_payloads,
        global_analyses: snapshot.files.clone(),
        ts_resolver_config,
        py_roots,
    }
}

// ============================================================================
// Duplicate severity detection helpers
// ============================================================================

/// Known DTO type names that are expected to be duplicated across Rust↔TS
const DTO_WHITELIST: &[&str] = &[
    "visit",
    "task",
    "chatmessage",
    "attachment",
    "patient",
    "message",
    "user",
    "config",
    "settings",
    "response",
    "request",
    "error",
    "result",
    "event",
    "state",
    "status",
    "payload",
    "data",
];

/// System constants expected across languages
const SYSTEM_CONSTANTS: &[&str] = &["touch_id", "keychain", "app_name", "version", "default"];

/// Generic method/function names that should be excluded from duplicate detection.
/// These are common patterns across different types/structs and not actual duplicates.
const GENERIC_METHOD_NAMES: &[&str] = &[
    "new",     // Constructor pattern (Rust, Python __new__, etc.)
    "default", // Default trait implementation
    "from",    // From trait / factory pattern
    "into",    // Into trait / conversion
    "clone",   // Clone trait
    "process", // Too generic - processing logic
    "load",    // Too generic - loading data
    "save",    // Too generic - saving data
    "run",     // Too generic - execution
    "init",    // Too generic - initialization
    "build",   // Builder pattern
    "execute", // Too generic - execution
    "handle",  // Too generic - event handling
    "create",  // Too generic - factory pattern
    "update",  // Too generic - mutation
    "delete",  // Too generic - removal
    "get",     // Too generic - getter
    "set",     // Too generic - setter
];

/// Check if a name is a generic method that should be excluded from duplicate detection
fn is_generic_method(name: &str) -> bool {
    GENERIC_METHOD_NAMES.contains(&name)
}

/// Detect language family from file path
fn detect_language(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" => "rust",
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "mts" | "astro" => "typescript",
        "py" | "pyi" => "python",
        "css" | "scss" | "less" => "css",
        _ => "unknown",
    }
}

/// Extract package/directory context from path
fn extract_package(path: &str) -> String {
    // Extract first meaningful directory after src/ or src-tauri/
    let parts: Vec<&str> = path.split('/').collect();

    // Find src or src-tauri, take next directory
    for (i, part) in parts.iter().enumerate() {
        if (*part == "src" || *part == "src-tauri") && i + 1 < parts.len() {
            return parts[i + 1].to_string();
        }
    }

    // Fallback: use parent directory
    parts
        .iter()
        .rev()
        .nth(1)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "root".to_string())
}

/// Extract crate/npm package root from path
/// Returns the crate name or package directory (e.g., "rmcp_mux", "rmcp_memex", "loctree-rs")
fn extract_crate_root(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();

    // For monorepo structures like:
    // - "rmcp_mux/src/..." -> "rmcp_mux"
    // - "loctree-rs/src/..." -> "loctree-rs"
    // - "packages/frontend/src/..." -> "packages/frontend"

    // Take first meaningful directory component
    if parts.is_empty() {
        return "root".to_string();
    }

    // Skip leading . or empty parts
    for part in &parts {
        if !part.is_empty() && *part != "." && *part != ".." {
            return part.to_string();
        }
    }

    "root".to_string()
}

fn is_implicit_symbol_export(export: &crate::types::ExportSymbol) -> bool {
    matches!(
        export.kind.as_str(),
        "class" | "struct" | "enum" | "trait" | "type" | "union" | "interface"
    ) && export
        .name
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
}

/// Check if symbol name matches DTO whitelist
fn is_whitelisted_dto(name: &str) -> bool {
    let lc = name.to_lowercase();
    DTO_WHITELIST.iter().any(|dto| lc.contains(dto))
        || SYSTEM_CONSTANTS.iter().any(|c| lc.contains(c))
}

/// Check if files span multiple languages
fn is_cross_lang(files: &[String]) -> bool {
    let langs: HashSet<_> = files.iter().map(|f| detect_language(f)).collect();
    langs.len() > 1
}

/// Check if path is in src-tauri (backend)
fn is_backend_path(path: &str) -> bool {
    path.contains("src-tauri") || path.contains("src-rs") || path.ends_with(".rs")
}

/// Config file patterns that export "default" or common utilities
const CONFIG_PATTERNS: &[&str] = &[
    "eslint",
    "vite.config",
    "vitest.config",
    "playwright.config",
    "postcss.config",
    "tailwind.config",
    "jest.config",
    "webpack.config",
    "rollup.config",
    "babel.config",
    "tsconfig",
    ".d.ts",
];

/// Check if file is a config file (expected to have default exports)
fn is_config_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    CONFIG_PATTERNS.iter().any(|p| lower.contains(p))
}

/// Compute severity for a duplicate export
fn compute_severity(name: &str, files: &[String], packages: &[String]) -> (DupSeverity, String) {
    let cross_lang = is_cross_lang(files);
    let whitelisted = is_whitelisted_dto(name);
    let is_generic = is_generic_method(name);

    if is_cli_or_script_duplicate_idiom(name, files) {
        return (
            DupSeverity::ReExportOrGeneric,
            format!("CLI/script runtime idiom: {}", name),
        );
    }

    if is_shell_or_make_duplicate_idiom(name, files) {
        return (
            DupSeverity::ReExportOrGeneric,
            format!("Shell/Make runtime idiom: {}", name),
        );
    }

    // Count config files
    let config_count = files.iter().filter(|f| is_config_file(f)).count();
    let all_config = config_count == files.len();
    let mostly_config = files.len() > 1 && config_count as f64 / files.len() as f64 > 0.5;

    // Severity 0: Config file exports (default, new, etc.)
    // These are expected to be duplicated across config files
    if all_config || (mostly_config && (name == "default" || is_generic)) {
        return (
            DupSeverity::CrossLangExpected,
            "Config file export".to_string(),
        );
    }

    // Severity 1: Generic methods (new, from, clone, etc.) - usually OK
    // These are common patterns and not real duplicates
    if is_generic {
        return (
            DupSeverity::ReExportOrGeneric,
            format!("Generic method/pattern: {}", name),
        );
    }

    // Check for src-tauri vs src split (expected for Tauri apps)
    let has_backend = files.iter().any(|f| is_backend_path(f));
    let has_frontend = files.iter().any(|f| !is_backend_path(f));
    let fe_be_split = has_backend && has_frontend;

    // Severity 0: Cross-language expected duplicates
    if cross_lang && (whitelisted || fe_be_split) {
        return (
            DupSeverity::CrossLangExpected,
            format!(
                "Cross-lang DTO ({})",
                if whitelisted {
                    "whitelisted"
                } else {
                    "FE↔BE"
                }
            ),
        );
    }

    // HIGHEST SEVERITY 4: Cross-crate duplicates (REAL ISSUES!)
    // Same symbol name in DIFFERENT crates/packages
    // Example: HostKind enum defined in both rmcp_mux and rmcp_memex
    let crate_roots: HashSet<String> = files.iter().map(|f| extract_crate_root(f)).collect();
    let cross_crate = crate_roots.len() > 1;

    if cross_crate {
        let crate_list: Vec<String> = crate_roots.iter().cloned().collect();
        return (
            DupSeverity::CrossCrate,
            format!("Cross-crate duplicate in: {}", crate_list.join(", ")),
        );
    }

    // Severity 3: Cross-module duplicates (same crate, different modules)
    // e.g., Attachment in icons/ vs hooks/
    if packages.len() >= 3 {
        return (
            DupSeverity::CrossModule,
            format!("Symbol in {} different modules", packages.len()),
        );
    }

    // Check for potentially conflicting contexts
    let contexts: Vec<_> = packages.iter().map(|p| p.to_lowercase()).collect();
    let has_ui = contexts
        .iter()
        .any(|c| c.contains("icon") || c.contains("component") || c.contains("ui"));
    let has_logic = contexts.iter().any(|c| {
        c.contains("hook")
            || c.contains("service")
            || c.contains("util")
            || c.contains("context")
            || c.contains("store")
    });
    if has_ui && has_logic && !whitelisted {
        return (
            DupSeverity::CrossModule,
            "UI vs logic context mismatch".to_string(),
        );
    }

    // Severity 1: Same-package or expected structure
    (
        DupSeverity::SamePackage,
        if cross_lang {
            "Cross-lang (not whitelisted)".to_string()
        } else {
            "Same language duplicate".to_string()
        },
    )
}

fn is_shell_or_make_duplicate_idiom(name: &str, files: &[String]) -> bool {
    const SHELL_MAKE_IDIOMS: &[&str] = &[
        ".PHONY", "PATH", "usage", "die", "cleanup", "info", "warn", "error", "debug", "help",
        "install", "build", "test", "clean",
    ];

    if !SHELL_MAKE_IDIOMS.contains(&name) {
        return false;
    }

    files.iter().all(|file| {
        let lower = file.to_ascii_lowercase();
        lower == "makefile"
            || lower.ends_with("/makefile")
            || lower.ends_with(".mk")
            || lower.ends_with(".make")
            || lower.ends_with(".sh")
            || lower.ends_with(".bash")
            || lower.ends_with(".zsh")
            || lower.ends_with(".fish")
    })
}

fn is_cli_or_script_duplicate_idiom(name: &str, files: &[String]) -> bool {
    const CLI_SCRIPT_IDIOMS: &[&str] = &[
        "main", "usage", "die", "cleanup", "info", "warn", "error", "debug", "help",
    ];

    CLI_SCRIPT_IDIOMS.contains(&name) && files.iter().all(|file| is_script_like_path(file))
}

fn is_script_like_path(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    lower == "install.sh"
        || lower.ends_with("/install.sh")
        || lower.starts_with("scripts/")
        || lower.contains("/scripts/")
        || lower.starts_with("tools/")
        || lower.starts_with("skills/")
        || lower.ends_with(".sh")
        || lower.ends_with(".bash")
        || lower.ends_with(".zsh")
        || lower.ends_with(".fish")
        || lower.ends_with(".py")
        || lower.ends_with(".js")
        || lower.ends_with(".mjs")
}

fn dedupe_file_list(files: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    files
        .iter()
        .filter(|file| seen.insert((*file).clone()))
        .cloned()
        .collect()
}

/// Rank duplicates by severity (helper for snapshot conversion)
fn rank_duplicates(
    export_index: &ExportIndex,
    analyses: &[FileAnalysis],
    ignore_exact: &HashSet<String>,
    ignore_prefixes: &[String],
) -> Vec<RankedDup> {
    use super::classify::is_dev_file;

    // Build lookup map for export line numbers
    let export_lines: std::collections::HashMap<(String, String), Option<usize>> = analyses
        .iter()
        .flat_map(|a| {
            a.exports
                .iter()
                .map(|e| ((a.path.clone(), e.name.clone()), e.line))
        })
        .collect();

    let mut ranked = Vec::new();
    for (name, files) in export_index {
        let files = dedupe_file_list(files);
        if files.len() <= 1 {
            continue;
        }
        let lc = name.to_lowercase();
        if ignore_exact.contains(&lc) {
            continue;
        }
        if ignore_prefixes.iter().any(|p| lc.starts_with(p)) {
            continue;
        }

        let mut prod_count = 0;
        let mut dev_count = 0;
        for f in &files {
            if is_dev_file(f) {
                dev_count += 1;
            } else {
                prod_count += 1;
            }
        }

        // Skip if all are dev files
        if prod_count == 0 {
            continue;
        }

        let score = files.len() + prod_count;
        let canonical = files.first().cloned().unwrap_or_default();
        let canonical_line = export_lines
            .get(&(canonical.clone(), name.clone()))
            .copied()
            .flatten();

        // Build locations with line numbers
        let locations: Vec<DupLocation> = files
            .iter()
            .map(|f| DupLocation {
                file: f.clone(),
                line: export_lines
                    .get(&(f.clone(), name.clone()))
                    .copied()
                    .flatten(),
            })
            .collect();

        // Refactor targets = all files except canonical
        let refactors: Vec<String> = files.iter().filter(|f| *f != &canonical).cloned().collect();

        // Extract unique packages for severity calculation
        let packages: Vec<String> = files
            .iter()
            .map(|f| extract_package(f))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        // Compute severity and reason
        let cross_lang = is_cross_lang(&files);
        let (severity, reason) = compute_severity(name, &files, &packages);

        ranked.push(RankedDup {
            name: name.clone(),
            score,
            files: files.clone(),
            locations,
            prod_count,
            dev_count,
            canonical,
            canonical_line,
            refactors,
            severity,
            is_cross_lang: cross_lang,
            packages,
            reason,
        });
    }

    // Sort by severity (descending), then by score
    ranked.sort_by(|a, b| {
        let sev_cmp = (b.severity as u8).cmp(&(a.severity as u8));
        if sev_cmp != std::cmp::Ordering::Equal {
            sev_cmp
        } else {
            b.score.cmp(&a.score)
        }
    });
    ranked
}

pub(crate) fn build_py_roots(root_canon: &Path, extra_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    // Always include project root
    let root_canon = root_canon
        .canonicalize()
        .unwrap_or_else(|_| root_canon.to_path_buf());
    roots.push(root_canon.clone());

    // Common src/ layout
    let src = root_canon.join("src");
    if src.is_dir() {
        roots.push(src.canonicalize().unwrap_or(src));
    }

    // Common nested service layout, e.g. repo/api-router/app where imports use
    // `app.*` from files under the service directory.
    if let Ok(entries) = std::fs::read_dir(&root_canon) {
        for entry in entries.flatten() {
            let service_root = entry.path();
            if !service_root.is_dir() {
                continue;
            }
            if service_root.join("pyproject.toml").exists()
                || service_root.join("setup.py").exists()
                || service_root.join("setup.cfg").exists()
                || service_root.join("app/__init__.py").exists()
            {
                roots.push(service_root.canonicalize().unwrap_or(service_root));
            }
        }
    }

    // Discover from pyproject.toml (poetry/setuptools)
    let pyproject = root_canon.join("pyproject.toml");
    if pyproject.exists()
        && let Ok(text) = std::fs::read_to_string(&pyproject)
        && let Ok(val) = text.parse::<toml::Table>()
    {
        // tool.poetry.packages = [{ include = "...", from = "src" }]
        if let Some(packages) = val
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("packages"))
            .and_then(|p| p.as_array())
        {
            for pkg in packages {
                if let Some(include) = pkg.get("include").and_then(|i| i.as_str()) {
                    let from = pkg.get("from").and_then(|f| f.as_str());
                    let base = from
                        .map(|f| root_canon.join(f))
                        .unwrap_or_else(|| root_canon.clone());
                    let candidate = base.join(include);
                    if candidate.exists() {
                        roots.push(candidate.canonicalize().unwrap_or(candidate));
                    }
                }
            }
        }

        // tool.setuptools.packages.find.where = ["src", ...]
        if let Some(where_arr) = val
            .get("tool")
            .and_then(|t| t.get("setuptools"))
            .and_then(|s| s.get("packages"))
            .and_then(|p| p.get("find"))
            .and_then(|f| f.get("where"))
            .and_then(|w| w.as_array())
        {
            for entry in where_arr {
                if let Some(path_str) = entry.as_str() {
                    let candidate = root_canon.join(path_str);
                    if candidate.exists() {
                        roots.push(candidate.canonicalize().unwrap_or(candidate));
                    }
                }
            }
        }
    }

    // User-provided overrides
    for extra in extra_roots {
        let candidate = if extra.is_absolute() {
            extra.clone()
        } else {
            root_canon.join(extra)
        };
        if candidate.exists() {
            roots.push(candidate.canonicalize().unwrap_or(candidate));
        } else {
            eprintln!(
                "[loctree][warn] --py-root '{}' not found under {}; skipping",
                extra.display(),
                root_canon.display()
            );
        }
    }

    roots.sort();
    roots.dedup();
    roots
}

/// Build cascades (re-export chains) from analyses
fn build_cascades(analyses: &[FileAnalysis]) -> Vec<(String, String)> {
    let mut cascades = Vec::new();
    for analysis in analyses {
        for reexport in &analysis.reexports {
            if !reexport.source.is_empty() {
                cascades.push((analysis.path.clone(), reexport.source.clone()));
            }
        }
    }
    cascades
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_normalize_module_id_preserves_language() {
        // Test that different languages with same base path get different normalized IDs (except TS/JS family)
        let rust_module = normalize_module_id("src/utils.rs");
        let ts_module = normalize_module_id("src/utils.ts");
        let tsx_module = normalize_module_id("src/utils.tsx");

        assert_eq!(rust_module.path, "src/utils");
        assert_eq!(rust_module.lang, "rs");

        assert_eq!(ts_module.path, "src/utils");
        assert_eq!(ts_module.lang, "ts");

        assert_eq!(tsx_module.path, "src/utils");
        assert_eq!(tsx_module.lang, "ts");

        // Keys should be different across languages, but TS/JS family collapses
        assert_ne!(rust_module.as_key(), ts_module.as_key());
        assert_ne!(rust_module.as_key(), tsx_module.as_key());
        assert_eq!(ts_module.as_key(), tsx_module.as_key());
    }

    #[test]
    fn detects_common_python_roots_and_pyproject() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let root_canon = root.canonicalize().unwrap();

        // src layout
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        // setuptools style
        std::fs::create_dir_all(root.join("services")).unwrap();
        // poetry package from "src"
        let pyproject = r#"
[tool.poetry]
name = "example"
version = "0.1.0"
packages = [
    { include = "app", from = "src" }
]

[tool.setuptools.packages.find]
where = ["services"]
"#;
        std::fs::write(root.join("pyproject.toml"), pyproject).unwrap();
        // extra user provided
        let extra_dir = root.join("custom");
        std::fs::create_dir_all(&extra_dir).unwrap();
        let extra_dir_canon = extra_dir.canonicalize().unwrap();

        let roots = build_py_roots(root, &[PathBuf::from("custom"), PathBuf::from("missing")]);

        assert!(roots.contains(&root_canon));
        assert!(roots.contains(&root_canon.join("src")));
        assert!(roots.contains(&root_canon.join("src/app")));
        assert!(roots.contains(&root_canon.join("services")));
        assert!(roots.contains(&extra_dir_canon));
    }

    #[test]
    fn detects_nested_python_service_roots() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let root_canon = root.canonicalize().unwrap();

        std::fs::create_dir_all(root.join("api-router/app")).unwrap();
        std::fs::write(root.join("api-router/app/__init__.py"), "").unwrap();

        let roots = build_py_roots(root, &[]);

        assert!(roots.contains(&root_canon));
        assert!(roots.contains(&root_canon.join("api-router")));
    }

    #[test]
    fn test_normalize_module_id_index_files() {
        // Test that index files are normalized but preserve language
        let ts_index = normalize_module_id("src/components/index.ts");
        let rs_index = normalize_module_id("src/components/index.rs");

        assert_eq!(ts_index.path, "src/components");
        assert_eq!(ts_index.lang, "ts");

        assert_eq!(rs_index.path, "src/components");
        assert_eq!(rs_index.lang, "rs");

        // Should not collide
        assert_ne!(ts_index.as_key(), rs_index.as_key());
        assert_eq!(ts_index.as_key(), "src/components:ts");
        assert_eq!(rs_index.as_key(), "src/components:rs");
    }

    #[test]
    fn test_normalize_module_id_cross_language_collision() {
        // This is the core bug fix - ensure foo.rs and foo.ts don't collide
        let rust_file = normalize_module_id("src/foo.rs");
        let ts_file = normalize_module_id("src/foo.ts");
        let ts_index = normalize_module_id("src/foo/index.ts");

        // All have same base path
        assert_eq!(rust_file.path, "src/foo");
        assert_eq!(ts_file.path, "src/foo");
        assert_eq!(ts_index.path, "src/foo");

        // But different languages
        assert_eq!(rust_file.lang, "rs");
        assert_eq!(ts_file.lang, "ts");
        assert_eq!(ts_index.lang, "ts");

        // Keys should prevent collisions
        assert_eq!(rust_file.as_key(), "src/foo:rs");
        assert_eq!(ts_file.as_key(), "src/foo:ts");
        assert_eq!(ts_index.as_key(), "src/foo:ts");

        // Rust and TS files should NOT match
        assert_ne!(rust_file.as_key(), ts_file.as_key());

        // TS file and TS index SHOULD match (same language, same normalized path)
        assert_eq!(ts_file.as_key(), ts_index.as_key());
    }

    #[test]
    fn test_normalized_module_from_key() {
        let module = NormalizedModule {
            path: "src/utils".to_string(),
            lang: "ts".to_string(),
        };

        let key = module.as_key();
        assert_eq!(key, "src/utils:ts");

        let parsed = NormalizedModule::from_key(&key).unwrap();
        assert_eq!(parsed.path, "src/utils");
        assert_eq!(parsed.lang, "ts");
        assert_eq!(parsed, module);
    }

    #[test]
    fn test_normalized_module_various_extensions() {
        // Test all supported extensions
        let extensions = vec![
            ("file.ts", "ts"),
            ("file.tsx", "ts"),
            ("file.js", "ts"),
            ("file.jsx", "ts"),
            ("file.mjs", "ts"),
            ("file.cjs", "ts"),
            ("file.rs", "rs"),
            ("file.py", "py"),
            ("file.css", "css"),
            ("file.svelte", "ts"),
            ("file.vue", "ts"),
            ("file.astro", "ts"),
        ];

        for (input, expected_lang) in extensions {
            let module = normalize_module_id(input);
            assert_eq!(module.path, "file", "Failed for {}", input);
            assert_eq!(module.lang, expected_lang, "Failed for {}", input);
        }
    }

    #[test]
    fn test_normalized_module_windows_paths() {
        // Test Windows-style paths are normalized
        let module = normalize_module_id("src\\utils\\helpers.ts");
        assert_eq!(module.path, "src/utils/helpers");
        assert_eq!(module.lang, "ts");
        assert_eq!(module.as_key(), "src/utils/helpers:ts");
    }

    #[test]
    fn test_normalized_module_from_key_round_trip() {
        // Test round-trip conversion between module and key
        let test_cases = vec![
            ("src/utils:ts", "src/utils", "ts"),
            ("components/Button:ts", "components/Button", "ts"),
            ("lib/helpers:ts", "lib/helpers", "ts"),
            ("core:rs", "core", "rs"),
        ];

        for (key, expected_path, expected_lang) in test_cases {
            let module = NormalizedModule::from_key(key).unwrap();
            assert_eq!(module.path, expected_path);
            assert_eq!(module.lang, expected_lang);
            assert_eq!(module.as_key(), key);
        }
    }

    #[test]
    fn test_normalized_module_from_key_invalid() {
        // Test that invalid keys return None
        assert!(NormalizedModule::from_key("invalid_key_without_colon").is_none());
        assert!(NormalizedModule::from_key("").is_none());
    }

    #[test]
    fn test_shell_duplicate_idioms_are_informational() {
        let files = vec!["Makefile".to_string(), "scripts/install.mk".to_string()];
        let packages = files
            .iter()
            .map(|file| extract_package(file))
            .collect::<Vec<_>>();

        let (severity, reason) = compute_severity("usage", &files, &packages);

        assert_eq!(severity, DupSeverity::ReExportOrGeneric);
        assert!(reason.contains("Shell/Make runtime idiom"));
    }

    #[test]
    fn test_cli_script_duplicate_idioms_are_informational() {
        let files = vec![
            "scripts/build_marketplace_bundle.py".to_string(),
            "tools/scripts/github/repo-transfer.py".to_string(),
        ];
        let packages = files
            .iter()
            .map(|file| extract_package(file))
            .collect::<Vec<_>>();

        let (severity, reason) = compute_severity("main", &files, &packages);

        assert_eq!(severity, DupSeverity::ReExportOrGeneric);
        assert!(reason.contains("CLI/script runtime idiom"));
    }

    #[test]
    fn test_normalized_module_hash_and_eq() {
        // Test that NormalizedModule can be used in HashSet/HashMap
        use std::collections::HashSet;

        let mut set = HashSet::new();

        let mod1 = normalize_module_id("src/utils.ts");
        let mod2 = normalize_module_id("src/utils.ts");
        let mod3 = normalize_module_id("src/utils.rs");

        set.insert(mod1.clone());

        // Same module shouldn't be inserted twice
        assert!(set.contains(&mod2));
        assert_eq!(set.len(), 1);

        // Different language should be different
        set.insert(mod3.clone());
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_is_index_like_vue_files() {
        // Test that Vue index files are recognized
        assert!(is_index_like("src/components/index.vue"));
        assert!(is_index_like("components/index.vue"));
        assert!(is_index_like("/absolute/path/index.vue"));

        // Test case insensitivity
        assert!(is_index_like("src/components/INDEX.VUE"));

        // Non-index Vue files should return false
        assert!(!is_index_like("src/components/Button.vue"));
        assert!(!is_index_like("src/App.vue"));

        // Other index files should still work
        assert!(is_index_like("src/index.ts"));
        assert!(is_index_like("src/index.js"));
        assert!(is_index_like("src/index.svelte"));
        assert!(is_index_like("src/pages/index.astro"));
    }
}
