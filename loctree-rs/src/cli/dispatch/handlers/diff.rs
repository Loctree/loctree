//! Diff-related command handlers
//!
//! Handles: impact, diff (with auto_scan_base), problems_only

use super::super::super::command::{DiffOptions, ImpactCommandOptions};
use super::super::{DispatchResult, GlobalOptions, load_or_create_snapshot};

fn handle_changed_files_summary(
    opts: &DiffOptions,
    global: &GlobalOptions,
    since_path: &str,
) -> DispatchResult {
    use crate::git::{ChangeStatus, GitRepo};
    use serde_json::json;
    use std::path::Path;

    let git_repo = match GitRepo::discover(Path::new(".")) {
        Ok(repo) => repo,
        Err(e) => {
            eprintln!("[loct][error] Not a git repository: {}", e);
            eprintln!("[loct][hint] --changed-files requires a git repository");
            return DispatchResult::Exit(1);
        }
    };

    let changed_files = match git_repo.changed_files(since_path, "HEAD") {
        Ok(files) => files,
        Err(e) => {
            eprintln!(
                "[loct][error] Failed to summarize changed files from '{}': {}",
                since_path, e
            );
            eprintln!("[loct][hint] Provide a branch, tag, commit, or HEAD~N ref.");
            return DispatchResult::Exit(1);
        }
    };

    if global.json || opts.jsonl {
        if opts.jsonl {
            for file in &changed_files {
                match serde_json::to_string(file) {
                    Ok(json) => println!("{}", json),
                    Err(e) => {
                        eprintln!("[loct][error] Failed to serialize changed file: {}", e);
                        return DispatchResult::Exit(1);
                    }
                }
            }
        } else {
            let payload = json!({
                "from": since_path,
                "to": "HEAD",
                "total": changed_files.len(),
                "changed_files": changed_files,
            });
            match serde_json::to_string_pretty(&payload) {
                Ok(json) => println!("{}", json),
                Err(e) => {
                    eprintln!("[loct][error] Failed to serialize changed files: {}", e);
                    return DispatchResult::Exit(1);
                }
            }
        }
        return DispatchResult::Exit(0);
    }

    println!("Changed Files ({})", changed_files.len());
    println!("  From: {}", since_path);
    println!("  To:   HEAD");
    println!();
    for file in changed_files {
        let marker = match file.status {
            ChangeStatus::Added => "+",
            ChangeStatus::Deleted => "-",
            ChangeStatus::Modified => "~",
            ChangeStatus::Renamed => "R",
            ChangeStatus::Copied => "C",
        };
        match (file.old_path, file.new_path) {
            (Some(old), Some(new)) if old != new => {
                println!("  {} {} -> {}", marker, old.display(), new.display());
            }
            (Some(path), _) | (_, Some(path)) => {
                println!("  {} {}", marker, path.display());
            }
            (None, None) => {}
        }
    }

    DispatchResult::Exit(0)
}

