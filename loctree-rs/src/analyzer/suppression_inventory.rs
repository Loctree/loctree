//! Source-side silencer inventory — literal-only cross-language detector.
//!
//! Surfaces every `#[allow(...)]`, `#[ignore]`, `unsafe { ... }`, `// nosemgrep`,
//! `@ts-ignore`, `eslint-disable`, `# noqa`, `# type: ignore`, `# pylint: disable`,
//! `# mypy:`, and `# shellcheck disable` directive in the repo as a structured
//! record (kind / file / line / snippet / rule_id).
//!
//! # Tier boundary (LITERAL-ONLY, free-tier scope)
//!
//! This module ships in the **free tier** of Loctree. It performs **regex /
//! literal string matching only**. There is NO embedding-based similarity,
//! NO LLM classification, NO "this suppression looks suspicious because
//! semantically similar ones in other files were fixed" scoring. Every match
//! is reported verbatim with the exact line snippet that triggered it.
//!
//! Semantic enrichment (suspicious / stale / similar-to-fixed) lives in the
//! **paid tier** (Wave 7+, post-aicx-library integration). Any future change
//! to this module that crosses that boundary MUST be gated behind a clean
//! `feature = "semantic"` flag (or runtime tier check) and MUST NOT degrade
//! the free-tier path.
//!
//! Why this matters: the silencer surface is one of the highest-leverage
//! "forgotten gems" detectors in any repo (parked `#[allow(dead_code)]`
//! frequently flags real work the team forgot about). Keeping it free-tier
//! means every operator can run `loct suppressions --summary` on day one
//! without paying for semantic infrastructure. Semantic add-ons later are
//! pure delta value, never the gate.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

/// Kind of silencer detected at a source-side call site.
///
/// These are the source-side annotations engineers leave in code to mute
/// linters/checkers — NOT to be confused with `crate::suppressions::SuppressionType`
/// which models loctree's own finding-suppression file (`.loctree/suppressions.toml`).
/// Different concepts, similar word — the name collision is documented in
/// `~/internal-artifacts/loctree/loctree-feedback.md` (2026-05-17 entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SilencerKind {
    /// Rust `#[allow(...)]` (or `#[expect(...)]`) — anything except the
    /// `dead_code` lint, which gets its own bucket because it's the most
    /// commonly forgotten-gem signal.
    Allow,
    /// Rust `#[allow(dead_code)]` / `#[expect(dead_code)]` — parked work the
    /// team likely forgot about. Highest-leverage triage target.
    DeadCode,
    /// `// nosemgrep` (or `# nosemgrep`) comment.
    Nosemgrep,
    /// `// @ts-ignore` (suppresses TypeScript error on the next line).
    TsIgnore,
    /// `// @ts-expect-error` (acceptable; tracked).
    TsExpectError,
    /// `// @ts-nocheck` (disables type checking for the whole file).
    TsNocheck,
    /// `// eslint-disable` / `eslint-disable-line` / `eslint-disable-next-line`.
    EslintDisable,
    /// Python `# noqa` (flake8-family suppression).
    Noqa,
    /// Python `# type: ignore` (mypy-family suppression).
    TypeIgnore,
    /// Python `# pylint: disable=...`.
    PylintDisable,
    /// Python `# mypy: ...` (file-level mypy directive).
    MypyIgnore,
    /// Shell `# shellcheck disable=SCxxxx`.
    Shellcheck,
    /// Rust `unsafe { ... }` block with non-env-var body. Real unsafe.
    Unsafe,
    /// Rust 2024 `unsafe { std::env::set_var(...) / std::env::remove_var(...) }`
    /// boilerplate — semantically not unsafe, but the language requires the
    /// keyword now that the calls are marked `unsafe fn`. Triaged separately
    /// from real unsafe so audits aren't drowned in noise.
    UnsafeEnvVar,
    /// Rust `#[ignore]` test attribute (skipped test).
    Ignore,
}

impl SilencerKind {
    /// Human-friendly label used in `--summary` output.
    pub fn label(self) -> &'static str {
        match self {
            SilencerKind::Allow => "allow",
            SilencerKind::DeadCode => "dead-code",
            SilencerKind::Nosemgrep => "nosemgrep",
            SilencerKind::TsIgnore => "ts-ignore",
            SilencerKind::TsExpectError => "ts-expect-error",
            SilencerKind::TsNocheck => "ts-nocheck",
            SilencerKind::EslintDisable => "eslint-disable",
            SilencerKind::Noqa => "noqa",
            SilencerKind::TypeIgnore => "type-ignore",
            SilencerKind::PylintDisable => "pylint-disable",
            SilencerKind::MypyIgnore => "mypy-ignore",
            SilencerKind::Shellcheck => "shellcheck",
            SilencerKind::Unsafe => "unsafe",
            SilencerKind::UnsafeEnvVar => "unsafe-env-var",
            SilencerKind::Ignore => "ignore",
        }
    }

    /// All kinds, in canonical reporting order (most-common first).
    pub fn all() -> &'static [SilencerKind] {
        &[
            SilencerKind::Nosemgrep,
            SilencerKind::DeadCode,
            SilencerKind::Allow,
            SilencerKind::Ignore,
            SilencerKind::Unsafe,
            SilencerKind::UnsafeEnvVar,
            SilencerKind::TsIgnore,
            SilencerKind::TsExpectError,
            SilencerKind::TsNocheck,
            SilencerKind::EslintDisable,
            SilencerKind::Noqa,
            SilencerKind::TypeIgnore,
            SilencerKind::PylintDisable,
            SilencerKind::MypyIgnore,
            SilencerKind::Shellcheck,
        ]
    }

    /// Parse a kind from a CLI/MCP filter token. Accepts both kebab and
    /// snake-case forms and a few common aliases.
    pub fn from_filter(token: &str) -> Option<Self> {
        match token.trim().to_lowercase().as_str() {
            "allow" => Some(SilencerKind::Allow),
            "dead-code" | "dead_code" | "deadcode" => Some(SilencerKind::DeadCode),
            "nosemgrep" => Some(SilencerKind::Nosemgrep),
            "ts-ignore" | "tsignore" => Some(SilencerKind::TsIgnore),
            "ts-expect-error" | "tsexpecterror" | "ts-expect" => Some(SilencerKind::TsExpectError),
            "ts-nocheck" | "tsnocheck" => Some(SilencerKind::TsNocheck),
            "eslint-disable" | "eslint" | "eslintdisable" => Some(SilencerKind::EslintDisable),
            "noqa" => Some(SilencerKind::Noqa),
            "type-ignore" | "type_ignore" | "typeignore" => Some(SilencerKind::TypeIgnore),
            "pylint-disable" | "pylint" | "pylintdisable" => Some(SilencerKind::PylintDisable),
            "mypy-ignore" | "mypy" | "mypyignore" => Some(SilencerKind::MypyIgnore),
            "shellcheck" | "shell" => Some(SilencerKind::Shellcheck),
            "unsafe" => Some(SilencerKind::Unsafe),
            "unsafe-env-var" | "unsafe_env_var" | "unsafe-env" | "envvar" => {
                Some(SilencerKind::UnsafeEnvVar)
            }
            "ignore" => Some(SilencerKind::Ignore),
            _ => None,
        }
    }
}

