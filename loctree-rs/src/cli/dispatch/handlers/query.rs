//! Query-related command handlers
//!
//! Handles: query, jq_query

use super::super::super::command::{
    BodyOptions, FindOptions, JqQueryOptions, QueryKind, QueryOptions,
};
use super::super::{
    DispatchResult, GlobalOptions, load_or_create_query_snapshot_for_roots, load_or_create_snapshot,
};

const DEFAULT_WHERE_SYMBOL_LIMIT: usize = 25;

pub fn handle_find_where_symbol_command(
    opts: &FindOptions,
    global: &GlobalOptions,
) -> DispatchResult {
    use crate::query::query_where_symbol;

    let target = opts
        .query
        .clone()
        .or_else(|| opts.queries.first().cloned())
        .unwrap_or_default();

    if target.is_empty() {
        eprintln!("Error: Query cannot be empty");
        return DispatchResult::Exit(1);
    }

    let roots = opts.scan_roots();
    let query_global = query_global_options(global);
    let snapshot = match load_or_create_query_snapshot_for_roots(&roots, &query_global) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[loct][error] {}", e);
            return DispatchResult::Exit(1);
        }
    };

    let result = query_where_symbol(&snapshot, &target).bounded(if opts.all {
        None
    } else {
        Some(opts.limit.unwrap_or(DEFAULT_WHERE_SYMBOL_LIMIT))
    });

    // Output results
    if global.json {
        match serde_json::to_string_pretty(&result) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize results: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else {
        println!("where-symbol '{}':", result.target);
        if result.results.is_empty() {
            println!("  (no results)");
        } else {
            for m in &result.results {
                if let Some(line) = m.line {
                    print!("  {}:{}", m.file, line);
                } else {
                    print!("  {}", m.file);
                }
                if let Some(ref ctx) = m.context {
                    print!(" - {}", ctx);
                }
                println!();
            }
        }
        print_query_truncation(&result);
    }

    DispatchResult::Exit(0)
}

/// Handle the `body` command - bounded symbol source retrieval.
pub fn handle_body_command(opts: &BodyOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::body::query_symbol_body;

    let roots = opts
        .root
        .as_ref()
        .map(|r| vec![r.clone()])
        .unwrap_or_else(|| vec![std::path::PathBuf::from(".")]);
    let query_global = query_global_options(global);
    let snapshot = match load_or_create_query_snapshot_for_roots(&roots, &query_global) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[loct][error] {}", e);
            return DispatchResult::Exit(1);
        }
    };

    let unfiltered = query_symbol_body(&snapshot, &opts.symbol, opts.line_cap);
    let had_candidates = !unfiltered.bodies.is_empty();
    let result = unfiltered.filtered_to_file(opts.file.as_deref());

    if global.json {
        match serde_json::to_string_pretty(&result) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize results: {}", e);
                return DispatchResult::Exit(1);
            }
        }
        // A module redirect is an ANSWER, not a miss: the engine named the
        // declaration site, the module file and the symbols inside it. Exiting
        // non-zero here would tell every caller (and every verifier) that the
        // query failed while the payload is full.
        let answered = !result.bodies.is_empty() || result.module_redirect.is_some();
        return DispatchResult::Exit(if answered { 0 } else { 1 });
    }

    if result.bodies.is_empty() {
        if let Some(file) = opts.file.as_deref().filter(|_| had_candidates) {
            println!(
                "body '{}': no definition in '{}' (definitions exist elsewhere).",
                result.symbol, file
            );
            println!(
                "  hint: drop --file or run `loct body {}` to list all candidates.",
                result.symbol
            );
        } else if let Some(redirect) = &result.module_redirect {
            render_module_redirect(redirect);
            // Answered — see the JSON path above for why this is Exit(0).
            return DispatchResult::Exit(0);
        } else {
            println!("body '{}': (no source body found)", result.symbol);
            println!(
                "  hint: run `loct query where-symbol {}` to locate the symbol first.",
                result.symbol
            );
        }
        return DispatchResult::Exit(1);
    }

    if result.bodies.len() > 1 {
        println!(
            "body '{}': multiple exact definitions found; choose one:",
            result.symbol
        );
        for body in &result.bodies {
            println!(
                "  {}:{}-{} [{}]",
                body.file, body.start_line, body.end_line, body.language
            );
        }
        println!(
            "  hint: qualify with --file <path>, or use a qualified symbol, e.g. path::sym or Type::method."
        );
        return DispatchResult::Exit(1);
    }

    for body in &result.bodies {
        println!(
            "── {} [{}] {}:{}-{} ──",
            body.symbol, body.language, body.file, body.start_line, body.end_line
        );
        println!("{}", body.source);
        if body.extent == crate::body::EXTENT_WINDOW {
            println!(
                "  … extent unproven: showing a fixed {}-line window; the body's real end \
                 was not confirmed (truncated: true).",
                body.end_line - body.start_line + 1
            );
        } else if body.truncated {
            println!(
                "  … truncated: showing {} of {} lines (cap {}). Use --max-lines to widen.",
                body.end_line - body.start_line + 1,
                body.total_lines,
                body.line_cap
            );
        }
        println!();
    }

    DispatchResult::Exit(0)
}

