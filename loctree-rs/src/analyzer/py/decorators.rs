//! Python decorator parsing and framework detection.
//!
//! Handles:
//! - Framework decorator detection (pytest, FastAPI, Flask, Django, etc.)
//! - Route decorator parsing for web frameworks
//! - Type extraction from decorator parameters (response_model, Depends, etc.)
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use super::helpers::{bytes_match_keyword, extract_ascii_ident};

/// Extract type names from decorator parameters.
/// Detects patterns like:
/// - response_model=ClassName
/// - response_model=List[ClassName]
/// - Depends(ClassName)
/// - Depends(get_func)
pub(super) fn extract_decorator_type_usages(line: &str, local_uses: &mut Vec<String>) {
    if !line.contains('(') {
        return;
    }
    const SKIP_IDENTS: &[&str] = &[
        "None",
        "True",
        "False",
        "str",
        "int",
        "float",
        "bool",
        "bytes",
        "list",
        "dict",
        "set",
        "tuple",
        "frozenset",
        "type",
        "object",
        "Any",
        "Union",
        "Optional",
        "List",
        "Dict",
        "Set",
        "Tuple",
        "Callable",
        "Sequence",
        "Mapping",
        "Iterable",
        "Iterator",
        "Type",
        "self",
        "cls",
    ];
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        // Check for response_model= or response_class= using byte comparison
        if bytes_match_keyword(bytes, i, b"response_model=")
            || bytes_match_keyword(bytes, i, b"response_class=")
        {
            // Skip to after the '=' (both "response_model=" and "response_class=" are 15 chars)
            i += 15;
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            extract_type_from_decorator(line, &mut i, local_uses, SKIP_IDENTS);
            continue;
        }
        if bytes_match_keyword(bytes, i, b"Depends(") {
            i += 8;
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < len && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
                let start = i;
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let ident = extract_ascii_ident(bytes, start, i);
                if !ident.is_empty()
                    && !SKIP_IDENTS.contains(&ident.as_str())
                    && !local_uses.contains(&ident)
                {
                    local_uses.push(ident);
                }
            }
            continue;
        }
        i += 1;
    }
}

fn extract_type_from_decorator(
    line: &str,
    pos: &mut usize,
    local_uses: &mut Vec<String>,
    skip_idents: &[&str],
) {
    let bytes = line.as_bytes();
    let len = bytes.len();
    let i = *pos;
    if i >= len || !(bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
        return;
    }
    let start = i;
    let mut j = i;
    while j < len && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
        j += 1;
    }
    let ident = extract_ascii_ident(bytes, start, j);
    *pos = j;
    if j < len && bytes[j] == b'[' {
        j += 1;
        let mut bracket_depth = 1;
        while j < len && bracket_depth > 0 {
            match bytes[j] {
                b'[' => bracket_depth += 1,
                b']' => bracket_depth -= 1,
                _ if bytes[j].is_ascii_alphabetic() || bytes[j] == b'_' => {
                    let inner_start = j;
                    while j < len && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                        j += 1;
                    }
                    let inner_ident = extract_ascii_ident(bytes, inner_start, j);
                    if !inner_ident.is_empty()
                        && !skip_idents.contains(&inner_ident.as_str())
                        && !local_uses.contains(&inner_ident)
                    {
                        local_uses.push(inner_ident);
                    }
                    continue;
                }
                _ => {}
            }
            j += 1;
        }
        *pos = j;
    } else if !ident.is_empty()
        && !skip_idents.contains(&ident.as_str())
        && !local_uses.contains(&ident)
    {
        local_uses.push(ident);
    }
}

/// Check if a decorator line indicates a framework that "uses" the decorated function.
/// Returns true for pytest fixtures, CLI decorators, web route handlers, etc.
pub(super) fn is_framework_decorator(line: &str) -> bool {
    let lower = line.to_lowercase();

    // pytest fixtures and parametrize
    if lower.contains("@pytest.fixture")
        || lower.contains("@fixture")
        || lower.contains("@pytest.mark")
        || lower.contains("@pytest.parametrize")
    {
        return true;
    }

    // Click/Typer CLI
    if lower.contains(".command")
        || lower.contains("@click.")
        || lower.contains("@app.command")
        || lower.contains("@typer.")
    {
        return true;
    }

    // FastAPI routes
    if lower.contains("@app.get")
        || lower.contains("@app.post")
        || lower.contains("@app.put")
        || lower.contains("@app.delete")
        || lower.contains("@app.patch")
        || lower.contains("@router.get")
        || lower.contains("@router.post")
        || lower.contains("@router.put")
        || lower.contains("@router.delete")
        || lower.contains("@router.patch")
        || lower.contains("@api_router.")
    {
        return true;
    }

    // Flask routes
    if lower.contains("@app.route")
        || lower.contains("@blueprint.route")
        || lower.contains(".route(")
    {
        return true;
    }

    // Celery tasks
    if lower.contains("@celery.task")
        || lower.contains("@app.task")
        || lower.contains("@shared_task")
    {
        return true;
    }

    // Django
    if lower.contains("@admin.register")
        || lower.contains("@receiver")
        || lower.contains("@login_required")
        || lower.contains("@permission_required")
    {
        return true;
    }

    // arq worker
    if lower.contains("@cron") || lower.contains("@func") {
        return true;
    }

    // rumps (macOS menu bar apps)
    if lower.contains("@rumps.") || lower.contains(".timer(") {
        return true;
    }

    // Generic callback/event patterns
    if lower.contains("@on_event")
        || lower.contains("@event_handler")
        || lower.contains("@callback")
        || lower.contains("@hook")
        || lower.contains("@register")
    {
        return true;
    }

    false
}

