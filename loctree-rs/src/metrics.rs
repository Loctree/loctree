//! Canonical repository metrics derived from the snapshot graph.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::snapshot::Snapshot;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryMetrics {
    pub file_count: usize,
    pub edge_count: usize,
    pub total_loc: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncomingImportMetric {
    pub file: String,
    pub importers_direct: usize,
    pub import_edges: usize,
    pub loc: usize,
}

pub fn repository_metrics(snapshot: &Snapshot) -> RepositoryMetrics {
    RepositoryMetrics {
        file_count: snapshot.canonical_file_count(),
        edge_count: snapshot.edges.len(),
        total_loc: snapshot.files.iter().map(|file| file.loc).sum(),
    }
}

pub fn incoming_import_metrics(snapshot: &Snapshot) -> HashMap<String, IncomingImportMetric> {
    let mut metrics: HashMap<String, IncomingImportMetric> = snapshot
        .files
        .iter()
        .map(|file| {
            (
                file.path.clone(),
                IncomingImportMetric {
                    file: file.path.clone(),
                    importers_direct: 0,
                    import_edges: 0,
                    loc: file.loc,
                },
            )
        })
        .collect();
    let mut importers_by_file: HashMap<String, HashSet<String>> = HashMap::new();

    for edge in &snapshot.edges {
        let imported = snapshot.normalize_path(&edge.to);
        let importer = snapshot.normalize_path(&edge.from);
        metrics
            .entry(imported.clone())
            .or_insert_with(|| IncomingImportMetric {
                file: imported.clone(),
                importers_direct: 0,
                import_edges: 0,
                loc: 0,
            })
            .import_edges += 1;
        importers_by_file
            .entry(imported)
            .or_default()
            .insert(importer);
    }

    for (file, importers) in importers_by_file {
        if let Some(metric) = metrics.get_mut(&file) {
            metric.importers_direct = importers.len();
        }
    }

    metrics
}

pub fn importer_counts_direct(snapshot: &Snapshot) -> HashMap<String, usize> {
    incoming_import_metrics(snapshot)
        .into_iter()
        .map(|(file, metric)| (file, metric.importers_direct))
        .collect()
}

pub fn import_edge_counts(snapshot: &Snapshot) -> HashMap<String, usize> {
    incoming_import_metrics(snapshot)
        .into_iter()
        .map(|(file, metric)| (file, metric.import_edges))
        .collect()
}

pub fn top_hubs_by_importers_direct(
    snapshot: &Snapshot,
    limit: usize,
) -> Vec<IncomingImportMetric> {
    top_hubs_by_importers_direct_filtered(snapshot, limit, |_| true)
}

pub fn top_hubs_by_importers_direct_filtered<F>(
    snapshot: &Snapshot,
    limit: usize,
    mut include: F,
) -> Vec<IncomingImportMetric>
where
    F: FnMut(&IncomingImportMetric) -> bool,
{
    let mut ranked: Vec<IncomingImportMetric> = incoming_import_metrics(snapshot)
        .into_values()
        .filter(|metric| metric.importers_direct > 0)
        .filter(|metric| include(metric))
        .collect();
    ranked.sort_by(|a, b| {
        b.importers_direct
            .cmp(&a.importers_direct)
            .then_with(|| b.import_edges.cmp(&a.import_edges))
            .then_with(|| a.file.cmp(&b.file))
    });
    ranked.truncate(limit);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::GraphEdge;
    use crate::types::FileAnalysis;

    fn fixture_snapshot() -> Snapshot {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        for name in ["hub.rs", "consumer_a.rs", "consumer_b.rs"] {
            snapshot.files.push(FileAnalysis::new(name.to_string()));
        }
        for symbol in ["A", "B"] {
            snapshot.edges.push(GraphEdge {
                from: "consumer_a.rs".to_string(),
                to: "hub.rs".to_string(),
                label: symbol.to_string(),
            });
        }
        snapshot.edges.push(GraphEdge {
            from: "consumer_b.rs".to_string(),
            to: "hub.rs".to_string(),
            label: "C".to_string(),
        });
        snapshot
    }

    #[test]
    fn importer_metrics_separate_unique_importers_from_raw_edges() {
        let snapshot = fixture_snapshot();
        let metrics = incoming_import_metrics(&snapshot);
        let hub = metrics.get("hub.rs").expect("hub metric");

        assert_eq!(hub.importers_direct, 2);
        assert_eq!(hub.import_edges, 3);
    }

    #[test]
    fn top_hubs_rank_by_unique_importers_before_raw_edges() {
        let snapshot = fixture_snapshot();
        let hubs = top_hubs_by_importers_direct(&snapshot, 1);

        assert_eq!(hubs[0].file, "hub.rs");
        assert_eq!(hubs[0].importers_direct, 2);
        assert_eq!(hubs[0].import_edges, 3);
    }
}
