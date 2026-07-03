//! Unified search - aggregates symbol, semantic, and dead code results in one call
//!
//! Agent-friendly: no need to know which flag to use, get everything at once.
//!
//! Multi-term queries ("A B" or "A|B") use cross-matching: show where terms
//! MEET each other (same file, same function signature), not flat OR results.

use crate::analyzer::dead_parrots::{
    DeadFilterConfig, SimilarityCandidate, SymbolMatchKind, SymbolSearchResult, find_dead_exports,
    find_similar, search_symbol,
};
use crate::colors::Painter;
use crate::types::{ColorMode, FileAnalysis, OutputMode};
use serde::Serialize;
use serde_json::json;

/// A match for a parameter in a function export.
#[derive(Debug, Clone, Serialize)]
pub struct ParamMatch {
    pub file: String,
    pub line: Option<usize>,
    pub function: String,
    pub param_name: String,
    pub param_type: Option<String>,
}

/// A lint suppression match (e.g., #[allow(dead_code)], @ts-ignore)
#[derive(Debug, Clone, Serialize)]
pub struct SuppressionMatch {
    pub file: String,
    pub line: usize,
    pub suppression_type: String, // "rust_allow", "ts_ignore", "eslint_disable", etc.
    pub lint_name: String,        // the actual lint being suppressed
    pub context: String,          // the full line for context
}

/// Aggregated search results
#[derive(Debug, Serialize)]
pub struct SearchResults {
    pub query: String,
    pub symbol_matches: SymbolSearchResult,
    pub param_matches: Vec<ParamMatch>,
    pub semantic_matches: Vec<SimilarityCandidate>,
    pub dead_status: DeadStatus,
    /// Lint suppression matches (e.g., #[allow(dead_code)])
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suppression_matches: Vec<SuppressionMatch>,
    /// Files containing 2+ different query terms (multi-query cross-match)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cross_matches: Vec<CrossMatchFile>,
}

/// A file that matches multiple query terms (cross-match)
#[derive(Debug, Clone, Serialize)]
pub struct CrossMatchFile {
    pub file: String,
    pub matched_terms: Vec<CrossMatchTerm>,
}

/// The type of match found in cross-match analysis
#[derive(Debug, Clone, Serialize)]
pub enum MatchType {
    /// Exported symbol (function, struct, class, etc.)
    Export { kind: String },
    /// Imported symbol
    Import { source: String },
    /// Function parameter
    Parameter {
        function: String,
        param_type: Option<String>,
    },
}

/// A single term match within a cross-match file
#[derive(Debug, Clone, Serialize)]
pub struct CrossMatchTerm {
    pub term: String,
    pub line: usize,
    pub context: String,
    pub match_type: MatchType,
}

/// Dead code status for the searched symbol
#[derive(Debug, Serialize)]
pub struct DeadStatus {
    pub is_exported: bool,
    pub is_dead: bool,
    pub dead_in_files: Vec<String>,
}

/// A clearly-labeled fuzzy suggestion for `loct find --literal` mode.
///
/// These are **never** literal matches — they are name-similarity hints
/// surfaced strictly *alongside* (and never inside) the literal truth layer,
/// so an agent can see "did you mean…" candidates without ever mistaking a
/// suggestion for evidence. The `source` marker is the deliberate inverse of
/// [`crate::analyzer::occurrences::LiteralOccurrence::source`] (`"literal"`):
/// a fuzzy suggestion is always `"fuzzy"`.
#[derive(Debug, Clone, Serialize)]
pub struct FuzzySuggestion {
    pub symbol: String,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    pub score: f64,
    /// Always `"fuzzy"`. Provenance marker — a suggestion is not evidence.
    pub source: &'static str,
}

/// Minimum name-similarity score for a fuzzy candidate to be *shown* as a
/// suggestion. The `find` presentation surfaces — `--literal` fuzzy_suggestions
/// and the default "Semantic Matches" section — show fuzzy hits as "did you
/// mean…" next to exact truth, so a weak name-guess there is noise that drowns
/// the real hits; below this score, drop it. The raw [`find_similar`] engine
/// keeps every candidate — only these surfaces apply the floor — so the
/// `where-symbol` zero-result fallback still gets its best-effort guess.
const FUZZY_NAME_MIN_SCORE: f64 = 0.7;