/// Handle the impact command - analyze impact of a file
pub fn handle_impact_command(
    opts: &ImpactCommandOptions,
    global: &GlobalOptions,
) -> DispatchResult {
    use crate::fs_utils::{SanitizedPath, explain_ignore_for_path};
    use crate::impact::{ImpactOptions, analyze_impact, format_impact_text};
    use std::path::Path;

    let target_path = Path::new(&opts.target);
    let cwd = std::env::current_dir().unwrap_or_default();
    // If absolute path outside cwd, find its git root; otherwise use opts.root or cwd
    let root = if target_path.is_absolute() && !target_path.starts_with(&cwd) {
        std::iter::successors(target_path.parent(), |p| p.parent())
            .find(|p| p.join(".git").exists())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| opts.root.clone().unwrap_or(cwd.clone()))
    } else {
        opts.root.clone().unwrap_or(cwd.clone())
    };

    let snapshot = match load_or_create_snapshot(&root, global) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[loct][error] {}", e);
            return DispatchResult::Exit(1);
        }
    };
    // Rebase target to relative path and validate it exists in snapshot
    let rel_target = target_path
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(opts.target.clone());
    if !snapshot
        .files
        .iter()
        .any(|f| f.path == rel_target || f.path.ends_with(&rel_target))
    {
        let candidate = if target_path.is_absolute() {
            target_path.to_path_buf()
        } else {
            root.join(target_path)
        };
        if candidate.is_file()
            && let Ok(sanitized) = SanitizedPath::within(&root, &candidate)
            && let Some(note) = explain_ignore_for_path(&root, sanitized.as_path())
        {
            eprintln!(
                "[loct][error] File exists but is excluded from snapshot: {}",
                rel_target
            );
            eprintln!("[loct][hint] Detected exclusion: {}", note);
            eprintln!(
                "[loct][hint] Use `loct slice {} --json` for a core-only fallback read, or adjust ignore/include rules and run `loct scan --full-scan`.",
                rel_target
            );
            return DispatchResult::Exit(1);
        }
        eprintln!("[loct][error] File not found in snapshot: {}", rel_target);
        return DispatchResult::Exit(1);
    }

    let result = analyze_impact(
        &snapshot,
        &rel_target,
        &ImpactOptions {
            max_depth: opts.depth,
            include_reexports: true,
        },
    );
    if global.json {
        match serde_json::to_string_pretty(&result) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("[loct][error] Failed to serialize: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    } else {
        print!("{}", format_impact_text(&result));
    }
    DispatchResult::Exit(0)
}

