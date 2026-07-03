//! TypeScript / TSX `LangExtractor` implementation.
//!
//! Stage 1 covers:
//! - exports: `export function`, `export class`, `export const|let|var`,
//!   `export default <expr>`, `export type|interface`, `export { ... }`,
//!   `export * from`.
//! - imports: `import_statement` with default / namespace / named bindings.
//! - calls: every `call_expression`; `name` is the trailing identifier of a
//!   member expression, otherwise the full callee text.
//!
//! Parity gaps versus OXC are documented in
//! `internal-artifacts/reports/lsp/19-cross-lang-stage-1.md` (Stage 2 follow-ups).

use std::sync::OnceLock;

use tree_sitter::{Node, Query, QueryCursor, StreamingIterator};

use super::{CallEntry, ExportSymbol, ImportBinding, ImportEntry, LangExtractor};
use crate::{LangParser, Language, LoctreeTree};

/// TypeScript extractor. Public so callers can construct it without going
/// through a registry; pairs with the `TypeScriptParser` already wired into
/// `Parsers::new_default`.
pub struct TsExtractor;

/// TSX extractor. Shares query bodies with `TsExtractor` but binds against the
/// TSX grammar so JSX-bearing files parse cleanly.
pub struct TsxExtractor;

impl LangParser for TsExtractor {
    fn language(&self) -> Language {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    }

    fn lang_id(&self) -> &'static str {
        "typescript"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["ts", "cts", "mts"]
    }
}

impl LangParser for TsxExtractor {
    fn language(&self) -> Language {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    }

    fn lang_id(&self) -> &'static str {
        "tsx"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["tsx"]
    }
}

impl LangExtractor for TsExtractor {
    fn extract_exports(&self, tree: &LoctreeTree) -> Vec<ExportSymbol> {
        extract_exports_with(tree, ts_query_exports())
    }

    fn extract_imports(&self, tree: &LoctreeTree) -> Vec<ImportEntry> {
        extract_imports_generic(tree, ts_query_imports())
    }

    fn extract_calls(&self, tree: &LoctreeTree) -> Vec<CallEntry> {
        extract_calls_generic(tree, ts_query_calls())
    }
}

impl LangExtractor for TsxExtractor {
    fn extract_exports(&self, tree: &LoctreeTree) -> Vec<ExportSymbol> {
        extract_exports_with(tree, tsx_query_exports())
    }

    fn extract_imports(&self, tree: &LoctreeTree) -> Vec<ImportEntry> {
        extract_imports_generic(tree, tsx_query_imports())
    }

    fn extract_calls(&self, tree: &LoctreeTree) -> Vec<CallEntry> {
        extract_calls_generic(tree, tsx_query_calls())
    }
}

// ---------------------------------------------------------------------------
// Query bodies (shared between TS and TSX; bound to per-grammar `Language`).
// ---------------------------------------------------------------------------

const EXPORTS_QUERY: &str = r#"
(export_statement
  declaration: (function_declaration name: (identifier) @name)) @export.function

(export_statement
  declaration: (class_declaration name: (type_identifier) @name)) @export.class

(export_statement
  declaration: (lexical_declaration
    (variable_declarator name: (identifier) @name))) @export.lexical

(export_statement
  declaration: (variable_declaration
    (variable_declarator name: (identifier) @name))) @export.var

(export_statement
  declaration: (interface_declaration name: (type_identifier) @name)) @export.interface

(export_statement
  declaration: (type_alias_declaration name: (type_identifier) @name)) @export.type

(export_statement
  declaration: (enum_declaration name: (identifier) @name)) @export.enum

(export_statement
  value: (identifier) @name) @export.default_ident

(export_statement
  (export_clause
    (export_specifier name: (identifier) @name))) @export.named
"#;

const IMPORTS_QUERY: &str = r#"
(import_statement
  source: (string) @source) @import
"#;

const CALLS_QUERY: &str = r#"
(call_expression
  function: (_) @callee) @call
"#;

fn ts_query_exports() -> &'static Query {
    static Q: OnceLock<Query> = OnceLock::new();
    Q.get_or_init(|| {
        compile(
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            EXPORTS_QUERY,
        )
    })
}

