//! Custom LSP request: `loctree/body`
//!
//! Bounded symbol source-body retrieval over LSP. Mirrors the `loct body`
//! CLI: once a symbol's defining file + line is known, return the bounded
//! source text of that symbol's body (brace-balanced, line-capped, with
//! explicit truncation metadata) so editor plugins never have to shell out
//! to `grep`/`sed`/`awk`.
//!
//! The response is byte-for-byte the same JSON shape as `loct body --json`
//! (`{ symbol, bodies: [{ symbol, file, start_line, end_line, language,
//! source, truncated, total_lines, line_cap }] }`) so plugins can share a
//! single type across the CLI and LSP surfaces. The engine
//! ([`loctree::body::query_symbol_body`]) already serializes to that shape,
//! so [`BodyResponse`] is a thin re-export of [`loctree::body::BodyResult`]
//! — no field-mapping conversion is required.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use std::path::PathBuf;

use loctree::body::BodyResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters for `loctree/body`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BodyParams {
    /// Symbol name to retrieve the bounded source body for.
    pub symbol: String,
    /// Maximum number of source lines per body. Defaults to the engine's
    /// [`loctree::body::DEFAULT_BODY_LINE_CAP`] when omitted.
    #[serde(default)]
    pub max_lines: Option<usize>,
    /// Optional file filter (repo-relative path) to disambiguate when the
    /// symbol is defined in more than one file. When present, only bodies
    /// whose `file` ends with this path are returned.
    #[serde(default)]
    pub file: Option<String>,
    /// Workspace project root override. Reserved for Plan 13
    /// (multi-workspace context); ignored in single-workspace mode.
    #[serde(default)]
    pub project: Option<PathBuf>,
}

/// `loctree/body` response.
///
/// Serializes to exactly the `loct body --json` shape:
/// `{ "symbol": "...", "bodies": [{ "symbol", "file", "start_line",
/// "end_line", "language", "source", "truncated", "total_lines",
/// "line_cap" }] }`.
#[derive(Debug, Clone, Serialize)]
pub struct BodyResponse {
    /// Symbol name queried.
    pub symbol: String,
    /// Bodies found (one per defining file/line).
    pub bodies: Vec<loctree::body::SymbolBody>,
}

impl BodyResponse {
    /// Build a response from the engine result, applying the optional
    /// `file` disambiguation filter.
    pub fn from_result(result: BodyResult, file_filter: Option<&str>) -> Self {
        let bodies = match file_filter {
            Some(needle) if !needle.is_empty() => result
                .bodies
                .into_iter()
                .filter(|b| b.file == needle || b.file.ends_with(needle))
                .collect(),
            _ => result.bodies,
        };
        BodyResponse {
            symbol: result.symbol,
            bodies,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_deserialize_minimal() {
        let json = serde_json::json!({ "symbol": "resolveServerBinary" });
        let params: BodyParams = serde_json::from_value(json).expect("minimal params parse");
        assert_eq!(params.symbol, "resolveServerBinary");
        assert!(params.max_lines.is_none());
        assert!(params.file.is_none());
        assert!(params.project.is_none());
    }

    #[test]
    fn params_deserialize_full() {
        let json = serde_json::json!({
            "symbol": "resolveServerBinary",
            "max_lines": 40,
            "file": "src/server.rs",
        });
        let params: BodyParams = serde_json::from_value(json).expect("full params parse");
        assert_eq!(params.max_lines, Some(40));
        assert_eq!(params.file.as_deref(), Some("src/server.rs"));
    }

    fn body(file: &str) -> loctree::body::SymbolBody {
        loctree::body::SymbolBody {
            symbol: "f".into(),
            file: file.into(),
            start_line: 1,
            end_line: 3,
            language: "rs".into(),
            source: "fn f() {}".into(),
            truncated: false,
            total_lines: 3,
            line_cap: 200,
        }
    }

    #[test]
    fn from_result_without_filter_keeps_all_bodies() {
        let result = BodyResult {
            symbol: "f".into(),
            bodies: vec![body("src/a.rs"), body("src/b.rs")],
        };
        let response = BodyResponse::from_result(result, None);
        assert_eq!(response.bodies.len(), 2);
    }

    #[test]
    fn from_result_with_file_filter_keeps_matching_suffix() {
        let result = BodyResult {
            symbol: "f".into(),
            bodies: vec![body("crate/src/a.rs"), body("crate/src/b.rs")],
        };
        let response = BodyResponse::from_result(result, Some("src/a.rs"));
        assert_eq!(response.bodies.len(), 1);
        assert_eq!(response.bodies[0].file, "crate/src/a.rs");
    }

    #[test]
    fn response_serializes_to_loct_body_json_shape() {
        let result = BodyResult {
            symbol: "f".into(),
            bodies: vec![body("src/a.rs")],
        };
        let response = BodyResponse::from_result(result, None);
        let json = serde_json::to_value(&response).expect("serialize body response");
        let obj = json.as_object().expect("object");
        assert_eq!(obj["symbol"], "f");
        let entry = json["bodies"][0].as_object().expect("body object");
        for key in [
            "symbol",
            "file",
            "start_line",
            "end_line",
            "language",
            "source",
            "truncated",
            "total_lines",
            "line_cap",
        ] {
            assert!(entry.contains_key(key), "missing key {key} in {json}");
        }
        assert_eq!(entry.len(), 9, "exactly 9 fields per body: {json}");
    }
}
