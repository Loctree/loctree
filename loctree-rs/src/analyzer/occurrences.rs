//! Literal occurrence truth layer.
//!
//! `loct occurrences <ident>` answers a single, narrow question with zero
//! fuzz: **where does this exact identifier literally appear in the source?**
//!
//! Unlike `find` (which works off the AST/tagmap and can omit local variables
//! buried inside a large function body) this scanner walks raw file bytes and
//! reports every identifier-boundary match. "Not found" here means *not found*
//! — there are no fuzzy suggestions promoted as primary results, because a
//! suggestion is not evidence.
//!
//! The CodeScribe `utterance_id` failure class is the canonical motivation: a
//! `let mut utterance_id` plus later `utterance_id += 1` increments living
//! inside a 400-line function were invisible to `find`/`tagmap`. The literal
//! scanner sees them because it does not depend on symbol extraction.

use serde::{Deserialize, Serialize};
use strsim::levenshtein;

use crate::snapshot::Snapshot;
use crate::types::{FileAnalysis, ImportEntry, ReexportKind};

mod reporting;
mod summary;

use summary::{role_summary, scope_classification_counts, suggested_next};

/// Local, single-line classification of what an occurrence *looks like*.
///
/// This is **not** dataflow and never claims to be. It reads only the one line
/// the identifier sits on and answers a conservative "what does this site look
/// like?". Two complementary axes share this one enum:
///
/// * **Rust role shapes** (`definition_like` / `mutation_like` /
///   `field_emit_like`) — the original W2-B conservative dataflow hints, proven
///   against a `let` binding, an assignment operator, and a struct-literal field
///   shorthand. The `_like` suffix is load-bearing: it says "looks like", not
///   "is". An agent reading `mutation_like` must treat it as a cheap local
///   signal, not a verified interprocedural mutation.
/// * **Token-type shapes** (`css_property` / `class_token` / `custom_property` /
///   `comment` / `string_literal` / `data_attribute` / `identifier`) —
///   language-aware lexical classification so a literal hit on a CSS/TS token is
///   labelled by *what kind of token* it is, not left as a blanket `unknown`.
///
/// [`OccurrenceKind::Unknown`] is now an **honest fallback only**: it fires when
/// the single-line context cannot prove any role and the matched text is not a
/// clean identifier token — never as the silent default it used to be for every
/// non-Rust hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceKind {
    /// `let [mut] <ident> ...` — a binding / initialization site (Rust role).
    DefinitionLike,
    /// Rust `use` / `pub use` binding site.
    ImportLike,
    /// `<ident> <assign-op> ...` — `=` / `+=` / `-=` / … assignment (Rust role).
    /// Excludes `==` (comparison) and `=>` (match arm).
    MutationLike,
    /// `<ident>` as a struct-literal field shorthand: `Type { <ident>, .. }`,
    /// with the enclosing `{` on the **same line** (Rust role).
    FieldEmitLike,
    /// A CSS declaration property name, e.g. `backdrop-filter:` (the token sits
    /// immediately before a `:` declaration colon).
    CssProperty,
    /// A class/selector token: a CSS class selector (`.token`) or a
    /// utility/Tailwind class inside a `class=`/`className=` attribute string.
    ClassToken,
    /// A CSS custom property / variable, e.g. `--token` or `var(--token)`.
    CustomProperty,
    /// The occurrence sits inside a `//`/`#` line comment or a `/* … */` block
    /// comment (single-line lexical view).
    Comment,
    /// The occurrence sits inside a string/template literal.
    StringLiteral,
    /// An HTML/JSX `data-*` attribute name, e.g. `data-token`.
    DataAttribute,
    /// A bare identifier token in code that matches none of the more specific
    /// shapes above. The honest "it's an identifier read/use" label that
    /// replaces the old blanket `unknown` for ordinary code hits.
    Identifier,
    /// Honest fallback: the single-line context cannot prove any role and the
    /// matched text is not a clean identifier token.
    #[default]
    Unknown,
}

impl OccurrenceKind {
    /// Stable lowercase label, byte-for-byte identical to the serialized JSON
    /// value. Reused for human CLI output so the two surfaces never drift.
    pub fn as_str(&self) -> &'static str {
        match self {
            OccurrenceKind::DefinitionLike => "definition_like",
            OccurrenceKind::ImportLike => "import_like",
            OccurrenceKind::MutationLike => "mutation_like",
            OccurrenceKind::FieldEmitLike => "field_emit_like",
            OccurrenceKind::CssProperty => "css_property",
            OccurrenceKind::ClassToken => "class_token",
            OccurrenceKind::CustomProperty => "custom_property",
            OccurrenceKind::Comment => "comment",
            OccurrenceKind::StringLiteral => "string_literal",
            OccurrenceKind::DataAttribute => "data_attribute",
            OccurrenceKind::Identifier => "identifier",
            OccurrenceKind::Unknown => "unknown",
        }
    }
}

/// Query-level shape for a literal scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    Empty,
    Identifier,
    Phrase,
    Symbolic,
}

/// Exact matching strategy used for this literal scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    IdentifierBoundary,
    WholeTokenBoundary,
    FixedString,
    /// Free regex over raw file text (no identifier boundary). Used by
    /// `find --regex` / the pattern-scan surface for security/privacy audits.
    Regex,
}

/// Agent-readable role derived from [`OccurrenceKind`].
///
/// This is deliberately coarser than `occurrence_kind`: agents need a compact
/// "what should I do with this hit?" label, while `occurrence_kind` preserves
/// the exact local lexical evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchRole {
    Definition,
    LocalBinding,
    Import,
    Mutation,
    FieldEmission,
    StyleProperty,
    ClassToken,
    StyleVariable,
    Comment,
    StringLiteral,
    DataAttribute,
    Reference,
    Unknown,
}

impl MatchRole {
    pub fn from_occurrence_kind(kind: OccurrenceKind) -> Self {
        match kind {
            OccurrenceKind::DefinitionLike => MatchRole::LocalBinding,
            OccurrenceKind::ImportLike => MatchRole::Import,
            OccurrenceKind::MutationLike => MatchRole::Mutation,
            OccurrenceKind::FieldEmitLike => MatchRole::FieldEmission,
            OccurrenceKind::CssProperty => MatchRole::StyleProperty,
            OccurrenceKind::ClassToken => MatchRole::ClassToken,
            OccurrenceKind::CustomProperty => MatchRole::StyleVariable,
            OccurrenceKind::Comment => MatchRole::Comment,
            OccurrenceKind::StringLiteral => MatchRole::StringLiteral,
            OccurrenceKind::DataAttribute => MatchRole::DataAttribute,
            OccurrenceKind::Identifier => MatchRole::Reference,
            OccurrenceKind::Unknown => MatchRole::Unknown,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            MatchRole::Definition => "definition",
            MatchRole::LocalBinding => "local_binding",
            MatchRole::Import => "import",
            MatchRole::Mutation => "mutation",
            MatchRole::FieldEmission => "field_emission",
            MatchRole::StyleProperty => "style_property",
            MatchRole::ClassToken => "class_token",
            MatchRole::StyleVariable => "style_variable",
            MatchRole::Comment => "comment",
            MatchRole::StringLiteral => "string_literal",
            MatchRole::DataAttribute => "data_attribute",
            MatchRole::Reference => "reference",
            MatchRole::Unknown => "unknown",
        }
    }
}

/// Confidence for the local role classification, not for the literal match
/// itself. Literal matches are exact; this field says how strong the local role
/// inference is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchConfidence {
    High,
    Medium,
    Low,
}

impl MatchConfidence {
    pub fn from_occurrence_kind(kind: OccurrenceKind) -> Self {
        match kind {
            OccurrenceKind::Unknown => MatchConfidence::Low,
            OccurrenceKind::Identifier => MatchConfidence::Medium,
            _ => MatchConfidence::High,
        }
    }
}

fn match_confidence(kind: OccurrenceKind, role: MatchRole) -> MatchConfidence {
    if matches!(role, MatchRole::Import | MatchRole::LocalBinding)
        || (role == MatchRole::Definition && kind == OccurrenceKind::Identifier)
    {
        MatchConfidence::High
    } else {
        MatchConfidence::from_occurrence_kind(kind)
    }
}

/// Coarse file-scope bucket for agent triage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeClassification {
    Production,
    Test,
    Docs,
    Config,
    Generated,
    Unknown,
}

impl ScopeClassification {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScopeClassification::Production => "production",
            ScopeClassification::Test => "test",
            ScopeClassification::Docs => "docs",
            ScopeClassification::Config => "config",
            ScopeClassification::Generated => "generated",
            ScopeClassification::Unknown => "unknown",
        }
    }
}

/// Aggregated file-scope counts for the current result set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopeClassificationCount {
    pub scope_classification: ScopeClassification,
    pub count: usize,
}

/// A suggested next command for turning literal evidence into structural
/// context. Suggestions are hints only; they never substitute for the literal
/// match result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SuggestedNext {
    pub command: String,
    pub reason: String,
}

/// Symbol-table hint surfaced only when literal occurrences are absent.
///
/// These are prefix/substring/fuzzy matches against known symbols, not evidence
/// that the queried identifier exists. Keeping them on a separate field
/// preserves the core literal contract while giving agents an immediate
/// typo/rename lead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NearMatch {
    pub symbol: String,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    pub kind: String,
    pub match_kind: &'static str,
    pub source: &'static str,
}

/// Definition-vs-callsite roll-up over the whole result set.
///
/// Agents asking "is this symbol mostly *defined* here or mostly *used* here?"
/// should not have to walk every occurrence to find out. This compact rollup
/// answers it in one glance, bucketing by [`MatchRole`]:
///
/// * `definitions` — true symbol-definition sites (`MatchRole::Definition`).
/// * `callsites` — every site that *touches* the symbol without being its
///   top-level definition: references, mutations, field emissions, and local
///   bindings. This is the "where is it used / written?" bucket.
/// * `imports` — `use`/`import` binding sites.
/// * `non_code` — comments, string literals, data attributes (evidence that is
///   present in text but is not executable code touching the symbol).
/// * `other` — style/class/variable tokens and honest `unknown` fallbacks.
///
/// Counts are computed over the FULL occurrence set, before any `count_only`
/// slimming or pagination, so the rollup stays truthful even on a shortened
/// page. Additive: the field is `Option`, omitted entirely when there are no
/// hits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoleSummary {
    /// True symbol-definition sites.
    pub definitions: usize,
    /// Reference / mutation / field-emission / local-binding sites.
    pub callsites: usize,
    /// Import / `use` binding sites.
    pub imports: usize,
    /// Comment / string-literal / data-attribute sites (text, not live code).
    pub non_code: usize,
    /// Style / class / variable tokens and honest `unknown` fallbacks.
    pub other: usize,
    /// Files that carry at least one definition-role hit, in first-seen order.
    /// This is the "go look here for the definition" pointer for an agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub definition_files: Vec<String>,
}

/// Importer/consumer context for one file that carries literal hits.
///
/// Lifts a flat hit list into blast-radius awareness: for each file where the
/// query literally appears, who imports that file (consumers — the impact
/// surface) and what that file imports (its dependencies). Built from the
/// snapshot's canonical import edges, the same source `loct slice`/`impact`
/// read, so the two surfaces never drift. Additive and token-thrifty: emitted
/// only when the snapshot proves at least one edge for the file, and each list
/// is capped (`truncated` marks when it was).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileContext {
    /// File path (as stored in the snapshot, relative to project root).
    pub file: String,
    /// Coarse bucket for this file.
    pub scope_classification: ScopeClassification,
    /// Number of literal occurrences in this file (full set, pre-pagination).
    pub hits: usize,
    /// Files that import this file — its consumers / blast radius. Capped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imported_by: Vec<String>,
    /// Files this file imports — its dependencies. Capped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
    /// `true` when `imported_by`/`imports` were truncated for token economy.
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated: bool,
}

/// Symbol anchor attached to a literal occurrence when the snapshot can prove it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SymbolAnchor {
    pub name: String,
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    pub kind: String,
    pub symbol_id: String,
}

/// 1-based point in a literal occurrence range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OccurrencePoint {
    /// 1-based line number.
    pub line: usize,
    /// 1-based column offset. For range ends, this is exclusive.
    pub column: usize,
}

/// 1-based, single-line range for one literal occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OccurrenceRange {
    /// Inclusive start point.
    pub start: OccurrencePoint,
    /// Exclusive end point.
    pub end: OccurrencePoint,
}

