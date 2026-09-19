//! Handler for `loct cache` commands (list, clean).

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

use crate::cli::command::{CacheAction, CacheOptions};
use crate::snapshot::{
    Snapshot, SnapshotCacheLock, SnapshotCacheLockError, SnapshotMetadata, cache_base_dir,
    project_cache_dir, try_acquire_snapshot_cache_lock,
    try_acquire_snapshot_cache_lock_for_bucket_id,
};
use time::{OffsetDateTime, format_description::well_known::Iso8601};

use super::super::DispatchResult;
use super::resource_limits::{
    CACHE_ENUM_TIME_CEILING, DirSizeSample, EnumerationBudget, MAX_LIST_ROWS,
    MAX_SNAPSHOT_METADATA_PARSE_BYTES, MAX_WALK_ENTRIES_PER_BUCKET, SnapshotMetadataRead,
    bounded_dir_size, read_snapshot_metadata_bounded,
};

pub fn handle_cache_command(opts: &CacheOptions) -> DispatchResult {
    match &opts.action {
        CacheAction::List { all, project } => handle_list(*all, project.as_deref()),
        CacheAction::Clean {
            all,
            project,
            older_than,
            max_size,
            force,
        } => handle_clean(
            *all,
            project.as_deref(),
            older_than.as_deref(),
            max_size.as_deref(),
            *force,
        ),
    }
}

/// `loct cache list` is project-local by default (audit class H, LCT-E01):
/// the global inventory walks every bucket and reads every snapshot's
/// metadata, which is exactly the enumeration that blew past resource
/// ceilings. The global walk stays available behind explicit `--all`.
fn handle_list(all: bool, project: Option<&Path>) -> DispatchResult {
    let base = cache_base_dir();
    let projects_dir = base.join("projects");

    if !projects_dir.exists() {
        println!("No cached projects found.");
        println!("Cache dir: {}", base.display());
        return DispatchResult::Exit(0);
    }

    if all {
        return handle_list_global(&projects_dir);
    }

    let Some(project_root) = resolve_list_project_root(project) else {
        let global_bucket_count = count_cache_buckets(&projects_dir);
        println!(
            "Not inside a scanned project. `loct cache list` is project-local by default;\n\
             pass --project <DIR> or opt in to the global inventory with `loct cache list --all`."
        );
        println!(
            "Global cache holds {}{} bucket(s) at {}.",
            if global_bucket_count.incomplete {
                "≥"
            } else {
                ""
            },
            global_bucket_count.count,
            projects_dir.display()
        );
        return DispatchResult::Exit(0);
    };

    let bucket_dir = project_cache_dir(&project_root);
    let bucket_id = bucket_dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let global_bucket_count = count_cache_buckets(&projects_dir);
    let other_buckets = global_bucket_count.count.saturating_sub(1);

    println!("Cache scope: project-local ({})", project_root.display());

    if !bucket_dir.is_dir() {
        println!(
            "No cache found for project {} (bucket {}).",
            project_root.display(),
            bucket_id
        );
        println!(
            "Global cache holds {}{} bucket(s) — use `loct cache list --all` for the full inventory.",
            if global_bucket_count.incomplete {
                "≥"
            } else {
                ""
            },
            global_bucket_count.count
        );
        return DispatchResult::Exit(0);
    }

    let budget = EnumerationBudget::start(CACHE_ENUM_TIME_CEILING);
    let row = collect_cache_bucket_row(&bucket_id, &bucket_dir, Some(&budget));

    println!("Cache: {}", projects_dir.display());
    println!();
    println!("Org/Repo | Path | Cache size MB | Meta");
    println!("--- | --- | --- | ---");
    print_bucket_row(&row);
    println!();
    println!(
        "{}{} other bucket(s) in the global cache — use `loct cache list --all` for the full inventory.",
        if global_bucket_count.incomplete {
            "≥"
        } else {
            ""
        },
        other_buckets
    );

    DispatchResult::Exit(0)
}

/// Bounded global inventory (explicit `--all` opt-in): per-bucket walks are
/// entry-capped, snapshot metadata is stream-parsed behind a size ceiling,
/// the whole pass carries a wall-clock budget, and output is row-capped.
/// Every truncation is reported, never silent.
fn handle_list_global(projects_dir: &Path) -> DispatchResult {
    let budget = EnumerationBudget::start(CACHE_ENUM_TIME_CEILING);
    let entries = match fs::read_dir(projects_dir) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("Failed to read cache directory: {}", err);
            return DispatchResult::Exit(1);
        }
    };

    let mut total_size: u64 = 0;
    let mut measured_bucket_count: usize = 0;
    let mut any_size_lower_bound = false;
    let mut rows: Vec<CacheBucketRow> = Vec::with_capacity(MAX_LIST_ROWS);

    let inventory = visit_cache_bucket_entries(entries, Some(&budget), |entry| {
        let path = entry.path();
        let bucket_id = entry.file_name().to_string_lossy().to_string();
        let row = collect_cache_bucket_row(&bucket_id, &path, Some(&budget));
        measured_bucket_count = measured_bucket_count.saturating_add(1);
        total_size = total_size.saturating_add(row.size_bytes);
        any_size_lower_bound |= row.truncated;
        retain_cache_bucket_row(&mut rows, row, MAX_LIST_ROWS);
    });

    rows.sort_by(cache_bucket_row_order);

    if measured_bucket_count == 0 && !inventory.time_ceiling_hit && inventory.errors == 0 {
        println!("No cached projects found.");
        println!("Cache dir: {}", projects_dir.display());
        return DispatchResult::Exit(0);
    }

    println!("Cache: {}", projects_dir.display());
    println!();
    println!("Org/Repo | Path | Cache size MB | Meta");
    println!("--- | --- | --- | ---");

    let omitted_rows = measured_bucket_count.saturating_sub(rows.len());
    for row in &rows {
        print_bucket_row(row);
    }

    println!();
    let bucket_count_is_lower_bound = inventory.errors > 0 || inventory.time_ceiling_hit;
    let total_size_is_lower_bound = bucket_count_is_lower_bound || any_size_lower_bound;
    println!(
        "{}{} cache bucket(s), {}{:.2} MB total",
        if bucket_count_is_lower_bound {
            "≥"
        } else {
            ""
        },
        measured_bucket_count,
        if total_size_is_lower_bound { "≥" } else { "" },
        size_in_mb(total_size),
    );
    if omitted_rows > 0 {
        println!(
            "note: output truncated to {} rows ({} measured row(s) omitted) — narrow with --project <DIR>.",
            MAX_LIST_ROWS, omitted_rows
        );
    }
    if inventory.time_ceiling_hit {
        println!(
            "note: enumeration stopped at the {}s time ceiling; remaining cache entries were not inspected — narrow with --project <DIR>.",
            budget.ceiling_secs()
        );
    }
    if inventory.errors > 0 {
        println!(
            "note: cache bucket enumeration encountered {} filesystem/metadata error(s); bucket count and total size are lower bounds.",
            inventory.errors
        );
    }

    DispatchResult::Exit(0)
}

fn cache_bucket_row_order(a: &CacheBucketRow, b: &CacheBucketRow) -> Ordering {
    b.size_bytes
        .cmp(&a.size_bytes)
        .then_with(|| a.org_repo.cmp(&b.org_repo))
        .then_with(|| a.project_path.cmp(&b.project_path))
}

