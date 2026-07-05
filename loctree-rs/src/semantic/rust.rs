//! Layer 3 semantic analyzer for Rust source.
//!
//! Inputs:  FileAnalysis (Layer 1 sensor at `analyzer/rust/`) + IdiomRegistry (T0)
//! Outputs: SemanticFacts (T0 contract)
//!
//! Five passes, all idempotent:
//!   1. Idiom classification (per-symbol, by registry name lookup)
//!   2. Trait-impl reachability (`impl Trait for Type` blocks → method names reached)
//!   3. Derive macro reachability (`#[derive(Trait, ...)]` → trait method names reached)
//!   4. cfg-gate classification (`#[cfg(test)]` modules / `#[cfg(feature = "X")]`)
//!   5. `pub use` re-export reachability
//!
//! Layer 1 sensor at `analyzer/rust/` is read-only from this module.
//!
//! Out of scope (deferred to later cuts):
//!   - Macro expansion (treat `#[derive]` as a known pattern; do not expand)
//!   - `cargo expand`-level analysis
//!   - Tauri command detection (Cut 3B T3)
//!   - Tree-sitter migration (Cut 2.5+; current implementation is regex/state-machine)
//!
//! Known limitations of the regex/state-machine approach:
//!   - Strings containing `{` or `}` inside an `impl` block can mislead the
//!     brace-depth tracker. After comment stripping, this is rare in practice.
//!   - Macro-generated impl blocks (other than `#[derive]`) are not inspected.

use std::collections::{BTreeSet, HashMap};

use once_cell::sync::Lazy;
use regex::Regex;

use crate::semantic::{
    Classifier, IdiomRegistry, IdiomTag, ReachReason, RuntimeRole, RuntimeSemanticAnalyzer,
    SemanticFacts, SymbolId, TagSource,
};
use crate::types::{FileAnalysis, Language, Visibility};

/// `FileAnalysis::language` is a string. Layer 1 emits `"rs"` for Rust files
/// via `analyzer/classify::detect_language`.
const LANG_STR: &str = "rs";

pub struct RustRuntimeSemantics;

impl RuntimeSemanticAnalyzer for RustRuntimeSemantics {
    fn language(&self) -> Language {
        Language::Rust
    }

    fn analyze(
        &self,
        files: &[FileAnalysis],
        registry: &IdiomRegistry,
        out: &mut SemanticFacts,
    ) -> anyhow::Result<()> {
        for file in files {
            if file.language != LANG_STR {
                continue;
            }

            self.classify_idioms(file, registry, out);

            // For trait/derive/cfg/pub-use we need the raw file content. Files
            // that vanished between scan and analysis are skipped, not fatal.
            // The path goes through validate-and-canonicalize first so a malformed
            // sensor entry can never reach the filesystem read.
            let Some(content) = crate::semantic::io::try_read_validated_semantic_input(&file.path)?
            else {
                continue;
            };
            let stripped = strip_comments(&content);

            self.classify_trait_impls(file, &stripped, out);
            self.classify_derive_emissions(file, &stripped, out);
            self.classify_cfg_gates(file, &stripped, out);
            self.classify_pub_use_reexports(file, &stripped, out);
        }

        self.compute_reachability(out);
        Ok(())
    }
}

impl RustRuntimeSemantics {
    fn classify_idioms(
        &self,
        file: &FileAnalysis,
        registry: &IdiomRegistry,
        out: &mut SemanticFacts,
    ) {
        for export in &file.exports {
            let Some(entry) = registry.lookup(Language::Rust, &export.name) else {
                continue;
            };
            let symbol_id: SymbolId = format!("{}::{}", file.path, export.name);
            out.idiom_tags.entry(symbol_id).or_default().push(IdiomTag {
                name: entry.name.clone(),
                classifier: entry.classifier.clone(),
                runtime_role: entry.runtime_role.clone(),
                source: TagSource::EmbeddedDefault,
                reasoning: entry.reasoning.clone(),
            });
        }
    }

    /// Find `impl <Trait> for <Type> { ... }` (and inherent `impl <Type> { ... }`)
    /// blocks; mark every `pub fn` declared inside as reached. The Layer 1
    /// sensor flags these as exports because the regex matches `pub fn`, but
    /// they are reached through trait dispatch (`x.method()`) or inherent
    /// method-call syntax, not through static `Type::method` call sites that
    /// the import resolver can see.
    fn classify_trait_impls(&self, file: &FileAnalysis, stripped: &str, out: &mut SemanticFacts) {
        let impls = parse_impl_blocks(stripped);
        if impls.is_empty() {
            return;
        }
        let exported_names: BTreeSet<&str> = file.exports.iter().map(|e| e.name.as_str()).collect();
        for block in impls {
            let (classifier, role, reasoning) = match block.trait_name {
                Some(ref trait_name) => (
                    Classifier::Custom("rust:trait_impl_method".to_string()),
                    RuntimeRole::LibraryHelper,
                    format!(
                        "Defined inside `impl {} for {}` block; reached through {} trait dispatch.",
                        trait_name, block.type_name, trait_name
                    ),
                ),
                None => (
                    Classifier::Custom("rust:inherent_impl_method".to_string()),
                    RuntimeRole::LibraryHelper,
                    format!(
                        "Defined inside inherent `impl {}` block; reached via method-call syntax.",
                        block.type_name
                    ),
                ),
            };
            tag_methods(
                file,
                &block.method_names,
                &exported_names,
                out,
                classifier,
                role,
                reasoning,
            );
        }
    }

