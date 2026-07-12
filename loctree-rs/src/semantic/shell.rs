//! Layer 3 semantic analyzer for shell scripts (bash/sh/zsh/fish).
//!
//! Inputs:  FileAnalysis (Layer 1 sensor) + IdiomRegistry (T0)
//! Outputs: SemanticFacts (T0 contract)
//!
//! Five passes:
//!   1. Idiom classification (per-symbol, by name and alias)
//!   2. Dispatch graph extraction (`case ... esac` -> DispatchEdge)
//!   3. Source-include resolution (`. path.sh` / `source path.sh`)
//!   4. Env contract detection (uppercase `$VAR` references)
//!   5. Reachability propagation (idiom roles + dispatch handlers + source includes)
//!
//! Layer 1 sensor at `analyzer/shell.rs` is read-only from this module.
//!
//! Out-of-scope (deferred to later cuts):
//!   - Function-pointer dispatch beyond `var="handler"; "$var"` (detected, not resolved)
//!   - Eval-string dispatch resolution (markers only)
//!   - Tree-sitter migration (Cut 2.5+; current implementation is regex/state-machine)

use std::collections::BTreeSet;
use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::semantic::{
    Classifier, DispatchEdge, DispatchKind, EnvContract, IdiomRegistry, IdiomTag, ReachReason,
    RuntimeRole, RuntimeSemanticAnalyzer, SemanticFacts, SymbolId, TagSource,
};
use crate::types::{FileAnalysis, Language};

/// `FileAnalysis::language` is a string, not the typed enum.
/// Layer 1 emits "shell" for sh/bash/zsh/fish via `analyzer/classify.rs`.
const LANG_STR: &str = "shell";

pub struct ShellSemantics;

impl RuntimeSemanticAnalyzer for ShellSemantics {
    fn language(&self) -> Language {
        Language::Shell
    }

    fn analyze(
        &self,
        files: &[FileAnalysis],
        registry: &IdiomRegistry,
        out: &mut SemanticFacts,
    ) -> anyhow::Result<()> {
        for file in files {
            if file.language != LANG_STR {
                continue;
            }

            self.classify_idioms(file, registry, out);

            // For dispatch / source / env we need the raw file content. Files
            // that vanished between scan and analysis are skipped, not fatal.
            // Path goes through validate-and-canonicalize first so a malformed
            // sensor entry can never reach the filesystem read.
            let Some(content) = crate::semantic::io::try_read_validated_semantic_input(&file.path)?
            else {
                continue;
            };
            let stripped = strip_comments_and_heredocs(&content);

            self.extract_dispatch_graph(file, &stripped, out);
            self.resolve_source_includes(file, &stripped, files, out);
            self.detect_env_contracts(file, &stripped, registry, out);
        }

        self.compute_reachability(files, out);
        Ok(())
    }
}

impl ShellSemantics {
    fn classify_idioms(
        &self,
        file: &FileAnalysis,
        registry: &IdiomRegistry,
        out: &mut SemanticFacts,
    ) {
        for export in &file.exports {
            let Some(entry) = registry.lookup(Language::Shell, &export.name) else {
                continue;
            };
            let symbol_id: SymbolId = format!("{}::{}", file.path, export.name);
            out.idiom_tags.entry(symbol_id).or_default().push(IdiomTag {
                name: entry.name.clone(),
                classifier: entry.classifier.clone(),
                runtime_role: entry.runtime_role.clone(),
                source: TagSource::EmbeddedDefault,
                reasoning: entry.reasoning.clone(),
            });
        }
    }

    fn extract_dispatch_graph(
        &self,
        file: &FileAnalysis,
        stripped_content: &str,
        out: &mut SemanticFacts,
    ) {
        for edge in parse_case_dispatch(stripped_content, &file.path) {
            out.dispatch_edges.push(edge);
        }
    }

    fn resolve_source_includes(
        &self,
        file: &FileAnalysis,
        stripped_content: &str,
        all_files: &[FileAnalysis],
        out: &mut SemanticFacts,
    ) {
        for sourced_path in parse_source_directives(stripped_content) {
            for sourced_file in all_files {
                if sourced_file.path == file.path {
                    continue;
                }
                if !path_matches_source_directive(&sourced_path, &sourced_file.path, &file.path) {
                    continue;
                }
                for export in &sourced_file.exports {
                    let symbol_id: SymbolId = format!("{}::{}", sourced_file.path, export.name);
                    out.reachability.reached_symbols.insert(symbol_id.clone());
                    out.reachability.reasons.insert(
                        symbol_id,
                        ReachReason::SourceInclude {
                            from_file: file.path.clone(),
                        },
                    );
                }
            }
        }
    }

    fn detect_env_contracts(
        &self,
        file: &FileAnalysis,
        stripped_content: &str,
        registry: &IdiomRegistry,
        out: &mut SemanticFacts,
    ) {
        // Assignment-scope predicate (W2-c): a `$VAR` read is an env contract
        // ONLY when the file never assigns `VAR=` itself and VAR is not a
        // shell/CI-managed builtin. SHOUTING_CASE alone is a character class,
        // not semantics — `APP_NAME=x; echo $APP_NAME` is a script-local
        // variable, not an environment dependency.
        let assigned = collect_assigned_shell_vars(stripped_content);
        for env_name in extract_env_var_references(stripped_content) {
            if assigned.contains(&env_name) || is_shell_runtime_var(&env_name) {
                continue;
            }
            let is_known_env = registry
                .lookup(Language::Shell, &env_name)
                .map(|entry| matches!(entry.classifier, Classifier::EnvVar))
                .unwrap_or(false);

            // Always record contracts for SHOUTING_CASE references; the registry
            // simply documents conventional standard-env reasoning when present.
            // (Lower-case identifiers are filtered earlier in extract_env_var_references.)
            let _ = is_known_env;

            match out.env_contracts.iter_mut().find(|c| c.name == env_name) {
                Some(c) => {
                    if !c.used_in_files.contains(&file.path) {
                        c.used_in_files.push(file.path.clone());
                    }
                }
                None => {
                    out.env_contracts.push(EnvContract {
                        name: env_name,
                        used_in_files: vec![file.path.clone()],
                        required_for: Vec::new(),
                        occurrences: Vec::new(),
                    });
                }
            }
        }
    }