/// Handle auto-scan-base diff: create worktree, scan, compare, cleanup
pub fn handle_auto_scan_base_diff(
    opts: &DiffOptions,
    global: &GlobalOptions,
    since_path: &str,
) -> DispatchResult {
    use crate::diff::SnapshotDiff;
    use crate::git::GitRepo;
    use crate::snapshot::Snapshot;
    use std::path::Path;
    use tempfile::TempDir;

    // Discover git repository
    let git_repo = match GitRepo::discover(Path::new(".")) {
        Ok(repo) => repo,
        Err(e) => {
            eprintln!("[loct][error] Not a git repository: {}", e);
            eprintln!("[loct][hint] --auto-scan-base requires a git repository");
            return DispatchResult::Exit(1);
        }
    };

    // Verify target branch exists
    if let Err(e) = git_repo.resolve_ref(since_path) {
        eprintln!(
            "[loct][error] Failed to resolve branch '{}': {}",
            since_path, e
        );
        eprintln!("[loct][hint] Ensure the branch/commit exists");
        return DispatchResult::Exit(1);
    }

    if !global.quiet {
        eprintln!("[loct] Creating temporary worktree for '{}'...", since_path);
    }

    // Create temporary directory for worktree
    let temp_dir = match TempDir::new() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("[loct][error] Failed to create temp directory: {}", e);
            return DispatchResult::Exit(1);
        }
    };

    let worktree_path = temp_dir
        .path()
        .join(format!("loctree-diff-{}", since_path.replace('/', "-")));

    // Create worktree
    if let Err(e) = git_repo.create_worktree(since_path, &worktree_path) {
        eprintln!("[loct][error] Failed to create worktree: {}", e);
        eprintln!("[loct][hint] Ensure branch exists and worktree can be created");
        return DispatchResult::Exit(1);
    }

    // Ensure cleanup happens even if we encounter errors
    let cleanup = || {
        if let Err(e) = git_repo.remove_worktree(&worktree_path)
            && !global.quiet
        {
            eprintln!("[loct][warning] Failed to remove worktree: {}", e);
        }
    };

    // Scan the worktree
    if !global.quiet {
        eprintln!("[loct] Scanning worktree...");
    }

    // Scan the worktree using run_init with the unified file universe so the
    // diff compares the same file set as the current snapshot (detect-applied
    // extensions + .loctignore), not a broader default-extension scan.
    let worktree_snapshot = {
        let parsed = crate::snapshot::unified_scan_args(&worktree_path, global.verbose);

        let root_list = vec![worktree_path.clone()];

        if let Err(e) = crate::snapshot::run_init(&root_list, &parsed) {
            eprintln!("[loct][error] Failed to scan worktree: {}", e);
            cleanup();
            return DispatchResult::Exit(1);
        }

        // Load the snapshot we just created
        match Snapshot::load(&worktree_path) {
            Ok(snap) => snap,
            Err(e) => {
                eprintln!("[loct][error] Failed to load worktree snapshot: {}", e);
                cleanup();
                return DispatchResult::Exit(1);
            }
        }
    };

    // Load current snapshot (auto-scan if missing)
    let current_snapshot = match load_or_create_snapshot(Path::new("."), global) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[loct][error] Failed to load current snapshot: {}", e);
            cleanup();
            return DispatchResult::Exit(1);
        }
    };

    // Get changed files using git diff
    let changed_files = match git_repo.changed_files(since_path, "HEAD") {
        Ok(files) => files,
        Err(e) => {
            if !global.quiet {
                eprintln!("[loct][warning] Failed to get changed files: {}", e);
            }
            vec![]
        }
    };

    // Get commit info
    let from_commit = git_repo.get_commit_info(since_path).ok();
    let to_commit = git_repo.get_commit_info("HEAD").ok();

    // Compare snapshots (artifact fence default-on; --include-artifacts opts out)
    let diff = SnapshotDiff::compare_fenced(
        &worktree_snapshot,
        &current_snapshot,
        from_commit,
        to_commit,
        &changed_files,
        opts.include_artifacts,
    );

    // Cleanup worktree
    cleanup();

    if !global.quiet {
        eprintln!("[loct] Worktree cleaned up");
    }

    // Output results
    if global.json || opts.jsonl {
        // JSON/JSONL output
        if opts.jsonl {
            match serde_json::to_string(&diff) {
                Ok(json) => println!("{}", json),
                Err(e) => {
                    eprintln!("[loct][error] Failed to serialize diff: {}", e);
                    return DispatchResult::Exit(1);
                }
            }
        } else {
            match serde_json::to_string_pretty(&diff) {
                Ok(json) => println!("{}", json),
                Err(e) => {
                    eprintln!("[loct][error] Failed to serialize diff: {}", e);
                    return DispatchResult::Exit(1);
                }
            }
        }
    } else {
        // Human-readable output
        println!("Snapshot Diff (auto-scanned):");
        println!("  From: {} (scanned in worktree)", since_path);
        println!("  To:   (current)");
        println!();
        println!("Summary: {}", diff.impact.summary);
        println!("Risk Score: {:.2}", diff.impact.risk_score);
        println!();

        if !diff.files.added.is_empty() {
            println!("Files Added ({}):", diff.files.added.len());
            for path in diff.files.added.iter().take(20) {
                println!("  + {}", path.display());
            }
            if diff.files.added.len() > 20 {
                println!("  ... and {} more", diff.files.added.len() - 20);
            }
            println!();
        }

        if !diff.files.removed.is_empty() {
            println!("Files Removed ({}):", diff.files.removed.len());
            for path in diff.files.removed.iter().take(20) {
                println!("  - {}", path.display());
            }
            if diff.files.removed.len() > 20 {
                println!("  ... and {} more", diff.files.removed.len() - 20);
            }
            println!();
        }

        if !diff.files.modified.is_empty() {
            println!("Files Modified ({}):", diff.files.modified.len());
            for path in diff.files.modified.iter().take(20) {
                println!("  ~ {}", path.display());
            }
            if diff.files.modified.len() > 20 {
                println!("  ... and {} more", diff.files.modified.len() - 20);
            }
            println!();
        }

        if !diff.exports.removed.is_empty() {
            println!("Exports Removed ({}):", diff.exports.removed.len());
            for export in diff.exports.removed.iter().take(10) {
                println!(
                    "  - {} ({}) in {}",
                    export.name,
                    export.kind,
                    export.file.display()
                );
            }
            if diff.exports.removed.len() > 10 {
                println!("  ... and {} more", diff.exports.removed.len() - 10);
            }
        }

        if !diff.exports.artifact_excluded.is_empty() {
            println!(
                "{} (exports in artifact files; use --include-artifacts to inspect)",
                diff.exports.artifact_excluded.summary_line()
            );
        }
    }

    DispatchResult::Exit(0)
}

