//! Tauri command coverage analysis.
//!
//! Matches frontend `invoke()` calls with backend `#[tauri::command]` handlers.
//! Identifies missing handlers (FE calls without BE impl) and unused handlers
//! (BE impl without FE calls).

use std::collections::{HashMap, HashSet};

use globset::GlobSet;
use heck::ToSnakeCase;
use regex::Regex;
use std::sync::OnceLock;

use super::report::{CommandGap, Confidence, StringLiteralMatch};
use crate::types::FileAnalysis;

pub type CommandUsage = HashMap<String, Vec<(String, usize, String)>>;

/// Normalize a command name for comparison (snake_case, lowercase, alphanumeric only)
/// Used to match FE calls (camelCase) with BE handlers (snake_case)
pub fn normalize_cmd_name(name: &str) -> String {
    let mut buffered = String::new();
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            buffered.push(ch);
        } else {
            buffered.push('_');
        }
    }
    buffered
        .to_snake_case()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

fn strip_excluded_paths(
    paths: &[(String, usize, String)],
    focus: &Option<GlobSet>,
    exclude: &Option<GlobSet>,
) -> Vec<(String, usize)> {
    paths
        .iter()
        .filter_map(|(p, line, _)| {
            let pb = std::path::Path::new(p);
            if let Some(ex) = exclude
                && ex.is_match(pb)
            {
                return None;
            }
            if let Some(focus_globs) = focus
                && !focus_globs.is_match(pb)
            {
                return None;
            }
            Some((p.clone(), *line))
        })
        .collect()
}

/// Regex for finding string literals in frontend code.
/// Reserved for future content-based scanning.
fn regex_string_literal() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"['"]([a-z][a-z0-9_]*)['"]"#).expect("valid regex"))
}

/// Scan frontend files for string literals matching a handler name.
/// Returns matches that might indicate dynamic invoke usage.
pub fn find_string_literal_matches(
    handler_name: &str,
    analyses: &[FileAnalysis],
) -> Vec<StringLiteralMatch> {
    let mut matches = Vec::new();
    let normalized = normalize_cmd_name(handler_name);

    // Generate search variations (snake_case, camelCase, with/without _command)
    let variations: HashSet<String> = {
        let mut v = HashSet::new();
        v.insert(handler_name.to_string());
        v.insert(normalized.clone());

        // snake_case variant
        let snake = handler_name.chars().fold(String::new(), |mut acc, c| {
            if c.is_ascii_uppercase() && !acc.is_empty() {
                acc.push('_');
            }
            acc.push(c.to_ascii_lowercase());
            acc
        });
        v.insert(snake.clone());
        v.insert(normalize_cmd_name(&snake));

        // Without _command suffix
        if let Some(stripped) = handler_name.strip_suffix("_command") {
            v.insert(stripped.to_string());
            v.insert(normalize_cmd_name(stripped));
        }
        v
    };

    for analysis in analyses {
        // Skip Rust files - we want frontend string literals
        if analysis.path.ends_with(".rs") {
            continue;
        }

        // Check exports that might be allowlists or command constants
        for export in &analysis.exports {
            let export_lower = export.name.to_lowercase();
            if variations.iter().any(|v| export_lower.contains(v)) {
                matches.push(StringLiteralMatch {
                    file: analysis.path.clone(),
                    line: export.line.unwrap_or(0),
                    context: "export/const".to_string(),
                });
            }
        }

        // Check event constants that might reference handler names
        for (const_name, const_val) in &analysis.event_consts {
            let val_normalized = normalize_cmd_name(const_val);
            if variations.contains(&val_normalized)
                || variations.iter().any(|v| const_val.contains(v))
            {
                matches.push(StringLiteralMatch {
                    file: analysis.path.clone(),
                    line: 0, // Line not available from event_consts
                    context: format!("const {} = '{}'", const_name, const_val),
                });
            }
        }

        // Check string literals (registry-style arrays/objects)
        for lit in &analysis.string_literals {
            let val_normalized = normalize_cmd_name(&lit.value);
            if variations.contains(&val_normalized)
                || variations
                    .iter()
                    .any(|v| lit.value.contains(v) || val_normalized.contains(v))
            {
                matches.push(StringLiteralMatch {
                    file: analysis.path.clone(),
                    line: lit.line,
                    context: format!("string \"{}\"", lit.value),
                });
            }
        }
    }

    matches
}

