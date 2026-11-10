//! Shared temporal acquisition for CLI and MCP. Git refs are frozen before scanning.
use crate::git::{CommitInfo, GitRepo};
use crate::snapshot::{
    AcquireOptions, Snapshot, SnapshotReusePolicy, SnapshotRootStrategy, acquire_snapshot,
};
use std::path::Path;

mod paths;

pub struct TemporalSnapshot {
    pub snapshot: Snapshot,
    /// None denotes a snapshot artifact or working tree, never a clean commit claim.
    pub commit: Option<CommitInfo>,
}

/// Freeze both Git endpoints before either historical scan begins.
pub fn resolve_pair(
    project: &Path,
    since: &str,
    to: Option<&str>,
) -> Result<(TemporalSnapshot, TemporalSnapshot), String> {
    let freeze = |endpoint: &str| -> Result<String, String> {
        if project.join(endpoint).exists() {
            Ok(endpoint.to_owned())
        } else {
            GitRepo::discover(project)
                .and_then(|repo| repo.resolve_ref(endpoint))
                .map_err(|e| e.to_string())
        }
    };
    let since = freeze(since)?;
    let to = to.map(freeze).transpose()?;
    Ok((
        resolve_snapshot(project, Some(&since))?,
        resolve_snapshot(project, to.as_deref())?,
    ))
}