fn ts_query_imports() -> &'static Query {
    static Q: OnceLock<Query> = OnceLock::new();
    Q.get_or_init(|| {
        compile(
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            IMPORTS_QUERY,
        )
    })
}

fn ts_query_calls() -> &'static Query {
    static Q: OnceLock<Query> = OnceLock::new();
    Q.get_or_init(|| {
        compile(
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            CALLS_QUERY,
        )
    })
}

fn tsx_query_exports() -> &'static Query {
    static Q: OnceLock<Query> = OnceLock::new();
    Q.get_or_init(|| compile(tree_sitter_typescript::LANGUAGE_TSX.into(), EXPORTS_QUERY))
}

fn tsx_query_imports() -> &'static Query {
    static Q: OnceLock<Query> = OnceLock::new();
    Q.get_or_init(|| compile(tree_sitter_typescript::LANGUAGE_TSX.into(), IMPORTS_QUERY))
}

fn tsx_query_calls() -> &'static Query {
    static Q: OnceLock<Query> = OnceLock::new();
    Q.get_or_init(|| compile(tree_sitter_typescript::LANGUAGE_TSX.into(), CALLS_QUERY))
}

fn compile(lang: Language, body: &str) -> Query {
    Query::new(&lang, body).expect("Plan 19 extractor query is well-formed")
}

// ---------------------------------------------------------------------------
// Generic match walkers — reused by JS extractor (same node naming).
// ---------------------------------------------------------------------------

pub(super) fn extract_exports_with(tree: &LoctreeTree, query: &Query) -> Vec<ExportSymbol> {
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    let mut matches = cursor.matches(query, tree.tree.root_node(), tree.source.as_slice());

    while let Some(m) = matches.next() {
        // Each match yields a `@name` capture (identifier) and a marker
        // capture (e.g. `@export.function`) on the surrounding statement.
        let mut name_capture: Option<Node> = None;
        let mut marker: Option<&str> = None;
        let mut export_node: Option<Node> = None;
        for cap in m.captures {
            let cap_name = capture_names.get(cap.index as usize).copied().unwrap_or("");
            if cap_name == "name" {
                name_capture = Some(cap.node);
            } else if let Some(rest) = cap_name.strip_prefix("export.") {
                marker = Some(rest);
                export_node = Some(cap.node);
            }
        }
        let (Some(name_node), Some(kind_marker), Some(stmt_node)) =
            (name_capture, marker, export_node)
        else {
            continue;
        };
        let name = node_text(&name_node, &tree.source).to_string();
        let kind = match kind_marker {
            "function" => "function",
            "class" => "class",
            "lexical" | "var" => "const",
            "interface" => "interface",
            "type" => "type",
            "enum" => "enum",
            "default_ident" => "default",
            "named" => "reexport",
            _ => "unknown",
        };
        let export_type = if kind_marker == "default_ident" {
            "default"
        } else if kind_marker == "named" {
            "reexport"
        } else {
            "named"
        };
        let line = stmt_node.start_position().row + 1;
        let range = stmt_node.byte_range();
        out.push(ExportSymbol {
            name,
            kind: kind.to_string(),
            export_type: export_type.to_string(),
            line: Some(line),
            byte_range: (range.start, range.end),
        });
    }
    out
}

pub(super) fn extract_imports_generic(tree: &LoctreeTree, query: &Query) -> Vec<ImportEntry> {
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    let mut matches = cursor.matches(query, tree.tree.root_node(), tree.source.as_slice());

    while let Some(m) = matches.next() {
        let mut import_node: Option<Node> = None;
        let mut source_node: Option<Node> = None;
        for cap in m.captures {
            let cap_name = capture_names.get(cap.index as usize).copied().unwrap_or("");
            match cap_name {
                "import" => import_node = Some(cap.node),
                "source" => source_node = Some(cap.node),
                _ => {}
            }
        }
        let (Some(stmt), Some(src)) = (import_node, source_node) else {
            continue;
        };
        let raw_source = node_text(&src, &tree.source);
        let source = strip_string_quotes(raw_source).to_string();
        let line = stmt.start_position().row + 1;
        let range = stmt.byte_range();
        let symbols = walk_import_clause(&stmt, &tree.source);
        out.push(ImportEntry {
            source,
            line: Some(line),
            symbols,
            byte_range: (range.start, range.end),
        });
    }
    out
}