fn retain_cache_bucket_row(rows: &mut Vec<CacheBucketRow>, row: CacheBucketRow, limit: usize) {
    if limit == 0 {
        return;
    }
    if rows.len() < limit {
        rows.push(row);
        return;
    }

    let worst_index = rows
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| cache_bucket_row_order(a, b))
        .map(|(index, _)| index)
        .expect("a full bounded row set is non-empty");
    if cache_bucket_row_order(&row, &rows[worst_index]).is_lt() {
        rows[worst_index] = row;
    }
}

fn print_bucket_row(row: &CacheBucketRow) {
    println!(
        "{} | {} | {}{:.2} | {}",
        row.org_repo,
        row.project_path,
        if row.truncated { "≥" } else { "" },
        size_in_mb(row.size_bytes),
        row.meta,
    );
}

/// Resolve the project root a project-local `cache list` should target:
/// the explicit `--project` argument (walked up to its loctree root when one
/// exists) or the loctree root above the current working directory.
fn resolve_list_project_root(project: Option<&Path>) -> Option<std::path::PathBuf> {
    match project {
        Some(path) => {
            let absolute = if path.is_relative() {
                std::env::current_dir().unwrap_or_default().join(path)
            } else {
                path.to_path_buf()
            };
            Some(Snapshot::find_loctree_root(&absolute).unwrap_or(absolute))
        }
        None => {
            let cwd = std::env::current_dir().ok()?;
            Snapshot::find_loctree_root(&cwd)
        }
    }
}

/// Cheap bucket count: one `read_dir`, no per-bucket walks.
#[derive(Debug, Default, Clone, Copy)]
struct CacheBucketVisitSummary {
    errors: usize,
    time_ceiling_hit: bool,
}

fn visit_cache_bucket_entries(
    entries: impl IntoIterator<Item = std::io::Result<std::fs::DirEntry>>,
    budget: Option<&EnumerationBudget>,
    mut visit: impl FnMut(std::fs::DirEntry),
) -> CacheBucketVisitSummary {
    let mut summary = CacheBucketVisitSummary::default();

    for entry in entries {
        if budget.is_some_and(EnumerationBudget::exceeded) {
            summary.time_ceiling_hit = true;
            break;
        }

        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                summary.errors += 1;
                continue;
            }
        };
        let metadata = match fs::metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(_) => {
                summary.errors += 1;
                continue;
            }
        };
        if !metadata.is_dir() {
            continue;
        }
        if budget.is_some_and(EnumerationBudget::exceeded) {
            summary.time_ceiling_hit = true;
            break;
        }

        visit(entry);
    }

    summary
}

#[derive(Debug)]
struct CacheBucketEnumeration {
    entries: Vec<std::fs::DirEntry>,
    errors: usize,
}

impl CacheBucketEnumeration {
    fn into_complete(self) -> Result<Vec<std::fs::DirEntry>, usize> {
        if self.errors == 0 {
            Ok(self.entries)
        } else {
            Err(self.errors)
        }
    }
}

fn collect_cache_bucket_entries(
    entries: impl IntoIterator<Item = std::io::Result<std::fs::DirEntry>>,
) -> CacheBucketEnumeration {
    let mut buckets = Vec::new();
    let summary = visit_cache_bucket_entries(entries, None, |entry| buckets.push(entry));

    CacheBucketEnumeration {
        entries: buckets,
        errors: summary.errors,
    }
}

/// Enumerate `<cache base>/projects` through the validated-root gate: the
/// directory must canonicalize underneath `cache_base`, so a symlinked or
/// relocated cache can never steer the walk outside Loctree's own cache.
fn read_cache_bucket_entries(cache_base: &Path) -> std::io::Result<CacheBucketEnumeration> {
    let projects_dir = cache_base.join("projects");
    Ok(collect_cache_bucket_entries(
        crate::fs_utils::read_dir_within(cache_base, &projects_dir)?,
    ))
}

#[derive(Debug, Clone, Copy)]
struct CacheBucketCount {
    count: usize,
    incomplete: bool,
}

fn count_cache_buckets(projects_dir: &Path) -> CacheBucketCount {
    let budget = EnumerationBudget::start(CACHE_ENUM_TIME_CEILING);
    let entries = match fs::read_dir(projects_dir) {
        Ok(entries) => entries,
        Err(_) => {
            return CacheBucketCount {
                count: 0,
                incomplete: true,
            };
        }
    };
    let mut count: usize = 0;
    let summary =
        visit_cache_bucket_entries(entries, Some(&budget), |_| count = count.saturating_add(1));

    CacheBucketCount {
        count,
        incomplete: summary.errors > 0 || summary.time_ceiling_hit,
    }
}

fn handle_clean(
    all: bool,
    project: Option<&std::path::Path>,
    older_than: Option<&str>,
    max_size: Option<&str>,
    force: bool,
) -> DispatchResult {
    let max_age_secs = match older_than {
        Some(raw) => match parse_duration_days(raw) {
            Some(seconds) => Some(seconds),
            None => {
                eprintln!(
                    "Failed to parse --older-than '{}': expected a whole number of days, e.g. 7d or 30d.",
                    raw
                );
                return DispatchResult::Exit(2);
            }
        },
        None => None,
    };

    let size_budget = match max_size {
        Some(raw) => match parse_size_budget(raw) {
            Some(bytes) => Some(bytes),
            None => {
                eprintln!(
                    "Failed to parse --max-size '{}': expected e.g. 1GB, 500MB, 250M, or plain bytes.",
                    raw
                );
                return DispatchResult::Exit(2);
            }
        },
        None => None,
    };

    let base = cache_base_dir();
    let projects_dir = base.join("projects");

    // `--project` is an address. Resolve it before the empty-cache shortcut so
    // an unknown/empty selector cannot report "Nothing to clean" (exit 0) or
    // fall through to cwd.
    if let Some(proj) = project {
        return handle_clean_project(proj, &projects_dir, force);
    }

    if !projects_dir.exists() {
        println!("Nothing to clean.");
        return DispatchResult::Exit(0);
    }

    let enumeration = match read_cache_bucket_entries(&base) {
        Ok(enumeration) => enumeration,
        Err(err) => {
            eprintln!(
                "Cannot safely enumerate cache buckets in {}: {}.",
                projects_dir.display(),
                err
            );
            return DispatchResult::Exit(2);
        }
    };
    let entries = match enumeration.into_complete() {
        Ok(entries) => entries,
        Err(errors) => {
            eprintln!(
                "Cannot safely enumerate cache buckets: encountered {} filesystem/metadata error(s). No cache entries were removed.",
                errors
            );
            return DispatchResult::Exit(2);
        }
    };

    if entries.is_empty() {
        println!("Nothing to clean.");
        return DispatchResult::Exit(0);
    }

    // Lock every bucket in deterministic path order before recency/size
    // measurement and hold the guards through preview or deletion. Without
    // this, a concurrent Snapshot::save can rewrite a bucket after mtime/size
    // were sampled and make a successful --max-size report false.
    let _bucket_locks = match try_lock_global_cache_buckets(&entries) {
        Ok(locks) => locks,
        Err((bucket_id, err)) => {
            eprintln!(
                "Cannot safely clean cache: failed to lock bucket '{}': {}. No cache entries were removed.",
                bucket_id, err
            );
            return DispatchResult::Exit(2);
        }
    };

    let mut to_remove: Vec<(std::path::PathBuf, DirSizeSample)> = Vec::new();

    for entry in &entries {
        let path = entry.path();

        if all {
            to_remove.push((path.clone(), dir_size(&path)));
        } else if let Some(max_secs) = max_age_secs {
            let age_secs = path
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| SystemTime::now().duration_since(t).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            if age_secs >= max_secs {
                to_remove.push((path.clone(), dir_size(&path)));
            }
        }
    }

    // Apply size-budget eviction: keep newest buckets up to budget, evict
    // the rest (oldest first). This is additive with --older-than: items
    // already on the removal list stay there; remaining (newer) buckets
    // are evaluated against the budget.
    if let Some(budget) = size_budget {
        let kept_entries: Vec<_> = entries
            .iter()
            .filter(|e| !to_remove.iter().any(|(p, _)| p == &e.path()))
            .collect();

        let extra = match evict_to_budget(&kept_entries, budget) {
            Ok(extra) => extra,
            Err(path) => {
                eprintln!(
                    "Cannot safely apply --max-size: bucket measurement was incomplete for {} \
                     (entry/time ceiling or filesystem/traversal/metadata error). Narrow the \
                     cleanup with --project or --older-than.",
                    path.display()
                );
                return DispatchResult::Exit(2);
            }
        };
        for (path, size) in extra {
            // Avoid double-counting if --older-than already nominated it.
            if !to_remove.iter().any(|(p, _)| p == &path) {
                to_remove.push((path, size));
            }
        }
    }

    if to_remove.is_empty() {
        println!("Nothing to clean (no entries match criteria).");
        return DispatchResult::Exit(0);
    }

    let total_size = summarize_sizes(&to_remove);

    if !force {
        eprintln!(
            "Will remove {} project(s) ({}).",
            to_remove.len(),
            format_size_sample(total_size)
        );
        eprintln!("Use --force to skip this confirmation.");
        return DispatchResult::Exit(1);
    }

    let summary = remove_candidates(&to_remove);

    println!(
        "Cleaned {} project(s), freed {}.",
        summary.removed,
        format_size_sample(summary.freed)
    );

    if summary.failed == 0 {
        DispatchResult::Exit(0)
    } else {
        eprintln!("Failed to remove {} project(s).", summary.failed);
        DispatchResult::Exit(1)
    }
}

