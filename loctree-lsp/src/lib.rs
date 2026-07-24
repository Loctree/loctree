//! Loctree Language Server Protocol implementation
//!
//! Provides IDE integration for dead code detection, cycles, and navigation.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

pub const BUILD_VERSION: &str = env!("LOCTREE_LSP_BUILD_VERSION");
pub const GIT_COMMIT: &str = env!("LOCTREE_LSP_GIT_COMMIT");
pub const GIT_DIRTY: bool = env!("LOCTREE_LSP_GIT_DIRTY").as_bytes()[0] == b'1';

use tower::ServiceBuilder;
use tower_lsp::jsonrpc::Request;
use tower_lsp::{LspService, Server};

pub mod actions;
pub mod aicx;
pub mod ast_query;
mod backend;
pub mod body;
pub mod code_lens;
pub mod context_atlas;
pub mod context_pack;
pub mod cursor;
pub mod diagnostic_codes;
mod diagnostics;
pub mod diff;
pub mod find;
pub mod follow;
pub mod health;
mod hover;
pub mod impact;
pub mod live_ast;
mod navigation;
pub mod protocol;
pub mod semantic;
pub mod slice;
mod snapshot;
pub mod symbol_context;
pub mod watcher;
pub mod workspaces;

pub use actions::{OPEN_ATLAS_CARD_COMMAND, atlas_card_action, validate_open_atlas_card_args};
pub use aicx::{AicxEntry, AicxParams, AicxResponse};
pub use ast_query::{AstQueryMatch, AstQueryParams, AstQueryResponse};
pub use backend::{
    Backend, initialize_result, server_capabilities, server_info, static_initialize_result,
};
pub use body::{BodyParams, BodyResponse};
pub use code_lens::{code_lens_for_file, format_title};
pub use context_atlas::{CardPointer, ContextAtlasParams, ContextAtlasResponse};
pub use context_pack::{ContextPackParams, ContextPackResponse};
pub use cursor::{CursorError, CursorState};
pub use diagnostic_codes::{ALL_EMITTED_CODES, DiagnosticCode, all_consumed_codes};
pub use diff::{DiffEdge, DiffParams, DiffResponse, DiffSymbol};
pub use find::{DeadStatusView, FindParams, FindResponse, SymbolSearchView};
pub use follow::{
    FollowParams, FollowResponse, FollowSummary, IMPLEMENTED_SCOPES, STUB_SCOPES, SUPPORTED_SCOPES,
};
pub use health::{HealthParams, HealthResponse, RiskItem};
pub use impact::{ImpactParams, ImpactResponse, ImporterEntry};
pub use live_ast::{
    DocumentChanged, LiveAstStore, LiveDocument, LiveSymbol, LoctreeDocumentChanged,
    LoctreeSymbolChanged, SymbolChange, SymbolChangeKind, SymbolChangeLocation, SymbolChanged,
    SymbolMetadata,
};
pub use protocol::{
    DEFAULT_CHUNK_SIZE, MAX_CHUNK_SIZE, Paginated, ResponseIdentity, chunk_size_from_options,
    paginate, single_page,
};
pub use semantic::{SemanticData, SemanticParams, SemanticResponse};
pub use slice::{SliceFileEntry, SliceParams, SliceResponse};
pub use snapshot::SnapshotState;
pub use symbol_context::{
    BodyContext, OccurrenceView, OccurrencesContext, ParentContext, SymbolContextParams,
    SymbolContextResponse, SymbolIdentity, SymbolPosition,
};
pub use watcher::{
    LoctreeScanProgress, ScanPhase, ScanProgress, ScanStats, WatcherConfig, config_from_options,
    should_trigger_rescan,
};
pub use workspaces::{
    DEFAULT_MAX_DEPTH as WORKSPACES_DEFAULT_MAX_DEPTH,
    MAX_DEPTH_CEILING as WORKSPACES_MAX_DEPTH_CEILING, WorkspaceInfo, WorkspacesParams,
    WorkspacesResponse, discover_loctree_dirs, max_depth_from_options,
};

/// Run the LSP server over stdio.
///
/// `root` pins the workspace root at startup (from `--root`). When `None`,
/// the workspace root is discovered from the LSP `initialize` handshake as
/// before — fully backward compatible.
pub async fn run_server(root: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(move |client| match root {
        Some(r) => Backend::with_root(client, r),
        None => Backend::new(client),
    })
    .custom_method("loctree/refresh", Backend::refresh)
    .custom_method("loctree/contextAtlas", Backend::context_atlas)
    .custom_method("loctree/body", Backend::body)
    .custom_method("loctree/symbolContext", Backend::symbol_context)
    .custom_method("loctree/contextPack", Backend::context_pack)
    .custom_method("loctree/find", Backend::find)
    .custom_method("loctree/follow", Backend::follow)
    .custom_method("loctree/health", Backend::health)
    .custom_method("loctree/impact", Backend::impact)
    .custom_method("loctree/slice", Backend::slice)
    .custom_method("loctree/workspaces", Backend::workspaces)
    .custom_method("loctree/diff", Backend::diff)
    .custom_method("loctree/semantic", Backend::semantic)
    .custom_method("loctree/aicx", Backend::aicx)
    .custom_method("loctree/astQuery", Backend::ast_query)
    .finish();
    let service = ServiceBuilder::new()
        .map_request(|mut req: Request| {
            // Some LSP clients send `shutdown` with `"params": null`.
            // tower-lsp treats null as invalid for no-params requests.
            let params_is_null = req.params().map(|v| v.is_null()).unwrap_or(false);
            if req.method() == "shutdown" && params_is_null {
                let method = req.method().to_string();
                let id = req.id().cloned();
                req = match id {
                    Some(id) => Request::build(method).id(id).finish(),
                    None => Request::build(method).finish(),
                };
            }
            req
        })
        .service(service);
    Server::new(stdin, stdout, socket).serve(service).await;

    Ok(())
}
