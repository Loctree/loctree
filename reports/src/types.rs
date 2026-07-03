//! Report data types for structuring analysis results.
//!
//! These types define the data model for reports. They're designed to be:
//!
//! - **Serializable** - Easy JSON import/export via serde
//! - **Clone-friendly** - Components can share data without borrowing issues
//! - **Default-able** - Create partial reports with `..Default::default()`
//!
//! # Example
//!
//! ```rust
//! use report_leptos::types::{ReportSection, AiInsight, RankedDup};
//!
//! let section = ReportSection {
//!     root: "my-project/src".into(),
//!     files_analyzed: 42,
//!     insights: vec![
//!         AiInsight {
//!             title: "Circular Import Detected".into(),
//!             severity: "high".into(),
//!             message: "Consider breaking the cycle...".into(),
//!         }
//!     ],
//!     ranked_dups: vec![
//!         RankedDup {
//!             name: "formatDate".into(),
//!             files: vec!["utils/date.ts".into(), "helpers/format.ts".into()],
//!             score: 5,
//!             ..Default::default()
//!         }
//!     ],
//!     ..Default::default()
//! };
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Confidence level for dead export and handler detection.
///
/// CERTAIN - Will definitely break/is definitely unused
///   - Unregistered handlers (has #[tauri::command] but NOT in invoke_handler![])
///   - Missing handlers (FE calls invoke() but no handler exists)
///
/// HIGH - Very likely unused, worth fixing
///   - Export with 0 imports across all scanned files
///   - Handler registered but 0 invoke() calls found
///
/// SMELL - Worth checking, might be intentional
///   - Twins (same name in multiple files)
///   - Low import count relative to codebase size
///   - String literal matches found (may be used dynamically)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Confidence {
    /// CERTAIN - Will definitely break/is definitely unused
    Certain,
    /// HIGH - Very likely unused, worth fixing
    High,
    /// SMELL - Worth checking, might be intentional
    Smell,
}

impl Confidence {
    /// Get indicator for this confidence level
    pub fn indicator(&self) -> &'static str {
        match self {
            Confidence::Certain => "[!!]",
            Confidence::High => "[!]",
            Confidence::Smell => "[?]",
        }
    }
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Confidence::Certain => write!(f, "CERTAIN"),
            Confidence::High => write!(f, "HIGH"),
            Confidence::Smell => write!(f, "SMELL"),
        }
    }
}

/// A string literal that might indicate dynamic command usage.
///
/// Used to track potential false positives when detecting unused handlers.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StringLiteralMatch {
    /// File path where the literal was found
    pub file: String,
    /// Line number (1-indexed)
    pub line: usize,
    /// Context type: "allowlist", "const", "object_key", "array_item"
    pub context: String,
}

/// Full command bridge for FE↔BE comparison table.
///
/// Represents a single Tauri command with all frontend call sites
/// and the corresponding backend handler location.
///
/// # Status Values
///
/// - `"ok"` - Command properly matched (FE calls + BE handler)
/// - `"missing_handler"` - FE calls exist but no BE handler
/// - `"unused_handler"` - BE handler exists but no FE calls
/// - `"unregistered_handler"` - BE handler exists but not in generate_handler![]
///
/// # Example
///
/// ```rust
/// use report_leptos::types::CommandBridge;
///
/// let bridge = CommandBridge {
///     name: "get_user".into(),
///     fe_locations: vec![
///         ("src/api.ts".into(), 42),
///         ("src/components/Profile.tsx".into(), 15),
///     ],
///     be_location: Some(("src-tauri/src/commands/user.rs".into(), 10, "get_user".into())),
///     status: "ok".into(),
///     language: "rs".into(),
///     comm_type: "invoke".into(),
///     emits_events: vec![],
/// };
/// ```
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CommandBridge {
    /// Command name (exposed_name from Tauri)
    pub name: String,
    /// Frontend call locations (file, line)
    #[serde(default)]
    pub fe_locations: Vec<(String, usize)>,
    /// Backend handler location (file, line, impl_symbol) - None if missing
    pub be_location: Option<(String, usize, String)>,
    /// Status: "ok", "missing_handler", "unused_handler", "unregistered_handler"
    pub status: String,
    /// Language (ts, rs, etc.)
    #[serde(default)]
    pub language: String,
    /// Communication pattern: "invoke" | "invoke+emit" | "emit-only"
    #[serde(default)]
    pub comm_type: String,
    /// Events emitted by this command's handler
    #[serde(default)]
    pub emits_events: Vec<String>,
}

/// High-priority task for a first-shot plan (action + verify).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PriorityTask {
    /// Priority rank (1 = highest).
    pub priority: u8,
    /// Task category, e.g. "dead_export", "duplicate".
    pub kind: String,
    /// Primary target (symbol, file, or module).
    pub target: String,
    /// Location hint (file:line).
    pub location: String,
    /// Why this is important.
    pub why: String,
    /// Risk severity of leaving it unfixed: high|medium|low
    pub risk: String,
    /// Suggested fix (short).
    pub fix_hint: String,
    /// Verification command to confirm fix.
    pub verify_cmd: String,
}