    /// Find `#[derive(Trait1, Trait2, ...)]` directives and mark exported
    /// methods whose name matches a canonical method of the derived trait set
    /// as reached. Only conservative, well-known derive traits are inspected;
    /// unknown derives produce no reachability tags.
    fn classify_derive_emissions(
        &self,
        file: &FileAnalysis,
        stripped: &str,
        out: &mut SemanticFacts,
    ) {
        let traits = parse_derive_traits(stripped);
        if traits.is_empty() {
            return;
        }
        let derived_methods: BTreeSet<String> = traits
            .iter()
            .flat_map(|t| derived_method_names(t).iter().map(|s| (*s).to_string()))
            .collect();
        if derived_methods.is_empty() {
            return;
        }
        for export in &file.exports {
            if !derived_methods.contains(&export.name) {
                continue;
            }
            let symbol_id: SymbolId = format!("{}::{}", file.path, export.name);
            out.idiom_tags.entry(symbol_id).or_default().push(IdiomTag {
                name: export.name.clone(),
                classifier: Classifier::Custom("rust:derive_emission".to_string()),
                runtime_role: RuntimeRole::LibraryHelper,
                source: TagSource::InferredFromCode,
                reasoning: format!(
                    "Method name matches a derive-emitted impl for one of: {}.",
                    traits.join(", ")
                ),
            });
        }
    }

    /// Classify `#[cfg(test)]` modules and `#[cfg(feature = "X")]` items as
    /// reached, with a custom classifier so callers can distinguish the
    /// reachability path. cfg-test exports are reached by `cargo test`; cfg-
    /// feature exports are reached when the feature is enabled.
    fn classify_cfg_gates(&self, file: &FileAnalysis, stripped: &str, out: &mut SemanticFacts) {
        let cfg_test_ranges = parse_cfg_test_modules(stripped);
        let cfg_feature_lines = parse_cfg_feature_attrs(stripped);

        for export in &file.exports {
            let Some(line) = export.line else {
                continue;
            };

            if cfg_test_ranges
                .iter()
                .any(|(start, end)| line >= *start && line <= *end)
            {
                let symbol_id: SymbolId = format!("{}::{}", file.path, export.name);
                out.idiom_tags.entry(symbol_id).or_default().push(IdiomTag {
                    name: export.name.clone(),
                    classifier: Classifier::Custom("rust:cfg_test".to_string()),
                    runtime_role: RuntimeRole::LibraryHelper,
                    source: TagSource::InferredFromCode,
                    reasoning: "Defined inside `#[cfg(test)]` mod; reached by `cargo test`.".into(),
                });
                continue;
            }

            if let Some(feature) = cfg_feature_lines.get(&line) {
                let symbol_id: SymbolId = format!("{}::{}", file.path, export.name);
                out.idiom_tags.entry(symbol_id).or_default().push(IdiomTag {
                    name: export.name.clone(),
                    classifier: Classifier::Custom("rust:cfg_feature".to_string()),
                    runtime_role: RuntimeRole::LibraryHelper,
                    source: TagSource::InferredFromCode,
                    reasoning: format!(
                        "Gated by `#[cfg(feature = \"{}\")]`; reached when feature is enabled.",
                        feature
                    ),
                });
            }
        }
    }

    /// Detect `pub use foo::Bar` and `pub use foo::{A, B as C}` and mark
    /// re-exported names as reached. `pub use foo::*` (wildcard) is skipped.
    fn classify_pub_use_reexports(
        &self,
        file: &FileAnalysis,
        stripped: &str,
        out: &mut SemanticFacts,
    ) {
        let reexports = parse_pub_use_targets(stripped);
        let reexported: BTreeSet<&str> = reexports.iter().map(String::as_str).collect();
        if reexports.is_empty() {
            // Still tag `kind == "reexport"` exports — the Layer 1 sensor
            // flagged them as re-exports without us needing to re-parse.
            self.tag_kind_reexports(file, out);
            return;
        }
        for export in &file.exports {
            if export.kind == "reexport" || reexported.contains(export.name.as_str()) {
                let symbol_id: SymbolId = format!("{}::{}", file.path, export.name);
                out.idiom_tags.entry(symbol_id).or_default().push(IdiomTag {
                    name: export.name.clone(),
                    classifier: Classifier::Custom("rust:pub_use_reexport".to_string()),
                    runtime_role: RuntimeRole::PublicEntrypoint,
                    source: TagSource::InferredFromCode,
                    reasoning: "Re-exported via `pub use`; reached through downstream importers."
                        .into(),
                });
            }
        }
    }

