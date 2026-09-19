//! Delivery verifiers for the Swift report-truth line.
//!
//! These tests are the truth boundary for the `loctree-swift-report-truth`
//! plan: they are committed on the baseline branch *before* the repair cuts
//! branch off it, so an isolated worker proves its fix against a contract it
//! did not write. Four of them describe the target state and are expected to
//! fail until their cut lands; `crowds_not_in_health_metrics` guards a
//! contract that is already correct.
//!
//! Every fixture is copied into a `TempDir`, turned into a git checkout and
//! scanned with a private cache. The `git init` is not ceremony: `loct scan`
//! refuses non-git directories, so a fixture that skips it fails on the
//! refusal instead of on the property under test.
//!
//! # Why the `zz_` prefix
//!
//! `cargo test -p loctree <filter>` runs every integration target of the
//! package in alphabetical order, and each one prints its own
//! `test result:` line — including the ones where the filter matched
//! nothing. The delivery gate for this line reads that output through
//! `tail -3`, so it sees the *last* target's summary, not this file's.
//! Sorting after `watch_lock_cli` is what makes the gate measure the fence
//! it names. Keep this target last, or pin the gate with
//! `--test zz_swift_report_truth` instead.
//!
//! `health_truth_gate_target_sorts_last` enforces that invariant, because
//! stating it here was not enough to stop a later cut from adding a target
//! that sorts past this one.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use loctree::analyzer::health_score::{HealthMetrics, calculate_health_score};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn fixtures_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Recursive copy that keeps file permissions — `scripts/*.sh` is only a
/// runtime entrypoint while it stays executable.
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .expect("git must be available to build a scannable fixture checkout");
    assert!(
        status.success(),
        "git {args:?} failed in {}",
        root.display()
    );
}

/// `loct` bound to one fixture root and one private cache.
///
/// `LOCT_NO_GITIGNORE=1` matters for measurement, not tidiness: without it the
/// first scan writes a `.gitignore` into the fixture copy and every file count
/// in this module shifts by one.
fn loct_at(root: &Path, cache: &Path) -> Command {
    let mut cmd = cargo_bin_cmd!("loct");
    cmd.current_dir(root)
        .env("LOCT_CACHE_DIR", cache)
        .env("LOCT_OPEN_BROWSER", "0")
        .env("LOCT_NO_GITIGNORE", "1");
    cmd
}

/// Isolated fixture checkout plus its private snapshot cache.
fn swift_fixture(name: &str) -> (TempDir, TempDir) {
    let temp = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    copy_dir_all(&fixtures_path().join(name), temp.path()).unwrap();
    git(
        temp.path(),
        &["-c", "init.defaultBranch=main", "init", "-q"],
    );
    git(temp.path(), &["add", "-A", "-f"]);
    loct_at(temp.path(), cache.path())
        .args(["scan", "--full-scan"])
        .assert()
        .success();
    (temp, cache)
}

fn json_of(root: &Path, cache: &Path, args: &[&str]) -> Value {
    let output = loct_at(root, cache)
        .args(args)
        .assert()
        .success()
        .get_output()
        .clone();
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("`loct {}` must emit JSON: {e}", args.join(" ")))
}

fn audit_json(root: &Path, cache: &Path) -> Value {
    json_of(root, cache, &["audit", "--json"])
}

fn findings_summary(root: &Path, cache: &Path) -> Value {
    json_of(root, cache, &["findings", "--summary"])
}

fn dead_json(root: &Path, cache: &Path) -> Vec<Value> {
    json_of(root, cache, &["dead", "--json", "--full"])
        .as_array()
        .expect("`loct dead --json` must emit an array")
        .clone()
}

