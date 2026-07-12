//! Custom LSP request: `loctree/find`
//!
//! Semantic-aware symbol search. Returns categorized matches (symbols,
//! params, similarity, dead status, cross-matches) — same shape as the
//! `loct find` CLI, mapped to JSON for daemon-mode agent consumption.
//!
//! Plan 07 of the LSP roadmap.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::path::PathBuf;

use loctree::analyzer::classify::language_from_path;
use loctree::analyzer::dead_parrots::{SimilarityCandidate, SymbolFileMatch, SymbolMatchKind};
use loctree::analyzer::occurrences::{
    FileScope, OccurrenceResults, ReportOptions, ScanOptions, scan_files_with_scope,
};
use loctree::analyzer::search::{
    CrossMatchFile, FuzzySuggestion, ParamMatch, SearchResults, SuppressionMatch,
};
use loctree::snapshot::Snapshot;
use loctree::types::SymbolIdV1;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::cursor::{CursorError, CursorState};
use crate::protocol::{DEFAULT_CHUNK_SIZE, Paginated, paginate, single_page};

const DEFAULT_LIMIT: usize = 50;
const SNAPSHOT_ID_FALLBACK: &str = "snapshot:unknown";
const SYMBOL_CURSOR_KIND: &str = "loctree/find.symbol_matches";
const PARAM_CURSOR_KIND: &str = "loctree/find.param_matches";
const SEMANTIC_CURSOR_KIND: &str = "loctree/find.semantic_matches";
const SUPPRESSION_CURSOR_KIND: &str = "loctree/find.suppression_matches";
const CROSS_CURSOR_KIND: &str = "loctree/find.cross_matches";

fn default_mode() -> String {
    "single".to_string()
}

/// Parameters for `loctree/find`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindParams {
    /// Search query. May contain `|` for explicit OR matching.
    pub query: String,
    /// Search mode: `single` (default), `split`, `and`, or `literal`.
    ///
    /// - `single`: query passed verbatim to the analyzer.
    /// - `split`: whitespace becomes `|` so each token is OR-matched.
    /// - `and`: same query expansion as `split`, but the response keeps
    ///   only `cross_matches` (files where 2+ terms meet); the other
    ///   match buckets are emptied so callers don't reason on partial OR
    ///   hits.
    /// - `literal`: the W1 exact-identifier truth layer. Scans raw source
    ///   bytes for identifier-boundary occurrences and returns them in
    ///   `literal_matches`; the AST/fuzzy buckets are emptied. At parity
    ///   with `loct occurrences` / `loct find --literal` — same scanner,
    ///   same bytes, same answer. "Not found" means not found.
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Optional language tag filter (`rust`, `typescript`, `python`, ...).
    /// Filters by the analyzer's path-language classifier on every match list.
    #[serde(default)]
    pub lang: Option<String>,
    /// Optional file/path scope. In literal mode this narrows exact occurrence
    /// scanning to the requested snapshot path; in non-literal modes this is
    /// reserved for future parity and ignored.
    #[serde(default)]
    pub file: Option<String>,
    /// Keep only matches that appear in the analyzer's dead-export list.
    #[serde(default)]
    pub dead_only: bool,
    /// Keep only matches that are symbol definitions (drops imports / usages).
    #[serde(default)]
    pub exported_only: bool,
    /// Cap each match list to this many entries (default 50).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Workspace project root override. Reserved for Plan 13
    /// (multi-workspace context); ignored in single-workspace mode.
    #[serde(default)]
    pub project: Option<PathBuf>,
    /// Opaque Plan 12 cursor returned by any paginated match bucket.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Requested page size for paginated match buckets.
    #[serde(default)]
    pub chunk_size: Option<usize>,
    /// Literal mode only: require tighter whole-token boundaries, matching
    /// `loct occurrences --whole-token` / MCP `find(mode="literal")`.
    #[serde(default)]
    pub whole_token: bool,
    /// Literal mode only: attach per-file occurrence counts.
    #[serde(default)]
    pub group_by_file: bool,
    /// Literal mode only: suppress the occurrence list and keep counters.
    #[serde(default)]
    pub count_only: bool,
    /// Literal mode only: alias for `count_only`, matching MCP/CLI wording.
    #[serde(default)]
    pub slim: bool,
    /// Literal mode only: zero-based occurrence offset for paged output.
    #[serde(default)]
    pub offset: usize,
    /// **Plan 18 v1 contract.** Stable identifier for the symbol the
    /// caller wants to anchor on (`<file_path>::<symbol_name>`). When
    /// present, the response echoes it back so agents can correlate
    /// chunked or paginated calls without re-deriving the id.
    ///
    /// v1 ids only survive renames within a file when the symbol name
    /// is unchanged — see [`SymbolIdV1`] doc comment for the v2
    /// contract roadmap (deferred until the tree-sitter substrate
    /// from Plan 16 lands).
    #[serde(default)]
    pub symbol_id: Option<SymbolIdV1>,
}

