//! Query API for fast lookups against the cached snapshot.
//!
//! Provides interactive queries without re-scanning:
//! - `who-imports <file>` - Find all files that import a given file
//! - `where-symbol <symbol>` - Find where a symbol is defined
//! - `component-of <file>` - Show what component/module a file belongs to
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::snapshot::Snapshot;

// ============================================================================
// Constants
// ============================================================================

/// Maximum depth for BFS traversal of re-export chains.
/// Prevents infinite loops in pathological cases (circular re-exports).
const MAX_REEXPORT_DEPTH: usize = 50;

/// File extensions we recognize for index file detection
const INDEX_EXTENSIONS: [&str; 5] = ["ts", "tsx", "js", "astro", "svelte"];

static RE_SWIFT_TYPE_IDENT: Lazy<Regex> =
    Lazy::new(|| match Regex::new(r"\b([A-Z][A-Za-z0-9_]*)\b") {
        Ok(re) => re,
        Err(err) => panic!("valid Swift type regex: {err}"),
    });
static RE_SWIFT_CAST: Lazy<Regex> = Lazy::new(|| {
    match Regex::new(r"\b(?:as|is)\b\s*[!?]?\s*([A-Z][A-Za-z0-9_]*(?:\s*<[^>]+>)?)") {
        Ok(re) => re,
        Err(err) => panic!("valid Swift cast regex: {err}"),
    }
});

// ============================================================================
// Helper Functions
// ============================================================================

/// Generate index file variants for a directory path.
/// `foo/bar` → `["foo/bar/index.ts", "foo/bar/index.tsx", "foo/bar/index.js"]`
fn index_variants(path: &str) -> Vec<String> {
    INDEX_EXTENSIONS
        .iter()
        .map(|ext| format!("{}/index.{}", path, ext))
        .collect()
}

/// Strip index file suffix from a path if present.
/// `foo/bar/index.ts` → `Some("foo/bar")`
/// `foo/bar/utils.ts` → `None`
fn strip_index_suffix(path: &str) -> Option<&str> {
    for ext in INDEX_EXTENSIONS {
        let suffix = format!("/index.{}", ext);
        if let Some(stripped) = path.strip_suffix(&suffix) {
            return Some(stripped);
        }
    }
    None
}

/// Check if a path looks like a file (has known extension)
fn has_file_extension(path: &str) -> bool {
    path.ends_with(".ts")
        || path.ends_with(".tsx")
        || path.ends_with(".js")
        || path.ends_with(".jsx")
        || path.ends_with(".rs")
        || path.ends_with(".py")
        || path.ends_with(".astro")
        || path.ends_with(".svelte")
}

fn component_path_candidates(snapshot: &Snapshot, symbol: &str) -> Vec<String> {
    let wanted = symbol.trim();
    if wanted.is_empty() {
        return Vec::new();
    }

    snapshot
        .files
        .iter()
        .filter_map(|file| {
            let stem = file
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&file.path)
                .rsplit_once('.')
                .map(|(stem, _)| stem)
                .unwrap_or(&file.path);
            (stem == wanted).then(|| file.path.clone())
        })
        .collect()
}

/// Normalize path for comparison (handles relative vs absolute, trailing slashes)
fn normalize_path(path: &str) -> String {
    path.trim_start_matches("./")
        .trim_end_matches('/')
        .to_string()
}

/// Check if two paths match, considering:
/// - Exact match
/// - Suffix match (edge.to ends with /target)
/// - Folder match (target is index file, edge.to is folder)
///
/// STRICTER than before: avoids `utils.ts` matching `other-utils.ts`
fn paths_match(edge_to: &str, target: &str) -> bool {
    let edge_norm = normalize_path(edge_to);
    let target_norm = normalize_path(target);

    // Exact match
    if edge_norm == target_norm {
        return true;
    }

    // Suffix match: edge.to ends with /target (full path segment)
    if edge_norm.ends_with(&format!("/{}", target_norm)) {
        return true;
    }

    // Folder match: target is index file, edge.to points to folder
    // e.g., target = "foo/index.ts", edge.to = "foo"
    if let Some(folder) = strip_index_suffix(&target_norm)
        && (edge_norm == folder || edge_norm.ends_with(&format!("/{}", folder)))
    {
        return true;
    }

    false
}