fn is_blank_project_selector(proj: &Path) -> bool {
    proj.as_os_str().is_empty() || proj.to_string_lossy().trim().is_empty()
}

fn known_cache_bucket_ids(projects_dir: &Path) -> Vec<String> {
    let entries = match fs::read_dir(projects_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut ids = Vec::new();
    visit_cache_bucket_entries(entries, None, |entry| {
        ids.push(entry.file_name().to_string_lossy().into_owned());
    });
    ids.sort();
    ids
}

fn format_unknown_project_refusal(requested: &Path, known_buckets: &[String]) -> String {
    let mut msg = format!(
        "Refusing cache clean: no cache bucket for project '{}'.\nNo cache entries were removed.",
        requested.display()
    );
    if known_buckets.is_empty() {
        msg.push_str("\nKnown cache buckets: (none)");
    } else {
        msg.push_str("\nKnown cache buckets:");
        for id in known_buckets {
            msg.push_str("\n  ");
            msg.push_str(id);
        }
    }
    msg
}

fn handle_clean_project(proj: &Path, projects_dir: &Path, force: bool) -> DispatchResult {
    if is_blank_project_selector(proj) {
        eprintln!(
            "Refusing empty --project. Pass an explicit project directory; an empty string is not cwd."
        );
        eprintln!("No cache entries were removed.");
        return DispatchResult::Exit(2);
    }

    let proj_path = if proj.is_relative() {
        std::env::current_dir().unwrap_or_default().join(proj)
    } else {
        proj.to_path_buf()
    };
    let cache_dir = project_cache_dir(&proj_path);
    if !cache_dir.exists() {
        eprintln!(
            "{}",
            format_unknown_project_refusal(&proj_path, &known_cache_bucket_ids(projects_dir))
        );
        return DispatchResult::Exit(2);
    }
    // Hold the same exclusive lock Snapshot::save/load use so a concurrent
    // writer cannot mutate the bucket between measurement and deletion.
    let _cache_lock = match try_acquire_snapshot_cache_lock(&proj_path) {
        Ok(lock) => lock,
        Err(err) => {
            eprintln!(
                "Cannot safely clean cache for {}: {}. No cache entries were removed.",
                proj_path.display(),
                err
            );
            return DispatchResult::Exit(2);
        }
    };
    let size = dir_size(&cache_dir);
    if !force {
        eprintln!(
            "Will remove cache for {} ({}).",
            proj_path.display(),
            format_size_sample(size)
        );
        eprintln!("Use --force to skip this confirmation.");
        return DispatchResult::Exit(1);
    }
    if let Err(err) = fs::remove_dir_all(&cache_dir) {
        eprintln!("Failed to remove {}: {}", cache_dir.display(), err);
        return DispatchResult::Exit(1);
    }
    println!(
        "Removed cache for {} ({})",
        proj_path.display(),
        format_size_sample(size)
    );
    DispatchResult::Exit(0)
}

/// Acquire non-blocking exclusive locks for every global cache bucket, in
/// deterministic path order. On the first failure, previously acquired locks
/// are released automatically when the local `Vec` is dropped.
fn try_lock_global_cache_buckets(
    entries: &[std::fs::DirEntry],
) -> Result<Vec<SnapshotCacheLock>, (String, SnapshotCacheLockError)> {
    let mut ordered: Vec<&std::fs::DirEntry> = entries.iter().collect();
    ordered.sort_by_key(|entry| entry.path());

    let mut locks = Vec::with_capacity(ordered.len());
    for entry in ordered {
        let bucket_id = entry.file_name().to_string_lossy().into_owned();
        match try_acquire_snapshot_cache_lock_for_bucket_id(&bucket_id) {
            Ok(lock) => locks.push(lock),
            Err(err) => return Err((bucket_id, err)),
        }
    }
    Ok(locks)
}

#[derive(Debug, PartialEq, Eq)]
struct CacheBucketRow {
    org_repo: String,
    project_path: String,
    size_bytes: u64,
    /// True when an entry/time ceiling or filesystem error made the size walk
    /// incomplete: `size_bytes` is a lower bound, rendered with a `≥` prefix.
    truncated: bool,
    meta: String,
}

#[derive(Clone, Debug)]
struct CacheSnapshotRecord {
    metadata: SnapshotMetadata,
    modified_at: SystemTime,
    is_latest_pointer: bool,
}

#[derive(Debug, Default)]
struct CacheBucketStats {
    size_bytes: u64,
    truncated: bool,
    snapshots: Vec<CacheSnapshotRecord>,
}

fn collect_cache_bucket_row(
    bucket_id: &str,
    bucket_dir: &Path,
    budget: Option<&EnumerationBudget>,
) -> CacheBucketRow {
    let stats = collect_cache_bucket_stats(bucket_dir, budget);
    let snapshots = effective_bucket_snapshots(&stats.snapshots);
    let project_path =
        select_project_path(&snapshots).unwrap_or_else(|| "(unknown path)".to_string());
    let org_repo = resolve_org_repo_label(&snapshots, bucket_id, &project_path);
    let meta = format_cache_meta(&snapshots);

    CacheBucketRow {
        org_repo,
        project_path,
        size_bytes: stats.size_bytes,
        truncated: stats.truncated,
        meta,
    }
}

/// Walk one bucket with an entry cap and the pass-wide time budget. Snapshot
/// metadata comes from the streaming bounded reader — never a whole-file
/// `fs::read` (audit class H root cause).
fn collect_cache_bucket_stats(
    bucket_dir: &Path,
    budget: Option<&EnumerationBudget>,
) -> CacheBucketStats {
    collect_cache_bucket_stats_entries(walkdir::WalkDir::new(bucket_dir), bucket_dir, budget)
}

fn collect_cache_bucket_stats_entries(
    entries: impl IntoIterator<Item = walkdir::Result<walkdir::DirEntry>>,
    bucket_dir: &Path,
    budget: Option<&EnumerationBudget>,
) -> CacheBucketStats {
    let mut size_bytes: u64 = 0;
    let mut walked: u64 = 0;
    let mut truncated = false;
    let mut snapshots = Vec::new();

    for entry in entries {
        walked += 1;
        if walked > MAX_WALK_ENTRIES_PER_BUCKET || budget.is_some_and(EnumerationBudget::exceeded) {
            truncated = true;
            break;
        }

        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                truncated = true;
                continue;
            }
        };
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => {
                truncated = true;
                continue;
            }
        };

        if !metadata.is_file() {
            continue;
        }

        size_bytes = size_bytes.saturating_add(metadata.len());

        if entry.file_name().to_str() != Some("snapshot.json") {
            continue;
        }

        let modified_at = match metadata.modified() {
            Ok(modified_at) => modified_at,
            Err(_) => {
                truncated = true;
                continue;
            }
        };
        if let Some(snapshot) = read_snapshot_record(entry.path(), bucket_dir, modified_at) {
            snapshots.push(snapshot);
        }
    }

    CacheBucketStats {
        size_bytes,
        truncated,
        snapshots,
    }
}