/// High-connectivity file that makes a good context anchor.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HubFile {
    /// Relative path of the file.
    pub path: String,
    /// Lines of code in the file.
    pub loc: usize,
    /// Number of imports.
    pub imports_count: usize,
    /// Number of exports.
    pub exports_count: usize,
    /// Number of importers (reverse deps).
    pub importers_count: usize,
    /// Number of registered commands (if any).
    pub commands_count: usize,
    /// Suggested command to slice this file.
    pub slice_cmd: String,
}

/// High fan-in file surfaced by the import hotspot analyzer.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HotspotFile {
    /// Relative path of the file.
    pub file: String,
    /// Number of files importing this file.
    pub importers: usize,
    /// Hotspot category, e.g. CORE, SHARED, or PERIPHERAL.
    pub category: String,
    /// Suggested command to inspect this hotspot.
    pub slice_cmd: String,
}

/// Directory or file node used by the report tree view.
///
/// Contains relative path, aggregated LOC, and children.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TreeNode {
    /// Relative path of this file/directory.
    pub path: String,
    /// Lines of code aggregated for this node (file LOC + children).
    pub loc: usize,
    /// Child nodes.
    #[serde(default)]
    pub children: Vec<TreeNode>,
}

/// A gap between frontend command invocations and backend handlers.
///
/// Used for Tauri applications to track:
/// - Commands called from frontend but missing in backend
/// - Backend handlers that exist but aren't registered
/// - Registered handlers never called from frontend
///
/// # Example
///
/// ```rust
/// use report_leptos::types::CommandGap;
///
/// let gap = CommandGap {
///     name: "get_user_data".into(),
///     implementation_name: Some("getUserData".into()),
///     locations: vec![
///         ("src/api.ts".into(), 42),
///         ("src/components/Profile.tsx".into(), 15),
///     ],
///     ..Default::default()
/// };
/// ```
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CommandGap {
    /// Command name as invoked
    pub name: String,
    /// Actual implementation name (if different due to case conversion)
    pub implementation_name: Option<String>,
    /// File paths and line numbers where this command appears
    pub locations: Vec<(String, usize)>,
    /// Detection confidence level
    pub confidence: Option<Confidence>,
    /// String literal matches that may indicate dynamic usage
    #[serde(default)]
    pub string_literal_matches: Vec<StringLiteralMatch>,
}

/// AI-generated insight about code quality or potential issues.
///
/// Insights are suggestions generated during analysis that highlight
/// patterns, anti-patterns, or areas for improvement.
///
/// # Severity Levels
///
/// - `"high"` - Critical issues requiring immediate attention
/// - `"medium"` - Important but not urgent
/// - `"low"` - Suggestions and nice-to-haves
///
/// # Example
///
/// ```rust
/// use report_leptos::types::AiInsight;
///
/// let insight = AiInsight {
///     title: "Large Module Detected".into(),
///     severity: "medium".into(),
///     message: "Consider splitting utils.ts (1500 LOC) into smaller modules.".into(),
/// };
/// ```
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AiInsight {
    /// Short title for the insight
    pub title: String,
    /// Severity: "high", "medium", or "low"
    pub severity: String,
    /// Detailed explanation and recommendations
    pub message: String,
}

/// A node in the import/dependency graph.
///
/// Each node represents a file in the codebase with positioning
/// data for graph visualization.
///
/// # Fields
///
/// - `x`, `y` - Pre-computed positions (0.0-1.0 normalized)
/// - `component` - ID of the connected component this node belongs to
/// - `degree` - Number of edges (imports + exports)
/// - `detached` - True if node has no connections
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphNode {
    /// Unique identifier (usually file path)
    pub id: String,
    /// Display label (usually filename)
    pub label: String,
    /// Lines of code in this file
    pub loc: usize,
    /// X position (0.0-1.0)
    pub x: f32,
    /// Y position (0.0-1.0)
    pub y: f32,
    /// Connected component ID
    pub component: usize,
    /// Edge count (in + out degree)
    pub degree: usize,
    /// True if isolated (no imports or exports)
    pub detached: bool,
}

/// A connected component in the import graph.
///
/// The graph is decomposed into connected components to identify
/// isolated module clusters. The main component (largest) typically
/// represents the core application, while smaller components may
/// indicate dead code or independent utilities.
///
/// # Tauri Integration
///
/// For Tauri apps, `tauri_frontend` and `tauri_backend` count how many
/// nodes in this component belong to frontend vs backend code.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphComponent {
    /// Component ID (0 = main/largest)
    pub id: usize,
    /// Number of nodes in this component
    pub size: usize,
    /// Number of edges in this component
    #[serde(rename = "edges")]
    pub edge_count: usize,
    /// List of node IDs in this component
    pub nodes: Vec<String>,
    /// Count of isolated (degree-0) nodes
    pub isolated_count: usize,
    /// Sample node for preview
    pub sample: String,
    /// Total lines of code in component
    pub loc_sum: usize,
    /// True if entirely detached from main component
    pub detached: bool,
    /// Count of frontend files (Tauri apps)
    pub tauri_frontend: usize,
    /// Count of backend files (Tauri apps)
    pub tauri_backend: usize,
}