/// A single silencer match — one literal occurrence at a specific file/line.
///
/// `rule_id` carries lint specifics where available (e.g. `dead_code`, `SC2086`,
/// `react-hooks/exhaustive-deps`, `unused_imports`). For directives without a
/// rule body (`#[ignore]`, bare `// nosemgrep`) this is `None`.
///
/// Literal-only — no semantic enrichment. See module-level docs for the tier
/// boundary.
#[derive(Debug, Clone, Serialize)]
pub struct SilencerMatch {
    pub kind: SilencerKind,
    pub file: String,
    pub line: usize,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
}

/// Per-kind aggregation used by `--summary` and the MCP tool response.
#[derive(Debug, Clone, Serialize)]
pub struct SilencerInventory {
    /// All matches in scan order, after filter/ignore application.
    pub matches: Vec<SilencerMatch>,
    /// Count per kind (kinds with zero matches are omitted).
    pub counts: BTreeMap<String, usize>,
    /// Unique files per kind (kinds with zero matches are omitted).
    pub files_per_kind: BTreeMap<String, usize>,
    /// Total match count after filter/ignore application.
    pub total: usize,
    /// Total unique files touched by any matched silencer.
    pub total_files: usize,
}

impl SilencerInventory {
    fn from_matches(matches: Vec<SilencerMatch>) -> Self {
        let mut counts: HashMap<SilencerKind, usize> = HashMap::new();
        let mut files: HashMap<SilencerKind, HashSet<String>> = HashMap::new();
        let mut all_files: HashSet<String> = HashSet::new();
        for m in &matches {
            *counts.entry(m.kind).or_insert(0) += 1;
            files.entry(m.kind).or_default().insert(m.file.clone());
            all_files.insert(m.file.clone());
        }
        let counts_out: BTreeMap<String, usize> = counts
            .into_iter()
            .map(|(k, v)| (k.label().to_string(), v))
            .collect();
        let files_out: BTreeMap<String, usize> = files
            .into_iter()
            .map(|(k, v)| (k.label().to_string(), v.len()))
            .collect();
        let total = matches.len();
        let total_files = all_files.len();
        Self {
            matches,
            counts: counts_out,
            files_per_kind: files_out,
            total,
            total_files,
        }
    }
}

// =============================================================================
// Pre-compiled regexes
// =============================================================================

// Rust `#[allow(...)]` / `#[expect(...)]` / `#[deny(...)]`. We capture the inner
// list so we can split into individual lint names and distinguish `dead_code`.
static RUST_ALLOW_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#\[(allow|expect|deny|warn)\(([^)]+)\)\]").unwrap());

// Rust `#[ignore]` test attribute (with or without reason string).
static RUST_IGNORE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*#\[ignore").unwrap());

// `unsafe {` block-open marker. We detect the brace then peek the body to
// decide UnsafeEnvVar vs Unsafe.
static RUST_UNSAFE_BLOCK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bunsafe\s*\{").unwrap());

// `std::env::set_var(...)` / `std::env::remove_var(...)` — body marker for
// Rust 2024 env-var boilerplate.
static RUST_ENV_VAR_CALL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:std::)?env::(?:set_var|remove_var)\s*\(").unwrap());

// Active nosemgrep directive: comment marker followed by `nosemgrep`,
// optionally `: rule-id`, with NOTHING ELSE meaningful after. Prose
// comments like `// nosemgrep can also appear in JS files` do NOT match
// (extra prose after the keyword disqualifies). This mirrors how Semgrep
// actually treats suppressions in practice.
static NOSEMGREP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?://|#)\s*nosemgrep(?:\s*:\s*([\w\-./:]+))?\s*$").unwrap());

// TypeScript directives. Distinct from each other.
static TS_IGNORE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@ts-ignore\b").unwrap());
static TS_EXPECT_ERR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@ts-expect-error\b").unwrap());
static TS_NOCHECK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@ts-nocheck\b").unwrap());

// eslint-disable family. Capture the optional rule list.
static ESLINT_DISABLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"eslint-disable(?:-next-line|-line)?(?:\s+([\w\-,/@\s]+?))?(?:\s*\*/|$|\s*//)")
        .unwrap()
});

// Python `# noqa` (optionally with rule codes like `# noqa: E501,F401`).
static PY_NOQA_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#\s*noqa(?:\s*:\s*([\w,\s]+))?").unwrap());

// Python `# type: ignore` (optionally with rule like `# type: ignore[arg-type]`).
static PY_TYPE_IGNORE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#\s*type\s*:\s*ignore(?:\[([\w\-,\s]+)\])?").unwrap());

// Python `# pylint: disable=...`.
static PY_PYLINT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#\s*pylint\s*:\s*disable\s*=\s*([\w\-,\s]+)").unwrap());

// Python `# mypy: ...` file-level directives.
static PY_MYPY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#\s*mypy\s*:\s*([\w\-=,\s]+)").unwrap());