/// Result of a query operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    /// Query kind (who-imports, where-symbol, component-of)
    pub kind: String,
    /// Target that was queried (file path or symbol name)
    pub target: String,
    /// Matching results
    pub results: Vec<QueryMatch>,
}

/// A single query match
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryMatch {
    /// File path
    pub file: String,
    /// Line number (if applicable)
    pub line: Option<usize>,
    /// Additional context (e.g., import statement, symbol definition)
    pub context: Option<String>,
}

/// Resolution result for one Swift identifier seen in a type-position span.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwiftTypeReference {
    /// Referenced type name.
    pub name: String,
    /// First line where this type-position reference was observed.
    pub line: usize,
    /// Source snippet that produced the reference.
    pub context: String,
    /// Module-wide resolution status.
    pub status: SwiftTypeResolutionStatus,
    /// Definition location when the type resolves inside the indexed module.
    pub definition: Option<QueryMatch>,
    /// Existing unresolved sentinel id for unresolved candidates.
    pub symbol_id: Option<String>,
}

/// Swift type-position classification status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SwiftTypeResolutionStatus {
    Resolved,
    External,
    Unresolved,
}

/// Result payload for `loct query swift-types <file>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwiftTypeReferenceResult {
    pub kind: String,
    pub target: String,
    pub references: Vec<SwiftTypeReference>,
}

/// Query for files that import a given file or symbol (who-imports)
/// Follows re-export chains transitively to find all importers.
///
/// If the input looks like a symbol name (no path separators), it will first
/// resolve the symbol to file paths where it's defined, then find importers.
///
/// ## Algorithm
/// Uses BFS with depth limiting to traverse re-export chains:
/// `App.tsx → features/index.ts (reexport) → Component.tsx`
///
/// ## Path Matching
/// Uses `paths_match()` for strict comparison - avoids false positives
/// like `utils.ts` matching `other-utils.ts`.
pub fn query_who_imports(snapshot: &Snapshot, target: &str) -> QueryResult {
    use std::collections::HashSet;

    let mut results = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();

    // Determine if target is a symbol name or file path
    let is_symbol = !target.contains('/') && !has_file_extension(target);

    // Collect starting files to check
    let mut to_check: Vec<String> = if is_symbol {
        // Resolve symbol to file paths first
        let symbol_query = query_where_symbol(snapshot, target);
        let mut files: Vec<String> = symbol_query.results.into_iter().map(|m| m.file).collect();
        if files.is_empty() {
            files = component_path_candidates(snapshot, target);
        }
        if files.is_empty() {
            return QueryResult {
                kind: "who-imports".to_string(),
                target: target.to_string(),
                results: vec![],
            };
        }
        files
    } else {
        vec![normalize_path(target)]
    };

    // For each initial file, also check folder variant (strip index suffix)
    let initial_files: Vec<String> = to_check.clone();
    for file in &initial_files {
        if let Some(folder) = strip_index_suffix(file) {
            to_check.push(folder.to_string());
        }
    }

    // BFS with depth limiting
    let mut depth = 0;
    while let Some(current) = to_check.pop() {
        // Safety: prevent infinite loops in pathological cases
        if depth > MAX_REEXPORT_DEPTH {
            break;
        }

        if visited.contains(&current) {
            continue;
        }
        visited.insert(current.clone());
        depth += 1;

        // If this looks like a folder, also check index file variants
        if !has_file_extension(&current) {
            for variant in index_variants(&current) {
                if !visited.contains(&variant) {
                    to_check.push(variant);
                }
            }
        }

        // Find edges pointing to current target
        for edge in &snapshot.edges {
            if paths_match(&edge.to, &current) {
                if edge.label == "reexport" {
                    // Follow re-export chain
                    if !visited.contains(&edge.from) {
                        to_check.push(edge.from.clone());
                    }
                } else {
                    // Regular import - this is an actual consumer
                    results.push(QueryMatch {
                        file: edge.from.clone(),
                        line: None,
                        context: Some(format!("imports via {}", edge.label)),
                    });
                }
            }
        }
    }

    // Deduplicate and sort results
    results.sort_by(|a, b| a.file.cmp(&b.file));
    results.dedup_by(|a, b| a.file == b.file);

    QueryResult {
        kind: "who-imports".to_string(),
        target: target.to_string(),
        results,
    }
}

