//! Handler for `loct doctor` cache identity and scope diagnostics.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Iso8601};

use crate::cli::command::{DoctorOptions, GlobalOptions};
use crate::snapshot::{
    Snapshot, SnapshotMetadata, cache_base_dir, is_git_dirty, normalize_roots_for_scope_compare,
    project_cache_dir, resolve_snapshot_root,
};

use super::super::DispatchResult;

const CACHE_WARNING_THRESHOLD_BYTES: u64 = 5_000_000_000;

/// Stable JSON output schema for `loct doctor`.
///
/// Schema version: `"1.1"`.
///
/// Consumers (CI gates, agent context packs, external tooling) may rely on:
/// - top-level fields: `schema_version`, `generated_at`, `entries`
/// - per-entry fields: `project_id`, `canonical_root`, `last_scan_branch`,
///   `last_scan_commit`, `last_scan_mtime`, `cache_size_bytes`,
///   `scope_state`, `fix_command`
/// - `scope_state` discriminator: `"Fresh" | "StaleCommit" | "DirtyWorktree"
///   | "ScopeMismatch" | "Corrupt" | "NotFound"`
///
/// Schema bumps follow semver: bug fixes don't bump, additive fields are
/// minor (`"1.1"`), breaking changes are major (`"2.0"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub schema_version: String,
    pub generated_at: String,
    pub entries: Vec<CacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub project_id: String,
    pub canonical_root: Option<String>,
    pub last_scan_branch: Option<String>,
    pub last_scan_commit: Option<String>,
    pub last_scan_mtime: Option<String>,
    pub cache_size_bytes: Option<u64>,
    pub scope_state: Option<ScopeState>,
    pub fix_command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScopeState {
    Fresh,
    StaleCommit,
    DirtyWorktree,
    ScopeMismatch {
        expected: Vec<String>,
        actual: Vec<String>,
    },
    Corrupt(String),
    NotFound,
}

#[derive(Debug, Deserialize)]
struct SnapshotEnvelope {
    #[serde(default)]
    metadata: SnapshotMetadata,
}

pub fn run(opts: &DoctorOptions, global: &GlobalOptions) -> DispatchResult {
    if uses_legacy_doctor(opts, global) {
        return super::analysis::handle_doctor_command(opts, global);
    }

    // No mode flag picked → pick the right default for *this* invocation.
    // The vc-init skill instructs operators to call `doctor()` before any
    // edit window to compare fingerprint against the last call. The old
    // default (global cache list of every cached project) made that reflex
    // useless — operators got 9000+ tmp fixtures, not a diagnostic for the
    // current repo. New default: if cwd has a snapshot, report on that
    // project. Otherwise, hint at the available flags and fall back to the
    // historical `--list` so existing scripts that grep the output still
    // work.
    let default_storage;
    let opts_with_default: &DoctorOptions = if needs_default_mode(opts, global) {
        match infer_default_mode(opts) {
            DefaultMode::PerProject(root) => {
                default_storage = DoctorOptions {
                    scope: true,
                    project: Some(root),
                    ..opts.clone()
                };
                &default_storage
            }
            DefaultMode::GlobalList { hint } => {
                if let Some(message) = hint {
                    eprintln!("{message}");
                }
                default_storage = DoctorOptions {
                    list: true,
                    ..opts.clone()
                };
                &default_storage
            }
        }
    } else {
        opts
    };

    // --fix needs scope_state populated to know what to purge. Force --scope
    // on so the operator can pass `--fix` without remembering the implied flag.
    let effective_storage;
    let opts_for_report: &DoctorOptions = if opts_with_default.fix && !opts_with_default.scope {
        effective_storage = DoctorOptions {
            scope: true,
            ..opts_with_default.clone()
        };
        &effective_storage
    } else {
        opts_with_default
    };

    let mut report = match build_report(opts_for_report) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("[loct][doctor] {err:#}");
            return DispatchResult::Exit(1);
        }
    };

    if opts_with_default.fix {
        match run_fix(&report, opts_with_default) {
            Ok(()) => match build_report(opts_for_report) {
                Ok(refreshed) => report = refreshed,
                Err(err) => {
                    eprintln!("[loct][doctor] post-fix rebuild failed: {err:#}");
                    return DispatchResult::Exit(1);
                }
            },
            Err(FixError::NeedsTty) => {
                eprintln!(
                    "[loct][doctor] --fix requires either a TTY or --yes flag for non-interactive use"
                );
                return DispatchResult::Exit(2);
            }
            Err(FixError::Other(err)) => {
                eprintln!("[loct][doctor] {err:#}");
                return DispatchResult::Exit(1);
            }
        }
    }

    if opts_with_default.json || global.json {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("[loct][doctor] failed to serialize report: {err}");
                return DispatchResult::Exit(1);
            }
        }
    } else {
        render_human(&report);
    }
    DispatchResult::Exit(0)
}

fn uses_legacy_doctor(opts: &DoctorOptions, global: &GlobalOptions) -> bool {
    !global.json
        && !opts.cache
        && !opts.scope
        && !opts.list
        && !opts.json
        && !opts.fix
        && !opts.yes
        && opts.project.is_none()
        && (opts.include_tests || opts.apply_suppressions || !opts.roots.is_empty())
}

/// True when the operator did not pick a mode flag and the handler should
/// pick a sensible default. Mode flags are: `--cache`, `--scope`, `--list`,
/// `--fix`, `--project`, `--json`. Anything else (positional roots,
/// `--include-tests`, `--apply-suppressions`) is handled by the legacy
/// doctor path above and never reaches this function in default mode.
fn needs_default_mode(opts: &DoctorOptions, global: &GlobalOptions) -> bool {
    !opts.cache
        && !opts.scope
        && !opts.list
        && !opts.fix
        && !opts.json
        && !global.json
        && opts.project.is_none()
        && opts.roots.is_empty()
}

/// What the handler should do when the operator typed bare `loct doctor`.
enum DefaultMode {
    /// The current working directory has a recognizable snapshot; run a
    /// per-project scope diagnostic for it (the Living Tree fingerprint
    /// check that the `vc-init` skill assumes).
    PerProject(PathBuf),
    /// No snapshot near the cwd; fall back to the historical global cache
    /// listing so existing scripts still work. The hint, when present, is
    /// emitted to stderr so the operator learns about the better invocations.
    GlobalList { hint: Option<String> },
}

