//! CSS styles for the HTML report.
//!
//! This module contains the complete CSS for rendering reports,
//! including responsive layouts, dark mode support, and graph styling.
//!
//! # Customization
//!
//! To extend or override styles:
//!
//! ```rust
//! use report_leptos::styles::REPORT_CSS;
//!
//! let my_css = ".custom-class { color: red; }";
//! let combined = format!("{}\n{}", REPORT_CSS, my_css);
//! ```
//!
//! # Features
//!
//! - Vista Galaxy Black Steel Theme (Space/Holographic)
//! - App-like layout (Fixed Sidebar + Scrollable Content)
//! - Monospace typography (JetBrains Mono / Inter)
//! - Responsive tables and graphs
//! - Tab navigation styling
//! - Cytoscape graph container styling

/// Content Security Policy for the report
/// Note: Permissive policy to allow local file:// viewing of reports
pub const CSP: &str = "default-src 'self' file: data: blob:; script-src 'self' 'unsafe-inline' 'unsafe-eval' file: data: blob:; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; font-src 'self' data: https://fonts.gstatic.com; img-src 'self' data: blob: file:; connect-src 'self';";

/// Complete CSS for the report.
pub const REPORT_CSS: &str = r#"
/* ============================================
   Loctree Report — editorial dark, evidence-first
   Aligned with loctree-com /cloud styling discipline:
     warm ink/bone palette, two-color accent system
     (amber narrative, teal interaction), mono-cap
     eyebrows, display-serif titles, semantic status
     tokens. See loctree-com/styles/tokens.css for
     the source-of-truth tokens this mirrors.
   ============================================ */

@import url('https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600&family=Instrument+Serif:ital@0;1&family=JetBrains+Mono:wght@400;500&display=swap');

/* ============================================
   Brand primitives + semantic role tokens
   (mirrors loctree-com/styles/tokens.css)
   ============================================ */
:root {
    /* Brand primitives (warm dark editorial) */
    --report-ink:      #0e0e0e;
    --report-ink2:     #161616;
    --report-ink3:     #1e1e1e;
    --report-bone:        #f5f1e7;
    --report-bone-dim:    rgba(245, 241, 231, 0.86);
    --report-bone-mute:   rgba(245, 241, 231, 0.64);
    --report-bone-faint:  rgba(245, 241, 231, 0.20);

    /* Two-color editorial accent system */
    --report-amber: #c99a3b;  /* narrative accent — hero stat, evidence emphasis */
    --report-teal:  #3d7a72;  /* interaction accent — hover/focus/state */

    /* Semantic status tokens — never reuse for branding */
    --report-status-success: var(--report-teal);
    --report-status-warning: var(--report-amber);
    --report-status-info:    var(--report-bone-dim);
    --report-status-danger:  #b86a5c;

    /* Typography roles */
    --report-font-display: 'Instrument Serif', Georgia, ui-serif, serif;
    --report-font-body:    'Inter', system-ui, -apple-system, sans-serif;
    --report-font-mono:    'JetBrains Mono', ui-monospace, 'SF Mono', Consolas, monospace;

    /* Typography scale */
    --report-type-mono-cap: 0.75rem;
    --report-type-meta:     0.875rem;
    --report-type-body:     clamp(1rem, 0.35vw + 0.94rem, 1.125rem);
    --report-type-h3:       clamp(1.25rem, 1.6vw, 1.5rem);
    --report-type-h2:       clamp(1.875rem, 4vw, 2.625rem);
    --report-type-h1:       clamp(2.25rem, 6vw, 3.75rem);

    --report-tracking-tight: -0.02em;
    --report-tracking-wider: 0.08em;
    --report-tracking-widest: 0.16em;

    --report-lh-tight: 1.15;
    --report-lh-default: 1.55;

    /* Spacing scale (4px base) */
    --report-space-1:  0.25rem;
    --report-space-2:  0.5rem;
    --report-space-3:  0.75rem;
    --report-space-4:  1rem;
    --report-space-5:  1.25rem;
    --report-space-6:  1.5rem;
    --report-space-8:  2rem;
    --report-space-10: 2.5rem;

    /* Existing app-shell still references --theme-* tokens.
       Map them to the new editorial palette so the whole UI
       inherits the warm dark default at zero refactor risk. */
    --theme-bg-deep:           var(--report-ink);
    --theme-bg-surface:        var(--report-ink2);
    --theme-bg-surface-elevated: var(--report-ink3);

    --theme-text-primary:      var(--report-bone);
    --theme-text-secondary:    var(--report-bone-dim);
    --theme-text-tertiary:     var(--report-bone-mute);

    --theme-accent:     var(--report-teal);
    --theme-accent-rgb: 61, 122, 114;

    --theme-border:        var(--report-bone-faint);
    --theme-border-strong: rgba(245, 241, 231, 0.30);

    --theme-hover:         rgba(245, 241, 231, 0.04);
    --theme-hover-strong:  rgba(245, 241, 231, 0.07);

    /* Scrollbar (warm dark) */
    --theme-scrollbar:        rgba(245, 241, 231, 0.12);
    --theme-scrollbar-hover:  rgba(245, 241, 231, 0.22);
    --scrollbar-bg:           var(--theme-scrollbar);
    --scrollbar-bg-hover:     var(--theme-scrollbar-hover);

    /* Gradients (editorial) */
    --gradient-nav:     linear-gradient(135deg, rgba(14,14,14,0.92) 0%, rgba(22,22,22,0.92) 100%);
    --gradient-sidebar: linear-gradient(180deg, rgba(14,14,14,0.95) 0%, rgba(22,22,22,0.92) 100%);
    --gradient-main:    linear-gradient(180deg, rgba(14,14,14,0.94) 0%, rgba(22,22,22,0.90) 55%, rgba(30,30,30,0.86) 100%);

    /* Dimensions */
    --radius-lg: 14px;
    --radius-md: 10px;
    --radius-sm: 6px;

    --sidebar-width: 280px;
    --header-height: 68px;

    --font-sans: var(--report-font-body);
    --font-mono: var(--report-font-mono);

    color-scheme: dark light;
}

/* Tooltip safety layer */
.tooltip-floating {
    z-index: 9999 !important;
}

/* ============================================
   Theme: Dark Mode (default — editorial dark)
   The :root above already maps theme-* to the warm
   editorial palette. .dark stays as an explicit
   re-declaration so the JS toggle still has a
   selector to land on (and so the values are
   discoverable in DevTools).
   ============================================ */
.dark,
html.dark {
    --theme-bg-deep:           var(--report-ink);
    --theme-bg-surface:        var(--report-ink2);
    --theme-bg-surface-elevated: var(--report-ink3);

    --theme-text-primary:      var(--report-bone);
    --theme-text-secondary:    var(--report-bone-dim);
    --theme-text-tertiary:     var(--report-bone-mute);

    --theme-accent:     var(--report-teal);
    --theme-accent-rgb: 61, 122, 114;

    --theme-border:        var(--report-bone-faint);
    --theme-border-strong: rgba(245, 241, 231, 0.30);

    --theme-hover:         rgba(245, 241, 231, 0.04);
    --theme-hover-strong:  rgba(245, 241, 231, 0.07);

    --theme-scrollbar:        rgba(245, 241, 231, 0.12);
    --theme-scrollbar-hover:  rgba(245, 241, 231, 0.22);
    --scrollbar-bg:           var(--theme-scrollbar);
    --scrollbar-bg-hover:     var(--theme-scrollbar-hover);

    --gradient-nav:     linear-gradient(135deg, rgba(14,14,14,0.92) 0%, rgba(22,22,22,0.92) 100%);
    --gradient-sidebar: linear-gradient(180deg, rgba(14,14,14,0.95) 0%, rgba(22,22,22,0.92) 100%);
    --gradient-main:    linear-gradient(180deg, rgba(14,14,14,0.94) 0%, rgba(22,22,22,0.90) 55%, rgba(30,30,30,0.86) 100%);
}

/* ============================================
   Theme: Light Mode — opt-in only via .light
   (warm off-white surface, ink text)
   ============================================ */
.light,
html.light {
    --theme-bg-deep:           #fbfbf8;
    --theme-bg-surface:        #ffffff;
    --theme-bg-surface-elevated: #f5f3ee;

    --theme-text-primary:      #050504;
    --theme-text-secondary:    rgba(5, 5, 4, 0.86);
    --theme-text-tertiary:     rgba(5, 5, 4, 0.60);

    --theme-accent:     #2d5a55;  /* darker teal for AA contrast on light */
    --theme-accent-rgb: 45, 90, 85;

    --theme-border:        rgba(5, 5, 4, 0.12);
    --theme-border-strong: rgba(5, 5, 4, 0.20);

    --theme-hover:         rgba(5, 5, 4, 0.04);
    --theme-hover-strong:  rgba(5, 5, 4, 0.08);

    --theme-scrollbar:        rgba(5, 5, 4, 0.18);
    --theme-scrollbar-hover:  rgba(5, 5, 4, 0.30);
    --scrollbar-bg:           var(--theme-scrollbar);
    --scrollbar-bg-hover:     var(--theme-scrollbar-hover);

    --gradient-nav:     linear-gradient(135deg, rgba(255,255,255,0.96) 0%, rgba(251,251,248,0.96) 100%);
    --gradient-sidebar: linear-gradient(180deg, rgba(255,255,255,0.96) 0%, rgba(251,251,248,0.92) 100%);
    --gradient-main:    linear-gradient(180deg, rgba(255,255,255,0.95) 0%, rgba(251,251,248,0.90) 100%);
}

/* Reset & Base */
*, *::before, *::after { box-sizing: border-box; }

body {
    font-family: var(--font-sans);
    background: var(--theme-bg-deep);
    color: var(--theme-text-primary);
    line-height: 1.5;
    margin: 0;
    height: 100vh;
    overflow: hidden;
    font-size: 13px;
}

a { color: inherit; text-decoration: none; }
code, pre { font-family: var(--font-mono); }

/* Layout Shell */
.app-shell {
    display: flex;
    height: 100vh;
    width: 100vw;
    overflow: hidden;
    background: var(--theme-bg-deep);
}

/* Sidebar */
.app-sidebar {
    width: var(--sidebar-width);
    background: var(--gradient-sidebar);
    border-right: 1px solid var(--theme-border);
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
    z-index: 20;
}

.sidebar-header {
    height: var(--header-height);
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 24px;
    border-bottom: 1px solid var(--theme-border);
}

/* Theme Toggle Button */
.theme-toggle {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    border-radius: var(--radius-sm);
    background: var(--theme-hover);
    border: 1px solid var(--theme-border);
    color: var(--theme-text-secondary);
    cursor: pointer;
    transition: all 0.2s ease;
    flex-shrink: 0;
}

