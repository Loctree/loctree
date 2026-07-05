//! Snapshot comparison engine for temporal analysis
//!
//! This module compares loctree snapshots between different commits,
//! providing semantic analysis of how the codebase structure changed.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::analyzer::classify::{ArtifactFenceStats, artifact_class};
use crate::git::{ChangeStatus, ChangedFile, CommitInfo};
use crate::snapshot::Snapshot;

/// Result of comparing two snapshots
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotDiff {
    /// Information about the source commit
    pub from_commit: Option<CommitInfo>,
    /// Information about the target commit (None = working tree)
    pub to_commit: Option<CommitInfo>,
    /// Files that changed between snapshots
    pub files: FilesDiff,
    /// Changes in the import/export graph
    pub graph: GraphDiff,
    /// Changes in exported symbols
    pub exports: ExportsDiff,
    /// Impact analysis
    pub impact: ImpactAnalysis,
}

/// Diff of files between snapshots
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FilesDiff {
    /// Files added in the new snapshot
    pub added: Vec<PathBuf>,
    /// Files removed from the old snapshot
    pub removed: Vec<PathBuf>,
    /// Files modified between snapshots
    pub modified: Vec<PathBuf>,
    /// Files renamed (old_path -> new_path)
    pub renamed: Vec<(PathBuf, PathBuf)>,
}

impl FilesDiff {
    pub fn from_changed_files(changes: &[ChangedFile]) -> Self {
        let mut diff = FilesDiff::default();

        for change in changes {
            match change.status {
                ChangeStatus::Added => {
                    if let Some(path) = &change.new_path {
                        diff.added.push(path.clone());
                    }
                }
                ChangeStatus::Deleted => {
                    if let Some(path) = &change.old_path {
                        diff.removed.push(path.clone());
                    }
                }
                ChangeStatus::Modified => {
                    if let Some(path) = &change.new_path {
                        diff.modified.push(path.clone());
                    }
                }
                ChangeStatus::Renamed | ChangeStatus::Copied => {
                    if let (Some(old), Some(new)) = (&change.old_path, &change.new_path) {
                        diff.renamed.push((old.clone(), new.clone()));
                    }
                }
            }
        }

        diff
    }

    /// Total number of changes
    pub fn total_changes(&self) -> usize {
        self.added.len() + self.removed.len() + self.modified.len() + self.renamed.len()
    }
}

/// Edge in the import graph (for diff operations)
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiffEdge {
    /// Source file (importer)
    pub from: PathBuf,
    /// Target file (imported)
    pub to: PathBuf,
    /// Imported symbols (if known)
    pub symbols: Vec<String>,
}

/// Diff of the import graph
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GraphDiff {
    /// New import edges added
    pub edges_added: Vec<DiffEdge>,
    /// Import edges removed
    pub edges_removed: Vec<DiffEdge>,
}

/// An exported symbol
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExportedSymbol {
    /// File containing the export
    pub file: PathBuf,
    /// Symbol name
    pub name: String,
    /// Symbol kind (function, class, const, etc.)
    pub kind: String,
}

/// Diff of exports
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExportsDiff {
    /// New exports added
    pub added: Vec<ExportedSymbol>,
    /// Exports removed
    pub removed: Vec<ExportedSymbol>,
    /// Exports cut by the artifact fence (vendored/fixture/generated/template
    /// files) — counted so the cut is never silent. Empty when the fence is
    /// disabled via `--include-artifacts`.
    #[serde(default, skip_serializing_if = "ArtifactFenceStats::is_empty")]
    pub artifact_excluded: ArtifactFenceStats,
}

/// Impact analysis of the changes
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ImpactAnalysis {
    /// Number of files affected by changes
    pub affected_files: usize,
    /// Files that consume changed exports
    pub affected_consumers: Vec<PathBuf>,
    /// Risk score (0.0 - 1.0)
    pub risk_score: f64,
    /// Summary of the impact
    pub summary: String,
}

impl SnapshotDiff {
    /// Compare two snapshots and produce a diff (artifact fence on).
    pub fn compare(
        from_snapshot: &Snapshot,
        to_snapshot: &Snapshot,
        from_commit: Option<CommitInfo>,
        to_commit: Option<CommitInfo>,
        changed_files: &[ChangedFile],
    ) -> Self {
        Self::compare_fenced(
            from_snapshot,
            to_snapshot,
            from_commit,
            to_commit,
            changed_files,
            false,
        )
    }

