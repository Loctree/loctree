//! CSS Layout Analysis Module
//!
//! Scans CSS/SCSS files for layout-related properties:
//! - z-index values (for layer management)
//! - position: sticky/fixed (for scroll-aware elements)
//! - display: grid/flex (for layout containers)

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;
use walkdir::WalkDir;

use crate::cli::command::LayoutmapOptions;

/// A CSS layout finding
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayoutFinding {
    /// z-index value found
    ZIndex {
        file: String,
        line: usize,
        selector: String,
        z_index: i32,
    },
    /// position: sticky or fixed
    Sticky {
        file: String,
        line: usize,
        selector: String,
        position: String,
    },
    /// display: grid
    Grid {
        file: String,
        line: usize,
        selector: String,
    },
    /// display: flex
    Flex {
        file: String,
        line: usize,
        selector: String,
    },
}

/// Scan CSS files for layout properties
pub fn scan_css_layout(root: &Path, opts: &LayoutmapOptions) -> io::Result<Vec<LayoutFinding>> {
    let mut findings = Vec::new();

    // CSS file extensions to scan
    let css_extensions = ["css", "scss", "sass", "less"];

    // Also scan JS/TS files for CSS-in-JS (styled-components, emotion, etc.)
    let js_extensions = ["js", "jsx", "ts", "tsx"];

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_ignored(e.path()))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        let is_css = css_extensions.contains(&ext);
        let is_js = js_extensions.contains(&ext);

        if !is_css && !is_js {
            continue;
        }

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let relative_path = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        // Check custom exclude patterns
        if is_excluded(&relative_path, &opts.exclude) {
            continue;
        }

        if is_css {
            parse_css_file(&content, &relative_path, opts, &mut findings);
        } else if is_js {
            parse_css_in_js(&content, &relative_path, opts, &mut findings);
        }
    }

    // Apply filters
    if opts.zindex_only {
        findings.retain(|f| matches!(f, LayoutFinding::ZIndex { .. }));
    }
    if opts.sticky_only {
        findings.retain(|f| matches!(f, LayoutFinding::Sticky { .. }));
    }
    if opts.grid_only {
        findings.retain(|f| matches!(f, LayoutFinding::Grid { .. }));
    }

    // Filter by min z-index if specified
    if let Some(min_z) = opts.min_zindex {
        findings.retain(|f| match f {
            LayoutFinding::ZIndex { z_index, .. } => *z_index >= min_z,
            _ => true,
        });
    }

    Ok(findings)
}

fn is_ignored(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    path_str.contains("node_modules")
        || path_str.contains(".git")
        || path_str.contains("dist/")
        || path_str.contains("build/")
        || path_str.contains("target/")
        || path_str.contains(".next/")
        || path_str.contains("coverage/")
}

/// Check if path matches any exclude pattern
fn is_excluded(path: &str, exclude_patterns: &[String]) -> bool {
    if exclude_patterns.is_empty() {
        return false;
    }

    for pattern in exclude_patterns {
        // Simple glob matching: support ** and *
        if glob_matches(pattern, path) {
            return true;
        }
    }
    false
}

/// Simple glob pattern matching
/// Supports: ** (any path), * (any segment), literal text
fn glob_matches(pattern: &str, path: &str) -> bool {
    // Normalize paths
    let pattern = pattern.replace('\\', "/");
    let path = path.replace('\\', "/");

    // Handle common patterns
    if pattern.contains("**") {
        // **/ at start means "anywhere in path"
        let core = pattern.trim_start_matches("**/").trim_end_matches("/**");
        if path.contains(core) {
            return true;
        }
    }

    // Simple contains check for patterns like ".obsidian" or "prototype"
    let simple_pattern = pattern
        .trim_start_matches("**/")
        .trim_end_matches("/**")
        .trim_matches('*');

    if !simple_pattern.is_empty() && path.contains(simple_pattern) {
        return true;
    }

    false
}