/// Scan raw file content for string literal occurrences of handler name.
/// This is a more thorough scan that finds any string literal matching.
/// Reserved for future content-based scanning beyond analysis data.
pub fn scan_content_for_handler_literals(
    handler_name: &str,
    content: &str,
    file_path: &str,
) -> Vec<StringLiteralMatch> {
    let mut matches = Vec::new();
    let normalized = normalize_cmd_name(handler_name);

    for caps in regex_string_literal().captures_iter(content) {
        if let Some(lit) = caps.get(1) {
            let lit_str = lit.as_str();
            let lit_normalized = normalize_cmd_name(lit_str);

            if lit_normalized == normalized {
                // Calculate line number
                let line = content[..lit.start()].matches('\n').count() + 1;
                matches.push(StringLiteralMatch {
                    file: file_path.to_string(),
                    line,
                    context: format!("string_literal '{}'", lit_str),
                });
            }
        }
    }

    matches
}

pub fn compute_command_gaps(
    fe_commands: &CommandUsage,
    be_commands: &CommandUsage,
    focus_set: &Option<GlobSet>,
    exclude_set: &Option<GlobSet>,
) -> (Vec<CommandGap>, Vec<CommandGap>) {
    compute_command_gaps_with_confidence(fe_commands, be_commands, focus_set, exclude_set, &[])
}

/// Compute command gaps with confidence scoring based on string literal analysis.
pub fn compute_command_gaps_with_confidence(
    fe_commands: &CommandUsage,
    be_commands: &CommandUsage,
    focus_set: &Option<GlobSet>,
    exclude_set: &Option<GlobSet>,
    analyses: &[FileAnalysis],
) -> (Vec<CommandGap>, Vec<CommandGap>) {
    let fe_norms: HashMap<String, String> = fe_commands
        .keys()
        .map(|k| (k.clone(), normalize_cmd_name(k)))
        .collect();
    let be_norms: HashMap<String, String> = be_commands
        .keys()
        .map(|k| (k.clone(), normalize_cmd_name(k)))
        .collect();
    let be_norm_set: HashSet<String> = be_norms.values().cloned().collect();
    let fe_norm_set: HashSet<String> = fe_norms.values().cloned().collect();

    let missing_handlers: Vec<CommandGap> = fe_commands
        .iter()
        .filter_map(|(name, locs)| {
            let norm = fe_norms
                .get(name)
                .cloned()
                .unwrap_or_else(|| normalize_cmd_name(name));
            if be_norm_set.contains(&norm) {
                return None;
            }
            let kept = strip_excluded_paths(locs, focus_set, exclude_set);
            if kept.is_empty() {
                None
            } else {
                let impl_name = locs
                    .iter()
                    .find(|(p, l, _)| p == &kept[0].0 && *l == kept[0].1)
                    .map(|(_, _, n)| n.clone());
                Some(CommandGap {
                    name: name.clone(),
                    implementation_name: impl_name,
                    locations: kept,
                    confidence: None, // Missing handlers don't have confidence
                    string_literal_matches: Vec::new(),
                })
            }
        })
        .collect();

    let unused_handlers: Vec<CommandGap> = be_commands
        .iter()
        .filter_map(|(name, locs)| {
            let norm = be_norms
                .get(name)
                .cloned()
                .unwrap_or_else(|| normalize_cmd_name(name));
            if fe_norm_set.contains(&norm) {
                return None;
            }
            let kept = strip_excluded_paths(locs, focus_set, exclude_set);
            if kept.is_empty() {
                None
            } else {
                let impl_name = locs
                    .iter()
                    .find(|(p, l, _)| p == &kept[0].0 && *l == kept[0].1)
                    .map(|(_, _, n)| n.clone());

                // Find string literal matches for confidence scoring
                let string_literal_matches = find_string_literal_matches(name, analyses);
                let confidence = if string_literal_matches.is_empty() {
                    Confidence::High
                } else {
                    Confidence::Smell // String literals suggest possible dynamic usage
                };

                Some(CommandGap {
                    name: name.clone(),
                    implementation_name: impl_name,
                    locations: kept,
                    confidence: Some(confidence),
                    string_literal_matches,
                })
            }
        })
        .collect();

    (missing_handlers, unused_handlers)
}

