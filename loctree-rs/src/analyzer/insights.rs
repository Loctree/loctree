use std::collections::HashMap;
use std::path::Path;

use super::for_ai::has_tauri_stack;
use super::{AiInsight, CommandGap, RankedDup};
use crate::types::FileAnalysis;

/// Detect files with the same stem (filename without extension) across different languages.
/// This helps identify potential binding pairs (e.g., py/ts/rs files that wrap the same functionality).
fn find_cross_lang_stem_matches(files: &[FileAnalysis]) -> Vec<(String, Vec<(String, String)>)> {
    let binding_langs: &[&str] = &["py", "ts", "rs", "js"];

    // Group files by stem -> Vec<(path, language)>
    let mut stem_map: HashMap<String, Vec<(String, String)>> = HashMap::new();

    for file in files {
        // Skip test/generated files
        if file.is_test || file.is_generated {
            continue;
        }

        // Only consider binding-relevant languages
        if !binding_langs.contains(&file.language.as_str()) {
            continue;
        }

        let path = Path::new(&file.path);
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            // Skip common generic names that would create noise
            let lower_stem = stem.to_lowercase();
            if matches!(
                lower_stem.as_str(),
                "index" | "mod" | "lib" | "main" | "utils" | "helpers" | "types" | "constants"
            ) {
                continue;
            }

            stem_map
                .entry(stem.to_string())
                .or_default()
                .push((file.path.clone(), file.language.clone()));
        }
    }

    // Filter to only stems with multiple languages
    let mut matches: Vec<(String, Vec<(String, String)>)> = stem_map
        .into_iter()
        .filter(|(_, entries)| {
            let langs: std::collections::HashSet<_> = entries.iter().map(|(_, l)| l).collect();
            langs.len() > 1 // At least 2 different languages
        })
        .collect();

    // Sort for deterministic output
    matches.sort_by(|a, b| a.0.cmp(&b.0));
    matches
}