    /// Compare two snapshots with explicit artifact-fence control.
    ///
    /// With `include_artifacts == false` (default), exports living in
    /// vendored/fixture/generated/template files (e.g. 182 "removed exports"
    /// from a deleted `public_dist/`) are cut from the added/removed export
    /// sections and counted in `exports.artifact_excluded`.
    pub fn compare_fenced(
        from_snapshot: &Snapshot,
        to_snapshot: &Snapshot,
        from_commit: Option<CommitInfo>,
        to_commit: Option<CommitInfo>,
        changed_files: &[ChangedFile],
        include_artifacts: bool,
    ) -> Self {
        let files = FilesDiff::from_changed_files(changed_files);
        let graph = Self::compare_graphs(from_snapshot, to_snapshot);
        let mut exports = Self::compare_exports(from_snapshot, to_snapshot);

        if !include_artifacts {
            let mut fence = ArtifactFenceStats::default();
            let keep = |export: &ExportedSymbol, fence: &mut ArtifactFenceStats| {
                let class = artifact_class(&export.file.to_string_lossy(), None);
                if class.is_artifact() {
                    fence.record(class);
                    false
                } else {
                    true
                }
            };
            exports.removed.retain(|e| keep(e, &mut fence));
            exports.added.retain(|e| keep(e, &mut fence));
            exports.artifact_excluded = fence;
        }

        let impact = Self::analyze_impact(&files, &graph, &exports, to_snapshot);

        Self {
            from_commit,
            to_commit,
            files,
            graph,
            exports,
            impact,
        }
    }

    /// Compare import graphs between snapshots
    fn compare_graphs(from: &Snapshot, to: &Snapshot) -> GraphDiff {
        let mut diff = GraphDiff::default();

        // Build edge sets for comparison
        let from_edges = Self::extract_edges(from);
        let to_edges = Self::extract_edges(to);

        // Find added edges
        for edge in &to_edges {
            if !from_edges.contains(edge) {
                diff.edges_added.push(edge.clone());
            }
        }

        // Find removed edges
        for edge in &from_edges {
            if !to_edges.contains(edge) {
                diff.edges_removed.push(edge.clone());
            }
        }

        diff
    }

    /// Extract edges from snapshot
    fn extract_edges(snapshot: &Snapshot) -> HashSet<DiffEdge> {
        let mut edges = HashSet::new();

        // Use snapshot.edges which contains GraphEdge structs
        for edge in &snapshot.edges {
            // Parse symbols from label (label format: "symbol1, symbol2" or empty)
            let symbols: Vec<String> = if edge.label.is_empty() {
                Vec::new()
            } else {
                edge.label.split(", ").map(|s| s.to_string()).collect()
            };

            edges.insert(DiffEdge {
                from: PathBuf::from(&edge.from),
                to: PathBuf::from(&edge.to),
                symbols,
            });
        }

        edges
    }

    /// Compare exports between snapshots
    fn compare_exports(from: &Snapshot, to: &Snapshot) -> ExportsDiff {
        let mut diff = ExportsDiff::default();

        let from_exports = Self::extract_exports(from);
        let to_exports = Self::extract_exports(to);

        for export in &to_exports {
            if !from_exports.contains(export) {
                diff.added.push(export.clone());
            }
        }

        for export in &from_exports {
            if !to_exports.contains(export) {
                diff.removed.push(export.clone());
            }
        }

        diff
    }

    /// Extract exports from snapshot
    fn extract_exports(snapshot: &Snapshot) -> HashSet<ExportedSymbol> {
        let mut exports = HashSet::new();

        // Use snapshot.files which is Vec<FileAnalysis>
        for file_info in &snapshot.files {
            let file_path = PathBuf::from(&file_info.path);

            for export in &file_info.exports {
                exports.insert(ExportedSymbol {
                    file: file_path.clone(),
                    name: export.name.clone(),
                    kind: export.kind.clone(),
                });
            }
        }

        exports
    }

    /// Analyze the impact of changes
    fn analyze_impact(
        files: &FilesDiff,
        graph: &GraphDiff,
        exports: &ExportsDiff,
        to_snapshot: &Snapshot,
    ) -> ImpactAnalysis {
        let affected_files = files.total_changes();

        // Find consumers of changed files
        let changed_paths: HashSet<String> = files
            .modified
            .iter()
            .chain(files.removed.iter())
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        let mut affected_consumers = Vec::new();
        for file_info in &to_snapshot.files {
            for import in &file_info.imports {
                if let Some(resolved) = &import.resolved_path
                    && changed_paths.contains(resolved)
                {
                    affected_consumers.push(PathBuf::from(&file_info.path));
                    break;
                }
            }
        }

        // Calculate risk score
        let risk_score = Self::calculate_risk_score(files, graph, exports);

        // Generate summary
        let summary = Self::generate_summary(files, graph, exports, &affected_consumers);

        ImpactAnalysis {
            affected_files,
            affected_consumers,
            risk_score,
            summary,
        }
    }

