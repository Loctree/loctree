use std::collections::HashSet;
use std::path::Path;

use crate::types::{FileAnalysis, ImportEntry, ImportKind, ImportResolutionKind};

use super::ast_js;
use super::resolvers::TsPathResolver;

/// Represents a script block extracted from HTML
#[derive(Debug, Clone)]
pub struct ScriptBlock {
    pub content: String,
    pub src: Option<String>,
    pub line_offset: usize,
}

/// Extract script blocks from HTML content
pub fn extract_script_blocks(html: &str) -> Vec<ScriptBlock> {
    let mut blocks = Vec::new();
    let mut current_line = 1;

    // Simple regex-based extraction for <script> tags
    // This is intentionally simple - no full HTML parser needed for this use case
    let mut chars = html.char_indices().peekable();

    while let Some((idx, ch)) = chars.next() {
        if ch == '\n' {
            current_line += 1;
        }

        // Look for opening <script tag
        if ch == '<' {
            // Check if this is a script tag
            let remaining = &html[idx..];
            if remaining.starts_with("<script") {
                // Extract the full opening tag
                if let Some(tag_end_idx) = find_tag_end(remaining) {
                    let tag = &remaining[..tag_end_idx];

                    // Parse src attribute from the opening tag
                    let src = parse_src_attribute(tag);

                    // If it has a src attribute, it's an external script reference
                    if let Some(src_path) = src.clone() {
                        blocks.push(ScriptBlock {
                            content: String::new(),
                            src: Some(src_path),
                            line_offset: current_line,
                        });
                    }

                    // Find the closing </script> tag
                    let after_tag = &html[idx + tag_end_idx..];
                    if let Some(close_idx) = after_tag.find("</script>") {
                        let content = &after_tag[..close_idx];

                        // Only extract inline scripts (no src attribute)
                        if src.is_none() && !content.trim().is_empty() {
                            blocks.push(ScriptBlock {
                                content: content.to_string(),
                                src: None,
                                line_offset: current_line,
                            });
                        }

                        // Count newlines in the script content
                        current_line += content.chars().filter(|&c| c == '\n').count();
                    }

                    // Skip past the opening tag
                    let skip = tag_end_idx.saturating_sub(1);
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
    }

    blocks
}

/// Find the end of an HTML tag (the '>' character)
fn find_tag_end(tag_str: &str) -> Option<usize> {
    tag_str.find('>').map(|i| i + 1)
}

/// Parse the src attribute from a script tag
fn parse_src_attribute(tag: &str) -> Option<String> {
    if let Some(src_start) = tag.find("src=") {
        let after_eq = &tag[src_start + 4..];
        if let Some(quote) = after_eq.chars().next()
            && (quote == '"' || quote == '\'')
        {
            let src_value = after_eq[1..].split(quote).next().unwrap_or("").trim();

            if !src_value.is_empty() {
                return Some(src_value.to_string());
            }
        }
    }

    None
}

/// Analyze HTML file - extract scripts and parse them as JavaScript
pub(crate) fn analyze_html_file(
    content: &str,
    path: &Path,
    root: &Path,
    extensions: Option<&HashSet<String>>,
    ts_resolver: Option<&TsPathResolver>,
    relative: String,
    command_cfg: &super::ast_js::CommandDetectionConfig,
) -> FileAnalysis {
    let mut combined_analysis = FileAnalysis {
        path: relative.clone(),
        language: "html".to_string(),
        ..Default::default()
    };

    let script_blocks = extract_script_blocks(content);

    for block in script_blocks {
        // Handle external script references (src attribute)
        if let Some(src_path) = block.src {
            // Create an import entry for external scripts
            combined_analysis.imports.push(ImportEntry {
                line: None,
                source: src_path.clone(),
                source_raw: src_path.clone(),
                kind: ImportKind::SideEffect,
                resolved_path: None, // Will be resolved later by the resolver
                is_bare: !src_path.starts_with('.') && !src_path.starts_with('/'),
                symbols: vec![],
                resolution: ImportResolutionKind::Unknown, // External script, resolution unknown
                is_type_checking: false,
                is_lazy: false,
                is_crate_relative: false,
                is_super_relative: false,
                is_self_relative: false,
                raw_path: src_path,
                is_mod_declaration: false,
            });
            continue;
        }

        // Parse inline script content as JavaScript
        if !block.content.trim().is_empty() {
            let script_analysis = ast_js::analyze_js_file_ast(
                &block.content,
                path,
                root,
                extensions,
                ts_resolver,
                relative.clone(),
                command_cfg,
            );

            // Merge the analysis results
            merge_analysis(&mut combined_analysis, script_analysis, block.line_offset);
        }
    }

    combined_analysis
}

/// Merge script analysis into the combined HTML analysis
/// Adjusts line numbers based on the script block's position in the HTML
fn merge_analysis(target: &mut FileAnalysis, source: FileAnalysis, line_offset: usize) {
    // Merge imports
    for imp in source.imports {
        target.imports.push(imp);
    }

    // Merge exports (adjust line numbers)
    for mut exp in source.exports {
        if let Some(line) = exp.line {
            exp.line = Some(line + line_offset);
        }
        target.exports.push(exp);
    }

    // Merge reexports
    for re in source.reexports {
        target.reexports.push(re);
    }

    // Merge dynamic imports
    for dyn_imp in source.dynamic_imports {
        target.dynamic_imports.push(dyn_imp);
    }

    // Merge command calls (adjust line numbers)
    for mut cmd in source.command_calls {
        cmd.line += line_offset;
        target.command_calls.push(cmd);
    }

    // Merge event emits/listens (adjust line numbers)
    for mut evt in source.event_emits {
        evt.line += line_offset;
        target.event_emits.push(evt);
    }

    for mut evt in source.event_listens {
        evt.line += line_offset;
        target.event_listens.push(evt);
    }

    // Merge event constants
    target.event_consts.extend(source.event_consts);

    // Merge signature uses (adjust line numbers)
    for mut sig in source.signature_uses {
        sig.line = sig.line.map(|l| l + line_offset);
        target.signature_uses.push(sig);
    }

    // Merge string literals (adjust line numbers)
    for mut lit in source.string_literals {
        lit.line += line_offset;
        target.string_literals.push(lit);
    }

    // Merge local uses (critical for dead code detection)
    for usage in source.local_uses {
        if !target.local_uses.contains(&usage) {
            target.local_uses.push(usage);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_basic_script() {
        let html = r#"
<!DOCTYPE html>
<html>
<head>
    <script>
        console.log("Hello");
    </script>
</head>
</html>
        "#;

        let blocks = extract_script_blocks(html);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].content.contains("console.log"));
    }

    #[test]
    fn test_extract_module_script() {
        let html = r#"
<script type="module">
    import { foo } from './bar.js';
</script>
        "#;

        let blocks = extract_script_blocks(html);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].content.contains("import"));
    }

    #[test]
    fn test_extract_external_script() {
        let html = r#"
<script src="./external.js"></script>
<script src="/absolute/path.js"></script>
<script src="https://cdn.example.com/lib.js"></script>
        "#;

        let blocks = extract_script_blocks(html);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].src, Some("./external.js".to_string()));
        assert_eq!(blocks[1].src, Some("/absolute/path.js".to_string()));
        assert_eq!(
            blocks[2].src,
            Some("https://cdn.example.com/lib.js".to_string())
        );
    }

    #[test]
    fn test_extract_mixed_scripts() {
        let html = r#"
<html>
<script src="external.js"></script>
<script>
    const x = 1;
</script>
<script type="module">
    import { y } from './y.js';
</script>
</html>
        "#;

        let blocks = extract_script_blocks(html);
        assert_eq!(blocks.len(), 3);

        // First: external script
        assert!(blocks[0].src.is_some());

        // Second: inline JavaScript
        assert!(blocks[1].src.is_none());
        assert!(blocks[1].content.contains("const x"));

        // Third: inline module
        assert!(blocks[2].src.is_none());
        assert!(blocks[2].content.contains("import"));
    }

    #[test]
    fn test_empty_script_ignored() {
        let html = r#"
<script></script>
<script>   </script>
        "#;

        let blocks = extract_script_blocks(html);
        // Empty scripts should be filtered out
        assert_eq!(blocks.len(), 0);
    }

    #[test]
    fn test_line_offset_tracking() {
        let html = r#"<!DOCTYPE html>
<html>
<head>
    <script>
        const x = 1;
    </script>
</head>
</html>"#;

        let blocks = extract_script_blocks(html);
        assert_eq!(blocks.len(), 1);
        // Script starts on line 4 (after <!DOCTYPE>, <html>, <head>, <script>)
        assert_eq!(blocks[0].line_offset, 4);
    }
}