/// Render the `body` answer for a name that turned out to be a module.
///
/// The old text was a dead end — "(no source body found)" plus a hint at
/// `where-symbol`, which itself answered nothing. This prints what the engine
/// actually knows: the declaration site, the file the module resolves to, the
/// symbols inside it that DO have bodies, and the fuzzy near-misses that
/// `find --json` was already carrying in `fuzzy_suggestions`.
fn render_module_redirect(redirect: &crate::body::ModuleRedirect) {
    use crate::body::MODULE_REDIRECT_SYMBOL_CAP;

    println!(
        "body '{}': that name is a MODULE, not a symbol with a source body.",
        redirect.module
    );
    for decl in &redirect.declarations {
        match (decl.line, decl.module_file.as_deref()) {
            (Some(line), Some(path)) => {
                println!("  declared: {}:{}  ->  {}", decl.declared_in, line, path)
            }
            (Some(line), None) => println!("  declared: {}:{}", decl.declared_in, line),
            (None, Some(path)) => println!("  declared: {}  ->  {}", decl.declared_in, path),
            (None, None) => println!("  declared: {}", decl.declared_in),
        }
    }

    if !redirect.symbols.is_empty() {
        println!("  symbols in it (these have bodies):");
        for sym in redirect.symbols.iter().take(MODULE_REDIRECT_SYMBOL_CAP) {
            match sym.line {
                Some(line) => println!("    {} [{}]  {}:{}", sym.name, sym.kind, sym.file, line),
                None => println!("    {} [{}]  {}", sym.name, sym.kind, sym.file),
            }
        }
        if redirect.symbols.len() > MODULE_REDIRECT_SYMBOL_CAP {
            println!(
                "    … {} more (loct focus on the module directory for the full list)",
                redirect.symbols.len() - MODULE_REDIRECT_SYMBOL_CAP
            );
        }
    }

    if !redirect.suggestions.is_empty() {
        let hints: Vec<String> = redirect
            .suggestions
            .iter()
            .map(|s| format!("{} ({:.2})", s.symbol, s.score))
            .collect();
        println!("  did you mean: {}", hints.join(", "));
    }

    // Best next command, in order of how much it resembles what was asked:
    // the top name-similarity hit, then the first symbol with a real body
    // (constants are `decl` — offering `loct body CERTAIN_WEIGHT` for a query
    // about `health_score` is technically valid and practically useless).
    let next = redirect
        .suggestions
        .first()
        .map(|s| s.symbol.clone())
        .or_else(|| {
            redirect
                .symbols
                .iter()
                .find(|s| s.kind != "decl")
                .or_else(|| redirect.symbols.first())
                .map(|s| s.name.clone())
        });
    if let Some(next) = next {
        println!("  hint: loct body {}", next);
    }
}

