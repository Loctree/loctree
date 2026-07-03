//! Parsers for scan-related commands: auto, scan, tree.
//!
//! These commands handle the initial codebase scanning and visualization.

use std::path::PathBuf;

use super::super::command::{
    AutoOptions, Command, ScanOptions, TreeOptions, WatchMode, WatchOptions,
};

/// Parse `loct auto [options]` command - the default full analysis command.
pub(super) fn parse_auto_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err(
            "loct auto - Full auto-scan with stack detection (default command)

USAGE:
    loct auto [OPTIONS] [PATHS...]
    loct [OPTIONS] [PATHS...]    # 'auto' is the default command

DESCRIPTION:
    Performs a comprehensive analysis of your codebase:
    - Detects project type and language stack automatically
    - Builds dependency graph and import relationships
    - Analyzes code structure and exports
    - Identifies potential issues (dead code, cycles, etc.)

    This is the default command when no subcommand is specified.

OPTIONS:
    --full-scan          Force full rescan (ignore cache)
    --scan-all           Scan all files including hidden/ignored
    --no-duplicates      Hide duplicate export sections in CLI output
    --no-dynamic-imports Hide dynamic import sections in CLI output
    --help, -h           Show this help message

ARGUMENTS:
    [PATHS...]           Root directories to scan (default: current directory)

EXAMPLES:
    loct                         # Auto-scan current directory
    loct auto                    # Explicit auto command
    loct auto --full-scan        # Force full rescan
    loct auto src/ lib/          # Scan specific directories
    loct context                 # Agent-ready Markdown ContextPack (preferred)
    loct context --json          # Full ContextPack JSON for tooling

See `loct --help-legacy` for deprecated flag migration."
                .to_string(),
        );
    }

    let mut opts = AutoOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--full-scan" => {
                opts.full_scan = true;
                i += 1;
            }
            "--scan-all" => {
                opts.scan_all = true;
                i += 1;
            }
            "--for-agent-feed" => {
                eprintln!(
                    "warning: --for-agent-feed is deprecated; use `loct context --json` for an agent-ready ContextPack."
                );
                opts.for_agent_feed = true;
                i += 1;
            }
            "--agent-json" => {
                eprintln!(
                    "warning: --agent-json is deprecated; use `loct context --json` for a raw ContextPack."
                );
                opts.for_agent_feed = true;
                opts.agent_json = true;
                i += 1;
            }
            "--no-duplicates" => {
                opts.suppress_duplicates = true;
                i += 1;
            }
            "--no-dynamic-imports" => {
                opts.suppress_dynamic = true;
                i += 1;
            }
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'auto' command.", arg));
            }
        }
    }

    // Default to current directory if no roots specified
    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Auto(opts))
}

/// Parse `loct scan [options]` command - build/update snapshot.
pub(super) fn parse_scan_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct scan - Build/update snapshot for current HEAD

USAGE:
    loct scan [OPTIONS] [PATHS...]

DESCRIPTION:
    Scans the codebase and updates the internal snapshot database.
    This command builds the dependency graph, analyzes imports/exports,
    and prepares data for other commands like 'dead', 'cycles', 'tree'.

    Unlike 'auto', this command only builds the snapshot without
    running additional analysis passes.

OPTIONS:
    --full-scan       Force full rescan, ignore cached data
    --scan-all        Include hidden and ignored files
    --watch           Watch for changes and re-scan automatically
    --help, -h        Show this help message

ARGUMENTS:
    [PATHS...]        Root directories to scan (default: current directory)