/// A single literal occurrence of the searched identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiteralOccurrence {
    /// File path (as stored in the snapshot, relative to project root).
    pub file: String,
    /// 1-based line number.
    pub line: usize,
    /// 1-based column (char offset) where the identifier starts.
    pub column: usize,
    /// 1-based single-line range for this exact match. `end.column` is
    /// exclusive, matching editor/LSP selection semantics while preserving the
    /// existing human `line`/`column` fields.
    pub range: OccurrenceRange,
    /// The exact text matched (always equal to the query for identifier search).
    pub matched_text: String,
    /// The full source line, trimmed of trailing newline, for human context.
    pub context: String,
    /// Provenance marker. Always `"literal"` — never a fuzzy/semantic guess.
    pub source: &'static str,
    /// Conservative single-line classification of this site. See
    /// [`OccurrenceKind`]. Never a dataflow claim — `unknown` when unproven.
    pub occurrence_kind: OccurrenceKind,
    /// Compact agent-facing role derived conservatively from
    /// [`occurrence_kind`](Self::occurrence_kind).
    pub match_role: MatchRole,
    /// Confidence in `match_role`, not in the literal match itself.
    pub confidence: MatchConfidence,
    /// Coarse bucket for where this match sits in the repository.
    pub scope_classification: ScopeClassification,
    /// Nearest enclosing function/symbol, or file-level fallback when this hit is
    /// outside a symbol body (for example imports at module top level).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enclosing_symbol: Option<SymbolAnchor>,
    /// Definition this occurrence resolves to when the snapshot can prove one.
    /// Local bindings are intentionally not promoted to symbol definitions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_definition: Option<SymbolAnchor>,
    /// Same-named definitions in the repo. This is how literal hits stay
    /// rg-complete while still exposing twin ambiguity explicitly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub definition_candidates: Vec<SymbolAnchor>,
}

/// A per-file occurrence count, emitted by the `group_by_file` rollup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileCount {
    /// File path (as stored in the snapshot, relative to project root).
    pub file: String,
    /// Number of literal occurrences in this file.
    pub count: usize,
    /// Coarse bucket for this file.
    pub scope_classification: ScopeClassification,
}

/// Metadata for a deliberately paged occurrence list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OccurrencePage {
    /// Zero-based offset into the full occurrence set.
    pub offset: usize,
    /// Maximum number of occurrences requested for this page.
    pub limit: usize,
    /// Number of occurrences actually returned in this page.
    pub returned: usize,
    /// Whether more occurrences exist after this page.
    pub has_more: bool,
    /// Offset to request for the next page. Omitted on the final page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
}

/// Aggregated literal-occurrence result for one identifier query.
///
/// New fields (`total`, `by_file`, `slim`, `page`) are **additive** and
/// serialized conservatively: `by_file`, `slim`, and `page` only appear when the
/// caller opts in, so existing consumers (context-atlas, suppressions, other
/// agents) see the same shape they always have. `total` is the occurrence count
/// *before* any slim suppression or page slicing — reliable even when
/// `occurrences` is intentionally shortened.
/// Detailed scope statistics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiteralScopeStats {
    pub files_in_universe: usize,
    pub files_scanned: usize,
    pub vendored: usize,
    pub fixtures: usize,
    pub generated: usize,
    pub templates: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct OccurrenceResults {
    /// The identifier that was searched for.
    pub query: String,
    /// Coarse shape of the query string (`identifier`, `phrase`, `symbolic`,
    /// or `empty`).
    pub query_kind: QueryKind,
    /// Exact literal matching strategy used for this query.
    pub match_mode: MatchMode,
    /// Every literal occurrence found, in (file, line, column) order.
    ///
    /// Emptied (but the file/total counters preserved) when `slim`/`count_only`
    /// is requested, so an agent can ask "how many / where" without paying the
    /// token cost of the full list.
    pub occurrences: Vec<LiteralOccurrence>,
    /// Number of distinct files containing at least one occurrence.
    pub files_matched: usize,
    /// Total number of occurrences found, independent of `slim` truncation.
    pub total: usize,
    /// Per-file occurrence rollup. `Some` only when `group_by_file` is requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_file: Option<Vec<FileCount>>,
    /// `true` when the full `occurrences` list was intentionally suppressed for
    /// token economy (`count_only`/`slim`). Distinguishes "empty because slim"
    /// from "empty because not found".
    #[serde(skip_serializing_if = "is_false")]
    pub slim: bool,
    /// Page metadata. `Some` only when the caller requested `limit`/`offset`
    /// pagination for the occurrence list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<OccurrencePage>,
    /// Provenance marker for the whole result set. Always `"literal"`.
    pub source: &'static str,
    /// Coverage line: "scanned 723 of 767 repo files; excluded: generated(28), vendored(9), fixtures(7)"
    #[serde(default)]
    pub coverage_line: String,
    /// Detailed scope statistics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<LiteralScopeStats>,
    /// Counts by coarse file-scope bucket for the result set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope_classifications: Vec<ScopeClassificationCount>,
    /// Next structural commands an agent can run after reading this literal
    /// result. These remain suggestions, not evidence.
    pub suggested_next: Vec<SuggestedNext>,
    /// Prefix/substring symbol-table hints when the literal result set is empty.
    /// These are never primary matches and are omitted when exact occurrences
    /// exist or no nearby symbol names are known.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub near_matches: Vec<NearMatch>,
    /// Definition-vs-callsite roll-up for the whole result set. `Some` whenever
    /// there is at least one hit; omitted on a not-found result. Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_summary: Option<RoleSummary>,
    /// Per-file importer/consumer context for files carrying hits. Populated by
    /// [`enrich_with_snapshot`] from the snapshot's import edges; empty when no
    /// snapshot enrichment ran or no edges were proven. Additive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_context: Vec<FileContext>,
}

#[inline]
fn is_false(b: &bool) -> bool {
    !*b
}

/// Identifier-boundary control for the literal scan.
///
/// Default (`whole_token = false`) preserves the historical behavior exactly:
/// `[A-Za-z0-9_]` are identifier-internal, so `backdrop` still matches inside
/// the hyphenated CSS token `--vista-z-overlay-backdrop`. Opt-in
/// `whole_token = true` additionally treats `-` as token-internal, so the same
/// query no longer lights up `overlay-backdrop` / `--vista-z-overlay-backdrop`
/// — the z-index noise the literal layer used to drag in.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanOptions {
    /// Treat `-` as part of the token (tighter boundary). Opt-in, no default
    /// regression.
    pub whole_token: bool,
}

/// Output-shaping controls applied *after* scanning, shared by every surface so
/// CLI and MCP stay byte-for-byte at parity.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReportOptions {
    /// Attach a per-file occurrence rollup (`by_file`).
    pub group_by_file: bool,
    /// Suppress the full `occurrences` list, keeping only counters (`slim`).
    pub count_only: bool,
    /// Zero-based occurrence offset for paged output.
    pub offset: usize,
    /// Maximum number of occurrences to return in the current page.
    pub limit: Option<usize>,
}

/// Optional exact file/path scope for literal scans.
///
/// This is deliberately path-only, not a search system. It lets `find --literal
/// --file`, LSP/MCP `file=...`, and tests feed the same already-selected file
/// set into the exact occurrence scanner.
#[derive(Debug, Clone, Copy, Default)]
pub struct FileScope<'a> {
    /// Relative path/prefix to keep, e.g. `src/app.css`. Leading `./` and
    /// platform separators are normalized. A file matches when it is exactly
    /// this path or ends with `/<scope>`.
    pub file: Option<&'a str>,
}

impl FileScope<'_> {
    pub fn matches(&self, path: &str) -> bool {
        let Some(file) = self.file else {
            return true;
        };
        path_matches_scope(path, file)
    }
}

/// Rust/C-family identifier character test: `[A-Za-z0-9_]`.
///
/// We treat `_` and ASCII alphanumerics as identifier-internal. Anything else
/// (operators, whitespace, punctuation, `.`, `:`) is a boundary. This is what
/// makes the match a *token* match rather than a naive substring: searching
/// `id` must not light up `utterance_id`, `valid`, or `id_map`.
#[inline]
fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Boundary test under a given [`ScanOptions`]. With `whole_token`, `-` also
/// counts as token-internal, so a query like `backdrop` no longer matches
/// inside `overlay-backdrop` / `--vista-z-overlay-backdrop`.
#[inline]
fn is_boundary_char(c: char, whole_token: bool) -> bool {
    is_ident_char(c) || (whole_token && c == '-')
}

#[inline]
fn is_boundary_byte(b: u8, whole_token: bool) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || (whole_token && b == b'-')
}

fn uses_token_boundaries(query: &str, whole_token: bool) -> bool {
    !query.is_empty()
        && query
            .chars()
            .all(|c| is_ident_char(c) || (whole_token && c == '-'))
}

fn query_kind(query: &str) -> QueryKind {
    if query.is_empty() {
        QueryKind::Empty
    } else if is_identifier(query) {
        QueryKind::Identifier
    } else if query.chars().any(char::is_whitespace) {
        QueryKind::Phrase
    } else {
        QueryKind::Symbolic
    }
}

fn match_mode(query: &str, whole_token: bool) -> MatchMode {
    if uses_token_boundaries(query, whole_token) {
        if whole_token {
            MatchMode::WholeTokenBoundary
        } else {
            MatchMode::IdentifierBoundary
        }
    } else {
        MatchMode::FixedString
    }
}

fn occurrences_in_ascii_line(line: &str, needle: &str, whole_token: bool) -> Vec<usize> {
    let line_bytes = line.as_bytes();
    let needle_bytes = needle.as_bytes();
    let n = line_bytes.len();
    let m = needle_bytes.len();
    if m == 0 || m > n {
        return Vec::new();
    }

    let boundary_aware = uses_token_boundaries(needle, whole_token);
    let mut cols = Vec::new();
    let mut start = 0usize;
    while start + m <= n {
        if line_bytes[start..start + m] == needle_bytes[..] {
            let left_ok = !boundary_aware
                || start == 0
                || !is_boundary_byte(line_bytes[start - 1], whole_token);
            let right_ok = !boundary_aware
                || start + m == n
                || !is_boundary_byte(line_bytes[start + m], whole_token);
            if left_ok && right_ok {
                cols.push(start + 1); // ASCII byte offset == char column.
                start += m;
                continue;
            }
        }
        start += 1;
    }
    cols
}

/// Find every literal occurrence of `ident` within a single line.
///
/// Returns 1-based column offsets (counted in `char`s, not bytes) at which a
/// match begins. Identifier-like queries stay boundary-delimited, so searching
/// `id` does not light up `utterance_id`. Phrase and punctuation queries are
/// fixed-string literals, so `snapshot fresh` behaves like raw filesystem
/// literal search instead of an identifier lookup.
///
/// This is the default-boundary entry point (`whole_token = false`), kept stable
/// for every existing caller. Use [`occurrences_in_line_with`] for tighter
/// `whole_token` boundaries.
pub fn occurrences_in_line(line: &str, ident: &str) -> Vec<usize> {
    occurrences_in_line_with(line, ident, false)
}

/// [`occurrences_in_line`] with explicit token-boundary control. When
/// `whole_token` is set, hyphenated neighbors are treated as the same token for
/// token-like queries, so `backdrop` only fires on a free-standing token.
pub fn occurrences_in_line_with(line: &str, needle: &str, whole_token: bool) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }

    if line.is_ascii() && needle.is_ascii() {
        return occurrences_in_ascii_line(line, needle, whole_token);
    }

    // Work on chars so column numbers are stable for non-ASCII source lines.
    let line_chars: Vec<char> = line.chars().collect();
    let needle_chars: Vec<char> = needle.chars().collect();
    let n = line_chars.len();
    let m = needle_chars.len();
    if m == 0 || m > n {
        return Vec::new();
    }

    let boundary_aware = uses_token_boundaries(needle, whole_token);
    let mut cols = Vec::new();
    let mut start = 0usize;
    while start + m <= n {
        if line_chars[start..start + m] == needle_chars[..] {
            let left_ok = !boundary_aware
                || start == 0
                || !is_boundary_char(line_chars[start - 1], whole_token);
            let right_ok = !boundary_aware
                || start + m == n
                || !is_boundary_char(line_chars[start + m], whole_token);
            if left_ok && right_ok {
                cols.push(start + 1); // 1-based column
                start += m;
                continue;
            }
        }
        start += 1;
    }
    cols
}

fn occurrence_range(line: usize, column: usize, matched_text: &str) -> OccurrenceRange {
    OccurrenceRange {
        start: OccurrencePoint { line, column },
        end: OccurrencePoint {
            line,
            column: column + matched_text.chars().count(),
        },
    }
}

