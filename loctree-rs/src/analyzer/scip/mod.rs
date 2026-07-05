//! Decode-only SCIP importer for C/C++ deep index mode.
//!
//! Loctree never runs `scip-clang`. When a project already contains
//! `index.scip`, this module decodes it with prost and merges compiler-grade
//! facts into `SymbolGraph` as `ScipClang` / `Precise`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use prost::Message;

use crate::fs_utils;
use crate::symbols::{
    Confidence, FileSymbolSummary, LanguageId, OccurrenceRole, SymbolEdge, SymbolEdgeKind,
    SymbolEngineRun, SymbolGraph, SymbolId, SymbolKind, SymbolNode, SymbolOccurrence,
    SymbolProvenance, TextRange,
};

pub mod proto;

const INDEX_FILE_NAME: &str = "index.scip";
const SCIP_DEFINITION_ROLE: i32 = 1;
const SCIP_IMPORT_ROLE: i32 = 2;

/// Decode `<root>/index.scip` when present. Missing or corrupt indexes degrade
/// to `Ok(None)` so deep mode never blocks the normal scan path.
pub fn import_index_at(root: &Path) -> io::Result<Option<SymbolGraph>> {
    let index_path = root.join(INDEX_FILE_NAME);
    if !index_path.exists() {
        return Ok(None);
    }

    let bytes = fs_utils::read_within(root, &index_path)?;
    let index = match proto::Index::decode(bytes.as_slice()) {
        Ok(index) => index,
        Err(err) => {
            eprintln!(
                "[loctree][warn] failed to decode SCIP index {}; skipping deep-index import: {}",
                index_path.display(),
                err
            );
            return Ok(None);
        }
    };

    let graph = index_to_graph(root, index);
    if graph.is_empty() {
        Ok(None)
    } else {
        Ok(Some(graph))
    }
}

/// Import all project roots, logging and skipping roots whose index cannot be
/// consumed. Used by runtime paths where deep-index is opportunistic.
pub fn import_indexes(root_list: &[PathBuf]) -> Option<SymbolGraph> {
    let mut merged = SymbolGraph::new();
    for root in root_list {
        match import_index_at(root) {
            Ok(Some(graph)) => merge_graphs(&mut merged, graph),
            Ok(None) => {}
            Err(err) => eprintln!(
                "[loctree][warn] failed to read SCIP index under {}; skipping deep-index import: {}",
                root.display(),
                err
            ),
        }
    }
    if merged.is_empty() {
        None
    } else {
        Some(merged)
    }
}

/// Merge `incoming` into `target`, deduping by stable SCIP descriptor while
/// preserving existing heuristic edges/occurrences.
pub fn merge_graphs(target: &mut SymbolGraph, incoming: SymbolGraph) {
    for symbol in incoming.symbols {
        match target
            .symbols
            .iter()
            .position(|existing| existing.id == symbol.id)
        {
            Some(idx) => {
                if target.symbols[idx].provenance != SymbolProvenance::ScipClang {
                    target.symbols[idx] = symbol;
                }
            }
            None => target.symbols.push(symbol),
        }
    }

    let mut seen_occurrences: HashSet<_> = target.occurrences.iter().map(occurrence_key).collect();
    for occurrence in incoming.occurrences {
        if seen_occurrences.insert(occurrence_key(&occurrence)) {
            target.occurrences.push(occurrence);
        }
    }

    let mut seen_edges: HashSet<_> = target.edges.iter().map(edge_key).collect();
    for edge in incoming.edges {
        if seen_edges.insert(edge_key(&edge)) {
            target.edges.push(edge);
        }
    }

    merge_file_projection(target, incoming.file_projection);
    merge_engine_runs(target, incoming.engines);
}