/// Decide whether the cwd looks like a scanned project (so we can default
/// to per-project diagnostic mode) or whether to keep the historical global
/// list. Detection uses the same `Snapshot::find_loctree_root` walk that
/// every other doctor entry point uses, so all three sources of truth
/// (parser, MCP guard, CLI guard) agree on what counts as "a project here".
fn infer_default_mode(_opts: &DoctorOptions) -> DefaultMode {
    let cwd = match std::env::current_dir() {
        Ok(path) => path,
        Err(err) => {
            return DefaultMode::GlobalList {
                hint: Some(format!(
                    "[loct][doctor] could not resolve cwd ({err}); falling back to --list"
                )),
            };
        }
    };

    match Snapshot::find_loctree_root(&cwd) {
        Some(project_root) => DefaultMode::PerProject(project_root),
        None => DefaultMode::GlobalList {
            hint: Some(format!(
                "[loct][doctor] no snapshot found near {}.\n\
                 [loct][doctor] hint: run `loct scan` to enable the per-project diagnostic, \
                 or use `loct doctor --list` for the global cache, \
                 `loct doctor --scope --project <PATH>` for an explicit project.",
                cwd.display()
            )),
        },
    }
}

fn build_report(opts: &DoctorOptions) -> Result<DoctorReport> {
    // Resolve --project upward to the actual snapshot root (matches scan's behavior).
    // Without this, a sub-directory PATH would hash to a project_id that doesn't
    // exist in the cache, even when the parent project is fully scanned.
    let resolved_project: Option<PathBuf> = if opts.scope {
        opts.project
            .as_ref()
            .map(|p| resolve_snapshot_root(std::slice::from_ref(p)))
    } else {
        opts.project.clone()
    };

    // `target_id` is the SHA-256-prefix project_id derived from `--project`.
    // We pass it as a plain string to `enumerate_cache_entries` so the
    // operator-supplied path NEVER reaches a filesystem sink — only string
    // equality against `entry.file_name()` of locally-iterated cache dirs.
    let target_id: Option<String> = resolved_project.as_deref().map(project_id_for);

    let mut entries = if opts.list || opts.cache || opts.scope {
        enumerate_cache_entries(target_id.as_deref())?
    } else {
        Vec::new()
    };

    if opts.scope {
        annotate_scope(&mut entries, resolved_project.as_deref());
    }

    Ok(DoctorReport {
        schema_version: "1.1".to_string(),
        generated_at: now_iso8601(),
        entries,
    })
}

/// Populate `scope_state` and `fix_command` for every entry. When
/// `requested_root` is `Some`, all entries are validated against that root
/// (the operator's `--project` choice). Otherwise each entry is validated
/// against its own canonical root.
///
/// If `requested_root` is `Some` but no entry was enumerated for that root
/// (the cache is empty for that project), a synthetic `NotFound` entry is
/// appended so the operator sees the gap rather than a silent empty list.
fn annotate_scope(entries: &mut Vec<CacheEntry>, requested_root: Option<&Path>) {
    for entry in entries.iter_mut() {
        let canonical_path = entry.canonical_root.as_deref().map(PathBuf::from);
        let asked_root: Option<&Path> = requested_root.or(canonical_path.as_deref());
        let state = validate_scope(entry, asked_root);
        let project_root = asked_root.unwrap_or(Path::new("."));
        entry.fix_command = fix_command_for(&state, project_root);
        entry.scope_state = Some(state);
    }

    if entries.is_empty()
        && let Some(project_root) = requested_root
    {
        entries.push(CacheEntry {
            project_id: project_id_for(project_root),
            canonical_root: Some(project_root.display().to_string()),
            last_scan_branch: None,
            last_scan_commit: None,
            last_scan_mtime: None,
            cache_size_bytes: None,
            scope_state: Some(ScopeState::NotFound),
            fix_command: Some(format!("loct scan --project {}", project_root.display())),
        });
    }
}

/// Classify a single cache entry against the operator's expected scope.
///
/// Validation reuses three existing sources of truth:
/// 1. `resolve_snapshot_root` — same project-root resolution as `loct scan`
/// 2. `normalize_roots_for_scope_compare` — same canonical-form comparator
///    used by the CLI guard (`load_or_create_snapshot_for_roots`) and the
///    MCP guard (`get_snapshot`)
/// 3. `Snapshot::is_commit_stale` + `is_git_dirty` — same staleness gates
///    used elsewhere; we split them so the operator sees *why* something
///    is stale.
fn validate_scope(entry: &CacheEntry, requested_root: Option<&Path>) -> ScopeState {
    let canonical_root = match &entry.canonical_root {
        Some(value) => PathBuf::from(value),
        None => return ScopeState::Corrupt("no canonical_root in cached snapshot".into()),
    };

    let asked_root = requested_root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| canonical_root.clone());

    let snapshot = match Snapshot::load(&asked_root) {
        Ok(snapshot) => snapshot,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return ScopeState::NotFound,
        Err(err) => return ScopeState::Corrupt(format!("load failed: {err}")),
    };

    let snapshot_root = resolve_snapshot_root(std::slice::from_ref(&asked_root));
    let requested =
        normalize_roots_for_scope_compare(std::iter::once(asked_root.as_path()), &snapshot_root);
    let actual = normalize_roots_for_scope_compare(
        snapshot.metadata.roots.iter().map(Path::new),
        &snapshot_root,
    );

    if requested != actual {
        return ScopeState::ScopeMismatch {
            expected: requested,
            actual,
        };
    }

    if snapshot.is_commit_stale(&snapshot_root) {
        return ScopeState::StaleCommit;
    }
    if matches!(is_git_dirty(&snapshot_root), Some(true)) {
        return ScopeState::DirtyWorktree;
    }
    ScopeState::Fresh
}

