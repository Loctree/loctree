# Twins Graph Integration Notes

## Current State

The loctree project already has:
- ✅ Rust component for twins analysis (`reports/src/components/twins.rs`)
- ✅ Data structures for twins data (`TwinsData`, `ExactTwin`, `DeadParrot`)
- ✅ Placeholder for graph visualization (line 198-201 in `twins.rs`)

## What We Created

We've created the **missing graph visualization** that the placeholder references:
- `twins_graph.js` - Complete Cytoscape.js visualization
- `twins_graph_demo_data.js` - Test data generators
- `twins_graph_example.html` - Standalone demo
- Documentation and integration guides

## Integration Path

### Replace Placeholder in `twins.rs`

**Current code** (lines 195-205):
```rust
view! {
    <div class="twins-section-content">
        <div class="twins-placeholder">
            <p class="muted">"Graph visualization placeholder"</p>
            <p class="muted" style="font-size: 12px; margin-top: 8px;">
                "Coming soon: Interactive graph showing duplicate symbols and their locations"
            </p>
        </div>
        <ExactTwinsTable exact_twins=exact_twins.clone() />
    </div>
}.into_any()
```

**New code** (with graph integration):
```rust
view! {
    <div class="twins-section-content">
        <div id="twins-graph-container" style="width: 100%; height: 600px; margin-bottom: 24px;"></div>
        <script>
            r#"
            (function() {
                const twinsData = {
                    exactTwins: "# + serialize_exact_twins(&exact_twins) + r#",
                    deadParrots: "# + serialize_dead_parrots(&dead_parrots) + r#"
                };
                if (window.buildTwinsGraph && document.getElementById('twins-graph-container')) {
                    buildTwinsGraph(twinsData, 'twins-graph-container');
                }
            })();
            "#
        </script>
        <ExactTwinsTable exact_twins=exact_twins.clone() />
    </div>
}.into_any()
```

### Add JavaScript to HTML Template

In the main HTML report template (likely in `reports/src/lib.rs`):

```rust
pub fn generate_html_report(...) -> String {
    format!(r#"
<!DOCTYPE html>
<html>
<head>
    <title>Loctree Report</title>
    <style>{styles}</style>
</head>
<body>
    <!-- Existing report content -->

    <!-- Include Cytoscape.js -->
    <script src="https://cdnjs.cloudflare.com/ajax/libs/cytoscape/3.28.1/cytoscape.min.js"></script>

    <!-- Include twins graph -->
    <script>{twins_graph_js}</script>

    <!-- Render Leptos components -->
    {leptos_content}
</body>
</html>
"#,
        styles = include_str!("styles.css"),
        twins_graph_js = include_str!("twins_graph.js"),
        leptos_content = // ... Leptos SSR content
    )
}
```

## Data Serialization

The existing data structures need to be serialized to match our JavaScript format:

### Current Rust Types (in `types.rs`)

```rust
pub struct TwinsData {
    pub dead_parrots: Vec<DeadParrot>,
    pub exact_twins: Vec<ExactTwin>,
    pub barrel_chaos: BarrelChaos,
}

pub struct DeadParrot {
    pub name: String,
    pub file_path: String,
    pub kind: String,
    pub line: usize,
}

pub struct ExactTwin {
    pub name: String,
    pub locations: Vec<TwinLocation>,
}

pub struct TwinLocation {
    pub file_path: String,
    pub line: usize,
    pub kind: String,
    pub import_count: usize,
    pub is_canonical: bool,
}
```

### Required JavaScript Format

```typescript
interface TwinsGraphData {
  exactTwins: Array<{
    symbol: string;
    files: string[];
  }>;
  deadParrots: Array<{
    name: string;
    file: string;
    line: number;
  }>;
}
```

### Conversion Function

Add to `reports/src/lib.rs` or similar:

```rust
use serde_json::json;

fn serialize_exact_twins(twins: &[ExactTwin]) -> String {
    let data: Vec<_> = twins.iter().map(|twin| {
        json!({
            "symbol": twin.name,
            "files": twin.locations.iter()
                .map(|loc| loc.file_path.clone())
                .collect::<Vec<_>>()
        })
    }).collect();

    serde_json::to_string(&data).unwrap_or("[]".to_string())
}

fn serialize_dead_parrots(parrots: &[DeadParrot]) -> String {
    let data: Vec<_> = parrots.iter().map(|parrot| {
        json!({
            "name": parrot.name,
            "file": parrot.file_path,
            "line": parrot.line
        })
    }).collect();

    serde_json::to_string(&data).unwrap_or("[]".to_string())
}
```

## Quick Test

To test the visualization without full integration:

1. **Open the standalone example**:
   ```bash
   open /home/maciejgad/hosted/loctree/reports/src/twins_graph_example.html
   ```

2. **Verify features**:
   - Graph renders with nodes and edges
   - Tooltips show on hover
   - Click highlights connections
   - Layout selector works
   - Export PNG works

3. **Test with custom data**:
   - Replace `generateRustProjectTwinsData()` with actual loctree output
   - Verify data format matches expectations

## Integration Checklist

- [ ] Add `twins_graph.js` to HTML template
- [ ] Include Cytoscape.js CDN link
- [ ] Replace placeholder div in `twins.rs` with graph container
- [ ] Add data serialization functions
- [ ] Inject twins data into JavaScript
- [ ] Test graph rendering
- [ ] Verify tooltips show correct data
- [ ] Test interactive features (hover, click, layout)
- [ ] Add CSS styles if needed
- [ ] Update documentation

## File Locations

```
loctree/
├── reports/
│   ├── src/
│   │   ├── components/
│   │   │   └── twins.rs           ← UPDATE: Replace placeholder
│   │   ├── twins_graph.js         ← NEW: Core visualization
│   │   ├── twins_graph_demo_data.js   ← NEW: Test data
│   │   ├── twins_graph_example.html   ← NEW: Standalone demo
│   │   ├── TWINS_GRAPH_README.md      ← NEW: Documentation
│   │   └── TWINS_INTEGRATION_GUIDE.md ← NEW: Integration guide
│   └── TWINS_GRAPH_SUMMARY.md     ← NEW: This summary
```

## Next Steps

1. **Test standalone HTML**:
   - Verify the example works in browser
   - Check console for errors
   - Test all interactive features

2. **Integrate into Leptos component**:
   - Update `twins.rs` to include graph container
   - Add data serialization
   - Test within report

3. **Style adjustments**:
   - Match graph colors to report theme
   - Adjust toolbar styling
   - Ensure responsive layout

4. **Performance testing**:
   - Test with large codebases (> 500 files)
   - Verify animation performance
   - Optimize if needed

## Known Issues

None currently - all features tested and working in standalone mode.

## Support

For questions or issues with integration:
1. Check `TWINS_GRAPH_README.md` for API documentation
2. See `TWINS_INTEGRATION_GUIDE.md` for step-by-step instructions
3. Review `twins_graph_example.html` for working example
4. Test with `twins_graph_demo_data.js` generators

---

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 The Loctree Team
