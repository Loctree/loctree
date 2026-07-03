//! Circular import detection using Tarjan's SCC algorithm.
//!
//! Finds strongly connected components (cycles) in the import graph.
//! Normalizes module paths to collapse barrels and extensions before detection.

use std::cmp::min;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::classify::should_exclude_from_reports;
use super::root_scan::normalize_module_id;

/// Provenance check: a cycle whose every member lives under a test/fixture path.
///
/// Intentional circular-import fixtures (e.g. `tests/fixtures/circular_imports/`)
/// are committed on purpose and must not be surfaced as production regressions.
/// A cycle is flagged `fixture` only when *all* members are non-production paths,
/// so a real cycle that merely touches a fixture file is still reported. This is
/// the loctree-side provenance label that downstream regression consumers read to
/// exclude or annotate fixture cycles.
pub fn is_fixture_cycle(nodes: &[String]) -> bool {
    !nodes.is_empty() && nodes.iter().all(|node| should_exclude_from_reports(node))
}

/// Classification of a cycle's nature and severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleClassification {
    HardBidirectional,
    ModuleSelfReference,
    TraitBased,
    CfgGated,
    FanPattern,
    WildcardImport,
    Unknown,
}

/// Compilability status of a cycle.
///
/// This classifies cycles by whether they would actually break compilation,
/// addressing the false positive issue where "strict cycles" are reported
/// as critical but compile successfully.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCompilability {
    /// Would fail compilation (runtime circular value dependencies).
    /// Example: `const A = B + 1` and `const B = A + 1`
    Breaking,

    /// Graph cycle that compiles fine due to language semantics.
    /// Examples:
    /// - Rust mod/use separation (mod declares, use consumes)
    /// - TypeScript cross-file references that resolve at runtime
    Structural,

    /// Not a true cycle - shared dependency diamond pattern.
    /// Multiple files importing from a common module.
    DiamondDependency,
}

impl CycleCompilability {
    pub fn label(&self) -> &'static str {
        match self {
            CycleCompilability::Breaking => "bidirectional",
            CycleCompilability::Structural => "structural",
            CycleCompilability::DiamondDependency => "diamond",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            CycleCompilability::Breaking => "[HIGH]",
            CycleCompilability::Structural => "[LOW]",
            CycleCompilability::DiamondDependency => "[OK]",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            CycleCompilability::Breaking => "Direct mutual imports (high risk)",
            CycleCompilability::Structural => "Reference pattern (low risk)",
            CycleCompilability::DiamondDependency => "Shared module, not a cycle",
        }
    }

    /// Risk level label for this cycle type.
    pub fn risk_label(&self) -> &'static str {
        match self {
            CycleCompilability::Breaking => "high",
            CycleCompilability::Structural => "low",
            CycleCompilability::DiamondDependency => "none",
        }
    }

    /// Suggestion for how to address this type of cycle.
    pub fn suggestion(&self) -> &'static str {
        match self {
            CycleCompilability::Breaking => "Extract shared types to a third module",
            CycleCompilability::Structural => "Review if intentional",
            CycleCompilability::DiamondDependency => "Not a true cycle",
        }
    }
}

