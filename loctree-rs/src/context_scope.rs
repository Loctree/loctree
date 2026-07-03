use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use globset::{GlobBuilder, GlobMatcher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use strsim::levenshtein;

use crate::snapshot::Snapshot;

const SELECTOR_KINDS: [&str; 4] = ["path", "tag", "import", "reach"];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScopeReport {
    pub selectors: Vec<String>,
    pub matched_files: usize,
    pub empty: bool,
    pub fingerprint: String,
    pub named_resolved_from: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_selectors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selector_match_counts: Vec<SelectorMatchCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectorMatchCount {
    pub selector: String,
    pub matched_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskReport {
    pub text: String,
    pub mode: String,
    pub authority: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedScope {
    pub report: ScopeReport,
    matched: HashSet<String>,
}

impl ResolvedScope {
    pub fn matched_files(&self) -> Vec<String> {
        let mut files: Vec<String> = self.matched.iter().cloned().collect();
        files.sort();
        files
    }

    pub fn contains(&self, file: &str) -> bool {
        self.matched.contains(file)
    }
}

#[derive(Debug)]
pub enum ScopeError {
    InvalidGlob {
        pattern: String,
        source: globset::Error,
    },
    NamedScopeNotFound {
        name: String,
        available: String,
        suggestion: String,
    },
    EmptyScope {
        selectors: Vec<String>,
        hint: String,
        selector_match_counts: Vec<SelectorMatchCount>,
    },
    ScopesConfigInvalid(toml::de::Error),
    ScopesConfigRead(std::io::Error),
    SymbolNotFound(String),
}

impl std::fmt::Display for ScopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidGlob { pattern, source } => {
                write!(f, "invalid glob pattern '{pattern}': {source}")
            }
            Self::NamedScopeNotFound {
                name,
                available,
                suggestion,
            } => {
                writeln!(
                    f,
                    "scope not found: '{name}' is not a known selector kind or named scope."
                )?;
                writeln!(f, "Supported selector kinds: path:, tag:, import:, reach:")?;
                if available.is_empty() {
                    write!(
                        f,
                        "No named scopes are configured. Use explicit selector syntax, e.g. `--scope path:core/` or `--scope path:src/agent/`."
                    )
                } else {
                    write!(f, "Available named scopes: [{available}]{suggestion}")
                }
            }
            Self::EmptyScope {
                selectors,
                hint,
                selector_match_counts,
            } => {
                writeln!(f, "--scope matched zero files: {}", selectors.join(", "))?;
                if !hint.is_empty() {
                    writeln!(f, "hint: {hint}")?;
                }
                if selector_match_counts.len() > 1
                    && selector_match_counts
                        .iter()
                        .any(|count| count.matched_files > 0)
                {
                    let counts = selector_match_counts
                        .iter()
                        .map(|count| format!("{}={}", count.selector, count.matched_files))
                        .collect::<Vec<_>>()
                        .join(", ");
                    write!(
                        f,
                        "note: repeated --scope selectors are intersected (AND); individual matches: {counts}"
                    )
                } else {
                    Ok(())
                }
            }
            Self::ScopesConfigInvalid(err) => {
                write!(f, "failed to parse .loctree/scopes.toml: {err}")
            }
            Self::ScopesConfigRead(err) => {
                write!(f, "failed to read .loctree/scopes.toml: {err}")
            }
            Self::SymbolNotFound(symbol) => write!(
                f,
                "symbol not found in dispatch graph: '{symbol}'. Run `loct find --mode where-symbol <name>` to verify."
            ),
        }
    }
}

impl std::error::Error for ScopeError {}

impl From<toml::de::Error> for ScopeError {
    fn from(value: toml::de::Error) -> Self {
        Self::ScopesConfigInvalid(value)
    }
}

impl From<std::io::Error> for ScopeError {
    fn from(value: std::io::Error) -> Self {
        Self::ScopesConfigRead(value)
    }
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct ScopesConfig {
    #[serde(default)]
    pub scopes: HashMap<String, NamedScope>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NamedScope {
    pub description: Option<String>,
    pub selectors: Vec<String>,
}

impl ScopesConfig {
    pub fn load(project_root: &Path) -> Result<Self, ScopeError> {
        let path = project_root.join(".loctree").join("scopes.toml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let body = fs::read_to_string(path)?;
        Ok(toml::from_str(&body)?)
    }

    pub fn available_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.scopes.keys().cloned().collect();
        names.sort();
        names
    }
}

#[derive(Debug)]
enum Selector {
    Path { matcher: PathMatcher },
    Tag(String),
    Import(String),
    Reach { files: HashSet<String> },
}

#[derive(Debug)]
enum PathMatcher {
    Prefix(String),
    Glob(GlobMatcher),
}

impl PathMatcher {
    fn matches(&self, path: &str) -> bool {
        match self {
            Self::Prefix(prefix) => path == prefix || path.starts_with(prefix),
            Self::Glob(matcher) => matcher.is_match(path),
        }
    }
}

pub fn resolve_scope(
    raw: &[String],
    project_root: &Path,
    snapshot: &Snapshot,
) -> Result<ResolvedScope, ScopeError> {
    let config = ScopesConfig::load(project_root)?;
    resolve_scope_with_config(raw, &config, snapshot)
}

pub fn resolve_scope_with_config(
    raw: &[String],
    config: &ScopesConfig,
    snapshot: &Snapshot,
) -> Result<ResolvedScope, ScopeError> {
    let mut expanded = Vec::new();
    let mut named = Vec::new();
    for value in raw {
        expand_selector_value(value, config, &mut expanded, &mut named)?;
    }

    let mut selectors = Vec::new();
    for value in &expanded {
        selectors.push(parse_selector(value, snapshot)?);
    }

    let selector_match_counts = count_selector_matches(&expanded, &selectors, snapshot);

    let mut matched = HashSet::new();
    for file in &snapshot.files {
        if selectors
            .iter()
            .all(|selector| selector_matches(selector, &file.path, snapshot))
        {
            matched.insert(file.path.clone());
        }
    }

    let fingerprint = fingerprint_selectors(&expanded);
    let report = ScopeReport {
        selectors: raw.to_vec(),
        matched_files: matched.len(),
        empty: matched.is_empty(),
        fingerprint,
        named_resolved_from: if named.is_empty() {
            None
        } else {
            Some(named.join(", "))
        },
        resolved_selectors: expanded,
        selector_match_counts,
    };

    if report.empty {
        return Err(ScopeError::EmptyScope {
            selectors: report.selectors.clone(),
            hint: empty_scope_hint(&report, snapshot),
            selector_match_counts: report.selector_match_counts.clone(),
        });
    }

    Ok(ResolvedScope { report, matched })
}

fn count_selector_matches(
    raw: &[String],
    selectors: &[Selector],
    snapshot: &Snapshot,
) -> Vec<SelectorMatchCount> {
    if selectors.len() <= 1 {
        return Vec::new();
    }
    raw.iter()
        .zip(selectors.iter())
        .map(|(selector, parsed)| SelectorMatchCount {
            selector: selector.clone(),
            matched_files: snapshot
                .files
                .iter()
                .filter(|file| selector_matches(parsed, &file.path, snapshot))
                .count(),
        })
        .collect()
}

fn expand_selector_value(
    value: &str,
    config: &ScopesConfig,
    out: &mut Vec<String>,
    named: &mut Vec<String>,
) -> Result<(), ScopeError> {
    if selector_kind(value).is_some() {
        out.push(value.to_string());
        return Ok(());
    }

    if let Some(scope) = config.scopes.get(value) {
        named.push(value.to_string());
        out.extend(scope.selectors.iter().cloned());
        return Ok(());
    }

    let available = config.available_names();
    let suggestion = nearest_scope(value, &available)
        .map(|name| format!("\nDid you mean '{name}'?"))
        .unwrap_or_default();
    Err(ScopeError::NamedScopeNotFound {
        name: value.to_string(),
        available: available.join(", "),
        suggestion,
    })
}

fn selector_kind(value: &str) -> Option<&str> {
    let (kind, _) = value.split_once(':')?;
    SELECTOR_KINDS.contains(&kind).then_some(kind)
}

fn parse_selector(value: &str, snapshot: &Snapshot) -> Result<Selector, ScopeError> {
    let Some((kind, body)) = value.split_once(':') else {
        unreachable!("named selectors are expanded before parsing")
    };
    match kind {
        "path" => parse_path_selector(body),
        "tag" => Ok(Selector::Tag(body.to_string())),
        "import" => Ok(Selector::Import(snapshot.normalize_path(body))),
        "reach" => parse_reach_selector(body, snapshot),
        _ => unreachable!("selector_kind filters invalid kinds"),
    }
}

fn parse_path_selector(body: &str) -> Result<Selector, ScopeError> {
    let normalized = normalize_selector_path(body);
    let matcher = if has_glob_meta(&normalized) {
        PathMatcher::Glob(
            GlobBuilder::new(&normalized)
                .literal_separator(true)
                .build()
                .map_err(|source| ScopeError::InvalidGlob {
                    pattern: normalized.clone(),
                    source,
                })?
                .compile_matcher(),
        )
    } else {
        PathMatcher::Prefix(normalized.clone())
    };
    Ok(Selector::Path { matcher })
}

fn parse_reach_selector(body: &str, snapshot: &Snapshot) -> Result<Selector, ScopeError> {
    let mut files = HashSet::new();
    if let Some(facts) = &snapshot.semantic_facts {
        for symbol_id in facts
            .reachability
            .reached_symbols
            .iter()
            .chain(facts.reachability.unreached_symbols.iter())
        {
            if symbol_matches(symbol_id, body)
                && let Some((file, _)) = symbol_id.split_once("::")
            {
                files.insert(file.to_string());
            }
        }
        for edge in &facts.dispatch_edges {
            if edge.handler_symbol == body || edge.handler_symbol.ends_with(&format!("::{body}")) {
                if let Some(handler_file) = &edge.handler_file {
                    files.insert(handler_file.clone());
                }
                files.insert(edge.from_file.clone());
            }
        }
    }
    for file in &snapshot.files {
        if file.exports.iter().any(|export| export.name == body) {
            files.insert(file.path.clone());
        }
    }
    if files.is_empty() {
        return Err(ScopeError::SymbolNotFound(body.to_string()));
    }
    Ok(Selector::Reach { files })
}

fn selector_matches(selector: &Selector, path: &str, snapshot: &Snapshot) -> bool {
    match selector {
        Selector::Path { matcher, .. } => matcher.matches(path),
        Selector::Tag(tag) => file_has_tag(snapshot, path, tag),
        Selector::Import(target) => imports_target(snapshot, path, target),
        Selector::Reach { files, .. } => files.contains(path),
    }
}

fn file_has_tag(snapshot: &Snapshot, path: &str, tag: &str) -> bool {
    let Some(facts) = &snapshot.semantic_facts else {
        return false;
    };
    facts.idiom_tags.iter().any(|(symbol_id, tags)| {
        symbol_id
            .split_once("::")
            .map(|(file, _)| file == path)
            .unwrap_or(false)
            && tags.iter().any(|t| t.name == tag)
    })
}

fn imports_target(snapshot: &Snapshot, path: &str, target: &str) -> bool {
    let target_stripped = strip_path_extension(target);
    snapshot.edges.iter().any(|edge| {
        edge.from == path
            && (edge.to == target || strip_path_extension(&edge.to) == target_stripped)
    }) || snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .is_some_and(|file| {
            file.imports.iter().any(|import| {
                import.resolved_path.as_deref() == Some(target)
                    || import.source == target
                    || strip_path_extension(&import.source) == target_stripped
            })
        })
}

fn nearest_scope<'a>(value: &str, available: &'a [String]) -> Option<&'a str> {
    available
        .iter()
        .map(|name| (name.as_str(), levenshtein(value, name)))
        .filter(|(_, distance)| *distance <= 3)
        .min_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)))
        .map(|(name, _)| name)
}