    fn compute_reachability(&self, files: &[FileAnalysis], out: &mut SemanticFacts) {
        // Mark dispatch_edge handler symbols as reached.
        let edges = out.dispatch_edges.clone();
        for edge in &edges {
            for f in files {
                if f.language != LANG_STR {
                    continue;
                }
                if !f.exports.iter().any(|e| e.name == edge.handler_symbol) {
                    continue;
                }
                let symbol_id = format!("{}::{}", f.path, edge.handler_symbol);
                out.reachability.reached_symbols.insert(symbol_id.clone());
                out.reachability.reasons.insert(
                    symbol_id,
                    ReachReason::DispatchHandler {
                        from_symbol: edge.from_file.clone(),
                        dispatch_kind: edge.dispatch_kind.clone(),
                    },
                );
            }
        }

        // Idiom-tagged symbols whose runtime role implies external invocation
        // are reached even without a static import edge.
        let tag_pairs: Vec<(SymbolId, RuntimeRole)> = out
            .idiom_tags
            .iter()
            .flat_map(|(id, tags)| {
                tags.iter()
                    .map(move |t| (id.clone(), t.runtime_role.clone()))
            })
            .collect();
        for (symbol_id, role) in tag_pairs {
            if matches!(
                role,
                RuntimeRole::UserFacing
                    | RuntimeRole::PrimaryEntrypoint
                    | RuntimeRole::PublicEntrypoint
                    | RuntimeRole::EnvInput
                    | RuntimeRole::LibraryHelper
            ) {
                out.reachability.reached_symbols.insert(symbol_id.clone());
                out.reachability
                    .reasons
                    .entry(symbol_id)
                    .or_insert(ReachReason::IdiomRuntimeRole(role));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pure helpers — testable without disk access.
// ---------------------------------------------------------------------------

static RE_CASE_HEAD: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*case\b").expect("valid case head regex"));

static RE_ESAC: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*esac\b").expect("valid esac regex"));

static RE_SOURCE_HEAD: Lazy<Regex> = Lazy::new(|| {
    // Match line prefix only (`source ` or `. `). The argument extraction is
    // handled in `extract_first_shell_arg` so we can deal with quoted paths
    // that contain spaces or shell substitutions like `$(dirname "$0")`.
    Regex::new(r"^\s*(?:source|\.)\s+(.+?)\s*$").expect("valid source head regex")
});

static RE_ENV_VAR: Lazy<Regex> = Lazy::new(|| {
    // Capture `$VAR`, `${VAR}`, `${VAR:-default}`. Uppercase + digits + underscore.
    // Lower-case and Mixed are excluded by character class — those are local vars.
    Regex::new(r"\$\{?([A-Z_][A-Z0-9_]*)\b").expect("valid env var regex")
});

static RE_BRANCH_HEAD: Lazy<Regex> = Lazy::new(|| {
    // A case branch line: optional pattern (no parens, no `;`), then `)`, then body.
    // The body may continue on the same line or with `;;` terminating.
    // Patterns may include shell wildcards (`*`, `?`, `[...]`) and `|` separators.
    Regex::new(r"^\s*([^()\s][^()]*?)\)\s*(.*)$").expect("valid branch head regex")
});

static RE_LEADING_IDENT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*([A-Za-z_][A-Za-z0-9_]*)").expect("valid ident regex"));

static RE_ASSIGNMENT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*[A-Za-z_][A-Za-z0-9_]*\s*=").expect("valid assignment regex"));

/// Builtins / keywords whose appearance as the first token of a case body
/// must NOT be treated as a dispatch handler (they are control flow, not jumps).
const SKIP_FIRST_TOKENS: &[&str] = &[
    "shift", "unset", "local", "export", "declare", "readonly", "set", "return", "exit", "break",
    "continue", "echo", "printf", "true", "false", ":",
];

fn parse_case_dispatch(stripped_content: &str, file_path: &str) -> Vec<DispatchEdge> {
    let mut edges = Vec::new();
    let lines: Vec<&str> = stripped_content.lines().collect();

    // Walk the file looking for `case ... in`. Each match opens a case block.
    // Inside, accumulate lines until matching `esac`, then parse branches.
    let mut i = 0;
    while i < lines.len() {
        if RE_CASE_HEAD.is_match(lines[i]) && lines[i].contains(" in") {
            let case_start = i;
            // Find matching esac. Nested `case ... esac` is rare in shell;
            // we depth-track minimally.
            let mut depth: usize = 1;
            let mut j = i + 1;
            while j < lines.len() {
                if RE_CASE_HEAD.is_match(lines[j]) && lines[j].contains(" in") {
                    depth += 1;
                } else if RE_ESAC.is_match(lines[j]) {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                j += 1;
            }
            // Process branches from case_start+1 .. j (exclusive)
            for (branch_idx, line) in lines.iter().enumerate().take(j).skip(case_start + 1) {
                if let Some(edge) = extract_branch_edge(line, branch_idx, file_path) {
                    edges.push(edge);
                }
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    edges
}

/// Parse a single `pattern) body ;;` line into one DispatchEdge if a handler
/// is identifiable, or None for assignments / compound bodies / control-flow-only.
/// Multi-pattern branches (`a|b|c) handler ;;`) emit ONE edge per branch.
fn extract_branch_edge(line: &str, line_idx: usize, file_path: &str) -> Option<DispatchEdge> {
    let caps = RE_BRANCH_HEAD.captures(line)?;
    let pattern_part = caps.get(1)?.as_str().trim();
    let body = caps.get(2)?.as_str().trim();

    // Reject lines that are NOT branches: e.g., function defs `foo() {`,
    // arithmetic `(( ... ))`, or generic lines that happened to contain `)`.
    // A real branch starts with a pattern token, not `(` or `{`.
    if pattern_part.starts_with('{') || pattern_part.starts_with('(') {
        return None;
    }
    if body.starts_with('{') || body.starts_with('(') {
        return None; // compound command body
    }
    // Reject function-definition signatures: `foo()` lands on this regex too.
    if body.is_empty() || body == "{" {
        return None;
    }

    // Walk command segments separated by `;`, `&&`, `||` to find the first
    // segment that names a callable handler.
    let segments = split_command_segments(body);
    for segment in segments {
        let segment = segment.trim();
        if segment.is_empty() || segment == ";;" {
            continue;
        }
        if RE_ASSIGNMENT.is_match(segment) {
            continue; // variable assignment, not a call
        }
        let Some(ident_caps) = RE_LEADING_IDENT.captures(segment) else {
            continue;
        };
        let candidate = ident_caps.get(1)?.as_str();
        if SKIP_FIRST_TOKENS.contains(&candidate) {
            continue;
        }
        return Some(DispatchEdge {
            from_file: file_path.to_string(),
            from_line: (line_idx + 1) as u32,
            dispatch_kind: DispatchKind::CaseStatement,
            handler_symbol: candidate.to_string(),
            handler_file: None,
        });
    }
    None
}

/// Split a shell command body on `;`, `&&`, `||`, and `;;` to surface
/// individual commands. Naive: does not respect quotes or subshells, but the
/// content has been comment- and heredoc-stripped, so noise is bounded.
fn split_command_segments(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let two = if i + 1 < bytes.len() {
            &bytes[i..=i + 1]
        } else {
            &bytes[i..=i]
        };
        if two == b";;" || two == b"&&" || two == b"||" {
            out.push(&body[start..i]);
            i += 2;
            start = i;
            continue;
        }
        if c == b';' {
            out.push(&body[start..i]);
            i += 1;
            start = i;
            continue;
        }
        i += 1;
    }
    if start < bytes.len() {
        out.push(&body[start..]);
    }
    out
}

fn parse_source_directives(stripped_content: &str) -> Vec<String> {
    let mut sources = Vec::new();
    for line in stripped_content.lines() {
        let Some(caps) = RE_SOURCE_HEAD.captures(line) else {
            continue;
        };
        let rest = caps.get(1).map(|m| m.as_str()).unwrap_or("").trim();
        let Some(arg) = extract_first_shell_arg(rest) else {
            continue;
        };
        if arg.is_empty() || arg == "." || arg == ".." {
            continue;
        }
        sources.push(arg);
    }
    sources
}

/// Extract the first whitespace-separated argument from a shell command tail,
/// honouring single- and double-quote boundaries. Quotes are kept inside the
/// returned string only when they are nested inside a `$(...)` expansion;
/// outermost matching quotes are stripped.
///
/// Examples:
///   `./lib.sh args`                         -> Some("./lib.sh")
///   `"path with space.sh"`                  -> Some("path with space.sh")
///   `"$(dirname \"$0\")/helpers.sh"`        -> Some("$(dirname \"$0\")/helpers.sh")
fn extract_first_shell_arg(rest: &str) -> Option<String> {
    // Iterate over `chars` (not raw bytes) so multi-byte UTF-8 paths like
    // `./żółć.sh` survive intact. The previous byte-loop pushed every input
    // byte as a Latin-1 `char`, which corrupted any non-ASCII codepoint.
    let mut chars = rest.chars().peekable();

    // Skip leading whitespace.
    while let Some(&c) = chars.peek() {
        if c == ' ' || c == '\t' {
            chars.next();
        } else {
            break;
        }
    }
    chars.peek()?;

    let mut buf = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut paren_depth: usize = 0;
    let mut prev_char: Option<char> = None;

    while let Some(c) = chars.next() {
        if !in_single && !in_double && paren_depth == 0 && (c == ' ' || c == '\t' || c == ';') {
            break;
        }

        match c {
            '\\' if !in_single => {
                buf.push(c);
                if let Some(next) = chars.next() {
                    buf.push(next);
                    prev_char = Some(next);
                } else {
                    prev_char = Some(c);
                }
                continue;
            }
            '\'' if !in_double && paren_depth == 0 => {
                in_single = !in_single;
                buf.push(c);
            }
            '"' if !in_single && paren_depth == 0 => {
                in_double = !in_double;
                buf.push(c);
            }
            '(' if !in_single && prev_char == Some('$') => {
                // `$(` opens a command substitution. Shell expands this even
                // inside double-quoted strings, so depth bumps regardless of
                // quote state. While paren_depth > 0 we ignore quote toggles
                // for whitespace-termination purposes.
                paren_depth += 1;
                buf.push(c);
            }
            '(' if paren_depth > 0 => {
                paren_depth += 1;
                buf.push(c);
            }
            ')' if paren_depth > 0 => {
                paren_depth -= 1;
                buf.push(c);
            }
            _ => buf.push(c),
        }
        prev_char = Some(c);
    }

    if buf.is_empty() {
        return None;
    }

    // Strip outermost matching quotes if both ends are the same quote char.
    // ASCII quote characters are 1 byte each, so byte-slicing 1..len-1 stays
    // on UTF-8 boundaries even if the contents include multi-byte chars.
    if buf.len() >= 2 {
        let first = buf.chars().next().unwrap();
        let last = buf.chars().last().unwrap();
        if first == last && (first == '"' || first == '\'') {
            buf = buf[1..buf.len() - 1].to_string();
        }
    }
    Some(buf)
}

fn extract_env_var_references(stripped_content: &str) -> Vec<String> {
    let mut envs: BTreeSet<String> = BTreeSet::new();
    for caps in RE_ENV_VAR.captures_iter(stripped_content) {
        if let Some(m) = caps.get(1) {
            envs.insert(m.as_str().to_string());
        }
    }
    envs.into_iter().collect()
}

static RE_SHELL_ASSIGN: Lazy<Regex> = Lazy::new(|| {
    // `VAR=`, `export VAR=`, `local VAR=`, `declare -r VAR=`, `readonly VAR=`,
    // `typeset VAR=` at command position (line start; comment/heredoc content
    // is stripped before this runs). Declaration keywords without `=`
    // (`local VAR`, `declare VAR`) are handled by RE_SHELL_DECL below.
    Regex::new(r"(?m)^\s*(?:export\s+|local\s+|readonly\s+|(?:declare|typeset)\s+(?:-[A-Za-z]+\s+)*)?([A-Z_][A-Z0-9_]*)=")
        .expect("valid shell assignment regex")
});

static RE_SHELL_DECL: Lazy<Regex> = Lazy::new(|| {
    // Value-less declarations: `local VAR`, `declare VAR`, `readonly VAR`,
    // `export VAR`, plus `read VAR` / `read -r VAR` which assigns from stdin.
    Regex::new(r"(?m)^\s*(?:local|declare|typeset|readonly|export|read)\s+(?:-[A-Za-z]+\s+)*([A-Z_][A-Z0-9_]*)\b")
        .expect("valid shell declaration regex")
});

static RE_FOR_VAR: Lazy<Regex> = Lazy::new(|| {
    // `for VAR in ...` loop variable.
    Regex::new(r"(?m)^\s*for\s+([A-Z_][A-Z0-9_]*)\s+in\b").expect("valid for-loop var regex")
});

/// Collect every SHOUTING_CASE variable the file assigns itself —
/// `VAR=`, `export VAR=`, `local`/`declare`/`readonly`/`typeset`, `read`,
/// and `for VAR in` loop heads. A read of an assigned variable is a
/// script-local data flow, not an environment contract.
pub(crate) fn collect_assigned_shell_vars(stripped_content: &str) -> BTreeSet<String> {
    let mut assigned: BTreeSet<String> = BTreeSet::new();
    for re in [&*RE_SHELL_ASSIGN, &*RE_SHELL_DECL, &*RE_FOR_VAR] {
        for caps in re.captures_iter(stripped_content) {
            if let Some(m) = caps.get(1) {
                assigned.insert(m.as_str().to_string());
            }
        }
    }
    assigned
}

/// Exact-name shell/CI builtins that don't match the prefix rules below.
const SHELL_BUILTIN_VARS: &[&str] = &[
    // bash-managed
    "BASH",
    "BASHOPTS",
    "BASHPID",
    "COMPREPLY",
    "DIRSTACK",
    "EUID",
    "FUNCNAME",
    "GROUPS",
    "HOSTNAME",
    "HOSTTYPE",
    "IFS",
    "LINENO",
    "MACHTYPE",
    "MAPFILE",
    "OLDPWD",
    "OPTARG",
    "OPTERR",
    "OPTIND",
    "OSTYPE",
    "PIPESTATUS",
    "PPID",
    "PS1",
    "PS2",
    "PS3",
    "PS4",
    "PWD",
    "RANDOM",
    "REPLY",
    "SECONDS",
    "SHELL",
    "SHELLOPTS",
    "SHLVL",
    "TMOUT",
    "UID",
    // zsh-managed
    "ZSH",
    "ZSH_NAME",
    "ZSH_VERSION",
    "ZDOTDIR",
    // CI runner toggle (GitHub/GitLab both export plain `CI`)
    "CI",
];

/// Is `name` a variable the shell or CI runner itself provides/maintains?
/// Those are never *environment contracts* of the repo: `BASH_REMATCH`,
/// `COMP_WORDS`, `GITHUB_ENV`, `RUNNER_OS`... exist regardless of any
/// declaration file, so reporting them as orphan env reads is pure noise.
///
/// NOTE: OS user-environment vars like `HOME` / `PATH` / `LANG` are NOT
/// listed here — they are genuine environment inputs and remain contracts;
/// env-truth separately suppresses them from orphan warnings.
pub(crate) fn is_shell_runtime_var(name: &str) -> bool {
    name.starts_with("BASH_")
        || name.starts_with("COMP_")
        || name.starts_with("ZSH_")
        || name.starts_with("GITHUB_")
        || name.starts_with("RUNNER_")
        || name.starts_with("ACTIONS_")
        || SHELL_BUILTIN_VARS.contains(&name)
}

/// Match a directive like `./lib.sh`, `lib.sh`, `$(dirname "$0")/lib.sh`
/// against a candidate file path. Strategy: substitute shell expressions out,
/// then prefer canonical relative resolution; fall back to basename equality.
fn path_matches_source_directive(directive: &str, candidate: &str, source_file: &str) -> bool {
    let cleaned = strip_shell_expressions(directive);
    if cleaned.is_empty() {
        return false;
    }

    // Direct equality (raw or normalized) first.
    if candidate == cleaned {
        return true;
    }

    // Try resolving cleaned as relative to source_file's directory.
    let source_dir = Path::new(source_file).parent().unwrap_or(Path::new(""));
    let resolved = source_dir.join(&cleaned);
    if resolved == Path::new(candidate) {
        return true;
    }
    if let (Ok(canonical_resolved), Ok(canonical_candidate)) =
        (resolved.canonicalize(), Path::new(candidate).canonicalize())
        && canonical_resolved == canonical_candidate
    {
        return true;
    }

    // Basename fallback. Useful for `$(dirname "$0")/lib.sh` style and
    // for unit tests that build FileAnalysis with synthetic paths.
    let directive_basename = Path::new(&cleaned)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let candidate_basename = Path::new(candidate)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if directive_basename.is_empty() || candidate_basename.is_empty() {
        return false;
    }
    directive_basename == candidate_basename
}

/// Replace `$(...)`, `${...}`, `$VAR` substrings with empty so the remainder
/// (typically a literal path tail) can be reasoned about. Best-effort, not a
/// full shell expression parser. Handles a single level of `$(...)` nesting,
/// which covers the synthetic-dispatch / dirname-style patterns seen in the
/// LOCTREE_NEXT.md false-positive corpus. Deeper nesting falls back to the
/// basename-match path in `path_matches_source_directive`.
fn strip_shell_expressions(input: &str) -> String {
    static RE_DOLLAR_PAREN: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\$\([^)]*\)").expect("valid dollar-paren regex"));
    static RE_DOLLAR_BRACE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\$\{[^}]*\}").expect("valid dollar-brace regex"));
    static RE_BARE_VAR: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"\$[A-Za-z_][A-Za-z0-9_]*|\$[0-9]+|\$[*@#?!$-]").expect("valid bare var regex")
    });

    let mut out = RE_DOLLAR_PAREN.replace_all(input, "").to_string();
    out = RE_DOLLAR_BRACE.replace_all(&out, "").to_string();
    out = RE_BARE_VAR.replace_all(&out, "").to_string();
    out.trim().trim_matches(['\'', '"']).to_string()
}

/// Strip `#`-comments (respecting `\#` escapes and quoted strings) and replace
/// heredoc bodies with blank lines (preserving line numbers).
///
/// Recognises:
///   - `<<EOF ... EOF`
///   - `<<-EOF ... EOF` (allow-indent form)
///   - `<<'EOF' ... EOF`, `<<"EOF" ... EOF`, `<<\EOF ... EOF` (no-expansion forms)
///
/// Heredocs that end at end-of-file (unterminated) blank to EOF.
fn strip_comments_and_heredocs(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_heredoc: Option<HeredocState> = None;

    for line in content.lines() {
        if let Some(state) = &in_heredoc {
            let candidate = if state.allow_indent {
                line.trim_start_matches(['\t'])
            } else {
                line
            };
            if candidate.trim_end() == state.marker {
                in_heredoc = None;
            }
            // Either way, blank this line to preserve line numbering.
            out.push('\n');
            continue;
        }

        // Look for heredoc starter. We do this BEFORE comment stripping so a
        // `<<EOF` that lives in `# <<EOF` (commented out) is correctly ignored.
        let pre_comment = strip_line_comment(line);
        if let Some((before_marker, marker, allow_indent)) = parse_heredoc_starter(&pre_comment) {
            out.push_str(&before_marker);
            out.push('\n');
            in_heredoc = Some(HeredocState {
                marker,
                allow_indent,
            });
            continue;
        }

        out.push_str(&pre_comment);
        out.push('\n');
    }
    out
}

struct HeredocState {
    marker: String,
    allow_indent: bool,
}

fn strip_line_comment(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;
    while let Some(c) = chars.next() {
        if escape {
            result.push(c);
            escape = false;
            continue;
        }
        match c {
            '\\' if !in_single => {
                result.push(c);
                escape = true;
            }
            '\'' if !in_double => {
                in_single = !in_single;
                result.push(c);
            }
            '"' if !in_single => {
                in_double = !in_double;
                result.push(c);
            }
            '#' if !in_single && !in_double => {
                // Treat `#` as comment-start only when at start-of-line or after
                // whitespace (so `foo#bar` stays untouched, matching shell behaviour).
                let last = result.chars().last();
                if last.is_none_or(|c| c.is_whitespace()) {
                    break;
                }
                result.push(c);
            }
            _ => result.push(c),
        }
        let _ = chars.peek();
    }
    result
}

/// Locate a heredoc starter on the line. Returns
/// `(content_before_starter, marker_string, allow_indent)`.
fn parse_heredoc_starter(line: &str) -> Option<(String, String, bool)> {
    static RE_HEREDOC: Lazy<Regex> = Lazy::new(|| {
        // <<- optional, leading dash means tab-indent stripping is allowed.
        // Marker can be: BARE, 'BARE', "BARE", \BARE.
        Regex::new(r#"<<(-?)\s*(?:'([^']+)'|"([^"]+)"|\\?([A-Za-z_][A-Za-z0-9_]*))"#)
            .expect("valid heredoc regex")
    });

    let caps = RE_HEREDOC.captures(line)?;
    let full_match = caps.get(0)?;
    let allow_indent = !caps.get(1)?.as_str().is_empty();
    let marker = caps
        .get(2)
        .or_else(|| caps.get(3))
        .or_else(|| caps.get(4))?
        .as_str()
        .to_string();
    let before = line[..full_match.start()].to_string();
    Some((before, marker, allow_indent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ExportSymbol;

    fn shell_file(path: &str, exports: &[(&str, &str)]) -> FileAnalysis {
        let mut fa = FileAnalysis::new(path.to_string());
        fa.language = "shell".to_string();
        for (name, kind) in exports {
            fa.exports.push(ExportSymbol::new(
                (*name).to_string(),
                kind,
                "named",
                Some(1),
            ));
        }
        fa
    }

    fn write_tmp(dir: &Path, name: &str, content: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, content).expect("write tmp file");
        path.to_string_lossy().into_owned()
    }

    // ---- shell-arg extraction ----

    #[test]
    fn extract_first_shell_arg_handles_utf8() {
        // Multi-byte codepoints (Polish accents, emoji, non-ASCII paths) must
        // survive intact. The previous byte-loop corrupted them by pushing
        // each input byte as a separate Latin-1 char.
        assert_eq!(
            extract_first_shell_arg("./żółć.sh args"),
            Some("./żółć.sh".to_string())
        );
        assert_eq!(
            extract_first_shell_arg("\"żółć.sh\" args"),
            Some("żółć.sh".to_string())
        );
        // ASCII path still works (regression).
        assert_eq!(
            extract_first_shell_arg("./lib.sh extra"),
            Some("./lib.sh".to_string())
        );
        // `$(...)` substitution is unchanged for ASCII inputs.
        assert_eq!(
            extract_first_shell_arg(r#""$(dirname "$0")/helpers.sh""#),
            Some(r#"$(dirname "$0")/helpers.sh"#.to_string())
        );
    }

    // ---- idiom classification ----

    #[test]
    fn idiom_classify_canonical_and_aliased_names() {
        let analyzer = ShellSemantics;
        let registry = IdiomRegistry::load_defaults().unwrap();
        let files = vec![shell_file(
            "/synthetic/install.sh",
            &[
                ("usage", "function"),
                ("die", "function"),
                ("main", "function"),
                ("_info", "function"),
                ("_warn", "function"),
                ("_error", "function"),
            ],
        )];
        let mut out = SemanticFacts::default();
        analyzer.analyze(&files, &registry, &mut out).unwrap();

        let names: Vec<&String> = out.idiom_tags.values().flatten().map(|t| &t.name).collect();
        assert!(names.iter().any(|n| n.as_str() == "usage"));
        assert!(names.iter().any(|n| n.as_str() == "die"));
        assert!(names.iter().any(|n| n.as_str() == "main"));
        // alias resolution
        assert!(
            names.iter().any(|n| n.as_str() == "info"),
            "_info should resolve to info"
        );
        assert!(
            names.iter().any(|n| n.as_str() == "warn"),
            "_warn should resolve to warn"
        );
        assert!(
            names.iter().any(|n| n.as_str() == "error"),
            "_error should resolve to error"
        );
        assert!(
            out.idiom_tags.len() >= 6,
            "expected at least 6 idiom-tagged symbols, got {}",
            out.idiom_tags.len()
        );
    }

    // ---- dispatch graph ----

    #[test]
    fn dispatch_graph_basic_case() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "dispatch.sh",
            r#"#!/usr/bin/env bash
main() {
    case "$1" in
        deploy)   deploy_impl ;;
        rollback) rollback_impl ;;
        *)        usage ;;
    esac
}
"#,
        );
        let mut fa = shell_file(
            &path,
            &[
                ("main", "function"),
                ("deploy_impl", "function"),
                ("rollback_impl", "function"),
                ("usage", "function"),
            ],
        );
        fa.path = path.clone();
        let registry = IdiomRegistry::load_defaults().unwrap();
        let mut out = SemanticFacts::default();
        ShellSemantics.analyze(&[fa], &registry, &mut out).unwrap();

        assert_eq!(
            out.dispatch_edges.len(),
            3,
            "expected 3 edges, got {:?}",
            out.dispatch_edges
        );
        let handlers: Vec<&str> = out
            .dispatch_edges
            .iter()
            .map(|e| e.handler_symbol.as_str())
            .collect();
        assert!(handlers.contains(&"deploy_impl"));
        assert!(handlers.contains(&"rollback_impl"));
        assert!(handlers.contains(&"usage"));
    }

    #[test]
    fn dispatch_graph_multi_pattern_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "multi.sh",
            r#"#!/usr/bin/env bash
case "$x" in
    start|begin|init) start_impl ;;
esac
"#,
        );
        let fa = shell_file(&path, &[("start_impl", "function")]);
        let mut out = SemanticFacts::default();
        ShellSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        assert_eq!(
            out.dispatch_edges.len(),
            1,
            "multi-pattern branch must emit ONE edge; got {:?}",
            out.dispatch_edges
        );
        assert_eq!(out.dispatch_edges[0].handler_symbol, "start_impl");
    }

    #[test]
    fn dispatch_graph_handles_shift_prefix() {
        // shift; deploy_impl "$@" — handler is deploy_impl, not shift.
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "shift.sh",
            r#"#!/usr/bin/env bash
case "$1" in
    deploy) shift; deploy_impl "$@" ;;
