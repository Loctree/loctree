//! Rust import parsing and module resolution.
//!
//! Handles parsing of `use` statements and mapping module paths to file paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Read a Rust module file after re-asserting that its canonical form is a
/// descendant of `allowed_root` (the crate root).
///
/// SaaS-safety helper for [`CrateModuleMap::scan_module`]: although the
/// recursive scanner only ever builds module paths via `PathBuf::join` on
/// `self.crate_root`, the crate root itself originates from operator input
/// (`--root`, `LOCT_CACHE_DIR`, MCP payload). The
/// [`crate::fs_utils::SanitizedPath`] gate canonicalizes + re-checks
/// containment immediately before `read_to_string`, so the boundary guard
/// sits at the same call site as the I/O sink and is visible to Semgrep's
/// `tainted-path` data-flow analysis. Without this helper the analyzer
/// would have to follow data flow across the `&mut self` recursion, which
/// it cannot do.
///
/// If the path falls outside `allowed_root` the function returns
/// `PermissionDenied` so the scanner fails closed.
fn read_module_file_within_root(allowed_root: &Path, file_path: &Path) -> std::io::Result<String> {
    crate::fs_utils::read_to_string_within(allowed_root, file_path)
}

/// Parse brace names from Rust use statements, returning (original, exported) pairs.
/// For `use foo::{Bar, Baz as Qux}` returns [("Bar", "Bar"), ("Baz", "Qux")]
pub(super) fn parse_rust_brace_names(raw: &str) -> Vec<(String, String)> {
    raw.split(',')
        .filter_map(|item| {
            let trimmed = item.trim();
            if trimmed.is_empty() {
                return None;
            }
            if trimmed == "self" {
                return None;
            }
            if let Some((original, alias)) = trimmed.split_once(" as ") {
                let original_name = original
                    .trim()
                    .rsplit("::")
                    .next()
                    .unwrap_or(original.trim());
                Some((original_name.to_string(), alias.trim().to_string()))
            } else {
                // Extract the last segment for nested paths like `models::Visit`
                let last_segment = trimmed.rsplit("::").next().unwrap_or(trimmed).trim();
                if last_segment.is_empty() {
                    None
                } else {
                    // No alias - original and exported are the same
                    Some((last_segment.to_string(), last_segment.to_string()))
                }
            }
        })
        .collect()
}

/// Maps Rust module paths (like `crate::foo::bar`) to their corresponding file paths.
/// This is needed to resolve crate-internal imports for dead code detection.
#[derive(Debug, Clone)]
pub struct CrateModuleMap {
    /// Map from module path (e.g., "foo::bar") to file path relative to crate root
    modules: HashMap<String, PathBuf>,
    /// Crate root directory
    crate_root: PathBuf,
}

impl CrateModuleMap {
    /// Build a module map by scanning the crate starting from lib.rs or main.rs
    pub fn build(crate_root: &Path) -> std::io::Result<Self> {
        let mut map = CrateModuleMap {
            modules: HashMap::new(),
            crate_root: crate_root.to_path_buf(),
        };

        // Find the crate entry point (lib.rs or main.rs)
        let lib_rs = crate_root.join("src").join("lib.rs");
        let main_rs = crate_root.join("src").join("main.rs");

        let entry_point = if lib_rs.exists() {
            lib_rs
        } else if main_rs.exists() {
            main_rs
        } else {
            return Ok(map); // No entry point found, return empty map
        };

        // Parse the entry point to build the module tree
        map.scan_module(&entry_point, "")?;

        Ok(map)
    }

