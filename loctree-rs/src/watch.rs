//! Watch mode for live snapshot refresh during iterative development.
//!
//! This module provides file system watching capabilities that:
//! - Monitor file changes in real-time
//! - Debounce changes to avoid thrashing (500ms default)
//! - Reuse unchanged file analyses and rebuild all cross-file derived facts
//! - Respect existing ignore patterns (.gitignore, .loctreeignore)
//! - Allow graceful shutdown via Ctrl+C

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, new_debouncer};

use crate::analyzer::root_scan::RawAnalysisCache;
use crate::args::ParsedArgs;
use crate::fs_utils::GitIgnoreChecker;
use crate::snapshot::{self, Snapshot};

/// Watch configuration
pub struct WatchConfig {
    /// Paths to watch
    pub roots: Vec<PathBuf>,
    /// Debounce duration (default: 500ms)
    pub debounce_duration: Duration,
    /// File extensions to watch (empty = all)
    pub extensions: Option<Vec<String>>,
    /// Gitignore checker for filtering
    pub gitignore: Option<GitIgnoreChecker>,
    /// Optional hook invoked after every successful snapshot save (initial
    /// scan, incremental patch, fallback rescan).
    ///
    /// Used by long-lived co-processes that mirror the snapshot — `loct watch
    /// --report` re-renders `.loctree/report.html` here, future MCP surfaces
    /// can push a `snapshot_updated` event from the same callback. The hook
    /// is owned by the watch loop, so it must be `Send` but does not need to
    /// be `Sync`; rate-limiting is the hook's own responsibility.
    pub on_snapshot_updated: Option<Box<dyn FnMut() + Send>>,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            roots: vec![PathBuf::from(".")],
            debounce_duration: Duration::from_millis(500),
            extensions: None,
            gitignore: None,
            on_snapshot_updated: None,
        }
    }
}

/// Parsed file facts only; derived relationships always come from the canonical pipeline.
struct ScanInfra {
    all_roots_canon: Vec<PathBuf>,
    snapshot_root: PathBuf,
    raw_cache: RawAnalysisCache,
    git_context: snapshot::GitContext,
}

/// Stats from a single patch operation.
struct PatchStats {
    updated: usize,
    added: usize,
    deleted: usize,
}