pub fn resolve_snapshot(
    project: &Path,
    endpoint: Option<&str>,
) -> Result<TemporalSnapshot, String> {
    if let Some(value) = endpoint {
        let path = project.join(value);
        if path.exists() {
            return Snapshot::load(&path)
                .map(|snapshot| TemporalSnapshot {
                    snapshot,
                    commit: None,
                })
                .map_err(|e| format!("snapshot '{}': {e}", path.display()));
        }
        let repo = GitRepo::discover(project).map_err(|e| e.to_string())?;
        let sha = repo.resolve_ref(value).map_err(|e| e.to_string())?;
        let commit = repo.get_commit_info(&sha).map_err(|e| e.to_string())?;
        let temp = tempfile::TempDir::new().map_err(|e| e.to_string())?;
        let export = repo
            .export_tree(&sha, temp.path())
            .map_err(|e| e.to_string())?;
        let mut scan = crate::snapshot::unified_scan_args(&export, false);
        scan.force_non_git = true;
        scan.full_scan = true;
        let mut snapshot = crate::snapshot::build_snapshot_for_strategy(
            std::slice::from_ref(&export),
            &scan,
            true,
            SnapshotRootStrategy::Exact,
            false,
        )
        .map_err(|e| e.to_string())?;
        paths::normalize_owned_paths(&mut snapshot, &export).map_err(|e| e.to_string())?;
        snapshot.metadata.git_commit = Some(sha);
        snapshot.metadata.git_branch = None;
        Ok(TemporalSnapshot {
            snapshot,
            commit: Some(commit),
        })
    } else {
        let mut snapshot = acquire_snapshot(
            &[project.to_path_buf()],
            SnapshotReusePolicy::Strict,
            &AcquireOptions {
                fresh: true,
                quiet: true,
                print_scan_summary: false,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        // The working-tree side must use the same semantic coordinates as
        // historical exports, without rewriting its persisted scan artifact.
        let root = crate::snapshot::resolve_snapshot_root(&[project.to_path_buf()]);
        paths::normalize_owned_paths(&mut snapshot, &root).map_err(|e| e.to_string())?;
        Ok(TemporalSnapshot {
            snapshot,
            commit: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Repository, Signature};

    fn commit_all(repo: &Repository) -> String {
        let mut index = repo.index().expect("fixture index");
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .expect("stage fixture");
        index.write().expect("write fixture index");
        let tree = repo
            .find_tree(index.write_tree().expect("fixture tree"))
            .expect("find tree");
        let sig = Signature::now("Fixture", "fixture@example.invalid").expect("fixture signature");
        let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "fixture",
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .expect("commit fixture")
        .to_string()
    }

    fn make_repo() -> (tempfile::TempDir, Repository, String, String) {
        let dir = tempfile::tempdir().expect("fixture directory");
        let repo = Repository::init(dir.path()).expect("fixture repository");
        for (name, text) in [
            (
                "Makefile",
                "include common.mk\n.PHONY: all\nall:\n\t@echo ok\n",
            ),
            ("common.mk", "VALUE = one\n"),
            ("other.mk", "VALUE = two\n"),
            ("README.md", "one\n"),
        ] {
            std::fs::write(dir.path().join(name), text).expect("fixture file");
        }
        let first = commit_all(&repo);
        std::fs::write(dir.path().join("README.md"), "two\n").expect("readme change");
        let second = commit_all(&repo);
        (dir, repo, first, second)
    }

    #[test]
    fn historical_make_edges_ignore_readme_changes_and_fresh_export_roots() {
        let (dir, repo, first, second) = make_repo();
        let (base, target) =
            resolve_pair(dir.path(), &first, Some(&second)).expect("historical pair");
        assert!(
            base.snapshot
                .edges
                .iter()
                .any(|edge| edge.from == "Makefile" && edge.to == "common.mk"),
            "{:?}",
            base.snapshot.edges
        );
        let again =
            resolve_snapshot(dir.path(), Some(&first)).expect("fresh export of same commit");
        std::fs::write(dir.path().join("README.md"), "dirty readme\n").expect("dirty readme");
        let current = resolve_snapshot(dir.path(), None).expect("working tree");
        assert!(current.commit.is_none());
        for other in [&target, &again, &current] {
            let delta = crate::diff::SnapshotDiff::compare(
                &base.snapshot,
                &other.snapshot,
                None,
                None,
                &[],
            );
            assert!(delta.graph.edges_added.is_empty(), "{:?}", delta.graph);
            assert!(delta.graph.edges_removed.is_empty(), "{:?}", delta.graph);
            assert!(delta.exports.added.is_empty());
            assert!(delta.exports.removed.is_empty());
        }
        assert_eq!(
            repo.head()
                .expect("head")
                .target()
                .expect("oid")
                .to_string(),
            second
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).expect("readme"),
            "dirty readme\n"
        );
        assert_eq!(repo.statuses(None).expect("status").len(), 1);
        assert_eq!(repo.worktrees().expect("worktrees").len(), 0);
        assert!(
            base.snapshot
                .metadata
                .roots
                .iter()
                .all(|root| !Path::new(root).exists()),
            "exports must be disposable"
        );
    }

    #[test]
    fn historical_make_real_include_change_is_visible() {
        let (dir, repo, first, _) = make_repo();
        std::fs::write(
            dir.path().join("Makefile"),
            "include other.mk\n.PHONY: all\nall:\n\t@echo ok\n",
        )
        .expect("include change");
        let changed = commit_all(&repo);
        let (base, target) =
            resolve_pair(dir.path(), &first, Some(&changed)).expect("historical pair");
        let delta = crate::diff::SnapshotDiff::compare(
            &base.snapshot,
            &target.snapshot,
            base.commit,
            target.commit,
            &[],
        );
        assert_eq!(delta.graph.edges_removed.len(), 1);
        assert_eq!(delta.graph.edges_added.len(), 1);
        assert_eq!(delta.graph.edges_removed[0].to, Path::new("common.mk"));
        assert_eq!(delta.graph.edges_added[0].to, Path::new("other.mk"));
    }

    fn commit(repo: &Repository, text: &str) -> String {
        std::fs::write(
            repo.workdir()
                .expect("Git temporal fixture operation")
                .join("entry.ts"),
            text,
        )
        .expect("Git temporal fixture operation");
        let mut index = repo.index().expect("Git temporal fixture operation");
        index
            .add_path(Path::new("entry.ts"))
            .expect("Git temporal fixture operation");
        index.write().expect("Git temporal fixture operation");
        let tree_id = index.write_tree().expect("Git temporal fixture operation");
        let tree = repo
            .find_tree(tree_id)
            .expect("Git temporal fixture operation");
        let sig = Signature::now("Fixture", "fixture@example.invalid")
            .expect("Git temporal fixture operation");
        let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
        let parents: Vec<_> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree, &parents)
            .expect("Git temporal fixture operation")
            .to_string()
    }

    #[test]
    fn refs_tags_and_dirty_target_keep_identity_and_checkout() {
        let dir = tempfile::TempDir::new().expect("Git temporal fixture operation");
        let repo = Repository::init(dir.path()).expect("Git temporal fixture operation");
        let first = commit(&repo, "export const before = 1;\n");
        repo.tag_lightweight(
            "v1",
            &repo
                .find_object(
                    git2::Oid::from_str(&first).expect("Git temporal fixture operation"),
                    None,
                )
                .expect("Git temporal fixture operation"),
            false,
        )
        .expect("Git temporal fixture operation");
        let second = commit(&repo, "export const after = 2;\n");
        let before_worktrees = repo
            .worktrees()
            .expect("Git temporal fixture operation")
            .len();
        let base =
            resolve_snapshot(dir.path(), Some("v1")).expect("Git temporal fixture operation");
        let target =
            resolve_snapshot(dir.path(), Some("HEAD")).expect("Git temporal fixture operation");
        assert_eq!(
            base.commit
                .as_ref()
                .expect("Git temporal fixture operation")
                .hash,
            first
        );
        assert_eq!(
            target
                .commit
                .as_ref()
                .expect("Git temporal fixture operation")
                .hash,
            second
        );
        assert!(
            base.snapshot
                .files
                .iter()
                .any(|file| file.path == "entry.ts")
        );
        let delta = crate::diff::SnapshotDiff::compare(
            &base.snapshot,
            &target.snapshot,
            base.commit,
            target.commit,
            &[],
        );
        assert!(
            delta
                .exports
                .removed
                .iter()
                .any(|symbol| symbol.name == "before")
        );
        assert!(
            delta
                .exports
                .added
                .iter()
                .any(|symbol| symbol.name == "after")
        );
        std::fs::write(dir.path().join("entry.ts"), "export const dirty = 3;\n")
            .expect("Git temporal fixture operation");
        let current = resolve_snapshot(dir.path(), None).expect("Git temporal fixture operation");
        assert!(current.commit.is_none());
        assert!(
            current
                .snapshot
                .files
                .iter()
                .flat_map(|file| &file.exports)
                .any(|symbol| symbol.name == "dirty")
        );
        assert!(resolve_snapshot(dir.path(), Some("missing-ref")).is_err());
        assert_eq!(
            repo.head()
                .expect("Git temporal fixture operation")
                .target()
                .expect("Git temporal fixture operation")
                .to_string(),
            second
        );
        assert_eq!(
            repo.worktrees()
                .expect("Git temporal fixture operation")
                .len(),
            before_worktrees
        );
        assert!(
            std::fs::read_to_string(dir.path().join("entry.ts"))
                .expect("Git temporal fixture operation")
                .contains("dirty")
        );
        let changes = GitRepo::discover(dir.path())
            .expect("Git temporal fixture operation")
            .changed_files_in_worktree(&first)
            .expect("Git temporal fixture operation");
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn existing_invalid_path_is_not_reinterpreted_as_ref() {
        let dir = tempfile::TempDir::new().expect("Git temporal fixture operation");
        let repo = Repository::init(dir.path()).expect("Git temporal fixture operation");
        commit(&repo, "export const value = 1;\n");
        std::fs::create_dir(dir.path().join("HEAD")).expect("Git temporal fixture operation");
        let error = resolve_snapshot(dir.path(), Some("HEAD"))
            .err()
            .expect("Git temporal fixture operation");
        assert!(error.contains("snapshot '"), "{error}");
    }
}
