//! Single-instance lock for long-lived watch loops.
//!
//! `loct scan --watch` and `loct watch` previously had no mutual exclusion.
//! In repos with multiple agent shells (Vibecrafted Living Tree), this trivially
//! produced 5+ concurrent watchers against the same `.loctree/` snapshot, each
//! hammering the same DB and pushing duplicate events into downstream MCP
//! consumers. The result was MCP timeouts and an opaque doctrine-drift: agents
//! falling off Loctree-first discipline because the perception layer was
//! unreliable for reasons that never surfaced near the cause.
//!
//! The lock is acquired *inside* the holding process via `fs4`'s
//! `try_lock_exclusive` (kernel `flock(LOCK_EX|LOCK_NB)` on Unix,
//! `LockFileEx` on Windows). Because the lock lives on a file descriptor,
//! it releases automatically when the holder dies — including SIGKILL,
//! panic, or OOM — without any stale-PID-file dance.
//!
//! The lock identity is the canonical snapshot root (the same path
//! `resolve_snapshot_root` returns). Two invocations with `.` vs an absolute
//! repo path therefore collide as intended, but two truly different repos
//! can hold their own locks side by side.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};

/// Recommended exit code on lock contention. 75 is the BSD `EX_TEMPFAIL`
/// convention — "temporary failure; user is invited to retry."
pub const EXIT_LOCK_CONTENDED: i32 = 75;

/// What to do when the lock is already held.
#[derive(Debug, Clone)]
pub enum LockMode {
    /// Fail fast on contention. Print holder info on stderr and exit.
    Default,
    /// SIGTERM the current holder, wait briefly, then take the lock.
    Replace,
    /// Block until the lock is free, with an optional timeout.
    /// `None` means wait indefinitely.
    Wait(Option<Duration>),
}

/// Information persisted into the lock file's contents so a second invocation
/// can name the holder. The kernel flock is the source of truth — this content
/// is descriptive only. Best-effort: a partially written / empty payload still
/// reports the contention correctly, just with less detail.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HolderInfo {
    pub pid: u32,
    /// Unix epoch seconds when the holder acquired the lock.
    pub started_at: i64,
    /// `std::env::args().collect()` of the holder.
    pub argv: Vec<String>,
    /// Canonical snapshot root the holder is watching (for diagnostics).
    pub snapshot_root: String,
}

impl fmt::Display for HolderInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let started = chrono::DateTime::from_timestamp(self.started_at, 0)
            .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
            .unwrap_or_else(|| "?".into());
        let argv = if self.argv.is_empty() {
            "<unknown>".to_string()
        } else {
            self.argv.join(" ")
        };
        write!(f, "pid={} started={} argv={}", self.pid, started, argv)
    }
}

/// Why lock acquisition failed (or how it succeeded with side effects).
#[derive(Debug)]
pub enum LockError {
    /// Another process holds the lock. Includes best-effort holder info.
    HeldBy(HolderInfo),
    /// IO error opening / locking the lock file (permission, disk, etc.).
    Io(std::io::Error),
    /// `--wait` timed out without acquiring the lock.
    WaitTimeout(HolderInfo),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockError::HeldBy(info) => write!(
                f,
                "watch already running for this repo ({info}); pass --replace to recycle or --wait to block"
            ),
            LockError::Io(e) => write!(f, "watch lock IO error: {e}"),
            LockError::WaitTimeout(info) => write!(
                f,
                "timed out waiting for watch lock; holder still alive ({info})"
            ),
        }
    }
}

impl std::error::Error for LockError {}

impl From<std::io::Error> for LockError {
    fn from(e: std::io::Error) -> Self {
        LockError::Io(e)
    }
}

/// RAII guard. While alive, the `File` keeps the underlying flock held.
/// Drop closes the fd which the kernel uses to release the lock — even on
/// `SIGKILL`, the kernel-side close still runs.
///
/// Do not call `release()` from a signal handler — instead, let the process
/// exit normally (or be killed). The fd close cleans up either way.
pub struct WatchLock {
    file: File,
    path: PathBuf,
    snapshot_root: PathBuf,
}

