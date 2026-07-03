//! Leptos UI components for rendering HTML reports.
//!
//! This module contains modular, reusable components for building
//! static HTML reports. Each component is a Leptos `#[component]`
//! function that can be composed to create custom report layouts.
//!
//! # Component Hierarchy
//!
//! ```text
//! ReportDocument
//! ├── Sidebar (navigation via nav-items)
//! └── ReportSectionView (per analyzed directory)
//!     ├── Header (path + stats badges)
//!     ├── TabContent: Overview
//!     │   ├── AnalysisSummary
//!     │   └── AiInsightsPanel
//!     ├── TabContent: Duplicates
//!     │   ├── DuplicateExportsTable
//!     │   └── CascadesList
//!     ├── TabContent: Dynamic Imports
//!     │   └── DynamicImportsTable
//!     ├── TabContent: Commands (Tauri)
//!     │   └── TauriCommandCoverage
//!     └── TabContent: Graph
//!         └── GraphContainer
//! ```
//!
//! # Usage
//!
//! Components are typically used via [`crate::render_report`], but
//! can be used directly for custom layouts:
//!
//! ```rust,ignore
//! use leptos::prelude::*;
//! use report_leptos::components::{TabContent, AiInsightsPanel};
//!
//! view! {
//!     <TabContent root_id="my-project" tab_name="overview" active=true>
//!         <AiInsightsPanel insights=my_insights />
//!     </TabContent>
//! }
//! ```

mod action_plan;
mod atlas_view;
mod audit;
mod cascades;
mod commands;
mod context_atlas;
mod coverage;
mod crowds;
mod cycles;
mod dead_code;
mod dist;
mod document;
mod duplicates;
mod dynamic_imports;
mod for_ai;
mod graph;
mod health_gauge;
mod hotspots;
mod hub_files;
pub mod icons;
mod insights;
mod pipelines;
mod quick_commands;
mod refactor_plan;
mod section;
mod tabs;
mod tools_view;
mod tree;
mod twins;

pub use action_plan::ActionPlanPanel;
pub use atlas_view::AtlasView;
pub use audit::AuditPanel;
pub use cascades::CascadesList;
pub use commands::TauriCommandCoverage;
pub use context_atlas::ContextAtlasPanel;
pub use coverage::Coverage;
pub use crowds::Crowds;
pub use cycles::Cycles;
pub use dead_code::DeadCode;
pub use dist::DistPanel;
pub use document::ReportDocument;
pub use duplicates::DuplicateExportsTable;
pub use dynamic_imports::DynamicImportsTable;
pub use for_ai::AiSummaryPanel;
pub use graph::GraphContainer;
pub use health_gauge::{HealthIndicator, HealthScoreGauge};
pub use hotspots::Hotspots;
pub use hub_files::HubFilesPanel;
pub use icons::*;
pub use insights::{AiInsightsPanel, AnalysisSummary};
pub use pipelines::Pipelines;
pub use quick_commands::QuickCommandsPanel;
pub use refactor_plan::RefactorPlan;
pub use section::ReportSectionView;
pub use tabs::TabContent;
pub use tools_view::ToolsView;
pub use tree::TreeView;
pub use twins::Twins;