/// Query for where a symbol is defined (where-symbol).
///
/// This is an exact resolver. Fuzzy suggestions belong to `find`, not to
/// source-location commands that downstream tools use as anchors.
pub fn query_where_symbol(snapshot: &Snapshot, symbol: &str) -> QueryResult {
    let mut results = Vec::new();
    let symbol = symbol.trim();
    let (qualified_type, method_name) = parse_rust_method_query(symbol);

    for file in &snapshot.files {
        if qualified_type.is_none() {
            for exp in &file.exports {
                if exp.name == symbol {
                    results.push(QueryMatch {
                        file: file.path.clone(),
                        line: exp.line,
                        context: Some(format!("export {} {}", exp.kind, exp.name)),
                    });
                }
            }

            for local in &file.local_symbols {
                if local.name == symbol {
                    let context = if local.context.is_empty() {
                        format!("local {} {}", local.kind, local.name)
                    } else {
                        local.context.clone()
                    };
                    results.push(QueryMatch {
                        file: file.path.clone(),
                        line: local.line,
                        context: Some(context),
                    });
                }
            }
        }

        for method in &file.impl_methods {
            let method_matches = method.name == method_name
                && qualified_type.is_none_or(|ty| method.qualifier == ty);
            if method_matches {
                let context = if let Some(trait_qualifier) = &method.trait_qualifier {
                    format!(
                        "impl method {}::{} (trait {})",
                        method.qualifier, method.name, trait_qualifier
                    )
                } else {
                    format!("impl method {}::{}", method.qualifier, method.name)
                };
                results.push(QueryMatch {
                    file: file.path.clone(),
                    line: method.line,
                    context: Some(context),
                });
            }
        }
    }

    // Symbol-graph definitions (C-family tree-sitter extraction, Wave B).
    // Sites already matched via exports/local_symbols are skipped so the two
    // surfaces do not produce duplicate rows for the same definition.
    if qualified_type.is_none()
        && let Some(graph) = &snapshot.symbol_graph
    {
        for node in graph.lookup(symbol) {
            let Some(file) = node.file.as_ref().map(|p| p.display().to_string()) else {
                continue;
            };
            let line = node.range.map(|r| r.start_line);
            if results.iter().any(|m| m.file == file && m.line == line) {
                continue;
            }
            let context = node
                .signature
                .clone()
                .unwrap_or_else(|| format!("symbol {}", node.name));
            results.push(QueryMatch {
                file,
                line,
                context: Some(context),
            });
        }
    }

    results.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.context.cmp(&b.context))
    });
    results.dedup_by(|a, b| a.file == b.file && a.line == b.line && a.context == b.context);

    QueryResult {
        kind: "where-symbol".to_string(),
        target: symbol.to_string(),
        results,
    }
}

