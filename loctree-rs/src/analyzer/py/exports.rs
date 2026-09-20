//! Python __all__ list parsing and re-export handling.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

use std::path::{Path, PathBuf};

use super::super::regexes::regex_py_all;
use super::helpers::is_valid_python_identifier;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LazyReexport {
    pub symbol: String,
    pub module: String,
    pub line: usize,
}

fn strip_line_comment(line: &str) -> String {
    let mut out = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push(c);
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            '\'' if !in_double => {
                in_single = !in_single;
                out.push(c);
            }
            '"' if !in_single => {
                in_double = !in_double;
                out.push(c);
            }
            '#' if !in_single && !in_double => {
                break;
            }
            _ => out.push(c),
        }
    }
    out
}

fn extract_string_literal(s: &str) -> Option<String> {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        Some(s[1..s.len() - 1].to_string())
    } else {
        None
    }
}

fn extract_name_equality(line: &str, param: &str) -> Option<String> {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("case ") {
        let rest = rest.trim().trim_end_matches(':').trim();
        return extract_string_literal(rest);
    }

    if trimmed.starts_with("if ") || trimmed.starts_with("elif ") {
        let cond = trimmed.split(':').next().unwrap_or(trimmed);
        for part in cond.split(" and ").chain(cond.split(" or ")) {
            if let Some((lhs, rhs)) = part.split_once("==") {
                let lhs = lhs
                    .trim()
                    .trim_start_matches("if ")
                    .trim_start_matches("elif ")
                    .trim();
                let rhs = rhs.trim();
                if lhs == param
                    && let Some(s) = extract_string_literal(rhs)
                {
                    return Some(s);
                } else if rhs == param
                    && let Some(s) = extract_string_literal(lhs)
                {
                    return Some(s);
                }
            }
        }
    }
    None
}

fn extract_name_in_set(line: &str, param: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if (trimmed.starts_with("if ") || trimmed.starts_with("elif ")) && trimmed.contains(" in ") {
        let cond = trimmed.split(':').next().unwrap_or(trimmed);
        if let Some((lhs, rhs)) = cond.split_once(" in ") {
            let lhs = lhs
                .trim()
                .trim_start_matches("if ")
                .trim_start_matches("elif ")
                .trim();
            if lhs == param {
                let rhs = rhs.trim().trim_matches(|c| {
                    c == '(' || c == ')' || c == '[' || c == ']' || c == '{' || c == '}'
                });
                let items: Vec<String> = rhs
                    .split(',')
                    .filter_map(|item| extract_string_literal(item.trim()))
                    .filter(|s| is_valid_python_identifier(s))
                    .collect();
                if !items.is_empty() {
                    return Some(items);
                }
            }
        }
    }
    None
}

fn extract_import_module_arg(line: &str) -> Option<String> {
    if let Some(pos) = line.find("import_module(") {
        let after = &line[pos + "import_module(".len()..];
        let arg = after
            .split(',')
            .next()
            .and_then(|a| a.split(')').next())
            .unwrap_or("")
            .trim();
        return extract_string_literal(arg);
    }
    None
}

fn extract_getattr_call(line: &str) -> Option<(String, String)> {
    if let Some(pos) = line.find("getattr(") {
        let after = &line[pos + "getattr(".len()..];
        let mut depth = 0;
        let mut comma_pos = None;
        let mut close_pos = None;
        for (i, c) in after.char_indices() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => {
                    if depth == 0 {
                        close_pos = Some(i);
                        break;
                    }
                    depth -= 1;
                }
                ',' if depth == 0 && comma_pos.is_none() => {
                    comma_pos = Some(i);
                }
                _ => {}
            }
        }
        if let (Some(cp), Some(cl)) = (comma_pos, close_pos) {
            let target = after[..cp].trim().to_string();
            let attr = after[cp + 1..cl].trim().to_string();
            return Some((target, attr));
        }
    }
    None
}

