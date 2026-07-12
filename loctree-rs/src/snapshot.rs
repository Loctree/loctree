//! Snapshot module for persisting the code graph to disk.
//!
//! This module implements the "scan once, slice many" philosophy:
//! - `loctree init` or bare `loctree` scans the project and saves a snapshot
//! - Subsequent queries load the snapshot for instant context slicing

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::args::ParsedArgs;
use crate::fs_utils::StaticAssetName;
use crate::types::{FileAnalysis, OutputMode};
use fs4::fs_std::FileExt;

/// Current schema version for snapshot format.
pub const SNAPSHOT_SCHEMA_VERSION: &str = "0.11.0";

/// Extract major.minor from a semver string (e.g. "0.8.10" -> "0.8")
/// Patch bumps don't change the snapshot schema, so we only compare major.minor.
fn schema_major_minor(version: &str) -> &str {
    // Find second dot (end of minor)
    match version
        .find('.')
        .and_then(|i| version[i + 1..].find('.').map(|j| i + 1 + j))
    {
        Some(pos) => &version[..pos],
        None => version, // no patch component, return as-is
    }
}

/// Default snapshot directory name
pub const SNAPSHOT_DIR: &str = ".loctree";

/// Default snapshot file name
pub const SNAPSHOT_FILE: &str = "snapshot.json";

/// Environment variable to override the cache base directory.
const LOCT_CACHE_DIR_ENV: &str = "LOCT_CACHE_DIR";
const LEGACY_MIGRATION_MARKER: &str = ".snapshot-migrated-to-cache";
const REUSE_FENCE_FILE: &str = "snapshot.reuse-fence";
const REUSE_FENCE_ALGORITHM: &str = "sha256:loctree-reuse-fence-v1";

/// Returns the global cache base directory for loctree artifacts.
///
/// Priority:
/// 1. `LOCT_CACHE_DIR` environment variable
/// 2. Platform default: `~/Library/Caches/loctree` (macOS) or `$XDG_CACHE_HOME/loctree` (Linux)
/// 3. Fallback: OS temp dir (for environments without a home/cache directory)
pub fn cache_base_dir() -> PathBuf {
    if let Ok(custom) = std::env::var(LOCT_CACHE_DIR_ENV) {
        let custom = custom.trim();
        if !custom.is_empty() {
            return PathBuf::from(custom);
        }
    }
    if let Some(cache_dir) = dirs::cache_dir() {
        return cache_dir.join("loctree");
    }
    // Last resort: CWD-local .loctree/ (backward compat for envs without $HOME)
    PathBuf::from(SNAPSHOT_DIR)
}

/// Returns the cache directory for a specific project.
///
/// Layout: `<cache_base>/projects/<project_id>/`
/// where `project_id` is the first 16 hex chars of SHA-256(canonical_project_root).
pub fn project_cache_dir(root: &Path) -> PathBuf {
    let canonical = cache_key_root(root);
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let project_id = sha256_hex(hasher.finalize())
        .chars()
        .take(16)
        .collect::<String>();

    // If LOCT_CACHE_DIR is set:
    // - Relative path => interpret relative to the project root (so scans from subdirs still write to root/.loctree)
    // - Absolute path => treat as a multi-project cache base (we still namespace by project_id)
    if let Ok(custom) = std::env::var(LOCT_CACHE_DIR_ENV) {
        let custom = custom.trim();
        if !custom.is_empty() {
            let custom_path = PathBuf::from(custom);
            if custom_path.is_relative() {
                return canonical.join(custom_path);
            }
            return custom_path.join("projects").join(project_id);
        }
    }

    cache_base_dir().join("projects").join(project_id)
}

fn cache_key_root(root: &Path) -> PathBuf {
    let absolute = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|err| {
                panic!(
                    "cannot derive absolute project cache key for relative root {}: {err}",
                    root.display()
                )
            })
            .join(root)
    };
    canonicalize_existing_ancestor(&absolute)
}

fn canonicalize_existing_ancestor(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    let mut missing = Vec::new();
    let mut cursor = path;
    loop {
        if let Ok(canonical_parent) = cursor.canonicalize() {
            let mut rebuilt = canonical_parent;
            for component in missing.iter().rev() {
                rebuilt.push(component);
            }
            return rebuilt;
        }
        let Some(name) = cursor.file_name() else {
            return path.to_path_buf();
        };
        missing.push(name.to_os_string());
        let Some(parent) = cursor.parent() else {
            return path.to_path_buf();
        };
        cursor = parent;
    }
}

fn project_cache_id(root: &Path) -> String {
    project_cache_dir(root)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let canonical = cache_key_root(root);
            let mut hasher = Sha256::new();
            hasher.update(canonical.to_string_lossy().as_bytes());
            sha256_hex(hasher.finalize())
                .chars()
                .take(16)
                .collect::<String>()
        })
}

fn project_cache_lock_path(root: &Path) -> PathBuf {
    cache_base_dir()
        .join("locks")
        .join(format!("{}.lock", project_cache_id(root)))
}

struct SnapshotCacheLock {
    file: File,
}

impl Drop for SnapshotCacheLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn acquire_snapshot_cache_lock(root: &Path) -> io::Result<SnapshotCacheLock> {
    let lock_path = project_cache_lock_path(root);
    if let Some(dir) = lock_path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    FileExt::lock_exclusive(&file)?;
    Ok(SnapshotCacheLock { file })
}

fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Returns the project-local config directory (for user-editable files).
///
/// Config files (config.toml, suppressions.toml) stay in the project — they are
/// user-editable and may be version-controlled. Only cache artifacts move to global cache.
pub fn project_config_dir(root: &Path) -> PathBuf {
    root.join(SNAPSHOT_DIR)
}

fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent for atomic write"))?;
    let mut tmp = tempfile::Builder::new()
        .prefix("loctree_tmp")
        .tempfile_in(dir)
        .map_err(|err| cache_write_error(path, err))?;
    tmp.write_all(contents.as_ref())
        .map_err(|err| cache_write_error(path, err))?;
    tmp.flush().map_err(|err| cache_write_error(path, err))?;
    tmp.persist(path)
        .map_err(|err| cache_write_error(path, err.error))?;
    Ok(())
}

fn resolve_snapshot_file_path(root: &Path, file_path: &str) -> PathBuf {
    let raw = PathBuf::from(file_path);
    if raw.is_absolute() {
        return raw;
    }
    root.join(raw)
}

fn hash_file_state(root: &Path, file_path: &str) -> io::Result<String> {
    let path = resolve_snapshot_file_path(root, file_path);
    match read_file_under_root(root, &path) {
        Ok(bytes) => Ok(format!("present:{}", sha256_hex(Sha256::digest(&bytes)))),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok("missing".to_string()),
        Err(err) => Err(err),
    }
}

fn parse_reuse_fence_file_hashes(contents: &str) -> HashMap<String, String> {
    contents
        .lines()
        .filter_map(|line| line.strip_prefix("[FILE] "))
        .filter_map(|entry| {
            let (hash, path) = entry.split_once('\t')?;
            Some((path.to_string(), hash.to_string()))
        })
        .collect()
}

/// True when a repo-relative path (as printed by `git status --porcelain`)
/// points into a `.loctree/` artifact directory — loct's own output
/// (snapshots, context-atlas, reports, logs).
///
/// The scanner never indexes `.loctree/` (hidden-dir filter), so these files
/// can never invalidate snapshot content. But on a repo where `.loctree/` is
/// NOT gitignored, the very first scan writes `./.loctree/context-atlas/`
/// into the worktree and `git status` reports it as untracked dirt — which
/// used to make every Strict-policy analytic verb [DRIFT]-rescan forever on
/// loct's own artifact. The freshness guardian must be blind to them.
fn is_loctree_artifact_path(path: &str) -> bool {
    path.trim_start_matches("./")
        .split('/')
        .any(|segment| segment == ".loctree")
}

fn git_dirty_paths(root: &Path) -> Option<Vec<String>> {
    let output = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_git_status_paths(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn parse_git_status_paths(status: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in status.lines() {
        if line.len() < 4 {
            continue;
        }
        let entry = line[3..].trim();
        if let Some((old_path, new_path)) = entry.split_once(" -> ") {
            paths.push(unquote_git_status_path(old_path));
            paths.push(unquote_git_status_path(new_path));
        } else {
            paths.push(unquote_git_status_path(entry));
        }
    }
    // Loct's own artifacts are never part of the indexed universe and must
    // never count as worktree dirt for freshness decisions.
    paths.retain(|path| !is_loctree_artifact_path(path));
    paths.sort();
    paths.dedup();
    paths
}

fn unquote_git_status_path(path: &str) -> String {
    path.trim_matches('"').replace("\\\"", "\"")
}

fn should_rescan_for_unindexed_dirty_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with("makefile") {
        return true;
    }
    let Some(ext) = lower.rsplit('.').next().filter(|ext| *ext != lower) else {
        return false;
    };
    matches!(
        ext,
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "py"
            | "sh"
            | "bash"
            | "zsh"
            | "css"
            | "html"
            | "svelte"
            | "astro"
            | "md"
            | "toml"
            | "yml"
            | "yaml"
            | "zig"
            | "config"
    )
}

/// First-scan hygiene: make sure `.loctree/` is gitignored in `root`.
///
/// Loct writes its own artifacts (`context-atlas/`, pointer files) into
/// `./.loctree/`; on a repo where that directory is not ignored they show up
/// as untracked dirt and eventually get committed by broad `git add -A`
/// workflows. The freshness guardian is already blind to `.loctree/`
/// ([`is_loctree_artifact_path`]), so this is repo hygiene, not drift
/// correctness.
///
/// Best-effort and deliberately conservative:
/// - only acts when `root` is a git repository root; the entry is appended
///   proactively even before `./.loctree/` exists on disk, because a plain
///   `loct scan` writes nothing into the worktree yet — the first later
///   `loct context` / auto-artifact write is what needs the entry,
/// - asks `git check-ignore` (honors global excludes and nested ignore
///   files) instead of re-implementing gitignore semantics,
/// - on any git uncertainty it keeps its hands off the user's file.
///
/// Returns `Ok(true)` when the entry was appended. The caller owns the loud
/// stderr announcement and the quiet-mode / `LOCT_NO_GITIGNORE` gating —
/// this function never prints, so it is never a silent mutation path on its
/// own.
fn ensure_loctree_gitignore_entry(root: &Path) -> io::Result<bool> {
    if !root.join(".git").exists() {
        return Ok(false);
    }
    let ignored_target = format!("{SNAPSHOT_DIR}/");
    let Some(check) = Command::new("git")
        .args(["check-ignore", "-q", ignored_target.as_str()])
        .current_dir(root)
        .output()
        .ok()
    else {
        return Ok(false);
    };
    // 0 = already ignored, 1 = not ignored, anything else = git error.
    if check.status.code() != Some(1) {
        return Ok(false);
    }

    let path = root.join(".gitignore");
    let mut body = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str("# Loctree artifacts (added by loct scan)\n.loctree/\n");
    write_atomic(&path, body)?;
    Ok(true)
}

/// True when `LOCT_NO_GITIGNORE` opts out of the first-scan gitignore append.
fn gitignore_append_disabled() -> bool {
    std::env::var_os("LOCT_NO_GITIGNORE").is_some_and(|value| !value.is_empty() && value != "0")
}

fn cache_write_error(path: &Path, err: io::Error) -> io::Error {
    if !is_storage_full_error(&err) {
        return err;
    }

    let cache_base = cache_base_dir();
    let target = path.display();
    let cache_root = cache_base.display();
    io::Error::new(
        err.kind(),
        format!(
            "{err} while writing {target}. Loctree cache may be full at {cache_root}; run `loct cache list` then `loct cache prune --max-size 1GB --force` or set `LOCT_CACHE_DIR` to a larger volume."
        ),
    )
}

fn is_storage_full_error(err: &io::Error) -> bool {
    err.raw_os_error() == Some(28) || err.to_string().contains("No space left on device")
}

/// Read a file after re-asserting that its canonical form is a descendant
/// of `allowed_root`.
///
/// SaaS-safety helper: callers may have already validated `path` somewhere
/// upstream, but Semgrep's `tainted-path` analysis only follows local
/// data-flow. The [`crate::fs_utils::SanitizedPath`] gate inside
/// `read_within` canonicalizes + re-checks `starts_with(allowed_root)`
/// immediately before `fs::read` so the boundary guard sits at the same
/// call site as the I/O sink.
fn read_file_under_root(allowed_root: &Path, path: &Path) -> io::Result<Vec<u8>> {
    crate::fs_utils::read_within(allowed_root, path)
}

fn read_text_under_cache_root(root: &Path, path: &Path) -> io::Result<String> {
    crate::fs_utils::read_to_string_within(&project_cache_dir(root), path)
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.canonicalize().ok()?;
    loop {
        let git_dir = current.join(".git");
        if git_dir.is_dir() || git_dir.is_file() {
            return Some(current);
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return None,
        }
    }
}

fn normalize_root_dir(root: &Path) -> PathBuf {
    let base = if root.is_file() {
        root.parent().unwrap_or(root).to_path_buf()
    } else {
        root.to_path_buf()
    };
    base.canonicalize().unwrap_or(base)
}

fn has_project_marker(root: &Path) -> bool {
    const MARKERS: [&str; 16] = [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "tsconfig.json",
        "deno.json",
        "deno.jsonc",
        "go.mod",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "composer.json",
        // Python projects without pyproject.toml
        "requirements.txt",
        "setup.py",
        "setup.cfg",
        // Common project root markers
        "Makefile",
        "pubspec.yaml",
    ];
    MARKERS.iter().any(|marker| root.join(marker).is_file())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotRootStrategy {
    Project,
    Exact,
}

fn resolve_exact_snapshot_root(root_list: &[PathBuf]) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_default();
    let root = root_list.first().cloned().unwrap_or(cwd);
    let normalized = normalize_root_dir(&root);
    normalized.canonicalize().unwrap_or(normalized)
}

pub fn resolve_snapshot_root_with_strategy(
    root_list: &[PathBuf],
    strategy: SnapshotRootStrategy,
) -> PathBuf {
    if strategy == SnapshotRootStrategy::Exact {
        return resolve_exact_snapshot_root(root_list);
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    let roots: Vec<PathBuf> = if root_list.is_empty() {
        vec![cwd.clone()]
    } else {
        root_list
            .iter()
            .map(|root| normalize_root_dir(root))
            .collect()
    };

    // If the given root itself looks like a project (has tsconfig.json, package.json, etc.),
    // use it directly — don't walk upward past an explicit project boundary.
    if roots.len() == 1 && has_project_marker(&roots[0]) {
        return roots[0].clone();
    }

    // Prefer git root — the most reliable project boundary. Checked before
    // find_loctree_root to avoid walking past .git into unrelated parent caches
    // (e.g. a stale cache entry at "/" would trap all non-marker projects).
    if let Some(first_git) = roots.first().and_then(|root| find_git_root(root))
        && roots
            .iter()
            .all(|root| find_git_root(root).as_ref() == Some(&first_git))
    {
        return first_git;
    }

    let mut loctree_roots: Vec<PathBuf> = roots
        .iter()
        .filter_map(|root| Snapshot::find_loctree_root(root))
        .collect();
    if let Some(first) = loctree_roots.pop()
        && loctree_roots.iter().all(|root| root == &first)
    {
        return first;
    }

    find_git_root(&cwd).unwrap_or(cwd)
}

pub fn resolve_snapshot_root(root_list: &[PathBuf]) -> PathBuf {
    resolve_snapshot_root_with_strategy(root_list, SnapshotRootStrategy::Project)
}

/// Normalize root paths for scope comparison between requested roots and snapshot metadata.
///
/// - Resolves relative paths against `snapshot_root`
/// - Canonicalizes when possible (falls back to the un-canonical form on error)
/// - Normalizes path separators to forward slash
/// - Sorts and deduplicates the result
///
/// Used by the CLI and the MCP server to detect scope mismatches between a
/// stored snapshot and the roots a caller is asking about — e.g. a snapshot
/// written from a fixture sub-tree being mistakenly served for the workspace
/// root because both share the same `project_id`.
pub fn normalize_roots_for_scope_compare<'a, I>(roots: I, snapshot_root: &Path) -> Vec<String>
where
    I: Iterator<Item = &'a Path>,
{
    let mut normalized = Vec::new();
    for root in roots {
        let candidate = if root.is_absolute() {
            root.to_path_buf()
        } else {
            snapshot_root.join(root)
        };
        let canon = candidate.canonicalize().unwrap_or(candidate);
        normalized.push(canon.to_string_lossy().replace('\\', "/"));
    }
    normalized.sort();
    normalized.dedup();
    normalized
}

#[derive(Clone, Debug)]
pub(crate) struct DeclaredEntrypoint {
    pub(crate) source: String,
    pub(crate) path: String,
    pub(crate) exists: bool,
    pub(crate) resolved: bool,
    pub(crate) note: Option<String>,
}

fn normalize_snapshot_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn resolve_declared_path(root: &Path, raw: &str) -> Option<(String, bool)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('*') || trimmed.contains('?') {
        return None;
    }
    if trimmed.starts_with("node:") {
        return None;
    }
    let cleaned = trimmed.trim_start_matches("./");
    let base = Path::new(cleaned);
    let full = if base.is_absolute() {
        base.to_path_buf()
    } else {
        root.join(cleaned)
    };
    let exists = full.exists();
    let rel = full.strip_prefix(root).unwrap_or(&full);
    Some((normalize_snapshot_path(&rel.to_string_lossy()), exists))
}

fn note_for_declared_path(path: &str, source: &str) -> Option<String> {
    let lowered = path.to_lowercase();
    if source.contains("types") || lowered.ends_with(".d.ts") {
        return Some("types entry (non-runtime)".to_string());
    }
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => {
            Some("js/ts entrypoint markers not detected".to_string())
        }
        _ => None,
    }
}

pub(crate) fn collect_declared_entrypoints(summary: &ManifestSummary) -> Vec<DeclaredEntrypoint> {
    let mut declared = Vec::new();
    let root = PathBuf::from(&summary.root);

    if let Some(pkg) = &summary.package_json {
        for (label, value) in [
            ("package.json:main", pkg.main.as_ref()),
            ("package.json:module", pkg.module.as_ref()),
            ("package.json:types", pkg.types.as_ref()),
        ] {
            if let Some(raw) = value {
                if let Some((path, exists)) = resolve_declared_path(&root, raw) {
                    declared.push(DeclaredEntrypoint {
                        source: label.to_string(),
                        path,
                        exists,
                        resolved: true,
                        note: note_for_declared_path(raw, label),
                    });
                } else {
                    declared.push(DeclaredEntrypoint {
                        source: label.to_string(),
                        path: raw.to_string(),
                        exists: false,
                        resolved: false,
                        note: Some("unresolved manifest path".to_string()),
                    });
                }
            }
        }
        for entry in &pkg.exports {
            let source = format!("package.json:exports:{}", entry.key);
            if let Some((path, exists)) = resolve_declared_path(&root, &entry.path) {
                declared.push(DeclaredEntrypoint {
                    source,
                    path,
                    exists,
                    resolved: true,
                    note: note_for_declared_path(&entry.path, "package.json:exports"),
                });
            } else {
                declared.push(DeclaredEntrypoint {
                    source,
                    path: entry.path.clone(),
                    exists: false,
                    resolved: false,
                    note: Some("unresolved manifest path".to_string()),
                });
            }
        }
        for entry in &pkg.bin {
            let source = format!("package.json:bin:{}", entry.key);
            if let Some((path, exists)) = resolve_declared_path(&root, &entry.path) {
                declared.push(DeclaredEntrypoint {
                    source,
                    path,
                    exists,
                    resolved: true,
                    note: note_for_declared_path(&entry.path, "package.json:bin"),
                });
            } else {
                declared.push(DeclaredEntrypoint {
                    source,
                    path: entry.path.clone(),
                    exists: false,
                    resolved: false,
                    note: Some("unresolved manifest path".to_string()),
                });
            }
        }
    }

    if let Some(cargo) = &summary.cargo_toml {
        if let Some(lib_path) = &cargo.lib_path {
            if let Some((path, exists)) = resolve_declared_path(&root, lib_path) {
                declared.push(DeclaredEntrypoint {
                    source: "Cargo.toml:lib".to_string(),
                    path,
                    exists,
                    resolved: true,
                    note: None,
                });
            }
        } else {
            let lib_default = root.join("src/lib.rs");
            let rel = normalize_snapshot_path(
                &lib_default
                    .strip_prefix(&root)
                    .unwrap_or(&lib_default)
                    .to_string_lossy(),
            );
            if lib_default.exists() {
                declared.push(DeclaredEntrypoint {
                    source: "Cargo.toml:lib:default".to_string(),
                    path: rel,
                    exists: true,
                    resolved: true,
                    note: None,
                });
            }
        }

        if cargo.bins.is_empty() {
            let main_default = root.join("src/main.rs");
            let rel = normalize_snapshot_path(
                &main_default
                    .strip_prefix(&root)
                    .unwrap_or(&main_default)
                    .to_string_lossy(),
            );
            if main_default.exists() {
                declared.push(DeclaredEntrypoint {
                    source: "Cargo.toml:bin:default".to_string(),
                    path: rel,
                    exists: true,
                    resolved: true,
                    note: None,
                });
            }
        } else {
            for bin in &cargo.bins {
                let source = format!("Cargo.toml:bin:{}", bin.name);
                let path_value = bin
                    .path
                    .clone()
                    .unwrap_or_else(|| format!("src/bin/{}.rs", bin.name));
                if let Some((path, exists)) = resolve_declared_path(&root, &path_value) {
                    declared.push(DeclaredEntrypoint {
                        source,
                        path,
                        exists,
                        resolved: true,
                        note: None,
                    });
                }
            }
        }

        for member in &cargo.workspace_members {
            let member_root = root.join(member);
            if !member_root.join("Cargo.toml").exists() {
                continue;
            }
            let member_lib = member_root.join("src/lib.rs");
            if member_lib.exists() {
                let rel = normalize_snapshot_path(
                    &member_lib
                        .strip_prefix(&root)
                        .unwrap_or(&member_lib)
                        .to_string_lossy(),
                );
                declared.push(DeclaredEntrypoint {
                    source: format!("Cargo.toml:member:{}:lib", member),
                    path: rel,
                    exists: true,
                    resolved: true,
                    note: None,
                });
            }
            let member_main = member_root.join("src/main.rs");
            if member_main.exists() {
                let rel = normalize_snapshot_path(
                    &member_main
                        .strip_prefix(&root)
                        .unwrap_or(&member_main)
                        .to_string_lossy(),
                );
                declared.push(DeclaredEntrypoint {
                    source: format!("Cargo.toml:member:{}:bin", member),
                    path: rel,
                    exists: true,
                    resolved: true,
                    note: None,
                });
            }
        }
    }

    if let Some(py) = &summary.pyproject_toml {
        for script in &py.scripts {
            declared.push(DeclaredEntrypoint {
                source: "pyproject.toml:scripts".to_string(),
                path: script.clone(),
                exists: false,
                resolved: false,
                note: Some("script entry (no path mapping)".to_string()),
            });
        }
        for entry in &py.entry_points {
            declared.push(DeclaredEntrypoint {
                source: "pyproject.toml:entry-points".to_string(),
                path: entry.clone(),
                exists: false,
                resolved: false,
                note: Some("entry-point group (no path mapping)".to_string()),
            });
        }
    }

    declared
}

