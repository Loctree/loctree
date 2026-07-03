//! Refactor plan generation for architectural reorganization.
//!
//! Analyzes module coupling and suggests safe file reorganization strategies.
//! Uses existing building blocks (impact, focus, cycles) with layer detection
//! heuristics and topological ordering.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

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

        map.insert(layer, value.to_string());
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

/// Complete refactor plan for a directory.
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
}

// ============================================================================
// Layer Detection
// ============================================================================

/// Detect the architectural layer of a file based on path heuristics.
pub fn detect_layer(file_path: &str) -> Layer {
    let path_lower = file_path.to_lowercase();

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
        if mv.direct_consumers > 3 {
            // Build list of symbols from file's exports
            let symbols: Vec<String> = snapshot
                .files
                .iter()
                .find(|f| f.path == mv.source)
                .map(|f| f.exports.iter().map(|e| e.name.clone()).collect())
                .unwrap_or_default();

            if !symbols.is_empty() {
                // Generate re-export code based on file type
                let code = generate_shim_code(&mv.source, &mv.target, &symbols);

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
fn generate_shim_code(old_path: &str, new_path: &str, symbols: &[String]) -> String {
    let ext = Path::new(old_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    // Calculate relative path from old to new
    let relative_path = calculate_relative_import(old_path, new_path);

    match ext {
        "ts" | "tsx" | "js" | "jsx" | "mjs" => {
            if symbols.len() <= 5 {
                format!(
                    "// Shim for backward compatibility\nexport {{ {} }} from '{}';",
                    symbols.join(", "),
                    relative_path
                )
            } else {
                format!(
                    "// Shim for backward compatibility\nexport * from '{}';",
                    relative_path
                )
            }
        }
        "rs" => {
            format!(
                "// Shim for backward compatibility\npub use {}::*;",
                relative_path.replace('/', "::")
            )
        }
        "py" => {
            format!(
                "# Shim for backward compatibility\nfrom {} import *",
                relative_path.replace('/', ".")
            )
        }
        _ => format!("// TODO: Create re-export shim pointing to {}", new_path),
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

    // Remove extension for import
    if let Some(pos) = result.rfind('.') {
        result.truncate(pos);
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
/// Returns `None` if no files need reorganization.
pub fn generate_refactor_plan(
    snapshot: &Snapshot,
    target_dir: &str,
    target_layout: Option<&HashMap<Layer, String>>,
) -> Option<RefactorPlan> {
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
    let mut layer_before: HashMap<String, usize> = HashMap::new();
    let mut layer_after: HashMap<String, usize> = HashMap::new();

    for file in &focus.core {
        let current_layer = detect_layer(&file.path);
        *layer_before
            .entry(current_layer.display_name().to_string())
            .or_default() += 1;

        // Analyze impact for this file
        let impact = analyze_impact(snapshot, &file.path, &impact_opts);

        // Determine target layer
        let target_layer = if current_layer == Layer::Unknown {
            // Try to infer from content or neighbors
            infer_target_layer(&file.path, snapshot)
        } else {
            current_layer
        };

        *layer_after
            .entry(target_layer.display_name().to_string())
            .or_default() += 1;

        // Only create a move if layer changed or file is misplaced
        let needs_move =
            current_layer == Layer::Unknown || !file.path.contains(current_layer.canonical_dir());

        if needs_move {
            let in_cycle = files_in_cycles.contains(file.path.as_str());
            let risk = calculate_risk(
                impact.direct_consumers.len(),
                impact.transitive_consumers.len(),
                file.loc,
                in_cycle,
            );

            // Build target path
            let target_path =
                build_target_path(&file.path, target_layer, target_layout, target_dir);

            let reason = build_move_reason(current_layer, target_layer, in_cycle);
            let verify_cmd = format!("loct impact {}", target_path);

            let affected_files: Vec<String> = impact
                .direct_consumers
                .iter()
                .map(|e| e.file.clone())
                .collect();

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

    if moves.is_empty() {
        return None;
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
    })
}

/// Infer target layer from file content and neighbors.
fn infer_target_layer(file_path: &str, snapshot: &Snapshot) -> Layer {
    // Check what the file imports
    let imports: Vec<&str> = snapshot
        .edges
        .iter()
        .filter(|e| e.from == file_path)
        .map(|e| e.to.as_str())
        .collect();

    // Count layers of imported files
    let mut layer_counts: HashMap<Layer, usize> = HashMap::new();
    for imp in imports {
        let layer = detect_layer(imp);
        if layer != Layer::Unknown {
            *layer_counts.entry(layer).or_default() += 1;
        }
    }

    // Return most common layer, or Infra as default
    layer_counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(layer, _)| layer)
        .unwrap_or(Layer::Infra)
}

/// Build target path for a file move.
fn build_target_path(
    source: &str,
    layer: Layer,
    custom_layout: Option<&HashMap<Layer, String>>,
    base_dir: &str,
) -> String {
    let filename = Path::new(source)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(source);

    let layer_dir = custom_layout
        .and_then(|m| m.get(&layer))
        .map(|s| s.as_str())
        .unwrap_or_else(|| layer.canonical_dir());

    format!(
        "{}/{}/{}",
        base_dir.trim_end_matches('/'),
        layer_dir,
        filename
    )
}

/// Build reason string for a move.
fn build_move_reason(current: Layer, target: Layer, in_cycle: bool) -> String {
    let mut parts = Vec::new();

    if current == Layer::Unknown {
        parts.push(format!("Unclassified → {}", target.display_name()));
    } else if current != target {
        parts.push(format!(
            "{} → {}",
            current.display_name(),
            target.display_name()
        ));
    }

    if in_cycle {
        parts.push("In cycle".to_string());
    }

    if parts.is_empty() {
        "Misplaced file".to_string()
    } else {
        parts.join(", ")
    }
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
    let mut lines: Vec<String> = Vec::new();

    for mv in moves {
        // Ensure parent directory exists
        if let Some(parent) = Path::new(&mv.target).parent() {
            lines.push(format!("mkdir -p {}", parent.display()));
        }
        lines.push(format!("git mv {} {}", mv.source, mv.target));
    }

    lines.join("\n")
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(result, "./b");
    }

    #[test]
    fn test_calculate_relative_import_parent_dir() {
        let result = calculate_relative_import("src/components/Button.tsx", "src/utils/format.ts");
        assert_eq!(result, "../utils/format");
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
