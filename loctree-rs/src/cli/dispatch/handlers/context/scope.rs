//! Cut 11 — auto-scope discovery for `loct context` (zero-flag UX).
//!
//! When the operator types `loct context` with no `--file` / `--task` /
//! `--changed`, we still need to surface the most relevant slice. The
//! discovery algorithm is intentionally deterministic: same git state +
//! same worktree state = same scope output.
//!
//! Priority order:
//! 1. Dirty worktree → use changed-file scope (treat as `--changed`).
//! 2. Branch name carrying intent (e.g. `feat/cut11-context-pill`) → keep
//!    as a hint (recorded in `AutoScope::branch_hint`).
//! 3. Recent commits (last 5) → recorded in `AutoScope::commit_hints`.
//! 4. Fall back to top-N hubs by importer count (the highest-leverage
//!    files in the snapshot).
//!
//! The scope is consumed by the pill renderer, which uses the structured
//! data to pick which targets to compose slices for and which hints to
//! surface in TL;DR. The pill never invents scope: every target it shows
//! was either in the dirty worktree, a top-importer hub, or an explicit
//! flag from the operator.

use std::path::Path;
use std::process::Command;

use crate::metrics::top_hubs_by_importers_direct_filtered;
use crate::snapshot::Snapshot;

/// Auto-derived scope for a zero-flag `loct context` invocation.
#[derive(Debug, Clone, Default)]
pub struct AutoScope {
    /// Files derived from git status (dirty worktree only). May be empty.
    pub changed_files: Vec<String>,
    /// Top-N hub files by importer count.
    pub top_hubs: Vec<HubEntry>,
    /// Recent activity (top-N files modified in the last 24-48h, by commit).
    pub recent_files: Vec<String>,
    /// Branch name, if a git branch is checked out.
    pub branch: Option<String>,
    /// Branch-derived intent hint (parsed from `feat/<x>` / `fix/<x>` form).
    pub branch_hint: Option<String>,
    /// Last 5 commit subject lines (newest first).
    pub commit_hints: Vec<String>,
    /// `true` when the worktree had uncommitted changes at scope discovery.
    /// Canonical source for the worktree label rendered into the pill header
    /// (kept in sync with `pack.risk.dirty_worktree`).
    pub dirty: bool,
}

#[derive(Debug, Clone)]
pub struct HubEntry {
    pub file: String,
    pub importers: usize,
}

/// Default ceiling for the top-hubs list when no explicit flag was given.
pub const DEFAULT_TOP_HUBS: usize = 30;

/// Derive an `AutoScope` from snapshot + git state.
///
/// `root` is the directory the pill is being generated for (typically the
/// snapshot root). All git operations run with `current_dir(root)`.
pub fn discover(snapshot: &Snapshot, root: &Path) -> AutoScope {
    let branch = current_branch(root);
    let dirty = worktree_dirty(root).unwrap_or(false);
    let changed_files = if dirty {
        changed_files(root)
    } else {
        Vec::new()
    };
    let recent_files = recently_committed(root, 24);
    let commit_hints = recent_commit_subjects(root, 5);

    let branch_hint = branch.as_deref().and_then(parse_branch_hint);
    let top_hubs = top_hubs_by_importer(snapshot, DEFAULT_TOP_HUBS);

    AutoScope {
        changed_files,
        top_hubs,
        recent_files,
        branch,
        branch_hint,
        commit_hints,
        dirty,
    }
}

/// Produce a stable identifier for the auto-scope. The determinism contract
/// (same git state -> same identifier) is asserted by the unit tests; gated
/// to `cfg(test)` until a non-test caller adopts it (the obvious candidate is
/// a future snapshot-cache key, but no such consumer exists yet).
#[cfg(test)]
pub fn scope_identifier(scope: &AutoScope) -> String {
    let mut buf = String::new();
    if let Some(branch) = &scope.branch {
        buf.push_str(branch);
    }
    buf.push('|');
    buf.push_str(if scope.dirty { "dirty" } else { "clean" });
    buf.push('|');
    buf.push_str(&scope.changed_files.len().to_string());
    buf.push('|');
    buf.push_str(&scope.top_hubs.len().to_string());
    buf
}

fn current_branch(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let trimmed = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if trimmed.is_empty() || trimmed == "HEAD" {
        return None;
    }
    Some(trimmed)
}

fn worktree_dirty(root: &Path) -> Option<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

