//! Watch-related command handlers
//!
//! Handles: scan_watch, watch (loct watch [--dev|--bg|--lsp|--http|--report]),
//! coverage.

use std::path::{Path, PathBuf};

use super::super::super::command::{Command, CoverageOptions, ScanOptions, WatchOptions};
use super::super::{
    DispatchResult, GlobalOptions, command_to_parsed_args, load_or_create_snapshot,
};
use crate::progress::Spinner;
use crate::watch_lock::{EXIT_LOCK_CONTENDED, LockError, LockMode, WatchLock, acquire};

/// Run the watch loop after acquiring the single-instance lock for the repo.
///
/// Shared codepath for both `loct scan --watch` (legacy) and
/// `loct watch [--dev|--bg|--lsp]` (the new shape). Returns the appropriate
/// `DispatchResult` exit code:
///
/// * `0` — watcher exited cleanly.
/// * `1` — watcher failed.
/// * `75` (`EXIT_LOCK_CONTENDED`) — another `--watch` already runs against
///   this repo.
fn run_watch_with_lock(
    roots: Vec<PathBuf>,
    parsed_args: &crate::args::ParsedArgs,
    extensions: Option<Vec<String>>,
    gitignore: Option<crate::fs_utils::GitIgnoreChecker>,
    lock_mode: LockMode,
    on_snapshot_updated: Option<Box<dyn FnMut() + Send>>,
) -> DispatchResult {
    use crate::watch::{WatchConfig, watch_and_rescan};
    use std::time::Duration;

    let snapshot_root = crate::snapshot::resolve_snapshot_root(&roots);

    let _lock: WatchLock = match acquire(&snapshot_root, lock_mode) {
        Ok(guard) => guard,
        Err(LockError::HeldBy(info)) => {
            eprintln!(
                "[watch] another `loct --watch` already runs for this repo: {}",
                info
            );
            eprintln!(
                "[watch] hint: pass `--replace` to recycle it, or `--wait[=SECONDS]` to block."
            );
            return DispatchResult::Exit(EXIT_LOCK_CONTENDED);
        }
        Err(LockError::WaitTimeout(info)) => {
            eprintln!(
                "[watch] timed out waiting for lock; holder still alive: {}",
                info
            );
            return DispatchResult::Exit(EXIT_LOCK_CONTENDED);
        }
        Err(LockError::Io(e)) => {
            eprintln!("[watch] failed to acquire lock: {}", e);
            return DispatchResult::Exit(1);
        }
    };

    let config = WatchConfig {
        roots,
        debounce_duration: Duration::from_millis(500),
        extensions,
        gitignore,
        on_snapshot_updated,
    };

    match watch_and_rescan(config, parsed_args) {
        Ok(_) => DispatchResult::Exit(0),
        Err(e) => {
            eprintln!("[watch] Error: {}", e);
            DispatchResult::Exit(1)
        }
    }
    // `_lock` is dropped here, releasing the kernel-side flock.
}

/// Handle the scan command with watch mode
pub fn handle_scan_watch_command(opts: &ScanOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::detect::apply_detected_stack;
    use crate::fs_utils::GitIgnoreChecker;

    // Build ParsedArgs for scanning
    let mut parsed_args = command_to_parsed_args(&Command::Scan(opts.clone()), global);

    // Auto-detect stack if first root exists
    let roots = if opts.roots.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        opts.roots.clone()
    };

    if let Some(root) = roots.first() {
        let mut library_mode = parsed_args.library_mode;
        apply_detected_stack(
            root,
            &mut parsed_args.extensions,
            &mut parsed_args.ignore_patterns,
            &mut parsed_args.tauri_preset,
            &mut library_mode,
            &mut parsed_args.py_roots,
            parsed_args.verbose,
        );
        parsed_args.library_mode = library_mode;
    }

    // Build gitignore checker
    let gitignore = if parsed_args.use_gitignore
        && let Some(root) = roots.first()
    {
        GitIgnoreChecker::new(root)
    } else {
        None
    };

    // Convert extensions from HashSet to Vec
    let extensions = parsed_args
        .extensions
        .as_ref()
        .map(|set| set.iter().cloned().collect::<Vec<String>>());

    let lock_mode = if opts.replace {
        LockMode::Replace
    } else if let Some(secs) = opts.wait_seconds {
        LockMode::Wait(Some(std::time::Duration::from_secs(secs)))
    } else if opts.wait_indefinite {
        LockMode::Wait(None)
    } else {
        LockMode::Default
    };

    run_watch_with_lock(roots, &parsed_args, extensions, gitignore, lock_mode, None)
}