.theme-toggle:hover {
    background: var(--theme-hover-strong);
    color: var(--theme-text-primary);
    border-color: var(--theme-border-strong);
}

/* Test Toggle Button */
.test-toggle-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    width: 100%;
    padding: 8px 12px;
    border-radius: var(--radius-sm);
    background: var(--theme-hover);
    border: 1px solid var(--theme-border);
    color: var(--theme-text-secondary);
    cursor: pointer;
    transition: all 0.2s ease;
    font-size: 12px;
    font-weight: 500;
}

.test-toggle-btn:hover {
    background: var(--theme-hover-strong);
    color: var(--theme-text-primary);
    border-color: var(--theme-border-strong);
}

#test-toggle-icon {
    font-size: 16px;
    transition: opacity 0.2s ease;
}

/* Show sun icon in dark mode, moon icon in light mode */
.theme-icon-light { display: block; }
.theme-icon-dark { display: none; }

.dark .theme-icon-light,
html.dark .theme-icon-light { display: none; }
.dark .theme-icon-dark,
html.dark .theme-icon-dark { display: block; }

@media (prefers-color-scheme: dark) {
    :root:not(.light) .theme-icon-light { display: none; }
    :root:not(.light) .theme-icon-dark { display: block; }
}

.logo-box {
    display: flex;
    align-items: center;
    gap: 10px;
    font-weight: 600;
    font-size: 14px;
    color: var(--theme-text-primary);
    letter-spacing: 0.02em;
}

.logo-img {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.25);
}

.logo-text {
    display: flex;
    flex-direction: column;
    line-height: 1.2;
}

.sidebar-nav {
    flex: 1;
    overflow-y: auto;
    padding: 24px 16px;
    display: flex;
    flex-direction: column;
    gap: 4px;
}

/* Nav items styled like tab buttons - unified design */
.nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 16px;
    border-radius: var(--radius-lg);
    color: var(--theme-text-secondary);
    transition: all 0.2s ease;
    font-size: 13px;
    font-weight: 500;
    border: none;
    background: transparent;
    cursor: pointer;
    text-decoration: none;
    /* Prevent label overflow */
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
}

.nav-item:hover {
    background: var(--theme-hover-strong);
    color: var(--theme-text-primary);
}

.nav-item.active {
    background: var(--theme-bg-surface);
    color: var(--theme-accent);
    box-shadow: 0 1px 3px rgba(0,0,0,0.12);
}

.nav-section-title {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--theme-text-tertiary);
    margin: 24px 14px 8px;
}

/* Main Area */
.app-main {
    flex: 1;
    display: flex;
    flex-direction: column;
    position: relative;
    background: var(--gradient-main);
    min-width: 0; /* Prevent flex overflow */
}

/* Sticky Header — editorial hero rhythm.
   Auto-grows to accommodate the identity badge, eyebrow,
   display title and provenance row introduced by the
   loctree-com styling discipline pass. */
.app-header {
    min-height: var(--header-height);
    flex-shrink: 0;
    background: var(--gradient-nav);
    border-bottom: 1px solid var(--theme-border);
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 24px;
    padding: 18px 32px;
    backdrop-filter: blur(12px);
    z-index: 10;
}

.header-title {
    flex: 1 1 auto;
    min-width: 0;
}

/* Legacy h1 retained as a fallback (some panels still emit a
   plain h1 inside .header-title). The editorial pass renders
   .report-section-title instead, but we keep this for safety. */
.header-title > h1 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
    color: var(--theme-text-primary);
}

.header-stats {
    flex-shrink: 0;
    align-self: flex-start;
}

.header-title p,
.header-path {
    margin: 2px 0 0;
    font-size: 11px;
    color: var(--theme-text-tertiary);
    font-family: var(--font-mono);
    max-width: 300px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

/* Header Stats Badges */
.header-stats {
    display: flex;
    gap: 8px;
}

.stat-badge {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 8px 14px;
    background: var(--theme-bg-surface);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-md);
    min-width: 60px;
}

.stat-badge-value {
    font-size: 16px;
    font-weight: 600;
    color: var(--theme-accent);
    font-family: var(--font-mono);
}

.stat-badge-label {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--theme-text-tertiary);
    margin-top: 2px;
}

/* Tabs */
.header-tabs {
    display: flex;
    gap: 6px;
    background: rgba(0,0,0,0.2);
    padding: 4px;
    border-radius: var(--radius-md);
    border: 1px solid var(--theme-border);
}

.tab-btn {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 16px;
    border-radius: 8px;
    font-size: 12px;
    font-weight: 500;
    color: var(--theme-text-secondary);
    cursor: pointer;
    transition: all 0.2s;
    background: transparent;
    border: none;
    /* Prevent label overflow */
    white-space: nowrap;
    flex-shrink: 0;
}

.tab-btn:hover {
    color: var(--theme-text-primary);
    background: var(--theme-hover);
}

.tab-btn.active {
    background: rgba(163, 184, 199, 0.15);
    color: var(--theme-accent);
    box-shadow: 0 1px 2px rgba(0,0,0,0.2);
}

/* Content Scroll Area */
.app-content {
    flex: 1;
    overflow-y: auto;
    padding: 32px;
    scroll-behavior: smooth;
}

/* Content Panels */
.content-container {
    max-width: 1100px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 24px;
}

.panel {
    background: var(--theme-bg-surface);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-lg);
    padding: 24px;
    box-shadow: 0 4px 20px rgba(0,0,0,0.2);
}

.panel h3 {
    margin-top: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--theme-text-primary);
    margin-bottom: 16px;
    display: flex;
    align-items: center;
    gap: 8px;
}

/* Tables & Lists */
.data-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
}

.data-table th {
    text-align: left;
    color: var(--theme-text-tertiary);
    font-weight: 500;
    padding: 12px 16px;
    border-bottom: 1px solid var(--theme-border);
}

.data-table td {
    padding: 12px 16px;
    border-bottom: 1px solid rgba(114, 124, 139, 0.08);
    color: var(--theme-text-secondary);
}

.data-table tr:last-child td { border-bottom: none; }
.data-table tr:hover td { background: var(--theme-hover); }

code {
    background: rgba(0,0,0,0.2);
    padding: 2px 6px;
    border-radius: 4px;
    color: var(--theme-accent);
    font-size: 0.9em;
}

/* Analysis Summary */
.analysis-summary {
    margin-bottom: 24px;
}

.analysis-summary h3 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 16px;
}

.summary-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(140px, 1fr));
    gap: 16px;
}

.summary-stat {
    background: var(--theme-bg-surface);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-md);
    padding: 16px;
    text-align: center;
}

.dist-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 12px 20px;
    margin: 18px 0 22px;
    color: var(--theme-text-secondary);
    font-size: 13px;
}

.dist-table-stack {
    display: grid;
    gap: 20px;
}

.dist-table-caption {
    margin: 0 0 10px;
    color: var(--theme-text-secondary);
    font-size: 13px;
}

.dist-status-badge {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border-radius: 999px;
    font-family: var(--report-font-mono);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: var(--report-tracking-wider);
    text-transform: uppercase;
    border: 1px solid currentColor;
}

.dist-status-fully-shaken {
    color: var(--report-status-danger);
    background: rgba(184, 106, 92, 0.10);
}

.dist-status-partially-shaken {
    color: var(--report-status-warning);
    background: rgba(201, 154, 59, 0.10);
}

/* ============================================
   Action Plan Panel
   ============================================ */

.action-plan-panel .action-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
}

.action-plan-panel .action-item {
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-md);
    padding: 12px 14px;
    background: var(--theme-bg-surface-elevated);
}

.action-plan-panel .action-head {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    font-family: var(--font-mono);
    font-size: 12px;
}

.action-plan-panel .action-priority {
    background: rgba(var(--theme-accent-rgb), 0.12);
    color: var(--theme-accent);
    padding: 2px 6px;
    border-radius: 4px;
    font-weight: 600;
}

.action-plan-panel .action-kind {
    color: var(--theme-text-tertiary);
}

.action-plan-panel .action-risk {
    padding: 2px 6px;
    border-radius: 4px;
    font-weight: 700;
    text-transform: uppercase;
    font-size: 10px;
    letter-spacing: 0.4px;
}

.action-plan-panel .risk-high {
    background: rgba(184, 106, 92, 0.12);
    color: var(--report-status-danger);
    border: 1px solid currentColor;
}

.action-plan-panel .risk-medium {
    background: rgba(201, 154, 59, 0.12);
    color: var(--report-status-warning);
    border: 1px solid currentColor;
}

.action-plan-panel .risk-low {
    background: rgba(61, 122, 114, 0.12);
    color: var(--report-status-success);
    border: 1px solid currentColor;
}

.action-plan-panel .action-why,
.action-plan-panel .action-fix,
.action-plan-panel .action-verify,
.action-plan-panel .action-location {
    margin-top: 6px;
    font-size: 12px;
    color: var(--theme-text-secondary);
}

.action-plan-panel .action-label {
    display: inline-block;
    min-width: 52px;
    color: var(--theme-text-tertiary);
    font-weight: 700;
    text-transform: uppercase;
    font-size: 10px;
    letter-spacing: 0.3px;
    margin-right: 6px;
}

.action-plan-panel .action-verify code {
    margin-right: 6px;
}

/* ============================================
   Hub Files Panel
   ============================================ */

.hub-files-panel .hub-table code {
    font-size: 11px;
}

.hub-files-panel .hub-table td {
    vertical-align: top;
}

.hub-files-panel .hub-table .copy-btn {
    margin-left: 6px;
}

.stat-value {
    display: block;
    font-size: 28px;
    font-weight: 600;
    color: var(--theme-accent);
    margin-bottom: 4px;
}

.stat-label {
    display: block;
    font-size: 12px;
    color: var(--theme-text-tertiary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
}

/* Command Coverage Summary */
.coverage-summary {
    padding: 12px 16px;
    background: var(--theme-bg-surface-elevated);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-md);
    margin-bottom: 16px;
    font-size: 13px;
}

.text-warning {
    color: var(--report-status-warning);
}

.text-muted {
    color: var(--theme-text-tertiary);
}

/* AI Insights */
.insight-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
}

.insight-item {
    padding: 16px;
    border-radius: var(--radius-md);
    background: var(--theme-hover);
    border: 1px solid var(--theme-border);
    display: flex;
    gap: 12px;
}

.insight-icon { flex-shrink: 0; margin-top: 2px; }
.insight-content strong { display: block; margin-bottom: 4px; color: var(--theme-text-primary); }
.insight-content p { margin: 0; color: var(--theme-text-secondary); line-height: 1.5; }

