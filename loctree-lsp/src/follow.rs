//! Custom LSP request: `loctree/follow`
//!
//! Consolidates the analyzer's structural-signal surface (cycles,
//! dead exports, twins, hotspots, commands, events, pipelines, ...)
//! under one verb so daemon-mode agents have one entry point for
//! "show me the structural smells in this scope" instead of reaching
//! for seven separate requests.
//!
//! Plan 15 of the LSP roadmap.
//!
//! ## Stage 2 truth pass (2026-05-08)
//!
//! Stage 2 wired `commands`, `events`, and `pipelines` from the data
//! that already lives in [`Snapshot::command_bridges`] and
//! [`Snapshot::event_bridges`]. Stage 3 wires `trace` through the analyzer's
//! handler trace engine so editor clients can inspect handler evidence through
//! this same gateway.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::collections::HashMap;
use std::path::PathBuf;

use loctree::analysis_reports::{
    CoverageReportOptions, HealthReportOptions, coverage_report, health_report,
};
use loctree::analyzer::coverage::CommandUsage;
use loctree::analyzer::coverage_gaps::{CoverageGap, Severity as CoverageSeverity};
use loctree::analyzer::trace::trace_handler;
use loctree::snapshot::{CommandBridge, EventBridge, Snapshot};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::cursor::{CursorError, CursorState};
use crate::protocol::{DEFAULT_CHUNK_SIZE, Paginated, paginate, single_page};

const DEFAULT_LIMIT: usize = 25;
const HOTSPOT_IMPORTERS_THRESHOLD: usize = 10;
const SNAPSHOT_ID_FALLBACK: &str = "snapshot:unknown";
const FOLLOW_ITEMS_CURSOR_KIND: &str = "loctree/follow.items";

/// Full advertised scope vocabulary for `loctree/follow`.
///
/// Stays as the wire contract list — every scope a client may pass.
/// To distinguish "advertised AND wired" from "advertised but stubbed"
/// (Stage 2 truth pass), see [`IMPLEMENTED_SCOPES`] / [`STUB_SCOPES`].
pub const SUPPORTED_SCOPES: &[&str] = &[
    "cycles",
    "dead",
    "twins",
    "hotspots",
    "coverage",
    "trace",
    "commands",
    "events",
    "pipelines",
    "all",
];

/// Scopes whose handler returns real, snapshot-derived data.
///
/// Capability advertisement should expose this list separately so
/// clients can probe without having to issue a request to discover
/// stub scopes the hard way.
pub const IMPLEMENTED_SCOPES: &[&str] = &[
    "cycles",
    "dead",
    "twins",
    "hotspots",
    "coverage",
    "trace",
    "commands",
    "events",
    "pipelines",
    "all",
];

/// Scopes whose handler returns the stub envelope from
/// [`FollowResponse::unsupported`]. Mirrors [`SUPPORTED_SCOPES`] minus
/// [`IMPLEMENTED_SCOPES`].
pub const STUB_SCOPES: &[&str] = &[];

/// Parameters for `loctree/follow`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FollowParams {
    pub scope: String,
    /// For `scope = "trace"` — the symbol / handler to trace.
    #[serde(default)]
    pub handler: Option<String>,
    /// Cap on emitted items per bucket (default 25).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Workspace project root override. Reserved for Plan 13
    /// (multi-workspace context).
    #[serde(default)]
    pub project: Option<PathBuf>,
    /// Opaque Plan 12 cursor returned by `items.next_cursor`.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Requested page size for the paginated item payload.
    #[serde(default)]
    pub chunk_size: Option<usize>,
}

/// Stable envelope around any scope's payload.
#[derive(Debug, Clone, Serialize)]
pub struct FollowResponse {
    pub scope: String,
    pub items: Paginated<Value>,
    pub summary: FollowSummary,
}

/// Compact summary so callers can route on severity without parsing
/// `items`.
#[derive(Debug, Clone, Serialize)]
pub struct FollowSummary {
    pub count: usize,
    /// `"low" | "medium" | "high"`.
    pub severity: String,
    /// Optional human-readable message (used for stub scopes / errors).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl FollowResponse {
    pub fn unsupported(scope: &str) -> Self {
        Self {
            scope: scope.to_string(),
            items: single_page(json!([])),
            summary: FollowSummary {
                count: 0,
                severity: "low".into(),
                message: Some(format!(
                    "scope `{scope}` is not implemented in loctree-lsp yet — request the same data via `loct {scope}` for now"
                )),
            },
        }
    }

