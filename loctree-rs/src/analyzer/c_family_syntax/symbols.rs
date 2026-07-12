//! Tree-sitter declaration extraction for the C family.
//!
//! Walks the syntax tree once and emits [`SymbolNode`]s (provenance
//! `TreeSitter`) plus a `Definition`/`Declaration` occurrence per node
//! (confidence `Heuristic` — Tier 1 never claims `Precise`).

use crate::symbols::{
    Confidence, LanguageId, OccurrenceRole, SymbolId, SymbolKind, SymbolNode, SymbolOccurrence,
    SymbolProvenance, TextRange,
};
use std::path::PathBuf;
use tree_sitter::{Node, Parser, Tree};

/// Hard cap on extracted definitions per file — degenerate generated sources
/// should not balloon the snapshot.
const MAX_SYMBOLS_PER_FILE: usize = 4000;

/// Identifier-ish node kinds across the four grammars, used for name
/// resolution fallback.
const IDENT_KINDS: &[&str] = &[
    "identifier",
    "simple_identifier",
    "type_identifier",
    "field_identifier",
    "method_identifier",
    "namespace_identifier",
    "operator_name",
    "destructor_name",
];

#[derive(Default)]
pub(super) struct Extraction {
    pub nodes: Vec<SymbolNode>,
    pub occurrences: Vec<SymbolOccurrence>,
    /// Byte ranges of definition name tokens, so the usage pass does not
    /// re-count a definition site as a reference.
    pub name_ranges: Vec<(usize, usize)>,
    /// Parsed tree, reused by the usage pass (parse once per file).
    pub tree: Option<Tree>,
}

pub(super) fn grammar_for(lang: LanguageId) -> tree_sitter::Language {
    match lang {
        LanguageId::Swift => tree_sitter_swift::LANGUAGE.into(),
        LanguageId::ObjC | LanguageId::ObjCpp => tree_sitter_objc::LANGUAGE.into(),
        LanguageId::C => tree_sitter_c::LANGUAGE.into(),
        LanguageId::Cpp => tree_sitter_cpp::LANGUAGE.into(),
    }
}

/// Human/agent-facing label for a [`SymbolKind`] — also the `kind` segment of
/// the Tier-1 [`SymbolId`] descriptor.
pub(super) fn kind_label(kind: &SymbolKind) -> String {
    match kind {
        SymbolKind::Type => "type".into(),
        SymbolKind::Class => "class".into(),
        SymbolKind::Struct => "struct".into(),
        SymbolKind::Protocol => "protocol".into(),
        SymbolKind::Enum => "enum".into(),
        SymbolKind::Func => "func".into(),
        SymbolKind::Method => "method".into(),
        SymbolKind::Property => "property".into(),
        SymbolKind::Field => "field".into(),
        SymbolKind::Var => "var".into(),
        SymbolKind::Macro => "macro".into(),
        SymbolKind::Typedef => "typedef".into(),
        SymbolKind::Selector => "selector".into(),
        SymbolKind::Module => "module".into(),
        SymbolKind::Namespace => "namespace".into(),
        SymbolKind::Other(s) => s.clone(),
    }
}

pub(super) fn extract(content: &str, relative: &str, lang: LanguageId) -> Extraction {
    let mut out = Extraction::default();
    let mut parser = Parser::new();
    if parser.set_language(&grammar_for(lang)).is_err() {
        return out;
    }
    let Some(tree) = parser.parse(content, None) else {
        return out;
    };
    let src = content.as_bytes();

    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if out.nodes.len() >= MAX_SYMBOLS_PER_FILE {
            break;
        }
        if let Some((kind, role)) = classify(node, lang)
            && let Some((name, name_node)) = resolve_name(node, src, lang, &kind)
        {
            push_symbol(
                &mut out, relative, lang, kind, role, name, node, name_node, src,
            );
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.is_named() {
                stack.push(child);
            }
        }
    }
    out.tree = Some(tree);
    out
}

