//! Directory-level holographic focus - Extract context for AI agents
//!
//! Like `slicer.rs` but operates on directories instead of single files.
//! Extracts a 3-layer context:
//! - Core: All files within the target directory
//! - Deps: External files imported by core (outside the directory)
//! - Consumers: Files outside the directory that import core files
//!
//! Additionally tracks internal edges (imports within the directory).

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::slicer::{
    CoreSymbol, SliceFile, SuggestedNext, authority_labels, collect_core_symbols,
    suggested_next_for_symbols,
};
use crate::snapshot::Snapshot;

/// Configuration for focus operation
#[derive(Debug, Clone)]
pub struct FocusConfig {
    /// Include consumer layer (files that import core)
    pub include_consumers: bool,
    /// Maximum depth for external dependency traversal (default: 2)
    pub max_depth: usize,
}

impl Default for FocusConfig {
    fn default() -> Self {
        Self {
            include_consumers: true,
            max_depth: 2,
        }
    }
}

/// Statistics about the focus result
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FocusStats {
    /// Number of files in core (within directory)
    pub core_files: usize,
    /// Total LOC in core layer
    pub core_loc: usize,
    /// Number of internal import edges (within directory)
    pub internal_edges: usize,
    /// Number of external dependency files
    pub deps_files: usize,
    /// Total LOC in deps layer
    pub deps_loc: usize,
    /// Number of consumer files
    pub consumers_files: usize,
    /// Total LOC in consumers layer
    pub consumers_loc: usize,
    /// Total files across all layers
    pub total_files: usize,
    /// Total LOC across all layers
    pub total_loc: usize,
}

/// The complete focus result for a directory
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HolographicFocus {
    /// Target directory that was focused
    pub target: String,
    /// Core layer files (all files within directory)
    pub core: Vec<SliceFile>,
    /// External dependencies layer files
    pub deps: Vec<SliceFile>,
    /// Consumer layer files (who imports from this directory)
    pub consumers: Vec<SliceFile>,
    /// Symbols defined by files in the target directory.
    #[serde(default)]
    pub core_symbols: Vec<CoreSymbol>,
    /// Provenance labels represented in this response.
    #[serde(default)]
    pub authority_labels: Vec<String>,
    /// Concrete next Loctree commands an agent can run from this focus.
    #[serde(default)]
    pub suggested_next: Vec<SuggestedNext>,
    /// Command bridges involving files in the directory
    pub command_bridges: Vec<String>,
    /// Event bridges involving files in the directory
    pub event_bridges: Vec<String>,
    /// Statistics
    pub stats: FocusStats,
}

/// Strip common extensions from a path for matching
fn strip_extension(path: &str) -> &str {
    const EXTENSIONS: &[&str] = &[
        ".tsx", ".ts", ".jsx", ".js", ".mjs", ".cjs", ".rs", ".py", ".css", ".scss", ".sass",
    ];
    for ext in EXTENSIONS {
        if let Some(stripped) = path.strip_suffix(ext) {
            return stripped;
        }
    }
    path
}

/// Normalize a directory path for matching
fn normalize_directory(path: &str) -> String {
    let normalized = path.trim_start_matches("./").replace('\\', "/");
    // Ensure no trailing slash for consistent matching
    normalized.trim_end_matches('/').to_string()
}

