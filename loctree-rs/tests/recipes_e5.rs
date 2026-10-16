//! E5 recipe tests — Loctree engine TDD fleet, 2026-09-26.
//!
//! Covers the findings whose fix lives inside `scan_roots`'s multi-file
//! pipeline (G1, G4.2, G6, G8-cross-language) and therefore can't be
//! exercised as a same-crate inline unit test the way the single-file
//! findings (G7, G9, H9, F2a, H10) are. These run the real `loct` binary
//! end-to-end against small purpose-built fixtures, mirroring the
//! conventions already proven in `tests/e2e_cli.rs`.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use std::path::PathBuf;
use tempfile::TempDir;

/// Binary-wide fallback cache so no test command ever writes fixture
/// snapshots into the operator-global cache. Mirrors
/// `tests/e2e_cli.rs::test_cache_dir`.
fn test_cache_dir() -> &'static std::path::Path {
    static CACHE: std::sync::LazyLock<TempDir> =
        std::sync::LazyLock::new(|| TempDir::new().expect("shared test cache dir"));
    CACHE.path()
}

fn fixtures_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Get a command pointing to the `loct` binary, isolated cache + non-git
/// root allowance. Mirrors `tests/e2e_cli.rs::loct`.
fn loct() -> Command {
    let mut cmd = cargo_bin_cmd!("loct");
    cmd.env("LOCT_OPEN_BROWSER", "0");
    cmd.env("LOCT_CACHE_DIR", test_cache_dir());
    cmd.env(loctree::snapshot::LOCT_ALLOW_NON_GIT_ROOT_ENV, "1");
    cmd
}

/// Run `loct [.edges[]]` in `fixture` and return the parsed edge array.
///
/// Wrapped in `[...]` rather than a bare `.edges`: the existing e2e suite
/// (`tests/e2e_cli.rs`) only ever exercises this query engine through that
/// exact bracketed-streaming shape (`[.edges[] | select(...)] | length`) or
/// plain scalar/dotted paths (`.metadata.file_count`); a bare top-level
/// array field with no proven precedent is a needless risk when this
/// equivalent, already-proven shape is available.
fn snapshot_edges(fixture: &std::path::Path) -> Vec<Value> {
    let cache = TempDir::new().expect("temp cache");
    let output = loct()
        .current_dir(fixture)
        .env("LOCT_CACHE_DIR", cache.path())
        .arg("[.edges[]]")
        .assert()
        .success()
        .get_output()
        .clone();
    let edges: Value = serde_json::from_slice(&output.stdout).expect("[.edges[]] must be JSON");
    edges.as_array().cloned().unwrap_or_default()
}

fn edge_str<'a>(edge: &'a Value, key: &str) -> Option<&'a str> {
    edge.get(key).and_then(Value::as_str)
}

/// G1: `crate::foo::bar()` with no `use` statement must still surface as a
/// caller edge — path-qualified calls previously never became edges because
/// the import-edge loop only looks at `analysis.imports` (empty here; there
/// is nothing to `use`).
#[test]
fn g1_rust_path_qualified_call_becomes_a_call_edge() {
    let fixture = fixtures_path().join("recipes_e5_rust_call");
    let edges = snapshot_edges(&fixture);

    let hit = edges.iter().find(|e| {
        edge_str(e, "label") == Some("call")
            && edge_str(e, "from") == Some("app/caller.rs")
            && edge_str(e, "to") == Some("app/foo.rs")
    });
    assert!(
        hit.is_some(),
        "expected a call edge app/caller.rs -> app/foo.rs for `crate::foo::bar()`, got edges: {edges:#?}"
    );
}

/// G4.2: `pub use crate::text::GlyphCache;` must resolve to the defining
/// file. The reexport-resolution match in `root_scan.rs` had no "rs" arm,
/// so `ReexportEntry.resolved` (and the corresponding "reexport" graph edge)
/// stayed `None`/absent forever for every Rust `pub use`.
#[test]
fn g4_2_rust_pub_use_reexport_resolves_to_defining_file() {
    let fixture = fixtures_path().join("recipes_e5_rust_reexport");
    let edges = snapshot_edges(&fixture);

    let hit = edges.iter().find(|e| {
        edge_str(e, "label") == Some("reexport")
            && edge_str(e, "from") == Some("app/lib.rs")
            && edge_str(e, "to") == Some("app/text.rs")
    });
    assert!(
        hit.is_some(),
        "expected a reexport edge app/lib.rs -> app/text.rs for `pub use text::GlyphCache;`, got edges: {edges:#?}"
    );
}