/// Compute fuzzy name-similarity suggestions for an identifier, for use as the
/// explicitly-labeled *secondary* section of `loct find --literal`.
///
/// The returned items are suggestions, not evidence; callers MUST keep them out
/// of the primary literal-match set. This wraps [`find_similar`] and stamps each
/// candidate with `source = "fuzzy"` so the provenance is unambiguous wherever
/// the suggestion travels. Candidates scoring below [`FUZZY_NAME_MIN_SCORE`]
/// are dropped: in literal mode a weak name-guess is noise, not signal.
pub fn literal_fuzzy_suggestions(ident: &str, analyses: &[FileAnalysis]) -> Vec<FuzzySuggestion> {
    find_similar(ident, analyses)
        .into_iter()
        .filter(|candidate| candidate.score >= FUZZY_NAME_MIN_SCORE)
        .map(|candidate| FuzzySuggestion {
            symbol: candidate.symbol,
            file: candidate.file,
            line: candidate.line,
            score: candidate.score,
            source: "fuzzy",
        })
        .collect()
}

/// Search for a query in function parameters across all analyses.
///
/// Single term: returns all params matching the term (flat search).
/// Multi-term ("foo|bar"): cross-match mode - returns only params from functions
/// where 2+ different terms match in the signature (param names, types, or function name).
/// This shows RELATIONSHIPS between terms, not just individual occurrences.
fn search_params(query: &str, analyses: &[FileAnalysis]) -> Vec<ParamMatch> {
    let terms: Vec<String> = query.split('|').map(|t| t.trim().to_lowercase()).collect();
    let is_multi = terms.len() >= 2;

    if !is_multi {
        // Single term: flat search (unchanged behavior)
        return search_params_flat(&terms[0], analyses);
    }

    // Multi-term: cross-match at function level
    // Only return params from functions where 2+ different terms are present
    let mut matches = Vec::new();

    for analysis in analyses {
        for export in &analysis.exports {
            // Collect which terms match in this function's signature
            let mut matched_terms: std::collections::HashSet<&str> =
                std::collections::HashSet::new();
            let mut term_params: Vec<ParamMatch> = Vec::new();

            // Check function name against terms
            let fn_lower = export.name.to_lowercase();
            for term in &terms {
                if fn_lower.contains(term.as_str()) {
                    matched_terms.insert(term);
                }
            }

            // Check each param (name + type) against terms
            for param in &export.params {
                let name_lower = param.name.to_lowercase();
                let type_lower = param
                    .type_annotation
                    .as_ref()
                    .map(|t| t.to_lowercase())
                    .unwrap_or_default();

                for term in &terms {
                    if name_lower.contains(term.as_str()) || type_lower.contains(term.as_str()) {
                        matched_terms.insert(term);
                        term_params.push(ParamMatch {
                            file: analysis.path.clone(),
                            line: export.line,
                            function: export.name.clone(),
                            param_name: param.name.clone(),
                            param_type: param.type_annotation.clone(),
                        });
                    }
                }
            }

            // Only include if 2+ different terms matched in this function
            if matched_terms.len() >= 2 {
                matches.extend(term_params);
            }
        }
    }

    matches
}

/// Flat param search for a single term (internal helper)
fn search_params_flat(term: &str, analyses: &[FileAnalysis]) -> Vec<ParamMatch> {
    let mut matches = Vec::new();
    for analysis in analyses {
        for export in &analysis.exports {
            for param in &export.params {
                let name_lower = param.name.to_lowercase();
                if name_lower.contains(term) {
                    matches.push(ParamMatch {
                        file: analysis.path.clone(),
                        line: export.line,
                        function: export.name.clone(),
                        param_name: param.name.clone(),
                        param_type: param.type_annotation.clone(),
                    });
                }
            }
        }
    }
    matches
}

