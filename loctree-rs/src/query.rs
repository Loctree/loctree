//! Query API for fast lookups against the cached snapshot.
//!
//! Provides interactive queries without re-scanning:
//! - `who-imports <file>` - Find all files that import a given file
//! - `where-symbol <symbol>` - Find where a symbol is defined
//! - `component-of <file>` - Show what component/module a file belongs to
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::analyzer::occurrences::{FileScope, IndexedUniverse};
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

/// `who-imports` edge match, disambiguated against the snapshot's real files.
///
/// `paths_match()`'s suffix branch treats any `edge.to` that ends with
/// `/{target}` as the same file as `target` -- correct when `target` is a
/// bare/partial name (`utils.ts`) that has exactly one real owner, but wrong
/// when `target` is itself the exact path of a real, distinct file. A
/// vendored copy at `lua-api-crates/mux/src/pane.rs` legitimately ends with
/// `/mux/src/pane.rs`, but that suffix hit is a name collision with someone
/// else's ancestors, not a relative/absolute spelling of the genuine
/// `mux/src/pane.rs` (G10.1, loctree-feedback.md). Once `target` resolves to one
/// concrete file in the snapshot, only an exact match to THAT file counts;
/// the folder-to-index heuristic stays available regardless, since it names
/// a resolution formality (folder import -> its index file), never a
/// collision between two genuine files.
fn edge_targets_query(snapshot: &Snapshot, edge_to: &str, target: &str) -> bool {
    let edge_norm = normalize_path(edge_to);
    let target_norm = normalize_path(target);

    if edge_norm == target_norm {
        return true;
    }

    // Folder match: target is index file, edge.to points to folder.
    if let Some(folder) = strip_index_suffix(&target_norm)
        && (edge_norm == folder || edge_norm.ends_with(&format!("/{}", folder)))
    {
        return true;
    }

    // The suffix heuristic is only trustworthy when `target` does not
    // itself name a real, distinct file already present in the snapshot.
    let target_is_real_file = snapshot
        .files
        .iter()
        .any(|f| normalize_path(&f.path) == target_norm);
    if target_is_real_file {
        return false;
    }

    edge_norm.ends_with(&format!("/{}", target_norm))
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

    /// Number of matches before public-output pagination.
    #[serde(default)]
    pub total: usize,

    /// Number of rows actually emitted in `results`.
    #[serde(default)]
    pub emitted: usize,

    /// Zero-based offset of the emitted window. Structural queries currently
    /// emit from zero; the field is explicit so consumers never infer it.
    #[serde(default)]
    pub offset: usize,

    /// Requested public limit, when one was applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,

    /// True when another row exists after the emitted window.
    #[serde(default)]
    pub has_more: bool,

    /// True when the public result list is a bounded prefix.
    #[serde(default)]
    pub truncated: bool,

    /// Indexed file universe used to answer this query.
    #[serde(default)]
    pub universe: IndexedUniverse,
}

impl QueryResult {
    fn complete(kind: &str, target: &str, results: Vec<QueryMatch>, snapshot: &Snapshot) -> Self {
        let total = results.len();
        Self {
            kind: kind.to_string(),
            target: target.to_string(),
            results,
            total,
            emitted: total,
            offset: 0,
            limit: None,
            has_more: false,
            truncated: false,
            universe: IndexedUniverse::from_snapshot(
                snapshot,
                FileScope::default(),
                snapshot.files.len(),
            ),
        }
    }