fn index_to_graph(root: &Path, index: proto::Index) -> SymbolGraph {
    let mut graph = SymbolGraph::new();
    let mut symbols_by_id: HashMap<String, SymbolNode> = HashMap::new();
    let mut projection_by_file: HashMap<PathBuf, FileSymbolSummary> = HashMap::new();
    let mut occurrence_count = 0;

    for document in index.documents {
        let Some(language) = language_id(&document.language, &document.relative_path) else {
            continue;
        };
        let file = PathBuf::from(&document.relative_path);
        let source = fs::read_to_string(root.join(&document.relative_path)).unwrap_or_default();

        for info in document.symbols {
            if info.symbol.is_empty() {
                continue;
            }
            let id = SymbolId::new(info.symbol.clone());
            symbols_by_id.insert(
                info.symbol.clone(),
                SymbolNode {
                    id: id.clone(),
                    language,
                    kind: symbol_kind(&info, &info.symbol),
                    name: symbol_name(&info, &info.symbol),
                    qualified_name: None,
                    module: module_name(&info.symbol),
                    usr: Some(info.symbol.clone()),
                    file: Some(file.clone()),
                    range: None,
                    signature: optional_string(info.signature_documentation.clone()),
                    visibility: None,
                    provenance: SymbolProvenance::ScipClang,
                },
            );
            for relationship in info.relationships {
                if let Some(edge) = relationship_edge(id.clone(), relationship) {
                    graph.edges.push(edge);
                }
            }
        }

        for occurrence in document.occurrences {
            if occurrence.symbol.is_empty() {
                continue;
            }
            let Some(range) = decode_range(&occurrence.range, &source) else {
                continue;
            };
            let symbol_id = SymbolId::new(occurrence.symbol.clone());
            occurrence_count += 1;
            graph.occurrences.push(SymbolOccurrence {
                symbol_id: symbol_id.clone(),
                file: file.clone(),
                range,
                role: occurrence_role(occurrence.symbol_roles),
                confidence: Confidence::Precise,
                engine: SymbolProvenance::ScipClang,
            });
            if occurrence.symbol_roles & SCIP_DEFINITION_ROLE != 0 {
                projection_by_file
                    .entry(file.clone())
                    .or_insert_with(|| FileSymbolSummary {
                        file: file.clone(),
                        defined: Vec::new(),
                        referenced: Vec::new(),
                    })
                    .defined
                    .push(symbol_id.clone());
                if let Some(symbol) = symbols_by_id.get_mut(&occurrence.symbol) {
                    symbol.range = Some(range);
                }
            } else {
                projection_by_file
                    .entry(file.clone())
                    .or_insert_with(|| FileSymbolSummary {
                        file: file.clone(),
                        defined: Vec::new(),
                        referenced: Vec::new(),
                    })
                    .referenced
                    .push(symbol_id);
            }
            for relationship in occurrence.relationships {
                if let Some(edge) =
                    relationship_edge(SymbolId::new(occurrence.symbol.clone()), relationship)
                {
                    graph.edges.push(edge);
                }
            }
        }
    }

    graph.symbols = symbols_by_id.into_values().collect();
    graph
        .symbols
        .sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    graph.file_projection = projection_by_file.into_values().collect();
    for projection in &mut graph.file_projection {
        projection
            .defined
            .sort_by(|a, b| a.as_str().cmp(b.as_str()));
        projection.defined.dedup();
        projection
            .referenced
            .sort_by(|a, b| a.as_str().cmp(b.as_str()));
        projection.referenced.dedup();
    }
    graph.file_projection.sort_by(|a, b| a.file.cmp(&b.file));

    if !graph.is_empty() {
        graph.engines.push(SymbolEngineRun {
            engine: SymbolProvenance::ScipClang,
            symbol_count: graph.symbols.len(),
            occurrence_count,
            tool_version: None,
        });
    }
    graph
}

fn language_id(language: &str, relative_path: &str) -> Option<LanguageId> {
    match language.to_ascii_lowercase().as_str() {
        "c" => Some(LanguageId::C),
        "cpp" | "c++" => Some(LanguageId::Cpp),
        "" => match Path::new(relative_path)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
        {
            "c" | "h" => Some(LanguageId::C),
            "cc" | "cpp" | "cxx" | "hpp" => Some(LanguageId::Cpp),
            _ => None,
        },
        _ => None,
    }
}

fn decode_range(range: &[i32], source: &str) -> Option<TextRange> {
    let (start_line, start_col, end_line, end_col) = match range {
        [line, start_col, end_col] => (*line, *start_col, *line, *end_col),
        [start_line, start_col, end_line, end_col] => {
            (*start_line, *start_col, *end_line, *end_col)
        }
        _ => return None,
    };
    if [start_line, start_col, end_line, end_col]
        .iter()
        .any(|n| *n < 0)
    {
        return None;
    }
    let start_line = start_line as usize;
    let start_col = start_col as usize;
    let end_line = end_line as usize;
    let end_col = end_col as usize;
    Some(TextRange {
        start_byte: byte_offset(source, start_line, start_col),
        end_byte: byte_offset(source, end_line, end_col),
        start_line: start_line + 1,
        start_col: start_col + 1,
        end_line: end_line + 1,
        end_col: end_col + 1,
    })
}

