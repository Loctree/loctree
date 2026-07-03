//! Context Atlas pointer panel — top-of-report banner that surfaces a
//! materialized Context Atlas (precomputed reading cards) for the project.
//!
//! Rendered only when `loct auto` produced `<artifacts_dir>/context-atlas/`
//! and the analyzer attached the pointer to `ReportSection::context_atlas`.
//!
//! Each path-typed line carries a Copy button (reusing the existing
//! `data-copy` handler in `document.rs::APP_SCRIPT`) so an agent or operator
//! can paste the path straight into a terminal or editor.

use crate::components::icons::{ICON_CLIPBOARD_LIST, Icon};
use crate::types::ContextAtlasInfo;
use leptos::prelude::*;

/// Top-of-report banner pointing at the materialized Context Atlas.
#[component]
pub fn ContextAtlasPanel(atlas: ContextAtlasInfo) -> impl IntoView {
    let cards_count = atlas.cards.len();
    let manifest_for_copy = atlas.manifest.clone();
    let recommended_for_copy = atlas.recommended_start.clone();
    let atlas_dir_for_copy = atlas.atlas_dir.clone();

    view! {
        <section class="context-atlas-panel">
            <header class="context-atlas-header">
                <h3>
                    <Icon path=ICON_CLIPBOARD_LIST />
                    "Context Atlas Ready"
                    <span class="badge-new">{format!("{cards_count} cards")}</span>
                </h3>
                <p class="context-atlas-message">{atlas.message.clone()}</p>
            </header>

            <div class="context-atlas-paths">
                <div class="context-atlas-path-row">
                    <span class="context-atlas-path-label">"Start here"</span>
                    <code class="context-atlas-path">{atlas.recommended_start.clone()}</code>
                    <button
                        class="copy-btn"
                        data-copy=recommended_for_copy
                        title="Copy path to first card"
                    >
                        "Copy"
                    </button>
                </div>
                <div class="context-atlas-path-row">
                    <span class="context-atlas-path-label">"Manifest"</span>
                    <code class="context-atlas-path">{atlas.manifest.clone()}</code>
                    <button
                        class="copy-btn"
                        data-copy=manifest_for_copy
                        title="Copy manifest path"
                    >
                        "Copy"
                    </button>
                </div>
                <div class="context-atlas-path-row">
                    <span class="context-atlas-path-label">"Atlas dir"</span>
                    <code class="context-atlas-path">{atlas.atlas_dir.clone()}</code>
                    <button
                        class="copy-btn"
                        data-copy=atlas_dir_for_copy
                        title="Copy atlas directory"
                    >
                        "Copy"
                    </button>
                </div>
            </div>

            {(!atlas.cards.is_empty()).then(|| view! {
                <div class="context-atlas-cards">
                    <h4>"Recommended reading path"</h4>
                    <ol class="context-atlas-card-list">
                        {atlas.cards.into_iter().map(|card| {
                            view! {
                                <li class="context-atlas-card">
                                    <span class="context-atlas-card-title">{card.title}</span>
                                    <code class="context-atlas-card-path">{card.path}</code>
                                    <span class="context-atlas-card-lines">{format!("{} lines", card.lines)}</span>
                                    <p class="context-atlas-card-why">{card.why}</p>
                                </li>
                            }
                        }).collect::<Vec<_>>()}
                    </ol>
                </div>
            })}
        </section>
    }
}
