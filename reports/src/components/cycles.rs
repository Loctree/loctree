//! Circular imports (cycles) component
//!
//! Displays circular import chains detected in the codebase.
//! Differentiates between strict cycles (always circular) and
//! lazy cycles (circular only via lazy/dynamic imports).

use crate::components::icons::{ICON_WARNING_CIRCLE, Icon};
use leptos::prelude::*;

/// Displays circular import cycles with visual differentiation
/// between strict (structural) and lazy (review) cycles.
///
/// # Props
///
/// * `strict_cycles` - Vec of cycles of eager dependency edges (A → B → C → A)
/// * `lazy_cycles` - Vec of cycles only via lazy imports (can be resolved at runtime)
///
/// # Display
///
/// - Strict cycles shown as structural review evidence
/// - Lazy cycles shown in yellow/orange with ICON_WARNING_CIRCLE (warning)
/// - Each cycle rendered as: A → B → C → A
/// - Empty state when no cycles detected
///
/// # Example
///
/// ```rust,ignore
/// view! {
///     <Cycles
///         strict_cycles=vec![
///             vec!["a.ts".into(), "b.ts".into(), "a.ts".into()]
///         ]
///         lazy_cycles=vec![
///             vec!["x.ts".into(), "y.ts".into(), "x.ts".into()]
///         ]
///     />
/// }
/// ```
#[component]
pub fn Cycles(
    /// Strict dependency cycles (execution outcome unverified)
    strict_cycles: Vec<Vec<String>>,
    /// Lazy circular imports (via dynamic/lazy imports only)
    lazy_cycles: Vec<Vec<String>>,
) -> impl IntoView {
    let strict_count = strict_cycles.len();
    let lazy_count = lazy_cycles.len();
    let total_count = strict_count + lazy_count;

    // Empty state - no cycles detected
    if total_count == 0 {
        return view! {
            <div class="panel">
                <h3>
                    <Icon path=ICON_WARNING_CIRCLE color="#27ae60" />
                    "Circular Imports"
                    <span class="count-badge count-badge-success">"0"</span>
                </h3>
                <div class="cycles-empty">
                    <p class="muted">"No circular imports detected"</p>
                </div>
            </div>
        }
        .into_any();
    }

    view! {
        <div class="panel">
            <h3>
                <Icon path=ICON_WARNING_CIRCLE color="#e67e22" />
                "Circular Imports"
                <span class="count-badge count-badge-warning">{total_count.to_string()}</span>
            </h3>

            // Strict Cycles Section (Structural)
            {if strict_count > 0 {
                view! {
                    <div class="cycles-section cycles-section-strict">
                        <div class="cycles-section-header">
                            <Icon path=ICON_WARNING_CIRCLE color="#e67e22" />
                            <h4>"Strict Cycles"</h4>
                            <span class="count-badge count-badge-warning">{strict_count.to_string()}</span>
                        </div>
                        <p class="cycles-section-desc">
                            "Structural coupling: compiler/runtime outcome unverified. Review initialization order and value access."
                        </p>
                        <div class="cycles-list">
                            {strict_cycles.into_iter().enumerate().map(|(idx, cycle)| {
                                view! {
                                    <CycleItem cycle=cycle idx=idx is_lazy=false />
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    </div>
                }.into_any()
            } else {
                view! { "" }.into_any()
            }}

            // Lazy Cycles Section (Warning)
            {if lazy_count > 0 {
                view! {
                    <div class="cycles-section cycles-section-lazy">
                        <div class="cycles-section-header">
                            <Icon path=ICON_WARNING_CIRCLE color="#e67e22" />
                            <h4>"Lazy Cycles"</h4>
                            <span class="count-badge count-badge-warning">{lazy_count.to_string()}</span>
                        </div>
                        <p class="cycles-section-desc">
                            "Warning: Cycles exist only via lazy/dynamic imports. May still cause issues."
                        </p>
                        <div class="cycles-list">
                            {lazy_cycles.into_iter().enumerate().map(|(idx, cycle)| {
                                view! {
                                    <CycleItem cycle=cycle idx=idx is_lazy=true />
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    </div>
                }.into_any()
            } else {
                view! { "" }.into_any()
            }}
        </div>
    }
    .into_any()
}

/// Individual cycle item showing the circular path
#[component]
fn CycleItem(
    /// The cycle path (last element should equal first to show closure)
    cycle: Vec<String>,
    /// Index for numbering
    idx: usize,
    /// Whether this is a lazy cycle (changes styling)
    is_lazy: bool,
) -> impl IntoView {
    let cycle_class = if is_lazy {
        "cycle-item cycle-item-lazy"
    } else {
        "cycle-item cycle-item-strict"
    };

    // Format cycle as: file1 → file2 → file3 → file1
    let cycle_path = cycle.join(" → ");

    view! {
        <div class=cycle_class>
            <span class="cycle-number">{format!("#{}", idx + 1)}</span>
            <code class="cycle-path">{cycle_path}</code>
        </div>
    }
}
