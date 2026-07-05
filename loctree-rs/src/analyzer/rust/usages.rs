//! Usage extraction functions for Rust analyzer.
//!
//! This module contains functions for extracting various types of symbol usages
//! from Rust source code, including:
//! - Function calls (bare and path-qualified)
//! - Type usages in signatures, struct fields, and expressions
//! - Constant and static usages in expressions
//! - Type alias qualified paths (e.g., `io::Result`)
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

use crate::types::{FileAnalysis, ImportEntry, SignatureUse, SignatureUseKind};

use super::offset_to_line;
use super::preprocess::strip_comments;

/// Regex to match public function signatures in Rust code.
/// Captures function name, parameters, and return type.
pub(super) fn regex_rust_pub_fn_signature() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?m)^\s*pub\s*(?:\([^)]*\)\s*)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)\s*\((?P<params>[^)]*)\)\s*(?:->\s*(?P<ret>[^{;]+))?"#,
        )
        .expect("valid pub fn regex")
    })
}

/// Extract type tokens from a type annotation segment.
/// Looks for identifiers that start with uppercase or contain `::`.
/// Skips common standard library types like String, Vec, Option, Result.
pub(super) fn extract_rust_type_tokens(segment: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut types = Vec::new();
    for token in segment.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != ':') {
        if token.is_empty() {
            continue;
        }
        let first = token.chars().next().unwrap_or('_');
        let looks_like_type = first.is_ascii_uppercase() || token.contains("::");
        if !looks_like_type {
            continue;
        }
        const SKIP: &[&str] = &["Self", "String", "Vec", "Option", "Result"];
        if SKIP.contains(&token) {
            continue;
        }
        if seen.insert(token.to_string()) {
            types.push(token.to_string());
        }
    }
    types
}

/// Collect type usages from public function signatures.
/// Scans for `pub fn` declarations and extracts types from parameters and return types.
/// Records these as signature uses for dependency tracking.
pub(super) fn collect_rust_signature_uses(content: &str, analysis: &mut FileAnalysis) {
    for caps in regex_rust_pub_fn_signature().captures_iter(content) {
        let fn_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        if fn_name.is_empty() {
            continue;
        }
        let params = caps.name("params").map(|m| m.as_str()).unwrap_or("");
        let ret = caps.name("ret").map(|m| m.as_str()).unwrap_or("");
        let line = offset_to_line(content, caps.get(0).map(|m| m.start()).unwrap_or(0));

        for ty in extract_rust_type_tokens(params) {
            analysis.signature_uses.push(SignatureUse {
                function: fn_name.to_string(),
                usage: SignatureUseKind::Parameter,
                type_name: ty.clone(),
                line: Some(line),
            });
            if !analysis.local_uses.contains(&ty) {
                analysis.local_uses.push(ty);
            }
        }
        for ty in extract_rust_type_tokens(ret) {
            analysis.signature_uses.push(SignatureUse {
                function: fn_name.to_string(),
                usage: SignatureUseKind::Return,
                type_name: ty.clone(),
                line: Some(line),
            });
            if !analysis.local_uses.contains(&ty) {
                analysis.local_uses.push(ty);
            }
        }
    }
}