// Shell `# shellcheck disable=SCxxxx[,SCyyyy]`.
static SHELLCHECK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#\s*shellcheck\s+disable\s*=\s*([\w,\s]+)").unwrap());

// =============================================================================
// Scanner
// =============================================================================

/// Recursively walk the project and return every literal silencer occurrence.
///
/// The walk uses `walkdir` and applies a fixed set of always-skipped
/// directories (`target/`, `node_modules/`, `.git/`, `dist/`, `build/`,
/// `.venv/`, `venv/`, `__pycache__/`). Hidden directories are skipped except
/// `.github/` (CI silencers belong in the inventory).
///
/// The optional `extra_ignore_globs` (intended for `.semgrepignore` patterns)
/// are matched against each candidate file's repo-relative path.
///
/// `filter`: if non-empty, only kinds in this set are returned. An empty set
/// means "all kinds".
///
/// LITERAL-ONLY (see module docs). No semantic enrichment.
pub fn scan_repo(
    root: &Path,
    filter: &HashSet<SilencerKind>,
    extra_ignore_globs: &[String],
) -> Vec<SilencerMatch> {
    use walkdir::WalkDir;

    let mut matches: Vec<SilencerMatch> = Vec::new();
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let ignore_matcher = build_ignore_matcher(extra_ignore_globs);

    let walker = WalkDir::new(&canonical_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            if entry.depth() == 0 {
                return true;
            }
            // Always skip these heavy/irrelevant dirs.
            if matches!(
                name.as_ref(),
                "target"
                    | "node_modules"
                    | ".git"
                    | "dist"
                    | "build"
                    | ".venv"
                    | "venv"
                    | "__pycache__"
                    | ".cargo"
                    | ".npm"
            ) {
                return false;
            }
            // Skip other dotfiles/dotdirs except `.github/` (CI silencers count).
            if name.starts_with('.') && name != ".github" {
                return false;
            }
            true
        });

    for dent in walker.flatten() {
        let path = dent.path();
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !is_supported_ext(ext) {
            continue;
        }

        let rel = path
            .strip_prefix(&canonical_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        if ignore_matcher.is_match(&rel) {
            continue;
        }

        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };

        scan_file(&rel, &content, ext, filter, &mut matches);
    }

    // Stable order for reproducible CLI output.
    matches.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.kind.cmp(&b.kind))
    });

    matches
}

/// Convenience wrapper that scans the repo and aggregates into an `Inventory`.
pub fn inventory(
    root: &Path,
    filter: &HashSet<SilencerKind>,
    extra_ignore_globs: &[String],
) -> SilencerInventory {
    SilencerInventory::from_matches(scan_repo(root, filter, extra_ignore_globs))
}

fn is_supported_ext(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "mts"
            | "cts"
            | "svelte"
            | "astro"
            | "vue"
            | "py"
            | "pyi"
            | "sh"
            | "bash"
            | "zsh"
            | "ksh"
    )
}

fn want(filter: &HashSet<SilencerKind>, kind: SilencerKind) -> bool {
    filter.is_empty() || filter.contains(&kind)
}

/// Lexer carry state at a Rust line boundary.
///
/// Only constructs that can legally span a newline are represented: normal
/// `"..."` strings (Rust permits literal newlines and `\`-continuations inside
/// them), raw strings `r#"..."#`, and block comments `/* ... */` (which nest in
/// Rust). Char literals and `//` line comments never cross a newline, so they
/// collapse back to `Code` at end of line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RustCarry {
    Code,
    Str,
    RawStr(usize),
    BlockComment(u32),
}

/// Walk one Rust source line from a carry-in state.
///
/// Returns `(end_state, pos_in_code)` where `end_state` is the carry to hand to
/// the next line and `pos_in_code` reports whether byte offset `until` lives in
/// real code (not inside a string, raw string, or comment). Pass
/// `until = usize::MAX` when only the carry state is needed.
///
/// This is a deliberately small lexer — just enough to stop the silencer
/// scanners from matching `#[allow(...)]` / `unsafe { }` / `#[ignore]` tokens
/// that live inside multi-line help-text string constants or block comments
/// (the most common false positive when a tool documents the very directives it
/// detects). It is still literal/lexical (free-tier); no semantic enrichment.
fn rust_scan_line(line: &str, start: RustCarry, until: usize) -> (RustCarry, bool) {
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut state = start;
    let mut i = 0usize;
    let mut taken = false;
    let mut in_code = true;

    while i < len {
        // Snapshot the state the byte at `until` lives in, the moment we reach
        // or step past it (escapes may advance two bytes at a time).
        if !taken && i >= until {
            in_code = matches!(state, RustCarry::Code);
            taken = true;
        }
        match state {
            RustCarry::Code => {
                let b = bytes[i];
                // `//` line comment — rest of line is comment, never carries.
                if b == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
                    if !taken && until >= i {
                        in_code = false;
                    }
                    return (RustCarry::Code, in_code);
                }
                // `/*` block comment open.
                if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
                    state = RustCarry::BlockComment(1);
                    i += 2;
                    continue;
                }
                // Raw string opener (`r"`, `r#"`, `br##"`, …).
                if let Some((hashes, skip)) = raw_string_open(bytes, i) {
                    state = RustCarry::RawStr(hashes);
                    i += skip;
                    continue;
                }
                // Normal / byte string opener.
                if b == b'"' {
                    state = RustCarry::Str;
                    i += 1;
                    continue;
                }
                // Char literal (consumed whole so `'"'` never toggles a string)
                // or lifetime/label (consume only the leading quote).
                if b == b'\'' {
                    i += char_literal_len(bytes, i);
                    continue;
                }
                i += 1;
            }
            RustCarry::Str => {
                let b = bytes[i];
                if b == b'\\' {
                    i += 2; // skip the escaped byte (covers `\"`, `\\`, `\`-EOL)
                    continue;
                }
                if b == b'"' {
                    state = RustCarry::Code;
                }
                i += 1;
            }
            RustCarry::RawStr(hashes) => {
                if bytes[i] == b'"' && raw_string_close(bytes, i + 1, hashes) {
                    state = RustCarry::Code;
                    i += 1 + hashes;
                    continue;
                }
                i += 1;
            }
            RustCarry::BlockComment(depth) => {
                if bytes[i] == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
                    state = RustCarry::BlockComment(depth + 1);
                    i += 2;
                    continue;
                }
                if bytes[i] == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                    state = if depth <= 1 {
                        RustCarry::Code
                    } else {
                        RustCarry::BlockComment(depth - 1)
                    };
                    i += 2;
                    continue;
                }
                i += 1;
            }
        }
    }

    if !taken {
        in_code = matches!(state, RustCarry::Code);
    }
    (state, in_code)
}

