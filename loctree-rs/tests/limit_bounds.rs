use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::Path;

fn loct(root: &Path, cache: &Path) -> Command {
    let mut command = Command::cargo_bin("loct").expect("loct binary");
    command
        .current_dir(root)
        .env("LOCT_CACHE_DIR", cache)
        .env("NO_COLOR", "1")
        // Fixtures live in TMPDIR, outside any git checkout; the scan guard
        // (snapshot.rs) documents this env var as its test-side counterpart.
        .env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1");
    command
}

fn json_output(root: &Path, cache: &Path, args: &[&str]) -> (Value, usize) {
    let output = loct(root, cache)
        .args(args)
        .assert()
        .success()
        .get_output()
        .clone();
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "expected JSON ({error}); stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (value, output.stdout.len())
}

fn array_len(value: &Value, pointer: &str) -> usize {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn symbol_match_len(value: &Value) -> usize {
    value
        .pointer("/symbol_matches/files")
        .and_then(Value::as_array)
        .map_or(0, |files| {
            files.iter().map(|file| array_len(file, "/matches")).sum()
        })
}

#[test]
fn aggregate_limit_is_global_and_payloads_stay_bounded() {
    let project = tempfile::tempdir().expect("project tempdir");
    let cache = tempfile::tempdir().expect("cache tempdir");
    let src = project.path().join("src");
    fs::create_dir_all(&src).expect("create src");
    fs::write(
        src.join("a.ts"),
        r#"
import { emit, listen } from '@tauri-apps/api/event';
export function duplicate() { return 1; }
export function duplicate_two() { return 11; }
export function duplicate_three() { return 12; }
export function alpha() { return 2; }
emit('event-one', {}); listen('event-one', () => {});
emit('event-two', {}); listen('event-two', () => {});
emit('event-three', {}); listen('event-three', () => {});
"#,
    )
    .expect("write a.ts");
    fs::write(
        src.join("b.ts"),
        r#"
export function duplicate() { return 3; }
export function duplicate_two() { return 13; }
export function duplicate_three() { return 14; }
export function beta() { return 4; }
export function gamma() { return 5; }
"#,
    )
    .expect("write b.ts");

    let (twins, twins_bytes) = json_output(
        project.path(),
        cache.path(),
        &["twins", "--limit", "2", "--json", "."],
    );
    // Round-2 N3 contract: `--limit` spends on the twin families (exact,
    // then route). Dead parrots sit outside that budget under their own
    // DEAD_PARROTS_PAGE_CAP, and each barrel list is capped at the limit.
    let twin_family_returned =
        array_len(&twins, "/exact_twins") + array_len(&twins, "/route_twins");
    assert!(
        twin_family_returned <= 2,
        "twin families exceeded --limit: {twins}"
    );
    assert!(
        array_len(&twins, "/dead_parrots") <= 10,
        "dead parrots exceeded their fixed page cap: {twins}"
    );
    for barrel_list in [
        "/barrel_chaos/missing_barrels",
        "/barrel_chaos/deep_chains",
        "/barrel_chaos/inconsistent_paths",
    ] {
        assert!(
            array_len(&twins, barrel_list) <= 2,
            "{barrel_list} exceeded --limit: {twins}"
        );
    }
    let twins_returned = array_len(&twins, "/dead_parrots")
        + array_len(&twins, "/exact_twins")
        + array_len(&twins, "/route_twins")
        + array_len(&twins, "/barrel_chaos/missing_barrels")
        + array_len(&twins, "/barrel_chaos/deep_chains")
        + array_len(&twins, "/barrel_chaos/inconsistent_paths");
    assert_eq!(twins["page"]["semantics"], "global");
    assert_eq!(twins["page"]["returned"], twins_returned);
    assert!(
        twins["page"]["total"].as_u64().unwrap_or(0) as usize > twins_returned,
        "fixture must exercise twin-family truncation: {twins}"
    );
    assert_eq!(twins["page"]["has_more"], true);
    assert!(
        twins_bytes < 24_000,
        "twins payload was {twins_bytes} bytes"
    );

    let (events, events_bytes) = json_output(
        project.path(),
        cache.path(),
        &["events", "--limit", "2", "--json", "."],
    );
    let events_returned =
        array_len(&events, "/event_bridges") + array_len(&events, "/symbol_events");
    assert!(
        events_returned <= 2,
        "events exceeded global limit: {events}"
    );
    assert_eq!(events["page"]["semantics"], "global");
    assert_eq!(events["page"]["returned"], events_returned);
    assert!(
        events["page"]["total"].as_u64().unwrap_or(0) > 2,
        "fixture must exercise truncation: {events}"
    );
    assert_eq!(events["page"]["has_more"], true);
    assert!(
        events_bytes < 24_000,
        "events payload was {events_bytes} bytes"
    );

    let (follow, follow_bytes) = json_output(
        project.path(),
        cache.path(),
        &["follow", "all", "--limit", "2", "--json", "."],
    );
    let follow_returned = array_len(&follow, "/dead/top")
        + array_len(&follow, "/cycles/cycles")
        + array_len(&follow, "/hotspots/top");
    assert!(
        follow_returned <= 2,
        "follow exceeded global limit: {follow}"
    );
    assert_eq!(follow["page"]["semantics"], "global");
    assert_eq!(follow["page"]["returned"], follow_returned);
    assert!(
        follow["page"]["total"].as_u64().unwrap_or(0) > 2,
        "fixture must exercise truncation: {follow}"
    );
    assert_eq!(follow["page"]["has_more"], true);
    assert!(
        follow_bytes < 24_000,
        "follow payload was {follow_bytes} bytes"
    );

    let (discover, discover_bytes) = json_output(
        project.path(),
        cache.path(),
        &[
            "find",
            "--discover",
            "duplicate|alpha|beta|gamma",
            "--limit",
            "2",
            "--json",
        ],
    );
    let discover_returned = symbol_match_len(&discover)
        + array_len(&discover, "/param_matches")
        + array_len(&discover, "/semantic_matches")
        + array_len(&discover, "/suppression_matches")
        + array_len(&discover, "/cross_matches")
        + array_len(&discover, "/dead_status/dead_in_files");
    assert!(
        discover_returned <= 2,
        "discover exceeded global limit: {discover}"
    );
    assert_eq!(discover["page"]["semantics"], "global");
    assert_eq!(discover["page"]["returned"], discover_returned);
    assert!(
        discover["page"]["total"].as_u64().unwrap_or(0) > 2,
        "fixture must exercise truncation: {discover}"
    );
    assert_eq!(discover["page"]["has_more"], true);
    assert_eq!(discover["total"], discover["page"]["total"]);
    assert_eq!(discover["emitted"], discover["page"]["returned"]);
    assert_eq!(discover["offset"], 0);
    assert_eq!(discover["truncated"], true);
    assert_eq!(discover["has_more"], true);
    assert_eq!(discover["universe"]["indexed_files"], 2);
    assert_eq!(discover["universe"]["scanned_files"], 2);
    for required in [
        "tracked",
        "untracked",
        "ignored",
        "generated",
        "fixtures",
        "exclusions",
    ] {
        assert!(
            discover["universe"].get(required).is_some(),
            "discover universe must declare {required}: {discover}"
        );
    }
    assert!(
        discover_bytes < 24_000,
        "discover payload was {discover_bytes} bytes"
    );

    let (unbounded_discover, _) = json_output(
        project.path(),
        cache.path(),
        &["find", "--discover", "duplicate", "--json"],
    );
    assert_eq!(
        unbounded_discover["total"], unbounded_discover["emitted"],
        "unbounded discovery must reconcile total and emitted"
    );
    assert_eq!(unbounded_discover["offset"], 0);
    assert_eq!(unbounded_discover["truncated"], false);
    assert_eq!(unbounded_discover["has_more"], false);
    assert_eq!(unbounded_discover["universe"]["indexed_files"], 2);
}
