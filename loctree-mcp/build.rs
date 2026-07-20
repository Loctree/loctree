//! Build-time identity stamp for `loctree-mcp`.
//!
//! Captures the git commit (short SHA + dirty flag + `git describe`) of the
//! checkout the binary is built from and exports it via `cargo:rustc-env` so
//! the running server can announce it. The point is drift detection: two
//! binaries at the same crate version (e.g. `0.13.0`) but different commits are
//! otherwise indistinguishable, so an agent talking to a STALE MCP server reads
//! the same `serverInfo.version` as a fresh one and never notices the binary
//! lags source HEAD. Stamping the commit into the `initialize` handshake makes
//! that gap loud — see `loctree-feedback.md` ("live binary predates the committed
//! fix").
//!
//! Robustness: every git call is best-effort. Outside a git checkout (crates.io
//! tarball, vendored source) the stamp degrades to the plain crate version with
//! commit `unknown` — the build never fails for lack of git.
//!
//! Vibecrafted with AI Agents by VetCoders (c)2024-2026 LibraxisAI

use std::process::Command;

/// Run `git <args>` and return trimmed stdout, or `None` on any failure
/// (git missing, not a repo, non-zero exit, empty output).
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn main() {
    // Always re-run when the build script itself changes.
    println!("cargo:rerun-if-changed=build.rs");
    // Allow packaged / reproducible builds to pin the stamp explicitly.
    println!("cargo:rerun-if-env-changed=LOCTREE_MCP_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=LOCTREE_MCP_BUILD_VERSION");

    // Re-stamp when HEAD moves so the binary tracks the checkout it was built
    // from. Resolve the git dir (workspace member builds run from a subdir).
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/index");
    }
    if let Some(common_dir) = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]) {
        println!("cargo:rerun-if-changed={common_dir}/packed-refs");
        if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={common_dir}/{head_ref}");
        }
    }

    // Explicit override wins (lets a release/packaging pipeline inject a known
    // commit without a live git tree).
    let commit = std::env::var("LOCTREE_MCP_GIT_COMMIT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .or_else(|| git(&["rev-parse", "--short=8", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());

    // Uncommitted changes mean the binary does not correspond to any commit.
    let dirty = git(&["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    // Richer human-facing stamp: tag + distance + short sha (+ `-dirty`).
    let describe = git(&["describe", "--always", "--dirty", "--tags"]).unwrap_or_else(|| {
        if dirty && commit != "unknown" {
            format!("{commit}-dirty")
        } else {
            commit.clone()
        }
    });

    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());

    // Semver build metadata (`+...`) is valid and is exactly where an agent
    // already looks for a version: `0.13.0+g<sha>` or `0.13.0+g<sha>.dirty`.
    let build_version = if std::env::var("LOCTREE_MCP_BUILD_VERSION")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .is_some()
    {
        std::env::var("LOCTREE_MCP_BUILD_VERSION").unwrap()
    } else if commit == "unknown" {
        pkg_version.clone()
    } else if dirty {
        format!("{pkg_version}+g{commit}.dirty")
    } else {
        format!("{pkg_version}+g{commit}")
    };

    println!("cargo:rustc-env=LOCTREE_MCP_GIT_COMMIT={commit}");
    println!(
        "cargo:rustc-env=LOCTREE_MCP_GIT_DIRTY={}",
        if dirty { "1" } else { "0" }
    );
    println!("cargo:rustc-env=LOCTREE_MCP_GIT_DESCRIBE={describe}");
    println!("cargo:rustc-env=LOCTREE_MCP_BUILD_VERSION={build_version}");
}
