//! Parsers for Tauri-specific commands: commands, events, routes.
//!
//! These commands analyze Tauri application contracts and IPC communication.

use std::path::PathBuf;

use super::super::command::{Command, CommandsOptions, EventsOptions, RoutesOptions};

/// Parse `loct commands [options]` command - show Tauri command bridges.
pub(super) fn parse_commands_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct commands - Show Tauri command bridges (FE <-> BE)

USAGE:
    loct commands [OPTIONS] [PATHS...]

OPTIONS:
    --name <PATTERN>   Filter to commands matching pattern
    --missing, --missing-only
                       Show only missing handlers (FE calls -> no BE)
    --unused, --unused-only
                       Show only unused handlers (BE exists -> no FE calls)
    --limit <N>        Maximum results to show (default: unlimited)
    --no-duplicates    Hide duplicate export sections in CLI output
    --no-dynamic-imports Hide dynamic import sections in CLI output
    --help, -h         Show this help message

EXAMPLES:
    loct commands
    loct commands --missing"
            .to_string());
    }

    let mut opts = CommandsOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--name" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--name requires a pattern".to_string())?;
                opts.name_filter = Some(value.clone());
                i += 2;
            }
            "--missing" | "--missing-only" => {
                opts.missing_only = true;
                i += 1;
            }
            "--unused" | "--unused-only" => {
                opts.unused_only = true;
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
            "--limit" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--limit requires a number".to_string())?;
                opts.limit = Some(
                    value
                        .parse()
                        .map_err(|_| format!("Invalid limit value: {}", value))?,
                );
                i += 2;
            }
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'commands' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Commands(opts))
}

/// Parse `loct events [options]` command - show event flow and issues.
pub(super) fn parse_events_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct events - Show event flow and issues

USAGE:
    loct events [OPTIONS] [PATHS...]

OPTIONS:
    --ghost(s)   Show only ghost events (emitted but never listened)
    --orphan(s)  Show only orphan listeners (listening but never emitted)
    --races      Show only potential race conditions (multiple emitters)
    --no-duplicates      Hide duplicate export sections in CLI output
    --no-dynamic-imports Hide dynamic import sections in CLI output
    --include-artifacts  Disable the artifact fence (show event bridges from
                         vendored/minified/generated files)
    --help, -h   Show this help message

EXAMPLES:
    loct events
    loct events --ghost"
            .to_string());
    }

    let mut opts = EventsOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--ghost" | "--ghosts" => {
                opts.ghost = true;
                i += 1;
            }
            "--orphan" | "--orphans" => {
                opts.orphan = true;
                i += 1;
            }
            "--races" => {
                opts.races = true;
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
            "--fe-sync" => {
                opts.fe_sync = true;
                i += 1;
            }
            "--include-artifacts" => {
                opts.include_artifacts = true;
                i += 1;
            }
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'events' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Events(opts))
}

/// Parse `loct routes [options]` command - list backend/web routes.
pub(super) fn parse_routes_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct routes - List backend/web routes (FastAPI/Flask)

USAGE:
    loct routes [OPTIONS] [PATHS...]

OPTIONS:
    --framework <NAME>   Filter by framework label (fastapi, flask)
    --path <PATTERN>     Filter by route path substring
    --help, -h           Show this help message

EXAMPLES:
    loct routes
    loct routes --framework fastapi"
            .to_string());
    }

    let mut opts = RoutesOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--framework" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--framework requires a value".to_string())?;
                opts.framework = Some(value.clone());
                i += 2;
            }
            "--path" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--path requires a value".to_string())?;
                opts.path_filter = Some(value.clone());
                i += 2;
            }
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'routes' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Routes(opts))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_commands_command() {
        let args = vec!["--missing".into()];
        let result = parse_commands_command(&args).unwrap();
        if let Command::Commands(opts) = result {
            assert!(opts.missing_only);
        } else {
            panic!("Expected Commands command");
        }
    }

    #[test]
    fn test_parse_events_command() {
        let args = vec!["--ghost".into()];
        let result = parse_events_command(&args).unwrap();
        if let Command::Events(opts) = result {
            assert!(opts.ghost);
        } else {
            panic!("Expected Events command");
        }
    }

    #[test]
    fn test_parse_routes_command() {
        let args = vec!["--framework".into(), "fastapi".into()];
        let result = parse_routes_command(&args).unwrap();
        if let Command::Routes(opts) = result {
            assert_eq!(opts.framework, Some("fastapi".into()));
        } else {
            panic!("Expected Routes command");
        }
    }
}
