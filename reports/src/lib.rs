#![recursion_limit = "256"]

//! # report-leptos
//!
//! Leptos SSR renderer for generating static HTML reports.
//!
//! This crate provides a type-safe, component-based approach to generating
//! beautiful HTML reports using [Leptos](https://leptos.dev/) server-side rendering.
//! Originally built for [loctree](https://github.com/Loctree/Loctree) codebase
//! analysis, it can be used independently for any static report generation needs.
//!
//! ## Features
//!
//! - **Zero JavaScript Runtime** - Pure SSR, no hydration needed
//! - **Component-Based** - Modular, reusable UI components
//! - **Type-Safe** - Full Rust type safety from data to HTML
//! - **Interactive Graphs** - Cytoscape.js integration for dependency visualization
//!
//! ## Quick Start
//!
//! ```rust
//! use report_leptos::{render_report, JsAssets, types::ReportSection};
//!
//! // Create report data
//! let section = ReportSection {
//!     root: "my-project".into(),
//!     files_analyzed: 42,
//!     ..Default::default()
//! };
//!
//! // Configure JS assets (optional, for graph visualization)
//! let js_assets = JsAssets::default();
//!
//! // Render to HTML string
//! let html = render_report(&[section], &js_assets, false);
//!
//! // Write to file
//! std::fs::write("report.html", html).unwrap();
//! ```
//!
//! ## Architecture
//!
//! The crate is organized into modules:
//!
//! - [`types`] - Data structures for report content
//! - [`components`] - Leptos UI components
//! - [`styles`] - CSS constants
//!
//! ## Leptos 0.8 SSR
//!
//! This library uses Leptos 0.8's `RenderHtml` trait:
//!
//! ```rust,ignore
//! use leptos::tachys::view::RenderHtml;
//!
//! let view = view! { <MyComponent /> };
//! let html: String = view.to_html();
//! ```
//!
//! No reactive runtime or hydration is needed - pure static HTML generation.
//!
//! ---
//!
//! Developed with 💀 by The Loctree Team ⓒ 2025-2026

#![doc(html_root_url = "https://docs.rs/report-leptos/0.1.0")]
#![warn(missing_docs)]
#![warn(rustdoc::missing_crate_level_docs)]

pub mod components;
pub mod styles;
pub mod types;

use components::ReportDocument;
use leptos::prelude::*;
use leptos::tachys::view::RenderHtml;
use types::ReportSection;

/// Render a complete HTML report from analyzed sections.
///
/// This is the main entry point for generating reports. It takes a slice of
/// [`ReportSection`] data and produces a complete HTML document as a string.
///
/// # Arguments
///
/// * `sections` - Slice of report sections to render
/// * `js_assets` - Paths to JavaScript assets for graph visualization
/// * `has_tauri` - Whether to show Tauri coverage tab (only for Tauri projects)
///
/// # Returns
///
/// A complete HTML document as a `String`, including `<!DOCTYPE html>`.
///
/// # Example
///
/// ```rust
/// use report_leptos::{render_report, JsAssets, types::ReportSection};
///
/// let section = ReportSection {
///     root: "src".into(),
///     files_analyzed: 100,
///     ..Default::default()
/// };
///
/// let html = render_report(&[section], &JsAssets::default(), false);
/// assert!(html.starts_with("<!DOCTYPE html>"));
/// ```
pub fn render_report(sections: &[ReportSection], js_assets: &JsAssets, has_tauri: bool) -> String {
    let doc = view! {
        <ReportDocument sections=sections.to_vec() js_assets=js_assets.clone() has_tauri=has_tauri />
    };

    let html = doc.to_html();

    // Leptos doesn't include DOCTYPE, so we add it
    format!("<!DOCTYPE html>\n{}", html)
}

