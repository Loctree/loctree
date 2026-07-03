//! Refactor plan panel - architectural refactoring visualization.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use leptos::prelude::*;

use crate::components::icons::{
    ICON_CHECK_CIRCLE, ICON_SIREN, ICON_TREE_STRUCTURE, ICON_WARNING_CIRCLE, Icon,
};
use crate::types::{RefactorPhase, RefactorPlanData, RefactorShim, RefactorStats};

/// Main refactor plan panel showing phases, moves, and shims.
#[component]
pub fn RefactorPlan(plan: Option<RefactorPlanData>) -> impl IntoView {
    match plan {
        Some(data) => render_plan(data).into_any(),
        None => view! {
            <div class="panel refactor-empty">
                <p class="muted">"No refactor plan available. Run "</p>
                <code>"loct plan &lt;directory&gt;"</code>
                <p class="muted">" to generate architectural suggestions."</p>
            </div>
        }
        .into_any(),
    }
}

fn render_plan(plan: RefactorPlanData) -> impl IntoView {
    let stats = plan.stats.clone();
    let has_cycles = !plan.cyclic_groups.is_empty();

    view! {
        <div class="refactor-plan-panel">
            <RefactorSummary stats=stats.clone() />
            <LayerDistribution stats=stats />
            {has_cycles.then(|| view! {
                <CyclicWarning groups=plan.cyclic_groups.clone() />
            })}
            <ExecutionPhases phases=plan.phases.clone() />
            {(!plan.shims.is_empty()).then(|| view! {
                <ShimmingStrategy shims=plan.shims.clone() />
            })}
        </div>
    }
}

#[component]
fn RefactorSummary(stats: RefactorStats) -> impl IntoView {
    view! {
        <div class="panel refactor-summary">
            <h3>
                <Icon path=ICON_TREE_STRUCTURE class="icon-sm" />
                " Refactor Strategist"
            </h3>
            <div class="refactor-stats-grid">
                <div class="stat-item">
                    <span class="stat-value">{stats.total_files}</span>
                    <span class="stat-label">"files analyzed"</span>
                </div>
                <div class="stat-item">
                    <span class="stat-value">{stats.files_to_move}</span>
                    <span class="stat-label">"to move"</span>
                </div>
                <div class="stat-item">
                    <span class="stat-value">{stats.shims_needed}</span>
                    <span class="stat-label">"shims needed"</span>
                </div>
            </div>
            <div class="risk-badges">
                {stats.by_risk.iter().map(|(risk, count)| {
                    let class = format!("risk-badge risk-{}", risk.to_lowercase());
                    let icon = match risk.to_lowercase().as_str() {
                        "low" => ICON_CHECK_CIRCLE,
                        "medium" => ICON_WARNING_CIRCLE,
                        _ => ICON_SIREN,
                    };
                    view! {
                        <span class=class>
                            <Icon path=icon class="icon-xs" />
                            {format!(" {} {}", count, risk.to_uppercase())}
                        </span>
                    }
                }).collect::<Vec<_>>()}
            </div>
        </div>
    }
}

#[component]
fn LayerDistribution(stats: RefactorStats) -> impl IntoView {
    let max_before = stats.layer_before.values().max().copied().unwrap_or(1);
    let max_after = stats.layer_after.values().max().copied().unwrap_or(1);

    view! {
        <div class="panel layer-distribution">
            <h4>"Layer Distribution"</h4>
            <div class="distribution-grid">
                <div class="distribution-column">
                    <h5>"Before"</h5>
                    {stats.layer_before.iter().map(|(layer, count)| {
                        let width = (*count as f64 / max_before as f64 * 100.0) as u32;
                        view! {
                            <div class="layer-bar">
                                <span class="layer-name">{layer.clone()}</span>
                                <div class="bar-track">
                                    <div class="bar-fill before" style=format!("width:{}%", width)></div>
                                </div>
                                <span class="layer-count">{*count}</span>
                            </div>
                        }
                    }).collect::<Vec<_>>()}
                </div>
                <div class="distribution-column">
                    <h5>"After"</h5>
                    {stats.layer_after.iter().map(|(layer, count)| {
                        let width = (*count as f64 / max_after as f64 * 100.0) as u32;
                        view! {
                            <div class="layer-bar">
                                <span class="layer-name">{layer.clone()}</span>
                                <div class="bar-track">
                                    <div class="bar-fill after" style=format!("width:{}%", width)></div>
                                </div>
                                <span class="layer-count">{*count}</span>
                            </div>
                        }
                    }).collect::<Vec<_>>()}
                </div>
            </div>
        </div>
    }
}