#[allow(clippy::too_many_arguments)]
fn push_symbol(
    out: &mut Extraction,
    relative: &str,
    lang: LanguageId,
    kind: SymbolKind,
    role: OccurrenceRole,
    name: String,
    node: Node,
    name_node: Option<Node>,
    src: &[u8],
) {
    let range = text_range(node);
    let label = kind_label(&kind);
    let range_hash = fnv1a(
        format!(
            "{}:{}:{}:{}",
            relative, range.start_byte, range.end_byte, name
        )
        .as_bytes(),
    );
    let id = SymbolId::from_parts(relative, &label, &name, range_hash);

    let signature = node
        .utf8_text(src)
        .ok()
        .and_then(|t| t.lines().next())
        .map(|l| {
            let trimmed = l.trim();
            let mut sig: String = trimmed.chars().take(160).collect();
            if trimmed.chars().count() > 160 {
                sig.push('…');
            }
            sig
        });

    if let Some(n) = name_node {
        out.name_ranges.push((n.start_byte(), n.end_byte()));
    }
    out.occurrences.push(SymbolOccurrence {
        symbol_id: id.clone(),
        file: PathBuf::from(relative),
        range,
        role,
        confidence: Confidence::Heuristic,
        engine: SymbolProvenance::TreeSitter,
    });
    out.nodes.push(SymbolNode {
        id,
        language: lang,
        kind,
        name,
        qualified_name: None,
        module: None,
        usr: None,
        file: Some(PathBuf::from(relative)),
        range: Some(range),
        signature,
        visibility: None,
        provenance: SymbolProvenance::TreeSitter,
    });
}

/// Map a grammar node to a declared-symbol kind + occurrence role.
fn classify(node: Node, lang: LanguageId) -> Option<(SymbolKind, OccurrenceRole)> {
    let kind = node.kind();
    match lang {
        LanguageId::Swift => match kind {
            // `class_declaration` covers class/struct/enum/actor/extension —
            // the leading keyword token disambiguates (see `swift_class_kind`).
            "class_declaration" => Some((swift_class_kind(node), OccurrenceRole::Definition)),
            "protocol_declaration" => Some((SymbolKind::Protocol, OccurrenceRole::Definition)),
            "function_declaration" => Some((
                if has_type_body_ancestor(node) {
                    SymbolKind::Method
                } else {
                    SymbolKind::Func
                },
                OccurrenceRole::Definition,
            )),
            "protocol_function_declaration" => {
                Some((SymbolKind::Method, OccurrenceRole::Declaration))
            }
            "init_declaration" => Some((SymbolKind::Method, OccurrenceRole::Definition)),
            "property_declaration" | "protocol_property_declaration" => {
                Some((SymbolKind::Property, OccurrenceRole::Definition))
            }
            "call_expression" => Some((
                SymbolKind::Other("command".to_string()),
                OccurrenceRole::Definition,
            )),
            "typealias_declaration" => Some((SymbolKind::Typedef, OccurrenceRole::Definition)),
            _ => None,
        },
        LanguageId::ObjC | LanguageId::ObjCpp => match kind {
            "class_interface" => Some((SymbolKind::Class, OccurrenceRole::Declaration)),
            "class_implementation" => Some((SymbolKind::Class, OccurrenceRole::Definition)),
            "protocol_declaration" => Some((SymbolKind::Protocol, OccurrenceRole::Definition)),
            "method_declaration" => Some((SymbolKind::Method, OccurrenceRole::Declaration)),
            "method_definition" => Some((SymbolKind::Method, OccurrenceRole::Definition)),
            "property_declaration" => Some((SymbolKind::Property, OccurrenceRole::Declaration)),
            "function_definition" => Some((SymbolKind::Func, OccurrenceRole::Definition)),
            "type_definition" => Some((SymbolKind::Typedef, OccurrenceRole::Definition)),
            _ => None,
        },
        LanguageId::C => match kind {
            "function_definition" => Some((SymbolKind::Func, OccurrenceRole::Definition)),
            "struct_specifier" if node.child_by_field_name("body").is_some() => {
                Some((SymbolKind::Struct, OccurrenceRole::Definition))
            }
            "enum_specifier" if node.child_by_field_name("body").is_some() => {
                Some((SymbolKind::Enum, OccurrenceRole::Definition))
            }
            "union_specifier" if node.child_by_field_name("body").is_some() => Some((
                SymbolKind::Other("union".to_string()),
                OccurrenceRole::Definition,
            )),
            "type_definition" => Some((SymbolKind::Typedef, OccurrenceRole::Definition)),
            _ => None,
        },
        LanguageId::Cpp => match kind {
            "function_definition" => Some((
                if has_type_body_ancestor(node) {
                    SymbolKind::Method
                } else {
                    SymbolKind::Func
                },
                OccurrenceRole::Definition,
            )),
            "class_specifier" if node.child_by_field_name("body").is_some() => {
                Some((SymbolKind::Class, OccurrenceRole::Definition))
            }
            "struct_specifier" if node.child_by_field_name("body").is_some() => {
                Some((SymbolKind::Struct, OccurrenceRole::Definition))
            }
            "enum_specifier" if node.child_by_field_name("body").is_some() => {
                Some((SymbolKind::Enum, OccurrenceRole::Definition))
            }
            "namespace_definition" => Some((SymbolKind::Namespace, OccurrenceRole::Definition)),
            "type_definition" => Some((SymbolKind::Typedef, OccurrenceRole::Definition)),
            "alias_declaration" => Some((SymbolKind::Typedef, OccurrenceRole::Definition)),
            _ => None,
        },
    }
}