/// JavaScript asset paths for graph visualization.
///
/// The report uses [Cytoscape.js](https://js.cytoscape.org/) with layout plugins
/// for interactive dependency graph visualization. You can provide paths to:
///
/// - CDN URLs (e.g., unpkg.com)
/// - Local bundled files (for offline use)
/// - Empty strings (graph will show placeholder)
///
/// # Example
///
/// ```rust
/// use report_leptos::JsAssets;
///
/// // CDN paths (with Cytoscape fallback, no WASM)
/// let assets = JsAssets {
///     cytoscape_path: "https://unpkg.com/cytoscape@3/dist/cytoscape.min.js".into(),
///     dagre_path: "https://unpkg.com/dagre@0.8/dist/dagre.min.js".into(),
///     cytoscape_dagre_path: "https://unpkg.com/cytoscape-dagre@2/cytoscape-dagre.js".into(),
///     layout_base_path: "https://unpkg.com/layout-base@2/layout-base.js".into(),
///     cose_base_path: "https://unpkg.com/cose-base@2/cose-base.js".into(),
///     cytoscape_cose_bilkent_path: "https://unpkg.com/cytoscape-cose-bilkent@4/cytoscape-cose-bilkent.js".into(),
///     ..Default::default() // wasm_base64, wasm_js_glue = None
/// };
///
/// // Or use defaults (empty paths - graph shows placeholder)
/// let assets = JsAssets::default();
/// ```
#[derive(Clone, Default, Debug)]
pub struct JsAssets {
    /// Path to cytoscape.min.js
    pub cytoscape_path: String,
    /// Path to dagre.min.js (for hierarchical layouts)
    pub dagre_path: String,
    /// Path to cytoscape-dagre.js plugin
    pub cytoscape_dagre_path: String,
    /// Path to layout-base.js (required by cose-base)
    pub layout_base_path: String,
    /// Path to cose-base.js (required by cytoscape-cose-bilkent)
    pub cose_base_path: String,
    /// Path to cytoscape-cose-bilkent.js plugin (for force-directed layouts)
    pub cytoscape_cose_bilkent_path: String,
    /// Inline WASM module (base64 encoded) for native graph rendering
    pub wasm_base64: Option<String>,
    /// Inline JS glue code for WASM module
    pub wasm_js_glue: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_empty_report() {
        let sections: Vec<ReportSection> = vec![];
        let assets = JsAssets::default();
        let html = render_report(&sections, &assets, false);

        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<html"));
        assert!(html.contains("loctree"));
    }

    #[test]
    fn renders_section_with_data() {
        let section = ReportSection {
            root: "test-root".into(),
            files_analyzed: 42,
            ..Default::default()
        };
        let assets = JsAssets::default();
        let html = render_report(&[section], &assets, false);

        assert!(html.contains("test-root"));
        assert!(html.contains("42"));
    }

    #[test]
    fn renders_sidebar_version_once() {
        let html = render_report(&[], &JsAssets::default(), false);
        let version = env!("CARGO_PKG_VERSION");

        assert!(html.contains(&format!("loctree v{version}")));
        assert!(!html.contains(&format!("loctree v{version}{version}")));
    }

    #[test]
    fn graph_data_to_dot_format() {
        use types::{GraphData, GraphNode};

        let graph = GraphData {
            nodes: vec![
                GraphNode {
                    id: "src/main.ts".into(),
                    label: "main.ts".into(),
                    loc: 150,
                    x: 0.5,
                    y: 0.5,
                    component: 0,
                    degree: 2,
                    detached: false,
                },
                GraphNode {
                    id: "src/utils.ts".into(),
                    label: "utils.ts".into(),
                    loc: 50,
                    x: 0.3,
                    y: 0.7,
                    component: 0,
                    degree: 1,
                    detached: false,
                },
            ],
            edges: vec![("src/main.ts".into(), "src/utils.ts".into(), "import".into())],
            components: vec![],
            main_component_id: 0,
            ..Default::default()
        };

        let dot = graph.to_dot();

        // Verify DOT structure
        assert!(dot.starts_with("digraph loctree"));
        assert!(dot.contains("src/main.ts"));
        assert!(dot.contains("src/utils.ts"));
        assert!(dot.contains("->"));
        assert!(dot.contains("fillcolor"));
    }

    #[test]
    fn graph_data_to_dot_escapes_special_chars() {
        use types::{GraphData, GraphNode};

        let graph = GraphData {
            nodes: vec![GraphNode {
                id: "src/file\"with\"quotes.ts".into(),
                label: "file\"quotes".into(),
                loc: 10,
                x: 0.0,
                y: 0.0,
                component: 0,
                degree: 0,
                detached: false,
            }],
            edges: vec![],
            components: vec![],
            main_component_id: 0,
            ..Default::default()
        };

        let dot = graph.to_dot();

        // Quotes should be escaped
        assert!(dot.contains("\\\""));
        // Raw unescaped quote should not appear in node definitions
        assert!(!dot.contains("file\"with\"quotes"));
    }

