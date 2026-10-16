//! Context Atlas pointer panel — top-of-report banner that surfaces a
//! materialized Context Atlas (precomputed reading cards) for the project.
//!
//! Rendered only when the analyzer admitted the section's source-local Atlas
//! and attached its resolved paths to `ReportSection::context_atlas`.
//!
//! Each path-typed line carries a Copy button (reusing the existing
//! `data-copy` handler in `document.rs::APP_SCRIPT`) so an agent or operator
//! can paste the path straight into a terminal or editor. Cards whose body
//! was embedded at generation time additionally open in-page via
//! `<details>` — clicking a reading-path entry shows the card itself.

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
                    <code class="context-atlas-path">{if atlas.recommended_start.is_empty() { "Unavailable".to_string() } else { atlas.recommended_start.clone() }}</code>
                    <button
                        class="copy-btn"
                        disabled=atlas.recommended_start.is_empty()
                        data-copy=(!recommended_for_copy.is_empty()).then_some(recommended_for_copy)
                        title="Copy path to first card"
                    >
                        "Copy"
                    </button>
                </div>
                <div class="context-atlas-path-row">
                    <span class="context-atlas-path-label">"Manifest"</span>
                    <code class="context-atlas-path">{if atlas.manifest.is_empty() { "Unavailable".to_string() } else { atlas.manifest.clone() }}</code>
                    <button
                        class="copy-btn"
                        disabled=atlas.manifest.is_empty()
                        data-copy=(!manifest_for_copy.is_empty()).then_some(manifest_for_copy)
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
                        disabled=atlas.atlas_dir.is_empty()
                        data-copy=(!atlas_dir_for_copy.is_empty()).then_some(atlas_dir_for_copy)
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
                            let path_for_copy = card.path.clone();
                            let unavailable = card.path.is_empty();
                            let path_label = if unavailable { "Unavailable".to_string() } else { card.path };
                            match card.body {
                                Some(body) => view! {
                                    <li class="context-atlas-card">
                                        <details class="context-atlas-card-details">
                                            <summary class="context-atlas-card-summary">
                                                <span class="context-atlas-card-title">{card.title}</span>
                                                <code class="context-atlas-card-path">{path_label}</code>
                                                <span class="context-atlas-card-lines">{format!("{} lines", card.lines)}</span>
                                            </summary>
                                            <p class="context-atlas-card-why">{card.why}</p>
                                            <button
                                                class="copy-btn"
                                                disabled=unavailable
                                                data-copy=(!unavailable).then_some(path_for_copy)
                                                title="Copy card path"
                                            >
                                                "Copy path"
                                            </button>
                                            <pre class="atlas-card-body"><code>{body}</code></pre>
                                        </details>
                                    </li>
                                }.into_any(),
                                None => view! {
                                    <li class="context-atlas-card">
                                        <span class="context-atlas-card-title">{card.title}</span>
                                        <code class="context-atlas-card-path">{path_label}</code>
                                        <span class="context-atlas-card-lines">{format!("{} lines", card.lines)}</span>
                                        <p class="context-atlas-card-why">{card.why}</p>
                                        <button
                                            class="copy-btn"
                                            disabled=unavailable
                                            data-copy=(!unavailable).then_some(path_for_copy)
                                            title="Copy card path"
                                        >
                                            "Copy path"
                                        </button>
                                    </li>
                                }.into_any(),
                            }
                        }).collect::<Vec<_>>()}
                    </ol>
                </div>
            })}
        </section>
    }
}

#[cfg(test)]
mod tests {
    use crate::types::{ContextAtlasCardInfo, ContextAtlasInfo, ReportSection};
    use crate::{JsAssets, render_report};

    #[test]
    fn resolved_atlas_paths_and_exact_body_reach_both_ssr_surfaces() {
        let atlas_dir = "/source/project/.loctree/context-atlas";
        let card_path = format!("{atlas_dir}/00-core-map.md");
        let atlas = ContextAtlasInfo {
            atlas_dir: atlas_dir.into(),
            manifest: format!("{atlas_dir}/manifest.md"),
            manifest_json: format!("{atlas_dir}/manifest.json"),
            recommended_start: card_path.clone(),
            cards: vec![ContextAtlasCardInfo {
                id: "core".into(),
                title: "Core Map".into(),
                path: card_path.clone(),
                lines: 2,
                body: Some("# Source <repo>\nsecond line\n".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let section = ReportSection {
            root: "/source/project".into(),
            context_atlas: Some(atlas),
            ..Default::default()
        };
        let html = render_report(&[section], &JsAssets::default(), false);
        assert_eq!(
            html.matches("# Source &lt;repo&gt;\nsecond line\n").count(),
            2
        );
        assert_eq!(
            html.matches(&format!("data-copy=\"{card_path}\"")).count(),
            3
        );
        assert_eq!(
            html.matches(&format!("data-copy=\"{atlas_dir}/manifest.md\""))
                .count(),
            2
        );
        assert_eq!(
            html.matches(&format!("data-copy=\"{atlas_dir}\"")).count(),
            2
        );
        assert!(!html.contains("data-copy=\"00-core-map.md\""));
    }

    #[test]
    fn unavailable_atlas_targets_have_no_copy_payload() {
        let section = ReportSection {
            root: "/source/project".into(),
            context_atlas: Some(ContextAtlasInfo {
                cards: vec![ContextAtlasCardInfo {
                    id: "core".into(),
                    title: "Missing card".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let html = render_report(&[section], &JsAssets::default(), false);
        assert!(html.contains("Unavailable"));
        assert!(!html.contains("data-copy=\"\""));
        assert!(html.contains("disabled title=\"Copy card path\""));
        assert!(html.contains("disabled title=\"Copy atlas directory\""));
    }
}