/// Extract identifiers used in expressions and variable declarations.
/// This catches:
/// - Constants in generic parameters: `foo::<BUFFER_SIZE, _>`
/// - Constants in array sizes: `[0; BUFFER_SIZE]`
/// - Types in let bindings: `let x: Config = ...`
/// - Types in struct literals: `Config { ... }`
pub(super) fn extract_identifier_usages(content: &str, local_uses: &mut Vec<String>) {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    // Skip Rust keywords that look like identifiers
    const KEYWORDS: &[&str] = &[
        "if", "else", "while", "for", "loop", "match", "return", "break", "continue", "fn", "let",
        "const", "static", "pub", "use", "mod", "struct", "enum", "impl", "trait", "type", "where",
        "unsafe", "async", "await", "move", "ref", "mut", "self", "super", "crate", "dyn", "as",
        "in", "true", "false", "Some", "None", "Ok", "Err", "bool", "char", "str", "u8", "u16",
        "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64",
    ];

    fn add_if_uppercase(ident: &str, uses: &mut Vec<String>) {
        if ident.is_empty() {
            return;
        }
        let first_char = ident.chars().next().unwrap_or('_');
        if first_char.is_ascii_uppercase() && !uses.contains(&ident.to_string()) {
            uses.push(ident.to_string());
        }
    }

    while i < len {
        // Look for `<` which could be start of generic parameters
        if bytes[i] == b'<' {
            i += 1;
            // Skip whitespace after `<`
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }

            // Now look for identifier inside the generic
            if i < len && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
                let start = i;
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let ident = &content[start..i];
                if !KEYWORDS.contains(&ident) {
                    add_if_uppercase(ident, local_uses);
                }
            }
            continue;
        }

        // Look for `: Type` patterns (type annotations)
        if bytes[i] == b':' {
            i += 1;
            // Skip whitespace and possible second `:` (for `::`)
            while i < len && (bytes[i].is_ascii_whitespace() || bytes[i] == b':') {
                i += 1;
            }

            // Now look for identifier after `:`
            if i < len && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
                let start = i;
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let ident = &content[start..i];
                if !KEYWORDS.contains(&ident) {
                    add_if_uppercase(ident, local_uses);
                }
            }
            continue;
        }

        // Look for struct literals or other identifiers: `TypeName {` or `CONST.method()`
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &content[start..i];

            // Skip whitespace
            let saved_i = i;
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }

            // Check what follows the identifier:
            // - `{` = struct literal (e.g., `Config { ... }`)
            // - `.` = method call on const/static (e.g., `CONST.as_bytes()`)
            // - `(` = handled by bare_function_calls
            if i < len && !KEYWORDS.contains(&ident) {
                if bytes[i] == b'{' {
                    // Struct literal
                    add_if_uppercase(ident, local_uses);
                    i += 1; // Move past `{`
                } else if bytes[i] == b'.' {
                    // Method call on identifier (likely a const/static)
                    add_if_uppercase(ident, local_uses);
                    i += 1; // Move past `.`
                } else {
                    // Not a special pattern, restore position
                    i = saved_i;
                    i += 1;
                }
            } else {
                // Keyword or end of content
                i = saved_i;
                if i < len {
                    i += 1;
                }
            }
        } else {
            i += 1;
        }
    }
}

/// Extract identifiers that are followed by `(` indicating a function call.
/// This catches bare function calls like `my_func(arg)` within the same file.
pub(super) fn extract_bare_function_calls(content: &str, local_uses: &mut Vec<String>) {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Look for identifier followed by `(`
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &content[start..i];

            // Skip whitespace
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }

            // Check if followed by `(` (function call) or `!` (macro call)
            if i < len && (bytes[i] == b'(' || bytes[i] == b'!') {
                // Skip Rust keywords that aren't function calls
                const KEYWORDS: &[&str] = &[
                    "if", "else", "while", "for", "loop", "match", "return", "break", "continue",
                    "fn", "let", "const", "static", "pub", "use", "mod", "struct", "enum", "impl",
                    "trait", "type", "where", "unsafe", "async", "await", "move", "ref", "mut",
                    "self", "super", "crate", "dyn", "as", "in", "true", "false",
                ];
                if !KEYWORDS.contains(&ident) && !local_uses.contains(&ident.to_string()) {
                    local_uses.push(ident.to_string());
                }
            }
        } else {
            i += 1;
        }
    }
}

