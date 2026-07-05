//! Query-related command handlers
//!
//! Handles: query, jq_query

use super::super::super::command::{
    BodyOptions, FindOptions, JqQueryOptions, QueryKind, QueryOptions,
};
use super::super::{
    DispatchResult, GlobalOptions, load_or_create_query_snapshot_for_roots, load_or_create_snapshot,
};

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

    let roots = vec![std::path::PathBuf::from(".")];
    let query_global = query_global_options(global);
    let snapshot = match load_or_create_query_snapshot_for_roots(&roots, &query_global) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[loct][error] {}", e);
            return DispatchResult::Exit(1);
        }
    };

    let result = query_where_symbol(&snapshot, &target);

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
    }

    DispatchResult::Exit(0)
}

/// Handle the `body` command - bounded symbol source retrieval.
pub fn handle_body_command(opts: &BodyOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::body::query_symbol_body;

    let roots = vec![std::path::PathBuf::from(".")];
    let query_global = query_global_options(global);
    let snapshot = match load_or_create_query_snapshot_for_roots(&roots, &query_global) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[loct][error] {}", e);
            return DispatchResult::Exit(1);
        }
    };

    let result = query_symbol_body(&snapshot, &opts.symbol, opts.line_cap);

    if global.json {
        match serde_json::to_string_pretty(&result) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize results: {}", e);
                return DispatchResult::Exit(1);
            }
        }
        return DispatchResult::Exit(if result.bodies.is_empty() { 1 } else { 0 });
    }

    if result.bodies.is_empty() {
        println!("body '{}': (no source body found)", result.symbol);
        println!(
            "  hint: run `loct query where-symbol {}` to locate the symbol first.",
            result.symbol
        );
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
        println!("  hint: use a qualified symbol when available, e.g. Type::method.");
        return DispatchResult::Exit(1);
    }

    for body in &result.bodies {
        println!(
            "── {} [{}] {}:{}-{} ──",
            body.symbol, body.language, body.file, body.start_line, body.end_line
        );
        println!("{}", body.source);
        if body.truncated {
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

/// Handle the query command directly
pub fn handle_query_command(opts: &QueryOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::query::{
        SwiftTypeResolutionStatus, classify_swift_type_references, query_component_of,
        query_where_symbol, query_who_imports,
    };

    // Load snapshot (auto-scan if missing)
    let roots = vec![std::path::PathBuf::from(".")];
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
    }

    DispatchResult::Exit(0)
}

fn query_global_options(global: &GlobalOptions) -> GlobalOptions {
    let mut scoped = global.clone();
    if !scoped.verbose {
        scoped.quiet = true;
    }
    scoped
}

/// Handle the jq query command - execute jaq filter on snapshot
pub fn handle_jq_query_command(opts: &JqQueryOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::jaq_query::{JaqExecutor, format_output};
    use std::path::Path;

    // Load snapshot (auto-scan if missing)
    // If user specified explicit snapshot_path, try that first
    let snapshot = if let Some(ref explicit_path) = opts.snapshot_path {
        // User specified explicit path - use it directly without auto-create
        use crate::snapshot::Snapshot;
        let snapshot_path = match Snapshot::find_latest_snapshot(Some(explicit_path.as_ref())) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[loct][error] {}", e);
                eprintln!("[loct][hint] Specified snapshot path not found.");
                return DispatchResult::Exit(1);
            }
        };
        match std::fs::read_to_string(&snapshot_path)
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
    let snapshot_json = match serde_json::to_value(&snapshot) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[loct][error] Failed to serialize snapshot: {}", e);
            return DispatchResult::Exit(1);
        }
    };

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
