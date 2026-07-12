//! Integration tests for the `loctree/astQuery` LSP request (Plan 20).
//!
//! Unit tests next to `loctree-lsp::ast_query` cover params parsing,
//! glob scope, the live-document overlay, and capability shape. This
//! file completes Plan 20's last open acceptance criterion: at least
//! three *real* tree-sitter queries run end-to-end against on-disk
//! TS/JS/TSX fixtures, exercising the same `compute` path the LSP
//! handler calls.
//!
//! Each test stages the static fixtures from `tests/fixtures/ast_query/`
//! into a tempdir, builds a snapshot whose `files` list points at the
//! staged paths, and calls `loctree-lsp::ast_query::compute` directly.
//! That mirrors how `Backend::ast_query` invokes the handler in the
//! real server, minus the JSON-RPC envelope.

use std::fs;
use std::path::{Path, PathBuf};

use loctree::snapshot::Snapshot;
use loctree::types::FileAnalysis;
use loctree_lsp::AstQueryParams;
use loctree_lsp::ast_query::{self, AstQueryError, AstQueryScope};
use tempfile::TempDir;

const FIXTURE_FILES: &[(&str, &str)] = &[
    ("src/greet.ts", "typescript"),
    ("src/util.js", "javascript"),
    ("src/Button.tsx", "tsx"),
];

/// Stage every fixture under `loctree-lsp/tests/fixtures/ast_query/`
/// into a fresh tempdir and return a `Snapshot` whose `files` table
/// matches what a real `loct scan` would have produced for that tree.
fn setup_fixture() -> (TempDir, Snapshot) {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("ast_query");

    let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
    for (rel, language) in FIXTURE_FILES {
        let src = fixture_root.join(rel);
        let dst = root.join(rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).expect("mkdir fixture parent");
        }
        fs::copy(&src, &dst).unwrap_or_else(|err| {
            panic!(
                "copy fixture `{}` -> `{}`: {err}",
                src.display(),
                dst.display()
            )
        });
        snapshot.files.push(file_analysis(rel, language));
    }
    (temp, snapshot)
}

fn file_analysis(path: &str, language: &str) -> FileAnalysis {
    FileAnalysis {
        path: path.to_string(),
        language: language.to_string(),
        ..Default::default()
    }
}

fn params(language: &str, query: &str) -> AstQueryParams {
    serde_json::from_value(serde_json::json!({
        "language": language,
        "query": query,
    }))
    .expect("params parse")
}

fn params_with_glob(language: &str, query: &str, glob: &str) -> AstQueryParams {
    AstQueryParams {
        language: language.into(),
        query: query.into(),
        scope: AstQueryScope {
            paths: None,
            glob: Some(glob.into()),
        },
        limit: None,
        project: None,
    }
}

fn root_path(temp: &TempDir) -> PathBuf {
    temp.path().to_path_buf()
}

/// (1) Real tree-sitter query against the TypeScript fixture.
/// Captures every `function_declaration` name identifier — the canonical
/// "find functions by name" pattern. Asserts match count, capture name,
/// and that the byte range materializes the expected snippet.
#[test]
fn typescript_function_declaration_query_returns_named_capture() {
    let (temp, snapshot) = setup_fixture();
    let root = root_path(&temp);

    let params = params_with_glob(
        "typescript",
        "(function_declaration name: (identifier) @name)",
        "**/*.ts",
    );
    let response = ast_query::compute(&snapshot, &root, &params).expect("ts function query");

    assert_eq!(
        response.total, 1,
        "exactly one function_declaration in greet.ts; got {:?}",
        response.matches
    );
    assert!(!response.truncated);
    let m = &response.matches[0];
    assert_eq!(m.file, "src/greet.ts");
    assert_eq!(m.capture_name, "name");
    assert_eq!(m.snippet, "greet");
    assert!(m.byte_end > m.byte_start, "byte range must be non-empty");
    assert!(m.line >= 1 && m.column >= 1, "1-based positions");
}

/// (2) Real tree-sitter query against the JavaScript fixture.
/// Captures `lexical_declaration` (the `export const VERSION = ...`
/// node). Glob restricts scope to JS so the capture comes from `util.js`
/// only, proving the path filter cooperates with a typed language.
#[test]
fn javascript_lexical_declaration_query_with_glob_scope() {
    let (temp, snapshot) = setup_fixture();
    let root = root_path(&temp);

    let params = params_with_glob("javascript", "(lexical_declaration) @decl", "**/*.js");
    let response = ast_query::compute(&snapshot, &root, &params).expect("js lex query");

    assert_eq!(response.total, 1, "one const in util.js");
    let m = &response.matches[0];
    assert_eq!(m.file, "src/util.js");
    assert_eq!(m.capture_name, "decl");
    assert!(
        m.snippet.contains("VERSION") && m.snippet.contains("1.0.0"),
        "snippet must materialize the declaration body; got {:?}",
        m.snippet
    );
}

