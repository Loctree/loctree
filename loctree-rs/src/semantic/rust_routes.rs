//! Axum route detector (finding H3(a) — recipes round, executor E8).
//!
//! Layer 1 (`analyzer/rust/mod.rs`) has no Rust route sensor today — only
//! Python decorator-based routes (`analyzer/py/decorators.rs`) populate
//! `FileAnalysis.routes`. This module detects axum's
//! `.route("path", get(handler))` registration pattern (typically chained
//! off `Router::new()`) from raw Rust source text, reusing the existing,
//! framework-agnostic [`RouteInfo`] shape.
//!
//! ## Wiring
//!
//! The integrator closed the original wiring gap on the Layer 1 side:
//! `analyzer/rust/mod.rs` calls [`detect_axum_routes`] per file at scan
//! time — on the stripped event buffer, so routes inside doc comments and
//! `#[cfg(test)]` blocks don't count — and assigns the result to
//! `FileAnalysis.routes`, mirroring the Python decorator sensor.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use crate::analyzer::regexes::regex_axum_route;
use crate::types::RouteInfo;

/// Detect axum `.route("path", method(handler))` registrations in raw Rust
/// source text. Deliberately regex-based (mirrors this crate's existing
/// Rust route/command detectors in `analyzer/regexes.rs`), not a full
/// syn/AST parse — sufficient for the common single-method-per-route call
/// shape described in [`regex_axum_route`].
///
/// Called from Layer 1 (`analyzer/rust/mod.rs`) per file at scan time; see
/// the module-level doc comment.
pub(crate) fn detect_axum_routes(content: &str) -> Vec<RouteInfo> {
    let mut routes = Vec::new();
    for caps in regex_axum_route().captures_iter(content) {
        let Some(path) = caps.get(1).map(|m| m.as_str().to_string()) else {
            continue;
        };
        let Some(method) = caps.get(2).map(|m| m.as_str().to_string()) else {
            continue;
        };
        let handler = caps.get(3).map(|m| m.as_str().to_string());
        let line = caps
            .get(0)
            .map(|m| byte_offset_to_line(content, m.start()))
            .unwrap_or(1);

        routes.push(RouteInfo {
            framework: "axum".to_string(),
            method: method.to_uppercase(),
            path: Some(path),
            name: handler,
            line,
        });
    }
    routes
}

fn byte_offset_to_line(content: &str, offset: usize) -> usize {
    let bound = offset.min(content.len());
    1 + content[..bound].bytes().filter(|b| *b == b'\n').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// H3(a) RED: a Rust fixture with `Router::new().route("/api/x",
    /// get(handler))` must produce a non-empty route list.
    #[test]
    fn detects_single_route_with_get_handler() {
        let content = r#"
let app = Router::new()
    .route("/api/x", get(handler));
"#;
        let routes = detect_axum_routes(content);
        assert!(
            !routes.is_empty(),
            "routes must be non-empty for a Router::new().route(...) chain"
        );
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].framework, "axum");
        assert_eq!(routes[0].method, "GET");
        assert_eq!(routes[0].path.as_deref(), Some("/api/x"));
        assert_eq!(routes[0].name.as_deref(), Some("handler"));
    }

    #[test]
    fn detects_multiple_routes_in_a_chain() {
        let content = r#"
Router::new()
    .route("/api/x", get(get_x))
    .route("/api/y", post(create_y))
"#;
        let routes = detect_axum_routes(content);
        assert_eq!(routes.len(), 2);
        assert!(
            routes
                .iter()
                .any(|r| r.path.as_deref() == Some("/api/x") && r.method == "GET")
        );
        assert!(
            routes
                .iter()
                .any(|r| r.path.as_deref() == Some("/api/y") && r.method == "POST")
        );
    }

    #[test]
    fn ignores_non_route_text() {
        let content = "let x = 1;\nfn foo() {}\n";
        assert!(detect_axum_routes(content).is_empty());
    }

    #[test]
    fn line_number_points_at_the_route_call() {
        let content = "let app = Router::new()\n    .route(\"/api/x\", get(handler));\n";
        let routes = detect_axum_routes(content);
        assert_eq!(routes[0].line, 2);
    }
}