fn compute_entrypoint_drift(
    manifest_summary: &[ManifestSummary],
    entrypoints: &[EntrypointSummary],
) -> EntrypointDriftSummary {
    let mut drift = EntrypointDriftSummary::default();

    let mut declared_paths: HashSet<String> = HashSet::new();
    let entrypoint_paths: HashSet<String> = entrypoints
        .iter()
        .map(|e| normalize_snapshot_path(&e.path))
        .collect();

    for summary in manifest_summary {
        for declared in collect_declared_entrypoints(summary) {
            if !declared.resolved {
                drift.declared_unresolved.push(EntrypointDriftItem {
                    source: declared.source,
                    path: declared.path,
                    note: declared.note,
                });
                continue;
            }
            let path = normalize_snapshot_path(&declared.path);
            declared_paths.insert(path.clone());
            if !declared.exists {
                drift.declared_missing.push(EntrypointDriftItem {
                    source: declared.source,
                    path,
                    note: declared.note,
                });
            } else if !entrypoint_paths.contains(&path) {
                drift.declared_without_marker.push(EntrypointDriftItem {
                    source: declared.source,
                    path,
                    note: declared.note,
                });
            }
        }
    }

    for entry in entrypoints {
        let path = normalize_snapshot_path(&entry.path);
        if !declared_paths.contains(&path) {
            drift.code_only_entrypoints.push(EntrypointSummary {
                path,
                kinds: entry.kinds.clone(),
            });
        }
    }

    drift
}

/// Parse `owner/repo` from a git remote URL.
///
/// Handles common shapes:
/// - HTTPS: `https://github.com/owner/repo.git`
/// - SSH:   `git@github.com:owner/repo.git`
/// - Plain: `github.com/owner/repo`
///
/// Returns `None` for URLs that don't contain an `owner/repo` pair.
pub fn parse_owner_repo(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    // Normalize: strip trailing `.git`
    let url = url.strip_suffix(".git").unwrap_or(url);

    // SSH style: git@host:owner/repo
    if let Some(after_colon) = url.strip_prefix("git@").and_then(|rest| {
        // Find the colon that separates host from path
        rest.find(':').map(|i| &rest[i + 1..])
    }) {
        let parts: Vec<&str> = after_colon.split('/').collect();
        if parts.len() >= 2 {
            return Some(format!(
                "{}/{}",
                parts[parts.len() - 2],
                parts[parts.len() - 1]
            ));
        }
        return None;
    }

    // HTTPS / plain style: ...host/owner/repo
    let segments: Vec<&str> = url.split('/').collect();
    if segments.len() >= 2 {
        let repo = segments[segments.len() - 1];
        let owner = segments[segments.len() - 2];
        // Guard: owner shouldn't be a protocol or empty
        if !owner.is_empty() && !owner.contains(':') {
            return Some(format!("{owner}/{repo}"));
        }
    }

    None
}

/// Extract just the repo name (last path segment) from a git remote URL.
fn parse_repo_name(url: &str) -> Option<String> {
    let url = url.trim();
    url.rsplit('/')
        .next()
        .or_else(|| url.rsplit(':').next())
        .map(|s| s.trim_end_matches(".git").to_string())
        .filter(|s| !s.is_empty())
}

/// Git workspace context for artifact isolation.
///
/// Used to store snapshots per branch@commit (e.g., `.loctree/main@abc123/snapshot.json`).
#[derive(Clone, Debug)]
pub struct GitContext {
    /// Repository name (extracted from remote origin — last segment only).
    pub repo: Option<String>,
    /// Full `owner/repo` identifier derived from git remote origin.
    pub owner_repo: Option<String>,
    /// Current branch name.
    pub branch: Option<String>,
    /// Short commit hash.
    pub commit: Option<String>,
    /// Combined identifier: `branch@commit` (sanitized for filesystem).
    pub scan_id: Option<String>,
}

/// Metadata about the snapshot
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    /// Schema version for compatibility checking
    #[serde(default)]
    pub schema_version: String,
    /// Timestamp when snapshot was generated (ISO 8601)
    #[serde(default)]
    pub generated_at: String,
    /// Root path(s) that were scanned
    #[serde(default)]
    pub roots: Vec<String>,
    /// Detected languages in the project
    #[serde(default)]
    pub languages: HashSet<String>,
    /// Total number of files scanned
    #[serde(default)]
    pub file_count: usize,
    /// Total lines of code
    #[serde(default)]
    pub total_loc: usize,
    /// Scan duration in milliseconds
    #[serde(default)]
    pub scan_duration_ms: u64,
    /// Resolver configuration (tsconfig paths, etc.)
    #[serde(default)]
    pub resolver_config: Option<ResolverConfig>,
    /// Manifest summaries (package.json, Cargo.toml, pyproject.toml)
    #[serde(default)]
    pub manifest_summary: Vec<ManifestSummary>,
    /// Detected entrypoints across files
    #[serde(default)]
    pub entrypoints: Vec<EntrypointSummary>,
    /// Drift between declared manifest roots and code entrypoints
    #[serde(default)]
    pub entrypoint_drift: EntrypointDriftSummary,
    /// Git repository name (extracted from remote origin — last segment only).
    /// Kept for backward compatibility with older snapshots.
    #[serde(default)]
    pub git_repo: Option<String>,
    /// Full `owner/repo` identifier derived from git remote origin URL.
    /// New in v0.8.17. Missing in older snapshots (defaults to `None`).
    #[serde(default)]
    pub git_owner_repo: Option<String>,
    /// Git branch name
    #[serde(default)]
    pub git_branch: Option<String>,
    /// Git commit hash (short)
    #[serde(default)]
    pub git_commit: Option<String>,
    /// Combined scan identifier (e.g., branch@sha) for artifact isolation
    #[serde(default)]
    pub git_scan_id: Option<String>,
}

/// Configuration for path resolution (aliases, etc.)
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ResolverConfig {
    /// TypeScript/JavaScript path aliases from tsconfig.json
    pub ts_paths: HashMap<String, Vec<String>>,
    /// Base URL for TypeScript resolution
    pub ts_base_url: Option<String>,
    /// Python root paths
    pub py_roots: Vec<String>,
    /// Rust crate roots
    pub rust_crate_roots: Vec<String>,
}

