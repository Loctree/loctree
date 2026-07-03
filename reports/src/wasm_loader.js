/**
 * WASM Graph Loader for loctree reports
 *
 * Loads the WASM graph renderer with Cytoscape.js fallback.
 * Developed with 💀 by The Loctree Team ⓒ 2025-2026 
 */

(function () {
  'use strict';

  // Check for WASM support
  const hasWasmSupport = typeof WebAssembly === 'object' &&
    typeof WebAssembly.instantiate === 'function';

  // State
  let wasmModule = null;
  let wasmReady = false;
  let useFallback = false;

  /**
   * Decode base64 to Uint8Array
   */
  function base64ToBytes(base64) {
    const binaryString = atob(base64);
    const bytes = new Uint8Array(binaryString.length);
    for (let i = 0; i < binaryString.length; i++) {
      bytes[i] = binaryString.charCodeAt(i);
    }
    return bytes;
  }

  /**
   * Load WASM module from base64 encoded string
   */
  async function loadWasmFromBase64(wasmBase64, jsGlue) {
    if (!hasWasmSupport) {
      throw new Error('WebAssembly not supported');
    }

    // Decode base64 to bytes
    const wasmBytes = base64ToBytes(wasmBase64);

    // Create module from bytes
    const wasmModule = await WebAssembly.compile(wasmBytes);

    // If we have JS glue code, evaluate it to set up imports
    if (jsGlue) {
      // The glue code expects to find the WASM module
      // We need to eval it in a way that exposes the init function
      const glueScript = document.createElement('script');
      glueScript.textContent = jsGlue;
      document.head.appendChild(glueScript);
    }

    return wasmModule;
  }

  /**
   * Initialize WASM module
   */
  async function initWasm() {
    const wasmConfig = window.__LOCTREE_WASM_CONFIG;

    if (!wasmConfig || !wasmConfig.wasmBase64) {
      console.log('[loctree] No WASM config found, using Cytoscape fallback');
      useFallback = true;
      return false;
    }

    try {
      wasmModule = await loadWasmFromBase64(
        wasmConfig.wasmBase64,
        wasmConfig.jsGlue
      );
      wasmReady = true;
      console.log('[loctree] WASM module loaded successfully');
      return true;
    } catch (error) {
      console.warn('[loctree] WASM loading failed, using Cytoscape fallback:', error);
      useFallback = true;
      return false;
    }
  }

  /**
   * Render graph using WASM (placeholder - outputs DOT for now)
   * Uses DOM API instead of innerHTML to prevent XSS
   */
  function renderGraphWasm(graphData, container, isDark) {
    // Clear container safely
    while (container.firstChild) {
      container.removeChild(container.firstChild);
    }

    const dot = isDark ? graphData.dotDark : graphData.dot;

    if (!dot) {
      const errorDiv = document.createElement('div');
      errorDiv.className = 'graph-error';
      errorDiv.textContent = 'No DOT data available';
      container.appendChild(errorDiv);
      return;
    }

    const nodeCount = graphData.nodes ? graphData.nodes.length : 0;
    const edgeCount = graphData.edges ? graphData.edges.length : 0;

    // Build DOM structure safely
    const wrapper = document.createElement('div');
    wrapper.className = 'wasm-graph-placeholder';
    wrapper.style.cssText = `
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      height: 100%;
      color: var(--theme-text-secondary);
      font-family: var(--font-mono);
      font-size: 12px;
      gap: 8px;
    `;

    const title = document.createElement('div');
    title.style.cssText = 'font-size: 14px; color: var(--theme-text-primary);';
    title.textContent = 'WASM Graph Renderer';
    wrapper.appendChild(title);

    const stats = document.createElement('div');
    stats.textContent = `${nodeCount} nodes, ${edgeCount} edges`;
    wrapper.appendChild(stats);

    const info = document.createElement('div');
    info.style.cssText = 'opacity: 0.7; font-size: 10px;';
    info.textContent = `DOT format ready (${dot.length} chars)`;
    wrapper.appendChild(info);

    const details = document.createElement('details');
    details.style.cssText = 'margin-top: 12px; max-width: 100%; overflow: auto;';

    const summary = document.createElement('summary');
    summary.style.cursor = 'pointer';
    summary.textContent = 'View DOT source';
    details.appendChild(summary);

    const pre = document.createElement('pre');
    pre.style.cssText = `
      text-align: left;
      padding: 8px;
      background: var(--theme-surface);
      border-radius: 4px;
      max-height: 200px;
      overflow: auto;
      font-size: 10px;
    `;
    pre.textContent = dot; // textContent is safe, no need to escape
    details.appendChild(pre);

    wrapper.appendChild(details);
    container.appendChild(wrapper);
  }

  /**
   * Initialize all graphs on the page
   */
  async function initGraphs() {
    const graphs = window.__LOCTREE_GRAPHS || [];

    if (graphs.length === 0) {
      console.log('[loctree] No graphs to render');
      return;
    }

    // Try to init WASM first
    const wasmAvailable = await initWasm();

    // Check current theme
    const isDark = document.documentElement.classList.contains('dark');

    for (const graphData of graphs) {
      const container = document.getElementById(graphData.id);
      if (!container) {
        console.warn(`[loctree] Graph container not found: ${graphData.id}`);
        continue;
      }

      // Find WASM target div
      const wasmTarget = container.querySelector('.graph-wasm-target');

      if (wasmAvailable && wasmTarget && graphData.dot) {
        // Use WASM renderer
        renderGraphWasm(graphData, wasmTarget, isDark);
        container.classList.add('wasm-rendered');
      } else if (typeof initCytoscapeGraph === 'function') {
        // Fallback to Cytoscape
        if (wasmTarget) wasmTarget.remove(); // Remove WASM target, Cytoscape needs bare container
        initCytoscapeGraph(graphData, container);
        container.classList.add('cytoscape-rendered');
      } else {
        // No renderer available - use DOM API for safety
        while (container.firstChild) {
          container.removeChild(container.firstChild);
        }
        const errorDiv = document.createElement('div');
        errorDiv.className = 'graph-error';
        errorDiv.style.cssText = `
          display: flex;
          align-items: center;
          justify-content: center;
          height: 100%;
          color: var(--theme-text-secondary);
        `;
        errorDiv.textContent = 'Graph renderer not available';
        container.appendChild(errorDiv);
      }
    }
  }

  // Listen for theme changes to re-render WASM graphs
  const observer = new MutationObserver((mutations) => {
    for (const mutation of mutations) {
      if (mutation.attributeName === 'class') {
        const isDark = document.documentElement.classList.contains('dark');
        // Re-render WASM graphs with new theme
        document.querySelectorAll('.wasm-rendered .graph-wasm-target').forEach(target => {
          const graphId = target.dataset.graphId;
          const graphData = (window.__LOCTREE_GRAPHS || []).find(g => g.id === graphId);
          if (graphData) {
            renderGraphWasm(graphData, target, isDark);
          }
        });
      }
    }
  });

  // Observe theme changes on html element
  observer.observe(document.documentElement, { attributes: true });

  // Export for external use
  window.__LOCTREE_WASM = {
    initGraphs,
    renderGraphWasm,
    hasWasmSupport,
    isReady: () => wasmReady,
    isFallback: () => useFallback
  };

  // Auto-init when DOM is ready
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initGraphs);
  } else {
    initGraphs();
  }
})();
