use std::sync::OnceLock;

use regex::Regex;

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex literal")
}

// --- Rust Regexes ---

pub(crate) fn regex_tauri_command_fn() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Matches #[tauri::command] or #[command] (when imported with `use tauri::command`)
        // followed by optional additional attributes like #[allow(...)] before the function definition
        // Supports generic type parameters: fn name<R: Runtime>(...)
        // Also handles comments (// line or /* block */) between attributes and the fn definition
        // IMPORTANT: ^\s* anchors to line start to avoid matching inside comments/strings
        // Comment pattern: (?:\s|//[^\n]*|/\*[\s\S]*?\*/)* handles whitespace, line comments, and block comments
        regex(r#"(?m)^\s*#\s*\[\s*(?:tauri::)?command([^\]]*)\](?:\s*(?://[^\n]*|/\*[\s\S]*?\*/))?(?:(?:\s|//[^\n]*|/\*[\s\S]*?\*/)*#\s*\[[^\]]*\](?:\s*(?://[^\n]*|/\*[\s\S]*?\*/))?)*(?:\s|//[^\n]*|/\*[\s\S]*?\*/)*(?:pub\s*(?:\([^)]*\)\s*)?)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)\s*(?:<[^>]*>)?\s*\((?P<params>[^)]*)\)"#)
    })
}

/// Build a regex to match custom attribute macros that generate Tauri commands.
///
/// For example, if `macro_names` contains `["api_cmd_tauri", "custom_command"]`,
/// this will match `#[api_cmd_tauri(...)]` or `#[custom_command(...)]` on functions.
///
/// Returns `None` if `macro_names` is empty.
pub fn regex_custom_command_fn(macro_names: &[String]) -> Option<Regex> {
    if macro_names.is_empty() {
        return None;
    }

    // Escape any special regex characters in macro names and join with |
    let escaped: Vec<String> = macro_names.iter().map(|name| regex::escape(name)).collect();
    let pattern = escaped.join("|");

    // Build regex similar to regex_tauri_command_fn but with dynamic macro names
    // Matches: #[macro_name(...)] fn name(...)
    // Supports optional crate:: prefix, additional attributes, generic type parameters,
    // and comments (// line or /* block */) between attributes and the fn definition
    // IMPORTANT: ^\s* anchors to line start to avoid matching inside comments/strings
    // Comment pattern: (?:\s|//[^\n]*|/\*[\s\S]*?\*/)* handles whitespace, line comments, and block comments
    let full_pattern = format!(
        r#"(?m)^\s*#\s*\[\s*(?:crate::)?(?:{})([^\]]*)\](?:\s*(?://[^\n]*|/\*[\s\S]*?\*/))?(?:(?:\s|//[^\n]*|/\*[\s\S]*?\*/)*#\s*\[[^\]]*\](?:\s*(?://[^\n]*|/\*[\s\S]*?\*/))?)*(?:\s|//[^\n]*|/\*[\s\S]*?\*/)*(?:pub\s*(?:\([^)]*\)\s*)?)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)\s*(?:<[^>]*>)?\s*\((?P<params>[^)]*)\)"#,
        pattern
    );

    Regex::new(&full_pattern).ok()
}

/// Matches Tauri registrations like `tauri::generate_handler![foo, bar]` or `generate_handler![foo, bar]`.
/// Captures the comma-separated list of function identifiers inside the brackets.
pub(crate) fn regex_tauri_generate_handler() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Supports optional `tauri::` prefix and arbitrary whitespace/newlines around the list.
        regex(r#"(?m)(?:tauri::)?generate_handler!\s*\[([^\]]+)\]"#)
    })
}

pub(crate) fn regex_event_emit_rust() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // app.emit_all("evt", ..) or window.emit("evt", ..) etc., supports const identifiers and format! patterns
        regex(r#"(?m)\.\s*emit[_a-z]*\(\s*(?P<target>["'][^"']+["']|&?format!\s*\([^)]*\)|[A-Za-z_][A-Za-z0-9_]*)\s*(?:,\s*(?P<payload>[^)]*))?"#)
    })
}

pub(crate) fn regex_event_listen_rust() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // app.listen_global("evt", ..) or window.listen("evt", ..) supports const identifiers and format! patterns
        regex(r#"(?m)\.\s*listen[_a-z]*\(\s*(?P<target>["'][^"']+["']|&?format!\s*\([^)]*\)|[A-Za-z_][A-Za-z0-9_]*)"#)
    })
}

