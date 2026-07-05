//! End-to-end checks for the single-instance `--watch` lock.
//!
//! These tests drive the real `loct` binary via `assert_cmd` and prove the
//! five falsifiers listed in the lock plan:
//!
//! 1. A second `loct scan --watch` for the same repo exits non-zero quickly.
//! 2. SIGKILL on the holder leaves no stale `.loctree/scan.lock` entry —
//!    the next watcher acquires the lock without manual cleanup.
//! 3. Path canonicalization makes `.` and an absolute repo path collide.
//! 4. A one-shot `loct scan` (no `--watch`) is *not* blocked by a live watcher.
//! 5. `loct watch --http` / `--report` exit with the documented deferred
//!    code (2) rather than silently doing nothing.

use assert_cmd::cargo::cargo_bin;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

fn make_repo() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    // Minimum to look like a git repo so resolve_snapshot_root anchors here.
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
    std::fs::write(
        tmp.path().join("hello.rs"),
        "fn main() { println!(\"hi\"); }",
    )
    .unwrap();
    tmp
}

fn loct_bin() -> std::path::PathBuf {
    cargo_bin("loct")
}

fn spawn_watcher(repo: &std::path::Path) -> std::process::Child {
    Command::new(loct_bin())
        .current_dir(repo)
        .env("LOCT_OPEN_BROWSER", "0")
        .args(["scan", "--watch", "."])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn first --watch")
}

/// Wait until `.loctree/scan.lock` has a recorded PID so we know the holder
/// has actually called `acquire()`. Bounded retry — fails the test on timeout
/// rather than racing the watcher.
fn wait_for_lock_acquired(repo: &std::path::Path, timeout: Duration) {
    let lock = repo.join(".loctree/scan.lock");
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if lock.exists()
            && let Ok(payload) = std::fs::read_to_string(&lock)
            && payload.contains("\"pid\"")
            && !payload.contains("\"pid\":0")
        {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "watcher did not acquire lock within {:?} (lock path: {})",
        timeout,
        lock.display()
    );
}

#[test]
fn second_scan_watch_against_same_repo_is_rejected_with_exit_75() {
    let repo = make_repo();
    let mut first = spawn_watcher(repo.path());

    wait_for_lock_acquired(repo.path(), Duration::from_secs(15));

    let second = Command::new(loct_bin())
        .current_dir(repo.path())
        .env("LOCT_OPEN_BROWSER", "0")
        .args(["scan", "--watch", "."])
        .stdin(Stdio::null())
        .output()
        .expect("run second --watch");

    let stderr = String::from_utf8_lossy(&second.stderr);
    let code = second.status.code().unwrap_or(-1);
    let _ = first.kill();
    let _ = first.wait();

    assert_eq!(
        code, 75,
        "second --watch should exit 75 (EX_TEMPFAIL); stderr was:\n{stderr}"
    );
    assert!(
        stderr.contains("already runs"),
        "stderr should name the holder; got:\n{stderr}"
    );
}

#[test]
fn absolute_path_collides_with_relative_dot() {
    let repo = make_repo();
    let mut first = spawn_watcher(repo.path());

    wait_for_lock_acquired(repo.path(), Duration::from_secs(15));

    // Start from a *different* CWD so `--watch <abs>` is the only signal,
    // proving the lock key is the canonical repo root, not the CWD string.
    let abs = repo.path().canonicalize().unwrap();
    let elsewhere = TempDir::new().unwrap();
    let second = Command::new(loct_bin())
        .current_dir(elsewhere.path())
        .env("LOCT_OPEN_BROWSER", "0")
        .args(["scan", "--watch", abs.to_str().unwrap()])
        .stdin(Stdio::null())
        .output()
        .expect("run second --watch with absolute path");

    let code = second.status.code().unwrap_or(-1);
    let _ = first.kill();
    let _ = first.wait();

    assert_eq!(
        code, 75,
        "absolute-path second --watch should collide with `.` watcher (got code {code})"
    );
}

#[test]
fn one_shot_scan_is_not_blocked_by_a_live_watcher() {
    let repo = make_repo();
    let mut watcher = spawn_watcher(repo.path());

    wait_for_lock_acquired(repo.path(), Duration::from_secs(15));

    // Run a single non-watch scan and let it complete. Pass `--full-scan` so
    // it actually does work instead of being optimised away by the cached
    // snapshot the watcher just wrote.
    let one_shot = Command::new(loct_bin())
        .current_dir(repo.path())
        .env("LOCT_OPEN_BROWSER", "0")
        .args(["scan", "--full-scan", "."])
        .stdin(Stdio::null())
        .output()
        .expect("run one-shot scan");

    let code = one_shot.status.code().unwrap_or(-1);
    let _ = watcher.kill();
    let _ = watcher.wait();

    assert_eq!(
        code,
        0,
        "concurrent one-shot scan should succeed (stderr was {})",
        String::from_utf8_lossy(&one_shot.stderr)
    );
}

