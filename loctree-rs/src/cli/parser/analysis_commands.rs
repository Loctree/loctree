//! Parsers for code analysis commands: dead, cycles, find, query, impact, twins.
//!
//! These commands analyze the codebase for issues, patterns, and relationships.

use std::path::PathBuf;

use super::super::command::{
    BodyOptions, Command, CyclesOptions, DeadOptions, FindOptions, ImpactCommandOptions,
    OccurrencesOptions, QueryKind, QueryOptions, TwinsOptions,
};

/// Parse `loct dead [options]` command - detect unused exports.
pub(super) fn parse_dead_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct dead - Detect unused exports / dead code

USAGE:
    loct dead [OPTIONS] [PATHS...]

DESCRIPTION:
    Finds exported symbols that are never imported anywhere in the codebase.
    Uses import graph analysis with alias-awareness to minimize false positives.

OPTIONS:
    --confidence <LEVEL>   Filter by confidence: high, medium, low (default: all)
    --top <N>              Limit to top N results (default: 20)
    --full, --all          Show all results (ignore top limit)
    --path <PATTERN>       Filter to files matching pattern
    --with-tests           Include test files in analysis
    --exclude-tests        Exclude test files (default)
    --with-helpers         Include helper/utility files
    --workspace-closed     Treat pub in library crates as crate-internal
    --help, -h             Show this help message

EXAMPLES:
    loct dead                          # All dead exports
    loct dead --confidence high        # Only high-confidence
    loct dead --path src/components/   # Dead exports in components
    loct dead --top 50                 # Top 50 dead exports
    loct dead --workspace-closed       # Flag unused pub in lib crates"
            .to_string());
    }

    let mut opts = DeadOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--confidence" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    "--confidence requires a value (high, medium, low)".to_string()
                })?;
                opts.confidence = Some(value.clone());
                i += 2;
            }
            "--top" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--top requires a number".to_string())?;
                opts.top = Some(value.parse().map_err(|_| "--top requires a number")?);
                i += 2;
            }
            "--full" | "--all" => {
                opts.full = true;
                i += 1;
            }
            "--path" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--path requires a pattern".to_string())?;
                opts.path_filter = Some(value.clone());
                i += 2;
            }
            "--with-tests" => {
                opts.with_tests = true;
                i += 1;
            }
            "--exclude-tests" => {
                opts.with_tests = false;
                i += 1;
            }
            "--with-helpers" => {
                opts.with_helpers = true;
                i += 1;
            }
            "--with-shadows" => {
                opts.with_shadows = true;
                i += 1;
            }
            "--with-ambient" | "--include-ambient" => {
                opts.with_ambient = true;
                i += 1;
            }
            "--with-dynamic" | "--include-dynamic" => {
                opts.with_dynamic = true;
                i += 1;
            }
            "--workspace-closed" => {
                opts.workspace_closed = true;
                i += 1;
            }
            _ if !arg.starts_with('-') => {
                opts.roots.push(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'dead' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Dead(opts))
}

/// Parse `loct cycles [options]` command - detect circular imports.
pub(super) fn parse_cycles_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct cycles - Detect circular import chains

USAGE:
    loct cycles [OPTIONS] [PATHS...]

DESCRIPTION:
    Detects circular dependencies in your import graph and classifies them
    by compilability impact.

OPTIONS:
    --path <PATTERN>     Filter to files matching path pattern
    --breaking-only      Only show cycles that would break compilation
    --explain            Show detailed explanation for each cycle
    --legacy             Use legacy output format (old grouping by pattern)
    --include-artifacts  Disable the artifact fence (report fixture/vendored
                         cycles in the main section)
    --help, -h           Show this help message

EXAMPLES:
    loct cycles                       # Show all cycles with new format
    loct cycles --breaking-only       # Only show compilation-breaking cycles
    loct cycles --explain             # Detailed pattern explanations"
            .to_string());
    }

    let mut opts = CyclesOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--path" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--path requires a pattern".to_string())?;
                opts.path_filter = Some(value.clone());
                i += 2;
            }
            "--breaking-only" => {
                opts.breaking_only = true;
                i += 1;
            }
            "--explain" => {
                opts.explain = true;
                i += 1;
            }
            "--legacy" => {
                opts.legacy_format = true;
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
                return Err(format!("Unknown option '{}' for 'cycles' command.", arg));
            }
        }
    }

    if opts.roots.is_empty() {
        opts.roots.push(PathBuf::from("."));
    }

    Ok(Command::Cycles(opts))
}

/// Build a friendly "unknown option" error for `find`, redirecting the most
/// common docs/runtime drift mistakes (from loctree-fail.md) to the real syntax
/// instead of the bare generic message.
fn find_unknown_option_error(arg: &str) -> String {
    let hint = match arg {
        "--format" => Some(
            "`find` has no `--format`; use the global `--json` flag for machine output \
             (`loct find <query> --json`).",
        ),
        "--group-by" => Some("did you mean `--group-by-file`? (literal mode only)"),
        "--count" => Some("did you mean `--count-only` / `--slim`? (literal mode only)"),
        "--where" | "--wheresymbol" => {
            Some("did you mean `--where-symbol` (or `loct query where-symbol <SYMBOL>`)?")
        }
        _ => None,
    };
    match hint {
        Some(h) => format!("Unknown option '{arg}' for 'find' command. {h}"),
        None => format!(
            "Unknown option '{arg}' for 'find' command. Search modes are direct flags: \
             --literal, --regex, --where-symbol, --who-imports, --dead, --exported (or \
             `--mode <name>`). Filters: --symbol/-s, --file/-f, --lang, --limit. Output: global \
             --json. Run `loct find --help` for the full list."
        ),
    }
}

/// Parse `loct find [options]` command - semantic search for symbols.
pub(super) fn parse_find_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct find - Exact literal search by default; broad discovery on request

USAGE:
    loct find [QUERY...] [OPTIONS]

