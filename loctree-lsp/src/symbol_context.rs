//! Custom LSP request: `loctree/symbolContext`
//!
//! The keystone of the Context-King surface. Given a `file` + `position`
//! (and an optional `symbol` hint), it returns one bounded, literal context
//! pack for the symbol under the cursor:
//!
//! * **export / internal status** — computed from the SYMBOL IDENTITY at
//!   `file + position` via the snapshot lookup that `hover.rs` already uses
//!   (`ExportSymbol` / `ImplMethod` / `LocalSymbol` on the matching file/line).
//!   It is **not** derived from `find`'s literal `dead_status`, which is a stub
//!   (`is_exported: false` hardcoded) in literal mode. `internal` means the
//!   symbol resolved in this file and is not exported.
//! * **body** — the bounded source body for the symbol, reusing
//!   [`loctree::body::query_symbol_body`] with `file` disambiguation so a common
//!   name (`parse` / `run` / `load`) resolves to the definition in THIS file,
//!   truncated to `body_max_lines`.
//! * **occurrences** — literal identifier-boundary occurrences for the symbol,
//!   reusing the shared occurrences scanner (the same `find::scan_literal` path),
//!   with `total`, `same_file_total`, and `occurrence_limit` pagination.
//! * **parent_context** — best-effort enclosing function/class name from the
//!   live tree-sitter tree. Never hard-fails: missing → `source: "unavailable"`.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::path::PathBuf;

use loctree::analyzer::occurrences::{OccurrenceResults, ScanOptions};
use loctree::snapshot::Snapshot;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tower_lsp::lsp_types::{Position, Range};

/// Default body line cap when the caller omits `body_max_lines`.
pub const DEFAULT_BODY_MAX_LINES: usize = 80;
/// Default occurrence page size when the caller omits `occurrence_limit`.
pub const DEFAULT_OCCURRENCE_LIMIT: usize = 50;

/// 0-based LSP-style position. Mirrors `tower_lsp::lsp_types::Position` but
/// re-declared with `JsonSchema` so the capability advertisement can publish a
/// schema for the params type.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
pub struct SymbolPosition {
    /// 0-based line.
    pub line: u32,
    /// 0-based character offset.
    pub character: u32,
}

impl From<SymbolPosition> for Position {
    fn from(p: SymbolPosition) -> Self {
        Position {
            line: p.line,
            character: p.character,
        }
    }
}

/// Parameters for `loctree/symbolContext`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SymbolContextParams {
    /// Active file (workspace-relative path, or any suffix the snapshot stores).
    pub file: String,
    /// Cursor position. Symbol identity is resolved from `file + position`.
    pub position: SymbolPosition,
    /// Optional symbol-name hint. When omitted the identity is taken from the
    /// declaration sitting on `position.line` in `file`.
    #[serde(default)]
    pub symbol: Option<String>,
    /// Maximum body source lines. Defaults to [`DEFAULT_BODY_MAX_LINES`].
    #[serde(default)]
    pub body_max_lines: Option<usize>,
    /// Occurrence page size. Defaults to [`DEFAULT_OCCURRENCE_LIMIT`].
    #[serde(default)]
    pub occurrence_limit: Option<usize>,
    /// When true, return only same-file occurrences (cheap hover path).
    #[serde(default)]
    pub same_file_only: bool,
    /// Zero-based occurrence offset for paged output.
    #[serde(default)]
    pub offset: usize,
    /// Tighter literal boundaries (treat `-` as token-internal). Mirrors
    /// `loct occurrences --whole-token`.
    #[serde(default)]
    pub whole_token: bool,
    /// Workspace project root override. Reserved for Plan 13
    /// (multi-workspace context); ignored in single-workspace mode.
    #[serde(default)]
    pub project: Option<PathBuf>,
}

impl SymbolContextParams {
    /// Resolved body line cap.
    pub fn body_max_lines_resolved(&self) -> usize {
        self.body_max_lines
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_BODY_MAX_LINES)
    }

    /// Resolved occurrence page size.
    pub fn occurrence_limit_resolved(&self) -> usize {
        self.occurrence_limit
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_OCCURRENCE_LIMIT)
    }
}

