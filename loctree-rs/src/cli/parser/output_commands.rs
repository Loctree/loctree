//! Parsers for output/reporting commands: report, findings, info, lint, diff, jq_query.
//!
//! These commands generate reports, output analysis results, and support different formats.

use std::path::PathBuf;

use super::super::command::{
    Command, DiffOptions, FindingsOptions, GlobalOptions, HelpOptions, InfoOptions,
    InsightsOptions, JqQueryOptions, LintOptions, ManifestsOptions, ParsedCommand,
    PipelinesOptions, ReportOptions, SuppressionsOptions,
};
use super::helpers::is_jq_filter;

/// Parse `loct info [path]` command - show snapshot metadata.
pub(super) fn parse_info_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct info - Show snapshot metadata and project info

USAGE:
    loct info [PATH]

ARGUMENTS:
    [PATH]     Root directory to analyze (default: current directory)

OPTIONS:
    --help, -h   Show this help message

EXAMPLES:
    loct info
    loct info src/"
            .to_string());
    }

    let mut opts = InfoOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            _ if !arg.starts_with('-') => {
                opts.root = Some(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'info' command.", arg));
            }
        }
    }

    Ok(Command::Info(opts))
}

/// Parse `loct lint [options]` command - structural lint and policy checks.
pub(super) fn parse_lint_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct lint - Structural lint and policy checks

USAGE:
    loct lint [OPTIONS] [PATHS...]

OPTIONS:
    --entrypoints    Validate entrypoint files exist and are properly configured
    --fail           Exit with code 1 if any violations found (CI mode)
    --sarif          Output in SARIF format (GitHub Code Scanning compatible)
    --tauri          Enable Tauri-specific contract checks (commands, events)
    --deep           Include ts/react/memory lint checks
    --ts             Include TypeScript lint checks
    --react          Include React lint checks
    --memory         Include memory leak lint checks
    --no-duplicates  Hide duplicate export sections in CLI output
    --no-dynamic-imports Hide dynamic import sections in CLI output
    --help, -h       Show this help message

EXAMPLES:
    loct lint
    loct lint --fail"
            .to_string());
    }

    let mut opts = LintOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--entrypoints" => {
                opts.entrypoints = true;
                i += 1;
            }
            "--fail" => {
                opts.fail = true;
                i += 1;
            }
            "--sarif" => {
                opts.sarif = true;
                i += 1;
            }
            "--tauri" => {
                opts.tauri = true;
                i += 1;
            }
            "--deep" => {
                opts.deep = true;
                i += 1;
            }
            "--ts" => {
                opts.ts = true;
                i += 1;
            }
            "--react" => {
                opts.react = true;
                i += 1;
            }
            "--memory" => {
                opts.memory = true;
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
                return Err(format!("Unknown option '{}' for 'lint' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Lint(opts))
}

/// Parse `loct pipelines [options]` command - pipeline summary.
pub(super) fn parse_pipelines_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct pipelines - Pipeline summary (events/commands/risks)

USAGE:
    loct pipelines [PATHS...]

OPTIONS:
    --help, -h   Show this help message

EXAMPLES:
    loct pipelines
    loct pipelines ."
            .to_string());
    }

    let mut opts = PipelinesOptions::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'pipelines' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Pipelines(opts))
}

/// Parse `loct insights [options]` command - AI insights.
pub(super) fn parse_insights_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct insights - AI insights summary

USAGE:
    loct insights [PATHS...]

OPTIONS:
    --help, -h   Show this help message

EXAMPLES:
    loct insights
    loct insights ."
            .to_string());
    }

    let mut opts = InsightsOptions::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'insights' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Insights(opts))
}

/// Parse `loct manifests [options]` command - manifest summaries.
pub(super) fn parse_manifests_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct manifests - Manifest summaries

USAGE:
    loct manifests [PATHS...]

OPTIONS:
    --help, -h   Show this help message

EXAMPLES:
    loct manifests
    loct manifests ."
            .to_string());
    }

    let mut opts = ManifestsOptions::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'manifests' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Manifests(opts))
}

/// Parse `loct findings [options]` command - emit canonical findings JSON.
pub(super) fn parse_findings_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct findings - Emit the canonical findings artifact