    /// Calculate risk score (0.0 - 1.0)
    fn calculate_risk_score(files: &FilesDiff, graph: &GraphDiff, exports: &ExportsDiff) -> f64 {
        let mut score = 0.0;

        // File changes
        score += files.removed.len() as f64 * 0.1;
        score += files.modified.len() as f64 * 0.05;

        // Graph changes
        score += graph.edges_removed.len() as f64 * 0.05;

        // Export changes
        score += exports.removed.len() as f64 * 0.15;

        // Clamp to 0.0 - 1.0
        score.min(1.0)
    }

    /// Generate human-readable summary
    fn generate_summary(
        files: &FilesDiff,
        graph: &GraphDiff,
        exports: &ExportsDiff,
        affected_consumers: &[PathBuf],
    ) -> String {
        let mut parts = Vec::new();

        if !files.added.is_empty() {
            parts.push(format!("{} files added", files.added.len()));
        }
        if !files.removed.is_empty() {
            parts.push(format!("{} files removed", files.removed.len()));
        }
        if !files.modified.is_empty() {
            parts.push(format!("{} files modified", files.modified.len()));
        }
        if !graph.edges_added.is_empty() {
            parts.push(format!("{} imports added", graph.edges_added.len()));
        }
        if !graph.edges_removed.is_empty() {
            parts.push(format!("{} imports removed", graph.edges_removed.len()));
        }
        if !exports.removed.is_empty() {
            parts.push(format!("{} exports removed", exports.removed.len()));
        }
        if !affected_consumers.is_empty() {
            parts.push(format!("{} consumers affected", affected_consumers.len()));
        }

        if parts.is_empty() {
            "No significant changes".to_string()
        } else {
            parts.join(", ")
        }
    }

    /// Convert to JSON value
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::GraphEdge;
    use crate::types::{ExportSymbol, FileAnalysis};

    #[test]
    fn test_files_diff_from_changed_files() {
        let changes = vec![
            ChangedFile {
                old_path: None,
                new_path: Some(PathBuf::from("new.ts")),
                status: ChangeStatus::Added,
            },
            ChangedFile {
                old_path: Some(PathBuf::from("old.ts")),
                new_path: None,
                status: ChangeStatus::Deleted,
            },
            ChangedFile {
                old_path: Some(PathBuf::from("mod.ts")),
                new_path: Some(PathBuf::from("mod.ts")),
                status: ChangeStatus::Modified,
            },
        ];

        let diff = FilesDiff::from_changed_files(&changes);

        assert_eq!(diff.added, vec![PathBuf::from("new.ts")]);
        assert_eq!(diff.removed, vec![PathBuf::from("old.ts")]);
        assert_eq!(diff.modified, vec![PathBuf::from("mod.ts")]);
        assert_eq!(diff.total_changes(), 3);
    }

    #[test]
    fn test_files_diff_renamed() {
        let changes = vec![ChangedFile {
            old_path: Some(PathBuf::from("old.ts")),
            new_path: Some(PathBuf::from("new.ts")),
            status: ChangeStatus::Renamed,
        }];

        let diff = FilesDiff::from_changed_files(&changes);
        assert_eq!(diff.renamed.len(), 1);
        assert_eq!(diff.renamed[0].0, PathBuf::from("old.ts"));
        assert_eq!(diff.renamed[0].1, PathBuf::from("new.ts"));
    }

    #[test]
    fn test_files_diff_copied() {
        let changes = vec![ChangedFile {
            old_path: Some(PathBuf::from("src.ts")),
            new_path: Some(PathBuf::from("copy.ts")),
            status: ChangeStatus::Copied,
        }];

        let diff = FilesDiff::from_changed_files(&changes);
        assert_eq!(diff.renamed.len(), 1); // Copied uses same handling as renamed
    }

    #[test]
    fn test_files_diff_empty() {
        let changes: Vec<ChangedFile> = vec![];
        let diff = FilesDiff::from_changed_files(&changes);
        assert_eq!(diff.total_changes(), 0);
    }

