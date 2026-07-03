# Twin DNA Graph Visualization

A stunning interactive graph visualization for displaying "twin" symbols (functions/types with identical names exported from different files) and "dead parrots" (duplicate exports).

## Features

### Visual Design

- **Node Color Gradient**: Green (0 dead parrots) → Yellow → Orange → Red (many dead parrots)
- **Node Size**: Proportional to the number of exports from that file
- **Edge Color Gradient**: Blue (few shared symbols) → Purple → Magenta (many shared symbols)
- **Edge Thickness**: Proportional to the number of shared symbols between files
- **Border Styling**: Red border for files with dead parrots, blue for clean files

### Interactive Features

1. **Hover on Node**
   - Shows file path with filter highlighting
   - Displays export count, dead parrot count, and connection degree
   - Lists all dead parrots in that file (with line numbers)
   - Hints for double-click to open in editor

2. **Hover on Edge**
   - Shows count of shared symbols
   - Lists all shared symbol names

3. **Click on Node**
   - Highlights the node and all its connections
   - Dims unrelated nodes and edges
   - Creates a visual focus on the selected component

4. **Click on Background**
   - Clears all highlights
   - Returns to full graph view

5. **Double-Click on Node**
   - Opens the file in your editor via `loctree://` URL scheme (if configured)

### Layout Algorithms

The graph supports multiple layout algorithms optimized for different use cases:

- **COSE (Force-Directed)** - Default, groups related files with physics simulation
- **COSE-Bilkent** - Advanced force-directed with better performance
- **Concentric** - Arranges nodes in concentric circles by connection degree
- **Circle** - Simple circular layout
- **Grid** - Organized grid layout

The default COSE layout has intelligent edge length calculation:
- **Shorter edges** for files with many shared symbols (pulls them together)
- **Stronger repulsion** for files with many dead parrots (spreads them out)
- **Higher elasticity** for edges with more shared symbols (stronger attraction)

## Usage

### Basic Integration

```html
<!DOCTYPE html>
<html>
<head>
  <title>Twins Graph</title>
</head>
<body>
  <div id="twins-graph" style="width: 100%; height: 800px;"></div>

  <!-- Include Cytoscape.js -->
  <script src="https://cdnjs.cloudflare.com/ajax/libs/cytoscape/3.28.1/cytoscape.min.js"></script>

  <!-- Include twins_graph.js -->
  <script src="twins_graph.js"></script>

  <script>
    // Your twins data from loctree analyzer
    const twinsData = {
      exactTwins: [
        { symbol: 'parse', files: ['src/a.rs', 'src/b.rs', 'src/c.rs'] },
        { symbol: 'analyze', files: ['src/x.rs', 'src/y.rs'] },
        // ...
      ],
      deadParrots: [
        { name: 'parse', file: 'src/a.rs', line: 45 },
        { name: 'parse', file: 'src/b.rs', line: 23 },
        // ...
      ]
    };

    // Initialize graph
    const cy = buildTwinsGraph(
      twinsData,
      'twins-graph',
      'loctree://' // Optional: base URL for editor integration
    );
  </script>
</body>
</html>
```

### Data Format

#### Input Data Structure

```typescript
interface TwinsData {
  exactTwins: ExactTwin[];
  deadParrots: DeadParrot[];
}

interface ExactTwin {
  symbol: string;      // The duplicated symbol name
  files: string[];     // Files that export this symbol
}

interface DeadParrot {
  name: string;        // Symbol name
  file: string;        // File path
  line: number;        // Line number where it's defined
}
```

#### Example Data

```javascript
const exampleData = {
  exactTwins: [
    {
      symbol: 'Error',
      files: [
        'src/error.rs',
        'src/parser/error.rs',
        'src/analyzer/error.rs'
      ]
    },
    {
      symbol: 'Config',
      files: ['src/config.rs', 'src/formatter/config.rs']
    }
  ],
  deadParrots: [
    { name: 'Error', file: 'src/error.rs', line: 12 },
    { name: 'Error', file: 'src/parser/error.rs', line: 34 },
    { name: 'Error', file: 'src/analyzer/error.rs', line: 56 },
    { name: 'Config', file: 'src/config.rs', line: 23 },
    { name: 'Config', file: 'src/formatter/config.rs', line: 45 }
  ]
};
```

### API Reference

#### `buildTwinsGraph(twinsData, containerId, openBase)`

Creates and renders the twins graph.

**Parameters:**
- `twinsData` (Object) - The twins analysis data
  - `exactTwins` (Array) - Array of twin objects
  - `deadParrots` (Array) - Array of dead parrot objects
- `containerId` (string) - ID of the DOM container element
- `openBase` (string, optional) - Base URL for editor integration (e.g., `'loctree://'`)

**Returns:**
- Cytoscape instance or `null` if container not found