fn empty_scope_hint(report: &ScopeReport, snapshot: &Snapshot) -> String {
    for selector in &report.resolved_selectors {
        let Some(path) = selector.strip_prefix("path:") else {
            continue;
        };
        let normalized = normalize_selector_path(path);
        if has_glob_meta(&normalized) {
            if is_single_segment_glob(&normalized) {
                return format!(
                    "`*` does not cross `/` in path globs. Try `path:{normalized}/**` for recursive directory matches."
                );
            }
            return "Check the path glob against `loct tree`; glob `*` matches within one path segment only.".to_string();
        }
        let prefixes = top_level_path_prefixes(snapshot);
        if let Some(nearest) = nearest_path_prefix(&normalized, &prefixes) {
            return format!("Did you mean `path:{nearest}`?");
        }
    }
    "Run `loct tree` to inspect available path prefixes, then retry --scope with a concrete selector.".to_string()
}

fn is_single_segment_glob(path: &str) -> bool {
    has_glob_meta(path) && !path.contains('/')
}

fn top_level_path_prefixes(snapshot: &Snapshot) -> Vec<String> {
    let mut prefixes = HashSet::new();
    for file in &snapshot.files {
        let normalized = normalize_selector_path(&file.path);
        if let Some((top, _)) = normalized.split_once('/') {
            prefixes.insert(format!("{top}/"));
        } else {
            prefixes.insert(normalized);
        }
    }
    let mut prefixes: Vec<String> = prefixes.into_iter().collect();
    prefixes.sort();
    prefixes
}

