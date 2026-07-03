use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::json;
use std::io::IsTerminal;

use crate::fs_utils::{
    GitIgnoreChecker, build_ignore_matchers, count_lines, is_allowed_hidden, should_ignore,
    sort_dir_entries,
};
use crate::types::{
    COLOR_RED, COLOR_RESET, Collectors, ColorMode, LargeEntry, LineEntry, Options, OutputMode,
    Stats,
};

fn compile_path_filter(pattern: Option<&str>) -> io::Result<Option<regex::Regex>> {
    pattern
        .map(regex::Regex::new)
        .transpose()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))
}

fn matches_path_filter(entry: &LineEntry, filter: Option<&regex::Regex>) -> bool {
    filter.is_none_or(|regex| regex.is_match(&entry.relative_path))
}

/// List of common build artifact directory names that typically contain
/// millions of files and slow down tools like Spotlight.
const BUILD_ARTIFACT_DIRS: &[&str] = &[
    // JavaScript/Node.js
    "node_modules",
    ".pnpm-store",
    // PHP
    "vendor",
    // Python
    ".venv",
    "venv",
    "env",
    "ENV",
    // Rust
    "target",
    // General build outputs
    "dist",
    "build",
    "out",
    // Testing/Coverage
    "coverage",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    // Java/Gradle
    ".gradle",
    // JavaScript bundlers
    ".parcel-cache",
    ".next",
    ".nuxt",
    ".turbo",
    ".cache",
    // Dart/Flutter
    ".dart_tool",
    // Terraform
    ".terraform",
    ".terraform.d",
    // iOS/macOS
    "Pods",
    "DerivedData",
    // React Native/Expo
    ".expo",
    ".expo-shared",
    // Svelte/Angular/Vercel/Serverless
    ".svelte-kit",
    ".angular",
    ".vercel",
    ".serverless",
];

struct WalkContext<'a> {
    options: &'a Options,
    root_canon: &'a Path,
    git_checker: Option<&'a GitIgnoreChecker>,
}

/// Enumerate the entries of a directory after re-asserting that its
/// canonical form is a descendant of `allowed_root`.
///
/// SaaS-safety helper for [`walk`]: callers have typically already
/// validated `dir` against `allowed_root` upstream, but Semgrep's
/// `tainted-path` analysis only follows local data-flow. The
/// [`crate::fs_utils::SanitizedPath`] gate inside `read_dir_within`
/// re-runs canonicalize + `starts_with` immediately before `read_dir` so
/// the boundary guard sits at the same call site as the I/O sink.
fn read_dir_within_root(allowed_root: &Path, dir: &Path) -> io::Result<std::fs::ReadDir> {
    crate::fs_utils::read_dir_within(allowed_root, dir)
}

