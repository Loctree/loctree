//! LSP Backend implementation for loctree
//!
//! Provides lifecycle handlers and document synchronization for the LSP server.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashmap::DashMap;
use loctree::snapshot::Snapshot;
use notify::{RecursiveMode, Watcher};
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::actions;
use crate::aicx as aicx_handler;
use crate::aicx::{AicxParams, AicxResponse};
use crate::ast_query::{self as ast_query_handler, AstQueryParams, AstQueryResponse};
use crate::body::{BodyParams, BodyResponse};
use crate::code_lens;
use crate::context_atlas::{self, ContextAtlasParams, ContextAtlasResponse};
use crate::context_pack::{self, ContextPackParams, ContextPackResponse};
use crate::diagnostic_codes::DiagnosticCode;
use crate::diagnostics;
use crate::diff::{self as diff_handler, DiffParams, DiffResponse, DiffSession};
use crate::find::{self, FindParams, FindResponse};
use crate::follow::{self, FollowParams, FollowResponse};
use crate::health::{self, HealthParams, HealthResponse};
use crate::impact::{self, ImpactParams, ImpactResponse};
use crate::live_ast::{
    self, LiveAstStore, LoctreeDocumentChanged, LoctreeSymbolChanged, SymbolMetadata,
};
use crate::navigation::get_word_at_position;
use crate::protocol::{ResponseIdentity, chunk_size_from_options, code_lens_from_options};
use crate::semantic::{self as semantic_handler, SemanticParams, SemanticResponse};
use crate::slice::{self, SliceParams, SliceResponse};
use crate::snapshot::SnapshotState;
use crate::symbol_context::{self, SymbolContextParams, SymbolContextResponse};
use crate::watcher::{
    LoctreeScanProgress, ScanPhase, ScanProgress, ScanStats, WatcherConfig, config_from_options,
    should_trigger_rescan,
};
use crate::workspaces::{
    self, WorkspaceInfo, WorkspacesParams, WorkspacesResponse, max_depth_from_options,
};
use loctree::types::SymbolIdV1;

pub fn server_info() -> ServerInfo {
    ServerInfo {
        name: "Loctree Language Server".to_string(),
        version: Some(crate::BUILD_VERSION.to_string()),
    }
}

/// Build a capability advertisement for a `loctree/*` request whose
/// params type derives [`schemars::JsonSchema`].
///
/// The result carries `available: true` plus a `requestSchema` field
/// holding the JSON Schema for the params type — editor-side LSP APIs
/// (JetBrains LSP, vscode-languageclient typed bindings, custom client
/// builders) can use it to render typed forms / validate calls without
/// having to ship duplicate type definitions.
fn request_capability(schema: schemars::Schema) -> serde_json::Value {
    serde_json::json!({
        "available": true,
        "requestSchema": schema,
    })
}

/// Same as [`request_capability`] but folds an `extras` object into
/// the resulting capability map. Used by namespaces that publish
/// per-handler metadata alongside the schema (e.g. `loctree/follow`'s
/// implemented-vs-stub scope split, `loctree/find` and `loctree/aicx`'s
/// `symbol_id_version`, `loctree/semantic`'s scope deferral table).
fn request_capability_with(
    schema: schemars::Schema,
    extras: serde_json::Value,
) -> serde_json::Value {
    let mut base = request_capability(schema);
    if let (Some(map), serde_json::Value::Object(extra_map)) = (base.as_object_mut(), extras) {
        for (k, v) in extra_map {
            map.insert(k, v);
        }
    }
    base
}

pub fn server_capabilities(
    document_changed_capability: serde_json::Value,
    code_lens_enabled: bool,
) -> ServerCapabilities {
    let symbol_id_version = serde_json::json!({
        "symbol_id_version": loctree::types::SymbolIdV1::VERSION,
    });
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                // Plan 17 v2: INCREMENTAL sync — `did_change` now
                // translates each `TextDocumentContentChangeEvent` into a
                // tree-sitter `InputEdit` and feeds them to
                // `Parsers::parse_incremental`. Range-less events still
                // fall back to full reparse via `LiveAstStore::update`.
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(true),
                })),
                ..Default::default()
            },
        )),
        // Hover intentionally not advertised: rust-analyzer / tsserver own hover
        // in the IDE. Loctree does not fight native hovers; structural context
        // lives in the Context Pill. The `hover` trait method remains as a valid
        // (now unadvertised) handler.
        hover_provider: None,
        // Do NOT advertise pull diagnostics. tower-lsp 0.20 does not route
        // textDocument/diagnostic or workspace/diagnostic to a handler, so
        // advertising `diagnostic_provider` made clients pull every ~2s and get
        // a flood of -32601 "Method not found" errors. Diagnostics are delivered
        // via the PUSH model (`client.publish_diagnostics` on open/change/save/
        // refresh), which works without this capability. The `diagnostic` pull
        // handler was removed for the same reason — re-add it alongside a
        // tower-lsp upgrade if pull diagnostics ever become routable.
        diagnostic_provider: None,
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        // Code lenses are OPT-IN, disabled by default: advertising them
        // unconditionally clutters the gutter alongside the real language
        // server's lenses. The feature is loctree-additive (not a masquerade),
        // so the `code_lens` trait handler stays wired and works whenever the
        // client opts in via `initializationOptions.codeLens = true`.
        code_lens_provider: code_lens_enabled.then_some(CodeLensOptions {
            resolve_provider: Some(false),
        }),
        // Go-to-Definition intentionally not advertised: VS Code's native
        // "Go to Definition" implies a SEMANTIC resolution from the language
        // server (rust-analyzer / tsserver / pyright / gopls), which do it
        // better. Loctree's definition is a snapshot-graph lookup and must not
        // compete on the mainstream languages. The `goto_definition` trait
        // method remains as a valid (now unadvertised) handler. Same class of
        // false-semantic-duplicate as `references_provider` below.
        definition_provider: None,
        // References intentionally not advertised: VS Code's native "Find All
        // References" implies SEMANTIC references (rust-analyzer territory).
        // Loctree serves literal occurrences and must not masquerade as a
        // semantic provider — that data lives in the Context Pill ("used by /
        // literal occurrences"). The `references` trait method remains as a
        // valid (now unadvertised) handler.
        references_provider: None,
        // Intentionally advertise an EMPTY command list. The server still
        // handles `workspace/executeCommand` for OPEN_ATLAS_CARD_COMMAND (see
        // `execute_command` below), but editor wrappers register that command
        // themselves for custom UX (the VS Code extension opens the returned
        // card file). Advertising it here makes vscode-languageclient's
        // ExecuteCommandFeature ALSO register it, colliding with the wrapper
        // ("command 'loctree.openAtlasCard' already exists") and crashing client
        // init. Keeping the list empty lets the wrapper own the command while
        // the server keeps serving the request.
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: vec![],
            work_done_progress_options: WorkDoneProgressOptions::default(),
        }),
        experimental: Some(serde_json::json!({
            "loctree/refresh": { "available": true },
            "loctree/scanProgress": { "available": true },
            "loctree/contextAtlas": request_capability(
                schemars::schema_for!(ContextAtlasParams)
            ),
            "loctree/contextPack": request_capability(
                schemars::schema_for!(ContextPackParams)
            ),
            "loctree/openAtlasCard": { "available": true },
            // Plan 15 + Stage 2 truth pass: split advertised vocabulary from the
            // actually-wired subset so clients can probe without round-tripping.
            "loctree/follow": request_capability_with(
                schemars::schema_for!(FollowParams),
                serde_json::json!({
                    "scopes": follow::SUPPORTED_SCOPES,
                    "implemented_scopes": follow::IMPLEMENTED_SCOPES,
                    "stub_scopes": follow::STUB_SCOPES,
                    "stub_reason": {
                        "trace": "handler-graph walker not yet portable from CLI; use `loct trace --handler <name>` until Stage 3+"
                    }
                }),
            ),
            "loctree/body": request_capability(schemars::schema_for!(BodyParams)),
            "loctree/symbolContext": request_capability(
                schemars::schema_for!(SymbolContextParams)
            ),
            "loctree/slice": request_capability(schemars::schema_for!(SliceParams)),
            "loctree/impact": request_capability(schemars::schema_for!(ImpactParams)),
            "loctree/find": request_capability_with(
                schemars::schema_for!(FindParams),
                symbol_id_version.clone(),
            ),
            "loctree/health": request_capability(schemars::schema_for!(HealthParams)),
            "loctree/workspaces": request_capability(
                schemars::schema_for!(WorkspacesParams)
            ),
            "loctree/diff": request_capability(schemars::schema_for!(DiffParams)),
            "loctree/semantic": request_capability_with(
                schemars::schema_for!(SemanticParams),
                serde_json::json!({
                    "supported_scopes": ["file", "project"],
                    "deferred_scopes": ["symbol"],
                    "deferral_reason": {
                        "symbol": "tree-sitter substrate not yet integrated (Plan 16 prerequisite for stable byte-range SymbolId - Plan 18 v2)"
                    }
                }),
            ),
            "loctree/aicx": request_capability_with(
                schemars::schema_for!(AicxParams),
                symbol_id_version,
            ),
            "loctree/documentChanged": document_changed_capability,
            "loctree/symbolChanged": live_ast::symbol_changed_capability_json(),
            "loctree/astQuery": ast_query_handler::capability_json(),
        })),
        ..Default::default()
    }
}

