//! Per-document live AST cache for the LSP daemon.
//!
//! Plan 17 — wires the [`loctree-ast`] substrate (Plan 16) into the LSP edit
//! lifecycle. Each open JS/TS/TSX document keeps a live tree-sitter tree keyed
//! by URI; on every `textDocument/didChange` the store applies tree-sitter
//! [`InputEdit`]s and reparses incrementally so subsequent reads see the buffer
//! the editor is showing — without rescanning the workspace.
//!
//! ## Boundary
//!
//! - **Sync mode**: `INCREMENTAL`. Each `TextDocumentContentChangeEvent` is
//!   translated to a tree-sitter [`InputEdit`] over the previous content, then
//!   handed to [`Parsers::parse_incremental`]. Range-less events (full
//!   document replacement) still fall through the full-parse path so the
//!   daemon stays compatible with `change.text`-only servers.
//! - **Per-language scope**: only the languages exposed by
//!   [`loctree_ast::Parsers::new_default`] (`javascript`, `typescript`, `tsx`,
//!   `python`).
//!   Files outside that set silently bypass the store — they don't get a live
//!   tree and `loctree/documentChanged` is not emitted for them.
//! - **No symbol extractors**: the per-document `exports` / `imports` slice
//!   from the original Plan 17 sketch depends on per-language extractors
//!   that belong to Plan 19. The notification carries `lang`, `version`,
//!   `has_error`, `root_kind`, and a parse-duration probe — the smallest
//!   credible signal that the daemon parsed the buffer the editor showed.
//! - **Position encoding**: LSP defaults to UTF-16 code units. The translator
//!   walks `prev_content` line by line and converts `Position(line, char)`
//!   into a UTF-16 code-unit prefix on the matching line, then derives a
//!   byte offset from the same UTF-8 line buffer. Servers that negotiate a
//!   different encoding via `general.positionEncodings` should plumb that
//!   through — for now we follow the spec default.
//!
//! Consumers (currently [`crate::ast_query`]) can ask the store for a tree
//! via [`LiveAstStore::get_for_path`] and receive `None` when no open document
//! corresponds to the requested workspace-relative path. The fallback path
//! through `loctree-ast` reparsing the on-disk file stays as the safety net
//! for closed documents and unsupported languages.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use dashmap::DashMap;
use loctree::types::SymbolIdV1;
use loctree_ast::{InputEdit, LoctreeTree, Parsers, Point, Query, QueryCursor, StreamingIterator};
use serde::{Deserialize, Serialize};
use tower_lsp::lsp_types::notification::Notification;
use tower_lsp::lsp_types::{Position, Range, TextDocumentContentChangeEvent, Url};

/// One open document tracked by the live AST store.
///
/// Wrapped in [`Arc`] inside the store so concurrent readers (the LSP
/// request handlers) can hold a snapshot of the tree while a fresh
/// `did_change` parse swaps the entry — no reader-writer lock needed.
pub struct LiveDocument {
    /// Loctree-AST tree + source bytes + canonical language id.
    pub tree: LoctreeTree,
    /// LSP document version reported by the client. -1 when the client
    /// did not provide one (older protocol versions).
    pub version: i32,
    /// Wall-clock duration of the most recent parse. Surfaced via the
    /// `loctree/documentChanged` notification so agents can spot
    /// pathological inputs without instrumenting the daemon themselves.
    pub parse_duration_ms: f64,
    /// UTF-8 source content kept alongside the tree so range-based
    /// `TextDocumentContentChangeEvent`s can be translated into
    /// tree-sitter [`InputEdit`]s without rebuilding the buffer from
    /// disk. Always equals `String::from_utf8_lossy(&tree.source)` —
    /// stored as a separate `String` so range translation can splice
    /// without unnecessary allocations.
    pub content: String,
}

// `LoctreeTree` does not implement `Debug`, so emit a redacted
// summary that still gives operators enough to spot lifecycle issues
// at log time.
impl std::fmt::Debug for LiveDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveDocument")
            .field("lang", &self.tree.lang)
            .field("version", &self.version)
            .field("source_bytes", &self.tree.source.len())
            .field("parse_duration_ms", &self.parse_duration_ms)
            .finish()
    }
}

impl LiveDocument {
    /// Convenience accessor: workspace-relative path string derived from
    /// `uri` against `workspace_root`. Returns `None` when the URI is
    /// not a file URL or escapes the workspace root.
    pub fn workspace_relative(uri: &Url, workspace_root: &Path) -> Option<String> {
        let abs = uri.to_file_path().ok()?;
        let rel = abs.strip_prefix(workspace_root).ok()?;
        Some(rel.to_string_lossy().replace('\\', "/"))
    }
}

