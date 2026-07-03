//! Root document component - the complete HTML page
//!
//! Implements the App Shell layout with Sidebar and Main Content areas.

use super::{
    ICON_ARROWS_CLOCKWISE, ICON_BOOK_OPEN, ICON_CLIPBOARD_LIST, ICON_COPY, ICON_FLASK, ICON_GHOST,
    ICON_GRAPH, ICON_LIGHTNING, ICON_PACKAGE, ICON_PLUG, ICON_SQUARES_FOUR, ICON_TOOLBOX,
    ICON_TREE_STRUCTURE, ICON_TWINS, ICON_USERS, Icon, ReportSectionView,
};
use crate::JsAssets;
use crate::styles::{CSP, REPORT_CSS};
use crate::types::ReportSection;
use leptos::prelude::*;

/// Crate version, sourced at compile time from Cargo metadata so generated
/// reports never lie about which renderer produced them.
pub(crate) const REPORT_RENDERER_VERSION: &str = env!("CARGO_PKG_VERSION");

// Inline data URI for the loctree logo (ensures logo renders offline in reports)
const LOGO_DATA_URI: &str = "data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMzYwIiBoZWlnaHQ9IjM2MCIgdmlld0JveD0iMCAwIDM2MCAzNjAiIHhtbG5zPSJodHRwOi8vd3d3LnczLm9yZy8yMDAwL3N2ZyIgcm9sZT0iaW1nIiBhcmlhLWxhYmVsbGVkYnk9InRpdGxlIGRlc2MiPgogIDx0aXRsZSBpZD0idGl0bGUiPkxvY3RyZWUgTG9nbzwvdGl0bGU+CiAgPGRlc2MgaWQ9ImRlc2MiPk1pbmltYWxpc3Qgbm9kZSB0cmVlIC0gZHluYW1pYyBhbmQgc2xpZ2h0bHkgdW5zZXR0bGluZzwvZGVzYz4KCiAgPGRlZnM+CiAgICA8c3R5bGU+CiAgICAgIC5ub2RlIHsgZmlsbDogI2UwZTBlMDsgfQogICAgICAuc3RlbSB7IHN0cm9rZTogI2UwZTBlMDsgc3Ryb2tlLXdpZHRoOiAxMDsgc3Ryb2tlLWxpbmVjYXA6IHJvdW5kOyB9CiAgICA8L3N0eWxlPgogIDwvZGVmcz4KCiAgPCEtLSBSb3cgMSAtIDMgbm9kZXMgKHRvcCwgc3ltbWV0cmljLCB0aWdodGVuZWQgMjBweCkgLS0+CiAgPGNpcmNsZSBjbGFzcz0ibm9kZSIgY3g9Ijc1IiBjeT0iNTAiIHI9IjE2Ii8+CiAgPGNpcmNsZSBjbGFzcz0ibm9kZSIgY3g9IjE4MCIgY3k9IjUwIiByPSIxNiIvPgogIDxjaXJjbGUgY2xhc3M9Im5vZGUiIGN4PSIyODUiIGN5PSI1MCIgcj0iMTYiLz4KCiAgPCEtLSBSb3cgMiAtIDEgbm9kZSAodW5zZXR0bGluZ2x5IG9mZi1jZW50ZXIgdG8gbGVmdCkgLS0+CiAgPGNpcmNsZSBjbGFzcz0ibm9kZSIgY3g9IjE0MCIgY3k9IjEyMCIgcj0iMTYiLz4KCiAgPCEtLSBSb3cgMyAtIDMgbm9kZXMgKHN5bW1ldHJpYywgdGlnaHRlbmVkIDIwcHgpIC0tPgogIDxjaXJjbGUgY2xhc3M9Im5vZGUiIGN4PSI3NSIgY3k9IjE5MCIgcj0iMTYiLz4KICA8Y2lyY2xlIGNsYXNzPSJub2RlIiBjeD0iMTgwIiBjeT0iMTkwIiByPSIxNiIvPgogIDxjaXJjbGUgY2xhc3M9Im5vZGUiIGN4PSIyODUiIGN5PSIxOTAiIHI9IjE2Ii8+CgogIDwhLS0gU3RlbSAodmVydGljYWwsIHNoaWZ0ZWQgcmlnaHQsIHRoaWNrZXIpIC0tPgogIDxsaW5lIGNsYXNzPSJzdGVtIiB4MT0iMjEwIiB5MT0iMjIwIiB4Mj0iMjEwIiB5Mj0iMjc1Ii8+CgogIDwhLS0gUm9vdCBub2RlIGF0IGJvdHRvbSAtLT4KICA8Y2lyY2xlIGNsYXNzPSJub2RlIiBjeD0iMjA1IiBjeT0iMzEwIiByPSIxNiIvPgo8L3N2Zz4K";

