//! Parsers for miscellaneous commands: crowd, tagmap, suppress, dist, layoutmap,
//! health, audit, doctor, help.
//!
//! These commands handle various specialized functionality.

use std::path::PathBuf;

use super::super::command::{
    AtlasOptions, AuditOptions, CacheAction, CacheOptions, Command, CrowdOptions, DistOptions,
    DoctorOptions, EnvTruthOptions, HealthOptions, HelpOptions, InventoryOptions, LayoutmapOptions,
    PlanOptions, PrismOptions, PruneOldArtifactsOptions, SnapshotPathOptions, SuppressOptions,
    TagmapOptions,
};

/// Parse `loct crowd [pattern] [options]` command - detect functional crowds.
pub(super) fn parse_crowd_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err(
            "loct crowd - Detect functional crowds (similar files clustering)

USAGE:
    loct crowd [PATTERN] [OPTIONS]

ARGUMENTS:
    [PATTERN]    Pattern to detect crowd around (e.g., \"message\", \"patient\")
                 If not specified, auto-detects all crowds

OPTIONS:
    --auto, -a         Detect all crowds automatically
    --min-size <N>     Minimum crowd size to report (default: 2)
    --limit <N>        Maximum crowds to show (default: 10)
    --include-tests    Include test files (excluded by default)
    --help, -h         Show this help message

EXAMPLES:
    loct crowd                  # Auto-detect all crowds
    loct crowd message          # Find files clustering around \"message\"
    loct crowd --limit 5        # Show top 5 crowds"
                .to_string(),
        );
    }

    let mut opts = CrowdOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--auto" | "-a" => {
                opts.auto_detect = true;
                i += 1;
            }
            "--min-size" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--min-size requires a number".to_string())?;
                opts.min_size = Some(value.parse().map_err(|_| "--min-size requires a number")?);
                i += 2;
            }
            "--limit" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--limit requires a number".to_string())?;
                opts.limit = Some(value.parse().map_err(|_| "--limit requires a number")?);
                i += 2;
            }
            "--include-tests" => {
                opts.include_tests = true;
                i += 1;
            }
            _ if !arg.starts_with('-') => {
                // Positional argument is the pattern (if not a root path)
                if opts.pattern.is_none() && !std::path::Path::new(arg).exists() {
                    opts.pattern = Some(arg.clone());
                } else {
                    opts.roots.push(PathBuf::from(arg));
                }
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'crowd' command.", arg));
            }
        }
    }

    // If no pattern and no auto flag, enable auto-detect
    if opts.pattern.is_none() && !opts.auto_detect {
        opts.auto_detect = true;
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Crowd(opts))
}

/// Parse `loct tagmap <keyword> [options]` command - map files by keyword.
pub(super) fn parse_tagmap_command(args: &[String]) -> Result<Command, String> {
    if args.is_empty() {
        return Err("tagmap requires a keyword. Usage: loct tagmap <keyword>".to_string());
    }

    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h")
        && let Some(help) = Command::format_command_help("tagmap")
    {
        println!("{}", help);
        std::process::exit(0);
    }

    let mut opts = TagmapOptions::default();

    // First positional argument is the keyword
    if !args[0].starts_with('-') {
        opts.keyword = args[0].clone();
    } else {
        return Err("tagmap requires a keyword as first argument".to_string());
    }

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--include-tests" => {
                opts.include_tests = true;
                i += 1;
            }
            "--limit" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--limit requires a number".to_string())?;
                opts.limit = Some(value.parse().map_err(|_| "--limit requires a number")?);
                i += 2;
            }
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'tagmap' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Tagmap(opts))
}

/// Parse `loct prism --task A --task B [options]` command.
pub(super) fn parse_prism_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h")
        && let Some(help) = Command::format_command_help("prism")
    {
        return Err(help.to_string());
    }

    let mut opts = PrismOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--task" | "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--task requires text".to_string())?;
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    opts.tasks.push(trimmed.to_string());
                }
                i += 2;
            }
            "--project" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--project requires a path".to_string())?;
                opts.project = Some(PathBuf::from(value));
                i += 2;
            }
            "--aicx-project" | "--aicx-bucket" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{arg} requires a bucket name"))?;
                opts.aicx_project_override = Some(value.clone());
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
            "--json" => {
                opts.json = true;
                i += 1;
            }
            "--limit" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--limit requires a number".to_string())?;
                opts.limit = value.parse().map_err(|_| "--limit requires a number")?;
                i += 2;
            }
            _ if !arg.starts_with('-') => {
                opts.tasks.push(arg.trim().to_string());
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'prism' command.", arg));
            }
        }
    }

    opts.tasks.retain(|task| !task.trim().is_empty());
    opts.tasks.dedup();

    if opts.tasks.len() < 2 {
        return Err(
            "prism requires at least two task framings. Usage: loct prism --task \"auth\" --task \"auth core\""
                .to_string(),
        );
    }

    Ok(Command::Prism(opts))
}