/// Locally classify one occurrence by its single-line lexical neighborhood.
///
/// `col_1based` is the column [`occurrences_in_line`] reports — the 1-based
/// `char` offset where `ident` begins on `line`. The function reads only this
/// one line; it never looks at sibling lines, the AST, or types. Three proven
/// Rust shapes are recognized, in priority order:
///
/// 1. `let [mut] <ident>` → [`OccurrenceKind::DefinitionLike`]. Checked first so
///    that `let x = 0` reads as a definition, not a mutation, even though `=`
///    follows the name.
/// 2. `<ident> <assign-op>` → [`OccurrenceKind::MutationLike`].
/// 3. struct-literal field shorthand (`Type { <ident>, .. }`, opening `{` on the
///    same line) → [`OccurrenceKind::FieldEmitLike`].
///
/// Anything else stays [`OccurrenceKind::Unknown`].
pub fn classify_occurrence(line: &str, ident: &str, col_1based: usize) -> OccurrenceKind {
    let chars: Vec<char> = line.chars().collect();
    let start = col_1based.saturating_sub(1);
    let ident_len = ident.chars().count();
    if ident_len == 0 || start + ident_len > chars.len() {
        return OccurrenceKind::Unknown;
    }

    let prefix: String = chars[..start].iter().collect();
    let suffix: String = chars[start + ident_len..].iter().collect();
    let pre = prefix.trim_end();
    let suf = suffix.trim_start();

    // 1. `let [mut] <ident>` binding.
    if prefix_is_let_binding(pre) {
        return OccurrenceKind::DefinitionLike;
    }

    // 2. `<ident> <assign-op>` assignment / compound assignment.
    if suffix_is_assignment(suf) {
        return OccurrenceKind::MutationLike;
    }

    // 3. struct-literal field shorthand. The innermost still-open delimiter on
    //    this line must be `{` (a struct literal / block, not a call's `(`), the
    //    name carries no `: value` (bare shorthand), and it is terminated by `,`
    //    or `}`. Multiline literals whose `{` is on a previous line stay
    //    `unknown` — we never guess past the current line.
    if innermost_open_delim(pre) == Some('{')
        && (suf.is_empty() || suf.starts_with(',') || suf.starts_with('}'))
    {
        return OccurrenceKind::FieldEmitLike;
    }

    OccurrenceKind::Unknown
}

/// True when the whitespace-delimited token(s) immediately before the identifier
/// are `let` or `let mut` — a binding introducer.
fn prefix_is_let_binding(pre: &str) -> bool {
    let mut toks = pre.split_whitespace().rev();
    match toks.next() {
        Some("let") => true,
        Some("mut") => toks.next() == Some("let"),
        _ => false,
    }
}

/// True when `suf` (already left-trimmed) begins with an assignment operator.
/// Recognizes `=` and the compound operators, while rejecting `==` (equality)
/// and `=>` (match arm) which are not mutations.
fn suffix_is_assignment(suf: &str) -> bool {
    const COMPOUND: [&str; 10] = ["<<=", ">>=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^="];
    if COMPOUND.iter().any(|op| suf.starts_with(op)) {
        return true;
    }
    let mut cs = suf.chars();
    if cs.next() == Some('=') {
        // Bare `=` assignment, but not `==` or `=>`.
        return !matches!(cs.next(), Some('=') | Some('>'));
    }
    false
}

/// Return the innermost still-open bracket delimiter after scanning `pre`
/// (`(`, `[`, or `{`), or `None` if every opener on this line was closed. Used
/// to tell a struct literal's `{` apart from a call's `(`.
fn innermost_open_delim(pre: &str) -> Option<char> {
    let mut stack: Vec<char> = Vec::new();
    for c in pre.chars() {
        match c {
            '(' | '[' | '{' => stack.push(c),
            ')' if stack.last() == Some(&'(') => {
                stack.pop();
            }
            ']' if stack.last() == Some(&'[') => {
                stack.pop();
            }
            '}' if stack.last() == Some(&'{') => {
                stack.pop();
            }
            _ => {}
        }
    }
    stack.last().copied()
}

/// The slice of language families the token-type classifier reasons about.
///
/// Everything outside these families falls into [`TokenLang::Other`], where we
/// still recognize comments / strings / identifiers but skip language-specific
/// shapes (CSS properties, JSX `data-*`, Rust roles).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenLang {
    Rust,
    /// TS/TSX/JS/JSX and friends.
    Ts,
    /// CSS/SCSS/SASS/LESS.
    Css,
    Other,
}

/// Map a snapshot path to a [`TokenLang`] by file extension.
fn lang_of_path(path: &str) -> TokenLang {
    let ext = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.').map(|(_, e)| e))
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => TokenLang::Rust,
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => TokenLang::Ts,
        "css" | "scss" | "sass" | "less" => TokenLang::Css,
        _ => TokenLang::Other,
    }
}

/// Hand-authored source-language extensions. Used as the `production` fallback
/// for files that match no test/docs/config/generated convention. Kept broad
/// across the languages loctree indexes so non-Rust/TS sources (Python, Go,
/// Ruby, JVM, native, scripting, …) get an honest `production` bucket instead
/// of collapsing into `unknown`. Anything outside this set stays `unknown` so
/// the bucket remains a truthful signal, never a guess.
fn is_source_extension(ext: &str) -> bool {
    matches!(
        ext,
        // Rust + web (already `production` under the prior TokenLang match).
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "vue"
            | "svelte"
            | "astro"
            // Python.
            | "py"
            | "pyi"
            | "pyx"
            // Go / Ruby / PHP.
            | "go"
            | "rb"
            | "rake"
            | "php"
            // JVM family.
            | "java"
            | "kt"
            | "kts"
            | "scala"
            | "groovy"
            | "clj"
            | "cljs"
            // Native.
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "cxx"
            | "hpp"
            | "hh"
            | "m"
            | "mm"
            // .NET / Swift / functional / systems.
            | "cs"
            | "fs"
            | "swift"
            | "dart"
            | "lua"
            | "ex"
            | "exs"
            | "erl"
            | "hs"
            | "ml"
            | "mli"
            | "nim"
            | "zig"
            | "jl"
            // Shell / scripting — hand-authored runtime logic.
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "ps1"
            // Schema / query that carries runtime logic.
            | "sql"
            | "graphql"
            | "gql"
            | "proto"
    )
}

fn scope_classification(path: &str) -> ScopeClassification {
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(lower.as_str());
    let ext = file.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    // Slash-prefixed copy so a leading path segment (repo-root `tests/`,
    // `build/`, `dist/`, `docs/`, `.github/`, …) matches the same `/segment/`
    // checks as a nested one. Python/JS repos very commonly place these at the
    // repo root, where the bare `contains("/tests/")` form would miss them.
    let slashed = format!("/{lower}");

    if slashed.contains("/dist/")
        || slashed.contains("/build/")
        || slashed.contains("/target/")
        || slashed.contains("/generated/")
        || slashed.contains("/gen/")
        || slashed.contains("/__pycache__/")
        || slashed.contains("/node_modules/")
        || slashed.contains(".egg-info/")
        || file.contains(".gen.")
        || file.contains(".generated.")
        || file.ends_with(".min.js")
        || file.ends_with(".min.css")
        || file.ends_with(".pyc")
        || file.ends_with("_pb2.py")
        || file.ends_with("_pb2.pyi")
    {
        return ScopeClassification::Generated;
    }
    if slashed.contains("/docs/")
        || file == "readme.md"
        || file == "changelog.md"
        || file.ends_with(".md")
        || file.ends_with(".mdx")
        || file.ends_with(".rst")
    {
        return ScopeClassification::Docs;
    }
    if slashed.contains("/test/")
        || slashed.contains("/tests/")
        || slashed.contains("/__tests__/")
        || slashed.contains("/fixtures/")
        || file.contains(".test.")
        || file.contains(".spec.")
        || file.ends_with("_test.rs")
        // Python (pytest/unittest) — `test_*.py`, `*_test.py`, `conftest.py`.
        || file == "conftest.py"
        || file.ends_with("_test.py")
        || (file.starts_with("test_") && file.ends_with(".py"))
        // Go / Ruby unit conventions.
        || file.ends_with("_test.go")
        || file.ends_with("_test.rb")
        || file.ends_with("_spec.rb")
    {
        return ScopeClassification::Test;
    }
    if slashed.contains("/.github/")
        || slashed.contains("/.loctree/")
        || file.starts_with('.')
        || matches!(
            file,
            "cargo.toml"
                | "cargo.lock"
                | "package.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
                | "package-lock.json"
                | "tsconfig.json"
                | "vite.config.ts"
                | "makefile"
                // Python project / build metadata.
                | "setup.py"
                | "setup.cfg"
                | "tox.ini"
                | "pipfile"
                | "pipfile.lock"
                | "poetry.lock"
                | "manifest.in"
                | "dockerfile"
        )
        || file.ends_with(".toml")
        || file.ends_with(".yaml")
        || file.ends_with(".yml")
        || file.ends_with(".json")
        || file.ends_with(".lock")
        || file.ends_with(".ini")
        || file.ends_with(".cfg")
        || (file.starts_with("requirements") && file.ends_with(".txt"))
    {
        return ScopeClassification::Config;
    }

    // Production fallback is language-aware: any recognized source extension is
    // `production`; genuinely unrecognized kinds stay `unknown` so the bucket
    // never overclaims. This is what keeps Python/Go/JVM/native sources from
    // collapsing into `unknown` the way the prior Rust/TS/CSS-only match did.
    if is_source_extension(ext) {
        ScopeClassification::Production
    } else {
        ScopeClassification::Unknown
    }
}

/// Lexical state of a prefix string at the point the identifier begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lex {
    Code,
    Str,
    LineComment,
    BlockComment,
}

/// Walk `prefix` (everything on the line *before* the identifier) and report the
/// lexical state at its end. A tiny single-line lexer — it tracks string and
/// comment regions so a hit inside `// …`, `/* … */`, or `"…"` is recognized
/// without the AST. `line_comment` enables `//` line comments (off for CSS).
fn lex_state(prefix: &str, line_comment: bool) -> Lex {
    let chars: Vec<char> = prefix.chars().collect();
    let mut state = Lex::Code;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match state {
            Lex::Code => {
                if line_comment && c == '/' && chars.get(i + 1) == Some(&'/') {
                    state = Lex::LineComment;
                    i += 2;
                    continue;
                }
                if c == '/' && chars.get(i + 1) == Some(&'*') {
                    state = Lex::BlockComment;
                    i += 2;
                    continue;
                }
                if c == '"' || c == '\'' || c == '`' {
                    state = Lex::Str;
                }
            }
            Lex::Str => {
                if c == '\\' {
                    i += 2;
                    continue;
                }
                // Any closing quote returns to code; good enough for single-line
                // classification (we never need to track *which* quote opened).
                if c == '"' || c == '\'' || c == '`' {
                    state = Lex::Code;
                }
            }
            Lex::BlockComment => {
                if c == '*' && chars.get(i + 1) == Some(&'/') {
                    state = Lex::Code;
                    i += 2;
                    continue;
                }
            }
            Lex::LineComment => {}
        }
        i += 1;
    }
    state
}

/// True when `s` is a clean identifier token (`[A-Za-z0-9_]+`, not all digits).
/// Used to decide whether the honest fallback is `identifier` or `unknown`.
fn is_identifier(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_ident_char) && !s.chars().all(|c| c.is_ascii_digit())
}

/// CSS-specific token shapes, given the prefix/suffix around the match.
fn classify_css(prefix: &str, suffix: &str) -> Option<OccurrenceKind> {
    // Trailing run of `[-A-Za-z0-9_]` is the hyphenated lead glued to our token.
    let lead: String = prefix
        .chars()
        .rev()
        .take_while(|c| is_ident_char(*c) || *c == '-')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let before_lead = prefix[..prefix.len() - lead.len()].chars().next_back();

    // `--token` / `var(--token)` / inside a `--…-token` custom-property name.
    if lead.starts_with("--") || prefix.ends_with("--") {
        return Some(OccurrenceKind::CustomProperty);
    }
    // `.token` / `.overlay-token` selector.
    if before_lead == Some('.') {
        return Some(OccurrenceKind::ClassToken);
    }
    // `token:` declaration property (value side stays identifier).
    if suffix.trim_start().starts_with(':') {
        return Some(OccurrenceKind::CssProperty);
    }
    None
}

