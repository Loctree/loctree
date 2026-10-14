//! Similarity scoring between files for crowd detection

use std::collections::{HashMap, HashSet};

use crate::snapshot::GraphEdge;
use crate::types::FileAnalysis;

/// Calculate Jaccard similarity between two sets of imports
pub fn jaccard_similarity(imports_a: &HashSet<String>, imports_b: &HashSet<String>) -> f32 {
    if imports_a.is_empty() && imports_b.is_empty() {
        return 0.0;
    }

    let intersection = imports_a.intersection(imports_b).count();
    let union = imports_a.union(imports_b).count();

    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

/// Build import sets for each file
pub fn build_import_sets(files: &[FileAnalysis]) -> HashMap<String, HashSet<String>> {
    let mut result = HashMap::new();

    for file in files {
        let imports: HashSet<String> = file.imports.iter().map(|imp| imp.source.clone()).collect();
        result.insert(file.path.clone(), imports);
    }

    result
}

/// Calculate similarity matrix for a set of files
pub fn similarity_matrix(
    file_paths: &[String],
    import_sets: &HashMap<String, HashSet<String>>,
) -> Vec<(String, String, f32)> {
    let mut similarities = Vec::new();

    for (i, path_a) in file_paths.iter().enumerate() {
        for path_b in file_paths.iter().skip(i + 1) {
            let set_a = import_sets.get(path_a).cloned().unwrap_or_default();
            let set_b = import_sets.get(path_b).cloned().unwrap_or_default();

            let sim = jaccard_similarity(&set_a, &set_b);
            if sim > 0.3 {
                // Only include significant similarities
                similarities.push((path_a.clone(), path_b.clone(), sim));
            }
        }
    }

    similarities.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    similarities
}

/// Count how many files import each file (popularity) - simple direct counting
/// Note: This doesn't follow re-export chains. Use count_importers_transitive for that.
pub fn count_importers(files: &[FileAnalysis]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    for file in files {
        for imp in &file.imports {
            *counts.entry(imp.source.clone()).or_insert(0) += 1;
        }
    }

    counts
}

const BARREL_INDEX_NAMES: [&str; 3] = ["index.ts", "index.tsx", "index.js"];

/// Graph edges keyed by every slash-boundary suffix of their target, so
/// "which edges point at this path?" is a hash probe instead of a scan over
/// every edge. Build it once per crowd run and share it between files and
/// patterns: on a 20k-edge repo the former per-file scan (with a string
/// allocation per edge) cost minutes after the walk itself took seconds.
pub struct EdgeIndex<'a> {
    by_target_suffix: HashMap<&'a str, Vec<&'a GraphEdge>>,
}

impl<'a> EdgeIndex<'a> {
    pub fn new(edges: &'a [GraphEdge]) -> Self {
        let mut by_target_suffix: HashMap<&'a str, Vec<&'a GraphEdge>> = HashMap::new();
        for edge in edges {
            let to = edge.to.as_str();
            if to.is_empty() {
                continue;
            }
            by_target_suffix.entry(to).or_default().push(edge);
            for (idx, ch) in to.char_indices() {
                if ch == '/' {
                    let suffix = &to[idx + 1..];
                    if !suffix.is_empty() {
                        by_target_suffix.entry(suffix).or_default().push(edge);
                    }
                }
            }
        }
        Self { by_target_suffix }
    }

    /// Edges whose target is exactly `path` or ends with `/path`.
    fn targeting(&self, path: &str) -> &[&'a GraphEdge] {
        self.by_target_suffix
            .get(path)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Edges pointing at `path` under barrel normalization: a folder and its
    /// `index.{ts,tsx,js}` name the same module, so an edge aimed at either
    /// spelling counts for both. Matching is slash-boundary exact: `foo.rs`
    /// is never a hit for `myfoo.rs`, and `src/a` never for `src/abc.ts`.
    fn targeting_module(&self, path: &str) -> Vec<&'a GraphEdge> {
        let mut out: Vec<&'a GraphEdge> = self.targeting(path).to_vec();

        if let Some(folder) = barrel_folder(path) {
            out.extend_from_slice(self.targeting(folder));
        }

        if is_folder_like(path) {
            for index_name in BARREL_INDEX_NAMES {
                let alias = format!("{path}/{index_name}");
                out.extend_from_slice(self.targeting(&alias));
            }
        }

        out
    }
}

