//! Integration test for the `loctree/find` custom LSP request (Plan 07).
//!
//! Covers params parsing, mode/lang/exported_only/dead_only/limit
//! post-processing, and the response shape contract.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use loctree::analyzer::dead_parrots::{
    SimilarityCandidate, SymbolFileMatch, SymbolMatch, SymbolMatchKind, SymbolSearchResult,
};
use loctree::analyzer::occurrences::{FileScope, ScanOptions, scan_files, scan_files_with};
use loctree::analyzer::search::{CrossMatchFile, DeadStatus, ParamMatch, SearchResults};
use loctree::snapshot::Snapshot;
use loctree::types::FileAnalysis;
use loctree_lsp::{CursorState, FindParams, find};

fn sym_match(line: usize, kind: SymbolMatchKind, ctx: &str) -> SymbolMatch {
    SymbolMatch {
        line,
        context: ctx.into(),
        is_definition: matches!(kind, SymbolMatchKind::Definition),
        kind,
    }
}

fn fixture_results() -> SearchResults {
    SearchResults {
        query: "Auth".into(),
        symbol_matches: SymbolSearchResult {
            found: true,
            total_matches: 4,
            files: vec![
                SymbolFileMatch {
                    file: "src/auth.rs".into(),
                    matches: vec![
                        sym_match(10, SymbolMatchKind::Definition, "fn auth_user"),
                        sym_match(42, SymbolMatchKind::Usage, "auth_user()"),
                    ],
                },
                SymbolFileMatch {
                    file: "src/legacy.ts".into(),
                    matches: vec![sym_match(8, SymbolMatchKind::Import, "import auth")],
                },
                SymbolFileMatch {
                    file: "src/dead.rs".into(),
                    matches: vec![sym_match(5, SymbolMatchKind::Definition, "fn dead_auth")],
                },
            ],
        },
        param_matches: vec![
            ParamMatch {
                file: "src/auth.rs".into(),
                line: Some(10),
                function: "auth_user".into(),
                param_name: "token".into(),
                param_type: Some("Auth".into()),
            },
            ParamMatch {
                file: "src/legacy.ts".into(),
                line: Some(20),
                function: "validate".into(),
                param_name: "auth".into(),
                param_type: Some("string".into()),
            },
        ],
        semantic_matches: vec![
            SimilarityCandidate {
                symbol: "AuthUser".into(),
                file: "src/auth.rs".into(),
                score: 0.91,
                line: Some(10),
            },
            SimilarityCandidate {
                symbol: "Authenticator".into(),
                file: "src/legacy.ts".into(),
                score: 0.55,
                line: Some(20),
            },
        ],
        dead_status: DeadStatus {
            is_exported: true,
            is_dead: true,
            dead_in_files: vec!["src/dead.rs".into()],
        },
        suppression_matches: vec![],
        cross_matches: vec![],
    }
}

#[test]
fn params_deserialize_minimal() {
    let json = serde_json::json!({ "query": "Auth" });
    let params: FindParams = serde_json::from_value(json).expect("minimal params parse");
    assert_eq!(params.query, "Auth");
    assert_eq!(params.mode, "single");
    assert!(params.lang.is_none());
    assert!(!params.dead_only);
    assert!(!params.exported_only);
    assert!(params.limit.is_none());
}

#[test]
fn params_deserialize_full() {
    let json = serde_json::json!({
        "query": "Auth User",
        "mode": "and",
        "lang": "rust",
        "dead_only": true,
        "exported_only": true,
        "limit": 10,
        "cursor": "opaque-token",
        "chunk_size": 30
    });
    let params: FindParams = serde_json::from_value(json).expect("full params parse");
    assert_eq!(params.mode, "and");
    assert_eq!(params.lang.as_deref(), Some("rust"));
    assert!(params.dead_only);
    assert!(params.exported_only);
    assert_eq!(params.limit, Some(10));
    assert_eq!(params.cursor.as_deref(), Some("opaque-token"));
    assert_eq!(params.chunk_size, Some(30));
}

