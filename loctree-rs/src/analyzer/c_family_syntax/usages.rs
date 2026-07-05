//! Local reference/call occurrences for symbols defined in the same file.
//!
//! Wave-B scope is intentionally file-local: identifiers matching a symbol
//! defined in this file become `Reference`/`Call` occurrences (confidence
//! `Heuristic`). Identifiers matching nothing local are emitted as
//! **unresolved candidates** (sentinel `unresolved::<name>` descriptor,
//! deduplicated per `(name, role)`) so the Wave C-1 snapshot-build resolver
//! can bind them cross-file; candidates that stay unresolved are dropped
//! from the merged graph there.

use crate::symbols::{
    Confidence, OccurrenceRole, SymbolId, SymbolOccurrence, SymbolProvenance, resolve,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tree_sitter::{Node, Tree};

/// Cap on usage occurrences per file to keep snapshots bounded.
const MAX_USAGES_PER_FILE: usize = 2000;

/// Cap on distinct unresolved candidates per file: one occurrence per
/// `(name, role)` pair is all the cross-file resolver needs.
const MAX_UNRESOLVED_PER_FILE: usize = 512;

/// Names that are never useful cross-file candidates.
const CANDIDATE_BLOCKLIST: &[&str] = &["self", "super", "Self", "init", "_"];

/// Node kinds whose parent marks the identifier as a call site.
const CALL_PARENT_KINDS: &[&str] = &[
    "call_expression",    // C / C++ / Swift / ObjC function calls
    "call_suffix",        // Swift call argument suffix
    "message_expression", // ObjC [receiver selector]
];

pub(super) fn collect_usages(
    tree: &Tree,
    content: &str,
    relative: &str,
    defined: &HashMap<String, SymbolId>,
    definition_name_ranges: &[(usize, usize)],
) -> Vec<SymbolOccurrence> {
    let mut out = Vec::new();
    let src = content.as_bytes();
    let mut candidate_seen: HashSet<(String, OccurrenceRole)> = HashSet::new();
    let definition_name_ranges: HashSet<(usize, usize)> =
        definition_name_ranges.iter().copied().collect();

    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if out.len() >= MAX_USAGES_PER_FILE {
            break;
        }
        if is_identifier(node) {
            let range = (node.start_byte(), node.end_byte());
            if !definition_name_ranges.contains(&range)
                && let Ok(text) = node.utf8_text(src)
            {
                let role = occurrence_role(node);
                if let Some(symbol_id) = defined.get(text) {
                    out.push(SymbolOccurrence {
                        symbol_id: symbol_id.clone(),
                        file: PathBuf::from(relative),
                        range: super::symbols::text_range(node),
                        role,
                        confidence: Confidence::Heuristic,
                        engine: SymbolProvenance::TreeSitter,
                    });
                } else if text.len() >= 2
                    && !CANDIDATE_BLOCKLIST.contains(&text)
                    && candidate_seen.len() < MAX_UNRESOLVED_PER_FILE
                    && candidate_seen.insert((text.to_string(), role))
                {
                    out.push(SymbolOccurrence {
                        symbol_id: resolve::unresolved_id(text),
                        file: PathBuf::from(relative),
                        range: super::symbols::text_range(node),
                        role,
                        confidence: Confidence::Heuristic,
                        engine: SymbolProvenance::TreeSitter,
                    });
                }
            }
            continue;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.is_named() {
                stack.push(child);
            }
        }
    }
    out
}

fn is_identifier(node: Node) -> bool {
    matches!(
        node.kind(),
        "identifier"
            | "simple_identifier"
            | "type_identifier"
            | "field_identifier"
            | "method_identifier"
    )
}

fn occurrence_role(node: Node) -> OccurrenceRole {
    let mut cur = node.parent();
    let mut hops = 0;
    while let Some(p) = cur {
        if hops >= 4 {
            break;
        }
        if CALL_PARENT_KINDS.contains(&p.kind()) {
            return OccurrenceRole::Call;
        }
        cur = p.parent();
        hops += 1;
    }
    OccurrenceRole::Reference
}