pub(super) fn extract_calls_generic(tree: &LoctreeTree, query: &Query) -> Vec<CallEntry> {
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    let mut matches = cursor.matches(query, tree.tree.root_node(), tree.source.as_slice());

    while let Some(m) = matches.next() {
        let mut call_node: Option<Node> = None;
        let mut callee_node: Option<Node> = None;
        for cap in m.captures {
            let cap_name = capture_names.get(cap.index as usize).copied().unwrap_or("");
            match cap_name {
                "call" => call_node = Some(cap.node),
                "callee" => callee_node = Some(cap.node),
                _ => {}
            }
        }
        let (Some(call), Some(callee)) = (call_node, callee_node) else {
            continue;
        };
        let callee_text = node_text(&callee, &tree.source).to_string();
        let name =
            trailing_identifier(&callee, &tree.source).unwrap_or_else(|| callee_text.clone());
        let line = call.start_position().row + 1;
        let range = call.byte_range();
        out.push(CallEntry {
            name,
            callee: callee_text,
            byte_range: (range.start, range.end),
            line,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Import-clause walker — manual, since tree-sitter's nested capture story is
// noisier than a structured walk for default + namespace + named in one pass.
// ---------------------------------------------------------------------------

fn walk_import_clause(import_stmt: &Node, source: &[u8]) -> Vec<ImportBinding> {
    let mut bindings = Vec::new();
    let mut cursor = import_stmt.walk();
    for child in import_stmt.children(&mut cursor) {
        if child.kind() == "import_clause" {
            let mut clause_cursor = child.walk();
            for sub in child.children(&mut clause_cursor) {
                match sub.kind() {
                    // Default import: `import Foo from 'x'`
                    "identifier" => {
                        bindings.push(ImportBinding {
                            local_name: node_text(&sub, source).to_string(),
                            imported: None,
                            is_default: true,
                            is_namespace: false,
                        });
                    }
                    // `import * as Foo from 'x'`
                    "namespace_import" => {
                        if let Some(ident) = first_child_of_kind(&sub, "identifier") {
                            bindings.push(ImportBinding {
                                local_name: node_text(&ident, source).to_string(),
                                imported: None,
                                is_default: false,
                                is_namespace: true,
                            });
                        }
                    }
                    // `import { a, b as c } from 'x'`
                    "named_imports" => {
                        let mut named_cursor = sub.walk();
                        for spec in sub.children(&mut named_cursor) {
                            if spec.kind() == "import_specifier" {
                                let imported_name = field_text(&spec, "name", source);
                                let alias = field_text(&spec, "alias", source);
                                let (local, imported) = match (imported_name, alias) {
                                    (Some(name), Some(alias)) => (alias, Some(name)),
                                    (Some(name), None) => (name.clone(), None),
                                    (None, Some(alias)) => (alias, None),
                                    (None, None) => continue,
                                };
                                bindings.push(ImportBinding {
                                    local_name: local,
                                    imported,
                                    is_default: false,
                                    is_namespace: false,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    bindings
}

fn trailing_identifier(callee: &Node, source: &[u8]) -> Option<String> {
    match callee.kind() {
        "identifier" | "property_identifier" | "type_identifier" => {
            Some(node_text(callee, source).to_string())
        }
        "member_expression" => {
            let prop = callee.child_by_field_name("property")?;
            Some(node_text(&prop, source).to_string())
        }
        _ => None,
    }
}

fn node_text<'a>(node: &Node, source: &'a [u8]) -> &'a str {
    let range = node.byte_range();
    std::str::from_utf8(&source[range.start..range.end]).unwrap_or("")
}

fn field_text(node: &Node, field: &str, source: &[u8]) -> Option<String> {
    let f = node.child_by_field_name(field)?;
    Some(node_text(&f, source).to_string())
}

fn first_child_of_kind<'tree>(node: &Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn strip_string_quotes(raw: &str) -> &str {
    let trimmed = raw.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'' || bytes[0] == b'`')
        && bytes[0] == bytes[bytes.len() - 1]
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}
