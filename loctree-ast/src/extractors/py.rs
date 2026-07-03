//! Python `LangExtractor` implementation.
//!
//! This is the tree-sitter substrate only. It does not rewire the existing
//! Python analyzer; it gives callers a typed extractor with the same export /
//! import / call contract as the JS/TS extractors.

use std::sync::OnceLock;

use tree_sitter::{Node, Query, QueryCursor, StreamingIterator};

use super::{CallEntry, ExportSymbol, ImportBinding, ImportEntry, LangExtractor};
use crate::{LangParser, Language, LoctreeTree};

/// Python extractor. Public so callers can construct it without going through
/// a registry; pairs with the `PythonParser` wired into `Parsers::new_default`.
pub struct PyExtractor;

impl LangParser for PyExtractor {
    fn language(&self) -> Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn lang_id(&self) -> &'static str {
        "python"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["py", "pyi"]
    }
}

impl LangExtractor for PyExtractor {
    fn extract_exports(&self, tree: &LoctreeTree) -> Vec<ExportSymbol> {
        extract_exports(tree)
    }

    fn extract_imports(&self, tree: &LoctreeTree) -> Vec<ImportEntry> {
        extract_imports(tree)
    }

    fn extract_calls(&self, tree: &LoctreeTree) -> Vec<CallEntry> {
        extract_calls(tree)
    }
}

const CALLS_QUERY: &str = r#"
(call
  function: (_) @callee) @call
"#;

fn py_query_calls() -> &'static Query {
    static Q: OnceLock<Query> = OnceLock::new();
    Q.get_or_init(|| Query::new(&tree_sitter_python::LANGUAGE.into(), CALLS_QUERY).unwrap())
}

fn extract_exports(tree: &LoctreeTree) -> Vec<ExportSymbol> {
    let mut out = Vec::new();
    let root = tree.tree.root_node();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor).filter(|child| child.is_named()) {
        match child.kind() {
            "function_definition" => {
                push_definition_export(&mut out, &child, "function", &tree.source)
            }
            "class_definition" => push_definition_export(&mut out, &child, "class", &tree.source),
            "decorated_definition" => {
                if let Some(def) = child.child_by_field_name("definition") {
                    match def.kind() {
                        "function_definition" => {
                            push_definition_export(&mut out, &def, "function", &tree.source)
                        }
                        "class_definition" => {
                            push_definition_export(&mut out, &def, "class", &tree.source)
                        }
                        _ => {}
                    }
                }
            }
            "expression_statement" => {
                if let Some(assignment) = first_child_of_kind(&child, "assignment") {
                    push_assignment_exports(&mut out, &assignment, &tree.source);
                }
            }
            _ => {}
        }
    }

    out
}

fn push_definition_export(out: &mut Vec<ExportSymbol>, node: &Node, kind: &str, source: &[u8]) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let range = node.byte_range();
    out.push(ExportSymbol {
        name: node_text(&name_node, source).to_string(),
        kind: kind.to_string(),
        export_type: "named".to_string(),
        line: Some(node.start_position().row + 1),
        byte_range: (range.start, range.end),
    });
}

fn push_assignment_exports(out: &mut Vec<ExportSymbol>, assignment: &Node, source: &[u8]) {
    let Some(left) = assignment.child_by_field_name("left") else {
        return;
    };
    let range = assignment.byte_range();
    let line = assignment.start_position().row + 1;
    let names = assigned_names(&left, source);

    if names.len() == 1 && names[0] == "__all__" {
        push_dunder_all_exports(out, assignment, source);
        return;
    }

    for name in names.into_iter().filter(|name| name != "__all__") {
        out.push(ExportSymbol {
            name,
            kind: "const".to_string(),
            export_type: "named".to_string(),
            line: Some(line),
            byte_range: (range.start, range.end),
        });
    }
}

fn push_dunder_all_exports(out: &mut Vec<ExportSymbol>, assignment: &Node, source: &[u8]) {
    let Some(right) = assignment.child_by_field_name("right") else {
        return;
    };
    if !matches!(right.kind(), "list" | "tuple") {
        return;
    }
    let range = assignment.byte_range();
    let line = assignment.start_position().row + 1;
    let mut names = Vec::new();
    collect_string_literals(&right, source, &mut names);
    for name in names {
        out.push(ExportSymbol {
            name,
            kind: "all".to_string(),
            export_type: "__all__".to_string(),
            line: Some(line),
            byte_range: (range.start, range.end),
        });
    }
}

fn assigned_names(left: &Node, source: &[u8]) -> Vec<String> {
    match left.kind() {
        "identifier" => vec![node_text(left, source).to_string()],
        "pattern_list" | "tuple_pattern" | "list_pattern" => {
            let mut cursor = left.walk();
            left.children(&mut cursor)
                .filter(|child| child.kind() == "identifier")
                .map(|child| node_text(&child, source).to_string())
                .collect()
        }
        _ => Vec::new(),
    }
}

fn collect_string_literals(node: &Node, source: &[u8], out: &mut Vec<String>) {
    if node.kind() == "string" {
        out.push(strip_python_string_quotes(node_text(node, source)).to_string());
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor).filter(|child| child.is_named()) {
        collect_string_literals(&child, source, out);
    }
}

fn extract_imports(tree: &LoctreeTree) -> Vec<ImportEntry> {
    let mut out = Vec::new();
    let root = tree.tree.root_node();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor).filter(|child| child.is_named()) {
        match child.kind() {
            "import_statement" => push_import_statement(&mut out, &child, &tree.source),
            "import_from_statement" | "future_import_statement" => {
                push_import_from_statement(&mut out, &child, &tree.source)
            }
            _ => {}
        }
    }

    out
}

