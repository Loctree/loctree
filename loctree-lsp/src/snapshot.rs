//! Snapshot loading and watching for loctree LSP
//!
//! Loads `.loctree/snapshot.json` and watches for changes.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use loctree::analyzer::cycles::find_cycles;
use loctree::analyzer::dead_parrots::{DeadFilterConfig, find_dead_exports};
use loctree::analyzer::twins::{ExactTwin, detect_exact_twins};
use loctree::fs_utils::load_loctignore_dead_ok_globs;
use loctree::snapshot::Snapshot;
use tokio::sync::RwLock;

/// Snapshot state wrapper for async access
#[derive(Clone)]
pub struct SnapshotState {
    inner: Arc<RwLock<Option<LoadedSnapshot>>>,
}

/// Loaded snapshot with metadata
pub struct LoadedSnapshot {
    /// The parsed snapshot
    pub snapshot: Snapshot,
    /// Workspace root directory (for staleness checks and .loctignore resolution)
    pub workspace_root: PathBuf,
}

impl SnapshotState {
    /// Create a new empty snapshot state
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
        }
    }

    /// Load snapshot from workspace root using loctree library API.
    ///
    /// Uses `Snapshot::load()` which handles global cache, branch@commit format,
    /// legacy fallback, and schema migration automatically.
    ///
    /// The synchronous `Snapshot::load()` call runs on a blocking thread via
    /// `spawn_blocking` to avoid stalling the async LSP event loop.
    pub async fn load(&self, workspace_root: &Path) -> Result<(), SnapshotError> {
        let root = workspace_root.to_path_buf();
        let snapshot = tokio::task::spawn_blocking({
            let root = root.clone();
            move || Snapshot::load(&root)
        })
        .await
        .map_err(|e| SnapshotError::ReadError(root.clone(), format!("task join error: {}", e)))?
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SnapshotError::NotFound(root.join(".loctree"))
            } else {
                SnapshotError::ReadError(root.clone(), e.to_string())
            }
        })?;

        let loaded = LoadedSnapshot {
            snapshot,
            workspace_root: root,
        };

        let mut guard = self.inner.write().await;
        *guard = Some(loaded);

        Ok(())
    }

    /// Reload snapshot from disk using the stored workspace root.
    pub async fn reload(&self) -> Result<(), SnapshotError> {
        let workspace_root = {
            let guard = self.inner.read().await;
            guard
                .as_ref()
                .map(|loaded| loaded.workspace_root.clone())
                .ok_or(SnapshotError::NotLoaded)?
        };
        self.load(&workspace_root).await
    }

    /// Get read access to the snapshot
    pub async fn get(&self) -> Option<tokio::sync::RwLockReadGuard<'_, Option<LoadedSnapshot>>> {
        let guard = self.inner.read().await;
        if guard.is_some() { Some(guard) } else { None }
    }

    /// Check if snapshot is loaded
    pub async fn is_loaded(&self) -> bool {
        self.inner.read().await.is_some()
    }

    /// Get the workspace root path (if snapshot is loaded).
    pub async fn workspace_root(&self) -> Option<PathBuf> {
        let guard = self.inner.read().await;
        guard.as_ref().map(|loaded| loaded.workspace_root.clone())
    }

    /// Get dead exports for a specific file using the loctree library dead export detection.
    ///
    /// Uses `find_dead_exports()` from the dead_parrots module — Tarjan-based analysis
    /// with confidence levels, suppression support, and `.loctignore` dead-ok globs.
    pub async fn dead_exports_for_file(&self, file_path: &str) -> Vec<DeadExportInfo> {
        let guard = self.inner.read().await;
        let Some(loaded) = guard.as_ref() else {
            return Vec::new();
        };

        let dead_ok_globs = load_loctignore_dead_ok_globs(&loaded.workspace_root);
        let config = DeadFilterConfig {
            dead_ok_globs,
            ..DeadFilterConfig::default()
        };

        let all_dead = find_dead_exports(&loaded.snapshot.files, true, None, config);

        all_dead
            .into_iter()
            .filter(|d| d.file.ends_with(file_path) || file_path.ends_with(&d.file))
            .map(|d| DeadExportInfo {
                symbol: d.symbol,
                line: d.line.unwrap_or(1),
                confidence: d.confidence,
                reason: d.reason,
            })
            .collect()
    }

    /// Get exact twins involving a specific file.
    ///
    /// Uses `detect_exact_twins()` from the twins module and filters the results
    /// to twins where at least one location matches `file_path`.
    pub async fn twins_for_file(&self, file_path: &str) -> Vec<ExactTwin> {
        let guard = self.inner.read().await;
        let Some(loaded) = guard.as_ref() else {
            return Vec::new();
        };

        detect_exact_twins(&loaded.snapshot.files, false)
            .into_iter()
            .filter(|twin| {
                twin.locations.iter().any(|loc| {
                    loc.file_path.ends_with(file_path) || file_path.ends_with(&loc.file_path)
                })
            })
            .collect()
    }

    /// Get cycles involving a specific file using Tarjan's SCC algorithm.
    ///
    /// Uses `find_cycles()` from the cycles module — finds all strongly connected
    /// components, not just bidirectional edges.
    pub async fn cycles_for_file(&self, file_path: &str) -> Vec<CycleInfo> {
        let guard = self.inner.read().await;
        let Some(loaded) = guard.as_ref() else {
            return Vec::new();
        };

        let edges: Vec<(String, String, String)> = loaded
            .snapshot
            .edges
            .iter()
            .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
            .collect();

        let cycles = find_cycles(&edges);
        let normalized_target = normalize_path(file_path);

        cycles
            .into_iter()
            .filter_map(|files| {
                let current_file = files
                    .iter()
                    .find(|f| paths_match(&normalize_path(f), &normalized_target))
                    .cloned()?;

                let cycle_type = if files.len() == 2 {
                    "bidirectional".to_string()
                } else {
                    format!("{}-way cycle", files.len())
                };

                let import_line = find_cycle_import_line(
                    &loaded.snapshot.files,
                    &loaded.snapshot.edges,
                    &files,
                    &current_file,
                );

                Some(CycleInfo {
                    files,
                    cycle_type,
                    import_line,
                })
            })
            .collect()
    }

    /// Find where a symbol is defined.
    ///
    /// This looks at:
    /// 1. Edges: if current file imports from another file, check if symbol matches edge label
    /// 2. Export index: if symbol name exists in export_index, find which file exports it
    /// 3. File exports: look through files to find exports matching the symbol name
    ///
    /// # Arguments
    /// * `current_file` - The file where the cursor is (for context on imports)
    /// * `symbol` - The symbol name to find definition for
    ///
    /// # Returns
    /// * `Some(DefinitionLocation)` if definition found
    /// * `None` if no definition found
    pub async fn find_definition(
        &self,
        current_file: &str,
        symbol: &str,
    ) -> Option<DefinitionLocation> {
        let guard = self.inner.read().await;
        let loaded = guard.as_ref()?;
        let snapshot = &loaded.snapshot;

        // Normalize file path for matching (strip leading ./ or /)
        let normalized_current = normalize_path(current_file);

        // Strategy 1: Look at edges from current file and match symbol with edge label
        for edge in &snapshot.edges {
            let edge_from = normalize_path(&edge.from);
            if paths_match(&edge_from, &normalized_current) {
                // Check if edge label matches symbol (edge.label contains the imported symbol)
                if edge.label == symbol || edge.label.contains(symbol) {
                    // Found an edge - now find the export line in target file
                    let target_file = &edge.to;
                    if let Some(line) = find_export_line_in_snapshot(snapshot, target_file, symbol)
                    {
                        return Some(DefinitionLocation {
                            file: target_file.clone(),
                            line,
                        });
                    }
                    // Even without exact line, return the target file at line 1
                    return Some(DefinitionLocation {
                        file: target_file.clone(),
                        line: 1,
                    });
                }
            }
        }

        // Strategy 2: Use export_index to find symbol
        if let Some(files) = snapshot.export_index.get(symbol)
            && let Some(file_path) = files.first()
        {
            if let Some(line) = find_export_line_in_snapshot(snapshot, file_path, symbol) {
                return Some(DefinitionLocation {
                    file: file_path.clone(),
                    line,
                });
            }
            return Some(DefinitionLocation {
                file: file_path.clone(),
                line: 1,
            });
        }

        // Strategy 3: Search all files' exports for the symbol
        for file in &snapshot.files {
            for export in &file.exports {
                if export.name == symbol {
                    return Some(DefinitionLocation {
                        file: file.path.clone(),
                        line: export.line.unwrap_or(1),
                    });
                }
            }
        }

        None
    }

    /// Find all references to a symbol exported from a file
    ///
    /// Returns a list of ReferenceInfo for all files that import the symbol.
    ///
    /// # Arguments
    /// * `file_path` - The file containing the export (can be relative or absolute)
    /// * `symbol` - The symbol name to find references for (optional, if None finds all importers)
    pub async fn find_references(
        &self,
        file_path: &str,
        symbol: Option<&str>,
    ) -> Vec<ReferenceInfo> {
        let guard = self.inner.read().await;
        let Some(loaded) = guard.as_ref() else {
            return Vec::new();
        };

        let mut references = Vec::new();

        // Normalize the file path for comparison
        let normalized_target = normalize_path(file_path);

        // Find all edges where this file is the "to" (imported) target
        for edge in &loaded.snapshot.edges {
            let edge_to_normalized = normalize_path(&edge.to);

            // Check if this edge points to our target file
            if paths_match(&normalized_target, &edge_to_normalized) {
                // If a symbol is specified, check if the edge label matches
                if let Some(sym) = symbol {
                    let label_contains_symbol = edge
                        .label
                        .split(',')
                        .map(|s: &str| s.trim())
                        .any(|s| s == sym || s == "*");

                    if !label_contains_symbol {
                        continue;
                    }
                }

                // Try to find the specific import line in the importing file
                let import_line = find_import_line(&loaded.snapshot.files, &edge.from, &edge.to);

                references.push(ReferenceInfo {
                    file: edge.from.clone(),
                    line: import_line.unwrap_or(0),
                });
            }
        }

        // Deduplicate references
        references.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
        references.dedup_by(|a, b| a.file == b.file && a.line == b.line);

        references
    }

    /// Find the export location for a symbol in a file
    ///
    /// Returns (file_path, line) if found
    pub async fn find_export_location(
        &self,
        file_path: &str,
        symbol: &str,
    ) -> Option<(String, usize)> {
        let guard = self.inner.read().await;
        let loaded = guard.as_ref()?;

        let normalized_target = normalize_path(file_path);

        // Find the file in the snapshot
        for file in &loaded.snapshot.files {
            let file_normalized = normalize_path(&file.path);
            if paths_match(&normalized_target, &file_normalized) {
                // Find the export
                for export in &file.exports {
                    if export.name == symbol {
                        return Some((file.path.clone(), export.line.unwrap_or(1)));
                    }
                }
            }
        }

        None
    }
}

