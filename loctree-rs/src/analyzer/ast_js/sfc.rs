//! Single File Component (SFC) script and template extraction.
//!
//! This module handles extraction of script and template content from
//! Svelte (.svelte), Vue (.vue), and Astro (.astro) Single File Components.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RuneKind {
    State,
    Derived,
    Props,
    Bindable,
    Effect,
    Inspect,
    Host,
}

impl RuneKind {
    pub(super) fn export_kind(&self) -> &'static str {
        match self {
            Self::State => "rune_state",
            Self::Derived => "rune_derived",
            Self::Props => "rune_props",
            Self::Bindable => "rune_bindable",
            Self::Effect => "rune_effect",
            Self::Inspect => "rune_inspect_debug",
            Self::Host => "rune_host",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuneDeclaration {
    pub(super) name: String,
    pub(super) kind: RuneKind,
    pub(super) line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SnippetDeclaration {
    pub(super) name: String,
    pub(super) line: usize,
    pub(super) params: Vec<String>,
}

/// Extract script content from a Svelte file.
///
/// Handles both `<script>` and `<script lang="ts">` variants.
pub(super) fn extract_svelte_script(content: &str) -> String {
    extract_sfc_script(content)
}

/// Extract script content from a Vue Single File Component (SFC).
///
/// Handles `<script>`, `<script setup>`, `<script lang="ts">` variants.
pub(super) fn extract_vue_script(content: &str) -> String {
    extract_sfc_script(content)
}

/// Extract Astro frontmatter delimited by leading `---` fence lines.
pub(super) fn extract_astro_frontmatter(content: &str) -> String {
    find_astro_frontmatter_bounds(content)
        .map(|(start, end, _)| content[start..end].to_string())
        .unwrap_or_default()
}

/// Extract Astro template content after frontmatter, or the whole file when no frontmatter exists.
pub(super) fn extract_astro_template(content: &str) -> String {
    find_astro_frontmatter_bounds(content)
        .map(|(_, _, after)| content[after..].to_string())
        .unwrap_or_else(|| content.to_string())
}

/// Extract inline Astro `<script>` blocks from the template body.
pub(super) fn extract_astro_scripts(content: &str) -> String {
    extract_tag_blocks(&extract_astro_template(content), "script")
}

/// Extract inline Astro `<style>` blocks from the template body.
pub(super) fn extract_astro_styles(content: &str) -> String {
    extract_tag_blocks(&extract_astro_template(content), "style")
}

/// Common SFC script extraction used by both Svelte and Vue.
fn extract_sfc_script(content: &str) -> String {
    // Match <script> or <script lang="ts"> or <script module> etc.
    // Use lazy matching to capture all script blocks
    let script_regex = Regex::new(r#"<script[^>]*>([\s\S]*?)</script>"#).ok();

    if let Some(re) = script_regex {
        let mut scripts = Vec::new();
        for caps in re.captures_iter(content) {
            if let Some(script_content) = caps.get(1) {
                scripts.push(script_content.as_str().to_string());
            }
        }
        scripts.join("\n")
    } else {
        String::new()
    }
}

fn find_astro_frontmatter_bounds(content: &str) -> Option<(usize, usize, usize)> {
    let mut offset = 0;
    let mut lines = content.split_inclusive('\n');
    let first = lines.next()?;
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return None;
    }

    let frontmatter_start = first.len();
    offset += first.len();

    for line in lines {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "---" {
            return Some((frontmatter_start, offset, offset + line.len()));
        }
        offset += line.len();
    }

    None
}

fn extract_tag_blocks(content: &str, tag: &str) -> String {
    let pattern = format!(r#"<{tag}[^>]*>([\s\S]*?)</{tag}>"#);
    let Ok(re) = Regex::new(&pattern) else {
        return String::new();
    };

    re.captures_iter(content)
        .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract template content from a Svelte file (everything outside <script> and <style>).
pub(super) fn extract_svelte_template(content: &str) -> String {
    let mut result = content.to_string();
    if let Ok(script_re) = Regex::new(r#"<script[^>]*>[\s\S]*?</script>"#) {
        result = script_re.replace_all(&result, "").to_string();
    }
    if let Ok(style_re) = Regex::new(r#"<style[^>]*>[\s\S]*?</style>"#) {
        result = style_re.replace_all(&result, "").to_string();
    }
    result
}

/// Extract template content from a Vue file (everything inside <template> tags).
pub(super) fn extract_vue_template(content: &str) -> String {
    let template_regex = Regex::new(r#"<template[^>]*>([\s\S]*?)</template>"#).ok();

    if let Some(re) = template_regex {
        let mut templates = Vec::new();
        for caps in re.captures_iter(content) {
            if let Some(template_content) = caps.get(1) {
                templates.push(template_content.as_str().to_string());
            }
        }
        templates.join("\n")
    } else {
        String::new()
    }
}

pub(super) fn extract_svelte5_runes(script_content: &str) -> Vec<RuneDeclaration> {
    let mut declarations = Vec::new();

    if let Ok(re) = Regex::new(
        r#"(?m)\b(?:export\s+)?(?:let|const|var)\s+([A-Za-z_$][\w$]*)\s*=\s*\$(state|derived|bindable)(?:\.(raw|snapshot|by))?\s*\("#,
    ) {
        for caps in re.captures_iter(script_content) {
            let Some(name) = caps.get(1) else {
                continue;
            };
            let Some(kind_match) = caps.get(2) else {
                continue;
            };
            let kind = match kind_match.as_str() {
                "state" => RuneKind::State,
                "derived" => RuneKind::Derived,
                "bindable" => RuneKind::Bindable,
                _ => continue,
            };
            declarations.push(RuneDeclaration {
                name: name.as_str().to_string(),
                kind,
                line: line_for_offset(script_content, name.start()),
            });
        }
    }

    if let Ok(re) = Regex::new(
        r#"(?m)\b(?:export\s+)?(?:let|const|var)\s+([A-Za-z_$][\w$]*)\s*=\s*\$props(?:\.\w+)?\s*\("#,
    ) {
        for caps in re.captures_iter(script_content) {
            if let Some(name) = caps.get(1) {
                declarations.push(RuneDeclaration {
                    name: name.as_str().to_string(),
                    kind: RuneKind::Props,
                    line: line_for_offset(script_content, name.start()),
                });
            }
        }
    }

    if let Ok(re) = Regex::new(
        r#"(?m)\b(?:export\s+)?(?:let|const|var)\s*\{([^}]*)\}\s*=\s*\$props(?:\.\w+)?\s*\("#,
    ) {
        for caps in re.captures_iter(script_content) {
            let Some(bindings) = caps.get(1) else {
                continue;
            };
            let line = line_for_offset(script_content, bindings.start());
            for name in extract_destructured_names(bindings.as_str()) {
                declarations.push(RuneDeclaration {
                    name,
                    kind: RuneKind::Props,
                    line,
                });
            }
        }
    }

    for (rune, kind) in [
        ("effect", RuneKind::Effect),
        ("inspect", RuneKind::Inspect),
        ("host", RuneKind::Host),
    ] {
        let pattern = format!(r#"(?m)\${rune}(?:\.\w+)?\s*\("#);
        let Ok(re) = Regex::new(&pattern) else {
            continue;
        };
        for mat in re.find_iter(script_content) {
            declarations.push(RuneDeclaration {
                name: format!("${rune}"),
                kind: kind.clone(),
                line: line_for_offset(script_content, mat.start()),
            });
        }
    }

    declarations
}

pub(super) fn extract_svelte_snippets(template: &str) -> Vec<SnippetDeclaration> {
    let Ok(re) = Regex::new(r#"\{#snippet\s+([A-Za-z_$][\w$]*)\s*\(([^)]*)\)\}"#) else {
        return Vec::new();
    };

    re.captures_iter(template)
        .filter_map(|caps| {
            let name = caps.get(1)?;
            let params = caps
                .get(2)
                .map(|m| {
                    m.as_str()
                        .split(',')
                        .filter_map(|param| {
                            let name = param
                                .trim()
                                .split([':', '=', ' '])
                                .next()
                                .unwrap_or("")
                                .trim();
                            if name.is_empty() {
                                None
                            } else {
                                Some(name.to_string())
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            Some(SnippetDeclaration {
                name: name.as_str().to_string(),
                line: line_for_offset(template, name.start()),
                params,
            })
        })
        .collect()
}

fn extract_destructured_names(bindings: &str) -> Vec<String> {
    bindings
        .split(',')
        .filter_map(|part| {
            let mut raw = part.trim();
            if raw.is_empty() {
                return None;
            }
            raw = raw.trim_start_matches("...");
            let raw = raw.split('=').next().unwrap_or(raw).trim();
            let name = raw.rsplit(':').next().unwrap_or(raw).trim();
            let name = name.trim_start_matches("...");
            if is_identifier(name) {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

fn line_for_offset(content: &str, offset: usize) -> usize {
    content[..offset].bytes().filter(|b| *b == b'\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vue_script_extraction_basic() {
        let vue_content = r#"
<script>
const message = 'Hello'
export const greeting = message + ' World'
</script>

<template>
  <div>{{ greeting }}</div>
</template>
        "#;

        let extracted = extract_vue_script(vue_content);
        assert!(extracted.contains("const message = 'Hello'"));
        assert!(extracted.contains("export const greeting"));
        assert!(!extracted.contains("<template>"));
    }

    #[test]
    fn test_svelte_template_extraction() {
        let content = r#"
<script>
    let count = 0;
</script>

<button on:click={() => count++}>
    {count}
</button>

<style>
    button { color: red; }
</style>
        "#;

        let template = extract_svelte_template(content);
        assert!(!template.contains("let count = 0"));
        assert!(!template.contains("button { color: red; }"));
        assert!(template.contains("on:click"));
        assert!(template.contains("{count}"));
    }

    #[test]
    fn test_vue_template_extraction() {
        let content = r#"
<script>
    const count = 0;
</script>

<template>
    <button @click="increment">
        {{ count }}
    </button>
</template>

<style scoped>
    button { color: red; }
</style>
        "#;

        let template = extract_vue_template(content);
        assert!(!template.contains("const count = 0"));
        assert!(!template.contains("button { color: red; }"));
        assert!(template.contains("@click"));
        assert!(template.contains("{{ count }}"));
    }

    #[test]
    fn test_astro_frontmatter_extraction() {
        let content = r#"---
import Card from "../components/Card.astro";
export interface Props { title: string; }
const { title } = Astro.props;
---
<Card title={title} />
"#;

        let frontmatter = extract_astro_frontmatter(content);
        assert!(frontmatter.contains("import Card"));
        assert!(frontmatter.contains("export interface Props"));
        assert!(!frontmatter.contains("<Card"));
    }

    #[test]
    fn test_astro_no_frontmatter_is_empty() {
        let content = "<html><body>No frontmatter</body></html>";

        assert_eq!(extract_astro_frontmatter(content), "");
        assert_eq!(extract_astro_template(content), content);
    }

    #[test]
    fn test_astro_scripts_and_styles_extraction() {
        let content = r#"---
const title = "Demo";
---
<script>
import boot from "../boot";
boot();
</script>
<style>
.card { color: red; }
</style>
"#;

        assert!(extract_astro_scripts(content).contains("import boot"));
        assert!(extract_astro_styles(content).contains(".card"));
    }

    #[test]
    fn test_svelte5_rune_extraction() {
        let script = r#"
let count = $state(0);
const doubled = $derived(count * 2);
let raw = $state.raw([]);
let snapshot = $state.snapshot(count);
let computed = $derived.by(() => count + 1);
let name = $bindable("Ada");
$effect(() => console.log(count));
$inspect(count);
$host();
"#;

        let runes = extract_svelte5_runes(script);
        assert!(
            runes
                .iter()
                .any(|r| r.name == "count" && r.kind == RuneKind::State)
        );
        assert!(
            runes
                .iter()
                .any(|r| r.name == "doubled" && r.kind == RuneKind::Derived)
        );
        assert!(
            runes
                .iter()
                .any(|r| r.name == "raw" && r.kind == RuneKind::State)
        );
        assert!(
            runes
                .iter()
                .any(|r| r.name == "snapshot" && r.kind == RuneKind::State)
        );
        assert!(
            runes
                .iter()
                .any(|r| r.name == "computed" && r.kind == RuneKind::Derived)
        );
        assert!(
            runes
                .iter()
                .any(|r| r.name == "name" && r.kind == RuneKind::Bindable)
        );
        assert!(
            runes
                .iter()
                .any(|r| r.name == "$effect" && r.kind == RuneKind::Effect)
        );
        assert!(
            runes
                .iter()
                .any(|r| r.name == "$inspect" && r.kind == RuneKind::Inspect)
        );
        assert!(
            runes
                .iter()
                .any(|r| r.name == "$host" && r.kind == RuneKind::Host)
        );
    }

    #[test]
    fn test_svelte5_props_destructuring_extraction() {
        let script = r#"
let { title, count = 0, value: alias, ...rest } = $props();
"#;

        let names: Vec<_> = extract_svelte5_runes(script)
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert!(names.contains(&"title".to_string()));
        assert!(names.contains(&"count".to_string()));
        assert!(names.contains(&"alias".to_string()));
        assert!(names.contains(&"rest".to_string()));
    }

    #[test]
    fn test_svelte_snippet_extraction() {
        let template = r#"
{#snippet row(item: Item, index)}
  <li>{index}: {item.name}</li>
{/snippet}
"#;

        let snippets = extract_svelte_snippets(template);
        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].name, "row");
        assert_eq!(snippets[0].params, vec!["item", "index"]);
    }
}