fn read_snapshot_record(
    snapshot_path: &Path,
    bucket_dir: &Path,
    modified_at: SystemTime,
) -> Option<CacheSnapshotRecord> {
    let metadata =
        match read_snapshot_metadata_bounded(snapshot_path, MAX_SNAPSHOT_METADATA_PARSE_BYTES) {
            SnapshotMetadataRead::Parsed(metadata) => *metadata,
            SnapshotMetadataRead::OversizeSkipped { .. } | SnapshotMetadataRead::Unreadable => {
                return None;
            }
        };
    let is_latest_pointer = snapshot_path
        .parent()
        .and_then(Path::file_name)
        .and_then(|segment| segment.to_str())
        == Some("latest")
        && snapshot_path
            .parent()
            .and_then(Path::parent)
            .is_some_and(|parent| parent == bucket_dir);

    Some(CacheSnapshotRecord {
        metadata,
        modified_at,
        is_latest_pointer,
    })
}

fn effective_bucket_snapshots(snapshots: &[CacheSnapshotRecord]) -> Vec<&CacheSnapshotRecord> {
    let actual: Vec<_> = snapshots
        .iter()
        .filter(|snapshot| !snapshot.is_latest_pointer)
        .collect();
    if actual.is_empty() {
        snapshots.iter().collect()
    } else {
        actual
    }
}

fn select_project_path(snapshots: &[&CacheSnapshotRecord]) -> Option<String> {
    snapshots
        .iter()
        .flat_map(|snapshot| snapshot.metadata.roots.iter())
        .map(|root| root.trim())
        .filter(|root| !root.is_empty())
        .map(str::to_string)
        .min_by(compare_root_display)
}

fn compare_root_display(left: &String, right: &String) -> Ordering {
    path_depth(left)
        .cmp(&path_depth(right))
        .then_with(|| left.len().cmp(&right.len()))
        .then_with(|| left.cmp(right))
}

fn path_depth(path: &str) -> usize {
    Path::new(path).components().count()
}

fn resolve_org_repo_label(
    snapshots: &[&CacheSnapshotRecord],
    bucket_id: &str,
    project_path: &str,
) -> String {
    snapshots
        .iter()
        .filter_map(|snapshot| option_str(&snapshot.metadata.git_owner_repo))
        .max_by(|left, right| compare_option_str(left, right))
        .map(str::to_string)
        .or_else(|| {
            snapshots
                .iter()
                .filter_map(|snapshot| option_str(&snapshot.metadata.git_repo))
                .max_by(|left, right| compare_option_str(left, right))
                .map(|repo| format!("unknown/{repo}"))
        })
        .or_else(|| fallback_local_org_repo(project_path))
        .unwrap_or_else(|| format!("unknown/{bucket_id}"))
}

fn fallback_local_org_repo(project_path: &str) -> Option<String> {
    if project_path == "(unknown path)" {
        return None;
    }

    let repo_name = Path::new(project_path)
        .file_name()
        .and_then(|segment| segment.to_str())
        .map(str::trim)
        .filter(|segment| !segment.is_empty())?;

    Some(format!("local/{repo_name}"))
}

fn format_cache_meta(snapshots: &[&CacheSnapshotRecord]) -> String {
    if snapshots.is_empty() {
        return "scans 0; latest unknown; schema unknown".to_string();
    }

    let root_count = distinct_non_empty_values(
        snapshots
            .iter()
            .flat_map(|snapshot| snapshot.metadata.roots.iter())
            .map(|root| root.as_str()),
    )
    .len();
    let branch_count = distinct_non_empty_values(
        snapshots
            .iter()
            .filter_map(|snapshot| option_str(&snapshot.metadata.git_branch)),
    )
    .len();
    let schemas = distinct_non_empty_values(
        snapshots
            .iter()
            .filter_map(|snapshot| non_empty_str(snapshot.metadata.schema_version.as_str())),
    );
    let latest = snapshots
        .iter()
        .copied()
        .max_by(|a, b| compare_snapshot_records(a, b))
        .expect("snapshots is non-empty");

    let mut parts = vec![format!("scans {}", snapshots.len())];
    if root_count > 1 {
        parts.push(format!("roots {root_count}"));
    }
    if branch_count > 1 {
        parts.push(format!("branches {branch_count}"));
    }
    parts.push(format!("latest {}", latest_timestamp(latest)));
    if let Some(reference) = format_git_reference(latest) {
        parts.push(format!("ref {reference}"));
    }
    parts.push(format_schema_meta(&schemas, latest));

    parts.join("; ")
}

fn distinct_non_empty_values<'a>(values: impl IntoIterator<Item = &'a str>) -> BTreeSet<&'a str> {
    values
        .into_iter()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect()
}

fn compare_snapshot_records(left: &CacheSnapshotRecord, right: &CacheSnapshotRecord) -> Ordering {
    left.modified_at
        .cmp(&right.modified_at)
        .then_with(|| {
            non_empty_str(left.metadata.generated_at.as_str())
                .cmp(&non_empty_str(right.metadata.generated_at.as_str()))
        })
        .then_with(|| {
            option_str(&left.metadata.git_scan_id).cmp(&option_str(&right.metadata.git_scan_id))
        })
        .then_with(|| {
            select_first_root(left.metadata.roots.as_slice())
                .cmp(&select_first_root(right.metadata.roots.as_slice()))
        })
}

fn select_first_root(roots: &[String]) -> Option<&str> {
    roots
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .find(|root| !root.is_empty())
}

fn latest_timestamp(snapshot: &CacheSnapshotRecord) -> String {
    non_empty_str(snapshot.metadata.generated_at.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format_system_time(snapshot.modified_at))
}

