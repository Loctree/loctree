//! Watch mode for live snapshot refresh during iterative development.
//!
//! This module provides file system watching capabilities that:
//! - Monitor file changes in real-time
//! - Debounce changes to avoid thrashing (500ms default)
//! - **Incrementally patch** the snapshot for changed files only (no full rescan)
//! - Respect existing ignore patterns (.gitignore, .loctreeignore)
//! - Periodically do a full rescan to keep bridges, barrels, etc. fresh
//! - Allow graceful shutdown via Ctrl+C

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant, UNIX_EPOCH};

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, new_debouncer};

use crate::analyzer::ast_js::CommandDetectionConfig;
use crate::analyzer::resolvers::{
    TsPathResolver, resolve_js_relative, resolve_python_absolute, resolve_python_relative,
    resolve_rust_import,
};
use crate::analyzer::runner::default_analyzer_exts;
use crate::analyzer::scan::{AnalyzeContext, analyze_file, python_stdlib};
use crate::args::ParsedArgs;
use crate::config::LoctreeConfig;
use crate::fs_utils::GitIgnoreChecker;
use crate::snapshot::{self, GraphEdge, Snapshot};
use crate::types::ImportKind;

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
    /// scan, periodic full rescan, incremental patch, fallback rescan).
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

/// Cached scan infrastructure — created once at watch start, reused for every patch.
///
/// Caches git context to avoid spawning 3 git subprocesses on every patch.
/// Git context is refreshed on periodic full rescans (every [`FULL_RESCAN_INTERVAL`] changes).
struct ScanInfra {
    /// Primary root (first root, used for analysis)
    root_canon: PathBuf,
    /// All roots canonicalized (for multi-root strip_prefix)
    all_roots_canon: Vec<PathBuf>,
    snapshot_root: PathBuf,
    extensions: Option<HashSet<String>>,
    ts_resolver: Option<TsPathResolver>,
    py_roots: Vec<PathBuf>,
    py_stdlib: HashSet<String>,
    custom_command_macros: Vec<String>,
    command_detection: CommandDetectionConfig,
    /// Cached git context (commit, branch, scan_id). Refreshed on full rescan.
    git_context: snapshot::GitContext,
}

/// Stats from a single patch operation.
struct PatchStats {
    updated: usize,
    added: usize,
    deleted: usize,
}