/// `loctree/find` response — shape mirrors `SearchResults` but with
/// post-processing applied (mode/lang/dead_only/exported_only/limit).
#[derive(Debug, Serialize)]
pub struct FindResponse {
    /// Echo of the query string used by the analyzer (after mode expansion).
    pub query: String,
    /// Symbol-name matches (definitions, imports, usages).
    pub symbol_matches: Paginated<SymbolSearchView>,
    /// Function parameter matches.
    pub param_matches: Paginated<Vec<ParamMatch>>,
    /// Fuzzy / similarity candidates with score.
    pub semantic_matches: Paginated<Vec<SimilarityCandidate>>,
    /// Lint-suppression matches (`#[allow(...)]`, `eslint-disable`, ...).
    pub suppression_matches: Paginated<Vec<SuppressionMatch>>,
    /// Files containing 2+ different query terms (multi-query cross-match).
    pub cross_matches: Paginated<Vec<CrossMatchFile>>,
    /// Dead-code status for the searched symbol.
    pub dead_status: DeadStatusView,
    /// Total file count across all returned match lists (de-duplicated).
    pub total_matches: usize,
    /// Echo of the v1 [`SymbolIdV1`] supplied in the request, if any.
    /// Lets paginated / multi-call agents track the anchor symbol
    /// without re-deriving the id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_id: Option<SymbolIdV1>,
    /// Wire version of the symbol-id contract in this response. Always
    /// emitted so clients can probe before relying on body-sensitive
    /// behavior (current value: `"v1-string"` — body hash deferred to
    /// Plan 18 v2).
    pub symbol_id_version: &'static str,
    /// **Literal mode only.** Exact identifier-boundary occurrences from the
    /// W1 truth layer, byte-for-byte identical to `loct occurrences` /
    /// `loct find --literal`. Absent (omitted from JSON) for every other mode,
    /// so existing clients are unaffected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub literal_matches: Option<OccurrenceResults>,
    /// **Literal mode only.** Fuzzy name-similarity hints kept strictly
    /// separate from `literal_matches` (provenance `"fuzzy"`). A suggestion is
    /// not evidence and is never promoted into the literal set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub literal_fuzzy: Option<Vec<FuzzySuggestion>>,
}

/// Filtered projection of `SymbolSearchResult` so we can drop matches
/// post-hoc per `lang`/`exported_only` filters and re-aggregate counts.
#[derive(Debug, Serialize)]
pub struct SymbolSearchView {
    pub found: bool,
    pub total_matches: usize,
    pub files: Vec<SymbolFileMatch>,
}

/// LSP projection of `analyzer::search::DeadStatus`. Same fields, kept
/// in this crate so we don't expose the internal type tree to clients.
#[derive(Debug, Serialize)]
pub struct DeadStatusView {
    pub is_exported: bool,
    pub is_dead: bool,
    pub dead_in_files: Vec<String>,
}