/// Language-aware single-line classification of one literal occurrence.
///
/// Priority: comment → string → language-specific shapes → identifier fallback.
/// Rust additionally consults the conservative role classifier
/// ([`classify_occurrence`]) before falling back to `identifier`. `unknown` is
/// reserved for the genuinely unclassifiable (e.g. a hyphen/dot query that is
/// not a clean identifier and matches no language shape).
fn classify_token(lang: TokenLang, line: &str, ident: &str, col_1based: usize) -> OccurrenceKind {
    let chars: Vec<char> = line.chars().collect();
    let start = col_1based.saturating_sub(1);
    let ident_len = ident.chars().count();
    if ident_len == 0 || start + ident_len > chars.len() {
        return OccurrenceKind::Unknown;
    }
    let prefix: String = chars[..start].iter().collect();
    let suffix: String = chars[start + ident_len..].iter().collect();

    let line_comment = !matches!(lang, TokenLang::Css);
    match lex_state(&prefix, line_comment) {
        Lex::LineComment | Lex::BlockComment => return OccurrenceKind::Comment,
        Lex::Str => {
            // A class/utility token inside a `class=`/`className=` attribute is a
            // class_token (Tailwind/utility); any other string hit is a string
            // literal.
            if matches!(lang, TokenLang::Ts) && prefix.contains("class") {
                return OccurrenceKind::ClassToken;
            }
            return OccurrenceKind::StringLiteral;
        }
        Lex::Code => {}
    }

    match lang {
        TokenLang::Css => {
            if let Some(kind) = classify_css(&prefix, &suffix) {
                return kind;
            }
        }
        TokenLang::Ts => {
            if prefix.ends_with("data-") {
                return OccurrenceKind::DataAttribute;
            }
        }
        TokenLang::Rust => {
            let role = classify_occurrence(line, ident, col_1based);
            if role != OccurrenceKind::Unknown {
                return role;
            }
        }
        TokenLang::Other => {}
    }

    if is_identifier(ident) {
        OccurrenceKind::Identifier
    } else {
        OccurrenceKind::Unknown
    }
}

fn rust_import_line_flags(text: &str) -> Vec<bool> {
    let mut flags = Vec::new();
    let mut in_use = false;
    for raw_line in text.lines() {
        let trimmed = raw_line.trim_start();
        let starts_use = trimmed.starts_with("use ") || trimmed.starts_with("pub use ");
        if starts_use {
            in_use = true;
        }
        flags.push(in_use);
        if in_use && trimmed.contains(';') {
            in_use = false;
        }
    }
    flags
}

fn derive_match_role(
    occurrence_kind: OccurrenceKind,
    line: &str,
    ident: &str,
    col_1based: usize,
) -> MatchRole {
    if occurrence_kind != OccurrenceKind::Identifier {
        return MatchRole::from_occurrence_kind(occurrence_kind);
    }

    let chars: Vec<char> = line.chars().collect();
    let start = col_1based.saturating_sub(1);
    let ident_len = ident.chars().count();
    if ident_len == 0 || start + ident_len > chars.len() {
        return MatchRole::Reference;
    }

    let prefix: String = chars[..start].iter().collect();
    let previous_token = prefix
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .rfind(|token| !token.is_empty());

    if matches!(
        previous_token,
        Some("fn" | "struct" | "enum" | "trait" | "type" | "mod" | "const" | "static")
    ) {
        MatchRole::Definition
    } else {
        MatchRole::Reference
    }
}

/// Scan a single file's text for literal identifier occurrences (default
/// boundary). Thin wrapper over [`scan_text_with`].
/// Coverage line for literal/regex scans. Since W2-02 the artifact fence
/// classifies instead of excluding, so the line reports `artifact-flagged:`
/// buckets — deliberately NOT the shared `excluded:` summary shape, which
/// still means real exclusion in diff/watch/analysis fences.
fn coverage_line_for(
    files_scanned: usize,
    files_in_universe: usize,
    stats: &crate::analyzer::classify::ArtifactFenceStats,
) -> String {
    let mut line = format!("scanned {files_scanned} of {files_in_universe} repo files");
    let mut parts = Vec::new();
    if stats.vendored > 0 {
        parts.push(format!("vendored({})", stats.vendored));
    }
    if stats.fixtures > 0 {
        parts.push(format!("fixtures({})", stats.fixtures));
    }
    if stats.generated > 0 {
        parts.push(format!("generated({})", stats.generated));
    }
    if stats.templates > 0 {
        parts.push(format!("templates({})", stats.templates));
    }
    if !parts.is_empty() {
        line.push_str("; artifact-flagged: ");
        line.push_str(&parts.join(", "));
    }
    line
}

/// Longest context line emitted verbatim. Anything longer (minified bundles,
/// generated one-liners — now scanned instead of fenced out) is windowed
/// around the match so one vendored hit cannot balloon the payload.
const MAX_CONTEXT_CHARS: usize = 240;
/// Chars kept on each side of the match when a long line is windowed.
const CONTEXT_WINDOW_CHARS: usize = 100;

/// Trimmed context line, windowed around the match when the raw line exceeds
/// [`MAX_CONTEXT_CHARS`]. `col` is the 1-based char column of the match start.
fn context_snippet(raw_line: &str, col: usize, match_chars: usize) -> String {
    let trimmed = raw_line.trim_end();
    let total = trimmed.chars().count();
    if total <= MAX_CONTEXT_CHARS {
        return trimmed.to_string();
    }
    let match_start = col.saturating_sub(1);
    let start = match_start.saturating_sub(CONTEXT_WINDOW_CHARS);
    let end = (match_start + match_chars + CONTEXT_WINDOW_CHARS).min(total);
    let window: String = trimmed.chars().skip(start).take(end - start).collect();
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < total { "…" } else { "" };
    format!("{prefix}{window}{suffix}")
}

pub fn scan_text(path: &str, text: &str, ident: &str) -> Vec<LiteralOccurrence> {
    scan_text_with(path, text, ident, ScanOptions::default())
}

/// [`scan_text`] with explicit [`ScanOptions`]. Occurrence classification is
/// language-aware, derived from `path`'s extension.
pub fn scan_text_with(
    path: &str,
    text: &str,
    ident: &str,
    opts: ScanOptions,
) -> Vec<LiteralOccurrence> {
    let lang = lang_of_path(path);
    let scope_classification = scope_classification(path);
    let rust_import_lines = (lang == TokenLang::Rust).then(|| rust_import_line_flags(text));
    let mut out = Vec::new();
    for (idx, raw_line) in text.lines().enumerate() {
        for col in occurrences_in_line_with(raw_line, ident, opts.whole_token) {
            let line = idx + 1;
            let mut occurrence_kind = classify_token(lang, raw_line, ident, col);
            if rust_import_lines
                .as_ref()
                .and_then(|flags| flags.get(idx))
                .copied()
                .unwrap_or(false)
            {
                occurrence_kind = OccurrenceKind::ImportLike;
            }
            let match_role = derive_match_role(occurrence_kind, raw_line, ident, col);
            out.push(LiteralOccurrence {
                file: path.to_string(),
                line,
                column: col,
                range: occurrence_range(line, col, ident),
                matched_text: ident.to_string(),
                context: context_snippet(raw_line, col, ident.chars().count()),
                source: "literal",
                occurrence_kind,
                match_role,
                confidence: match_confidence(occurrence_kind, match_role),
                scope_classification,
                enclosing_symbol: None,
                resolved_definition: None,
                definition_candidates: Vec::new(),
            });
        }
    }
    out
}

/// Scan a set of (path, content) pairs for literal occurrences of `ident`
/// (default boundary). Thin wrapper over [`scan_files_with`].
///
/// Results are sorted by `(file, line, column)` for deterministic output.
pub fn scan_files<'a, I>(files: I, ident: &str) -> OccurrenceResults
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    scan_files_with(files, ident, ScanOptions::default())
}

/// [`scan_files`] with explicit [`ScanOptions`] (e.g. `whole_token`). The
/// returned [`OccurrenceResults`] always carries the full occurrence list and
/// `total`; call [`OccurrenceResults::apply_report`] afterwards for
/// `group_by_file` / `count_only` shaping.
pub fn scan_files_with<'a, I>(files: I, ident: &str, opts: ScanOptions) -> OccurrenceResults
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    scan_files_with_scope(files, ident, opts, FileScope::default())
}

/// [`scan_files_with`] with an optional exact file/path scope. The scanner
/// stays literal-only; file scoping only selects which snapshot paths are fed
/// into it.
pub fn scan_files_with_scope<'a, I>(
    files: I,
    ident: &str,
    opts: ScanOptions,
    scope: FileScope<'_>,
) -> OccurrenceResults
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut occurrences = Vec::new();
    let mut files_scanned = 0;
    let mut stats = crate::analyzer::classify::ArtifactFenceStats::default();

    for (path, content) in files {
        if !scope.matches(path) {
            continue;
        }
        // Literal truth: artifact-classed files (fixtures, vendored, generated,
        // templates) are SCANNED like everything else — skipping them made
        // `--literal` under-report versus rg while still claiming trustworthy
        // absence (W2-02 scorecard correctness loss). The class tally survives
        // as accounting so the coverage line still tells the noise story.
        let class = crate::analyzer::classify::artifact_class(path, Some(content));
        if class.is_artifact() {
            stats.record(class);
        }
        files_scanned += 1;
        occurrences.extend(scan_text_with(path, content, ident, opts));
    }
    occurrences.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.column.cmp(&b.column))
    });

    let mut seen = std::collections::BTreeSet::new();
    for occ in &occurrences {
        seen.insert(occ.file.clone());
    }

    let files_in_universe = files_scanned;
    let coverage_line = coverage_line_for(files_scanned, files_in_universe, &stats);

    let total = occurrences.len();
    let scope_classifications = scope_classification_counts(&occurrences);
    let suggested_next = suggested_next(ident, &occurrences);
    let role = role_summary(&occurrences);
    OccurrenceResults {
        query: ident.to_string(),
        query_kind: query_kind(ident),
        match_mode: match_mode(ident, opts.whole_token),
        files_matched: seen.len(),
        total,
        occurrences,
        by_file: None,
        slim: false,
        page: None,
        source: "literal",
        coverage_line,
        scope: Some(LiteralScopeStats {
            files_in_universe,
            files_scanned,
            vendored: stats.vendored,
            fixtures: stats.fixtures,
            generated: stats.generated,
            templates: stats.templates,
        }),
        scope_classifications,
        suggested_next,
        near_matches: Vec::new(),
        role_summary: role,
        file_context: Vec::new(),
    }
}

/// Scan a single file's text for regex matches. Each non-overlapping match on a
/// line becomes one occurrence; `matched_text` is the ACTUAL matched substring
/// (not the pattern). The context label (`comment` / `string_literal` / `code`)
/// is derived exactly like the literal scanner so a privacy/secret audit can
/// tell a live hit from a commented-out one. Line-based, like the literal scan —
/// patterns do not match across newlines.
pub fn scan_text_regex(path: &str, text: &str, re: &regex::Regex) -> Vec<LiteralOccurrence> {
    let lang = lang_of_path(path);
    let scope_classification = scope_classification(path);
    let mut out = Vec::new();
    for (idx, raw_line) in text.lines().enumerate() {
        for m in re.find_iter(raw_line) {
            let line = idx + 1;
            // 1-based char column where the match starts.
            let col = raw_line[..m.start()].chars().count() + 1;
            let matched = m.as_str();
            if matched.is_empty() {
                continue; // never emit zero-width matches (e.g. `a*` on empty span)
            }
            let occurrence_kind = classify_token(lang, raw_line, matched, col);
            let match_role = derive_match_role(occurrence_kind, raw_line, matched, col);
            out.push(LiteralOccurrence {
                file: path.to_string(),
                line,
                column: col,
                range: occurrence_range(line, col, matched),
                matched_text: matched.to_string(),
                context: context_snippet(raw_line, col, matched.chars().count()),
                source: "regex",
                occurrence_kind,
                match_role,
                confidence: match_confidence(occurrence_kind, match_role),
                scope_classification,
                enclosing_symbol: None,
                resolved_definition: None,
                definition_candidates: Vec::new(),
            });
        }
    }
    out
}