/// Classify Swift type-position references against the module-wide symbol graph.
///
/// This is a deliberately bounded first-cut heuristic for single-file LSP
/// false-positive triage. It only inspects spans that look like Swift type
/// positions: annotation clauses after `:`, inheritance/conformance lists,
/// generic argument lists, `->` return types, and `as`/`is` casts. It is not a
/// parser and can miss multiline or syntactically exotic types; later cuts can
/// replace the extractor while keeping this result shape.
pub fn classify_swift_type_references(
    snapshot: &Snapshot,
    target: &str,
    source: &str,
) -> SwiftTypeReferenceResult {
    let mut references = extract_swift_type_references(source);

    for reference in &mut references {
        if is_swift_external_type(&reference.name) {
            reference.status = SwiftTypeResolutionStatus::External;
            continue;
        }

        let result = query_where_symbol(snapshot, &reference.name);
        if let Some(definition) = result.results.into_iter().find(|m| {
            m.context
                .as_deref()
                .is_some_and(|ctx| is_type_definition_context(ctx, &reference.name))
        }) {
            reference.status = SwiftTypeResolutionStatus::Resolved;
            reference.definition = Some(definition);
        } else {
            reference.status = SwiftTypeResolutionStatus::Unresolved;
            reference.symbol_id =
                Some(crate::symbols::resolve::unresolved_id(&reference.name).to_string());
        }
    }

    SwiftTypeReferenceResult {
        kind: "swift-types".to_string(),
        target: target.to_string(),
        references,
    }
}

fn extract_swift_type_references(source: &str) -> Vec<SwiftTypeReference> {
    use std::collections::HashSet;

    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for (idx, raw_line) in source.lines().enumerate() {
        let line = strip_swift_line_comment(raw_line);
        let context = line.trim();
        let stringless = strip_swift_string_literals(line);
        let trimmed = stringless.trim();
        if trimmed.is_empty() || context.starts_with("import ") {
            continue;
        }
        let line_no = idx + 1;

        for segment in swift_type_segments(trimmed) {
            for name in type_names_from_segment(&segment) {
                if seen.insert(name.clone()) {
                    out.push(SwiftTypeReference {
                        name,
                        line: line_no,
                        context: context.to_string(),
                        status: SwiftTypeResolutionStatus::Unresolved,
                        definition: None,
                        symbol_id: None,
                    });
                }
            }
        }
    }

    out.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.name.cmp(&b.name)));
    out
}

fn swift_type_segments(line: &str) -> Vec<String> {
    let mut segments = Vec::new();

    for part in line.split(':').skip(1) {
        let segment = truncate_type_segment(part);
        if !segment.trim().is_empty() {
            segments.push(segment);
        }
    }

    for part in line.split("->").skip(1) {
        let segment = truncate_type_segment(part);
        if !segment.trim().is_empty() {
            segments.push(segment);
        }
    }

    let mut rest = line;
    while let Some(start) = rest.find('<') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('>') else {
            break;
        };
        let segment = after_start[..end].to_string();
        if !segment.trim().is_empty() {
            segments.push(segment);
        }
        rest = &after_start[end + 1..];
    }

    for caps in RE_SWIFT_CAST.captures_iter(line) {
        if let Some(m) = caps.get(1) {
            segments.push(m.as_str().to_string());
        }
    }

    segments
}