/// Complete graph data for visualization.
///
/// Contains all nodes, edges, and component metadata needed
/// to render an interactive dependency graph with Cytoscape.js or DOT/graphviz.
///
/// # Example
///
/// ```rust
/// use report_leptos::types::{GraphData, GraphNode, GraphComponent};
///
/// let graph = GraphData {
///     nodes: vec![
///         GraphNode {
///             id: "src/main.ts".into(),
///             label: "main.ts".into(),
///             loc: 150,
///             x: 0.5,
///             y: 0.5,
///             component: 0,
///             degree: 5,
///             detached: false,
///         }
///     ],
///     edges: vec![
///         ("src/main.ts".into(), "src/utils.ts".into(), "import".into()),
///     ],
///     components: vec![],
///     main_component_id: 0,
///     ..Default::default()
/// };
///
/// // Convert to DOT format for graphviz/dot_ix
/// let dot = graph.to_dot();
/// assert!(dot.contains("digraph"));
/// ```
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GraphData {
    /// All nodes (files) in the graph
    pub nodes: Vec<GraphNode>,
    /// Edges as (from, to, kind) tuples
    pub edges: Vec<(String, String, String)>,
    /// Connected components
    pub components: Vec<GraphComponent>,
    /// ID of the main (largest) component
    pub main_component_id: usize,
    /// Whether this graph was truncated due to size limits
    #[serde(default)]
    pub truncated: bool,
    /// Total number of nodes before truncation (same as nodes.len() if not truncated)
    #[serde(default)]
    pub total_nodes: usize,
    /// Total number of edges before truncation (same as edges.len() if not truncated)
    #[serde(default)]
    pub total_edges: usize,
    /// Reason for truncation, if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<String>,
}

impl GraphData {
    /// Convert graph data to DOT format for graphviz/dot_ix rendering.
    ///
    /// Generates a DOT language representation with:
    /// - Node styling based on LOC (size), component (color), detached status
    /// - Edge styling based on kind (import vs reexport)
    /// - Subgraph clusters for connected components
    ///
    /// # Example
    ///
    /// ```rust
    /// use report_leptos::types::GraphData;
    ///
    /// let graph = GraphData::default();
    /// let dot = graph.to_dot();
    /// assert!(dot.starts_with("digraph loctree"));
    /// ```
    pub fn to_dot(&self) -> String {
        let mut dot = String::with_capacity(self.nodes.len() * 100 + self.edges.len() * 50);

        dot.push_str("digraph loctree {\n");
        dot.push_str("  // Graph attributes\n");
        dot.push_str(
            "  graph [rankdir=TB, splines=true, overlap=false, nodesep=0.5, ranksep=0.8];\n",
        );
        dot.push_str(
            "  node [shape=box, style=\"rounded,filled\", fontname=\"sans-serif\", fontsize=10];\n",
        );
        dot.push_str("  edge [arrowsize=0.7, fontsize=8];\n\n");

        // Group nodes by component for subgraph clustering
        let mut component_nodes: std::collections::HashMap<usize, Vec<&GraphNode>> =
            std::collections::HashMap::new();
        for node in &self.nodes {
            component_nodes
                .entry(node.component)
                .or_default()
                .push(node);
        }

        // Render each component as a subgraph cluster
        for (comp_id, nodes) in &component_nodes {
            let is_main = *comp_id == self.main_component_id;
            let cluster_style = if is_main {
                "style=invis" // Main component: no visible cluster border
            } else {
                "style=dashed, color=\"#888888\"" // Other components: dashed border
            };

            dot.push_str(&format!("  subgraph cluster_{} {{\n", comp_id));
            dot.push_str(&format!("    {};\n", cluster_style));
            dot.push_str(&format!("    label=\"Component {}\";\n", comp_id));

            for node in nodes {
                let escaped_id = escape_dot_string(&node.id);
                let escaped_label = escape_dot_string(&node.label);

                // Node color based on status
                let fill_color = if node.detached {
                    "#d1830f" // Orange for detached
                } else if *comp_id == self.main_component_id {
                    "#4f81e1" // Blue for main component
                } else {
                    "#6c757d" // Gray for other components
                };

                // Node size based on LOC (min 0.3, max 1.5)
                let size = 0.3 + (node.loc as f32 / 500.0).min(1.2);

                dot.push_str(&format!(
                    "    \"{}\" [label=\"{}\\n({} LOC)\", fillcolor=\"{}\", width={:.2}, height={:.2}];\n",
                    escaped_id, escaped_label, node.loc, fill_color, size, size * 0.6
                ));
            }

            dot.push_str("  }\n\n");
        }

        // Render edges
        dot.push_str("  // Edges\n");
        for (from, to, kind) in &self.edges {
            let escaped_from = escape_dot_string(from);
            let escaped_to = escape_dot_string(to);

            let edge_style = match kind.as_str() {
                "reexport" => "color=\"#e67e22\", style=bold",
                _ => "color=\"#888888\"",
            };

            dot.push_str(&format!(
                "  \"{}\" -> \"{}\" [{}];\n",
                escaped_from, escaped_to, edge_style
            ));
        }

        dot.push_str("}\n");
        dot
    }

