//! Acceptance and regression tests for W4-03:
//! MCP server single-instance lock and idle-timeout watchdog.
//!
//! 1. Single-instance guard: A second `loctree-mcp` for the same project root is
//!    refused with an explicit message detailing the existing PID.
//! 2. Stale-lock cleanup & SIGTERM: Killing the first process cleanly removes `mcp.pid`,
//!    allowing a subsequent process to acquire the lock.
//! 3. Idle-timeout watchdog: A process with no requests exits automatically after
//!    `LOCT_MCP_IDLE_MIN` minutes (verified with N=0.01 min = 0.6s).
//! 4. Activity reset: Incoming requests reset the idle timer so active sessions remain alive.
//! 5. Stdin EOF: Existing `--exit-on-stdin-eof` behavior continues without regression.
//! 6. Distinct project isolation: Multiple processes for distinct project roots run concurrently.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn sample_project(name: &str) -> TempDir {
    let tmp = TempDir::new().expect("create sample project tempdir");
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).expect("create src dir");
    fs::write(
        tmp.path().join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
    )
    .expect("write Cargo.toml");
    fs::write(
        src.join("lib.rs"),
        "pub fn sample_entry() -> &'static str { \"sample\" }\n",
    )
    .expect("write lib.rs");
    tmp
}

fn project_cache_dir(project_path: &Path) -> PathBuf {
    loctree::snapshot::project_cache_dir(project_path)
}

fn read_announced_addr(child: &mut Child) -> SocketAddr {
    const PREFIX: &str = "loctree-mcp http listening on ";
    const DEADLINE: Duration = Duration::from_secs(15);

    let stdout = child.stdout.take().expect("child stdout piped");
    let (tx, rx) = mpsc::channel::<Result<SocketAddr, String>>();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _ = tx.send(Err(
                        "server exited before announcing a listening address".into()
                    ));
                    return;
                }
                Ok(_) => {
                    if let Some(rest) = line.trim().strip_prefix(PREFIX) {
                        let _ = tx.send(rest.parse::<SocketAddr>().map_err(|e| {
                            format!("parse announced listening address {rest:?}: {e}")
                        }));
                        return;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("read server stdout: {e}")));
                    return;
                }
            }
        }
    });

    match rx.recv_timeout(DEADLINE) {
        Ok(Ok(addr)) => addr,
        Ok(Err(msg)) => panic!("{msg}"),
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("server did not announce a listening address within {DEADLINE:?}");
        }
    }
}

fn http_get_status(addr: SocketAddr, path: &str) -> u16 {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .expect("write request");

    let mut raw = String::new();
    stream
        .read_to_string(&mut raw)
        .unwrap_or_else(|e| panic!("read response from {addr}: {e}"));
    let first_line = raw.lines().next().unwrap_or_default();
    let status_str = first_line.split_whitespace().nth(1).unwrap_or("0");
    status_str.parse::<u16>().unwrap_or(0)
}

