//! Axum route detector (finding H3(a) — recipes round, executor E8).
//!
//! This module detects axum's
//! `.route("path", get(handler))` registration pattern (typically chained
//! off `Router::new()`) from Rust syntax nodes, reusing the existing,
//! framework-agnostic [`RouteInfo`] shape.
//!
//! ## Wiring
//!
//! The integrator closed the original wiring gap on the Layer 1 side:
//! `analyzer/rust/mod.rs` calls [`detect_axum_routes`] per file at scan
//! time on original source, so syntax spans retain truthful line numbers.
//! Comments, string examples and `#[cfg(test)]` items do not count. It assigns the result to
//! `FileAnalysis.routes`, mirroring the Python decorator sensor.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use crate::types::RouteInfo;
use tree_sitter::{Node, Parser};

/// Detect the narrow literal-path / method-combinator / named-handler shape.
/// This is declaration evidence, not receiver type inference or mount resolution.
pub(crate) fn detect_axum_routes(content: &str) -> Vec<RouteInfo> {
    if !content.contains("route") {
        return Vec::new();
    }
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .is_err()
    {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };
    let mut routes = Vec::new();
    collect_axum_routes(tree.root_node(), content, &mut routes);
    routes.sort_by_key(|route| route.line);
    routes
}

fn collect_axum_routes(node: Node<'_>, source: &str, routes: &mut Vec<RouteInfo>) {
    // Attribute items are siblings of their target in the Rust grammar.
    let mut test_only = false;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "attribute_item" {
            if let Some(attribute) = child.named_child(0)
                && let Some(name) = attribute.named_child(0)
                && &source[name.byte_range()] == "cfg"
                && let Some(arguments) = attribute.child_by_field_name("arguments")
            {
                test_only |= cfg_predicates(arguments, source).as_slice() == [Some(false)];
            }
            continue;
        }
        if matches!(child.kind(), "line_comment" | "block_comment") {
            continue;
        }
        if !test_only {
            if child.kind() == "call_expression"
                && let Some(route) = axum_route(child, source)
            {
                routes.push(route);
            }
            collect_axum_routes(child, source, routes);
        }
        test_only = false;
    }
}

// Evaluate only what is known with cfg(test) disabled. Feature/target flags are
// unknown, so their branches remain visible rather than guessing a build profile.
fn cfg_predicates(arguments: Node<'_>, source: &str) -> Vec<Option<bool>> {
    let mut cursor = arguments.walk();
    let tokens: Vec<_> = arguments
        .children(&mut cursor)
        .filter(|node| !matches!(node.kind(), "(" | ")" | "line_comment" | "block_comment"))
        .collect();
    tokens
        .split(|node| node.kind() == ",")
        .filter(|tokens| !tokens.is_empty())
        .map(|tokens| cfg_non_test_enabled(tokens, source))
        .collect()
}

fn cfg_non_test_enabled(tokens: &[Node<'_>], source: &str) -> Option<bool> {
    match tokens {
        [name] if &source[name.byte_range()] == "test" => Some(false),
        [name, arguments] if arguments.kind() == "token_tree" => {
            let values = cfg_predicates(*arguments, source);
            match &source[name.byte_range()] {
                "not" if values.len() == 1 => values[0].map(|value| !value),
                "all" if values.contains(&Some(false)) => Some(false),
                "all" if values.iter().all(|value| *value == Some(true)) => Some(true),
                "any" if values.contains(&Some(true)) => Some(true),
                "any" if values.iter().all(|value| *value == Some(false)) => Some(false),
                _ => None,
            }
        }
        _ => None,
    }
}

fn axum_route(call: Node<'_>, source: &str) -> Option<RouteInfo> {
    let function = call.child_by_field_name("function")?;
    if call.has_error() || function.kind() != "field_expression" {
        return None;
    }
    let field = function.child_by_field_name("field")?;
    if &source[field.byte_range()] != "route" {
        return None;
    }
    let args = call.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    let args: Vec<_> = args
        .named_children(&mut cursor)
        .filter(|node| !matches!(node.kind(), "line_comment" | "block_comment"))
        .collect();
    let [path, method] = args.as_slice() else {
        return None;
    };
    let path = match path.kind() {
        "string_literal" => serde_json::from_str::<String>(&source[path.byte_range()]).ok()?,
        "raw_string_literal" => {
            let text = &source[path.byte_range()];
            text[text.find('"')? + 1..text.rfind('"')?].to_string()
        }
        _ => return None,
    };
    // Preserve the documented first-method-only interpretation of get(a).post(b).
    let mut method = *method;
    loop {
        if method.kind() != "call_expression" {
            return None;
        }
        let function = method.child_by_field_name("function")?;
        if function.kind() == "field_expression" {
            method = function.child_by_field_name("value")?;
        } else {
            break;
        }
    }
    let function = method.child_by_field_name("function")?;
    if function.kind() != "identifier" {
        return None;
    }
    let method_name = &source[function.byte_range()];
    if !matches!(
        method_name,
        "get"
            | "post"
            | "put"
            | "delete"
            | "patch"
            | "head"
            | "options"
            | "trace"
            | "any"
            | "get_service"
    ) {
        return None;
    }
    let handlers = method.child_by_field_name("arguments")?;
    let mut cursor = handlers.walk();
    let handlers: Vec<_> = handlers
        .named_children(&mut cursor)
        .filter(|node| !matches!(node.kind(), "line_comment" | "block_comment"))
        .collect();
    let [handler] = handlers.as_slice() else {
        return None;
    };
    if !matches!(handler.kind(), "identifier" | "scoped_identifier") {
        return None;
    }
    Some(RouteInfo {
        framework: "axum".to_string(),
        method: method_name.to_uppercase(),
        path: Some(path),
        name: Some(source[handler.byte_range()].to_string()),
        line: field.start_position().row + 1,
    })
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