esac
"#,
        );
        let fa = shell_file(&path, &[("deploy_impl", "function")]);
        let mut out = SemanticFacts::default();
        ShellSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        assert_eq!(out.dispatch_edges.len(), 1);
        assert_eq!(out.dispatch_edges[0].handler_symbol, "deploy_impl");
    }

    #[test]
    fn dispatch_graph_skips_assignments() {
        // case "$kind" in prod) handler="deploy_impl" ;; esac — variable assignment,
        // NOT a dispatch (this is function-pointer setup, T1 out of scope).
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "assign.sh",
            r#"#!/usr/bin/env bash
case "$kind" in
    prod)  handler="deploy_impl" ;;
    stage) handler="rollback_impl" ;;
esac
"#,
        );
        let fa = shell_file(&path, &[]);
        let mut out = SemanticFacts::default();
        ShellSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        assert!(
            out.dispatch_edges.is_empty(),
            "assignments must not produce dispatch edges; got {:?}",
            out.dispatch_edges
        );
    }

    #[test]
    fn dispatch_does_not_match_inside_heredoc() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "heredoc.sh",
            r#"#!/usr/bin/env bash
cat <<EOF
case "$x" in
  foo) bar ;;
esac
EOF
"#,
        );
        let fa = shell_file(&path, &[]);
        let mut out = SemanticFacts::default();
        ShellSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        assert!(
            out.dispatch_edges.is_empty(),
            "heredoc body must not produce dispatch edges; got {:?}",
            out.dispatch_edges
        );
    }

    #[test]
    fn dispatch_does_not_match_inside_comments() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "comment.sh",
            r#"#!/usr/bin/env bash