/// An identifier-continuation byte (so a raw-string `r` is not mistaken for the
/// tail of an identifier like `foor`).
fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// If a raw-string opener begins at `i`, return `(hash_count,
/// bytes_to_skip_through_opening_quote)`. Accepts an optional `b` byte-string
/// prefix. Returns `None` when `r` only continues an identifier.
fn raw_string_open(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    let len = bytes.len();
    if i > 0 && is_ident_byte(bytes[i - 1]) {
        return None;
    }
    let mut j = i;
    if j < len && bytes[j] == b'b' {
        j += 1;
    }
    if j >= len || bytes[j] != b'r' {
        return None;
    }
    j += 1;
    let hash_start = j;
    while j < len && bytes[j] == b'#' {
        j += 1;
    }
    if j < len && bytes[j] == b'"' {
        Some((j - hash_start, (j - i) + 1))
    } else {
        None
    }
}

/// A raw string closes on `"` followed by exactly `hashes` `#`. `j` is the
/// index just after the closing quote.
fn raw_string_close(bytes: &[u8], j: usize, hashes: usize) -> bool {
    if j + hashes > bytes.len() {
        return false;
    }
    bytes[j..j + hashes].iter().all(|&b| b == b'#')
}

/// Bytes to consume for a char literal or lifetime starting at the `'` at `i`.
/// Char literals (`'a'`, `'\n'`, `'\''`, `'"'`) are consumed whole so their
/// contents never toggle string state; lifetimes/labels (`'a`) consume only the
/// leading quote.
fn char_literal_len(bytes: &[u8], i: usize) -> usize {
    let len = bytes.len();
    // Escaped char literal: '\?...' — closing quote within a small window.
    if i + 1 < len && bytes[i + 1] == b'\\' {
        let mut j = i + 2;
        while j < len && j <= i + 5 {
            if bytes[j] == b'\'' {
                return j - i + 1;
            }
            j += 1;
        }
        return 1; // malformed; treat as lifetime-ish
    }
    // Simple char literal: 'X'
    if i + 2 < len && bytes[i + 2] == b'\'' && bytes[i + 1] != b'\'' {
        return 3;
    }
    // Lifetime or label.
    1
}

/// Compute, for every line, the lexer carry state at its first byte. Index 0 is
/// always `Code`; each entry is the end state of the previous line.
fn rust_line_carry_states(lines: &[&str]) -> Vec<RustCarry> {
    let mut states = Vec::with_capacity(lines.len());
    let mut carry = RustCarry::Code;
    for line in lines {
        states.push(carry);
        carry = rust_scan_line(line, carry, usize::MAX).0;
    }
    states
}

/// Is byte offset `pos` on `line` real code, given the carry-in `start` state?
fn rust_position_in_code(line: &str, start: RustCarry, pos: usize) -> bool {
    rust_scan_line(line, start, pos).1
}

fn scan_file(
    rel_path: &str,
    content: &str,
    ext: &str,
    filter: &HashSet<SilencerKind>,
    out: &mut Vec<SilencerMatch>,
) {
    let lines: Vec<&str> = content.lines().collect();
    // Cross-line lexer state, computed once per Rust file so each line knows
    // whether it begins inside a multi-line string / block comment.
    let rust_carry: Vec<RustCarry> = if ext == "rs" {
        rust_line_carry_states(&lines)
    } else {
        Vec::new()
    };

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let snippet = line.trim().to_string();

        match ext {
            "rs" => scan_rust_line(
                LineCtx {
                    file: rel_path,
                    line,
                    line_num,
                    snippet: &snippet,
                    lines: &lines,
                    idx,
                    line_start: rust_carry[idx],
                },
                filter,
                out,
            ),
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" | "svelte" | "astro"
            | "vue" => {
                scan_js_ts_line(rel_path, line, line_num, &snippet, filter, out);
                // nosemgrep can also appear in JS/TS files
                scan_nosemgrep_line(rel_path, line, line_num, &snippet, filter, out);
            }
            "py" | "pyi" => {
                scan_python_line(rel_path, line, line_num, &snippet, filter, out);
                scan_nosemgrep_line(rel_path, line, line_num, &snippet, filter, out);
            }
            "sh" | "bash" | "zsh" | "ksh" => {
                scan_shell_line(rel_path, line, line_num, &snippet, filter, out);
                scan_nosemgrep_line(rel_path, line, line_num, &snippet, filter, out);
            }
            _ => {}
        }
    }
}

/// Per-line context bundled to keep `scan_rust_line` under clippy's
/// too-many-arguments threshold without a noisy `#[allow]`.
struct LineCtx<'a> {
    file: &'a str,
    line: &'a str,
    line_num: usize,
    snippet: &'a str,
    lines: &'a [&'a str],
    idx: usize,
    /// Lexer carry state at the first byte of this line — lets the attribute /
    /// `unsafe` scanners skip tokens inside a multi-line string or block comment
    /// opened on a previous line.
    line_start: RustCarry,
}