/// The complete HTML document for the report
#[component]
pub fn ReportDocument(
    sections: Vec<ReportSection>,
    js_assets: JsAssets,
    /// Whether to show Tauri coverage tab (only for Tauri projects)
    #[prop(default = false)]
    has_tauri: bool,
) -> impl IntoView {
    let has_dist = sections.iter().any(|section| section.dist.is_some());

    view! {
        <html>
            <head>
                <meta charset="UTF-8" />
                <meta http-equiv="Content-Security-Policy" content=CSP />
                <title>"Loctree Report"</title>
                <style>{REPORT_CSS}</style>
            </head>
            <body>
                <div class="app-shell">
                    <aside class="app-sidebar">
                        <div class="sidebar-header">
                            <div class="logo-box">
                                <img class="logo-img" src=LOGO_DATA_URI alt="loctree logo" />
                                <div class="logo-text">
                                    <span style="color:var(--theme-accent)">"Loctree"</span>
                                    <span style="opacity:0.5">"Report"</span>
                                </div>
                            </div>
                            <button class="theme-toggle" data-role="theme-toggle" title="Toggle light/dark mode">
                                <svg class="theme-icon-light" xmlns="http://www.w3.org/2000/svg" width="18" height="18" fill="currentColor" viewBox="0 0 256 256">
                                    <path d="M120,40V16a8,8,0,0,1,16,0V40a8,8,0,0,1-16,0Zm72,88a64,64,0,1,1-64-64A64.07,64.07,0,0,1,192,128Zm-16,0a48,48,0,1,0-48,48A48.05,48.05,0,0,0,176,128ZM58.34,69.66A8,8,0,0,0,69.66,58.34l-16-16A8,8,0,0,0,42.34,53.66Zm0,116.68-16,16a8,8,0,0,0,11.32,11.32l16-16a8,8,0,0,0-11.32-11.32ZM192,72a8,8,0,0,0,5.66-2.34l16-16a8,8,0,0,0-11.32-11.32l-16,16A8,8,0,0,0,192,72Zm5.66,114.34a8,8,0,0,0-11.32,11.32l16,16a8,8,0,0,0,11.32-11.32ZM48,128a8,8,0,0,0-8-8H16a8,8,0,0,0,0,16H40A8,8,0,0,0,48,128Zm80,80a8,8,0,0,0-8,8v24a8,8,0,0,0,16,0V216A8,8,0,0,0,128,208Zm112-88H216a8,8,0,0,0,0,16h24a8,8,0,0,0,0-16Z"></path>
                                </svg>
                                <svg class="theme-icon-dark" xmlns="http://www.w3.org/2000/svg" width="18" height="18" fill="currentColor" viewBox="0 0 256 256">
                                    <path d="M233.54,142.23a8,8,0,0,0-8-2,88.08,88.08,0,0,1-109.8-109.8,8,8,0,0,0-10-10,104.84,104.84,0,0,0-52.91,37A104,104,0,0,0,136,224a103.09,103.09,0,0,0,62.52-20.88,104.84,104.84,0,0,0,37-52.91A8,8,0,0,0,233.54,142.23ZM188.9,190.34A88,88,0,0,1,65.66,67.11a89,89,0,0,1,31.4-26A106,106,0,0,0,96,56,104.11,104.11,0,0,0,200,160a106,106,0,0,0,14.92-1.06A89,89,0,0,1,188.9,190.34Z"></path>
                                </svg>
                            </button>
                        </div>

                        <nav class="sidebar-nav">
                            <button class="nav-item active" data-tab="overview">
                                <Icon path=ICON_SQUARES_FOUR class="icon-sm" />
                                "Overview"
                            </button>
                            <button class="nav-item" data-tab="atlas">
                                <Icon path=ICON_BOOK_OPEN class="icon-sm" />
                                "Atlas"
                            </button>
                            <button class="nav-item" data-tab="tools">
                                <Icon path=ICON_TOOLBOX class="icon-sm" />
                                "Tools"
                            </button>
                            <button class="nav-item" data-tab="audit">
                                <Icon path=ICON_CLIPBOARD_LIST class="icon-sm" />
                                "Audit"
                            </button>
                            <button class="nav-item" data-tab="dups">
                                <Icon path=ICON_COPY class="icon-sm" />
                                "Duplicates"
                            </button>
                            <button class="nav-item" data-tab="dynamic">
                                <Icon path=ICON_LIGHTNING class="icon-sm" />
                                "Dynamic imports"
                            </button>
                            {if has_dist {
                                view! {
                                    <button class="nav-item" data-tab="dist">
                                        <Icon path=ICON_PACKAGE class="icon-sm" />
                                        "Bundles"
                                    </button>
                                }.into_any()
                            } else {
                                view! { "" }.into_any()
                            }}
                            {if has_tauri {
                                view! {
                                    <button class="nav-item" data-tab="commands">
                                        <Icon path=ICON_PLUG class="icon-sm" />
                                        "Tauri coverage"
                                    </button>
                                    <button class="nav-item" data-tab="pipelines">
                                        <Icon path=ICON_PLUG class="icon-sm" />
                                        "Pipelines"
                                    </button>
                                }.into_any()
                            } else {
                                view! { "" }.into_any()
                            }}
                            <button class="nav-item" data-tab="crowds">
                                <Icon path=ICON_USERS class="icon-sm" />
                                "Crowds"
                            </button>
                            <button class="nav-item" data-tab="hotspots">
                                <Icon path=ICON_GRAPH class="icon-sm" />
                                "Hotspots"
                            </button>
                            <button class="nav-item" data-tab="cycles">
                                <Icon path=ICON_ARROWS_CLOCKWISE class="icon-sm" />
                                "Cycles"
                            </button>
                            <button class="nav-item" data-tab="dead">
                                <Icon path=ICON_GHOST class="icon-sm" />
                                "Dead Code"
                            </button>
                            <button class="nav-item" data-tab="twins">
                                <Icon path=ICON_TWINS class="icon-sm" />
                                "Twins"
                            </button>
                            <button class="nav-item" data-tab="refactor">
                                <Icon path=ICON_TREE_STRUCTURE class="icon-sm" />
                                "Refactor"
                            </button>
                            <button class="nav-item" data-tab="coverage">
                                <Icon path=ICON_FLASK class="icon-sm" />
                                "Coverage"
                            </button>
                            <button class="nav-item" data-tab="graph">
                                <Icon path=ICON_GRAPH class="icon-sm" />
                                "Graph"
                            </button>
                            <button class="nav-item" data-tab="tree">
                                <Icon path=ICON_TREE_STRUCTURE class="icon-sm" />
                                "Tree"
                            </button>
                        </nav>

                        <div class="app-footer">
                            <button id="toggle-tests-btn" class="test-toggle-btn" title="Toggle test file visibility">
                                <span id="test-toggle-icon"><Icon path=ICON_FLASK size="16" /></span>
                                <span id="test-toggle-text">"Hide Tests"</span>
                            </button>
                            <div style="margin-top: 8px; font-size: 11px;">
                                {"loctree v0.13.0".to_string()}
                                <br />
                                <span style="color:var(--theme-text-tertiary)">"Generated artifact"</span>
                            </div>
                        </div>
                    </aside>

                    <main class="app-main">
                        {sections.iter().cloned().enumerate().map(|(idx, section)| {
                            let view_id = format!("section-view-{}", idx);
                            let active = idx == 0;
                            view! {
                                <ReportSectionView
                                    section=section
                                    active=active
                                    view_id=view_id
                                />
                            }
                        }).collect::<Vec<_>>()}

                        <ReportEvidenceFooter sections=sections.clone() />
                    </main>
                </div>

                <GraphScripts js_assets=js_assets />
            </body>
        </html>
    }
}