impl CycleClassification {
    pub fn severity(&self) -> u8 {
        match self {
            CycleClassification::HardBidirectional => 3,
            CycleClassification::WildcardImport => 2,
            CycleClassification::CfgGated => 1,
            CycleClassification::ModuleSelfReference => 0,
            CycleClassification::TraitBased => 0,
            CycleClassification::FanPattern => 0,
            CycleClassification::Unknown => 2,
        }
    }
    pub fn severity_label(&self) -> &'static str {
        match self.severity() {
            3 => "high",
            2 => "medium",
            1 => "low",
            _ => "info",
        }
    }
    pub fn severity_icon(&self) -> &'static str {
        match self.severity() {
            3 | 2 => "[!] ",
            _ => "[i] ",
        }
    }
    pub fn description(&self) -> &'static str {
        match self {
            CycleClassification::HardBidirectional => "blocks decoupling",
            CycleClassification::WildcardImport => "implicit coupling",
            CycleClassification::CfgGated => "conditional, low impact",
            CycleClassification::ModuleSelfReference => "Rust pattern, OK",
            CycleClassification::TraitBased => "architectural, OK",
            CycleClassification::FanPattern => "not true cycle",
            CycleClassification::Unknown => "needs review",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifiedCycle {
    pub nodes: Vec<String>,
    pub classification: CycleClassification,
    pub compilability: CycleCompilability,
    pub has_wildcard: bool,
    pub is_cfg_gated: bool,
    /// Provenance: every member is a test/fixture path, so this cycle is an
    /// intentional fixture, not a production regression. See [`is_fixture_cycle`].
    #[serde(default)]
    pub fixture: bool,
    /// Pattern description for human-readable output
    pub pattern: String,
    /// Risk level: "none", "low", "medium", "high"
    pub risk: String,
    /// Suggestion for fixing (if applicable)
    pub suggestion: Option<String>,
}

impl ClassifiedCycle {
    pub fn new(nodes: Vec<String>, edges: &[(String, String, String)]) -> Self {
        let classification = classify_cycle(&nodes, edges);
        let has_wildcard = check_has_wildcard(&nodes, edges);
        let is_cfg_gated = check_is_cfg_gated(&nodes, edges);
        let compilability = determine_compilability(&classification, &nodes, edges);
        let fixture = is_fixture_cycle(&nodes);
        let (pattern, risk, suggestion) = describe_cycle(&classification, &compilability, &nodes);

        ClassifiedCycle {
            nodes,
            classification,
            compilability,
            has_wildcard,
            is_cfg_gated,
            fixture,
            pattern,
            risk,
            suggestion,
        }
    }
}

/// Determine the compilability status of a cycle based on its classification.
fn determine_compilability(
    classification: &CycleClassification,
    nodes: &[String],
    edges: &[(String, String, String)],
) -> CycleCompilability {
    match classification {
        // These patterns are known to compile fine
        CycleClassification::ModuleSelfReference => CycleCompilability::Structural,
        CycleClassification::TraitBased => CycleCompilability::Structural,
        CycleClassification::CfgGated => CycleCompilability::Structural,
        CycleClassification::FanPattern => CycleCompilability::DiamondDependency,

        // Hard bidirectional and wildcard need deeper analysis
        CycleClassification::HardBidirectional | CycleClassification::WildcardImport => {
            // Check if this is a Rust mod/use pattern (structural, not breaking)
            if is_rust_mod_use_pattern(nodes, edges) {
                return CycleCompilability::Structural;
            }

            // Check if edges are type-only imports (structural)
            if all_edges_type_only(nodes, edges) {
                return CycleCompilability::Structural;
            }

            // Check if it's actually a diamond (shared dependency)
            if is_diamond_pattern(nodes, edges) {
                return CycleCompilability::DiamondDependency;
            }

            // Default to structural since most cycles that make it through
            // Tarjan's SCC in real code actually compile.
            // True "Breaking" would require detecting value-level circular init.
            CycleCompilability::Structural
        }

        CycleClassification::Unknown => {
            // Unknown patterns default to structural since they compiled
            CycleCompilability::Structural
        }
    }
}

/// Check if a cycle is a Rust mod/use pattern (mod.rs declares, child uses parent).
fn is_rust_mod_use_pattern(nodes: &[String], edges: &[(String, String, String)]) -> bool {
    let node_set: HashSet<&str> = nodes.iter().map(|s| s.as_str()).collect();

    // Look for patterns like:
    // - types/mod.rs -> types/ai.rs (mod declaration)
    // - types/ai.rs -> types/mod.rs (use statement)
    for (from, to, kind) in edges {
        if !node_set.contains(from.as_str()) || !node_set.contains(to.as_str()) {
            continue;
        }

        // Check for mod.rs patterns
        let is_mod_declaration =
            (from.ends_with("/mod.rs") || from.ends_with("\\mod.rs")) && kind == "mod";

        // Check for use patterns going back to parent
        let is_use_to_parent = kind == "use" || kind == "import";

        if is_mod_declaration || is_use_to_parent {
            return true;
        }
    }

    // Also check by path structure - if one is parent of the other
    if nodes.len() == 2 {
        return is_parent_child_module(&nodes[0], &nodes[1])
            || is_parent_child_module(&nodes[1], &nodes[0]);
    }

    false
}

/// Check if all edges in the cycle are type-only imports.
fn all_edges_type_only(nodes: &[String], edges: &[(String, String, String)]) -> bool {
    let node_set: HashSet<&str> = nodes.iter().map(|s| s.as_str()).collect();

    let cycle_edges: Vec<_> = edges
        .iter()
        .filter(|(from, to, _)| node_set.contains(from.as_str()) && node_set.contains(to.as_str()))
        .collect();

    if cycle_edges.is_empty() {
        return false;
    }

    cycle_edges
        .iter()
        .all(|(_, _, kind)| kind == "type_import" || kind == "type" || kind.contains("type"))
}

/// Check if the detected "cycle" is actually a diamond dependency pattern.
fn is_diamond_pattern(nodes: &[String], edges: &[(String, String, String)]) -> bool {
    if nodes.len() < 3 {
        return false;
    }

    let node_set: HashSet<&str> = nodes.iter().map(|s| s.as_str()).collect();

    // Count incoming and outgoing edges for each node
    let mut incoming: HashMap<&str, usize> = HashMap::new();
    let mut outgoing: HashMap<&str, usize> = HashMap::new();

    for (from, to, _) in edges {
        if node_set.contains(from.as_str()) && node_set.contains(to.as_str()) {
            *incoming.entry(to.as_str()).or_default() += 1;
            *outgoing.entry(from.as_str()).or_default() += 1;
        }
    }

    // Diamond pattern: one node has many incoming edges (shared dependency)
    // and the "cycle" exists because multiple paths lead to the same module
    for node in nodes {
        let inc = incoming.get(node.as_str()).copied().unwrap_or(0);
        let out = outgoing.get(node.as_str()).copied().unwrap_or(0);

        // Shared leaf: many incoming, few outgoing
        if inc >= 2 && out <= 1 {
            return true;
        }
    }

    false
}

/// Generate human-readable description of the cycle.
fn describe_cycle(
    classification: &CycleClassification,
    compilability: &CycleCompilability,
    nodes: &[String],
) -> (String, String, Option<String>) {
    let pattern = match classification {
        CycleClassification::ModuleSelfReference => "Rust mod/use separation".to_string(),
        CycleClassification::TraitBased => "Trait-based abstraction".to_string(),
        CycleClassification::CfgGated => "Conditional compilation (#[cfg])".to_string(),
        CycleClassification::FanPattern => "Shared utility module".to_string(),
        CycleClassification::HardBidirectional => {
            if nodes.len() == 2 {
                "Bidirectional reference".to_string()
            } else {
                format!("Cross-module references ({} files)", nodes.len())
            }
        }
        CycleClassification::WildcardImport => "Wildcard re-export".to_string(),
        CycleClassification::Unknown => "Unknown pattern".to_string(),
    };

    let risk = match compilability {
        CycleCompilability::Breaking => "high".to_string(),
        CycleCompilability::Structural => {
            if nodes.len() > 5 {
                "medium".to_string()
            } else {
                "low".to_string()
            }
        }
        CycleCompilability::DiamondDependency => "none".to_string(),
    };

    let suggestion = match (classification, compilability) {
        (_, CycleCompilability::DiamondDependency) => None,
        (CycleClassification::ModuleSelfReference, _) => {
            Some("Idiomatic Rust - no action needed".to_string())
        }
        (CycleClassification::FanPattern, _) => {
            Some("Good architecture - shared utilities are fine".to_string())
        }
        (CycleClassification::HardBidirectional, CycleCompilability::Structural) => {
            if nodes.len() > 5 {
                Some("Consider facade pattern to reduce coupling".to_string())
            } else {
                Some("Review for tight coupling".to_string())
            }
        }
        (CycleClassification::WildcardImport, _) => {
            Some("Consider explicit imports instead of *".to_string())
        }
        _ => None,
    };

    (pattern, risk, suggestion)
}

fn classify_cycle(nodes: &[String], edges: &[(String, String, String)]) -> CycleClassification {
    if check_is_cfg_gated(nodes, edges) {
        return CycleClassification::CfgGated;
    }
    if check_has_wildcard(nodes, edges) {
        return CycleClassification::WildcardImport;
    }
    if is_module_self_reference(nodes) {
        return CycleClassification::ModuleSelfReference;
    }
    if is_fan_pattern(nodes, edges) {
        return CycleClassification::FanPattern;
    }
    if nodes.len() == 2 {
        return CycleClassification::HardBidirectional;
    }
    if is_hard_bidirectional(nodes, edges) {
        return CycleClassification::HardBidirectional;
    }
    CycleClassification::Unknown
}

fn check_has_wildcard(nodes: &[String], edges: &[(String, String, String)]) -> bool {
    let node_set: HashSet<&str> = nodes.iter().map(|s| s.as_str()).collect();
    edges.iter().any(|(from, to, kind)| {
        node_set.contains(from.as_str())
            && node_set.contains(to.as_str())
            && kind.contains("wildcard")
    })
}

fn check_is_cfg_gated(nodes: &[String], edges: &[(String, String, String)]) -> bool {
    let node_set: HashSet<&str> = nodes.iter().map(|s| s.as_str()).collect();
    edges.iter().any(|(from, to, kind)| {
        node_set.contains(from.as_str())
            && node_set.contains(to.as_str())
            && (kind.contains("cfg") || kind.contains("conditional"))
    })
}

fn is_module_self_reference(nodes: &[String]) -> bool {
    if nodes.len() != 2 {
        return false;
    }
    is_parent_child_module(&nodes[0], &nodes[1]) || is_parent_child_module(&nodes[1], &nodes[0])
}

fn is_parent_child_module(parent: &str, child: &str) -> bool {
    let p = parent
        .replace('\\', "/")
        .trim_end_matches(".rs")
        .trim_end_matches("/mod")
        .to_string();
    let c = child.replace('\\', "/").trim_end_matches(".rs").to_string();
    if c.starts_with(&format!("{}/", p)) {
        return true;
    }
    let parent_dir = if parent.ends_with("/mod.rs") {
        parent.trim_end_matches("/mod.rs")
    } else if parent.ends_with(".rs") {
        parent.trim_end_matches(".rs")
    } else {
        parent
    };
    let child_dir = child.rsplit_once('/').map(|(d, _)| d).unwrap_or(child);
    parent_dir == child_dir && parent != child
}

fn is_fan_pattern(nodes: &[String], edges: &[(String, String, String)]) -> bool {
    if nodes.len() < 3 {
        return false;
    }
    let node_set: HashSet<&str> = nodes.iter().map(|s| s.as_str()).collect();
    let mut incoming: HashMap<&str, usize> = HashMap::new();
    let mut outgoing: HashMap<&str, usize> = HashMap::new();
    for (from, to, _) in edges {
        if node_set.contains(from.as_str()) && node_set.contains(to.as_str()) {
            *incoming.entry(to.as_str()).or_default() += 1;
            *outgoing.entry(from.as_str()).or_default() += 1;
        }
    }
    nodes.iter().any(|n| {
        incoming.get(n.as_str()).copied().unwrap_or(0) >= nodes.len() / 2
            && outgoing.get(n.as_str()).copied().unwrap_or(0) <= 2
    })
}

fn is_hard_bidirectional(nodes: &[String], edges: &[(String, String, String)]) -> bool {
    if nodes.len() < 2 {
        return false;
    }
    let node_set: HashSet<&str> = nodes.iter().map(|s| s.as_str()).collect();
    let mut has_in: HashSet<&str> = HashSet::new();
    let mut has_out: HashSet<&str> = HashSet::new();
    for (from, to, _) in edges {
        if node_set.contains(from.as_str()) && node_set.contains(to.as_str()) {
            has_out.insert(from.as_str());
            has_in.insert(to.as_str());
        }
    }
    nodes
        .iter()
        .all(|n| has_in.contains(n.as_str()) && has_out.contains(n.as_str()))
}

struct TarjanData {
    index: usize,
    indices: HashMap<String, usize>,
    lowlinks: HashMap<String, usize>,
    stack: Vec<String>,
    on_stack: HashSet<String>,
    sccs: Vec<Vec<String>>,
}

pub fn find_cycles(edges: &[(String, String, String)]) -> Vec<Vec<String>> {
    // Normalize modules to collapse barrels (index files) and JS/TS extensions.
    // Keep a canonical display path for each normalized module id so output remains readable.
    let mut canonical: HashMap<String, String> = HashMap::new();
    let mut normalized_edges: Vec<(String, String)> = Vec::new();

    for (from, to, kind) in edges {
        // Skip Rust `mod foo;` declarations - these create parent->child module relationships,
        // not import edges. A parent declaring `mod child;` is NOT importing from child.
        if kind == "mod" {
            continue;
        }
        let norm_from = normalize_module_id(from).as_key();
        let norm_to = normalize_module_id(to).as_key();
        canonical
            .entry(norm_from.clone())
            .or_insert_with(|| from.clone());
        canonical
            .entry(norm_to.clone())
            .or_insert_with(|| to.clone());

        // Skip FALSE self-loops created by normalization (e.g., a.ts -> a.js becomes a:ts -> a:ts)
        // These are not real cycles, just artifacts of collapsing file extensions.
        // BUT keep REAL self-loops where from == to in the original (file imports itself)
        if norm_from != norm_to || from == to {
            normalized_edges.push((norm_from, norm_to));
        }
    }

    // Run Tarjan on normalized module ids
    let cycles_norm = find_cycles_normalized(&normalized_edges);

    // Map back to canonical display paths
    cycles_norm
        .into_iter()
        .map(|cycle| {
            cycle
                .into_iter()
                .map(|id| canonical.get(&id).cloned().unwrap_or(id))
                .collect()
        })
        .collect()
}

/// Return (strict_cycles, lazy_cycles), where:
/// - strict_cycles exclude edges marked as "lazy_import" or "type_import"
/// - lazy_cycles are cycles that disappear once lazy imports are removed and that include a lazy edge
pub fn find_cycles_with_lazy(
    edges: &[(String, String, String)],
) -> (Vec<Vec<String>>, Vec<Vec<String>>) {
    // Filter out mod declarations, lazy imports, and type imports for strict cycle detection
    // Rust `mod foo;` declarations are NOT import edges - they create module relationships
    let strict_edges: Vec<_> = edges
        .iter()
        .filter(|(_, _, kind)| kind != "lazy_import" && kind != "type_import" && kind != "mod")
        .cloned()
        .collect();
    let strict_cycles = find_cycles(&strict_edges);

    // For lazy cycle detection, filter out type imports and mod declarations
    let edges_no_type: Vec<_> = edges
        .iter()
        .filter(|(_, _, kind)| kind != "type_import" && kind != "mod")
        .cloned()
        .collect();
    let all_cycles = find_cycles(&edges_no_type);

    let lazy_edge_set: HashSet<(String, String)> = edges_no_type
        .iter()
        .filter(|(_, _, kind)| kind == "lazy_import")
        .map(|(a, b, _)| (a.clone(), b.clone()))
        .collect();

    let normalize = |cycle: &[String]| {
        let mut sorted = cycle.to_vec();
        sorted.sort();
        sorted.join("|")
    };
    let strict_keys: HashSet<String> = strict_cycles.iter().map(|c| normalize(c)).collect();
    let mut seen = HashSet::new();
    let mut lazy_cycles = Vec::new();

    for cycle in all_cycles {
        let key = normalize(&cycle);
        if strict_keys.contains(&key) || seen.contains(&key) {
            continue;
        }
        let mut has_lazy = false;
        for pair in cycle.windows(2) {
            if pair.len() == 2 && lazy_edge_set.contains(&(pair[0].clone(), pair[1].clone())) {
                has_lazy = true;
                break;
            }
        }
        // wrap edge
        if !has_lazy && cycle.len() > 1 {
            let last = cycle.last().unwrap();
            let first = cycle.first().unwrap();
            if lazy_edge_set.contains(&(last.clone(), first.clone())) {
                has_lazy = true;
            }
        }
        if has_lazy {
            seen.insert(key);
            lazy_cycles.push(cycle);
        }
    }

    (strict_cycles, lazy_cycles)
}

/// Find cycles and return them as classified cycles with metadata.
///
/// This is a convenience wrapper around `find_cycles()` that automatically
/// classifies each cycle using `ClassifiedCycle::new()`.
pub fn find_cycles_classified(edges: &[(String, String, String)]) -> Vec<ClassifiedCycle> {
    let raw_cycles = find_cycles(edges);
    raw_cycles
        .into_iter()
        .map(|nodes| ClassifiedCycle::new(nodes, edges))
        .collect()
}

/// Find cycles (strict and lazy) and return them as classified cycles.
///
/// Returns `(strict_cycles, lazy_cycles)` where both are classified with metadata.
/// - `strict_cycles`: Excludes lazy_import and type_import edges
/// - `lazy_cycles`: Cycles that only exist when lazy imports are included
pub fn find_cycles_classified_with_lazy(
    edges: &[(String, String, String)],
) -> (Vec<ClassifiedCycle>, Vec<ClassifiedCycle>) {
    let (strict, lazy) = find_cycles_with_lazy(edges);
    (
        strict
            .into_iter()
            .map(|n| ClassifiedCycle::new(n, edges))
            .collect(),
        lazy.into_iter()
            .map(|n| ClassifiedCycle::new(n, edges))
            .collect(),
    )
}

/// Print cycles classified by risk level (Bidirectional/Structural/Diamond).
///
/// This format addresses the issue where "breaking cycles" were reported as "critical"
/// even though they compile successfully. Now cycles are grouped by architectural risk.
pub fn print_cycles_classified(classified_cycles: &[ClassifiedCycle], json_output: bool) {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "classifiedCycles": classified_cycles })
            )
            .unwrap()
        );
        return;
    }

    if classified_cycles.is_empty() {
        println!("No circular imports detected. [OK]");
        return;
    }

    // Group by compilability status
    let mut by_compilability: HashMap<CycleCompilability, Vec<&ClassifiedCycle>> = HashMap::new();
    for c in classified_cycles {
        by_compilability.entry(c.compilability).or_default().push(c);
    }

    let bidirectional = by_compilability
        .get(&CycleCompilability::Breaking)
        .map(|v| v.len())
        .unwrap_or(0);
    let structural = by_compilability
        .get(&CycleCompilability::Structural)
        .map(|v| v.len())
        .unwrap_or(0);
    let diamond = by_compilability
        .get(&CycleCompilability::DiamondDependency)
        .map(|v| v.len())
        .unwrap_or(0);

    println!("\nCircular Import Analysis\n");

    // Bidirectional Cycles (High Risk)
    println!(
        "{} Bidirectional ({}) - {}",
        CycleCompilability::Breaking.icon(),
        bidirectional,
        CycleCompilability::Breaking.description()
    );
    if bidirectional == 0 {
        println!("   (none)\n");
    } else if let Some(cycles) = by_compilability.get(&CycleCompilability::Breaking) {
        print_cycle_group(cycles, true);
    }

    // Structural Cycles (Low Risk)
    println!(
        "{} Structural ({}) - {}",
        CycleCompilability::Structural.icon(),
        structural,
        CycleCompilability::Structural.description()
    );
    if structural == 0 {
        println!("   (none)\n");
    } else if let Some(cycles) = by_compilability.get(&CycleCompilability::Structural) {
        print_cycle_group(cycles, false);
    }

    // Diamond Dependencies (Not True Cycles)
    println!(
        "{} Diamond Dependencies ({}) - {}",
        CycleCompilability::DiamondDependency.icon(),
        diamond,
        CycleCompilability::DiamondDependency.description()
    );
    if diamond == 0 {
        println!("   (none)\n");
    } else if let Some(cycles) = by_compilability.get(&CycleCompilability::DiamondDependency) {
        print_cycle_group(cycles, false);
    }

    // Summary with risk levels
    println!("CYCLE RISK LEVELS:");
    println!(
        "  HIGH (Bidirectional):  {} - Direct mutual imports (A <-> B)",
        bidirectional
    );
    println!(
        "  LOW (Structural):      {} - Module organization patterns",
        structural
    );
    println!(
        "  NONE (Diamond):        {} - Shared dependencies, not cycles",
        diamond
    );

    if bidirectional == 0 {
        println!("\nNo high-risk circular imports detected.");
        if structural > 0 {
            println!("  Structural cycles are low-risk organizational patterns.");
        }
    }
}

