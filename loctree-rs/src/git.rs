//! Git operations for temporal awareness
//!
//! This module provides native git operations using libgit2 (git2 crate).
//! It enables loctree to analyze repository history and compare snapshots
//! across different commits.

use git2::{DiffOptions, Oid, Repository};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use time::{OffsetDateTime, format_description};

const GIT_TIMESTAMP_FORMAT: &str = "[year]-[month]-[day]T[hour]:[minute]:[second]Z";

fn format_git_timestamp(timestamp: i64) -> String {
    let datetime =
        OffsetDateTime::from_unix_timestamp(timestamp).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let format = format_description::parse_borrowed::<2>(GIT_TIMESTAMP_FORMAT)
        .expect("static git timestamp format should parse");

    datetime.format(&format).unwrap_or_default()
}

/// Find the git repository root by searching upward from the given path.
///
/// Uses libgit2's `Repository::discover()` which properly handles:
/// - Nested directories (searches upward to find .git)
/// - Git worktrees (where .git is a file, not a directory)
/// - Submodules
///
/// Returns `None` if no git repository is found.
///
/// # Example
/// ```ignore
/// // From /home/user/project/src/deep/nested/file.rs
/// // finds /home/user/project (where .git lives)
/// let root = find_git_root(Path::new("/home/user/project/src/deep/nested"));
/// assert_eq!(root, Some(PathBuf::from("/home/user/project")));
/// ```
pub fn find_git_root(path: &Path) -> Option<PathBuf> {
    Repository::discover(path)
        .ok()
        .and_then(|repo| repo.workdir().map(|p| p.to_path_buf()))
}

/// Error type for git operations
#[derive(Debug)]
pub enum GitError {
    /// Not a git repository
    NotARepository(String),
    /// Failed to resolve reference (branch, tag, commit)
    RefNotFound(String),
    /// Git operation failed
    OperationFailed(String),
    /// IO error
    IoError(std::io::Error),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::NotARepository(path) => {
                write!(f, "not a git repository: {}", path)
            }
            GitError::RefNotFound(reference) => {
                write!(f, "reference not found: {}", reference)
            }
            GitError::OperationFailed(msg) => {
                write!(f, "git operation failed: {}", msg)
            }
            GitError::IoError(e) => {
                write!(f, "IO error: {}", e)
            }
        }
    }
}

impl std::error::Error for GitError {}

impl From<git2::Error> for GitError {
    fn from(e: git2::Error) -> Self {
        GitError::OperationFailed(e.message().to_string())
    }
}

impl From<std::io::Error> for GitError {
    fn from(e: std::io::Error) -> Self {
        GitError::IoError(e)
    }
}

/// Information about a commit
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitInfo {
    /// Full commit hash
    pub hash: String,
    /// Short commit hash (7 chars)
    pub short_hash: String,
    /// Author name
    pub author: String,
    /// Author email
    pub author_email: String,
    /// Commit timestamp (ISO 8601)
    pub date: String,
    /// Unix timestamp
    pub timestamp: i64,
    /// Commit message (first line)
    pub message: String,
    /// Full commit message
    pub message_full: String,
}

/// Wrapper around a git repository
pub struct GitRepo {
    repo: Repository,
    path: PathBuf,
}

impl GitRepo {
    /// Discover a git repository from the given path
    /// Searches upward from the path to find .git directory
    pub fn discover(path: &Path) -> Result<Self, GitError> {
        let repo = Repository::discover(path)
            .map_err(|_| GitError::NotARepository(path.display().to_string()))?;

        let workdir = repo
            .workdir()
            .ok_or_else(|| GitError::NotARepository("bare repository".to_string()))?;

        Ok(Self {
            path: workdir.to_path_buf(),
            repo,
        })
    }

    /// Get the repository root path
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get the current HEAD commit hash
    pub fn head_commit(&self) -> Result<String, GitError> {
        let head = self.repo.head()?;
        let commit = head.peel_to_commit()?;
        Ok(commit.id().to_string())
    }

    /// Resolve a reference (branch, tag, commit hash, HEAD~n) to a commit hash
    pub fn resolve_ref(&self, reference: &str) -> Result<String, GitError> {
        // Try to parse as OID first (commit hash)
        if let Ok(oid) = Oid::from_str(reference)
            && self.repo.find_commit(oid).is_ok()
        {
            return Ok(oid.to_string());
        }

        // Try to resolve as a reference
        let obj = self
            .repo
            .revparse_single(reference)
            .map_err(|_| GitError::RefNotFound(reference.to_string()))?;

        let commit = obj.peel_to_commit().map_err(|_| {
            GitError::RefNotFound(format!("{} does not point to a commit", reference))
        })?;

        Ok(commit.id().to_string())
    }