/// Evidence/share footer rendered at the bottom of the document.
///
/// Communicates provenance for non-Rust readers and reviewers attaching the
/// artifact to a ticket or sales thread: renderer version, generated time,
/// source project root, git ref when available, and a reproduction command.
#[component]
fn ReportEvidenceFooter(sections: Vec<ReportSection>) -> impl IntoView {
    let primary = sections.first().cloned();
    let renderer = REPORT_RENDERER_VERSION;

    let (root, generated_at, schema, git_ref) = match primary.as_ref() {
        Some(s) => (
            s.root.clone(),
            s.generated_at.clone().unwrap_or_default(),
            match (s.schema_name.clone(), s.schema_version.clone()) {
                (Some(n), Some(v)) => format!("{}@{}", n, v),
                (Some(n), None) => n,
                _ => String::new(),
            },
            match (s.git_branch.clone(), s.git_commit.clone()) {
                (Some(b), Some(c)) => format!("{}@{}", b, c),
                (Some(b), None) => b,
                _ => String::new(),
            },
        ),
        None => (String::new(), String::new(), String::new(), String::new()),
    };

    let repro = if root.is_empty() {
        "loct report --output report.html".to_string()
    } else {
        format!(
            "loct report --output report.html  # cwd: {}",
            shorten_for_repro(&root)
        )
    };

    view! {
        <footer class="report-evidence-footer" role="contentinfo" aria-label="Generated report provenance">
            <p class="evidence-eyebrow">"Generated Loctree Report — provenance"</p>
            <div class="evidence-grid">
                <div class="evidence-item">
                    <span class="evidence-label">"Renderer"</span>
                    <span class="evidence-value">{format!("loctree-suite v{}", renderer)}</span>
                </div>
                <div class="evidence-item">
                    <span class="evidence-label">"Source project"</span>
                    <span class="evidence-value report-path-wrap">{if root.is_empty() { "(unspecified)".to_string() } else { root.clone() }}</span>
                </div>
                {(!generated_at.is_empty()).then(|| view! {
                    <div class="evidence-item">
                        <span class="evidence-label">"Generated at"</span>
                        <span class="evidence-value">{generated_at.clone()}</span>
                    </div>
                })}
                {(!git_ref.is_empty()).then(|| view! {
                    <div class="evidence-item">
                        <span class="evidence-label">"Git"</span>
                        <span class="evidence-value">{git_ref.clone()}</span>
                    </div>
                })}
                {(!schema.is_empty()).then(|| view! {
                    <div class="evidence-item">
                        <span class="evidence-label">"Schema"</span>
                        <span class="evidence-value">{schema.clone()}</span>
                    </div>
                })}
            </div>
            <p class="evidence-fineprint">"Reproduce this artifact:"</p>
            <pre class="evidence-repro" aria-label="Reproduction command">{repro}</pre>
            <p class="evidence-fineprint">
                "This is a generated Loctree report — analyzer-derived static evidence, "
                "safe to attach to a ticket or open from disk. No SaaS state, no entitlement data, "
                "no checkout. See "
                <a href="https://loct.io/cloud" style="color:var(--theme-text-secondary);text-decoration:underline">"loct.io/cloud"</a>
                " for hosted Loctree."
            </p>
        </footer>
    }
}