fn walk(
    dir: &Path,
    ctx: &WalkContext,
    prefix_parts: &mut Vec<bool>,
    collectors: &mut Collectors,
    depth: usize,
    visited: &mut HashSet<PathBuf>,
) -> io::Result<bool> {
    let dir_canon = dir.canonicalize()?;
    if !dir_canon.starts_with(ctx.root_canon) {
        return Ok(false);
    }
    if !visited.insert(dir_canon.clone()) {
        return Ok(false);
    }

    // SaaS-safety: `ctx.root_canon` arrives from `--root`, `LOCT_CACHE_DIR`,
    // or the MCP payload, so even though `dir_canon` was checked against it
    // a few lines above, that guard is invisible to Semgrep's local
    // `tainted-path` data-flow analysis. `read_dir_within_root` re-runs
    // canonicalize + `starts_with` immediately before the `read_dir` sink
    // so the boundary guard sits at the same call site as the I/O.
    let mut dir_entries: Vec<_> = read_dir_within_root(ctx.root_canon, &dir_canon)?
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let is_hidden = name_str.starts_with('.');
            ctx.options.show_hidden || !is_hidden || is_allowed_hidden(&name_str)
        })
        .collect();

    sort_dir_entries(dir_entries.as_mut_slice());

    let len = dir_entries.len();
    let mut any_included = false;
    for (idx, entry) in dir_entries.into_iter().enumerate() {
        let path = entry.path();
        let is_last = idx + 1 == len;
        let mut prefix = String::new();
        for &has_more in prefix_parts.iter() {
            if has_more {
                prefix.push_str("│   ");
            } else {
                prefix.push_str("    ");
            }
        }
        let branch = if is_last { "└── " } else { "├── " };
        let name = entry.file_name().to_string_lossy().to_string();
        let label = format!("{}{}{}", prefix, branch, name);

        let relative = path
            .canonicalize()
            .unwrap_or_else(|_| path.clone())
            .strip_prefix(ctx.root_canon)
            .unwrap_or(&path)
            .to_path_buf();

        // Handle --find-artifacts mode: find build artifact directories
        if ctx.options.find_artifacts {
            let is_dir = path.is_dir();
            // Skip files - we only care about directories
            if !is_dir {
                continue;
            }
            // Check if this directory is a build artifact
            let is_artifact = BUILD_ARTIFACT_DIRS.contains(&name.as_str());
            if is_artifact {
                // Found an artifact directory - output its path and DON'T recurse into it (prune)
                let relative_display = if relative.as_os_str().is_empty() {
                    name.clone()
                } else {
                    relative.to_string_lossy().to_string()
                };
                collectors.entries.push(LineEntry {
                    label: relative_display.clone(),
                    loc: None,
                    relative_path: relative_display,
                    is_dir: true,
                    is_large: false,
                });
                collectors.stats.directories += 1;
                any_included = true;
                // Don't recurse - prune this directory
                continue;
            }
            // Not an artifact - recurse to find artifacts inside
            if ctx.options.max_depth.is_none_or(|max| depth < max) {
                prefix_parts.push(!is_last);
                let child_has = walk(&path, ctx, prefix_parts, collectors, depth + 1, visited)?;
                prefix_parts.pop();
                if child_has {
                    any_included = true;
                }
            }
            continue;
        }

        // Handle --show-ignored mode: show ONLY gitignored files
        if ctx.options.show_ignored {
            // In show_ignored mode, we want to show files that ARE ignored
            // Check if this file is ignored by gitignore
            let is_gitignored = ctx
                .git_checker
                .map(|checker| checker.is_ignored(&path))
                .unwrap_or(false);
            // Skip files that are NOT ignored (we only want ignored files)
            if !is_gitignored {
                // But still recurse into directories to find ignored files within
                if path.is_dir() && ctx.options.max_depth.is_none_or(|max| depth < max) {
                    prefix_parts.push(!is_last);
                    let _ = walk(&path, ctx, prefix_parts, collectors, depth + 1, visited);
                    prefix_parts.pop();
                }
                continue;
            }
        } else if should_ignore(&path, ctx.options, ctx.git_checker) {
            // Normal mode: skip ignored files
            continue;
        }

        let mut loc = None;
        let is_dir = path.is_dir();
        let mut include_current = false;

        if path.is_file() {
            let ext = path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("")
                .to_lowercase();
            let matches_ext = ctx
                .options
                .extensions
                .as_ref()
                .is_none_or(|set| set.contains(&ext));
            if matches_ext {
                loc = count_lines(&path);
                if let Some(value) = loc {
                    collectors.stats.files += 1;
                    collectors.stats.files_with_loc += 1;
                    collectors.stats.total_loc += value;
                    if value >= ctx.options.loc_threshold {
                        let relative_display = if relative.as_os_str().is_empty() {
                            name.clone()
                        } else {
                            relative.to_string_lossy().to_string()
                        };
                        collectors.large_entries.push(LargeEntry {
                            path: relative_display.clone(),
                            loc: value,
                        });
                    }
                    include_current = true;
                }
            }
        }

        let relative_display = if relative.as_os_str().is_empty() {
            name.clone()
        } else {
            relative.to_string_lossy().to_string()
        };
        let is_large = loc.is_some_and(|v| v >= ctx.options.loc_threshold);

        if is_dir && ctx.options.max_depth.is_none_or(|max| depth < max) {
            // Save position BEFORE recursing so we can insert directory entry
            // before its children (not after, which causes inverted hierarchy)
            let insert_pos = collectors.entries.len();
            prefix_parts.push(!is_last);
            let child_has = walk(&path, ctx, prefix_parts, collectors, depth + 1, visited)?;
            prefix_parts.pop();
            if child_has {
                collectors.stats.directories += 1;
                // Insert directory BEFORE its children (at saved position)
                collectors.entries.insert(
                    insert_pos,
                    LineEntry {
                        label,
                        loc,
                        relative_path: relative_display,
                        is_dir,
                        is_large,
                    },
                );
                any_included = true;
            }
        } else if include_current {
            // Files: push at end (correct order)
            collectors.entries.push(LineEntry {
                label,
                loc,
                relative_path: relative_display,
                is_dir,
                is_large,
            });
            any_included = true;
        }
    }

    Ok(any_included)
}