    /// Get commit information for a given reference
    pub fn get_commit_info(&self, reference: &str) -> Result<CommitInfo, GitError> {
        let oid_str = self.resolve_ref(reference)?;
        let oid = Oid::from_str(&oid_str)?;
        let commit = self.repo.find_commit(oid)?;

        let author = commit.author();
        let time = commit.time();

        // Format timestamp
        let timestamp = time.seconds();
        let date = format_git_timestamp(timestamp);

        let message_full = commit.message().unwrap_or("").to_string();
        let message = message_full.lines().next().unwrap_or("").to_string();

        Ok(CommitInfo {
            hash: oid_str.clone(),
            short_hash: oid_str.chars().take(7).collect(),
            author: author.name().unwrap_or("Unknown").to_string(),
            author_email: author.email().unwrap_or("").to_string(),
            date,
            timestamp,
            message,
            message_full,
        })
    }

    /// Get commit log for a file or the entire repository
    pub fn log(&self, file_path: Option<&Path>, limit: usize) -> Result<Vec<CommitInfo>, GitError> {
        let mut revwalk = self.repo.revwalk()?;
        revwalk.push_head()?;
        revwalk.set_sorting(git2::Sort::TIME)?;

        let mut commits = Vec::new();

        for oid_result in revwalk {
            if commits.len() >= limit {
                break;
            }

            let oid = oid_result?;
            let commit = self.repo.find_commit(oid)?;

            // If file_path is specified, check if the commit touches that file
            if let Some(path) = file_path
                && !self.commit_touches_file(&commit, path)?
            {
                continue;
            }

            let author = commit.author();
            let time = commit.time();
            let timestamp = time.seconds();
            let date = format_git_timestamp(timestamp);

            let message_full = commit.message().unwrap_or("").to_string();
            let message = message_full.lines().next().unwrap_or("").to_string();

            commits.push(CommitInfo {
                hash: oid.to_string(),
                short_hash: oid.to_string().chars().take(7).collect(),
                author: author.name().unwrap_or("Unknown").to_string(),
                author_email: author.email().unwrap_or("").to_string(),
                date,
                timestamp,
                message,
                message_full,
            });
        }

        Ok(commits)
    }

    /// Check if a commit modifies a specific file
    fn commit_touches_file(
        &self,
        commit: &git2::Commit,
        file_path: &Path,
    ) -> Result<bool, GitError> {
        let tree = commit.tree()?;

        // Get parent tree (if exists)
        let parent_tree = if commit.parent_count() > 0 {
            Some(commit.parent(0)?.tree()?)
        } else {
            None
        };

        let mut opts = DiffOptions::new();
        opts.pathspec(file_path);

        let diff =
            self.repo
                .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))?;

        Ok(diff.deltas().count() > 0)
    }

    /// Get the list of files changed between two commits
    pub fn changed_files(&self, from: &str, to: &str) -> Result<Vec<ChangedFile>, GitError> {
        let from_oid = Oid::from_str(&self.resolve_ref(from)?)?;
        let to_oid = Oid::from_str(&self.resolve_ref(to)?)?;

        let from_commit = self.repo.find_commit(from_oid)?;
        let to_commit = self.repo.find_commit(to_oid)?;

        let from_tree = from_commit.tree()?;
        let to_tree = to_commit.tree()?;

        let diff = self
            .repo
            .diff_tree_to_tree(Some(&from_tree), Some(&to_tree), None)?;

        let mut files = Vec::new();

        for delta in diff.deltas() {
            let status = match delta.status() {
                git2::Delta::Added => ChangeStatus::Added,
                git2::Delta::Deleted => ChangeStatus::Deleted,
                git2::Delta::Modified => ChangeStatus::Modified,
                git2::Delta::Renamed => ChangeStatus::Renamed,
                git2::Delta::Copied => ChangeStatus::Copied,
                _ => ChangeStatus::Modified,
            };

            let old_path = delta.old_file().path().map(|p| p.to_path_buf());
            let new_path = delta.new_file().path().map(|p| p.to_path_buf());

            files.push(ChangedFile {
                old_path,
                new_path,
                status,
            });
        }

        Ok(files)
    }

    /// Get file content at a specific commit
    pub fn file_content_at(&self, reference: &str, file_path: &Path) -> Result<String, GitError> {
        let oid_str = self.resolve_ref(reference)?;
        let oid = Oid::from_str(&oid_str)?;
        let commit = self.repo.find_commit(oid)?;
        let tree = commit.tree()?;

        let entry = tree.get_path(file_path).map_err(|_| {
            GitError::OperationFailed(format!(
                "file '{}' not found at commit {}",
                file_path.display(),
                &oid_str[..7]
            ))
        })?;

        let blob = self.repo.find_blob(entry.id())?;
        let content = std::str::from_utf8(blob.content())
            .map_err(|_| GitError::OperationFailed("file is not valid UTF-8".to_string()))?;

        Ok(content.to_string())
    }

    /// List all files in the repository at a specific commit
    pub fn list_files_at(&self, reference: &str) -> Result<Vec<PathBuf>, GitError> {
        let oid_str = self.resolve_ref(reference)?;
        let oid = Oid::from_str(&oid_str)?;
        let commit = self.repo.find_commit(oid)?;
        let tree = commit.tree()?;

        let mut files = Vec::new();
        tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
            if entry.kind() == Some(git2::ObjectType::Blob) {
                let path = if dir.is_empty() {
                    PathBuf::from(entry.name().unwrap_or(""))
                } else {
                    PathBuf::from(dir).join(entry.name().unwrap_or(""))
                };
                files.push(path);
            }
            git2::TreeWalkResult::Ok
        })?;

        Ok(files)
    }

    /// Create a temporary worktree for a specific branch/commit
    /// Returns the path to the worktree directory
    pub fn create_worktree(&self, reference: &str, worktree_path: &Path) -> Result<(), GitError> {
        use std::process::Command;

        // Resolve the reference to ensure it exists
        self.resolve_ref(reference)?;

        // Use git worktree add command (libgit2 doesn't support worktrees well)
        let output = Command::new("git")
            .arg("worktree")
            .arg("add")
            .arg(worktree_path)
            .arg(reference)
            .current_dir(&self.path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitError::OperationFailed(format!(
                "Failed to create worktree: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Remove a worktree
    pub fn remove_worktree(&self, worktree_path: &Path) -> Result<(), GitError> {
        use std::process::Command;

        let output = Command::new("git")
            .arg("worktree")
            .arg("remove")
            .arg(worktree_path)
            .arg("--force")
            .current_dir(&self.path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitError::OperationFailed(format!(
                "Failed to remove worktree: {}",
                stderr
            )));
        }

        Ok(())
    }
}

/// Status of a changed file
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
}

/// Information about a changed file
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangedFile {
    pub old_path: Option<PathBuf>,
    pub new_path: Option<PathBuf>,
    pub status: ChangeStatus,
}

impl ChangedFile {
    /// Get the effective path (new_path for added/modified, old_path for deleted)
    pub fn path(&self) -> Option<&Path> {
        self.new_path.as_deref().or(self.old_path.as_deref())
    }
}

/// A single line of blame information
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlameEntry {
    /// Line number (1-indexed)
    pub line: usize,
    /// Commit hash that introduced this line
    pub commit_hash: String,
    /// Short commit hash
    pub short_hash: String,
    /// Author name
    pub author: String,
    /// Commit timestamp (ISO 8601)
    pub date: String,
    /// The line content
    pub content: String,
}

/// Symbol blame information (Rust MVP: fn, struct, enum, impl, trait)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymbolBlame {
    /// Symbol name
    pub name: String,
    /// Symbol type: "fn", "struct", "enum", "impl", "trait", "mod", "const", "static"
    pub kind: String,
    /// Start line (1-indexed)
    pub start_line: usize,
    /// End line (1-indexed, inclusive)
    pub end_line: usize,
    /// Commit that introduced this symbol (based on first line)
    pub introduced_by: CommitInfo,
    /// Last modification commit (based on any line in symbol)
    pub last_modified_by: Option<CommitInfo>,
}