/// How many patches before we do a full rescan (to refresh bridges, barrels, etc.)
const FULL_RESCAN_INTERVAL: usize = 50;

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
    if let Err(e) = snapshot::run_init(&config.roots, parsed_args) {
        eprintln!("[watch] Initial scan failed: {e}");
        return Err(anyhow::anyhow!("Initial scan failed: {e}"));
    }

    // Load snapshot into memory and set up scan infrastructure
    let mut infra = setup_scan_infra(&config.roots, parsed_args)?;
    let mut snap = Snapshot::load(&infra.snapshot_root)
        .map_err(|e| anyhow::anyhow!("Failed to load snapshot after initial scan: {e}"))?;

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

    let mut patch_count: usize = 0;

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

                // Decide: incremental patch or full rescan
                patch_count += 1;
                if patch_count >= FULL_RESCAN_INTERVAL {
                    // Periodic full rescan to refresh bridges, barrels, etc.
                    eprintln!(
                        "[watch] Periodic full rescan (every {} changes)...",
                        FULL_RESCAN_INTERVAL
                    );
                    if let Err(e) = snapshot::run_init(&config.roots, parsed_args) {
                        eprintln!("[watch] Full re-scan failed: {e}");
                        continue;
                    }
                    match Snapshot::load(&infra.snapshot_root) {
                        Ok(fresh) => {
                            let file_count = fresh.files.len();
                            snap = fresh;
                            patch_count = 0;
                            eprintln!(
                                "[{}] [OK] Full rescan: {} files in {:.2}s",
                                chrono::Local::now().format("%H:%M:%S"),
                                file_count,
                                start.elapsed().as_secs_f64()
                            );
                            fire_snapshot_updated("periodic-full");
                        }
                        Err(e) => {
                            eprintln!("[watch] Failed to reload snapshot: {e}");
                        }
                    }
                    continue;
                }

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
                        if let Err(e2) = snapshot::run_init(&config.roots, parsed_args) {
                            eprintln!("[watch] Full re-scan also failed: {e2}");
                            continue;
                        }
                        match Snapshot::load(&infra.snapshot_root) {
                            Ok(fresh) => {
                                snap = fresh;
                                patch_count = 0;
                                eprintln!(
                                    "[{}] [OK] Full rescan: {} files in {:.2}s",
                                    chrono::Local::now().format("%H:%M:%S"),
                                    snap.files.len(),
                                    start.elapsed().as_secs_f64()
                                );
                                fire_snapshot_updated("fallback-full");
                            }
                            Err(e3) => {
                                eprintln!("[watch] Failed to reload snapshot: {e3}");
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
fn setup_scan_infra(roots: &[PathBuf], parsed_args: &ParsedArgs) -> anyhow::Result<ScanInfra> {
    let root = roots.first().cloned().unwrap_or_else(|| PathBuf::from("."));
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.clone());

    // Canonicalize all roots for multi-root strip_prefix
    let all_roots_canon: Vec<PathBuf> = roots
        .iter()
        .map(|r| r.canonicalize().unwrap_or_else(|_| r.clone()))
        .collect();

    let snapshot_root = snapshot::resolve_snapshot_root(roots);

    // Extensions: use parsed_args or defaults
    let extensions = parsed_args
        .extensions
        .clone()
        .or_else(|| Some(default_analyzer_exts()));

    // TypeScript path resolver
    let ts_resolver = TsPathResolver::from_tsconfig(&root_canon);

    // Python roots
    let py_roots = crate::analyzer::root_scan::build_py_roots(&root_canon, &parsed_args.py_roots);
    let py_stdlib = python_stdlib();

    // Tauri/command detection config
    let loctree_config = LoctreeConfig::load(&root);
    let custom_command_macros = loctree_config.tauri.command_macros;
    let command_detection = CommandDetectionConfig::new(
        &loctree_config.tauri.dom_exclusions,
        &loctree_config.tauri.non_invoke_exclusions,
        &loctree_config.tauri.invalid_command_names,
    );

    // Cache git context once (avoids spawning 3 git subprocesses per patch)
    let git_context = Snapshot::git_context_for(&snapshot_root);

    Ok(ScanInfra {
        root_canon,
        all_roots_canon,
        snapshot_root,
        extensions,
        ts_resolver,
        py_roots,
        py_stdlib,
        custom_command_macros,
        command_detection,
        git_context,
    })
}

/// Incrementally patch a snapshot for changed files.
///
/// Only re-analyzes files in `changed_paths`. Deleted files are removed.
/// Edges from changed files are recalculated. Export index is rebuilt.
///
/// **Note:** `command_bridges`, `event_bridges`, and `barrels` are NOT updated
/// during incremental patches — they require cross-file correlation that only
/// the full `run_init()` pipeline performs. These are refreshed on periodic
/// full rescans (every [`FULL_RESCAN_INTERVAL`] changes).
///
/// This is ~30-40x faster than a full rescan for single-file changes.
fn patch_snapshot(
    snapshot: &mut Snapshot,
    infra: &mut ScanInfra,
    changed_paths: &HashSet<PathBuf>,
    parsed_args: &ParsedArgs,
) -> anyhow::Result<PatchStats> {
    // loctree-feedback hak 2026-05-23 #4 (L9 closure): the watch daemon used to
    // cache `git_context` once during setup and stamp every incremental
    // patch with that frozen commit hash. Commits landing between the
    // 50-patch full-rescan boundaries left snapshots with fresh files but
    // a stale `git_commit` — breaking `vc-init` and MCP `doctor()`
    // fingerprint comparisons. Re-resolve git context per patch so the
    // commit hash always matches the working tree at stamp time. The
    // call is three cheap `git rev-parse` invocations; cost is negligible
    // versus the analyzer work that follows.
    let fresh_git = Snapshot::git_context_for(&infra.snapshot_root);
    if fresh_git.commit != infra.git_context.commit || fresh_git.branch != infra.git_context.branch
    {
        infra.git_context = fresh_git;
    }
    let mut stats = PatchStats {
        updated: 0,
        added: 0,
        deleted: 0,
    };

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

        // Skip directories (debouncer may report dir events)
        if path.is_dir() {
            continue;
        }

        // Check if file was deleted
        if !path.exists() {
            let before = snapshot.files.len();
            snapshot.files.retain(|f| f.path != rel_path);
            snapshot
                .edges
                .retain(|e| e.from != rel_path && e.to != rel_path);
            if snapshot.files.len() < before {
                stats.deleted += 1;
            }
            continue;
        }

        // Re-analyze the file
        let analyze_ctx = AnalyzeContext {
            root_canon: &infra.root_canon,
            extensions: infra.extensions.as_ref(),
            ts_resolver: infra.ts_resolver.as_ref(),
            py_roots: &infra.py_roots,
            py_stdlib: &infra.py_stdlib,
            symbol: parsed_args.symbol.as_deref(),
            custom_command_macros: &infra.custom_command_macros,
            command_cfg: &infra.command_detection,
        };
        let analysis = match analyze_file(path, &analyze_ctx) {
            Ok(mut a) => {
                // Set mtime + size for future cache hits
                if let Ok(metadata) = std::fs::metadata(path) {
                    a.mtime = metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    a.size = metadata.len();
                }
                a
            }
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                // Binary file — skip
                continue;
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to analyze {}: {}",
                    path.display(),
                    e
                ));
            }
        };

        // Remove old edges FROM this file
        snapshot.edges.retain(|e| e.from != rel_path);

        // Build new edges from the re-analyzed imports
        let file_ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        for imp in &analysis.imports {
            if imp.is_type_checking {
                continue;
            }

            let resolved = imp
                .resolved_path
                .clone()
                .or_else(|| match file_ext.as_str() {
                    "py" => {
                        if imp.source.starts_with('.') {
                            resolve_python_relative(
                                &imp.source,
                                path,
                                &infra.root_canon,
                                infra.extensions.as_ref(),
                            )
                        } else {
                            resolve_python_absolute(
                                &imp.source,
                                &infra.py_roots,
                                &infra.root_canon,
                                infra.extensions.as_ref(),
                            )
                        }
                    }
                    "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "css" | "svelte" | "vue"
                    | "astro" => {
                        if imp.source.starts_with('.') {
                            resolve_js_relative(
                                path,
                                &infra.root_canon,
                                &imp.source,
                                infra.extensions.as_ref(),
                            )
                        } else {
                            infra
                                .ts_resolver
                                .as_ref()
                                .and_then(|r| r.resolve(&imp.source, infra.extensions.as_ref()))
                        }
                    }
                    "rs" if imp.is_mod_declaration => {
                        resolve_rust_import(&imp.source, path, &infra.root_canon, &infra.root_canon)
                    }
                    _ => None,
                });

            if let Some(target) = resolved {
                let label = if imp.is_mod_declaration {
                    "mod"
                } else if imp.is_lazy {
                    "lazy_import"
                } else {
                    match imp.kind {
                        ImportKind::Static | ImportKind::Type | ImportKind::SideEffect => "import",
                        ImportKind::Dynamic => "dynamic_import",
                    }
                };

                snapshot.edges.push(GraphEdge {
                    from: rel_path.clone(),
                    to: target,
                    label: label.to_string(),
                });
            }
        }

        // Also handle reexport edges (match root_scan.rs resolution logic)
        for re in &analysis.reexports {
            let resolved = re.resolved.clone().or_else(|| {
                let spec = &re.source;
                if spec.starts_with('.') {
                    resolve_js_relative(path, &infra.root_canon, spec, infra.extensions.as_ref())
                } else if matches!(
                    file_ext.as_str(),
                    "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "svelte" | "vue" | "astro"
                ) {
                    infra
                        .ts_resolver
                        .as_ref()
                        .and_then(|r| r.resolve(spec, infra.extensions.as_ref()))
                } else {
                    None
                }
            });
            if let Some(target) = resolved {
                snapshot.edges.push(GraphEdge {
                    from: rel_path.clone(),
                    to: target,
                    label: "reexport".to_string(),
                });
            }
        }

        // Replace or insert file analysis
        if let Some(existing) = snapshot.files.iter_mut().find(|f| f.path == rel_path) {
            *existing = analysis;
            stats.updated += 1;
        } else {
            snapshot.files.push(analysis);
            stats.added += 1;
        }
    }

    // Rebuild export_index from all files (fast iteration, no I/O).
    // Must match root_scan.rs filtering: exclude test fixtures, .d.ts declarations, reexports.
    snapshot.export_index.clear();
    for file in &snapshot.files {
        // Exclude test fixtures from export index (same filter as root_scan.rs)
        if crate::analyzer::classify::should_exclude_from_reports(&file.path) {
            continue;
        }
        let is_decl = [".d.ts", ".d.tsx", ".d.mts", ".d.cts"]
            .iter()
            .any(|ext| file.path.ends_with(ext));
        for exp in &file.exports {
            if exp.kind == "reexport" {
                continue;
            }
            if exp.export_type == "default" {
                continue;
            }
            // Skip ambient "default" from .d.ts files
            if is_decl && exp.name.to_lowercase() == "default" {
                continue;
            }
            snapshot
                .export_index
                .entry(exp.name.clone())
                .or_default()
                .push(file.path.clone());
        }
    }

    // Update metadata using cached git context (no subprocess spawning)
    snapshot.metadata.git_commit = infra.git_context.commit.clone();
    snapshot.metadata.git_branch = infra.git_context.branch.clone();
    snapshot.metadata.git_scan_id = infra.git_context.scan_id.clone();
    snapshot.metadata.generated_at = chrono::Utc::now().to_rfc3339();
    snapshot.metadata.file_count = snapshot.files.len();
    snapshot.metadata.total_loc = snapshot.files.iter().map(|f| f.loc).sum();

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
            root_canon: root_canon.clone(),
            all_roots_canon: vec![root_canon.clone()],
            snapshot_root: root.clone(),
            extensions: Some(
                crate::analyzer::runner::default_analyzer_exts()
                    .into_iter()
                    .collect(),
            ),
            ts_resolver: None,
            py_roots: vec![],
            py_stdlib: HashSet::new(),
            custom_command_macros: vec![],
            command_detection: CommandDetectionConfig::default(),
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

        let parsed = ParsedArgs::default();
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
            root_canon: root_canon.clone(),
            all_roots_canon: vec![root_canon.clone()],
            snapshot_root: root.clone(),
            extensions: Some(
                crate::analyzer::runner::default_analyzer_exts()
                    .into_iter()
                    .collect(),
            ),
            ts_resolver: None,
            py_roots: vec![],
            py_stdlib: HashSet::new(),
            custom_command_macros: vec![],
            command_detection: CommandDetectionConfig::default(),
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

        let parsed = ParsedArgs::default();
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
            root_canon: root_canon.clone(),
            all_roots_canon: vec![root_canon.clone()],
            snapshot_root: root.clone(),
            extensions: Some(
                crate::analyzer::runner::default_analyzer_exts()
                    .into_iter()
                    .collect(),
            ),
            ts_resolver: None,
            py_roots: vec![],
            py_stdlib: HashSet::new(),
            custom_command_macros: vec![],
            command_detection: CommandDetectionConfig::default(),
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

        let parsed = ParsedArgs::default();
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
            root_canon: root_canon.clone(),
            all_roots_canon: vec![root_canon.clone()],
            snapshot_root: root.clone(),
            extensions: Some(
                crate::analyzer::runner::default_analyzer_exts()
                    .into_iter()
                    .collect(),
            ),
            ts_resolver: None,
            py_roots: vec![],
            py_stdlib: HashSet::new(),
            custom_command_macros: vec![],
            command_detection: CommandDetectionConfig::default(),
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

        let parsed = ParsedArgs::default();
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
            root_canon: root_canon.clone(),
            all_roots_canon: vec![root_canon.clone()],
            snapshot_root: root.clone(),
            extensions: Some(
                crate::analyzer::runner::default_analyzer_exts()
                    .into_iter()
                    .collect(),
            ),
            ts_resolver: None,
            py_roots: vec![],
            py_stdlib: HashSet::new(),
            custom_command_macros: vec![],
            command_detection: CommandDetectionConfig::default(),
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

        let parsed = ParsedArgs::default();
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
}