/// Compute gaps for backend handlers that are defined but never registered with Tauri.
///
/// `be_commands` is the full backend command usage map (including both registered and
/// unregistered handlers). `registered_impls` is the set of Rust function names that
/// are actually registered via `tauri::generate_handler![...]` across the project.
///
/// We treat a command name as "unregistered" if **none** of its implementation symbols
/// appear in `registered_impls`. Paths are filtered through `focus_set` / `exclude_set`
/// in the same way as in `compute_command_gaps`.
pub fn compute_unregistered_handlers(
    be_commands: &CommandUsage,
    registered_impls: &std::collections::HashSet<String>,
    focus_set: &Option<GlobSet>,
    exclude_set: &Option<GlobSet>,
) -> Vec<CommandGap> {
    be_commands
        .iter()
        .filter_map(|(name, locs)| {
            // If any impl symbol for this command is registered, skip it.
            let has_registered_impl = locs
                .iter()
                .any(|(_, _, impl_name)| registered_impls.contains(impl_name));
            if has_registered_impl {
                return None;
            }

            let kept = strip_excluded_paths(locs, focus_set, exclude_set);
            if kept.is_empty() {
                return None;
            }

            let impl_name = locs
                .iter()
                .find(|(p, l, _)| p == &kept[0].0 && *l == kept[0].1)
                .map(|(_, _, n)| n.clone());

            Some(CommandGap {
                name: name.clone(),
                implementation_name: impl_name,
                locations: kept,
                confidence: None, // Unregistered handlers don't have confidence scoring
                string_literal_matches: Vec::new(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use globset::{Glob, GlobSetBuilder};

    #[test]
    fn matches_commands_across_casing() {
        let mut fe: CommandUsage = HashMap::new();
        fe.insert(
            "fetchUserData".into(),
            vec![("src/fe.ts".into(), 10usize, "fetchUserData".into())],
        );
        let mut be: CommandUsage = HashMap::new();
        be.insert(
            "fetch_user_data".into(),
            vec![("src/be.rs".into(), 20usize, "fetch_user_data".into())],
        );
        let (missing, unused) = compute_command_gaps(&fe, &be, &None, &None);
        assert!(missing.is_empty(), "should detect matching handler");
        assert!(unused.is_empty(), "should detect frontend usage");
    }

    #[test]
    fn ignores_excluded_paths_before_gap_report() {
        let mut builder = GlobSetBuilder::new();
        builder.add(Glob::new("**/ignored/**").expect("valid glob"));
        let exclude_set = Some(builder.build().expect("build globset"));
        let mut fe: CommandUsage = HashMap::new();
        fe.insert(
            "audio-play".into(),
            vec![("ignored/fe.ts".into(), 5usize, "audio-play".into())],
        );
        let mut be: CommandUsage = HashMap::new();
        be.insert(
            "audio_play".into(),
            vec![("src/handler.rs".into(), 8usize, "audio_play".into())],
        );
        let (missing, unused) = compute_command_gaps(&fe, &be, &None, &exclude_set);
        assert!(missing.is_empty());
        assert!(unused.is_empty());
    }

    /// Tests that commands with rename attribute match correctly.
    /// Simulates: BE has `alpha_status_command` with `rename = "alpha_status"`,
    /// FE invokes `alpha_status`. They should match.
    #[test]
    fn matches_renamed_commands() {
        // When root_scan processes `#[tauri::command(rename = "alpha_status")]`,
        // it uses exposed_name ("alpha_status") as the key, not the function name.
        // So be_commands will have key "alpha_status", not "alpha_status_command".
        let mut fe: CommandUsage = HashMap::new();
        fe.insert(
            "alpha_status".into(),
            vec![("src/service.ts".into(), 42usize, "alpha_status".into())],
        );
        let mut be: CommandUsage = HashMap::new();
        // Key is the exposed name, impl_name is the function name
        be.insert(
            "alpha_status".into(),
            vec![(
                "src-tauri/src/commands/alpha_gate.rs".into(),
                15usize,
                "alpha_status_command".into(),
            )],
        );
        let (missing, unused) = compute_command_gaps(&fe, &be, &None, &None);
        assert!(
            missing.is_empty(),
            "FE invoke('alpha_status') should match BE handler with rename='alpha_status'"
        );
        assert!(
            unused.is_empty(),
            "BE alpha_status handler should be detected as used"
        );
    }

    /// Tests that suffix stripping doesn't break renamed commands.
    /// If someone uses `rename = "some_thing_command"` (unusual but valid),
    /// the _command suffix should still be stripped for matching.
    #[test]
    fn suffix_stripping_on_exposed_name() {
        let mut fe: CommandUsage = HashMap::new();
        fe.insert(
            "some_thing".into(),
            vec![("src/app.ts".into(), 10usize, "some_thing".into())],
        );
        let mut be: CommandUsage = HashMap::new();
        // Edge case: exposed name has _command suffix (will be stripped)
        be.insert(
            "some_thing".into(), // After suffix stripping in root_scan
            vec![("src-tauri/handler.rs".into(), 5usize, "impl_fn".into())],
        );
        let (missing, unused) = compute_command_gaps(&fe, &be, &None, &None);
        assert!(missing.is_empty());
        assert!(unused.is_empty());
    }

    /// Tests confidence scoring for unused handlers.
    /// HIGH confidence = no string literal matches found.
    /// SMELL confidence = string literal matches found (may be dynamic usage).
    #[test]
    fn confidence_scoring_for_unused_handlers() {
        use super::Confidence;
        use crate::types::{ExportSymbol, FileAnalysis};

        let fe: CommandUsage = HashMap::new();
        // No FE usage - both BE handlers are unused
        let mut be: CommandUsage = HashMap::new();
        be.insert(
            "truly_unused".into(),
            vec![("src-tauri/cmd.rs".into(), 10usize, "truly_unused".into())],
        );
        be.insert(
            "get_pin_status".into(),
            vec![("src-tauri/cmd.rs".into(), 20usize, "get_pin_status".into())],
        );

        // Create analyses with exports that reference one handler name
        let mut analysis = FileAnalysis::new("src/commands.ts".into());
        analysis.exports.push(ExportSymbol::new(
            "GET_PIN_STATUS_CMD".into(), // Contains handler name
            "const",
            "named",
            Some(5),
        ));

        let (missing, unused) =
            compute_command_gaps_with_confidence(&fe, &be, &None, &None, &[analysis]);

        assert!(missing.is_empty());
        assert_eq!(unused.len(), 2);

        // Find handlers by name
        let truly_unused = unused.iter().find(|g| g.name == "truly_unused").unwrap();
        let pin_status = unused.iter().find(|g| g.name == "get_pin_status").unwrap();

        // truly_unused should have HIGH confidence (no string literal matches)
        assert_eq!(truly_unused.confidence, Some(Confidence::High));
        assert!(truly_unused.string_literal_matches.is_empty());

        // get_pin_status should have SMELL confidence (string literal match found)
        assert_eq!(pin_status.confidence, Some(Confidence::Smell));
        assert!(!pin_status.string_literal_matches.is_empty());
    }

    #[test]
    fn test_normalize_cmd_name() {
        // Basic snake_case normalization
        assert_eq!(normalize_cmd_name("get_user"), "getuser");
        assert_eq!(normalize_cmd_name("getUser"), "getuser");
        assert_eq!(normalize_cmd_name("GetUser"), "getuser");

        // With special characters
        assert_eq!(normalize_cmd_name("get-user"), "getuser");
        assert_eq!(normalize_cmd_name("get.user"), "getuser");
        assert_eq!(normalize_cmd_name("get::user"), "getuser");

        // Numbers preserved
        assert_eq!(normalize_cmd_name("get_user_v2"), "getuserv2");
        assert_eq!(normalize_cmd_name("http2_request"), "http2request");
    }

    #[test]
    fn test_strip_excluded_paths_with_focus() {
        let paths = vec![
            ("src/api.ts".to_string(), 10, "api".to_string()),
            ("lib/utils.ts".to_string(), 20, "utils".to_string()),
            ("test/mock.ts".to_string(), 30, "mock".to_string()),
        ];

        // Focus on src only
        let mut builder = GlobSetBuilder::new();
        builder.add(Glob::new("src/**").expect("valid glob"));
        let focus = Some(builder.build().expect("build globset"));

        let result = strip_excluded_paths(&paths, &focus, &None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "src/api.ts");
    }

    #[test]
    fn test_strip_excluded_paths_with_exclude() {
        let paths = vec![
            ("src/api.ts".to_string(), 10, "api".to_string()),
            (
                "node_modules/pkg/index.ts".to_string(),
                20,
                "pkg".to_string(),
            ),
            ("src/main.ts".to_string(), 30, "main".to_string()),
        ];

        // Exclude node_modules
        let mut builder = GlobSetBuilder::new();
        builder.add(Glob::new("node_modules/**").expect("valid glob"));
        let exclude = Some(builder.build().expect("build globset"));

        let result = strip_excluded_paths(&paths, &None, &exclude);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|(p, _)| !p.contains("node_modules")));
    }

    #[test]
    fn test_strip_excluded_paths_both() {
        let paths = vec![
            ("src/api.ts".to_string(), 10, "api".to_string()),
            ("src/test/mock.ts".to_string(), 20, "mock".to_string()),
            ("lib/utils.ts".to_string(), 30, "utils".to_string()),
        ];

        // Focus on src, exclude test
        let mut focus_builder = GlobSetBuilder::new();
        focus_builder.add(Glob::new("src/**").expect("valid glob"));
        let focus = Some(focus_builder.build().expect("build globset"));

        let mut exclude_builder = GlobSetBuilder::new();
        exclude_builder.add(Glob::new("**/test/**").expect("valid glob"));
        let exclude = Some(exclude_builder.build().expect("build globset"));

        let result = strip_excluded_paths(&paths, &focus, &exclude);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "src/api.ts");
    }

    #[test]
    fn test_find_string_literal_matches_event_consts() {
        use crate::types::FileAnalysis;

        let mut analysis = FileAnalysis::new("src/events.ts".into());
        analysis
            .event_consts
            .insert("FETCH_USER_EVENT".to_string(), "fetch_user".to_string());

        let matches = find_string_literal_matches("fetch_user", &[analysis]);
        assert_eq!(matches.len(), 1);
        assert!(matches[0].context.contains("FETCH_USER_EVENT"));
    }

    #[test]
    fn test_find_string_literal_matches_skips_rust() {
        use crate::types::{ExportSymbol, FileAnalysis};

        let mut rust_analysis = FileAnalysis::new("src-tauri/src/handlers.rs".into());
        rust_analysis.exports.push(ExportSymbol::new(
            "get_user".into(),
            "fn",
            "named",
            Some(10),
        ));

        let matches = find_string_literal_matches("get_user", &[rust_analysis]);
        assert!(matches.is_empty()); // Should skip .rs files
    }

    #[test]
    fn test_find_string_literal_matches_with_command_suffix() {
        use crate::types::{ExportSymbol, FileAnalysis};

        let mut analysis = FileAnalysis::new("src/commands.ts".into());
        analysis.exports.push(ExportSymbol::new(
            "save_config_command".into(),
            "const",
            "named",
            Some(5),
        ));

        // Search with _command suffix - should find match
        let matches = find_string_literal_matches("save_config_command", &[analysis.clone()]);
        assert!(!matches.is_empty());

        // Search without _command suffix - should also find (due to variation generation)
        let matches2 = find_string_literal_matches("save_config", &[analysis]);
        // The export "save_config_command" contains "save_config"
        assert!(!matches2.is_empty());
    }

    #[test]
    fn test_compute_unregistered_handlers_basic() {
        let mut be: CommandUsage = HashMap::new();
        be.insert(
            "registered_handler".into(),
            vec![("src-tauri/cmd.rs".into(), 10, "registered_handler".into())],
        );
        be.insert(
            "unregistered_handler".into(),
            vec![("src-tauri/cmd.rs".into(), 20, "unregistered_handler".into())],
        );

        let registered: HashSet<String> = ["registered_handler".to_string()].into_iter().collect();

        let unregistered = compute_unregistered_handlers(&be, &registered, &None, &None);
        assert_eq!(unregistered.len(), 1);
        assert_eq!(unregistered[0].name, "unregistered_handler");
    }

    #[test]
    fn test_compute_unregistered_handlers_all_registered() {
        let mut be: CommandUsage = HashMap::new();
        be.insert(
            "handler_a".into(),
            vec![("src-tauri/cmd.rs".into(), 10, "handler_a".into())],
        );
        be.insert(
            "handler_b".into(),
            vec![("src-tauri/cmd.rs".into(), 20, "handler_b".into())],
        );

        let registered: HashSet<String> = ["handler_a".to_string(), "handler_b".to_string()]
            .into_iter()
            .collect();

        let unregistered = compute_unregistered_handlers(&be, &registered, &None, &None);
        assert!(unregistered.is_empty());
    }

    #[test]
    fn test_compute_unregistered_handlers_with_exclude() {
        let mut be: CommandUsage = HashMap::new();
        be.insert(
            "test_handler".into(),
            vec![("test/mock.rs".into(), 10, "test_handler".into())],
        );

        let registered: HashSet<String> = HashSet::new();

        let mut exclude_builder = GlobSetBuilder::new();
        exclude_builder.add(Glob::new("test/**").expect("valid glob"));
        let exclude = Some(exclude_builder.build().expect("build globset"));

        let unregistered = compute_unregistered_handlers(&be, &registered, &None, &exclude);
        assert!(unregistered.is_empty()); // Excluded by path
    }

    #[test]
    fn test_compute_command_gaps_missing_handler() {
        let mut fe: CommandUsage = HashMap::new();
        fe.insert(
            "missing_handler".into(),
            vec![("src/app.ts".into(), 10, "missing_handler".into())],
        );

        let be: CommandUsage = HashMap::new(); // No backend handlers

        let (missing, unused) = compute_command_gaps(&fe, &be, &None, &None);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].name, "missing_handler");
        assert!(unused.is_empty());
    }

    #[test]
    fn test_compute_command_gaps_unused_handler() {
        let fe: CommandUsage = HashMap::new(); // No frontend usage

        let mut be: CommandUsage = HashMap::new();
        be.insert(
            "unused_handler".into(),
            vec![("src-tauri/cmd.rs".into(), 10, "unused_handler".into())],
        );

        let (missing, unused) = compute_command_gaps(&fe, &be, &None, &None);
        assert!(missing.is_empty());
        assert_eq!(unused.len(), 1);
        assert_eq!(unused[0].name, "unused_handler");
    }

    #[test]
    fn test_scan_content_for_handler_literals() {
        let content = r#"
            const handler = 'get_user';
            invoke('get_user');
            const other = "different";
        "#;

        let matches = scan_content_for_handler_literals("get_user", content, "src/test.ts");
        assert_eq!(matches.len(), 2); // Two occurrences of 'get_user'
        assert!(matches.iter().all(|m| m.file == "src/test.ts"));
    }

    #[test]
    fn test_scan_content_no_matches() {
        let content = r#"
            const handler = 'other_handler';
            invoke('different_command');
        "#;

        let matches = scan_content_for_handler_literals("get_user", content, "src/test.ts");
        assert!(matches.is_empty());
    }
}
