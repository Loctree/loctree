(function () {
  const graphs = window.__LOCTREE_GRAPHS || [];
  const formatNum = (n) => (typeof n === "number" && n.toLocaleString ? n.toLocaleString() : n || 0);
  const cyInstances = new Set();
  const darkToggles = new Set();

  // Small DOM construction helper used by the toolbars below.
  // Spec describes element shape declaratively; values are written via
  // textContent / setAttribute, never innerHTML — so no string template
  // ever reaches the parser. attrs: plain object. children: array of
  // (DOM node | string | spec object). text: shorthand for a single text child.
  const el = (tag, spec) => {
    spec = spec || {};
    const node = document.createElement(tag);
    if (spec.className) node.className = spec.className;
    if (spec.text !== undefined && spec.text !== null) node.textContent = String(spec.text);
    if (spec.style) {
      for (const k in spec.style) {
        if (Object.prototype.hasOwnProperty.call(spec.style, k)) node.style[k] = spec.style[k];
      }
    }
    if (spec.attrs) {
      for (const k in spec.attrs) {
        if (!Object.prototype.hasOwnProperty.call(spec.attrs, k)) continue;
        const v = spec.attrs[k];
        if (v === false || v === null || v === undefined) continue;
        if (v === true) node.setAttribute(k, "");
        else node.setAttribute(k, String(v));
      }
    }
    if (spec.children) {
      spec.children.forEach((child) => {
        if (child === null || child === undefined || child === false) return;
        if (child instanceof Node) node.appendChild(child);
        else if (typeof child === "string") node.appendChild(document.createTextNode(child));
        else if (typeof child === "object" && child.tag) node.appendChild(el(child.tag, child));
      });
    }
    return node;
  };
  const filterElements = (elements, opts) => {
    const text = (opts.text || "").toLowerCase();
    const minDeg = parseInt(opts.minDeg || "0", 10) || 0;
    const allowedComponents = opts.allowedComponents || new Set();
    let nodes = elements.nodes.map((n) => ({ data: { ...n.data }, position: { ...n.position } }));
    if (text) nodes = nodes.filter((n) => (n.data.id || "").toLowerCase().includes(text));
    if (allowedComponents.size) nodes = nodes.filter((n) => allowedComponents.has(n.data.component));

    let edges = elements.edges.map((e) => ({ data: { ...e.data } }));
    const nodeSet = new Set(nodes.map((n) => n.data.id));
    edges = edges.filter((e) => nodeSet.has(e.data.source) && nodeSet.has(e.data.target));

    if (minDeg > 0) {
      const deg = {};
      edges.forEach((e) => {
        deg[e.data.source] = (deg[e.data.source] || 0) + 1;
        deg[e.data.target] = (deg[e.data.target] || 0) + 1;
      });
      nodes = nodes.filter((n) => (deg[n.data.id] || 0) >= minDeg);
      const filteredSet = new Set(nodes.map((n) => n.data.id));
      edges = edges.filter((e) => filteredSet.has(e.data.source) && filteredSet.has(e.data.target));
    }
    return { nodes, edges };
  };

  // Graph-only dark mode (independent of page theme)
  const applyGraphDarkTheme = (on, graphs) => {
    graphs
      .filter(Boolean)
      .forEach((inst) => {
        if (inst && typeof inst.style === "function") {
          const style = inst.style();
          // Node text color: light for dark mode, dark for light mode
          style.selector("node").style("color", on ? "#eef2ff" : "#1a1a2e").update();
          // Edge text background: dark for dark mode, light for light mode
          style.selector("edge").style("text-background-color", on ? "#0f1115" : "#fff").update();
          // Graph container background via cytoscape
          inst.container().style.backgroundColor = on ? "#0f1115" : "#ffffff";
        }
      });
  };
  const setGraphDarkMode = (on) => applyGraphDarkTheme(on, Array.from(cyInstances));
  const applyGraphDarkShared = (on) => {
    darkToggles.forEach((chk) => {
      if (chk) chk.checked = on;
    });
    setGraphDarkMode(on);
  };

  graphs.forEach((g) => {
    const container = document.getElementById(g.id);
    if (!container || container.dataset.enhanced === "1") return;
    container.dataset.enhanced = "1";

    const components = Array.isArray(g.components) ? g.components : [];
    const componentMap = new Map();
    components.forEach((c) => componentMap.set(c.id, c));
    const detachedSet = new Set(components.filter((c) => c.detached).map((c) => c.id));
    const openBase = g.openBase || null;

    const originalParent = container.parentNode;
    const targetParent = originalParent || container.parentNode;

    // ========================================
    // Side-by-side split layout
    // ========================================
    const splitContainer = document.createElement("div");
    splitContainer.className = "graph-split-container";

    // LEFT PANEL: Component list with inner scroll
    const leftPanel = document.createElement("div");
    leftPanel.className = "graph-left-panel";

    // Component filter toolbar — built via DOM construction (no innerHTML)
    const componentBar = el("div", {
      className: "graph-toolbar component-toolbar",
      children: [
        el("label", {
          children: [
            "Component filter:",
            el("select", {
              attrs: { "data-role": "component-filter" },
              children: [
                el("option", { attrs: { value: "all" }, text: "All components" }),
                el("option", { attrs: { value: "isolates" }, text: "Isolates / size≤2" }),
                el("option", { attrs: { value: "size" }, text: "Size ≤ slider" }),
              ],
            }),
          ],
        }),
        el("label", {
          children: [
            "threshold:",
            el("input", {
              attrs: {
                type: "range",
                min: "1",
                max: "64",
                value: "8",
                "data-role": "component-threshold",
              },
            }),
            el("span", { attrs: { "data-role": "component-threshold-label" }, text: "8" }),
          ],
        }),
        el("span", {
          className: "graph-controls",
          children: [
            el("button", { attrs: { "data-role": "component-highlight" }, text: "Highlight selected" }),
            el("button", { attrs: { "data-role": "component-dim" }, text: "Dim others" }),
            el("button", { attrs: { "data-role": "component-copy" }, text: "Copy file list" }),
            el("button", { attrs: { "data-role": "component-export-json" }, text: "Export JSON" }),
            el("button", { attrs: { "data-role": "component-export-csv" }, text: "Export CSV" }),
            el("button", { attrs: { "data-role": "component-show-isolates" }, text: "Show isolates" }),
          ],
        }),
      ],
    });

    // Component panel — built via DOM construction (no innerHTML)
    const componentPanel = el("div", {
      className: "component-panel",
      children: [
        el("div", {
          className: "component-panel-header",
          children: [
            el("div", {
              children: [
                el("strong", { text: "Disconnected components" }),
                " ",
                el("span", { className: "muted", attrs: { "data-role": "component-summary" } }),
              ],
            }),
            el("div", {
              className: "panel-actions",
              children: [
                el("label", {
                  children: [
                    "show size ≤ ",
                    el("input", {
                      style: { width: "70px" },
                      attrs: {
                        type: "number",
                        min: "1",
                        value: "8",
                        "data-role": "component-size-limit",
                      },
                    }),
                  ],
                }),
                el("button", { attrs: { "data-role": "component-reset" }, text: "Reset view" }),
              ],
            }),
          ],
        }),
        el("table", {
          children: [
            el("thead", {
              children: [
                el("tr", {
                  children: [
                    el("th", { text: "id" }),
                    el("th", { text: "size" }),
                    el("th", { text: "sample" }),
                    el("th", { text: "isolated" }),
                    el("th", { text: "edges" }),
                    el("th", { text: "LOC" }),
                    el("th", { text: "actions" }),
                  ],
                }),
              ],
            }),
            el("tbody", { attrs: { "data-role": "component-table" } }),
          ],
        }),
      ],
    });

    leftPanel.appendChild(componentBar);
    leftPanel.appendChild(componentPanel);

    // RESIZE HANDLE
    const resizeHandle = document.createElement("div");
    resizeHandle.className = "graph-resize-handle";

    // RIGHT PANEL: Graph pinned to viewport
    const rightPanel = document.createElement("div");
    rightPanel.className = "graph-right-panel";

    // Graph controls toolbar — built via DOM construction (no innerHTML)
    const layoutOption = (value, label, selected) =>
      el("option", { attrs: { value, selected: selected ? true : false }, text: label });
    const legendItem = (color, label) =>
      el("span", {
        children: [
          el("span", { className: "legend-dot", style: { background: color } }),
          " " + label,
        ],
      });
    const toolbar = el("div", {
      className: "graph-toolbar",
      children: [
        el("label", {
          children: [
            "filter:",
            el("input", {
              attrs: {
                type: "text",
                size: "18",
                placeholder: "path substring or /regex/",
                "data-role": "filter-text",
                title: "Filter by path. Use plain text for substring match, or /pattern/ for regex.",
              },
            }),
          ],
        }),
        el("label", {
          children: [
            "min degree:",
            el("input", {
              style: { width: "60px" },
              attrs: {
                type: "number",
                min: "0",
                value: "0",
                "data-role": "min-degree",
              },
            }),
          ],
        }),
        el("label", {
          children: [
            "layout:",
            el("select", {
              attrs: { "data-role": "layout-select" },
              children: [
                layoutOption("cose", "cose (force)", false),
                layoutOption("dagre", "dagre (hierarchy)", false),
                layoutOption("cose-bilkent", "cose-bilkent", false),
                layoutOption("concentric", "concentric", true),
                layoutOption("breadthfirst", "breadthfirst", false),
                layoutOption("preset", "preset (original)", false),
              ],
            }),
          ],
        }),
        el("label", {
          children: [
            el("input", { attrs: { type: "checkbox", "data-role": "toggle-labels", checked: true } }),
            " labels",
          ],
        }),
        el("label", {
          children: [
            el("input", { attrs: { type: "checkbox", "data-role": "graph-dark" } }),
            " graph dark",
          ],
        }),
        el("span", {
          className: "graph-controls",
          children: [
            el("button", { attrs: { "data-role": "fit" }, text: "fit" }),
            el("button", { attrs: { "data-role": "relayout" }, text: "relayout" }),
            el("button", { attrs: { "data-role": "reset" }, text: "reset" }),
            el("button", { attrs: { "data-role": "fullscreen" }, text: "fullscreen" }),
            el("button", { attrs: { "data-role": "png" }, text: "png" }),
            el("button", { attrs: { "data-role": "json" }, text: "json" }),
          ],
        }),
        el("div", {
          className: "graph-legend",
          children: [
            legendItem("#4f81e1", "file"),
            legendItem("#888", "import"),
            legendItem("#e67e22", "re-export"),
            legendItem("#d1830f", "detached"),
          ],
        }),
      ],
    });

    // Move graph container into right panel
    container.style.height = "";  // Remove fixed height, let flex handle it
    container.style.flex = "1";
    container.style.minHeight = "0";

    rightPanel.appendChild(toolbar);
    rightPanel.appendChild(container);

    // Assemble split layout
    splitContainer.appendChild(leftPanel);
    splitContainer.appendChild(resizeHandle);
    splitContainer.appendChild(rightPanel);

    if (targetParent) targetParent.appendChild(splitContainer);

    // ========================================
    // Resize handle drag functionality
    // ========================================
    let isResizing = false;
    let startX = 0;
    let startWidth = 0;

    resizeHandle.addEventListener("mousedown", (e) => {
      isResizing = true;
      startX = e.clientX;
      startWidth = leftPanel.offsetWidth;
      resizeHandle.classList.add("active");
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      e.preventDefault();
    });

    document.addEventListener("mousemove", (e) => {
      if (!isResizing) return;
      const delta = e.clientX - startX;
      const newWidth = Math.min(600, Math.max(280, startWidth + delta));
      leftPanel.style.width = newWidth + "px";
    });

    document.addEventListener("mouseup", () => {
      if (isResizing) {
        isResizing = false;
        resizeHandle.classList.remove("active");
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
      }
    });

    const componentSelect = componentBar.querySelector('[data-role="component-filter"]');
    const sizeSlider = componentBar.querySelector('[data-role="component-threshold"]');
    const sizeLabel = componentBar.querySelector('[data-role="component-threshold-label"]');
    const tableBody = componentPanel.querySelector('[data-role="component-table"]');
    const summaryEl = componentPanel.querySelector('[data-role="component-summary"]');
    const sizeLimitInput = componentPanel.querySelector('[data-role="component-size-limit"]');
    const componentReset = componentPanel.querySelector('[data-role="component-reset"]');

    const addComponentOptions = () => {
      const sorted = [...components].sort((a, b) => a.size - b.size || a.id - b.id);
      sorted.forEach((comp) => {
        const opt = document.createElement("option");
        opt.value = `comp-${comp.id}`;
        const labelSample = comp.sample || (Array.isArray(comp.nodes) && comp.nodes[0]) || "";
        opt.textContent = `C${comp.id} • ${comp.size} nodes • ${labelSample}`;
        opt.dataset.size = comp.size;
        componentSelect.appendChild(opt);
      });
    };
    addComponentOptions();

    const state = {
      viewComponents: new Set(),
      highlightComponents: new Set(),
      sizeThreshold: parseInt(sizeSlider?.value || "8", 10) || 8,
      dimOthers: true,
    };

    const syncSize = (val) => {
      const safe = Math.max(1, Math.min(128, val || state.sizeThreshold));
      state.sizeThreshold = safe;
      if (sizeLabel) sizeLabel.textContent = safe;
      if (sizeSlider && sizeSlider.value !== String(safe)) sizeSlider.value = safe;
      if (sizeLimitInput && sizeLimitInput.value !== String(safe)) sizeLimitInput.value = safe;
      const sizeOption = componentSelect.querySelector('option[value="size"]');
      if (sizeOption) sizeOption.textContent = `Size ≤ ${safe}`;
    };
    syncSize(state.sizeThreshold);

    // Layout configuration helper - supports multiple algorithms
    const getLayoutConfig = (name, nodeCount) => {
      // Animate only moderate-sized graphs (fewer than 150 nodes) to avoid performance issues
      const animate = nodeCount < 150;
      const configs = {
        cose: {
          name: "cose",
          animate,
          animationDuration: animate ? 500 : 0,
          fit: true,
          padding: 30,
          nodeRepulsion: function(node) { return 8000; },
          idealEdgeLength: function(edge) { return 100; },
          edgeElasticity: function(edge) { return 100; },
          nestingFactor: 1.2,
          gravity: 1,
          numIter: 1000,
          initialTemp: 1000,
          coolingFactor: 0.99,
          minTemp: 1.0,
          randomize: false,
        },
        "cose-bilkent": {
          name: "cose-bilkent",
          animate,
          animationDuration: animate ? 500 : 0,
          fit: true,
          padding: 30,
          nodeRepulsion: 4500,
          idealEdgeLength: 80,
          edgeElasticity: 0.45,
          nestingFactor: 0.1,
          gravity: 0.25,
          numIter: 2500,
          tile: true,
          tilingPaddingVertical: 10,
          tilingPaddingHorizontal: 10,
          gravityRangeCompound: 1.5,
          gravityCompound: 1.0,
          gravityRange: 3.8,
          randomize: true,
        },
        dagre: {
          name: "dagre",
          animate,
          animationDuration: animate ? 500 : 0,
          fit: true,
          padding: 30,
          rankDir: "TB",  // top-to-bottom (hierarchy: caller → callee)
          nodeSep: 50,
          rankSep: 80,
          edgeSep: 10,
          ranker: "network-simplex",  // tight-tree, longest-path, network-simplex
        },
        concentric: {
          name: "concentric",
          animate,
          animationDuration: animate ? 500 : 0,
          fit: true,
          padding: 30,
          minNodeSpacing: 50,
          concentric: function(node) { return node.data("degree") || 0; },
          levelWidth: function(nodes) { return Math.max(1, Math.ceil(nodes.length / 8)); },
          clockwise: true,
          startAngle: 3 / 2 * Math.PI,
        },
        breadthfirst: {
          name: "breadthfirst",
          animate,
          animationDuration: animate ? 500 : 0,
          fit: true,
          padding: 30,
          directed: true,
          spacingFactor: 1.5,
          circle: false,
          grid: false,
          avoidOverlap: true,
        },
        preset: {
          name: "preset",
          animate: false,
          fit: true,
        },
      };
      return configs[name] || configs.preset;
    };

    const buildElements = () => {
      const rawNodes = Array.isArray(g.nodes) ? g.nodes : [];
      const rawEdges = Array.isArray(g.edges) ? g.edges : [];
      const nodeToComponent = new Map();
      const nodes = rawNodes.map((n) => {
        const size = Math.max(4, Math.min(30, Math.sqrt((n && n.loc) || 1)));
        const comp = n.component || 0;
        const compSize = (componentMap.get(comp) || {}).size || 0;
        const detached = detachedSet.has(comp) || !!n.detached;
        const isolate = (n.degree || 0) === 0 || compSize <= 2;
        const id = n.id || "";
        nodeToComponent.set(id, comp);
        return {
          data: {
            id,
            label: n.label || id || "",
            loc: n.loc || 0,
            size,
            full: id || "",
            component: comp,
            degree: n.degree || 0,
            detached,
            componentSize: compSize,
            isolate: isolate ? 1 : 0,
            color: detached ? "#d1830f" : "#4f81e1",
          },
          position: { x: n.x || 0, y: n.y || 0 },
        };
      });
      const edges = rawEdges.map((e, idx) => {
        const kind = (e && e[2]) || "import";
        const sourceComp = nodeToComponent.get(e[0]) || nodeToComponent.get(e[1]) || 0;
        const detached = detachedSet.has(sourceComp);
        const color = detached ? "#d1830f" : kind === "reexport" ? "#e67e22" : "#888";
        return {
          data: {
            id: "e" + idx,
            source: e[0],
            target: e[1],
            label: kind,
            kind,
            color,
            component: sourceComp,
            detached: detached ? 1 : 0,
          },
        };
      });
      return { nodes, edges };
    };

    const original = buildElements();
    const emptyOverlay = document.createElement("div");
    emptyOverlay.className = "graph-empty";
    emptyOverlay.style.display = "none";
    container.appendChild(emptyOverlay);
    let cy = cytoscape({
      container,
      elements: original,
      style: [
        { selector: "node", style: { label: "data(label)", "font-size": 10, "text-wrap": "wrap", "text-max-width": 120, "background-color": "data(color)", color: "#fff", width: "data(size)", height: "data(size)", "overlay-padding": 8, "overlay-opacity": 0 } },
        { selector: "node.detached", style: { "background-color": "#d1830f" } },
        { selector: "node.isolate", style: { "border-width": 2, "border-color": "#d74d26" } },
        { selector: "node.highlight", style: { "border-width": 3, "border-color": "#111", "shadow-blur": 12, "shadow-color": "#111", "shadow-opacity": 0.45, "shadow-offset-x": 0, "shadow-offset-y": 0, "z-index": 999 } },
        { selector: "node.dimmed", style: { opacity: 0.15 } },
        { selector: "edge", style: { "curve-style": "bezier", width: 1.1, "line-color": "data(color)", "target-arrow-color": "data(color)", "target-arrow-shape": "triangle", "arrow-scale": 0.7, label: "", "font-size": 9, "text-background-color": "#fff", "text-background-opacity": 0.8, "text-background-padding": 2 } },
        { selector: "edge.detached", style: { "line-color": "#d1830f", "target-arrow-color": "#d1830f" } },
        { selector: "edge.highlight", style: { width: 2, opacity: 0.9 } },
        { selector: "edge.dimmed", style: { opacity: 0.08 } },
      ],
      layout: { name: "preset", animate: false, fit: true },
    });
    cyInstances.add(cy);

    const download = (filename, content, type) => {
      const blob = new Blob([content], { type });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = filename;
      document.body.appendChild(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(url), 500);
    };

    const gatherSelectedComponents = () => {
      if (state.highlightComponents.size) return new Set(state.highlightComponents);
      if (state.viewComponents.size) return new Set(state.viewComponents);
      return new Set();
    };

    const applyHighlight = (forceDim) => {
      const highlightSet = gatherSelectedComponents();
      const dim = forceDim === undefined ? state.dimOthers : forceDim;

      cy.nodes().removeClass("dimmed highlight isolate detached");
      cy.edges().removeClass("dimmed highlight detached");

      cy.nodes().filter((n) => n.data("detached")).addClass("detached");
      cy.edges().filter((e) => e.data("detached")).addClass("detached");
      cy.nodes()
        .filter((n) => (n.data("isolate") || 0) === 1 || (n.data("componentSize") || 0) <= 2)
        .addClass("isolate");

      if (!highlightSet.size) return;
      const nodes = cy.nodes().filter((n) => highlightSet.has(n.data("component")));
      const edges = cy.edges().filter((e) => highlightSet.has(e.data("component")));
      nodes.addClass("highlight");
      edges.addClass("highlight");
      if (dim) {
        cy.nodes().not(nodes).addClass("dimmed");
        cy.edges().not(edges).addClass("dimmed");
      }
    };

    const layoutSelect = toolbar.querySelector('[data-role="layout-select"]');
    const getSelectedLayout = () => layoutSelect?.value || "concentric";

    const applyFilters = (runLayout = true) => {
      const text = (toolbar.querySelector('[data-role="filter-text"]')?.value || "").toLowerCase();
      const minDeg = parseInt(toolbar.querySelector('[data-role="min-degree"]')?.value || "0", 10) || 0;
      const allowedComponents = state.viewComponents;
      const filtered = filterElements(original, { text, minDeg, allowedComponents });
      let nodes = filtered.nodes;
      let edges = filtered.edges;

      if (nodes.length === 0) {
        emptyOverlay.style.display = "block";
        cy.elements().remove();
        return;
      }
      emptyOverlay.style.display = "none";

      cy.elements().remove();
      cy.add({ nodes, edges });

      const showLabels = toolbar.querySelector('[data-role="toggle-labels"]').checked;
      const autoHide = nodes.length > 800;
      const labelsOn = showLabels && !autoHide;
      cy.style().selector("node").style("label", labelsOn ? "data(label)" : "").update();

      if (runLayout) {
        const layoutName = getSelectedLayout();
        const layoutConfig = getLayoutConfig(layoutName, nodes.length);
        cy.layout(layoutConfig).run();
      }
      applyHighlight();
    };

    const runRelayout = () => {
      const layoutName = getSelectedLayout();
      const nodeCount = cy.nodes().length;
      const layoutConfig = getLayoutConfig(layoutName, nodeCount);
      cy.layout(layoutConfig).run();
    };

    // Fit / reset / relayout / dark / fullscreen
    const fitBtn = toolbar.querySelector('[data-role="fit"]');
    const relayoutBtn = toolbar.querySelector('[data-role="relayout"]');
    const resetBtn = toolbar.querySelector('[data-role="reset"]');
    const darkChk = toolbar.querySelector('[data-role="graph-dark"]');
    const fsBtn = toolbar.querySelector('[data-role="fullscreen"]');
    const pngBtn = toolbar.querySelector('[data-role="png"]');
    const jsonBtn = toolbar.querySelector('[data-role="json"]');

    if (fitBtn) fitBtn.addEventListener("click", () => cy.fit());
    if (relayoutBtn) relayoutBtn.addEventListener("click", runRelayout);
    if (layoutSelect) layoutSelect.addEventListener("change", runRelayout);
    if (resetBtn)
      resetBtn.addEventListener("click", () => {
        cy.elements().remove();
        cy.add(original);
        state.viewComponents = new Set();
        state.highlightComponents = new Set();
        layoutSelect.value = "preset";
        applyFilters(false);
        cy.layout({ name: "preset", animate: false, fit: true }).run();
      });

    if (pngBtn)
      pngBtn.addEventListener("click", () => {
        const dark = darkChk && darkChk.checked;
        const dataUrl = cy.png({ bg: dark ? "#0f1115" : "#ffffff", full: true, scale: 2 });
        const a = document.createElement("a");
        a.href = dataUrl;
        a.download = `${g.id}-graph.png`;
        document.body.appendChild(a);
        a.click();
        a.remove();
      });

    if (jsonBtn)
      jsonBtn.addEventListener("click", () => {
        const payload = {
          nodes: cy.nodes().map((n) => n.data()),
          edges: cy.edges().map((e) => ({ source: e.data("source"), target: e.data("target"), kind: e.data("kind") })),
          filter: toolbar.querySelector('[data-role="filter-text"]')?.value || "",
          minDegree: parseInt(toolbar.querySelector('[data-role="min-degree"]')?.value || "0", 10) || 0,
          components,
          highlightedComponents: Array.from(state.highlightComponents),
          viewedComponents: Array.from(state.viewComponents),
        };
        download(`${g.id}-graph.json`, JSON.stringify(payload, null, 2), "application/json");
      });

    if (darkChk) {
      darkToggles.add(darkChk);
      darkChk.addEventListener("change", () => applyGraphDarkShared(darkChk.checked));
    }

    const fsTarget = container;
    if (fsBtn && fsTarget && fsTarget.requestFullscreen) {
      fsBtn.addEventListener("click", () => {
        if (document.fullscreenElement) {
          document.exitFullscreen();
        } else {
          fsTarget.requestFullscreen().catch(() => {});
        }
      });
      document.addEventListener("fullscreenchange", () => {
        fsBtn.textContent = document.fullscreenElement ? "exit fullscreen" : "fullscreen";
        if (!document.fullscreenElement) cy.fit();
      });
    }

    // Tooltip on hover/click (sticky behavior)
    const tooltip = document.createElement("div");
    tooltip.style.position = "fixed";
    tooltip.style.pointerEvents = "auto";
    tooltip.style.background = "#111";
    tooltip.style.color = "#fff";
    tooltip.style.padding = "6px 8px";
    tooltip.style.borderRadius = "6px";
    tooltip.style.fontSize = "12px";
    tooltip.style.display = "none";
    tooltip.style.zIndex = 9999;
    document.body.appendChild(tooltip);

    let nodeHover = false;
    let tooltipHover = false;
    let hideTimeout = null;

    const hideTip = () => {
      if (hideTimeout) {
        clearTimeout(hideTimeout);
        hideTimeout = null;
      }
      nodeHover = false;
      tooltipHover = false;
      tooltip.style.display = "none";
    };

    const scheduleHide = () => {
      if (hideTimeout) {
        clearTimeout(hideTimeout);
      }
      hideTimeout = setTimeout(() => {
        if (!nodeHover && !tooltipHover) {
          hideTip();
        }
      }, 350);
    };

    const showTip = (evt, node) => {
      // Cancel any pending hide
      if (hideTimeout) {
        clearTimeout(hideTimeout);
        hideTimeout = null;
      }

      const data = node.data();
      const path = data.full || data.id;
      const comp = componentMap.get(data.component);
      const compLabel = comp ? `C${comp.id} (${comp.size} nodes${comp.detached ? ", detached" : ""})` : "—";

      // Get current filter text for highlighting
      const filterInput = toolbar.querySelector('[data-role="filter-text"]');
      const filterText = (filterInput?.value || "").toLowerCase().trim();

      // Get incoming/outgoing edges for context
      const incomingEdges = cy.edges().filter(e => e.data("target") === data.id);
      const outgoingEdges = cy.edges().filter(e => e.data("source") === data.id);

      // Build tooltip using safe DOM APIs (no innerHTML with user data)
      tooltip.textContent = ""; // Clear previous content

      // Row 1: Path with optional highlight
      const pathRow = document.createElement("div");
      pathRow.style.marginBottom = "4px";
      const pathStrong = document.createElement("strong");
      if (filterText && path.toLowerCase().includes(filterText)) {
        // Case-insensitive highlight using DOM nodes
        const idx = path.toLowerCase().indexOf(filterText);
        pathStrong.appendChild(document.createTextNode(path.slice(0, idx)));
        const mark = document.createElement("mark");
        mark.style.cssText = "background:#ffd700;color:#000;padding:0 2px;border-radius:2px";
        mark.textContent = path.slice(idx, idx + filterText.length);
        pathStrong.appendChild(mark);
        pathStrong.appendChild(document.createTextNode(path.slice(idx + filterText.length)));
      } else {
        pathStrong.textContent = path;
      }
      pathRow.appendChild(pathStrong);
      tooltip.appendChild(pathRow);

      // Row 2: LOC and degree
      const statsRow = document.createElement("div");
      statsRow.textContent = `LOC: ${data.loc || 0} | degree: ${data.degree || 0}`;
      tooltip.appendChild(statsRow);

      // Row 3: Edge info
      const edgeRow = document.createElement("div");
      edgeRow.textContent = `imports: ${outgoingEdges.length} | imported by: ${incomingEdges.length}`;
      tooltip.appendChild(edgeRow);

      // Row 4: Component info
      const compRow = document.createElement("div");
      compRow.textContent = `component: ${compLabel}`;
      tooltip.appendChild(compRow);

      // Row 5: Actions (copy button + open link)
      const actionsRow = document.createElement("div");
      actionsRow.style.cssText = "margin-top:4px;display:flex;gap:8px;align-items:center";

      const copyBtn = document.createElement("button");
      copyBtn.textContent = "copy path";
      copyBtn.style.cssText = "font-size:10px;cursor:pointer";
      copyBtn.addEventListener("click", () => navigator.clipboard.writeText(path));
      actionsRow.appendChild(copyBtn);

      if (openBase) {
        const openLink = document.createElement("a");
        openLink.href = `${openBase}/open?f=${encodeURIComponent(path)}&l=1`;
        openLink.textContent = "open in editor";
        openLink.style.cssText = "color:#6af;text-decoration:underline;font-size:10px";
        actionsRow.appendChild(openLink);
      }
      tooltip.appendChild(actionsRow);
      const rect = container.getBoundingClientRect();
      // Fixed positioning is relative to viewport, no scroll offset needed
      let left = rect.left + evt.renderedPosition.x + 12;
      let top = rect.top + evt.renderedPosition.y + 12;
      // Ensure tooltip stays within viewport bounds (measure after content)
      tooltip.style.visibility = "hidden";
      tooltip.style.display = "block";
      const bounds = tooltip.getBoundingClientRect();
      const tooltipWidth = bounds.width || 220;
      const tooltipHeight = bounds.height || 120;
      const maxLeft = window.innerWidth - tooltipWidth - 10;
      const maxTop = window.innerHeight - tooltipHeight - 10;
      if (left > maxLeft) left = maxLeft;
      if (top > maxTop) top = Math.max(10, top - tooltipHeight - 24);
      tooltip.style.left = left + "px";
      tooltip.style.top = top + "px";
      tooltip.style.visibility = "visible";
      nodeHover = true;
    };

    tooltip.addEventListener("mouseenter", () => {
      tooltipHover = true;
      if (hideTimeout) {
        clearTimeout(hideTimeout);
        hideTimeout = null;
      }
    });
    tooltip.addEventListener("mouseleave", () => {
      tooltipHover = false;
      scheduleHide();
    });

    cy.off("mouseover");
    cy.off("mouseout");
    cy.off("tap");
    cy.off("tapdrag");
    cy.off("pan");
    cy.off("zoom");
    cy.on("mouseover", "node", (evt) => {
      nodeHover = true;
      if (hideTimeout) { clearTimeout(hideTimeout); hideTimeout = null; }
      showTip(evt, evt.target);
    });
    cy.on("mouseout", "node", () => {
      nodeHover = false;
      scheduleHide();
    });
    cy.on("tap", "node", (evt) => {
      nodeHover = true;
      showTip(evt, evt.target);
    });
    cy.on("tapdrag", "node", () => {
      nodeHover = false;
      hideTip();
    });
    // Hide tooltip on pan/zoom to avoid stale position
    cy.on("pan zoom", () => {
      if (tooltip.style.display !== "none") {
        hideTip();
      }
    });

    const updateComponentFilter = () => {
      const val = componentSelect.value;
      const set = new Set();
      if (val === "isolates") {
        components.filter((c) => c.size <= 2 || c.isolated_count > 0).forEach((c) => set.add(c.id));
      } else if (val === "size") {
        components.filter((c) => c.size <= state.sizeThreshold).forEach((c) => set.add(c.id));
      } else if (val.startsWith("comp-")) {
        const id = parseInt(val.replace("comp-", ""), 10);
        if (Number.isFinite(id)) set.add(id);
      }
      state.viewComponents = set;
      state.highlightComponents = new Set(set);
      applyFilters();
    };

    const renderComponentTable = () => {
      const limit = parseInt(sizeLimitInput.value || state.sizeThreshold, 10) || state.sizeThreshold;
      syncSize(limit);
      const rows = [...components].sort((a, b) => a.size - b.size || a.id - b.id);
      const filtered = rows.filter((c) => c.size <= limit);
      tableBody.textContent = ""; // Clear using safe DOM API
      filtered.forEach((comp) => {
        const sample = comp.sample || (comp.nodes && comp.nodes[0]) || "";
        const sampleHref = openBase ? `${openBase}/open?f=${encodeURIComponent(sample)}&l=1` : null;
        const warn = comp.detached ? " (detached)" : "";
        const edgeCount = comp.edges !== undefined ? comp.edges : comp.edge_count;

        // Build table row using safe DOM APIs (no innerHTML with user data)
        const tr = document.createElement("tr");

        // Cell 1: Component ID with warning
        const td1 = document.createElement("td");
        td1.textContent = `C${comp.id}${warn}`;
        tr.appendChild(td1);

        // Cell 2: Size
        const td2 = document.createElement("td");
        td2.textContent = comp.size;
        tr.appendChild(td2);

        // Cell 3: Sample file (link or code)
        const td3 = document.createElement("td");
        if (sampleHref) {
          const link = document.createElement("a");
          link.href = sampleHref;
          link.textContent = sample;
          td3.appendChild(link);
        } else {
          const code = document.createElement("code");
          code.textContent = sample;
          td3.appendChild(code);
        }
        tr.appendChild(td3);

        // Cell 4: Isolated count
        const td4 = document.createElement("td");
        td4.textContent = comp.isolated_count;
        tr.appendChild(td4);

        // Cell 5: Edge count
        const td5 = document.createElement("td");
        td5.textContent = edgeCount || 0;
        tr.appendChild(td5);

        // Cell 6: LOC sum
        const td6 = document.createElement("td");
        td6.textContent = formatNum(comp.loc_sum);
        tr.appendChild(td6);

        // Cell 7: Highlight button
        const td7 = document.createElement("td");
        const btn = document.createElement("button");
        btn.setAttribute("data-role", "component-focus");
        btn.setAttribute("data-comp", comp.id);
        btn.textContent = "Highlight";
        td7.appendChild(btn);
        tr.appendChild(td7);

        tableBody.appendChild(tr);
      });
      summaryEl.textContent = `${filtered.length} / ${components.length} components ≤ ${limit} nodes • detached: ${detachedSet.size} • isolates: ${
        components.filter((c) => c.size <= 2 || c.isolated_count > 0).length
      }`;
      tableBody.querySelectorAll('[data-role="component-focus"]').forEach((btn) => {
        btn.addEventListener("click", (evt) => {
          const compId = parseInt(evt.currentTarget.getAttribute("data-comp"), 10);
          if (!Number.isFinite(compId)) return;
          componentSelect.value = `comp-${compId}`;
          state.viewComponents = new Set([compId]);
          state.highlightComponents = new Set([compId]);
          applyFilters();
          const nodes = cy.nodes().filter((n) => n.data("component") === compId);
          if (nodes.length) cy.fit(nodes, 30);
        });
      });
    };

    const showIsolatesBtn = componentBar.querySelector('[data-role="component-show-isolates"]');
    const highlightBtn = componentBar.querySelector('[data-role="component-highlight"]');
    const dimBtn = componentBar.querySelector('[data-role="component-dim"]');
    const copyBtn = componentBar.querySelector('[data-role="component-copy"]');
    const exportJsonBtn = componentBar.querySelector('[data-role="component-export-json"]');
    const exportCsvBtn = componentBar.querySelector('[data-role="component-export-csv"]');

    const gatherNodesForExport = () => {
      const target = gatherSelectedComponents();
      const nodes = target.size ? cy.nodes().filter((n) => target.has(n.data("component"))) : cy.nodes();
      return nodes.map((n) => n.data());
    };

    if (showIsolatesBtn) showIsolatesBtn.addEventListener("click", () => {
      componentSelect.value = "isolates";
      updateComponentFilter();
    });
    if (componentSelect) componentSelect.addEventListener("change", updateComponentFilter);
    if (sizeSlider)
      sizeSlider.addEventListener("input", (e) => {
        syncSize(parseInt(e.target.value, 10));
        if (componentSelect.value === "size") updateComponentFilter();
        renderComponentTable();
      });
    if (sizeLimitInput)
      sizeLimitInput.addEventListener("input", (e) => {
        syncSize(parseInt(e.target.value, 10));
        if (componentSelect.value === "size") updateComponentFilter();
        renderComponentTable();
      });
    if (componentReset)
      componentReset.addEventListener("click", () => {
        componentSelect.value = "all";
        state.viewComponents = new Set();
        state.highlightComponents = new Set();
        applyFilters();
      });
    if (highlightBtn)
      highlightBtn.addEventListener("click", () => {
        state.dimOthers = false;
        applyHighlight(false);
        const comps = gatherSelectedComponents();
        if (comps.size) {
          const nodes = cy.nodes().filter((n) => comps.has(n.data("component")));
          if (nodes.length) cy.fit(nodes, 30);
        }
      });
    if (dimBtn) dimBtn.addEventListener("click", () => {
      state.dimOthers = true;
      applyHighlight(true);
    });
    if (copyBtn)
      copyBtn.addEventListener("click", () => {
        const nodes = gatherNodesForExport();
        const lines = nodes.map((n) => `${n.id || ""}, loc=${n.loc || 0}, degree=${n.degree || 0}, comp=C${n.component || "?"}`);
        navigator.clipboard.writeText(lines.join("\n"));
      });
    if (exportJsonBtn)
      exportJsonBtn.addEventListener("click", () => {
        const nodes = gatherNodesForExport();
        download(`${g.id}-component.json`, JSON.stringify(nodes, null, 2), "application/json");
      });
    if (exportCsvBtn)
      exportCsvBtn.addEventListener("click", () => {
        const nodes = gatherNodesForExport();
        const header = "path,loc,degree,component";
        const rows = nodes.map((n) => `${n.id || ""},${n.loc || 0},${n.degree || 0},C${n.component || ""}`);
        download(`${g.id}-component.csv`, [header, ...rows].join("\n"), "text/csv");
      });

    toolbar.querySelectorAll("input").forEach((inp) => {
      inp.addEventListener("input", () => applyFilters());
      inp.addEventListener("change", () => applyFilters());
    });

    renderComponentTable();
    applyFilters();
    // Initial fit after layout completes (layout is async)
    cy.one("layoutstop", () => {
      cy.fit();
    });
  });
})();