/// Suggested resolution for a non-Fresh scope state. The string is
/// machine-discoverable (looks like a real CLI invocation) and human-readable.
fn fix_command_for(state: &ScopeState, project_root: &Path) -> Option<String> {
    let display = project_root.display();
    match state {
        ScopeState::Fresh => None,
        ScopeState::StaleCommit | ScopeState::DirtyWorktree => {
            Some(format!("loct scan --fresh --project {display}"))
        }
        ScopeState::ScopeMismatch { .. } => Some(format!("loct doctor --fix --project {display}")),
        ScopeState::Corrupt(_) => Some(format!("manual: investigate {display}")),
        ScopeState::NotFound => Some(format!("loct scan --project {display}")),
    }
}

/// Compute the project_id (same SHA-256 prefix used by the global cache)
/// for an arbitrary root. `project_cache_dir` returns
/// `<cache>/projects/<project_id>`; the final segment is the id.
fn project_id_for(root: &Path) -> String {
    project_cache_dir(root)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_default()
}

/// Internal error type for `run_fix`. Distinguishes the "needs a TTY"
/// non-interactive failure (exit code 2 — recoverable by the operator
/// adding `--yes`) from generic IO/parse errors (exit code 1).
#[derive(Debug)]
enum FixError {
    NeedsTty,
    Other(anyhow::Error),
}

impl From<anyhow::Error> for FixError {
    fn from(err: anyhow::Error) -> Self {
        FixError::Other(err)
    }
}

/// Walk the report's scope-mismatched entries and purge each one's flat
/// fallback snapshot, optionally with per-entry interactive confirmation.
///
/// Discipline:
/// - Operates ONLY on entries whose `scope_state == ScopeMismatch`. Fresh,
///   StaleCommit, DirtyWorktree, Corrupt, and NotFound are never touched.
/// - Stale/dirty/corrupt entries get a hint via `render_human`; the operator
///   resolves them with `loct scan --fresh`, not with `--fix`.
/// - Purge action removes only the flat fallback (`<project_id>/snapshot.json`).
///   It does NOT touch the per-scan `<branch>@<commit>/snapshot.json`
///   subdirectories: those are real history, not the contamination class
///   the MCP scope guard catches.
/// - In non-interactive mode (stdin not a TTY) without `--yes`, returns
///   `FixError::NeedsTty` so the caller can exit with code 2 instead of
///   silently auto-purging. No accidental purges in CI.
fn run_fix(report: &DoctorReport, opts: &DoctorOptions) -> std::result::Result<(), FixError> {
    #[cfg(not(test))]
    use std::io::IsTerminal;
    use std::io::Write;

    let mismatches: Vec<&CacheEntry> = report
        .entries
        .iter()
        .filter(|entry| matches!(entry.scope_state, Some(ScopeState::ScopeMismatch { .. })))
        .collect();

    if mismatches.is_empty() {
        eprintln!("[loct][doctor] no scope-mismatched cache entries to fix.");
        return Ok(());
    }

    #[cfg(not(test))]
    let is_tty = std::io::stdin().is_terminal();
    #[cfg(test)]
    let is_tty = false;
    if !is_tty && !opts.yes {
        return Err(FixError::NeedsTty);
    }

    if opts.yes {
        eprintln!(
            "[loct][doctor] about to purge {} scope-mismatched cache entr{}:",
            mismatches.len(),
            if mismatches.len() == 1 { "y" } else { "ies" }
        );
        for entry in &mismatches {
            eprintln!(
                "  {} ({})",
                entry.project_id,
                entry.canonical_root.as_deref().unwrap_or("?")
            );
        }
        eprintln!("(--yes given; proceeding without per-entry prompt.)");
        for entry in &mismatches {
            purge_flat_fallback(entry).map_err(FixError::from)?;
            eprintln!("  purged {}", entry.project_id);
        }
        return Ok(());
    }

    for entry in &mismatches {
        eprint!(
            "[loct][doctor] purge scope-mismatched cache for {} ({})? [y/N]: ",
            entry.project_id,
            entry.canonical_root.as_deref().unwrap_or("?")
        );
        std::io::stderr().flush().ok();
        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .map_err(|err| FixError::Other(anyhow::Error::from(err)))?;
        if input.trim().eq_ignore_ascii_case("y") {
            purge_flat_fallback(entry).map_err(FixError::from)?;
            eprintln!("  purged.");
        } else {
            eprintln!("  skipped.");
        }
    }
    Ok(())
}

/// Remove the flat-fallback snapshot for a cache entry (the file the MCP
/// scope guard treats as untrusted). Per-scan `<branch>@<commit>/`
/// directories are intentionally left alone — they are immutable scan
/// history, not contamination.
fn purge_flat_fallback(entry: &CacheEntry) -> Result<()> {
    let flat_path = cache_base_dir()
        .join("projects")
        .join(&entry.project_id)
        .join("snapshot.json");
    match fs::remove_file(&flat_path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("remove {}", flat_path.display())),
    }
}