/* Graph */
.graph-wrapper {
    width: 100%;
    height: calc(100vh - var(--header-height) - 64px);
    background: var(--theme-bg-deep);
    border-radius: var(--radius-lg);
    border: 1px solid var(--theme-border);
    overflow: hidden;
    position: relative;
}

#cy { width: 100%; height: 100%; }

/* Scrollbar - theme-aware */
::-webkit-scrollbar { width: 8px; height: 8px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: var(--theme-scrollbar, rgba(114, 124, 139, 0.2)); border-radius: 4px; }
::-webkit-scrollbar-thumb:hover { background: var(--theme-scrollbar-hover, rgba(114, 124, 139, 0.4)); }

/* Firefox scrollbar */
* {
    scrollbar-width: thin;
    scrollbar-color: var(--theme-scrollbar, rgba(114, 124, 139, 0.2)) transparent;
}

/* Footer */
.app-footer {
    margin-top: auto;
    padding: 24px 16px;
    text-align: center;
    color: var(--theme-text-tertiary);
    font-size: 11px;
    border-top: 1px solid var(--theme-border);
}

/* ============================================
   Section & Tab Visibility (CRITICAL)
   ============================================ */

/* Section views - only show active */
.section-view {
    display: none;
    height: 100%;
    flex-direction: column;
}

.section-view.active {
    display: flex;
}

/* Tab panels - only show active */
.tab-panel {
    display: none;
}

.tab-panel.active {
    display: block;
}

/* Tab bar alias for JS selector */
.tab-bar {
    /* Inherits from .header-tabs */
}

/* ============================================
   Graph Container & Toolbars
   ============================================ */

/* ============================================
   Graph Split Layout (Side-by-Side)
   ============================================ */

.graph-split-container {
    display: flex;
    height: calc(100vh - var(--header-height) - 32px);
    gap: 0;
    position: relative;
}

.graph-left-panel {
    width: 380px;
    min-width: 280px;
    max-width: 600px;
    display: flex;
    flex-direction: column;
    background: var(--theme-bg-surface);
    border-right: 1px solid var(--theme-border);
    overflow: hidden;
}

.graph-left-panel .component-panel {
    flex: 1;
    overflow-y: auto;
    margin: 0;
    border: none;
    border-radius: 0;
}

.graph-left-panel .component-panel-header {
    position: sticky;
    top: 0;
    z-index: 5;
    padding: 8px 10px;
    font-size: 11px;
}

/* Compact table for left panel */
.graph-left-panel .component-panel table {
    font-size: 11px;
}

.graph-left-panel .component-panel th {
    padding: 6px 8px;
    font-size: 10px;
}

.graph-left-panel .component-panel td {
    padding: 4px 8px;
}

.graph-left-panel .component-panel code {
    font-size: 10px;
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    display: inline-block;
}

.graph-left-panel .component-toolbar {
    padding: 6px 10px;
    font-size: 11px;
    flex-wrap: wrap;
    gap: 6px;
}

.graph-left-panel .component-toolbar label {
    font-size: 10px;
}

.graph-left-panel .component-toolbar button {
    padding: 3px 6px;
    font-size: 10px;
}

.graph-left-panel .panel-actions {
    gap: 6px;
}

.graph-left-panel .panel-actions label {
    font-size: 10px;
}

.graph-left-panel .panel-actions input {
    padding: 2px 4px;
    width: 50px !important;
}

.graph-right-panel {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 400px;
    overflow: hidden;
}

.graph-right-panel .graph-toolbar {
    flex-shrink: 0;
    margin: 0;
    border-radius: 0;
    border-left: none;
    border-right: none;
}

.graph-right-panel .graph {
    flex: 1;
    min-height: 0;
    border-radius: 0;
    border: none;
    border-top: 1px solid var(--theme-border);
}

/* Resize handle */
.graph-resize-handle {
    width: 6px;
    cursor: col-resize;
    background: var(--theme-border);
    transition: background 0.15s;
    flex-shrink: 0;
}

.graph-resize-handle:hover,
.graph-resize-handle.active {
    background: var(--theme-accent);
}

/* Graph container - fallback for non-split */
.graph {
    width: 100%;
    height: calc(100vh - var(--header-height) - 200px);
    min-height: 400px;
    background: var(--theme-bg-deep);
    border-radius: var(--radius-md);
    border: 1px solid var(--theme-border);
}

.graph-empty {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 200px;
    color: var(--theme-text-tertiary);
    font-style: italic;
    background: var(--theme-bg-surface);
    border-radius: var(--radius-md);
    border: 1px dashed var(--theme-border);
}

/* Graph toolbars */
.graph-toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    background: var(--theme-bg-surface);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-md);
    margin-bottom: 12px;
    font-size: 12px;
}

.graph-toolbar label {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--theme-text-secondary);
}

.graph-toolbar input[type="text"],
.graph-toolbar input[type="number"],
.graph-toolbar select {
    background: var(--theme-bg-deep);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-sm);
    padding: 4px 8px;
    color: var(--theme-text-primary);
    font-size: 12px;
    font-family: var(--font-mono);
}

.graph-toolbar input[type="checkbox"] {
    accent-color: var(--theme-accent);
}

.graph-toolbar button {
    background: var(--theme-bg-surface-elevated);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-sm);
    padding: 4px 10px;
    color: var(--theme-text-secondary);
    font-size: 11px;
    cursor: pointer;
    transition: all 0.15s ease;
}

.graph-toolbar button:hover {
    background: rgba(163, 184, 199, 0.1);
    color: var(--theme-text-primary);
    border-color: var(--theme-accent);
}

.component-toolbar {
    background: var(--theme-bg-surface-elevated);
}

.graph-controls {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-left: auto;
}

/* Graph legend */
.graph-legend {
    display: flex;
    gap: 16px;
    padding: 8px 0;
    font-size: 11px;
    color: var(--theme-text-tertiary);
}

.graph-legend span {
    display: flex;
    align-items: center;
    gap: 6px;
}

.legend-dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    display: inline-block;
}

/* Graph hint */
.graph-hint {
    padding: 12px 16px;
    background: rgba(163, 184, 199, 0.05);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-md);
    font-size: 12px;
    color: var(--theme-text-tertiary);
    margin-top: 12px;
}

/* ============================================
   Component Panel (Disconnected Components)
   ============================================ */

.component-panel {
    background: var(--theme-bg-surface);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-md);
    margin-bottom: 12px;
    overflow: hidden;
}

.component-panel-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 16px;
    background: var(--theme-bg-surface-elevated);
    border-bottom: 1px solid var(--theme-border);
    font-size: 13px;
}

.component-panel-header strong {
    color: var(--theme-text-primary);
}

.panel-actions {
    display: flex;
    align-items: center;
    gap: 12px;
}

.panel-actions label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--theme-text-secondary);
}

.panel-actions input {
    background: var(--theme-bg-deep);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-sm);
    padding: 4px 8px;
    color: var(--theme-text-primary);
    font-size: 12px;
}

.panel-actions button {
    background: var(--theme-bg-deep);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-sm);
    padding: 4px 10px;
    color: var(--theme-text-secondary);
    font-size: 11px;
    cursor: pointer;
}

.panel-actions button:hover {
    border-color: var(--theme-accent);
    color: var(--theme-text-primary);
}

.component-panel table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
}

.component-panel th {
    text-align: left;
    padding: 10px 16px;
    color: var(--theme-text-tertiary);
    font-weight: 500;
    border-bottom: 1px solid var(--theme-border);
    background: var(--theme-bg-surface);
}

.component-panel td {
    padding: 10px 16px;
    color: var(--theme-text-secondary);
    border-bottom: 1px solid rgba(114, 124, 139, 0.08);
}

.component-panel tr:hover td {
    background: var(--theme-hover);
}

.component-panel button {
    background: transparent;
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-sm);
    padding: 3px 8px;
    color: var(--theme-text-tertiary);
    font-size: 10px;
    cursor: pointer;
}

.component-panel button:hover {
    border-color: var(--theme-accent);
    color: var(--theme-accent);
}

/* ============================================
   Tauri Command Tables
   ============================================ */

.command-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
    margin-top: 16px;
}

.command-table th {
    text-align: left;
    padding: 12px 16px;
    color: var(--theme-text-tertiary);
    font-weight: 500;
    border-bottom: 1px solid var(--theme-border);
}

.command-table td {
    padding: 12px 16px;
    border-bottom: 1px solid rgba(114, 124, 139, 0.08);
    color: var(--theme-text-secondary);
}

.command-pill {
    display: inline-block;
    padding: 2px 8px;
    border-radius: 4px;
    font-family: var(--font-mono);
    font-size: 12px;
    background: rgba(163, 184, 199, 0.1);
    color: var(--theme-accent);
}

/* Module grouping */
.module-group {
    margin-bottom: 24px;
}

.module-header {
    font-size: 13px;
    font-weight: 500;
    color: var(--theme-text-secondary);
    margin-bottom: 12px;
    padding-bottom: 8px;
    border-bottom: 1px solid var(--theme-border);
}

/* FE↔BE Bridge Comparison Table */
.bridge-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
    margin-top: 16px;
    background: var(--theme-bg-surface);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-md);
    overflow: hidden;
}

.bridge-table thead th {
    text-align: left;
    padding: 10px 14px;
    color: var(--theme-text-tertiary);
    font-weight: 500;
    background: var(--theme-bg-surface-elevated);
    border-bottom: 1px solid var(--theme-border);
}

.bridge-table tbody td {
    padding: 8px 14px;
    border-bottom: 1px solid rgba(114, 124, 139, 0.08);
    color: var(--theme-text-secondary);
    vertical-align: top;
}

.bridge-table tbody tr:last-child td {
    border-bottom: none;
}

.bridge-table tbody tr:hover td {
    background: var(--theme-hover);
}

.bridge-table .status-cell {
    font-weight: 500;
    white-space: nowrap;
}

.bridge-table .loc-cell {
    font-family: var(--font-mono);
    font-size: 11px;
    max-width: 300px;
    overflow: hidden;
    text-overflow: ellipsis;
}

.bridge-table .loc-cell a {
    color: var(--theme-accent);
}

.bridge-table .loc-cell a:hover {
    text-decoration: underline;
}

/* Bridge row status colors */
.bridge-table tr.status-ok .status-cell {
    color: var(--report-status-success);
}

.bridge-table tr.status-missing .status-cell {
    color: var(--report-status-warning);
}

.bridge-table tr.status-unused .status-cell {
    color: var(--theme-text-tertiary);
}

.bridge-table tr.status-unregistered .status-cell {
    color: var(--report-status-danger);
}

