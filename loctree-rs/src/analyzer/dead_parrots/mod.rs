//! Dead Parrots Module - Janitor tools for code analysis and cleanup
//!
//! Named after the Monty Python sketch and the example-app project's "Dead Parrot Protocol"
//! for identifying unused/dead code that "just resting" but is actually dead.
//!
//! This module contains:
//! - Symbol search (`--symbol`)
//! - Impact analysis (`--impact`)
//! - Similarity check (`--check`/`--sim`)
//! - Dead exports detection (`--dead`)

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::types::{FileAnalysis, ReexportKind};

use super::root_scan::normalize_module_id;

/// Re-export info: (reexporter_file, original_name, exported_alias)
type ReexportInfoEntry = (String, String, String);
/// Map from (file_norm, symbol) to list of re-export entries
type ReexportInfoMap = HashMap<(String, String), Vec<ReexportInfoEntry>>;

/// Shadow export: export that exists but is never imported because another file exports the same symbol
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ShadowExport {
    /// Symbol name that is shadowed
    pub symbol: String,
    /// File that exports the symbol but is USED (imported through barrel/re-export)
    pub used_file: String,
    /// Line number in used file
    pub used_line: Option<usize>,
    /// Files that export the same symbol but are DEAD (never imported)
    pub dead_files: Vec<ShadowExportFile>,
    /// Total LOC across all dead files
    pub total_dead_loc: usize,
}

/// Individual dead file in a shadow export scenario
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ShadowExportFile {
    /// File path
    pub file: String,
    /// Line number where symbol is exported
    pub line: Option<usize>,
    /// Lines of code in this file
    pub loc: usize,
}

// Submodules
pub mod filters;
mod languages;
pub mod output;
pub mod search;

pub use filters::{SuppressionCounts, apply_semantic_suppression};

// Re-export public types and functions
pub use output::{
    print_dead_exports, print_impact_results, print_shadow_exports, print_similarity_results,
    print_symbol_results,
};
pub use search::{
    ImpactResult, SimilarityCandidate, SymbolFileMatch, SymbolMatch, SymbolMatchKind,
    SymbolSearchResult, analyze_impact, find_similar, search_symbol,
};

// Internal imports
use filters::{
    has_sys_modules_injection, is_ambient_export, is_dynamic_exec_template, is_flow_type_export,
    is_jsx_runtime_export, is_python_test_export, is_python_test_path, is_weakmap_registry_export,
    should_skip_dead_export_check,
};
use languages::{
    crate_import_matches_file, is_in_python_all, is_python_dunder_method, is_python_library,
    is_python_stdlib_export, is_rust_const_table, is_svelte_component_api, rust_has_known_derives,
};

