//! Git timeline entry points into structural revision comparison.
use crate::cli::command::{GlobalOptions, HistoryOptions};
use crate::cli::dispatch::DispatchResult;
use crate::git::GitRepo;
use std::path::{Component, Path, PathBuf};

fn file_scope(repo: &GitRepo, input: &str) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let root = repo.path().canonicalize().map_err(|e| e.to_string())?;
    let absolute = cwd.join(input);
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    let relative = normalized
        .strip_prefix(&root)
        .map_err(|_| "history file must be inside the repository".to_string())?;
    if relative.as_os_str().is_empty() || normalized.is_dir() {
        return Err("history expects a file, not a directory".to_string());
    }
    Ok(relative.to_path_buf())
}

pub fn handle_history_command(opts: &HistoryOptions, global: &GlobalOptions) -> DispatchResult {
    let result = (|| -> Result<serde_json::Value, String> {
        let repo = GitRepo::discover(Path::new(".")).map_err(|e| e.to_string())?;
        let at = repo.resolve_ref(&opts.at).map_err(|e| e.to_string())?;
        let file = opts
            .file
            .as_deref()
            .map(|path| file_scope(&repo, path))
            .transpose()?;
        let commits = repo
            .log_at(&at, file.as_deref(), opts.limit)
            .map_err(|e| e.to_string())?;
        if file.is_some() && commits.is_empty() {
            return Err("no history for this file at the selected revision".to_string());
        }
        let comparisons = commits
            .iter()
            .map(|commit| {
                let parents = repo.parent_ids(&commit.hash).map_err(|e| e.to_string())?;
                let since = parents.first();
                let command = since.map(|parent| {
                    format!("loct diff --since {} --to {}", parent, commit.hash)
                });
                let boundary = since.is_none().then_some("initial_commit");
                let reason = since.is_none().then_some(
                    "Initial commit has no parent; no commit-to-commit structural comparison is available.",
                );
                Ok(serde_json::json!({
                    "commit": commit.hash,
                    "command": command,
                    "comparison": "first_parent",
                    "parents": parents,
                    "since": since,
                    "to": commit.hash,
                    "boundary": boundary,
                    "reason": reason,
                }))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(
            serde_json::json!({"at": at, "file": file, "commits": commits, "suggested_next": comparisons, "comparison_scope": "repository"}),
        )
    })();
    match result {
        Err(error) => {
            eprintln!("[loct][error] {error}");
            DispatchResult::Exit(1)
        }
        Ok(value) => {
            if global.json {
                match serde_json::to_string_pretty(&value) {
                    Ok(output) => println!("{output}"),
                    Err(error) => {
                        eprintln!("[loct][error] {error}");
                        return DispatchResult::Exit(1);
                    }
                }
            } else {
                println!("Git history at {}", value["at"].as_str().unwrap_or(""));
                if let Some(commits) = value["commits"].as_array() {
                    for commit in commits {
                        println!(
                            "{}  {}  {}",
                            commit["short_hash"].as_str().unwrap_or(""),
                            commit["date"].as_str().unwrap_or(""),
                            commit["message"].as_str().unwrap_or("")
                        );
                    }
                }
                if let Some(steps) = value["suggested_next"].as_array() {
                    println!(
                        "\nChanges IN each commit (first parent -> commit, including merges):"
                    );
                    println!("Diffs compare the whole repository; FILE filters the timeline only.");
                    for step in steps {
                        if let Some(command) = step["command"].as_str() {
                            println!("  {command}");
                        } else if let Some(reason) = step["reason"].as_str() {
                            println!("  {}: {reason}", step["commit"].as_str().unwrap_or(""));
                        }
                    }
                }
            }
            DispatchResult::Exit(0)
        }
    }
}
