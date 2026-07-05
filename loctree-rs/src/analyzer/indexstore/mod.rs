//! macOS-gated IndexStore importer for Swift / Objective-C deep mode.
//!
//! Loctree owns store detection, graph merge, and authority semantics here.
//! Raw Apple IndexStore records are read through a subprocess dump boundary so
//! default builds do not link libIndexStore or inherit Xcode dylib/runtime
//! contracts. Set `LOCTREE_INDEXSTORE_DUMP` to a command that accepts the store
//! path as argv[1] and emits the JSONL contract parsed by [`reader`].

mod reader;

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use crate::symbols::{
    Confidence, FileSymbolSummary, OccurrenceRole, SymbolEngineRun, SymbolGraph, SymbolId,
    SymbolProvenance,
};

const STORE_DUMP_ENV: &str = "LOCTREE_INDEXSTORE_DUMP";
const STORE_PATH_ENV: &str = "LOCTREE_INDEXSTORE_PATH";
const MAX_DETECT_DIRS: usize = 20_000;

#[derive(Debug, Clone)]
pub struct IndexStoreIngest {
    pub graph: SymbolGraph,
    pub stores: Vec<PathBuf>,
}

pub fn discover_stores(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut stores = Vec::new();
    if let Ok(value) = std::env::var(STORE_PATH_ENV) {
        stores.extend(std::env::split_paths(&value).filter(|path| path.exists()));
    }

    for root in roots {
        discover_store_dirs(root, &mut stores);
    }

    stores.sort();
    stores.dedup();
    stores
}

pub fn ingest_roots(roots: &[PathBuf]) -> io::Result<Option<IndexStoreIngest>> {
    let Some(command) = std::env::var_os(STORE_DUMP_ENV).map(PathBuf::from) else {
        return Ok(None);
    };
    ingest_roots_with_dump_command(roots, &command)
}

pub fn ingest_roots_with_dump_command(
    roots: &[PathBuf],
    dump_command: &Path,
) -> io::Result<Option<IndexStoreIngest>> {
    let stores = discover_stores(roots);
    if stores.is_empty() {
        return Ok(None);
    }

    let mut graph = SymbolGraph::new();
    for store in &stores {
        let root = owning_root(roots, store);
        let mut fragment = reader::read_store_with_command(root, store, dump_command)?;
        graph.symbols.append(&mut fragment.symbols);
        graph.occurrences.append(&mut fragment.occurrences);
        graph.edges.append(&mut fragment.edges);
        graph.file_projection.append(&mut fragment.file_projection);
    }
    dedupe_index_graph(&mut graph);

    if graph.is_empty() {
        return Ok(None);
    }

    graph.engines.push(SymbolEngineRun {
        engine: SymbolProvenance::IndexStore,
        symbol_count: graph.symbols.len(),
        occurrence_count: graph.occurrences.len(),
        tool_version: Some("jsonl-subprocess".to_string()),
    });

    Ok(Some(IndexStoreIngest { graph, stores }))
}

pub fn merge_into_graph(target: &mut SymbolGraph, incoming: SymbolGraph) {
    let mut id_rewrites: HashMap<SymbolId, SymbolId> = HashMap::new();
    let mut by_usr: HashMap<String, usize> = target
        .symbols
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| node.usr.as_ref().map(|usr| (usr.clone(), idx)))
        .collect();

    for incoming_node in incoming.symbols {
        let Some(usr) = incoming_node.usr.clone() else {
            target.symbols.push(incoming_node);
            continue;
        };

        if let Some(existing_idx) = by_usr.get(&usr).copied() {
            id_rewrites.insert(
                incoming_node.id.clone(),
                target.symbols[existing_idx].id.clone(),
            );
            upgrade_node(&mut target.symbols[existing_idx], incoming_node);
            continue;
        }

        if let Some(existing_idx) = find_heuristic_match(target, &incoming_node) {
            let old_id = target.symbols[existing_idx].id.clone();
            let new_id = incoming_node.id.clone();
            id_rewrites.insert(old_id, new_id.clone());
            id_rewrites.insert(incoming_node.id.clone(), new_id);
            upgrade_node(&mut target.symbols[existing_idx], incoming_node);
            by_usr.insert(usr, existing_idx);
            continue;
        }

        by_usr.insert(usr, target.symbols.len());
        target.symbols.push(incoming_node);
    }

    for occ in &mut target.occurrences {
        if let Some(new_id) = id_rewrites.get(&occ.symbol_id) {
            occ.symbol_id = new_id.clone();
        }
    }
    for edge in &mut target.edges {
        if let Some(new_id) = id_rewrites.get(&edge.from) {
            edge.from = new_id.clone();
        }
        if let Some(new_id) = id_rewrites.get(&edge.to) {
            edge.to = new_id.clone();
        }
    }

    let mut seen_occurrences = occurrence_keys(&target.occurrences);
    for mut occ in incoming.occurrences {
        if let Some(new_id) = id_rewrites.get(&occ.symbol_id) {
            occ.symbol_id = new_id.clone();
        }
        let key = occurrence_key(&occ);
        if seen_occurrences.insert(key) {
            target.occurrences.push(occ);
        }
    }

    let mut seen_edges: HashSet<_> = target
        .edges
        .iter()
        .map(|edge| {
            (
                edge.from.clone(),
                edge.to.clone(),
                edge.kind,
                edge.provenance,
                edge.confidence,
            )
        })
        .collect();
    for mut edge in incoming.edges {
        if let Some(new_id) = id_rewrites.get(&edge.from) {
            edge.from = new_id.clone();
        }
        if let Some(new_id) = id_rewrites.get(&edge.to) {
            edge.to = new_id.clone();
        }
        let key = (
            edge.from.clone(),
            edge.to.clone(),
            edge.kind,
            edge.provenance,
            edge.confidence,
        );
        if seen_edges.insert(key) {
            target.edges.push(edge);
        }
    }

    for run in incoming.engines {
        target.engines.push(run);
    }
    rebuild_file_projection(target);
}