# case "$x" in foo) bar ;; esac
"#,
        );
        let fa = shell_file(&path, &[]);
        let mut out = SemanticFacts::default();
        ShellSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        assert!(out.dispatch_edges.is_empty());
    }

    // ---- source includes ----

    #[test]
    fn source_include_relative_path_marks_reached() {
        let tmp = tempfile::tempdir().unwrap();
        let lib_path = write_tmp(
            tmp.path(),
            "lib.sh",
            "lib_func() { echo hi; }\n_check_health() { return 0; }\n",
        );
        let main_path = write_tmp(
            tmp.path(),
            "a.sh",
            r#"#!/usr/bin/env bash
. ./lib.sh

deploy_impl() {
    _check_health || exit 1
}
"#,
        );
        let lib = shell_file(
            &lib_path,
            &[("lib_func", "function"), ("_check_health", "function")],
        );
        let main = shell_file(&main_path, &[("deploy_impl", "function")]);

        let mut out = SemanticFacts::default();
        ShellSemantics
            .analyze(
                &[lib, main],
                &IdiomRegistry::load_defaults().unwrap(),
                &mut out,
            )
            .unwrap();

        let lib_func_id = format!("{}::lib_func", lib_path);
        let check_health_id = format!("{}::_check_health", lib_path);
        assert!(
            out.reachability.reached_symbols.contains(&lib_func_id),
            "lib_func should be reached via SourceInclude; reached={:?}",
            out.reachability.reached_symbols
        );
        assert!(out.reachability.reached_symbols.contains(&check_health_id));
        match out.reachability.reasons.get(&lib_func_id) {
            Some(ReachReason::SourceInclude { from_file }) => {
                assert_eq!(from_file, &main_path);
            }
            other => panic!("expected SourceInclude reason, got {other:?}"),
        }
    }

    #[test]
    fn source_include_handles_dollar_dirname_pattern() {
        // $(dirname "$0")/helpers.sh — outer quotes stripped, basename match
        // resolves to the candidate file. Use a NON-idiom symbol so that the
        // only path to reachability is SourceInclude, not idiom role.
        let tmp = tempfile::tempdir().unwrap();
        let helpers = write_tmp(tmp.path(), "helpers.sh", "_check_health() { return 0; }\n");
        let main = write_tmp(
            tmp.path(),
            "main.sh",
            "#!/usr/bin/env bash\n. \"$(dirname \"$0\")/helpers.sh\"\n",
        );
        let helpers_fa = shell_file(&helpers, &[("_check_health", "function")]);
        let main_fa = shell_file(&main, &[]);
        let mut out = SemanticFacts::default();
        ShellSemantics
            .analyze(
                &[helpers_fa, main_fa],
                &IdiomRegistry::load_defaults().unwrap(),
                &mut out,
            )
            .unwrap();
        let id = format!("{}::_check_health", helpers);
        assert!(
            out.reachability.reached_symbols.contains(&id),
            "_check_health via $(dirname)/helpers.sh should be reached; reached={:?}",
            out.reachability.reached_symbols
        );
        match out.reachability.reasons.get(&id) {
            Some(ReachReason::SourceInclude { from_file }) => {
                assert_eq!(from_file, &main);
            }
            other => panic!("expected SourceInclude reason, got {other:?}"),
        }
    }

    // ---- env contracts ----

    #[test]
    fn env_contract_detects_path_home() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "env.sh",
            r#"#!/usr/bin/env bash