    fn tag_kind_reexports(&self, file: &FileAnalysis, out: &mut SemanticFacts) {
        for export in &file.exports {
            if export.kind == "reexport" {
                let symbol_id: SymbolId = format!("{}::{}", file.path, export.name);
                out.idiom_tags.entry(symbol_id).or_default().push(IdiomTag {
                    name: export.name.clone(),
                    classifier: Classifier::Custom("rust:pub_use_reexport".to_string()),
                    runtime_role: RuntimeRole::PublicEntrypoint,
                    source: TagSource::InferredFromCode,
                    reasoning:
                        "Sensor-tagged `reexport` symbol; reached through downstream importers."
                            .into(),
                });
            }
        }
    }

    fn compute_reachability(&self, out: &mut SemanticFacts) {
        // Idiom-tagged symbols whose runtime role implies external invocation
        // are reached even without a static call site.
        let tag_pairs: Vec<(SymbolId, RuntimeRole)> = out
            .idiom_tags
            .iter()
            .flat_map(|(id, tags)| {
                tags.iter()
                    .map(move |t| (id.clone(), t.runtime_role.clone()))
            })
            .collect();
        for (symbol_id, role) in tag_pairs {
            if matches!(
                role,
                RuntimeRole::PrimaryEntrypoint
                    | RuntimeRole::PublicEntrypoint
                    | RuntimeRole::UserFacing
                    | RuntimeRole::LibraryHelper
                    | RuntimeRole::EnvInput
            ) {
                out.reachability.reached_symbols.insert(symbol_id.clone());
                out.reachability
                    .reasons
                    .entry(symbol_id)
                    .or_insert(ReachReason::IdiomRuntimeRole(role));
            }
        }
    }
}

fn tag_methods(
    file: &FileAnalysis,
    method_names: &[String],
    exported_names: &BTreeSet<&str>,
    out: &mut SemanticFacts,
    classifier: Classifier,
    role: RuntimeRole,
    reasoning: String,
) {
    for method in method_names {
        if !exported_names.contains(method.as_str()) {
            continue;
        }
        let symbol_id: SymbolId = format!("{}::{}", file.path, method);
        out.idiom_tags.entry(symbol_id).or_default().push(IdiomTag {
            name: method.clone(),
            classifier: classifier.clone(),
            runtime_role: role.clone(),
            source: TagSource::InferredFromCode,
            reasoning: reasoning.clone(),
        });
    }
}

// ---------------------------------------------------------------------------
// Pure parser helpers — testable without disk access.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct ImplMethodInBlock {
    pub(crate) name: String,
    pub(crate) byte_offset: usize,
    pub(crate) is_async: bool,
    pub(crate) visibility: Visibility,
}

#[derive(Debug, Clone)]
pub(crate) struct ImplBlock {
    /// Trait name when `impl Trait for Type`; None for inherent `impl Type`.
    pub(crate) trait_name: Option<String>,
    /// Type name (or full path tail).
    pub(crate) type_name: String,
    /// Method names defined inside the block.
    pub(crate) method_names: Vec<String>,
    /// Method definitions with source offsets and signature metadata.
    pub(crate) methods: Vec<ImplMethodInBlock>,
}

static RE_IMPL_HEAD: Lazy<Regex> = Lazy::new(|| {
    // `impl<...>? <Path>(?: for <Type>)? {` — optional generics, optional `for` clause,
    // optional `where` clause. Tolerates `:` in path segments for `std::fmt::Display`.
    Regex::new(
        r"(?m)^\s*impl(?:\s*<[^>]*>)?\s+([A-Za-z_][A-Za-z0-9_:]*)(?:\s*<[^>]*>)?(?:\s+for\s+([A-Za-z_][A-Za-z0-9_:]*)(?:\s*<[^>]*>)?)?\s*(?:where\b[^\{]*)?\{",
    )
    .expect("valid impl-head regex")
});

static RE_FN_DECL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?m)^\s*(?P<vis>pub(?:\s*\([^)]*\))?\s+)?(?P<prefix>(?:async\s+|const\s+|unsafe\s+|extern(?:\s+\x22[^\x22]+\x22)?\s+)*)fn\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)",
    )
    .expect("valid fn-decl regex")
});

static RE_DERIVE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"#\[derive\s*\(([^)]+)\)\]").expect("valid derive regex"));

static RE_CFG_TEST_MOD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?m)^\s*#\[cfg\s*\(\s*test\s*\)\]\s+(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{",
    )
    .expect("valid cfg-test-mod regex")
});