.bridge-table tr.status-missing {
    background: rgba(230, 126, 34, 0.05);
}

.bridge-table tr.status-unregistered {
    background: rgba(192, 57, 43, 0.05);
}

/* Gap details toggle */
.gap-details {
    margin-top: 24px;
}

.gap-details summary {
    cursor: pointer;
    color: var(--theme-text-tertiary);
    font-size: 12px;
    padding: 8px 0;
}

.gap-details summary:hover {
    color: var(--theme-text-secondary);
}

/* Text success color */
.text-success {
    color: var(--report-status-success);
}

/* ============================================
   Utility Classes
   ============================================ */

.muted {
    color: var(--theme-text-tertiary);
}

.icon-sm {
    width: 16px;
    height: 16px;
    flex-shrink: 0;
}

/* Range slider styling */
input[type="range"] {
    -webkit-appearance: none;
    background: var(--theme-bg-deep);
    border-radius: 4px;
    height: 6px;
    cursor: pointer;
}

input[type="range"]::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 14px;
    height: 14px;
    background: var(--theme-accent);
    border-radius: 50%;
    cursor: pointer;
}

/* ============================================
   Quick Commands Panel (v0.6 features)
   ============================================ */

.quick-commands-panel {
    background: var(--theme-bg-surface);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-lg);
    padding: 20px 24px;
    margin-top: 8px;
}

.quick-commands-panel h3 {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 0 0 16px 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--theme-text-primary);
}

.badge-new {
    font-size: 9px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    background: linear-gradient(135deg, rgba(163, 184, 199, 0.2) 0%, rgba(79, 129, 225, 0.2) 100%);
    color: var(--theme-accent);
    padding: 3px 8px;
    border-radius: 6px;
    border: 1px solid rgba(163, 184, 199, 0.3);
}

.commands-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: 16px;
}

.command-group {
    background: var(--theme-bg-surface-elevated);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-md);
    padding: 16px;
}

.command-group.highlight {
    border-color: rgba(163, 184, 199, 0.3);
    background: linear-gradient(135deg, var(--theme-bg-surface-elevated) 0%, rgba(163, 184, 199, 0.05) 100%);
}

.command-group h4 {
    margin: 0 0 6px 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--theme-text-primary);
}

.command-group .command-desc {
    margin: 0 0 12px 0;
    font-size: 11px;
    color: var(--theme-text-tertiary);
}

.command-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
}

.command-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    background: var(--theme-bg-deep);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-sm);
    font-size: 11px;
}

.command-item:hover {
    border-color: var(--theme-border-strong);
}

.command-code {
    flex: 1;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--theme-accent);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    background: transparent;
    padding: 0;
}

.command-desc-inline {
    font-size: 10px;
    color: var(--theme-text-tertiary);
    white-space: nowrap;
}

.copy-btn {
    flex-shrink: 0;
    background: transparent;
    border: none;
    padding: 2px 4px;
    cursor: pointer;
    font-size: 12px;
    opacity: 0.6;
    transition: opacity 0.15s;
}

.copy-btn:hover {
    opacity: 1;
}

.commands-footer {
    margin-top: 16px;
    padding-top: 12px;
    border-top: 1px solid var(--theme-border);
}

.commands-footer p {
    margin: 0;
    font-size: 11px;
    color: var(--theme-text-tertiary);
}

.commands-footer code {
    font-size: 10px;
    background: var(--theme-bg-deep);
    padding: 2px 6px;
    border-radius: 4px;
    color: var(--theme-accent);
}

/* ============================================
   Tree Component Styles
   ============================================ */

.tree-panel {
    display: flex;
    flex-direction: column;
    gap: 12px;
}

.tree-header {
    display: flex;
    align-items: center;
    gap: 12px;
}

.tree-header h3 {
    margin: 0;
    white-space: nowrap;
}

.tree-stats {
    font-size: 13px;
    color: var(--theme-text-muted);
    padding: 4px 10px;
    background: var(--theme-surface);
    border-radius: 12px;
    border: 1px solid var(--theme-border);
    cursor: help;
}

.tree-controls {
    display: flex;
    gap: 4px;
}

.tree-btn {
    padding: 6px 10px;
    border: 1px solid var(--theme-border);
    border-radius: 6px;
    background: var(--theme-surface);
    color: var(--theme-text);
    cursor: pointer;
    font-size: 14px;
    transition: all 0.15s ease;
}

.tree-btn:hover {
    background: var(--theme-bg-surface-elevated);
    border-color: var(--theme-border-strong);
}

.tree-filter {
    flex: 1;
    padding: 8px 12px;
    border: 1px solid var(--theme-border);
    border-radius: 8px;
    background: var(--theme-surface);
    color: var(--theme-text);
    font-size: 13px;
}

.tree-filter:focus {
    outline: none;
    border-color: var(--theme-accent);
}

.tree-container {
    max-height: calc(100vh - 280px);
    min-height: 400px;
    overflow-y: auto;
    padding-right: 8px;
}

.tree-node {
    font-family: "JetBrains Mono", "SFMono-Regular", monospace;
    font-size: 12px;
}

.tree-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 8px;
    border-radius: 4px;
    cursor: default;
    transition: background 0.1s ease;
}

.tree-row:hover {
    background: var(--theme-bg-surface-elevated);
}

.tree-row-dir {
    cursor: pointer;
}

.tree-row-dir:hover {
    background: rgba(var(--theme-accent-rgb), 0.1);
}

.tree-left {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    flex: 1;
}

.tree-connector {
    color: var(--theme-text-tertiary);
    white-space: pre;
    flex-shrink: 0;
}

.tree-chevron {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    font-size: 10px;
    color: var(--theme-text-secondary);
    transition: transform 0.15s ease;
    flex-shrink: 0;
}

.tree-chevron.collapsed {
    transform: rotate(0deg);
}

.tree-chevron:not(.collapsed) {
    transform: rotate(90deg);
}

.tree-icon {
    flex-shrink: 0;
    font-size: 14px;
}

.tree-path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--theme-text);
}

.tree-highlight {
    background: rgba(255, 200, 0, 0.3);
    color: inherit;
    padding: 0 2px;
    border-radius: 2px;
}

.tree-right {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
    margin-left: 12px;
}

.tree-loc-bar {
    width: 60px;
    height: 4px;
    background: var(--theme-border);
    border-radius: 2px;
    overflow: hidden;
}

.tree-loc-fill {
    height: 100%;
    background: var(--theme-accent);
    border-radius: 2px;
    transition: width 0.2s ease;
}

.tree-loc {
    color: var(--theme-text-tertiary);
    font-size: 11px;
    min-width: 60px;
    text-align: right;
}

.tree-children {
    overflow: hidden;
    transition: max-height 0.2s ease, opacity 0.15s ease;
}

.tree-children.collapsed {
    max-height: 0 !important;
    opacity: 0;
    pointer-events: none;
}

/* ============================================
   Crowds Component Styles
   ============================================ */

.crowds-list {
    display: flex;
    flex-direction: column;
    gap: 20px;
    margin-top: 16px;
}

.crowd-card {
    background: var(--theme-bg-surface-elevated);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-lg);
    padding: 20px;
    transition: border-color 0.2s ease;
}

.crowd-card:hover {
    border-color: var(--theme-border-strong);
}

.crowd-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 16px;
    padding-bottom: 12px;
    border-bottom: 1px solid var(--theme-border);
}

.crowd-pattern {
    display: flex;
    align-items: center;
    gap: 12px;
    flex: 1;
}

.crowd-pattern code {
    font-size: 14px;
    font-weight: 600;
    color: var(--theme-accent);
    background: rgba(163, 184, 199, 0.1);
    padding: 6px 12px;
    border-radius: var(--radius-md);
}

.crowd-member-count {
    font-size: 12px;
}

.crowd-score {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 8px 16px;
    background: var(--theme-bg-deep);
    border-radius: var(--radius-md);
    border: 2px solid var(--score-color, var(--theme-border));
    min-width: 80px;
}

.score-value {
    font-size: 24px;
    font-weight: 700;
    font-family: var(--font-mono);
    color: var(--score-color, var(--theme-text-primary));
    line-height: 1;
}

.score-label {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--theme-text-tertiary);
    margin-top: 4px;
}

.crowd-issues {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-bottom: 16px;
}

.issue-badge {
    display: inline-block;
    padding: 6px 12px;
    border-radius: var(--radius-sm);
    font-size: 11px;
    font-weight: 500;
    border: 1px solid;
}

.issue-critical {
    background: rgba(192, 57, 43, 0.1);
    border-color: rgba(192, 57, 43, 0.3);
    color: var(--report-status-danger);
}

.issue-warning {
    background: rgba(230, 126, 34, 0.1);
    border-color: rgba(230, 126, 34, 0.3);
    color: var(--report-status-warning);
}

.issue-info {
    background: rgba(49, 130, 206, 0.1);
    border-color: rgba(49, 130, 206, 0.3);
    color: var(--theme-accent);
}

.crowd-members {
    margin-top: 12px;
}

.crowd-members .data-table {
    font-size: 12px;
}

.crowd-members .data-table th {
    padding: 8px 12px;
    font-size: 11px;
}

.crowd-members .data-table td {
    padding: 8px 12px;
}

.file-path {
    max-width: 400px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    display: inline-block;
}

/* ============================================
   Dead Code Component Styles
   ============================================ */

.dead-code-summary {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 16px;
    background: var(--theme-bg-surface-elevated);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-md);
    margin-bottom: 16px;
}

.filter-toggle {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--theme-text-secondary);
    cursor: pointer;
}

.filter-toggle input[type="checkbox"] {
    accent-color: var(--theme-accent);
    cursor: pointer;
}

.dead-exports-table {
    font-size: 13px;
}

.dead-exports-table .file-cell code,
.dead-exports-table .symbol-cell code {
    font-family: var(--font-mono);
    font-size: 12px;
}

.dead-exports-table .file-cell a {
    color: var(--theme-accent);
    text-decoration: none;
}

.dead-exports-table .file-cell a:hover {
    text-decoration: underline;
}

.dead-exports-table .line-cell {
    font-family: var(--font-mono);
    font-size: 11px;
    text-align: center;
    color: var(--theme-text-tertiary);
}

.dead-exports-table .confidence-cell {
    text-align: center;
}

.confidence-badge {
    display: inline-block;
    padding: 4px 10px;
    border-radius: var(--radius-sm);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
}

.confidence-badge.confidence-very-high {
    background: rgba(192, 57, 43, 0.15);
    color: var(--report-status-danger);
    border: 1px solid rgba(192, 57, 43, 0.3);
}