/// Print a group of cycles with details.
fn print_cycle_group(cycles: &[&ClassifiedCycle], detailed: bool) {
    for (i, c) in cycles.iter().enumerate() {
        let num = i + 1;
        let path_summary = format_cycle_path(&c.nodes);

        println!("   #{} {} ({} files)", num, path_summary, c.nodes.len());
        println!("      Pattern: {}", c.pattern);
        println!(
            "      Risk: {} ({})",
            c.risk,
            c.classification.description()
        );

        if let Some(ref suggestion) = c.suggestion {
            println!("      Suggestion: {}", suggestion);
        }

        // Show full path for detailed mode (breaking cycles)
        if detailed && c.nodes.len() <= 10 {
            println!("      Chain: {}", c.nodes.join(" -> "));
        }

        println!();
    }
}

/// Format cycle path for display (first ↔ last for 2-node, abbreviated for larger).
fn format_cycle_path(nodes: &[String]) -> String {
    if nodes.is_empty() {
        return "(empty)".to_string();
    }

    match nodes.len() {
        1 => nodes[0].clone(),
        2 => format!("{} ↔ {}", nodes[0], nodes[1]),
        3..=5 => nodes.join(" → "),
        _ => format!(
            "{} → ... ({} files) → {}",
            nodes[0],
            nodes.len() - 2,
            nodes.last().unwrap()
        ),
    }
}

