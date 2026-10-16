//! Refactor plan generation for architectural reorganization.
//!
//! Analyzes module coupling and suggests safe file reorganization strategies.
//! Uses existing building blocks (impact, focus, cycles) with layer detection
//! heuristics and topological ordering.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::focuser::{FocusConfig, HolographicFocus};
use crate::impact::{ImpactOptions, analyze_impact};
use crate::snapshot::Snapshot;

pub mod output;
pub use output::{output_as_json, output_as_markdown, output_as_script};

// ============================================================================
// Data Structures
// ============================================================================

/// Architectural layer classification for files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    /// UI layer: components, views, pages
    UI,
    /// Application layer: hooks, services, stores
    App,
    /// Kernel layer: core business logic, domain models
    Kernel,
    /// Infrastructure layer: utils, helpers, adapters, API clients
    Infra,
    /// Test layer: test files and fixtures
    Test,
    /// Unknown layer: could not classify
    Unknown,
}

impl Layer {
    /// Get the canonical directory name for this layer.
    pub fn canonical_dir(&self) -> &'static str {
        match self {
            Layer::UI => "ui",
            Layer::App => "app",
            Layer::Kernel => "kernel",
            Layer::Infra => "infra",
            Layer::Test => "tests",
            Layer::Unknown => "misc",
        }
    }

    /// Get display name for reports.
    pub fn display_name(&self) -> &'static str {
        match self {
            Layer::UI => "UI",
            Layer::App => "App",
            Layer::Kernel => "Kernel",
            Layer::Infra => "Infra",
            Layer::Test => "Test",
            Layer::Unknown => "Unknown",
        }
    }

    /// Parse from string (for target-layout option).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "ui" | "components" | "views" | "pages" => Some(Layer::UI),
            "app" | "application" | "services" | "hooks" => Some(Layer::App),
            "kernel" | "core" | "domain" | "models" => Some(Layer::Kernel),
            "infra" | "infrastructure" | "utils" | "lib" => Some(Layer::Infra),
            "test" | "tests" | "spec" | "specs" => Some(Layer::Test),
            _ => None,
        }
    }
}

/// Parse `--target-layout` spec like `kernel=src/kernel,ui=src/ui`.
pub fn parse_target_layout_spec(spec: &str) -> Result<HashMap<Layer, String>, String> {
    let mut map: HashMap<Layer, String> = HashMap::new();

    for entry in spec.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let (raw_key, raw_value) = entry.split_once('=').ok_or_else(|| {
            format!(
                "Invalid --target-layout entry '{}': expected KEY=PATH",
                entry
            )
        })?;

        let key = raw_key.trim();
        let value = raw_value.trim();
        if value.is_empty() {
            return Err(format!(
                "Invalid --target-layout entry '{}': PATH cannot be empty",
                entry
            ));
        }

        let layer = Layer::parse(key).ok_or_else(|| {
            format!(
                "Unknown layer '{}' in --target-layout (use ui|app|kernel|infra|tests)",
                key
            )
        })?;

        if !portable_relative_path(value) {
            return Err(format!(
                "Invalid --target-layout path '{value}': use a portable relative path without traversal"
            ));
        }
        if map.insert(layer, value.to_string()).is_some() {
            return Err(format!("Duplicate layer '{key}' in --target-layout"));
        }
    }

    if map.is_empty() {
        return Err("Invalid --target-layout: empty spec".to_string());
    }

    Ok(map)
}

/// Risk level for a file move operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    /// Low risk: few consumers, small file, not in cycle
    Low,
    /// Medium risk: moderate consumers or file size
    Medium,
    /// High risk: many consumers, large file, or part of a cycle
    High,
}

impl RiskLevel {
    /// Get display label.
    pub fn label(&self) -> &'static str {
        match self {
            RiskLevel::Low => "LOW",
            RiskLevel::Medium => "MEDIUM",
            RiskLevel::High => "HIGH",
        }
    }

    /// Get color for terminal output.
    pub fn color(&self) -> &'static str {
        match self {
            RiskLevel::Low => "green",
            RiskLevel::Medium => "yellow",
            RiskLevel::High => "red",
        }
    }
}

/// A proposed file move operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Move {
    /// Source file path (relative to project root)
    pub source: String,
    /// Suggested target path
    pub target: String,
    /// Current detected layer
    pub current_layer: Layer,
    /// Suggested target layer
    pub target_layer: Layer,
    /// Risk level for this move
    pub risk: RiskLevel,
    /// Number of direct consumers (files that import this)
    pub direct_consumers: usize,
    /// Number of transitive consumers (indirect dependents)
    pub transitive_consumers: usize,
    /// Lines of code in the file
    pub loc: usize,
    /// Reason for the suggested move
    pub reason: String,
    /// Verification command to run after move
    pub verify_cmd: String,
    /// List of affected files (consumers that need import updates)
    pub affected_files: Vec<String>,
}

/// A re-export shim to maintain backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shim {
    /// Original file path (where the shim will be created)
    pub old_path: String,
    /// New file location
    pub new_path: String,
    /// Symbols to re-export
    pub symbols: Vec<String>,
    /// Number of files that import from old path
    pub importer_count: usize,
    /// Generated re-export code
    pub code: String,
}

