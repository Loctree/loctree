//! Vector-based health score calculation for loctree.
//!
//! Three severity dimensions with logarithmic normalization:
//! - **CERTAIN**: Definitely broken (missing handlers, breaking cycles)
//! - **HIGH**: Very likely issues (dead exports, unused handlers)
//! - **SMELL**: Worth checking (twins, barrel chaos, structural cycles)
//!
//! The log-normalization ensures fair comparison across project sizes:
//! - A 100-file project with 10 twins is penalized more than
//! - A 10,000-file project with 10 twins
//!
//! # Example
//!
//! ```rust
//! use loctree::analyzer::health_score::{HealthMetrics, calculate_health_score};
//!
//! let metrics = HealthMetrics {
//!     missing_handlers: 2,
//!     twins_same_language: 10,
//!     loc: 50000,
//!     files: 200,
//!     ..Default::default()
//! };
//!
//! let score = calculate_health_score(&metrics);
//! assert!(score.health > 0);
//! assert!(score.details.certain.penalty > 0.0);
//! ```

use serde::{Deserialize, Serialize};

/// Weights for each severity level (percentage of max 100)
pub const CERTAIN_WEIGHT: f64 = 50.0; // max 50% penalty
pub const HIGH_WEIGHT: f64 = 30.0; // max 30% penalty
pub const SMELL_WEIGHT: f64 = 20.0; // max 20% penalty

/// Logarithmic normalization to prevent explosion on large projects.
///
/// Returns 0.0 - 1.0 range where:
/// - 0.0 = no issues
/// - 1.0 = extremely high issue density
///
/// Formula: ln(1 + count) / ln(1 + LOC)
///
/// This ensures that the same absolute number of issues has less impact
/// on larger projects, which is intuitive and fair.
#[inline]
pub fn log_normalize(count: usize, loc: usize) -> f64 {
    if loc == 0 || count == 0 {
        return 0.0;
    }
    let issue_log = (1.0 + count as f64).ln();
    let loc_log = (1.0 + loc as f64).ln();
    (issue_log / loc_log).min(1.0)
}

