//! Barrel file chaos detection for the twins analysis system.
//!
//! Detects three types of barrel-related issues:
//! 1. **Missing barrels** - directories with multiple files imported externally but no index.ts
//! 2. **Deep re-export chains** - index.ts → sub/index.ts → sub/sub/index.ts (depth > 2)
//! 3. **Inconsistent import paths** - same symbol imported via different paths

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::snapshot::Snapshot;

/// Complete barrel chaos analysis results
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BarrelAnalysis {
    /// Directories missing barrel files
    pub missing_barrels: Vec<MissingBarrel>,
    /// Deep re-export chains
    pub deep_chains: Vec<ReexportChain>,
    /// Inconsistent import paths for same symbols
    pub inconsistent_paths: Vec<InconsistentImport>,
}

/// A directory that should have a barrel file but doesn't
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MissingBarrel {
    /// Directory path
    pub directory: String,
    /// Number of files in this directory
    pub file_count: usize,
    /// Number of imports from outside this directory
    pub external_import_count: usize,
}

/// A re-export chain that's too deep
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReexportChain {
    /// Symbol being re-exported
    pub symbol: String,
    /// Chain of files from final consumer to original definition
    /// Example: ["index.ts", "sub/index.ts", "sub/types.ts"]
    pub chain: Vec<String>,
    /// Depth of the chain
    pub depth: usize,
}

/// A symbol imported via multiple inconsistent paths
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InconsistentImport {
    /// Symbol name
    pub symbol: String,
    /// Canonical (most-used) path
    pub canonical_path: String,
    /// Alternative paths and their usage counts
    pub alternative_paths: Vec<(String, usize)>,
}

/// Analyze barrel chaos in the snapshot
pub fn analyze_barrel_chaos(snapshot: &Snapshot) -> BarrelAnalysis {
    // Skip barrel analysis for pure Rust projects
    // Barrel pattern (index.ts/index.js) is specific to TypeScript/JavaScript
    if is_pure_rust_project(snapshot) {
        return BarrelAnalysis {
            missing_barrels: Vec::new(),
            deep_chains: Vec::new(),
            inconsistent_paths: Vec::new(),
        };
    }

    let missing_barrels = detect_missing_barrels(snapshot);
    let deep_chains = detect_deep_chains(snapshot);
    let inconsistent_paths = detect_inconsistent_paths(snapshot);

    BarrelAnalysis {
        missing_barrels,
        deep_chains,
        inconsistent_paths,
    }
}

/// Check if this is a pure Rust project (no TS/JS files)
fn is_pure_rust_project(snapshot: &Snapshot) -> bool {
    // Check if we have any TypeScript/JavaScript files in the snapshot
    let has_ts_js = snapshot.files.iter().any(|file| {
        let path = file.path.to_lowercase();
        path.ends_with(".ts")
            || path.ends_with(".tsx")
            || path.ends_with(".js")
            || path.ends_with(".jsx")
            || path.ends_with(".mjs")
            || path.ends_with(".cjs")
    });

    // If no TS/JS files found, this is likely a pure Rust project
    !has_ts_js
}

/// Detect directories missing barrel files
///
/// A directory needs a barrel if:
/// 1. It has no index.ts/index.js
/// 2. It has multiple files (> 1)
/// 3. Files in this dir are imported from outside (threshold: 3+ external imports)
fn detect_missing_barrels(snapshot: &Snapshot) -> Vec<MissingBarrel> {
    // Group files by directory
    let mut dir_files: HashMap<String, Vec<String>> = HashMap::new();
    for file in &snapshot.files {
        if let Some(dir) = get_directory(&file.path) {
            dir_files.entry(dir).or_default().push(file.path.clone());
        }
    }

    // Build import map: file -> set of files that import it
    let mut importers: HashMap<String, HashSet<String>> = HashMap::new();
    for edge in &snapshot.edges {
        importers
            .entry(edge.to.clone())
            .or_default()
            .insert(edge.from.clone());
    }

    let mut missing = Vec::new();

    for (dir, files) in &dir_files {
        // Skip if directory has index file
        if has_index_file(files) {
            continue;
        }

        // Skip if only one file in directory
        if files.len() <= 1 {
            continue;
        }

        // Count external imports (imports from outside this directory)
        let mut external_import_count = 0;
        for file in files {
            if let Some(file_importers) = importers.get(file) {
                for importer in file_importers {
                    // Check if importer is outside this directory
                    if let Some(importer_dir) = get_directory(importer)
                        && importer_dir != *dir
                    {
                        external_import_count += 1;
                    }
                }
            }
        }

        // Flag if external imports exceed threshold
        const EXTERNAL_IMPORT_THRESHOLD: usize = 3;
        if external_import_count >= EXTERNAL_IMPORT_THRESHOLD {
            missing.push(MissingBarrel {
                directory: dir.clone(),
                file_count: files.len(),
                external_import_count,
            });
        }
    }

    // Sort by external import count descending
    missing.sort_by_key(|b| std::cmp::Reverse(b.external_import_count));
    missing
}

