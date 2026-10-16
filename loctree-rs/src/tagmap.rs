//! Shared literal tagmap evidence collection, ordered before pagination.

use std::collections::{BTreeMap, HashSet};

use serde_json::{Value, json};

use crate::types::FileAnalysis;

/// Normalize separators and ASCII case without inventing lexical evidence.
pub fn tagmap_normalize(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Preserve raw substring recall, including non-ASCII terms, before normalization.
pub fn tagmap_matches(value: &str, keyword_lower: &str, keyword_normalized: &str) -> bool {
    let value_lower = value.to_ascii_lowercase();
    value_lower.contains(keyword_lower)
        || (!keyword_normalized.is_empty()
            && tagmap_normalize(&value_lower).contains(keyword_normalized))
}

/// Distinguish name evidence from source-line context (including comment echoes).
pub fn tagmap_symbol_usage_kind(
    usage_name: &str,
    usage_context: &str,
    keyword_lower: &str,
    keyword_normalized: &str,
) -> Option<(&'static str, bool)> {
    let context = usage_context.trim_start();
    let comment = context.starts_with('#')
        || context.starts_with("//")
        || context.starts_with("/*")
        || context.starts_with("<!--");
    if !comment && tagmap_matches(usage_name, keyword_lower, keyword_normalized) {
        Some(("symbol-usage", true))
    } else if tagmap_matches(usage_context, keyword_lower, keyword_normalized) {
        Some(("symbol-usage-context", false))
    } else {
        None
    }
}

/// Collect the complete logical evidence universe, then rank deterministically.
/// Definitions and import identities precede usages, literals, metadata, and
/// context. A context line is one fact, not one fact per unrelated token.
pub fn collect_tagmap_facts(files: &[FileAnalysis], keyword: &str) -> Vec<Value> {
    let lower = keyword.to_ascii_lowercase();
    let normalized = tagmap_normalize(keyword);
    let matches = |value: &str| tagmap_matches(value, &lower, &normalized);
    let mut facts: Vec<(u8, Value)> = Vec::new();
    let mut contexts = BTreeMap::new();

    for file in files {
        let mut definitions = HashSet::new();
        for export in &file.exports {
            let primary = matches(&export.name);
            if primary || matches(&export.kind) || matches(&export.export_type) {
                let mut fact = json!({
                    "kind": "export", "file": file.path, "name": export.name,
                    "symbol_kind": export.kind, "line": export.line
                });
                if !primary {
                    fact["confidence"] = json!("low");
                }
                definitions.insert((export.name.as_str(), export.line));
                facts.push((if primary { 0 } else { 5 }, fact));
            }
        }
        for local in &file.local_symbols {
            let primary = matches(&local.name);
            if (primary || matches(&local.kind))
                && !definitions.contains(&(local.name.as_str(), local.line))
            {
                let mut fact = json!({
                    "kind": "local-symbol", "file": file.path, "name": local.name,
                    "symbol_kind": local.kind, "line": local.line, "context": local.context
                });
                if !primary {
                    fact["confidence"] = json!("low");
                }
                facts.push((if primary { 1 } else { 5 }, fact));
            } else if !primary && matches(&local.context) {
                contexts
                    .entry((
                        file.path.as_str(),
                        local.line,
                        local.line.is_none().then_some(local.context.as_str()),
                    ))
                    .or_insert(local.context.as_str());
            }
        }
        for usage in &file.symbol_usages {
            match tagmap_symbol_usage_kind(&usage.name, &usage.context, &lower, &normalized) {
                Some((kind, true)) => facts.push((
                    3,
                    json!({
                        "kind": kind, "file": file.path, "name": usage.name,
                        "line": usage.line, "context": usage.context, "confidence": "high"
                    }),
                )),
                Some((_, false)) => {
                    contexts
                        .entry((file.path.as_str(), Some(usage.line), None))
                        .or_insert(usage.context.as_str());
                }
                None => {}
            }
        }
        for import in &file.imports {
            let primary =
                matches(&import.source) || import.resolved_path.as_deref().is_some_and(matches);
            if primary || matches(&import.source_raw) {
                let mut fact = json!({
                    "kind": "import-source", "file": file.path, "source": import.source,
                    "source_raw": import.source_raw, "line": import.line
                });
                if !primary {
                    fact["confidence"] = json!("low");
                }
                facts.push((if primary { 2 } else { 5 }, fact));
            }
            for symbol in &import.symbols {
                if matches(&symbol.name) || symbol.alias.as_deref().is_some_and(matches) {
                    facts.push((
                        2,
                        json!({
                            "kind": "import-symbol", "file": file.path, "name": symbol.name,
                            "alias": symbol.alias, "source": import.source, "line": import.line
                        }),
                    ));
                }
            }
        }
        for literal in &file.string_literals {
            if matches(&literal.value) {
                facts.push((
                    4,
                    json!({
                        "kind": "string-literal", "file": file.path,
                        "value": literal.value, "line": literal.line
                    }),
                ));
            }
        }
    }
    for ((file, line, _), context) in contexts {
        facts.push((
            6,
            json!({
                "kind": "symbol-usage-context", "file": file, "line": line,
                "name": keyword, "context": context, "confidence": "low"
            }),
        ));
    }
    facts.sort_by_cached_key(|(rank, fact)| {
        // A short matching identity (GlyphSet) is closer lexical evidence than
        // a long compound (createGlyphShareRouter), without privileging paths.
        let identity_length = ["name", "alias", "source", "value"]
            .iter()
            .filter_map(|key| fact[*key].as_str())
            .filter(|value| matches(value))
            .map(|value| {
                if normalized.is_empty() {
                    value.chars().count()
                } else {
                    tagmap_normalize(value).len()
                }
            })
            .min()
            .unwrap_or(usize::MAX);
        (
            *rank,
            identity_length,
            fact["file"].as_str().unwrap_or_default().to_owned(),
            fact["line"].as_u64().unwrap_or_default(),
            fact.to_string(),
        )
    });
    facts.dedup();
    facts.into_iter().map(|(_, fact)| fact).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ExportSymbol, LocalSymbol, SymbolUsage};

    #[test]
    fn definitions_include_examples_and_are_independent_of_snapshot_order() {
        let mut example = FileAnalysis::new("examples/owner.rs".to_string());
        example.exports.push(ExportSymbol::new(
            "glyph".to_string(),
            "function",
            "named",
            Some(8),
        ));
        example.local_symbols.push(LocalSymbol {
            name: "glyph".to_string(),
            kind: "function".to_string(),
            line: Some(8),
            context: "pub fn glyph() {}".to_string(),
            is_exported: true,
        });
        let mut usage = FileAnalysis::new("aaa_usage.py".to_string());
        usage.symbol_usages.push(SymbolUsage {
            name: "glyph".to_string(),
            line: 1,
            context: "glyph()".to_string(),
        });
        let forward = collect_tagmap_facts(&[usage.clone(), example.clone()], "glyph");
        assert_eq!(forward, collect_tagmap_facts(&[example, usage], "glyph"));
        assert_eq!(forward.len(), 2, "export and local are one definition");
        assert_eq!(forward[0]["file"], "examples/owner.rs");
        assert_eq!(forward[0]["kind"], "export");
        assert_eq!(forward[1]["kind"], "symbol-usage");
    }

    #[test]
    fn context_is_one_low_confidence_line_even_with_a_name_echo() {
        let mut file = FileAnalysis::new("notes.py".to_string());
        for name in ["glyph", "from", "Other", "Other"] {
            file.symbol_usages.push(SymbolUsage {
                name: name.to_string(),
                line: 3,
                context: "# glyph from Other".to_string(),
            });
        }
        file.local_symbols.push(LocalSymbol {
            name: "irrelevant".to_string(),
            kind: "variable".to_string(),
            line: Some(3),
            context: "# glyph from Other".to_string(),
            is_exported: false,
        });
        let facts = collect_tagmap_facts(&[file], "glyph");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0]["kind"], "symbol-usage-context");
        assert_eq!(facts[0]["confidence"], "low");
    }

    #[test]
    fn unknown_context_lines_do_not_collapse_distinct_evidence() {
        let mut file = FileAnalysis::new("notes.ts".to_string());
        for context in ["const a = glyph();", "const b = glyph();"] {
            file.local_symbols.push(LocalSymbol {
                name: "unrelated".to_string(),
                kind: "variable".to_string(),
                line: None,
                context: context.to_string(),
                is_exported: false,
            });
        }
        assert_eq!(collect_tagmap_facts(&[file], "glyph").len(), 2);
    }

    #[test]
    fn normalization_preserves_raw_non_ascii_and_rejects_absent_terms() {
        assert!(tagmap_matches(
            "glyph_engine",
            "GLYPH-ENGINE".to_ascii_lowercase().as_str(),
            "glyphengine"
        ));
        assert!(tagmap_matches("星", "星", ""));
        assert!(!tagmap_matches("unrelated", "星", ""));
        assert!(collect_tagmap_facts(&[], "glyph").is_empty());
    }

    #[test]
    fn tighter_matching_identities_precede_long_compounds_without_path_rules() {
        let mut broad = FileAnalysis::new("aaa.rs".to_string());
        broad.exports.push(ExportSymbol::new(
            "createGlyphShareRouter".to_string(),
            "function",
            "named",
            Some(1),
        ));
        let mut tight = FileAnalysis::new("examples/zzz.rs".to_string());
        tight.exports.push(ExportSymbol::new(
            "GlyphSet".to_string(),
            "type",
            "named",
            Some(1),
        ));
        let mut exact = FileAnalysis::new("zzz.rs".to_string());
        exact.exports.push(ExportSymbol::new(
            "glyph".to_string(),
            "function",
            "named",
            Some(1),
        ));
        let facts = collect_tagmap_facts(&[broad, tight, exact], "glyph");
        assert_eq!(
            facts
                .iter()
                .map(|fact| fact["name"].as_str().expect("identity"))
                .collect::<Vec<_>>(),
            ["glyph", "GlyphSet", "createGlyphShareRouter"]
        );
    }
}
