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
    // D18: every emitted top-level family spends the same global budget.
    let twins_returned = array_len(&twins, "/dead_parrots")
        + array_len(&twins, "/exact_twins")
        + array_len(&twins, "/route_twins")
        + array_len(&twins, "/barrel_chaos/missing_barrels")
        + array_len(&twins, "/barrel_chaos/deep_chains")
        + array_len(&twins, "/barrel_chaos/inconsistent_paths");
    assert!(
        twins_returned <= 2,
        "all families must share --limit: {twins}"
    );
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

const TWINS_FAMILIES: [&str; 6] = [
    "/exact_twins",
    "/route_twins",
    "/dead_parrots",
    "/barrel_chaos/missing_barrels",
    "/barrel_chaos/deep_chains",
    "/barrel_chaos/inconsistent_paths",
];

fn twin_rows(value: &Value) -> Vec<(String, Value)> {
    TWINS_FAMILIES
        .iter()
        .flat_map(|path| {
            value
                .pointer(path)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(move |item| (path.to_string(), item.clone()))
        })
        .collect()
}

fn pagination_fixture(with_twins: bool) -> (tempfile::TempDir, tempfile::TempDir) {
    let project = tempfile::tempdir().expect("project");
    let cache = tempfile::tempdir().expect("cache");
    let src = project.path().join("src");
    fs::create_dir_all(src.join("parts")).expect("parts");
    for i in 0..3 {
        fs::write(src.join(format!("parts/p{i}.ts")), format!(
            "export function used_{i}() {{ return {i}; }}\nexport function unused_{i}() {{ return {i}; }}\n"
        )).expect("part");
        fs::write(
            src.join(format!("consumer_{i}.ts")),
            format!("import {{ used_{i} }} from './parts/p{i}';\nused_{i}();\n"),
        )
        .expect("consumer");
    }
    for i in 0..14 {
        fs::write(
            src.join(format!("orphan_{i}.ts")),
            format!("export function orphan_{i}() {{ return {i}; }}\n"),
        )
        .expect("orphan");
    }
    fs::create_dir_all(project.path().join("tests")).expect("tests");
    fs::write(
        project.path().join("tests/extra.ts"),
        "export function only_in_tests() {}\n",
    )
    .expect("test source");
    if with_twins {
        fs::create_dir_all(src.join("barrel/level1/level2")).expect("barrel chain");
        fs::write(
            src.join("barrel/target.ts"),
            "export const chain_target = 42;\n",
        )
        .expect("chain target");
        for (file, target) in [
            ("barrel/index.ts", "./level1"),
            ("barrel/level1/index.ts", "./level2"),
            ("barrel/level1/level2/index.ts", "../../target"),
        ] {
            fs::write(
                src.join(file),
                format!("export {{ chain_target }} from '{target}';\n"),
            )
            .expect("reexport");
        }
        for (i, target) in [
            "./barrel/target",
            "./barrel",
            "./barrel/level1",
            "./barrel/level1/level2",
        ]
        .iter()
        .enumerate()
        {
            for j in 0..2 {
                fs::write(
                    src.join(format!("path_{i}_{j}.ts")),
                    format!(
                        "import {{ chain_target }} from '{target}';\nconsole.log(chain_target);\n"
                    ),
                )
                .expect("alternative path");
            }
        }
        for i in 0..3 {
            fs::write(
                src.join(format!("twin_{i}.ts")),
                "export function duplicate() { return 1; }\n",
            )
            .expect("twin");
            fs::write(src.join(format!("route_{i}.py")), format!(
                "from fastapi import FastAPI\napp = FastAPI()\n@app.get('/same')\ndef handler_{i}():\n    return {i}\n"
            )).expect("route");
        }
    }
    (project, cache)
}

fn assert_twins_pages(root: &Path, cache: &Path, filters: &[&str]) -> Value {
    let mut args = vec!["twins", "--json"];
    args.extend_from_slice(filters);
    let (full, _) = json_output(root, cache, &args);
    let expected = twin_rows(&full);
    let total = expected.len();
    assert!(total > 2, "fixture census: {full}");
    let mut joined = Vec::new();
    for offset in (0..total).step_by(2) {
        let offset_arg = offset.to_string();
        let mut paged_args = args.clone();
        paged_args.extend_from_slice(&["--limit", "2", "--offset", &offset_arg]);
        let (page, _) = json_output(root, cache, &paged_args);
        let rows = twin_rows(&page);
        assert!(rows.len() <= 2, "global limit: {page}");
        assert_eq!(page["page"]["returned"], rows.len());
        assert_eq!(page["page"]["total"], total);
        assert_eq!(page["page"]["has_more"], offset + rows.len() < total);
        // Nested arrays have an independent bound. Restore them from the full
        // census after checking omission metadata, then compare actual findings.
        for (family, mut item) in rows {
            let index = joined.len();
            for field in ["locations", "chain", "alternative_paths"] {
                if let Some(items) = item[field].as_array() {
                    let all = expected[index].1[field]
                        .as_array()
                        .expect("full nested array");
                    assert_eq!(items, &all[..all.len().min(2)]);
                    assert_eq!(item[format!("{field}_total")], all.len());
                    assert_eq!(
                        item[format!("{field}_omitted")],
                        all.len().saturating_sub(2)
                    );
                    item[field] = expected[index].1[field].clone();
                    item[format!("{field}_omitted")] = 0.into();
                    item[format!("{field}_truncated")] = false.into();
                }
            }
            joined.push((family, item));
        }
    }
    assert_eq!(joined, expected, "pages must reconstruct the exact census");
    let (repeat, _) = json_output(root, cache, &args);
    assert_eq!(
        twin_rows(&repeat),
        expected,
        "stable ordering across processes"
    );
    for offset in [total, total + 1, 9999] {
        let offset_arg = offset.to_string();
        let mut paged_args = args.clone();
        paged_args.extend_from_slice(&["--limit", "2", "--offset", &offset_arg]);
        let (page, _) = json_output(root, cache, &paged_args);
        assert!(twin_rows(&page).is_empty(), "out of range: {page}");
        assert_eq!(page["page"]["has_more"], false);
        assert_eq!(page["page"]["total"], total);
    }
    let mut zero_args = args.clone();
    zero_args.extend_from_slice(&["--limit", "0"]);
    let (zero, _) = json_output(root, cache, &zero_args);
    assert!(twin_rows(&zero).is_empty());
    assert_eq!(zero["page"]["returned"], 0);
    assert_eq!(zero["page"]["total"], total);
    assert_eq!(zero["page"]["has_more"], true);
    let mut tail_args = args;
    tail_args.extend_from_slice(&["--offset", "1"]);
    let (tail, _) = json_output(root, cache, &tail_args);
    assert_eq!(twin_rows(&tail), expected[1..]);
    assert_eq!(tail["page"]["has_more"], false);
    full
}

