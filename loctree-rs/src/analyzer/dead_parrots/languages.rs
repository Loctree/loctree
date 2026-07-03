//! Language-specific checks for dead export detection

use std::fs;

use crate::types::FileAnalysis;

pub(super) fn is_svelte_component_api(file_path: &str, export_name: &str) -> bool {
    // Only applies to .svelte and .svelte.{js,ts} files (Svelte modules)
    let is_svelte_file = file_path.ends_with(".svelte")
        || file_path.ends_with(".svelte.js")
        || file_path.ends_with(".svelte.ts");
    if !is_svelte_file {
        return false;
    }

    // Common component API method names used via bind:this
    const COMPONENT_API_METHODS: &[&str] = &[
        // Modal/dialog patterns
        "show",
        "hide",
        "open",
        "close",
        "toggle",
        "dismiss",
        // Form/input patterns
        "focus",
        "blur",
        "select",
        "selectAll",
        "clear",
        "reset",
        "validate",
        "submit",
        // Text/editor patterns
        "getText",
        "setText",
        "getValue",
        "setValue",
        "getContent",
        "setContent",
        "insertText",
        "replaceText",
        // Scroll patterns
        "scrollTo",
        "scrollToTop",
        "scrollToBottom",
        "scrollIntoView",
        // Animation/transition patterns
        "play",
        "pause",
        "stop",
        "restart",
        "animate",
        // State patterns
        "enable",
        "disable",
        "activate",
        "deactivate",
        "expand",
        "collapse",
        // Lifecycle patterns
        "init",
        "destroy",
        "refresh",
        "update",
        "reload",
        // Svelte reactive getter object patterns (exposed via bind:this)
        "imports",
        "exports",
        "getters",
        "state",
        "values",
    ];

    // Check exact match
    if COMPONENT_API_METHODS.contains(&export_name) {
        return true;
    }

    // Check prefix patterns (e.g., scrollToElement, setFoo, getFoo, applyPr, isActive)
    // These are common patterns for component methods called via bind:this
    const API_PREFIXES: &[&str] = &[
        "scroll",
        "get",
        "set",
        "on",
        "handle",
        "apply",
        "is",
        "has",
        "can",
        "should",
        "do",
        "trigger",
        "emit",
        "fire",
        "dispatch",
        "notify",
        "load",
        "fetch",
        "save",
        "delete",
        "add",
        "remove",
        "insert",
        "append",
        "prepend",
        "move",
        "swap",
        "sort",
        "filter",
        "find",
        "search",
        "check",
        "verify",
        "compute",
        "calculate",
        "render",
        "draw",
        // CRUD patterns
        "create",
        "update",
        "edit",
        "reset",
        "clear",
        "refresh",
        "submit",
        // Navigation/UI patterns
        "show",
        "hide",
        "open",
        "close",
        "toggle",
        "select",
        "click",
        "press",
        // Validation patterns
        "validate",
        "sanitize",
        "normalize",
        "format",
        "parse",
        "serialize",
        "deserialize",
    ];
    for prefix in API_PREFIXES {
        if export_name.starts_with(prefix)
            && export_name.len() > prefix.len()
            && export_name
                .chars()
                .nth(prefix.len())
                .is_some_and(|c| c.is_uppercase())
        {
            return true;
        }
    }

    false
}

/// Check if an export is a JSX runtime export consumed by compilers.
/// These exports are used by TypeScript/Babel when compiling JSX, configured via tsconfig.json:
/// { "jsx": "react-jsx", "jsxImportSource": "solid-js" }
pub(super) fn is_rust_const_table(analysis: &FileAnalysis) -> bool {
    if analysis.language != "rs" {
        return false;
    }
    let const_exports: Vec<_> = analysis
        .exports
        .iter()
        .filter(|e| e.kind == "const")
        .collect();
    if const_exports.len() < 8 {
        return false;
    }

    let shouting: usize = const_exports
        .iter()
        .filter(|e| {
            let name = e.name.as_str();
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        })
        .count();

    // Heuristic: mostly uppercase consts, very few non-const exports => treat as data table.
    let non_const_exports = analysis.exports.len().saturating_sub(const_exports.len());
    shouting * 4 >= const_exports.len() * 3 && non_const_exports <= 2
}
pub(super) fn is_python_library(root: &std::path::Path) -> bool {
    root.join("setup.py").exists()
        || root.join("pyproject.toml").exists()
        || root.join("setup.cfg").exists()
        // CPython stdlib pattern: Lib/ directory at root
        || root.join("Lib").is_dir()
}

/// Check if export is in __all__ list (public API in Python libraries)
pub(super) fn is_in_python_all(analysis: &FileAnalysis, export_name: &str) -> bool {
    analysis
        .exports
        .iter()
        .any(|e| e.name == export_name && e.kind == "__all__")
}

