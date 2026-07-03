//! Wave C-1 — heuristic cross-file usage resolution over a merged
//! [`SymbolGraph`].
//!
//! The Wave-B tree-sitter pass is file-local: an identifier only becomes an
//! occurrence when its name is defined in the *same* file. Cross-file truth —
//! the Swift intra-module case where files see each other **without**
//! `import`, so the import graph is structurally silent — is resolved here,
//! at snapshot build, after all per-file fragments are merged.
//!
//! Evidence channel: the extractor emits candidate occurrences whose
//! [`SymbolId`] carries the [`UNRESOLVED_PREFIX`] sentinel descriptor
//! (`unresolved::<name>`). This function resolves each candidate against the
//! merged definition table (scope-aware, per-language unit: Swift module /
//! C-family translation-unit pool), rewrites the occurrence to the real
//! symbol id, synthesizes `References`/`Calls` edges from the enclosing
//! declaration, and derives `Conforms`/`Inherits`/`Overrides` edges from
//! declaration signatures. Candidates that stay unresolved are dropped from
//! the merged graph — the raw sentinel occurrences live on in the per-file
//! fragments, so a re-scan re-resolves from scratch.
//!
//! Authority discipline: every synthesized edge is `Confidence::Heuristic`
//! and inherits its provenance from the source occurrence engine. Name+scope
//! resolution is not compiler truth and never pretends to be.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::{
    Confidence, FileSymbolSummary, LanguageId, OccurrenceRole, SymbolEdge, SymbolEdgeKind,
    SymbolGraph, SymbolId, SymbolKind, SymbolProvenance,
};

/// Sentinel descriptor prefix for unresolved candidate occurrences emitted by
/// the Tier-1 extractor. Never appears in a post-resolution merged graph.
pub const UNRESOLVED_PREFIX: &str = "unresolved::";

/// Wrap a bare identifier into the sentinel candidate descriptor.
pub fn unresolved_id(name: &str) -> SymbolId {
    SymbolId(format!("{UNRESOLVED_PREFIX}{name}"))
}

/// Extract the identifier from a sentinel candidate descriptor, if it is one.
pub fn unresolved_name(id: &SymbolId) -> Option<&str> {
    id.as_str().strip_prefix(UNRESOLVED_PREFIX)
}

/// Resolution scope unit (research §4.3): Swift binds per module (files see
/// each other without `import`); C / C++ / ObjC / ObjC++ share one
/// header-include world, so Tier 1 pools them into a single unit and relies
/// on the unique-name guard for honesty.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ScopeUnit {
    Swift(String),
    CFamily,
}

/// Map a repo-relative path to its resolution scope.
fn scope_unit(path: &str) -> Option<ScopeUnit> {
    match path.rsplit('.').next() {
        Some("swift") => Some(ScopeUnit::Swift(swift_module(path))),
        Some("m" | "mm" | "c" | "cc" | "cpp" | "cxx" | "h" | "hpp") => Some(ScopeUnit::CFamily),
        _ => None,
    }
}

/// SwiftPM-layout module heuristic: `Sources/<Module>/...` and
/// `Tests/<Module>/...` name the module; anything else collapses to the
/// empty module (single-module repos and flat fixtures).
fn swift_module(path: &str) -> String {
    let comps: Vec<&str> = path.split('/').collect();
    for (idx, comp) in comps.iter().enumerate() {
        if (*comp == "Sources" || *comp == "Tests") && idx + 2 <= comps.len().saturating_sub(1) {
            return comps[idx + 1].to_string();
        }
    }
    String::new()
}

/// Target kinds a bare-name candidate may bind to. Everything except `Var` /
/// `Field` / `Module` / `Namespace`: locals and members bound by name alone
/// are too noisy even for a labeled heuristic.
fn resolvable_target(kind: &SymbolKind) -> bool {
    !matches!(
        kind,
        SymbolKind::Var | SymbolKind::Field | SymbolKind::Module | SymbolKind::Namespace
    )
}