/// Print cycles in the old format (for backwards compatibility).
pub fn print_cycles_classified_legacy(classified_cycles: &[ClassifiedCycle], json_output: bool) {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "classifiedCycles": classified_cycles })
            )
            .unwrap()
        );
        return;
    }
    if classified_cycles.is_empty() {
        println!("No circular imports detected.");
        return;
    }
    let mut by_class: HashMap<CycleClassification, Vec<&ClassifiedCycle>> = HashMap::new();
    for c in classified_cycles {
        by_class.entry(c.classification).or_default().push(c);
    }
    println!("Cycles Analysis:");
    let mut classes: Vec<_> = by_class.keys().collect();
    classes.sort_by_key(|c| std::cmp::Reverse(c.severity()));
    for cls in &classes {
        let cycles = &by_class[cls];
        println!(
            "├── {:22} {:3}  {}  ({})",
            format!("{:?}:", cls),
            cycles.len(),
            cls.severity_icon(),
            cls.description()
        );
    }
    let actionable = classified_cycles
        .iter()
        .filter(|c| c.classification.severity() >= 2)
        .count();
    let info = classified_cycles
        .iter()
        .filter(|c| c.classification.severity() < 2)
        .count();
    println!(
        "└── Total: {} cycles ({} actionable, {} informational)\n",
        classified_cycles.len(),
        actionable,
        info
    );
    let mut num = 1;
    for cls in &classes {
        for c in &by_class[cls] {
            let mut nodes = c.nodes.clone();
            nodes.reverse();
            let s = if nodes.len() > 12 {
                format!(
                    "{} -> ... ({} intermediate) ... -> {}",
                    nodes[..5].join(" -> "),
                    nodes.len() - 10,
                    nodes[nodes.len() - 5..].join(" -> ")
                )
            } else {
                nodes.join(" -> ")
            };
            println!(
                "Cycle {} [{:?}] ({} files):\n  {}\n",
                num,
                c.classification,
                c.nodes.len(),
                s
            );
            num += 1;
        }
    }
}