static RE_CFG_FEATURE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"#\[cfg\s*\(\s*feature\s*=\s*"([^"]+)"\s*\)\]"#).expect("valid cfg-feature regex")
});

static RE_PUB_USE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*pub(?:\([^)]*\))?\s+use\s+([^;]+);").expect("valid pub-use regex")
});

pub(crate) fn parse_impl_blocks(stripped: &str) -> Vec<ImplBlock> {
    let mut blocks = Vec::new();
    let bytes = stripped.as_bytes();

    for caps in RE_IMPL_HEAD.captures_iter(stripped) {
        let head_match = caps.get(0).expect("regex match has full capture");
        let head_end = head_match.end();
        let first = caps.get(1).map(|m| m.as_str().to_string());
        let second = caps.get(2).map(|m| m.as_str().to_string());

        let (trait_name, type_name) = match (first, second) {
            (Some(t), Some(ty)) => (Some(t), ty),
            (Some(ty), None) => (None, ty),
            _ => continue,
        };

        // Find matching close brace via depth tracking. The opening `{` is at
        // head_end - 1; depth starts at 1 to account for it.
        let mut depth: i32 = 1;
        let mut close: usize = stripped.len();
        for (i, &b) in bytes.iter().enumerate().skip(head_end) {
            if b == b'{' {
                depth += 1;
            } else if b == b'}' {
                depth -= 1;
                if depth == 0 {
                    close = i;
                    break;
                }
            }
        }

        let block_body = &stripped[head_end..close.min(stripped.len())];
        let mut methods = Vec::new();
        let mut method_details = Vec::new();
        for caps in RE_FN_DECL.captures_iter(block_body) {
            if let Some(name) = caps.name("name") {
                let method_name = name.as_str().to_string();
                methods.push(method_name.clone());
                let prefix = caps.name("prefix").map(|m| m.as_str()).unwrap_or_default();
                let visibility = parse_visibility(caps.name("vis").map(|m| m.as_str()));
                method_details.push(ImplMethodInBlock {
                    name: method_name,
                    byte_offset: head_end + name.start(),
                    is_async: prefix.split_whitespace().any(|word| word == "async"),
                    visibility,
                });
            }
        }

        blocks.push(ImplBlock {
            trait_name,
            type_name,
            method_names: methods,
            methods: method_details,
        });
    }

    blocks
}

fn parse_visibility(raw: Option<&str>) -> Visibility {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Visibility::Private;
    };
    if raw == "pub" {
        return Visibility::Public;
    }
    if raw.starts_with("pub(") && raw.ends_with(')') {
        let inner = raw.trim_start_matches("pub(").trim_end_matches(')').trim();
        if inner == "crate" {
            Visibility::Crate
        } else {
            Visibility::Restricted(inner.to_string())
        }
    } else {
        Visibility::Public
    }
}

fn parse_derive_traits(stripped: &str) -> Vec<String> {
    let mut traits: BTreeSet<String> = BTreeSet::new();
    for caps in RE_DERIVE.captures_iter(stripped) {
        let Some(list) = caps.get(1) else {
            continue;
        };
        for name in list.as_str().split(',') {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            // Strip path segments (`serde::Deserialize` -> `Deserialize`).
            let last = name.rsplit("::").next().unwrap_or(name).trim();
            if last.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                traits.insert(last.to_string());
            }
        }
    }
    traits.into_iter().collect()
}

/// Map well-known derive trait names to the methods they emit.
fn derived_method_names(trait_name: &str) -> &'static [&'static str] {
    match trait_name {
        "Debug" | "Display" => &["fmt"],
        "Clone" => &["clone", "clone_from"],
        "Default" => &["default"],
        "Hash" => &["hash"],
        "PartialEq" => &["eq", "ne"],
        "PartialOrd" => &["partial_cmp", "lt", "le", "gt", "ge"],
        "Ord" => &["cmp"],
        "Iterator" => &["next", "size_hint"],
        "Serialize" => &["serialize"],
        "Deserialize" => &["deserialize"],
        "Drop" => &["drop"],
        "From" => &["from"],
        "TryFrom" => &["try_from"],
        "AsRef" => &["as_ref"],
        "AsMut" => &["as_mut"],
        // Eq, Copy, Send, Sync emit no methods of their own.
        _ => &[],
    }
}

/// Find `#[cfg(test)] mod NAME { ... }` blocks and return inclusive
/// (start_line, end_line) pairs (1-based lines) covering attribute through `}`.
fn parse_cfg_test_modules(stripped: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let bytes = stripped.as_bytes();
    for caps in RE_CFG_TEST_MOD.captures_iter(stripped) {
        let head_match = caps.get(0).expect("regex match");
        let head_start = head_match.start();
        let head_end = head_match.end();
        let start_line = byte_offset_to_line(stripped, head_start);

        let mut depth: i32 = 1;
        let mut close: usize = stripped.len();
        for (i, &b) in bytes.iter().enumerate().skip(head_end) {
            if b == b'{' {
                depth += 1;
            } else if b == b'}' {
                depth -= 1;
                if depth == 0 {
                    close = i;
                    break;
                }
            }
        }
        let end_line = byte_offset_to_line(stripped, close.min(stripped.len()));
        out.push((start_line, end_line));
    }
    out
}

