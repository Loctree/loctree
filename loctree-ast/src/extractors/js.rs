//! JavaScript `LangExtractor` implementation.
//!
//! Wraps the same query bodies as the TS extractor, but compiled against the
//! `tree-sitter-javascript` grammar so JS-only constructs (no TS types, no
//! interfaces, JS-style class declarations) parse cleanly.

use std::sync::OnceLock;

use tree_sitter::Query;

use super::ts::{extract_calls_generic, extract_exports_with, extract_imports_generic};
use super::{CallEntry, ExportSymbol, ImportEntry, LangExtractor};
use crate::{LangParser, Language, LoctreeTree};

pub struct JsExtractor;

impl LangParser for JsExtractor {
    fn language(&self) -> Language {
        tree_sitter_javascript::LANGUAGE.into()
    }

    fn lang_id(&self) -> &'static str {
        "javascript"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["js", "cjs", "mjs", "jsx"]
    }
}

const JS_EXPORTS_QUERY: &str = r#"
(export_statement
  declaration: (function_declaration name: (identifier) @name)) @export.function

(export_statement
  declaration: (class_declaration name: (identifier) @name)) @export.class

(export_statement
  declaration: (lexical_declaration
    (variable_declarator name: (identifier) @name))) @export.lexical

(export_statement
  declaration: (variable_declaration
    (variable_declarator name: (identifier) @name))) @export.var

(export_statement
  value: (identifier) @name) @export.default_ident

(export_statement
  (export_clause
    (export_specifier name: (identifier) @name))) @export.named
"#;

const JS_IMPORTS_QUERY: &str = r#"
(import_statement
  source: (string) @source) @import
"#;

const JS_CALLS_QUERY: &str = r#"
(call_expression
  function: (_) @callee) @call
"#;

fn js_query_exports() -> &'static Query {
    static Q: OnceLock<Query> = OnceLock::new();
    Q.get_or_init(|| {
        Query::new(
            &Language::from(tree_sitter_javascript::LANGUAGE),
            JS_EXPORTS_QUERY,
        )
        .expect("Plan 19 JS exports query is well-formed")
    })
}

fn js_query_imports() -> &'static Query {
    static Q: OnceLock<Query> = OnceLock::new();
    Q.get_or_init(|| {
        Query::new(
            &Language::from(tree_sitter_javascript::LANGUAGE),
            JS_IMPORTS_QUERY,
        )
        .expect("Plan 19 JS imports query is well-formed")
    })
}

fn js_query_calls() -> &'static Query {
    static Q: OnceLock<Query> = OnceLock::new();
    Q.get_or_init(|| {
        Query::new(
            &Language::from(tree_sitter_javascript::LANGUAGE),
            JS_CALLS_QUERY,
        )
        .expect("Plan 19 JS calls query is well-formed")
    })
}

impl LangExtractor for JsExtractor {
    fn extract_exports(&self, tree: &LoctreeTree) -> Vec<ExportSymbol> {
        extract_exports_with(tree, js_query_exports())
    }

    fn extract_imports(&self, tree: &LoctreeTree) -> Vec<ImportEntry> {
        extract_imports_generic(tree, js_query_imports())
    }

    fn extract_calls(&self, tree: &LoctreeTree) -> Vec<CallEntry> {
        extract_calls_generic(tree, js_query_calls())
    }
}