fn parse_lazy_dict(dict_str: &str, line: usize, results: &mut Vec<LazyReexport>) {
    let Some(start) = dict_str.find('{') else {
        return;
    };
    let Some(end) = dict_str.rfind('}') else {
        return;
    };
    if start >= end {
        return;
    }
    let inner = &dict_str[start + 1..end];

    for entry in inner.split(',') {
        let entry = entry.trim();
        if entry.is_empty() || entry.starts_with('#') {
            continue;
        }
        let clean_entry = entry.split('#').next().unwrap_or("").trim();
        if let Some((k, v)) = clean_entry.split_once(':') {
            let key = k.trim().trim_matches(|c| c == '\'' || c == '"').trim();
            let val = v.trim();

            if val.starts_with('[') || val.starts_with('(') {
                // Form: ".mod": ["sym1", "sym2"]
                let mod_name = key;
                let list_inner = val.trim_matches(|c| c == '[' || c == ']' || c == '(' || c == ')');
                for item in list_inner.split(',') {
                    let sym = item.trim().trim_matches(|c| c == '\'' || c == '"').trim();
                    if is_valid_python_identifier(sym) && !mod_name.is_empty() {
                        results.push(LazyReexport {
                            symbol: sym.to_string(),
                            module: mod_name.to_string(),
                            line,
                        });
                    }
                }
            } else {
                // Form: "sym": ".mod"
                let val_str = val.trim_matches(|c| c == '\'' || c == '"').trim();
                if is_valid_python_identifier(key) && !val_str.is_empty() {
                    results.push(LazyReexport {
                        symbol: key.to_string(),
                        module: val_str.to_string(),
                        line,
                    });
                }
            }
        }
    }
}