DESCRIPTION:
    Plain `loct find QUERY` is the truth layer: an exact identifier-boundary
    scan over source bytes (the same substrate as `loct occurrences`).
    Use --discover for the broad AST/parameter/fuzzy engine. Discovery is useful
    for exploration, but its candidates are not literal evidence.

OPTIONS:
    --literal                           Explicit alias for the default literal mode
    --discover                          Broad AST/parameter/fuzzy discovery (explicit opt-in)
    --regex                             Regex over raw file TEXT (not just identifiers); keeps coverage
                                        accounting + context labels. For secret/privacy audits where
                                        --literal cannot evaluate a pattern. Mutually exclusive with --literal.
    --whole-token                       (literal) Treat '-' as token-internal: 'backdrop' no longer matches
                                        inside 'overlay-backdrop'/'--sample-z-overlay-backdrop' (opt-in, no default change)
    --group-by-file                     (literal) Add a per-file occurrence rollup ('by_file')
    --count-only, --slim                (literal) Suppress the full occurrence list, keep counters only
    --compact                           (literal) Terse path:line human output
    --offset <N>                        (literal) Zero-based occurrence offset for paged output
    --root <PATH>, --project <PATH>     Project root to scan (default: current directory)
    --include-untracked                 Scan fresh untracked files in-memory (does not mutate snapshot)
    --path <PATTERN>                    Alias for --file (path/suffix scope in literal mode)
    --or                                Combine multiple QUERY args with OR (legacy behavior)
    --symbol <PATTERN>, -s <PATTERN>    Search for symbols matching regex
    --pattern <PATTERN>                 Alias for --symbol (regex)
    --file <PATTERN>, -f <PATTERN>      Search for files matching regex; in --literal, exact path/suffix scope
    --similar <SYMBOL>                  Find symbols with similar names (fuzzy)
    --who-imports                       Find files that import QUERY (same graph path as `loct query who-imports`)
    --where-symbol                      Resolve where a symbol is defined/exported (same as `loct query where-symbol`)
    --dead                              Only show dead/unused symbols
    --exported                          Only show exported symbols
    --mode <NAME>                       Alias that dispatches to a mode flag instead of typing the flag directly:
                                        literal | regex | where-symbol | who-imports | dead | exported | discover.
                                        e.g. `--mode where-symbol` == `--where-symbol`. Compat shim for `--mode <x>` muscle memory.
    --lang <LANG>                       Filter by language (ts, rs, js, py, etc.)
    --limit <N>                         Maximum results to show (default: 50 literal, 25 where-symbol)
    --all                               Emit all literal/where-symbol results (explicitly unbounded)
    --help, -h                          Show this help message