/// Reference information for a symbol
#[derive(Debug, Clone)]
pub struct ReferenceInfo {
    /// File path where the reference occurs
    pub file: String,
    /// Line number (0 if unknown)
    pub line: usize,
}

/// Definition lookup result
#[derive(Debug, Clone)]
pub struct DefinitionLocation {
    /// Target file path (relative to project root)
    pub file: String,
    /// 1-based line number where symbol is defined
    pub line: usize,
}

/// Find the line number where a file imports another file
fn find_import_line(
    files: &[loctree::types::FileAnalysis],
    importer_path: &str,
    imported_path: &str,
) -> Option<usize> {
    let importer_normalized = normalize_path(importer_path);
    let imported_normalized = normalize_path(imported_path);

    // Find the importer file
    for file in files {
        let file_normalized = normalize_path(&file.path);
        if paths_match(&importer_normalized, &file_normalized) {
            // Find the import statement
            for import in &file.imports {
                if let Some(ref resolved) = import.resolved_path {
                    let resolved_normalized = normalize_path(resolved);
                    if paths_match(&imported_normalized, &resolved_normalized) {
                        return import.line;
                    }
                }
                // Also check source_raw for path matching
                let source_normalized = normalize_path(&import.source);
                if paths_match(&imported_normalized, &source_normalized) {
                    return import.line;
                }
            }
        }
    }

    None
}