    /// Bound a public query response without weakening internal resolvers.
    pub fn bounded(mut self, limit: Option<usize>) -> Self {
        self.total = self.results.len();
        if let Some(limit) = limit {
            self.results.truncate(limit);
            self.limit = Some(limit);
        }
        self.emitted = self.results.len();
        self.has_more = self.emitted < self.total;
        self.truncated = self.has_more;
        self
    }
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
/// Verify an `implicit_symbol` edge against real symbol usages.
///
/// `implicit_symbol` edges connect Swift-style module members by bare-name
/// heuristics. Before `who-imports` reports `importer` as a consumer of
/// `target_file`, require an actual recorded usage:
/// - symbol query (`symbol = Some(name)`): the importer must use that exact
///   name; the match carries the first usage line.
/// - file query (`symbol = None`): the importer must use one of the target
///   file's implicit-eligible exported type names.
///
/// Returns `None` when no real reference exists — the edge is then treated
/// as noise and dropped from the result, which also sanitizes results built
/// from stale snapshots that still carry pre-guard implicit edges.
fn implicit_reference_match(
    snapshot: &Snapshot,
    importer: &str,
    target_file: &str,
    symbol: Option<&str>,
) -> Option<QueryMatch> {
    use crate::analyzer::root_scan::is_implicit_symbol_export;

    let importer_analysis = snapshot
        .files
        .iter()
        .find(|f| paths_match(&f.path, importer))?;

    let (name, line) = match symbol {
        Some(name) => {
            let usage = importer_analysis
                .symbol_usages
                .iter()
                .find(|u| u.name == name)?;
            (name.to_string(), usage.line)
        }
        None => {
            let target = snapshot
                .files
                .iter()
                .find(|f| paths_match(&f.path, target_file))?;
            let eligible: std::collections::HashSet<&str> = target
                .exports
                .iter()
                .filter(|e| is_implicit_symbol_export(e))
                .map(|e| e.name.as_str())
                .collect();
            let usage = importer_analysis
                .symbol_usages
                .iter()
                .find(|u| eligible.contains(u.name.as_str()))?;
            (usage.name.clone(), usage.line)
        }
    };

    Some(QueryMatch {
        file: importer_analysis.path.clone(),
        line: Some(line),
        context: Some(format!("references {name} (implicit module scope)")),
    })
}

/// Outcome of checking a regular import edge against a symbol-scoped
/// `who-imports` query. Three-way rather than a plain bool/Option: absence of
/// a name match is only proof of exclusion when we actually HAVE import data
/// to check it against — a snapshot with no mirrored `ImportEntry` for the
/// edge (partial scan, or a fixture that only carries the graph edge) must
/// not be treated the same as one that positively names a different symbol.
enum SymbolImportEvidence {
    /// A concrete `ImportEntry` names the symbol (or imports it via a glob,
    /// which cannot prove absence of anything).
    Named { line: Option<usize>, label: String },
    /// The importer has `ImportEntry`s resolving to `target_file`, and NONE
    /// of them name the symbol or use a glob — positive proof this importer
    /// consumes a sibling export, not the queried one.
    Excluded,
    /// No `ImportEntry` resolving to `target_file` was found for this
    /// importer (missing file, or no mirrored import data) — cannot prove
    /// absence, so the caller keeps the edge counted the way file-mode
    /// queries always have.
    Unknown,
}

/// Verify a regular import edge against a symbol-scoped `who-imports` query.
///
/// A regular (non-reexport, non-implicit) edge only proves "importer
/// resolves to this file" — reaching the file that defines the queried
/// symbol otherwise credited EVERY importer of that file as a consumer of
/// THAT symbol, even when its actual `use` statement named an unrelated
/// sibling export from the same module (atuin: `who-imports
/// effective_data_dir` returned 92 files while 2 actually used it —
/// loctree-feedback.md G2). Mirrors `implicit_reference_match`'s symbol-precise
/// discipline, checked against `ImportEntry.symbols` instead of usage sites.
fn symbol_import_evidence(
    snapshot: &Snapshot,
    edge: &crate::snapshot::GraphEdge,
    target_file: &str,
    symbol: &str,
) -> SymbolImportEvidence {
    let Some(importer_analysis) = snapshot
        .files
        .iter()
        .find(|f| paths_match(&f.path, &edge.from))
    else {
        return SymbolImportEvidence::Unknown;
    };

    // `Type::method` queries: a `use` statement only ever names the TYPE
    // (`use foo::Recorder;`), never the method, so match against the
    // qualifier when one is present rather than the full qualified string.
    let (qualifier, _method) = parse_rust_method_query(symbol);
    let import_name = qualifier.unwrap_or(symbol);

    let mut saw_matching_entry = false;
    for entry in &importer_analysis.imports {
        let names_file = entry
            .resolved_path
            .as_deref()
            .is_some_and(|p| paths_match(p, target_file))
            || paths_match(&entry.source, target_file);
        if !names_file {
            continue;
        }
        saw_matching_entry = true;

        let is_glob = entry.symbols.iter().any(|s| s.name == "*");
        let names_symbol = entry
            .symbols
            .iter()
            .any(|s| s.name == import_name || s.alias.as_deref() == Some(import_name));

        if is_glob || names_symbol {
            return SymbolImportEvidence::Named {
                line: entry.line,
                label: edge.label.clone(),
            };
        }
    }

    if saw_matching_entry {
        SymbolImportEvidence::Excluded
    } else {
        SymbolImportEvidence::Unknown
    }
}

pub fn query_who_imports(snapshot: &Snapshot, target: &str) -> QueryResult {
    use std::collections::HashSet;

    let mut results = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();

    // Determine if target is a symbol name or file path
    let is_symbol = !target.contains('/') && !has_file_extension(target);
    // When the query names a symbol, implicit (module-scope) edges must be
    // verified against real usages of THAT symbol, not mere edge existence.
    let symbol_name: Option<&str> = is_symbol.then_some(target);

    // Collect starting files to check
    let mut to_check: Vec<String> = if is_symbol {
        // Resolve symbol to file paths first
        let symbol_query = query_where_symbol(snapshot, target);
        let mut files: Vec<String> = symbol_query.results.into_iter().map(|m| m.file).collect();
        if files.is_empty() {
            files = component_path_candidates(snapshot, target);
        }
        if files.is_empty() {
            return QueryResult::complete("who-imports", target, vec![], snapshot);
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
            if edge_targets_query(snapshot, &edge.to, &current) {
                if edge.label == "reexport" {
                    // Follow re-export chain
                    if !visited.contains(&edge.from) {
                        to_check.push(edge.from.clone());
                    }
                } else if edge.is_implicit_symbol() {
                    // Implicit module-scope edge (Swift-style): a heuristic
                    // guess, not an import. Only report it when the alleged
                    // importer actually references the symbol, and point at
                    // the real reference line. Blind reporting turned every
                    // file in a Swift module into an "importer"
                    // (loctree-feedback.md 2026-07-25, blinksh/blink).
                    if let Some(m) =
                        implicit_reference_match(snapshot, &edge.from, &current, symbol_name)
                    {
                        results.push(m);
                    }
                } else {
                    // Regular import - this proves "importer resolves to
                    // this file". A symbol-scoped query additionally
                    // requires the import to actually name the symbol
                    // (loctree-feedback.md G2): reaching the file that defines
                    // it is not proof every one of its importers consumes
                    // THIS symbol rather than a sibling export. Glob imports,
                    // and importers we have no mirrored ImportEntry data for,
                    // stay counted (can't prove absence either way);
                    // file-mode queries (symbol_name == None) are unaffected
                    // — they ask about the FILE, not a symbol inside it.
                    match symbol_name {
                        Some(sym) => {
                            match symbol_import_evidence(snapshot, edge, &current, sym) {
                                SymbolImportEvidence::Named { line, label } => {
                                    results.push(QueryMatch {
                                        file: edge.from.clone(),
                                        line,
                                        context: Some(format!("imports {sym} via {label}")),
                                    });
                                }
                                SymbolImportEvidence::Excluded => {
                                    // Positive proof: this importer names a
                                    // different, sibling symbol from the
                                    // same file, not the queried one.
                                }
                                SymbolImportEvidence::Unknown => {
                                    results.push(QueryMatch {
                                        file: edge.from.clone(),
                                        line: None,
                                        context: Some(format!("imports via {}", edge.label)),
                                    });
                                }
                            }
                        }
                        None => {
                            results.push(QueryMatch {
                                file: edge.from.clone(),
                                line: None,
                                context: Some(format!("imports via {}", edge.label)),
                            });
                        }
                    }
                }
            }
        }
    }

    // Deduplicate and sort results
    results.sort_by(|a, b| a.file.cmp(&b.file));
    results.dedup_by(|a, b| a.file == b.file);

    QueryResult::complete("who-imports", target, results, snapshot)
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
                    let context = rust_where_symbol_context(&file.path, &exp.kind, &exp.name)
                        .unwrap_or_else(|| format!("export {} {}", exp.kind, exp.name));
                    results.push(QueryMatch {
                        file: file.path.clone(),
                        line: exp.line,
                        context: Some(context),
                    });
                }
            }

            for local in &file.local_symbols {
                if local.name == symbol {
                    let context = rust_where_symbol_context(&file.path, &local.kind, &local.name)
                        .unwrap_or_else(|| {
                            if local.context.is_empty() {
                                format!("local {} {}", local.kind, local.name)
                            } else {
                                local.context.clone()
                            }
                        });
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
            // Tree-sitter ranges start at leading attributes (`@main` sits on
            // the line above `struct FixtureApp`), while the per-language
            // analyzers anchor the same declaration on its keyword line. A
            // node whose span contains an already-matched line in the same
            // file is the same physical declaration seen from a second
            // surface, not a second definition — emitting it would duplicate
            // the body downstream (LCT-F01).
            let same_declaration = results.iter().any(|m| {
                m.file == file
                    && (m.line == line
                        || node
                            .range
                            .zip(m.line)
                            .is_some_and(|(r, l)| l >= r.start_line && l <= r.end_line))
            });
            if same_declaration {
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

    // One physical definition may be indexed as an export, local symbol and
    // impl method. Public location truth is file+line, not the number of index
    // surfaces that happened to observe it. Prefer the richer qualified impl
    // context by sorting it first, then collapse the duplicate location.
    results.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| context_rank(&a.context).cmp(&context_rank(&b.context)))
            .then_with(|| a.context.cmp(&b.context))
    });
    results.dedup_by(|a, b| a.file == b.file && a.line == b.line);

    // A module name is a symbol too. `mod health_score;` is not an export, a
    // local symbol, an impl method or a graph node, so without this the query
    // answered "(no results)" for a name the snapshot fully knows — and `body`
    // inherited the dead end (its hint pointed here).
    for decl in module_declarations(snapshot, symbol) {
        let already_known = results
            .iter()
            .any(|m| m.file == decl.declared_in && m.line == decl.line);
        if already_known {
            continue;
        }
        results.push(QueryMatch {
            file: decl.declared_in.clone(),
            line: decl.line,
            context: Some(decl.context()),
        });
    }

    QueryResult::complete("where-symbol", symbol, results, snapshot)
}