/// Find `#[cfg(feature = "X")]` attributes and return a map from the line of
/// the next item declaration to the feature name.
fn parse_cfg_feature_attrs(stripped: &str) -> HashMap<usize, String> {
    let mut map = HashMap::new();
    let lines: Vec<&str> = stripped.lines().collect();
    for (idx, line) in lines.iter().enumerate() {
        let Some(caps) = RE_CFG_FEATURE.captures(line) else {
            continue;
        };
        let Some(feature) = caps.get(1).map(|m| m.as_str().to_string()) else {
            continue;
        };
        if feature.is_empty() {
            continue;
        }
        let mut probe = idx + 1;
        while probe < lines.len() {
            let probe_trim = lines[probe].trim_start();
            if probe_trim.is_empty() || probe_trim.starts_with("#[") {
                probe += 1;
                continue;
            }
            // Lines are 1-based in ExportSymbol; `probe` is 0-based here.
            map.insert(probe + 1, feature.clone());
            break;
        }
    }
    map
}

fn parse_pub_use_targets(stripped: &str) -> Vec<String> {
    let mut targets = Vec::new();
    for caps in RE_PUB_USE.captures_iter(stripped) {
        let Some(body) = caps.get(1).map(|m| m.as_str()) else {
            continue;
        };
        targets.extend(extract_pub_use_names(body));
    }
    targets
}

fn extract_pub_use_names(body: &str) -> Vec<String> {
    let body = body.trim();
    if let Some(brace_open) = body.find('{')
        && let Some(brace_close) = body.rfind('}')
        && brace_close > brace_open
    {
        let inside = &body[brace_open + 1..brace_close];
        return inside
            .split(',')
            .filter_map(|item| pub_use_item_alias(item.trim()))
            .collect();
    }
    if body.ends_with('*') {
        return Vec::new();
    }
    pub_use_item_alias(body).into_iter().collect()
}

fn pub_use_item_alias(item: &str) -> Option<String> {
    let item = item.trim().trim_end_matches(';').trim();
    if item.is_empty() {
        return None;
    }
    let words: Vec<&str> = item.split_whitespace().collect();
    let main = *words.first()?;
    let resolved = if words.get(1).copied() == Some("as") {
        words.get(2).copied()?.to_string()
    } else {
        main.rsplit("::").next().unwrap_or(main).to_string()
    };
    let resolved = resolved.trim().to_string();
    if resolved.is_empty() || resolved == "*" {
        return None;
    }
    Some(resolved)
}

fn byte_offset_to_line(content: &str, offset: usize) -> usize {
    let bound = offset.min(content.len());
    1 + content[..bound].bytes().filter(|b| *b == b'\n').count()
}

