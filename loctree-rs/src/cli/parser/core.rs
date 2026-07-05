//! Core parsing logic: syntax detection, global options extraction, and main entry point.
//!
//! This module contains the main `parse_command` function that serves as the entry point
//! for the new CLI parser, along with syntax detection and global option handling.

use std::path::PathBuf;

use super::super::command::{Command, GlobalOptions, HelpOptions, ParsedCommand};
use super::analysis_commands::{
    parse_body_command, parse_cycles_command, parse_dead_command, parse_find_command,
    parse_impact_command, parse_occurrences_command, parse_query_command, parse_twins_command,
};
use super::context_commands::{
    parse_context_command, parse_coverage_command, parse_focus_command, parse_follow_command,
    parse_hotspots_command, parse_repo_view_command, parse_slice_command, parse_trace_command,
};
use super::helpers::{SUBCOMMANDS, is_jq_filter, parse_color_mode, suggest_similar_command};
use super::misc_commands::{
    parse_audit_command, parse_cache_command, parse_crowd_command, parse_dist_command,
    parse_doctor_command, parse_env_truth_command, parse_health_command, parse_help_command,
    parse_layoutmap_command, parse_plan_command, parse_prism_command,
    parse_prune_old_artifacts_command, parse_suppress_command, parse_tagmap_command,
};
use super::output_commands::{
    parse_diff_command, parse_findings_command, parse_info_command, parse_insights_command,
    parse_jq_query_command, parse_lint_command, parse_manifests_command, parse_pipelines_command,
    parse_report_command, parse_suppressions_command,
};
use super::scan_commands::{
    parse_auto_command, parse_scan_command, parse_tree_command, parse_watch_command,
};
use super::tauri_commands::{parse_commands_command, parse_events_command, parse_routes_command};

/// Check if the argument list appears to use new-style subcommands.
///
/// Returns true if the first non-flag argument is a known subcommand,
/// or if only global flags like --help/--version are present.
pub fn uses_new_syntax(args: &[String]) -> bool {
    let mut i = 0;
    let mut findings_alias = false;
    while i < args.len() {
        let arg = &args[i];

        // Skip global flags that can appear before subcommand
        if arg == "--json"
            || arg == "--quiet"
            || arg == "--verbose"
            || arg == "--library-mode"
            || arg == "--python-library"
            || arg == "--fresh"
            || arg == "--no-scan"
            || arg == "--fail-stale"
            || arg == "--for-ai"
            || arg == "--findings"
            || arg == "--summary"
            || arg == "--watch"
            || arg == "-v"
            || arg == "-q"
        {
            if arg == "--findings" || arg == "--summary" {
                findings_alias = true;
            }
            i += 1;
            continue;
        }

        // Handle flags with optional/required values
        if arg.starts_with("--color") || arg.starts_with("--py-root") {
            // --color=auto or --py-root=Lib (value in same arg)
            if arg.contains('=') {
                i += 1;
            } else {
                // --color auto or --py-root Lib (value in next arg)
                i += 2;
            }
            continue;
        }

        // These are always valid in new syntax (not legacy-specific)
        if arg == "--help"
            || arg == "-h"
            || arg == "--help-legacy"
            || arg == "--help-full"
            || arg == "--version"
            || arg == "-V"
        {
            return true;
        }
        // If we hit a flag, it's likely legacy syntax
        if arg.starts_with('-') {
            return false;
        }
        // First positional argument - check if it's a subcommand or jq filter
        if findings_alias {
            return true;
        }
        return SUBCOMMANDS.contains(&arg.as_str())
            || Command::retired_command_message(arg).is_some()
            || is_jq_filter(arg);
    }
    // No arguments = default to auto (new syntax)
    true
}

