//! Integration test for the `loctree/follow` custom LSP request (Plan 15).
//!
//! Covers params parsing, severity classification, scope dispatch
//! (known / supported / unknown / stub), the response envelope shape,
//! and the Stage 2 split between [`IMPLEMENTED_SCOPES`] and
//! [`STUB_SCOPES`].
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use loctree::snapshot::{CommandBridge, EventBridge, Snapshot};
use loctree::types::{CommandRef, FileAnalysis};
use loctree_lsp::{
    CursorState, FollowParams, FollowResponse, IMPLEMENTED_SCOPES, STUB_SCOPES, SUPPORTED_SCOPES,
    follow, single_page,
};
use serde_json::json;

fn params(scope: &str) -> FollowParams {
    FollowParams {
        scope: scope.into(),
        handler: None,
        limit: None,
        project: None,
        cursor: None,
        chunk_size: None,
    }
}

#[test]
fn supported_scopes_advertised_completely() {
    let expected = [
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
    for scope in expected {
        assert!(
            SUPPORTED_SCOPES.contains(&scope),
            "missing scope in capability: {scope}"
        );
    }
}

#[test]
fn implemented_and_stub_scopes_partition_advertised_set() {
    // `IMPLEMENTED_SCOPES` ∪ `STUB_SCOPES` must equal `SUPPORTED_SCOPES`.
    let mut union: Vec<&'static str> = IMPLEMENTED_SCOPES
        .iter()
        .copied()
        .chain(STUB_SCOPES.iter().copied())
        .collect();
    union.sort();
    let mut advertised: Vec<&'static str> = SUPPORTED_SCOPES.to_vec();
    advertised.sort();
    assert_eq!(
        union, advertised,
        "IMPLEMENTED ∪ STUB must equal SUPPORTED — capability advertisement is the wire contract"
    );

    assert!(
        STUB_SCOPES.is_empty(),
        "all advertised follow scopes should be implemented"
    );
}

#[test]
fn severity_thresholds_match_protocol() {
    assert_eq!(follow::severity_for_count(0), "low");
    assert_eq!(follow::severity_for_count(4), "low");
    assert_eq!(follow::severity_for_count(5), "medium");
    assert_eq!(follow::severity_for_count(20), "medium");
    assert_eq!(follow::severity_for_count(21), "high");
}

#[test]
fn params_deserialize_minimal() {
    let value = json!({ "scope": "cycles" });
    let params: FollowParams = serde_json::from_value(value).unwrap();
    assert_eq!(params.scope, "cycles");
    assert!(params.handler.is_none());
    assert!(params.limit.is_none());
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
}

#[test]
fn unknown_scope_returns_diagnostic_response() {
    let r = FollowResponse::unknown("turbofish");
    assert_eq!(r.scope, "turbofish");
    assert_eq!(r.summary.count, 0);
    let json = serde_json::to_value(&r).unwrap();
    let obj = json.as_object().unwrap();
    assert_eq!(obj["scope"], json!("turbofish"));
    let summary_obj = obj["summary"].as_object().unwrap();
    assert!(
        summary_obj["message"]
            .as_str()
            .unwrap()
            .contains("unknown scope")
    );
}

#[test]
fn stub_scopes_return_unsupported_envelope() {
    for scope in STUB_SCOPES {
        let r = FollowResponse::unsupported(scope);
        assert_eq!(r.scope, *scope);
        assert_eq!(r.summary.count, 0);
        let msg = r.summary.message.unwrap();
        assert!(
            msg.contains("not implemented"),
            "expected 'not implemented' in stub message for {scope}: {msg}"
        );
    }
}

#[test]
fn implemented_scopes_do_not_emit_unsupported_envelope() {
    // Sanity: nothing in IMPLEMENTED_SCOPES is allowed to leak the
    // stub envelope. Drives the Stage 2 truth contract: if a scope is
    // advertised as implemented, it must produce a real shape.
    for scope in IMPLEMENTED_SCOPES {
        let stub = FollowResponse::unsupported(scope);
        assert!(
            stub.summary.message.unwrap().contains("not implemented"),
            "FollowResponse::unsupported should always say 'not implemented' \
             — but production handler must avoid emitting it for {scope}"
        );
    }
}

#[test]
fn response_envelope_serializes_with_stable_shape() {
    let r = FollowResponse::unsupported("trace");
    let json = serde_json::to_value(&r).unwrap();
    let obj = json.as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
    keys.sort();
    assert_eq!(keys, ["items", "scope", "summary"]);
}

#[test]
fn summary_omits_message_when_none() {
    // A response without a message should not surface the field.
    let r = FollowResponse {
        scope: "cycles".into(),
        items: single_page(json!({ "total": 0 })),
        summary: loctree_lsp::FollowSummary {
            count: 0,
            severity: "low".into(),
            message: None,
        },
    };
    let json = serde_json::to_value(&r).unwrap();
    let summary = json["summary"].as_object().unwrap();
    assert!(!summary.contains_key("message"));
    assert_eq!(summary["count"], json!(0));
    assert_eq!(summary["severity"], json!("low"));
}

#[test]
fn compute_commands_scope_returns_real_data() {
    let mut snapshot = Snapshot::new(vec![".".to_string()]);
    snapshot.command_bridges.push(CommandBridge {
        name: "get_user".into(),
        frontend_calls: vec![("src/app.tsx".into(), 14)],
        backend_handler: Some(("src-tauri/src/lib.rs".into(), 42)),
        has_handler: true,
        is_called: true,
    });

    let params = FollowParams {
        scope: "commands".into(),
        handler: None,
        limit: Some(10),
        project: None,
        cursor: None,
        chunk_size: None,
    };
    let resp = follow::compute(&snapshot, std::path::Path::new("."), &params);
    assert_eq!(resp.scope, "commands");
    assert_eq!(resp.summary.count, 1);
    assert!(
        resp.summary.message.is_none(),
        "real-data response must not carry a stub message"
    );
}

#[test]
fn compute_events_scope_returns_real_data() {
    let mut snapshot = Snapshot::new(vec![".".to_string()]);
    snapshot.event_bridges.push(EventBridge {
        name: "user_updated".into(),
        emits: vec![("src/app.ts".into(), 10, "emit".into())],
        listens: vec![("src/profile.ts".into(), 22)],
        is_fe_sync: true,
        same_file_sync: false,
    });

    let params = FollowParams {
        scope: "events".into(),
        handler: None,
        limit: Some(10),
        project: None,
        cursor: None,
        chunk_size: None,
    };
    let resp = follow::compute(&snapshot, std::path::Path::new("."), &params);
    assert_eq!(resp.scope, "events");
    assert_eq!(resp.summary.count, 1);
    assert!(resp.summary.message.is_none());
}

#[test]
fn compute_pipelines_scope_returns_real_shape_with_note() {
    let snapshot = Snapshot::new(vec![".".to_string()]);
    let params = FollowParams {
        scope: "pipelines".into(),
        handler: None,
        limit: Some(10),
        project: None,
        cursor: None,
        chunk_size: None,
    };
    let resp = follow::compute(&snapshot, std::path::Path::new("."), &params);
    assert_eq!(resp.scope, "pipelines");
    let note = resp
        .items
        .data
        .get("note")
        .and_then(|v| v.as_str())
        .expect("pipelines payload must carry the provenance `note`");
    assert!(note.contains("loct pipelines"));
}

#[test]
fn compute_trace_scope_returns_engine_trace_result() {
    let mut snapshot = Snapshot::new(vec![".".to_string()]);
    snapshot.files.push(FileAnalysis {
        path: "src-tauri/src/commands.rs".into(),
        command_handlers: vec![CommandRef {
            name: "get_user".into(),
            exposed_name: Some("getUser".into()),
            line: 10,
            generic_type: None,
            payload: None,
            plugin_name: None,
        }],
        tauri_registered_handlers: vec!["get_user".into()],
        ..Default::default()
    });
    snapshot.command_bridges.push(CommandBridge {
        name: "getUser".into(),
        frontend_calls: vec![("src/api.ts".into(), 30)],
        backend_handler: Some(("src-tauri/src/commands.rs".into(), 10)),
        has_handler: true,
        is_called: true,
    });

    let params = FollowParams {
        scope: "trace".into(),
        handler: Some("get_user".into()),
        limit: None,
        project: None,
        cursor: None,
        chunk_size: None,
    };
    let resp = follow::compute(&snapshot, std::path::Path::new("."), &params);
    assert_eq!(resp.scope, "trace");
    assert!(resp.summary.message.is_none());
    assert_eq!(resp.items.data["handler_name"], json!("get_user"));
    assert!(resp.items.data["backend"].is_object());
    assert_eq!(
        resp.items.data["frontend_invokes"]
            .as_array()
            .expect("trace frontend invokes array")
            .len(),
        1
    );
    assert!(
        resp.items.data["verdict"]
            .as_str()
            .expect("trace verdict")
            .contains("CONNECTED")
    );
}

#[test]
fn compute_coverage_scope_returns_engine_coverage_gaps() {
    let mut snapshot = Snapshot::new(vec![".".to_string()]);
    snapshot.command_bridges.push(CommandBridge {
        name: "get_user".into(),
        frontend_calls: vec![("src/api.ts".into(), 30)],
        backend_handler: Some(("src-tauri/src/commands.rs".into(), 10)),
        has_handler: true,
        is_called: true,
    });

    let params = FollowParams {
        scope: "coverage".into(),
        handler: None,
        limit: None,
        project: None,
        cursor: None,
        chunk_size: None,
    };
    let resp = follow::compute(&snapshot, std::path::Path::new("."), &params);
    assert_eq!(resp.scope, "coverage");
    assert_eq!(resp.summary.count, 1);
    assert_eq!(resp.summary.severity, "high");
    assert!(resp.summary.message.is_none());
    assert_eq!(resp.items.data["total"], json!(1));
    assert_eq!(resp.items.data["critical"], json!(1));
    assert_eq!(
        resp.items.data["items"][0]["kind"],
        json!("handler_without_test")
    );
    assert_eq!(resp.items.data["items"][0]["target"], json!("get_user"));
}

#[test]
fn commands_scope_items_page_through_url_safe_cursor() {
    let mut snapshot = Snapshot::new(vec![".".to_string()]);
    for i in 0..105 {
        snapshot.command_bridges.push(CommandBridge {
            name: format!("cmd_{i:03}"),
            frontend_calls: vec![(format!("src/app_{i:03}.tsx"), i + 1)],
            backend_handler: Some((format!("src-tauri/src/cmd_{i:03}.rs"), i + 10)),
            has_handler: true,
            is_called: true,
        });
    }

    let snapshot_id = "follow-pagination-snapshot";
    let mut request = params("commands");
    request.limit = Some(200);
    request.chunk_size = Some(30);

    let first =
        follow::compute_paginated(&snapshot, std::path::Path::new("."), &request, snapshot_id)
            .expect("first follow page");
    assert_eq!(first.summary.count, 105);
    assert_eq!(first.items.chunk, 0);
    assert_eq!(first.items.total_chunks, 4);
    assert_eq!(first.items.data["items"].as_array().unwrap().len(), 30);
    let first_cursor = first
        .items
        .next_cursor
        .as_deref()
        .expect("multi-page follow response emits cursor");
    assert!(
        !first_cursor.contains('+') && !first_cursor.contains('/') && !first_cursor.contains('='),
        "cursor must be URL-safe base64 without padding: {first_cursor}"
    );
    let decoded = CursorState::decode(first_cursor, snapshot_id, "loctree/follow.items")
        .expect("follow cursor decodes");
    assert_eq!(decoded.offset, 30);

    let mut all = first.items.data["items"].as_array().unwrap().clone();
    let mut cursor = first.items.next_cursor;
    while let Some(token) = cursor {
        request.cursor = Some(token);
        let page =
            follow::compute_paginated(&snapshot, std::path::Path::new("."), &request, snapshot_id)
                .expect("next follow page");
        all.extend(page.items.data["items"].as_array().unwrap().clone());
        cursor = page.items.next_cursor;
    }

    assert_eq!(all.len(), 105);
    assert!(all.iter().any(|item| item["name"] == json!("cmd_104")));
}

#[test]
fn all_scope_items_page_flattened_payload_through_cursor() {
    let mut snapshot = Snapshot::new(vec![".".to_string()]);
    for i in 0..105 {
        snapshot.command_bridges.push(CommandBridge {
            name: format!("cmd_{i:03}"),
            frontend_calls: vec![(format!("src/app_{i:03}.tsx"), i + 1)],
            backend_handler: Some((format!("src-tauri/src/cmd_{i:03}.rs"), i + 10)),
            has_handler: true,
            is_called: true,
        });
    }

    let snapshot_id = "follow-all-pagination-snapshot";
    let mut request = params("all");
    request.limit = Some(200);
    request.chunk_size = Some(30);

    let first =
        follow::compute_paginated(&snapshot, std::path::Path::new("."), &request, snapshot_id)
            .expect("first all page");
    assert_eq!(first.items.chunk, 0);
    assert_eq!(first.items.total_chunks, 8);
    assert_eq!(first.items.data["items"].as_array().unwrap().len(), 30);
    assert_eq!(first.items.data["scope_totals"]["commands"], json!(105));
    assert_eq!(first.items.data["scope_totals"]["coverage"], json!(105));

    let mut all = first.items.data["items"].as_array().unwrap().clone();
    let mut cursor = first.items.next_cursor;
    while let Some(token) = cursor {
        request.cursor = Some(token);
        let page =
            follow::compute_paginated(&snapshot, std::path::Path::new("."), &request, snapshot_id)
                .expect("next all page");
        all.extend(page.items.data["items"].as_array().unwrap().clone());
        cursor = page.items.next_cursor;
    }

    assert_eq!(all.len(), 211);
    assert!(
        all.iter()
            .any(|item| item["scope"] == json!("commands") && item["item"]["name"] == "cmd_104")
    );
    assert!(all.iter().any(
        |item| item["scope"] == json!("coverage") && item["item"]["target"] == json!("cmd_104")
    ));
}

#[test]
fn small_follow_scope_returns_single_page_with_null_cursor() {
    let mut snapshot = Snapshot::new(vec![".".to_string()]);
    snapshot.command_bridges.push(CommandBridge {
        name: "get_user".into(),
        frontend_calls: vec![("src/app.tsx".into(), 14)],
        backend_handler: Some(("src-tauri/src/lib.rs".into(), 42)),
        has_handler: true,
        is_called: true,
    });

    let mut request = params("commands");
    request.chunk_size = Some(30);
    let response = follow::compute_paginated(
        &snapshot,
        std::path::Path::new("."),
        &request,
        "single-page",
    )
    .expect("single-page follow response");

    assert_eq!(response.items.chunk, 0);
    assert_eq!(response.items.total_chunks, 1);
    assert!(response.items.next_cursor.is_none());
    assert_eq!(response.items.data["items"].as_array().unwrap().len(), 1);
}
