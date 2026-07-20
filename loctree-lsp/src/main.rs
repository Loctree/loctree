//! Loctree LSP Server binary entry point
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::io::Write as _;
use std::panic;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgAction, Parser};
use loctree_lsp::{run_server, static_initialize_result};

#[derive(Debug, Parser)]
#[command(
    name = "loctree-lsp",
    version = env!("LOCTREE_LSP_BUILD_VERSION"),
    about = "Language Server Protocol server for Loctree"
)]
struct Args {
    /// Enable debug logging for the LSP server.
    #[arg(long, action = ArgAction::SetTrue)]
    debug: bool,

    /// Use stdio transport (default and only transport). Accepted as a no-op
    /// for compatibility with LSP clients — notably `vscode-languageclient`,
    /// which always appends `--stdio` for stdio transport.
    #[arg(long, action = ArgAction::SetTrue)]
    stdio: bool,

    /// Print the server initialize result, including capabilities, as JSON and exit.
    #[arg(long, action = ArgAction::SetTrue)]
    capabilities: bool,

    /// Pin the LSP workspace root at startup.
    ///
    /// When set, the server adopts this directory as its workspace root
    /// immediately — before, and regardless of, the LSP `initialize`
    /// handshake's `rootUri`. Used by `loct watch --lsp`, which spawns
    /// this server as a co-process that may never receive a client
    /// `initialize`. Without `--root`, the workspace root is discovered
    /// from `initialize` exactly as before (backward compatible).
    #[arg(long, value_name = "DIR")]
    root: Option<PathBuf>,
}

/// Write to stderr without risking a secondary panic.
/// Inside a panic hook, `eprintln!` itself can panic when stderr is a broken pipe.
fn safe_stderr_log(line: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(line.as_bytes());
    let _ = stderr.write_all(b"\n");
    let _ = stderr.flush();
}

fn install_panic_hook() {
    panic::set_hook(Box::new(|panic_info| {
        let msg = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic".to_string()
        };

        // Broken pipe is expected when an editor disconnects
        if msg.contains("Broken pipe") || msg.contains("os error 32") {
            safe_stderr_log("[loctree-lsp] Editor disconnected (broken pipe), shutting down");
            std::process::exit(0);
        } else {
            let location = panic_info
                .location()
                .map(|loc| format!(" at {}:{}:{}", loc.file(), loc.line(), loc.column()))
                .unwrap_or_default();
            safe_stderr_log(&format!("[loctree-lsp] Panic{}: {}", location, msg));
        }
    }));
}

/// Ignore SIGPIPE so broken pipes surface as EPIPE errors instead of killing the process.
#[cfg(unix)]
fn ignore_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

#[cfg(not(unix))]
fn ignore_sigpipe() {}

#[tokio::main]
async fn main() -> ExitCode {
    ignore_sigpipe();
    install_panic_hook();

    let args = Args::parse();

    if args.capabilities {
        let result = static_initialize_result();
        if let Err(err) = serde_json::to_writer_pretty(std::io::stdout(), &result) {
            safe_stderr_log(&format!(
                "[loctree-lsp] Failed to print capabilities: {err}"
            ));
            return ExitCode::FAILURE;
        }
        println!();
        return ExitCode::SUCCESS;
    }

    let log_level = if args.debug {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive(log_level.into()),
        )
        .with_writer(std::io::stderr)
        // Prevent tracing from recursively writing fallback errors when stderr is closed.
        .log_internal_errors(false)
        .init();

    // `--stdio` is the default transport; the flag exists only for client
    // compatibility. Record it so the field is observed, not silenced.
    tracing::debug!(stdio = args.stdio, "loctree-lsp starting (stdio transport)");

    match run_server(args.root).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let err_str = format!("{:?}", e);
            if err_str.contains("Broken pipe") || err_str.contains("os error 32") {
                safe_stderr_log("[loctree-lsp] Editor disconnected, shutting down");
                ExitCode::SUCCESS
            } else {
                safe_stderr_log(&format!("[loctree-lsp] Error: {:#}", e));
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_flag_parses_workspace_pin() {
        let args = Args::parse_from(["loctree-lsp", "--root", "/tmp/x"]);
        assert_eq!(args.root, Some(PathBuf::from("/tmp/x")));
    }

    #[test]
    fn stdio_flag_is_accepted_for_client_compat() {
        // vscode-languageclient always appends `--stdio`; the server must
        // accept it without erroring (regression: clap rejected it -> exit 2).
        let args = Args::parse_from(["loctree-lsp", "--stdio"]);
        assert!(args.stdio);
        let default = Args::parse_from(["loctree-lsp"]);
        assert!(!default.stdio);
    }

    #[test]
    fn no_root_defaults_to_initialize_driven() {
        let args = Args::parse_from(["loctree-lsp"]);
        assert!(args.root.is_none());
    }
}
