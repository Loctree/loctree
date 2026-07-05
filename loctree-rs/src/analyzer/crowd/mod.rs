//! Crowd detection - find groups of files with similar import patterns
//!
//! Detects "functional crowds" - multiple files clustering around the same
//! functionality (e.g., 3 hooks handling "message" state, 2 components rendering messages).

pub mod clustering;
pub mod output;
pub mod similarity;
pub mod types;

pub use clustering::{cluster_by_name, detect_name_patterns};
pub use output::*;
pub use similarity::{
    build_import_sets, count_importers, count_importers_transitive, jaccard_similarity,
    similarity_matrix,
};
pub use types::*;

use crate::snapshot::GraphEdge;
use crate::types::FileAnalysis;

/// Detect a crowd around a specific pattern
/// Note: For accurate importer counts with re-exports, use detect_crowd_with_edges
pub fn detect_crowd(files: &[FileAnalysis], pattern: &str) -> Crowd {
    detect_crowd_internal(files, pattern, None)
}

/// Detect a crowd around a specific pattern with transitive importer counting
/// This follows re-export chains (barrel files) for accurate importer counts
pub fn detect_crowd_with_edges(
    files: &[FileAnalysis],
    pattern: &str,
    edges: &[GraphEdge],
) -> Crowd {
    detect_crowd_internal(files, pattern, Some(edges))
}

fn detect_crowd_internal(
    files: &[FileAnalysis],
    pattern: &str,
    edges: Option<&[GraphEdge]>,
) -> Crowd {
    // 1. Find files matching the pattern by name
    let matching_files = cluster_by_name(files, pattern);

    if matching_files.is_empty() {
        return Crowd {
            pattern: pattern.to_string(),
            members: vec![],
            score: 0.0,
            issues: vec![],
            context_type: None,
        };
    }

    // 2. Build import sets and count importers
    let import_sets = build_import_sets(files);

    // Use transitive counting if edges are available
    let importer_counts = if let Some(edges) = edges {
        count_importers_transitive(files, edges)
    } else {
        count_importers(files)
    };

    // 3. Calculate similarities between crowd members
    let similarities = similarity_matrix(&matching_files, &import_sets);

    // 4. Build crowd members with stats
    let mut members: Vec<CrowdMember> = matching_files
        .iter()
        .map(|path| {
            let importer_count = importer_counts.get(path).copied().unwrap_or(0);
            let similarity_scores: Vec<(String, f32)> = similarities
                .iter()
                .filter(|(a, b, _)| a == path || b == path)
                .map(|(a, b, score)| {
                    let other = if a == path { b } else { a };
                    (other.clone(), *score)
                })
                .collect();

            // Find the FileAnalysis for this path to get is_test flag
            let is_test = files
                .iter()
                .find(|f| &f.path == path)
                .map(|f| f.is_test)
                .unwrap_or(false);

            CrowdMember {
                file: path.clone(),
                match_reason: MatchReason::NameMatch {
                    matched: pattern.to_string(),
                },
                importer_count,
                similarity_scores,
                is_test,
            }
        })
        .collect();

    // Sort by importer count (most popular first)
    members.sort_by_key(|b| std::cmp::Reverse(b.importer_count));

    // 5. Detect issues
    let issues = detect_issues(&members, &similarities);

    // 6. Calculate crowd score
    let score = calculate_crowd_score(&members, &issues);

    // 7. Infer context type
    let context_type = infer_context_type(pattern, &members);

    Crowd {
        pattern: pattern.to_string(),
        members,
        score,
        issues,
        context_type,
    }
}

/// Detect issues in a crowd
fn detect_issues(
    members: &[CrowdMember],
    similarities: &[(String, String, f32)],
) -> Vec<CrowdIssue> {
    let mut issues = Vec::new();

    if members.len() < 2 {
        return issues;
    }

    // Check for usage asymmetry
    if let Some(primary) = members.first() {
        let underused: Vec<String> = members
            .iter()
            .skip(1)
            .filter(|m| m.importer_count <= primary.importer_count / 3)
            .map(|m| m.file.clone())
            .collect();

        if !underused.is_empty() {
            issues.push(CrowdIssue::UsageAsymmetry {
                primary: primary.file.clone(),
                underused,
            });
        }
    }

    // Check for high similarity (potential duplicates)
    let high_similarity_pairs: Vec<String> = similarities
        .iter()
        .filter(|(_, _, score)| *score > 0.6)
        .map(|(a, b, _)| {
            format!(
                "{} ↔ {}",
                a.rsplit('/').next().unwrap_or(a),
                b.rsplit('/').next().unwrap_or(b)
            )
        })
        .collect();

    if !high_similarity_pairs.is_empty() {
        issues.push(CrowdIssue::ExportOverlap {
            files: high_similarity_pairs.clone(),
            overlap: vec!["high import similarity".to_string()],
        });
    }

    // Check for name collisions (files with very similar names)
    let filenames: Vec<&str> = members
        .iter()
        .map(|m| m.file.rsplit('/').next().unwrap_or(&m.file))
        .collect();

    if members.len() >= 3 {
        issues.push(CrowdIssue::NameCollision {
            files: filenames.iter().map(|s| s.to_string()).collect(),
        });
    }

    issues
}