/// Parse `loct suppress [options]` command - manage false positive suppressions.
pub(super) fn parse_suppress_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct suppress - Manage false positive suppressions

USAGE:
    loct suppress <type> <symbol> [OPTIONS]
    loct suppress --list
    loct suppress --clear

TYPES:
    twins         Exact twin (same symbol in multiple files)
    dead_parrot   Dead parrot (export with 0 imports)
    dead_export   Dead export (unused export)
    circular      Circular import

OPTIONS:
    --file <path>       Suppress only for this specific file
    --reason <text>     Reason for suppression (for documentation)
    --remove            Remove a suppression instead of adding
    --list              List all current suppressions
    --clear             Clear all suppressions

EXAMPLES:
    loct suppress twins Message --reason \"FE/BE mirror OK\"
    loct suppress dead_parrot unusedFunc --file src/utils.ts
    loct suppress --list
    loct suppress twins Message --remove"
            .to_string());
    }

    let mut opts = SuppressOptions::default();
    let mut i = 0;
    let mut positional_count = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--list" => {
                opts.list = true;
                i += 1;
            }
            "--clear" => {
                opts.clear = true;
                i += 1;
            }
            "--remove" => {
                opts.remove = true;
                i += 1;
            }
            "--file" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--file requires a path".to_string())?;
                opts.file = Some(value.clone());
                i += 2;
            }
            "--reason" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--reason requires a value".to_string())?;
                opts.reason = Some(value.clone());
                i += 2;
            }
            "--path" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--path requires a directory".to_string())?;
                opts.path = Some(PathBuf::from(value));
                i += 2;
            }
            _ => {
                if arg.starts_with('-') {
                    return Err(format!("Unknown option '{}' for 'suppress' command.", arg));
                }
                // Positional: first is type, second is symbol
                match positional_count {
                    0 => opts.suppression_type = Some(arg.clone()),
                    1 => opts.symbol = Some(arg.clone()),
                    _ => return Err(format!("Unexpected argument '{}'.", arg)),
                }
                positional_count += 1;
                i += 1;
            }
        }
    }

    Ok(Command::Suppress(opts))
}

/// Parse `loct dist [options]` command - analyze bundle distribution.
pub(super) fn parse_dist_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h")
        && let Some(help) = Command::format_command_help("dist")
    {
        return Err(help.to_string());
    }

    let mut opts = DistOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--source-map" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--source-map requires a path".to_string())?;
                opts.source_maps.push(PathBuf::from(value));
                i += 2;
            }
            "--src" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--src requires a directory path".to_string())?;
                opts.src = Some(PathBuf::from(value));
                i += 2;
            }
            "--report" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--report requires a file path".to_string())?;
                opts.report_path = Some(PathBuf::from(value));
                i += 2;
            }
            _ if !arg.starts_with('-') => {
                // Legacy positional shorthand: first is source-map input, second is src
                if opts.source_maps.is_empty() {
                    opts.source_maps.push(PathBuf::from(arg));
                } else if opts.src.is_none() {
                    opts.src = Some(PathBuf::from(arg));
                } else {
                    return Err(format!(
                        "Unexpected argument '{}'. dist takes --src, repeated --source-map, and optional --report.",
                        arg
                    ));
                }
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'dist' command.", arg));
            }
        }
    }

    if opts.source_maps.is_empty() {
        return Err(
            "'dist' command requires at least one --source-map <path>. Usage: loct dist --src src/ --source-map dist/ or loct dist --src src/ --source-map dist/main.js.map --source-map dist/chunks/"
                .to_string(),
        );
    }

    if opts.src.is_none() {
        return Err(
            "'dist' command requires --src <dir>. Usage: loct dist --src src/ --source-map dist/main.js.map"
                .to_string(),
        );
    }

    Ok(Command::Dist(opts))
}

