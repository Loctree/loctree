//! Bounded, source-first context for an identifier's default `find` result.
//! Reuses `body` for live definitions; usage labels describe local syntax,
//! never a fabricated call graph or a binding across same-name definitions.

use serde::Serialize;

use crate::analyzer::occurrences::{
    LiteralOccurrence, MatchRole, OccurrenceResults, ScopeClassification, SymbolAnchor,
};
use crate::body::{SymbolBody, query_symbol_body};
use crate::snapshot::Snapshot;

const PREVIEW_LINES: usize = 12;
const MAX_DEFINITIONS: usize = 3;

#[derive(Serialize)]
pub(super) struct SymbolContext {
    definitions: Vec<SymbolBody>,
    total_definitions: usize,
    definitions_truncated: bool,
    /// These are the requested page's lexical sites, not resolved callers.
    uses: Vec<UseSite>,
}

#[derive(Serialize)]
struct UseSite {
    file: String,
    line: usize,
    column: usize,
    syntax: &'static str,
    context: String,
    enclosing_symbol: Option<SymbolAnchor>,
    scope: ScopeClassification,
}

impl SymbolContext {
    pub(super) fn collect(snapshot: &Snapshot, results: &OccurrenceResults) -> Self {
        let mut bodies = query_symbol_body(snapshot, &results.query, Some(PREVIEW_LINES)).bodies;
        if let Some(scope) = &results.file_scope {
            bodies.retain(|body| scope.matched_paths.contains(&body.file));
        }
        if results.generated_skipped.is_some() {
            // The occurrence scan has already applied both content/path and
            // snapshot generated fences. Reuse that admitted file set rather
            // than reintroducing a skipped definition via body enrichment.
            bodies.retain(|body| results.file_context.iter().any(|ctx| ctx.file == body.file));
        }
        let total_definitions = bodies.len();
        bodies.truncate(MAX_DEFINITIONS);
        let uses = results
            .occurrences
            .iter()
            .filter(|occ| occ.match_role != MatchRole::Definition)
            .map(|occ| UseSite {
                file: occ.file.clone(),
                line: occ.line,
                column: occ.column,
                syntax: usage_syntax(occ),
                context: occ.context.clone(),
                enclosing_symbol: occ.enclosing_symbol.clone(),
                scope: occ.scope_classification,
            })
            .collect();
        Self {
            definitions: bodies,
            total_definitions,
            definitions_truncated: total_definitions > MAX_DEFINITIONS,
            uses,
        }
    }