pub fn initialize_result(
    document_changed_capability: serde_json::Value,
    code_lens_enabled: bool,
) -> InitializeResult {
    InitializeResult {
        server_info: Some(server_info()),
        capabilities: server_capabilities(document_changed_capability, code_lens_enabled),
    }
}

pub fn static_initialize_result() -> InitializeResult {
    // The static / `--capabilities` view shows the default-off surface:
    // code lenses are opt-in (see `code_lens_from_options`).
    initialize_result(LiveAstStore::new().capability_json(), false)
}

/// Loctree LSP backend state
pub struct Backend {
    /// LSP client for sending notifications/responses
    client: Client,
    /// Document content cache (uri -> content)
    documents: DashMap<Url, String>,
    /// Cached diagnostics per document URI
    cached_diagnostics: DashMap<Url, Vec<Diagnostic>>,
    /// Bounded LRU cache for literal scans — avoids re-reading the whole
    /// workspace from disk on repeated hover / "Load more" over the same query.
    literal_cache: find::LiteralScanCache,
    /// Workspace root path
    workspace_root: RwLock<Option<String>>,
    /// Loaded snapshot state for the **root workspace**.
    ///
    /// Plan 13 keeps this as the canonical handle for the LSP root —
    /// every doc-sync, navigation, and diagnostics path that does not
    /// carry a `project` field still goes here. Sub-projects discovered
    /// under the root live in [`Self::extra_workspaces`] keyed by
    /// canonical path. There is exactly one snapshot per addressable
    /// workspace; the root is intentionally not duplicated into the
    /// extras map.
    snapshot: SnapshotState,
    /// Per-sub-project snapshots discovered under the root workspace.
    ///
    /// Keyed by canonical absolute path (`fs::canonicalize`). Populated
    /// at `initialized` time when [`workspaces::discover_loctree_dirs`]
    /// finds `.loctree/` directories below the root. Routing handlers
    /// look here for `params.project` overrides; misses fall back to
    /// the root snapshot.
    extra_workspaces: Arc<RwLock<HashMap<PathBuf, SnapshotState>>>,
    /// Discovery depth honored at `initialized` (Plan 13). Default
    /// [`workspaces::DEFAULT_MAX_DEPTH`]; clamped to
    /// [`workspaces::MAX_DEPTH_CEILING`].
    workspaces_max_depth: RwLock<usize>,
    /// Per-workspace diff history (Plan 11). Keyed by canonical
    /// project root (the same key as [`Self::extra_workspaces`] plus
    /// the LSP root). Tracks the previous-scan and last-query
    /// snapshots so `loctree/diff` can compute session-local deltas
    /// without re-running the analyzer.
    diff_sessions: Arc<RwLock<HashMap<PathBuf, Arc<RwLock<DiffSession>>>>>,
    /// Default chunk size from initializationOptions (Plan 12),
    /// reused by Plan 14's `scope=project` paginator.
    default_chunk_size: RwLock<usize>,
    /// Watcher configuration parsed from `initializationOptions`.
    watcher_config: RwLock<WatcherConfig>,
    /// Handle to the spawned watcher task (Plan 10). `None` until the
    /// watcher starts, dropped on `shutdown`.
    watcher_task: RwLock<Option<tokio::task::JoinHandle<()>>>,
    /// Plan 17 MVP: per-URI live tree-sitter cache for open JS/TS/TSX
    /// documents. Populated on `did_open` / `did_change`, drained on
    /// `did_close`. Consumed by [`Self::ast_query`] before it falls
    /// back to the on-disk reparse path.
    live_ast: LiveAstStore,
    /// Plan 18 v2: per-URI symbol tracker. Keyed by document URI;
    /// inner map keys symbols by [`SymbolIdV1`] (`<file>::<symbol>`)
    /// for back-compat with the v1 wire contract. The cache feeds the
    /// `loctree/symbolChanged` diff classifier on every INCREMENTAL
    /// `did_change`, so consumers see at most one notification per
    /// edit transaction.
    symbol_tracker: Arc<RwLock<HashMap<Url, HashMap<SymbolIdV1, SymbolMetadata>>>>,
}

impl Backend {
    /// Create a new Backend instance with no pinned workspace root.
    ///
    /// The workspace root is discovered from the LSP `initialize`
    /// handshake. This is the long-standing editor-driven path.
    pub fn new(client: Client) -> Self {
        Self::with_optional_root(client, None)
    }

    /// Create a Backend with a workspace root pinned at startup.
    ///
    /// Used by `loct watch --lsp` (via `--root`) so the server has a
    /// workspace root before any LSP `initialize` arrives — the watch
    /// co-process may never receive one. A later `initialize` carrying a
    /// `rootUri` will NOT override this pin; see
    /// [`LanguageServer::initialize`].
    pub fn with_root(client: Client, root: PathBuf) -> Self {
        Self::with_optional_root(client, Some(root.to_string_lossy().to_string()))
    }

    /// Shared constructor. `root` pre-populates [`Self::workspace_root`]
    /// synchronously (no async lock acquisition), which is why the pin is
    /// stored as a plain field init rather than written through the lock.
    fn with_optional_root(client: Client, root: Option<String>) -> Self {
        if let Some(ref pinned) = root {
            tracing::info!("Workspace root pinned via --root: {}", pinned);
        }
        Self {
            client,
            documents: DashMap::new(),
            cached_diagnostics: DashMap::new(),
            literal_cache: find::LiteralScanCache::new(),
            workspace_root: RwLock::new(root),
            snapshot: SnapshotState::new(),
            extra_workspaces: Arc::new(RwLock::new(HashMap::new())),
            workspaces_max_depth: RwLock::new(workspaces::DEFAULT_MAX_DEPTH),
            diff_sessions: Arc::new(RwLock::new(HashMap::new())),
            default_chunk_size: RwLock::new(crate::protocol::DEFAULT_CHUNK_SIZE),
            watcher_config: RwLock::new(WatcherConfig::default()),
            watcher_task: RwLock::new(None),
            live_ast: LiveAstStore::new(),
            symbol_tracker: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Borrow the per-document live AST store. Used by tests to seed
    /// the cache and by the LSP backend to share a single registry
    /// across handlers.
    #[cfg(test)]
    pub fn live_ast(&self) -> &LiveAstStore {
        &self.live_ast
    }

    /// Resolve a `project` override to the snapshot it belongs to.
    ///
    /// `None` (or a path that canonicalizes to the LSP's root workspace)
    /// returns the root [`SnapshotState`]. Anything else is looked up
    /// in [`Self::extra_workspaces`]; misses fall back to the root with
    /// a debug-log breadcrumb so the operator can see a stale `project`
    /// param without a 500-class error. Cloning a `SnapshotState` is
    /// an `Arc` bump — cheap, never blocks.
    async fn routed_snapshot(&self, project: Option<&Path>) -> SnapshotState {
        let Some(target) = self.canonicalize_project(project).await else {
            return self.snapshot.clone();
        };
        let extras = self.extra_workspaces.read().await;
        if let Some(state) = extras.get(&target) {
            return state.clone();
        }
        drop(extras);
        tracing::debug!(
            "loctree/route: project {:?} not in extras, falling back to root",
            target
        );
        self.snapshot.clone()
    }

    /// Resolve a `project` override to the workspace root path used
    /// for filesystem operations (atlas paths, diff snapshots, …).
    ///
    /// Returns `None` only when the LSP has no workspace root yet
    /// (pre-`initialized`). Unknown sub-projects fall back to the root.
    async fn routed_root(&self, project: Option<&Path>) -> Option<PathBuf> {
        let Some(target) = self.canonicalize_project(project).await else {
            return self.workspace_root_path().await;
        };
        let extras = self.extra_workspaces.read().await;
        if extras.contains_key(&target) {
            return Some(target);
        }
        drop(extras);
        self.workspace_root_path().await
    }

    /// Canonicalize a `project` override and return it when it points
    /// to a real, addressable workspace. `None` means "use the root";
    /// the special-case is encoded by returning `None` here too.
    async fn canonicalize_project(&self, project: Option<&Path>) -> Option<PathBuf> {
        let project = project?;
        let candidate = if project.is_absolute() {
            project.to_path_buf()
        } else {
            // Resolve relative paths against the LSP root.
            let root = self.workspace_root_path().await?;
            root.join(project)
        };
        let canonical = workspaces::canonicalize(&candidate);
        let root_path = self.workspace_root_path().await;
        if root_path.as_ref() == Some(&canonical) {
            return None; // routes to root
        }
        Some(canonical)
    }

    async fn workspace_root_path(&self) -> Option<PathBuf> {
        self.workspace_root
            .read()
            .await
            .as_deref()
            .map(PathBuf::from)
    }

    /// Walk the workspace root for `.loctree/` sub-projects (Plan 13)
    /// and load their snapshots into [`Self::extra_workspaces`].
    ///
    /// Idempotent — re-running it after the watcher reloads picks up
    /// newly created sub-projects without dropping existing entries.
    async fn discover_and_load_workspaces(&self) {
        let Some(root) = self.workspace_root_path().await else {
            return;
        };
        let depth = *self.workspaces_max_depth.read().await;
        let dirs = tokio::task::spawn_blocking({
            let root = root.clone();
            move || workspaces::discover_loctree_dirs(&root, depth)
        })
        .await
        .unwrap_or_default();

        if dirs.is_empty() {
            return;
        }
        tracing::info!(
            "loctree-lsp: discovered {} sub-project(s) under {}",
            dirs.len(),
            root.display()
        );

        let mut extras = self.extra_workspaces.write().await;
        for sub in dirs {
            // Skip if already present — preserve loaded snapshot.
            let entry = extras.entry(sub.clone()).or_insert_with(SnapshotState::new);
            // Best-effort load. Failures are logged and skipped — the
            // sub-project still appears in `loctree/workspaces` with
            // `has_snapshot: false` so agents can decide.
            if let Err(err) = entry.load(&sub).await {
                tracing::warn!(
                    "loctree-lsp: sub-workspace {} load failed: {}",
                    sub.display(),
                    err
                );
            }
        }
    }

    /// Custom request handler for `loctree/workspaces` (Plan 13).
    ///
    /// Returns the root workspace plus every sub-project the daemon
    /// knows about. Each row carries `has_snapshot`, `files`,
    /// `languages`, and `snapshot_age_seconds` so agents can decide
    /// whether to ask for a refresh before issuing a routed request.
    pub async fn workspaces(&self, _params: WorkspacesParams) -> Result<WorkspacesResponse> {
        let mut rows: Vec<WorkspaceInfo> = Vec::new();

        if let Some(root) = self.workspace_root_path().await {
            rows.push(workspace_info(&self.snapshot, &root, true).await);
        }

        let extras = self.extra_workspaces.read().await;
        let mut sub_rows: Vec<WorkspaceInfo> = Vec::with_capacity(extras.len());
        for (path, state) in extras.iter() {
            sub_rows.push(workspace_info(state, path, false).await);
        }
        sub_rows.sort_by(|a, b| a.root.cmp(&b.root));
        rows.extend(sub_rows);

        Ok(WorkspacesResponse { workspaces: rows })
    }

    /// Look up (or lazily create) the [`DiffSession`] for a routed
    /// workspace (Plan 11). Sessions live in `Backend::diff_sessions`
    /// and are keyed by canonical path so `lastQuery` continues to
    /// resolve across requests within the same daemon process.
    async fn diff_session_for(&self, key: &Path) -> Arc<RwLock<DiffSession>> {
        let canonical = workspaces::canonicalize(key);
        let mut sessions = self.diff_sessions.write().await;
        sessions
            .entry(canonical)
            .or_insert_with(|| Arc::new(RwLock::new(DiffSession::default())))
            .clone()
    }

    /// Custom request handler for `loctree/diff` (Plan 11).
    ///
    /// Returns session-local structural deltas for the routed
    /// workspace. `since` accepts `epoch`, `lastScan`, `lastQuery`,
    /// or returns a typed `unsupported_since` error for git revs
    /// (deferred to v2 — see [`crate::diff`] module docs).
    pub async fn diff(&self, params: DiffParams) -> Result<DiffResponse> {
        let routed = self.routed_snapshot(params.project.as_deref()).await;
        let routed_root = self
            .routed_root(params.project.as_deref())
            .await
            .ok_or_else(|| tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
                message: "loctree-lsp has no workspace root yet — wait for `initialized`".into(),
                data: None,
            })?;

        let guard = routed
            .get()
            .await
            .ok_or_else(|| tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
                message: "loctree snapshot not loaded yet — try again after `initialized`".into(),
                data: None,
            })?;
        let loaded = guard.as_ref().ok_or_else(|| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
            message: "loctree snapshot is empty".into(),
            data: None,
        })?;