USAGE:
    loct findings [OPTIONS] [PATHS...]

DESCRIPTION:
    Writes the canonical findings artifact to stdout as JSON.
    This is the machine-truth surface for dead code, cycles, duplicates,
    entrypoint drift, quick wins, and related health signals.

OPTIONS:
    --summary          Emit health score + counts only
    --help, -h         Show this help message

ARGUMENTS:
    [PATHS...]         Root directories to analyze (default: current directory)

EXAMPLES:
    loct findings
    loct findings --summary
    loct findings . | jq '.dead_parrots | length'"
            .to_string());
    }

    let mut opts = FindingsOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--summary" => {
                opts.summary = true;
                i += 1;
            }
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'findings' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Findings(opts))
}

/// Parse `loct report [options]` command - generate HTML report + cached artifacts.
pub(super) fn parse_report_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct report - Generate HTML report + cached artifacts

USAGE:
    loct report [OPTIONS] [PATHS...]

DESCRIPTION:
    Runs full analysis and writes the full HTML report plus cached artifacts
    such as findings.json, agent.json, analysis.json, and report.sarif.

OPTIONS:
    --output, -o <FILE>  Write HTML report to file (default: auto-generate name)
    --serve              Start HTTP server to view report
    --port <PORT>        Server port (default: 8080, with --serve)
    --editor <EDITOR>    Editor for click-to-open (code, cursor, windsurf, jetbrains)
    --help, -h           Show this help message

EXAMPLES:
    loct report
    loct report --output report.html
    loct report --serve"
            .to_string());
    }

    let mut opts = ReportOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--format" => {
                return Err(
                    "`loct report --format` is not supported. `loct report` writes HTML plus cached artifacts; use `loct findings` or the saved JSON artifacts for machine-readable output.".to_string(),
                );
            }
            "--output" | "-o" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--output requires a file path".to_string())?;
                opts.output = Some(PathBuf::from(value));
                i += 2;
            }
            "--serve" => {
                opts.serve = true;
                i += 1;
            }
            "--port" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--port requires a number".to_string())?;
                opts.port = Some(value.parse().map_err(|_| "--port requires a number")?);
                i += 2;
            }
            "--editor" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--editor requires a value".to_string())?;
                opts.editor = Some(value.clone());
                i += 2;
            }
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'report' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Report(opts))
}

/// Parse `loct diff [options]` command - compare snapshots.
pub(super) fn parse_diff_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct diff - Compare snapshots between branches/commits

USAGE:
    loct diff --since <SNAPSHOT> [--to <SNAPSHOT>] [OPTIONS]
    loct diff <SNAPSHOT1> [SNAPSHOT2]

OPTIONS:
    --since <SNAPSHOT>    Base snapshot to compare from (required)
    --to <SNAPSHOT>       Target snapshot to compare to (default: current working tree)
    --auto-scan-base      Automatically create git worktree and scan target branch
    --changed-files       Show only the changed-file summary for <ref>..HEAD
    --jsonl               Output in JSONL format (one change per line)
    --problems-only       Show only regressions (new dead code, new cycles)
    --include-artifacts   Disable the artifact fence (include exports from
                          generated/vendored files in the diff)
    --help, -h            Show this help message

EXAMPLES:
    loct diff --since main
    loct diff --since HEAD~1"
            .to_string());
    }

    let mut opts = DiffOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--since" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--since requires a snapshot ID or path".to_string())?;
                opts.since = Some(value.clone());
                i += 2;
            }
            "--to" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--to requires a snapshot ID or path".to_string())?;
                opts.to = Some(value.clone());
                i += 2;
            }
            "--auto-scan-base" => {
                opts.auto_scan_base = true;
                i += 1;
            }
            "--changed-files" => {
                opts.changed_files = true;
                i += 1;
            }
            "--jsonl" => {
                opts.jsonl = true;
                i += 1;
            }
            "--problems-only" => {
                opts.problems_only = true;
                i += 1;
            }
            "--include-artifacts" => {
                opts.include_artifacts = true;
                i += 1;
            }
            _ if !arg.starts_with('-') => {
                // First positional arg is --since value
                if opts.since.is_none() {
                    opts.since = Some(arg.clone());
                } else if opts.to.is_none() {
                    opts.to = Some(arg.clone());
                } else {
                    return Err(format!(
                        "Unexpected argument '{}'. diff takes at most two snapshot IDs.",
                        arg
                    ));
                }
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'diff' command.", arg));
            }
        }
    }

    if opts.since.is_none() {
        return Err(
            "'diff' command requires a snapshot ID to compare from.\nUsage: loct diff --since <snapshot-id> [--to <snapshot-id>]"
                .to_string(),
        );
    }

    Ok(Command::Diff(opts))
}