.confidence-badge.confidence-high {
    background: rgba(230, 126, 34, 0.15);
    color: var(--report-status-warning);
    border: 1px solid rgba(230, 126, 34, 0.3);
}

.confidence-badge.confidence-medium {
    background: rgba(49, 130, 206, 0.15);
    color: var(--theme-accent);
    border: 1px solid rgba(49, 130, 206, 0.3);
}

.dead-exports-table .reason-cell {
    font-size: 12px;
    max-width: 300px;
    color: var(--theme-text-secondary);
}

.dead-exports-table tr.confidence-very-high {
    background: rgba(192, 57, 43, 0.03);
}

.dead-exports-table tr.confidence-high {
    background: rgba(230, 126, 34, 0.03);
}

.dead-exports-table tr:hover {
    background: var(--theme-hover-strong) !important;
}

/* ============================================
   Cycles Component
   ============================================ */

/* Count badges */
.count-badge {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 24px;
    height: 20px;
    padding: 0 8px;
    border-radius: 10px;
    font-size: 11px;
    font-weight: 600;
    font-family: var(--font-mono);
    margin-left: auto;
}

.count-badge-success {
    background: rgba(39, 174, 96, 0.15);
    color: var(--report-status-success);
    border: 1px solid rgba(39, 174, 96, 0.3);
}

.count-badge-warning {
    background: rgba(230, 126, 34, 0.15);
    color: var(--report-status-warning);
    border: 1px solid rgba(230, 126, 34, 0.3);
}

.count-badge-critical {
    background: rgba(192, 57, 43, 0.15);
    color: var(--report-status-danger);
    border: 1px solid rgba(192, 57, 43, 0.3);
}

/* Empty state */
.cycles-empty {
    padding: 32px;
    text-align: center;
    background: rgba(39, 174, 96, 0.05);
    border-radius: var(--radius-md);
    border: 1px dashed rgba(39, 174, 96, 0.3);
}

.cycles-empty p {
    color: var(--report-status-success);
    font-size: 13px;
    margin: 0;
}

/* Cycles section */
.cycles-section {
    margin-bottom: 24px;
    padding: 20px;
    border-radius: var(--radius-md);
    border: 1px solid var(--theme-border);
}

.cycles-section-strict {
    background: rgba(192, 57, 43, 0.05);
    border-color: rgba(192, 57, 43, 0.2);
}

.cycles-section-lazy {
    background: rgba(230, 126, 34, 0.05);
    border-color: rgba(230, 126, 34, 0.2);
}

.cycles-section-header {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 12px;
}

.cycles-section-header h4 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--theme-text-primary);
}

.cycles-section-desc {
    font-size: 12px;
    color: var(--theme-text-secondary);
    margin: 0 0 16px 0;
    padding-left: 30px;
}

/* Cycles list */
.cycles-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
}

/* Individual cycle item */
.cycle-item {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    background: var(--theme-bg-surface);
    border-radius: var(--radius-md);
    border: 1px solid var(--theme-border);
}

.cycle-item-strict {
    border-left: 3px solid var(--report-status-danger);
}

.cycle-item-lazy {
    border-left: 3px solid var(--report-status-warning);
}

.cycle-number {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 32px;
    height: 24px;
    padding: 0 8px;
    background: var(--theme-hover);
    border: 1px solid var(--theme-border);
    border-radius: 6px;
    font-size: 11px;
    font-weight: 600;
    font-family: var(--font-mono);
    color: var(--theme-text-tertiary);
    flex-shrink: 0;
}

.cycle-path {
    flex: 1;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--theme-text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    background: rgba(0, 0, 0, 0.2);
    padding: 4px 8px;
    border-radius: 4px;
}

/* ============================================
   Pipeline Component Styles
   ============================================ */

.pipelines-panel {
    max-width: 100%;
}

.pipelines-summary {
    margin-bottom: 16px;
}

.pipeline-stats {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
}

.stat-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    border-radius: 12px;
    font-size: 12px;
    font-weight: 500;
}

.stat-total {
    background: var(--theme-bg-surface-elevated);
    color: var(--theme-text-secondary);
    border: 1px solid var(--theme-border);
}

.stat-ok {
    background: rgba(39, 174, 96, 0.15);
    color: var(--report-status-success);
}

.stat-missing {
    background: rgba(231, 76, 60, 0.15);
    color: #e74c3c;
}

.stat-unused {
    background: rgba(149, 165, 166, 0.2);
    color: #95a5a6;
}

.stat-unreg {
    background: rgba(230, 126, 34, 0.15);
    color: var(--report-status-warning);
}

.pipelines-filters {
    display: flex;
    justify-content: space-between;
    align-items: center;
    flex-wrap: wrap;
    gap: 12px;
    margin-bottom: 20px;
    padding: 12px 16px;
    background: var(--theme-bg-surface);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-md);
}

.filter-buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
}

.filter-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-sm);
    background: var(--theme-bg-surface);
    color: var(--theme-text-secondary);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
}

.filter-btn:hover {
    background: var(--theme-bg-surface-elevated);
    border-color: var(--theme-primary);
}

.filter-btn.active {
    background: var(--theme-primary);
    border-color: var(--theme-primary);
    color: white;
}

.filter-count {
    padding: 2px 6px;
    background: rgba(255, 255, 255, 0.2);
    border-radius: 8px;
    font-size: 10px;
}

.filter-btn:not(.active) .filter-count {
    background: rgba(0, 0, 0, 0.1);
}

.search-box {
    flex: 0 0 auto;
}

.search-input {
    padding: 8px 12px;
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-sm);
    background: var(--theme-bg-surface);
    color: var(--theme-text-primary);
    font-size: 13px;
    min-width: 200px;
}

.search-input:focus {
    outline: none;
    border-color: var(--theme-primary);
    box-shadow: 0 0 0 2px rgba(52, 152, 219, 0.2);
}

.search-input::placeholder {
    color: var(--theme-text-tertiary);
}

.no-results {
    text-align: center;
    padding: 32px;
}

.cards-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 16px;
}

.pipeline-card {
    background: var(--theme-bg-surface);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-lg);
    overflow: hidden;
    transition: all 0.2s ease;
}

.pipeline-card:hover {
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.1);
}

.pipeline-card.status-ok {
    border-left: 3px solid var(--report-status-success);
}

.pipeline-card.status-missing {
    border-left: 3px solid #e74c3c;
}

.pipeline-card.status-unused {
    border-left: 3px solid #95a5a6;
}

.pipeline-card.status-unreg {
    border-left: 3px solid var(--report-status-warning);
}

.card-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 16px;
    cursor: pointer;
    background: var(--theme-bg-surface-elevated);
}

.card-header:hover {
    background: var(--theme-bg-hover);
}

.card-title {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
}

.command-name {
    font-family: var(--font-mono);
    font-size: 14px;
    font-weight: 600;
    color: var(--theme-text-primary);
}

.status-badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 3px 8px;
    border-radius: 10px;
    font-size: 11px;
    font-weight: 500;
}

.status-badge.status-ok {
    background: rgba(39, 174, 96, 0.15);
    color: var(--report-status-success);
}

.status-badge.status-missing {
    background: rgba(231, 76, 60, 0.15);
    color: #e74c3c;
}

.status-badge.status-unused {
    background: rgba(149, 165, 166, 0.2);
    color: #7f8c8d;
}

.status-badge.status-unreg {
    background: rgba(230, 126, 34, 0.15);
    color: var(--report-status-warning);
}

.expand-icon {
    color: var(--theme-text-tertiary);
    font-size: 12px;
    transition: transform 0.2s ease;
}

/* Chain Visualization */
.chain-viz {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 16px;
    background: var(--theme-bg-surface);
}

.chain-node {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 8px 16px;
    border-radius: var(--radius-md);
    min-width: 80px;
    transition: all 0.2s ease;
}

.chain-node.active {
    background: var(--theme-bg-surface-elevated);
    border: 1px solid var(--theme-border);
}

.chain-node.inactive {
    background: rgba(0, 0, 0, 0.05);
    border: 1px dashed var(--theme-border);
    opacity: 0.6;
}

.chain-node.fe.active {
    border-color: #3498db;
    background: rgba(52, 152, 219, 0.1);
}

.chain-node.be.active {
    border-color: #9b59b6;
    background: rgba(155, 89, 182, 0.1);
}

.node-icon {
    font-size: 12px;
    font-weight: 700;
    padding: 4px 8px;
    border-radius: 4px;
    background: rgba(0, 0, 0, 0.1);
}

.chain-node.fe .node-icon {
    background: #3498db;
    color: white;
}

.chain-node.be .node-icon {
    background: #9b59b6;
    color: white;
}

.chain-node.inactive .node-icon {
    background: var(--theme-text-tertiary);
    color: white;
}

.node-label {
    font-size: 11px;
    color: var(--theme-text-secondary);
    text-align: center;
}

.chain-arrow {
    display: flex;
    align-items: center;
    color: var(--theme-border);
    font-size: 14px;
}

.chain-arrow.active {
    color: var(--theme-primary);
}

.arrow-line {
    display: block;
    width: 20px;
    height: 2px;
    background: currentColor;
}

.arrow-head {
    font-weight: bold;
}

/* Card Details (Expanded) */
.card-details {
    padding: 16px;
    border-top: 1px solid var(--theme-border);
    background: var(--theme-bg-surface);
}

.detail-section {
    margin-bottom: 16px;
}

.detail-section:last-child {
    margin-bottom: 0;
}

.detail-section h4 {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    font-weight: 600;
    color: var(--theme-text-secondary);
    margin-bottom: 8px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
}

.location-list {
    list-style: none;
    padding: 0;
    margin: 0;
}

.location-list li {
    padding: 6px 10px;
    background: var(--theme-bg-surface-elevated);
    border-radius: var(--radius-sm);
    margin-bottom: 4px;
    font-size: 12px;
    display: flex;
    align-items: center;
    gap: 4px;
}

.location-list .file-path {
    font-family: var(--font-mono);
    color: var(--theme-primary);
}

.location-list .line-num {
    color: var(--theme-text-tertiary);
    font-family: var(--font-mono);
}

.location-list .impl-name {
    color: var(--theme-text-tertiary);
    font-size: 11px;
}

.card-details .warning {
    color: var(--report-status-warning);
    font-size: 12px;
}

/* ============================================
   Split Panel View (FE/BE Side-by-Side)
   ============================================ */

.split-panel-container {
    display: grid;
    grid-template-columns: 1fr 80px 1fr;
    gap: 0;
    min-height: 400px;
    margin-top: 16px;
}

.split-panel {
    background: var(--theme-bg-surface);
    border-radius: var(--radius-md);
    padding: 16px;
    overflow-y: auto;
    max-height: 600px;
    border: 1px solid var(--theme-border);
}

