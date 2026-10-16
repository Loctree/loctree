use serde_json::Value;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn fixture() -> TempDir {
    let project = TempDir::new().expect("fixture directory");
    for (name, source) in [
        (
            "aaa_context.py",
            include_str!("fixtures/tagmap_discovery/aaa_context.py"),
        ),
        (
            "bbb_import.py",
            include_str!("fixtures/tagmap_discovery/bbb_import.py"),
        ),
        (
            "zzz_owner.py",
            include_str!("fixtures/tagmap_discovery/zzz_owner.py"),
        ),
    ] {
        fs::write(project.path().join(name), source).expect("fixture source");
    }
    project
}

fn tagmap(project: &TempDir, keyword: &str, limit: usize) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_loct"))
        .current_dir(project.path())
        .env("LOCT_CACHE_DIR", project.path().join(".cache"))
        .arg("--force-non-git-repository-snapshot")
        .args(["tagmap", keyword, "--limit", &limit.to_string(), "--json"])
        .output()
        .expect("run tagmap");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("tagmap JSON")
}

#[test]
fn tagmap_first_page_prefers_declarations_and_import_identities() {
    let project = fixture();
    let full = tagmap(&project, "glyph", 100);
    let page = tagmap(&project, "glyph", 2);
    let facts = page["facts"]["items"].as_array().expect("facts");
    assert!(facts.iter().any(|f| f["name"] == "glyph_engine"), "{page}");
    assert!(
        facts
            .iter()
            .any(|f| f["kind"] == "import-source" || f["kind"] == "import-symbol"),
        "{page}"
    );
    assert_eq!(
        facts,
        &full["facts"]["items"].as_array().expect("full facts")[..2]
    );
    assert_eq!(page["facts"]["count"], full["facts"]["count"]);
    assert_eq!(
        page["facts"]["omitted_by_limit"].as_u64(),
        Some(full["facts"]["count"].as_u64().expect("count") - 2)
    );
    let contexts: Vec<_> = full["facts"]["items"]
        .as_array()
        .expect("full facts")
        .iter()
        .filter(|f| f["kind"] == "symbol-usage-context" && f["file"] == "aaa_context.py")
        .collect();
    assert_eq!(
        contexts.len(),
        1,
        "one source line must not emit unrelated tokens: {full}"
    );
    assert_eq!(tagmap(&project, "glyph", 2), page, "stable ordering");
}

#[test]
fn tagmap_retains_context_recall_aliases_normalization_and_absence() {
    let project = fixture();
    let context = tagmap(&project, "lunar-key", 1);
    assert_eq!(context["facts"]["count"], 1, "{context}");
    assert_eq!(context["facts"]["items"][0]["kind"], "symbol-usage-context");
    assert_eq!(context["facts"]["items"][0]["confidence"], "low");
    let alias = tagmap(&project, "RenderGlyph", 1);
    assert_eq!(
        alias["facts"]["items"][0]["kind"], "import-symbol",
        "{alias}"
    );
    assert_eq!(alias["facts"]["items"][0]["alias"], "RenderGlyph");
    let normalized = tagmap(&project, "GLYPH-ENGINE", 1);
    assert_eq!(normalized["facts"]["items"][0]["name"], "glyph_engine");
    assert_eq!(
        tagmap(&project, "notpresentatall987", 1)["facts"]["count"],
        0
    );
    assert!(
        tagmap(&project, "glyph", 0)["facts"]["items"]
            .as_array()
            .expect("zero page")
            .is_empty()
    );
}
