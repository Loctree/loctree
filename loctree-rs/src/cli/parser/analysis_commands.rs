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
    --help, -h             Show this help message

EXAMPLES:
    loct dead                          # All dead exports
    loct dead --confidence high        # Only high-confidence
    loct dead --path src/components/   # Dead exports in components
    loct dead --top 50                 # Top 50 dead exports"
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

/// Parse `loct find [options]` command - semantic search for symbols.
pub(super) fn parse_find_command(args: &[String]) -> Result<Command, String> {
    // Check for help flag first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("loct find - Semantic search for symbols by name pattern

USAGE:
    loct find [QUERY...] [OPTIONS]

DESCRIPTION:
    Semantic search for symbols (functions, classes, types) matching name patterns.
    Uses regex patterns. Multi-arg QUERY defaults to split-mode (subqueries + cross-match).
    Use --or to combine multiple QUERY args into a single OR regex.
    Uses snapshot for instant results (15x faster than re-scanning).

DEFAULT vs --literal:
    Default 'find' is AST/fuzzy-aware: it matches symbols, parameters, and
    similar names — great for discovery, but it can miss local variables buried
    in a function body and it promotes fuzzy 'did you mean' candidates.
    '--literal' is the truth layer: it scans raw source bytes for exact
    identifier-boundary occurrences (same substrate as 'loct occurrences').
    Primary results are literal only; fuzzy suggestions, if any, are kept in a
    separate labeled section and never mixed in. 'Not found' means not found.

OPTIONS:
    --literal                           Literal exact-identifier scan (truth layer, no fuzzy primaries)
    --regex                             Regex over raw file TEXT (not just identifiers); keeps coverage
                                        accounting + context labels. For secret/privacy audits where
                                        --literal cannot evaluate a pattern. Mutually exclusive with --literal.
    --whole-token                       (literal) Treat '-' as token-internal: 'backdrop' no longer matches
                                        inside 'overlay-backdrop'/'--sample-z-overlay-backdrop' (opt-in, no default change)
    --group-by-file                     (literal) Add a per-file occurrence rollup ('by_file')
    --count-only, --slim                (literal) Suppress the full occurrence list, keep counters only
    --offset <N>                        (literal) Zero-based occurrence offset for paged output
    --or                                Combine multiple QUERY args with OR (legacy behavior)
    --symbol <PATTERN>, -s <PATTERN>    Search for symbols matching regex
    --pattern <PATTERN>                 Alias for --symbol (regex)
    --file <PATTERN>, -f <PATTERN>      Search for files matching regex; in --literal, exact path/suffix scope
    --similar <SYMBOL>                  Find symbols with similar names (fuzzy)
    --who-imports                       Find files that import QUERY (same graph path as `loct query who-imports`)
    --dead                              Only show dead/unused symbols
    --exported                          Only show exported symbols
    --lang <LANG>                       Filter by language (ts, rs, js, py, etc.)
    --limit <N>                         Maximum results to show; in --literal, page size for occurrences
    --help, -h                          Show this help message

EXAMPLES:
    loct find Patient                   # Find symbols containing \"Patient\"
    loct find Props Options ViewModel   # Split-mode: run subqueries + cross-match
    loct find \"Props Options\"          # AND-mode: require ALL terms (quoted)
    loct find --or foo bar baz          # Legacy: combine with OR
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
            "--file" | "-f" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--file requires a pattern".to_string())?;
                opts.file = Some(value.clone());
                i += 2;
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
            "--offset" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--offset requires a number".to_string())?;
                opts.offset = value.parse().map_err(|_| "--offset requires a number")?;
                i += 2;
            }
            "--" => {
                // Support `loct find --literal -- "--config-dir"` and similar for dash-prefixed
                // literals (REPEAT from loctree-feedback.md 2903,2978,2990,3052). The `--` separator
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
                    return Err(format!("Unknown option '{}' for 'find' command.", arg));
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
    Walks raw source bytes of snapshot files and reports every
    identifier-boundary occurrence of <IDENT>. Token-aware (not naive
    substring), literal only (no fuzzy suggestions promoted as primary).
    'Not found' means not found.

OPTIONS:
    --root <PATH>        Project root to scan (default: current directory)
    --whole-token        Treat '-' as token-internal: 'backdrop' no longer matches inside
                         'overlay-backdrop'/'--sample-z-overlay-backdrop' (opt-in, no default change)
    --group-by-file      Add a per-file occurrence rollup ('by_file')
    --count-only, --slim Suppress the full occurrence list, keep counters only ('slim')
    --limit <N>          Maximum number of occurrences to return in this page
    --offset <N>         Zero-based occurrence offset for paged output
    --json               Emit JSON (file, line, column, matched_text, context, source, occurrence_kind)
    --help, -h           Show this help message

EXAMPLES:
    loct occurrences utterance_id
    loct occurrences utterance_id --json
    loct occurrences backdrop --whole-token            # Exclude hyphenated z-index noise
    loct occurrences agent --limit 50 --offset 100 --json  # Page through large result sets
    loct occurrences backdrop --group-by-file --count-only --json  # Per-file counts, no list"
                .to_string(),
        );
    }

    let mut opts = OccurrencesOptions::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--root" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--root requires a path".to_string())?;
                opts.roots.push(PathBuf::from(value));
                i += 2;
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
                // Mirrors the find --literal -- <str> fix (loctree-feedback.md 2903 et al).
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
    loct query <KIND> <TARGET>

QUERY KINDS:
    who-imports <FILE>        Find all files that import the specified file
    where-symbol <SYMBOL>     Find where a symbol is defined/exported
    component-of <FILE>       Show which components/modules contain this file
    swift-types <SWIFT_FILE>  Classify Swift type-position references

EXAMPLES:
    loct query who-imports src/utils.ts
    loct query where-symbol PatientRecord
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

    Ok(Command::Query(QueryOptions { kind, target }))
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
    lines of its definition without ever shelling out to grep/sed.
    Body extraction is brace-balanced (Rust, TS/JS, C-family) with a
    fixed-window fallback for brace-less languages (e.g. Python).

OPTIONS:
    --max-lines <N>   Cap source lines returned per body (default: 200)
    --json            Emit JSON (file, start/end line, language, source)
    --help, -h        Show this help message

EXAMPLES:
    loct body transcription_session
    loct body handle_query_command --max-lines 80
    loct body query_where_symbol --json"
            .to_string());
    }

    let mut symbol: Option<String> = None;
    let mut line_cap: Option<usize> = None;
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

    Ok(Command::Body(BodyOptions { symbol, line_cap }))
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

    #[test]
    fn test_parse_query_who_imports() {
        let args = vec!["who-imports".into(), "src/utils.ts".into()];
        let result = parse_query_command(&args).unwrap();
        if let Command::Query(opts) = result {
            assert!(matches!(opts.kind, QueryKind::WhoImports));
            assert_eq!(opts.target, "src/utils.ts");
        } else {
            panic!("Expected Query command");
        }
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
        let args = vec!["--dead-only".into()];
        let result = parse_twins_command(&args).unwrap();
        if let Command::Twins(opts) = result {
            assert!(opts.dead_only);
        } else {
            panic!("Expected Twins command");
        }
    }

    // Contract: `loct twins` has no `--strict` flag. The `health` summary footer
    // must not advertise `loct twins --strict` (loctree-feedback 2026-06-14). Pin the
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
}