/// Find the import line in a cycle for a specific file.
///
/// Tries to match the "next" file in cycle order first, then falls back to any
/// outgoing edge from `current_file` to another node in the same cycle.
fn find_cycle_import_line(
    files: &[loctree::types::FileAnalysis],
    edges: &[loctree::snapshot::GraphEdge],
    cycle_files: &[String],
    current_file: &str,
) -> Option<usize> {
    let current_normalized = normalize_path(current_file);
    let mut target_candidates = Vec::new();
    let mut seen_targets = HashSet::new();

    let mut push_target = |target: &str| {
        let normalized = normalize_path(target);
        if seen_targets.insert(normalized) {
            target_candidates.push(target.to_string());
        }
    };

    if let Some(idx) = cycle_files
        .iter()
        .position(|f| paths_match(&normalize_path(f), &current_normalized))
    {
        let next = &cycle_files[(idx + 1) % cycle_files.len()];
        push_target(next);
    }

    for edge in edges {
        if !paths_match(&normalize_path(&edge.from), &current_normalized) {
            continue;
        }
        if cycle_files
            .iter()
            .any(|f| paths_match(&normalize_path(f), &normalize_path(&edge.to)))
        {
            push_target(&edge.to);
        }
    }

    target_candidates
        .into_iter()
        .find_map(|target| find_import_line(files, current_file, &target))
}

