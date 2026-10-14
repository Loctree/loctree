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

    // H6: Single File Components carry their layout CSS inside a `<style>`
    // block, not as a whole-file CSS/CSS-in-JS document - the extension
    // lists above skipped them entirely, so a Svelte/Astro-only design
    // system produced zero layout findings no matter how much CSS it had.
    let sfc_style_extensions = ["svelte", "astro"];

    // H6: best-effort `.gitignore` augmentation of the hardcoded ignore list
    // in `is_ignored` — see `gitignore_patterns` doc comment for why this is
    // additive-only, not a full replacement.
    let extra_ignore_patterns = gitignore_patterns(root);

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_ignored(e.path(), &extra_ignore_patterns))
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
        let is_sfc_style = sfc_style_extensions.contains(&ext);

        if !is_css && !is_js && !is_sfc_style {
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
        } else if is_sfc_style {
            for block in extract_style_blocks(&content) {
                let mut block_findings = Vec::new();
                parse_css_file(&block.content, &relative_path, opts, &mut block_findings);
                for mut finding in block_findings {
                    adjust_finding_line(&mut finding, block.line_offset);
                    findings.push(finding);
                }
            }
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

/// H6: best-effort `.gitignore`-aware augmentation of the hardcoded ignore
/// list in [`is_ignored`]. Full gitignore semantics (negation, per-directory
/// precedence, nested `.gitignore` files, `**` globs) are out of scope for a
/// dependency-free implementation — adding the `ignore` crate would be the
/// principled fix, but that is a `Cargo.toml` edit outside this executor's
/// file-edit scope (COMPILE_EMBARGO), and `scan_css_layout`'s public
/// signature can't grow a `Snapshot` parameter (the H5 fallback used in
/// `env_truth/discovery.rs`) without breaking its caller
/// (`cli/dispatch/handlers/analysis.rs`, also not owned by this executor).
///
/// Reads `<root>/.gitignore` once and treats each plain (non-glob) line as
/// an additional "path contains this segment" rule, layered ON TOP OF (never
/// instead of) the hardcoded baseline in `is_ignored` — so this can only
/// ever ignore MORE paths, never less, and can't regress anything the
/// baseline already covered. Lines containing `*` are skipped rather than
/// interpreted as globs, to avoid a wrong substring match masquerading as a
/// glob match.
fn gitignore_patterns(root: &Path) -> Vec<String> {
    let Ok(content) = fs::read_to_string(root.join(".gitignore")) else {
        return Vec::new();
    };
    content
        .lines()
        .map(str::trim)
        // `!negation` lines are skipped by name: these rules only ever ADD
        // ignores on top of the baseline, so un-ignoring is out of scope
        // (audit N08 names this boundary instead of silently mangling it).
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('!'))
        // Globs: the common `*.ext` form is honored below; any other glob is
        // skipped rather than misread as a substring.
        .filter(|line| !line.contains('*') || (line.starts_with("*.") && !line[1..].contains('*')))
        .map(|line| {
            line.trim_start_matches('/')
                .trim_end_matches('/')
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

/// Does `pattern` match `path_str` on path-segment boundaries? A raw
/// substring check let `cache` swallow `src/cachetools.css` (audit N08).
fn pattern_matches_path(path_str: &str, pattern: &str) -> bool {
    if let Some(ext_pattern) = pattern.strip_prefix('*') {
        // `*.ext` — file-name suffix match.
        return path_str.ends_with(ext_pattern);
    }
    let mut from = 0usize;
    while let Some(pos) = path_str[from..].find(pattern) {
        let begin = from + pos;
        let end = begin + pattern.len();
        let bytes = path_str.as_bytes();
        let before_ok = begin == 0 || bytes[begin - 1] == b'/';
        let after_ok = end == bytes.len() || bytes[end] == b'/';
        if before_ok && after_ok {
            return true;
        }
        from = begin + 1;
    }
    false
}

fn is_ignored(path: &Path, extra_patterns: &[String]) -> bool {
    let path_str = path.to_string_lossy();
    let baseline = path_str.contains("node_modules")
        || path_str.contains(".git")
        || path_str.contains("dist/")
        || path_str.contains("build/")
        || path_str.contains("target/")
        || path_str.contains(".next/")
        || path_str.contains("coverage/");
    baseline
        || extra_patterns
            .iter()
            .any(|p| pattern_matches_path(&path_str, p))
}

/// A `<style>` block extracted from a Single File Component (Svelte
/// `.svelte` / Astro `.astro`), with the 1-based line number where its
/// content begins in the ORIGINAL file (so findings inside it can be
/// translated back to real file line numbers via [`adjust_finding_line`]).
struct StyleBlock {
    content: String,
    line_offset: usize,
}

/// Extract `<style>...</style>` block contents from an SFC source file.
///
/// Mirrors `html_analyzer::extract_script_blocks`'s simple, regex-free
/// tag-scanning approach (deliberately not a full HTML/template parser) but
/// for `<style>` instead of `<script>`. Unlike that function before the
/// H3(c) fix, this one advances the char iterator past content + closing
/// tag in a single step from the start, so it never double-counts a
/// newline — line counting has exactly one owner here.
fn extract_style_blocks(source: &str) -> Vec<StyleBlock> {
    let mut blocks = Vec::new();
    let mut current_line = 1;
    let mut chars = source.char_indices().peekable();

    while let Some((idx, ch)) = chars.next() {
        if ch == '\n' {
            current_line += 1;
        }

        if ch == '<' {
            let remaining = &source[idx..];
            if remaining.starts_with("<style")
                && let Some(tag_end_idx) = remaining.find('>').map(|i| i + 1)
            {
                let after_tag = &source[idx + tag_end_idx..];
                let consumed_len = if let Some(close_idx) = after_tag.find("</style>") {
                    let content = &after_tag[..close_idx];
                    if !content.trim().is_empty() {
                        blocks.push(StyleBlock {
                            content: content.to_string(),
                            line_offset: current_line,
                        });
                    }
                    tag_end_idx + close_idx + "</style>".len()
                } else {
                    tag_end_idx
                };

                let skip = consumed_len.saturating_sub(1);
                for _ in 0..skip {
                    if let Some((_, ch)) = chars.next()
                        && ch == '\n'
                    {
                        current_line += 1;
                    }
                }
            }
        }
    }

    blocks
}

/// Translate a finding's line number (relative to a style block's own
/// content, 1-based from that block's first line) back to the real line
/// number in the original SFC source file.
fn adjust_finding_line(finding: &mut LayoutFinding, block_line_offset: usize) {
    let delta = block_line_offset.saturating_sub(1);
    match finding {
        LayoutFinding::ZIndex { line, .. }
        | LayoutFinding::Sticky { line, .. }
        | LayoutFinding::Grid { line, .. }
        | LayoutFinding::Flex { line, .. } => *line += delta,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// H6 RED: a `.svelte` component's `<style>` block must be scanned for
    /// layout classes/selectors — the old css/js-only extension lists made
    /// Svelte/Astro-only design systems invisible to `loct layoutmap`.
    #[test]
    fn svelte_style_block_layout_findings_recorded() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("HeroSection.svelte"),
            r#"<script lang="ts">
    let count = 0;
</script>

<div class="hero">{count}</div>

<style>
.hero {
    position: sticky;
    z-index: 42;
    display: grid;
}
</style>
"#,
        )
        .unwrap();

        let findings = scan_css_layout(root, &LayoutmapOptions::default()).unwrap();
        assert!(
            findings
                .iter()
                .any(|f| matches!(f, LayoutFinding::ZIndex { z_index: 42, .. })),
            "z-index inside a .svelte <style> block must be recorded, found: {:?}",
            findings
        );
        assert!(
            findings
                .iter()
                .any(|f| matches!(f, LayoutFinding::Sticky { .. })),
            "sticky position inside a .svelte <style> block must be recorded, found: {:?}",
            findings
        );
        assert!(
            findings
                .iter()
                .any(|f| matches!(f, LayoutFinding::Grid { .. })),
            "display: grid inside a .svelte <style> block must be recorded, found: {:?}",
            findings
        );
    }

    /// H6: line numbers reported for a `<style>` block finding must map back
    /// to the REAL file line, not the offset-within-the-block line.
    #[test]
    fn svelte_style_block_finding_line_is_real_file_line() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // z-index sits on line 7 of the real file (1-based).
        fs::write(
            root.join("Panel.svelte"),
            "<script>\n  let x = 1;\n</script>\n\n<div />\n\n<style>\n.panel { z-index: 7; }\n</style>\n",
        )
        .unwrap();

        let findings = scan_css_layout(root, &LayoutmapOptions::default()).unwrap();
        let z = findings
            .iter()
            .find_map(|f| match f {
                LayoutFinding::ZIndex {
                    line, z_index: 7, ..
                } => Some(*line),
                _ => None,
            })
            .expect("z-index finding must be present");
        assert_eq!(z, 8, "the z-index line inside <style> is real file line 8");
    }

    /// H6: `.gitignore`-listed directories are ignored on top of the
    /// hardcoded baseline, without needing the `ignore` crate or a signature
    /// change to `scan_css_layout`.
    #[test]
    fn gitignored_directory_is_skipped() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "vendor-styles\n").unwrap();
        fs::create_dir_all(root.join("vendor-styles")).unwrap();
        fs::write(root.join("vendor-styles/lib.css"), ".x { z-index: 999; }\n").unwrap();
        fs::write(root.join("app.css"), ".y { z-index: 1; }\n").unwrap();

        let findings = scan_css_layout(root, &LayoutmapOptions::default()).unwrap();
        assert!(
            !findings
                .iter()
                .any(|f| matches!(f, LayoutFinding::ZIndex { z_index: 999, .. })),
            "gitignored vendor-styles/ must be skipped, found: {:?}",
            findings
        );
        assert!(
            findings
                .iter()
                .any(|f| matches!(f, LayoutFinding::ZIndex { z_index: 1, .. })),
            "non-ignored app.css must still be scanned"
        );
    }
    /// N08 (audit): gitignore-derived patterns match on path segments, not
    /// raw substrings, and the common `*.ext` glob works.
    #[test]
    fn gitignore_patterns_match_segments_not_substrings() {
        assert!(pattern_matches_path("src/cache/x.css", "cache"));
        assert!(!pattern_matches_path("src/cachetools/x.css", "cache"));
        assert!(pattern_matches_path("a/b/generated.min.css", "*.min.css"));
        assert!(!pattern_matches_path("a/b/generated.css", "*.min.css"));
        assert!(pattern_matches_path("vendor/lib/x.css", "vendor/lib"));
    }
}