.split-panel h4 {
    margin: 0 0 12px 0;
    font-size: 14px;
    color: var(--theme-text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
}

.panel-items {
    display: flex;
    flex-direction: column;
    gap: 8px;
}

.split-item {
    background: var(--theme-bg-surface-elevated);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-sm);
    padding: 10px 12px;
    cursor: pointer;
    transition: border-color 0.2s;
}

.split-item:hover {
    border-color: var(--theme-accent);
}

.split-item.status-ok {
    border-left: 3px solid var(--report-status-success);
}

.split-item.status-missing {
    border-left: 3px solid #e74c3c;
}

.split-item.status-unused {
    border-left: 3px solid #95a5a6;
}

.split-item-name {
    font-family: var(--font-mono);
    font-size: 13px;
    font-weight: 600;
    color: var(--theme-text-primary);
}

.split-item-location {
    font-size: 11px;
    color: var(--theme-text-tertiary);
    margin-top: 4px;
}

.connection-svg {
    width: 80px;
    height: 100%;
    min-height: 400px;
}

.connection-line {
    stroke: var(--theme-accent);
    stroke-width: 2;
    fill: none;
}

.connection-line.missing {
    stroke: #e74c3c;
    stroke-dasharray: 4;
}

/* Split panel specific styles */
.split-panel-fe {
    border-right: none;
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
}

.split-panel-be {
    border-left: none;
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
}

.split-panel-connections {
    display: flex;
    align-items: stretch;
    background: var(--theme-bg-surface);
    border-top: 1px solid var(--theme-border);
    border-bottom: 1px solid var(--theme-border);
}

.split-panel h4 {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-bottom: 12px;
    border-bottom: 1px solid var(--theme-border);
}

.split-item-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
}

.split-item-status {
    display: flex;
    align-items: center;
}

.split-item-status.status-ok {
    color: var(--report-status-success);
}

.split-item-status.status-missing {
    color: #e74c3c;
}

.split-item-status.status-unused {
    color: #95a5a6;
}

.split-item-status.status-unreg {
    color: var(--report-status-warning);
}

.split-item.status-unreg {
    border-left: 3px solid var(--report-status-warning);
}

.split-item-placeholder {
    background: rgba(231, 76, 60, 0.05);
    border-style: dashed;
}

.split-item-placeholder .split-item-name {
    display: flex;
    align-items: center;
    gap: 6px;
    color: #e74c3c;
}

.split-item-location a {
    color: var(--theme-accent);
}

.split-item-location a:hover {
    text-decoration: underline;
}

/* View toggle buttons */
.view-toggle {
    display: flex;
    gap: 4px;
    margin-left: auto;
}

.view-btn {
    padding: 6px 10px;
    border: 1px solid var(--theme-border);
    background: var(--theme-bg-surface);
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-size: 14px;
    color: var(--theme-text-secondary);
    transition: all 0.15s ease;
}

.view-btn:hover {
    background: var(--theme-bg-surface-elevated);
    border-color: var(--theme-border-strong);
}

.view-btn.active {
    background: var(--theme-accent);
    color: white;
    border-color: var(--theme-accent);
}

/* Communication type badge */
.comm-badge {
    font-size: 11px;
    padding: 2px 6px;
    border-radius: 3px;
    margin-left: 8px;
}

.comm-badge.comm-emit {
    background: rgba(155, 89, 182, 0.2);
    color: #9b59b6;
}

.comm-badge.comm-invoke {
    background: rgba(52, 152, 219, 0.2);
    color: #3498db;
}

/* ============================================
   Health Score Gauge
   ============================================ */

/* Overview Hero - Health Gauge + Summary side by side */
.overview-hero {
    display: flex;
    align-items: flex-start;
    gap: 32px;
    padding: 24px;
    background: var(--theme-bg-surface);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-lg);
    box-shadow: 0 4px 20px rgba(0,0,0,0.2);
}

.overview-hero > .health-gauge {
    flex-shrink: 0;
}

.overview-summary-wrapper {
    flex: 1;
    min-width: 0;
}

.overview-summary-wrapper .analysis-summary {
    margin-bottom: 0;
}

@media (max-width: 768px) {
    .overview-hero {
        flex-direction: column;
        align-items: center;
        gap: 20px;
    }

    .overview-summary-wrapper {
        width: 100%;
    }
}

.health-gauge {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 16px;
}

.gauge-svg {
    display: block;
}

.gauge-progress {
    transition: stroke-dashoffset 0.6s ease-out;
}

.gauge-status {
    font-size: 13px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
}

/* Compact inline health indicator */
.health-indicator {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-family: var(--font-mono);
    font-size: 12px;
}

/* ============================================
   Audit Panel Component
   ============================================ */

.audit-panel {
    background: var(--theme-bg-surface);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-lg);
    padding: 24px;
}

.audit-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 24px;
    padding-bottom: 16px;
    border-bottom: 1px solid var(--theme-border);
}

.audit-header h3 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    color: var(--theme-text-primary);
}

.health-badge {
    display: flex;
    align-items: baseline;
    gap: 2px;
    padding: 8px 16px;
    border-radius: var(--radius-md);
    font-family: var(--font-mono);
    font-weight: 700;
    border: 2px solid;
}

.health-badge.critical {
    background: rgba(192, 57, 43, 0.1);
    border-color: rgba(192, 57, 43, 0.5);
    color: var(--report-status-danger);
}

.health-badge.warning {
    background: rgba(230, 126, 34, 0.1);
    border-color: rgba(230, 126, 34, 0.5);
    color: var(--report-status-warning);
}

.health-badge.good {
    background: rgba(39, 174, 96, 0.1);
    border-color: rgba(39, 174, 96, 0.5);
    color: var(--report-status-success);
}

.health-value {
    font-size: 24px;
}

.health-max {
    font-size: 14px;
    opacity: 0.6;
}

/* Audit sections */
.audit-section {
    margin-bottom: 24px;
    padding: 16px;
    border-radius: var(--radius-md);
    border: 1px solid var(--theme-border);
}

.audit-section:last-of-type {
    margin-bottom: 16px;
}

.audit-critical {
    background: rgba(192, 57, 43, 0.05);
    border-color: rgba(192, 57, 43, 0.2);
}

.audit-warning {
    background: rgba(230, 126, 34, 0.05);
    border-color: rgba(230, 126, 34, 0.2);
}

.audit-quick-wins {
    background: rgba(39, 174, 96, 0.05);
    border-color: rgba(39, 174, 96, 0.2);
}

.audit-section-title {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 8px 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--theme-text-primary);
}

.audit-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: 4px;
    font-size: 11px;
    font-weight: 700;
    font-family: var(--font-mono);
}

.audit-critical .audit-icon {
    background: rgba(192, 57, 43, 0.2);
    color: var(--report-status-danger);
}

.audit-warning .audit-icon {
    background: rgba(230, 126, 34, 0.2);
    color: var(--report-status-warning);
}

.audit-quick-wins .audit-icon {
    background: rgba(39, 174, 96, 0.2);
    color: var(--report-status-success);
}

.audit-section-desc {
    margin: 0 0 12px 0;
    font-size: 12px;
    color: var(--theme-text-tertiary);
}

/* Audit list */
.audit-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
}

.audit-item {
    padding: 8px 12px;
    background: var(--theme-bg-surface);
    border-radius: var(--radius-sm);
    border: 1px solid var(--theme-border);
    font-size: 13px;
    transition: background 0.15s ease;
}

.audit-item:hover {
    background: var(--theme-bg-surface-elevated);
}

.audit-checkbox-label {
    display: flex;
    align-items: center;
    gap: 10px;
    cursor: pointer;
}

.audit-checkbox {
    flex-shrink: 0;
    width: 16px;
    height: 16px;
    accent-color: var(--theme-accent);
    cursor: pointer;
}

.audit-checkbox:checked + .audit-symbol,
.audit-checkbox:checked + .audit-cycle,
.audit-checkbox:checked ~ span {
    text-decoration: line-through;
    opacity: 0.5;
}

.audit-symbol {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--theme-accent);
    background: rgba(163, 184, 199, 0.1);
    padding: 2px 6px;
    border-radius: 4px;
}

.audit-cycle {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--theme-text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 500px;
}

.audit-location {
    font-size: 11px;
    color: var(--theme-text-tertiary);
    font-family: var(--font-mono);
}

.audit-category {
    background: transparent;
    border: none;
    padding: 4px 0;
    font-weight: 600;
}

.audit-category-icon {
    color: var(--theme-text-secondary);
}

.audit-count {
    font-family: var(--font-mono);
    color: var(--theme-accent);
}

.audit-sub-item {
    margin-left: 20px;
}

.audit-more {
    font-style: italic;
    color: var(--theme-text-tertiary);
    background: transparent;
    border: none;
}

/* Quick win categories */
.audit-category-cleanup {
    color: var(--theme-text-secondary);
}

.audit-category-refactor {
    color: #3498db;
}

.audit-category-optimize {
    color: #9b59b6;
}

.audit-category-test {
    color: var(--report-status-warning);
}

/* Empty state */
.audit-empty {
    padding: 32px;
    text-align: center;
    background: rgba(39, 174, 96, 0.05);
    border-radius: var(--radius-md);
    border: 1px dashed rgba(39, 174, 96, 0.3);
}

.audit-empty p {
    margin: 0;
    color: var(--report-status-success);
    font-size: 14px;
}

/* Footer */
.audit-footer {
    margin-top: 16px;
    padding-top: 16px;
    border-top: 1px solid var(--theme-border);
}

.audit-tip {
    margin: 0;
    font-size: 12px;
    color: var(--theme-text-tertiary);
}

.audit-tip code {
    font-size: 11px;
    background: var(--theme-bg-deep);
    padding: 2px 6px;
    border-radius: 4px;
    color: var(--theme-accent);
}

/* ============================================
   Refactor Plan Panel
   𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders
   ============================================ */

.refactor-plan-panel {
    display: flex;
    flex-direction: column;
    gap: 20px;
}

.refactor-summary {
    display: flex;
    flex-direction: column;
    gap: 16px;
}

.refactor-summary h3 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    font-size: 16px;
    color: var(--theme-text-primary);
}

.refactor-stats-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 16px;
}

.refactor-stats-grid .stat-item {
    text-align: center;
    padding: 12px;
    background: var(--theme-hover);
    border-radius: var(--radius-md);
}

.refactor-stats-grid .stat-value {
    font-size: 24px;
    font-weight: 600;
    color: var(--theme-accent);
    display: block;
}