/// Start watching for file changes and trigger re-scans.
///
/// Callers MUST acquire `crate::watch_lock::WatchLock` before invoking this
/// function and keep the guard alive until it returns. The lock guarantees
/// at most one `--watch` per repository.
pub fn watch_and_rescan(mut config: WatchConfig, parsed_args: &ParsedArgs) -> anyhow::Result<()> {
    let (tx, rx) = channel();

    // Detach the snapshot-updated hook from the config so the rest of the
    // function can keep borrowing `&config.roots` / `&config.extensions`
    // immutably while we call `hook()` with a separate `&mut`.
    let mut on_snapshot_updated = config.on_snapshot_updated.take();
    let mut fire_snapshot_updated = |label: &str| {
        if let Some(hook) = on_snapshot_updated.as_mut() {
            (hook)();
            let _ = label; // reserved for verbose tracing in a future cut
        }
    };

    // Create debouncer with specified duration
    let mut debouncer = new_debouncer(
        config.debounce_duration,
        None, // No separate tick rate
        move |result: DebounceEventResult| {
            if let Err(e) = tx.send(result) {
                eprintln!("[watch] Error sending event: {e}");
            }
        },
    )?;

    // Add paths to watch
    for root in &config.roots {
        debouncer
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| anyhow::anyhow!("Failed to watch {}: {}", root.display(), e))?;
    }

    // Perform initial full scan
    eprintln!("[watch] Initial scan...");
    let start = Instant::now();
    let mut infra = setup_scan_infra(&config.roots, parsed_args)?;
    let mut snap = snapshot::build_snapshot_with_cache(
        &infra.all_roots_canon,
        parsed_args,
        false,
        snapshot::SnapshotRootStrategy::Project,
        true,
        Some(&infra.raw_cache),
    )?;

    let initial_count = snap.files.len();
    eprintln!(
        "[watch] [OK] Scanned {} files in {:.2}s",
        initial_count,
        start.elapsed().as_secs_f64()
    );
    fire_snapshot_updated("initial");

    // Print watching status
    let timestamp = chrono::Local::now().format("%H:%M:%S");
    eprintln!("[{}] Watching {} files...", timestamp, initial_count);
    eprintln!("[watch] Press Ctrl+C to exit");

    // Watch loop
    loop {
        match rx.recv() {
            Ok(Ok(events)) => {
                // Filter events to only those we care about
                let changed_paths =
                    collect_changed_paths(&events, &config.extensions, &config.gitignore);

                if changed_paths.is_empty() {
                    continue;
                }

                // Print what changed
                let timestamp = chrono::Local::now().format("%H:%M:%S");
                if changed_paths.len() == 1 {
                    eprintln!(
                        "[{}] Changed: {}",
                        timestamp,
                        changed_paths.iter().next().unwrap().display()
                    );
                } else {
                    eprintln!("[{}] Changed {} files", timestamp, changed_paths.len());
                }

                let start = Instant::now();

                // Incremental patch
                match patch_snapshot(&mut snap, &mut infra, &changed_paths, parsed_args) {
                    Ok(stats) => {
                        // Save patched snapshot
                        if let Err(e) = snap.save(&infra.snapshot_root) {
                            eprintln!("[watch] Failed to save snapshot: {e}");
                            continue;
                        }

                        let elapsed = start.elapsed();
                        let action = if stats.deleted > 0 {
                            format!("{} updated, {} deleted", stats.updated, stats.deleted)
                        } else if stats.added > 0 {
                            format!("{} updated, {} added", stats.updated, stats.added)
                        } else {
                            format!("{} updated", stats.updated)
                        };
                        eprintln!(
                            "[{}] [OK] Patched {} ({} files total) in {:.2}s",
                            chrono::Local::now().format("%H:%M:%S"),
                            action,
                            snap.files.len(),
                            elapsed.as_secs_f64()
                        );
                        fire_snapshot_updated("patch");
                    }
                    Err(e) => {
                        // Patch failed — fall back to full rescan
                        eprintln!(
                            "[watch] Patch failed ({}), falling back to full rescan...",
                            e
                        );
                        infra
                            .raw_cache
                            .lock()
                            .map_err(|_| anyhow::anyhow!("raw analysis cache poisoned"))?
                            .clear();
                        match snapshot::build_snapshot_with_cache(
                            &infra.all_roots_canon,
                            parsed_args,
                            true,
                            snapshot::SnapshotRootStrategy::Project,
                            true,
                            Some(&infra.raw_cache),
                        ) {
                            Ok(fresh) => {
                                snap = fresh;
                                eprintln!(
                                    "[{}] [OK] Full rescan: {} files in {:.2}s",
                                    chrono::Local::now().format("%H:%M:%S"),
                                    snap.files.len(),
                                    start.elapsed().as_secs_f64()
                                );
                                fire_snapshot_updated("fallback-full");
                            }
                            Err(e3) => {
                                eprintln!("[watch] Full re-scan also failed: {e3}");
                            }
                        }
                    }
                }
            }
            Ok(Err(errors)) => {
                for error in errors {
                    eprintln!("[watch] Error: {error}");
                }
            }
            Err(e) => {
                eprintln!("[watch] Watch error: {e}");
                break;
            }
        }
    }

    Ok(())
}

/// Create scan infrastructure once — reused for every patch in the watch loop.
fn setup_scan_infra(roots: &[PathBuf], _parsed_args: &ParsedArgs) -> anyhow::Result<ScanInfra> {
    // Canonicalize all roots for multi-root strip_prefix
    let all_roots_canon: Vec<PathBuf> = roots
        .iter()
        .map(|r| r.canonicalize().unwrap_or_else(|_| r.clone()))
        .collect();

    let snapshot_root = snapshot::resolve_snapshot_root(roots);

    let git_context = Snapshot::git_context_for(&snapshot_root);

    Ok(ScanInfra {
        all_roots_canon,
        snapshot_root,
        raw_cache: RawAnalysisCache::default(),
        git_context,
    })
}