/// Apply the requested mode to the query string. The analyzer's
/// `normalize_query` handles whitespace internally, but we expand here
/// so the response's `query` field reflects what the caller sees.
pub fn build_query(params: &FindParams) -> String {
    match params.mode.as_str() {
        "split" | "and" => params
            .query
            .split_whitespace()
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("|"),
        _ => params.query.clone(),
    }
}

/// Map `SearchResults` from the analyzer into the LSP response, applying
/// the post-processing filters from `params`.
pub fn build_response(results: SearchResults, params: &FindParams) -> FindResponse {
    build_response_paginated(results, params, SNAPSHOT_ID_FALLBACK)
        .expect("default find pagination should not fail")
}

/// Map `SearchResults` from the analyzer into the cursor-aware LSP response.
pub fn build_response_paginated(
    results: SearchResults,
    params: &FindParams,
    snapshot_id: &str,
) -> Result<FindResponse, CursorError> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    let chunk_size = params.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);
    let lang_filter = params.lang.as_deref().and_then(language_filter);
    let cursor_state = match params.cursor.as_deref() {
        Some(token) => Some(CursorState::decode_raw(token)?),
        None => None,
    };
    let offsets = match_offsets(cursor_state.as_ref(), snapshot_id)?;

    // 1. symbol matches: language filter, exported_only filter, then
    //    per-file `matches` truncation, finally cap files at `limit`.
    let symbol_matches = filter_symbol_matches(
        results.symbol_matches.files,
        lang_filter,
        params.exported_only,
        limit,
    );
    let symbol_total: usize = symbol_matches.files.iter().map(|f| f.matches.len()).sum();

    // 2. param matches: language filter only.
    let mut param_matches: Vec<ParamMatch> = results
        .param_matches
        .into_iter()
        .filter(|m| matches_language(&m.file, lang_filter))
        .collect();
    truncate(&mut param_matches, limit);

    // 3. semantic matches: language filter, sort by score desc, cap.
    let mut semantic_matches: Vec<SimilarityCandidate> = results
        .semantic_matches
        .into_iter()
        .filter(|c| matches_language(&c.file, lang_filter))
        .collect();
    semantic_matches.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    truncate(&mut semantic_matches, limit);

    // 4. suppression matches: language filter, cap.
    let mut suppression_matches: Vec<SuppressionMatch> = results
        .suppression_matches
        .into_iter()
        .filter(|m| matches_language(&m.file, lang_filter))
        .collect();
    truncate(&mut suppression_matches, limit);

    // 5. cross-matches: language filter (any term in the file matches
    //    the lang filter passes — coarse but predictable), cap.
    let mut cross_matches: Vec<CrossMatchFile> = results
        .cross_matches
        .into_iter()
        .filter(|c| matches_language(&c.file, lang_filter))
        .collect();
    truncate(&mut cross_matches, limit);

    let dead_status = DeadStatusView {
        is_exported: results.dead_status.is_exported,
        is_dead: results.dead_status.is_dead,
        dead_in_files: results
            .dead_status
            .dead_in_files
            .into_iter()
            .filter(|f| matches_language(f, lang_filter))
            .collect(),
    };

    let mut raw = FindResponseRaw {
        query: results.query,
        symbol_matches: SymbolSearchView {
            found: !symbol_matches.files.is_empty(),
            total_matches: symbol_total,
            files: symbol_matches.files,
        },
        param_matches,
        semantic_matches,
        suppression_matches,
        cross_matches,
        dead_status,
        symbol_id: params.symbol_id.clone(),
        symbol_id_version: SymbolIdV1::VERSION,
    };

    // 6. dead_only: prune to only matches that touch dead-export files.
    if params.dead_only {
        apply_dead_only(&mut raw);
    }

    // 7. and: keep only cross_matches (others zeroed).
    if params.mode == "and" {
        raw.symbol_matches = SymbolSearchView {
            found: false,
            total_matches: 0,
            files: Vec::new(),
        };
        raw.param_matches.clear();
        raw.semantic_matches.clear();
        raw.suppression_matches.clear();
    }

    paginate_response(raw, offsets, chunk_size, snapshot_id)
}

