//! Semantic acceptance harness: measures dead_parrots/twins/findings noise
//! before vs. after semantic facts are wired into the report layer.
//!
//! All assertions are live (no `#[ignore]`): the harness gates idiom
//! suppression, shell-dispatch reachability, and `.PHONY` Makefile
//! metadata preservation on every run.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
struct FindingsCounts {
    dead_parrots: u32,
    dead_parrots_high_confidence: u32,
    duplicate_groups: u32,
    idiom_suppressed: u32,
    make_metadata_suppressed: u32,
    shell_reachable_by_dispatch: u32,
}

fn run_findings(fixture_dir: &Path) -> FindingsCounts {
    // Run `loct --fresh findings --json` against the fixture and parse counts.
    let output = Command::new(env!("CARGO_BIN_EXE_loct"))
        .args(["--fresh", "findings", "--json"])
        .current_dir(fixture_dir)
        .output()
        .expect("loct execution failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("findings json parse");

    FindingsCounts {
        dead_parrots: v
            .pointer("/dead_parrots")
            .and_then(|x| x.as_array())
            .map(|a| a.len() as u32)
            .unwrap_or(0),
        dead_parrots_high_confidence: v
            .pointer("/dead_parrots")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter(|x| {
                        x.pointer("/confidence")
                            .and_then(|c| c.as_str())
                            .map(|s| s == "very-high" || s == "high")
                            .unwrap_or(false)
                    })
                    .count() as u32
            })
            .unwrap_or(0),
        duplicate_groups: v
            .pointer("/duplicate_groups")
            .and_then(|x| x.as_array())
            .map(|a| a.len() as u32)
            .unwrap_or(0),
        idiom_suppressed: v
            .pointer("/idiom_suppressed")
            .and_then(|x| x.as_u64())
            .map(|n| n as u32)
            .unwrap_or(0),
        make_metadata_suppressed: v
            .pointer("/make_metadata_suppressed")
            .and_then(|x| x.as_u64())
            .map(|n| n as u32)
            .unwrap_or(0),
        shell_reachable_by_dispatch: v
            .pointer("/shell_reachable_by_dispatch")
            .and_then(|x| x.as_u64())
            .map(|n| n as u32)
            .unwrap_or(0),
    }
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn benchmark_can_run() {
    // Smoke test: harness can execute on shell_rich and produce non-zero counts.
    // Proves the harness is wired correctly.
    let counts = run_findings(&fixture_path("shell_rich"));
    eprintln!("shell_rich counts: {:?}", counts);
    // Expect SOME findings (the fixture is intentionally noisy at T4 baseline).
    assert!(
        counts.dead_parrots > 0 || counts.duplicate_groups > 0,
        "fixture must produce baseline findings before semantic suppression"
    );
}

#[test]
fn shell_rich_noise_drops_after_semantic() {
    let counts = run_findings(&fixture_path("shell_rich"));

    // Acceptance: idiom-only helpers (usage, die, _info, _warn, etc.) must be suppressed.
    assert!(
        counts.idiom_suppressed > 0,
        "expected idiom_suppressed > 0, got {}",
        counts.idiom_suppressed
    );

    // Acceptance: case-dispatch handlers must be marked reached.
    assert!(
        counts.shell_reachable_by_dispatch > 0,
        "expected shell_reachable_by_dispatch > 0, got {}",
        counts.shell_reachable_by_dispatch
    );

    // Acceptance: high-confidence dead_parrots count should drop materially.
    // Threshold to be set by T3 owner based on baseline benchmark.
    // assert!(counts.dead_parrots_high_confidence <= EXPECTED_THRESHOLD);
}

#[test]
fn make_rich_metadata_not_dead() {
    let counts = run_findings(&fixture_path("make_rich"));

    assert!(
        counts.make_metadata_suppressed > 0,
        "expected .PHONY targets suppressed, got {}",
        counts.make_metadata_suppressed
    );
}

#[test]
fn bench_emit_json() {
    // Optional: when run with --nocapture, write a comparable JSON to disk.
    let counts = run_findings(&fixture_path("shell_rich"));
    let path = std::env::temp_dir().join(format!(
        "semantic-bench-{}.json",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    ));
    std::fs::write(&path, serde_json::to_string_pretty(&counts).unwrap())
        .expect("write bench json");
    eprintln!("bench output: {}", path.display());
}
