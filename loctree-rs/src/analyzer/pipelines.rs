use std::collections::{HashMap, HashSet};

use globset::GlobSet;
use serde_json::json;

use crate::analyzer::coverage::CommandUsage;
use crate::types::{FileAnalysis, PayloadMap};
use once_cell::sync::Lazy;
use std::env;

fn normalize_event(name: &str) -> String {
    resolve_event_alias(name)
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c == '*' {
                // Keep wildcards for dynamic pattern matching
                '*'
            } else {
                '_'
            }
        })
        .collect::<String>()
}

/// Check if two event patterns match, considering wildcards (*) for dynamic patterns.
/// Examples:
///   - "event" matches "event" (exact)
///   - "event_*" matches "event_123" (pattern)
///   - "event_*" matches "event_*" (same pattern)
fn events_match(pattern1: &str, pattern2: &str) -> bool {
    // Exact match
    if pattern1 == pattern2 {
        return true;
    }

    // Check if either is a dynamic pattern (contains *)
    if !pattern1.contains('*') && !pattern2.contains('*') {
        return false;
    }

    // Convert patterns to regex-like matching
    let p1_parts: Vec<&str> = pattern1.split('*').collect();
    let p2_parts: Vec<&str> = pattern2.split('*').collect();

    // If both have same structure and non-wildcard parts match, they match
    if p1_parts.len() == p2_parts.len() {
        return p1_parts.iter().zip(p2_parts.iter()).all(|(a, b)| a == b);
    }

    false
}

/// Optional event aliasing via env `LOCT_EVENT_ALIASES` (comma or semicolon separated `old=new`).
/// Helps bridge cross-language or naming-drifted events (e.g., rust://foo=tauri://foo).
static EVENT_ALIASES: Lazy<std::collections::HashMap<String, String>> = Lazy::new(|| {
    let mut map = std::collections::HashMap::new();
    if let Ok(raw) = env::var("LOCT_EVENT_ALIASES") {
        for pair in raw.split([',', ';']) {
            let trimmed = pair.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some((from, to)) = trimmed.split_once('=') {
                map.insert(from.trim().to_lowercase(), to.trim().to_string());
            }
        }
    }
    map
});

fn resolve_event_alias(name: &str) -> String {
    let lower = name.to_lowercase();
    if let Some(alias) = EVENT_ALIASES.get(&lower) {
        alias.clone()
    } else {
        name.to_string()
    }
}

fn is_in_scope(path: &str, focus: &Option<GlobSet>, exclude: &Option<GlobSet>) -> bool {
    let pb = std::path::Path::new(path);
    if let Some(ex) = exclude
        && ex.is_match(pb)
    {
        return false;
    }
    if let Some(focus_globs) = focus
        && !focus_globs.is_match(pb)
    {
        return false;
    }
    true
}