fn truncate_type_segment(segment: &str) -> String {
    let stop = segment
        .char_indices()
        .find_map(|(idx, ch)| matches!(ch, '=' | '{' | ')' | ';').then_some(idx))
        .unwrap_or(segment.len());
    segment[..stop]
        .split(" where ")
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

fn type_names_from_segment(segment: &str) -> Vec<String> {
    let mut names = Vec::new();
    for caps in RE_SWIFT_TYPE_IDENT.captures_iter(segment) {
        let Some(m) = caps.get(1) else { continue };
        if segment[..m.start()].ends_with('.') {
            continue;
        }
        let name = m.as_str();
        if name == "Self" {
            continue;
        }
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

fn strip_swift_line_comment(line: &str) -> &str {
    let mut in_str = false;
    let bytes = line.as_bytes();
    let mut idx = 0;
    while idx + 1 < bytes.len() {
        let ch = bytes[idx] as char;
        match ch {
            '\\' => {
                idx += 2;
                continue;
            }
            '"' => in_str = !in_str,
            '/' if !in_str && bytes[idx + 1] == b'/' => return &line[..idx],
            _ => {}
        }
        idx += 1;
    }
    line
}

fn strip_swift_string_literals(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_str = false;
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if in_str => {
                out.push(' ');
                if chars.next().is_some() {
                    out.push(' ');
                }
            }
            '"' => {
                in_str = !in_str;
                out.push(' ');
            }
            _ if in_str => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

fn is_type_definition_context(context: &str, name: &str) -> bool {
    let normalized = context.trim();
    let export_prefixes = [
        "export class ",
        "export struct ",
        "export enum ",
        "export protocol ",
        "export typealias ",
    ];
    if export_prefixes
        .iter()
        .any(|prefix| normalized == format!("{prefix}{name}"))
    {
        return true;
    }

    let declaration_needles = [
        format!("class {name}"),
        format!("struct {name}"),
        format!("enum {name}"),
        format!("protocol {name}"),
        format!("typealias {name}"),
    ];
    declaration_needles
        .iter()
        .any(|needle| normalized.contains(needle))
}

fn is_swift_external_type(name: &str) -> bool {
    // Curated noise guard for build-free Swift triage. Keep this small and
    // boring: stdlib scalar/collection/protocol names plus common Foundation,
    // SwiftUI, Combine, and Dispatch types that single-file SourceKit often
    // sees without module context.
    matches!(
        name,
        "Any"
            | "AnyObject"
            | "Never"
            | "String"
            | "Substring"
            | "Character"
            | "Bool"
            | "Int"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Double"
            | "Float"
            | "Array"
            | "Dictionary"
            | "Set"
            | "Optional"
            | "Result"
            | "Void"
            | "Task"
            | "Error"
            | "Codable"
            | "Encodable"
            | "Decodable"
            | "Hashable"
            | "Equatable"
            | "Comparable"
            | "Identifiable"
            | "Sendable"
            | "URL"
            | "Data"
            | "Date"
            | "UUID"
            | "Decimal"
            | "NSError"
            | "NSObject"
            | "Notification"
            | "NotificationCenter"
            | "Bundle"
            | "FileManager"
            | "UserDefaults"
            | "IndexPath"
            | "CGFloat"
            | "CGPoint"
            | "CGSize"
            | "CGRect"
            | "DispatchQueue"
            | "MainActor"
            | "View"
            | "Text"
            | "Color"
            | "Image"
            | "Button"
            | "VStack"
            | "HStack"
            | "ZStack"
            | "List"
            | "ForEach"
            | "Binding"
            | "State"
            | "StateObject"
            | "ObservedObject"
            | "EnvironmentObject"
            | "Published"
            | "ObservableObject"
    )
}

fn parse_rust_method_query(symbol: &str) -> (Option<&str>, &str) {
    if let Some((qualifier, method)) = symbol.rsplit_once("::")
        && !qualifier.trim().is_empty()
        && !method.trim().is_empty()
    {
        return (Some(qualifier.trim()), method.trim());
    }
    (None, symbol)
}

/// Query for what component a file belongs to (component-of)
pub fn query_component_of(snapshot: &Snapshot, file: &str) -> QueryResult {
    let mut results = Vec::new();

    // Look for barrel files (index.ts) that re-export this file
    for barrel in &snapshot.barrels {
        if barrel
            .targets
            .iter()
            .any(|t| t == file || t.ends_with(file))
        {
            results.push(QueryMatch {
                file: barrel.path.clone(),
                line: None,
                context: Some(format!("barrel with {} re-exports", barrel.reexport_count)),
            });
        }
    }

    // Also check edges to find parent directories
    for edge in &snapshot.edges {
        if edge.to == file || edge.to.ends_with(file) {
            // Parent module that imports this file
            results.push(QueryMatch {
                file: edge.from.clone(),
                line: None,
                context: Some("parent module".to_string()),
            });
        }
    }

    QueryResult {
        kind: "component-of".to_string(),
        target: file.to_string(),
        results,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FileAnalysis;

    fn mock_snapshot() -> Snapshot {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);

        // Add some test files
        let mut file1 = FileAnalysis::new("src/utils.ts".into());
        file1.exports.push(crate::types::ExportSymbol {
            name: "helper".to_string(),
            kind: "function".to_string(),
            export_type: "named".to_string(),
            line: Some(10),
            params: Vec::new(),
            symbol_id: crate::types::SymbolIdV1::default(),
        });

        let mut file2 = FileAnalysis::new("src/app.ts".into());
        file2.exports.push(crate::types::ExportSymbol {
            name: "PostAuthBootstrapOverlay".to_string(),
            kind: "class".to_string(),
            export_type: "named".to_string(),
            line: Some(42),
            params: Vec::new(),
            symbol_id: crate::types::SymbolIdV1::default(),
        });

        snapshot.files.push(file1);
        snapshot.files.push(file2);

        // Add an edge (app.ts imports utils.ts)
        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "src/app.ts".to_string(),
            to: "src/utils.ts".to_string(),
            label: "import".to_string(),
        });

        snapshot
    }

    #[test]
    fn test_query_who_imports() {
        let snapshot = mock_snapshot();
        let result = query_who_imports(&snapshot, "src/utils.ts");

        assert_eq!(result.kind, "who-imports");
        assert_eq!(result.target, "src/utils.ts");
        assert!(!result.results.is_empty());
    }

    #[test]
    fn test_query_where_symbol() {
        let snapshot = mock_snapshot();
        let result = query_where_symbol(&snapshot, "helper");

        assert_eq!(result.kind, "where-symbol");
        assert_eq!(result.target, "helper");
    }

    #[test]
    fn test_query_where_symbol_is_exact_not_substring() {
        let snapshot = mock_snapshot();
        let result = query_where_symbol(&snapshot, "bootstrap");

        assert_eq!(result.kind, "where-symbol");
        assert_eq!(result.target, "bootstrap");
        assert!(
            result.results.is_empty(),
            "where-symbol should not fuzzy-match exports"
        );
    }

    #[test]
    fn test_query_where_symbol_resolves_impl_methods() {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);
        let mut file = FileAnalysis::new("src/recorder.rs".into());
        file.impl_methods.push(crate::types::ImplMethod {
            name: "start".to_string(),
            qualifier: "Recorder".to_string(),
            line: Some(12),
            visibility: crate::types::Visibility::Public,
            ..Default::default()
        });
        snapshot.files.push(file);

        let qualified = query_where_symbol(&snapshot, "Recorder::start");
        assert_eq!(qualified.results.len(), 1);
        assert_eq!(qualified.results[0].file, "src/recorder.rs");
        assert_eq!(qualified.results[0].line, Some(12));
        assert_eq!(
            qualified.results[0].context.as_deref(),
            Some("impl method Recorder::start")
        );

        let bare = query_where_symbol(&snapshot, "start");
        assert_eq!(bare.results.len(), 1);
        assert_eq!(bare.results[0].file, "src/recorder.rs");
    }

    #[test]
    fn test_swift_type_reference_classification_resolves_unresolved_and_external() {
        let mut snapshot = Snapshot::new(vec!["Pensieve".to_string()]);

        let mut app_state =
            FileAnalysis::new("Pensieve/Sources/Pensieve/App/AppState.swift".into());
        app_state.exports.push(crate::types::ExportSymbol {
            name: "AppState".to_string(),
            kind: "class".to_string(),
            export_type: "named".to_string(),
            line: Some(58),
            params: Vec::new(),
            symbol_id: crate::types::SymbolIdV1::default(),
        });
        app_state.exports.push(crate::types::ExportSymbol {
            name: "DocumentRef".to_string(),
            kind: "struct".to_string(),
            export_type: "named".to_string(),
            line: Some(334),
            params: Vec::new(),
            symbol_id: crate::types::SymbolIdV1::default(),
        });
        snapshot.files.push(app_state);

        let source = include_str!("../tests/fixtures/swift_type_refs/TypeReferenceProbe.swift");

        let result = classify_swift_type_references(
            &snapshot,
            "Pensieve/Sources/Pensieve/App/AppController.swift",
            source,
        );

        let app_state = result
            .references
            .iter()
            .find(|r| r.name == "AppState")
            .expect("AppState should be classified");
        assert!(matches!(
            app_state.status,
            SwiftTypeResolutionStatus::Resolved
        ));
        assert_eq!(
            app_state.definition.as_ref().map(|m| m.file.as_str()),
            Some("Pensieve/Sources/Pensieve/App/AppState.swift")
        );
        assert_eq!(app_state.definition.as_ref().and_then(|m| m.line), Some(58));

        let document_ref = result
            .references
            .iter()
            .find(|r| r.name == "DocumentRef")
            .expect("DocumentRef should be classified");
        assert!(matches!(
            document_ref.status,
            SwiftTypeResolutionStatus::Resolved
        ));
        assert_eq!(
            document_ref.definition.as_ref().and_then(|m| m.line),
            Some(334)
        );

        let missing = result
            .references
            .iter()
            .find(|r| r.name == "TotallyMadeUpType")
            .expect("missing type should be classified");
        assert!(matches!(
            missing.status,
            SwiftTypeResolutionStatus::Unresolved
        ));
        assert_eq!(
            missing.symbol_id.as_deref(),
            Some("unresolved::TotallyMadeUpType")
        );

        for external in ["String", "URL"] {
            let reference = result
                .references
                .iter()
                .find(|r| r.name == external)
                .expect("allowlisted external type should be classified");
            assert!(
                matches!(reference.status, SwiftTypeResolutionStatus::External),
                "{external} should be external, not unresolved"
            );
        }
    }

    #[test]
    fn test_query_component_of() {
        let snapshot = mock_snapshot();
        let result = query_component_of(&snapshot, "src/utils.ts");

        assert_eq!(result.kind, "component-of");
        assert_eq!(result.target, "src/utils.ts");
    }

    #[test]
    fn test_query_who_imports_follows_reexport_chain() {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);

        // Setup: App.tsx → index.ts (import) → Component.tsx (reexport)
        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "src/App.tsx".to_string(),
            to: "src/features/index.ts".to_string(),
            label: "import".to_string(),
        });
        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "src/features/index.ts".to_string(),
            to: "src/features/Component.tsx".to_string(),
            label: "reexport".to_string(),
        });

        // Query who imports Component.tsx - should find App.tsx through the chain
        let result = query_who_imports(&snapshot, "src/features/Component.tsx");

        assert_eq!(result.kind, "who-imports");
        assert!(
            !result.results.is_empty(),
            "Should find App.tsx as importer"
        );
        assert!(
            result.results.iter().any(|r| r.file == "src/App.tsx"),
            "App.tsx should be in results"
        );
    }

    #[test]
    fn test_query_who_imports_resolves_component_file_basename() {
        let mut snapshot = Snapshot::new(vec!["site".to_string()]);

        snapshot.files.push(FileAnalysis::new(
            "site/src/components/HeroSectionV2.svelte".into(),
        ));
        snapshot
            .files
            .push(FileAnalysis::new("site/src/pages/index.astro".into()));
        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "site/src/pages/index.astro".to_string(),
            to: "site/src/components/HeroSectionV2.svelte".to_string(),
            label: "import".to_string(),
        });

        let result = query_who_imports(&snapshot, "HeroSectionV2");

        assert!(
            result
                .results
                .iter()
                .any(|r| r.file == "site/src/pages/index.astro"),
            "component filename stem should resolve to importers when no export symbol exists"
        );
    }

    #[test]
    fn test_query_who_imports_multi_level_reexport() {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);

        // Setup: App.tsx → ai-suite/index.ts → system/index.ts → AISystemHost.tsx
        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "src/App.tsx".to_string(),
            to: "src/features/ai-suite/index.ts".to_string(),
            label: "import".to_string(),
        });
        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "src/features/ai-suite/index.ts".to_string(),
            to: "src/features/ai-suite/system".to_string(),
            label: "reexport".to_string(),
        });
        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "src/features/ai-suite/system/index.ts".to_string(),
            to: "src/features/ai-suite/system/AISystemHost.tsx".to_string(),
            label: "reexport".to_string(),
        });

        // Query who imports AISystemHost.tsx - should find App.tsx through the 3-level chain
        let result = query_who_imports(&snapshot, "src/features/ai-suite/system/AISystemHost.tsx");

        assert!(
            !result.results.is_empty(),
            "Should find importers through re-export chain"
        );
    }

    // ========================================
    // Path matching tests (stricter matching)
    // ========================================

    #[test]
    fn test_paths_match_exact() {
        assert!(paths_match("src/utils.ts", "src/utils.ts"));
        assert!(paths_match("./src/utils.ts", "src/utils.ts"));
        assert!(paths_match("src/utils.ts", "./src/utils.ts"));
    }

    #[test]
    fn test_paths_match_suffix() {
        assert!(paths_match("src/components/utils.ts", "utils.ts"));
        assert!(paths_match("src/deep/nested/file.ts", "file.ts"));
    }

    #[test]
    fn test_paths_match_no_false_positives() {
        // CRITICAL: utils.ts should NOT match other-utils.ts
        assert!(!paths_match("src/other-utils.ts", "utils.ts"));
        assert!(!paths_match("src/my-utils.ts", "utils.ts"));
        assert!(!paths_match("src/utils-helper.ts", "utils.ts"));
    }

    #[test]
    fn test_paths_match_folder_to_index() {
        // foo/index.ts should match foo
        assert!(paths_match("src/components", "src/components/index.ts"));
        assert!(paths_match("features", "features/index.tsx"));
    }

    #[test]
    fn test_index_variants() {
        let variants = index_variants("src/components");
        assert_eq!(variants.len(), 5);
        assert!(variants.contains(&"src/components/index.ts".to_string()));
        assert!(variants.contains(&"src/components/index.tsx".to_string()));
        assert!(variants.contains(&"src/components/index.js".to_string()));
        assert!(variants.contains(&"src/components/index.astro".to_string()));
        assert!(variants.contains(&"src/components/index.svelte".to_string()));
    }

    #[test]
    fn test_strip_index_suffix() {
        assert_eq!(strip_index_suffix("foo/bar/index.ts"), Some("foo/bar"));
        assert_eq!(strip_index_suffix("foo/bar/index.tsx"), Some("foo/bar"));
        assert_eq!(strip_index_suffix("foo/bar/index.js"), Some("foo/bar"));
        assert_eq!(strip_index_suffix("foo/bar/utils.ts"), None);
        assert_eq!(strip_index_suffix("foo/bar"), None);
    }

    #[test]
    fn test_has_file_extension() {
        assert!(has_file_extension("foo.ts"));
        assert!(has_file_extension("bar.tsx"));
        assert!(has_file_extension("baz.rs"));
        assert!(has_file_extension("qux.py"));
        assert!(!has_file_extension("foo"));
        assert!(!has_file_extension("foo/bar"));
    }

    #[test]
    fn test_query_who_imports_stricter_matching() {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);

        // Setup: app.ts imports utils.ts, NOT other-utils.ts
        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "src/app.ts".to_string(),
            to: "src/utils.ts".to_string(),
            label: "import".to_string(),
        });
        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "src/other.ts".to_string(),
            to: "src/other-utils.ts".to_string(),
            label: "import".to_string(),
        });

        // Query who imports utils.ts - should find app.ts but NOT other.ts
        let result = query_who_imports(&snapshot, "src/utils.ts");

        assert!(
            result.results.iter().any(|r| r.file == "src/app.ts"),
            "Should find app.ts as importer of utils.ts"
        );
        assert!(
            !result.results.iter().any(|r| r.file == "src/other.ts"),
            "Should NOT find other.ts (imports other-utils.ts, not utils.ts)"
        );
    }
}