/// Statistics for the refactor plan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanStats {
    /// Total files analyzed
    pub total_files: usize,
    /// Files that need to be moved
    pub files_to_move: usize,
    /// Shims needed for backward compatibility
    pub shims_needed: usize,
    /// Layer distribution before refactoring
    pub layer_before: HashMap<String, usize>,
    /// Layer distribution after refactoring
    pub layer_after: HashMap<String, usize>,
    /// Count by risk level
    pub by_risk: HashMap<String, usize>,
}

/// A phase of execution (grouped by risk level).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactorPhase {
    /// Phase name (e.g., "Phase 1: LOW Risk")
    pub name: String,
    /// Risk level for this phase
    pub risk: RiskLevel,
    /// Moves in this phase
    pub moves: Vec<Move>,
    /// Git commands for this phase
    pub git_script: String,
}

/// A kept file whose relocation is not justified or not supported.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolvedCandidate {
    pub source: String,
    pub layer: Layer,
    pub proposed_target: Option<String>,
    pub reason: String,
}

/// Complete refactor plan, including candidates that are not authorized moves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactorPlan {
    /// Target directory that was analyzed
    pub target: String,
    /// All proposed moves, ordered by risk (LOW first)
    pub moves: Vec<Move>,
    /// Shims needed for backward compatibility
    pub shims: Vec<Shim>,
    /// Files involved in cyclic dependencies
    pub cyclic_groups: Vec<Vec<String>>,
    /// Execution phases
    pub phases: Vec<RefactorPhase>,
    /// Statistics
    pub stats: PlanStats,
    /// Physical repository root required by the guarded script.
    #[serde(default)]
    pub repository_root: String,
    /// Exact source, manifest and compiler content covered by the contract.
    #[serde(default)]
    pub source_hashes: HashMap<String, String>,
    /// Kept files and the missing evidence or unsupported contract.
    #[serde(default)]
    pub unresolved: Vec<UnresolvedCandidate>,
}

// ============================================================================
// Layer Detection
// ============================================================================

/// Detect the architectural layer of a file based on path heuristics.
pub fn detect_layer(file_path: &str) -> Layer {
    let path_lower = format!("/{}", file_path.to_lowercase());

    // Test files
    if path_lower.contains("/test/")
        || path_lower.contains("/tests/")
        || path_lower.contains("/__tests__/")
        || path_lower.contains(".test.")
        || path_lower.contains(".spec.")
        || path_lower.contains("_test.")
        || path_lower.ends_with("_test.rs")
        || path_lower.ends_with("_test.go")
    {
        return Layer::Test;
    }

    // UI layer patterns
    if path_lower.contains("/components/")
        || path_lower.contains("/views/")
        || path_lower.contains("/pages/")
        || path_lower.contains("/ui/")
        || path_lower.contains("/widgets/")
        || path_lower.contains("/screens/")
        || path_lower.ends_with(".tsx")
        || path_lower.ends_with(".vue")
        || path_lower.ends_with(".svelte")
        || path_lower.ends_with(".astro")
    {
        // But not if it's a hook or store
        if !path_lower.contains("use") && !path_lower.contains("store") {
            return Layer::UI;
        }
    }

    // App layer patterns (hooks, services, stores)
    if path_lower.contains("/hooks/")
        || path_lower.contains("/services/")
        || path_lower.contains("/stores/")
        || path_lower.contains("/state/")
        || path_lower.contains("/context/")
        || path_lower.contains("/providers/")
    {
        return Layer::App;
    }

    // Check for React hook files (useXxx.ts)
    if let Some(filename) = Path::new(file_path).file_stem().and_then(|s| s.to_str())
        && filename.starts_with("use")
        && filename.len() > 3
        && filename.chars().nth(3).unwrap_or('_').is_uppercase()
    {
        return Layer::App;
    }

    // Kernel layer patterns (core, domain, models)
    if path_lower.contains("/core/")
        || path_lower.contains("/domain/")
        || path_lower.contains("/models/")
        || path_lower.contains("/entities/")
        || path_lower.contains("/kernel/")
        || path_lower.contains("/business/")
    {
        return Layer::Kernel;
    }

    // Infra layer patterns (utils, helpers, lib, adapters)
    if path_lower.contains("/utils/")
        || path_lower.contains("/helpers/")
        || path_lower.contains("/lib/")
        || path_lower.contains("/infra/")
        || path_lower.contains("/infrastructure/")
        || path_lower.contains("/adapters/")
        || path_lower.contains("/api/")
        || path_lower.contains("/clients/")
    {
        return Layer::Infra;
    }

    Layer::Unknown
}

// ============================================================================
// Risk Calculation
// ============================================================================

/// Calculate risk level based on impact metrics.
pub fn calculate_risk(
    direct_consumers: usize,
    transitive_consumers: usize,
    loc: usize,
    in_cycle: bool,
) -> RiskLevel {
    // High risk thresholds
    if in_cycle {
        return RiskLevel::High;
    }
    if direct_consumers >= 10 || transitive_consumers >= 50 {
        return RiskLevel::High;
    }
    if loc >= 500 {
        return RiskLevel::High;
    }

    // Medium risk thresholds
    if direct_consumers >= 5 || transitive_consumers >= 20 {
        return RiskLevel::Medium;
    }
    if loc >= 200 {
        return RiskLevel::Medium;
    }

    RiskLevel::Low
}