/// Detect module-level `__getattr__` lazy re-exports in a Python file.
///
/// Recognizes PEP 562 lazy barrels:
/// 1. Mapping dicts (e.g. `_LAZY_IMPORTS = {"foo": ".mod"}`)
/// 2. Inline conditional imports (`if name == "foo": from .mod import foo; return foo`)
/// 3. Attribute forwarding (`return getattr(sub, name)`) where `sub` was imported
/// 4. Direct `importlib.import_module` delegates
pub(super) fn extract_getattr_lazy_reexports(content: &str) -> Vec<LazyReexport> {
    if !content.contains("__getattr__") {
        return Vec::new();
    }

    let mut results: Vec<LazyReexport> = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut getattr_start = None;
    let mut getattr_param = String::new();

    // 1. Locate module-level `def __getattr__(...)`
    for (idx, line) in lines.iter().enumerate() {
        let trimmed_start = line.trim_start();
        let indent = line.len() - trimmed_start.len();
        if indent == 0
            && let Some(rest) = trimmed_start
                .strip_prefix("def __getattr__")
                .or_else(|| trimmed_start.strip_prefix("async def __getattr__"))
        {
            let rest = rest.trim();
            if let Some(inside_parens) = rest.strip_prefix('(').and_then(|r| r.split(')').next()) {
                let first_param = inside_parens.split(',').next().unwrap_or("").trim();
                let param_name = first_param.split(':').next().unwrap_or(first_param).trim();
                if is_valid_python_identifier(param_name) {
                    getattr_start = Some(idx + 1);
                    getattr_param = param_name.to_string();
                    break;
                }
            }
        }
    }

    let Some(getattr_line) = getattr_start else {
        return Vec::new();
    };

    // 2. Collect imports in this file to map module aliases/names
    let mut imported_module_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for line in &lines {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("from ") {
            if let Some((mod_part, names_part)) = rest.split_once(" import ") {
                let mod_part = mod_part.trim();
                let names = names_part
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches('(')
                    .trim_matches(')');
                for name in names.split(',') {
                    let name = name.trim();
                    if name.is_empty() {
                        continue;
                    }
                    let (orig, alias) = if let Some((lhs, rhs)) = name.split_once(" as ") {
                        (lhs.trim(), rhs.trim())
                    } else {
                        (name, name)
                    };
                    if mod_part == "." {
                        imported_module_map.insert(alias.to_string(), format!(".{}", orig));
                    } else {
                        imported_module_map
                            .insert(alias.to_string(), format!("{}.{}", mod_part, orig));
                    }
                }
            }
        } else if let Some(rest) = trimmed.strip_prefix("import ") {
            for part in rest.split(',') {
                let part = part.trim();
                if let Some((mod_name, alias)) = part.split_once(" as ") {
                    imported_module_map
                        .insert(alias.trim().to_string(), mod_name.trim().to_string());
                } else if !part.is_empty() {
                    let last_seg = part.rsplit('.').next().unwrap_or(part);
                    imported_module_map.insert(last_seg.to_string(), part.to_string());
                }
            }
        }
    }

    // 3. Scan dict mappings:
    let mut in_dict = false;
    let mut dict_buffer = String::new();
    for line in &lines {
        let trimmed = line.trim();
        if !in_dict {
            if (trimmed.contains('{') && trimmed.contains('=')) || trimmed.starts_with('{') {
                in_dict = true;
                dict_buffer.clear();
                dict_buffer.push_str(line);
                if trimmed.contains('}') {
                    in_dict = false;
                    parse_lazy_dict(&dict_buffer, getattr_line, &mut results);
                }
            }
        } else {
            dict_buffer.push('\n');
            dict_buffer.push_str(line);
            if trimmed.contains('}') {
                in_dict = false;
                parse_lazy_dict(&dict_buffer, getattr_line, &mut results);
            }
        }
    }

    // 4. Scan inside `def __getattr__` body
    let mut in_body = false;
    let mut body_lines: Vec<(usize, &str)> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let trimmed_start = line.trim_start();
        let indent = line.len() - trimmed_start.len();

        if line_num == getattr_line {
            in_body = true;
            continue;
        }

        if in_body {
            if indent == 0 && !trimmed_start.is_empty() && !trimmed_start.starts_with('#') {
                break;
            }
            body_lines.push((line_num, trimmed_start));
        }
    }

    let all_names = parse_all_list(content);
    let mut current_candidate_symbol: Option<String> = None;
    let mut current_candidate_symbols: Vec<String> = Vec::new();
    let mut current_candidate_module: Option<String> = None;

    for (line_num, line) in body_lines {
        let without_comment = line.split('#').next().unwrap_or("").trim();
        if without_comment.is_empty() {
            continue;
        }

        // Match `if name == "sym":` or `case "sym":`
        if let Some(sym) = extract_name_equality(without_comment, &getattr_param) {
            current_candidate_symbol = Some(sym);
            current_candidate_symbols.clear();
        }

        // Match `if name in ("sym1", "sym2"):`
        if let Some(syms) = extract_name_in_set(without_comment, &getattr_param) {
            current_candidate_symbols = syms;
            current_candidate_symbol = None;
        }

        // Match `from .mod import sym`
        if let Some(rest) = without_comment.strip_prefix("from ")
            && let Some((mod_part, names_part)) = rest.split_once(" import ")
        {
            let mod_str = mod_part.trim();
            let clean_names = names_part.trim().trim_matches('(').trim_matches(')');
            for item in clean_names.split(',') {
                let item = item.trim();
                let sym = item.split(" as ").next().unwrap_or(item).trim();
                if is_valid_python_identifier(sym) {
                    results.push(LazyReexport {
                        symbol: sym.to_string(),
                        module: mod_str.to_string(),
                        line: line_num,
                    });
                }
            }
        }

        // Match `importlib.import_module(".mod", ...)`
        if let Some(mod_str) = extract_import_module_arg(without_comment) {
            current_candidate_module = Some(mod_str);
        }

        // Match `return getattr(target, name)` or `return getattr(target, "sym")`
        if let Some((target_expr, attr_expr)) = extract_getattr_call(without_comment) {
            let target_mod = if let Some(m) = &current_candidate_module {
                Some(m.clone())
            } else if let Some(mod_str) = extract_import_module_arg(&target_expr) {
                Some(mod_str)
            } else if let Some(imported_mod) = imported_module_map.get(&target_expr) {
                Some(imported_mod.clone())
            } else if is_valid_python_identifier(&target_expr) {
                Some(format!(".{}", target_expr))
            } else {
                None
            };

            if let Some(module) = target_mod {
                if let Some(attr_name) = extract_string_literal(&attr_expr) {
                    if is_valid_python_identifier(&attr_name) {
                        results.push(LazyReexport {
                            symbol: attr_name,
                            module: module.clone(),
                            line: line_num,
                        });
                    }
                } else if attr_expr == getattr_param {
                    if let Some(cand_sym) = &current_candidate_symbol {
                        results.push(LazyReexport {
                            symbol: cand_sym.clone(),
                            module: module.clone(),
                            line: line_num,
                        });
                    } else if !current_candidate_symbols.is_empty() {
                        for sym in &current_candidate_symbols {
                            results.push(LazyReexport {
                                symbol: sym.clone(),
                                module: module.clone(),
                                line: line_num,
                            });
                        }
                    } else if !all_names.is_empty() {
                        for sym in &all_names {
                            results.push(LazyReexport {
                                symbol: sym.clone(),
                                module: module.clone(),
                                line: line_num,
                            });
                        }
                    }
                }
            }
        }

        // If we had a candidate symbol and now see `return target`
        if let Some(cand_sym) = &current_candidate_symbol
            && let Some(target) = without_comment.strip_prefix("return ")
        {
            let target = target.trim();
            if let Some(module) = &current_candidate_module {
                results.push(LazyReexport {
                    symbol: cand_sym.clone(),
                    module: module.clone(),
                    line: line_num,
                });
            } else if target.contains('.') {
                let mod_part = target.split('.').next().unwrap_or("").trim();
                if let Some(resolved_mod) = imported_module_map.get(mod_part) {
                    results.push(LazyReexport {
                        symbol: cand_sym.clone(),
                        module: resolved_mod.clone(),
                        line: line_num,
                    });
                }
            }
        }
    }

    results.dedup_by(|a, b| a.symbol == b.symbol && a.module == b.module);
    results
}