    pub fn unknown(scope: &str) -> Self {
        Self {
            scope: scope.to_string(),
            items: single_page(json!([])),
            summary: FollowSummary {
                count: 0,
                severity: "low".into(),
                message: Some(format!(
                    "unknown scope `{scope}` — supported: cycles, dead, twins, hotspots, coverage, trace, commands, events, pipelines, all"
                )),
            },
        }
    }
}

/// Severity classifier for a single bucket count.
///
/// - `low` < 5 items
/// - `medium` 5..=20 items
/// - `high` > 20 items
pub fn severity_for_count(count: usize) -> &'static str {
    if count > 20 {
        "high"
    } else if count >= 5 {
        "medium"
    } else {
        "low"
    }
}

/// Top-level dispatcher.
pub fn compute(
    snapshot: &Snapshot,
    root: &std::path::Path,
    params: &FollowParams,
) -> FollowResponse {
    compute_paginated(snapshot, root, params, SNAPSHOT_ID_FALLBACK)
        .expect("default follow pagination should not fail")
}

/// Top-level dispatcher with Plan 12 cursor support.
pub fn compute_paginated(
    snapshot: &Snapshot,
    root: &std::path::Path,
    params: &FollowParams,
    snapshot_id: &str,
) -> Result<FollowResponse, CursorError> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    let chunk_size = params.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);
    let offset = follow_offset(params.cursor.as_deref(), snapshot_id)?;
    match params.scope.as_str() {
        "cycles" => Ok(follow_cycles(snapshot, root)),
        "dead" => follow_dead(snapshot, root, limit, offset, chunk_size, snapshot_id),
        "twins" => follow_twins(snapshot, root, limit, offset, chunk_size, snapshot_id),
        "hotspots" => follow_hotspots(snapshot, limit, offset, chunk_size, snapshot_id),
        "coverage" => follow_coverage(snapshot, limit, offset, chunk_size, snapshot_id),
        "commands" => follow_commands(snapshot, limit, offset, chunk_size, snapshot_id),
        "events" => follow_events(snapshot, limit, offset, chunk_size, snapshot_id),
        "pipelines" => follow_pipelines(snapshot, limit, offset, chunk_size, snapshot_id),
        "trace" => Ok(follow_trace(snapshot, params.handler.as_deref())),
        "all" => follow_all(snapshot, root, limit, offset, chunk_size, snapshot_id),
        _ => Ok(FollowResponse::unknown(params.scope.as_str())),
    }
}

fn follow_offset(cursor: Option<&str>, snapshot_id: &str) -> Result<usize, CursorError> {
    let Some(token) = cursor else {
        return Ok(0);
    };
    let state = CursorState::decode(token, snapshot_id, FOLLOW_ITEMS_CURSOR_KIND)?;
    Ok(state.offset)
}

fn paginated_object<T>(
    fields: Map<String, Value>,
    collection_key: &str,
    entries: &[T],
    offset: usize,
    chunk_size: usize,
    snapshot_id: &str,
) -> Result<Paginated<Value>, CursorError>
where
    T: Clone + Serialize,
{
    let page = paginate(
        entries,
        offset,
        chunk_size,
        snapshot_id,
        FOLLOW_ITEMS_CURSOR_KIND,
    )?;
    let mut data = fields;
    data.insert(
        collection_key.to_string(),
        serde_json::to_value(page.data).expect("follow item page should serialize"),
    );
    Ok(Paginated {
        chunk: page.chunk,
        total_chunks: page.total_chunks,
        next_cursor: page.next_cursor,
        data: Value::Object(data),
        advisory: page.advisory,
    })
}

fn follow_cycles(snapshot: &Snapshot, root: &std::path::Path) -> FollowResponse {
    let report = health_report(snapshot, root, HealthReportOptions::default());
    let total = report.cycles.total;
    let items = single_page(json!({
        "total": total,
        "high_risk": report.cycles.high_risk,
        "structural": report.cycles.structural,
    }));
    FollowResponse {
        scope: "cycles".into(),
        items,
        summary: FollowSummary {
            count: total,
            severity: severity_for_count(total).into(),
            message: None,
        },
    }
}

