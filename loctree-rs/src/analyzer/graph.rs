use std::collections::{HashMap, HashSet};

use super::coverage::CommandUsage;
use super::report::{GraphComponent, GraphData, GraphNode};
use crate::types::FileAnalysis;

pub const MAX_GRAPH_NODES: usize = 8000;
pub const MAX_GRAPH_EDGES: usize = 12000;

fn layout_positions(comps: &[Vec<String>]) -> HashMap<String, (f32, f32)> {
    let cols = (comps.len() as f32).sqrt().ceil() as usize + 1;
    let spacing = 1200f32;
    let mut positions: HashMap<String, (f32, f32)> = HashMap::new();
    for (idx, comp) in comps.iter().enumerate() {
        let row = idx / cols;
        let col = idx % cols;
        let cx = (col as f32) * spacing;
        let cy = (row as f32) * spacing;
        let n = comp.len().max(1) as f32;
        let radius = 160.0 + 30.0 * n.sqrt();
        for (i, node) in comp.iter().enumerate() {
            let theta = (i as f32) * (std::f32::consts::TAU / n);
            let jitter = 12.0 * (i as f32 % 3.0) - 12.0;
            let x = cx + radius * theta.cos() + jitter;
            let y = cy + radius * theta.sin() - jitter;
            positions.insert(node.clone(), (x, y));
        }
    }
    positions
}

/// Number of edges touching each node, over the untruncated edge list.
/// Empty endpoints (unresolved imports) are skipped, matching `compute_components`.
fn incident_degrees(edges: &[(String, String, String)]) -> HashMap<String, usize> {
    let mut degrees: HashMap<String, usize> = HashMap::new();
    for (a, b, _) in edges {
        if a.is_empty() || b.is_empty() {
            continue;
        }
        *degrees.entry(a.clone()).or_insert(0) += 1;
        *degrees.entry(b.clone()).or_insert(0) += 1;
    }
    degrees
}

type ComponentResult = (
    Vec<Vec<String>>,
    HashMap<String, usize>,
    HashMap<String, usize>,
);

fn compute_components(nodes: &[String], edges: &[(String, String, String)]) -> ComponentResult {
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for n in nodes {
        adj.entry(n.clone()).or_default();
    }
    for (a, b, _) in edges {
        if a.is_empty() || b.is_empty() {
            continue;
        }
        let entry = adj.entry(a.clone()).or_default();
        if !entry.contains(b) {
            entry.push(b.clone());
        }
        let back = adj.entry(b.clone()).or_default();
        if !back.contains(a) {
            back.push(a.clone());
        }
    }

    let degrees: HashMap<String, usize> = adj.iter().map(|(k, v)| (k.clone(), v.len())).collect();

    let mut visited: HashSet<String> = HashSet::new();
    let mut comps: Vec<Vec<String>> = Vec::new();
    for n in nodes {
        if visited.contains(n) {
            continue;
        }
        let mut stack = vec![n.clone()];
        let mut comp = Vec::new();
        visited.insert(n.clone());
        while let Some(cur) = stack.pop() {
            comp.push(cur.clone());
            if let Some(neigh) = adj.get(&cur) {
                for nb in neigh {
                    if visited.insert(nb.clone()) {
                        stack.push(nb.clone());
                    }
                }
            }
        }
        comps.push(comp);
    }

    comps.sort_by(|a, b| {
        b.len().cmp(&a.len()).then(
            a.first()
                .unwrap_or(&String::new())
                .cmp(b.first().unwrap_or(&String::new())),
        )
    });

    let mut node_to_component: HashMap<String, usize> = HashMap::new();
    for (idx, comp) in comps.iter().enumerate() {
        let cid = idx + 1;
        for node in comp {
            node_to_component.insert(node.clone(), cid);
        }
    }

    (comps, node_to_component, degrees)
}