/// Parse a CSS/SCSS file for layout properties
fn parse_css_file(
    content: &str,
    file_path: &str,
    opts: &LayoutmapOptions,
    findings: &mut Vec<LayoutFinding>,
) {
    // Track current selector for context
    let mut current_selector = String::new();
    let mut brace_depth: usize = 0;

    // Regex patterns (compile once)
    let zindex_re = Regex::new(r"z-index\s*:\s*(-?\d+)").unwrap();
    let position_re = Regex::new(r"position\s*:\s*(sticky|fixed)").unwrap();
    let display_grid_re = Regex::new(r"display\s*:\s*grid").unwrap();
    let display_flex_re = Regex::new(r"display\s*:\s*flex").unwrap();
    let selector_re = Regex::new(r"^([^{]+)\{").unwrap();

    for (line_num, line) in content.lines().enumerate() {
        let line_num = line_num + 1; // 1-indexed
        let trimmed = line.trim();

        // Track brace depth for selector context
        let open_braces = line.matches('{').count();
        let close_braces = line.matches('}').count();

        // Detect selector at start of block
        if let Some(caps) = selector_re.captures(trimmed)
            && brace_depth == 0
        {
            current_selector = caps
                .get(1)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
        }

        brace_depth = brace_depth.saturating_add(open_braces);
        brace_depth = brace_depth.saturating_sub(close_braces);

        // Reset selector when we exit all blocks
        if brace_depth == 0 {
            current_selector.clear();
        }

        // Detect z-index
        if !opts.sticky_only
            && !opts.grid_only
            && let Some(caps) = zindex_re.captures(line)
            && let Some(z_str) = caps.get(1)
            && let Ok(z) = z_str.as_str().parse::<i32>()
        {
            findings.push(LayoutFinding::ZIndex {
                file: file_path.to_string(),
                line: line_num,
                selector: current_selector.clone(),
                z_index: z,
            });
        }

        // Detect sticky/fixed position
        if !opts.zindex_only
            && !opts.grid_only
            && let Some(caps) = position_re.captures(line)
            && let Some(pos) = caps.get(1)
        {
            findings.push(LayoutFinding::Sticky {
                file: file_path.to_string(),
                line: line_num,
                selector: current_selector.clone(),
                position: pos.as_str().to_string(),
            });
        }

        // Detect display: grid
        if !opts.zindex_only && !opts.sticky_only && display_grid_re.is_match(line) {
            findings.push(LayoutFinding::Grid {
                file: file_path.to_string(),
                line: line_num,
                selector: current_selector.clone(),
            });
        }

        // Detect display: flex (only if not filtering)
        if !opts.zindex_only
            && !opts.sticky_only
            && !opts.grid_only
            && display_flex_re.is_match(line)
        {
            findings.push(LayoutFinding::Flex {
                file: file_path.to_string(),
                line: line_num,
                selector: current_selector.clone(),
            });
        }
    }
}

/// Parse CSS-in-JS (styled-components, emotion, etc.)
fn parse_css_in_js(
    content: &str,
    file_path: &str,
    opts: &LayoutmapOptions,
    findings: &mut Vec<LayoutFinding>,
) {
    // Check if file contains styled-components or emotion patterns
    let has_css_in_js = content.contains("styled.")
        || content.contains("styled(")
        || content.contains("css`")
        || content.contains("@emotion");

    if !has_css_in_js {
        return;
    }

    // Extract template literals containing CSS
    let template_re = Regex::new(r"`([^`]*(?:z-index|position|display)[^`]*)`").unwrap();
    let zindex_re = Regex::new(r"z-index\s*:\s*(-?\d+)").unwrap();
    let position_re = Regex::new(r"position\s*:\s*(sticky|fixed)").unwrap();
    let display_grid_re = Regex::new(r"display\s*:\s*grid").unwrap();
    let display_flex_re = Regex::new(r"display\s*:\s*flex").unwrap();

    // Get styled component name for context
    let styled_re = Regex::new(r"(?:const|let)\s+(\w+)\s*=\s*styled").unwrap();

    let mut current_component = String::new();

    for (line_num, line) in content.lines().enumerate() {
        let line_num = line_num + 1;

        // Track component name
        if let Some(caps) = styled_re.captures(line)
            && let Some(name) = caps.get(1)
        {
            current_component = name.as_str().to_string();
        }

        // Look for CSS properties in template literals
        if let Some(caps) = template_re.captures(line) {
            let css_content = caps.get(1).map(|m| m.as_str()).unwrap_or("");

            let selector = if current_component.is_empty() {
                "(inline)".to_string()
            } else {
                current_component.clone()
            };

            // z-index
            if !opts.sticky_only
                && !opts.grid_only
                && let Some(zcaps) = zindex_re.captures(css_content)
                && let Some(z_str) = zcaps.get(1)
                && let Ok(z) = z_str.as_str().parse::<i32>()
            {
                findings.push(LayoutFinding::ZIndex {
                    file: file_path.to_string(),
                    line: line_num,
                    selector: selector.clone(),
                    z_index: z,
                });
            }

            // sticky/fixed
            if !opts.zindex_only
                && !opts.grid_only
                && let Some(pcaps) = position_re.captures(css_content)
                && let Some(pos) = pcaps.get(1)
            {
                findings.push(LayoutFinding::Sticky {
                    file: file_path.to_string(),
                    line: line_num,
                    selector: selector.clone(),
                    position: pos.as_str().to_string(),
                });
            }

            // grid
            if !opts.zindex_only && !opts.sticky_only && display_grid_re.is_match(css_content) {
                findings.push(LayoutFinding::Grid {
                    file: file_path.to_string(),
                    line: line_num,
                    selector: selector.clone(),
                });
            }

            // flex
            if !opts.zindex_only
                && !opts.sticky_only
                && !opts.grid_only
                && display_flex_re.is_match(css_content)
            {
                findings.push(LayoutFinding::Flex {
                    file: file_path.to_string(),
                    line: line_num,
                    selector: selector.clone(),
                });
            }
        }
    }
}