fn follow_dead(
    snapshot: &Snapshot,
    root: &std::path::Path,
    limit: usize,
    offset: usize,
    chunk_size: usize,
    snapshot_id: &str,
) -> Result<FollowResponse, CursorError> {
    let report = health_report(snapshot, root, HealthReportOptions::default());
    let total = report.dead_exports.total;
    let mut top_files = report.dead_exports.top_files.clone();
    top_files.truncate(limit);
    let mut fields = Map::new();
    fields.insert("total".into(), json!(total));
    fields.insert(
        "high_confidence".into(),
        json!(report.dead_exports.high_confidence),
    );
    fields.insert(
        "low_confidence".into(),
        json!(report.dead_exports.low_confidence),
    );
    let items = paginated_object(
        fields,
        "top_files",
        &top_files,
        offset,
        chunk_size,
        snapshot_id,
    )?;
    Ok(FollowResponse {
        scope: "dead".into(),
        items,
        summary: FollowSummary {
            count: total,
            severity: severity_for_count(total).into(),
            message: None,
        },
    })
}

fn follow_twins(
    snapshot: &Snapshot,
    root: &std::path::Path,
    limit: usize,
    offset: usize,
    chunk_size: usize,
    snapshot_id: &str,
) -> Result<FollowResponse, CursorError> {
    let report = health_report(snapshot, root, HealthReportOptions::default());
    let total = report.twins.total;
    let mut top_groups = report.twins.top_groups.clone();
    top_groups.truncate(limit);
    let mut fields = Map::new();
    fields.insert("total".into(), json!(total));
    let items = paginated_object(
        fields,
        "top_groups",
        &top_groups,
        offset,
        chunk_size,
        snapshot_id,
    )?;
    Ok(FollowResponse {
        scope: "twins".into(),
        items,
        summary: FollowSummary {
            count: total,
            severity: severity_for_count(total).into(),
            message: None,
        },
    })
}

fn hotspot_entries(snapshot: &Snapshot) -> Vec<Value> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for edge in &snapshot.edges {
        *counts.entry(edge.to.as_str()).or_insert(0) += 1;
    }
    let mut hotspots: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, c)| *c >= HOTSPOT_IMPORTERS_THRESHOLD)
        .map(|(file, c)| (file.to_string(), c))
        .collect();
    hotspots.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    hotspots
        .iter()
        .map(|(file, c)| json!({ "file": file, "importers": c }))
        .collect()
}

fn follow_hotspots(
    snapshot: &Snapshot,
    limit: usize,
    offset: usize,
    chunk_size: usize,
    snapshot_id: &str,
) -> Result<FollowResponse, CursorError> {
    let mut entries = hotspot_entries(snapshot);
    let total = entries.len();
    entries.truncate(limit);
    let mut fields = Map::new();
    fields.insert("total".into(), json!(total));
    fields.insert("threshold".into(), json!(HOTSPOT_IMPORTERS_THRESHOLD));
    let items = paginated_object(fields, "items", &entries, offset, chunk_size, snapshot_id)?;

    Ok(FollowResponse {
        scope: "hotspots".into(),
        items,
        summary: FollowSummary {
            count: total,
            severity: severity_for_count(total).into(),
            message: None,
        },
    })
}

fn coverage_gaps(snapshot: &Snapshot) -> Vec<CoverageGap> {
    coverage_report(
        snapshot,
        CoverageReportOptions {
            include_gaps: true,
            include_tests: false,
            ..CoverageReportOptions::default()
        },
    )
    .gaps
}

fn coverage_counts(gaps: &[CoverageGap]) -> (usize, usize, usize, usize) {
    let mut critical = 0;
    let mut high = 0;
    let mut medium = 0;
    let mut low = 0;
    for gap in gaps {
        match gap.severity {
            CoverageSeverity::Critical => critical += 1,
            CoverageSeverity::High => high += 1,
            CoverageSeverity::Medium => medium += 1,
            CoverageSeverity::Low => low += 1,
        }
    }
    (critical, high, medium, low)
}

