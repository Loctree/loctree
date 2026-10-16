use std::collections::{BTreeMap, HashSet};

use super::{
    HitShape, LiteralOccurrence, MatchConfidence, MatchRole, RoleSummary, ScopeClassification,
    ScopeClassificationCount, SuggestedNext, is_identifier,
};

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Git conflict markers whose *middle* form is periodic: `=======` matches at
/// every 7th column of a decorative `# ==========` banner, so one banner line
/// alone yields ~10 hits. Anchoring is the fix, not filtering.
const CONFLICT_MARKERS: [&str; 3] = ["<<<<<<<", "=======", ">>>>>>>"];

/// Suggest the anchored conflict-marker pattern when the caller ran an
/// unanchored one.
///
/// Only fires for a pattern that names a conflict marker and carries neither
/// `^` nor `$` — the moment a caller anchors anything, they own the shape and
/// this hint would be noise. Regex path only: `--literal` never evaluates
/// anchors, so recommending them there would be a lie.
pub(super) fn conflict_marker_anchor_hint(pattern: &str) -> Option<SuggestedNext> {
    if pattern.contains('^') || pattern.contains('$') {
        return None;
    }
    if !CONFLICT_MARKERS.iter().any(|m| pattern.contains(m)) {
        return None;
    }
    Some(SuggestedNext {
        command: r"loct find --regex '^(<{7} |={7}$|>{7} )'".to_string(),
        reason:
            "unanchored conflict markers match mid-line: `=======` hits every 7th column of a `# =====` banner — anchor to line starts/ends for real conflict hunks"
                .to_string(),
    })
}