/// Disambiguate Swift `class_declaration` into class/struct/enum/extension by
/// scanning the leading keyword tokens.
fn swift_class_kind(node: Node) -> SymbolKind {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class" => return SymbolKind::Class,
            "struct" => return SymbolKind::Struct,
            "enum" => return SymbolKind::Enum,
            "actor" => return SymbolKind::Class,
            "extension" => return SymbolKind::Other("extension".to_string()),
            _ => {}
        }
    }
    SymbolKind::Type
}

/// True when the node lives inside a type body (class/struct/protocol), which
/// promotes functions to methods.
fn has_type_body_ancestor(node: Node) -> bool {
    let mut cur = node.parent();
    while let Some(p) = cur {
        match p.kind() {
            // Swift bodies
            "class_body" | "protocol_body" | "enum_class_body"
            // C++ member lists
            | "field_declaration_list" => return true,
            _ => {}
        }
        cur = p.parent();
    }
    false
}

/// Resolve the declared name of a node: explicit `name` field first, then the
/// `declarator` chain (C/C++), then a bounded identifier-descendant search.
fn resolve_name<'a>(
    node: Node<'a>,
    src: &[u8],
    lang: LanguageId,
    kind: &SymbolKind,
) -> Option<(String, Option<Node<'a>>)> {
    if lang == LanguageId::Swift && matches!(kind, SymbolKind::Other(label) if label == "command") {
        return swift_command_label(node, src);
    }

    // ObjC method selectors: prefer the dedicated selector node so
    // `- (void)reload` resolves to `reload`, not the return type.
    if matches!(kind, SymbolKind::Method | SymbolKind::Selector)
        && matches!(lang, LanguageId::ObjC | LanguageId::ObjCpp)
        && let Some(found) = objc_method_name(node, src)
    {
        return Some(found);
    }

    if matches!(kind, SymbolKind::Method | SymbolKind::Selector)
        && let Some(n) = find_descendant_of_kind(node, "method_identifier", 6)
    {
        return text_of(n, src).map(|t| (t, Some(n)));
    }

    if matches!(lang, LanguageId::ObjC | LanguageId::ObjCpp)
        && matches!(kind, SymbolKind::Property)
        && let Some(found) = objc_property_name(node, src)
    {
        return Some(found);
    }

    if let Some(field) = node.child_by_field_name("name")
        && let Some(n) = ident_within(field, 4)
    {
        return text_of(n, src).map(|t| (t, Some(n)));
    }

    // Swift `init` has no name token worth resolving.
    if node.kind() == "init_declaration" {
        return Some(("init".to_string(), None));
    }

    let mut cur = node;
    while let Some(d) = cur.child_by_field_name("declarator") {
        cur = d;
    }
    if cur.id() != node.id()
        && let Some(n) = ident_within(cur, 6)
    {
        return text_of(n, src).map(|t| (t, Some(n)));
    }

    let n = ident_within(node, 4)?;
    text_of(n, src).map(|t| (t, Some(n)))
}