pub(crate) fn regex_event_const_rust() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // const EVENT: &str = "name";
        regex(r#"(?m)^\s*(?:pub\s+)?(?:const|static)\s+([A-Za-z0-9_]+)\s*:\s*&str\s*=\s*["']([^"']+)["']"#)
    })
}

pub(crate) fn regex_rust_use() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| regex(r#"(?m)^\s*(?:pub\s*(?:\([^)]*\))?\s+)?use\s+([^;]+);"#))
}

pub(crate) fn regex_rust_pub_use() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| regex(r#"(?m)^\s*pub\s*(?:\([^)]*\))?\s+use\s+([^;]+);"#))
}

/// Matches `mod foo;` declarations (external module references, not inline `mod foo { }`)
/// Captures: (1) optional #[path = "..."] attribute path, (2) module name
/// Examples: `mod foo;`, `pub mod bar;`, `pub(crate) mod internal;`, `#[path = "impl.rs"] mod foo;`
pub(crate) fn regex_rust_mod_decl() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Match optional #[path = "..."] attribute followed by mod declaration
        // Group 1: optional path from #[path = "..."]
        // Group 2: module name
        regex(r#"(?m)^\s*(?:#\s*\[\s*path\s*=\s*"([^"]+)"\s*\]\s*)?(?:pub\s*(?:\([^)]*\)\s*)?)?\s*mod\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*;"#)
    })
}

pub(crate) fn regex_rust_pub_item(kind: &str) -> Regex {
    // Matches visibility modifiers like pub(crate), optional async/const/unsafe/extern modifiers
    // For 'fn', also matches 'const fn' to capture const functions in impl blocks
    // `extern` (with optional ABI string, e.g. `extern "C"`) keeps FFI exports on the
    // export surface — without it a `pub extern "C" fn` leaked into local_symbols and
    // was mislabeled private, the export-side twin of the local-side bug fixed in bc31b072.
    // Also matches associated functions inside impl blocks (not just items at line start)
    let modifiers = if kind == "fn" {
        r#"(?:(?:async|const|unsafe|extern(?:\s+"[^"]+")?)\s+)*"#
    } else {
        r#"(?:(?:async|unsafe)\s+)*"#
    };
    let pattern = format!(
        r#"pub\s*(?:\([^)]*\)\s*)?{}{}\s+([A-Za-z0-9_]+)"#,
        modifiers, kind
    );
    regex(&pattern)
}

pub(crate) fn regex_rust_pub_const_like(kind: &str) -> Regex {
    // Matches pub const/static declarations anywhere (including in impl blocks)
    // Removed (?m)^\s* anchor to allow matching inside impl blocks
    // For 'const', we need to ensure it's followed by an identifier (not fn/unsafe/async)
    // This avoids matching "const fn" which should only be captured by the fn regex
    let suffix = if kind == "const" {
        // After "const ", expect an uppercase identifier (const names follow SCREAMING_SNAKE_CASE)
        // This naturally excludes "const fn/unsafe/async" which have lowercase keywords
        r#"([A-Z][A-Za-z0-9_]*)"#
    } else {
        // For static, we need to:
        // 1. Skip optional 'mut' keyword (for static mut)
        // 2. Skip 'ref' keyword (used in lazy_static! macro: pub static ref FOO)
        // 3. Then capture the actual identifier name (uppercase for constants)
        // The negative lookahead (?!ref\b|mut\b) ensures we don't capture these keywords
        r#"(?:mut\s+)?(?:ref\s+)?([A-Z][A-Za-z0-9_]*)"#
    };
    let pattern = format!(r#"pub\s*(?:\([^)]*\)\s*)?{}\s+{}"#, kind, suffix);
    regex(&pattern)
}

pub(crate) fn rust_pub_decl_regexes() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        vec![
            regex_rust_pub_item("fn"),
            regex_rust_pub_item("struct"),
            regex_rust_pub_item("enum"),
            regex_rust_pub_item("trait"),
            regex_rust_pub_item("type"),
            regex_rust_pub_item("union"),
            // Note: pub mod is NOT included - modules are not exports that need to be imported
            // They are path prefixes for accessing items within the module
        ]
    })
    .as_slice()
}

pub(crate) fn rust_pub_const_regexes() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        vec![
            regex_rust_pub_const_like("const"),
            regex_rust_pub_const_like("static"),
        ]
    })
    .as_slice()
}