/// `src/x/index.ts` → `src/x`; anything else → `None`.
fn barrel_folder(path: &str) -> Option<&str> {
    BARREL_INDEX_NAMES
        .iter()
        .find_map(|index_name| path.strip_suffix(index_name)?.strip_suffix('/'))
}

/// A path that is not itself a JS/TS module may be a folder whose barrel
/// `index.*` resolves the import.
fn is_folder_like(path: &str) -> bool {
    !(path.ends_with(".ts") || path.ends_with(".tsx") || path.ends_with(".js"))
}

/// Count direct importers per file from graph edges: every in-edge (imports
/// and re-exports alike — a barrel directly couples to its target), without
/// following re-export chains. Companion of count_importers_transitive,
/// which additionally walks re-export chains upstream to real consumers.
pub fn count_importers_direct_edges(
    files: &[FileAnalysis],
    edges: &[GraphEdge],
) -> HashMap<String, usize> {
    let index = EdgeIndex::new(edges);
    files
        .iter()
        .map(|file| {
            (
                file.path.clone(),
                count_direct_importers_for_file(&file.path, &index),
            )
        })
        .collect()
}

/// Distinct edge sources whose target matches the file, no re-export traversal.
fn count_direct_importers_for_file(file: &str, index: &EdgeIndex<'_>) -> usize {
    index
        .targeting_module(file)
        .iter()
        .map(|edge| edge.from.as_str())
        .collect::<HashSet<_>>()
        .len()
}

/// Count importers using transitive re-export chain tracking
/// This uses the snapshot edges to follow re-exports like index.ts barrels
pub fn count_importers_transitive(
    files: &[FileAnalysis],
    edges: &[GraphEdge],
) -> HashMap<String, usize> {
    let index = EdgeIndex::new(edges);
    files
        .iter()
        .map(|file| {
            (
                file.path.clone(),
                count_transitive_importers_for_file(&file.path, &index),
            )
        })
        .collect()
}

/// Transitive importers of one file: BFS upstream through `reexport` edges,
/// counting the distinct sources of every other edge met on the way.
fn count_transitive_importers_for_file<'a>(file: &'a str, index: &EdgeIndex<'a>) -> usize {
    let mut importers: HashSet<&'a str> = HashSet::new();
    let mut visited: HashSet<&'a str> = HashSet::new();
    let mut to_check: Vec<&'a str> = vec![file];

    while let Some(current) = to_check.pop() {
        if !visited.insert(current) {
            continue;
        }
        for edge in index.targeting_module(current) {
            if edge.label == "reexport" {
                // Follow the re-export chain to the real consumers.
                if !visited.contains(edge.from.as_str()) {
                    to_check.push(edge.from.as_str());
                }
            } else {
                importers.insert(edge.from.as_str());
            }
        }
    }

    importers.len()
}

/// Importer counts for every file, both views, computed once per crowd run.
/// They do not depend on the pattern, so one instance serves every crowd.
pub struct ImporterCounts {
    pub transitive: HashMap<String, usize>,
    pub direct: HashMap<String, usize>,
}

impl ImporterCounts {
    /// Graph-backed counts: one `EdgeIndex` shared by both views.
    pub fn from_edges(files: &[FileAnalysis], edges: &[GraphEdge]) -> Self {
        let index = EdgeIndex::new(edges);
        let mut transitive = HashMap::with_capacity(files.len());
        let mut direct = HashMap::with_capacity(files.len());
        for file in files {
            transitive.insert(
                file.path.clone(),
                count_transitive_importers_for_file(&file.path, &index),
            );
            direct.insert(
                file.path.clone(),
                count_direct_importers_for_file(&file.path, &index),
            );
        }
        Self { transitive, direct }
    }

    /// Textual counts when no graph is available: both views carry the same
    /// number, as before.
    pub fn from_imports(files: &[FileAnalysis]) -> Self {
        let direct = count_importers(files);
        Self {
            transitive: direct.clone(),
            direct,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: &str, to: &str, label: &str) -> GraphEdge {
        GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            label: label.to_string(),
        }
    }