/// Scan a set of (path, content) pairs for regex matches, sharing the artifact
/// fence + coverage accounting of [`scan_files_with_scope`]. This is what lets a
/// `find --regex` privacy/secret audit report "scanned N of M files; excluded:
/// generated(…)" — the accounting the raw `grep`/`sed` fallback could never give.
/// Unlike literal mode, a successfully-compiled-and-run pattern makes absence
/// trustworthy: the pattern WAS evaluated as a pattern.
pub fn scan_files_with_regex<'a, I>(
    files: I,
    re: &regex::Regex,
    scope: FileScope<'_>,
) -> OccurrenceResults
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut occurrences = Vec::new();
    let mut files_scanned = 0;
    let mut stats = crate::analyzer::classify::ArtifactFenceStats::default();

    for (path, content) in files {
        if !scope.matches(path) {
            continue;
        }
        // Same truth contract as the literal scan: artifact-classed files are
        // scanned and only tallied, never skipped — `regex_trust.absence_trustworthy`
        // is a lie if 180 fixture files were silently left out.
        let class = crate::analyzer::classify::artifact_class(path, Some(content));
        if class.is_artifact() {
            stats.record(class);
        }
        files_scanned += 1;
        occurrences.extend(scan_text_regex(path, content, re));
    }
    occurrences.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.column.cmp(&b.column))
    });

    let mut seen = std::collections::BTreeSet::new();
    for occ in &occurrences {
        seen.insert(occ.file.clone());
    }

    let files_in_universe = files_scanned;
    let coverage_line = coverage_line_for(files_scanned, files_in_universe, &stats);

    let pattern = re.as_str().to_string();
    let total = occurrences.len();
    let scope_classifications = scope_classification_counts(&occurrences);
    let suggested_next = suggested_next(&pattern, &occurrences);
    let role = role_summary(&occurrences);
    OccurrenceResults {
        query: pattern,
        query_kind: QueryKind::Symbolic,
        match_mode: MatchMode::Regex,
        files_matched: seen.len(),
        total,
        occurrences,
        by_file: None,
        slim: false,
        page: None,
        source: "regex",
        coverage_line,
        scope: Some(LiteralScopeStats {
            files_in_universe,
            files_scanned,
            vendored: stats.vendored,
            fixtures: stats.fixtures,
            generated: stats.generated,
            templates: stats.templates,
        }),
        scope_classifications,
        suggested_next,
        near_matches: Vec::new(),
        role_summary: role,
        file_context: Vec::new(),
    }
}

/// Add AST/snapshot awareness to literal results without changing the literal
/// occurrence set.
pub fn enrich_with_snapshot(results: &mut OccurrenceResults, snapshot: &Snapshot) {
    if results.occurrences.is_empty() || results.query.is_empty() {
        return;
    }

    let index = SnapshotOccurrenceIndex::new(snapshot, &results.query, &results.occurrences);
    for occ in &mut results.occurrences {
        occ.definition_candidates = index.definition_candidates.clone();
        occ.enclosing_symbol = index.enclosing_symbol(occ);
        occ.resolved_definition = index.resolve_occurrence(occ);
    }
    results.file_context = file_contexts(snapshot, &results.occurrences);
    results.suggested_next = suggested_next(&results.query, &results.occurrences);
}

pub fn attach_near_matches(results: &mut OccurrenceResults, analyses: &[FileAnalysis]) {
    if results.total == 0 {
        results.near_matches = near_symbol_matches(&results.query, analyses);
    }
}

const MAX_NEAR_MATCHES: usize = 8;

pub fn near_symbol_matches(query: &str, analyses: &[FileAnalysis]) -> Vec<NearMatch> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let query_lower = query.to_ascii_lowercase();
    let mut matches = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    for analysis in analyses {
        for export in &analysis.exports {
            push_near_match(
                &mut matches,
                &mut seen,
                &query_lower,
                &export.name,
                &analysis.path,
                export.line,
                &export.kind,
            );
        }
        for local in &analysis.local_symbols {
            push_near_match(
                &mut matches,
                &mut seen,
                &query_lower,
                &local.name,
                &analysis.path,
                local.line,
                &local.kind,
            );
        }
    }

    matches.sort_by(|a, b| {
        match_rank(a.match_kind)
            .cmp(&match_rank(b.match_kind))
            .then(a.symbol.len().cmp(&b.symbol.len()))
            .then(a.symbol.cmp(&b.symbol))
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    matches.truncate(MAX_NEAR_MATCHES);
    matches
}

fn push_near_match(
    matches: &mut Vec<NearMatch>,
    seen: &mut std::collections::BTreeSet<(String, String, Option<usize>, String)>,
    query_lower: &str,
    symbol: &str,
    file: &str,
    line: Option<usize>,
    kind: &str,
) {
    let symbol_lower = symbol.to_ascii_lowercase();
    if symbol_lower == query_lower {
        return;
    }
    let match_kind = if symbol_lower.starts_with(query_lower) {
        "prefix"
    } else if symbol_lower.contains(query_lower) {
        "substring"
    } else if is_fuzzy_near_match(query_lower, &symbol_lower) {
        "fuzzy"
    } else {
        return;
    };
    let key = (symbol.to_string(), file.to_string(), line, kind.to_string());
    if !seen.insert(key) {
        return;
    }
    matches.push(NearMatch {
        symbol: symbol.to_string(),
        file: file.to_string(),
        line,
        kind: kind.to_string(),
        match_kind,
        source: "symbol_table",
    });
}

fn match_rank(kind: &str) -> u8 {
    match kind {
        "prefix" => 0,
        "substring" => 1,
        "fuzzy" => 2,
        _ => 3,
    }
}

fn is_fuzzy_near_match(query_lower: &str, symbol_lower: &str) -> bool {
    if query_lower.len() < 4 {
        return false;
    }
    let distance = levenshtein(query_lower, symbol_lower);
    let max_distance = if query_lower.len() >= 12 { 3 } else { 2 };
    distance > 0 && distance <= max_distance
}

/// Maximum number of hit-carrying files we attach importer/consumer context for.
const MAX_FILE_CONTEXT_FILES: usize = 25;
/// Maximum entries kept per `imported_by`/`imports` list before truncation.
const MAX_FILE_CONTEXT_EDGES: usize = 12;

/// Build per-file importer/consumer context from the snapshot's canonical import
/// edges. Every hit-carrying file is emitted — files without a proven edge
/// (docs, resources, scan-only surfaces) still contribute hit count and scope
/// classification, which is real context for a prose/doc query (W2-02 lift
/// loss: prose hits lived only in edge-less files, so `file_context` came back
/// empty). Lists are capped for token economy. The edge graph here is the
/// exact same `snapshot.edges` adjacency `loct slice`/`impact` read, so the
/// literal surface never disagrees with the structural one.
fn file_contexts(snapshot: &Snapshot, occurrences: &[LiteralOccurrence]) -> Vec<FileContext> {
    // Hit-carrying files in first-seen order, with per-file counts + scope.
    let mut order: Vec<String> = Vec::new();
    let mut hits: std::collections::HashMap<String, (usize, ScopeClassification)> =
        std::collections::HashMap::new();
    for occ in occurrences {
        let entry = hits.entry(occ.file.clone()).or_insert_with(|| {
            order.push(occ.file.clone());
            (0, occ.scope_classification)
        });
        entry.0 += 1;
    }

    // Forward (dependencies) and reverse (consumers) adjacency, keyed by a
    // normalized path so `./`-prefixed and back-slashed edge paths still match
    // the occurrence file paths.
    let norm = |p: &str| p.trim_start_matches("./").replace('\\', "/");
    let mut imported_by: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut imports: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for edge in &snapshot.edges {
        imported_by
            .entry(norm(&edge.to))
            .or_default()
            .push(edge.from.clone());
        imports
            .entry(norm(&edge.from))
            .or_default()
            .push(edge.to.clone());
    }

    let mut out = Vec::new();
    for file in order {
        if out.len() >= MAX_FILE_CONTEXT_FILES {
            break;
        }
        let key = norm(&file);
        let (hit_count, scope) = hits
            .get(&file)
            .copied()
            .unwrap_or((0, ScopeClassification::Unknown));
        let mut consumers = imported_by.get(&key).cloned().unwrap_or_default();
        let mut deps = imports.get(&key).cloned().unwrap_or_default();
        consumers.sort();
        consumers.dedup();
        deps.sort();
        deps.dedup();
        let truncated =
            consumers.len() > MAX_FILE_CONTEXT_EDGES || deps.len() > MAX_FILE_CONTEXT_EDGES;
        consumers.truncate(MAX_FILE_CONTEXT_EDGES);
        deps.truncate(MAX_FILE_CONTEXT_EDGES);
        out.push(FileContext {
            file,
            scope_classification: scope,
            hits: hit_count,
            imported_by: consumers,
            imports: deps,
            truncated,
        });
    }
    out
}

struct SnapshotOccurrenceIndex {
    query: String,
    definition_candidates: Vec<SymbolAnchor>,
    local_bindings: std::collections::HashMap<String, Vec<SymbolAnchor>>,
    imports: std::collections::HashMap<String, SymbolAnchor>,
    enclosing: std::collections::HashMap<String, Vec<SymbolAnchor>>,
}

impl SnapshotOccurrenceIndex {
    fn new(snapshot: &Snapshot, query: &str, occurrences: &[LiteralOccurrence]) -> Self {
        let mut definition_candidates = Vec::new();
        let mut enclosing: std::collections::HashMap<String, Vec<SymbolAnchor>> =
            std::collections::HashMap::new();

        for file in &snapshot.files {
            let mut file_symbols = Vec::new();
            for export in &file.exports {
                let anchor = SymbolAnchor {
                    name: export.name.clone(),
                    file: file.path.clone(),
                    line: export.line,
                    kind: export.kind.clone(),
                    symbol_id: crate::types::SymbolIdV1::from_parts(&file.path, &export.name)
                        .as_str()
                        .to_string(),
                };
                if export.name == query {
                    definition_candidates.push(anchor.clone());
                }
                file_symbols.push(anchor);
            }
            for local in &file.local_symbols {
                let anchor = SymbolAnchor {
                    name: local.name.clone(),
                    file: file.path.clone(),
                    line: local.line,
                    kind: local.kind.clone(),
                    symbol_id: format!(
                        "{}::local::{}::{}",
                        file.path,
                        local.name,
                        local.line.unwrap_or(0)
                    ),
                };
                file_symbols.push(anchor);
            }
            file_symbols.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.name.cmp(&b.name)));
            enclosing.insert(file.path.clone(), file_symbols);
        }

        definition_candidates.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.kind.cmp(&b.kind))
        });
        definition_candidates
            .dedup_by(|a, b| a.file == b.file && a.line == b.line && a.name == b.name);

        let mut local_bindings: std::collections::HashMap<String, Vec<SymbolAnchor>> =
            std::collections::HashMap::new();
        for occ in occurrences {
            if occ.match_role == MatchRole::LocalBinding {
                local_bindings
                    .entry(occ.file.clone())
                    .or_default()
                    .push(SymbolAnchor {
                        name: occ.matched_text.clone(),
                        file: occ.file.clone(),
                        line: Some(occ.line),
                        kind: "local_binding".to_string(),
                        symbol_id: format!(
                            "{}::local::{}::{}:{}",
                            occ.file, occ.matched_text, occ.line, occ.column
                        ),
                    });
            }
        }
        for bindings in local_bindings.values_mut() {
            bindings.sort_by(|a, b| {
                a.line
                    .cmp(&b.line)
                    .then_with(|| a.symbol_id.cmp(&b.symbol_id))
            });
        }

        let mut imports = std::collections::HashMap::new();
        for file in &snapshot.files {
            for import in &file.imports {
                if !import_mentions(import, query) {
                    continue;
                }
                if let Some(def) = resolve_import_definition(snapshot, file, import, query) {
                    imports.entry(file.path.clone()).or_insert(def);
                }
            }
        }

        Self {
            query: query.to_string(),
            definition_candidates,
            local_bindings,
            imports,
            enclosing,
        }
    }

    fn enclosing_symbol(&self, occ: &LiteralOccurrence) -> Option<SymbolAnchor> {
        let nearest = self.enclosing.get(&occ.file).and_then(|symbols| {
            symbols
                .iter()
                .rfind(|symbol| symbol.line.is_some_and(|line| line <= occ.line))
                .cloned()
        });
        nearest.or_else(|| {
            Some(SymbolAnchor {
                name: occ.file.clone(),
                file: occ.file.clone(),
                line: None,
                kind: "file".to_string(),
                symbol_id: occ.file.clone(),
            })
        })
    }

    fn resolve_occurrence(&self, occ: &LiteralOccurrence) -> Option<SymbolAnchor> {
        match occ.match_role {
            MatchRole::Definition => self
                .definition_candidates
                .iter()
                .find(|def| def.file == occ.file && def.line == Some(occ.line))
                .cloned(),
            MatchRole::LocalBinding => None,
            MatchRole::Import => self.imports.get(&occ.file).cloned(),
            MatchRole::Reference => self
                .local_binding_before(occ)
                .or_else(|| self.imports.get(&occ.file).cloned())
                .or_else(|| {
                    self.definition_candidates
                        .iter()
                        .find(|def| def.file == occ.file)
                        .cloned()
                })
                .or_else(|| {
                    (self.definition_candidates.len() == 1)
                        .then(|| self.definition_candidates.first().cloned())
                        .flatten()
                }),
            _ => None,
        }
    }

    fn local_binding_before(&self, occ: &LiteralOccurrence) -> Option<SymbolAnchor> {
        self.local_bindings.get(&occ.file).and_then(|bindings| {
            bindings
                .iter()
                .rfind(|binding| {
                    binding.name == self.query && binding.line.is_some_and(|line| line < occ.line)
                })
                .cloned()
        })
    }
}