fn scan_rust_line(ctx: LineCtx<'_>, filter: &HashSet<SilencerKind>, out: &mut Vec<SilencerMatch>) {
    let LineCtx {
        file,
        line,
        line_num,
        snippet,
        lines,
        idx,
        line_start,
    } = ctx;
    let trimmed = line.trim_start();

    // 1. nosemgrep (Rust files have `// nosemgrep` too)
    scan_nosemgrep_line(file, line, line_num, snippet, filter, out);

    // Skip doc-comment context for the attribute / unsafe scanners:
    // `///`, `//!`, and `//` line comments contain examples and prose, not
    // active silencers.
    if trimmed.starts_with("///") || trimmed.starts_with("//!") || trimmed.starts_with("//") {
        return;
    }

    // 2. #[allow(...)] / #[expect(...)] / #[deny(...)] — distinguish dead_code
    // Skip the attribute scan if the `#[` token sits inside a string literal or
    // a comment (e.g. help text or test fixture content, including multi-line
    // string constants opened on an earlier line). The `unsafe { ... }` scan
    // below carries its own position check.
    let attr_in_code = line
        .find("#[")
        .map(|hi| rust_position_in_code(line, line_start, hi))
        .unwrap_or(false);
    if attr_in_code && let Some(caps) = RUST_ALLOW_RE.captures(line) {
        let directive = caps.get(1).map(|m| m.as_str()).unwrap_or("allow");
        let lints_raw = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        for lint in lints_raw.split(',') {
            let lint = lint.trim();
            if lint.is_empty() {
                continue;
            }
            let lint_normalized = lint.trim_start_matches("clippy::").to_string();
            let is_dead = lint_normalized == "dead_code";
            let kind = if is_dead {
                SilencerKind::DeadCode
            } else {
                SilencerKind::Allow
            };
            if want(filter, kind) {
                out.push(SilencerMatch {
                    kind,
                    file: file.to_string(),
                    line: line_num,
                    snippet: snippet.to_string(),
                    rule_id: Some(format!("{}({})", directive, lint)),
                });
            }
        }
    }

    // 3. #[ignore] test attribute
    if attr_in_code && RUST_IGNORE_RE.is_match(line) && want(filter, SilencerKind::Ignore) {
        out.push(SilencerMatch {
            kind: SilencerKind::Ignore,
            file: file.to_string(),
            line: line_num,
            snippet: snippet.to_string(),
            rule_id: None,
        });
    }

    // 4. unsafe { ... } — distinguish env-var boilerplate
    if let Some(unsafe_match) = RUST_UNSAFE_BLOCK_RE.find(line) {
        // Skip when the `unsafe` token sits inside a string literal or comment
        // (including a multi-line string opened on an earlier line).
        if !rust_position_in_code(line, line_start, unsafe_match.start()) {
            return;
        }
        // Determine UnsafeEnvVar by checking THIS line (single-line
        // `unsafe { ... }`) OR scanning forward through the block body
        // (multi-line). We walk forward until we hit the matching `}` (naive
        // depth counter) or run out of lines, and if any body line calls
        // `std::env::set_var` / `std::env::remove_var`, the whole block is
        // classified as `UnsafeEnvVar`. This catches Rust 2024 boilerplate
        // where the unsafe-wrap is required by the new function signature
        // but the body is pure env-var manipulation.
        let is_env_var = if RUST_ENV_VAR_CALL_RE.is_match(line) {
            true
        } else {
            let mut depth: i32 =
                line.matches('{').count() as i32 - line.matches('}').count() as i32;
            let mut found = false;
            for body in lines.iter().skip(idx + 1) {
                if RUST_ENV_VAR_CALL_RE.is_match(body) {
                    found = true;
                    break;
                }
                depth += body.matches('{').count() as i32 - body.matches('}').count() as i32;
                if depth <= 0 {
                    break;
                }
                // Safety budget — don't scan unbounded files.
                if depth > 32 {
                    break;
                }
            }
            found
        };

        let kind = if is_env_var {
            SilencerKind::UnsafeEnvVar
        } else {
            SilencerKind::Unsafe
        };
        if want(filter, kind) {
            out.push(SilencerMatch {
                kind,
                file: file.to_string(),
                line: line_num,
                snippet: snippet.to_string(),
                rule_id: None,
            });
        }
    }
}

fn scan_js_ts_line(
    file: &str,
    line: &str,
    line_num: usize,
    snippet: &str,
    filter: &HashSet<SilencerKind>,
    out: &mut Vec<SilencerMatch>,
) {
    if TS_IGNORE_RE.is_match(line) && want(filter, SilencerKind::TsIgnore) {
        out.push(SilencerMatch {
            kind: SilencerKind::TsIgnore,
            file: file.to_string(),
            line: line_num,
            snippet: snippet.to_string(),
            rule_id: None,
        });
    }
    if TS_EXPECT_ERR_RE.is_match(line) && want(filter, SilencerKind::TsExpectError) {
        out.push(SilencerMatch {
            kind: SilencerKind::TsExpectError,
            file: file.to_string(),
            line: line_num,
            snippet: snippet.to_string(),
            rule_id: None,
        });
    }
    if TS_NOCHECK_RE.is_match(line) && want(filter, SilencerKind::TsNocheck) {
        out.push(SilencerMatch {
            kind: SilencerKind::TsNocheck,
            file: file.to_string(),
            line: line_num,
            snippet: snippet.to_string(),
            rule_id: None,
        });
    }
    if want(filter, SilencerKind::EslintDisable)
        && let Some(caps) = ESLINT_DISABLE_RE.captures(line)
    {
        let rule = caps
            .get(1)
            .map(|m| m.as_str().trim().to_string())
            .filter(|s| !s.is_empty());
        out.push(SilencerMatch {
            kind: SilencerKind::EslintDisable,
            file: file.to_string(),
            line: line_num,
            snippet: snippet.to_string(),
            rule_id: rule,
        });
    }
}