fn format_system_time(timestamp: SystemTime) -> String {
    OffsetDateTime::from(timestamp)
        .format(&Iso8601::DEFAULT)
        .unwrap_or_else(|_| "unknown".to_string())
}

fn format_git_reference(snapshot: &CacheSnapshotRecord) -> Option<String> {
    match (
        option_str(&snapshot.metadata.git_branch),
        option_str(&snapshot.metadata.git_commit),
    ) {
        (Some(branch), Some(commit)) => Some(format!("{branch}@{commit}")),
        (Some(branch), None) => Some(branch.to_string()),
        (None, Some(commit)) => Some(commit.to_string()),
        (None, None) => None,
    }
}

fn format_schema_meta(schemas: &BTreeSet<&str>, latest_snapshot: &CacheSnapshotRecord) -> String {
    match schemas.len() {
        0 => "schema unknown".to_string(),
        1 => format!("schema {}", schemas.iter().next().expect("single schema")),
        count => {
            let latest_schema = non_empty_str(latest_snapshot.metadata.schema_version.as_str())
                .unwrap_or("unknown");
            format!("schema {latest_schema} (+{} more)", count - 1)
        }
    }
}

fn non_empty_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn option_str(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn compare_option_str(left: &str, right: &str) -> Ordering {
    path_depth(left)
        .cmp(&path_depth(right))
        .then_with(|| left.len().cmp(&right.len()))
        .then_with(|| left.cmp(right))
}

/// Calculate the size of a directory with an explicit lower-bound marker.
fn dir_size(path: &std::path::Path) -> DirSizeSample {
    bounded_dir_size(path, MAX_WALK_ENTRIES_PER_BUCKET, None)
}

fn size_in_mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn format_size_sample(sample: DirSizeSample) -> String {
    format!(
        "{}{}",
        if sample.truncated { "≥" } else { "" },
        format_size(sample.bytes)
    )
}

fn summarize_sizes(candidates: &[(std::path::PathBuf, DirSizeSample)]) -> DirSizeSample {
    candidates.iter().fold(
        DirSizeSample {
            bytes: 0,
            truncated: false,
        },
        |summary, (_, sample)| DirSizeSample {
            bytes: summary.bytes.saturating_add(sample.bytes),
            truncated: summary.truncated || sample.truncated,
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemovalSummary {
    removed: usize,
    failed: usize,
    freed: DirSizeSample,
}

fn remove_candidates(candidates: &[(std::path::PathBuf, DirSizeSample)]) -> RemovalSummary {
    let mut summary = RemovalSummary {
        removed: 0,
        failed: 0,
        freed: DirSizeSample {
            bytes: 0,
            truncated: false,
        },
    };

    for (path, size) in candidates {
        if let Err(err) = fs::remove_dir_all(path) {
            summary.failed += 1;
            eprintln!("Failed to remove {}: {}", path.display(), err);
        } else {
            summary.removed += 1;
            summary.freed.bytes = summary.freed.bytes.saturating_add(size.bytes);
            summary.freed.truncated |= size.truncated;
            if let Some(name) = path.file_name() {
                eprintln!(
                    "  removed {} ({})",
                    name.to_string_lossy(),
                    format_size_sample(*size)
                );
            }
        }
    }

    summary
}

/// Parse "7d" or "30d" into seconds.
fn parse_duration_days(s: &str) -> Option<u64> {
    let trimmed = s.trim().to_lowercase();
    let days = if let Some(days_str) = trimmed.strip_suffix('d') {
        days_str.parse::<u64>().ok()?
    } else {
        // Try plain number as days
        trimmed.parse::<u64>().ok()?
    };
    days.checked_mul(86400)
}

/// Parse a size budget like `1GB`, `500MB`, `250M`, `123456` into bytes.
///
/// Accepts decimal multipliers (1 GB = 1_000_000_000) because operators
/// reading vendor docs (Apple, OS reports) overwhelmingly speak in
/// SI-prefixed sizes. Internally cache sizes are byte counts so the SI
/// definition is preferred over the binary one — the user wrote `1GB`
/// because they want ~1 billion bytes, not 1_073_741_824. Whitespace
/// is stripped; case is folded.
///
/// Source hak: 2026-05-23 div0 system-cleanup (~16.6 GB cache without
/// retention policy; operator could not free disk). See loctree-fail.md.
fn parse_size_budget(s: &str) -> Option<u64> {
    let trimmed: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_lowercase();
    let (digits, multiplier): (&str, u64) = if let Some(rest) = lower.strip_suffix("gb") {
        (rest, 1_000_000_000)
    } else if let Some(rest) = lower.strip_suffix("mb") {
        (rest, 1_000_000)
    } else if let Some(rest) = lower.strip_suffix("kb") {
        (rest, 1_000)
    } else if let Some(rest) = lower.strip_suffix('g') {
        (rest, 1_000_000_000)
    } else if let Some(rest) = lower.strip_suffix('m') {
        (rest, 1_000_000)
    } else if let Some(rest) = lower.strip_suffix('k') {
        (rest, 1_000)
    } else if let Some(rest) = lower.strip_suffix('b') {
        (rest, 1)
    } else {
        (lower.as_str(), 1)
    };

    let number: f64 = digits.parse().ok()?;
    if number < 0.0 || !number.is_finite() {
        return None;
    }
    let bytes = (number * multiplier as f64).round();
    if bytes < 0.0 || bytes > u64::MAX as f64 {
        return None;
    }
    Some(bytes as u64)
}

/// Given a list of bucket directory entries and a byte budget, return the
/// list of buckets (path, size) that must be evicted to fit. Newest buckets
/// (by mtime) are kept first; remaining buckets are evicted oldest-first.
fn evict_to_budget(
    entries: &[&std::fs::DirEntry],
    budget_bytes: u64,
) -> Result<Vec<(std::path::PathBuf, DirSizeSample)>, std::path::PathBuf> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let buckets: Result<Vec<(std::path::PathBuf, SystemTime, DirSizeSample)>, std::path::PathBuf> =
        entries
            .iter()
            .map(|entry| {
                let path = entry.path();
                let mtime = path
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .map_err(|_| path.clone())?;
                let size = dir_size(&path);
                Ok((path, mtime, size))
            })
            .collect();
    evict_measured_to_budget(buckets?, budget_bytes)
}

fn evict_measured_to_budget(
    buckets: Vec<(std::path::PathBuf, SystemTime, DirSizeSample)>,
    budget_bytes: u64,
) -> Result<Vec<(std::path::PathBuf, DirSizeSample)>, std::path::PathBuf> {
    if let Some((path, _, _)) = buckets.iter().find(|(_, _, sample)| sample.truncated) {
        return Err(path.clone());
    }

    let exact_buckets = buckets
        .iter()
        .map(|(path, mtime, sample)| (path.clone(), *mtime, sample.bytes))
        .collect();
    let evicted = evict_to_budget_core(exact_buckets, budget_bytes);
    Ok(evicted
        .into_iter()
        .map(|(path, bytes)| {
            (
                path,
                DirSizeSample {
                    bytes,
                    truncated: false,
                },
            )
        })
        .collect())
}

/// Pure-logic core for size-budget eviction. Takes `(path, mtime, size)`
/// tuples instead of `DirEntry` so unit tests can inject deterministic
/// mtimes without filesystem manipulation. The retained set is always the
/// largest newest-first prefix that fits; the remaining suffix is returned
/// for eviction oldest-first.
fn evict_to_budget_core(
    mut buckets: Vec<(std::path::PathBuf, SystemTime, u64)>,
    budget_bytes: u64,
) -> Vec<(std::path::PathBuf, u64)> {
    buckets.sort_by_key(|b| std::cmp::Reverse(b.1));

    let mut cumulative: u64 = 0;
    let mut retained = 0;
    for (_, _, size) in &buckets {
        match cumulative.checked_add(*size) {
            Some(next) if next <= budget_bytes => {
                cumulative = next;
                retained += 1;
            }
            _ => break,
        }
    }

    buckets
        .into_iter()
        .skip(retained)
        .rev()
        .map(|(path, _, size)| (path, size))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0B");
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(1024), "1.0KB");
        assert_eq!(format_size(1536), "1.5KB");
        assert_eq!(format_size(1048576), "1.0MB");
        assert_eq!(
            format_size_sample(DirSizeSample {
                bytes: 1048576,
                truncated: true,
            }),
            "≥1.0MB"
        );
    }

    #[test]
    fn test_parse_duration_days() {
        assert_eq!(parse_duration_days("7d"), Some(7 * 86400));
        assert_eq!(parse_duration_days("30d"), Some(30 * 86400));
        assert_eq!(parse_duration_days("1d"), Some(86400));
        assert_eq!(parse_duration_days("30"), Some(30 * 86400));
        assert_eq!(parse_duration_days("abc"), None);
        assert_eq!(parse_duration_days(&u64::MAX.to_string()), None);
    }

    /// Source hak: 2026-05-23 div0 system-cleanup. Parser must accept the
    /// common operator vocabulary (GB/MB/KB plus shorter G/M/K aliases) and
    /// hard-fail on garbage so the handler does not silently nuke the
    /// whole cache.
    #[test]
    fn parse_size_budget_supports_human_units() {
        assert_eq!(parse_size_budget("1GB"), Some(1_000_000_000));
        assert_eq!(parse_size_budget("500MB"), Some(500_000_000));
        assert_eq!(parse_size_budget("250M"), Some(250_000_000));
        assert_eq!(parse_size_budget("1500KB"), Some(1_500_000));
        assert_eq!(parse_size_budget("2G"), Some(2_000_000_000));
        assert_eq!(parse_size_budget("123456"), Some(123_456));
        assert_eq!(parse_size_budget("1.5GB"), Some(1_500_000_000));
        // case + whitespace tolerant
        assert_eq!(parse_size_budget(" 1 gb "), Some(1_000_000_000));
        // garbage rejected
        assert_eq!(parse_size_budget("abc"), None);
        assert_eq!(parse_size_budget(""), None);
        assert_eq!(parse_size_budget("-1GB"), None);
    }

    #[test]
    fn cache_bucket_enumeration_errors_are_rejected_for_cleanup() {
        let iterator_error = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "fixture bucket cannot be enumerated",
        );
        let enumeration =
            collect_cache_bucket_entries([Err::<std::fs::DirEntry, _>(iterator_error)]);

        assert!(
            matches!(enumeration.into_complete(), Err(1)),
            "cleanup must reject an incomplete top-level bucket inventory"
        );

        let temp = TempDir::new().expect("create cache root");
        let bucket = temp.path().join("removed-after-read-dir");
        fs::create_dir(&bucket).expect("create bucket");
        let entry = fs::read_dir(temp.path())
            .expect("read cache root")
            .next()
            .expect("bucket entry")
            .expect("read bucket entry");
        fs::remove_dir(&bucket).expect("remove bucket before metadata read");

        let enumeration = collect_cache_bucket_entries([Ok(entry)]);
        assert_eq!(enumeration.errors, 1);
        assert!(enumeration.entries.is_empty());
    }

    #[test]
    fn cache_bucket_inventory_stops_before_visiting_after_budget_expiry() {
        let temp = TempDir::new().expect("create cache root");
        fs::create_dir(temp.path().join("unvisited-bucket")).expect("create bucket");
        let budget = EnumerationBudget::start(std::time::Duration::ZERO);
        let mut visited = 0;

        let summary = visit_cache_bucket_entries(
            fs::read_dir(temp.path()).expect("read cache root"),
            Some(&budget),
            |_| visited += 1,
        );

        assert!(summary.time_ceiling_hit);
        assert_eq!(visited, 0, "expired inventories must not retain a bucket");
    }

    #[test]
    fn cache_bucket_rows_stay_bounded_and_keep_the_largest_entries() {
        let mut rows = Vec::new();
        for size_bytes in [1, 4, 3, 2] {
            retain_cache_bucket_row(
                &mut rows,
                CacheBucketRow {
                    org_repo: format!("org/repo-{size_bytes}"),
                    project_path: format!("/repo-{size_bytes}"),
                    size_bytes,
                    truncated: false,
                    meta: String::new(),
                },
                2,
            );
        }
        rows.sort_by(cache_bucket_row_order);

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter().map(|row| row.size_bytes).collect::<Vec<_>>(),
            vec![4, 3]
        );
    }

    /// Newest buckets stay under the budget; older overflow gets evicted.
    /// Pure-logic test — no filesystem manipulation, deterministic.
    #[test]
    fn evict_to_budget_core_keeps_newest_within_limit() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let buckets = vec![
            (
                std::path::PathBuf::from("A_oldest"),
                now - std::time::Duration::from_secs(30),
                100,
            ),
            (
                std::path::PathBuf::from("B_middle"),
                now - std::time::Duration::from_secs(20),
                100,
            ),
            (
                std::path::PathBuf::from("C_newest"),
                now - std::time::Duration::from_secs(10),
                100,
            ),
        ];

        // Budget 250 B fits B + C (200 B newest), evicts A.
        let evicted = evict_to_budget_core(buckets.clone(), 250);
        assert_eq!(evicted.len(), 1);
        assert_eq!(
            evicted[0].0.file_name().unwrap().to_string_lossy(),
            "A_oldest"
        );
    }

    /// Budget so tight it cannot hold even the newest single bucket: every
    /// bucket gets evicted.
    #[test]
    fn evict_to_budget_core_evicts_everything_when_budget_too_small() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let buckets = vec![(std::path::PathBuf::from("only"), now, 1_000)];
        let evicted = evict_to_budget_core(buckets, 100);
        assert_eq!(evicted.len(), 1);
    }

    #[test]
    fn evict_to_budget_core_never_keeps_older_bucket_after_newer_overflow() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let buckets = vec![
            (
                std::path::PathBuf::from("older-small"),
                now - std::time::Duration::from_secs(10),
                50,
            ),
            (std::path::PathBuf::from("newest-oversized"), now, 1_000),
        ];

        let evicted = evict_to_budget_core(buckets, 100);

        assert_eq!(
            evicted
                .iter()
                .map(|(path, _)| path.as_path())
                .collect::<Vec<_>>(),
            vec![
                std::path::Path::new("older-small"),
                std::path::Path::new("newest-oversized"),
            ],
            "once a newer bucket overflows, no older bucket may be retained"
        );
    }

    /// Budget large enough to hold everything: no eviction.
    #[test]
    fn evict_to_budget_core_keeps_all_when_under_budget() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let buckets = vec![
            (
                std::path::PathBuf::from("a"),
                now - std::time::Duration::from_secs(10),
                100,
            ),
            (
                std::path::PathBuf::from("b"),
                now - std::time::Duration::from_secs(5),
                100,
            ),
        ];
        let evicted = evict_to_budget_core(buckets, 10_000);
        assert!(evicted.is_empty());
    }

    #[test]
    fn evict_measured_to_budget_rejects_truncated_measurements() {
        let path = std::path::PathBuf::from("too-large-to-measure-exactly");
        let buckets = vec![(
            path.clone(),
            SystemTime::UNIX_EPOCH,
            DirSizeSample {
                bytes: 100,
                truncated: true,
            },
        )];

        assert_eq!(evict_measured_to_budget(buckets, 50), Err(path));
    }

    #[test]
    fn evict_to_budget_rejects_unreadable_bucket_recency() {
        let temp = TempDir::new().expect("create cache root");
        let bucket = temp.path().join("removed-before-mtime");
        fs::create_dir(&bucket).expect("create bucket");
        let entry = fs::read_dir(temp.path())
            .expect("read cache root")
            .next()
            .expect("bucket entry")
            .expect("read bucket entry");
        fs::remove_dir(&bucket).expect("remove bucket before mtime read");

        assert_eq!(evict_to_budget(&[&entry], 0), Err(bucket));
    }

    #[test]
    fn removal_summary_counts_only_successfully_removed_bytes() {
        let temp = TempDir::new().expect("create temp dir");
        let present = temp.path().join("present");
        let missing = temp.path().join("missing");
        fs::create_dir(&present).expect("create removable bucket");

        let summary = remove_candidates(&[
            (
                present,
                DirSizeSample {
                    bytes: 4,
                    truncated: false,
                },
            ),
            (
                missing,
                DirSizeSample {
                    bytes: 999,
                    truncated: false,
                },
            ),
        ]);

        assert_eq!(summary.removed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.freed.bytes, 4);
        assert!(!summary.freed.truncated);
    }

    #[test]
    #[serial_test::serial]
    fn try_lock_global_cache_buckets_is_deterministic_and_contended() {
        let (cache_dir, _guard) = crate::snapshot::test_env::isolated_cache();
        let projects = cache_dir.path().join("projects");
        let bucket_a = projects.join("aaaaaaaaaaaaaaaa");
        let bucket_b = projects.join("bbbbbbbbbbbbbbbb");
        fs::create_dir_all(&bucket_a).expect("create bucket a");
        fs::create_dir_all(&bucket_b).expect("create bucket b");
        fs::write(bucket_a.join("payload"), [1_u8; 32]).expect("write a");
        fs::write(bucket_b.join("payload"), [2_u8; 32]).expect("write b");

        let entries: Vec<_> = fs::read_dir(&projects)
            .expect("read projects")
            .map(|entry| entry.expect("dir entry"))
            .collect();
        assert_eq!(entries.len(), 2);

        let locks = try_lock_global_cache_buckets(&entries).expect("lock all free buckets");
        assert_eq!(locks.len(), 2);

        // Second acquisition of the same set must fail closed (contended) while
        // the first guards are still held.
        let contended = try_lock_global_cache_buckets(&entries).expect_err("must contend");
        assert!(matches!(contended.1, SnapshotCacheLockError::Contended));
        drop(locks);

        // After release, locks are acquirable again.
        let _reacquired = try_lock_global_cache_buckets(&entries).expect("reacquire after release");
    }

    #[test]
    #[serial_test::serial]
    fn handle_clean_max_size_aborts_without_deletion_while_writer_holds_lock() {
        let (cache_dir, _guard) = crate::snapshot::test_env::isolated_cache();
        let projects = cache_dir.path().join("projects");
        let bucket_a = projects.join("aaaaaaaaaaaaaaaa");
        let bucket_b = projects.join("bbbbbbbbbbbbbbbb");
        fs::create_dir_all(&bucket_a).expect("create bucket a");
        fs::create_dir_all(&bucket_b).expect("create bucket b");
        fs::write(bucket_a.join("payload"), [1_u8; 64]).expect("write a");
        fs::write(bucket_b.join("payload"), [2_u8; 64]).expect("write b");

        // Simulate Snapshot::save holding the per-bucket exclusive lock.
        let _writer = try_acquire_snapshot_cache_lock_for_bucket_id("aaaaaaaaaaaaaaaa")
            .expect("writer acquires bucket lock");

        match handle_clean(false, None, None, Some("1B"), true) {
            DispatchResult::Exit(2) => {}
            DispatchResult::Exit(code) => {
                panic!("expected fail-closed exit 2, got exit {code}")
            }
            DispatchResult::ShowHelp
            | DispatchResult::ShowLegacyHelp
            | DispatchResult::ShowVersion
            | DispatchResult::Continue(_) => {
                panic!("expected fail-closed exit 2, got non-exit dispatch result")
            }
        }

        assert!(
            bucket_a.exists() && bucket_b.exists(),
            "contended cleanup must not delete any measured bucket"
        );
    }

    #[test]
    fn test_select_project_path_prefers_shortest_root() {
        let now = SystemTime::UNIX_EPOCH;
        let primary = CacheSnapshotRecord {
            metadata: SnapshotMetadata {
                schema_version: "0.9.0".to_string(),
                generated_at: "2026-03-31T16:18:00Z".to_string(),
                roots: vec!["/tmp/demo".to_string()],
                git_owner_repo: Some("vetcoders/demo".to_string()),
                git_repo: Some("demo".to_string()),
                git_branch: Some("main".to_string()),
                git_commit: Some("abc123".to_string()),
                ..SnapshotMetadata::default()
            },
            modified_at: now,
            is_latest_pointer: false,
        };
        let nested = CacheSnapshotRecord {
            metadata: SnapshotMetadata {
                schema_version: "0.9.0".to_string(),
                generated_at: "2026-03-31T16:19:00Z".to_string(),
                roots: vec!["/tmp/demo/src".to_string()],
                git_owner_repo: Some("vetcoders/demo".to_string()),
                git_repo: Some("demo".to_string()),
                git_branch: Some("feature".to_string()),
                git_commit: Some("def456".to_string()),
                ..SnapshotMetadata::default()
            },
            modified_at: now,
            is_latest_pointer: false,
        };

        let snapshots = vec![&primary, &nested];
        assert_eq!(
            select_project_path(&snapshots),
            Some("/tmp/demo".to_string())
        );
    }

    #[test]
    fn test_resolve_org_repo_label_uses_local_fallback_for_non_git_bucket() {
        let snapshot = CacheSnapshotRecord {
            metadata: SnapshotMetadata {
                schema_version: "0.9.0".to_string(),
                generated_at: "2026-03-31T16:18:00Z".to_string(),
                roots: vec!["/tmp/local-project".to_string()],
                ..SnapshotMetadata::default()
            },
            modified_at: SystemTime::UNIX_EPOCH,
            is_latest_pointer: false,
        };
        let snapshots = vec![&snapshot];

        assert_eq!(
            resolve_org_repo_label(&snapshots, "abc123deadbeef00", "/tmp/local-project"),
            "local/local-project"
        );
    }

    #[test]
    fn test_format_cache_meta_skips_latest_pointer_duplicates() {
        let older = CacheSnapshotRecord {
            metadata: SnapshotMetadata {
                schema_version: "0.9.0".to_string(),
                generated_at: "2026-03-30T12:00:00Z".to_string(),
                roots: vec!["/tmp/demo".to_string()],
                git_owner_repo: Some("vetcoders/demo".to_string()),
                git_repo: Some("demo".to_string()),
                git_branch: Some("main".to_string()),
                git_commit: Some("aaa111".to_string()),
                ..SnapshotMetadata::default()
            },
            modified_at: SystemTime::UNIX_EPOCH,
            is_latest_pointer: false,
        };
        let newer = CacheSnapshotRecord {
            metadata: SnapshotMetadata {
                schema_version: "0.9.0".to_string(),
                generated_at: "2026-03-31T12:00:00Z".to_string(),
                roots: vec!["/tmp/demo".to_string()],
                git_owner_repo: Some("vetcoders/demo".to_string()),
                git_repo: Some("demo".to_string()),
                git_branch: Some("feature".to_string()),
                git_commit: Some("bbb222".to_string()),
                ..SnapshotMetadata::default()
            },
            modified_at: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10),
            is_latest_pointer: false,
        };
        let latest_pointer = CacheSnapshotRecord {
            metadata: SnapshotMetadata {
                schema_version: "0.9.0".to_string(),
                generated_at: "2026-03-31T12:00:00Z".to_string(),
                roots: vec!["/tmp/demo".to_string()],
                git_owner_repo: Some("vetcoders/demo".to_string()),
                git_repo: Some("demo".to_string()),
                git_branch: Some("feature".to_string()),
                git_commit: Some("bbb222".to_string()),
                ..SnapshotMetadata::default()
            },
            modified_at: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(20),
            is_latest_pointer: true,
        };

        let snapshots = [older, newer, latest_pointer];
        let effective = effective_bucket_snapshots(&snapshots);
        assert_eq!(
            format_cache_meta(&effective),
            "scans 2; branches 2; latest 2026-03-31T12:00:00Z; ref feature@bbb222; schema 0.9.0"
        );
    }

    #[test]
    fn test_collect_cache_bucket_row_falls_back_without_snapshot_metadata() {
        let temp = TempDir::new().expect("create temp bucket");
        fs::write(temp.path().join("artifact.bin"), b"cache-bytes").expect("write artifact");

        let row = collect_cache_bucket_row("feedfacecafebeef", temp.path(), None);

        assert_eq!(row.org_repo, "unknown/feedfacecafebeef");
        assert_eq!(row.project_path, "(unknown path)");
        assert_eq!(row.meta, "scans 0; latest unknown; schema unknown");
        assert!(row.size_bytes > 0);
        assert!(!row.truncated);
    }

    /// Audit class H: an exhausted time budget must surface as `truncated`,
    /// never as a polished exact size.
    #[test]
    fn collect_cache_bucket_row_marks_truncated_on_expired_budget() {
        let temp = TempDir::new().expect("create temp bucket");
        fs::write(temp.path().join("artifact.bin"), b"cache-bytes").expect("write artifact");

        let expired = EnumerationBudget::start(std::time::Duration::ZERO);
        let row = collect_cache_bucket_row("feedfacecafebeef", temp.path(), Some(&expired));

        assert!(row.truncated, "expired budget must mark the row truncated");
    }

    #[test]
    fn collect_cache_bucket_row_marks_walk_errors_incomplete() {
        let temp = TempDir::new().expect("create cache root");
        let missing = temp.path().join("missing-bucket");

        let row = collect_cache_bucket_row("feedfacecafebeef", &missing, None);

        assert!(
            row.truncated,
            "walk errors must mark the row as a lower bound"
        );
        assert_eq!(row.size_bytes, 0);
    }

    #[test]
    fn collect_cache_bucket_stats_marks_metadata_errors_incomplete() {
        let temp = TempDir::new().expect("create cache root");
        let path = temp.path().join("removed-after-walk");
        fs::write(&path, b"cache-bytes").expect("write cache file");
        let entry = walkdir::WalkDir::new(&path)
            .into_iter()
            .next()
            .expect("root entry")
            .expect("walk cache file");
        fs::remove_file(&path).expect("remove file before metadata read");

        let stats = collect_cache_bucket_stats_entries([Ok(entry)], temp.path(), None);

        assert!(
            stats.truncated,
            "metadata errors must mark cache-list statistics incomplete"
        );
        assert_eq!(stats.size_bytes, 0);
    }

    fn assert_exit(result: DispatchResult, expected: i32) {
        match result {
            DispatchResult::Exit(code) => assert_eq!(code, expected, "expected exit {expected}"),
            DispatchResult::ShowHelp
            | DispatchResult::ShowLegacyHelp
            | DispatchResult::ShowVersion
            | DispatchResult::Continue(_) => {
                panic!("expected exit {expected}, got non-exit dispatch result")
            }
        }
    }

    /// Handler belt-and-suspenders: an empty path must not resolve to cwd
    /// even if a caller bypasses the parser.
    #[test]
    #[serial_test::serial]
    fn w4_04_cache_clean_empty_path_handler_refuses_without_deleting() {
        let (_cache_dir, _guard) = crate::snapshot::test_env::isolated_cache();
        let known_project = TempDir::new().expect("known project");
        let known_bucket = project_cache_dir(known_project.path());
        fs::create_dir_all(&known_bucket).expect("create known bucket");
        fs::write(known_bucket.join("payload"), b"keep-me").expect("write payload");

        assert_exit(
            handle_clean(false, Some(Path::new("")), None, None, true),
            2,
        );
        assert!(
            known_bucket.exists(),
            "empty --project must not delete any bucket"
        );
    }

    #[test]
    #[serial_test::serial]
    fn w4_04_cache_clean_unknown_project_refused_with_known_buckets() {
        let (_cache_dir, _guard) = crate::snapshot::test_env::isolated_cache();
        let known_project = TempDir::new().expect("known project");
        let known_bucket = project_cache_dir(known_project.path());
        fs::create_dir_all(&known_bucket).expect("create known bucket");
        fs::write(known_bucket.join("payload"), b"keep-me").expect("write payload");
        let bucket_id = known_bucket
            .file_name()
            .expect("bucket id")
            .to_string_lossy()
            .into_owned();

        let missing = TempDir::new().expect("unknown project");
        let projects_dir = crate::snapshot::cache_base_dir().join("projects");
        let known = known_cache_bucket_ids(&projects_dir);
        assert!(
            known.contains(&bucket_id),
            "fixture bucket {bucket_id} must appear in known ids: {known:?}"
        );
        let msg = format_unknown_project_refusal(missing.path(), &known);
        assert!(
            msg.contains("Refusing cache clean"),
            "unknown project must refuse, got: {msg}"
        );
        assert!(
            msg.contains("Known cache buckets"),
            "refusal must list known buckets, got: {msg}"
        );
        assert!(
            msg.contains(&bucket_id),
            "refusal must name the live bucket {bucket_id}, got: {msg}"
        );
        assert!(
            msg.contains("No cache entries were removed"),
            "refusal must attest zero deletions, got: {msg}"
        );

        assert_exit(
            handle_clean(false, Some(missing.path()), None, None, true),
            2,
        );
        assert!(
            known_bucket.exists(),
            "unknown project --force must not delete known buckets"
        );
    }

    #[test]
    #[serial_test::serial]
    fn w4_04_cache_clean_project_without_force_is_preview_only() {
        let (_cache_dir, _guard) = crate::snapshot::test_env::isolated_cache();
        let project = TempDir::new().expect("project");
        let bucket = project_cache_dir(project.path());
        fs::create_dir_all(&bucket).expect("create bucket");
        fs::write(bucket.join("payload"), b"preview-me").expect("write payload");

        assert_exit(
            handle_clean(false, Some(project.path()), None, None, false),
            1,
        );
        assert!(
            bucket.exists(),
            "preview without --force must not delete the addressed bucket"
        );
    }
}