    /// Convert graph data to DOT format with dark theme colors.
    pub fn to_dot_dark(&self) -> String {
        // Same structure but with dark-theme-appropriate colors
        let mut dot = String::with_capacity(self.nodes.len() * 100 + self.edges.len() * 50);

        dot.push_str("digraph loctree {\n");
        dot.push_str("  graph [rankdir=TB, splines=true, overlap=false, nodesep=0.5, ranksep=0.8, bgcolor=\"#0f1115\"];\n");
        dot.push_str("  node [shape=box, style=\"rounded,filled\", fontname=\"sans-serif\", fontsize=10, fontcolor=\"#eef2ff\"];\n");
        dot.push_str("  edge [arrowsize=0.7, fontsize=8, fontcolor=\"#aaa\"];\n\n");

        // Simplified rendering for dark theme (same structure, different colors)
        for node in &self.nodes {
            let escaped_id = escape_dot_string(&node.id);
            let escaped_label = escape_dot_string(&node.label);

            let fill_color = if node.detached {
                "#d1830f"
            } else if node.component == self.main_component_id {
                "#4f81e1"
            } else {
                "#4a5568"
            };

            dot.push_str(&format!(
                "  \"{}\" [label=\"{}\\n({} LOC)\", fillcolor=\"{}\"];\n",
                escaped_id, escaped_label, node.loc, fill_color
            ));
        }

        for (from, to, kind) in &self.edges {
            let escaped_from = escape_dot_string(from);
            let escaped_to = escape_dot_string(to);

            let edge_style = match kind.as_str() {
                "reexport" => "color=\"#e67e22\"",
                _ => "color=\"#666666\"",
            };

            dot.push_str(&format!(
                "  \"{}\" -> \"{}\" [{}];\n",
                escaped_from, escaped_to, edge_style
            ));
        }

        dot.push_str("}\n");
        dot
    }
}