/// Convert an LSP [`Position`] to a `(byte_offset, Point)` pair against
/// `content`. Returns `None` when `position.line` exceeds the line
/// count or `position.character` is past the end of the line.
///
/// Encoding: LSP `character` is UTF-16 code units (the protocol
/// default). Non-BMP characters take 2 code units, so we walk the
/// matching line UTF-8 char by char and accumulate UTF-16 units until
/// we hit `character`, then read the byte offset.
fn position_to_byte(content: &str, position: Position) -> Option<(usize, Point)> {
    let target_line = position.line as usize;
    let target_char = position.character as usize;

    // Locate the start byte of `target_line`.
    let mut byte_offset = 0usize;
    let mut line_idx = 0usize;
    for line in content.split_inclusive('\n') {
        if line_idx == target_line {
            // Walk UTF-16 code units until we hit `target_char`.
            let mut utf16_cursor = 0usize;
            for (rel_byte, ch) in line.char_indices() {
                if utf16_cursor == target_char {
                    return Some((
                        byte_offset + rel_byte,
                        Point {
                            row: target_line,
                            column: rel_byte,
                        },
                    ));
                }
                utf16_cursor += ch.len_utf16();
                if utf16_cursor > target_char {
                    // Position pointed inside a surrogate pair — fall
                    // back to the next char boundary so the edit stays
                    // on a UTF-8 boundary.
                    return Some((
                        byte_offset + rel_byte + ch.len_utf8(),
                        Point {
                            row: target_line,
                            column: rel_byte + ch.len_utf8(),
                        },
                    ));
                }
            }
            // Position equals end-of-line / end-of-document.
            // `split_inclusive` keeps trailing `\n` in `line.len()`;
            // for the EOL position we want the offset *before* the
            // newline, which equals `line.len() - newline_bytes`.
            let trailing = if line.ends_with("\r\n") {
                2
            } else if line.ends_with('\n') {
                1
            } else {
                0
            };
            let line_len_no_newline = line.len() - trailing;
            return Some((
                byte_offset + line_len_no_newline,
                Point {
                    row: target_line,
                    column: line_len_no_newline,
                },
            ));
        }
        byte_offset += line.len();
        line_idx += 1;
    }

    // Past the last line: clients can address `Position { line: lines, character: 0 }`
    // as the end-of-document anchor. Accept it; reject anything beyond.
    if target_line == line_idx && target_char == 0 {
        Some((
            byte_offset,
            Point {
                row: target_line,
                column: 0,
            },
        ))
    } else {
        None
    }
}

/// Translate a single `TextDocumentContentChangeEvent` against the
/// previous full content, returning the matching tree-sitter
/// [`InputEdit`] and the post-edit content slice the next event must
/// reason against.
///
/// Returns `None` when the event has no `range` (full-document replace)
/// or when the positions cannot be resolved against `prev_content`.
/// Callers should treat `None` as "fall back to a full reparse".
pub fn translate_change_event(
    prev_content: &str,
    event: &TextDocumentContentChangeEvent,
) -> Option<(InputEdit, String)> {
    let Range { start, end } = event.range?;
    let (start_byte, start_position) = position_to_byte(prev_content, start)?;
    let (old_end_byte, old_end_position) = position_to_byte(prev_content, end)?;

    if old_end_byte < start_byte {
        // Defensive: client sent end before start. Tree-sitter would
        // panic on a negative range; bail out so the caller can fall
        // back to a full reparse.
        return None;
    }

    let new_text_bytes = event.text.as_bytes();
    let new_end_byte = start_byte + new_text_bytes.len();

    // Compute the new end Point by walking the inserted text.
    let mut row = start_position.row;
    let mut column = start_position.column;
    for ch in event.text.chars() {
        if ch == '\n' {
            row += 1;
            column = 0;
        } else {
            column += ch.len_utf8();
        }
    }
    let new_end_position = Point { row, column };

    let mut new_content = String::with_capacity(prev_content.len() + new_text_bytes.len());
    new_content.push_str(&prev_content[..start_byte]);
    new_content.push_str(&event.text);
    new_content.push_str(&prev_content[old_end_byte..]);

    let edit = InputEdit {
        start_byte,
        old_end_byte,
        new_end_byte,
        start_position,
        old_end_position,
        new_end_position,
    };

    Some((edit, new_content))
}

/// Translate a sequence of LSP change events against `prev_content`,
/// returning the accumulated [`InputEdit`]s and the final post-edit
/// content. Returns `None` if **any** event lacks a range — the caller
/// should treat the last event's `text` as the full new document and
/// fall through to a non-incremental parse, matching the LSP spec
/// (range-less events imply full-document replacement).
pub fn translate_change_events(
    prev_content: &str,
    events: &[TextDocumentContentChangeEvent],
) -> Option<(Vec<InputEdit>, String)> {
    let mut edits = Vec::with_capacity(events.len());
    let mut current = std::borrow::Cow::Borrowed(prev_content);
    for event in events {
        let (edit, next) = translate_change_event(&current, event)?;
        edits.push(edit);
        current = std::borrow::Cow::Owned(next);
    }
    Some((edits, current.into_owned()))
}

/// Concurrent map of open document URIs to their live tree-sitter state.
///
/// Holds an internal [`Arc<Parsers>`] so parsing is dispatched through
/// the same registry the cold-scan fallback uses; new languages added to
/// `loctree-ast::Parsers::new_default` automatically become eligible for
/// the live cache without changes to this layer.
#[derive(Clone)]
pub struct LiveAstStore {
    documents: Arc<DashMap<Url, Arc<LiveDocument>>>,
    parsers: Arc<Parsers>,
}

impl Default for LiveAstStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for LiveAstStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveAstStore")
            .field("open_documents", &self.documents.len())
            .field("languages", &self.parsers.language_ids())
            .finish()
    }
}

impl LiveAstStore {
    pub fn new() -> Self {
        Self {
            documents: Arc::new(DashMap::new()),
            parsers: Arc::new(Parsers::new_default()),
        }
    }