/// G6: every zig import edge's `to` must be root-relative like every other
/// resolver's output (JS/TS, Python, Rust) — `resolve_zig_import` used to
/// return the OS-canonical *absolute* path, making the edge invisible to
/// impact/cycles' root-relative-keyed lookups. Uses the existing
/// `zig_project` fixture (main.zig -> helpers.zig), read-only.
#[test]
fn g6_zig_import_edges_are_root_relative() {
    let fixture = fixtures_path().join("zig_project");
    let edges = snapshot_edges(&fixture);

    let zig_edges: Vec<&Value> = edges
        .iter()
        .filter(|e| edge_str(e, "to").is_some_and(|to| to.ends_with(".zig")))
        .collect();
    assert!(
        !zig_edges.is_empty(),
        "expected at least one zig->zig edge from the fixture, got edges: {edges:#?}"
    );
    for edge in &zig_edges {
        let to = edge_str(edge, "to").unwrap_or_default();
        assert!(
            !to.starts_with('/'),
            "zig edge target must be root-relative (like every other resolver), got: {to:?} in {edge:?}"
        );
    }
}

/// G8 (cross-language half): a Swift `import Foundation` (a bare usage of
/// the word "Foundation") must never wire an `implicit_symbol` edge to an
/// unrelated Python `class Foundation` living elsewhere in the repo. Before
/// the language filter, the exporter-count guard counted exporters across
/// ALL languages, so a name with exactly one exporter anywhere (regardless
/// of language) looked "unambiguous" even when that one exporter was in a
/// different language than the usage site.
#[test]
fn g8_cross_language_implicit_symbol_edge_is_rejected() {
    let fixture = fixtures_path().join("recipes_e5_swift_python_cross_lang");
    let edges = snapshot_edges(&fixture);

    let bad_edge = edges.iter().find(|e| {
        edge_str(e, "label") == Some("implicit_symbol")
            && edge_str(e, "from") == Some("App.swift")
            && edge_str(e, "to") == Some("app.py")
    });
    assert!(
        bad_edge.is_none(),
        "must not create a cross-language implicit_symbol edge Swift -> Python, got edges: {edges:#?}"
    );
}

/// N02 (audit 2026-09-27, P0): a bare-name mention must never become a
/// `call` edge. `widget` in the fixture is a plain local binding; the only
/// exporter of that name is a Python file. The old pass trusted "exactly one
/// global exporter" with no language or call-shape check, wiring
/// caller.rs -> widget.py out of nothing.
#[test]
fn g1_bare_identifier_never_becomes_a_cross_language_call_edge() {
    let fixture = fixtures_path().join("recipes_e5_rust_call_negative");
    let edges = snapshot_edges(&fixture);

    let bogus: Vec<&Value> = edges
        .iter()
        .filter(|e| {
            edge_str(e, "label") == Some("call") && edge_str(e, "from") == Some("app/caller.rs")
        })
        .collect();
    assert!(
        bogus.is_empty(),
        "a bare local identifier must not produce call edges, got: {bogus:#?}"
    );
}

/// N02 (audit 2026-09-27, P0): a path-qualified call whose leaf name has
/// SEVERAL same-language exporters is still resolvable — the qualifier says
/// which file. `crate::diff::analyze()` must wire caller.rs -> diff.rs and
/// never caller.rs -> perf.rs, even though both export `analyze`.
#[test]
fn g1_qualified_call_with_homonyms_resolves_via_qualifier() {
    let fixture = fixtures_path().join("recipes_e5_rust_call_homonyms");
    let edges = snapshot_edges(&fixture);

    let to_diff = edges.iter().any(|e| {
        edge_str(e, "label") == Some("call")
            && edge_str(e, "from") == Some("app/caller.rs")
            && edge_str(e, "to") == Some("app/diff.rs")
    });
    let to_perf = edges.iter().any(|e| {
        edge_str(e, "label") == Some("call")
            && edge_str(e, "from") == Some("app/caller.rs")
            && edge_str(e, "to") == Some("app/perf.rs")
    });
    assert!(
        to_diff,
        "qualified call must resolve the homonym to diff.rs, got edges: {edges:#?}"
    );
    assert!(
        !to_perf,
        "qualified call must never also guess the OTHER homonym exporter (perf.rs), got edges: {edges:#?}"
    );
}

