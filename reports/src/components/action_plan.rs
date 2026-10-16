//! Action plan panel - first-shot tasks with fix + verify.

use leptos::prelude::*;

use crate::components::icons::{ICON_CLIPBOARD_LIST, Icon};
use crate::types::PriorityTask;

/// Action plan panel showing top priority tasks.
#[component]
pub fn ActionPlanPanel(tasks: Vec<PriorityTask>) -> impl IntoView {
    if tasks.is_empty() {
        return view! { "" }.into_any();
    }

    view! {
        <div class="panel action-plan-panel">
            <h3>
                <Icon path=ICON_CLIPBOARD_LIST class="icon-sm" />
                "Action Plan"
            </h3>
            <p class="muted">"First-pass tasks and review candidates with evidence and verification commands."</p>
            <ol class="action-list">
                {tasks.into_iter().map(|task| {
                    let risk_class = match task.risk.as_str() {
                        "high" => "risk-high",
                        "medium" => "risk-medium",
                        _ => "risk-low",
                    };
                    let verify_cmd = task.verify_cmd.clone();
                    let is_review = task.kind == "duplicate_candidate";
                    // A reading budget, not CSS clipping: full guidance remains
                    // explicitly expandable, including for older report producers.
                    let hint_chars = task.fix_hint.chars().count();
                    let preview = task.fix_hint.chars().take(360).collect::<String>();
                    let hint = if hint_chars > 360 {
                        view! {
                            <details>
                                <summary>{format!("{}… ({} more characters; expand full guidance)", preview, hint_chars - 360)}</summary>
                                <p>{task.fix_hint.clone()}</p>
                            </details>
                        }.into_any()
                    } else {
                        view! { <span>{task.fix_hint.clone()}</span> }.into_any()
                    };
                    view! {
                        <li class="action-item">
                            <div class="action-head">
                                <span class="action-priority">{format!("#{}", task.priority)}</span>
                                <code class="action-target">{task.target.clone()}</code>
                                <span class="action-kind">{task.kind.clone()}</span>
                                <span class=format!("action-risk {}", risk_class)>{task.risk}</span>
                            </div>
                            <div class="action-why">{task.why.clone()}</div>
                            <div class="action-fix">
                                <span class="action-label">{if is_review { "Review" } else { "Fix" }}</span>
                                {hint}
                            </div>
                            {is_review.then(|| view! {
                                <div class="tab-bar">
                                    <button type="button" class="tab-btn" data-tab="twins">"Full twin evidence"</button>
                                </div>
                            })}
                            <div class="action-verify">
                                <span class="action-label">"Verify"</span>
                                <code>{verify_cmd.clone()}</code>
                                <button class="copy-btn" data-copy=verify_cmd>"Copy"</button>
                            </div>
                            <div class="action-location">
                                <span class="action-label">"Location"</span>
                                <span>{task.location}</span>
                            </div>
                        </li>
                    }
                }).collect::<Vec<_>>()}
            </ol>
        </div>
    }
    .into_any()
}

// Tests live in reports/src/lib.rs via render_report.