fn byte_offset(source: &str, line: usize, col: usize) -> usize {
    let mut offset = 0;
    for (idx, segment) in source.split_inclusive('\n').enumerate() {
        if idx == line {
            return offset + col.min(segment.len());
        }
        offset += segment.len();
    }
    source.len()
}

fn occurrence_role(symbol_roles: i32) -> OccurrenceRole {
    if symbol_roles & SCIP_DEFINITION_ROLE != 0 {
        OccurrenceRole::Definition
    } else if symbol_roles & SCIP_IMPORT_ROLE != 0 {
        OccurrenceRole::Import
    } else {
        OccurrenceRole::Reference
    }
}

fn relationship_edge(from: SymbolId, relationship: proto::Relationship) -> Option<SymbolEdge> {
    if relationship.symbol.is_empty() {
        return None;
    }
    let kind = if relationship.is_implementation {
        SymbolEdgeKind::Implements
    } else if relationship.is_type_definition {
        SymbolEdgeKind::Declares
    } else if relationship.is_definition {
        SymbolEdgeKind::Defines
    } else {
        SymbolEdgeKind::References
    };
    Some(SymbolEdge {
        from,
        to: SymbolId::new(relationship.symbol),
        kind,
        provenance: SymbolProvenance::ScipClang,
        confidence: Confidence::Precise,
    })
}

fn symbol_name(info: &proto::SymbolInformation, symbol: &str) -> String {
    if !info.display_name.is_empty() {
        return info.display_name.clone();
    }
    symbol
        .split_whitespace()
        .last()
        .unwrap_or(symbol)
        .trim_end_matches('.')
        .trim_end_matches("().")
        .trim_end_matches("()")
        .to_string()
}

fn symbol_kind(info: &proto::SymbolInformation, symbol: &str) -> SymbolKind {
    let haystack = format!("{} {}", info.display_name, info.signature_documentation);
    if symbol.contains("()") || haystack.contains('(') {
        SymbolKind::Func
    } else if haystack.contains("class ") {
        SymbolKind::Class
    } else if haystack.contains("struct ") {
        SymbolKind::Struct
    } else if haystack.contains("enum ") {
        SymbolKind::Enum
    } else {
        SymbolKind::Other("scip".to_string())
    }
}

fn module_name(symbol: &str) -> Option<String> {
    symbol
        .split_whitespace()
        .nth(2)
        .filter(|s| !s.is_empty() && *s != ".")
        .map(ToOwned::to_owned)
}

fn optional_string(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

fn occurrence_key(
    occurrence: &SymbolOccurrence,
) -> (String, PathBuf, TextRange, OccurrenceRole, SymbolProvenance) {
    (
        occurrence.symbol_id.as_str().to_string(),
        occurrence.file.clone(),
        occurrence.range,
        occurrence.role,
        occurrence.engine,
    )
}

fn edge_key(edge: &SymbolEdge) -> (String, String, SymbolEdgeKind, SymbolProvenance, Confidence) {
    (
        edge.from.as_str().to_string(),
        edge.to.as_str().to_string(),
        edge.kind,
        edge.provenance,
        edge.confidence,
    )
}

fn merge_file_projection(target: &mut SymbolGraph, incoming: Vec<FileSymbolSummary>) {
    for projection in incoming {
        match target
            .file_projection
            .iter_mut()
            .find(|existing| existing.file == projection.file)
        {
            Some(existing) => {
                existing.defined.extend(projection.defined);
                existing.defined.sort_by(|a, b| a.as_str().cmp(b.as_str()));
                existing.defined.dedup();
                existing.referenced.extend(projection.referenced);
                existing
                    .referenced
                    .sort_by(|a, b| a.as_str().cmp(b.as_str()));
                existing.referenced.dedup();
            }
            None => target.file_projection.push(projection),
        }
    }
    target.file_projection.sort_by(|a, b| a.file.cmp(&b.file));
}

fn merge_engine_runs(target: &mut SymbolGraph, incoming: Vec<SymbolEngineRun>) {
    for run in incoming {
        match target.engines.iter_mut().find(|existing| {
            existing.engine == run.engine && existing.tool_version == run.tool_version
        }) {
            Some(existing) => {
                existing.symbol_count += run.symbol_count;
                existing.occurrence_count += run.occurrence_count;
            }
            None => target.engines.push(run),
        }
    }
}