EXAMPLES:
    loct find Patient                   # Every exact identifier-boundary occurrence
    loct find --discover Patient        # Broad AST/parameter/fuzzy discovery
    loct find --discover Props Options ViewModel # Split discovery + cross-match
    loct find --discover --or foo bar baz # Legacy OR discovery
    loct find --symbol \".*Config$\"      # Regex: symbols ending with Config
    loct find --literal utterance_id    # Literal truth: every exact occurrence
    loct find --literal utterance_id --json  # Literal matches as JSON (literal_matches section)
    loct find --literal backdrop --whole-token   # Exclude hyphenated z-index noise
    loct find --literal agent --limit 50 --offset 100 --json  # Page through large literal result sets
    loct find --literal backdrop --group-by-file --count-only --json  # Per-file counts, no list
    loct find --regex '100\\.[0-9]+\\.[0-9]+' --json  # Pattern scan with coverage (secret/privacy audit)
    loct find --regex 'AKIA[0-9A-Z]{16}'         # AWS-key shape over raw text, fenced + labeled"
            .to_string());
    }

    let mut opts = FindOptions::default();
    let mut queries: Vec<String> = Vec::new();
    let mut who_imports = false;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--or" => {
                opts.or_mode = true;
                i += 1;
            }
            "--symbol" | "-s" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--symbol requires a pattern".to_string())?;
                opts.symbol = Some(value.clone());
                i += 2;
            }
            "--pattern" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    "--pattern requires a pattern (alias for --symbol)".to_string()
                })?;
                opts.symbol = Some(value.clone());
                i += 2;
            }
            "--file" | "-f" | "--path" => {
                // `--path` is a natural alias for file-scope filtering (loctree-fail:
                // agents type `find --path <dir>` after learning `--path` on dead/routes).
                let flag = arg.as_str();
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{flag} requires a pattern"))?;
                opts.file = Some(value.clone());
                i += 2;
            }
            "--root" | "--project" => {
                // Help advertises `--root`; agents also pass `--project` (parity with
                // `loct context --project`). Both set the scan root for find.
                let flag = arg.as_str();
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{flag} requires a path"))?;
                opts.roots.push(PathBuf::from(value));
                i += 2;
            }
            "--compact" => {
                opts.compact = true;
                i += 1;
            }
            "--include-untracked" => {
                opts.include_untracked = true;
                i += 1;
            }
            "--impact" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--impact requires a file path".to_string())?;
                opts.impact = Some(value.clone());
                i += 2;
            }
            "--similar" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--similar requires a symbol name".to_string())?;
                opts.similar = Some(value.clone());
                i += 2;
            }
            "--literal" => {
                opts.literal = true;
                i += 1;
            }
            "--discover" => {
                opts.discover = true;
                i += 1;
            }
            "--regex" => {
                opts.regex = true;
                i += 1;
            }
            "--whole-token" => {
                opts.whole_token = true;
                i += 1;
            }
            "--group-by-file" => {
                opts.group_by_file = true;
                i += 1;
            }
            "--count-only" | "--slim" => {
                opts.count_only = true;
                i += 1;
            }
            "--where-symbol" => {
                opts.where_symbol = true;
                i += 1;
            }
            "--mode" => {
                // Compat alias for `--mode <x>` muscle memory (loctree-fail.md: agents
                // repeatedly type `loct find --mode where-symbol` / `--mode literal` and
                // hit `Unknown option '--mode'`). Dispatch cleanly onto the existing mode
                // flags where the mapping is 1:1; refuse ambiguous/unknown modes with a
                // pointed message instead of the generic unknown-option error.
                let value = args.get(i + 1).ok_or_else(|| {
                    "--mode requires a value: literal | regex | where-symbol | who-imports | \
                     dead | exported | fuzzy. Each is also a direct flag (e.g. `--where-symbol`)."
                        .to_string()
                })?;
                match value.as_str() {
                    "literal" => opts.literal = true,
                    "regex" => opts.regex = true,
                    "where-symbol" | "where_symbol" => opts.where_symbol = true,
                    "who-imports" | "who_imports" => who_imports = true,
                    "dead" => opts.dead_only = true,
                    "exported" => opts.exported_only = true,
                    "discover" | "fuzzy" | "ast" | "symbol" => opts.discover = true,
                    "default" => opts.literal = true,
                    "similar" => {
                        return Err(
                            "`--mode similar` needs a target symbol. Use `loct find --similar <SYMBOL>` instead."
                                .to_string(),
                        );
                    }
                    other => {
                        return Err(format!(
                            "Unknown find --mode '{other}'. Valid modes: literal, regex, where-symbol, \
                             who-imports, dead, exported, fuzzy. Each is also a direct flag (e.g. \
                             `--where-symbol`); for a where-symbol lookup you can also run \
                             `loct query where-symbol <SYMBOL>`."
                        ));
                    }
                }
                i += 2;
            }
            "--who-imports" => {
                who_imports = true;
                i += 1;
            }
            "--dead" => {
                opts.dead_only = true;
                i += 1;
            }
            "--exported" => {
                opts.exported_only = true;
                i += 1;
            }
            "--lang" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--lang requires a language".to_string())?;
                opts.lang = Some(value.clone());
                i += 2;
            }
            "--limit" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--limit requires a number".to_string())?;
                opts.limit = Some(value.parse().map_err(|_| "--limit requires a number")?);
                i += 2;
            }
            "--all" => {
                opts.all = true;
                i += 1;
            }
            "--offset" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--offset requires a number".to_string())?;
                opts.offset = value.parse().map_err(|_| "--offset requires a number")?;
                i += 2;
            }
            "--" => {
                // Support `loct find --literal -- "--config-dir"` and similar for dash-prefixed
                // literals (REPEAT from loctree-fail.md 2903,2978,2990,3052). The `--` separator
                // ends option parsing; following tokens (even starting with -) are taken as queries.
                // This also aids --literal queries that look like flags.
                i += 1;
                while i < args.len() {
                    queries.push(args[i].clone());
                    i += 1;
                }
                // i advanced; break to avoid reprocessing
                break;
            }
            _ if !arg.starts_with('-') => {
                // Collect all positional args as queries (multi-query support!)
                queries.push(arg.clone());
                i += 1;
            }
            _ => {
                // After --literal/--regex, be lenient with a single following dashed token as the
                // query itself (e.g. loct find --literal "--prompt-file", or a regex like
                // `--regex "-?[0-9]+"`) before falling to error.
                if (opts.literal || opts.regex) && queries.is_empty() && arg.starts_with('-') {
                    queries.push(arg.clone());
                    i += 1;
                } else {
                    return Err(find_unknown_option_error(arg));
                }
            }
        }
    }

    // Preserve positional queries as provided; dispatch decides split/AND/OR behavior.
    if !queries.is_empty() {
        opts.queries = queries.clone();
    }

    if opts.literal && opts.regex {
        return Err(
            "--literal and --regex are mutually exclusive: --literal is exact-string truth, \
             --regex evaluates a pattern. Pick one."
                .to_string(),
        );
    }

    let discovery_selected = opts.discover
        || opts.or_mode
        || opts.symbol.is_some()
        || opts.impact.is_some()
        || opts.similar.is_some()
        || opts.dead_only
        || opts.exported_only
        || opts.lang.is_some()
        || (opts.file.is_some() && queries.is_empty());

    if opts.discover && (opts.literal || opts.regex) {
        return Err(
            "--discover cannot be combined with --literal or --regex: choose one evidence mode"
                .to_string(),
        );
    }

    if opts.all && opts.limit.is_some() {
        return Err("--all and --limit are mutually exclusive".to_string());
    }

    if !discovery_selected && !opts.regex && !opts.where_symbol && !who_imports {
        // Multi-arg and `A|B` are multi-literal OR on the exact-truth substrate
        // (not discovery). Agents used to get silent fixed_string-0 on pipes and
        // fall back to grep — multi-literal closes that hole.
        opts.literal = true;
    }

    if opts.literal && !opts.all && opts.limit.is_none() {
        opts.limit = Some(50);
    }

    if who_imports {
        if opts.symbol.is_some()
            || opts.file.is_some()
            || opts.impact.is_some()
            || opts.similar.is_some()
            || opts.literal
            || opts.where_symbol
            || opts.dead_only
            || opts.exported_only
            || opts.lang.is_some()
        {
            return Err(
                "--who-imports cannot be combined with other find modes or filters".to_string(),
            );
        }
        if queries.len() != 1 {
            return Err(
                "--who-imports requires exactly one file or symbol target. Usage: loct find <target> --who-imports"
                    .to_string(),
            );
        }
        let target = queries
            .first()
            .map(|q| q.trim())
            .filter(|q| !q.is_empty())
            .ok_or_else(|| {
                "--who-imports requires exactly one file or symbol target. Usage: loct find <target> --who-imports"
                    .to_string()
            })?;
        return Ok(Command::Query(QueryOptions {
            kind: QueryKind::WhoImports,
            target: target.to_string(),
            limit: None,
            all: false,
            // Preserve find --root/--project so who-imports does not silently
            // re-home to cwd (loctree-fail / skeptic: wrong universe).
            roots: opts.roots.clone(),
        }));
    }

    // Validate that at least one search criterion is specified and not empty
    let effective_query = opts
        .query
        .as_ref()
        .or_else(|| opts.queries.first())
        .or(opts.symbol.as_ref())
        .or(opts.file.as_ref())
        .or(opts.similar.as_ref())
        .or(opts.impact.as_ref());

    if effective_query.is_some_and(|q| q.trim().is_empty()) {
        return Err("Error: Query cannot be empty".to_string());
    }

    Ok(Command::Find(opts))
}