/// Extract uppercase identifiers used as function arguments.
/// This catches constants passed to functions like `.timer(COPILOT_DEBOUNCE_TIMEOUT)`
/// or `advance_clock(BUFFER_SIZE)`.
pub(super) fn extract_function_arguments(content: &str, local_uses: &mut Vec<String>) {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    // Rust keywords that look like identifiers but aren't values
    const KEYWORDS: &[&str] = &[
        "if", "else", "while", "for", "loop", "match", "return", "break", "continue", "fn", "let",
        "const", "static", "pub", "use", "mod", "struct", "enum", "impl", "trait", "type", "where",
        "unsafe", "async", "await", "move", "ref", "mut", "self", "super", "crate", "dyn", "as",
        "in", "true", "false", "Some", "None", "Ok", "Err", "bool", "char", "str", "u8", "u16",
        "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64",
    ];

    fn add_if_uppercase(ident: &str, uses: &mut Vec<String>, keywords: &[&str]) {
        if ident.is_empty() || keywords.contains(&ident) {
            return;
        }
        // Only add if ALL characters are uppercase/underscore/digits (like CONST_NAME)
        // This avoids false positives from regular identifiers
        let is_const_style = ident
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit());
        if is_const_style
            && ident.chars().any(|c| c.is_ascii_uppercase())
            && !uses.contains(&ident.to_string())
        {
            uses.push(ident.to_string());
        }
    }

    while i < len {
        // Look for `(` or `,` which could precede a function argument
        if bytes[i] == b'(' || bytes[i] == b',' {
            i += 1;
            // Skip whitespace after `(` or `,`
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }

            // Check if we have an identifier starting with uppercase
            if i < len && bytes[i].is_ascii_uppercase() {
                let start = i;
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let ident = &content[start..i];
                add_if_uppercase(ident, local_uses, KEYWORDS);
            }
        } else {
            i += 1;
        }
    }
}

/// Regex to find all valid Rust identifiers in content.
pub(super) fn identifier_finder() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").expect("valid identifier regex"))
}

/// Collect all identifier mentions from content, excluding common keywords and standard types.
/// This is a catch-all for identifiers not picked up by more specific extraction functions.
/// W2.2 fix: exclude comment tokens (///, //) and definition-site tokens (the `pub fn NAME`
/// or `pub struct NAME` line itself, and its doc comment) from local_uses. Otherwise a Rust
/// `pub fn` is structurally never dead (its own def line + docs always "use" the name).
/// See loctree-feedback.md:2928 (collect_identifier_mentions).
pub(super) fn collect_identifier_mentions(content: &str, local_uses: &mut Vec<String>) {
    const SKIP: &[&str] = &[
        "if", "else", "while", "for", "loop", "match", "return", "break", "continue", "fn", "let",
        "const", "static", "pub", "use", "mod", "struct", "enum", "impl", "trait", "type", "where",
        "unsafe", "async", "await", "move", "ref", "mut", "self", "super", "crate", "dyn", "as",
        "in", "true", "false", "Some", "None", "Ok", "Err", "bool", "char", "str", "u8", "u16",
        "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64",
        "String", "Vec", "Option", "Result", "Self",
    ];

    for line in content.lines() {
        let t = line.trim_start();
        if t.starts_with("//")
            || t.starts_with("/*")
            || t.starts_with("///")
            || t.starts_with("//!")
        {
            continue; // skip comment tokens entirely for local_uses (W2.2)
        }
        // rough: if this line declares a name, don't let that self-name count as a "use"
        // of the export for dead detection.
        let is_def_line = t.starts_with("pub ")
            || t.starts_with("fn ")
            || t.starts_with("struct ")
            || t.starts_with("enum ")
            || t.starts_with("const ")
            || t.starts_with("static ")
            || t.starts_with("type ")
            || t.starts_with("mod ")
            || t.starts_with("trait ");
        for cap in identifier_finder().find_iter(line) {
            let ident = cap.as_str();
            if SKIP.contains(&ident) {
                continue;
            }
            if is_def_line && line.contains(ident) {
                // definition site mention of this ident — do not count as local "use" of the export
                // (its own def line / sig should not keep a pub item "live")
                continue;
            }
            if !local_uses.contains(&ident.to_string()) {
                local_uses.push(ident.to_string());
            }
        }
    }
}