pub fn build_pipeline_summary(
    analyses: &[FileAnalysis],
    focus: &Option<GlobSet>,
    exclude: &Option<GlobSet>,
    fe_commands: &CommandUsage,
    be_commands: &CommandUsage,
    fe_payloads: &PayloadMap,
    be_payloads: &PayloadMap,
) -> serde_json::Value {
    #[derive(Clone)]
    struct Site {
        norm: String,
        raw: String,
        path: String,
        line: usize,
        awaited: bool,
        payload: Option<String>,
    }

    #[derive(Default, Clone)]
    struct EventRecord {
        raw_names: HashSet<String>,
        emitters: Vec<Site>,
        listeners: Vec<Site>,
    }

    let mut events: HashMap<String, EventRecord> = HashMap::new();
    let mut path_emit_map: HashMap<String, Vec<Site>> = HashMap::new();

    for analysis in analyses {
        let path = analysis.path.clone();
        if !is_in_scope(&path, focus, exclude) {
            continue;
        }
        for ev in &analysis.event_emits {
            let raw_display = ev.raw_name.clone().unwrap_or_else(|| ev.name.clone());
            let norm = normalize_event(&ev.name);
            let site = Site {
                norm: norm.clone(),
                raw: raw_display.clone(),
                path: path.clone(),
                line: ev.line,
                awaited: ev.awaited,
                payload: ev.payload.clone(),
            };
            path_emit_map
                .entry(path.clone())
                .or_default()
                .push(site.clone());
            let rec = events.entry(norm).or_default();
            rec.raw_names.insert(raw_display);
            rec.emitters.push(site);
        }
        for ev in &analysis.event_listens {
            let raw_display = ev.raw_name.clone().unwrap_or_else(|| ev.name.clone());
            let norm = normalize_event(&ev.name);
            let site = Site {
                norm: norm.clone(),
                raw: raw_display.clone(),
                path: path.clone(),
                line: ev.line,
                awaited: ev.awaited,
                payload: ev.payload.clone(),
            };
            let rec = events.entry(norm).or_default();
            rec.raw_names.insert(raw_display);
            rec.listeners.push(site);
        }
    }

    // Post-process: match dynamic patterns with each other
    // If we have "event:*" emits and "event:*" listens, they should match
    let event_keys: Vec<String> = events.keys().cloned().collect();
    let mut matched_patterns: HashMap<String, String> = HashMap::new(); // pattern -> canonical

    for i in 0..event_keys.len() {
        for j in (i + 1)..event_keys.len() {
            let key1 = &event_keys[i];
            let key2 = &event_keys[j];

            if events_match(key1, key2) {
                // Merge these two event patterns
                let canonical = if key1.contains('*') && !key2.contains('*') {
                    key2.clone()
                } else if !key1.contains('*') && key2.contains('*') {
                    key1.clone()
                } else {
                    // Both are patterns or both are static, use the first one
                    key1.clone()
                };

                matched_patterns.insert(key1.clone(), canonical.clone());
                matched_patterns.insert(key2.clone(), canonical);
            }
        }
    }

    // Merge matched event records
    let mut merged_events: HashMap<String, EventRecord> = HashMap::new();
    for (key, mut rec) in events {
        let canonical = matched_patterns.get(&key).unwrap_or(&key).clone();
        let entry = merged_events.entry(canonical).or_default();
        entry.raw_names.extend(rec.raw_names);
        entry.emitters.append(&mut rec.emitters);
        entry.listeners.append(&mut rec.listeners);
    }

    let mut event_items = Vec::new();
    let mut ghost_emits = Vec::new();
    let mut orphan_listeners = Vec::new();
    let mut risks = Vec::new();
    let mut call_payloads: PayloadMap = HashMap::new();
    let mut handler_payloads: PayloadMap = HashMap::new();
    let mut string_literals_by_path: HashMap<String, Vec<(String, usize)>> = HashMap::new();
    for analysis in analyses {
        let lits: Vec<(String, usize)> = analysis
            .string_literals
            .iter()
            .map(|s| (normalize_event(&s.value), s.line))
            .collect();
        string_literals_by_path.insert(analysis.path.clone(), lits);
    }

    for (name, entries) in fe_payloads {
        call_payloads
            .entry(name.clone())
            .or_default()
            .extend(entries.clone());
    }
    for (name, entries) in be_payloads {
        handler_payloads
            .entry(name.clone())
            .or_default()
            .extend(entries.clone());
    }

    for (norm, rec) in &merged_events {
        let mut emitters = rec.emitters.clone();
        emitters.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
        let mut listeners = rec.listeners.clone();
        listeners.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));

        let has_emit = !emitters.is_empty();
        let has_listen = !listeners.is_empty();
        let mut status = match (has_emit, has_listen) {
            (true, true) => "ok",
            (true, false) => "ghost",
            (false, true) => "orphan",
            _ => "unknown",
        };

        // Heuristic: if only listeners exist but the same file contains the event literal,
        // synthesize a self-emitter to avoid noisy orphan for runtime-patched/self emissions.
        if status == "orphan" {
            for site in &listeners {
                if let Some(lits) = string_literals_by_path.get(&site.path)
                    && let Some((_, line)) = lits.iter().find(|(lit, _)| lit == norm).cloned()
                {
                    emitters.push(Site {
                        norm: norm.clone(),
                        raw: site.raw.clone(),
                        path: site.path.clone(),
                        line,
                        awaited: false,
                        payload: None,
                    });
                }
            }
            if !emitters.is_empty() {
                status = "ok";
            }
        }

        let mut aliases: Vec<String> = rec.raw_names.iter().cloned().collect();
        aliases.sort();
        if aliases.len() > 1 {
            risks.push(json!({
                "type": "name_mismatch",
                "normalized": norm,
                "aliases": aliases,
            }));
        }

        if status == "ghost" {
            for site in &emitters {
                let mut confidence = "high";
                let mut recommendation = "safe_to_remove";

                let is_literal = site.raw.starts_with('"') || site.raw.starts_with('\'');
                let is_tauri = site.raw.contains("tauri://");
                let is_template = !is_literal && site.raw.contains('`');

                if is_tauri {
                    confidence = "low";
                    recommendation = "check_system_docs";
                } else if is_template || site.raw.contains("${") {
                    confidence = "low";
                    recommendation = "verify_dynamic_value";
                } else if !is_literal {
                    // Identifier or variable that wasn't resolved to a literal
                    confidence = "low";
                    recommendation = "verify_variable_value";
                }

                ghost_emits.push(json!({
                    "name": site.raw,
                    "path": site.path,
                    "line": site.line,
                    "normalized": norm,
                    "payload": site.payload,
                    "confidence": confidence,
                    "recommendation": recommendation,
                }));
            }
        }
        if status == "orphan" {
            for site in &listeners {
                if site.raw.starts_with("tauri://") {
                    continue;
                }
                orphan_listeners.push(json!({
                    "name": site.raw,
                    "path": site.path,
                    "line": site.line,
                    "normalized": norm,
                    "awaited": site.awaited,
                }));
            }
        }

        let canonical = aliases.first().cloned().unwrap_or_else(|| norm.clone());
        event_items.push(json!({
            "name": canonical,
            "normalized": norm,
            "aliases": aliases,
            "status": status,
            "emitCount": emitters.len(),
            "listenCount": listeners.len(),
            "emitters": emitters.iter().map(|s| json!({"path": s.path, "line": s.line, "name": s.raw, "payload": s.payload})).collect::<Vec<_>>(),
            "listeners": listeners.iter().map(|s| json!({"path": s.path, "line": s.line, "name": s.raw, "awaited": s.awaited})).collect::<Vec<_>>(),
        }));
    }

    event_items.sort_by(|a, b| {
        let a_name = a["normalized"].as_str().unwrap_or("");
        let b_name = b["normalized"].as_str().unwrap_or("");
        a_name.cmp(b_name)
    });
    ghost_emits.sort_by(|a, b| {
        let a_name = a["name"].as_str().unwrap_or("");
        let b_name = b["name"].as_str().unwrap_or("");
        a_name
            .cmp(b_name)
            .then(
                a["path"]
                    .as_str()
                    .unwrap_or("")
                    .cmp(b["path"].as_str().unwrap_or("")),
            )
            .then(
                a["line"]
                    .as_u64()
                    .unwrap_or(0)
                    .cmp(&b["line"].as_u64().unwrap_or(0)),
            )
    });
    orphan_listeners.sort_by(|a, b| {
        let a_name = a["name"].as_str().unwrap_or("");
        let b_name = b["name"].as_str().unwrap_or("");
        a_name
            .cmp(b_name)
            .then(
                a["path"]
                    .as_str()
                    .unwrap_or("")
                    .cmp(b["path"].as_str().unwrap_or("")),
            )
            .then(
                a["line"]
                    .as_u64()
                    .unwrap_or(0)
                    .cmp(&b["line"].as_u64().unwrap_or(0)),
            )
    });

    // Heuristic race detection: invoke appears before any listener in same file
    // and listeners that are never awaited.
    for analysis in analyses {
        if !is_in_scope(&analysis.path, focus, exclude) {
            continue;
        }
        if analysis.command_calls.is_empty() || analysis.event_listens.is_empty() {
            continue;
        }
        let first_call = analysis
            .command_calls
            .iter()
            .min_by_key(|c| c.line)
            .cloned();
        let first_listen = analysis
            .event_listens
            .iter()
            .min_by_key(|e| e.line)
            .cloned();
        let first_awaited = analysis
            .event_listens
            .iter()
            .filter(|e| e.awaited)
            .min_by_key(|e| e.line)
            .cloned();

        if let (Some(call), Some(listen)) = (first_call.clone(), first_listen.clone())
            && call.line < listen.line
        {
            risks.push(json!({
                "type": "invoke_before_listen",
                "path": analysis.path,
                "line": call.line,
                "command": call.name,
                "details": "invoke is called before any listener is registered; event may be missed"
            }));
        }

        if let Some(listen) = first_listen {
            if !listen.awaited {
                risks.push(json!({
                    "type": "listen_not_awaited",
                    "path": analysis.path,
                    "line": listen.line,
                    "details": "listener registration is not awaited; first events may race"
                }));
            } else if let Some(call) = first_call
                && let Some(aw) = first_awaited
                && call.line < aw.line
            {
                risks.push(json!({
                    "type": "invoke_before_awaited_listen",
                    "path": analysis.path,
                    "line": call.line,
                    "details": "invoke is issued before awaited listener is registered",
                    "command": call.name,
                }));
            }
        }
    }

    // Command chains: where calls/handlers live and what they emit
    let command_names: HashSet<String> = fe_commands
        .keys()
        .chain(be_commands.keys())
        .cloned()
        .collect();
    let mut chains = Vec::new();
    let total_commands = command_names.len();
    for name in &command_names {
        let calls: Vec<_> = fe_commands
            .get(name)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|(p, _, _)| is_in_scope(p, focus, exclude))
            .collect();
        let handlers: Vec<_> = be_commands
            .get(name)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|(p, _, _)| is_in_scope(p, focus, exclude))
            .collect();

        let mut handler_emits = Vec::new();
        for (path, _line, handler_name) in &handlers {
            if let Some(evts) = path_emit_map.get(path) {
                for evt in evts {
                    handler_emits.push(json!({
                        "name": evt.raw,
                        "normalized": evt.norm,
                        "path": path,
                        "line": evt.line,
                        "handler": handler_name,
                        "payload": evt.payload,
                    }));
                }
            }
        }

        let status = if handlers.is_empty() && !calls.is_empty() {
            "missing_handler"
        } else if calls.is_empty() && !handlers.is_empty() {
            "unused_handler"
        } else {
            "ok"
        };

        chains.push(json!({
            "name": name,
            "status": status,
            "callCount": calls.len(),
            "handlerCount": handlers.len(),
            "calls": calls.iter().map(|(p,l,alias)| {
                let payload = call_payloads
                    .get(name)
                    .and_then(|entries| entries.iter().find(|(pp,ll,_)| pp == p && *ll == *l))
                    .and_then(|(_,_,pl)| pl.clone());
                json!({"path": p, "line": l, "alias": alias, "payload": payload})
            }).collect::<Vec<_>>(),
            "handlers": handlers.iter().map(|(p,l,alias)| {
                let payload = handler_payloads
                    .get(name)
                    .and_then(|entries| entries.iter().find(|(pp,ll,_)| pp == p && *ll == *l))
                    .and_then(|(_,_,pl)| pl.clone());
                json!({"path": p, "line": l, "name": alias, "payload": payload})
            }).collect::<Vec<_>>(),
            "handlerEmits": handler_emits,
        }));
    }
    chains.sort_by(|a, b| {
        let a_name = a["name"].as_str().unwrap_or("");
        let b_name = b["name"].as_str().unwrap_or("");
        a_name.cmp(b_name)
    });

    let stats = json!({
        "emitters": merged_events.values().map(|r| r.emitters.len()).sum::<usize>(),
        "listeners": merged_events.values().map(|r| r.listeners.len()).sum::<usize>(),
        "distinctEmitted": merged_events.values().filter(|r| !r.emitters.is_empty()).count(),
        "distinctListened": merged_events.values().filter(|r| !r.listeners.is_empty()).count(),
        "matched": merged_events.values().filter(|r| !r.emitters.is_empty() && !r.listeners.is_empty()).count(),
        "ghostCount": ghost_emits.len(),
        "orphanCount": orphan_listeners.len(),
    });

    json!({
        "events": {
            "items": event_items,
            "ghostEmits": ghost_emits,
            "orphanListeners": orphan_listeners,
            "stats": stats,
        },
        "commands": {
            "chains": chains,
            "stats": {
                "total": total_commands,
                "withCalls": fe_commands.len(),
                "withHandlers": be_commands.len(),
            }
        },
        "risks": risks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CommandRef, EventRef, FileAnalysis};

    fn mk_event(name: &str, line: usize, kind: &str, awaited: bool) -> EventRef {
        EventRef {
            raw_name: Some(name.to_string()),
            name: name.to_string(),
            line,
            kind: kind.to_string(),
            awaited,
            payload: None,
            is_dynamic: false,
        }
    }

    #[test]
    fn detects_ghost_orphan_and_command_chain_status() {
        let mut fe_commands: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
        fe_commands.insert(
            "unified_ai_chat".into(),
            vec![("src/frontend.ts".into(), 3, "unified_ai_chat".into())],
        );
        fe_commands.insert(
            "missing_cmd".into(),
            vec![("src/frontend.ts".into(), 4, "missing_cmd".into())],
        );

        let mut be_commands: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
        be_commands.insert(
            "unified_ai_chat".into(),
            vec![("src/backend.rs".into(), 15, "unified_ai_chat".into())],
        );
        be_commands.insert(
            "unused_cmd".into(),
            vec![("src/backend.rs".into(), 20, "unused_cmd".into())],
        );

        let fe_payloads: HashMap<String, Vec<(String, usize, Option<String>)>> = HashMap::new();
        let be_payloads: HashMap<String, Vec<(String, usize, Option<String>)>> = HashMap::new();

        // FE file with matching emit/listen
        let mut fe = FileAnalysis::new("src/frontend.ts".into());
        fe.event_emits
            .push(mk_event("vista://ok", 10, "emit_literal", false));
        fe.event_listens
            .push(mk_event("vista://ok", 5, "listen_literal", true));
        fe.command_calls.push(CommandRef {
            name: "unified_ai_chat".into(),
            exposed_name: None,
            line: 3,
            generic_type: None,
            payload: None,
            plugin_name: None,
        });

        // BE file emitting ghost event and handling command
        let mut be = FileAnalysis::new("src/backend.rs".into());
        be.event_emits
            .push(mk_event("vista://ghost", 20, "emit_literal", false));
        be.command_handlers.push(CommandRef {
            name: "unified_ai_chat".into(),
            exposed_name: None,
            line: 15,
            generic_type: None,
            payload: None,
            plugin_name: None,
        });

        // Racy file: invoke before listener and not awaited
        let mut racy = FileAnalysis::new("src/racy.ts".into());
        racy.command_calls.push(CommandRef {
            name: "racy_cmd".into(),
            exposed_name: None,
            line: 1,
            generic_type: None,
            payload: None,
            plugin_name: None,
        });
        racy.event_listens
            .push(mk_event("vista://racy", 10, "listen_literal", false));

        let analyses = vec![fe, be, racy];
        let summary = build_pipeline_summary(
            &analyses,
            &None,
            &None,
            &fe_commands,
            &be_commands,
            &fe_payloads,
            &be_payloads,
        );

        let events = summary["events"]
            .as_object()
            .expect("events section present");
        let ghost = events["ghostEmits"]
            .as_array()
            .expect("ghostEmits array present");
        assert!(ghost.iter().any(|g| g["name"] == "vista://ghost"));

        let orphans = events["orphanListeners"]
            .as_array()
            .expect("orphanListeners array present");
        assert!(orphans.iter().any(|o| o["name"] == "vista://racy"));

        let chains = summary["commands"]["chains"]
            .as_array()
            .expect("chains array");
        let status_map: HashMap<_, _> = chains
            .iter()
            .map(|c| {
                (
                    c.get("name").and_then(|n| n.as_str()).unwrap_or_default(),
                    c.get("status").and_then(|s| s.as_str()).unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(status_map.get("unified_ai_chat"), Some(&"ok"));
        assert_eq!(status_map.get("missing_cmd"), Some(&"missing_handler"));
        assert_eq!(status_map.get("unused_cmd"), Some(&"unused_handler"));

        let risks = summary["risks"].as_array().expect("risks array present");
        assert!(risks.iter().any(|r| r["type"] == "invoke_before_listen"));
        assert!(risks.iter().any(|r| r["type"] == "listen_not_awaited"));
    }

    #[test]
    fn detects_name_mismatch_risk() {
        // Test event with multiple aliases should trigger name_mismatch risk
        // Both normalize to "user_created" but have different raw names
        let mut fe = FileAnalysis::new("src/app.ts".into());
        fe.event_emits
            .push(mk_event("user-created", 10, "emit_literal", false));
        fe.event_emits
            .push(mk_event("user_created", 20, "emit_literal", false));
        fe.event_listens
            .push(mk_event("user-created", 5, "listen_literal", false));

        let analyses = vec![fe];
        let fe_commands: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
        let be_commands: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
        let fe_payloads: HashMap<String, Vec<(String, usize, Option<String>)>> = HashMap::new();
        let be_payloads: HashMap<String, Vec<(String, usize, Option<String>)>> = HashMap::new();

        let summary = build_pipeline_summary(
            &analyses,
            &None,
            &None,
            &fe_commands,
            &be_commands,
            &fe_payloads,
            &be_payloads,
        );

        let risks = summary["risks"].as_array().expect("risks array");
        assert!(risks.iter().any(|r| r["type"] == "name_mismatch"));
    }

    #[test]
    fn ghost_emit_tauri_url_low_confidence() {
        // Ghost emit with tauri:// should have low confidence
        let mut be = FileAnalysis::new("src/backend.rs".into());
        be.event_emits
            .push(mk_event("tauri://focus", 10, "emit_literal", false));

        let analyses = vec![be];
        let fe_commands: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
        let be_commands: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
        let fe_payloads: HashMap<String, Vec<(String, usize, Option<String>)>> = HashMap::new();
        let be_payloads: HashMap<String, Vec<(String, usize, Option<String>)>> = HashMap::new();

        let summary = build_pipeline_summary(
            &analyses,
            &None,
            &None,
            &fe_commands,
            &be_commands,
            &fe_payloads,
            &be_payloads,
        );

        let events = summary["events"].as_object().expect("events section");
        let ghost = events["ghostEmits"].as_array().expect("ghostEmits array");
        let tauri_ghost = ghost.iter().find(|g| g["name"] == "tauri://focus");
        assert!(tauri_ghost.is_some());
        assert_eq!(tauri_ghost.unwrap()["confidence"], "low");
        assert_eq!(tauri_ghost.unwrap()["recommendation"], "check_system_docs");
    }

    #[test]
    fn ghost_emit_template_low_confidence() {
        // Ghost emit with template string should have low confidence
        let mut be = FileAnalysis::new("src/backend.rs".into());
        be.event_emits.push(EventRef {
            raw_name: Some("`user:${id}`".to_string()),
            name: "`user:${id}`".to_string(),
            line: 15,
            kind: "emit_template".to_string(),
            awaited: false,
            payload: None,
            is_dynamic: false,
        });

        let analyses = vec![be];
        let fe_commands: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
        let be_commands: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
        let fe_payloads: HashMap<String, Vec<(String, usize, Option<String>)>> = HashMap::new();
        let be_payloads: HashMap<String, Vec<(String, usize, Option<String>)>> = HashMap::new();

        let summary = build_pipeline_summary(
            &analyses,
            &None,
            &None,
            &fe_commands,
            &be_commands,
            &fe_payloads,
            &be_payloads,
        );

        let events = summary["events"].as_object().expect("events section");
        let ghost = events["ghostEmits"].as_array().expect("ghostEmits array");
        assert!(!ghost.is_empty());
        let template_ghost = &ghost[0];
        assert_eq!(template_ghost["confidence"], "low");
        assert_eq!(template_ghost["recommendation"], "verify_dynamic_value");
    }

    #[test]
    fn dynamic_event_pattern_matching() {
        // Test that dynamic patterns match each other
        // Rust: format!("ai-stream-token:{}", req_id)
        // TypeScript: `ai-stream-token:${requestId}`
        let mut be = FileAnalysis::new("src/backend.rs".into());
        be.event_emits.push(EventRef {
            raw_name: Some("format!(\"ai-stream-token:{}\")".to_string()),
            name: "ai-stream-token:*".to_string(), // normalized pattern with *
            line: 10,
            kind: "emit_dynamic".to_string(),
            awaited: false,
            payload: None,
            is_dynamic: true,
        });

        let mut fe = FileAnalysis::new("src/frontend.ts".into());
        fe.event_listens.push(EventRef {
            raw_name: Some("`ai-stream-token:${...}`".to_string()),
            name: "ai-stream-token:*".to_string(), // normalized pattern with *
            line: 20,
            kind: "listen_dynamic".to_string(),
            awaited: false,
            payload: None,
            is_dynamic: true,
        });

        let analyses = vec![be, fe];
        let fe_commands: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
        let be_commands: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
        let fe_payloads: HashMap<String, Vec<(String, usize, Option<String>)>> = HashMap::new();
        let be_payloads: HashMap<String, Vec<(String, usize, Option<String>)>> = HashMap::new();

        let summary = build_pipeline_summary(
            &analyses,
            &None,
            &None,
            &fe_commands,
            &be_commands,
            &fe_payloads,
            &be_payloads,
        );

        let events = summary["events"].as_object().expect("events section");
        let items = events["items"].as_array().expect("items array");

        // Should have exactly one merged event (dynamic patterns matched)
        assert_eq!(
            items.len(),
            1,
            "Dynamic patterns should be merged into one event"
        );

        let event = &items[0];
        assert_eq!(
            event["status"], "ok",
            "Dynamic pattern event should be matched (ok status)"
        );
        assert_eq!(event["emitCount"], 1);
        assert_eq!(event["listenCount"], 1);

        // Should not have orphan listeners or ghost emits
        let orphans = events["orphanListeners"]
            .as_array()
            .expect("orphanListeners");
        let ghosts = events["ghostEmits"].as_array().expect("ghostEmits");
        assert_eq!(
            orphans.len(),
            0,
            "Dynamic patterns should not create orphan listeners"
        );
        assert_eq!(
            ghosts.len(),
            0,
            "Dynamic patterns should not create ghost emits"
        );
    }

    #[test]
    fn ghost_emit_identifier_low_confidence() {
        // Ghost emit with identifier (not literal) should have low confidence
        let mut be = FileAnalysis::new("src/backend.rs".into());
        be.event_emits.push(EventRef {
            raw_name: Some("EVENT_NAME".to_string()),
            name: "EVENT_NAME".to_string(),
            line: 20,
            kind: "emit_ident".to_string(),
            awaited: false,
            payload: None,
            is_dynamic: false,
        });

        let analyses = vec![be];
        let fe_commands: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
        let be_commands: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
        let fe_payloads: HashMap<String, Vec<(String, usize, Option<String>)>> = HashMap::new();
        let be_payloads: HashMap<String, Vec<(String, usize, Option<String>)>> = HashMap::new();

        let summary = build_pipeline_summary(
            &analyses,
            &None,
            &None,
            &fe_commands,
            &be_commands,
            &fe_payloads,
            &be_payloads,
        );

        let events = summary["events"].as_object().expect("events section");
        let ghost = events["ghostEmits"].as_array().expect("ghostEmits array");
        assert!(!ghost.is_empty());
        let ident_ghost = &ghost[0];
        assert_eq!(ident_ghost["confidence"], "low");
        assert_eq!(ident_ghost["recommendation"], "verify_variable_value");
    }

    #[test]
    fn orphan_listener_tauri_url_skipped() {
        // Orphan listeners with tauri:// should be skipped
        let mut fe = FileAnalysis::new("src/app.ts".into());
        fe.event_listens.push(mk_event(
            "tauri://window-created",
            10,
            "listen_literal",
            false,
        ));

        let analyses = vec![fe];
        let fe_commands: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
        let be_commands: HashMap<String, Vec<(String, usize, String)>> = HashMap::new();
        let fe_payloads: HashMap<String, Vec<(String, usize, Option<String>)>> = HashMap::new();
        let be_payloads: HashMap<String, Vec<(String, usize, Option<String>)>> = HashMap::new();

        let summary = build_pipeline_summary(
            &analyses,
            &None,
            &None,
            &fe_commands,
            &be_commands,
            &fe_payloads,
            &be_payloads,
        );

        let events = summary["events"].as_object().expect("events section");
        let orphans = events["orphanListeners"]
            .as_array()
            .expect("orphanListeners array");
        // tauri:// should not be in orphans
        assert!(
            !orphans
                .iter()
                .any(|o| o["name"] == "tauri://window-created")
        );
    }
}