/// Parse `loct occurrences <ident>` command - literal exact-identifier scan.
pub(super) fn parse_occurrences_command(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err(
            "loct occurrences - Literal exact-identifier scan (truth layer)

USAGE:
    loct occurrences <IDENT> [OPTIONS]

DESCRIPTION:
    Walks source bytes of files in the current snapshot and reports every
    identifier-boundary occurrence of <IDENT>. Token-aware (not naive
    substring), literal only (no fuzzy suggestions promoted as primary).
    'Not found' means no match in the stated snapshot scope; it is not proof
    about ignored, generated, unsupported, or otherwise unindexed files.

OPTIONS:
    --root <PATH>        Project root to scan (default: current directory)
    --include-untracked  Scan fresh untracked files in-memory (does not mutate snapshot)
    --regex              Evaluate <IDENT> as a regular expression over raw file text
                         (same engine, coverage line and paging as 'find --regex')
    --whole-token        Treat '-' as token-internal: 'backdrop' no longer matches inside
                         'overlay-backdrop'/'--sample-z-overlay-backdrop' (opt-in, no default change)
    --group-by-file      Add a per-file occurrence rollup ('by_file')
    --count-only, --slim Suppress the full occurrence list, keep counters only ('slim')
    --compact            Human output only: print path:line plus one context line per hit
    --limit <N>          Maximum number of occurrences to return in this page
    --offset <N>         Zero-based occurrence offset for paged output
    --json               Emit JSON (file, line, column, matched_text, context, source, occurrence_kind)
    --help, -h           Show this help message

EXAMPLES:
    loct occurrences utterance_id
    loct occurrences utterance_id --json
    loct occurrences backdrop --whole-token            # Exclude hyphenated z-index noise
    loct occurrences utterance_id --compact            # Terse path:line context for agents
    loct occurrences agent --limit 50 --offset 100 --json  # Page through large result sets
    loct occurrences backdrop --group-by-file --count-only --json  # Per-file counts, no list
    loct occurrences 'twin\\w+' --regex                    # Pattern mode, in parity with find --regex"
                .to_string(),
        );
    }

    let mut opts = OccurrencesOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--root" | "--project" => {
                // Same pair `find` accepts: help advertises `--root`, agents reach for
                // `--project` by analogy with `loct context --project`. Occurrences is
                // the literal oracle agents cross-check `find` against, so rejecting
                // the spelling that just worked on `find` reads as "this project has
                // no occurrences" rather than "wrong flag".
                let flag = arg.as_str();
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{flag} requires a path"))?;
                opts.roots.push(PathBuf::from(value));
                i += 2;
            }
            "--include-untracked" => {
                opts.include_untracked = true;
                i += 1;
            }
            "--regex" => {
                opts.regex = true;
                i += 1;
            }
            "--whole-token" => {
                opts.whole_token = true;
                i += 1;
            }
            "--group-by-file" => {
                opts.group_by_file = true;
                i += 1;
            }
            "--count-only" | "--slim" => {
                opts.count_only = true;
                i += 1;
            }
            "--compact" => {
                opts.compact = true;
                i += 1;
            }
            "--limit" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--limit requires a number".to_string())?;
                opts.limit = Some(value.parse().map_err(|_| "--limit requires a number")?);
                i += 2;
            }
            "--offset" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--offset requires a number".to_string())?;
                opts.offset = value.parse().map_err(|_| "--offset requires a number")?;
                i += 2;
            }
            "--" => {
                // Support `loct occurrences -- "--config-dir"` (dash-prefixed literal idents).
                // Mirrors the find --literal -- <str> fix (loctree-fail.md 2903 et al).
                i += 1;
                if i < args.len() && opts.ident.is_empty() {
                    opts.ident = args[i].clone();
                }
                break;
            }
            _ if !arg.starts_with('-') => {
                if opts.ident.is_empty() {
                    opts.ident = arg.clone();
                } else {
                    return Err(format!(
                        "Unexpected argument '{}'. occurrences takes one identifier.",
                        arg
                    ));
                }
                i += 1;
            }
            _ => {
                return Err(format!(
                    "Unknown option '{}' for 'occurrences' command.",
                    arg
                ));
            }
        }
    }

    if opts.ident.trim().is_empty() {
        return Err(
            "'occurrences' command requires an identifier. Usage: loct occurrences <ident>"
                .to_string(),
        );
    }

    Ok(Command::Occurrences(opts))
}

/// Parse `loct query <kind> <target>` command - graph queries.
pub(super) fn parse_query_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct query - Graph queries (who-imports, who-exports, etc.)

USAGE:
    loct query <KIND> <TARGET> [OPTIONS]

QUERY KINDS:
    who-imports <FILE>        Find all files that import the specified file
    where-symbol <SYMBOL>     Find exact definitions (25 results by default)
    component-of <FILE>       Show which components/modules contain this file
    swift-types <SWIFT_FILE>  Classify Swift type-position references

OPTIONS:
    --limit <N>              Cap public results (kind default when omitted)
    --all                    Emit the complete result set
    --root <PATH>            Project root to scan (default: current directory)
    --project <PATH>         Alias for --root

