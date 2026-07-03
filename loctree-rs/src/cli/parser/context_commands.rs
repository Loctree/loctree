//! Parsers for context extraction commands: slice, trace, focus, coverage, hotspots.
//!
//! These commands extract specific views/slices of the codebase for analysis.

use std::path::PathBuf;

use super::super::command::{
    Command, ContextOptions, CoverageOptions, FocusOptions, FollowOptions, HotspotsOptions,
    RepoViewOptions, SliceOptions, TraceOptions,
};

/// Parse `loct slice <target> [options]` command - extract file + dependencies.
pub(super) fn parse_slice_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct slice - Extract file + dependencies for AI context

USAGE:
    loct slice <TARGET_PATH> [OPTIONS]

OPTIONS:
    --consumers, -c      Include reverse dependencies (default; compatibility no-op)
    --no-consumers       Hide reverse dependencies (old leaf-only behavior)
    --depth <N>          Maximum dependency depth to traverse (default: unlimited)
    --root <PATH>        Project root for resolving relative imports
    --rescan             Force snapshot update before slicing
    --help, -h           Show this help message

EXAMPLES:
    loct slice src/main.rs
    loct slice src/utils.ts --no-consumers"
            .to_string());
    }

    let mut opts = SliceOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--consumers" | "-c" => {
                opts.consumers = true;
                i += 1;
            }
            "--no-consumers" => {
                opts.consumers = false;
                i += 1;
            }
            "--depth" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--depth requires a value".to_string())?;
                opts.depth = Some(value.parse().map_err(|_| "--depth requires a number")?);
                i += 2;
            }
            "--root" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--root requires a path".to_string())?;
                opts.root = Some(PathBuf::from(value));
                i += 2;
            }
            "--rescan" => {
                opts.rescan = true;
                i += 1;
            }
            _ if !arg.starts_with('-') => {
                if opts.target.is_empty() {
                    opts.target = arg.clone();
                } else {
                    return Err(format!(
                        "Unexpected argument '{}'. slice takes one target path.",
                        arg
                    ));
                }
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'slice' command.", arg));
            }
        }
    }

    if opts.target.is_empty() {
        return Err(
            "'slice' command requires a target file path. Usage: loct slice <path>".to_string(),
        );
    }

    Ok(Command::Slice(opts))
}

/// Parse `loct context [options]` command - produce an agent context pack.
pub(super) fn parse_context_command(args: &[String]) -> Result<Command, String> {
    let mut opts = ContextOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--file" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--file requires a path".to_string())?;
                opts.file = Some(PathBuf::from(value));
                i += 2;
            }
            "--changed" => {
                opts.changed = true;
                i += 1;
            }
            "--task" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--task requires a value".to_string())?;
                opts.task = Some(value.clone());
                i += 2;
            }
            "--scope" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--scope requires a selector or named scope".to_string())?;
                opts.scopes.push(value.clone());
                i += 2;
            }
            "--with-aicx" => {
                opts.with_aicx = true;
                opts.no_aicx = false;
                i += 1;
            }
            "--no-aicx" => {
                opts.no_aicx = true;
                opts.with_aicx = false;
                i += 1;
            }
            "--project" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--project requires a path".to_string())?;
                opts.project = Some(PathBuf::from(value));
                i += 2;
            }
            "--aicx-project" | "--aicx-bucket" => {
                // Plan L04 / Finding #16 — operator override for the AICX
                // project bucket. Distinct from `--project` (path);
                // `--aicx-project` takes the bucket NAME aicx itself uses.
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--aicx-project requires a bucket name".to_string())?;
                opts.aicx_project_override = Some(value.clone());
                i += 2;
            }
            "--json" => {
                opts.json = true;
                i += 1;
            }
            "--full" => {
                // Cut 11: opt in to the full ContextPack. JSON is the default
                // machine format; `--full --markdown` renders the same pack
                // for humans. Pill markdown remains the zero-flag default.
                opts.full = true;
                i += 1;
            }
            "--markdown" | "--md" => {
                opts.markdown = true;
                i += 1;
            }
            "--format" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--format requires a value (markdown or json)".to_string())?;
                match value.as_str() {
                    "markdown" | "md" => opts.markdown = true,
                    "json" => opts.json = true,
                    other => {
                        return Err(format!(
                            "Unknown format '{}', expected markdown or json",
                            other
                        ));
                    }
                }
                i += 2;
            }
            _ if !arg.starts_with('-') => {
                if opts.file.is_none() {
                    let path = PathBuf::from(arg);
                    if opts.project.is_none() && path.is_dir() {
                        opts.project = Some(path);
                    } else {
                        opts.file = Some(path);
                    }
                    i += 1;
                } else {
                    return Err(format!(
                        "Unexpected argument '{}'. Use --task for task text or --file for a target file.",
                        arg
                    ));
                }
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'context' command.", arg));
            }
        }
    }

    Ok(Command::Context(opts))
}

