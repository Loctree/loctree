//! Tauri plugin detection and command parsing.
//!
//! Handles detection of `#[command(root = "crate")]` plugin commands
//! and inference of plugin identifiers from file paths.

use regex::Regex;

/// Extract plugin name from command attribute if it has `root = "crate"`.
///
/// For Tauri plugin commands, the attribute contains `root = "crate"` which indicates
/// this is a plugin command. In this case, we attempt to extract the plugin name from
/// the file path (the crate/module name).
///
/// Returns Some(plugin_name) if `root = "crate"` is present, None otherwise.
pub(super) fn extract_plugin_name(attr_raw: &str) -> Option<String> {
    let inner = attr_raw
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();

    if inner.is_empty() {
        return None;
    }

    // Check if root = "crate" is present
    for part in inner.split(',') {
        let trimmed = part.trim();
        if let Some((key, raw_val)) = trimmed.split_once('=') {
            let key = key.trim();
            let val = raw_val.trim().trim_matches(['"', '\'']).to_string();
            if key == "root" && val == "crate" {
                // This is a plugin command - we'll need to determine plugin name
                // from the file path during post-processing. Return a placeholder.
                // The actual plugin name will be inferred from the crate structure.
                return Some(String::new());
            }
        }
    }

    None
}

/// Extract plugin identifier from file content or path.
///
/// Tries multiple strategies:
/// 1. File-level attribute `#![plugin(identifier = "window")]` (test fixtures)
/// 2. Path-based inference from `tauri-plugin-XXX` crate names
/// 3. Path-based inference from `plugins/XXX/` directory structure
///
/// Returns Some(identifier) if found, None otherwise.
pub(super) fn extract_plugin_identifier(content: &str, relative_path: &str) -> Option<String> {
    // Strategy 1: Match #![plugin(identifier = "...")]
    if let Ok(re) =
        Regex::new(r#"(?m)^#!\s*\[\s*plugin\s*\(\s*identifier\s*=\s*"([^"]+)"\s*\)\s*\]"#)
        && let Some(caps) = re.captures(content)
        && let Some(m) = caps.get(1)
    {
        return Some(m.as_str().to_string());
    }

    // Strategy 2: Infer from path containing "tauri-plugin-XXX"
    // Examples: tauri-plugin-window/src/lib.rs → "window"
    //           crates/tauri-plugin-dialog/src/commands.rs → "dialog"
    if let Ok(re) = Regex::new(r"tauri-plugin-([a-z][a-z0-9_-]*)")
        && let Some(caps) = re.captures(relative_path)
        && let Some(m) = caps.get(1)
    {
        return Some(m.as_str().replace('-', "_"));
    }

    // Strategy 3: Infer from plugins/XXX/ directory structure
    // Examples: plugins/window/src/lib.rs → "window"
    //           src-tauri/plugins/dialog/src/commands.rs → "dialog"
    if let Ok(re) = Regex::new(r"plugins/([a-z][a-z0-9_-]*)/")
        && let Some(caps) = re.captures(relative_path)
        && let Some(m) = caps.get(1)
    {
        return Some(m.as_str().replace('-', "_"));
    }

    // Strategy 4: Infer from */src/XXX/plugin.rs pattern (Tauri core repo structure)
    // Examples: crates/tauri/src/path/plugin.rs → "path"
    //           crates/tauri/src/window/plugin.rs → "window"
    //           crates/tauri/src/menu/plugin.rs → "menu"
    if (relative_path.ends_with("/plugin.rs") || relative_path.ends_with("\\plugin.rs"))
        && let Ok(re) = Regex::new(r"/([a-z][a-z0-9_-]*)/plugin\.rs$")
        && let Some(caps) = re.captures(relative_path)
        && let Some(m) = caps.get(1)
    {
        return Some(m.as_str().replace('-', "_"));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_plugin_name_with_root_crate() {
        assert!(extract_plugin_name(r#"(root = "crate")"#).is_some());
        assert!(extract_plugin_name(r#"(root = "crate", rename = "foo")"#).is_some());
    }

    #[test]
    fn test_extract_plugin_name_without_root() {
        assert!(extract_plugin_name("").is_none());
        assert!(extract_plugin_name(r#"(rename = "foo")"#).is_none());
    }

    #[test]
    fn test_extract_plugin_identifier_from_attribute() {
        let content = r#"#![plugin(identifier = "window")]"#;
        assert_eq!(
            extract_plugin_identifier(content, "some/path.rs"),
            Some("window".to_string())
        );
    }

    #[test]
    fn test_extract_plugin_identifier_from_tauri_plugin_path() {
        assert_eq!(
            extract_plugin_identifier("", "tauri-plugin-window/src/lib.rs"),
            Some("window".to_string())
        );
        assert_eq!(
            extract_plugin_identifier("", "crates/tauri-plugin-dialog/src/commands.rs"),
            Some("dialog".to_string())
        );
    }

    #[test]
    fn test_extract_plugin_identifier_from_plugins_dir() {
        assert_eq!(
            extract_plugin_identifier("", "plugins/window/src/lib.rs"),
            Some("window".to_string())
        );
        assert_eq!(
            extract_plugin_identifier("", "src-tauri/plugins/dialog/src/commands.rs"),
            Some("dialog".to_string())
        );
    }

    #[test]
    fn test_extract_plugin_identifier_hyphen_to_underscore() {
        assert_eq!(
            extract_plugin_identifier("", "tauri-plugin-my-plugin/src/lib.rs"),
            Some("my_plugin".to_string())
        );
    }

    #[test]
    fn test_extract_plugin_identifier_none() {
        assert_eq!(extract_plugin_identifier("", "src/main.rs"), None);
    }

    #[test]
    fn test_extract_plugin_identifier_from_plugin_rs_pattern() {
        // Strategy 4: Tauri core repo pattern */src/XXX/plugin.rs
        assert_eq!(
            extract_plugin_identifier("", "crates/tauri/src/path/plugin.rs"),
            Some("path".to_string())
        );
        assert_eq!(
            extract_plugin_identifier("", "crates/tauri/src/window/plugin.rs"),
            Some("window".to_string())
        );
        assert_eq!(
            extract_plugin_identifier("", "crates/tauri/src/menu/plugin.rs"),
            Some("menu".to_string())
        );
        assert_eq!(
            extract_plugin_identifier("", "some/other/src/my-plugin/plugin.rs"),
            Some("my_plugin".to_string())
        );
    }
}
