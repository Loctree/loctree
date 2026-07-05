#![cfg(feature = "deep-index")]

use std::fs;
use std::path::{Path, PathBuf};

use loctree::analyzer::scip::proto::{
    Document, Index, Occurrence, Relationship, SymbolInformation,
};
use loctree::analyzer::scip::{import_index_at, merge_graphs};
use loctree::symbols::{
    Confidence, LanguageId, OccurrenceRole, SymbolEdge, SymbolEdgeKind, SymbolGraph, SymbolId,
    SymbolKind, SymbolNode, SymbolProvenance,
};
use prost::Message;

const ADD_SYMBOL: &str = "scip-clang c main.c add().";
const MAIN_SYMBOL: &str = "scip-clang c main.c main().";

fn write_fixture_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = include_str!("fixtures/cfamily/scip/main.c");
    fs::write(dir.path().join("main.c"), source).expect("write source");

    let index = Index {
        external_symbols: Vec::new(),
        documents: vec![Document {
            relative_path: "main.c".to_string(),
            language: "c".to_string(),
            occurrences: vec![
                Occurrence {
                    range: vec![0, 4, 7],
                    symbol: ADD_SYMBOL.to_string(),
                    symbol_roles: 1,
                    relationships: Vec::new(),
                },
                Occurrence {
                    range: vec![5, 11, 14],
                    symbol: ADD_SYMBOL.to_string(),
                    symbol_roles: 0,
                    relationships: Vec::new(),
                },
            ],
            symbols: vec![
                SymbolInformation {
                    symbol: ADD_SYMBOL.to_string(),
                    display_name: "add".to_string(),
                    signature_documentation: "int add(int lhs, int rhs)".to_string(),
                    relationships: Vec::new(),
                },
                SymbolInformation {
                    symbol: MAIN_SYMBOL.to_string(),
                    display_name: "main".to_string(),
                    signature_documentation: "int main(void)".to_string(),
                    relationships: vec![Relationship {
                        symbol: ADD_SYMBOL.to_string(),
                        is_reference: true,
                        is_implementation: false,
                        is_type_definition: false,
                        is_definition: false,
                    }],
                },
            ],
        }],
    };

    let mut bytes = Vec::new();
    index.encode(&mut bytes).expect("encode scip fixture");
    fs::write(dir.path().join("index.scip"), bytes).expect("write index.scip");
    dir
}

#[test]
fn decodes_scip_index_as_precise_c_symbols() {
    let project = write_fixture_project();

    let graph = import_index_at(project.path())
        .expect("import succeeds")
        .expect("fixture index exists");

    assert_eq!(graph.symbols.len(), 2);
    assert!(graph.symbols.iter().all(|s| {
        s.language == LanguageId::C
            && s.kind == SymbolKind::Func
            && s.provenance == SymbolProvenance::ScipClang
    }));
    assert!(graph.occurrences.iter().all(|o| {
        o.file == PathBuf::from("main.c")
            && o.confidence == Confidence::Precise
            && o.engine == SymbolProvenance::ScipClang
    }));
    assert!(
        graph
            .occurrences
            .iter()
            .any(|o| o.symbol_id.as_str() == ADD_SYMBOL && o.role == OccurrenceRole::Definition)
    );
    assert!(graph.edges.iter().any(|e| e.from.as_str() == MAIN_SYMBOL
        && e.to.as_str() == ADD_SYMBOL
        && e.kind == SymbolEdgeKind::References
        && e.confidence == Confidence::Precise));
}

#[test]
fn merges_precise_scip_without_replacing_heuristic_graph() {
    let project = write_fixture_project();
    let precise = import_index_at(project.path())
        .expect("import succeeds")
        .expect("fixture index exists");

    let heuristic_id = SymbolId::new(ADD_SYMBOL);
    let mut base = SymbolGraph::new();
    base.symbols.push(SymbolNode {
        id: heuristic_id.clone(),
        language: LanguageId::C,
        kind: SymbolKind::Func,
        name: "add".to_string(),
        qualified_name: None,
        module: None,
        usr: None,
        file: Some(PathBuf::from("main.c")),
        range: None,
        signature: None,
        visibility: None,
        provenance: SymbolProvenance::TreeSitter,
    });
    base.edges.push(SymbolEdge {
        from: SymbolId::new("heuristic-caller"),
        to: heuristic_id,
        kind: SymbolEdgeKind::Calls,
        provenance: SymbolProvenance::TreeSitter,
        confidence: Confidence::Heuristic,
    });

    merge_graphs(&mut base, precise);

    assert_eq!(
        base.symbols
            .iter()
            .filter(|s| s.id.as_str() == ADD_SYMBOL)
            .count(),
        1,
        "same SCIP descriptor must dedupe symbol nodes"
    );
    assert!(
        base.symbols.iter().any(|s| {
            s.id.as_str() == ADD_SYMBOL && s.provenance == SymbolProvenance::ScipClang
        })
    );
    assert!(base.edges.iter().any(|e| {
        e.from.as_str() == "heuristic-caller"
            && e.to.as_str() == ADD_SYMBOL
            && e.confidence == Confidence::Heuristic
    }));
    assert!(base.edges.iter().any(|e| {
        e.from.as_str() == MAIN_SYMBOL
            && e.to.as_str() == ADD_SYMBOL
            && e.confidence == Confidence::Precise
    }));
}

#[test]
fn scip_missing_or_corrupt_index_degrades_without_panicking() {
    let missing = tempfile::tempdir().expect("tempdir");
    assert!(
        import_index_at(missing.path())
            .expect("missing index is not an error")
            .is_none()
    );

    fs::write(missing.path().join("index.scip"), b"not protobuf").expect("write corrupt index");
    assert!(
        import_index_at(missing.path())
            .expect("corrupt index is logged and skipped")
            .is_none()
    );
}

#[test]
fn import_path_is_named_index_scip() {
    let project = write_fixture_project();
    assert!(Path::new(project.path()).join("index.scip").exists());
}