/// Strip `// ... ` line comments and `/* ... */` block comments, replacing
/// stripped chars with spaces so column-position semantics are preserved
/// (one input char produces exactly one output char).
///
/// Naive but adequate after Layer 1 has already filtered non-Rust content:
///   - Rust string literals and char literals are skipped intact.
///   - Lifetimes (`'a`, `'static`) are not mistaken for char literals.
///   - Raw strings (`r"..."`, `r#"..."#`) are NOT specifically supported; their
///     contents are walked as if they were normal strings, which is safe for
///     comment stripping (no `//` or `/*` introduced).
///
/// The scan iterates over `chars`, not raw bytes, so multi-byte UTF-8
/// codepoints inside string literals or comments survive intact instead of
/// being shredded into Latin-1 garbage by `byte as char`.
pub(crate) fn strip_comments(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    let mut in_string = false;
    let mut in_char = false;
    let mut in_block_comment = false;

    while let Some(c) = chars.next() {
        if in_block_comment {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                out.push(' ');
                out.push(' ');
                in_block_comment = false;
                continue;
            }
            out.push(if c == '\n' { '\n' } else { ' ' });
            continue;
        }

        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            continue;
        }

        if in_char {
            out.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
                continue;
            }
            if c == '\'' {
                in_char = false;
            }
            continue;
        }

        // Comments.
        if c == '/' && chars.peek() == Some(&'/') {
            // First `/` and the second `/` we're about to consume both
            // become spaces; then every char up to (not including) `\n`
            // also becomes a space.
            chars.next();
            out.push(' ');
            out.push(' ');
            while let Some(&peeked) = chars.peek() {
                if peeked == '\n' {
                    break;
                }
                chars.next();
                out.push(' ');
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            in_block_comment = true;
            out.push(' ');
            out.push(' ');
            continue;
        }

        // String start.
        if c == '"' {
            in_string = true;
            out.push(c);
            continue;
        }

        // Char vs lifetime: `'a` / `'static` (alphanumeric+underscore alpha
        // suffix) is a lifetime, not a char literal. A char literal looks like
        // `'x'` or `'\n'` — single token followed by closing quote.
        if c == '\'' {
            // Need 2-3 chars of lookahead. `Peekable<Chars>` is `Clone`, so
            // we fork a copy and walk it without disturbing the main cursor.
            let mut look = chars.clone();
            let look1 = look.next();
            let look2 = look.next();
            let look3 = look.next();
            let is_char_literal = if look1 == Some('\\') {
                // `'\X'` escaped char.
                look3 == Some('\'')
            } else {
                look2 == Some('\'')
            };
            if is_char_literal {
                in_char = true;
                out.push(c);
                continue;
            }
            // Lifetime — copy verbatim.
            out.push(c);
            continue;
        }

        out.push(c);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ExportSymbol;
    use std::path::Path;

    fn rust_file(path: &str, exports: &[(&str, &str, Option<usize>)]) -> FileAnalysis {
        let mut fa = FileAnalysis::new(path.to_string());
        fa.language = "rs".to_string();
        for (name, kind, line) in exports {
            fa.exports
                .push(ExportSymbol::new((*name).to_string(), kind, "named", *line));
        }
        fa
    }

    fn write_tmp(dir: &Path, name: &str, content: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, content).expect("write tmp file");
        path.to_string_lossy().into_owned()
    }

    // ---- comment stripping ----------------------------------------------------

    #[test]
    fn strip_comments_preserves_utf8() {
        // String literal contents must survive intact; the previous byte-loop
        // shredded multi-byte UTF-8 chars into Latin-1 garbage by pushing
        // each input byte as its own `char`.
        let input = "let s = \"café\";\n";
        let out = strip_comments(input);
        assert!(
            out.contains("\"café\""),
            "string content must keep multi-byte chars intact: {out:?}"
        );

        // Comments containing UTF-8 should be replaced with spaces (one space
        // per input char), while the trailing newline is preserved verbatim.
        let input = "// żółć comment\nlet x = 1;\n";
        let out = strip_comments(input);
        let lines: Vec<&str> = out.split('\n').collect();
        assert_eq!(lines.len(), 3, "newlines preserved: {out:?}");
        // First line is the stripped comment — every char must be a space.
        assert!(
            lines[0].chars().all(|c| c == ' '),
            "comment line must be all spaces: {:?}",
            lines[0]
        );
        // One space per input char (`// żółć comment` = 15 chars).
        assert_eq!(lines[0].chars().count(), 15);
        // Code line survives.
        assert_eq!(lines[1], "let x = 1;");

        // Block comment with UTF-8 content.
        let input = "let a = 1; /* zażółć */ let b = 2;";
        let out = strip_comments(input);
        assert!(out.contains("let a = 1;"));
        assert!(out.contains("let b = 2;"));
        assert!(!out.contains("zażółć"));
    }

    // ---- idiom classification ------------------------------------------------

    #[test]
    fn idiom_classify_canonical_rust_names() {
        let analyzer = RustRuntimeSemantics;
        let registry = IdiomRegistry::load_defaults().unwrap();
        let files = vec![rust_file(
            "/synthetic/lib.rs",
            &[
                ("main", "function", Some(1)),
                ("new", "function", Some(2)),
                ("from", "function", Some(3)),
                ("clone", "function", Some(4)),
                ("fmt", "function", Some(5)),
                ("drop", "function", Some(6)),
                ("serialize", "function", Some(7)),
                ("deserialize", "function", Some(8)),
            ],
        )];
        let mut out = SemanticFacts::default();
        analyzer.analyze(&files, &registry, &mut out).unwrap();
        let names: BTreeSet<&str> = out
            .idiom_tags
            .values()
            .flatten()
            .map(|t| t.name.as_str())
            .collect();
        for expected in [
            "main",
            "new",
            "from",
            "clone",
            "fmt",
            "drop",
            "serialize",
            "deserialize",
        ] {
            assert!(
                names.contains(expected),
                "expected idiom `{expected}` to classify; got: {names:?}"
            );
        }
    }

    #[test]
    fn idiom_main_is_primary_entrypoint() {
        let registry = IdiomRegistry::load_defaults().unwrap();
        let entry = registry
            .lookup(Language::Rust, "main")
            .expect("main idiom registered");
        assert_eq!(entry.classifier, Classifier::PrimaryEntrypoint);
        assert_eq!(entry.runtime_role, RuntimeRole::PrimaryEntrypoint);
    }

    // ---- trait-impl reachability --------------------------------------------

    #[test]
    fn trait_impl_method_marked_reached() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "trait_impl.rs",
            r#"
