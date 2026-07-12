//! Process spawn + parser glue for the [`super::AicxClient`] wrapper.
//!
//! Stays purely synchronous (no `tokio::process`) to align with Loctree's sync
//! scan workflow. A poll-based timeout caps stuck invocations at
//! [`DEFAULT_TIMEOUT`]. JSON output is parsed via `serde_json`; `aicx steer`
//! falls back to the published 3-line text block format because that
//! subcommand does not yet emit JSON.

use serde::Deserialize;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::{AicxIntent, AicxSearchResult, AicxSteerResult, OracleStatus};

/// Environment variable used by tests and operators to override the `aicx`
/// binary path. When unset, the wrapper falls back to `aicx` on `PATH`.
pub const AICX_BINARY_ENV: &str = "LOCT_AICX_BINARY";

/// Default ceiling on how long a single `aicx` invocation may run.
///
/// Cut 5 follow-up: T0 originally set this to 5 s, but `aicx intents` on a
/// busy bucket (loctree-suite during the Cut 4/5 marathon, ~100 rows in 7 s
/// wall clock) routinely exceeds that budget, leaving the memory slice
/// silently empty. 15 s gives realistic stores headroom while still
/// bounding worst-case latency. Operators can override per-process via
/// `LOCT_AICX_TIMEOUT_SECS` (e.g. `5` to restore the old conservative
/// limit, or `60` for forensic wide-window queries).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// Environment variable used to override [`DEFAULT_TIMEOUT`] (in seconds).
pub const AICX_TIMEOUT_ENV: &str = "LOCT_AICX_TIMEOUT_SECS";

const POLL_INTERVAL: Duration = Duration::from_millis(20);