/// Handle the query command directly
pub fn handle_query_command(opts: &QueryOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::query::{
        SwiftTypeResolutionStatus, classify_swift_type_references, query_component_of,
        query_where_symbol, query_who_imports,
    };

    // Load snapshot (auto-scan if missing). Honor --root/--project from find/query.
    let roots = opts.scan_roots();
    let query_global = query_global_options(global);
    let snapshot = match load_or_create_query_snapshot_for_roots(&roots, &query_global) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[loct][error] {}", e);
            return DispatchResult::Exit(1);
        }
    };

    if matches!(opts.kind, QueryKind::SwiftTypes) {
        let source = match std::fs::read_to_string(&opts.target) {
            Ok(source) => source,
            Err(e) => {
                eprintln!("[loct][error] Failed to read {}: {}", opts.target, e);
                return DispatchResult::Exit(1);
            }
        };
        let result = classify_swift_type_references(&snapshot, &opts.target, &source);

        if global.json {
            match serde_json::to_string_pretty(&result) {
                Ok(json) => println!("{}", json),
                Err(e) => {
                    eprintln!("[loct][error] Failed to serialize results: {}", e);
                    return DispatchResult::Exit(1);
                }
            }
        } else {
            println!("swift-types '{}':", result.target);
            if result.references.is_empty() {
                println!("  (no type-position references)");
            } else {
                for reference in &result.references {
                    match reference.status {
                        SwiftTypeResolutionStatus::Resolved => {
                            if let Some(definition) = &reference.definition {
                                let line = definition
                                    .line
                                    .map(|line| format!(":{}", line))
                                    .unwrap_or_default();
                                print!(
                                    "  {}: RESOLVED -> {}{}",
                                    reference.name, definition.file, line
                                );
                                if let Some(ctx) = &definition.context {
                                    print!(" - {}", ctx);
                                }
                                println!(" (ref line {})", reference.line);
                            } else {
                                println!(
                                    "  {}: RESOLVED (ref line {})",
                                    reference.name, reference.line
                                );
                            }
                        }
                        SwiftTypeResolutionStatus::External => {
                            println!(
                                "  {}: EXTERNAL (Swift/Foundation/SwiftUI allowlist, ref line {})",
                                reference.name, reference.line
                            );
                        }
                        SwiftTypeResolutionStatus::Unresolved => {
                            let symbol_id = reference.symbol_id.as_deref().unwrap_or("");
                            println!(
                                "  {}: UNRESOLVED {} (ref line {})",
                                reference.name, symbol_id, reference.line
                            );
                        }
                    }
                }
            }
        }

        return DispatchResult::Exit(0);
    }

    // Execute the query
    let result = match opts.kind {
        QueryKind::WhoImports => query_who_imports(&snapshot, &opts.target),
        QueryKind::WhereSymbol => query_where_symbol(&snapshot, &opts.target),
        QueryKind::ComponentOf => query_component_of(&snapshot, &opts.target),
        QueryKind::SwiftTypes => unreachable!("swift-types handled before QueryResult path"),
    };
    let result = if matches!(opts.kind, QueryKind::WhereSymbol) {
        result.bounded(if opts.all {
            None
        } else {
            Some(opts.limit.unwrap_or(DEFAULT_WHERE_SYMBOL_LIMIT))
        })
    } else {
        result
    };

    // Output results
    if global.json {
        // JSON output
        match serde_json::to_string_pretty(&result) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize results: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else {
        // Human-readable output
        println!("{} '{}':", result.kind, result.target);
        if result.results.is_empty() {
            println!("  (no results)");
        } else {
            for m in &result.results {
                if let Some(line) = m.line {
                    print!("  {}:{}", m.file, line);
                } else {
                    print!("  {}", m.file);
                }
                if let Some(ref ctx) = m.context {
                    print!(" - {}", ctx);
                }
                println!();
            }
        }
        print_query_truncation(&result);
    }

    DispatchResult::Exit(0)
}

fn print_query_truncation(result: &crate::query::QueryResult) {
    println!(
        "  accounting: total={}, emitted={}, offset={}, truncated={}, has_more={}",
        result.total, result.emitted, result.offset, result.truncated, result.has_more
    );
    println!("  {}", result.universe.summary_line());
    if result.truncated {
        println!(
            "  … showing {} of {} exact definitions; qualify the symbol, pass --limit <N>, or use --all.",
            result.results.len(),
            result.total
        );
    }
}

fn query_global_options(global: &GlobalOptions) -> GlobalOptions {
    let mut scoped = global.clone();
    if !scoped.verbose {
        scoped.quiet = true;
    }
    scoped
}

/// Artifacts the jq surface can be pointed at with `--artifact <name>`.
///
/// `snapshot` is the historical default and stays first. The rest are the
/// sibling artifacts a full `loct` run already writes next to it — the place
/// where `.summary`, `.dead_parrots` and `.cycles` actually live.
pub const QUERYABLE_ARTIFACTS: &[&str] = &[
    "snapshot", "agent", "findings", "analysis", "manifest", "handlers", "dead", "circular",
];

/// Directory holding snapshot.json and its sibling artifacts for `root`.
fn artifacts_dir_for(root: &std::path::Path) -> std::path::PathBuf {
    let snapshot_path = super::inventory::resolve_snapshot_path(root);
    snapshot_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| root.to_path_buf())
}