/// Walk the on-disk cache and assemble one [`CacheEntry`] per `<cache>/projects/<id>/`
/// directory. When `target_id` is `Some`, only the matching project_id is
/// inspected; the comparison is a plain string equality against
/// `DirEntry::file_name()`, so the operator-supplied `--project` value never
/// reaches a filesystem sink — only a hash-derived id does.
///
/// All filesystem reads use **local-call sources**: the outer `read_dir`
/// targets `cache_base_dir().join("projects")`, every nested path is
/// derived from `DirEntry::path()` (a value produced by the iteration
/// itself, not a function parameter). This is what keeps the
/// path-traversal taint analyzer satisfied without `nosemgrep` annotations.
fn enumerate_cache_entries(target_id: Option<&str>) -> Result<Vec<CacheEntry>> {
    let base = cache_base_dir();
    let projects_dir = base.join("projects");
    if !projects_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    let projects = fs::read_dir(&projects_dir)
        .with_context(|| format!("read cache projects directory {}", projects_dir.display()))?;

    for project_entry in projects {
        let project_entry = project_entry.with_context(|| {
            format!("read cache project entry under {}", projects_dir.display())
        })?;
        let file_type = project_entry.file_type().with_context(|| {
            format!(
                "read file type for cache entry {}",
                project_entry.path().display()
            )
        })?;
        if !file_type.is_dir() {
            continue;
        }

        let project_id = project_entry.file_name().to_string_lossy().to_string();
        if let Some(wanted) = target_id
            && project_id != wanted
        {
            continue;
        }

        // From here on every path is derived from the iterator's own
        // `DirEntry::path()` — i.e. a local-call source. No function
        // parameter touches filesystem APIs.
        let project_dir = project_entry.path();
        let mut candidates: Vec<(PathBuf, SystemTime)> = Vec::new();

        // Conventional snapshot locations we check first.
        for fixed_path in [
            project_dir.join("latest").join("snapshot.json"),
            project_dir.join("snapshot.json"),
        ] {
            if let Ok(metadata) = fs::metadata(&fixed_path)
                && metadata.is_file()
            {
                if let Ok(modified) = metadata.modified() {
                    candidates.push((fixed_path, modified));
                    continue;
                }
                return Err(anyhow::anyhow!(
                    "read mtime for cache project {project_id} snapshot {}",
                    candidates
                        .last()
                        .map(|(p, _)| p.display().to_string())
                        .unwrap_or_default()
                ));
            }
        }

        // Fall back to per-scan `<branch>@<commit>/snapshot.json` subdirs.
        if candidates.is_empty() {
            let children = fs::read_dir(&project_dir).with_context(|| {
                format!(
                    "read cache project {project_id} directory {}",
                    project_dir.display()
                )
            })?;
            for child in children {
                let child = child.with_context(|| {
                    format!(
                        "read cache project {project_id} entry under {}",
                        project_dir.display()
                    )
                })?;
                let child_type = child.file_type().with_context(|| {
                    format!(
                        "read file type for cache project {project_id} entry {}",
                        child.path().display()
                    )
                })?;
                if !child_type.is_dir() {
                    continue;
                }
                let candidate_path = child.path().join("snapshot.json");
                if let Ok(metadata) = fs::metadata(&candidate_path)
                    && metadata.is_file()
                    && let Ok(modified) = metadata.modified()
                {
                    candidates.push((candidate_path, modified));
                }
            }
        }

        // Pick the most recent snapshot (mtime, tie-break on path).
        candidates.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));

        // Inline metadata read — `path` here is a local-call-derived
        // PathBuf (via `DirEntry::path()` + `Path::join`), not a parameter.
        let metadata = match candidates.last() {
            Some((path, _)) => {
                let bytes = fs::read(path).with_context(|| {
                    format!(
                        "read cache project {project_id} snapshot {}",
                        path.display()
                    )
                })?;
                let envelope: SnapshotEnvelope =
                    serde_json::from_slice(&bytes).with_context(|| {
                        format!(
                            "parse cache project {project_id} snapshot {}",
                            path.display()
                        )
                    })?;
                Some(envelope.metadata)
            }
            None => None,
        };

        entries.push(CacheEntry {
            project_id: project_id.clone(),
            canonical_root: metadata.as_ref().and_then(first_root),
            last_scan_branch: metadata.as_ref().and_then(|meta| meta.git_branch.clone()),
            last_scan_commit: metadata.as_ref().and_then(|meta| meta.git_commit.clone()),
            last_scan_mtime: candidates.last().map(|(_, ts)| format_system_time(*ts)),
            cache_size_bytes: Some(dir_size_bytes(&project_dir)),
            scope_state: None,
            fix_command: None,
        });
    }

    entries.sort_by(|left, right| left.project_id.cmp(&right.project_id));
    Ok(entries)
}

fn first_root(metadata: &SnapshotMetadata) -> Option<String> {
    metadata
        .roots
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .find(|root| !root.is_empty())
        .map(str::to_string)
}

fn now_iso8601() -> String {
    OffsetDateTime::now_utc()
        .format(&Iso8601::DEFAULT)
        .unwrap_or_else(|_| "unknown".to_string())
}

fn format_system_time(timestamp: SystemTime) -> String {
    OffsetDateTime::from(timestamp)
        .format(&Iso8601::DEFAULT)
        .unwrap_or_else(|_| "unknown".to_string())
}

fn render_human(report: &DoctorReport) {
    let base = cache_base_dir();
    if report.entries.is_empty() {
        println!(
            "No cached projects found at {}",
            base.join("projects").display()
        );
        return;
    }

    let any_scope = report
        .entries
        .iter()
        .any(|entry| entry.scope_state.is_some());

    println!("Cached projects ({} total)", report.entries.len());
    if any_scope {
        println!("project_id | canonical_root | branch@commit | last_scan | scope | fix");
        println!("--- | --- | --- | --- | --- | ---");
    } else {
        println!("project_id | canonical_root | branch@commit | last_scan");
        println!("--- | --- | --- | ---");
    }
    for entry in &report.entries {
        if any_scope {
            println!(
                "{} | {} | {} | {} | {} | {}",
                entry.project_id,
                entry.canonical_root.as_deref().unwrap_or("(unknown root)"),
                format_scan_ref(entry),
                entry.last_scan_mtime.as_deref().unwrap_or("(unknown time)"),
                format_scope_state(entry.scope_state.as_ref()),
                entry.fix_command.as_deref().unwrap_or("-")
            );
            if let Some(hint) = scope_hint(entry.scope_state.as_ref()) {
                println!("    hint:   {hint}");
            }
        } else {
            println!(
                "{} | {} | {} | {}",
                entry.project_id,
                entry.canonical_root.as_deref().unwrap_or("(unknown root)"),
                format_scan_ref(entry),
                entry.last_scan_mtime.as_deref().unwrap_or("(unknown time)")
            );
        }
    }
    if let Some(warning) = cache_size_warning(report) {
        println!();
        println!("{warning}");
        println!("hint:   run `loct cache list` or `loct cache clean --max-size 5GB --force`");
    }
}

fn cache_size_warning(report: &DoctorReport) -> Option<String> {
    let total: u64 = report
        .entries
        .iter()
        .filter_map(|entry| entry.cache_size_bytes)
        .sum();
    (total > CACHE_WARNING_THRESHOLD_BYTES).then(|| {
        format!(
            "warning: loctree cache is {} total, above the 5GB warning threshold",
            format_bytes(total)
        )
    })
}

fn dir_size_bytes(path: &Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
        .sum()
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1_000 {
        format!("{bytes}B")
    } else if bytes < 1_000_000 {
        format!("{:.1}KB", bytes as f64 / 1_000.0)
    } else if bytes < 1_000_000_000 {
        format!("{:.1}MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{:.1}GB", bytes as f64 / 1_000_000_000.0)
    }
}