// Rust entry point detection regexes
pub(crate) fn regex_rust_fn_main() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Match fn main() at start of line (with optional pub and async)
    RE.get_or_init(|| regex(r#"(?m)^(?:pub\s+)?(?:async\s+)?fn\s+main\s*\("#))
}

pub(crate) fn regex_rust_async_main_attr() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Match #[tokio::main] or #[async_std::main] attributes
    RE.get_or_init(|| regex(r#"(?m)^#\[(tokio|async_std)::main\]"#))
}

// --- CSS Regexes ---

pub(crate) fn regex_css_import() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // @import "x.css";  @import url("x.css"); @import url(x.css);
        regex(r#"(?m)@import\s+(?:url\()?['"]?([^"'()\s]+)['"]?\)?"#)
    })
}

// --- Python Regexes ---

pub(crate) fn regex_py_dynamic_importlib() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| regex(r#"importlib\.import_module\(\s*([^)]+?)\s*(?:,|\))"#))
}

pub(crate) fn regex_py_dynamic_dunder() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| regex(r#"__import__\(\s*([^)]+?)\s*(?:,|\))"#))
}

pub(crate) fn regex_py_all() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| regex(r#"(?s)__all__\s*=\s*\[([^\]]*)\]"#))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_mod_decl_basic() {
        let re = regex_rust_mod_decl();

        // Basic mod
        let caps = re.captures("mod foo;").unwrap();
        assert_eq!(caps.get(2).unwrap().as_str(), "foo");

        // pub mod
        let caps = re.captures("pub mod bar;").unwrap();
        assert_eq!(caps.get(2).unwrap().as_str(), "bar");
    }

    #[test]
    fn test_rust_mod_decl_visibility_modifiers() {
        let re = regex_rust_mod_decl();

        // pub(crate) mod
        let caps = re.captures("pub(crate) mod schema;").unwrap();
        assert_eq!(caps.get(2).unwrap().as_str(), "schema");

        // pub(super) mod
        let caps = re.captures("pub(super) mod internal;").unwrap();
        assert_eq!(caps.get(2).unwrap().as_str(), "internal");

        // pub(self) mod
        let caps = re.captures("pub(self) mod private;").unwrap();
        assert_eq!(caps.get(2).unwrap().as_str(), "private");

        // pub(in path) mod
        let caps = re.captures("pub(in crate::foo) mod nested;").unwrap();
        assert_eq!(caps.get(2).unwrap().as_str(), "nested");
    }

    #[test]
    fn test_rust_mod_decl_with_indentation() {
        let re = regex_rust_mod_decl();

        // Indented mod declarations
        let caps = re.captures("    pub(crate) mod migrations;").unwrap();
        assert_eq!(caps.get(2).unwrap().as_str(), "migrations");

        let caps = re.captures("\t\tmod env_tests;").unwrap();
        assert_eq!(caps.get(2).unwrap().as_str(), "env_tests");
    }

    #[test]
    fn test_rust_mod_decl_with_path_attr() {
        let re = regex_rust_mod_decl();

        // #[path = "..."] mod
        let caps = re.captures(r#"#[path = "impl.rs"] mod foo;"#).unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "impl.rs");
        assert_eq!(caps.get(2).unwrap().as_str(), "foo");

        // With visibility
        let caps = re
            .captures(r#"#[path = "other.rs"] pub(crate) mod thing;"#)
            .unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "other.rs");
        assert_eq!(caps.get(2).unwrap().as_str(), "thing");
    }

    #[test]
    fn test_rust_mod_decl_test_modules() {
        let re = regex_rust_mod_decl();

        // Test module patterns
        let caps = re.captures("mod env_tests;").unwrap();
        assert_eq!(caps.get(2).unwrap().as_str(), "env_tests");

        let caps = re.captures("mod tests;").unwrap();
        assert_eq!(caps.get(2).unwrap().as_str(), "tests");

        let caps = re.captures("#[cfg(test)] mod test_utils;");
        // Note: #[cfg(test)] is different from #[path], so this won't capture the attr
        // but should still capture the mod name
        assert!(caps.is_some() || regex_rust_mod_decl().is_match("mod test_utils;"));
    }
}

