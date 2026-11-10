//! Types for crowd detection (functional associations and review candidates)
//!
//! A "crowd" is a cluster of files that serve similar purposes or handle the same domain,
//! not by itself evidence of fragmentation or duplication in the codebase.
//!
//! Example crowds:
//! - 3 different hooks all managing "message" state
//! - 2 components both rendering messages with overlapping exports
//! - Multiple utils files with similar string manipulation functions

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};

/// UI context type for a crowd - helps AI agents understand the architectural role
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextType {
    /// Navigation elements: sidebars, drawers, nav bars, rails
    Rail,
    /// Content panels: cards, sections, content areas
    Panel,
    /// Overlays: modals, dialogs, popups, toasts
    Modal,
    /// User input: forms, inputs, selects, pickers
    Form,
    /// Data display: lists, tables, grids, data views
    List,
    /// State management: hooks, stores, context providers
    State,
    /// API/data fetching: services, clients, fetchers
    Api,
    /// Utility/helper functions
    Util,
    /// Generic/unclassified
    Other,
}

impl std::fmt::Display for ContextType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContextType::Rail => write!(f, "rail"),
            ContextType::Panel => write!(f, "panel"),
            ContextType::Modal => write!(f, "modal"),
            ContextType::Form => write!(f, "form"),
            ContextType::List => write!(f, "list"),
            ContextType::State => write!(f, "state"),
            ContextType::Api => write!(f, "api"),
            ContextType::Util => write!(f, "util"),
            ContextType::Other => write!(f, "other"),
        }
    }
}

/// A detected "crowd" of files clustering around similar functionality
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Crowd {
    /// The pattern/keyword this crowd clusters around (e.g., "message", "chat")
    pub pattern: String,
    /// Files in this crowd, sorted by usage (most used first)
    pub members: Vec<CrowdMember>,
    /// Evidenced concern score (0-10, higher = more problematic); membership
    /// and import similarity alone contribute nothing.
    pub score: f32,
    /// Detected issues
    pub issues: Vec<CrowdIssue>,
    /// Inferred UI/architectural context type
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_type: Option<ContextType>,
}

/// A file that's part of a crowd
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrowdMember {
    /// File path relative to project root
    pub file: String,
    /// Why this file is in the crowd
    pub match_reason: MatchReason,
    /// Number of files that import this one, following re-export chains:
    /// direct importers plus upstream consumers of any barrel/glob that
    /// re-exports this file. For textual direct importers only, see
    /// `importer_count_direct`.
    pub importer_count: usize,
    /// Direct importers only: files with a textual import or re-export edge
    /// pointing at this file, without following re-export chains.
    #[serde(default)]
    pub importer_count_direct: usize,
    /// Similarity scores with other crowd members (file_path, similarity_score)
    pub similarity_scores: Vec<(String, f32)>,
    /// Whether this is a test file
    #[serde(default)]
    pub is_test: bool,
}

/// Why a file matched the crowd pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MatchReason {
    /// File/export name matches pattern
    NameMatch {
        /// The matched string (filename, export name, etc.)
        matched: String,
    },
    /// High import similarity with other crowd members
    ImportSimilarity {
        /// Similarity score (0.0-1.0)
        similarity: f32,
    },
    /// Exports similar types/functions
    ExportSimilarity {
        /// File this one is similar to
        similar_to: String,
    },
}

/// Evidenced review candidate, not a redundancy or deletion verdict.
#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CrowdIssue {
    /// Same exact, case-sensitive leaf stem after removing only the final extension.
    NameCollision {
        /// Full root-relative file paths with that stem (at least two).
        files: Vec<String>,
    },
    /// A usage gap between peers supported by NameCollision or ExportOverlap.
    UsageAsymmetry {
        /// The more-imported peer, not an architectural authority.
        primary: String,
        /// Lower-usage peer candidates, not evidence of redundant implementations.
        underused: Vec<String>,
    },
    /// Exact shared indexed export names, excluding default/star placeholders.
    ExportOverlap {
        /// Files with overlapping exports
        files: Vec<String>,
        /// Overlapping export names
        overlap: Vec<String>,
    },
    /// Related functionality is scattered
    Fragmentation {
        /// Categories/themes found scattered across crowd
        categories: Vec<String>,
    },
}

impl CrowdIssue {
    fn qualification(&self) -> &'static str {
        match self {
            Self::NameCollision { .. } => {
                "Review candidate: same exact case-sensitive leaf stem (final extension removed). Namespaces/platform variants may be intentional; not a duplication or deletion verdict."
            }
            Self::UsageAsymmetry { .. } => {
                "Peer comparison candidate only: see name_collision/export_overlap evidence and member importer counts. Lower usage does not establish redundancy or architectural authority."
            }
            Self::ExportOverlap { .. } => {
                "Review candidate: exact shared indexed export names, not shared imports or proof of duplicate implementation. Re-exports and namespace/platform variants may be intentional."
            }
            Self::Fragmentation { .. } => {
                "Review candidate only; related functionality need not be consolidated."
            }
        }
    }
}

// MCP's legacy string issues use Debug; keep their evidence and qualification
// aligned with the structured payload without changing the response container.
impl std::fmt::Debug for CrowdIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = match self {
            Self::NameCollision { files } => {
                let mut debug = f.debug_struct("NameCollision");
                debug.field("files", files);
                debug
            }
            Self::UsageAsymmetry { primary, underused } => {
                let mut debug = f.debug_struct("UsageAsymmetry");
                debug
                    .field("primary", primary)
                    .field("underused", underused);
                debug
            }
            Self::ExportOverlap { files, overlap } => {
                let mut debug = f.debug_struct("ExportOverlap");
                debug.field("files", files).field("overlap", overlap);
                debug
            }
            Self::Fragmentation { categories } => {
                let mut debug = f.debug_struct("Fragmentation");
                debug.field("categories", categories);
                debug
            }
        };
        debug.field("qualification", &self.qualification()).finish()
    }
}

// Add qualification on the wire without changing Rust variants or their fields.
// Older payloads still deserialize; clients may ignore the additive field.
impl Serialize for CrowdIssue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        match self {
            Self::NameCollision { files } => {
                map.serialize_entry("type", "name_collision")?;
                map.serialize_entry("files", files)?;
            }
            Self::UsageAsymmetry { primary, underused } => {
                map.serialize_entry("type", "usage_asymmetry")?;
                map.serialize_entry("primary", primary)?;
                map.serialize_entry("underused", underused)?;
            }
            Self::ExportOverlap { files, overlap } => {
                map.serialize_entry("type", "export_overlap")?;
                map.serialize_entry("files", files)?;
                map.serialize_entry("overlap", overlap)?;
            }
            Self::Fragmentation { categories } => {
                map.serialize_entry("type", "fragmentation")?;
                map.serialize_entry("categories", categories)?;
            }
        }
        map.serialize_entry("qualification", self.qualification())?;
        map.end()
    }
}