fn rust_call_identity_fixture() -> TempDir {
    let fixture = TempDir::new().expect("fixture");
    for (path, source) in [
        (
            "Cargo.toml",
            "[workspace]\nmembers = ['engine', 'lsp']\nresolver = '2'\n",
        ),
        (
            "engine/Cargo.toml",
            "[package]\nname = 'engine'\nversion = '0.1.0'\nedition = '2024'\n",
        ),
        (
            "lsp/Cargo.toml",
            "[package]\nname = 'lsp'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\nengine = { path = '../engine' }\n",
        ),
        (
            "engine/src/lib.rs",
            "pub mod semantic;\npub mod api;\npub mod other;\npub mod callers;\n",
        ),
        (
            "engine/src/semantic.rs",
            "pub struct Classifier;\npub fn classify() {}\n",
        ),
        (
            "engine/src/api/mod.rs",
            "pub mod nested;\npub mod worker;\n",
        ),
        (
            "engine/src/api/nested.rs",
            "pub fn leaf() {}\npub fn shared() {}\n",
        ),
        (
            "engine/src/api/worker.rs",
            "pub struct Worker;\nimpl Worker {\n    pub fn execute() {}\n}\n",
        ),
        (
            "engine/src/other.rs",
            "pub fn shared() {}\npub fn unique_wrong_namespace() {}\npub struct Other;\nimpl Other {\n    pub fn execute() {}\n}\n",
        ),
        (
            "lsp/src/lib.rs",
            "pub mod backend;\npub mod api;\npub mod callers;\n",
        ),
        (
            "lsp/src/backend.rs",
            "pub struct Backend;\nimpl Backend {\n    pub fn semantic() {}\n}\n",
        ),
        ("lsp/src/api/mod.rs", "pub mod nested;\n"),
        (
            "lsp/src/api/nested.rs",
            "pub fn leaf() {}\npub fn shared() {}\n",
        ),
        (
            "lsp/src/callers.rs",
            "pub fn run() {\n    engine::api::nested::shared();\n    crate::api::nested::leaf();\n}\n",
        ),
        (
            "engine/src/callers/mod.rs",
            "pub mod mentions;\npub mod qualified;\npub mod aliases;\npub mod uncertain;\npub mod scoped;\npub mod ambiguous;\n",
        ),
        (
            "engine/src/callers/mentions.rs",
            r##"use crate::semantic::Classifier;
// crate::backend::semantic();
/* crate::backend::semantic(); */
#[cfg_attr(any(), crate::backend::semantic())]
pub fn mentions(_: Option<crate::semantic::Classifier>) {
    let _ = "crate::backend::semantic()";
    let _ = r#"crate::backend::semantic()"#;
    crate::semantic::classify();
}
"##,
        ),
        (
            "engine/src/callers/qualified.rs",
            "pub fn run() {\n    crate::api::nested::leaf();\n    crate::api::nested::shared();\n    crate::api::worker::Worker::execute();\n}\n",
        ),
        (
            "engine/src/callers/aliases.rs",
            "use crate::api as renamed;\nuse crate::api::worker::Worker as Runner;\npub fn run() {\n    renamed::nested::leaf();\n    Runner::execute();\n}\n",
        ),
        (
            "engine/src/callers/uncertain.rs",
            "pub fn run() {\n    missing::semantic();\n    missing::unique_wrong_namespace();\n    crate::api::missing::unique_wrong_namespace();\n    crate::api::nested::Worker::execute();\n    foreign::python_only();\n    foreign::typescript_only();\n}\n",
        ),
        (
            "engine/src/callers/scoped.rs",
            "pub fn local_alias() {\n    use crate::api as renamed;\n    let _brace = '}';\n    renamed::nested::leaf();\n}\npub fn out_of_scope() {\n    renamed::worker::Worker::execute();\n}\n",
        ),
        (
            "engine/src/callers/ambiguous.rs",
            "use crate::api as renamed;\nuse crate::other as renamed;\npub fn ambiguous() {\n    renamed::nested::leaf();\n}\n",
        ),
        ("names.py", "def python_only():\n    pass\n"),
        ("names.ts", "export function typescript_only() {}\n"),
    ] {
        let path = fixture.path().join(path);
        std::fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture directory");
        std::fs::write(path, source).expect("fixture source");
    }
    fixture
}