echo "$PATH:$HOME/bin"
echo "${LANG:-en_US.UTF-8}"
"#,
        );
        let fa = shell_file(&path, &[]);
        let mut out = SemanticFacts::default();
        ShellSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        let names: Vec<&str> = out.env_contracts.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"PATH"));
        assert!(names.contains(&"HOME"));
        assert!(names.contains(&"LANG"));
        let path_contract = out.env_contracts.iter().find(|c| c.name == "PATH").unwrap();
        assert_eq!(path_contract.used_in_files, vec![path]);
    }

    #[test]
    fn env_contract_skips_locally_assigned_uppercase_vars() {
        // W2-c regression (CodeScribe 126 / suite 152 false orphans):
        // `APP_NAME=x` + `echo $APP_NAME` is a script-local variable, not an
        // env contract. Same for ANSI color locals and loop variables.
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "assigned.sh",
            r#"#!/usr/bin/env bash
APP_NAME="codescribe"
GREEN='\033[0;32m'
NC='\033[0m'
export RELEASE_CHANNEL="stable"
local SCOPED_VAR="x"
declare -r FROZEN_VAR="y"
readonly LOCKED_VAR="z"
for TARGET in linux darwin; do
    echo "build $TARGET"
done
echo -e "${GREEN}${APP_NAME}${NC} $RELEASE_CHANNEL $SCOPED_VAR $FROZEN_VAR $LOCKED_VAR"
echo "real env: $DEPLOY_TOKEN"
"#,
        );
        let fa = shell_file(&path, &[]);
        let mut out = SemanticFacts::default();
        ShellSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        let names: Vec<&str> = out.env_contracts.iter().map(|c| c.name.as_str()).collect();
        for local in [
            "APP_NAME",
            "GREEN",
            "NC",
            "RELEASE_CHANNEL",
            "SCOPED_VAR",
            "FROZEN_VAR",
            "LOCKED_VAR",
            "TARGET",
        ] {
            assert!(
                !names.contains(&local),
                "locally-assigned `{local}` must not become an env contract; got {names:?}"
            );
        }
        // The genuinely-unassigned read survives as a real contract.
        assert!(
            names.contains(&"DEPLOY_TOKEN"),
            "unassigned $DEPLOY_TOKEN must remain an env contract; got {names:?}"
        );
    }

    #[test]
    fn env_contract_skips_shell_and_ci_builtins() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "builtins.sh",
            r#"#!/usr/bin/env bash