/// Which sibling artifacts carry `key` at their top level.
///
/// Runs only on the miss path: a wrong key should not cost a scan, but once
/// the answer is "not here" the honest follow-up is "then where" — read from
/// disk, not from a hardcoded table that can rot away from the producers.
fn artifacts_carrying_key(dir: &std::path::Path, key: &str, skip: &str) -> Vec<String> {
    QUERYABLE_ARTIFACTS
        .iter()
        .filter(|name| **name != skip)
        .filter(|name| {
            let path = dir.join(format!("{name}.json"));
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
                .and_then(|value| value.as_object().map(|obj| obj.contains_key(key)))
                .unwrap_or(false)
        })
        .map(|name| (*name).to_string())
        .collect()
}

/// Report a top-level key miss instead of answering `null` in silence.
///
/// A missing top-level key is a miss, not a value: the caller asked the
/// document a question it cannot answer, and jq's `null` hides that behind a
/// legal-looking result. Deeper misses stay silent on purpose — see
/// [`crate::jaq_query::leading_top_level_key`].
fn report_top_level_miss(
    key: &str,
    doc: &serde_json::Value,
    source_label: &str,
    artifacts_dir: Option<&std::path::Path>,
    current_artifact: &str,
) {
    let available: Vec<&str> = doc
        .as_object()
        .map(|obj| obj.keys().map(String::as_str).collect())
        .unwrap_or_default();
    eprintln!("[loct][error] no key '.{key}' in {source_label}");
    eprintln!(
        "[loct][hint] available top-level keys: {}",
        available.join(", ")
    );
    let Some(dir) = artifacts_dir else {
        return;
    };
    let elsewhere = artifacts_carrying_key(dir, key, current_artifact);
    if let Some(first) = elsewhere.first() {
        eprintln!(
            "[loct][hint] '.{key}' lives in: {} — try: loct '.{key}' --artifact {first}",
            elsewhere.join(", ")
        );
    } else if !dir.join("findings.json").is_file() {
        eprintln!(
            "[loct][hint] findings artifacts are not materialized yet — run `loct` (full pass), then `loct '.dead_parrots | length' --artifact findings`"
        );
    }
}

/// Handle the jq query command - execute jaq filter on snapshot or a sibling artifact
pub fn handle_jq_query_command(opts: &JqQueryOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::jaq_query::{JaqExecutor, format_output, leading_top_level_key};
    use std::path::Path;

    let artifact_name = opts.artifact.as_deref().unwrap_or("snapshot");

    // Resolve the scan directory once. snapshot.json and every sibling artifact
    // live side by side in it, so one resolution serves both branches below and
    // the "then where does this key live" probe on the miss path.
    let explicit_snapshot = if let Some(ref explicit_path) = opts.snapshot_path {
        use crate::snapshot::Snapshot;
        match Snapshot::find_latest_snapshot(Some(explicit_path.as_ref())) {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("[loct][error] {}", e);
                eprintln!("[loct][hint] Specified snapshot path not found.");
                return DispatchResult::Exit(1);
            }
        }
    } else {
        None
    };

    let artifacts_dir = match explicit_snapshot {
        Some(ref path) => path.parent().map(|p| p.to_path_buf()),
        None => Some(artifacts_dir_for(Path::new("."))),
    };

    // Load the document the filter runs against.
    let (source_json, source_label) = if artifact_name != "snapshot" {
        let Some(ref dir) = artifacts_dir else {
            eprintln!("[loct][error] cannot resolve the artifact directory");
            return DispatchResult::Exit(1);
        };
        let path = dir.join(format!("{artifact_name}.json"));
        let body = match std::fs::read_to_string(&path) {
            Ok(body) => body,
            Err(e) => {
                eprintln!(
                    "[loct][error] artifact '{artifact_name}' not readable at {}: {e}",
                    path.display()
                );
                eprintln!(
                    "[loct][hint] run `loct` (full pass) to materialize findings artifacts, then retry"
                );
                return DispatchResult::Exit(1);
            }
        };
        match serde_json::from_str::<serde_json::Value>(&body) {
            Ok(v) => (v, format!("{artifact_name}.json")),
            Err(e) => {
                eprintln!(
                    "[loct][error] artifact '{artifact_name}' at {} is not valid JSON: {e}",
                    path.display()
                );
                return DispatchResult::Exit(1);
            }
        }
    } else {
        let snapshot = if let Some(ref snapshot_path) = explicit_snapshot {
            match std::fs::read_to_string(snapshot_path)
                .map_err(std::io::Error::other)
                .and_then(|content| {
                    serde_json::from_str::<crate::snapshot::Snapshot>(&content)
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                }) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[loct][error] Failed to load snapshot: {}", e);
                    return DispatchResult::Exit(1);
                }
            }
        } else {
            // No explicit path - use load_or_create_snapshot for auto-scan
            match load_or_create_snapshot(Path::new("."), global) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[loct][error] {}", e);
                    return DispatchResult::Exit(1);
                }
            }
        };

        // Convert snapshot to JSON value for jaq
        match serde_json::to_value(&snapshot) {
            Ok(v) => (v, "snapshot.json".to_string()),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize snapshot: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    };

    // A missing top-level key is a miss, not a value. jq would answer `null`
    // and the caller would read that as "the repo has no summary" instead of
    // "this document has no such key"; name what IS there instead.
    if let Some(key) = leading_top_level_key(&opts.filter)
        && source_json
            .as_object()
            .is_some_and(|obj| !obj.contains_key(&key))
    {
        report_top_level_miss(
            &key,
            &source_json,
            &source_label,
            artifacts_dir.as_deref(),
            artifact_name,
        );
        return DispatchResult::Exit(1);
    }

    let snapshot_json = source_json;

    // Execute the jaq filter
    let executor = JaqExecutor::new();
    let results = match executor.execute(
        &opts.filter,
        &snapshot_json,
        &opts.string_args,
        &opts.json_args,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[loct][error] Filter execution failed: {}", e);
            return DispatchResult::Exit(1);
        }
    };

    // Output results
    for result in &results {
        let output = format_output(result, opts.raw_output, opts.compact_output);
        println!("{}", output);
    }

    // Exit status mode: exit 1 if no results or all results are false/null
    if opts.exit_status {
        if results.is_empty() {
            return DispatchResult::Exit(1);
        }

        // Check if all results are false or null
        let all_false_or_null = results
            .iter()
            .all(|v| v.is_null() || (v.as_bool().is_some() && !v.as_bool().unwrap()));

        if all_false_or_null {
            return DispatchResult::Exit(1);
        }
    }

    DispatchResult::Exit(0)
}

