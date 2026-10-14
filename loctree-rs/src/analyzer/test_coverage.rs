//! Test coverage analysis - structural coverage based on imports
//!
//! Analyzes which production code is actually imported by tests.
//! This provides "structural coverage" - what code is touched by test imports,
//! without running tests or inspecting runtime behavior.

use crate::snapshot::Snapshot;
use crate::types::{FileAnalysis, ReexportKind};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// A production symbol and its test coverage status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolCoverage {
    /// Symbol name (export name)
    pub symbol: String,
    /// File where symbol is defined
    pub defined_in: PathBuf,
    /// Line number of definition
    pub line: usize,
    /// Test files that import this symbol
    pub tested_by: Vec<PathBuf>,
    /// True if at least one test imports this symbol
    pub is_covered: bool,
}

/// Coverage summary for handlers specifically
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandlerCoverage {
    /// Handler function name
    pub name: String,
    /// Backend file where handler is defined
    pub backend_file: PathBuf,
    /// Line number of handler definition
    pub line: usize,
    /// Number of frontend calls to this handler
    pub frontend_calls: usize,
    /// Test files that import this handler
    pub test_imports: Vec<PathBuf>,
    /// Overall coverage status
    pub coverage_status: CoverageStatus,
}

/// Coverage status classification
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    /// Has both FE usage and test coverage
    FullyCovered,
    /// Used in FE but no tests
    MissingTests,
    /// Has tests but unused in FE (suspicious)
    TestOnly,
    /// Neither FE nor tests (dead code)
    Uncovered,
}

/// Report of test coverage analysis
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestCoverageReport {
    /// Handler-specific coverage analysis
    pub handlers: Vec<HandlerCoverage>,
    /// Production exports without any test imports
    pub exports_without_tests: Vec<SymbolCoverage>,
    /// Number of test files found
    pub test_file_count: usize,
    /// Number of production files found
    pub prod_file_count: usize,
    /// Overall coverage percentage (0.0-100.0)
    pub coverage_percent: f32,
}

/// Analyze test coverage from a snapshot
///
/// This function:
/// 1. Separates test files from production files
/// 2. Builds import graph: test files -> production symbols
/// 3. Analyzes handlers for FE usage + test coverage
/// 4. Identifies uncovered exports
pub fn analyze_test_coverage(snapshot: &Snapshot) -> TestCoverageReport {
    let (test_files, prod_files) = partition_files(&snapshot.files);

    let test_file_count = test_files.len();
    let prod_file_count = prod_files.len();

    // Build map: production_file_path -> exported_symbols
    let prod_exports = build_export_map(&prod_files);

    // Build map: production_symbol -> test_files_that_import_it
    let mut symbol_coverage = build_symbol_coverage_map(&test_files, &prod_files, &prod_exports);

    // H4(c): credit files with an inline `#[cfg(test)]` module (or Zig
    // `test "…"` block) as covering their own exports - see
    // `credit_inline_test_coverage` for the per-symbol inline-test credit,
    // not a per-symbol one.
    credit_inline_test_coverage(&prod_files, &mut symbol_coverage);

    // Analyze handlers (Tauri commands)
    let handlers = analyze_handler_coverage(snapshot, &symbol_coverage);

    // Find exports without test coverage
    let exports_without_tests = find_uncovered_exports(&prod_files, &symbol_coverage);

    // Calculate overall coverage percentage.
    // N1 (round-2): both sides of the ratio count only testable export kinds
    // (function/method). Types, consts, re-exports and FFI externs have no
    // behavior to cover; counting them in the denominator type-inflated the
    // percentage.
    let mut total_exports = 0usize;
    let mut covered_exports = 0usize;
    for file in &prod_files {
        for export in &file.exports {
            if !super::coverage_gaps::is_testable_export_kind(&export.kind) {
                continue;
            }
            total_exports += 1;
            let coverage_key = format!("{}::{}", file.path, export.name);
            if symbol_coverage.contains_key(&coverage_key) {
                covered_exports += 1;
            }
        }
    }
    let coverage_percent = if total_exports > 0 {
        (covered_exports as f32 / total_exports as f32) * 100.0
    } else {
        0.0
    };

    TestCoverageReport {
        handlers,
        exports_without_tests,
        test_file_count,
        prod_file_count,
        coverage_percent,
    }
}

