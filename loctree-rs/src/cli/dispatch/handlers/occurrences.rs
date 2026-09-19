//! `loct occurrences <query>` handler — literal exact query scan.
//!
//! Loads (or creates) the snapshot, enumerates its files, reads each file's
//! raw bytes, and reports every literal occurrence of the queried text.
//! Identifier-like queries stay token-boundary aware; phrase/punctuation queries
//! behave as fixed strings. Primary matches are never AST/fuzzy hits. Zero-hit
//! output may add separate symbol-table near-match hints, never evidence.
//!
//! `--regex` switches the same command to pattern evaluation over raw file
//! text, sharing the engine, coverage line and paging with `find --regex`.

use std::path::{Path, PathBuf};

use super::super::super::command::{FindOptions, OccurrencesOptions};
use super::super::{DispatchResult, GlobalOptions, load_or_create_query_snapshot_for_roots};
use crate::analyzer::occurrences::{
    FileScope, FileScopeResolution, LiteralOccurrence, MatchMode, OccurrenceResults, ReportOptions,
    ScanOptions, attach_near_matches, enrich_with_snapshot, expand_literal_patterns,
    scan_files_multi_literal, scan_files_with, scan_files_with_regex,
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

    if opts.regex {
        return handle_occurrences_regex(opts, &roots, global);
    }

    let patterns = expand_literal_patterns(std::slice::from_ref(&opts.ident));
    if patterns.is_empty() {
        eprintln!(
            "[loct][error] 'occurrences' requires an identifier. Usage: loct occurrences <ident>"
        );
        return DispatchResult::Exit(1);
    }

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
    let mut results = scan_multi_literal(
        &borrowed,
        &patterns,
        ScanOptions {
            whole_token: opts.whole_token,
        },
        FileScope::default(),
    );
    enrich_with_snapshot(&mut results, &snapshot);
    if results.total == 0 {
        for pattern in &patterns {
            let mut probe = scan_files_with(
                std::iter::empty::<(&str, &str)>(),
                pattern,
                ScanOptions {
                    whole_token: opts.whole_token,
                },
            );
            attach_near_matches(&mut probe, &snapshot.files);
            results.near_matches.extend(probe.near_matches);
        }
        results
            .near_matches
            .sort_by(|a, b| a.symbol.cmp(&b.symbol).then(a.file.cmp(&b.file)));
        results
            .near_matches
            .dedup_by(|a, b| a.symbol == b.symbol && a.file == b.file);
    }
    results.declare_snapshot_universe(&snapshot, FileScope::default());
    results.apply_report(ReportOptions {
        group_by_file: opts.group_by_file,
        count_only: opts.count_only,
        offset: opts.offset,
        limit: opts.limit,
    });

    let zero_hit_ctx = ZeroHitCtx {
        project_root: Some(base.as_path()),
        snapshot_stale: snapshot.is_stale(&base),
    };
    if global.json {
        match serde_json::to_string_pretty(&results) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize results: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else {
        print_human(&results, opts.compact, zero_hit_ctx);
    }

    DispatchResult::Exit(0)
}

/// Handle `loct occurrences <pattern> --regex` — pattern mode of the literal
/// oracle.
///
/// Flag parity with `find --regex`, not a second implementation: same
/// `scan_files_with_regex` engine, same snapshot universe, same coverage line,
/// same `--limit/--offset` paging. Before this existed, an agent that had just
/// run `loct find --regex 'PAT'` and reached for `loct occurrences 'PAT'
/// --regex` to cross-check it got "Unknown option" — which on the surface
/// whose whole job is confirming the other one is indistinguishable from the
/// tool disagreeing.
///
/// Like `find --regex`, this deliberately skips `enrich_with_snapshot`: a
/// pattern is not a symbol name, so resolving it against the symbol table would
/// invent structure the match never had.
fn handle_occurrences_regex(
    opts: &OccurrencesOptions,
    roots: &[PathBuf],
    global: &GlobalOptions,
) -> DispatchResult {
    let pattern = opts.ident.trim();
    if pattern.is_empty() {
        eprintln!(
            "[loct][error] 'occurrences --regex' requires a pattern. Usage: loct occurrences <pattern> --regex"
        );
        return DispatchResult::Exit(1);
    }
    let re = match regex::Regex::new(pattern) {
        Ok(re) => re,
        Err(e) => {
            // Loud by design, exactly as in `find --regex`: a malformed pattern
            // must never pass as a trustworthy "0 matches".
            eprintln!("[loct][error] invalid --regex pattern '{}': {}", pattern, e);
            return DispatchResult::Exit(1);
        }
    };

    let query_global = query_global_options(global);
    let snapshot = match load_or_create_query_snapshot_for_roots(roots, &query_global) {
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
    let mut results = scan_files_with_regex(borrowed, &re, FileScope::default());
    results.declare_snapshot_universe(&snapshot, FileScope::default());
    results.apply_report(ReportOptions {
        group_by_file: opts.group_by_file,
        count_only: opts.count_only,
        offset: opts.offset,
        limit: opts.limit,
    });

    let zero_hit_ctx = ZeroHitCtx {
        project_root: Some(base.as_path()),
        snapshot_stale: snapshot.is_stale(&base),
    };
    if global.json {
        match serde_json::to_string_pretty(&results) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize results: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else {
        print_human(&results, opts.compact, zero_hit_ctx);
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
///
/// Multi-pattern OR (agent anti-grep surface):
/// - `loct find A B` → exact OR of A and B
/// - `loct find 'A|B'` → same, when every segment is a simple literal
///   (not regex). This prevents the silent fixed_string-0 trap on pipes.
pub fn handle_find_literal_command(opts: &FindOptions, global: &GlobalOptions) -> DispatchResult {
    let patterns = literal_find_patterns(opts);
    if patterns.is_empty() {
        eprintln!(
            "[loct][error] 'find --literal' requires a query. Usage: loct find --literal <query> [query...]"
        );
        return DispatchResult::Exit(1);
    }
    let display_query = patterns.join("|");

    let roots = opts.scan_roots();
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
    let file_scope = opts.file.as_deref().map(|requested| {
        FileScopeResolution::resolve(
            requested,
            snapshot.files.iter().map(|file| file.path.as_str()),
        )
    });
    let scan_scope = file_scope
        .as_ref()
        .map(FileScopeResolution::scan_scope)
        .unwrap_or_default();

    // PRIMARY: literal truth layer (identical substrate to `loct occurrences`).
    // Multi-pattern expands agent `A|B` / multi-arg into exact-literal union.
    let mut literal_matches = scan_multi_literal(
        &borrowed,
        &patterns,
        ScanOptions {
            whole_token: opts.whole_token,
        },
        scan_scope,
    );
    enrich_with_snapshot(&mut literal_matches, &snapshot);
    if literal_matches.total == 0 {
        for pattern in &patterns {
            let mut probe = scan_files_with(
                std::iter::empty::<(&str, &str)>(),
                pattern,
                ScanOptions {
                    whole_token: opts.whole_token,
                },
            );
            attach_near_matches(&mut probe, &snapshot.files);
            literal_matches.near_matches.extend(probe.near_matches);
        }
        literal_matches
            .near_matches
            .sort_by(|a, b| a.symbol.cmp(&b.symbol).then(a.file.cmp(&b.file)));
        literal_matches
            .near_matches
            .dedup_by(|a, b| a.symbol == b.symbol && a.file == b.file);
    }
    literal_matches.declare_snapshot_universe(&snapshot, scan_scope);
    let file_scope_resolved = file_scope.as_ref().is_none_or(|scope| scope.resolved);
    let file_scoped = file_scope.as_ref().is_some_and(|scope| scope.resolved);
    literal_matches.file_scope = file_scope;
    literal_matches.apply_report(ReportOptions {
        group_by_file: opts.group_by_file,
        count_only: opts.count_only,
        offset: opts.offset,
        limit: opts.limit,
    });

    // SECONDARY (strictly separate): fuzzy name-similarity hints, labeled
    // `source: "fuzzy"`. Never merged into `literal_matches`.
    let fuzzy_suggestions = if patterns.len() == 1 {
        literal_fuzzy_suggestions(patterns[0].as_str(), &snapshot.files)
    } else {
        let mut all = Vec::new();
        for pattern in &patterns {
            all.extend(literal_fuzzy_suggestions(pattern, &snapshot.files));
        }
        all
    };

    // Multi-literal expansion of `A|B` is exact-string OR, not regex evaluation.
    // Absence for the scanned universe is trustworthy for each simple pattern
    // (and their union). Absolute whole-repo absence is not, while exclusion
    // boundaries remain (outside_snapshot / unreadable / …).
    let multi = patterns.len() > 1 || literal_matches.match_mode == MatchMode::MultiLiteral;
    let looks_like_regex = !multi && query_has_regex_metachars(display_query.as_str());
    let zero_hit_ctx = ZeroHitCtx {
        project_root: Some(base.as_path()),
        snapshot_stale: snapshot.is_stale(&base),
    };
    let absence = absence_trust(
        &literal_matches.universe,
        file_scope_resolved,
        file_scoped,
        looks_like_regex,
        literal_matches.total,
        named_zero_hit_exclusion(&literal_matches, zero_hit_ctx),
    );

    if global.json {
        let payload = serde_json::json!({
            "mode": "literal",
            "query": display_query,
            "patterns": patterns,
            "literal_matches": literal_matches,
            // Mode-stable alias. Without it the envelope key changes with the
            // mode (`literal_matches` here, `regex_matches` under --regex), so a
            // parser written against one silently yields zero occurrences on the
            // other — it reads as "not found" rather than "wrong key", which is
            // the exact failure shape the coverage line exists to prevent.
            // Duplicates the payload on purpose: the mode-specific key stays for
            // existing consumers (loctree-lsp and loctree-mcp both read it).
            "matches": literal_matches,
            "fuzzy_suggestions": fuzzy_suggestions,
            "literal_trust": {
                "query_has_regex_metachars": looks_like_regex,
                "matched_as_exact_string": true,
                "multi_literal": multi,
                // Scanned-universe trust (historical key). Absolute whole-repo
                // claims must also check absence_scope / exclusion_caveat.
                "absence_trustworthy": absence.for_scanned,
                "absence_trustworthy_for_scanned": absence.for_scanned,
                "absolute_absence_trustworthy": absence.absolute,
                "absence_scope": absence.scope,
                "exclusion_caveat": absence.exclusion_caveat,
                "file_scope_resolved": file_scope_resolved,
            },
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize results: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else if opts.compact {
        print_human(&literal_matches, true, zero_hit_ctx);
    } else {
        print_literal_find_human(
            display_query.as_str(),
            &literal_matches,
            &fuzzy_suggestions,
            zero_hit_ctx,
        );
    }

    DispatchResult::Exit(0)
}

/// Scan one or more exact-literal patterns and merge (engine-shared path).
fn scan_multi_literal(
    files: &[(&str, &str)],
    patterns: &[String],
    opts: ScanOptions,
    scope: FileScope<'_>,
) -> OccurrenceResults {
    scan_files_multi_literal(files, patterns, opts, scope)
}

/// Handle `loct find --regex <pattern>` — regex over raw file TEXT.
///
/// This is the mode `--literal` could never be: `--literal` is exact-string and,
/// on a query carrying regex metacharacters, can only report "matched as exact
/// string" (loctree-fail.md 2026-06-21 — the dangerous false-clean). `--regex`
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

    let roots = opts.scan_roots();
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
    let file_scope = opts.file.as_deref().map(|requested| {
        FileScopeResolution::resolve(
            requested,
            snapshot.files.iter().map(|file| file.path.as_str()),
        )
    });
    let scan_scope = file_scope
        .as_ref()
        .map(FileScopeResolution::scan_scope)
        .unwrap_or_default();

    let mut matches = scan_files_with_regex(borrowed, &re, scan_scope);
    // No enrich_with_snapshot: a regex pattern is not a symbol name, so symbol
    // resolution against it would be meaningless. Matches stay raw-text truth.
    matches.declare_snapshot_universe(&snapshot, scan_scope);
    let file_scope_resolved = file_scope.as_ref().is_none_or(|scope| scope.resolved);
    let file_scoped = file_scope.as_ref().is_some_and(|scope| scope.resolved);
    matches.file_scope = file_scope;
    matches.apply_report(ReportOptions {
        group_by_file: opts.group_by_file,
        count_only: opts.count_only,
        offset: opts.offset,
        limit: opts.limit,
    });

    let zero_hit_ctx = ZeroHitCtx {
        project_root: Some(base.as_path()),
        snapshot_stale: snapshot.is_stale(&base),
    };
    let absence = absence_trust(
        &matches.universe,
        file_scope_resolved,
        file_scoped,
        false,
        matches.total,
        named_zero_hit_exclusion(&matches, zero_hit_ctx),
    );

    if global.json {
        let payload = serde_json::json!({
            "mode": "regex",
            "query": pattern,
            "regex_matches": matches,
            // Mode-stable alias — see the literal branch above.
            "matches": matches,
            "regex_trust": {
                "pattern_compiled": true,
                // Pattern was evaluated as a pattern over the scanned universe.
                // Absolute whole-repo absence is conditional when exclusions>0.
                "absence_trustworthy": absence.for_scanned,
                "absence_trustworthy_for_scanned": absence.for_scanned,
                "absolute_absence_trustworthy": absence.absolute,
                "absence_scope": absence.scope,
                "exclusion_caveat": absence.exclusion_caveat,
                "file_scope_resolved": file_scope_resolved,
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
        print_regex_find_human(pattern.trim(), &matches, zero_hit_ctx);
    }

    DispatchResult::Exit(0)
}

/// Machine-readable absence trust for JSON surfaces (literal + regex).
struct AbsenceTrust {
    /// Trustworthy for the scanned/indexed universe (and resolved file scope).
    for_scanned: bool,
    /// Absolute whole-repo (or whole-file when file-scoped) trust.
    absolute: bool,
    /// `"absolute" | "scanned_universe" | "untrustworthy"`.
    scope: &'static str,
    exclusion_caveat: Option<String>,
}

/// Extra signals the zero-hit printer needs beyond the result payload.
#[derive(Clone, Copy, Default)]
struct ZeroHitCtx<'a> {
    project_root: Option<&'a Path>,
    snapshot_stale: bool,
}

fn absence_trust(
    universe: &crate::analyzer::occurrences::IndexedUniverse,
    file_scope_resolved: bool,
    file_scoped: bool,
    looks_like_regex_literal: bool,
    total: usize,
    named_exclusion: Option<String>,
) -> AbsenceTrust {
    let for_scanned =
        file_scope_resolved && universe.scan_complete && (total > 0 || !looks_like_regex_literal);
    // File-scoped resolved queries prove absence inside that one path.
    // Repo-wide queries cannot claim absolute absence while exclusion
    // boundaries remain (outside_snapshot always lists unindexed gaps).
    let absolute = for_scanned && (file_scoped || !universe.has_exclusion_boundary());
    let scope = if !for_scanned {
        "untrustworthy"
    } else if absolute {
        "absolute"
    } else {
        "scanned_universe"
    };
    // Named causes (unresolved / .loctignore path / stale snapshot) beat the
    // generic outside_snapshot caveat. Only surface a caveat when absolute
    // trust is withheld — otherwise agents see a contradictory pair.
    let exclusion_caveat = if absolute {
        None
    } else if named_exclusion.is_some() {
        named_exclusion
    } else {
        universe.absence_exclusion_caveat()
    };
    AbsenceTrust {
        for_scanned,
        absolute,
        scope,
        exclusion_caveat,
    }
}

fn file_scope_loctignore_hint(
    project_root: Option<&Path>,
    scope: Option<&FileScopeResolution>,
) -> Option<String> {
    let (root, scope) = (project_root?, scope?);
    if scope.resolved {
        return None;
    }
    crate::fs_utils::loctignore_exclusion_hint(root, &scope.requested)
}

/// Human zero-hit sentence that names the cause instead of a generic
/// "absence is (not) trustworthy".
fn zero_hit_absence_text(
    results: &OccurrenceResults,
    mode: AbsenceMode,
    looks_like_regex: bool,
    ctx: ZeroHitCtx<'_>,
) -> String {
    if !results.universe.scan_complete {
        return "not found — absence is NOT trustworthy because at least one indexed path could not be scanned".to_string();
    }

    let file_scope = results.file_scope.as_ref();
    let unresolved = file_scope.is_some_and(|scope| !scope.resolved);
    if unresolved {
        let requested = file_scope.map(|s| s.requested.as_str()).unwrap_or("");
        let status = file_scope.map(|s| s.status).unwrap_or("unresolved");
        if let Some(hint) = file_scope_loctignore_hint(ctx.project_root, file_scope) {
            return format!(
                "not found — file scope status={status}; excluded-by-.loctignore `{requested}` — {hint}"
            );
        }
        if ctx.snapshot_stale {
            return format!(
                "not found — file scope status={status}; requested `{requested}` is not in this snapshot, and the snapshot is stale — absence is NOT trustworthy (rescan with `loct scan`)"
            );
        }
        return format!(
            "not found — file scope status={status}; requested `{requested}` did not resolve to an indexed path — absence is NOT trustworthy"
        );
    }

    if matches!(mode, AbsenceMode::Literal) && looks_like_regex {
        return "0 exact-string matches — NOT a trustworthy absence: the query contains regex metacharacters and `--literal` matches literally, so a pattern was never evaluated. For a regex search use a pattern-aware tool.".to_string();
    }

    if ctx.snapshot_stale {
        return "not found — snapshot is stale; absence is NOT trustworthy (rescan with `loct scan`)".to_string();
    }

    let file_scoped = file_scope.is_some_and(|scope| scope.resolved);
    if !file_scoped && let Some(caveat) = results.universe.absence_exclusion_caveat() {
        let prefix = match mode {
            AbsenceMode::Regex => "not found — pattern evaluated; ",
            AbsenceMode::Literal => "not found — literal ",
        };
        return format!("{prefix}{caveat}");
    }
    match mode {
        AbsenceMode::Regex => "not found — pattern evaluated; absence is trustworthy".to_string(),
        AbsenceMode::Literal => "not found — literal absence is trustworthy".to_string(),
    }
}

/// Human render for `find --regex`. Mirrors the literal printer's structure
/// (coverage line, per-file rollup, page, per-hit role label) but labels the
/// header as regex and never prints fuzzy suggestions (there are none).
fn print_regex_find_human(pattern: &str, results: &OccurrenceResults, ctx: ZeroHitCtx<'_>) {
    println!(
        "Regex matches of /{}/ ({} in {} file(s)) [source: regex]",
        pattern, results.total, results.files_matched
    );
    if !results.coverage_line.is_empty() {
        println!("  {}", results.coverage_line);
    }
    print_file_scope(results);
    if results.total == 0 {
        print_zero_hit_absence(results, AbsenceMode::Regex, false, ctx);
        return;
    }
    print_file_rollup(results);
    print_page(results);
    print_role_summary(results);
    if results.slim {
        println!("  (match list suppressed — count_only/slim)");
        print_suggested_next(results);
        return;
    }
    print_occurrence_rows(&results.occurrences, false);
    // Only on a non-empty hit set: the zero-hit suggestion pair is worded for
    // *literal* absence ("broaden from literal absence…"), which would be a
    // lie in regex mode where the pattern really was evaluated.
    print_suggested_next(results);
}

/// Max further columns named inline before the list is elided with `…`.
const MAX_GROUPED_COLS: usize = 5;

/// Spans of consecutive equal keys as `(start_index, len)`, covering `keys` in
/// order. Pure and total: an empty input yields an empty span list.
///
/// Callers rely on the *consecutive* wording: occurrence sets are sorted by
/// `(file, line, column)`, so same-line hits are always adjacent, and a set
/// that is not sorted simply groups less — never wrongly.
fn line_group_spans<K: PartialEq>(keys: &[K]) -> Vec<(usize, usize)> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        if let Some(last) = spans.last_mut()
            && &keys[last.0] == key
        {
            last.1 += 1;
        } else {
            spans.push((i, 1));
        }
    }
    spans
}

/// The ` (+N more at cols …)` tail for hits collapsed into one printed row.
///
/// Empty when nothing was collapsed. At most [`MAX_GROUPED_COLS`] columns are
/// named; the rest become `…`. The count is always the FULL number of
/// collapsed hits — the row must never under-report how much it folded.
fn more_cols_suffix(extra_cols: &[usize]) -> String {
    if extra_cols.is_empty() {
        return String::new();
    }
    let shown = extra_cols
        .iter()
        .take(MAX_GROUPED_COLS)
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let elided = if extra_cols.len() > MAX_GROUPED_COLS {
        ",…"
    } else {
        ""
    };
    format!(" (+{} more at cols {}{})", extra_cols.len(), shown, elided)
}

/// Per-hit annotations (resolved definition, enclosing symbol, fan-in, ignore
/// flag) rendered as the row's trailing detail.
fn occurrence_detail_suffix(occ: &LiteralOccurrence, include_ignored: bool) -> String {
    let mut suffix = String::new();
    if let Some(definition) = &occ.resolved_definition {
        suffix.push_str(&format!("  => {}", definition.symbol_id));
    }
    if let Some(enclosing) = &occ.enclosing_symbol {
        suffix.push_str(&format!("  in {}", enclosing.symbol_id));
    }
    if let Some(facts) = &occ.enclosing_facts {
        suffix.push_str(&format!(
            "  [fan-in {}, {}]",
            facts.fan_in,
            if facts.exported { "exported" } else { "local" }
        ));
    }
    if include_ignored && occ.ignored {
        suffix.push_str("  [ignored: .loctignore]");
    }
    suffix
}

/// Print the hit rows, collapsing multiple hits on the SAME line into one row.
///
/// Purely presentational: `total` / `emitted` / pagination / JSON are the raw
/// counts and stay untouched. Grouping runs over whatever occurrences are in
/// the emitted page, so a page boundary splitting one line just yields two
/// grouped rows rather than a wrong count.
///
/// A decorative `# =========` banner scanned for an unanchored `=======`
/// produces ~10 hits on one line; ungrouped that is 10 near-identical rows per
/// banner, which buried the real conflict hunks in a 505-match sweep.
///
/// Rows only collapse when their trailing detail is identical too — a differing
/// resolved definition or enclosing symbol is evidence, never noise.
fn print_occurrence_rows(occurrences: &[LiteralOccurrence], include_ignored: bool) {
    let rows: Vec<(&str, usize, String)> = occurrences
        .iter()
        .map(|occ| {
            (
                occ.file.as_str(),
                occ.line,
                occurrence_detail_suffix(occ, include_ignored),
            )
        })
        .collect();
    for (start, len) in line_group_spans(&rows) {
        let occ = &occurrences[start];
        let extra: Vec<usize> = occurrences[start + 1..start + len]
            .iter()
            .map(|o| o.column)
            .collect();
        println!(
            "  {}:{}:{}  [{}]  {}{}{}",
            occ.file,
            occ.line,
            occ.column,
            occ.match_role.as_str(),
            occ.context,
            rows[start].2,
            more_cols_suffix(&extra)
        );
    }
}

/// Whether the zero-hit line is for `--regex` or default/`--literal`.
enum AbsenceMode {
    Regex,
    Literal,
}

/// Zero-hit human absence contract shared by regex and literal printers.
///
/// Absolute "absence is trustworthy" is forbidden for **repo-wide** queries
/// when the universe declares exclusion boundaries: that was a false
/// guarantee for gitignored / unindexed surfaces (e.g. generated FFI
/// bindings outside the snapshot). A resolved `--file` scope is absolute
/// for that one path.
fn print_zero_hit_absence(
    results: &OccurrenceResults,
    mode: AbsenceMode,
    looks_like_regex: bool,
    ctx: ZeroHitCtx<'_>,
) {
    println!(
        "  ({})",
        zero_hit_absence_text(results, mode, looks_like_regex, ctx)
    );
}

fn named_zero_hit_exclusion(results: &OccurrenceResults, ctx: ZeroHitCtx<'_>) -> Option<String> {
    if let Some(hint) = file_scope_loctignore_hint(ctx.project_root, results.file_scope.as_ref()) {
        let requested = results
            .file_scope
            .as_ref()
            .map(|s| s.requested.as_str())
            .unwrap_or("");
        return Some(format!("excluded-by-.loctignore `{requested}` — {hint}"));
    }
    if results
        .file_scope
        .as_ref()
        .is_some_and(|scope| !scope.resolved)
    {
        let requested = results
            .file_scope
            .as_ref()
            .map(|s| s.requested.as_str())
            .unwrap_or("");
        let status = results
            .file_scope
            .as_ref()
            .map(|s| s.status)
            .unwrap_or("unresolved");
        return Some(format!(
            "unresolved file scope status={status} requested=`{requested}`"
        ));
    }
    if ctx.snapshot_stale {
        return Some("stale snapshot".to_string());
    }
    None
}

/// Detect regex metacharacters that strongly imply the caller meant a *pattern*
/// rather than a literal string. A lone `.` is deliberately EXCLUDED: it is
/// ambiguous (IP addresses like `100.64.0.1`, filenames like `package.json`) and
/// flagging it would flood every legitimate literal query with false warnings.
/// The 2026-06-21 loctree-fail report draws exactly this line — the clean
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

/// Expand find options into one or more exact-literal patterns (multi-arg + `A|B`).
fn literal_find_patterns(opts: &FindOptions) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    if !opts.queries.is_empty() {
        raw.extend(opts.queries.iter().cloned());
    } else if let Some(q) = opts.query.clone() {
        raw.push(q);
    } else if let Some(s) = opts.symbol.clone() {
        raw.push(s);
    } else if let Some(s) = opts.similar.clone() {
        raw.push(s);
    }
    expand_literal_patterns(&raw)
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

fn print_human(results: &OccurrenceResults, compact: bool, ctx: ZeroHitCtx<'_>) {
    if compact {
        print_compact(results, ctx);
        return;
    }
    // The header names the mode the scan actually ran in. Calling a `--regex`
    // hit set "literal occurrences" would undo the very parity the flag adds.
    let regex_mode = results.source == "regex";
    println!(
        "{} of '{}' ({} in {} file(s)) [source: {}]",
        if regex_mode {
            "Regex occurrences"
        } else {
            "Literal occurrences"
        },
        results.query,
        results.total,
        results.files_matched,
        results.source
    );
    if !results.coverage_line.is_empty() {
        println!("  {}", results.coverage_line);
    }
    print_file_scope(results);
    if results.total == 0 {
        if regex_mode {
            // A compiled pattern that matched nothing is a trustworthy absence;
            // the literal wording ("no exact occurrences") would understate it.
            print_zero_hit_absence(results, AbsenceMode::Regex, false, ctx);
        } else if results.file_scope.is_some() {
            print_zero_hit_absence(results, AbsenceMode::Literal, false, ctx);
        } else {
            print_no_exact_occurrences(results, "  ");
        }
        print_suggested_next(results);
        return;
    }
    print_file_rollup(results);
    print_page(results);
    print_role_summary(results);
    print_hit_shape(results);
    print_file_context(results);
    if results.slim {
        println!("  (occurrence list suppressed — count_only/slim)");
        print_suggested_next(results);
        return;
    }
    print_occurrence_rows(&results.occurrences, false);
    print_suggested_next(results);
}

fn print_compact(results: &OccurrenceResults, ctx: ZeroHitCtx<'_>) {
    print_file_scope(results);
    if results.total == 0 {
        if results.file_scope.is_some() {
            print_zero_hit_absence(results, AbsenceMode::Literal, false, ctx);
        } else {
            print_no_exact_occurrences(results, "");
        }
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

fn print_file_scope(results: &OccurrenceResults) {
    let Some(scope) = &results.file_scope else {
        return;
    };
    let matches = if scope.matched_paths.is_empty() {
        "none".to_string()
    } else {
        scope.matched_paths.join(", ")
    };
    println!(
        "  file scope: requested='{}', status={}, resolved={}, indexed={}, matched={}",
        scope.requested, scope.status, scope.resolved, scope.indexed, matches
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
fn print_literal_find_human(
    query: &str,
    literal: &OccurrenceResults,
    fuzzy: &[FuzzySuggestion],
    ctx: ZeroHitCtx<'_>,
) {
    let looks_like_regex = query_has_regex_metachars(query);
    println!(
        "=== Literal Matches ({} in {} file(s)) [source: {}] ===",
        literal.total, literal.files_matched, literal.source
    );
    if !literal.coverage_line.is_empty() {
        println!("  {}", literal.coverage_line);
    }
    print_file_scope(literal);
    if literal.total == 0 {
        print_zero_hit_absence(literal, AbsenceMode::Literal, looks_like_regex, ctx);
    } else {
        if looks_like_regex {
            println!(
                "  (note: matched as an exact string; `--literal` does not evaluate regex metacharacters)"
            );
        }
        print_file_rollup(literal);
        print_page(literal);
        print_role_summary(literal);
        print_hit_shape(literal);
        print_file_context(literal);
        if literal.slim {
            println!("  (occurrence list suppressed — count_only/slim)");
        } else {
            print_occurrence_rows(&literal.occurrences, true);
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

/// Print the hit-set shape (single_writer / read_only / …) when present.
fn print_hit_shape(results: &OccurrenceResults) {
    let Some(shape) = &results.hit_shape else {
        return;
    };
    print!(
        "  shape: {}  (defs={}, writes={}, reads={}; authority={})",
        shape.label, shape.definitions, shape.writers, shape.readers, shape.authority
    );
    if let Some(note) = &shape.note {
        print!(" — {note}");
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
    use super::{
        AbsenceMode, ZeroHitCtx, line_group_spans, more_cols_suffix, query_has_regex_metachars,
        zero_hit_absence_text,
    };
    use crate::analyzer::occurrences::{FileScopeResolution, ScanOptions, scan_files_with_scope};

    #[test]
    fn banner_line_hits_collapse_into_one_row() {
        // The 505-match sweep: an unanchored `=======` matched every 7th column
        // of a decorative `# =========` banner, so ONE line produced 10 rows.
        // Grouping renders it as one row plus an honest `+9 more` tail.
        let banner_cols: Vec<usize> = (0..10).map(|i| 3 + i * 7).collect();
        let keys: Vec<(&str, usize, String)> = banner_cols
            .iter()
            .map(|_| ("docs/banner.md", 5, String::new()))
            .collect();

        let spans = line_group_spans(&keys);
        assert_eq!(spans, vec![(0, 10)], "one line must be one group");

        let tail = more_cols_suffix(&banner_cols[1..]);
        assert_eq!(tail, " (+9 more at cols 10,17,24,31,38,…)");
    }

    #[test]
    fn line_group_spans_splits_on_line_file_and_detail_changes() {
        // Different lines never merge…
        let keys = [
            ("a.rs", 1, String::new()),
            ("a.rs", 1, String::new()),
            ("a.rs", 2, String::new()),
            ("b.rs", 2, String::new()),
        ];
        assert_eq!(line_group_spans(&keys), vec![(0, 2), (2, 1), (3, 1)]);

        // …and neither do same-line hits whose trailing detail differs: a
        // differing resolved definition is evidence, not noise.
        let mixed = [
            ("a.rs", 7, "  => mod::Alpha".to_string()),
            ("a.rs", 7, "  => mod::Beta".to_string()),
        ];
        assert_eq!(line_group_spans(&mixed), vec![(0, 1), (1, 1)]);

        let empty: [(&str, usize, String); 0] = [];
        assert!(line_group_spans(&empty).is_empty());
    }

    #[test]
    fn more_cols_suffix_reports_full_count_even_when_columns_are_elided() {
        assert_eq!(more_cols_suffix(&[]), "");
        assert_eq!(more_cols_suffix(&[12]), " (+1 more at cols 12)");
        // Exactly the cap: every column named, no ellipsis.
        assert_eq!(
            more_cols_suffix(&[1, 2, 3, 4, 5]),
            " (+5 more at cols 1,2,3,4,5)"
        );
        // Over the cap: count stays truthful, the list is what gets cut.
        assert_eq!(
            more_cols_suffix(&[1, 2, 3, 4, 5, 6, 7]),
            " (+7 more at cols 1,2,3,4,5,…)"
        );
    }

    #[test]
    fn regex_metachars_flag_pattern_queries_but_not_plain_literals() {
        // Regression for the 2026-06-21 loctree-fail report: `--literal` must not
        // claim a "trustworthy absence" for a query it could only exact-match.
        // Pattern-shaped queries (the dangerous false-clean case) must flag true.
        for pattern in [
            r"100\.[0-9]+\.[0-9]+",
            r"/Users/[^/]+/",
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

    #[test]
    fn w1_01_zero_hit_names_loctignore_exclusion() {
        // G-LITERAL-SCOPE-BLIND / G-DOCS-IGNORE: a zero-hit on a path that
        // exists but is parked by .loctignore must name that path, not hide
        // behind generic outside_snapshot / "absence is trustworthy".
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("ignored_w1_01")).expect("mkdir ignored");
        std::fs::write(
            root.join("ignored_w1_01/secret.rs"),
            "TOKEN_W1_01_IGNORED\n",
        )
        .expect("write ignored");
        std::fs::create_dir_all(root.join("src")).expect("mkdir src");
        std::fs::write(root.join("src/lib.rs"), "fn main() {}\n").expect("write src");
        std::fs::write(root.join(".loctignore"), "ignored_w1_01/\n").expect("loctignore");

        let ignored_scope = FileScopeResolution::resolve("ignored_w1_01", ["src/lib.rs"]);
        assert!(!ignored_scope.resolved);
        assert_eq!(ignored_scope.status, "unresolved");

        let mut results = scan_files_with_scope(
            [("src/lib.rs", "fn main() {}\n")],
            "TOKEN_W1_01_IGNORED",
            ScanOptions::default(),
            ignored_scope.scan_scope(),
        );
        results.file_scope = Some(ignored_scope);

        let text = zero_hit_absence_text(
            &results,
            AbsenceMode::Literal,
            false,
            ZeroHitCtx {
                project_root: Some(root),
                snapshot_stale: false,
            },
        );
        assert!(
            text.contains("excluded-by-.loctignore"),
            "must name .loctignore exclusion, got: {text}"
        );
        assert!(
            text.contains("`ignored_w1_01`"),
            "must name the excluded path, got: {text}"
        );
        assert!(
            !text.contains("outside_snapshot"),
            "must not fall back to generic outside_snapshot, got: {text}"
        );

        let missing = FileScopeResolution::resolve("no/such/dir", ["src/lib.rs"]);
        results.file_scope = Some(missing);
        let unresolved_text = zero_hit_absence_text(
            &results,
            AbsenceMode::Literal,
            false,
            ZeroHitCtx {
                project_root: Some(root),
                snapshot_stale: false,
            },
        );
        assert!(
            unresolved_text.contains("unresolved"),
            "missing scope must name unresolved, not 0 of 0, got: {unresolved_text}"
        );
        assert!(
            unresolved_text.contains("`no/such/dir`"),
            "must name the requested path, got: {unresolved_text}"
        );

        let stale_text = zero_hit_absence_text(
            &results,
            AbsenceMode::Literal,
            false,
            ZeroHitCtx {
                project_root: Some(root),
                snapshot_stale: true,
            },
        );
        assert!(
            stale_text.contains("stale"),
            "stale snapshot must be named, got: {stale_text}"
        );
    }
}