struct FindResponseRaw {
    query: String,
    symbol_matches: SymbolSearchView,
    param_matches: Vec<ParamMatch>,
    semantic_matches: Vec<SimilarityCandidate>,
    suppression_matches: Vec<SuppressionMatch>,
    cross_matches: Vec<CrossMatchFile>,
    dead_status: DeadStatusView,
    symbol_id: Option<SymbolIdV1>,
    symbol_id_version: &'static str,
}

#[derive(Debug, Clone, Copy, Default)]
struct MatchOffsets {
    symbol_matches: usize,
    param_matches: usize,
    semantic_matches: usize,
    suppression_matches: usize,
    cross_matches: usize,
}

fn match_offsets(
    cursor: Option<&CursorState>,
    snapshot_id: &str,
) -> Result<MatchOffsets, CursorError> {
    let Some(cursor) = cursor else {
        return Ok(MatchOffsets::default());
    };
    if cursor.snapshot_id != snapshot_id {
        return Err(CursorError::SnapshotDrifted {
            expected: snapshot_id.into(),
            got: cursor.snapshot_id.clone(),
        });
    }
    let mut offsets = MatchOffsets::default();
    match cursor.kind.as_str() {
        SYMBOL_CURSOR_KIND => offsets.symbol_matches = cursor.offset,
        PARAM_CURSOR_KIND => offsets.param_matches = cursor.offset,
        SEMANTIC_CURSOR_KIND => offsets.semantic_matches = cursor.offset,
        SUPPRESSION_CURSOR_KIND => offsets.suppression_matches = cursor.offset,
        CROSS_CURSOR_KIND => offsets.cross_matches = cursor.offset,
        other => {
            return Err(CursorError::KindMismatch {
                expected: format!(
                    "{SYMBOL_CURSOR_KIND}|{PARAM_CURSOR_KIND}|{SEMANTIC_CURSOR_KIND}|{SUPPRESSION_CURSOR_KIND}|{CROSS_CURSOR_KIND}"
                ),
                got: other.into(),
            });
        }
    }
    Ok(offsets)
}

fn paginate_response(
    raw: FindResponseRaw,
    offsets: MatchOffsets,
    chunk_size: usize,
    snapshot_id: &str,
) -> Result<FindResponse, CursorError> {
    let symbol_page = paginate(
        &raw.symbol_matches.files,
        offsets.symbol_matches,
        chunk_size,
        snapshot_id,
        SYMBOL_CURSOR_KIND,
    )?;
    let symbol_matches = Paginated {
        chunk: symbol_page.chunk,
        total_chunks: symbol_page.total_chunks,
        next_cursor: symbol_page.next_cursor,
        data: SymbolSearchView {
            found: !symbol_page.data.is_empty(),
            total_matches: symbol_page.data.iter().map(|file| file.matches.len()).sum(),
            files: symbol_page.data,
        },
        advisory: symbol_page.advisory,
    };

    let param_matches = paginate(
        &raw.param_matches,
        offsets.param_matches,
        chunk_size,
        snapshot_id,
        PARAM_CURSOR_KIND,
    )?;
    let semantic_matches = paginate(
        &raw.semantic_matches,
        offsets.semantic_matches,
        chunk_size,
        snapshot_id,
        SEMANTIC_CURSOR_KIND,
    )?;
    let suppression_matches = paginate(
        &raw.suppression_matches,
        offsets.suppression_matches,
        chunk_size,
        snapshot_id,
        SUPPRESSION_CURSOR_KIND,
    )?;
    let cross_matches = paginate(
        &raw.cross_matches,
        offsets.cross_matches,
        chunk_size,
        snapshot_id,
        CROSS_CURSOR_KIND,
    )?;

    let total_matches = symbol_matches.data.total_matches
        + param_matches.data.len()
        + semantic_matches.data.len()
        + suppression_matches.data.len()
        + cross_matches.data.len();

    Ok(FindResponse {
        query: raw.query,
        symbol_matches,
        param_matches,
        semantic_matches,
        suppression_matches,
        cross_matches,
        dead_status: raw.dead_status,
        total_matches,
        symbol_id: raw.symbol_id,
        symbol_id_version: raw.symbol_id_version,
        // Non-literal modes never carry the literal truth layer.
        literal_matches: None,
        literal_fuzzy: None,
    })
}

