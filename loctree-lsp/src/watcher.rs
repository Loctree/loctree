//! Background filesystem watcher with debounced rescan + scan-progress
//! notifications. Plan 10 of the LSP roadmap.
//!
//! Design:
//! - A single `notify::RecommendedWatcher` subscribes to the workspace
//!   root at start-up.
//! - Filesystem events flow through a `tokio::sync::mpsc` channel into a
//!   background tokio task.
//! - The task collects events for `debounce_ms` after the first event of
//!   a batch fires, then triggers a full rescan via the analyzer's
//!   `run_init_with_options`.
//! - During the rescan the server emits `loctree/scanProgress`
//!   notifications (`scanning` → `done` or `scanning` → `failed`).
//! - Snapshot replacement is atomic: the rescan writes a new
//!   `snapshot.json` to the global cache, and the LSP backend's
//!   `SnapshotState::load` swaps the in-RAM `Arc<RwLock<...>>` value.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders ⓒ 2025-2026 Vetcoders

use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_lsp::lsp_types::notification::Notification;

/// Default debounce window when init options don't provide one.
pub const DEFAULT_DEBOUNCE_MS: u64 = 300;

/// Fallback when init options don't say otherwise — watcher is on by default.
pub const DEFAULT_ENABLED: bool = true;

/// Default polling tick when collecting events from the watcher channel.
pub const POLL_INTERVAL_MS: u64 = 50;

/// Phase of a scan progress notification.
///
/// JSON-serialized as lowercase string for simple client consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanPhase {
    Scanning,
    Composing,
    Done,
    Failed,
}

impl ScanPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            ScanPhase::Scanning => "scanning",
            ScanPhase::Composing => "composing",
            ScanPhase::Done => "done",
            ScanPhase::Failed => "failed",
        }
    }
}

/// Payload of `loctree/scanProgress` notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProgress {
    pub phase: String,
    pub files_processed: usize,
    pub total_files: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta_seconds: Option<f64>,
    /// Optional message — populated on `failed` to carry the cause.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Stable class of a failed rescan: `enospc` or `other`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Class of a failed workspace rescan, named on `loctree/scanProgress`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RescanFailureReason {
    /// POSIX ENOSPC / `ErrorKind::StorageFull` / "no space left on device".
    Enospc,
    /// Any other rescan or snapshot-reload error.
    Other,
}

impl RescanFailureReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enospc => "enospc",
            Self::Other => "other",
        }
    }

    pub fn from_message(message: &str) -> Self {
        if looks_like_enospc(message) {
            Self::Enospc
        } else {
            Self::Other
        }
    }

    pub fn from_io(err: &std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::StorageFull {
            return Self::Enospc;
        }
        #[cfg(unix)]
        {
            if err.raw_os_error() == Some(libc::ENOSPC) {
                return Self::Enospc;
            }
        }
        Self::from_message(&err.to_string())
    }

    pub fn from_error(err: &(dyn std::error::Error + 'static)) -> Self {
        let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
        while let Some(cause) = current {
            if let Some(io_err) = cause.downcast_ref::<std::io::Error>()
                && Self::from_io(io_err) == Self::Enospc
            {
                return Self::Enospc;
            }
            if Self::from_message(&cause.to_string()) == Self::Enospc {
                return Self::Enospc;
            }
            current = cause.source();
        }
        Self::Other
    }
}

fn looks_like_enospc(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("enospc")
        || lower.contains("no space left")
        || lower.contains("not enough space")
        || lower.contains("disk full")
}

/// Exponential backoff for repeated event-driven rescan failures.
///
/// The watcher keeps collecting filesystem events; this type only
/// decides whether `run_scan` is eligible. A successful rescan resets it.
#[derive(Debug, Clone, Default)]
pub struct RescanBackoff {
    consecutive_failures: u32,
    next_eligible: Option<Instant>,
    last_reason: Option<RescanFailureReason>,
}

impl RescanBackoff {
    pub const BASE_DELAY: Duration = Duration::from_millis(250);
    pub const MAX_DELAY: Duration = Duration::from_secs(30);