fn import_mentions(import: &ImportEntry, query: &str) -> bool {
    import.symbols.iter().any(|symbol| {
        symbol.name == query || symbol.alias.as_deref().is_some_and(|alias| alias == query)
    }) || import.source.ends_with(query)
        || import.raw_path.contains(query)
        || import.source_raw.contains(query)
}

fn resolve_import_definition(
    snapshot: &Snapshot,
    file: &FileAnalysis,
    import: &ImportEntry,
    query: &str,
) -> Option<SymbolAnchor> {
    let target_path = import
        .resolved_path
        .as_deref()
        .and_then(|path| snapshot_path_for(snapshot, path))
        .or_else(|| rust_import_target_path(snapshot, &file.path, import));
    let target_path = target_path?;
    export_anchor(snapshot, &target_path, query)
        .or_else(|| reexport_anchor(snapshot, &target_path, query))
        .or_else(|| unique_module_export_anchor(snapshot, &target_path, query))
}

fn export_anchor(snapshot: &Snapshot, path: &str, query: &str) -> Option<SymbolAnchor> {
    snapshot.files.iter().find_map(|candidate| {
        (candidate.path == path).then(|| {
            candidate
                .exports
                .iter()
                .find(|export| export.name == query)
                .map(|export| SymbolAnchor {
                    name: export.name.clone(),
                    file: candidate.path.clone(),
                    line: export.line,
                    kind: export.kind.clone(),
                    symbol_id: crate::types::SymbolIdV1::from_parts(&candidate.path, &export.name)
                        .as_str()
                        .to_string(),
                })
        })?
    })
}

fn reexport_anchor(snapshot: &Snapshot, target_path: &str, query: &str) -> Option<SymbolAnchor> {
    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == target_path)?;
    for reexport in &file.reexports {
        match &reexport.kind {
            ReexportKind::Star => {
                let source_path = reexport
                    .resolved
                    .as_deref()
                    .and_then(|path| snapshot_path_for(snapshot, path))
                    .or_else(|| rust_source_target_path(snapshot, &file.path, &reexport.source));
                if let Some(source_path) = source_path
                    && let Some(anchor) = export_anchor(snapshot, &source_path, query)
                {
                    return Some(anchor);
                }
            }
            ReexportKind::Named(names) => {
                let Some((original, _exported)) = names
                    .iter()
                    .find(|(original, exported)| original == query || exported == query)
                else {
                    continue;
                };
                let source_path = reexport
                    .resolved
                    .as_deref()
                    .and_then(|path| snapshot_path_for(snapshot, path))
                    .or_else(|| {
                        rust_reexport_named_source_target_path(
                            snapshot,
                            &file.path,
                            &reexport.source,
                            original,
                        )
                    });
                if let Some(source_path) = source_path
                    && let Some(anchor) = export_anchor(snapshot, &source_path, original)
                {
                    return Some(anchor);
                }
            }
        }
    }
    None
}

fn unique_module_export_anchor(
    snapshot: &Snapshot,
    target_path: &str,
    query: &str,
) -> Option<SymbolAnchor> {
    let module_dir = module_dir_for_path(target_path)?;
    let mut matches: Vec<_> = snapshot
        .files
        .iter()
        .filter(|file| file.path.starts_with(&module_dir) && file.path != target_path)
        .flat_map(|file| {
            file.exports
                .iter()
                .filter(move |export| export.name == query)
                .map(move |export| SymbolAnchor {
                    name: export.name.clone(),
                    file: file.path.clone(),
                    line: export.line,
                    kind: export.kind.clone(),
                    symbol_id: crate::types::SymbolIdV1::from_parts(&file.path, &export.name)
                        .as_str()
                        .to_string(),
                })
        })
        .collect();
    matches.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)));
    matches.dedup_by(|a, b| a.file == b.file && a.line == b.line && a.name == b.name);
    (matches.len() == 1).then(|| matches.remove(0))
}

fn module_dir_for_path(path: &str) -> Option<String> {
    let normalized = normalize_scope_path(path);
    if let Some(dir) = normalized.strip_suffix("/mod.rs") {
        return Some(format!("{dir}/"));
    }
    normalized
        .strip_suffix(".rs")
        .map(|module| format!("{module}/"))
}

fn snapshot_path_for(snapshot: &Snapshot, path: &str) -> Option<String> {
    let normalized = normalize_scope_path(path);
    snapshot
        .files
        .iter()
        .find(|file| {
            normalize_scope_path(&file.path) == normalized || file.path.ends_with(&normalized)
        })
        .map(|file| file.path.clone())
}

fn rust_import_target_path(
    snapshot: &Snapshot,
    importing_file: &str,
    import: &ImportEntry,
) -> Option<String> {
    let source = if import.source.is_empty() {
        import.raw_path.as_str()
    } else {
        import.source.as_str()
    };
    rust_source_target_path(snapshot, importing_file, source)
}

fn rust_reexport_named_source_target_path(
    snapshot: &Snapshot,
    importing_file: &str,
    source: &str,
    original: &str,
) -> Option<String> {
    let mut source = source.trim();
    if source
        .rsplit("::")
        .next()
        .is_some_and(|last| last == original)
    {
        source = source
            .rsplit_once("::")
            .map(|(prefix, _)| prefix)
            .unwrap_or(source);
    }
    rust_source_target_path(snapshot, importing_file, source)
}

fn rust_source_target_path(
    snapshot: &Snapshot,
    importing_file: &str,
    source: &str,
) -> Option<String> {
    let mut source = source;
    if let Some((prefix, _)) = source.split_once('{') {
        source = prefix.trim_end_matches("::").trim();
    }
    source = source.trim_end_matches("::*");
    let source = source.trim().trim_end_matches(';').trim_end_matches("::");
    if source.is_empty() || source.starts_with("std::") || source.starts_with("core::") {
        return None;
    }

    let normalized_file = normalize_scope_path(importing_file);
    let mut base: Vec<&str> = normalized_file.split('/').collect();
    let file_name = base.pop().unwrap_or("");
    if file_name == "mod.rs" {
        // `mod.rs` already lives at the current module directory.
    }

    let mut segments: Vec<&str> = source.split("::").filter(|s| !s.is_empty()).collect();
    if segments.first() == Some(&"crate") {
        segments.remove(0);
        base.clear();
        base.push("src");
    } else {
        while segments.first() == Some(&"super") {
            segments.remove(0);
            if file_name == "mod.rs" {
                base.pop();
            }
        }
        if segments.first() == Some(&"self") {
            segments.remove(0);
        }
    }

    if segments.is_empty() {
        return None;
    }

    let mut module_path = base.join("/");
    if !module_path.is_empty() {
        module_path.push('/');
    }
    module_path.push_str(&segments.join("/"));
    let direct = format!("{module_path}.rs");
    let mod_rs = format!("{module_path}/mod.rs");
    snapshot_path_for(snapshot, &direct).or_else(|| snapshot_path_for(snapshot, &mod_rs))
}

fn normalize_scope_path(path: &str) -> String {
    path.trim().trim_start_matches("./").replace('\\', "/")
}