    #[test]
    fn test_risk_score_clamped() {
        let files = FilesDiff {
            removed: (0..100)
                .map(|i| PathBuf::from(format!("file{}.ts", i)))
                .collect(),
            ..Default::default()
        };
        let graph = GraphDiff::default();
        let exports = ExportsDiff::default();

        let score = SnapshotDiff::calculate_risk_score(&files, &graph, &exports);
        assert!(score <= 1.0);
    }

    #[test]
    fn test_risk_score_components() {
        // Test modified files contribution
        let files = FilesDiff {
            modified: vec![PathBuf::from("a.ts")],
            ..Default::default()
        };
        let score1 = SnapshotDiff::calculate_risk_score(
            &files,
            &GraphDiff::default(),
            &ExportsDiff::default(),
        );
        assert!(score1 > 0.0);

        // Test removed exports contribution
        let exports = ExportsDiff {
            removed: vec![ExportedSymbol {
                file: PathBuf::from("a.ts"),
                name: "foo".to_string(),
                kind: "function".to_string(),
            }],
            ..Default::default()
        };
        let score2 = SnapshotDiff::calculate_risk_score(
            &FilesDiff::default(),
            &GraphDiff::default(),
            &exports,
        );
        assert!(score2 > 0.0);
    }

    #[test]
    fn test_generate_summary_empty() {
        let summary = SnapshotDiff::generate_summary(
            &FilesDiff::default(),
            &GraphDiff::default(),
            &ExportsDiff::default(),
            &[],
        );
        assert_eq!(summary, "No significant changes");
    }

    #[test]
    fn test_generate_summary_with_changes() {
        let files = FilesDiff {
            added: vec![PathBuf::from("new.ts")],
            removed: vec![PathBuf::from("old.ts")],
            modified: vec![PathBuf::from("mod.ts")],
            ..Default::default()
        };
        let summary = SnapshotDiff::generate_summary(
            &files,
            &GraphDiff::default(),
            &ExportsDiff::default(),
            &[],
        );
        assert!(summary.contains("1 files added"));
        assert!(summary.contains("1 files removed"));
        assert!(summary.contains("1 files modified"));
    }

    #[test]
    fn test_generate_summary_with_graph_changes() {
        let graph = GraphDiff {
            edges_added: vec![DiffEdge {
                from: PathBuf::from("a.ts"),
                to: PathBuf::from("b.ts"),
                symbols: vec![],
            }],
            edges_removed: vec![DiffEdge {
                from: PathBuf::from("c.ts"),
                to: PathBuf::from("d.ts"),
                symbols: vec![],
            }],
        };
        let summary = SnapshotDiff::generate_summary(
            &FilesDiff::default(),
            &graph,
            &ExportsDiff::default(),
            &[],
        );
        assert!(summary.contains("1 imports added"));
        assert!(summary.contains("1 imports removed"));
    }

    #[test]
    fn test_generate_summary_with_consumers() {
        let summary = SnapshotDiff::generate_summary(
            &FilesDiff::default(),
            &GraphDiff::default(),
            &ExportsDiff::default(),
            &[PathBuf::from("consumer.ts")],
        );
        assert!(summary.contains("1 consumers affected"));
    }

    fn mock_metadata() -> crate::snapshot::SnapshotMetadata {
        crate::snapshot::SnapshotMetadata {
            schema_version: crate::snapshot::SNAPSHOT_SCHEMA_VERSION.to_string(),
            generated_at: "2025-01-01T00:00:00Z".to_string(),
            roots: vec![".".to_string()],
            languages: std::collections::HashSet::new(),
            file_count: 0,
            total_loc: 0,
            scan_duration_ms: 0,
            resolver_config: None,
            manifest_summary: Vec::new(),
            entrypoints: Vec::new(),
            entrypoint_drift: crate::snapshot::EntrypointDriftSummary::default(),
            git_repo: None,
            git_owner_repo: None,
            git_branch: None,
            git_commit: None,
            git_scan_id: None,
        }
    }

    fn mock_snapshot_with_edges(edges: Vec<GraphEdge>) -> Snapshot {
        Snapshot {
            metadata: mock_metadata(),
            files: vec![],
            edges,
            export_index: std::collections::HashMap::new(),
            command_bridges: vec![],
            event_bridges: vec![],
            barrels: vec![],
            semantic_facts: None,
            symbol_graph: None,
        }
    }