/// Parse `loct suppressions [OPTIONS] [ROOT]` — source-side silencer inventory.
///
/// Literal-only detection wrapper. See
/// `loctree::analyzer::suppression_inventory` module docs for the tier
/// boundary (semantic enrichment lives in paid-tier Wave 7+, NOT here).
pub(super) fn parse_suppressions_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err(
            crate::cli::command::Command::format_command_help("suppressions")
                .unwrap_or("loct suppressions - help unavailable")
                .to_string(),
        );
    }

    let mut opts = SuppressionsOptions::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--type" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--type requires a kind token".to_string())?;
                opts.kinds.push(value.clone());
                i += 2;
            }
            _ if arg.starts_with("--type=") => {
                let value = arg.trim_start_matches("--type=");
                opts.kinds.push(value.to_string());
                i += 1;
            }
            "--summary" => {
                opts.summary = true;
                i += 1;
            }
            "--json" => {
                opts.json = true;
                i += 1;
            }
            "--include-fixtures" => {
                opts.include_fixtures = true;
                i += 1;
            }
            "--no-fixtures" => {
                // Default; accept for ergonomic parity with the explicit flag.
                opts.include_fixtures = false;
                i += 1;
            }
            _ if !arg.starts_with('-') => {
                if opts.root.is_none() {
                    opts.root = Some(std::path::PathBuf::from(arg));
                } else {
                    return Err(format!(
                        "Unexpected positional '{}'. `suppressions` takes at most one ROOT.",
                        arg
                    ));
                }
                i += 1;
            }
            _ => {
                return Err(format!(
                    "Unknown option '{}' for 'suppressions' command.",
                    arg
                ));
            }
        }
    }

    Ok(Command::Suppressions(opts))
}