/// Extract identifiers from path-qualified calls like `foo::bar::func()` or `Type::new()`
/// These are usages that don't require a `use` import.
/// For `Foo::bar::baz()`, we record ALL segments: Foo, bar, baz (each might be a pub export)
pub(super) fn extract_path_qualified_calls(content: &str, local_uses: &mut Vec<String>) {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    // Helper to add identifier if not already present
    fn add_ident(ident: &str, uses: &mut Vec<String>) {
        if !ident.is_empty() && !uses.contains(&ident.to_string()) {
            uses.push(ident.to_string());
        }
    }

    while i < len {
        // Look for identifier followed by `::`
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &content[start..i];

            // Skip whitespace
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }

            // Check if followed by `::`
            if i + 1 < len && bytes[i] == b':' && bytes[i + 1] == b':' {
                // This is a path-qualified usage (Type::method or module::func)
                // Record the first identifier (it's a type or module being used)
                add_ident(ident, local_uses);

                // Now scan the rest of the path, recording all segments
                while i + 1 < len && bytes[i] == b':' && bytes[i + 1] == b':' {
                    i += 2;
                    // Skip whitespace
                    while i < len && bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    // Read next identifier
                    let seg_start = i;
                    while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                        i += 1;
                    }
                    if i > seg_start {
                        let seg = &content[seg_start..i];
                        add_ident(seg, local_uses);
                    }
                    // Skip whitespace
                    while i < len && bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                }
            }
        } else {
            i += 1;
        }
    }
}

/// Extract type alias qualified path usage patterns like `io::Result`, `fs::File`, etc.
/// This tracks when type aliases from imported modules are used via qualified paths
/// without explicit `use` statements for the type itself.
///
/// Example:
/// ```rust,no_run
/// use std::io;  // imports the `io` module
/// fn foo() -> io::Result<()> { Ok(()) }  // uses `Result` from `io` via qualified path
/// ```
///
/// This function analyzes imports to find modules, then scans for `module::Type` patterns
/// where Type starts with uppercase (indicating it's a type/trait/const).
pub(super) fn extract_type_alias_qualified_paths(
    content: &str,
    imports: &[ImportEntry],
    local_uses: &mut Vec<String>,
) {
    // Build a set of module names that were imported
    // Track both the full import source and the last segment (for common patterns like std::io -> io)
    let mut imported_modules: HashSet<String> = HashSet::new();

    for imp in imports {
        // For imports like `use std::io`, the source is "std::io"
        // We want to track both "io" (last segment) and "std::io" (full path)
        imported_modules.insert(imp.source.clone());

        // Also track the last segment (module name)
        if let Some(last_segment) = imp.source.rsplit("::").next()
            && !last_segment.is_empty()
            && last_segment != "*"
        {
            imported_modules.insert(last_segment.to_string());
        }

        // For star imports like `use foo::*`, track "foo" as a module
        if imp.symbols.iter().any(|s| s.name == "*")
            && let Some(module) = imp.source.rsplit("::").next()
        {
            imported_modules.insert(module.to_string());
        }
    }

    // Now scan content for patterns like `module::Type` where:
    // - `module` is in imported_modules
    // - `Type` starts with uppercase (type/trait/const naming convention)
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    fn add_if_new(name: &str, uses: &mut Vec<String>) {
        if !name.is_empty() && !uses.contains(&name.to_string()) {
            uses.push(name.to_string());
        }
    }

    while i < len {
        // Look for identifier
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &content[start..i];

            // Skip whitespace
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }

            // Check if followed by `::`
            if i + 1 < len && bytes[i] == b':' && bytes[i + 1] == b':' {
                // Check if this identifier is an imported module
                if imported_modules.contains(ident) {
                    i += 2; // Skip `::`

                    // Skip whitespace
                    while i < len && bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }

                    // Read the next identifier (the type/trait/const name)
                    let type_start = i;
                    while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                        i += 1;
                    }

                    if i > type_start {
                        let type_name = &content[type_start..i];
                        // Only track if it looks like a Type (starts with uppercase)
                        // This catches Result, Error, File, etc. but not lowercase functions
                        if let Some(first_char) = type_name.chars().next()
                            && first_char.is_ascii_uppercase()
                        {
                            add_if_new(type_name, local_uses);
                        }
                    }
                }
            }
        } else {
            i += 1;
        }
    }
}

