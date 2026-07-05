//! Symbol search, impact analysis, and similarity detection

use serde::Serialize;
use std::collections::HashSet;

use crate::similarity::similarity;
use crate::types::FileAnalysis;

use crate::analyzer::root_scan::{RootContext, normalize_module_id};

use super::paths_match;

/// Result of symbol search across the codebase
#[derive(Debug, Clone, Serialize)]
pub struct SymbolSearchResult {
    pub found: bool,
    pub total_matches: usize,
    pub files: Vec<SymbolFileMatch>,
}

/// Matches in a single file
#[derive(Debug, Clone, Serialize)]
pub struct SymbolFileMatch {
    pub file: String,
    pub matches: Vec<SymbolMatch>,
}

/// The kind of symbol match found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SymbolMatchKind {
    Definition,
    Import,
    Usage,
}

/// A single symbol match
#[derive(Debug, Clone, Serialize)]
pub struct SymbolMatch {
    pub line: usize,
    pub context: String,
    pub is_definition: bool,
    pub kind: SymbolMatchKind,
}

/// Result of impact analysis
#[derive(Debug, Clone, Serialize)]
pub struct ImpactResult {
    pub targets: Vec<String>,
    pub dependents: Vec<String>,
}

/// Result of similarity check
#[derive(Debug, Clone, Serialize)]
pub struct SimilarityCandidate {
    pub symbol: String,
    pub file: String,
    pub score: f64,
    /// 1-based line number for navigable output (None for file-path matches).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

/// Search for symbol occurrences across analyzed files (case-insensitive).
/// Supports regex patterns (e.g., `foo|bar` for OR matching).
/// Falls back to export list so it works even without `--symbol` pre-scan.
pub fn search_symbol(symbol: &str, analyses: &[FileAnalysis]) -> SymbolSearchResult {
    use regex::RegexBuilder;

    // Build case-insensitive regex matcher
    let matcher = RegexBuilder::new(symbol)
        .case_insensitive(true)
        .build()
        .ok();

    // Fallback to simple contains if regex is invalid
    let needle = symbol.to_lowercase();
    let matches_text = |text: &str| -> bool {
        if let Some(ref re) = matcher {
            re.is_match(text)
        } else {
            text.to_lowercase().contains(&needle)
        }
    };

    let mut files = Vec::new();
    let mut total_matches = 0;

    for analysis in analyses {
        let mut matches = Vec::new();

        // 1) Recorded line matches (only present if scan was run with --symbol)
        for m in &analysis.matches {
            if matches_text(&m.context) {
                let ctx_lower = m.context.to_lowercase();
                let is_def = ctx_lower.contains("export ")
                    || ctx_lower.contains("pub ")
                    || ctx_lower.contains("function ")
                    || ctx_lower.contains("class ")
                    || ctx_lower.contains("const ")
                    || ctx_lower.contains("let ")
                    || ctx_lower.contains("var ")
                    || ctx_lower.starts_with("fn ");
                matches.push(SymbolMatch {
                    line: m.line,
                    context: m.context.clone(),
                    is_definition: is_def,
                    kind: if is_def {
                        SymbolMatchKind::Definition
                    } else {
                        SymbolMatchKind::Usage
                    },
                });
            }
        }

        // 2) Exports list (always available) - regex or substring match
        for exp in &analysis.exports {
            if matches_text(&exp.name) {
                matches.push(SymbolMatch {
                    line: exp.line.unwrap_or(0),
                    context: format!("export {} {}", exp.kind, exp.name),
                    is_definition: true,
                    kind: SymbolMatchKind::Definition,
                });
            }
        }

        // 3) Local symbols (non-exported definitions)
        for local in &analysis.local_symbols {
            if matches_text(&local.name) {
                let context = if local.context.is_empty() {
                    format!("local {} {}", local.kind, local.name)
                } else {
                    local.context.clone()
                };
                matches.push(SymbolMatch {
                    line: local.line.unwrap_or(0),
                    context,
                    is_definition: true,
                    kind: SymbolMatchKind::Definition,
                });
            }
        }

        // 4) Import symbols (local bindings + aliases)
        for imp in &analysis.imports {
            if imp.symbols.is_empty() {
                continue;
            }
            let import_kw = if matches!(imp.kind, crate::types::ImportKind::Type) {
                "import type"
            } else {
                "import"
            };
            let source = if imp.source_raw.is_empty() {
                imp.source.as_str()
            } else {
                imp.source_raw.as_str()
            };
            for sym in &imp.symbols {
                let mut matched = matches_text(&sym.name);
                if !matched && let Some(alias) = &sym.alias {
                    matched = matches_text(alias);
                }
                if matched {
                    let display = if sym.is_default {
                        sym.name.clone()
                    } else if let Some(alias) = &sym.alias {
                        format!("{} as {}", sym.name, alias)
                    } else {
                        sym.name.clone()
                    };
                    let context = format!("{} {} from {}", import_kw, display, source);
                    matches.push(SymbolMatch {
                        line: imp.line.unwrap_or(0),
                        context,
                        is_definition: false,
                        kind: SymbolMatchKind::Import,
                    });
                }
            }
        }

        // 5) Local usage sites
        for usage in &analysis.symbol_usages {
            if matches_text(&usage.name) {
                let context = if usage.context.is_empty() {
                    format!("use {}", usage.name)
                } else {
                    usage.context.clone()
                };
                matches.push(SymbolMatch {
                    line: usage.line,
                    context,
                    is_definition: false,
                    kind: SymbolMatchKind::Usage,
                });
            }
        }

        if !matches.is_empty() {
            total_matches += matches.len();
            files.push(SymbolFileMatch {
                file: analysis.path.clone(),
                matches,
            });
        }
    }

    SymbolSearchResult {
        found: !files.is_empty(),
        total_matches,
        files,
    }
}

/// Analyze impact of changing a file - find all files that depend on it
pub fn analyze_impact(
    target_path: &str,
    analyses: &[FileAnalysis],
    contexts: &[RootContext],
) -> Option<ImpactResult> {
    let mut targets = Vec::new();
    for analysis in analyses {
        // Use proper path matching to avoid false positives
        if paths_match(&analysis.path, target_path) {
            targets.push(analysis.path.clone());
        }
    }

    if targets.is_empty() {
        return None;
    }

    // Build target sets for both normalized and full paths
    let normalized_targets: HashSet<String> = targets
        .iter()
        .map(|t| normalize_module_id(t).as_key())
        .collect();
    let full_targets: HashSet<String> = targets.iter().cloned().collect();
    let mut dependent_ids = HashSet::new();

    for ctx in contexts {
        for (source, target, _weight) in &ctx.graph_edges {
            // Match against both normalized module IDs and full paths
            // (edges may use full paths after snapshot format changes)
            let target_normalized = normalize_module_id(target).as_key();
            if normalized_targets.contains(target)
                || normalized_targets.contains(&target_normalized)
                || full_targets.contains(target)
            {
                dependent_ids.insert(source.clone());
            }
        }
    }

    let mut deps = Vec::new();
    for analysis in analyses {
        // Match against both full path and normalized (edges may use either)
        let id = normalize_module_id(&analysis.path).as_key();
        if dependent_ids.contains(&id) || dependent_ids.contains(&analysis.path) {
            deps.push(analysis.path.clone());
        }
    }
    deps.sort();
    deps.dedup();

    Some(ImpactResult {
        targets,
        dependents: deps,
    })
}

/// Find similar components/symbols in the codebase
pub fn find_similar(query: &str, analyses: &[FileAnalysis]) -> Vec<SimilarityCandidate> {
    let mut candidates: Vec<SimilarityCandidate> = Vec::new();

    for analysis in analyses {
        // Check file path similarity
        let path_score = similarity(query, &analysis.path);
        if path_score > 0.3 {
            candidates.push(SimilarityCandidate {
                symbol: analysis.path.clone(),
                file: "file path".to_string(),
                score: path_score,
                line: None,
            });
        }

        // Check exported symbols
        for exp in &analysis.exports {
            let sym_score = similarity(query, &exp.name);
            if sym_score > 0.4 {
                candidates.push(SimilarityCandidate {
                    symbol: exp.name.clone(),
                    file: format!("export in {}", analysis.path),
                    score: sym_score,
                    line: exp.line,
                });
            }
        }
    }

    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.dedup_by(|a, b| a.symbol == b.symbol && a.file == b.file);
    candidates.truncate(20);

    candidates
}
