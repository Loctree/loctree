//! Custom LSP request: `loctree/impact`
//!
//! Returns blast-radius analysis (direct + optional transitive consumers)
//! for a target file using `loctree::impact::analyze_impact`. Plan 06 of
//! the LSP roadmap. Pre-refactor / pre-delete safety check for daemon-mode
//! agents.
//!
//! Severity heuristic (per plan):
//!   - `low`    → fewer than 5 total consumers
//!   - `medium` → 5..=20 consumers AND max_depth <= 3
//!   - `high`   → more than 20 consumers OR max_depth > 3
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::path::PathBuf;

use loctree::impact::{ImpactEntry, ImpactOptions, ImpactResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters for `loctree/impact`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImpactParams {
    /// Target file. Repo-relative or absolute — normalized by the analyzer.
    pub target: PathBuf,
    /// When true, include transitive consumers (consumers-of-consumers).
    /// When false, the request short-circuits BFS at depth 1.
    #[serde(default)]
    pub transitive: bool,
    /// Workspace project root override. Reserved for Plan 13
    /// (multi-workspace context); ignored in single-workspace mode.
    #[serde(default)]
    pub project: Option<PathBuf>,
}

/// One importer entry — paths-only by contract.
#[derive(Debug, Clone, Serialize)]
pub struct ImporterEntry {
    /// Repo-relative path of the importing file.
    pub path: String,
    /// Distance from target (1 = direct, 2+ = transitive).
    pub depth: usize,
}

/// `loctree/impact` response payload.
#[derive(Debug, Clone, Serialize)]
pub struct ImpactResponse {
    /// Files that directly import the target (depth = 1).
    pub direct: Vec<ImporterEntry>,
    /// Transitive consumers (depth ≥ 2). Empty when `transitive: false`.
    pub transitive: Vec<ImporterEntry>,
    /// `direct.len() + transitive.len()`.
    pub total: usize,
    /// Heuristic severity label: `"low" | "medium" | "high"`.
    pub blast_severity: String,
    /// Human-readable warnings (e.g. dynamic-import flags).
    pub warnings: Vec<String>,
}

impl ImpactResponse {
    /// Map a `loctree::impact::ImpactResult` into the LSP response shape.
    ///
    /// `transitive_requested` determines whether `result.transitive_consumers`
    /// is surfaced. Even when `false`, the underlying analyzer may have been
    /// run with `max_depth = Some(1)`, in which case `transitive_consumers`
    /// is already empty.
    pub fn from_impact(result: &ImpactResult, transitive_requested: bool) -> Self {
        let direct: Vec<ImporterEntry> = result.direct_consumers.iter().map(map_entry).collect();
        let transitive: Vec<ImporterEntry> = if transitive_requested {
            result.transitive_consumers.iter().map(map_entry).collect()
        } else {
            Vec::new()
        };

        let total = direct.len() + transitive.len();
        let blast_severity = severity_label(total, result.max_depth).to_string();
        let warnings = collect_warnings(result.direct_consumers.iter().chain(
            if transitive_requested {
                result.transitive_consumers.iter()
            } else {
                [].iter()
            },
        ));

        ImpactResponse {
            direct,
            transitive,
            total,
            blast_severity,
            warnings,
        }
    }
}

fn map_entry(entry: &ImpactEntry) -> ImporterEntry {
    ImporterEntry {
        path: entry.file.clone(),
        depth: entry.depth,
    }
}

/// Pure severity classifier (per plan).
///
/// - `high`   when `total > 20` OR `max_depth > 3`
/// - `low`    when `total < 5` (and depth not forcing high)
/// - `medium` otherwise (5..=20 with depth ≤ 3)
///
/// Depth dominates: a 2-importer chain that reaches depth 4 is still `high`,
/// because deep dependency chains amplify any change's surprise factor.
pub fn severity_label(total: usize, max_depth: usize) -> &'static str {
    if max_depth > 3 || total > 20 {
        "high"
    } else if total < 5 {
        "low"
    } else {
        "medium"
    }
}

fn collect_warnings<'a, I>(entries: I) -> Vec<String>
where
    I: Iterator<Item = &'a ImpactEntry>,
{
    let mut dynamic_files: Vec<&str> = entries
        .filter(|e| is_dynamic_import(&e.import_type))
        .map(|e| e.file.as_str())
        .collect();
    dynamic_files.sort();
    dynamic_files.dedup();

    let mut warnings = Vec::new();
    if !dynamic_files.is_empty() {
        warnings.push(format!(
            "{} importer(s) use dynamic imports — runtime impact may differ from static analysis",
            dynamic_files.len()
        ));
    }
    warnings
}

fn is_dynamic_import(import_type: &str) -> bool {
    matches!(
        import_type,
        "dynamic" | "dynamic-import" | "import()" | "require()"
    )
}

/// Resolve a target into a string the analyzer can consume.
pub fn target_string(params: &ImpactParams) -> String {
    params.target.to_string_lossy().into_owned()
}

/// Build `ImpactOptions` honoring `transitive` (cap depth when not requested).
pub fn options_from_params(params: &ImpactParams) -> ImpactOptions {
    let defaults = ImpactOptions::default();
    ImpactOptions {
        max_depth: if params.transitive {
            defaults.max_depth
        } else {
            Some(1)
        },
        include_reexports: defaults.include_reexports,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_low_under_five_consumers() {
        // Low only when total < 5 AND depth ≤ 3 (depth dominates above that).
        assert_eq!(severity_label(0, 0), "low");
        assert_eq!(severity_label(4, 0), "low");
        assert_eq!(severity_label(4, 3), "low");
    }

    #[test]
    fn severity_medium_within_band() {
        assert_eq!(severity_label(5, 1), "medium");
        assert_eq!(severity_label(20, 3), "medium");
    }

    #[test]
    fn severity_high_when_too_many_consumers() {
        assert_eq!(severity_label(21, 1), "high");
        assert_eq!(severity_label(100, 0), "high");
    }

    #[test]
    fn severity_high_when_depth_exceeds_three() {
        // Depth dominates over total — even tiny consumer counts go to "high"
        // when the chain reaches depth 4.
        assert_eq!(severity_label(2, 4), "high");
        assert_eq!(severity_label(5, 4), "high");
        assert_eq!(severity_label(15, 5), "high");
    }

    #[test]
    fn options_short_circuits_when_transitive_off() {
        let params = ImpactParams {
            target: PathBuf::from("x"),
            transitive: false,
            project: None,
        };
        let opts = options_from_params(&params);
        assert_eq!(opts.max_depth, Some(1));
    }

    #[test]
    fn options_uses_default_depth_when_transitive_on() {
        let params = ImpactParams {
            target: PathBuf::from("x"),
            transitive: true,
            project: None,
        };
        let opts = options_from_params(&params);
        assert_eq!(opts.max_depth, ImpactOptions::default().max_depth);
    }

    #[test]
    fn dynamic_import_detection() {
        assert!(is_dynamic_import("dynamic"));
        assert!(is_dynamic_import("dynamic-import"));
        assert!(is_dynamic_import("import()"));
        assert!(is_dynamic_import("require()"));
        assert!(!is_dynamic_import("import"));
        assert!(!is_dynamic_import("reexport"));
    }
}