    pub fn delay_for_attempt(attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::ZERO;
        }
        let exp = (attempt - 1).min(7);
        Self::BASE_DELAY
            .saturating_mul(1u32 << exp)
            .min(Self::MAX_DELAY)
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    pub fn last_reason(&self) -> Option<RescanFailureReason> {
        self.last_reason
    }

    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        let deadline = self.next_eligible?;
        let wait = deadline.checked_duration_since(now)?;
        if wait.is_zero() { None } else { Some(wait) }
    }

    pub fn allow(&self, now: Instant) -> bool {
        match self.next_eligible {
            None => true,
            Some(deadline) => now >= deadline,
        }
    }

    pub fn record_failure(&mut self, now: Instant, reason: RescanFailureReason) -> Duration {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.last_reason = Some(reason);
        let delay = Self::delay_for_attempt(self.consecutive_failures);
        self.next_eligible = Some(now + delay);
        delay
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn deferred_progress(&self, now: Instant) -> Option<ScanProgress> {
        let remaining = self.remaining(now)?;
        let reason = self.last_reason.unwrap_or(RescanFailureReason::Other);
        Some(ScanProgress::failed_with_reason(
            format!(
                "rescan deferred: {} (retry in {}ms)",
                reason.as_str(),
                remaining.as_millis()
            ),
            reason,
        ))
    }

    pub fn failure_progress(&self, detail: impl Into<String>) -> ScanProgress {
        let reason = self.last_reason.unwrap_or(RescanFailureReason::Other);
        ScanProgress::failed_with_reason(format!("{}: {}", reason.as_str(), detail.into()), reason)
    }
}

/// Count summary for a completed scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanStats {
    pub files_processed: usize,
    pub total_files: usize,
}

impl ScanProgress {
    pub fn phase_only(phase: ScanPhase) -> Self {
        Self {
            phase: phase.as_str().into(),
            files_processed: 0,
            total_files: 0,
            eta_seconds: None,
            message: None,
            reason: None,
        }
    }

    pub fn with_counts(phase: ScanPhase, stats: ScanStats) -> Self {
        Self {
            phase: phase.as_str().into(),
            files_processed: stats.files_processed,
            total_files: stats.total_files,
            eta_seconds: None,
            message: None,
            reason: None,
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        let message = message.into();
        let kind = RescanFailureReason::from_message(&message);
        Self::failed_with_reason(message, kind)
    }

    pub fn failed_with_reason(message: impl Into<String>, reason: RescanFailureReason) -> Self {
        Self {
            phase: ScanPhase::Failed.as_str().into(),
            files_processed: 0,
            total_files: 0,
            eta_seconds: None,
            message: Some(message.into()),
            reason: Some(reason.as_str().into()),
        }
    }
}

/// `loctree/scanProgress` notification handle.
pub enum LoctreeScanProgress {}

impl Notification for LoctreeScanProgress {
    const METHOD: &'static str = "loctree/scanProgress";
    type Params = ScanProgress;
}

/// Configuration extracted from LSP `initialize` options.
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    pub enabled: bool,
    pub debounce: Duration,
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_ENABLED,
            debounce: Duration::from_millis(DEFAULT_DEBOUNCE_MS),
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
        }
    }
}

/// Parse the `initializationOptions` blob the client passed at startup.
///
/// Honored keys (all optional, all under the `loctree.watcher.*` namespace):
/// - `loctree.watcher.enabled`         — bool, default true
/// - `loctree.watcher.debounceMs`      — u64,  default 300
/// - `loctree.watcher.includePatterns` — array of glob strings
/// - `loctree.watcher.excludePatterns` — array of glob strings
///
/// Accepts both nested (`{"loctree":{"watcher":{...}}}`) and flat
/// (`{"loctree.watcher.enabled":true}`) shapes.
pub fn config_from_options(options: Option<&Value>) -> WatcherConfig {
    let mut cfg = WatcherConfig::default();
    let Some(options) = options else { return cfg };

    if let Some(enabled) = lookup_bool(options, &["loctree", "watcher", "enabled"]) {
        cfg.enabled = enabled;
    }
    if let Some(ms) = lookup_u64(options, &["loctree", "watcher", "debounceMs"]) {
        cfg.debounce = Duration::from_millis(ms);
    }
    if let Some(patterns) = lookup_string_array(options, &["loctree", "watcher", "includePatterns"])
    {
        cfg.include_patterns = patterns;
    }
    if let Some(patterns) = lookup_string_array(options, &["loctree", "watcher", "excludePatterns"])
    {
        cfg.exclude_patterns = patterns;
    }
    cfg
}

/// Decide whether a filesystem path should trigger a rescan.
///
/// Excludes always win over includes; an empty include list means
/// "include everything". Pattern matching is suffix-based for now —
/// a true glob layer can come later if any of the rejected paths show
/// up in agent traces.
pub fn should_trigger_rescan(path: &Path, cfg: &WatcherConfig) -> bool {
    let path_str = path.to_string_lossy();
    if cfg
        .exclude_patterns
        .iter()
        .any(|pat| path_str.contains(pat.as_str()))
    {
        return false;
    }
    if cfg.include_patterns.is_empty() {
        return true;
    }
    cfg.include_patterns
        .iter()
        .any(|pat| path_str.contains(pat.as_str()))
}