    /// Languages eligible for live parsing — directly from the
    /// [`Parsers`] registry so the LSP capability advertisement stays in
    /// step with what the substrate can actually parse.
    pub fn languages(&self) -> Vec<&'static str> {
        self.parsers.language_ids()
    }

    /// Whether the underlying `loctree-ast` registry knows the
    /// extension of `path`. Used by [`Self::update`] for an early-out
    /// before allocating source bytes, and exposed for tests / probe
    /// callers.
    pub fn supports_path(&self, path: &Path) -> bool {
        self.parsers.for_path(path).is_some()
    }

    /// Parse `source` for `uri` and store the resulting tree. Used by
    /// `did_open` / `did_save` and by INCREMENTAL fall-through when an
    /// event lacks a `range`. Returns:
    ///
    /// - `Some(DocumentChanged)` when the URI mapped to a supported
    ///   language and the parse succeeded — the caller should forward
    ///   the payload to clients via [`LoctreeDocumentChanged`].
    /// - `None` when the URI is not a file URL, the extension is not
    ///   in [`Parsers::new_default`], or tree-sitter rejected the
    ///   buffer. The daemon stays silent rather than emitting a
    ///   notification it cannot back with real AST data.
    pub fn update(&self, uri: &Url, version: i32, source: &str) -> Option<DocumentChanged> {
        let path = uri.to_file_path().ok()?;
        let parser = self.parsers.for_path(&path)?;
        let lang: &'static str = parser.lang_id();

        // Full reparse path. Reuse the previous tree as an incremental
        // hint when present so tree-sitter can copy unchanged subtrees;
        // empty edit list means "reparse from scratch but with the
        // previous tree as a starting point".
        let started = Instant::now();
        let tree = match self.documents.get(uri) {
            Some(prev) if prev.tree.lang == lang => self
                .parsers
                .parse_incremental(&prev.tree, source.as_bytes(), &[])
                .ok()?,
            _ => self.parsers.parse(parser, source.as_bytes()).ok()?,
        };
        let parse_duration_ms = started.elapsed().as_secs_f64() * 1000.0;

        let has_error = tree.has_error();
        let root_kind = tree.root_kind().to_string();

        let document = Arc::new(LiveDocument {
            tree,
            version,
            parse_duration_ms,
            content: source.to_string(),
        });
        self.documents.insert(uri.clone(), document);

        Some(DocumentChanged {
            uri: uri.clone(),
            lang: lang.to_string(),
            version,
            has_error,
            root_kind,
            parse_duration_ms,
        })
    }

    /// Apply a sequence of LSP `TextDocumentContentChangeEvent`s
    /// (`INCREMENTAL` sync) to the cached tree for `uri`.
    ///
    /// Falls back to [`Self::update`] when:
    /// - no live tree exists for `uri` (first event for the document);
    /// - any event has `range == None` (LSP spec: full-document replace).
    ///   In that case the *last* event's `text` is the new authoritative
    ///   buffer.
    /// - the edit positions cannot be resolved against `prev_content`
    ///   (out-of-range — likely a client bug; we recover by reparsing).
    ///
    /// Returns the same `DocumentChanged` payload shape as
    /// [`Self::update`], so `did_change` can forward it unchanged.
    pub fn apply_change(
        &self,
        uri: &Url,
        version: i32,
        events: &[TextDocumentContentChangeEvent],
    ) -> Option<DocumentChanged> {
        if events.is_empty() {
            return None;
        }

        // If any event is range-less, the spec says the last `text`
        // wins as the full document. Fall back to a full reparse.
        if events.iter().any(|e| e.range.is_none()) {
            let last_text = events.iter().rev().find_map(|e| {
                if e.range.is_none() {
                    Some(e.text.as_str())
                } else {
                    None
                }
            })?;
            return self.update(uri, version, last_text);
        }

        // Need an existing live document to compute byte offsets
        // against. If the editor sent didChange before didOpen (rare,
        // but possible during state recovery), fall back to a full
        // parse using the joined event texts.
        let prev = match self.documents.get(uri) {
            Some(entry) => entry.clone(),
            None => {
                // Best-effort recovery: treat the joined event payload
                // as the document. The client should have sent
                // didOpen first; emitting nothing would silently desync
                // the cache.
                let mut joined = String::new();
                for e in events {
                    joined.push_str(&e.text);
                }
                return self.update(uri, version, &joined);
            }
        };

        let path = match uri.to_file_path() {
            Ok(p) => p,
            Err(_) => return None,
        };
        let parser = self.parsers.for_path(&path)?;
        let lang: &'static str = parser.lang_id();

        let Some((edits, new_content)) = translate_change_events(&prev.content, events) else {
            // Translation failed — recover by treating the last event's
            // text as the post-edit buffer (best-effort) and reparsing
            // from scratch. Better than emitting a desynced tree.
            let last_text = events.last().map(|e| e.text.clone()).unwrap_or_default();
            return self.update(uri, version, &last_text);
        };

        let started = Instant::now();
        let tree = self
            .parsers
            .parse_incremental(&prev.tree, new_content.as_bytes(), &edits)
            .ok()?;
        let parse_duration_ms = started.elapsed().as_secs_f64() * 1000.0;

        let has_error = tree.has_error();
        let root_kind = tree.root_kind().to_string();

        let document = Arc::new(LiveDocument {
            tree,
            version,
            parse_duration_ms,
            content: new_content,
        });
        self.documents.insert(uri.clone(), document);

        Some(DocumentChanged {
            uri: uri.clone(),
            lang: lang.to_string(),
            version,
            has_error,
            root_kind,
            parse_duration_ms,
        })
    }

    /// Drop the cached tree for `uri`. Returns true when an entry was
    /// actually removed; false on a no-op close (e.g. unsupported
    /// language that never made it into the store).
    pub fn remove(&self, uri: &Url) -> bool {
        self.documents.remove(uri).is_some()
    }

    /// Look up the cached document for `uri`. Cheap clone — the inner
    /// payload is in [`Arc`]. Returns `None` when the document was not
    /// open or the most recent parse failed.
    pub fn get(&self, uri: &Url) -> Option<Arc<LiveDocument>> {
        self.documents.get(uri).map(|entry| entry.clone())
    }

    /// Look up a cached document by its workspace-relative path
    /// (`src/foo.ts`, `tests/bar.tsx`, …) under `workspace_root`.
    ///
    /// Used by [`crate::ast_query`] to route file-scoped query
    /// execution at the live tree when the editor has the file open
    /// without a save. Falls back to `None` when the path is not open
    /// or the URI cannot be reconstructed.
    pub fn get_for_path(&self, workspace_root: &Path, rel_path: &str) -> Option<Arc<LiveDocument>> {
        let abs = workspace_root.join(rel_path);
        let uri = Url::from_file_path(&abs).ok()?;
        self.get(&uri)
    }

    /// Number of open documents currently tracked. Test affordance.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    /// Whether there are no open documents currently tracked.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    /// Build the structured payload returned in the
    /// `experimental.loctree/documentChanged` capability slot.
    pub fn capability_json(&self) -> serde_json::Value {
        serde_json::json!({
            "available": true,
            "languages": self.languages(),
            "sync_mode": "incremental",
            "incremental_edits": true,
            "position_encoding": "utf-16",
            "extractors": false,
            "extractors_reason": "per-language exports/imports extractors are Plan 19 — the \
                                  notification carries `lang`/`version`/`has_error`/`root_kind` \
                                  + `parse_duration_ms` only until extractors land."
        })
    }

    /// Drop every cached document. Test affordance + `shutdown` hook.
    pub fn clear(&self) {
        self.documents.clear();
    }
}