/// Escape special characters for DOT string literals.
fn escape_dot_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Location of a duplicate export with line number
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DupLocation {
    /// File path
    pub file: String,
    /// Line number (1-indexed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

/// Severity levels for duplicate exports
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DupSeverity {
    /// Cross-language expected (Rust↔TS DTOs) - noise
    CrossLangExpected = 0,
    /// Re-exports and generic names (new, from, clone) - usually OK
    ReExportOrGeneric = 1,
    /// Same-package duplicate - potential issue
    #[default]
    SamePackage = 2,
    /// Same symbol in different modules/packages - worth reviewing
    CrossModule = 3,
    /// Same symbol in different crates/packages - REAL issue
    CrossCrate = 4,
}

/// A duplicate export found across multiple files.
///
/// Identifies symbols (functions, classes, types) that are exported
/// from multiple locations, which may indicate copy-paste code or
/// naming collisions.
///
/// # Scoring
///
/// The `score` combines frequency and context:
/// - Higher score = more problematic
/// - `prod_count` vs `dev_count` helps prioritize production code
///
/// # Example
///
/// ```rust
/// use report_leptos::types::RankedDup;
///
/// let dup = RankedDup {
///     name: "formatDate".into(),
///     files: vec![
///         "src/utils/date.ts".into(),
///         "src/helpers/format.ts".into(),
///         "src/legacy/utils.ts".into(),
///     ],
///     score: 15,
///     prod_count: 2,
///     dev_count: 1,
///     canonical: "src/utils/date.ts".into(),
///     refactors: vec![
///         "src/helpers/format.ts".into(),
///         "src/legacy/utils.ts".into(),
///     ],
///     ..Default::default()
/// };
/// ```
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RankedDup {
    /// Export name
    pub name: String,
    /// All files exporting this symbol
    pub files: Vec<String>,
    /// Locations with line numbers (file, line)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub locations: Vec<DupLocation>,
    /// Priority score (higher = more important to fix)
    pub score: usize,
    /// Count in production code paths
    pub prod_count: usize,
    /// Count in dev/test code paths
    pub dev_count: usize,
    /// Recommended canonical location
    pub canonical: String,
    /// Line number in canonical file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_line: Option<usize>,
    /// Files that should import from canonical instead
    pub refactors: Vec<String>,
    /// Severity level: 0=cross-lang expected, 1=same-package, 2=semantic conflict
    #[serde(default)]
    pub severity: DupSeverity,
    /// True if duplicate spans multiple languages (Rust↔TS)
    #[serde(default)]
    pub is_cross_lang: bool,
    /// Distinct packages/directories containing this symbol
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<String>,
    /// Explanation for the severity classification
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// Match reason for crowd membership (mirrors loctree-rs::analyzer::crowd::MatchReason).
#[derive(Clone, Debug, Serialize, Deserialize)]
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

/// Issue detected in a crowd (mirrors loctree-rs::analyzer::crowd::CrowdIssue).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CrowdIssue {
    /// Multiple files with very similar names
    NameCollision {
        /// Files with colliding names
        files: Vec<String>,
    },
    /// Some files have much lower usage than others
    UsageAsymmetry {
        /// The primary/most-used file
        primary: String,
        /// Underused files that might be redundant
        underused: Vec<String>,
    },
    /// Files export similar things
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

/// A member of a crowd.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CrowdMember {
    /// File path
    pub file: String,
    /// Why this file matched the crowd
    pub match_reason: MatchReason,
    /// Number of files importing this one
    pub importer_count: usize,
    /// Similarity scores with other members (file, score)
    #[serde(default)]
    pub similarity_scores: Vec<(String, f32)>,
    /// Whether this is a test file
    #[serde(default)]
    pub is_test: bool,
}

/// A group of files with similar names/patterns.
///
/// Crowds indicate potential naming collisions, fragmentation,
/// or copy-paste duplication across the codebase.
///
/// # Scoring
///
/// - 0-4: Low severity (acceptable naming patterns)
/// - 4-7: Medium severity (worth reviewing)
/// - 7-10: High severity (likely problematic)
///
/// # Example
///
/// ```rust
/// use report_leptos::types::{Crowd, CrowdMember, MatchReason, CrowdIssue};
///
/// let crowd = Crowd {
///     pattern: "message".into(),
///     members: vec![
///         CrowdMember {
///             file: "src/message.ts".into(),
///             match_reason: MatchReason::NameMatch {
///                 matched: "message".into(),
///             },
///             importer_count: 15,
///             similarity_scores: vec![],
///             is_test: false,
///         },
///         CrowdMember {
///             file: "src/components/Message.tsx".into(),
///             match_reason: MatchReason::NameMatch {
///                 matched: "Message".into(),
///             },
///             importer_count: 8,
///             similarity_scores: vec![],
///             is_test: false,
///         },
///     ],
///     score: 6.5,
///     issues: vec![
///         CrowdIssue::NameCollision {
///             files: vec!["src/message.ts".into(), "src/components/Message.tsx".into()],
///         },
///     ],
/// };
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Crowd {
    /// Pattern name (e.g., "message", "chat")
    pub pattern: String,
    /// Files matching this pattern
    pub members: Vec<CrowdMember>,
    /// Severity score (0-10, higher = worse)
    pub score: f32,
    /// Issues detected in this crowd
    #[serde(default)]
    pub issues: Vec<CrowdIssue>,
}

/// A dead export (symbol exported but never imported).
///
/// Represents code that appears to be unused - exported from a file
/// but never imported anywhere in the analyzed codebase.
///
/// # Example
///
/// ```rust
/// use report_leptos::types::DeadExport;
///
/// let dead = DeadExport {
///     file: "src/utils/legacy.ts".into(),
///     symbol: "formatOldDate".into(),
///     line: Some(42),
///     confidence: "very-high".into(),
///     reason: "No imports found in codebase".into(),
///     open_url: Some("loctree://open?f=src/utils/legacy.ts&l=42".into()),
///     is_test: false,
/// };
/// ```
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeadExport {
    /// File path containing the dead export
    pub file: String,
    /// Symbol name (function, class, type, etc.)
    pub symbol: String,
    /// Line number where symbol is defined (1-indexed)
    pub line: Option<usize>,
    /// Confidence level: "high", "very-high"
    pub confidence: String,
    /// Human-readable reason why this is considered dead
    pub reason: String,
    /// Optional URL for opening in editor (loctree://open protocol)
    pub open_url: Option<String>,
    /// Whether this is a test file
    #[serde(default)]
    pub is_test: bool,
}

/// Bundle distribution analysis derived from production source maps.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DistReport {
    /// Source directory that was compared against the bundle(s)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[serde(rename = "srcDir")]
    pub src_dir: String,
    /// Paths of source maps used for the analysis
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "sourceMapPaths")]
    pub source_map_paths: Vec<String>,
    /// Number of source maps included in the comparison
    #[serde(rename = "sourceMaps")]
    pub source_maps: usize,
    /// Total exports discovered in source
    #[serde(rename = "sourceExports")]
    pub source_exports: usize,
    /// Exports still present in at least one bundle
    #[serde(rename = "bundledExports")]
    pub bundled_exports: usize,
    /// Exports fully tree-shaken out of the bundle surface
    #[serde(rename = "deadExports")]
    pub dead_exports: Vec<DistDeadExport>,
    /// Human-friendly reduction percentage (legacy field kept for compatibility)
    pub reduction: String,
    /// True when every source map had symbol-level coverage
    #[serde(rename = "symbolLevel")]
    pub symbol_level: bool,
    /// Analysis granularity: file, symbol, or mixed
    #[serde(rename = "analysisLevel")]
    pub analysis_level: DistAnalysisLevel,
    /// Number of exports removed from the bundle surface
    #[serde(rename = "treeShakenExports")]
    pub tree_shaken_exports: usize,
    /// Percentage of exports removed from the bundle surface
    #[serde(rename = "treeShakenPct")]
    pub tree_shaken_pct: usize,
    /// Percentage of exports retained in at least one bundle
    #[serde(rename = "coveragePct")]
    pub coverage_pct: usize,
    /// Per-file rollup for impacted files
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "impactedFiles")]
    pub impacted_files: Vec<DistFileImpact>,
    /// Candidate class counts derived from the chunk matrix
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(rename = "candidateCounts")]
    pub candidate_counts: BTreeMap<String, usize>,
    /// Ranked runtime candidates derived from bundle/chunk coverage
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<DistCandidate>,
}