/// Parse `loct layoutmap [options]` command - analyze CSS layout.
pub(super) fn parse_layoutmap_command(args: &[String]) -> Result<Command, String> {
    // Check for --help first
    if args.iter().any(|a| a == "--help" || a == "-h")
        && let Some(help) = Command::format_command_help("layoutmap")
    {
        println!("{}", help);
        std::process::exit(0);
    }

    let mut opts = LayoutmapOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--zindex" | "--z-index" | "--zindex-only" => {
                opts.zindex_only = true;
                i += 1;
            }
            "--sticky" | "--sticky-only" => {
                opts.sticky_only = true;
                i += 1;
            }
            "--grid" | "--grid-only" => {
                opts.grid_only = true;
                i += 1;
            }
            "--min-zindex" | "--min-z" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--min-zindex requires a value".to_string())?;
                opts.min_zindex = Some(value.parse::<i32>().map_err(|_| {
                    format!("Invalid z-index value '{}', expected a number", value)
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
            "--exclude" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--exclude requires a glob pattern".to_string())?;
                opts.exclude.push(value.clone());
                i += 2;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'layoutmap' command.", arg));
            }
        }
    }

    Ok(Command::Layoutmap(opts))
}

/// Parse `loct health [options]` command - codebase health check.
pub(super) fn parse_health_command(args: &[String]) -> Result<Command, String> {
    // Check for --help first
    if args.iter().any(|a| a == "--help" || a == "-h")
        && let Some(help) = Command::format_command_help("health")
    {
        println!("{}", help);
        std::process::exit(0);
    }

    let mut opts = HealthOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--include-tests" => {
                opts.include_tests = true;
                i += 1;
            }
            _ => {
                // Treat as root path
                if arg.starts_with("--") {
                    return Err(format!("Unknown option '{}' for 'health' command.", arg));
                }
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
        }
    }

    Ok(Command::Health(opts))
}

/// Parse `loct audit [options]` command - security audit.
pub(super) fn parse_audit_command(args: &[String]) -> Result<Command, String> {
    // Check for --help first
    if args.iter().any(|a| a == "--help" || a == "-h")
        && let Some(help) = Command::format_command_help("audit")
    {
        println!("{}", help);
        std::process::exit(0);
    }

    let mut opts = AuditOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--include-tests" => {
                opts.include_tests = true;
                i += 1;
            }
            "--todos" | "-t" => {
                opts.todos = true;
                i += 1;
            }
            "--limit" => {
                i += 1;
                if i < args.len() {
                    opts.limit = Some(
                        args[i]
                            .parse()
                            .map_err(|_| format!("Invalid limit value: {}", args[i]))?,
                    );
                    i += 1;
                } else {
                    return Err("--limit requires a numeric value".to_string());
                }
            }
            "--stdout" => {
                return Err(
                    "`loct audit` writes markdown reports to an artifact file only. Use `--json` for stdout-oriented automation.".to_string(),
                );
            }
            "--no-open" => {
                opts.no_open = true;
                i += 1;
            }
            _ => {
                // Treat as root path
                if arg.starts_with("--") {
                    return Err(format!("Unknown option '{}' for 'audit' command.", arg));
                }
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
        }
    }

    Ok(Command::Audit(opts))
}

/// Parse `loct doctor [options]` command - inspect cache identity and scope.
pub(super) fn parse_doctor_command(args: &[String]) -> Result<Command, String> {
    // Check for --help first
    if args.iter().any(|a| a == "--help" || a == "-h")
        && let Some(help) = Command::format_command_help("doctor")
    {
        println!("{}", help);
        std::process::exit(0);
    }

    let mut opts = DoctorOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--cache" => {
                opts.cache = true;
                i += 1;
            }
            "--scope" => {
                opts.scope = true;
                i += 1;
            }
            "--list" => {
                opts.list = true;
                i += 1;
            }
            "--json" => {
                opts.json = true;
                i += 1;
            }
            "--fix" => {
                opts.fix = true;
                i += 1;
            }
            "--yes" => {
                opts.yes = true;
                i += 1;
            }
            "--project" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--project requires a path".to_string())?;
                opts.project = Some(PathBuf::from(value));
                i += 2;
            }
            "--include-tests" => {
                opts.include_tests = true;
                i += 1;
            }
            "--apply-suppressions" => {
                opts.apply_suppressions = true;
                i += 1;
            }
            _ => {
                // Treat as root path
                if arg.starts_with("--") {
                    return Err(format!("Unknown option '{}' for 'doctor' command.", arg));
                }
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
        }
    }

    // No mode selected → leave all flags false so the handler can pick
    // per-project diagnostic when cwd has a snapshot, falling back to the
    // global cache list when it does not. The historical default
    // (`opts.list = true`) is preserved as the no-snapshot fallback inside
    // `handlers::doctor::run`; running `loct doctor --list` still produces
    // the same global table as before.

    Ok(Command::Doctor(opts))
}