fn shorten_for_repro(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 4 {
        path.to_string()
    } else {
        let tail: Vec<&str> = parts.iter().rev().take(3).copied().collect();
        let mut joined: Vec<&str> = tail.into_iter().rev().collect();
        joined.insert(0, "…");
        joined.join("/")
    }
}

/// JavaScript for graph initialization and UI interactivity
#[component]
fn GraphScripts(js_assets: JsAssets) -> impl IntoView {
    let has_graph_assets = !js_assets.cytoscape_path.is_empty();

    view! {
        // App navigation script FIRST (must run even if graph fails)
        <script>{APP_SCRIPT}</script>
        // Graph-specific scripts (only when assets are provided)
        // Load order matters: layout-base -> cose-base -> cytoscape-cose-bilkent
        {has_graph_assets.then(|| view! {
            <script src=js_assets.cytoscape_path.clone()></script>
            <script src=js_assets.dagre_path.clone()></script>
            <script src=js_assets.cytoscape_dagre_path.clone()></script>
            <script src=js_assets.layout_base_path.clone()></script>
            <script src=js_assets.cose_base_path.clone()></script>
            <script src=js_assets.cytoscape_cose_bilkent_path.clone()></script>
            <script>{include_str!("../graph_bootstrap.js")}</script>
            <script>{include_str!("../twins_graph.js")}</script>
            <script>{include_str!("../crowds_graph.js")}</script>
        })}
    }
}