    fn mock_snapshot_with_files(files: Vec<FileAnalysis>) -> Snapshot {
        Snapshot {
            metadata: mock_metadata(),
            files,
            edges: vec![],
            export_index: std::collections::HashMap::new(),
            command_bridges: vec![],
            event_bridges: vec![],
            barrels: vec![],
            semantic_facts: None,
            symbol_graph: None,
        }
    }

    #[test]
    fn test_extract_edges_empty() {
        let snapshot = mock_snapshot_with_edges(vec![]);
        let edges = SnapshotDiff::extract_edges(&snapshot);
        assert!(edges.is_empty());
    }

    #[test]
    fn test_extract_edges() {
        let snapshot = mock_snapshot_with_edges(vec![GraphEdge {
            from: "a.ts".to_string(),
            to: "b.ts".to_string(),
            label: "foo, bar".to_string(),
        }]);
        let edges = SnapshotDiff::extract_edges(&snapshot);
        assert_eq!(edges.len(), 1);
        let edge = edges.iter().next().unwrap();
        assert_eq!(edge.from, PathBuf::from("a.ts"));
        assert_eq!(edge.to, PathBuf::from("b.ts"));
        assert_eq!(edge.symbols, vec!["foo", "bar"]);
    }

    #[test]
    fn test_extract_exports() {
        let file = FileAnalysis {
            path: "utils.ts".to_string(),
            exports: vec![ExportSymbol::new(
                "helper".to_string(),
                "function",
                "named",
                Some(10),
            )],
            ..Default::default()
        };

        let snapshot = mock_snapshot_with_files(vec![file]);
        let exports = SnapshotDiff::extract_exports(&snapshot);
        assert_eq!(exports.len(), 1);
    }

    /// Artifact fence (w1-b): deleting public_dist/ must not flood the diff
    /// with "removed exports" from generated bundles.
    #[test]
    fn compare_fenced_cuts_generated_removed_exports() {
        let dist_file = FileAnalysis {
            path: "public_dist/loctree-landing-2401b7f4.js".to_string(),
            exports: vec![ExportSymbol::new(
                "wasm_bindgen".to_string(),
                "function",
                "named",
                Some(1),
            )],
            ..Default::default()
        };
        let product_file = FileAnalysis {
            path: "src/api.ts".to_string(),
            exports: vec![ExportSymbol::new(
                "fetchUser".to_string(),
                "function",
                "named",
                Some(3),
            )],
            ..Default::default()
        };

        let from = mock_snapshot_with_files(vec![dist_file, product_file]);
        let to = mock_snapshot_with_files(vec![]);

        // Default fence: generated export cut + counted, product export kept
        let diff = SnapshotDiff::compare_fenced(&from, &to, None, None, &[], false);
        assert!(
            !diff
                .exports
                .removed
                .iter()
                .any(|e| e.file.to_string_lossy().contains("public_dist")),
            "generated exports must be fenced out of removed (was: {:?})",
            diff.exports.removed
        );
        assert!(
            diff.exports.removed.iter().any(|e| e.name == "fetchUser"),
            "product removed export must survive the fence"
        );
        assert_eq!(diff.exports.artifact_excluded.generated, 1);
        assert_eq!(
            diff.exports.artifact_excluded.summary_line(),
            "excluded: generated(1)"
        );

        // Opt-out restores full truth
        let diff_all = SnapshotDiff::compare_fenced(&from, &to, None, None, &[], true);
        assert!(
            diff_all
                .exports
                .removed
                .iter()
                .any(|e| e.file.to_string_lossy().contains("public_dist")),
            "--include-artifacts must restore generated exports"
        );
        assert!(diff_all.exports.artifact_excluded.is_empty());
    }

    #[test]
    fn test_diff_edge_equality() {
        let edge1 = DiffEdge {
            from: PathBuf::from("a.ts"),
            to: PathBuf::from("b.ts"),
            symbols: vec!["foo".to_string()],
        };
        let edge2 = DiffEdge {
            from: PathBuf::from("a.ts"),
            to: PathBuf::from("b.ts"),
            symbols: vec!["foo".to_string()],
        };
        assert_eq!(edge1, edge2);

        let mut set = HashSet::new();
        set.insert(edge1.clone());
        assert!(set.contains(&edge2));
    }

    #[test]
    fn test_snapshot_diff_to_json() {
        let diff = SnapshotDiff {
            from_commit: None,
            to_commit: None,
            files: FilesDiff::default(),
            graph: GraphDiff::default(),
            exports: ExportsDiff::default(),
            impact: ImpactAnalysis {
                affected_files: 0,
                affected_consumers: vec![],
                risk_score: 0.0,
                summary: "No changes".to_string(),
            },
        };
        let json = diff.to_json();
        assert!(!json.is_null());
    }

