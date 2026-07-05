//! Python decorator parsing and framework detection.
//!
//! Handles:
//! - Framework decorator detection (pytest, FastAPI, Flask, Django, etc.)
//! - Route decorator parsing for web frameworks
//! - Type extraction from decorator parameters (response_model, Depends, etc.)
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

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

/// Extract first quoted string literal content from text (single or double quotes).
pub(super) fn extract_first_string_literal(text: &str) -> Option<String> {
    let mut in_quote: Option<char> = None;
    let mut buf = String::new();
    for ch in text.chars() {
        if let Some(q) = in_quote {
            if ch == q {
                return Some(buf);
            } else {
                buf.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
        }
    }
    None
}

/// Parse a decorator line into a route if it matches common web frameworks.
pub(super) fn parse_route_decorator(
    line: &str,
    line_num: usize,
) -> Option<crate::types::RouteInfo> {
    let lower = line.to_lowercase();
    let mut framework = None;
    let mut method = None;
    let mut methods_param: Option<String> = None;

    for (pat, m) in [
        ("@app.get", "GET"),
        ("@app.post", "POST"),
        ("@app.put", "PUT"),
        ("@app.delete", "DELETE"),
        ("@app.patch", "PATCH"),
        ("@router.get", "GET"),
        ("@router.post", "POST"),
        ("@router.put", "PUT"),
        ("@router.delete", "DELETE"),
        ("@router.patch", "PATCH"),
        ("@api_router.get", "GET"),
        ("@api_router.post", "POST"),
        ("@api_router.put", "PUT"),
        ("@api_router.delete", "DELETE"),
        ("@api_router.patch", "PATCH"),
    ] {
        if lower.contains(pat) {
            framework = Some("fastapi");
            method = Some(m);
            break;
        }
    }

    if framework.is_none()
        && (lower.contains("@app.route")
            || lower.contains("@blueprint.route")
            || lower.contains(".route("))
    {
        framework = Some("flask");
        // Try to extract explicit methods list - use original line, not lowercased
        if let Some(pos) = line.find("methods")
            && let Some(start) = line[pos..].find('[')
            && let Some(end) = line[pos + start + 1..].find(']')
        {
            let body = &line[pos + start + 1..pos + start + 1 + end];
            let tokens: Vec<String> = body
                .split([',', ' ', '\t'])
                .filter_map(|p| {
                    let trimmed = p.trim().trim_matches(|c| c == '"' || c == '\'');
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_uppercase())
                    }
                })
                .collect();
            if !tokens.is_empty() {
                methods_param = Some(tokens.join(","));
            }
        }
        method = Some(methods_param.as_deref().unwrap_or("route"));
    }

    let framework = framework?;
    let method = method.unwrap_or("route");
    let path = extract_first_string_literal(line);

    Some(crate::types::RouteInfo {
        framework: framework.to_string(),
        method: method.to_string(),
        path,
        name: None,
        line: line_num,
    })
}