/// Per-file rollup for bundle distribution analysis.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DistFileImpact {
    /// Source file path
    pub file: String,
    /// Exports declared in the file
    #[serde(rename = "sourceExports")]
    pub source_exports: usize,
    /// Exports retained in at least one bundle
    #[serde(rename = "bundledExports")]
    pub bundled_exports: usize,
    /// Exports removed from the bundle surface
    #[serde(rename = "treeShakenExports")]
    pub tree_shaken_exports: usize,
    /// Status: fully-shaken or partially-shaken
    pub status: String,
}

/// Single export removed from all analyzed bundles.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DistDeadExport {
    /// File path containing the export
    pub file: String,
    /// Line number of the export
    pub line: usize,
    /// Exported symbol name
    pub name: String,
    /// Export kind (function, const, type, ...)
    pub kind: String,
}

/// Ranked runtime candidate from the dist analyzer.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DistCandidate {
    /// File containing the export
    pub file: String,
    /// Line number of the export
    pub line: usize,
    /// Exported symbol name
    pub name: String,
    /// Export kind (function, const, type, ...)
    pub kind: String,
    /// Candidate class
    #[serde(rename = "class")]
    pub class_name: DistCandidateClass,
    /// Confidence level
    pub confidence: DistConfidence,
    /// Ranking score from the analyzer
    pub rank: usize,
    /// Number of chunks where the export appears
    #[serde(rename = "seenInChunks")]
    pub seen_in_chunks: usize,
    /// Number of boot chunks where the export appears
    #[serde(rename = "bootChunks")]
    pub boot_chunks: usize,
    /// Number of feature chunks where the export appears
    #[serde(rename = "featureChunks")]
    pub feature_chunks: usize,
    /// Human-readable chunk labels
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "chunkNames")]
    pub chunk_names: Vec<String>,
    /// Source modules that dynamically import this file
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "dynamicImporters")]
    pub dynamic_importers: Vec<String>,
    /// Source modules that statically import this file
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "staticImporters")]
    pub static_importers: Vec<String>,
    /// Analyzer notes
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Candidate class emitted by the dist analyzer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistCandidateClass {
    /// Export is absent from every analyzed chunk.
    DeadInAllChunks,
    /// Export only appears in boot-path chunks.
    BootPathOnly,
    /// Export only appears in feature chunks.
    FeatureLocal,
    /// Export is marked lazy in source but still shows up in boot chunks.
    FakeLazy,
    /// Export needs manual verification before deletion.
    #[default]
    VerifyFirst,
}

/// Confidence level for dist candidates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DistConfidence {
    /// Weak evidence; verify manually.
    #[default]
    Low,
    /// Reasonable signal with some ambiguity.
    Medium,
    /// Strong signal with low ambiguity.
    High,
}

/// Granularity of source-map coverage used by the distribution analysis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DistAnalysisLevel {
    /// File-level coverage only
    #[default]
    File,
    /// Line-level coverage from source map mappings
    Line,
    /// Symbol-level coverage for every source map
    Symbol,
    /// Mixed file-level and symbol-level coverage
    Mixed,
}

/// A gap in test coverage
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoverageGap {
    /// Type of gap (handler_without_test, event_without_test, etc.)
    pub kind: GapKind,
    /// Target symbol/handler name
    pub target: String,
    /// Location (file:line)
    pub location: String,
    /// Severity level
    pub severity: Severity,
    /// Recommendation message
    pub recommendation: String,
    /// Additional context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Related file paths
    #[serde(default)]
    pub files: Vec<String>,
}

/// Type of coverage gap
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    /// Handler used in production but not tested
    HandlerWithoutTest,
    /// Event emitted in production but not tested
    EventWithoutTest,
    /// Export used in production but not tested
    ExportWithoutTest,
    /// Tested but not used in production (suspicious)
    TestedButUnused,
}

/// Severity level for coverage gaps
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Critical - Handler without test (can break runtime)
    Critical,
    /// High - Event without test (data flow issues)
    High,
    /// Medium - Export without test (integration gaps)
    Medium,
    /// Low - Tested but unused (cleanup candidate)
    Low,
}

/// Twins analysis data (dead parrots, exact twins, barrel chaos)
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TwinsData {
    /// Dead parrots - symbols with 0 imports
    pub dead_parrots: Vec<DeadParrot>,
    /// Exact twins - symbols with same name in different files
    pub exact_twins: Vec<ExactTwin>,
    /// Barrel chaos analysis
    pub barrel_chaos: BarrelChaos,
}