/// Detect deep re-export chains
///
/// Traces re-exports to find chains deeper than threshold (2).
/// Uses snapshot edges to build re-export graph.
fn detect_deep_chains(snapshot: &Snapshot) -> Vec<ReexportChain> {
    // Build re-export graph from edges
    // We consider an edge a re-export if it's from an index.* file
    let mut reexport_graph: HashMap<String, Vec<String>> = HashMap::new();

    for edge in &snapshot.edges {
        if is_barrel_file(&edge.from) {
            reexport_graph
                .entry(edge.from.clone())
                .or_default()
                .push(edge.to.clone());
        }
    }

    // Find export index: symbol -> original file
    let _symbol_origins: HashMap<&str, &str> = snapshot
        .export_index
        .iter()
        .flat_map(|(symbol, files)| {
            files
                .iter()
                .map(move |file| (symbol.as_str(), file.as_str()))
        })
        .collect();

    let mut deep_chains = Vec::new();

    // For each barrel file, trace re-export chains
    for (barrel, targets) in &reexport_graph {
        for target in targets {
            let chain = trace_reexport_chain(barrel, target, &reexport_graph);

            if chain.len() > 2 {
                // Extract symbols from final target file
                if let Some(file_analysis) = snapshot.files.iter().find(|f| &f.path == target) {
                    for export in &file_analysis.exports {
                        deep_chains.push(ReexportChain {
                            symbol: export.name.clone(),
                            chain: chain.clone(),
                            depth: chain.len() - 1,
                        });
                    }
                }
            }
        }
    }

    // Deduplicate and sort by depth
    deep_chains.sort_by_key(|b| std::cmp::Reverse(b.depth));
    deep_chains
}