fn coverage_summary_severity(
    critical: usize,
    high: usize,
    medium: usize,
    _low: usize,
) -> &'static str {
    if critical > 0 || high > 0 {
        "high"
    } else if medium > 0 {
        "medium"
    } else {
        "low"
    }
}

fn follow_coverage(
    snapshot: &Snapshot,
    limit: usize,
    offset: usize,
    chunk_size: usize,
    snapshot_id: &str,
) -> Result<FollowResponse, CursorError> {
    let mut gaps = coverage_gaps(snapshot);
    let total = gaps.len();
    let (critical, high, medium, low) = coverage_counts(&gaps);
    gaps.truncate(limit);
    let mut fields = Map::new();
    fields.insert("total".into(), json!(total));
    fields.insert("critical".into(), json!(critical));
    fields.insert("high".into(), json!(high));
    fields.insert("medium".into(), json!(medium));
    fields.insert("low".into(), json!(low));
    let items = paginated_object(fields, "items", &gaps, offset, chunk_size, snapshot_id)?;

    Ok(FollowResponse {
        scope: "coverage".into(),
        items,
        summary: FollowSummary {
            count: total,
            severity: coverage_summary_severity(critical, high, medium, low).into(),
            message: None,
        },
    })
}

fn follow_trace(snapshot: &Snapshot, handler: Option<&str>) -> FollowResponse {
    let Some(handler) = handler.filter(|h| !h.trim().is_empty()) else {
        return FollowResponse {
            scope: "trace".into(),
            items: single_page(json!([])),
            summary: FollowSummary {
                count: 0,
                severity: "low".into(),
                message: Some("scope `trace` requires a non-empty `handler` parameter".into()),
            },
        };
    };

    let fe_commands = command_usage_from_bridges(snapshot);
    let be_commands = CommandUsage::new();
    let registered_handlers = snapshot
        .files
        .iter()
        .flat_map(|file| file.tauri_registered_handlers.iter().cloned())
        .collect();
    let result = trace_handler(
        handler,
        &snapshot.files,
        &fe_commands,
        &be_commands,
        &registered_handlers,
    );
    let count = usize::from(result.backend.is_some())
        + result.frontend_invokes.len()
        + result.frontend_mentions.len();
    let severity = if result.verdict.contains("CONNECTED") {
        "low"
    } else {
        "high"
    };

    FollowResponse {
        scope: "trace".into(),
        items: single_page(serde_json::to_value(result).expect("trace result should serialize")),
        summary: FollowSummary {
            count,
            severity: severity.into(),
            message: None,
        },
    }
}

fn command_usage_from_bridges(snapshot: &Snapshot) -> CommandUsage {
    let mut usage = CommandUsage::new();
    for bridge in &snapshot.command_bridges {
        for (file, line) in &bridge.frontend_calls {
            usage.entry(bridge.name.clone()).or_default().push((
                file.clone(),
                *line,
                bridge.name.clone(),
            ));
        }
    }
    usage
}

/// Project [`Snapshot::command_bridges`] onto the wire shape.
///
/// `count` reports the **total** number of bridges in the snapshot;
/// the `items` array is sorted by descending invoke-site count and
/// then truncated to `limit`. `severity` is derived from `count`,
/// not from the truncated slice, so the operator never sees "low"
/// for a snapshot that actually carries hundreds of commands.
fn command_entries(snapshot: &Snapshot) -> (Vec<Value>, usize, usize, usize) {
    let mut bridges: Vec<&CommandBridge> = snapshot.command_bridges.iter().collect();
    bridges.sort_by(|a, b| {
        b.frontend_calls
            .len()
            .cmp(&a.frontend_calls.len())
            .then_with(|| a.name.cmp(&b.name))
    });
    let total = bridges.len();
    let unhandled: usize = bridges.iter().filter(|b| !b.has_handler).count();
    let uncalled: usize = bridges.iter().filter(|b| !b.is_called).count();

    let entries: Vec<Value> = bridges
        .into_iter()
        .map(|bridge| {
            json!({
                "name": bridge.name,
                "has_handler": bridge.has_handler,
                "is_called": bridge.is_called,
                "invoke_sites": bridge.frontend_calls.len(),
                "handler": bridge.backend_handler.as_ref().map(|(file, line)| json!({
                    "file": file,
                    "line": line,
                })),
            })
        })
        .collect();
    (entries, total, unhandled, uncalled)
}

