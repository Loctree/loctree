//! C-family runtime dispatch idioms (Swift / ObjC) layered on top of the
//! build-free symbol graph.
//!
//! This module deliberately stays heuristic: NotificationCenter and selector
//! dispatch are paired by static literal/name text only. No KVO,
//! `performSelector`, storyboard/XIB, or compiler-index inference is attempted.

use crate::symbols::{
    Confidence, LanguageId, SymbolEdge, SymbolEdgeKind, SymbolEngineRun, SymbolGraph, SymbolId,
    SymbolKind, SymbolNode, SymbolProvenance, SymbolVisibility, TextRange,
};
use crate::types::FileAnalysis;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Enrich the merged snapshot symbol graph with Swift/ObjC runtime dispatch
/// edges discovered from static source idioms.
pub(crate) fn enrich_symbol_graph(
    graph: &mut SymbolGraph,
    files: &[FileAnalysis],
    roots: &[PathBuf],
) {
    let before = graph.edges.len();

    for file in files {
        let Some(language) = language_for_path(&file.path) else {
            continue;
        };
        let Some(source_path) = resolve_source_path(&file.path, roots) else {
            continue;
        };
        let Ok(source) = fs::read_to_string(&source_path) else {
            continue;
        };
        enrich_source(graph, &file.path, language, &source);
    }

    let added = graph.edges.len().saturating_sub(before);
    if added > 0 {
        if let Some(run) = graph
            .engines
            .iter_mut()
            .find(|run| run.engine == SymbolProvenance::Heuristic && run.tool_version.is_none())
        {
            run.symbol_count = graph
                .symbols
                .iter()
                .filter(|node| node.provenance == SymbolProvenance::Heuristic)
                .count();
        } else {
            graph.engines.push(SymbolEngineRun {
                engine: SymbolProvenance::Heuristic,
                symbol_count: graph
                    .symbols
                    .iter()
                    .filter(|node| node.provenance == SymbolProvenance::Heuristic)
                    .count(),
                occurrence_count: 0,
                tool_version: None,
            });
        }
    }
}

fn enrich_source(graph: &mut SymbolGraph, file: &str, language: LanguageId, source: &str) {
    let constants = objc_notification_constants(source);
    let hits = match language {
        LanguageId::Swift => swift_hits(file, source),
        LanguageId::ObjC | LanguageId::ObjCpp => objc_hits(file, source, &constants),
        LanguageId::C | LanguageId::Cpp => Vec::new(),
    };
    let mut node_ids: HashSet<String> = graph
        .symbols
        .iter()
        .map(|symbol| symbol.id.as_str().to_string())
        .collect();
    let mut edge_ids: HashSet<(String, String, SymbolEdgeKind)> = graph
        .edges
        .iter()
        .map(|edge| {
            (
                edge.from.as_str().to_string(),
                edge.to.as_str().to_string(),
                edge.kind,
            )
        })
        .collect();

    for hit in hits {
        let source_id = SymbolId::new(format!(
            "{file}::runtime::{kind}::{line}::{name}",
            kind = hit.source_kind,
            line = hit.line,
            name = stable_fragment(&hit.name)
        ));
        let target_id = SymbolId::new(format!(
            "runtime::{kind}::{name}",
            kind = hit.target_kind,
            name = stable_fragment(&hit.name)
        ));

        push_node(
            graph,
            &mut node_ids,
            SymbolNode {
                id: source_id.clone(),
                language,
                kind: SymbolKind::Other(hit.source_kind.to_string()),
                name: format!("{} {}", hit.source_kind, hit.name),
                qualified_name: None,
                module: None,
                usr: None,
                file: Some(PathBuf::from(file)),
                range: Some(line_range(source, hit.line)),
                signature: None,
                visibility: Some(SymbolVisibility::Unknown),
                provenance: SymbolProvenance::Heuristic,
            },
        );
        push_node(
            graph,
            &mut node_ids,
            SymbolNode {
                id: target_id.clone(),
                language,
                kind: SymbolKind::Other(hit.target_kind.to_string()),
                name: hit.name.clone(),
                qualified_name: None,
                module: None,
                usr: None,
                file: None,
                range: None,
                signature: None,
                visibility: Some(SymbolVisibility::Unknown),
                provenance: SymbolProvenance::Heuristic,
            },
        );
        push_edge(
            graph,
            &mut edge_ids,
            SymbolEdge {
                from: source_id,
                to: target_id,
                kind: hit.edge_kind,
                provenance: SymbolProvenance::Heuristic,
                confidence: Confidence::Heuristic,
            },
        );
    }
}