#[test]
fn g1_module_mentions_do_not_fabricate_reverse_cross_crate_calls() {
    let fixture = rust_call_identity_fixture();
    let edges = snapshot_edges(fixture.path());
    let caller = "engine/src/callers/mentions.rs";
    let targets: Vec<_> = edges
        .iter()
        .filter(|e| edge_str(e, "from") == Some(caller))
        .map(|e| edge_str(e, "to").expect("edge target"))
        .collect();
    assert!(
        targets.contains(&"engine/src/semantic.rs"),
        "real semantic dependency: {edges:#?}"
    );
    assert!(
        !targets.contains(&"lsp/src/backend.rs"),
        "module token is not Backend::semantic(): {edges:#?}"
    );
}

#[test]
fn g1_calls_resolve_exact_crate_module_and_receiver_identity() {
    let fixture = rust_call_identity_fixture();
    let edges = snapshot_edges(fixture.path());
    for (caller, expected) in [
        (
            "engine/src/callers/qualified.rs",
            vec!["engine/src/api/nested.rs", "engine/src/api/worker.rs"],
        ),
        (
            "engine/src/callers/aliases.rs",
            vec![
                "engine/src/api/mod.rs",
                "engine/src/api/nested.rs",
                "engine/src/api/worker.rs",
            ],
        ),
        (
            "lsp/src/callers.rs",
            vec!["engine/src/api/nested.rs", "lsp/src/api/nested.rs"],
        ),
    ] {
        let mut targets: Vec<_> = edges
            .iter()
            .filter(|e| edge_str(e, "from") == Some(caller))
            .map(|e| edge_str(e, "to").expect("edge target"))
            .collect();
        targets.sort_unstable();
        targets.dedup();
        assert_eq!(
            targets, expected,
            "wrong target identity for {caller}: {edges:#?}"
        );
    }
}

#[test]
fn g1_unresolved_namespaces_and_foreign_namesakes_stay_uncertain() {
    let fixture = rust_call_identity_fixture();
    let edges = snapshot_edges(fixture.path());
    let targets: Vec<_> = edges
        .iter()
        .filter(|e| edge_str(e, "from") == Some("engine/src/callers/uncertain.rs"))
        .collect();
    assert!(
        targets.is_empty(),
        "unresolved calls must not guess exporters: {targets:#?}"
    );
}

#[test]
fn g1_function_local_module_alias_does_not_escape_its_scope() {
    let fixture = rust_call_identity_fixture();
    let edges = snapshot_edges(fixture.path());
    let mut targets: Vec<_> = edges
        .iter()
        .filter(|e| {
            edge_str(e, "from") == Some("engine/src/callers/scoped.rs")
                && edge_str(e, "label") == Some("call")
        })
        .map(|e| edge_str(e, "to").expect("edge target"))
        .collect();
    targets.sort_unstable();
    assert_eq!(targets, vec!["engine/src/api/nested.rs"], "{edges:#?}");
}

#[test]
fn g1_ambiguous_module_alias_does_not_choose_a_global_exporter() {
    let fixture = rust_call_identity_fixture();
    let edges = snapshot_edges(fixture.path());
    assert!(
        !edges.iter().any(
            |e| edge_str(e, "from") == Some("engine/src/callers/ambiguous.rs")
                && edge_str(e, "label") == Some("call")
        ),
        "{edges:#?}"
    );
}