fn push_import_statement(out: &mut Vec<ImportEntry>, stmt: &Node, source: &[u8]) {
    let mut cursor = stmt.walk();
    for child in stmt.children(&mut cursor).filter(|child| child.is_named()) {
        match child.kind() {
            "dotted_name" => {
                let module = node_text(&child, source).to_string();
                push_import_entry(out, stmt, module.clone(), vec![module_binding(module)]);
            }
            "aliased_import" => {
                let Some(name_node) = child.child_by_field_name("name") else {
                    continue;
                };
                let module = node_text(&name_node, source).to_string();
                let alias = child
                    .child_by_field_name("alias")
                    .map(|node| node_text(&node, source).to_string())
                    .unwrap_or_else(|| module.clone());
                push_import_entry(
                    out,
                    stmt,
                    module.clone(),
                    vec![alias_binding(alias, module)],
                );
            }
            _ => {}
        }
    }
}

fn push_import_from_statement(out: &mut Vec<ImportEntry>, stmt: &Node, source: &[u8]) {
    let source_name = if stmt.kind() == "future_import_statement" {
        "__future__".to_string()
    } else {
        stmt.child_by_field_name("module_name")
            .map(|node| node_text(&node, source).to_string())
            .unwrap_or_default()
    };
    let symbols = imported_symbols(stmt, source);
    push_import_entry(out, stmt, source_name, symbols);
}

fn imported_symbols(stmt: &Node, source: &[u8]) -> Vec<ImportBinding> {
    let mut symbols = Vec::new();
    let mut cursor = stmt.walk();
    for child in stmt.children(&mut cursor).filter(|child| child.is_named()) {
        match child.kind() {
            "dotted_name" if child_by_field_eq(stmt, "module_name", &child) => {}
            "relative_import" => {}
            "dotted_name" => {
                let imported = node_text(&child, source).to_string();
                symbols.push(named_binding(imported.clone(), None));
            }
            "aliased_import" => {
                let Some(name_node) = child.child_by_field_name("name") else {
                    continue;
                };
                let imported = node_text(&name_node, source).to_string();
                let alias = child
                    .child_by_field_name("alias")
                    .map(|node| node_text(&node, source).to_string())
                    .unwrap_or_else(|| imported.clone());
                symbols.push(named_binding(alias, Some(imported)));
            }
            "wildcard_import" => {
                symbols.push(named_binding("*".to_string(), Some("*".to_string())))
            }
            _ => {}
        }
    }
    symbols
}

fn push_import_entry(
    out: &mut Vec<ImportEntry>,
    stmt: &Node,
    source_name: String,
    symbols: Vec<ImportBinding>,
) {
    let range = stmt.byte_range();
    out.push(ImportEntry {
        source: source_name,
        line: Some(stmt.start_position().row + 1),
        symbols,
        byte_range: (range.start, range.end),
    });
}

fn module_binding(module: String) -> ImportBinding {
    let local = module.split('.').next().unwrap_or(&module).to_string();
    ImportBinding {
        local_name: local,
        imported: Some(module),
        is_default: false,
        is_namespace: true,
    }
}

fn alias_binding(local_name: String, imported: String) -> ImportBinding {
    ImportBinding {
        local_name,
        imported: Some(imported),
        is_default: false,
        is_namespace: true,
    }
}

fn named_binding(local_name: String, imported: Option<String>) -> ImportBinding {
    ImportBinding {
        local_name,
        imported,
        is_default: false,
        is_namespace: false,
    }
}

fn extract_calls(tree: &LoctreeTree) -> Vec<CallEntry> {
    let query = py_query_calls();
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
        let range = call.byte_range();
        out.push(CallEntry {
            name,
            callee: callee_text,
            byte_range: (range.start, range.end),
            line: call.start_position().row + 1,
        });
    }

    out
}

fn trailing_identifier(callee: &Node, source: &[u8]) -> Option<String> {
    match callee.kind() {
        "identifier" => Some(node_text(callee, source).to_string()),
        "attribute" => {
            let attr = callee.child_by_field_name("attribute")?;
            Some(node_text(&attr, source).to_string())
        }
        _ => None,
    }
}

fn first_child_of_kind<'tree>(node: &Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn child_by_field_eq(parent: &Node, field: &str, child: &Node) -> bool {
    parent
        .child_by_field_name(field)
        .is_some_and(|field_child| field_child.id() == child.id())
}

fn node_text<'a>(node: &Node, source: &'a [u8]) -> &'a str {
    let range = node.byte_range();
    std::str::from_utf8(&source[range.start..range.end]).unwrap_or("")
}

fn strip_python_string_quotes(raw: &str) -> &str {
    let trimmed = raw.trim();
    let bytes = trimmed.as_bytes();
    let Some(start) = bytes.iter().position(|b| matches!(b, b'\'' | b'"')) else {
        return trimmed;
    };
    let quote = bytes[start];
    if bytes.len() >= start + 6 && bytes[start..].starts_with(&[quote, quote, quote]) {
        return trimmed
            .get(start + 3..trimmed.len().saturating_sub(3))
            .unwrap_or("");
    }
    if bytes.len() > start + 1 && bytes[bytes.len() - 1] == quote {
        trimmed.get(start + 1..trimmed.len() - 1).unwrap_or("")
    } else {
        trimmed
    }
}