/// Application logic (Navigation, Tabs, Resize, Theme Toggle, Copy)
const APP_SCRIPT: &str = r#"
(() => {
  // -1. Copy Button Handler
  document.querySelectorAll('.copy-btn[data-copy]').forEach(btn => {
      btn.addEventListener('click', () => {
          const text = btn.dataset.copy;
          navigator.clipboard.writeText(text).then(() => {
              const orig = btn.textContent;
              btn.textContent = 'Copied';
              setTimeout(() => btn.textContent = orig, 1500);
          });
      });
  });
  // 0. Theme Initialization & Toggle
  const initTheme = () => {
      const stored = localStorage.getItem('loctree-theme');
      if (stored === 'dark') {
          document.documentElement.classList.add('dark');
          document.documentElement.classList.remove('light');
      } else if (stored === 'light') {
          document.documentElement.classList.add('light');
          document.documentElement.classList.remove('dark');
      } else {
          if (window.matchMedia('(prefers-color-scheme: dark)').matches) {
              document.documentElement.classList.add('dark');
          }
      }
  };

  const toggleTheme = () => {
      const isDark = document.documentElement.classList.contains('dark') ||
          (!document.documentElement.classList.contains('light') &&
           window.matchMedia('(prefers-color-scheme: dark)').matches);

      if (isDark) {
          document.documentElement.classList.remove('dark');
          document.documentElement.classList.add('light');
          localStorage.setItem('loctree-theme', 'light');
      } else {
          document.documentElement.classList.add('dark');
          document.documentElement.classList.remove('light');
          localStorage.setItem('loctree-theme', 'dark');
      }

      document.querySelectorAll('[data-role="dark"]').forEach(chk => {
          chk.checked = document.documentElement.classList.contains('dark');
      });
  };

  initTheme();

  const themeToggle = document.querySelector('[data-role="theme-toggle"]');
  if (themeToggle) {
      themeToggle.addEventListener('click', toggleTheme);
  }

  // 1. Sidebar Navigation (Tab Switching)
  document.querySelectorAll('.sidebar-nav .nav-item[data-tab]').forEach(btn => {
      btn.addEventListener('click', () => {
          const tabName = btn.dataset.tab;

          // Update Sidebar buttons
          document.querySelectorAll('.sidebar-nav .nav-item').forEach(b => b.classList.remove('active'));
          btn.classList.add('active');

          // Update all tab panels across all sections
          document.querySelectorAll('.tab-panel').forEach(p => {
              const isActive = p.dataset.tabName === tabName;
              p.classList.toggle('active', isActive);

              if (isActive && tabName === 'graph') {
                  window.dispatchEvent(new Event('resize'));
              }
          });

          // Also update header tab-bar buttons if present (for visual sync)
          document.querySelectorAll('.tab-bar .tab-btn').forEach(b => {
              b.classList.toggle('active', b.dataset.tab === tabName);
          });
      });
  });

  // 2. Header Tab Switching (if still present, syncs with sidebar)
  document.querySelectorAll('.tab-bar .tab-btn').forEach(btn => {
      btn.addEventListener('click', () => {
          const tabName = btn.dataset.tab;

          // Trigger sidebar button click to keep everything in sync
          const sidebarBtn = document.querySelector(`.sidebar-nav .nav-item[data-tab="${tabName}"]`);
          if (sidebarBtn) {
              sidebarBtn.click();
          }
      });
  });

  // 3. Twins Section Toggle - handles collapsible sections in Twins tab
  document.querySelectorAll('.twins-section-header[data-toggle]').forEach(btn => {
      btn.addEventListener('click', () => {
          const targetId = btn.dataset.toggle;
          const content = document.getElementById(targetId);
          const toggle = btn.querySelector('.twins-section-toggle');

          if (content) {
              const isHidden = content.style.display === 'none';
              content.style.display = isHidden ? 'block' : 'none';
              if (toggle) {
                  toggle.textContent = isHidden ? '▼' : '▶';
              }

              // Initialize Cytoscape graph when twins-exact-content is opened
              if (isHidden && targetId === 'twins-exact-content' && window.__TWINS_DATA__) {
                  const container = document.getElementById('twins-graph-container');
                  if (container && typeof buildTwinsGraph === 'function') {
                      buildTwinsGraph(window.__TWINS_DATA__, 'twins-graph-container');
                  }
              }
          }
      });
  });

  // 3b. Crowds Graph Toggle - handles graph view in Crowds tab
  document.querySelectorAll('.crowds-section-header[data-toggle]').forEach(btn => {
      btn.addEventListener('click', () => {
          const targetId = btn.dataset.toggle;
          const content = document.getElementById(targetId);
          const toggle = btn.querySelector('.crowds-graph-toggle');

          if (content) {
              const isHidden = content.style.display === 'none';
              content.style.display = isHidden ? 'block' : 'none';
              if (toggle) {
                  toggle.textContent = isHidden ? '▼' : '▶';
              }

              // Initialize Cytoscape graph when crowds-graph-content is opened
              if (isHidden && targetId === 'crowds-graph-content' && window.__CROWDS_DATA__) {
                  const container = document.getElementById('crowds-graph-container');
                  if (container && typeof buildCrowdsGraph === 'function') {
                      buildCrowdsGraph(window.__CROWDS_DATA__, 'crowds-graph-container');
                  }
              }
          }
      });
  });

  // 3c. Pipeline Card Toggle - expand/collapse pipeline cards
  document.querySelectorAll('.card-header[data-pipeline-toggle]').forEach(header => {
      header.addEventListener('click', () => {
          const card = header.closest('.pipeline-card');
          const details = card.querySelector('.card-details');
          const toggle = header.querySelector('.expand-icon');

          if (details) {
              const isHidden = details.style.display === 'none';
              details.style.display = isHidden ? 'block' : 'none';
              if (toggle) {
                  toggle.textContent = isHidden ? '▼' : '▶';
              }
          }
      });
  });

  // 3d. Pipeline Filter Buttons - filter cards by status
  document.querySelectorAll('[data-pipeline-filter]').forEach(btn => {
      btn.addEventListener('click', () => {
          const filter = btn.dataset.pipelineFilter;

          // Update active button
          document.querySelectorAll('[data-pipeline-filter]').forEach(b => b.classList.remove('active'));
          btn.classList.add('active');

          // Filter cards
          document.querySelectorAll('.pipeline-card').forEach(card => {
              const status = card.dataset.pipelineStatus;
              const show = filter === 'all' || status === filter;
              card.style.display = show ? '' : 'none';
          });
      });
  });

  // 3e. Pipeline Search - filter cards by name
  const pipelineSearch = document.querySelector('[data-pipeline-search]');
  if (pipelineSearch) {
      pipelineSearch.addEventListener('input', (e) => {
          const query = e.target.value.toLowerCase();

          document.querySelectorAll('.pipeline-card').forEach(card => {
              const name = card.dataset.pipelineName || '';
              const matchesSearch = name.includes(query);
              // Also respect current filter
              const currentFilter = document.querySelector('[data-pipeline-filter].active')?.dataset.pipelineFilter || 'all';
              const status = card.dataset.pipelineStatus;
              const matchesFilter = currentFilter === 'all' || status === currentFilter;

              card.style.display = (matchesSearch && matchesFilter) ? '' : 'none';
          });
      });
  }

  // 3f. Pipeline View Toggle - switch between card grid and split panel views
  document.querySelectorAll('[data-pipeline-view]').forEach(btn => {
      btn.addEventListener('click', function() {
          const mode = this.dataset.pipelineView;

          // Toggle active button
          document.querySelectorAll('[data-pipeline-view]').forEach(b => b.classList.remove('active'));
          this.classList.add('active');

          // Toggle views
          const gridView = document.querySelector('.cards-grid');
          const splitView = document.querySelector('.split-panel-container');

          if (mode === 'split') {
              if (gridView) gridView.style.display = 'none';
              if (splitView) {
                  splitView.style.display = 'grid';
                  setTimeout(drawPipelineConnections, 100);
              }
          } else {
              if (gridView) gridView.style.display = 'grid';
              if (splitView) splitView.style.display = 'none';
          }
      });
  });

  // Draw SVG connections between FE and BE items in split panel view
  function drawPipelineConnections() {
      const svg = document.querySelector('.connection-svg');
      if (!svg) return;

      // Clear existing paths
      svg.innerHTML = '';

      const svgRect = svg.getBoundingClientRect();

      document.querySelectorAll('[data-split-fe]').forEach(fe => {
          const name = fe.dataset.splitFe;
          const be = document.querySelector('[data-split-be="' + name + '"]');

          const feRect = fe.getBoundingClientRect();
          const y1 = feRect.top + feRect.height / 2 - svgRect.top;

          const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');

          if (be) {
              const beRect = be.getBoundingClientRect();
              const y2 = beRect.top + beRect.height / 2 - svgRect.top;
              // Bezier curve from left to right
              path.setAttribute('d', 'M 0 ' + y1 + ' C 40 ' + y1 + ', 40 ' + y2 + ', 80 ' + y2);
              path.classList.add('connection-line');
          } else {
              // Missing handler - draw dashed line to middle
              path.setAttribute('d', 'M 0 ' + y1 + ' L 60 ' + y1);
              path.classList.add('connection-line', 'missing');
          }

          svg.appendChild(path);
      });
  }

  // Redraw connections on window resize
  window.addEventListener('resize', () => {
      if (document.querySelector('.split-panel-container')?.style.display !== 'none') {
          drawPipelineConnections();
      }
  });

  // 3g. Refactor Phase Toggle - collapse/expand phases
  document.querySelectorAll('.phase-header[data-toggle="phase"]').forEach(header => {
      header.addEventListener('click', () => {
          const card = header.closest('.phase-card');
          if (card) {
              card.classList.toggle('collapsed');
          }
      });
  });

  // 4. Test Files Toggle - Hide/Show test file rows
  const toggleTestsBtn = document.getElementById('toggle-tests-btn');
  const toggleIcon = document.getElementById('test-toggle-icon');
  const toggleText = document.getElementById('test-toggle-text');

  // Initialize state from localStorage
  const testsHidden = localStorage.getItem('loctree-hide-tests') === 'true';

  const updateTestsVisibility = (hide) => {
    const testItems = document.querySelectorAll('[data-is-test="true"]');
    testItems.forEach(el => {
      el.style.display = hide ? 'none' : '';
    });

    // Update button state
    if (toggleText) {
      toggleText.textContent = hide ? 'Show Tests' : 'Hide Tests';
    }
    if (toggleIcon) {
      toggleIcon.style.opacity = hide ? '0.5' : '1';
    }

    // Save to localStorage
    localStorage.setItem('loctree-hide-tests', hide ? 'true' : 'false');
  };

  // Apply initial state
  updateTestsVisibility(testsHidden);

  // Add click handler
  if (toggleTestsBtn) {
    toggleTestsBtn.addEventListener('click', () => {
      const currentlyHidden = localStorage.getItem('loctree-hide-tests') === 'true';
      updateTestsVisibility(!currentlyHidden);
    });
  }
})();
"#;