/// Extract type names used in struct/enum field definitions.
/// This catches types like `Vec<DiffEdge>`, `Option<HubFile>`, `HashMap<K, V>` etc.
/// that are used as field types within the same file.
///
/// Patterns handled:
/// - `pub struct Foo { field: SomeType, ... }`
/// - `pub struct Foo { field: Vec<SomeType>, ... }`
/// - `pub enum Foo { Variant { field: SomeType }, ... }`
/// - Tuple structs: `pub struct Foo(SomeType, AnotherType);`
pub(super) fn extract_struct_field_types(content: &str, local_uses: &mut Vec<String>) {
    // Strip comments first to avoid false positives from type names in comments
    let content_no_comments = strip_comments(content);

    // Helper to add identifier if not already present and looks like a type name
    fn add_type_if_valid(name: &str, uses: &mut Vec<String>) {
        if name.is_empty() {
            return;
        }
        // Type names typically start with uppercase
        let first_char = name.chars().next().unwrap_or('_');
        if !first_char.is_ascii_uppercase() {
            return;
        }
        // Skip common standard library types (they're not local exports)
        const STD_TYPES: &[&str] = &[
            "Vec",
            "Option",
            "Result",
            "String",
            "Box",
            "Rc",
            "Arc",
            "Cell",
            "RefCell",
            "HashMap",
            "HashSet",
            "BTreeMap",
            "BTreeSet",
            "VecDeque",
            "LinkedList",
            "Mutex",
            "RwLock",
            "Cow",
            "PathBuf",
            "OsString",
            "CString",
            "Duration",
            "Instant",
            "SystemTime",
            "NonZeroU8",
            "NonZeroU16",
            "NonZeroU32",
            "NonZeroU64",
            "NonZeroUsize",
            "NonZeroI8",
            "NonZeroI16",
            "NonZeroI32",
            "NonZeroI64",
            "NonZeroIsize",
            "PhantomData",
            "Pin",
            "ManuallyDrop",
            "MaybeUninit",
            "Self",
        ];
        if STD_TYPES.contains(&name) {
            return;
        }
        if !uses.contains(&name.to_string()) {
            uses.push(name.to_string());
        }
    }

    // Extract type tokens from a type annotation string
    fn extract_types_from_annotation(annotation: &str, uses: &mut Vec<String>) {
        let bytes = annotation.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        while i < len {
            if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
                let start = i;
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let ident = &annotation[start..i];
                add_type_if_valid(ident, uses);
            } else {
                i += 1;
            }
        }
    }

    // Inner function to parse struct block content for type annotations
    fn parse_struct_block_for_types(block: &str, uses: &mut Vec<String>) {
        let bytes = block.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        while i < len {
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= len {
                break;
            }
            if bytes[i] == b':' {
                i += 1;
                while i < len && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                let type_start = i;
                let mut depth = 0;
                while i < len {
                    match bytes[i] {
                        b'<' | b'(' | b'[' | b'{' => depth += 1,
                        b'>' | b')' | b']' | b'}' => {
                            if depth > 0 {
                                depth -= 1;
                            } else if bytes[i] == b'}' {
                                break;
                            }
                        }
                        b',' if depth == 0 => break,
                        _ => {}
                    }
                    i += 1;
                }
                if type_start < i {
                    let type_annotation = &block[type_start..i];
                    extract_types_from_annotation(type_annotation, uses);
                }
            } else {
                i += 1;
            }
        }
    }

    // Find struct/enum blocks and extract field types from comment-stripped content
    let bytes = content_no_comments.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Skip multi-byte UTF-8 characters (non-ASCII) since keywords are ASCII
        if bytes[i] >= 0x80 {
            i += 1;
            continue;
        }

        // Check for 'struct' or 'enum' keyword by looking at bytes directly
        let is_struct = i + 7 <= len
            && bytes[i] == b's'
            && bytes[i + 1] == b't'
            && bytes[i + 2] == b'r'
            && bytes[i + 3] == b'u'
            && bytes[i + 4] == b'c'
            && bytes[i + 5] == b't'
            && (bytes[i + 6] == b' ' || bytes[i + 6] == b'\t' || bytes[i + 6] == b'\n');
        let is_enum = !is_struct
            && i + 5 <= len
            && bytes[i] == b'e'
            && bytes[i + 1] == b'n'
            && bytes[i + 2] == b'u'
            && bytes[i + 3] == b'm'
            && (bytes[i + 4] == b' ' || bytes[i + 4] == b'\t' || bytes[i + 4] == b'\n');

        if is_struct || is_enum {
            let keyword_len = if is_struct { 6 } else { 4 };
            i += keyword_len;

            while i < len {
                let ch = bytes[i];
                if ch == b'{' {
                    i += 1;
                    let mut depth = 1;
                    let block_start = i;
                    while i < len && depth > 0 {
                        match bytes[i] {
                            b'{' => depth += 1,
                            b'}' => depth -= 1,
                            _ => {}
                        }
                        if depth > 0 {
                            i += 1;
                        }
                    }
                    // Safe to slice since we're tracking ASCII braces
                    if let Some(block) = content_no_comments.get(block_start..i) {
                        parse_struct_block_for_types(block, local_uses);
                    }
                    break;
                } else if ch == b'(' {
                    i += 1;
                    let paren_start = i;
                    let mut depth = 1;
                    while i < len && depth > 0 {
                        match bytes[i] {
                            b'(' => depth += 1,
                            b')' => depth -= 1,
                            _ => {}
                        }
                        if depth > 0 {
                            i += 1;
                        }
                    }
                    // Safe to slice since we're tracking ASCII parens
                    if let Some(tuple_content) = content_no_comments.get(paren_start..i) {
                        extract_types_from_annotation(tuple_content, local_uses);
                    }
                    break;
                } else if ch == b';' {
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_identifier_usages() {
        let content = r#"
            let x: Config = Config { field: value };
            foo::<CONST_VALUE, _>();
            let arr = [0; ARRAY_SIZE];
            SomeType { inner: data };
            CONST_NAME.as_bytes();
        "#;
        let mut uses = Vec::new();
        extract_identifier_usages(content, &mut uses);
        // Type annotations after `:`
        assert!(uses.contains(&"Config".to_string()));
        // Generic parameters after `<`
        assert!(uses.contains(&"CONST_VALUE".to_string()));
        // Struct literals (identifier before `{`)
        assert!(uses.contains(&"SomeType".to_string()));
        // Method calls on uppercase identifiers
        assert!(uses.contains(&"CONST_NAME".to_string()));
        // Note: ARRAY_SIZE after `;` is not caught by this function
        // (that's handled by collect_identifier_mentions fallback)
    }

    #[test]
    fn test_extract_bare_function_calls() {
        let content = r#"
            my_func(arg);
            another_func();
            println!("test");
        "#;
        let mut uses = Vec::new();
        extract_bare_function_calls(content, &mut uses);
        assert!(uses.contains(&"my_func".to_string()));
        assert!(uses.contains(&"another_func".to_string()));
        assert!(uses.contains(&"println".to_string()));
    }

    #[test]
    fn test_extract_function_arguments() {
        let content = r#"
            timer(COPILOT_DEBOUNCE_TIMEOUT);
            advance_clock(BUFFER_SIZE);
            process(SOME_CONST, another_var);
        "#;
        let mut uses = Vec::new();
        extract_function_arguments(content, &mut uses);
        assert!(uses.contains(&"COPILOT_DEBOUNCE_TIMEOUT".to_string()));
        assert!(uses.contains(&"BUFFER_SIZE".to_string()));
        assert!(uses.contains(&"SOME_CONST".to_string()));
        // should not contain lowercase identifiers
        assert!(!uses.contains(&"another_var".to_string()));
    }

    #[test]
    fn test_extract_path_qualified_calls() {
        let content = r#"
            Foo::bar::baz();
            Type::new();
            module::function();
        "#;
        let mut uses = Vec::new();
        extract_path_qualified_calls(content, &mut uses);
        assert!(uses.contains(&"Foo".to_string()));
        assert!(uses.contains(&"bar".to_string()));
        assert!(uses.contains(&"baz".to_string()));
        assert!(uses.contains(&"Type".to_string()));
        assert!(uses.contains(&"new".to_string()));
        assert!(uses.contains(&"module".to_string()));
        assert!(uses.contains(&"function".to_string()));
    }

    #[test]
    fn test_extract_type_alias_qualified_paths() {
        use crate::types::{ImportEntry, ImportKind, ImportSymbol};

        let content = r#"
            fn foo() -> io::Result<()> {
                let file: fs::File = fs::File::open("test")?;
                Ok(())
            }
        "#;
        let mut imp1 = ImportEntry::new("std::io".to_string(), ImportKind::Static);
        imp1.symbols.push(ImportSymbol {
            name: "io".to_string(),
            alias: None,
            is_default: false,
        });
        let mut imp2 = ImportEntry::new("std::fs".to_string(), ImportKind::Static);
        imp2.symbols.push(ImportSymbol {
            name: "fs".to_string(),
            alias: None,
            is_default: false,
        });
        let imports = vec![imp1, imp2];
        let mut uses = Vec::new();
        extract_type_alias_qualified_paths(content, &imports, &mut uses);
        assert!(uses.contains(&"Result".to_string()));
        assert!(uses.contains(&"File".to_string()));
    }

    #[test]
    fn test_extract_struct_field_types() {
        let content = r#"
            pub struct MyStruct {
                field1: CustomType,
                field2: Vec<AnotherType>,
                field3: Option<ThirdType>,
            }
            pub enum MyEnum {
                Variant { data: DataType },
            }
            pub struct TupleStruct(FirstType, SecondType);
        "#;
        let mut uses = Vec::new();
        extract_struct_field_types(content, &mut uses);
        assert!(uses.contains(&"CustomType".to_string()));
        assert!(uses.contains(&"AnotherType".to_string()));
        assert!(uses.contains(&"ThirdType".to_string()));
        assert!(uses.contains(&"DataType".to_string()));
        assert!(uses.contains(&"FirstType".to_string()));
        assert!(uses.contains(&"SecondType".to_string()));
        // Should not contain standard types
        assert!(!uses.contains(&"Vec".to_string()));
        assert!(!uses.contains(&"Option".to_string()));
    }

    #[test]
    fn test_strip_comments() {
        let content = r#"
            // Line comment with TypeName
            let x = 5; // Another comment
            /* Block comment with AnotherType */
            let y = 10;
        "#;
        let stripped = strip_comments(content);
        assert!(!stripped.contains("TypeName"));
        assert!(!stripped.contains("AnotherType"));
        assert!(stripped.contains("let x = 5"));
        assert!(stripped.contains("let y = 10"));
    }

    #[test]
    fn test_extract_rust_type_tokens() {
        let segment = "Vec<CustomType>, Option<AnotherType>";
        let types = extract_rust_type_tokens(segment);
        assert!(types.contains(&"CustomType".to_string()));
        assert!(types.contains(&"AnotherType".to_string()));
        assert!(!types.contains(&"Vec".to_string()));
        assert!(!types.contains(&"Option".to_string()));
    }
}
