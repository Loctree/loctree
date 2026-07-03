//! Helper functions for command parsing.
//!
//! This module contains utility functions used across multiple command parsers:
//! - Color mode parsing
//! - JQ filter detection
//! - Command suggestion via Levenshtein distance

use strsim::levenshtein;

use crate::types::ColorMode;

/// Known subcommand names for the new CLI interface.
pub(crate) const SUBCOMMANDS: &[&str] = &[
    "auto",
    "agent",
    "scan",
    "watch",
    "tree",
    "slice",
    "s", // alias for slice
    "context",
    "repo-view",
    "find",
    "f", // alias for find
    "occurrences",
    "findings",
    "dead",
    "d", // alias for dead
    "unused",
    "cycles",
    "c", // alias for cycles
    "trace",
    "commands",
    "events",
    "pipelines",
    "insights",
    "manifests",
    "info",
    "lint",
    "report",
    "prism",
    "help",
    "query",
    "q", // alias for query
    "body",
    "diff",
    "crowd",
    "tagmap",
    "twins",
    "t", // alias for twins
    "suppress",
    "suppressions",
    "routes",
    "dist",
    "coverage",
    "impact",
    "i", // alias for impact
    "focus",
    "hotspots",
    "follow",
    "layoutmap",
    "health",
    "h", // alias for health
    "audit",
    "doctor",
    "env-truth",
    "envtruth",
    "plan",
    "p", // alias for plan
    "cache",
    "prune-old-artifacts",
];

/// Check if an argument looks like a new-style subcommand.
pub fn is_subcommand(arg: &str) -> bool {
    SUBCOMMANDS.contains(&arg)
}

/// Suggest a similar command using Levenshtein distance.
/// Returns Some(suggestion) if a close match is found (distance <= 2).
pub(super) fn suggest_similar_command(input: &str) -> Option<&'static str> {
    let input_lower = input.to_lowercase();
    let mut best_match: Option<(&str, usize)> = None;

    for &cmd in SUBCOMMANDS {
        let distance = levenshtein(&input_lower, cmd);
        // Only suggest if distance is small (max 2 for reasonable similarity)
        if distance <= 2 {
            if let Some((_, best_dist)) = best_match {
                if distance < best_dist {
                    best_match = Some((cmd, distance));
                }
            } else {
                best_match = Some((cmd, distance));
            }
        }
    }

    best_match.map(|(cmd, _)| cmd)
}

/// Check if argument looks like a jq filter expression
pub(super) fn is_jq_filter(arg: &str) -> bool {
    let trimmed = arg.trim();
    if trimmed.is_empty() {
        return false;
    }

    // Starts with . [ or { = jq filter
    if trimmed.starts_with('.') || trimmed.starts_with('[') || trimmed.starts_with('{') {
        // But not path-like ./foo or .\foo
        if trimmed.starts_with("./") || trimmed.starts_with(".\\") {
            return false;
        }
        // If it's a dotfile that exists on disk, treat as path
        if trimmed.starts_with('.')
            && !trimmed.contains('[')
            && !trimmed.contains('|')
            && std::path::Path::new(trimmed).exists()
        {
            return false;
        }
        return true;
    }
    false
}

/// Parse color mode from string value.
pub(super) fn parse_color_mode(value: &str) -> Result<ColorMode, String> {
    match value.to_lowercase().as_str() {
        "auto" => Ok(ColorMode::Auto),
        "always" | "yes" | "true" => Ok(ColorMode::Always),
        "never" | "no" | "false" => Ok(ColorMode::Never),
        _ => Err(format!(
            "Invalid color mode '{}'. Use: auto, always, or never.",
            value
        )),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_subcommand() {
        assert!(is_subcommand("auto"));
        assert!(is_subcommand("scan"));
        assert!(is_subcommand("tree"));
        assert!(is_subcommand("slice"));
        assert!(is_subcommand("prism"));
        assert!(is_subcommand("dead"));
        assert!(is_subcommand("findings"));
        assert!(is_subcommand("trace"));
        assert!(!is_subcommand("--tree"));
        assert!(!is_subcommand("-A"));
        assert!(!is_subcommand("unknown"));
    }

    #[test]
    fn test_is_jq_filter() {
        // Valid jq filters
        assert!(is_jq_filter(".metadata"));
        assert!(is_jq_filter(".files[]"));
        assert!(is_jq_filter(".files[0]"));
        assert!(is_jq_filter("[.files]"));
        assert!(is_jq_filter("{foo: .bar}"));
        assert!(is_jq_filter(".foo | .bar"));

        // Not jq filters
        assert!(!is_jq_filter("./foo"));
        assert!(!is_jq_filter(".\\foo"));
        assert!(!is_jq_filter("scan"));
        assert!(!is_jq_filter("--help"));
        assert!(!is_jq_filter(""));
    }

    #[test]
    fn test_parse_color_mode() {
        assert!(matches!(parse_color_mode("auto"), Ok(ColorMode::Auto)));
        assert!(matches!(parse_color_mode("always"), Ok(ColorMode::Always)));
        assert!(matches!(parse_color_mode("yes"), Ok(ColorMode::Always)));
        assert!(matches!(parse_color_mode("never"), Ok(ColorMode::Never)));
        assert!(matches!(parse_color_mode("no"), Ok(ColorMode::Never)));
        assert!(parse_color_mode("invalid").is_err());
    }
}