pub fn collect_ai_insights(
    files: &[FileAnalysis],
    dups: &[RankedDup],
    cascades: &[(String, String)],
    gap_missing: &[CommandGap],
    _gap_unused: &[CommandGap],
) -> Vec<AiInsight> {
    let mut insights = Vec::new();

    // Cross-language stem hint (Objective 7)
    let cross_lang_matches = find_cross_lang_stem_matches(files);
    if !cross_lang_matches.is_empty() {
        let examples: Vec<String> = cross_lang_matches
            .iter()
            .take(5)
            .map(|(stem, entries)| {
                let langs: Vec<_> = entries.iter().map(|(_, l)| l.as_str()).collect();
                format!("'{}' ({})", stem, langs.join("/"))
            })
            .collect();

        insights.push(AiInsight {
            title: "Potential cross-language binding pairs".to_string(),
            severity: "info".to_string(),
            message: format!(
                "Found {} file stem(s) shared across languages: {}. These may be binding pairs (e.g., Python/Rust FFI or TS/Rust Tauri commands). Check if they should share types/interfaces.",
                cross_lang_matches.len(),
                examples.join(", ")
            ),
        });
    }

    let huge_files: Vec<_> = files.iter().filter(|f| f.loc > 2000).collect();
    if !huge_files.is_empty() {
        insights.push(AiInsight {
            title: "Huge files detected".to_string(),
            severity: "medium".to_string(),
            message: format!(
                "Found {} files with > 2000 LOC (e.g. {}). Consider splitting them.",
                huge_files.len(),
                huge_files[0].path
            ),
        });
    }

    if dups.len() > 10 {
        insights.push(AiInsight {
            title: "High number of duplicate exports".to_string(),
            severity: "medium".to_string(),
            message: format!(
                "Found {} duplicate export groups. Consider refactoring.",
                dups.len()
            ),
        });
    }

    if cascades.len() > 20 {
        insights.push(AiInsight {
            title: "Many re-export chains".to_string(),
            severity: "low".to_string(),
            message: format!(
                "Found {} re-export cascades. This might affect tree-shaking/bundling.",
                cascades.len()
            ),
        });
    }

    // Gate Tauri-specific insight on actual Tauri stack presence.
    // loctree-feedback hak 2026-05-18 Screenscribe HAK 2: pure Python/JS
    // projects with custom JS event dispatch were getting false HIGH
    // severity "Missing Tauri Handlers" because the gap-detector picks
    // up any `invoke('foo')`-like call. Reuse the `has_tauri_stack`
    // heuristic from `for_ai` (already powering `extract_quick_wins`).
    if !gap_missing.is_empty() && has_tauri_stack(files) {
        insights.push(AiInsight {
            title: "Missing Tauri Handlers".to_string(),
            severity: "high".to_string(),
            message: format!(
                "Frontend calls {} commands that are missing in Backend.",
                gap_missing.len()
            ),
        });
    }

    insights
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::report::DupSeverity;

    fn mock_file(path: &str, language: &str, loc: usize) -> FileAnalysis {
        FileAnalysis {
            path: path.to_string(),
            language: language.to_string(),
            loc,
            ..Default::default()
        }
    }

    fn mock_file_test(path: &str, language: &str, loc: usize) -> FileAnalysis {
        FileAnalysis {
            path: path.to_string(),
            language: language.to_string(),
            loc,
            is_test: true,
            ..Default::default()
        }
    }

    #[test]
    fn test_cross_lang_stem_matches_finds_pairs() {
        let files = vec![
            mock_file("src/audio.rs", "rs", 100),
            mock_file("src/audio.ts", "ts", 50),
            mock_file("lib/player.py", "py", 80),
            mock_file("lib/player.ts", "ts", 60),
        ];

        let matches = find_cross_lang_stem_matches(&files);
        assert_eq!(matches.len(), 2);

        let stems: Vec<&str> = matches.iter().map(|(s, _)| s.as_str()).collect();
        assert!(stems.contains(&"audio"));
        assert!(stems.contains(&"player"));
    }

    #[test]
    fn test_cross_lang_stem_matches_ignores_generic_names() {
        let files = vec![
            mock_file("src/index.rs", "rs", 100),
            mock_file("src/index.ts", "ts", 50),
            mock_file("lib/utils.py", "py", 80),
            mock_file("lib/utils.ts", "ts", 60),
            mock_file("mod.rs", "rs", 10),
            mock_file("mod.py", "py", 10),
        ];

        let matches = find_cross_lang_stem_matches(&files);
        assert!(matches.is_empty(), "Should ignore generic names");
    }

    #[test]
    fn test_cross_lang_stem_matches_ignores_test_files() {
        let files = vec![
            mock_file_test("src/audio.rs", "rs", 100),
            mock_file("src/audio.ts", "ts", 50),
        ];

        let matches = find_cross_lang_stem_matches(&files);
        assert!(matches.is_empty(), "Should ignore test files");
    }

    #[test]
    fn test_cross_lang_stem_matches_ignores_same_lang() {
        let files = vec![
            mock_file("src/audio.ts", "ts", 100),
            mock_file("lib/audio.ts", "ts", 50),
        ];

        let matches = find_cross_lang_stem_matches(&files);
        assert!(matches.is_empty(), "Should not match same language");
    }

    #[test]
    fn test_collect_ai_insights_huge_files() {
        let files = vec![
            mock_file("src/huge.ts", "ts", 3000),
            mock_file("src/small.ts", "ts", 100),
        ];

        let insights = collect_ai_insights(&files, &[], &[], &[], &[]);

        assert!(insights.iter().any(|i| i.title.contains("Huge files")));
    }

    #[test]
    fn test_collect_ai_insights_many_dups() {
        let files = vec![mock_file("src/a.ts", "ts", 100)];

        let dups: Vec<RankedDup> = (0..15)
            .map(|i| RankedDup {
                name: format!("dup{}", i),
                files: vec![format!("file{}.ts", i)],
                locations: vec![],
                score: i,
                prod_count: 1,
                dev_count: 0,
                canonical: format!("file{}.ts", i),
                canonical_line: None,
                refactors: vec![],
                severity: DupSeverity::SamePackage,
                is_cross_lang: false,
                packages: vec![],
                reason: String::new(),
            })
            .collect();

        let insights = collect_ai_insights(&files, &dups, &[], &[], &[]);

        assert!(
            insights
                .iter()
                .any(|i| i.title.contains("duplicate exports"))
        );
    }

    #[test]
    fn test_collect_ai_insights_many_cascades() {
        let files = vec![mock_file("src/a.ts", "ts", 100)];

        let cascades: Vec<(String, String)> = (0..25)
            .map(|i| (format!("from{}.ts", i), format!("to{}.ts", i)))
            .collect();

        let insights = collect_ai_insights(&files, &[], &cascades, &[], &[]);

        assert!(
            insights
                .iter()
                .any(|i| i.title.contains("re-export chains"))
        );
    }

    /// Tauri-stack proxy fixture for tests that exercise the
    /// `Missing Tauri Handlers` insight. Matches the `has_tauri_stack`
    /// heuristic in `for_ai.rs` so the post-2026-05-25 gate is
    /// satisfied. Mirrors `for_ai::tests::tauri_stack_marker()`.
    fn tauri_stack_marker() -> FileAnalysis {
        FileAnalysis {
            path: "src-tauri/tauri.conf.json".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_collect_ai_insights_missing_handlers() {
        // With a real Tauri stack present, `Missing Tauri Handlers`
        // remains a HIGH-severity quick-win signal.
        let files = vec![mock_file("src/a.ts", "ts", 100), tauri_stack_marker()];

        let missing = vec![CommandGap {
            name: "missing_cmd".to_string(),
            implementation_name: None,
            locations: vec![("src/a.ts".to_string(), 10)],
            confidence: None,
            string_literal_matches: vec![],
        }];

        let insights = collect_ai_insights(&files, &[], &[], &missing, &[]);

        assert!(
            insights
                .iter()
                .any(|i| i.title.contains("Missing Tauri Handlers"))
        );
        assert!(insights.iter().any(|i| i.severity == "high"));
    }

    #[test]
    fn test_collect_ai_insights_skips_missing_handlers_when_no_tauri_stack() {
        // loctree-feedback hak 2026-05-18 Screenscribe HAK 2 regression
        // guard: pure Python/JS repo with custom JS event dispatch
        // produces non-empty `gap_missing`, but no `tauri.conf.json`,
        // no `src-tauri/`, no `@tauri-apps/` imports. The insight must
        // be suppressed — otherwise every non-Tauri project gets a
        // false HIGH severity recommendation to add a Tauri handler.
        let files = vec![
            mock_file("src/cli.py", "py", 200),
            mock_file("src/frontend.js", "js", 80),
        ];

        let missing = vec![
            CommandGap {
                name: "reattach-workspace".to_string(),
                implementation_name: None,
                locations: vec![("src/frontend.js".to_string(), 42)],
                confidence: None,
                string_literal_matches: vec![],
            },
            CommandGap {
                name: "seek-to-timestamp".to_string(),
                implementation_name: None,
                locations: vec![("src/frontend.js".to_string(), 60)],
                confidence: None,
                string_literal_matches: vec![],
            },
        ];

        let insights = collect_ai_insights(&files, &[], &[], &missing, &[]);

        assert!(
            !insights
                .iter()
                .any(|i| i.title.contains("Missing Tauri Handlers")),
            "non-Tauri repo with custom JS events must not surface 'Missing Tauri Handlers' insight; got: {:?}",
            insights
                .iter()
                .map(|i| (i.title.as_str(), i.severity.as_str()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_collect_ai_insights_empty_inputs() {
        let insights = collect_ai_insights(&[], &[], &[], &[], &[]);
        assert!(insights.is_empty());
    }

    #[test]
    fn test_cross_lang_with_generated_files() {
        let mut generated = mock_file("src/audio.rs", "rs", 100);
        generated.is_generated = true;

        let files = vec![generated, mock_file("src/audio.ts", "ts", 50)];

        let matches = find_cross_lang_stem_matches(&files);
        assert!(matches.is_empty(), "Should ignore generated files");
    }

    #[test]
    fn test_collect_ai_insights_cross_lang_binding() {
        // Create files with matching stems across languages
        let files = vec![
            mock_file("src/audio_processor.rs", "rs", 200),
            mock_file("src/audio_processor.ts", "ts", 150),
            mock_file("lib/video_encoder.py", "py", 100),
            mock_file("lib/video_encoder.rs", "rs", 120),
        ];

        let insights = collect_ai_insights(&files, &[], &[], &[], &[]);

        assert!(
            insights
                .iter()
                .any(|i| i.title.contains("cross-language binding"))
        );
        assert!(insights.iter().any(|i| i.severity == "info"));
    }
}