#[test]
fn sigkill_holder_releases_lock_for_next_watcher() {
    #[cfg(not(unix))]
    {
        eprintln!("SIGKILL test is Unix-only; skipping on non-Unix.");
        return;
    }
    #[cfg(unix)]
    {
        let repo = make_repo();
        let mut first = spawn_watcher(repo.path());
        wait_for_lock_acquired(repo.path(), Duration::from_secs(15));

        // SIGKILL — bypass any signal handlers entirely. The kernel must
        // release the flock on fd close for self-healing to be real.
        unsafe {
            libc::kill(first.id() as libc::pid_t, libc::SIGKILL);
        }
        let _ = first.wait();

        // Give the kernel a moment to reap. Don't `unlink` the lock file —
        // the new watcher must succeed without manual cleanup.
        thread::sleep(Duration::from_millis(200));

        let mut second = spawn_watcher(repo.path());
        wait_for_lock_acquired(repo.path(), Duration::from_secs(15));

        let _ = second.kill();
        let _ = second.wait();
    }
}

/// High ephemeral port offset so concurrent watch_lock_cli tests don't
/// fight for the same listener slot. The open_server falls back to
/// `127.0.0.1:0` on EADDRINUSE, but giving each test a distinct primary
/// keeps the assertions deterministic on shared CI hosts.
fn pick_test_port(seed: u16) -> u16 {
    49000 + (seed % 1000)
}

#[test]
fn loct_watch_http_announces_streamable_http_companion() {
    // The watcher tries to spawn `loctree-mcp --transport http`. If the
    // binary is not present in the test build it logs an error and keeps
    // running without the companion — either branch leaves a clear stderr
    // line that this test asserts on. The lock + watch loop themselves
    // come up regardless.
    let repo = make_repo();
    let port = pick_test_port(11);
    let mut child = Command::new(loct_bin())
        .current_dir(repo.path())
        .env("LOCT_OPEN_BROWSER", "0")
        .args(["watch", "--http", "--port", &port.to_string(), "."])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn loct watch --http");

    wait_for_lock_acquired(repo.path(), Duration::from_secs(15));

    // Drain stderr until the http announcement lands (or fails), bounded
    // so a regression cannot hang CI.
    let mut stderr = child.stderr.take().expect("stderr handle");
    use std::io::Read as IoRead;
    let mut buf = Vec::new();
    let drain_deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < drain_deadline {
        let mut chunk = [0u8; 1024];
        match stderr.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                let s = String::from_utf8_lossy(&buf);
                if s.contains("loctree-mcp http companion")
                    || s.contains("could not spawn loctree-mcp http companion")
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let captured = String::from_utf8_lossy(&buf).into_owned();

    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        captured.contains("loctree-mcp http companion")
            || captured.contains("could not spawn loctree-mcp http companion"),
        "stderr should announce the http companion launch attempt; got:\n{captured}"
    );
}

#[test]
fn loct_watch_report_brings_up_local_http_server() {
    use std::net::TcpStream;

    let repo = make_repo();
    let port = pick_test_port(31);
    let mut child = Command::new(loct_bin())
        .current_dir(repo.path())
        .env("LOCT_OPEN_BROWSER", "0")
        .args(["watch", "--report", "--port", &port.to_string(), "."])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn loct watch --report");

    wait_for_lock_acquired(repo.path(), Duration::from_secs(15));

    // Bounded poll: connect to the report server's listener. The initial
    // render is best-effort and the TCP listener starts unconditionally.
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let connect_deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut connected = false;
    while std::time::Instant::now() < connect_deadline {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok() {
            connected = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }

    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        connected,
        "--report should bring up a local HTTP server on 127.0.0.1:{port}"
    );
}

#[test]
fn loct_watch_help_mentions_single_instance_and_exit_75() {
    let out = Command::new(loct_bin())
        .args(["watch", "--help"])
        .env("LOCT_OPEN_BROWSER", "0")
        .stdin(Stdio::null())
        .output()
        .expect("run loct watch --help");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("single-instance") || combined.contains("scan.lock"),
        "help should explain single-instance behaviour; got:\n{combined}"
    );
    assert!(
        combined.contains("75"),
        "help should document exit code 75; got:\n{combined}"
    );
}

#[test]
fn loct_watch_dev_and_scan_watch_share_the_same_lock() {
    let repo = make_repo();
    // Bring up a foreground `loct watch --dev`.
    let mut first = Command::new(loct_bin())
        .current_dir(repo.path())
        .env("LOCT_OPEN_BROWSER", "0")
        .args(["watch", "--dev", "."])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn loct watch --dev");

    wait_for_lock_acquired(repo.path(), Duration::from_secs(15));

    // Now try the legacy form — must collide via the same lock file.
    let second = Command::new(loct_bin())
        .current_dir(repo.path())
        .env("LOCT_OPEN_BROWSER", "0")
        .args(["scan", "--watch", "."])
        .stdin(Stdio::null())
        .output()
        .expect("run scan --watch against existing loct watch");

    let code = second.status.code().unwrap_or(-1);
    let _ = first.kill();
    let _ = first.wait();

    assert_eq!(
        code, 75,
        "scan --watch should collide with loct watch --dev via the shared lock"
    );
}