/// Handle the diff command directly
pub fn handle_diff_command(opts: &DiffOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::diff::SnapshotDiff;
    use crate::snapshot::Snapshot;
    use std::path::Path;

    // For MVP: Load snapshots from paths or IDs
    // `--since` is required and points to a snapshot path or ID
    let since_path = if let Some(s) = opts.since.as_ref() {
        s
    } else {
        eprintln!("[loct][error] --since is required for diff.");
        eprintln!("[loct][hint] try: loct diff --since <snapshot_path|branch@sha|HEAD~N>");
        return DispatchResult::Exit(1);
    };

    // Handle --auto-scan-base: create worktree, scan, compare, cleanup
    if opts.auto_scan_base {
        return handle_auto_scan_base_diff(opts, global, since_path);
    }

    if opts.changed_files {
        return handle_changed_files_summary(opts, global, since_path);
    }

    // Load "from" snapshot
    let from_snapshot = match Snapshot::load(Path::new(since_path)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "[loct][error] Failed to load snapshot from '{}': {}",
                since_path, e
            );
            eprintln!("[loct][hint] Provide a valid snapshot path or run 'loct scan' first.");
            return DispatchResult::Exit(1);
        }
    };

    // Load "to" snapshot (current if not specified)
    let to_snapshot = if let Some(ref to_path) = opts.to {
        match Snapshot::load(Path::new(to_path)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[loct][error] Failed to load snapshot from '{}': {}",
                    to_path, e
                );
                return DispatchResult::Exit(1);
            }
        }
    } else {
        // Load current snapshot from .loctree/ (auto-scan if missing)
        match load_or_create_snapshot(Path::new("."), global) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[loct][error] Failed to load current snapshot: {}", e);
                return DispatchResult::Exit(1);
            }
        }
    };

    // For now, we don't have git commit info in this flow
    // In future, we could extract it from snapshot metadata
    let from_commit = None;
    let to_commit = None;

    // We don't have changed_files info without git integration
    // For snapshot-to-snapshot diff, we'll compute it from the diff itself
    let changed_files = vec![];

    // Compare snapshots (artifact fence default-on; --include-artifacts opts out)
    let diff = SnapshotDiff::compare_fenced(
        &from_snapshot,
        &to_snapshot,
        from_commit,
        to_commit,
        &changed_files,
        opts.include_artifacts,
    );

    // If problems_only flag is set, compute NEW problems only
    if opts.problems_only {
        return handle_problems_only_diff(
            &from_snapshot,
            &to_snapshot,
            &diff,
            since_path,
            opts,
            global,
        );
    }

    // Output results (full diff)
    if global.json || opts.jsonl {
        // JSON/JSONL output
        if opts.jsonl {
            // One-line JSON (compact)
            match serde_json::to_string(&diff) {
                Ok(json) => println!("{}", json),
                Err(e) => {
                    eprintln!("[loct][error] Failed to serialize diff: {}", e);
                    return DispatchResult::Exit(1);
                }
            }
        } else {
            // Pretty JSON
            match serde_json::to_string_pretty(&diff) {
                Ok(json) => println!("{}", json),
                Err(e) => {
                    eprintln!("[loct][error] Failed to serialize diff: {}", e);
                    return DispatchResult::Exit(1);
                }
            }
        }
    } else {
        // Human-readable output
        println!("Snapshot Diff:");
        println!("  From: {}", since_path);
        if let Some(ref to_path) = opts.to {
            println!("  To:   {}", to_path);
        } else {
            println!("  To:   (current)");
        }
        println!();
        println!("Summary: {}", diff.impact.summary);
        println!("Risk Score: {:.2}", diff.impact.risk_score);
        println!();

        if !diff.files.added.is_empty() {
            println!("Files Added ({}):", diff.files.added.len());
            for path in &diff.files.added {
                println!("  + {}", path.display());
            }
            println!();
        }

        if !diff.files.removed.is_empty() {
            println!("Files Removed ({}):", diff.files.removed.len());
            for path in &diff.files.removed {
                println!("  - {}", path.display());
            }
            println!();
        }

        if !diff.files.modified.is_empty() {
            println!("Files Modified ({}):", diff.files.modified.len());
            for path in &diff.files.modified {
                println!("  ~ {}", path.display());
            }
            println!();
        }

        if !diff.exports.removed.is_empty() {
            println!("Exports Removed ({}):", diff.exports.removed.len());
            for export in &diff.exports.removed {
                println!(
                    "  - {} ({}) in {}",
                    export.name,
                    export.kind,
                    export.file.display()
                );
            }
            println!();
        }

        if !diff.exports.added.is_empty() {
            println!("Exports Added ({}):", diff.exports.added.len());
            for export in &diff.exports.added {
                println!(
                    "  + {} ({}) in {}",
                    export.name,
                    export.kind,
                    export.file.display()
                );
            }
            println!();
        }

        if !diff.exports.artifact_excluded.is_empty() {
            println!(
                "{} (exports in artifact files; use --include-artifacts to inspect)",
                diff.exports.artifact_excluded.summary_line()
            );
        }
    }

    DispatchResult::Exit(0)
}