EXAMPLES:
    loct query who-imports src/utils.ts
    loct query where-symbol PatientRecord
    loct query where-symbol Foo --project /path/to/sibling
    loct query swift-types Sources/App/AppController.swift"
            .to_string());
    }

    if args.len() < 2 {
        return Err(
            "query command requires a kind and target.\nUsage: loct query <kind> <target>\nKinds: who-imports, where-symbol, component-of"
                .to_string(),
        );
    }

    let kind_str = &args[0];
    let target = args[1].clone();
    let mut limit = None;
    let mut all = false;
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--limit" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--limit requires a number".to_string())?;
                limit = Some(value.parse().map_err(|_| "--limit requires a number")?);
                i += 2;
            }
            "--all" => {
                all = true;
                i += 1;
            }
            "--root" | "--project" => {
                let flag = args[i].as_str();
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{flag} requires a path"))?;
                roots.push(PathBuf::from(value));
                i += 2;
            }
            other => return Err(format!("Unknown query option '{other}'")),
        }
    }
    if all && limit.is_some() {
        return Err("--all and --limit are mutually exclusive".to_string());
    }

    let kind = match kind_str.as_str() {
        "who-imports" => QueryKind::WhoImports,
        "where-symbol" => QueryKind::WhereSymbol,
        "component-of" => QueryKind::ComponentOf,
        "swift-types" | "swift-type-refs" => QueryKind::SwiftTypes,
        _ => {
            return Err(format!(
                "Unknown query kind '{}'. Valid kinds: who-imports, where-symbol, component-of, swift-types",
                kind_str
            ));
        }
    };

    Ok(Command::Query(QueryOptions {
        kind,
        target,
        limit,
        all,
        roots,
    }))
}

/// Parse `loct body <symbol> [options]` command - bounded symbol source retrieval.
pub(super) fn parse_body_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct body - Show the bounded source body/range of a symbol

USAGE:
    loct body <SYMBOL> [OPTIONS]

DESCRIPTION:
    Once `where-symbol` locates a symbol, `body` shows the actual source
    lines of its definition directly from indexed source.
    Body extents are structural: brace balancing (Rust, Swift, TS/JS,
    C-family), indentation blocks for Python def/class, bracket-balanced
    collection consts, and single-line statements. When no extent can be
    proven the result falls back to a fixed window and is reported as
    truncated (extent \"window\").

OPTIONS:
    --max-lines <N>   Cap source lines returned per body (default: 200)
    --file <PATH>     Qualify an ambiguous symbol to one defining file
                      (exact repo-relative path or path suffix)
    --root <PATH>     Project root to scan (default: current directory)
    --project <PATH>  Alias for --root
    --json            Emit JSON (file, start/end line, language, source,
                      truncated, extent)
    --help, -h        Show this help message

EXAMPLES:
    loct body transcription_session
    loct body handle_query_command --max-lines 80
    loct body build --file src/beta.py
    loct body query_where_symbol --json
    loct body helper --root /path/to/project"
            .to_string());
    }

    let mut symbol: Option<String> = None;
    let mut line_cap: Option<usize> = None;
    let mut file: Option<String> = None;
    let mut root: Option<PathBuf> = None;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--max-lines" | "--line-cap" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--max-lines requires a number".to_string())?;
                line_cap = Some(value.parse().map_err(|_| "--max-lines requires a number")?);
                i += 2;
            }
            "--file" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--file requires a path".to_string())?;
                file = Some(value.clone());
                i += 2;
            }
            "--root" | "--project" => {
                let flag = args[i].as_str();
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{flag} requires a path"))?;
                root = Some(PathBuf::from(value));
                i += 2;
            }
            _ if !arg.starts_with('-') => {
                if symbol.is_none() {
                    symbol = Some(arg.clone());
                } else {
                    return Err(format!(
                        "Unexpected argument '{}'. body takes one symbol name.",
                        arg
                    ));
                }
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'body' command.", arg));
            }
        }
    }

    let symbol = symbol.ok_or_else(|| {
        "'body' command requires a symbol name. Usage: loct body <symbol>".to_string()
    })?;

    Ok(Command::Body(BodyOptions {
        symbol,
        line_cap,
        file,
        root,
    }))
}

/// Parse `loct impact <file> [options]` command - analyze impact of file changes.
pub(super) fn parse_impact_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct impact - Analyze impact of modifying/removing a file

USAGE:
    loct impact <FILE> [OPTIONS]

OPTIONS:
    --depth <N>          Limit traversal depth (default: unlimited)
    --root <PATH>        Project root (default: current directory)
    --help, -h           Show this help message

EXAMPLES:
    loct impact src/utils.ts
    loct impact src/api.ts --depth 2"
            .to_string());
    }

    let mut opts = ImpactCommandOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--depth" | "--max-depth" => {
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
            _ if !arg.starts_with('-') => {
                if opts.target.is_empty() {
                    opts.target = arg.clone();
                } else {
                    return Err(format!(
                        "Unexpected argument '{}'. impact takes one target path.",
                        arg
                    ));
                }
                i += 1;
            }
            _ => {
                return Err(format!("Unknown option '{}' for 'impact' command.", arg));
            }
        }
    }

    if opts.target.is_empty() {
        return Err(
            "'impact' command requires a target file path. Usage: loct impact <path>".to_string(),
        );
    }

    Ok(Command::Impact(opts))
}

