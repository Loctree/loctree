# report-leptos

Leptos SSR renderer for generating static HTML reports from code analysis data.

[![Crates.io](https://img.shields.io/crates/v/report-leptos.svg)](https://crates.io/crates/report-leptos)
[![Documentation](https://docs.rs/report-leptos/badge.svg)](https://docs.rs/report-leptos)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

## Overview

`report-leptos` is a standalone library that generates beautiful, interactive HTML reports using [Leptos](https://leptos.dev/) server-side rendering. Originally built for [loctree](https://github.com/Loctree/Loctree) codebase analysis, it can be used independently for any static report generation needs.

### Key Features

- **Zero JavaScript Runtime** - Pure SSR, no hydration needed
- **Component-Based Architecture** - Modular, reusable UI components
- **Interactive Graph Visualization** - Cytoscape.js integration for dependency graphs
- **Responsive Design** - Works on desktop and mobile
- **Dark Mode Ready** - Built-in theme support
- **Type-Safe** - Full Rust type safety from data to HTML

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
report-leptos = "0.1"
```

## Quick Start

```rust
use report_leptos::{render_report, JsAssets, types::ReportSection};

fn main() {
    // Create report data
    let section = ReportSection {
        root: "my-project".into(),
        files_analyzed: 42,
        ..Default::default()
    };

    // Configure JS assets (for graph visualization)
    let js_assets = JsAssets {
        cytoscape_path: "cytoscape.min.js".into(),
        dagre_path: "dagre.min.js".into(),
        cytoscape_dagre_path: "cytoscape-dagre.js".into(),
        cytoscape_cose_bilkent_path: "cytoscape-cose-bilkent.js".into(),
    };

    // Render to HTML string
    let html = render_report(&[section], &js_assets);

    // Write to file
    std::fs::write("report.html", html).unwrap();
}
```

## Architecture

```
report-leptos/
├── src/
│   ├── lib.rs           # Public API: render_report(), JsAssets
│   ├── types.rs         # Data structures (ReportSection, GraphData, etc.)
│   ├── styles.rs        # CSS constants
│   └── components/      # Leptos UI components
│       ├── mod.rs       # Component exports
│       ├── document.rs  # Root HTML document wrapper
│       ├── section.rs   # Report section container
│       ├── tabs.rs      # Tab navigation UI
│       ├── insights.rs  # AI insights panel
│       ├── duplicates.rs # Duplicate exports table
│       ├── cascades.rs  # Cascade dependencies list
│       ├── dynamic_imports.rs # Dynamic imports table
│       ├── commands.rs  # Tauri command coverage
│       └── graph.rs     # Cytoscape graph container
```

## Components

### ReportDocument

Root component that wraps the entire HTML document:

```rust
use report_leptos::components::ReportDocument;

view! {
    <ReportDocument sections=sections js_assets=assets />
}
```

### ReportSectionView

Displays a single analyzed directory with tabbed content:

```rust
use report_leptos::components::ReportSectionView;

view! {
    <ReportSectionView section=section idx=0 js_assets=assets />
}
```

### TabBar & TabContent

Reusable tab navigation:

```rust
use report_leptos::components::{TabBar, TabContent};

view! {
    <TabBar section_idx=0 tabs=vec!["Overview", "Details"] />
    <TabContent section_idx=0 tab_idx=0 active=true>
        // Content here
    </TabContent>
}
```

## Data Types

### ReportSection

Main container for analysis results:

```rust
pub struct ReportSection {
    pub root: String,              // Analyzed directory
    pub files_analyzed: usize,     // File count
    pub ranked_dups: Vec<RankedDup>,  // Duplicate exports
    pub cascades: Vec<(String, String)>,  // Cascade imports
    pub circular_imports: Vec<Vec<String>>, // Dependency cycles
    pub dynamic: Vec<(String, Vec<String>)>,  // Dynamic imports
    pub missing_handlers: Vec<CommandGap>,  // Tauri gaps
    pub graph: Option<GraphData>,  // Dependency graph
    pub insights: Vec<AiInsight>,  // AI suggestions
    // ... more fields
}
```

### GraphData

For interactive dependency visualization:

```rust
pub struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<(String, String, String)>,  // from, to, kind
    pub components: Vec<GraphComponent>,
    pub main_component_id: usize,
}
```

### AiInsight

AI-generated code quality hints:

```rust
pub struct AiInsight {
    pub title: String,
    pub severity: String,  // "high", "medium", "low"
    pub message: String,
}
```

## Styling

CSS is embedded in the generated HTML. To customize:

```rust
use report_leptos::styles::REPORT_CSS;

// Extend or override styles
let custom_css = format!("{}\n{}", REPORT_CSS, my_custom_css);
```

## Graph Visualization

The library integrates with Cytoscape.js for interactive graphs. You need to provide the JS asset paths:

```rust
let js_assets = JsAssets {
    cytoscape_path: "https://unpkg.com/cytoscape@3/dist/cytoscape.min.js".into(),
    dagre_path: "https://unpkg.com/dagre@0.8/dist/dagre.min.js".into(),
    cytoscape_dagre_path: "https://unpkg.com/cytoscape-dagre@2/cytoscape-dagre.js".into(),
    cytoscape_cose_bilkent_path: "https://unpkg.com/cytoscape-cose-bilkent@4/cytoscape-cose-bilkent.js".into(),
};
```

Or bundle them locally for offline use.

## Integration with loctree

This library is the rendering engine for [loctree](https://github.com/Loctree/Loctree) HTML reports. When using loctree with `--html-report`:

```bash
loctree src --html-report analysis.html
```

The report is generated using this library's SSR components.

## Leptos 0.8 SSR

This library uses Leptos 0.8's `RenderHtml` trait for server-side rendering:

```rust
use leptos::prelude::*;
use leptos::tachys::view::RenderHtml;

let view = view! { <MyComponent /> };
let html: String = view.to_html();
```

No reactive runtime or hydration is needed - pure static HTML generation.

## Performance

- **Fast** - Leptos SSR is compiled Rust, not interpreted templates
- **Small** - No JavaScript framework overhead in output
- **Streaming** - Large reports render progressively (when using async)

## Examples

See the `examples/` directory:

```bash
cargo run --example basic_report
cargo run --example with_graph
cargo run --example custom_styles
```

## Contributing

Contributions welcome! Please read [CONTRIBUTING.md](../CONTRIBUTING.md) first.

## License

MIT License - see [LICENSE](LICENSE) for details.

---

Developed with 💀 by The Loctree Team ⓒ 2025-2026 