/// Payload emitted on every successful live parse. The shape is the
/// minimum credible signal of "the daemon parsed the buffer the editor
/// showed" — extractor-driven diff fields (`exports_added`, …) join the
/// payload when Plan 19 lands and stay backwards-compatible because
/// they are additive serde fields.
///
/// `lang` is owned [`String`] (not `&'static str`) so `tower_lsp`'s
/// notification trait — which requires `DeserializeOwned` — can round
/// trip the payload on integration tests / mock clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentChanged {
    pub uri: Url,
    pub lang: String,
    pub version: i32,
    pub has_error: bool,
    pub root_kind: String,
    pub parse_duration_ms: f64,
}

/// `loctree/documentChanged` notification handle.
pub enum LoctreeDocumentChanged {}

impl Notification for LoctreeDocumentChanged {
    const METHOD: &'static str = "loctree/documentChanged";
    type Params = DocumentChanged;
}

// ----------------------------------------------------------------------
// Plan 18 v2 — symbol-level granularity surface
// ----------------------------------------------------------------------

/// One top-level symbol extracted from a live tree-sitter tree. Plan 18
/// v2 minimum-viable shape: `(name, kind, byte_range)` is enough to
/// classify added / removed / moved / rewritten without a full
/// per-language extractor.
///
/// Plan 19 will replace the inline tree walker that produces these with
/// a `LangExtractor::extract_exports` trait surface that also fills in
/// imports, re-exports, and type-level kinds. Until then this carries
/// only function and class declarations — see the doc comment on
/// [`extract_live_symbols`] for the heuristic boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSymbol {
    /// Identifier captured from the declaration node.
    pub name: String,
    /// Symbol kind: `"function"` or `"class"` in Plan 18 v2.
    pub kind: String,
    /// Inclusive-exclusive byte offsets of the declaration node in the
    /// source buffer. Used both as the symbol's location and as input
    /// to [`SymbolIdV2`](loctree::types::SymbolIdV2) hashing.
    pub byte_range: (usize, usize),
    /// 0-based line number of the declaration's start (so the
    /// notification can carry `from.line` / `to.line` without the
    /// client recomputing it from byte offsets).
    pub line: usize,
    /// `DefaultHasher` over the declaration's source bytes. Used by
    /// the diff classifier to suppress noisy `moved` events that fire
    /// purely because a sibling rename shifted everything below it —
    /// when the body bytes are byte-identical the symbol is treated
    /// as unchanged regardless of where it landed in the buffer.
    pub body_hash: u64,
}

/// Per-symbol metadata cached on the LSP backend across edits. The
/// backend stores `HashMap<Url, HashMap<SymbolIdV1, SymbolMetadata>>`
/// keyed by file URI; on every INCREMENTAL `did_change` the live
/// extractor produces a fresh map and [`diff_symbol_sets`] classifies
/// the delta.
#[derive(Debug, Clone)]
pub struct SymbolMetadata {
    /// Most recent observation time (used for stale-cache eviction in
    /// future revisions; currently informational).
    pub last_seen: SystemTime,
    /// Inclusive-exclusive byte offsets of the symbol's declaration
    /// node in the most recent successful parse.
    pub byte_range: Option<(usize, usize)>,
    /// Tree-sitter node id (kept opt-in because the substrate ids are
    /// not stable across reparses; reserved for v3 tracking when a
    /// stable assignment scheme lands).
    pub ast_node_id: Option<u64>,
    /// Previous byte ranges observed for this id, newest-first. Plan
    /// 18 v2 keeps the last 4 entries — enough for the move-trail
    /// query `loctree/symbolHistory` Plan 19+ will surface.
    pub prev_locations: Vec<(usize, usize)>,
    /// Hash of the symbol's declaration source bytes from the most
    /// recent observation. Used by [`diff_symbol_sets`] to suppress
    /// downstream-shift noise when only the offset changed.
    pub body_hash: u64,
}