if [[ "$1" =~ ^v([0-9]+) ]]; then
    echo "major ${BASH_REMATCH[1]}"
fi
echo "completing ${COMP_WORDS[0]}"
echo "ref=$1" >> "$GITHUB_ENV"
echo "runner: $RANDOM on $OSTYPE"
echo "still a contract: $HOME and $MY_SERVICE_URL"
"#,
        );
        let fa = shell_file(&path, &[]);
        let mut out = SemanticFacts::default();
        ShellSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        let names: Vec<&str> = out.env_contracts.iter().map(|c| c.name.as_str()).collect();
        for builtin in [
            "BASH_REMATCH",
            "COMP_WORDS",
            "GITHUB_ENV",
            "RANDOM",
            "OSTYPE",
        ] {
            assert!(
                !names.contains(&builtin),
                "shell/CI builtin `{builtin}` must not become an env contract; got {names:?}"
            );
        }
        // OS user-environment vars and project vars remain contracts.
        assert!(names.contains(&"HOME"));
        assert!(names.contains(&"MY_SERVICE_URL"));
    }

    #[test]
    fn collect_assigned_shell_vars_covers_all_forms() {
        let assigned = collect_assigned_shell_vars(
            "FOO=1\nexport BAR=2\nlocal BAZ=3\ndeclare -ri QUX=4\nreadonly QUUX\nfor IDX in 1 2; do :; done\nread -r LINE_VAR\n",
        );
        for v in ["FOO", "BAR", "BAZ", "QUX", "QUUX", "IDX", "LINE_VAR"] {
            assert!(assigned.contains(v), "missing {v} in {assigned:?}");
        }
        // A read-only reference is not an assignment.
        assert!(!collect_assigned_shell_vars("echo $ONLY_READ\n").contains("ONLY_READ"));
    }

    #[test]
    fn env_contract_skips_lowercase_locals() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "local.sh",
            r#"#!/usr/bin/env bash