fn strip_alias_prefix(path: &str) -> &str {
    // Drop leading alias markers like @core/... -> core/...
    let without_at = path.trim_start_matches('@');
    if let Some(idx) = without_at.find('/') {
        &without_at[idx + 1..]
    } else {
        without_at
    }
}
fn paths_match(a: &str, b: &str) -> bool {
    // Quick exact match check first
    if a == b {
        return true;
    }

    // Normalize separators to forward slashes
    let a_norm = a.replace('\\', "/");
    let b_norm = b.replace('\\', "/");
    // Trim leading "./" to align relative specs with normalized paths
    let a_clean = a_norm.trim_start_matches("./").to_string();
    let b_clean = b_norm.trim_start_matches("./").to_string();

    // On Windows, compare case-insensitively to avoid false mismatches on case variants
    let (a_clean, b_clean) = if cfg!(windows) {
        (a_clean.to_lowercase(), b_clean.to_lowercase())
    } else {
        (a_clean, b_clean)
    };

    if a_clean == b_clean {
        return true;
    }

    // Also allow alias-stripped comparisons (e.g., @core/utils vs src/core/utils)
    let a_alias = strip_alias_prefix(&a_clean);
    let b_alias = strip_alias_prefix(&b_clean);
    if a_alias == b_clean || b_alias == a_clean || a_alias == b_alias {
        return true;
    }

    // Normalize to module ids (collapse extensions/index) and compare paths
    let mod_a = normalize_module_id(&a_clean);
    let mod_b = normalize_module_id(&b_clean);
    if mod_a.path == mod_b.path || mod_a.as_key() == mod_b.as_key() {
        return true;
    }

    // Check if one is a suffix of the other at a path component boundary
    // This handles "src/App.tsx" vs "App.tsx" but prevents "foo.ts" matching "foo.test.ts"
    if a_clean.len() > b_clean.len() {
        // Check if a ends with b at a component boundary
        if let Some(suffix_start) = a_clean.rfind(&b_clean) {
            // Valid if b is at the start OR preceded by a separator
            if suffix_start == 0 || a_clean.chars().nth(suffix_start - 1) == Some('/') {
                return true;
            }
        }
    } else if b_clean.len() > a_clean.len() {
        // Check if b ends with a at a component boundary
        if let Some(suffix_start) = b_clean.rfind(&a_clean) {
            // Valid if a is at the start OR preceded by a separator
            if suffix_start == 0 || b_clean.chars().nth(suffix_start - 1) == Some('/') {
                return true;
            }
        }
    }

    false
}
/// Serde default for [`DeadExport::action`]: dead exports are *candidates*,
/// never verdicts. There is intentionally no "delete" action in the contract.
fn default_dead_action() -> String {
    "delete_candidate".to_string()
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct DeadExport {
    pub file: String,
    pub symbol: String,
    pub line: Option<usize>,
    pub confidence: String,
    /// Human-readable reason explaining why this export is considered dead
    pub reason: String,
    /// IDE integration URL (loctree://open?f={file}&l={line})
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub open_url: Option<String>,
    /// Whether this is a test file
    #[serde(default)]
    pub is_test: bool,
    /// Action contract: always `delete_candidate`. The detector proposes,
    /// the operator decides — `action: "delete"` is not part of this surface.
    #[serde(default = "default_dead_action")]
    pub action: String,
    /// Entry-point fence: true when the file is a declared or detected
    /// runtime entrypoint (Cargo `[[bin]]`, package.json `main`/`bin`,
    /// shebang script, Swift `@main`). Entry-point files must never be
    /// promoted into delete quick-wins.
    #[serde(default)]
    pub entrypoint: bool,
}

/// Controls which files are considered during dead-export detection.
#[derive(Debug, Clone, Default)]
pub struct DeadFilterConfig {
    /// Include tests and fixtures (default: false)
    pub include_tests: bool,
    /// Include helper/scripts/docs files (default: false)
    pub include_helpers: bool,
    /// Treat project as library/framework (ignore examples/demos noise)
    pub library_mode: bool,
    /// Extra example/demo globs to ignore when library_mode is enabled
    pub example_globs: Vec<String>,
    /// Python library mode: exports in __all__ are public API, not dead
    pub python_library_mode: bool,
    /// Include ambient declarations (declare global/module/namespace) in dead export analysis.
    /// By default these are excluded as they're consumed by TypeScript compiler, not imports.
    pub include_ambient: bool,
    /// Include dynamically generated symbols (exec/eval/compile templates) in dead export analysis.
    /// By default these are excluded as they're generated at runtime, not actual dead code.
    pub include_dynamic: bool,
    /// Glob patterns for suppressing dead-export findings.
    ///
    /// This is intended to be populated from `.loctignore` directives like:
    /// `@loctignore:dead-ok src/generated/**`
    pub dead_ok_globs: Vec<String>,
}
pub fn find_dead_exports(
    analyses: &[FileAnalysis],
    high_confidence: bool,
    open_base: Option<&str>,
    config: DeadFilterConfig,
) -> Vec<DeadExport> {
    let example_globset = if config.library_mode && !config.example_globs.is_empty() {
        let mut builder = globset::GlobSetBuilder::new();
        for pat in &config.example_globs {
            match globset::Glob::new(pat) {
                Ok(glob) => {
                    builder.add(glob);
                }
                Err(e) => {
                    eprintln!(
                        "[loctree][warn] invalid library_example_glob '{}': {}",
                        pat, e
                    );
                }
            }
        }
        builder.build().ok()
    } else {
        None
    };

    let dead_ok_globset = if !config.dead_ok_globs.is_empty() {
        let mut builder = globset::GlobSetBuilder::new();
        let mut any = false;

        let mut add_glob = |pat: &str| {
            let mut pat = pat.trim().replace('\\', "/");
            if let Some(rest) = pat.strip_prefix("./") {
                pat = rest.to_string();
            }
            if let Some(rest) = pat.strip_prefix('/') {
                pat = rest.to_string();
            }
            if pat.is_empty() {
                return;
            }
            match globset::Glob::new(&pat) {
                Ok(glob) => {
                    builder.add(glob);
                    any = true;
                }
                Err(e) => {
                    eprintln!("[loctree][warn] invalid dead-ok glob '{}': {}", pat, e);
                }
            }
        };

        for pat in &config.dead_ok_globs {
            let trimmed = pat.trim();
            if trimmed.is_empty() {
                continue;
            }
            // A trailing slash means "directory" in gitignore-ish conventions.
            // We add both the directory itself and its contents.
            if let Some(base) = trimmed.strip_suffix('/') {
                if !base.is_empty() {
                    add_glob(base);
                    add_glob(&format!("{}/**", base));
                }
            } else {
                add_glob(trimmed);
            }
        }

        if any { builder.build().ok() } else { None }
    } else {
        None
    };

    // Detect Python library mode if enabled
    let is_py_library = config.python_library_mode
        && analyses.iter().any(|a| {
            a.path.ends_with(".py")
                && std::path::Path::new(&a.path)
                    .ancestors()
                    .any(is_python_library)
        });

    // Skip Go for now to avoid false positives until package-level usage is implemented
    let analyses: Vec<&FileAnalysis> = analyses
        .iter()
        .filter(|a| !a.path.ends_with(".go"))
        .collect();

    // Cross-file reference signal for languages whose usage is by identifier
    // reference, not by symbol-level import (Swift): the union of every
    // identifier each file references. `parse_symbol_usages` already excludes a
    // file's own export names, so a name appearing here means "referenced by
    // some file other than (only) its definition". Import-edge dead detection is
    // blind to this, which is why every Swift declaration looked unused.
    let referenced_idents: HashSet<String> = analyses
        .iter()
        .flat_map(|a| a.symbol_usages.iter().map(|u| u.name.clone()))
        .collect();

    // Build usage set: (resolved_path, symbol_name)
    let mut used_exports: HashSet<(String, String)> = HashSet::new();
    // Track all imported symbol names as fallback (handles $lib/, @scope/, monorepo paths)
    let mut all_imported_symbols: HashSet<String> = HashSet::new();
    // Track crate-internal imports for Rust: (raw_path, symbol_name)
    let mut crate_internal_imports: Vec<(String, String)> = Vec::new();

    // === INFORMATIVE OUTPUT: Build detailed lookup maps for reason messages ===
    // Import counts: how many times each (file, symbol) is imported
    let mut import_counts: HashMap<(String, String), usize> = HashMap::new();
    // Re-export info: (file_norm, symbol) -> Vec<(reexporter_file, original_name, exported_alias)>
    let mut reexport_info: ReexportInfoMap = HashMap::new();
    // Dynamic import sources: file_norm -> Vec<importer_file>
    let mut dynamic_import_sources: HashMap<String, Vec<String>> = HashMap::new();

    for analysis in &analyses {
        for imp in &analysis.imports {
            let target_norm = if let Some(target) = &imp.resolved_path {
                // Use resolved path if available
                normalize_module_id(target).as_key()
            } else {
                // Fallback to source for bare imports (e.g., npm packages)
                // This ensures we don't mark exports as dead when they're imported without resolution
                normalize_module_id(&imp.source).as_key()
            };

            // Track named imports
            for sym in &imp.symbols {
                let used_name = if sym.is_default {
                    "default".to_string()
                } else {
                    sym.name.clone()
                };
                used_exports.insert((target_norm.clone(), used_name.clone()));
                // Track all imported symbol names as fallback for unresolved/incorrectly resolved paths
                // This catches symbols imported via $lib/, @scope/, or other aliases that may not resolve correctly
                if !used_name.is_empty() {
                    all_imported_symbols.insert(used_name.clone());
                }

                // Track crate-internal imports (crate::, super::, self::)
                if imp.is_crate_relative || imp.is_super_relative || imp.is_self_relative {
                    crate_internal_imports.push((imp.raw_path.clone(), used_name.clone()));
                }

                // INFORMATIVE: Count imports per (file, symbol)
                *import_counts
                    .entry((target_norm.clone(), used_name))
                    .or_insert(0) += 1;
            }
        }
        // Track dynamic imports for informative output
        for dyn_imp in &analysis.dynamic_imports {
            let dyn_norm = normalize_module_id(dyn_imp).as_key();
            dynamic_import_sources
                .entry(dyn_norm)
                .or_default()
                .push(analysis.path.clone());
        }
        // Track re-exports as usage (if A re-exports B, A uses B)
        for re in &analysis.reexports {
            let target_norm = re
                .resolved
                .as_ref()
                .map(|t| normalize_module_id(t).as_key())
                .unwrap_or_else(|| normalize_module_id(&re.source).as_key());
            match &re.kind {
                ReexportKind::Star => {
                    used_exports.insert((target_norm, "*".to_string()));
                }
                ReexportKind::Named(names) => {
                    for (original, exported) in names {
                        // Mark original name as used in target module
                        used_exports.insert((target_norm.clone(), original.clone()));
                        // INFORMATIVE: Track re-export info with alias
                        reexport_info
                            .entry((target_norm.clone(), original.clone()))
                            .or_default()
                            .push((analysis.path.clone(), original.clone(), exported.clone()));
                    }
                }
            }
        }
    }

    // CRITICAL FIX FOR SVELTE .d.ts RE-EXPORTS (60% of FPs):
    // TypeScript declaration files (.d.ts) re-export from implementation files (.js/.ts)
    // Pattern: foo.d.ts has `export { bar } from './foo.js'`
    // The exports in foo.js are NOT dead - they're the implementation for the .d.ts types
    // This fixes false positives like Svelte's easing functions being marked as dead
    let dts_reexports: Vec<_> = analyses
        .iter()
        .filter(|a| {
            a.path.ends_with(".d.ts") || a.path.ends_with(".d.mts") || a.path.ends_with(".d.cts")
        })
        .flat_map(|a| &a.reexports)
        .collect();

    for re in dts_reexports {
        // Mark the re-exported symbols from the source file as used
        let target_norm = re
            .resolved
            .as_ref()
            .map(|t| normalize_module_id(t).as_key())
            .unwrap_or_else(|| normalize_module_id(&re.source).as_key());

        match &re.kind {
            ReexportKind::Star => {
                // Star re-export: mark all exports from target as used
                used_exports.insert((target_norm, "*".to_string()));
            }
            ReexportKind::Named(names) => {
                // Named re-export: mark specific symbols as used
                for (original, _exported) in names {
                    used_exports.insert((target_norm.clone(), original.clone()));
                }
            }
        }
    }

    // Build set of all Tauri registered command handlers (used via generate_handler![])
    let tauri_handlers: HashSet<String> = analyses
        .iter()
        .flat_map(|a| a.tauri_registered_handlers.iter().cloned())
        .collect();

    // Go: gather identifiers used anywhere within the same directory (package-level)
    let mut go_local_uses_by_dir: HashMap<String, HashSet<String>> = HashMap::new();
    for analysis in analyses.iter().filter(|a| a.path.ends_with(".go")) {
        if let Some(dir) = std::path::Path::new(&analysis.path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
        {
            go_local_uses_by_dir
                .entry(dir)
                .or_default()
                .extend(analysis.local_uses.iter().cloned());
        }
    }

    // Build set of all path-qualified symbols from Rust files
    // These are calls like `command::branch::handle()` that don't use `use` imports
    let rust_path_qualified_symbols: HashSet<String> = analyses
        .iter()
        .filter(|a| a.path.ends_with(".rs"))
        .flat_map(|a| a.local_uses.iter().cloned())
        .collect();

    let shell_command_uses: HashSet<String> = analyses
        .iter()
        .filter(|a| a.language == "shell")
        .flat_map(|a| a.local_uses.iter().cloned())
        .collect();

    let shell_sourced_paths: Vec<String> = analyses
        .iter()
        .filter(|a| a.language == "shell")
        .flat_map(|a| a.imports.iter())
        .map(|imp| imp.resolved_path.as_ref().unwrap_or(&imp.source).clone())
        .collect();

    // Build transitive closure of files reachable from dynamic imports.
    // React.lazy(), Next.js dynamic(), and other code-splitting patterns use dynamic imports.
    // Files imported this way (and all their dependencies) should not be considered "dead".
    let dynamically_reachable: HashSet<String> = {
        // Build import graph: file_path -> list of resolved import paths
        let mut import_graph: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for analysis in &analyses {
            let key = normalize_module_id(&analysis.path).as_key();
            let imports: Vec<String> = analysis
                .imports
                .iter()
                .filter_map(|imp| imp.resolved_path.as_ref())
                .map(|p| normalize_module_id(p).as_key())
                .collect();
            import_graph.insert(key, imports);
        }

        // Collect initial set of dynamically imported files
        let mut reachable: HashSet<String> = HashSet::new();

        // First, collect from ImportEntry items with ImportKind::Dynamic (has resolved_path)
        // This is more reliable than raw dynamic_imports strings
        for analysis in &analyses {
            for imp in &analysis.imports {
                if matches!(imp.kind, crate::types::ImportKind::Dynamic) {
                    // Use resolved path if available (most reliable)
                    if let Some(resolved) = &imp.resolved_path {
                        let resolved_key = normalize_module_id(resolved).as_key();
                        reachable.insert(resolved_key);
                    }
                    // Also try to match the raw source path
                    let source_norm = normalize_module_id(&imp.source);
                    let source_key = source_norm.as_key();
                    let source_alias = strip_alias_prefix(&source_norm.path).to_string();

                    // Find matching file in analyses
                    for a in &analyses {
                        let a_norm = normalize_module_id(&a.path);
                        let a_key = a_norm.as_key();
                        if paths_match(&imp.source, &a_norm.path)
                            || paths_match(&imp.source, &a.path)
                            || a_norm.path.ends_with(&source_alias)
                        {
                            reachable.insert(a_key);
                            break;
                        }
                    }
                    reachable.insert(source_key);
                    if !source_alias.is_empty() {
                        reachable.insert(source_alias);
                    }
                }
            }
        }

        // Also check raw dynamic_imports strings as fallback
        for analysis in &analyses {
            for dyn_imp in &analysis.dynamic_imports {
                let dyn_norm = normalize_module_id(dyn_imp);
                let dyn_key = dyn_norm.as_key();
                let dyn_alias = strip_alias_prefix(&dyn_norm.path).to_string();
                // Find matching file in analyses
                for a in &analyses {
                    let a_norm = normalize_module_id(&a.path);
                    let a_key = a_norm.as_key();
                    if paths_match(dyn_imp, &a_norm.path)
                        || paths_match(dyn_imp, &a.path)
                        || a_norm.path.starts_with(&dyn_norm.path)
                        || a_norm.path.starts_with(&dyn_alias)
                        || a_norm.path.ends_with(&dyn_alias)
                    {
                        reachable.insert(a_key);
                        break;
                    }
                }
                // Also add the normalized dynamic import path itself
                reachable.insert(dyn_key.clone());
                // Alias-prefix fallback for unresolvable module ids (e.g., @core/foo)
                if !dyn_alias.is_empty() {
                    reachable.insert(dyn_alias.clone());
                }
            }
        }

        // BFS to compute transitive closure
        let mut queue: std::collections::VecDeque<String> = reachable.iter().cloned().collect();
        while let Some(current) = queue.pop_front() {
            if let Some(imports) = import_graph.get(&current) {
                for imp in imports {
                    if !reachable.contains(imp) {
                        reachable.insert(imp.clone());
                        queue.push_back(imp.clone());
                    }
                }
            }
        }

        reachable
    };

    // Identify dead exports
    let mut dead_candidates = Vec::new();

    for analysis in analyses {
        // Skip files that should be excluded from dead export detection
        if should_skip_dead_export_check(analysis, &config, example_globset.as_ref()) {
            continue;
        }

        // Skip lib.rs and main.rs - they are crate entry points:
        // - lib.rs is the crate's public API, called via qualified paths like `crate_name::func()`
        // - main.rs is the binary entry point, its exports are not meant to be imported
        let is_crate_root = analysis.path == "lib.rs"
            || analysis.path == "main.rs"
            || analysis.path.ends_with("/lib.rs")
            || analysis.path.ends_with("/main.rs");
        if is_crate_root {
            continue;
        }

        if is_rust_const_table(analysis) {
            continue;
        }

        let path_norm = normalize_module_id(&analysis.path).as_key();
        let is_go_file = analysis.path.ends_with(".go");
        let is_shell_file = analysis.language == "shell";
        let is_make_file = analysis.language == "make";

        // Skip noisy generated Go bindings (protobuf/grpc)
        if is_go_file
            && (analysis.path.ends_with(".pb.go")
                || analysis.path.ends_with(".pb.gw.go")
                || analysis.path.contains(".pb.")
                || analysis.path.contains(".pbjson"))
        {
            continue;
        }

        // Temporarily skip Go dead detection to avoid high FP until full package-level usage is implemented
        if is_go_file {
            continue;
        }

        // Skip if file is reachable from dynamic imports (directly or transitively)
        // This handles React.lazy(), Next.js dynamic(), and other code-splitting patterns
        if dynamically_reachable.contains(&path_norm) {
            continue;
        }

        let local_uses: HashSet<_> = analysis.local_uses.iter().cloned().collect();

        for exp in &analysis.exports {
            let is_rust_file = analysis.path.ends_with(".rs");
            let is_swift_file = analysis.path.ends_with(".swift");
            if is_make_file {
                // Make targets/variables are build-runtime declarations. A
                // future Make analyzer can report private unreachable targets,
                // but import absence is not dead code for Makefiles.
                continue;
            }
            if is_shell_file && exp.kind != "function" {
                // Exported env vars such as PATH and VIBECRAFTED_HOME are
                // process contracts, not dead-code candidates.
                continue;
            }
            if exp.kind == "reexport" {
                // Skip barrel bindings to avoid double-reporting re-exported symbols
                continue;
            }

            // Rust-specific heuristics: skip common macro-derived public types and CLI args
            let rust_macro_marked = is_rust_file
                && rust_has_known_derives(
                    &analysis.path,
                    &[
                        "serialize",
                        "deserialize",
                        "parser",
                        "args",
                        "valueenum",
                        "subcommand",
                        "fromargmatches",
                    ],
                );
            let rust_cli_pattern = is_rust_file
                && (exp.name.ends_with("Args")
                    || exp.name.ends_with("Command")
                    || exp.name.ends_with("Response")
                    || exp.name.ends_with("Request"));
            if rust_macro_marked || rust_cli_pattern {
                continue;
            }

            // Python-specific heuristics: skip framework magic patterns
            let is_python_file = analysis.path.ends_with(".py");
            let python_framework_magic = is_python_file
                && (
                    // arq framework looks up WorkerSettings by name convention
                    exp.name == "WorkerSettings"
                    // Standard Python package versioning
                    || exp.name == "__version__"
                    // pytest fixtures can be used without explicit import (via conftest.py)
                    || (analysis.path.contains("conftest") && exp.kind == "def")
                );
            if python_framework_magic {
                continue;
            }

            // Python library mode: skip exports in __all__ (public API)
            // Also check CPython stdlib pattern independent of is_py_library flag
            // because CPython stdlib has unique Lib/ structure that should be recognized
            let is_stdlib = is_python_file && is_python_stdlib_export(analysis, &exp.name);
            if (is_py_library || is_stdlib) && is_python_file {
                // Skip exports in __all__ (definitive public API marker)
                if is_in_python_all(analysis, &exp.name) {
                    continue;
                }
                // Skip CPython stdlib public API (in Lib/ directory)
                if is_stdlib {
                    continue;
                }
                // Skip dunder methods (__init__, __str__, etc. - runtime protocol)
                if is_python_dunder_method(&exp.name) {
                    continue;
                }
            }

            // Django/Wagtail mixin pattern heuristic:
            // Classes ending in "Mixin" are typically used via multiple inheritance
            // and their methods are called via MRO (Method Resolution Order), not directly imported
            // Common patterns: LoginRequiredMixin, PermissionRequiredMixin, ButtonsColumnMixin, etc.
            let is_django_mixin =
                is_python_file && exp.kind == "class" && exp.name.ends_with("Mixin");
            if is_django_mixin {
                // Skip mixin classes from dead export detection
                // They're used via inheritance which may not be fully tracked in complex codebases
                continue;
            }
            if is_python_test_export(analysis, exp) || is_python_test_path(&analysis.path) {
                continue;
            }

            if exp.name == "default"
                && (analysis.path.ends_with("page.tsx") || analysis.path.ends_with("layout.tsx"))
            {
                // Next.js / framework roots - ignore default export
                continue;
            }

            // JS/TS runtime/framework exports that are inherently used via tooling/framework
            let is_ts_file = analysis.path.ends_with(".ts")
                || analysis.path.ends_with(".tsx")
                || analysis.path.ends_with(".js")
                || analysis.path.ends_with(".jsx")
                || analysis.path.ends_with(".mjs")
                || analysis.path.ends_with(".cjs");
            let ts_runtime_symbol = is_ts_file
                && (matches!(
                    exp.name.as_str(),
                    "jsx" | "jsxs" | "jsxDEV" | "Fragment" | "VoidComponent" | "Component"
                ) || analysis.path.contains("jsx-runtime"));
            let ts_framework_magic = is_ts_file
                && (matches!(
                    exp.name.as_str(),
                    "start" | "resolveRoute" | "enhance" | "load" | "PageLoad" | "LayoutLoad"
                ) || analysis.path.contains("sveltekit")
                    || analysis.path.contains("app/navigation"));
            // VSCode extension lifecycle hooks: extension host invokes activate()
            // and deactivate() by name when the bundle file declared in
            // package.json#main is loaded. They never appear as imports in any
            // .ts file because the extension host loads the module via Node
            // require(), not an ES import. Without this filter loctree reports
            // them as "very-high confidence: remove", which is the exact lie
            // the truth-of-findings branch exists to fix.
            let is_extension_entry = analysis.path.ends_with("/extension.ts")
                || analysis.path.ends_with("/extension.tsx")
                || analysis.path.ends_with("/extension.js")
                || analysis.path.ends_with("/extension.jsx")
                || analysis.path.ends_with("/extension.mjs")
                || analysis.path.ends_with("/extension.cjs")
                || analysis.path == "extension.ts"
                || analysis.path == "extension.js";
            let vscode_lifecycle_hook = is_ts_file
                && is_extension_entry
                && matches!(exp.name.as_str(), "activate" | "deactivate");
            if ts_runtime_symbol || ts_framework_magic || vscode_lifecycle_hook {
                continue;
            }

            if high_confidence && exp.name == "default" {
                // High confidence: ignore "default" exports (too often implicit usage)
                continue;
            }

            let is_used = used_exports.contains(&(path_norm.clone(), exp.name.clone()));
            // Also check if "*" was imported from this file
            let star_used = used_exports.contains(&(path_norm.clone(), "*".to_string()));
            let locally_used = local_uses.contains(&exp.name);
            let go_pkg_used = if analysis.path.ends_with(".go") {
                std::path::Path::new(&analysis.path)
                    .parent()
                    .and_then(|p| go_local_uses_by_dir.get(&p.to_string_lossy().to_string()))
                    .is_some_and(|set| set.contains(&exp.name))
            } else {
                false
            };
            // Check if this is a Tauri command handler registered via generate_handler![]
            let is_tauri_handler = tauri_handlers.contains(&exp.name);
            // Fallback: check if symbol is imported anywhere by name
            // This handles cases where path resolution fails (monorepos, $lib/, @scope/ packages)
            let imported_by_name = all_imported_symbols.contains(&exp.name);
            // Check if this is likely a Svelte component API method (called via bind:this)
            let is_svelte_api = is_svelte_component_api(&analysis.path, &exp.name);
            // Check if this Rust symbol is called via path qualification (e.g., `module::func()`)
            let is_rust_path_qualified =
                analysis.path.ends_with(".rs") && rust_path_qualified_symbols.contains(&exp.name);

            // Check if this export is imported via crate-internal paths (crate::, super::, self::)
            // Use fuzzy matching since nested brace imports may have symbol names with extra chars
            let crate_import_count = crate_internal_imports
                .iter()
                .filter(|(raw_path, symbol)| {
                    // Exact match or symbol contains the export name (handles "MENU_GAP}" matching "MENU_GAP")
                    let symbol_matches = symbol == &exp.name
                        || symbol.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
                            == exp.name
                        || symbol.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '_')
                            == exp.name;
                    symbol_matches && crate_import_matches_file(raw_path, &analysis.path, &exp.name)
                })
                .count();
            let is_crate_imported = crate_import_count > 0;

            // Check if this is a JSX runtime export (jsx, jsxs, Fragment, etc.)
            // These are consumed by TypeScript/Babel compiler, not by regular imports
            let is_jsx_runtime = is_jsx_runtime_export(&exp.name, &analysis.path);

            // Check if this is a Flow type-only export
            // Flow type exports are consumed by Flow type checker, not by runtime imports
            let is_flow_type = is_flow_type_export(exp, analysis);

            // Check if this export is used in a WeakMap/WeakSet registry pattern
            // These are dynamically accessed and won't show up in static imports
            let is_weak_registry = is_weakmap_registry_export(exp, analysis);

            // Check if this is an ambient declaration export (declare global/module/namespace)
            // These are consumed by TypeScript compiler for type augmentation, not by imports
            // Skip check if user requested to include ambient declarations
            let is_ambient = !config.include_ambient && is_ambient_export(exp, analysis);

            // Check if this export matches a dynamically generated pattern (exec/eval/compile)
            // These are template placeholders like "get%s" that generate functions at runtime
            // Skip check if user requested to include dynamic symbols
            let is_dynamic_generated =
                !config.include_dynamic && is_dynamic_exec_template(&exp.name, analysis);

            // Check if file uses sys.modules monkey-patching (e.g., sys.modules['compat'] = wrapper)
            // ALL exports from such files are accessible at runtime via the injected module name
            let is_sys_modules_injected =
                !config.include_dynamic && has_sys_modules_injection(analysis);

            // Check if this is a default export from a dynamically imported file
            // React lazy(), Vue async components, and Next.js dynamic() use dynamic imports
            // which typically only consume the default export
            let is_dynamically_imported_default =
                exp.export_type == "default" && dynamic_import_sources.contains_key(&path_norm);
            let shell_called_by_name = is_shell_file && shell_command_uses.contains(&exp.name);
            let shell_sourced_public_api = is_shell_file
                && !exp.name.starts_with('_')
                && shell_sourced_paths
                    .iter()
                    .any(|source_path| paths_match(source_path, &analysis.path));
            // Swift usage is by identifier reference (instance member, override,
            // protocol conformance), never by symbol import. `locally_used`
            // (above) now covers same-file references; this covers cross-file
            // ones via the aggregated reference set. Together they keep a
            // referenced Swift declaration out of the candidate set instead of
            // letting import-edge absence flag it.
            let is_swift_referenced = is_swift_file && referenced_idents.contains(&exp.name);

            if !is_used
                && !star_used
                && !locally_used
                && !go_pkg_used
                && !is_tauri_handler
                && !imported_by_name
                && !is_svelte_api
                && !is_rust_path_qualified
                && !is_crate_imported
                && !is_jsx_runtime
                && !is_flow_type
                && !is_weak_registry
                && !is_ambient
                && !is_dynamic_generated
                && !is_sys_modules_injected
                && !is_dynamically_imported_default
                && !shell_called_by_name
                && !shell_sourced_public_api
                && !is_swift_referenced
            {
                let open_url = super::build_open_url(&analysis.path, exp.line, open_base);

                // Calculate actual counts for informative output
                let import_count = import_counts
                    .get(&(path_norm.clone(), exp.name.clone()))
                    .copied()
                    .unwrap_or(0);
                let reexport_entries = reexport_info
                    .get(&(path_norm.clone(), exp.name.clone()))
                    .cloned()
                    .unwrap_or_default();
                let reexport_count = reexport_entries.len();
                let dynamic_count = dynamic_import_sources
                    .get(&path_norm)
                    .map(|v| v.len())
                    .unwrap_or(0);

                // Build human-readable reason with context for user decision
                let reason = if is_shell_file {
                    format!(
                        "Shell function '{}' has no detected shell call sites. \
                         Checked: same-file commands, cross-file shell command words, sourced public API. \
                         Shell dispatch can be dynamic, so verify case/handler wiring before removal.",
                        exp.name
                    )
                } else if is_rust_file {
                    format!(
                        "Exported symbol '{}' has no detected usages. \
                         Checked: use statements ({}), path-qualified calls (0), \
                         crate:: imports ({}), Tauri invoke_handler (not found). \
                         Consider: If this is a public API consumed externally, it's expected. \
                         If internal-only, consider removing or making private.",
                        exp.name, import_count, crate_import_count
                    )
                } else if is_swift_file {
                    format!(
                        "Swift declaration '{}' has no detected references: not used \
                         within its own file and not referenced by any other file. \
                         Swift `import` is module-level, so import-edge absence is not \
                         evidence — this is a true no-reference result. Verify it is not \
                         reached dynamically (#selector / @objc, KVC, SwiftUI or Storyboard \
                         lookup by name, protocol-witness dispatch, or @main) before removing.",
                        exp.name
                    )
                } else {
                    // Build detailed re-export info for informative output
                    let reexport_details = if !reexport_entries.is_empty() {
                        let details: Vec<String> = reexport_entries
                            .iter()
                            .take(3) // Limit to 3 for readability
                            .map(|(file, original, alias)| {
                                if original != alias {
                                    format!("as '{}' in {}", alias, file)
                                } else {
                                    file.clone()
                                }
                            })
                            .collect();
                        let more = if reexport_entries.len() > 3 {
                            format!(" (+{} more)", reexport_entries.len() - 3)
                        } else {
                            String::new()
                        };
                        format!(" ({}{})", details.join(", "), more)
                    } else {
                        String::new()
                    };

                    // Build dynamic import details
                    let dynamic_details = if dynamic_count > 0 {
                        let sources: Vec<String> = dynamic_import_sources
                            .get(&path_norm)
                            .map(|v| v.iter().take(2).cloned().collect())
                            .unwrap_or_default();
                        let more = if dynamic_count > 2 {
                            format!(" +{} more", dynamic_count - 2)
                        } else {
                            String::new()
                        };
                        format!(" (by {}{})", sources.join(", "), more)
                    } else {
                        String::new()
                    };

                    format!(
                        "Exported symbol '{}' has no detected imports. \
                         Checked: import statements ({}), re-exports ({}){}, \
                         dynamic imports ({}){}, JSX references (0). \
                         Consider: If used via barrel exports or external packages, verify manually. \
                         If truly unused, safe to remove.",
                        exp.name,
                        import_count,
                        reexport_count,
                        reexport_details,
                        dynamic_count,
                        dynamic_details
                    )
                };

                dead_candidates.push(DeadExport {
                    file: analysis.path.clone(),
                    symbol: exp.name.clone(),
                    line: exp.line,
                    confidence: if is_shell_file {
                        "medium".to_string()
                    } else if high_confidence {
                        "very-high".to_string()
                    } else {
                        "high".to_string()
                    },
                    reason,
                    open_url: Some(open_url),
                    is_test: analysis.is_test,
                    action: default_dead_action(),
                    entrypoint: false,
                });
            }
        }
    }

    if let Some(globs) = dead_ok_globset {
        dead_candidates.retain(|d| {
            let lower = d.file.to_ascii_lowercase();
            !globs.is_match(&d.file) && !globs.is_match(&lower)
        });
    }

    let mut seen_dead: HashSet<(String, String, Option<usize>)> = HashSet::new();
    dead_candidates.retain(|d| seen_dead.insert((d.file.clone(), d.symbol.clone(), d.line)));

    dead_candidates
}

// ============================================================================
// Dead truth — canonical pipeline (single count) + cross-check before verdict
// ============================================================================

/// Counters describing what the cross-check changed. Candidates are degraded
/// (confidence → "low") with the evidence appended to `reason`; they are
/// never silently dropped — the operator sees the same list everywhere.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
pub struct CrossCheckStats {
    /// Candidates degraded because the literal layer found identifier hits
    /// outside the defining file (graph blindness: test-dir imports,
    /// lazy `import('x').then(m => m.Y)` member access, qualified calls).
    pub literal_identifier_degraded: usize,
    /// Candidates degraded because of a string-literal reference outside the
    /// defining file (spawn-by-string, dynamic loading by name).
    pub literal_string_degraded: usize,
    /// Candidates degraded because the symbol graph (when present in the
    /// snapshot) carries live references outside the definition.
    pub symbol_graph_degraded: usize,
    /// Candidates marked as runtime entrypoints (never delete quick-wins).
    pub entrypoint_fenced: usize,
}

/// Result of the canonical dead-export pipeline. All public surfaces
/// (`loct dead`, `loct twins`, `loct findings`, repo-view) must consume this
/// so the repo has exactly one dead number per snapshot.
#[derive(Debug, Clone, Default)]
pub struct DeadTruth {
    pub dead: Vec<DeadExport>,
    pub suppression: SuppressionCounts,
    pub cross_check: CrossCheckStats,
}

/// The one canonical [`DeadFilterConfig`]: defaults plus `.loctignore`
/// dead-ok globs merged from every snapshot root.
pub fn canonical_dead_filter(snapshot: &crate::snapshot::Snapshot) -> DeadFilterConfig {
    let mut dead_ok_globs: Vec<String> = snapshot
        .metadata
        .roots
        .iter()
        .flat_map(|root| crate::fs_utils::load_loctignore_dead_ok_globs(std::path::Path::new(root)))
        .collect();
    dead_ok_globs.sort();
    dead_ok_globs.dedup();
    DeadFilterConfig {
        dead_ok_globs,
        ..Default::default()
    }
}

/// Canonical dead-export computation: one config, semantic suppression,
/// literal + symbol-graph cross-check, entry-point fence. This is the single
/// source of the dead number for every reporting surface.
pub fn compute_dead_truth(snapshot: &crate::snapshot::Snapshot) -> DeadTruth {
    compute_dead_truth_with(snapshot, canonical_dead_filter(snapshot), false)
}

/// [`compute_dead_truth`] with an explicit filter config (CLI flags such as
/// `--with-tests` are operator-requested deviations; the pipeline stages stay
/// identical so flagged runs remain comparable with the canonical number).
pub fn compute_dead_truth_with(
    snapshot: &crate::snapshot::Snapshot,
    config: DeadFilterConfig,
    high_confidence: bool,
) -> DeadTruth {
    let raw = find_dead_exports(&snapshot.files, high_confidence, None, config);

    let mut suppression = SuppressionCounts::default();
    let candidates = match snapshot.semantic_facts.as_ref() {
        Some(facts) => apply_semantic_suppression(raw, facts, &mut suppression),
        None => raw,
    };

    let (dead, cross_check) = cross_check_dead_exports(candidates, snapshot);

    DeadTruth {
        dead,
        suppression,
        cross_check,
    }
}

/// Maximum file size the disk literal pass will read. Larger files are
/// skipped (generated bundles would dominate scan time, and the artifact
/// fence already keeps them out of the candidate set).
const CROSS_CHECK_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Maximum stored evidence hits per needle — enough for the reason string.
const CROSS_CHECK_MAX_HITS: usize = 8;

fn norm_rel_path(p: &str) -> String {
    p.replace('\\', "/").trim_start_matches("./").to_string()
}

/// File stems too generic to serve as spawn-by-string evidence.
fn stem_is_generic(stem: &str) -> bool {
    stem.len() < 4
        || matches!(
            stem,
            "index" | "main" | "utils" | "types" | "test" | "tests" | "setup" | "config"
        )
}

/// Candidate-file stem variants used for spawn-by-string detection
/// (`stt_bridge.rs` spawned as `"stt-bridge"` must still match).
fn stem_variants(path: &str) -> Vec<String> {
    let Some(stem) = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
    else {
        return Vec::new();
    };
    if stem_is_generic(stem) {
        return Vec::new();
    }
    let mut variants = vec![stem.to_string()];
    let dashed = stem.replace('_', "-");
    if dashed != stem {
        variants.push(dashed);
    }
    let underscored = stem.replace('-', "_");
    if underscored != stem && !variants.contains(&underscored) {
        variants.push(underscored);
    }
    variants
}

/// Cross-check dead-export candidates against the literal truth layer and —
/// when the snapshot carries one — the symbol graph, then apply the
/// entry-point fence. Evidence degrades confidence and lands in `reason`;
/// candidates are never silently dropped here.
pub fn cross_check_dead_exports(
    mut candidates: Vec<DeadExport>,
    snapshot: &crate::snapshot::Snapshot,
) -> (Vec<DeadExport>, CrossCheckStats) {
    use crate::analyzer::occurrences::{OccurrenceKind, occurrences_in_line, scan_text};

    let mut stats = CrossCheckStats::default();
    if candidates.is_empty() {
        return (candidates, stats);
    }

    let symbols: HashSet<String> = candidates.iter().map(|c| c.symbol.clone()).collect();
    // candidate file -> stem variants (spawn-by-string fence)
    let mut stems_by_file: HashMap<String, Vec<String>> = HashMap::new();
    for c in &candidates {
        stems_by_file
            .entry(c.file.clone())
            .or_insert_with(|| stem_variants(&c.file));
    }
    let all_stems: HashSet<String> = stems_by_file.values().flatten().cloned().collect();

    // Evidence: needle -> hits (file, line). Deduped via BTreeSet keys.
    let mut ident_hits: HashMap<String, Vec<(String, usize)>> = HashMap::new();
    let mut string_hits: HashMap<String, Vec<(String, usize)>> = HashMap::new();
    let mut seen: std::collections::BTreeSet<(String, String, usize, bool)> =
        std::collections::BTreeSet::new();

    let push_hit =
        |map: &mut HashMap<String, Vec<(String, usize)>>,
         needle: &str,
         file: &str,
         line: usize,
         is_string: bool,
         seen: &mut std::collections::BTreeSet<(String, String, usize, bool)>| {
            if !seen.insert((needle.to_string(), file.to_string(), line, is_string)) {
                return;
            }
            let entry = map.entry(needle.to_string()).or_default();
            if entry.len() < CROSS_CHECK_MAX_HITS {
                entry.push((file.to_string(), line));
            }
        };

    // --- Pass 1: snapshot facts (always available, zero IO) -----------------
    for fa in &snapshot.files {
        for usage in &fa.symbol_usages {
            if symbols.contains(&usage.name) {
                push_hit(
                    &mut ident_hits,
                    &usage.name,
                    &fa.path,
                    usage.line,
                    false,
                    &mut seen,
                );
            }
        }
        for lit in &fa.string_literals {
            for sym in &symbols {
                if !occurrences_in_line(&lit.value, sym).is_empty() {
                    push_hit(&mut string_hits, sym, &fa.path, lit.line, true, &mut seen);
                }
            }
            for stem in &all_stems {
                if !occurrences_in_line(&lit.value, stem).is_empty() {
                    push_hit(&mut string_hits, stem, &fa.path, lit.line, true, &mut seen);
                }
            }
        }
    }

    // --- Pass 2: disk literal scan (raw-byte truth, fills graph blind spots)
    let root = snapshot
        .metadata
        .roots
        .first()
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir());
    if let Some(root) = &root {
        for fa in &snapshot.files {
            let full = root.join(&fa.path);
            match std::fs::metadata(&full) {
                Ok(meta) if meta.len() <= CROSS_CHECK_MAX_FILE_BYTES => {}
                _ => continue,
            }
            let Ok(content) = std::fs::read_to_string(&full) else {
                continue;
            };
            for sym in &symbols {
                if !content.contains(sym.as_str()) {
                    continue;
                }
                for occ in scan_text(&fa.path, &content, sym) {
                    match occ.occurrence_kind {
                        OccurrenceKind::Comment => {}
                        OccurrenceKind::StringLiteral => {
                            push_hit(&mut string_hits, sym, &fa.path, occ.line, true, &mut seen);
                        }
                        _ => {
                            push_hit(&mut ident_hits, sym, &fa.path, occ.line, false, &mut seen);
                        }
                    }
                }
            }
            for stem in &all_stems {
                if !content.contains(stem.as_str()) {
                    continue;
                }
                for occ in scan_text(&fa.path, &content, stem) {
                    if occ.occurrence_kind == OccurrenceKind::StringLiteral {
                        push_hit(&mut string_hits, stem, &fa.path, occ.line, true, &mut seen);
                    }
                }
            }
        }
    }

    // --- Pass 3: symbol graph (feature-gated on snapshot presence) ----------
    // graph_refs: (norm file, symbol) -> live reference count outside the file
    let mut graph_refs: HashMap<(String, String), usize> = HashMap::new();
    if let Some(graph) = snapshot.symbol_graph.as_ref()
        && !graph.symbols.is_empty()
    {
        use crate::symbols::OccurrenceRole;
        // definition file per symbol id (only ids whose name is a candidate)
        let mut def_by_id: HashMap<&crate::symbols::SymbolId, (String, String)> = HashMap::new();
        for node in &graph.symbols {
            if !symbols.contains(&node.name) {
                continue;
            }
            if let Some(file) = &node.file {
                def_by_id.insert(
                    &node.id,
                    (norm_rel_path(&file.to_string_lossy()), node.name.clone()),
                );
            }
        }
        for occ in &graph.occurrences {
            if !matches!(
                occ.role,
                OccurrenceRole::Reference | OccurrenceRole::Call | OccurrenceRole::Import
            ) {
                continue;
            }
            if let Some((def_file, name)) = def_by_id.get(&occ.symbol_id) {
                let occ_file = norm_rel_path(&occ.file.to_string_lossy());
                if &occ_file != def_file {
                    *graph_refs
                        .entry((def_file.clone(), name.clone()))
                        .or_insert(0) += 1;
                }
            }
        }
    }

    // --- Entry-point fence ---------------------------------------------------
    let entrypoint_paths = filters::runtime_entrypoint_paths(snapshot);
    let mut probe_cache: HashMap<String, bool> = HashMap::new();
    let mut is_entrypoint_file = |file: &str| -> bool {
        let norm = norm_rel_path(file);
        if entrypoint_paths.contains(&norm) {
            return true;
        }
        if let Some(root) = &root {
            return *probe_cache
                .entry(norm.clone())
                .or_insert_with(|| filters::probe_entrypoint_marker(&root.join(file), file));
        }
        false
    };

    // --- Verdict assembly: degrade with evidence, fence entrypoints ---------
    for candidate in &mut candidates {
        let cand_norm = norm_rel_path(&candidate.file);
        let outside = |hits: Option<&Vec<(String, usize)>>| -> Vec<(String, usize)> {
            hits.map(|v| {
                v.iter()
                    .filter(|(file, _)| norm_rel_path(file) != cand_norm)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
        };

        let ident_outside = outside(ident_hits.get(&candidate.symbol));
        let mut string_outside = outside(string_hits.get(&candidate.symbol));
        for stem in stems_by_file.get(&candidate.file).into_iter().flatten() {
            string_outside.extend(outside(string_hits.get(stem)));
        }
        let graph_count = graph_refs
            .get(&(cand_norm.clone(), candidate.symbol.clone()))
            .copied()
            .unwrap_or(0);

        let mut notes: Vec<String> = Vec::new();
        let mut degrade = false;

        if let Some((file, line)) = ident_outside.first() {
            degrade = true;
            stats.literal_identifier_degraded += 1;
            notes.push(format!(
                "literal: {} identifier hit(s) outside def (first: {}:{})",
                ident_outside.len(),
                file,
                line
            ));
        } else {
            notes.push("literal: 0 hits outside def".to_string());
        }

        if let Some((file, line)) = string_outside.first() {
            degrade = true;
            stats.literal_string_degraded += 1;
            notes.push(format!(
                "string-literal reference outside def ({}:{}) — possible spawn-by-string/dynamic load",
                file, line
            ));
        }

        if graph_count > 0 {
            degrade = true;
            stats.symbol_graph_degraded += 1;
            notes.push(format!(
                "symbol graph: {} live reference(s) outside def",
                graph_count
            ));
        }

        if is_entrypoint_file(&candidate.file) {
            candidate.entrypoint = true;
            stats.entrypoint_fenced += 1;
            notes.push(
                "entry-point fence: runtime entrypoint — excluded from delete quick-wins"
                    .to_string(),
            );
        } else {
            notes.push("not an entrypoint".to_string());
        }

        if degrade && candidate.confidence != "low" {
            candidate.confidence = "low".to_string();
        }
        candidate.action = default_dead_action();
        candidate.reason = format!(
            "{} Cross-check: {}.",
            candidate.reason.trim_end(),
            notes.join("; ")
        );
    }

    (candidates, stats)
}

/// Detect shadow exports: same symbol exported by multiple files, but only one is actually used.
///
/// This identifies "zombie" files that export symbols which are masked by barrel re-exports
/// from other files. For example:
/// - `stores/conversationHostStore.ts` exports `conversationHostStore` (361 LOC) - DEAD
/// - `aiStore/slices/conversationHostSlice.ts` exports `conversationHostStore` - USED
/// - Barrel `@ai-suite/state` re-exports from the NEW file, old file is zombie
pub fn find_shadow_exports(analyses: &[FileAnalysis]) -> Vec<ShadowExport> {
    // Build map of symbol_name -> Vec<(file_path, line, export)>
    let mut symbol_map: HashMap<String, Vec<(String, Option<usize>, String)>> = HashMap::new();

    for analysis in analyses {
        for exp in &analysis.exports {
            // Skip re-export bindings (we only care about original exports)
            if exp.kind == "reexport" {
                continue;
            }

            symbol_map.entry(exp.name.clone()).or_default().push((
                analysis.path.clone(),
                exp.line,
                exp.kind.clone(),
            ));
        }
    }

    // Build set of (file, symbol) that are actually imported
    let mut used_exports: HashSet<(String, String)> = HashSet::new();

    for analysis in analyses {
        for imp in &analysis.imports {
            let target_norm = if let Some(target) = &imp.resolved_path {
                normalize_module_id(target).as_key()
            } else {
                normalize_module_id(&imp.source).as_key()
            };

            for sym in &imp.symbols {
                let used_name = if sym.is_default {
                    "default".to_string()
                } else {
                    sym.name.clone()
                };
                used_exports.insert((target_norm.clone(), used_name));
            }
        }

        // Also check re-exports
        for re in &analysis.reexports {
            let target_norm = re
                .resolved
                .as_ref()
                .map(|t| normalize_module_id(t).as_key())
                .unwrap_or_else(|| normalize_module_id(&re.source).as_key());
            match &re.kind {
                ReexportKind::Star => {
                    used_exports.insert((target_norm, "*".to_string()));
                }
                ReexportKind::Named(names) => {
                    for (original, _exported) in names {
                        used_exports.insert((target_norm.clone(), original.clone()));
                    }
                }
            }
        }
    }

    let mut shadows = Vec::new();

    // Find symbols exported by multiple files
    for (symbol, exporters) in symbol_map {
        if exporters.len() <= 1 {
            continue; // Not a duplicate, skip
        }

        // Check which files are actually imported
        let mut used_files = Vec::new();
        let mut dead_files = Vec::new();

        for (file, line, _kind) in &exporters {
            let file_norm = normalize_module_id(file).as_key();
            let is_used = used_exports.contains(&(file_norm.clone(), symbol.clone()))
                || used_exports.contains(&(file_norm, "*".to_string()));

            if is_used {
                used_files.push((file.clone(), *line));
            } else {
                // Get LOC for this file
                let loc = analyses
                    .iter()
                    .find(|a| a.path == *file)
                    .map(|a| a.loc)
                    .unwrap_or(0);

                dead_files.push(ShadowExportFile {
                    file: file.clone(),
                    line: *line,
                    loc,
                });
            }
        }

        // Only report if we have both used and dead files
        if !used_files.is_empty() && !dead_files.is_empty() {
            // Use the first used file as the canonical one
            let (used_file, used_line) = used_files.into_iter().next().unwrap();
            let total_dead_loc = dead_files.iter().map(|f| f.loc).sum();

            shadows.push(ShadowExport {
                symbol,
                used_file,
                used_line,
                dead_files,
                total_dead_loc,
            });
        }
    }

    // Sort by total_dead_loc descending (highest impact first)
    shadows.sort_by_key(|b| std::cmp::Reverse(b.total_dead_loc));

    shadows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OutputMode;
    use crate::types::{
        ExportSymbol, ImportEntry, ImportKind, ImportSymbol, ReexportEntry, ReexportKind,
        SymbolMatch as TypesSymbolMatch,
    };

    fn mock_file(path: &str) -> FileAnalysis {
        FileAnalysis {
            path: path.to_string(),
            ..Default::default()
        }
    }

    fn mock_file_with_exports(path: &str, exports: Vec<&str>) -> FileAnalysis {
        FileAnalysis {
            path: path.to_string(),
            exports: exports
                .into_iter()
                .enumerate()
                .map(|(i, name)| ExportSymbol {
                    name: name.to_string(),
                    kind: "function".to_string(),
                    export_type: "named".to_string(),
                    line: Some(i + 1),
                    params: Vec::new(),

                    symbol_id: crate::types::SymbolIdV1::default(),
                })
                .collect(),
            ..Default::default()
        }
    }

    fn mock_file_with_matches(path: &str, matches: Vec<(usize, &str)>) -> FileAnalysis {
        FileAnalysis {
            path: path.to_string(),
            matches: matches
                .into_iter()
                .map(|(line, ctx)| TypesSymbolMatch {
                    line,
                    context: ctx.to_string(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn test_search_symbol_empty() {
        let analyses: Vec<FileAnalysis> = vec![];
        let result = search_symbol("foo", &analyses);
        assert!(!result.found);
        assert!(result.files.is_empty());
    }

    #[test]
    fn test_search_symbol_no_matches() {
        let analyses = vec![mock_file("src/utils.ts"), mock_file("src/helpers.ts")];
        let result = search_symbol("foo", &analyses);
        assert!(!result.found);
    }

    #[test]
    fn test_search_symbol_with_matches() {
        let analyses = vec![
            mock_file_with_matches(
                "src/utils.ts",
                vec![(10, "const foo = 1"), (20, "return foo")],
            ),
            mock_file("src/helpers.ts"),
        ];
        let result = search_symbol("foo", &analyses);
        assert!(result.found);
        assert_eq!(result.files.len(), 1);
    }

    #[test]
    fn test_find_dead_exports_respects_from_imports() {
        let exporter = mock_file_with_exports("pkg/module.py", vec!["Foo"]);
        let mut importer = mock_file("main.py");
        let mut imp = ImportEntry::new("pkg.module".to_string(), ImportKind::Static);
        imp.resolved_path = Some("pkg/module.py".to_string());
        imp.symbols.push(ImportSymbol {
            name: "Foo".to_string(),
            alias: None,
            is_default: false,
        });
        importer.imports.push(imp);

        let result = find_dead_exports(
            &[importer, exporter],
            false,
            None,
            DeadFilterConfig::default(),
        );
        assert!(
            result.is_empty(),
            "export imported with explicit symbol should not be dead"
        );
    }

    #[test]
    fn test_find_dead_exports_respects_local_usage() {
        let mut file = mock_file_with_exports("app.py", vec!["refresh"]);
        file.local_uses.push("refresh".to_string());
        let result = find_dead_exports(&[file], false, None, DeadFilterConfig::default());
        assert!(
            result.is_empty(),
            "locally referenced export should not be marked dead"
        );
    }

    #[test]
    fn test_find_dead_exports_treats_shell_runtime_semantics_conservatively() {
        let shell = FileAnalysis {
            path: "src/install.sh".to_string(),
            language: "shell".to_string(),
            exports: vec![
                ExportSymbol::new("usage".to_string(), "function", "named", Some(1)),
                ExportSymbol::new("PATH".to_string(), "env", "named", Some(5)),
                ExportSymbol::new("_unused_private".to_string(), "function", "named", Some(9)),
            ],
            local_uses: vec!["usage".to_string()],
            ..Default::default()
        };

        let result = find_dead_exports(&[shell], true, None, DeadFilterConfig::default());

        assert!(!result.iter().any(|dead| dead.symbol == "usage"));
        assert!(!result.iter().any(|dead| dead.symbol == "PATH"));
        let private_helper = result
            .iter()
            .find(|dead| dead.symbol == "_unused_private")
            .expect("unused private shell helper should still be reported");
        assert_eq!(private_helper.confidence, "medium");
    }

    #[test]
    fn test_find_dead_exports_respects_sourced_shell_public_api() {
        let library = FileAnalysis {
            path: "lib/common.sh".to_string(),
            language: "shell".to_string(),
            exports: vec![
                ExportSymbol::new("log_info".to_string(), "function", "named", Some(1)),
                ExportSymbol::new("_private_helper".to_string(), "function", "named", Some(5)),
            ],
            ..Default::default()
        };

        let mut source_import = ImportEntry::new("./lib/common.sh".to_string(), ImportKind::Static);
        source_import.resolved_path = Some("lib/common.sh".to_string());
        let script = FileAnalysis {
            path: "src/install.sh".to_string(),
            language: "shell".to_string(),
            imports: vec![source_import],
            ..Default::default()
        };

        let result = find_dead_exports(&[library, script], true, None, DeadFilterConfig::default());

        assert!(
            !result.iter().any(|dead| dead.symbol == "log_info"),
            "public functions in sourced shell libraries are API surface"
        );
        assert!(
            result.iter().any(|dead| dead.symbol == "_private_helper"),
            "private sourced helpers may still be reported when unreachable"
        );
    }

    #[test]
    fn test_find_dead_exports_skips_makefile_targets() {
        let makefile = FileAnalysis {
            path: "Makefile".to_string(),
            language: "make".to_string(),
            exports: vec![
                ExportSymbol::new("help".to_string(), "target", "named", Some(1)),
                ExportSymbol::new(".PHONY".to_string(), "special_target", "named", Some(3)),
                ExportSymbol::new("VERSION".to_string(), "var", "named", Some(5)),
            ],
            ..Default::default()
        };

        let result = find_dead_exports(&[makefile], true, None, DeadFilterConfig::default());

        assert!(
            result.is_empty(),
            "Make targets and variables are build-runtime declarations, not dead exports"
        );
    }

    #[test]
    fn test_find_dead_exports_swift_credits_local_and_cross_file_refs() {
        // Swift `import` resolves a MODULE, never a symbol, so import-edge
        // absence flagged nearly every declaration (~2340 FPs in the field).
        // The fix credits two reference signals the analyzer now emits:
        //   - local_uses: same-file references to own declarations
        //   - symbol_usages: identifiers a file references (→ cross-file)
        // Only a declaration referenced NOWHERE stays a candidate.
        let model = FileAnalysis {
            path: "Sources/App/model.swift".to_string(),
            language: "swift".to_string(),
            exports: vec![
                ExportSymbol::new("DocumentSession".to_string(), "struct", "named", Some(1)),
                ExportSymbol::new("documentTitle".to_string(), "var", "named", Some(2)),
                ExportSymbol::new("normalize".to_string(), "func", "named", Some(5)),
                ExportSymbol::new(
                    "AbandonedScratchModel".to_string(),
                    "struct",
                    "named",
                    Some(9),
                ),
            ],
            // normalize() and documentTitle are read by a sibling method (lines
            // other than their declaration) — same-file use.
            local_uses: vec!["normalize".to_string(), "documentTitle".to_string()],
            ..Default::default()
        };
        let view = FileAnalysis {
            path: "Sources/App/view.swift".to_string(),
            language: "swift".to_string(),
            exports: vec![ExportSymbol::new(
                "DocumentView".to_string(),
                "struct",
                "named",
                Some(1),
            )],
            // view.swift references DocumentSession across files.
            symbol_usages: vec![crate::types::SymbolUsage {
                name: "DocumentSession".to_string(),
                line: 2,
                context: "let session: DocumentSession".to_string(),
            }],
            ..Default::default()
        };

        let result = find_dead_exports(&[model, view], false, None, DeadFilterConfig::default());
        let dead: Vec<&str> = result.iter().map(|d| d.symbol.as_str()).collect();

        assert!(
            !dead.contains(&"DocumentSession"),
            "cross-file referenced Swift symbol must not be dead; got {:?}",
            dead
        );
        assert!(
            !dead.contains(&"normalize") && !dead.contains(&"documentTitle"),
            "same-file-used Swift symbols must not be dead; got {:?}",
            dead
        );
        assert!(
            dead.contains(&"AbandonedScratchModel"),
            "a Swift declaration referenced nowhere must still be flagged; got {:?}",
            dead
        );
    }

    #[test]
    fn test_find_dead_exports_respects_type_imports() {
        let exporter = mock_file_with_exports("client/actions.ts", vec!["Action"]);
        let mut importer = mock_file("client/state.ts");
        let mut imp = ImportEntry::new("client/actions".to_string(), ImportKind::Type);
        imp.resolved_path = Some("client/actions.ts".to_string());
        imp.symbols.push(ImportSymbol {
            name: "Action".to_string(),
            alias: None,
            is_default: false,
        });
        importer.imports.push(imp);

        let result = find_dead_exports(
            &[importer, exporter],
            true,
            None,
            DeadFilterConfig::default(),
        );
        assert!(
            result.is_empty(),
            "type-only import should count as usage for dead export detection"
        );
    }

    #[test]
    fn test_find_dead_exports_cross_extension_match() {
        let exporter = mock_file_with_exports("src/ComboBox.tsx", vec!["ComboBox"]);
        let mut importer = mock_file("src/app.js");
        let mut imp = ImportEntry::new("./ComboBox".to_string(), ImportKind::Static);
        imp.resolved_path = Some("src/ComboBox.tsx".to_string());
        imp.symbols.push(ImportSymbol {
            name: "ComboBox".to_string(),
            alias: None,
            is_default: false,
        });
        importer.imports.push(imp);

        let result = find_dead_exports(
            &[importer, exporter],
            false,
            None,
            DeadFilterConfig::default(),
        );
        assert!(
            result.is_empty(),
            "imports across JS/TSX extensions should prevent dead export marking"
        );
    }

    #[test]
    fn test_find_dead_exports_respects_crate_imports() {
        let exporter = mock_file_with_exports("src/ui/constants.rs", vec!["MENU_GAP"]);
        let mut importer = mock_file("src/main.rs");
        let mut imp = ImportEntry::new(
            "crate::ui::constants::MENU_GAP".to_string(),
            ImportKind::Static,
        );
        imp.raw_path = "crate::ui::constants::MENU_GAP".to_string();
        imp.is_crate_relative = true;
        imp.symbols.push(ImportSymbol {
            name: "MENU_GAP".to_string(),
            alias: None,
            is_default: false,
        });
        importer.imports.push(imp);

        let result = find_dead_exports(
            &[importer, exporter],
            false,
            None,
            DeadFilterConfig::default(),
        );
        assert!(
            result.is_empty(),
            "crate-internal imports should prevent dead export marking. Found: {:?}",
            result
        );
    }

    #[test]
    fn test_find_dead_exports_respects_super_imports() {
        let exporter = mock_file_with_exports("src/types.rs", vec!["Config"]);
        let mut importer = mock_file("src/ui/widget.rs");
        let mut imp = ImportEntry::new("super::types::Config".to_string(), ImportKind::Static);
        imp.raw_path = "super::types::Config".to_string();
        imp.is_super_relative = true;
        imp.symbols.push(ImportSymbol {
            name: "Config".to_string(),
            alias: None,
            is_default: false,
        });
        importer.imports.push(imp);

        let result = find_dead_exports(
            &[importer, exporter],
            false,
            None,
            DeadFilterConfig::default(),
        );
        assert!(
            result.is_empty(),
            "super:: imports should prevent dead export marking. Found: {:?}",
            result
        );
    }

    #[test]
    fn test_crate_import_matches_file_basic() {
        // Test basic crate:: import matching
        assert!(
            crate_import_matches_file(
                "crate::ui::constants::MENU_GAP",
                "src/ui/constants.rs",
                "MENU_GAP"
            ),
            "should match crate::ui::constants with src/ui/constants.rs"
        );

        assert!(
            crate_import_matches_file("crate::types::Config", "src/types.rs", "Config"),
            "should match crate::types with src/types.rs"
        );

        assert!(
            crate_import_matches_file("super::utils::helper", "utils.rs", "helper"),
            "should match super::utils with utils.rs"
        );

        // Test non-matches
        assert!(
            !crate_import_matches_file("crate::ui::constants::X", "src/ui/layout.rs", "X"),
            "should NOT match constants with layout.rs"
        );

        assert!(
            !crate_import_matches_file("external::package::Foo", "src/foo.rs", "Foo"),
            "should NOT match non-crate imports"
        );
    }

    #[test]
    fn test_print_symbol_results_no_matches() {
        let result = SymbolSearchResult {
            found: false,
            total_matches: 0,
            files: vec![],
        };
        // Should not panic
        print_symbol_results("foo", &result, false);
        print_symbol_results("foo", &result, true);
    }

    #[test]
    fn test_print_symbol_results_with_matches() {
        let result = SymbolSearchResult {
            found: true,
            total_matches: 1,
            files: vec![SymbolFileMatch {
                file: "src/utils.ts".to_string(),
                matches: vec![SymbolMatch {
                    line: 10,
                    context: "const foo = 1".to_string(),
                    is_definition: true,
                    kind: SymbolMatchKind::Definition,
                }],
            }],
        };
        // Should not panic
        print_symbol_results("foo", &result, false);
        print_symbol_results("foo", &result, true);
    }

    #[test]
    fn test_find_similar_empty() {
        let analyses: Vec<FileAnalysis> = vec![];
        let result = find_similar("Button", &analyses);
        assert!(result.is_empty());
    }

    #[test]
    fn test_find_similar_by_path() {
        let analyses = vec![mock_file("Button.tsx"), mock_file("src/utils/helpers.ts")];
        let result = find_similar("Button", &analyses);
        // Path similarity is computed against full path - shorter path gives higher score
        assert!(!result.is_empty());
        assert!(result.iter().any(|c| c.symbol.contains("Button")));
    }

    #[test]
    fn test_find_similar_by_export() {
        let analyses = vec![mock_file_with_exports(
            "src/utils.ts",
            vec!["useButton", "formatDate"],
        )];
        let result = find_similar("Button", &analyses);
        assert!(result.iter().any(|c| c.symbol == "useButton"));
    }

    #[test]
    fn test_print_similarity_results_empty() {
        let candidates: Vec<SimilarityCandidate> = vec![];
        // Should not panic
        print_similarity_results("foo", &candidates, false);
        print_similarity_results("foo", &candidates, true);
    }

    #[test]
    fn test_print_similarity_results_with_matches() {
        let candidates = vec![SimilarityCandidate {
            symbol: "fooBar".to_string(),
            file: "export in src/utils.ts".to_string(),
            score: 0.8,
            line: Some(42),
        }];
        // Should not panic
        print_similarity_results("foo", &candidates, false);
        print_similarity_results("foo", &candidates, true);
    }

    #[test]
    fn test_find_dead_exports_empty() {
        let analyses: Vec<FileAnalysis> = vec![];
        let result = find_dead_exports(&analyses, false, None, DeadFilterConfig::default());
        assert!(result.is_empty());
    }

    #[test]
    fn test_find_dead_exports_all_used() {
        let mut importer = mock_file("src/app.ts");
        importer.imports = vec![{
            let mut imp = ImportEntry::new("./utils".to_string(), ImportKind::Static);
            imp.resolved_path = Some("src/utils.ts".to_string());
            imp.symbols = vec![ImportSymbol {
                name: "helper".to_string(),
                alias: None,
                is_default: false,
            }];
            imp
        }];

        let exporter = mock_file_with_exports("src/utils.ts", vec!["helper"]);

        let analyses = vec![importer, exporter];
        let result = find_dead_exports(&analyses, false, None, DeadFilterConfig::default());
        assert!(result.is_empty());
    }

    #[test]
    fn test_find_dead_exports_unused() {
        let analyses = vec![
            mock_file("src/app.ts"),
            mock_file_with_exports("src/utils.ts", vec!["unusedHelper"]),
        ];
        let result = find_dead_exports(&analyses, false, None, DeadFilterConfig::default());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].symbol, "unusedHelper");
    }

    #[test]
    fn test_find_dead_exports_dead_ok_glob_suppresses() {
        let analyses = vec![
            mock_file("src/app.ts"),
            mock_file_with_exports("src/generated/utils.ts", vec!["unusedHelper"]),
        ];
        let result = find_dead_exports(
            &analyses,
            false,
            None,
            DeadFilterConfig {
                include_tests: false,
                include_helpers: false,
                library_mode: false,
                example_globs: Vec::new(),
                python_library_mode: false,
                include_ambient: false,
                include_dynamic: false,
                dead_ok_globs: vec!["src/generated/**".to_string()],
            },
        );
        assert!(
            result.is_empty(),
            "dead-ok glob should suppress dead exports for matching files: {:?}",
            result
        );
    }

    #[test]
    fn test_find_dead_exports_skips_tests() {
        let mut test_file =
            mock_file_with_exports("src/__tests__/utils.test.ts", vec!["testHelper"]);
        test_file.is_test = true;

        let analyses = vec![mock_file("src/app.ts"), test_file];
        let result = find_dead_exports(&analyses, false, None, DeadFilterConfig::default());
        assert!(result.is_empty());
    }

    #[test]
    fn test_find_dead_exports_includes_tests_when_requested() {
        let mut test_file =
            mock_file_with_exports("src/__tests__/utils.test.ts", vec!["testHelper"]);
        test_file.is_test = true;

        let analyses = vec![mock_file("src/app.ts"), test_file];
        let result = find_dead_exports(
            &analyses,
            false,
            None,
            DeadFilterConfig {
                include_tests: true,
                include_helpers: false,
                library_mode: false,
                example_globs: Vec::new(),
                python_library_mode: false,
                include_ambient: false,
                include_dynamic: false,
                dead_ok_globs: Vec::new(),
            },
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].symbol, "testHelper");
    }

    #[test]
    fn test_find_dead_exports_skips_helpers_by_default() {
        let helper = mock_file_with_exports("scripts/cleanup.py", vec!["orphan"]);
        let analyses = vec![mock_file("src/app.ts"), helper];
        let result = find_dead_exports(&analyses, false, None, DeadFilterConfig::default());
        assert!(result.is_empty(), "helper scripts should be skipped");
    }

    #[test]
    fn test_find_dead_exports_skips_shell_operator_glue_by_default() {
        for path in [
            "ai-hooks/loct-grep-augment.sh",
            "loctree-plugin/hooks/loct-grep-augment.sh",
            "loctree-rs/completions/loct.bash",
        ] {
            let mut helper = mock_file_with_exports(path, vec!["augment_who_imports"]);
            helper.language = "shell".to_string();

            let result = find_dead_exports(
                &[mock_file("src/app.ts"), helper],
                true,
                None,
                DeadFilterConfig::default(),
            );
            assert!(
                result.is_empty(),
                "shell operator glue should not produce dead-export findings for {path}: {result:?}"
            );
        }
    }

    #[test]
    fn test_find_dead_exports_skips_jsx_runtime_files() {
        // JSX runtime files should be completely skipped from dead export detection
        let mut jsx_runtime = mock_file_with_exports(
            "packages/solid-js/jsx-runtime/index.ts",
            vec!["jsx", "jsxs", "jsxDEV", "Fragment"],
        );
        jsx_runtime.language = "ts".to_string();

        let result = find_dead_exports(&[jsx_runtime], false, None, DeadFilterConfig::default());
        assert!(
            result.is_empty(),
            "JSX runtime files should be completely skipped: {:?}",
            result
        );
    }

    #[test]
    fn test_find_dead_exports_skips_jsx_runtime_exports() {
        // Individual JSX runtime exports (jsx, jsxs, Fragment) in jsx-runtime paths should not be flagged
        let mut runtime_file = mock_file_with_exports(
            "node_modules/solid-js/jsx-runtime.js",
            vec!["jsx", "jsxs", "jsxDEV", "Fragment", "createComponent"],
        );
        runtime_file.language = "js".to_string();

        let result = find_dead_exports(&[runtime_file], false, None, DeadFilterConfig::default());
        // File should be skipped entirely due to jsx-runtime path pattern
        assert!(
            result.is_empty(),
            "JSX runtime exports should not be flagged as dead: {:?}",
            result
        );
    }

    #[test]
    fn test_jsx_runtime_export_detection() {
        // Test the helper function directly
        assert!(is_jsx_runtime_export(
            "jsx",
            "packages/solid-js/jsx-runtime/index.ts"
        ));
        assert!(is_jsx_runtime_export("jsxs", "vue/jsx-runtime.js"));
        assert!(is_jsx_runtime_export("jsxDEV", "react/jsx-dev-runtime.js"));
        assert!(is_jsx_runtime_export(
            "Fragment",
            "preact/jsx-runtime/index.mjs"
        ));
        assert!(is_jsx_runtime_export("jsxsDEV", "solid/jsx_runtime.ts"));

        // Non JSX runtime exports should not match
        assert!(!is_jsx_runtime_export("Component", "jsx-runtime/index.ts"));
        assert!(!is_jsx_runtime_export("jsx", "src/utils/helpers.ts"));
        assert!(!is_jsx_runtime_export("createElement", "jsx-runtime.js"));
    }

    #[test]
    fn test_find_dead_exports_skips_vscode_extension_activate_deactivate() {
        // editors/vscode/src/extension.ts exports activate/deactivate that are
        // invoked by the VSCode extension host via Node require() — never
        // appearing in any import statement. Reporting them as "very-high
        // confidence: remove" was the canonical truth-of-findings lie.
        let extension = mock_file_with_exports(
            "editors/vscode/src/extension.ts",
            vec!["activate", "deactivate"],
        );

        let result = find_dead_exports(&[extension], false, None, DeadFilterConfig::default());
        assert!(
            !result.iter().any(|d| d.symbol == "activate"),
            "VSCode activate hook must not be flagged dead: {:?}",
            result
        );
        assert!(
            !result.iter().any(|d| d.symbol == "deactivate"),
            "VSCode deactivate hook must not be flagged dead: {:?}",
            result
        );
    }

    #[test]
    fn test_find_dead_exports_extension_filter_only_lifecycle_names() {
        // The filter must be narrow: a helper unrelated to VSCode in the same
        // extension.ts file is still a legitimate dead-export candidate.
        let extension = mock_file_with_exports(
            "editors/vscode/src/extension.ts",
            vec!["activate", "deactivate", "internalHelper"],
        );

        let result = find_dead_exports(&[extension], false, None, DeadFilterConfig::default());
        // activate/deactivate suppressed, internalHelper still surfaces if truly dead
        assert!(!result.iter().any(|d| d.symbol == "activate"));
        assert!(!result.iter().any(|d| d.symbol == "deactivate"));
        assert!(
            result.iter().any(|d| d.symbol == "internalHelper"),
            "non-lifecycle exports in extension.ts must still be reportable: {:?}",
            result
        );
    }

    #[test]
    fn test_find_dead_exports_extension_filter_only_extension_path() {
        // The filter must be narrow on path too: a random module exporting an
        // `activate` symbol from a non-extension file is still a candidate.
        let random = mock_file_with_exports("src/lifecycle/manager.ts", vec!["activate"]);

        let result = find_dead_exports(&[random], false, None, DeadFilterConfig::default());
        assert!(
            result.iter().any(|d| d.symbol == "activate"),
            "activate() in non-extension paths must still be reportable: {:?}",
            result
        );
    }

    #[test]
    fn test_find_dead_exports_high_confidence_skips_default() {
        let analyses = vec![
            mock_file("src/app.ts"),
            mock_file_with_exports("src/utils.ts", vec!["default", "helper"]),
        ];
        let result = find_dead_exports(&analyses, true, None, DeadFilterConfig::default());
        assert!(!result.iter().any(|d| d.symbol == "default"));
    }

    #[test]
    fn test_find_dead_exports_skips_dynamic_import_without_extension() {
        let mut importer = mock_file("src/app.tsx");
        importer.dynamic_imports = vec!["./utils".to_string()];

        let exporter = mock_file_with_exports("src/utils/index.ts", vec!["foo"]);

        let result = find_dead_exports(
            &[importer, exporter],
            false,
            None,
            DeadFilterConfig::default(),
        );
        assert!(
            result.is_empty(),
            "dynamic import should mark module as used"
        );
    }

    #[test]
    fn test_dynamic_import_with_alias_prefix_marks_reachable() {
        let mut importer = mock_file("src/app.ts");
        importer.dynamic_imports = vec!["@core/utils".to_string()];

        let exporter = mock_file_with_exports("src/core/utils/index.ts", vec!["helper"]);

        let result = find_dead_exports(
            &[importer, exporter],
            false,
            None,
            DeadFilterConfig::default(),
        );
        assert!(
            result.is_empty(),
            "alias-prefixed dynamic import should keep target reachable"
        );
    }

    #[test]
    fn test_find_dead_exports_counts_default_import_usage() {
        let mut importer = mock_file("src/app.ts");
        importer.imports = vec![{
            let mut imp = ImportEntry::new("./utils".to_string(), ImportKind::Static);
            imp.resolved_path = Some("src/utils.ts".to_string());
            imp.symbols = vec![ImportSymbol {
                name: "AliasDefault".to_string(),
                alias: None,
                is_default: true,
            }];
            imp
        }];

        let mut exporter = mock_file_with_exports("src/utils.ts", vec!["default"]);
        exporter.exports[0].kind = "default".to_string();
        exporter.exports[0].export_type = "default".to_string();

        let result = find_dead_exports(
            &[importer, exporter],
            false,
            None,
            DeadFilterConfig::default(),
        );
        assert!(
            result.is_empty(),
            "default import should mark export as used"
        );
    }

    #[test]
    fn test_react_lazy_default_export_not_dead() {
        // React lazy() pattern: const Component = lazy(() => import('./Component'))
        // This dynamically imports the default export, so default should NOT be dead
        let mut importer = mock_file("src/App.tsx");
        importer.dynamic_imports = vec!["./PasswordResetModal".to_string()];

        let mut exporter = mock_file("src/PasswordResetModal.tsx");
        exporter.exports = vec![ExportSymbol {
            name: "PasswordResetModal".to_string(),
            kind: "function".to_string(),
            export_type: "default".to_string(),
            line: Some(23),
            params: Vec::new(),

            symbol_id: crate::types::SymbolIdV1::default(),
        }];

        let result = find_dead_exports(
            &[importer, exporter],
            false,
            None,
            DeadFilterConfig::default(),
        );
        assert!(
            result.is_empty(),
            "React lazy() default export should NOT be marked as dead: {:?}",
            result
        );
    }

    #[test]
    fn test_react_lazy_with_subdirectory_resolved_path() {
        // example-app pattern: lazy(() => import('./features/settings/PasswordResetModal').then(...))
        // Component is in a subdirectory, import uses resolved_path via ImportKind::Dynamic
        let mut importer = mock_file("src/App.tsx");
        // Add dynamic import via ImportEntry with resolved_path (like real AST produces)
        let mut dyn_import = ImportEntry::new(
            "./features/settings/PasswordResetModal".to_string(),
            ImportKind::Dynamic,
        );
        dyn_import.resolved_path = Some("src/features/settings/PasswordResetModal.tsx".to_string());
        importer.imports.push(dyn_import);
        // Also add to dynamic_imports for backward compat
        importer.dynamic_imports = vec!["./features/settings/PasswordResetModal".to_string()];

        let mut exporter = mock_file("src/features/settings/PasswordResetModal.tsx");
        exporter.exports = vec![ExportSymbol {
            name: "PasswordResetModal".to_string(),
            kind: "function".to_string(),
            export_type: "default".to_string(),
            line: Some(23),
            params: Vec::new(),

            symbol_id: crate::types::SymbolIdV1::default(),
        }];

        let result = find_dead_exports(
            &[importer, exporter],
            false,
            None,
            DeadFilterConfig::default(),
        );
        assert!(
            result.is_empty(),
            "React lazy() with resolved_path in subdirectory should NOT be marked as dead: {:?}",
            result
        );
    }

    #[test]
    fn test_react_lazy_named_export_via_then_pattern() {
        // example-app pattern: lazy(() => import('./X').then((m) => ({ default: m.ComponentName })))
        // This extracts a NAMED export and re-wraps it as default for React.lazy()
        // The file has NAMED export (not default), but it's still used via dynamic import
        let mut importer = mock_file("src/App.tsx");
        let mut dyn_import =
            ImportEntry::new("./features/DashboardView".to_string(), ImportKind::Dynamic);
        dyn_import.resolved_path = Some("src/features/DashboardView.tsx".to_string());
        importer.imports.push(dyn_import);

        let mut exporter = mock_file("src/features/DashboardView.tsx");
        // Note: export_type is "named", not "default" - the .then() pattern wraps it
        exporter.exports = vec![ExportSymbol {
            name: "DashboardView".to_string(),
            kind: "function".to_string(),
            export_type: "named".to_string(),
            line: Some(15),
            params: Vec::new(),

            symbol_id: crate::types::SymbolIdV1::default(),
        }];

        let result = find_dead_exports(
            &[importer, exporter],
            false,
            None,
            DeadFilterConfig::default(),
        );
        // The ENTIRE FILE should be skipped when it's dynamically imported
        // because we can't know which exports the .then() pattern extracts
        assert!(
            result.is_empty(),
            "Dynamic import with .then() pattern should skip entire file: {:?}",
            result
        );
    }

    #[test]
    fn test_find_dead_exports_skips_reexport_bindings() {
        let mut barrel = mock_file_with_exports("src/index.ts", vec!["Foo"]);
        if let Some(first) = barrel.exports.first_mut() {
            first.kind = "reexport".to_string();
        }
        barrel.reexports.push(ReexportEntry {
            source: "./foo".to_string(),
            kind: ReexportKind::Named(vec![("Foo".to_string(), "Foo".to_string())]),
            resolved: Some("src/foo.ts".to_string()),
        });

        let result = find_dead_exports(&[barrel], false, None, DeadFilterConfig::default());
        assert!(
            result.is_empty(),
            "reexport-only barrels should not be reported as dead exports"
        );
    }

    #[test]
    fn test_print_dead_exports_json() {
        let dead = vec![DeadExport {
            file: "src/utils.ts".to_string(),
            symbol: "unused".to_string(),
            line: Some(10),
            confidence: "high".to_string(),
            reason: "No imports found for 'unused'. Checked: resolved imports (0 matches), star re-exports (none), local references (none)".to_string(),
            open_url: Some("loctree://open?f=src%2Futils.ts&l=10".to_string()),
            is_test: false,
            action: "delete_candidate".to_string(),
            entrypoint: false,
        }];
        // Should not panic
        print_dead_exports(&dead, OutputMode::Json, false, 20);
    }

    #[test]
    fn test_print_dead_exports_human() {
        let dead = vec![DeadExport {
            file: "src/utils.ts".to_string(),
            symbol: "unused".to_string(),
            line: None,
            confidence: "high".to_string(),
            reason: "No imports found for 'unused'. Checked: resolved imports (0 matches), star re-exports (none), local references (none)".to_string(),
            open_url: None,
            is_test: false,
            action: "delete_candidate".to_string(),
            entrypoint: false,
        }];
        // Should not panic
        print_dead_exports(&dead, OutputMode::Human, false, 20);
        print_dead_exports(&dead, OutputMode::Human, true, 20);
    }

    #[test]
    fn test_print_dead_exports_many() {
        let dead: Vec<DeadExport> = (0..60)
            .map(|i| DeadExport {
                file: format!("src/file{}.ts", i),
                symbol: format!("unused{}", i),
                line: Some(i),
                confidence: "high".to_string(),
                reason: format!("No imports found for 'unused{}'. Checked: resolved imports (0 matches), star re-exports (none), local references (none)", i),
                open_url: Some(format!("loctree://open?f=src%2Ffile{}.ts&l={}", i, i)),
                is_test: false,
            action: "delete_candidate".to_string(),
            entrypoint: false,
            })
            .collect();
        // Should truncate to limit and show "... and N more"
        print_dead_exports(&dead, OutputMode::Human, false, 50);
    }

    #[test]
    fn test_django_wagtail_mixin_not_dead() {
        // Test that Django/Wagtail mixins used in inheritance are not marked as dead
        // This tests the integration between py.rs (which tracks inheritance) and dead_parrots.rs

        use crate::types::{ExportSymbol, FileAnalysis};

        // Mixin definition file
        let mixin_file = FileAnalysis {
            path: "myapp/mixins.py".to_string(),
            language: "py".to_string(),
            exports: vec![
                ExportSymbol::new("LoginRequiredMixin".to_string(), "class", "named", Some(1)),
                ExportSymbol::new(
                    "PermissionRequiredMixin".to_string(),
                    "class",
                    "named",
                    Some(5),
                ),
                ExportSymbol::new("ButtonsColumnMixin".to_string(), "class", "named", Some(10)),
            ],
            ..Default::default()
        };

        // View file that uses the mixins
        let mut view_file = FileAnalysis {
            path: "myapp/views.py".to_string(),
            language: "py".to_string(),
            exports: vec![], // No exports to avoid noise
            // Simulate what py.rs does: add base classes to local_uses
            local_uses: vec![
                "LoginRequiredMixin".to_string(),
                "PermissionRequiredMixin".to_string(),
                "ButtonsColumnMixin".to_string(),
            ],
            ..Default::default()
        };

        // Add import entry to track the relationship
        use crate::types::{ImportEntry, ImportKind, ImportSymbol};
        let mut imp = ImportEntry::new("myapp.mixins".to_string(), ImportKind::Static);
        imp.resolved_path = Some("myapp/mixins.py".to_string());
        imp.symbols = vec![
            ImportSymbol {
                name: "LoginRequiredMixin".to_string(),
                alias: None,
                is_default: false,
            },
            ImportSymbol {
                name: "PermissionRequiredMixin".to_string(),
                alias: None,
                is_default: false,
            },
            ImportSymbol {
                name: "ButtonsColumnMixin".to_string(),
                alias: None,
                is_default: false,
            },
        ];
        view_file.imports.push(imp);

        let analyses = vec![mixin_file, view_file];
        let result = find_dead_exports(&analyses, false, None, DeadFilterConfig::default());

        // All mixins should be marked as used (both via imports AND local_uses from inheritance tracking)
        assert!(
            result.is_empty(),
            "Django/Wagtail mixins should not be marked as dead. Found dead: {:?}",
            result
        );
    }

    #[test]
    fn test_django_mixin_pattern_common_names() {
        // Test common Django/Wagtail mixin naming patterns
        // These should not be flagged as dead even if static analysis misses some usage
        use crate::types::{ExportSymbol, FileAnalysis};

        let mixin_file = FileAnalysis {
            path: "django/contrib/auth/mixins.py".to_string(),
            language: "py".to_string(),
            exports: vec![
                // Standard Django mixins that are ALWAYS used via MRO, never called directly
                ExportSymbol::new("LoginRequiredMixin".to_string(), "class", "named", Some(1)),
                ExportSymbol::new(
                    "PermissionRequiredMixin".to_string(),
                    "class",
                    "named",
                    Some(10),
                ),
                ExportSymbol::new(
                    "UserPassesTestMixin".to_string(),
                    "class",
                    "named",
                    Some(20),
                ),
                // Non-mixin class (should be flagged if unused)
                ExportSymbol::new("AuthHelper".to_string(), "class", "named", Some(30)),
            ],
            ..Default::default()
        };

        // No imports - testing heuristic fallback for common Django patterns
        let analyses = vec![mixin_file];
        let result = find_dead_exports(&analyses, false, None, DeadFilterConfig::default());

        // Mixins ending in "Mixin" should NOT be flagged (heuristic protection)
        let mixin_names: Vec<_> = result
            .iter()
            .filter(|d| d.symbol.ends_with("Mixin"))
            .collect();
        assert!(
            mixin_names.is_empty(),
            "Classes ending in 'Mixin' should not be flagged as dead (Django/Wagtail pattern). Found: {:?}",
            mixin_names
        );

        // Non-mixin classes (like AuthHelper) SHOULD be flagged if truly unused
        let has_non_mixin = result.iter().any(|d| d.symbol == "AuthHelper");
        assert!(
            has_non_mixin,
            "Non-mixin classes like 'AuthHelper' should still be flagged when unused"
        );
    }

    #[test]
    fn test_weakmap_registry_skips_dead_exports() {
        // Test that exports in files with WeakMap/WeakSet are not marked as dead
        // This handles React DevTools and similar code where exports are stored dynamically

        let weakmap_file = FileAnalysis {
            path: "src/devtools.ts".to_string(),
            language: "ts".to_string(),
            has_weak_collections: true, // File contains new WeakMap() or new WeakSet()
            exports: vec![
                ExportSymbol::new(
                    "registerComponent".to_string(),
                    "function",
                    "named",
                    Some(10),
                ),
                ExportSymbol::new(
                    "getComponentData".to_string(),
                    "function",
                    "named",
                    Some(20),
                ),
            ],
            ..Default::default()
        };

        // Simulate that these exports are NOT imported anywhere (would normally be dead)
        // But they should not be flagged because the file has WeakMap/WeakSet

        let analyses = vec![weakmap_file];
        let result = find_dead_exports(&analyses, false, None, DeadFilterConfig::default());

        assert!(
            result.is_empty(),
            "Exports in files with WeakMap/WeakSet should NOT be flagged as dead. Found: {:?}",
            result
        );
    }

    #[test]
    fn test_paths_match_exact() {
        assert!(paths_match("src/App.tsx", "src/App.tsx"));
        assert!(paths_match("foo.ts", "foo.ts"));
    }

    #[test]
    fn test_paths_match_with_separators() {
        // Should handle different separators
        assert!(paths_match("src/App.tsx", "src\\App.tsx"));
        assert!(paths_match(
            "src\\components\\Button.tsx",
            "src/components/Button.tsx"
        ));
    }

    #[test]
    fn test_paths_match_normalizes_index_and_extension() {
        assert!(paths_match("src/utils/index.ts", "./utils"));
        assert!(paths_match("src/components/Foo.tsx", "src/components/Foo"));
        assert!(paths_match("components/Foo.tsx", "components/Foo.jsx"));
    }

    #[test]
    fn test_paths_match_suffix() {
        // Should match when one is a suffix of another at component boundary
        assert!(paths_match("src/App.tsx", "App.tsx"));
        assert!(paths_match("src/components/Button.tsx", "Button.tsx"));
        assert!(paths_match("Button.tsx", "src/components/Button.tsx"));
    }

    #[test]
    fn test_paths_match_no_false_positives() {
        // Should NOT match foo.ts with foo.test.ts (this is the critical fix)
        assert!(!paths_match("foo.ts", "foo.test.ts"));
        assert!(!paths_match("Button.tsx", "Button.test.tsx"));
        assert!(!paths_match("utils.ts", "utils.spec.ts"));

        // Should NOT match when substring is in the middle
        assert!(!paths_match("App.tsx", "src/MyApp.tsx"));
        assert!(!paths_match("Button.tsx", "src/BigButton.tsx"));
    }

    #[test]
    fn test_python_stdlib_exports_not_dead() {
        // Test that CPython stdlib exports in __all__ are not marked as dead
        // This addresses the 100% FP rate on python/cpython smoke test

        // Simulate calendar.py module with APRIL constant in __all__
        let calendar_module = FileAnalysis {
            path: "Lib/calendar.py".to_string(),
            language: "py".to_string(),
            exports: vec![
                ExportSymbol::new("APRIL".to_string(), "__all__", "named", Some(1)),
                ExportSymbol::new("APRIL".to_string(), "const", "named", Some(10)),
            ],
            ..Default::default()
        };

        // Simulate csv.py module with DictWriter in __all__
        let csv_module = FileAnalysis {
            path: "Lib/csv.py".to_string(),
            language: "py".to_string(),
            exports: vec![
                ExportSymbol::new("DictWriter".to_string(), "__all__", "named", Some(1)),
                ExportSymbol::new("DictWriter".to_string(), "class", "named", Some(50)),
            ],
            ..Default::default()
        };

        // Simulate typing.py module with override in __all__
        let typing_module = FileAnalysis {
            path: "Lib/typing.py".to_string(),
            language: "py".to_string(),
            exports: vec![
                ExportSymbol::new("override".to_string(), "__all__", "named", Some(1)),
                ExportSymbol::new("override".to_string(), "function", "named", Some(200)),
            ],
            ..Default::default()
        };

        let analyses = vec![calendar_module, csv_module, typing_module];

        // Run dead export detection with python_library_mode enabled
        let dead_exports = find_dead_exports(
            &analyses,
            false,
            None,
            DeadFilterConfig {
                include_tests: false,
                include_helpers: false,
                library_mode: false,
                example_globs: Vec::new(),
                python_library_mode: true, // Enable Python library mode
                include_ambient: false,
                include_dynamic: false,
                dead_ok_globs: Vec::new(),
            },
        );

        // Verify that NONE of these stdlib exports are marked as dead
        // They're all in __all__ lists and are public API for millions of Python programs
        assert!(
            dead_exports.is_empty(),
            "CPython stdlib exports in __all__ should NOT be marked as dead. Found: {:?}",
            dead_exports
        );
    }

    #[test]
    fn test_python_stdlib_uppercase_constants_not_dead() {
        // Test that UPPER_CASE constants in stdlib are treated as public API
        // even if not in __all__ (some stdlib modules don't have explicit __all__)

        let module = FileAnalysis {
            path: "Lib/socket.py".to_string(),
            language: "py".to_string(),
            exports: vec![
                ExportSymbol::new("AF_INET".to_string(), "const", "named", Some(10)),
                ExportSymbol::new("SOCK_STREAM".to_string(), "const", "named", Some(20)),
            ],
            ..Default::default()
        };

        let analyses = vec![module];

        let dead_exports = find_dead_exports(
            &analyses,
            false,
            None,
            DeadFilterConfig {
                include_tests: false,
                include_helpers: false,
                library_mode: false,
                example_globs: Vec::new(),
                python_library_mode: true,
                include_ambient: false,
                include_dynamic: false,
                dead_ok_globs: Vec::new(),
            },
        );

        // UPPER_CASE constants in stdlib should not be marked as dead
        assert!(
            dead_exports.is_empty(),
            "CPython stdlib UPPER_CASE constants should NOT be dead. Found: {:?}",
            dead_exports
        );
    }

    #[test]
    fn test_shadow_export_detection() {
        // Test shadow export detection: same symbol exported by multiple files, only one used
        // Pattern: stores/conversationHostStore.ts exports conversationHostStore (DEAD)
        //          aiStore/slices/conversationHostSlice.ts exports conversationHostStore (USED)

        use crate::types::{ImportEntry, ImportKind, ImportSymbol};

        // Old file that exports conversationHostStore (361 LOC) - will be DEAD
        let old_store = FileAnalysis {
            path: "stores/conversationHostStore.ts".to_string(),
            language: "ts".to_string(),
            loc: 361,
            exports: vec![ExportSymbol::new(
                "conversationHostStore".to_string(),
                "const",
                "named",
                Some(42),
            )],
            ..Default::default()
        };

        // New file that exports conversationHostStore - will be USED
        let new_slice = FileAnalysis {
            path: "aiStore/slices/conversationHostSlice.ts".to_string(),
            language: "ts".to_string(),
            loc: 120,
            exports: vec![ExportSymbol::new(
                "conversationHostStore".to_string(),
                "const",
                "named",
                Some(15),
            )],
            ..Default::default()
        };

        // File that imports from the NEW location
        let mut importer = FileAnalysis {
            path: "components/Chat.tsx".to_string(),
            language: "tsx".to_string(),
            ..Default::default()
        };
        let mut imp = ImportEntry::new(
            "aiStore/slices/conversationHostSlice".to_string(),
            ImportKind::Static,
        );
        imp.resolved_path = Some("aiStore/slices/conversationHostSlice.ts".to_string());
        imp.symbols.push(ImportSymbol {
            name: "conversationHostStore".to_string(),
            alias: None,
            is_default: false,
        });
        importer.imports.push(imp);

        let analyses = vec![old_store, new_slice, importer];
        let shadows = find_shadow_exports(&analyses);

        assert_eq!(shadows.len(), 1, "Should find exactly one shadow export");

        let shadow = &shadows[0];
        assert_eq!(shadow.symbol, "conversationHostStore");
        assert_eq!(
            shadow.used_file, "aiStore/slices/conversationHostSlice.ts",
            "New file should be marked as USED"
        );
        assert_eq!(shadow.dead_files.len(), 1, "Should have one dead file");
        assert_eq!(
            shadow.dead_files[0].file, "stores/conversationHostStore.ts",
            "Old file should be marked as DEAD"
        );
        assert_eq!(
            shadow.dead_files[0].loc, 361,
            "Should track LOC of dead file"
        );
        assert_eq!(shadow.total_dead_loc, 361);
    }

    #[test]
    fn test_python_non_stdlib_requires_all() {
        // Test that non-stdlib Python files still require proper __all__ or usage
        // to avoid being marked as dead

        let user_module = FileAnalysis {
            path: "myapp/utils.py".to_string(), // NOT in Lib/
            language: "py".to_string(),
            exports: vec![ExportSymbol::new(
                "helper".to_string(),
                "function",
                "named",
                Some(10),
            )],
            ..Default::default()
        };

        let analyses = vec![user_module];

        let dead_exports = find_dead_exports(
            &analyses,
            false,
            None,
            DeadFilterConfig {
                include_tests: false,
                include_helpers: false,
                library_mode: false,
                example_globs: Vec::new(),
                python_library_mode: true,
                include_ambient: false,
                include_dynamic: false,
                dead_ok_globs: Vec::new(),
            },
        );

        // User code without __all__ or usage SHOULD be marked as dead
        assert_eq!(
            dead_exports.len(),
            1,
            "Non-stdlib exports without __all__ should be marked as dead"
        );
        assert_eq!(dead_exports[0].symbol, "helper");
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::types::{
        ExportSymbol, ImportEntry, ImportKind, ImportSymbol, ReexportEntry, ReexportKind,
    };

    #[test]
    fn test_recommendations_pdf_not_dead() {
        let mut importer = FileAnalysis {
            path: "src/services/recommendationsExportService.ts".to_string(),
            ..Default::default()
        };
        let mut imp = ImportEntry::new(
            "../components/pdf/RecommendationsPDFTemplate".to_string(),
            ImportKind::Static,
        );
        imp.resolved_path = Some("src/components/pdf/RecommendationsPDFTemplate.tsx".to_string());
        imp.symbols.push(ImportSymbol {
            name: "RecommendationsPDFTemplate".to_string(),
            alias: None,
            is_default: false,
        });
        importer.imports.push(imp);

        let exporter = FileAnalysis {
            path: "src/components/pdf/RecommendationsPDFTemplate.tsx".to_string(),
            exports: vec![ExportSymbol {
                name: "RecommendationsPDFTemplate".to_string(),
                kind: "function".to_string(),
                export_type: "named".to_string(),
                line: Some(25),
                params: Vec::new(),

                symbol_id: crate::types::SymbolIdV1::default(),
            }],
            ..Default::default()
        };

        let result = find_dead_exports(
            &[importer, exporter],
            false,
            None,
            DeadFilterConfig::default(),
        );
        assert!(
            result.is_empty(),
            "RecommendationsPDFTemplate should NOT be dead. Found: {:?}",
            result
        );
    }

    #[test]
    fn test_dts_reexport_marks_implementation_as_used() {
        // Test the Svelte .d.ts re-export pattern (60% of FPs)
        // Pattern: easing/index.d.ts re-exports from easing/index.js
        // The exports in index.js should NOT be marked as dead

        // Implementation file (.js)
        let mut implementation = FileAnalysis {
            path: "packages/svelte/src/easing/index.js".to_string(),
            language: "js".to_string(),
            ..Default::default()
        };
        implementation.exports = vec![
            ExportSymbol::new("linear".to_string(), "function", "named", Some(1)),
            ExportSymbol::new("backIn".to_string(), "function", "named", Some(5)),
            ExportSymbol::new("backOut".to_string(), "function", "named", Some(10)),
        ];

        // Declaration file (.d.ts) that re-exports from implementation
        let mut declaration = FileAnalysis {
            path: "packages/svelte/src/easing/index.d.ts".to_string(),
            language: "ts".to_string(),
            ..Default::default()
        };
        declaration.reexports.push(ReexportEntry {
            source: "./index.js".to_string(),
            kind: ReexportKind::Named(vec![
                ("linear".to_string(), "linear".to_string()),
                ("backIn".to_string(), "backIn".to_string()),
                ("backOut".to_string(), "backOut".to_string()),
            ]),
            resolved: Some("packages/svelte/src/easing/index.js".to_string()),
        });

        let result = find_dead_exports(
            &[implementation, declaration],
            false,
            None,
            DeadFilterConfig::default(),
        );

        // All easing functions should be marked as used (re-exported by .d.ts)
        assert!(
            result.is_empty(),
            "Exports re-exported by .d.ts should NOT be marked as dead. Found dead: {:?}",
            result
        );
    }

    #[test]
    fn test_dts_star_reexport_marks_all_as_used() {
        // Test .d.ts star re-export pattern
        // Pattern: index.d.ts has `export * from './impl.js'`

        let mut implementation = FileAnalysis {
            path: "lib/impl.js".to_string(),
            language: "js".to_string(),
            ..Default::default()
        };
        implementation.exports = vec![
            ExportSymbol::new("funcA".to_string(), "function", "named", Some(1)),
            ExportSymbol::new("funcB".to_string(), "function", "named", Some(5)),
            ExportSymbol::new("funcC".to_string(), "function", "named", Some(10)),
        ];

        let mut declaration = FileAnalysis {
            path: "lib/index.d.ts".to_string(),
            language: "ts".to_string(),
            ..Default::default()
        };
        declaration.reexports.push(ReexportEntry {
            source: "./impl.js".to_string(),
            kind: ReexportKind::Star,
            resolved: Some("lib/impl.js".to_string()),
        });

        let result = find_dead_exports(
            &[implementation, declaration],
            false,
            None,
            DeadFilterConfig::default(),
        );

        // All functions should be marked as used (star re-export from .d.ts)
        assert!(
            result.is_empty(),
            "Exports re-exported via star by .d.ts should NOT be marked as dead. Found dead: {:?}",
            result
        );
    }
}