#[test]
fn build_query_modes() {
    let mut params = FindParams {
        query: "Auth User".into(),
        mode: "single".into(),
        lang: None,
        file: None,
        dead_only: false,
        exported_only: false,
        limit: None,
        project: None,
        cursor: None,
        chunk_size: None,
        whole_token: false,
        group_by_file: false,
        count_only: false,
        slim: false,
        offset: 0,
        symbol_id: None,
    };
    assert_eq!(find::build_query(&params), "Auth User");

    params.mode = "split".into();
    assert_eq!(find::build_query(&params), "Auth|User");

    params.mode = "and".into();
    assert_eq!(find::build_query(&params), "Auth|User");
}

#[test]
fn lang_filter_keeps_only_rust_files() {
    let params = FindParams {
        query: "Auth".into(),
        mode: "single".into(),
        lang: Some("rust".into()),
        file: None,
        dead_only: false,
        exported_only: false,
        limit: None,
        project: None,
        cursor: None,
        chunk_size: None,
        whole_token: false,
        group_by_file: false,
        count_only: false,
        slim: false,
        offset: 0,
        symbol_id: None,
    };
    let response = find::build_response(fixture_results(), &params);

    assert!(
        response
            .symbol_matches
            .data
            .files
            .iter()
            .all(|f| f.file.ends_with(".rs")),
        "lang=rust must drop non-rs files; got {:?}",
        response.symbol_matches.data.files
    );
    assert!(
        response
            .param_matches
            .data
            .iter()
            .all(|m| m.file.ends_with(".rs")),
        "param matches must be filtered by language too"
    );
    assert!(
        response
            .semantic_matches
            .data
            .iter()
            .all(|c| c.file.ends_with(".rs"))
    );
    assert!(
        response
            .dead_status
            .dead_in_files
            .iter()
            .all(|f| f.ends_with(".rs"))
    );
}

#[test]
fn exported_only_keeps_only_definition_matches() {
    let params = FindParams {
        query: "Auth".into(),
        mode: "single".into(),
        lang: None,
        file: None,
        dead_only: false,
        exported_only: true,
        limit: None,
        project: None,
        cursor: None,
        chunk_size: None,
        whole_token: false,
        group_by_file: false,
        count_only: false,
        slim: false,
        offset: 0,
        symbol_id: None,
    };
    let response = find::build_response(fixture_results(), &params);
    for file in &response.symbol_matches.data.files {
        for m in &file.matches {
            assert!(
                matches!(m.kind, SymbolMatchKind::Definition),
                "exported_only must drop non-definition matches; saw {:?} in {}",
                m.kind,
                file.file
            );
        }
    }
    // src/legacy.ts had only an Import — its file entry should be gone.
    assert!(
        !response
            .symbol_matches
            .data
            .files
            .iter()
            .any(|f| f.file == "src/legacy.ts"),
        "files with no surviving matches must be dropped"
    );
}

#[test]
fn dead_only_keeps_only_dead_export_files() {
    let params = FindParams {
        query: "Auth".into(),
        mode: "single".into(),
        lang: None,
        file: None,
        dead_only: true,
        exported_only: false,
        limit: None,
        project: None,
        cursor: None,
        chunk_size: None,
        whole_token: false,
        group_by_file: false,
        count_only: false,
        slim: false,
        offset: 0,
        symbol_id: None,
    };
    let response = find::build_response(fixture_results(), &params);
    for file in &response.symbol_matches.data.files {
        assert!(
            response.dead_status.dead_in_files.contains(&file.file),
            "dead_only must keep only files in dead_status; leaked: {}",
            file.file
        );
    }
    assert!(
        response.param_matches.data.is_empty(),
        "no fixture param_matches touch the dead file"
    );
}

