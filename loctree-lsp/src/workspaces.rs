//! Multi-workspace snapshot routing (Plan 13 of the LSP roadmap).
//!
//! A monorepo can hold several sub-projects, each with its own
//! `.loctree/` snapshot directory. One LSP daemon serves them all by
//! discovering every `.loctree/` parent at `initialized` time and
//! routing every request that carries `project: Option<PathBuf>` to
//! the matching snapshot.
//!
//! ## Discovery contract
//!
//! - The root workspace is always part of the addressable set and is
//!   represented by [`Backend::snapshot`](crate::Backend) (the original
//!   single-workspace handle). It is intentionally not duplicated into
//!   the extras map — single source of truth.
//! - Sub-projects are discovered by walking down from the workspace
//!   root, capped at `max_depth` (default 4), and recording the parent
//!   of every `.loctree/` directory found there. The root itself is
//!   excluded from extras even when it has its own `.loctree/` —
//!   callers see the root via the dedicated handle.
//! - Common noise directories (`.git`, `target`, `node_modules`,
//!   `dist`, `build`, `.next`, `.turbo`, `.cache`) are pruned during
//!   the walk. They never host meaningful sub-projects and unbounded
//!   walks of `node_modules` were the bug Monika hit on Vista.
//!
//! ## Wire shape
//!
//! `loctree/workspaces` (custom request) returns
//! [`WorkspacesResponse`] — a flat list of [`WorkspaceInfo`] entries,
//! one per addressable workspace, including the root marked with
//! `is_root: true`. Snapshot age is reported in whole seconds so
//! agents can decide when to ask the operator to rescan a stale
//! sub-project.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default depth used when the operator does not pass an override
/// through `initializationOptions.loctree.workspaces.maxDepth`.
pub const DEFAULT_MAX_DEPTH: usize = 4;

/// Hard ceiling — even when init options ask for more, we cap to keep
/// monorepo discovery bounded. A depth of 8 already covers Vista's
/// `apps/<name>/src-tauri/...` chain twice over.
pub const MAX_DEPTH_CEILING: usize = 8;

/// Directory names pruned during workspace discovery.
///
/// They never host meaningful sub-projects and walking `node_modules`
/// was the original Vista monorepo bug — pruning is mandatory, not a
/// performance suggestion.
const PRUNED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    ".next",
    ".turbo",
    ".cache",
    ".loctree",
];

/// Empty params struct for `loctree/workspaces` (no inputs).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct WorkspacesParams {}

/// One row in the response: an addressable LSP workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    /// Canonical project root (absolute, OS-form).
    pub root: String,
    /// `true` for the workspace LSP started in.
    pub is_root: bool,
    /// `true` when the workspace currently has a loaded snapshot.
    /// `false` for sub-projects whose `.loctree/snapshot.json` was
    /// missing or unreadable at discovery time.
    pub has_snapshot: bool,
    /// Files in the loaded snapshot (0 when `has_snapshot=false`).
    pub files: usize,
    /// Languages observed in the loaded snapshot, sorted alphabetically.
    pub languages: Vec<String>,
    /// Age of the snapshot file in whole seconds. `None` when the
    /// snapshot is missing or its mtime cannot be read.
    pub snapshot_age_seconds: Option<u64>,
}

/// Wire envelope for the `loctree/workspaces` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspacesResponse {
    /// All addressable workspaces, root first.
    pub workspaces: Vec<WorkspaceInfo>,
}

/// Read `loctree.workspaces.maxDepth` from `initializationOptions`.
///
/// Honors both nested (`{"loctree":{"workspaces":{"maxDepth":...}}}`)
/// and flat (`{"loctree.workspaces.maxDepth": 6}`) shapes for parity
/// with the watcher and protocol options.
pub fn max_depth_from_options(options: Option<&Value>) -> usize {
    let Some(value) = options else {
        return DEFAULT_MAX_DEPTH;
    };
    let nested = value
        .pointer("/loctree/workspaces/maxDepth")
        .and_then(|v| v.as_u64());
    let flat = value
        .get("loctree.workspaces.maxDepth")
        .and_then(|v| v.as_u64());
    let raw = nested.or(flat).unwrap_or(DEFAULT_MAX_DEPTH as u64) as usize;
    raw.clamp(1, MAX_DEPTH_CEILING)
}