fn invocation_timeout() -> Duration {
    std::env::var(AICX_TIMEOUT_ENV)
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// Resolve which binary to invoke for `aicx`.
fn aicx_binary() -> PathBuf {
    std::env::var(AICX_BINARY_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("aicx"))
}

/// Probe whether the `aicx` binary can be invoked at all.
///
/// Used by callers (e.g. the `loct context --with-aicx` composer) to
/// distinguish "binary missing" from "binary present but no relevant
/// intents". Tries `aicx --version` with a short timeout; returns `true`
/// only when the process exits successfully. Discards stdout/stderr.
///
/// The probe result is cached for the process lifetime: `loct` is a
/// one-shot CLI, and the composer + pill renderer used to re-spawn this
/// probe up to four times per `loct context` run for the same answer.
pub fn is_aicx_available() -> bool {
    static PROBE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PROBE.get_or_init(is_aicx_available_uncached)
}

fn is_aicx_available_uncached() -> bool {
    let bin = aicx_binary();
    let mut child = match Command::new(&bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    let started = Instant::now();
    let probe_timeout = Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if started.elapsed() > probe_timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Emit a single debug line on stderr, gated by `LOCT_DEBUG`.
pub(super) fn debug_log(msg: impl AsRef<str>) {
    if std::env::var("LOCT_DEBUG")
        .ok()
        .filter(|v| !v.is_empty())
        .is_some()
    {
        eprintln!("[loctree::aicx] {}", msg.as_ref());
    }
}

/// Failure shape for a single `aicx` invocation.
///
/// `Timeout` is kept distinct from `Failed` so the memory-slice composer can
/// report "overlay skipped (timeout)" instead of presenting a timed-out
/// store as an empty one — the two demand different operator reactions
/// (raise `LOCT_AICX_TIMEOUT_SECS` vs. nothing to recall).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AicxRunFailure {
    /// The child outlived [`invocation_timeout`] and was killed.
    Timeout,
    /// Spawn failure, wait failure, or non-zero exit.
    Failed,
}

/// Run `aicx <args>` and return captured stdout. Returns `None` and emits a
/// debug line on any failure (binary missing, non-zero exit, timeout).
pub(super) fn run_aicx(args: &[&str]) -> Option<String> {
    run_aicx_outcome(args, None).ok()
}

/// Run `aicx <args>` and return captured stdout, keeping the failure kind.
///
/// `timeout_cap` tightens (never widens) the env-tunable invocation
/// timeout — budgeted callers pass their remaining wall-clock budget.
///
/// Stdout and stderr are drained in dedicated worker threads so a child that
/// emits more than the OS pipe buffer (~64 KiB on macOS) does not block on
/// `write` while the parent is still polling `try_wait`. Without that drain
/// the previous implementation would silently time out on any `aicx intents`
/// payload above the buffer ceiling.
pub(super) fn run_aicx_outcome(
    args: &[&str],
    timeout_cap: Option<Duration>,
) -> Result<String, AicxRunFailure> {
    let bin = aicx_binary();
    let mut cmd = Command::new(&bin);
    cmd.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            debug_log(format!(
                "spawn failed for {} (args={:?}): {}",
                bin.display(),
                args,
                e
            ));
            return Err(AicxRunFailure::Failed);
        }
    };

    let (tx_out, rx_out) = std::sync::mpsc::channel();
    let _stdout_handle = child.stdout.take().map(|mut handle| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = handle.read_to_end(&mut buf);
            let _ = tx_out.send(buf);
        })
    });

    let (tx_err, rx_err) = std::sync::mpsc::channel();
    let _stderr_handle = child.stderr.take().map(|mut handle| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = handle.read_to_end(&mut buf);
            let _ = tx_err.send(buf);
        })
    });

    let started = Instant::now();
    let base_timeout = invocation_timeout();
    let timeout = timeout_cap
        .map(|cap| cap.min(base_timeout))
        .unwrap_or(base_timeout);
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() > timeout {
                    #[cfg(unix)]
                    {
                        let _ = Command::new("kill")
                            .arg("-9")
                            .arg(format!("-{}", child.id()))
                            .status();
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = child.kill();
                    }
                    let _ = child.wait();
                    debug_log(format!("timeout after {:?} (args={:?})", timeout, args));
                    return Err(AicxRunFailure::Timeout);
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => {
                #[cfg(unix)]
                {
                    let _ = Command::new("kill")
                        .arg("-9")
                        .arg(format!("-{}", child.id()))
                        .status();
                }
                #[cfg(not(unix))]
                {
                    let _ = child.kill();
                }
                let _ = child.wait();
                debug_log(format!("try_wait error (args={:?}): {}", args, e));
                return Err(AicxRunFailure::Failed);
            }
        }
    };

    // Child exited → pipes should close within grace period.
    let stdout_buf = rx_out
        .recv_timeout(Duration::from_millis(100))
        .unwrap_or_default();
    let stderr_buf = rx_err
        .recv_timeout(Duration::from_millis(100))
        .unwrap_or_default();

    if !exit_status.success() {
        debug_log(format!(
            "aicx exited with {:?} (args={:?}): stderr={}",
            exit_status,
            args,
            String::from_utf8_lossy(&stderr_buf).trim()
        ));
        return Err(AicxRunFailure::Failed);
    }

    Ok(String::from_utf8_lossy(&stdout_buf).into_owned())
}

// ---- intents ----------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct IntentWire {
    kind: String,
    summary: String,
    project: String,
    agent: String,
    date: String,
    #[serde(default)]
    timestamp: Option<String>,
    session_id: String,
    source_chunk: String,
    #[serde(default)]
    frame_kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IntentEnvelope {
    /// Top-level retrieval-layer provenance (closes audit finding A8).
    /// Older AICX builds omit this field — `#[serde(default)]` keeps the
    /// parser tolerant.
    #[serde(default)]
    oracle_status: Option<OracleStatus>,
    #[serde(default)]
    items: Vec<IntentWire>,
}