    #[test]
    fn renders_action_plan_panel() {
        use types::PriorityTask;

        let section = ReportSection {
            root: "test-root".into(),
            files_analyzed: 1,
            priority_tasks: vec![PriorityTask {
                priority: 1,
                kind: "dead_export".into(),
                target: "GhostFunc".into(),
                location: "src/ghost.rs:42".into(),
                why: "Exported but unused".into(),
                risk: "high".into(),
                fix_hint: "Remove unused export".into(),
                verify_cmd: "loct dead --confidence high".into(),
            }],
            ..Default::default()
        };
        let assets = JsAssets::default();
        let html = render_report(&[section], &assets, false);

        assert!(html.contains("Action Plan"));
        assert!(html.contains("GhostFunc"));
        assert!(html.contains("src/ghost.rs:42"));
    }

    #[test]
    fn renders_hub_files_panel() {
        use types::HubFile;

        let section = ReportSection {
            root: "test-root".into(),
            files_analyzed: 1,
            hub_files: vec![HubFile {
                path: "src/lib.rs".into(),
                loc: 120,
                imports_count: 3,
                exports_count: 7,
                importers_count: 5,
                commands_count: 1,
                slice_cmd: "loct slice src/lib.rs".into(),
            }],
            ..Default::default()
        };
        let assets = JsAssets::default();
        let html = render_report(&[section], &assets, false);

        assert!(html.contains("Context Anchors"));
        assert!(html.contains("src/lib.rs"));
        assert!(html.contains("loct slice src/lib.rs"));
    }

    #[test]
    fn renders_hotspots_panel() {
        use types::HotspotFile;

        let section = ReportSection {
            root: "test-root".into(),
            files_analyzed: 1,
            hotspots: vec![HotspotFile {
                file: "src/shared/state.rs".into(),
                importers: 17,
                category: "CORE".into(),
                slice_cmd: "loct slice src/shared/state.rs".into(),
            }],
            ..Default::default()
        };
        let assets = JsAssets::default();
        let html = render_report(&[section], &assets, false);

        assert!(html.contains("Hotspots"));
        assert!(html.contains("Import Hotspots"));
        assert!(html.contains("src/shared/state.rs"));
        assert!(html.contains("17"));
        assert!(html.contains("loct slice src/shared/state.rs"));
    }

    // ────────────────────────────────────────────────────────────────────
    // Editorial styling discipline (plan 24) — assert the polished surface.
    // These tests guard the loctree-com /cloud styling alignment so future
    // refactors do not silently regress the public artifact's identity.
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn renders_editorial_identity_badge() {
        let section = ReportSection {
            root: "demo-project".into(),
            files_analyzed: 1,
            ..Default::default()
        };
        let assets = JsAssets::default();
        let html = render_report(&[section], &assets, false);

        assert!(
            html.contains("Generated Loctree Report"),
            "identity badge should communicate provenance, not 'buy now'"
        );
        assert!(html.contains("report-identity-badge"));
        assert!(
            !html.contains("Checkout"),
            "no SaaS checkout copy in artifact"
        );
        assert!(
            !html.contains("Buy now"),
            "no SaaS purchase CTA in artifact"
        );
    }

    #[test]
    fn renders_editorial_section_hero_pattern() {
        let section = ReportSection {
            root: "demo-project".into(),
            files_analyzed: 1,
            ..Default::default()
        };
        let assets = JsAssets::default();
        let html = render_report(&[section], &assets, false);

        // eyebrow / display title / meta-row hierarchy
        assert!(html.contains("report-eyebrow"));
        assert!(html.contains("report-section-title"));
        assert!(html.contains("report-sticky-hero"));
    }

    #[test]
    fn renders_evidence_footer_with_provenance() {
        let section = ReportSection {
            root: "/very/deep/nested/path/to/some-project".into(),
            files_analyzed: 7,
            git_branch: Some("main".into()),
            git_commit: Some("abc1234".into()),
            generated_at: Some("2026-05-15T05:10:00Z".into()),
            schema_name: Some("loctree".into()),
            schema_version: Some("0.10.2".into()),
            ..Default::default()
        };
        let assets = JsAssets::default();
        let html = render_report(&[section], &assets, false);

        assert!(html.contains("report-evidence-footer"));
        assert!(html.contains("Generated Loctree Report — provenance"));
        assert!(html.contains("Renderer"));
        assert!(html.contains("Source project"));
        assert!(html.contains("Reproduce this artifact"));
        assert!(html.contains("loct report --output report.html"));
        assert!(html.contains(env!("CARGO_PKG_VERSION")));
        assert!(html.contains("main@abc1234"));
        assert!(html.contains("loctree@0.10.2"));
        // Cross-link to /cloud is editorial fineprint, not a CTA
        assert!(html.contains("loct.io/cloud"));
    }