impl WatchLock {
    /// Lock file path inside the snapshot root.
    pub fn lock_path_for(snapshot_root: &Path) -> PathBuf {
        snapshot_root.join(".loctree").join("scan.lock")
    }

    /// Canonical repo / snapshot root the lock guards.
    pub fn snapshot_root(&self) -> &Path {
        &self.snapshot_root
    }

    /// On-disk lock file path (mostly useful for tests and diagnostics).
    pub fn lock_path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WatchLock {
    fn drop(&mut self) {
        // `fs4` releases the flock on `unlock_exclusive`. We also rely on fd
        // close to release if `unlock` is skipped (e.g. panic during drop) —
        // that is the actual self-healing guarantee.
        let _ = FileExt::unlock(&self.file);
    }
}

fn read_holder_info(file: &mut File) -> HolderInfo {
    let mut buf = String::new();
    let _ = file.seek(SeekFrom::Start(0));
    let _ = file.read_to_string(&mut buf);
    serde_json::from_str::<HolderInfo>(&buf).unwrap_or_default()
}

fn write_holder_info(file: &mut File, info: &HolderInfo) -> std::io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    let payload = serde_json::to_string(info).unwrap_or_else(|_| "{}".into());
    file.write_all(payload.as_bytes())?;
    file.flush()?;
    Ok(())
}

fn try_signal_term(_pid: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        // Best-effort SIGTERM. We deliberately use libc directly rather than
        // pulling in `nix`, because libc is already a transitive dep.
        // SAFETY: `kill` is async-signal-safe and just sends a signal; pid is
        // an integer we got from /proc /etc.
        let rc = unsafe { libc::kill(_pid as libc::pid_t, libc::SIGTERM) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "--replace not yet supported on non-Unix platforms",
        ))
    }
}