/// Scan the snapshot's files for literal identifier-boundary occurrences of
/// `ident`, reading raw bytes from disk relative to `base`.
///
/// This is the LSP surface of the W1 literal truth layer. It reuses the shared
/// [`loctree::analyzer::occurrences::scan_files`] scanner — the same one
/// `loct occurrences` / `loct find --literal` and the MCP `find(mode=literal)`
/// use — so every surface reports the identical file/line set for a given
/// snapshot. Only the file-enumeration glue is mirrored from the CLI handler
/// (`read_snapshot_contents`); there is no second scanner.
///
/// Unreadable files (binary, deleted, permission) are skipped — they are simply
/// not literal match sites, exactly as in the CLI.
pub fn scan_literal(
    snapshot: &Snapshot,
    base: Option<&std::path::Path>,
    ident: &str,
    opts: ScanOptions,
    scope: FileScope<'_>,
) -> OccurrenceResults {
    let contents: Vec<(String, String)> = snapshot
        .files
        .iter()
        .filter_map(|file| {
            let snapshot_path = std::path::Path::new(&file.path);
            let resolved = match base {
                Some(root) => {
                    if snapshot_path.is_absolute() {
                        // Absolute snapshot paths are used only after validating
                        // they exist; never silently fall back elsewhere.
                        if snapshot_path.exists() {
                            snapshot_path.to_path_buf()
                        } else {
                            return None;
                        }
                    } else {
                        // Relative snapshot paths MUST resolve under the
                        // workspace root. Do NOT fall back to a CWD-relative
                        // path (a different LSP client / odd process CWD could
                        // otherwise leak files outside the workspace). If the
                        // workspace-relative file is absent, skip it.
                        let joined = root.join(snapshot_path);
                        if joined.exists() {
                            joined
                        } else {
                            return None;
                        }
                    }
                }
                None => std::path::PathBuf::from(&file.path),
            };
            std::fs::read_to_string(&resolved)
                .ok()
                .map(|text| (file.path.clone(), text))
        })
        .collect();
    let borrowed = contents
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect::<Vec<_>>();
    scan_files_with_scope(borrowed, ident.trim(), opts, scope)
}

#[derive(PartialEq, Eq)]
struct LiteralCacheKey {
    /// Workspace root the scan resolves against. Two workspaces that share a
    /// git HEAD (monorepo sub-projects) must NOT collide, so the root is part
    /// of the key.
    base: Option<String>,
    /// Content fingerprint of the candidate file set — see
    /// [`file_set_fingerprint`]. This is the freshness signal: it changes on
    /// any on-disk edit, not just on commit, so a saved-but-uncommitted edit
    /// invalidates the entry (a git-commit-granular snapshot id does not).
    fingerprint: u64,
    ident: String,
    whole_token: bool,
    file: Option<String>,
}

