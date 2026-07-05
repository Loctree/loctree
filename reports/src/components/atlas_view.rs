//! Atlas sidebar view — first-class surface for the six Context Atlas cards.
//!
//! Mirrors the cards materialized by loctree's atlas module
//! (`loctree-rs/src/cli/dispatch/handlers/context/atlas.rs`):
//! `core / structural / runtime / memory / verification / risk`.
//!
//! When [`ContextAtlasInfo`] is present (atlas was materialized for this report)
//! we display the live card pointers and line counts. When it is absent we
//! still render the static description so the operator sees what the atlas
//! surface IS, even before running `loct auto`.
//!
//! This view is one of the two MCP-first sidebar surfaces — pair it with
//! [`super::tools_view::ToolsView`] to show *what loctree precomputes* (atlas)
//! and *what an agent calls into* (tools) side by side.

use crate::components::icons::{ICON_BOOK_OPEN, Icon};
use crate::types::ContextAtlasInfo;
use leptos::prelude::*;
use std::collections::HashMap;

/// Static spec for one of the six atlas cards. Mirrors the `CardSpec`
/// hardcoded in `loctree-rs/src/cli/dispatch/handlers/context/atlas.rs::materialize_context_atlas`.
#[derive(Clone, Copy)]
struct AtlasCardSpec {
    /// Card identifier (matches `ContextAtlasCardInfo::id`).
    id: &'static str,
    /// Human-readable card title.
    title: &'static str,
    /// On-disk filename when the atlas is materialized.
    filename: &'static str,
    /// Why an agent should read this card.
    why: &'static str,
    /// What harm reading this card prevents.
    saves: &'static str,
}

/// The six canonical atlas cards, in recommended reading order.
const ATLAS_CARDS: &[AtlasCardSpec] = &[
    AtlasCardSpec {
        id: "core",
        title: "Core Map",
        filename: "00-core-map.md",
        why: "Repo identity, current risk, authority labels, safe next commands.",
        saves: "wrong project state, stale assumptions, unsafe first actions",
    },
    AtlasCardSpec {
        id: "structural",
        title: "Structural Map",
        filename: "01-structural-map.md",
        why: "Files, symbols, imports, consumers, entrypoints; read before edits/refactors.",
        saves: "missed consumers, wrong impact, blind dependency edits",
    },
    AtlasCardSpec {
        id: "runtime",
        title: "Runtime Map",
        filename: "02-runtime-map.md",
        why: "Runtime behavior, framework hints, env contracts, reachability.",
        saves: "wrong tests, hidden runtime coupling, config mistakes",
    },
    AtlasCardSpec {
        id: "memory",
        title: "Memory Trail",
        filename: "03-memory-trail.md",
        why: "Prior decisions, outcomes, tasks, and AICX continuity when available.",
        saves: "repeated work, forgotten decisions, reimplemented tasks",
    },
    AtlasCardSpec {
        id: "verification",
        title: "Verification Gates",
        filename: "04-verification-gates.md",
        why: "Commands and likely tests most relevant to validate changes.",
        saves: "wrong validation path, skipped downstream checks, false confidence",
    },
    AtlasCardSpec {
        id: "risk",
        title: "Risk Register",
        filename: "05-risk-register.md",
        why: "Hotspots, cache/snapshot health, stale assumptions, next risk-reducing actions.",
        saves: "release blockers, high fan-in surprises, stale-cache decisions",
    },
];