/// Single manifest entry (key -> path).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ManifestEntry {
    pub key: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PackageJsonSummary {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub package_type: Option<String>,
    #[serde(default)]
    pub main: Option<String>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub types: Option<String>,
    #[serde(default)]
    pub exports: Vec<ManifestEntry>,
    #[serde(default)]
    pub bin: Vec<ManifestEntry>,
    #[serde(default)]
    pub workspaces: Vec<String>,
    #[serde(default)]
    pub scripts: Vec<String>,
    #[serde(default)]
    pub package_manager: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CargoBinSummary {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CargoTomlSummary {
    #[serde(default)]
    pub package_name: Option<String>,
    #[serde(default)]
    pub workspace_members: Vec<String>,
    #[serde(default)]
    pub workspace_default_members: Vec<String>,
    #[serde(default)]
    pub lib_path: Option<String>,
    #[serde(default)]
    pub bins: Vec<CargoBinSummary>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub crate_roots: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PyProjectSummary {
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub poetry_name: Option<String>,
    #[serde(default)]
    pub scripts: Vec<String>,
    #[serde(default)]
    pub entry_points: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ManifestSummary {
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub package_json: Option<PackageJsonSummary>,
    #[serde(default)]
    pub cargo_toml: Option<CargoTomlSummary>,
    #[serde(default)]
    pub pyproject_toml: Option<PyProjectSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct EntrypointSummary {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub kinds: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct EntrypointDriftItem {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct EntrypointDriftSummary {
    #[serde(default)]
    pub declared_missing: Vec<EntrypointDriftItem>,
    #[serde(default)]
    pub declared_without_marker: Vec<EntrypointDriftItem>,
    #[serde(default)]
    pub code_only_entrypoints: Vec<EntrypointSummary>,
    #[serde(default)]
    pub declared_unresolved: Vec<EntrypointDriftItem>,
}

impl EntrypointDriftSummary {
    pub fn is_empty(&self) -> bool {
        self.declared_missing.is_empty()
            && self.declared_without_marker.is_empty()
            && self.code_only_entrypoints.is_empty()
            && self.declared_unresolved.is_empty()
    }
}

/// Graph edge representing an import relationship
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphEdge {
    /// Source file path (importer)
    pub from: String,
    /// Target file path (imported)
    pub to: String,
    /// Edge label (import type, symbol name, etc.)
    pub label: String,
}

/// Command bridge mapping (FE invoke -> BE handler)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandBridge {
    /// Command name
    pub name: String,
    /// Frontend call locations (file, line)
    pub frontend_calls: Vec<(String, usize)>,
    /// Backend handler location (file, line)
    pub backend_handler: Option<(String, usize)>,
    /// Whether the command has a handler
    pub has_handler: bool,
    /// Whether the command is called from frontend
    pub is_called: bool,
}

/// Event bridge mapping (emit -> listen)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventBridge {
    /// Event name
    pub name: String,
    /// Emit locations (file, line, kind)
    pub emits: Vec<(String, usize, String)>,
    /// Listen locations (file, line)
    pub listens: Vec<(String, usize)>,
    /// True if this is a FE↔FE sync pattern (emit and listen both in frontend)
    #[serde(default)]
    pub is_fe_sync: bool,
    /// True if emit and listen are in the same file (strongest FE↔FE indicator)
    #[serde(default)]
    pub same_file_sync: bool,
}

/// Export index entry (used by VS2 slice module)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportEntry {
    /// Symbol name
    pub name: String,
    /// Files that export this symbol
    pub files: Vec<String>,
}

/// The complete snapshot of the code graph
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    /// Snapshot metadata
    pub metadata: SnapshotMetadata,
    /// All file analyses (nodes in the graph)
    #[serde(default)]
    pub files: Vec<FileAnalysis>,
    /// Graph edges (import relationships)
    #[serde(default)]
    pub edges: Vec<GraphEdge>,
    /// Export index (symbol -> files mapping)
    #[serde(default)]
    pub export_index: HashMap<String, Vec<String>>,
    /// Command bridges (FE <-> BE)
    #[serde(default)]
    pub command_bridges: Vec<CommandBridge>,
    /// Event bridges (emit <-> listen)
    #[serde(default)]
    pub event_bridges: Vec<EventBridge>,
    /// Detected barrel files
    #[serde(default)]
    pub barrels: Vec<BarrelFile>,
    /// Layer 3 semantic facts collected during scan. `None` for snapshots
    /// produced before semantic integration (Cut 3A) or when no Layer 3
    /// analyzer applies (e.g. repos without shell or make).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_facts: Option<crate::semantic::SemanticFacts>,
    /// Symbol graph — semantic-topology layer beside the file-level
    /// `import_graph` (edges/export_index). `None` for snapshots produced
    /// before C-family symbol awareness (Wave A) or when no symbol engine ran.
    /// Optional + `skip_serializing_if` keeps older snapshots byte-compatible:
    /// every existing consumer of this struct sees the same wire shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_graph: Option<crate::symbols::SymbolGraph>,
}

/// Stable content fingerprint for a snapshot.
///
/// The hash is built from sorted structural facts instead of raw JSON so it is
/// deterministic across process-local `HashMap`/`HashSet` iteration order.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotFingerprint {
    pub algorithm: String,
    pub value: String,
    pub schema_version: String,
    pub file_count: usize,
    pub edge_count: usize,
    pub root_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotGitMetadata {
    pub repo: Option<String>,
    pub owner_repo: Option<String>,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub scan_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotStaleness {
    pub stale: bool,
    pub commit_stale: bool,
    pub dirty_worktree: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotAuthority {
    pub fingerprint: SnapshotFingerprint,
    pub generated_at: String,
    pub roots: Vec<String>,
    pub git: SnapshotGitMetadata,
    pub staleness: SnapshotStaleness,
}

/// Information about a barrel file (index.ts re-exporting)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BarrelFile {
    /// Path to the barrel file
    pub path: String,
    /// Module ID (normalized path)
    pub module_id: String,
    /// Number of re-exports
    pub reexport_count: usize,
    /// Target files being re-exported
    pub targets: Vec<String>,
}

impl Snapshot {
    /// Create a new empty snapshot
    pub fn new(roots: Vec<String>) -> Self {
        let now = time::OffsetDateTime::now_utc();
        let generated_at = now
            .format(&time::format_description::well_known::Iso8601::DEFAULT)
            .unwrap_or_else(|_| "unknown".to_string());

        // Get git info from the first root directory instead of CWD
        let git_info = if let Some(root_path_str) = roots.first() {
            Self::git_context_for(Path::new(root_path_str))
        } else {
            Self::current_git_context()
        };

        Self {
            metadata: SnapshotMetadata {
                schema_version: SNAPSHOT_SCHEMA_VERSION.to_string(),
                generated_at,
                roots,
                languages: HashSet::new(),
                file_count: 0,
                total_loc: 0,
                scan_duration_ms: 0,
                resolver_config: None,
                manifest_summary: Vec::new(),
                entrypoints: Vec::new(),
                entrypoint_drift: EntrypointDriftSummary::default(),
                git_repo: git_info.repo,
                git_owner_repo: git_info.owner_repo,
                git_branch: git_info.branch,
                git_commit: git_info.commit,
                git_scan_id: git_info.scan_id,
            },
            files: Vec::new(),
            edges: Vec::new(),
            export_index: HashMap::new(),
            command_bridges: Vec::new(),
            event_bridges: Vec::new(),
            barrels: Vec::new(),
            semantic_facts: None,
            symbol_graph: None,
        }
    }

    /// Get git repository info (repo name, owner/repo, branch, commit) for given root.
    ///
    /// Uses libgit2's repository discovery to properly find the git root,
    /// even when called from a deeply nested subdirectory. This fixes issues
    /// where git commands would fail if `root` wasn't directly inside a git repo.
    /// Returns `(repo_name, owner_repo, branch, commit)`.
    fn get_git_info(
        root: &Path,
    ) -> (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        use std::process::{Command, Stdio};

        // Find the actual git root (searches upward from root)
        let git_root = match crate::git::find_git_root(root) {
            Some(r) => r,
            None => return (None, None, None, None),
        };

        let remote_url = Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(&git_root)
            .stderr(Stdio::null())
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout)
                        .ok()
                        .map(|s| s.trim().to_string())
                } else {
                    None
                }
            });

        let repo = remote_url.as_deref().and_then(parse_repo_name);
        let owner_repo = remote_url.as_deref().and_then(parse_owner_repo);

        let branch = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&git_root)
            .stderr(Stdio::null())
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout)
                        .ok()
                        .map(|s| s.trim().to_string())
                } else {
                    None
                }
            });
        let commit = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(&git_root)
            .stderr(Stdio::null())
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout)
                        .ok()
                        .map(|s| s.trim().to_string())
                } else {
                    None
                }
            });
        (repo, owner_repo, branch, commit)
    }

    /// Sanitise branch/commit for filesystem path segments
    fn sanitize_ref(value: &str) -> String {
        value.replace(['/', '\\', ' ', ':'], "_").trim().to_string()
    }

    /// Build git context for given root (repo, owner_repo, branch, commit, scan_id)
    pub fn git_context_for(root: &Path) -> GitContext {
        let (repo, owner_repo, branch, commit) = Self::get_git_info(root);
        let scan_id = branch.as_ref().map(|b| {
            let base = Self::sanitize_ref(b);
            commit.as_ref().map_or(base.clone(), |c| {
                format!("{}@{}", base, Self::sanitize_ref(c))
            })
        });
        GitContext {
            repo,
            owner_repo,
            branch,
            commit,
            scan_id,
        }
    }
    /// Build git context for CWD (backwards compat)
    pub fn current_git_context() -> GitContext {
        Self::git_context_for(&std::env::current_dir().unwrap_or_default())
    }

    /// Canonical file count for surfaces that report what Loctree can query.
    pub fn canonical_file_count(&self) -> usize {
        self.files.len()
    }

    fn git_context_for_root_with_metadata_fallback(
        root: &Path,
        metadata: Option<&SnapshotMetadata>,
    ) -> GitContext {
        let root_git = Self::git_context_for(root);
        GitContext {
            repo: root_git
                .repo
                .or_else(|| metadata.and_then(|meta| meta.git_repo.clone())),
            owner_repo: root_git
                .owner_repo
                .or_else(|| metadata.and_then(|meta| meta.git_owner_repo.clone())),
            branch: root_git
                .branch
                .or_else(|| metadata.and_then(|meta| meta.git_branch.clone())),
            commit: root_git
                .commit
                .or_else(|| metadata.and_then(|meta| meta.git_commit.clone())),
            scan_id: root_git
                .scan_id
                .or_else(|| metadata.and_then(|meta| meta.git_scan_id.clone())),
        }
    }

    /// Stable SHA-256 fingerprint of snapshot structural authority.
    pub fn fingerprint(&self) -> String {
        self.fingerprint_report().value
    }

    /// Path to the content reuse fence sidecar for this snapshot.
    pub fn reuse_fence_path(root: &Path) -> PathBuf {
        Self::snapshot_path(root)
            .parent()
            .map(|dir| dir.join(REUSE_FENCE_FILE))
            .unwrap_or_else(|| PathBuf::from(REUSE_FENCE_FILE))
    }

    fn reuse_fence_path_for_snapshot(&self, root: &Path) -> PathBuf {
        if let Some(scan_id) = self.metadata.git_scan_id.as_deref() {
            return project_cache_dir(root).join(scan_id).join(REUSE_FENCE_FILE);
        }
        Self::reuse_fence_path(root)
    }

    /// Hash the current on-disk contents of the files represented by this snapshot.
    ///
    /// Unlike the structural fingerprint, this fence is meant for query-time
    /// cache reuse in a dirty worktree. If the files Loctree actually indexed
    /// have not changed, read-only commands can reuse the snapshot even when
    /// unrelated or unsupported files are dirty.
    pub fn compute_reuse_fence(&self, root: &Path) -> io::Result<String> {
        fn field(hasher: &mut Sha256, key: &str, value: &str) {
            hasher.update(key.len().to_le_bytes());
            hasher.update(key.as_bytes());
            hasher.update(value.len().to_le_bytes());
            hasher.update(value.as_bytes());
        }

        let mut hasher = Sha256::new();
        field(&mut hasher, "algorithm", REUSE_FENCE_ALGORITHM);
        field(&mut hasher, "schema_version", &self.metadata.schema_version);

        let mut files: Vec<_> = self.files.iter().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        for file in files {
            field(&mut hasher, "file", &file.path);
            let path = resolve_snapshot_file_path(root, &file.path);
            match read_file_under_root(root, &path) {
                Ok(bytes) => {
                    field(&mut hasher, "state", "present");
                    hasher.update(bytes.len().to_le_bytes());
                    hasher.update(Sha256::digest(&bytes));
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    field(&mut hasher, "state", "missing");
                }
                Err(err) => return Err(err),
            }
        }

        Ok(format!(
            "{}:{}",
            REUSE_FENCE_ALGORITHM,
            sha256_hex(hasher.finalize())
        ))
    }

    /// Refresh the query-time content fence sidecar.
    pub fn refresh_reuse_fence(&self, root: &Path) -> io::Result<()> {
        let path = self.reuse_fence_path_for_snapshot(root);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let fence = self.compute_reuse_fence(root)?;
        let mut body = format!("[REUSE_FENCE] {}\n", fence);
        for (file, hash) in self.reuse_fence_file_hashes(root)? {
            body.push_str("[FILE] ");
            body.push_str(&hash);
            body.push('\t');
            body.push_str(&file);
            body.push('\n');
        }
        write_atomic(&path, body)
    }

    /// Return true when the current indexed file contents match the saved fence.
    pub fn reuse_fence_matches(&self, root: &Path) -> io::Result<bool> {
        let path = self.reuse_fence_path_for_snapshot(root);
        let saved = match read_text_under_cache_root(root, &path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(err),
        };
        let Some(saved_fence) = saved
            .lines()
            .find_map(|line| line.trim().strip_prefix("[REUSE_FENCE] "))
        else {
            return Ok(false);
        };

        if let Some(dirty_paths) = git_dirty_paths(root) {
            let saved_hashes = parse_reuse_fence_file_hashes(&saved);
            let indexed_paths: HashSet<_> =
                self.files.iter().map(|file| file.path.as_str()).collect();
            for dirty_path in dirty_paths {
                if indexed_paths.contains(dirty_path.as_str()) {
                    let Some(saved_hash) = saved_hashes.get(dirty_path.as_str()) else {
                        return Ok(false);
                    };
                    if &hash_file_state(root, &dirty_path)? != saved_hash {
                        return Ok(false);
                    }
                } else if should_rescan_for_unindexed_dirty_path(&dirty_path) {
                    return Ok(false);
                }
            }
            return Ok(true);
        }

        Ok(saved_fence == self.compute_reuse_fence(root)?)
    }

    fn reuse_fence_file_hashes(&self, root: &Path) -> io::Result<BTreeMap<String, String>> {
        let mut hashes = BTreeMap::new();
        for file in &self.files {
            hashes.insert(file.path.clone(), hash_file_state(root, &file.path)?);
        }
        Ok(hashes)
    }

    /// Stable fingerprint plus compact cardinality metadata.
    pub fn fingerprint_report(&self) -> SnapshotFingerprint {
        let mut hasher = Sha256::new();
        self.update_fingerprint_hasher(&mut hasher);
        SnapshotFingerprint {
            algorithm: "sha256:loctree-snapshot-authority-v1".to_string(),
            value: sha256_hex(hasher.finalize()),
            schema_version: self.metadata.schema_version.clone(),
            file_count: self.canonical_file_count(),
            edge_count: self.edges.len(),
            root_count: self.metadata.roots.len(),
        }
    }

    /// Snapshot authority facade for integrations that need git metadata and staleness.
    pub fn authority_report(&self, fallback_root: &Path) -> SnapshotAuthority {
        let root_to_check = self
            .metadata
            .roots
            .first()
            .map(Path::new)
            .unwrap_or(fallback_root);
        let git =
            Self::git_context_for_root_with_metadata_fallback(root_to_check, Some(&self.metadata));

        let commit_stale = self.is_commit_stale(root_to_check);
        let dirty_worktree = is_git_dirty(root_to_check);
        SnapshotAuthority {
            fingerprint: self.fingerprint_report(),
            generated_at: self.metadata.generated_at.clone(),
            roots: self.metadata.roots.clone(),
            git: SnapshotGitMetadata {
                repo: git.repo,
                owner_repo: git.owner_repo,
                branch: git.branch,
                commit: git.commit,
                scan_id: git.scan_id,
            },
            staleness: SnapshotStaleness {
                stale: commit_stale || dirty_worktree.unwrap_or(false),
                commit_stale,
                dirty_worktree,
            },
        }
    }

    fn update_fingerprint_hasher(&self, hasher: &mut Sha256) {
        fn field(hasher: &mut Sha256, key: &str, value: &str) {
            hasher.update(key.len().to_le_bytes());
            hasher.update(key.as_bytes());
            hasher.update(value.len().to_le_bytes());
            hasher.update(value.as_bytes());
        }

        field(hasher, "schema_version", &self.metadata.schema_version);
        for root in BTreeSet::from_iter(self.metadata.roots.iter().map(String::as_str)) {
            field(hasher, "root", root);
        }
        for language in BTreeSet::from_iter(self.metadata.languages.iter().map(String::as_str)) {
            field(hasher, "language", language);
        }
        field(
            hasher,
            "metadata_file_count",
            &self.canonical_file_count().to_string(),
        );
        field(
            hasher,
            "metadata_total_loc",
            &self.metadata.total_loc.to_string(),
        );
        for value in [
            ("git_repo", self.metadata.git_repo.as_deref()),
            ("git_owner_repo", self.metadata.git_owner_repo.as_deref()),
            ("git_branch", self.metadata.git_branch.as_deref()),
            ("git_commit", self.metadata.git_commit.as_deref()),
            ("git_scan_id", self.metadata.git_scan_id.as_deref()),
        ] {
            field(hasher, value.0, value.1.unwrap_or(""));
        }

        let mut files: Vec<_> = self.files.iter().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        for file in files {
            field(hasher, "file_path", &file.path);
            field(hasher, "file_language", &file.language);
            field(hasher, "file_kind", &file.kind);
            field(hasher, "file_loc", &file.loc.to_string());
            field(hasher, "file_size", &file.size.to_string());

            let mut imports: Vec<_> = file.imports.iter().collect();
            imports.sort_by(|a, b| {
                a.source
                    .cmp(&b.source)
                    .then_with(|| a.source_raw.cmp(&b.source_raw))
                    .then_with(|| a.line.cmp(&b.line))
            });
            for import in imports {
                field(hasher, "import_source", &import.source);
                field(hasher, "import_raw", &import.source_raw);
                field(hasher, "import_line", &format!("{:?}", import.line));
            }

            let mut exports: Vec<_> = file.exports.iter().collect();
            exports.sort_by(|a, b| {
                a.name
                    .cmp(&b.name)
                    .then_with(|| a.kind.cmp(&b.kind))
                    .then_with(|| a.export_type.cmp(&b.export_type))
                    .then_with(|| a.line.cmp(&b.line))
            });
            for export in exports {
                field(hasher, "export_name", &export.name);
                field(hasher, "export_kind", &export.kind);
                field(hasher, "export_type", &export.export_type);
                field(hasher, "export_line", &format!("{:?}", export.line));
            }
        }

        let mut edges: Vec<_> = self.edges.iter().collect();
        edges.sort_by(|a, b| {
            a.from
                .cmp(&b.from)
                .then_with(|| a.to.cmp(&b.to))
                .then_with(|| a.label.cmp(&b.label))
        });
        for edge in edges {
            field(hasher, "edge_from", &edge.from);
            field(hasher, "edge_to", &edge.to);
            field(hasher, "edge_label", &edge.label);
        }

        let mut commands: Vec<_> = self.command_bridges.iter().collect();
        commands.sort_by(|a, b| a.name.cmp(&b.name));
        for command in commands {
            field(hasher, "command", &command.name);
            field(
                hasher,
                "command_has_handler",
                &command.has_handler.to_string(),
            );
            field(hasher, "command_is_called", &command.is_called.to_string());
        }

        let mut events: Vec<_> = self.event_bridges.iter().collect();
        events.sort_by(|a, b| a.name.cmp(&b.name));
        for event in events {
            field(hasher, "event", &event.name);
            field(hasher, "event_emit_count", &event.emits.len().to_string());
            field(
                hasher,
                "event_listen_count",
                &event.listens.len().to_string(),
            );
        }
    }

    fn cache_snapshot_paths(root: &Path) -> Vec<PathBuf> {
        let cache_dir = project_cache_dir(root);
        let mut paths = Vec::new();
        if let Some(seg) = Self::git_context_for(root).scan_id {
            paths.push(cache_dir.join(seg).join(SNAPSHOT_FILE));
        }
        // Cache flat fallback (non-git or pre-git-layout artifacts)
        paths.push(cache_dir.join(SNAPSHOT_FILE));
        paths
    }

    fn legacy_snapshot_paths(root: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Some(seg) = Self::git_context_for(root).scan_id {
            paths.push(root.join(SNAPSHOT_DIR).join(seg).join(SNAPSHOT_FILE));
        }
        paths.push(root.join(SNAPSHOT_DIR).join(SNAPSHOT_FILE));
        paths
    }

    fn candidate_snapshot_paths(root: &Path) -> Vec<PathBuf> {
        // Always prefer cache paths over legacy project-local paths.
        let mut paths = Self::cache_snapshot_paths(root);
        paths.extend(Self::legacy_snapshot_paths(root));
        paths
    }

    fn first_existing_path(paths: &[PathBuf]) -> Option<PathBuf> {
        paths.iter().find(|p| p.exists()).cloned()
    }

    fn newest_snapshot_path(snapshots: &mut [(PathBuf, std::time::SystemTime)]) -> Option<PathBuf> {
        snapshots.sort_by_key(|b| std::cmp::Reverse(b.1));
        snapshots.first().map(|(path, _)| path.clone())
    }

    fn warn_dual_snapshot_sources(cache_path: &Path, legacy_path: &Path) {
        eprintln!(
            "[loctree][warn] Both cache and legacy snapshots found; using cache: {} (legacy ignored: {})",
            cache_path.display(),
            legacy_path.display()
        );
    }

    fn cache_path_for_legacy_snapshot(root: &Path, legacy_snapshot_path: &Path) -> PathBuf {
        let legacy_base = root.join(SNAPSHOT_DIR);
        let cache_dir = project_cache_dir(root);
        if let Ok(relative) = legacy_snapshot_path.strip_prefix(&legacy_base)
            && relative.ends_with(Path::new(SNAPSHOT_FILE))
        {
            return cache_dir.join(relative);
        }
        Self::snapshot_path(root)
    }

    fn write_legacy_migration_marker(
        root: &Path,
        legacy_snapshot_path: &Path,
        cache_snapshot_path: &Path,
    ) -> io::Result<()> {
        let legacy_dir = root.join(SNAPSHOT_DIR);
        fs::create_dir_all(&legacy_dir)?;
        let marker_path = legacy_dir.join(LEGACY_MIGRATION_MARKER);
        if marker_path.exists() {
            return Ok(());
        }
        let marker_contents = format!(
            "legacy_snapshot={}\ncache_snapshot={}\n",
            legacy_snapshot_path.display(),
            cache_snapshot_path.display()
        );
        write_atomic(&marker_path, marker_contents)
    }

    /// Reads the legacy snapshot file and copies it to the global cache directory.
    ///
    /// Safety: `legacy_snapshot_path` is validated via canonicalization and
    /// `starts_with` to ensure it resides within `root`. The validated path
    /// is rebuilt from its canonical components before any filesystem read.
    fn migrate_legacy_snapshot_to_cache(
        root: &Path,
        legacy_snapshot_path: &Path,
    ) -> io::Result<PathBuf> {
        // Canonicalize both paths to resolve symlinks and ".." components
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let canonical_legacy = legacy_snapshot_path
            .canonicalize()
            .unwrap_or_else(|_| legacy_snapshot_path.to_path_buf());

        // Extract the relative portion within the project root
        let relative = canonical_legacy
            .strip_prefix(&canonical_root)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "Legacy snapshot path escapes project root: {}",
                        legacy_snapshot_path.display()
                    ),
                )
            })?;

        // Reject any path component that attempts traversal
        for component in relative.components() {
            if let std::path::Component::ParentDir = component {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Path traversal detected in snapshot path",
                ));
            }
        }

        // canonical_legacy is already canonicalized and verified to be under canonical_root.
        // Keep the validated path as-is to avoid rebuilding from potentially tainted pieces.
        let validated_path = canonical_legacy;

        let cache_snapshot_path = Self::cache_path_for_legacy_snapshot(root, &validated_path);
        if cache_snapshot_path.exists() {
            return Ok(cache_snapshot_path);
        }

        // SaaS-safety: re-assert containment immediately before the read so
        // Semgrep's `tainted-path` analysis can see the boundary guard right
        // next to the I/O sink (rather than trusting the earlier
        // `strip_prefix` check 30 lines up). `read_file_under_root` re-runs
        // canonicalize + `starts_with` against `canonical_root`.
        let bytes = read_file_under_root(&canonical_root, &validated_path)?;
        if let Some(parent) = cache_snapshot_path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(&cache_snapshot_path, bytes)?;

        if let Err(err) =
            Self::write_legacy_migration_marker(root, legacy_snapshot_path, &cache_snapshot_path)
        {
            eprintln!(
                "[loctree][warn] Snapshot migrated but failed to write migration marker: {}",
                err
            );
        }

        eprintln!(
            "[loctree][info] Migrated legacy snapshot to cache: {} -> {}",
            legacy_snapshot_path.display(),
            cache_snapshot_path.display()
        );

        Ok(cache_snapshot_path)
    }

    /// Get the snapshot file path for a given root (writes go here).
    ///
    /// Returns a path under the global cache directory.
    pub fn snapshot_path(root: &Path) -> PathBuf {
        let cache_dir = project_cache_dir(root);
        if let Some(seg) = Self::git_context_for(root).scan_id {
            cache_dir.join(seg).join(SNAPSHOT_FILE)
        } else {
            cache_dir.join(SNAPSHOT_FILE)
        }
    }

    /// Directory where snapshot and artifacts should be stored for the current scan.
    ///
    /// Returns a path under the global cache directory.
    pub fn artifacts_dir(root: &Path) -> PathBuf {
        let path = Self::snapshot_path(root);
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| project_cache_dir(root))
    }

    fn remove_any_path(path: &Path) {
        let meta = fs::symlink_metadata(path);
        let Ok(meta) = meta else {
            return;
        };
        if meta.is_dir() {
            let _ = fs::remove_dir_all(path);
        } else {
            let _ = fs::remove_file(path);
        }
    }

    fn refresh_latest_artifacts(root: &Path) -> io::Result<()> {
        let Some(scan_id) = Self::git_context_for(root).scan_id else {
            return Ok(());
        };

        let base_dir = project_cache_dir(root);
        let scan_dir = base_dir.join(&scan_id);
        if !scan_dir.exists() {
            return Ok(());
        }

        // Validate scan_dir is contained within base_dir (prevent path traversal via crafted git refs)
        let canon_base = base_dir.canonicalize().unwrap_or_else(|_| base_dir.clone());
        let canon_scan = scan_dir.canonicalize().unwrap_or_else(|_| scan_dir.clone());
        if !canon_scan.starts_with(&canon_base) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "scan directory escapes cache base: {}",
                    canon_scan.display()
                ),
            ));
        }

        let latest_dir = base_dir.join("latest");
        Self::remove_any_path(&latest_dir);
        fs::create_dir_all(&latest_dir)?;

        // Keep this list small and stable; these are the key artifacts CI and
        // tooling depend on. Other ad-hoc outputs (e.g. circular.json) can
        // still be found in the scan_id dir.
        //
        // SaaS-safety: each pointer is mirrored through
        // `mirror_pointer_artifact`, which takes a `&'static str` and only
        // joins it onto trusted, pre-validated roots (`canon_scan`,
        // `base_dir`, `latest_dir`). Listing pointers as separate literal
        // arguments — rather than iterating a `&[&str]` by reference — keeps
        // Semgrep's `tainted-path` analysis able to see that the filename
        // component is a compile-time constant on its own. No `nosemgrep`
        // suppression required.
        Self::mirror_pointer_artifact(
            &canon_scan,
            &base_dir,
            &latest_dir,
            StaticAssetName::new("snapshot.json"),
        )?;
        Self::mirror_pointer_artifact(
            &canon_scan,
            &base_dir,
            &latest_dir,
            StaticAssetName::new("analysis.json"),
        )?;
        Self::mirror_pointer_artifact(
            &canon_scan,
            &base_dir,
            &latest_dir,
            StaticAssetName::new("findings.json"),
        )?;
        Self::mirror_pointer_artifact(
            &canon_scan,
            &base_dir,
            &latest_dir,
            StaticAssetName::new("agent.json"),
        )?;
        Self::mirror_pointer_artifact(
            &canon_scan,
            &base_dir,
            &latest_dir,
            StaticAssetName::new("manifest.json"),
        )?;
        Self::mirror_pointer_artifact(
            &canon_scan,
            &base_dir,
            &latest_dir,
            StaticAssetName::new("report.sarif"),
        )?;
        Self::mirror_pointer_artifact(
            &canon_scan,
            &base_dir,
            &latest_dir,
            StaticAssetName::new("dead.json"),
        )?;
        Self::mirror_pointer_artifact(
            &canon_scan,
            &base_dir,
            &latest_dir,
            StaticAssetName::new("handlers.json"),
        )?;
        Self::mirror_pointer_artifact(
            &canon_scan,
            &base_dir,
            &latest_dir,
            StaticAssetName::new("circular.json"),
        )?;
        Self::mirror_pointer_artifact(
            &canon_scan,
            &base_dir,
            &latest_dir,
            StaticAssetName::new("py_races.json"),
        )?;
        Self::mirror_pointer_artifact(
            &canon_scan,
            &base_dir,
            &latest_dir,
            StaticAssetName::new("report.html"),
        )?;

        Ok(())
    }

    /// Mirror one CI-stable artifact from the per-scan cache directory into
    /// both the flat base-dir pointer location and the `latest/` mirror.
    ///
    /// `name` is a [`StaticAssetName`] by design: callers must pass a
    /// compile-time string literal so Semgrep's `tainted-path` analysis
    /// can prove the filename component is not derived from operator or
    /// MCP input. The source read goes through
    /// [`crate::fs_utils::SanitizedPath`] anchored at `canon_scan` so the
    /// boundary guard sits at the same call site as the `fs::read` sink.
    /// Both destination paths are rooted in callers' pre-validated
    /// directories (`base_dir` and `latest_dir`, derived from the same
    /// canonical cache root). The function is a no-op when the source
    /// artifact does not exist — a common case because not every analyzer
    /// emits every pointer.
    fn mirror_pointer_artifact(
        canon_scan: &Path,
        base_dir: &Path,
        latest_dir: &Path,
        name: StaticAssetName,
    ) -> io::Result<()> {
        let src = canon_scan.join(name.as_str());
        if !src.exists() {
            return Ok(());
        }
        let bytes = crate::fs_utils::read_within(canon_scan, &src)?;

        // Stable pointers at base dir: `.loctree/agent.json`, `.loctree/findings.json`, ...
        // These are ignored by default gitignore (config.toml/suppressions.toml are explicitly unignored).
        let dst_flat = base_dir.join(name.as_str());
        write_atomic(&dst_flat, &bytes)?;

        // Mirror into `.loctree/latest/` for snapshot-as-proof workflows.
        let dst_latest = latest_dir.join(name.as_str());
        write_atomic(&dst_latest, &bytes)?;

        Ok(())
    }

    /// Check if a snapshot exists for the given root (checks both cache and legacy paths).
    pub fn exists(root: &Path) -> bool {
        Self::candidate_snapshot_paths(root)
            .iter()
            .any(|p| p.exists())
    }

    /// Walk upward from `start` looking for a loctree project root.
    ///
    /// A directory is considered a root if it has a `.loctree/` config dir
    /// (user-editable files like config.toml, suppressions.toml) OR if its
    /// global cache directory contains at least one snapshot.
    ///
    /// Stops at git boundaries: once we pass a `.git` directory without finding
    /// a loctree root, we don't continue into unrelated parent directories.
    pub fn find_loctree_root(start: &Path) -> Option<PathBuf> {
        let mut current = start.canonicalize().ok()?;
        let mut passed_git = false;
        loop {
            // Check for .loctree config dir (config.toml, suppressions.toml, .loctreeignore)
            if current.join(SNAPSHOT_DIR).exists() {
                return Some(current);
            }
            // Check global cache — require an actual snapshot, not just an empty dir
            let cache = project_cache_dir(&current);
            if cache.is_dir() && Self::cache_has_snapshot(&cache) {
                return Some(current);
            }
            // Track git boundaries — don't walk past a .git into unrelated parents
            if current.join(".git").exists() {
                if passed_git {
                    // Already passed one git root, don't walk into another project
                    return None;
                }
                passed_git = true;
            }
            match current.parent() {
                Some(parent) if parent != current => current = parent.to_path_buf(),
                _ => return None,
            }
        }
    }

    /// Returns true if a cache directory contains at least one snapshot.json.
    fn cache_has_snapshot(cache_dir: &Path) -> bool {
        if cache_dir.join(SNAPSHOT_FILE).exists() {
            return true;
        }
        if let Ok(entries) = std::fs::read_dir(cache_dir) {
            for entry in entries.flatten() {
                if entry.path().join(SNAPSHOT_FILE).exists() {
                    return true;
                }
            }
        }
        false
    }

    /// Normalize a path to be relative to snapshot roots
    ///
    /// Handles:
    /// - Absolute paths: strips snapshot root prefix
    /// - Relative paths with ./: removes the prefix
    /// - Windows paths: normalizes backslashes to forward slashes
    ///
    /// # Examples
    /// ```ignore
    /// // Given snapshot with root "/home/foo/project"
    /// snapshot.normalize_path("/home/foo/project/src/main.rs") // => "src/main.rs"
    /// snapshot.normalize_path("./src/main.rs") // => "src/main.rs"
    /// snapshot.normalize_path("src\\main.rs") // => "src/main.rs"
    /// ```
    pub fn normalize_path(&self, path: &str) -> String {
        let path = path.trim_start_matches("./").replace('\\', "/");

        // If path is absolute, try to strip snapshot root prefixes
        if path.starts_with('/') {
            for root in &self.metadata.roots {
                let root_normalized = root.trim_end_matches('/');
                if let Some(relative) = path.strip_prefix(root_normalized) {
                    // Remove leading slash from relative path
                    return relative.trim_start_matches('/').to_string();
                }
            }
        }

        path
    }

    /// Find the most recent snapshot in .loctree/*/snapshot.json
    ///
    /// This function is useful for query mode where we want to automatically
    /// discover the latest snapshot without requiring explicit path specification.
    ///
    /// # Arguments
    /// * `explicit_path` - If provided, use this path directly instead of searching
    ///
    /// # Returns
    /// * `Ok(PathBuf)` - Path to the snapshot file
    /// * `Err(String)` - Helpful error message if no snapshot found
    ///
    /// # Example
    /// ```ignore
    /// // Auto-discover latest snapshot
    /// let path = Snapshot::find_latest_snapshot(None)?;
    ///
    /// // Use explicit path
    /// let path = Snapshot::find_latest_snapshot(Some(Path::new(".loctree/main@abc123/snapshot.json")))?;
    /// ```
    pub fn find_latest_snapshot(explicit_path: Option<&Path>) -> Result<PathBuf, String> {
        // If explicit path provided, validate and return it
        if let Some(path) = explicit_path {
            if path.exists() {
                return Ok(path.to_path_buf());
            } else {
                return Err(format!(
                    "Snapshot not found at '{}'. Run `loct scan` first.",
                    path.display()
                ));
            }
        }

        // Search for .loctree directory starting from current directory
        let cwd = std::env::current_dir()
            .map_err(|e| format!("Failed to get current directory: {}", e))?;

        Self::find_latest_snapshot_in(&cwd)
    }

    /// Find latest snapshot starting from a given root directory.
    /// Prefers global cache as source of truth and falls back to legacy `.loctree/` only if needed.
    pub fn find_latest_snapshot_in(root: &Path) -> Result<PathBuf, String> {
        let effective_root =
            Self::find_loctree_root(root).unwrap_or_else(|| normalize_root_dir(root));

        let mut cache_snapshots: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
        let mut legacy_snapshots: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

        // Search global cache directory for this project (effective root, not CWD)
        let cache_dir = project_cache_dir(&effective_root);
        Self::collect_snapshots_from_dir(&cache_dir, &mut cache_snapshots);

        // Search legacy project-local .loctree/ directory
        let legacy_dir = effective_root.join(SNAPSHOT_DIR);
        Self::collect_snapshots_from_dir(&legacy_dir, &mut legacy_snapshots);

        let cache_latest = Self::newest_snapshot_path(&mut cache_snapshots);
        let legacy_latest = Self::newest_snapshot_path(&mut legacy_snapshots);

        match (cache_latest, legacy_latest) {
            (Some(cache_path), Some(legacy_path)) => {
                Self::warn_dual_snapshot_sources(&cache_path, &legacy_path);
                Ok(cache_path)
            }
            (Some(cache_path), None) => Ok(cache_path),
            (None, Some(legacy_path)) => {
                match Self::migrate_legacy_snapshot_to_cache(&effective_root, &legacy_path) {
                    Ok(migrated) => Ok(migrated),
                    Err(err) => {
                        eprintln!(
                            "[loctree][warn] Failed to migrate legacy snapshot to cache, using legacy path: {}",
                            err
                        );
                        Ok(legacy_path)
                    }
                }
            }
            (None, None) => {
                Err("No snapshot found. Run `loct scan` first to create one.".to_string())
            }
        }
    }

    /// Collect all snapshot.json files from a directory (flat + subdirs).
    fn collect_snapshots_from_dir(
        dir: &Path,
        snapshots: &mut Vec<(PathBuf, std::time::SystemTime)>,
    ) {
        // Check flat: dir/snapshot.json
        let flat_path = dir.join(SNAPSHOT_FILE);
        if let Ok(meta) = fs::metadata(&flat_path)
            && let Ok(mtime) = meta.modified()
        {
            snapshots.push((flat_path, mtime));
        }
        // Check subdirs: dir/*/snapshot.json
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let snapshot_path = path.join(SNAPSHOT_FILE);
                    if let Ok(meta) = fs::metadata(&snapshot_path)
                        && let Ok(mtime) = meta.modified()
                    {
                        snapshots.push((snapshot_path, mtime));
                    }
                }
            }
        }
    }

    /// Check if the git HEAD has moved since this snapshot was created.
    ///
    /// This is a lightweight check (commit hash comparison only).
    /// Returns `false` for non-git directories.
    pub fn is_commit_stale(&self, root: &Path) -> bool {
        if let Some(snapshot_commit) = &self.metadata.git_commit
            && let Ok(repo) = crate::git::GitRepo::discover(root)
            && let Ok(current_commit) = repo.head_commit()
        {
            // Snapshot stores short hash, head_commit() returns full —
            // prefix comparison handles both directions.
            let is_same = current_commit.starts_with(snapshot_commit)
                || snapshot_commit.starts_with(&current_commit);
            return !is_same;
        }
        false
    }

    /// Check if this snapshot is stale relative to the current repository state.
    ///
    /// A snapshot is considered stale if:
    /// - Git HEAD has moved since the snapshot was created (commit mismatch)
    /// - The worktree has uncommitted changes (dirty worktree)
    ///
    /// Use `is_commit_stale()` for a cheaper check that ignores dirty worktree
    /// (suitable for CLI commands where rescanning on every dirty state is too aggressive).
    ///
    /// Returns `false` for non-git directories (no staleness concept without VCS).
    pub fn is_stale(&self, root: &Path) -> bool {
        if self.is_commit_stale(root) {
            return true;
        }
        // Check dirty worktree: uncommitted changes mean snapshot may not
        // reflect the files on disk (the common refactoring scenario). A
        // freshly saved snapshot of an already-dirty tree is not stale,
        // though: the reuse fence proves whether the indexed bytes still
        // match what this snapshot captured.
        if matches!(is_git_dirty(root), Some(true)) {
            return !self.reuse_fence_matches(root).unwrap_or(false);
        }
        false
    }

    /// Check whether sampled source files have changed since this snapshot was generated.
    ///
    /// The walk is intentionally bounded so callers can keep a fast warm-cache path.
    /// A positive result means the snapshot should be refreshed through the normal
    /// incremental scan path; unchanged files will still come from cached analyses.
    pub fn is_stale_by_mtime(&self, root: &Path) -> bool {
        self.files_changed_since_scan(root, 100).unwrap_or(0) > 0
    }

    /// Count sampled files whose filesystem mtime is newer than snapshot generation time.
    pub fn files_changed_since_scan(&self, root: &Path, sample_limit: usize) -> io::Result<usize> {
        let Some(scan_time) = self.generated_at_system_time() else {
            return Ok(0);
        };

        let mut options = crate::types::Options {
            ignore_paths: vec![root.join(SNAPSHOT_DIR), project_cache_dir(root)],
            use_gitignore: true,
            ..Default::default()
        };
        if options.ignore_paths.iter().any(|path| path.is_relative()) {
            options.ignore_paths = options
                .ignore_paths
                .into_iter()
                .map(|path| root.join(path))
                .collect();
        }
        let git_checker = crate::fs_utils::GitIgnoreChecker::new(root);
        let mut visited = HashSet::new();
        let mut files = Vec::new();
        crate::fs_utils::gather_files(
            root,
            &options,
            0,
            git_checker.as_ref(),
            &mut visited,
            &mut files,
        )?;

        let mut changed = 0;
        for path in files.into_iter().take(sample_limit) {
            if fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .map(|mtime| mtime > scan_time)
                .unwrap_or(false)
            {
                changed += 1;
            }
        }
        Ok(changed)
    }

    pub fn is_older_than(&self, max_age: Duration) -> bool {
        let Some(scan_time) = self.generated_at_system_time() else {
            return false;
        };
        SystemTime::now()
            .duration_since(scan_time)
            .map(|age| age > max_age)
            .unwrap_or(false)
    }

    fn generated_at_system_time(&self) -> Option<SystemTime> {
        let parsed = chrono::DateTime::parse_from_rfc3339(&self.metadata.generated_at).ok()?;
        let secs = parsed.timestamp();
        if secs >= 0 {
            UNIX_EPOCH.checked_add(Duration::new(secs as u64, parsed.timestamp_subsec_nanos()))
        } else {
            UNIX_EPOCH.checked_sub(Duration::new(
                secs.unsigned_abs(),
                parsed.timestamp_subsec_nanos(),
            ))
        }
    }

    /// Save snapshot to disk.
    ///
    /// Always writes — the previous "skip if same commit" optimization was removed
    /// because it caused stale snapshots to persist through refactoring workflows
    /// (the core use case for loctree). Atomic writes keep this fast enough.
    pub fn save(&self, root: &Path) -> io::Result<()> {
        let _cache_lock = acquire_snapshot_cache_lock(root)?;
        let snapshot_path = Self::snapshot_path(root);
        if let Some(dir) = snapshot_path.parent() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        write_atomic(&snapshot_path, json)?;

        // Best-effort content fence for dirty-worktree query reuse. Synthetic
        // unit-test snapshots often reference files that do not exist on disk,
        // so failure here must not make ordinary snapshot persistence brittle.
        let _ = self.refresh_reuse_fence(root);

        // Refresh stable pointers (base_dir/*.json + base_dir/latest/) for CI and human workflows.
        // This is a no-op for non-git dirs (no scan_id).
        let _ = Self::refresh_latest_artifacts(root);

        Ok(())
    }

    /// Load snapshot from disk (used by VS2 slice module)
    pub fn load(root: &Path) -> io::Result<Self> {
        let _cache_lock = acquire_snapshot_cache_lock(root)?;
        let cache_candidates = Self::cache_snapshot_paths(root);
        let legacy_candidates = Self::legacy_snapshot_paths(root);
        let cache_snapshot = Self::first_existing_path(&cache_candidates);
        let legacy_snapshot = Self::first_existing_path(&legacy_candidates);

        let snapshot_path = match (cache_snapshot, legacy_snapshot) {
            (Some(cache_path), Some(legacy_path)) => {
                Self::warn_dual_snapshot_sources(&cache_path, &legacy_path);
                cache_path
            }
            (Some(cache_path), None) => cache_path,
            (None, Some(legacy_path)) => {
                match Self::migrate_legacy_snapshot_to_cache(root, &legacy_path) {
                    Ok(migrated) => migrated,
                    Err(err) => {
                        eprintln!(
                            "[loctree][warn] Failed to migrate legacy snapshot to cache, using legacy path: {}",
                            err
                        );
                        legacy_path
                    }
                }
            }
            (None, None) => {
                let primary = Self::snapshot_path(root);
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "No snapshot found. Run `loctree` first to create one.\nExpected: {}",
                        primary.display()
                    ),
                ));
            }
        };

        // SaaS-safety: `snapshot_path` originated from one of two trusted
        // roots — the project's global cache directory or the project's
        // local `.loctree/` config directory — but in a SaaS context `root`
        // itself arrives from `--root`, `LOCT_CACHE_DIR`, or the MCP
        // payload. Re-assert containment against both trusted roots
        // immediately before the read so Semgrep's `tainted-path` analysis
        // can see the boundary guard right next to the I/O sink.
        let content = Self::read_snapshot_within_trusted_roots(root, &snapshot_path)?;
        let snapshot: Self = serde_json::from_str(&content)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // Check schema version compatibility (major.minor only - patch bumps don't change schema)
        if schema_major_minor(&snapshot.metadata.schema_version)
            != schema_major_minor(SNAPSHOT_SCHEMA_VERSION)
        {
            eprintln!(
                "[loctree][warn] Snapshot schema version mismatch: found {}, expected {}. Consider re-running `loctree`.",
                snapshot.metadata.schema_version, SNAPSHOT_SCHEMA_VERSION
            );
        }

        Ok(snapshot)
    }

    /// Read a snapshot JSON file after re-asserting that its canonical form
    /// lives under one of the two trusted snapshot roots for `root`: the
    /// global cache directory (`project_cache_dir`) or the project-local
    /// `.loctree/` directory (`SNAPSHOT_DIR`). Returns the file's contents
    /// as UTF-8.
    ///
    /// SaaS-safety helper for [`Self::load`]: the multi-root variant of
    /// [`crate::fs_utils::SanitizedPath`] canonicalizes the snapshot path
    /// and asserts membership under at least one trusted root immediately
    /// before the `fs::read_to_string` sink, so the boundary guard is
    /// visible to Semgrep's `tainted-path` analysis at the same call site
    /// as the I/O.
    fn read_snapshot_within_trusted_roots(root: &Path, snapshot_path: &Path) -> io::Result<String> {
        let cache_root = project_cache_dir(root);
        let local_root = root.join(SNAPSHOT_DIR);
        crate::fs_utils::read_to_string_within_any(
            &[cache_root.as_path(), local_root.as_path()],
            snapshot_path,
        )
    }

    /// Get map of file path -> FileAnalysis for incremental reuse
    pub fn cached_analyses(&self) -> HashMap<String, FileAnalysis> {
        self.files
            .iter()
            .map(|f| (f.path.clone(), f.clone()))
            .collect()
    }

    /// Update metadata after scan
    pub fn finalize_metadata(&mut self, scan_duration_ms: u64) {
        self.metadata.file_count = self.canonical_file_count();
        self.metadata.total_loc = self.files.iter().map(|f| f.loc).sum();
        self.metadata.scan_duration_ms = scan_duration_ms;

        // Collect languages from files
        for file in &self.files {
            if !file.language.is_empty() {
                self.metadata.languages.insert(file.language.clone());
            }
        }
    }

    /// Print summary of the snapshot
    pub fn print_summary(&self, root: &Path) {
        let snapshot_path = Self::snapshot_path(root);
        let pretty_path = snapshot_path
            .strip_prefix(root)
            .map(|p| format!("./{}", p.display()))
            .unwrap_or_else(|_| snapshot_path.display().to_string());
        crate::progress::info(&format!("Saved to {}", pretty_path));

        let languages: Vec<_> = self.metadata.languages.iter().collect();
        if !languages.is_empty() {
            eprintln!(
                "Languages: {}",
                languages
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        let handler_count = self
            .command_bridges
            .iter()
            .filter(|b| b.has_handler)
            .count();
        let missing_handlers = self
            .command_bridges
            .iter()
            .filter(|b| !b.has_handler && b.is_called)
            .count();
        let unused_handlers = self
            .command_bridges
            .iter()
            .filter(|b| b.has_handler && !b.is_called)
            .count();

        if handler_count > 0 || missing_handlers > 0 {
            eprint!("Commands: {} handlers", handler_count);
            if missing_handlers > 0 {
                eprint!(", {} missing", missing_handlers);
            }
            if unused_handlers > 0 {
                eprint!(", {} unused", unused_handlers);
            }
            eprintln!();
        }

        let event_count = self.event_bridges.len();
        if event_count > 0 {
            eprintln!("Events: {} tracked", event_count);
        }

        // Check for cycles or issues
        let barrel_count = self.barrels.len();
        if barrel_count > 0 {
            eprintln!("Barrels: {} detected", barrel_count);
        }

        // Count duplicate exports (symbols exported from multiple files)
        let duplicate_count = self
            .export_index
            .values()
            .filter(|files| files.len() > 1)
            .count();
        if duplicate_count > 0 {
            eprintln!("Duplicates: {} export groups", duplicate_count);
        }

        // Count indexed parameters (NEW in 0.8.4)
        let param_count: usize = self
            .files
            .iter()
            .flat_map(|f| f.exports.iter())
            .map(|e| e.params.len())
            .sum();
        if param_count > 0 {
            let func_with_params = self
                .files
                .iter()
                .flat_map(|f| f.exports.iter())
                .filter(|e| !e.params.is_empty())
                .count();
            eprintln!(
                "Params: {} indexed ({} functions)",
                param_count, func_with_params
            );
        }

        eprintln!("Status: OK");
        eprintln!();
        eprintln!("Next steps:");
        eprintln!("  loct --for-ai                # Project overview for AI agents");
        eprintln!("  loct context                 # Atlas + pill + memory continuity");
        eprintln!("  loct slice <file> --json     # Extract context with dependencies");
        eprintln!("  loct twins                   # Dead parrots + duplicates + barrel chaos");
        eprintln!("  loct '.files | length'       # jq-style queries on snapshot");
        eprintln!("  loct query who-imports <f>   # Quick graph queries");
    }
}

/// Best-effort check for uncommitted changes in the working tree.
///
/// Returns `Some(true)` if dirty, `Some(false)` if clean, `None` if `root`
/// is not inside a git repository (or git is unreachable).
///
/// `Command::output()` returns Ok even when git itself exits non-zero
/// (e.g. "fatal: not a git repository"), so we MUST inspect
/// `output.status.success()` before treating empty stdout as "clean".
/// Otherwise non-git dirs would falsely report as `Some(false)`.
///
/// Loct's own `.loctree/` artifacts (context-atlas, reports, logs) are
/// excluded: they are never indexed, so they can never invalidate snapshot
/// content — counting them as dirt made the guardian [DRIFT]-rescan forever
/// on repos where `.loctree/` is not gitignored.
pub fn is_git_dirty(root: &Path) -> Option<bool> {
    let output = Command::new("git")
        .arg("status")
        .arg("--porcelain")
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Some(!parse_git_status_paths(&stdout).is_empty())
}

// ============================================================================
// Snapshot freshness authority (the guardian)
// ============================================================================
//
// `acquire_snapshot` is the ONLY place that decides whether an existing
// snapshot is fresh enough to serve, must be reused via the content fence,
// can be trusted because a live `loct watch` keeps it current, or has to be
// rebuilt. Every consumer (CLI dispatch, `loct find`, slicer, dist analysis,
// the MCP server) goes through this function instead of hand-rolling its own
// staleness logic. Internal rescans triggered here use `unified_scan_args`
// so the rebuilt snapshot covers the SAME file universe as the initial scan
// (previously: detect-narrowed initial scan vs. default-extension rescan
// produced 673 vs 767 files and a self-sustaining [DRIFT] loop).

/// How aggressively an existing snapshot may be reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SnapshotReusePolicy {
    /// Rescan whenever git HEAD moved or the worktree is dirty.
    /// For analytic commands where content truth matters (dead, twins, ...).
    #[default]
    Strict,
    /// Reuse the snapshot when the content fence proves the indexed bytes
    /// did not change (query/occurrences/find fast path).
    ReuseFence,
    /// Trust any existing snapshot with a matching scope; scan only when the
    /// snapshot is missing. Use only for callers that can tolerate stale data.
    TrustExisting,
}