        // Borrow the current snapshot via Arc so the session can keep
        // it for the next `lastQuery` request without holding the
        // outer guard across .await.
        let current_snapshot = Arc::new(loaded.snapshot.clone());
        drop(guard);

        let session_handle = self.diff_session_for(&routed_root).await;
        let session = session_handle.read().await;
        let baseline_arc = match params.since.as_str() {
            "epoch" => None,
            "lastScan" => session.last_scan(),
            "lastQuery" => session.last_query(),
            marker if marker.starts_with("snapshot:") => {
                match session.snapshot_for_marker(marker) {
                    Some(snapshot) => Some(snapshot),
                    None => return Ok(diff_handler::unsupported_since(marker)),
                }
            }
            other => return Ok(diff_handler::unsupported_since(other)),
        };
        drop(session);

        let snapshot_id = semantic_handler::snapshot_pagination_id(&current_snapshot);
        let chunk_size = match params.chunk_size {
            Some(value) => value,
            None => *self.default_chunk_size.read().await,
        };
        let baseline = baseline_arc.as_deref();
        let response = diff_handler::compute_paginated(
            baseline,
            &current_snapshot,
            &params.since,
            params.cursor.as_deref(),
            chunk_size,
            &snapshot_id,
        )
        .map_err(|err| {
            tower_lsp::jsonrpc::Error::invalid_params(format!(
                "loctree/diff cursor decode failed: {err}"
            ))
        })?;

