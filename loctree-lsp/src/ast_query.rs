//! Custom LSP request: `loctree/astQuery` (Plan 20).
//!
//! MVP scope: read-only tree-sitter queries over files already known to the
//! current snapshot. As of P0 Stage 2 the handler also consults the
//! [`crate::live_ast::LiveAstStore`] before falling back to disk — when an
//! editor has the file open, the query runs against the live (possibly
//! unsaved) tree-sitter tree from `loctree-ast`. Closed files keep the
//! original on-disk reparse path.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::{Glob, GlobSet, GlobSetBuilder};
use loctree::snapshot::Snapshot;
use loctree_ast::{LoctreeTree, Parsers, Query, QueryCursor, StreamingIterator};
use serde::{Deserialize, Serialize};

use crate::live_ast::{LiveAstStore, LiveDocument};

const DEFAULT_LIMIT: usize = 100;
const MAX_SNIPPET_CHARS: usize = 200;

/// Parameters for `loctree/astQuery`.
#[derive(Debug, Deserialize)]
pub struct AstQueryParams {
    /// `auto` or a concrete `loctree-ast` language id (`javascript`,
    /// `typescript`, `tsx`).
    #[serde(default = "default_language")]
    pub language: String,
    /// Tree-sitter query text, or `@library/<name>` for a curated query.
    pub query: String,
    /// Optional file scope. Empty scope means all snapshot files supported by
    /// `loctree-ast`.
    #[serde(default)]
    pub scope: AstQueryScope,
    /// Maximum capture entries returned. Defaults to 100.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Workspace project root override, routed by the backend.
    #[serde(default)]
    pub project: Option<PathBuf>,
}

fn default_language() -> String {
    "auto".into()
}