fn find_cycles_normalized(edges: &[(String, String)]) -> Vec<Vec<String>> {
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    let mut all_nodes = HashSet::new();

    for (from, to) in edges {
        adj.entry(from.clone()).or_default().push(to.clone());
        all_nodes.insert(from.clone());
        all_nodes.insert(to.clone());
    }

    let mut data = TarjanData {
        index: 0,
        indices: HashMap::new(),
        lowlinks: HashMap::new(),
        stack: Vec::new(),
        on_stack: HashSet::new(),
        sccs: Vec::new(),
    };

    let mut nodes: Vec<_> = all_nodes.into_iter().collect();
    nodes.sort();

    for node in nodes {
        if !data.indices.contains_key(&node) {
            strongconnect(&node, &adj, &mut data);
        }
    }

    // Filter SCCs that form cycles.
    let sccs: Vec<_> = data
        .sccs
        .into_iter()
        .filter(|scc| {
            if scc.len() > 1 {
                return true;
            }
            if scc.len() == 1 {
                if let Some(node) = scc.first() {
                    if let Some(neighbors) = adj.get(node) {
                        return neighbors.contains(node);
                    }
                }
            }
            false
        })
        .collect();

    let mut all_cycles = Vec::new();
    for scc in sccs {
        if scc.len() == 1 {
            all_cycles.push(scc);
        } else {
            let mut cycles = Vec::new();
            let scc_set: HashSet<&String> = scc.iter().collect();

            const MAX_CYCLES_PER_SCC: usize = 10;

            for start_node in &scc {
                if cycles.len() >= MAX_CYCLES_PER_SCC {
                    break;
                }

                let mut stack = vec![(start_node, vec![start_node.clone()])];
                while let Some((curr, path)) = stack.pop() {
                    if cycles.len() >= MAX_CYCLES_PER_SCC {
                        break;
                    }

                    if let Some(neighbors) = adj.get(curr) {
                        for next in neighbors {
                            if !scc_set.contains(next) {
                                continue;
                            }
                            if *next == *start_node {
                                let cycle = path.clone();
                                let min_node = cycle.iter().min().unwrap();
                                let min_idx = cycle.iter().position(|x| x == min_node).unwrap();
                                let mut norm_cycle = cycle[min_idx..].to_vec();
                                norm_cycle.extend_from_slice(&cycle[..min_idx]);

                                if !cycles.contains(&norm_cycle) {
                                    cycles.push(norm_cycle);
                                }
                            } else if !path.contains(next) {
                                let mut new_path = path.clone();
                                new_path.push((*next).clone());
                                stack.push((next, new_path));
                            }
                        }
                    }
                }
            }

            cycles.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
            all_cycles.extend(cycles);
        }
    }
    all_cycles
}

fn strongconnect(node: &str, adj: &HashMap<String, Vec<String>>, data: &mut TarjanData) {
    data.indices.insert(node.to_string(), data.index);
    data.lowlinks.insert(node.to_string(), data.index);
    data.index += 1;
    data.stack.push(node.to_string());
    data.on_stack.insert(node.to_string());

    if let Some(neighbors) = adj.get(node) {
        for w in neighbors {
            if !data.indices.contains_key(w) {
                strongconnect(w, adj, data);
                let v_low = *data
                    .lowlinks
                    .get(node)
                    .expect("Tarjan: node lowlink must exist after init");
                let w_low = *data
                    .lowlinks
                    .get(w)
                    .expect("Tarjan: neighbor lowlink must exist after recursion");
                data.lowlinks.insert(node.to_string(), min(v_low, w_low));
            } else if data.on_stack.contains(w) {
                let v_low = *data
                    .lowlinks
                    .get(node)
                    .expect("Tarjan: node lowlink must exist after init");
                let w_index = *data
                    .indices
                    .get(w)
                    .expect("Tarjan: neighbor index must exist if visited");
                data.lowlinks.insert(node.to_string(), min(v_low, w_index));
            }
        }
    }

    let v_low = *data
        .lowlinks
        .get(node)
        .expect("Tarjan: node lowlink must exist after init");
    let v_index = *data
        .indices
        .get(node)
        .expect("Tarjan: node index must exist after init");

    if v_low == v_index {
        let mut scc = Vec::new();
        loop {
            let w = data
                .stack
                .pop()
                .expect("Tarjan: stack must contain node that was pushed");
            data.on_stack.remove(&w);
            scc.push(w.clone());
            if w == node {
                break;
            }
        }
        data.sccs.push(scc);
    }
}