fn follow_commands(
    snapshot: &Snapshot,
    limit: usize,
    offset: usize,
    chunk_size: usize,
    snapshot_id: &str,
) -> Result<FollowResponse, CursorError> {
    let (mut entries, total, unhandled, uncalled) = command_entries(snapshot);
    entries.truncate(limit);
    let mut fields = Map::new();
    fields.insert("total".into(), json!(total));
    fields.insert("unhandled".into(), json!(unhandled));
    fields.insert("uncalled".into(), json!(uncalled));
    let items = paginated_object(fields, "items", &entries, offset, chunk_size, snapshot_id)?;

    Ok(FollowResponse {
        scope: "commands".into(),
        items,
        summary: FollowSummary {
            count: total,
            severity: severity_for_count(total).into(),
            message: None,
        },
    })
}

/// Project [`Snapshot::event_bridges`] onto the wire shape.
///
/// `count` reports the total number of events; `items` is sorted by
/// descending `(emit_count + listen_count)` so the most-active events
/// surface first.
fn event_entries(snapshot: &Snapshot) -> (Vec<Value>, usize, usize, usize, usize) {
    let mut bridges: Vec<&EventBridge> = snapshot.event_bridges.iter().collect();
    bridges.sort_by(|a, b| {
        let sum_b = b.emits.len() + b.listens.len();
        let sum_a = a.emits.len() + a.listens.len();
        sum_b.cmp(&sum_a).then_with(|| a.name.cmp(&b.name))
    });
    let total = bridges.len();
    let ghosts: usize = bridges
        .iter()
        .filter(|e| !e.emits.is_empty() && e.listens.is_empty())
        .count();
    let orphans: usize = bridges
        .iter()
        .filter(|e| e.emits.is_empty() && !e.listens.is_empty())
        .count();
    let fe_sync: usize = bridges.iter().filter(|e| e.is_fe_sync).count();

    let entries: Vec<Value> = bridges
        .into_iter()
        .map(|bridge| {
            json!({
                "name": bridge.name,
                "emit_count": bridge.emits.len(),
                "listen_count": bridge.listens.len(),
                "is_fe_sync": bridge.is_fe_sync,
                "same_file_sync": bridge.same_file_sync,
            })
        })
        .collect();
    (entries, total, ghosts, orphans, fe_sync)
}

fn follow_events(
    snapshot: &Snapshot,
    limit: usize,
    offset: usize,
    chunk_size: usize,
    snapshot_id: &str,
) -> Result<FollowResponse, CursorError> {
    let (mut entries, total, ghosts, orphans, fe_sync) = event_entries(snapshot);
    entries.truncate(limit);
    let mut fields = Map::new();
    fields.insert("total".into(), json!(total));
    fields.insert("ghost_emits".into(), json!(ghosts));
    fields.insert("orphan_listeners".into(), json!(orphans));
    fields.insert("fe_sync".into(), json!(fe_sync));
    let items = paginated_object(fields, "items", &entries, offset, chunk_size, snapshot_id)?;

    Ok(FollowResponse {
        scope: "events".into(),
        items,
        summary: FollowSummary {
            count: total,
            severity: severity_for_count(total).into(),
            message: None,
        },
    })
}