pub fn build_graph_data(
    analyses: &[FileAnalysis],
    graph_edges: &[(String, String, String)],
    loc_map: &HashMap<String, usize>,
    fe_commands: &CommandUsage,
    be_commands: &CommandUsage,
    max_nodes: usize,
    max_edges: usize,
) -> (Option<GraphData>, Option<String>) {
    let mut nodes: HashSet<String> = analyses.iter().map(|a| a.path.clone()).collect();
    for (a, b, _) in graph_edges {
        if !a.is_empty() {
            nodes.insert(a.clone());
        }
        if !b.is_empty() {
            nodes.insert(b.clone());
        }
    }

    if nodes.is_empty() {
        return (None, None);
    }

    // Store original counts before truncation
    let total_nodes = nodes.len();
    let total_edges = graph_edges.len();
    let mut truncated = false;
    let mut truncation_reason = None;

    // Truncate to limits if exceeded (instead of discarding entire graph).
    // The cut keeps the most connected surface: nodes ranked by degree over the
    // FULL edge list, edges ranked by the degree of their endpoints. Ties break
    // on path so the same snapshot always yields the same graph.
    if total_nodes > max_nodes || total_edges > max_edges {
        truncated = true;
        truncation_reason = Some(format!(
            "Graph exceeds limits: {} nodes (limit: {}), {} edges (limit: {}); kept highest-degree nodes and edges",
            total_nodes, max_nodes, total_edges, max_edges
        ));
    }

    let mut nodes_vec: Vec<String> = nodes.into_iter().collect();
    nodes_vec.sort();

    let truncated_edges: Vec<(String, String, String)> = if truncated {
        let full_degrees = incident_degrees(graph_edges);
        let degree_of = |n: &String| full_degrees.get(n).copied().unwrap_or(0);

        if nodes_vec.len() > max_nodes {
            nodes_vec.sort_by(|a, b| degree_of(b).cmp(&degree_of(a)).then_with(|| a.cmp(b)));
            nodes_vec.truncate(max_nodes);
            nodes_vec.sort();
        }

        let node_set: HashSet<&str> = nodes_vec.iter().map(String::as_str).collect();
        let mut kept: Vec<(String, String, String)> = graph_edges
            .iter()
            .filter(|(a, b, _)| node_set.contains(a.as_str()) && node_set.contains(b.as_str()))
            .cloned()
            .collect();
        if kept.len() > max_edges {
            let weight = |e: &(String, String, String)| degree_of(&e.0) + degree_of(&e.1);
            kept.sort_by(|x, y| weight(y).cmp(&weight(x)).then_with(|| x.cmp(y)));
            kept.truncate(max_edges);
        }

        eprintln!(
            "Warning: Graph truncated: showing {} of {} nodes, {} of {} edges (highest-degree first)",
            nodes_vec.len(),
            total_nodes,
            kept.len(),
            total_edges
        );
        kept
    } else {
        graph_edges.to_vec()
    };

    let (component_nodes, node_to_component, degrees) =
        compute_components(&nodes_vec, &truncated_edges);
    let positions = layout_positions(&component_nodes);
    let main_component_id = if component_nodes.is_empty() { 0 } else { 1 };

    let mut component_meta: Vec<GraphComponent> = Vec::new();
    for (idx, comp_nodes) in component_nodes.iter().enumerate() {
        let mut sorted_nodes = comp_nodes.clone();
        sorted_nodes.sort();
        let cid = idx + 1;
        let comp_set: HashSet<String> = sorted_nodes.iter().cloned().collect();
        let edge_count = truncated_edges
            .iter()
            .filter(|(a, b, _)| comp_set.contains(a) && comp_set.contains(b))
            .count();
        let isolated_count = sorted_nodes
            .iter()
            .filter(|n| degrees.get(*n).cloned().unwrap_or(0) == 0)
            .count();
        let loc_sum: usize = sorted_nodes
            .iter()
            .map(|n| loc_map.get(n).cloned().unwrap_or(0))
            .sum();
        let sample = sorted_nodes.first().cloned().unwrap_or_default();

        let tauri_frontend = fe_commands
            .values()
            .flat_map(|locs| locs.iter())
            .filter(|(path, _, _)| comp_set.contains(path))
            .count();
        let tauri_backend = be_commands
            .values()
            .flat_map(|locs| locs.iter())
            .filter(|(path, _, _)| comp_set.contains(path))
            .count();
        let detached = main_component_id != 0 && cid != main_component_id;

        component_meta.push(GraphComponent {
            id: cid,
            size: sorted_nodes.len(),
            edge_count,
            nodes: sorted_nodes,
            isolated_count,
            sample,
            loc_sum,
            detached,
            tauri_frontend,
            tauri_backend,
        });
    }

    let graph_nodes: Vec<GraphNode> = nodes_vec
        .iter()
        .filter_map(|id| {
            if id.is_empty() {
                return None;
            }
            let (x, y) = positions.get(id).cloned().unwrap_or((0.0, 0.0));
            let loc = loc_map.get(id).cloned().unwrap_or(0);
            let label = id.rsplit('/').next().unwrap_or(id.as_str()).to_string();
            let component = *node_to_component.get(id).unwrap_or(&0);
            let degree = *degrees.get(id).unwrap_or(&0);
            let detached = main_component_id != 0 && component != main_component_id;
            Some(GraphNode {
                id: id.clone(),
                label,
                loc,
                x,
                y,
                component,
                degree,
                detached,
            })
        })
        .collect();

    (
        Some(GraphData {
            nodes: graph_nodes,
            edges: truncated_edges,
            components: component_meta,
            main_component_id,
            truncated,
            total_nodes,
            total_edges,
            truncation_reason,
        }),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FileAnalysis;

    fn mock_file(path: &str, loc: usize) -> FileAnalysis {
        FileAnalysis {
            path: path.to_string(),
            loc,
            ..Default::default()
        }
    }

    #[test]
    fn test_layout_positions_empty() {
        let comps: Vec<Vec<String>> = vec![];
        let positions = layout_positions(&comps);
        assert!(positions.is_empty());
    }

    #[test]
    fn test_layout_positions_single_component() {
        let comps = vec![vec!["a.ts".to_string(), "b.ts".to_string()]];
        let positions = layout_positions(&comps);

        assert_eq!(positions.len(), 2);
        assert!(positions.contains_key("a.ts"));
        assert!(positions.contains_key("b.ts"));
    }

    #[test]
    fn test_layout_positions_multiple_components() {
        let comps = vec![
            vec!["a.ts".to_string()],
            vec!["b.ts".to_string()],
            vec!["c.ts".to_string()],
        ];
        let positions = layout_positions(&comps);

        assert_eq!(positions.len(), 3);
        // Components should have different positions
        let (ax, ay) = positions["a.ts"];
        let (bx, by) = positions["b.ts"];
        let (cx, cy) = positions["c.ts"];

        // They shouldn't all be at exactly the same spot
        assert!(ax != bx || ay != by || bx != cx || by != cy);
    }

    #[test]
    fn test_compute_components_empty() {
        let nodes: Vec<String> = vec![];
        let edges: Vec<(String, String, String)> = vec![];

        let (comps, node_map, degrees) = compute_components(&nodes, &edges);
        assert!(comps.is_empty());
        assert!(node_map.is_empty());
        assert!(degrees.is_empty());
    }

    #[test]
    fn test_compute_components_single_node() {
        let nodes = vec!["a.ts".to_string()];
        let edges: Vec<(String, String, String)> = vec![];

        let (comps, node_map, degrees) = compute_components(&nodes, &edges);

        assert_eq!(comps.len(), 1);
        assert_eq!(comps[0], vec!["a.ts"]);
        assert_eq!(*node_map.get("a.ts").unwrap(), 1);
        assert_eq!(*degrees.get("a.ts").unwrap(), 0);
    }

    #[test]
    fn test_compute_components_connected_pair() {
        let nodes = vec!["a.ts".to_string(), "b.ts".to_string()];
        let edges = vec![("a.ts".to_string(), "b.ts".to_string(), "import".to_string())];

        let (comps, node_map, degrees) = compute_components(&nodes, &edges);

        assert_eq!(comps.len(), 1);
        assert_eq!(comps[0].len(), 2);
        // Both nodes in same component
        assert_eq!(node_map.get("a.ts"), node_map.get("b.ts"));
        assert_eq!(*degrees.get("a.ts").unwrap(), 1);
        assert_eq!(*degrees.get("b.ts").unwrap(), 1);
    }

    #[test]
    fn test_compute_components_disconnected() {
        let nodes = vec!["a.ts".to_string(), "b.ts".to_string()];
        let edges: Vec<(String, String, String)> = vec![];

        let (comps, node_map, _) = compute_components(&nodes, &edges);

        assert_eq!(comps.len(), 2);
        // Different components
        assert_ne!(node_map.get("a.ts"), node_map.get("b.ts"));
    }

    #[test]
    fn test_compute_components_ignores_empty_edges() {
        let nodes = vec!["a.ts".to_string()];
        let edges = vec![
            ("".to_string(), "a.ts".to_string(), "import".to_string()),
            ("a.ts".to_string(), "".to_string(), "import".to_string()),
        ];

        let (comps, _, degrees) = compute_components(&nodes, &edges);
        assert_eq!(comps.len(), 1);
        // Empty node connections should be ignored
        assert_eq!(*degrees.get("a.ts").unwrap(), 0);
    }

    #[test]
    fn test_build_graph_data_empty() {
        let analyses: Vec<FileAnalysis> = vec![];
        let edges: Vec<(String, String, String)> = vec![];
        let loc_map: HashMap<String, usize> = HashMap::new();
        let fe_commands: CommandUsage = HashMap::new();
        let be_commands: CommandUsage = HashMap::new();

        let (graph, warning) = build_graph_data(
            &analyses,
            &edges,
            &loc_map,
            &fe_commands,
            &be_commands,
            MAX_GRAPH_NODES,
            MAX_GRAPH_EDGES,
        );

        assert!(graph.is_none());
        assert!(warning.is_none());
    }

    #[test]
    fn test_build_graph_data_simple() {
        let analyses = vec![mock_file("a.ts", 100), mock_file("b.ts", 50)];
        let edges = vec![("a.ts".to_string(), "b.ts".to_string(), "import".to_string())];
        let mut loc_map = HashMap::new();
        loc_map.insert("a.ts".to_string(), 100);
        loc_map.insert("b.ts".to_string(), 50);
        let fe_commands: CommandUsage = HashMap::new();
        let be_commands: CommandUsage = HashMap::new();

        let (graph, warning) = build_graph_data(
            &analyses,
            &edges,
            &loc_map,
            &fe_commands,
            &be_commands,
            MAX_GRAPH_NODES,
            MAX_GRAPH_EDGES,
        );

        assert!(graph.is_some());
        assert!(warning.is_none());

        let g = graph.unwrap();
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.components.len(), 1);
        assert_eq!(g.main_component_id, 1);
        // Should not be truncated
        assert!(!g.truncated);
        assert_eq!(g.total_nodes, 2);
        assert_eq!(g.total_edges, 1);
        assert!(g.truncation_reason.is_none());
    }

    #[test]
    fn test_build_graph_data_exceeds_limits() {
        let analyses: Vec<FileAnalysis> = (0..100)
            .map(|i| mock_file(&format!("file{}.ts", i), 10))
            .collect();
        let edges: Vec<(String, String, String)> = vec![];
        let loc_map: HashMap<String, usize> = HashMap::new();
        let fe_commands: CommandUsage = HashMap::new();
        let be_commands: CommandUsage = HashMap::new();

        // Set very low limits
        let (graph, warning) = build_graph_data(
            &analyses,
            &edges,
            &loc_map,
            &fe_commands,
            &be_commands,
            10, // max 10 nodes
            100,
        );

        // Should return truncated graph, not None
        assert!(graph.is_some());
        assert!(warning.is_none());

        let g = graph.unwrap();
        assert!(g.truncated);
        assert_eq!(g.total_nodes, 100);
        assert_eq!(g.nodes.len(), 10); // Truncated to max_nodes
        assert!(g.truncation_reason.is_some());
        assert!(g.truncation_reason.unwrap().contains("exceeds limits"));
    }

    #[test]
    fn test_build_graph_data_with_detached_components() {
        let analyses = vec![
            mock_file("a.ts", 100),
            mock_file("b.ts", 50),
            mock_file("c.ts", 30),
            mock_file("d.ts", 20),
        ];
        // a-b connected, c-d connected, but separate
        let edges = vec![
            ("a.ts".to_string(), "b.ts".to_string(), "import".to_string()),
            ("c.ts".to_string(), "d.ts".to_string(), "import".to_string()),
        ];
        let loc_map: HashMap<String, usize> = HashMap::new();
        let fe_commands: CommandUsage = HashMap::new();
        let be_commands: CommandUsage = HashMap::new();

        let (graph, _) = build_graph_data(
            &analyses,
            &edges,
            &loc_map,
            &fe_commands,
            &be_commands,
            MAX_GRAPH_NODES,
            MAX_GRAPH_EDGES,
        );

        let g = graph.unwrap();
        assert_eq!(g.components.len(), 2);

        // One component should be detached (not main)
        let detached_count = g.components.iter().filter(|c| c.detached).count();
        assert_eq!(detached_count, 1);
    }

    #[test]
    fn test_build_graph_data_edge_truncation() {
        // Create 5 nodes with 10 edges
        let analyses = vec![
            mock_file("a.ts", 10),
            mock_file("b.ts", 10),
            mock_file("c.ts", 10),
            mock_file("d.ts", 10),
            mock_file("e.ts", 10),
        ];
        let edges = vec![
            ("a.ts".to_string(), "b.ts".to_string(), "import".to_string()),
            ("b.ts".to_string(), "c.ts".to_string(), "import".to_string()),
            ("c.ts".to_string(), "d.ts".to_string(), "import".to_string()),
            ("d.ts".to_string(), "e.ts".to_string(), "import".to_string()),
            ("e.ts".to_string(), "a.ts".to_string(), "import".to_string()),
            ("a.ts".to_string(), "c.ts".to_string(), "import".to_string()),
            ("b.ts".to_string(), "d.ts".to_string(), "import".to_string()),
            ("c.ts".to_string(), "e.ts".to_string(), "import".to_string()),
            ("d.ts".to_string(), "a.ts".to_string(), "import".to_string()),
            ("e.ts".to_string(), "b.ts".to_string(), "import".to_string()),
        ];
        let loc_map: HashMap<String, usize> = HashMap::new();
        let fe_commands: CommandUsage = HashMap::new();
        let be_commands: CommandUsage = HashMap::new();

        // Limit edges to 3
        let (graph, warning) = build_graph_data(
            &analyses,
            &edges,
            &loc_map,
            &fe_commands,
            &be_commands,
            100, // Allow all nodes
            3,   // Limit edges
        );

        assert!(graph.is_some());
        assert!(warning.is_none());

        let g = graph.unwrap();
        assert!(g.truncated);
        assert_eq!(g.total_edges, 10);
        assert!(g.edges.len() <= 3); // Should be truncated to max 3
        assert!(g.truncation_reason.is_some());
    }

    #[test]
    fn test_graph_node_labels() {
        let analyses = vec![mock_file("src/components/Button.tsx", 100)];
        let edges: Vec<(String, String, String)> = vec![];
        let mut loc_map = HashMap::new();
        loc_map.insert("src/components/Button.tsx".to_string(), 100);
        let fe_commands: CommandUsage = HashMap::new();
        let be_commands: CommandUsage = HashMap::new();

        let (graph, _) = build_graph_data(
            &analyses,
            &edges,
            &loc_map,
            &fe_commands,
            &be_commands,
            MAX_GRAPH_NODES,
            MAX_GRAPH_EDGES,
        );

        let g = graph.unwrap();
        let node = &g.nodes[0];
        // Label should be just filename, not full path
        assert_eq!(node.label, "Button.tsx");
        assert_eq!(node.id, "src/components/Button.tsx");
    }

    #[test]
    fn test_node_truncation_keeps_highest_degree_not_alphabetical() {
        // 20 isolated files that sort FIRST alphabetically, plus a hub that sorts LAST
        // with five dependants. An alphabetical cut would keep the isolated files
        // and drop the hub; a degree cut must do the opposite.
        let mut analyses: Vec<FileAnalysis> = (0..20)
            .map(|i| mock_file(&format!("a/isolated{i:02}.ts"), 10))
            .collect();
        analyses.push(mock_file("z/hub.ts", 500));
        let leaves: Vec<String> = (1..=5).map(|i| format!("y/leaf{i}.ts")).collect();
        for leaf in &leaves {
            analyses.push(mock_file(leaf, 20));
        }
        let edges: Vec<(String, String, String)> = leaves
            .iter()
            .map(|leaf| (leaf.clone(), "z/hub.ts".to_string(), "import".to_string()))
            .collect();
        let loc_map: HashMap<String, usize> = HashMap::new();
        let fe_commands: CommandUsage = HashMap::new();
        let be_commands: CommandUsage = HashMap::new();

        let (graph, _) = build_graph_data(
            &analyses,
            &edges,
            &loc_map,
            &fe_commands,
            &be_commands,
            6, // hub + five leaves exactly
            MAX_GRAPH_EDGES,
        );

        let g = graph.unwrap();
        assert!(g.truncated);
        assert_eq!(g.total_nodes, 26);
        let kept: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            kept,
            vec![
                "y/leaf1.ts",
                "y/leaf2.ts",
                "y/leaf3.ts",
                "y/leaf4.ts",
                "y/leaf5.ts",
                "z/hub.ts",
            ]
        );
        // Every edge lives between kept nodes, so none is lost.
        assert_eq!(g.edges.len(), 5);
        assert!(g.truncation_reason.unwrap().contains("highest-degree"));
    }

    #[test]
    fn test_edge_truncation_prefers_hub_edges() {
        // A hub with four dependants (edge weight 5) and two disconnected pairs
        // (edge weight 2). Input order puts the pairs first so a positional
        // `.take()` would keep them; the degree cut must keep the hub edges.
        let analyses = vec![
            mock_file("p1.ts", 1),
            mock_file("p2.ts", 1),
            mock_file("p3.ts", 1),
            mock_file("p4.ts", 1),
            mock_file("hub.ts", 1),
            mock_file("h1.ts", 1),
            mock_file("h2.ts", 1),
            mock_file("h3.ts", 1),
            mock_file("h4.ts", 1),
        ];
        let e = |a: &str, b: &str| (a.to_string(), b.to_string(), "import".to_string());
        let edges = vec![
            e("p1.ts", "p2.ts"),
            e("p3.ts", "p4.ts"),
            e("h4.ts", "hub.ts"),
            e("h3.ts", "hub.ts"),
            e("h2.ts", "hub.ts"),
            e("h1.ts", "hub.ts"),
        ];
        let loc_map: HashMap<String, usize> = HashMap::new();
        let fe_commands: CommandUsage = HashMap::new();
        let be_commands: CommandUsage = HashMap::new();

        let (graph, _) = build_graph_data(
            &analyses,
            &edges,
            &loc_map,
            &fe_commands,
            &be_commands,
            100,
            4,
        );

        let g = graph.unwrap();
        assert!(g.truncated);
        assert_eq!(g.total_edges, 6);
        let kept: Vec<(&str, &str)> = g
            .edges
            .iter()
            .map(|(a, b, _)| (a.as_str(), b.as_str()))
            .collect();
        assert_eq!(
            kept,
            vec![
                ("h1.ts", "hub.ts"),
                ("h2.ts", "hub.ts"),
                ("h3.ts", "hub.ts"),
                ("h4.ts", "hub.ts"),
            ]
        );
    }
}