/// First-class Context Atlas view — six cards, with live data when the atlas
/// is materialized.
#[component]
pub fn AtlasView(
    /// Optional materialized atlas pointer (present when `loct auto` ran).
    atlas: Option<ContextAtlasInfo>,
) -> impl IntoView {
    let materialized = atlas.is_some();
    let real_lines: HashMap<String, usize> = atlas
        .as_ref()
        .map(|a| a.cards.iter().map(|c| (c.id.clone(), c.lines)).collect())
        .unwrap_or_default();
    let real_paths: HashMap<String, String> = atlas
        .as_ref()
        .map(|a| {
            a.cards
                .iter()
                .map(|c| (c.id.clone(), c.path.clone()))
                .collect()
        })
        .unwrap_or_default();

    let atlas_dir = atlas
        .as_ref()
        .map(|a| a.atlas_dir.clone())
        .unwrap_or_default();
    let manifest = atlas
        .as_ref()
        .map(|a| a.manifest.clone())
        .unwrap_or_default();
    let recommended_start = atlas
        .as_ref()
        .map(|a| a.recommended_start.clone())
        .unwrap_or_default();
    let message = atlas.as_ref().map(|a| a.message.clone()).unwrap_or_else(|| {
        "Context Atlas is the precomputed repo understanding an agent would otherwise rediscover \
             manually through open / grep / read cycles. Tokens are cheaper than wrong assumptions. \
             Run `loct auto` to materialize the six cards below for this project.".to_string()
    });

    view! {
        <div class="panel atlas-view">
            <header class="atlas-header">
                <h3>
                    <Icon path=ICON_BOOK_OPEN />
                    "Context Atlas"
                    {if materialized {
                        view! { <span class="count-badge count-badge-success">"6 cards · ready"</span> }.into_any()
                    } else {
                        view! { <span class="count-badge">"6 cards · template"</span> }.into_any()
                    }}
                </h3>
                <p class="atlas-message">{message}</p>
                {(!atlas_dir.is_empty()).then(|| {
                    let atlas_dir_copy = atlas_dir.clone();
                    let manifest_copy = manifest.clone();
                    let start_copy = recommended_start.clone();
                    view! {
                        <div class="atlas-paths">
                            <div class="atlas-path-row">
                                <span class="atlas-path-label">"Atlas dir"</span>
                                <code class="atlas-path">{atlas_dir.clone()}</code>
                                <button class="copy-btn" data-copy=atlas_dir_copy title="Copy atlas directory">"Copy"</button>
                            </div>
                            <div class="atlas-path-row">
                                <span class="atlas-path-label">"Manifest"</span>
                                <code class="atlas-path">{manifest.clone()}</code>
                                <button class="copy-btn" data-copy=manifest_copy title="Copy manifest path">"Copy"</button>
                            </div>
                            <div class="atlas-path-row">
                                <span class="atlas-path-label">"Start here"</span>
                                <code class="atlas-path">{recommended_start.clone()}</code>
                                <button class="copy-btn" data-copy=start_copy title="Copy first card path">"Copy"</button>
                            </div>
                        </div>
                    }
                })}
            </header>

            <div class="atlas-cards-grid">
                {ATLAS_CARDS.iter().enumerate().map(|(idx, spec)| {
                    let id_key = spec.id.to_string();
                    let lines = real_lines.get(&id_key).copied();
                    let path = real_paths
                        .get(&id_key)
                        .cloned()
                        .unwrap_or_else(|| spec.filename.to_string());
                    view! {
                        <article class="atlas-card">
                            <header class="atlas-card-header">
                                <span class="atlas-card-step">{format!("#{}", idx)}</span>
                                <span class="atlas-card-id">{spec.id}</span>
                                <h4 class="atlas-card-title">{spec.title}</h4>
                            </header>
                            <p class="atlas-card-why">
                                <span class="atlas-card-label">"Why read it: "</span>
                                {spec.why}
                            </p>
                            <p class="atlas-card-saves">
                                <span class="atlas-card-label">"Saves you from: "</span>
                                {spec.saves}
                            </p>
                            <footer class="atlas-card-meta">
                                <code class="atlas-card-file">{path}</code>
                                {match lines {
                                    Some(l) => view! {
                                        <span class="atlas-card-lines">{format!("{} lines", l)}</span>
                                    }.into_any(),
                                    None => view! {
                                        <span class="atlas-card-lines muted">"template — run "<code>"loct auto"</code></span>
                                    }.into_any(),
                                }}
                            </footer>
                        </article>
                    }
                }).collect::<Vec<_>>()}
            </div>

            <footer class="atlas-footer">
                <p class="atlas-footer-fineprint">
                    "The atlas is materialized by "
                    <code>"loct auto"</code>
                    " (or via the MCP "
                    <code>"context"</code>
                    " tool with "
                    <code>"format='markdown'"</code>
                    "). Cards live under "
                    <code>".loctree/context-atlas/"</code>
                    " inside the project root."
                </p>
            </footer>
        </div>
    }
}
