//! Import hotspots panel - files with high fan-in.

use leptos::prelude::*;

use crate::components::icons::{ICON_GRAPH, Icon};
use crate::types::HotspotFile;

/// Panel listing files with the highest importer counts.
#[component]
pub fn Hotspots(hotspots: Vec<HotspotFile>) -> impl IntoView {
    if hotspots.is_empty() {
        return view! { "" }.into_any();
    }

    view! {
        <div class="panel hotspots-panel">
            <h3>
                <Icon path=ICON_GRAPH class="icon-sm" />
                "Import Hotspots"
            </h3>
            <p class="muted">"High fan-in files with the largest downstream blast radius."</p>
            <table class="data-table hotspots-table">
                <thead>
                    <tr>
                        <th>"File"</th>
                        <th>"Importers"</th>
                        <th>"Category"</th>
                        <th>"Slice"</th>
                    </tr>
                </thead>
                <tbody>
                    {hotspots.into_iter().map(|hotspot| {
                        let slice_cmd = hotspot.slice_cmd.clone();
                        view! {
                            <tr>
                                <td><code>{hotspot.file}</code></td>
                                <td>{hotspot.importers}</td>
                                <td>{hotspot.category}</td>
                                <td>
                                    <code>{slice_cmd.clone()}</code>
                                    <button class="copy-btn" data-copy=slice_cmd>"Copy"</button>
                                </td>
                            </tr>
                        }
                    }).collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

// Tests live in reports/src/lib.rs via render_report.