/// Fingerprint the file set [`scan_literal`] would read: fold each file's
/// resolved `(path, len, mtime)` into one hash. Because `scan_literal` reads
/// LIVE disk bytes (the snapshot only supplies the file *list*), the cache must
/// key on actual on-disk state, not a commit-granular snapshot id — otherwise a
/// saved-but-uncommitted edit (the dominant editor workflow) is served stale.
/// `stat` is far cheaper than the `read_to_string` per file the cache avoids on
/// a hit, so the storm case still wins.
///
/// Freshness granularity is `(len, mtime)`, not a content hash: an edit that
/// keeps the byte length AND lands in the same filesystem mtime tick is not
/// detected. That is near-impossible for a human save (nanosecond mtime on
/// APFS/ext4/NTFS, 16-entry window) and is a strict, large improvement over a
/// commit-granular key, which served stale for the whole span between commits.
fn file_set_fingerprint(snapshot: &Snapshot, base: Option<&std::path::Path>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for file in &snapshot.files {
        let snapshot_path = std::path::Path::new(&file.path);
        let resolved = match base {
            Some(root) if !snapshot_path.is_absolute() => root.join(snapshot_path),
            _ => std::path::PathBuf::from(&file.path),
        };
        file.path.hash(&mut hasher);
        match std::fs::metadata(&resolved) {
            Ok(meta) => {
                meta.len().hash(&mut hasher);
                if let Ok(mtime) = meta.modified()
                    && let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH)
                {
                    dur.as_nanos().hash(&mut hasher);
                }
            }
            // Absent/unreadable hashes distinctly from any present state.
            Err(_) => 0u8.hash(&mut hasher),
        }
    }
    hasher.finish()
}

/// Bounded LRU cache for [`scan_literal`] results.
///
/// `scan_literal` reads every workspace file from disk on each call, so repeated
/// hover / "Load more" over the same query would otherwise re-scan the whole
/// tree — a hidden "scan storm" on large workspaces. The cache is keyed by
/// `(workspace root, file-set fingerprint, identifier, whole_token, file
/// scope)`. The fingerprint (resolved path + len + mtime per file) is the
/// freshness signal: any on-disk edit changes it and forces a fresh scan, so a
/// reloaded or edited workspace is never served stale matches; entries also age
/// out under the capacity bound. The expensive scan runs WITHOUT the lock held
/// so distinct concurrent queries do not serialize behind one another.
pub struct LiteralScanCache {
    inner: std::sync::Mutex<std::collections::VecDeque<(LiteralCacheKey, OccurrenceResults)>>,
    cap: usize,
}

impl LiteralScanCache {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(std::collections::VecDeque::new()),
            cap: 16,
        }
    }

    /// Return the cached literal scan for this `(base, ident, opts, scope)` over
    /// the current on-disk state, or run [`scan_literal`] and cache the result.
    pub fn get_or_scan(
        &self,
        snapshot: &Snapshot,
        base: Option<&std::path::Path>,
        ident: &str,
        opts: ScanOptions,
        scope: FileScope<'_>,
    ) -> OccurrenceResults {
        let key = LiteralCacheKey {
            base: base.map(|p| p.display().to_string()),
            fingerprint: file_set_fingerprint(snapshot, base),
            ident: ident.to_string(),
            whole_token: opts.whole_token,
            file: scope.file.map(str::to_string),
        };

        // Fast path: a hit clones the cached result and moves the entry to the
        // front (most-recently-used).
        if let Ok(mut guard) = self.inner.lock()
            && let Some(pos) = guard.iter().position(|(k, _)| *k == key)
        {
            let (k, v) = guard.remove(pos).expect("position is within bounds");
            let result = v.clone();
            guard.push_front((k, v));
            return result;
        }

        // Miss: run the scan WITHOUT holding the lock.
        let result = scan_literal(snapshot, base, ident, opts, scope);

        if let Ok(mut guard) = self.inner.lock() {
            // A concurrent miss may have inserted the same key meanwhile.
            if let Some(pos) = guard.iter().position(|(k, _)| *k == key) {
                guard.remove(pos);
            }
            guard.push_front((key, result.clone()));
            while guard.len() > self.cap {
                guard.pop_back();
            }
        }

        result
    }
}