/// Handle the dedicated `loct watch` subcommand (the new shape).
///
/// Modes:
///   * `--dev` (default) — foreground watcher with single-instance lock.
///   * `--bg` — daemonize: re-spawn self detached, exit parent.
///   * `--lsp` — foreground watcher + co-spawned `loctree-lsp`.
///   * `--http` — foreground watcher + co-spawned `loctree-mcp` over
///     streamable-http on `--port` (default 5174).
///   * `--report` — foreground watcher + in-process HTTP server that
///     serves `.loctree/report.html` on `--port` (default 5075) and
///     re-renders on every snapshot save.
pub fn handle_watch_command(opts: &WatchOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::cli::command::WatchMode;
    use crate::detect::apply_detected_stack;
    use crate::fs_utils::GitIgnoreChecker;

    // Translate watch options into a ParsedArgs derived from the scan path.
    let scan_opts = ScanOptions {
        roots: opts.roots.clone(),
        full_scan: opts.full_scan,
        scan_all: opts.scan_all,
        watch: true,
        replace: opts.replace,
        wait_seconds: opts.wait_seconds,
        wait_indefinite: opts.wait_indefinite,
    };

    let mut parsed_args = command_to_parsed_args(&Command::Scan(scan_opts.clone()), global);

    let roots = if opts.roots.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        opts.roots.clone()
    };

    if let Some(root) = roots.first() {
        let mut library_mode = parsed_args.library_mode;
        apply_detected_stack(
            root,
            &mut parsed_args.extensions,
            &mut parsed_args.ignore_patterns,
            &mut parsed_args.tauri_preset,
            &mut library_mode,
            &mut parsed_args.py_roots,
            parsed_args.verbose,
        );
        parsed_args.library_mode = library_mode;
    }

    let gitignore = if parsed_args.use_gitignore
        && let Some(root) = roots.first()
    {
        GitIgnoreChecker::new(root)
    } else {
        None
    };

    let extensions = parsed_args
        .extensions
        .as_ref()
        .map(|set| set.iter().cloned().collect::<Vec<String>>());

    if matches!(opts.mode, WatchMode::Bg) {
        return spawn_background_watcher(&roots, opts);
    }

    let lock_mode = if opts.replace {
        LockMode::Replace
    } else if let Some(secs) = opts.wait_seconds {
        LockMode::Wait(Some(std::time::Duration::from_secs(secs)))
    } else if opts.wait_indefinite {
        LockMode::Wait(None)
    } else {
        LockMode::Default
    };

    // For `--lsp`, co-spawn loctree-lsp alongside the watch loop. The child
    // is killed automatically when the parent exits (via `Drop` on `Child`).
    let _lsp_child: Option<std::process::Child> = if matches!(opts.mode, WatchMode::Lsp) {
        spawn_lsp_companion().map(Some).unwrap_or_else(|e| {
            eprintln!("[watch] could not spawn loctree-lsp companion: {e}");
            eprintln!("[watch] continuing without LSP co-process.");
            None
        })
    } else {
        None
    };

    // For `--http`, co-spawn `loctree-mcp` over streamable-http. Behaves
    // exactly like the `--lsp` companion: child inherits stdio, is killed
    // automatically on parent exit.
    let _mcp_child: Option<std::process::Child> = if matches!(opts.mode, WatchMode::Http) {
        let port = opts.port.unwrap_or(5174);
        let watched_root = crate::snapshot::resolve_snapshot_root(&roots);
        match spawn_http_mcp_companion(port, &watched_root) {
            Ok(child) => Some(child),
            Err(e) => {
                eprintln!("[watch] could not spawn loctree-mcp http companion: {e}");
                eprintln!("[watch] continuing without --http co-process.");
                None
            }
        }
    } else {
        None
    };

    // For `--report`, start the local report server in-process and queue a
    // re-render after every snapshot save. The server thread dies when the
    // process exits; the lock + watch loop stay in this function.
    let (on_snapshot_updated, _report_server) = if matches!(opts.mode, WatchMode::Report) {
        let port = opts.port.unwrap_or(5075);
        match bring_up_report_server(&roots, port) {
            Ok((hook, server)) => (Some(hook), Some(server)),
            Err(e) => {
                eprintln!("[watch] could not bring up --report server: {e}");
                eprintln!("[watch] continuing without report serve.");
                (None, None)
            }
        }
    } else {
        (None, None)
    };

    run_watch_with_lock(
        roots,
        &parsed_args,
        extensions,
        gitignore,
        lock_mode,
        on_snapshot_updated,
    )
}