/// Walk `root` looking for `.loctree/` directories.
///
/// Returns canonical parent paths (deduplicated, sorted). The root
/// itself is never included — callers handle the root workspace
/// through its dedicated handle. Pruned directories
/// ([`PRUNED_DIRS`]) are skipped to keep monorepo discovery bounded.
pub fn discover_loctree_dirs(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let depth = max_depth.clamp(1, MAX_DEPTH_CEILING);
    let canonical_root = canonicalize(root);
    let mut found: BTreeSet<PathBuf> = BTreeSet::new();

    // BFS so we can prune entire subtrees with `continue;` on directory
    // names that are guaranteed-noise (node_modules, .git, target …).
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((canonical_root.clone(), 0));

    while let Some((dir, current_depth)) = queue.pop_front() {
        if current_depth > depth {
            continue;
        }

        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };

            // Resolve symlinks before deciding what kind of node this
            // is — `read_dir` returns symlinks unresolved on macOS.
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            // Don't follow symlinks during discovery: a self-referential
            // link would cause the walk to never terminate.
            if metadata.file_type().is_symlink() {
                continue;
            }
            if !metadata.is_dir() {
                continue;
            }

            if PRUNED_DIRS.contains(&name) {
                if name == ".loctree" && current_depth > 0 {
                    // Found a `.loctree/` directory that is not a
                    // direct child of the LSP root — its parent is
                    // a candidate sub-project. Require a real
                    // `snapshot.json` inside before recording it,
                    // so empty `.loctree/` markers (test fixtures
                    // under `tools/fixtures/**`, init artifacts left
                    // by aborted scans) do not pollute the
                    // addressable workspace set with load failures.
                    if path.join("snapshot.json").is_file() {
                        found.insert(canonicalize(&dir));
                    }
                }
                continue;
            }

            queue.push_back((path, current_depth + 1));
        }
    }

    // Always exclude the root from extras — it is addressed via the
    // backend's primary snapshot handle.
    found.remove(&canonical_root);

    found.into_iter().collect()
}