pub fn run_tree(root_list: &[PathBuf], parsed: &crate::args::ParsedArgs) -> io::Result<()> {
    let path_filter = compile_path_filter(parsed.tree_path_filter.as_deref())?;
    let options = Options {
        extensions: parsed.extensions.clone(),
        ignore_paths: Vec::new(),
        ignore_globs: None,
        use_gitignore: parsed.use_gitignore,
        max_depth: parsed.max_depth,
        color: parsed.color,
        output: parsed.output,
        summary: parsed.summary,
        summary_limit: parsed.summary_limit,
        summary_only: parsed.summary_only,
        show_hidden: parsed.show_hidden,
        show_ignored: parsed.show_ignored,
        loc_threshold: parsed.loc_threshold,
        analyze_limit: parsed.analyze_limit,
        report_path: None,
        serve: false,
        editor_cmd: None,
        max_graph_nodes: parsed.max_graph_nodes,
        max_graph_edges: parsed.max_graph_edges,
        verbose: parsed.verbose,
        scan_all: parsed.scan_all,
        symbol: None,
        impact: None,
        find_artifacts: parsed.find_artifacts,
    };

    let mut json_results = Vec::new();

    for (idx, root_path) in root_list.iter().enumerate() {
        let ignore_matchers = build_ignore_matchers(&parsed.ignore_patterns, root_path);
        let root_canon = root_path
            .canonicalize()
            .unwrap_or_else(|_| root_path.clone());
        let root_options = Options {
            ignore_paths: ignore_matchers.ignore_paths,
            ignore_globs: ignore_matchers.ignore_globs,
            loc_threshold: parsed.loc_threshold,
            ..options.clone()
        };

        let git_checker = if root_options.use_gitignore {
            GitIgnoreChecker::new(root_path)
        } else {
            None
        };

        let mut entries: Vec<LineEntry> = Vec::new();
        let mut large_entries: Vec<LargeEntry> = Vec::new();
        let mut prefix_parts: Vec<bool> = Vec::new();
        let mut stats = Stats::default();
        let mut visited: HashSet<PathBuf> = HashSet::new();

        let mut collectors = Collectors {
            entries: &mut entries,
            large_entries: &mut large_entries,
            stats: &mut stats,
        };

        let walk_ctx = WalkContext {
            options: &root_options,
            root_canon: &root_canon,
            git_checker: git_checker.as_ref(),
        };
        walk(
            root_path,
            &walk_ctx,
            &mut prefix_parts,
            &mut collectors,
            0,
            &mut visited,
        )?;

        // Special output for --find-artifacts: just paths, one per line
        if root_options.find_artifacts {
            for entry in &entries {
                // Output absolute path for easy use with rm/trash commands
                let abs_path = root_canon.join(&entry.relative_path);
                println!("{}", abs_path.display());
            }
            continue;
        }

        let display_entries: Vec<&LineEntry> = entries
            .iter()
            .filter(|entry| matches_path_filter(entry, path_filter.as_ref()))
            .collect();

        if parsed.tree_files_only {
            for entry in display_entries
                .iter()
                .copied()
                .filter(|entry| !entry.is_dir)
            {
                println!("{}", entry.relative_path);
            }
            continue;
        }

        let mut sorted_large = large_entries;
        sorted_large.sort_by_key(|b| std::cmp::Reverse(b.loc));

        let summary = json!({
            "directories": stats.directories,
            "files": stats.files,
            "filesWithLoc": stats.files_with_loc,
            "totalLoc": stats.total_loc,
            "largeFiles": sorted_large
                .iter()
                .take(root_options.summary_limit)
                .map(|e| json!({"path": e.path, "loc": e.loc}))
                .collect::<Vec<_>>()
        });

        if matches!(root_options.output, OutputMode::Json | OutputMode::Jsonl) {
            let entries_json: Vec<_> = if root_options.summary_only {
                sorted_large
                    .iter()
                    .take(root_options.summary_limit)
                    .map(|entry| {
                        json!({
                            "path": entry.path,
                            "type": "file",
                            "loc": entry.loc,
                            "isLarge": true,
                        })
                    })
                    .collect()
            } else {
                display_entries
                    .iter()
                    .map(|entry| {
                        json!({
                            "path": entry.relative_path,
                            "type": if entry.is_dir { "dir" } else { "file" },
                            "loc": entry.loc,
                            "isLarge": entry.is_large,
                        })
                    })
                    .collect()
            };

            let payload = json!({
                "root": root_path,
                "options": {
                    "exts": root_options.extensions.as_ref().map(|set| {
                        let mut exts: Vec<_> = set.iter().cloned().collect();
                        exts.sort();
                        exts
                    }),
                    "ignore": root_options
                        .ignore_paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>(),
                    "maxDepth": root_options.max_depth,
                    "useGitignore": root_options.use_gitignore,
                    "color": match root_options.color {
                        ColorMode::Auto => "auto",
                        ColorMode::Always => "always",
                        ColorMode::Never => "never",
                    },
                    "summary": if root_options.summary {
                        serde_json::Value::from(root_options.summary_limit)
                    } else {
                        serde_json::Value::Bool(false)
                    },
                },
                "summary": summary,
                "entries": entries_json,
            });

            if matches!(root_options.output, OutputMode::Jsonl) {
                match serde_json::to_string(&payload) {
                    Ok(line) => println!("{}", line),
                    Err(err) => {
                        eprintln!("[loctree][warn] failed to serialize JSONL line: {}", err)
                    }
                }
            } else {
                json_results.push(payload);
            }
            continue;
        }

        if root_options.summary_only && matches!(root_options.output, OutputMode::Human) {
            if idx > 0 {
                println!();
            }

            let root_name = root_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| root_path.display().to_string());

            println!("{}/", root_name);
            if sorted_large.is_empty() {
                println!(
                    "No files exceed the large-file threshold ({} LOC).",
                    root_options.loc_threshold
                );
            } else {
                println!(
                    "Top {} files (>= {} LOC):",
                    root_options.summary_limit, root_options.loc_threshold
                );
                for item in sorted_large.iter().take(root_options.summary_limit) {
                    println!("  {} ({} LOC)", item.path, item.loc);
                }
            }
            println!(
                "\nSummary: directories: {}, files: {}, files with LOC: {}, total LOC: {}",
                stats.directories, stats.files, stats.files_with_loc, stats.total_loc
            );
            continue;
        }

        if idx > 0 {
            println!();
        }

        if display_entries.is_empty() {
            println!("{}/ (empty)", root_path.display());
            continue;
        }

        let max_label_len = display_entries
            .iter()
            .map(|entry| entry.label.len())
            .max()
            .unwrap_or(0);
        let root_name = root_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| root_path.display().to_string());

        let color_enabled = matches!(root_options.color, ColorMode::Always)
            || (matches!(root_options.color, ColorMode::Auto) && std::io::stdout().is_terminal());

        println!("{}/", root_name);
        for entry in &display_entries {
            if let Some(loc) = entry.loc {
                let line = format!("{:<width$}  {:>6}", entry.label, loc, width = max_label_len);
                if color_enabled && entry.is_large {
                    println!("{}{}{}", COLOR_RED, line, COLOR_RESET);
                } else {
                    println!("{}", line);
                }
            } else {
                println!("{}", entry.label);
            }
        }

        if !sorted_large.is_empty() {
            println!("\nLarge files (>= {} LOC):", root_options.loc_threshold);
            for item in &sorted_large {
                let summary_line = format!("  {} ({} LOC)", item.path, item.loc);
                if color_enabled {
                    println!("{}{}{}", COLOR_RED, summary_line, COLOR_RESET);
                } else {
                    println!("{}", summary_line);
                }
            }
        }

        if root_options.summary {
            println!(
                "\nSummary: directories: {}, files: {}, files with LOC: {}, total LOC: {}",
                stats.directories, stats.files, stats.files_with_loc, stats.total_loc
            );
            if sorted_large.is_empty() {
                println!("No files exceed the large-file threshold.");
            }
        }
    }

    if matches!(options.output, OutputMode::Json) {
        if json_results.len() == 1 {
            match serde_json::to_string_pretty(&json_results[0]) {
                Ok(out) => println!("{}", out),
                Err(err) => eprintln!("[loctree][warn] failed to serialize JSON: {}", err),
            }
        } else {
            match serde_json::to_string_pretty(&json_results) {
                Ok(out) => println!("{}", out),
                Err(err) => eprintln!("[loctree][warn] failed to serialize JSON: {}", err),
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_tree() -> TempDir {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // Create directories
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("lib")).unwrap();

        // Create files with content
        fs::write(
            root.join("src/main.ts"),
            "export function main() {\n  console.log('hello');\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("src/utils.ts"),
            "export const add = (a: number, b: number) => a + b;\n",
        )
        .unwrap();
        fs::write(
            root.join("lib/helper.ts"),
            "export function help() { return 'help'; }\n",
        )
        .unwrap();

        temp
    }

    fn default_parsed_args() -> crate::args::ParsedArgs {
        crate::args::ParsedArgs::default()
    }

    #[test]
    fn test_run_tree_basic() {
        let temp = create_test_tree();
        let roots = vec![temp.path().to_path_buf()];
        let parsed = default_parsed_args();

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_tree_with_summary() {
        let temp = create_test_tree();
        let roots = vec![temp.path().to_path_buf()];
        let mut parsed = default_parsed_args();
        parsed.summary = true;

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_tree_json_output() {
        let temp = create_test_tree();
        let roots = vec![temp.path().to_path_buf()];
        let mut parsed = default_parsed_args();
        parsed.output = OutputMode::Json;

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_tree_jsonl_output() {
        let temp = create_test_tree();
        let roots = vec![temp.path().to_path_buf()];
        let mut parsed = default_parsed_args();
        parsed.output = OutputMode::Jsonl;

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_tree_with_extension_filter() {
        let temp = create_test_tree();
        let roots = vec![temp.path().to_path_buf()];
        let mut parsed = default_parsed_args();
        parsed.extensions = Some(["ts".to_string()].into_iter().collect());

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_tree_with_max_depth() {
        let temp = create_test_tree();
        let roots = vec![temp.path().to_path_buf()];
        let mut parsed = default_parsed_args();
        parsed.max_depth = Some(1);

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_tree_empty_directory() {
        let temp = TempDir::new().unwrap();
        let roots = vec![temp.path().to_path_buf()];
        let parsed = default_parsed_args();

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_tree_multiple_roots() {
        let temp1 = create_test_tree();
        let temp2 = create_test_tree();
        let roots = vec![temp1.path().to_path_buf(), temp2.path().to_path_buf()];
        let parsed = default_parsed_args();

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_tree_show_hidden() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // Create hidden file
        fs::write(root.join(".hidden.ts"), "const hidden = true;\n").unwrap();
        fs::write(root.join("visible.ts"), "const visible = true;\n").unwrap();

        let roots = vec![temp.path().to_path_buf()];
        let mut parsed = default_parsed_args();
        parsed.show_hidden = true;

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_tree_with_gitignore() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // Create .gitignore
        fs::write(root.join(".gitignore"), "*.log\nnode_modules/\n").unwrap();
        fs::write(root.join("app.ts"), "const app = 'app';\n").unwrap();
        fs::write(root.join("debug.log"), "log content\n").unwrap();

        let roots = vec![temp.path().to_path_buf()];
        let mut parsed = default_parsed_args();
        parsed.use_gitignore = true;

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_tree_with_loc_threshold() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // Create a file with many lines
        let large_content: String = (0..100)
            .map(|i| format!("const line{} = {};\n", i, i))
            .collect();
        fs::write(root.join("large.ts"), large_content).unwrap();

        let roots = vec![temp.path().to_path_buf()];
        let mut parsed = default_parsed_args();
        parsed.loc_threshold = 50;

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_find_artifacts_mode() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // Create build artifact directories
        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::create_dir_all(root.join("dist")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();

        // Add some files
        fs::write(root.join("node_modules/package.json"), "{}").unwrap();
        fs::write(root.join("src/main.ts"), "export default {};\n").unwrap();

        let roots = vec![temp.path().to_path_buf()];
        let mut parsed = default_parsed_args();
        parsed.find_artifacts = true;

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_artifact_dirs_constant() {
        // Verify BUILD_ARTIFACT_DIRS contains expected directories
        assert!(BUILD_ARTIFACT_DIRS.contains(&"node_modules"));
        assert!(BUILD_ARTIFACT_DIRS.contains(&"target"));
        assert!(BUILD_ARTIFACT_DIRS.contains(&"dist"));
        assert!(BUILD_ARTIFACT_DIRS.contains(&".venv"));
        assert!(BUILD_ARTIFACT_DIRS.contains(&"vendor"));
    }

    #[test]
    fn test_run_tree_with_ignore_patterns() {
        let temp = create_test_tree();
        let roots = vec![temp.path().to_path_buf()];
        let mut parsed = default_parsed_args();
        parsed.ignore_patterns = vec!["lib".to_string()];

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_tree_with_color_always() {
        let temp = create_test_tree();
        let roots = vec![temp.path().to_path_buf()];
        let mut parsed = default_parsed_args();
        parsed.color = ColorMode::Always;

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_tree_nested_directories() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // Create deeply nested structure
        fs::create_dir_all(root.join("a/b/c/d")).unwrap();
        fs::write(
            root.join("a/b/c/d/deep.ts"),
            "export const deep = 'deep';\n",
        )
        .unwrap();

        let roots = vec![temp.path().to_path_buf()];
        let parsed = default_parsed_args();

        let result = run_tree(&roots, &parsed);
        assert!(result.is_ok());
    }
}