#[derive(Debug, Clone)]
struct RuntimeHit {
    line: usize,
    name: String,
    edge_kind: SymbolEdgeKind,
    source_kind: &'static str,
    target_kind: &'static str,
}

fn swift_hits(file: &str, source: &str) -> Vec<RuntimeHit> {
    let mut hits = Vec::new();
    for (idx, _) in source.match_indices(".post") {
        if is_line_comment(source, idx) {
            continue;
        }
        let window = call_window(source, idx, 6);
        if window.contains("NotificationCenter") || window.contains("name:") {
            if let Some(name) = extract_argument_name(window, &["name:"]) {
                hits.push(RuntimeHit {
                    line: line_number(source, idx),
                    name,
                    edge_kind: SymbolEdgeKind::NotificationEmit,
                    source_kind: "notification_emit_site",
                    target_kind: "notification",
                });
            }
        }
    }
    for (idx, _) in source.match_indices("addObserver") {
        if is_line_comment(source, idx) {
            continue;
        }
        let window = call_window(source, idx, 8);
        if let Some(name) = extract_argument_name(window, &["forName:", "name:"]) {
            hits.push(RuntimeHit {
                line: line_number(source, idx),
                name,
                edge_kind: SymbolEdgeKind::NotificationObserve,
                source_kind: "notification_observe_site",
                target_kind: "notification",
            });
        }
    }
    for (idx, _) in source.match_indices("publisher") {
        if is_line_comment(source, idx) {
            continue;
        }
        let window = call_window(source, idx, 6);
        if let Some(name) = extract_argument_name(window, &["for:"]) {
            hits.push(RuntimeHit {
                line: line_number(source, idx),
                name,
                edge_kind: SymbolEdgeKind::NotificationObserve,
                source_kind: "notification_observe_site",
                target_kind: "notification",
            });
        }
    }
    dedupe_hits(file, hits)
}

fn objc_hits(file: &str, source: &str, constants: &HashMap<String, String>) -> Vec<RuntimeHit> {
    let mut hits = Vec::new();

    for (idx, _) in source.match_indices("postNotificationName:") {
        if is_line_comment(source, idx) {
            continue;
        }
        let window = call_window(source, idx, 4);
        if let Some(name) = extract_argument_name(window, &["postNotificationName:"])
            .map(|name| resolve_objc_constant(&name, constants))
        {
            hits.push(RuntimeHit {
                line: line_number(source, idx),
                name,
                edge_kind: SymbolEdgeKind::NotificationEmit,
                source_kind: "notification_emit_site",
                target_kind: "notification",
            });
        }
    }
    for (idx, _) in source.match_indices("addObserver") {
        if is_line_comment(source, idx) {
            continue;
        }
        let window = call_window(source, idx, 8);
        if let Some(name) = extract_argument_name(window, &["name:"])
            .map(|name| resolve_objc_constant(&name, constants))
        {
            hits.push(RuntimeHit {
                line: line_number(source, idx),
                name,
                edge_kind: SymbolEdgeKind::NotificationObserve,
                source_kind: "notification_observe_site",
                target_kind: "notification",
            });
        }
        hits.extend(selector_hits(file, source, idx, window));
    }
    for (idx, _) in source.match_indices("addTarget") {
        if is_line_comment(source, idx) {
            continue;
        }
        let window = call_window(source, idx, 8);
        hits.extend(selector_hits(file, source, idx, window));
    }

    dedupe_hits(file, hits)
}

fn selector_hits(_file: &str, source: &str, idx: usize, window: &str) -> Vec<RuntimeHit> {
    let mut hits = Vec::new();
    if !window.contains("@selector(") {
        return hits;
    }
    let mut remaining = window;
    while let Some(selector_idx) = remaining.find("@selector(") {
        let after = &remaining[selector_idx + "@selector(".len()..];
        if let Some(end) = after.find(')') {
            let selector = after[..end].trim();
            if !selector.is_empty() {
                hits.push(RuntimeHit {
                    line: line_number(source, idx),
                    name: selector.to_string(),
                    edge_kind: SymbolEdgeKind::SelectorMessage,
                    source_kind: "selector_message_site",
                    target_kind: "selector",
                });
            }
            remaining = &after[end + 1..];
        } else {
            break;
        }
    }
    hits
}

fn objc_notification_constants(source: &str) -> HashMap<String, String> {
    let mut constants = HashMap::new();
    for line in source.lines() {
        if !line.contains("NSString") || !line.contains('=') || !line.contains("@\"") {
            continue;
        }
        let Some((left, right)) = line.split_once('=') else {
            continue;
        };
        let Some(name) = left
            .split(|ch: char| ch.is_whitespace() || ch == '*')
            .rfind(|part| !part.is_empty())
        else {
            continue;
        };
        if let Some(value) = parse_quoted_literal(right) {
            constants.insert(name.to_string(), value);
        }
    }
    constants
}

