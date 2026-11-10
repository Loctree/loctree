//! Twins Module - Semantic Duplicate Detection
//!
//! Finds two types of code issues:
//! 1. **Dead Parrots**: Exported symbols with zero imports
//! 2. **Exact Twins**: Symbols with the same name exported from different files
//!
//! These are candidates for removal or consolidation.
//!
//! # Philosophy
//!
//! Not all exports need imports to be useful:
//! - Library entry points (lib.rs, index.ts)
//! - CLI handlers (main.rs)
//! - Test fixtures
//! - Framework magic (Next.js pages, Tauri commands)
//!
//! This module focuses on **internal application code** where zero imports
//! or duplicate names usually indicate dead code or naming conflicts.

use serde::Serialize;
use std::collections::HashMap;

use crate::semantic::{RuntimeRole, SemanticFacts};
use crate::types::{FileAnalysis, OutputMode};

/// A single symbol entry in the registry
#[derive(Debug, Clone, Serialize)]
pub struct SymbolEntry {
    /// Symbol name
    pub name: String,
    /// Symbol kind (function, type, const, class, interface, re-export)
    pub kind: String,
    /// File path where symbol is exported
    pub file_path: String,
    /// Line number (if available)
    pub line: usize,
    /// Number of files that import this symbol
    pub import_count: usize,
}

/// Result of twins analysis
#[derive(Debug, Clone, Serialize)]
pub struct TwinsResult {
    /// All dead parrots (0 imports)
    pub dead_parrots: Vec<SymbolEntry>,
    /// Total symbols analyzed
    pub total_symbols: usize,
    /// Total files analyzed
    pub total_files: usize,
}

/// Build symbol registry from file analyses
///
/// Counts how many times each symbol is imported across the codebase.
/// If `include_tests` is false, test files and fixtures are excluded.
pub fn build_symbol_registry(
    analyses: &[FileAnalysis],
    include_tests: bool,
) -> HashMap<(String, String), SymbolEntry> {
    use crate::analyzer::classify::{ArtifactClass, artifact_class, should_exclude_from_reports};
    let mut registry: HashMap<(String, String), SymbolEntry> = HashMap::new();

    // First pass: Register all exports
    for analysis in analyses {
        // Skip test files to avoid pytest/Jest fixtures being treated as dead parrots
        if !include_tests && analysis.is_test {
            continue;
        }
        // Skip test fixtures and mock files
        if !include_tests && should_exclude_from_reports(&analysis.path) {
            continue;
        }
        // Artifact fence: vendored/minified/generated/template files never
        // contribute twin/dead-parrot symbols; fixtures follow include_tests.
        match artifact_class(&analysis.path, None) {
            ArtifactClass::Product => {}
            ArtifactClass::Fixture => {
                if !include_tests {
                    continue;
                }
            }
            _ => continue,
        }
        let is_make_file = analysis.language == "make";
        let is_shell_file = analysis.language == "shell";
        for export in &analysis.exports {
            if is_make_file {
                continue;
            }
            if is_shell_file && export.kind != "function" {
                continue;
            }
            let key = (analysis.path.clone(), export.name.clone());
            registry.insert(
                key,
                SymbolEntry {
                    name: export.name.clone(),
                    kind: export.kind.clone(),
                    file_path: analysis.path.clone(),
                    line: export.line.unwrap_or(0),
                    import_count: 0,
                },
            );
        }
    }

    // Second pass: Count imports
    for analysis in analyses {
        for import in &analysis.imports {
            // Get resolved path (or fall back to source)
            let target_path = import.resolved_path.as_ref().unwrap_or(&import.source);

            // Count each imported symbol
            for symbol in &import.symbols {
                let symbol_name = if symbol.is_default {
                    "default".to_string()
                } else {
                    symbol.name.clone()
                };

                let key = (target_path.clone(), symbol_name);
                if let Some(entry) = registry.get_mut(&key) {
                    entry.import_count += 1;
                }
            }
        }
    }

    registry
}

/// Check if a file is an entry point that shouldn't have dead parrot warnings
fn is_entry_point(path: &str) -> bool {
    // Rust crate roots
    path == "lib.rs"
        || path == "main.rs"
        || path.ends_with("/lib.rs")
        || path.ends_with("/main.rs")
        // TypeScript/JavaScript index entry points
        || path.ends_with("/index.ts")
        || path.ends_with("/index.tsx")
        || path.ends_with("/index.js")
        || path.ends_with("/index.jsx")
        || path.ends_with("/index.mjs")
        // TypeScript/JavaScript App entry points (React, Vue, Tauri, etc.)
        || path.ends_with("/App.tsx")
        || path.ends_with("/App.jsx")
        || path.ends_with("/App.ts")
        || path.ends_with("/App.js")
        || path.ends_with("/app.tsx")
        || path.ends_with("/app.jsx")
        // TypeScript/JavaScript main entry points (Vite, Tauri, Electron, etc.)
        || path.ends_with("/main.ts")
        || path.ends_with("/main.tsx")
        || path.ends_with("/main.js")
        || path.ends_with("/main.jsx")
        // Next.js special files (App Router + Pages Router)
        || path.ends_with("/_app.tsx")
        || path.ends_with("/_app.jsx")
        || path.ends_with("/_document.tsx")
        || path.ends_with("/_document.jsx")
        || path.ends_with("/layout.tsx")
        || path.ends_with("/layout.jsx")
        || path.ends_with("/page.tsx")
        || path.ends_with("/page.jsx")
        // Python package roots
        || path.ends_with("/__init__.py")
        // Go package main
        || (path.ends_with(".go") && path.contains("/cmd/"))
        // Custom application entry points (common naming patterns)
        || is_application_entry_pattern(path)
}

/// Check if file matches common application entry point naming patterns
fn is_application_entry_pattern(path: &str) -> bool {
    // Extract filename from path
    let filename = path.rsplit('/').next().unwrap_or(path);
    let name_lower = filename.to_lowercase();

    // Common entry point patterns (case-insensitive base check)
    // MainApplication.tsx, AppEntry.ts, Bootstrap.tsx, etc.
    let entry_patterns = [
        "application.", // MainApplication.tsx, Application.tsx
        "bootstrap.",   // Bootstrap.tsx, bootstrap.ts
        "entry.",       // Entry.tsx, AppEntry.ts
        "appshell.",    // AppShell.tsx
        "appinit.",     // AppInit.tsx
    ];

    for pattern in entry_patterns {
        if name_lower.contains(pattern) {
            return true;
        }
    }

    // Also check for app-shell directory pattern (common in Tauri/Electron)
    if path.contains("/app-shell/") || path.contains("/app_shell/") {
        return true;
    }

    false
}

/// Check if a file is a mod.rs (Rust module declaration file)
fn is_mod_rs(path: &str) -> bool {
    path == "mod.rs" || path.ends_with("/mod.rs")
}

/// Check if export is a common framework magic pattern
fn is_framework_magic(name: &str, kind: &str) -> bool {
    // Python dunder methods
    if name.starts_with("__") && name.ends_with("__") {
        return true;
    }
    // React/Svelte component conventions (PascalCase default exports)
    if kind == "default"
        && name
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
    {
        return true;
    }
    // Common framework hooks
    if name.starts_with("use")
        && name.len() > 3
        && name
            .chars()
            .nth(3)
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
    {
        return true;
    }
    // Django/Python mixins
    if kind == "class" && name.ends_with("Mixin") {
        return true;
    }
    // Rust trait implementations often not directly imported
    if kind == "impl" || kind == "trait" {
        return true;
    }
    false
}

/// Check if symbol name is a common re-export pattern (barrel exports)
fn is_barrel_reexport(_name: &str, kind: &str) -> bool {
    // Re-exports are not dead - they're forwarding from another module
    kind == "re-export" || kind == "reexport"
}

/// Find dead parrots - symbols with 0 imports
pub fn find_dead_parrots(
    analyses: &[FileAnalysis],
    _dead_only: bool,
    include_tests: bool,
) -> TwinsResult {
    let registry = build_symbol_registry(analyses, include_tests);

    // Build set of Tauri handlers (registered commands)
    let tauri_handlers: std::collections::HashSet<String> = analyses
        .iter()
        .flat_map(|a| a.tauri_registered_handlers.iter().cloned())
        .collect();

    // Build set of locally used symbols per file
    let all_local_uses: std::collections::HashSet<String> = analyses
        .iter()
        .flat_map(|a| a.local_uses.iter().cloned())
        .collect();

    // Build set of all imported symbol names (fallback for unresolved paths)
    let all_imported_names: std::collections::HashSet<String> = analyses
        .iter()
        .flat_map(|a| a.imports.iter())
        .flat_map(|imp| imp.symbols.iter())
        .map(|sym| {
            if sym.is_default {
                "default".to_string()
            } else {
                sym.name.clone()
            }
        })
        .collect();

    // Build set of dynamically imported file paths (React lazy, Vue async components, etc.)
    // These typically import the default export, so we track which files are dynamically imported
    // Use normalize_module_id to get consistent path format for matching
    use super::root_scan::normalize_module_id;
    use crate::types::ImportKind;

    // Build set of normalized file paths that are dynamically imported
    let mut dynamic_import_targets: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    // First, collect from ImportEntry items with ImportKind::Dynamic (has resolved_path)
    // This is more reliable than raw dynamic_imports strings
    for analysis in analyses {
        for imp in &analysis.imports {
            if matches!(imp.kind, ImportKind::Dynamic) {
                // Use resolved path if available (most reliable)
                if let Some(resolved) = &imp.resolved_path {
                    dynamic_import_targets.insert(normalize_module_id(resolved).as_key());
                }
                // Also add the raw source path normalized
                dynamic_import_targets.insert(normalize_module_id(&imp.source).as_key());
            }
        }
    }

    // Also check raw dynamic_imports strings as fallback and for matching
    for analysis in analyses {
        for dyn_imp in &analysis.dynamic_imports {
            let dyn_norm = normalize_module_id(dyn_imp);
            dynamic_import_targets.insert(dyn_norm.as_key());

            // Also add suffix-matched files (handles ./components/X matching src/components/X)
            let dyn_alias = dyn_norm
                .path
                .trim_start_matches("./")
                .trim_start_matches('@')
                .to_string();
            for a in analyses {
                let a_norm = normalize_module_id(&a.path);
                if a_norm.path.ends_with(&dyn_alias) {
                    dynamic_import_targets.insert(a_norm.as_key());
                }
            }
        }
    }

    let mut dead_parrots: Vec<SymbolEntry> = registry
        .values()
        .filter(|entry| {
            // Skip if has imports
            if entry.import_count > 0 {
                return false;
            }
            // Skip Tauri commands
            if tauri_handlers.contains(&entry.name) {
                return false;
            }
            // Skip locally used symbols
            if all_local_uses.contains(&entry.name) {
                return false;
            }
            // Skip if imported by name anywhere (handles unresolved paths)
            if all_imported_names.contains(&entry.name) {
                return false;
            }
            // Skip entry points (lib.rs, index.ts, __init__.py, etc.)
            if is_entry_point(&entry.file_path) {
                return false;
            }
            // Skip mod.rs files (Rust module declarations)
            if is_mod_rs(&entry.file_path) {
                return false;
            }
            // Skip framework magic patterns
            if is_framework_magic(&entry.name, &entry.kind) {
                return false;
            }
            // Skip barrel re-exports
            if is_barrel_reexport(&entry.name, &entry.kind) {
                return false;
            }
            // Skip default exports from dynamically imported files (React lazy, Vue async, etc.)
            // Dynamic imports like lazy(() => import('./Component')) only consume the default export
            // Normalize the file path the same way dynamic imports are normalized
            let file_norm = normalize_module_id(&entry.file_path).as_key();
            if dynamic_import_targets.contains(&file_norm) {
                return false;
            }
            true
        })
        .cloned()
        .collect();

    // Sort by file path, then symbol name for consistent output
    dead_parrots.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then_with(|| a.name.cmp(&b.name))
    });

    TwinsResult {
        dead_parrots,
        total_symbols: registry.len(),
        total_files: analyses.len(),
    }
}