// ============================================================================
// Cycle Detection (simplified Tarjan for file groups)
// ============================================================================

/// Detect groups of files involved in cyclic dependencies.
pub fn detect_cyclic_groups(snapshot: &Snapshot, files: &[String]) -> Vec<Vec<String>> {
    let file_set: HashSet<&str> = files.iter().map(|s| s.as_str()).collect();

    // Build adjacency list for files within scope
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &snapshot.edges {
        if file_set.contains(edge.from.as_str()) && file_set.contains(edge.to.as_str()) {
            adjacency
                .entry(edge.from.as_str())
                .or_default()
                .push(edge.to.as_str());
        }
    }

    // Tarjan's SCC algorithm
    struct TarjanState<'a> {
        adjacency: HashMap<&'a str, Vec<&'a str>>,
        index_counter: usize,
        stack: Vec<&'a str>,
        on_stack: HashSet<&'a str>,
        index: HashMap<&'a str, usize>,
        lowlink: HashMap<&'a str, usize>,
        sccs: Vec<Vec<String>>,
    }

    fn strongconnect<'a>(v: &'a str, state: &mut TarjanState<'a>) {
        state.index.insert(v, state.index_counter);
        state.lowlink.insert(v, state.index_counter);
        state.index_counter += 1;
        state.stack.push(v);
        state.on_stack.insert(v);

        if let Some(neighbors) = state.adjacency.get(v) {
            let neighbors: Vec<&'a str> = neighbors.clone();
            for w in neighbors {
                if !state.index.contains_key(w) {
                    strongconnect(w, state);
                    let w_lowlink = *state.lowlink.get(w).unwrap();
                    let v_lowlink = state.lowlink.get_mut(v).unwrap();
                    *v_lowlink = (*v_lowlink).min(w_lowlink);
                } else if state.on_stack.contains(w) {
                    let w_index = *state.index.get(w).unwrap();
                    let v_lowlink = state.lowlink.get_mut(v).unwrap();
                    *v_lowlink = (*v_lowlink).min(w_index);
                }
            }
        }

        if state.lowlink.get(v) == state.index.get(v) {
            let mut scc: Vec<String> = Vec::new();
            loop {
                let w = state.stack.pop().unwrap();
                state.on_stack.remove(w);
                scc.push(w.to_string());
                if w == v {
                    break;
                }
            }
            // Only include SCCs with more than 1 node (actual cycles)
            if scc.len() > 1 {
                state.sccs.push(scc);
            }
        }
    }

    let mut state = TarjanState {
        adjacency,
        index_counter: 0,
        stack: Vec::new(),
        on_stack: HashSet::new(),
        index: HashMap::new(),
        lowlink: HashMap::new(),
        sccs: Vec::new(),
    };

    for file in files {
        if !state.index.contains_key(file.as_str()) {
            strongconnect(file.as_str(), &mut state);
        }
    }

    state.sccs
}

// ============================================================================
// Shim Detection
// ============================================================================

