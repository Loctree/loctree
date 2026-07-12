//! Handler for `loct prune-old-artifacts` — local `.loctree/` housekeeping.
//!
//! Living-tree projects accumulate per-branch snapshot directories
//! (`<branch>@<commit>/snapshot.json`) under `.loctree/` over agent runs.
//! Stale sub-`.loctree/` directories from foreign envs or older runs can
//! shadow legitimate project roots (causing scope-drift bugs) and inflate
//! the repo. This command enumerates them, keeps the N newest per dir,
//! and offers a dry-run preview by default.

use std::cmp::Reverse;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::cli::command::PruneOldArtifactsOptions;

use super::super::DispatchResult;

const LOCTREE_DIR: &str = ".loctree";

#[derive(Debug)]
struct SnapshotDir {
    path: PathBuf,
    mtime: SystemTime,
    size_bytes: u64,
}

pub fn handle_prune_old_artifacts(opts: &PruneOldArtifactsOptions) -> DispatchResult {
    let root = opts
        .root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let root = match root.canonicalize() {
        Ok(p) => p,
        Err(err) => {
            eprintln!(
                "[loct][prune] cannot resolve root {}: {err}",
                root.display()
            );
            return DispatchResult::Exit(1);
        }
    };

    let mut loctree_dirs: Vec<PathBuf> = Vec::new();
    let root_loctree = root.join(LOCTREE_DIR);
    if root_loctree.is_dir() {
        loctree_dirs.push(root_loctree);
    }
    if opts.include_sub {
        walk_for_loctree_dirs(&root, &mut loctree_dirs);
    }

    if loctree_dirs.is_empty() {
        println!(
            "[loct][prune] no `.loctree/` dirs found under {} — nothing to prune.",
            root.display()
        );
        return DispatchResult::Exit(0);
    }

    let mode = if opts.apply { "apply" } else { "dry-run" };
    println!(
        "[loct][prune] mode: {mode}, keep: {}, include-sub: {}",
        opts.keep, opts.include_sub
    );
    println!("[loct][prune] root: {}", root.display());
    println!();

    let mut total_removed_bytes: u64 = 0;
    let mut total_removed_dirs: usize = 0;

    for loctree_dir in &loctree_dirs {
        let mut snapshot_dirs = collect_per_branch_snapshot_dirs(loctree_dir);
        if snapshot_dirs.len() <= opts.keep {
            println!(
                "  {} — {} snapshot(s), within keep threshold (skipped)",
                loctree_dir.display(),
                snapshot_dirs.len()
            );
            continue;
        }

        snapshot_dirs.sort_by_key(|s| Reverse(s.mtime));
        let (keep, drop) = snapshot_dirs.split_at(opts.keep);

        println!(
            "  {} — {} snapshot(s), keep {} newest, prune {}:",
            loctree_dir.display(),
            snapshot_dirs.len(),
            keep.len(),
            drop.len()
        );
        for kept in keep {
            println!("    [keep] {}", kept.path.display());
        }
        for victim in drop {
            let label = if opts.apply { "drop" } else { "would-drop" };
            println!(
                "    [{label}] {} ({:.2} MB)",
                victim.path.display(),
                victim.size_bytes as f64 / 1_048_576.0
            );
            if opts.apply
                && let Err(err) = fs::remove_dir_all(&victim.path)
            {
                eprintln!(
                    "    [error] failed to remove {}: {err}",
                    victim.path.display()
                );
                continue;
            }
            total_removed_bytes = total_removed_bytes.saturating_add(victim.size_bytes);
            total_removed_dirs += 1;
        }
    }

    println!();
    let action = if opts.apply {
        "removed"
    } else {
        "would remove"
    };
    println!(
        "[loct][prune] {action} {total_removed_dirs} snapshot dir(s), {:.2} MB",
        total_removed_bytes as f64 / 1_048_576.0
    );
    if !opts.apply && total_removed_dirs > 0 {
        println!("[loct][prune] re-run with `--apply` to actually delete.");
    }

    DispatchResult::Exit(0)
}