/// Spawn the watcher as a detached background child and return immediately.
///
/// Strategy: re-exec the current binary with the original argv but `--bg`
/// replaced by `--dev`, redirect child stdout/stderr into the repo's
/// `.loctree/watch.log`, then exit the parent. The child acquires the lock
/// inside its own process so SIGKILL self-healing still applies.
fn spawn_background_watcher(roots: &[PathBuf], opts: &WatchOptions) -> DispatchResult {
    use std::fs::OpenOptions;
    use std::process::Command;

    let snapshot_root = crate::snapshot::resolve_snapshot_root(roots);
    if let Err(e) = std::fs::create_dir_all(snapshot_root.join(".loctree")) {
        eprintln!("[watch] failed to create .loctree dir for bg log: {e}");
        return DispatchResult::Exit(1);
    }
    let log_path = snapshot_root.join(".loctree/watch.log");
    let log_file = match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[watch] failed to open log {}: {e}", log_path.display());
            return DispatchResult::Exit(1);
        }
    };
    let log_file_err = match log_file.try_clone() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[watch] failed to clone log fd: {e}");
            return DispatchResult::Exit(1);
        }
    };

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[watch] cannot resolve current executable: {e}");
            return DispatchResult::Exit(1);
        }
    };

    // Build child argv: `loct watch --dev <roots> [--replace|--wait...]`.
    let mut cmd = Command::new(exe);
    cmd.arg("watch").arg("--dev");
    if opts.replace {
        cmd.arg("--replace");
    }
    if let Some(secs) = opts.wait_seconds {
        cmd.arg(format!("--wait={}", secs));
    } else if opts.wait_indefinite {
        cmd.arg("--wait");
    }
    if opts.full_scan {
        cmd.arg("--full-scan");
    }
    if opts.scan_all {
        cmd.arg("--scan-all");
    }
    for r in roots {
        cmd.arg(r);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(log_file)
        .stderr(log_file_err);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Detach from parent process group so the child survives the
        // parent's exit and isn't killed by Ctrl+C in the launching shell.
        unsafe {
            cmd.pre_exec(|| {
                let r = libc::setsid();
                if r == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    match cmd.spawn() {
        Ok(child) => {
            println!(
                "[watch] started in background (pid {}). log: {}",
                child.id(),
                log_path.display()
            );
            DispatchResult::Exit(0)
        }
        Err(e) => {
            eprintln!("[watch] failed to spawn background watcher: {e}");
            DispatchResult::Exit(1)
        }
    }
}

/// Best-effort spawn of `loctree-mcp --transport http --bind 127.0.0.1:<port>`
/// as a sibling child of the watch loop.
///
/// The companion is pinned to the watched repo via `--root` (`watched_root`) —
/// the same canonical root the watch lock keys on — so there is exactly one
/// `loct watch` + one streamable-http MCP companion per repo root (see the
/// watch lock). `loctree-mcp` stays universal: `--root` only sets the default
/// project, and any MCP request carrying its own `project` overrides it. The
/// `loctree-lsp` companion instead derives its workspace from the LSP
/// `initialize` handshake and takes no root.
///
/// Resolves the binary in this order:
///   1. `LOCTREE_MCP_BIN` env override.
///   2. Sibling of the current executable (e.g. `target/debug/loctree-mcp`).
///   3. `loctree-mcp` on `$PATH`.
fn spawn_http_mcp_companion(
    port: u16,
    watched_root: &Path,
) -> Result<std::process::Child, std::io::Error> {
    use std::process::{Command, Stdio};

    let bin = if let Ok(p) = std::env::var("LOCTREE_MCP_BIN") {
        PathBuf::from(p)
    } else if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
        && parent.join("loctree-mcp").exists()
    {
        parent.join("loctree-mcp")
    } else {
        PathBuf::from("loctree-mcp")
    };

    let bind = format!("127.0.0.1:{port}");
    let mut cmd = Command::new(&bin);
    cmd.arg("--transport").arg("http").arg("--bind").arg(&bind);
    // Pin the companion's default project to the watched repo root. The binary
    // stays universal — a request carrying its own `project` overrides this —
    // but a bare `/context_pack` / MCP call resolves against the repo the user
    // is actually watching.
    cmd.arg("--root").arg(watched_root);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    eprintln!(
        "[watch] starting loctree-mcp http companion on http://{}/ ({})",
        bind,
        bin.display()
    );
    cmd.spawn()
}

/// `on_snapshot_updated` hook the watch loop calls after each successful
/// snapshot save; closes over the most-recent report-render child handle.
type ReportRenderHook = Box<dyn FnMut() + Send>;

/// Pair returned by [`bring_up_report_server`]: the snapshot hook + the
/// TCP listener thread handle.
type ReportServerHandle = (ReportRenderHook, std::thread::JoinHandle<()>);

/// Bring up the `--report` HTTP server in-process and return:
///   1. an `on_snapshot_updated` hook that re-renders `.loctree/report.html`
///      via a child `loct report --output <path>` process. The hook stores
///      the most recent child so a slow render naturally throttles spammy
///      file-system events.
///   2. a `JoinHandle` owning the TCP listener thread. The thread stays
///      alive as long as the watch loop runs and dies on process exit.
fn bring_up_report_server(
    roots: &[PathBuf],
    port: u16,
) -> Result<ReportServerHandle, std::io::Error> {
    use crate::analyzer::open_server::{EditorConfig, start_open_server};
    use std::sync::{Arc, Mutex};

    let snapshot_root = crate::snapshot::resolve_snapshot_root(roots);
    let report_dir = snapshot_root.join(".loctree");
    std::fs::create_dir_all(&report_dir)?;
    let report_path = report_dir.join("report.html");

    // Render an initial report so the server has something to serve on
    // first request. Best-effort: failures fall back to "404 not found" on
    // the open_server side until the next watch tick succeeds.
    if let Err(e) = render_report_once(roots, &report_path) {
        eprintln!("[watch] initial report render failed: {e}; server will start anyway");
    }

    let (base, handle) = start_open_server(
        roots.to_vec(),
        EditorConfig::from_args(None, None),
        Some(report_path.clone()),
        Some(port),
    )
    .ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("could not bind report server on :{port}"),
        )
    })?;

    eprintln!("[watch] report server live at {}", base);

    // Re-render hook: own the most recent child handle so we naturally
    // skip spawning a new render while the previous one is still running.
    let last_child: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));
    let roots_owned: Vec<PathBuf> = roots.to_vec();
    let report_path_owned = report_path.clone();
    let hook_child = Arc::clone(&last_child);
    let hook = Box::new(move || {
        let mut slot = match hook_child.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(child) = slot.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {
                    // Previous render finished; ok to spawn a new one.
                }
                Ok(None) => {
                    // Still rendering — drop this tick, the in-flight
                    // child will pick up the latest snapshot anyway.
                    return;
                }
                Err(_) => {
                    // Lost track of child; clear and continue.
                }
            }
        }
        match spawn_report_render(&roots_owned, &report_path_owned) {
            Ok(child) => *slot = Some(child),
            Err(e) => {
                eprintln!("[watch] could not spawn report re-render: {e}");
                *slot = None;
            }
        }
    });

    Ok((hook, handle))
}