**Example:**
```javascript
const cy = buildTwinsGraph(data, 'graph-container', 'loctree://');
```

### Toolbar Controls

The graph includes a built-in toolbar with:

- **Stats Display**: Shows total files, twins, and dead parrots
- **Layout Selector**: Switch between different layout algorithms
- **Fit Button**: Fits the graph to the viewport
- **Reset Button**: Clears highlights and refits the graph
- **Export PNG Button**: Downloads the graph as a high-resolution PNG image

### Color Coding

#### Node Colors (Dead Parrot Count)

| Count | Color | Meaning |
|-------|-------|---------|
| 0 | Green `#22c55e` | No dead parrots - clean file |
| 1-25% | Green → Yellow | Few dead parrots |
| 25-50% | Yellow → Orange | Moderate dead parrots |
| 50-75% | Orange → Deep Orange | Many dead parrots |
| 75-100% | Deep Orange → Red `#dc2626` | Critical - needs refactoring |

#### Edge Colors (Shared Symbol Count)

| Count | Color | Meaning |
|-------|-------|---------|
| Low | Light Blue `#60a5fa` | Few shared symbols |
| Medium | Purple `#a855f7` | Moderate symbol sharing |
| High | Magenta `#ec4899` | Heavy symbol sharing - potential namespace collision |

### Performance Considerations

- **Animation**: Enabled for graphs with < 150 nodes, disabled for larger graphs
- **Layout Iterations**: 1500 iterations for optimal positioning
- **Repulsion/Attraction**: Dynamic based on node properties
- **Tooltip Delay**: 100ms debounce to avoid flicker

### Integration with loctree

This graph is designed to integrate seamlessly with the loctree analyzer:

1. Run loctree analyzer to detect twins/dead parrots
2. Export data in the required format
3. Embed the graph in the HTML report
4. Enable `loctree://` URL scheme for editor integration

### Customization

#### Styling

Modify the graph style by editing `getTwinsGraphStyle()` in `twins_graph.js`:

```javascript
// Example: Change node color gradient
function getNodeColor(deadParrotCount, maxDeadParrots) {
  // Custom color logic here
  return customColor;
}
```

#### Layout Parameters

Adjust layout behavior in the `cytoscape()` initialization:

```javascript
layout: {
  name: 'cose',
  nodeRepulsion: 8000,        // Increase for more spread
  idealEdgeLength: 100,       // Shorter = tighter clusters
  edgeElasticity: 100,        // Higher = stronger attraction
  gravity: 0.5,               // Higher = more centered
  // ... other parameters
}
```

## Architecture

### Core Functions

1. **`buildTwinsGraph()`** - Main entry point, creates the graph
2. **`processTwinsData()`** - Converts input data to Cytoscape elements
3. **`getTwinsGraphStyle()`** - Generates dynamic Cytoscape styles
4. **`setupInteractivity()`** - Adds hover/click/double-click handlers
5. **`setupToolbar()`** - Creates control toolbar
6. **`getNodeColor()`** - Calculates node color based on dead parrots
7. **`getEdgeColor()`** - Calculates edge color based on shared symbols
8. **`interpolateColor()`** - Smooth color gradient interpolation

### Data Flow

```
Input: TwinsData
    ↓
processTwinsData()
    ↓
Cytoscape Elements (nodes, edges, stats)
    ↓
getTwinsGraphStyle()
    ↓
Cytoscape Instance
    ↓
setupInteractivity() + setupToolbar()
    ↓
Interactive Graph
```

## Example Use Cases

### 1. Identify Namespace Pollution

Files with red nodes and thick edges are exporting many duplicate symbols - candidates for refactoring into a shared module.

### 2. Find Module Boundaries

Clusters of tightly connected files suggest logical module boundaries. Files on the periphery might belong to different modules.

### 3. Detect Dead Code

Files with many dead parrots but few connections might contain dead code or poorly organized utilities.

### 4. Prioritize Refactoring

- **High Priority**: Red nodes with many thick edges (central, problematic files)
- **Medium Priority**: Orange nodes with moderate connections
- **Low Priority**: Green nodes (clean files)

## Browser Compatibility

- Chrome/Edge: Full support
- Firefox: Full support
- Safari: Full support (with backdrop-filter)
- Mobile: Touch-optimized (tap, double-tap, pinch-to-zoom)

## Dependencies

- **Cytoscape.js**: v3.28.1 or later
- Modern browser with ES6 support

## Future Enhancements

Potential improvements for future versions:

- [ ] Export to GraphML/DOT format
- [ ] Clustering algorithm to group related files
- [ ] Time-series view showing evolution of twins over commits
- [ ] Integration with LSP for real-time symbol resolution
- [ ] AI-powered refactoring suggestions
- [ ] Diff view comparing two git commits

---

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 The Loctree Team
