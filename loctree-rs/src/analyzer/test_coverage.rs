//! Test coverage analysis - structural coverage based on imports
//!
//! Analyzes which production code is actually imported by tests.
//! This provides "structural coverage" - what code is touched by test imports,
//! without running tests or inspecting runtime behavior.

use crate::snapshot::Snapshot;
use crate::types::FileAnalysis;
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
    let symbol_coverage = build_symbol_coverage_map(&test_files, &prod_files, &prod_exports);

    // Analyze handlers (Tauri commands)
    let handlers = analyze_handler_coverage(snapshot, &symbol_coverage);

    // Find exports without test coverage
    let exports_without_tests = find_uncovered_exports(&prod_files, &symbol_coverage);

    // Calculate overall coverage percentage
    let total_exports: usize = prod_exports.values().map(|syms| syms.len()).sum();
    let covered_exports = symbol_coverage.len();
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

/// Find production exports that have no test coverage
fn find_uncovered_exports(
    prod_files: &[&FileAnalysis],
    symbol_coverage: &HashMap<String, HashSet<PathBuf>>,
) -> Vec<SymbolCoverage> {
    let mut uncovered = Vec::new();

    for file in prod_files {
        for export in &file.exports {
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
    use crate::types::{ExportSymbol, ImportEntry, ImportKind, ImportSymbol};

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
}