fn nearest_path_prefix<'a>(value: &str, available: &'a [String]) -> Option<&'a str> {
    available
        .iter()
        .map(|name| (name.as_str(), levenshtein(value, name)))
        .min_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)))
        .map(|(name, _)| name)
}

fn fingerprint_selectors(selectors: &[String]) -> String {
    let mut hasher = Sha256::new();
    for selector in selectors {
        hasher.update(selector.as_bytes());
        hasher.update([0]);
    }
    hex_prefix(hasher.finalize().as_slice(), 12)
}

fn hex_prefix(bytes: &[u8], chars: usize) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        .chars()
        .take(chars)
        .collect()
}

fn normalize_selector_path(path: &str) -> String {
    path.trim_start_matches("./").replace('\\', "/")
}

fn has_glob_meta(path: &str) -> bool {
    path.contains('*') || path.contains('?') || path.contains('[')
}

fn symbol_matches(symbol_id: &str, raw: &str) -> bool {
    symbol_id == raw
        || symbol_id
            .split_once("::")
            .map(|(_, symbol)| symbol == raw)
            .unwrap_or(false)
}

fn strip_path_extension(path: &str) -> &str {
    match path.rfind('.') {
        Some(idx) => &path[..idx],
        None => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FileAnalysis;

    fn file(path: &str) -> FileAnalysis {
        FileAnalysis {
            path: path.to_string(),
            language: "rust".to_string(),
            ..Default::default()
        }
    }

    fn snapshot(paths: &[&str]) -> Snapshot {
        let mut snapshot = Snapshot::new(vec![".".to_string()]);
        snapshot.files = paths.iter().map(|path| file(path)).collect();
        snapshot
    }

    #[test]
    fn scope_path_literal_separator_does_not_cross_directory() {
        let snapshot = snapshot(&["src/lib.rs", "src/nested/lib.rs"]);
        let scope = resolve_scope_with_config(
            &["path:src/*.rs".to_string()],
            &ScopesConfig::default(),
            &snapshot,
        )
        .unwrap();
        assert_eq!(scope.matched_files(), vec!["src/lib.rs"]);
    }

    #[test]
    fn scope_path_prefix_filters_directory() {
        let snapshot = snapshot(&["src/lib.rs", "tests/lib.rs"]);
        let scope = resolve_scope_with_config(
            &["path:src/".to_string()],
            &ScopesConfig::default(),
            &snapshot,
        )
        .unwrap();
        assert_eq!(scope.matched_files(), vec!["src/lib.rs"]);
    }

    #[test]
    fn scope_reports_individual_counts_for_empty_intersection() {
        let snapshot = snapshot(&["editors/jetbrains/build.gradle.kts", "Makefile"]);
        let err = resolve_scope_with_config(
            &[
                "path:editors/jetbrains".to_string(),
                "path:Makefile".to_string(),
            ],
            &ScopesConfig::default(),
            &snapshot,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("--scope matched zero files"));
        assert!(err.contains("repeated --scope selectors are intersected (AND)"));
        assert!(err.contains("path:editors/jetbrains=1"));
        assert!(err.contains("path:Makefile=1"));
    }

    #[test]
    fn scope_named_resolution_from_config() {
        let snapshot = snapshot(&["src/lib.rs", "tests/lib.rs"]);
        let config = ScopesConfig {
            scopes: HashMap::from([(
                "core".to_string(),
                NamedScope {
                    description: None,
                    selectors: vec!["path:src/".to_string()],
                },
            )]),
        };
        let scope = resolve_scope_with_config(&["core".to_string()], &config, &snapshot).unwrap();
        assert_eq!(scope.report.named_resolved_from.as_deref(), Some("core"));
        assert_eq!(scope.matched_files(), vec!["src/lib.rs"]);
    }

    #[test]
    fn unknown_scope_without_named_scopes_teaches_explicit_selector_syntax() {
        let snapshot = snapshot(&["src/lib.rs"]);
        let err =
            resolve_scope_with_config(&["core".to_string()], &ScopesConfig::default(), &snapshot)
                .unwrap_err()
                .to_string();

        assert!(err.contains("Supported selector kinds: path:, tag:, import:, reach:"));
        assert!(err.contains("No named scopes are configured"));
        assert!(err.contains("`--scope path:core/`"));
        assert!(!err.contains("Available named scopes: []"));
    }

    #[test]
    fn unknown_scope_with_named_scopes_still_suggests_nearest_name() {
        let snapshot = snapshot(&["src/lib.rs"]);
        let config = ScopesConfig {
            scopes: HashMap::from([(
                "source".to_string(),
                NamedScope {
                    description: None,
                    selectors: vec!["path:src/".to_string()],
                },
            )]),
        };
        let err = resolve_scope_with_config(&["sorce".to_string()], &config, &snapshot)
            .unwrap_err()
            .to_string();

        assert!(err.contains("Supported selector kinds: path:, tag:, import:, reach:"));
        assert!(err.contains("Available named scopes: [source]"));
        assert!(err.contains("Did you mean 'source'?"));
    }

    #[test]
    fn scope_fingerprint_is_deterministic_and_order_sensitive() {
        let snapshot = snapshot(&["src/lib.rs"]);
        let a = resolve_scope_with_config(
            &["path:src/".to_string(), "path:src/lib.rs".to_string()],
            &ScopesConfig::default(),
            &snapshot,
        )
        .unwrap();
        let b = resolve_scope_with_config(
            &["path:src/".to_string(), "path:src/lib.rs".to_string()],
            &ScopesConfig::default(),
            &snapshot,
        )
        .unwrap();
        let c = resolve_scope_with_config(
            &["path:src/lib.rs".to_string(), "path:src/".to_string()],
            &ScopesConfig::default(),
            &snapshot,
        )
        .unwrap();
        assert_eq!(a.report.fingerprint, b.report.fingerprint);
        assert_ne!(a.report.fingerprint, c.report.fingerprint);
    }
}