/// Scope selector for `loctree/astQuery`.
#[derive(Debug, Default, Deserialize)]
pub struct AstQueryScope {
    /// Snapshot-relative or absolute file paths.
    #[serde(default)]
    pub paths: Option<Vec<PathBuf>>,
    /// Glob matched against snapshot-relative paths.
    #[serde(default)]
    pub glob: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AstQueryMatch {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub byte_start: usize,
    pub byte_end: usize,
    pub capture_name: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AstQueryResponse {
    pub matches: Vec<AstQueryMatch>,
    pub total: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstQueryError {
    QueryCompileError {
        line: usize,
        col: usize,
        msg: String,
    },
    LanguageUnsupported {
        language: String,
    },
    ScopeNotFound {
        reason: String,
    },
}

impl AstQueryError {
    pub fn kind(&self) -> &'static str {
        match self {
            AstQueryError::QueryCompileError { .. } => "query_compile_error",
            AstQueryError::LanguageUnsupported { .. } => "language_unsupported",
            AstQueryError::ScopeNotFound { .. } => "scope_not_found",
        }
    }
}

pub fn compute(
    snapshot: &Snapshot,
    workspace_root: &Path,
    params: &AstQueryParams,
) -> Result<AstQueryResponse, AstQueryError> {
    compute_with_live(snapshot, workspace_root, params, None)
}

/// Variant of [`compute`] that consults a live document store before
/// reparsing files from disk. When an editor has the file open, the
/// query runs against the live tree-sitter tree managed by
/// [`crate::live_ast`]; closed files keep the on-disk reparse path.
///
/// `live` is `None` from unit tests or callers that don't want to
/// share the LSP backend's store (e.g. CLI parity tests).
pub fn compute_with_live(
    snapshot: &Snapshot,
    workspace_root: &Path,
    params: &AstQueryParams,
    live: Option<&LiveAstStore>,
) -> Result<AstQueryResponse, AstQueryError> {
    let parsers = Parsers::new_default();
    let language = params.language.trim();
    let auto_language = language.eq_ignore_ascii_case("auto") || language.is_empty();
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);

    if !auto_language && parsers.lookup(language).is_none() {
        return Err(AstQueryError::LanguageUnsupported {
            language: language.to_string(),
        });
    }

    let scope = collect_scope(snapshot, workspace_root, &params.scope)?;
    let mut matches = Vec::new();
    let mut total = 0usize;
    let mut live_files = 0usize;
    let mut disk_files = 0usize;

    for rel_path in scope {
        let parser = match parsers.for_path(Path::new(&rel_path)) {
            Some(parser) => parser,
            None => continue,
        };
        if !auto_language && parser.lang_id() != normalize_requested_language(language) {
            continue;
        }

        let query_source = resolve_query_source(parser.lang_id(), &params.query)?;
        let query = Query::new(&parser.language(), &query_source).map_err(|err| {
            AstQueryError::QueryCompileError {
                line: err.row + 1,
                col: err.column + 1,
                msg: err.message,
            }
        })?;

        let live_doc = live.and_then(|store| store.get_for_path(workspace_root, &rel_path));
        let payload =
            match TreePayload::resolve(live_doc.as_ref(), &parsers, workspace_root, &rel_path) {
                Some(p) => p,
                None => continue,
            };
        match payload.source_kind() {
            TreeSourceKind::LiveDocument => live_files += 1,
            TreeSourceKind::Disk => disk_files += 1,
        }

        let capture_names = query.capture_names();
        let mut cursor = QueryCursor::new();
        let mut query_matches =
            cursor.matches(&query, payload.tree().tree.root_node(), payload.bytes());
        while let Some(query_match) = query_matches.next() {
            for capture in query_match.captures {
                total += 1;
                if matches.len() >= limit {
                    continue;
                }
                let node = capture.node;
                let start = node.start_position();
                let range = node.byte_range();
                let capture_name = capture_names
                    .get(capture.index as usize)
                    .copied()
                    .unwrap_or("capture")
                    .to_string();
                matches.push(AstQueryMatch {
                    file: rel_path.clone(),
                    line: start.row + 1,
                    column: start.column + 1,
                    byte_start: range.start,
                    byte_end: range.end,
                    capture_name,
                    snippet: snippet(payload.bytes(), range.start, range.end),
                });
            }
        }
    }

    let _ = (live_files, disk_files); // currently surfaced via tracing only.
    tracing::debug!(
        target: "loctree-lsp::ast_query",
        live_files = live_files,
        disk_files = disk_files,
        total_matches = total,
        truncated = total > matches.len(),
        "ast_query executed"
    );

    Ok(AstQueryResponse {
        truncated: total > matches.len(),
        total,
        matches,
    })
}

/// Source the query is reading: either the editor's live tree-sitter
/// tree (Plan 17 MVP) or a freshly reparsed on-disk file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TreeSourceKind {
    LiveDocument,
    Disk,
}

/// Owned-or-borrowed payload carrying the tree-sitter tree and the
/// source bytes the query reads. `LiveDocument` keeps the inner
/// [`Arc<LiveDocument>`] alive for the duration of the query so the
/// borrowed `&LoctreeTree` and `&[u8]` stay valid.
enum TreePayload {
    LiveDocument(Arc<LiveDocument>),
    Disk { tree: LoctreeTree },
}

impl TreePayload {
    fn resolve(
        live: Option<&Arc<LiveDocument>>,
        parsers: &Parsers,
        workspace_root: &Path,
        rel_path: &str,
    ) -> Option<Self> {
        if let Some(doc) = live {
            return Some(TreePayload::LiveDocument(Arc::clone(doc)));
        }
        let abs_path = workspace_root.join(rel_path);
        let source = fs::read(&abs_path).ok()?;
        let tree = parsers.parse_path(Path::new(rel_path), &source).ok()?;
        Some(TreePayload::Disk { tree })
    }

    fn tree(&self) -> &LoctreeTree {
        match self {
            TreePayload::LiveDocument(doc) => &doc.tree,
            TreePayload::Disk { tree } => tree,
        }
    }

