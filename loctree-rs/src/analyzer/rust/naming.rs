//! Naming and casing utilities for Rust command analysis.
//!
//! Handles rename/rename_all attributes in Tauri commands and casing conversions.

/// Split a name into lowercase words, handling snake_case, kebab-case, and camelCase.
pub(super) fn split_words_lower(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;

    for ch in name.chars() {
        if ch == '_' || ch == '-' {
            if !current.is_empty() {
                words.push(current.to_lowercase());
                current.clear();
            }
            prev_lower = false;
            continue;
        }

        if ch.is_ascii_uppercase() && prev_lower && !current.is_empty() {
            words.push(current.to_lowercase());
            current.clear();
        }

        current.push(ch);
        prev_lower = ch.is_ascii_lowercase();
    }

    if !current.is_empty() {
        words.push(current.to_lowercase());
    }

    words.retain(|w| !w.is_empty());
    words
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// Apply rename_all casing style to a function name.
pub(super) fn apply_rename_all(fn_name: &str, style: &str) -> String {
    let words = split_words_lower(fn_name);
    if words.is_empty() {
        return fn_name.to_string();
    }

    match style {
        "snake_case" => words.join("_"),
        "kebab-case" => words.join("-"),
        "camelCase" => {
            let mut out = words[0].clone();
            for w in words.iter().skip(1) {
                out.push_str(&capitalize(w));
            }
            out
        }
        "PascalCase" | "UpperCamelCase" => {
            let mut out = String::new();
            for w in &words {
                out.push_str(&capitalize(w));
            }
            out
        }
        "lowercase" => words.join("").to_lowercase(),
        "UPPERCASE" => words.join("").to_uppercase(),
        "SCREAMING_SNAKE_CASE" => words.join("_").to_uppercase(),
        _ => fn_name.to_string(),
    }
}

/// Determine the exposed command name from attribute and function name.
///
/// Handles `rename = "..."` and `rename_all = "..."` attributes.
pub(super) fn exposed_command_name(attr_raw: &str, fn_name: &str) -> String {
    let inner = attr_raw
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();

    if inner.is_empty() {
        return fn_name.to_string();
    }

    let mut rename: Option<String> = None;
    let mut rename_all: Option<String> = None;

    for part in inner.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some((key, raw_val)) = trimmed.split_once('=') {
            let key = key.trim();
            let val = raw_val.trim().trim_matches(['"', '\'']).to_string();
            if val.is_empty() {
                continue;
            }
            if key == "rename" {
                rename = Some(val);
            } else if key == "rename_all" {
                rename_all = Some(val);
            }
        }
    }

    if let Some(explicit) = rename {
        return explicit;
    }
    if let Some(style) = rename_all {
        return apply_rename_all(fn_name, &style);
    }

    fn_name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_words_lower_snake_case() {
        assert_eq!(
            split_words_lower("get_user_data"),
            vec!["get", "user", "data"]
        );
    }

    #[test]
    fn test_split_words_lower_camel_case() {
        assert_eq!(
            split_words_lower("getUserData"),
            vec!["get", "user", "data"]
        );
    }

    #[test]
    fn test_split_words_lower_pascal_case() {
        assert_eq!(
            split_words_lower("GetUserData"),
            vec!["get", "user", "data"]
        );
    }

    #[test]
    fn test_apply_rename_all_camel() {
        assert_eq!(
            apply_rename_all("get_user_data", "camelCase"),
            "getUserData"
        );
    }

    #[test]
    fn test_apply_rename_all_pascal() {
        assert_eq!(
            apply_rename_all("get_user_data", "PascalCase"),
            "GetUserData"
        );
    }

    #[test]
    fn test_apply_rename_all_screaming() {
        assert_eq!(
            apply_rename_all("get_user_data", "SCREAMING_SNAKE_CASE"),
            "GET_USER_DATA"
        );
    }

    #[test]
    fn test_exposed_command_name_explicit_rename() {
        assert_eq!(
            exposed_command_name(r#"(rename = "customName")"#, "original_fn"),
            "customName"
        );
    }

    #[test]
    fn test_exposed_command_name_rename_all() {
        assert_eq!(
            exposed_command_name(r#"(rename_all = "camelCase")"#, "get_user_data"),
            "getUserData"
        );
    }

    #[test]
    fn test_exposed_command_name_empty() {
        assert_eq!(exposed_command_name("", "my_function"), "my_function");
    }
}