/// Individual issue identifier for drill-down
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthIssue {
    /// Type of issue: "missing_handler", "dead_export", "twin", etc.
    pub kind: String,
    /// Target identifier (handler name, symbol, file path)
    pub target: String,
    /// Optional location (file:line)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

/// A single severity dimension with normalized density
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeverityDimension {
    /// Raw count of issues at this severity level
    pub count: usize,
    /// Penalty contribution to final score (0-50/30/20 based on weight)
    pub penalty: f64,
    /// Sample of issue identifiers for drill-down (up to 10)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<HealthIssue>,
}

/// Breakdown by severity dimensions
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HealthDetails {
    /// CERTAIN: Definitely broken (runtime crash, won't work)
    pub certain: SeverityDimension,
    /// HIGH: Very likely issues (dead code, unused exports)
    pub high: SeverityDimension,
    /// SMELL: Worth checking (duplicates, architectural issues)
    pub smell: SeverityDimension,
}

/// Project size context
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectSize {
    pub files: usize,
    pub loc: usize,
}

/// Complete health score with breakdown
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthScore {
    /// Overall health score 0-100 (higher is better)
    pub health: u8,
    /// Breakdown by severity level
    pub details: HealthDetails,
    /// Normalized density metric (total issues/LOC adjusted)
    pub normalized_density: f64,
    /// Project size context
    pub project_size: ProjectSize,
}

impl Default for HealthScore {
    fn default() -> Self {
        Self {
            health: 100,
            details: HealthDetails::default(),
            normalized_density: 0.0,
            project_size: ProjectSize::default(),
        }
    }
}

/// Collected metrics for health score calculation.
///
/// This is the input structure - collect metrics from various sources
/// and pass to `calculate_health_score()`.
#[derive(Debug, Clone, Default)]
pub struct HealthMetrics {
    // === CERTAIN severity (runtime crash / won't work) ===
    /// Handlers called from frontend but missing in backend
    pub missing_handlers: usize,
    /// Handlers with #[tauri::command] but not in generate_handler![]
    pub unregistered_handlers: usize,
    /// Hard bidirectional cycles that break compilation/runtime
    pub breaking_cycles: usize,

    // === HIGH severity (very likely dead code) ===
    /// Handlers with HIGH confidence of being unused (0 calls, no string matches)
    pub unused_high_confidence: usize,
    /// Exports with 0 imports across all scanned files
    pub dead_exports: usize,
    /// Twins with 0 imports (exported but never used)
    pub twins_dead_parrots: usize,

    // === SMELL severity (worth checking) ===
    /// Same symbol exported from multiple files in same language
    pub twins_same_language: usize,
    /// Barrel chaos issues (missing barrels, deep chains, inconsistent paths)
    pub barrel_chaos_count: usize,
    /// Compilable but smelly circular dependencies
    pub structural_cycles: usize,
    /// Opaque passthrough imports (cascades)
    pub cascade_imports: usize,
    /// Duplicate export names (divided by 5 in calculation)
    pub duplicate_exports: usize,

    // === Project context ===
    pub files: usize,
    pub loc: usize,

    // === Optional: issue details for drill-down ===
    /// Details for CERTAIN issues (up to 10)
    pub certain_items: Vec<HealthIssue>,
    /// Details for HIGH issues (up to 10)
    pub high_items: Vec<HealthIssue>,
    /// Details for SMELL issues (up to 10)
    pub smell_items: Vec<HealthIssue>,
}

/// Calculate health score from collected metrics.
///
/// Uses 3-dimensional severity vector with log-normalization:
/// - CERTAIN (50% weight): missing_handlers, unregistered_handlers, breaking_cycles
/// - HIGH (30% weight): unused_high_confidence, dead_exports, twins_dead_parrots
/// - SMELL (20% weight): twins_same_language, barrel_chaos, structural_cycles, etc.
///
/// Log-normalization ensures fair comparison across project sizes.
pub fn calculate_health_score(metrics: &HealthMetrics) -> HealthScore {
    // === Aggregate counts per severity ===
    let certain_count =
        metrics.missing_handlers + metrics.unregistered_handlers + metrics.breaking_cycles;

    let high_count =
        metrics.unused_high_confidence + metrics.dead_exports + metrics.twins_dead_parrots;

    let smell_count = metrics.twins_same_language
        + metrics.barrel_chaos_count
        + metrics.structural_cycles
        + metrics.cascade_imports
        + (metrics.duplicate_exports / 5); // 5 duplicates = 1 smell issue

    // === Log-normalize each dimension ===
    let certain_norm = log_normalize(certain_count, metrics.loc);
    let high_norm = log_normalize(high_count, metrics.loc);
    let smell_norm = log_normalize(smell_count, metrics.loc);

    // === Apply weights ===
    let certain_penalty = certain_norm * CERTAIN_WEIGHT;
    let high_penalty = high_norm * HIGH_WEIGHT;
    let smell_penalty = smell_norm * SMELL_WEIGHT;

    let total_penalty = certain_penalty + high_penalty + smell_penalty;
    let health = (100.0 - total_penalty).max(0.0).round() as u8;

    // === Overall normalized density ===
    let total_issues = certain_count + high_count + smell_count;
    let normalized_density = log_normalize(total_issues, metrics.loc);

    // === Build result ===
    HealthScore {
        health,
        details: HealthDetails {
            certain: SeverityDimension {
                count: certain_count,
                penalty: certain_penalty,
                items: metrics.certain_items.iter().take(10).cloned().collect(),
            },
            high: SeverityDimension {
                count: high_count,
                penalty: high_penalty,
                items: metrics.high_items.iter().take(10).cloned().collect(),
            },
            smell: SeverityDimension {
                count: smell_count,
                penalty: smell_penalty,
                items: metrics.smell_items.iter().take(10).cloned().collect(),
            },
        },
        normalized_density,
        project_size: ProjectSize {
            files: metrics.files,
            loc: metrics.loc,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_normalize_zero_loc() {
        assert_eq!(log_normalize(10, 0), 0.0);
    }

    #[test]
    fn test_log_normalize_zero_issues() {
        assert_eq!(log_normalize(0, 10000), 0.0);
    }

    #[test]
    fn test_log_normalize_scales_with_loc() {
        // More LOC = lower normalized value for same issue count
        let small_project = log_normalize(10, 1000);
        let large_project = log_normalize(10, 100000);
        assert!(
            small_project > large_project,
            "small={} should be > large={}",
            small_project,
            large_project
        );
    }

    #[test]
    fn test_log_normalize_max_one() {
        // Even extreme values stay <= 1.0
        let extreme = log_normalize(1_000_000, 100);
        assert!(extreme <= 1.0, "extreme={} should be <= 1.0", extreme);
    }

    #[test]
    fn test_log_normalize_reasonable_values() {
        // 10 issues in 10k LOC should be low (~0.2)
        let normal = log_normalize(10, 10_000);
        assert!(
            normal > 0.0 && normal < 0.5,
            "normal={} should be between 0.0 and 0.5",
            normal
        );

        // 100 issues in 10k LOC should be moderate (~0.5)
        let moderate = log_normalize(100, 10_000);
        assert!(
            moderate > normal,
            "moderate={} should be > normal={}",
            moderate,
            normal
        );
    }

    #[test]
    fn test_health_score_perfect() {
        let metrics = HealthMetrics {
            loc: 10000,
            files: 100,
            ..Default::default()
        };
        let score = calculate_health_score(&metrics);
        assert_eq!(score.health, 100);
        assert_eq!(score.details.certain.count, 0);
        assert_eq!(score.details.high.count, 0);
        assert_eq!(score.details.smell.count, 0);
    }

    #[test]
    fn test_health_score_certain_dominates() {
        let mut metrics = HealthMetrics {
            loc: 10000,
            files: 100,
            ..Default::default()
        };
        metrics.missing_handlers = 5;

        let score = calculate_health_score(&metrics);
        assert!(
            score.health < 100,
            "health={} should be < 100",
            score.health
        );
        assert!(
            score.details.certain.penalty > 0.0,
            "certain.penalty should be > 0"
        );
        assert_eq!(score.details.high.penalty, 0.0);
        assert_eq!(score.details.smell.penalty, 0.0);
    }

    #[test]
    fn test_health_score_smell_minor_impact() {
        let mut metrics = HealthMetrics {
            loc: 10000,
            files: 100,
            ..Default::default()
        };
        metrics.twins_same_language = 10;

        let score = calculate_health_score(&metrics);
        // SMELL has max 20% weight, so impact is limited
        assert!(
            score.health >= 80,
            "health={} should be >= 80 for smell-only issues",
            score.health
        );
    }

    #[test]
    fn test_health_score_never_negative() {
        let mut metrics = HealthMetrics {
            loc: 100, // Small project
            files: 5,
            ..Default::default()
        };
        metrics.missing_handlers = 100;
        metrics.dead_exports = 100;
        metrics.barrel_chaos_count = 100;

        let score = calculate_health_score(&metrics);
        assert!(score.health <= 100, "health should never exceed 100");
    }

    #[test]
    fn test_weights_sum_to_100() {
        assert_eq!(
            CERTAIN_WEIGHT + HIGH_WEIGHT + SMELL_WEIGHT,
            100.0,
            "weights should sum to 100"
        );
    }

    #[test]
    fn test_health_score_project_size_matters() {
        // Same issues, different project sizes
        let small_metrics = HealthMetrics {
            missing_handlers: 5,
            twins_same_language: 20,
            loc: 1000,
            files: 10,
            ..Default::default()
        };

        let large_metrics = HealthMetrics {
            missing_handlers: 5,
            twins_same_language: 20,
            loc: 100000,
            files: 500,
            ..Default::default()
        };

        let small_score = calculate_health_score(&small_metrics);
        let large_score = calculate_health_score(&large_metrics);

        // Large project should have higher health (less penalty per issue)
        assert!(
            large_score.health > small_score.health,
            "large={} should be > small={}",
            large_score.health,
            small_score.health
        );
    }

    #[test]
    fn test_duplicate_exports_divided_by_5() {
        let mut metrics = HealthMetrics {
            loc: 10000,
            files: 100,
            ..Default::default()
        };

        // 4 duplicates = 0 smell issues (4/5 = 0)
        metrics.duplicate_exports = 4;
        let score1 = calculate_health_score(&metrics);

        // 5 duplicates = 1 smell issue (5/5 = 1)
        metrics.duplicate_exports = 5;
        let score2 = calculate_health_score(&metrics);

        assert!(
            score1.details.smell.count < score2.details.smell.count,
            "5 duplicates should add to smell count"
        );
    }

    #[test]
    fn test_health_score_serialization() {
        let metrics = HealthMetrics {
            missing_handlers: 2,
            twins_same_language: 10,
            loc: 50000,
            files: 200,
            ..Default::default()
        };

        let score = calculate_health_score(&metrics);
        let json = serde_json::to_string_pretty(&score).unwrap();

        assert!(json.contains("\"health\""));
        assert!(json.contains("\"details\""));
        assert!(json.contains("\"certain\""));
        assert!(json.contains("\"normalized_density\""));
    }
}
