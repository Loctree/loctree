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
        // index.ts / index.js is a JS/TS culture. A stray .ts elsewhere must
        // not produce barrel advice for Rust/Python/Swift/shell directories.
        if !directory_uses_js_ts_barrel_culture(files) {
            continue;
        }

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

/// True for names that look like a real identifier — guards against parser
/// artifacts (`*`, `}`, empty strings from a namespace/brace import that
/// failed to parse cleanly) surfacing as an "inconsistent import" symbol.
fn is_identifier_like(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {
            chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        }
        _ => false,
    }
}

/// Detect symbols imported via inconsistent paths
///
/// Groups imports by (symbol name, resolved definition identity) — NOT by
/// bare name alone. A bare-name grouping merges unrelated symbols that only
/// happen to share a name (`std::process::Command` and `crate::cli::Command`
/// both surface as "Command"); keying by the resolved definition path (when
/// the import graph resolved one, else the raw source string itself, which
/// is then its own identity) keeps them apart while still collapsing
/// textually different paths that resolve to the SAME definition — the
/// actual "inconsistent path" signal. Canonical selection then prefers the
/// definition site (a source whose resolved target is not a re-exporting
/// barrel) over raw import-count majority, so a widely-imported barrel
/// re-export never outranks the file that actually defines the symbol.
fn detect_inconsistent_paths(snapshot: &Snapshot) -> Vec<InconsistentImport> {
    // Group key: (symbol name, definition identity). Inner map: raw source
    // string -> import count.
    let mut symbol_sources: HashMap<(String, String), HashMap<String, usize>> = HashMap::new();
    // Per group, the resolved target file for each raw source (when known)
    // — used only to prefer a non-barrel canonical over raw majority.
    let mut source_targets: HashMap<(String, String), HashMap<String, Option<String>>> =
        HashMap::new();

    // N11 (audit): the definition identity must be the symbol's ORIGIN, not
    // the direct import target. Keying on `resolved_path` alone split the
    // barrel spelling and the definition spelling of the SAME symbol into
    // two one-source groups, hiding exactly the inconsistency this pass
    // exists to report. Follow named (and star) reexports from a barrel to
    // the file that actually defines the symbol before grouping.
    let mut reexport_origin: HashMap<(String, String), String> = HashMap::new();
    for file in &snapshot.files {
        for reexport in &file.reexports {
            let Some(resolved) = &reexport.resolved else {
                continue;
            };
            match &reexport.kind {
                crate::types::ReexportKind::Named(pairs) => {
                    for (_original, exported) in pairs {
                        reexport_origin
                            .insert((file.path.clone(), exported.clone()), resolved.clone());
                    }
                }
                crate::types::ReexportKind::Star => {
                    reexport_origin.insert((file.path.clone(), "*".to_string()), resolved.clone());
                }
            }
        }
    }
    let follow_origin = |start: &str, name: &str| -> String {
        let mut current = start.to_string();
        for _ in 0..16 {
            let next = reexport_origin
                .get(&(current.clone(), name.to_string()))
                .or_else(|| reexport_origin.get(&(current.clone(), "*".to_string())));
            match next {
                Some(next) if *next != current => current = next.clone(),
                _ => break,
            }
        }
        current
    };

    for file in &snapshot.files {
        for import in &file.imports {
            for symbol in &import.symbols {
                let symbol_name = symbol.alias.as_ref().unwrap_or(&symbol.name);
                if !is_identifier_like(symbol_name) {
                    continue;
                }

                let definition_key = import
                    .resolved_path
                    .as_deref()
                    .map(|path| follow_origin(path, &symbol.name))
                    .unwrap_or_else(|| import.source.clone());
                let group_key = (symbol_name.clone(), definition_key);

                symbol_sources
                    .entry(group_key.clone())
                    .or_default()
                    .entry(import.source.clone())
                    .and_modify(|count| *count += 1)
                    .or_insert(1);
                source_targets
                    .entry(group_key)
                    .or_default()
                    .entry(import.source.clone())
                    .or_insert_with(|| import.resolved_path.clone());
            }
        }
    }

    let mut inconsistent = Vec::new();

    for ((symbol, definition_key), sources) in symbol_sources {
        // Skip if symbol only imported from one distinct raw path within
        // this definition group.
        if sources.len() <= 1 {
            continue;
        }

        let targets = source_targets
            .get(&(symbol.clone(), definition_key))
            .cloned()
            .unwrap_or_default();

        // Canonical = the definition site (resolved target is not a
        // re-exporting index.* barrel), not whichever raw path happens to
        // be imported most. Ties (or no resolvable target at all) fall
        // back to raw import-count majority, then alphabetical order for
        // determinism.
        let mut sources_vec: Vec<_> = sources.into_iter().collect();
        sources_vec.sort_by(|(a_path, a_count), (b_path, b_count)| {
            let a_is_barrel = targets
                .get(a_path)
                .and_then(|t| t.as_deref())
                .is_some_and(is_barrel_file);
            let b_is_barrel = targets
                .get(b_path)
                .and_then(|t| t.as_deref())
                .is_some_and(is_barrel_file);
            a_is_barrel
                .cmp(&b_is_barrel)
                .then_with(|| b_count.cmp(a_count))
                .then_with(|| a_path.cmp(b_path))
        });

        if let Some((canonical, _canonical_count)) = sources_vec.first().cloned() {
            let alternative_paths: Vec<_> = sources_vec
                .iter()
                .filter(|(path, _)| path != &canonical)
                .map(|(path, count)| (path.clone(), *count))
                .collect();

            // Only flag if there are significant alternatives (not just 1 usage)
            if alternative_paths.iter().any(|(_, count)| *count > 1) {
                inconsistent.push(InconsistentImport {
                    symbol: symbol.clone(),
                    canonical_path: canonical,
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

fn is_js_ts_barrel_language(lang: &str) -> bool {
    // classify maps tsx→ts and jsx/mjs/cjs→js; mts/cts stay as the raw ext.
    matches!(lang, "ts" | "js" | "mts" | "cts")
}

/// Dominant language of a directory (strict unique max). Ties → none.
fn directory_dominant_language(files: &[String]) -> Option<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for file in files {
        let lang = super::classify::language_from_path(file);
        if lang.is_empty() {
            continue;
        }
        *counts.entry(lang).or_insert(0) += 1;
    }
    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    match ranked.as_slice() {
        [(lang, count), rest @ ..] if rest.iter().all(|(_, other)| other < count) => {
            Some(lang.clone())
        }
        _ => None,
    }
}

fn directory_uses_js_ts_barrel_culture(files: &[String]) -> bool {
    directory_dominant_language(files).is_some_and(|lang| is_js_ts_barrel_language(&lang))
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
                scan_diagnostics: None,
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
            scan_diagnostics: None,
        };

        let analysis = analyze_barrel_chaos(&rust_snapshot);
        assert!(analysis.missing_barrels.is_empty());
        assert!(analysis.deep_chains.is_empty());
        assert!(analysis.inconsistent_paths.is_empty());
    }

    fn file(path: &str) -> crate::types::FileAnalysis {
        crate::types::FileAnalysis {
            path: path.to_string(),
            ..Default::default()
        }
    }

    fn edge(from: &str, to: &str) -> crate::snapshot::GraphEdge {
        crate::snapshot::GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            label: "import".to_string(),
        }
    }

    fn snapshot_with(
        files: Vec<crate::types::FileAnalysis>,
        edges: Vec<crate::snapshot::GraphEdge>,
    ) -> Snapshot {
        Snapshot {
            metadata: crate::snapshot::SnapshotMetadata {
                ..Default::default()
            },
            files,
            edges,
            export_index: std::collections::HashMap::new(),
            command_bridges: Vec::new(),
            event_bridges: Vec::new(),
            barrels: Vec::new(),
            semantic_facts: None,
            symbol_graph: None,
            scan_diagnostics: None,
        }
    }

    /// Acceptance: Rust crate + one stray .ts elsewhere → zero barrel advice for Rust dirs.
    ///
    /// rust/src has 3 files and 3 external imports, so the old repo-wide guard
    /// (is_pure_rust_project defeated by tools/stray.ts) would have emitted
    /// "create index.ts" for the Rust crate.
    fn rust_crate_plus_stray_ts() -> Snapshot {
        snapshot_with(
            vec![
                file("rust/src/lib.rs"),
                file("rust/src/a.rs"),
                file("rust/src/b.rs"),
                file("rust/bin/main.rs"),
                file("tools/stray.ts"),
            ],
            vec![
                edge("rust/bin/main.rs", "rust/src/lib.rs"),
                edge("rust/bin/main.rs", "rust/src/a.rs"),
                edge("rust/bin/main.rs", "rust/src/b.rs"),
            ],
        )
    }

    /// Acceptance: TS directory without index.ts → "create index.ts" still fires.
    fn ts_dir_without_index() -> Snapshot {
        snapshot_with(
            vec![
                file("frontend/utils.ts"),
                file("frontend/types.ts"),
                file("frontend/helpers.ts"),
                file("app.ts"),
            ],
            vec![
                edge("app.ts", "frontend/utils.ts"),
                edge("app.ts", "frontend/types.ts"),
                edge("app.ts", "frontend/helpers.ts"),
            ],
        )
    }

    #[test]
    fn w3_02_directory_language_gate_is_per_directory() {
        assert_eq!(
            directory_dominant_language(&[
                "rust/src/lib.rs".into(),
                "rust/src/a.rs".into(),
                "rust/src/b.rs".into(),
            ]),
            Some("rs".to_string())
        );
        assert_eq!(
            directory_dominant_language(
                &["frontend/utils.ts".into(), "frontend/types.tsx".into(),]
            ),
            Some("ts".to_string())
        );
        assert_eq!(
            directory_dominant_language(&["a.rs".into(), "b.ts".into()]),
            None,
            "ties stay silent"
        );
        assert!(!directory_uses_js_ts_barrel_culture(&[
            "rust/src/lib.rs".into(),
            "rust/src/a.rs".into(),
        ]));
        assert!(directory_uses_js_ts_barrel_culture(&[
            "frontend/utils.ts".into(),
            "frontend/types.tsx".into(),
        ]));
        assert!(
            directory_uses_js_ts_barrel_culture(&["esm/a.mts".into(), "esm/b.mts".into()]),
            "mts is TypeScript barrel culture (classify leaves the raw ext)"
        );
    }

    #[test]
    fn w3_02_rust_crate_with_stray_ts_gets_zero_rust_barrel_advice() {
        // Acceptance: Rust crate + one stray .ts elsewhere → zero barrel advice for Rust dirs.
        let snapshot = rust_crate_plus_stray_ts();
        assert!(
            !is_pure_rust_project(&snapshot),
            "stray .ts must defeat the repo-wide rust skip"
        );

        let analysis = analyze_barrel_chaos(&snapshot);
        assert!(
            analysis.missing_barrels.is_empty(),
            "Rust crate + stray .ts must yield zero missing-barrel advice, got {:?}",
            analysis.missing_barrels
        );
        assert!(
            analysis
                .missing_barrels
                .iter()
                .all(|b| !b.directory.starts_with("rust")),
            "Rust dirs must not get barrel advice: {:?}",
            analysis.missing_barrels
        );

        let formatted = format_barrel_analysis(&analysis);
        assert!(
            !formatted.contains("rust/src"),
            "formatted advice leaked a Rust dir:\n{formatted}"
        );
        assert!(
            !formatted.contains("create index.ts"),
            "stray .ts must not turn on create index.ts for a Rust crate:\n{formatted}"
        );
    }

    #[test]
    fn w3_02_ts_directory_without_index_still_advises_create_index_ts() {
        // Acceptance: TS directory without index.ts → "create index.ts" still fires.
        let snapshot = ts_dir_without_index();
        let analysis = analyze_barrel_chaos(&snapshot);
        assert!(
            analysis
                .missing_barrels
                .iter()
                .any(|b| b.directory == "frontend"),
            "TS dir without index.ts must still be advised: {:?}",
            analysis.missing_barrels
        );

        let formatted = format_barrel_analysis(&analysis);
        assert!(
            formatted.contains("create index.ts"),
            "TS missing-barrel advice must still say create index.ts:\n{formatted}"
        );
        assert!(
            formatted.contains("frontend/"),
            "formatted advice must name the TS dir:\n{formatted}"
        );
    }

    #[test]
    fn w3_02_barrel_advice_respects_directory_language() {
        // Mixed repo: both acceptance fixtures in one snapshot (the original hak).
        // Repo-wide is_pure_rust_project is false because of the .ts files, so
        // the old guard would advise "create index.ts" for rust/src and pkg.
        let snapshot = snapshot_with(
            vec![
                file("rust/src/lib.rs"),
                file("rust/src/a.rs"),
                file("rust/src/b.rs"),
                file("rust/bin/main.rs"),
                file("pkg/mod_a.py"),
                file("pkg/mod_b.py"),
                file("pkg/mod_c.py"),
                file("app.py"),
                file("tools/stray.ts"),
                file("frontend/utils.ts"),
                file("frontend/types.ts"),
                file("frontend/helpers.ts"),
                file("app.ts"),
            ],
            vec![
                edge("rust/bin/main.rs", "rust/src/lib.rs"),
                edge("rust/bin/main.rs", "rust/src/a.rs"),
                edge("rust/bin/main.rs", "rust/src/b.rs"),
                edge("app.py", "pkg/mod_a.py"),
                edge("app.py", "pkg/mod_b.py"),
                edge("app.py", "pkg/mod_c.py"),
                edge("app.ts", "frontend/utils.ts"),
                edge("app.ts", "frontend/types.ts"),
                edge("app.ts", "frontend/helpers.ts"),
            ],
        );

        assert!(
            !is_pure_rust_project(&snapshot),
            "mixed fixture must defeat the repo-wide rust skip"
        );

        let analysis = analyze_barrel_chaos(&snapshot);

        assert!(
            analysis
                .missing_barrels
                .iter()
                .all(|b| !b.directory.starts_with("rust")),
            "Rust dirs must not get barrel advice: {:?}",
            analysis.missing_barrels
        );
        assert!(
            analysis
                .missing_barrels
                .iter()
                .all(|b| b.directory != "pkg"),
            "Python dirs stay silent (W3-06 owns __init__): {:?}",
            analysis.missing_barrels
        );
        assert!(
            analysis
                .missing_barrels
                .iter()
                .any(|b| b.directory == "frontend"),
            "TS dir without index.ts must still be advised: {:?}",
            analysis.missing_barrels
        );
        assert_eq!(
            analysis
                .missing_barrels
                .iter()
                .map(|b| b.directory.as_str())
                .collect::<Vec<_>>(),
            vec!["frontend"],
            "only the TS dir should be missing a barrel: {:?}",
            analysis.missing_barrels
        );

        let formatted = format_barrel_analysis(&analysis);
        assert!(
            !formatted.contains("rust/src"),
            "formatted advice leaked a Rust dir:\n{formatted}"
        );
        assert!(
            formatted.contains("create index.ts"),
            "TS missing-barrel advice must still say create index.ts:\n{formatted}"
        );
        assert!(
            formatted.contains("frontend/"),
            "formatted advice must name the TS dir:\n{formatted}"
        );
    }

    /// F4 RED: a bare-name grouping merges `std::process::Command` (external,
    /// unresolvable) with `crate::cli::Command` (resolves internally to
    /// `src/cli/mod.rs`, imported via two different textual paths). They
    /// must never collapse into one InconsistentImport group by name alone.
    #[test]
    fn w4_inconsistent_paths_never_merges_different_definitions_by_bare_name() {
        use crate::types::{ImportEntry, ImportKind, ImportSymbol};

        fn command_import(source: &str, resolved: Option<&str>) -> ImportEntry {
            let mut import = ImportEntry::new(source.to_string(), ImportKind::Static);
            import.resolved_path = resolved.map(|r| r.to_string());
            import.symbols.push(ImportSymbol {
                name: "Command".to_string(),
                alias: None,
                is_default: false,
            });
            import
        }

        let mut std_importer = file("src/spawn.rs");
        std_importer
            .imports
            .push(command_import("std::process::Command", None));
        std_importer
            .imports
            .push(command_import("std::process::Command", None));

        let mut cli_importer_a = file("src/one.rs");
        cli_importer_a.imports.push(command_import(
            "crate::cli::Command",
            Some("src/cli/mod.rs"),
        ));
        cli_importer_a.imports.push(command_import(
            "crate::cli::Command",
            Some("src/cli/mod.rs"),
        ));

        let mut cli_importer_b = file("src/two.rs");
        cli_importer_b.imports.push(command_import(
            "crate::cli::commands::Command",
            Some("src/cli/mod.rs"),
        ));
        cli_importer_b.imports.push(command_import(
            "crate::cli::commands::Command",
            Some("src/cli/mod.rs"),
        ));

        let snapshot = snapshot_with(
            vec![std_importer, cli_importer_a, cli_importer_b],
            Vec::new(),
        );

        let inconsistent = detect_inconsistent_paths(&snapshot);

        assert_eq!(
            inconsistent.len(),
            1,
            "the std-lib Command (one raw source, filtered) and the \
             crate::cli Command (two raw sources, one real group) must \
             produce exactly one InconsistentImport: {inconsistent:?}"
        );
        assert!(
            inconsistent[0].canonical_path != "std::process::Command"
                && inconsistent[0]
                    .alternative_paths
                    .iter()
                    .all(|(path, _)| path != "std::process::Command"),
            "std::process::Command must never appear in the crate::cli \
             group at all (canonical or alternative): {inconsistent:?}"
        );
    }

    /// F4 RED: parser artifacts (`*`, `}`) from a namespace/brace import that
    /// failed to parse cleanly must never surface as an inconsistent-path
    /// symbol.
    #[test]
    fn w4_inconsistent_paths_filters_non_identifier_parser_artifacts() {
        use crate::types::{ImportEntry, ImportKind, ImportSymbol};

        fn bad_import(source: &str, symbol_name: &str) -> ImportEntry {
            let mut import = ImportEntry::new(source.to_string(), ImportKind::Static);
            import.symbols.push(ImportSymbol {
                name: symbol_name.to_string(),
                alias: None,
                is_default: false,
            });
            import
        }

        let mut a = file("src/a.ts");
        a.imports.push(bad_import("./one", "}"));
        a.imports.push(bad_import("./two", "}"));
        let mut b = file("src/b.ts");
        b.imports.push(bad_import("./one", "*"));
        b.imports.push(bad_import("./two", "*"));

        let snapshot = snapshot_with(vec![a, b], Vec::new());
        let inconsistent = detect_inconsistent_paths(&snapshot);

        assert!(
            inconsistent
                .iter()
                .all(|i| i.symbol != "}" && i.symbol != "*"),
            "parser artifacts must never appear as an inconsistent-path \
             symbol: {inconsistent:?}"
        );
    }
    /// N11 (audit): the SAME symbol imported via its barrel and via its
    /// defining file is ONE group with the definition as canonical — even
    /// when the barrel spelling has more imports. Direct-target grouping
    /// split them into two invisible one-source groups.
    #[test]
    fn n11_barrel_and_direct_spellings_group_and_definition_wins() {
        use crate::types::{ImportEntry, ImportKind, ImportSymbol, ReexportEntry, ReexportKind};

        fn widget_import(source: &str, resolved: &str) -> ImportEntry {
            let mut import = ImportEntry::new(source.to_string(), ImportKind::Static);
            import.resolved_path = Some(resolved.to_string());
            import.symbols.push(ImportSymbol {
                name: "Widget".to_string(),
                alias: None,
                is_default: false,
            });
            import
        }

        let mut barrel = file("src/index.ts");
        barrel.reexports.push(ReexportEntry {
            source: "./thing".to_string(),
            kind: ReexportKind::Named(vec![("Widget".to_string(), "Widget".to_string())]),
            resolved: Some("src/thing.ts".to_string()),
        });

        // Barrel spelling is the MAJORITY (3 imports); direct has 2.
        let mut a = file("src/a.ts");
        a.imports.push(widget_import("./index", "src/index.ts"));
        a.imports.push(widget_import("./index", "src/index.ts"));
        let mut b = file("src/b.ts");
        b.imports.push(widget_import("./index", "src/index.ts"));
        let mut c = file("src/c.ts");
        c.imports.push(widget_import("./thing", "src/thing.ts"));
        c.imports.push(widget_import("./thing", "src/thing.ts"));

        let snapshot = snapshot_with(vec![barrel, a, b, c], Vec::new());
        let inconsistent = detect_inconsistent_paths(&snapshot);

        assert_eq!(
            inconsistent.len(),
            1,
            "barrel and direct spellings of the same Widget are one inconsistency: {inconsistent:?}"
        );
        assert_eq!(
            inconsistent[0].canonical_path, "./thing",
            "the definition spelling wins over the barrel majority: {inconsistent:?}"
        );
    }
}