impl SymbolMetadata {
    fn from_symbol(sym: &LiveSymbol) -> Self {
        Self {
            last_seen: SystemTime::now(),
            byte_range: Some(sym.byte_range),
            ast_node_id: None,
            prev_locations: Vec::new(),
            body_hash: sym.body_hash,
        }
    }

    fn record_move(&mut self, prev_range: (usize, usize), new_range: (usize, usize)) {
        self.prev_locations.insert(0, prev_range);
        if self.prev_locations.len() > 4 {
            self.prev_locations.truncate(4);
        }
        self.byte_range = Some(new_range);
        self.last_seen = SystemTime::now();
    }
}

/// Hash a declaration's source bytes via [`DefaultHasher`]. Pure
/// helper used by [`extract_live_symbols`] and the diff classifier.
fn hash_body(source: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

/// Classification of a single symbol-level edit. The kind set matches
/// the capability advertisement so clients can rely on the four labels
/// without probing.
///
/// **Heuristic, not semantic.** The classifier is purely syntactic — it
/// keys on `(name, start_byte)` against the previous parse. `start_byte`
/// is the location anchor (where the declaration begins) — far more
/// stable than the full `byte_range` because renames change the
/// declaration's length but leave its starting offset intact.
///
/// | Previous                   | Current                    | Kind        |
/// |----------------------------|----------------------------|-------------|
/// | name=N, start=S, range=R   | name=N, start=S, range=R   | (no event)  |
/// | name=N, start=S            | name=N, start=S', range≠   | `moved`     |
/// | name=N, start=S            | name=M, start=S            | `rewritten` |
/// | (absent)                   | name=N, start=S            | `added`     |
/// | name=N, start=S            | (absent)                   | `removed`   |
///
/// Two symbols with identical bodies but different names produce
/// `rewritten` (same start, new name); two symbols whose name and body
/// are unchanged but whose start byte shifted produce `moved`. Plan 18
/// v3 will add cross-file moves and semantic-equivalence dedup; until
/// then, callers must treat the classification as a syntactic hint,
/// not ground truth.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SymbolChangeKind {
    Added,
    Removed,
    Moved,
    Rewritten,
}

impl SymbolChangeKind {
    /// Stable string label for the capability advertisement and the
    /// notification payload. Wire shape matches the JSON `rename_all`.
    pub const fn as_str(&self) -> &'static str {
        match self {
            SymbolChangeKind::Added => "added",
            SymbolChangeKind::Removed => "removed",
            SymbolChangeKind::Moved => "moved",
            SymbolChangeKind::Rewritten => "rewritten",
        }
    }

    /// Full set of kinds advertised by the capability JSON. Order
    /// matches the heuristic table in the type-level doc.
    pub const ALL: [&'static str; 4] = ["added", "removed", "moved", "rewritten"];
}

/// Endpoint of a symbol change — carries the byte range and 0-based
/// line so clients don't need to recompute either from the URI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SymbolChangeLocation {
    pub byte_range: (usize, usize),
    pub line: usize,
}

/// One classified change item in the [`SymbolChanged`] notification.
/// `from` is `None` for `added`; `to` is `None` for `removed`;
/// both are populated for `moved` / `rewritten`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SymbolChange {
    /// V1 string id (`<file>::<symbol>`) that the change is keyed on.
    pub id: SymbolIdV1,
    pub kind: SymbolChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<SymbolChangeLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<SymbolChangeLocation>,
}

/// Payload of `loctree/symbolChanged`. Emitted at most once per
/// `did_change` when the symbol set diff is non-empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolChanged {
    pub uri: Url,
    pub version: i32,
    pub changes: Vec<SymbolChange>,
}

/// `loctree/symbolChanged` notification handle.
pub enum LoctreeSymbolChanged {}

impl Notification for LoctreeSymbolChanged {
    const METHOD: &'static str = "loctree/symbolChanged";
    type Params = SymbolChanged;
}