/// Reduce the snapshot's command + event bridges to a "pipelines"
/// view — the same surface `loct pipelines` exposes, minus the rich
/// payload analysis (which depends on FE/BE command-usage maps that
/// only the analyzer build path constructs).
///
/// The LSP path is honest about that boundary: it carries
/// `note: "structural pipeline view from snapshot bridges; run \
/// `loct pipelines` for full ghost/orphan/race analysis"` so agents
/// know when to escalate to the CLI for deeper analysis.
fn pipeline_entries(snapshot: &Snapshot) -> (Vec<Value>, usize, usize, usize) {
    let mut events: Vec<&EventBridge> = snapshot.event_bridges.iter().collect();
    events.sort_by(|a, b| {
        let pri_b = (b.emits.is_empty(), b.listens.is_empty(), b.name.clone());
        let pri_a = (a.emits.is_empty(), a.listens.is_empty(), a.name.clone());
        pri_a.cmp(&pri_b)
    });
    let event_total = events.len();
    let ghost_emits: usize = events
        .iter()
        .filter(|e| !e.emits.is_empty() && e.listens.is_empty())
        .count();
    let orphan_listeners: usize = events
        .iter()
        .filter(|e| e.emits.is_empty() && !e.listens.is_empty())
        .count();

    let unhandled_commands: Vec<&CommandBridge> = snapshot
        .command_bridges
        .iter()
        .filter(|c| !c.has_handler && c.is_called)
        .collect();
    let uncalled_commands: Vec<&CommandBridge> = snapshot
        .command_bridges
        .iter()
        .filter(|c| c.has_handler && !c.is_called)
        .collect();

    let mut entries: Vec<Value> = events
        .iter()
        .map(|bridge| {
            let status = if !bridge.emits.is_empty() && !bridge.listens.is_empty() {
                "ok"
            } else if !bridge.emits.is_empty() {
                "ghost"
            } else if !bridge.listens.is_empty() {
                "orphan"
            } else {
                "empty"
            };
            json!({
                "name": bridge.name,
                "status": status,
                "emit_count": bridge.emits.len(),
                "listen_count": bridge.listens.len(),
                "kind": "event",
            })
        })
        .collect();

    entries.extend(unhandled_commands.iter().map(|c| {
        json!({
            "kind": "unhandled_command",
            "name": c.name,
            "invoke_sites": c.frontend_calls.len(),
        })
    }));

    entries.extend(uncalled_commands.iter().map(|c| {
        json!({
            "kind": "uncalled_command",
            "name": c.name,
            "handler_file": c.backend_handler.as_ref().map(|(f, _)| f.clone()),
        })
    }));

    let issue_count = ghost_emits + orphan_listeners + unhandled_commands.len();
    (
        entries,
        event_total,
        issue_count,
        ghost_emits + orphan_listeners,
    )
}

fn follow_pipelines(
    snapshot: &Snapshot,
    limit: usize,
    offset: usize,
    chunk_size: usize,
    snapshot_id: &str,
) -> Result<FollowResponse, CursorError> {
    let (mut entries, event_total, issue_count, event_issues) = pipeline_entries(snapshot);
    entries.truncate(limit);
    let mut fields = Map::new();
    fields.insert("event_total".into(), json!(event_total));
    fields.insert(
        "command_total".into(),
        json!(snapshot.command_bridges.len()),
    );
    fields.insert("event_issues".into(), json!(event_issues));
    fields.insert(
        "note".into(),
        json!(
            "structural pipeline view from snapshot bridges; run `loct pipelines` for full ghost/orphan/race analysis"
        ),
    );
    let items = paginated_object(fields, "items", &entries, offset, chunk_size, snapshot_id)?;

    Ok(FollowResponse {
        scope: "pipelines".into(),
        items,
        summary: FollowSummary {
            count: issue_count,
            severity: severity_for_count(issue_count).into(),
            message: None,
        },
    })
}