/// Partition files into test files and production files
fn partition_files(files: &[FileAnalysis]) -> (Vec<&FileAnalysis>, Vec<&FileAnalysis>) {
    let mut test_files = Vec::new();
    let mut prod_files = Vec::new();

    for file in files {
        // Artifact fence: vendored/minified/fixture/generated/template files
        // are neither tests nor production surface — they would inflate
        // "exports without tests" with non-product symbols.
        if super::classify::artifact_class(&file.path, None).is_artifact() {
            continue;
        }
        if file.is_test || file.kind == "test" {
            test_files.push(file);
        } else if file.kind == "code" {
            // Only consider "code" files as production (exclude config, stories, etc.)
            prod_files.push(file);
        }
    }

    (test_files, prod_files)
}

/// Build map of production file path -> exported symbols
fn build_export_map(prod_files: &[&FileAnalysis]) -> HashMap<String, Vec<(String, usize)>> {
    let mut map: HashMap<String, Vec<(String, usize)>> = HashMap::new();

    for file in prod_files {
        let mut symbols = Vec::new();
        for export in &file.exports {
            symbols.push((export.name.clone(), export.line.unwrap_or(0)));
        }
        if !symbols.is_empty() {
            map.insert(file.path.clone(), symbols);
        }
    }

    map
}

/// Build map of production symbols -> test files that import them
fn build_symbol_coverage_map(
    test_files: &[&FileAnalysis],
    prod_files: &[&FileAnalysis],
    prod_exports: &HashMap<String, Vec<(String, usize)>>,
) -> HashMap<String, HashSet<PathBuf>> {
    let mut coverage_map: HashMap<String, HashSet<PathBuf>> = HashMap::new();

    // Create reverse map: file_path -> file for quick lookup
    let prod_file_map: HashMap<&str, &FileAnalysis> =
        prod_files.iter().map(|f| (f.path.as_str(), *f)).collect();

    for test_file in test_files {
        for import in &test_file.imports {
            // Find which production file this import resolves to
            let target_path = if let Some(resolved) = &import.resolved_path {
                resolved.as_str()
            } else {
                continue;
            };

            // Check if this is a production file we're tracking
            if !prod_file_map.contains_key(target_path) {
                continue;
            }

            // Get symbols imported from this file
            for symbol in &import.symbols {
                let symbol_name = if let Some(_alias) = &symbol.alias {
                    // Use original name, not alias
                    &symbol.name
                } else {
                    &symbol.name
                };

                // Build unique key: file_path::symbol_name
                let coverage_key = format!("{}::{}", target_path, symbol_name);

                coverage_map
                    .entry(coverage_key)
                    .or_default()
                    .insert(PathBuf::from(&test_file.path));
            }

            // Handle star imports (import *)
            if import.symbols.is_empty() && !import.source.is_empty() {
                // This is a side-effect or star import
                // Consider all exports from this file as "covered"
                if let Some(exports) = prod_exports.get(target_path) {
                    for (export_name, _line) in exports {
                        let coverage_key = format!("{}::{}", target_path, export_name);
                        coverage_map
                            .entry(coverage_key)
                            .or_default()
                            .insert(PathBuf::from(&test_file.path));
                    }
                }
            }
        }
    }

    coverage_map
}

/// Analyze handler coverage (Tauri commands)
fn analyze_handler_coverage(
    snapshot: &Snapshot,
    symbol_coverage: &HashMap<String, HashSet<PathBuf>>,
) -> Vec<HandlerCoverage> {
    let mut handlers = Vec::new();

    for bridge in &snapshot.command_bridges {
        if !bridge.has_handler {
            continue; // Skip commands without handlers
        }

        let Some((backend_file, line)) = &bridge.backend_handler else {
            continue;
        };

        // Count frontend calls
        let frontend_calls = bridge.frontend_calls.len();

        // Find test imports for this handler
        let handler_key = format!("{}::{}", backend_file, bridge.name);
        let test_imports: Vec<PathBuf> = symbol_coverage
            .get(&handler_key)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default();

        // Determine coverage status
        let has_tests = !test_imports.is_empty();
        let has_fe_calls = frontend_calls > 0;

        let coverage_status = match (has_fe_calls, has_tests) {
            (true, true) => CoverageStatus::FullyCovered,
            (true, false) => CoverageStatus::MissingTests,
            (false, true) => CoverageStatus::TestOnly,
            (false, false) => CoverageStatus::Uncovered,
        };

        handlers.push(HandlerCoverage {
            name: bridge.name.clone(),
            backend_file: PathBuf::from(backend_file),
            line: *line,
            frontend_calls,
            test_imports,
            coverage_status,
        });
    }

    handlers
}