/// Calculate crowd score (0-10, higher = more problematic)
fn calculate_crowd_score(members: &[CrowdMember], issues: &[CrowdIssue]) -> f32 {
    let mut score = 0.0;

    // More members = higher score
    score += (members.len() as f32 - 1.0).min(3.0);

    // More issues = higher score
    score += issues.len() as f32 * 1.5;

    // Usage asymmetry adds to score
    for issue in issues {
        if let CrowdIssue::UsageAsymmetry { underused, .. } = issue {
            score += underused.len() as f32 * 0.5;
        }
    }

    score.min(10.0)
}

/// Infer the UI/architectural context type for a crowd based on pattern and file paths
fn infer_context_type(pattern: &str, members: &[CrowdMember]) -> Option<ContextType> {
    let pattern_lower = pattern.to_lowercase();

    // Collect all paths for additional signals
    let paths: Vec<&str> = members.iter().map(|m| m.file.as_str()).collect();
    let paths_lower: String = paths.join(" ").to_lowercase();

    // Rail/Navigation patterns
    if pattern_lower.contains("nav")
        || pattern_lower.contains("sidebar")
        || pattern_lower.contains("drawer")
        || pattern_lower.contains("rail")
        || pattern_lower.contains("menu")
        || pattern_lower.contains("toolbar")
        || paths_lower.contains("/nav/")
        || paths_lower.contains("/navigation/")
        || paths_lower.contains("/sidebar/")
    {
        return Some(ContextType::Rail);
    }

    // Modal/Dialog patterns
    if pattern_lower.contains("modal")
        || pattern_lower.contains("dialog")
        || pattern_lower.contains("popup")
        || pattern_lower.contains("overlay")
        || pattern_lower.contains("toast")
        || pattern_lower.contains("alert")
        || pattern_lower.contains("confirm")
        || paths_lower.contains("/modal/")
        || paths_lower.contains("/dialog/")
    {
        return Some(ContextType::Modal);
    }

    // Form patterns
    if pattern_lower.contains("form")
        || pattern_lower.contains("input")
        || pattern_lower.contains("field")
        || pattern_lower.contains("select")
        || pattern_lower.contains("picker")
        || pattern_lower.contains("checkbox")
        || pattern_lower.contains("radio")
        || pattern_lower.contains("textarea")
        || paths_lower.contains("/form/")
        || paths_lower.contains("/forms/")
        || paths_lower.contains("/inputs/")
    {
        return Some(ContextType::Form);
    }

    // List/Table patterns
    if pattern_lower.contains("list")
        || pattern_lower.contains("table")
        || pattern_lower.contains("grid")
        || pattern_lower.contains("row")
        || pattern_lower.contains("column")
        || pattern_lower.contains("data")
        || paths_lower.contains("/list/")
        || paths_lower.contains("/table/")
    {
        return Some(ContextType::List);
    }

    // Panel/Content patterns
    if pattern_lower.contains("panel")
        || pattern_lower.contains("card")
        || pattern_lower.contains("section")
        || pattern_lower.contains("content")
        || pattern_lower.contains("view")
        || pattern_lower.contains("page")
        || paths_lower.contains("/panel/")
        || paths_lower.contains("/cards/")
    {
        return Some(ContextType::Panel);
    }

    // State management patterns
    if pattern_lower.contains("hook")
        || pattern_lower.contains("store")
        || pattern_lower.contains("context")
        || pattern_lower.contains("state")
        || pattern_lower.contains("provider")
        || pattern_lower.contains("reducer")
        || pattern_lower.contains("atom")
        || paths_lower.contains("/hooks/")
        || paths_lower.contains("/store/")
        || paths_lower.contains("/stores/")
        || paths_lower.contains("/context/")
    {
        return Some(ContextType::State);
    }

    // API/Service patterns
    if pattern_lower.contains("api")
        || pattern_lower.contains("service")
        || pattern_lower.contains("client")
        || pattern_lower.contains("fetch")
        || pattern_lower.contains("request")
        || pattern_lower.contains("endpoint")
        || paths_lower.contains("/api/")
        || paths_lower.contains("/services/")
        || paths_lower.contains("/clients/")
    {
        return Some(ContextType::Api);
    }

    // Utility patterns
    if pattern_lower.contains("util")
        || pattern_lower.contains("helper")
        || pattern_lower.contains("common")
        || pattern_lower.contains("shared")
        || pattern_lower.contains("lib")
        || paths_lower.contains("/utils/")
        || paths_lower.contains("/helpers/")
        || paths_lower.contains("/lib/")
    {
        return Some(ContextType::Util);
    }

    // If no specific pattern matched, return None (will show as "other" if needed)
    None
}

/// Auto-detect all crowds in the codebase
pub fn detect_all_crowds(files: &[FileAnalysis]) -> Vec<Crowd> {
    detect_all_crowds_internal(files, None)
}

/// Auto-detect all crowds with transitive importer counting
pub fn detect_all_crowds_with_edges(files: &[FileAnalysis], edges: &[GraphEdge]) -> Vec<Crowd> {
    detect_all_crowds_internal(files, Some(edges))
}

fn detect_all_crowds_internal(files: &[FileAnalysis], edges: Option<&[GraphEdge]>) -> Vec<Crowd> {
    let patterns = detect_name_patterns(files);

    patterns
        .into_iter()
        .take(10) // Limit to top 10 patterns
        .map(|pattern| detect_crowd_internal(files, &pattern, edges))
        .filter(|crowd| crowd.members.len() >= 2) // Only include actual crowds
        .collect()
}