fn lookup_bool(value: &Value, path: &[&str]) -> Option<bool> {
    lookup_nested(value, path)
        .or_else(|| lookup_flat(value, path))
        .and_then(|v| v.as_bool())
}

fn lookup_u64(value: &Value, path: &[&str]) -> Option<u64> {
    lookup_nested(value, path)
        .or_else(|| lookup_flat(value, path))
        .and_then(|v| v.as_u64())
}

fn lookup_string_array(value: &Value, path: &[&str]) -> Option<Vec<String>> {
    lookup_nested(value, path)
        .or_else(|| lookup_flat(value, path))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
}

fn lookup_nested<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(key)?;
    }
    Some(cursor)
}

fn lookup_flat<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let flat_key = path.join(".");
    value.get(&flat_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn config_defaults_when_no_options() {
        let cfg = config_from_options(None);
        assert!(cfg.enabled);
        assert_eq!(cfg.debounce, Duration::from_millis(DEFAULT_DEBOUNCE_MS));
        assert!(cfg.include_patterns.is_empty());
        assert!(cfg.exclude_patterns.is_empty());
    }

    #[test]
    fn config_reads_nested_options() {
        let opts = json!({
            "loctree": {
                "watcher": {
                    "enabled": false,
                    "debounceMs": 500,
                    "includePatterns": ["src/", "tests/"],
                    "excludePatterns": ["target/", ".git/"]
                }
            }
        });
        let cfg = config_from_options(Some(&opts));
        assert!(!cfg.enabled);
        assert_eq!(cfg.debounce, Duration::from_millis(500));
        assert_eq!(cfg.include_patterns, vec!["src/", "tests/"]);
        assert_eq!(cfg.exclude_patterns, vec!["target/", ".git/"]);
    }

    #[test]
    fn config_reads_flat_keys() {
        let opts = json!({
            "loctree.watcher.enabled": false,
            "loctree.watcher.debounceMs": 250
        });
        let cfg = config_from_options(Some(&opts));
        assert!(!cfg.enabled);
        assert_eq!(cfg.debounce, Duration::from_millis(250));
    }

    #[test]
    fn config_falls_back_to_defaults_on_missing_keys() {
        let opts = json!({ "loctree": { "other": { "thing": 1 } } });
        let cfg = config_from_options(Some(&opts));
        assert!(cfg.enabled);
        assert_eq!(cfg.debounce, Duration::from_millis(DEFAULT_DEBOUNCE_MS));
    }

    #[test]
    fn should_trigger_rescan_includes_all_when_no_filters() {
        let cfg = WatcherConfig::default();
        assert!(should_trigger_rescan(&PathBuf::from("src/lib.rs"), &cfg));
        assert!(should_trigger_rescan(
            &PathBuf::from("docs/SHIPPING.md"),
            &cfg
        ));
    }

    #[test]
    fn should_trigger_rescan_respects_exclude() {
        let cfg = WatcherConfig {
            exclude_patterns: vec!["target/".into(), ".git/".into()],
            ..Default::default()
        };
        assert!(!should_trigger_rescan(
            &PathBuf::from("target/debug/loctree"),
            &cfg
        ));
        assert!(!should_trigger_rescan(&PathBuf::from(".git/HEAD"), &cfg));
        assert!(should_trigger_rescan(&PathBuf::from("src/lib.rs"), &cfg));
    }

    #[test]
    fn should_trigger_rescan_respects_include_when_set() {
        let cfg = WatcherConfig {
            include_patterns: vec!["src/".into()],
            ..Default::default()
        };
        assert!(should_trigger_rescan(&PathBuf::from("src/lib.rs"), &cfg));
        assert!(!should_trigger_rescan(
            &PathBuf::from("scripts/build.sh"),
            &cfg
        ));
    }

    #[test]
    fn should_trigger_rescan_exclude_wins_over_include() {
        let cfg = WatcherConfig {
            include_patterns: vec!["src/".into()],
            exclude_patterns: vec!["src/generated/".into()],
            ..Default::default()
        };
        assert!(should_trigger_rescan(&PathBuf::from("src/lib.rs"), &cfg));
        assert!(!should_trigger_rescan(
            &PathBuf::from("src/generated/api.rs"),
            &cfg
        ));
    }

    #[test]
    fn scan_phase_strings_match_protocol_contract() {
        assert_eq!(ScanPhase::Scanning.as_str(), "scanning");
        assert_eq!(ScanPhase::Composing.as_str(), "composing");
        assert_eq!(ScanPhase::Done.as_str(), "done");
        assert_eq!(ScanPhase::Failed.as_str(), "failed");
    }

    #[test]
    fn scan_progress_serializes_minimal_shape() {
        let progress = ScanProgress::phase_only(ScanPhase::Scanning);
        let json = serde_json::to_value(&progress).expect("scan progress serializes");
        let obj = json.as_object().expect("scan progress is an object");
        // Optional fields with None must not appear in JSON.
        assert!(!obj.contains_key("eta_seconds"));
        assert!(!obj.contains_key("message"));
        assert_eq!(obj["phase"], json!("scanning"));
    }

    #[test]
    fn scan_progress_with_counts_serializes_real_totals() {
        let progress = ScanProgress::with_counts(
            ScanPhase::Composing,
            ScanStats {
                files_processed: 42,
                total_files: 42,
            },
        );
        let json = serde_json::to_value(&progress).expect("counted scan progress serializes");
        assert_eq!(json["phase"], json!("composing"));
        assert_eq!(json["files_processed"], json!(42));
        assert_eq!(json["total_files"], json!(42));
        assert!(json.get("eta_seconds").is_none());
        assert!(json.get("message").is_none());
    }

    #[test]
    fn scan_progress_failed_includes_message() {
        let progress = ScanProgress::failed("scan timed out");
        let json = serde_json::to_value(&progress).expect("failed scan progress serializes");
        assert_eq!(json["phase"], json!("failed"));
        assert_eq!(json["message"], json!("scan timed out"));
    }

    #[test]
    fn loctree_scan_progress_method_name_matches_contract() {
        assert_eq!(LoctreeScanProgress::METHOD, "loctree/scanProgress");
    }

    #[test]
    fn w4_05_lsp_rescan_backs_off_on_enospc() {
        let t0 = Instant::now();
        let mut backoff = RescanBackoff::default();
        assert!(backoff.allow(t0));
        assert!(backoff.deferred_progress(t0).is_none());

        let enospc = std::io::Error::new(
            std::io::ErrorKind::StorageFull,
            "ENOSPC: No space left on device",
        );
        assert_eq!(
            RescanFailureReason::from_io(&enospc),
            RescanFailureReason::Enospc
        );
        #[cfg(unix)]
        {
            let raw = std::io::Error::from_raw_os_error(libc::ENOSPC);
            assert_eq!(
                RescanFailureReason::from_io(&raw),
                RescanFailureReason::Enospc
            );
            assert_eq!(
                RescanFailureReason::from_error(&raw),
                RescanFailureReason::Enospc
            );
        }

        let mut delays = Vec::new();
        let mut now = t0;
        for _ in 0..3 {
            assert!(
                backoff.allow(now),
                "rescan must be eligible when recording the next ENOSPC failure"
            );
            let delay = backoff.record_failure(now, RescanFailureReason::from_io(&enospc));
            delays.push(delay);

            let progress = backoff.failure_progress(enospc.to_string());
            assert_eq!(progress.reason.as_deref(), Some("enospc"));
            let message = progress.message.as_deref().unwrap_or_default();
            assert!(
                message.contains("enospc"),
                "failed diagnostics must name enospc, got {message:?}"
            );

            assert!(
                !backoff.allow(now),
                "immediate retry after ENOSPC must be deferred"
            );
            let deferred = backoff
                .deferred_progress(now)
                .expect("deferred diagnostic while backoff is in force");
            assert_eq!(deferred.reason.as_deref(), Some("enospc"));
            assert!(
                deferred
                    .message
                    .as_deref()
                    .is_some_and(|m| m.contains("enospc")),
                "deferred diagnostics must name enospc, got {:?}",
                deferred.message
            );
            now += delay;
        }

        assert_eq!(delays[0], RescanBackoff::BASE_DELAY);
        assert_eq!(delays[1], RescanBackoff::BASE_DELAY.saturating_mul(2));
        assert_eq!(delays[2], RescanBackoff::BASE_DELAY.saturating_mul(4));
        assert!(
            delays[1] > delays[0] && delays[2] > delays[1],
            "backoff must grow across three ENOSPC failures, got {delays:?}"
        );

        backoff.reset();
        assert!(backoff.allow(now));
        assert_eq!(backoff.consecutive_failures(), 0);
        assert!(backoff.last_reason().is_none());
        assert!(backoff.deferred_progress(now).is_none());

        let other = std::io::Error::other("permission denied");
        assert_eq!(
            RescanFailureReason::from_io(&other),
            RescanFailureReason::Other
        );
        backoff.record_failure(now, RescanFailureReason::from_io(&other));
        let other_progress = backoff.failure_progress(other.to_string());
        assert_eq!(other_progress.reason.as_deref(), Some("other"));
        assert!(
            !other_progress
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("enospc"),
            "non-ENOSPC diagnostics must not be named enospc, got {:?}",
            other_progress.message
        );
    }
}