/// Parse jq-style query command (e.g., `loct '.metadata'`).
pub(super) fn parse_jq_query_command(
    args: &[String],
    global: &GlobalOptions,
) -> Result<ParsedCommand, String> {
    if args.is_empty() {
        return Err("jq query requires a filter expression".to_string());
    }

    let mut opts = JqQueryOptions::default();

    // First arg should be the filter
    let mut i = if is_jq_filter(&args[0]) {
        opts.filter = args[0].clone();
        1
    } else {
        return Err(format!("Expected jq filter expression, got: '{}'", args[0]));
    };

    // Parse remaining jq-specific flags
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-r" | "--raw-output" => {
                opts.raw_output = true;
                i += 1;
            }
            "-c" | "--compact-output" => {
                opts.compact_output = true;
                i += 1;
            }
            "-e" | "--exit-status" => {
                opts.exit_status = true;
                i += 1;
            }
            "--arg" => {
                let name = args
                    .get(i + 1)
                    .ok_or_else(|| "--arg requires a name and value".to_string())?;
                let value = args
                    .get(i + 2)
                    .ok_or_else(|| "--arg requires a name and value".to_string())?;
                opts.string_args.push((name.clone(), value.clone()));
                i += 3;
            }
            "--argjson" => {
                let name = args
                    .get(i + 1)
                    .ok_or_else(|| "--argjson requires a name and JSON value".to_string())?;
                let json_value = args
                    .get(i + 2)
                    .ok_or_else(|| "--argjson requires a name and JSON value".to_string())?;
                opts.json_args.push((name.clone(), json_value.clone()));
                i += 3;
            }
            "--snapshot" => {
                let path = args
                    .get(i + 1)
                    .ok_or_else(|| "--snapshot requires a path".to_string())?;
                opts.snapshot_path = Some(PathBuf::from(path));
                i += 2;
            }
            "--help" | "-h" => {
                return Ok(ParsedCommand::new(
                    Command::Help(HelpOptions {
                        command: Some("jq".to_string()),
                        ..Default::default()
                    }),
                    global.clone(),
                ));
            }
            _ => {
                return Err(format!("Unknown option '{}' for jq query mode", arg));
            }
        }
    }

    Ok(ParsedCommand::new(Command::JqQuery(opts), global.clone()))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_info_command() {
        let args = vec!["src/".into()];
        let result = parse_info_command(&args).unwrap();
        if let Command::Info(opts) = result {
            assert_eq!(opts.root, Some(PathBuf::from("src/")));
        } else {
            panic!("Expected Info command");
        }
    }

    #[test]
    fn test_parse_lint_command() {
        let args = vec!["--fail".into(), "--tauri".into()];
        let result = parse_lint_command(&args).unwrap();
        if let Command::Lint(opts) = result {
            assert!(opts.fail);
            assert!(opts.tauri);
        } else {
            panic!("Expected Lint command");
        }
    }

    #[test]
    fn test_parse_report_command() {
        let args = vec!["--serve".into(), "--port".into(), "9000".into()];
        let result = parse_report_command(&args).unwrap();
        if let Command::Report(opts) = result {
            assert!(opts.serve);
            assert_eq!(opts.port, Some(9000));
        } else {
            panic!("Expected Report command");
        }
    }

    #[test]
    fn test_parse_report_command_rejects_format_flag() {
        let args = vec!["--format".into(), "json".into()];
        let err = parse_report_command(&args).expect_err("report should reject --format");
        assert!(err.contains("loct findings"));
    }

    #[test]
    fn test_parse_findings_command() {
        let args = vec!["--summary".into(), "src/".into()];
        let result = parse_findings_command(&args).unwrap();
        if let Command::Findings(opts) = result {
            assert!(opts.summary);
            assert_eq!(opts.roots, vec![PathBuf::from("src/")]);
        } else {
            panic!("Expected Findings command");
        }
    }

    #[test]
    fn test_parse_diff_command() {
        let args = vec!["--since".into(), "main".into()];
        let result = parse_diff_command(&args).unwrap();
        if let Command::Diff(opts) = result {
            assert_eq!(opts.since, Some("main".into()));
        } else {
            panic!("Expected Diff command");
        }
    }

    #[test]
    fn test_parse_diff_changed_files_flag() {
        let args = vec!["--since".into(), "main".into(), "--changed-files".into()];
        let result = parse_diff_command(&args).unwrap();
        if let Command::Diff(opts) = result {
            assert_eq!(opts.since, Some("main".into()));
            assert!(opts.changed_files);
        } else {
            panic!("Expected Diff command");
        }
    }

    #[test]
    fn test_parse_jq_query_basic() {
        let global = GlobalOptions::default();
        let args = vec![".metadata".into()];
        let result = parse_jq_query_command(&args, &global).unwrap();
        if let Command::JqQuery(opts) = result.command {
            assert_eq!(opts.filter, ".metadata");
            assert!(!opts.raw_output);
            assert!(!opts.compact_output);
        } else {
            panic!("Expected JqQuery command");
        }
    }

    #[test]
    fn test_parse_jq_query_with_flags() {
        let global = GlobalOptions::default();
        let args = vec![".files[]".into(), "-r".into(), "-c".into()];
        let result = parse_jq_query_command(&args, &global).unwrap();
        if let Command::JqQuery(opts) = result.command {
            assert_eq!(opts.filter, ".files[]");
            assert!(opts.raw_output);
            assert!(opts.compact_output);
        } else {
            panic!("Expected JqQuery command");
        }
    }

    #[test]
    fn test_parse_jq_query_with_snapshot() {
        let global = GlobalOptions::default();
        let args = vec![
            ".metadata".into(),
            "--snapshot".into(),
            ".loctree/snap.json".into(),
        ];
        let result = parse_jq_query_command(&args, &global).unwrap();
        if let Command::JqQuery(opts) = result.command {
            assert_eq!(
                opts.snapshot_path,
                Some(PathBuf::from(".loctree/snap.json"))
            );
        } else {
            panic!("Expected JqQuery command");
        }
    }
}