/// Result of symbol blame analysis for a file
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileSymbolBlame {
    /// File path
    pub path: String,
    /// Language (e.g., "rust")
    pub language: String,
    /// Symbols with blame information
    pub symbols: Vec<SymbolBlame>,
}

impl GitRepo {
    /// Get blame information for a file
    pub fn blame_file(&self, file_path: &Path) -> Result<Vec<BlameEntry>, GitError> {
        let blame = self.repo.blame_file(file_path, None)?;
        let file_content =
            std::fs::read_to_string(self.path.join(file_path)).map_err(GitError::IoError)?;
        let lines: Vec<&str> = file_content.lines().collect();

        let mut entries = Vec::new();

        // Iterate over hunks and expand each to individual line entries
        for hunk in blame.iter() {
            let oid = hunk.final_commit_id();
            let sig = hunk.final_signature();

            // Format timestamp
            let timestamp = sig.when().seconds();
            let date = format_git_timestamp(timestamp);

            let author = sig.name().unwrap_or("Unknown").to_string();
            let commit_hash = oid.to_string();
            let short_hash: String = commit_hash.chars().take(7).collect();

            // Each hunk covers lines from final_start_line to final_start_line + lines_in_hunk - 1
            let start_line = hunk.final_start_line(); // 1-based
            let num_lines = hunk.lines_in_hunk();

            for offset in 0..num_lines {
                let line_num = start_line + offset;
                let line_idx = line_num.saturating_sub(1); // Convert to 0-based index
                if line_idx >= lines.len() {
                    break;
                }

                entries.push(BlameEntry {
                    line: line_num,
                    commit_hash: commit_hash.clone(),
                    short_hash: short_hash.clone(),
                    author: author.clone(),
                    date: date.clone(),
                    content: lines.get(line_idx).unwrap_or(&"").to_string(),
                });
            }
        }

        // Sort by line number to ensure consistent ordering
        entries.sort_by_key(|e| e.line);
        Ok(entries)
    }