/// Options for `acquire_snapshot`. Mirrors the CLI global flags without
/// depending on the CLI module so the MCP server can construct it from lib.
#[derive(Clone, Debug)]
pub struct AcquireOptions {
    /// Force rescan even if a snapshot exists (`--fresh`).
    pub fresh: bool,
    /// Fail (or, with `no_scan_uses_stale`, serve stale) instead of scanning (`--no-scan`).
    pub no_scan: bool,
    /// Fail when the snapshot is stale — CI mode (`--fail-stale`).
    pub fail_stale: bool,
    /// Suppress freshness notes ([DRIFT]/[REUSE_FENCE]/...) on stderr.
    pub quiet: bool,
    /// Verbose freshness diagnostics.
    pub verbose: bool,
    /// Internal rescans emit JSON-mode scan output.
    pub json: bool,
    /// Print the full scan summary after an internal rescan (interactive UX).
    pub print_scan_summary: bool,
    /// MCP semantics: with `no_scan`, return the stale snapshot instead of erroring.
    pub no_scan_uses_stale: bool,
    /// Internal rescans walk the full tree instead of incremental (dist exact-scope path).
    pub full_scan: bool,
    /// Snapshot root resolution strategy (Project walks to git root; Exact pins the dir).
    pub strategy: SnapshotRootStrategy,
    /// Override `.loctignore` for this acquisition only: build an ephemeral,
    /// non-persisted superset snapshot that also contains files normally
    /// excluded by `.loctignore`, each marked `ignored=true`. The persisted
    /// project snapshot and every default command are left untouched. Does not
    /// override `.gitignore` or heavy-directory presets.
    pub include_ignored: bool,
}

impl Default for AcquireOptions {
    fn default() -> Self {
        Self {
            fresh: false,
            no_scan: false,
            fail_stale: false,
            quiet: false,
            verbose: false,
            json: false,
            print_scan_summary: false,
            no_scan_uses_stale: false,
            full_scan: false,
            strategy: SnapshotRootStrategy::Project,
            include_ignored: false,
        }
    }
}

/// Build the canonical scan arguments for a root: the SAME universe the
/// interactive `loct` entrypoint uses (default analyzer extensions + detect
/// ignores/presets + `.loctignore`; stack detection intentionally no longer
/// narrows extensions). Every internal rescan must go through this builder
/// so initial scan and rescan agree on the file set.
pub fn unified_scan_args(root: &Path, verbose: bool) -> ParsedArgs {
    unified_scan_args_with_ignore(root, verbose, false)
}

/// Same as [`unified_scan_args`], but when `include_ignored` is true the
/// `.loctignore` patterns are kept OUT of `ignore_patterns` (so those files are
/// gathered) and stored in `loctignore_override_patterns` instead, with
/// `include_ignored` set so the scan can mark them `ignored=true`. Heavy-dir
/// presets and `.gitignore` still apply. Default behavior (`include_ignored =
/// false`) is byte-identical to before.
pub fn unified_scan_args_with_ignore(
    root: &Path,
    verbose: bool,
    include_ignored: bool,
) -> ParsedArgs {
    let mut parsed = ParsedArgs {
        verbose,
        use_gitignore: true,
        ..Default::default()
    };
    let mut library_mode = parsed.library_mode;
    crate::detect::apply_detected_stack(
        root,
        &mut parsed.extensions,
        &mut parsed.ignore_patterns,
        &mut parsed.tauri_preset,
        &mut library_mode,
        &mut parsed.py_roots,
        verbose,
    );
    parsed.library_mode = library_mode;
    let loctignore = crate::fs_utils::load_loctreeignore(root);
    if include_ignored {
        parsed.include_ignored = true;
        parsed.loctignore_override_patterns = loctignore;
    } else {
        parsed.ignore_patterns.extend(loctignore);
    }
    parsed
}

/// True when a live `loct watch` holds the watch lock for `snapshot_root`
/// AND the snapshot's scan_id matches the current git context. In that case
/// the watcher is responsible for freshness and the guardian may trust the
/// snapshot without re-hashing the tree.
pub fn watch_keeps_snapshot_fresh(snapshot: &Snapshot, snapshot_root: &Path) -> bool {
    let Some(snapshot_scan_id) = snapshot.metadata.git_scan_id.as_deref() else {
        return false;
    };
    if !crate::watch_lock::is_held(snapshot_root) {
        return false;
    }
    let current = Snapshot::git_context_for(snapshot_root);
    current.scan_id.as_deref() == Some(snapshot_scan_id)
}

/// Normalize roots exactly as the caller asked for them: relative paths are
/// anchored to the process cwd, not to the resolved snapshot root. This keeps a
/// subdir command like `cd tests/fixtures/foo && loct occurrences x` from
/// accidentally accepting a repo-root snapshot as the same scope.
fn normalize_requested_roots_for_scope_compare(roots: &[PathBuf]) -> Vec<String> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let roots: Vec<PathBuf> = if roots.is_empty() {
        vec![cwd.clone()]
    } else {
        roots.to_vec()
    };
    normalize_roots_for_scope_compare(roots.iter().map(|r| r.as_path()), &cwd)
}

fn canonical_roots_for_scan_metadata(roots: &[PathBuf]) -> Vec<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let roots: Vec<PathBuf> = if roots.is_empty() {
        vec![cwd.clone()]
    } else {
        roots.to_vec()
    };
    roots
        .into_iter()
        .map(|root| {
            let candidate = if root.is_absolute() {
                root
            } else {
                cwd.join(root)
            };
            candidate.canonicalize().unwrap_or(candidate)
        })
        .collect()
}