/// Walk the live tree-sitter tree and collect top-level function /
/// class declarations as [`LiveSymbol`]s.
///
/// **v1 minimum extraction (Plan 18 v2).** Uses tree-sitter
/// [`Query`] over the live tree to capture `function_declaration` and
/// `class_declaration` (TS/JS) plus the TS-only `abstract_class_declaration`.
/// Imports, re-exports, type aliases, and inner symbols stay invisible
/// until Plan 19 lands proper `LangExtractor::extract_exports` per
/// language.
///
/// TODO(plan-19): replace this walker with the lang-specific extractor
/// once the trait surface lands. The notification kinds and the cache
/// shape stay stable across that swap.
pub fn extract_live_symbols(tree: &LoctreeTree) -> Vec<LiveSymbol> {
    let parsers = Parsers::new_default();
    let Some(parser) = parsers.lookup(tree.lang) else {
        return Vec::new();
    };
    let language = parser.language();
    let source = tree.source.as_slice();

    // Per-language query — TypeScript / TSX have abstract_class_declaration,
    // JavaScript does not.
    let query_text = match tree.lang {
        "javascript" => {
            "(function_declaration name: (identifier) @decl_name) @decl
             (generator_function_declaration name: (identifier) @decl_name) @decl
             (class_declaration name: (identifier) @decl_name) @decl"
        }
        "typescript" | "tsx" => {
            "(function_declaration name: (identifier) @decl_name) @decl
             (generator_function_declaration name: (identifier) @decl_name) @decl
             (function_signature name: (identifier) @decl_name) @decl
             (class_declaration name: (type_identifier) @decl_name) @decl
             (abstract_class_declaration name: (type_identifier) @decl_name) @decl"
        }
        _ => return Vec::new(),
    };

    let Ok(query) = Query::new(&language, query_text) else {
        return Vec::new();
    };
    let capture_names = query.capture_names();
    let decl_idx = capture_names.iter().position(|n| *n == "decl");
    let name_idx = capture_names.iter().position(|n| *n == "decl_name");
    let (Some(decl_idx), Some(name_idx)) = (decl_idx, name_idx) else {
        return Vec::new();
    };

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.tree.root_node(), source);
    let mut out = Vec::new();
    while let Some(m) = matches.next() {
        let mut decl_range: Option<(usize, usize, usize)> = None;
        let mut decl_name: Option<String> = None;
        let mut decl_kind: Option<&'static str> = None;
        for cap in m.captures {
            if cap.index as usize == decl_idx {
                let node = cap.node;
                let r = node.byte_range();
                decl_range = Some((r.start, r.end, node.start_position().row));
                decl_kind = Some(match node.kind() {
                    "class_declaration" | "abstract_class_declaration" => "class",
                    _ => "function",
                });
            } else if cap.index as usize == name_idx {
                let r = cap.node.byte_range();
                if let Ok(name) = std::str::from_utf8(&source[r]) {
                    decl_name = Some(name.to_string());
                }
            }
        }
        if let (Some((start, end, row)), Some(name), Some(kind)) =
            (decl_range, decl_name, decl_kind)
        {
            let body_hash = hash_body(&source[start..end]);
            out.push(LiveSymbol {
                name,
                kind: kind.to_string(),
                byte_range: (start, end),
                line: row,
                body_hash,
            });
        }
    }
    out
}

/// Build the `(name, kind, byte_range)` map keyed by [`SymbolIdV1`] for
/// a workspace-relative file path.
pub fn build_symbol_map(
    file_path: &str,
    symbols: &[LiveSymbol],
) -> HashMap<SymbolIdV1, LiveSymbol> {
    let mut map = HashMap::with_capacity(symbols.len());
    for sym in symbols {
        let id = SymbolIdV1::from_parts(file_path, &sym.name);
        map.insert(id, sym.clone());
    }
    map
}