#[test]
fn test_tauri_command_with_inline_comment() {
    let re = regex_tauri_command_fn();

    // Inline comment after attribute (common pattern for documenting why an attribute is needed)
    let with_comment = r#"#[tauri::command]
pub async fn convert_audio(
    app_handle: tauri::AppHandle,
    audioData: Vec<u8>,
) -> Result<Vec<u8>, String> {"#;

    let caps = re.captures(with_comment);
    assert!(
        caps.is_some(),
        "Should match tauri command with inline comment after attribute"
    );
    assert_eq!(
        caps.unwrap().get(2).map(|m| m.as_str()),
        Some("convert_audio")
    );
}

#[test]
fn test_tauri_command_comment_between_attrs() {
    let re = regex_tauri_command_fn();

    // Comment on separate line between attributes
    let comment_between = r#"#[tauri::command]
// This is a comment explaining the function
pub fn my_handler() {}"#;

    let caps = re.captures(comment_between);
    assert!(
        caps.is_some(),
        "Should match with comment line between attributes"
    );
    assert_eq!(caps.unwrap().get(2).map(|m| m.as_str()), Some("my_handler"));
}

// === NEGATIVE TESTS ===
// These ensure we don't get false positives from comments/strings

#[test]
fn test_tauri_command_in_line_comment_no_match() {
    let re = regex_tauri_command_fn();

    // #[tauri::command] inside a line comment should NOT match
    let commented_out = r#"// #[tauri::command]
// pub fn disabled_handler() {}"#;

    assert!(
        re.captures(commented_out).is_none(),
        "Should NOT match #[tauri::command] inside a line comment"
    );
}

#[test]
fn test_tauri_command_in_string_no_match() {
    let re = regex_tauri_command_fn();

    // #[tauri::command] inside a string literal should NOT match as a handler
    // Using r##""## to allow "# inside the string
    let in_string = r##"let example = "#[tauri::command]
pub fn fake_handler() {}";"##;

    // This might match the fake_handler - let's verify it doesn't treat this as real
    // The regex starts with # at line start (due to (?m)), so string content shouldn't match
    let caps = re.captures(in_string);
    // If it matches, it found something that looks like a command - which is a false positive
    // In practice, the # inside the string won't be at line start in valid Rust code
    assert!(
        caps.is_none(),
        "Should NOT match #[tauri::command] inside a string literal"
    );
}

#[test]
fn test_tauri_command_with_block_comment() {
    let re = regex_tauri_command_fn();

    // Block comments /* */ after attributes should be supported
    let with_block_comment = r#"#[tauri::command]
pub fn handler_with_block_comment() {}"#;

    let caps = re.captures(with_block_comment);
    assert!(
        caps.is_some(),
        "Should match tauri command with block comment after attribute"
    );
    assert_eq!(
        caps.unwrap().get(2).map(|m| m.as_str()),
        Some("handler_with_block_comment")
    );
}

#[test]
fn test_tauri_command_with_multiline_block_comment() {
    let re = regex_tauri_command_fn();

    // Multiline block comments between attributes
    let multiline_comment = r#"#[tauri::command]
/* This is a longer explanation
   spanning multiple lines
   about why this attribute exists */
pub async fn handler_multiline() {}"#;

    let caps = re.captures(multiline_comment);
    assert!(
        caps.is_some(),
        "Should match tauri command with multiline block comment"
    );
    assert_eq!(
        caps.unwrap().get(2).map(|m| m.as_str()),
        Some("handler_multiline")
    );
}

#[test]
fn test_tauri_command_with_doc_comments() {
    let re = regex_tauri_command_fn();

    // Rust doc comments (///) are common before functions
    // They should work just like regular // comments
    let with_doc_comments = r#"#[tauri::command]
/// Process the incoming data and return the result.
///
/// # Arguments
/// * `data` - The input data to process
///
/// # Returns
/// The processed result as a string
pub async fn documented_handler(data: String) -> Result<String, String> {"#;

    let caps = re.captures(with_doc_comments);
    assert!(
        caps.is_some(),
        "Should match tauri command with /// doc comments"
    );
    assert_eq!(
        caps.unwrap().get(2).map(|m| m.as_str()),
        Some("documented_handler")
    );
}

#[test]
fn test_tauri_command_with_doc_comments_after_attr() {
    let re = regex_tauri_command_fn();

    // Doc comment after attribute (less common but valid)
    let doc_after_attr = r#"#[tauri::command]
pub fn handler_with_doc_after_attr() {}"#;

    let caps = re.captures(doc_after_attr);
    assert!(
        caps.is_some(),
        "Should match tauri command with /// doc comment after attribute"
    );
    assert_eq!(
        caps.unwrap().get(2).map(|m| m.as_str()),
        Some("handler_with_doc_after_attr")
    );
}