/// Parse `loct env-truth [options]` command — Cut 8 declaration-side env audit.
pub(super) fn parse_env_truth_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h")
        && let Some(help) = Command::format_command_help("env-truth")
    {
        println!("{}", help);
        std::process::exit(0);
    }

    let mut opts = EnvTruthOptions {
        include_orphans: true,
        ..EnvTruthOptions::default()
    };
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--json" => {
                opts.json = true;
                i += 1;
            }
            "--md" | "--markdown" => {
                opts.markdown = true;
                i += 1;
            }
            "--all" => {
                opts.all = true;
                i += 1;
            }
            "--hashes" | "--show-hashes" => {
                opts.show_hashes = true;
                i += 1;
            }
            "--name" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--name requires a value".to_string())?;
                opts.name = Some(value.clone());
                i += 2;
            }
            "--no-orphans" => {
                opts.no_orphans = true;
                opts.include_orphans = false;
                i += 1;
            }
            "--include-orphans" => {
                opts.include_orphans = true;
                opts.no_orphans = false;
                i += 1;
            }
            "--stale-threshold-days" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--stale-threshold-days requires a numeric value".to_string())?;
                let parsed: u32 = value.parse().map_err(|_| {
                    "--stale-threshold-days must be a non-negative integer".to_string()
                })?;
                opts.stale_threshold_days = Some(parsed);
                i += 2;
            }
            "--fail-on" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--fail-on requires a kind".to_string())?;
                opts.fail_on.push(value.clone());
                i += 2;
            }
            "--paths" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--paths requires a comma-separated list".to_string())?;
                for piece in value.split(',') {
                    let trimmed = piece.trim();
                    if !trimmed.is_empty() {
                        opts.restricted_paths.push(PathBuf::from(trimmed));
                    }
                }
                i += 2;
            }
            _ => {
                if arg.starts_with("--") {
                    return Err(format!("Unknown option '{}' for 'env-truth' command.", arg));
                }
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
        }
    }

    Ok(Command::EnvTruth(opts))
}

/// Parse `loct help [command]` command - show help.
pub(super) fn parse_help_command(args: &[String]) -> Result<Command, String> {
    let mut opts = HelpOptions::default();

    for arg in args {
        match arg.as_str() {
            "--legacy" => opts.legacy = true,
            "--full" => opts.full = true,
            _ if !arg.starts_with('-') => opts.command = Some(arg.clone()),
            _ => {
                return Err(format!("Unknown option '{}' for 'help' command.", arg));
            }
        }
    }

    Ok(Command::Help(opts))
}

/// Parse `loct plan [options] [path]` command - generate refactoring plan.
pub(super) fn parse_plan_command(args: &[String]) -> Result<Command, String> {
    // Check for --help first
    if args.iter().any(|a| a == "--help" || a == "-h")
        && let Some(help) = Command::format_command_help("plan")
    {
        println!("{}", help);
        std::process::exit(0);
    }

    let mut opts = PlanOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--target-layout" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--target-layout requires a value".to_string())?;
                opts.target_layout = Some(value.clone());
                i += 2;
            }
            "--markdown" | "--md" => {
                opts.markdown = true;
                i += 1;
            }
            "--json" => {
                opts.json = true;
                i += 1;
            }
            "--script" | "--sh" => {
                opts.script = true;
                i += 1;
            }
            "--all" => {
                opts.all = true;
                i += 1;
            }
            "--output" | "-o" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--output requires a path".to_string())?;
                opts.output = Some(PathBuf::from(value));
                i += 2;
            }
            "--no-open" => {
                opts.no_open = true;
                i += 1;
            }
            "--include-tests" => {
                opts.include_tests = true;
                i += 1;
            }
            "--min-coupling" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--min-coupling requires a number".to_string())?;
                opts.min_coupling = Some(
                    value
                        .parse()
                        .map_err(|_| "--min-coupling requires a number (0.0-1.0)")?,
                );
                i += 2;
            }
            "--max-module-size" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--max-module-size requires a number".to_string())?;
                opts.max_module_size = Some(
                    value
                        .parse()
                        .map_err(|_| "--max-module-size requires a number")?,
                );
                i += 2;
            }
            _ => {
                // Treat as root path
                if arg.starts_with("--") {
                    return Err(format!("Unknown option '{}' for 'plan' command.", arg));
                }
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
        }
    }

    // Default to markdown if no format specified
    if !opts.markdown && !opts.json && !opts.script && !opts.all {
        opts.markdown = true;
    }

    Ok(Command::Plan(opts))
}