        // Advance the lastQuery marker only on success — `epoch` and
        // `lastScan` callers also benefit from a fresh baseline so the
        // next `lastQuery` call is consistent with what they just saw.
        let mut session = session_handle.write().await;
        session.advance(current_snapshot);
        Ok(response)
    }

    /// Custom request handler for `loctree/semantic` (Plan 14).
    ///
    /// Routes to per-workspace snapshot, validates `scope`, and
    /// delegates to [`crate::semantic`] for the actual composer call.
    /// `scope = "symbol"` returns the staged
    /// `symbol_scope_unimplemented` response with a hint to use
    /// `scope = "file"` instead.
    pub async fn semantic(&self, params: SemanticParams) -> Result<SemanticResponse> {
        let routed = self.routed_snapshot(params.project.as_deref()).await;
        let guard = routed
            .get()
            .await
            .ok_or_else(|| tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
                message: "loctree snapshot not loaded yet — try again after `initialized`".into(),
                data: None,
            })?;
        let loaded = guard.as_ref().ok_or_else(|| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
            message: "loctree snapshot is empty".into(),
            data: None,
        })?;

        let snapshot_id = semantic_handler::snapshot_pagination_id(&loaded.snapshot);

        match params.scope.as_str() {
            "file" => {
                let target = params.target.clone().ok_or_else(|| {
                    tower_lsp::jsonrpc::Error::invalid_params(
                        "loctree/semantic scope=file requires a target file path",
                    )
                })?;
                let data = semantic_handler::compute_file_scope(
                    &loaded.snapshot,
                    &target,
                    params.project.clone(),
                    &params.kinds,
                );
                Ok(SemanticResponse {
                    status: "ok".to_string(),
                    hint: None,
                    scope: "file".to_string(),
                    target: Some(target),
                    data,
                    pagination: semantic_handler::singleton_pagination(),
                })
            }
            "symbol" => Ok(semantic_handler::symbol_scope_response(params.target)),
            "project" => {
                let chunk_size = match params.chunk_size {
                    Some(value) => value,
                    None => *self.default_chunk_size.read().await,
                };
                let data = semantic_handler::compute_project_scope(
                    &loaded.snapshot,
                    params.project.clone(),
                    &params.kinds,
                );
                let (paged_data, pagination) = semantic_handler::paginate_project(
                    data,
                    params.cursor.as_deref(),
                    chunk_size,
                    &snapshot_id,
                )
                .map_err(|err| {
                    tower_lsp::jsonrpc::Error::invalid_params(format!(
                        "loctree/semantic cursor decode failed: {err}"
                    ))
                })?;
                Ok(SemanticResponse {
                    status: "ok".to_string(),
                    hint: None,
                    scope: "project".to_string(),
                    target: None,
                    data: paged_data,
                    pagination,
                })
            }
            other => Err(tower_lsp::jsonrpc::Error::invalid_params(format!(
                "loctree/semantic unknown scope: `{other}` (use file|symbol|project)"
            ))),
        }
    }

    /// Custom request handler for `loctree/aicx` (Plan 08).
    ///
    /// Read-only AICX memory continuity for the routed workspace.
    /// Always succeeds at the LSP-error level — if AICX is missing,
    /// the response carries `status = "aicx_unavailable"` with a hint.
    pub async fn aicx(&self, params: AicxParams) -> Result<AicxResponse> {
        let routed = self.routed_snapshot(params.project.as_deref()).await;
        let routed_root = self.routed_root(params.project.as_deref()).await;

        let guard = routed
            .get()
            .await
            .ok_or_else(|| tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
                message: "loctree snapshot not loaded yet — try again after `initialized`".into(),
                data: None,
            })?;
        let loaded = guard.as_ref().ok_or_else(|| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
            message: "loctree snapshot is empty".into(),
            data: None,
        })?;

        Ok(aicx_handler::compute(
            &loaded.snapshot,
            &params,
            routed_root.as_deref(),
        ))
    }

    /// Custom request handler for `loctree/astQuery` (Plan 20).
    ///
    /// Reparses snapshot files on demand through `loctree-ast`. As of
    /// the P0 Stage 2 cut on top of Plan 16's substrate, the handler
    /// also consults the per-URI [`LiveAstStore`] before falling back to
    /// disk — open JS/TS/TSX documents query their live (possibly
    /// unsaved) tree-sitter tree, while closed files keep the on-disk
    /// path. Capability JSON advertises `liveDocumentCache: true`.
    pub async fn ast_query(&self, params: AstQueryParams) -> Result<AstQueryResponse> {
        let routed = self.routed_snapshot(params.project.as_deref()).await;
        let guard = routed
            .get()
            .await
            .ok_or_else(|| tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
                message: "loctree snapshot not loaded yet — try again after `initialized`".into(),
                data: None,
            })?;
        let loaded = guard.as_ref().ok_or_else(|| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
            message: "loctree snapshot is empty".into(),
            data: None,
        })?;

        ast_query_handler::compute_with_live(
            &loaded.snapshot,
            &loaded.workspace_root,
            &params,
            Some(&self.live_ast),
        )
        .map_err(ast_query_handler::to_lsp_error)
    }

    /// Load snapshot from workspace with auto-scan and staleness detection.
    ///
    /// Pattern from MCP server:
    /// - If no snapshot exists → auto-scan → load
    /// - If snapshot is stale (git HEAD changed) → rescan → reload
    /// - Otherwise → use existing snapshot
    async fn load_snapshot(&self) {
        let root = self.workspace_root.read().await.clone();
        let Some(root_path) = root else { return };
        let path = PathBuf::from(&root_path);

        match self.snapshot.load(&path).await {
            Ok(()) => {
                // Check if loaded snapshot is stale
                let stale = if let Some(guard) = self.snapshot.get().await {
                    guard
                        .as_ref()
                        .map(|loaded| is_snapshot_stale(&loaded.snapshot, &path))
                        .unwrap_or(false)
                } else {
                    false
                };

                if stale {
                    tracing::info!("Snapshot stale, rescanning {}", root_path);
                    self.client
                        .log_message(MessageType::INFO, "loctree: snapshot stale, rescanning...")
                        .await;
                    if run_scan(&path).await.is_ok() {
                        let _ = self.snapshot.load(&path).await;
                    }
                }

                tracing::info!("Loaded snapshot from {}", root_path);
                self.client
                    .log_message(MessageType::INFO, "loctree snapshot loaded")
                    .await;
            }
            Err(crate::snapshot::SnapshotError::NotFound(_)) => {
                // No snapshot exists — auto-scan the project
                tracing::info!("No snapshot found, scanning {}...", root_path);
                self.client
                    .log_message(MessageType::INFO, "loctree: no snapshot, scanning...")
                    .await;

                if let Err(e) = run_scan(&path).await {
                    tracing::warn!("Auto-scan failed: {}", e);
                    self.client
                        .log_message(
                            MessageType::WARNING,
                            format!("loctree auto-scan failed: {}", e),
                        )
                        .await;
                    return;
                }

                match self.snapshot.load(&path).await {
                    Ok(()) => {
                        tracing::info!("Snapshot loaded after auto-scan for {}", root_path);
                        self.client
                            .log_message(
                                MessageType::INFO,
                                "loctree snapshot loaded (auto-scanned)",
                            )
                            .await;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load after scan: {}", e);
                        self.client
                            .log_message(MessageType::WARNING, format!("{}", e))
                            .await;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to load snapshot: {}", e);
                self.client
                    .log_message(MessageType::WARNING, format!("{}", e))
                    .await;
            }
        }
    }

    /// Refresh snapshot + diagnostics (used by editor integration)
    async fn refresh_snapshot(&self) {
        self.load_snapshot().await;
        // Plan 13: also refresh every known sub-project so the routing
        // map and the root snapshot stay aligned after a manual refresh.
        self.discover_and_load_workspaces().await;
        let uris: Vec<Url> = self
            .documents
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        for uri in uris {
            self.publish_diagnostics(uri).await;
        }
        self.client
            .log_message(MessageType::INFO, "loctree-lsp refreshed")
            .await;
    }

    /// Custom notification handler for loctree/refresh
    pub async fn refresh(&self) {
        self.refresh_snapshot().await;
    }

    /// Start the background filesystem watcher (Plan 10 of the LSP roadmap).
    ///
    /// Subscribes to `root` recursively, debounces fs events per the
    /// configured window, and triggers an incremental rescan + snapshot
    /// reload on each batch. Emits `loctree/scanProgress` notifications
    /// at `scanning` → `done` (or `failed`).
    ///
    /// No-op when:
    /// - the watcher is disabled via `loctree.watcher.enabled = false`,
    /// - a watcher task is already running.
    async fn start_watcher(&self, root: PathBuf) {
        let config = self.watcher_config.read().await.clone();
        if !config.enabled {
            tracing::info!("loctree watcher disabled via init options");
            return;
        }
        if self.watcher_task.read().await.is_some() {
            tracing::debug!("loctree watcher already running");
            return;
        }

        let client = self.client.clone();
        let snapshot = self.snapshot.clone();
        // Plan 13: clone the extras handle so the watcher loop can
        // reload sub-projects on every successful rescan, keeping
        // per-workspace freshness in step with the root.
        let extras = self.extra_workspaces.clone();
        // Plan 11: the watcher rotates each workspace's last_scan
        // baseline before the reload — the previous "current"
        // snapshot becomes the new lastScan baseline.
        let diff_sessions = self.diff_sessions.clone();
        let root_for_task = root.clone();

        // tokio mpsc carries notify events from the (sync) watcher
        // closure into the async debounce task.
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<notify::Event>();
        let event_handler = move |result: notify::Result<notify::Event>| {
            if let Ok(event) = result {
                let _ = event_tx.send(event);
            }
        };

        let mut watcher = match notify::recommended_watcher(event_handler) {
            Ok(w) => w,
            Err(err) => {
                tracing::warn!("loctree watcher init failed: {err}");
                return;
            }
        };
        if let Err(err) = watcher.watch(&root, RecursiveMode::Recursive) {
            tracing::warn!("loctree watcher.watch failed for {root:?}: {err}");
            return;
        }

        let task = tokio::spawn(async move {
            // Move the watcher into the task so its lifetime tracks the
            // task. When the task ends (shutdown), the watcher drops and
            // the OS subscription is released.
            let _watcher_keepalive = watcher;

            while let Some(first) = event_rx.recv().await {
                // Collect everything that arrives within the debounce
                // window; treat them as a single batch.
                let deadline = tokio::time::Instant::now() + config.debounce;
                let mut paths: Vec<PathBuf> = first.paths;
                while let Ok(Some(event)) = tokio::time::timeout_at(deadline, event_rx.recv()).await
                {
                    paths.extend(event.paths);
                }

                // Drop noise: events from `target/`, `.git/` etc. shouldn't
                // trigger a rescan. If nothing in the batch is relevant,
                // skip the whole iteration.
                if !paths.iter().any(|p| should_trigger_rescan(p, &config)) {
                    continue;
                }

                client
                    .send_notification::<LoctreeScanProgress>(ScanProgress::phase_only(
                        ScanPhase::Scanning,
                    ))
                    .await;

                let scan_root = root_for_task.clone();
                match run_scan(&scan_root).await {
                    Ok(scan_stats) => {
                        client
                            .send_notification::<LoctreeScanProgress>(ScanProgress::with_counts(
                                ScanPhase::Composing,
                                scan_stats,
                            ))
                            .await;

                        // Plan 11: capture the root's previous snapshot
                        // before the reload so it can serve as the
                        // `lastScan` baseline.
                        rotate_last_scan(&snapshot, &diff_sessions, &scan_root).await;

                        if let Err(err) = snapshot.load(&scan_root).await {
                            client
                                .send_notification::<LoctreeScanProgress>(ScanProgress::failed(
                                    format!("snapshot reload failed: {err}"),
                                ))
                                .await;
                            continue;
                        }

                        // Plan 13: refresh every sub-project the daemon
                        // knows about. Best-effort — a single failing
                        // reload should not abort the whole batch (the
                        // sub-project simply keeps its previous state).
                        let extra_handles: Vec<(PathBuf, SnapshotState)> = {
                            let guard = extras.read().await;
                            guard.iter().map(|(p, s)| (p.clone(), s.clone())).collect()
                        };
                        for (sub_root, sub_state) in extra_handles {
                            // Same rotation discipline as the root path
                            // — capture lastScan before reload.
                            rotate_last_scan(&sub_state, &diff_sessions, &sub_root).await;

                            // We rescan each sub-project independently —
                            // its own `.loctree/` is the source of truth.
                            if let Err(err) = run_scan(&sub_root).await {
                                tracing::warn!(
                                    "loctree-lsp: sub-workspace rescan failed for {}: {err}",
                                    sub_root.display()
                                );
                                continue;
                            }
                            if let Err(err) = sub_state.load(&sub_root).await {
                                tracing::warn!(
                                    "loctree-lsp: sub-workspace reload failed for {}: {err}",
                                    sub_root.display()
                                );
                            }
                        }

                        client
                            .send_notification::<LoctreeScanProgress>(ScanProgress::with_counts(
                                ScanPhase::Done,
                                scan_stats,
                            ))
                            .await;
                    }
                    Err(err) => {
                        client
                            .send_notification::<LoctreeScanProgress>(ScanProgress::failed(
                                format!("rescan failed: {err}"),
                            ))
                            .await;
                    }
                }
            }
        });

        *self.watcher_task.write().await = Some(task);
    }

    /// Custom request handler for `loctree/contextAtlas` (Plan 02 of the LSP roadmap).
    ///
    /// Returns a typed pointer to the Context Atlas materialized at
    /// `<workspace_root>/.loctree/context-atlas/manifest.json`. Cards
    /// are surfaced as paths only — agents read content from disk.
    /// When the atlas is missing, the response carries `status: "missing"`
    /// and `next_action: "loct auto"` so the caller can decide.
    pub async fn context_atlas(&self, params: ContextAtlasParams) -> Result<ContextAtlasResponse> {
        // Plan 13: route via `params.project` when present.
        let routed = self.routed_root(params.project.as_deref()).await;
        let workspace_root = routed.ok_or_else(|| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
            message: "loctree-lsp has no workspace root yet — wait for `initialized`".into(),
            data: None,
        })?;
        Ok(context_atlas::compute(&workspace_root, &params))
    }

    /// Custom request handler for `loctree/contextPack`.
    ///
    /// Streams materialized Context Atlas cards one section at a time with the
    /// same cursor/card semantics as MCP HTTP `/context_pack`, but reuses the
    /// already-loaded routed LSP snapshot instead of shelling out or scanning.
    pub async fn context_pack(&self, params: ContextPackParams) -> Result<ContextPackResponse> {
        let routed = self.routed_snapshot(params.project.as_deref()).await;
        let routed_root = self
            .routed_root(params.project.as_deref())
            .await
            .ok_or_else(|| tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
                message: "loctree-lsp has no workspace root yet — wait for `initialized`".into(),
                data: None,
            })?;
        let guard = routed
            .get()
            .await
            .ok_or_else(|| tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
                message: "loctree snapshot not loaded yet — try again after `initialized`".into(),
                data: None,
            })?;
        let loaded = guard.as_ref().ok_or_else(|| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
            message: "loctree snapshot is empty".into(),
            data: None,
        })?;

        context_pack::compute(&routed_root, &loaded.snapshot, &params).map_err(|err| {
            let code = match err {
                context_pack::ContextPackError::BadRequest(_) => {
                    tower_lsp::jsonrpc::ErrorCode::InvalidParams
                }
                context_pack::ContextPackError::NotFound(_) => {
                    tower_lsp::jsonrpc::ErrorCode::ServerError(-32004)
                }
                context_pack::ContextPackError::Gone(_) => {
                    tower_lsp::jsonrpc::ErrorCode::ServerError(-32010)
                }
                context_pack::ContextPackError::Internal(_) => {
                    tower_lsp::jsonrpc::ErrorCode::InternalError
                }
            };
            let mut lsp_error = tower_lsp::jsonrpc::Error {
                code,
                message: err.message().to_string().into(),
                data: None,
            };
            lsp_error.data = Some(serde_json::json!({
                "method": "loctree/contextPack",
                "kind": err.kind(),
            }));
            lsp_error
        })
    }

    /// Custom request handler for `loctree/health` (Plan 09 of the LSP roadmap).
    ///
    /// Repo-readiness gate. Returns 0-100 score, status (green/yellow/red),
    /// cycle/dead-export/twin/hotspot counts, snapshot freshness, and
    /// recommended actions. Daemon-mode agents can refuse destructive edits
    /// when score < 50 or breaking cycles exist.
    ///
    /// Errors:
    /// - `ServerError(-32001)` when no snapshot has loaded yet.
    pub async fn health(&self, params: HealthParams) -> Result<HealthResponse> {
        // Plan 13: route to per-workspace snapshot when `project` is set.
        let routed = self.routed_snapshot(params.project.as_deref()).await;
        let guard = routed
            .get()
            .await
            .ok_or_else(|| tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
                message: "loctree snapshot not loaded yet — try again after `initialized`".into(),
                data: None,
            })?;
        let loaded = guard.as_ref().ok_or_else(|| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
            message: "loctree snapshot is empty".into(),
            data: None,
        })?;

        let stale = is_snapshot_stale(&loaded.snapshot, &loaded.workspace_root);
        Ok(health::compute_health(
            &loaded.snapshot,
            &loaded.workspace_root,
            stale,
            &params,
        ))
    }

    /// Custom request handler for `loctree/follow` (Plan 15 of the LSP roadmap).
    ///
    /// Consolidates the analyzer's structural-signal surface (cycles,
    /// dead exports, twins, hotspots, ...) under one verb. Mirrors
    /// `loct follow <scope>` from the CLI.
    ///
    /// Errors:
    /// - `ServerError(-32001)` when no snapshot has loaded yet.
    pub async fn follow(&self, params: FollowParams) -> Result<FollowResponse> {
        let routed = self.routed_snapshot(params.project.as_deref()).await;
        let guard = routed
            .get()
            .await
            .ok_or_else(|| tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
                message: "loctree snapshot not loaded yet — try again after `initialized`".into(),
                data: None,
            })?;
        let loaded = guard.as_ref().ok_or_else(|| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
            message: "loctree snapshot is empty".into(),
            data: None,
        })?;

        let snapshot_id = semantic_handler::snapshot_pagination_id(&loaded.snapshot);
        let chunk_size = match params.chunk_size {
            Some(value) => value,
            None => *self.default_chunk_size.read().await,
        };
        let mut params = params;
        params.chunk_size = Some(chunk_size);

        follow::compute_paginated(
            &loaded.snapshot,
            &loaded.workspace_root,
            &params,
            &snapshot_id,
        )
        .map_err(|err| {
            tower_lsp::jsonrpc::Error::invalid_params(format!(
                "loctree/follow cursor decode failed: {err}"
            ))
        })
    }

    /// Custom request handler for `loctree/body`.
    ///
    /// Returns the bounded source body for `symbol` using
    /// [`loctree::body::query_symbol_body`] — the same engine the `loct body`
    /// CLI uses, so the response is byte-for-byte the `loct body --json` shape
    /// (`{ symbol, bodies: [...] }`). An optional `file` filter disambiguates
    /// symbols defined in more than one file.
    ///
    /// Errors:
    /// - `ServerError(-32001)` when no snapshot has loaded yet.
    pub async fn body(&self, params: BodyParams) -> Result<BodyResponse> {
        let routed = self.routed_snapshot(params.project.as_deref()).await;
        let guard = routed
            .get()
            .await
            .ok_or_else(|| tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
                message: "loctree snapshot not loaded yet — try again after `initialized`".into(),
                data: None,
            })?;
        let loaded = guard.as_ref().ok_or_else(|| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
            message: "loctree snapshot is empty".into(),
            data: None,
        })?;

        let result =
            loctree::body::query_symbol_body(&loaded.snapshot, &params.symbol, params.max_lines);

        Ok(BodyResponse::from_result(result, params.file.as_deref()))
    }

    /// Custom request handler for `loctree/find` (Plan 07 of the LSP roadmap).
    ///
    /// Semantic-aware symbol search. Delegates to
    /// `loctree::analyzer::search::run_search` and applies LSP-side filters
    /// (mode / lang / dead_only / exported_only / limit).
    ///
    /// Errors:
    /// - `ServerError(-32001)` when no snapshot has loaded yet.
    pub async fn find(&self, params: FindParams) -> Result<FindResponse> {
        let routed = self.routed_snapshot(params.project.as_deref()).await;
        let guard = routed
            .get()
            .await
            .ok_or_else(|| tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
                message: "loctree snapshot not loaded yet — try again after `initialized`".into(),
                data: None,
            })?;
        let loaded = guard.as_ref().ok_or_else(|| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
            message: "loctree snapshot is empty".into(),
            data: None,
        })?;

        // Mode: literal — the W1 exact-identifier truth layer. Intercept BEFORE
        // run_search so the fuzzy/AST engine never touches the literal answer.
        // Reuses the shared occurrences scanner via `find::scan_literal`, so the
        // file/line set is identical to `loct occurrences` and the MCP
        // `find(mode=literal)` for the same snapshot.
        if params.mode.eq_ignore_ascii_case("literal") {
            let base = self.routed_root(params.project.as_deref()).await;
            let literal = self.literal_cache.get_or_scan(
                &loaded.snapshot,
                base.as_deref(),
                &params.query,
                loctree::analyzer::occurrences::ScanOptions {
                    whole_token: params.whole_token,
                },
                loctree::analyzer::occurrences::FileScope {
                    file: params.file.as_deref(),
                },
            );
            let fuzzy = loctree::analyzer::search::literal_fuzzy_suggestions(
                params.query.trim(),
                &loaded.snapshot.files,
            );
            return Ok(find::build_literal_response(literal, fuzzy, &params));
        }

        let query = find::build_query(&params);
        let results = loctree::analyzer::search::run_search(&query, &loaded.snapshot.files);
        let snapshot_id = semantic_handler::snapshot_pagination_id(&loaded.snapshot);
        let chunk_size = match params.chunk_size {
            Some(value) => value,
            None => *self.default_chunk_size.read().await,
        };
        let mut params = params;
        params.chunk_size = Some(chunk_size);

        find::build_response_paginated(results, &params, &snapshot_id).map_err(|err| {
            tower_lsp::jsonrpc::Error::invalid_params(format!(
                "loctree/find cursor decode failed: {err}"
            ))
        })
    }

    /// Custom request handler for `loctree/impact` (Plan 06 of the LSP roadmap).
    ///
    /// Returns blast-radius analysis (direct + optional transitive consumers)
    /// for a target file using `loctree::impact::analyze_impact`. Severity is
    /// classified per plan heuristic; dynamic-import edges are flagged as
    /// warnings.
    ///
    /// Errors:
    /// - `ServerError(-32001)` when no snapshot has loaded yet.
    pub async fn impact(&self, params: ImpactParams) -> Result<ImpactResponse> {
        let routed = self.routed_snapshot(params.project.as_deref()).await;
        let guard = routed
            .get()
            .await
            .ok_or_else(|| tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
                message: "loctree snapshot not loaded yet — try again after `initialized`".into(),
                data: None,
            })?;
        let loaded = guard.as_ref().ok_or_else(|| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
            message: "loctree snapshot is empty".into(),
            data: None,
        })?;

        let target = impact::target_string(&params);
        let opts = impact::options_from_params(&params);
        let result = loctree::impact::analyze_impact(&loaded.snapshot, &target, &opts);

        Ok(ImpactResponse::from_impact(&result, params.transitive))
    }

    /// Custom request handler for `loctree/slice` (Plan 05 of the LSP roadmap).
    ///
    /// Returns a holographic slice (core + deps + optional consumers) for the
    /// target file using `loctree::slicer::HolographicSlice`. Paths-only by
    /// contract — agents fetch file content separately.
    ///
    /// Errors:
    /// - `ServerError(-32001)` when no snapshot has loaded yet.
    /// - `InvalidParams` when the target file is not present in the current snapshot.
    pub async fn slice(&self, params: SliceParams) -> Result<SliceResponse> {
        let routed = self.routed_snapshot(params.project.as_deref()).await;
        let guard = routed
            .get()
            .await
            .ok_or_else(|| tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
                message: "loctree snapshot not loaded yet — try again after `initialized`".into(),
                data: None,
            })?;
        let loaded = guard.as_ref().ok_or_else(|| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
            message: "loctree snapshot is empty".into(),
            data: None,
        })?;

        let target = slice::target_string(&params);
        let cfg = slice::config_from_params(&params);

        let holographic =
            loctree::slicer::HolographicSlice::from_path(&loaded.snapshot, &target, &cfg)
                .ok_or_else(|| {
                    let mut err = tower_lsp::jsonrpc::Error::invalid_params(format!(
                        "target not present in snapshot: {target}"
                    ));
                    err.data = Some(serde_json::json!({ "target": target }));
                    err
                })?;

        let snapshot_id = semantic_handler::snapshot_pagination_id(&loaded.snapshot);
        let chunk_size = match params.chunk_size {
            Some(value) => value,
            None => *self.default_chunk_size.read().await,
        };

        let identity = ResponseIdentity::from_snapshot(
            params.project.as_deref(),
            &loaded.workspace_root,
            &loaded.snapshot,
            snapshot_id.clone(),
        );

        SliceResponse::from_holographic_paginated(
            &holographic,
            params.cursor.as_deref(),
            chunk_size,
            &snapshot_id,
        )
        .map(|response| response.with_identity(identity))
        .map_err(|err| {
            tower_lsp::jsonrpc::Error::invalid_params(format!(
                "loctree/slice cursor decode failed: {err}"
            ))
        })
    }

    /// Custom request handler for `loctree/symbolContext` — the keystone
    /// of the Context-King surface.
    ///
    /// Returns one bounded literal context pack for the symbol at
    /// `file + position`: export/internal status (resolved from the symbol
    /// IDENTITY via the snapshot lookup `hover.rs` uses — NOT from `find`
    /// literal's `dead_status` stub), the bounded body (reusing
    /// [`loctree::body::query_symbol_body`] with `file` disambiguation),
    /// paginated literal occurrences (reusing the shared occurrences
    /// scanner), and best-effort `parent_context` from the live tree.
    ///
    /// Errors:
    /// - `ServerError(-32001)` when no snapshot has loaded yet.
    pub async fn symbol_context(
        &self,
        params: SymbolContextParams,
    ) -> Result<SymbolContextResponse> {
        let routed = self.routed_snapshot(params.project.as_deref()).await;
        let guard = routed
            .get()
            .await
            .ok_or_else(|| tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
                message: "loctree snapshot not loaded yet — try again after `initialized`".into(),
                data: None,
            })?;
        let loaded = guard.as_ref().ok_or_else(|| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::ServerError(-32001),
            message: "loctree snapshot is empty".into(),
            data: None,
        })?;

        let position: Position = params.position.into();

        // 1. Symbol IDENTITY → export/internal. Reuses the same snapshot
        //    file/line lookup the hover provider uses; falls back to the
        //    `symbol` hint when no declaration sits on the cursor line.
        let identity = symbol_context::resolve_identity(
            &loaded.snapshot,
            &params.file,
            position,
            params.symbol.as_deref(),
        );

        // Resolved name: identity wins, else the caller's hint, else empty.
        let symbol_name = identity
            .as_ref()
            .map(|id| id.name.clone())
            .or_else(|| params.symbol.clone())
            .unwrap_or_default();

        let (exported, internal, range, kind, defined_in) = match &identity {
            Some(id) => (
                Some(id.exported),
                Some(id.internal),
                symbol_context::line_range(id.line),
                id.kind.clone(),
                id.defined_in.clone(),
            ),
            None => (None, None, None, None, None),
        };

        // 2. Body — disambiguated to the file that actually DECLARES the symbol.
        //    For a local symbol that is `params.file`; for a symbol resolved
        //    cross-file through the import graph it is the declaring file
        //    `defined_in`, so an imported symbol shows its real body instead of
        //    body_error="not_found_in_file". We still never scan-and-guess a
        //    body from an unrelated same-named file (the showBody trap).
        let body_file = defined_in.as_deref().unwrap_or(params.file.as_str());
        let body_resolution = if symbol_name.is_empty() {
            symbol_context::BodyResolution {
                body: None,
                error: None,
            }
        } else {
            symbol_context::build_body(
                &loaded.snapshot,
                &symbol_name,
                body_file,
                params.body_max_lines_resolved(),
            )
        };

        // 3. Occurrences — literal scan, reusing the shared scanner the same
        //    way `loctree/find` mode=literal does.
        let occurrences = if symbol_name.is_empty() {
            symbol_context::OccurrencesContext {
                total: 0,
                same_file_total: 0,
                returned: Vec::new(),
                has_more: false,
                next_offset: None,
            }
        } else {
            let base = self.routed_root(params.project.as_deref()).await;
            let literal = self.literal_cache.get_or_scan(
                &loaded.snapshot,
                base.as_deref(),
                &symbol_name,
                symbol_context::scan_options(&params),
                loctree::analyzer::occurrences::FileScope::default(),
            );
            symbol_context::build_occurrences(
                &literal,
                &params.file,
                params.same_file_only,
                params.offset,
                params.occurrence_limit_resolved(),
            )
        };

        // 4. Parent context — best-effort from the live tree, never fatal.
        let parent_context = match self.routed_root(params.project.as_deref()).await {
            Some(root) => Some(symbol_context::build_parent_context(
                &self.live_ast,
                &root,
                &params.file,
                position,
            )),
            None => Some(symbol_context::ParentContext::unavailable()),
        };

        Ok(SymbolContextResponse {
            symbol: symbol_name,
            file: params.file,
            range,
            kind,
            exported,
            internal,
            defined_in,
            body: body_resolution.body,
            body_error: body_resolution.error.map(|s| s.to_string()),
            occurrences,
            parent_context,
        })
    }

    /// Plan 17 MVP: parse `content` through [`LiveAstStore`] and emit
    /// the `loctree/documentChanged` notification when the parse
    /// produced a tree. Plan 18 v2: also runs the symbol-set diff and
    /// emits `loctree/symbolChanged` when the parse changed the
    /// top-level function / class set.
    ///
    /// Silent no-op for unsupported file extensions — the store
    /// doesn't accept them, so the daemon stays quiet rather than
    /// emitting a notification it cannot back with real AST data.
    async fn update_live_ast(&self, uri: &Url, version: i32, content: &str) {
        if let Some(payload) = self.live_ast.update(uri, version, content) {
            self.client
                .send_notification::<LoctreeDocumentChanged>(payload.clone())
                .await;
            self.emit_symbol_changes(uri, payload.version).await;
        }
    }

    /// Plan 17 v2: feed INCREMENTAL `TextDocumentContentChangeEvent`s
    /// through [`LiveAstStore::apply_change`], emit
    /// `loctree/documentChanged`, and keep [`Self::documents`] in sync
    /// with the post-edit content so existing handlers (goto_def,
    /// references) still see what the editor is showing. Plan 18 v2
    /// also runs the symbol-set diff after the parse and emits
    /// `loctree/symbolChanged` when the top-level function / class set
    /// changed.
    async fn apply_live_ast_changes(
        &self,
        uri: &Url,
        version: i32,
        events: &[tower_lsp::lsp_types::TextDocumentContentChangeEvent],
    ) {
        if let Some(payload) = self.live_ast.apply_change(uri, version, events) {
            // Mirror the post-edit content into the legacy `documents`
            // cache so non-AST handlers stay coherent. The live store
            // is the source of truth for the buffer; this is a
            // bookkeeping echo.
            if let Some(doc) = self.live_ast.get(uri) {
                self.documents.insert(uri.clone(), doc.content.clone());
            }
            let payload_version = payload.version;
            self.client
                .send_notification::<LoctreeDocumentChanged>(payload)
                .await;
            self.emit_symbol_changes(uri, payload_version).await;
        } else {
            // Unsupported language or no live tree was produced — fall
            // back to the legacy "treat last event text as full doc"
            // behavior so non-AST handlers still see the latest buffer.
            if let Some(last) = events.last() {
                self.documents.insert(uri.clone(), last.text.clone());
            }
        }
    }

    /// Plan 18 v2: extract symbols from the live tree for `uri`, diff
    /// against the cached metadata in [`Self::symbol_tracker`], and
    /// emit `loctree/symbolChanged` when the diff is non-empty.
    ///
    /// Resolves the workspace-relative path for the URI so the
    /// `SymbolIdV1` keys stay stable across edits; if the URI escapes
    /// the workspace root the file basename is used as a fallback.
    async fn emit_symbol_changes(&self, uri: &Url, version: i32) {
        let Some(doc) = self.live_ast.get(uri) else {
            return;
        };

        // Resolve workspace-relative path with a stable fallback.
        let file_path = if let Some(root) = self.workspace_root_path().await {
            crate::live_ast::LiveDocument::workspace_relative(uri, &root).unwrap_or_else(|| {
                uri.to_file_path()
                    .ok()
                    .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| uri.path().trim_start_matches('/').to_string())
            })
        } else {
            uri.to_file_path()
                .ok()
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
                .unwrap_or_else(|| uri.path().trim_start_matches('/').to_string())
        };

        let symbols = live_ast::extract_live_symbols(&doc.tree);
        let current_map = live_ast::build_symbol_map(&file_path, &symbols);

        let prev = {
            let tracker = self.symbol_tracker.read().await;
            tracker.get(uri).cloned()
        };

        let (changes, next_metadata) =
            live_ast::diff_symbol_sets(&file_path, prev.as_ref(), &current_map);

        // Always commit the new metadata so the next edit diffs against
        // a fresh baseline — even if no changes fired this pass.
        {
            let mut tracker = self.symbol_tracker.write().await;
            tracker.insert(uri.clone(), next_metadata);
        }

        if changes.is_empty() {
            return;
        }
        self.client
            .send_notification::<LoctreeSymbolChanged>(crate::live_ast::SymbolChanged {
                uri: uri.clone(),
                version,
                changes,
            })
            .await;
    }

    /// Trigger diagnostics for a document
    async fn publish_diagnostics(&self, uri: Url) {
        // Extract file path from URI
        let file_path = uri.path();
        tracing::debug!("Analyzing: {}", file_path);

        // Collect diagnostics from snapshot
        let diags = diagnostics::collect_diagnostics(&self.snapshot, file_path).await;

        // Cache and publish
        self.cached_diagnostics.insert(uri.clone(), diags.clone());
        self.client.publish_diagnostics(uri, diags, None).await;
    }

    /// Get the workspace root path
    pub async fn workspace_root(&self) -> Option<String> {
        self.workspace_root.read().await.clone()
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Store workspace root — unless a `--root` pin already established
        // one. The pin is a deliberate fixed root (`loct watch --lsp`), so
        // a client's `rootUri` must not silently relocate the workspace.
        if let Some(root) = params.root_uri {
            let mut wr = self.workspace_root.write().await;
            if let Some(pinned) = wr.as_deref() {
                tracing::info!(
                    "Ignoring initialize rootUri {} — workspace root already pinned to {}",
                    root.path(),
                    pinned
                );
            } else {
                *wr = Some(root.path().to_string());
                tracing::info!("Workspace root: {}", root.path());
            }
        }

        // Plan 10: read watcher config from initializationOptions.
        let watcher_cfg = config_from_options(params.initialization_options.as_ref());
        tracing::debug!(
            "watcher config: enabled={} debounce_ms={}",
            watcher_cfg.enabled,
            watcher_cfg.debounce.as_millis()
        );
        *self.watcher_config.write().await = watcher_cfg;

        // Plan 13: read multi-workspace discovery depth.
        let depth = max_depth_from_options(params.initialization_options.as_ref());
        *self.workspaces_max_depth.write().await = depth;
        tracing::debug!("workspaces config: max_depth={}", depth);

        // Plan 12: read default chunk size for paginated handlers.
        let chunk = chunk_size_from_options(params.initialization_options.as_ref());
        *self.default_chunk_size.write().await = chunk;
        tracing::debug!("protocol config: default_chunk_size={}", chunk);

        // Code lenses are opt-in (off by default) to avoid cluttering the
        // gutter alongside the real language server's lenses.
        let code_lens_enabled = code_lens_from_options(params.initialization_options.as_ref());
        tracing::debug!("protocol config: code_lens_enabled={}", code_lens_enabled);

        Ok(initialize_result(
            self.live_ast.capability_json(),
            code_lens_enabled,
        ))
    }

    async fn initialized(&self, _: InitializedParams) {
        tracing::info!("loctree-lsp server initialized");

        // Load snapshot from workspace
        self.load_snapshot().await;

        // Plan 13: discover sub-project `.loctree/` directories and
        // load their snapshots into the routing map. Single-workspace
        // setups produce an empty map and stay on the fast path.
        self.discover_and_load_workspaces().await;

        // Plan 10: kick off the background watcher once the initial
        // snapshot is available.
        if let Some(root) = self.workspace_root.read().await.clone() {
            self.start_watcher(PathBuf::from(root)).await;
        }

        self.client
            .log_message(MessageType::INFO, "loctree-lsp ready")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        tracing::info!("loctree-lsp server shutting down");
        if let Some(handle) = self.watcher_task.write().await.take() {
            handle.abort();
        }
        // Plan 17 MVP: drop every cached live tree on shutdown so the
        // process exit doesn't leak the parser arenas.
        self.live_ast.clear();
        // Plan 18 v2: drop the symbol tracker so the next process
        // start sees a clean baseline.
        self.symbol_tracker.write().await.clear();
        Ok(())
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        match params.command.as_str() {
            cmd if cmd == actions::OPEN_ATLAS_CARD_COMMAND => {
                let Some(workspace_root) = self.workspace_root_path().await else {
                    let mut error = tower_lsp::jsonrpc::Error::invalid_params(
                        "loctree.openAtlasCard requires a known workspace root".to_string(),
                    );
                    error.data = Some(serde_json::json!({ "command": cmd }));
                    return Err(error);
                };
                match actions::validate_open_atlas_card_args(&params.arguments, &workspace_root) {
                    Ok(card_path) => Ok(Some(serde_json::json!({
                        "ok": true,
                        "card_path": card_path.display().to_string(),
                    }))),
                    Err(err) => {
                        let mut error = tower_lsp::jsonrpc::Error::invalid_params(err.to_string());
                        error.data = Some(serde_json::json!({ "command": cmd }));
                        Err(error)
                    }
                }
            }
            other => Err(tower_lsp::jsonrpc::Error::invalid_params(format!(
                "loctree-lsp does not handle executeCommand `{other}`"
            ))),
        }
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let version = params.text_document.version;
        let content = params.text_document.text.clone();
        tracing::debug!("did_open: {} ({} bytes)", uri, content.len());

        // Store document content
        self.documents.insert(uri.clone(), content.clone());

        // Plan 17 MVP: parse the just-opened buffer through
        // `loctree-ast` and emit `loctree/documentChanged` so agents
        // tracking edits don't have to poll for a fresh tree.
        self.update_live_ast(&uri, version, &content).await;

        // Trigger diagnostics
        self.publish_diagnostics(uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let version = params.text_document.version;
        tracing::debug!(
            "did_change: {} ({} events)",
            uri,
            params.content_changes.len()
        );

        if params.content_changes.is_empty() {
            return;
        }

        // Plan 17 v2: INCREMENTAL sync. Each event carries either a
        // `range`-scoped delta (per-edit `InputEdit` translation +
        // tree-sitter incremental reparse) or a range-less full-text
        // replacement (the store falls back to a full parse). The
        // helper keeps the legacy `documents` cache aligned with the
        // post-edit buffer.
        self.apply_live_ast_changes(&uri, version, &params.content_changes)
            .await;

        // Trigger diagnostics on change
        self.publish_diagnostics(uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        tracing::debug!("did_save: {}", uri);

        // Update content if provided
        if let Some(text) = params.text {
            self.documents.insert(uri.clone(), text.clone());
            // Plan 17 MVP: keep the live tree in lockstep with the
            // saved buffer. Same FULL-parse path as did_change — version
            // -1 because `DidSaveTextDocumentParams` has no version
            // field; agents that care can match on URI.
            self.update_live_ast(&uri, -1, &text).await;
        }

        // Trigger full diagnostics on save
        self.publish_diagnostics(uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        tracing::debug!("did_close: {}", uri);

        // Remove from document cache
        self.documents.remove(&uri);

        // Plan 17 MVP: drop the live tree so future `astQuery` calls
        // for this file fall back to the on-disk reparse path.
        self.live_ast.remove(&uri);

        // Plan 18 v2: drop the symbol tracker entry so a future open
        // diffs against an empty baseline (every symbol fires `added`).
        {
            let mut tracker = self.symbol_tracker.write().await;
            tracker.remove(&uri);
        }

        // Clear cached diagnostics
        self.cached_diagnostics.remove(&uri);

        // Publish empty diagnostics to clear any shown in the editor
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn did_change_configuration(&self, _params: DidChangeConfigurationParams) {
        tracing::debug!("did_change_configuration");
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri.clone();
        let range = params.range;
        tracing::debug!("code_action: {} at {:?}", uri, range);

        let mut code_actions: Vec<CodeActionOrCommand> = Vec::new();
        let file_path = uri.path();

        // Get document content for symbol detection
        let content = self.documents.get(&uri).map(|doc| doc.clone());

        // Get cached diagnostics for this file
        let diagnostics_in_range: Vec<Diagnostic> = self
            .cached_diagnostics
            .get(&uri)
            .map(|d| {
                d.iter()
                    .filter(|diag| ranges_overlap(&diag.range, &range))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        // Get workspace root for quickfix actions
        let workspace_root = self.workspace_root.read().await;
        let root = workspace_root.as_deref();

        // Add quickfix actions for diagnostics (cycles, dead exports)
        for diag in &diagnostics_in_range {
            if let Some(NumberOrString::String(code)) = &diag.code {
                match code.as_str() {
                    code if code == DiagnosticCode::CircularImport.as_str() || code == "cycle" => {
                        // Add quickfix actions for cycles
                        let quickfix_actions = actions::cycle_fixes(diag, &uri);
                        for action in quickfix_actions {
                            code_actions.push(CodeActionOrCommand::CodeAction(action));
                        }

                        // Get cycle info from snapshot and add cycle refactors
                        let cycles = self.snapshot.cycles_for_file(file_path).await;
                        for cycle in cycles {
                            let cycle_actions = actions::cycle_refactors(file_path, &cycle.files);
                            for action in cycle_actions {
                                code_actions.push(CodeActionOrCommand::CodeAction(action));
                            }
                        }
                    }
                    code if code == DiagnosticCode::DeadExport.as_str() => {
                        // Add quickfix actions for dead exports
                        let quickfix_actions = actions::dead_export_fixes(diag, &uri, root);
                        for action in quickfix_actions {
                            code_actions.push(CodeActionOrCommand::CodeAction(action));
                        }

                        // Extract symbol from diagnostic message for refactor actions
                        if let Some(symbol) = extract_symbol_from_diagnostic(&diag.message) {
                            let export_actions = actions::export_refactors(&symbol, &uri, 0);
                            for action in export_actions {
                                code_actions.push(CodeActionOrCommand::CodeAction(action));
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Plan 04: offer "Open Context Atlas card" for any diagnostic
            // whose code maps to a card. Atlas-missing → silent (no broken
            // link). Resolves the workspace root each iteration so we
            // don't hold the lock across the whole `for` body.
            if let Some(root_str) = root {
                let workspace_path = std::path::Path::new(root_str);
                if let Some(action) = actions::atlas_card_action(diag, workspace_path) {
                    code_actions.push(CodeActionOrCommand::CodeAction(action));
                }
            }
        }

        // Release workspace root lock before further async operations
        drop(workspace_root);

        // Add file-level refactoring actions
        let consumers = self.snapshot.find_references(file_path, None).await;
        let file_actions = actions::file_refactors(&uri, file_path, consumers.len());
        for action in file_actions {
            code_actions.push(CodeActionOrCommand::CodeAction(action));
        }

        // Add symbol-specific refactoring actions if cursor is on a symbol
        if let Some(ref content) = content
            && let Some(symbol) = get_word_at_position(content, range.start)
        {
            // Find how many files import this symbol
            let references = self
                .snapshot
                .find_references(file_path, Some(&symbol))
                .await;
            let symbol_actions = actions::export_refactors(&symbol, &uri, references.len());
            for action in symbol_actions {
                code_actions.push(CodeActionOrCommand::CodeAction(action));
            }
        }

        if code_actions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(code_actions))
        }
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = params.text_document.uri.clone();
        let file_path = uri.path();
        tracing::debug!("code_lens: {}", file_path);

        let lenses = code_lens::code_lens_for_file(&self.snapshot, file_path).await;
        if lenses.is_empty() {
            Ok(None)
        } else {
            Ok(Some(lenses))
        }
    }
}

/// Check if two ranges overlap
fn ranges_overlap(a: &Range, b: &Range) -> bool {
    // Ranges overlap if neither is completely before or after the other
    !(a.end.line < b.start.line
        || (a.end.line == b.start.line && a.end.character < b.start.character)
        || b.end.line < a.start.line
        || (b.end.line == a.start.line && b.end.character < a.start.character))
}

/// Extract symbol name from diagnostic message
///
/// Parses messages like "Export 'foo' is unused (0 imports)" to extract "foo"
fn extract_symbol_from_diagnostic(message: &str) -> Option<String> {
    // Look for text between single quotes
    let start = message.find('\'')?;
    let rest = &message[start + 1..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

/// Check if snapshot is stale (git HEAD changed since the snapshot was created).
///
/// Uses loctree's git integration to compare HEAD commit with the snapshot's
/// recorded commit hash. Returns false if git info is unavailable.
fn is_snapshot_stale(snapshot: &Snapshot, project: &Path) -> bool {
    if let Some(snapshot_commit) = &snapshot.metadata.git_commit
        && let Some(current_commit) = loctree::git::GitRepo::discover(project)
            .ok()
            .and_then(|r| r.head_commit().ok())
    {
        return !(current_commit.starts_with(snapshot_commit)
            || snapshot_commit.starts_with(&current_commit));
    }
    false
}

/// Per-workspace diff-session map shared between Backend and the
/// watcher loop. Aliased to dodge clippy's `type_complexity`
/// complaint without losing the explicit `Arc<RwLock<DiffSession>>`
/// nesting that gives each workspace independent locking.
type DiffSessionMap = Arc<RwLock<HashMap<PathBuf, Arc<RwLock<DiffSession>>>>>;

/// Plan 11 helper: lift the current snapshot out of `state` and
/// install it as the `last_scan` baseline for the workspace at `root`
/// before the watcher reloads. No-op when the workspace currently has
/// no loaded snapshot — the next reload will bootstrap the baseline
/// directly.
async fn rotate_last_scan(state: &SnapshotState, sessions: &DiffSessionMap, root: &Path) {
    let snapshot_arc = {
        let guard = match state.get().await {
            Some(g) => g,
            None => return,
        };
        match guard.as_ref() {
            Some(loaded) => Arc::new(loaded.snapshot.clone()),
            None => return,
        }
    };

    let canonical = workspaces::canonicalize(root);
    let mut sessions = sessions.write().await;
    let session_handle = sessions
        .entry(canonical)
        .or_insert_with(|| Arc::new(RwLock::new(DiffSession::default())))
        .clone();
    drop(sessions);
    session_handle.write().await.set_last_scan(snapshot_arc);
}

/// Build a [`WorkspaceInfo`] row from a [`SnapshotState`] + workspace
/// path. Helper used by the `loctree/workspaces` handler — keeps the
/// per-workspace serialization shape in one place so root and
/// sub-projects render identically.
async fn workspace_info(state: &SnapshotState, root: &Path, is_root: bool) -> WorkspaceInfo {
    let canonical = workspaces::canonicalize(root);
    let snapshot_age_seconds = workspaces::snapshot_age(&canonical);

    let guard = state.get().await;
    let (has_snapshot, files, languages) = match guard {
        Some(g) => match g.as_ref() {
            Some(loaded) => {
                let mut langs: Vec<String> =
                    loaded.snapshot.metadata.languages.iter().cloned().collect();
                langs.sort();
                (true, loaded.snapshot.files.len(), langs)
            }
            None => (false, 0, Vec::new()),
        },
        None => (false, 0, Vec::new()),
    };

    WorkspaceInfo {
        root: canonical.display().to_string(),
        is_root,
        has_snapshot,
        files,
        languages,
        snapshot_age_seconds,
    }
}

/// Run loctree scan in-process using library API.
///
/// Uses `spawn_blocking` since `run_init_with_options` is synchronous.
/// `quiet_summary: true` prevents stdout pollution (LSP uses stdio for JSON-RPC).
/// Respects `.loctignore` patterns from the project root.
async fn run_scan(project: &Path) -> anyhow::Result<ScanStats> {
    let project = project.to_path_buf();
    tokio::task::spawn_blocking(move || {
        use loctree::args::ParsedArgs;
        let roots = vec![project.clone()];
        let parsed = ParsedArgs {
            ignore_patterns: loctree::fs_utils::load_loctreeignore(&project),
            ..ParsedArgs::default()
        };
        // LSP receives an explicit workspace root from the client. Treat that
        // path as authoritative so non-git folders do not fall back to the
        // daemon process CWD and write the snapshot under the wrong project id.
        loctree::snapshot::run_init_with_options_for_strategy(
            &roots,
            &parsed,
            true,
            loctree::snapshot::SnapshotRootStrategy::Exact,
        )
        .map_err(|e| anyhow::anyhow!("Scan failed: {}", e))?;
        let snapshot = loctree::snapshot::Snapshot::load(&project)
            .map_err(|e| anyhow::anyhow!("Scan completed but snapshot reload failed: {}", e))?;
        let total_files = snapshot.metadata.file_count.max(snapshot.files.len());
        Ok(ScanStats {
            files_processed: total_files,
            total_files,
        })
    })
    .await?
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    fn doc_changed_cap() -> serde_json::Value {
        LiveAstStore::new().capability_json()
    }

    #[test]
    fn code_lens_provider_absent_by_default() {
        let caps = server_capabilities(doc_changed_cap(), false);
        assert!(caps.code_lens_provider.is_none());
    }

    #[test]
    fn code_lens_provider_present_when_opted_in() {
        let caps = server_capabilities(doc_changed_cap(), true);
        assert!(caps.code_lens_provider.is_some());
    }

    #[test]
    fn static_initialize_result_keeps_code_lens_off() {
        let result = static_initialize_result();
        assert!(result.capabilities.code_lens_provider.is_none());
    }
}