/// Handle problems-only diff output: show only NEW problems
pub fn handle_problems_only_diff(
    from_snapshot: &crate::snapshot::Snapshot,
    to_snapshot: &crate::snapshot::Snapshot,
    _diff: &crate::diff::SnapshotDiff,
    since_path: &str,
    opts: &DiffOptions,
    global: &GlobalOptions,
) -> DispatchResult {
    use crate::analyzer::cycles::find_cycles_with_lazy;
    use crate::analyzer::dead_parrots::{DeadFilterConfig, find_dead_exports};
    use serde_json::json;
    use std::collections::HashSet;

    // 1. Find dead exports in both snapshots
    let dead_config = DeadFilterConfig::default();
    let from_dead = find_dead_exports(&from_snapshot.files, true, None, dead_config.clone());
    let to_dead = find_dead_exports(&to_snapshot.files, true, None, dead_config);

    // Build sets for comparison (use symbol, not name)
    let from_dead_set: HashSet<(&str, &str)> = from_dead
        .iter()
        .map(|d| (d.file.as_str(), d.symbol.as_str()))
        .collect();

    let new_dead_exports: Vec<_> = to_dead
        .iter()
        .filter(|d| !from_dead_set.contains(&(d.file.as_str(), d.symbol.as_str())))
        .collect();

    // 2. Find circular imports (cycles) in both snapshots
    // Extract edges from snapshots
    let from_edges: Vec<(String, String, String)> = from_snapshot
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
        .collect();
    let to_edges: Vec<(String, String, String)> = to_snapshot
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
        .collect();

    let from_cycles = find_cycles_with_lazy(&from_edges).0;
    let to_cycles = find_cycles_with_lazy(&to_edges).0;

    // Build cycle signature sets for comparison
    let from_cycle_sigs: HashSet<String> = from_cycles
        .iter()
        .map(|cycle| {
            let mut sorted = cycle.clone();
            sorted.sort();
            sorted.join("|")
        })
        .collect();

    let new_cycles: Vec<_> = to_cycles
        .iter()
        .filter(|cycle| {
            let mut sorted = (*cycle).clone();
            sorted.sort();
            let sig = sorted.join("|");
            !from_cycle_sigs.contains(&sig)
        })
        .collect();

    // 3. Find missing handlers in both snapshots
    let from_missing: HashSet<String> = from_snapshot
        .command_bridges
        .iter()
        .filter(|b| !b.has_handler && b.is_called)
        .map(|b| b.name.clone())
        .collect();

    let new_missing_handlers: Vec<_> = to_snapshot
        .command_bridges
        .iter()
        .filter(|b| !b.has_handler && b.is_called && !from_missing.contains(&b.name))
        .collect();

    let total_problems = new_dead_exports.len() + new_cycles.len() + new_missing_handlers.len();

    // Output results
    if global.json || opts.jsonl {
        let problems = json!({
            "from": since_path,
            "to": opts.to.as_deref().unwrap_or("(current)"),
            "new_problems": {
                "dead_exports": new_dead_exports.iter().map(|d| json!({
                    "file": d.file,
                    "symbol": d.symbol,
                    "confidence": d.confidence,
                    "line": d.line,
                    "reason": d.reason,
                })).collect::<Vec<_>>(),
                "circular_imports": new_cycles.iter().map(|cycle| json!({
                    "path": cycle,
                    "length": cycle.len(),
                })).collect::<Vec<_>>(),
                "missing_handlers": new_missing_handlers.iter().map(|b| json!({
                    "name": b.name,
                    "frontend_calls": b.frontend_calls,
                })).collect::<Vec<_>>(),
            },
            "summary": {
                "new_dead_exports": new_dead_exports.len(),
                "new_circular_imports": new_cycles.len(),
                "new_missing_handlers": new_missing_handlers.len(),
            }
        });

        if opts.jsonl {
            match serde_json::to_string(&problems) {
                Ok(json) => println!("{}", json),
                Err(e) => {
                    eprintln!("[loct][error] Failed to serialize problems: {}", e);
                    return DispatchResult::Exit(1);
                }
            }
        } else {
            match serde_json::to_string_pretty(&problems) {
                Ok(json) => println!("{}", json),
                Err(e) => {
                    eprintln!("[loct][error] Failed to serialize problems: {}", e);
                    return DispatchResult::Exit(1);
                }
            }
        }
    } else {
        // Human-readable output
        println!("New Problems Since Last Snapshot:");
        println!("  From: {}", since_path);
        if let Some(ref to_path) = opts.to {
            println!("  To:   {}", to_path);
        } else {
            println!("  To:   (current)");
        }
        println!();

        if total_problems == 0 {
            println!("[OK] No new problems detected!");
        } else {
            if !new_dead_exports.is_empty() {
                println!("New Dead Exports ({}):", new_dead_exports.len());
                for export in &new_dead_exports {
                    let confidence_indicator = match export.confidence.as_str() {
                        "high" => "[!!]",
                        "medium" => "[!]",
                        _ => "[-]",
                    };
                    let line_info = export.line.map(|l| format!(":{}", l)).unwrap_or_default();
                    println!(
                        "  {} {} in {}{} [{}]",
                        confidence_indicator,
                        export.symbol,
                        export.file,
                        line_info,
                        export.confidence
                    );
                }
                println!();
            }

            if !new_cycles.is_empty() {
                println!("New Circular Imports ({}):", new_cycles.len());
                for cycle in &new_cycles {
                    println!("  Cycle of {} files:", cycle.len());
                    for (i, file) in cycle.iter().enumerate() {
                        if i == cycle.len() - 1 {
                            println!("    {} -> (back to {})", file, cycle[0]);
                        } else {
                            println!("    {}", file);
                        }
                    }
                }
                println!();
            }

            if !new_missing_handlers.is_empty() {
                println!("New Missing Handlers ({}):", new_missing_handlers.len());
                for bridge in &new_missing_handlers {
                    println!("  Command: {}", bridge.name);
                    println!("    Frontend calls ({}):", bridge.frontend_calls.len());
                    for (file, line) in &bridge.frontend_calls {
                        println!("      {}:{}", file, line);
                    }
                }
                println!();
            }

            println!("Summary: {} new problem(s) detected", total_problems);
        }

        return DispatchResult::Exit(if total_problems > 0 { 1 } else { 0 });
    }

    // For JSON output, exit with non-zero if problems found
    DispatchResult::Exit(if total_problems > 0 { 1 } else { 0 })
}