/// Parse `loct cache <list|clean> [options]` command.
pub(super) fn parse_cache_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") || args.is_empty() {
        return Err("loct cache - Manage snapshot cache

USAGE:
    loct cache <SUBCOMMAND> [OPTIONS]

SUBCOMMANDS:
    list                   List the current project's cache bucket (project-local by default)
    clean                  Remove cached snapshots
    prune|gc|clear-stale   Alias for clean, intended for quota/ENOSPC recovery

LIST OPTIONS:
    --project <DIR>        Inspect the cache bucket for a specific project
    --all, --global        Bounded global inventory of every bucket (opt-in;
                           per-bucket walks and total wall time are capped,
                           omissions are reported as lower bounds)

CLEAN OPTIONS:
    --all                  Target every project bucket (requires --force to delete)
    --project <DIR>        Only clean cache for a specific project
    --older-than <DAYS>d   Only remove entries older than N days (e.g., 7d, 30d)
    --max-size <SIZE>      Cap total cache size; evict oldest buckets first
                           (e.g., 1GB, 500MB, 250M, or plain bytes); fails
                           closed if inventory, recency, or size is incomplete
    --force, -f            Skip confirmation prompt

EXAMPLES:
    loct cache list                        # Current project's bucket
    loct cache list --all                  # Bounded global inventory (opt-in)
    loct cache clean --all                 # Preview removal of every bucket
    loct cache clean --all --force         # Remove every bucket
    loct cache clean --project .           # Clean cache for current project
    loct cache clean --older-than 30d      # Remove entries older than 30 days
    loct cache clean --max-size 1GB        # Evict oldest until total < 1 GB"
            .to_string());
    }

    let sub = args[0].as_str();
    let sub_args = &args[1..];

    match sub {
        "list" | "ls" => {
            let mut all = false;
            let mut project = None;
            let mut i = 0;
            while i < sub_args.len() {
                match sub_args[i].as_str() {
                    "--all" | "--global" | "-a" => all = true,
                    "--project" | "-p" => {
                        i += 1;
                        if i >= sub_args.len() {
                            return Err("--project requires a directory argument".to_string());
                        }
                        project = Some(parse_cache_project_arg(&sub_args[i])?);
                    }
                    other => return Err(format!("Unknown cache list option: {}", other)),
                }
                i += 1;
            }
            Ok(Command::Cache(CacheOptions {
                action: CacheAction::List { all, project },
            }))
        }
        "clean" | "rm" | "purge" | "prune" | "gc" | "clear-stale" => {
            let mut all = false;
            let mut project = None;
            let mut older_than = None;
            let mut max_size = None;
            let mut force = false;
            let mut i = 0;
            while i < sub_args.len() {
                match sub_args[i].as_str() {
                    "--all" | "--global" | "-a" => all = true,
                    "--project" | "-p" => {
                        i += 1;
                        if i >= sub_args.len() {
                            return Err("--project requires a directory argument".to_string());
                        }
                        project = Some(parse_cache_project_arg(&sub_args[i])?);
                    }
                    "--older-than" => {
                        i += 1;
                        if i >= sub_args.len() {
                            return Err(
                                "--older-than requires a duration (e.g., 7d, 30d)".to_string()
                            );
                        }
                        older_than = Some(sub_args[i].clone());
                    }
                    "--max-size" => {
                        i += 1;
                        if i >= sub_args.len() {
                            return Err("--max-size requires a size argument (e.g., 1GB, 500MB)"
                                .to_string());
                        }
                        max_size = Some(sub_args[i].clone());
                    }
                    "--force" | "-f" => force = true,
                    other => return Err(format!("Unknown cache clean option: {}", other)),
                }
                i += 1;
            }

            if project.is_some() && (all || older_than.is_some() || max_size.is_some()) {
                return Err(
                    "--project cannot be combined with --all, --older-than, or --max-size"
                        .to_string(),
                );
            }
            if all && (older_than.is_some() || max_size.is_some()) {
                return Err("--all cannot be combined with --older-than or --max-size".to_string());
            }
            if !all && project.is_none() && older_than.is_none() && max_size.is_none() {
                return Err(
                    "Refusing unscoped cache cleanup. Choose --project, --older-than, --max-size, or explicitly pass --all."
                        .to_string(),
                );
            }

            Ok(Command::Cache(CacheOptions {
                action: CacheAction::Clean {
                    all,
                    project,
                    older_than,
                    max_size,
                    force,
                },
            }))
        }
        other => Err(format!(
            "Unknown cache subcommand '{}'. Use 'list', 'clean', or 'prune'.",
            other
        )),
    }
}

/// `--project ""` must not parse as cwd. Empty/whitespace is not an address.
fn parse_cache_project_arg(raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err(
            "Refusing empty --project. Pass an explicit project directory; an empty string is not cwd."
                .to_string(),
        );
    }
    Ok(PathBuf::from(raw))
}

/// Parse `loct prune-old-artifacts [PATH] [OPTIONS]` — local `.loctree/` housekeeping.
pub(super) fn parse_prune_old_artifacts_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "loct prune-old-artifacts - Prune old per-branch snapshot artifacts