/// Incrementally patch a snapshot for changed files.
///
/// Reuse unchanged, pre-finalization analyses, then run canonical reduction.
/// Publish only a complete replacement; a failed build leaves the old snapshot intact.
fn patch_snapshot(
    snapshot: &mut Snapshot,
    infra: &mut ScanInfra,
    changed_paths: &HashSet<PathBuf>,
    parsed_args: &ParsedArgs,
) -> anyhow::Result<PatchStats> {
    // Branch changes invalidate parsing context, not merely the metadata stamp.
    let fresh_git = Snapshot::git_context_for(&infra.snapshot_root);
    if fresh_git.commit != infra.git_context.commit || fresh_git.branch != infra.git_context.branch
    {
        infra
            .raw_cache
            .lock()
            .map_err(|_| anyhow::anyhow!("raw analysis cache poisoned"))?
            .clear();
        infra.git_context = fresh_git;
    }
    let mut stats = PatchStats {
        updated: 0,
        added: 0,
        deleted: 0,
    };

    let mut invalidated = HashSet::new();
    let mut structural = false;
    let mut configuration = false;
    for path in changed_paths {
        // Compute relative path — try all roots for strip_prefix (multi-root support).
        // For deleted files, canonicalize the parent dir (file no longer exists).
        let canon = path.canonicalize().unwrap_or_else(|_| {
            path.parent()
                .and_then(|p| p.canonicalize().ok())
                .map(|p| p.join(path.file_name().unwrap_or_default()))
                .unwrap_or_else(|| path.clone())
        });
        let rel_path = infra
            .all_roots_canon
            .iter()
            .find_map(|root| {
                canon
                    .strip_prefix(root)
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| path.to_string_lossy().to_string())
            .replace('\\', "/");

        let existed = snapshot.files.iter().any(|f| f.path == rel_path);
        if !path.exists() && existed {
            stats.deleted += 1;
        } else if path.is_file() && existed {
            stats.updated += 1;
        } else if path.is_file() {
            stats.added += 1;
        }
        structural |= !existed || !path.is_file();
        // Resolver/configuration inputs can change facts inside unchanged files.
        configuration |= matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("json" | "jsonc" | "json5" | "toml" | "cfg" | "ini" | "yaml" | "yml")
        ) || matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".gitignore" | ".loctignore" | ".loctreeignore" | "setup.py")
        );
        invalidated.insert(canon);
    }

    {
        let mut cache = infra
            .raw_cache
            .lock()
            .map_err(|_| anyhow::anyhow!("raw analysis cache poisoned"))?;
        let before = cache.len();
        cache.retain(|(_, path), analysis| {
            !configuration && path.is_file()
                && !invalidated.iter().any(|changed| path.starts_with(changed))
                // Parse-time import resolution depends on the file universe.
                && !(structural && (!analysis.imports.is_empty() || !analysis.reexports.is_empty()
                    || !analysis.cargo_targets.is_empty()))
        });
        if parsed_args.verbose {
            eprintln!(
                "[watch] Invalidated {} parsed file(s); configuration={}, structural={}",
                before - cache.len(),
                configuration,
                structural
            );
        }
    }
    *snapshot = snapshot::build_snapshot_with_cache(
        &infra.all_roots_canon,
        parsed_args,
        true,
        snapshot::SnapshotRootStrategy::Project,
        false,
        Some(&infra.raw_cache),
    )?;

    Ok(stats)
}

/// Collect paths that changed from debounced events
fn collect_changed_paths(
    events: &[notify_debouncer_full::DebouncedEvent],
    extensions: &Option<Vec<String>>,
    gitignore: &Option<GitIgnoreChecker>,
) -> HashSet<PathBuf> {
    let mut paths = HashSet::new();

    for event in events {
        for path in &event.paths {
            // Skip if gitignored
            if let Some(checker) = gitignore
                && checker.is_ignored(path)
            {
                continue;
            }

            // Skip if wrong extension
            if let Some(exts) = extensions {
                let has_matching_ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|ext_str| exts.iter().any(|e| e == ext_str));
                if !has_matching_ext {
                    continue;
                }
            }

            // Skip directories
            if path.is_dir() {
                continue;
            }

            paths.insert(path.clone());
        }
    }

    paths
}