/// Check if a Python export is part of the stdlib public API
/// Returns true for exports that are in __all__ lists in CPython's Lib/ directory
pub(super) fn is_python_stdlib_export(analysis: &FileAnalysis, export_name: &str) -> bool {
    // Check if file is in CPython stdlib structure (Lib/ directory)
    if !analysis.path.contains("/Lib/") && !analysis.path.starts_with("Lib/") {
        return false;
    }

    // All exports in __all__ of stdlib modules are public API
    // This includes constants like calendar.APRIL, classes like csv.DictWriter, etc.
    if is_in_python_all(analysis, export_name) {
        return true;
    }

    // Additional stdlib patterns: top-level public symbols (not starting with _)
    // in stdlib modules that don't have explicit __all__
    if !export_name.starts_with('_') {
        // Constants (UPPER_CASE) in stdlib are typically public API
        if export_name
            .chars()
            .all(|c| c.is_uppercase() || c.is_ascii_digit() || c == '_')
        {
            return true;
        }

        // Classes and functions in stdlib without __all__ are typically public
        // Only if the file doesn't have any __all__ (if __all__ exists, it's definitive)
        let has_explicit_all = analysis.exports.iter().any(|e| e.kind == "__all__");
        if !has_explicit_all {
            return true;
        }
    }

    false
}

/// Check if export is a Python dunder method (protocol methods, never dead)
pub(super) fn is_python_dunder_method(export_name: &str) -> bool {
    export_name.starts_with("__") && export_name.ends_with("__")
}

pub(super) fn rust_has_known_derives(path: &str, keywords: &[&str]) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    let lower = content.to_lowercase();
    lower.contains("derive(") && keywords.iter().any(|kw| lower.contains(kw))
}

pub(super) fn crate_import_matches_file(
    import_raw_path: &str,
    export_file_path: &str,
    symbol_name: &str,
) -> bool {
    // Only handle Rust crate-internal imports
    if !import_raw_path.starts_with("crate::")
        && !import_raw_path.starts_with("super::")
        && !import_raw_path.starts_with("self::")
    {
        return false;
    }

    // Normalize the export file path for matching
    let export_normalized = export_file_path.replace('\\', "/");

    // Extract module path segments from import
    // e.g., "crate::ui::constants::MENU_GAP" -> ["ui", "constants"]
    let import_segments: Vec<&str> = import_raw_path
        .split("::")
        .filter(|s| *s != "crate" && *s != "super" && *s != "self" && *s != symbol_name)
        .collect();

    if import_segments.is_empty() {
        return false;
    }

    // Build potential module path patterns
    // For "crate::ui::constants::X", we check if file ends with:
    // - "ui/constants.rs"
    // - "ui/constants/mod.rs"
    // - just "constants.rs" (simple heuristic)

    let module_path = import_segments.join("/");

    // Check various patterns:
    // 1. Full path match: "src/ui/constants.rs"
    if export_normalized.contains(&format!("{}.rs", module_path))
        || export_normalized.contains(&format!("{}/mod.rs", module_path))
        || export_normalized.contains(&format!("{}/lib.rs", module_path))
    {
        return true;
    }

    // 2. Last segment match (simple heuristic): "constants.rs"
    if let Some(last_segment) = import_segments.last() {
        let file_stem = export_normalized
            .rsplit('/')
            .next()
            .unwrap_or("")
            .trim_end_matches(".rs");

        if file_stem == *last_segment {
            return true;
        }
    }

    // 3. super:: relative match - check if export is in parent directory
    if import_raw_path.starts_with("super::") && !import_segments.is_empty() {
        // For super::types::Config, check if file name is "types.rs"
        if let Some(first_segment) = import_segments.first() {
            let file_name = export_normalized.rsplit('/').next().unwrap_or("");
            if file_name == format!("{}.rs", first_segment) {
                return true;
            }
        }
    }

    // 4. Fallback heuristic for complex nested imports like:
    //    crate::{..., code_context_menus::{..., MENU_GAP}, ...}
    // Check if BOTH the symbol name AND the file's module name appear in raw_path
    let file_stem = export_normalized
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim_end_matches(".rs")
        .trim_end_matches("/mod");

    // Symbol must appear as a word boundary (not part of another identifier)
    let symbol_pattern = format!(r"\b{}\b", regex::escape(symbol_name));
    let module_pattern = format!(r"\b{}\b", regex::escape(file_stem));

    if let (Ok(sym_re), Ok(mod_re)) = (
        regex::Regex::new(&symbol_pattern),
        regex::Regex::new(&module_pattern),
    ) && sym_re.is_match(import_raw_path)
        && mod_re.is_match(import_raw_path)
    {
        return true;
    }

    false
}
