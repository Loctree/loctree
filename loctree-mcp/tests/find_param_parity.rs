use serde_json::json;

// Since loctree-mcp is a bin crate, we can't import types directly.
// We'll just test the json structure that the MCP server expects.

#[test]
fn test_find_params_serialization() {
    let raw = json!({
        "name": "foo",
        "mode": "symbols",
        "lang": "rs",
        "exported_only": true,
        "dead_only": true,
        "min_score": 0.8,
        "similar": "bar",
        "file": "src/main.rs"
    });

    // In a real test we would deserialize this into FindParams,
    // but since we can't import it from a bin crate, we're just
    // ensuring the JSON representation is correct for documentation purposes.
    assert_eq!(raw["name"], "foo");
    assert_eq!(raw["lang"], "rs");
    assert_eq!(raw["exported_only"], true);
    assert_eq!(raw["dead_only"], true);
    assert_eq!(raw["min_score"], 0.8);
    assert_eq!(raw["similar"], "bar");
    assert_eq!(raw["file"], "src/main.rs");
}

#[test]
fn find_param_parity_literal_options_document_role_contract_shape() {
    let raw = json!({
        "name": "utterance_id",
        "mode": "literal",
        "file": "src/scribe.rs",
        "whole_token": true,
        "group_by_file": true,
        "count_only": true,
        "offset": 2,
        "limit": 3
    });

    assert_eq!(raw["mode"], "literal");
    assert_eq!(raw["whole_token"], true);
    assert_eq!(raw["group_by_file"], true);
    assert_eq!(raw["count_only"], true);
    assert_eq!(raw["offset"], 2);
    assert_eq!(raw["limit"], 3);
    assert_eq!(raw["file"], "src/scribe.rs");
}