USAGE:
    loct prune-old-artifacts [PATH] [OPTIONS]

OPTIONS:
    --root <PATH>      Project root to scan (default: current directory)
    --keep <N>         Keep N newest per-branch snapshots per `.loctree/` (default: 3)
    --include-sub      Also walk into sub-`.loctree/` dirs (e.g. `src-tauri/.loctree/`)
    --apply            Actually delete files (default: dry-run preview)
    --help, -h         Show this help message

EXAMPLES:
    loct prune-old-artifacts                       # Dry-run preview, root only
    loct prune-old-artifacts --apply               # Actually delete in root .loctree/
    loct prune-old-artifacts --include-sub         # Preview including sub-projects
    loct prune-old-artifacts --include-sub --apply # Full sweep, hard apply
    loct prune-old-artifacts --keep 5 --apply      # Keep 5 newest per dir"
        );
        std::process::exit(0);
    }

    let mut opts = PruneOldArtifactsOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--keep" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--keep requires N".to_string())?;
                opts.keep = v
                    .parse::<usize>()
                    .map_err(|_| "--keep requires a positive number".to_string())?;
                i += 2;
            }
            "--include-sub" => {
                opts.include_sub = true;
                i += 1;
            }
            "--apply" => {
                opts.apply = true;
                i += 1;
            }
            "--root" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--root requires PATH".to_string())?;
                opts.root = Some(PathBuf::from(v));
                i += 2;
            }
            other if !other.starts_with('-') && opts.root.is_none() => {
                opts.root = Some(PathBuf::from(other));
                i += 1;
            }
            other => {
                return Err(format!(
                    "Unknown option '{}' for 'prune-old-artifacts' command.",
                    other
                ));
            }
        }
    }

    Ok(Command::PruneOldArtifacts(opts))
}

/// Parse `loct snapshot-path [PROJECT] [OPTIONS]`.
pub(super) fn parse_snapshot_path_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err(
            crate::cli::command::Command::format_command_help("snapshot-path")
                .unwrap_or("loct snapshot-path")
                .to_string(),
        );
    }

    let mut opts = SnapshotPathOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                opts.json = true;
                i += 1;
            }
            "--verbose" | "-v" => {
                opts.verbose_siblings = true;
                i += 1;
            }
            "--project" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--project requires PATH".to_string())?;
                opts.project = Some(PathBuf::from(v));
                i += 2;
            }
            other if !other.starts_with('-') && opts.project.is_none() => {
                opts.project = Some(PathBuf::from(other));
                i += 1;
            }
            other => {
                return Err(format!(
                    "Unknown option '{}' for 'snapshot-path' command.",
                    other
                ));
            }
        }
    }
    Ok(Command::SnapshotPath(opts))
}

/// Parse `loct inventory [PROJECT] [OPTIONS]`.
pub(super) fn parse_inventory_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err(
            crate::cli::command::Command::format_command_help("inventory")
                .unwrap_or("loct inventory")
                .to_string(),
        );
    }

    let mut opts = InventoryOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            // Default output is already JSONL; accept the flag as a no-op alias.
            "--jsonl" => {
                i += 1;
            }
            "--receipt-only" => {
                opts.receipt_only = true;
                i += 1;
            }
            "--no-receipt" => {
                opts.no_receipt = true;
                i += 1;
            }
            "--include-tests" => {
                opts.include_tests = true;
                i += 1;
            }
            "--include-generated" => {
                opts.include_generated = true;
                i += 1;
            }
            "--units-only" => {
                opts.units_only = true;
                i += 1;
            }
            "--prefix" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--prefix requires PATH".to_string())?;
                opts.path_prefix = Some(v.clone());
                i += 2;
            }
            "--project" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--project requires PATH".to_string())?;
                opts.project = Some(PathBuf::from(v));
                i += 2;
            }
            other if !other.starts_with('-') && opts.project.is_none() => {
                opts.project = Some(PathBuf::from(other));
                i += 1;
            }
            other => {
                return Err(format!(
                    "Unknown option '{}' for 'inventory' command.",
                    other
                ));
            }
        }
    }

    if opts.receipt_only && opts.no_receipt {
        return Err("--receipt-only and --no-receipt are mutually exclusive".to_string());
    }

    Ok(Command::Inventory(opts))
}