pub(super) fn suggested_next(query: &str, occurrences: &[LiteralOccurrence]) -> Vec<SuggestedNext> {
    let quoted_query = shell_quote(query);
    if occurrences.is_empty() {
        return vec![
            SuggestedNext {
                command: format!("loct find --discover {quoted_query} --json"),
                reason: "broaden from literal absence to symbol and fuzzy search without treating suggestions as evidence".to_string(),
            },
            SuggestedNext {
                command: format!("loct query where-symbol {quoted_query} --json"),
                reason: "check whether the query is a known symbol definition rather than a literal occurrence".to_string(),
            },
        ];
    }

    let mut out = Vec::new();
    let shape = hit_shape(occurrences);

    // Shape-driven next moves first when the distribution is trustworthy.
    if let Some(shape) = shape.as_ref() {
        match shape.label {
            "single_writer" => {
                if let Some(def_file) = occurrences
                    .iter()
                    .find(|o| o.match_role == MatchRole::Definition)
                    .map(|o| o.file.as_str())
                {
                    out.push(SuggestedNext {
                        command: format!("loct slice {}", shell_quote(def_file)),
                        reason: "single-writer flag: inspect the defining file's consumers before changing the write site"
                            .to_string(),
                    });
                }
                if let Some(write) = occurrences
                    .iter()
                    .find(|o| o.match_role == MatchRole::Mutation)
                {
                    out.push(SuggestedNext {
                        command: format!(
                            "loct body {}",
                            shell_quote(
                                write
                                    .enclosing_symbol
                                    .as_ref()
                                    .map(|s| s.name.as_str())
                                    .unwrap_or(query)
                            )
                        ),
                        reason: "open the function that performs the sole write".to_string(),
                    });
                }
            }
            "read_only" => {
                out.push(SuggestedNext {
                    command: format!("loct query where-symbol {quoted_query} --json"),
                    reason:
                        "read-only hit set — confirm the definition, then audit callers via impact"
                            .to_string(),
                });
            }
            "reference_only" => {
                out.push(SuggestedNext {
                    command: format!("loct query where-symbol {quoted_query} --json"),
                    reason: "no definition role in hits — locate the declaration before interpreting reads"
                        .to_string(),
                });
            }
            _ => {}
        }
    }

    if is_identifier(query) {
        if !out.iter().any(|s| s.command.starts_with("loct body ")) {
            out.push(SuggestedNext {
                command: format!("loct body {quoted_query} --json"),
                reason: "open the definition/body for the matched identifier when available"
                    .to_string(),
            });
        }
        if !out.iter().any(|s| s.command.contains("where-symbol")) {
            out.push(SuggestedNext {
                command: format!("loct query where-symbol {quoted_query} --json"),
                reason: "separate definition locations from literal read/write sites".to_string(),
            });
        }
    }
    if let Some(first) = occurrences
        .iter()
        .find(|hit| hit.match_role == MatchRole::Definition)
        .or_else(|| occurrences.first())
        && !out.iter().any(|s| s.command.starts_with("loct slice "))
    {
        out.push(SuggestedNext {
            command: format!("loct slice {}", shell_quote(&first.file)),
            reason:
                "inspect imports, dependencies, and consumers around the defining file or first literal hit"
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

/// Symbol kinds whose value can actually be assigned. Only these justify the
/// dataflow narrative (`single_writer` / `read_only`) — a module, a type or a
/// function has no "write site" to be the single writer of.
fn kind_is_value_like(kind: &str) -> bool {
    matches!(
        kind.trim().to_ascii_lowercase().as_str(),
        "variable"
            | "var"
            | "let"
            | "const"
            | "constant"
            | "static"
            | "field"
            | "property"
            | "member"
            | "binding"
    )
}

/// The declaration keyword immediately preceding the matched identifier, if any.
fn definition_introducer(occ: &LiteralOccurrence) -> Option<String> {
    let chars: Vec<char> = occ.context.chars().collect();
    let start = occ.column.saturating_sub(1).min(chars.len());
    let prefix: String = chars[..start].iter().collect();
    prefix
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .rfind(|token| !token.is_empty())
        .map(|token| token.to_string())
}

/// Does this definition site describe a *value* (variable / field / constant)?
///
/// Symbol-table truth wins when present; otherwise the lexical introducer
/// decides. An introducer we cannot name is treated as NOT value-like on
/// purpose: withholding a dataflow story costs an agent one extra command,
/// while confabulating one costs it the wrong edit.
fn definition_is_value_like(occ: &LiteralOccurrence) -> bool {
    if let Some(kind) = occ
        .resolved_definition
        .as_ref()
        .or_else(|| {
            occ.definition_candidates
                .iter()
                .find(|def| def.file == occ.file && def.line == Some(occ.line))
        })
        .map(|def| def.kind.as_str())
    {
        return kind_is_value_like(kind);
    }
    // No symbol-table entry: fall back to the same lexical introducer that
    // promoted this hit to `Definition` in the first place. `pub mod
    // health_score;` reaches Definition through `derive_match_role`'s
    // introducer table (`mod`), never through the symbol table — reading that
    // introducer back is the only thing that can tell the shape layer this is
    // a module and not a state flag.
    definition_introducer(occ).is_some_and(|intro| kind_is_value_like(&intro))
}

/// The single definition in a hit set, when there is exactly one.
fn sole_definition(occurrences: &[LiteralOccurrence]) -> Option<&LiteralOccurrence> {
    let mut defs = occurrences
        .iter()
        .filter(|o| o.match_role == MatchRole::Definition);
    let first = defs.next()?;
    defs.next().is_none().then_some(first)
}

/// Synthesize a distribution shape from role counts.
///
/// Returns `None` for an empty set. Labels stay conservative: mixed or sparse
/// patterns become `mixed` / `reference_only` / `unknown` rather than a
/// confident wrong story. Authority is `loctree_derived` only when at least one
/// high-confidence definition is present (symbol-table proven); otherwise
/// `semantic_guess`.
///
/// The dataflow labels (`single_writer`, `read_only`) additionally require the
/// sole definition to be a *value* declaration. `loct find health_score` used
/// to narrate "1 definition, 1 write site, 60 read sites — state flag /
/// single-writer pattern" when that definition was `pub mod health_score;`:
/// role counts alone cannot tell a module from a variable, so counting was
/// never enough authority for the story.
pub(super) fn hit_shape(occurrences: &[LiteralOccurrence]) -> Option<HitShape> {
    if occurrences.is_empty() {
        return None;
    }

    let mut definitions = 0usize;
    let mut writers = 0usize;
    let mut readers = 0usize;
    let mut high_conf_definitions = 0usize;
    let mut non_test = 0usize;

    for occ in occurrences {
        if occ.scope_classification != ScopeClassification::Test {
            non_test += 1;
        }
        match occ.match_role {
            MatchRole::Definition => {
                definitions += 1;
                if occ.confidence == MatchConfidence::High {
                    high_conf_definitions += 1;
                }
            }
            MatchRole::Mutation | MatchRole::FieldEmission => writers += 1,
            MatchRole::Reference | MatchRole::LocalBinding => readers += 1,
            _ => {}
        }
    }

    let authority = if high_conf_definitions > 0 {
        "loctree_derived"
    } else {
        "semantic_guess"
    };

    // Dataflow narration is gated on the definition being a value: a module
    // or type declaration has nothing to write to.
    let value_definition = sole_definition(occurrences).is_some_and(definition_is_value_like);
    let withheld_intro = (!value_definition && definitions == 1)
        .then(|| sole_definition(occurrences).and_then(definition_introducer))
        .flatten();

    let (label, note) = if non_test == 0 {
        (
            "test_only",
            Some("every hit sits in a test-scoped file".to_string()),
        )
    } else if definitions == 1 && writers == 1 && readers >= 1 && value_definition {
        (
            "single_writer",
            Some(format!(
                "1 definition, 1 write site, {readers} read site(s) — state flag / single-writer pattern"
            )),
        )
    } else if definitions == 1 && writers == 0 && readers >= 1 && value_definition {
        (
            "read_only",
            Some(format!(
                "1 definition and {readers} read site(s); no assignment sites in the hit set"
            )),
        )
    } else if definitions >= 1 && writers == 0 && readers == 0 {
        (
            "definition_only",
            Some(
                "definition site(s) only — no reads or writes in the scanned universe".to_string(),
            ),
        )
    } else if definitions == 0 && writers == 0 && readers >= 1 {
        (
            "reference_only",
            Some("no definition role in this hit set — pair with where-symbol or body".to_string()),
        )
    } else if definitions > 0 || writers > 0 || readers > 0 {
        let note = match withheld_intro.as_deref() {
            Some(intro) => format!(
                "{definitions} def / {writers} write / {readers} read — the definition is a `{intro}` declaration, not a value; dataflow shape withheld"
            ),
            None => format!(
                "{definitions} def / {writers} write / {readers} read — inspect roles before acting"
            ),
        };
        ("mixed", Some(note))
    } else {
        ("unknown", None)
    };

    Some(HitShape {
        label,
        definitions,
        writers,
        readers,
        authority,
        note,
    })
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
        references: 0,
        mutations: 0,
        field_emissions: 0,
        local_bindings: 0,
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
            MatchRole::Reference => {
                summary.callsites += 1;
                summary.references += 1;
            }
            MatchRole::Mutation => {
                summary.callsites += 1;
                summary.mutations += 1;
            }
            MatchRole::FieldEmission => {
                summary.callsites += 1;
                summary.field_emissions += 1;
            }
            MatchRole::LocalBinding => {
                summary.callsites += 1;
                summary.local_bindings += 1;
            }
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

/// C: one entity-first group for the default (non-`--literal`) summary.
///
/// Hits sharing a resolved definition roll into one header with their
/// production call sites listed; test/doc/other hits collapse to counts so a
/// hot symbol's summary does not drown in its own test suite. `--literal`
/// output never calls this — it stays the flat per-hit list, byte-compatible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupedEntity {
    /// The resolved symbol's stable id, when the snapshot proved one. `None`
    /// groups every unresolved hit together rather than fabricating an
    /// identity for it — see `group_by_entity`'s key choice.
    pub symbol_id: Option<String>,
    /// Header label: the resolved definition's `name (file)`, or the bare
    /// matched text suffixed `(unresolved)` when nothing resolved.
    pub label: String,
    /// Definition sites in this group, as `(file, line)`. A definition is
    /// where the entity lives, not somebody calling it — counting it among
    /// "production caller(s)" overstated fan-in by one everywhere (N05).
    pub definitions: Vec<(String, usize)>,
    /// Production-scope non-definition hits in this group, as
    /// `(file, line, role)`, in the order they appear in the input
    /// (already ranked by the caller).
    pub production_callers: Vec<(String, usize, MatchRole)>,
    /// Test-scope hit count, collapsed — not listed individually.
    pub test_count: usize,
    /// Docs-scope hit count, collapsed.
    pub docs_count: usize,
    /// Every remaining scope (Config/Generated/Unknown), collapsed.
    pub other_count: usize,
}

/// Group a hit set entity-first: one header per resolved definition (or, for
/// hits that resolved nothing, one shared `(unresolved)` group) instead of
/// the flat file/line list `--literal` prints. Preserves the input order for
/// group first-appearance and for each group's production callers, so
/// grouping a already-ranked (`rank_occurrences`) hit set keeps that order.
pub fn group_by_entity(occurrences: &[LiteralOccurrence]) -> Vec<GroupedEntity> {
    let mut order: Vec<Option<String>> = Vec::new();
    let mut groups: std::collections::HashMap<Option<String>, GroupedEntity> =
        std::collections::HashMap::new();

    for occ in occurrences {
        let key = occ
            .resolved_definition
            .as_ref()
            .map(|def| def.symbol_id.clone());
        let group = groups.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            let label = match &occ.resolved_definition {
                Some(def) => format!("{} ({})", def.name, def.file),
                None => format!("{} (unresolved)", occ.matched_text),
            };
            GroupedEntity {
                symbol_id: key.clone(),
                label,
                definitions: Vec::new(),
                production_callers: Vec::new(),
                test_count: 0,
                docs_count: 0,
                other_count: 0,
            }
        });
        match occ.scope_classification {
            ScopeClassification::Production => {
                if occ.match_role == MatchRole::Definition {
                    group.definitions.push((occ.file.clone(), occ.line));
                } else {
                    group
                        .production_callers
                        .push((occ.file.clone(), occ.line, occ.match_role));
                }
            }
            ScopeClassification::Test => group.test_count += 1,
            ScopeClassification::Docs => group.docs_count += 1,
            ScopeClassification::Config
            | ScopeClassification::Generated
            | ScopeClassification::Unknown => group.other_count += 1,
        }
    }

    order
        .into_iter()
        .filter_map(|key| groups.remove(&key))
        .collect()
}

#[cfg(test)]
mod grouping_tests {
    use super::super::{ScanOptions, scan_files_with_scope};
    use super::*;

    #[test]
    fn discovery_advances_to_definition_structure_instead_of_repeating_find() {
        let res = scan_files_with_scope(
            [
                ("docs/GUIDE.md", "Use widget here.\n"),
                ("src/lib.rs", "pub fn widget() {}\n"),
            ],
            "widget",
            ScanOptions::default(),
            super::super::FileScope::default(),
        );
        let steps = suggested_next("widget", &res.occurrences);
        assert!(
            steps
                .iter()
                .any(|step| step.command == "loct slice 'src/lib.rs'")
        );
        assert!(
            !steps
                .iter()
                .any(|step| step.command.starts_with("loct find "))
        );
        assert!(steps.iter().any(|step| step.command == "loct follow all"));
    }

    #[test]
    fn group_by_entity_lists_production_callers_and_collapses_test_hits() {
        let res = scan_files_with_scope(
            [
                (
                    "src/lib.rs",
                    "pub fn widget() {}\nfn use_it() { widget(); }\n",
                ),
                (
                    "src/lib_test.rs",
                    "fn t1() { widget(); }\nfn t2() { widget(); }\n",
                ),
                ("docs/GUIDE.md", "call widget() to render\n"),
            ],
            "widget",
            ScanOptions::default(),
            super::super::FileScope::default(),
        );
        let groups = group_by_entity(&res.occurrences);
        assert_eq!(
            groups.len(),
            1,
            "every hit here is the same bare-query group (no snapshot enrichment ran)"
        );
        let group = &groups[0];
        assert!(
            group
                .production_callers
                .iter()
                .any(|(file, _, _)| file == "src/lib.rs"),
            "production hits are listed individually: {:?}",
            group.production_callers
        );
        assert_eq!(
            group.test_count, 2,
            "the two lib_test.rs hits collapse to a count, not two more rows"
        );
        assert_eq!(group.docs_count, 1, "the docs mention collapses too");
    }

    #[test]
    fn group_by_entity_splits_distinct_resolved_definitions() {
        let mut res = scan_files_with_scope(
            [
                ("src/a.rs", "pub fn widget() {}\n"),
                ("src/b.rs", "pub fn widget() {}\n"),
            ],
            "widget",
            ScanOptions::default(),
            super::super::FileScope::default(),
        );
        let snapshot: crate::snapshot::Snapshot = serde_json::from_str(
            r#"{
                "metadata": {},
                "files": [
                    {"path": "src/a.rs", "language": "rust", "exports": [{"name": "widget", "kind": "function", "export_type": "named", "line": 1}]},
                    {"path": "src/b.rs", "language": "rust", "exports": [{"name": "widget", "kind": "function", "export_type": "named", "line": 1}]}
                ]
            }"#,
        )
        .expect("minimal snapshot deserializes via serde defaults");
        super::super::enrich_with_snapshot(&mut res, &snapshot);

        let groups = group_by_entity(&res.occurrences);
        assert_eq!(
            groups.len(),
            2,
            "two distinct resolved definitions must NOT merge into one group"
        );
    }
}