/// Count files that would be tracked (used only by external callers, not watch loop)
pub fn count_tracked_files(
    roots: &[PathBuf],
    extensions: &Option<Vec<String>>,
    gitignore: &Option<GitIgnoreChecker>,
) -> usize {
    let mut count = 0;

    for root in roots {
        if let Ok(walker) = walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
        {
            for entry in walker {
                if !entry.file_type().is_file() {
                    continue;
                }

                let path = entry.path();

                // Skip if gitignored
                if let Some(checker) = gitignore
                    && checker.is_ignored(path)
                {
                    continue;
                }

                // Skip if wrong extension
                if let Some(exts) = extensions {
                    let has_matching_ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|ext_str| exts.iter().any(|e| e == ext_str));
                    if !has_matching_ext {
                        continue;
                    }
                }

                count += 1;
            }
        }
    }

    count
}

#[cfg(test)]
#[path = "watch_parity_tests.rs"]
mod parity_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_watch_config_defaults() {
        let config = WatchConfig::default();
        assert_eq!(config.roots, vec![PathBuf::from(".")]);
        assert_eq!(config.debounce_duration, Duration::from_millis(500));
        assert!(config.extensions.is_none());
        assert!(config.gitignore.is_none());
    }

    #[test]
    fn test_watch_config_custom() {
        let config = WatchConfig {
            roots: vec![PathBuf::from("src"), PathBuf::from("tests")],
            debounce_duration: Duration::from_millis(1000),
            extensions: Some(vec!["ts".to_string(), "tsx".to_string()]),
            gitignore: None,
            on_snapshot_updated: None,
        };

        assert_eq!(config.roots.len(), 2);
        assert_eq!(config.debounce_duration, Duration::from_millis(1000));
        assert!(config.extensions.is_some());
        assert_eq!(config.extensions.unwrap(), vec!["ts", "tsx"]);
    }

    #[test]
    fn test_count_tracked_files() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();

        fs::write(temp.path().join("test1.ts"), "").unwrap();
        fs::write(temp.path().join("test2.ts"), "").unwrap();
        fs::write(temp.path().join("test3.js"), "").unwrap();
        fs::write(temp.path().join("readme.txt"), "").unwrap();

        let count = count_tracked_files(std::slice::from_ref(&root), &None, &None);
        assert_eq!(count, 4);

        let extensions = Some(vec!["ts".to_string()]);
        let count_filtered = count_tracked_files(std::slice::from_ref(&root), &extensions, &None);
        assert_eq!(count_filtered, 2);
    }

    #[test]
    fn test_count_tracked_files_empty_directory() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let count = count_tracked_files(&[root], &None, &None);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_count_tracked_files_nested() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let subdir = temp.path().join("subdir");
        fs::create_dir(&subdir).unwrap();

        fs::write(temp.path().join("root.ts"), "").unwrap();
        fs::write(subdir.join("nested.ts"), "").unwrap();

        let count = count_tracked_files(&[root], &None, &None);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_count_tracked_files_multiple_roots() {
        let temp1 = TempDir::new().unwrap();
        let temp2 = TempDir::new().unwrap();
        let root1 = temp1.path().to_path_buf();
        let root2 = temp2.path().to_path_buf();

        fs::write(temp1.path().join("file1.ts"), "").unwrap();
        fs::write(temp2.path().join("file2.ts"), "").unwrap();

        let count = count_tracked_files(&[root1, root2], &None, &None);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_count_tracked_files_with_extension_filter() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();

        fs::write(temp.path().join("file.ts"), "").unwrap();
        fs::write(temp.path().join("file.js"), "").unwrap();
        fs::write(temp.path().join("file.tsx"), "").unwrap();
        fs::write(temp.path().join("readme.md"), "").unwrap();

        let extensions = Some(vec!["ts".to_string(), "tsx".to_string()]);
        let count = count_tracked_files(&[root], &extensions, &None);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_count_tracked_files_ignores_subdirectories() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let subdir = temp.path().join("subdir");
        fs::create_dir(&subdir).unwrap();

        fs::write(temp.path().join("file.ts"), "").unwrap();

        let count = count_tracked_files(&[root], &None, &None);
        assert_eq!(count, 1);
    }

    #[test]
    fn test_debounce_duration_custom() {
        let config = WatchConfig {
            debounce_duration: Duration::from_millis(250),
            ..Default::default()
        };
        assert_eq!(config.debounce_duration, Duration::from_millis(250));
    }

    #[test]
    fn test_watch_config_with_gitignore() {
        let config = WatchConfig {
            gitignore: None,
            ..Default::default()
        };
        assert!(config.gitignore.is_none());
    }

    #[test]
    fn test_watch_config_with_multiple_extensions() {
        let config = WatchConfig {
            extensions: Some(vec![
                "ts".to_string(),
                "tsx".to_string(),
                "js".to_string(),
                "jsx".to_string(),
            ]),
            ..Default::default()
        };

        assert_eq!(config.extensions.unwrap().len(), 4);
    }

    #[test]
    fn test_patch_snapshot_updates_existing_file() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let root_canon = root.canonicalize().unwrap();

        // Create a simple TS file
        let file = root.join("src").join("app.ts");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "export function hello() { return 42; }").unwrap();

        // Build minimal snapshot with one existing file
        let mut snap = Snapshot::new(vec![]);
        snap.files.push(crate::types::FileAnalysis {
            path: "src/app.ts".to_string(),
            ..Default::default()
        });

        let mut infra = ScanInfra {
            all_roots_canon: vec![root_canon.clone()],
            snapshot_root: root.clone(),
            raw_cache: RawAnalysisCache::default(),
            git_context: snapshot::GitContext {
                repo: None,
                owner_repo: None,
                branch: None,
                commit: None,
                scan_id: None,
            },
        };

        let mut changed = HashSet::new();
        changed.insert(file);

        let parsed = ParsedArgs {
            force_non_git: true,
            ..Default::default()
        };
        let stats = patch_snapshot(&mut snap, &mut infra, &changed, &parsed).unwrap();

        assert_eq!(stats.updated, 1, "should count as updated");
        assert_eq!(stats.added, 0, "existing file should not be added");
        assert_eq!(stats.deleted, 0);
        assert_eq!(snap.files.len(), 1);
        // File should have been re-analyzed (exports detected)
        assert!(!snap.files[0].exports.is_empty() || snap.files[0].loc > 0);
    }

    #[test]
    fn test_patch_snapshot_adds_new_file() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let root_canon = root.canonicalize().unwrap();

        // Create a new TS file (not in snapshot)
        let file = root.join("src").join("new.ts");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "export const x = 1;").unwrap();

        let mut snap = Snapshot::new(vec![]);

        let mut infra = ScanInfra {
            all_roots_canon: vec![root_canon.clone()],
            snapshot_root: root.clone(),
            raw_cache: RawAnalysisCache::default(),
            git_context: snapshot::GitContext {
                repo: None,
                owner_repo: None,
                branch: None,
                commit: None,
                scan_id: None,
            },
        };

        let mut changed = HashSet::new();
        changed.insert(file);

        let parsed = ParsedArgs {
            force_non_git: true,
            ..Default::default()
        };
        let stats = patch_snapshot(&mut snap, &mut infra, &changed, &parsed).unwrap();

        assert_eq!(stats.added, 1, "new file should be added");
        assert_eq!(stats.updated, 0, "new file should NOT count as updated");
        assert_eq!(snap.files.len(), 1);
        assert_eq!(snap.files[0].path, "src/new.ts");
    }

    #[test]
    fn test_patch_snapshot_deletes_file() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let root_canon = root.canonicalize().unwrap();

        // File doesn't exist on disk but is in snapshot.
        // Create the parent dir so canonicalize() works on the parent.
        let src_dir = root.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let file = src_dir.join("deleted.ts");

        let mut snap = Snapshot::new(vec![]);
        snap.files.push(crate::types::FileAnalysis {
            path: "src/deleted.ts".to_string(),
            ..Default::default()
        });
        snap.edges.push(snapshot::GraphEdge {
            from: "src/deleted.ts".to_string(),
            to: "src/other.ts".to_string(),
            label: "import".to_string(),
        });

        let mut infra = ScanInfra {
            all_roots_canon: vec![root_canon.clone()],
            snapshot_root: root.clone(),
            raw_cache: RawAnalysisCache::default(),
            git_context: snapshot::GitContext {
                repo: None,
                owner_repo: None,
                branch: None,
                commit: None,
                scan_id: None,
            },
        };

        let mut changed = HashSet::new();
        changed.insert(file);

        let parsed = ParsedArgs {
            force_non_git: true,
            ..Default::default()
        };
        let stats = patch_snapshot(&mut snap, &mut infra, &changed, &parsed).unwrap();

        assert_eq!(stats.deleted, 1);
        assert_eq!(stats.updated, 0);
        assert!(snap.files.is_empty(), "deleted file should be removed");
        assert!(
            snap.edges.is_empty(),
            "edges from deleted file should be removed"
        );
    }

    #[test]
    fn test_export_index_excludes_test_fixtures() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let root_canon = root.canonicalize().unwrap();

        // Create a test fixture file and a real source file
        let fixture = root.join("tests").join("fixtures").join("fixture.ts");
        let source = root.join("src").join("utils.ts");
        fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&fixture, "export function testHelper() {}").unwrap();
        fs::write(&source, "export function realUtil() {}").unwrap();

        let mut snap = Snapshot::new(vec![]);

        let mut infra = ScanInfra {
            all_roots_canon: vec![root_canon.clone()],
            snapshot_root: root.clone(),
            raw_cache: RawAnalysisCache::default(),
            git_context: snapshot::GitContext {
                repo: None,
                owner_repo: None,
                branch: None,
                commit: None,
                scan_id: None,
            },
        };

        let mut changed = HashSet::new();
        changed.insert(fixture);
        changed.insert(source);

        let parsed = ParsedArgs {
            force_non_git: true,
            ..Default::default()
        };
        patch_snapshot(&mut snap, &mut infra, &changed, &parsed).unwrap();

        // The fixture's export should NOT be in the export_index
        assert!(
            !snap.export_index.contains_key("testHelper"),
            "test fixture exports should be excluded from export_index"
        );
        // The real source's export SHOULD be in the export_index
        assert!(
            snap.export_index.contains_key("realUtil"),
            "real source exports should be in export_index"
        );
    }

    /// Regression for loctree-feedback hak 2026-05-23 #4 (L9 closure): when the
    /// cached `infra.git_context.commit` differs from the live git HEAD,
    /// `patch_snapshot` MUST refresh the cache and stamp the snapshot with
    /// the fresh commit. Before this fix, every incremental patch between
    /// two periodic full-rescans stamped the stale daemon-start commit
    /// even though file contents and `generated_at` were fresh.
    #[test]
    fn test_patch_snapshot_refreshes_stale_git_commit() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let root_canon = root.canonicalize().unwrap();

        let file = root.join("src").join("app.ts");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "export const x = 1;").unwrap();

        let mut snap = Snapshot::new(vec![]);
        snap.files.push(crate::types::FileAnalysis {
            path: "src/app.ts".to_string(),
            ..Default::default()
        });

        // Seed cached git context with an obviously stale commit value.
        // Whether the temp dir is or is not a git repo, the live result
        // of `git_context_for` will differ from this placeholder
        // ("stale-deadbeef"), so the refresh branch in `patch_snapshot`
        // is exercised regardless of test-host git availability.
        let mut infra = ScanInfra {
            all_roots_canon: vec![root_canon.clone()],
            snapshot_root: root.clone(),
            raw_cache: RawAnalysisCache::default(),
            git_context: snapshot::GitContext {
                repo: Some("stale-repo".to_string()),
                owner_repo: Some("stale/stale-repo".to_string()),
                branch: Some("stale-branch".to_string()),
                commit: Some("stale-deadbeef".to_string()),
                scan_id: Some("stale-branch@stale-deadbeef".to_string()),
            },
        };

        let mut changed = HashSet::new();
        changed.insert(file);

        let parsed = ParsedArgs {
            force_non_git: true,
            ..Default::default()
        };
        patch_snapshot(&mut snap, &mut infra, &changed, &parsed).unwrap();

        // After patch, infra MUST have been refreshed off the placeholder.
        assert_ne!(
            infra.git_context.commit.as_deref(),
            Some("stale-deadbeef"),
            "patch_snapshot must refresh the cached git_context off the daemon-start value"
        );
        // And the snapshot metadata MUST match the (now refreshed) cache,
        // never the stale placeholder.
        assert_ne!(
            snap.metadata.git_commit.as_deref(),
            Some("stale-deadbeef"),
            "patch_snapshot must NOT stamp the snapshot with the stale daemon-start commit"
        );
        assert_eq!(
            snap.metadata.git_commit, infra.git_context.commit,
            "snapshot metadata commit must mirror the refreshed cache"
        );
    }

    #[test]
    fn test_multi_root_strip_prefix() {
        // Test that path resolution correctly strips prefix from multiple roots
        // (without calling analyze_file, which validates single-root)
        let temp1 = TempDir::new().unwrap();
        let temp2 = TempDir::new().unwrap();
        let root1_canon = temp1.path().canonicalize().unwrap();
        let root2_canon = temp2.path().canonicalize().unwrap();

        let all_roots = [root1_canon.clone(), root2_canon.clone()];

        // File under root1
        let file1 = root1_canon.join("src").join("app.ts");
        let rel1 = all_roots
            .iter()
            .find_map(|root| {
                file1
                    .strip_prefix(root)
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
            })
            .unwrap();
        assert_eq!(rel1, "src/app.ts");

        // File under root2
        let file2 = root2_canon.join("lib").join("utils.ts");
        let rel2 = all_roots
            .iter()
            .find_map(|root| {
                file2
                    .strip_prefix(root)
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
            })
            .unwrap();
        assert_eq!(rel2, "lib/utils.ts");
    }

    #[test]
    fn patch_preserves_type_edges_and_replaces_them_when_values_change() {
        for (ext, type_a, type_b, value_a, value_b) in [
            (
                "ts",
                "import type { B } from './b.ts'; export interface A { b: B }",
                "import type { A } from './a.ts'; export interface B { a: A }",
                "import { b } from './b.ts'; export const a = b + 1;",
                "import { a } from './a.ts'; export const b = a + 1;",
            ),
            (
                "py",
                "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from b import B\nclass A: pass\n",
                "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from a import A\nclass B: pass\n",
                "from b import b\ndef a(): return b()\n",
                "from a import a\ndef b(): return a()\n",
            ),
        ] {
            let temp = TempDir::new().expect("fixture");
            let root = temp.path().canonicalize().expect("canonical root");
            let a = root.join(format!("a.{ext}"));
            let b = root.join(format!("b.{ext}"));
            let paths = HashSet::from([a.clone(), b.clone()]);
            let parsed = ParsedArgs {
                force_non_git: true,
                ..Default::default()
            };
            let mut infra = setup_scan_infra(std::slice::from_ref(&root), &parsed)
                .expect("watch infrastructure");
            let mut snap = Snapshot::new(vec![root.display().to_string()]);
            for (source_a, source_b, label, cycle_count) in [
                (type_a, type_b, "type_import", 0),
                (value_a, value_b, "import", 1),
                (type_a, type_b, "type_import", 0),
            ] {
                fs::write(&a, source_a).expect("a source");
                fs::write(&b, source_b).expect("b source");
                patch_snapshot(&mut snap, &mut infra, &paths, &parsed).expect("patch");
                assert_eq!(snap.edges.len(), 2, "{ext}: edges must be replaced");
                assert!(snap.edges.iter().all(|e| e.label == label), "{ext}");
                let edges = snap
                    .edges
                    .iter()
                    .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
                    .collect::<Vec<_>>();
                let (strict, lazy) = crate::analyzer::cycles::find_cycles_with_lazy(&edges);
                assert_eq!(strict.len(), cycle_count, "{ext}");
                assert!(lazy.is_empty());
            }
        }
    }
}