/// Parse `loct repo-view [project]` command - agent-ready repo overview.
pub(super) fn parse_repo_view_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct repo-view - Repository overview for AI agents

USAGE:
    loct repo-view [PROJECT]

ARGUMENTS:
    [PROJECT]       Project root to analyze (default: current directory)

DESCRIPTION:
    First-class CLI counterpart of the MCP repo-view tool, with an optional
    project path. For full agent-ready context prefer `loct context`.

EXAMPLES:
    loct repo-view
    loct repo-view /path/to/project"
            .to_string());
    }

    let mut opts = RepoViewOptions::default();
    for arg in args {
        if arg.starts_with('-') {
            return Err(format!("Unknown option '{}' for 'repo-view' command.", arg));
        }
        if opts.project.is_some() {
            return Err(format!(
                "Unexpected argument '{}'. repo-view takes at most one project path.",
                arg
            ));
        }
        opts.project = Some(PathBuf::from(arg));
    }

    Ok(Command::RepoView(opts))
}

/// Parse `loct follow [scope] [options] [roots...]` command.
pub(super) fn parse_follow_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct follow - Unified signal follower

USAGE:
    loct follow [SCOPE] [OPTIONS] [PATHS...]

SCOPES:
    all, dead, cycles, twins, hotspots, trace, commands, events, pipelines

OPTIONS:
    --handler <NAME>     Handler name for trace scope
    --limit <N>          Limit results where the underlying scope supports it
    --help, -h           Show this help message

EXAMPLES:
    loct follow
    loct follow dead
    loct follow cycles --limit 20
    loct follow trace --handler my_command"
            .to_string());
    }

    let valid_scopes = [
        "all",
        "dead",
        "cycles",
        "twins",
        "hotspots",
        "trace",
        "commands",
        "events",
        "pipelines",
    ];
    let mut opts = FollowOptions::default();
    let mut scope_seen = false;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--handler" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--handler requires a value".to_string())?;
                opts.handler = Some(value.clone());
                i += 2;
            }
            "--limit" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--limit requires a number".to_string())?;
                opts.limit = Some(value.parse().map_err(|_| "--limit requires a number")?);
                i += 2;
            }
            _ if !arg.starts_with('-') && !scope_seen && valid_scopes.contains(&arg.as_str()) => {
                opts.scope = arg.clone();
                scope_seen = true;
                i += 1;
            }
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'follow' command.", arg));
            }
        }
    }

    if opts.scope == "trace" && opts.handler.is_none() {
        return Err("follow trace requires --handler <name>".to_string());
    }

    Ok(Command::Follow(opts))
}

/// Parse `loct trace <handler> [roots]` command - trace Tauri/IPC handler.
pub(super) fn parse_trace_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct trace - Trace a Tauri/IPC handler end-to-end

USAGE:
    loct trace <handler> [ROOTS...]
    loct trace --handler <handler> [ROOTS...]

ARGUMENTS:
    <handler>         Handler name to trace (required)
    [ROOTS...]        Root directories to scan (default: current directory)

EXAMPLES:
    loct trace toggle_assistant
    loct trace --handler toggle_assistant
    loct trace standard_command apps/desktop"
            .to_string());
    }

    let mut opts = TraceOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--handler" => {
                i += 1;
                let Some(handler) = args.get(i) else {
                    return Err("--handler requires a value".to_string());
                };
                opts.handler = handler.clone();
            }
            flag if flag.starts_with('-') => {
                return Err(format!("Unknown option '{}' for 'trace' command.", arg));
            }
            positional if opts.handler.is_empty() => {
                opts.handler = positional.to_string();
            }
            root => {
                opts.roots.push(PathBuf::from(root));
            }
        }
        i += 1;
    }

    if opts.handler.is_empty() {
        return Err(
            "trace requires a handler name. Usage: loct trace <handler> [ROOTS...]".to_string(),
        );
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Trace(opts))
}