/// The single snapshot-freshness authority.
///
/// Load an existing snapshot when it is trustworthy, otherwise rescan with
/// the unified file universe. All DRIFT / REUSE_FENCE / watch-fast-path /
/// `--fresh` / `--no-scan` / `--fail-stale` decisions live HERE and only here.
pub fn acquire_snapshot(
    roots: &[PathBuf],
    policy: SnapshotReusePolicy,
    opts: &AcquireOptions,
) -> io::Result<Snapshot> {
    let snapshot_root = resolve_snapshot_root_with_strategy(roots, opts.strategy);
    let requested_roots = normalize_requested_roots_for_scope_compare(roots);

    // `--include-ignored`: build an ephemeral, non-persisted superset that also
    // contains `.loctignore`-excluded files (marked `ignored=true`). The cached
    // project snapshot is intentionally never read or written here, so default
    // commands keep seeing the clean universe regardless of this override read.
    if opts.include_ignored {
        let scan_roots = canonical_roots_for_scan_metadata(roots);
        let universe_root = scan_roots
            .first()
            .cloned()
            .unwrap_or_else(|| snapshot_root.clone());
        let mut parsed = unified_scan_args_with_ignore(&universe_root, opts.verbose, true);
        parsed.full_scan = true;
        parsed.output = if opts.json {
            OutputMode::Json
        } else {
            OutputMode::Human
        };
        // Always quiet: this is an ephemeral override READ, not a scan. Printing
        // a "Saved to ..." summary here would be misleading (nothing is persisted)
        // and would pollute stdout for JSON consumers.
        return build_snapshot_for_strategy(&scan_roots, &parsed, true, opts.strategy, false);
    }

    if !opts.fresh {
        match Snapshot::load(&snapshot_root) {
            Ok(s) => {
                // Guard against stale scope reuse: if snapshot roots differ from
                // requested roots, force a rescan (or fail with --no-scan).
                let snapshot_roots = normalize_roots_for_scope_compare(
                    s.metadata.roots.iter().map(Path::new),
                    &snapshot_root,
                );
                if requested_roots != snapshot_roots {
                    if opts.no_scan {
                        return Err(io::Error::other(format!(
                            "Snapshot scope mismatch and --no-scan is set.\nrequested roots: [{}]\nsnapshot roots:  [{}]\nRun `loct scan` (or the command without --no-scan) to refresh scope.",
                            requested_roots.join(", "),
                            snapshot_roots.join(", ")
                        )));
                    }
                    if !opts.quiet {
                        eprintln!(
                            "[loct] Snapshot roots differ from requested roots; refreshing snapshot scope.\n  requested: [{}]\n  snapshot:  [{}]",
                            requested_roots.join(", "),
                            snapshot_roots.join(", ")
                        );
                    }
                } else if policy == SnapshotReusePolicy::TrustExisting {
                    // Read-verbs: any scope-matching snapshot is good enough.
                    return Ok(s);
                } else if watch_keeps_snapshot_fresh(&s, &snapshot_root) {
                    // Watch fast-path: a live watcher owns freshness for this
                    // root — trust the snapshot without re-hashing the tree.
                    if opts.verbose && !opts.quiet {
                        eprintln!(
                            "[loct] [REUSE_FENCE] via watch: live `loct watch` holds the lock and scan_id matches; trusting snapshot."
                        );
                    }
                    return Ok(s);
                } else if s.is_commit_stale(&snapshot_root) {
                    // Snapshot is stale because git HEAD moved.
                    // --fail-stale / --no-scan: hard fail for CI pipelines.
                    // Default: auto-rescan to keep data fresh (core refactoring workflow).
                    if policy == SnapshotReusePolicy::ReuseFence
                        && !opts.fail_stale
                        && !opts.no_scan
                        && s.reuse_fence_matches(&snapshot_root).unwrap_or(false)
                    {
                        if opts.verbose && !opts.quiet {
                            eprintln!(
                                "[loct] [REUSE_FENCE] snapshot content unchanged across commit; reusing cached snapshot."
                            );
                        }
                        return Ok(s);
                    }

                    if opts.no_scan && opts.no_scan_uses_stale && !opts.fail_stale {
                        if !opts.quiet {
                            eprintln!(
                                "[loct] Snapshot stale, but no_scan=true; using stale snapshot."
                            );
                        }
                        return Ok(s);
                    }
                    if opts.fail_stale || opts.no_scan {
                        let snap_commit = s.metadata.git_commit.as_deref().unwrap_or("unknown");
                        let current = Snapshot::git_context_for(&snapshot_root)
                            .commit
                            .unwrap_or_else(|| "unknown".into());
                        return Err(io::Error::other(format!(
                            "Snapshot is stale: snapshot commit={} but current HEAD={}. Run 'loct scan' to refresh.",
                            &snap_commit[..7.min(snap_commit.len())],
                            &current[..7.min(current.len())]
                        )));
                    }
                    if !opts.quiet {
                        eprintln!("[loct] [DRIFT] Snapshot content changed, rescanning...");
                    }
                } else if matches!(is_git_dirty(&snapshot_root), Some(true)) {
                    if s.reuse_fence_matches(&snapshot_root).unwrap_or(false) {
                        if opts.verbose && !opts.quiet {
                            eprintln!(
                                "[loct] [REUSE_FENCE] snapshot content unchanged; reusing cached snapshot."
                            );
                        }
                        return Ok(s);
                    }

                    if opts.no_scan && opts.no_scan_uses_stale && !opts.fail_stale {
                        if !opts.quiet {
                            eprintln!(
                                "[loct] Snapshot stale (dirty worktree), but no_scan=true; using stale snapshot."
                            );
                        }
                        return Ok(s);
                    }
                    if opts.fail_stale || opts.no_scan {
                        return Err(io::Error::other(
                            "Snapshot content drift detected in dirty worktree. Run 'loct scan' to refresh.",
                        ));
                    }
                    if !opts.quiet {
                        eprintln!("[loct] [DRIFT] Snapshot content changed, rescanning...");
                    }
                } else {
                    // Snapshot is fresh — use it directly.
                    return Ok(s);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // No snapshot - check if --no-scan forbids auto-scan
                if opts.no_scan {
                    return Err(io::Error::other(
                        "No snapshot found and --no-scan is set. Run 'loct' first to create a snapshot.",
                    ));
                }
                if !opts.quiet {
                    eprintln!("[loct] No snapshot found, running initial scan...");
                }
            }
            Err(e) => return Err(e), // Other errors (corruption, etc.) - fail
        }
    } else {
        // --fresh: force rescan
        if !opts.quiet {
            eprintln!("[loct] --fresh: forcing rescan...");
        }
    }

    // Rescan with the unified file universe (same set as the initial scan).
    let scan_roots = canonical_roots_for_scan_metadata(roots);
    let universe_root = scan_roots
        .first()
        .cloned()
        .unwrap_or_else(|| snapshot_root.clone());
    let mut parsed = unified_scan_args(&universe_root, opts.verbose);
    parsed.full_scan = opts.full_scan;
    parsed.output = if opts.json {
        OutputMode::Json
    } else {
        OutputMode::Human
    };

    run_init_with_options_for_strategy(
        &scan_roots,
        &parsed,
        !opts.print_scan_summary,
        opts.strategy,
    )?;

    // Now load the freshly created snapshot
    Snapshot::load(&snapshot_root)
}

/// Run the init command: scan the project and save snapshot
///
/// # Arguments
/// * `root_list` - List of root directories to scan
/// * `parsed` - Parsed command-line arguments
/// * `quiet_summary` - If true, skip printing the summary (useful for internal scans like dist mode)
pub fn run_init_with_options(
    root_list: &[PathBuf],
    parsed: &ParsedArgs,
    quiet_summary: bool,
) -> io::Result<()> {
    run_init_with_options_for_strategy(
        root_list,
        parsed,
        quiet_summary,
        SnapshotRootStrategy::Project,
    )
}

pub fn run_init_with_options_for_strategy(
    root_list: &[PathBuf],
    parsed: &ParsedArgs,
    quiet_summary: bool,
    snapshot_strategy: SnapshotRootStrategy,
) -> io::Result<()> {
    build_snapshot_for_strategy(root_list, parsed, quiet_summary, snapshot_strategy, true)
        .map(|_| ())
}

/// Core scan-and-build routine shared by the persisting init path and the
/// ephemeral `--include-ignored` override. When `persist` is true the built
/// snapshot is saved to the project cache (the classic `run_init` behavior);
/// when false the snapshot is returned without ever touching the cache, so an
/// override read cannot pollute the clean project snapshot.
pub(crate) fn build_snapshot_for_strategy(
    root_list: &[PathBuf],
    parsed: &ParsedArgs,
    quiet_summary: bool,
    snapshot_strategy: SnapshotRootStrategy,
    persist: bool,
) -> io::Result<Snapshot> {
    use crate::analyzer::coverage::{compute_command_gaps, normalize_cmd_name};
    use crate::analyzer::root_scan::{ScanConfig, scan_roots};
    use crate::analyzer::runner::default_analyzer_exts;
    use crate::analyzer::scan::{opt_globset, python_stdlib};
    use crate::config::LoctreeConfig;

    let start_time = Instant::now();
    let mut parsed = parsed.clone();

    // Snapshot root defaults to the first provided root (common UX: keep artifacts near target),
    // falling back to CWD if multiple roots are provided.
    let snapshot_root = resolve_snapshot_root_with_strategy(root_list, snapshot_strategy);

    // First touch of this project: only the very first scan may offer the
    // `.loctree/` gitignore entry (a later missing entry means the operator
    // removed it on purpose). The offer runs before the walk so the generated
    // `.gitignore` participates in the same hidden-truth snapshot as future
    // drift rescans.
    let first_scan = !Snapshot::exists(&snapshot_root);

    // Validate at least one root was specified
    if root_list.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "No root directory specified",
        ));
    }

    // Try to load existing snapshot for incremental scanning.
    // Only reuse cached analyses if the old snapshot is from the same git branch —
    // cross-branch cache reuse can contaminate results (different file contents,
    // same mtimes after branch switch).
    let cached_analyses: Option<HashMap<String, FileAnalysis>> = if !parsed.full_scan
        && Snapshot::exists(&snapshot_root)
    {
        match Snapshot::load(&snapshot_root) {
            Ok(old_snapshot) => {
                // Validate git context: only reuse cache from same branch.
                // After branch switch, files may differ despite same mtimes.
                let current_ctx = Snapshot::git_context_for(&snapshot_root);
                let same_branch = match (&old_snapshot.metadata.git_branch, &current_ctx.branch) {
                    (Some(old_b), Some(cur_b)) => old_b == cur_b,
                    (None, None) => true, // Non-git: always reuse
                    _ => false,
                };
                if same_branch {
                    if parsed.verbose {
                        eprintln!(
                            "[loctree][incremental] Loaded existing snapshot ({} files cached)",
                            old_snapshot.files.len()
                        );
                    }
                    Some(old_snapshot.cached_analyses())
                } else {
                    if parsed.verbose {
                        eprintln!(
                            "[loctree][incremental] Branch changed ({} → {}), full rescan",
                            old_snapshot.metadata.git_branch.as_deref().unwrap_or("?"),
                            current_ctx.branch.as_deref().unwrap_or("?"),
                        );
                    }
                    None
                }
            }
            Err(e) => {
                if parsed.verbose {
                    eprintln!(
                        "[loctree][warn] Could not load snapshot for incremental: {}",
                        e
                    );
                }
                None
            }
        }
    } else {
        None
    };

    // Log scan mode for clarity (especially in CI)
    let scan_mode = if parsed.full_scan {
        "full (--full-scan)"
    } else if cached_analyses.is_some() {
        "incremental (mtime-based)"
    } else {
        "fresh (no existing snapshot)"
    };

    let gitignore_entry_added = if first_scan && !quiet_summary && !gitignore_append_disabled() {
        match ensure_loctree_gitignore_entry(&snapshot_root) {
            Ok(added) => added,
            Err(err) => {
                eprintln!("[loctree][warn] could not add '.loctree/' to .gitignore: {err}");
                false
            }
        }
    } else {
        false
    };

    // Show spinner during scan (Black-style feedback), unless this scan is an
    // internal query refresh where stdout/stderr should stay result-only.
    let spinner = if quiet_summary {
        None
    } else {
        Some(crate::progress::Spinner::new(&format!(
            "Scanning ({})...",
            scan_mode
        )))
    };

    // Prepare scan configuration (reusing existing infrastructure)
    let py_stdlib = python_stdlib();
    let focus_set = opt_globset(&parsed.focus_patterns);
    let exclude_set = opt_globset(&parsed.exclude_report_patterns);

    let base_extensions = parsed
        .extensions
        .clone()
        .or_else(|| Some(default_analyzer_exts()));

    // Load custom Tauri command macros from .loctree/config.toml
    let loctree_config = root_list
        .first()
        .map(|root| LoctreeConfig::load(root))
        .unwrap_or_default();
    parsed.library_mode = parsed.library_mode || loctree_config.library_mode;
    if parsed.library_mode && parsed.library_example_globs.is_empty() {
        parsed.library_example_globs = loctree_config.library_example_globs.clone();
    }
    let command_detection = crate::analyzer::ast_js::CommandDetectionConfig::new(
        &loctree_config.tauri.dom_exclusions,
        &loctree_config.tauri.non_invoke_exclusions,
        &loctree_config.tauri.invalid_command_names,
    )
    .with_event_wrappers(&loctree_config.event_wrappers);
    let custom_command_macros = loctree_config.tauri.command_macros;

    let scan_config = ScanConfig {
        roots: root_list,
        parsed: &parsed,
        extensions: base_extensions,
        focus_set: &focus_set,
        exclude_set: &exclude_set,
        ignore_exact: HashSet::new(),
        ignore_prefixes: Vec::new(),
        py_stdlib: &py_stdlib,
        cached_analyses: cached_analyses.as_ref(),
        collect_edges: true, // Always collect edges for snapshot (needed by slice)
        custom_command_macros: &custom_command_macros,
        command_detection,
    };

    // Perform the scan
    let scan_results = scan_roots(scan_config)?;

    // Second spinner for building snapshot (can take a while for large codebases)
    let build_spinner = if quiet_summary {
        None
    } else {
        Some(crate::progress::Spinner::new("Building snapshot..."))
    };

    // Build the snapshot from scan results
    let mut snapshot = Snapshot::new(root_list.iter().map(|p| p.display().to_string()).collect());

    // Populate files from all contexts
    for ctx in &scan_results.contexts {
        snapshot.files.extend(ctx.analyses.clone());

        // Add graph edges
        for (from, to, label) in &ctx.graph_edges {
            snapshot.edges.push(GraphEdge {
                from: from.clone(),
                to: to.clone(),
                label: label.clone(),
            });
        }

        // Add export index
        for (name, files) in &ctx.export_index {
            snapshot
                .export_index
                .entry(name.clone())
                .or_default()
                .extend(files.clone());
        }

        // Add barrels
        for barrel in &ctx.barrels {
            snapshot.barrels.push(BarrelFile {
                path: barrel.path.clone(),
                module_id: barrel.module_id.clone(),
                reexport_count: barrel.reexport_count,
                targets: barrel.targets.clone(),
            });
        }

        // Collect languages
        for lang in &ctx.languages {
            snapshot.metadata.languages.insert(lang.clone());
        }
    }

    // Merge per-file C-family symbol fragments (Wave B tree-sitter extraction)
    // into the snapshot-level symbol graph. Fragments stay on the FileAnalysis
    // so incremental scans keep symbols for unchanged files.
    {
        let mut symbol_graph = crate::symbols::SymbolGraph::new();
        for file in &snapshot.files {
            if let Some(fragment) = &file.symbol_fragment {
                symbol_graph
                    .symbols
                    .extend(fragment.symbols.iter().cloned());
                symbol_graph
                    .occurrences
                    .extend(fragment.occurrences.iter().cloned());
                symbol_graph.edges.extend(fragment.edges.iter().cloned());
                symbol_graph
                    .file_projection
                    .extend(fragment.file_projection.iter().cloned());
            }
        }
        #[cfg(feature = "deep-index")]
        if let Some(scip_graph) = crate::analyzer::scip::import_indexes(root_list) {
            crate::analyzer::scip::merge_graphs(&mut symbol_graph, scip_graph);
        }
        #[cfg(all(target_os = "macos", feature = "deep-index-macos"))]
        match crate::analyzer::indexstore::ingest_roots(root_list) {
            Ok(Some(ingest)) => {
                if parsed.verbose {
                    eprintln!(
                        "[loctree][indexstore] importing {} existing store(s)",
                        ingest.stores.len()
                    );
                }
                crate::analyzer::indexstore::merge_into_graph(&mut symbol_graph, ingest.graph);
            }
            Ok(None) => {
                if parsed.verbose {
                    eprintln!(
                        "[loctree][indexstore] no existing store/helper found; skipping deep import"
                    );
                }
            }
            Err(err) => {
                eprintln!("[loctree][warn] IndexStore import skipped: {err}");
            }
        }
        // Wave C-1: resolve cross-file usage candidates (Swift intra-module
        // names, C-family unique-name matches) into Heuristic edges and the
        // `file_projection.referenced` lists that back `slice`. Runs after
        // every engine merged so deep-mode symbols also serve as targets;
        // runs before engine counting so dropped unresolved candidates do
        // not inflate occurrence counts.
        {
            let file_imports: std::collections::HashMap<String, Vec<String>> = snapshot
                .files
                .iter()
                .map(|f| {
                    (
                        f.path.clone(),
                        f.imports.iter().map(|i| i.source.clone()).collect(),
                    )
                })
                .collect();
            crate::symbols::resolve::resolve_cross_file(&mut symbol_graph, &file_imports);
        }
        crate::semantic::c_family::enrich_symbol_graph(
            &mut symbol_graph,
            &snapshot.files,
            root_list,
        );
        if !symbol_graph.is_empty() {
            if !symbol_graph
                .engines
                .iter()
                .any(|run| run.engine == crate::symbols::SymbolProvenance::TreeSitter)
                && symbol_graph
                    .symbols
                    .iter()
                    .any(|node| node.provenance == crate::symbols::SymbolProvenance::TreeSitter)
            {
                let occurrence_count = symbol_graph
                    .occurrences
                    .iter()
                    .filter(|occ| occ.engine == crate::symbols::SymbolProvenance::TreeSitter)
                    .count();
                let symbol_count = symbol_graph
                    .symbols
                    .iter()
                    .filter(|node| node.provenance == crate::symbols::SymbolProvenance::TreeSitter)
                    .count();
                symbol_graph.engines.push(crate::symbols::SymbolEngineRun {
                    engine: crate::symbols::SymbolProvenance::TreeSitter,
                    symbol_count,
                    occurrence_count,
                    tool_version: None,
                });
            }
            snapshot.symbol_graph = Some(symbol_graph);
        }
    }

    // Summarize manifests and derive crate roots (ScanOnce -> SliceMany)
    let mut manifest_summary = Vec::new();
    let mut rust_crate_roots = Vec::new();
    for root in root_list {
        let summary = crate::analyzer::manifests::summarize_manifests(root);
        if let Some(cargo) = &summary.cargo_toml {
            rust_crate_roots.extend(cargo.crate_roots.clone());
        }
        manifest_summary.push(summary);
    }
    rust_crate_roots.sort();
    rust_crate_roots.dedup();
    snapshot.metadata.manifest_summary = manifest_summary;

    // Aggregate detected entrypoints for fast lookup
    let entrypoints = crate::analyzer::entrypoints::find_entrypoints(&snapshot.files)
        .into_iter()
        .map(|(path, kinds)| EntrypointSummary { path, kinds })
        .collect();
    snapshot.metadata.entrypoints = entrypoints;
    snapshot.metadata.entrypoint_drift = compute_entrypoint_drift(
        &snapshot.metadata.manifest_summary,
        &snapshot.metadata.entrypoints,
    );

    // Build registered handlers set to filter BE commands (same as in loct.rs/loctree.rs)
    let registered_impls: HashSet<String> = scan_results
        .global_analyses
        .iter()
        .flat_map(|a| a.tauri_registered_handlers.iter().cloned())
        .collect();

    // Filter BE commands to only include registered handlers (or all if no registration info)
    let mut global_be_registered: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
    for (name, locs) in &scan_results.global_be_commands {
        for (path, line, impl_name) in locs {
            if registered_impls.is_empty() || registered_impls.contains(impl_name) {
                global_be_registered.entry(name.clone()).or_default().push((
                    path.clone(),
                    *line,
                    impl_name.clone(),
                ));
            }
        }
    }

    // Build command bridges from global command data
    // Use normalized names for matching (handles camelCase FE vs snake_case BE)
    let (_missing_handlers, _unused_handlers) = compute_command_gaps(
        &scan_results.global_fe_commands,
        &global_be_registered,
        &focus_set,
        &exclude_set,
    );

    // Build normalized lookup maps for cross-matching
    // FE: normalized_name -> original_names (can have multiple originals mapping to same normalized)
    let mut fe_by_norm: HashMap<String, Vec<String>> = HashMap::new();
    for name in scan_results.global_fe_commands.keys() {
        fe_by_norm
            .entry(normalize_cmd_name(name))
            .or_default()
            .push(name.clone());
    }

    // BE: normalized_name -> original_names (only registered handlers)
    let mut be_by_norm: HashMap<String, Vec<String>> = HashMap::new();
    for name in global_be_registered.keys() {
        be_by_norm
            .entry(normalize_cmd_name(name))
            .or_default()
            .push(name.clone());
    }

    // Collect all unique normalized command names
    let mut all_normalized: HashSet<String> = HashSet::new();
    all_normalized.extend(fe_by_norm.keys().cloned());
    all_normalized.extend(be_by_norm.keys().cloned());

    // Create command bridges using normalized matching
    for norm_name in all_normalized {
        // Get all FE original names that normalize to this
        let fe_originals = fe_by_norm.get(&norm_name).cloned().unwrap_or_default();
        // Get all BE original names that normalize to this (registered only)
        let be_originals = be_by_norm.get(&norm_name).cloned().unwrap_or_default();

        // Collect all FE calls (from all original names that map here)
        let fe_calls: Vec<(String, usize)> = fe_originals
            .iter()
            .flat_map(|orig| {
                scan_results
                    .global_fe_commands
                    .get(orig)
                    .map(|v| {
                        v.iter()
                            .map(|(f, l, _)| (f.clone(), *l))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .collect();

        // Get BE handler (prefer first BE original name found, registered only)
        let (be_handler, canonical_name) = be_originals
            .first()
            .and_then(|orig| {
                global_be_registered
                    .get(orig)
                    .and_then(|v| v.first())
                    .map(|(f, l, _)| (Some((f.clone(), *l)), orig.clone()))
            })
            .unwrap_or_else(|| {
                // No BE handler, use first FE name as canonical
                (
                    None,
                    fe_originals.first().cloned().unwrap_or(norm_name.clone()),
                )
            });

        let has_handler = be_handler.is_some();
        let is_called = !fe_calls.is_empty();

        snapshot.command_bridges.push(CommandBridge {
            name: canonical_name,
            frontend_calls: fe_calls,
            backend_handler: be_handler,
            has_handler,
            is_called,
        });
    }

    // Build event bridges from file analyses
    let mut event_emits_map: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
    let mut event_listens_map: HashMap<String, Vec<(String, usize)>> = HashMap::new();

    for file in &snapshot.files {
        for emit in &file.event_emits {
            event_emits_map.entry(emit.name.clone()).or_default().push((
                file.path.clone(),
                emit.line,
                emit.kind.clone(),
            ));
        }
        for listen in &file.event_listens {
            event_listens_map
                .entry(listen.name.clone())
                .or_default()
                .push((file.path.clone(), listen.line));
        }
    }

    let mut all_events: HashSet<String> = HashSet::new();
    all_events.extend(event_emits_map.keys().cloned());
    all_events.extend(event_listens_map.keys().cloned());

    // Helper to check if a file is frontend code (TypeScript/JavaScript)
    let is_frontend_file = |path: &str| {
        snapshot
            .files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.language == "typescript" || f.language == "javascript")
            .unwrap_or(false)
    };

    for event_name in all_events {
        let emits = event_emits_map
            .get(&event_name)
            .cloned()
            .unwrap_or_default();
        let listens = event_listens_map
            .get(&event_name)
            .cloned()
            .unwrap_or_default();

        // Detect FE↔FE sync pattern:
        // 1. Has both emits and listens
        // 2. All emits are from frontend files
        // 3. All listens are from frontend files
        // 4. No Rust involvement (Rust files would have "rust" language)
        let has_emit = !emits.is_empty();
        let has_listen = !listens.is_empty();
        let all_emits_fe = emits.iter().all(|(path, _, _)| is_frontend_file(path));
        let all_listens_fe = listens.iter().all(|(path, _)| is_frontend_file(path));
        let is_fe_sync = has_emit && has_listen && all_emits_fe && all_listens_fe;

        // Check if emit and listen are in the same file (strongest indicator)
        let same_file_sync = if is_fe_sync {
            let emit_files: HashSet<&str> =
                emits.iter().map(|(path, _, _)| path.as_str()).collect();
            let listen_files: HashSet<&str> =
                listens.iter().map(|(path, _)| path.as_str()).collect();
            !emit_files.is_disjoint(&listen_files)
        } else {
            false
        };

        snapshot.event_bridges.push(EventBridge {
            name: event_name.clone(),
            emits,
            listens,
            is_fe_sync,
            same_file_sync,
        });
    }

    // Store resolver configuration from scan results for caching
    if scan_results.ts_resolver_config.is_some()
        || !scan_results.py_roots.is_empty()
        || !rust_crate_roots.is_empty()
    {
        snapshot.metadata.resolver_config = Some(ResolverConfig {
            ts_paths: scan_results
                .ts_resolver_config
                .as_ref()
                .map(|c| c.ts_paths.clone())
                .unwrap_or_default(),
            ts_base_url: scan_results
                .ts_resolver_config
                .as_ref()
                .and_then(|c| c.ts_base_url.clone()),
            py_roots: scan_results.py_roots.clone(),
            rust_crate_roots,
        });
    }

    // Layer 3 — semantic facts. Computed on every init so that the persisted
    // snapshot carries the same suppression context that `findings.json` will
    // observe later. Empty when neither shell nor make participate in the scan.
    snapshot.semantic_facts = Some(crate::semantic::compute_semantic_facts(
        &snapshot.files,
        &snapshot_root,
    ));

    // Finalize metadata
    let duration_ms = start_time.elapsed().as_millis() as u64;
    snapshot.finalize_metadata(duration_ms);

    // Quote the same indexed snapshot count that metadata and query surfaces use.
    let file_count = snapshot.canonical_file_count();
    if let Some(spinner) = &spinner {
        spinner.finish_success(&format!(
            "Scanned {} in {:.2}s",
            crate::progress::format_count(file_count, "file", "files"),
            start_time.elapsed().as_secs_f64()
        ));
    }

    // Finish build spinner
    if let Some(build_spinner) = &build_spinner {
        build_spinner.finish_clear();
    }

    // Save snapshot (skipped for ephemeral override reads: `persist == false`).
    if persist {
        snapshot.save(&snapshot_root)?;
    }

    // Print summary (unless quiet mode)
    if !quiet_summary {
        snapshot.print_summary(&snapshot_root);
    }

    // Auto mode: emit full artifact set into the artifact directory (global cache by default)
    if parsed.auto_outputs {
        let artifacts_spinner = crate::progress::Spinner::new("Generating artifacts...");
        match write_auto_artifacts(
            &snapshot_root,
            root_list,
            &scan_results,
            &parsed,
            Some(&snapshot.metadata),
            None,
        ) {
            Ok(paths) => {
                artifacts_spinner.finish_clear();
                if !paths.is_empty() {
                    eprintln!(
                        "Artifacts saved under {}:",
                        Snapshot::artifacts_dir(&snapshot_root).display()
                    );
                    for p in paths {
                        eprintln!("  - {}", p);
                    }
                }
            }
            Err(err) => {
                artifacts_spinner.finish_error("Failed to generate artifacts");
                eprintln!("[loctree][warn] failed to write auto artifacts: {}", err);
            }
        }
    }

    // Announce after the snapshot and optional artifacts have landed so the
    // operator sees the note at the end of the scan output, while the file
    // itself was already present during the walk.
    if gitignore_entry_added {
        eprintln!(
            "[loct] Added '.loctree/' to .gitignore — loct writes its own artifacts there (context-atlas, pointer files) and they are not repo content. Remove the line to track them, or set LOCT_NO_GITIGNORE=1 to disable this offer."
        );
    }

    Ok(snapshot)
}

/// Run the init command: scan the project and save snapshot
///
/// This is a convenience wrapper around `run_init_with_options` with default behavior
/// (prints summary). For internal scans that should be quiet, use `run_init_with_options` directly.
pub fn run_init(root_list: &[PathBuf], parsed: &ParsedArgs) -> io::Result<()> {
    run_init_with_options(root_list, parsed, false)
}

/// In auto mode, generate the full set of analysis artifacts in the artifact directory.
pub(crate) fn write_auto_artifacts(
    snapshot_root: &Path,
    roots: &[PathBuf],
    scan_results: &crate::analyzer::root_scan::ScanResults,
    parsed: &ParsedArgs,
    metadata_override: Option<&SnapshotMetadata>,
    dist: Option<crate::analyzer::dist::DistResult>,
) -> io::Result<Vec<String>> {
    use crate::analyzer::coverage::{
        CommandUsage, compute_command_gaps_with_confidence, compute_unregistered_handlers,
    };
    use crate::analyzer::cycles::find_cycles_with_lazy;
    use crate::analyzer::dead_parrots::find_dead_exports;
    use crate::analyzer::output::{
        GlobalContext, RootArtifacts, attach_dist_to_sections, process_root_context, write_report,
    };
    use crate::analyzer::pipelines::build_pipeline_summary;
    use crate::analyzer::sarif::{SarifInputs, generate_sarif_string};
    use crate::analyzer::scan::opt_globset;
    use serde_json::json;

    const DEFAULT_EXCLUDE_REPORT_PATTERNS: &[&str] =
        &["**/__tests__/**", "scripts/semgrep-fixtures/**"];
    const SCHEMA_NAME: &str = "loctree-json";
    const SCHEMA_VERSION: &str = SNAPSHOT_SCHEMA_VERSION;

    let mut created = Vec::new();

    let loctree_dir = Snapshot::artifacts_dir(snapshot_root);
    fs::create_dir_all(&loctree_dir)?;

    let report_path = loctree_dir.join("report.html");
    let analysis_json_path = loctree_dir.join("analysis.json");
    let sarif_path = loctree_dir.join("report.sarif");
    let circular_json_path = loctree_dir.join("circular.json");
    let races_json_path = loctree_dir.join("py_races.json");

    let focus_set = opt_globset(&parsed.focus_patterns);
    let mut exclude_patterns = parsed.exclude_report_patterns.clone();
    exclude_patterns.extend(
        DEFAULT_EXCLUDE_REPORT_PATTERNS
            .iter()
            .map(|p| p.to_string()),
    );
    let exclude_set = opt_globset(&exclude_patterns);

    let registered_impls: HashSet<String> = scan_results
        .global_analyses
        .iter()
        .flat_map(|a| a.tauri_registered_handlers.iter().cloned())
        .collect();

    let mut global_be_registered: CommandUsage = std::collections::HashMap::new();
    for (name, locs) in &scan_results.global_be_commands {
        for (path, line, impl_name) in locs {
            if registered_impls.is_empty() || registered_impls.contains(impl_name) {
                global_be_registered.entry(name.clone()).or_default().push((
                    path.clone(),
                    *line,
                    impl_name.clone(),
                ));
            }
        }
    }

    let (global_missing_handlers, global_unused_handlers) = compute_command_gaps_with_confidence(
        &scan_results.global_fe_commands,
        &global_be_registered,
        &focus_set,
        &exclude_set,
        &scan_results.global_analyses,
    );

    let global_unregistered_handlers = compute_unregistered_handlers(
        &scan_results.global_be_commands,
        &registered_impls,
        &focus_set,
        &exclude_set,
    );

    let pipeline_summary = build_pipeline_summary(
        &scan_results.global_analyses,
        &focus_set,
        &exclude_set,
        &scan_results.global_fe_commands,
        &scan_results.global_be_commands,
        &scan_results.global_fe_payloads,
        &scan_results.global_be_payloads,
    );

    let mut json_results = Vec::new();
    let mut report_sections = Vec::new();
    let analysis_args = ParsedArgs {
        graph: true,
        report_path: Some(report_path.clone()),
        output: OutputMode::Json,
        summary: true,
        summary_limit: parsed.summary_limit,
        analyze_limit: parsed.analyze_limit,
        top_dead_symbols: parsed.top_dead_symbols,
        skip_dead_symbols: parsed.skip_dead_symbols,
        focus_patterns: parsed.focus_patterns.clone(),
        exclude_report_patterns: exclude_patterns.clone(),
        max_graph_nodes: parsed.max_graph_nodes,
        max_graph_edges: parsed.max_graph_edges,
        ..ParsedArgs::default()
    };

    let git_root = metadata_override
        .and_then(|metadata| metadata.roots.first().map(PathBuf::from))
        .or_else(|| roots.first().cloned())
        .unwrap_or_else(|| snapshot_root.to_path_buf());
    let git_ctx =
        Snapshot::git_context_for_root_with_metadata_fallback(&git_root, metadata_override);

    for (idx, ctx) in scan_results.contexts.iter().cloned().enumerate() {
        let RootArtifacts {
            json_items,
            report_section,
        } = process_root_context(
            idx,
            ctx,
            &analysis_args,
            &GlobalContext {
                fe_commands: &scan_results.global_fe_commands,
                be_commands: &scan_results.global_be_commands,
                missing_handlers: &global_missing_handlers,
                unregistered_handlers: &global_unregistered_handlers,
                unused_handlers: &global_unused_handlers,
                pipeline_summary: &pipeline_summary,
                git: Some(&git_ctx),
                schema_name: SCHEMA_NAME,
                schema_version: SCHEMA_VERSION,
                analyses: &scan_results.global_analyses,
            },
        );
        json_results.extend(json_items);
        if let Some(section) = report_section {
            report_sections.push(section);
        }
    }

    if let Some(ref dist_result) = dist {
        attach_dist_to_sections(
            &mut report_sections,
            dist_result.clone(),
            Path::new(&dist_result.src_dir),
        );
    }

    // HTML render is deferred until *after* the Context Atlas is materialized
    // (see the single emission site further down). Rendering here would
    // double-emit the `[OK] Report → ...` progress line and pay SSR twice
    // for no benefit — `render_html_report` is pure over `&[ReportSection]`,
    // so the second pass produces identical bytes.

    let all_graph_edges: Vec<_> = scan_results
        .contexts
        .iter()
        .flat_map(|ctx| ctx.graph_edges.clone())
        .collect();
    let (cycles, lazy_cycles) = find_cycles_with_lazy(&all_graph_edges);
    write_atomic(
        &circular_json_path,
        serde_json::to_string_pretty(&json!({
            "circularImports": cycles,
            "lazyCircularImports": lazy_cycles
        }))
        .map_err(io::Error::other)?,
    )?;
    created.push(
        circular_json_path
            .strip_prefix(&loctree_dir)
            .unwrap_or(&circular_json_path)
            .display()
            .to_string(),
    );

    let race_items: Vec<_> = scan_results
        .global_analyses
        .iter()
        .flat_map(|a| {
            a.py_race_indicators.iter().map(move |ind| {
                json!({
                    "path": a.path,
                    "line": ind.line,
                    "type": ind.concurrency_type,
                    "pattern": ind.pattern,
                    "risk": ind.risk,
                    "message": ind.message,
                })
            })
        })
        .collect();
    write_atomic(
        &races_json_path,
        serde_json::to_string_pretty(&race_items).map_err(io::Error::other)?,
    )?;
    created.push(
        races_json_path
            .strip_prefix(&loctree_dir)
            .unwrap_or(&races_json_path)
            .display()
            .to_string(),
    );

    let mut languages: Vec<String> = scan_results
        .contexts
        .iter()
        .flat_map(|ctx| ctx.languages.iter().cloned())
        .collect();
    languages.sort();
    languages.dedup();
    let total_loc: usize = scan_results.global_analyses.iter().map(|a| a.loc).sum();
    let file_count = metadata_override
        .map(|metadata| metadata.file_count)
        .unwrap_or_else(|| scan_results.global_analyses.len());

    let entrypoint_drift = if let Some(meta) = metadata_override {
        meta.entrypoint_drift.clone()
    } else {
        let manifest_summary: Vec<ManifestSummary> = roots
            .iter()
            .map(|root| crate::analyzer::manifests::summarize_manifests(root))
            .collect();
        let entrypoints =
            crate::analyzer::entrypoints::find_entrypoints(&scan_results.global_analyses)
                .into_iter()
                .map(|(path, kinds)| EntrypointSummary { path, kinds })
                .collect::<Vec<_>>();
        compute_entrypoint_drift(&manifest_summary, &entrypoints)
    };

    let bundle = json!({
        "schema": { "name": SCHEMA_NAME, "version": SCHEMA_VERSION },
        "generatedAt": time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Iso8601::DEFAULT)
            .unwrap_or_else(|_| "unknown".to_string()),
        "git": {
            "repo": git_ctx.repo,
            "branch": git_ctx.branch,
            "commit": git_ctx.commit,
            "scanId": git_ctx.scan_id,
        },
        "stats": {
            "files": file_count,
            "loc": total_loc,
            "languages": languages,
        },
        "analysis": json_results,
        "pipelineSummary": pipeline_summary,
        "circularImports": cycles,
        "pyRaceIndicators": race_items,
        "entrypointDrift": entrypoint_drift,
    });
    write_atomic(
        &analysis_json_path,
        serde_json::to_string_pretty(&bundle).map_err(io::Error::other)?,
    )?;
    created.push(
        analysis_json_path
            .strip_prefix(&loctree_dir)
            .unwrap_or(&analysis_json_path)
            .display()
            .to_string(),
    );

    // Generate SARIF report for CI integration
    let all_ranked_dups: Vec<_> = scan_results
        .contexts
        .iter()
        .flat_map(|ctx| ctx.filtered_ranked.clone())
        .collect();
    let high_confidence = parsed.dead_confidence.as_deref() == Some("high");
    let mut dead_ok_globs = crate::fs_utils::load_loctignore_dead_ok_globs(snapshot_root);
    dead_ok_globs.sort();
    dead_ok_globs.dedup();
    let dead_exports = find_dead_exports(
        &scan_results.global_analyses,
        high_confidence,
        None,
        crate::analyzer::dead_parrots::DeadFilterConfig {
            include_tests: false,
            include_helpers: false,
            library_mode: parsed.library_mode,
            example_globs: parsed.library_example_globs.clone(),
            python_library_mode: parsed.python_library,
            include_ambient: false,
            include_dynamic: false,
            dead_ok_globs,
        },
    );

    // Build minimal snapshot for SARIF enrichment and findings analysis
    let minimal_snapshot = Snapshot {
        metadata: metadata_override
            .cloned()
            .unwrap_or_else(|| SnapshotMetadata {
                roots: vec![snapshot_root.to_string_lossy().to_string()],
                languages: languages.iter().cloned().collect(),
                file_count,
                total_loc,
                entrypoint_drift: entrypoint_drift.clone(),
                git_repo: git_ctx.repo.clone(),
                git_owner_repo: git_ctx.owner_repo.clone(),
                git_branch: git_ctx.branch.clone(),
                git_commit: git_ctx.commit.clone(),
                git_scan_id: git_ctx.scan_id.clone(),
                ..Default::default()
            }),
        files: scan_results.global_analyses.clone(),
        edges: all_graph_edges
            .iter()
            .map(|(from, to, label)| GraphEdge {
                from: from.clone(),
                to: to.clone(),
                label: label.clone(),
            })
            .collect(),
        export_index: Default::default(),
        command_bridges: vec![],
        event_bridges: vec![],
        barrels: vec![],
        semantic_facts: Some(crate::semantic::compute_semantic_facts(
            &scan_results.global_analyses,
            snapshot_root,
        )),
        symbol_graph: None,
    };

    let sarif_content = generate_sarif_string(SarifInputs {
        duplicate_exports: &all_ranked_dups,
        missing_handlers: &global_missing_handlers,
        unused_handlers: &global_unused_handlers,
        dead_exports: &dead_exports,
        circular_imports: &cycles,
        pipeline_summary: &pipeline_summary,
        snapshot: Some(&minimal_snapshot),
    })
    .map_err(|err| io::Error::other(format!("Failed to serialize SARIF: {err}")))?;
    write_atomic(&sarif_path, sarif_content)?;
    created.push(
        sarif_path
            .strip_prefix(&loctree_dir)
            .unwrap_or(&sarif_path)
            .display()
            .to_string(),
    );

    // Save dead exports to standalone JSON for easy access
    let dead_json_path = loctree_dir.join("dead.json");
    let dead_json = json!({
        "deadExports": dead_exports.iter().map(|d| {
            json!({
                "file": d.file,
                "symbol": d.symbol,
                "line": d.line,
                "confidence": format!("{:?}", d.confidence),
                "reason": d.reason,
            })
        }).collect::<Vec<_>>(),
        "count": dead_exports.len(),
    });
    write_atomic(
        &dead_json_path,
        serde_json::to_string_pretty(&dead_json).map_err(io::Error::other)?,
    )?;
    created.push(
        dead_json_path
            .strip_prefix(&loctree_dir)
            .unwrap_or(&dead_json_path)
            .display()
            .to_string(),
    );

    // Save command handlers coverage to standalone JSON
    let handlers_json_path = loctree_dir.join("handlers.json");
    let handlers_json = json!({
        "missingHandlers": global_missing_handlers.iter().map(|gap| {
            json!({
                "command": gap.name,
                "locations": gap.locations.iter().map(|(path, line)| {
                    json!({ "path": path, "line": line })
                }).collect::<Vec<_>>(),
                "why": format!("Frontend calls invoke('{}') but no #[tauri::command] handler found", gap.name),
                "impact": "Runtime error: 'command {} not found' when invoked from frontend",
                "suggestedFix": "Create handler with #[tauri::command] and register in invoke_handler![]",
            })
        }).collect::<Vec<_>>(),
        "unusedHandlers": global_unused_handlers.iter().map(|gap| {
            json!({
                "command": gap.name,
                "implementationName": gap.implementation_name,
                "locations": gap.locations.iter().map(|(path, line)| {
                    json!({ "path": path, "line": line })
                }).collect::<Vec<_>>(),
                "confidence": gap.confidence.as_ref().map(|c| format!("{:?}", c)),
                "why": format!("Handler '{}' is registered but no invoke() calls found in frontend", gap.name),
                "impact": "Dead code - handler exists but is never called",
                "suggestedFix": "If intentionally unused (e.g., for tests), ignore. Otherwise, remove handler.",
            })
        }).collect::<Vec<_>>(),
        "unregisteredHandlers": global_unregistered_handlers.iter().map(|gap| {
            json!({
                "handler": gap.name,
                "implementationName": gap.implementation_name,
                "locations": gap.locations.iter().map(|(path, line)| {
                    json!({ "path": path, "line": line })
                }).collect::<Vec<_>>(),
                "why": format!("#[tauri::command] fn {}() found but NOT in invoke_handler![] macro", gap.name),
                "impact": "Command exists but is unreachable from frontend - invoke() calls will fail",
                "suggestedFix": "Add to invoke_handler![] in main.rs or lib.rs, or remove if unused",
            })
        }).collect::<Vec<_>>(),
        "summary": {
            "missing": global_missing_handlers.len(),
            "unused": global_unused_handlers.len(),
            "unregistered": global_unregistered_handlers.len(),
        },
    });
    write_atomic(
        &handlers_json_path,
        serde_json::to_string_pretty(&handlers_json).map_err(io::Error::other)?,
    )?;
    created.push(
        handlers_json_path
            .strip_prefix(&loctree_dir)
            .unwrap_or(&handlers_json_path)
            .display()
            .to_string(),
    );

    // Save findings.json - consolidated issue report
    let findings_json_path = loctree_dir.join("findings.json");
    let findings_config = crate::analyzer::findings::FindingsConfig {
        high_confidence,
        library_mode: parsed.library_mode,
        python_library: parsed.python_library,
        example_globs: parsed.library_example_globs.clone(),
    };
    let findings = crate::analyzer::findings::Findings::produce(
        scan_results,
        &minimal_snapshot,
        findings_config,
        dist.clone(),
    );
    let findings_json = findings.to_json().map_err(io::Error::other)?;
    write_atomic(&findings_json_path, &findings_json)?;
    created.push(
        findings_json_path
            .strip_prefix(&loctree_dir)
            .unwrap_or(&findings_json_path)
            .display()
            .to_string(),
    );

    // Save agent.json - AI-optimized bundle (used by CI and agent tooling)
    let agent_json_path = loctree_dir.join("agent.json");
    let agent_report = crate::analyzer::for_ai::generate_for_ai_report(
        &snapshot_root.to_string_lossy(),
        &report_sections,
        &scan_results.global_analyses,
        Some(&minimal_snapshot),
    );
    let agent_json = serde_json::to_vec_pretty(&agent_report).map_err(io::Error::other)?;
    write_atomic(&agent_json_path, &agent_json)?;
    created.push(
        agent_json_path
            .strip_prefix(&loctree_dir)
            .unwrap_or(&agent_json_path)
            .display()
            .to_string(),
    );

    // Save context-atlas/* - named Markdown cards for AI agents. This is the
    // canonical materialized context surface; MCP/CLI can point at it instead
    // of returning a single oversized context payload.
    let atlas_opts = crate::cli::command::ContextOptions {
        file: None,
        changed: false,
        task: None,
        scopes: Vec::new(),
        with_aicx: true,
        no_aicx: false,
        project: Some(snapshot_root.to_path_buf()),
        aicx_project_override: None,
        json: true,
        full: true,
        markdown: false,
    };
    // Materialize the Context Atlas first (report.html below points at it).
    // We capture the manifest summary here and announce it AFTER write_report
    // so the stdout sequence reads: [OK] Report → ... then [OK] Context Atlas → ...
    // then the "Artifacts saved under ..." block. Single source of truth for
    // the surface announcement; HTML pointer + on-disk cards stay in lockstep.
    let atlas_announce: Option<(String, usize)> =
        match crate::cli::dispatch::compose_context_pack_from_snapshot(
            &atlas_opts,
            snapshot_root,
            &minimal_snapshot,
        )
        .and_then(|pack| {
            crate::cli::dispatch::materialize_context_atlas(&pack, snapshot_root, None)
                .map_err(anyhow::Error::from)
        }) {
            Ok(atlas_manifest) => {
                created.push(
                    PathBuf::from(&atlas_manifest.manifest)
                        .strip_prefix(&loctree_dir)
                        .unwrap_or_else(|_| Path::new("context-atlas/manifest.md"))
                        .display()
                        .to_string(),
                );
                created.push(
                    PathBuf::from(&atlas_manifest.manifest_json)
                        .strip_prefix(&loctree_dir)
                        .unwrap_or_else(|_| Path::new("context-atlas/manifest.json"))
                        .display()
                        .to_string(),
                );
                // Compute cwd-relative display path the same way write_report does
                // (analyzer/output.rs:1699). Absolute fallback when manifest sits
                // outside cwd (e.g. global artifact cache).
                let manifest_path = PathBuf::from(&atlas_manifest.manifest);
                let display_path = std::env::current_dir()
                    .ok()
                    .and_then(|cwd| manifest_path.strip_prefix(&cwd).ok().map(PathBuf::from))
                    .map(|p| format!("./{}", p.display()))
                    .unwrap_or_else(|| manifest_path.display().to_string());
                Some((display_path, atlas_manifest.cards.len()))
            }
            Err(e) => {
                eprintln!("[loctree][warn] failed to write Context Atlas: {e}");
                None
            }
        };

    // Render report.html AFTER the Context Atlas is materialized so the
    // HTML surface can point at on-disk atlas state. Single emission —
    // the pre-atlas pass was removed (it would have produced identical
    // bytes anyway) so we never double-pay SSR or double-print progress.
    write_report(&report_path, &report_sections, parsed.verbose)?;
    created.push(
        report_path
            .strip_prefix(&loctree_dir)
            .unwrap_or(&report_path)
            .display()
            .to_string(),
    );

    // Announce the Context Atlas right after write_report so operators see
    // context-state in the same OK-pill rhythm as report.html. Mirrors the
    // verbose/non-verbose split from analyzer/output.rs:1704-1708.
    if let Some((display_path, card_count)) = atlas_announce {
        if parsed.verbose {
            eprintln!(
                "[loctree][debug] wrote Context Atlas to {} ({} cards)",
                display_path, card_count
            );
        } else {
            crate::progress::success(&format!(
                "Context Atlas → {} ({} cards)",
                display_path, card_count
            ));
        }
    }

    // Save manifest.json - index of artifacts for AI agents
    let manifest_json_path = loctree_dir.join("manifest.json");
    let findings_size_kb = findings_json.len() / 1024;
    let agent_size_kb = agent_json.len() / 1024;
    let manifest = crate::analyzer::findings::Manifest::produce(
        &minimal_snapshot,
        findings_size_kb,
        agent_size_kb,
        dist.as_ref(),
    );
    let manifest_json = manifest.to_json().map_err(io::Error::other)?;
    write_atomic(&manifest_json_path, &manifest_json)?;
    created.push(
        manifest_json_path
            .strip_prefix(&loctree_dir)
            .unwrap_or(&manifest_json_path)
            .display()
            .to_string(),
    );

    // Now that the full artifact set exists, refresh stable pointers (base_dir/*.json + base_dir/latest/).
    // Snapshot::save() runs this before auto artifacts are generated, so we do it again here.
    if let Err(e) = Snapshot::refresh_latest_artifacts(snapshot_root) {
        eprintln!("[loctree][warn] failed to refresh latest pointers: {}", e);
    }

    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::ParsedArgs;
    use crate::slicer::{HolographicSlice, SliceConfig};
    use crate::types::ExportSymbol;
    use serial_test::serial;
    use std::process::Command;
    use tempfile::TempDir;

    struct DirGuard {
        path: PathBuf,
    }

    impl DirGuard {
        fn new(path: PathBuf) -> Self {
            Self { path }
        }
    }

    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    struct CurrentDirGuard {
        original: PathBuf,
    }

    impl CurrentDirGuard {
        fn set(path: &Path) -> Self {
            let original = std::env::current_dir().expect("capture current dir");
            std::env::set_current_dir(path).expect("set current dir");
            Self { original }
        }
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.original).expect("restore current dir");
        }
    }

    fn run_git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_git_fixture(root: &Path, branch: &str, remote: &str, file_name: &str) {
        run_git(root, &["init"]);
        run_git(root, &["checkout", "-b", branch]);
        run_git(root, &["config", "user.email", "loctree-test@example.com"]);
        run_git(root, &["config", "user.name", "Loctree Test"]);
        std::fs::write(root.join(file_name), format!("{file_name}\n")).expect("fixture file");
        run_git(root, &["add", file_name]);
        run_git(root, &["commit", "-m", "fixture"]);
        run_git(root, &["remote", "add", "origin", remote]);
    }

    #[test]
    fn loctree_artifact_paths_are_invisible_to_freshness_dirt() {
        assert!(is_loctree_artifact_path(".loctree/"));
        assert!(is_loctree_artifact_path(
            ".loctree/context-atlas/manifest.json"
        ));
        assert!(is_loctree_artifact_path("./.loctree/report.html"));
        assert!(is_loctree_artifact_path("src-tauri/.loctree/watch.log"));
        assert!(!is_loctree_artifact_path(".loctignore"));
        assert!(!is_loctree_artifact_path(".loctree.json"));
        assert!(!is_loctree_artifact_path("src/loctree.rs"));
    }

    #[test]
    fn parse_git_status_paths_filters_loctree_artifacts() {
        let status =
            "?? .loctree/\n?? .loctree/context-atlas/manifest.json\n M src/lib.rs\n?? notes.md\n";
        assert_eq!(
            parse_git_status_paths(status),
            vec!["notes.md".to_string(), "src/lib.rs".to_string()]
        );
    }

    #[test]
    fn parse_git_status_paths_only_loctree_dirt_means_clean() {
        // The exact post-first-scan state on a repo WITHOUT `.loctree/` in
        // .gitignore: loct's own artifacts are the only untracked entries.
        // The guardian must treat this tree as clean or every analytic verb
        // DRIFT-rescans forever on loct's own output.
        let status = "?? .loctree/\n";
        assert!(parse_git_status_paths(status).is_empty());
    }

    #[test]
    fn gitignore_entry_appended_once_preserving_existing_content() {
        let tmp = TempDir::new().expect("temp dir");
        let root = tmp.path();
        run_git(root, &["init"]);
        std::fs::create_dir_all(root.join(".loctree")).expect("mkdir .loctree");
        // Existing content WITHOUT a trailing newline — append must not glue lines.
        std::fs::write(root.join(".gitignore"), "target/").expect("seed gitignore");

        assert!(ensure_loctree_gitignore_entry(root).expect("first append"));
        let body = std::fs::read_to_string(root.join(".gitignore")).expect("read gitignore");
        assert!(
            body.starts_with("target/\n"),
            "existing content must be preserved on its own line: {body:?}"
        );
        assert_eq!(
            body.matches(".loctree/").count(),
            1,
            "single entry: {body:?}"
        );

        // Entry now ignored via git semantics → second call is a no-op.
        assert!(!ensure_loctree_gitignore_entry(root).expect("second call"));
        let body_after = std::fs::read_to_string(root.join(".gitignore")).expect("read gitignore");
        assert_eq!(body, body_after, "no duplicate entry on repeated calls");
    }

    #[test]
    fn gitignore_entry_created_when_file_missing() {
        // No `.gitignore` and no `./.loctree/` yet — the plain `loct scan`
        // state. The entry is appended proactively for the artifacts a later
        // `loct context` / auto run will write.
        let tmp = TempDir::new().expect("temp dir");
        let root = tmp.path();
        run_git(root, &["init"]);

        assert!(ensure_loctree_gitignore_entry(root).expect("append"));
        let body = std::fs::read_to_string(root.join(".gitignore")).expect("gitignore created");
        assert!(
            body.lines().any(|line| line.trim() == ".loctree/"),
            "entry present: {body:?}"
        );
    }

    #[test]
    fn gitignore_entry_hands_off_without_git() {
        // No `.git`: never create or touch .gitignore.
        let no_git = TempDir::new().expect("temp dir");
        std::fs::create_dir_all(no_git.path().join(".loctree")).expect("mkdir .loctree");
        assert!(!ensure_loctree_gitignore_entry(no_git.path()).expect("no git"));
        assert!(!no_git.path().join(".gitignore").exists());
    }

    #[test]
    #[serial]
    fn test_snapshot_save_load_roundtrip() {
        let tmp = TempDir::new().expect("failed to create temp dir for snapshot roundtrip test");
        let root = tmp.path();

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
        snapshot.metadata.languages.insert("rust".to_string());
        snapshot.metadata.languages.insert("typescript".to_string());

        // Save
        snapshot
            .save(root)
            .expect("failed to save snapshot in roundtrip test");

        // Verify file exists
        assert!(Snapshot::exists(root));

        // Load
        let loaded = Snapshot::load(root).expect("failed to load snapshot in roundtrip test");

        assert_eq!(loaded.metadata.schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert!(loaded.metadata.languages.contains("rust"));
        assert!(loaded.metadata.languages.contains("typescript"));
    }

    #[test]
    #[serial]
    fn indexes_extensionless_python_shebang_entrypoint_for_slice_and_counts() {
        let tmp = TempDir::new().expect("tmp dir");
        let root = tmp.path();
        let runner = root.join("run-tool");
        std::fs::write(
            &runner,
            "#!/usr/bin/env python3\n\ndef main():\n    return 42\n",
        )
        .expect("write python entrypoint");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&runner)
                .expect("runner metadata")
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&runner, perms).expect("chmod runner");
        }

        let parsed = ParsedArgs {
            full_scan: true,
            ..ParsedArgs::default()
        };
        run_init_with_options_for_strategy(
            &[root.to_path_buf()],
            &parsed,
            true,
            SnapshotRootStrategy::Exact,
        )
        .expect("scan temp project");

        let snapshot = Snapshot::load(root).expect("load snapshot");
        let entry = snapshot
            .files
            .iter()
            .find(|file| file.path == "run-tool")
            .expect("extensionless python entrypoint should be indexed");

        assert_eq!(entry.language, "py");
        assert_eq!(entry.loc, 4);
        assert!(entry.exports.iter().any(|export| export.name == "main"));
        assert_eq!(snapshot.metadata.file_count, 1);
        assert_eq!(snapshot.metadata.total_loc, 4);
        assert!(snapshot.metadata.languages.contains("py"));

        let slice = HolographicSlice::from_path(&snapshot, "run-tool", &SliceConfig::default())
            .expect("slice should resolve extensionless shebang file by path");
        assert_eq!(slice.target, "run-tool");
        assert_eq!(slice.stats.core_files, 1);
        assert_eq!(slice.stats.core_loc, 4);
    }

    #[test]
    #[serial]
    fn test_reuse_fence_matches_until_indexed_content_changes() {
        let tmp = TempDir::new().expect("failed to create temp dir for reuse fence test");
        let root = tmp.path();
        let source_path = root.join("src/lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent")).expect("mkdir");
        std::fs::write(&source_path, "pub fn alpha() {}\n").expect("write source");

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
        snapshot
            .files
            .push(crate::types::FileAnalysis::new("src/lib.rs".to_string()));
        snapshot.save(root).expect("save snapshot");

        assert!(
            snapshot.reuse_fence_matches(root).expect("fence check"),
            "freshly saved fence should match indexed file contents"
        );

        std::fs::write(&source_path, "pub fn beta() {}\n").expect("modify source");

        assert!(
            !snapshot.reuse_fence_matches(root).expect("fence check"),
            "content drift in an indexed file should break the fence"
        );
    }

    #[test]
    #[serial]
    fn strict_acquire_trusts_fresh_snapshot_of_preexisting_dirty_content() {
        let tmp = TempDir::new().expect("temp dir for dirty snapshot freshness test");
        let root = tmp.path();
        let source_path = root.join("src/lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent")).expect("mkdir");
        std::fs::write(&source_path, "pub fn alpha() {}\n").expect("write source");
        init_git_repo(root);

        std::fs::write(&source_path, "pub fn beta() {}\n").expect("dirty tracked source");
        assert_eq!(is_git_dirty(root), Some(true));

        let parsed = ParsedArgs {
            full_scan: true,
            ..ParsedArgs::default()
        };
        run_init_with_options_for_strategy(
            &[root.to_path_buf()],
            &parsed,
            true,
            SnapshotRootStrategy::Exact,
        )
        .expect("scan dirty project state");

        let snapshot = Snapshot::load(root).expect("load freshly saved snapshot");
        assert!(
            snapshot.reuse_fence_matches(root).expect("fence check"),
            "fresh scan must fence the dirty content it just indexed"
        );
        assert!(
            !snapshot.is_stale(root),
            "freshly saved snapshot must not report stale before any later edit"
        );

        let acquired = acquire_snapshot(
            &[root.to_path_buf()],
            SnapshotReusePolicy::Strict,
            &AcquireOptions {
                no_scan: true,
                fail_stale: true,
                quiet: true,
                strategy: SnapshotRootStrategy::Exact,
                ..Default::default()
            },
        )
        .expect("strict no-scan acquire must reuse the fresh dirty snapshot");

        assert_eq!(acquired.fingerprint(), snapshot.fingerprint());
    }

    fn init_git_repo(root: &Path) {
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .expect("run git");
            assert!(out.status.success(), "git {:?} failed: {:?}", args, out);
        };
        git(&["init"]);
        git(&["config", "user.email", "agents@vetcoders.io"]);
        git(&["config", "user.name", "guardian-test"]);
        git(&["add", "."]);
        git(&["commit", "-m", "init"]);
    }

    #[test]
    #[serial]
    fn test_watch_fast_path_trusts_snapshot_without_rehash() {
        let tmp = TempDir::new().expect("temp dir for watch fast-path test");
        let root = tmp.path();
        let source_path = root.join("src/lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent")).expect("mkdir");
        std::fs::write(&source_path, "pub fn alpha() {}\n").expect("write source");
        init_git_repo(root);

        // Snapshot pinned to the current git scan_id (what `loct watch` maintains).
        let canonical_root = root.canonicalize().expect("canonical root");
        let ctx = Snapshot::git_context_for(&canonical_root);
        let mut snapshot = Snapshot::new(vec![canonical_root.display().to_string()]);
        snapshot
            .files
            .push(crate::types::FileAnalysis::new("src/lib.rs".to_string()));
        snapshot.metadata.git_scan_id = ctx.scan_id.clone();
        snapshot.metadata.git_commit = ctx.commit.clone();
        snapshot.metadata.git_branch = ctx.branch.clone();
        snapshot.save(&canonical_root).expect("save snapshot");
        let saved_generated_at = snapshot.metadata.generated_at.clone();

        // Dirty the worktree so the content fence would NOT match: without a
        // live watch, Strict policy must rescan; with one, it must trust.
        std::fs::write(&source_path, "pub fn beta() {}\n").expect("modify source");
        assert!(
            !snapshot
                .reuse_fence_matches(&canonical_root)
                .expect("fence check"),
            "precondition: content fence must be broken"
        );

        // No watcher yet — sanity check on the helper.
        assert!(!watch_keeps_snapshot_fresh(&snapshot, &canonical_root));

        // Hold the watch lock as a live watcher would.
        let guard =
            crate::watch_lock::acquire(&canonical_root, crate::watch_lock::LockMode::Default)
                .expect("acquire watch lock");
        assert!(
            watch_keeps_snapshot_fresh(&snapshot, &canonical_root),
            "live lock + matching scan_id must mark the snapshot watch-fresh"
        );

        let acquired = acquire_snapshot(
            std::slice::from_ref(&canonical_root),
            SnapshotReusePolicy::Strict,
            &AcquireOptions {
                quiet: true,
                ..Default::default()
            },
        )
        .expect("guardian must serve the watch-maintained snapshot");
        assert_eq!(
            acquired.metadata.generated_at, saved_generated_at,
            "guardian must trust the cached snapshot (no rescan / no re-hash) while watch is alive"
        );

        // scan_id mismatch must disable the fast path even while locked.
        let mut drifted = acquired.clone();
        drifted.metadata.git_scan_id = Some("other-branch@deadbeef".to_string());
        assert!(
            !watch_keeps_snapshot_fresh(&drifted, &canonical_root),
            "stale scan_id must not be trusted even with a live watch"
        );

        drop(guard);
        assert!(
            !watch_keeps_snapshot_fresh(&snapshot, &canonical_root),
            "released lock must disable the fast path"
        );
    }

    #[test]
    #[serial]
    fn test_reuse_fence_rejects_indexed_file_path_traversal() {
        let tmp = TempDir::new().expect("failed to create temp dir for traversal test");
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(&root).expect("mkdir repo");

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
        snapshot
            .files
            .push(crate::types::FileAnalysis::new("../secret.rs".to_string()));

        let err = snapshot
            .compute_reuse_fence(&root)
            .expect_err("path traversal should be rejected before reading");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    #[serial]
    fn test_reuse_fence_rejects_cache_sidecar_path_traversal() {
        let tmp = TempDir::new().expect("failed to create temp dir for cache traversal test");
        let root = tmp.path();

        let mut snapshot = Snapshot::new(vec![root.display().to_string()]);
        snapshot.metadata.git_scan_id = Some("../outside".to_string());

        let err = snapshot
            .reuse_fence_matches(root)
            .expect_err("cache sidecar traversal should be rejected before reading");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    #[serial]
    fn test_snapshot_not_found() {
        let tmp = TempDir::new().expect("failed to create temp dir for not_found test");
        let result = Snapshot::load(tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_snapshot_new_creates_valid_metadata() {
        let snapshot = Snapshot::new(vec!["src".to_string()]);
        assert_eq!(snapshot.metadata.schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snapshot.metadata.roots, vec!["src".to_string()]);
        assert!(snapshot.metadata.languages.is_empty());
        assert_eq!(snapshot.metadata.file_count, 0);
        assert!(!snapshot.metadata.generated_at.is_empty());
    }

    #[test]
    #[serial]
    fn test_snapshot_path() {
        let path = Snapshot::snapshot_path(Path::new("/some/project"));
        // Snapshot path should be under the global cache, not project-local
        assert!(path.ends_with("snapshot.json"));
        // Should NOT be under the project directory anymore
        assert!(
            !path.starts_with("/some/project/.loctree"),
            "snapshot should go to global cache, not project-local .loctree"
        );
    }

    #[test]
    fn test_snapshot_path_uses_root_git_context() {
        // Non-git directory should use legacy path (no scan_id)
        let path = Snapshot::snapshot_path(Path::new("/tmp/loctree"));
        assert!(
            path.ends_with("snapshot.json"),
            "should end with snapshot.json"
        );
        // Git directory (cwd) should include scan_id
        let cwd = std::env::current_dir().unwrap();
        let ctx = Snapshot::git_context_for(&cwd);
        if let Some(scan) = ctx.scan_id {
            let path = Snapshot::snapshot_path(&cwd);
            assert!(
                path.display().to_string().contains(&scan),
                "git dir should include scan_id"
            );
        }
    }

    #[test]
    #[serial]
    fn test_artifacts_dir_uses_root_git_context() {
        // Non-git directory should use global cache
        let dir = Snapshot::artifacts_dir(Path::new("/tmp/loctree"));
        assert!(
            !dir.starts_with("/tmp/loctree/.loctree"),
            "artifacts should go to global cache, not project-local"
        );
        // Git directory should include scan_id
        let cwd = std::env::current_dir().unwrap();
        let ctx = Snapshot::git_context_for(&cwd);
        if let Some(scan) = ctx.scan_id {
            let dir = Snapshot::artifacts_dir(&cwd);
            assert!(
                dir.display().to_string().contains(&scan),
                "git dir should include scan_id"
            );
        }
    }

    #[test]
    #[serial]
    fn test_snapshot_exists_false() {
        let tmp = TempDir::new().expect("create temp dir");
        assert!(!Snapshot::exists(tmp.path()));
    }

    #[test]
    #[serial]
    fn test_snapshot_exists_true() {
        let tmp = TempDir::new().expect("create temp dir");
        let snapshot = Snapshot::new(vec!["src".to_string()]);
        snapshot.save(tmp.path()).expect("save");
        assert!(Snapshot::exists(tmp.path()));
    }

    #[test]
    #[serial]
    fn test_find_loctree_root_none() {
        let tmp = TempDir::new().expect("create temp dir");
        // Create a subdirectory without .loctree
        let subdir = tmp.path().join("sub");
        std::fs::create_dir(&subdir).expect("create subdir");
        assert!(Snapshot::find_loctree_root(&subdir).is_none());
    }

    #[test]
    #[serial]
    fn test_find_loctree_root_found() {
        let tmp = TempDir::new().expect("create temp dir");
        // Create .loctree directory at root
        std::fs::create_dir(tmp.path().join(SNAPSHOT_DIR)).expect("create .loctree");
        // Create a nested subdirectory
        let subdir = tmp.path().join("a/b/c");
        std::fs::create_dir_all(&subdir).expect("create nested subdir");
        let found = Snapshot::find_loctree_root(&subdir);
        assert!(found.is_some());
        let found = found.unwrap();
        assert!(found.join(SNAPSHOT_DIR).exists());
    }

    #[test]
    #[serial]
    fn test_snapshot_with_files() {
        let tmp = TempDir::new().expect("create temp dir");
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);

        // Add file analysis
        let file = FileAnalysis::new("src/main.ts".into());
        snapshot.files.push(file);
        snapshot.metadata.file_count = 1;

        snapshot.save(tmp.path()).expect("save");
        let loaded = Snapshot::load(tmp.path()).expect("load");

        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files[0].path, "src/main.ts");
        assert_eq!(loaded.metadata.file_count, 1);
    }

    #[test]
    #[serial]
    fn test_snapshot_with_edges() {
        let tmp = TempDir::new().expect("create temp dir");
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);

        snapshot.edges.push(GraphEdge {
            from: "a.ts".to_string(),
            to: "b.ts".to_string(),
            label: "foo".to_string(),
        });

        snapshot.save(tmp.path()).expect("save");
        let loaded = Snapshot::load(tmp.path()).expect("load");

        assert_eq!(loaded.edges.len(), 1);
        assert_eq!(loaded.edges[0].from, "a.ts");
        assert_eq!(loaded.edges[0].to, "b.ts");
    }

    #[test]
    fn snapshot_fingerprint_is_stable_for_reordered_graph_facts() {
        let mut first = Snapshot::new(vec!["src".to_string(), "crates".to_string()]);
        first.metadata.languages = HashSet::from(["rust".to_string(), "typescript".to_string()]);
        first.metadata.file_count = 2;
        first.metadata.total_loc = 42;
        first.files.push(FileAnalysis {
            path: "src/b.rs".to_string(),
            language: "rust".to_string(),
            loc: 20,
            size: 200,
            exports: vec![ExportSymbol::new(
                "beta".to_string(),
                "function",
                "named",
                Some(3),
            )],
            ..FileAnalysis::default()
        });
        first.files.push(FileAnalysis {
            path: "src/a.rs".to_string(),
            language: "rust".to_string(),
            loc: 22,
            size: 220,
            exports: vec![ExportSymbol::new(
                "alpha".to_string(),
                "function",
                "named",
                Some(1),
            )],
            ..FileAnalysis::default()
        });
        first.edges.push(GraphEdge {
            from: "src/a.rs".to_string(),
            to: "src/b.rs".to_string(),
            label: "import".to_string(),
        });

        let mut second = first.clone();
        second.metadata.roots.reverse();
        second.files.reverse();
        second.edges.reverse();

        assert_eq!(first.fingerprint(), second.fingerprint());
        let report = first.fingerprint_report();
        assert_eq!(report.algorithm, "sha256:loctree-snapshot-authority-v1");
        assert_eq!(report.file_count, 2);
        assert_eq!(report.edge_count, 1);
    }

    #[test]
    fn snapshot_authority_report_exposes_fingerprint_git_and_staleness() {
        let tmp = TempDir::new().expect("create temp dir");
        let mut snapshot = Snapshot::new(vec![tmp.path().display().to_string()]);
        snapshot.metadata.git_repo = Some("loctree".to_string());
        snapshot.metadata.git_owner_repo = Some("Loctree/loctree-suite".to_string());
        snapshot.metadata.git_branch = Some("main".to_string());
        snapshot.metadata.git_commit = Some("abc1234".to_string());
        snapshot.metadata.git_scan_id = Some("main@abc1234".to_string());

        let authority = snapshot.authority_report(tmp.path());

        assert_eq!(authority.git.repo.as_deref(), Some("loctree"));
        assert_eq!(
            authority.git.owner_repo.as_deref(),
            Some("Loctree/loctree-suite")
        );
        assert_eq!(authority.staleness.dirty_worktree, None);
        assert!(!authority.staleness.stale);
        assert_eq!(authority.fingerprint.value, snapshot.fingerprint());
    }

    #[test]
    #[serial]
    fn receipt_git_from_roots() {
        let repo_a = TempDir::new().expect("repo a");
        let repo_b = TempDir::new().expect("repo b");
        init_git_fixture(
            repo_a.path(),
            "repo-a-branch",
            "git@github.com:Owner/repo-a.git",
            "a.txt",
        );
        init_git_fixture(
            repo_b.path(),
            "repo-b-branch",
            "git@github.com:Caller/repo-b.git",
            "b.txt",
        );

        let expected = Snapshot::git_context_for(repo_a.path());
        let leaked = Snapshot::git_context_for(repo_b.path());
        assert_ne!(expected.owner_repo, leaked.owner_repo);
        assert_ne!(expected.branch, leaked.branch);

        let mut snapshot = Snapshot::new(vec![repo_a.path().display().to_string()]);
        snapshot.metadata.git_repo = leaked.repo.clone();
        snapshot.metadata.git_owner_repo = leaked.owner_repo.clone();
        snapshot.metadata.git_branch = leaked.branch.clone();
        snapshot.metadata.git_commit = leaked.commit.clone();
        snapshot.metadata.git_scan_id = leaked.scan_id.clone();

        let _cwd = CurrentDirGuard::set(repo_b.path());
        let authority = snapshot.authority_report(repo_b.path());

        assert_eq!(authority.git.repo, expected.repo);
        assert_eq!(authority.git.owner_repo, expected.owner_repo);
        assert_eq!(authority.git.branch, expected.branch);
        assert_eq!(authority.git.commit, expected.commit);
        assert_eq!(authority.git.scan_id, expected.scan_id);
        assert_ne!(authority.git.owner_repo, leaked.owner_repo);
    }

    #[test]
    #[serial]
    fn test_snapshot_with_command_bridges() {
        let tmp = TempDir::new().expect("create temp dir");
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);

        snapshot.command_bridges.push(CommandBridge {
            name: "get_user".to_string(),
            frontend_calls: vec![("app.ts".to_string(), 10)],
            backend_handler: Some(("handlers.rs".to_string(), 20)),
            has_handler: true,
            is_called: true,
        });

        snapshot.save(tmp.path()).expect("save");
        let loaded = Snapshot::load(tmp.path()).expect("load");

        assert_eq!(loaded.command_bridges.len(), 1);
        assert_eq!(loaded.command_bridges[0].name, "get_user");
        assert!(loaded.command_bridges[0].has_handler);
    }

    #[test]
    #[serial]
    fn test_snapshot_with_event_bridges() {
        let tmp = TempDir::new().expect("create temp dir");
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);

        snapshot.event_bridges.push(EventBridge {
            name: "user_updated".to_string(),
            emits: vec![("events.ts".to_string(), 10, "emit".to_string())],
            listens: vec![("listener.ts".to_string(), 20)],
            is_fe_sync: false,
            same_file_sync: false,
        });

        snapshot.save(tmp.path()).expect("save");
        let loaded = Snapshot::load(tmp.path()).expect("load");

        assert_eq!(loaded.event_bridges.len(), 1);
        assert_eq!(loaded.event_bridges[0].name, "user_updated");
    }

    #[test]
    #[serial]
    fn test_snapshot_with_barrels() {
        let tmp = TempDir::new().expect("create temp dir");
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);

        snapshot.barrels.push(BarrelFile {
            path: "src/index.ts".to_string(),
            module_id: "src".to_string(),
            reexport_count: 5,
            targets: vec!["src/utils.ts".to_string()],
        });

        snapshot.save(tmp.path()).expect("save");
        let loaded = Snapshot::load(tmp.path()).expect("load");

        assert_eq!(loaded.barrels.len(), 1);
        assert_eq!(loaded.barrels[0].reexport_count, 5);
    }

    #[test]
    fn test_snapshot_metadata_serde() {
        let metadata = SnapshotMetadata {
            schema_version: SNAPSHOT_SCHEMA_VERSION.to_string(),
            generated_at: "2025-01-01T00:00:00Z".to_string(),
            roots: vec!["src".to_string()],
            languages: HashSet::from(["ts".to_string()]),
            file_count: 10,
            total_loc: 1000,
            scan_duration_ms: 500,
            resolver_config: Some(ResolverConfig {
                ts_paths: HashMap::from([("@/*".to_string(), vec!["src/*".to_string()])]),
                ts_base_url: Some("./src".to_string()),
                py_roots: vec![],
                rust_crate_roots: vec![],
            }),
            manifest_summary: Vec::new(),
            entrypoints: Vec::new(),
            entrypoint_drift: EntrypointDriftSummary::default(),
            git_repo: None,
            git_owner_repo: None,
            git_branch: None,
            git_commit: None,
            git_scan_id: None,
        };

        let json = serde_json::to_string(&metadata).expect("serialize");
        let deser: SnapshotMetadata = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deser.file_count, 10);
        assert!(deser.resolver_config.is_some());
    }

    #[test]
    fn test_graph_edge_serde() {
        let edge = GraphEdge {
            from: "a.ts".to_string(),
            to: "b.ts".to_string(),
            label: "import".to_string(),
        };

        let json = serde_json::to_string(&edge).expect("serialize");
        let deser: GraphEdge = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deser.from, "a.ts");
        assert_eq!(deser.label, "import");
    }

    #[test]
    fn test_resolver_config_default() {
        let config = ResolverConfig::default();
        assert!(config.ts_paths.is_empty());
        assert!(config.ts_base_url.is_none());
        assert!(config.py_roots.is_empty());
        assert!(config.rust_crate_roots.is_empty());
    }

    #[test]
    #[serial]
    fn test_snapshot_export_index() {
        let mut snapshot = Snapshot::new(vec!["src".to_string()]);

        snapshot
            .export_index
            .insert("Button".to_string(), vec!["src/Button.tsx".to_string()]);

        let json = serde_json::to_string_pretty(&snapshot).expect("serialize snapshot");
        let loaded: Snapshot = serde_json::from_str(&json).expect("deserialize snapshot");

        assert!(loaded.export_index.contains_key("Button"));
        assert_eq!(
            loaded.export_index.get("Button").unwrap(),
            &vec!["src/Button.tsx".to_string()]
        );
    }

    #[test]
    #[serial]
    fn test_find_latest_snapshot_explicit_path_exists() {
        let tmp = TempDir::new().expect("create temp dir");
        let snapshot_path = tmp.path().join(SNAPSHOT_DIR).join(SNAPSHOT_FILE);

        // Create snapshot directory and file
        std::fs::create_dir_all(snapshot_path.parent().unwrap()).expect("create dir");
        let snapshot = Snapshot::new(vec!["src".to_string()]);
        let json = serde_json::to_string_pretty(&snapshot).unwrap();
        std::fs::write(&snapshot_path, json).expect("write snapshot");

        // Should return the explicit path
        let result = Snapshot::find_latest_snapshot(Some(&snapshot_path));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), snapshot_path);
    }

    #[test]
    #[serial]
    fn test_find_latest_snapshot_explicit_path_not_exists() {
        let result =
            Snapshot::find_latest_snapshot(Some(Path::new("/nonexistent/path/snapshot.json")));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Snapshot not found"));
        assert!(err.contains("Run `loct scan` first"));
    }

    #[test]
    #[serial]
    fn test_find_latest_snapshot_picks_newest_by_mtime() {
        let tmp = TempDir::new().expect("create temp dir");
        let loctree_dir = tmp.path().join(SNAPSHOT_DIR);
        let _cleanup = DirGuard::new(project_cache_dir(tmp.path()));

        // Create two branch@sha subdirectories with snapshots
        let old_dir = loctree_dir.join("main@old123");
        let new_dir = loctree_dir.join("main@new456");
        std::fs::create_dir_all(&old_dir).expect("create old dir");
        std::fs::create_dir_all(&new_dir).expect("create new dir");

        let old_snapshot_path = old_dir.join(SNAPSHOT_FILE);
        let new_snapshot_path = new_dir.join(SNAPSHOT_FILE);

        // Write old snapshot first
        let snapshot = Snapshot::new(vec!["src".to_string()]);
        let json = serde_json::to_string_pretty(&snapshot).unwrap();
        std::fs::write(&old_snapshot_path, &json).expect("write old snapshot");

        // Wait a tiny bit to ensure mtime difference
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Write new snapshot
        std::fs::write(&new_snapshot_path, &json).expect("write new snapshot");

        // Use find_latest_snapshot_in to avoid changing global cwd (thread-safe)
        let result = Snapshot::find_latest_snapshot_in(tmp.path());

        assert!(result.is_ok());
        let found_path = result.unwrap();
        // Should find the newer snapshot
        assert!(
            found_path.to_string_lossy().contains("new456"),
            "Expected newest snapshot, got: {}",
            found_path.display()
        );
    }

    #[test]
    #[serial]
    fn test_find_latest_snapshot_no_loctree_dir() {
        let tmp = TempDir::new().expect("create temp dir");
        // No .loctree directory and no cache entry

        // Use find_latest_snapshot_in to avoid changing global cwd (thread-safe)
        let result = Snapshot::find_latest_snapshot_in(tmp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("No snapshot found"),
            "Expected 'No snapshot found' in error: {}",
            err
        );
    }

    #[test]
    #[serial]
    fn test_find_latest_snapshot_empty_loctree_dir() {
        let tmp = TempDir::new().expect("create temp dir");
        // Create empty .loctree directory (no snapshots)
        std::fs::create_dir(tmp.path().join(SNAPSHOT_DIR)).expect("create .loctree");

        // Use find_latest_snapshot_in to avoid changing global cwd (thread-safe)
        let result = Snapshot::find_latest_snapshot_in(tmp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("No snapshot found"),
            "Expected 'No snapshot found' in error: {}",
            err
        );
    }

    #[test]
    #[serial]
    fn test_find_latest_snapshot_legacy_path() {
        let tmp = TempDir::new().expect("create temp dir");
        let loctree_dir = tmp.path().join(SNAPSHOT_DIR);
        let _cleanup = DirGuard::new(project_cache_dir(tmp.path()));

        // Create legacy snapshot at .loctree/snapshot.json (not in subdirectory)
        std::fs::create_dir_all(&loctree_dir).expect("create .loctree dir");
        let legacy_path = loctree_dir.join(SNAPSHOT_FILE);

        let snapshot = Snapshot::new(vec!["src".to_string()]);
        let json = serde_json::to_string_pretty(&snapshot).unwrap();
        std::fs::write(&legacy_path, json).expect("write legacy snapshot");

        // Use find_latest_snapshot_in to avoid changing global cwd (thread-safe)
        let result = Snapshot::find_latest_snapshot_in(tmp.path());

        assert!(result.is_ok());
        // Legacy path should be migrated to cache and returned from there.
        let found = result.unwrap().canonicalize().unwrap_or_default();
        let expected = Snapshot::snapshot_path(tmp.path())
            .canonicalize()
            .unwrap_or_default();
        assert_eq!(found, expected);
        assert!(
            legacy_path.exists(),
            "legacy source remains for compatibility"
        );
    }

    #[test]
    #[serial]
    fn test_find_latest_snapshot_global_cache_from_subdir() {
        let project = TempDir::new().expect("create temp project dir");
        let nested = project.path().join("a/b/c");
        std::fs::create_dir_all(&nested).expect("create nested dirs");

        // Ensure we don't leave cache artifacts around after this test.
        let _cleanup = DirGuard::new(project_cache_dir(project.path()));

        // Save snapshot for project root -> goes to global cache (or temp dir fallback)
        let snapshot = Snapshot::new(vec!["src".to_string()]);
        snapshot.save(project.path()).expect("save snapshot");

        // Discover from nested subdir: should resolve effective root via global cache
        let found = Snapshot::find_latest_snapshot_in(&nested).expect("find snapshot");
        let expected = Snapshot::snapshot_path(project.path());

        // Compare canonicalized paths to handle /private/var vs /var on macOS
        let found = found.canonicalize().unwrap_or(found);
        let expected = expected.canonicalize().unwrap_or(expected);
        assert_eq!(found, expected);
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use serial_test::serial;
    use sha2::{Digest, Sha256};
    use std::ffi::OsString;
    use std::process::Command;
    use tempfile::TempDir;

    const CACHE_ENV: &str = "LOCT_CACHE_DIR";

    #[derive(Debug)]
    struct EnvVarGuard {
        key: &'static str,
        original: Option<OsString>,
    }

    #[derive(Debug)]
    struct CurrentDirGuard {
        original: PathBuf,
    }

    impl CurrentDirGuard {
        fn set(path: &Path) -> Self {
            let original = std::env::current_dir().expect("capture current dir");
            std::env::set_current_dir(path).expect("set current dir");
            Self { original }
        }
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.original).expect("restore current dir");
        }
    }

    impl EnvVarGuard {
        fn set_path(key: &'static str, value: &Path) -> Self {
            let guard = Self {
                key,
                original: std::env::var_os(key),
            };
            set_env_var(key, value.as_os_str());
            guard
        }

        fn clear(key: &'static str) -> Self {
            let guard = Self {
                key,
                original: std::env::var_os(key),
            };
            remove_env_var(key);
            guard
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => set_env_var(self.key, value),
                None => remove_env_var(self.key),
            }
        }
    }

    fn set_env_var<K: AsRef<std::ffi::OsStr>, V: AsRef<std::ffi::OsStr>>(key: K, value: V) {
        unsafe {
            std::env::set_var(key, value);
        }
    }

    fn remove_env_var<K: AsRef<std::ffi::OsStr>>(key: K) {
        unsafe {
            std::env::remove_var(key);
        }
    }

    fn expected_project_id(root: &Path) -> String {
        let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let mut hasher = Sha256::new();
        hasher.update(canonical.to_string_lossy().as_bytes());
        sha256_hex(hasher.finalize())
            .chars()
            .take(16)
            .collect::<String>()
    }

    fn display_artifact_path(artifact: &Path, loctree_dir: &Path) -> String {
        artifact
            .strip_prefix(loctree_dir)
            .unwrap_or(artifact)
            .display()
            .to_string()
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap_or_else(|e| panic!("failed to run git {:?}: {e}", args));
        assert!(
            output.status.success(),
            "git {:?} failed.\nstdout: {}\nstderr: {}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap_or_else(|e| panic!("failed to run git {:?}: {e}", args));
        assert!(
            output.status.success(),
            "git {:?} failed.\nstdout: {}\nstderr: {}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    #[serial]
    fn storage_full_cache_write_error_teaches_cache_cleanup() {
        let tmp = TempDir::new().expect("create temp dir");
        let custom = tmp.path().join("custom-cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, &custom);
        let target = custom.join("projects").join("abc123").join("snapshot.json");

        let err = cache_write_error(&target, io::Error::from_raw_os_error(28));
        let message = err.to_string();

        assert!(
            message.contains("loct cache list"),
            "ENOSPC hint should teach cache inspection: {message}"
        );
        assert!(
            message.contains("loct cache prune --max-size 1GB --force"),
            "ENOSPC hint should teach bounded cache GC: {message}"
        );
        assert!(
            message.contains("LOCT_CACHE_DIR"),
            "ENOSPC hint should teach alternate cache location: {message}"
        );
    }

    #[test]
    #[serial]
    fn cache_base_dir_uses_loct_cache_dir_override() {
        let tmp = TempDir::new().expect("create temp dir");
        let custom = tmp.path().join("custom-cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, &custom);

        let actual = cache_base_dir();
        assert_eq!(actual, custom);
        assert!(actual.is_absolute(), "cache base should be absolute");
    }

    #[test]
    #[serial]
    fn cache_base_dir_defaults_to_platform_cache_dir() {
        let _guard = EnvVarGuard::clear(CACHE_ENV);

        let actual = cache_base_dir();
        let expected = dirs::cache_dir()
            .map(|path| path.join("loctree"))
            .unwrap_or_else(|| PathBuf::from(SNAPSHOT_DIR));

        assert_eq!(actual, expected);
        if dirs::cache_dir().is_some() {
            assert!(
                actual.is_absolute(),
                "platform cache dir should be absolute"
            );
        }
    }

    #[test]
    #[serial]
    fn project_cache_dir_uses_expected_sha256_id() {
        let _guard = EnvVarGuard::clear(CACHE_ENV);
        let project = TempDir::new().expect("create temp project dir");

        let expected_id = expected_project_id(project.path());
        let actual = project_cache_dir(project.path());

        assert_eq!(actual, cache_base_dir().join("projects").join(&expected_id));
        assert_eq!(expected_id.len(), 16);
        assert!(expected_id.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    #[serial]
    fn project_cache_dir_honors_absolute_cache_override_structure() {
        let tmp = TempDir::new().expect("create temp dir");
        let custom_base = tmp.path().join("global-cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, &custom_base);
        let project = TempDir::new().expect("create temp project dir");

        let expected_id = expected_project_id(project.path());
        let actual = project_cache_dir(project.path());
        let expected = custom_base.join("projects").join(expected_id);

        assert_eq!(actual, expected);
    }

    #[test]
    #[serial]
    fn project_cache_dir_differs_for_different_roots() {
        let _guard = EnvVarGuard::clear(CACHE_ENV);
        let project_a = TempDir::new().expect("create temp dir A");
        let project_b = TempDir::new().expect("create temp dir B");

        let cache_a = project_cache_dir(project_a.path());
        let cache_b = project_cache_dir(project_b.path());

        let id_a = cache_a
            .file_name()
            .expect("cache dir should have id segment")
            .to_string_lossy()
            .to_string();
        let id_b = cache_b
            .file_name()
            .expect("cache dir should have id segment")
            .to_string_lossy()
            .to_string();

        assert_ne!(id_a, id_b);
    }

    #[test]
    #[serial]
    fn project_cache_dir_is_stable_for_same_root() {
        let _guard = EnvVarGuard::clear(CACHE_ENV);
        let project = TempDir::new().expect("create temp project dir");

        let first = project_cache_dir(project.path());
        let second = project_cache_dir(project.path());

        assert_eq!(first, second);
    }

    #[test]
    #[serial]
    fn project_cache_dir_anchors_noncanonical_relative_roots_before_hashing() {
        let _guard = EnvVarGuard::clear(CACHE_ENV);
        let cwd = TempDir::new().expect("create temp cwd");
        let _cwd_guard = CurrentDirGuard::set(cwd.path());
        let relative = PathBuf::from("not-yet-created");
        let absolute = cwd.path().join(&relative);

        let relative_cache = project_cache_dir(&relative);
        let absolute_cache = project_cache_dir(&absolute);

        assert_eq!(
            relative_cache, absolute_cache,
            "relative roots that cannot be canonicalized must be absolutized before project_id hashing"
        );
    }

    #[test]
    #[serial]
    fn snapshot_load_lock_does_not_create_empty_project_bucket() {
        let tmp = TempDir::new().expect("create temp dir");
        let custom = tmp.path().join("custom-cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, &custom);
        let project = TempDir::new().expect("create temp project dir");

        let project_bucket = project_cache_dir(project.path());
        let lock_path = project_cache_lock_path(project.path());
        let err = Snapshot::load(project.path()).expect_err("empty project has no snapshot");

        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(
            !project_bucket.exists(),
            "load-time locking must not create an empty project cache bucket"
        );
        assert!(
            lock_path.exists(),
            "cache-level lock file should be created"
        );
    }

    #[test]
    #[serial]
    fn project_cache_dir_normalizes_trailing_slash() {
        let _guard = EnvVarGuard::clear(CACHE_ENV);
        let project = TempDir::new().expect("create temp project dir");
        let canonical = project
            .path()
            .canonicalize()
            .expect("canonicalize project path");
        let with_trailing_slash = PathBuf::from(format!("{}/", canonical.display()));

        let without_slash = project_cache_dir(&canonical);
        let with_slash = project_cache_dir(&with_trailing_slash);

        assert_eq!(without_slash, with_slash);
    }

    #[test]
    #[serial]
    fn artifacts_dir_for_non_git_root_matches_project_cache_dir() {
        let _guard = EnvVarGuard::clear(CACHE_ENV);
        let project = TempDir::new().expect("create temp project dir");

        let artifacts = Snapshot::artifacts_dir(project.path());
        let cache = project_cache_dir(project.path());

        assert_eq!(artifacts, cache);
    }

    #[test]
    #[serial]
    fn artifacts_dir_sanitizes_branch_in_scan_segment() {
        let _guard = EnvVarGuard::clear(CACHE_ENV);
        let repo = TempDir::new().expect("create temp repo");
        let root = repo.path();
        std::fs::write(root.join("README.md"), "init").expect("write seed file");

        run_git(root, &["init"]);
        run_git(root, &["config", "user.email", "test@example.com"]);
        run_git(root, &["config", "user.name", "Test User"]);
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-m", "init"]);
        run_git(root, &["checkout", "-b", "release/v0.8.13"]);

        let commit = git_stdout(root, &["rev-parse", "--short", "HEAD"]);
        let artifacts = Snapshot::artifacts_dir(root);
        let scan_segment = artifacts
            .file_name()
            .expect("artifacts dir should end with scan segment")
            .to_string_lossy()
            .to_string();

        assert_eq!(scan_segment, format!("release_v0.8.13@{commit}"));
    }

    #[test]
    fn artifact_display_is_relative_without_dot_prefix() {
        let loctree_dir = PathBuf::from("/tmp/cache/loctree/projects/abc/main@1234");
        let artifact = loctree_dir.join("report.html");

        let display = display_artifact_path(&artifact, &loctree_dir);

        assert_eq!(display, "report.html");
        assert!(!display.starts_with("./"));
        assert!(!display.contains(".//"));
        assert!(!Path::new(&display).is_absolute());
    }

    #[test]
    fn artifact_display_falls_back_to_absolute_when_strip_prefix_fails() {
        let loctree_dir = PathBuf::from("/tmp/cache/loctree/projects/abc/main@1234");
        let artifact = PathBuf::from("/tmp/other/path/report.html");

        let display = display_artifact_path(&artifact, &loctree_dir);

        assert_eq!(display, artifact.display().to_string());
        assert!(!display.starts_with("./"));
        assert!(!display.contains(".//"));
        assert!(Path::new(&display).is_absolute());
    }

    #[test]
    #[serial]
    fn resolve_snapshot_root_does_not_walk_up_past_explicit_project_marker() {
        let _guard = EnvVarGuard::clear(CACHE_ENV);
        let workspace = TempDir::new().expect("create temp workspace");
        let subproject = workspace.path().join("apps/web");
        std::fs::create_dir_all(&subproject).expect("create nested project");
        std::fs::write(subproject.join("package.json"), "{}").expect("write package.json");

        run_git(workspace.path(), &["init"]);

        let resolved = resolve_snapshot_root(std::slice::from_ref(&subproject));
        let expected = subproject.canonicalize().expect("canonicalize subproject");
        let actual = resolved.canonicalize().expect("canonicalize resolved root");
        assert_eq!(actual, expected);
    }

    #[test]
    #[serial]
    fn resolve_snapshot_root_with_exact_strategy_keeps_requested_subtree() {
        let _guard = EnvVarGuard::clear(CACHE_ENV);
        let workspace = TempDir::new().expect("create temp workspace");
        let src = workspace.path().join("apps/web/src");
        std::fs::create_dir_all(&src).expect("create nested src dir");

        run_git(workspace.path(), &["init"]);

        let resolved = resolve_snapshot_root_with_strategy(
            std::slice::from_ref(&src),
            SnapshotRootStrategy::Exact,
        );
        let expected = src.canonicalize().expect("canonicalize src root");
        assert_eq!(resolved, expected);
    }

    #[test]
    #[serial]
    fn load_prefers_cache_when_both_cache_and_legacy_exist() {
        let cache_root = TempDir::new().expect("create temp cache dir");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, cache_root.path());
        let project = TempDir::new().expect("create temp project dir");

        let cache_path = Snapshot::snapshot_path(project.path());
        std::fs::create_dir_all(
            cache_path
                .parent()
                .expect("cache snapshot path must have parent"),
        )
        .expect("create cache snapshot parent");
        let cache_snapshot = Snapshot::new(vec!["cache-source".to_string()]);
        std::fs::write(
            &cache_path,
            serde_json::to_string_pretty(&cache_snapshot).expect("serialize cache snapshot"),
        )
        .expect("write cache snapshot");

        let legacy_path = project.path().join(SNAPSHOT_DIR).join(SNAPSHOT_FILE);
        std::fs::create_dir_all(
            legacy_path
                .parent()
                .expect("legacy snapshot path must have parent"),
        )
        .expect("create legacy snapshot parent");
        std::thread::sleep(std::time::Duration::from_millis(10));
        let legacy_snapshot = Snapshot::new(vec!["legacy-source".to_string()]);
        std::fs::write(
            &legacy_path,
            serde_json::to_string_pretty(&legacy_snapshot).expect("serialize legacy snapshot"),
        )
        .expect("write legacy snapshot");

        let loaded = Snapshot::load(project.path()).expect("load snapshot");
        assert_eq!(loaded.metadata.roots, vec!["cache-source".to_string()]);

        let marker_path = project
            .path()
            .join(SNAPSHOT_DIR)
            .join(LEGACY_MIGRATION_MARKER);
        assert!(
            !marker_path.exists(),
            "marker should not be written when cache already exists"
        );
    }

    #[test]
    #[serial]
    fn load_migrates_legacy_snapshot_to_cache_with_marker() {
        let cache_root = TempDir::new().expect("create temp cache dir");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, cache_root.path());
        let project = TempDir::new().expect("create temp project dir");

        let legacy_path = project.path().join(SNAPSHOT_DIR).join(SNAPSHOT_FILE);
        std::fs::create_dir_all(
            legacy_path
                .parent()
                .expect("legacy snapshot path must have parent"),
        )
        .expect("create legacy snapshot parent");
        let legacy_snapshot = Snapshot::new(vec!["legacy-source".to_string()]);
        std::fs::write(
            &legacy_path,
            serde_json::to_string_pretty(&legacy_snapshot).expect("serialize legacy snapshot"),
        )
        .expect("write legacy snapshot");

        let loaded = Snapshot::load(project.path()).expect("load migrated snapshot");
        assert_eq!(loaded.metadata.roots, vec!["legacy-source".to_string()]);

        let cache_path = Snapshot::snapshot_path(project.path());
        assert!(cache_path.exists(), "cache snapshot should be created");

        let marker_path = project
            .path()
            .join(SNAPSHOT_DIR)
            .join(LEGACY_MIGRATION_MARKER);
        assert!(marker_path.exists(), "migration marker should be created");
        let marker = std::fs::read_to_string(&marker_path).expect("read migration marker");
        assert!(marker.contains("legacy_snapshot="));
        assert!(marker.contains("cache_snapshot="));

        std::fs::remove_file(&legacy_path).expect("remove legacy snapshot");
        let loaded_again = Snapshot::load(project.path()).expect("load from cache after migration");
        assert_eq!(
            loaded_again.metadata.roots,
            vec!["legacy-source".to_string()]
        );
    }

    // ── parse_owner_repo tests ──────────────────────────────────

    #[test]
    fn snapshot_parse_owner_repo_https() {
        assert_eq!(
            parse_owner_repo("https://github.com/Loctree/loctree.git"),
            Some("Loctree/loctree".to_string()),
        );
        assert_eq!(
            parse_owner_repo("https://github.com/Loctree/loctree"),
            Some("Loctree/loctree".to_string()),
        );
    }

    #[test]
    fn snapshot_parse_owner_repo_ssh() {
        assert_eq!(
            parse_owner_repo("git@github.com:Loctree/loctree.git"),
            Some("Loctree/loctree".to_string()),
        );
        assert_eq!(
            parse_owner_repo("git@gitlab.example.com:org/sub-repo.git"),
            Some("org/sub-repo".to_string()),
        );
    }

    #[test]
    fn snapshot_parse_owner_repo_edge_cases() {
        // Empty / whitespace
        assert_eq!(parse_owner_repo(""), None);
        assert_eq!(parse_owner_repo("   "), None);

        // No path segments → None (bare hostname)
        assert_eq!(parse_owner_repo("localhost"), None);
    }

    #[test]
    fn snapshot_parse_repo_name_extracts_last_segment() {
        assert_eq!(
            parse_repo_name("https://github.com/Loctree/loctree.git"),
            Some("loctree".to_string()),
        );
        assert_eq!(
            parse_repo_name("git@github.com:Loctree/loctree.git"),
            Some("loctree".to_string()),
        );
    }

    // ── backward-compat serde roundtrip ───────────────────────

    #[test]
    fn snapshot_metadata_backward_compat_old_json_missing_owner_repo() {
        // Simulates loading a snapshot saved before git_owner_repo existed.
        let old_json = r#"{
            "schema_version": "0.8.16",
            "generated_at": "2025-06-01T00:00:00Z",
            "roots": ["src"],
            "languages": [],
            "file_count": 5,
            "total_loc": 500,
            "scan_duration_ms": 100,
            "git_repo": "loctree",
            "git_branch": "main",
            "git_commit": "abc1234",
            "git_scan_id": "main@abc1234"
        }"#;

        let meta: SnapshotMetadata =
            serde_json::from_str(old_json).expect("deserialize old snapshot");
        assert_eq!(meta.git_repo, Some("loctree".to_string()));
        assert_eq!(meta.git_owner_repo, None); // graceful default
        assert_eq!(meta.git_branch, Some("main".to_string()));
    }

    #[test]
    fn snapshot_backcompat_v0_10_2_file_analysis_defaults_new_foundation_fields() {
        let snapshot: Snapshot =
            serde_json::from_str(include_str!("../tests/fixtures/snapshot_v0_10_2.json"))
                .expect("deserialize v0.10.2 snapshot");
        assert_eq!(snapshot.metadata.schema_version, "0.10.2");
        let file = snapshot.files.first().expect("fixture file");
        assert!(file.impl_methods.is_empty());
        assert!(file.cargo_targets.is_empty());
        assert!(file.log_messages.is_empty());
        assert_eq!(file.crate_membership, None);
        assert_eq!(file.exports[0].name, "hello");
    }

    #[test]
    fn snapshot_metadata_roundtrip_with_owner_repo() {
        let meta = SnapshotMetadata {
            schema_version: SNAPSHOT_SCHEMA_VERSION.to_string(),
            generated_at: "2026-03-31T00:00:00Z".to_string(),
            roots: vec!["src".to_string()],
            languages: HashSet::from(["rust".to_string()]),
            file_count: 42,
            total_loc: 5000,
            scan_duration_ms: 200,
            resolver_config: None,
            manifest_summary: Vec::new(),
            entrypoints: Vec::new(),
            entrypoint_drift: EntrypointDriftSummary::default(),
            git_repo: Some("loctree".to_string()),
            git_owner_repo: Some("Loctree/loctree".to_string()),
            git_branch: Some("main".to_string()),
            git_commit: Some("d6ecd24".to_string()),
            git_scan_id: Some("main@d6ecd24".to_string()),
        };

        let json = serde_json::to_string_pretty(&meta).expect("serialize");
        let deser: SnapshotMetadata = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deser.git_repo, Some("loctree".to_string()));
        assert_eq!(deser.git_owner_repo, Some("Loctree/loctree".to_string()));
        assert_eq!(deser.git_branch, Some("main".to_string()));
        assert_eq!(deser.git_commit, Some("d6ecd24".to_string()));
    }

    #[test]
    #[serial]
    fn find_latest_snapshot_prefers_cache_even_when_legacy_is_newer() {
        let cache_root = TempDir::new().expect("create temp cache dir");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, cache_root.path());
        let project = TempDir::new().expect("create temp project dir");

        let cache_path = Snapshot::snapshot_path(project.path());
        std::fs::create_dir_all(
            cache_path
                .parent()
                .expect("cache snapshot path must have parent"),
        )
        .expect("create cache snapshot parent");
        let cache_snapshot = Snapshot::new(vec!["cache-source".to_string()]);
        std::fs::write(
            &cache_path,
            serde_json::to_string_pretty(&cache_snapshot).expect("serialize cache snapshot"),
        )
        .expect("write cache snapshot");

        let legacy_path = project.path().join(SNAPSHOT_DIR).join(SNAPSHOT_FILE);
        std::fs::create_dir_all(
            legacy_path
                .parent()
                .expect("legacy snapshot path must have parent"),
        )
        .expect("create legacy snapshot parent");
        std::thread::sleep(std::time::Duration::from_millis(10));
        let legacy_snapshot = Snapshot::new(vec!["legacy-source".to_string()]);
        std::fs::write(
            &legacy_path,
            serde_json::to_string_pretty(&legacy_snapshot).expect("serialize legacy snapshot"),
        )
        .expect("write legacy snapshot");

        let found =
            Snapshot::find_latest_snapshot_in(project.path()).expect("find latest snapshot");
        let found = found.canonicalize().unwrap_or(found);
        let expected = cache_path.canonicalize().unwrap_or(cache_path);
        assert_eq!(found, expected);
    }
}
