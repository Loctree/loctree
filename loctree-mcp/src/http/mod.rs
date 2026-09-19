//! HTTP transport surface for loctree-mcp.
//!
//! Hosts `rmcp::transport::streamable_http_server::StreamableHttpService`
//! at `/mcp` on the configured bind address, plus the paginated
//! `/context_pack` endpoint. Both sit behind an axum router carrying the
//! bearer-auth layer from [`auth`].
//!
//! Vibecrafted with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

pub mod auth;
pub mod context_pack;

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::LoctreeServer;
use auth::{AuthSettings, HttpAuth};

async fn activity_touch_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    crate::touch_activity();
    next.run(request).await
}

/// Build and run the streamable-http axum server.
///
/// The service factory builds a fresh [`LoctreeServer`] per session; sessions
/// are managed by `LocalSessionManager` (in-memory, default). On Ctrl-C the
/// `CancellationToken` returns from the watch loop and `axum::serve` shuts
/// down gracefully.
///
/// The auth posture is resolved *here*, before the listener is bound, rather
/// than being handed in by the caller. That keeps the fail-safe property
/// structural: there is no way to reach the router — and no way to open a
/// socket — without having gone through [`auth::resolve`], which refuses a
/// non-loopback bind that has neither configured tokens nor an explicit
/// `--allow-unauthenticated`.
pub async fn serve_http(
    bind: &str,
    snapshot_cache_capacity: usize,
    auth_settings: &AuthSettings,
) -> Result<()> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };

    // Security gate first: refuse before any socket exists.
    let posture = auth::resolve(bind, auth_settings).await?;

    let cancel = CancellationToken::new();
    let config = StreamableHttpServerConfig::default().with_cancellation_token(cancel.clone());

    let service: StreamableHttpService<LoctreeServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(LoctreeServer::with_cache_capacity(snapshot_cache_capacity)),
            std::sync::Arc::new(LocalSessionManager::default()),
            config,
        );

    let router = axum::Router::new()
        .route(
            "/context_pack",
            axum::routing::get(context_pack::context_pack_handler),
        )
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn(activity_touch_middleware));

    // `Router::layer` wraps every route registered so far, so this single call
    // covers both `/mcp` and `/context_pack`. `/context_pack` matters just as
    // much: it takes a project directory off the query string and reads it.
    let router = match &posture {
        HttpAuth::Enforced(manager) => router.layer(axum::middleware::from_fn_with_state(
            manager.clone(),
            auth::bearer_guard,
        )),
        HttpAuth::LoopbackOpen | HttpAuth::ExplicitlyUnauthenticated => router,
    };

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("could not bind streamable-http server on {bind}"))?;
    let local = listener.local_addr().ok();
    if let Some(addr) = local {
        // Announce the bound address on stdout so orchestrators and integration
        // tests can target an ephemeral (`:0`) bind without pre-reserving a port
        // and then racing the OS into handing that same port to a second
        // process. The HTTP transport leaves stdout free — the MCP protocol
        // rides the TCP socket at `/mcp`; only the stdio transport reserves
        // stdout for JSON-RPC. Flush explicitly: a piped stdout is
        // block-buffered, so a reader blocking on this line would otherwise
        // deadlock until process exit.
        use std::io::Write as _;
        let mut stdout = std::io::stdout();
        let _ = writeln!(stdout, "loctree-mcp http listening on {addr}");
        let _ = stdout.flush();
    }
    info!(
        "Server ready. Streamable-http MCP at http://{}/mcp — {}",
        local
            .map(|a| a.to_string())
            .unwrap_or_else(|| bind.to_string()),
        posture.describe()
    );

    // Graceful shutdown on Ctrl-C / SIGTERM. `axum::serve` honors the
    // cancellation token via `with_graceful_shutdown`.
    let shutdown = {
        let cancel = cancel.clone();
        async move {
            let _ = tokio::signal::ctrl_c().await;
            cancel.cancel();
        }
    };

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .with_context(|| "axum::serve exited with error")?;

    Ok(())
}