/// A dead parrot - symbol exported but never imported
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeadParrot {
    /// Symbol name
    pub name: String,
    /// File path where exported
    pub file_path: String,
    /// Line number
    pub line: usize,
    /// Symbol kind (function, type, const, class, etc.)
    pub kind: String,
    /// Whether this is a test file
    #[serde(default)]
    pub is_test: bool,
}

/// An exact twin - symbol with same name exported from multiple files
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExactTwin {
    /// Symbol name
    pub name: String,
    /// All locations where this symbol is exported
    pub locations: Vec<TwinLocation>,
}

/// A location where an exact twin is found
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TwinLocation {
    /// File path
    pub file_path: String,
    /// Line number
    pub line: usize,
    /// Export kind
    pub kind: String,
    /// Number of imports
    pub import_count: usize,
    /// True if this is the canonical (recommended) location
    pub is_canonical: bool,
}

/// Barrel chaos analysis
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BarrelChaos {
    /// Directories missing barrel files
    pub missing_barrels: Vec<MissingBarrel>,
    /// Deep re-export chains
    pub deep_chains: Vec<ReexportChain>,
    /// Inconsistent import paths
    pub inconsistent_paths: Vec<InconsistentImport>,
}

/// A directory missing a barrel file
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MissingBarrel {
    /// Directory path
    pub directory: String,
    /// Number of files in directory
    pub file_count: usize,
    /// Number of external imports
    pub external_import_count: usize,
}

/// A deep re-export chain
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReexportChain {
    /// Symbol name
    pub symbol: String,
    /// Chain of files (from consumer to definition)
    pub chain: Vec<String>,
    /// Depth of chain
    pub depth: usize,
}

/// Inconsistent import path for a symbol
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InconsistentImport {
    /// Symbol name
    pub symbol: String,
    /// Canonical (most-used) path
    pub canonical_path: String,
    /// Alternative paths with usage counts
    pub alternative_paths: Vec<(String, usize)>,
}

// ============================================================================
// Refactor Plan Types
// ============================================================================

/// A single file move in the refactor plan.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RefactorMove {
    /// Source file path
    pub source: String,
    /// Target file path
    pub target: String,
    /// Current architectural layer
    pub current_layer: String,
    /// Target architectural layer
    pub target_layer: String,
    /// Risk level (low, medium, high)
    pub risk: String,
    /// Lines of code in file
    pub loc: usize,
    /// Number of direct consumers (importers)
    pub direct_consumers: usize,
    /// Reason for move suggestion
    pub reason: String,
    /// Verification command
    pub verify_cmd: String,
}

/// A shim suggestion for backward compatibility.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RefactorShim {
    /// Original file path (where shim will be created)
    pub old_path: String,
    /// New file path (where code was moved)
    pub new_path: String,
    /// Number of importers that would need updating
    pub importer_count: usize,
    /// Generated shim code (pub use statement)
    pub code: String,
}

/// A phase in the refactor execution plan.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RefactorPhase {
    /// Phase name (e.g., "Phase 1: LOW Risk")
    pub name: String,
    /// Risk level for this phase (low, medium, high)
    pub risk: String,
    /// Moves in this phase
    #[serde(default)]
    pub moves: Vec<RefactorMove>,
    /// Git commands for this phase
    pub git_script: String,
}

/// Statistics about the refactor plan.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RefactorStats {
    /// Total files analyzed
    pub total_files: usize,
    /// Files that need to move
    pub files_to_move: usize,
    /// Shims that should be created
    pub shims_needed: usize,
    /// Layer distribution before refactoring (layer -> count)
    #[serde(default)]
    pub layer_before: std::collections::HashMap<String, usize>,
    /// Layer distribution after refactoring (layer -> count)
    #[serde(default)]
    pub layer_after: std::collections::HashMap<String, usize>,
    /// Risk breakdown (risk level -> count)
    #[serde(default)]
    pub by_risk: std::collections::HashMap<String, usize>,
}

/// Complete refactor plan data for visualization.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RefactorPlanData {
    /// Target directory analyzed
    pub target: String,
    /// Execution phases ordered by risk (LOW -> MEDIUM -> HIGH)
    #[serde(default)]
    pub phases: Vec<RefactorPhase>,
    /// Suggested shims for backward compatibility
    #[serde(default)]
    pub shims: Vec<RefactorShim>,
    /// Groups of files with cyclic dependencies
    #[serde(default)]
    pub cyclic_groups: Vec<Vec<String>>,
    /// Statistics summary
    #[serde(default)]
    pub stats: RefactorStats,
}