/// H4(b): `pub use` targets whose source root is a well-known stdlib/core
/// crate (`pub use std::process::Command;`) are not a symbol this repo
/// defines - they're forwarding an external item. `ExportSymbol` (the type
/// `file.exports` holds) carries no origin field, but the matching
/// `FileAnalysis::reexports` entry does (`ReexportEntry::source`, e.g.
/// `"std::process::Command"`). Deliberately conservative/short list: it
/// only recognizes the standard-library family, not arbitrary third-party
/// crates (that would need the file's own crate name to distinguish a
/// same-crate absolute path from an external one, which isn't available
/// here) - so this only ever REMOVES noise, never hides a real local gap.
const EXTERNAL_REEXPORT_ROOTS: &[&str] = &["std", "core", "alloc", "proc_macro", "proc_macro2"];

fn is_external_reexport_source(source: &str) -> bool {
    let root = source.split("::").next().unwrap_or(source);
    EXTERNAL_REEXPORT_ROOTS.contains(&root)
}

/// Names in `file.exports` that are re-exports of an external/stdlib item
/// per [`is_external_reexport_source`], cross-referenced from
/// `file.reexports`.
fn external_reexport_names(file: &FileAnalysis) -> HashSet<&str> {
    let mut names = HashSet::new();
    for reexport in &file.reexports {
        if !is_external_reexport_source(&reexport.source) {
            continue;
        }
        if let ReexportKind::Named(pairs) = &reexport.kind {
            for (_, exported) in pairs {
                names.insert(exported.as_str());
            }
        }
    }
    names
}

/// H4(c): does this file carry its own inline test module/blocks? Rust
/// `#[cfg(test)] mod ... { ... }` and Zig `test "…" { ... }` both put tests
/// in the SAME file as the production code they exercise, so a purely
/// path/kind-based test/prod partition (see `partition_files`) never
/// classifies such a file as having any test coverage at all - every one of
/// its exports reads as "uncovered" even on a thoroughly inline-tested
/// repo, producing a flat, wrong 0.0% coverage reading.
///
/// This is a best-effort, FILE-granularity signal only: it does not attempt
/// to map a specific inline test to the specific export it exercises -
/// `FileAnalysis` carries no such per-symbol data today, and adding a
/// dedicated sensor field is out of this executor's edit scope (`types.rs`
/// and the Layer 1 Rust/Zig sensors are owned elsewhere). Flagged for the
/// integrator: the principled fix is Layer 1 recording this once per file
/// instead of every downstream consumer re-reading the file from disk.
fn inline_test_region(file: &FileAnalysis, content: &str) -> Option<String> {
    match file.language.as_str() {
        "rs" => {
            let ranges = crate::analyzer::occurrences::cfg_test_line_ranges(content);
            if ranges.is_empty() {
                return None;
            }
            let mut region = String::new();
            for (idx, line) in content.lines().enumerate() {
                let line_no = idx + 1;
                if ranges
                    .iter()
                    .any(|(start, end)| line_no >= *start && line_no <= *end)
                {
                    region.push_str(line);
                    region.push('\n');
                }
            }
            (!region.trim().is_empty()).then_some(region)
        }
        "zig" => zig_test_regions(content),
        _ => None,
    }
}