fn follow_all(
    snapshot: &Snapshot,
    root: &std::path::Path,
    limit: usize,
    offset: usize,
    chunk_size: usize,
    snapshot_id: &str,
) -> Result<FollowResponse, CursorError> {
    let report = health_report(snapshot, root, HealthReportOptions::default());
    let cycles_count = report.cycles.total;
    let dead_count = report.dead_exports.total;
    let twins_count = report.twins.total;
    let mut entries = Vec::new();

    entries.push(json!({
        "scope": "cycles",
        "item": {
            "total": cycles_count,
            "high_risk": report.cycles.high_risk,
            "structural": report.cycles.structural,
        }
    }));
    entries.extend(
        report
            .dead_exports
            .top_files
            .iter()
            .take(limit)
            .map(|item| json!({ "scope": "dead", "item": item })),
    );
    entries.extend(
        report
            .twins
            .top_groups
            .iter()
            .take(limit)
            .map(|item| json!({ "scope": "twins", "item": item })),
    );
    entries.extend(
        hotspot_entries(snapshot)
            .into_iter()
            .take(limit)
            .map(|item| json!({ "scope": "hotspots", "item": item })),
    );
    let coverage_items = coverage_gaps(snapshot);
    let coverage_count = coverage_items.len();
    entries.extend(
        coverage_items
            .into_iter()
            .take(limit)
            .map(|item| json!({ "scope": "coverage", "item": item })),
    );
    entries.extend(
        command_entries(snapshot)
            .0
            .into_iter()
            .take(limit)
            .map(|item| json!({ "scope": "commands", "item": item })),
    );
    entries.extend(
        event_entries(snapshot)
            .0
            .into_iter()
            .take(limit)
            .map(|item| json!({ "scope": "events", "item": item })),
    );
    let (pipeline_items, _, pipeline_count, _) = pipeline_entries(snapshot);
    entries.extend(
        pipeline_items
            .into_iter()
            .take(limit)
            .map(|item| json!({ "scope": "pipelines", "item": item })),
    );

    let hotspot_count = hotspot_entries(snapshot).len();
    let command_count = snapshot.command_bridges.len();
    let event_count = snapshot.event_bridges.len();
    let total = cycles_count
        + dead_count
        + twins_count
        + hotspot_count
        + coverage_count
        + command_count
        + event_count
        + pipeline_count;
    let severity = severity_for_count(total).to_string();
    let mut fields = Map::new();
    fields.insert(
        "scope_totals".into(),
        json!({
            "cycles": cycles_count,
            "dead": dead_count,
            "twins": twins_count,
            "hotspots": hotspot_count,
            "coverage": coverage_count,
            "commands": command_count,
            "events": event_count,
            "pipelines": pipeline_count,
        }),
    );
    let items = paginated_object(fields, "items", &entries, offset, chunk_size, snapshot_id)?;

    Ok(FollowResponse {
        scope: "all".into(),
        items,
        summary: FollowSummary {
            count: total,
            severity,
            message: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_scopes_match_protocol_contract() {
        for scope in [
            "cycles",
            "dead",
            "twins",
            "hotspots",
            "coverage",
            "trace",
            "commands",
            "events",
            "pipelines",
            "all",
        ] {
            assert!(
                SUPPORTED_SCOPES.contains(&scope),
                "scope missing from SUPPORTED_SCOPES: {scope}"
            );
        }
        assert_eq!(SUPPORTED_SCOPES.len(), 10);
    }

    #[test]
    fn implemented_and_stub_scopes_partition_supported_scopes() {
        let mut union: Vec<&'static str> = IMPLEMENTED_SCOPES
            .iter()
            .copied()
            .chain(STUB_SCOPES.iter().copied())
            .collect();
        union.sort();
        let mut advertised: Vec<&'static str> = SUPPORTED_SCOPES.to_vec();
        advertised.sort();
        assert_eq!(union, advertised);

        // No scope can be both implemented and stub at the same time.
        for scope in IMPLEMENTED_SCOPES {
            assert!(
                !STUB_SCOPES.contains(scope),
                "scope `{scope}` cannot be in both IMPLEMENTED_SCOPES and STUB_SCOPES"
            );
        }
    }

    #[test]
    fn no_supported_scopes_are_stubbed() {
        assert!(STUB_SCOPES.is_empty());
        assert!(IMPLEMENTED_SCOPES.contains(&"trace"));
        assert!(IMPLEMENTED_SCOPES.contains(&"coverage"));
    }

    #[test]
    fn severity_thresholds() {
        assert_eq!(severity_for_count(0), "low");
        assert_eq!(severity_for_count(4), "low");
        assert_eq!(severity_for_count(5), "medium");
        assert_eq!(severity_for_count(20), "medium");
        assert_eq!(severity_for_count(21), "high");
        assert_eq!(severity_for_count(1000), "high");
    }

    #[test]
    fn unknown_scope_returns_diagnostic_response() {
        let r = FollowResponse::unknown("turbofish");
        assert_eq!(r.scope, "turbofish");
        assert_eq!(r.summary.count, 0);
        assert_eq!(r.summary.severity, "low");
        assert!(r.summary.message.unwrap().contains("unknown scope"));
        assert_eq!(r.items.data, json!([]));
        assert!(r.items.next_cursor.is_none());
    }

    #[test]
    fn unsupported_scope_returns_stub_response() {
        let r = FollowResponse::unsupported("trace");
        assert_eq!(r.scope, "trace");
        assert_eq!(r.summary.count, 0);
        assert!(r.summary.message.unwrap().contains("not implemented"));
    }

    #[test]
    fn follow_commands_projects_snapshot_bridges() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        snapshot.command_bridges.push(CommandBridge {
            name: "get_user".into(),
            frontend_calls: vec![("src/app.tsx".into(), 14)],
            backend_handler: Some(("src-tauri/src/lib.rs".into(), 42)),
            has_handler: true,
            is_called: true,
        });
        snapshot.command_bridges.push(CommandBridge {
            name: "ghost_cmd".into(),
            frontend_calls: vec![("src/app.tsx".into(), 81)],
            backend_handler: None,
            has_handler: false,
            is_called: true,
        });

        let resp = follow_commands(&snapshot, 50, 0, 50, "test").unwrap();
        assert_eq!(resp.scope, "commands");
        assert_eq!(resp.summary.count, 2);
        let total = resp
            .items
            .data
            .get("total")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(total, 2);
        let unhandled = resp
            .items
            .data
            .get("unhandled")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(unhandled, 1);
        let entries = resp.items.data.get("items").unwrap().as_array().unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn follow_events_classifies_ghost_and_orphan() {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        snapshot.event_bridges.push(EventBridge {
            name: "user_updated".into(),
            emits: vec![("src/app.ts".into(), 10, "emit".into())],
            listens: vec![("src/profile.ts".into(), 22)],
            is_fe_sync: true,
            same_file_sync: false,
        });
        snapshot.event_bridges.push(EventBridge {
            name: "ghost_event".into(),
            emits: vec![("src/app.ts".into(), 30, "emit".into())],
            listens: Vec::new(),
            is_fe_sync: false,
            same_file_sync: false,
        });
        snapshot.event_bridges.push(EventBridge {
            name: "orphan_event".into(),
            emits: Vec::new(),
            listens: vec![("src/app.ts".into(), 50)],
            is_fe_sync: false,
            same_file_sync: false,
        });

        let resp = follow_events(&snapshot, 50, 0, 50, "test").unwrap();
        assert_eq!(resp.scope, "events");
        assert_eq!(resp.summary.count, 3);
        assert_eq!(
            resp.items
                .data
                .get("ghost_emits")
                .and_then(|v| v.as_u64())
                .unwrap(),
            1
        );
        assert_eq!(
            resp.items
                .data
                .get("orphan_listeners")
                .and_then(|v| v.as_u64())
                .unwrap(),
            1
        );
        assert_eq!(
            resp.items
                .data
                .get("fe_sync")
                .and_then(|v| v.as_u64())
                .unwrap(),
            1
        );
    }

    #[test]
    fn follow_pipelines_carries_provenance_note() {
        let snapshot = Snapshot::new(vec![".".to_string()]);
        let resp = follow_pipelines(&snapshot, 50, 0, 50, "test").unwrap();
        assert_eq!(resp.scope, "pipelines");
        let note = resp
            .items
            .data
            .get("note")
            .and_then(|v| v.as_str())
            .unwrap();
        assert!(note.contains("loct pipelines"));
    }

    #[test]
    fn params_deserialize_minimal() {
        let value = json!({ "scope": "cycles" });
        let params: FollowParams = serde_json::from_value(value).unwrap();
        assert_eq!(params.scope, "cycles");
        assert!(params.limit.is_none());
        assert!(params.handler.is_none());
        assert!(params.project.is_none());
        assert!(params.cursor.is_none());
        assert!(params.chunk_size.is_none());
    }

    #[test]
    fn params_deserialize_full() {
        let value = json!({
            "scope": "trace",
            "handler": "auth_user",
            "limit": 50,
            "project": "/abs/repo",
            "cursor": "opaque",
            "chunk_size": 30
        });
        let params: FollowParams = serde_json::from_value(value).unwrap();
        assert_eq!(params.scope, "trace");
        assert_eq!(params.handler.as_deref(), Some("auth_user"));
        assert_eq!(params.limit, Some(50));
        assert_eq!(params.cursor.as_deref(), Some("opaque"));
        assert_eq!(params.chunk_size, Some(30));
        assert_eq!(
            params
                .project
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            Some("/abs/repo".into())
        );
    }
}