// ============================================================================
// Module declarations
// ============================================================================

/// A `mod <name>;` declaration site.
///
/// In Rust a module name is a real symbol, and `mod health_score;` is where it
/// enters the namespace — but it is not an export, a local symbol, an impl
/// method or a symbol-graph node, so every definition surface used to answer
/// "no results" for it. The declaration is recorded on the declaring file as an
/// import entry flagged `is_mod_declaration`; this type lifts it back out as
/// the definition site it is, together with the file the module resolves to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleDeclaration {
    /// Module name as declared (`health_score`).
    pub module: String,
    /// File carrying the `mod <name>;` declaration.
    pub declared_in: String,
    /// 1-based line of the declaration, when the analyzer anchored one.
    pub line: Option<usize>,
    /// File the module resolves to, when the analyzer resolved it.
    pub module_file: Option<String>,
}

impl ModuleDeclaration {
    /// Context string used when this declaration is rendered as a
    /// `where-symbol` match.
    pub fn context(&self) -> String {
        match &self.module_file {
            Some(path) => format!("module declaration `mod {};` -> {}", self.module, path),
            None => format!("module declaration `mod {};`", self.module),
        }
    }
}

/// Every `mod <name>;` declaration of `name` across the snapshot.
///
/// Pure snapshot read: the mod declarations were already indexed at scan time
/// (as `is_mod_declaration` import entries), so this neither re-parses sources
/// nor guesses from file names.
pub fn module_declarations(snapshot: &Snapshot, name: &str) -> Vec<ModuleDeclaration> {
    let name = name.trim();
    if name.is_empty() {
        return Vec::new();
    }
    let mut declarations = Vec::new();
    for file in &snapshot.files {
        for import in &file.imports {
            if !import.is_mod_declaration {
                continue;
            }
            let declares = import.symbols.iter().any(|s| s.name == name);
            if !declares {
                continue;
            }
            declarations.push(ModuleDeclaration {
                module: name.to_string(),
                declared_in: file.path.clone(),
                line: import.line,
                module_file: import.resolved_path.clone(),
            });
        }
    }
    declarations.sort_by(|a, b| a.declared_in.cmp(&b.declared_in).then(a.line.cmp(&b.line)));
    declarations.dedup_by(|a, b| a.declared_in == b.declared_in && a.line == b.line);
    declarations
}