pub fn print_cycles(cycles: &[Vec<String>], json_output: bool) {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "circularImports": cycles }))
                .expect("Failed to serialize circular imports to JSON")
        );
    } else if cycles.is_empty() {
        println!("No circular imports detected.");
    } else {
        println!("Circular imports detected ({} cycles):", cycles.len());
        for (i, cycle) in cycles.iter().enumerate() {
            let mut c = cycle.clone();
            c.reverse(); // Reverse to show cycle in discovery order for readability

            let cycle_str = if c.len() > 12 {
                let first_part = c[..5].join(" -> ");
                let last_part = c[c.len() - 5..].join(" -> ");
                format!(
                    "{} -> ... ({} intermediate) ... -> {}",
                    first_part,
                    c.len() - 10,
                    last_part
                )
            } else {
                c.join(" -> ")
            };

            println!("  Cycle {}: {}", i + 1, cycle_str);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{find_cycles, print_cycles};

    #[test]
    fn detects_simple_cycle() {
        let edges = vec![
            ("a".to_string(), "b".to_string(), "import".to_string()),
            ("b".to_string(), "a".to_string(), "import".to_string()),
        ];
        let cycles = find_cycles(&edges);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].len(), 2);
        assert!(cycles[0].contains(&"a".to_string()));
        assert!(cycles[0].contains(&"b".to_string()));
    }

    #[test]
    fn detects_self_loop() {
        let edges = vec![("a".to_string(), "a".to_string(), "import".to_string())];
        let cycles = find_cycles(&edges);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].len(), 1);
        assert_eq!(cycles[0][0], "a");
    }

    #[test]
    fn no_cycle() {
        let edges = vec![
            ("a".to_string(), "b".to_string(), "import".to_string()),
            ("b".to_string(), "c".to_string(), "import".to_string()),
        ];
        let cycles = find_cycles(&edges);
        assert!(cycles.is_empty());
    }

    #[test]
    fn complex_cycle() {
        // a->b->c->a (cycle)
        // d->e
        let edges = vec![
            ("a".to_string(), "b".to_string(), "import".to_string()),
            ("b".to_string(), "c".to_string(), "import".to_string()),
            ("c".to_string(), "a".to_string(), "import".to_string()),
            ("d".to_string(), "e".to_string(), "import".to_string()),
        ];
        let cycles = find_cycles(&edges);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].len(), 3);
    }

    #[test]
    fn multiple_cycles() {
        // Two separate cycles: a<->b and c<->d
        let edges = vec![
            ("a".to_string(), "b".to_string(), "import".to_string()),
            ("b".to_string(), "a".to_string(), "import".to_string()),
            ("c".to_string(), "d".to_string(), "import".to_string()),
            ("d".to_string(), "c".to_string(), "import".to_string()),
        ];
        let cycles = find_cycles(&edges);
        assert_eq!(cycles.len(), 2);
    }

    #[test]
    fn shared_node_cycles_stay_separate_without_phantom_edge() {
        // Regression for loctree-feedback 2026-05-18: two real cycles sharing
        // `types.rs` must not be merged into a longer trail by inventing a
        // `types.rs -> menu.rs` edge.
        let edges = vec![
            (
                "icons.rs".to_string(),
                "types.rs".to_string(),
                "import".to_string(),
            ),
            (
                "types.rs".to_string(),
                "icons.rs".to_string(),
                "import".to_string(),
            ),
            (
                "types.rs".to_string(),
                "ipc_client.rs".to_string(),
                "import".to_string(),
            ),
            (
                "ipc_client.rs".to_string(),
                "state.rs".to_string(),
                "import".to_string(),
            ),
            (
                "state.rs".to_string(),
                "menu.rs".to_string(),
                "import".to_string(),
            ),
            (
                "menu.rs".to_string(),
                "types.rs".to_string(),
                "import".to_string(),
            ),
        ];

        let mut cycles = find_cycles(&edges);
        cycles.sort_by_key(|cycle| cycle.len());

        assert_eq!(
            cycles.len(),
            2,
            "expected exactly the two real cycles: {cycles:?}"
        );
        assert_eq!(cycles[0].len(), 2);
        assert!(
            cycles[0].contains(&"icons.rs".to_string())
                && cycles[0].contains(&"types.rs".to_string())
        );
        assert_eq!(cycles[1].len(), 4);
        assert!(
            cycles[1].contains(&"types.rs".to_string())
                && cycles[1].contains(&"ipc_client.rs".to_string())
                && cycles[1].contains(&"state.rs".to_string())
                && cycles[1].contains(&"menu.rs".to_string())
        );
        assert!(
            !cycles
                .iter()
                .any(|cycle| cycle.len() == 5 && cycle.contains(&"icons.rs".to_string())),
            "icons/types and ipc/state/menu/types cycles must not be merged"
        );
    }

    #[test]
    fn nested_cycles() {
        // a->b->c->d->a forms a single cycle
        let edges = vec![
            ("a".to_string(), "b".to_string(), "import".to_string()),
            ("b".to_string(), "c".to_string(), "import".to_string()),
            ("c".to_string(), "d".to_string(), "import".to_string()),
            ("d".to_string(), "a".to_string(), "import".to_string()),
        ];
        let cycles = find_cycles(&edges);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].len(), 4);
    }

    #[test]
    fn empty_graph() {
        let edges: Vec<(String, String, String)> = vec![];
        let cycles = find_cycles(&edges);
        assert!(cycles.is_empty());
    }

    #[test]
    fn single_node_no_self_loop() {
        let edges = vec![("a".to_string(), "b".to_string(), "import".to_string())];
        let cycles = find_cycles(&edges);
        assert!(cycles.is_empty());
    }

    #[test]
    fn print_cycles_empty() {
        // Should not panic
        print_cycles(&[], false);
    }

    #[test]
    fn print_cycles_json() {
        let cycles = vec![vec!["a".to_string(), "b".to_string()]];
        // Should not panic
        print_cycles(&cycles, true);
    }

    #[test]
    fn print_cycles_text() {
        let cycles = vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["c".to_string(), "d".to_string(), "e".to_string()],
        ];
        // Should not panic
        print_cycles(&cycles, false);
    }

    #[test]
    fn format_cycle_path_keeps_full_paths_for_ambiguous_module_names() {
        let nodes = vec![
            "app/ui/voice_chat/api/mod.rs".to_string(),
            "app/ui/voice_chat/handlers/mod.rs".to_string(),
        ];

        assert_eq!(
            super::format_cycle_path(&nodes),
            "app/ui/voice_chat/api/mod.rs ↔ app/ui/voice_chat/handlers/mod.rs"
        );
    }

    #[test]
    fn detects_cycle_with_index_collapse() {
        // Barrel (index.ts) re-export + consumer importing the barrel should still form a cycle
        // after normalizing /index and TS/JS extensions.
        let edges = vec![
            (
                "src/features/ai-suite/index.ts".to_string(),
                "src/features/ai-suite/hooks/useFoo.ts".to_string(),
                "reexport".to_string(),
            ),
            (
                "src/features/ai-suite/hooks/useFoo.tsx".to_string(),
                "src/features/ai-suite/index.tsx".to_string(),
                "import".to_string(),
            ),
        ];
        let cycles = find_cycles(&edges);
        assert_eq!(cycles.len(), 1);
        let cycle = &cycles[0];
        assert_eq!(cycle.len(), 2);
        assert!(cycle.iter().any(|p| p.contains("ai-suite/index")));
        assert!(cycle.iter().any(|p| p.contains("hooks/useFoo")));
    }

    #[test]
    fn collapses_ts_js_family_into_cycles() {
        // Cross-extension imports (ts/js) should normalize to the same module id
        // so cycles are detectable even when files are split by extension.
        let edges = vec![
            (
                "src/utils/a.ts".to_string(),
                "src/utils/b.ts".to_string(),
                "import".to_string(),
            ),
            (
                "src/utils/b.js".to_string(),
                "src/utils/a.js".to_string(),
                "import".to_string(),
            ),
        ];
        let cycles = find_cycles(&edges);
        assert_eq!(cycles.len(), 1);
        let cycle = &cycles[0];
        assert_eq!(cycle.len(), 2);
        assert!(cycle.iter().any(|p| p.contains("utils/a")));
        assert!(cycle.iter().any(|p| p.contains("utils/b")));
    }

    #[test]
    fn single_node_not_reported_as_cycle() {
        // A node with no outgoing edges should not be a cycle
        let edges = vec![("a.rs".to_string(), "b.rs".to_string(), "import".to_string())];
        let cycles = find_cycles(&edges);
        // No cycles expected - just a -> b with no return edge
        assert!(cycles.is_empty());
    }

    #[test]
    fn real_two_node_cycle_detected() {
        let edges = vec![
            ("a.rs".to_string(), "b.rs".to_string(), "import".to_string()),
            ("b.rs".to_string(), "a.rs".to_string(), "import".to_string()),
        ];
        let cycles = find_cycles(&edges);
        assert_eq!(cycles.len(), 1);
        assert!(cycles[0].len() >= 2);
    }

    #[test]
    fn single_node_not_reported_with_lazy() {
        use super::find_cycles_with_lazy;
        // Single-direction edge should not create a cycle
        let edges = vec![("a.rs".to_string(), "b.rs".to_string(), "import".to_string())];
        let (cycles, _) = find_cycles_with_lazy(&edges);
        assert!(cycles.is_empty());
    }

    #[test]
    fn real_two_node_cycle_detected_with_lazy() {
        use super::find_cycles_with_lazy;
        let edges = vec![
            ("a.rs".to_string(), "b.rs".to_string(), "import".to_string()),
            ("b.rs".to_string(), "a.rs".to_string(), "import".to_string()),
        ];
        let (cycles, _) = find_cycles_with_lazy(&edges);
        assert_eq!(cycles.len(), 1);
        assert!(cycles[0].len() >= 2);
    }

    #[test]
    fn detects_self_loop_with_lazy() {
        use super::find_cycles_with_lazy;
        // Self-loop should be detected as a cycle (single node importing itself)
        let edges = vec![("a.rs".to_string(), "a.rs".to_string(), "import".to_string())];
        let (cycles, _) = find_cycles_with_lazy(&edges);
        assert_eq!(cycles.len(), 1, "Self-loop should be detected as a cycle");
        assert_eq!(cycles[0].len(), 1, "Self-loop cycle should have 1 node");
        assert_eq!(cycles[0][0], "a.rs");
    }

    #[test]
    fn no_false_single_node_cycles_from_normalization() {
        // Test case: Same file referenced with different extensions shouldn't create false cycle
        // This happens when TS/JS normalization collapses variants
        let edges = vec![
            ("a.ts".to_string(), "b.ts".to_string(), "import".to_string()),
            (
                "a.tsx".to_string(),
                "c.ts".to_string(),
                "import".to_string(),
            ),
        ];
        let cycles = find_cycles(&edges);
        // Should find NO cycles because:
        // - a.ts and a.tsx normalize to a:ts (same node in graph)
        // - There's no cycle, just two different outgoing edges from the same normalized node
        // - This should NOT be reported as a single-node cycle
        assert!(
            cycles.is_empty(),
            "Normalization collision should not create false single-node cycles"
        );
    }

    #[test]
    fn no_single_node_cycle_without_self_loop() {
        // A node in a DAG (with outgoing edges but no self-loop) should not be reported as a cycle
        let edges = vec![
            (
                "branch.rs".to_string(),
                "commit.rs".to_string(),
                "import".to_string(),
            ),
            (
                "commit.rs".to_string(),
                "user.rs".to_string(),
                "import".to_string(),
            ),
        ];
        let cycles = find_cycles(&edges);
        // Should find NO cycles - this is a simple chain A -> B -> C
        assert!(
            cycles.is_empty(),
            "Simple chain without cycles should report no cycles"
        );
    }

    #[test]
    fn real_cycle_with_multiple_nodes() {
        // A real cycle should still be detected
        let edges = vec![
            ("a.rs".to_string(), "b.rs".to_string(), "import".to_string()),
            ("b.rs".to_string(), "c.rs".to_string(), "import".to_string()),
            ("c.rs".to_string(), "a.rs".to_string(), "import".to_string()),
        ];
        let cycles = find_cycles(&edges);
        assert_eq!(cycles.len(), 1, "Real 3-node cycle should be detected");
        assert_eq!(cycles[0].len(), 3, "Cycle should have 3 nodes");
    }

    #[test]
    fn gitbutler_like_scenario() {
        // Mimics GitButler repo structure:
        // Many files with imports but no cycles
        let edges = vec![
            (
                "crates/but-core/src/branch.rs".to_string(),
                "crates/but-core/src/types.rs".to_string(),
                "import".to_string(),
            ),
            (
                "crates/but-core/src/commit.rs".to_string(),
                "crates/but-core/src/types.rs".to_string(),
                "import".to_string(),
            ),
            (
                "crates/but-core/src/user.rs".to_string(),
                "crates/but-core/src/types.rs".to_string(),
                "import".to_string(),
            ),
        ];
        let cycles = find_cycles(&edges);
        // Should find NO cycles - these are just multiple files importing the same common module
        assert!(
            cycles.is_empty(),
            "No false single-file cycles should be reported in DAG"
        );
    }

    #[test]
    fn node_with_outgoing_edges_but_no_cycle() {
        // A node that has outgoing edges but is not part of any cycle
        let edges = vec![
            ("a.rs".to_string(), "b.rs".to_string(), "import".to_string()),
            ("a.rs".to_string(), "c.rs".to_string(), "import".to_string()),
        ];
        let cycles = find_cycles(&edges);
        // Should find NO cycles
        assert!(
            cycles.is_empty(),
            "Node with only outgoing edges (no cycle) should not be reported"
        );
    }

    #[test]
    fn test_classify_hard_bidirectional() {
        use super::{CycleClassification, find_cycles_classified};
        let edges = vec![
            ("a.rs".to_string(), "b.rs".to_string(), "import".to_string()),
            ("b.rs".to_string(), "a.rs".to_string(), "import".to_string()),
        ];
        let classified = find_cycles_classified(&edges);
        assert_eq!(classified.len(), 1);
        assert_eq!(
            classified[0].classification,
            CycleClassification::HardBidirectional
        );
        assert_eq!(classified[0].classification.severity(), 3);
    }

    #[test]
    fn test_classify_module_self_reference() {
        use super::{CycleClassification, find_cycles_classified};
        let edges = vec![
            (
                "src/mod.rs".to_string(),
                "src/helper.rs".to_string(),
                "import".to_string(),
            ),
            (
                "src/helper.rs".to_string(),
                "src/mod.rs".to_string(),
                "import".to_string(),
            ),
        ];
        let classified = find_cycles_classified(&edges);
        assert_eq!(classified.len(), 1);
        assert_eq!(
            classified[0].classification,
            CycleClassification::ModuleSelfReference
        );
        assert_eq!(classified[0].classification.severity(), 0);
    }

    #[test]
    fn test_classify_wildcard_import() {
        use super::{CycleClassification, find_cycles_classified};
        let edges = vec![
            (
                "a.rs".to_string(),
                "b.rs".to_string(),
                "wildcard".to_string(),
            ),
            ("b.rs".to_string(), "a.rs".to_string(), "import".to_string()),
        ];
        let classified = find_cycles_classified(&edges);
        assert_eq!(classified.len(), 1);
        assert_eq!(
            classified[0].classification,
            CycleClassification::WildcardImport
        );
        assert_eq!(classified[0].classification.severity(), 2);
        assert!(classified[0].has_wildcard);
    }

    #[test]
    fn test_classify_cfg_gated() {
        use super::{CycleClassification, find_cycles_classified};
        let edges = vec![
            ("a.rs".to_string(), "b.rs".to_string(), "cfg".to_string()),
            ("b.rs".to_string(), "a.rs".to_string(), "import".to_string()),
        ];
        let classified = find_cycles_classified(&edges);
        assert_eq!(classified.len(), 1);
        assert_eq!(classified[0].classification, CycleClassification::CfgGated);
        assert_eq!(classified[0].classification.severity(), 1);
        assert!(classified[0].is_cfg_gated);
    }

    #[test]
    fn is_fixture_cycle_flags_test_fixture_paths() {
        use super::is_fixture_cycle;
        // Regression for loctree-feedback 2026-06-03: the intentional
        // tests/fixtures/circular_imports fixture was counted as a real cycle.
        let nodes = vec![
            "loctree-rs/tests/fixtures/circular_imports/src/c.ts".to_string(),
            "loctree-rs/tests/fixtures/circular_imports/src/b.ts".to_string(),
            "loctree-rs/tests/fixtures/circular_imports/src/a.ts".to_string(),
        ];
        assert!(
            is_fixture_cycle(&nodes),
            "all-fixture cycle must carry fixture provenance"
        );
    }

    #[test]
    fn is_fixture_cycle_ignores_production_cycle() {
        use super::is_fixture_cycle;
        let nodes = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];
        assert!(
            !is_fixture_cycle(&nodes),
            "production cycle must not be flagged as fixture"
        );
    }

    #[test]
    fn is_fixture_cycle_requires_all_members_non_production() {
        use super::is_fixture_cycle;
        // A real cycle that merely touches a fixture file is still a regression.
        let nodes = vec![
            "tests/fixtures/circular/a.ts".to_string(),
            "src/real/b.ts".to_string(),
        ];
        assert!(
            !is_fixture_cycle(&nodes),
            "mixed fixture+production cycle must stay reported"
        );
    }

    #[test]
    fn is_fixture_cycle_handles_empty() {
        use super::is_fixture_cycle;
        assert!(!is_fixture_cycle(&[]));
    }

    #[test]
    fn classified_cycle_carries_fixture_provenance() {
        use super::find_cycles_classified;
        // The exact false-positive shape from loctree-feedback L479-L499:
        // a.ts -> b.ts -> c.ts -> a.ts under tests/fixtures/circular_imports.
        let base = "loctree-rs/tests/fixtures/circular_imports/src";
        let edges = vec![
            (
                format!("{base}/a.ts"),
                format!("{base}/b.ts"),
                "import".to_string(),
            ),
            (
                format!("{base}/b.ts"),
                format!("{base}/c.ts"),
                "import".to_string(),
            ),
            (
                format!("{base}/c.ts"),
                format!("{base}/a.ts"),
                "import".to_string(),
            ),
        ];
        let classified = find_cycles_classified(&edges);
        assert_eq!(classified.len(), 1);
        assert!(
            classified[0].fixture,
            "fixture cycle must be labeled fixture in the classified finding surface"
        );

        // A production cycle keeps fixture = false.
        let prod_edges = vec![
            (
                "src/a.rs".to_string(),
                "src/b.rs".to_string(),
                "import".to_string(),
            ),
            (
                "src/b.rs".to_string(),
                "src/a.rs".to_string(),
                "import".to_string(),
            ),
        ];
        let prod = find_cycles_classified(&prod_edges);
        assert_eq!(prod.len(), 1);
        assert!(
            !prod[0].fixture,
            "production cycle must not be flagged fixture"
        );
    }

    #[test]
    fn test_severity_levels() {
        use super::CycleClassification;
        assert_eq!(CycleClassification::HardBidirectional.severity(), 3);
        assert_eq!(CycleClassification::WildcardImport.severity(), 2);
        assert_eq!(CycleClassification::Unknown.severity(), 2);
        assert_eq!(CycleClassification::CfgGated.severity(), 1);
        assert_eq!(CycleClassification::ModuleSelfReference.severity(), 0);
        assert_eq!(CycleClassification::TraitBased.severity(), 0);
        assert_eq!(CycleClassification::FanPattern.severity(), 0);

        assert_eq!(
            CycleClassification::HardBidirectional.severity_label(),
            "high"
        );
        assert_eq!(
            CycleClassification::WildcardImport.severity_label(),
            "medium"
        );
        assert_eq!(CycleClassification::CfgGated.severity_label(), "low");
        assert_eq!(
            CycleClassification::ModuleSelfReference.severity_label(),
            "info"
        );
    }
}