/// A complete report section for one analyzed directory.
///
/// This is the main data structure passed to [`crate::render_report`].
/// Each section represents analysis results for one source root.
///
/// # Example
///
/// ```rust
/// use report_leptos::types::ReportSection;
///
/// let section = ReportSection {
///     root: "packages/my-app/src".into(),
///     files_analyzed: 234,
///     analyze_limit: 500,
///     command_counts: (15, 18), // 15 frontend calls, 18 backend handlers
///     ..Default::default()
/// };
/// ```
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReportSection {
    /// Root directory that was analyzed
    pub root: String,
    /// Number of files analyzed
    pub files_analyzed: usize,
    /// Total lines of code across all analyzed files
    pub total_loc: usize,
    /// Number of files with re-exports (barrel files)
    pub reexport_files_count: usize,
    /// Number of files with dynamic imports
    pub dynamic_imports_count: usize,
    /// Duplicate exports ranked by priority
    pub ranked_dups: Vec<RankedDup>,
    /// Cascade import pairs (source, target)
    pub cascades: Vec<(String, String)>,
    /// Circular import components (strict cycles)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub circular_imports: Vec<Vec<String>>,
    /// Lazy circular imports (cycles only via dynamic/lazy imports)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lazy_circular_imports: Vec<Vec<String>>,
    /// Dynamic imports per file
    pub dynamic: Vec<(String, Vec<String>)>,
    /// Maximum files to analyze (0 = unlimited)
    pub analyze_limit: usize,
    /// Report generation time (RFC3339)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
    /// Schema name for artifact payload
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    /// Schema version for artifact payload
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
    /// Loctree CLI/library version that produced this report.
    ///
    /// Populated from `env!("CARGO_PKG_VERSION")` at the analyzer boundary so
    /// the rendered artifact carries the same provenance as JSON/MCP outputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loctree_version: Option<String>,
    /// Frontend commands missing backend handlers
    pub missing_handlers: Vec<CommandGap>,
    /// Backend handlers not registered in generate_handler![]
    pub unregistered_handlers: Vec<CommandGap>,
    /// Registered handlers never called from frontend
    pub unused_handlers: Vec<CommandGap>,
    /// (frontend_command_count, backend_handler_count)
    pub command_counts: (usize, usize),
    /// Full command bridges for FE↔BE comparison table
    #[serde(default)]
    pub command_bridges: Vec<CommandBridge>,
    /// Base URL for opening files in editor
    pub open_base: Option<String>,
    /// Directory tree with LOC per node
    #[serde(default)]
    pub tree: Option<Vec<TreeNode>>,
    /// Dependency graph data (if generated)
    pub graph: Option<GraphData>,
    /// Warning if graph was skipped (too large, etc.)
    pub graph_warning: Option<String>,
    /// AI-generated insights
    pub insights: Vec<AiInsight>,
    /// Git branch name (if available)
    pub git_branch: Option<String>,
    /// Git commit hash (if available)
    pub git_commit: Option<String>,
    /// Top actionable tasks (why + fix + verify)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub priority_tasks: Vec<PriorityTask>,
    /// High-connectivity context anchors
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hub_files: Vec<HubFile>,
    /// Import fan-in hotspots from the analyzer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hotspots: Vec<HotspotFile>,
    /// Crowd analysis results (naming collision detection)
    #[serde(default)]
    pub crowds: Vec<Crowd>,
    /// Dead exports (exported but never imported)
    #[serde(default)]
    pub dead_exports: Vec<DeadExport>,
    /// Bundle distribution analysis (source-map-backed tree-shaking view)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dist: Option<DistReport>,
    /// Twins analysis (dead parrots, exact twins, barrel chaos)
    #[serde(default, alias = "twins_data")]
    pub twins: Option<TwinsData>,
    /// Test coverage gaps (handlers/events without tests)
    #[serde(default)]
    pub coverage_gaps: Vec<CoverageGap>,
    /// Overall health score 0-100 (higher is better)
    #[serde(default)]
    pub health_score: Option<u8>,
    /// Refactor plan data (architectural reorganization suggestions)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refactor_plan: Option<RefactorPlanData>,
    /// Context Atlas pointer (when `loct auto` materialized navigable cards
    /// under `<artifacts_dir>/context-atlas/`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_atlas: Option<ContextAtlasInfo>,
}

/// Lightweight pointer to a materialized Context Atlas surface.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContextAtlasInfo {
    /// Absolute path to the materialized atlas directory.
    pub atlas_dir: String,
    /// Absolute path to the human-readable manifest (`manifest.md`).
    pub manifest: String,
    /// Absolute path to the machine-readable manifest (`manifest.json`).
    pub manifest_json: String,
    /// Absolute path to the recommended first card (`00-core-map.md`).
    pub recommended_start: String,
    /// One-line summary suitable for top-of-report rendering.
    pub message: String,
    /// Atlas card pointers for the recommended reading path.
    #[serde(default)]
    pub cards: Vec<ContextAtlasCardInfo>,
}

/// Metadata for a single Context Atlas card.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContextAtlasCardInfo {
    /// Card identifier (e.g. `"core"`, `"structural"`, `"runtime"`).
    pub id: String,
    /// Human-readable title (e.g. `"Core Map"`).
    pub title: String,
    /// Card filename inside the atlas dir (e.g. `"00-core-map.md"`).
    pub path: String,
    /// Line count of the rendered card.
    pub lines: usize,
    /// One-line "why read this card" hint.
    pub why: String,
}