    #[test]
    fn test_compare_graphs_added_edge() {
        let from = mock_snapshot_with_edges(vec![]);
        let to = mock_snapshot_with_edges(vec![GraphEdge {
            from: "a.ts".to_string(),
            to: "b.ts".to_string(),
            label: "".to_string(),
        }]);
        let diff = SnapshotDiff::compare_graphs(&from, &to);
        assert_eq!(diff.edges_added.len(), 1);
        assert!(diff.edges_removed.is_empty());
    }

    #[test]
    fn test_compare_graphs_removed_edge() {
        let from = mock_snapshot_with_edges(vec![GraphEdge {
            from: "a.ts".to_string(),
            to: "b.ts".to_string(),
            label: "".to_string(),
        }]);
        let to = mock_snapshot_with_edges(vec![]);
        let diff = SnapshotDiff::compare_graphs(&from, &to);
        assert!(diff.edges_added.is_empty());
        assert_eq!(diff.edges_removed.len(), 1);
    }

    #[test]
    fn test_compare_exports_added() {
        let from = mock_snapshot_with_files(vec![]);

        let file = FileAnalysis {
            path: "utils.ts".to_string(),
            exports: vec![ExportSymbol::new(
                "helper".to_string(),
                "function",
                "named",
                Some(10),
            )],
            ..Default::default()
        };
        let to = mock_snapshot_with_files(vec![file]);

        let diff = SnapshotDiff::compare_exports(&from, &to);
        assert_eq!(diff.added.len(), 1);
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn test_compare_exports_removed() {
        let file = FileAnalysis {
            path: "utils.ts".to_string(),
            exports: vec![ExportSymbol::new(
                "helper".to_string(),
                "function",
                "named",
                Some(10),
            )],
            ..Default::default()
        };
        let from = mock_snapshot_with_files(vec![file]);

        let to = mock_snapshot_with_files(vec![]);
        let diff = SnapshotDiff::compare_exports(&from, &to);
        assert!(diff.added.is_empty());
        assert_eq!(diff.removed.len(), 1);
    }

    #[test]
    fn test_generate_summary_with_exports_removed() {
        let exports = ExportsDiff {
            removed: vec![ExportedSymbol {
                file: PathBuf::from("a.ts"),
                name: "foo".to_string(),
                kind: "function".to_string(),
            }],
            ..Default::default()
        };
        let summary = SnapshotDiff::generate_summary(
            &FilesDiff::default(),
            &GraphDiff::default(),
            &exports,
            &[],
        );
        assert!(summary.contains("1 exports removed"));
    }

    #[test]
    fn test_full_compare() {
        let from = mock_snapshot_with_edges(vec![]);
        let to = mock_snapshot_with_edges(vec![GraphEdge {
            from: "a.ts".to_string(),
            to: "b.ts".to_string(),
            label: "foo".to_string(),
        }]);
        let changed_files = vec![ChangedFile {
            old_path: None,
            new_path: Some(PathBuf::from("a.ts")),
            status: ChangeStatus::Added,
        }];

        let diff = SnapshotDiff::compare(&from, &to, None, None, &changed_files);

        assert_eq!(diff.files.added.len(), 1);
        assert_eq!(diff.graph.edges_added.len(), 1);
        assert!(!diff.impact.summary.is_empty());
    }

    #[test]
    fn test_extract_edges_empty_label() {
        let snapshot = mock_snapshot_with_edges(vec![GraphEdge {
            from: "a.ts".to_string(),
            to: "b.ts".to_string(),
            label: "".to_string(),
        }]);
        let edges = SnapshotDiff::extract_edges(&snapshot);
        let edge = edges.iter().next().unwrap();
        assert!(edge.symbols.is_empty());
    }

    #[test]
    fn test_exported_symbol_equality() {
        let sym1 = ExportedSymbol {
            file: PathBuf::from("a.ts"),
            name: "foo".to_string(),
            kind: "function".to_string(),
        };
        let sym2 = ExportedSymbol {
            file: PathBuf::from("a.ts"),
            name: "foo".to_string(),
            kind: "function".to_string(),
        };
        assert_eq!(sym1, sym2);

        let mut set = HashSet::new();
        set.insert(sym1.clone());
        assert!(set.contains(&sym2));
    }
}
