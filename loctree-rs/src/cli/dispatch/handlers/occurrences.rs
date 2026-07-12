//! `loct occurrences <query>` handler — literal exact query scan.
//!
//! Loads (or creates) the snapshot, enumerates its files, reads each file's
//! raw bytes, and reports every literal occurrence of the queried text.
//! Identifier-like queries stay token-boundary aware; phrase/punctuation queries
//! behave as fixed strings. Primary matches are never AST/fuzzy hits. Zero-hit
//! output may add separate symbol-table near-match hints, never evidence.

use std::path::{Path, PathBuf};

use super::super::super::command::{FindOptions, OccurrencesOptions};
use super::super::{DispatchResult, GlobalOptions, load_or_create_query_snapshot_for_roots};
use crate::analyzer::occurrences::{
    FileScope, OccurrenceResults, ReportOptions, ScanOptions, attach_near_matches,
    enrich_with_snapshot, scan_files_with, scan_files_with_regex, scan_files_with_scope,
};
use crate::analyzer::search::{FuzzySuggestion, literal_fuzzy_suggestions};
use crate::snapshot::Snapshot;

/// Handle the `occurrences` command directly (does not go through ParsedArgs).
pub fn handle_occurrences_command(
    opts: &OccurrencesOptions,
    global: &GlobalOptions,
) -> DispatchResult {
    let roots: Vec<PathBuf> = if opts.roots.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        opts.roots.clone()
    };

    let query_global = query_global_options(global);
    let snapshot = match load_or_create_query_snapshot_for_roots(&roots, &query_global) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[loct][error] {}", e);
            return DispatchResult::Exit(1);
        }
    };

    let base = roots.first().cloned().unwrap_or_else(|| PathBuf::from("."));
    let contents = read_snapshot_contents(&snapshot, &base);
    let borrowed = contents
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect::<Vec<_>>();
    let mut results = scan_files_with(
        borrowed,
        opts.ident.trim(),
        ScanOptions {
            whole_token: opts.whole_token,
        },
    );
    enrich_with_snapshot(&mut results, &snapshot);
    attach_near_matches(&mut results, &snapshot.files);
    results.apply_report(ReportOptions {
        group_by_file: opts.group_by_file,
        count_only: opts.count_only,
        offset: opts.offset,
        limit: opts.limit,
    });

    if global.json {
        match serde_json::to_string_pretty(&results) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize results: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else {
        print_human(&results, opts.compact);
    }

    DispatchResult::Exit(0)
}

