//! Shared diagnostic-code registry for LSP producers and consumers.
//!
//! Diagnostic emitters use the canonical codes here. Code-action
//! consumers can still accept old aliases, but every production code in
//! [`ALL_EMITTED_CODES`] must map to an atlas card.

/// Diagnostic codes produced by loctree LSP diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticCode {
    DeadExport,
    CircularImport,
    TwinExport,
}

impl DiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeadExport => "dead-export",
            Self::CircularImport => "circular-import",
            Self::TwinExport => "twin-export",
        }
    }
}

/// Canonical codes emitted by `diagnostics/*.rs`.
pub const ALL_EMITTED_CODES: &[&str] = &[
    DiagnosticCode::DeadExport.as_str(),
    DiagnosticCode::CircularImport.as_str(),
    DiagnosticCode::TwinExport.as_str(),
];

/// Codes from the production registry that the atlas-card consumer accepts.
///
/// This is intentionally derived from the consumer mapping, so the
/// registry test fails if a future diagnostic code is emitted without an
/// atlas-card route.
pub fn all_consumed_codes() -> Vec<&'static str> {
    ALL_EMITTED_CODES
        .iter()
        .copied()
        .filter(|code| atlas_card_for_diagnostic_code(code).is_some())
        .collect()
}

/// Map a diagnostic `code` field to the best Context Atlas card.
///
/// The canonical production codes are the enum values above. Historical
/// aliases remain accepted so older diagnostics and tests do not lose
/// the quickfix affordance.
pub fn atlas_card_for_diagnostic_code(code: &str) -> Option<&'static str> {
    match code {
        // Dead-code family: runtime-map explains exports and reachability.
        "dead-export" | "dead_export" | "dead-parrot" | "dead_parrot" => Some("02-runtime-map.md"),
        // Cycle family: structural-map shows imports and consumers.
        "cycle"
        | "circular-import"
        | "circular_import"
        | "lazy-circular-import"
        | "lazy_circular_import" => Some("01-structural-map.md"),
        // Twin family: structural-map shows duplicate exports.
        "twin" | "twin-export" | "exact-twin" | "exact_twin" | "same-language-twin" => {
            Some("01-structural-map.md")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_emitted_code_is_consumed_by_atlas_mapping() {
        assert_eq!(ALL_EMITTED_CODES, all_consumed_codes());
    }

    #[test]
    fn aliases_still_route_to_existing_atlas_cards() {
        assert_eq!(
            atlas_card_for_diagnostic_code("dead_parrot"),
            Some("02-runtime-map.md")
        );
        assert_eq!(
            atlas_card_for_diagnostic_code("lazy_circular_import"),
            Some("01-structural-map.md")
        );
        assert_eq!(
            atlas_card_for_diagnostic_code("exact-twin"),
            Some("01-structural-map.md")
        );
    }
}