/// Parse `loct focus <dir> [options]` command - focus on specific directory.
pub(super) fn parse_focus_command(args: &[String]) -> Result<Command, String> {
    if args.is_empty() {
        return Err("focus requires a target directory. Usage: loct focus <dir>".to_string());
    }

    // Check for --help first
    if args.iter().any(|a| a == "--help" || a == "-h")
        && let Some(help) = Command::format_command_help("focus")
    {
        println!("{}", help);
        std::process::exit(0);
    }

    let mut opts = FocusOptions::default();

    // First positional argument is the target directory
    if !args[0].starts_with('-') {
        opts.target = args[0].clone();
    } else {
        return Err("focus requires a target directory as first argument".to_string());
    }

    let mut i = 1;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--consumers" | "-c" => {
                opts.consumers = true;
                i += 1;
            }
            "--no-consumers" => {
                opts.consumers = false;
                i += 1;
            }
            "--depth" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--depth requires a value".to_string())?;
                opts.depth =
                    Some(value.parse::<usize>().map_err(|_| {
                        format!("Invalid depth value '{}', expected a number", value)
                    })?);
                i += 2;
            }
            "--root" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--root requires a path".to_string())?;
                opts.root = Some(PathBuf::from(value));
                i += 2;
            }
            "--json" => {
                opts.json = true;
                i += 1;
            }
            "--markdown" | "--md" => {
                opts.markdown = true;
                i += 1;
            }
            "--format" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--format requires a value (markdown or json)".to_string())?;
                match value.as_str() {
                    "markdown" | "md" => opts.markdown = true,
                    "json" => opts.json = true,
                    other => {
                        return Err(format!(
                            "Unknown format '{}', expected markdown or json",
                            other
                        ));
                    }
                }
                i += 2;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'focus' command.", arg));
            }
        }
    }

    Ok(Command::Focus(opts))
}

/// Parse `loct coverage [options]` command - analyze test coverage gaps.
pub(super) fn parse_coverage_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct coverage - Analyze test coverage gaps

USAGE:
    loct coverage [OPTIONS] [PATHS...]

OPTIONS:
    --handlers       Show only handler coverage gaps
    --events         Show only event coverage gaps
    --tests          Show structural test coverage report
    --gaps           Show coverage gap analysis (default)
    --min-severity <LEVEL>
                     Filter by minimum severity (critical/high/medium/low)
    --include-artifacts
                     Disable the artifact fence (show findings from
                     vendored/minified/fixture/generated/template files)
    --json           Output as JSON
    --help, -h       Show this help message

EXAMPLES:
    loct coverage
    loct coverage --handlers"
            .to_string());
    }

    let mut opts = CoverageOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--handlers" | "--handlers-only" => {
                opts.handlers_only = true;
                i += 1;
            }
            "--events" | "--events-only" => {
                opts.events_only = true;
                i += 1;
            }
            "--tests" => {
                opts.tests = true;
                i += 1;
            }
            "--gaps" => {
                opts.gaps = true;
                i += 1;
            }
            "--include-artifacts" => {
                opts.include_artifacts = true;
                i += 1;
            }
            "--min-severity" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    "--min-severity requires a value (critical/high/medium/low)".to_string()
                })?;
                opts.min_severity = Some(value.clone());
                i += 2;
            }
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'coverage' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Coverage(opts))
}