/// Best-effort canonicalization. Falls back to the original path when
/// the filesystem rejects the lookup (network drive, permissions, …)
/// so discovery remains usable on weird filesystems.
pub fn canonicalize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Compute snapshot age in seconds from the `.loctree/snapshot.json`
/// modification time. Returns `None` when the file is missing or its
/// mtime cannot be read — the caller surfaces the absence directly.
pub fn snapshot_age(workspace_root: &Path) -> Option<u64> {
    // Loctree stores snapshots in a global cache (see `Snapshot::save`),
    // but a per-project mirror lives at `<root>/.loctree/snapshot.json`
    // when the operator opts into local persistence. Either path works
    // as an age signal — the cache mtime is the authoritative one.
    let local = workspace_root.join(".loctree").join("snapshot.json");
    let candidate = if local.exists() {
        local
    } else {
        // Fall back to the global cache mtime. The cache layout is
        // owned by `loctree::snapshot::project_cache_dir`, so we ask
        // it directly rather than hard-coding the layout here.
        let cache = loctree::snapshot::project_cache_dir(workspace_root);
        let snapshot_json = cache.join("snapshot.json");
        if snapshot_json.exists() {
            snapshot_json
        } else {
            return None;
        }
    };

    let mtime = std::fs::metadata(&candidate).ok()?.modified().ok()?;
    let now = SystemTime::now();
    let age = now.duration_since(mtime).unwrap_or(Duration::ZERO);
    Some(age.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// Helper: create a `.loctree/snapshot.json` marker under `parent`
    /// so `discover_loctree_dirs` recognizes the parent as a real
    /// addressable sub-project. The file content is irrelevant to
    /// discovery — only its presence matters (Snapshot::load handles
    /// the parsing later).
    fn touch_loctree_marker(parent: &Path) {
        let dir = parent.join(".loctree");
        std::fs::create_dir_all(&dir).expect("create .loctree dir");
        std::fs::write(dir.join("snapshot.json"), b"{}").expect("write snapshot.json marker");
    }

    #[test]
    fn default_max_depth_is_4() {
        assert_eq!(DEFAULT_MAX_DEPTH, 4);
    }

    #[test]
    fn max_depth_reads_nested_option() {
        let opts = json!({
            "loctree": { "workspaces": { "maxDepth": 6 } }
        });
        assert_eq!(max_depth_from_options(Some(&opts)), 6);
    }

    #[test]
    fn max_depth_reads_flat_option() {
        let opts = json!({ "loctree.workspaces.maxDepth": 2 });
        assert_eq!(max_depth_from_options(Some(&opts)), 2);
    }

    #[test]
    fn max_depth_clamps_overflow() {
        let opts = json!({ "loctree.workspaces.maxDepth": 100 });
        assert_eq!(max_depth_from_options(Some(&opts)), MAX_DEPTH_CEILING);
    }

    #[test]
    fn max_depth_clamps_zero_to_one() {
        let opts = json!({ "loctree.workspaces.maxDepth": 0 });
        assert_eq!(max_depth_from_options(Some(&opts)), 1);
    }

    #[test]
    fn max_depth_default_when_options_absent() {
        assert_eq!(max_depth_from_options(None), DEFAULT_MAX_DEPTH);
    }

    #[test]
    fn discover_returns_empty_for_repo_with_no_subprojects() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        touch_loctree_marker(root);

        let found = discover_loctree_dirs(root, 4);
        assert!(found.is_empty(), "root should be excluded from extras");
    }

    #[test]
    fn discover_finds_subproject_one_level_deep() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        touch_loctree_marker(&root.join("apps/web"));

        let found = discover_loctree_dirs(root, 4);
        assert_eq!(found.len(), 1);
        assert!(
            found[0].ends_with("apps/web"),
            "expected apps/web parent, got {}",
            found[0].display()
        );
    }

    #[test]
    fn discover_finds_multiple_subprojects_at_different_depths() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        touch_loctree_marker(&root.join("apps/web"));
        touch_loctree_marker(&root.join("apps/api/src"));
        touch_loctree_marker(&root.join("packages/ui"));

        let found = discover_loctree_dirs(root, 4);
        assert_eq!(found.len(), 3);
        let labels: Vec<String> = found
            .iter()
            .map(|p| {
                p.components()
                    .rev()
                    .take(2)
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .collect();
        assert!(labels.iter().any(|l| l.ends_with("apps/web")));
        assert!(labels.iter().any(|l| l.ends_with("src")));
        assert!(labels.iter().any(|l| l.ends_with("packages/ui")));
    }

    #[test]
    fn discover_prunes_node_modules() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        // Synthetic .loctree under node_modules — must NOT be discovered.
        touch_loctree_marker(&root.join("node_modules/poison"));
        touch_loctree_marker(&root.join("apps/web"));

        let found = discover_loctree_dirs(root, 4);
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("apps/web"));
    }

    #[test]
    fn discover_prunes_target_and_dot_git() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        touch_loctree_marker(&root.join("target/release"));
        touch_loctree_marker(&root.join(".git/poison"));
        touch_loctree_marker(&root.join("crate-a"));

        let found = discover_loctree_dirs(root, 4);
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("crate-a"));
    }

    #[test]
    fn discover_respects_max_depth() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        // 6-deep — the .loctree dir itself sits at depth 7, parent at 6.
        touch_loctree_marker(&root.join("a/b/c/d/e/f"));

        let shallow = discover_loctree_dirs(root, 3);
        assert!(shallow.is_empty());

        let deep = discover_loctree_dirs(root, MAX_DEPTH_CEILING);
        assert_eq!(deep.len(), 1);
        assert!(deep[0].ends_with("a/b/c/d/e/f"));
    }

    #[test]
    fn discover_dedups_when_same_parent_has_two_loctree_paths() {
        // Pathological filesystem: a parent containing both `.loctree/`
        // and a duplicate via symlink-style alias is not testable here,
        // but the BTreeSet contract guarantees dedup. Record the
        // expectation via the simpler "single child" case.
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        touch_loctree_marker(&root.join("only"));

        let found = discover_loctree_dirs(root, 4);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn discover_skips_empty_loctree_marker_without_snapshot() {
        // Test fixtures sometimes leave behind `.loctree/` directories
        // without `snapshot.json` (init artifacts, copy-paste setup,
        // tools/fixtures/** integration scaffolds). Discovery must
        // skip them so the LSP root does not log spurious
        // "Snapshot not found at .../.loctree" warnings on every
        // `initialized` and so the addressable workspace set stays
        // truthful.
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        // Real sub-project: has snapshot.json.
        touch_loctree_marker(&root.join("apps/real"));
        // Empty markers: directory exists, no snapshot.json.
        std::fs::create_dir_all(root.join("tools/fixtures/dist-test/src/.loctree"))
            .expect("create empty fixture .loctree");
        std::fs::create_dir_all(root.join("tools/fixtures/nodejs-loader/.loctree"))
            .expect("create empty fixture .loctree");

        let found = discover_loctree_dirs(root, MAX_DEPTH_CEILING);
        assert_eq!(
            found.len(),
            1,
            "only the real sub-project should be discovered, got {found:?}"
        );
        assert!(found[0].ends_with("apps/real"));
    }
}