pub fn path_matches_scope(path: &str, scope: &str) -> bool {
    let path = normalize_scope_path(path);
    let scope = normalize_scope_path(scope);
    !scope.is_empty() && (path == scope || path.ends_with(&format!("/{scope}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_boundary_rejects_substring_matches() {
        // `id` must NOT match inside `utterance_id`, `valid`, or `id_count`.
        let line = "let utterance_id = valid_id_count;";
        assert!(
            occurrences_in_line(line, "id").is_empty(),
            "naive substring match leaked across identifier boundaries"
        );
    }

    #[test]
    fn phrase_literals_use_fixed_string_semantics() {
        let line = "status: snapshot fresh; previous snapshot stale";
        assert_eq!(occurrences_in_line(line, "snapshot fresh"), vec![9]);
        assert_eq!(occurrences_in_line(line, "snapshot stale"), vec![34]);
    }

    #[test]
    fn token_boundary_matches_whole_identifier() {
        let line = "let mut utterance_id = 0;";
        let cols = occurrences_in_line(line, "utterance_id");
        assert_eq!(cols, vec![9], "expected one boundary match at column 9");
    }

    #[test]
    fn matches_multiple_occurrences_on_one_line() {
        let line = "utterance_id = utterance_id + 1;";
        let cols = occurrences_in_line(line, "utterance_id");
        assert_eq!(cols, vec![1, 16]);
    }

    #[test]
    fn matches_with_operator_punctuation_boundary() {
        // `utterance_id += 1` — boundary is whitespace then `+`.
        let line = "        utterance_id += 1;";
        let cols = occurrences_in_line(line, "utterance_id");
        assert_eq!(cols, vec![9]);
    }

    #[test]
    fn ascii_fast_path_matches_unicode_fallback_columns() {
        assert_eq!(
            occurrences_in_line_with("alpha beta alpha", "alpha", false),
            vec![1, 12]
        );

        // Non-ASCII prefix forces the char-based fallback; the reported column
        // remains a 1-based char column, not a byte offset.
        assert_eq!(occurrences_in_line("zażółć alpha", "alpha"), vec![8]);
    }

    #[test]
    fn scan_text_finds_init_and_increment_and_field() {
        let text = "fn run() {\n    let mut utterance_id = 0;\n    utterance_id += 1;\n    Event { utterance_id };\n}\n";
        let occ = scan_text("src/lib.rs", text, "utterance_id");
        assert_eq!(occ.len(), 3, "init + increment + field emission");
        assert_eq!(occ[0].line, 2);
        assert_eq!(occ[1].line, 3);
        assert_eq!(occ[2].line, 4);
        assert_eq!(
            occ[0].range,
            OccurrenceRange {
                start: OccurrencePoint {
                    line: 2,
                    column: 13
                },
                end: OccurrencePoint {
                    line: 2,
                    column: 25
                }
            }
        );
        assert!(occ.iter().all(|o| o.source == "literal"));
        assert!(occ.iter().all(|o| o.matched_text == "utterance_id"));
        // Single-line classification rides along on every occurrence.
        assert_eq!(occ[0].occurrence_kind, OccurrenceKind::DefinitionLike);
        assert_eq!(occ[1].occurrence_kind, OccurrenceKind::MutationLike);
        assert_eq!(occ[2].occurrence_kind, OccurrenceKind::FieldEmitLike);
        assert_eq!(occ[0].match_role, MatchRole::LocalBinding);
        assert_eq!(occ[1].match_role, MatchRole::Mutation);
        assert_eq!(occ[2].match_role, MatchRole::FieldEmission);
        assert_eq!(occ[0].confidence, MatchConfidence::High);
        assert_eq!(occ[0].scope_classification, ScopeClassification::Production);
    }

    // ----- scope_classification: multi-language coarse bucketing -----
    //
    // Regression guard for the Python (and peer-language) underfeed: before the
    // fix, every non-Rust/TS/CSS source collapsed to `unknown`. These pin the
    // honest buckets across languages while keeping `unknown` strictly for
    // genuinely unrecognized file kinds.

    #[test]
    fn scope_classification_marks_python_sources_production() {
        for path in [
            "src/chat_generator.py",
            "app/models.py",
            "chat_generator.py",
            "pkg/service.go",
            "lib/widget.rb",
            "src/Main.java",
            "src/main.kt",
            "include/engine.hpp",
            "cmd/server/main.go",
        ] {
            assert_eq!(
                scope_classification(path),
                ScopeClassification::Production,
                "{path} should classify as production, not unknown"
            );
        }
    }

    #[test]
    fn scope_classification_keeps_rust_ts_css_production() {
        for path in [
            "loctree-rs/src/lib.rs",
            "editors/vscode/src/gateway.ts",
            "web/src/App.tsx",
            "web/src/styles.scss",
            "web/src/main.js",
        ] {
            assert_eq!(
                scope_classification(path),
                ScopeClassification::Production,
                "{path} must remain production (no regression)"
            );
        }
    }

    #[test]
    fn scope_classification_detects_python_tests() {
        for path in [
            "tests/test_chat.py",
            "app/test_models.py",
            "app/models_test.py",
            "conftest.py",
            "pkg/service_test.go",
            "spec/widget_spec.rb",
        ] {
            assert_eq!(
                scope_classification(path),
                ScopeClassification::Test,
                "{path} should classify as test"
            );
        }
    }

    #[test]
    fn scope_classification_detects_python_config() {
        for path in [
            "setup.py",
            "setup.cfg",
            "tox.ini",
            "pyproject.toml",
            "requirements.txt",
            "requirements-dev.txt",
            "Pipfile",
        ] {
            assert_eq!(
                scope_classification(path),
                ScopeClassification::Config,
                "{path} should classify as config"
            );
        }
    }

    #[test]
    fn scope_classification_detects_python_generated() {
        for path in [
            "app/__pycache__/models.cpython-312.pyc",
            "proto/service_pb2.py",
            "proto/service_pb2.pyi",
            "build/output.py",
        ] {
            assert_eq!(
                scope_classification(path),
                ScopeClassification::Generated,
                "{path} should classify as generated"
            );
        }
    }

    #[test]
    fn scope_classification_unknown_stays_honest() {
        // Genuinely unrecognized kinds must NOT be upgraded to production.
        for path in [
            "data/blob.bin",
            "assets/logo.png",
            "fixtures-root/sample.dat",
            "weird.xyz",
            "no_extension_file",
        ] {
            assert_eq!(
                scope_classification(path),
                ScopeClassification::Unknown,
                "{path} should stay unknown — an honest signal, not a guess"
            );
        }
    }

    // ----- classify_occurrence: the three proven Rust shapes + unknown -----
    //
    // These tests deliberately encode the W2-B acceptance strings verbatim and
    // document the limits of single-line classification. They do NOT claim
    // dataflow: a multiline struct literal stays `unknown` on purpose.

    /// Helper: classify `ident` at its first boundary occurrence on `line`.
    fn classify_first(line: &str, ident: &str) -> OccurrenceKind {
        let col = occurrences_in_line(line, ident)[0];
        classify_occurrence(line, ident, col)
    }

    #[test]
    fn classify_let_binding_is_definition_like() {
        assert_eq!(
            classify_first("    let mut utterance_id: u64 = 0;", "utterance_id"),
            OccurrenceKind::DefinitionLike
        );
        // `let` without `mut`, and no type annotation (the `=` must NOT win).
        assert_eq!(
            classify_first("    let utterance_id = 0;", "utterance_id"),
            OccurrenceKind::DefinitionLike
        );
    }

    #[test]
    fn classify_assignment_operators_are_mutation_like() {
        assert_eq!(
            classify_first("        utterance_id += 1;", "utterance_id"),
            OccurrenceKind::MutationLike
        );
        assert_eq!(
            classify_first("    utterance_id = next;", "utterance_id"),
            OccurrenceKind::MutationLike
        );
        assert_eq!(
            classify_first("    utterance_id <<= 2;", "utterance_id"),
            OccurrenceKind::MutationLike
        );
    }

    #[test]
    fn classify_comparison_and_match_arm_are_not_mutation() {
        // `==` is equality, `=>` is a match arm — neither is a mutation.
        assert_eq!(
            classify_first("    if utterance_id == 0 {", "utterance_id"),
            OccurrenceKind::Unknown
        );
        assert_eq!(
            classify_first("        utterance_id => handle(),", "utterance_id"),
            OccurrenceKind::Unknown
        );
        assert_eq!(
            classify_first("    if utterance_id >= 0 {", "utterance_id"),
            OccurrenceKind::Unknown
        );
    }

    #[test]
    fn classify_struct_field_shorthand_is_field_emit_like() {
        assert_eq!(
            classify_first("    UtteranceFinal { utterance_id, text };", "utterance_id"),
            OccurrenceKind::FieldEmitLike
        );
        // Shorthand somewhere in the middle of a single-line literal.
        assert_eq!(
            classify_first("    Event { seq, utterance_id, text }", "utterance_id"),
            OccurrenceKind::FieldEmitLike
        );
    }

    #[test]
    fn classify_is_conservative_for_ambiguous_sites() {
        // Multiline struct literal: opening `{` is on a previous line, so the
        // single-line view cannot prove field emission. Honest `unknown`.
        assert_eq!(
            classify_first("                    utterance_id,", "utterance_id"),
            OccurrenceKind::Unknown
        );
        // Function-call argument is NOT a struct field (innermost delim is `(`).
        assert_eq!(
            classify_first("    push(seq, utterance_id, text)", "utterance_id"),
            OccurrenceKind::Unknown
        );
        // Field-with-value longhand (`field: ty`) is not one of the three shapes.
        assert_eq!(
            classify_first("    pub utterance_id: u64,", "utterance_id"),
            OccurrenceKind::Unknown
        );
        // A plain read.
        assert_eq!(
            classify_first("    let n = utterance_id;", "utterance_id"),
            OccurrenceKind::Unknown
        );
    }

    #[test]
    fn occurrence_kind_label_matches_serialized_value() {
        // The human label and the JSON value must never drift.
        assert_eq!(OccurrenceKind::DefinitionLike.as_str(), "definition_like");
        assert_eq!(OccurrenceKind::MutationLike.as_str(), "mutation_like");
        assert_eq!(OccurrenceKind::FieldEmitLike.as_str(), "field_emit_like");
        assert_eq!(OccurrenceKind::Unknown.as_str(), "unknown");
        let json = serde_json::to_string(&OccurrenceKind::FieldEmitLike).unwrap();
        assert_eq!(json, "\"field_emit_like\"");
    }

    #[test]
    fn empty_query_finds_nothing() {
        assert!(occurrences_in_line("anything", "").is_empty());
        let res = scan_files([("a.rs", "anything")], "");
        assert!(res.occurrences.is_empty());
    }

    // ----- language-aware occurrence_kind taxonomy -----
    //
    // Each new kind has a dedicated single-line fixture, proving the classifier
    // labels real CSS/TS/Rust token shapes instead of a blanket `unknown`.

    /// Classify `ident`'s first occurrence in `text` for the given `path`
    /// (extension drives language selection).
    fn first_kind(path: &str, text: &str, ident: &str) -> OccurrenceKind {
        scan_text(path, text, ident)[0].occurrence_kind
    }

    #[test]
    fn css_property_name_is_css_property() {
        assert_eq!(
            first_kind(
                "styles.css",
                "  backdrop-filter: blur(4px);",
                "backdrop-filter"
            ),
            OccurrenceKind::CssProperty
        );
    }

    #[test]
    fn css_class_selector_is_class_token() {
        assert_eq!(
            first_kind("a.css", ".backdrop { opacity: 0.5; }", "backdrop"),
            OccurrenceKind::ClassToken
        );
    }

    #[test]
    fn css_custom_property_is_custom_property() {
        assert_eq!(
            first_kind("a.css", "  --backdrop: rgba(0,0,0,0.4);", "backdrop"),
            OccurrenceKind::CustomProperty
        );
        // Usage via var(--…) is also a custom property.
        assert_eq!(
            first_kind("a.css", "  color: var(--backdrop);", "backdrop"),
            OccurrenceKind::CustomProperty
        );
    }

    #[test]
    fn css_block_comment_is_comment() {
        assert_eq!(
            first_kind("a.css", "  /* backdrop tuning */", "backdrop"),
            OccurrenceKind::Comment
        );
    }

    #[test]
    fn ts_line_comment_is_comment() {
        assert_eq!(
            first_kind("a.ts", "  // backdrop overlay handling", "backdrop"),
            OccurrenceKind::Comment
        );
    }

    #[test]
    fn ts_string_literal_is_string_literal() {
        assert_eq!(
            first_kind("a.ts", "  const msg = \"open backdrop\";", "backdrop"),
            OccurrenceKind::StringLiteral
        );
    }

    #[test]
    fn ts_class_attribute_token_is_class_token() {
        assert_eq!(
            first_kind(
                "a.tsx",
                "  <div className=\"flex backdrop blur\" />",
                "backdrop"
            ),
            OccurrenceKind::ClassToken
        );
    }

    #[test]
    fn ts_data_attribute_is_data_attribute() {
        assert_eq!(
            first_kind("a.tsx", "  <div data-backdrop=\"true\" />", "backdrop"),
            OccurrenceKind::DataAttribute
        );
    }

    #[test]
    fn css_selector_occurrence_carries_line_and_range_metadata() {
        let occ = scan_text(
            "styles.css",
            ".checkout-success {\n  color: var(--checkout-success);\n}",
            "checkout-success",
        );

        assert_eq!(occ.len(), 2);
        assert_eq!(occ[0].line, 1);
        assert_eq!(occ[0].column, 2);
        assert_eq!(
            occ[0].range,
            OccurrenceRange {
                start: OccurrencePoint { line: 1, column: 2 },
                end: OccurrencePoint {
                    line: 1,
                    column: 18
                }
            }
        );
        assert_eq!(occ[0].occurrence_kind, OccurrenceKind::ClassToken);
        assert_eq!(occ[1].occurrence_kind, OccurrenceKind::CustomProperty);
    }

    #[test]
    fn scan_files_with_scope_keeps_only_requested_file() {
        let res = scan_files_with_scope(
            [
                ("src/a.css", ".checkout-success {}"),
                ("src/b.css", ".checkout-success {}"),
            ],
            "checkout-success",
            ScanOptions::default(),
            FileScope {
                file: Some("src/b.css"),
            },
        );

        assert_eq!(res.total, 1);
        assert_eq!(res.files_matched, 1);
        assert_eq!(res.occurrences[0].file, "src/b.css");
    }

    #[test]
    fn scan_files_with_regex_finds_pattern_and_labels_context() {
        // loctree-feedback.md (2026-06-21): `--literal` false-cleans a regex query
        // like `100\.[0-9]+\.[0-9]+`. `--regex` actually evaluates the pattern.
        let re = regex::Regex::new(r"100\.[0-9]+\.[0-9]+\.[0-9]+").unwrap();
        let res = scan_files_with_regex(
            [
                ("app.py", "HOST = \"100.64.0.1\"\n"),
                ("notes.md", "node at 100.127.0.1 ok\n"),
            ],
            &re,
            FileScope::default(),
        );
        assert_eq!(res.total, 2);
        assert_eq!(res.files_matched, 2);
        assert_eq!(res.match_mode, MatchMode::Regex);
        assert_eq!(res.source, "regex");
        // matched_text is the ACTUAL match, not the pattern.
        let py = res.occurrences.iter().find(|o| o.file == "app.py").unwrap();
        assert_eq!(py.matched_text, "100.64.0.1");
        assert_eq!(py.source, "regex");
        // Context label distinguishes a live string-literal hit from prose — the
        // discrimination a secret/privacy audit needs.
        assert_eq!(py.occurrence_kind, OccurrenceKind::StringLiteral);
    }

    #[test]
    fn scan_files_with_regex_scans_and_flags_generated_artifacts() {
        // W2-02 truth contract: artifact-classed files are scanned and tallied,
        // never skipped — otherwise `absence_trustworthy: true` lies about files
        // the scan silently left out (scorecard correctness loss vs rg).
        let re = regex::Regex::new(r"secret").unwrap();
        let res = scan_files_with_regex(
            [
                ("src/app.rs", "let secret = load();\n"),
                ("dist/bundle.js", "var secret=1\n"),
            ],
            &re,
            FileScope::default(),
        );
        assert_eq!(res.total, 2, "generated file is scanned, not skipped");
        let hit_files: Vec<&str> = res.occurrences.iter().map(|o| o.file.as_str()).collect();
        assert!(hit_files.contains(&"src/app.rs"));
        assert!(hit_files.contains(&"dist/bundle.js"));
        let scope = res.scope.as_ref().unwrap();
        assert_eq!(scope.files_scanned, 2);
        assert_eq!(scope.generated, 1, "the class tally survives as accounting");
        assert!(res.coverage_line.contains("artifact-flagged: generated(1)"));
        assert!(!res.coverage_line.contains("excluded:"));
    }

    #[test]
    fn literal_scan_covers_fixture_paths() {
        // Regression for the W2-01 suite regex loss: rg saw hits under
        // tests/fixtures/** that the fence dropped from the literal universe.
        let res = scan_files(
            [
                ("src/lib.rs", "pub struct OccurrenceRole;\n"),
                (
                    "tests/fixtures/cfamily/README.md",
                    "documents OccurrenceRole coverage\n",
                ),
            ],
            "OccurrenceRole",
        );
        assert_eq!(res.total, 2, "fixture hit must be reported");
        assert_eq!(res.files_matched, 2);
        let scope = res.scope.as_ref().unwrap();
        assert_eq!(scope.files_scanned, 2);
        assert_eq!(scope.fixtures, 1);
    }

    #[test]
    fn context_snippet_windows_only_long_lines() {
        let short = "let x = secret;";
        assert_eq!(context_snippet(short, 9, 6), short);

        let long = format!("{}secret{}", "a".repeat(500), "b".repeat(500));
        let snip = context_snippet(&long, 501, 6);
        assert!(snip.contains("secret"));
        assert!(snip.starts_with('…') && snip.ends_with('…'));
        assert!(
            snip.chars().count() <= 2 * CONTEXT_WINDOW_CHARS + 6 + 2,
            "window stays bounded, got {} chars",
            snip.chars().count()
        );
    }

    #[test]
    fn scan_files_with_regex_empty_on_no_match() {
        let re = regex::Regex::new(r"203\.0\.\d+\.\d+").unwrap();
        let res = scan_files_with_regex(
            [("app.py", "HOST = \"100.64.0.1\"\n")],
            &re,
            FileScope::default(),
        );
        assert_eq!(res.total, 0);
        assert_eq!(res.files_matched, 0);
        // The file WAS scanned — absence is real, not skipped.
        assert_eq!(res.scope.as_ref().unwrap().files_scanned, 1);
    }

    #[test]
    fn scan_results_carry_query_contract_and_suggested_next() {
        let res = scan_files(
            [("src/lib.rs", "pub fn LoctreeServer() {}")],
            "LoctreeServer",
        );
        assert_eq!(res.query_kind, QueryKind::Identifier);
        assert_eq!(res.match_mode, MatchMode::IdentifierBoundary);
        assert_eq!(
            res.occurrences[0].occurrence_kind,
            OccurrenceKind::Identifier
        );
        assert_eq!(res.occurrences[0].match_role, MatchRole::Definition);
        assert_eq!(res.occurrences[0].confidence, MatchConfidence::High);
        assert!(
            res.suggested_next
                .iter()
                .any(|s| s.command == "loct body 'LoctreeServer' --json")
        );
        assert_eq!(
            res.scope_classifications[0],
            ScopeClassificationCount {
                scope_classification: ScopeClassification::Production,
                count: 1,
            }
        );
    }

    #[test]
    fn role_summary_buckets_definitions_callsites_and_imports() {
        // One definition (`pub fn alpha`), two callsites (the `alpha()` reads),
        // and one import (`use crate::alpha`). The rollup must separate them so an
        // agent sees "defined once, used twice" without walking every hit.
        let res = scan_files(
            [(
                "src/lib.rs",
                "pub fn alpha() {}\nfn beta() { alpha(); alpha(); }\nuse crate::alpha;",
            )],
            "alpha",
        );
        let summary = res
            .role_summary
            .expect("role_summary is present whenever there is at least one hit");
        assert_eq!(summary.definitions, 1, "the `pub fn alpha` definition site");
        assert_eq!(summary.callsites, 2, "the two `alpha()` read sites");
        assert_eq!(summary.imports, 1, "the `use crate::alpha` import site");
        assert!(
            summary.definition_files.contains(&"src/lib.rs".to_string()),
            "the definition file is pointed to for follow-up"
        );
    }

    #[test]
    fn not_found_omits_role_summary() {
        // A not-found result must omit the rollup entirely (additive: the field
        // is `None`, not a misleading all-zeros object).
        let res = scan_files([("src/lib.rs", "pub fn present() {}")], "MissingSymbol");
        assert_eq!(res.total, 0);
        assert!(res.role_summary.is_none());
        assert!(res.file_context.is_empty());
    }

    #[test]
    fn enrich_attaches_importer_consumer_context_from_edges() {
        // `page.rs` imports `widget.rs`; a literal hit in `widget.rs` must carry
        // the consumer (`page.rs`) so the flat hit list becomes blast-radius
        // aware. Edges come from the snapshot — the same source `slice`/`impact`
        // read — so the literal and structural surfaces never disagree.
        let mut res = scan_files([("src/widget.rs", "pub fn render() {}")], "render");
        let snapshot: Snapshot = serde_json::from_str(
            r#"{
                "metadata": {},
                "files": [
                    {"path": "src/widget.rs"},
                    {"path": "src/page.rs"}
                ],
                "edges": [
                    {"from": "src/page.rs", "to": "src/widget.rs", "label": "import"}
                ]
            }"#,
        )
        .expect("minimal snapshot deserializes via serde defaults");
        enrich_with_snapshot(&mut res, &snapshot);
        let ctx = res
            .file_context
            .iter()
            .find(|c| c.file == "src/widget.rs")
            .expect("file context attached for the hit-carrying file");
        assert!(
            ctx.imported_by.contains(&"src/page.rs".to_string()),
            "page.rs consumes widget.rs, so it is listed as an importer"
        );
        assert_eq!(ctx.hits, 1, "one literal hit in widget.rs");
        assert!(
            !ctx.truncated,
            "a single edge is well under the truncation cap"
        );
    }

    #[test]
    fn zero_results_suggest_broadening_without_fuzzy_evidence() {
        let res = scan_files([("src/lib.rs", "pub fn present() {}")], "MissingSymbol");
        assert_eq!(res.total, 0);
        assert_eq!(res.query_kind, QueryKind::Identifier);
        assert!(res.scope_classifications.is_empty());
        assert!(res.suggested_next.iter().any(|s| {
            s.command == "loct find 'MissingSymbol' --json"
                && s.reason
                    .contains("without treating suggestions as evidence")
        }));
    }

    #[test]
    fn ts_bare_identifier_is_identifier() {
        assert_eq!(
            first_kind("a.ts", "  const x = backdrop;", "backdrop"),
            OccurrenceKind::Identifier
        );
    }

    #[test]
    fn rust_doc_comment_is_comment() {
        assert_eq!(
            first_kind("a.rs", "/// uses the backdrop counter", "backdrop"),
            OccurrenceKind::Comment
        );
    }

    #[test]
    fn rust_plain_read_is_identifier_not_unknown() {
        // The honest upgrade: an ordinary read is `identifier`, not `unknown`.
        assert_eq!(
            first_kind("a.rs", "    let n = backdrop;", "backdrop"),
            OccurrenceKind::Identifier
        );
    }

    // ----- whole_token boundary -----

    #[test]
    fn whole_token_excludes_hyphenated_neighbors() {
        let line = "  z-index: var(--vista-z-overlay-backdrop);";
        // Default boundary: `backdrop` leaks into the hyphenated custom property.
        assert_eq!(occurrences_in_line_with(line, "backdrop", false).len(), 1);
        // whole_token: hyphen is token-internal, so the noisy hit disappears.
        assert!(occurrences_in_line_with(line, "backdrop", true).is_empty());
    }

    #[test]
    fn whole_token_keeps_free_standing_token() {
        let line = ".backdrop { opacity: 0.5; }";
        assert_eq!(occurrences_in_line_with(line, "backdrop", true), vec![2]);
    }

    #[test]
    fn scan_files_with_whole_token_drops_z_index_noise() {
        let css = ".backdrop {}\n  --vista-z-overlay-backdrop: 40;";
        let loose = scan_files_with([("a.css", css)], "backdrop", ScanOptions::default());
        let tight = scan_files_with(
            [("a.css", css)],
            "backdrop",
            ScanOptions { whole_token: true },
        );
        assert_eq!(loose.total, 2, "default boundary keeps both hits");
        assert_eq!(
            tight.total, 1,
            "whole_token keeps only the free-standing one"
        );
    }

    // ----- aggregation: group_by_file + count_only/slim -----

    #[test]
    fn group_by_file_rolls_up_per_file_counts() {
        let mut res = scan_files(
            [
                ("a.css", ".backdrop {}\n.backdrop {}"),
                ("b.css", ".backdrop {}"),
            ],
            "backdrop",
        );
        res.apply_report(ReportOptions {
            group_by_file: true,
            count_only: false,
            offset: 0,
            limit: None,
        });
        let by_file = res.by_file.expect("group_by_file populates by_file");
        assert_eq!(by_file.len(), 2);
        assert_eq!(
            by_file[0],
            FileCount {
                file: "a.css".into(),
                count: 2,
                scope_classification: ScopeClassification::Production,
            }
        );
        assert_eq!(
            by_file[1],
            FileCount {
                file: "b.css".into(),
                count: 1,
                scope_classification: ScopeClassification::Production,
            }
        );
        // The full list is still present (group_by_file is additive).
        assert_eq!(res.occurrences.len(), 3);
    }

    #[test]
    fn count_only_suppresses_occurrences_but_keeps_total() {
        let mut res = scan_files([("a.css", ".backdrop {}\n.backdrop {}")], "backdrop");
        res.apply_report(ReportOptions {
            group_by_file: false,
            count_only: true,
            offset: 0,
            limit: None,
        });
        assert!(res.slim, "count_only marks the result slim");
        assert!(res.occurrences.is_empty(), "occurrences suppressed");
        assert_eq!(res.total, 2, "total survives slim truncation");
        assert_eq!(res.files_matched, 1);
    }

    #[test]
    fn slim_serialization_is_distinguishable_from_not_found() {
        let mut res = scan_files([("a.css", ".backdrop {}")], "backdrop");
        res.apply_report(ReportOptions {
            group_by_file: false,
            count_only: true,
            offset: 0,
            limit: None,
        });
        let json = serde_json::to_value(&res).unwrap();
        assert_eq!(json["slim"], serde_json::json!(true));
        assert_eq!(json["total"], serde_json::json!(1));
        // by_file omitted when not requested (additive, no shape churn).
        assert!(json.get("by_file").is_none());
    }

    #[test]
    fn paged_report_returns_requested_slice_with_next_offset() {
        let mut res = scan_files(
            [("a.css", ".backdrop {}\n.backdrop {}\n.backdrop {}")],
            "backdrop",
        );
        res.apply_report(ReportOptions {
            group_by_file: false,
            count_only: false,
            offset: 1,
            limit: Some(1),
        });

        let page = res.page.expect("limit populates page metadata");
        assert_eq!(res.total, 3, "total remains the full occurrence count");
        assert_eq!(res.occurrences.len(), 1);
        assert_eq!(res.occurrences[0].line, 2);
        assert_eq!(page.offset, 1);
        assert_eq!(page.limit, 1);
        assert_eq!(page.returned, 1);
        assert!(page.has_more);
        assert_eq!(page.next_offset, Some(2));
    }

    #[test]
    fn paged_report_handles_final_page_without_next_offset() {
        let mut res = scan_files([("a.css", ".backdrop {}\n.backdrop {}")], "backdrop");
        res.apply_report(ReportOptions {
            group_by_file: false,
            count_only: false,
            offset: 1,
            limit: Some(10),
        });

        let page = res.page.expect("limit populates page metadata");
        assert_eq!(res.occurrences.len(), 1);
        assert_eq!(page.returned, 1);
        assert!(!page.has_more);
        assert_eq!(page.next_offset, None);
    }

    #[test]
    fn test_coverage_contract_statistics() {
        let files = [
            ("src/lib.rs", "pub fn hello() {}"),
            ("tests/fixtures/foo.rs", "fn test_foo() {}"),
            ("dist/bundle.js", "console.log(1);"),
        ];
        let res = scan_files_with_scope(
            files.iter().map(|(p, c)| (*p, *c)),
            "hello",
            ScanOptions::default(),
            FileScope::default(),
        );
        let scope = res.scope.expect("scope stats must be populated");
        // W2-02 truth contract: every universe file is scanned; artifact
        // classes are accounting overlays, not subtractions from coverage.
        assert_eq!(scope.files_scanned, 3);
        assert_eq!(scope.fixtures, 1);
        assert_eq!(scope.generated, 1);
        assert_eq!(scope.vendored, 0);
        assert_eq!(scope.templates, 0);
        assert_eq!(scope.files_in_universe, 3);
        assert_eq!(scope.files_scanned, scope.files_in_universe);
        assert!(res.coverage_line.contains("scanned 3 of 3 repo files"));
        assert!(res.coverage_line.contains("fixtures(1)"));
        assert!(res.coverage_line.contains("generated(1)"));
    }

    #[test]
    fn new_kind_labels_match_serialized_values() {
        for (kind, label) in [
            (OccurrenceKind::CssProperty, "css_property"),
            (OccurrenceKind::ClassToken, "class_token"),
            (OccurrenceKind::CustomProperty, "custom_property"),
            (OccurrenceKind::Comment, "comment"),
            (OccurrenceKind::StringLiteral, "string_literal"),
            (OccurrenceKind::DataAttribute, "data_attribute"),
            (OccurrenceKind::Identifier, "identifier"),
        ] {
            assert_eq!(kind.as_str(), label);
            assert_eq!(
                serde_json::to_string(&kind).unwrap(),
                format!("\"{label}\"")
            );
        }
    }
}