    /// Get symbol-level blame for a Rust file (MVP)
    /// Extracts fn, struct, enum, impl, trait, mod, const, static and maps them to commits
    pub fn symbol_blame_rust(&self, file_path: &Path) -> Result<FileSymbolBlame, GitError> {
        use regex::Regex;

        let file_content =
            std::fs::read_to_string(self.path.join(file_path)).map_err(GitError::IoError)?;

        // Get blame for the file
        let blame_entries = self.blame_file(file_path)?;

        // Rust symbol patterns (MVP: simple regex, not full parser)
        // These patterns match the start of a symbol definition
        let symbol_patterns = [
            (r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)", "fn"),
            (r"^\s*(?:pub\s+)?struct\s+(\w+)", "struct"),
            (r"^\s*(?:pub\s+)?enum\s+(\w+)", "enum"),
            (
                r"^\s*impl(?:<[^>]+>)?\s+(?:(\w+)|(?:\w+\s+for\s+(\w+)))",
                "impl",
            ),
            (r"^\s*(?:pub\s+)?trait\s+(\w+)", "trait"),
            (r"^\s*(?:pub\s+)?mod\s+(\w+)", "mod"),
            (r"^\s*(?:pub\s+)?const\s+(\w+)", "const"),
            (r"^\s*(?:pub\s+)?static\s+(\w+)", "static"),
        ];

        let compiled_patterns: Vec<(Regex, &str)> = symbol_patterns
            .iter()
            .filter_map(|(pattern, kind)| Regex::new(pattern).ok().map(|re| (re, *kind)))
            .collect();

        let lines: Vec<&str> = file_content.lines().collect();
        let mut symbols = Vec::new();
        let mut brace_stack = 0;
        let mut current_symbol: Option<(String, String, usize)> = None; // (name, kind, start_line)

        for (line_idx, line) in lines.iter().enumerate() {
            let line_num = line_idx + 1;

            // Check for new symbol definition (only when not inside another symbol)
            if brace_stack == 0 {
                for (re, kind) in &compiled_patterns {
                    if let Some(captures) = re.captures(line) {
                        // Get the first non-None capture group (symbol name)
                        let name = captures
                            .iter()
                            .skip(1)
                            .find_map(|m| m.map(|m| m.as_str().to_string()))
                            .unwrap_or_else(|| format!("anonymous_{}", line_num));

                        current_symbol = Some((name, kind.to_string(), line_num));
                        break;
                    }
                }
            }

            // Track brace nesting
            for ch in line.chars() {
                match ch {
                    '{' => brace_stack += 1,
                    '}' => {
                        if brace_stack > 0 {
                            brace_stack -= 1;
                        }
                        // Symbol ends when braces are balanced
                        if brace_stack == 0
                            && let Some((name, kind, start_line)) = current_symbol.take()
                        {
                            // Find blame for this symbol
                            let introduced_blame =
                                blame_entries.iter().find(|e| e.line == start_line);

                            // Find last modification (latest timestamp in symbol range)
                            let symbol_blames: Vec<_> = blame_entries
                                .iter()
                                .filter(|e| e.line >= start_line && e.line <= line_num)
                                .collect();

                            // Get commit info for introduced_by
                            let introduced_by = if let Some(blame) = introduced_blame {
                                self.get_commit_info(&blame.commit_hash)
                                    .unwrap_or_else(|_| CommitInfo {
                                        hash: blame.commit_hash.clone(),
                                        short_hash: blame.short_hash.clone(),
                                        author: blame.author.clone(),
                                        author_email: String::new(),
                                        date: blame.date.clone(),
                                        timestamp: 0,
                                        message: String::new(),
                                        message_full: String::new(),
                                    })
                            } else {
                                CommitInfo {
                                    hash: "unknown".to_string(),
                                    short_hash: "unknown".to_string(),
                                    author: "Unknown".to_string(),
                                    author_email: String::new(),
                                    date: String::new(),
                                    timestamp: 0,
                                    message: String::new(),
                                    message_full: String::new(),
                                }
                            };

                            // Find last modified (most recent commit in symbol)
                            // ISO 8601 dates can be compared lexicographically
                            let last_modified_by = symbol_blames
                                .iter()
                                .max_by(|a, b| a.date.cmp(&b.date))
                                .and_then(|b| {
                                    if b.commit_hash != introduced_by.hash {
                                        self.get_commit_info(&b.commit_hash).ok()
                                    } else {
                                        None
                                    }
                                });

                            symbols.push(SymbolBlame {
                                name,
                                kind,
                                start_line,
                                end_line: line_num,
                                introduced_by,
                                last_modified_by,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        // Handle symbol without closing brace (e.g., mod declaration without body)
        if let Some((name, kind, start_line)) = current_symbol
            && let Some(blame) = blame_entries.iter().find(|e| e.line == start_line)
        {
            let introduced_by = self
                .get_commit_info(&blame.commit_hash)
                .unwrap_or_else(|_| CommitInfo {
                    hash: blame.commit_hash.clone(),
                    short_hash: blame.short_hash.clone(),
                    author: blame.author.clone(),
                    author_email: String::new(),
                    date: blame.date.clone(),
                    timestamp: 0,
                    message: String::new(),
                    message_full: String::new(),
                });

            symbols.push(SymbolBlame {
                name,
                kind,
                start_line,
                end_line: lines.len(),
                introduced_by,
                last_modified_by: None,
            });
        }

        Ok(FileSymbolBlame {
            path: file_path.to_string_lossy().to_string(),
            language: "rust".to_string(),
            symbols,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn formats_git_timestamp_as_utc_iso_second() {
        assert_eq!(format_git_timestamp(0), "1970-01-01T00:00:00Z");
    }

    fn create_test_repo() -> (TempDir, GitRepo) {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path();

        // Initialize git repo
        Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .unwrap();

        // Configure git user
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

        // Create initial file and commit
        std::fs::write(path.join("main.ts"), "export function main() {}").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Initial commit"])
            .current_dir(path)
            .output()
            .unwrap();

        let repo = GitRepo::discover(path).unwrap();
        (temp_dir, repo)
    }

    #[test]
    #[serial]
    fn test_discover_git_repo() {
        let (temp_dir, repo) = create_test_repo();
        // Canonicalize paths to handle macOS /private/var vs /var symlink
        let expected = temp_dir.path().canonicalize().unwrap();
        let actual = repo.path().canonicalize().unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_discover_non_git_dir_fails() {
        let temp_dir = TempDir::new().unwrap();
        let result = GitRepo::discover(temp_dir.path());
        assert!(matches!(result, Err(GitError::NotARepository(_))));
    }

    // === find_git_root tests ===

    #[test]
    #[serial]
    fn test_find_git_root_from_repo_root() {
        let (temp_dir, _repo) = create_test_repo();
        let root = super::find_git_root(temp_dir.path());
        assert!(root.is_some());
        // Canonicalize to handle macOS /private/var symlink
        let expected = temp_dir.path().canonicalize().unwrap();
        let actual = root.unwrap().canonicalize().unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    #[serial]
    fn test_find_git_root_from_nested_dir() {
        let (temp_dir, _repo) = create_test_repo();
        let path = temp_dir.path();

        // Create deeply nested directory structure
        let nested = path.join("src").join("deep").join("nested").join("dir");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("file.rs"), "// test").unwrap();

        // find_git_root should find the repo root from nested dir
        let root = super::find_git_root(&nested);
        assert!(root.is_some(), "Should find git root from nested directory");

        let expected = temp_dir.path().canonicalize().unwrap();
        let actual = root.unwrap().canonicalize().unwrap();
        assert_eq!(
            actual, expected,
            "Should return the repo root, not the nested dir"
        );
    }

    #[test]
    fn test_find_git_root_non_git_dir() {
        let temp_dir = TempDir::new().unwrap();
        let root = super::find_git_root(temp_dir.path());
        assert!(root.is_none(), "Should return None for non-git directory");
    }

    #[test]
    #[serial]
    fn test_find_git_root_nested_repo_chooses_closest() {
        // Create outer repo
        let outer_dir = TempDir::new().unwrap();
        let outer_path = outer_dir.path();

        Command::new("git")
            .args(["init"])
            .current_dir(outer_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(outer_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(outer_path)
            .output()
            .unwrap();
        std::fs::write(outer_path.join("outer.txt"), "outer").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(outer_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "outer"])
            .current_dir(outer_path)
            .output()
            .unwrap();

        // Create inner repo (nested git repo)
        let inner_path = outer_path.join("packages").join("inner");
        std::fs::create_dir_all(&inner_path).unwrap();

        Command::new("git")
            .args(["init"])
            .current_dir(&inner_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&inner_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&inner_path)
            .output()
            .unwrap();
        std::fs::write(inner_path.join("inner.txt"), "inner").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(&inner_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "inner"])
            .current_dir(&inner_path)
            .output()
            .unwrap();

        // Create deep nested dir inside inner repo
        let deep = inner_path.join("src").join("deep");
        std::fs::create_dir_all(&deep).unwrap();

        // find_git_root from deep should find INNER repo (closest), not outer
        let root = super::find_git_root(&deep);
        assert!(root.is_some(), "Should find git root from nested repo");

        let inner_canon = inner_path.canonicalize().unwrap();
        let found_canon = root.unwrap().canonicalize().unwrap();
        assert_eq!(
            found_canon, inner_canon,
            "Should find closest (inner) repo, not outer"
        );
    }

    #[test]
    #[serial]
    fn test_find_git_root_worktree() {
        // Create main repo
        let main_dir = TempDir::new().unwrap();
        let main_path = main_dir.path();

        Command::new("git")
            .args(["init"])
            .current_dir(main_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(main_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(main_path)
            .output()
            .unwrap();
        std::fs::write(main_path.join("main.txt"), "main").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(main_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(main_path)
            .output()
            .unwrap();

        // Create a branch for the worktree
        Command::new("git")
            .args(["branch", "feature"])
            .current_dir(main_path)
            .output()
            .unwrap();

        // Create worktree in a sibling directory
        let worktree_dir = TempDir::new().unwrap();
        let worktree_path = worktree_dir.path().join("feature-worktree");

        let output = Command::new("git")
            .args([
                "worktree",
                "add",
                worktree_path.to_str().unwrap(),
                "feature",
            ])
            .current_dir(main_path)
            .output()
            .unwrap();

        if !output.status.success() {
            // Skip test if git worktree not supported (old git version)
            eprintln!("Skipping worktree test: git worktree not available");
            return;
        }

        // Verify .git is a file (not directory) in worktree
        let git_path = worktree_path.join(".git");
        assert!(git_path.exists(), "Worktree should have .git");
        assert!(
            git_path.is_file(),
            "Worktree .git should be a file, not directory"
        );

        // find_git_root should work from worktree
        let root = super::find_git_root(&worktree_path);
        assert!(root.is_some(), "Should find git root from worktree");

        let worktree_canon = worktree_path.canonicalize().unwrap();
        let found_canon = root.unwrap().canonicalize().unwrap();
        assert_eq!(
            found_canon, worktree_canon,
            "Should return worktree path as root"
        );

        // Cleanup worktree
        let _ = Command::new("git")
            .args([
                "worktree",
                "remove",
                "--force",
                worktree_path.to_str().unwrap(),
            ])
            .current_dir(main_path)
            .output();
    }

    #[test]
    #[serial]
    fn test_head_commit() {
        let (_temp_dir, repo) = create_test_repo();
        let head = repo.head_commit().unwrap();
        assert_eq!(head.len(), 40); // SHA-1 hash length
    }

    #[test]
    #[serial]
    fn test_resolve_head() {
        let (_temp_dir, repo) = create_test_repo();
        let head = repo.resolve_ref("HEAD").unwrap();
        assert_eq!(head.len(), 40);
    }

    #[test]
    #[serial]
    fn test_get_commit_info() {
        let (_temp_dir, repo) = create_test_repo();
        let info = repo.get_commit_info("HEAD").unwrap();
        assert_eq!(info.author, "Test User");
        assert_eq!(info.message, "Initial commit");
        assert_eq!(info.short_hash.len(), 7);
    }

    #[test]
    #[serial]
    fn test_log() {
        let (_temp_dir, repo) = create_test_repo();
        let commits = repo.log(None, 10).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].message, "Initial commit");
    }

    #[test]
    #[serial]
    fn test_file_content_at() {
        let (_temp_dir, repo) = create_test_repo();
        let content = repo.file_content_at("HEAD", Path::new("main.ts")).unwrap();
        assert_eq!(content, "export function main() {}");
    }

    #[test]
    #[serial]
    fn test_list_files_at() {
        let (_temp_dir, repo) = create_test_repo();
        let files = repo.list_files_at("HEAD").unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0], PathBuf::from("main.ts"));
    }

    #[test]
    #[serial]
    fn test_changed_files() {
        let (temp_dir, repo) = create_test_repo();
        let path = temp_dir.path();

        // Make another commit with a new file
        std::fs::write(path.join("utils.ts"), "export function add() {}").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add utils"])
            .current_dir(path)
            .output()
            .unwrap();

        let changes = repo.changed_files("HEAD~1", "HEAD").unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].status, ChangeStatus::Added);
        assert_eq!(changes[0].new_path, Some(PathBuf::from("utils.ts")));
    }

    #[test]
    fn test_git_error_display_not_a_repository() {
        let err = GitError::NotARepository("/some/path".to_string());
        assert_eq!(format!("{}", err), "not a git repository: /some/path");
    }

    #[test]
    fn test_git_error_display_ref_not_found() {
        let err = GitError::RefNotFound("main".to_string());
        assert_eq!(format!("{}", err), "reference not found: main");
    }

    #[test]
    fn test_git_error_display_operation_failed() {
        let err = GitError::OperationFailed("something went wrong".to_string());
        assert_eq!(
            format!("{}", err),
            "git operation failed: something went wrong"
        );
    }

    #[test]
    fn test_git_error_display_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = GitError::IoError(io_err);
        assert!(format!("{}", err).contains("IO error"));
    }

    #[test]
    fn test_git_error_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let git_err: GitError = io_err.into();
        assert!(matches!(git_err, GitError::IoError(_)));
    }

    #[test]
    fn test_changed_file_path_new_path() {
        let file = ChangedFile {
            old_path: Some(PathBuf::from("old.ts")),
            new_path: Some(PathBuf::from("new.ts")),
            status: ChangeStatus::Renamed,
        };
        assert_eq!(file.path(), Some(Path::new("new.ts")));
    }

    #[test]
    fn test_changed_file_path_old_path_only() {
        let file = ChangedFile {
            old_path: Some(PathBuf::from("deleted.ts")),
            new_path: None,
            status: ChangeStatus::Deleted,
        };
        assert_eq!(file.path(), Some(Path::new("deleted.ts")));
    }

    #[test]
    fn test_changed_file_path_none() {
        let file = ChangedFile {
            old_path: None,
            new_path: None,
            status: ChangeStatus::Modified,
        };
        assert!(file.path().is_none());
    }

    #[test]
    #[serial]
    fn test_resolve_ref_nonexistent() {
        let (_temp_dir, repo) = create_test_repo();
        let result = repo.resolve_ref("nonexistent-branch");
        assert!(matches!(result, Err(GitError::RefNotFound(_))));
    }

    #[test]
    #[serial]
    fn test_resolve_ref_with_commit_hash() {
        let (_temp_dir, repo) = create_test_repo();
        let head = repo.head_commit().unwrap();
        let resolved = repo.resolve_ref(&head).unwrap();
        assert_eq!(resolved, head);
    }

    #[test]
    #[serial]
    fn test_log_with_file_filter() {
        let (temp_dir, repo) = create_test_repo();
        let path = temp_dir.path();

        // Create another file and commit
        std::fs::write(path.join("utils.ts"), "export const x = 1;").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add utils"])
            .current_dir(path)
            .output()
            .unwrap();

        // Log for main.ts should only show initial commit
        let commits = repo.log(Some(Path::new("main.ts")), 10).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].message, "Initial commit");

        // Log for utils.ts should only show second commit
        let commits = repo.log(Some(Path::new("utils.ts")), 10).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].message, "Add utils");
    }

    #[test]
    #[serial]
    fn test_log_limit() {
        let (temp_dir, repo) = create_test_repo();
        let path = temp_dir.path();

        // Create multiple commits
        for i in 1..5 {
            std::fs::write(path.join("main.ts"), format!("version {}", i)).unwrap();
            Command::new("git")
                .args(["add", "."])
                .current_dir(path)
                .output()
                .unwrap();
            Command::new("git")
                .args(["commit", "-m", &format!("Commit {}", i)])
                .current_dir(path)
                .output()
                .unwrap();
        }

        // Limit should work
        let commits = repo.log(None, 2).unwrap();
        assert_eq!(commits.len(), 2);
    }

    #[test]
    #[serial]
    fn test_file_content_at_nonexistent() {
        let (_temp_dir, repo) = create_test_repo();
        let result = repo.file_content_at("HEAD", Path::new("nonexistent.ts"));
        assert!(result.is_err());
    }

    #[test]
    #[serial]
    fn test_changed_files_modified() {
        let (temp_dir, repo) = create_test_repo();
        let path = temp_dir.path();

        // Modify existing file
        std::fs::write(path.join("main.ts"), "export function main() { return 1; }").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Modify main"])
            .current_dir(path)
            .output()
            .unwrap();

        let changes = repo.changed_files("HEAD~1", "HEAD").unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].status, ChangeStatus::Modified);
    }

