#[cfg(not(all(target_os = "macos", feature = "deep-index-macos")))]
#[test]
fn indexstore_import_skips_cleanly_without_macos_feature() {
    // The deep reader is intentionally compiled out unless both gates are true.
}

#[cfg(all(target_os = "macos", feature = "deep-index-macos"))]
mod macos_feature {
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    use loctree::analyzer::indexstore::{ingest_roots_with_dump_command, merge_into_graph};
    use loctree::symbols::{
        Confidence, LanguageId, OccurrenceRole, SymbolGraph, SymbolId, SymbolKind, SymbolNode,
        SymbolOccurrence, SymbolProvenance, TextRange,
    };
    use tempfile::tempdir;

    #[test]
    fn indexstore_jsonl_import_merges_precise_usr_over_heuristic_symbol() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let swift = root.join("Sources/App/Workspace.swift");
        fs::create_dir_all(swift.parent().unwrap()).expect("source dir");
        fs::write(&swift, "struct WorkspaceSubstrate {}\n").expect("swift fixture");
        let objc = root.join("Sources/App/WorkspaceBridge.m");
        fs::write(&objc, "@implementation WorkspaceBridge\n@end\n").expect("objc fixture");

        let store = root.join(".build/debug/index/store");
        fs::create_dir_all(&store).expect("index store dir");

        let helper = root.join("dump-indexstore.sh");
        let mut script = fs::File::create(&helper).expect("helper");
        writeln!(
            script,
            r#"#!/bin/sh
cat <<'JSONL'
{{"kind":"symbol","usr":"s:9Pensieve18WorkspaceSubstrateV","name":"WorkspaceSubstrate","language":"swift","symbol_kind":"struct","file":"Sources/App/Workspace.swift","range":{{"start_line":1,"start_col":8,"end_line":1,"end_col":26}},"visibility":"internal"}}
{{"kind":"symbol","usr":"s:9Pensieve18WorkspaceSubstrateV","name":"WorkspaceSubstrate","language":"objc","symbol_kind":"class","file":"Sources/App/WorkspaceBridge.m","range":{{"start_line":1,"start_col":17,"end_line":1,"end_col":32}},"visibility":"internal"}}
{{"kind":"occurrence","usr":"s:9Pensieve18WorkspaceSubstrateV","file":"Sources/App/Workspace.swift","role":"definition","range":{{"start_line":1,"start_col":8,"end_line":1,"end_col":26}}}}
{{"kind":"occurrence","usr":"s:9Pensieve18WorkspaceSubstrateV","file":"Sources/App/Workspace.swift","role":"reference","range":{{"start_line":3,"start_col":14,"end_line":3,"end_col":32}}}}
{{"kind":"occurrence","usr":"s:9Pensieve18WorkspaceSubstrateV","file":"Sources/App/WorkspaceBridge.m","role":"reference","range":{{"start_line":1,"start_col":17,"end_line":1,"end_col":32}}}}
JSONL"#
        )
        .expect("write helper");
        let mut perms = fs::metadata(&helper)
            .expect("helper metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&helper, perms).expect("chmod helper");

        let ingest = ingest_roots_with_dump_command(&[root.to_path_buf()], &helper)
            .expect("ingest")
            .expect("ingest graph");
        assert_eq!(ingest.stores, vec![store]);
        assert!(
            ingest
                .graph
                .symbols
                .iter()
                .all(|s| s.provenance == SymbolProvenance::IndexStore)
        );
        assert!(
            ingest
                .graph
                .occurrences
                .iter()
                .all(|o| o.confidence == Confidence::Precise)
        );

        let old_id = SymbolId::new("Sources/App/Workspace.swift::struct::WorkspaceSubstrate::0001");
        let mut target = SymbolGraph::new();
        target.symbols.push(SymbolNode {
            id: old_id.clone(),
            language: LanguageId::Swift,
            kind: SymbolKind::Struct,
            name: "WorkspaceSubstrate".to_string(),
            qualified_name: None,
            module: None,
            usr: None,
            file: Some("Sources/App/Workspace.swift".into()),
            range: None,
            signature: None,
            visibility: None,
            provenance: SymbolProvenance::TreeSitter,
        });
        target.occurrences.push(SymbolOccurrence {
            symbol_id: old_id,
            file: "Sources/App/Workspace.swift".into(),
            range: TextRange {
                start_line: 1,
                start_col: 8,
                end_line: 1,
                end_col: 26,
                ..TextRange::default()
            },
            role: OccurrenceRole::Definition,
            confidence: Confidence::Heuristic,
            engine: SymbolProvenance::TreeSitter,
        });

        merge_into_graph(&mut target, ingest.graph);

        assert_eq!(
            target.symbols.len(),
            1,
            "USR import should upgrade, not duplicate"
        );
        assert_eq!(
            target.symbols[0].usr.as_deref(),
            Some("s:9Pensieve18WorkspaceSubstrateV")
        );
        assert_eq!(target.symbols[0].provenance, SymbolProvenance::IndexStore);
        assert!(
            target
                .occurrences
                .iter()
                .any(|o| o.confidence == Confidence::Precise
                    && o.role == OccurrenceRole::Reference)
        );
        assert!(
            target
                .occurrences
                .iter()
                .all(|o| o.symbol_id.as_str() == "s:9Pensieve18WorkspaceSubstrateV")
        );
        assert!(
            target.occurrences.iter().any(|o| {
                o.file == std::path::PathBuf::from("Sources/App/WorkspaceBridge.m")
                    && o.confidence == Confidence::Precise
            }),
            "Swift and ObjC occurrences sharing a USR should resolve to one precise symbol"
        );
    }
}