EXAMPLES:
    loct scan                    # Scan current directory
    loct scan --full-scan        # Force complete rescan
    loct scan src/ lib/          # Scan specific directories
    loct scan --scan-all         # Include all files (even hidden)
    loct scan --watch            # Watch mode with live refresh"
            .to_string());
    }

    let mut opts = ScanOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--full-scan" => {
                opts.full_scan = true;
                i += 1;
            }
            "--scan-all" => {
                opts.scan_all = true;
                i += 1;
            }
            "--watch" => {
                opts.watch = true;
                i += 1;
            }
            "--replace" => {
                opts.replace = true;
                i += 1;
            }
            "--wait" => {
                opts.wait_indefinite = true;
                i += 1;
            }
            s if s.starts_with("--wait=") => {
                let raw = s.trim_start_matches("--wait=");
                let secs: u64 = raw
                    .parse()
                    .map_err(|_| format!("--wait={} must be a non-negative integer", raw))?;
                opts.wait_seconds = Some(secs);
                i += 1;
            }
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'scan' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    if (opts.replace || opts.wait_seconds.is_some() || opts.wait_indefinite) && !opts.watch {
        return Err("--replace and --wait only make sense together with --watch.".to_string());
    }

    Ok(Command::Scan(opts))
}

/// Parse `loct watch [MODE] [OPTIONS] [PATHS...]` — the new shape of `loct scan --watch`.
pub(super) fn parse_watch_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        // Help text is rendered via `Command::format_command_help("watch")`
        // so the parser only has to refuse to keep going.
        return Err(super::super::command::Command::format_command_help("watch")
            .unwrap_or("loct watch — see --help")
            .to_string());
    }

    let mut opts = WatchOptions::default();
    let mut explicit_mode = false;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--dev" => {
                if explicit_mode {
                    return Err("`loct watch`: only one mode flag may be passed".to_string());
                }
                opts.mode = WatchMode::Dev;
                explicit_mode = true;
                i += 1;
            }
            "--bg" => {
                if explicit_mode {
                    return Err("`loct watch`: only one mode flag may be passed".to_string());
                }
                opts.mode = WatchMode::Bg;
                explicit_mode = true;
                i += 1;
            }
            "--lsp" => {
                if explicit_mode {
                    return Err("`loct watch`: only one mode flag may be passed".to_string());
                }
                opts.mode = WatchMode::Lsp;
                explicit_mode = true;
                i += 1;
            }
            "--http" => {
                if explicit_mode {
                    return Err("`loct watch`: only one mode flag may be passed".to_string());
                }
                opts.mode = WatchMode::Http;
                explicit_mode = true;
                i += 1;
            }
            "--report" => {
                if explicit_mode {
                    return Err("`loct watch`: only one mode flag may be passed".to_string());
                }
                opts.mode = WatchMode::Report;
                explicit_mode = true;
                i += 1;
            }
            "--full-scan" => {
                opts.full_scan = true;
                i += 1;
            }
            "--scan-all" => {
                opts.scan_all = true;
                i += 1;
            }
            "--replace" => {
                opts.replace = true;
                i += 1;
            }
            "--wait" => {
                opts.wait_indefinite = true;
                i += 1;
            }
            s if s.starts_with("--wait=") => {
                let raw = s.trim_start_matches("--wait=");
                let secs: u64 = raw
                    .parse()
                    .map_err(|_| format!("--wait={} must be a non-negative integer", raw))?;
                opts.wait_seconds = Some(secs);
                i += 1;
            }
            "--port" => {
                if i + 1 >= args.len() {
                    return Err("--port requires a value (e.g. --port 5174)".to_string());
                }
                let raw = &args[i + 1];
                let port: u16 = raw
                    .parse()
                    .map_err(|_| format!("--port {} must be a u16 (1..65535)", raw))?;
                opts.port = Some(port);
                i += 2;
            }
            s if s.starts_with("--port=") => {
                let raw = s.trim_start_matches("--port=");
                let port: u16 = raw
                    .parse()
                    .map_err(|_| format!("--port={} must be a u16 (1..65535)", raw))?;
                opts.port = Some(port);
                i += 1;
            }
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'watch' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Watch(opts))
}

/// Parse `loct tree [options]` command - display LOC tree.
pub(super) fn parse_tree_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct tree - Display LOC tree / structural overview

USAGE:
    loct tree [OPTIONS] [PATHS...]

DESCRIPTION:
    Displays a hierarchical tree view of your codebase structure,
    annotated with lines of code (LOC) metrics for each directory
    and file. Helps understand code distribution and organization.

    Similar to 'tree' command but with LOC counting and better
    handling of gitignored files.