    pub(super) fn print(&self, results: &OccurrenceResults) {
        println!("  definitions (live source): {}", self.total_definitions);
        if self.total_definitions == 0 {
            println!("    no source definition resolved; literal hits below remain valid");
        }
        for body in &self.definitions {
            println!(
                "    {}:{}-{} [{}]{}",
                body.file,
                body.start_line,
                body.end_line,
                body.language,
                if body.truncated {
                    " (preview; truncated)"
                } else {
                    ""
                }
            );
            for (offset, line) in body.source.lines().enumerate() {
                println!("      {:>4} | {line}", body.start_line + offset);
            }
            if body.truncated {
                println!(
                    "      … use body for the remaining source (extent: {})",
                    body.extent
                );
            }
        }
        if self.definitions_truncated {
            println!(
                "    … {} more matching definitions; qualify body with --file",
                self.total_definitions - self.definitions.len()
            );
        }
        // Retain the existing identity rollup and test/docs collapse when
        // actual non-definition sites have resolved identities.
        let grouped = results.occurrences.iter().any(|occ| {
            occ.match_role != MatchRole::Definition && occ.resolved_definition.is_some()
        }) && super::print_grouped_summary(&results.occurrences);
        println!("  uses (lexical sites on this page, not a call graph):");
        let mut collapsed = 0;
        for site in &self.uses {
            if grouped
                && matches!(
                    site.scope,
                    ScopeClassification::Test | ScopeClassification::Docs
                )
            {
                collapsed += 1;
                continue;
            }
            let enclosing = site
                .enclosing_symbol
                .as_ref()
                .filter(|symbol| symbol.kind != "file")
                .map(|symbol| format!(" in {}", symbol.name))
                .unwrap_or_default();
            println!(
                "    {}:{}:{} [{}]{}",
                site.file, site.line, site.column, site.syntax, enclosing
            );
            println!("      {}", site.context.trim());
        }
        if self.uses.is_empty() {
            println!("    none on this page");
        }
        if collapsed > 0 {
            println!(
                "    ({collapsed} test/docs row(s) collapsed into the entity rollup above — find --literal prints every row)"
            );
        }
        super::print_file_rollup(results);
        super::print_page(results);
        super::print_role_summary(results);
        if !results.file_context.is_empty() {
            println!("  file impact (file importers, not symbol users):");
            for ctx in results.file_context.iter().take(3) {
                let fan_in = results
                    .occurrences
                    .iter()
                    .find(|occ| occ.file == ctx.file)
                    .and_then(|occ| occ.enclosing_facts.as_ref())
                    .map(|facts| facts.fan_in);
                println!(
                    "    {}: file fan-in {}, {} dependency path(s) in the bounded list{}",
                    ctx.file,
                    fan_in
                        .map(|count| count.to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    ctx.imports.len(),
                    if ctx.truncated {
                        " (bounded lists)"
                    } else {
                        ""
                    }
                );
                for importer in ctx.imported_by.iter().take(3) {
                    println!("      <- {importer}");
                }
                if ctx.imported_by.len() > 3 {
                    println!(
                        "      … {} more importers in the bounded list; slice shows the file neighborhood",
                        ctx.imported_by.len() - 3
                    );
                }
            }
            if results.file_context.len() > 3 {
                println!("    … {} more hit files", results.file_context.len() - 3);
            }
        }
        println!("  coverage: {}", results.coverage_line);
        super::print_file_scope(results);
        println!("  suggested next:");
        let quoted_query = shell_quote(&results.query);
        println!("    loct body {quoted_query} — complete definition source / disambiguation");
        if let Some(body) = self.definitions.first() {
            println!(
                "    loct slice {} — dependencies and consumers of the defining file",
                shell_quote(&body.file)
            );
        } else if let Some(site) = self.uses.first() {
            println!(
                "    loct slice {} — inspect the hit file",
                shell_quote(&site.file)
            );
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn usage_syntax(occ: &LiteralOccurrence) -> &'static str {
    if occ.match_role != MatchRole::Reference || !occ.file.ends_with(".rs") {
        return occ.match_role.as_str();
    }
    // Only classify when the full-line coordinate agrees with the excerpt.
    // Long/windowed lines safely remain generic references.
    let chars: Vec<char> = occ.context.chars().collect();
    let start = occ.column.saturating_sub(1);
    let end = start.saturating_add(occ.matched_text.chars().count());
    if end > chars.len() || chars[start..end].iter().collect::<String>() != occ.matched_text {
        return "reference";
    }
    let prefix: String = chars[..start].iter().collect();
    let suffix: String = chars[end..].iter().collect();
    let prefix = prefix.trim_end();
    if prefix.ends_with("->") {
        "return_type"
    } else if prefix.ends_with(':') && !prefix.ends_with("::") {
        "type_annotation"
    } else if suffix.trim_start().starts_with('(') {
        "call_expression"
    } else if suffix.trim_start().starts_with('{')
        && (prefix.is_empty()
            || prefix.ends_with('=')
            || prefix.ends_with('(')
            || prefix.ends_with("::")
            || prefix.split_whitespace().last() == Some("return"))
    {
        "construction_expression"
    } else {
        "reference"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::occurrences::scan_files;

    #[test]
    fn rust_usage_labels_separate_return_construction_and_annotation_from_calls() {
        let source = "fn make() -> Request {\n    Request { value: 1 }\n}\nfn send(stop: Request) {}\nfn read() { Request(); }\n// Request()\n";
        let results = scan_files([("src/lib.rs", source)], "Request");
        let labels: Vec<_> = results.occurrences.iter().map(usage_syntax).collect();
        assert_eq!(
            labels,
            [
                "return_type",
                "construction_expression",
                "type_annotation",
                "call_expression",
                "comment"
            ]
        );
    }

    #[test]
    fn impl_and_condition_braces_are_not_struct_construction() {
        let results = scan_files(
            [(
                "src/lib.rs",
                "impl Request {\n}\nif Request {\n}\nmatch Request {\n}\n",
            )],
            "Request",
        );
        assert!(
            results
                .occurrences
                .iter()
                .all(|occ| usage_syntax(occ) == "reference")
        );
    }
}
