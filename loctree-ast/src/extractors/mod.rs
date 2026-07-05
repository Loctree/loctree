//! Plan 19 Stage 1 — `LangExtractor` trait and shared per-language extraction
//! contract for tree-sitter-backed exports / imports / calls.
//!
//! This module is intentionally **thin** and **leaf** — it does not depend on
//! `loctree-rs` types so it stays cycle-free. Mirroring shapes (`ExportSymbol`,
//! `ImportEntry`, `CallEntry`) live here as Plan 19 v1 contract; the cold-scan
//! dispatcher in `loctree-rs` adapts these into the existing `loctree::types`
//! representations behind a feature flag (`analyzer.parser = "ts"`).
//!
//! Stage 1 ships TS/JS only. Other languages (`py`, `rs`, `go`, `css`, `dart`,
//! SFC) stay queued for Stage 2 — see `docs/plans/lsp/19-cross-language-unified-surface.md`.

use crate::{LangParser, LoctreeTree};

pub mod js;
pub mod py;
pub mod ts;

pub use js::JsExtractor;
pub use py::PyExtractor;
pub use ts::TsExtractor;

/// Plan 19 v1 export shape — file-coarse view of an exported symbol.
///
/// Mirrors `loctree::types::ExportSymbol` so the cold-scan dispatcher can
/// translate without the `loctree-ast` crate taking a reverse dependency on
/// `loctree`. Once Stage 2 lands and OXC removal is in scope, this becomes the
/// canonical shape and the duplicate is collapsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportSymbol {
    /// Exported name as written in source.
    pub name: String,
    /// Symbol kind: `"function"`, `"class"`, `"const"`, `"type"`, etc.
    pub kind: String,
    /// Export type: `"named"`, `"default"`, `"reexport"`.
    pub export_type: String,
    /// 1-based line number of the declaration, when available.
    pub line: Option<usize>,
    /// Byte range of the declaration (Plan 19 stage 1: covers full export
    /// statement node, not just the identifier).
    pub byte_range: (usize, usize),
}

/// Plan 19 v1 import shape — module path plus the bound symbols.
///
/// Mirrors `loctree::types::ImportEntry` for the cold-scan dispatcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportEntry {
    /// Module specifier as written (`./utils/greeting`, `react`, ...).
    pub source: String,
    /// 1-based line number of the import statement.
    pub line: Option<usize>,
    /// Bound symbols. Empty on side-effect imports (`import 'styles.css'`).
    pub symbols: Vec<ImportBinding>,
    /// Byte range of the import statement node.
    pub byte_range: (usize, usize),
}

/// A single name introduced by an import statement. Captures default,
/// namespace, and named bindings uniformly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportBinding {
    /// Name as imported into the module scope (`React`, `useState`, alias).
    pub local_name: String,
    /// Original name in the source module when an alias is in play
    /// (`import { foo as bar }` -> `local_name = "bar"`, `imported = Some("foo")`).
    pub imported: Option<String>,
    /// True when this is a default import (`import Foo from "x"`).
    pub is_default: bool,
    /// True when this is a namespace import (`import * as X from "y"`).
    pub is_namespace: bool,
}

/// Plan 19 v1 call shape — minimum-viable callsite record.
///
/// `name` is the function/method identifier being called. `callee` is the full
/// callee expression as written (`foo`, `obj.method`, `ns.fn`). Subsequent
/// stages may upgrade this into structured callee references; Stage 1 keeps it
/// string-shaped to land the dispatch surface without a deeper graph
/// commitment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallEntry {
    /// Trailing identifier of the call expression (rightmost segment of a
    /// member expression). Falls back to the full callee when no member
    /// expression is involved.
    pub name: String,
    /// Full callee expression text as it appears in source.
    pub callee: String,
    /// Byte range of the call expression node.
    pub byte_range: (usize, usize),
    /// 1-based line number of the call site.
    pub line: usize,
}

/// Plan 19 trait — extends `LangParser` with the three primary cold-scan
/// extraction surfaces. Stage 1 implements this for TS/JS only.
pub trait LangExtractor: LangParser {
    /// Walk the tree and emit every export the language considers public
    /// (named, default, re-export). The returned vec preserves source order.
    fn extract_exports(&self, tree: &LoctreeTree) -> Vec<ExportSymbol>;

    /// Walk the tree and emit every import statement (static, type-only,
    /// side-effect, namespace, dynamic when expressible). Stage 1 covers
    /// `import_statement` only — dynamic `import()` is a Stage 2 follow-up.
    fn extract_imports(&self, tree: &LoctreeTree) -> Vec<ImportEntry>;

    /// Walk the tree and emit every call expression. Stage 1 ships this as
    /// best-effort; parity with OXC's call resolution is a Stage 2 follow-up.
    fn extract_calls(&self, tree: &LoctreeTree) -> Vec<CallEntry>;
}