/// Classify the delta between a previous and a current symbol set for a
/// single file URI. Returns the change list plus the updated metadata
/// map the caller should commit back to its workspace tracker.
///
/// `prev` may be `None` for the first parse of a file; in that case
/// every current symbol is reported as `added`. Symbols present in
/// `prev` but missing from `current` are reported as `removed`.
///
/// The classifier keys on the **start byte** of each declaration
/// (`byte_range.0`) — that's the location anchor that survives renames
/// (which change the declaration's length but not its start offset).
/// Concretely:
///   - id (file::name) present in both: same start_byte → unchanged
///     (no event); different start_byte → `moved`.
///   - id missing in current but a current symbol with a *different*
///     name shares the prev start_byte → `rewritten`. The old id is
///     consumed and not additionally reported as `removed`.
///   - new id whose start_byte doesn't match any prev start_byte →
///     `added`.
///   - prev id missing entirely → `removed`.
pub fn diff_symbol_sets(
    file_path: &str,
    prev: Option<&HashMap<SymbolIdV1, SymbolMetadata>>,
    current: &HashMap<SymbolIdV1, LiveSymbol>,
) -> (Vec<SymbolChange>, HashMap<SymbolIdV1, SymbolMetadata>) {
    let mut changes: Vec<SymbolChange> = Vec::new();
    let mut next_metadata: HashMap<SymbolIdV1, SymbolMetadata> = HashMap::new();

    let empty_prev: HashMap<SymbolIdV1, SymbolMetadata> = HashMap::new();
    let prev = prev.unwrap_or(&empty_prev);

    // Reverse lookup `start_byte → prev_id`. Used to detect `rewritten`
    // (same start, different name).
    let mut prev_start_to_id: HashMap<usize, SymbolIdV1> = HashMap::new();
    for (id, meta) in prev.iter() {
        if let Some(range) = meta.byte_range {
            prev_start_to_id.insert(range.0, id.clone());
        }
    }

    let mut consumed_prev: std::collections::HashSet<SymbolIdV1> = std::collections::HashSet::new();

    for (id, sym) in current.iter() {
        let new_range = sym.byte_range;
        match prev.get(id) {
            Some(prev_meta) => {
                // Same id — same name. Either unchanged (same start)
                // or moved.
                let prev_range = prev_meta.byte_range;
                consumed_prev.insert(id.clone());
                let mut meta = prev_meta.clone();
                if prev_range == Some(new_range) {
                    meta.last_seen = SystemTime::now();
                    meta.body_hash = sym.body_hash;
                    next_metadata.insert(id.clone(), meta);
                    continue;
                }
                // Body-bytes unchanged: a sibling rename / line insert
                // shifted the offsets but this symbol's own bytes are
                // identical. Treat as unchanged so renames don't fire
                // a cascade of `moved` events for every sibling below.
                if prev_meta.body_hash == sym.body_hash {
                    meta.byte_range = Some(new_range);
                    meta.last_seen = SystemTime::now();
                    next_metadata.insert(id.clone(), meta);
                    continue;
                }
                let prev_start = prev_range.map(|r| r.0);
                if prev_start == Some(new_range.0) {
                    // Body changed but anchor didn't move — silent
                    // metadata refresh, no notification (identity by
                    // anchor is stable).
                    meta.byte_range = Some(new_range);
                    meta.last_seen = SystemTime::now();
                    meta.body_hash = sym.body_hash;
                    next_metadata.insert(id.clone(), meta);
                    continue;
                }
                if let Some(prev_range) = prev_range {
                    meta.record_move(prev_range, new_range);
                } else {
                    meta.byte_range = Some(new_range);
                    meta.last_seen = SystemTime::now();
                }
                meta.body_hash = sym.body_hash;
                next_metadata.insert(id.clone(), meta);
                changes.push(SymbolChange {
                    id: id.clone(),
                    kind: SymbolChangeKind::Moved,
                    from: prev_range.map(|r| SymbolChangeLocation {
                        byte_range: r,
                        line: 0, // prev line not tracked in v2.
                    }),
                    to: Some(SymbolChangeLocation {
                        byte_range: new_range,
                        line: sym.line,
                    }),
                });
            }
            None => {
                // New id. If a prev id shared the same start byte but
                // had a different name → `rewritten`. Else `added`.
                if let Some(prev_id) = prev_start_to_id.get(&new_range.0)
                    && !consumed_prev.contains(prev_id)
                {
                    consumed_prev.insert(prev_id.clone());
                    let prev_range = prev.get(prev_id).and_then(|meta| meta.byte_range);
                    next_metadata.insert(id.clone(), SymbolMetadata::from_symbol(sym));
                    changes.push(SymbolChange {
                        id: id.clone(),
                        kind: SymbolChangeKind::Rewritten,
                        from: prev_range.map(|r| SymbolChangeLocation {
                            byte_range: r,
                            line: 0,
                        }),
                        to: Some(SymbolChangeLocation {
                            byte_range: new_range,
                            line: sym.line,
                        }),
                    });
                    continue;
                }
                next_metadata.insert(id.clone(), SymbolMetadata::from_symbol(sym));
                changes.push(SymbolChange {
                    id: id.clone(),
                    kind: SymbolChangeKind::Added,
                    from: None,
                    to: Some(SymbolChangeLocation {
                        byte_range: new_range,
                        line: sym.line,
                    }),
                });
            }
        }
    }

    // Anything in prev but not in current and not consumed by a
    // `rewritten` classification is `removed`.
    for (id, meta) in prev.iter() {
        if consumed_prev.contains(id) {
            continue;
        }
        if current.contains_key(id) {
            continue;
        }
        changes.push(SymbolChange {
            id: id.clone(),
            kind: SymbolChangeKind::Removed,
            from: meta.byte_range.map(|r| SymbolChangeLocation {
                byte_range: r,
                line: 0,
            }),
            to: None,
        });
    }

    let _ = file_path; // file_path reserved for v3 cross-file move tracking.
    (changes, next_metadata)
}