    /// Recursively scan a module file and register its submodules.
    ///
    /// SaaS-safety: `file_path` arrives either as the operator-supplied crate
    /// entry point (lib.rs / main.rs under `crate_root`) or as a value derived
    /// from `PathBuf::join` over `self.crate_root` further down the recursion.
    /// Both flows ultimately trace back to `crate_root`, which itself arrives
    /// from `--root` / `LOCT_CACHE_DIR` / the MCP payload. We re-assert
    /// containment at the I/O sink so Semgrep's local `tainted-path`
    /// data-flow analysis can see the boundary guard at the same call site
    /// as the `read_to_string` sink.
    fn scan_module(&mut self, file_path: &Path, module_prefix: &str) -> std::io::Result<()> {
        let content = read_module_file_within_root(&self.crate_root, file_path)?;

        // Find all `mod foo;` declarations
        // Regex pattern: `pub mod name;` or `mod name;`
        let mod_regex = regex::Regex::new(r"(?m)^\s*(?:pub\s+)?mod\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*;")
            .expect("valid mod regex");

        for caps in mod_regex.captures_iter(&content) {
            if let Some(mod_name) = caps.get(1) {
                let mod_name = mod_name.as_str();
                let module_path = if module_prefix.is_empty() {
                    mod_name.to_string()
                } else {
                    format!("{}::{}", module_prefix, mod_name)
                };

                // Determine where to look for the module file based on the current file's structure:
                // 1. If current file is foo.rs -> look in foo/ directory
                // 2. If current file is foo/mod.rs -> look in foo/ directory
                // 3. Otherwise (lib.rs, main.rs) -> look in same directory

                let parent = file_path.parent().unwrap_or(file_path);
                let file_name = file_path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                let search_dirs: Vec<PathBuf> = if file_name.ends_with(".rs")
                    && file_name != "mod.rs"
                    && file_name != "lib.rs"
                    && file_name != "main.rs"
                {
                    // For foo.rs, submodules can be in foo/ directory
                    let module_dir = parent.join(file_name.strip_suffix(".rs").unwrap());
                    vec![module_dir, parent.to_path_buf()]
                } else {
                    // For mod.rs, lib.rs, main.rs, submodules are in the same directory
                    vec![parent.to_path_buf()]
                };

                let mut found = false;
                for search_dir in search_dirs {
                    // Try to find the module file - Rust supports two conventions:
                    // 1. foo.rs (in search directory)
                    // 2. foo/mod.rs (subdirectory with mod.rs)
                    let mod_file = search_dir.join(format!("{}.rs", mod_name));
                    let mod_dir_file = search_dir.join(mod_name).join("mod.rs");

                    if mod_file.exists() {
                        // Register the module and scan it recursively
                        if let Ok(relative) = mod_file.strip_prefix(&self.crate_root) {
                            self.modules
                                .insert(module_path.clone(), relative.to_path_buf());
                        }
                        // Recursively scan the module file
                        let _ = self.scan_module(&mod_file, &module_path);
                        found = true;
                        break;
                    } else if mod_dir_file.exists() {
                        // Register the module directory and scan it recursively
                        if let Ok(relative) = mod_dir_file.strip_prefix(&self.crate_root) {
                            self.modules
                                .insert(module_path.clone(), relative.to_path_buf());
                        }
                        // Recursively scan the module file
                        let _ = self.scan_module(&mod_dir_file, &module_path);
                        found = true;
                        break;
                    }
                }

                if !found {
                    // Module file not found - this is okay, might be in a different workspace or conditional
                    // Just skip it
                }
            }
        }

        Ok(())
    }

    /// Resolve a module path to a file path.
    /// Handles:
    /// - `crate::foo::bar` - absolute from crate root
    /// - `super::bar` - relative to parent module
    /// - `self::bar` - relative to current module
    /// - `foo::bar` (no prefix) - relative to current module
    pub fn resolve_module_path(&self, from_file: &Path, import_path: &str) -> Option<PathBuf> {
        // Handle `crate::` prefix - absolute from crate root
        if let Some(rest) = import_path.strip_prefix("crate::") {
            return self.resolve_absolute(rest);
        }

        // Get the current module path from the file path
        let current_module = self.file_to_module_path(from_file)?;

        // Handle `super::` prefix - go up one level
        if let Some(rest) = import_path.strip_prefix("super::") {
            let parent_module = self.parent_module(&current_module)?;
            let target_path = if rest.is_empty() {
                parent_module
            } else {
                format!("{}::{}", parent_module, rest)
            };
            return self.resolve_absolute(&target_path);
        }

        // Handle `self::` prefix - same module
        if let Some(rest) = import_path.strip_prefix("self::") {
            let target_path = format!("{}::{}", current_module, rest);
            return self.resolve_absolute(&target_path);
        }

        // No prefix - try current module first, then parent modules (Rust 2015 style)
        // In Rust 2018+, bare imports must use crate:: prefix, but we're lenient for analysis

        // Build list of paths to try, from most specific to least specific
        let mut paths_to_try = Vec::new();

        if !current_module.is_empty() {
            paths_to_try.push(format!("{}::{}", current_module, import_path));

            // Walk up parent modules
            let mut current = current_module.to_string();
            while !current.is_empty() {
                if let Some(parent) = self.parent_module(&current) {
                    if parent.is_empty() {
                        paths_to_try.push(import_path.to_string());
                    } else {
                        paths_to_try.push(format!("{}::{}", parent, import_path));
                    }
                    current = parent;
                } else {
                    paths_to_try.push(import_path.to_string());
                    break;
                }
            }
        } else {
            // Already at root
            paths_to_try.push(import_path.to_string());
        }

        // Try each path in order
        for path in paths_to_try {
            if let Some(resolved) = self.resolve_absolute_exact(&path) {
                return Some(resolved);
            }
        }

        // If still not found, try with segment stripping (for type/function resolution)
        self.resolve_absolute(import_path)
    }