#[test]
fn dead_only_and_mode_and_combination_preserves_dead_cross_matches() {
    let mut results = fixture_results();
    results.cross_matches = vec![
        CrossMatchFile {
            file: "src/dead.rs".into(),
            matched_terms: vec![],
        },
        CrossMatchFile {
            file: "src/auth.rs".into(),
            matched_terms: vec![],
        },
    ];

    let params = FindParams {
        query: "Auth User".into(),
        mode: "and".into(),
        lang: None,
        file: None,
        dead_only: true,
        exported_only: false,
        limit: None,
        project: None,
        cursor: None,
        chunk_size: None,
        whole_token: false,
        group_by_file: false,
        count_only: false,
        slim: false,
        offset: 0,
        symbol_id: None,
    };
    let response = find::build_response(results, &params);

    assert!(response.symbol_matches.data.files.is_empty());
    assert!(response.param_matches.data.is_empty());
    assert!(response.semantic_matches.data.is_empty());
    assert!(response.suppression_matches.data.is_empty());
    assert_eq!(response.cross_matches.data.len(), 1);
    assert_eq!(response.cross_matches.data[0].file, "src/dead.rs");
    assert_eq!(response.total_matches, 1);
}

#[test]
fn lang_filter_handles_extensionless_analyzer_filename_heuristics() {
    let mut results = fixture_results();
    results.symbol_matches.files.push(SymbolFileMatch {
        file: "Makefile".into(),
        matches: vec![sym_match(1, SymbolMatchKind::Definition, "deploy:")],
    });
    results.param_matches.push(ParamMatch {
        file: "Makefile".into(),
        line: Some(1),
        function: "deploy".into(),
        param_name: "target".into(),
        param_type: None,
    });
    results.semantic_matches.push(SimilarityCandidate {
        symbol: "MakeDeploy".into(),
        file: "build/GNUmakefile".into(),
        score: 0.99,
        line: Some(1),
    });
    results.dead_status.dead_in_files = vec!["Makefile".into(), "src/dead.rs".into()];
    results.cross_matches = vec![
        CrossMatchFile {
            file: "Makefile".into(),
            matched_terms: vec![],
        },
        CrossMatchFile {
            file: "src/auth.rs".into(),
            matched_terms: vec![],
        },
    ];

    let params = FindParams {
        query: "deploy target".into(),
        mode: "single".into(),
        lang: Some("make".into()),
        file: None,
        dead_only: false,
        exported_only: false,
        limit: None,
        project: None,
        cursor: None,
        chunk_size: None,
        whole_token: false,
        group_by_file: false,
        count_only: false,
        slim: false,
        offset: 0,
        symbol_id: None,
    };
    let response = find::build_response(results, &params);

    assert_eq!(response.symbol_matches.data.files.len(), 1);
    assert_eq!(response.symbol_matches.data.files[0].file, "Makefile");
    assert_eq!(response.param_matches.data.len(), 1);
    assert_eq!(response.param_matches.data[0].file, "Makefile");
    assert_eq!(response.semantic_matches.data.len(), 1);
    assert_eq!(response.semantic_matches.data[0].file, "build/GNUmakefile");
    assert_eq!(response.dead_status.dead_in_files, vec!["Makefile"]);
    assert_eq!(response.cross_matches.data.len(), 1);
    assert_eq!(response.cross_matches.data[0].file, "Makefile");
}

#[test]
fn limit_caps_each_match_list() {
    let params = FindParams {
        query: "Auth".into(),
        mode: "single".into(),
        lang: None,
        file: None,
        dead_only: false,
        exported_only: false,
        limit: Some(1),
        project: None,
        cursor: None,
        chunk_size: None,
        whole_token: false,
        group_by_file: false,
        count_only: false,
        slim: false,
        offset: 0,
        symbol_id: None,
    };
    let response = find::build_response(fixture_results(), &params);
    assert!(response.symbol_matches.data.files.len() <= 1);
    assert!(response.param_matches.data.len() <= 1);
    assert!(response.semantic_matches.data.len() <= 1);
}