/// Bounded body sub-object.
#[derive(Debug, Clone, Serialize)]
pub struct BodyContext {
    /// Bounded source text (already truncated to `body_max_lines`).
    pub source: String,
    /// 1-based start line of the body.
    pub start_line: usize,
    /// 1-based end line actually returned (inclusive).
    pub end_line: usize,
    /// True if the body exceeded `body_max_lines` and was truncated.
    pub truncated: bool,
    /// Total lines the full body spans (pre-cap).
    pub total_lines: usize,
}

/// A single literal occurrence in the response.
#[derive(Debug, Clone, Serialize)]
pub struct OccurrenceView {
    /// File path (as stored in the snapshot).
    pub file: String,
    /// 1-based line.
    pub line: usize,
    /// 1-based column.
    pub column: usize,
    /// Full source line context (trimmed).
    pub context: String,
    /// Conservative single-line classification of this site.
    pub kind: String,
}

/// Occurrences sub-object with same-file totals + pagination.
#[derive(Debug, Clone, Serialize)]
pub struct OccurrencesContext {
    /// Total literal occurrences across the searched scope.
    pub total: usize,
    /// Total literal occurrences in the requested `file` only.
    pub same_file_total: usize,
    /// Occurrences returned in this page.
    pub returned: Vec<OccurrenceView>,
    /// Whether more occurrences exist after this page.
    pub has_more: bool,
    /// Offset to request for the next page. Omitted on the final page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
}

/// Best-effort enclosing-symbol context.
#[derive(Debug, Clone, Serialize)]
pub struct ParentContext {
    /// Enclosing function/class name, if resolvable from the live tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Provenance: `"live_ast"` when resolved, `"unavailable"` otherwise.
    pub source: &'static str,
}

impl ParentContext {
    /// The honest "we could not resolve a parent" value. Never a hard error.
    pub fn unavailable() -> Self {
        ParentContext {
            name: None,
            source: "unavailable",
        }
    }
}

/// `loctree/symbolContext` response. Matches the frozen contract exactly.
#[derive(Debug, Clone, Serialize)]
pub struct SymbolContextResponse {
    /// Resolved symbol name.
    pub symbol: String,
    /// File the request anchored on (echoed from params).
    pub file: String,
    /// Range of the symbol declaration, when a line was resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    /// Symbol kind (`function`, `class`, `const`, …) when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `Some(true)` when the symbol is exported from `file`, `Some(false)` when
    /// it resolved internal, `None` when identity could not be resolved.
    pub exported: Option<bool>,
    /// `Some(true)` when the symbol resolved in `file` and is NOT exported.
    pub internal: Option<bool>,
    /// Workspace-relative DECLARING file, set ONLY when the symbol was resolved
    /// CROSS-FILE through the import graph (the cursor sat on a usage of an
    /// imported symbol whose declaration lives in another file). `None` when the
    /// symbol resolved locally in `file`, or stayed unresolved. The UI uses this
    /// to label "defined in <D>" and to know `body` came from `D`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defined_in: Option<String>,
    /// Bounded body, when a definition was located IN the requested file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyContext>,
    /// Set when `body` is absent because the symbol resolves only in OTHER
    /// files (`"not_found_in_file"`). We never surface a cross-file body, so
    /// the UI can show "no body in this file" instead of the wrong definition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_error: Option<String>,
    /// Literal occurrences with same-file totals + pagination.
    pub occurrences: OccurrencesContext,
    /// Best-effort enclosing function/class context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_context: Option<ParentContext>,
}

/// Resolved symbol identity from the snapshot at `file + position`.
///
/// This is the keystone correctness boundary: export/internal status is read
/// from the matching `ExportSymbol` / `ImplMethod` / `LocalSymbol` on the
/// cursor line, reusing the same path-match + line-match the hover provider
/// uses (`SnapshotState::find_export_at_position`). It is NOT taken from
/// `find` literal's `dead_status` stub.
#[derive(Debug, Clone)]
pub struct SymbolIdentity {
    /// Resolved symbol name.
    pub name: String,
    /// 1-based declaration line, when known.
    pub line: Option<usize>,
    /// Symbol kind, when known.
    pub kind: Option<String>,
    /// True when the symbol is exported from the file.
    pub exported: bool,
    /// True when the symbol resolved internal (resolved && not exported).
    pub internal: bool,
    /// Workspace-relative declaring file, set ONLY when the identity was
    /// resolved CROSS-FILE through the import graph (the cursor was on a usage
    /// of an imported symbol whose declaration lives in another file). `None`
    /// when the symbol resolved in the requested file, or stayed unresolved.
    pub defined_in: Option<String>,
}

