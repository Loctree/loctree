use std::collections::{BTreeMap, HashSet};

use super::{
    LiteralOccurrence, MatchRole, RoleSummary, ScopeClassification, ScopeClassificationCount,
    SuggestedNext, is_identifier,
};

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn suggested_next(query: &str, occurrences: &[LiteralOccurrence]) -> Vec<SuggestedNext> {
    let quoted_query = shell_quote(query);
    if occurrences.is_empty() {
        return vec![
            SuggestedNext {
                command: format!("loct find {quoted_query} --json"),
                reason: "broaden from literal absence to symbol and fuzzy search without treating suggestions as evidence".to_string(),
            },
            SuggestedNext {
                command: format!("loct query where-symbol {quoted_query} --json"),
                reason: "check whether the query is a known symbol definition rather than a literal occurrence".to_string(),
            },
        ];
    }

    let mut out = Vec::new();
    if is_identifier(query) {
        out.push(SuggestedNext {
            command: format!("loct body {quoted_query} --json"),
            reason: "open the definition/body for the matched identifier when available"
                .to_string(),
        });
        out.push(SuggestedNext {
            command: format!("loct find --literal {quoted_query} --json"),
            reason: "confirm literal parity before narrowing to structural interpretation"
                .to_string(),
        });
        out.push(SuggestedNext {
            command: format!("loct query where-symbol {quoted_query} --json"),
            reason: "separate definition locations from literal read/write sites".to_string(),
        });
    }
    if let Some(first) = occurrences.first() {
        out.push(SuggestedNext {
            command: format!("loct slice {}", shell_quote(&first.file)),
            reason:
                "inspect imports, dependencies, and consumers around the first literal-hit file"
                    .to_string(),
        });
    }
    out.push(SuggestedNext {
        command: "loct follow all".to_string(),
        reason:
            "look for repo-level dead, cycle, twin, hotspot, and trace signals after local evidence"
                .to_string(),
    });
    out
}

/// Bucket the full occurrence set into the definition-vs-callsite [`RoleSummary`].
/// Returns `None` for an empty set so a not-found result omits the rollup.
pub(super) fn role_summary(occurrences: &[LiteralOccurrence]) -> Option<RoleSummary> {
    if occurrences.is_empty() {
        return None;
    }
    let mut summary = RoleSummary {
        definitions: 0,
        callsites: 0,
        imports: 0,
        non_code: 0,
        other: 0,
        definition_files: Vec::new(),
    };
    let mut def_files_seen = HashSet::new();
    for occ in occurrences {
        match occ.match_role {
            MatchRole::Definition => {
                summary.definitions += 1;
                if def_files_seen.insert(occ.file.as_str()) {
                    summary.definition_files.push(occ.file.clone());
                }
            }
            MatchRole::Reference
            | MatchRole::Mutation
            | MatchRole::FieldEmission
            | MatchRole::LocalBinding => summary.callsites += 1,
            MatchRole::Import => summary.imports += 1,
            MatchRole::Comment | MatchRole::StringLiteral | MatchRole::DataAttribute => {
                summary.non_code += 1
            }
            MatchRole::StyleProperty
            | MatchRole::ClassToken
            | MatchRole::StyleVariable
            | MatchRole::Unknown => summary.other += 1,
        }
    }
    Some(summary)
}

pub(super) fn scope_classification_counts(
    occurrences: &[LiteralOccurrence],
) -> Vec<ScopeClassificationCount> {
    let mut counts: BTreeMap<&'static str, (ScopeClassification, usize)> = BTreeMap::new();
    for occ in occurrences {
        let entry = counts
            .entry(occ.scope_classification.as_str())
            .or_insert((occ.scope_classification, 0));
        entry.1 += 1;
    }
    counts
        .into_values()
        .map(|(scope_classification, count)| ScopeClassificationCount {
            scope_classification,
            count,
        })
        .collect()
}