local x="value"
echo "$x"
"#,
        );
        let fa = shell_file(&path, &[]);
        let mut out = SemanticFacts::default();
        ShellSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        assert!(
            !out.env_contracts.iter().any(|c| c.name == "x"),
            "lowercase $x must not become an env contract"
        );
    }

    // ---- robustness / cross-language ----

    #[test]
    fn unrelated_language_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(tmp.path(), "x.py", "def usage(): pass\n");
        let mut fa = shell_file(&path, &[("usage", "function")]);
        fa.language = "python".to_string();
        let mut out = SemanticFacts::default();
        ShellSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        assert!(out.idiom_tags.is_empty());
        assert!(out.dispatch_edges.is_empty());
        assert!(out.env_contracts.is_empty());
        assert!(out.reachability.reached_symbols.is_empty());
    }

    #[test]
    fn missing_file_does_not_panic() {
        // FileAnalysis points at a non-existent path; analysis should silently skip
        // dispatch/env passes for that file (idiom classification still runs from
        // the in-memory exports list).
        let fa = shell_file("/nonexistent/missing.sh", &[("usage", "function")]);
        let mut out = SemanticFacts::default();
        ShellSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        // Idiom tag still emitted from in-memory exports.
        assert_eq!(out.idiom_tags.len(), 1);
        // No dispatch / env content because file read failed.
        assert!(out.dispatch_edges.is_empty());
        assert!(out.env_contracts.is_empty());
    }

    // ---- reachability propagation ----

    #[test]
    fn reachability_marks_dispatch_handler_and_idiom_roles() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "reach.sh",
            r#"#!/usr/bin/env bash