/// Operator-facing remediation hint for a non-Fresh scope state. Keep in
/// sync with `fix_command_for`: `fix_command` is machine-discoverable, the
/// hint here is the human-readable explanation of *why* and *how*.
fn scope_hint(state: Option<&ScopeState>) -> Option<String> {
    match state? {
        ScopeState::StaleCommit => {
            Some("stale-commit — rerun `loct scan --fresh` to refresh".to_string())
        }
        ScopeState::DirtyWorktree => {
            Some("dirty-worktree — rerun `loct scan --fresh` to refresh".to_string())
        }
        ScopeState::ScopeMismatch { .. } => {
            Some("scope mismatch — fix with `loct doctor --fix --project <root>`".to_string())
        }
        ScopeState::Corrupt(reason) => Some(format!(
            "corrupt cache — manual investigation needed ({reason})"
        )),
        ScopeState::NotFound => {
            Some("not-found — run `loct scan --project <root>` to populate".to_string())
        }
        ScopeState::Fresh => None,
    }
}

fn format_scope_state(state: Option<&ScopeState>) -> String {
    match state {
        None => "-".to_string(),
        Some(ScopeState::Fresh) => "fresh".to_string(),
        Some(ScopeState::StaleCommit) => "stale-commit".to_string(),
        Some(ScopeState::DirtyWorktree) => "dirty-worktree".to_string(),
        Some(ScopeState::ScopeMismatch { expected, actual }) => format!(
            "scope-mismatch (expected=[{}] actual=[{}])",
            expected.join(","),
            actual.join(",")
        ),
        Some(ScopeState::Corrupt(reason)) => format!("corrupt ({reason})"),
        Some(ScopeState::NotFound) => "not-found".to_string(),
    }
}