fn changed_files(root: &Path) -> Vec<String> {
    let output = match Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(root)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let body = String::from_utf8_lossy(&output.stdout);
    let mut files = Vec::new();
    for line in body.lines() {
        // Porcelain format: "XY path" or "XY path -> renamed".
        if line.len() < 3 {
            continue;
        }
        let path_part = line[3..].trim();
        let normalized = path_part
            .rsplit(" -> ")
            .next()
            .unwrap_or(path_part)
            .trim_matches('"')
            .to_string();
        if !normalized.is_empty() {
            files.push(normalized);
        }
    }
    files.sort();
    files.dedup();
    files
}

fn recently_committed(root: &Path, hours: u64) -> Vec<String> {
    let since = format!("--since={hours} hours ago");
    let output = match Command::new("git")
        .args(["log", "--name-only", "--pretty=format:", &since])
        .current_dir(root)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let body = String::from_utf8_lossy(&output.stdout);
    let mut files = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        files.push(trimmed.to_string());
    }
    files.sort();
    files.dedup();
    // Keep recent slice tight — pill budget cares about ranking, not volume.
    files.truncate(20);
    files
}

fn recent_commit_subjects(root: &Path, count: usize) -> Vec<String> {
    let output = match Command::new("git")
        .args(["log", &format!("-{count}"), "--pretty=format:%h %s"])
        .current_dir(root)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let body = String::from_utf8_lossy(&output.stdout);
    body.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Branch hints encode the operator's current intent. Convert
/// `feat/cut11-context-pill` → `Cut 11 context pill` for human consumption.
fn parse_branch_hint(branch: &str) -> Option<String> {
    let payload = branch
        .split_once('/')
        .map(|(_, suffix)| suffix)
        .unwrap_or(branch);
    if payload.is_empty() {
        return None;
    }
    let humanized = payload
        .replace(['-', '_'], " ")
        .split_whitespace()
        .map(capitalize_word)
        .collect::<Vec<_>>()
        .join(" ");
    if humanized.is_empty() {
        None
    } else {
        Some(humanized)
    }
}

fn capitalize_word(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn top_hubs_by_importer(snapshot: &Snapshot, limit: usize) -> Vec<HubEntry> {
    // loctree-feedback hak 2026-05-23 #1: test fixtures must not be
    // promoted to repo-root hub status. A fixture file with internal
    // edges (`tests/fixtures/tauri_app/src/App.tsx` ↔
    // `tests/fixtures/tauri_app/src/components/...`) was being
    // surfaced as `Path: App.tsx` + Authority: RepoVerified + Role:
    // Target, which is structurally false: the file lives under
    // `tests/fixtures/`, not at the canonical repo root, and its
    // authority is *FixtureSource*, not *RepoVerified*. We drop fixture
    // targets from auto-scope while keeping the importer metric itself
    // canonical in `crate::metrics`.
    top_hubs_by_importers_direct_filtered(snapshot, limit, |metric| !is_fixture_path(&metric.file))
        .into_iter()
        .map(|metric| HubEntry {
            file: metric.file,
            importers: metric.importers_direct,
        })
        .collect()
}

/// Returns true when `path` lives under a directory the project clearly
/// marks as test fixture / mock / snapshot territory. Used by scope
/// discovery to keep fixtures out of hub ranking and by the pill
/// renderer to demote their authority.
///
/// Matched fragments (case-sensitive, slash-bounded):
///   - `tests/fixtures/`
///   - `test/fixtures/`
///   - `tests/data/`
///   - `test_fixtures/`
///   - `__fixtures__/`
///   - `__snapshots__/`
///   - `testdata/`
///   - `fixtures/`
///   - `mocks/`
///
/// Backslash variants (Windows paths) are normalized to forward slashes
/// before matching.
pub fn is_fixture_path(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let normalized = path.replace('\\', "/");
    const FRAGMENTS: &[&str] = &[
        "tests/fixtures/",
        "test/fixtures/",
        "tests/data/",
        "test_fixtures/",
        "__fixtures__/",
        "__snapshots__/",
        "testdata/",
        "/fixtures/",
        "/mocks/",
    ];
    for frag in FRAGMENTS {
        if normalized.contains(frag) {
            return true;
        }
    }
    // Bare top-level `fixtures/` or `mocks/` (no leading slash) also count.
    if normalized.starts_with("fixtures/") || normalized.starts_with("mocks/") {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::GraphEdge;
    use crate::types::FileAnalysis;

    fn fixture_snapshot() -> Snapshot {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        for name in ["hub.rs", "consumer_a.rs", "consumer_b.rs", "isolated.rs"] {
            snapshot.files.push(FileAnalysis::new(name.to_string()));
        }
        for from in ["consumer_a.rs", "consumer_b.rs"] {
            snapshot.edges.push(GraphEdge {
                from: from.to_string(),
                to: "hub.rs".to_string(),
                label: "import".to_string(),
            });
        }
        snapshot
    }

    #[test]
    fn top_hubs_ranks_by_importer_count() {
        let snapshot = fixture_snapshot();
        let hubs = top_hubs_by_importer(&snapshot, 5);
        assert!(!hubs.is_empty());
        assert_eq!(hubs[0].file, "hub.rs");
        assert_eq!(hubs[0].importers, 2);
    }

    #[test]
    fn top_hubs_respects_limit() {
        let snapshot = fixture_snapshot();
        let hubs = top_hubs_by_importer(&snapshot, 1);
        assert_eq!(hubs.len(), 1);
    }

    #[test]
    fn parse_branch_hint_handles_feat_prefix() {
        assert_eq!(
            parse_branch_hint("feat/cut11-context-pill").as_deref(),
            Some("Cut11 Context Pill")
        );
    }

    #[test]
    fn parse_branch_hint_handles_no_prefix() {
        assert_eq!(parse_branch_hint("main").as_deref(), Some("Main"));
    }

    #[test]
    fn parse_branch_hint_returns_none_for_empty() {
        assert!(parse_branch_hint("").is_none());
        assert!(parse_branch_hint("feat/").is_none());
    }

    /// loctree-feedback hak 2026-05-23 #1 regression: fixture files (under
    /// `tests/fixtures/`, `__snapshots__/`, etc.) must NOT enter the hub
    /// ranking that drives `loct context` scope selection. A pure-fixture
    /// edge graph must produce an empty top-hubs list.
    #[test]
    fn fixture_paths_are_excluded_from_top_hubs() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        for name in [
            "loctree-rs/tests/fixtures/tauri_app/src/App.tsx",
            "loctree-rs/tests/fixtures/tauri_app/src/components/Hero.tsx",
            "loctree-rs/tests/fixtures/tauri_app/src/components/Footer.tsx",
        ] {
            snapshot.files.push(FileAnalysis::new(name.to_string()));
        }
        snapshot.edges.push(GraphEdge {
            from: "loctree-rs/tests/fixtures/tauri_app/src/components/Hero.tsx".to_string(),
            to: "loctree-rs/tests/fixtures/tauri_app/src/App.tsx".to_string(),
            label: "import".to_string(),
        });
        snapshot.edges.push(GraphEdge {
            from: "loctree-rs/tests/fixtures/tauri_app/src/components/Footer.tsx".to_string(),
            to: "loctree-rs/tests/fixtures/tauri_app/src/App.tsx".to_string(),
            label: "import".to_string(),
        });
        let hubs = top_hubs_by_importer(&snapshot, 10);
        assert!(
            hubs.is_empty(),
            "fixtures must not show up as repo hubs, got: {hubs:?}"
        );
    }

    #[test]
    fn is_fixture_path_matches_canonical_layouts() {
        for path in [
            "loctree-rs/tests/fixtures/tauri_app/src/App.tsx",
            "tests/fixtures/sample.json",
            "tests/data/repo.toml",
            "src/__fixtures__/users.json",
            "components/__snapshots__/Button.snap",
            "test_fixtures/payload.yaml",
            "pkg/testdata/golden.txt",
            "fixtures/scenario_a/input.json",
            "mocks/api_response.json",
        ] {
            assert!(
                is_fixture_path(path),
                "expected fixture-path match for {path}"
            );
        }
    }

    #[test]
    fn is_fixture_path_keeps_real_source_clean() {
        for path in [
            "src/main.rs",
            "loctree-rs/src/cli/dispatch/handlers/context/pill.rs",
            "components/App.tsx",
            "test_utils/helpers.rs",
            "tests/integration.rs",
        ] {
            assert!(
                !is_fixture_path(path),
                "expected real-source pass for {path}"
            );
        }
    }

    #[test]
    fn scope_identifier_is_deterministic_for_identical_state() {
        let scope = AutoScope {
            branch: Some("main".to_string()),
            dirty: false,
            changed_files: vec![],
            top_hubs: vec![HubEntry {
                file: "x".to_string(),
                importers: 1,
            }],
            ..AutoScope::default()
        };
        let a = scope_identifier(&scope);
        let b = scope_identifier(&scope);
        assert_eq!(a, b);
        assert!(a.contains("main"));
        assert!(a.contains("clean"));
    }
}
