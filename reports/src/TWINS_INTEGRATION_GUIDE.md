# Twins Graph Integration Guide

Step-by-step guide for integrating the Twin DNA Graph into loctree's HTML report generator.

## Integration Steps

### 1. Include the Script in Reports

Add `twins_graph.js` to the HTML report template (likely in `reports/src/lib.rs` or similar):

```rust
// In your report generator
pub fn generate_html_report(analysis: &Analysis) -> String {
    format!(r#"
<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Loctree Analysis Report</title>
    <style>{}</style>
</head>
<body>
    <!-- Existing report sections -->

    <!-- NEW: Twins Graph Section -->
    <section id="twins-section">
        <h2>Twin DNA Graph</h2>
        <div id="twins-graph-container" style="width: 100%; height: 800px; background: #0f1115; border-radius: 8px;"></div>
    </section>

    <script src="https://cdnjs.cloudflare.com/ajax/libs/cytoscape/3.28.1/cytoscape.min.js"></script>
    <script>{}</script>
    <script>
        // Initialize twins graph with data from analysis
        const twinsData = {{}};
        buildTwinsGraph(twinsData, 'twins-graph-container', 'loctree://');
    </script>
</body>
</html>
"#,
        include_str!("styles.css"),
        include_str!("twins_graph.js"),
        generate_twins_data_json(analysis)
    )
}
```

### 2. Generate Twins Data JSON

Create a function to convert the twins analysis to JSON:

```rust
// In reports/src/lib.rs or types.rs

use serde::Serialize;

#[derive(Serialize)]
struct TwinsGraphData {
    #[serde(rename = "exactTwins")]
    exact_twins: Vec<ExactTwin>,
    #[serde(rename = "deadParrots")]
    dead_parrots: Vec<DeadParrot>,
}

#[derive(Serialize)]
struct ExactTwin {
    symbol: String,
    files: Vec<String>,
}

#[derive(Serialize)]
struct DeadParrot {
    name: String,
    file: String,
    line: usize,
}

fn generate_twins_data_json(analysis: &Analysis) -> String {
    let mut exact_twins_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut dead_parrots = Vec::new();

    // Process the analysis results
    for (symbol_name, locations) in &analysis.symbol_exports {
        if locations.len() > 1 {
            // This is a twin - same symbol in multiple files
            let files: Vec<String> = locations.iter()
                .map(|loc| loc.file.clone())
                .collect();

            exact_twins_map.insert(symbol_name.clone(), files);

            // Add dead parrots for each occurrence
            for loc in locations {
                dead_parrots.push(DeadParrot {
                    name: symbol_name.clone(),
                    file: loc.file.clone(),
                    line: loc.line,
                });
            }
        }
    }

    let exact_twins: Vec<ExactTwin> = exact_twins_map
        .into_iter()
        .map(|(symbol, files)| ExactTwin { symbol, files })
        .collect();

    let data = TwinsGraphData {
        exact_twins,
        dead_parrots,
    };

    serde_json::to_string(&data).unwrap_or_else(|_| "{}".to_string())
}
```

### 3. Extract Twins Data from Analyzer

Modify the analyzer to track symbol exports:

```rust
// In loctree-rs/src/analyzer/rust.rs or similar

pub struct SymbolLocation {
    pub file: String,
    pub line: usize,
    pub kind: SymbolKind, // function, struct, enum, etc.
}

pub struct Analysis {
    // ... existing fields ...

    /// Map of symbol name -> locations where it's exported
    pub symbol_exports: HashMap<String, Vec<SymbolLocation>>,
}

// When analyzing a file:
impl RustAnalyzer {
    fn analyze_exports(&mut self, file_path: &str, syntax: &SyntaxNode) {
        // Find all public items
        for node in syntax.descendants() {
            match node.kind() {
                SyntaxKind::FN if has_pub_visibility(&node) => {
                    if let Some(name) = get_function_name(&node) {
                        let line = get_line_number(&node);
                        self.analysis.symbol_exports
                            .entry(name.clone())
                            .or_default()
                            .push(SymbolLocation {
                                file: file_path.to_string(),
                                line,
                                kind: SymbolKind::Function,
                            });
                    }
                }
                SyntaxKind::STRUCT if has_pub_visibility(&node) => {
                    // Similar for structs
                }
                SyntaxKind::ENUM if has_pub_visibility(&node) => {
                    // Similar for enums
                }
                // ... other item types
                _ => {}
            }
        }
    }
}
```

### 4. Add CSS Styles