impl Default for LiteralScanCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the `loctree/find` response for `mode: "literal"`.
///
/// The AST/fuzzy buckets (symbol/param/semantic/suppression/cross) are
/// intentionally emptied: literal mode is the exact-occurrence truth layer, not
/// the semantic engine. `literal_matches` carries the same `OccurrenceResults`
/// the CLI produces; `literal_fuzzy` keeps name-similarity hints strictly
/// separate. `total_matches` reports the literal occurrence count so a caller
/// can branch on `total_matches > 0` regardless of mode.
pub fn build_literal_response(
    mut literal: OccurrenceResults,
    fuzzy: Vec<FuzzySuggestion>,
    params: &FindParams,
) -> FindResponse {
    literal.apply_report(ReportOptions {
        group_by_file: params.group_by_file,
        count_only: params.count_only || params.slim,
        offset: params.offset,
        limit: params.limit,
    });
    let total_matches = literal.total;
    FindResponse {
        query: literal.query.clone(),
        symbol_matches: single_page(SymbolSearchView {
            found: false,
            total_matches: 0,
            files: Vec::new(),
        }),
        param_matches: single_page(Vec::new()),
        semantic_matches: single_page(Vec::new()),
        suppression_matches: single_page(Vec::new()),
        cross_matches: single_page(Vec::new()),
        dead_status: DeadStatusView {
            is_exported: false,
            is_dead: false,
            dead_in_files: Vec::new(),
        },
        total_matches,
        symbol_id: params.symbol_id.clone(),
        symbol_id_version: SymbolIdV1::VERSION,
        literal_matches: Some(literal),
        literal_fuzzy: Some(fuzzy),
    }
}

fn filter_symbol_matches(
    files: Vec<SymbolFileMatch>,
    lang_filter: Option<&str>,
    exported_only: bool,
    limit: usize,
) -> SymbolSearchView {
    let mut filtered: Vec<SymbolFileMatch> = files
        .into_iter()
        .filter(|f| matches_language(&f.file, lang_filter))
        .map(|mut f| {
            if exported_only {
                f.matches
                    .retain(|m| matches!(m.kind, SymbolMatchKind::Definition));
            }
            f
        })
        .filter(|f| !f.matches.is_empty())
        .collect();

    truncate(&mut filtered, limit);
    let total: usize = filtered.iter().map(|f| f.matches.len()).sum();
    SymbolSearchView {
        found: !filtered.is_empty(),
        total_matches: total,
        files: filtered,
    }
}

fn apply_dead_only(response: &mut FindResponseRaw) {
    let dead_files: std::collections::HashSet<String> =
        response.dead_status.dead_in_files.iter().cloned().collect();
    if dead_files.is_empty() {
        response.symbol_matches = SymbolSearchView {
            found: false,
            total_matches: 0,
            files: Vec::new(),
        };
        response.param_matches.clear();
        response.semantic_matches.clear();
        response.suppression_matches.clear();
        response.cross_matches.clear();
        return;
    }

    response
        .symbol_matches
        .files
        .retain(|f| dead_files.contains(&f.file));
    response.symbol_matches.found = !response.symbol_matches.files.is_empty();
    response.symbol_matches.total_matches = response
        .symbol_matches
        .files
        .iter()
        .map(|f| f.matches.len())
        .sum();
    response
        .param_matches
        .retain(|m| dead_files.contains(&m.file));
    response
        .semantic_matches
        .retain(|c| dead_files.contains(&c.file));
    response
        .suppression_matches
        .retain(|m| dead_files.contains(&m.file));
    response
        .cross_matches
        .retain(|c| dead_files.contains(&c.file));
}

fn truncate<T>(vec: &mut Vec<T>, limit: usize) {
    if vec.len() > limit {
        vec.truncate(limit);
    }
}

fn matches_language(path: &str, lang_filter: Option<&str>) -> bool {
    let Some(expected) = lang_filter else {
        return true;
    };
    language_from_path(path) == expected
}