/// Handle `loct find --literal <query>` — literal truth mode of `find`.
///
/// Built directly on the W1-A occurrences substrate so its primary results are
/// byte-for-byte identical to `loct occurrences`. Fuzzy name-similarity
/// suggestions are computed separately and returned in their own labeled
/// section; they are NEVER promoted into the literal matches. This is what lets
/// an agent trust `--literal` absence: when the mode says literal, the answer
/// is literal, and suggestions stay behind the glass.
pub fn handle_find_literal_command(opts: &FindOptions, global: &GlobalOptions) -> DispatchResult {
    // Resolve the single literal query to scan for. `find` always scopes to `.`.
    let ident = literal_find_ident(opts);
    let ident = match ident {
        Some(id) if !id.trim().is_empty() => id,
        _ => {
            eprintln!(
                "[loct][error] 'find --literal' requires a query. Usage: loct find --literal <query>"
            );
            return DispatchResult::Exit(1);
        }
    };

    let roots = vec![PathBuf::from(".")];
    let query_global = query_global_options(global);
    let snapshot = match load_or_create_query_snapshot_for_roots(&roots, &query_global) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[loct][error] {}", e);
            return DispatchResult::Exit(1);
        }
    };

    let base = roots.first().cloned().unwrap_or_else(|| PathBuf::from("."));
    let contents = read_snapshot_contents(&snapshot, &base);
    let borrowed = contents
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect::<Vec<_>>();

    // PRIMARY: literal truth layer (identical substrate to `loct occurrences`).
    let mut literal_matches = scan_files_with_scope(
        borrowed,
        ident.trim(),
        ScanOptions {
            whole_token: opts.whole_token,
        },
        FileScope {
            file: opts.file.as_deref(),
        },
    );
    enrich_with_snapshot(&mut literal_matches, &snapshot);
    attach_near_matches(&mut literal_matches, &snapshot.files);
    literal_matches.apply_report(ReportOptions {
        group_by_file: opts.group_by_file,
        count_only: opts.count_only,
        offset: opts.offset,
        limit: opts.limit,
    });

    // SECONDARY (strictly separate): fuzzy name-similarity hints, labeled
    // `source: "fuzzy"`. Never merged into `literal_matches`.
    let fuzzy_suggestions = literal_fuzzy_suggestions(ident.trim(), &snapshot.files);

    // A query carrying regex metacharacters could not have been evaluated as a
    // pattern by literal (exact-string) mode. Surfacing this is critical for
    // security/privacy audits: without it, `--literal` hands a confident "absence
    // is trustworthy" clean-bill for a query it never actually pattern-matched.
    let looks_like_regex = query_has_regex_metachars(ident.trim());
    let absence_trustworthy = literal_matches.total > 0 || !looks_like_regex;

    if global.json {
        let payload = serde_json::json!({
            "mode": "literal",
            "query": ident,
            "literal_matches": literal_matches,
            "fuzzy_suggestions": fuzzy_suggestions,
            "literal_trust": {
                "query_has_regex_metachars": looks_like_regex,
                "matched_as_exact_string": true,
                "absence_trustworthy": absence_trustworthy,
            },
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize results: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else {
        print_literal_find_human(ident.trim(), &literal_matches, &fuzzy_suggestions);
    }

    DispatchResult::Exit(0)
}

/// Handle `loct find --regex <pattern>` — regex over raw file TEXT.
///
/// This is the mode `--literal` could never be: `--literal` is exact-string and,
/// on a query carrying regex metacharacters, can only report "matched as exact
/// string" (loctree-feedback.md 2026-06-21 — the dangerous false-clean). `--regex`
/// actually compiles and evaluates the pattern, so a clean result is genuinely
/// trustworthy. It keeps loct's artifact-fence coverage accounting and per-hit
/// context labels (comment / string_literal / code) that the grep/sed fallback
/// cannot give — exactly where verification trust matters most.
pub fn handle_find_regex_command(opts: &FindOptions, global: &GlobalOptions) -> DispatchResult {
    let pattern = literal_find_ident(opts);
    let pattern = match pattern {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            eprintln!(
                "[loct][error] 'find --regex' requires a pattern. Usage: loct find --regex <pattern>"
            );
            return DispatchResult::Exit(1);
        }
    };

    let re = match regex::Regex::new(pattern.trim()) {
        Ok(re) => re,
        Err(e) => {
            // A failed compile is loud by design: never let a malformed pattern
            // pass as a trustworthy "0 matches".
            eprintln!(
                "[loct][error] invalid --regex pattern '{}': {}",
                pattern.trim(),
                e
            );
            return DispatchResult::Exit(1);
        }
    };

    let roots = vec![PathBuf::from(".")];
    let query_global = query_global_options(global);
    let snapshot = match load_or_create_query_snapshot_for_roots(&roots, &query_global) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[loct][error] {}", e);
            return DispatchResult::Exit(1);
        }
    };

    let base = roots.first().cloned().unwrap_or_else(|| PathBuf::from("."));
    let contents = read_snapshot_contents(&snapshot, &base);
    let borrowed = contents
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect::<Vec<_>>();

    let mut matches = scan_files_with_regex(
        borrowed,
        &re,
        FileScope {
            file: opts.file.as_deref(),
        },
    );
    // No enrich_with_snapshot: a regex pattern is not a symbol name, so symbol
    // resolution against it would be meaningless. Matches stay raw-text truth.
    matches.apply_report(ReportOptions {
        group_by_file: opts.group_by_file,
        count_only: opts.count_only,
        offset: opts.offset,
        limit: opts.limit,
    });

    if global.json {
        let payload = serde_json::json!({
            "mode": "regex",
            "query": pattern,
            "regex_matches": matches,
            "regex_trust": {
                "pattern_compiled": true,
                // The pattern WAS evaluated as a pattern, so unlike --literal a
                // clean result here is a trustworthy absence.
                "absence_trustworthy": true,
            },
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize results: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else {
        print_regex_find_human(pattern.trim(), &matches);
    }

    DispatchResult::Exit(0)
}

/// Human render for `find --regex`. Mirrors the literal printer's structure
/// (coverage line, per-file rollup, page, per-hit role label) but labels the
/// header as regex and never prints fuzzy suggestions (there are none).
fn print_regex_find_human(pattern: &str, results: &OccurrenceResults) {
    println!(
        "Regex matches of /{}/ ({} in {} file(s)) [source: regex]",
        pattern, results.total, results.files_matched
    );
    if !results.coverage_line.is_empty() {
        println!("  {}", results.coverage_line);
    }
    if results.total == 0 {
        println!("  (not found — pattern evaluated; absence is trustworthy)");
        return;
    }
    print_file_rollup(results);
    print_page(results);
    print_role_summary(results);
    if results.slim {
        println!("  (match list suppressed — count_only/slim)");
        return;
    }
    for occ in &results.occurrences {
        println!(
            "  {}:{}:{}  [{}]  {}",
            occ.file,
            occ.line,
            occ.column,
            occ.match_role.as_str(),
            occ.context
        );
    }
}

/// Detect regex metacharacters that strongly imply the caller meant a *pattern*
/// rather than a literal string. A lone `.` is deliberately EXCLUDED: it is
/// ambiguous (IP addresses like `100.64.0.1`, filenames like `package.json`) and
/// flagging it would flood every legitimate literal query with false warnings.
/// The 2026-06-21 loctree-feedback report draws exactly this line — the clean
/// `100.64.0.1` (dots only) versus the dangerous `100\.[0-9]+\.[0-9]+`
/// (backslash, character class, quantifier).
fn query_has_regex_metachars(query: &str) -> bool {
    query.chars().any(|c| {
        matches!(
            c,
            '\\' | '[' | ']' | '(' | ')' | '{' | '}' | '+' | '*' | '?' | '^' | '$' | '|'
        )
    })
}

fn query_global_options(global: &GlobalOptions) -> GlobalOptions {
    let mut scoped = global.clone();
    if !scoped.verbose {
        scoped.quiet = true;
    }
    scoped
}

/// Resolve the query for `find --literal` from a bare positional query,
/// `--symbol`, or the legacy `query` field. Literal mode takes exactly one.
fn literal_find_ident(opts: &FindOptions) -> Option<String> {
    opts.query
        .clone()
        .or_else(|| opts.queries.first().cloned())
        .or_else(|| opts.symbol.clone())
        .or_else(|| opts.similar.clone())
}

/// Read every snapshot file's content (best-effort: skip unreadable files
/// silently — a binary/deleted file is simply not a literal match site).
///
/// Shared by `occurrences` and `find --literal` so both scan the exact same
/// bytes from the exact same file set — the contract that keeps their literal
/// results identical.
fn read_snapshot_contents(snapshot: &Snapshot, base: &Path) -> Vec<(String, String)> {
    let mut contents: Vec<(String, String)> = Vec::new();
    for file in &snapshot.files {
        let resolved = resolve_path(base, &file.path);
        if let Ok(text) = std::fs::read_to_string(&resolved) {
            contents.push((file.path.clone(), text));
        }
    }
    contents
}

/// Resolve a snapshot-relative path against the scan root. Falls back to the
/// raw path if joining does not yield an existing file (e.g. already absolute).
fn resolve_path(base: &Path, rel: &str) -> PathBuf {
    let joined = base.join(rel);
    if joined.exists() {
        return joined;
    }
    let raw = PathBuf::from(rel);
    if raw.exists() {
        return raw;
    }
    joined
}

fn print_human(results: &OccurrenceResults, compact: bool) {
    if compact {
        print_compact(results);
        return;
    }
    println!(
        "Literal occurrences of '{}' ({} in {} file(s)) [source: {}]",
        results.query, results.total, results.files_matched, results.source
    );
    if !results.coverage_line.is_empty() {
        println!("  {}", results.coverage_line);
    }
    if results.total == 0 {
        print_no_exact_occurrences(results, "  ");
        print_suggested_next(results);
        return;
    }
    print_file_rollup(results);
    print_page(results);
    print_role_summary(results);
    print_file_context(results);
    if results.slim {
        println!("  (occurrence list suppressed — count_only/slim)");
        print_suggested_next(results);
        return;
    }
    for occ in &results.occurrences {
        let mut suffix = String::new();
        if let Some(definition) = &occ.resolved_definition {
            suffix.push_str(&format!("  => {}", definition.symbol_id));
        }
        if let Some(enclosing) = &occ.enclosing_symbol {
            suffix.push_str(&format!("  in {}", enclosing.symbol_id));
        }
        println!(
            "  {}:{}:{}  [{}]  {}{}",
            occ.file,
            occ.line,
            occ.column,
            occ.match_role.as_str(),
            occ.context,
            suffix
        );
    }
    print_suggested_next(results);
}

fn print_compact(results: &OccurrenceResults) {
    if results.total == 0 {
        print_no_exact_occurrences(results, "");
        return;
    }
    if results.slim {
        println!("occurrence list suppressed; total={}", results.total);
        return;
    }
    for occ in &results.occurrences {
        println!("{}:{} {}", occ.file, occ.line, occ.context);
    }
}

fn print_no_exact_occurrences(results: &OccurrenceResults, indent: &str) {
    if results.near_matches.is_empty() {
        println!("{}no exact occurrences of '{}'", indent, results.query);
        return;
    }
    let symbols = results
        .near_matches
        .iter()
        .map(|m| m.symbol.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "{}no exact occurrences of '{}'; near-matches: {}",
        indent, results.query, symbols
    );
}

/// Render the per-file occurrence rollup, when `group_by_file` populated it.
fn print_file_rollup(results: &OccurrenceResults) {
    if let Some(by_file) = &results.by_file {
        println!("  by file:");
        for fc in by_file {
            println!("    {:>5}  {}", fc.count, fc.file);
        }
    }
}

/// Render page metadata, when `limit`/`offset` pagination populated it.
fn print_page(results: &OccurrenceResults) {
    if let Some(page) = &results.page {
        match page.next_offset {
            Some(next) => println!(
                "  page: offset={}, limit={}, returned={}, next_offset={} (more available)",
                page.offset, page.limit, page.returned, next
            ),
            None => println!(
                "  page: offset={}, limit={}, returned={} (final page)",
                page.offset, page.limit, page.returned
            ),
        }
    }
}

/// Human output for `find --literal`: literal matches as the primary block,
/// then fuzzy suggestions in a clearly-labeled separate section that can never
/// be mistaken for evidence.
fn print_literal_find_human(query: &str, literal: &OccurrenceResults, fuzzy: &[FuzzySuggestion]) {
    let looks_like_regex = query_has_regex_metachars(query);
    println!(
        "=== Literal Matches ({} in {} file(s)) [source: {}] ===",
        literal.total, literal.files_matched, literal.source
    );
    if !literal.coverage_line.is_empty() {
        println!("  {}", literal.coverage_line);
    }
    if literal.total == 0 {
        if looks_like_regex {
            // NOT a trustworthy absence: the query carries regex metacharacters,
            // but `--literal` did an exact-string match and never evaluated it as
            // a pattern. Printing "absence is trustworthy" here would be a FALSE
            // CLEAN for a security/privacy audit.
            println!("  (0 exact-string matches — NOT a trustworthy absence: the query contains");
            println!("   regex metacharacters and `--literal` matches literally, so a pattern was");
            println!("   never evaluated. For a regex search use a pattern-aware tool.)");
        } else {
            println!("  (not found — literal absence is trustworthy)");
        }
    } else {
        if looks_like_regex {
            println!(
                "  (note: matched as an exact string; `--literal` does not evaluate regex metacharacters)"
            );
        }
        print_file_rollup(literal);
        print_page(literal);
        print_role_summary(literal);
        print_file_context(literal);
        if literal.slim {
            println!("  (occurrence list suppressed — count_only/slim)");
        } else {
            for occ in &literal.occurrences {
                let mut suffix = String::new();
                if let Some(definition) = &occ.resolved_definition {
                    suffix.push_str(&format!("  => {}", definition.symbol_id));
                }
                if let Some(enclosing) = &occ.enclosing_symbol {
                    suffix.push_str(&format!("  in {}", enclosing.symbol_id));
                }
                println!(
                    "  {}:{}:{}  [{}]  {}{}",
                    occ.file,
                    occ.line,
                    occ.column,
                    occ.match_role.as_str(),
                    occ.context,
                    suffix
                );
            }
        }
    }
    print_suggested_next(literal);

    // Fuzzy suggestions stay behind the glass: separate header, explicit
    // disclaimer, never folded into the literal block above.
    if !fuzzy.is_empty() {
        println!();
        println!(
            "=== Fuzzy Suggestions ({}) — NOT literal matches, hints only ===",
            fuzzy.len()
        );
        for s in fuzzy {
            match s.line {
                Some(line) => println!(
                    "  ~ {} (score {:.2}) in {}:{}  [source: {}]",
                    s.symbol, s.score, s.file, line, s.source
                ),
                None => println!(
                    "  ~ {} (score {:.2}) in {}  [source: {}]",
                    s.symbol, s.score, s.file, s.source
                ),
            }
        }
    }
}

fn print_suggested_next(results: &OccurrenceResults) {
    if results.suggested_next.is_empty() {
        return;
    }
    println!("  suggested next:");
    for suggestion in &results.suggested_next {
        println!("    {} - {}", suggestion.command, suggestion.reason);
    }
}

/// Render the definition-vs-callsite roll-up. One compact line so an agent sees
/// "is this mostly defined or mostly used here?" without walking every hit.
fn print_role_summary(results: &OccurrenceResults) {
    let Some(summary) = &results.role_summary else {
        return;
    };
    let mut parts = Vec::new();
    if summary.definitions > 0 {
        parts.push(format!("{} definition", summary.definitions));
    }
    if summary.callsites > 0 {
        parts.push(format!("{} callsite", summary.callsites));
    }
    if summary.imports > 0 {
        parts.push(format!("{} import", summary.imports));
    }
    if summary.non_code > 0 {
        parts.push(format!("{} non-code", summary.non_code));
    }
    if summary.other > 0 {
        parts.push(format!("{} other", summary.other));
    }
    if parts.is_empty() {
        return;
    }
    print!("  roles: {}", parts.join(", "));
    if !summary.definition_files.is_empty() {
        print!("  (defs in: {})", summary.definition_files.join(", "));
    }
    println!();
}

/// Render per-file importer/consumer context — the literal hit's blast radius.
fn print_file_context(results: &OccurrenceResults) {
    if results.file_context.is_empty() {
        return;
    }
    println!("  file context:");
    for ctx in &results.file_context {
        let mut line = format!(
            "    {} ({} hit{}, {})",
            ctx.file,
            ctx.hits,
            if ctx.hits == 1 { "" } else { "s" },
            ctx.scope_classification.as_str()
        );
        if !ctx.imported_by.is_empty() {
            line.push_str(&format!("  consumers: {}", ctx.imported_by.join(", ")));
        }
        if !ctx.imports.is_empty() {
            line.push_str(&format!("  deps: {}", ctx.imports.join(", ")));
        }
        if ctx.truncated {
            line.push_str("  (…truncated)");
        }
        println!("{}", line);
    }
}

#[cfg(test)]
mod tests {
    use super::query_has_regex_metachars;

    #[test]
    fn regex_metachars_flag_pattern_queries_but_not_plain_literals() {
        // Regression for the 2026-06-21 loctree-feedback report: `--literal` must not
        // claim a "trustworthy absence" for a query it could only exact-match.
        // Pattern-shaped queries (the dangerous false-clean case) must flag true.
        for pattern in [
            r"100\.[0-9]+\.[0-9]+",
            r"/home/[^/]+/",
            "foo|bar",
            "key.*path",
            "a+b",
            "(group)",
            "name$",
            "^anchor",
        ] {
            assert!(
                query_has_regex_metachars(pattern),
                "pattern-shaped query {pattern:?} must be flagged as regex-like"
            );
        }

        // Plain literals — including dotted IPs/filenames — must NOT flag, or the
        // warning floods every legitimate literal search. This is the exact line
        // the report drew: clean `100.64.0.1` vs dangerous `100\.[0-9]+`.
        for literal in [
            "100.64.0.1",
            "package.json",
            "run_agent_send_with_fallback",
            "BUNDLE_JUNK_EXCLUDES",
            "loctree-mcp",
            "--version",
        ] {
            assert!(
                !query_has_regex_metachars(literal),
                "plain literal {literal:?} must not be flagged as regex-like"
            );
        }
    }
}