Include these styles in your `styles.rs` or CSS file:

```css
/* Twins Graph Section */
#twins-section {
    margin: 40px 0;
    padding: 20px;
    background: linear-gradient(135deg, #0f1115 0%, #1a1a2e 100%);
    border-radius: 12px;
}

#twins-section h2 {
    text-align: center;
    margin-bottom: 20px;
    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    background-clip: text;
}

#twins-graph-container {
    position: relative;
    border: 2px solid #333;
}

/* Twins graph tooltips are styled inline in twins_graph.js */
```

### 5. Testing

Create a test file to verify the integration:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_twins_data_generation() {
        let mut analysis = Analysis::default();

        // Add some test data
        analysis.symbol_exports.insert("parse".to_string(), vec![
            SymbolLocation { file: "src/a.rs".into(), line: 10, kind: SymbolKind::Function },
            SymbolLocation { file: "src/b.rs".into(), line: 20, kind: SymbolKind::Function },
        ]);

        let json = generate_twins_data_json(&analysis);

        // Verify JSON structure
        let data: TwinsGraphData = serde_json::from_str(&json).unwrap();
        assert_eq!(data.exact_twins.len(), 1);
        assert_eq!(data.dead_parrots.len(), 2);
    }
}
```

## File Structure

After integration, your project should look like:

```
loctree/
├── loctree-rs/
│   └── src/
│       ├── analyzer/
│       │   ├── rust.rs          # Extract symbol exports
│       │   └── mod.rs
│       └── lib.rs
├── reports/
│   ├── src/
│   │   ├── lib.rs               # Main report generator
│   │   ├── types.rs             # TwinsGraphData structs
│   │   ├── twins_graph.js       # NEW: Graph visualization
│   │   ├── graph_bootstrap.js   # Existing graph code
│   │   └── styles.rs
│   └── report.html              # Generated report
```

## Example Output

When integrated, the report will show:

1. **Stats Summary**: Total files, twins, dead parrots
2. **Interactive Graph**: Visual representation of symbol relationships
3. **Toolbar**: Layout selector, fit, reset, export PNG
4. **Tooltips**: Hover to see details
5. **Click Interactions**: Highlight connections, open in editor

## Verification Checklist

- [ ] `twins_graph.js` included in HTML report
- [ ] Cytoscape.js CDN link added
- [ ] `generate_twins_data_json()` implemented
- [ ] Analyzer tracks symbol exports
- [ ] CSS styles added
- [ ] Test data works in example HTML
- [ ] Graph renders in actual report
- [ ] Tooltips show correct information
- [ ] Layout algorithms work
- [ ] Export PNG functions
- [ ] Double-click opens files (if editor integration set up)

## Troubleshooting

### Graph doesn't render

1. Check browser console for errors
2. Verify Cytoscape.js loaded: `typeof cytoscape` should be `"function"`
3. Verify container exists: `document.getElementById('twins-graph-container')`
4. Check data format: `console.log(twinsData)`

### No nodes visible

- Verify `twinsData.exactTwins` is not empty
- Check that files are being tracked correctly
- Ensure `processTwinsData()` creates nodes

### Tooltips not showing

- Check z-index conflicts with other elements
- Verify tooltip element is added to body
- Check browser console for JavaScript errors

### Performance issues

- For graphs with > 500 nodes, consider:
    - Disabling animations (`animate: false`)
    - Using simpler layout (`layout: { name: 'preset' }`)
    - Reducing layout iterations (`numIter: 500`)

## Advanced Features

### Custom Symbol Filtering

Filter by symbol type (functions, structs, enums):

```javascript
// Add to toolbar
const filterSelect = document.createElement('select');
filterSelect.innerHTML = `
  <option value="all">All Symbols</option>
  <option value="functions">Functions Only</option>
  <option value="structs">Structs Only</option>
  <option value="enums">Enums Only</option>
`;
```

### Git Integration

Show which commit introduced each twin:

```rust
// Add git blame info to DeadParrot
struct DeadParrot {
    name: String,
    file: String,
    line: usize,
    commit: Option<String>,    // NEW
    author: Option<String>,    // NEW
    timestamp: Option<i64>,    // NEW
}
```

### Severity Scoring

Calculate a "twins severity score" for prioritization:

```javascript
function calculateSeverity(deadParrotCount, exportCount, connectionCount) {
    return (deadParrotCount * 10) +
           (exportCount * 2) +
           (connectionCount * 5);
}
```

---

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 The Loctree Team
