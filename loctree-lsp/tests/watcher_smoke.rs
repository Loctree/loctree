//! Integration test for the background watcher (Plan 10).
//!
//! A true LSP-stdio smoke test (spawn the binary, drive JSON-RPC,
//! assert `loctree/scanProgress` arrives) is captured as a follow-up
//! in the plan report — it requires a test harness around the LSP
//! wire that doesn't yet exist in this crate. The tests below cover
//! the watcher-side logic that survives independent of a Client:
//!
//! - JSON shape of `loctree/scanProgress` payloads
//! - `WatcherConfig` parsing of realistic `initializationOptions`
//! - `should_trigger_rescan` exclusion semantics on real-world paths
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::path::PathBuf;
use std::time::Duration;

use loctree_lsp::{
    LoctreeScanProgress, ScanPhase, ScanProgress, ScanStats, WatcherConfig, config_from_options,
    should_trigger_rescan,
};
use tower_lsp::lsp_types::notification::Notification;

#[test]
fn watcher_method_name_is_loctree_scan_progress() {
    assert_eq!(LoctreeScanProgress::METHOD, "loctree/scanProgress");
}

#[test]
fn scan_progress_done_payload_shape() {
    let progress = ScanProgress::phase_only(ScanPhase::Done);
    let json = serde_json::to_value(&progress).expect("done progress serializes");
    assert_eq!(json["phase"], serde_json::json!("done"));
    assert_eq!(json["files_processed"], serde_json::json!(0));
    assert_eq!(json["total_files"], serde_json::json!(0));
    assert!(json.get("eta_seconds").is_none());
    assert!(json.get("message").is_none());
}

#[test]
fn scan_progress_counted_payload_shape() {
    let progress = ScanProgress::with_counts(
        ScanPhase::Done,
        ScanStats {
            files_processed: 50,
            total_files: 50,
        },
    );
    let json = serde_json::to_value(&progress).expect("counted progress serializes");
    assert_eq!(json["phase"], serde_json::json!("done"));
    assert_eq!(json["files_processed"], serde_json::json!(50));
    assert_eq!(json["total_files"], serde_json::json!(50));
    assert!(json.get("eta_seconds").is_none());
    assert!(json.get("message").is_none());
}

#[test]
fn scan_progress_failed_carries_message() {
    let progress = ScanProgress::failed("disk full");
    let json = serde_json::to_value(&progress).expect("failed progress serializes");
    assert_eq!(json["phase"], serde_json::json!("failed"));
    assert_eq!(json["message"], serde_json::json!("disk full"));
}

#[test]
fn config_parses_realistic_initialization_options() {
    let opts = serde_json::json!({
        "loctree": {
            "watcher": {
                "enabled": true,
                "debounceMs": 500,
                "includePatterns": ["src/", "tests/"],
                "excludePatterns": ["target/", ".git/", "node_modules/"]
            }
        }
    });
    let cfg = config_from_options(Some(&opts));
    assert!(cfg.enabled);
    assert_eq!(cfg.debounce, Duration::from_millis(500));
    assert_eq!(cfg.include_patterns, vec!["src/", "tests/"]);
    assert_eq!(
        cfg.exclude_patterns,
        vec!["target/", ".git/", "node_modules/"]
    );
}

#[test]
fn config_disabled_via_init_options() {
    let opts = serde_json::json!({
        "loctree": { "watcher": { "enabled": false } }
    });
    let cfg = config_from_options(Some(&opts));
    assert!(!cfg.enabled);
}

#[test]
fn rescan_filter_skips_target_and_git_dirs() {
    let cfg = WatcherConfig {
        exclude_patterns: vec!["target/".into(), ".git/".into(), "node_modules/".into()],
        ..Default::default()
    };
    let cases = [
        ("target/debug/loctree", false),
        ("target/release/incremental", false),
        (".git/objects/aa/bbcc", false),
        ("node_modules/some-pkg/index.js", false),
        ("src/lib.rs", true),
        ("loctree-lsp/src/watcher.rs", true),
        ("docs/SHIPPING.md", true),
    ];
    for (raw, expected) in cases {
        let path = PathBuf::from(raw);
        assert_eq!(
            should_trigger_rescan(&path, &cfg),
            expected,
            "rescan filter mismatched for {raw}"
        );
    }
}

#[test]
fn rescan_filter_with_include_only() {
    let cfg = WatcherConfig {
        include_patterns: vec!["src/".into(), "tests/".into()],
        ..Default::default()
    };
    assert!(should_trigger_rescan(&PathBuf::from("src/main.rs"), &cfg));
    assert!(should_trigger_rescan(&PathBuf::from("tests/e2e.rs"), &cfg));
    assert!(!should_trigger_rescan(&PathBuf::from("Cargo.toml"), &cfg));
    assert!(!should_trigger_rescan(
        &PathBuf::from("docs/notes.md"),
        &cfg
    ));
}