#[component]
fn CyclicWarning(groups: Vec<Vec<String>>) -> impl IntoView {
    view! {
        <div class="panel cyclic-warning">
            <h4>
                <Icon path=ICON_WARNING_CIRCLE class="icon-sm" />
                " Cyclic Dependencies"
            </h4>
            <p class="muted">"Move these together or break the cycle first:"</p>
            {groups.iter().enumerate().map(|(i, group)| {
                view! {
                    <div class="cycle-group">
                        <strong>{format!("Cycle {}", i + 1)}</strong>
                        <ul>
                            {group.iter().map(|f| view! { <li><code>{f.clone()}</code></li> }).collect::<Vec<_>>()}
                        </ul>
                    </div>
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}

#[component]
fn ExecutionPhases(phases: Vec<RefactorPhase>) -> impl IntoView {
    view! {
        <div class="execution-phases">
            {phases.into_iter().map(|phase| {
                let risk_class = format!("phase-card risk-{}", phase.risk.to_lowercase());
                let risk_icon = match phase.risk.to_lowercase().as_str() {
                    "low" => ICON_CHECK_CIRCLE,
                    "medium" => ICON_WARNING_CIRCLE,
                    _ => ICON_SIREN,
                };
                let git_script = phase.git_script.clone();

                view! {
                    <div class=risk_class>
                        <div class="phase-header" data-toggle="phase">
                            <span class="phase-toggle">"▼"</span>
                            <span class="phase-icon">
                                <Icon path=risk_icon class="icon-sm" />
                            </span>
                            <span class="phase-name">{phase.name.clone()}</span>
                            <span class="phase-count">{format!("({} files)", phase.moves.len())}</span>
                        </div>
                        <div class="phase-content">
                            <table class="moves-table">
                                <thead>
                                    <tr>
                                        <th>"File"</th>
                                        <th>"From"</th>
                                        <th>"To"</th>
                                        <th>"LOC"</th>
                                        <th>"Consumers"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {phase.moves.iter().map(|mv| {
                                        let filename = mv.source.split('/').next_back().unwrap_or(&mv.source);
                                        view! {
                                            <tr>
                                                <td><code>{filename.to_string()}</code></td>
                                                <td>{mv.current_layer.clone()}</td>
                                                <td>{mv.target_layer.clone()}</td>
                                                <td>{mv.loc}</td>
                                                <td>{mv.direct_consumers}</td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                            <div class="phase-commands">
                                <strong>"Commands:"</strong>
                                <pre><code>{git_script.clone()}</code></pre>
                                <button class="copy-btn" data-copy=git_script>"Copy"</button>
                            </div>
                        </div>
                    </div>
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}

#[component]
fn ShimmingStrategy(shims: Vec<RefactorShim>) -> impl IntoView {
    view! {
        <div class="panel shimming-strategy">
            <h4>"Shimming Strategy"</h4>
            <p class="muted">"Create re-export shims for backward compatibility:"</p>
            {shims.into_iter().map(|shim| {
                let code = shim.code.clone();
                view! {
                    <div class="shim-item">
                        <div class="shim-header">
                            <code>{shim.old_path.clone()}</code>
                            <span class="muted">{format!("({} importers)", shim.importer_count)}</span>
                        </div>
                        <pre class="shim-code"><code>{code.clone()}</code></pre>
                        <button class="copy-btn" data-copy=code>"Copy"</button>
                    </div>
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}