/// Print twins results in human-readable format
pub fn print_twins_human(result: &TwinsResult) {
    if result.dead_parrots.is_empty() {
        println!("No dead parrots found - all exports are imported!");
        return;
    }

    println!("DEAD PARROTS ({} found)", result.dead_parrots.len());
    println!();

    // Group by file for cleaner output
    let mut by_file: HashMap<String, Vec<&SymbolEntry>> = HashMap::new();
    for entry in &result.dead_parrots {
        by_file
            .entry(entry.file_path.clone())
            .or_default()
            .push(entry);
    }

    let mut files: Vec<_> = by_file.keys().collect();
    files.sort();

    for file in files {
        let entries = &by_file[file];
        println!("  {}", file);
        for entry in entries {
            println!(
                "    ├─ {} ({}:{}) - {} imports",
                entry.name, entry.kind, entry.line, entry.import_count
            );
        }
        println!();
    }

    println!("Summary:");
    println!("  Total symbols: {}", result.total_symbols);
    println!("  Dead parrots: {}", result.dead_parrots.len());
    println!("  Files analyzed: {}", result.total_files);
}

/// Print twins results in JSON format
pub fn print_twins_json(result: &TwinsResult) {
    let output = serde_json::json!({
        "dead_parrots": result.dead_parrots.iter().map(|e| {
            serde_json::json!({
                "name": e.name,
                "file": e.file_path,
                "line": e.line,
                "kind": e.kind,
                "import_count": e.import_count,
            })
        }).collect::<Vec<_>>(),
        "summary": {
            "symbols": result.total_symbols,
            "files": result.total_files,
            "dead_parrots": result.dead_parrots.len(),
        }
    });

    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}

/// Print twins results based on output mode
pub fn print_twins_result(result: &TwinsResult, output: OutputMode) {
    match output {
        OutputMode::Json | OutputMode::Jsonl => print_twins_json(result),
        OutputMode::Human => print_twins_human(result),
    }
}

// ============================================================================
// EXACT TWIN DETECTION
// ============================================================================

/// A location where an exact twin symbol is found
#[derive(Clone, Debug, Serialize)]
pub struct TwinLocation {
    /// File path where the symbol is exported
    pub file_path: String,
    /// Line number (1-based)
    pub line: usize,
    /// Export kind: "export", "re-export", "type", "default", etc.
    pub kind: String,
    /// Number of imports of this specific export
    pub import_count: usize,
    /// True if this is the "source of truth" (canonical definition)
    pub is_canonical: bool,
    /// Signature fingerprint for functions (sorted types used in params/return)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_fingerprint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TwinClassification {
    Duplicate,
    Namesake,
    Mirror,
}

/// Shape-evidence class for a twin group (W2-b: classification instead of
/// name-collision sold as duplication).
///
/// - `Exact` — every location carries a signature fingerprint and the
///   fingerprints are identical. The ONLY class that earns a "consolidate"
///   recommendation.
/// - `ShapeSimilar` — fingerprints present at every location and overlapping
///   (Jaccard >= 0.4) but not identical. Review, do not auto-consolidate.
/// - `NameCollision` — same name, no shape evidence of duplication:
///   kind mismatch, missing fingerprints (extraction carries no field/param
///   data for this symbol kind or language — never EXACT in that case), or
///   methods on different impl types (registry/trait pattern). Informational.
/// - `Idiom` — idiomatic constructor/conversion methods (`from_env`,
///   `to_json`, ...) on unrelated types. Convention, not debt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TwinClass {
    Exact,
    ShapeSimilar,
    NameCollision,
    Idiom,
}

impl TwinClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            TwinClass::Exact => "EXACT",
            TwinClass::ShapeSimilar => "SHAPE_SIMILAR",
            TwinClass::NameCollision => "NAME_COLLISION",
            TwinClass::Idiom => "IDIOM",
        }
    }
}

/// Historical dead-parrot cap, retained for source compatibility.
/// `loct twins` now pages all finding families globally; `follow twins` emits
/// no dead parrots.
pub const DEAD_PARROTS_PAGE_CAP: usize = 10;

fn twin_class_page_rank(class: TwinClass) -> u8 {
    match class {
        TwinClass::Exact => 0,
        TwinClass::ShapeSimilar => 1,
        TwinClass::NameCollision => 2,
        TwinClass::Idiom => 3,
    }
}

/// Parse a CSV class filter.
///
/// Tokens: `EXACT`, `SHAPE_SIMILAR`, `NAME_COLLISION`, `IDIOM`.
/// Family aliases: `structural` = EXACT + SHAPE_SIMILAR, `naming` = NAME_COLLISION + IDIOM.
/// Matching is case-insensitive. Unknown tokens are an error.
pub fn parse_twin_class_filter(spec: &str) -> Result<Vec<TwinClass>, String> {
    let mut out = Vec::new();
    if spec.trim().is_empty() {
        return Err(
            "twin class filter is empty. Expected EXACT, SHAPE_SIMILAR, NAME_COLLISION, IDIOM, structural, or naming"
                .to_string(),
        );
    }
    for raw in spec.split(',') {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        let classes: &[TwinClass] = match token.to_ascii_uppercase().as_str() {
            "EXACT" => &[TwinClass::Exact],
            "SHAPE_SIMILAR" | "SHAPE-SIMILAR" => &[TwinClass::ShapeSimilar],
            "NAME_COLLISION" | "NAME-COLLISION" => &[TwinClass::NameCollision],
            "IDIOM" => &[TwinClass::Idiom],
            "STRUCTURAL" => &[TwinClass::Exact, TwinClass::ShapeSimilar],
            "NAMING" => &[TwinClass::NameCollision, TwinClass::Idiom],
            _ => {
                return Err(format!(
                    "unknown twin class '{token}'. Expected EXACT, SHAPE_SIMILAR, NAME_COLLISION, IDIOM, structural, or naming"
                ));
            }
        };
        for class in classes {
            if !out.contains(class) {
                out.push(*class);
            }
        }
    }
    if out.is_empty() {
        return Err(
            "twin class filter is empty. Expected EXACT, SHAPE_SIMILAR, NAME_COLLISION, IDIOM, structural, or naming"
                .to_string(),
        );
    }
    Ok(out)
}

/// `(skip, take)` windows for families walked in order.
///
/// `offset` skips items across the families. `limit: None` keeps everything
/// that remains after the skip.
pub fn page_family_windows(
    lengths: &[usize],
    offset: usize,
    limit: Option<usize>,
) -> Vec<(usize, usize)> {
    let mut skip_left = offset;
    let mut take_left = limit;
    let mut windows = Vec::with_capacity(lengths.len());
    for &len in lengths {
        let skip = skip_left.min(len);
        skip_left -= skip;
        let available = len - skip;
        let take = match take_left {
            Some(left) => {
                let n = left.min(available);
                take_left = Some(left - n);
                n
            }
            None => available,
        };
        windows.push((skip, take));
    }
    windows
}

/// True when items remain after `offset + returned` in a filtered census of `total`.
pub fn page_has_more(total: usize, offset: usize, returned: usize) -> bool {
    offset.saturating_add(returned) < total
}