#[cfg(test)]
mod project_root_tests {
    use super::*;
    use crate::cli::command::{FindOptions, QueryKind, QueryOptions};
    use crate::query::query_where_symbol;
    use crate::snapshot::test_env;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command as Proc;

    fn git_init_with_file(root: &std::path::Path, rel: &str, body: &str) {
        fs::create_dir_all(root.join(PathBuf::from(rel).parent().unwrap_or(root))).unwrap();
        fs::write(root.join(rel), body).unwrap();
        let run = |args: &[&str]| {
            let out = Proc::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init"]);
        run(&["config", "user.email", "t@t.com"]);
        run(&["config", "user.name", "t"]);
        run(&["add", "."]);
        run(&["commit", "-m", "seed"]);
    }

    /// End-to-end: --project scopes where-symbol / who-imports load path so a
    /// unique symbol in sibling-a is found under that project and not under
    /// sibling-b (skeptic cluster B gap).
    // `isolated_cache()` swaps two process-global env vars (LOCT_CACHE_DIR,
    // LOCT_ALLOW_NON_GIT_ROOT), so every holder must be serialized — a
    // concurrent test swapping LOCT_CACHE_DIR between this test's scan and its
    // load makes the snapshot land in one cache and be looked up in another.
    // Every other isolated_cache() caller in the crate is already #[serial];
    // this one was not, which is why it failed only on the Linux runner.
    #[test]
    #[serial_test::serial]
    fn where_symbol_and_who_imports_honor_explicit_project_root() {
        let (_cache_dir, _cache_env) = test_env::isolated_cache();
        let base = tempfile::tempdir().unwrap();
        let sibling_a = base.path().join("sibling-a");
        let sibling_b = base.path().join("sibling-b");
        fs::create_dir_all(sibling_a.join("src")).unwrap();
        fs::create_dir_all(sibling_b.join("src")).unwrap();

        git_init_with_file(&sibling_a, "src/lib.rs", "pub fn UNIQUE_SYM_AAA() {}\n");
        git_init_with_file(&sibling_b, "src/lib.rs", "pub fn OTHER_ONLY_SYM() {}\n");

        let global = GlobalOptions {
            quiet: true,
            force_non_git: false,
            ..Default::default()
        };

        // where-symbol --project sibling-a finds the unique symbol
        let snap_a = load_or_create_query_snapshot_for_roots(
            std::slice::from_ref(&sibling_a),
            &query_global_options(&global),
        )
        .expect("scan sibling-a");
        let hit_a = query_where_symbol(&snap_a, "UNIQUE_SYM_AAA");
        assert!(
            !hit_a.results.is_empty(),
            "where-symbol under sibling-a must find UNIQUE_SYM_AAA; got {hit_a:?}"
        );

        // Same symbol under sibling-b: not present
        let snap_b = load_or_create_query_snapshot_for_roots(
            std::slice::from_ref(&sibling_b),
            &query_global_options(&global),
        )
        .expect("scan sibling-b");
        let hit_b = query_where_symbol(&snap_b, "UNIQUE_SYM_AAA");
        assert!(
            hit_b.results.is_empty(),
            "where-symbol under sibling-b must not invent UNIQUE_SYM_AAA; got {hit_b:?}"
        );

        // Wiring: FindOptions / QueryOptions scan_roots carry project
        let find_opts = FindOptions {
            queries: vec!["UNIQUE_SYM_AAA".into()],
            where_symbol: true,
            roots: vec![sibling_a.clone()],
            ..Default::default()
        };
        assert_eq!(find_opts.scan_roots(), vec![sibling_a.clone()]);

        let query_opts = QueryOptions {
            kind: QueryKind::WhoImports,
            target: "src/lib.rs".into(),
            limit: None,
            all: false,
            roots: vec![sibling_a.clone()],
        };
        assert_eq!(query_opts.scan_roots(), vec![sibling_a]);

        // Handler entry (where-symbol) returns 0 when scoped correctly
        let result = handle_find_where_symbol_command(&find_opts, &global);
        assert!(matches!(result, DispatchResult::Exit(0)));
    }

    #[test]
    #[serial_test::serial]
    fn w2_03_body_root_flag_scopes_lookup() {
        let (_cache_dir, _cache_env) = test_env::isolated_cache();
        let base = tempfile::tempdir().unwrap();
        let sibling_a = base.path().join("sibling-a");
        let sibling_b = base.path().join("sibling-b");
        fs::create_dir_all(sibling_a.join("src")).unwrap();
        fs::create_dir_all(sibling_b.join("src")).unwrap();

        git_init_with_file(
            &sibling_a,
            "src/lib.rs",
            "pub fn UNIQUE_BODY_SYM_AAA() {\n    let val = 100;\n}\n",
        );
        git_init_with_file(
            &sibling_b,
            "src/lib.rs",
            "pub fn OTHER_BODY_SYM_BBB() {\n    let val = 200;\n}\n",
        );

        let global = GlobalOptions {
            quiet: true,
            json: true,
            force_non_git: false,
            ..Default::default()
        };

        // loct body UNIQUE_BODY_SYM_AAA --root sibling-a succeeds
        let body_opts_a = BodyOptions {
            symbol: "UNIQUE_BODY_SYM_AAA".into(),
            line_cap: None,
            file: None,
            root: Some(sibling_a.clone()),
        };
        let res_a = handle_body_command(&body_opts_a, &global);
        assert!(
            matches!(res_a, DispatchResult::Exit(0)),
            "body command with root sibling-a must succeed"
        );

        // loct body UNIQUE_BODY_SYM_AAA --root sibling-b fails (not in sibling-b)
        let body_opts_b = BodyOptions {
            symbol: "UNIQUE_BODY_SYM_AAA".into(),
            line_cap: None,
            file: None,
            root: Some(sibling_b.clone()),
        };
        let res_b = handle_body_command(&body_opts_b, &global);
        assert!(
            matches!(res_b, DispatchResult::Exit(1)),
            "body command with root sibling-b must fail for UNIQUE_BODY_SYM_AAA"
        );

        // Also verify snapshot source reading: reading through query_symbol_body works with root
        let snap_a = load_or_create_query_snapshot_for_roots(
            std::slice::from_ref(&sibling_a),
            &query_global_options(&global),
        )
        .expect("scan sibling-a");
        let body_result = crate::body::query_symbol_body(&snap_a, "UNIQUE_BODY_SYM_AAA", None);
        assert_eq!(body_result.bodies.len(), 1);
        assert!(body_result.bodies[0].source.contains("let val = 100;"));
        assert_eq!(body_result.bodies[0].extent, crate::body::EXTENT_BRACE);
        assert!(!body_result.bodies[0].truncated);
    }
}