/// Find the line number where a symbol is exported in a given file
fn find_export_line_in_snapshot(
    snapshot: &Snapshot,
    file_path: &str,
    symbol: &str,
) -> Option<usize> {
    let normalized_target = normalize_path(file_path);

    for file in &snapshot.files {
        let normalized_file = normalize_path(&file.path);
        if paths_match(&normalized_file, &normalized_target) {
            for export in &file.exports {
                if export.name == symbol {
                    return export.line;
                }
            }
        }
    }
    None
}

/// Normalize a file path by stripping leading ./ or /
fn normalize_path(path: &str) -> String {
    path.trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

/// Check if two normalized paths match (handles suffix matching for relative paths)
fn paths_match(a: &str, b: &str) -> bool {
    a == b || a.ends_with(b) || b.ends_with(a)
}

impl Default for SnapshotState {
    fn default() -> Self {
        Self::new()
    }
}

/// Dead export info for diagnostics
#[derive(Debug, Clone)]
pub struct DeadExportInfo {
    pub symbol: String,
    pub line: usize,
    pub confidence: String,
    pub reason: String,
}

/// Cycle info for diagnostics
#[derive(Debug, Clone)]
pub struct CycleInfo {
    pub files: Vec<String>,
    pub cycle_type: String,
    pub import_line: Option<usize>,
}

/// Snapshot loading errors
#[derive(Debug)]
pub enum SnapshotError {
    /// Snapshot file not found
    NotFound(PathBuf),
    /// Error reading snapshot file
    ReadError(PathBuf, String),
    /// Error parsing snapshot JSON
    ParseError(PathBuf, String),
    /// Invalid path structure
    InvalidPath(PathBuf),
    /// Snapshot not yet loaded
    NotLoaded,
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotError::NotFound(path) => {
                write!(
                    f,
                    "Snapshot not found at {:?}. Run `loct` to scan your project first.",
                    path
                )
            }
            SnapshotError::ReadError(path, e) => {
                write!(f, "Error reading snapshot {:?}: {}", path, e)
            }
            SnapshotError::ParseError(path, e) => {
                write!(f, "Error parsing snapshot {:?}: {}", path, e)
            }
            SnapshotError::InvalidPath(path) => {
                write!(f, "Invalid snapshot path: {:?}", path)
            }
            SnapshotError::NotLoaded => {
                write!(f, "Snapshot not loaded")
            }
        }
    }
}

impl std::error::Error for SnapshotError {}

#[cfg(test)]
mod tests {
    use super::*;
    use loctree::snapshot::{GraphEdge, Snapshot, project_cache_dir};
    use loctree::types::{ExportSymbol, FileAnalysis, ImportEntry, ImportKind};
    use tempfile::TempDir;

    fn build_export(name: &str, line: usize) -> ExportSymbol {
        ExportSymbol {
            name: name.to_string(),
            kind: "function".to_string(),
            export_type: "named".to_string(),
            line: Some(line),
            params: Vec::new(),

            symbol_id: ::loctree::types::SymbolIdV1::default(),
        }
    }

    /// Write snapshot using `Snapshot::save()` so it goes to the same cache
    /// location that `Snapshot::load()` looks at. This ensures consistency
    /// between save → load → reload cycles.
    fn write_snapshot(root: &Path, snapshot: &Snapshot) {
        snapshot.save(root).expect("save snapshot");
    }

    /// Clean up the global cache directory for a temp project root.
    /// Prevents test data from accumulating in `~/.cache/loctree/projects/`.
    fn cleanup_cache(root: &Path) {
        let cache_dir = project_cache_dir(root);
        let _ = std::fs::remove_dir_all(cache_dir);
    }

    #[tokio::test]
    async fn snapshot_loads_and_reloads() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
        snapshot.files = vec![FileAnalysis {
            path: "src/lib.rs".to_string(),
            exports: vec![build_export("hello", 5)],
            ..Default::default()
        }];

        write_snapshot(root, &snapshot);

        let state = SnapshotState::new();
        state.load(root).await.expect("load snapshot");
        assert!(state.is_loaded().await);

        // Update snapshot with new export and save → overwrite in cache
        snapshot.files[0].exports.push(build_export("world", 10));
        write_snapshot(root, &snapshot);
        state.reload().await.expect("reload snapshot");