    /// Resolve an absolute module path with exact match only (no segment stripping)
    fn resolve_absolute_exact(&self, module_path: &str) -> Option<PathBuf> {
        self.modules.get(module_path).cloned()
    }

    /// Resolve an absolute module path (without crate:: prefix)
    /// This version strips segments to find containing modules (for type/function resolution)
    fn resolve_absolute(&self, module_path: &str) -> Option<PathBuf> {
        self.modules.get(module_path).cloned().or_else(|| {
            // If exact match not found, try to find by stripping last segment
            // (e.g., `foo::Bar` -> `foo.rs` where Bar is a type/fn in foo)
            let mut parts: Vec<&str> = module_path.split("::").collect();
            while !parts.is_empty() {
                parts.pop();
                let partial = parts.join("::");
                if let Some(path) = self.modules.get(&partial) {
                    return Some(path.clone());
                }
            }
            None
        })
    }

    /// Convert a file path to its module path
    fn file_to_module_path(&self, file_path: &Path) -> Option<String> {
        let relative = file_path.strip_prefix(&self.crate_root).ok()?;

        // Convert path to module path: src/foo/bar.rs -> foo::bar
        let mut parts = Vec::new();
        let path_components: Vec<_> = relative.components().collect();

        for (i, component) in path_components.iter().enumerate() {
            let component_str = component.as_os_str().to_str()?;
            if component_str == "src" {
                continue;
            }

            // Check if this is the last component (the file itself)
            let is_last = i == path_components.len() - 1;

            if is_last {
                // For lib.rs or main.rs, this is the root module
                if component_str == "lib.rs" || component_str == "main.rs" {
                    break; // Root module
                }
                // For mod.rs, don't add it (parent dir is the module)
                if component_str == "mod.rs" {
                    break;
                }
                // For foo.rs, add "foo"
                if component_str.ends_with(".rs") {
                    let name = component_str.strip_suffix(".rs").unwrap_or(component_str);
                    parts.push(name);
                }
            } else {
                // Directory component - add it
                parts.push(component_str);
            }
        }

        if parts.is_empty() {
            Some(String::new()) // Root module
        } else {
            Some(parts.join("::"))
        }
    }

    /// Get parent module path (e.g., "foo::bar::baz" -> "foo::bar")
    fn parent_module(&self, module_path: &str) -> Option<String> {
        if module_path.is_empty() {
            return None; // Root module has no parent
        }

        let mut parts: Vec<&str> = module_path.split("::").collect();
        if parts.is_empty() {
            return None;
        }

        parts.pop();
        if parts.is_empty() {
            Some(String::new()) // Parent is root
        } else {
            Some(parts.join("::"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_brace_names_simple() {
        let result = parse_rust_brace_names("Foo, Bar, Baz");
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], ("Foo".to_string(), "Foo".to_string()));
        assert_eq!(result[1], ("Bar".to_string(), "Bar".to_string()));
        assert_eq!(result[2], ("Baz".to_string(), "Baz".to_string()));
    }

    #[test]
    fn test_parse_brace_names_with_alias() {
        let result = parse_rust_brace_names("Foo as F, Bar");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], ("Foo".to_string(), "F".to_string()));
        assert_eq!(result[1], ("Bar".to_string(), "Bar".to_string()));
    }

    #[test]
    fn test_parse_brace_names_nested_path() {
        let result = parse_rust_brace_names("models::Visit, types::Config");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], ("Visit".to_string(), "Visit".to_string()));
        assert_eq!(result[1], ("Config".to_string(), "Config".to_string()));
    }

    #[test]
    fn test_parse_brace_names_self_excluded() {
        let result = parse_rust_brace_names("self, Foo, self::Bar");
        assert_eq!(result.len(), 2); // self is excluded, self::Bar keeps Bar
    }

    #[test]
    fn test_parse_brace_names_empty() {
        let result = parse_rust_brace_names("  ,  ,  ");
        assert!(result.is_empty());
    }
}