/// Idiomatic constructor/conversion method names: `from_env`, `to_json`,
/// `into_inner`, `as_str`, `with_capacity`, `try_parse`, ... Same name on
/// unrelated types is a language convention, not duplication.
fn is_idiom_method_name(name: &str) -> bool {
    const IDIOM_PREFIXES: &[&str] = &["from_", "to_", "into_", "as_", "with_", "try_", "is_"];
    IDIOM_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Classify a twin group by shape evidence.
///
/// `all_methods` must be true when every location is a method on some impl
/// type (kind == "method" from indexed extraction, or the symbol appears in
/// `FileAnalysis::impl_methods` for regex-based Rust extraction).
///
/// `impl_qualifiers` carries the implemented type/receiver per location
/// (from `FileAnalysis::impl_methods[].qualifier`) when known, `None` when
/// not. Two or more locations with DIFFERENT known qualifiers (F3: a
/// delegation chain like `Screen::resize_pane_with_id` calling
/// `Tab::resize_pane_with_id`) is proof the match is a same-name/same-shape
/// coincidence, never a literal duplicate — this refuses EXACT even when
/// the folded param/return type fingerprint is identical, because that
/// fingerprint never compared the bodies (or the receivers) at all. Pass an
/// empty slice when receiver data is unavailable; that never downgrades.
pub fn classify_twin_class(
    name: &str,
    locations: &[TwinLocation],
    signature_similarity: Option<f32>,
    all_methods: bool,
    impl_qualifiers: &[Option<&str>],
) -> TwinClass {
    if all_methods {
        // Methods of different enums/structs or impls of the same trait:
        // name equality is the contract (registry pattern), not duplication.
        return if is_idiom_method_name(name) {
            TwinClass::Idiom
        } else {
            TwinClass::NameCollision
        };
    }

    // Kind mismatch (struct vs trait, function vs class) is never a duplicate.
    let mut kinds: Vec<&str> = locations.iter().map(|l| l.kind.as_str()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    if kinds.len() > 1 {
        return TwinClass::NameCollision;
    }

    // EXACT requires shape evidence at EVERY location. If extraction carries
    // no signature/field data (structs, enums, unsupported languages), the
    // class is NAME_COLLISION — never EXACT on name equality alone.
    let all_have_fingerprints =
        !locations.is_empty() && locations.iter().all(|l| l.signature_fingerprint.is_some());
    if !all_have_fingerprints {
        return TwinClass::NameCollision;
    }

    let distinct_qualifiers: std::collections::HashSet<&str> =
        impl_qualifiers.iter().filter_map(|q| *q).collect();
    let receivers_differ = distinct_qualifiers.len() > 1;

    match signature_similarity {
        Some(sim) if sim >= 0.999 && receivers_differ => TwinClass::NameCollision,
        Some(sim) if sim >= 0.999 => TwinClass::Exact,
        Some(sim) if sim >= 0.4 => TwinClass::ShapeSimilar,
        _ => TwinClass::NameCollision,
    }
}

/// An exact twin - a symbol exported from multiple files
#[derive(Clone, Debug, Serialize)]
pub struct ExactTwin {
    /// Symbol name
    pub name: String,
    /// All locations where this symbol is exported
    pub locations: Vec<TwinLocation>,
    /// Signature similarity score (0.0 = different, 1.0 = identical signatures)
    /// None if signatures couldn't be computed (non-functions, missing data)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_similarity: Option<f32>,
    /// Classification of the twin
    pub classification: TwinClassification,
    /// Shape-evidence class (additive JSON field, W2-b):
    /// EXACT / SHAPE_SIMILAR / NAME_COLLISION / IDIOM.
    pub class: TwinClass,
    /// W1-03 / W2-01 contract: every location carries the same signature
    /// fingerprint. Missing fingerprints or a mismatch is shapeless.
    /// Body hashes are not on this snapshot path — signature only.
    #[serde(default)]
    pub shape_match: bool,
    /// W1-03 / W2-01 contract: the project manifest declares exactly one
    /// non-test module/target (Package.swift with one `.target` /
    /// `.executableTarget`). `false` means not established, including unknown
    /// source ownership; never inferred from language identity.
    #[serde(default)]
    pub single_module_target: bool,
    /// W1-03 / W2-01 contract: shapeless collision inside a single-module
    /// target must not feed health/score. The sensor still lists the group.
    /// Also true for shape-backed candidates whose owned source evidence is
    /// unavailable. Those remain visible, but require verification before action.
    #[serde(default)]
    pub exclude_from_score: bool,
}

/// Page order: EXACT and SHAPE_SIMILAR, then NAME_COLLISION and IDIOM.
/// Within a class, longer location lists stay first, then names break ties.
/// Locations use the detector's canonical preference (imports, shortest path,
/// alphabetical tie). Pin that preference before paging, including cold scans
/// whose file traversal order may differ from the sorted persisted snapshot.
pub fn order_twins_for_page(twins: &mut [ExactTwin]) {
    for twin in twins.iter_mut() {
        twin.locations.sort_by(|a, b| {
            b.import_count
                .cmp(&a.import_count)
                .then(a.file_path.len().cmp(&b.file_path.len()))
                .then(a.file_path.cmp(&b.file_path))
                .then(a.line.cmp(&b.line))
                .then(a.kind.cmp(&b.kind))
        });
        for (index, location) in twin.locations.iter_mut().enumerate() {
            location.is_canonical = index == 0;
        }
    }
    twins.sort_by(|a, b| {
        twin_class_page_rank(a.class)
            .cmp(&twin_class_page_rank(b.class))
            .then(b.locations.len().cmp(&a.locations.len()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// Drop groups outside `classes`. An empty filter keeps every group.
pub fn retain_twin_classes(twins: &mut Vec<ExactTwin>, classes: &[TwinClass]) {
    if classes.is_empty() {
        return;
    }
    twins.retain(|twin| classes.contains(&twin.class));
}

/// True when every location carries the same signature fingerprint.
/// Missing fingerprints (typical for Swift stored properties and protocol
/// witnesses) are shapeless — name equality is not shape evidence.
pub fn locations_have_shape_match(locations: &[TwinLocation]) -> bool {
    if locations.len() < 2 {
        return false;
    }
    let Some(first) = locations[0].signature_fingerprint.as_deref() else {
        return false;
    };
    locations[1..]
        .iter()
        .all(|loc| loc.signature_fingerprint.as_deref() == Some(first))
}

/// Count non-test SwiftPM product targets in a `Package.swift` body.
/// `.testTarget(` is excluded. Literal token scan, not a Swift parser.
pub fn count_swiftpm_product_targets(src: &str) -> usize {
    let mut body = String::with_capacity(src.len());
    for line in src.lines() {
        let cut = line.find("//").unwrap_or(line.len());
        body.push_str(&line[..cut]);
        body.push('\n');
    }
    [
        ".executableTarget(",
        ".binaryTarget(",
        ".macro(",
        ".target(",
    ]
    .iter()
    .map(|token| body.matches(token).count())
    .sum()
}

/// W1-03 gate for `duplicate_groups`: a namesake (or shapeless single-module
/// collision) stays on the sensor and must not increment the findings count.
/// Reads the existing `classification: namesake` plus `exclude_from_score`.
pub fn omit_from_duplicate_groups(twin: &ExactTwin) -> bool {
    twin.exclude_from_score || matches!(twin.classification, TwinClassification::Namesake)
}

/// Manifest-derived: the project declares exactly one non-test SwiftPM target.
/// Legacy rootless query: without an owner this fact is unproven (`false`).
/// Production detection uses `TwinSources` to prove the per-owner manifest
/// gate. Language identity is never consulted.
pub fn project_is_single_module_target(_analyses: &[FileAnalysis]) -> bool {
    // Without an explicit source owner the manifest fact is unproven.
    false
}

/// Filesystem evidence for one twin computation. Display paths and SymbolIDs
/// stay untouched. Only explicit, canonical project roots authorize reads.
/// Relative paths in a multi-root snapshot have no recorded per-file owner;
/// they are unknown, never assigned to the first root or searched by existence.
#[derive(Default)]
pub struct TwinSources {
    // Preserve the declared spelling as well as realpath (macOS /var alias).
    // Lexical ownership is chosen before the confinement gate follows symlinks.
    roots: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    allow_relative: bool,
    contents: HashMap<String, Option<String>>,
}

impl TwinSources {
    pub fn for_root(root: &std::path::Path) -> Self {
        Self {
            roots: std::path::absolute(root)
                .ok()
                .and_then(|declared| {
                    root.canonicalize()
                        .ok()
                        .map(|canonical| (declared, canonical))
                })
                .into_iter()
                .collect(),
            allow_relative: true,
            ..Self::default()
        }
    }

    pub fn for_snapshot(snapshot: &crate::snapshot::Snapshot) -> Self {
        let roots = snapshot
            .metadata
            .roots
            .iter()
            .map(std::path::PathBuf::from)
            .filter(|root| root.is_absolute())
            .filter_map(|root| root.canonicalize().ok().map(|canonical| (root, canonical)))
            .collect();
        Self {
            roots,
            // Missing/relative owners must not collapse a multi-root snapshot
            // into apparent single-root provenance.
            allow_relative: snapshot.metadata.roots.len() == 1,
            ..Self::default()
        }
    }

    /// A request root may rebase a single-root snapshot, but cannot erase
    /// multiple source owners recorded by a multi-root scan.
    pub fn for_request(snapshot: &crate::snapshot::Snapshot, root: &std::path::Path) -> Self {
        if snapshot.metadata.roots.len() > 1 {
            Self::for_snapshot(snapshot)
        } else {
            Self::for_root(root)
        }
    }

    fn owner(&self, file: &str) -> Option<&std::path::Path> {
        let path = std::path::Path::new(file);
        if path.is_absolute() {
            self.roots
                .iter()
                .filter(|(declared, canonical)| {
                    path.starts_with(declared) || path.starts_with(canonical)
                })
                .max_by_key(|(declared, canonical)| {
                    declared
                        .components()
                        .count()
                        .max(canonical.components().count())
                })
                .map(|(_, canonical)| canonical.as_path())
        } else if self.allow_relative && self.roots.len() == 1 {
            Some(self.roots[0].1.as_path())
        } else {
            None
        }
    }

    pub(crate) fn read(&mut self, file: &str) -> Option<&str> {
        if !self.contents.contains_key(file) {
            let content = self.owner(file).and_then(|root| {
                let path = root.join(file);
                crate::fs_utils::read_to_string_within(root, &path).ok()
            });
            self.contents.insert(file.to_string(), content);
        }
        self.contents
            .get(file)
            .and_then(|content| content.as_deref())
    }

    fn single_module_for(&mut self, analyses: &[FileAnalysis], locations: &[TwinLocation]) -> bool {
        let Some(owner) = locations
            .first()
            .and_then(|loc| self.owner(&loc.file_path))
            .map(|p| p.to_path_buf())
        else {
            return false;
        };
        if locations
            .iter()
            .any(|loc| self.owner(&loc.file_path) != Some(owner.as_path()))
        {
            return false;
        }
        let manifests: Vec<_> = analyses
            .iter()
            .filter(|a| {
                std::path::Path::new(&a.path)
                    .file_name()
                    .is_some_and(|name| name == "Package.swift")
                    && self.owner(&a.path) == Some(owner.as_path())
            })
            .collect();
        !manifests.is_empty()
            && manifests.iter().all(|a| {
                self.read(&a.path)
                    .is_some_and(|src| count_swiftpm_product_targets(src) == 1)
            })
    }
}

/// Language category for twin classification
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Language {
    TypeScript,
    JavaScript,
    Rust,
    Python,
    Go,
    Swift,
    Zig,
    Other,
}

/// Twin category based on language distribution
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum TwinCategory {
    /// All locations are in the same language (likely a real duplicate)
    SameLanguage(Language),
    /// Locations span multiple languages (likely intentional FE/BE pair)
    CrossLanguage,
    /// Symbols share a name but are fundamentally different concepts (e.g. function vs class, or 0.0 similarity)
    Namesake,
}

/// Detect language from file extension
pub fn detect_language(path: &str) -> Language {
    if path.ends_with(".ts") || path.ends_with(".tsx") || path.ends_with(".mts") {
        Language::TypeScript
    } else if path.ends_with(".js")
        || path.ends_with(".jsx")
        || path.ends_with(".mjs")
        || path.ends_with(".cjs")
    {
        Language::JavaScript
    } else if path.ends_with(".rs") {
        Language::Rust
    } else if path.ends_with(".py") || path.ends_with(".pyi") {
        Language::Python
    } else if path.ends_with(".go") {
        Language::Go
    } else if path.ends_with(".swift") {
        Language::Swift
    } else if path.ends_with(".zig") {
        Language::Zig
    } else {
        Language::Other
    }
}

/// Categorize a twin based on languages of its locations.
///
/// Priority: cross-language mirror beats kind-mismatch (intentional FE/BE pair
/// often crosses language boundaries with different `kind` strings — e.g.
/// TypeScript `interface Foo` + Rust `struct Foo`). Within a single language,
/// different kinds → namesake (e.g. struct vs trait in two Rust crates).
pub fn categorize_twin(twin: &ExactTwin) -> TwinCategory {
    let languages: std::collections::HashSet<Language> = twin
        .locations
        .iter()
        .map(|loc| detect_language(&loc.file_path))
        .collect();

    if languages.len() > 1 {
        return TwinCategory::CrossLanguage;
    }

    let kinds: std::collections::HashSet<_> = twin.locations.iter().map(|loc| &loc.kind).collect();
    if kinds.len() > 1 {
        return TwinCategory::Namesake;
    }

    if let Some(sim) = twin.signature_similarity
        && sim == 0.0
    {
        return TwinCategory::Namesake;
    }

    TwinCategory::SameLanguage(languages.into_iter().next().unwrap_or(Language::Other))
}

/// Generic method names that should be excluded from twin detection
/// These are common names that appear in many files by design, not by accident.
pub const GENERIC_METHOD_NAMES: &[&str] = &[
    // Rust/OOP constructors and traits
    "new",
    "default",
    "from",
    "into",
    "clone",
    "drop",
    "deref",
    "as_ref",
    "as_mut",
    "try_from",
    "try_into",
    "with_config",
    // Entry points (every binary has one)
    "main",
    "app",
    "App",
    // Lifecycle methods
    "init",
    "setup",
    "teardown",
    "cleanup",
    "dispose",
    "destroy",
    "mount",
    "unmount",
    // CRUD operations
    "create",
    "read",
    "update",
    "delete",
    "get",
    "set",
    "load",
    "save",
    "fetch",
    "store",
    // Process/execution
    "run",
    "start",
    "stop",
    "execute",
    "process",
    "handle",
    "dispatch",
    // I/O operations
    "open",
    "close",
    "write",
    "send",
    "receive",
    "connect",
    "disconnect",
    // Serialization
    "parse",
    "format",
    "serialize",
    "deserialize",
    "encode",
    "decode",
    "to_string",
    "from_str",
    "to_json",
    "from_json",
    "to_bytes",
    "from_bytes",
    "as_str",
    "as_bytes",
    "into_inner",
    "inner",
    "get_inner",
    "unwrap",
    "unwrap_or",
    "ok",
    "err",
    // State management
    "reset",
    "clear",
    "flush",
    "refresh",
    // UI/rendering
    "render",
    "draw",
    "display",
    "show",
    "hide",
    // Validation/configuration
    "validate",
    "configure",
    "build",
    // Testing (fixtures, setup)
    "test",
    "fixture",
    "mock",
    "stub",
    "spy",
    // Python special methods
    "__init__",
    "__new__",
    "__str__",
    "__repr__",
    // Common generic names
    "apply",
    "call",
    "invoke",
    "notify",
    "emit",
    "on",
    "off",
    "add",
    "remove",
    "insert",
    "push",
    "pop",
    "len",
    "size",
    "count",
    "index",
    "find",
    "search",
    "filter",
    "map",
    "reduce",
    "sort",
    "compare",
    "equals",
    "hash",
    "copy",
    "merge",
    "split",
    "join",
    "concat",
    "append",
    "extend",
    "contains",
    "exists",
    "is_empty",
    "is_valid",
    "check",
    "verify",
    "assert",
    "expect",
    "should",
    "must",
    "can",
    "will",
    "with",
    "label",
    "name",
    "id",
    "key",
    "value",
    "data",
    "info",
    "error",
    "warn",
    "debug",
    "log",
    "print",
    "trace",
];

fn is_generic_method(name: &str) -> bool {
    GENERIC_METHOD_NAMES.contains(&name)
}

/// Compute a fingerprint for a function's signature based on its parameter and return types.
/// Returns a sorted, normalized string like "String,User|Result<bool>" (params|return).
fn compute_signature_fingerprint(
    analyses: &[FileAnalysis],
    file_path: &str,
    function_name: &str,
) -> Option<String> {
    // Find the FileAnalysis for this file
    let analysis = analyses.iter().find(|a| a.path == file_path)?;

    // Collect types used in this function's signature
    let mut param_types: Vec<String> = Vec::new();
    let mut return_types: Vec<String> = Vec::new();

    for sig_use in &analysis.signature_uses {
        if sig_use.function == function_name {
            match sig_use.usage {
                crate::types::SignatureUseKind::Parameter => {
                    param_types.push(sig_use.type_name.clone());
                }
                crate::types::SignatureUseKind::Return => {
                    return_types.push(sig_use.type_name.clone());
                }
            }
        }
    }

    // If no signature info found, return None
    if param_types.is_empty() && return_types.is_empty() {
        return None;
    }

    // Sort for consistent comparison
    param_types.sort();
    return_types.sort();

    // Create fingerprint: "param1,param2|return1,return2"
    let params_str = param_types.join(",");
    let returns_str = return_types.join(",");

    Some(format!("{}|{}", params_str, returns_str))
}

/// Recommended consolidation action for a twin group, gated on signature evidence.
///
/// The `follow twins` surface previously emitted a single hardcoded action —
/// "consolidate into single module" — for every group regardless of whether the
/// signatures actually matched. That recommendation is destructive when applied
/// to twins with signature_similarity 0.0 or 0.12 (same name, different shape):
/// merging five unrelated `compute` functions into one module replaces a
/// readable naming collision with a broken refactor.
///
/// The thresholds match the existing `high_similarity_groups` cutoff used in
/// `print_exact_twins_json` (>= 0.8 → real duplicate).
pub fn twin_action(twin: &ExactTwin) -> &'static str {
    if twin.classification == TwinClassification::Mirror {
        return "verify the mirror contract is current; do not consolidate";
    }
    if twin.exclude_from_score && matches!(twin.class, TwinClass::Exact | TwinClass::ShapeSimilar) {
        return "verify owned source and target evidence before consolidation; excluded from score";
    }
    // "Consolidate" is earned by shape evidence (class EXACT), never by name
    // equality alone.
    match twin.class {
        TwinClass::Exact => "consolidate into single module",
        TwinClass::ShapeSimilar => "review overlapping shapes; consolidate only after manual diff",
        TwinClass::NameCollision => {
            "name collision (informational); rename to disambiguate or confirm intentional topical naming"
        }
        TwinClass::Idiom => "idiomatic convention (constructor/conversion); no action",
    }
}

/// Compute Jaccard similarity between two fingerprints.
/// Returns 1.0 for identical, 0.0 for completely different.
fn fingerprint_similarity(fp1: &str, fp2: &str) -> f32 {
    if fp1 == fp2 {
        return 1.0;
    }

    // Split into individual types
    let types1: std::collections::HashSet<&str> =
        fp1.split([',', '|']).filter(|s| !s.is_empty()).collect();
    let types2: std::collections::HashSet<&str> =
        fp2.split([',', '|']).filter(|s| !s.is_empty()).collect();

    if types1.is_empty() && types2.is_empty() {
        return 1.0; // Both empty = same
    }

    let intersection = types1.intersection(&types2).count();
    let union = types1.union(&types2).count();

    if union == 0 {
        return 0.0;
    }

    intersection as f32 / union as f32
}

/// Compute average pairwise similarity for a group of fingerprints.
fn compute_group_similarity(fingerprints: &[Option<String>]) -> Option<f32> {
    let mut valid_fps: Vec<&String> = fingerprints.iter().filter_map(|f| f.as_ref()).collect();
    // Registry iteration is randomized per process. Keep the same f32 pairwise
    // mean (including repeated signatures), but accumulate in canonical order.
    // Sorting locations later cannot undo order-dependent floating point sums.
    valid_fps.sort_unstable();

    if valid_fps.len() < 2 {
        return None; // Need at least 2 to compare
    }

    let mut total_similarity = 0.0;
    let mut count = 0;

    for i in 0..valid_fps.len() {
        for j in (i + 1)..valid_fps.len() {
            total_similarity += fingerprint_similarity(valid_fps[i], valid_fps[j]);
            count += 1;
        }
    }

    if count == 0 {
        return None;
    }

    Some(total_similarity / count as f32)
}

/// Detect exact twins: symbols with the same name exported from different files
///
/// This is the simple version without framework awareness.
/// For framework-aware filtering, use `detect_exact_twins_with_frameworks`.
pub fn detect_exact_twins(analyses: &[FileAnalysis], include_tests: bool) -> Vec<ExactTwin> {
    detect_exact_twins_with_frameworks(analyses, include_tests, None)
}

/// Drop twin groups that Layer 3 semantic facts identify as convention rather
/// than refactoring debt: two `usage` printers in two scripts are not a
/// duplicate to consolidate, they are an idiom every script repeats.
///
/// A group is dropped when every location of the twin shares at least one
/// idiom tag whose `runtime_role` marks the symbol as a known convention
/// (`LibraryHelper`, `UserFacing`, `Metadata`, `PublicEntrypoint`,
/// `PrimaryEntrypoint`, `EnvInput`).
pub fn filter_idiom_twins(twins: Vec<ExactTwin>, facts: &SemanticFacts) -> Vec<ExactTwin> {
    if facts.idiom_tags.is_empty() {
        return twins;
    }

    twins
        .into_iter()
        .filter(|twin| !is_convention_twin(twin, facts))
        .collect()
}

fn is_convention_twin(twin: &ExactTwin, facts: &SemanticFacts) -> bool {
    if twin.locations.len() < 2 {
        return false;
    }

    let mut shared_role_names: Option<std::collections::HashSet<String>> = None;

    for loc in &twin.locations {
        let symbol_id = format!("{}::{}", loc.file_path, twin.name);
        let Some(tags) = facts.idiom_tags.get(&symbol_id) else {
            return false;
        };

        let role_names: std::collections::HashSet<String> = tags
            .iter()
            .filter(|tag| is_convention_role(&tag.runtime_role))
            .map(|tag| tag.name.clone())
            .collect();

        if role_names.is_empty() {
            return false;
        }

        shared_role_names = Some(match shared_role_names {
            Some(prev) => prev.intersection(&role_names).cloned().collect(),
            None => role_names,
        });

        if shared_role_names.as_ref().is_some_and(|s| s.is_empty()) {
            return false;
        }
    }

    shared_role_names.is_some_and(|s| !s.is_empty())
}

fn is_convention_role(role: &RuntimeRole) -> bool {
    matches!(
        role,
        RuntimeRole::LibraryHelper
            | RuntimeRole::UserFacing
            | RuntimeRole::Metadata
            | RuntimeRole::PublicEntrypoint
            | RuntimeRole::PrimaryEntrypoint
            | RuntimeRole::EnvInput
    )
}

/// Detect exact twins with framework-aware filtering
///
/// # Arguments
/// * `analyses` - File analysis data
/// * `include_tests` - Whether to include test files
/// * `frameworks` - Optional list of detected frameworks. When provided, intentional
///   framework conventions (like GET in SvelteKit +server.ts) are filtered out.
///   Pass `None` or `&[]` to show all duplicates.
pub fn detect_exact_twins_with_frameworks(
    analyses: &[FileAnalysis],
    include_tests: bool,
    frameworks: Option<&[crate::analyzer::frameworks::Framework]>,
) -> Vec<ExactTwin> {
    detect_exact_twins_with_sources(
        analyses,
        include_tests,
        frameworks,
        &mut TwinSources::default(),
    )
}

/// Root-aware twin pipeline; the rootless API retains AST evidence only and
/// qualifies unavailable source instead of reading the caller's directory.
pub fn detect_exact_twins_with_sources(
    analyses: &[FileAnalysis],
    include_tests: bool,
    frameworks: Option<&[crate::analyzer::frameworks::Framework]>,
    sources: &mut TwinSources,
) -> Vec<ExactTwin> {
    let registry = build_symbol_registry(analyses, include_tests);

    // Methods declared inside impl blocks (regex-based Rust extraction exports
    // `pub fn` inside `impl` with kind "function"; impl_methods carries the
    // truth that they are methods on a type). Used to classify same-name
    // methods on different types as NAME_COLLISION / IDIOM, never EXACT.
    let impl_method_set: std::collections::HashSet<(&str, &str)> = analyses
        .iter()
        .flat_map(|a| {
            a.impl_methods
                .iter()
                .map(move |m| (a.path.as_str(), m.name.as_str()))
        })
        .collect();

    // Build map: symbol_name -> Vec<(file_path, line, kind, import_count)>
    let mut symbol_map: HashMap<String, Vec<(String, usize, String, usize)>> = HashMap::new();

    for ((file_path, symbol_name), entry) in &registry {
        // Skip re-exports and __all__ entries - they're intentional API design, not duplicates
        // __all__ in Python is a declaration of public API, not a new definition
        if entry.kind == "reexport" || entry.kind == "re-export" || entry.kind == "__all__" {
            continue;
        }
        symbol_map.entry(symbol_name.clone()).or_default().push((
            file_path.clone(),
            entry.line,
            entry.kind.clone(),
            entry.import_count,
        ));
    }

    // Get frameworks slice for filtering
    let fw_slice = frameworks.unwrap_or(&[]);

    // Filter to only symbols exported from multiple files
    let mut twins: Vec<ExactTwin> = Vec::new();

    for (name, locations_raw) in symbol_map {
        // Skip generic method names (new, from, clone, etc.)
        if is_generic_method(&name) {
            continue;
        }

        // Skip if only one location (not a duplicate)
        if locations_raw.len() <= 1 {
            continue;
        }

        // Framework convention filtering:
        // If ALL locations are framework conventions for the same export,
        // it's an intentional pattern (e.g., GET in every +server.ts)
        if !fw_slice.is_empty() {
            let all_are_conventions = locations_raw.iter().all(|(file_path, _, _, _)| {
                crate::analyzer::frameworks::is_framework_convention(&name, file_path, fw_slice)
            });
            if all_are_conventions {
                continue;
            }
        }

        // Build locations with import counts and signature fingerprints
        let mut locations: Vec<TwinLocation> = locations_raw
            .iter()
            .map(|(file, line, kind, import_count)| {
                // Compute signature fingerprint for functions (including const arrow functions)
                // TS arrow functions: export const foo = () => {}
                // Named exports that might be functions
                let signature_fingerprint = if kind == "function"
                    || kind == "var"
                    || kind == "decl"
                    || kind == "const"
                    || kind == "named"
                {
                    compute_signature_fingerprint(analyses, file, &name)
                } else {
                    None
                };

                TwinLocation {
                    file_path: file.clone(),
                    line: *line,
                    kind: kind.clone(),
                    import_count: *import_count,
                    is_canonical: false, // Will determine below
                    signature_fingerprint,
                }
            })
            .collect();

        // Compute signature similarity across all locations
        let fingerprints: Vec<Option<String>> = locations
            .iter()
            .map(|l| l.signature_fingerprint.clone())
            .collect();
        let signature_similarity = compute_group_similarity(&fingerprints);

        // Same method name on different impl types (trait impls, inherent
        // methods, enum methods). Kept in the result as informational
        // NAME_COLLISION / IDIOM — name-collision has audit value (systemic
        // twin-pairs), but it is never narrated as a duplicate to consolidate.
        let all_methods = locations.iter().all(|l| {
            l.kind == "method" || impl_method_set.contains(&(l.file_path.as_str(), name.as_str()))
        });

        // Class B: Token from a comment block flagged as a type declaration.
        let mut is_comment = false;
        let mut source_unknown = false;
        for loc in &locations {
            if let Some(content) = sources.read(&loc.file_path)
                && let Some(line) = content.lines().nth(loc.line.saturating_sub(1))
            {
                let trimmed = line.trim();
                if trimmed.starts_with("//")
                    || trimmed.starts_with("/*")
                    || trimmed.starts_with("*")
                    || (trimmed.starts_with('#')
                        && matches!(
                            std::path::Path::new(&loc.file_path)
                                .extension()
                                .and_then(|e| e.to_str()),
                            Some("py" | "pyi" | "sh" | "bash" | "zsh")
                        ))
                {
                    is_comment = true;
                    break;
                }
            } else {
                source_unknown = true;
            }
        }
        if is_comment {
            continue;
        }

        // Determine canonical location:
        // 1. Most imports
        // 2. If tie, shortest path (likely more central)
        // 3. If still tie, first alphabetically (deterministic)
        if !locations.is_empty() {
            let max_imports = locations.iter().map(|l| l.import_count).max().unwrap_or(0);

            let mut canonicals: Vec<&mut TwinLocation> = locations
                .iter_mut()
                .filter(|l| l.import_count == max_imports)
                .collect();

            // If multiple have max imports, pick shortest path
            if canonicals.len() > 1 {
                canonicals.sort_by_key(|l| l.file_path.len());
            }

            // Mark first as canonical
            if let Some(canonical) = canonicals.first_mut() {
                canonical.is_canonical = true;
            }
        }

        let mut kinds: Vec<_> = locations.iter().map(|l| &l.kind).collect();
        kinds.sort();
        kinds.dedup();
        let different_kinds = kinds.len() > 1;

        let is_mirror = locations
            .iter()
            .any(|l| l.file_path.contains("/ffi/") || l.file_path.contains("/bindings/"))
            && locations.iter().any(|l| {
                l.file_path.contains("/ipc/")
                    || l.file_path.contains("/client/")
                    || l.file_path.contains("ipc_client")
            });

        // F3: per-location receiver/impl-type, when `impl_methods` recorded
        // one for that (file, name). Used only to REFUSE a false EXACT
        // (never to force one) when the fingerprint folds two different
        // receivers' shapes onto the same string.
        let impl_qualifiers: Vec<Option<&str>> = locations
            .iter()
            .map(|loc| {
                analyses
                    .iter()
                    .find(|a| a.path == loc.file_path)
                    .and_then(|a| a.impl_methods.iter().find(|m| m.name == name))
                    .map(|m| m.qualifier.as_str())
            })
            .collect();

        let class = classify_twin_class(
            &name,
            &locations,
            signature_similarity,
            all_methods,
            &impl_qualifiers,
        );

        // Legacy classification stays in the contract; derive it consistently
        // with the shape-evidence class: only shape-backed twins are
        // "duplicate", everything without evidence is "namesake".
        let classification = if is_mirror {
            TwinClassification::Mirror
        } else if different_kinds {
            TwinClassification::Namesake
        } else {
            match class {
                TwinClass::Exact | TwinClass::ShapeSimilar => TwinClassification::Duplicate,
                TwinClass::NameCollision | TwinClass::Idiom => TwinClassification::Namesake,
            }
        };

        let shape_match = locations_have_shape_match(&locations);
        let single_module_target = sources.single_module_for(analyses, &locations);
        twins.push(ExactTwin {
            name,
            locations,
            signature_similarity,
            classification,
            class,
            shape_match,
            single_module_target,
            exclude_from_score: (!shape_match && single_module_target)
                || (source_unknown && matches!(class, TwinClass::Exact | TwinClass::ShapeSimilar)),
        });
    }

    // Sort by number of locations (most duplicated first)
    twins.sort_by_key(|b| std::cmp::Reverse(b.locations.len()));

    twins
}

/// Print exact twins in human-readable format
pub fn print_exact_twins_human(twins: &[ExactTwin]) {
    if twins.is_empty() {
        println!("No exact twins found - all symbol names are unique!");
        return;
    }

    // Classify by recommendation, not just language. Namesakes and mirrors can
    // be same-language, but they must not be narrated as consolidation work.
    let duplicates: Vec<_> = twins
        .iter()
        .filter(|twin| twin.classification == TwinClassification::Duplicate)
        .collect();
    let non_duplicates: Vec<_> = twins
        .iter()
        .filter(|twin| twin.classification != TwinClassification::Duplicate)
        .collect();

    println!("EXACT TWINS ({} found)", twins.len());
    println!();

    if !duplicates.is_empty() {
        println!(
            "  [!] DUPLICATES ({} groups) - likely need consolidation:",
            duplicates.len()
        );
        println!();
        for twin in &duplicates {
            print_twin_details(twin);
        }
    }

    if !non_duplicates.is_empty() {
        println!(
            "  [i] NAMESAKES / MIRRORS ({} groups) - do not consolidate blindly:",
            non_duplicates.len()
        );
        println!();
        for twin in &non_duplicates {
            print_twin_details(twin);
        }
    }

    println!("Summary:");
    println!("  Duplicate groups: {} (actionable)", duplicates.len());
    println!(
        "  Namesake / mirror groups: {} (verify or rename)",
        non_duplicates.len()
    );
    let total_dups: usize = twins.iter().map(|t| t.locations.len()).sum();
    println!("  Total duplicate definitions: {}", total_dups);
}

/// Helper to print individual twin details
fn print_twin_details(twin: &ExactTwin) {
    println!("  Symbol: {}", twin.name);
    println!("    Classification: {:?}", twin.classification);
    println!("    Class: {}", twin.class.as_str());
    println!("    Action: {}", twin_action(twin));
    for loc in &twin.locations {
        let canonical_marker = if loc.is_canonical { " CANONICAL" } else { "" };
        println!(
            "    ├─ {}:{} ({}) - {} imports{}",
            loc.file_path, loc.line, loc.kind, loc.import_count, canonical_marker
        );
    }
    // Add suggestion based on import counts
    let zero_import_count = twin
        .locations
        .iter()
        .filter(|l| l.import_count == 0)
        .count();
    if zero_import_count > 0 && zero_import_count < twin.locations.len() {
        println!(
            "    └─ [TIP] {} location(s) have 0 imports - candidates for removal or consolidation",
            zero_import_count
        );
    }
    println!();
}

/// Print exact twins in JSON format
pub fn print_exact_twins_json(twins: &[ExactTwin]) {
    // Categorize twins
    let (same_lang, cross_lang): (Vec<_>, Vec<_>) = twins
        .iter()
        .partition(|twin| matches!(categorize_twin(twin), TwinCategory::SameLanguage(_)));

    // Count twins with high signature similarity (likely real duplicates)
    let high_similarity_count = twins
        .iter()
        .filter(|t| t.signature_similarity.map(|s| s >= 0.8).unwrap_or(false))
        .count();

    let twin_to_json = |twin: &ExactTwin| {
        let category = categorize_twin(twin);
        let mut json = serde_json::json!({
            "name": twin.name,
            "category": match category {
                TwinCategory::SameLanguage(lang) => format!("same_language:{:?}", lang).to_lowercase(),
                TwinCategory::CrossLanguage => "cross_language".to_string(),
                TwinCategory::Namesake => "namesake".to_string(),
            },
            "classification": twin.classification,
            "class": twin.class,
            "shape_match": twin.shape_match,
            "single_module_target": twin.single_module_target,
            "exclude_from_score": twin.exclude_from_score,
            "action": twin_action(twin),
            "locations": twin.locations.iter().map(|loc| {
                let mut loc_json = serde_json::json!({
                    "file": loc.file_path,
                    "line": loc.line,
                    "kind": loc.kind,
                    "imports": loc.import_count,
                    "canonical": loc.is_canonical,
                    "language": format!("{:?}", detect_language(&loc.file_path)).to_lowercase(),
                });
                // Add signature fingerprint if present
                if let Some(ref fp) = loc.signature_fingerprint {
                    loc_json["signature_fingerprint"] = serde_json::json!(fp);
                }
                loc_json
            }).collect::<Vec<_>>(),
        });
        // Add signature similarity if computed
        if let Some(sim) = twin.signature_similarity {
            json["signature_similarity"] = serde_json::json!(sim);
        }
        json
    };

    let output = serde_json::json!({
        "exact_twins": twins.iter().map(twin_to_json).collect::<Vec<_>>(),
        "summary": {
            "total_groups": twins.len(),
            "same_language_groups": same_lang.len(),
            "cross_language_groups": cross_lang.len(),
            "high_similarity_groups": high_similarity_count,
            "total_duplicates": twins.iter().map(|t| t.locations.len()).sum::<usize>(),
        }
    });

    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}

/// Print exact twins based on output mode
pub fn print_exact_twins(twins: &[ExactTwin], output: OutputMode) {
    match output {
        OutputMode::Json | OutputMode::Jsonl => print_exact_twins_json(twins),
        OutputMode::Human => print_exact_twins_human(twins),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ExportSymbol, ImportEntry, ImportKind, ImportSymbol};

    #[test]
    fn group_similarity_is_bit_exact_for_every_fingerprint_permutation() {
        fn permute(values: &mut [Option<String>], start: usize, expected: u32) {
            if start == values.len() {
                assert_eq!(
                    compute_group_similarity(values)
                        .expect("pair mean")
                        .to_bits(),
                    expected
                );
                return;
            }
            for i in start..values.len() {
                values.swap(start, i);
                permute(values, start + 1, expected);
                values.swap(start, i);
            }
        }
        let mut values = ["A,B,C|D", "A|B", "C|D,E", "A,B,C|D", "Z|Q", "A|D,E,F"]
            .map(|s| Some(s.to_string()))
            .to_vec();
        values.push(None);
        let expected = compute_group_similarity(&values)
            .expect("pair mean")
            .to_bits();
        permute(&mut values, 0, expected);
        assert_eq!(compute_group_similarity(&[None, Some("A|B".into())]), None);
        assert_eq!(
            compute_group_similarity(&[Some("A|B".into()), Some("A|B".into())]),
            Some(1.0)
        );
    }

    fn mock_file_with_exports(path: &str, exports: Vec<(&str, &str)>) -> FileAnalysis {
        FileAnalysis {
            path: path.to_string(),
            exports: exports
                .into_iter()
                .enumerate()
                .map(|(i, (name, kind))| ExportSymbol {
                    name: name.to_string(),
                    kind: kind.to_string(),
                    export_type: "named".to_string(),
                    line: Some(i + 1),
                    params: Vec::new(),

                    symbol_id: crate::types::SymbolIdV1::default(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn test_build_symbol_registry_empty() {
        let analyses: Vec<FileAnalysis> = vec![];
        let registry = build_symbol_registry(&analyses, false);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_build_symbol_registry_no_imports() {
        let analyses = vec![
            mock_file_with_exports("a.ts", vec![("foo", "function")]),
            mock_file_with_exports("b.ts", vec![("bar", "function")]),
        ];

        let registry = build_symbol_registry(&analyses, false);
        assert_eq!(registry.len(), 2);

        let foo_entry = registry
            .get(&("a.ts".to_string(), "foo".to_string()))
            .unwrap();
        assert_eq!(foo_entry.import_count, 0);
    }

    #[test]
    fn test_build_symbol_registry_with_imports() {
        let exporter = mock_file_with_exports("utils.ts", vec![("helper", "function")]);
        let mut importer = FileAnalysis {
            path: "app.ts".to_string(),
            ..Default::default()
        };

        let mut import = ImportEntry::new("./utils".to_string(), ImportKind::Static);
        import.resolved_path = Some("utils.ts".to_string());
        import.symbols.push(ImportSymbol {
            name: "helper".to_string(),
            alias: None,
            is_default: false,
        });
        importer.imports.push(import);

        let registry = build_symbol_registry(&[exporter, importer], false);

        let helper_entry = registry
            .get(&("utils.ts".to_string(), "helper".to_string()))
            .unwrap();
        assert_eq!(helper_entry.import_count, 1);
    }

    #[test]
    fn test_build_symbol_registry_skips_tests() {
        let test_file = FileAnalysis {
            path: "tests/test_api_integration.py".to_string(),
            is_test: true,
            exports: vec![ExportSymbol {
                name: "TestHealthEndpoints".to_string(),
                kind: "class".to_string(),
                export_type: "named".to_string(),
                line: Some(10),
                params: Vec::new(),

                symbol_id: crate::types::SymbolIdV1::default(),
            }],
            ..Default::default()
        };
        let normal_file = mock_file_with_exports("app.py", vec![("App", "class")]);

        // With include_tests=false, test files should be skipped
        let registry = build_symbol_registry(&[test_file.clone(), normal_file.clone()], false);
        assert_eq!(registry.len(), 1);
        assert!(registry.contains_key(&("app.py".to_string(), "App".to_string())));

        // With include_tests=true, test files should be included
        let registry_with_tests = build_symbol_registry(&[test_file, normal_file], true);
        assert_eq!(registry_with_tests.len(), 2);
    }

    #[test]
    fn test_build_symbol_registry_ignores_shell_env_and_make_targets() {
        let shell = FileAnalysis {
            path: "src/install.sh".to_string(),
            language: "shell".to_string(),
            exports: vec![
                ExportSymbol::new("usage".to_string(), "function", "named", Some(1)),
                ExportSymbol::new("PATH".to_string(), "env", "named", Some(5)),
            ],
            ..Default::default()
        };
        let makefile = FileAnalysis {
            path: "Makefile".to_string(),
            language: "make".to_string(),
            exports: vec![
                ExportSymbol::new("help".to_string(), "target", "named", Some(1)),
                ExportSymbol::new(".PHONY".to_string(), "special_target", "named", Some(3)),
            ],
            ..Default::default()
        };

        let registry = build_symbol_registry(&[shell], false);
        assert!(registry.contains_key(&("src/install.sh".to_string(), "usage".to_string())));
        assert!(!registry.contains_key(&("src/install.sh".to_string(), "PATH".to_string())));

        let registry = build_symbol_registry(&[makefile], false);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_find_dead_parrots_respects_shell_local_uses() {
        let shell = FileAnalysis {
            path: "src/vetcoders.sh".to_string(),
            language: "shell".to_string(),
            exports: vec![
                ExportSymbol::new(
                    "_vetcoders_spawn_script".to_string(),
                    "function",
                    "named",
                    Some(1),
                ),
                ExportSymbol::new("_unused_private".to_string(), "function", "named", Some(8)),
            ],
            local_uses: vec!["_vetcoders_spawn_script".to_string()],
            ..Default::default()
        };

        let result = find_dead_parrots(&[shell], true, false);

        assert!(
            !result
                .dead_parrots
                .iter()
                .any(|entry| entry.name == "_vetcoders_spawn_script")
        );
        assert!(
            result
                .dead_parrots
                .iter()
                .any(|entry| entry.name == "_unused_private")
        );
    }

    #[test]
    fn test_find_dead_parrots() {
        let used_file = mock_file_with_exports("used.ts", vec![("used", "function")]);
        let dead_file = mock_file_with_exports("dead.ts", vec![("unused", "function")]);

        let mut importer = FileAnalysis {
            path: "app.ts".to_string(),
            ..Default::default()
        };

        let mut import = ImportEntry::new("./used".to_string(), ImportKind::Static);
        import.resolved_path = Some("used.ts".to_string());
        import.symbols.push(ImportSymbol {
            name: "used".to_string(),
            alias: None,
            is_default: false,
        });
        importer.imports.push(import);

        let result = find_dead_parrots(&[used_file, dead_file, importer], true, false);

        assert_eq!(result.dead_parrots.len(), 1);
        assert_eq!(result.dead_parrots[0].name, "unused");
        assert_eq!(result.total_symbols, 2);
    }

    #[test]
    fn test_find_dead_parrots_skips_dynamic_imports() {
        // Create a component that is dynamically imported via React lazy()
        let lazy_component = FileAnalysis {
            path: "src/components/PasswordResetModal.tsx".to_string(),
            exports: vec![ExportSymbol {
                name: "PasswordResetModal".to_string(),
                kind: "function".to_string(),
                export_type: "default".to_string(),
                line: Some(23),
                params: Vec::new(),

                symbol_id: crate::types::SymbolIdV1::default(),
            }],
            ..Default::default()
        };

        // Create a file that imports it dynamically
        let importer = FileAnalysis {
            path: "src/App.tsx".to_string(),
            dynamic_imports: vec!["./components/PasswordResetModal".to_string()],
            ..Default::default()
        };

        // Create a truly dead file (no static or dynamic imports)
        let dead_file = mock_file_with_exports("dead.ts", vec![("unused", "function")]);

        let result = find_dead_parrots(&[lazy_component, importer, dead_file], true, false);

        // The dynamic import should NOT be marked as dead
        // Only the truly unused export should be in dead_parrots
        assert_eq!(result.dead_parrots.len(), 1);
        assert_eq!(result.dead_parrots[0].name, "unused");
    }

    // Exact twin detection tests
    #[test]
    fn test_detect_exact_twins_no_duplicates() {
        let analyses = vec![
            mock_file_with_exports("a.ts", vec![("foo", "function")]),
            mock_file_with_exports("b.ts", vec![("bar", "function")]),
        ];

        let twins = detect_exact_twins(&analyses, false);
        assert!(twins.is_empty());
    }

    #[test]
    fn test_detect_exact_twins_simple() {
        let analyses = vec![
            mock_file_with_exports("a.ts", vec![("Button", "class")]),
            mock_file_with_exports("b.ts", vec![("Button", "class")]),
        ];

        let twins = detect_exact_twins(&analyses, false);
        assert_eq!(twins.len(), 1);
        assert_eq!(twins[0].name, "Button");
        assert_eq!(twins[0].locations.len(), 2);
    }

    #[test]
    fn test_detect_exact_twins_canonical_by_path() {
        let analyses = vec![
            mock_file_with_exports("shared/types.ts", vec![("Message", "type")]),
            mock_file_with_exports("hooks/useChat.ts", vec![("Message", "type")]),
        ];

        let twins = detect_exact_twins(&analyses, false);
        assert_eq!(twins.len(), 1);

        // Canonical should be shortest path
        let canonical = twins[0].locations.iter().find(|l| l.is_canonical).unwrap();
        assert_eq!(canonical.file_path, "shared/types.ts");
    }

    #[test]
    fn test_detect_exact_twins_canonical_by_imports() {
        let a = mock_file_with_exports("a.ts", vec![("Foo", "type")]);
        let b = mock_file_with_exports("b.ts", vec![("Foo", "type")]);

        // Import from a.ts
        let mut importer = FileAnalysis {
            path: "app.ts".to_string(),
            ..Default::default()
        };
        let mut import = ImportEntry::new("./a".to_string(), ImportKind::Static);
        import.resolved_path = Some("a.ts".to_string());
        import.symbols.push(ImportSymbol {
            name: "Foo".to_string(),
            alias: None,
            is_default: false,
        });
        importer.imports.push(import);

        let twins = detect_exact_twins(&[a, b, importer], false);
        assert_eq!(twins.len(), 1);

        // Canonical should be the one with imports (a.ts)
        let canonical = twins[0].locations.iter().find(|l| l.is_canonical).unwrap();
        assert_eq!(canonical.file_path, "a.ts");
        assert_eq!(canonical.import_count, 1);
    }

    #[test]
    fn test_detect_exact_twins_three_locations() {
        let analyses = vec![
            mock_file_with_exports("a.ts", vec![("Common", "type")]),
            mock_file_with_exports("b.ts", vec![("Common", "type")]),
            mock_file_with_exports("c.ts", vec![("Common", "type")]),
        ];

        let twins = detect_exact_twins(&analyses, false);
        assert_eq!(twins.len(), 1);
        assert_eq!(twins[0].locations.len(), 3);
    }

    #[test]
    fn test_twin_action_duplicate_recommends_consolidate() {
        let twin = ExactTwin {
            name: "foo".to_string(),
            locations: vec![],
            signature_similarity: Some(1.0),
            classification: TwinClassification::Duplicate,
            class: TwinClass::Exact,
            shape_match: true,
            single_module_target: false,
            exclude_from_score: false,
        };
        assert_eq!(twin_action(&twin), "consolidate into single module");
    }

    #[test]
    fn test_twin_action_namesake_flags_false_twin() {
        let twin = ExactTwin {
            name: "foo".to_string(),
            locations: vec![],
            signature_similarity: Some(0.0),
            classification: TwinClassification::Namesake,
            class: TwinClass::NameCollision,
            shape_match: false,
            single_module_target: false,
            exclude_from_score: false,
        };
        let namesake = twin_action(&twin);
        assert!(namesake.contains("rename"));
        assert!(namesake.contains("disambiguate"));
    }

    #[test]
    fn test_twin_action_mirror_requires_verification() {
        let twin = ExactTwin {
            name: "foo".to_string(),
            locations: vec![],
            signature_similarity: None,
            classification: TwinClassification::Mirror,
            class: TwinClass::NameCollision,
            shape_match: false,
            single_module_target: false,
            exclude_from_score: false,
        };
        let mirror = twin_action(&twin);
        assert!(mirror.contains("verify"));
        assert!(mirror.contains("contract"));
    }

    #[test]
    fn test_twin_action_never_consolidates_without_shape_evidence() {
        for class in [
            TwinClass::ShapeSimilar,
            TwinClass::NameCollision,
            TwinClass::Idiom,
        ] {
            let twin = ExactTwin {
                name: "foo".to_string(),
                locations: vec![],
                signature_similarity: None,
                classification: TwinClassification::Duplicate,
                class,
                shape_match: false,
                single_module_target: false,
                exclude_from_score: false,
            };
            assert!(
                !twin_action(&twin).starts_with("consolidate"),
                "class {:?} must not earn a consolidate action",
                class
            );
        }
    }

    #[test]
    fn test_detect_exact_twins_classifies_method_name_parallelism_as_collision() {
        let analyses = vec![
            mock_file_with_exports("a.rs", vec![("summary_line", "method")]),
            mock_file_with_exports("b.rs", vec![("summary_line", "method")]),
        ];

        let twins = detect_exact_twins(&analyses, false);

        // Same method names on different impl types stay visible but are
        // classified NAME_COLLISION (informational), never consolidate fodder.
        assert_eq!(twins.len(), 1);
        assert_eq!(twins[0].class, TwinClass::NameCollision);
        assert_eq!(twins[0].classification, TwinClassification::Namesake);
        assert!(!twin_action(&twins[0]).starts_with("consolidate"));
    }

    #[test]
    fn test_detect_exact_twins_classifies_idiom_constructors() {
        let analyses = vec![
            mock_file_with_exports("a.rs", vec![("from_env", "method")]),
            mock_file_with_exports("b.rs", vec![("from_env", "method")]),
        ];

        let twins = detect_exact_twins(&analyses, false);

        assert_eq!(twins.len(), 1);
        assert_eq!(twins[0].class, TwinClass::Idiom);
        assert!(!twin_action(&twins[0]).starts_with("consolidate"));
    }

    #[test]
    fn test_detect_exact_twins_no_fingerprints_never_exact() {
        // Two structs with the same name: extraction carries no field data,
        // so the class must be NAME_COLLISION — never EXACT on name alone.
        let twins = detect_exact_twins(
            &[
                mock_file_with_exports("a.rs", vec![("QuickWin", "struct")]),
                mock_file_with_exports("b.rs", vec![("QuickWin", "struct")]),
            ],
            false,
        );

        assert_eq!(twins.len(), 1);
        assert_eq!(twins[0].class, TwinClass::NameCollision);
        assert_ne!(twins[0].class, TwinClass::Exact);
        assert!(!twin_action(&twins[0]).starts_with("consolidate"));
    }

    #[test]
    fn test_classify_twin_class_exact_requires_identical_fingerprints() {
        let loc = |fp: Option<&str>| TwinLocation {
            file_path: "x.rs".to_string(),
            line: 1,
            kind: "function".to_string(),
            import_count: 0,
            is_canonical: false,
            signature_fingerprint: fp.map(|s| s.to_string()),
        };

        // Identical fingerprints everywhere -> EXACT
        let locs = vec![loc(Some("Cfg|Out")), loc(Some("Cfg|Out"))];
        assert_eq!(
            classify_twin_class("render_widget", &locs, Some(1.0), false, &[]),
            TwinClass::Exact
        );

        // Overlapping but different -> SHAPE_SIMILAR
        let locs = vec![loc(Some("Cfg|Out")), loc(Some("Cfg,Extra|Out"))];
        assert_eq!(
            classify_twin_class("render_widget", &locs, Some(0.66), false, &[]),
            TwinClass::ShapeSimilar
        );

        // Missing fingerprint at one location -> NAME_COLLISION, never EXACT
        let locs = vec![loc(Some("Cfg|Out")), loc(None)];
        assert_eq!(
            classify_twin_class("render_widget", &locs, Some(1.0), false, &[]),
            TwinClass::NameCollision
        );
    }

    /// F3 RED: two methods, same name, same folded param/return-type
    /// fingerprint, but on DIFFERENT receiver structs (a delegation chain
    /// like `Screen::resize_pane_with_id` calling
    /// `Tab::resize_pane_with_id`). The fingerprint alone cannot see this —
    /// it never compares receivers or bodies — so this must never earn
    /// EXACT/"consolidate".
    #[test]
    fn test_classify_twin_class_refuses_exact_when_receivers_differ() {
        let loc = |fp: Option<&str>| TwinLocation {
            file_path: "x.rs".to_string(),
            line: 1,
            kind: "function".to_string(),
            import_count: 0,
            is_canonical: false,
            signature_fingerprint: fp.map(|s| s.to_string()),
        };
        let locs = vec![
            loc(Some("PaneId|Result<()>")),
            loc(Some("PaneId|Result<()>")),
        ];

        // No receiver info known -> unchanged behavior, still EXACT.
        assert_eq!(
            classify_twin_class("resize_pane_with_id", &locs, Some(1.0), false, &[]),
            TwinClass::Exact
        );

        // Same identical fingerprints, but the two locations are methods on
        // different receiver structs. Must never classify as EXACT.
        let qualifiers = [Some("Screen"), Some("Tab")];
        let class =
            classify_twin_class("resize_pane_with_id", &locs, Some(1.0), false, &qualifiers);
        assert_ne!(
            class,
            TwinClass::Exact,
            "different receivers must never classify as EXACT"
        );
        assert_eq!(class, TwinClass::NameCollision);
        assert!(
            !twin_action(&ExactTwin {
                name: "resize_pane_with_id".to_string(),
                locations: locs,
                signature_similarity: Some(1.0),
                classification: TwinClassification::Namesake,
                class,
                shape_match: true,
                single_module_target: false,
                exclude_from_score: false,
            })
            .starts_with("consolidate")
        );
    }

    /// F3: Swift/Zig files must be recognized by `detect_language`, not fall
    /// back to `Other` — real language identity for `same_language`/
    /// `cross_language` bucketing and for the JSON report's per-location
    /// `language` field (both derived from this function).
    ///
    /// Note (known limit, documented rather than faked): `TwinLocation` has
    /// no `language` field of its own — `audit_report.rs` and `for_ai.rs`
    /// construct it directly (outside this module's edit scope for this
    /// fix), so threading `FileAnalysis::language` onto the struct would
    /// require touching those call sites too. This test locks in the
    /// narrower, still-real fix: the extension table itself stops
    /// collapsing Swift/Zig into "other".
    #[test]
    fn detect_language_recognizes_swift_and_zig() {
        assert_eq!(detect_language("Sources/App/Widget.swift"), Language::Swift);
        assert_eq!(detect_language("src/main.zig"), Language::Zig);
        assert_ne!(detect_language("Sources/App/Widget.swift"), Language::Other);
        assert_ne!(detect_language("src/main.zig"), Language::Other);
    }

    #[test]
    fn test_detect_exact_twins_classifies_kind_mismatch_as_namesake() {
        let twins = detect_exact_twins(
            &[
                mock_file_with_exports("a.rs", vec![("CliOptions", "struct")]),
                mock_file_with_exports("b.rs", vec![("CliOptions", "trait")]),
            ],
            false,
        );

        assert_eq!(twins.len(), 1);
        assert_eq!(twins[0].classification, TwinClassification::Namesake);
        assert!(twin_action(&twins[0]).contains("rename"));
    }

    #[test]
    fn test_detect_exact_twins_classifies_ffi_ipc_pair_as_mirror() {
        let twins = detect_exact_twins(
            &[
                mock_file_with_exports(
                    "shell-agent/ffi/src/lib.rs",
                    vec![("restart_service", "function")],
                ),
                mock_file_with_exports(
                    "tray-agent/src/ipc_client.rs",
                    vec![("restart_service", "function")],
                ),
            ],
            false,
        );

        assert_eq!(twins.len(), 1);
        assert_eq!(twins[0].classification, TwinClassification::Mirror);
        assert!(twin_action(&twins[0]).contains("do not consolidate"));
    }

    fn write_package_swift(dir: &std::path::Path, body: &str) -> FileAnalysis {
        let path = dir.join("Package.swift");
        std::fs::write(&path, body).expect("write Package.swift");
        FileAnalysis {
            path: path.to_string_lossy().into_owned(),
            ..Default::default()
        }
    }

    /// Shapeless name collisions carry `shape_match = false`.
    #[test]
    fn namesake_shapeless_groups_set_shape_match_false() {
        let twins = detect_exact_twins(
            &[
                mock_file_with_exports("a.swift", vec![("text", "let")]),
                mock_file_with_exports("b.swift", vec![("text", "let")]),
            ],
            false,
        );
        assert_eq!(twins.len(), 1);
        assert!(!twins[0].shape_match);
        assert!(!twins[0].single_module_target);
        assert!(!twins[0].exclude_from_score);
        assert_eq!(twins[0].classification, TwinClassification::Namesake);
        assert!(
            omit_from_duplicate_groups(&twins[0]),
            "namesake classification is the findings skip, even without a one-target manifest"
        );
    }

    /// One SwiftPM product target is a manifest fact, not a language assumption.
    #[test]
    fn namesake_single_module_package_swift_sets_exclude_from_score() {
        // Unique, 0700, owned by this test and removed on drop — a predictable
        // name under the shared temp dir is exactly what Semgrep's temp-dir rule
        // flags, and `tempfile` is already a dependency.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let manifest = write_package_swift(
            &dir,
            r#"
import PackageDescription
let package = Package(
    name: "NamesakeApp",
    targets: [.executableTarget(name: "UI", path: "Sources/UI")]
)
"#,
        );
        let twins = detect_exact_twins_with_sources(
            &[
                manifest,
                mock_file_with_exports("Sources/UI/A.swift", vec![("text", "let")]),
                mock_file_with_exports("Sources/UI/B.swift", vec![("text", "let")]),
            ],
            false,
            None,
            &mut TwinSources::for_root(&dir),
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(twins.len(), 1, "sensor must still list the group");
        assert!(!twins[0].shape_match);
        assert!(twins[0].single_module_target);
        assert!(twins[0].exclude_from_score);
    }

    /// Two SwiftPM targets: suppression must not fire, even for shapeless namesakes.
    #[test]
    fn namesake_multi_target_package_swift_does_not_suppress() {
        // Unique, 0700, owned by this test and removed on drop — a predictable
        // name under the shared temp dir is exactly what Semgrep's temp-dir rule
        // flags, and `tempfile` is already a dependency.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let manifest = write_package_swift(
            &dir,
            r#"
import PackageDescription
let package = Package(
    name: "NamesakeApp",
    targets: [
        .target(name: "App"),
        .target(name: "Kit"),
        .testTarget(name: "AppTests", dependencies: ["App"]),
    ]
)
"#,
        );
        let twins = detect_exact_twins_with_sources(
            &[
                manifest,
                mock_file_with_exports("Sources/App/A.swift", vec![("text", "let")]),
                mock_file_with_exports("Sources/Kit/B.swift", vec![("text", "let")]),
            ],
            false,
            None,
            &mut TwinSources::for_root(&dir),
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(twins.len(), 1, "sensor must still list the group");
        assert!(!twins[0].shape_match);
        assert!(
            !twins[0].single_module_target,
            "two .target( declarations are not a single module"
        );
        assert!(
            !twins[0].exclude_from_score,
            "multi-target projects must not suppress shapeless namesakes"
        );
    }

    /// `loct twins` keeps every group; only metadata changes.
    #[test]
    fn namesake_sensor_keeps_all_groups() {
        let twins = detect_exact_twins(
            &[
                mock_file_with_exports(
                    "A.swift",
                    vec![("text", "let"), ("body", "var"), ("makeNSView", "func")],
                ),
                mock_file_with_exports(
                    "B.swift",
                    vec![("text", "let"), ("body", "var"), ("makeNSView", "func")],
                ),
            ],
            false,
        );
        assert_eq!(twins.len(), 3);
        assert!(twins.iter().all(|t| !t.shape_match));
        assert!(twins.iter().all(|t| !t.exclude_from_score));
    }

    #[test]
    fn namesake_matching_fingerprints_are_not_shapeless() {
        let loc = |fp: &str| TwinLocation {
            file_path: "x.rs".to_string(),
            line: 1,
            kind: "function".to_string(),
            import_count: 0,
            is_canonical: false,
            signature_fingerprint: Some(fp.to_string()),
        };
        assert!(locations_have_shape_match(&[
            loc("String|View"),
            loc("String|View")
        ]));
        assert!(!locations_have_shape_match(&[
            loc("String|View"),
            loc("Int|View")
        ]));
        assert!(!locations_have_shape_match(&[
            loc("String|View"),
            TwinLocation {
                file_path: "y.rs".to_string(),
                line: 2,
                kind: "function".to_string(),
                import_count: 0,
                is_canonical: false,
                signature_fingerprint: None,
            }
        ]));
    }

    #[test]
    fn namesake_test_target_does_not_count_as_product_module() {
        let src = r#"
            targets: [
                .executableTarget(name: "App"),
                .testTarget(name: "AppTests", dependencies: ["App"]),
            ]
        "#;
        assert_eq!(count_swiftpm_product_targets(src), 1);
        let commented = "// .target(name: \"Ghost\")\n.executableTarget(name: \"App\")\n";
        assert_eq!(count_swiftpm_product_targets(commented), 1);
    }

    fn page_twin(name: &str, class: TwinClass, locations: usize) -> ExactTwin {
        ExactTwin {
            name: name.to_string(),
            locations: (0..locations)
                .map(|i| TwinLocation {
                    file_path: format!("{name}-{i}.rs"),
                    line: 1,
                    kind: "function".to_string(),
                    import_count: 0,
                    is_canonical: i == 0,
                    signature_fingerprint: None,
                })
                .collect(),
            signature_similarity: None,
            classification: TwinClassification::Namesake,
            class,
            shape_match: false,
            single_module_target: false,
            exclude_from_score: false,
        }
    }

    #[test]
    fn parse_twin_class_filter_accepts_tokens_and_family_aliases() {
        assert_eq!(
            parse_twin_class_filter("structural").unwrap(),
            vec![TwinClass::Exact, TwinClass::ShapeSimilar]
        );
        assert_eq!(
            parse_twin_class_filter("naming").unwrap(),
            vec![TwinClass::NameCollision, TwinClass::Idiom]
        );
        assert_eq!(
            parse_twin_class_filter("exact, SHAPE_SIMILAR, idiom").unwrap(),
            vec![TwinClass::Exact, TwinClass::ShapeSimilar, TwinClass::Idiom]
        );
        let err = parse_twin_class_filter("NOPE").unwrap_err();
        assert!(err.contains("unknown twin class"), "{err}");
    }

    #[test]
    fn order_twins_for_page_puts_structural_classes_before_naming() {
        let mut twins = vec![
            page_twin("body", TwinClass::NameCollision, 20),
            page_twin("keep_me_visible", TwinClass::Exact, 2),
            page_twin("from_env", TwinClass::Idiom, 8),
            page_twin("shape", TwinClass::ShapeSimilar, 3),
            page_twin("exact_small", TwinClass::Exact, 2),
        ];
        // Cold traversal order and stale canonical ties must not leak into
        // a later page's location prefix.
        for twin in &mut twins {
            twin.locations.reverse();
            for location in &mut twin.locations {
                location.is_canonical = true;
            }
        }
        order_twins_for_page(&mut twins);
        let names: Vec<_> = twins.iter().map(|twin| twin.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "exact_small",
                "keep_me_visible",
                "shape",
                "body",
                "from_env",
            ]
        );
        for twin in &twins {
            assert!(twin.locations[0].is_canonical);
            assert!(
                twin.locations[1..]
                    .iter()
                    .all(|location| !location.is_canonical)
            );
            assert_eq!(twin.locations[0].file_path, format!("{}-0.rs", twin.name));
        }
    }

    #[test]
    fn class_filter_runs_before_the_page_window() {
        let mut twins = vec![
            page_twin("body", TwinClass::NameCollision, 20),
            page_twin("keep_me_visible", TwinClass::Exact, 2),
            page_twin("shape", TwinClass::ShapeSimilar, 4),
        ];
        order_twins_for_page(&mut twins);
        let classes = parse_twin_class_filter("structural").unwrap();
        retain_twin_classes(&mut twins, &classes);
        assert!(
            twins
                .iter()
                .all(|twin| { matches!(twin.class, TwinClass::Exact | TwinClass::ShapeSimilar) })
        );
        let windows = page_family_windows(&[twins.len(), 0], 0, Some(1));
        let page: Vec<_> = twins
            .into_iter()
            .skip(windows[0].0)
            .take(windows[0].1)
            .collect();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].name, "keep_me_visible");
        assert!(page_has_more(2, 0, page.len()));
        // Offset skips the first structural group; the route family then
        // shares whatever limit remains.
        assert_eq!(
            page_family_windows(&[2, 4], 1, Some(3)),
            vec![(1, 1), (0, 2)]
        );
        assert_eq!(
            page_family_windows(&[5, 4], 0, Some(3)),
            vec![(0, 3), (0, 0)]
        );
    }
}

#[cfg(test)]
mod source_evidence_tests {
    use super::*;

    fn file(path: &str) -> FileAnalysis {
        let mut file = FileAnalysis::new(path.to_string());
        file.exports.push(crate::types::ExportSymbol {
            name: "shared".into(),
            kind: "function".into(),
            export_type: "named".into(),
            line: Some(1),
            params: Vec::new(),
            symbol_id: Default::default(),
        });
        file.signature_uses.push(crate::types::SignatureUse {
            function: "shared".into(),
            usage: crate::types::SignatureUseKind::Parameter,
            type_name: "Input".into(),
            line: Some(1),
        });
        file
    }

    #[test]
    fn missing_owned_source_retains_sensor_with_verification_action() {
        let root = tempfile::tempdir().expect("root");
        let files = [file("a.rs"), file("b.rs")];
        let twins = detect_exact_twins_with_sources(
            &files,
            false,
            None,
            &mut TwinSources::for_root(root.path()),
        );
        assert_eq!(twins.len(), 1);
        assert_eq!(twins[0].class, TwinClass::Exact);
        assert!(twins[0].exclude_from_score);
        assert!(twin_action(&twins[0]).contains("verify owned source"));
        assert!(!twins[0].single_module_target);
    }

    #[test]
    fn multiroot_relative_paths_never_borrow_first_existing_source() {
        let first = tempfile::tempdir().expect("first");
        let second = tempfile::tempdir().expect("second");
        for root in [first.path(), second.path()] {
            std::fs::write(root.join("a.rs"), "// foreign comment").expect("write");
            std::fs::write(root.join("b.rs"), "// foreign comment").expect("write");
        }
        let mut snapshot = crate::snapshot::Snapshot::new(vec![
            first.path().display().to_string(),
            second.path().display().to_string(),
        ]);
        snapshot.files = vec![file("a.rs"), file("b.rs")];
        let twins = detect_exact_twins_with_sources(
            &snapshot.files,
            false,
            None,
            &mut TwinSources::for_request(&snapshot, first.path()),
        );
        assert_eq!(
            twins.len(),
            1,
            "ambiguous sources cannot suppress sensor rows"
        );
        assert!(twins[0].exclude_from_score);
        let mut missing_owner = snapshot.clone();
        missing_owner.metadata.roots[1] = second.path().join("missing-root").display().to_string();
        let ambiguous = detect_exact_twins_with_sources(
            &missing_owner.files,
            false,
            None,
            &mut TwinSources::for_snapshot(&missing_owner),
        );
        assert_eq!(
            ambiguous.len(),
            1,
            "missing owner cannot authorize the other root's comments"
        );
        assert!(ambiguous[0].exclude_from_score);
        // Absolute, owned paths in distinct roots remain valid and retain identity.
        for (idx, root) in [first.path(), second.path()].iter().enumerate() {
            let path = root.join("owned.rs");
            std::fs::write(&path, "pub fn shared() {}\n").expect("write");
            snapshot.files[idx].path = path.display().to_string();
        }
        let twins = detect_exact_twins_with_sources(
            &snapshot.files,
            false,
            None,
            &mut TwinSources::for_snapshot(&snapshot),
        );
        assert_eq!(twins.len(), 1);
        assert!(!twins[0].exclude_from_score);
        assert!(!twins[0].single_module_target);
        assert!(
            twins[0]
                .locations
                .iter()
                .all(|l| std::path::Path::new(&l.file_path).is_absolute())
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_parent_escape_are_unknown_even_when_target_has_comments() {
        let root = tempfile::tempdir().expect("root");
        let foreign = tempfile::tempdir().expect("foreign");
        let target = foreign.path().join("foreign.rs");
        std::fs::write(&target, "// foreign evidence").expect("write");
        std::os::unix::fs::symlink(&target, root.path().join("a.rs")).expect("symlink");
        let mut sources = TwinSources::for_root(root.path());
        assert!(sources.read("a.rs").is_none());
        assert!(sources.read(target.to_str().expect("utf8")).is_none());
        assert!(sources.read("../foreign.rs").is_none());
        let twins = detect_exact_twins_with_sources(
            &[file("a.rs"), file("b.rs")],
            false,
            None,
            &mut sources,
        );
        assert_eq!(twins.len(), 1);
        assert!(twins[0].exclude_from_score);
    }
}