        cleanup_cache(root);
    }

    #[tokio::test]
    async fn finds_dead_exports_for_file() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
        snapshot.files = vec![FileAnalysis {
            path: "src/foo.rs".to_string(),
            exports: vec![build_export("Dead", 7)],
            ..Default::default()
        }];

        write_snapshot(root, &snapshot);

        let state = SnapshotState::new();
        state.load(root).await.expect("load snapshot");

        let dead = state.dead_exports_for_file("src/foo.rs").await;
        assert!(!dead.is_empty(), "Expected dead exports, found none");
        assert!(
            dead.iter().any(|d| d.symbol == "Dead"),
            "Expected 'Dead' symbol in dead exports: {:?}",
            dead.iter().map(|d| &d.symbol).collect::<Vec<_>>()
        );

        cleanup_cache(root);
    }

    #[tokio::test]
    async fn finds_cycles_for_file() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
        let mut a = FileAnalysis::new("src/a.rs".to_string());
        let mut import = ImportEntry::new("./b".to_string(), ImportKind::Static);
        import.resolved_path = Some("src/b.rs".to_string());
        import.line = Some(12);
        a.imports.push(import);
        snapshot.files = vec![a, FileAnalysis::new("src/b.rs".to_string())];
        snapshot.edges = vec![
            GraphEdge {
                from: "src/a.rs".to_string(),
                to: "src/b.rs".to_string(),
                label: "mod b".to_string(),
            },
            GraphEdge {
                from: "src/b.rs".to_string(),
                to: "src/a.rs".to_string(),
                label: "mod a".to_string(),
            },
        ];

        write_snapshot(root, &snapshot);

        let state = SnapshotState::new();
        state.load(root).await.expect("load snapshot");

        let cycles = state.cycles_for_file("src/a.rs").await;
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].cycle_type, "bidirectional");
        assert_eq!(cycles[0].import_line, Some(12));

        cleanup_cache(root);
    }

    #[tokio::test]
    async fn finds_twins_for_file() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
        snapshot.files = vec![
            FileAnalysis {
                path: "src/a.rs".to_string(),
                exports: vec![build_export("TwinFn", 10)],
                ..Default::default()
            },
            FileAnalysis {
                path: "src/b.rs".to_string(),
                exports: vec![build_export("TwinFn", 20)],
                ..Default::default()
            },
        ];

        write_snapshot(root, &snapshot);

        let state = SnapshotState::new();
        state.load(root).await.expect("load snapshot");

        let twins = state.twins_for_file("src/a.rs").await;
        assert_eq!(twins.len(), 1);
        assert_eq!(twins[0].name, "TwinFn");

        cleanup_cache(root);
    }

    #[tokio::test]
    async fn finds_definition_via_export_index() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
        snapshot
            .export_index
            .insert("Foo".to_string(), vec!["src/foo.rs".to_string()]);
        snapshot.files = vec![FileAnalysis {
            path: "src/foo.rs".to_string(),
            exports: vec![build_export("Foo", 12)],
            ..Default::default()
        }];

        write_snapshot(root, &snapshot);

        let state = SnapshotState::new();
        state.load(root).await.expect("load snapshot");

        let def = state
            .find_definition("src/main.rs", "Foo")
            .await
            .expect("definition");
        assert_eq!(def.file, "src/foo.rs");
        assert_eq!(def.line, 12);

        cleanup_cache(root);
    }

    #[tokio::test]
    async fn finds_references_and_export_location() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
        snapshot.files = vec![FileAnalysis {
            path: "src/target.rs".to_string(),
            exports: vec![build_export("Thing", 3)],
            ..Default::default()
        }];
        snapshot.edges = vec![GraphEdge {
            from: "src/importer.rs".to_string(),
            to: "src/target.rs".to_string(),
            label: "Thing".to_string(),
        }];

        write_snapshot(root, &snapshot);

        let state = SnapshotState::new();
        state.load(root).await.expect("load snapshot");

        let refs = state.find_references("src/target.rs", Some("Thing")).await;
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].file, "src/importer.rs");

        let export_loc = state
            .find_export_location("src/target.rs", "Thing")
            .await
            .expect("export location");
        assert_eq!(export_loc.0, "src/target.rs");
        assert_eq!(export_loc.1, 3);

        cleanup_cache(root);
    }

    #[test]
    fn normalize_and_match_paths() {
        let normalized = normalize_path("./src/lib.rs");
        assert_eq!(normalized, "src/lib.rs");
        assert!(paths_match("src/lib.rs", "/src/lib.rs"));
        assert!(paths_match("lib.rs", "src/lib.rs"));
    }
}