/// One-shot `loct report --output <path>` invocation (blocking).
fn render_report_once(roots: &[PathBuf], report_path: &std::path::Path) -> std::io::Result<()> {
    let mut child = spawn_report_render(roots, report_path)?;
    let status = child.wait()?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "loct report --output exited with {status}"
        )));
    }
    Ok(())
}

/// Spawn `loct report --output <path>` as a detached child.
///
/// Resolves the `loct` binary as the current executable so the child uses
/// the same build the operator is running. Sets `LOCT_OPEN_BROWSER=0` so the
/// child does not spawn an `open`/`xdg-open` window per render — the user
/// already has a browser tab pointed at the watch server.
fn spawn_report_render(
    roots: &[PathBuf],
    report_path: &std::path::Path,
) -> std::io::Result<std::process::Child> {
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("report").arg("--output").arg(report_path);
    for r in roots {
        cmd.arg(r);
    }
    cmd.env("LOCT_OPEN_BROWSER", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn()
}

/// Best-effort spawn of `loctree-lsp` as a sibling child process.
///
/// `loctree-lsp` derives its workspace from the LSP `initialize` handshake,
/// not from a launch-time flag — it accepts only `--debug`/`--capabilities`
/// and rejects `--root`. So no root is passed here.
///
/// Resolves the binary in this order:
///   1. `LOCTREE_LSP_BIN` env override.
///   2. Sibling of the current executable (e.g. `target/debug/loctree-lsp`).
///   3. `loctree-lsp` on `$PATH`.
fn spawn_lsp_companion() -> Result<std::process::Child, std::io::Error> {
    use std::process::{Command, Stdio};

    let bin = if let Ok(p) = std::env::var("LOCTREE_LSP_BIN") {
        PathBuf::from(p)
    } else if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
        && parent.join("loctree-lsp").exists()
    {
        parent.join("loctree-lsp")
    } else {
        PathBuf::from("loctree-lsp")
    };

    let mut cmd = Command::new(&bin);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    eprintln!("[watch] starting loctree-lsp companion: {}", bin.display());
    cmd.spawn()
}

/// Handle the coverage command - analyze test coverage gaps
pub fn handle_coverage_command(opts: &CoverageOptions, global: &GlobalOptions) -> DispatchResult {
    use crate::analyzer::coverage_gaps::{GapKind, Severity, find_coverage_gaps_fenced};
    use crate::analyzer::test_coverage::{CoverageStatus, analyze_test_coverage};
    use std::path::Path;

    let include_gaps = opts.gaps
        || !opts.tests
        || opts.handlers_only
        || opts.events_only
        || opts.min_severity.is_some();
    let include_tests = opts.tests;

    // Show spinner unless in quiet/json mode
    let spinner = if !global.quiet && !global.json {
        let label = if include_gaps && include_tests {
            "Analyzing coverage (gaps + tests)..."
        } else if include_tests {
            "Analyzing structural test coverage..."
        } else {
            "Analyzing test coverage gaps..."
        };
        Some(Spinner::new(label))
    } else {
        None
    };

    // Load snapshot (auto-scan if missing)
    let root = opts
        .roots
        .first()
        .map(|p| p.as_path())
        .unwrap_or(Path::new("."));

    let snapshot = match load_or_create_snapshot(root, global) {
        Ok(s) => s,
        Err(e) => {
            if let Some(s) = spinner {
                s.finish_error(&format!("Failed to load snapshot: {}", e));
            } else {
                eprintln!("[loct][error] {}", e);
            }
            return DispatchResult::Exit(1);
        }
    };

    // Find coverage gaps (artifact fence default-on; --include-artifacts opts out)
    let mut fence = crate::analyzer::classify::ArtifactFenceStats::default();
    let gaps = if include_gaps {
        let (mut gaps, fence_stats) = find_coverage_gaps_fenced(&snapshot, opts.include_artifacts);
        fence = fence_stats;

        // Apply filters
        if opts.handlers_only {
            gaps.retain(|g| matches!(g.kind, GapKind::HandlerWithoutTest));
        }
        if opts.events_only {
            gaps.retain(|g| matches!(g.kind, GapKind::EventWithoutTest));
        }
        if let Some(ref min_sev) = opts.min_severity {
            let min_level = match min_sev.to_lowercase().as_str() {
                "critical" => 0,
                "high" => 1,
                "medium" => 2,
                "low" => 3,
                _ => 4, // show all
            };
            gaps.retain(|g| {
                let level = match g.severity {
                    Severity::Critical => 0,
                    Severity::High => 1,
                    Severity::Medium => 2,
                    Severity::Low => 3,
                };
                level <= min_level
            });
        }
        gaps
    } else {
        Vec::new()
    };

    let test_report = if include_tests {
        Some(analyze_test_coverage(&snapshot))
    } else {
        None
    };

    if let Some(s) = spinner {
        if include_gaps && include_tests {
            s.finish_success(&format!(
                "Found {} gap(s), coverage {:.1}%",
                gaps.len(),
                test_report
                    .as_ref()
                    .map(|r| r.coverage_percent)
                    .unwrap_or(0.0)
            ));
        } else if include_tests {
            s.finish_success(&format!(
                "Coverage {:.1}% ({} test file(s))",
                test_report
                    .as_ref()
                    .map(|r| r.coverage_percent)
                    .unwrap_or(0.0),
                test_report.as_ref().map(|r| r.test_file_count).unwrap_or(0)
            ));
        } else {
            s.finish_success(&format!("Found {} coverage gap(s)", gaps.len()));
        }
    }

    // Zero silent cuts: report what the artifact fence removed.
    // JSON keeps its stdout shape; the fence line goes to stderr there.
    if !fence.is_empty() && global.json {
        eprintln!("[loct] {}", fence.summary_line());
    }

    // Output results
    if global.json {
        if include_gaps && include_tests {
            let combined = serde_json::json!({
                "gaps": gaps,
                "tests": test_report,
                "excluded_artifacts": fence,
            });
            match serde_json::to_string_pretty(&combined) {
                Ok(json) => println!("{}", json),
                Err(e) => {
                    eprintln!("[loct][error] Failed to serialize coverage output: {}", e);
                    return DispatchResult::Exit(1);
                }
            }
        } else if include_tests {
            match serde_json::to_string_pretty(&test_report) {
                Ok(json) => println!("{}", json),
                Err(e) => {
                    eprintln!("[loct][error] Failed to serialize test coverage: {}", e);
                    return DispatchResult::Exit(1);
                }
            }
        } else {
            match serde_json::to_string_pretty(&gaps) {
                Ok(json) => println!("{}", json),
                Err(e) => {
                    eprintln!("[loct][error] Failed to serialize coverage gaps: {}", e);
                    return DispatchResult::Exit(1);
                }
            }
        }
    } else {
        if include_tests {
            if let Some(report) = &test_report {
                let missing_tests: Vec<_> = report
                    .handlers
                    .iter()
                    .filter(|h| h.coverage_status == CoverageStatus::MissingTests)
                    .collect();
                let test_only: Vec<_> = report
                    .handlers
                    .iter()
                    .filter(|h| h.coverage_status == CoverageStatus::TestOnly)
                    .collect();
                let uncovered: Vec<_> = report
                    .handlers
                    .iter()
                    .filter(|h| h.coverage_status == CoverageStatus::Uncovered)
                    .collect();

                println!("Structural Test Coverage:");
                println!("  Test files:   {}", report.test_file_count);
                println!("  Prod files:   {}", report.prod_file_count);
                println!("  Coverage:     {:.1}%", report.coverage_percent);
                println!(
                    "  Handlers:     {} missing tests, {} test-only, {} uncovered",
                    missing_tests.len(),
                    test_only.len(),
                    uncovered.len()
                );
                println!(
                    "  Exports w/o tests: {}",
                    report.exports_without_tests.len()
                );

                if !missing_tests.is_empty() {
                    println!("\nHandlers missing tests ({}):", missing_tests.len());
                    for handler in missing_tests.iter().take(10) {
                        println!(
                            "  [!] {} ({}:{})",
                            handler.name,
                            handler.backend_file.display(),
                            handler.line
                        );
                    }
                    if missing_tests.len() > 10 {
                        println!("  ... and {} more", missing_tests.len() - 10);
                    }
                }
                if !report.exports_without_tests.is_empty() {
                    println!(
                        "\nExports without tests ({}):",
                        report.exports_without_tests.len()
                    );
                    for export in report.exports_without_tests.iter().take(10) {
                        println!(
                            "  [?] {} ({}:{})",
                            export.symbol,
                            export.defined_in.display(),
                            export.line
                        );
                    }
                    if report.exports_without_tests.len() > 10 {
                        println!("  ... and {} more", report.exports_without_tests.len() - 10);
                    }
                }
            }
            if include_gaps {
                println!();
            }
        }

        if include_gaps {
            if gaps.is_empty() {
                println!("[OK] No coverage gaps found - all production code is tested!");
                if !fence.is_empty() {
                    println!(
                        "{} (use --include-artifacts to inspect)",
                        fence.summary_line()
                    );
                }
                return DispatchResult::Exit(0);
            }
            println!("Test Coverage Gaps ({} found):\n", gaps.len());

            // Group by severity
            let critical: Vec<_> = gaps
                .iter()
                .filter(|g| matches!(g.severity, Severity::Critical))
                .collect();
            let high: Vec<_> = gaps
                .iter()
                .filter(|g| matches!(g.severity, Severity::High))
                .collect();
            let medium: Vec<_> = gaps
                .iter()
                .filter(|g| matches!(g.severity, Severity::Medium))
                .collect();
            let low: Vec<_> = gaps
                .iter()
                .filter(|g| matches!(g.severity, Severity::Low))
                .collect();

            if !critical.is_empty() {
                println!("CRITICAL - Handlers without tests ({}):", critical.len());
                for gap in critical.iter().take(10) {
                    println!("  [!!] {} ({})", gap.target, gap.location);
                    println!("       {}", gap.recommendation);
                }
                if critical.len() > 10 {
                    println!("  ... and {} more", critical.len() - 10);
                }
                println!();
            }

            if !high.is_empty() {
                println!("HIGH - Events without tests ({}):", high.len());
                for gap in high.iter().take(10) {
                    println!("  [!] {} ({})", gap.target, gap.location);
                    println!("      {}", gap.recommendation);
                }
                if high.len() > 10 {
                    println!("  ... and {} more", high.len() - 10);
                }
                println!();
            }

            if !medium.is_empty() {
                println!("MEDIUM - Exports without tests ({}):", medium.len());
                for gap in medium.iter().take(5) {
                    println!("  [?] {} ({})", gap.target, gap.location);
                }
                if medium.len() > 5 {
                    println!("  ... and {} more", medium.len() - 5);
                }
                println!();
            }

            if !low.is_empty() {
                println!("LOW - Tested but unused ({}):", low.len());
                for gap in low.iter().take(5) {
                    println!("  [-] {} ({})", gap.target, gap.location);
                }
                if low.len() > 5 {
                    println!("  ... and {} more", low.len() - 5);
                }
                println!();
            }

            // Summary
            let handler_count = gaps
                .iter()
                .filter(|g| matches!(g.kind, GapKind::HandlerWithoutTest))
                .count();
            let event_count = gaps
                .iter()
                .filter(|g| matches!(g.kind, GapKind::EventWithoutTest))
                .count();
            println!(
                "Summary: {} handlers, {} events without test coverage",
                handler_count, event_count
            );
            if !fence.is_empty() {
                println!(
                    "{} (use --include-artifacts to inspect)",
                    fence.summary_line()
                );
            }
            println!("\nRun `loct coverage --json` for machine-readable output.");
        }
    }

    DispatchResult::Exit(0)
}