OPTIONS:
    --depth <N>, -L <N>    Maximum depth to display (default: unlimited)
    --summary [N]          Show summary of top N largest items (default: 5)
    --top [N]              Show only top N largest items (default: 50)
    --loc <N>              Only show items with LOC >= N
    --min-loc <N>          Alias for --loc
    --show-hidden, -H      Include hidden files/directories
    --find-artifacts       Highlight build artifacts and generated files
    --show-ignored         Show gitignored files (normally hidden)
    --files                Print matching file paths only, one per line
    --match <REGEX>        Filter output paths by regex
    --help, -h             Show this help message

ARGUMENTS:
    [PATHS...]             Root directories to analyze (default: current directory)

EXAMPLES:
    loct tree                       # Full tree of current directory
    loct tree --depth 3             # Limit to 3 levels deep
    loct tree --summary             # Show top 5 largest items
    loct tree --summary 10          # Show top 10 largest items
    loct tree --loc 100             # Only show files/dirs with 100+ LOC
    loct tree src/ --show-hidden    # Include hidden files in src
    loct tree server --files --match 'test|route' # Exact file list for report/gate work"
            .to_string());
    }

    let mut opts = TreeOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--depth" | "-L" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--depth requires a value".to_string())?;
                opts.depth = Some(value.parse().map_err(|_| "--depth requires a number")?);
                i += 2;
            }
            "--summary" => {
                if let Some(next) = args.get(i + 1)
                    && !next.starts_with('-')
                {
                    opts.summary = Some(
                        next.parse()
                            .map_err(|_| "--summary value must be a number")?,
                    );
                    i += 2;
                    continue;
                }
                opts.summary = Some(5); // Default summary limit
                i += 1;
            }
            "--top" => {
                if let Some(next) = args.get(i + 1)
                    && !next.starts_with('-')
                {
                    opts.summary = Some(next.parse().map_err(|_| "--top value must be a number")?);
                    i += 2;
                } else {
                    opts.summary = Some(50);
                    i += 1;
                }
                opts.summary_only = true;
            }
            "--loc" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--loc requires a value".to_string())?;
                opts.loc_threshold = Some(value.parse().map_err(|_| "--loc requires a number")?);
                i += 2;
            }
            "--min-loc" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--min-loc requires a value".to_string())?;
                opts.loc_threshold =
                    Some(value.parse().map_err(|_| "--min-loc requires a number")?);
                i += 2;
            }
            "--show-hidden" | "-H" => {
                opts.show_hidden = true;
                i += 1;
            }
            "--find-artifacts" => {
                opts.find_artifacts = true;
                i += 1;
            }
            "--show-ignored" => {
                opts.show_ignored = true;
                i += 1;
            }
            "--files" => {
                opts.files_only = true;
                i += 1;
            }
            "--match" | "--path" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--match requires a regex".to_string())?;
                opts.path_filter = Some(value.clone());
                i += 2;
            }
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'tree' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Tree(opts))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_auto_default() {
        let result = parse_auto_command(&[]).unwrap();
        if let Command::Auto(opts) = result {
            assert!(opts.roots.contains(&PathBuf::from(".")));
            assert!(!opts.full_scan);
        } else {
            panic!("Expected Auto command");
        }
    }

    #[test]
    fn test_parse_auto_with_flags() {
        let args = vec!["--full-scan".into(), "--scan-all".into()];
        let result = parse_auto_command(&args).unwrap();
        if let Command::Auto(opts) = result {
            assert!(opts.full_scan);
            assert!(opts.scan_all);
        } else {
            panic!("Expected Auto command");
        }
    }

    #[test]
    fn test_parse_scan_command() {
        let args = vec!["--full-scan".into()];
        let result = parse_scan_command(&args).unwrap();
        if let Command::Scan(opts) = result {
            assert!(opts.full_scan);
        } else {
            panic!("Expected Scan command");
        }
    }

    #[test]
    fn test_parse_tree_command_with_depth() {
        let args = vec!["--depth".into(), "3".into()];
        let result = parse_tree_command(&args).unwrap();
        if let Command::Tree(opts) = result {
            assert_eq!(opts.depth, Some(3));
        } else {
            panic!("Expected Tree command");
        }
    }

    #[test]
    fn test_parse_tree_command_with_files_and_match() {
        let args = vec![
            "--files".into(),
            "--match".into(),
            "test|route".into(),
            "server".into(),
        ];
        let result = parse_tree_command(&args).unwrap();
        if let Command::Tree(opts) = result {
            assert!(opts.files_only);
            assert_eq!(opts.path_filter, Some("test|route".to_string()));
            assert_eq!(opts.roots, vec![PathBuf::from("server")]);
        } else {
            panic!("Expected Tree command");
        }
    }

    #[test]
    fn test_parse_scan_watch_with_replace() {
        let args = vec!["--watch".into(), "--replace".into()];
        let result = parse_scan_command(&args).unwrap();
        if let Command::Scan(opts) = result {
            assert!(opts.watch);
            assert!(opts.replace);
        } else {
            panic!("Expected Scan command");
        }
    }

    #[test]
    fn test_parse_scan_watch_with_wait_seconds() {
        let args = vec!["--watch".into(), "--wait=30".into()];
        let result = parse_scan_command(&args).unwrap();
        if let Command::Scan(opts) = result {
            assert!(opts.watch);
            assert_eq!(opts.wait_seconds, Some(30));
            assert!(!opts.wait_indefinite);
        } else {
            panic!("Expected Scan command");
        }
    }

    #[test]
    fn test_parse_scan_watch_with_wait_indefinite() {
        let args = vec!["--watch".into(), "--wait".into()];
        let result = parse_scan_command(&args).unwrap();
        if let Command::Scan(opts) = result {
            assert!(opts.wait_indefinite);
            assert_eq!(opts.wait_seconds, None);
        } else {
            panic!("Expected Scan command");
        }
    }

    #[test]
    fn test_parse_scan_rejects_replace_without_watch() {
        let args = vec!["--replace".into()];
        let err = parse_scan_command(&args).unwrap_err();
        assert!(
            err.contains("--watch"),
            "error should explain the constraint, got: {err}"
        );
    }

    #[test]
    fn test_parse_watch_default_is_dev_foreground() {
        let result = parse_watch_command(&[]).unwrap();
        if let Command::Watch(opts) = result {
            assert_eq!(opts.mode, WatchMode::Dev);
            assert!(opts.roots.contains(&PathBuf::from(".")));
            assert!(!opts.replace);
            assert!(!opts.wait_indefinite);
        } else {
            panic!("Expected Watch command");
        }
    }

    #[test]
    fn test_parse_watch_modes() {
        for (flag, expected) in [
            ("--dev", WatchMode::Dev),
            ("--bg", WatchMode::Bg),
            ("--lsp", WatchMode::Lsp),
            ("--http", WatchMode::Http),
            ("--report", WatchMode::Report),
        ] {
            let result = parse_watch_command(&[flag.into()]).unwrap();
            if let Command::Watch(opts) = result {
                assert_eq!(opts.mode, expected, "for flag {flag}");
            } else {
                panic!("Expected Watch command for {flag}");
            }
        }
    }

    #[test]
    fn test_parse_watch_rejects_two_mode_flags() {
        let args = vec!["--bg".into(), "--lsp".into()];
        let err = parse_watch_command(&args).unwrap_err();
        assert!(
            err.contains("only one mode"),
            "expected mode-conflict error, got: {err}"
        );
    }

    #[test]
    fn test_parse_watch_with_replace_and_wait() {
        let args = vec!["--bg".into(), "--replace".into(), "--wait=15".into()];
        let result = parse_watch_command(&args).unwrap();
        if let Command::Watch(opts) = result {
            assert_eq!(opts.mode, WatchMode::Bg);
            assert!(opts.replace);
            assert_eq!(opts.wait_seconds, Some(15));
        } else {
            panic!("Expected Watch command");
        }
    }

    #[test]
    fn test_parse_watch_with_positional_path() {
        let args = vec!["--dev".into(), "src/".into()];
        let result = parse_watch_command(&args).unwrap();
        if let Command::Watch(opts) = result {
            assert_eq!(opts.roots, vec![PathBuf::from("src/")]);
        } else {
            panic!("Expected Watch command");
        }
    }
}