/// Parse `loct atlas [PROJECT] [OPTIONS]`.
pub(super) fn parse_atlas_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err(crate::cli::command::Command::format_command_help("atlas")
            .unwrap_or("loct atlas")
            .to_string());
    }

    let mut opts = AtlasOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                opts.json = true;
                i += 1;
            }
            "--out" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--out requires DIR".to_string())?;
                opts.out_dir = Some(PathBuf::from(v));
                i += 2;
            }
            "--project" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--project requires PATH".to_string())?;
                opts.project = Some(PathBuf::from(v));
                i += 2;
            }
            other if !other.starts_with('-') && opts.project.is_none() => {
                opts.project = Some(PathBuf::from(other));
                i += 1;
            }
            other => {
                return Err(format!("Unknown option '{}' for 'atlas' command.", other));
            }
        }
    }
    Ok(Command::Atlas(opts))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_crowd_command() {
        let args = vec!["message".into()];
        let result = parse_crowd_command(&args).expect("parse crowd command");
        if let Command::Crowd(opts) = result {
            assert_eq!(opts.pattern, Some("message".into()));
        } else {
            panic!("Expected Crowd command");
        }
    }

    #[test]
    fn test_parse_crowd_auto_detect() {
        let args = vec!["--auto".into()];
        let result = parse_crowd_command(&args).expect("parse crowd auto command");
        if let Command::Crowd(opts) = result {
            assert!(opts.auto_detect);
            assert!(opts.pattern.is_none());
        } else {
            panic!("Expected Crowd command");
        }
    }

    #[test]
    fn test_parse_tagmap_command() {
        let args = vec!["patient".into()];
        let result = parse_tagmap_command(&args).expect("parse tagmap command");
        if let Command::Tagmap(opts) = result {
            assert_eq!(opts.keyword, "patient");
        } else {
            panic!("Expected Tagmap command");
        }
    }

    #[test]
    fn test_parse_suppress_list() {
        let args = vec!["--list".into()];
        let result = parse_suppress_command(&args).expect("parse suppress command");
        if let Command::Suppress(opts) = result {
            assert!(opts.list);
        } else {
            panic!("Expected Suppress command");
        }
    }

    #[test]
    fn test_parse_help_command() {
        let args = vec!["scan".into()];
        let result = parse_help_command(&args).expect("parse help command");
        if let Command::Help(opts) = result {
            assert_eq!(opts.command, Some("scan".into()));
        } else {
            panic!("Expected Help command");
        }
    }

    #[test]
    fn test_parse_cache_list() {
        let args = vec!["list".into()];
        let result = parse_cache_command(&args).expect("parse cache list command");
        assert!(matches!(
            result,
            Command::Cache(CacheOptions {
                action: CacheAction::List {
                    all: false,
                    project: None
                }
            })
        ));
    }

    /// Audit class H: the global cache walk is opt-in via `--all`/`--global`;
    /// `--project` narrows the project-local default explicitly.
    #[test]
    fn test_parse_cache_list_all_and_project() {
        let args = vec!["list".into(), "--all".into()];
        let result = parse_cache_command(&args).expect("parse cache list --all");
        assert!(matches!(
            result,
            Command::Cache(CacheOptions {
                action: CacheAction::List { all: true, .. }
            })
        ));

        let args = vec!["list".into(), "--global".into()];
        let result = parse_cache_command(&args).expect("parse cache list --global");
        assert!(matches!(
            result,
            Command::Cache(CacheOptions {
                action: CacheAction::List { all: true, .. }
            })
        ));

        let args = vec!["list".into(), "--project".into(), "/tmp/demo".into()];
        let result = parse_cache_command(&args).expect("parse cache list --project");
        match result {
            Command::Cache(CacheOptions {
                action: CacheAction::List { all, project },
            }) => {
                assert!(!all);
                assert_eq!(project, Some(PathBuf::from("/tmp/demo")));
            }
            other => panic!("expected cache list options, got {other:?}"),
        }

        let args = vec!["list".into(), "--project".into()];
        assert!(
            parse_cache_command(&args).is_err(),
            "--project without a value must be rejected"
        );
    }

    /// W4-04 / G-CACHE-GC: empty `--project` is a parser error, not cwd.
    #[test]
    fn w4_04_cache_clean_empty_project_refused() {
        for empty in ["", "   ", "\t"] {
            let args = vec![
                "clean".into(),
                "--project".into(),
                empty.into(),
                "--force".into(),
            ];
            let err = parse_cache_command(&args)
                .expect_err("empty --project must be a parser error, not cwd");
            assert!(
                err.contains("Refusing empty --project"),
                "empty {empty:?} must name the empty-project refusal, got: {err}"
            );
            assert!(
                err.contains("empty string is not cwd"),
                "refusal must say empty is not cwd, got: {err}"
            );
        }

        let list_err = parse_cache_command(&["list".into(), "--project".into(), "".into()])
            .expect_err("empty --project on list is also not cwd");
        assert!(list_err.contains("Refusing empty --project"));
    }

    #[test]
    fn test_parse_cache_clean_all() {
        let args: Vec<String> = vec!["clean".into(), "--all".into(), "--force".into()];
        let result = parse_cache_command(&args).expect("parse cache clean command");
        if let Command::Cache(CacheOptions {
            action: CacheAction::Clean { all, force, .. },
        }) = result
        {
            assert!(all);
            assert!(force);
        } else {
            panic!("Expected Cache Clean command");
        }
    }

    #[test]
    fn test_parse_cache_clean_rejects_unscoped_and_incompatible_options() {
        for args in [
            vec!["clean".into(), "--force".into()],
            vec!["prune".into(), "--force".into()],
            vec![
                "clean".into(),
                "--project".into(),
                "/tmp/demo".into(),
                "--older-than".into(),
                "7d".into(),
            ],
            vec![
                "clean".into(),
                "--all".into(),
                "--max-size".into(),
                "1GB".into(),
            ],
        ] {
            assert!(
                parse_cache_command(&args).is_err(),
                "unsafe or incompatible clean options must be rejected: {args:?}"
            );
        }
    }

    #[test]
    fn test_parse_cache_prune_quota_alias() {
        let args: Vec<String> = vec![
            "prune".into(),
            "--max-size".into(),
            "1GB".into(),
            "--force".into(),
        ];
        let result = parse_cache_command(&args).expect("parse cache prune command");
        if let Command::Cache(CacheOptions {
            action: CacheAction::Clean {
                force, max_size, ..
            },
        }) = result
        {
            assert!(force);
            assert_eq!(max_size, Some("1GB".to_string()));
        } else {
            panic!("Expected Cache Clean command");
        }
    }

    #[test]
    fn test_parse_health_command() {
        let args = vec!["--include-tests".into()];
        let result = parse_health_command(&args).expect("parse health command");
        if let Command::Health(opts) = result {
            assert!(opts.include_tests);
        } else {
            panic!("Expected Health command");
        }
    }

    #[test]
    fn test_parse_audit_command_defaults_to_full_report() {
        let args: Vec<String> = vec![];
        let result = parse_audit_command(&args).expect("parse audit command");
        if let Command::Audit(opts) = result {
            assert_eq!(opts.limit, None);
            assert!(!opts.todos);
        } else {
            panic!("Expected Audit command");
        }
    }

    #[test]
    fn test_parse_audit_command_accepts_explicit_limit() {
        let args = vec!["--limit".into(), "7".into()];
        let result = parse_audit_command(&args).expect("parse audit command with limit");
        if let Command::Audit(opts) = result {
            assert_eq!(opts.limit, Some(7));
        } else {
            panic!("Expected Audit command");
        }
    }

    #[test]
    fn test_parse_audit_command_rejects_stdout() {
        let args = vec!["--stdout".into()];
        let err = parse_audit_command(&args).expect_err("audit should reject stdout");
        assert!(err.contains("writes markdown reports to an artifact file only"));
    }

    #[test]
    fn test_parse_snapshot_path_command() {
        let args = vec!["--json".into(), "/tmp/proj".into()];
        let result = parse_snapshot_path_command(&args).expect("parse snapshot-path");
        if let Command::SnapshotPath(opts) = result {
            assert!(opts.json);
            assert_eq!(opts.project, Some(PathBuf::from("/tmp/proj")));
        } else {
            panic!("Expected SnapshotPath command");
        }
    }

    #[test]
    fn test_parse_inventory_command_flags() {
        let args = vec![
            "--receipt-only".into(),
            "--units-only".into(),
            "--prefix".into(),
            "src/".into(),
        ];
        let result = parse_inventory_command(&args).expect("parse inventory");
        if let Command::Inventory(opts) = result {
            assert!(opts.receipt_only);
            assert!(opts.units_only);
            assert_eq!(opts.path_prefix.as_deref(), Some("src/"));
        } else {
            panic!("Expected Inventory command");
        }
    }

    #[test]
    fn test_parse_inventory_rejects_receipt_only_with_no_receipt() {
        let args = vec!["--receipt-only".into(), "--no-receipt".into()];
        let err = parse_inventory_command(&args).expect_err("mutually exclusive");
        assert!(err.contains("mutually exclusive"));
    }

    #[test]
    fn test_parse_atlas_command() {
        let args = vec!["--out".into(), "/tmp/atlas".into(), "--json".into()];
        let result = parse_atlas_command(&args).expect("parse atlas");
        if let Command::Atlas(opts) = result {
            assert!(opts.json);
            assert_eq!(opts.out_dir, Some(PathBuf::from("/tmp/atlas")));
        } else {
            panic!("Expected Atlas command");
        }
    }
}