fn orphan_paths(audit: &Value) -> Vec<String> {
    audit["orphan_files"]["files"]
        .as_array()
        .map(|files| {
            files
                .iter()
                .filter_map(|f| f["path"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn u64_at(value: &Value, pointer: &str) -> u64 {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("missing unsigned number at {pointer} in {value}"))
}

/// `(definitions, call sites)` as the literal scan sees them.
fn literal_shape(root: &Path, cache: &Path, symbol: &str) -> (u64, u64) {
    let found = json_of(root, cache, &["find", "--literal", symbol, "--json"]);
    let shape = &found["literal_matches"]["hit_shape"];
    let definitions = shape["definitions"].as_u64().unwrap_or(0);
    let call_sites =
        shape["readers"].as_u64().unwrap_or(0) + shape["writers"].as_u64().unwrap_or(0);
    (definitions, call_sites)
}

fn is_high_confidence(candidate: &Value) -> bool {
    matches!(
        candidate["confidence"].as_str(),
        Some("high") | Some("very-high")
    )
}

// ============================================================
// W1-02 — a file's role makes it a graph root
// ============================================================

/// RED until W1-02. An XCTest target and a shebang script are roots by role:
/// nothing imports a test bundle or a release script, and nothing ever will.
/// The orphan rule matches the lowercase `"/tests/"` substring only, so the
/// Swift/SPM `AppTests/` convention slips past it.
#[test]
fn role_root_files_are_never_orphans() {
    let (fixture, cache) = swift_fixture("swift_role_roots");
    let audit = audit_json(fixture.path(), cache.path());
    let orphans = orphan_paths(&audit);
    let total = u64_at(&audit, "/orphan_files/total");

    assert_eq!(
        total, 0,
        "expected orphan_files.total == 0 — every file in this fixture is a \
         graph root by role (`@main` app, XCTest target, shebang script) or has \
         a real importer; got {total}: {orphans:?}"
    );
}

// ============================================================
// W1-03 — a shared name is not a duplication
// ============================================================

/// RED until W1-03. `text`, `body` and `makeNSView` are a stored property and
/// two protocol witnesses in two independent views — the names are dictated by
/// SwiftUI and NSViewRepresentable. The `namesake` classification already
/// exists; the aggregation has to read it.
#[test]
fn namesake_groups_do_not_inflate_duplicate_groups() {
    let (fixture, cache) = swift_fixture("swift_namesake");
    let audit = audit_json(fixture.path(), cache.path());

    // The sensor must keep reporting what it sees. A cut that empties
    // `twins.groups` instead of teaching the aggregation to read the
    // classification is a regression, not a fix.
    let groups = audit["twins"]["groups"]
        .as_array()
        .expect("twins.groups must stay in the audit payload")
        .clone();
    let namesakes: Vec<&str> = groups
        .iter()
        .filter(|g| g["classification"].as_str() == Some("namesake"))
        .filter_map(|g| g["name"].as_str())
        .collect();
    assert_eq!(
        namesakes.len(),
        3,
        "the raw sensor must still expose all three namesake groups \
         (text, body, makeNSView); got {namesakes:?}"
    );

    let summary = findings_summary(fixture.path(), cache.path());
    let duplicate_groups = u64_at(&summary, "/duplicate_groups");
    assert_eq!(
        duplicate_groups,
        0,
        "expected duplicate_groups == 0 — all {} twin group(s) in this fixture \
         are classified `namesake` ({namesakes:?}) and none is duplicated logic; \
         got {duplicate_groups}",
        groups.len()
    );
}

// ============================================================
// W2-01 — the aggregate must agree with the repository
// ============================================================

/// RED until W2-01 (after W1-01/02/03). A seven-file Swift app with no dead
/// code, no cycles and no duplicated logic must not be scored as unhealthy.
/// All three components are reported together: asserting them one by one would
/// let the first failure hide the rest from the cut that has to fix them.
#[test]
fn health_truth_healthy_swift_repo_scores_at_least_ninety() {
    let (fixture, cache) = swift_fixture("swift_health_truth");
    let audit = audit_json(fixture.path(), cache.path());
    let summary = findings_summary(fixture.path(), cache.path());

    let files = u64_at(&audit, "/summary/total_files");
    assert_eq!(
        files, 7,
        "fixture shape guard: the health-truth probe is a seven-file app, got {files}"
    );

    let orphans = u64_at(&audit, "/orphan_files/total");
    let duplicate_groups = u64_at(&summary, "/duplicate_groups");
    let health = u64_at(&summary, "/health_score");

    let mut violations = Vec::new();
    if orphans != 0 {
        violations.push(format!(
            "orphan_files.total: expected 0, got {orphans} ({:?})",
            orphan_paths(&audit)
        ));
    }
    if duplicate_groups != 0 {
        violations.push(format!(
            "duplicate_groups: expected 0, got {duplicate_groups}"
        ));
    }
    if health < 90 {
        violations.push(format!("health_score: expected >= 90, got {health}"));
    }

    assert!(
        violations.is_empty(),
        "a healthy seven-file Swift app must report as healthy — {} unmet \
         expectation(s): {}",
        violations.len(),
        violations.join("; ")
    );
}

/// A dead candidate the score is allowed to read: the detector resolved it
/// (`high`/`very-high`) and it is not a declared runtime entry.
fn is_scored_dead(candidate: &Value) -> bool {
    is_high_confidence(candidate) && candidate["entrypoint"].as_bool() != Some(true)
}

/// The score this repository would carry with every severity dimension empty.
///
/// Comparing against this instead of the literal `100` keeps the assertions
/// weight-agnostic: `CERTAIN_WEIGHT` / `HIGH_WEIGHT` / `SMELL_WEIGHT` are the
/// operator's to calibrate, and these tests must not quietly pin them.
fn clean_score_for(summary: &Value) -> u64 {
    calculate_health_score(&HealthMetrics {
        files: u64_at(summary, "/files") as usize,
        loc: u64_at(summary, "/loc") as usize,
        ..HealthMetrics::default()
    })
    .health
    .expect("nonempty fixture must remain scorable") as u64
}

/// Unresolved and entry-point dead candidates stay on the sensor and off the score.
///
/// `confidence: low` is the detector saying it could not resolve the
/// references, not that there are none — in Swift it cannot, because `import`
/// is module-level. `entrypoint: true` marks a declaration that has no caller
/// by definition. Both belong on the reported list; neither is a defect.
#[test]
fn health_truth_unresolved_and_entrypoint_dead_stay_off_the_score() {
    let (fixture, cache) = swift_fixture("swift_health_truth");
    let dead = dead_json(fixture.path(), cache.path());
    let summary = findings_summary(fixture.path(), cache.path());

    // Premise: the sensor really did produce candidates here. Without this the
    // test would pass on an empty list and prove nothing.
    assert!(
        !dead.is_empty(),
        "fixture guard: the dead sensor must still list candidates for this \
         probe, otherwise the filter under test is never exercised"
    );
    let scored: Vec<&Value> = dead.iter().filter(|c| is_scored_dead(c)).collect();
    assert!(
        scored.is_empty(),
        "fixture guard: every candidate in this probe is unresolved or an \
         entry point; a resolved one means the fixture changed: {scored:?}"
    );

    // The sensor keeps every candidate ...
    assert_eq!(
        u64_at(&summary, "/dead_parrots"),
        dead.len() as u64,
        "the reported dead count must stay the raw sensor count — filtering \
         belongs to the score, never to the finding list"
    );

    // ... and the score counts none of them.
    assert_eq!(
        u64_at(&summary, "/health_score"),
        clean_score_for(&summary),
        "expected the score of a repository with no defects — {} unresolved \
         candidate(s) must not reach the HIGH dimension: {dead:?}",
        dead.len()
    );
}

/// Name collisions without shape evidence feed no severity dimension.
///
/// `text`, `body` and `makeNSView` are a stored property and two protocol
/// witnesses, repeated across two views because SwiftUI and NSViewRepresentable
/// require those names. `W1-03` classifies them; this asserts the aggregation
/// reads that classification on every path it feeds — SMELL through
/// `twins_same_language` / `duplicate_exports`, and HIGH through the twin
/// parrots, whose import count is zero for every declaration in a Swift module.
#[test]
fn health_truth_namesake_groups_feed_no_health_dimension() {
    let (fixture, cache) = swift_fixture("swift_health_truth");
    let audit = audit_json(fixture.path(), cache.path());
    let summary = findings_summary(fixture.path(), cache.path());

    let namesakes: Vec<&str> = audit["twins"]["groups"]
        .as_array()
        .expect("twins.groups must stay in the audit payload")
        .iter()
        .filter(|g| g["classification"].as_str() == Some("namesake"))
        .filter_map(|g| g["name"].as_str())
        .collect();
    assert_eq!(
        namesakes.len(),
        3,
        "fixture guard: the sensor must still expose all three shared names \
         (text, body, makeNSView); got {namesakes:?}"
    );

    assert_eq!(
        u64_at(&summary, "/duplicate_groups"),
        0,
        "a shared name is not a duplication: {namesakes:?}"
    );
    assert_eq!(
        u64_at(&summary, "/health_score"),
        clean_score_for(&summary),
        "expected the score of a repository with no defects — {} shapeless \
         collision(s) must not reach any severity dimension: {namesakes:?}",
        namesakes.len()
    );
}

/// Files that are graph roots by role feed no severity dimension.
///
/// The probe carries all three shapes: an XCTest target, a shebang script and a
/// document. Nothing imports them and nothing ever will, so their absence from
/// the import graph is their definition, not a defect.
#[test]
fn health_truth_role_roots_feed_no_health_dimension() {
    let (fixture, cache) = swift_fixture("swift_health_truth");
    let audit = audit_json(fixture.path(), cache.path());
    let summary = findings_summary(fixture.path(), cache.path());

    // Premise: all three roles are actually present and bucketed as roots.
    for bucket in ["test_orphans", "script_orphans", "doc_orphans"] {
        let total = u64_at(&audit, &format!("/{bucket}/total"));
        assert_eq!(
            total, 1,
            "fixture guard: the probe must carry exactly one {bucket} entry, \
             otherwise this test does not exercise that role; got {total}"
        );
    }
    let orphans = u64_at(&audit, "/orphan_files/total");
    assert_eq!(
        orphans,
        0,
        "role roots are roots, not orphans to review; got {orphans}: {:?}",
        orphan_paths(&audit)
    );

    // No dead candidate — the only defect input this probe can produce —
    // originates from a role-root file.
    let from_roles: Vec<String> = dead_json(fixture.path(), cache.path())
        .iter()
        .filter(|c| is_scored_dead(c))
        .filter_map(|c| c["file"].as_str())
        .filter(|f| f.starts_with("AppTests/") || f.starts_with("scripts/") || f.ends_with(".md"))
        .map(str::to_owned)
        .collect();
    assert!(
        from_roles.is_empty(),
        "a test target, a script and a document must not contribute scored \
         defects: {from_roles:?}"
    );

    assert_eq!(
        u64_at(&summary, "/health_score"),
        clean_score_for(&summary),
        "expected the score of a repository with no defects — three role roots \
         must not reach any severity dimension"
    );
}

/// Negative control, and the teeth of this whole section.
///
/// Every test above asserts that something stopped being counted. Without this
/// one they would all still pass if the HIGH dimension were simply deleted.
/// `dead_code` is a TypeScript fixture whose four candidates the detector fully
/// resolves — those must keep costing health, in any language.
#[test]
fn health_truth_resolved_dead_still_costs_health() {
    let (fixture, cache) = swift_fixture("dead_code");
    let dead = dead_json(fixture.path(), cache.path());
    let summary = findings_summary(fixture.path(), cache.path());

    let scored = dead.iter().filter(|c| is_scored_dead(c)).count();
    assert!(
        scored >= 3,
        "fixture guard: this probe exists to carry resolved dead exports; \
         got {scored} of {}: {dead:?}",
        dead.len()
    );

    let health = u64_at(&summary, "/health_score");
    let clean = clean_score_for(&summary);
    assert!(
        health < clean,
        "expected {scored} resolved dead export(s) to cost health — got \
         {health}, the same as a repository with no defects ({clean}). The \
         score must reject non-defects, not stop counting defects."
    );
}

// ============================================================
// W1-01 — dead-export and literal find must tell one story
// ============================================================

/// The invariant `W1-01` owns: no declaration may be high-confidence dead
/// while the literal scan resolves call sites for it. The fixture exercises
/// the Swift call shapes that plausibly hide a reference — an extension method
/// in a sibling file reached by implicit and explicit `self`, a cross-file call
/// through a stored property, and a symbol whose only callers live in the
/// capitalised `AppTests/` directory.
///
/// Measured on the baseline this holds already (see the W0-01 report): the two
/// surfaces agree on every shape probed. It is committed as a regression fence
/// so `W1-01` starts as reconnaissance with a contract to keep, not as a repair
/// of a symptom nobody has reproduced.
#[test]
fn dead_find_agreement_holds_for_swift_call_shapes() {
    let (fixture, cache) = swift_fixture("swift_find_agreement");
    let dead = dead_json(fixture.path(), cache.path());

    for symbol in [
        "retireRecoveryAssociation",
        "drop",
        "auditRecoveryAssociation",
    ] {
        let (definitions, call_sites) = literal_shape(fixture.path(), cache.path(), symbol);
        assert!(
            definitions >= 1 && call_sites >= 1,
            "fixture guard: `{symbol}` must have a definition and at least one \
             literal call site, got definitions={definitions} call_sites={call_sites}"
        );

        let promoted = dead
            .iter()
            .find(|c| c["symbol"].as_str() == Some(symbol) && is_high_confidence(c));
        assert!(
            promoted.is_none(),
            "expected `{symbol}` to be absent from high-confidence dead — the \
             literal scan resolves {call_sites} call site(s) for it; the two \
             surfaces must not disagree: {promoted:?}"
        );
    }

    // Negative control: whatever credit keeps the live methods off the
    // high-confidence list must not also mask a genuinely unreferenced one.
    let dormant = dead
        .iter()
        .find(|c| c["symbol"].as_str() == Some("dormantRecoveryHook"))
        .expect("the dormant control must stay a dead candidate");
    assert!(
        is_high_confidence(dormant),
        "expected `dormantRecoveryHook` to stay high-confidence dead — it has \
         no call site anywhere: {dormant:?}"
    );
}

/// Regression fence for `c69205db`, at the verdict surface rather than the probe.
///
/// The fence already has unit coverage, but only over
/// `probe_framework_dispatched_declaration` itself — nothing asserted that a
/// framework witness survives the whole `loct dead` pipeline undemoted. This
/// closes that gap end to end.
///
/// It also makes the `W1-01` delivery gate measure something. `cargo test -p
/// loctree swift_framework` matched a single unit test in the *lib* target,
/// which runs first; the gate reads the run through `tail -3` and therefore saw
/// the last target's summary — `ok. 0 passed`. A gate that filters into an empty
/// set is trivially true. Living here, in the alphabetically last target, is what
/// puts the fence under the command that names it.
///
/// The two negative controls are the real teeth: without them a fence that
/// silenced every declaration would pass.
#[test]
fn swift_framework_fence_holds_end_to_end() {
    let (fixture, cache) = swift_fixture("swift_framework_fence");
    let dead = dead_json(fixture.path(), cache.path());

    // Framework-dispatched shapes: reached by selector, protocol witness or the
    // superclass, so no Swift call site exists anywhere by design.
    for symbol in ["windowWillClose", "handleCloseTap", "awakeFromNib"] {
        let promoted = dead
            .iter()
            .find(|c| c["symbol"].as_str() == Some(symbol) && is_high_confidence(c));
        assert!(
            promoted.is_none(),
            "expected `{symbol}` to be absent from high-confidence dead — the \
             framework dispatches it, so `no references` is the shape of a \
             correct declaration, not evidence of death: {promoted:?}"
        );
    }

    // Negative controls: ordinary app code with no caller must stay a delete
    // candidate. One shares an extension with the fenced shapes, so a fence that
    // keyed on the enclosing declaration instead of the member would fail here.
    for symbol in ["loadUnusedThing", "plainDormantHelper"] {
        let candidate = dead
            .iter()
            .find(|c| c["symbol"].as_str() == Some(symbol))
            .unwrap_or_else(|| {
                panic!(
                    "`{symbol}` is unreferenced app code and must stay a dead \
                     candidate — the fence must not swallow everything: {dead:?}"
                )
            });
        assert!(
            is_high_confidence(candidate),
            "expected `{symbol}` to stay high-confidence dead — it has no call \
             site and no framework shape: {candidate:?}"
        );
    }
}

// ============================================================
// Regression fence — crowds are a reporting surface, not a score input
// ============================================================

/// GREEN by design. The plan's fifth row did not survive falsification:
/// `detect_all_crowds` lands in `findings.crowds` and never reaches
/// `calculate_health_score`. The risk is the opposite of the one assumed — that
/// somebody wires it in later — so this fences the contract instead of changing it.
///
/// The destructuring below is the real guard: it is exhaustive on purpose, so a
/// new `HealthMetrics` input stops this test from compiling and forces the
/// author to justify it here.
#[test]
fn crowds_not_in_health_metrics() {
    let HealthMetrics {
        // CERTAIN
        missing_handlers,
        unregistered_handlers,
        breaking_cycles,
        // HIGH
        unused_high_confidence,
        dead_exports,
        twins_dead_parrots,
        // SMELL
        twins_same_language,
        barrel_chaos_count,
        structural_cycles,
        cascade_imports,
        duplicate_exports,
        // context + drill-down
        files,
        loc,
        certain_items,
        high_items,
        smell_items,
    } = HealthMetrics::default();

    let inputs = [
        missing_handlers,
        unregistered_handlers,
        breaking_cycles,
        unused_high_confidence,
        dead_exports,
        twins_dead_parrots,
        twins_same_language,
        barrel_chaos_count,
        structural_cycles,
        cascade_imports,
        duplicate_exports,
    ];
    assert_eq!(
        inputs.len(),
        11,
        "the health score has eleven issue inputs and none of them is crowd-derived"
    );
    assert_eq!(files + loc, 0, "project-size context defaults to zero");
    assert!(
        certain_items.is_empty() && high_items.is_empty() && smell_items.is_empty(),
        "drill-down details default to empty"
    );

    let clean = calculate_health_score(&HealthMetrics {
        files: 7,
        loc: 52,
        ..HealthMetrics::default()
    });
    assert_eq!(
        clean.health, Some(100),
        "a repository with zero issues scores 100 — no crowd count can move it"
    );

    // End to end: crowds are present in the audit payload and absent from the
    // scored summary. Losing either half breaks the contract.
    let (fixture, cache) = swift_fixture("swift_health_truth");
    let audit = audit_json(fixture.path(), cache.path());
    let clusters = audit["crowds"]["clusters"]
        .as_array()
        .expect("crowds must stay a reporting surface in the audit payload");
    assert!(
        !clusters.is_empty(),
        "the fixture must actually produce crowd clusters, otherwise this \
         fence proves nothing"
    );

    let summary = findings_summary(fixture.path(), cache.path());
    let scored_keys: Vec<&String> = summary
        .as_object()
        .expect("findings --summary must be an object")
        .keys()
        .filter(|k| k.contains("crowd"))
        .collect();
    assert!(
        scored_keys.is_empty(),
        "the scored summary must expose no crowd-derived key, got {scored_keys:?}"
    );
}

/// The gate that measures this file must be able to see it.
///
/// `cargo test -p loctree <filter>` prints one `test result:` line per
/// integration target, in alphabetical order, and the delivery gate reads that
/// stream through `tail -3`. A target sorting after this one therefore hands
/// the gate *its* summary — `ok. 0 passed; N filtered out` — which reads as a
/// green exit code while proving nothing. That is not hypothetical: a target
/// named `zzz_namesake_suppression.rs` landed on this branch and silently
/// blinded every gate of this line until it was renamed.
///
/// The module header already asked for this invariant in prose and prose lost.
/// Assert it instead: a future cut that reaches for one more `z` now fails
/// loudly here rather than quietly downgrading somebody else's evidence.
#[test]
fn health_truth_gate_target_sorts_last() {
    let tests_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut targets: Vec<String> = std::fs::read_dir(&tests_dir)
        .expect("tests/ must be readable")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let name = entry.file_name().into_string().ok()?;
            name.ends_with(".rs").then_some(name)
        })
        .collect();
    targets.sort();

    let last = targets
        .last()
        .expect("tests/ must hold integration targets");
    assert_eq!(
        last, "zz_swift_report_truth.rs",
        "this target must run last so `cargo test -p loctree <filter> | tail -3` \
         reports its summary; {last} sorts after it and would blind the gate. \
         Rename the offender, or pin every gate of this line with \
         `--test zz_swift_report_truth`. Full target list: {targets:?}"
    );
}