    #[test]
    #[serial]
    fn test_changed_files_deleted() {
        let (temp_dir, repo) = create_test_repo();
        let path = temp_dir.path();

        // Delete file
        std::fs::remove_file(path.join("main.ts")).unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Delete main"])
            .current_dir(path)
            .output()
            .unwrap();

        let changes = repo.changed_files("HEAD~1", "HEAD").unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].status, ChangeStatus::Deleted);
    }

    #[test]
    #[serial]
    fn test_commit_info_fields() {
        let (_temp_dir, repo) = create_test_repo();
        let info = repo.get_commit_info("HEAD").unwrap();

        // Verify all fields are populated
        assert!(!info.hash.is_empty());
        assert_eq!(info.short_hash.len(), 7);
        assert_eq!(info.author, "Test User");
        assert_eq!(info.author_email, "test@test.com");
        assert!(!info.date.is_empty());
        assert!(info.timestamp > 0);
        assert!(!info.message.is_empty());
        assert!(!info.message_full.is_empty());
    }

    #[test]
    #[serial]
    fn test_list_files_at_multiple() {
        let (temp_dir, repo) = create_test_repo();
        let path = temp_dir.path();

        // Create additional files and commit
        std::fs::create_dir(path.join("src")).unwrap();
        std::fs::write(path.join("src/utils.ts"), "export const x = 1;").unwrap();
        std::fs::write(path.join("config.json"), "{}").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add files"])
            .current_dir(path)
            .output()
            .unwrap();

        let files = repo.list_files_at("HEAD").unwrap();
        assert_eq!(files.len(), 3);
    }

    #[test]
    fn test_change_status_equality() {
        assert_eq!(ChangeStatus::Added, ChangeStatus::Added);
        assert_ne!(ChangeStatus::Added, ChangeStatus::Deleted);
        assert_eq!(ChangeStatus::Modified, ChangeStatus::Modified);
        assert_eq!(ChangeStatus::Renamed, ChangeStatus::Renamed);
        assert_eq!(ChangeStatus::Copied, ChangeStatus::Copied);
    }

    #[test]
    #[serial]
    fn test_create_and_remove_worktree() {
        let (temp_dir, repo) = create_test_repo();
        let path = temp_dir.path();

        // Create initial commit (we're on a default branch)
        let current_branch = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(path)
            .output()
            .unwrap();
        let current_branch = String::from_utf8_lossy(&current_branch.stdout)
            .trim()
            .to_string();

        // Create a new branch from current
        Command::new("git")
            .args(["branch", "test-branch"])
            .current_dir(path)
            .output()
            .unwrap();

        // Add a commit on the new branch
        Command::new("git")
            .args(["checkout", "test-branch"])
            .current_dir(path)
            .output()
            .unwrap();

        std::fs::write(path.join("branch.ts"), "export const test = 1;").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add branch file"])
            .current_dir(path)
            .output()
            .unwrap();

        // Go back to original branch
        Command::new("git")
            .args(["checkout", &current_branch])
            .current_dir(path)
            .output()
            .unwrap();

        // Create worktree
        let worktree_path = temp_dir.path().join("test-worktree");
        let result = repo.create_worktree("test-branch", &worktree_path);
        assert!(result.is_ok(), "Failed to create worktree: {:?}", result);

        // Verify worktree exists and has the branch file
        assert!(worktree_path.exists());
        assert!(worktree_path.join("branch.ts").exists());

        // Remove worktree
        let result = repo.remove_worktree(&worktree_path);
        assert!(result.is_ok(), "Failed to remove worktree: {:?}", result);
    }

    #[test]
    #[serial]
    fn test_create_worktree_nonexistent_branch() {
        let (temp_dir, repo) = create_test_repo();
        let worktree_path = temp_dir.path().join("test-worktree");

        // Try to create worktree for non-existent branch
        let result = repo.create_worktree("nonexistent-branch", &worktree_path);
        assert!(result.is_err());
    }

    fn create_rust_test_repo() -> (TempDir, GitRepo) {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path();

        // Initialize git repo
        Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .unwrap();

        // Configure git user
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

        // Create initial Rust file and commit
        let rust_content = r#"
pub fn hello() {
    println!("Hello");
}

struct Point {
    x: i32,
    y: i32,
}

impl Point {
    fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}
"#;
        std::fs::write(path.join("lib.rs"), rust_content).unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Initial Rust commit"])
            .current_dir(path)
            .output()
            .unwrap();

        let repo = GitRepo::discover(path).unwrap();
        (temp_dir, repo)
    }

    #[test]
    #[serial]
    fn test_blame_file() {
        let (_temp_dir, repo) = create_test_repo();
        let entries = repo.blame_file(Path::new("main.ts")).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].line, 1);
        assert_eq!(entries[0].author, "Test User");
        assert_eq!(entries[0].content, "export function main() {}");
    }

    #[test]
    #[serial]
    fn test_symbol_blame_rust() {
        let (_temp_dir, repo) = create_rust_test_repo();
        let result = repo.symbol_blame_rust(Path::new("lib.rs")).unwrap();

        assert_eq!(result.language, "rust");
        assert_eq!(result.path, "lib.rs");

        // Should find: fn hello, struct Point, impl Point, fn new
        assert!(result.symbols.len() >= 3);

        // Find the hello function
        let hello_fn = result
            .symbols
            .iter()
            .find(|s| s.name == "hello" && s.kind == "fn");
        assert!(hello_fn.is_some());
        let hello_fn = hello_fn.unwrap();
        assert_eq!(hello_fn.introduced_by.author, "Test User");

        // Find the Point struct
        let point_struct = result
            .symbols
            .iter()
            .find(|s| s.name == "Point" && s.kind == "struct");
        assert!(point_struct.is_some());
    }

    #[test]
    #[serial]
    fn test_blame_entry_fields() {
        let (_temp_dir, repo) = create_test_repo();
        let entries = repo.blame_file(Path::new("main.ts")).unwrap();

        let entry = &entries[0];
        assert!(!entry.commit_hash.is_empty());
        assert_eq!(entry.short_hash.len(), 7);
        assert!(!entry.date.is_empty());
    }

    #[test]
    fn test_symbol_blame_serde() {
        let symbol = SymbolBlame {
            name: "test_fn".to_string(),
            kind: "fn".to_string(),
            start_line: 1,
            end_line: 5,
            introduced_by: CommitInfo {
                hash: "abc123".to_string(),
                short_hash: "abc123".to_string(),
                author: "Test".to_string(),
                author_email: "test@test.com".to_string(),
                date: "2025-01-01T00:00:00Z".to_string(),
                timestamp: 0,
                message: "Test".to_string(),
                message_full: "Test".to_string(),
            },
            last_modified_by: None,
        };

        let json = serde_json::to_string(&symbol).unwrap();
        let parsed: SymbolBlame = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test_fn");
        assert_eq!(parsed.kind, "fn");
    }
}