pub struct Greeter;

impl std::fmt::Display for Greeter {
    pub fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) }
}
"#,
        );
        // Layer 1 sensor extracts `pub fn fmt` from inside the impl block.
        let fa = rust_file(
            &path,
            &[("Greeter", "struct", Some(2)), ("fmt", "function", Some(5))],
        );
        let mut out = SemanticFacts::default();
        RustRuntimeSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        let id = format!("{}::fmt", path);
        assert!(
            out.reachability.reached_symbols.contains(&id),
            "fmt should be reached via trait dispatch; reached={:?}",
            out.reachability.reached_symbols
        );
        let expected = Classifier::Custom("rust:trait_impl_method".to_string());
        let has_expected = out
            .idiom_tags
            .get(&id)
            .map(|tags| tags.iter().any(|t| t.classifier == expected))
            .unwrap_or(false);
        assert!(
            has_expected,
            "trait_impl_method classifier missing for {id}"
        );
    }

    #[test]
    fn inherent_impl_methods_marked_reached() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "inherent.rs",
            r#"
pub struct Cache;

impl Cache {
    pub fn lookup(&self, key: &str) -> Option<&str> { None }
    pub fn store(&mut self, k: String, v: String) {}
}
"#,
        );
        let fa = rust_file(
            &path,
            &[
                ("Cache", "struct", Some(2)),
                ("lookup", "function", Some(5)),
                ("store", "function", Some(6)),
            ],
        );
        let mut out = SemanticFacts::default();
        RustRuntimeSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        let lookup_id = format!("{}::lookup", path);
        let store_id = format!("{}::store", path);
        assert!(out.reachability.reached_symbols.contains(&lookup_id));
        assert!(out.reachability.reached_symbols.contains(&store_id));
    }

    // ---- derive emission -----------------------------------------------------

    #[test]
    fn derive_emitted_methods_marked_reached() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "derive.rs",
            r#"
#[derive(Debug, Clone, serde::Serialize)]
pub struct Config { pub name: String }

pub fn fmt() {}
pub fn clone() {}
pub fn serialize() {}
"#,
        );
        let fa = rust_file(
            &path,
            &[
                ("Config", "struct", Some(3)),
                ("fmt", "function", Some(5)),
                ("clone", "function", Some(6)),
                ("serialize", "function", Some(7)),
            ],
        );
        let mut out = SemanticFacts::default();
        RustRuntimeSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        let names: BTreeSet<&str> = out
            .idiom_tags
            .values()
            .flatten()
            .filter(
                |t| matches!(&t.classifier, Classifier::Custom(c) if c == "rust:derive_emission"),
            )
            .map(|t| t.name.as_str())
            .collect();
        assert!(names.contains("fmt"), "derive(Debug) should reach fmt");
        assert!(names.contains("clone"), "derive(Clone) should reach clone");
        assert!(
            names.contains("serialize"),
            "derive(Serialize) should reach serialize"
        );
    }

    // ---- cfg gates ---------------------------------------------------------

    #[test]
    fn cfg_test_module_marks_inner_exports_reached() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "cfg_test.rs",
            r#"
pub fn production() {}

#[cfg(test)]
mod tests {
    pub fn helper() {}
    pub fn another_helper() {}
}
"#,
        );
        let fa = rust_file(
            &path,
            &[
                ("production", "function", Some(2)),
                ("helper", "function", Some(6)),
                ("another_helper", "function", Some(7)),
            ],
        );
        let mut out = SemanticFacts::default();
        RustRuntimeSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        let helper_id = format!("{}::helper", path);
        let another_id = format!("{}::another_helper", path);
        assert!(
            out.reachability.reached_symbols.contains(&helper_id),
            "helper inside cfg(test) mod should be reached"
        );
        assert!(out.reachability.reached_symbols.contains(&another_id));
        let cfg_test_tags: usize = out
            .idiom_tags
            .values()
            .flatten()
            .filter(|t| matches!(&t.classifier, Classifier::Custom(c) if c == "rust:cfg_test"))
            .count();
        assert!(
            cfg_test_tags >= 2,
            "expected ≥2 cfg_test tags, got {cfg_test_tags}"
        );
    }

    #[test]
    fn cfg_feature_attached_function_classified() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "cfg_feature.rs",
            r#"