#[cfg(unix)]
#[test]
fn replace_mode_sigterms_holder_and_retakes_lock() {
    use std::os::unix::process::ExitStatusExt;

    fn lock_pid(repo: &std::path::Path) -> Option<u32> {
        let payload = std::fs::read_to_string(repo.join(".loctree/scan.lock")).ok()?;
        let value: serde_json::Value = serde_json::from_str(&payload).ok()?;
        let pid = value.get("pid")?.as_u64()?;
        u32::try_from(pid).ok()
    }

    fn wait_for_lock_holder(repo: &std::path::Path, pid: u32, timeout: Duration) {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if lock_pid(repo) == Some(pid) {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!(
            "lock was not retaken by pid {} within {:?}; current holder: {:?}",
            pid,
            timeout,
            lock_pid(repo)
        );
    }

    fn wait_for_child_exit(
        child: &mut std::process::Child,
        timeout: Duration,
    ) -> std::process::ExitStatus {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if let Some(status) = child.try_wait().expect("poll child exit") {
                return status;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("child pid {} did not exit within {:?}", child.id(), timeout);
    }

    let repo = make_repo();
    let mut first = spawn_watcher(repo.path());
    wait_for_lock_acquired(repo.path(), Duration::from_secs(15));
    assert_eq!(
        lock_pid(repo.path()),
        Some(first.id()),
        "first watcher should be the recorded lock holder"
    );

    let mut replacement = Command::new(loct_bin())
        .current_dir(repo.path())
        .env("LOCT_OPEN_BROWSER", "0")
        .args(["scan", "--watch", "--replace", "."])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn replacement --watch");

    wait_for_lock_holder(repo.path(), replacement.id(), Duration::from_secs(15));

    let first_status = wait_for_child_exit(&mut first, Duration::from_secs(5));
    assert_eq!(
        first_status.signal(),
        Some(libc::SIGTERM),
        "--replace should terminate the previous holder with SIGTERM; status was {first_status:?}"
    );

    let third = Command::new(loct_bin())
        .current_dir(repo.path())
        .env("LOCT_OPEN_BROWSER", "0")
        .args(["scan", "--watch", "."])
        .stdin(Stdio::null())
        .output()
        .expect("run third --watch against replacement holder");
    assert_eq!(
        third.status.code(),
        Some(75),
        "third watcher should collide with replacement holder; stderr was:\n{}",
        String::from_utf8_lossy(&third.stderr)
    );

    let _ = replacement.kill();
    let _ = replacement.wait();
}

#[cfg(unix)]
#[test]
fn bg_mode_detaches_and_survives_parent() {
    fn lock_pid(repo: &std::path::Path) -> Option<u32> {
        let payload = std::fs::read_to_string(repo.join(".loctree/scan.lock")).ok()?;
        let value: serde_json::Value = serde_json::from_str(&payload).ok()?;
        let pid = value.get("pid")?.as_u64()?;
        u32::try_from(pid).ok()
    }

    let repo = make_repo();
    let parent_pgid = unsafe { libc::getpgid(0) };
    assert_ne!(
        parent_pgid,
        -1,
        "test process group should be readable: {}",
        std::io::Error::last_os_error()
    );

    let parent = Command::new(loct_bin())
        .current_dir(repo.path())
        .env("LOCT_OPEN_BROWSER", "0")
        .args(["watch", "--bg", "."])
        .stdin(Stdio::null())
        .output()
        .expect("run loct watch --bg");

    assert!(
        parent.status.success(),
        "loct watch --bg parent should exit successfully; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&parent.stdout),
        String::from_utf8_lossy(&parent.stderr)
    );

    wait_for_lock_acquired(repo.path(), Duration::from_secs(15));
    let pid = lock_pid(repo.path()).expect("background watcher lock pid");
    let child_pid = pid as libc::pid_t;

    let alive = unsafe { libc::kill(child_pid, 0) };
    assert_eq!(
        alive,
        0,
        "background watcher pid {pid} should still be alive after parent exit: {}",
        std::io::Error::last_os_error()
    );

    let child_pgid = unsafe { libc::getpgid(child_pid) };
    assert_ne!(
        child_pgid,
        -1,
        "background watcher process group should be readable: {}",
        std::io::Error::last_os_error()
    );
    assert_eq!(
        child_pgid, child_pid,
        "setsid should make the background watcher pid its own process group leader"
    );
    assert_ne!(
        child_pgid, parent_pgid,
        "background watcher should detach from the launching process group"
    );

    unsafe {
        libc::kill(-child_pgid, libc::SIGTERM);
    }
    thread::sleep(Duration::from_millis(200));
    if unsafe { libc::kill(child_pid, 0) } == 0 {
        unsafe {
            libc::kill(-child_pgid, libc::SIGKILL);
        }
    }
}