/// Resolve cross-file usage candidates and synthesize heuristic edges.
///
/// `file_imports` maps a repo-relative path to the module names it imports
/// (`import Pensieve`, `@testable import Pensieve`) so Swift candidates can
/// also bind across an explicit module import, not only intra-module.
pub fn resolve_cross_file(graph: &mut SymbolGraph, file_imports: &HashMap<String, Vec<String>>) {
    if graph.symbols.is_empty() {
        // No definitions to bind to — drop any stray candidates and bail.
        graph
            .occurrences
            .retain(|o| unresolved_name(&o.symbol_id).is_none());
        return;
    }

    // Definition table: (scope, name) -> node indices, plus per-file node
    // lists for enclosing-declaration lookup.
    let mut defs: HashMap<(ScopeUnit, &str), Vec<usize>> = HashMap::new();
    let mut nodes_by_file: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut file_keys: Vec<String> = Vec::new();
    for node in &graph.symbols {
        if let Some(file) = &node.file {
            file_keys.push(file.display().to_string());
        } else {
            file_keys.push(String::new());
        }
    }
    for (idx, node) in graph.symbols.iter().enumerate() {
        let file = file_keys[idx].as_str();
        if file.is_empty() {
            continue;
        }
        nodes_by_file.entry(file).or_default().push(idx);
        if !resolvable_target(&node.kind) {
            continue;
        }
        if let Some(scope) = scope_unit(file) {
            defs.entry((scope, node.name.as_str()))
                .or_default()
                .push(idx);
        }
    }
    for nodes in nodes_by_file.values_mut() {
        nodes.sort_by_key(|idx| {
            graph.symbols[*idx]
                .range
                .map(|range| range.start_byte)
                .unwrap_or(usize::MAX)
        });
    }

    // Unique-name resolution within a scope: the honest Tier-1 guard. A name
    // defined in two places inside one scope stays ambiguous and unresolved.
    let lookup = |scope: &ScopeUnit, name: &str| -> Option<usize> {
        match defs.get(&(scope.clone(), name)) {
            Some(hits) if hits.len() == 1 => Some(hits[0]),
            _ => None,
        }
    };

    let mut edge_seen: HashSet<(SymbolId, SymbolId, SymbolEdgeKind)> = graph
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone(), e.kind))
        .collect();
    let mut new_edges: Vec<SymbolEdge> = Vec::new();
    let push_edge = |new_edges: &mut Vec<SymbolEdge>,
                     edge_seen: &mut HashSet<(SymbolId, SymbolId, SymbolEdgeKind)>,
                     from: &SymbolId,
                     to: &SymbolId,
                     kind: SymbolEdgeKind,
                     provenance: SymbolProvenance| {
        if from == to || from.is_empty() || to.is_empty() {
            return;
        }
        if edge_seen.insert((from.clone(), to.clone(), kind)) {
            new_edges.push(SymbolEdge {
                from: from.clone(),
                to: to.clone(),
                kind,
                provenance,
                confidence: Confidence::Heuristic,
            });
        }
    };

    // Pass 1 — declaration signatures: `Conforms` / `Inherits` (needed
    // before the override pass can walk superclass links).
    let signature_facts: Vec<(usize, Vec<String>)> = graph
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            matches!(
                node.kind,
                SymbolKind::Class
                    | SymbolKind::Struct
                    | SymbolKind::Enum
                    | SymbolKind::Protocol
                    | SymbolKind::Type
            ) || matches!(&node.kind, SymbolKind::Other(tag) if tag == "extension")
        })
        .filter_map(|(idx, node)| {
            let sig = node.signature.as_deref()?;
            let supers = supertypes_from_signature(sig, node.language);
            (!supers.is_empty()).then_some((idx, supers))
        })
        .collect();
    for (idx, supers) in &signature_facts {
        let node = &graph.symbols[*idx];
        let file = file_keys[*idx].as_str();
        let Some(scope) = scope_unit(file) else {
            continue;
        };
        let scopes = candidate_scopes(&scope, file, file_imports);
        for super_name in supers {
            let Some(target_idx) = scopes.iter().find_map(|s| lookup(s, super_name)) else {
                continue;
            };
            let target = &graph.symbols[target_idx];
            let kind = match target.kind {
                SymbolKind::Class => SymbolEdgeKind::Inherits,
                _ => SymbolEdgeKind::Conforms,
            };
            push_edge(
                &mut new_edges,
                &mut edge_seen,
                &node.id,
                &target.id,
                kind,
                node.provenance,
            );
        }
    }

    // Pass 2 — unresolved candidate occurrences: rewrite to real ids,
    // synthesize `References`/`Calls` from the enclosing declaration, and
    // record the file projection (`referenced`) used by `slice`.
    let mut referenced_by_file: HashMap<PathBuf, HashSet<SymbolId>> = HashMap::new();
    let mut resolved_occurrence_edges: Vec<(usize, usize)> = Vec::new(); // (occ idx, target node idx)
    for (occ_idx, occ) in graph.occurrences.iter().enumerate() {
        let Some(name) = unresolved_name(&occ.symbol_id) else {
            continue;
        };
        let occ_file = occ.file.display().to_string();
        let Some(scope) = scope_unit(&occ_file) else {
            continue;
        };
        let scopes = candidate_scopes(&scope, &occ_file, file_imports);
        let Some(target_idx) = scopes.iter().find_map(|s| lookup(s, name)) else {
            continue;
        };
        // Same-file names were already bound by the Wave-B pass; a candidate
        // resolving back into its own file adds nothing.
        if file_keys[target_idx] == occ_file {
            continue;
        }
        resolved_occurrence_edges.push((occ_idx, target_idx));
    }
    for (occ_idx, target_idx) in &resolved_occurrence_edges {
        let target_id = graph.symbols[*target_idx].id.clone();
        let occ_file = graph.occurrences[*occ_idx].file.clone();
        let occ_range = graph.occurrences[*occ_idx].range;
        let role = graph.occurrences[*occ_idx].role;
        let engine = graph.occurrences[*occ_idx].engine;

        let from_id = enclosing_symbol(graph, &nodes_by_file, &file_keys, &occ_file, occ_range);
        if let Some(from_id) = from_id {
            let kind = if role == OccurrenceRole::Call {
                SymbolEdgeKind::Calls
            } else {
                SymbolEdgeKind::References
            };
            push_edge(
                &mut new_edges,
                &mut edge_seen,
                &from_id,
                &target_id,
                kind,
                engine,
            );
        }
        let refs = referenced_by_file.entry(occ_file).or_default();
        refs.insert(target_id.clone());
        graph.occurrences[*occ_idx].symbol_id = target_id;
    }

    // Pass 3 — `Overrides`: a Swift method whose signature carries the
    // `override` modifier binds to the same-name method declared inside the
    // superclass body, when the `Inherits` link resolved in pass 1.
    let inherits: HashMap<&SymbolId, &SymbolId> = graph
        .edges
        .iter()
        .chain(new_edges.iter())
        .filter(|e| e.kind == SymbolEdgeKind::Inherits)
        .map(|e| (&e.from, &e.to))
        .collect();
    let node_by_id: HashMap<&SymbolId, usize> = graph
        .symbols
        .iter()
        .enumerate()
        .map(|(idx, n)| (&n.id, idx))
        .collect();
    let mut override_edges: Vec<(SymbolId, SymbolId, SymbolProvenance)> = Vec::new();
    for (idx, node) in graph.symbols.iter().enumerate() {
        if node.kind != SymbolKind::Method
            || !node
                .signature
                .as_deref()
                .is_some_and(|s| s.split_whitespace().any(|w| w == "override"))
        {
            continue;
        }
        let file = file_keys[idx].as_str();
        let Some(range) = node.range else { continue };
        // Enclosing class of the override, then its resolved superclass.
        let Some(class_id) = enclosing_symbol(
            graph,
            &nodes_by_file,
            &file_keys,
            &PathBuf::from(file),
            range,
        ) else {
            continue;
        };
        let Some(super_id) = inherits.get(&class_id) else {
            continue;
        };
        let Some(&super_idx) = node_by_id.get(*super_id) else {
            continue;
        };
        let super_node = &graph.symbols[super_idx];
        let (Some(super_file), Some(super_range)) = (&super_node.file, super_node.range) else {
            continue;
        };
        let super_file_key = super_file.display().to_string();
        let target = nodes_by_file
            .get(super_file_key.as_str())
            .into_iter()
            .flatten()
            .map(|&i| &graph.symbols[i])
            .find(|n| {
                n.kind == SymbolKind::Method
                    && n.name == node.name
                    && n.range.is_some_and(|r| {
                        r.start_byte >= super_range.start_byte && r.end_byte <= super_range.end_byte
                    })
            });
        if let Some(target) = target {
            override_edges.push((node.id.clone(), target.id.clone(), node.provenance));
        }
    }
    for (from, to, provenance) in override_edges {
        push_edge(
            &mut new_edges,
            &mut edge_seen,
            &from,
            &to,
            SymbolEdgeKind::Overrides,
            provenance,
        );
    }

    // Candidates that stayed unresolved are heuristic noise in a merged
    // graph; the raw sentinels persist in the per-file fragments.
    graph
        .occurrences
        .retain(|o| unresolved_name(&o.symbol_id).is_none());

    new_edges.sort_by(|a, b| {
        a.from
            .as_str()
            .cmp(b.from.as_str())
            .then_with(|| a.to.as_str().cmp(b.to.as_str()))
    });
    graph.edges.extend(new_edges);

    // Merge resolved references into the per-file projections.
    for (file, refs) in referenced_by_file {
        let mut refs: Vec<SymbolId> = refs.into_iter().collect();
        refs.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        if let Some(entry) = graph.file_projection.iter_mut().find(|p| p.file == file) {
            for id in refs {
                if !entry.referenced.contains(&id) {
                    entry.referenced.push(id);
                }
            }
        } else {
            graph.file_projection.push(FileSymbolSummary {
                file,
                defined: Vec::new(),
                referenced: refs,
            });
        }
    }
    graph.file_projection.sort_by(|a, b| a.file.cmp(&b.file));
}