#[test]
fn large_symbol_match_set_round_trips_through_cursor_pages() {
    let mut results = fixture_results();
    results.symbol_matches.files = (0..120)
        .map(|i| SymbolFileMatch {
            file: format!("src/symbol_{i:03}.rs"),
            matches: vec![sym_match(1, SymbolMatchKind::Definition, "fn target")],
        })
        .collect();
    results.symbol_matches.total_matches = 120;
    results.param_matches.clear();
    results.semantic_matches.clear();
    results.suppression_matches.clear();
    results.cross_matches.clear();
    results.dead_status.dead_in_files.clear();

    let snapshot_id = "main@find-pagination";
    let mut params = FindParams {
        query: "target".into(),
        mode: "single".into(),
        lang: None,
        file: None,
        dead_only: false,
        exported_only: false,
        limit: Some(120),
        project: None,
        cursor: None,
        chunk_size: Some(50),
        whole_token: false,
        group_by_file: false,
        count_only: false,
        slim: false,
        offset: 0,
        symbol_id: None,
    };

    let first =
        find::build_response_paginated(results, &params, snapshot_id).expect("first symbol page");
    assert_eq!(first.symbol_matches.chunk, 0);
    assert_eq!(first.symbol_matches.total_chunks, 3);
    assert_eq!(first.symbol_matches.data.files.len(), 50);
    assert!(first.param_matches.next_cursor.is_none());
    assert!(first.semantic_matches.next_cursor.is_none());
    assert!(first.cross_matches.next_cursor.is_none());

    let first_cursor = first
        .symbol_matches
        .next_cursor
        .as_deref()
        .expect("120 symbol files should emit a cursor");
    assert_url_safe_cursor(first_cursor);
    let decoded = CursorState::decode(first_cursor, snapshot_id, "loctree/find.symbol_matches")
        .expect("symbol cursor decodes");
    assert_eq!(decoded.offset, 50);

    let mut results = fixture_results();
    results.symbol_matches.files = (0..120)
        .map(|i| SymbolFileMatch {
            file: format!("src/symbol_{i:03}.rs"),
            matches: vec![sym_match(1, SymbolMatchKind::Definition, "fn target")],
        })
        .collect();
    results.symbol_matches.total_matches = 120;
    results.param_matches.clear();
    results.semantic_matches.clear();
    results.suppression_matches.clear();
    results.cross_matches.clear();
    results.dead_status.dead_in_files.clear();

    params.cursor = Some(first_cursor.to_string());
    let second =
        find::build_response_paginated(results, &params, snapshot_id).expect("second symbol page");
    assert_eq!(second.symbol_matches.chunk, 1);
    assert_eq!(
        second.symbol_matches.data.files[0].file,
        "src/symbol_050.rs"
    );
    assert_eq!(second.symbol_matches.data.files.len(), 50);

    let second_cursor = second
        .symbol_matches
        .next_cursor
        .as_deref()
        .expect("second page should emit final cursor");
    assert_url_safe_cursor(second_cursor);

    let mut results = fixture_results();
    results.symbol_matches.files = (0..120)
        .map(|i| SymbolFileMatch {
            file: format!("src/symbol_{i:03}.rs"),
            matches: vec![sym_match(1, SymbolMatchKind::Definition, "fn target")],
        })
        .collect();
    results.symbol_matches.total_matches = 120;
    results.param_matches.clear();
    results.semantic_matches.clear();
    results.suppression_matches.clear();
    results.cross_matches.clear();
    results.dead_status.dead_in_files.clear();

    params.cursor = Some(second_cursor.to_string());
    let final_page =
        find::build_response_paginated(results, &params, snapshot_id).expect("final symbol page");
    assert_eq!(final_page.symbol_matches.chunk, 2);
    assert_eq!(final_page.symbol_matches.data.files.len(), 20);
    assert_eq!(
        final_page.symbol_matches.data.files[19].file,
        "src/symbol_119.rs"
    );
    assert!(final_page.symbol_matches.next_cursor.is_none());

    let all_paths: Vec<_> = first
        .symbol_matches
        .data
        .files
        .into_iter()
        .chain(second.symbol_matches.data.files)
        .chain(final_page.symbol_matches.data.files)
        .map(|entry| entry.file)
        .collect();
    assert_eq!(all_paths.len(), 120);
    assert_eq!(all_paths[0], "src/symbol_000.rs");
    assert_eq!(all_paths[119], "src/symbol_119.rs");
}