fn text_of(node: Node, src: &[u8]) -> Option<String> {
    node.utf8_text(src)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn swift_command_label<'a>(node: Node<'a>, src: &[u8]) -> Option<(String, Option<Node<'a>>)> {
    if node.kind() != "call_expression" {
        return None;
    }
    let callee = first_child_identifier_text(node, src)?;
    let is_command_call = matches!(callee.as_str(), "CommandMenu" | "CommandGroup")
        || (callee == "Button" && swift_has_command_ancestor(node, src));
    if !is_command_call {
        return None;
    }
    let label = find_descendant_of_kind(node, "line_string_literal", 6)
        .or_else(|| find_descendant_of_kind(node, "multi_line_string_literal", 6))
        .or_else(|| find_descendant_of_kind(node, "raw_string_literal", 6))?;
    string_literal_text(label, src).map(|name| (name, Some(label)))
}

fn swift_has_command_ancestor(mut node: Node, src: &[u8]) -> bool {
    for _ in 0..8 {
        let Some(parent) = node.parent() else {
            return false;
        };
        if parent.kind() == "call_expression"
            && let Some(callee) = first_child_identifier_text(parent, src)
            && matches!(callee.as_str(), "CommandMenu" | "CommandGroup")
        {
            return true;
        }
        node = parent;
    }
    false
}

fn first_child_identifier_text(node: Node, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if IDENT_KINDS.contains(&child.kind()) {
            return text_of(child, src);
        }
    }
    None
}

fn string_literal_text(node: Node, src: &[u8]) -> Option<String> {
    find_descendant_of_kind(node, "line_str_text", 4)
        .or_else(|| find_descendant_of_kind(node, "multi_line_str_text", 4))
        .and_then(|n| text_of(n, src))
        .or_else(|| {
            text_of(node, src).map(|t| t.trim_matches('#').trim_matches('"').trim().to_string())
        })
        .filter(|s| !s.is_empty())
}

fn objc_method_name<'a>(node: Node<'a>, src: &[u8]) -> Option<(String, Option<Node<'a>>)> {
    if let Some(keyword) = find_descendant_of_kind(node, "keyword_declarator", 4)
        && let Some(name) = ident_within(keyword, 4)
    {
        return text_of(name, src).map(|t| (t, Some(name)));
    }
    if let Some(method) = find_descendant_of_kind(node, "method_identifier", 6) {
        return text_of(method, src).map(|t| (t, Some(method)));
    }
    None
}

fn objc_property_name<'a>(node: Node<'a>, src: &[u8]) -> Option<(String, Option<Node<'a>>)> {
    if let Some(field) = find_descendant_of_kind(node, "field_identifier", 6) {
        return text_of(field, src).map(|t| (t, Some(field)));
    }
    let declaration = find_descendant_of_kind(node, "struct_declaration", 4).unwrap_or(node);
    let ident = last_ident_within(declaration, 8)?;
    text_of(ident, src).map(|t| (t, Some(ident)))
}

/// Depth-bounded search for the first identifier-ish descendant.
fn ident_within(node: Node, max_depth: usize) -> Option<Node> {
    if IDENT_KINDS.contains(&node.kind()) {
        return Some(node);
    }
    if max_depth == 0 {
        return None;
    }
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    for child in children {
        if let Some(found) = ident_within(child, max_depth - 1) {
            return Some(found);
        }
    }
    None
}

/// Depth-bounded search for the last identifier-ish descendant.
fn last_ident_within(node: Node, max_depth: usize) -> Option<Node> {
    let mut found = IDENT_KINDS.contains(&node.kind()).then_some(node);
    if max_depth == 0 {
        return found;
    }
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    for child in children {
        if let Some(candidate) = last_ident_within(child, max_depth - 1) {
            found = Some(candidate);
        }
    }
    found
}

/// Depth-bounded search for the first descendant of an exact kind.
fn find_descendant_of_kind<'a>(node: Node<'a>, kind: &str, max_depth: usize) -> Option<Node<'a>> {
    if node.kind() == kind {
        return Some(node);
    }
    if max_depth == 0 {
        return None;
    }
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    for child in children {
        if let Some(found) = find_descendant_of_kind(child, kind, max_depth - 1) {
            return Some(found);
        }
    }
    None
}

pub(super) fn text_range(node: Node) -> TextRange {
    let start = node.start_position();
    let end = node.end_position();
    TextRange {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: start.row + 1,
        start_col: start.column + 1,
        end_line: end.row + 1,
        end_col: end.column + 1,
    }
}

/// Deterministic FNV-1a — the Tier-1 descriptor hash must be stable across
/// runs (std's `DefaultHasher` makes no such guarantee).
pub(super) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