    fn bytes(&self) -> &[u8] {
        match self {
            TreePayload::LiveDocument(doc) => &doc.tree.source,
            TreePayload::Disk { tree } => &tree.source,
        }
    }

    fn source_kind(&self) -> TreeSourceKind {
        match self {
            TreePayload::LiveDocument(_) => TreeSourceKind::LiveDocument,
            TreePayload::Disk { .. } => TreeSourceKind::Disk,
        }
    }
}

pub fn to_lsp_error(err: AstQueryError) -> tower_lsp::jsonrpc::Error {
    use tower_lsp::jsonrpc::{Error, ErrorCode};

    let data = match &err {
        AstQueryError::QueryCompileError { line, col, msg } => serde_json::json!({
            "kind": err.kind(),
            "line": line,
            "col": col,
            "message": msg,
        }),
        AstQueryError::LanguageUnsupported { language } => serde_json::json!({
            "kind": err.kind(),
            "language": language,
        }),
        AstQueryError::ScopeNotFound { reason } => serde_json::json!({
            "kind": err.kind(),
            "reason": reason,
        }),
    };

    Error {
        code: ErrorCode::InvalidParams,
        message: err.kind().into(),
        data: Some(data),
    }
}

pub fn capability_json() -> serde_json::Value {
    let parsers = Parsers::new_default();
    serde_json::json!({
        "available": true,
        "languages": parsers.language_ids(),
        "scope": "snapshot_files",
        "liveDocumentCache": true,
        "liveDocumentCacheNote": "open documents (JS/TS/TSX) run against the editor's \
                                  unsaved tree-sitter tree via `crate::live_ast`; closed \
                                  files reparse from disk through `loctree-ast`.",
    })
}

fn collect_scope(
    snapshot: &Snapshot,
    workspace_root: &Path,
    scope: &AstQueryScope,
) -> Result<Vec<String>, AstQueryError> {
    let path_filter = scope.paths.as_ref().map(|paths| {
        paths
            .iter()
            .map(|path| normalize_scope_path(workspace_root, path))
            .collect::<HashSet<_>>()
    });
    let glob_filter = compile_glob(scope.glob.as_deref())?;

    let mut files = Vec::new();
    for file in &snapshot.files {
        let rel = file.path.trim_start_matches("./").to_string();
        if let Some(paths) = &path_filter {
            let abs = workspace_root.join(&rel).to_string_lossy().into_owned();
            if !paths.contains(&rel) && !paths.contains(&abs) {
                continue;
            }
        }
        if let Some(glob) = &glob_filter
            && !glob.is_match(&rel)
        {
            continue;
        }
        files.push(rel);
    }

    if files.is_empty() {
        return Err(AstQueryError::ScopeNotFound {
            reason: "no snapshot files matched the requested astQuery scope".into(),
        });
    }
    Ok(files)
}

fn compile_glob(pattern: Option<&str>) -> Result<Option<GlobSet>, AstQueryError> {
    let Some(pattern) = pattern else {
        return Ok(None);
    };
    let glob = Glob::new(pattern).map_err(|err| AstQueryError::ScopeNotFound {
        reason: format!("invalid glob `{pattern}`: {err}"),
    })?;
    let mut builder = GlobSetBuilder::new();
    builder.add(glob);
    builder
        .build()
        .map(Some)
        .map_err(|err| AstQueryError::ScopeNotFound {
            reason: format!("invalid glob `{pattern}`: {err}"),
        })
}

fn normalize_scope_path(workspace_root: &Path, path: &Path) -> String {
    if path.is_absolute() {
        path.to_string_lossy().into_owned()
    } else {
        let rel = path.to_string_lossy().trim_start_matches("./").to_string();
        workspace_root.join(&rel).to_string_lossy().into_owned()
    }
}