/// Scopes a file may resolve into: its own unit first, then (Swift only) any
/// module it explicitly imports — the `@testable import App` path.
fn candidate_scopes(
    own: &ScopeUnit,
    file: &str,
    file_imports: &HashMap<String, Vec<String>>,
) -> Vec<ScopeUnit> {
    let mut scopes = vec![own.clone()];
    if matches!(own, ScopeUnit::Swift(_))
        && let Some(imports) = file_imports.get(file)
    {
        for module in imports {
            let scope = ScopeUnit::Swift(module.clone());
            if !scopes.contains(&scope) {
                scopes.push(scope);
            }
        }
    }
    scopes
}

/// Innermost declaration in `file` whose range contains `range` — the "from"
/// end of a synthesized usage edge.
fn enclosing_symbol(
    graph: &SymbolGraph,
    nodes_by_file: &HashMap<&str, Vec<usize>>,
    file_keys: &[String],
    file: &std::path::Path,
    range: super::TextRange,
) -> Option<SymbolId> {
    let key = file.display().to_string();
    let mut best: Option<(usize, usize)> = None; // (span, node idx)
    for &idx in nodes_by_file.get(key.as_str())? {
        debug_assert_eq!(file_keys[idx], key);
        let Some(node_range) = graph.symbols[idx].range else {
            continue;
        };
        if node_range.start_byte > range.start_byte {
            break;
        }
        let contains = node_range.start_byte <= range.start_byte
            && node_range.end_byte >= range.end_byte
            && (node_range.start_byte, node_range.end_byte) != (range.start_byte, range.end_byte);
        if !contains {
            continue;
        }
        let span = node_range.end_byte - node_range.start_byte;
        if best.is_none_or(|(best_span, _)| span < best_span) {
            best = Some((span, idx));
        }
    }
    best.map(|(_, idx)| graph.symbols[idx].id.clone())
}

