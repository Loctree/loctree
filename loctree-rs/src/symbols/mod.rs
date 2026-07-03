//! Symbol graph schema — the semantic-topology layer that lives **beside**
//! `import_graph`, not inside it.
//!
//! This is the Wave-A foundation for C-family (Swift / ObjC / ObjC++ / C / C++)
//! awareness described in `docs/research/2026-05-29-loctree-c-family-awareness.md`
//! (§4 schema, §7 integration blueprint). Wave B (tree-sitter extraction) and
//! Wave C (usage edges / deep-mode SCIP+IndexStore import) populate these types;
//! Wave A only lands the **shape**, the round-trip guarantee, and fixtures.
//!
//! ## Why a new module instead of `types.rs`
//!
//! `types.rs` is an import hub (~67-79 importers). Pushing a large schema into
//! it widens an already wide blast radius. `symbols/` is a **leaf** module:
//! it imports nothing from the hub, so adding it costs the hub zero new
//! coupling. The wire-level bridge to [`crate::types::SymbolIdV2`] is
//! string-only ([`SymbolId`] holds the same `<file>::<kind>::<name>::<hash>`
//! descriptor form), so no type dependency is required to interoperate.
//!
//! ## Authority discipline
//!
//! Every [`SymbolNode`] carries [`SymbolProvenance`] and every
//! [`SymbolOccurrence`] / [`SymbolEdge`] carries [`Confidence`]. An agent can
//! always tell whether an edge is a cheap tree-sitter heuristic or a precise
//! compiler-index fact — deep mode never pretends to a precision it does not
//! have. This mirrors the `LoctreeDerived` vs `RepoVerified` authority labels
//! already used in the context pack.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod query;
pub mod resolve;

/// Wire schema version for the symbol graph. Bump on any breaking change to
/// the serialized shape and emit a migration note in CHANGELOG (pinned by
/// `loctree-rs/tests/symbol_graph_roundtrip.rs`).
pub const SYMBOL_GRAPH_SCHEMA_VERSION: &str = "loctree.symbol_graph.v1";

/// SCIP-style stable identifier for a symbol.
///
/// Tier 2 (compiler index) engines emit real USR / SCIP descriptors. Tier 1
/// (build-free tree-sitter) emits a synthesized descriptor of the form
/// `language + normalized_scope + file + syntax_node_range_hash`, explicitly
/// flagged lower-[`Confidence`] at the occurrence/edge level.
///
/// **Bridge to [`crate::types::SymbolIdV2`].** The v2 id already produced by the
/// LSP live tracker (`<file>::<kind>::<name>::<hash16>`) is string-compatible:
/// callers construct a `SymbolId` from `symbol_id_v2.as_str()` without any type
/// import. This keeps `symbols/` a leaf while letting deep-mode import map 1:1.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolId(pub String);

impl SymbolId {
    /// Wrap a raw SCIP/USR descriptor string.
    pub fn new(descriptor: impl Into<String>) -> Self {
        Self(descriptor.into())
    }

    /// Build the Tier-1 build-free descriptor from its components.
    /// Mirrors the [`crate::types::SymbolIdV2`] layout so the two id spaces
    /// coincide for the same `(file, kind, name, range)` tuple.
    pub fn from_parts(file: &str, kind: &str, name: &str, range_hash: u64) -> Self {
        Self(format!("{file}::{kind}::{name}::{range_hash:016x}"))
    }

    /// Borrow the underlying descriptor string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when this id carries no descriptor.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Display for SymbolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SymbolId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SymbolId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// C-family language label for a symbol or occurrence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageId {
    C,
    Cpp,
    ObjC,
    ObjCpp,
    Swift,
}

/// Kind of a declared symbol. Closed set covers the C-family surface; `Other`
/// keeps the schema forward-compatible for kinds a future grammar surfaces.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    /// Generic type (alias target when a more specific kind is unknown).
    Type,
    Class,
    Struct,
    Protocol,
    Enum,
    Func,
    Method,
    Property,
    Field,
    Var,
    Macro,
    Typedef,
    /// ObjC `@selector` / message name.
    Selector,
    /// Swift module / C++ namespace / ObjC umbrella header.
    Module,
    Namespace,
    /// Escape hatch for kinds not yet modeled explicitly.
    Other(String),
}

/// Which engine asserted a symbol or edge. Doubles as the engine identity for
/// occurrences (`SymbolOccurrence::engine`) so there is one provenance space,
/// not two overlapping ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolProvenance {
    /// Tier 1, build-free, cross-platform. Cheap heuristic references.
    TreeSitter,
    /// Tier 2, Swift/ObjC, macOS, USR-based (IndexStoreDB).
    IndexStore,
    /// Tier 2, C/C++, SCIP via scip-clang (prost decode).
    ScipClang,
    /// Tier 2, Swift, SCIP via scip-swift (when mature — Flag W-2).
    ScipSwift,
    /// Tier 2, C/C++, clangd background index bridge.
    Clangd,
    /// Pure heuristic (e.g. literal-name dispatch pairing) — lowest authority.
    Heuristic,
}

/// Precision class for an occurrence or edge. Tier 1 is always `Heuristic`;
/// only compiler-index engines may claim `Precise`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Heuristic,
    Precise,
}