/// Capability JSON for `loctree/symbolChanged`. Plan 18 v2 flips
/// `available` from `false` to `true` and advertises the four diff
/// kinds the classifier emits.
pub fn symbol_changed_capability_json() -> serde_json::Value {
    serde_json::json!({
        "available": true,
        "kinds": SymbolChangeKind::ALL,
        "version": SymbolIdV1::VERSION,
        "scope": "open_documents_js_ts_tsx",
        "extraction": "v1-minimum",
        "extractionNote": "Plan 18 v2 walks function_declaration / class_declaration / \
                          abstract_class_declaration top-level nodes only. Plan 19's \
                          LangExtractor will replace the walker with full per-language \
                          extractor coverage; the notification shape stays stable.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn uri_for(path: &Path) -> Url {
        Url::from_file_path(path).expect("file URL")
    }

    #[test]
    fn supports_typescript_javascript_tsx() {
        let store = LiveAstStore::new();
        let langs = store.languages();
        assert!(langs.contains(&"javascript"));
        assert!(langs.contains(&"typescript"));
        assert!(langs.contains(&"tsx"));
    }

    #[test]
    fn supports_path_only_for_known_extensions() {
        let store = LiveAstStore::new();
        assert!(store.supports_path(Path::new("/tmp/a.ts")));
        assert!(store.supports_path(Path::new("/tmp/a.tsx")));
        assert!(store.supports_path(Path::new("/tmp/a.js")));
        // Python joined Parsers::new_default (loctree-ast), so the LSP auto-derives
        // .py/.pyi support from parsers.language_ids().
        assert!(store.supports_path(Path::new("/tmp/a.py")));
        assert!(store.supports_path(Path::new("/tmp/a.pyi")));
        // Still genuinely unsupported (no parser registered).
        assert!(!store.supports_path(Path::new("/tmp/a.rs")));
    }

    #[test]
    fn update_on_supported_file_emits_change_payload() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("demo.ts");
        let uri = uri_for(&path);

        let store = LiveAstStore::new();
        let payload = store
            .update(&uri, 1, "export const answer: number = 42;\n")
            .expect("emits payload");

        assert_eq!(payload.lang.as_str(), "typescript");
        assert_eq!(payload.version, 1);
        assert!(!payload.has_error, "valid TS must not parse with errors");
        assert!(!payload.root_kind.is_empty());
        assert!(payload.parse_duration_ms >= 0.0);
        assert_eq!(store.len(), 1);

        let cached = store.get(&uri).expect("cached document");
        assert_eq!(cached.tree.lang, "typescript");
        assert_eq!(cached.version, 1);
    }

    #[test]
    fn update_replaces_previous_version() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("demo.ts");
        let uri = uri_for(&path);

        let store = LiveAstStore::new();
        store
            .update(&uri, 1, "export const a = 1;\n")
            .expect("first parse");
        let payload = store
            .update(&uri, 7, "export const a = 1;\nexport const b = 2;\n")
            .expect("second parse");

        assert_eq!(payload.version, 7);
        assert_eq!(store.len(), 1);
        let doc = store.get(&uri).expect("doc");
        assert_eq!(doc.version, 7);
    }

    #[test]
    fn update_on_unsupported_extension_returns_none() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("demo.rs");
        let uri = uri_for(&path);

        let store = LiveAstStore::new();
        let result = store.update(&uri, 1, "fn main() {}\n");
        assert!(result.is_none());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn remove_drops_entry() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("demo.ts");
        let uri = uri_for(&path);

        let store = LiveAstStore::new();
        store.update(&uri, 1, "const x = 1;\n").expect("parse");
        assert_eq!(store.len(), 1);
        assert!(store.remove(&uri));
        assert_eq!(store.len(), 0);
        assert!(!store.remove(&uri), "second remove is a no-op");
    }

    #[test]
    fn get_for_path_resolves_relative_to_workspace_root() {
        let dir = tempdir().expect("tempdir");
        let workspace = dir.path();
        let src = workspace.join("src");
        std::fs::create_dir_all(&src).expect("mkdir");
        let file = src.join("demo.ts");
        let uri = uri_for(&file);

        let store = LiveAstStore::new();
        store
            .update(&uri, 1, "export const answer = 42;\n")
            .expect("parse");

        let lookup = store
            .get_for_path(workspace, "src/demo.ts")
            .expect("live doc for relative path");
        assert_eq!(lookup.tree.lang, "typescript");
    }

    #[test]
    fn workspace_relative_strips_root_and_normalizes_separator() {
        let dir = tempdir().expect("tempdir");
        let workspace = dir.path();
        let src = workspace.join("src");
        std::fs::create_dir_all(&src).expect("mkdir");
        let file = src.join("demo.ts");
        let uri = uri_for(&file);

        let rel = LiveDocument::workspace_relative(&uri, workspace).expect("relative");
        assert_eq!(rel, "src/demo.ts");
    }

    #[test]
    fn capability_json_advertises_substrate_state() {
        let store = LiveAstStore::new();
        let cap = store.capability_json();
        assert_eq!(cap["available"], serde_json::json!(true));
        assert_eq!(cap["sync_mode"], serde_json::json!("incremental"));
        assert_eq!(cap["incremental_edits"], serde_json::json!(true));
        assert_eq!(cap["position_encoding"], serde_json::json!("utf-16"));
        assert_eq!(cap["extractors"], serde_json::json!(false));
        let langs = cap["languages"].as_array().expect("languages array");
        assert!(langs.iter().any(|v| v == "typescript"));
    }

    #[test]
    fn malformed_typescript_still_returns_payload_with_has_error() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("broken.ts");
        let uri = uri_for(&path);

        let store = LiveAstStore::new();
        let payload = store
            .update(&uri, 1, "export const = ;\n")
            .expect("parse always returns a tree, even on errors");
        // Tree-sitter always produces a tree, even with parse errors.
        // The notification flags `has_error` so agents can downgrade
        // confidence on the live slice without re-running the parser.
        assert!(payload.has_error);
    }

    #[test]
    fn document_changed_notification_method_matches_contract() {
        assert_eq!(LoctreeDocumentChanged::METHOD, "loctree/documentChanged");
    }

    #[test]
    fn parsers_arc_shared_across_clones() {
        let store = LiveAstStore::new();
        let clone = store.clone();
        // Clone shares the inner parser registry — both surfaces report
        // the same language list without re-instantiating tree-sitter.
        assert_eq!(store.languages(), clone.languages());

        let dir = tempdir().expect("tempdir");
        let uri = uri_for(&dir.path().join("a.ts"));
        clone
            .update(&uri, 1, "const a = 1;\n")
            .expect("parse via clone");

        // The original sees the entry too because the DashMap is the
        // same allocation.
        assert!(store.get(&uri).is_some());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn unsupported_then_supported_does_not_leak_unsupported_uri() {
        let dir = tempdir().expect("tempdir");
        let store = LiveAstStore::new();
        let unsupported = uri_for(&dir.path().join("a.rs"));
        let supported = uri_for(&dir.path().join("b.ts"));

        assert!(store.update(&unsupported, 1, "fn main() {}\n").is_none());
        store
            .update(&supported, 1, "const x = 1;\n")
            .expect("parse");
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn clear_drops_all_documents() {
        let dir = tempdir().expect("tempdir");
        let store = LiveAstStore::new();
        let a = uri_for(&dir.path().join("a.ts"));
        let b = uri_for(&dir.path().join("b.tsx"));

        store.update(&a, 1, "const a = 1;\n").expect("parse a");
        store
            .update(&b, 1, "export const B = () => null;\n")
            .expect("parse b");
        assert_eq!(store.len(), 2);
        store.clear();
        assert_eq!(store.len(), 0);
    }
}