/// Check if a file path is within a directory
fn is_in_directory(file_path: &str, dir_path: &str) -> bool {
    let norm_file = file_path.trim_start_matches("./").replace('\\', "/");
    let norm_dir = normalize_directory(dir_path);

    // Either exact match (unlikely for dir vs file) or starts with dir/
    norm_file == norm_dir || norm_file.starts_with(&format!("{}/", norm_dir))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

impl HolographicFocus {
    /// Create a focus from a directory path using snapshot data
    pub fn from_path(snapshot: &Snapshot, target_dir: &str, config: &FocusConfig) -> Option<Self> {
        let normalized_target = normalize_directory(target_dir);

        // Build adjacency maps from snapshot edges
        let mut imports: HashMap<String, Vec<String>> = HashMap::new();
        let mut imported_by: HashMap<String, Vec<String>> = HashMap::new();

        for edge in &snapshot.edges {
            imports
                .entry(edge.from.clone())
                .or_default()
                .push(edge.to.clone());
            imported_by
                .entry(edge.to.clone())
                .or_default()
                .push(edge.from.clone());

            // Also store with stripped extension key for matching
            let from_stripped = strip_extension(&edge.from);
            let to_stripped = strip_extension(&edge.to);
            if from_stripped != edge.from {
                imports
                    .entry(from_stripped.to_string())
                    .or_default()
                    .push(edge.to.clone());
            }
            if to_stripped != edge.to {
                imported_by
                    .entry(to_stripped.to_string())
                    .or_default()
                    .push(edge.from.clone());
            }
        }

        // Layer 1: Core - all files within the target directory
        let mut core: Vec<SliceFile> = Vec::new();
        let mut core_paths: HashSet<String> = HashSet::new();

        for file in &snapshot.files {
            if is_in_directory(&file.path, &normalized_target) {
                core.push(SliceFile::from_analysis(file, "core", 0));
                core_paths.insert(file.path.clone());
                core_paths.insert(strip_extension(&file.path).to_string());
            }
        }

        // If no files found in directory, return None
        if core.is_empty() {
            return None;
        }

        // Count internal edges (both from and to are in core)
        let mut internal_edges = 0;
        for edge in &snapshot.edges {
            if is_in_directory(&edge.from, &normalized_target)
                && is_in_directory(&edge.to, &normalized_target)
            {
                internal_edges += 1;
            }
        }

        // Layer 2: Deps - external files imported by core (BFS)
        let mut deps: Vec<SliceFile> = Vec::new();
        let mut visited: HashSet<String> = core_paths.clone();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();

        // Start with direct imports from core files that are NOT in core
        for core_file in &core {
            let core_stripped = strip_extension(&core_file.path).to_string();

            let direct_deps: Vec<String> = imports
                .get(&core_file.path)
                .into_iter()
                .chain(imports.get(&core_stripped))
                .flatten()
                .cloned()
                .collect();

            for dep in direct_deps {
                if !is_in_directory(&dep, &normalized_target) {
                    let dep_stripped = strip_extension(&dep).to_string();
                    if !visited.contains(&dep) && !visited.contains(&dep_stripped) {
                        queue.push_back((dep.clone(), 1));
                        visited.insert(dep);
                        visited.insert(dep_stripped);
                    }
                }
            }
        }

        while let Some((path, depth)) = queue.pop_front() {
            if depth > config.max_depth {
                continue;
            }

            // Find matching file in snapshot
            let file = snapshot
                .files
                .iter()
                .find(|f| f.path == path || strip_extension(&f.path) == path);

            if let Some(file) = file {
                deps.push(SliceFile::from_analysis(file, "deps", depth));
            }

            // Go deeper for transitive deps
            if depth < config.max_depth {
                let path_stripped = strip_extension(&path).to_string();
                let transitive: Vec<String> = imports
                    .get(&path)
                    .into_iter()
                    .chain(imports.get(&path_stripped))
                    .flatten()
                    .cloned()
                    .collect();

                for dep in transitive {
                    let dep_stripped = strip_extension(&dep).to_string();
                    if !visited.contains(&dep) && !visited.contains(&dep_stripped) {
                        queue.push_back((dep.clone(), depth + 1));
                        visited.insert(dep);
                        visited.insert(dep_stripped);
                    }
                }
            }
        }

        // Layer 3: Consumers - external files that import core files
        let mut consumers: Vec<SliceFile> = Vec::new();
        if config.include_consumers {
            let mut consumer_paths: HashSet<String> = HashSet::new();

            for core_file in &core {
                let core_stripped = strip_extension(&core_file.path).to_string();

                let direct_consumers: Vec<String> = imported_by
                    .get(&core_file.path)
                    .into_iter()
                    .chain(imported_by.get(&core_stripped))
                    .flatten()
                    .cloned()
                    .collect();

                for consumer in direct_consumers {
                    // Only include if NOT in core directory
                    if !is_in_directory(&consumer, &normalized_target) {
                        consumer_paths.insert(consumer);
                    }
                }
            }

            // Convert consumer paths to SliceFile objects
            for consumer_path in consumer_paths {
                let file = snapshot
                    .files
                    .iter()
                    .find(|f| f.path == consumer_path || strip_extension(&f.path) == consumer_path);

                if let Some(file) = file {
                    consumers.push(SliceFile::from_analysis(file, "consumers", 1));
                }
            }
        }

        // Collect command bridges involving files in the directory
        let mut command_bridges: Vec<String> = Vec::new();
        for bridge in &snapshot.command_bridges {
            let involves_core = bridge
                .frontend_calls
                .iter()
                .any(|(f, _)| is_in_directory(f, &normalized_target))
                || bridge
                    .backend_handler
                    .as_ref()
                    .map(|(f, _)| is_in_directory(f, &normalized_target))
                    .unwrap_or(false);
            if involves_core {
                command_bridges.push(bridge.name.clone());
            }
        }

        // Collect event bridges involving files in the directory
        let mut event_bridges: Vec<String> = Vec::new();
        for bridge in &snapshot.event_bridges {
            let involves_core = bridge
                .emits
                .iter()
                .any(|(f, _, _)| is_in_directory(f, &normalized_target))
                || bridge
                    .listens
                    .iter()
                    .any(|(f, _)| is_in_directory(f, &normalized_target));
            if involves_core {
                event_bridges.push(bridge.name.clone());
            }
        }

        // Calculate stats
        let core_loc: usize = core.iter().map(|f| f.loc).sum();
        let deps_loc: usize = deps.iter().map(|f| f.loc).sum();
        let consumers_loc: usize = consumers.iter().map(|f| f.loc).sum();

        let stats = FocusStats {
            core_files: core.len(),
            core_loc,
            internal_edges,
            deps_files: deps.len(),
            deps_loc,
            consumers_files: consumers.len(),
            consumers_loc,
            total_files: core.len() + deps.len() + consumers.len(),
            total_loc: core_loc + deps_loc + consumers_loc,
        };

        // Sort for consistent output
        core.sort_by(|a, b| a.path.cmp(&b.path));
        deps.sort_by(|a, b| a.depth.cmp(&b.depth).then(a.path.cmp(&b.path)));
        consumers.sort_by(|a, b| a.path.cmp(&b.path));

        let mut core_symbols: Vec<CoreSymbol> = snapshot
            .files
            .iter()
            .filter(|file| is_in_directory(&file.path, &normalized_target))
            .flat_map(collect_core_symbols)
            .collect();
        core_symbols.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then(a.line.cmp(&b.line))
                .then(a.name.cmp(&b.name))
        });
        let suggested_next = suggested_next_for_symbols(
            format!("loct focus {}", shell_quote(&normalized_target)),
            &core_symbols,
        );

        Some(Self {
            target: normalized_target,
            core,
            deps,
            consumers,
            core_symbols,
            authority_labels: authority_labels(false),
            suggested_next,
            command_bridges,
            event_bridges,
            stats,
        })
    }

    /// Print focus in human-readable format
    pub fn print(&self) {
        println!("Focus: {}/", self.target);
        println!();

        println!(
            "Core ({} files, {} LOC):",
            self.stats.core_files, self.stats.core_loc
        );

        const DISPLAY_LIMIT: usize = 25;

        for (i, f) in self.core.iter().enumerate() {
            if i >= DISPLAY_LIMIT {
                println!(
                    "  ... and {} more (use --json for full list)",
                    self.core.len() - DISPLAY_LIMIT
                );
                break;
            }
            println!("  {} ({} LOC, {})", f.path, f.loc, f.descriptor());
        }

        println!();
        println!(
            "Internal edges: {} imports within directory",
            self.stats.internal_edges
        );

        if !self.core_symbols.is_empty() {
            println!("\nCore symbols ({}):", self.core_symbols.len());
            for symbol in self.core_symbols.iter().take(DISPLAY_LIMIT) {
                let line = symbol
                    .line
                    .map(|line| line.to_string())
                    .unwrap_or_else(|| "?".to_string());
                println!(
                    "  {}:{} {} {} [{}]",
                    symbol.file, line, symbol.kind, symbol.name, symbol.authority
                );
            }
            if self.core_symbols.len() > DISPLAY_LIMIT {
                println!(
                    "  ... and {} more (use --json for full list)",
                    self.core_symbols.len() - DISPLAY_LIMIT
                );
            }
        }

        if !self.deps.is_empty() {
            println!(
                "\nExternal Deps ({} files, {} LOC):",
                self.stats.deps_files, self.stats.deps_loc
            );

            for (i, f) in self.deps.iter().enumerate() {
                if i >= DISPLAY_LIMIT {
                    println!(
                        "  ... and {} more (use --json for full list)",
                        self.deps.len() - DISPLAY_LIMIT
                    );
                    break;
                }
                let indent = "  ".repeat(f.depth.min(3));
                println!(
                    "{}[d{}] {} ({} LOC, {})",
                    indent,
                    f.depth,
                    f.path,
                    f.loc,
                    f.descriptor()
                );
            }
        }

        if !self.consumers.is_empty() {
            println!(
                "\nConsumers ({} files, {} LOC):",
                self.stats.consumers_files, self.stats.consumers_loc
            );

            for (i, f) in self.consumers.iter().enumerate() {
                if i >= DISPLAY_LIMIT {
                    println!(
                        "  ... and {} more (use --json for full list)",
                        self.consumers.len() - DISPLAY_LIMIT
                    );
                    break;
                }
                println!("  {} ({} LOC, {})", f.path, f.loc, f.descriptor());
            }
        }

        if !self.command_bridges.is_empty() {
            println!("\nCommand bridges: {}", self.command_bridges.join(", "));
        }

        if !self.event_bridges.is_empty() {
            println!("Event bridges: {}", self.event_bridges.join(", "));
        }

        if !self.suggested_next.is_empty() {
            println!("\nsuggested next:");
            for step in &self.suggested_next {
                println!("  {}  # {}", step.command, step.reason);
            }
        }

        println!(
            "\nTotal: {} files, {} LOC ({} internal edges)",
            self.stats.total_files, self.stats.total_loc, self.stats.internal_edges
        );
    }

    /// Output as JSON
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "target": self.target,
            "core": self.core,
            "deps": self.deps,
            "consumers": self.consumers,
            "coreSymbols": self.core_symbols,
            "authorityLabels": self.authority_labels,
            "suggestedNext": self.suggested_next,
            "commandBridges": self.command_bridges,
            "eventBridges": self.event_bridges,
            "stats": self.stats,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{GraphEdge, Snapshot, SnapshotMetadata};
    use crate::types::FileAnalysis;

    fn create_test_snapshot() -> Snapshot {
        Snapshot {
            metadata: SnapshotMetadata {
                schema_version: crate::snapshot::SNAPSHOT_SCHEMA_VERSION.to_string(),
                generated_at: "2025-01-01T00:00:00Z".to_string(),
                roots: vec!["/test".to_string()],
                languages: ["typescript".to_string()].into_iter().collect(),
                file_count: 6,
                total_loc: 600,
                scan_duration_ms: 100,
                resolver_config: None,
                manifest_summary: Vec::new(),
                entrypoints: Vec::new(),
                entrypoint_drift: crate::snapshot::EntrypointDriftSummary::default(),
                git_repo: None,
                git_owner_repo: None,
                git_branch: None,
                git_commit: None,
                git_scan_id: None,
            },
            files: vec![
                // Files in src/features/patients/
                FileAnalysis {
                    path: "src/features/patients/index.ts".to_string(),
                    loc: 20,
                    language: "typescript".to_string(),
                    ..FileAnalysis::new("src/features/patients/index.ts".to_string())
                },
                FileAnalysis {
                    path: "src/features/patients/PatientsList.tsx".to_string(),
                    loc: 150,
                    language: "typescript".to_string(),
                    ..FileAnalysis::new("src/features/patients/PatientsList.tsx".to_string())
                },
                FileAnalysis {
                    path: "src/features/patients/usePatient.ts".to_string(),
                    loc: 80,
                    language: "typescript".to_string(),
                    ..FileAnalysis::new("src/features/patients/usePatient.ts".to_string())
                },
                // External files
                FileAnalysis {
                    path: "src/components/Button.tsx".to_string(),
                    loc: 100,
                    language: "typescript".to_string(),
                    ..FileAnalysis::new("src/components/Button.tsx".to_string())
                },
                FileAnalysis {
                    path: "src/App.tsx".to_string(),
                    loc: 200,
                    language: "typescript".to_string(),
                    ..FileAnalysis::new("src/App.tsx".to_string())
                },
                FileAnalysis {
                    path: "src/utils/api.ts".to_string(),
                    loc: 50,
                    language: "typescript".to_string(),
                    ..FileAnalysis::new("src/utils/api.ts".to_string())
                },
            ],
            edges: vec![
                // Internal edges (within patients/)
                GraphEdge {
                    from: "src/features/patients/index.ts".to_string(),
                    to: "src/features/patients/PatientsList.tsx".to_string(),
                    label: "reexport".to_string(),
                },
                GraphEdge {
                    from: "src/features/patients/PatientsList.tsx".to_string(),
                    to: "src/features/patients/usePatient.ts".to_string(),
                    label: "import".to_string(),
                },
                // External deps (patients imports external)
                GraphEdge {
                    from: "src/features/patients/PatientsList.tsx".to_string(),
                    to: "src/components/Button.tsx".to_string(),
                    label: "import".to_string(),
                },
                GraphEdge {
                    from: "src/features/patients/usePatient.ts".to_string(),
                    to: "src/utils/api.ts".to_string(),
                    label: "import".to_string(),
                },
                // Consumer (App imports patients)
                GraphEdge {
                    from: "src/App.tsx".to_string(),
                    to: "src/features/patients/index.ts".to_string(),
                    label: "import".to_string(),
                },
            ],
            export_index: Default::default(),
            command_bridges: vec![],
            event_bridges: vec![],
            barrels: vec![],
            semantic_facts: None,
            symbol_graph: None,
        }
    }

    #[test]
    fn test_focus_finds_core_files() {
        let snapshot = create_test_snapshot();
        let config = FocusConfig::default();

        let focus = HolographicFocus::from_path(&snapshot, "src/features/patients", &config)
            .expect("focus on patients");

        assert_eq!(focus.target, "src/features/patients");
        assert_eq!(focus.stats.core_files, 3);
        assert_eq!(focus.stats.core_loc, 250); // 20 + 150 + 80
    }

    #[test]
    fn test_focus_counts_internal_edges() {
        let snapshot = create_test_snapshot();
        let config = FocusConfig::default();

        let focus = HolographicFocus::from_path(&snapshot, "src/features/patients", &config)
            .expect("focus on patients");

        // Two internal edges: index->PatientsList, PatientsList->usePatient
        assert_eq!(focus.stats.internal_edges, 2);
    }

    #[test]
    fn test_focus_finds_external_deps() {
        let snapshot = create_test_snapshot();
        let config = FocusConfig::default();

        let focus = HolographicFocus::from_path(&snapshot, "src/features/patients", &config)
            .expect("focus on patients");

        // PatientsList imports Button, usePatient imports api
        assert_eq!(focus.stats.deps_files, 2);
        let dep_paths: Vec<_> = focus.deps.iter().map(|f| f.path.as_str()).collect();
        assert!(dep_paths.contains(&"src/components/Button.tsx"));
        assert!(dep_paths.contains(&"src/utils/api.ts"));
    }

    #[test]
    fn focus_default_includes_consumers_and_core_symbols() {
        let mut snapshot = create_test_snapshot();
        let index = snapshot
            .files
            .iter_mut()
            .find(|file| file.path == "src/features/patients/index.ts")
            .expect("fixture index.ts");
        index.exports.push(crate::types::ExportSymbol {
            name: "PatientsModule".to_string(),
            kind: "function".to_string(),
            export_type: "named".to_string(),
            line: Some(3),
            params: Vec::new(),
            symbol_id: Default::default(),
        });

        let focus = HolographicFocus::from_path(
            &snapshot,
            "src/features/patients",
            &FocusConfig::default(),
        )
        .expect("focus on patients");
        let json = focus.to_json();

        assert_eq!(
            focus.stats.consumers_files, 1,
            "focus should include external module consumers by default"
        );
        assert_eq!(focus.consumers[0].path, "src/App.tsx");
        assert!(
            json["coreSymbols"]
                .as_array()
                .expect("coreSymbols")
                .iter()
                .any(|symbol| {
                    symbol["name"] == "PatientsModule"
                        && symbol["file"] == "src/features/patients/index.ts"
                        && symbol["line"] == 3
                        && symbol["authority"] == "LoctreeDerived"
                }),
            "focus should expose core symbols for files in the directory: {json}"
        );
        assert!(
            json["suggestedNext"]
                .as_array()
                .expect("suggestedNext")
                .iter()
                .any(|step| step["command"] == "loct occurrences 'PatientsModule' --json"),
            "focus should carry non-empty suggested next steps: {json}"
        );
    }

    #[test]
    fn test_focus_finds_consumers() {
        let snapshot = create_test_snapshot();
        let config = FocusConfig {
            include_consumers: true,
            ..Default::default()
        };

        let focus = HolographicFocus::from_path(&snapshot, "src/features/patients", &config)
            .expect("focus on patients with consumers");

        // App.tsx imports from patients
        assert_eq!(focus.stats.consumers_files, 1);
        assert_eq!(focus.consumers[0].path, "src/App.tsx");
    }

    #[test]
    fn test_focus_not_found() {
        let snapshot = create_test_snapshot();
        let config = FocusConfig::default();

        let focus = HolographicFocus::from_path(&snapshot, "src/nonexistent", &config);
        assert!(focus.is_none());
    }

    #[test]
    fn test_focus_normalizes_paths() {
        let snapshot = create_test_snapshot();
        let config = FocusConfig::default();

        // All these should work the same
        let f1 = HolographicFocus::from_path(&snapshot, "src/features/patients", &config);
        let f2 = HolographicFocus::from_path(&snapshot, "src/features/patients/", &config);
        let f3 = HolographicFocus::from_path(&snapshot, "./src/features/patients", &config);

        assert!(f1.is_some());
        assert!(f2.is_some());
        assert!(f3.is_some());

        assert_eq!(f1.unwrap().stats.core_files, f2.unwrap().stats.core_files);
    }

    #[test]
    fn test_is_in_directory() {
        assert!(is_in_directory("src/foo/bar.ts", "src/foo"));
        assert!(is_in_directory("src/foo/sub/bar.ts", "src/foo"));
        assert!(!is_in_directory("src/foobar/baz.ts", "src/foo"));
        assert!(!is_in_directory("src/other/bar.ts", "src/foo"));
        assert!(is_in_directory("./src/foo/bar.ts", "src/foo"));
        assert!(is_in_directory("src/foo/bar.ts", "./src/foo"));
    }
}