.refactor-stats-grid .stat-label {
    font-size: 11px;
    color: var(--theme-text-tertiary);
    text-transform: uppercase;
}

.risk-badges {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
}

.risk-badge {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    border-radius: var(--radius-sm);
    font-size: 12px;
    font-weight: 500;
}

.risk-badge.risk-low {
    background: rgba(34, 197, 94, 0.15);
    color: var(--report-status-success);
}

.risk-badge.risk-medium {
    background: rgba(234, 179, 8, 0.15);
    color: #eab308;
}

.risk-badge.risk-high {
    background: rgba(239, 68, 68, 0.15);
    color: var(--report-status-danger);
}

/* Layer Distribution */
.layer-distribution {
    padding: 16px;
}

.layer-distribution h4 {
    margin: 0 0 16px 0;
    font-size: 14px;
    color: var(--theme-text-secondary);
}

.distribution-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 24px;
}

.distribution-column h5 {
    margin: 0 0 12px 0;
    font-size: 12px;
    color: var(--theme-text-secondary);
    text-transform: uppercase;
}

.layer-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
}

.layer-name {
    width: 70px;
    font-size: 11px;
    color: var(--theme-text-secondary);
    text-transform: capitalize;
}

.bar-track {
    flex: 1;
    height: 8px;
    background: var(--theme-hover);
    border-radius: 4px;
    overflow: hidden;
}

.bar-fill {
    height: 100%;
    border-radius: 4px;
    transition: width 0.3s ease;
}

.bar-fill.before {
    background: var(--theme-text-tertiary);
}

.bar-fill.after {
    background: var(--theme-accent);
}

.layer-count {
    width: 30px;
    text-align: right;
    font-size: 11px;
    color: var(--theme-text-tertiary);
    font-family: var(--font-mono);
}

/* Cyclic Warning */
.cyclic-warning {
    background: rgba(234, 179, 8, 0.08);
    border: 1px solid rgba(234, 179, 8, 0.3);
    border-radius: var(--radius-md);
    padding: 16px;
}

.cyclic-warning h4 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 8px 0;
    color: #eab308;
    font-size: 14px;
}

.cycle-group {
    margin: 12px 0;
    padding-left: 16px;
}

.cycle-group strong {
    font-size: 12px;
    color: var(--theme-text-secondary);
}

.cycle-group ul {
    margin: 4px 0;
    padding-left: 20px;
}

.cycle-group li {
    font-size: 12px;
    margin: 2px 0;
}

.cycle-group code {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--theme-text-primary);
}

/* Execution Phases */
.execution-phases {
    display: flex;
    flex-direction: column;
    gap: 16px;
}

.phase-card {
    border-radius: var(--radius-md);
    border: 1px solid var(--theme-border);
    overflow: hidden;
    background: var(--theme-bg-surface);
}

.phase-card.risk-low {
    border-left: 4px solid var(--report-status-success);
}

.phase-card.risk-medium {
    border-left: 4px solid #eab308;
}

.phase-card.risk-high {
    border-left: 4px solid var(--report-status-danger);
}

.phase-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px;
    background: var(--theme-hover);
    cursor: pointer;
    user-select: none;
}

.phase-header:hover {
    background: var(--theme-hover-strong);
}

.phase-toggle {
    font-size: 10px;
    color: var(--theme-text-tertiary);
    transition: transform 0.2s ease;
}

.phase-card.collapsed .phase-toggle {
    transform: rotate(-90deg);
}

.phase-card.collapsed .phase-content {
    display: none;
}

.phase-icon {
    display: flex;
    align-items: center;
}

.phase-name {
    font-weight: 500;
    color: var(--theme-text-primary);
}

.phase-count {
    color: var(--theme-text-tertiary);
    font-size: 12px;
    margin-left: auto;
}

.phase-content {
    padding: 16px;
}

.moves-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
}

.moves-table th {
    text-align: left;
    padding: 8px;
    background: var(--theme-hover);
    font-weight: 500;
    color: var(--theme-text-secondary);
    font-size: 11px;
    text-transform: uppercase;
}

.moves-table td {
    padding: 8px;
    border-bottom: 1px solid var(--theme-border);
    color: var(--theme-text-primary);
}

.moves-table code {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--theme-primary);
}

.moves-table tr:last-child td {
    border-bottom: none;
}

.phase-commands {
    margin-top: 16px;
    padding-top: 16px;
    border-top: 1px solid var(--theme-border);
}

.phase-commands strong {
    font-size: 12px;
    color: var(--theme-text-secondary);
}

.phase-commands pre {
    margin: 8px 0;
    padding: 12px;
    background: var(--theme-bg-deep);
    border-radius: var(--radius-sm);
    overflow-x: auto;
    font-size: 11px;
}

.phase-commands code {
    font-family: var(--font-mono);
    color: var(--theme-text-primary);
}

/* Shimming Strategy */
.shimming-strategy {
    padding: 16px;
}

.shimming-strategy h4 {
    margin: 0 0 8px 0;
    font-size: 14px;
    color: var(--theme-text-secondary);
}

.shim-item {
    margin: 16px 0;
    padding: 12px;
    background: var(--theme-hover);
    border-radius: var(--radius-sm);
}

.shim-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 8px;
}

.shim-header code {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--theme-primary);
}

.shim-code {
    margin: 8px 0;
    padding: 12px;
    background: var(--theme-bg-deep);
    border-radius: var(--radius-sm);
    font-size: 11px;
    overflow-x: auto;
}

.shim-code code {
    font-family: var(--font-mono);
    color: var(--theme-text-primary);
}

/* Empty State */
.refactor-empty {
    text-align: center;
    padding: 40px;
    color: var(--theme-text-tertiary);
}

.refactor-empty p {
    margin: 0;
    font-size: 14px;
}

.refactor-empty code {
    display: inline-block;
    margin: 8px 0;
    padding: 4px 8px;
    background: var(--theme-hover);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--theme-accent);
}

/* ============================================
   Responsive
   ============================================ */

@media (max-width: 900px) {
    .app-shell {
        flex-direction: column;
    }
    
    .app-sidebar {
        width: 100%;
        height: auto;
        border-right: none;
        border-bottom: 1px solid var(--theme-border);
    }
    
    .sidebar-nav {
        flex-direction: row;
        overflow-x: auto;
        padding: 12px;
    }
    
    .nav-section-title {
        display: none;
    }
    
    .app-footer {
        display: none;
    }
    
    .header-tabs {
        flex-wrap: wrap;
    }
    
    .graph-toolbar {
        flex-direction: column;
        align-items: stretch;
    }
    
    .graph-controls {
        margin-left: 0;
        justify-content: center;
    }
}

/* ============================================
   Editorial polish layer
   New shared classes that follow loctree-com
   /cloud discipline. Existing components keep
   their current styling — these classes add
   coherent hero/eyebrow/title/footer treatments
   for the report shell.
   ============================================ */

/* Mono-cap eyebrow (section labels, hero kickers, meta) */
.report-eyebrow {
    font-family: var(--report-font-mono);
    font-size: var(--report-type-mono-cap);
    text-transform: uppercase;
    letter-spacing: var(--report-tracking-widest);
    color: var(--theme-text-tertiary);
    font-weight: 500;
    line-height: var(--report-lh-tight);
    margin: 0 0 var(--report-space-3);
}

/* Display-serif title (hero, section header) */
.report-title-display {
    font-family: var(--report-font-display);
    font-weight: 400;
    font-style: normal;
    line-height: var(--report-lh-tight);
    letter-spacing: var(--report-tracking-tight);
    color: var(--theme-text-primary);
    margin: 0;
}

.report-title-display.size-h1 { font-size: var(--report-type-h1); }
.report-title-display.size-h2 { font-size: var(--report-type-h2); }
.report-title-display.size-h3 { font-size: var(--report-type-h3); font-family: var(--report-font-body); font-weight: 600; }

/* Section header rhythm — eyebrow + display title + supporting body */
.report-section-header {
    margin-bottom: var(--report-space-6);
}

.report-section-header .report-section-body {
    font-family: var(--report-font-body);
    font-size: var(--report-type-body);
    line-height: var(--report-lh-default);
    color: var(--theme-text-secondary);
    margin: var(--report-space-3) 0 0;
    max-width: 70ch;
}

/* Sticky header (in-report) — fortified hero wrapper */
.report-sticky-hero {
    display: flex;
    flex-direction: column;
    gap: var(--report-space-1);
    flex: 1 1 auto;
    min-width: 0;
}

.report-sticky-hero .report-eyebrow {
    margin-bottom: 0;
}

.report-sticky-hero .report-section-title {
    font-family: var(--report-font-display);
    font-size: clamp(1.25rem, 1.6vw, 1.75rem);
    font-weight: 400;
    line-height: var(--report-lh-tight);
    letter-spacing: var(--report-tracking-tight);
    color: var(--theme-text-primary);
    margin: 0;
}

.report-sticky-hero .report-meta-row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--report-space-1) var(--report-space-3);
    margin-top: 4px;
    font-family: var(--report-font-mono);
    font-size: 11px;
    color: var(--theme-text-tertiary);
}

.report-meta-row .report-meta {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 360px;
}

.report-meta-row .report-meta-label {
    text-transform: uppercase;
    letter-spacing: var(--report-tracking-wider);
    color: var(--theme-text-tertiary);
    opacity: 0.65;
}

.report-meta-row .report-meta-value {
    color: var(--theme-text-secondary);
}

/* Long-path treatment — wraps without breaking the layout */
.report-path-wrap {
    font-family: var(--report-font-mono);
    word-break: break-all;
    overflow-wrap: anywhere;
    color: var(--theme-text-tertiary);
}

/* Status semantic helpers (use these instead of raw hex) */
.report-status-dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 999px;
    background: currentColor;
    margin-right: 6px;
    vertical-align: 1px;
}

.report-status-success { color: var(--report-status-success); }
.report-status-warning { color: var(--report-status-warning); }
.report-status-info    { color: var(--report-status-info); }
.report-status-danger  { color: var(--report-status-danger); }

/* Severity badge — semantic + label-based, never color-only */
.report-severity-badge {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 3px 10px;
    border-radius: 999px;
    font-family: var(--report-font-mono);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: var(--report-tracking-wider);
    text-transform: uppercase;
    border: 1px solid currentColor;
    background: rgba(0, 0, 0, 0);
}

/* Generated artifact share/evidence footer */
.report-evidence-footer {
    margin: 48px auto 24px;
    padding: 24px 32px;
    max-width: 1100px;
    background: var(--theme-bg-surface);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-md);
    color: var(--theme-text-secondary);
    font-size: 13px;
    line-height: var(--report-lh-default);
}