#[test]
fn w4_03_mcp_single_instance_lock() {
    let project = sample_project("w4_03_lock_proj");
    let project_path = project.path();
    let cache_dir = project_cache_dir(project_path);
    let pid_file = cache_dir.join("mcp.pid");

    assert!(
        !pid_file.exists(),
        "pidfile must not exist before server start"
    );

    // Start instance 1
    let mut child1 = Command::new(env!("CARGO_BIN_EXE_loctree-mcp"))
        .args([
            "--transport",
            "http",
            "--bind",
            "127.0.0.1:0",
            "--root",
            &project_path.display().to_string(),
            "--log-level",
            "info",
        ])
        .env("LOCT_ALLOW_NON_GIT_ROOT", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn first loctree-mcp");

    let child1_pid = child1.id();
    let _addr1 = read_announced_addr(&mut child1);

    // Verify pidfile exists and contains child1_pid
    assert!(
        pid_file.exists(),
        "pidfile must exist while instance 1 is running"
    );
    let pid_content = fs::read_to_string(&pid_file).expect("read pidfile");
    assert_eq!(
        pid_content.trim(),
        child1_pid.to_string(),
        "pidfile must contain PID of running instance"
    );

    // Start instance 2 on the exact same project root
    let child2_output = Command::new(env!("CARGO_BIN_EXE_loctree-mcp"))
        .args([
            "--transport",
            "http",
            "--bind",
            "127.0.0.1:0",
            "--root",
            &project_path.display().to_string(),
            "--log-level",
            "info",
        ])
        .env("LOCT_ALLOW_NON_GIT_ROOT", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn second loctree-mcp");

    // Child 2 must fail with a clear refusal and message mentioning the existing PID
    assert!(
        !child2_output.status.success(),
        "second instance must exit with non-zero failure code"
    );
    let stderr2 = String::from_utf8_lossy(&child2_output.stderr);
    assert!(
        stderr2.contains("already running"),
        "stderr must report 'already running': {stderr2}"
    );
    assert!(
        stderr2.contains(&child1_pid.to_string()),
        "stderr must name the existing process PID {child1_pid}: {stderr2}"
    );
    assert!(
        stderr2.contains("refusing to start a second instance"),
        "stderr must explicitly state refusal to start: {stderr2}"
    );

    // Verify instance 1 is still alive
    assert!(
        child1.try_wait().expect("check child1 status").is_none(),
        "instance 1 must remain running after instance 2 refusal"
    );

    // Terminate instance 1 gracefully via SIGTERM to verify pidfile cleanup
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(child1_pid as i32, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child1.kill();
    }

    let status1 = child1.wait().expect("wait for child 1");
    assert!(
        status1.success(),
        "instance 1 should exit cleanly on SIGTERM"
    );

    // Runtime proof requirement: "zabić pierwszego, pidfile sprzątnięty"
    assert!(
        !pid_file.exists(),
        "pidfile must be cleaned up when instance 1 terminates"
    );

    // Start instance 3 on the same project: must now succeed since lock is free
    let mut child3 = Command::new(env!("CARGO_BIN_EXE_loctree-mcp"))
        .args([
            "--transport",
            "http",
            "--bind",
            "127.0.0.1:0",
            "--root",
            &project_path.display().to_string(),
            "--log-level",
            "info",
        ])
        .env("LOCT_ALLOW_NON_GIT_ROOT", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn third loctree-mcp");

    let child3_pid = child3.id();
    let _addr3 = read_announced_addr(&mut child3);

    assert!(pid_file.exists(), "pidfile must exist for instance 3");
    let pid3_content = fs::read_to_string(&pid_file).expect("read pidfile 3");
    assert_eq!(
        pid3_content.trim(),
        child3_pid.to_string(),
        "pidfile must contain PID of instance 3"
    );

    // Cleanup instance 3
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(child3_pid as i32, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child3.kill();
    }
    let _ = child3.wait();
    // Cross-process cleanup after SIGTERM is not atomic with wait() reaping:
    // the child's handler runs its own teardown, and on Linux CI the pidfile
    // removal lands after wait() returns. Poll with a deadline instead of
    // racing (yield_now, not sleep — see the no-sleep-in-tests contract).
    // 5s proved marginal on loaded CI runners (the yield loop competes with
    // the child's teardown for CPU); 30s bounds the wait without slowing the
    // common case, which exits the loop in milliseconds.
    let cleanup_deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while pid_file.exists() && std::time::Instant::now() < cleanup_deadline {
        std::thread::yield_now();
    }
    assert!(
        !pid_file.exists(),
        "pidfile must be cleaned up after instance 3 exit"
    );
}

#[test]
fn w4_03_mcp_idle_timeout_exits() {
    let project = sample_project("w4_03_idle_proj");
    let project_path = project.path();
    let cache_dir = project_cache_dir(project_path);
    let pid_file = cache_dir.join("mcp.pid");

    let start = Instant::now();

    // 0.01 minutes = 0.6 seconds
    let mut child = Command::new(env!("CARGO_BIN_EXE_loctree-mcp"))
        .args([
            "--transport",
            "http",
            "--bind",
            "127.0.0.1:0",
            "--root",
            &project_path.display().to_string(),
            "--log-level",
            "info",
        ])
        .env("LOCT_MCP_IDLE_MIN", "0.01")
        .env("LOCT_ALLOW_NON_GIT_ROOT", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn loctree-mcp with idle timeout");

    let _addr = read_announced_addr(&mut child);
    assert!(pid_file.exists(), "pidfile must exist while running");

    // Wait for the process to exit on its own due to idle timeout
    let status = child.wait().expect("wait for child idle exit");
    let elapsed = start.elapsed();

    assert!(
        status.success(),
        "idle timeout should result in a clean success exit"
    );
    assert!(
        elapsed >= Duration::from_millis(500),
        "idle timeout took {elapsed:?}, expected at least 0.5s"
    );
    assert!(
        elapsed <= Duration::from_secs(5),
        "idle timeout took {elapsed:?}, expected <= 5s"
    );
    assert!(
        !pid_file.exists(),
        "pidfile must be cleaned up after idle timeout exit"
    );
}

#[test]
fn w4_03_mcp_idle_timeout_reset_by_requests() {
    let project = sample_project("w4_03_idle_reset_proj");
    let project_path = project.path();

    // 0.03 minutes = 1.8 seconds idle timeout
    let mut child = Command::new(env!("CARGO_BIN_EXE_loctree-mcp"))
        .args([
            "--transport",
            "http",
            "--bind",
            "127.0.0.1:0",
            "--root",
            &project_path.display().to_string(),
            "--log-level",
            "info",
        ])
        .env("LOCT_MCP_IDLE_MIN", "0.03")
        .env("LOCT_ALLOW_NON_GIT_ROOT", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn loctree-mcp with idle timeout");

    let addr = read_announced_addr(&mut child);

    // Keep sending requests every 600ms for 3 iterations (total 1.8s+ of activity)
    for i in 1..=3 {
        thread::sleep(Duration::from_millis(600));
        let path = format!("/context_pack?project={}", project_path.display());
        let status = http_get_status(addr, &path);
        assert_eq!(
            status, 200,
            "request {i} should succeed while server is active"
        );
    }

    // Now stop sending requests; server should idle-exit in ~1.8 seconds
    let idle_start = Instant::now();
    let status = child.wait().expect("wait for idle exit");
    let idle_elapsed = idle_start.elapsed();

    assert!(status.success(), "idle exit must be clean exit code 0");
    assert!(
        idle_elapsed >= Duration::from_millis(1500),
        "idle exit took {idle_elapsed:?}, expected >= 1.5s after requests stopped"
    );
    assert!(
        idle_elapsed <= Duration::from_secs(6),
        "idle exit took {idle_elapsed:?}, expected <= 6s"
    );
}

#[test]
fn w4_03_mcp_exit_on_stdin_eof_no_regression() {
    let project = sample_project("w4_03_stdin_eof_proj");
    let project_path = project.path();
    let cache_dir = project_cache_dir(project_path);
    let pid_file = cache_dir.join("mcp.pid");

    let mut child = Command::new(env!("CARGO_BIN_EXE_loctree-mcp"))
        .args([
            "--transport",
            "http",
            "--bind",
            "127.0.0.1:0",
            "--root",
            &project_path.display().to_string(),
            "--exit-on-stdin-eof",
            "--log-level",
            "info",
        ])
        .env("LOCT_ALLOW_NON_GIT_ROOT", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn loctree-mcp with exit-on-stdin-eof");

    let _addr = read_announced_addr(&mut child);
    assert!(pid_file.exists(), "pidfile must exist");

    // Close stdin pipe
    drop(child.stdin.take());

    // Server must exit cleanly
    let status = child.wait().expect("wait for stdin EOF exit");
    assert!(status.success(), "stdin EOF exit must be clean");
    assert!(
        !pid_file.exists(),
        "pidfile must be cleaned up on stdin EOF exit"
    );
}

#[test]
fn w4_03_mcp_distinct_projects_run_concurrently() {
    let proj_a = sample_project("w4_03_proj_a");
    let proj_b = sample_project("w4_03_proj_b");

    let mut child_a = Command::new(env!("CARGO_BIN_EXE_loctree-mcp"))
        .args([
            "--transport",
            "http",
            "--bind",
            "127.0.0.1:0",
            "--root",
            &proj_a.path().display().to_string(),
            "--log-level",
            "info",
        ])
        .env("LOCT_ALLOW_NON_GIT_ROOT", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn child A");

    let addr_a = read_announced_addr(&mut child_a);

    let mut child_b = Command::new(env!("CARGO_BIN_EXE_loctree-mcp"))
        .args([
            "--transport",
            "http",
            "--bind",
            "127.0.0.1:0",
            "--root",
            &proj_b.path().display().to_string(),
            "--log-level",
            "info",
        ])
        .env("LOCT_ALLOW_NON_GIT_ROOT", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn child B");

    let addr_b = read_announced_addr(&mut child_b);

    assert_ne!(addr_a, addr_b);
    assert!(child_a.try_wait().expect("status A").is_none());
    assert!(child_b.try_wait().expect("status B").is_none());

    let pid_a = project_cache_dir(proj_a.path()).join("mcp.pid");
    let pid_b = project_cache_dir(proj_b.path()).join("mcp.pid");
    assert!(pid_a.exists());
    assert!(pid_b.exists());

    // Terminate both
    let _ = child_a.kill();
    let _ = child_b.kill();
    let _ = child_a.wait();
    let _ = child_b.wait();
}