fn scan_python_line(
    file: &str,
    line: &str,
    line_num: usize,
    snippet: &str,
    filter: &HashSet<SilencerKind>,
    out: &mut Vec<SilencerMatch>,
) {
    if want(filter, SilencerKind::Noqa)
        && let Some(caps) = PY_NOQA_RE.captures(line)
    {
        let rule = caps
            .get(1)
            .map(|m| m.as_str().trim().to_string())
            .filter(|s| !s.is_empty());
        out.push(SilencerMatch {
            kind: SilencerKind::Noqa,
            file: file.to_string(),
            line: line_num,
            snippet: snippet.to_string(),
            rule_id: rule,
        });
    }
    if want(filter, SilencerKind::TypeIgnore)
        && let Some(caps) = PY_TYPE_IGNORE_RE.captures(line)
    {
        let rule = caps
            .get(1)
            .map(|m| m.as_str().trim().to_string())
            .filter(|s| !s.is_empty());
        out.push(SilencerMatch {
            kind: SilencerKind::TypeIgnore,
            file: file.to_string(),
            line: line_num,
            snippet: snippet.to_string(),
            rule_id: rule,
        });
    }
    if want(filter, SilencerKind::PylintDisable)
        && let Some(caps) = PY_PYLINT_RE.captures(line)
    {
        let rule = caps
            .get(1)
            .map(|m| m.as_str().trim().to_string())
            .filter(|s| !s.is_empty());
        out.push(SilencerMatch {
            kind: SilencerKind::PylintDisable,
            file: file.to_string(),
            line: line_num,
            snippet: snippet.to_string(),
            rule_id: rule,
        });
    }
    if want(filter, SilencerKind::MypyIgnore)
        && let Some(caps) = PY_MYPY_RE.captures(line)
    {
        let body = caps
            .get(1)
            .map(|m| m.as_str().trim().to_string())
            .filter(|s| !s.is_empty());
        // Avoid double-counting `# mypy: ignore` which the type-ignore regex
        // wouldn't catch but also isn't necessarily "mypy directive". We treat
        // any `# mypy: X` as a MypyIgnore match. Distinct from `# type: ignore`.
        out.push(SilencerMatch {
            kind: SilencerKind::MypyIgnore,
            file: file.to_string(),
            line: line_num,
            snippet: snippet.to_string(),
            rule_id: body,
        });
    }
}

fn scan_shell_line(
    file: &str,
    line: &str,
    line_num: usize,
    snippet: &str,
    filter: &HashSet<SilencerKind>,
    out: &mut Vec<SilencerMatch>,
) {
    if want(filter, SilencerKind::Shellcheck)
        && let Some(caps) = SHELLCHECK_RE.captures(line)
    {
        let rule = caps
            .get(1)
            .map(|m| m.as_str().trim().to_string())
            .filter(|s| !s.is_empty());
        out.push(SilencerMatch {
            kind: SilencerKind::Shellcheck,
            file: file.to_string(),
            line: line_num,
            snippet: snippet.to_string(),
            rule_id: rule,
        });
    }
}

fn scan_nosemgrep_line(
    file: &str,
    line: &str,
    line_num: usize,
    snippet: &str,
    filter: &HashSet<SilencerKind>,
    out: &mut Vec<SilencerMatch>,
) {
    if !want(filter, SilencerKind::Nosemgrep) {
        return;
    }
    // Skip Rust/JS doc-comment context (`///`, `//!`) and lines where the
    // token sits inside a string literal — these are *documentation about*
    // silencers, not active silencers. Heuristic: if the `nosemgrep` token
    // is preceded by a `"` on the same line, it's almost certainly inside a
    // string literal (test fixture / help text). This is intentionally
    // conservative; semantic detection of "is this token active?" lives in
    // paid-tier scope (Wave 7+).
    let trimmed = line.trim_start();
    if trimmed.starts_with("///") || trimmed.starts_with("//!") {
        return;
    }
    if let Some(idx) = line.find("nosemgrep") {
        let before = &line[..idx];
        // Count unescaped `"` before the token. Odd count = inside string literal.
        let quote_count = before.matches('"').count() - before.matches("\\\"").count();
        if quote_count % 2 == 1 {
            return;
        }
    }
    if let Some(caps) = NOSEMGREP_RE.captures(line) {
        let rule = caps
            .get(1)
            .map(|m| m.as_str().trim().to_string())
            .filter(|s| !s.is_empty());
        out.push(SilencerMatch {
            kind: SilencerKind::Nosemgrep,
            file: file.to_string(),
            line: line_num,
            snippet: snippet.to_string(),
            rule_id: rule,
        });
    }
}

// =============================================================================
// .semgrepignore parsing (lightweight)
// =============================================================================

/// Load `.semgrepignore` patterns from a project root. Returns one pattern per
/// line, comments (`#`) and empty lines stripped. If the file does not exist,
/// returns an empty `Vec`.
///
/// `.semgrepignore` syntax is path-glob style (similar to `.gitignore` but
/// with different semantics — Semgrep applies them at scan-time, not via
/// `git`). We do a minimal-correct parse: line-oriented, `#` comments, blank
/// lines skipped, trailing whitespace trimmed. Directory patterns get a `**`
/// suffix appended so `loctree-rs/tests/` matches everything inside.
pub fn load_semgrepignore(root: &Path) -> Vec<String> {
    let path = root.join(".semgrepignore");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let pattern = line.trim_end_matches('/');
        if line.ends_with('/') || !pattern.contains('.') {
            // Directory-style entry — match everything inside.
            out.push(format!("{}/**", pattern));
        } else {
            out.push(pattern.to_string());
        }
    }
    out
}

/// Matcher that holds the compiled globs from `.semgrepignore` (plus any
/// additional patterns the caller wants applied). Backed by `globset` so the
/// match path stays cheap even on large repos.
struct IgnoreMatcher {
    set: Option<globset::GlobSet>,
    /// Plain prefix matches (e.g. bare file/dir like `loctree-rs/tests/`).
    prefixes: Vec<String>,
}

impl IgnoreMatcher {
    fn is_match(&self, rel_path: &str) -> bool {
        if let Some(set) = &self.set
            && set.is_match(rel_path)
        {
            return true;
        }
        self.prefixes.iter().any(|p| rel_path.starts_with(p))
    }
}