.report-evidence-footer .evidence-eyebrow {
    font-family: var(--report-font-mono);
    font-size: var(--report-type-mono-cap);
    text-transform: uppercase;
    letter-spacing: var(--report-tracking-widest);
    color: var(--theme-text-tertiary);
    margin: 0 0 var(--report-space-3);
}

.report-evidence-footer .evidence-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 12px 24px;
    margin-bottom: 16px;
}

.report-evidence-footer .evidence-item {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
}

.report-evidence-footer .evidence-label {
    font-family: var(--report-font-mono);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: var(--report-tracking-wider);
    color: var(--theme-text-tertiary);
}

.report-evidence-footer .evidence-value {
    font-family: var(--report-font-mono);
    font-size: 12px;
    color: var(--theme-text-primary);
    word-break: break-all;
    overflow-wrap: anywhere;
}

.report-evidence-footer .evidence-repro {
    margin-top: 8px;
    padding: 12px 14px;
    background: var(--theme-bg-deep);
    border: 1px dashed var(--theme-border);
    border-radius: var(--radius-sm);
    font-family: var(--report-font-mono);
    font-size: 12px;
    color: var(--theme-text-primary);
    white-space: pre-wrap;
    word-break: break-all;
    overflow-wrap: anywhere;
}

.report-evidence-footer .evidence-fineprint {
    margin: 12px 0 0;
    font-size: 12px;
    color: var(--theme-text-tertiary);
}

/* Identity badge — "GENERATED LOCTREE REPORT" */
.report-identity-badge {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px;
    border: 1px solid var(--theme-border-strong);
    border-radius: 999px;
    font-family: var(--report-font-mono);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: var(--report-tracking-widest);
    color: var(--theme-text-tertiary);
    background: var(--theme-hover);
}

.report-identity-badge::before {
    content: "";
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 999px;
    background: var(--report-amber);
    box-shadow: 0 0 0 2px rgba(201, 154, 59, 0.18);
}

/* Empty / fallback state for graph and large surfaces */
.report-fallback-empty {
    padding: 32px;
    border: 1px dashed var(--theme-border);
    border-radius: var(--radius-md);
    background: var(--theme-bg-surface);
    color: var(--theme-text-secondary);
    font-size: 13px;
    line-height: var(--report-lh-default);
    text-align: center;
}

.report-fallback-empty strong {
    display: block;
    margin-bottom: 6px;
    color: var(--theme-text-primary);
}

/* Accessible focus ring — works for buttons, nav, links */
:where(button, a, [role="button"], .nav-item, .tab-btn, .copy-btn, .theme-toggle):focus-visible {
    outline: 2px solid var(--report-teal);
    outline-offset: 2px;
    border-radius: var(--radius-sm);
}

/* Reduced-motion: kill micro-interactions */
@media (prefers-reduced-motion: reduce) {
    .nav-item,
    .tab-btn,
    .copy-btn,
    .theme-toggle,
    .test-toggle-btn {
        transition: none;
    }
}

/* Narrow viewport / print fallback for the in-report header */
@media (max-width: 900px) {
    .report-sticky-hero .report-meta-row {
        font-size: 10px;
    }

    .report-evidence-footer {
        margin-left: 12px;
        margin-right: 12px;
        padding: 18px 20px;
    }
}

/* =============================================================================
 * W2-C: Atlas + Tools sidebar views
 * Added by prompt W2-C_report-sidebar (Wave 2C).
 * Mirrors the panel/card chrome of cycles + audit so the new sidebar tabs
 * feel native to the existing report.
 * ============================================================================= */

/* Atlas view ---------------------------------------------------------------- */
.atlas-view .atlas-header h3 {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 0 0 12px 0;
    font-size: 18px;
}

.atlas-message {
    color: var(--theme-text-secondary);
    font-size: 13px;
    line-height: 1.55;
    margin: 0 0 16px 0;
    max-width: 70ch;
}

.atlas-paths {
    display: grid;
    gap: 8px;
    background: var(--theme-bg-surface-elevated);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-md);
    padding: 12px 14px;
    margin-bottom: 20px;
}

.atlas-path-row {
    display: flex;
    align-items: center;
    gap: 12px;
    font-size: 12px;
}

.atlas-path-label {
    flex: 0 0 90px;
    color: var(--theme-text-tertiary);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-size: 11px;
    font-weight: 600;
}

.atlas-path {
    flex: 1 1 auto;
    font-family: var(--font-mono);
    color: var(--theme-text-primary);
    background: var(--theme-bg-deep);
    padding: 4px 8px;
    border-radius: var(--radius-sm);
    overflow-x: auto;
    white-space: nowrap;
}

.atlas-cards-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 14px;
    margin-bottom: 20px;
}

.atlas-card {
    background: var(--theme-bg-surface);
    border: 1px solid var(--theme-border);
    border-left: 3px solid var(--theme-accent);
    border-radius: var(--radius-md);
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    transition: box-shadow 0.18s ease, transform 0.18s ease;
}

.atlas-card:hover {
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.10);
    transform: translateY(-1px);
}

.atlas-card-header {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
}

.atlas-card-step {
    font-family: var(--font-mono);
    font-size: 11px;
    font-weight: 600;
    color: var(--theme-text-tertiary);
}

.atlas-card-id {
    font-family: var(--font-mono);
    font-size: 11px;
    font-weight: 600;
    color: var(--theme-accent);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    background: rgba(var(--theme-accent-rgb), 0.12);
    padding: 2px 8px;
    border-radius: 8px;
}

.atlas-card-title {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
    color: var(--theme-text-primary);
}

.atlas-card-why,
.atlas-card-saves {
    margin: 0;
    font-size: 13px;
    line-height: 1.5;
    color: var(--theme-text-secondary);
}

.atlas-card-label {
    color: var(--theme-text-tertiary);
    font-weight: 600;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    margin-right: 4px;
}

.atlas-card-meta {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    margin-top: 4px;
    padding-top: 8px;
    border-top: 1px dashed var(--theme-border);
}

.atlas-card-file {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--theme-text-secondary);
    word-break: break-all;
}

.atlas-card-lines {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--theme-text-primary);
    white-space: nowrap;
}

.atlas-card-lines.muted {
    color: var(--theme-text-tertiary);
    font-style: italic;
}

.atlas-card-lines.muted code {
    font-style: normal;
    background: var(--theme-bg-deep);
    padding: 1px 4px;
    border-radius: 3px;
}

.atlas-footer {
    margin-top: 8px;
}

.atlas-footer-fineprint {
    font-size: 12px;
    color: var(--theme-text-tertiary);
    margin: 0;
    line-height: 1.55;
}

.atlas-footer-fineprint code {
    background: var(--theme-bg-deep);
    padding: 1px 5px;
    border-radius: 3px;
    font-family: var(--font-mono);
    font-size: 11px;
}

/* Tools view ---------------------------------------------------------------- */
.tools-view .tools-header h3 {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 0 0 12px 0;
    font-size: 18px;
}

.tools-intro {
    color: var(--theme-text-secondary);
    font-size: 13px;
    line-height: 1.55;
    margin: 0 0 20px 0;
    max-width: 80ch;
}

.tools-intro code {
    background: var(--theme-bg-deep);
    padding: 1px 5px;
    border-radius: 3px;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--theme-text-primary);
}

.tools-cards-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
    gap: 14px;
    margin-bottom: 20px;
}

.tools-card {
    background: var(--theme-bg-surface);
    border: 1px solid var(--theme-border);
    border-radius: var(--radius-md);
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    transition: box-shadow 0.18s ease, transform 0.18s ease;
}

.tools-card:hover {
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.10);
    transform: translateY(-1px);
}

.tools-card-header {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
}

.tools-card-section {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    padding: 2px 8px;
    border-radius: 8px;
    background: var(--theme-bg-surface-elevated);
    color: var(--theme-text-tertiary);
    border: 1px solid var(--theme-border);
}

.tools-card-section-start {
    background: rgba(var(--theme-accent-rgb), 0.14);
    color: var(--theme-accent);
    border-color: rgba(var(--theme-accent-rgb), 0.30);
}

.tools-card-section-map {
    background: rgba(52, 152, 219, 0.14);
    color: #2e86c1;
    border-color: rgba(52, 152, 219, 0.30);
}

.tools-card-section-silencer {
    background: rgba(230, 126, 34, 0.14);
    color: var(--report-status-warning);
    border-color: rgba(230, 126, 34, 0.30);
}

.tools-card-section-polarize {
    background: rgba(155, 89, 182, 0.14);
    color: #8e44ad;
    border-color: rgba(155, 89, 182, 0.30);
}

.tools-card-name {
    margin: 0;
    font-family: var(--font-mono);
    font-size: 16px;
    font-weight: 700;
    color: var(--theme-text-primary);
}

.tools-card-signature {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--theme-text-secondary);
    background: var(--theme-bg-deep);
    padding: 2px 8px;
    border-radius: var(--radius-sm);
    flex-basis: 100%;
    overflow-x: auto;
    white-space: nowrap;
}

.tools-card-desc {
    margin: 0;
    font-size: 13px;
    line-height: 1.5;
    color: var(--theme-text-secondary);
}

.tools-card-example {
    margin: 0;
    font-size: 12px;
}

.tools-card-example > summary {
    cursor: pointer;
    color: var(--theme-text-tertiary);
    font-weight: 600;
    padding: 4px 0;
    user-select: none;
}

.tools-card-example > summary:hover {
    color: var(--theme-accent);
}

.tools-card-example-pre {
    margin: 8px 0 6px 0;
    padding: 10px 12px;
    background: var(--theme-bg-deep);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: 11.5px;
    line-height: 1.45;
    color: var(--theme-text-primary);
    overflow-x: auto;
    white-space: pre;
}

.tools-card-doc {
    align-self: flex-start;
    font-size: 12px;
    color: var(--theme-accent);
    text-decoration: none;
    padding: 4px 0;
    font-weight: 600;
}

.tools-card-doc:hover {
    text-decoration: underline;
}

.tools-footer {
    margin-top: 8px;
}

.tools-footer-fineprint {
    font-size: 12px;
    color: var(--theme-text-tertiary);
    margin: 0;
    line-height: 1.55;
}

.tools-footer-fineprint code {
    background: var(--theme-bg-deep);
    padding: 1px 5px;
    border-radius: 3px;
    font-family: var(--font-mono);
    font-size: 11px;
}

@media (max-width: 900px) {
    .atlas-path-row {
        flex-wrap: wrap;
    }
    .atlas-path-label {
        flex-basis: 100%;
    }
    .tools-cards-grid,
    .atlas-cards-grid {
        grid-template-columns: 1fr;
    }
}
"#;
