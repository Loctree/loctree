//! WASM module for graph rendering in loctree reports.
//!
//! This module provides browser-native graph visualization using Rust/WASM.
//! It accepts graph data as JSON, converts to DOT format, and renders SVG.
//!
//! Uses canonical types from `report-leptos::types` and provides WASM-specific
//! rendering with themed color support.
//!
//! Developed with 💀 by The Loctree Team ⓒ 2025-2026

use wasm_bindgen::prelude::*;

// Re-export canonical types from report-leptos
pub use report_leptos::types::{GraphComponent, GraphData, GraphNode};

/// Initialize panic hook for better error messages in browser console.
#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

// Note: GraphData::to_dot() and GraphData::to_dot_dark() are provided by report-leptos::types
// No need to re-implement here - we use the canonical implementation

// ============================================================================
// WASM Exports
// ============================================================================

/// Parse JSON graph data and convert to DOT format.
///
/// # Arguments
/// * `json_data` - JSON string containing GraphData
/// * `dark_mode` - Whether to use dark theme colors
///
/// # Returns
/// DOT format string or error message
#[wasm_bindgen]
pub fn graph_to_dot(json_data: &str, dark_mode: bool) -> Result<String, JsValue> {
    let graph: GraphData = serde_json::from_str(json_data)
        .map_err(|e| JsValue::from_str(&format!("Failed to parse graph data: {}", e)))?;

    let dot = if dark_mode {
        graph.to_dot_dark()
    } else {
        graph.to_dot()
    };

    Ok(dot)
}

/// Render graph as SVG.
///
/// Currently returns a placeholder SVG. Full implementation will use
/// graphviz-wasm or dot_ix for actual rendering.
///
/// # Arguments
/// * `json_data` - JSON string containing GraphData
/// * `dark_mode` - Whether to use dark theme colors
///
/// # Returns
/// SVG string or error message
#[wasm_bindgen]
pub fn render_graph_svg(json_data: &str, dark_mode: bool) -> Result<String, JsValue> {
    let graph: GraphData = serde_json::from_str(json_data)
        .map_err(|e| JsValue::from_str(&format!("Failed to parse graph data: {}", e)))?;

    // For now, generate a simple placeholder SVG
    // TODO: Integrate graphviz-wasm or dot_ix for real rendering
    let node_count = graph.nodes.len();
    let edge_count = graph.edges.len();

    let bg_color = if dark_mode { "#1f2937" } else { "#ffffff" };
    let text_color = if dark_mode { "#e5e7eb" } else { "#1f2937" };
    let accent_color = if dark_mode { "#3b82f6" } else { "#2563eb" };

    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 400 200">
  <rect width="100%" height="100%" fill="{}"/>
  <text x="200" y="80" text-anchor="middle" font-family="sans-serif" font-size="16" fill="{}">
    Graph: {} nodes, {} edges
  </text>
  <text x="200" y="110" text-anchor="middle" font-family="sans-serif" font-size="12" fill="{}">
    WASM renderer placeholder
  </text>
  <text x="200" y="140" text-anchor="middle" font-family="sans-serif" font-size="10" fill="{}">
    DOT output ready, SVG rendering coming soon
  </text>
</svg>"#,
        bg_color, text_color, node_count, edge_count, accent_color, accent_color
    );

    Ok(svg)
}

/// Get DOT string for debugging/export purposes.
#[wasm_bindgen]
pub fn get_dot_string(json_data: &str, dark_mode: bool) -> Result<String, JsValue> {
    graph_to_dot(json_data, dark_mode)
}

/// Check if WASM module is loaded and functional.
#[wasm_bindgen]
pub fn health_check() -> String {
    "report-wasm v0.1.0 ready".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_to_dot() {
        let graph = GraphData {
            nodes: vec![GraphNode {
                id: "src/main.ts".into(),
                label: "main.ts".into(),
                loc: 100,
                x: 0.5,
                y: 0.5,
                component: 0,
                degree: 1,
                detached: false,
            }],
            edges: vec![],
            components: vec![],
            main_component_id: 0,
            ..Default::default()
        };

        let dot = graph.to_dot();
        assert!(dot.contains("digraph loctree"));
        assert!(dot.contains("src/main.ts"));
        assert!(dot.contains("100 LOC"));
    }

    #[test]
    fn test_graph_escapes_special_chars() {
        // Test that the canonical to_dot() properly escapes quotes
        let graph = GraphData {
            nodes: vec![GraphNode {
                id: "file\"with\"quotes.ts".into(),
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
        // Quotes should be escaped in the output
        assert!(dot.contains("\\\""));
    }

    #[test]
    fn test_dark_mode_colors() {
        let graph = GraphData {
            nodes: vec![GraphNode {
                id: "test".into(),
                label: "test".into(),
                loc: 50,
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

        let light = graph.to_dot();
        let dark = graph.to_dot_dark();

        // Light theme has standard colors, dark theme has dark background
        assert!(dark.contains("bgcolor=\"#0f1115\""));
        // Light version shouldn't have bgcolor
        assert!(!light.contains("bgcolor"));
    }
}