/// Parse the JSON emitted by `aicx intents --emit json`.
///
/// Older AICX builds emitted a bare array. Oracle-aware builds emit an envelope
/// with `oracle_status` plus `items`; Loctree now propagates the envelope's
/// `oracle_status` onto every row so downstream callers (the memory-slice
/// composer in `pack.rs` / `cli/dispatch/handlers/context/mod.rs`) can surface
/// `retrieval_mode` without having to re-parse the envelope shape.
pub(super) fn parse_intents(stdout: &str) -> Vec<AicxIntent> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let (wire, oracle_status): (Vec<IntentWire>, Option<OracleStatus>) =
        match serde_json::from_str::<Vec<IntentWire>>(trimmed) {
            Ok(v) => (v, None),
            Err(array_err) => match serde_json::from_str::<IntentEnvelope>(trimmed) {
                Ok(envelope) => (envelope.items, envelope.oracle_status),
                Err(envelope_err) => {
                    debug_log(format!(
                        "intents JSON parse error: array={}; envelope={}",
                        array_err, envelope_err
                    ));
                    return Vec::new();
                }
            },
        };
    wire.into_iter()
        .map(|w| AicxIntent {
            kind: w.kind,
            text: w.summary,
            agent: w.agent,
            date: w.date,
            timestamp: w.timestamp,
            session_id: w.session_id,
            project: w.project,
            source_chunk_path: w.source_chunk,
            frame_kind: w.frame_kind,
            oracle_status: oracle_status.clone(),
        })
        .collect()
}

// ---- search -----------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SearchEnvelope {
    /// Top-level retrieval-layer provenance (closes audit finding A8).
    /// AICX's `aicx_search` envelope always carries this; the field is
    /// optional only because older builds and bare-array test fixtures
    /// (parse_search_handles_envelope) do not include it.
    #[serde(default)]
    oracle_status: Option<OracleStatus>,
    #[serde(default)]
    items: Vec<SearchWire>,
}

#[derive(Debug, Deserialize)]
struct SearchWire {
    #[serde(default)]
    score: i64,
    #[serde(default)]
    label: Option<String>,
    project: String,
    agent: String,
    date: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    frame_kind: Option<String>,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    matches: Vec<String>,
    path: String,
}

/// Parse `aicx search ... -j` envelope into typed rows.
///
/// The envelope's top-level `oracle_status` is cloned onto every row so
/// downstream callers can interrogate retrieval-layer provenance per result
/// without re-parsing the envelope (closes audit finding A8). `None` is
/// preserved when AICX emits an older payload that predates the oracle
/// envelope.
pub(super) fn parse_search(stdout: &str) -> Vec<AicxSearchResult> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let envelope: SearchEnvelope = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(e) => {
            debug_log(format!("search JSON parse error: {}", e));
            return Vec::new();
        }
    };
    let oracle_status = envelope.oracle_status;
    envelope
        .items
        .into_iter()
        .map(|w| AicxSearchResult {
            score: w.score,
            label: w.label,
            project: w.project,
            agent: w.agent,
            date: w.date,
            timestamp: w.timestamp,
            frame_kind: w.frame_kind,
            session: w.session,
            matches: w.matches,
            path: w.path,
            oracle_status: oracle_status.clone(),
        })
        .collect()
}

// ---- steer ------------------------------------------------------------------

/// Parse the 3-line text block format emitted by `aicx steer`.
///
/// Each entry looks like:
/// ```text
/// <bucket> | <agent> | <date> | <kind>
///   run_id: <run>  prompt_id: <prompt>  model: <model>
///   <absolute path>
/// ```
/// Empty lines separate entries. Missing values are encoded as a single `-`
/// in the source (mapped to `None`).
pub(super) fn parse_steer(stdout: &str) -> Vec<AicxSteerResult> {
    let mut out = Vec::new();
    let mut lines = stdout.lines().peekable();
    while let Some(header) = lines.next() {
        let header = header.trim();
        if header.is_empty() {
            continue;
        }
        let parts: Vec<&str> = header.split('|').map(str::trim).collect();
        if parts.len() < 4 {
            continue;
        }
        let project = parts[0].to_string();
        let agent = parts[1].to_string();
        let date = parts[2].to_string();
        let kind = parts[3].to_string();

        let meta_line = match lines.next() {
            Some(l) => l.trim().to_string(),
            None => break,
        };
        let (run_id, prompt_id, model) = parse_steer_meta(&meta_line);

        let path_line = match lines.next() {
            Some(l) => l.trim().to_string(),
            None => break,
        };
        if path_line.is_empty() {
            continue;
        }

        out.push(AicxSteerResult {
            project,
            agent,
            date,
            kind,
            run_id,
            prompt_id,
            model,
            source_chunk_path: path_line,
        });

        // Skip optional blank separator before the next entry.
        if let Some(peek) = lines.peek()
            && peek.trim().is_empty()
        {
            lines.next();
        }
    }
    out
}