/// Concatenated bodies of Zig `test "…" { … }` blocks, brace-matched.
fn zig_test_regions(content: &str) -> Option<String> {
    let mut region = String::new();
    let mut search = 0usize;
    while let Some(pos) = content[search..].find("test \"") {
        let start = search + pos;
        let Some(brace_rel) = content[start..].find('{') else {
            break;
        };
        let brace = start + brace_rel;
        let mut depth = 0usize;
        let mut end = None;
        for (offset, ch) in content[brace..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(brace + offset + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            break;
        };
        region.push_str(&content[start..end]);
        region.push('\n');
        search = end;
    }
    (!region.is_empty()).then_some(region)
}

/// Boundary-aware "does this identifier appear in the region" check.
fn identifier_in_region(region: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let bytes = region.as_bytes();
    let mut from = 0usize;
    while let Some(pos) = region[from..].find(name) {
        let begin = from + pos;
        let end = begin + name.len();
        let before_ok =
            begin == 0 || !(bytes[begin - 1].is_ascii_alphanumeric() || bytes[begin - 1] == b'_');
        let after_ok =
            end == bytes.len() || !(bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_');
        if before_ok && after_ok {
            return true;
        }
        from = begin + 1;
    }
    false
}

/// Credit exports of an inline-tested prod file with a synthetic
/// "inline_test" marker — but only the exports whose NAME actually appears
/// inside the inline test region. "The file contains a test module" and
/// "this symbol is exercised by a test" are different facts: crediting every
/// export for the mere presence of `#[cfg(test)]` text handed coverage to
/// untouched functions, empty test modules, and even a marker inside a
/// comment (audit N07).
fn credit_inline_test_coverage(
    prod_files: &[&FileAnalysis],
    symbol_coverage: &mut HashMap<String, HashSet<PathBuf>>,
) {
    for file in prod_files {
        if file.language != "rs" && file.language != "zig" {
            continue;
        }
        let Ok(Some(content)) = crate::semantic::io::try_read_validated_semantic_input(&file.path)
        else {
            continue;
        };
        let Some(region) = inline_test_region(file, &content) else {
            continue;
        };
        let marker = PathBuf::from(format!("{}::inline_test", file.path));
        for export in &file.exports {
            if !identifier_in_region(&region, &export.name) {
                continue;
            }
            let key = format!("{}::{}", file.path, export.name);
            symbol_coverage
                .entry(key)
                .or_default()
                .insert(marker.clone());
        }
    }
}

/// Find production exports that have no test coverage
fn find_uncovered_exports(
    prod_files: &[&FileAnalysis],
    symbol_coverage: &HashMap<String, HashSet<PathBuf>>,
) -> Vec<SymbolCoverage> {
    let mut uncovered = Vec::new();

    for file in prod_files {
        let external_reexports = external_reexport_names(file);
        for export in &file.exports {
            // N1 (round-2): only functions/methods are testable exports;
            // types, consts, re-exports and FFI externs are excluded from
            // this list entirely (integrator decision - supersedes the
            // round-1 H4(b) guard that kept local re-exports visible here).
            if !super::coverage_gaps::is_testable_export_kind(&export.kind) {
                continue;
            }
            if external_reexports.contains(export.name.as_str()) {
                continue;
            }
            let coverage_key = format!("{}::{}", file.path, export.name);
            let tested_by: Vec<PathBuf> = symbol_coverage
                .get(&coverage_key)
                .map(|set| set.iter().cloned().collect())
                .unwrap_or_default();

            let is_covered = !tested_by.is_empty();

            if !is_covered {
                uncovered.push(SymbolCoverage {
                    symbol: export.name.clone(),
                    defined_in: PathBuf::from(&file.path),
                    line: export.line.unwrap_or(0),
                    tested_by: Vec::new(),
                    is_covered: false,
                });
            }
        }
    }

    uncovered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ExportSymbol, ImportEntry, ImportKind, ImportSymbol, ReexportEntry};

    fn make_test_file(path: &str, imports: Vec<ImportEntry>) -> FileAnalysis {
        let mut file = FileAnalysis::new(path.to_string());
        file.kind = "test".to_string();
        file.is_test = true;
        file.imports = imports;
        file
    }

    fn make_prod_file(path: &str, exports: Vec<ExportSymbol>) -> FileAnalysis {
        let mut file = FileAnalysis::new(path.to_string());
        file.kind = "code".to_string();
        file.is_test = false;
        file.exports = exports;
        file
    }

    fn make_import(resolved_path: &str, symbols: Vec<&str>) -> ImportEntry {
        let mut import = ImportEntry::new("./module".to_string(), ImportKind::Static);
        import.resolved_path = Some(resolved_path.to_string());
        import.symbols = symbols
            .into_iter()
            .map(|s| ImportSymbol {
                name: s.to_string(),
                alias: None,
                is_default: false,
            })
            .collect();
        import
    }

    #[test]
    fn test_partition_files() {
        let files = vec![
            make_test_file("src/__tests__/foo.test.ts", vec![]),
            make_prod_file("src/foo.ts", vec![]),
            make_prod_file("src/bar.ts", vec![]),
        ];

        let (test_files, prod_files) = partition_files(&files);
        assert_eq!(test_files.len(), 1);
        assert_eq!(prod_files.len(), 2);
    }

    #[test]
    fn test_build_export_map() {
        let prod_file = make_prod_file(
            "src/utils.ts",
            vec![
                ExportSymbol::new("formatDate".to_string(), "function", "named", Some(10)),
                ExportSymbol::new("parseDate".to_string(), "function", "named", Some(20)),
            ],
        );

        let prod_files = vec![&prod_file];
        let export_map = build_export_map(&prod_files);

        assert_eq!(export_map.len(), 1);
        assert!(export_map.contains_key("src/utils.ts"));
        let symbols = &export_map["src/utils.ts"];
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].0, "formatDate");
        assert_eq!(symbols[1].0, "parseDate");
    }

    #[test]
    fn test_symbol_coverage_basic() {
        let prod_file = make_prod_file(
            "src/utils.ts",
            vec![ExportSymbol::new(
                "formatDate".to_string(),
                "function",
                "named",
                Some(10),
            )],
        );

        let test_file = make_test_file(
            "src/__tests__/utils.test.ts",
            vec![make_import("src/utils.ts", vec!["formatDate"])],
        );

        let prod_files = vec![&prod_file];
        let test_files = vec![&test_file];
        let prod_exports = build_export_map(&prod_files);

        let coverage_map = build_symbol_coverage_map(&test_files, &prod_files, &prod_exports);

        let key = "src/utils.ts::formatDate";
        assert!(coverage_map.contains_key(key));
        let covered_by = &coverage_map[key];
        assert_eq!(covered_by.len(), 1);
        assert!(covered_by.contains(&PathBuf::from("src/__tests__/utils.test.ts")));
    }

    #[test]
    fn test_uncovered_exports() {
        let prod_file = make_prod_file(
            "src/utils.ts",
            vec![
                ExportSymbol::new("covered".to_string(), "function", "named", Some(10)),
                ExportSymbol::new("uncovered".to_string(), "function", "named", Some(20)),
            ],
        );

        let test_file = make_test_file(
            "src/__tests__/utils.test.ts",
            vec![make_import("src/utils.ts", vec!["covered"])],
        );

        let prod_files = vec![&prod_file];
        let test_files = vec![&test_file];
        let prod_exports = build_export_map(&prod_files);
        let coverage_map = build_symbol_coverage_map(&test_files, &prod_files, &prod_exports);

        let uncovered = find_uncovered_exports(&prod_files, &coverage_map);

        assert_eq!(uncovered.len(), 1);
        assert_eq!(uncovered[0].symbol, "uncovered");
        assert_eq!(uncovered[0].defined_in, PathBuf::from("src/utils.ts"));
        assert!(!uncovered[0].is_covered);
    }

    /// H4(b) RED: `pub use std::process::Command;` surfaces in
    /// `file.exports` as `kind == "reexport"`, but it forwards a stdlib item
    /// - it must not be reported as an untested app export.
    #[test]
    fn find_uncovered_exports_skips_stdlib_pub_use_reexport() {
        let mut prod_file = make_prod_file(
            "src/util.rs",
            vec![ExportSymbol::new(
                "Command".to_string(),
                "reexport",
                "named",
                None,
            )],
        );
        prod_file.reexports.push(ReexportEntry {
            source: "std::process::Command".to_string(),
            kind: crate::types::ReexportKind::Named(vec![(
                "Command".to_string(),
                "Command".to_string(),
            )]),
            resolved: None,
        });

        let prod_files = vec![&prod_file];
        let test_imports = HashMap::new();
        let uncovered = find_uncovered_exports(&prod_files, &test_imports);

        assert!(
            !uncovered.iter().any(|u| u.symbol == "Command"),
            "stdlib pub-use reexport must not show up as an untested export, found: {:?}",
            uncovered
        );
    }

    /// N1 (round-2): re-exports (local or stdlib) no longer appear in
    /// `exports_without_tests` at all - the kind allowlist
    /// (function/method only) supersedes the round-1 H4(b) guard that kept
    /// local `pub use` re-exports visible. The H4(b) stdlib filter stays as
    /// defense-in-depth for any future kind the allowlist might admit.
    #[test]
    fn find_uncovered_exports_skips_reexports_by_kind() {
        let mut prod_file = make_prod_file(
            "src/lib.rs",
            vec![ExportSymbol::new(
                "PublicThing".to_string(),
                "reexport",
                "named",
                None,
            )],
        );
        prod_file.reexports.push(ReexportEntry {
            source: "crate::inner::PublicThing".to_string(),
            kind: crate::types::ReexportKind::Named(vec![(
                "PublicThing".to_string(),
                "PublicThing".to_string(),
            )]),
            resolved: None,
        });

        let prod_files = vec![&prod_file];
        let test_imports = HashMap::new();
        let uncovered = find_uncovered_exports(&prod_files, &test_imports);

        assert!(
            !uncovered.iter().any(|u| u.symbol == "PublicThing"),
            "reexports are outside the testable-kind allowlist (round-2 N1), found: {:?}",
            uncovered
        );
    }

    /// N1 (round-2): struct/decl/extern exports are not "uncovered exports";
    /// an uncovered function still fires.
    #[test]
    fn find_uncovered_exports_skips_non_function_kinds() {
        let prod_file = make_prod_file(
            "src/lib.rs",
            vec![
                ExportSymbol::new("AppConfig".to_string(), "struct", "named", Some(1)),
                ExportSymbol::new("MAX_RETRIES".to_string(), "decl", "named", Some(2)),
                ExportSymbol::new("ax_copy_value".to_string(), "extern", "named", Some(3)),
                ExportSymbol::new("real_untested".to_string(), "function", "named", Some(4)),
            ],
        );

        let prod_files = vec![&prod_file];
        let uncovered = find_uncovered_exports(&prod_files, &HashMap::new());

        for type_like in ["AppConfig", "MAX_RETRIES", "ax_copy_value"] {
            assert!(
                !uncovered.iter().any(|u| u.symbol == type_like),
                "non-function export `{type_like}` must not be listed as uncovered, found: {:?}",
                uncovered
            );
        }
        assert!(
            uncovered.iter().any(|u| u.symbol == "real_untested"),
            "an uncovered function must still be listed, found: {:?}",
            uncovered
        );
    }

    /// N1 (round-2): the coverage percentage denominator counts only
    /// function/method exports - types must not inflate or deflate it.
    #[test]
    fn coverage_percent_counts_only_testable_kinds() {
        let prod_file = make_prod_file(
            "src/utils.rs",
            vec![
                ExportSymbol::new("covered".to_string(), "function", "named", Some(1)),
                ExportSymbol::new("uncovered".to_string(), "function", "named", Some(2)),
                ExportSymbol::new("SomeStruct".to_string(), "struct", "named", Some(3)),
                ExportSymbol::new("SOME_CONST".to_string(), "decl", "named", Some(4)),
            ],
        );
        let test_file = make_test_file(
            "tests/utils_test.rs",
            vec![make_import("src/utils.rs", vec!["covered"])],
        );

        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        snapshot.files = vec![prod_file, test_file];

        let report = analyze_test_coverage(&snapshot);
        assert_eq!(
            report.coverage_percent, 50.0,
            "1 of 2 function exports covered; struct/const must not enter the ratio"
        );
    }

    /// H4(c) RED: a Rust file with an inline `#[cfg(test)]` module must not
    /// read as 0% covered / all-exports-uncovered just because no separate
    /// test FILE imports it.
    #[test]
    fn test_inline_cfg_test_module_credits_own_file_coverage() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lib.rs");
        std::fs::write(
            &path,
            r#"
pub fn add(a: i32, b: i32) -> i32 { a + b }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds() { assert_eq!(add(1, 2), 3); }
}
"#,
        )
        .unwrap();
        let path_str = path.to_string_lossy().to_string();

        let mut prod_file = FileAnalysis::new(path_str);
        prod_file.kind = "code".to_string();
        prod_file.language = "rs".to_string();
        prod_file.exports = vec![ExportSymbol::new(
            "add".to_string(),
            "function",
            "named",
            Some(2),
        )];

        let mut snapshot = Snapshot::new(vec![tmp.path().to_string_lossy().to_string()]);
        snapshot.files = vec![prod_file];

        let report = analyze_test_coverage(&snapshot);

        assert_eq!(
            report.exports_without_tests.len(),
            0,
            "inline #[cfg(test)]-tested export should not count as uncovered, got: {:?}",
            report.exports_without_tests
        );
    }

    #[test]
    fn test_analyze_test_coverage() {
        let prod_file = make_prod_file(
            "src/utils.ts",
            vec![ExportSymbol::new(
                "formatDate".to_string(),
                "function",
                "named",
                Some(10),
            )],
        );

        let test_file = make_test_file(
            "src/__tests__/utils.test.ts",
            vec![make_import("src/utils.ts", vec!["formatDate"])],
        );

        let mut snapshot = Snapshot::new(vec!["src".to_string()]);
        snapshot.files = vec![prod_file, test_file];

        let report = analyze_test_coverage(&snapshot);

        assert_eq!(report.test_file_count, 1);
        assert_eq!(report.prod_file_count, 1);
        assert_eq!(report.coverage_percent, 100.0);
        assert_eq!(report.exports_without_tests.len(), 0);
    }
    /// N07 (audit): a file with an inline test module exercising ONLY `add`
    /// must not hand coverage to the untouched `other` export.
    #[test]
    fn inline_test_credit_is_per_symbol_not_per_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lib.rs");
        std::fs::write(
            &path,
            r#"
pub fn add(a: i32, b: i32) -> i32 { a + b }
pub fn other() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds() { assert_eq!(add(1, 2), 3); }
}
"#,
        )
        .unwrap();
        let path_str = path.to_string_lossy().to_string();

        let mut prod_file = FileAnalysis::new(path_str);
        prod_file.kind = "code".to_string();
        prod_file.language = "rs".to_string();
        prod_file.exports = vec![
            ExportSymbol::new("add".to_string(), "function", "named", Some(2)),
            ExportSymbol::new("other".to_string(), "function", "named", Some(3)),
        ];

        let mut snapshot = Snapshot::new(vec![tmp.path().to_string_lossy().to_string()]);
        snapshot.files = vec![prod_file];

        let report = analyze_test_coverage(&snapshot);
        assert!(
            report
                .exports_without_tests
                .iter()
                .any(|u| u.symbol == "other"),
            "`other` is never touched by the inline test and must stay uncovered, got: {:?}",
            report.exports_without_tests
        );
        assert!(
            !report
                .exports_without_tests
                .iter()
                .any(|u| u.symbol == "add"),
            "`add` is exercised by the inline test and must be covered"
        );
    }

    /// N07 (audit): an EMPTY `#[cfg(test)] mod tests {}` proves nothing.
    #[test]
    fn empty_inline_test_module_credits_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lib.rs");
        std::fs::write(
            &path,
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\n#[cfg(test)]\nmod tests {}\n",
        )
        .unwrap();
        let path_str = path.to_string_lossy().to_string();

        let mut prod_file = FileAnalysis::new(path_str);
        prod_file.kind = "code".to_string();
        prod_file.language = "rs".to_string();
        prod_file.exports = vec![ExportSymbol::new(
            "add".to_string(),
            "function",
            "named",
            Some(1),
        )];

        let mut snapshot = Snapshot::new(vec![tmp.path().to_string_lossy().to_string()]);
        snapshot.files = vec![prod_file];

        let report = analyze_test_coverage(&snapshot);
        assert!(
            report
                .exports_without_tests
                .iter()
                .any(|u| u.symbol == "add"),
            "an empty test module must not credit any export, got: {:?}",
            report.exports_without_tests
        );
    }
}