/// Parse command-line arguments into a ParsedCommand.
///
/// This is the main entry point for the new CLI parser. It:
/// 1. Extracts global options (--json, --quiet, etc.)
/// 2. Identifies the subcommand
/// 3. Parses command-specific options
///
/// Returns `None` if the arguments should be handled by the legacy parser.
pub fn parse_command(args: &[String]) -> Result<Option<ParsedCommand>, String> {
    // Quick check: if this looks like legacy syntax, return None
    if !uses_new_syntax(args) {
        return Ok(None);
    }

    let mut global = GlobalOptions::default();
    let mut remaining_args: Vec<String> = Vec::new();
    let mut subcommand: Option<String> = None;
    let mut for_ai_alias = false;
    let mut watch_alias = false;
    let mut help_requested = false;
    let mut legacy_findings_alias = false;
    let mut legacy_summary_only = false;

    // Check for jq-style query before extracting global options
    // This allows: loct '.metadata' to work without conflicts
    if !args.is_empty() && is_jq_filter(&args[0]) {
        return parse_jq_query_command(args, &global).map(Some);
    }

    // First pass: extract global options and find subcommand
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];

        match arg.as_str() {
            "--json" => {
                global.json = true;
                i += 1;
            }
            "--quiet" | "-q" => {
                global.quiet = true;
                i += 1;
            }
            "--verbose" | "-v" => {
                global.verbose = true;
                i += 1;
            }
            "--color" => {
                if let Some(value) = args.get(i + 1) {
                    global.color = parse_color_mode(value)?;
                    i += 2;
                } else {
                    global.color = crate::types::ColorMode::Always;
                    i += 1;
                }
            }
            _ if arg.starts_with("--color=") => {
                let value = arg.trim_start_matches("--color=");
                global.color = parse_color_mode(value)?;
                i += 1;
            }
            "--for-ai" => {
                for_ai_alias = true;
                i += 1;
            }
            "--watch" => {
                watch_alias = true;
                remaining_args.push(arg.clone());
                i += 1;
            }
            "--library-mode" => {
                global.library_mode = true;
                i += 1;
            }
            "--python-library" => {
                global.python_library = true;
                i += 1;
            }
            "--fresh" => {
                global.fresh = true;
                i += 1;
            }
            "--no-scan" => {
                global.no_scan = true;
                i += 1;
            }
            "--fail-stale" => {
                global.fail_stale = true;
                i += 1;
            }
            "--findings" => match subcommand.as_deref() {
                Some(_) => {
                    return Err(
                        "`--findings` is no longer a global flag. Use `loct findings`.".to_string(),
                    );
                }
                None => {
                    legacy_findings_alias = true;
                    i += 1;
                }
            },
            "--summary" => match subcommand.as_deref() {
                Some("tree") | Some("findings") | Some("suppressions") => {
                    remaining_args.push(arg.clone());
                    i += 1;
                }
                Some(_) => {
                    return Err(
                        "`--summary` is no longer a global flag. Use `loct findings --summary` for summary JSON, or keep `--summary` on `loct tree`."
                            .to_string(),
                    );
                }
                None => {
                    legacy_findings_alias = true;
                    legacy_summary_only = true;
                    i += 1;
                }
            },
            "--py-root" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--py-root requires a path".to_string())?;
                global.py_roots.push(PathBuf::from(value));
                i += 2;
            }
            _ if arg.starts_with("--py-root=") => {
                let value = arg.trim_start_matches("--py-root=");
                global.py_roots.push(PathBuf::from(value));
                i += 1;
            }
            "--help" | "-h" => {
                help_requested = true;
                i += 1;
            }
            "--help-legacy" => {
                return Ok(Some(ParsedCommand::new(
                    Command::Help(HelpOptions {
                        legacy: true,
                        ..Default::default()
                    }),
                    global,
                )));
            }
            "--help-full" => {
                return Ok(Some(ParsedCommand::new(
                    Command::Help(HelpOptions {
                        full: true,
                        ..Default::default()
                    }),
                    global,
                )));
            }
            "--version" | "-V" if subcommand.is_none() => {
                // `--version` is a global flag only BEFORE a subcommand (like
                // `git --version` vs `git log --version`). After a subcommand is
                // chosen, fall through so it reaches that subcommand's parser —
                // e.g. `loct find --literal --version` must search for the literal
                // string "--version", not print loct's version. (`--help`/`-h`
                // stay global so `loct find --help` keeps working.)
                return Ok(Some(ParsedCommand::new(Command::Version, global)));
            }
            "--" => {
                // POSIX end-of-options. Everything after `--` (including the `--`
                // itself) is positional and goes verbatim to the subcommand
                // parser, so a literal that looks like a flag can be searched:
                // `loct find --literal -- --version`. Without this the global scan
                // here would otherwise swallow `--version`/`-h` as loct's own.
                remaining_args.extend_from_slice(&args[i..]);
                break;
            }
            _ if arg.starts_with('-') => {
                // Unknown flag - pass to command-specific parser
                remaining_args.push(arg.clone());
                i += 1;
            }
            _ => {
                // Positional argument
                if subcommand.is_none()
                    && (SUBCOMMANDS.contains(&arg.as_str())
                        || Command::retired_command_message(arg).is_some())
                {
                    subcommand = Some(arg.clone());
                } else {
                    remaining_args.push(arg.clone());
                }
                i += 1;
            }
        }
    }

    if subcommand.is_none() && watch_alias {
        subcommand = Some("scan".to_string());
    }

    if legacy_findings_alias && subcommand.is_some() {
        return Err(
            "`--findings` and bare `--summary` are no longer global output flags. Use `loct findings` or `loct findings --summary`."
                .to_string(),
        );
    }

    if help_requested {
        let help_command = if legacy_findings_alias {
            Some("findings".to_string())
        } else {
            subcommand.clone()
        };
        return Ok(Some(ParsedCommand::new(
            Command::Help(HelpOptions {
                command: help_command,
                ..Default::default()
            }),
            global,
        )));
    }

    if legacy_findings_alias {
        let mut alias_args = remaining_args.clone();
        if legacy_summary_only {
            alias_args.insert(0, "--summary".to_string());
        }

        let command = parse_findings_command(&alias_args)?;
        let legacy_invocation = if args.is_empty() {
            "loct".to_string()
        } else {
            format!("loct {}", args.join(" "))
        };
        let suggested_invocation = {
            let mut parts = vec!["loct".to_string(), "findings".to_string()];
            if legacy_summary_only {
                parts.push("--summary".to_string());
            }
            parts.extend(
                remaining_args
                    .iter()
                    .filter(|arg| !arg.starts_with('-'))
                    .cloned(),
            );
            parts.join(" ")
        };

        return Ok(Some(ParsedCommand::from_legacy(
            command,
            global,
            legacy_invocation,
            suggested_invocation,
        )));
    }

    let mut command = match subcommand.as_deref() {
        None | Some("auto") => parse_auto_command(&remaining_args)?,
        Some("agent") => {
            let cmd = parse_auto_command(&remaining_args)?;
            match cmd {
                Command::Auto(mut opts) => {
                    opts.for_agent_feed = true;
                    opts.agent_json = true;
                    Command::Auto(opts)
                }
                other => other,
            }
        }
        Some("scan") => parse_scan_command(&remaining_args)?,
        Some("watch") => parse_watch_command(&remaining_args)?,
        Some("tree") => parse_tree_command(&remaining_args)?,
        Some("slice") | Some("s") => parse_slice_command(&remaining_args)?,
        Some("context") => parse_context_command(&remaining_args)?,
        Some("repo-view") => parse_repo_view_command(&remaining_args)?,
        Some("find") | Some("f") => parse_find_command(&remaining_args)?,
        Some("occurrences") => parse_occurrences_command(&remaining_args)?,
        Some("findings") => parse_findings_command(&remaining_args)?,
        Some("dead") | Some("unused") | Some("d") => parse_dead_command(&remaining_args)?,
        Some("cycles") | Some("c") => parse_cycles_command(&remaining_args)?,
        Some("trace") => parse_trace_command(&remaining_args)?,
        Some("commands") => parse_commands_command(&remaining_args)?,
        Some("events") => parse_events_command(&remaining_args)?,
        Some("pipelines") => parse_pipelines_command(&remaining_args)?,
        Some("insights") => parse_insights_command(&remaining_args)?,
        Some("manifests") => parse_manifests_command(&remaining_args)?,
        Some("routes") => parse_routes_command(&remaining_args)?,
        Some("info") => parse_info_command(&remaining_args)?,
        Some("lint") => parse_lint_command(&remaining_args)?,
        Some("report") => parse_report_command(&remaining_args)?,
        Some("prism") => parse_prism_command(&remaining_args)?,
        Some("help") => parse_help_command(&remaining_args)?,
        Some("query") | Some("q") => parse_query_command(&remaining_args)?,
        Some("body") => parse_body_command(&remaining_args)?,
        Some("impact") | Some("i") => parse_impact_command(&remaining_args)?,
        Some("diff") => parse_diff_command(&remaining_args)?,
        Some("crowd") => parse_crowd_command(&remaining_args)?,
        Some("tagmap") => parse_tagmap_command(&remaining_args)?,
        Some("twins") | Some("t") => parse_twins_command(&remaining_args)?,
        Some("suppress") => parse_suppress_command(&remaining_args)?,
        Some("suppressions") => parse_suppressions_command(&remaining_args)?,
        Some("sniff") => {
            return Err(Command::retired_command_message("sniff")
                .expect("retired sniff message")
                .to_string());
        }
        Some("dist") => parse_dist_command(&remaining_args)?,
        Some("coverage") => parse_coverage_command(&remaining_args)?,
        Some("focus") => parse_focus_command(&remaining_args)?,
        Some("hotspots") => parse_hotspots_command(&remaining_args)?,
        Some("follow") => parse_follow_command(&remaining_args)?,
        Some("layoutmap") => parse_layoutmap_command(&remaining_args)?,
        Some("zombie") => {
            return Err(Command::retired_command_message("zombie")
                .expect("retired zombie message")
                .to_string());
        }
        Some("health") | Some("h") => parse_health_command(&remaining_args)?,
        Some("audit") => parse_audit_command(&remaining_args)?,
        Some("doctor") => parse_doctor_command(&remaining_args)?,
        Some("env-truth") | Some("envtruth") => parse_env_truth_command(&remaining_args)?,
        Some("plan") | Some("p") => parse_plan_command(&remaining_args)?,
        Some("cache") => parse_cache_command(&remaining_args)?,
        Some("prune-old-artifacts") => parse_prune_old_artifacts_command(&remaining_args)?,
        Some(unknown) => {
            // Try to find a similar command using fuzzy matching
            let suggestion = suggest_similar_command(unknown);
            return Err(format!(
                "Unknown command '{}'. {}Run 'loct --help' for available commands.",
                unknown,
                suggestion.map_or(String::new(), |s| format!("Did you mean: {}?\n", s))
            ));
        }
    };

    if for_ai_alias {
        match command {
            Command::Auto(ref mut opts) => {
                opts.for_agent_feed = true;
                opts.agent_json = true;
                opts.full_scan = true;
            }
            _ => {
                return Err(
                    "--for-ai is only supported with the default scan (no subcommand)".to_string(),
                );
            }
        }
    }

    Ok(Some(ParsedCommand::new(command, global)))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uses_new_syntax() {
        // New syntax
        assert!(uses_new_syntax(&[]));
        assert!(uses_new_syntax(&["scan".into()]));
        assert!(uses_new_syntax(&["tree".into()]));
        assert!(uses_new_syntax(&["--json".into(), "scan".into()]));
        assert!(uses_new_syntax(&["--watch".into()]));
        assert!(uses_new_syntax(&["--for-ai".into()]));
        assert!(uses_new_syntax(&["--summary".into(), "src".into()]));

        // Legacy syntax
        assert!(!uses_new_syntax(&["--tree".into()]));
        assert!(!uses_new_syntax(&["-A".into()]));
        assert!(!uses_new_syntax(&["-A".into(), "--dead".into()]));
    }

    #[test]
    fn test_parse_auto_default() {
        let result = parse_command(&[]).unwrap().unwrap();
        assert_eq!(result.command.name(), "auto");
    }

    #[test]
    fn test_parse_for_ai_alias() {
        let args = vec!["--for-ai".into()];
        let result = parse_command(&args).unwrap().unwrap();
        if let Command::Auto(opts) = result.command {
            assert!(opts.for_agent_feed);
            assert!(opts.agent_json);
            assert!(opts.full_scan);
        } else {
            panic!("Expected Auto command");
        }
    }

    #[test]
    fn test_parse_repo_view_subcommand() {
        let args = vec!["repo-view".into(), ".".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "repo-view");
        if let Command::RepoView(opts) = result.command {
            assert_eq!(opts.project, Some(PathBuf::from(".")));
        } else {
            panic!("Expected RepoView command");
        }
    }

    #[test]
    fn test_parse_prism_subcommand() {
        let args = vec![
            "prism".into(),
            "--task".into(),
            "auth".into(),
            "--task".into(),
            "auth core".into(),
            "--aicx-project".into(),
            "loctree-suite".into(),
            "--json".into(),
        ];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "prism");
        assert!(result.global.json);
        if let Command::Prism(opts) = result.command {
            assert_eq!(opts.tasks, vec!["auth", "auth core"]);
            assert_eq!(opts.aicx_project_override.as_deref(), Some("loctree-suite"));
        } else {
            panic!("Expected Prism command");
        }
    }

    #[test]
    fn test_parse_follow_subcommand_defaults_to_all() {
        let args = vec!["follow".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "follow");
        if let Command::Follow(opts) = result.command {
            assert_eq!(opts.scope, "all");
            assert!(opts.handler.is_none());
        } else {
            panic!("Expected Follow command");
        }
    }

    #[test]
    fn test_parse_watch_alias_defaults_to_scan() {
        let args = vec!["--watch".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "scan");
        if let Command::Scan(opts) = result.command {
            assert!(opts.watch);
        } else {
            panic!("Expected Scan command");
        }
    }

    #[test]
    fn test_parse_scan_command() {
        let args = vec!["scan".into(), "--full-scan".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "scan");
        if let Command::Scan(opts) = result.command {
            assert!(opts.full_scan);
        } else {
            panic!("Expected Scan command");
        }
    }

    #[test]
    fn test_parse_tree_command_with_depth() {
        let args = vec!["tree".into(), "--depth".into(), "3".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "tree");
        if let Command::Tree(opts) = result.command {
            assert_eq!(opts.depth, Some(3));
        } else {
            panic!("Expected Tree command");
        }
    }

    #[test]
    fn test_parse_findings_subcommand() {
        let args = vec!["findings".into(), "--summary".into(), "src/".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "findings");
        if let Command::Findings(opts) = result.command {
            assert!(opts.summary);
            assert_eq!(opts.roots, vec![PathBuf::from("src/")]);
        } else {
            panic!("Expected Findings command");
        }
    }

    #[test]
    fn test_parse_slice_command() {
        let args = vec!["slice".into(), "src/main.rs".into(), "--consumers".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "slice");
        if let Command::Slice(opts) = result.command {
            assert_eq!(opts.target, "src/main.rs");
            assert!(opts.consumers);
        } else {
            panic!("Expected Slice command");
        }
    }

    #[test]
    fn test_parse_trace_command() {
        let args = vec!["trace".into(), "toggle_assistant".into(), "app".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "trace");
        if let Command::Trace(opts) = result.command {
            assert_eq!(opts.handler, "toggle_assistant");
            assert_eq!(opts.roots, vec![PathBuf::from("app")]);
        } else {
            panic!("Expected Trace command");
        }
    }

    #[test]
    fn test_parse_dead_command() {
        let args = vec!["dead".into(), "--confidence".into(), "high".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "dead");
        if let Command::Dead(opts) = result.command {
            assert_eq!(opts.confidence, Some("high".into()));
        } else {
            panic!("Expected Dead command");
        }
    }

    #[test]
    fn test_parse_global_json_flag() {
        let args = vec!["--json".into(), "scan".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert!(result.global.json);
        assert_eq!(result.command.name(), "scan");
    }

    #[test]
    fn test_parse_findings_legacy_alias() {
        let args = vec!["--findings".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert!(result.from_legacy);
        assert_eq!(result.suggested_invocation, Some("loct findings".into()));
        assert_eq!(result.command.name(), "findings");
    }

    #[test]
    fn test_parse_findings_legacy_alias_with_path() {
        let args = vec!["--findings".into(), "src".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert!(result.from_legacy);
        assert_eq!(
            result.suggested_invocation,
            Some("loct findings src".into())
        );
        if let Command::Findings(opts) = result.command {
            assert!(!opts.summary);
            assert_eq!(opts.roots, vec![PathBuf::from("src")]);
        } else {
            panic!("Expected Findings command");
        }
    }

    #[test]
    fn test_parse_summary_legacy_alias() {
        let args = vec!["--summary".into(), "src".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert!(result.from_legacy);
        assert_eq!(
            result.suggested_invocation,
            Some("loct findings --summary src".into())
        );
        if let Command::Findings(opts) = result.command {
            assert!(opts.summary);
            assert_eq!(opts.roots, vec![PathBuf::from("src")]);
        } else {
            panic!("Expected Findings command");
        }
    }

    #[test]
    fn test_parse_help_flag() {
        let args = vec!["--help".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "help");
    }

    #[test]
    fn test_parse_version_flag() {
        let args = vec!["--version".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "version");
    }

    #[test]
    fn test_version_after_subcommand_is_literal_query_not_version() {
        // Regression for 2026-06-20 loctree-feedback: `loct find --literal --version`
        // printed loct's OWN version instead of searching for the literal string
        // "--version". `--version` is global only BEFORE a subcommand.
        let args = vec!["find".into(), "--literal".into(), "--version".into()];
        let result = parse_command(&args).unwrap().unwrap();
        match result.command {
            Command::Find(opts) => {
                assert!(opts.literal, "--literal flag must still be set");
                assert!(
                    opts.queries.contains(&"--version".to_string()),
                    "flag-like literal must be captured as the query: {:?}",
                    opts.queries
                );
            }
            other => panic!("Expected Find command, got {}", other.name()),
        }
    }

    #[test]
    fn test_double_dash_terminator_carries_flaglike_literal() {
        // The POSIX `--` workaround documented in loctree-feedback must actually work
        // end-to-end through the global parser: `loct find --literal -- --version`.
        let args = vec![
            "find".into(),
            "--literal".into(),
            "--".into(),
            "--version".into(),
        ];
        let result = parse_command(&args).unwrap().unwrap();
        match result.command {
            Command::Find(opts) => {
                assert!(opts.literal);
                assert!(
                    opts.queries.contains(&"--version".to_string()),
                    "post-`--` token must be the literal query: {:?}",
                    opts.queries
                );
            }
            other => panic!("Expected Find command, got {}", other.name()),
        }
    }

    #[test]
    fn test_help_after_subcommand_still_resolves_help() {
        // The `--version` gating must NOT regress subcommand help: `loct find
        // --help` stays global and resolves to the help command for `find`.
        let args = vec!["find".into(), "--help".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "help");
    }

    #[test]
    fn test_legacy_syntax_returns_none() {
        let args = vec!["--tree".into()];
        let result = parse_command(&args).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_find_with_regex() {
        let args = vec![
            "find".into(),
            "--symbol".into(),
            ".*patient.*".into(),
            "--lang".into(),
            "ts".into(),
        ];
        let result = parse_command(&args).unwrap().unwrap();
        if let Command::Find(opts) = result.command {
            assert_eq!(opts.symbol, Some(".*patient.*".into()));
            assert_eq!(opts.lang, Some("ts".into()));
        } else {
            panic!("Expected Find command");
        }
    }

    #[test]
    fn test_parse_crowd_command() {
        let args = vec!["crowd".into(), "message".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "crowd");
        if let Command::Crowd(opts) = result.command {
            assert_eq!(opts.pattern, Some("message".into()));
        } else {
            panic!("Expected Crowd command");
        }
    }

    #[test]
    fn test_parse_crowd_auto_detect() {
        let args = vec!["crowd".into(), "--auto".into()];
        let result = parse_command(&args).unwrap().unwrap();
        if let Command::Crowd(opts) = result.command {
            assert!(opts.auto_detect);
            assert!(opts.pattern.is_none());
        } else {
            panic!("Expected Crowd command");
        }
    }

    #[test]
    fn test_parse_jq_query_basic() {
        let args = vec![".metadata".into()];
        let result = parse_command(&args).unwrap().unwrap();
        assert_eq!(result.command.name(), "jq");
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
        let args = vec![".files[]".into(), "-r".into(), "-c".into()];
        let result = parse_command(&args).unwrap().unwrap();
        if let Command::JqQuery(opts) = result.command {
            assert_eq!(opts.filter, ".files[]");
            assert!(opts.raw_output);
            assert!(opts.compact_output);
        } else {
            panic!("Expected JqQuery command");
        }
    }

    #[test]
    fn test_parse_jq_query_with_arg() {
        let args = vec![
            ".metadata".into(),
            "--arg".into(),
            "name".into(),
            "value".into(),
        ];
        let result = parse_command(&args).unwrap().unwrap();
        if let Command::JqQuery(opts) = result.command {
            assert_eq!(opts.string_args.len(), 1);
            assert_eq!(opts.string_args[0].0, "name");
            assert_eq!(opts.string_args[0].1, "value");
        } else {
            panic!("Expected JqQuery command");
        }
    }

    #[test]
    fn test_parse_jq_query_with_snapshot() {
        let args = vec![
            ".metadata".into(),
            "--snapshot".into(),
            ".loctree/snap.json".into(),
        ];
        let result = parse_command(&args).unwrap().unwrap();
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