    #[test]
    fn evidence_footer_handles_missing_provenance_gracefully() {
        let section = ReportSection {
            root: String::new(),
            files_analyzed: 0,
            ..Default::default()
        };
        let assets = JsAssets::default();
        let html = render_report(&[section], &assets, false);

        // Footer renders with safe defaults
        assert!(html.contains("report-evidence-footer"));
        assert!(html.contains("(unspecified)"));
        // Repro command still present without cwd suffix
        assert!(html.contains("loct report --output report.html"));
        // No stray empty git/schema chips
        assert!(!html.contains("Generated at</span>"));
    }

    #[test]
    fn editorial_token_layer_present_in_styles() {
        // Style sheet ships the warm editorial token primitives that mirror
        // loctree-com/styles/tokens.css. If these names disappear the
        // discipline is broken.
        assert!(styles::REPORT_CSS.contains("--report-bone"));
        assert!(styles::REPORT_CSS.contains("--report-amber"));
        assert!(styles::REPORT_CSS.contains("--report-teal"));
        assert!(styles::REPORT_CSS.contains("--report-status-success"));
        assert!(styles::REPORT_CSS.contains("--report-status-warning"));
        assert!(styles::REPORT_CSS.contains("--report-status-danger"));
        assert!(styles::REPORT_CSS.contains(".report-eyebrow"));
        assert!(styles::REPORT_CSS.contains(".report-section-title"));
        assert!(styles::REPORT_CSS.contains(".report-evidence-footer"));
        assert!(styles::REPORT_CSS.contains(".report-identity-badge"));
        assert!(styles::REPORT_CSS.contains(".report-fallback-empty"));
    }

    #[test]
    fn no_loctree_com_secrets_in_artifact() {
        // Belt-and-suspenders: even though we never thread loctree-com data
        // into the renderer, sweep the rendered HTML for forbidden surfaces.
        let section = ReportSection {
            root: "demo-project".into(),
            files_analyzed: 1,
            ..Default::default()
        };
        let assets = JsAssets::default();
        let html = render_report(&[section], &assets, false);

        // Polar product IDs (UUID-like) and stripe-style identifiers
        assert!(!html.contains("polar_"), "no Polar product IDs allowed");
        assert!(
            !html.contains("price_"),
            "no Polar/Stripe price IDs allowed"
        );
        assert!(
            !html.contains("/api/checkout"),
            "no SaaS checkout endpoints"
        );
        assert!(
            !html.contains("Add Cloud Sync"),
            "no SaaS purchase CTAs in static artifact"
        );
    }

    #[test]
    fn renders_long_paths_without_collapsing_layout() {
        let long = "/home/example/very/very/very/long/path/to/some/deeply/nested/source/project/with/a/lot/of/components".to_string();
        let section = ReportSection {
            root: long.clone(),
            files_analyzed: 1,
            ..Default::default()
        };
        let assets = JsAssets::default();
        let html = render_report(&[section], &assets, false);

        // Long path is rendered, but with the wrap class so it cannot blow
        // out the sticky header or evidence footer.
        assert!(html.contains(&long));
        assert!(html.contains("report-path-wrap"));
    }

    #[test]
    fn renders_dist_panel_when_present() {
        use types::{DistAnalysisLevel, DistDeadExport, DistFileImpact, DistReport};

        let section = ReportSection {
            root: "test-root".into(),
            files_analyzed: 1,
            dist: Some(DistReport {
                src_dir: "src".into(),
                source_map_paths: vec!["dist/app.js.map".into()],
                source_maps: 1,
                source_exports: 8,
                bundled_exports: 5,
                dead_exports: vec![DistDeadExport {
                    file: "src/unused.ts".into(),
                    line: 12,
                    name: "UnusedThing".into(),
                    kind: "function".into(),
                }],
                reduction: "38%".into(),
                symbol_level: true,
                analysis_level: DistAnalysisLevel::Symbol,
                tree_shaken_exports: 3,
                tree_shaken_pct: 38,
                coverage_pct: 63,
                impacted_files: vec![DistFileImpact {
                    file: "src/unused.ts".into(),
                    source_exports: 3,
                    bundled_exports: 0,
                    tree_shaken_exports: 3,
                    status: "fully-shaken".into(),
                }],
                candidate_counts: Default::default(),
                candidates: Vec::new(),
            }),
            ..Default::default()
        };
        let assets = JsAssets::default();
        let html = render_report(&[section], &assets, false);

        assert!(html.contains("Bundles"));
        assert!(html.contains("Bundle distribution"));
        assert!(html.contains("UnusedThing"));
        assert!(html.contains("63%"));
    }
}