/// Parse `loct hotspots [options]` command - find high-impact files.
pub(super) fn parse_hotspots_command(args: &[String]) -> Result<Command, String> {
    // Check for --help first
    if args.iter().any(|a| a == "--help" || a == "-h")
        && let Some(help) = Command::format_command_help("hotspots")
    {
        println!("{}", help);
        std::process::exit(0);
    }

    let mut opts = HotspotsOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--min" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--min requires a value".to_string())?;
                opts.min_imports =
                    Some(value.parse::<usize>().map_err(|_| {
                        format!("Invalid min value '{}', expected a number", value)
                    })?);
                i += 2;
            }
            "--limit" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--limit requires a value".to_string())?;
                opts.limit =
                    Some(value.parse::<usize>().map_err(|_| {
                        format!("Invalid limit value '{}', expected a number", value)
                    })?);
                i += 2;
            }
            "--leaves" => {
                opts.leaves_only = true;
                i += 1;
            }
            "--coupling" => {
                opts.coupling = true;
                i += 1;
            }
            "--root" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--root requires a path".to_string())?;
                opts.root = Some(PathBuf::from(value));
                i += 2;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'hotspots' command.", arg));
            }
        }
    }

    Ok(Command::Hotspots(opts))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_slice_command() {
        let args = vec!["src/main.rs".into(), "--consumers".into()];
        let result = parse_slice_command(&args).unwrap();
        if let Command::Slice(opts) = result {
            assert_eq!(opts.target, "src/main.rs");
            assert!(opts.consumers);
        } else {
            panic!("Expected Slice command");
        }
    }

    #[test]
    fn test_parse_trace_command() {
        let args = vec!["toggle_assistant".into(), "app".into()];
        let result = parse_trace_command(&args).unwrap();
        if let Command::Trace(opts) = result {
            assert_eq!(opts.handler, "toggle_assistant");
            assert_eq!(opts.roots, vec![PathBuf::from("app")]);
        } else {
            panic!("Expected Trace command");
        }
    }

    #[test]
    fn test_parse_trace_command_handler_flag() {
        let args = vec!["--handler".into(), "toggle_assistant".into(), "app".into()];
        let result = parse_trace_command(&args).unwrap();
        if let Command::Trace(opts) = result {
            assert_eq!(opts.handler, "toggle_assistant");
            assert_eq!(opts.roots, vec![PathBuf::from("app")]);
        } else {
            panic!("Expected Trace command");
        }
    }

    #[test]
    fn test_parse_context_command() {
        let args = vec![
            "--file".into(),
            "Cargo.toml".into(),
            "--changed".into(),
            "--task".into(),
            "fix exports".into(),
            "--with-aicx".into(),
            "--project".into(),
            ".".into(),
            "--json".into(),
        ];
        let result = parse_context_command(&args).unwrap();
        if let Command::Context(opts) = result {
            assert_eq!(opts.file, Some(PathBuf::from("Cargo.toml")));
            assert!(opts.changed);
            assert_eq!(opts.task.as_deref(), Some("fix exports"));
            assert!(opts.with_aicx);
            assert!(!opts.no_aicx);
            assert_eq!(opts.project, Some(PathBuf::from(".")));
            assert!(opts.json);
            assert!(!opts.full);
        } else {
            panic!("Expected Context command");
        }
    }

    #[test]
    fn test_parse_context_command_accepts_format_flag_for_cli_mcp_parity() {
        // W5.5 regression: --format (markdown|json) must work for CLI/MCP flag parity
        // (previously "Unknown option --format" while MCP context supported format).
        let args = vec![
            "--format".into(),
            "markdown".into(),
            "--file".into(),
            "src/lib.rs".into(),
        ];
        let result = parse_context_command(&args).unwrap();
        if let Command::Context(opts) = result {
            assert!(opts.markdown);
            assert_eq!(opts.file, Some(PathBuf::from("src/lib.rs")));
        } else {
            panic!("Expected Context command");
        }

        let argsj = vec!["--format".into(), "json".into()];
        let rj = parse_context_command(&argsj).unwrap();
        if let Command::Context(opts) = rj {
            assert!(opts.json);
        } else {
            panic!("Expected Context command");
        }
    }

    #[test]
    fn test_parse_context_scope_flags_are_repeatable() {
        let args = vec![
            "--scope".into(),
            "path:loctree-rs/src/cli/".into(),
            "--scope".into(),
            "tag:cli".into(),
        ];
        let result = parse_context_command(&args).unwrap();
        if let Command::Context(opts) = result {
            assert_eq!(
                opts.scopes,
                vec![
                    "path:loctree-rs/src/cli/".to_string(),
                    "tag:cli".to_string(),
                ]
            );
        } else {
            panic!("Expected Context command");
        }
    }

    #[test]
    fn test_parse_context_full_flag() {
        let args = vec!["--full".into(), "--markdown".into()];
        let result = parse_context_command(&args).unwrap();
        if let Command::Context(opts) = result {
            assert!(opts.full);
            assert!(opts.markdown);
        } else {
            panic!("Expected Context command");
        }
    }

    #[test]
    fn test_parse_context_no_aicx() {
        let result = parse_context_command(&["--no-aicx".into()]).unwrap();
        if let Command::Context(opts) = result {
            assert!(opts.no_aicx);
            assert!(!opts.with_aicx);
        } else {
            panic!("Expected Context command");
        }
    }

    #[test]
    fn test_parse_context_aicx_project_override() {
        // Plan L04 / Finding #16 — operator-supplied AICX bucket override.
        let args = vec![
            "--with-aicx".into(),
            "--aicx-project".into(),
            "loctree-suite".into(),
        ];
        let result = parse_context_command(&args).unwrap();
        if let Command::Context(opts) = result {
            assert!(opts.with_aicx);
            assert_eq!(
                opts.aicx_project_override.as_deref(),
                Some("loctree-suite"),
                "operator override flag must populate aicx_project_override"
            );
            // --project (path) is independent and still None.
            assert!(opts.project.is_none());
        } else {
            panic!("Expected Context command");
        }
    }

    #[test]
    fn test_parse_context_aicx_bucket_alias() {
        // `--aicx-bucket` alias accepted for the same intent.
        let args = vec!["--aicx-bucket".into(), "monorepo-frontend".into()];
        let result = parse_context_command(&args).unwrap();
        if let Command::Context(opts) = result {
            assert_eq!(
                opts.aicx_project_override.as_deref(),
                Some("monorepo-frontend")
            );
        } else {
            panic!("Expected Context command");
        }
    }

    #[test]
    fn test_parse_repo_view_command_with_project() {
        let args = vec!["/tmp/project".into()];
        let result = parse_repo_view_command(&args).unwrap();
        if let Command::RepoView(opts) = result {
            assert_eq!(opts.project, Some(PathBuf::from("/tmp/project")));
        } else {
            panic!("Expected RepoView command");
        }
    }

    #[test]
    fn test_parse_follow_command_with_scope_handler_and_limit() {
        let args = vec![
            "trace".into(),
            "--handler".into(),
            "my_command".into(),
            "--limit".into(),
            "20".into(),
            "app".into(),
        ];
        let result = parse_follow_command(&args).unwrap();
        if let Command::Follow(opts) = result {
            assert_eq!(opts.scope, "trace");
            assert_eq!(opts.handler.as_deref(), Some("my_command"));
            assert_eq!(opts.limit, Some(20));
            assert_eq!(opts.roots, vec![PathBuf::from("app")]);
        } else {
            panic!("Expected Follow command");
        }
    }

    #[test]
    fn test_parse_focus_command() {
        let args = vec!["src/ui".into(), "--consumers".into()];
        let result = parse_focus_command(&args).unwrap();
        if let Command::Focus(opts) = result {
            assert_eq!(opts.target, "src/ui");
            assert!(opts.consumers);
        } else {
            panic!("Expected Focus command");
        }
    }

    #[test]
    fn test_parse_focus_command_accepts_format_for_cli_mcp_parity() {
        // W5.5 regression fixture: --format must not be "Unknown option" (drift vs MCP)
        let args = vec!["src/ui".into(), "--format".into(), "markdown".into()];
        let result = parse_focus_command(&args).unwrap();
        if let Command::Focus(opts) = result {
            assert_eq!(opts.target, "src/ui");
            assert!(opts.markdown);
        } else {
            panic!("Expected Focus command");
        }

        let args2 = vec![".".into(), "--format".into(), "json".into()];
        let result2 = parse_focus_command(&args2).unwrap();
        if let Command::Focus(opts) = result2 {
            assert!(opts.json);
        } else {
            panic!("Expected Focus command");
        }
    }

    #[test]
    fn test_parse_coverage_command() {
        let args = vec!["--handlers".into()];
        let result = parse_coverage_command(&args).unwrap();
        if let Command::Coverage(opts) = result {
            assert!(opts.handlers_only);
        } else {
            panic!("Expected Coverage command");
        }
    }

    #[test]
    fn test_parse_hotspots_command() {
        let args = vec!["--min".into(), "5".into(), "--limit".into(), "10".into()];
        let result = parse_hotspots_command(&args).unwrap();
        if let Command::Hotspots(opts) = result {
            assert_eq!(opts.min_imports, Some(5));
            assert_eq!(opts.limit, Some(10));
        } else {
            panic!("Expected Hotspots command");
        }
    }
}
