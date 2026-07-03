//! Golden round-trip + backward-compatibility regression for the optional
//! `symbol_graph` snapshot section (`loctree.symbol_graph.v1`).
//!
//! Two invariants are pinned here:
//!   1. A hand-built `SymbolGraph` survives JSON serialize → deserialize
//!      byte-for-shape (`assert_eq`), and the schema version is stable.
//!   2. A `Snapshot` produced **before** this layer existed (no `symbol_graph`
//!      key at all) still deserializes, with `symbol_graph == None`, and a
//!      `symbol_graph`-less snapshot serializes **without** the key — so every
//!      existing consumer keeps its wire shape.

use std::path::PathBuf;

use loctree::Snapshot;
use loctree::symbols::{
    Confidence, FileSymbolSummary, LanguageId, OccurrenceRole, SYMBOL_GRAPH_SCHEMA_VERSION,
    SymbolEdge, SymbolEdgeKind, SymbolEngineRun, SymbolGraph, SymbolId, SymbolKind, SymbolNode,
    SymbolOccurrence, SymbolProvenance, SymbolVisibility, TextRange,
};

/// Every `SymbolEdgeKind` variant — keeps the enum and the wire format honest.
/// If a variant is added/removed the match below stops compiling, forcing this
/// golden to be updated deliberately.
fn all_edge_kinds() -> Vec<SymbolEdgeKind> {
    use SymbolEdgeKind::*;
    let kinds = vec![
        Defines,
        Declares,
        References,
        Calls,
        Overrides,
        Conforms,
        Implements,
        Inherits,
        Includes,
        ImportsModule,
        Instantiates,
        MacroExpands,
        SelectorMessage,
        NotificationEmit,
        NotificationObserve,
        IBOutletBinding,
        IBActionBinding,
        Bridges,
    ];
    // Exhaustiveness guard: this match must cover every variant.
    for k in &kinds {
        match k {
            Defines | Declares | References | Calls | Overrides | Conforms | Implements
            | Inherits | Includes | ImportsModule | Instantiates | MacroExpands
            | SelectorMessage | NotificationEmit | NotificationObserve | IBOutletBinding
            | IBActionBinding | Bridges => {}
        }
    }
    kinds
}

fn sample_graph() -> SymbolGraph {
    let store = SymbolId::from_parts(
        "swift/DocumentStore.swift",
        "class",
        "DocumentStore",
        0x1234_5678,
    );
    let persisting = SymbolId::new("swift/DocumentStore.swift::protocol::DocumentPersisting::00");
    let tests = SymbolId::new("swift/DocumentStoreTests.swift::class::DocumentStoreTests::ab");

    let symbols = vec![
        SymbolNode {
            id: store.clone(),
            language: LanguageId::Swift,
            kind: SymbolKind::Class,
            name: "DocumentStore".to_string(),
            qualified_name: Some("Pensieve.DocumentStore".to_string()),
            module: Some("Pensieve".to_string()),
            usr: None,
            file: Some(PathBuf::from("swift/DocumentStore.swift")),
            range: Some(TextRange {
                start_byte: 10,
                end_byte: 420,
                start_line: 20,
                start_col: 1,
                end_line: 32,
                end_col: 2,
            }),
            signature: Some("final class DocumentStore: DocumentPersisting".to_string()),
            visibility: Some(SymbolVisibility::Internal),
            provenance: SymbolProvenance::TreeSitter,
        },
        SymbolNode {
            id: persisting.clone(),
            language: LanguageId::Swift,
            kind: SymbolKind::Protocol,
            name: "DocumentPersisting".to_string(),
            qualified_name: None,
            module: Some("Pensieve".to_string()),
            usr: Some("s:8Pensieve18DocumentPersistingP".to_string()),
            file: Some(PathBuf::from("swift/DocumentStore.swift")),
            range: None,
            signature: None,
            visibility: Some(SymbolVisibility::Public),
            provenance: SymbolProvenance::IndexStore,
        },
        SymbolNode {
            id: SymbolId::new("objc/EditorViewController.h::class::EditorViewController::ff"),
            language: LanguageId::ObjC,
            kind: SymbolKind::Other("view_controller".to_string()),
            name: "EditorViewController".to_string(),
            qualified_name: None,
            module: None,
            usr: None,
            file: Some(PathBuf::from("objc/EditorViewController.h")),
            range: None,
            signature: None,
            visibility: Some(SymbolVisibility::Unknown),
            provenance: SymbolProvenance::Heuristic,
        },
    ];

    let occurrences = vec![
        SymbolOccurrence {
            symbol_id: store.clone(),
            file: PathBuf::from("swift/DocumentStore.swift"),
            range: TextRange {
                start_byte: 10,
                end_byte: 23,
                start_line: 20,
                start_col: 13,
                end_line: 20,
                end_col: 26,
            },
            role: OccurrenceRole::Definition,
            confidence: Confidence::Heuristic,
            engine: SymbolProvenance::TreeSitter,
        },
        SymbolOccurrence {
            symbol_id: store.clone(),
            file: PathBuf::from("swift/DocumentStoreTests.swift"),
            range: TextRange {
                start_byte: 200,
                end_byte: 213,
                start_line: 11,
                start_col: 20,
                end_line: 11,
                end_col: 33,
            },
            role: OccurrenceRole::Call,
            confidence: Confidence::Precise,
            engine: SymbolProvenance::IndexStore,
        },
    ];

    // One edge per kind, cycling endpoints — proves all 18 kinds serialize.
    let edges: Vec<SymbolEdge> = all_edge_kinds()
        .into_iter()
        .enumerate()
        .map(|(i, kind)| {
            let (from, to) = if i % 2 == 0 {
                (tests.clone(), store.clone())
            } else {
                (store.clone(), persisting.clone())
            };
            SymbolEdge {
                from,
                to,
                kind,
                provenance: SymbolProvenance::TreeSitter,
                confidence: Confidence::Heuristic,
            }
        })
        .collect();

    SymbolGraph {
        schema_version: SYMBOL_GRAPH_SCHEMA_VERSION.to_string(),
        engines: vec![SymbolEngineRun {
            engine: SymbolProvenance::TreeSitter,
            symbol_count: symbols.len(),
            occurrence_count: occurrences.len(),
            tool_version: Some("tree-sitter-swift@0.6".to_string()),
        }],
        symbols,
        occurrences,
        edges,
        file_projection: vec![FileSymbolSummary {
            file: PathBuf::from("swift/DocumentStore.swift"),
            defined: vec![store.clone(), persisting.clone()],
            referenced: vec![tests],
        }],
    }
}