/// Trace a re-export chain from start to final target
fn trace_reexport_chain(
    start: &str,
    target: &str,
    reexport_graph: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut chain = vec![start.to_string()];
    let mut current = target;
    let mut visited = HashSet::new();

    // Follow re-export chain until we hit a non-barrel file or cycle
    while is_barrel_file(current) && !visited.contains(current) {
        visited.insert(current.to_string());
        chain.push(current.to_string());

        if let Some(next_targets) = reexport_graph.get(current) {
            if let Some(next) = next_targets.first() {
                current = next;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    // Add final target if not a barrel
    if !is_barrel_file(current) && current != chain.last().unwrap() {
        chain.push(current.to_string());
    }

    chain
}

/// Detect symbols imported via inconsistent paths
///
/// Groups imports by symbol name, finds symbols imported from multiple paths,
/// marks most-used path as canonical.
fn detect_inconsistent_paths(snapshot: &Snapshot) -> Vec<InconsistentImport> {
    // Map: symbol -> map of (source_path -> import_count)
    let mut symbol_sources: HashMap<String, HashMap<String, usize>> = HashMap::new();

    for file in &snapshot.files {
        for import in &file.imports {
            for symbol in &import.symbols {
                let symbol_name = symbol.alias.as_ref().unwrap_or(&symbol.name);

                symbol_sources
                    .entry(symbol_name.clone())
                    .or_default()
                    .entry(import.source.clone())
                    .and_modify(|count| *count += 1)
                    .or_insert(1);
            }
        }
    }

    let mut inconsistent = Vec::new();

    for (symbol, sources) in symbol_sources {
        // Skip if symbol only imported from one path
        if sources.len() <= 1 {
            continue;
        }

        // Find canonical (most-used) path
        let mut sources_vec: Vec<_> = sources.into_iter().collect();
        sources_vec.sort_by_key(|b| std::cmp::Reverse(b.1));

        if let Some((canonical, _canonical_count)) = sources_vec.first() {
            let alternative_paths: Vec<_> = sources_vec
                .iter()
                .skip(1)
                .map(|(path, count)| (path.clone(), *count))
                .collect();

            // Only flag if there are significant alternatives (not just 1 usage)
            if alternative_paths.iter().any(|(_, count)| *count > 1) {
                inconsistent.push(InconsistentImport {
                    symbol: symbol.clone(),
                    canonical_path: canonical.clone(),
                    alternative_paths,
                });
            }
        }
    }

    // Sort by number of alternative paths
    inconsistent.sort_by_key(|b| std::cmp::Reverse(b.alternative_paths.len()));
    inconsistent
}

/// Get directory path from file path
fn get_directory(path: &str) -> Option<String> {
    Path::new(path)
        .parent()
        .and_then(|p| p.to_str())
        .map(|s| s.to_string())
}

/// Check if a file list contains an index file
fn has_index_file(files: &[String]) -> bool {
    files.iter().any(|f| {
        let filename = Path::new(f)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        filename.starts_with("index.")
    })
}

/// Check if a file is a barrel file (index.ts, index.js, etc.)
fn is_barrel_file(path: &str) -> bool {
    if let Some(filename) = Path::new(path).file_name().and_then(|n| n.to_str()) {
        filename.starts_with("index.")
    } else {
        false
    }
}

/// Format barrel analysis for display
pub fn format_barrel_analysis(analysis: &BarrelAnalysis) -> String {
    let mut output = String::new();
    output.push_str("📦 BARREL CHAOS\n\n");

    // Missing barrels section
    if !analysis.missing_barrels.is_empty() {
        output.push_str(&format!(
            "   Missing index.ts ({} directories):\n",
            analysis.missing_barrels.len()
        ));

        for barrel in analysis.missing_barrels.iter().take(10) {
            let suggestion = if barrel.file_count <= 2 {
                "maybe inline?"
            } else {
                "create index.ts"
            };

            output.push_str(&format!(
                "   ├─ {:<30} {} files, {} external imports → {}\n",
                format!("{}/", barrel.directory),
                barrel.file_count,
                barrel.external_import_count,
                suggestion
            ));
        }

        if analysis.missing_barrels.len() > 10 {
            output.push_str(&format!(
                "   └─ ... and {} more\n",
                analysis.missing_barrels.len() - 10
            ));
        }
        output.push('\n');
    }

    // Deep chains section
    if !analysis.deep_chains.is_empty() {
        output.push_str(&format!(
            "   Deep Re-export Chains ({}):\n",
            analysis.deep_chains.len()
        ));

        // Deduplicate by chain path for display
        let mut seen_chains = HashSet::new();
        let mut displayed = 0;

        for chain_data in &analysis.deep_chains {
            let chain_key = chain_data.chain.join(" -> ");

            if seen_chains.insert(chain_key.clone()) && displayed < 5 {
                let warning = if chain_data.depth > 2 { " [!]" } else { "" };
                output.push_str(&format!(
                    "   |- {}: {} (depth: {}){}\n",
                    chain_data.symbol, chain_key, chain_data.depth, warning
                ));
                displayed += 1;
            }
        }

        if analysis.deep_chains.len() > displayed {
            output.push_str(&format!(
                "   └─ ... and {} more\n",
                analysis.deep_chains.len() - displayed
            ));
        }
        output.push('\n');
    }

    // Inconsistent paths section
    if !analysis.inconsistent_paths.is_empty() {
        output.push_str("   Inconsistent Import Paths:\n");

        for inconsistent in analysis.inconsistent_paths.iter().take(5) {
            output.push_str(&format!("   ├─ {} imported via:\n", inconsistent.symbol));
            output.push_str(&format!(
                "   │   ├─ {} ({} files) ← CANONICAL\n",
                inconsistent.canonical_path,
                // Count would be stored separately, using placeholder
                "?"
            ));

            for (path, count) in &inconsistent.alternative_paths {
                output.push_str(&format!("   │   └─ {} ({} files) ← LEGACY\n", path, count));
            }
        }

        if analysis.inconsistent_paths.len() > 5 {
            output.push_str(&format!(
                "   └─ ... and {} more\n",
                analysis.inconsistent_paths.len() - 5
            ));
        }
    }

    if analysis.missing_barrels.is_empty()
        && analysis.deep_chains.is_empty()
        && analysis.inconsistent_paths.is_empty()
    {
        output.push_str("   [OK] No barrel chaos detected\n");
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_barrel_file() {
        assert!(is_barrel_file("src/index.ts"));
        assert!(is_barrel_file("src/components/index.js"));
        assert!(is_barrel_file("index.tsx"));
        assert!(!is_barrel_file("src/utils.ts"));
        assert!(!is_barrel_file("src/component.tsx"));
    }

    #[test]
    fn test_has_index_file() {
        let files = vec![
            "src/utils.ts".to_string(),
            "src/index.ts".to_string(),
            "src/types.ts".to_string(),
        ];
        assert!(has_index_file(&files));

        let no_index = vec!["src/utils.ts".to_string(), "src/types.ts".to_string()];
        assert!(!has_index_file(&no_index));
    }

    #[test]
    fn test_get_directory() {
        assert_eq!(
            get_directory("src/components/Button.tsx"),
            Some("src/components".to_string())
        );
        assert_eq!(get_directory("src/index.ts"), Some("src".to_string()));
        assert_eq!(get_directory("index.ts"), Some("".to_string()));
    }

    #[test]
    fn test_is_pure_rust_project() {
        use crate::snapshot::SnapshotMetadata;
        use crate::types::FileAnalysis;

        // Helper to create a snapshot with given files
        fn make_snapshot(file_paths: Vec<&str>) -> Snapshot {
            Snapshot {
                metadata: SnapshotMetadata {
                    ..Default::default()
                },
                files: file_paths
                    .iter()
                    .map(|path| FileAnalysis {
                        path: path.to_string(),
                        ..Default::default()
                    })
                    .collect(),
                edges: Vec::new(),
                export_index: std::collections::HashMap::new(),
                command_bridges: Vec::new(),
                event_bridges: Vec::new(),
                barrels: Vec::new(),
                semantic_facts: None,
                symbol_graph: None,
            }
        }

        // Pure Rust project - only .rs files
        let rust_snapshot = make_snapshot(vec!["src/main.rs", "src/lib.rs"]);
        assert!(is_pure_rust_project(&rust_snapshot));

        // TypeScript project
        let ts_snapshot = make_snapshot(vec!["src/index.ts", "src/utils.ts"]);
        assert!(!is_pure_rust_project(&ts_snapshot));

        // JavaScript project
        let js_snapshot = make_snapshot(vec!["src/index.js", "src/utils.jsx"]);
        assert!(!is_pure_rust_project(&js_snapshot));

        // Mixed project (Tauri-style)
        let mixed_snapshot = make_snapshot(vec!["src/main.rs", "src/index.tsx"]);
        assert!(!is_pure_rust_project(&mixed_snapshot));

        // Test different JS/TS extensions
        assert!(!is_pure_rust_project(&make_snapshot(vec!["app.mjs"])));
        assert!(!is_pure_rust_project(&make_snapshot(vec!["config.cjs"])));
    }

    #[test]
    fn test_analyze_barrel_chaos_skips_rust_projects() {
        use crate::snapshot::SnapshotMetadata;
        use crate::types::FileAnalysis;

        // Pure Rust project should return empty analysis
        let rust_snapshot = Snapshot {
            metadata: SnapshotMetadata {
                ..Default::default()
            },
            files: vec![
                FileAnalysis {
                    path: "src/main.rs".to_string(),
                    ..Default::default()
                },
                FileAnalysis {
                    path: "src/lib.rs".to_string(),
                    ..Default::default()
                },
            ],
            edges: Vec::new(),
            export_index: std::collections::HashMap::new(),
            command_bridges: Vec::new(),
            event_bridges: Vec::new(),
            barrels: Vec::new(),
            semantic_facts: None,
            symbol_graph: None,
        };

        let analysis = analyze_barrel_chaos(&rust_snapshot);
        assert!(analysis.missing_barrels.is_empty());
        assert!(analysis.deep_chains.is_empty());
        assert!(analysis.inconsistent_paths.is_empty());
    }
}