/// Acquire the watch lock for a given snapshot root. On success returns a
/// `WatchLock` guard. While the guard is alive, no other process can hold
/// the lock for the same `snapshot_root`.
///
/// `snapshot_root` should be the canonical root returned by
/// `crate::snapshot::resolve_snapshot_root`; this is how `.` and absolute
/// paths to the same repo end up sharing one lock.
pub fn acquire(snapshot_root: &Path, mode: LockMode) -> Result<WatchLock, LockError> {
    let snapshot_root = snapshot_root
        .canonicalize()
        .unwrap_or_else(|_| snapshot_root.to_path_buf());
    let loctree_dir = snapshot_root.join(".loctree");
    std::fs::create_dir_all(&loctree_dir)?;

    let lock_path = loctree_dir.join("scan.lock");
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;

    match &mode {
        LockMode::Default => {
            if FileExt::try_lock_exclusive(&file)? {
                inscribe_self(&mut file, &snapshot_root)?;
                Ok(WatchLock {
                    file,
                    path: lock_path,
                    snapshot_root,
                })
            } else {
                Err(LockError::HeldBy(read_holder_info(&mut file)))
            }
        }
        LockMode::Replace => {
            if FileExt::try_lock_exclusive(&file)? {
                inscribe_self(&mut file, &snapshot_root)?;
                return Ok(WatchLock {
                    file,
                    path: lock_path,
                    snapshot_root,
                });
            }
            let holder = read_holder_info(&mut file);
            if holder.pid != 0 {
                let _ = try_signal_term(holder.pid);
            }
            // Poll for up to 3 seconds for the holder to drop the lock.
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if FileExt::try_lock_exclusive(&file)? {
                    inscribe_self(&mut file, &snapshot_root)?;
                    return Ok(WatchLock {
                        file,
                        path: lock_path,
                        snapshot_root,
                    });
                }
                if Instant::now() >= deadline {
                    return Err(LockError::HeldBy(read_holder_info(&mut file)));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        LockMode::Wait(timeout) => {
            let start = Instant::now();
            loop {
                if FileExt::try_lock_exclusive(&file)? {
                    inscribe_self(&mut file, &snapshot_root)?;
                    return Ok(WatchLock {
                        file,
                        path: lock_path,
                        snapshot_root,
                    });
                }
                if let Some(limit) = timeout
                    && start.elapsed() >= *limit
                {
                    return Err(LockError::WaitTimeout(read_holder_info(&mut file)));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

/// Read-only probe: returns `true` when some process currently holds the
/// watch lock for `snapshot_root` (i.e. a live `loct watch` / `loct scan
/// --watch` owns snapshot freshness for that root).
///
/// Unlike `acquire`, this never creates the lock file and never writes holder
/// info — safe to call from read paths (the snapshot freshness guardian).
/// A missing lock file or any IO/lock error reports `false` (no watcher).
pub fn is_held(snapshot_root: &Path) -> bool {
    let snapshot_root = snapshot_root
        .canonicalize()
        .unwrap_or_else(|_| snapshot_root.to_path_buf());
    let lock_path = WatchLock::lock_path_for(&snapshot_root);
    let Ok(file) = OpenOptions::new().read(true).open(&lock_path) else {
        return false;
    };
    match FileExt::try_lock_shared(&file) {
        // Shared lock acquired — nobody holds the exclusive watch lock.
        Ok(true) => {
            let _ = FileExt::unlock(&file);
            false
        }
        // Exclusive lock held elsewhere — a watcher is alive.
        Ok(false) => true,
        Err(_) => false,
    }
}

fn inscribe_self(file: &mut File, snapshot_root: &Path) -> std::io::Result<()> {
    let info = HolderInfo {
        pid: std::process::id(),
        started_at: chrono::Utc::now().timestamp(),
        argv: std::env::args().collect(),
        snapshot_root: snapshot_root.to_string_lossy().to_string(),
    };
    write_holder_info(file, &info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn touch_repo(dir: &Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }

    #[test]
    fn acquire_creates_lock_file_under_loctree_dir() {
        let tmp = TempDir::new().unwrap();
        touch_repo(tmp.path());
        let guard = acquire(tmp.path(), LockMode::Default).expect("first acquire ok");
        let expected = tmp
            .path()
            .canonicalize()
            .unwrap()
            .join(".loctree/scan.lock");
        assert_eq!(guard.lock_path(), expected.as_path());
        assert!(expected.exists(), "lock file should be created");
    }

    #[test]
    fn second_acquire_within_same_process_is_rejected() {
        let tmp = TempDir::new().unwrap();
        touch_repo(tmp.path());
        let _guard = acquire(tmp.path(), LockMode::Default).expect("first ok");
        let second = acquire(tmp.path(), LockMode::Default);
        assert!(matches!(second, Err(LockError::HeldBy(_))));
    }

    #[test]
    fn drop_releases_lock() {
        let tmp = TempDir::new().unwrap();
        touch_repo(tmp.path());
        {
            let _guard = acquire(tmp.path(), LockMode::Default).expect("first ok");
        }
        // After drop, lock should be re-acquirable.
        let _again = acquire(tmp.path(), LockMode::Default).expect("re-acquire after drop");
    }

    #[test]
    fn holder_info_round_trip() {
        let tmp = TempDir::new().unwrap();
        touch_repo(tmp.path());
        let _guard = acquire(tmp.path(), LockMode::Default).expect("ok");
        let lock_path = tmp
            .path()
            .canonicalize()
            .unwrap()
            .join(".loctree/scan.lock");
        let mut f = OpenOptions::new().read(true).open(&lock_path).unwrap();
        let info = read_holder_info(&mut f);
        assert_eq!(info.pid, std::process::id());
        assert!(!info.argv.is_empty(), "argv recorded");
    }

    #[test]
    fn is_held_probe_tracks_lock_lifecycle_without_side_effects() {
        let tmp = TempDir::new().unwrap();
        touch_repo(tmp.path());

        // No lock file yet — probe must not create one.
        assert!(!is_held(tmp.path()), "no watcher yet");
        assert!(
            !WatchLock::lock_path_for(&tmp.path().canonicalize().unwrap()).exists(),
            "probe must not create the lock file"
        );

        let guard = acquire(tmp.path(), LockMode::Default).expect("acquire ok");
        assert!(is_held(tmp.path()), "held while guard alive");
        // Probe must not clobber holder info.
        let mut f = OpenOptions::new()
            .read(true)
            .open(guard.lock_path())
            .unwrap();
        let info = read_holder_info(&mut f);
        assert_eq!(info.pid, std::process::id(), "holder info preserved");

        drop(guard);
        assert!(!is_held(tmp.path()), "released after drop");
    }

    #[test]
    fn wait_mode_times_out_when_held() {
        let tmp = TempDir::new().unwrap();
        touch_repo(tmp.path());
        let _guard = acquire(tmp.path(), LockMode::Default).expect("ok");
        let started = Instant::now();
        let res = acquire(tmp.path(), LockMode::Wait(Some(Duration::from_millis(400))));
        let elapsed = started.elapsed();
        assert!(matches!(res, Err(LockError::WaitTimeout(_))));
        assert!(
            elapsed >= Duration::from_millis(400),
            "should wait at least the timeout"
        );
        assert!(
            elapsed < Duration::from_millis(2000),
            "should not wait excessively past timeout"
        );
    }

    #[test]
    fn lock_path_resolves_through_relative_and_absolute_to_same_file() {
        let tmp = TempDir::new().unwrap();
        touch_repo(tmp.path());

        let abs = tmp.path().canonicalize().unwrap();
        let guard = acquire(&abs, LockMode::Default).expect("ok");
        // Now from an alternate spelling (still pointing at the same dir),
        // acquire should be rejected — proving path canonicalization works.
        let alt = abs.join("./.");
        let second = acquire(&alt, LockMode::Default);
        assert!(matches!(second, Err(LockError::HeldBy(_))));
        drop(guard);
    }

    /// Cross-process: spawn a `sleep` child that acquires the lock through a
    /// helper binary scenario. Skipped if env var disables it, because we
    /// can't always rely on a built `loct` binary in unit test context.
    /// This is a smoke test — the real cross-process check lives in
    /// `loctree-rs/tests/watch_lock_cli.rs` via `assert_cmd`.
    #[test]
    fn fork_child_holding_lock_blocks_parent_acquire() {
        if std::env::var("LOCT_SKIP_FORK_TEST").is_ok() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        touch_repo(tmp.path());
        let tmp_path = tmp.path().to_path_buf();

        // Spawn a child shell that opens the lock file and flocks it via
        // `flock(1)` — available on Linux but not always macOS. On macOS we
        // fall back to a Rust helper. Skip if neither available.
        let flock_avail = Command::new("which")
            .arg("flock")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !flock_avail {
            return;
        }

        // Create .loctree dir first so flock(1) can open the path.
        std::fs::create_dir_all(tmp_path.join(".loctree")).unwrap();
        let lock_file = tmp_path.join(".loctree/scan.lock");
        std::fs::write(&lock_file, "{}").unwrap();

        let mut child = Command::new("flock")
            .args(["-x", lock_file.to_str().unwrap(), "-c", "sleep 2"])
            .spawn()
            .expect("spawn flock child");

        std::thread::sleep(Duration::from_millis(200));
        let parent_attempt = acquire(&tmp_path, LockMode::Default);
        assert!(
            matches!(parent_attempt, Err(LockError::HeldBy(_))),
            "parent should see lock as held"
        );

        let _ = child.kill();
        let _ = child.wait();
    }
}