/// Extract `run_id`, `prompt_id`, `model` from the second line of a steer
/// entry. The line uses two-space separation between key/value pairs and a
/// literal `-` as the "missing" sentinel.
fn parse_steer_meta(line: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut run_id = None;
    let mut prompt_id = None;
    let mut model = None;
    for chunk in line.split("  ") {
        let chunk = chunk.trim();
        if let Some(rest) = chunk.strip_prefix("run_id:") {
            run_id = sentinel(rest);
        } else if let Some(rest) = chunk.strip_prefix("prompt_id:") {
            prompt_id = sentinel(rest);
        } else if let Some(rest) = chunk.strip_prefix("model:") {
            model = sentinel(rest);
        }
    }
    (run_id, prompt_id, model)
}

/// Map AICX's `-` placeholder to `None`; otherwise return the trimmed value.
fn sentinel(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "-" {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_intents_handles_canonical_payload() {
        let payload = r#"[
            {
                "kind": "intent",
                "summary": "ship cut5",
                "evidence": [],
                "project": "loctree-suite",
                "agent": "claude",
                "date": "2026-04-28",
                "timestamp": "2026-04-28T00:00:00Z",
                "session_id": "abc123",
                "count": null,
                "source_chunk": "/home/x/.aicx/store/foo.md"
            }
        ]"#;
        let parsed = parse_intents(payload);
        assert_eq!(parsed.len(), 1);
        let intent = &parsed[0];
        assert_eq!(intent.kind, "intent");
        assert_eq!(intent.text, "ship cut5");
        assert_eq!(intent.session_id, "abc123");
        assert_eq!(intent.source_chunk_path, "/home/x/.aicx/store/foo.md");
        assert_eq!(intent.timestamp.as_deref(), Some("2026-04-28T00:00:00Z"));
        assert!(intent.frame_kind.is_none());
    }

    #[test]
    fn parse_intents_handles_oracle_envelope() {
        let payload = r#"{
            "oracle_status": {
                "backend": "filesystem_fuzzy",
                "index_kind": "none",
                "fallback_reason": "fallback_filesystem_fuzzy: content index unavailable",
                "store_root": "/home/x/.aicx",
                "indexed_count": 0,
                "scanned_count": 1,
                "candidate_count": 1,
                "source_paths_verified": true,
                "stale_or_unknown": true
            },
            "results": 1,
            "items": [
                {
                    "kind": "decision",
                    "summary": "keep oracle fallback explicit",
                    "evidence": [],
                    "project": "loctree-suite",
                    "agent": "codex",
                    "date": "2026-05-04",
                    "timestamp": "2026-05-04T12:00:00Z",
                    "session_id": "oracle-1",
                    "source_chunk": "/home/x/.aicx/store/oracle.md",
                    "frame_kind": "assistant"
                }
            ]
        }"#;
        let parsed = parse_intents(payload);
        assert_eq!(parsed.len(), 1);
        let intent = &parsed[0];
        assert_eq!(intent.kind, "decision");
        assert_eq!(intent.text, "keep oracle fallback explicit");
        assert_eq!(intent.agent, "codex");
        assert_eq!(intent.session_id, "oracle-1");
        assert_eq!(intent.frame_kind.as_deref(), Some("assistant"));
    }

    #[test]
    fn parse_intents_returns_empty_on_garbage() {
        assert!(parse_intents("").is_empty());
        assert!(parse_intents("not json at all { [").is_empty());
        assert!(parse_intents("null").is_empty());
    }

    #[test]
    fn parse_search_handles_envelope() {
        let payload = r#"{
            "results": 1,
            "scanned": 100,
            "items": [
                {
                    "score": 85,
                    "label": "HIGH",
                    "project": "Loctree/loctree-suite",
                    "agent": "claude",
                    "date": "2026-04-28",
                    "timestamp": "2026-04-28T00:00:00Z",
                    "frame_kind": "tool_call",
                    "session": "df9d7e52",
                    "cwd": "/tmp",
                    "matches": ["snippet a", "snippet b"],
                    "path": "/home/x/.aicx/foo.md"
                }
            ]
        }"#;
        let parsed = parse_search(payload);
        assert_eq!(parsed.len(), 1);
        let row = &parsed[0];
        assert_eq!(row.score, 85);
        assert_eq!(row.label.as_deref(), Some("HIGH"));
        assert_eq!(row.frame_kind.as_deref(), Some("tool_call"));
        assert_eq!(row.matches.len(), 2);
        assert_eq!(row.path, "/home/x/.aicx/foo.md");
    }

    #[test]
    fn parse_search_returns_empty_on_garbage() {
        assert!(parse_search("").is_empty());
        assert!(parse_search("garbage}").is_empty());
    }

    #[test]
    fn parse_steer_handles_three_line_blocks() {
        let payload = "Loctree/loctree-suite | claude | 2026-04-19 | conversations\n  run_id: -  prompt_id: -  model: -\n  /home/tester/.aicx/store/foo.md\n\nLoctree/loctree-suite | codex | 2026-04-25 | reports\n  run_id: mrbl-001  prompt_id: cut3-task  model: opus-4.7\n  /home/tester/.aicx/store/bar.md\n";
        let parsed = parse_steer(payload);
        assert_eq!(parsed.len(), 2);

        assert_eq!(parsed[0].agent, "claude");
        assert_eq!(parsed[0].kind, "conversations");
        assert!(parsed[0].run_id.is_none());
        assert!(parsed[0].prompt_id.is_none());
        assert!(parsed[0].model.is_none());

        assert_eq!(parsed[1].agent, "codex");
        assert_eq!(parsed[1].kind, "reports");
        assert_eq!(parsed[1].run_id.as_deref(), Some("mrbl-001"));
        assert_eq!(parsed[1].prompt_id.as_deref(), Some("cut3-task"));
        assert_eq!(parsed[1].model.as_deref(), Some("opus-4.7"));
        assert_eq!(
            parsed[1].source_chunk_path,
            "/home/tester/.aicx/store/bar.md"
        );
    }

    #[test]
    fn parse_steer_returns_empty_for_blank_input() {
        assert!(parse_steer("").is_empty());
        assert!(parse_steer("\n\n\n").is_empty());
    }

    #[test]
    fn parse_steer_skips_truncated_blocks() {
        let payload = "Loctree/loctree-suite | claude | 2026-04-19 | conversations\n  run_id: -  prompt_id: -  model: -\n";
        // Path line missing entirely → no entry is produced.
        let parsed = parse_steer(payload);
        assert!(parsed.is_empty());
    }

    // ---------------------------------------------------------------------
    // Audit finding A8 — `oracle_status` parsing + propagation
    // ---------------------------------------------------------------------

    use crate::aicx::{OracleBackend, OracleIndexKind};

    #[test]
    fn parse_search_propagates_filesystem_fuzzy_oracle_status() {
        // Wire shape mirrors `aicx::rank::CompactSearchResponse` exactly —
        // top-level `oracle_status` plus `items`. Filesystem-fuzzy is the
        // most operationally important variant: it tells the caller the
        // embedded semantic index is NOT serving this answer.
        let payload = r#"{
            "oracle_status": {
                "source_layer": "layer_1_canonical_corpus",
                "backend": "filesystem_fuzzy",
                "index_kind": "none",
                "fallback_reason": "fallback_filesystem_fuzzy: content index unavailable",
                "derived_view": "none_filesystem_scan",
                "store_root": "/home/x/.aicx",
                "indexed_count": 0,
                "scanned_count": 127,
                "candidate_count": 1,
                "source_paths_verified": true,
                "stale_or_unknown": true,
                "loctree_scope_safe": false,
                "loctree_scope_note": "unsafe_for_scope_narrowing; use as routing evidence, then read canonical chunks"
            },
            "results": 1,
            "scanned": 127,
            "items": [
                {
                    "score": 88,
                    "label": "HIGH",
                    "project": "Loctree/loctree-suite",
                    "agent": "codex",
                    "date": "2026-04-28",
                    "matches": ["snippet"],
                    "path": "/tmp/fuzzy.md"
                }
            ]
        }"#;

        let parsed = parse_search(payload);
        assert_eq!(parsed.len(), 1);
        let row = &parsed[0];
        let status = row
            .oracle_status
            .as_ref()
            .expect("filesystem_fuzzy envelope must propagate oracle_status");

        assert_eq!(status.backend, OracleBackend::FilesystemFuzzy);
        assert_eq!(status.index_kind, OracleIndexKind::None);
        assert_eq!(
            status.fallback_reason.as_deref(),
            Some("fallback_filesystem_fuzzy: content index unavailable"),
            "fallback_reason must propagate verbatim"
        );
        assert!(status.stale_or_unknown);
        assert!(!status.loctree_scope_safe);
        assert_eq!(status.scanned_count, 127);
        assert_eq!(status.candidate_count, 1);
        assert_eq!(status.retrieval_mode(), "filesystem_fuzzy_fallback");
    }

    #[test]
    fn parse_search_propagates_content_semantic_oracle_status() {
        // Embedded semantic backend — the high-quality retrieval path.
        // Closes finding A8 for the "embedded layer up" branch: callers
        // must see `embedded_semantic` so they can stop warning operators
        // about degraded retrieval.
        let payload = r#"{
            "oracle_status": {
                "source_layer": "layer_2_embedded_semantic",
                "backend": "content_semantic",
                "index_kind": "content_chunks",
                "fallback_reason": null,
                "derived_view": "embedded_semantic_top_k",
                "store_root": "/home/x/.aicx",
                "indexed_count": 5000,
                "scanned_count": 5000,
                "candidate_count": 5,
                "source_paths_verified": true,
                "stale_or_unknown": false,
                "loctree_scope_safe": true,
                "loctree_scope_note": "safe_as_semantic_oracle"
            },
            "results": 1,
            "scanned": 5000,
            "items": [
                {
                    "score": 95,
                    "label": "HIGH",
                    "project": "Loctree/loctree-suite",
                    "agent": "claude",
                    "date": "2026-04-30",
                    "matches": ["embedded match"],
                    "path": "/tmp/embed.md"
                }
            ]
        }"#;

        let parsed = parse_search(payload);
        let status = parsed[0]
            .oracle_status
            .as_ref()
            .expect("content_semantic envelope must propagate oracle_status");

        assert_eq!(status.backend, OracleBackend::ContentSemantic);
        assert_eq!(status.index_kind, OracleIndexKind::ContentChunks);
        assert_eq!(status.fallback_reason, None);
        assert!(!status.stale_or_unknown);
        assert!(status.loctree_scope_safe);
        assert_eq!(status.retrieval_mode(), "embedded_semantic");
    }

    #[test]
    fn parse_search_propagates_canonical_corpus_oracle_status() {
        // Canonical-corpus chunk scan: between the two extremes — no
        // semantic index, but no fuzzy fallback either.
        let payload = r#"{
            "oracle_status": {
                "source_layer": "layer_1_canonical_corpus",
                "backend": "canonical_corpus",
                "index_kind": "canonical_chunks",
                "derived_view": "canonical_chunk_scan_no_semantic_index",
                "store_root": "/home/x/.aicx",
                "indexed_count": 0,
                "scanned_count": 200,
                "candidate_count": 2,
                "source_paths_verified": true,
                "stale_or_unknown": false,
                "loctree_scope_safe": true
            },
            "results": 0,
            "scanned": 200,
            "items": [
                {
                    "score": 70,
                    "label": "MEDIUM",
                    "project": "x",
                    "agent": "claude",
                    "date": "2026-04-29",
                    "path": "/tmp/canon.md"
                }
            ]
        }"#;

        let parsed = parse_search(payload);
        let status = parsed[0].oracle_status.as_ref().unwrap();
        assert_eq!(status.backend, OracleBackend::CanonicalCorpus);
        assert_eq!(status.index_kind, OracleIndexKind::CanonicalChunks);
        assert_eq!(status.retrieval_mode(), "canonical_corpus");
    }

    #[test]
    fn parse_search_legacy_envelope_without_oracle_status() {
        // Older AICX builds (or test fixtures) emit no `oracle_status`.
        // The wrapper must stay tolerant — `oracle_status: None`, not a
        // parse error.
        let payload = r#"{
            "results": 1,
            "scanned": 10,
            "items": [
                {
                    "score": 50,
                    "label": "LOW",
                    "project": "x",
                    "agent": "claude",
                    "date": "2026-04-01",
                    "path": "/tmp/legacy.md"
                }
            ]
        }"#;
        let parsed = parse_search(payload);
        assert_eq!(parsed.len(), 1);
        assert!(
            parsed[0].oracle_status.is_none(),
            "legacy envelope without oracle_status must yield None, not a parse error"
        );
    }

    #[test]
    fn parse_search_unknown_backend_falls_through_to_unknown_variant() {
        // Forward-compat: a future AICX backend variant must NOT cause
        // the parser to fail. The mirror's `#[serde(other)]` catch-all
        // turns it into `OracleBackend::Unknown`.
        let payload = r#"{
            "oracle_status": {
                "backend": "future_quantum_oracle",
                "index_kind": "none"
            },
            "items": [
                {
                    "score": 1,
                    "label": "LOW",
                    "project": "x",
                    "agent": "claude",
                    "date": "2026-04-01",
                    "path": "/tmp/future.md"
                }
            ]
        }"#;
        let parsed = parse_search(payload);
        let status = parsed[0].oracle_status.as_ref().unwrap();
        assert_eq!(status.backend, OracleBackend::Unknown);
        assert_eq!(status.retrieval_mode(), "unknown");
    }

    #[test]
    fn parse_intents_propagates_oracle_status_to_every_row() {
        // Intents share the same oracle envelope as search. The composer
        // pulls retrieval-mode provenance from the intents path — so the
        // shell parser must propagate `oracle_status` onto every row.
        let payload = r#"{
            "oracle_status": {
                "backend": "filesystem_fuzzy",
                "index_kind": "none",
                "fallback_reason": "fallback_filesystem_fuzzy: content index unavailable",
                "stale_or_unknown": true
            },
            "results": 2,
            "items": [
                {
                    "kind": "decision",
                    "summary": "first",
                    "project": "x",
                    "agent": "claude",
                    "date": "2026-04-29",
                    "session_id": "s1",
                    "source_chunk": "/tmp/a.md"
                },
                {
                    "kind": "intent",
                    "summary": "second",
                    "project": "x",
                    "agent": "codex",
                    "date": "2026-04-30",
                    "session_id": "s2",
                    "source_chunk": "/tmp/b.md"
                }
            ]
        }"#;
        let parsed = parse_intents(payload);
        assert_eq!(parsed.len(), 2);
        for intent in &parsed {
            let status = intent.oracle_status.as_ref().unwrap_or_else(|| {
                panic!("intent {} must inherit envelope oracle_status", intent.text)
            });
            assert_eq!(status.backend, OracleBackend::FilesystemFuzzy);
            assert_eq!(
                status.fallback_reason.as_deref(),
                Some("fallback_filesystem_fuzzy: content index unavailable")
            );
            assert!(status.stale_or_unknown);
        }
    }

    #[test]
    fn parse_intents_bare_array_yields_no_oracle_status() {
        // Old AICX builds emit a bare `[...]` array. The parser must
        // handle that and leave `oracle_status` as `None` — never
        // fabricate a default-filled status, because that would be
        // indistinguishable from a real `Unknown` backend.
        let payload = r#"[
            {
                "kind": "intent",
                "summary": "legacy",
                "project": "x",
                "agent": "claude",
                "date": "2026-04-29",
                "session_id": "s1",
                "source_chunk": "/tmp/a.md"
            }
        ]"#;
        let parsed = parse_intents(payload);
        assert_eq!(parsed.len(), 1);
        assert!(
            parsed[0].oracle_status.is_none(),
            "bare-array wire must NOT synthesise an Unknown oracle_status"
        );
    }
}