/// Map a language tag to the normalized language label the analyzer stores.
/// Returns `None` for unknown tags so the filter degrades to
/// "no filtering" rather than "everything dropped".
fn language_filter(lang: &str) -> Option<&'static str> {
    match lang.to_ascii_lowercase().as_str() {
        "rust" | "rs" => Some("rs"),
        "typescript" | "ts" | "tsx" => Some("ts"),
        "javascript" | "js" | "jsx" => Some("js"),
        "python" | "py" => Some("py"),
        "shell" | "sh" | "bash" | "zsh" | "fish" => Some("shell"),
        "make" | "makefile" => Some("make"),
        "go" => Some("go"),
        "svelte" => Some("svelte"),
        "astro" => Some("astro"),
        "css" => Some("css"),
        "dart" => Some("dart"),
        "zig" | "zon" => Some("zig"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_params(mode: &str) -> FindParams {
        FindParams {
            query: "Auth User".into(),
            mode: mode.into(),
            lang: None,
            file: None,
            dead_only: false,
            exported_only: false,
            limit: None,
            project: None,
            cursor: None,
            chunk_size: None,
            whole_token: false,
            group_by_file: false,
            count_only: false,
            slim: false,
            offset: 0,
            symbol_id: None,
        }
    }

    #[test]
    fn build_query_single_passes_through() {
        let params = fixture_params("single");
        assert_eq!(build_query(&params), "Auth User");
    }

    #[test]
    fn build_query_split_pipes_tokens() {
        let params = fixture_params("split");
        assert_eq!(build_query(&params), "Auth|User");
    }

    #[test]
    fn build_query_and_pipes_tokens() {
        let params = fixture_params("and");
        assert_eq!(build_query(&params), "Auth|User");
    }

    #[test]
    fn language_filter_known_languages() {
        assert_eq!(language_filter("rust"), Some("rs"));
        assert_eq!(language_filter("RS"), Some("rs"));
        assert_eq!(language_filter("typescript"), Some("ts"));
        assert_eq!(language_filter("tsx"), Some("ts"));
        assert_eq!(language_filter("python"), Some("py"));
        assert_eq!(language_filter("makefile"), Some("make"));
        assert_eq!(language_filter("klingon"), None);
    }

    #[test]
    fn matches_language_passes_when_filter_empty() {
        assert!(matches_language("anything.weird", None));
    }

    #[test]
    fn matches_language_uses_analyzer_path_classification() {
        assert!(matches_language("src/lib.rs", Some("rs")));
        assert!(!matches_language("src/lib.ts", Some("rs")));
        assert!(matches_language("Makefile", Some("make")));
    }

    #[test]
    fn find_params_accept_symbol_id_v1() {
        let raw = serde_json::json!({
            "query": "compose_runtime_slice",
            "symbol_id": "src/pack.rs::compose_runtime_slice",
        });
        let params: FindParams = serde_json::from_value(raw).unwrap();
        assert_eq!(
            params.symbol_id.as_ref().map(|id| id.as_str().to_string()),
            Some("src/pack.rs::compose_runtime_slice".to_string())
        );
    }

    #[test]
    fn find_params_omit_symbol_id_when_absent() {
        let raw = serde_json::json!({ "query": "anything" });
        let params: FindParams = serde_json::from_value(raw).unwrap();
        assert!(params.symbol_id.is_none());
    }

    #[test]
    fn find_response_advertises_symbol_id_version_label() {
        use loctree::analyzer::dead_parrots::SymbolSearchResult;
        use loctree::analyzer::search::DeadStatus;

        let mut params = fixture_params("single");
        params.symbol_id = Some(SymbolIdV1::from_parts("a.rs", "f"));
        let results = SearchResults {
            query: "f".into(),
            symbol_matches: SymbolSearchResult {
                found: false,
                total_matches: 0,
                files: Vec::new(),
            },
            param_matches: Vec::new(),
            semantic_matches: Vec::new(),
            dead_status: DeadStatus {
                is_exported: false,
                is_dead: false,
                dead_in_files: Vec::new(),
            },
            suppression_matches: Vec::new(),
            cross_matches: Vec::new(),
        };
        let response = build_response(results, &params);
        assert_eq!(response.symbol_id_version, "v1-string");
        assert_eq!(
            response
                .symbol_id
                .as_ref()
                .map(|id| id.as_str().to_string()),
            Some("a.rs::f".to_string())
        );
    }
}