/// (3) Real tree-sitter query against the TSX fixture. JSX nodes only
/// parse under the `tsx` grammar, so this also proves language
/// dispatch routes TSX away from the plain `typescript` parser.
#[test]
fn tsx_jsx_element_query_captures_button_markup() {
    let (temp, snapshot) = setup_fixture();
    let root = root_path(&temp);

    let params = params_with_glob("tsx", "(jsx_element) @element", "**/*.tsx");
    let response = ast_query::compute(&snapshot, &root, &params).expect("tsx jsx query");

    assert_eq!(response.total, 1, "one <button> element in Button.tsx");
    let m = &response.matches[0];
    assert_eq!(m.file, "src/Button.tsx");
    assert_eq!(m.capture_name, "element");
    assert!(
        m.snippet.contains("<button>") && m.snippet.contains("</button>"),
        "snippet must include the JSX element source; got {:?}",
        m.snippet
    );
}

/// (4) Curated `@library/lexical_declarations` query — the only library
/// shipped at Stage 1 of Plan 20. Confirms it loads from
/// `loctree-lsp/queries/typescript/lexical_declarations.scm` and the
/// capture name advertised in the .scm file (`declaration`) round-trips
/// through the response.
#[test]
fn library_query_lexical_declarations_resolves_for_typescript() {
    let (temp, snapshot) = setup_fixture();
    let root = root_path(&temp);

    let params = params_with_glob("typescript", "@library/lexical_declarations", "**/*.ts");
    let response = ast_query::compute(&snapshot, &root, &params).expect("library query");

    // greet.ts has two `export const` declarations.
    assert_eq!(
        response.total, 2,
        "two lexical_declarations in greet.ts; got {:?}",
        response.matches
    );
    for m in &response.matches {
        assert_eq!(m.file, "src/greet.ts");
        assert_eq!(
            m.capture_name, "declaration",
            "library .scm file declares a `@declaration` capture"
        );
    }
}

/// (5) `language: "auto"` dispatch — the same query runs across every
/// supported parser, so every fixture file contributes matches. With
/// the curated library the .scm exists for JS, TS, and TSX, exercising
/// the per-language library-loading branch under the auto-dispatch
/// path in one shot.
#[test]
fn language_auto_dispatch_covers_every_supported_fixture() {
    let (temp, snapshot) = setup_fixture();
    let root = root_path(&temp);

    let params = params("auto", "@library/lexical_declarations");
    let response = ast_query::compute(&snapshot, &root, &params).expect("auto dispatch");

    let files: std::collections::BTreeSet<_> =
        response.matches.iter().map(|m| m.file.clone()).collect();
    assert!(
        files.contains("src/greet.ts"),
        "auto dispatch must hit TS fixture; saw {:?}",
        files
    );
    assert!(
        files.contains("src/util.js"),
        "auto dispatch must hit JS fixture; saw {:?}",
        files
    );
    assert!(
        files.contains("src/Button.tsx"),
        "auto dispatch must hit TSX fixture; saw {:?}",
        files
    );
    // greet.ts: 2 lexical decls, util.js: 1, Button.tsx: 1 ⇒ 4 matches total.
    assert_eq!(
        response.total, 4,
        "expected 4 lexical decls across fixtures"
    );
    for m in &response.matches {
        assert_eq!(m.capture_name, "declaration");
    }
}

/// (6) Typed errors — both `query_compile_error` and
/// `language_unsupported` round-trip the kind tag the LSP layer maps
/// onto JSON-RPC error data.
#[test]
fn typed_errors_for_bad_query_and_unsupported_language() {
    let (temp, snapshot) = setup_fixture();
    let root = root_path(&temp);

    // Broken s-expression triggers tree-sitter's compile error.
    let bad_query = params_with_glob("typescript", "(this is not balanced", "**/*.ts");
    let err = ast_query::compute(&snapshot, &root, &bad_query)
        .expect_err("malformed query must fail compile");
    assert_eq!(err.kind(), "query_compile_error");
    if let AstQueryError::QueryCompileError { line, col, msg } = err {
        assert!(line >= 1 && col >= 1, "1-based location");
        assert!(!msg.is_empty(), "compile error must carry a message");
    } else {
        panic!("expected QueryCompileError variant");
    }

    // `ruby` is not in `loctree-ast`'s default Parsers (js/ts/tsx/python) — this
    // is the canonical `language_unsupported` shape. (Python used to sit here, but
    // it gained a real extractor — see W1-01.)
    let unsupported = params("ruby", "(program) @m");
    let err = ast_query::compute(&snapshot, &root, &unsupported)
        .expect_err("ruby has no parser registered");
    assert_eq!(err.kind(), "language_unsupported");
    assert!(matches!(
        err,
        AstQueryError::LanguageUnsupported { ref language } if language == "ruby"
    ));
}
