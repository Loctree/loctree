//! Read-only query helpers over [`SymbolGraph`].
//!
//! Wave A shipped simple, correct, linear-scan implementations — enough to
//! back `find --where-symbol`, `slice`, and `impact` once Wave B populated
//! the graph. Wave C-1 makes `blast_radius` a transitive closure over a
//! reverse-edge adjacency map and adds the file-level consumer projection
//! that `slice` composes beside import consumers.

use super::{SymbolEdge, SymbolEdgeKind, SymbolGraph, SymbolId, SymbolNode, SymbolOccurrence};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

/// Edge kinds that propagate blast radius — a change to the target symbol can
/// affect the source of any of these. Mirrors the `impact symbol` traversal
/// set from the C-family research (§4.4).
const BLAST_RADIUS_EDGES: &[SymbolEdgeKind] = &[
    SymbolEdgeKind::References,
    SymbolEdgeKind::Calls,
    SymbolEdgeKind::Overrides,
    SymbolEdgeKind::Conforms,
    SymbolEdgeKind::Implements,
    SymbolEdgeKind::Inherits,
    SymbolEdgeKind::Instantiates,
    SymbolEdgeKind::SelectorMessage,
    SymbolEdgeKind::NotificationObserve,
];

impl SymbolGraph {
    /// All symbols whose `name` matches exactly. Backs `find --where-symbol`.
    pub fn lookup(&self, name: &str) -> Vec<&SymbolNode> {
        self.symbols.iter().filter(|n| n.name == name).collect()
    }

    /// Resolve a node by its stable id.
    pub fn node(&self, id: &SymbolId) -> Option<&SymbolNode> {
        self.symbols.iter().find(|n| &n.id == id)
    }

    /// All occurrences (use sites) of a symbol.
    pub fn references(&self, id: &SymbolId) -> Vec<&SymbolOccurrence> {
        self.occurrences
            .iter()
            .filter(|o| &o.symbol_id == id)
            .collect()
    }

    /// Edges that *call* the given symbol (`Calls` edges targeting `id`).
    pub fn callers(&self, id: &SymbolId) -> Vec<&SymbolEdge> {
        self.edges
            .iter()
            .filter(|e| e.kind == SymbolEdgeKind::Calls && &e.to == id)
            .collect()
    }

    /// Transitive blast radius: the ids of symbols that depend on `id` —
    /// directly or through a chain — via any [`BLAST_RADIUS_EDGES`] relation.
    /// BFS over a reverse-edge adjacency map; result order is breadth-first
    /// (direct dependents first), deterministic for a given edge order.
    pub fn blast_radius(&self, id: &SymbolId) -> Vec<SymbolId> {
        let mut reverse: HashMap<&SymbolId, Vec<&SymbolId>> = HashMap::new();
        for edge in &self.edges {
            if BLAST_RADIUS_EDGES.contains(&edge.kind) {
                reverse.entry(&edge.to).or_default().push(&edge.from);
            }
        }

        let mut out: Vec<SymbolId> = Vec::new();
        let mut seen: HashSet<&SymbolId> = HashSet::new();
        seen.insert(id);
        let mut queue: VecDeque<&SymbolId> = VecDeque::new();
        queue.push_back(id);
        while let Some(current) = queue.pop_front() {
            for &dependent in reverse.get(current).into_iter().flatten() {
                if seen.insert(dependent) {
                    out.push(dependent.clone());
                    queue.push_back(dependent);
                }
            }
        }
        out
    }

    /// Files that *use* symbols defined in `file`, with the names they touch.
    /// Backed by post-resolution occurrences (Wave C-1), so a Swift file gets
    /// symbol consumers even when its import consumers are zero. Sorted by
    /// consumer path; names sorted and deduplicated per consumer.
    pub fn file_symbol_consumers(&self, file: &Path) -> Vec<(String, Vec<String>)> {
        let defined: HashMap<&SymbolId, &str> = self
            .symbols
            .iter()
            .filter(|n| n.file.as_deref() == Some(file))
            .map(|n| (&n.id, n.name.as_str()))
            .collect();
        if defined.is_empty() {
            return Vec::new();
        }

        let mut by_consumer: HashMap<String, Vec<String>> = HashMap::new();
        for occ in &self.occurrences {
            if occ.file == file {
                continue;
            }
            if let Some(name) = defined.get(&occ.symbol_id) {
                let names = by_consumer
                    .entry(occ.file.display().to_string())
                    .or_default();
                if !names.contains(&name.to_string()) {
                    names.push(name.to_string());
                }
            }
        }

        let mut out: Vec<(String, Vec<String>)> = by_consumer.into_iter().collect();
        for (_, names) in &mut out {
            names.sort();
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}