#[test]
fn small_find_buckets_are_single_page_envelopes() {
    let params = FindParams {
        query: "Auth".into(),
        mode: "single".into(),
        lang: None,
        file: None,
        dead_only: false,
        exported_only: false,
        limit: None,
        project: None,
        cursor: None,
        chunk_size: Some(30),
        whole_token: false,
        group_by_file: false,
        count_only: false,
        slim: false,
        offset: 0,
        symbol_id: None,
    };

    let response = find::build_response_paginated(fixture_results(), &params, "snapshot")
        .expect("small fixture page");

    assert_eq!(response.symbol_matches.chunk, 0);
    assert_eq!(response.symbol_matches.total_chunks, 1);
    assert!(response.symbol_matches.next_cursor.is_none());
    assert_eq!(response.param_matches.chunk, 0);
    assert_eq!(response.param_matches.total_chunks, 1);
    assert!(response.param_matches.next_cursor.is_none());
    assert_eq!(response.semantic_matches.chunk, 0);
    assert_eq!(response.semantic_matches.total_chunks, 1);
    assert!(response.semantic_matches.next_cursor.is_none());
    assert_eq!(response.cross_matches.chunk, 0);
    assert_eq!(response.cross_matches.total_chunks, 1);
    assert!(response.cross_matches.next_cursor.is_none());
}

fn assert_url_safe_cursor(token: &str) {
    assert!(!token.contains('+'), "cursor should be URL-safe: {token}");
    assert!(!token.contains('/'), "cursor should be URL-safe: {token}");
    assert!(!token.contains('='), "cursor should not be padded: {token}");
}

#[test]
fn semantic_matches_sorted_by_score_desc() {
    let params = FindParams {
        query: "Auth".into(),
        mode: "single".into(),
        lang: None,
        file: None,
        dead_only: false,
        exported_only: false,
        limit: None,
        project: None,
        cursor: None,
        chunk_size: None,
        whole_token: false,
        group_by_file: false,
        count_only: false,
        slim: false,
        offset: 0,
        symbol_id: None,
    };
    let response = find::build_response(fixture_results(), &params);
    let scores: Vec<f64> = response
        .semantic_matches
        .data
        .iter()
        .map(|c| c.score)
        .collect();
    let mut sorted = scores.clone();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap());
    assert_eq!(scores, sorted, "semantic matches must be score-desc");
}

// ---------------------------------------------------------------------------
// Literal mode (W2-A): the LSP surface of the W1 literal truth layer.
// ---------------------------------------------------------------------------

const LITERAL_SOURCE: &str = "pub fn process() {\n    let mut utterance_id = 0;\n    utterance_id += 1;\n    let _evt = Event { utterance_id };\n    let _ = utterance_id;\n}\n";

fn literal_params() -> FindParams {
    FindParams {
        query: "utterance_id".into(),
        mode: "literal".into(),
        lang: None,
        file: None,
        dead_only: false,
        exported_only: false,
        limit: None,
        project: None,
        cursor: None,
        chunk_size: None,
        whole_token: false,
        group_by_file: false,
        count_only: false,
        slim: false,
        offset: 0,
        symbol_id: None,
    }
}

#[test]
fn literal_mode_params_parse() {
    let json = serde_json::json!({
        "query": "utterance_id",
        "mode": "literal",
        "whole_token": true,
        "group_by_file": true,
        "count_only": true,
        "offset": 2,
        "limit": 3,
        "file": "src/scribe.rs"
    });
    let params: FindParams = serde_json::from_value(json).expect("literal params parse");
    assert_eq!(params.mode, "literal");
    assert_eq!(params.query, "utterance_id");
    assert!(params.whole_token);
    assert!(params.group_by_file);
    assert!(params.count_only);
    assert_eq!(params.offset, 2);
    assert_eq!(params.limit, Some(3));
    assert_eq!(params.file.as_deref(), Some("src/scribe.rs"));
}