fn walk_for_loctree_dirs(dir: &Path, accumulator: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        // Skip noisy dirs and the `.loctree/` ones themselves (root .loctree
        // is added by the caller; nested .loctree dirs are surfaced as
        // separate accumulator entries via the parent walk).
        if matches!(
            name,
            ".git" | "node_modules" | "target" | ".venv" | "dist" | "build" | ".loctree"
        ) {
            continue;
        }
        let candidate = path.join(LOCTREE_DIR);
        if candidate.is_dir() {
            accumulator.push(candidate);
        }
        walk_for_loctree_dirs(&path, accumulator);
    }
}

fn collect_per_branch_snapshot_dirs(loctree_dir: &Path) -> Vec<SnapshotDir> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(loctree_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let snapshot_file = path.join("snapshot.json");
        if !snapshot_file.is_file() {
            continue;
        }
        let mtime = fs::metadata(&snapshot_file)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let size_bytes = directory_size(&path);
        out.push(SnapshotDir {
            path,
            mtime,
            size_bytes,
        });
    }
    out
}

fn directory_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match fs::metadata(&path) {
            Ok(meta) if meta.is_dir() => {
                total = total.saturating_add(directory_size(&path));
            }
            Ok(meta) => {
                total = total.saturating_add(meta.len());
            }
            Err(_) => {}
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_snapshot_dir(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("snapshot.json"), b"{\"placeholder\": true}").unwrap();
        dir
    }

    #[test]
    fn dry_run_does_not_delete() {
        let tmp = TempDir::new().unwrap();
        let loctree = tmp.path().join(".loctree");
        fs::create_dir_all(&loctree).unwrap();
        for branch in ["a@111", "b@222", "c@333", "d@444", "e@555"] {
            make_snapshot_dir(&loctree, branch);
        }
        let opts = PruneOldArtifactsOptions {
            root: Some(tmp.path().to_path_buf()),
            keep: 2,
            include_sub: false,
            apply: false,
        };
        let _ = handle_prune_old_artifacts(&opts);
        // Dry-run: all five must still exist.
        assert_eq!(collect_per_branch_snapshot_dirs(&loctree).len(), 5);
    }

    #[test]
    fn apply_keeps_newest_and_removes_rest() {
        let tmp = TempDir::new().unwrap();
        let loctree = tmp.path().join(".loctree");
        fs::create_dir_all(&loctree).unwrap();
        // Create dirs with deterministic mtimes: a oldest, e newest.
        for (i, branch) in ["a@111", "b@222", "c@333", "d@444", "e@555"]
            .iter()
            .enumerate()
        {
            let dir = make_snapshot_dir(&loctree, branch);
            let when =
                SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + i as u64);
            let snapshot = dir.join("snapshot.json");
            // Best-effort touch — if filetime sets fail, mtime falls back to
            // creation time which still preserves relative ordering.
            let _ = filetime_set(&snapshot, when);
        }
        let opts = PruneOldArtifactsOptions {
            root: Some(tmp.path().to_path_buf()),
            keep: 2,
            include_sub: false,
            apply: true,
        };
        let _ = handle_prune_old_artifacts(&opts);
        let remaining: Vec<String> = collect_per_branch_snapshot_dirs(&loctree)
            .into_iter()
            .map(|s| s.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(remaining.len(), 2);
        // Newest two (`d@444`, `e@555`) must survive.
        assert!(remaining.contains(&"d@444".to_string()));
        assert!(remaining.contains(&"e@555".to_string()));
    }

    fn filetime_set(path: &Path, when: SystemTime) -> std::io::Result<()> {
        // Touch via std: open + write empty preserves content, set_modified
        // updates mtime (Rust 1.75+).
        let file = fs::OpenOptions::new().write(true).open(path)?;
        file.set_modified(when)?;
        Ok(())
    }
}