fn build_ignore_matcher(patterns: &[String]) -> IgnoreMatcher {
    let mut builder = globset::GlobSetBuilder::new();
    let mut any = false;
    let mut prefixes: Vec<String> = Vec::new();
    for p in patterns {
        let raw = p.trim();
        if raw.is_empty() {
            continue;
        }
        // Strip leading `/` (semgrepignore-style "anchor at root").
        let cleaned = raw.trim_start_matches('/').to_string();
        // Direct glob match attempt.
        if let Ok(g) = globset::Glob::new(&cleaned) {
            builder.add(g);
            any = true;
        }
        // Also try with a `**/` prefix so bare file names match anywhere.
        if !cleaned.contains('/')
            && let Ok(g) = globset::Glob::new(&format!("**/{}", cleaned))
        {
            builder.add(g);
            any = true;
        }
        // Always also register as a prefix-match string (covers
        // `loctree-rs/tests/` style entries even if the glob compiler is picky).
        if !cleaned.contains('*') && !cleaned.contains('?') && !cleaned.contains('[') {
            prefixes.push(cleaned);
        }
    }
    let set = if any { builder.build().ok() } else { None };
    IgnoreMatcher { set, prefixes }
}

/// Resolve the working set of ignore globs for a project: optionally include
/// `.semgrepignore`, then dedupe.
pub fn resolve_ignore_globs(root: &Path, include_semgrepignore: bool) -> Vec<String> {
    if !include_semgrepignore {
        return Vec::new();
    }
    load_semgrepignore(root)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn from_filter_recognizes_canonical_tokens() {
        assert_eq!(
            SilencerKind::from_filter("allow"),
            Some(SilencerKind::Allow)
        );
        assert_eq!(
            SilencerKind::from_filter("dead-code"),
            Some(SilencerKind::DeadCode)
        );
        assert_eq!(
            SilencerKind::from_filter("dead_code"),
            Some(SilencerKind::DeadCode)
        );
        assert_eq!(
            SilencerKind::from_filter("nosemgrep"),
            Some(SilencerKind::Nosemgrep)
        );
        assert_eq!(
            SilencerKind::from_filter("ts-ignore"),
            Some(SilencerKind::TsIgnore)
        );
        assert_eq!(SilencerKind::from_filter("noqa"), Some(SilencerKind::Noqa));
        assert_eq!(
            SilencerKind::from_filter("unsafe-env-var"),
            Some(SilencerKind::UnsafeEnvVar)
        );
        assert_eq!(SilencerKind::from_filter("not-a-kind"), None);
    }

    #[test]
    fn rust_allow_dead_code_split_into_dead_code_bucket() {
        let tmp = TempDir::new().unwrap();
        // Initialize as git repo so the walker honors gitignore rules cleanly.
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        write(
            tmp.path(),
            "src/lib.rs",
            "#[allow(dead_code)]\nfn parked() {}\n#[allow(unused)]\nfn unused() {}\n",
        );

        let matches = scan_repo(tmp.path(), &HashSet::new(), &[]);
        let kinds: Vec<_> = matches.iter().map(|m| m.kind).collect();
        assert!(kinds.contains(&SilencerKind::DeadCode));
        assert!(kinds.contains(&SilencerKind::Allow));
    }

    #[test]
    fn rust_unsafe_env_var_distinguished_from_real_unsafe() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        write(
            tmp.path(),
            "src/main.rs",
            "fn a() { unsafe {\n    std::env::set_var(\"K\", \"v\");\n} }\n\
             fn b() { unsafe { std::ptr::null::<u8>().read(); } }\n",
        );

        let matches = scan_repo(tmp.path(), &HashSet::new(), &[]);
        let kinds: Vec<_> = matches.iter().map(|m| m.kind).collect();
        assert!(
            kinds.contains(&SilencerKind::UnsafeEnvVar),
            "env-var unsafe should be classified as UnsafeEnvVar, got: {:?}",
            kinds
        );
        assert!(
            kinds.contains(&SilencerKind::Unsafe),
            "real unsafe should be classified as Unsafe, got: {:?}",
            kinds
        );
    }

    #[test]
    fn nosemgrep_detected_cross_language() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        write(
            tmp.path(),
            "src/a.rs",
            "// nosemgrep: rust.foo\nfn x() {}\n",
        );
        write(
            tmp.path(),
            "lib/b.ts",
            "// nosemgrep\nexport const x = 1;\n",
        );
        write(tmp.path(), "scripts/c.py", "# nosemgrep\nx = 1\n");

        let mut filter = HashSet::new();
        filter.insert(SilencerKind::Nosemgrep);
        let matches = scan_repo(tmp.path(), &filter, &[]);
        assert_eq!(
            matches.len(),
            3,
            "expected 3 nosemgrep matches, got {:?}",
            matches
        );
    }

    #[test]
    fn ts_directives_classified_distinctly() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        write(
            tmp.path(),
            "src/x.ts",
            "// @ts-ignore\nconst a = 1;\n// @ts-expect-error\nconst b = 2;\n// @ts-nocheck\nconst c = 3;\n",
        );

        let matches = scan_repo(tmp.path(), &HashSet::new(), &[]);
        let kinds: Vec<_> = matches.iter().map(|m| m.kind).collect();
        assert!(kinds.contains(&SilencerKind::TsIgnore));
        assert!(kinds.contains(&SilencerKind::TsExpectError));
        assert!(kinds.contains(&SilencerKind::TsNocheck));
    }

    #[test]
    fn python_noqa_and_type_ignore_distinct() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        write(
            tmp.path(),
            "app.py",
            "x = 1  # noqa: E501\ny = foo()  # type: ignore[arg-type]\n",
        );

        let matches = scan_repo(tmp.path(), &HashSet::new(), &[]);
        let kinds: Vec<_> = matches.iter().map(|m| m.kind).collect();
        assert!(kinds.contains(&SilencerKind::Noqa));
        assert!(kinds.contains(&SilencerKind::TypeIgnore));
    }

    #[test]
    fn shellcheck_disable_detected() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        write(
            tmp.path(),
            "deploy.sh",
            "#!/usr/bin/env bash\n# shellcheck disable=SC2086,SC2155\necho $foo\n",
        );

        let matches = scan_repo(tmp.path(), &HashSet::new(), &[]);
        assert!(matches.iter().any(|m| m.kind == SilencerKind::Shellcheck));
    }

    #[test]
    fn rust_ignore_test_attribute_detected() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        write(
            tmp.path(),
            "tests/it.rs",
            "#[test]\n#[ignore]\nfn skipped() {}\n#[test]\n#[ignore = \"flaky\"]\nfn skipped_doc() {}\n",
        );

        let matches = scan_repo(tmp.path(), &HashSet::new(), &[]);
        let ignore_count = matches
            .iter()
            .filter(|m| m.kind == SilencerKind::Ignore)
            .count();
        assert_eq!(ignore_count, 2, "expected 2 #[ignore] matches");
    }

    #[test]
    fn semgrepignore_excludes_listed_paths() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        write(tmp.path(), ".semgrepignore", "fixtures/\n");
        write(tmp.path(), "src/a.rs", "// nosemgrep\nfn x() {}\n");
        write(tmp.path(), "fixtures/b.rs", "// nosemgrep\nfn y() {}\n");

        let globs = load_semgrepignore(tmp.path());
        let matches = scan_repo(tmp.path(), &HashSet::new(), &globs);
        let nosemgrep_files: Vec<_> = matches
            .iter()
            .filter(|m| m.kind == SilencerKind::Nosemgrep)
            .map(|m| m.file.as_str())
            .collect();
        assert!(nosemgrep_files.iter().any(|f| f.starts_with("src/")));
        assert!(
            !nosemgrep_files.iter().any(|f| f.starts_with("fixtures/")),
            "fixtures/ should be excluded via .semgrepignore, got: {:?}",
            nosemgrep_files
        );
    }

    #[test]
    fn filter_restricts_kinds() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        write(
            tmp.path(),
            "src/lib.rs",
            "// nosemgrep\n#[allow(dead_code)]\nfn x() {}\n",
        );
        let mut only_nose = HashSet::new();
        only_nose.insert(SilencerKind::Nosemgrep);
        let matches = scan_repo(tmp.path(), &only_nose, &[]);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].kind, SilencerKind::Nosemgrep);
    }

    #[test]
    fn silencers_inside_multiline_string_literal_not_matched() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        // A help-text constant that documents the very directives the scanner
        // detects. The `#[allow(...)]`, `#[ignore]`, and `unsafe { }` tokens all
        // live inside a multi-line string literal opened on an earlier line —
        // none are real silencers.
        write(
            tmp.path(),
            "src/help.rs",
            "pub const HELP: &str = \"Surfaces every silencer:\n\
             #[allow(...)] / #[expect(...)] (Rust), #[ignore] test attrs,\n\
             unsafe { } blocks, // nosemgrep, @ts-ignore.\";\nfn real() {}\n",
        );

        let matches = scan_repo(tmp.path(), &HashSet::new(), &[]);
        assert!(
            matches.is_empty(),
            "tokens inside a multi-line string constant must not be reported as silencers, got: {:?}",
            matches
        );
    }

    #[test]
    fn real_silencer_after_multiline_string_still_detected() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        // Guards against the cross-line tracker over-reaching into a false
        // negative: a documented `#[allow]` inside the string, then a genuine
        // `#[allow(dead_code)]` on real code after the string closes.
        write(
            tmp.path(),
            "src/help.rs",
            "pub const HELP: &str = \"mentions #[allow(dead_code)]\n\
             across two lines unsafe { }\";\n#[allow(dead_code)]\nfn parked() {}\n",
        );

        let matches = scan_repo(tmp.path(), &HashSet::new(), &[]);
        let kinds: Vec<_> = matches.iter().map(|m| m.kind).collect();
        assert_eq!(
            matches.len(),
            1,
            "exactly the real dead_code attribute should match, got: {:?}",
            matches
        );
        assert!(kinds.contains(&SilencerKind::DeadCode));
    }

    #[test]
    fn silencers_inside_block_comment_not_matched() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        write(
            tmp.path(),
            "src/lib.rs",
            "/* example block comment:\n#[allow(dead_code)]\nunsafe { drop(0) }\n*/\nfn real() {}\n",
        );

        let matches = scan_repo(tmp.path(), &HashSet::new(), &[]);
        assert!(
            matches.is_empty(),
            "tokens inside a multi-line block comment must not be reported, got: {:?}",
            matches
        );
    }

    #[test]
    fn char_literal_quote_does_not_hide_following_unsafe() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        // A `'"'` char literal contains a lone double-quote. A naive quote
        // counter would treat everything after it as "inside a string" and hide
        // the genuine `unsafe` block that follows.
        write(
            tmp.path(),
            "src/lib.rs",
            "fn q() { let _c = '\"'; unsafe { std::ptr::null::<u8>().read(); } }\n",
        );

        let matches = scan_repo(tmp.path(), &HashSet::new(), &[]);
        assert!(
            matches.iter().any(|m| m.kind == SilencerKind::Unsafe),
            "real unsafe after a quote char literal must still be detected, got: {:?}",
            matches
        );
    }

    #[test]
    fn raw_string_with_quotes_does_not_hide_following_unsafe() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        // A single-line raw string containing an unbalanced inner `"` must not
        // leak string state onto the genuine `unsafe` block on the next line.
        write(
            tmp.path(),
            "src/lib.rs",
            "fn p() { let _r = r#\"a single \" quote\"#; }\nfn q() { unsafe { std::ptr::null::<u8>().read(); } }\n",
        );

        let matches = scan_repo(tmp.path(), &HashSet::new(), &[]);
        assert!(
            matches.iter().any(|m| m.kind == SilencerKind::Unsafe),
            "real unsafe after a raw string must still be detected, got: {:?}",
            matches
        );
    }

    #[test]
    fn inventory_aggregates_counts_correctly() {
        let tmp = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output()
            .ok();
        write(
            tmp.path(),
            "src/a.rs",
            "#[allow(dead_code)]\nfn a() {}\n#[allow(dead_code)]\nfn b() {}\n",
        );
        let inv = inventory(tmp.path(), &HashSet::new(), &[]);
        assert_eq!(inv.counts.get("dead-code"), Some(&2));
        assert_eq!(inv.files_per_kind.get("dead-code"), Some(&1));
        assert_eq!(inv.total, 2);
        assert_eq!(inv.total_files, 1);
    }
}