usage() { echo "help"; }
deploy_impl() { echo "deploying"; }
main() {
    case "$1" in
        deploy) deploy_impl ;;
        *) usage ;;
    esac
}
main "$@"
"#,
        );
        let fa = shell_file(
            &path,
            &[
                ("usage", "function"),
                ("deploy_impl", "function"),
                ("main", "function"),
            ],
        );
        let mut out = SemanticFacts::default();
        ShellSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();

        let usage_id = format!("{}::usage", path);
        let deploy_id = format!("{}::deploy_impl", path);
        let main_id = format!("{}::main", path);

        assert!(
            out.reachability.reached_symbols.contains(&main_id),
            "main should be reached via PrimaryEntrypoint idiom"
        );
        assert!(
            out.reachability.reached_symbols.contains(&usage_id),
            "usage should be reached (idiom + dispatch)"
        );
        assert!(
            out.reachability.reached_symbols.contains(&deploy_id),
            "deploy_impl should be reached via dispatch"
        );
    }

    // ---- helper-level pure tests (no disk) ----

    #[test]
    fn strip_comments_inline_and_full_line() {
        let stripped = strip_comments_and_heredocs("foo bar # trailing\n# whole\nbaz");
        assert!(stripped.contains("foo bar"));
        assert!(!stripped.contains("trailing"));
        assert!(!stripped.contains("whole"));
        assert!(stripped.contains("baz"));
    }

    #[test]
    fn strip_keeps_hash_inside_quotes() {
        let stripped = strip_comments_and_heredocs(r#"echo "hello # world""#);
        assert!(stripped.contains("hello # world"));
    }

    #[test]
    fn strip_heredoc_blanks_body_preserves_lines() {
        let original = "before\ncat <<EOF\ncase x in foo) bar ;; esac\nEOF\nafter\n";
        let stripped = strip_comments_and_heredocs(original);
        let line_count = stripped.lines().count();
        assert_eq!(
            line_count,
            original.lines().count(),
            "line numbering must be preserved"
        );
        // Body lines blanked.
        assert!(stripped.contains("before"));
        assert!(stripped.contains("after"));
        assert!(!stripped.contains("foo) bar"));
    }

    #[test]
    fn extract_env_var_uppercase_only() {
        let envs = extract_env_var_references("$PATH and $home and ${LC_ALL:-en} and $1");
        assert!(envs.contains(&"PATH".to_string()));
        assert!(envs.contains(&"LC_ALL".to_string()));
        assert!(!envs.iter().any(|s| s == "home"));
        assert!(!envs.iter().any(|s| s == "1"));
    }

    #[test]
    fn parse_source_directives_basic() {
        let sources = parse_source_directives(
            "source ./lib.sh\n. helpers.sh\n. \"$(dirname \"$0\")/x.sh\"\n",
        );
        assert!(sources.iter().any(|s| s == "./lib.sh"));
        assert!(sources.iter().any(|s| s == "helpers.sh"));
        // Outer quotes stripped, inner $(...) (with its own nested quotes) preserved
        // so basename matching can still recover the trailing literal path segment.
        assert!(
            sources.iter().any(|s| s.ends_with("/x.sh")),
            "expected dollar-dirname source to retain trailing /x.sh; got {sources:?}"
        );
    }

    #[test]
    fn split_command_segments_basic() {
        let segs = split_command_segments("shift; deploy_impl \"$@\" ;;");
        // segments: "shift", " deploy_impl \"$@\" ", ""
        assert!(segs.iter().any(|s| s.trim() == "shift"));
        assert!(segs.iter().any(|s| s.trim().starts_with("deploy_impl")));
    }
}
