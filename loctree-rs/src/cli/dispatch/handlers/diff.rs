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

    let changed_files = match opts.to.as_deref().map_or_else(
        || git_repo.changed_files_in_worktree(since_path),
        |target| git_repo.changed_files(since_path, target),
    ) {
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
                "to": opts.to.as_deref().unwrap_or("working-tree"),
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
    println!("  To:   {}", opts.to.as_deref().unwrap_or("working-tree"));
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

/// Handle auto-scan-base diff: export the ref tree (no git worktree), scan, compare.
pub fn handle_auto_scan_base_diff(
    opts: &DiffOptions,
    global: &GlobalOptions,
    since_path: &str,
) -> DispatchResult {
    let mut compatible = opts.clone();
    compatible.since = Some(since_path.to_owned());
    compatible.auto_scan_base = false;
    handle_diff_command(&compatible, global)
}

/// Handle the diff command directly
pub fn handle_diff_command(opts: &DiffOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::diff::SnapshotDiff;
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

    if opts.auto_scan_base {
        return handle_auto_scan_base_diff(opts, global, since_path);
    }
    if opts.changed_files {
        return handle_changed_files_summary(opts, global, since_path);
    }
    let project = Path::new(".");
    let (from, to) =
        match crate::git_snapshot::resolve_pair(project, since_path, opts.to.as_deref()) {
            Ok(value) => value,
            Err(e) => {
                eprintln!("[loct][error] {e}");
                return DispatchResult::Exit(1);
            }
        };
    let changed_files = if let Some(base) = &from.commit {
        let repo = match crate::git::GitRepo::discover(project) {
            Ok(repo) => repo,
            Err(e) => {
                eprintln!("[loct][error] {e}");
                return DispatchResult::Exit(1);
            }
        };
        let result = match &to.commit {
            Some(target) => repo.changed_files(&base.hash, &target.hash),
            None if opts.to.is_none() => repo.changed_files_in_worktree(&base.hash),
            None => Ok(vec![]),
        };
        match result {
            Ok(files) => files,
            Err(e) => {
                eprintln!("[loct][error] {e}");
                return DispatchResult::Exit(1);
            }
        }
    } else {
        vec![]
    };
    let from_snapshot = from.snapshot;
    let to_snapshot = to.snapshot;
    let from_commit = from.commit;
    let to_commit = to.commit;

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

#[cfg(test)]
mod tests {
    use super::handle_auto_scan_base_diff;
    use crate::cli::command::{DiffOptions, GlobalOptions};
    use crate::cli::dispatch::DispatchResult;
    use serial_test::serial;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    fn git_worktree_list(repo: &Path) -> String {
        let output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(repo)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    struct CwdGuard(std::path::PathBuf);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    /// Acceptance: `loct diff --since HEAD~3 --auto-scan-base` must not
    /// register a git worktree.
    #[test]
    #[serial]
    fn w5_02_auto_scan_base_does_not_register_worktree() {
        let temp = TempDir::new().unwrap();
        let path = temp.path();
        Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(path)
            .output()
            .unwrap();
        for i in 0..4 {
            std::fs::write(path.join("main.ts"), format!("export const n = {i};\n")).unwrap();
            Command::new("git")
                .args(["add", "."])
                .current_dir(path)
                .output()
                .unwrap();
            Command::new("git")
                .args(["commit", "-m", &format!("c{i}")])
                .current_dir(path)
                .output()
                .unwrap();
        }

        let before = git_worktree_list(path);
        let _cwd = CwdGuard(std::env::current_dir().unwrap());
        std::env::set_current_dir(path).unwrap();

        let opts = DiffOptions {
            since: Some("HEAD~3".into()),
            auto_scan_base: true,
            jsonl: false,
            ..Default::default()
        };
        let global = GlobalOptions {
            quiet: true,
            json: true,
            ..Default::default()
        };
        let result = handle_auto_scan_base_diff(&opts, &global, "HEAD~3");
        assert!(
            matches!(result, DispatchResult::Exit(0)),
            "auto-scan-base should succeed on a tiny fixture"
        );
        let after = git_worktree_list(path);
        assert_eq!(
            before, after,
            "auto-scan-base must not add a git worktree; before={before:?} after={after:?}"
        );
    }
}