#[test]
fn schema_version_is_pinned() {
    assert_eq!(SYMBOL_GRAPH_SCHEMA_VERSION, "loctree.symbol_graph.v1");
    assert_eq!(SymbolGraph::new().schema_version, "loctree.symbol_graph.v1");
}

#[test]
fn symbol_graph_roundtrips_through_json() {
    let graph = sample_graph();
    let json = serde_json::to_string_pretty(&graph).expect("serialize");
    let back: SymbolGraph = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(graph, back, "symbol_graph must survive JSON round-trip");

    // All 18 edge kinds present after the round-trip.
    assert_eq!(back.edges.len(), 18);
}

#[test]
fn empty_graph_is_empty() {
    let g = SymbolGraph::new();
    assert!(g.is_empty());
    assert_eq!(g.schema_version, SYMBOL_GRAPH_SCHEMA_VERSION);
}

#[test]
fn query_helpers_resolve_against_sample() {
    let g = sample_graph();
    let hits = g.lookup("DocumentStore");
    assert_eq!(hits.len(), 1);
    let id = hits[0].id.clone();

    // Two occurrences of DocumentStore (definition + call).
    assert_eq!(g.references(&id).len(), 2);

    // Direct blast radius is non-empty (References/Calls/... edges target it).
    assert!(
        !g.blast_radius(&id).is_empty(),
        "DocumentStore should have dependents"
    );
}

// ---- Backward compatibility: the optional field must not break old snapshots.

/// A real snapshot serialized before `symbol_graph` existed deserializes fine
/// and yields `symbol_graph == None`.
#[test]
fn old_golden_snapshot_deserializes_without_symbol_graph() {
    let path = format!(
        "{}/tests/fixtures/snapshot_v0_10_2.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).expect("read golden snapshot");
    assert!(
        !raw.contains("symbol_graph"),
        "golden fixture predates the field"
    );
    let snap: Snapshot = serde_json::from_str(&raw).expect("old snapshot must still deserialize");
    assert!(
        snap.symbol_graph.is_none(),
        "missing section defaults to None"
    );
}

/// A snapshot without a symbol graph serializes **without** the key — existing
/// consumers see the exact same wire shape they saw before Wave A.
#[test]
fn snapshot_without_symbol_graph_omits_the_key() {
    let snap: Snapshot =
        serde_json::from_str(r#"{"metadata":{}}"#).expect("minimal snapshot deserializes");
    assert!(snap.symbol_graph.is_none());

    let value = serde_json::to_value(&snap).expect("serialize");
    assert!(
        value.get("symbol_graph").is_none(),
        "None symbol_graph must be skipped, not emitted as null"
    );
}

/// A snapshot WITH a symbol graph round-trips the section intact.
#[test]
fn snapshot_with_symbol_graph_roundtrips() {
    let mut snap: Snapshot =
        serde_json::from_str(r#"{"metadata":{}}"#).expect("minimal snapshot deserializes");
    snap.symbol_graph = Some(sample_graph());

    let json = serde_json::to_string(&snap).expect("serialize");
    assert!(json.contains("symbol_graph"));
    let back: Snapshot = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(
        back.symbol_graph.as_ref(),
        snap.symbol_graph.as_ref(),
        "attached symbol_graph must survive the snapshot round-trip"
    );
}