/// Visibility of a declared symbol. Superset spanning Swift access levels and
/// C-family linkage; `Unknown` when the engine cannot determine it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolVisibility {
    /// Swift `open` — public + subclassable/overridable across modules.
    Open,
    Public,
    /// Swift `package`.
    Package,
    /// Swift `internal` / C `extern` module-internal.
    Internal,
    /// Swift `fileprivate` / C static file linkage.
    FilePrivate,
    Private,
    Unknown,
}

/// Role a single occurrence of a symbol plays at a source location.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceRole {
    /// Defining occurrence (the body lives here).
    Definition,
    /// Forward declaration (`.h` prototype, `@interface`).
    Declaration,
    /// Read/name reference.
    Reference,
    /// Call / message send.
    Call,
    /// Import / include of the owning module/header.
    Import,
}

/// Kind of a directed edge between two symbols. Exhaustive set required by the
/// Wave-A acceptance contract; new kinds are additive (old snapshots never
/// carried them, so adding variants stays backward compatible).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolEdgeKind {
    // Universal
    Defines,
    Declares,
    References,
    Calls,
    // OO / protocols
    Overrides,
    Conforms,
    Implements,
    Inherits,
    // Preprocessor / modules
    Includes,
    ImportsModule,
    // C++
    Instantiates,
    MacroExpands,
    // ObjC
    SelectorMessage,
    // Runtime dispatch / events
    NotificationEmit,
    NotificationObserve,
    IBOutletBinding,
    IBActionBinding,
    // Cross-language
    Bridges,
}

/// Half-open source span. Byte offsets back the Tier-1 id hash; line/column are
/// 1-based for human-facing projections (`slice`, `impact`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextRange {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

/// A declared symbol node in the graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolNode {
    /// SCIP-style stable id (Tier 2) or synthesized descriptor (Tier 1).
    pub id: SymbolId,
    pub language: LanguageId,
    pub kind: SymbolKind,
    pub name: String,
    /// Fully-qualified name when the engine resolves scope (e.g. `Foo.bar`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    /// Swift module / C++ namespace / ObjC umbrella, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// Compiler USR when a Tier-2 engine supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usr: Option<String>,
    /// Defining file, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// Declaration range in `file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<TextRange>,
    /// Rendered signature, when the engine extracts one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<SymbolVisibility>,
    /// Which engine asserted this node.
    pub provenance: SymbolProvenance,
}

/// A single occurrence (use site) of a symbol.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolOccurrence {
    pub symbol_id: SymbolId,
    pub file: PathBuf,
    pub range: TextRange,
    pub role: OccurrenceRole,
    /// Precision of this occurrence. Tier 1 ⇒ `Heuristic`.
    pub confidence: Confidence,
    /// Engine that emitted this occurrence.
    pub engine: SymbolProvenance,
}

/// A directed semantic edge between two symbols.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolEdge {
    /// Source symbol (the referrer / caller / definer).
    pub from: SymbolId,
    /// Target symbol (the referent / callee / defined).
    pub to: SymbolId,
    pub kind: SymbolEdgeKind,
    /// Engine that asserted this edge.
    pub provenance: SymbolProvenance,
    /// Precision of this edge. Tier 1 ⇒ `Heuristic`.
    pub confidence: Confidence,
}

/// Record of one engine run that contributed to the graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolEngineRun {
    pub engine: SymbolProvenance,
    /// Number of symbol nodes this engine contributed.
    #[serde(default)]
    pub symbol_count: usize,
    /// Number of occurrences this engine contributed.
    #[serde(default)]
    pub occurrence_count: usize,
    /// Optional engine/tool version string for reproducibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_version: Option<String>,
}

/// Per-file projection of the graph, consumed by `slice <file>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSymbolSummary {
    pub file: PathBuf,
    /// Symbols defined in this file.
    #[serde(default)]
    pub defined: Vec<SymbolId>,
    /// Symbols referenced (but not defined) in this file.
    #[serde(default)]
    pub referenced: Vec<SymbolId>,
}

/// The symbol graph: a parallel semantic-topology layer beside `import_graph`.
///
/// Attached to [`crate::snapshot::Snapshot`] as an **optional** section
/// (`#[serde(default)]`); a snapshot produced before this layer existed
/// deserializes with `symbol_graph == None` and serializes byte-identically.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolGraph {
    /// Pinned to [`SYMBOL_GRAPH_SCHEMA_VERSION`].
    #[serde(default)]
    pub schema_version: String,
    /// Engines that populated this graph.
    #[serde(default)]
    pub engines: Vec<SymbolEngineRun>,
    #[serde(default)]
    pub symbols: Vec<SymbolNode>,
    #[serde(default)]
    pub occurrences: Vec<SymbolOccurrence>,
    #[serde(default)]
    pub edges: Vec<SymbolEdge>,
    /// Per-file projections for `slice`.
    #[serde(default)]
    pub file_projection: Vec<FileSymbolSummary>,
}

impl Default for SymbolGraph {
    fn default() -> Self {
        Self {
            schema_version: SYMBOL_GRAPH_SCHEMA_VERSION.to_string(),
            engines: Vec::new(),
            symbols: Vec::new(),
            occurrences: Vec::new(),
            edges: Vec::new(),
            file_projection: Vec::new(),
        }
    }
}

impl SymbolGraph {
    /// Build an empty graph stamped with the current schema version.
    pub fn new() -> Self {
        Self::default()
    }

    /// True when the graph holds no symbols, occurrences, or edges.
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty() && self.occurrences.is_empty() && self.edges.is_empty()
    }
}