fn discover_store_dirs(root: &Path, out: &mut Vec<PathBuf>) {
    if !root.exists() {
        return;
    }
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .max_depth(8)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_dir())
        .take(MAX_DETECT_DIRS)
    {
        let path = entry.path();
        if is_index_store_dir(path) {
            out.push(path.to_path_buf());
        }
    }
}

fn is_index_store_dir(path: &Path) -> bool {
    if path.file_name().and_then(|s| s.to_str()) == Some("store")
        && path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            == Some("index")
    {
        return true;
    }
    path.file_name().and_then(|s| s.to_str()) == Some("DataStore")
        && path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            == Some("Index.noindex")
}

fn owning_root<'a>(roots: &'a [PathBuf], store: &Path) -> &'a Path {
    roots
        .iter()
        .find(|root| store.starts_with(root))
        .map(PathBuf::as_path)
        .unwrap_or_else(|| {
            roots
                .first()
                .map(PathBuf::as_path)
                .unwrap_or(Path::new("."))
        })
}

fn dedupe_index_graph(graph: &mut SymbolGraph) {
    let mut seen_usr = HashSet::new();
    graph.symbols.retain(|node| {
        node.usr
            .as_ref()
            .is_none_or(|usr| seen_usr.insert(usr.clone()))
    });
    let mut seen_occ = HashSet::new();
    graph
        .occurrences
        .retain(|occ| seen_occ.insert(occurrence_key(occ)));
    rebuild_file_projection(graph);
}

fn find_heuristic_match(
    target: &SymbolGraph,
    incoming: &crate::symbols::SymbolNode,
) -> Option<usize> {
    target.symbols.iter().position(|node| {
        node.usr.is_none()
            && node.name == incoming.name
            && node.kind == incoming.kind
            && node.file == incoming.file
    })
}

fn upgrade_node(existing: &mut crate::symbols::SymbolNode, incoming: crate::symbols::SymbolNode) {
    existing.id = incoming.id;
    existing.usr = incoming.usr;
    existing.provenance = SymbolProvenance::IndexStore;
    existing.qualified_name = incoming.qualified_name.or(existing.qualified_name.take());
    existing.module = incoming.module.or(existing.module.take());
    existing.range = incoming.range.or(existing.range);
    existing.signature = incoming.signature.or(existing.signature.take());
    existing.visibility = incoming.visibility.or(existing.visibility);
}

fn occurrence_keys(
    occurrences: &[crate::symbols::SymbolOccurrence],
) -> HashSet<(SymbolId, PathBuf, usize, usize, OccurrenceRole, Confidence)> {
    occurrences.iter().map(occurrence_key).collect()
}

fn occurrence_key(
    occ: &crate::symbols::SymbolOccurrence,
) -> (SymbolId, PathBuf, usize, usize, OccurrenceRole, Confidence) {
    (
        occ.symbol_id.clone(),
        occ.file.clone(),
        occ.range.start_line,
        occ.range.start_col,
        occ.role,
        occ.confidence,
    )
}

fn rebuild_file_projection(graph: &mut SymbolGraph) {
    let mut projection: HashMap<PathBuf, FileSymbolSummary> = HashMap::new();
    for node in &graph.symbols {
        if let Some(file) = &node.file {
            projection
                .entry(file.clone())
                .or_insert_with(|| FileSymbolSummary {
                    file: file.clone(),
                    defined: Vec::new(),
                    referenced: Vec::new(),
                })
                .defined
                .push(node.id.clone());
        }
    }
    for occ in &graph.occurrences {
        let summary = projection
            .entry(occ.file.clone())
            .or_insert_with(|| FileSymbolSummary {
                file: occ.file.clone(),
                defined: Vec::new(),
                referenced: Vec::new(),
            });
        match occ.role {
            OccurrenceRole::Definition | OccurrenceRole::Declaration => {
                summary.defined.push(occ.symbol_id.clone());
            }
            OccurrenceRole::Reference | OccurrenceRole::Call | OccurrenceRole::Import => {
                summary.referenced.push(occ.symbol_id.clone());
            }
        }
    }
    let mut items: Vec<_> = projection.into_values().collect();
    for item in &mut items {
        item.defined.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        item.defined.dedup();
        item.referenced.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        item.referenced.dedup();
    }
    items.sort_by(|a, b| a.file.cmp(&b.file));
    graph.file_projection = items;
}