/// Parse Python `__all__` list and extract exported names.
/// Handles inline comments, multi-line lists, and both single and double quoted strings.
pub(super) fn parse_all_list(content: &str) -> Vec<String> {
    let mut names = Vec::new();
    for caps in regex_py_all().captures_iter(content) {
        let body = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        for line in body.lines() {
            let cleaned = strip_line_comment(line);
            let cleaned = cleaned.trim();
            if cleaned.is_empty() || cleaned.starts_with('#') {
                continue;
            }
            for item in cleaned.split(',') {
                let trimmed = item.trim();
                let mut name = trimmed
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim_matches(|c| c == '\'' || c == '"')
                    .trim()
                    .replace('\n', "")
                    .to_string();
                if name.starts_with('#') {
                    name.clear();
                }
                if !name.is_empty() {
                    names.push(name);
                }
            }
        }
    }
    names
}

/// Read __all__ list from a resolved module path.
/// Returns None if the file cannot be read or has no __all__ list.
pub(super) fn read_all_from_resolved(
    resolved: &Option<String>,
    root: &Path,
) -> Option<Vec<String>> {
    let path_str = resolved.as_ref()?;
    let candidate = {
        let p = PathBuf::from(path_str);
        if p.is_absolute() { p } else { root.join(p) }
    };
    let content = std::fs::read_to_string(&candidate).ok()?;
    let names = parse_all_list(&content);
    if names.is_empty() { None } else { Some(names) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_all_list() {
        let content = r#"__all__ = ["foo", "bar"]"#;
        let names = parse_all_list(content);
        assert_eq!(names, vec!["foo", "bar"]);
    }

    #[test]
    fn parses_all_list_with_comments() {
        let content = r#"
__all__ = [
    "foo",  # inline comment
    "bar",
    # "baz" is intentionally excluded
]
"#;
        let names = parse_all_list(content);
        assert_eq!(names, vec!["foo", "bar"]);
        assert!(!names.iter().any(|n| n.contains('#')));
        assert!(!names.contains(&"baz".to_string()));
    }

    #[test]
    fn parses_single_quoted_all_list() {
        let content = r#"__all__ = ['alpha', 'beta']"#;
        let names = parse_all_list(content);
        assert_eq!(names, vec!["alpha", "beta"]);
    }
}