    fn transitive(file: &str, edges: &[GraphEdge]) -> usize {
        count_transitive_importers_for_file(file, &EdgeIndex::new(edges))
    }

    fn direct(file: &str, edges: &[GraphEdge]) -> usize {
        count_direct_importers_for_file(file, &EdgeIndex::new(edges))
    }

    #[test]
    fn direct_and_transitive_counts_diverge_through_barrel() {
        // barrel.rs re-exports internal.rs; two consumers import the barrel.
        let edges = vec![
            edge("src/barrel.rs", "src/internal.rs", "reexport"),
            edge("src/consumer_a.rs", "src/barrel.rs", "import"),
            edge("src/consumer_b.rs", "src/barrel.rs", "import"),
        ];

        // Direct: only the barrel textually points at internal.rs.
        assert_eq!(direct("src/internal.rs", &edges), 1);
        // Transitive: the barrel's real consumers surface, the barrel itself
        // is followed through and not counted.
        assert_eq!(transitive("src/internal.rs", &edges), 2);
    }

    #[test]
    fn direct_and_transitive_agree_without_reexports() {
        let edges = vec![
            edge("src/a.rs", "src/target.rs", "import"),
            edge("src/b.rs", "src/target.rs", "import"),
        ];

        assert_eq!(direct("src/target.rs", &edges), 2);
        assert_eq!(transitive("src/target.rs", &edges), 2);
    }

    #[test]
    fn suffix_match_requires_a_slash_boundary() {
        // A longer spelling of the same path still counts; a different file
        // that merely ends with the same letters does not. The old scan
        // counted `src/myfoo.rs` for `foo.rs` through its folder-variant
        // `ends_with`, and `src/abc.ts` for `src/a` through `contains`.
        let edges = vec![
            edge("app/main.rs", "/abs/repo/src/foo.rs", "import"),
            edge("app/other.rs", "src/myfoo.rs", "import"),
            edge("app/third.ts", "src/abc.ts", "import"),
        ];

        assert_eq!(direct("src/foo.rs", &edges), 1);
        assert_eq!(transitive("src/foo.rs", &edges), 1);
        assert_eq!(direct("foo.rs", &edges), 1);
        assert_eq!(direct("src/a", &edges), 0);
        assert_eq!(transitive("src/a", &edges), 0);
    }

    #[test]
    fn folder_and_its_index_barrel_are_one_module() {
        // Edges may name the folder or its index file; both spellings of the
        // file resolve to the same importers, and a re-export chain through
        // the barrel is still followed.
        let edges = vec![
            edge("src/pages/home.tsx", "src/components", "import"),
            edge("src/pages/about.tsx", "src/components/index.ts", "import"),
            edge(
                "src/components/index.ts",
                "src/components/Button.tsx",
                "reexport",
            ),
        ];

        assert_eq!(direct("src/components", &edges), 2);
        assert_eq!(direct("src/components/index.ts", &edges), 2);
        assert_eq!(transitive("src/components/Button.tsx", &edges), 2);
    }

    #[test]
    fn importer_counts_match_per_file_counters() {
        let files = vec![
            FileAnalysis {
                path: "src/barrel.rs".to_string(),
                ..Default::default()
            },
            FileAnalysis {
                path: "src/internal.rs".to_string(),
                ..Default::default()
            },
        ];
        let edges = vec![
            edge("src/barrel.rs", "src/internal.rs", "reexport"),
            edge("src/consumer_a.rs", "src/barrel.rs", "import"),
            edge("src/consumer_b.rs", "src/barrel.rs", "import"),
        ];

        let counts = ImporterCounts::from_edges(&files, &edges);
        assert_eq!(counts.transitive["src/internal.rs"], 2);
        assert_eq!(counts.direct["src/internal.rs"], 1);
        assert_eq!(counts.transitive["src/barrel.rs"], 2);
        assert_eq!(counts.direct["src/barrel.rs"], 2);
        assert_eq!(
            counts.transitive,
            count_importers_transitive(&files, &edges)
        );
        assert_eq!(counts.direct, count_importers_direct_edges(&files, &edges));
    }
}