/// Parse supertype names out of a declaration signature (first source line of
/// the declaration). Swift `class Foo<T: Equatable>: Bar, Baz where ...`,
/// ObjC `@interface Foo : Bar <P1, P2>`, C++ `class D : public B`.
fn supertypes_from_signature(signature: &str, language: LanguageId) -> Vec<String> {
    let sig = strip_generic_params(signature);
    let head = sig.split(['{', ';']).next().unwrap_or("");
    let Some((_, clause)) = head.split_once(':') else {
        return Vec::new();
    };
    let clause = clause.split(" where ").next().unwrap_or("");

    let mut out: Vec<String> = Vec::new();
    let mut push_name = |raw: &str| {
        let token = raw
            .trim()
            .trim_end_matches(['?', '!'])
            .rsplit("::")
            .next()
            .unwrap_or("")
            .rsplit('.')
            .next()
            .unwrap_or("")
            .trim();
        if !token.is_empty()
            && token.chars().next().is_some_and(unicode_ident_start)
            && token.chars().all(|c| c.is_alphanumeric() || c == '_')
            && !out.iter().any(|n| n == token)
        {
            out.push(token.to_string());
        }
    };

    match language {
        LanguageId::ObjC | LanguageId::ObjCpp => {
            // Superclass before `<`, protocol list inside `<...>`.
            let (superclass, protocols) = match clause.split_once('<') {
                Some((sup, rest)) => (sup, rest.trim_end().trim_end_matches('>')),
                None => (clause, ""),
            };
            push_name(superclass);
            for proto in protocols.split(',') {
                push_name(proto);
            }
        }
        _ => {
            for item in clause.split(',').flat_map(|part| part.split('&')) {
                // C++ base clauses carry access/virtual specifiers.
                let name = item
                    .split_whitespace()
                    .rfind(|w| {
                        !matches!(*w, "public" | "private" | "protected" | "virtual" | "final")
                    })
                    .unwrap_or("");
                push_name(name);
            }
        }
    }
    out
}