#[cfg(feature = "memex")]
pub fn memex_index() {}
"#,
        );
        let fa = rust_file(&path, &[("memex_index", "function", Some(3))]);
        let mut out = SemanticFacts::default();
        RustRuntimeSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        let id = format!("{}::memex_index", path);
        let tag = out
            .idiom_tags
            .get(&id)
            .and_then(|tags| {
                tags.iter().find(
                    |t| matches!(&t.classifier, Classifier::Custom(c) if c == "rust:cfg_feature"),
                )
            })
            .expect("cfg_feature tag missing");
        assert!(
            tag.reasoning.contains("memex"),
            "feature name should be in reasoning: {}",
            tag.reasoning
        );
        assert!(out.reachability.reached_symbols.contains(&id));
    }

    // ---- pub use re-export -------------------------------------------------

    #[test]
    fn pub_use_target_marked_reachable() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            tmp.path(),
            "reexport.rs",
            "pub use crate::inner::PublicThing;\n",
        );
        // Sensor flags this as `kind=reexport` — we should also tag via parse path.
        let fa = rust_file(&path, &[("PublicThing", "reexport", Some(1))]);
        let mut out = SemanticFacts::default();
        RustRuntimeSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        let id = format!("{}::PublicThing", path);
        assert!(
            out.reachability.reached_symbols.contains(&id),
            "pub use target should be reached"
        );
    }

    // ---- robustness --------------------------------------------------------

    #[test]
    fn unrelated_language_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(tmp.path(), "x.py", "def fmt(): pass\n");
        let mut fa = rust_file(&path, &[("fmt", "function", Some(1))]);
        fa.language = "py".to_string();
        let mut out = SemanticFacts::default();
        RustRuntimeSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        assert!(out.idiom_tags.is_empty());
        assert!(out.reachability.reached_symbols.is_empty());
    }

    #[test]
    fn missing_file_does_not_panic() {
        let fa = rust_file("/nonexistent/missing.rs", &[("main", "function", Some(1))]);
        let mut out = SemanticFacts::default();
        RustRuntimeSemantics
            .analyze(&[fa], &IdiomRegistry::load_defaults().unwrap(), &mut out)
            .unwrap();
        // Idiom classification still runs from in-memory exports.
        assert_eq!(out.idiom_tags.len(), 1);
    }

    // ---- pure helpers -------------------------------------------------------

    #[test]
    fn parse_derive_traits_handles_paths_and_aliases() {
        let traits =
            parse_derive_traits("#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]");
        assert!(traits.contains(&"Debug".to_string()));
        assert!(traits.contains(&"Clone".to_string()));
        assert!(traits.contains(&"Serialize".to_string()));
        assert!(traits.contains(&"Deserialize".to_string()));
    }

    #[test]
    fn parse_pub_use_targets_handles_braces_and_aliases() {
        let targets = parse_pub_use_targets(
            "pub use crate::types::{ImportEntry, ExportSymbol as Export};\npub use crate::reg::Reg;\npub(crate) use crate::lib::Inner;\n",
        );
        assert!(targets.contains(&"ImportEntry".to_string()));
        assert!(targets.contains(&"Export".to_string()));
        assert!(targets.contains(&"Reg".to_string()));
        assert!(targets.contains(&"Inner".to_string()));
    }

    #[test]
    fn parse_pub_use_targets_skips_wildcard() {
        let targets = parse_pub_use_targets("pub use std::sync::*;\n");
        assert!(
            targets.is_empty(),
            "wildcard pub use should not produce targets, got {targets:?}"
        );
    }

    #[test]
    fn comment_strip_blanks_block_and_line_comments() {
        let stripped = strip_comments("a /* foo bar */ b\nc // end\nd");
        assert!(stripped.contains("a "));
        assert!(stripped.contains(" b"));
        assert!(!stripped.contains("foo bar"));
        assert!(!stripped.contains("end"));
        assert!(stripped.contains("c "));
        assert!(stripped.contains("d"));
        // Newlines preserved.
        assert_eq!(stripped.lines().count(), 3);
    }

    #[test]
    fn comment_strip_preserves_string_literals_and_lifetimes() {
        let stripped = strip_comments(r#"let s: &'static str = "hello // not a comment"; "#);
        assert!(stripped.contains("'static"));
        assert!(stripped.contains("hello // not a comment"));
    }

    #[test]
    fn parse_impl_blocks_extracts_trait_and_methods() {
        let source = r#"
impl Display for Foo {
    pub fn fmt(&self) -> Result {}
}

impl Foo {
    pub fn bar() {}
}
"#;
        let blocks = parse_impl_blocks(source);
        assert_eq!(blocks.len(), 2);
        let trait_block = blocks.iter().find(|b| b.trait_name.is_some()).unwrap();
        assert_eq!(trait_block.trait_name.as_deref(), Some("Display"));
        assert_eq!(trait_block.type_name, "Foo");
        assert_eq!(trait_block.method_names, vec!["fmt".to_string()]);
        assert_eq!(
            byte_offset_to_line(source, trait_block.methods[0].byte_offset),
            3
        );
        let inherent_block = blocks.iter().find(|b| b.trait_name.is_none()).unwrap();
        assert_eq!(inherent_block.type_name, "Foo");
        assert_eq!(inherent_block.method_names, vec!["bar".to_string()]);
        assert_eq!(
            byte_offset_to_line(source, inherent_block.methods[0].byte_offset),
            7
        );
    }
}