fn resolve_query_source(lang: &str, query: &str) -> Result<String, AstQueryError> {
    let Some(name) = query.strip_prefix("@library/") else {
        return Ok(query.to_string());
    };
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(AstQueryError::ScopeNotFound {
            reason: format!("invalid astQuery library name `{name}`"),
        });
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("queries")
        .join(lang)
        .join(format!("{name}.scm"));
    fs::read_to_string(&path).map_err(|_| AstQueryError::ScopeNotFound {
        reason: format!("query library `{name}` is not available for {lang}"),
    })
}

fn normalize_requested_language(language: &str) -> &str {
    match language {
        "js" | "jsx" | "node" => "javascript",
        "ts" => "typescript",
        other => other,
    }
}

fn snippet(source: &[u8], start: usize, end: usize) -> String {
    let start = start.min(source.len());
    let end = end.min(source.len()).max(start);
    let mut out = String::from_utf8_lossy(&source[start..end]).into_owned();
    if out.chars().count() > MAX_SNIPPET_CHARS {
        out = out.chars().take(MAX_SNIPPET_CHARS).collect();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use loctree::snapshot::Snapshot;
    use loctree::types::FileAnalysis;

    fn file(path: &str, language: &str) -> FileAnalysis {
        serde_json::from_value(serde_json::json!({
            "path": path,
            "language": language
        }))
        .expect("file analysis")
    }

    #[test]
    fn params_deserialize_minimal() {
        let json = serde_json::json!({
            "language": "typescript",
            "query": "(lexical_declaration) @decl"
        });
        let params: AstQueryParams = serde_json::from_value(json).expect("params parse");
        assert_eq!(params.language, "typescript");
        assert_eq!(params.query, "(lexical_declaration) @decl");
        assert!(params.scope.paths.is_none());
        assert!(params.scope.glob.is_none());
        assert!(params.limit.is_none());
    }

    #[test]
    fn query_matches_typescript_fixture() {
        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(src.join("demo.ts"), "export const answer: number = 42;\n").expect("write");

        let mut snapshot = Snapshot::new(vec![temp.path().display().to_string()]);
        snapshot.files.push(file("src/demo.ts", "typescript"));

        let params = AstQueryParams {
            language: "typescript".into(),
            query: "(lexical_declaration) @decl".into(),
            scope: AstQueryScope::default(),
            limit: None,
            project: None,
        };
        let response = compute(&snapshot, temp.path(), &params).expect("ast query");

        assert_eq!(response.total, 1);
        assert_eq!(response.matches[0].file, "src/demo.ts");
        assert_eq!(response.matches[0].capture_name, "decl");
        assert!(response.matches[0].snippet.contains("answer"));
    }

    #[test]
    fn unsupported_language_is_typed_error() {
        let snapshot = Snapshot::new(vec!["src".into()]);
        // `ruby` has no parser in loctree-ast Parsers::new_default (js/ts/tsx/python),
        // so it still exercises the typed language_unsupported path. (Python used to
        // sit here, but it gained a real extractor — see W1-01.)
        let params = AstQueryParams {
            language: "ruby".into(),
            query: "(program) @m".into(),
            scope: AstQueryScope::default(),
            limit: None,
            project: None,
        };

        let err = compute(&snapshot, Path::new("."), &params).expect_err("typed error");
        assert_eq!(err.kind(), "language_unsupported");
    }

    #[test]
    fn scope_glob_filters_snapshot_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(src.join("a.ts"), "const a = 1;\n").expect("write a");
        fs::write(src.join("b.js"), "const b = 2;\n").expect("write b");

        let mut snapshot = Snapshot::new(vec![temp.path().display().to_string()]);
        snapshot.files.push(file("src/a.ts", "typescript"));
        snapshot.files.push(file("src/b.js", "javascript"));

        let params = AstQueryParams {
            language: "auto".into(),
            query: "(lexical_declaration) @decl".into(),
            scope: AstQueryScope {
                paths: None,
                glob: Some("**/*.ts".into()),
            },
            limit: None,
            project: None,
        };
        let response = compute(&snapshot, temp.path(), &params).expect("ast query");

        assert_eq!(response.total, 1);
        assert_eq!(response.matches[0].file, "src/a.ts");
    }

    #[test]
    fn curated_library_query_is_available_for_typescript() {
        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(src.join("demo.ts"), "const answer = 42;\n").expect("write");

        let mut snapshot = Snapshot::new(vec![temp.path().display().to_string()]);
        snapshot.files.push(file("src/demo.ts", "typescript"));

        let params = AstQueryParams {
            language: "typescript".into(),
            query: "@library/lexical_declarations".into(),
            scope: AstQueryScope::default(),
            limit: None,
            project: None,
        };
        let response = compute(&snapshot, temp.path(), &params).expect("library query");

        assert_eq!(response.total, 1);
        assert_eq!(response.matches[0].capture_name, "declaration");
    }

    #[test]
    fn capability_reports_available_languages_and_live_cache() {
        let cap = capability_json();
        assert_eq!(cap["available"], true);
        // Plan 17 MVP: live cache is now wired for open JS/TS/TSX
        // documents. The note keeps the boundary visible — closed
        // files still reparse from disk.
        assert_eq!(cap["liveDocumentCache"], true);
        assert!(
            cap["liveDocumentCacheNote"]
                .as_str()
                .expect("note string")
                .contains("live_ast")
        );
        assert!(
            cap["languages"]
                .as_array()
                .expect("languages array")
                .iter()
                .any(|lang| lang == "typescript")
        );
    }

    #[test]
    fn live_document_overrides_disk_source() {
        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        // On-disk version has zero declarations.
        fs::write(src.join("demo.ts"), "// nothing here\n").expect("write disk");

        let mut snapshot = Snapshot::new(vec![temp.path().display().to_string()]);
        snapshot.files.push(file("src/demo.ts", "typescript"));

        // Live (unsaved) buffer carries one lexical declaration.
        let store = LiveAstStore::new();
        let abs = src.join("demo.ts");
        let uri = tower_lsp::lsp_types::Url::from_file_path(&abs).expect("uri");
        store
            .update(&uri, 1, "export const answer: number = 42;\n")
            .expect("live parse");

        let params = AstQueryParams {
            language: "typescript".into(),
            query: "(lexical_declaration) @decl".into(),
            scope: AstQueryScope::default(),
            limit: None,
            project: None,
        };
        let with_live = compute_with_live(&snapshot, temp.path(), &params, Some(&store))
            .expect("ast query with live");
        assert_eq!(with_live.total, 1, "live tree carries the declaration");
        assert!(with_live.matches[0].snippet.contains("answer"));

        // Without the live store the same snapshot reports zero matches
        // because the on-disk file is empty — proves the live path is
        // doing real work.
        let from_disk =
            compute_with_live(&snapshot, temp.path(), &params, None).expect("ast query disk");
        assert_eq!(from_disk.total, 0);
    }

    #[test]
    fn live_store_without_open_uri_falls_back_to_disk() {
        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(src.join("disk_only.ts"), "export const a = 1;\n").expect("write disk");

        let mut snapshot = Snapshot::new(vec![temp.path().display().to_string()]);
        snapshot.files.push(file("src/disk_only.ts", "typescript"));

        // Empty store — every snapshot file falls through to disk read.
        let store = LiveAstStore::new();
        let params = AstQueryParams {
            language: "typescript".into(),
            query: "(lexical_declaration) @decl".into(),
            scope: AstQueryScope::default(),
            limit: None,
            project: None,
        };
        let response = compute_with_live(&snapshot, temp.path(), &params, Some(&store))
            .expect("ast query falls back");
        assert_eq!(response.total, 1);
    }
}