fn format_scan_ref(entry: &CacheEntry) -> String {
    match (&entry.last_scan_branch, &entry.last_scan_commit) {
        (Some(branch), Some(commit)) => format!("{branch}@{commit}"),
        (Some(branch), None) => branch.clone(),
        (None, Some(commit)) => commit.clone(),
        (None, None) => "(unknown ref)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;
    use tempfile::TempDir;

    const CACHE_ENV: &str = "LOCT_CACHE_DIR";

    #[derive(Debug)]
    struct EnvVarGuard {
        key: &'static str,
        original: Option<OsString>,
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

    #[test]
    #[serial]
    fn empty_cache_returns_empty_entries() {
        let cache = TempDir::new().expect("create temp cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, cache.path());
        let entries = enumerate_cache_entries(None).expect("enumerate cache");

        assert!(entries.is_empty());
    }

    #[test]
    #[serial]
    fn loct_cache_dir_override_respected() {
        let cache = TempDir::new().expect("create temp cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, cache.path());

        assert_eq!(cache_base_dir(), cache.path());
    }

    // ------------------------------------------------------------------
    // Scope validation tests (Cut 2 T1)
    // ------------------------------------------------------------------

    /// Build a minimal but well-formed snapshot.json at the legacy path
    /// (`<root>/.loctree/snapshot.json`). Returns the project root.
    ///
    /// `roots` populates `metadata.roots`; `git_commit` populates
    /// `metadata.git_commit`. Both control which `ScopeState` the validator
    /// will produce.
    fn write_snapshot_at(
        root: &Path,
        roots: Vec<String>,
        git_commit: Option<&str>,
    ) -> std::path::PathBuf {
        let snapshot_dir = root.join(".loctree");
        std::fs::create_dir_all(&snapshot_dir).expect("create snapshot dir");
        let snapshot_path = snapshot_dir.join("snapshot.json");
        let body = serde_json::json!({
            "metadata": {
                "schema_version": env!("CARGO_PKG_VERSION"),
                "generated_at": "2026-04-25T18:00:00.000000000Z",
                "roots": roots,
                "languages": [],
                "file_count": 0,
                "total_loc": 0,
                "scan_duration_ms": 0,
                "manifest_summary": [],
                "entrypoints": [],
                "entrypoint_drift": {
                    "declared_missing": [],
                    "declared_without_marker": [],
                    "code_only_entrypoints": [],
                    "declared_unresolved": []
                },
                "git_commit": git_commit,
            },
            "files": [],
            "edges": [],
            "export_index": {},
            "command_bridges": [],
            "event_bridges": [],
            "barrels": [],
        });
        std::fs::write(&snapshot_path, serde_json::to_string_pretty(&body).unwrap())
            .expect("write snapshot.json");
        snapshot_path
    }

    /// Build a `CacheEntry` whose `canonical_root` points at the temp
    /// project. The other fields don't matter for scope validation.
    fn entry_for(root: &Path) -> CacheEntry {
        CacheEntry {
            project_id: project_id_for(root),
            canonical_root: Some(root.display().to_string()),
            last_scan_branch: None,
            last_scan_commit: None,
            last_scan_mtime: None,
            cache_size_bytes: None,
            scope_state: None,
            fix_command: None,
        }
    }

    #[test]
    #[serial]
    fn scope_validation_fresh_state() {
        // Cache override isolates the migration target so the test never
        // touches the real user cache.
        let cache = TempDir::new().expect("create temp cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, cache.path());

        let project = TempDir::new().expect("create temp project");
        let canonical_root = project
            .path()
            .canonicalize()
            .expect("canonicalize project root");
        write_snapshot_at(
            &canonical_root,
            vec![canonical_root.display().to_string()],
            None,
        );

        let entry = entry_for(&canonical_root);
        let state = validate_scope(&entry, None);

        assert_eq!(state, ScopeState::Fresh);
    }

    #[test]
    #[serial]
    fn scope_validation_scope_mismatch() {
        let cache = TempDir::new().expect("create temp cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, cache.path());

        let project = TempDir::new().expect("create temp project");
        let canonical_root = project
            .path()
            .canonicalize()
            .expect("canonicalize project root");
        // Snapshot was written from a *sub-tree*, but the operator queries
        // the parent. This is the bug the MCP guard caught — surface it as
        // ScopeMismatch.
        let subtree = canonical_root.join("sub");
        std::fs::create_dir_all(&subtree).expect("create subtree dir");
        write_snapshot_at(&canonical_root, vec![subtree.display().to_string()], None);

        let entry = entry_for(&canonical_root);
        let state = validate_scope(&entry, None);

        match state {
            ScopeState::ScopeMismatch { expected, actual } => {
                assert!(
                    !expected.is_empty(),
                    "expected roots should not be empty: {expected:?}"
                );
                assert!(
                    !actual.is_empty(),
                    "actual roots should not be empty: {actual:?}"
                );
                assert_ne!(expected, actual, "expected and actual must differ");
            }
            other => panic!("expected ScopeMismatch, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn scope_validation_not_found_state() {
        let cache = TempDir::new().expect("create temp cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, cache.path());

        let project = TempDir::new().expect("create temp project");
        let canonical_root = project
            .path()
            .canonicalize()
            .expect("canonicalize project root");
        // No snapshot.json written anywhere — Snapshot::load returns NotFound.

        let entry = entry_for(&canonical_root);
        let state = validate_scope(&entry, None);

        assert_eq!(state, ScopeState::NotFound);
    }

    #[test]
    #[serial]
    fn scope_validation_corrupt_state() {
        let cache = TempDir::new().expect("create temp cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, cache.path());

        let project = TempDir::new().expect("create temp project");
        let canonical_root = project
            .path()
            .canonicalize()
            .expect("canonicalize project root");
        let snapshot_dir = canonical_root.join(".loctree");
        std::fs::create_dir_all(&snapshot_dir).expect("create snapshot dir");
        std::fs::write(snapshot_dir.join("snapshot.json"), "{ this is not json")
            .expect("write malformed snapshot");

        let entry = entry_for(&canonical_root);
        let state = validate_scope(&entry, None);

        match state {
            ScopeState::Corrupt(reason) => {
                assert!(
                    reason.contains("load failed"),
                    "expected corrupt reason to mention load failure, got: {reason}"
                );
            }
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }

    #[test]
    fn scope_validation_corrupt_when_canonical_root_missing() {
        let entry = CacheEntry {
            project_id: "abc".to_string(),
            canonical_root: None,
            last_scan_branch: None,
            last_scan_commit: None,
            last_scan_mtime: None,
            cache_size_bytes: None,
            scope_state: None,
            fix_command: None,
        };

        let state = validate_scope(&entry, None);

        match state {
            ScopeState::Corrupt(reason) => assert!(reason.contains("no canonical_root")),
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }

    #[test]
    fn fix_command_suggestions_match_state() {
        let root = std::path::Path::new("/tmp/foo");

        assert!(fix_command_for(&ScopeState::Fresh, root).is_none());

        let stale = fix_command_for(&ScopeState::StaleCommit, root).expect("stale fix");
        assert!(stale.contains("--fresh"));
        assert!(stale.contains("/tmp/foo"));

        let dirty = fix_command_for(&ScopeState::DirtyWorktree, root).expect("dirty fix");
        assert!(dirty.contains("--fresh"));

        let mismatch = fix_command_for(
            &ScopeState::ScopeMismatch {
                expected: vec!["/tmp/foo".to_string()],
                actual: vec!["/tmp/bar".to_string()],
            },
            root,
        )
        .expect("mismatch fix");
        assert!(mismatch.contains("--fix"));

        let corrupt =
            fix_command_for(&ScopeState::Corrupt("oops".into()), root).expect("corrupt fix");
        assert!(corrupt.contains("manual"));

        let not_found = fix_command_for(&ScopeState::NotFound, root).expect("not-found fix");
        assert!(not_found.contains("loct scan"));
    }

    #[test]
    #[serial]
    fn annotate_scope_synthesizes_not_found_when_filter_misses() {
        let cache = TempDir::new().expect("create temp cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, cache.path());

        let project = TempDir::new().expect("create temp project");
        let canonical_root = project
            .path()
            .canonicalize()
            .expect("canonicalize project root");
        // Empty cache, but operator asked about a specific project. We
        // should synthesize a NotFound entry rather than swallow the gap.
        let mut entries: Vec<CacheEntry> = Vec::new();
        annotate_scope(&mut entries, Some(&canonical_root));

        assert_eq!(entries.len(), 1, "expected synthesized NotFound entry");
        let synthesized = &entries[0];
        assert_eq!(synthesized.scope_state, Some(ScopeState::NotFound));
        assert_eq!(synthesized.project_id, project_id_for(&canonical_root));
        let fix = synthesized.fix_command.as_deref().expect("fix command");
        assert!(fix.contains("loct scan"));
        assert!(fix.contains(&*canonical_root.display().to_string()));
    }

    #[test]
    fn format_scope_state_includes_diff_payload() {
        let rendered = format_scope_state(Some(&ScopeState::ScopeMismatch {
            expected: vec!["/a".to_string(), "/b".to_string()],
            actual: vec!["/c".to_string()],
        }));
        assert!(rendered.contains("scope-mismatch"));
        assert!(rendered.contains("/a"));
        assert!(rendered.contains("/c"));

        assert_eq!(format_scope_state(Some(&ScopeState::Fresh)), "fresh");
        assert_eq!(format_scope_state(None), "-");
    }

    // ------------------------------------------------------------------
    // JSON schema + --fix tests (Cut 2 T2)
    // ------------------------------------------------------------------

    /// Build a `CacheEntry` with a fixed scope state for purge-mode tests.
    /// `project_id` matches the on-disk cache layout so `purge_flat_fallback`
    /// can resolve the path.
    fn entry_with_state(project_id: &str, root: &Path, state: ScopeState) -> CacheEntry {
        CacheEntry {
            project_id: project_id.to_string(),
            canonical_root: Some(root.display().to_string()),
            last_scan_branch: Some("develop".to_string()),
            last_scan_commit: Some("9d563ff".to_string()),
            last_scan_mtime: Some("2026-04-25T11:00:00Z".to_string()),
            cache_size_bytes: Some(1024),
            scope_state: Some(state),
            fix_command: None,
        }
    }

    #[test]
    fn json_schema_roundtrip_preserves_all_fields() {
        let report = DoctorReport {
            schema_version: "1.1".to_string(),
            generated_at: "2026-04-25T12:00:00Z".to_string(),
            entries: vec![
                CacheEntry {
                    project_id: "abc123def4567890".to_string(),
                    canonical_root: Some("/home/foo/bar".to_string()),
                    last_scan_branch: Some("develop".to_string()),
                    last_scan_commit: Some("9d563ff".to_string()),
                    last_scan_mtime: Some("2026-04-25T11:00:00Z".to_string()),
                    cache_size_bytes: Some(1_024),
                    scope_state: Some(ScopeState::Fresh),
                    fix_command: None,
                },
                CacheEntry {
                    project_id: "ffff111122223333".to_string(),
                    canonical_root: Some("/home/foo/baz".to_string()),
                    last_scan_branch: None,
                    last_scan_commit: None,
                    last_scan_mtime: None,
                    cache_size_bytes: Some(5_000_000_001),
                    scope_state: Some(ScopeState::ScopeMismatch {
                        expected: vec!["/home/foo/baz".to_string()],
                        actual: vec!["/home/foo/baz/sub".to_string()],
                    }),
                    fix_command: Some("loct doctor --fix --project /home/foo/baz".to_string()),
                },
            ],
        };

        let json = serde_json::to_string_pretty(&report).expect("serialize");
        let back: DoctorReport = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.schema_version, "1.1");
        assert_eq!(back.entries.len(), 2);
        assert_eq!(back.entries[0].cache_size_bytes, Some(1_024));
        assert_eq!(back.entries[0].scope_state, Some(ScopeState::Fresh));
        match &back.entries[1].scope_state {
            Some(ScopeState::ScopeMismatch { expected, actual }) => {
                assert_eq!(expected, &vec!["/home/foo/baz".to_string()]);
                assert_eq!(actual, &vec!["/home/foo/baz/sub".to_string()]);
            }
            other => panic!("expected ScopeMismatch, got {other:?}"),
        }
        assert_eq!(
            back.entries[1].fix_command.as_deref(),
            Some("loct doctor --fix --project /home/foo/baz")
        );
    }

    #[test]
    fn cache_size_warning_triggers_above_threshold_only() {
        let mut report = DoctorReport {
            schema_version: "1.1".to_string(),
            generated_at: "2026-04-25T12:00:00Z".to_string(),
            entries: vec![CacheEntry {
                project_id: "abc123def4567890".to_string(),
                canonical_root: Some("/home/foo/bar".to_string()),
                last_scan_branch: None,
                last_scan_commit: None,
                last_scan_mtime: None,
                cache_size_bytes: Some(CACHE_WARNING_THRESHOLD_BYTES),
                scope_state: None,
                fix_command: None,
            }],
        };

        assert!(cache_size_warning(&report).is_none());

        report.entries[0].cache_size_bytes = Some(CACHE_WARNING_THRESHOLD_BYTES + 1);
        let warning = cache_size_warning(&report).expect("warning above threshold");
        assert!(warning.contains("above the 5GB warning threshold"));
    }

    #[test]
    #[serial]
    fn run_fix_on_fresh_entry_is_noop() {
        let cache = TempDir::new().expect("create temp cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, cache.path());

        let project = TempDir::new().expect("create temp project");
        let canonical_root = project.path().canonicalize().expect("canonicalize");
        let project_id = project_id_for(&canonical_root);

        // Drop a flat-fallback snapshot in the cache. If --fix wrongly purged
        // it, the file would disappear; we assert it survives.
        let flat_dir = cache.path().join("projects").join(&project_id);
        std::fs::create_dir_all(&flat_dir).expect("create flat fallback dir");
        let flat_snapshot = flat_dir.join("snapshot.json");
        std::fs::write(&flat_snapshot, "{}").expect("write flat snapshot");

        let report = DoctorReport {
            schema_version: "1.0".to_string(),
            generated_at: "now".to_string(),
            entries: vec![entry_with_state(
                &project_id,
                &canonical_root,
                ScopeState::Fresh,
            )],
        };
        let opts = DoctorOptions {
            yes: true,
            ..DoctorOptions::default()
        };

        run_fix(&report, &opts).expect("run_fix Fresh should be a no-op");
        assert!(
            flat_snapshot.exists(),
            "Fresh entry must NOT delete the flat fallback"
        );
    }

    #[test]
    #[serial]
    fn run_fix_on_scope_mismatch_purges_only_flat_fallback() {
        let cache = TempDir::new().expect("create temp cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, cache.path());

        let project = TempDir::new().expect("create temp project");
        let canonical_root = project.path().canonicalize().expect("canonicalize");
        let project_id = project_id_for(&canonical_root);

        let cache_dir = cache.path().join("projects").join(&project_id);
        std::fs::create_dir_all(&cache_dir).expect("create cache dir");
        // Flat fallback (the contamination we purge).
        let flat_snapshot = cache_dir.join("snapshot.json");
        std::fs::write(&flat_snapshot, "{}").expect("write flat snapshot");
        // Per-scan history (must remain after --fix).
        let scan_dir = cache_dir.join("develop@9d563ff");
        std::fs::create_dir_all(&scan_dir).expect("create scan dir");
        let scan_snapshot = scan_dir.join("snapshot.json");
        std::fs::write(&scan_snapshot, "{}").expect("write scan snapshot");

        let report = DoctorReport {
            schema_version: "1.0".to_string(),
            generated_at: "now".to_string(),
            entries: vec![entry_with_state(
                &project_id,
                &canonical_root,
                ScopeState::ScopeMismatch {
                    expected: vec![canonical_root.display().to_string()],
                    actual: vec![canonical_root.join("sub").display().to_string()],
                },
            )],
        };
        let opts = DoctorOptions {
            yes: true,
            ..DoctorOptions::default()
        };

        run_fix(&report, &opts).expect("run_fix ScopeMismatch should purge flat fallback");

        assert!(
            !flat_snapshot.exists(),
            "ScopeMismatch entry must purge the flat fallback"
        );
        assert!(
            scan_snapshot.exists(),
            "per-scan history must NOT be touched by --fix"
        );
    }

    #[test]
    #[serial]
    fn run_fix_non_interactive_without_yes_returns_needs_tty() {
        let cache = TempDir::new().expect("create temp cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, cache.path());

        let project = TempDir::new().expect("create temp project");
        let canonical_root = project.path().canonicalize().expect("canonicalize");
        let project_id = project_id_for(&canonical_root);

        let report = DoctorReport {
            schema_version: "1.0".to_string(),
            generated_at: "now".to_string(),
            entries: vec![entry_with_state(
                &project_id,
                &canonical_root,
                ScopeState::ScopeMismatch {
                    expected: vec![canonical_root.display().to_string()],
                    actual: vec![canonical_root.join("sub").display().to_string()],
                },
            )],
        };
        // Test runners run with stdin not attached to a TTY. With
        // `yes: false`, `run_fix` must refuse rather than silently purge.
        let opts = DoctorOptions {
            yes: false,
            ..DoctorOptions::default()
        };

        let outcome = run_fix(&report, &opts);
        assert!(
            matches!(outcome, Err(FixError::NeedsTty)),
            "non-interactive --fix without --yes must return NeedsTty, got: {outcome:?}"
        );
    }

    #[test]
    #[serial]
    fn run_fix_no_mismatches_is_noop_even_with_yes() {
        let cache = TempDir::new().expect("create temp cache");
        let _guard = EnvVarGuard::set_path(CACHE_ENV, cache.path());

        let report = DoctorReport {
            schema_version: "1.0".to_string(),
            generated_at: "now".to_string(),
            entries: Vec::new(),
        };
        let opts = DoctorOptions {
            yes: true,
            ..DoctorOptions::default()
        };

        run_fix(&report, &opts).expect("empty report --fix is a no-op");
    }

    #[test]
    fn scope_hint_covers_non_fresh_states() {
        assert!(scope_hint(None).is_none());
        assert!(scope_hint(Some(&ScopeState::Fresh)).is_none());

        let stale = scope_hint(Some(&ScopeState::StaleCommit)).expect("stale hint");
        assert!(stale.contains("loct scan --fresh"));

        let dirty = scope_hint(Some(&ScopeState::DirtyWorktree)).expect("dirty hint");
        assert!(dirty.contains("loct scan --fresh"));

        let mismatch = scope_hint(Some(&ScopeState::ScopeMismatch {
            expected: vec![],
            actual: vec![],
        }))
        .expect("mismatch hint");
        assert!(mismatch.contains("loct doctor --fix"));

        let corrupt =
            scope_hint(Some(&ScopeState::Corrupt("bad json".into()))).expect("corrupt hint");
        assert!(corrupt.contains("bad json"));

        let not_found = scope_hint(Some(&ScopeState::NotFound)).expect("not-found hint");
        assert!(not_found.contains("loct scan"));
    }

    // ------------------------------------------------------------------
    // L4-A: per-project default mode
    // ------------------------------------------------------------------

    #[test]
    fn needs_default_mode_detects_no_flag_invocation() {
        let opts = DoctorOptions::default();
        let global = GlobalOptions::default();
        assert!(needs_default_mode(&opts, &global));
    }

    #[test]
    fn needs_default_mode_respects_explicit_list() {
        let opts = DoctorOptions {
            list: true,
            ..DoctorOptions::default()
        };
        let global = GlobalOptions::default();
        assert!(
            !needs_default_mode(&opts, &global),
            "--list must skip default-mode injection so behavior stays the historical global list"
        );
    }

    #[test]
    fn needs_default_mode_respects_explicit_scope() {
        let opts = DoctorOptions {
            scope: true,
            ..DoctorOptions::default()
        };
        let global = GlobalOptions::default();
        assert!(!needs_default_mode(&opts, &global));
    }

    #[test]
    fn needs_default_mode_respects_project_argument() {
        let opts = DoctorOptions {
            project: Some(PathBuf::from("/tmp/foo")),
            ..DoctorOptions::default()
        };
        let global = GlobalOptions::default();
        assert!(
            !needs_default_mode(&opts, &global),
            "--project alone is an explicit intent; do not override it"
        );
    }

    #[test]
    fn needs_default_mode_respects_global_json() {
        let opts = DoctorOptions::default();
        let global = GlobalOptions {
            json: true,
            ..GlobalOptions::default()
        };
        assert!(
            !needs_default_mode(&opts, &global),
            "global --json is a machine-mode invocation; keep the historical empty list rather than \
             implicitly switching modes"
        );
    }

    #[test]
    #[serial]
    fn infer_default_mode_per_project_when_snapshot_present() {
        let project = TempDir::new().expect("create temp project");
        let canonical_root = project
            .path()
            .canonicalize()
            .expect("canonicalize project root");
        std::fs::create_dir_all(canonical_root.join(".loctree"))
            .expect("create .loctree dir for find_loctree_root marker");

        // Run from inside the project so std::env::current_dir() points there.
        let original_cwd = std::env::current_dir().expect("current dir");
        std::env::set_current_dir(&canonical_root).expect("cd into temp project");
        let result = infer_default_mode(&DoctorOptions::default());
        std::env::set_current_dir(original_cwd).expect("restore cwd");

        match result {
            DefaultMode::PerProject(root) => {
                let canon = root.canonicalize().unwrap_or(root);
                assert_eq!(
                    canon, canonical_root,
                    "PerProject root must point at the snapshot-owning directory"
                );
            }
            DefaultMode::GlobalList { hint } => panic!(
                "expected PerProject for a directory with .loctree; got GlobalList(hint={hint:?})"
            ),
        }
    }

    #[test]
    #[serial]
    fn infer_default_mode_global_list_when_no_snapshot_nearby() {
        let scratch = TempDir::new().expect("create scratch dir");
        let scratch_root = scratch
            .path()
            .canonicalize()
            .expect("canonicalize scratch dir");

        let original_cwd = std::env::current_dir().expect("current dir");
        std::env::set_current_dir(&scratch_root).expect("cd into scratch dir");
        let result = infer_default_mode(&DoctorOptions::default());
        std::env::set_current_dir(original_cwd).expect("restore cwd");

        match result {
            DefaultMode::GlobalList { hint } => {
                let hint = hint.expect("GlobalList without snapshot should emit a hint");
                assert!(
                    hint.contains("loct scan") && hint.contains("--list"),
                    "hint should mention both remediations; got: {hint}"
                );
            }
            DefaultMode::PerProject(root) => panic!(
                "expected GlobalList in a directory without .loctree; got PerProject({})",
                root.display()
            ),
        }
    }
}
