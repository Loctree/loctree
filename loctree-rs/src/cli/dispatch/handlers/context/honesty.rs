//! Cut 11 — measurement-honesty enums for the pill renderer.
//!
//! The pill is the brand-defining product surface. Every numeric metric in
//! it carries provenance: either we measured it (`Measured(n)`), or we
//! explicitly admit we didn't (`NotMeasured` / `NotApplicable`). This kills
//! misleading-green metrics like `dead_exports: 0` that previously read as
//! "we measured zero" when in reality the use-graph counts `pub use`
//! re-exports as live and never measured anything.
//!
//! The pill renderer treats these enums as the authoritative shape — the
//! renderer never produces a bare `0` for a metric that has known
//! measurement holes.

use serde::{Deserialize, Serialize};

/// Measurement status for the dead-exports metric.
///
/// Measurement contract has known holes (notably: Rust `pub use` re-exports
/// are counted as live by the use-graph). When that hole applies to the
/// current snapshot, surface `NotMeasured` with the reason instead of a
/// silent zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "value")]
pub enum DeadExportsStatus {
    /// Honest count — every export passed the use-graph check.
    Measured(u64),
    /// Skipped because the use-graph counts `pub use` re-exports as live;
    /// running `loct dead` directly on the relevant scope is the correct
    /// follow-up.
    SkippedDueToReExports,
    /// Language set has no dead-export semantics (e.g. CSS, shell).
    NotApplicable,
}

impl DeadExportsStatus {
    /// Render as a single label for use in the pill markdown.
    pub fn label(&self) -> String {
        match self {
            Self::Measured(n) => format!("{n} (RepoVerified)"),
            Self::SkippedDueToReExports => "not_measured (use-graph counts `pub use` \
                re-exports as live — known limitation; run `loct dead` for current count)"
                .to_string(),
            Self::NotApplicable => "not_applicable for this language set".to_string(),
        }
    }

    /// `true` when the metric was actually measured. Test-only today; the
    /// pill renderer matches on `Self::Measured(_)` inline rather than calling
    /// through this predicate. Keep gated until a live caller adopts it.
    #[cfg(test)]
    pub fn is_measured(&self) -> bool {
        matches!(self, Self::Measured(_))
    }
}

/// Measurement status for cycles / twins / hub counts. Same honesty contract
/// as `DeadExportsStatus`: measured numbers carry the count, everything else
/// surfaces an explicit reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "value")]
pub enum MeasurementStatus {
    Measured(u64),
    NotMeasured(String),
    NotApplicable,
}

impl MeasurementStatus {
    pub fn label(&self) -> String {
        match self {
            Self::Measured(n) => format!("{n}"),
            Self::NotMeasured(reason) => format!("not_measured ({reason})"),
            Self::NotApplicable => "not_applicable".to_string(),
        }
    }

    pub fn is_measured(&self) -> bool {
        matches!(self, Self::Measured(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_exports_measured_label_carries_authority() {
        let s = DeadExportsStatus::Measured(7);
        assert!(s.is_measured());
        let label = s.label();
        assert!(label.contains('7'));
        assert!(label.contains("RepoVerified"));
    }

    #[test]
    fn dead_exports_skipped_label_explains_limitation() {
        let s = DeadExportsStatus::SkippedDueToReExports;
        assert!(!s.is_measured());
        let label = s.label();
        assert!(label.contains("not_measured"));
        assert!(label.contains("re-exports"));
        assert!(label.contains("loct dead"));
    }

    #[test]
    fn dead_exports_not_applicable_label_states_reason() {
        let s = DeadExportsStatus::NotApplicable;
        assert!(!s.is_measured());
        let label = s.label();
        assert!(label.contains("not_applicable"));
    }

    #[test]
    fn measurement_status_round_trip() {
        let original = MeasurementStatus::NotMeasured("snapshot stale".to_string());
        let json = serde_json::to_string(&original).expect("serialize");
        let back: MeasurementStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, back);
    }

    #[test]
    fn measurement_status_label_renders_each_variant() {
        assert_eq!(MeasurementStatus::Measured(3).label(), "3");
        assert!(
            MeasurementStatus::NotMeasured("reason".to_string())
                .label()
                .starts_with("not_measured")
        );
        assert_eq!(MeasurementStatus::NotApplicable.label(), "not_applicable");
    }

    #[test]
    fn dead_exports_status_serde_round_trip_for_all_variants() {
        for sample in [
            DeadExportsStatus::Measured(0),
            DeadExportsStatus::Measured(42),
            DeadExportsStatus::SkippedDueToReExports,
            DeadExportsStatus::NotApplicable,
        ] {
            let json = serde_json::to_string(&sample).expect("serialize");
            let back: DeadExportsStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(sample, back);
        }
    }
}