#[test]
fn find_request_build_literal_response_carries_role_truth_and_empties_buckets() {
    let literal = scan_files([("src/scribe.rs", LITERAL_SOURCE)], "utterance_id");
    let response = find::build_literal_response(literal, vec![], &literal_params());

    // AST/fuzzy buckets are intentionally empty in literal mode.
    assert!(response.symbol_matches.data.files.is_empty());
    assert!(response.param_matches.data.is_empty());
    assert!(response.semantic_matches.data.is_empty());
    assert!(response.suppression_matches.data.is_empty());
    assert!(response.cross_matches.data.is_empty());

    // The literal truth layer carries the occurrences.
    let lit = response
        .literal_matches
        .as_ref()
        .expect("literal_matches present in literal mode");
    assert_eq!(lit.source, "literal");
    assert_eq!(lit.occurrences.len(), 4);
    assert_eq!(response.total_matches, 4, "total reflects occurrence count");
    let kinds: Vec<&str> = lit
        .occurrences
        .iter()
        .map(|o| o.occurrence_kind.as_str())
        .collect();
    assert_eq!(
        kinds,
        vec![
            "definition_like",
            "mutation_like",
            "field_emit_like",
            // The trailing `let _ = utterance_id;` read is now an honest
            // `identifier` (language-aware fallback), no longer blanket `unknown`.
            "identifier"
        ]
    );

    let json = serde_json::to_value(&response).expect("literal response json");
    let literal_json = &json["literal_matches"];
    assert_eq!(literal_json["query_kind"], "identifier");
    assert_eq!(literal_json["match_mode"], "identifier_boundary");
    assert!(
        literal_json["coverage_line"]
            .as_str()
            .is_some_and(|line| line.contains("scanned 1 of 1 repo files")),
        "LSP literal response must expose CLI coverage text: {json}"
    );
    assert_eq!(literal_json["scope"]["files_scanned"], 1);
    assert_eq!(
        literal_json["scope_classifications"][0]["scope_classification"],
        "production"
    );

    let occurrences = literal_json["occurrences"]
        .as_array()
        .expect("occurrence json array");
    let roles: Vec<&str> = occurrences
        .iter()
        .map(|o| o["match_role"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        roles,
        vec!["local_binding", "mutation", "field_emission", "reference"],
        "LSP literal response must expose the compact role contract: {json}"
    );
    let confidences: Vec<&str> = occurrences
        .iter()
        .map(|o| o["confidence"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(confidences, vec!["high", "high", "high", "medium"]);
    assert!(
        literal_json["suggested_next"]
            .as_array()
            .expect("suggested_next")
            .iter()
            .any(|s| s["command"] == "loct body 'utterance_id' --json"),
        "LSP literal response must preserve CLI suggested-next path: {json}"
    );
}

#[test]
fn build_literal_response_applies_rollup_slim_and_paging() {
    let literal = scan_files([("src/scribe.rs", LITERAL_SOURCE)], "utterance_id");
    let mut params = literal_params();
    params.group_by_file = true;
    params.count_only = true;
    params.offset = 1;
    params.limit = Some(2);

    let response = find::build_literal_response(literal, vec![], &params);
    let lit = response
        .literal_matches
        .as_ref()
        .expect("literal_matches present in literal mode");

    assert_eq!(response.total_matches, 4);
    assert_eq!(lit.total, 4);
    assert!(lit.slim);
    assert!(
        lit.occurrences.is_empty(),
        "count_only suppresses occurrence payload"
    );
    assert_eq!(
        lit.by_file.as_ref().expect("by_file rollup")[0].file,
        "src/scribe.rs"
    );
    let page = lit.page.as_ref().expect("literal page metadata");
    assert_eq!(page.offset, 1);
    assert_eq!(page.limit, 2);
    assert_eq!(page.returned, 2);
    assert!(page.has_more);
    assert_eq!(page.next_offset, Some(3));
}

#[test]
fn scan_literal_honors_whole_token_boundary() {
    let source = "let backdrop = 1; let css = \"overlay-backdrop\";";
    let loose = scan_files_with(
        [("src/app.tsx", source)],
        "backdrop",
        ScanOptions { whole_token: false },
    );
    let tight = scan_files_with(
        [("src/app.tsx", source)],
        "backdrop",
        ScanOptions { whole_token: true },
    );

    assert_eq!(loose.total, 2);
    assert_eq!(tight.total, 1);
    assert_eq!(tight.occurrences[0].occurrence_kind.as_str(), "identifier");
}

#[test]
fn non_literal_response_omits_literal_fields_from_json() {
    // Backward compatibility: existing modes never emit the literal keys.
    let response = find::build_response(fixture_results(), &fixture_params_single());
    let json = serde_json::to_value(&response).expect("serialize find response");
    assert!(
        json.get("literal_matches").is_none(),
        "non-literal response must not carry literal_matches: {json}"
    );
    assert!(json.get("literal_fuzzy").is_none());
}

#[test]
fn scan_literal_matches_shared_scanner_on_disk() {
    // Parity contract: the LSP scan_literal reads the same bytes and runs the
    // same scanner as `loct occurrences`, so its output is byte-for-byte equal
    // to a direct scan_files over the same content.
    let dir = tempfile::tempdir().expect("temp project");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
    std::fs::write(dir.path().join("src/scribe.rs"), LITERAL_SOURCE).expect("write scribe.rs");

    let mut snapshot = Snapshot::new(vec![dir.path().display().to_string()]);
    let file: FileAnalysis = serde_json::from_value(serde_json::json!({ "path": "src/scribe.rs" }))
        .expect("minimal FileAnalysis");
    snapshot.files.push(file);

    let scanned = find::scan_literal(
        &snapshot,
        Some(dir.path()),
        "utterance_id",
        ScanOptions { whole_token: false },
        FileScope::default(),
    );
    let expected = scan_files([("src/scribe.rs", LITERAL_SOURCE)], "utterance_id");

    assert_eq!(
        serde_json::to_value(&scanned).unwrap(),
        serde_json::to_value(&expected).unwrap(),
        "LSP literal scan must equal the shared scanner over the same bytes"
    );
}

#[test]
fn literal_scan_cache_matches_direct_scan_and_invalidates_on_edit() {
    // The cache must (1) return the same result as a direct scan and a stable
    // result across repeated calls on unchanged disk, and (2) invalidate when
    // the file changes on disk — even without a commit — so a saved edit is
    // never served stale (the cache keys on a file-set fingerprint, not a
    // commit-granular snapshot id).
    let dir = tempfile::tempdir().expect("temp project");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
    std::fs::write(dir.path().join("src/scribe.rs"), LITERAL_SOURCE).expect("write scribe.rs");

    let mut snapshot = Snapshot::new(vec![dir.path().display().to_string()]);
    let file: FileAnalysis = serde_json::from_value(serde_json::json!({ "path": "src/scribe.rs" }))
        .expect("minimal FileAnalysis");
    snapshot.files.push(file);

    let cache = find::LiteralScanCache::new();
    let scan = |c: &find::LiteralScanCache| {
        c.get_or_scan(
            &snapshot,
            Some(dir.path()),
            "utterance_id",
            ScanOptions { whole_token: false },
            FileScope::default(),
        )
    };
    let direct = || {
        find::scan_literal(
            &snapshot,
            Some(dir.path()),
            "utterance_id",
            ScanOptions { whole_token: false },
            FileScope::default(),
        )
    };

    // (1) Cache equals a direct scan; an immediate repeat (unchanged disk) is a
    // stable hit.
    let first = scan(&cache);
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(direct()).unwrap(),
        "cached scan must equal a direct scan"
    );
    assert_eq!(
        serde_json::to_value(scan(&cache)).unwrap(),
        serde_json::to_value(&first).unwrap(),
        "unchanged disk must return a stable result"
    );

    // (2) A saved edit (length changes -> fingerprint changes) invalidates the
    // entry: the next call reflects the NEW contents, never the stale page.
    std::fs::write(dir.path().join("src/scribe.rs"), "// utterance_id\n").expect("rewrite");
    let after_edit = scan(&cache);
    assert_eq!(
        serde_json::to_value(&after_edit).unwrap(),
        serde_json::to_value(direct()).unwrap(),
        "a saved edit must invalidate the cache and re-scan"
    );
    assert_ne!(
        serde_json::to_value(&after_edit).unwrap(),
        serde_json::to_value(&first).unwrap(),
        "post-edit result must differ from the pre-edit cached page"
    );
}

#[test]
fn scan_literal_honors_file_scope_and_preserves_range_metadata() {
    let dir = tempfile::tempdir().expect("temp project");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
    std::fs::write(
        dir.path().join("src/styles.css"),
        ".checkout-success { color: var(--checkout-success); }\n",
    )
    .expect("write styles");
    std::fs::write(
        dir.path().join("src/other.css"),
        ".checkout-success { opacity: 1; }\n",
    )
    .expect("write other");

    let mut snapshot = Snapshot::new(vec![dir.path().display().to_string()]);
    for path in ["src/styles.css", "src/other.css"] {
        let file: FileAnalysis = serde_json::from_value(serde_json::json!({ "path": path }))
            .expect("minimal FileAnalysis");
        snapshot.files.push(file);
    }

    let scanned = find::scan_literal(
        &snapshot,
        Some(dir.path()),
        "checkout-success",
        ScanOptions::default(),
        FileScope {
            file: Some("src/styles.css"),
        },
    );

    assert_eq!(scanned.files_matched, 1);
    assert_eq!(scanned.total, 2);
    assert!(
        scanned
            .occurrences
            .iter()
            .all(|o| o.file == "src/styles.css")
    );
    assert_eq!(scanned.occurrences[0].line, 1);
    assert_eq!(scanned.occurrences[0].column, 2);
    assert_eq!(scanned.occurrences[0].range.start.line, 1);
    assert_eq!(scanned.occurrences[0].range.start.column, 2);
    assert_eq!(scanned.occurrences[0].range.end.column, 18);
}

fn fixture_params_single() -> FindParams {
    FindParams {
        query: "Auth".into(),
        mode: "single".into(),
        lang: None,
        file: None,
        dead_only: false,
        exported_only: false,
        limit: None,
        project: None,
        cursor: None,
        chunk_size: None,
        whole_token: false,
        group_by_file: false,
        count_only: false,
        slim: false,
        offset: 0,
        symbol_id: None,
    }
}

#[test]
fn and_mode_zeroes_non_cross_match_buckets() {
    let mut results = fixture_results();
    // Inject a cross-match so it survives.
    results.cross_matches = vec![loctree::analyzer::search::CrossMatchFile {
        file: "src/auth.rs".into(),
        matched_terms: vec![],
    }];
    let params = FindParams {
        query: "Auth User".into(),
        mode: "and".into(),
        lang: None,
        file: None,
        dead_only: false,
        exported_only: false,
        limit: None,
        project: None,
        cursor: None,
        chunk_size: None,
        whole_token: false,
        group_by_file: false,
        count_only: false,
        slim: false,
        offset: 0,
        symbol_id: None,
    };
    let response = find::build_response(results, &params);
    assert!(response.symbol_matches.data.files.is_empty());
    assert!(response.param_matches.data.is_empty());
    assert!(response.semantic_matches.data.is_empty());
    assert!(response.suppression_matches.data.is_empty());
    assert_eq!(response.cross_matches.data.len(), 1);
}