/// Inspect decorators attached to functions, never decorator-like string text.
pub(super) fn collect_python_routes(source: &str) -> Vec<crate::types::RouteInfo> {
    if !source.contains('@') {
        return Vec::new();
    }
    let Ok(parsed) =
        loctree_ast::Parsers::new_default().parse_language("python", source.as_bytes())
    else {
        return Vec::new();
    };
    let mut routes = Vec::new();
    let mut nodes = vec![parsed.tree.root_node()];
    while let Some(node) = nodes.pop() {
        if node.kind() == "decorated_definition"
            && let Some(definition) = node.child_by_field_name("definition")
            && definition.kind() == "function_definition"
            && let Some(name) = definition.child_by_field_name("name")
        {
            let mut cursor = node.walk();
            for decorator in node
                .named_children(&mut cursor)
                .filter(|node| node.kind() == "decorator")
            {
                if let Some(mut route) = parse_route_decorator(decorator, source) {
                    route.name = Some(source[name.byte_range()].to_string());
                    routes.push(route);
                }
            }
        }
        let mut cursor = node.walk();
        nodes.extend(node.named_children(&mut cursor));
    }
    routes.sort_by_key(|route| route.line);
    routes
}

fn parse_route_decorator(
    decorator: tree_sitter::Node<'_>,
    source: &str,
) -> Option<crate::types::RouteInfo> {
    let call = decorator.named_child(0)?;
    if call.kind() != "call" || call.has_error() {
        return None;
    }
    let callee = call.child_by_field_name("function")?;
    if callee.kind() != "attribute" {
        return None;
    }
    let object = callee.child_by_field_name("object")?;
    let attribute = callee.child_by_field_name("attribute")?;
    let receiver = &source[object.byte_range()];
    let method = &source[attribute.byte_range()];
    let framework = if method == "route" {
        // Flask's existing receiver-name-independent .route convention.
        "flask"
    } else if matches!(receiver, "app" | "router" | "api_router")
        && matches!(method, "get" | "post" | "put" | "delete" | "patch")
    {
        "fastapi"
    } else {
        return None;
    };
    let arguments = call.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let mut path = None;
    let mut methods = None;
    let mut positional = 0;
    for argument in arguments.named_children(&mut cursor) {
        if argument.kind() == "comment" {
            continue;
        }
        if argument.kind() == "keyword_argument" {
            let name = argument.child_by_field_name("name")?;
            let value = argument.child_by_field_name("value")?;
            match &source[name.byte_range()] {
                "path" if framework == "fastapi" => {
                    path = Some(python_route_literal(value, source)?)
                }
                "rule" if framework == "flask" => path = Some(python_route_literal(value, source)?),
                "methods" if framework == "flask" => {
                    if !matches!(value.kind(), "list" | "tuple") {
                        return None;
                    }
                    let mut cursor = value.walk();
                    let values: Option<Vec<_>> = value
                        .named_children(&mut cursor)
                        .filter(|node| node.kind() != "comment")
                        .map(|node| {
                            python_route_literal(node, source).map(|method| method.to_uppercase())
                        })
                        .collect();
                    let values = values?;
                    if values.is_empty() {
                        return None;
                    }
                    methods = Some(values.join(","));
                }
                _ => {}
            }
        } else {
            if positional == 0 {
                path = Some(python_route_literal(argument, source)?);
            }
            positional += 1;
        }
    }
    Some(crate::types::RouteInfo {
        framework: framework.to_string(),
        method: if framework == "flask" {
            methods.unwrap_or_else(|| "route".to_string())
        } else {
            method.to_uppercase()
        },
        path: Some(path?),
        name: None,
        line: decorator.start_position().row + 1,
    })
}

fn python_route_literal(node: tree_sitter::Node<'_>, source: &str) -> Option<String> {
    if node.kind() != "string" || node.has_error() {
        return None;
    }
    let text = &source[node.byte_range()];
    let quote = text.find(['\'', '"'])?;
    let prefix = text[..quote].to_ascii_lowercase();
    if !matches!(prefix.as_str(), "" | "r" | "u") {
        return None;
    }
    let delimiter = if text[quote..].starts_with("\"\"\"") || text[quote..].starts_with("'''") {
        3
    } else {
        1
    };
    let body = &text[quote + delimiter..text.len() - delimiter];
    if prefix == "r" || !body.contains('\\') {
        return Some(body.to_string());
    }
    // Decode the common literal escapes; dynamic/interpolated strings stay out.
    let mut value = String::new();
    let mut chars = body.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            value.push(ch);
            continue;
        }
        match chars.next()? {
            '\\' => value.push('\\'),
            '\'' => value.push('\''),
            '"' => value.push('"'),
            'n' => value.push('\n'),
            'r' => value.push('\r'),
            't' => value.push('\t'),
            '\n' => {}
            _ => return None,
        }
    }
    Some(value)
}