/// Normalize a snapshot path for suffix-matching (strip `./` and leading `/`).
fn normalize_path(path: &str) -> String {
    path.trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

/// True when two paths match by equality or suffix, mirroring hover/snapshot.
fn paths_match(a: &str, b: &str) -> bool {
    let a = normalize_path(a);
    let b = normalize_path(b);
    a == b || a.ends_with(&b) || b.ends_with(&a)
}

/// Per-file declaration lookup: resolve a symbol's identity from the declaration
/// facts of ONE file (`exports` / `impl_methods` / `local_symbols`).
///
/// Priority order:
/// 1. an `ExportSymbol` on `target_line` (→ exported) — same as
///    `SnapshotState::find_export_at_position`.
/// 2. an `ImplMethod` on `target_line` (exported iff `Visibility::Public`).
/// 3. a `LocalSymbol` on `target_line` (exported iff `is_exported`).
/// 4. when a `name_hint` is supplied and no line matched, fall back to a
///    name-only export/method/local scan within THIS file so the badge is still
///    honest.
///
/// `target_line` is the 1-based declaration line to prefer (the cursor line for
/// same-file lookups); pass `None` to skip the line-priority step and go
/// straight to the name-only scan (cross-file declaring-file lookups, where the
/// cursor line belongs to the USING file, not the declaring file).
///
/// `name_hint` filters every step by exact name. For same-file lookups it is the
/// optional caller hint; for cross-file lookups it is the ORIGINAL imported name
/// (`ImportSymbol.name`, never the alias) so we look up the real declaration.
///
/// `internal = resolved && !exported`. `defined_in` is left `None` here — the
/// caller decides whether the resolution was same-file (`None`) or cross-file
/// (`Some(declaring_file)`).
fn resolve_in_file(
    file_analysis: &loctree::types::FileAnalysis,
    target_line: Option<usize>,
    name_hint: Option<&str>,
) -> Option<SymbolIdentity> {
    let name_matches = |name: &str| name_hint.map(|h| h == name).unwrap_or(true);

    // Line-priority steps (skipped when target_line is None).
    if let Some(line) = target_line {
        // 1. Export on the target line.
        if let Some(export) = file_analysis
            .exports
            .iter()
            .find(|e| e.line == Some(line) && name_matches(&e.name))
        {
            return Some(SymbolIdentity {
                name: export.name.clone(),
                line: export.line,
                kind: Some(export.kind.clone()),
                exported: true,
                internal: false,
                defined_in: None,
            });
        }

        // 2. Impl method on the target line (Rust). Exported iff public.
        if let Some(method) = file_analysis
            .impl_methods
            .iter()
            .find(|m| m.line == Some(line) && name_matches(&m.name))
        {
            let exported = matches!(method.visibility, loctree::types::Visibility::Public);
            return Some(SymbolIdentity {
                name: method.name.clone(),
                line: method.line,
                kind: Some("method".to_string()),
                exported,
                internal: !exported,
                defined_in: None,
            });
        }

        // 3. Local (non-exported or imported) symbol on the target line.
        if let Some(local) = file_analysis
            .local_symbols
            .iter()
            .find(|s| s.line == Some(line) && name_matches(&s.name))
        {
            return Some(SymbolIdentity {
                name: local.name.clone(),
                line: local.line,
                kind: Some(local.kind.clone()),
                exported: local.is_exported,
                internal: !local.is_exported,
                defined_in: None,
            });
        }
    }

    // 4. Name-only scan within this file (no line matched, or line skipped).
    let hint = name_hint?;
    if let Some(export) = file_analysis.exports.iter().find(|e| e.name == hint) {
        return Some(SymbolIdentity {
            name: export.name.clone(),
            line: export.line,
            kind: Some(export.kind.clone()),
            exported: true,
            internal: false,
            defined_in: None,
        });
    }
    if let Some(method) = file_analysis.impl_methods.iter().find(|m| m.name == hint) {
        let exported = matches!(method.visibility, loctree::types::Visibility::Public);
        return Some(SymbolIdentity {
            name: method.name.clone(),
            line: method.line,
            kind: Some("method".to_string()),
            exported,
            internal: !exported,
            defined_in: None,
        });
    }
    if let Some(local) = file_analysis.local_symbols.iter().find(|s| s.name == hint) {
        return Some(SymbolIdentity {
            name: local.name.clone(),
            line: local.line,
            kind: Some(local.kind.clone()),
            exported: local.is_exported,
            internal: !local.is_exported,
            defined_in: None,
        });
    }

    None
}

/// Resolve the symbol identity (name + export/internal status) at the cursor.
///
/// Strategy:
/// 1. **Same-file resolution** — run [`resolve_in_file`] on the requested file,
///    preferring a declaration on the cursor line, then the same-file name hint.
///    A symbol declared (or re-declared) in the current file always wins.
/// 2. **Import-graph cross-file resolution** — when the symbol does NOT resolve
///    in the current file, consult the current file's `imports`. Find the
///    `ImportEntry` whose `symbols[]` brings in this symbol (matched by
///    `ImportSymbol.alias` when present, else `.name`, and by the default
///    binding when `is_default`). When such an import exists AND its
///    `resolved_path` is `Some(D)`, run [`resolve_in_file`] on `D` using the
///    ORIGINAL imported name (`ImportSymbol.name`, never the alias) to read the
///    real declaration. The result carries `defined_in = Some(D)`.
/// 3. **Honest unresolved** — if no import brings the symbol in, or the matching
///    import has `resolved_path: None` (bare npm / stdlib / unresolved), return
///    `None`. We deliberately do NOT scan all files by name: two unrelated
///    modules can declare the same name, and name-guessing across them would
///    conflate distinct symbols. The import graph is the only cross-file edge we
///    trust.
///
/// `internal = resolved && !exported`. Returns `None` when no identity could be
/// resolved (caller leaves `exported`/`internal`/`defined_in` as `null`).
pub fn resolve_identity(
    snapshot: &Snapshot,
    file: &str,
    position: Position,
    symbol_hint: Option<&str>,
) -> Option<SymbolIdentity> {
    let file_analysis = snapshot.files.iter().find(|f| paths_match(&f.path, file))?;
    // LSP lines are 0-based; snapshot lines are 1-based.
    let target_line = position.line as usize + 1;

    // 1. Same-file resolution: cursor-line decl, then same-file name hint.
    if let Some(identity) = resolve_in_file(file_analysis, Some(target_line), symbol_hint) {
        return Some(identity);
    }

    // 2. Import-graph cross-file resolution. We only follow an explicit import
    //    edge — never a name-based scan across unrelated modules.
    //
    //    The symbol we look up is the caller's hint when given, otherwise the
    //    bare identifier under the cursor. Without a name we cannot identify
    //    which import binding the cursor sits on, so we stay unresolved.
    let wanted = symbol_hint.or_else(|| identifier_at(file_analysis, target_line))?;

    let (import_entry, import_symbol) = file_analysis.imports.iter().find_map(|imp| {
        imp.symbols
            .iter()
            .find(|s| import_binding_name(s) == wanted)
            .map(|s| (imp, s))
    })?;

    // resolved_path None → bare npm / stdlib / unresolved. Honesty over magic:
    // stay unresolved rather than guess a declaring file.
    let declaring_path = import_entry.resolved_path.as_ref()?;
    let declaring_file = snapshot
        .files
        .iter()
        .find(|f| paths_match(&f.path, declaring_path))?;

    // Look up the declaration in D by the ORIGINAL name, never the alias.
    // No line priority: the cursor line belongs to the USING file, not D.
    let mut identity = resolve_in_file(declaring_file, None, Some(&import_symbol.name))?;
    identity.defined_in = Some(declaring_file.path.clone());
    Some(identity)
}

/// Local binding name an `ImportSymbol` introduces into the importing file:
/// the alias when present, else the original name. This is the name a USAGE in
/// the importing file is written with, so it is what we match the cursor symbol
/// against. (`is_default` imports still carry the binding in `name`/`alias`.)
fn import_binding_name(symbol: &loctree::types::ImportSymbol) -> &str {
    symbol.alias.as_deref().unwrap_or(&symbol.name)
}

/// Best-effort bare identifier sitting on `target_line` (1-based) of `file`,
/// taken from the file's own declaration/usage facts. Used only to name the
/// symbol for import-graph lookup when the caller supplied no `symbol` hint;
/// it never resolves identity by itself.
fn identifier_at(file_analysis: &loctree::types::FileAnalysis, target_line: usize) -> Option<&str> {
    file_analysis
        .symbol_usages
        .iter()
        .find(|u| u.line == target_line)
        .map(|u| u.name.as_str())
}

/// Build the bounded [`BodyContext`] for `symbol`, disambiguated to `file`.
///
/// Reuses [`loctree::body::query_symbol_body`] (the `loct body` engine) and
/// keeps only the body defined in `file`, then enforces `body_max_lines`.
/// Outcome of resolving a symbol's body, disambiguated to the requested file.
///
/// We NEVER fall back to a body from another file: surfacing a definition that
/// lives elsewhere is the "demo works, product lies" trap — a common name like
/// `parse` / `run` / `init` would render the wrong source. When the symbol has
/// bodies but none in `file`, `body` is `None` and `error` is
/// `Some("not_found_in_file")` so the UI can say so honestly.
pub struct BodyResolution {
    pub body: Option<BodyContext>,
    pub error: Option<&'static str>,
}

/// `file` is the file to disambiguate the body to. For a LOCAL symbol this is
/// the requested file; for a symbol resolved CROSS-FILE through the import graph
/// it must be the DECLARING file `D` (so an imported symbol shows its real body
/// from `D`, not `body_error="not_found_in_file"` for the using file). The
/// caller passes `identity.defined_in` here when it is `Some`.
pub fn build_body(
    snapshot: &Snapshot,
    symbol: &str,
    file: &str,
    body_max_lines: usize,
) -> BodyResolution {
    let result = loctree::body::query_symbol_body(snapshot, symbol, Some(body_max_lines));
    // Disambiguate to the requested file (same suffix-filter as `loctree/body`).
    let had_any = !result.bodies.is_empty();
    let file_match = result
        .bodies
        .into_iter()
        .find(|b| paths_match(&b.file, file));

    match file_match {
        Some(body) => BodyResolution {
            body: Some(BodyContext {
                source: body.source,
                start_line: body.start_line,
                end_line: body.end_line,
                truncated: body.truncated,
                total_lines: body.total_lines,
            }),
            error: None,
        },
        // A definition exists, but not in the requested file. Report it honestly
        // rather than surfacing a body from elsewhere.
        None => BodyResolution {
            body: None,
            error: had_any.then_some("not_found_in_file"),
        },
    }
}

/// Shape literal occurrences into the response's [`OccurrencesContext`].
///
/// `results` is the full literal scan (across the whole scope). `same_file`
/// is the same scan restricted to `file`. Pagination slices the active scope
/// (same-file-only when requested, otherwise the full scope) by
/// `offset`/`limit`.
pub fn build_occurrences(
    results: &OccurrenceResults,
    file: &str,
    same_file_only: bool,
    offset: usize,
    limit: usize,
) -> OccurrencesContext {
    let total = results.total;
    let same_file_total = results
        .occurrences
        .iter()
        .filter(|o| paths_match(&o.file, file))
        .count();

    // The active scope we actually paginate over.
    let scope: Vec<&loctree::analyzer::occurrences::LiteralOccurrence> = if same_file_only {
        results
            .occurrences
            .iter()
            .filter(|o| paths_match(&o.file, file))
            .collect()
    } else {
        results.occurrences.iter().collect()
    };

    let scope_total = scope.len();
    let start = offset.min(scope_total);
    let end = start.saturating_add(limit).min(scope_total);
    let has_more = end < scope_total;
    let returned: Vec<OccurrenceView> = scope[start..end]
        .iter()
        .map(|o| OccurrenceView {
            file: o.file.clone(),
            line: o.line,
            column: o.column,
            context: o.context.clone(),
            kind: o.occurrence_kind.as_str().to_string(),
        })
        .collect();

    OccurrencesContext {
        total,
        same_file_total,
        returned,
        has_more,
        next_offset: has_more.then_some(end),
    }
}

/// Build the [`ScanOptions`] the literal scan should use.
pub fn scan_options(params: &SymbolContextParams) -> ScanOptions {
    ScanOptions {
        whole_token: params.whole_token,
    }
}

/// Best-effort parent-context name from the live tree-sitter tree.
///
/// Finds the live function/class declaration whose start line is at or above
/// the cursor and closest to it — a cheap "enclosing symbol" heuristic. When no
/// live document is open for the file (the common case for a request driven off
/// the snapshot) the result is `source: "unavailable"`, never an error.
pub fn build_parent_context(
    live_ast: &crate::live_ast::LiveAstStore,
    workspace_root: &std::path::Path,
    file: &str,
    position: Position,
) -> ParentContext {
    let Some(doc) = live_ast.get_for_path(workspace_root, file) else {
        return ParentContext::unavailable();
    };
    let symbols = crate::live_ast::extract_live_symbols(&doc.tree);
    let cursor_line = position.line as usize; // LiveSymbol.line is 0-based.

    // Best-effort ONLY: the closest declaration at or above the cursor line.
    // This is an approximation, not a true scope walk — it can name a preceding
    // sibling rather than the real enclosing function/class. `source:"live_ast"`
    // marks it as a hint; the UI must render it subtly and never as authoritative
    // truth. Tightening to byte-range enclosing is deferred (see spec, parent
    // context is best-effort pending measurement).
    let parent = symbols
        .iter()
        .filter(|s| s.line <= cursor_line)
        .max_by_key(|s| s.line);

    match parent {
        Some(sym) => ParentContext {
            name: Some(sym.name.clone()),
            source: "live_ast",
        },
        None => ParentContext::unavailable(),
    }
}

/// Build a one-line [`Range`] for a 1-based declaration line.
pub fn line_range(line: Option<usize>) -> Option<Range> {
    let line = line?;
    if line == 0 {
        return None;
    }
    let zero_based = (line - 1) as u32;
    Some(Range {
        start: Position {
            line: zero_based,
            character: 0,
        },
        end: Position {
            line: zero_based,
            character: u32::MAX,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use loctree::analyzer::occurrences::scan_files;
    use loctree::snapshot::Snapshot;
    use loctree::types::{ExportSymbol, FileAnalysis, ImplMethod, LocalSymbol, Visibility};

    fn export(name: &str, line: usize) -> ExportSymbol {
        ExportSymbol {
            name: name.to_string(),
            kind: "function".to_string(),
            export_type: "named".to_string(),
            line: Some(line),
            params: Vec::new(),
            symbol_id: loctree::types::SymbolIdV1::default(),
        }
    }

    fn local(name: &str, line: usize, exported: bool) -> LocalSymbol {
        LocalSymbol {
            name: name.to_string(),
            kind: "function".to_string(),
            line: Some(line),
            context: String::new(),
            is_exported: exported,
        }
    }

    fn snapshot_with(file: FileAnalysis) -> Snapshot {
        let mut snapshot = Snapshot::new(vec!["/tmp/x".to_string()]);
        snapshot.files.push(file);
        snapshot
    }

    fn pos(line: u32) -> Position {
        Position { line, character: 0 }
    }

    #[test]
    fn params_defaults_apply() {
        let json = serde_json::json!({
            "file": "src/a.rs",
            "position": { "line": 3, "character": 2 }
        });
        let params: SymbolContextParams = serde_json::from_value(json).expect("parse");
        assert_eq!(params.body_max_lines_resolved(), DEFAULT_BODY_MAX_LINES);
        assert_eq!(params.occurrence_limit_resolved(), DEFAULT_OCCURRENCE_LIMIT);
        assert!(!params.same_file_only);
        assert_eq!(params.offset, 0);
    }

    #[test]
    fn exported_symbol_resolves_exported_true() {
        let mut file = FileAnalysis::new("src/a.rs".to_string());
        file.exports.push(export("resolveServerBinary", 10));
        let snapshot = snapshot_with(file);

        // LSP line 9 (0-based) == snapshot line 10 (1-based).
        let identity = resolve_identity(&snapshot, "src/a.rs", pos(9), None).expect("identity");
        assert_eq!(identity.name, "resolveServerBinary");
        assert!(identity.exported, "export should be exported=true");
        assert!(!identity.internal);
    }

    #[test]
    fn local_non_exported_symbol_resolves_internal_true() {
        let mut file = FileAnalysis::new("src/a.rs".to_string());
        file.local_symbols.push(local("helperFn", 20, false));
        let snapshot = snapshot_with(file);

        let identity = resolve_identity(&snapshot, "src/a.rs", pos(19), None).expect("identity");
        assert_eq!(identity.name, "helperFn");
        assert!(!identity.exported, "non-exported local: exported=false");
        assert!(identity.internal, "non-exported local: internal=true");
    }

    #[test]
    fn private_impl_method_is_internal() {
        let mut file = FileAnalysis::new("src/a.rs".to_string());
        file.impl_methods.push(ImplMethod {
            name: "secret".to_string(),
            qualifier: "Thing".to_string(),
            line: Some(5),
            visibility: Visibility::Private,
            ..Default::default()
        });
        let snapshot = snapshot_with(file);

        let identity = resolve_identity(&snapshot, "src/a.rs", pos(4), None).expect("identity");
        assert!(identity.internal);
        assert!(!identity.exported);
    }

    #[test]
    fn public_impl_method_is_exported() {
        let mut file = FileAnalysis::new("src/a.rs".to_string());
        file.impl_methods.push(ImplMethod {
            name: "open".to_string(),
            qualifier: "Thing".to_string(),
            line: Some(7),
            visibility: Visibility::Public,
            ..Default::default()
        });
        let snapshot = snapshot_with(file);

        let identity = resolve_identity(&snapshot, "src/a.rs", pos(6), None).expect("identity");
        assert!(identity.exported);
        assert!(!identity.internal);
    }

    #[test]
    fn unresolved_identity_returns_none() {
        let file = FileAnalysis::new("src/a.rs".to_string());
        let snapshot = snapshot_with(file);
        assert!(resolve_identity(&snapshot, "src/a.rs", pos(0), None).is_none());
    }

    #[test]
    fn occurrences_paginate_with_next_offset() {
        let text = "foo;\nfoo;\nfoo;\nfoo;\n";
        let results = scan_files([("src/a.rs", text)], "foo");
        assert_eq!(results.total, 4);

        let page1 = build_occurrences(&results, "src/a.rs", false, 0, 2);
        assert_eq!(page1.total, 4);
        assert_eq!(page1.same_file_total, 4);
        assert_eq!(page1.returned.len(), 2);
        assert!(page1.has_more);
        assert_eq!(page1.next_offset, Some(2));

        let page2 = build_occurrences(&results, "src/a.rs", false, 2, 2);
        assert_eq!(page2.returned.len(), 2);
        assert!(!page2.has_more);
        assert_eq!(page2.next_offset, None);
    }

    #[test]
    fn same_file_total_counts_only_target_file() {
        let results = scan_files(
            [("src/a.rs", "foo;\nfoo;\n"), ("src/b.rs", "foo;\n")],
            "foo",
        );
        let ctx = build_occurrences(&results, "src/a.rs", false, 0, 50);
        assert_eq!(ctx.total, 3, "all files counted in total");
        assert_eq!(ctx.same_file_total, 2, "only src/a.rs counted same-file");
        assert_eq!(ctx.returned.len(), 3);
    }

    #[test]
    fn same_file_only_paginates_target_file_scope() {
        let results = scan_files(
            [("src/a.rs", "foo;\nfoo;\n"), ("src/b.rs", "foo;\n")],
            "foo",
        );
        let ctx = build_occurrences(&results, "src/a.rs", true, 0, 50);
        assert_eq!(ctx.total, 3, "total stays whole-scope");
        assert_eq!(ctx.same_file_total, 2);
        assert_eq!(ctx.returned.len(), 2, "returned limited to same-file scope");
        assert!(ctx.returned.iter().all(|o| o.file == "src/a.rs"));
    }

    #[test]
    fn line_range_is_zero_based() {
        let range = line_range(Some(10)).expect("range");
        assert_eq!(range.start.line, 9);
        assert!(line_range(Some(0)).is_none());
        assert!(line_range(None).is_none());
    }
}
