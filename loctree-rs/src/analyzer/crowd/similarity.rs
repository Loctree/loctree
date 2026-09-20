//! Similarity scoring between files for crowd detection

use std::collections::{HashMap, HashSet};

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
pub fn build_import_sets(files: &[crate::types::FileAnalysis]) -> HashMap<String, HashSet<String>> {
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
pub fn count_importers(files: &[crate::types::FileAnalysis]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    for file in files {
        for imp in &file.imports {
            *counts.entry(imp.source.clone()).or_insert(0) += 1;
        }
    }

    counts
}

/// Count direct importers per file from graph edges: every textual in-edge
/// (imports and re-exports alike — a barrel directly couples to its target),
/// without following re-export chains. Companion of count_importers_transitive,
/// which additionally walks re-export chains upstream to real consumers.
pub fn count_importers_direct_edges(
    files: &[crate::types::FileAnalysis],
    edges: &[crate::snapshot::GraphEdge],
) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    for file in files {
        let count = count_direct_importers_for_file(&file.path, edges);
        counts.insert(file.path.clone(), count);
    }

    counts
}

/// Count direct importers for a single file: distinct edge sources whose
/// target matches the file, no re-export traversal.
fn count_direct_importers_for_file(file: &str, edges: &[crate::snapshot::GraphEdge]) -> usize {
    edges
        .iter()
        .filter(|edge| edge_targets_file(edge, file))
        .map(|edge| edge.from.as_str())
        .collect::<HashSet<_>>()
        .len()
}

/// Shared edge-target matching with barrel/index normalization, so the direct
/// and transitive counters always agree on whether an edge points at a file.
fn edge_targets_file(edge: &crate::snapshot::GraphEdge, current: &str) -> bool {
    let current_folder = current
        .strip_suffix("/index.ts")
        .or_else(|| current.strip_suffix("/index.tsx"))
        .or_else(|| current.strip_suffix("/index.js"));

    edge.to == current
        || edge.to.ends_with(&format!("/{}", current))
        || (current.contains('/') && edge.to.contains(current))
        || current_folder
            .map(|f| edge.to == f || edge.to.ends_with(f))
            .unwrap_or(false)
}

/// Count importers using transitive re-export chain tracking
/// This uses the snapshot edges to follow re-exports like index.ts barrels
pub fn count_importers_transitive(
    files: &[crate::types::FileAnalysis],
    edges: &[crate::snapshot::GraphEdge],
) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    // For each file, count how many actual importers it has (following re-export chains)
    for file in files {
        let count = count_transitive_importers_for_file(&file.path, edges);
        counts.insert(file.path.clone(), count);
    }

    counts
}

/// Count transitive importers for a single file using BFS on re-export edges
fn count_transitive_importers_for_file(file: &str, edges: &[crate::snapshot::GraphEdge]) -> usize {
    use std::collections::HashSet;

    let mut importers: HashSet<String> = HashSet::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut to_check: Vec<String> = vec![file.to_string()];

    // Also check for the file without index suffix
    let normalized = file
        .trim_end_matches("/index.ts")
        .trim_end_matches("/index.tsx")
        .trim_end_matches("/index.js");
    if normalized != file {
        to_check.push(normalized.to_string());
    }

    // BFS to follow re-export chains
    while let Some(current) = to_check.pop() {
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current.clone());

        // Also add the index.ts variant if this is a folder path
        if !current.ends_with(".ts") && !current.ends_with(".tsx") && !current.ends_with(".js") {
            let index_variants = [
                format!("{}/index.ts", current),
                format!("{}/index.tsx", current),
                format!("{}/index.js", current),
            ];
            for variant in index_variants {
                if !visited.contains(&variant) {
                    to_check.push(variant);
                }
            }
        }

        for edge in edges {
            if edge_targets_file(edge, &current) {
                if edge.label == "reexport" {
                    // Follow re-export chain
                    if !visited.contains(&edge.from) {
                        to_check.push(edge.from.clone());
                    }
                } else {
                    // Regular import - count this as an importer
                    importers.insert(edge.from.clone());
                }
            }
        }
    }

    importers.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::GraphEdge;

    fn edge(from: &str, to: &str, label: &str) -> GraphEdge {
        GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            label: label.to_string(),
        }
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
        assert_eq!(
            count_direct_importers_for_file("src/internal.rs", &edges),
            1
        );
        // Transitive: the barrel's real consumers surface, the barrel itself
        // is followed through and not counted.
        assert_eq!(
            count_transitive_importers_for_file("src/internal.rs", &edges),
            2
        );
    }

    #[test]
    fn direct_and_transitive_agree_without_reexports() {
        let edges = vec![
            edge("src/a.rs", "src/target.rs", "import"),
            edge("src/b.rs", "src/target.rs", "import"),
        ];

        assert_eq!(count_direct_importers_for_file("src/target.rs", &edges), 2);
        assert_eq!(
            count_transitive_importers_for_file("src/target.rs", &edges),
            2
        );
    }
}
