//! Signal + panic handling for `loctree-mcp`.
//!
//! MCP servers are long-lived processes that talk JSON-RPC over stdio or
//! HTTP. We don't want to crash the process when:
//!   - the editor disconnects (broken pipe / EPIPE)
//!   - rmcp panics on a half-written response
//!
//! So we install a custom panic hook + ignore SIGPIPE so writes return
//! `EPIPE` errors that the surrounding code can handle, instead of being
//! killed by the kernel.
//!
//! Vibecrafted with AI Agents by VetCoders (c)2024-2026 LibraxisAI

use std::io::Write as _;
use std::panic;

/// Best-effort stderr logger that never re-panics.
///
/// Called from inside `install_panic_hook`'s closure so we can't use the
/// normal `tracing` macros (they may panic if the subscriber is in a bad
/// state). Locks stderr explicitly and ignores write failures.
pub(crate) fn safe_stderr_log(line: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(line.as_bytes());
    let _ = stderr.write_all(b"\n");
    let _ = stderr.flush();
}

/// Install a custom panic hook that catches broken-pipe panics from rmcp
/// (expected when the MCP client disconnects mid-write) and exits cleanly
/// with status 0. All other panics are logged with location info and the
/// default unwind behavior continues.
pub(crate) fn install_panic_hook() {
    panic::set_hook(Box::new(|panic_info| {
        let msg = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic".to_string()
        };

        // Check if this is a broken pipe - expected when client disconnects.
        if msg.contains("Broken pipe") || msg.contains("os error 32") {
            safe_stderr_log("[loctree-mcp] Client disconnected (broken pipe), shutting down");
            std::process::exit(0);
        } else {
            // Log other panics with location info.
            let location = panic_info
                .location()
                .map(|loc| format!(" at {}:{}:{}", loc.file(), loc.line(), loc.column()))
                .unwrap_or_default();
            safe_stderr_log(&format!("[loctree-mcp] Panic{}: {}", location, msg));
        }
    }));
}

/// Configure SIGPIPE handling to ignore broken pipes at OS level.
///
/// On Unix systems, writing to a closed pipe sends SIGPIPE which terminates
/// the process. We ignore it so the write fails with `EPIPE` instead and
/// surrounding code can handle the disconnect gracefully.
#[cfg(unix)]
pub(crate) fn ignore_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

#[cfg(not(unix))]
pub(crate) fn ignore_sigpipe() {
    // No-op on non-Unix platforms.
}