/// Parse `loct twins [options]` command - find dead parrots and duplicate exports.
pub(super) fn parse_twins_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err(
            "loct twins - Find dead parrots (0 imports) and duplicate exports

USAGE:
    loct twins [OPTIONS] [PATH]

OPTIONS:
    --path <DIR>       Root directory to analyze (default: current directory)
    --limit <N>        Maximum findings across all output families
    --dead-only        Show only dead parrots (exports with 0 imports)
    --include-tests    Include test files in analysis (excluded by default)
    --help, -h         Show this help message

EXAMPLES:
    loct twins
    loct twins --dead-only"
                .to_string(),
        );
    }

    let mut opts = TwinsOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--limit" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--limit requires a number".to_string())?;
                opts.limit = Some(value.parse().map_err(|_| "--limit requires a number")?);
                i += 2;
            }
            "--path" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--path requires a directory".to_string())?;
                opts.path = Some(PathBuf::from(value));
                i += 2;
            }
            "--dead-only" => {
                opts.dead_only = true;
                i += 1;
            }
            "--include-suppressed" => {
                opts.include_suppressed = true;
                i += 1;
            }
            "--include-tests" => {
                opts.include_tests = true;
                i += 1;
            }
            "--ignore-conventions" => {
                opts.ignore_conventions = true;
                i += 1;
            }
            _ => {
                // Treat as path if no flag prefix
                if !arg.starts_with('-') {
                    opts.path = Some(PathBuf::from(arg));
                    i += 1;
                } else {
                    return Err(format!("Unknown option '{}' for 'twins' command.", arg));
                }
            }
        }
    }

    Ok(Command::Twins(opts))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dead_command() {
        let args = vec!["--confidence".into(), "high".into()];
        let result = parse_dead_command(&args).unwrap();
        if let Command::Dead(opts) = result {
            assert_eq!(opts.confidence, Some("high".into()));
            assert!(!opts.workspace_closed);
        } else {
            panic!("Expected Dead command");
        }
    }

    #[test]
    fn w1_02_parse_dead_workspace_closed() {
        let result = parse_dead_command(&["--workspace-closed".into()]).unwrap();
        if let Command::Dead(opts) = result {
            assert!(opts.workspace_closed);
        } else {
            panic!("Expected Dead command");
        }
    }

    #[test]
    fn test_parse_cycles_command() {
        let args = vec!["--breaking-only".into()];
        let result = parse_cycles_command(&args).unwrap();
        if let Command::Cycles(opts) = result {
            assert!(opts.breaking_only);
        } else {
            panic!("Expected Cycles command");
        }
    }

    #[test]
    fn test_parse_find_with_regex() {
        let args = vec![
            "--symbol".into(),
            ".*patient.*".into(),
            "--lang".into(),
            "ts".into(),
        ];
        let result = parse_find_command(&args).unwrap();
        if let Command::Find(opts) = result {
            assert_eq!(opts.symbol, Some(".*patient.*".into()));
            assert_eq!(opts.lang, Some("ts".into()));
        } else {
            panic!("Expected Find command");
        }
    }

    #[test]
    fn test_parse_find_regex_flag_and_pattern() {
        let args = vec!["--regex".into(), r"100\.[0-9]+".into()];
        let result = parse_find_command(&args).unwrap();
        if let Command::Find(opts) = result {
            assert!(opts.regex);
            assert!(!opts.literal);
            assert_eq!(opts.queries, vec![r"100\.[0-9]+".to_string()]);
        } else {
            panic!("Expected Find command");
        }
    }

    #[test]
    fn test_parse_find_literal_and_regex_are_mutually_exclusive() {
        let args = vec!["--literal".into(), "--regex".into(), "foo".into()];
        let err = parse_find_command(&args).unwrap_err();
        assert!(
            err.contains("mutually exclusive"),
            "expected mutual-exclusion error, got: {err}"
        );
    }

    // --mode <x> compat alias (loctree-fail.md CLI flag drift): agents type
    // `loct find --mode where-symbol` / `--mode literal` and used to hit
    // `Unknown option '--mode'`. The alias must dispatch 1:1 onto the mode flags.
    #[test]
    fn test_parse_find_mode_where_symbol_alias() {
        let args = vec!["--mode".into(), "where-symbol".into(), "Foo".into()];
        let result = parse_find_command(&args).unwrap();
        if let Command::Find(opts) = result {
            assert!(
                opts.where_symbol,
                "--mode where-symbol must set where_symbol"
            );
            assert_eq!(opts.queries, vec!["Foo".to_string()]);
        } else {
            panic!("Expected Find command");
        }
    }

    #[test]
    fn test_parse_find_mode_literal_alias() {
        let args = vec!["--mode".into(), "literal".into(), "utterance_id".into()];
        let result = parse_find_command(&args).unwrap();
        if let Command::Find(opts) = result {
            assert!(opts.literal, "--mode literal must set literal");
            assert_eq!(opts.queries, vec!["utterance_id".to_string()]);
        } else {
            panic!("Expected Find command");
        }
    }

    #[test]
    fn test_parse_find_mode_who_imports_alias_dispatches_to_query() {
        // --who-imports (also reachable via --mode who-imports) rewrites to a Query.
        let args = vec!["--mode".into(), "who-imports".into(), "src/utils.ts".into()];
        let result = parse_find_command(&args).unwrap();
        if let Command::Query(opts) = result {
            assert!(matches!(opts.kind, QueryKind::WhoImports));
            assert_eq!(opts.target, "src/utils.ts");
            assert!(opts.roots.is_empty(), "default roots empty => cwd");
        } else {
            panic!("Expected Query command from --mode who-imports");
        }
    }

    #[test]
    fn test_parse_find_who_imports_preserves_project_root() {
        // Skeptic: who-imports rewrite used to drop --project and re-home to cwd.
        let args = vec![
            "--project".into(),
            "/tmp/sibling-a".into(),
            "src/lib.rs".into(),
            "--who-imports".into(),
        ];
        let result = parse_find_command(&args).unwrap();
        if let Command::Query(opts) = result {
            assert!(matches!(opts.kind, QueryKind::WhoImports));
            assert_eq!(opts.target, "src/lib.rs");
            assert_eq!(opts.roots, vec![PathBuf::from("/tmp/sibling-a")]);
            assert_eq!(opts.scan_roots(), vec![PathBuf::from("/tmp/sibling-a")]);
        } else {
            panic!("Expected Query command");
        }
    }

    // loctree-fail 2026-07-27 / 2026-08-11: help advertised --root/--compact while
    // the parser rejected them. Keep help ↔ parser on one contract.
    #[test]
    fn test_parse_find_accepts_root_project_path_and_compact() {
        let args = vec![
            "--root".into(),
            "/tmp/proj".into(),
            "--literal".into(),
            "FOO".into(),
            "--compact".into(),
        ];
        let result = parse_find_command(&args).unwrap();
        if let Command::Find(opts) = result {
            assert_eq!(opts.roots, vec![PathBuf::from("/tmp/proj")]);
            assert!(opts.compact);
            assert!(opts.literal);
            assert_eq!(opts.queries, vec!["FOO".to_string()]);
        } else {
            panic!("Expected Find command");
        }

        let args = vec![
            "--project".into(),
            "/tmp/sibling".into(),
            "--path".into(),
            "src/".into(),
            "BAR".into(),
        ];
        let result = parse_find_command(&args).unwrap();
        if let Command::Find(opts) = result {
            assert_eq!(opts.roots, vec![PathBuf::from("/tmp/sibling")]);
            assert_eq!(opts.file.as_deref(), Some("src/"));
            assert_eq!(opts.queries, vec!["BAR".to_string()]);
        } else {
            panic!("Expected Find command");
        }
    }

    // loctree-fail 2026-08-12: `--project` was healed on find but not on
    // occurrences, so the same spelling worked on one command and errored on the
    // other. Occurrences is the literal oracle agents cross-check find against —
    // an agent that scoped find to a sibling project and then reached for
    // occurrences got a hard parse error mid-investigation.
    #[test]
    fn test_parse_occurrences_accepts_project_as_root_alias() {
        for flag in ["--root", "--project"] {
            let args = vec!["FOO".into(), flag.into(), "/tmp/sibling".into()];
            let result = parse_occurrences_command(&args)
                .unwrap_or_else(|e| panic!("{flag} rejected by occurrences: {e}"));
            if let Command::Occurrences(opts) = result {
                assert_eq!(opts.ident, "FOO");
                assert_eq!(opts.roots, vec![PathBuf::from("/tmp/sibling")]);
            } else {
                panic!("Expected Occurrences command for {flag}");
            }
        }
    }

    // `occurrences` is the surface agents cross-check `find` against. A flag
    // that parses on one and errors on the other reads as "the tools
    // disagree", so `--regex` must land on both with the same meaning.
    #[test]
    fn test_occurrences_regex_flag_parity_with_find() {
        let args = vec!["twin\\w+".into(), "--regex".into()];
        let occurrences = parse_occurrences_command(&args)
            .unwrap_or_else(|e| panic!("occurrences rejected --regex: {e}"));
        let Command::Occurrences(occ) = occurrences else {
            panic!("Expected Occurrences command");
        };
        assert!(occ.regex, "--regex must select pattern mode");
        assert_eq!(
            occ.ident, "twin\\w+",
            "the pattern is not consumed as a flag"
        );

        let find =
            parse_find_command(&args).unwrap_or_else(|e| panic!("find rejected --regex: {e}"));
        let Command::Find(find) = find else {
            panic!("Expected Find command");
        };
        assert!(find.regex, "precondition: find already carries --regex");
        assert_eq!(
            find.queries.first().map(String::as_str),
            Some(occ.ident.as_str()),
            "both surfaces must keep the same pattern text"
        );
    }

    // Flag order must not matter, and `--regex` must compose with paging the
    // same way it does on `find`.
    #[test]
    fn test_occurrences_regex_parity_composes_with_paging() {
        let args = vec![
            "--regex".into(),
            "twin\\w+".into(),
            "--limit".into(),
            "10".into(),
            "--offset".into(),
            "20".into(),
        ];
        let Command::Occurrences(opts) = parse_occurrences_command(&args)
            .unwrap_or_else(|e| panic!("occurrences rejected leading --regex: {e}"))
        else {
            panic!("Expected Occurrences command");
        };
        assert!(opts.regex);
        assert_eq!(opts.ident, "twin\\w+");
        assert_eq!(opts.limit, Some(10));
        assert_eq!(opts.offset, 20);
    }

    // A path-less `--project` must fail loudly rather than swallow the next
    // positional as its value — otherwise the identifier silently becomes the
    // scan root and the search returns a confident empty result.
    #[test]
    fn test_parse_occurrences_project_requires_a_path() {
        let args = vec!["FOO".into(), "--project".into()];
        let err = parse_occurrences_command(&args).expect_err("expected a missing-path error");
        assert!(
            err.contains("--project"),
            "error should name the flag: {err}"
        );
    }

    #[test]
    fn test_parse_find_mode_fuzzy_explicitly_selects_discovery() {
        let args = vec!["--mode".into(), "fuzzy".into(), "Patient".into()];
        let result = parse_find_command(&args).unwrap();
        if let Command::Find(opts) = result {
            assert!(!opts.literal && !opts.regex && !opts.where_symbol);
            assert!(opts.discover);
            assert_eq!(opts.queries, vec!["Patient".to_string()]);
        } else {
            panic!("Expected Find command");
        }
    }

    #[test]
    fn test_plain_find_is_literal_truth_by_default() {
        let args = vec!["utterance_id".into()];
        let result = parse_find_command(&args).unwrap();
        if let Command::Find(opts) = result {
            assert!(opts.literal);
            assert!(!opts.discover);
            assert_eq!(opts.limit, Some(50));
        } else {
            panic!("Expected Find command");
        }
    }

    #[test]
    fn test_discover_preserves_broad_find_engine() {
        let args = vec!["--discover".into(), "Patient".into()];
        let result = parse_find_command(&args).unwrap();
        if let Command::Find(opts) = result {
            assert!(opts.discover);
            assert!(!opts.literal);
        } else {
            panic!("Expected Find command");
        }
    }

    #[test]
    fn test_plain_multi_query_is_multi_literal() {
        let args = vec!["Props".into(), "Options".into()];
        let result = parse_find_command(&args).unwrap();
        if let Command::Find(opts) = result {
            assert!(opts.literal);
            assert!(!opts.discover);
            assert_eq!(
                opts.queries,
                vec!["Props".to_string(), "Options".to_string()]
            );
        } else {
            panic!("Expected Find command");
        }
    }

    #[test]
    fn test_plain_find_all_is_explicitly_unbounded() {
        let args = vec!["utterance_id".into(), "--all".into()];
        let result = parse_find_command(&args).unwrap();
        if let Command::Find(opts) = result {
            assert!(opts.literal);
            assert!(opts.all);
            assert_eq!(opts.limit, None);
        } else {
            panic!("Expected Find command");
        }
    }

    #[test]
    fn test_parse_find_mode_similar_needs_target() {
        let args = vec!["--mode".into(), "similar".into()];
        let err = parse_find_command(&args).unwrap_err();
        assert!(
            err.contains("--similar"),
            "expected redirect to --similar, got: {err}"
        );
    }

    #[test]
    fn test_parse_find_mode_unknown_lists_valid_modes() {
        let args = vec!["--mode".into(), "bogus".into()];
        let err = parse_find_command(&args).unwrap_err();
        assert!(
            err.contains("where-symbol"),
            "should list valid modes: {err}"
        );
        assert!(
            err.contains("query where-symbol"),
            "should point at `loct query where-symbol`: {err}"
        );
    }

    #[test]
    fn test_parse_find_mode_missing_value_errors() {
        let args = vec!["--mode".into()];
        let err = parse_find_command(&args).unwrap_err();
        assert!(err.contains("--mode requires a value"), "got: {err}");
    }

    // Friendly unknown-option error redirects the common docs-drift flags
    // (loctree-fail.md: `--format markdown`, `--group-by`, ...) instead of the
    // bare generic message.
    #[test]
    fn test_parse_find_unknown_format_flag_redirects_to_json() {
        let args = vec!["--format".into(), "markdown".into(), "Foo".into()];
        let err = parse_find_command(&args).unwrap_err();
        assert!(
            err.contains("--json"),
            "should redirect --format to --json: {err}"
        );
    }

    #[test]
    fn test_parse_find_unknown_option_lists_modes() {
        let args = vec!["--frobnicate".into()];
        let err = parse_find_command(&args).unwrap_err();
        assert!(
            err.contains("--where-symbol"),
            "generic error should list modes: {err}"
        );
        assert!(
            err.contains("loct find --help"),
            "should point at help: {err}"
        );
    }

    #[test]
    fn test_parse_query_who_imports() {
        let args = vec!["who-imports".into(), "src/utils.ts".into()];
        let result = parse_query_command(&args).unwrap();
        if let Command::Query(opts) = result {
            assert!(matches!(opts.kind, QueryKind::WhoImports));
            assert_eq!(opts.target, "src/utils.ts");
            assert_eq!(opts.limit, None);
            assert!(!opts.all);
            assert!(opts.roots.is_empty());
        } else {
            panic!("Expected Query command");
        }
    }

    #[test]
    fn test_parse_query_accepts_project_root() {
        let args = vec![
            "where-symbol".into(),
            "Foo".into(),
            "--project".into(),
            "/tmp/sibling-b".into(),
        ];
        let result = parse_query_command(&args).unwrap();
        if let Command::Query(opts) = result {
            assert!(matches!(opts.kind, QueryKind::WhereSymbol));
            assert_eq!(opts.roots, vec![PathBuf::from("/tmp/sibling-b")]);
        } else {
            panic!("Expected Query command");
        }
    }

    #[test]
    fn test_parse_query_limit_and_all_contract() {
        let limited = parse_query_command(&[
            "where-symbol".into(),
            "new".into(),
            "--limit".into(),
            "7".into(),
        ])
        .unwrap();
        if let Command::Query(opts) = limited {
            assert_eq!(opts.limit, Some(7));
            assert!(!opts.all);
        } else {
            panic!("Expected Query command");
        }

        let all =
            parse_query_command(&["where-symbol".into(), "new".into(), "--all".into()]).unwrap();
        if let Command::Query(opts) = all {
            assert!(opts.all);
            assert_eq!(opts.limit, None);
        } else {
            panic!("Expected Query command");
        }

        let err = parse_query_command(&[
            "where-symbol".into(),
            "new".into(),
            "--all".into(),
            "--limit".into(),
            "2".into(),
        ])
        .unwrap_err();
        assert!(err.contains("mutually exclusive"));
    }

    #[test]
    fn test_parse_query_swift_types() {
        let args = vec![
            "swift-types".into(),
            "Sources/App/AppController.swift".into(),
        ];
        let result = parse_query_command(&args).unwrap();
        if let Command::Query(opts) = result {
            assert!(matches!(opts.kind, QueryKind::SwiftTypes));
            assert_eq!(opts.target, "Sources/App/AppController.swift");
        } else {
            panic!("Expected Query command");
        }
    }

    #[test]
    fn test_parse_twins_command() {
        let args = vec!["--dead-only".into(), "--limit".into(), "4".into()];
        let result = parse_twins_command(&args).unwrap();
        if let Command::Twins(opts) = result {
            assert!(opts.dead_only);
            assert_eq!(opts.limit, Some(4));
        } else {
            panic!("Expected Twins command");
        }
    }

    // Contract: `loct twins` has no `--strict` flag. The `health` summary footer
    // must not advertise `loct twins --strict` (loctree-fail 2026-06-14). Pin the
    // runtime truth so the hint is never re-added pointing at a rejected option.
    #[test]
    fn test_parse_twins_rejects_strict_flag() {
        let args = vec!["--strict".into()];
        let result = parse_twins_command(&args);
        assert!(
            result.is_err(),
            "twins must reject --strict; health hint must not suggest it"
        );
        assert!(result.unwrap_err().contains("Unknown option '--strict'"));
    }

    #[test]
    fn test_parse_body_command_options() {
        let args = vec![
            "my_sym".into(),
            "--root".into(),
            "/some/path".into(),
            "--file".into(),
            "src/lib.rs".into(),
            "--max-lines".into(),
            "50".into(),
        ];
        let result = parse_body_command(&args).unwrap();
        if let Command::Body(opts) = result {
            assert_eq!(opts.symbol, "my_sym");
            assert_eq!(opts.root, Some(PathBuf::from("/some/path")));
            assert_eq!(opts.file, Some("src/lib.rs".into()));
            assert_eq!(opts.line_cap, Some(50));
        } else {
            panic!("Expected Body command");
        }

        let args_project = vec![
            "my_sym".into(),
            "--project".into(),
            "/other/path".into(),
        ];
        let result_project = parse_body_command(&args_project).unwrap();
        if let Command::Body(opts) = result_project {
            assert_eq!(opts.root, Some(PathBuf::from("/other/path")));
        } else {
            panic!("Expected Body command");
        }
    }
}