#[test]
fn twins_pages_cross_families_and_preserve_nested_census() {
    let (project, cache) = pagination_fixture(true);
    let full = assert_twins_pages(project.path(), cache.path(), &[]);
    for family in [
        "/exact_twins",
        "/route_twins",
        "/dead_parrots",
        "/barrel_chaos/missing_barrels",
    ] {
        assert!(array_len(&full, family) > 0, "nonempty {family}: {full}");
    }
    let rows = twin_rows(&full);
    for (family, field) in [
        ("/exact_twins", "locations"),
        ("/route_twins", "locations"),
        ("/barrel_chaos/deep_chains", "chain"),
        ("/barrel_chaos/inconsistent_paths", "alternative_paths"),
    ] {
        let (offset, (_, item)) = rows
            .iter()
            .enumerate()
            .find(|(_, (path, item))| {
                path == family && item[field].as_array().is_some_and(|items| items.len() > 1)
            })
            .unwrap_or_else(|| panic!("fixture needs nested {family}/{field}: {full}"));
        let total = item[field].as_array().expect("nested").len();
        let offset_arg = offset.to_string();
        let (page, _) = json_output(
            project.path(),
            cache.path(),
            &["twins", "--json", "--limit", "1", "--offset", &offset_arg],
        );
        let actual = &twin_rows(&page)[0].1;
        assert_eq!(actual[format!("{field}_total")], total);
        assert_eq!(actual[format!("{field}_omitted")], total - 1);
        assert_eq!(page["page"]["nested_items_omitted"], total - 1);
        assert_eq!(page["page"]["nested_truncated"], true);
    }
    assert_twins_pages(project.path(), cache.path(), &["--class", "naming"]);
    assert_twins_pages(project.path(), cache.path(), &["--dead-only"]);
    let (follow, _) = json_output(
        project.path(),
        cache.path(),
        &["follow", "twins", "--json", "--limit", "2"],
    );
    assert_eq!(array_len(&follow, "/dead_parrots"), 0);
    assert!(twin_rows(&follow).len() <= 2);
    assert_eq!(
        follow["page"]["total"].as_u64().expect("total") as usize,
        twin_rows(&full).len() - array_len(&full, "/dead_parrots")
    );
}

#[test]
fn twins_dead_and_barrel_only_pages_terminate_without_exact_twins() {
    let (project, cache) = pagination_fixture(false);
    let full = assert_twins_pages(project.path(), cache.path(), &[]);
    assert_eq!(array_len(&full, "/exact_twins"), 0);
    assert_eq!(array_len(&full, "/route_twins"), 0);
    assert!(array_len(&full, "/dead_parrots") > 10);
    assert!(array_len(&full, "/barrel_chaos/missing_barrels") > 0);

    let (included, _) = json_output(
        project.path(),
        cache.path(),
        &["twins", "--json", "--include-tests"],
    );
    assert!(
        included["dead_parrots"]
            .as_array()
            .expect("dead")
            .iter()
            .any(|v| v["name"] == "only_in_tests")
    );
    let (excluded, _) = json_output(project.path(), cache.path(), &["twins", "--json"]);
    assert!(
        !excluded["dead_parrots"]
            .as_array()
            .expect("dead")
            .iter()
            .any(|v| v["name"] == "only_in_tests")
    );

    loct(project.path(), cache.path())
        .args(["suppress", "dead_parrot", "orphan_0"])
        .assert()
        .success();
    let suppressed = assert_twins_pages(project.path(), cache.path(), &[]);
    assert!(
        !suppressed["dead_parrots"]
            .as_array()
            .expect("dead")
            .iter()
            .any(|v| v["name"] == "orphan_0")
    );
    let unsuppressed = assert_twins_pages(project.path(), cache.path(), &["--include-suppressed"]);
    assert!(
        unsuppressed["dead_parrots"]
            .as_array()
            .expect("dead")
            .iter()
            .any(|v| v["name"] == "orphan_0")
    );
}