fn unicode_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

/// Erase balanced `<...>` spans so generic parameter lists do not confuse the
/// `:` clause split (`class Foo<T: Equatable>: Bar`).
fn strip_generic_params(signature: &str) -> String {
    let mut out = String::with_capacity(signature.len());
    let mut depth = 0usize;
    let mut chars = signature.chars().peekable();
    // ObjC protocol lists also use `<...>` but follow a `:` + superclass,
    // which the ObjC arm re-reads from the raw clause — here we only protect
    // the split for Swift/C++ where `<` before `:` is generics.
    let colon_pos = signature.find(':');
    let mut pos = 0usize;
    while let Some(c) = chars.next() {
        let _ = &mut chars;
        match c {
            '<' if colon_pos.is_some_and(|cp| pos < cp) => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
        pos += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::{SymbolNode, SymbolOccurrence, SymbolProvenance, TextRange};
    use super::*;

    fn node(
        file: &str,
        kind: SymbolKind,
        name: &str,
        lang: LanguageId,
        start: usize,
        end: usize,
        signature: &str,
    ) -> SymbolNode {
        SymbolNode {
            id: SymbolId::from_parts(file, "k", name, start as u64),
            language: lang,
            kind,
            name: name.to_string(),
            qualified_name: None,
            module: None,
            usr: None,
            file: Some(PathBuf::from(file)),
            range: Some(TextRange {
                start_byte: start,
                end_byte: end,
                start_line: 1,
                start_col: 1,
                end_line: 1,
                end_col: 1,
            }),
            signature: Some(signature.to_string()),
            visibility: None,
            provenance: SymbolProvenance::TreeSitter,
        }
    }

    fn candidate(file: &str, name: &str, role: OccurrenceRole, at: usize) -> SymbolOccurrence {
        SymbolOccurrence {
            symbol_id: unresolved_id(name),
            file: PathBuf::from(file),
            range: TextRange {
                start_byte: at,
                end_byte: at + name.len(),
                start_line: 1,
                start_col: 1,
                end_line: 1,
                end_col: 1,
            },
            role,
            confidence: Confidence::Heuristic,
            engine: SymbolProvenance::TreeSitter,
        }
    }

    fn two_file_swift_graph() -> SymbolGraph {
        let mut graph = SymbolGraph::new();
        graph.symbols.push(node(
            "swift/DocumentStore.swift",
            SymbolKind::Class,
            "DocumentStore",
            LanguageId::Swift,
            0,
            400,
            "final class DocumentStore: DocumentPersisting {",
        ));
        graph.symbols.push(node(
            "swift/DocumentStore.swift",
            SymbolKind::Protocol,
            "DocumentPersisting",
            LanguageId::Swift,
            500,
            600,
            "protocol DocumentPersisting {",
        ));
        graph.symbols.push(node(
            "swift/DocumentStoreTests.swift",
            SymbolKind::Class,
            "DocumentStoreTests",
            LanguageId::Swift,
            0,
            900,
            "final class DocumentStoreTests: XCTestCase {",
        ));
        graph.symbols.push(node(
            "swift/DocumentStoreTests.swift",
            SymbolKind::Method,
            "testPersistEmitsNotification",
            LanguageId::Swift,
            50,
            850,
            "func testPersistEmitsNotification() {",
        ));
        graph.occurrences.push(candidate(
            "swift/DocumentStoreTests.swift",
            "DocumentStore",
            OccurrenceRole::Call,
            120,
        ));
        graph
    }

    #[test]
    fn swift_same_module_call_resolves_with_heuristic_confidence() {
        let mut graph = two_file_swift_graph();
        resolve_cross_file(&mut graph, &HashMap::new());

        let store_id = graph.symbols[0].id.clone();
        let call = graph
            .edges
            .iter()
            .find(|e| e.kind == SymbolEdgeKind::Calls)
            .expect("cross-file Calls edge");
        assert_eq!(call.to, store_id);
        assert_eq!(call.confidence, Confidence::Heuristic);
        assert_eq!(call.provenance, SymbolProvenance::TreeSitter);
        // From-end is the innermost enclosing declaration (the test method).
        assert_eq!(call.from, graph.symbols[3].id);

        // Occurrence is rewritten to the real id; no sentinel survives.
        assert!(
            graph
                .occurrences
                .iter()
                .all(|o| unresolved_name(&o.symbol_id).is_none())
        );
        assert!(graph.occurrences.iter().any(|o| o.symbol_id == store_id
            && o.file.as_path() == std::path::Path::new("swift/DocumentStoreTests.swift")));

        // Projection records the consumer for `slice`.
        let proj = graph
            .file_projection
            .iter()
            .find(|p| p.file.as_path() == std::path::Path::new("swift/DocumentStoreTests.swift"))
            .expect("consumer projection");
        assert!(proj.referenced.contains(&store_id));
    }

    #[test]
    fn conforms_edge_from_signature() {
        let mut graph = two_file_swift_graph();
        resolve_cross_file(&mut graph, &HashMap::new());
        let conforms = graph
            .edges
            .iter()
            .find(|e| e.kind == SymbolEdgeKind::Conforms)
            .expect("Conforms edge from inheritance clause");
        assert_eq!(conforms.from, graph.symbols[0].id);
        assert_eq!(conforms.to, graph.symbols[1].id);
        assert_eq!(conforms.confidence, Confidence::Heuristic);
    }

    #[test]
    fn ambiguous_names_stay_unresolved_and_are_dropped() {
        let mut graph = two_file_swift_graph();
        // Second `DocumentStore` in the same module makes the name ambiguous.
        graph.symbols.push(node(
            "swift/Other.swift",
            SymbolKind::Class,
            "DocumentStore",
            LanguageId::Swift,
            0,
            100,
            "class DocumentStore {",
        ));
        resolve_cross_file(&mut graph, &HashMap::new());
        assert!(
            !graph.edges.iter().any(|e| e.kind == SymbolEdgeKind::Calls),
            "ambiguous candidate must not synthesize a Calls edge"
        );
        assert!(
            graph
                .occurrences
                .iter()
                .all(|o| unresolved_name(&o.symbol_id).is_none()),
            "unresolved candidates must be dropped from the merged graph"
        );
    }

    #[test]
    fn swift_modules_isolate_resolution() {
        let mut graph = SymbolGraph::new();
        graph.symbols.push(node(
            "Sources/AppA/Store.swift",
            SymbolKind::Class,
            "Store",
            LanguageId::Swift,
            0,
            100,
            "class Store {",
        ));
        graph.occurrences.push(candidate(
            "Sources/AppB/Consumer.swift",
            "Store",
            OccurrenceRole::Reference,
            10,
        ));
        resolve_cross_file(&mut graph, &HashMap::new());
        assert!(
            graph.edges.is_empty(),
            "AppB must not see AppA without import"
        );

        // With an explicit `import AppA` the same candidate resolves.
        let mut graph2 = SymbolGraph::new();
        graph2.symbols.push(node(
            "Sources/AppA/Store.swift",
            SymbolKind::Class,
            "Store",
            LanguageId::Swift,
            0,
            100,
            "class Store {",
        ));
        graph2.symbols.push(node(
            "Sources/AppB/Consumer.swift",
            SymbolKind::Class,
            "Consumer",
            LanguageId::Swift,
            0,
            200,
            "class Consumer {",
        ));
        graph2.occurrences.push(candidate(
            "Sources/AppB/Consumer.swift",
            "Store",
            OccurrenceRole::Reference,
            10,
        ));
        let imports = HashMap::from([(
            "Sources/AppB/Consumer.swift".to_string(),
            vec!["AppA".to_string()],
        )]);
        resolve_cross_file(&mut graph2, &imports);
        assert!(
            graph2
                .edges
                .iter()
                .any(|e| e.kind == SymbolEdgeKind::References),
            "explicit module import must open the scope"
        );
    }

    #[test]
    fn swift_override_binds_to_superclass_method() {
        let mut graph = SymbolGraph::new();
        graph.symbols.push(node(
            "swift/Base.swift",
            SymbolKind::Class,
            "BaseController",
            LanguageId::Swift,
            0,
            500,
            "class BaseController {",
        ));
        graph.symbols.push(node(
            "swift/Base.swift",
            SymbolKind::Method,
            "reload",
            LanguageId::Swift,
            50,
            120,
            "func reload() {",
        ));
        graph.symbols.push(node(
            "swift/Child.swift",
            SymbolKind::Class,
            "ChildController",
            LanguageId::Swift,
            0,
            500,
            "class ChildController: BaseController {",
        ));
        graph.symbols.push(node(
            "swift/Child.swift",
            SymbolKind::Method,
            "reload",
            LanguageId::Swift,
            60,
            140,
            "override func reload() {",
        ));
        resolve_cross_file(&mut graph, &HashMap::new());
        let overrides = graph
            .edges
            .iter()
            .find(|e| e.kind == SymbolEdgeKind::Overrides)
            .expect("Overrides edge");
        assert_eq!(overrides.from, graph.symbols[3].id);
        assert_eq!(overrides.to, graph.symbols[1].id);
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.kind == SymbolEdgeKind::Inherits),
            "Inherits edge from the class signature"
        );
    }

    #[test]
    fn objc_signature_supertypes() {
        let supers = supertypes_from_signature(
            "@interface EditorViewController : NSViewController <NSTextViewDelegate, Persisting>",
            LanguageId::ObjC,
        );
        assert_eq!(
            supers,
            vec!["NSViewController", "NSTextViewDelegate", "Persisting"]
        );
    }

    #[test]
    fn swift_generic_signature_supertypes() {
        let supers = supertypes_from_signature(
            "final class Cache<Key: Hashable, Value>: Storage, Codable where Key: Sendable {",
            LanguageId::Swift,
        );
        assert_eq!(supers, vec!["Storage", "Codable"]);
    }

    #[test]
    fn cpp_base_clause_supertypes() {
        let supers = supertypes_from_signature(
            "class Document : public Persistable, private detail::Buffer {",
            LanguageId::Cpp,
        );
        assert_eq!(supers, vec!["Persistable", "Buffer"]);
    }
}