/// Detect which moves need re-export shims for backward compatibility.
pub fn detect_needed_shims(snapshot: &Snapshot, moves: &[Move]) -> Vec<Shim> {
    let mut shims = Vec::new();

    for mv in moves {
        {
            // Build list of symbols from file's exports
            let symbols: Vec<String> = snapshot
                .files
                .iter()
                .find(|f| f.path == mv.source)
                .map(|f| {
                    f.exports
                        .iter()
                        .map(|e| {
                            if e.export_type == "default" {
                                "default".to_string()
                            } else {
                                e.name.clone()
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            if !symbols.is_empty() {
                // Generate re-export code based on file type
                let Some(code) = generate_shim_code(&mv.source, &mv.target, &symbols) else {
                    continue;
                };

                shims.push(Shim {
                    old_path: mv.source.clone(),
                    new_path: mv.target.clone(),
                    symbols,
                    importer_count: mv.direct_consumers,
                    code,
                });
            }
        }
    }

    shims
}

/// Generate re-export shim code based on file extension.
fn generate_shim_code(old_path: &str, new_path: &str, symbols: &[String]) -> Option<String> {
    let ext = Path::new(old_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    // Calculate relative path from old to new
    let relative_path = calculate_relative_import(old_path, new_path);

    match ext {
        "ts" | "js" | "mjs" if !symbols.is_empty() => {
            let mut code =
                format!("// Shim for backward compatibility\nexport * from '{relative_path}';\n");
            if symbols.iter().any(|s| s == "default") {
                code.push_str(&format!("export {{ default }} from '{relative_path}';\n"));
            }
            Some(code)
        }
        // Filesystem paths are not Rust module paths or Python package names.
        _ => None,
    }
}

/// Calculate relative import path between two files.
fn calculate_relative_import(from: &str, to: &str) -> String {
    let from_parts: Vec<&str> = from.split('/').collect();
    let to_parts: Vec<&str> = to.split('/').collect();

    // Find common prefix length
    let common_len = from_parts
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();

    // Build relative path
    let ups = from_parts.len() - common_len - 1; // -1 for filename
    let downs: Vec<&str> = to_parts[common_len..].to_vec();

    let mut result = String::new();
    if ups == 0 {
        result.push_str("./");
    } else {
        for _ in 0..ups {
            result.push_str("../");
        }
    }
    result.push_str(&downs.join("/"));

    // TypeScript's emitted ESM module uses .js; Node requires the extension.
    if result.ends_with(".ts") {
        result.truncate(result.len() - 3);
        result.push_str(".js");
    }

    result
}

// ============================================================================
// Move Ordering
// ============================================================================

/// Order moves by risk (LOW first) for safe incremental execution.
pub fn order_moves(moves: &mut [Move]) {
    moves.sort_by(|a, b| {
        // Primary: risk level (Low < Medium < High)
        match a.risk.cmp(&b.risk) {
            std::cmp::Ordering::Equal => {
                // Secondary: fewer consumers first
                match a.direct_consumers.cmp(&b.direct_consumers) {
                    std::cmp::Ordering::Equal => {
                        // Tertiary: smaller files first
                        a.loc.cmp(&b.loc)
                    }
                    other => other,
                }
            }
            other => other,
        }
    });
}

// ============================================================================
// Main Plan Generation
// ============================================================================

/// Generate a refactor plan for a target directory.
///
/// Returns `None` only when the target has no represented files.
pub fn generate_refactor_plan(
    snapshot: &Snapshot,
    target_dir: &str,
    target_layout: Option<&HashMap<Layer, String>>,
) -> Option<RefactorPlan> {
    let repository_root = snapshot
        .metadata
        .roots
        .first()
        .and_then(|root| std::fs::canonicalize(root).ok());
    let relative_target;
    let target_dir = if Path::new(target_dir).is_absolute() {
        let root = repository_root.as_ref()?;
        relative_target = std::fs::canonicalize(target_dir)
            .ok()?
            .strip_prefix(root)
            .ok()?
            .to_string_lossy()
            .into_owned();
        if relative_target.is_empty() {
            "."
        } else {
            &relative_target
        }
    } else {
        target_dir
    };
    // Get holographic focus for the target directory
    let config = FocusConfig {
        include_consumers: true,
        max_depth: 3,
    };

    let focus = HolographicFocus::from_path(snapshot, target_dir, &config)?;

    if focus.core.is_empty() {
        return None;
    }

    // Collect all file paths for cycle detection
    let file_paths: Vec<String> = focus.core.iter().map(|f| f.path.clone()).collect();

    // Detect cyclic groups
    let cyclic_groups = detect_cyclic_groups(snapshot, &file_paths);
    let files_in_cycles: HashSet<&str> = cyclic_groups
        .iter()
        .flat_map(|g| g.iter())
        .map(|s| s.as_str())
        .collect();

    // Generate moves
    let impact_opts = ImpactOptions::default();
    let mut moves: Vec<Move> = Vec::new();
    let mut unresolved = Vec::new();
    let mut source_hashes = HashMap::new();
    let base_dir = &focus.target;
    let mut layer_before: HashMap<String, usize> = HashMap::new();
    let mut layer_after: HashMap<String, usize> = HashMap::new();

    for file in &focus.core {
        let current_layer = detect_layer(&file.path);
        *layer_before
            .entry(current_layer.display_name().to_string())
            .or_default() += 1;

        let target_layer = current_layer;

        *layer_after
            .entry(target_layer.display_name().to_string())
            .or_default() += 1;

        let target_path = build_target_path(&file.path, target_layer, target_layout, base_dir);
        let analysis = snapshot
            .files
            .iter()
            .find(|f| f.path == file.path)
            .expect("focused file belongs to snapshot");
        let contract = relocation_contract(
            snapshot,
            analysis,
            &target_path,
            target_layout,
            repository_root.as_deref(),
        );
        match contract {
            Err(reason) => {
                unresolved.push(UnresolvedCandidate {
                    source: file.path.clone(),
                    layer: current_layer,
                    proposed_target: target_layout
                        .and_then(|layout| layout.get(&current_layer))
                        .map(|_| target_path),
                    reason,
                });
            }
            Ok(hashes) => {
                let impact = analyze_impact(snapshot, &file.path, &impact_opts);
                let in_cycle = files_in_cycles.contains(file.path.as_str());
                let risk = calculate_risk(
                    impact.direct_consumers.len(),
                    impact.transitive_consumers.len(),
                    file.loc,
                    in_cycle,
                );

                let reason = "Explicit layout; same-package literal ESM module; topology and old-path exports preserved".to_string();
                let verify_cmd = format!("loct impact {}", target_path);

                let affected_files: Vec<String> = impact
                    .direct_consumers
                    .iter()
                    .map(|e| e.file.clone())
                    .collect();

                source_hashes.extend(hashes);
                moves.push(Move {
                    source: file.path.clone(),
                    target: target_path,
                    current_layer,
                    target_layer,
                    risk,
                    direct_consumers: impact.direct_consumers.len(),
                    transitive_consumers: impact.transitive_consumers.len(),
                    loc: file.loc,
                    reason,
                    verify_cmd,
                    affected_files,
                });
            }
        }
    }

    // Reject the entire conflicting set rather than picking a random winner.
    let conflicts = move_conflicts(&moves);
    if !conflicts.is_empty() {
        for mv in moves.drain(..) {
            unresolved.push(UnresolvedCandidate {
                source: mv.source,
                layer: mv.current_layer,
                proposed_target: Some(mv.target),
                reason: conflicts.join("; "),
            });
        }
        source_hashes.clear();
    }

    // Order moves by risk
    order_moves(&mut moves);

    // Detect needed shims
    let shims = detect_needed_shims(snapshot, &moves);

    // Build phases
    let phases = build_phases(&moves);

    // Build stats
    let mut by_risk: HashMap<String, usize> = HashMap::new();
    for mv in &moves {
        *by_risk.entry(mv.risk.label().to_string()).or_default() += 1;
    }

    let stats = PlanStats {
        total_files: focus.core.len(),
        files_to_move: moves.len(),
        shims_needed: shims.len(),
        layer_before,
        layer_after,
        by_risk,
    };

    Some(RefactorPlan {
        target: target_dir.to_string(),
        moves,
        shims,
        cyclic_groups,
        phases,
        stats,
        repository_root: repository_root
            .map(|root| root.to_string_lossy().into_owned())
            .unwrap_or_default(),
        source_hashes,
        unresolved,
    })
}

fn portable_relative_path(path: &str) -> bool {
    !path.is_empty()
        && path
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-. /@+".contains(&c))
        && !path.contains("//")
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

fn protected_path(path: &str) -> bool {
    path.split('/').any(|part| {
        part.starts_with('.')
            || matches!(
                part.to_ascii_lowercase().as_str(),
                "node_modules"
                    | "vendor"
                    | "target"
                    | "dist"
                    | "build"
                    | "assets"
                    | "fixtures"
                    | "fixture"
                    | "tests"
                    | "test"
                    | "__tests__"
                    | "public"
                    | "static"
                    | "resources"
                    | "locales"
                    | "templates"
                    | "migrations"
            )
    }) || matches!(
        Path::new(path).file_stem().and_then(|s| s.to_str()),
        Some("index" | "main" | "__init__" | "__main__" | "lib" | "mod")
    )
}

fn package_boundary(root: &Path, path: &str) -> Option<PathBuf> {
    root.join(path)
        .parent()?
        .ancestors()
        .take_while(|p| p.starts_with(root))
        .find(|p| {
            [
                "package.json",
                "Cargo.toml",
                "pyproject.toml",
                "__init__.py",
            ]
            .iter()
            .any(|name| p.join(name).exists())
        })
        .map(Path::to_path_buf)
}

fn inspect_move_path(root: &Path, path: &str, destination: bool) -> Result<(), String> {
    let mut current = root.to_path_buf();
    let parts: Vec<_> = path.split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        if current.is_dir() {
            for entry in std::fs::read_dir(&current).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.eq_ignore_ascii_case(part)
                    && (name != *part || destination && index + 1 == parts.len())
                {
                    return Err(format!(
                        "Existing or case-colliding path: {}",
                        entry.path().display()
                    ));
                }
            }
        }
        current.push(part);
        if let Ok(metadata) = std::fs::symlink_metadata(&current)
            && (metadata.file_type().is_symlink() || index + 1 < parts.len() && !metadata.is_dir())
        {
            return Err(format!(
                "Symlink or non-directory ancestor: {}",
                current.display()
            ));
        }
    }
    Ok(())
}

// Deliberately bounded: no imports, functions, runtime effects or location-sensitive
// expressions. Reuse the source parser rather than infer safety from graph silence.
fn literal_esm_module(source: &str, path: &str) -> bool {
    use oxc_ast::ast::{Declaration, Expression, Statement, VariableDeclarationKind};
    let allocator = oxc_allocator::Allocator::default();
    let Ok(source_type) = oxc_span::SourceType::from_path(path) else {
        return false;
    };
    let parsed = oxc_parser::Parser::new(&allocator, source, source_type).parse();
    parsed.errors.is_empty()
        && parsed.program.hashbang.is_none()
        && parsed.program.directives.is_empty()
        && !parsed.program.body.is_empty()
        && parsed.program.body.iter().all(|statement| {
            let Statement::ExportNamedDeclaration(export) = statement else {
                return false;
            };
            let Some(Declaration::VariableDeclaration(declaration)) = &export.declaration else {
                return false;
            };
            declaration.kind == VariableDeclarationKind::Const
                && declaration.declarations.iter().all(|declarator| {
                    declarator.id.get_binding_identifier().is_some()
                        && matches!(
                            declarator.init,
                            Some(
                                Expression::NumericLiteral(_)
                                    | Expression::StringLiteral(_)
                                    | Expression::BooleanLiteral(_)
                                    | Expression::NullLiteral(_)
                                    | Expression::BigIntLiteral(_)
                            )
                        )
                })
        })
}

fn relocation_contract(
    snapshot: &Snapshot,
    file: &crate::types::FileAnalysis,
    target: &str,
    layout: Option<&HashMap<Layer, String>>,
    root: Option<&Path>,
) -> Result<HashMap<String, String>, String> {
    let layer = detect_layer(&file.path);
    if protected_path(&file.path)
        || file.is_test
        || file.is_generated
        || file.resource_kind.is_some()
        || !file.entry_points.is_empty()
        || snapshot
            .metadata
            .entrypoints
            .iter()
            .any(|e| e.path == file.path)
    {
        return Err(
            "Kept: package/runtime identity, resource, fixture or generated surface".into(),
        );
    }
    if layer == Layer::Unknown {
        return Err("Unknown layer: path and neighbor heuristics do not justify relocation".into());
    }
    if layout.and_then(|l| l.get(&layer)).is_none() {
        return Err("Candidate only: no explicit target layout for this layer".into());
    }
    if file.path == target {
        return Err("Kept: already within the requested destination".into());
    }
    if !portable_relative_path(&file.path)
        || !portable_relative_path(target)
        || protected_path(target)
    {
        return Err(
            "Unsupported path: require portable relative paths and unprotected destination".into(),
        );
    }
    if !matches!(
        Path::new(&file.path).extension().and_then(|s| s.to_str()),
        Some("ts" | "js" | "mjs")
    ) {
        return Err("Unsupported relocation/shim contract: Rust/Python modules and other languages stay in place".into());
    }
    let root = root.ok_or("Repository root unavailable; cannot verify filesystem identity")?;
    let boundary = package_boundary(root, &file.path);
    if boundary.is_none() || boundary != package_boundary(root, target) {
        return Err("Package boundary is missing or would change".into());
    }
    let package = boundary.expect("verified package boundary");
    let manifest_path = package.join("package.json");
    let manifest_source = std::fs::read_to_string(&manifest_path)
        .map_err(|_| "Unsupported ESM contract: package.json is required")?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_source)
        .map_err(|_| "Unsupported ESM contract: package.json must be valid JSON")?;
    if manifest["type"] != "module" {
        return Err("Unsupported ESM contract: require package.json type=module".into());
    }
    let mut inputs = vec![(manifest_path, manifest_source)];
    if file.path.ends_with(".ts") {
        let config_path = package.join("tsconfig.json");
        let config_source = std::fs::read_to_string(&config_path)
            .map_err(|_| "Unsupported TypeScript contract: explicit tsconfig.json ESM emission scope required")?;
        let config: serde_json::Value = serde_json::from_str(&config_source).map_err(
            |_| "Unsupported TypeScript contract: JSONC/extended configurations are not translated",
        )?;
        verify_typescript_emission(&config, root, &package, &file.path, target)?;
        inputs.push((config_path, config_source));
    }
    inspect_move_path(root, &file.path, false)?;
    inspect_move_path(root, target, true)?;
    let source = std::fs::read_to_string(root.join(&file.path))
        .map_err(|e| format!("Source unavailable: {e}"))?;
    if !file.imports.is_empty()
        || !file.reexports.is_empty()
        || !file.dynamic_imports.is_empty()
        || file.exports.is_empty()
        || file.exports.iter().any(|e| e.export_type != "named")
        || snapshot.edges.iter().any(|e| e.from == file.path)
        || !literal_esm_module(&source, &file.path)
    {
        return Err("Unsupported relocation contract: only literal-export ESM leaves; dependencies/runtime expressions require import and asset rewriting".into());
    }
    inputs.push((root.join(&file.path), source));
    Ok(inputs
        .into_iter()
        .map(|(path, content)| {
            (
                path.strip_prefix(root)
                    .expect("contract input within root")
                    .to_string_lossy()
                    .into_owned(),
                Sha256::digest(content.as_bytes())
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            )
        })
        .collect())
}

fn verify_typescript_emission(
    config: &serde_json::Value,
    root: &Path,
    package: &Path,
    source: &str,
    target: &str,
) -> Result<(), String> {
    let unsupported = || {
        "Unsupported TypeScript contract: require a simple explicit ESM emitting tsconfig with rootDir, outDir and include covering both paths; native/noEmit, extended/project-reference and nested scopes are withheld".to_string()
    };
    let options = &config["compilerOptions"];
    let module = options["module"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    let root_dir = options["rootDir"].as_str().ok_or_else(unsupported)?;
    let out_dir = options["outDir"].as_str().ok_or_else(unsupported)?;
    if config.get("extends").is_some()
        || config.get("references").is_some()
        || options.get("moduleSuffixes").is_some()
        || options.get("rootDirs").is_some()
        || options.get("outFile").is_some()
        || options["noEmit"] == true
        || options["emitDeclarationOnly"] == true
        || !matches!(
            module.as_str(),
            "nodenext" | "node16" | "esnext" | "es2022" | "es2020" | "es2015"
        )
        || root_dir != "." && !portable_relative_path(root_dir)
        || !portable_relative_path(out_dir)
    {
        return Err(unsupported());
    }
    let includes = config["include"].as_array().ok_or_else(unsupported)?;
    let matches = |patterns: &[serde_json::Value], relative: &Path| -> Result<bool, String> {
        for pattern in patterns {
            let raw = pattern.as_str().ok_or_else(unsupported)?;
            let pattern = raw.strip_prefix("./").unwrap_or(raw);
            let pattern = if !pattern.contains(['*', '?', '[', '{'])
                && Path::new(pattern).extension().is_none()
            {
                format!("{pattern}/**")
            } else {
                pattern.to_string()
            };
            let glob = globset::GlobBuilder::new(&pattern)
                .literal_separator(true)
                .build()
                .map_err(|_| unsupported())?;
            if glob.compile_matcher().is_match(relative) {
                return Ok(true);
            }
        }
        Ok(false)
    };
    for path in [source, target] {
        let full = root.join(path);
        let relative = full.strip_prefix(package).map_err(|_| unsupported())?;
        if !full.starts_with(package.join(root_dir))
            || full.starts_with(package.join(out_dir))
            || !matches(includes, relative)?
            || config["exclude"]
                .as_array()
                .is_some_and(|patterns| matches(patterns, relative).unwrap_or(true))
            || full
                .parent()
                .expect("file parent")
                .ancestors()
                .take_while(|p| *p != package)
                .any(|p| p.join("tsconfig.json").exists())
        {
            return Err(unsupported());
        }
    }
    Ok(())
}

fn move_conflicts(moves: &[Move]) -> Vec<String> {
    let endpoints: Vec<_> = moves
        .iter()
        .flat_map(|mv| [&mv.source, &mv.target])
        .map(|p| p.to_ascii_lowercase())
        .collect();
    let mut issues = Vec::new();
    for (i, left) in endpoints.iter().enumerate() {
        for right in &endpoints[i + 1..] {
            if left == right
                || left.starts_with(&format!("{right}/"))
                || right.starts_with(&format!("{left}/"))
            {
                issues.push(format!("Conflicting move paths (duplicate, case, cycle or ancestor overlap): {left} / {right}"));
            }
        }
    }
    issues.sort();
    issues.dedup();
    issues
}

/// One execution contract for all renderers, including multi-target bundles.
pub fn execution_blockers(plans: &[RefactorPlan]) -> Vec<String> {
    let moves: Vec<_> = plans.iter().flat_map(|p| p.moves.clone()).collect();
    let mut issues = move_conflicts(&moves);
    let root = plans
        .first()
        .map(|p| p.repository_root.as_str())
        .unwrap_or("");
    for plan in plans {
        if !plan.moves.is_empty() && (root.is_empty() || plan.repository_root != root) {
            issues.push("Missing or inconsistent repository roots".into());
        }
        if plan
            .source_hashes
            .keys()
            .any(|path| !portable_relative_path(path))
        {
            issues.push("Unsupported contract input path".into());
        }
        let phased: Vec<_> = plan.phases.iter().flat_map(|p| &p.moves).collect();
        if serde_json::to_value(&phased).ok() != serde_json::to_value(&plan.moves).ok() {
            issues.push("Phases do not match the canonical moves".into());
        }
        for mv in &plan.moves {
            if !portable_relative_path(&mv.source)
                || !portable_relative_path(&mv.target)
                || protected_path(&mv.source)
                || protected_path(&mv.target)
            {
                issues.push(format!("Unsupported move path: {}", mv.source));
            }
            if mv.current_layer == Layer::Unknown || mv.current_layer != mv.target_layer {
                issues.push(format!("Unjustified layer translation: {}", mv.source));
            }
            if Path::new(&mv.source).extension() != Path::new(&mv.target).extension() {
                issues.push(format!("Unsupported extension translation: {}", mv.source));
            }
            if !plan
                .source_hashes
                .get(&mv.source)
                .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                issues.push(format!("Missing source contract hash: {}", mv.source));
            }
            let shim = plan
                .shims
                .iter()
                .find(|s| s.old_path == mv.source && s.new_path == mv.target);
            if !shim.is_some_and(|s| {
                generate_shim_code(&s.old_path, &s.new_path, &s.symbols).as_ref() == Some(&s.code)
            }) {
                issues.push(format!(
                    "Unsupported or missing compatibility shim: {}",
                    mv.source
                ));
            }
        }
        if plan.shims.len() != plan.moves.len() {
            issues.push("Compatibility shims must match moves one-to-one".into());
        }
    }
    issues.sort();
    issues.dedup();
    issues
}

/// Build target path for a file move.
fn build_target_path(
    source: &str,
    layer: Layer,
    custom_layout: Option<&HashMap<Layer, String>>,
    base_dir: &str,
) -> String {
    let Some(layer_dir) = custom_layout.and_then(|m| m.get(&layer)) else {
        return source.to_string();
    };
    let base = if base_dir == "." {
        Path::new("")
    } else {
        Path::new(base_dir)
    };
    if Path::new(source).starts_with(base.join(layer_dir)) {
        return source.to_string();
    }
    let relative = Path::new(source)
        .strip_prefix(base)
        .unwrap_or(Path::new(source));
    base.join(layer_dir)
        .join(relative)
        .to_string_lossy()
        .into_owned()
}

/// Build execution phases from ordered moves.
fn build_phases(moves: &[Move]) -> Vec<RefactorPhase> {
    let mut phases: Vec<RefactorPhase> = Vec::new();
    let mut current_risk: Option<RiskLevel> = None;
    let mut current_moves: Vec<Move> = Vec::new();

    for mv in moves {
        if Some(mv.risk) != current_risk {
            if !current_moves.is_empty()
                && let Some(risk) = current_risk
            {
                phases.push(RefactorPhase {
                    name: format!("Phase {}: {} Risk", phases.len() + 1, risk.label()),
                    risk,
                    git_script: build_git_script(&current_moves),
                    moves: std::mem::take(&mut current_moves),
                });
            }
            current_risk = Some(mv.risk);
        }
        current_moves.push(mv.clone());
    }

    // Add final phase
    if !current_moves.is_empty()
        && let Some(risk) = current_risk
    {
        phases.push(RefactorPhase {
            name: format!("Phase {}: {} Risk", phases.len() + 1, risk.label()),
            risk,
            git_script: build_git_script(&current_moves),
            moves: current_moves,
        });
    }

    phases
}

/// Build git script for a set of moves.
fn build_git_script(moves: &[Move]) -> String {
    format!(
        "# {} move(s): use the full guarded --script output, --dry first.\n# Standalone phase commands are withheld: preflight and compatibility shims are required.",
        moves.len()
    )
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_and_python_filesystem_paths_are_not_module_shims() {
        for (old, new) in [
            ("src/utils/a.rs", "src/infra/utils/a.rs"),
            ("one/utils.py", "infra/one/utils.py"),
        ] {
            assert!(generate_shim_code(old, new, &["value".into()]).is_none());
        }
    }

    #[test]
    fn test_detect_layer_ui() {
        assert_eq!(detect_layer("src/components/Button.tsx"), Layer::UI);
        assert_eq!(detect_layer("src/views/Home.vue"), Layer::UI);
        assert_eq!(detect_layer("src/pages/index.tsx"), Layer::UI);
    }

    #[test]
    fn test_detect_layer_app() {
        assert_eq!(detect_layer("src/hooks/useAuth.ts"), Layer::App);
        assert_eq!(detect_layer("src/services/api.ts"), Layer::App);
        assert_eq!(detect_layer("src/stores/userStore.ts"), Layer::App);
    }

    #[test]
    fn test_detect_layer_kernel() {
        assert_eq!(detect_layer("src/core/engine.ts"), Layer::Kernel);
        assert_eq!(detect_layer("src/domain/Patient.ts"), Layer::Kernel);
        assert_eq!(detect_layer("src/models/User.ts"), Layer::Kernel);
    }

    #[test]
    fn test_detect_layer_infra() {
        assert_eq!(detect_layer("src/utils/format.ts"), Layer::Infra);
        assert_eq!(detect_layer("src/lib/helpers.ts"), Layer::Infra);
        assert_eq!(detect_layer("src/api/client.ts"), Layer::Infra);
    }

    #[test]
    fn test_detect_layer_test() {
        assert_eq!(detect_layer("src/__tests__/Button.test.tsx"), Layer::Test);
        assert_eq!(detect_layer("tests/integration.spec.ts"), Layer::Test);
        assert_eq!(detect_layer("src/utils_test.go"), Layer::Test);
    }

    #[test]
    fn test_calculate_risk_low() {
        assert_eq!(calculate_risk(2, 5, 100, false), RiskLevel::Low);
    }

    #[test]
    fn test_calculate_risk_medium() {
        assert_eq!(calculate_risk(6, 10, 100, false), RiskLevel::Medium);
        assert_eq!(calculate_risk(2, 25, 100, false), RiskLevel::Medium);
        assert_eq!(calculate_risk(2, 5, 300, false), RiskLevel::Medium);
    }

    #[test]
    fn test_calculate_risk_high() {
        assert_eq!(calculate_risk(12, 10, 100, false), RiskLevel::High);
        assert_eq!(calculate_risk(2, 60, 100, false), RiskLevel::High);
        assert_eq!(calculate_risk(2, 5, 600, false), RiskLevel::High);
        assert_eq!(calculate_risk(2, 5, 100, true), RiskLevel::High); // in cycle
    }

    #[test]
    fn test_calculate_relative_import_same_dir() {
        let result = calculate_relative_import("src/utils/a.ts", "src/utils/b.ts");
        assert_eq!(result, "./b.js");
    }

    #[test]
    fn test_calculate_relative_import_parent_dir() {
        let result = calculate_relative_import("src/components/Button.tsx", "src/utils/format.ts");
        assert_eq!(result, "../utils/format.js");
    }

    #[test]
    fn test_order_moves() {
        let mut moves = vec![
            Move {
                source: "a.ts".to_string(),
                target: "".to_string(),
                current_layer: Layer::Unknown,
                target_layer: Layer::Infra,
                risk: RiskLevel::High,
                direct_consumers: 5,
                transitive_consumers: 10,
                loc: 100,
                reason: "".to_string(),
                verify_cmd: "".to_string(),
                affected_files: vec![],
            },
            Move {
                source: "b.ts".to_string(),
                target: "".to_string(),
                current_layer: Layer::Unknown,
                target_layer: Layer::Infra,
                risk: RiskLevel::Low,
                direct_consumers: 1,
                transitive_consumers: 2,
                loc: 50,
                reason: "".to_string(),
                verify_cmd: "".to_string(),
                affected_files: vec![],
            },
        ];

        order_moves(&mut moves);

        assert_eq!(moves[0].source, "b.ts"); // Low risk first
        assert_eq!(moves[1].source, "a.ts"); // High risk last
    }
}