/// Language-native where-symbol labels for Rust.
///
/// Generic snapshot kinds are JS-shaped (`function`, `enum`). Rust `enum`
/// already had an honest label; `function` must print `fn {name}` so a
/// definition lookup greps the same token a human reads in source.
fn rust_where_symbol_context(path: &str, kind: &str, name: &str) -> Option<String> {
    if !path.ends_with(".rs") {
        return None;
    }
    match kind {
        "enum" => Some(format!("rust enum {name}")),
        "function" => Some(format!("fn {name}")),
        _ => None,
    }
}

fn context_rank(context: &Option<String>) -> u8 {
    match context.as_deref() {
        Some(value) if value.starts_with("impl method ") => 0,
        Some(value) if value.starts_with("local ") => 1,
        Some(value) if value.starts_with("export ") => 2,
        Some(_) => 3,
        None => 4,
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
        "rust enum ",
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

    QueryResult::complete("component-of", file, results, snapshot)
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

    /// Swift multi-file module regression (loctree-feedback.md 2026-07-25,
    /// blinksh/blink): `who-imports` must verify `implicit_symbol` edges
    /// against real symbol usages instead of reporting module membership.
    /// The snapshot deliberately carries the noisy pre-guard edges (as stale
    /// snapshots in the field still do).
    fn swift_module_snapshot() -> Snapshot {
        use crate::snapshot::{GraphEdge, IMPLICIT_SYMBOL_EDGE_LABEL};
        use crate::types::{ExportSymbol, SymbolUsage};

        let mut snapshot = Snapshot::new(vec!["Module".to_string()]);

        // Agent.swift: top-level class + nested `enum Error` flattened into
        // exports (the shape the Swift analyzer actually produces).
        let mut agent = FileAnalysis::new("Module/Agent.swift".into());
        agent.language = "swift".to_string();
        for (name, kind, line) in [("DefaultAgent", "class", 10), ("Error", "enum", 14)] {
            agent.exports.push(ExportSymbol::new(
                name.to_string(),
                kind,
                "named",
                Some(line),
            ));
        }

        // Consumer.swift: really references DefaultAgent.
        let mut consumer = FileAnalysis::new("Module/Consumer.swift".into());
        consumer.language = "swift".to_string();
        consumer.symbol_usages.push(SymbolUsage {
            name: "DefaultAgent".to_string(),
            line: 21,
            context: "let agent = DefaultAgent.instance".to_string(),
        });

        // Unrelated.swift: only uses the stdlib `Error` protocol; the stale
        // implicit edge to Agent.swift is a name-collision artifact.
        let mut unrelated = FileAnalysis::new("Module/Unrelated.swift".into());
        unrelated.language = "swift".to_string();
        unrelated.symbol_usages.push(SymbolUsage {
            name: "Error".to_string(),
            line: 5,
            context: "func run() throws -> Error".to_string(),
        });

        snapshot.files.push(agent);
        snapshot.files.push(consumer);
        snapshot.files.push(unrelated);

        for from in ["Module/Consumer.swift", "Module/Unrelated.swift"] {
            snapshot.edges.push(GraphEdge {
                from: from.to_string(),
                to: "Module/Agent.swift".to_string(),
                label: IMPLICIT_SYMBOL_EDGE_LABEL.to_string(),
            });
        }
        snapshot
    }

    #[test]
    fn who_imports_symbol_verifies_implicit_edges_against_real_usages() {
        let snapshot = swift_module_snapshot();
        let result = query_who_imports(&snapshot, "DefaultAgent");

        let files: Vec<&str> = result.results.iter().map(|m| m.file.as_str()).collect();
        assert_eq!(
            files,
            vec!["Module/Consumer.swift"],
            "only the file with a real DefaultAgent usage may be reported"
        );
        assert_eq!(
            result.results[0].line,
            Some(21),
            "implicit consumer must carry the real reference line"
        );
        assert_eq!(
            result.results[0].context.as_deref(),
            Some("references DefaultAgent (implicit module scope)")
        );
    }

    #[test]
    fn who_imports_file_filters_implicit_edges_to_eligible_type_usages() {
        let snapshot = swift_module_snapshot();
        let result = query_who_imports(&snapshot, "Module/Agent.swift");

        let files: Vec<&str> = result.results.iter().map(|m| m.file.as_str()).collect();
        // Consumer.swift uses DefaultAgent -> reported with a line.
        // Unrelated.swift uses only `Error`, which Agent.swift also exports
        // (nested enum flattened) — so a file-level query still credits it on
        // stale snapshots; the symbol-level query above is the exact surface.
        assert!(files.contains(&"Module/Consumer.swift"));
        let consumer = result
            .results
            .iter()
            .find(|m| m.file == "Module/Consumer.swift")
            .expect("consumer match");
        assert_eq!(consumer.line, Some(21));
    }

    /// G2 (loctree-feedback.md, atuin): a symbol-scoped `who-imports` must not
    /// credit every importer of the DEFINING FILE as a consumer of the
    /// queried symbol. Module `m` exports both `a` and `b`; file `x` only
    /// `use`s `a`, file `y` only `use`s `b`. `who-imports a` must return `x`
    /// alone, not `y`.
    #[test]
    fn who_imports_symbol_excludes_importers_of_sibling_symbols() {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);

        let mut module = FileAnalysis::new("src/m.rs".into());
        module.exports.push(crate::types::ExportSymbol::new(
            "a".to_string(),
            "function",
            "named",
            Some(1),
        ));
        module.exports.push(crate::types::ExportSymbol::new(
            "b".to_string(),
            "function",
            "named",
            Some(5),
        ));
        snapshot.files.push(module);

        let mut file_x = FileAnalysis::new("src/x.rs".into());
        let mut import_a =
            crate::types::ImportEntry::new("m".to_string(), crate::types::ImportKind::Static);
        import_a.resolved_path = Some("src/m.rs".to_string());
        import_a.symbols.push(crate::types::ImportSymbol {
            name: "a".to_string(),
            alias: None,
            is_default: false,
        });
        file_x.imports.push(import_a);
        snapshot.files.push(file_x);

        let mut file_y = FileAnalysis::new("src/y.rs".into());
        let mut import_b =
            crate::types::ImportEntry::new("m".to_string(), crate::types::ImportKind::Static);
        import_b.resolved_path = Some("src/m.rs".to_string());
        import_b.symbols.push(crate::types::ImportSymbol {
            name: "b".to_string(),
            alias: None,
            is_default: false,
        });
        file_y.imports.push(import_b);
        snapshot.files.push(file_y);

        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "src/x.rs".to_string(),
            to: "src/m.rs".to_string(),
            label: "import".to_string(),
        });
        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "src/y.rs".to_string(),
            to: "src/m.rs".to_string(),
            label: "import".to_string(),
        });

        let result = query_who_imports(&snapshot, "a");
        let files: Vec<&str> = result.results.iter().map(|m| m.file.as_str()).collect();
        assert_eq!(
            files,
            vec!["src/x.rs"],
            "only the importer that actually names `a` may be reported, not the importer of sibling `b`: {:?}",
            result.results
        );
    }

    /// G2 companion: glob/star imports (`use m::*;`) cannot prove absence of
    /// the symbol, so they must stay counted even though no `ImportEntry`
    /// names the symbol directly.
    #[test]
    fn who_imports_symbol_keeps_glob_imports_counted() {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);

        let mut module = FileAnalysis::new("src/m.rs".into());
        module.exports.push(crate::types::ExportSymbol::new(
            "a".to_string(),
            "function",
            "named",
            Some(1),
        ));
        snapshot.files.push(module);

        let mut file_glob = FileAnalysis::new("src/glob.rs".into());
        let mut import_star =
            crate::types::ImportEntry::new("m".to_string(), crate::types::ImportKind::Static);
        import_star.resolved_path = Some("src/m.rs".to_string());
        import_star.symbols.push(crate::types::ImportSymbol {
            name: "*".to_string(),
            alias: None,
            is_default: false,
        });
        file_glob.imports.push(import_star);
        snapshot.files.push(file_glob);

        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "src/glob.rs".to_string(),
            to: "src/m.rs".to_string(),
            label: "import".to_string(),
        });

        let result = query_who_imports(&snapshot, "a");
        let files: Vec<&str> = result.results.iter().map(|m| m.file.as_str()).collect();
        assert_eq!(
            files,
            vec!["src/glob.rs"],
            "glob imports cannot prove absence of the symbol and must stay counted"
        );
    }

    /// G2 companion: `lazy_import`-labelled edges must remain distinguishable
    /// in `who-imports` output after the symbol-precision filter — the edge
    /// label stays embedded in `context` for both file- and symbol-mode
    /// queries.
    #[test]
    fn who_imports_marks_lazy_import_edges_distinguishably() {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);
        snapshot
            .files
            .push(FileAnalysis::new("src/utils.ts".into()));
        snapshot.files.push(FileAnalysis::new("src/app.ts".into()));
        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "src/app.ts".to_string(),
            to: "src/utils.ts".to_string(),
            label: "lazy_import".to_string(),
        });

        let result = query_who_imports(&snapshot, "src/utils.ts");
        assert_eq!(result.results.len(), 1);
        assert!(
            result.results[0]
                .context
                .as_deref()
                .is_some_and(|c| c.contains("lazy_import")),
            "lazy_import edges must be distinguishable in who-imports output: {:?}",
            result.results[0].context
        );
    }

    /// G10.1 (loctree-feedback.md): a vendored copy at
    /// `lua-api-crates/mux/src/pane.rs` must not collide with a genuine
    /// `mux/src/pane.rs` that also exists in the snapshot. `paths_match`'s
    /// suffix heuristic alone cannot tell the two apart; `who-imports` must
    /// require an exact match once the query resolves to a real, distinct
    /// file.
    #[test]
    fn who_imports_does_not_collide_vendored_path_with_genuine_file() {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);

        // The genuine crate file.
        snapshot
            .files
            .push(FileAnalysis::new("mux/src/pane.rs".into()));
        // A vendored copy living under an unrelated ancestor directory.
        snapshot
            .files
            .push(FileAnalysis::new("lua-api-crates/mux/src/pane.rs".into()));

        // Only the vendored copy has an importer.
        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "lua-api-crates/mux/src/lib.rs".to_string(),
            to: "lua-api-crates/mux/src/pane.rs".to_string(),
            label: "import".to_string(),
        });

        let result = query_who_imports(&snapshot, "mux/src/pane.rs");
        assert!(
            result.results.is_empty(),
            "importer of the vendored lua-api-crates copy must not be reported as an importer \
             of the genuine mux/src/pane.rs: {:?}",
            result.results
        );
    }

    /// G10.1 companion: the legitimate case must still work — a bare/partial
    /// query name with exactly one real owner still suffix-matches.
    #[test]
    fn who_imports_still_suffix_matches_when_target_has_one_real_owner() {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);
        snapshot
            .files
            .push(FileAnalysis::new("src/components/utils.ts".into()));
        snapshot.files.push(FileAnalysis::new("src/app.ts".into()));
        snapshot.edges.push(crate::snapshot::GraphEdge {
            from: "src/app.ts".to_string(),
            to: "src/components/utils.ts".to_string(),
            label: "import".to_string(),
        });

        let result = query_who_imports(&snapshot, "utils.ts");
        let files: Vec<&str> = result.results.iter().map(|m| m.file.as_str()).collect();
        assert_eq!(
            files,
            vec!["src/app.ts"],
            "a bare filename with one real owner must still resolve via suffix match"
        );
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
    fn test_query_where_symbol_labels_rust_enum_honestly() {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);
        let mut file = FileAnalysis::new("src/main.rs".into());
        file.exports.push(crate::types::ExportSymbol::new(
            "Commands".to_string(),
            "enum",
            "named",
            Some(7),
        ));
        snapshot.files.push(file);

        let result = query_where_symbol(&snapshot, "Commands");
        assert_eq!(result.results.len(), 1);
        assert_eq!(
            result.results[0].context.as_deref(),
            Some("rust enum Commands")
        );

        let mut private_file = FileAnalysis::new("src/private.rs".into());
        private_file.local_symbols.push(crate::types::LocalSymbol {
            name: "PrivateCommands".to_string(),
            kind: "enum".to_string(),
            line: Some(11),
            context: "enum PrivateCommands {".to_string(),
            is_exported: false,
        });
        snapshot.files.push(private_file);

        let private = query_where_symbol(&snapshot, "PrivateCommands");
        assert_eq!(private.results.len(), 1);
        assert_eq!(
            private.results[0].context.as_deref(),
            Some("rust enum PrivateCommands")
        );
    }

    #[test]
    fn test_query_where_symbol_labels_rust_fn_honestly() {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);
        let mut file = FileAnalysis::new("src/analysis_reports.rs".into());
        file.exports.push(crate::types::ExportSymbol::new(
            "is_test_file_path".to_string(),
            "function",
            "named",
            Some(568),
        ));
        snapshot.files.push(file);

        let result = query_where_symbol(&snapshot, "is_test_file_path");
        assert_eq!(result.results.len(), 1);
        assert_eq!(
            result.results[0].context.as_deref(),
            Some("fn is_test_file_path")
        );

        let mut local_file = FileAnalysis::new("src/local.rs".into());
        local_file.local_symbols.push(crate::types::LocalSymbol {
            name: "is_test_file_path".to_string(),
            kind: "function".to_string(),
            line: Some(12),
            context: String::new(),
            is_exported: false,
        });
        snapshot.files.push(local_file);

        let local = query_where_symbol(&snapshot, "is_test_file_path");
        assert_eq!(local.results.len(), 2);
        assert!(
            local
                .results
                .iter()
                .all(|m| m.context.as_deref() == Some("fn is_test_file_path")),
            "Rust functions must print fn NAME, not export function NAME: {:?}",
            local
                .results
                .iter()
                .map(|m| m.context.as_deref())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_query_where_symbol_deduplicates_one_physical_definition() {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);
        let mut file = FileAnalysis::new("src/recorder.rs".into());
        file.exports.push(crate::types::ExportSymbol {
            name: "start".to_string(),
            kind: "function".to_string(),
            export_type: "named".to_string(),
            line: Some(12),
            params: Vec::new(),
            symbol_id: crate::types::SymbolIdV1::default(),
        });
        file.impl_methods.push(crate::types::ImplMethod {
            name: "start".to_string(),
            qualifier: "Recorder".to_string(),
            line: Some(12),
            visibility: crate::types::Visibility::Public,
            ..Default::default()
        });
        snapshot.files.push(file);

        let result = query_where_symbol(&snapshot, "start");
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.total, 1);
        assert_eq!(
            result.results[0].context.as_deref(),
            Some("impl method Recorder::start")
        );
    }

    #[test]
    fn test_query_result_bounded_reports_total_and_truncation() {
        let snapshot = mock_snapshot();
        let result = QueryResult::complete(
            "where-symbol",
            "new",
            (0..40)
                .map(|line| QueryMatch {
                    file: "src/lib.rs".to_string(),
                    line: Some(line),
                    context: None,
                })
                .collect(),
            &snapshot,
        )
        .bounded(Some(25));

        assert_eq!(result.results.len(), 25);
        assert_eq!(result.total, 40);
        assert_eq!(result.emitted, 25);
        assert_eq!(result.offset, 0);
        assert_eq!(result.limit, Some(25));
        assert!(result.has_more);
        assert!(result.truncated);
        assert_eq!(result.universe.indexed_files, snapshot.files.len());
        let json = serde_json::to_value(&result).expect("query result serializes");
        assert_eq!(json["total"], 40);
        assert_eq!(json["emitted"], 25);
        assert_eq!(json["offset"], 0);
        assert_eq!(json["truncated"], true);
        for required in [
            "tracked",
            "untracked",
            "ignored",
            "generated",
            "fixtures",
            "exclusions",
        ] {
            assert!(
                json["universe"].get(required).is_some(),
                "universe must declare {required}"
            );
        }
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
