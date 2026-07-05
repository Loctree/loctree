//! Plan 19 Stage 1 — tree-sitter parity sanity vs hand-computed expectations
//! on the `simple_ts` fixture. Runs as `#[ignore]` so it stays out of the
//! green test set; invoke explicitly to print a per-category match table.
//!
//! OXC's `analyze_js_file_ast` is `pub(crate)`, so direct cross-stack diff
//! lives in the report (`internal-artifacts/reports/lsp/19-cross-lang-stage-1.md`)
//! using the canonical CLI smoke (`LOCTREE_PARSER=ts ... vs ... LOCTREE_PARSER=oxc`).
//! This harness validates the tree-sitter side hits the expected counts.

use loctree::analyzer::scan::ts_dispatch_js;
use std::collections::HashSet;
use std::path::PathBuf;

fn fixture_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/simple_ts");
    path
}

#[test]
#[ignore]
fn ts_dispatch_matches_handcounted_expectations() {
    let root = fixture_root();
    let cases: &[(&str, &[&str], &[&str])] = &[
        // (relative path, expected exports, expected import sources)
        (
            "src/index.ts",
            &["main"],
            &["./utils/greeting", "./utils/date"],
        ),
        ("src/utils/greeting.ts", &["greet", "farewell"], &[]),
        ("src/utils/date.ts", &["formatDate", "parseDate"], &[]),
    ];

    let mut total_exports = 0usize;
    let mut hit_exports = 0usize;
    let mut total_imports = 0usize;
    let mut hit_imports = 0usize;

    for (rel, expected_exports, expected_imports) in cases {
        let abs = root.join(rel);
        let content = std::fs::read_to_string(&abs).expect("fixture readable");
        let analysis = ts_dispatch_js(&content, &abs, rel.to_string());

        let got_exports: HashSet<&str> = analysis.exports.iter().map(|e| e.name.as_str()).collect();
        let got_imports: HashSet<&str> =
            analysis.imports.iter().map(|i| i.source.as_str()).collect();

        for name in *expected_exports {
            total_exports += 1;
            if got_exports.contains(name) {
                hit_exports += 1;
            } else {
                eprintln!("MISS export {} in {}", name, rel);
            }
        }
        for src in *expected_imports {
            total_imports += 1;
            if got_imports.contains(src) {
                hit_imports += 1;
            } else {
                eprintln!("MISS import {} in {}", src, rel);
            }
        }

        println!(
            "{:<28} exports={} imports={} calls={}",
            rel,
            analysis.exports.len(),
            analysis.imports.len(),
            analysis.symbol_usages.len()
        );
    }

    println!(
        "EXPORTS hit {}/{} ({:.1}%)",
        hit_exports,
        total_exports,
        (hit_exports as f64 / total_exports as f64) * 100.0
    );
    println!(
        "IMPORTS hit {}/{} ({:.1}%)",
        hit_imports,
        total_imports,
        (hit_imports as f64 / total_imports as f64) * 100.0
    );

    assert_eq!(hit_exports, total_exports, "expected 100% export parity");
    assert_eq!(hit_imports, total_imports, "expected 100% import parity");
}