fn extract_argument_name(window: &str, labels: &[&str]) -> Option<String> {
    labels.iter().find_map(|label| {
        let label_idx = window.find(label)?;
        let expr = argument_expr(&window[label_idx + label.len()..]);
        normalize_dispatch_name(expr)
    })
}

fn argument_expr(input: &str) -> &str {
    let trimmed = input.trim_start();
    let end = trimmed
        .char_indices()
        .find_map(|(idx, ch)| matches!(ch, ',' | ')' | ']' | '\n').then_some(idx))
        .unwrap_or(trimmed.len());
    trimmed[..end].trim()
}

fn normalize_dispatch_name(expr: &str) -> Option<String> {
    let expr = expr.trim();
    if expr.is_empty() || expr == "nil" || expr == "NULL" {
        return None;
    }
    if let Some(value) = parse_quoted_literal(expr) {
        return Some(value);
    }
    if let Some(rest) = expr.strip_prefix("Notification.Name.") {
        return Some(format!(".{}", take_identifier(rest)?));
    }
    if let Some(rest) = expr.strip_prefix(".") {
        return Some(format!(".{}", take_identifier(rest)?));
    }
    Some(take_dispatch_token(expr)?.to_string())
}

fn parse_quoted_literal(input: &str) -> Option<String> {
    let start = input.find('"')? + 1;
    let rest = &input[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn resolve_objc_constant(name: &str, constants: &HashMap<String, String>) -> String {
    constants
        .get(name)
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

fn take_identifier(input: &str) -> Option<&str> {
    let end = input
        .char_indices()
        .find_map(|(idx, ch)| (!(ch.is_alphanumeric() || ch == '_' || ch == ':')).then_some(idx))
        .unwrap_or(input.len());
    (end > 0).then_some(&input[..end])
}

fn take_dispatch_token(input: &str) -> Option<&str> {
    let end = input
        .char_indices()
        .find_map(|(idx, ch)| {
            (!(ch.is_alphanumeric() || ch == '_' || ch == ':' || ch == '.')).then_some(idx)
        })
        .unwrap_or(input.len());
    (end > 0).then_some(&input[..end])
}

fn call_window(source: &str, start: usize, max_lines: usize) -> &str {
    let mut lines = 0usize;
    for (idx, ch) in source[start..].char_indices() {
        if ch == '\n' {
            lines += 1;
            if lines >= max_lines {
                return &source[start..start + idx];
            }
        }
    }
    &source[start..]
}

fn line_number(source: &str, byte_idx: usize) -> usize {
    source[..byte_idx]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn is_line_comment(source: &str, byte_idx: usize) -> bool {
    let line_start = source[..byte_idx]
        .rfind('\n')
        .map(|idx| idx + 1)
        .unwrap_or(0);
    source[line_start..byte_idx].trim_start().starts_with("//")
}

fn line_range(source: &str, line: usize) -> TextRange {
    let mut start_byte = 0usize;
    let mut current_line = 1usize;
    for (idx, byte) in source.bytes().enumerate() {
        if current_line == line {
            start_byte = idx;
            break;
        }
        if byte == b'\n' {
            current_line += 1;
        }
    }
    let end_byte = source[start_byte..]
        .find('\n')
        .map(|idx| start_byte + idx)
        .unwrap_or(source.len());
    TextRange {
        start_byte,
        end_byte,
        start_line: line,
        start_col: 1,
        end_line: line,
        end_col: end_byte.saturating_sub(start_byte) + 1,
    }
}

fn dedupe_hits(_file: &str, hits: Vec<RuntimeHit>) -> Vec<RuntimeHit> {
    let mut seen = HashSet::new();
    hits.into_iter()
        .filter(|hit| seen.insert((hit.line, hit.name.clone(), hit.edge_kind)))
        .collect()
}

fn push_node(graph: &mut SymbolGraph, seen: &mut HashSet<String>, node: SymbolNode) {
    if seen.insert(node.id.as_str().to_string()) {
        graph.symbols.push(node);
    }
}

fn push_edge(
    graph: &mut SymbolGraph,
    seen: &mut HashSet<(String, String, SymbolEdgeKind)>,
    edge: SymbolEdge,
) {
    let key = (
        edge.from.as_str().to_string(),
        edge.to.as_str().to_string(),
        edge.kind,
    );
    if seen.insert(key) {
        graph.edges.push(edge);
    }
}

fn stable_fragment(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '_' || ch == ':' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn language_for_path(path: &str) -> Option<LanguageId> {
    match Path::new(path).extension().and_then(|ext| ext.to_str()) {
        Some("swift") => Some(LanguageId::Swift),
        Some("m") | Some("h") => Some(LanguageId::ObjC),
        Some("mm") => Some(LanguageId::ObjCpp),
        _ => None,
    }
}

fn resolve_source_path(path: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    let raw = PathBuf::from(path);
    if raw.is_absolute() && raw.exists() {
        return Some(raw);
    }
    roots
        .iter()
        .map(|root| root.join(path))
        .find(|candidate| candidate.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swift_notification_center_edges_share_literal_target() {
        let source = r#"
import Foundation

final class DocumentEvents {
    func publishChange() {
        NotificationCenter.default.post(name: .vcDocumentChanged, object: self)
    }

    func observeChange() {
        NotificationCenter.default.addObserver(forName: .vcDocumentChanged, object: nil, queue: .main) { _ in }
    }
}
"#;
        let mut graph = SymbolGraph::new();
        enrich_source(
            &mut graph,
            "Sources/App/DocumentEvents.swift",
            LanguageId::Swift,
            source,
        );

        let emit = edge_to(
            &graph,
            SymbolEdgeKind::NotificationEmit,
            ".vcDocumentChanged",
        );
        let observe = edge_to(
            &graph,
            SymbolEdgeKind::NotificationObserve,
            ".vcDocumentChanged",
        );
        assert!(emit.is_some(), "missing emit edge: {:#?}", graph.edges);
        assert!(
            observe.is_some(),
            "missing observe edge: {:#?}",
            graph.edges
        );
        assert_eq!(emit.unwrap().to, observe.unwrap().to);
    }

    #[test]
    fn objc_notification_and_selector_edges_are_static() {
        let source = r#"
static NSString * const VCDocumentChangedNotification = @"vcDocumentChanged";

- (void)wireUp:(UIButton *)button {
    [[NSNotificationCenter defaultCenter] addObserver:self selector:@selector(handleDocumentChanged:) name:VCDocumentChangedNotification object:nil];
    [button addTarget:self action:@selector(handleTap:) forControlEvents:UIControlEventTouchUpInside];
}

- (void)publish {
    [[NSNotificationCenter defaultCenter] postNotificationName:VCDocumentChangedNotification object:self];
}
"#;
        let mut graph = SymbolGraph::new();
        enrich_source(
            &mut graph,
            "objc/EditorViewController.m",
            LanguageId::ObjC,
            source,
        );

        assert!(
            edge_to(
                &graph,
                SymbolEdgeKind::NotificationEmit,
                "vcDocumentChanged"
            )
            .is_some(),
            "missing ObjC notification emit: {:#?}",
            graph.edges
        );
        assert!(
            edge_to(
                &graph,
                SymbolEdgeKind::NotificationObserve,
                "vcDocumentChanged"
            )
            .is_some(),
            "missing ObjC notification observe: {:#?}",
            graph.edges
        );
        assert!(
            edge_to(
                &graph,
                SymbolEdgeKind::SelectorMessage,
                "handleDocumentChanged:"
            )
            .is_some(),
            "missing NSNotificationCenter selector: {:#?}",
            graph.edges
        );
        assert!(
            edge_to(&graph, SymbolEdgeKind::SelectorMessage, "handleTap:").is_some(),
            "missing target-action selector: {:#?}",
            graph.edges
        );
        assert!(graph.edges.iter().all(|edge| {
            edge.confidence == Confidence::Heuristic
                && edge.provenance == SymbolProvenance::Heuristic
        }));
    }

    #[test]
    fn qualified_notification_constants_are_not_truncated() {
        let source = r#"
import AppKit

final class ViewObserver {
    func observe() {
        NotificationCenter.default.addObserver(forName: NSView.boundsDidChangeNotification, object: nil, queue: .main) { _ in }
    }
}
"#;
        let mut graph = SymbolGraph::new();
        enrich_source(
            &mut graph,
            "Sources/App/ViewObserver.swift",
            LanguageId::Swift,
            source,
        );

        assert!(
            edge_to(
                &graph,
                SymbolEdgeKind::NotificationObserve,
                "NSView.boundsDidChangeNotification"
            )
            .is_some(),
            "missing qualified notification observe: {:#?}",
            graph.edges
        );
    }

    fn edge_to(graph: &SymbolGraph, kind: SymbolEdgeKind, target_name: &str) -> Option<SymbolEdge> {
        let target = graph
            .symbols
            .iter()
            .find(|node| node.name == target_name)
            .map(|node| node.id.clone())?;
        graph
            .edges
            .iter()
            .find(|edge| edge.kind == kind && edge.to == target)
            .cloned()
    }
}