/// Search for lint suppressions containing the query
/// Supports: #[allow(...)], #[deny(...)], @ts-ignore, @ts-expect-error, eslint-disable, # noqa
fn search_suppressions(query: &str, analyses: &[FileAnalysis]) -> Vec<SuppressionMatch> {
    use regex::Regex;
    use std::fs;

    let query_lower = query.to_lowercase();
    let mut matches = Vec::new();

    // Patterns for different suppression types
    // Rust: #[allow(lint)] or #[allow(clippy::lint)]
    let rust_allow_re = Regex::new(r"#\[(allow|deny|warn)\(([^)]+)\)\]").unwrap();
    // TypeScript/JavaScript: @ts-ignore, @ts-expect-error, eslint-disable
    let ts_ignore_re = Regex::new(r"@ts-(ignore|expect-error)").unwrap();
    let eslint_re = Regex::new(r"eslint-disable(-next-line|-line)?(\s+[\w-]+)?").unwrap();
    // Python: # noqa, # type: ignore
    let python_noqa_re = Regex::new(r"#\s*(noqa|type:\s*ignore)").unwrap();

    for analysis in analyses {
        // Read file content
        let content = match fs::read_to_string(&analysis.path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for (line_num, line) in content.lines().enumerate() {
            let line_lower = line.to_lowercase();
            let line_trimmed = line.trim();

            // Skip comments (lines starting with // or ///)
            if line_trimmed.starts_with("//") {
                continue;
            }

            // Rust #[allow(...)]
            if let Some(caps) = rust_allow_re.captures(line) {
                let directive = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let lints = caps.get(2).map(|m| m.as_str()).unwrap_or("");

                // Check if any lint in the allow matches our query
                for lint in lints.split(',') {
                    let lint = lint.trim();
                    let lint_lower = lint.to_lowercase();
                    if lint_lower.contains(&query_lower)
                        || lint_lower.replace("clippy::", "").contains(&query_lower)
                    {
                        matches.push(SuppressionMatch {
                            file: analysis.path.clone(),
                            line: line_num + 1,
                            suppression_type: format!("rust_{}", directive),
                            lint_name: lint.to_string(),
                            context: line.trim().to_string(),
                        });
                    }
                }
            }

            // TypeScript @ts-ignore / @ts-expect-error
            if ts_ignore_re.is_match(line)
                && query_lower.contains("ts-")
                && let Some(caps) = ts_ignore_re.captures(line)
            {
                let kind = caps.get(1).map(|m| m.as_str()).unwrap_or("ignore");
                matches.push(SuppressionMatch {
                    file: analysis.path.clone(),
                    line: line_num + 1,
                    suppression_type: "ts_directive".to_string(),
                    lint_name: format!("ts-{}", kind),
                    context: line.trim().to_string(),
                });
            }

            // ESLint disable
            if line_lower.contains("eslint-disable")
                && query_lower.contains("eslint")
                && let Some(caps) = eslint_re.captures(line)
            {
                let rule = caps
                    .get(2)
                    .map(|m| m.as_str().trim())
                    .unwrap_or("all")
                    .to_string();
                matches.push(SuppressionMatch {
                    file: analysis.path.clone(),
                    line: line_num + 1,
                    suppression_type: "eslint_disable".to_string(),
                    lint_name: rule,
                    context: line.trim().to_string(),
                });
            }

            // Python noqa / type: ignore
            if python_noqa_re.is_match(line)
                && (query_lower.contains("noqa") || query_lower.contains("type"))
                && let Some(caps) = python_noqa_re.captures(line)
            {
                let kind = caps.get(1).map(|m| m.as_str()).unwrap_or("noqa");
                matches.push(SuppressionMatch {
                    file: analysis.path.clone(),
                    line: line_num + 1,
                    suppression_type: "python_suppress".to_string(),
                    lint_name: kind.to_string(),
                    context: line.trim().to_string(),
                });
            }
        }
    }

    matches
}

/// Normalize query: convert spaces to | for OR matching
/// "A B C" → "A|B|C"
fn normalize_query(query: &str) -> String {
    if query.contains('|') {
        // Already has pipe - use as-is
        query.to_string()
    } else {
        // Split by whitespace, filter short tokens (min 2 chars), join with |
        let tokens: Vec<&str> = query.split_whitespace().filter(|t| t.len() >= 2).collect();
        if tokens.is_empty() {
            query.to_string()
        } else if tokens.len() == 1 {
            tokens[0].to_string()
        } else {
            tokens.join("|")
        }
    }
}

/// Run unified search - returns all result types
pub fn run_search(query: &str, analyses: &[FileAnalysis]) -> SearchResults {
    // Normalize: "A B C" → "A|B|C" for consistent OR matching
    let query = normalize_query(query);

    // 1. Symbol matches
    let symbol_matches = search_symbol(&query, analyses);

    // 2. Parameter matches (cross-matched for multi-term)
    let param_matches = search_params(&query, analyses);

    // 3. Semantic/similarity matches — name-similarity shown as "did you mean…",
    //    floored so sub-threshold guesses never masquerade as findings.
    let semantic_matches: Vec<_> = find_similar(&query, analyses)
        .into_iter()
        .filter(|candidate| candidate.score >= FUZZY_NAME_MIN_SCORE)
        .collect();

    // 4. Dead code status - check if query appears in dead exports
    let all_dead = find_dead_exports(analyses, false, None, DeadFilterConfig::default());
    let dead_for_query: Vec<_> = all_dead
        .iter()
        .filter(|d| d.symbol.to_lowercase().contains(&query.to_lowercase()))
        .collect();

    let is_exported = !symbol_matches.files.is_empty()
        || analyses.iter().any(|a| {
            a.exports
                .iter()
                .any(|e| e.name.to_lowercase().contains(&query.to_lowercase()))
        });

    let dead_status = DeadStatus {
        is_exported,
        is_dead: !dead_for_query.is_empty(),
        dead_in_files: dead_for_query.iter().map(|d| d.file.clone()).collect(),
    };

    // 5. Lint suppression matches
    let suppression_matches = search_suppressions(&query, analyses);

    // 6. Cross-match analysis - find files with 2+ different query terms
    let cross_matches = if query.contains('|') {
        compute_cross_matches(&query, analyses)
    } else {
        vec![]
    };

    SearchResults {
        query: query.to_string(),
        symbol_matches,
        param_matches,
        semantic_matches,
        dead_status,
        suppression_matches,
        cross_matches,
    }
}

/// Compute cross-matches: files containing 2+ different terms from a multi-query
/// Searches in: exports, imports, and function parameters
fn compute_cross_matches(query: &str, analyses: &[FileAnalysis]) -> Vec<CrossMatchFile> {
    use std::collections::HashMap;

    // Split query into individual terms
    let terms: Vec<&str> = query.split('|').filter(|t| !t.is_empty()).collect();
    if terms.len() < 2 {
        return vec![];
    }

    // For each file, track which terms match
    let mut file_matches: HashMap<String, Vec<CrossMatchTerm>> = HashMap::new();

    for analysis in analyses {
        for term in &terms {
            let term_lower = term.to_lowercase();

            // 1. Check EXPORTS
            for exp in &analysis.exports {
                if exp.name.to_lowercase().contains(&term_lower) {
                    file_matches
                        .entry(analysis.path.clone())
                        .or_default()
                        .push(CrossMatchTerm {
                            term: term.to_string(),
                            line: exp.line.unwrap_or(0),
                            context: format!("{} {}", exp.kind, exp.name),
                            match_type: MatchType::Export {
                                kind: exp.kind.clone(),
                            },
                        });
                }
            }

            // 2. Check IMPORTS
            for imp in &analysis.imports {
                for sym in &imp.symbols {
                    if sym.name.to_lowercase().contains(&term_lower) {
                        file_matches.entry(analysis.path.clone()).or_default().push(
                            CrossMatchTerm {
                                term: term.to_string(),
                                line: imp.line.unwrap_or(0),
                                context: format!("import {} from {}", sym.name, imp.source),
                                match_type: MatchType::Import {
                                    source: imp.source.clone(),
                                },
                            },
                        );
                    }
                }
            }

            // 3. Check PARAMETERS (param name and type annotation)
            for exp in &analysis.exports {
                for param in &exp.params {
                    // Match param name
                    if param.name.to_lowercase().contains(&term_lower) {
                        file_matches.entry(analysis.path.clone()).or_default().push(
                            CrossMatchTerm {
                                term: term.to_string(),
                                line: exp.line.unwrap_or(0),
                                context: format!(
                                    "{}({}: {})",
                                    exp.name,
                                    param.name,
                                    param.type_annotation.as_deref().unwrap_or("?")
                                ),
                                match_type: MatchType::Parameter {
                                    function: exp.name.clone(),
                                    param_type: param.type_annotation.clone(),
                                },
                            },
                        );
                    }
                    // Match param TYPE annotation
                    if let Some(typ) = &param.type_annotation
                        && typ.to_lowercase().contains(&term_lower)
                    {
                        file_matches.entry(analysis.path.clone()).or_default().push(
                            CrossMatchTerm {
                                term: term.to_string(),
                                line: exp.line.unwrap_or(0),
                                context: format!("{}({}: {})", exp.name, param.name, typ),
                                match_type: MatchType::Parameter {
                                    function: exp.name.clone(),
                                    param_type: Some(typ.clone()),
                                },
                            },
                        );
                    }
                }
            }
        }
    }

    // Filter to files with 2+ DIFFERENT terms
    let mut results: Vec<CrossMatchFile> = file_matches
        .into_iter()
        .filter_map(|(file, matches)| {
            // Count unique terms
            let unique_terms: std::collections::HashSet<_> =
                matches.iter().map(|m| &m.term).collect();
            if unique_terms.len() >= 2 {
                Some(CrossMatchFile {
                    file,
                    matched_terms: matches,
                })
            } else {
                None
            }
        })
        .collect();

    // Sort by number of matched terms (most first)
    results.sort_by_key(|b| std::cmp::Reverse(b.matched_terms.len()));
    results
}

// =============================================================================
// Display layer
// =============================================================================

/// Print search results - dispatches to single-term or multi-term display
pub fn print_search_results(
    results: &SearchResults,
    output: OutputMode,
    symbol_only: bool,
    dead_only: bool,
    semantic_only: bool,
    color: ColorMode,
) {
    if matches!(output, OutputMode::Json) {
        print_search_json(results, symbol_only, dead_only, semantic_only);
        return;
    }

    if matches!(output, OutputMode::Jsonl) {
        print_search_jsonl(results, symbol_only, dead_only, semantic_only);
        return;
    }

    let is_multi = results.query.contains('|');

    if is_multi && !results.cross_matches.is_empty() {
        print_search_multiterm(results, symbol_only, dead_only, semantic_only, color);
    } else {
        print_search_single(results, symbol_only, dead_only, semantic_only, color);
    }
}

/// Multi-term display: cross-match first (compact), symbols filtered to cross-match files
fn print_search_multiterm(
    results: &SearchResults,
    symbol_only: bool,
    dead_only: bool,
    semantic_only: bool,
    color: ColorMode,
) {
    let p = Painter::new(color);
    println!("Search results for: {}\n", results.query);

    // Collect cross-match file set for filtering
    let cross_files: std::collections::HashSet<&str> = results
        .cross_matches
        .iter()
        .map(|cm| cm.file.as_str())
        .collect();

    // 1. Cross-match files FIRST (compact summary)
    if !dead_only && !semantic_only {
        println!(
            "=== Cross-Match Files ({}) ===",
            results.cross_matches.len()
        );
        println!("  Files containing 2+ different query terms:\n");
        for cm in &results.cross_matches {
            // Group: term → [export_count, import_count, param_count]
            let mut term_summary: std::collections::BTreeMap<&str, [usize; 3]> =
                std::collections::BTreeMap::new();
            for t in &cm.matched_terms {
                let counts = term_summary.entry(&t.term).or_insert([0, 0, 0]);
                match &t.match_type {
                    MatchType::Export { .. } => counts[0] += 1,
                    MatchType::Import { .. } => counts[1] += 1,
                    MatchType::Parameter { .. } => counts[2] += 1,
                }
            }

            println!("  {} ({} terms)", p.path(&cm.file), term_summary.len());
            for (term, counts) in &term_summary {
                let mut parts = Vec::new();
                if counts[0] > 0 {
                    parts.push(format!("{} exports", counts[0]));
                }
                if counts[1] > 0 {
                    parts.push(format!("{} imports", counts[1]));
                }
                if counts[2] > 0 {
                    parts.push(format!("{} params", counts[2]));
                }
                println!("    ├─ {}: {}", term, parts.join(", "));
            }
        }
        println!();

        // 2. Symbol matches filtered to cross-match files only
        let filtered_count: usize = results
            .symbol_matches
            .files
            .iter()
            .filter(|f| cross_files.contains(f.file.as_str()))
            .map(|f| f.matches.len())
            .sum();

        if filtered_count > 0 {
            println!(
                "=== Symbol Matches ({} in cross-match files, {} total) ===",
                filtered_count, results.symbol_matches.total_matches
            );

            let mut definitions = Vec::new();
            let mut imports = Vec::new();
            let mut usages = Vec::new();

            for file_match in &results.symbol_matches.files {
                if !cross_files.contains(file_match.file.as_str()) {
                    continue;
                }
                for m in &file_match.matches {
                    match m.kind {
                        SymbolMatchKind::Definition => definitions.push((&file_match.file, m)),
                        SymbolMatchKind::Import => imports.push((&file_match.file, m)),
                        SymbolMatchKind::Usage => usages.push((&file_match.file, m)),
                    }
                }
            }

            print_symbol_sections(&p, &definitions, &imports, &usages);
        } else {
            println!(
                "=== Symbol Matches ({} total, none in cross-match files) ===\n",
                results.symbol_matches.total_matches
            );
        }
    }

    // 3. Parameter matches (already cross-matched at function level)
    if !dead_only && !semantic_only && !results.param_matches.is_empty() {
        print_param_matches(&results.param_matches);
    }

    // 4. Suppressions
    if !dead_only && !semantic_only && !results.suppression_matches.is_empty() {
        print_suppression_matches(&results.suppression_matches);
    }

    // 5. Semantic + Dead
    print_semantic_and_dead(results, symbol_only, dead_only, semantic_only, &p);
}

/// Single-term display: original layout (unchanged behavior)
fn print_search_single(
    results: &SearchResults,
    symbol_only: bool,
    dead_only: bool,
    semantic_only: bool,
    color: ColorMode,
) {
    let p = Painter::new(color);
    println!("Search results for: {}\n", results.query);

    // Symbol matches
    if !dead_only && !semantic_only {
        println!(
            "=== Symbol Matches ({}) ===",
            results.symbol_matches.total_matches
        );
        if results.symbol_matches.files.is_empty() {
            println!("  No symbol matches found.\n");
        } else {
            let mut definitions = Vec::new();
            let mut imports = Vec::new();
            let mut usages = Vec::new();

            for file_match in &results.symbol_matches.files {
                for m in &file_match.matches {
                    match m.kind {
                        SymbolMatchKind::Definition => definitions.push((&file_match.file, m)),
                        SymbolMatchKind::Import => imports.push((&file_match.file, m)),
                        SymbolMatchKind::Usage => usages.push((&file_match.file, m)),
                    }
                }
            }

            print_symbol_sections(&p, &definitions, &imports, &usages);
        }
    }

    // Parameter matches
    if !dead_only && !semantic_only && !results.param_matches.is_empty() {
        print_param_matches(&results.param_matches);
    }

    // Suppressions
    if !dead_only && !semantic_only && !results.suppression_matches.is_empty() {
        print_suppression_matches(&results.suppression_matches);
    }

    // Semantic + Dead
    print_semantic_and_dead(results, symbol_only, dead_only, semantic_only, &p);
}

// =============================================================================
// Shared print helpers
// =============================================================================

fn print_symbol_sections(
    p: &Painter,
    definitions: &[(&String, &crate::analyzer::dead_parrots::SymbolMatch)],
    imports: &[(&String, &crate::analyzer::dead_parrots::SymbolMatch)],
    usages: &[(&String, &crate::analyzer::dead_parrots::SymbolMatch)],
) {
    if !definitions.is_empty() {
        println!(
            "{}",
            p.header("── Definition ──────────────────────────────────────────────")
        );
        for (file, m) in definitions {
            let location = if m.line > 0 {
                format!("{}:{}", file, m.line)
            } else {
                file.to_string()
            };
            println!("  {} {}  {}", p.ok("[DEF]"), p.path(&location), m.context);
        }
        println!();
    }

    if !imports.is_empty() {
        let header = format!(
            "── Imports ({}) ────────────────────────────────────────────",
            imports.len()
        );
        println!("{}", p.header(&header));
        for (file, m) in imports {
            let location = if m.line > 0 {
                format!("{}:{}", file, m.line)
            } else {
                file.to_string()
            };
            println!("  {} {}  {}", p.info("[IMP]"), p.path(&location), m.context);
        }
        println!();
    }

    if !usages.is_empty() {
        let header = format!(
            "── Usages ({}) ─────────────────────────────────────────────",
            usages.len()
        );
        println!("{}", p.header(&header));
        for (file, m) in usages {
            let location = if m.line > 0 {
                format!("{}:{}", file, m.line)
            } else {
                file.to_string()
            };
            println!(
                "  {} {}  {}",
                p.symbol("[USE]"),
                p.path(&location),
                m.context
            );
        }
        println!();
    }
    println!();
}

fn print_param_matches(params: &[ParamMatch]) {
    println!("=== Parameter Matches ({}) ===", params.len());
    for pm in params {
        let type_info = pm
            .param_type
            .as_ref()
            .map(|t| format!(": {}", t))
            .unwrap_or_default();
        let line_info = pm.line.map(|l| format!(":{}", l)).unwrap_or_default();
        println!(
            "  {}{} - {}{} in {}()",
            pm.file, line_info, pm.param_name, type_info, pm.function
        );
    }
    println!();
}

fn print_suppression_matches(suppressions: &[SuppressionMatch]) {
    println!("=== Lint Suppressions ({}) ===", suppressions.len());
    for sm in suppressions {
        println!(
            "  {}:{} [{}] {}",
            sm.file, sm.line, sm.lint_name, sm.context
        );
    }
    println!();
}

fn print_semantic_and_dead(
    results: &SearchResults,
    symbol_only: bool,
    dead_only: bool,
    semantic_only: bool,
    p: &Painter,
) {
    // Semantic matches
    if !dead_only && !symbol_only {
        println!(
            "=== Semantic Matches ({}) ===",
            results.semantic_matches.len()
        );
        if results.semantic_matches.is_empty() {
            println!("  No semantic matches found.\n");
        } else {
            for candidate in &results.semantic_matches {
                println!("  {} (score: {:.2})", candidate.symbol, candidate.score);
                match candidate.line {
                    Some(line) => println!("    in {}:{}", p.path(&candidate.file), line),
                    None => println!("    in {}", p.path(&candidate.file)),
                }
            }
            println!();
        }
    }

    // Dead code status
    if !symbol_only && !semantic_only {
        println!("=== Dead Code Status ===");
        if !results.dead_status.is_exported {
            println!("  Symbol not found as export.\n");
        } else if results.dead_status.is_dead {
            println!("  WARNING: Symbol appears to be dead code in:");
            for file in &results.dead_status.dead_in_files {
                println!("    - {}", file);
            }
            println!();
        } else {
            println!("  OK: Symbol is used.\n");
        }
    }
}

// =============================================================================
// JSON output (unchanged - full data for tooling)
// =============================================================================

fn print_search_json(
    results: &SearchResults,
    symbol_only: bool,
    dead_only: bool,
    semantic_only: bool,
) {
    let output = if symbol_only {
        json!({
            "query": results.query,
            "symbol_matches": results.symbol_matches,
            "param_matches": results.param_matches,
        })
    } else if dead_only {
        json!({
            "query": results.query,
            "dead_status": results.dead_status,
        })
    } else if semantic_only {
        json!({
            "query": results.query,
            "semantic_matches": results.semantic_matches,
        })
    } else {
        json!({
            "query": results.query,
            "symbol_matches": results.symbol_matches,
            "param_matches": results.param_matches,
            "semantic_matches": results.semantic_matches,
            "suppression_matches": results.suppression_matches,
            "cross_matches": results.cross_matches,
            "dead_status": results.dead_status,
        })
    };

    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}

fn print_search_jsonl(
    results: &SearchResults,
    symbol_only: bool,
    dead_only: bool,
    semantic_only: bool,
) {
    // Each result type on its own line
    if !dead_only && !semantic_only {
        println!(
            "{}",
            json!({"type": "symbol_matches", "data": results.symbol_matches})
        );
        if !results.param_matches.is_empty() {
            println!(
                "{}",
                json!({"type": "param_matches", "data": results.param_matches})
            );
        }
        if !results.suppression_matches.is_empty() {
            println!(
                "{}",
                json!({"type": "suppression_matches", "data": results.suppression_matches})
            );
        }
        if !results.cross_matches.is_empty() {
            println!(
                "{}",
                json!({"type": "cross_matches", "data": results.cross_matches})
            );
        }
    }
    if !dead_only && !symbol_only {
        println!(
            "{}",
            json!({"type": "semantic_matches", "data": results.semantic_matches})
        );
    }
    if !symbol_only && !semantic_only {
        println!(
            "{}",
            json!({"type": "dead_status", "data": results.dead_status})
        );
    }
}
