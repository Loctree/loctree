//! Bundle distribution analysis using source maps.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use super::dist_vlq::decode_vlq_value;
use super::root_scan::normalize_module_id;
use crate::snapshot::Snapshot;
use crate::types::{ExportSymbol, FileAnalysis, ImportKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMapping {
    pub gen_line: usize,
    pub gen_col: usize,
    pub source_idx: Option<usize>,
    pub source_line: Option<usize>,
    pub source_col: Option<usize>,
    pub name_idx: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct SourceMap {
    version: u8,
    sources: Vec<String>,
    #[serde(default)]
    names: Vec<String>,
    mappings: String,
    #[serde(default)]
    #[serde(rename = "sourceRoot")]
    source_root: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DistResult {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[serde(rename = "srcDir")]
    pub src_dir: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "sourceMapPaths")]
    pub source_map_paths: Vec<String>,
    #[serde(rename = "sourceMaps")]
    pub source_maps: usize,
    #[serde(rename = "sourceExports")]
    pub source_exports: usize,
    #[serde(rename = "bundledExports")]
    pub bundled_exports: usize,
    #[serde(rename = "deadExports")]
    pub dead_exports: Vec<DeadBundleExport>,
    pub reduction: String,
    #[serde(rename = "symbolLevel")]
    pub symbol_level: bool,
    #[serde(rename = "analysisLevel")]
    pub analysis_level: DistAnalysisLevel,
    #[serde(rename = "treeShakenExports")]
    pub tree_shaken_exports: usize,
    #[serde(rename = "treeShakenPct")]
    pub tree_shaken_pct: usize,
    #[serde(rename = "coveragePct")]
    pub coverage_pct: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "impactedFiles")]
    pub impacted_files: Vec<DistFileImpact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chunks: Vec<DistChunkSummary>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(rename = "candidateCounts")]
    pub candidate_counts: BTreeMap<String, usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<DistCandidate>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistFileImpact {
    pub file: String,
    #[serde(rename = "sourceExports")]
    pub source_exports: usize,
    #[serde(rename = "bundledExports")]
    pub bundled_exports: usize,
    #[serde(rename = "treeShakenExports")]
    pub tree_shaken_exports: usize,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadBundleExport {
    pub file: String,
    pub line: usize,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DistAnalysisLevel {
    #[default]
    File,
    Line,
    Symbol,
    Mixed,
}

impl DistAnalysisLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Line => "line",
            Self::Symbol => "symbol",
            Self::Mixed => "mixed",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistCandidateClass {
    DeadInAllChunks,
    BootPathOnly,
    FeatureLocal,
    FakeLazy,
    #[default]
    VerifyFirst,
}

impl DistCandidateClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeadInAllChunks => "dead_in_all_chunks",
            Self::BootPathOnly => "boot_path_only",
            Self::FeatureLocal => "feature_local",
            Self::FakeLazy => "fake_lazy",
            Self::VerifyFirst => "verify_first",
        }
    }

    fn priority(self) -> usize {
        match self {
            Self::DeadInAllChunks => 5,
            Self::FakeLazy => 4,
            Self::VerifyFirst => 3,
            Self::FeatureLocal => 2,
            Self::BootPathOnly => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DistConfidence {
    #[default]
    Low,
    Medium,
    High,
}

impl DistConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    fn weight(self) -> usize {
        match self {
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DistChunkRole {
    Boot,
    #[default]
    Feature,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DistChunkSummary {
    pub path: String,
    pub label: String,
    pub role: DistChunkRole,
    #[serde(rename = "roleConfidence")]
    pub role_confidence: DistConfidence,
    #[serde(rename = "analysisLevel")]
    pub analysis_level: DistAnalysisLevel,
    #[serde(rename = "matchedSources")]
    pub matched_sources: usize,
    #[serde(rename = "entrypointHits")]
    pub entrypoint_hits: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DistCandidate {
    pub file: String,
    pub line: usize,
    pub name: String,
    pub kind: String,
    #[serde(rename = "class")]
    pub class_name: DistCandidateClass,
    pub confidence: DistConfidence,
    pub rank: usize,
    #[serde(rename = "seenInChunks")]
    pub seen_in_chunks: usize,
    #[serde(rename = "bootChunks")]
    pub boot_chunks: usize,
    #[serde(rename = "featureChunks")]
    pub feature_chunks: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "chunkNames")]
    pub chunk_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "dynamicImporters")]
    pub dynamic_importers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "staticImporters")]
    pub static_importers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Default)]
struct FileIndex {
    paths: Vec<String>,
    exact: HashMap<String, Vec<String>>,
    norm_paths: HashMap<String, Vec<String>>,
    norm_keys: HashMap<String, Vec<String>>,
}

#[derive(Debug, Default)]
struct ImportEvidence {
    dynamic_importers: HashMap<String, Vec<String>>,
    static_importers: HashMap<String, Vec<String>>,
}

#[derive(Debug)]
struct ChunkCoverage {
    path: PathBuf,
    label: String,
    matched_files: HashSet<String>,
    file_line_hits: HashMap<String, HashSet<usize>>,
    file_symbol_hits: HashMap<String, HashSet<String>>,
    role: DistChunkRole,
    role_confidence: DistConfidence,
    analysis_level: DistAnalysisLevel,
    entrypoint_hits: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct ExportChunkPresence {
    maybe_present: bool,
    reliable_present: bool,
    reliable_absent: bool,
}

impl FileIndex {
    fn build(analyses: &[FileAnalysis]) -> Self {
        let mut index = Self::default();
        for analysis in analyses {
            let path = analysis.path.clone();
            let clean = canonicalize_match_path(&path);
            index.paths.push(path.clone());
            push_path_key(&mut index.exact, clean, &path);
            let normalized = normalize_module_id(&path);
            push_path_key(&mut index.norm_paths, normalized.path.clone(), &path);
            push_path_key(&mut index.norm_keys, normalized.as_key(), &path);
        }
        index.paths.sort();
        index.paths.dedup();
        index
    }

    fn lookup(&self, source: &str) -> Vec<String> {
        let clean = canonicalize_match_path(source);
        let alias = strip_alias_prefix(&clean).to_string();
        let normalized = normalize_module_id(&clean);
        let normalized_alias = normalize_module_id(&alias);
        let mut matches = Vec::new();

        collect_path_matches(&self.exact, &clean, &mut matches);
        if alias != clean {
            collect_path_matches(&self.exact, &alias, &mut matches);
        }
        collect_path_matches(&self.norm_paths, &normalized.path, &mut matches);
        collect_path_matches(&self.norm_keys, &normalized.as_key(), &mut matches);
        if alias != clean {
            collect_path_matches(&self.norm_paths, &normalized_alias.path, &mut matches);
            collect_path_matches(&self.norm_keys, &normalized_alias.as_key(), &mut matches);
        }

        if matches.is_empty() {
            for path in &self.paths {
                if paths_match(path, &clean) || (!alias.is_empty() && paths_match(path, &alias)) {
                    push_unique(&mut matches, path.clone());
                }
            }
        }

        matches.sort();
        matches.dedup();
        matches
    }
}

fn push_path_key(map: &mut HashMap<String, Vec<String>>, key: String, path: &str) {
    let entry = map.entry(key).or_default();
    if !entry.iter().any(|existing| existing == path) {
        entry.push(path.to_string());
        entry.sort();
    }
}

fn collect_path_matches(map: &HashMap<String, Vec<String>>, key: &str, output: &mut Vec<String>) {
    if let Some(paths) = map.get(key) {
        for path in paths {
            push_unique(output, path.clone());
        }
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn trim_query_fragment(path: &str) -> &str {
    path.split(['?', '#']).next().unwrap_or(path)
}

fn canonicalize_match_path(path: &str) -> String {
    let mut clean = trim_query_fragment(path).replace('\\', "/");
    for prefix in [
        "webpack:///",
        "webpack://",
        "vite:///",
        "vite://",
        "file://",
        "/@fs/",
    ] {
        if let Some(stripped) = clean.strip_prefix(prefix) {
            clean = stripped.to_string();
        }
    }
    while let Some(stripped) = clean.strip_prefix("./") {
        clean = stripped.to_string();
    }
    if cfg!(windows) {
        clean.to_lowercase()
    } else {
        clean
    }
}

fn normalize_source_path(source: &str, source_root: Option<&str>) -> String {
    let source_clean = canonicalize_match_path(source);
    if let Some(root) = source_root {
        let root_clean = canonicalize_match_path(root);
        if !root_clean.is_empty() {
            return format!(
                "{}/{}",
                root_clean.trim_end_matches('/'),
                source_clean.trim_start_matches("./")
            );
        }
    }
    source_clean
}

fn strip_alias_prefix(path: &str) -> &str {
    let without_at = path.trim_start_matches('@');
    if let Some(idx) = without_at.find('/') {
        &without_at[idx + 1..]
    } else {
        without_at
    }
}

fn paths_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }

    let a_clean = canonicalize_match_path(a);
    let b_clean = canonicalize_match_path(b);
    if a_clean == b_clean {
        return true;
    }

    let a_alias = strip_alias_prefix(&a_clean);
    let b_alias = strip_alias_prefix(&b_clean);
    if a_alias == b_clean || b_alias == a_clean || a_alias == b_alias {
        return true;
    }

    let mod_a = normalize_module_id(&a_clean);
    let mod_b = normalize_module_id(&b_clean);
    if mod_a.path == mod_b.path || mod_a.as_key() == mod_b.as_key() {
        return true;
    }

    if a_clean.len() > b_clean.len() {
        if let Some(idx) = a_clean.rfind(&b_clean)
            && (idx == 0 || a_clean.chars().nth(idx - 1) == Some('/'))
        {
            return true;
        }
    } else if b_clean.len() > a_clean.len()
        && let Some(idx) = b_clean.rfind(&a_clean)
        && (idx == 0 || b_clean.chars().nth(idx - 1) == Some('/'))
    {
        return true;
    }

    false
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn discover_source_maps(inputs: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut discovered = Vec::new();
    let mut seen = HashSet::new();

    for input in inputs {
        if input.is_file() {
            let key = path_to_string(&input.canonicalize().unwrap_or_else(|_| input.to_path_buf()));
            if seen.insert(key) {
                discovered.push(input.clone());
            }
            continue;
        }

        if input.is_dir() {
            let mut found_in_dir = 0usize;
            for entry in WalkDir::new(input)
                .follow_links(true)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
            {
                if entry.path().extension().and_then(|ext| ext.to_str()) != Some("map") {
                    continue;
                }
                let candidate = entry.path().to_path_buf();
                let key = path_to_string(
                    &candidate
                        .canonicalize()
                        .unwrap_or_else(|_| candidate.clone()),
                );
                if seen.insert(key) {
                    discovered.push(candidate);
                    found_in_dir += 1;
                }
            }
            if found_in_dir == 0 {
                return Err(format!(
                    "No source maps found in directory: {}",
                    input.display()
                ));
            }
            continue;
        }

        return Err(format!(
            "Source map path does not exist: {}",
            input.display()
        ));
    }

    discovered.sort_by_key(|path| path_to_string(path));
    if discovered.is_empty() {
        return Err("At least one source map is required".to_string());
    }
    Ok(discovered)
}

fn parse_mappings(mappings: &str) -> Vec<SourceMapping> {
    let mut result = Vec::new();
    let mut gen_line = 0usize;
    let mut source_idx = 0i32;
    let mut source_line = 0i32;
    let mut source_col = 0i32;
    let mut name_idx = 0i32;

    for line in mappings.split(';') {
        let mut gen_col = 0i32;
        if !line.is_empty() {
            for segment in line.split(',') {
                if segment.is_empty() {
                    continue;
                }
                let mut chars = segment.chars();
                if let Some(delta) = decode_vlq_value(&mut chars) {
                    gen_col += delta;
                    let src_idx = decode_vlq_value(&mut chars).map(|d| {
                        source_idx += d;
                        source_idx as usize
                    });
                    let src_line = decode_vlq_value(&mut chars).map(|d| {
                        source_line += d;
                        source_line as usize
                    });
                    let src_col = decode_vlq_value(&mut chars).map(|d| {
                        source_col += d;
                        source_col as usize
                    });
                    let nm_idx = decode_vlq_value(&mut chars).map(|d| {
                        name_idx += d;
                        name_idx as usize
                    });
                    result.push(SourceMapping {
                        gen_line,
                        gen_col: gen_col as usize,
                        source_idx: src_idx,
                        source_line: src_line,
                        source_col: src_col,
                        name_idx: nm_idx,
                    });
                }
            }
        }
        gen_line += 1;
    }
    result
}

fn parse_source_map(path: &Path) -> Result<SourceMap, String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("Failed to read source map: {}", e))?;
    let map: SourceMap = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse source map JSON: {}", e))?;
    if map.version != 3 {
        return Err(format!("Unsupported source map version: {}", map.version));
    }
    Ok(map)
}

fn normalize_root_for_scope_compare(root: &Path, snapshot_root: &Path) -> String {
    let candidate = if root.is_absolute() {
        root.to_path_buf()
    } else {
        snapshot_root.join(root)
    };
    candidate
        .canonicalize()
        .unwrap_or(candidate)
        .to_string_lossy()
        .replace('\\', "/")
}

fn snapshot_scope_matches_requested_root(
    snapshot: &Snapshot,
    requested_root: &Path,
    snapshot_root: &Path,
) -> bool {
    let requested = normalize_root_for_scope_compare(requested_root, snapshot_root);
    let mut snapshot_roots: Vec<String> = snapshot
        .metadata
        .roots
        .iter()
        .map(|root| normalize_root_for_scope_compare(Path::new(root), snapshot_root))
        .collect();
    snapshot_roots.sort();
    snapshot_roots.dedup();
    snapshot_roots == vec![requested]
}

pub fn load_or_scan_src(src_dir: &Path) -> Result<Snapshot, String> {
    if !src_dir.exists() {
        return Err(format!(
            "Source directory does not exist: {}",
            src_dir.display()
        ));
    }
    if !src_dir.is_dir() {
        return Err(format!(
            "Source path is not a directory: {}",
            src_dir.display()
        ));
    }

    let snapshot_root = crate::snapshot::resolve_snapshot_root_with_strategy(
        &[src_dir.to_path_buf()],
        crate::snapshot::SnapshotRootStrategy::Exact,
    );

    // Thin call into the snapshot freshness authority: exact-scope snapshot,
    // strict reuse (dist analysis needs content truth), full-tree rescan.
    let snapshot = crate::snapshot::acquire_snapshot(
        std::slice::from_ref(&snapshot_root),
        crate::snapshot::SnapshotReusePolicy::Strict,
        &crate::snapshot::AcquireOptions {
            quiet: true,
            full_scan: true,
            strategy: crate::snapshot::SnapshotRootStrategy::Exact,
            ..Default::default()
        },
    )
    .map_err(|e| format!("Failed to scan source directory: {}", e))?;

    if !snapshot_scope_matches_requested_root(&snapshot, &snapshot_root, &snapshot_root) {
        return Err(format!(
            "Snapshot scope mismatch after scan: expected exact root '{}'",
            snapshot_root.display()
        ));
    }
    Ok(snapshot)
}

pub fn calculate_stats(
    analyses: &[FileAnalysis],
    dead_exports: &[DeadBundleExport],
) -> (usize, usize, String) {
    let total_exports: usize = analyses
        .iter()
        .map(|analysis| {
            analysis
                .exports
                .iter()
                .filter(|export| is_runtime_dist_eligible(analysis, export))
                .count()
        })
        .sum();
    let bundled = total_exports.saturating_sub(dead_exports.len());
    let reduction_pct = if total_exports > 0 {
        (dead_exports.len() as f64 / total_exports as f64 * 100.0).round() as usize
    } else {
        0
    };
    (total_exports, bundled, format!("{}%", reduction_pct))
}

fn calculate_percent(total: usize, count: usize) -> usize {
    if total > 0 {
        (count as f64 / total as f64 * 100.0).round() as usize
    } else {
        0
    }
}

fn collect_entrypoint_paths(snapshot: &Snapshot) -> HashSet<String> {
    let mut paths = HashSet::new();
    for entrypoint in &snapshot.metadata.entrypoints {
        paths.insert(entrypoint.path.clone());
    }
    for analysis in &snapshot.files {
        if !analysis.entry_points.is_empty() {
            paths.insert(analysis.path.clone());
        }
    }
    paths
}

fn build_chunk_coverage(
    map_path: &Path,
    snapshot: &Snapshot,
    index: &FileIndex,
    entrypoints: &HashSet<String>,
) -> Result<ChunkCoverage, String> {
    let source_map = parse_source_map(map_path)?;
    let mappings = parse_mappings(&source_map.mappings);
    let normalized_sources: Vec<String> = source_map
        .sources
        .iter()
        .map(|source| normalize_source_path(source, source_map.source_root.as_deref()))
        .collect();

    let mut matched_files = HashSet::new();
    let mut source_matches = Vec::with_capacity(normalized_sources.len());
    for source in &normalized_sources {
        let matches = index.lookup(source);
        for path in &matches {
            matched_files.insert(path.clone());
        }
        source_matches.push(matches);
    }

    let mut file_line_hits: HashMap<String, HashSet<usize>> = HashMap::new();
    let mut file_symbol_hits: HashMap<String, HashSet<String>> = HashMap::new();
    for mapping in &mappings {
        let Some(source_idx) = mapping.source_idx else {
            continue;
        };
        let Some(matches) = source_matches.get(source_idx) else {
            continue;
        };
        for file in matches {
            if let Some(source_line) = mapping.source_line {
                file_line_hits
                    .entry(file.clone())
                    .or_default()
                    .insert(source_line + 1);
            }
            if let Some(name_idx) = mapping.name_idx
                && let Some(name) = source_map.names.get(name_idx)
            {
                file_symbol_hits
                    .entry(file.clone())
                    .or_default()
                    .insert(name.clone());
            }
        }
    }

    let entrypoint_hits = matched_files
        .iter()
        .filter(|path| entrypoints.contains(*path))
        .count();
    let analysis_level = if !file_symbol_hits.is_empty() {
        DistAnalysisLevel::Symbol
    } else if !file_line_hits.is_empty() {
        DistAnalysisLevel::Line
    } else {
        DistAnalysisLevel::File
    };
    let label = map_path
        .file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| path_to_string(map_path), ToString::to_string);

    let _ = snapshot;

    Ok(ChunkCoverage {
        path: map_path.to_path_buf(),
        label,
        matched_files,
        file_line_hits,
        file_symbol_hits,
        role: DistChunkRole::Feature,
        role_confidence: DistConfidence::Low,
        analysis_level,
        entrypoint_hits,
    })
}

fn assign_chunk_roles(chunks: &mut [ChunkCoverage]) {
    if chunks.is_empty() {
        return;
    }
    if chunks.len() == 1 {
        chunks[0].role = DistChunkRole::Boot;
        chunks[0].role_confidence = DistConfidence::High;
        return;
    }

    if chunks.iter().any(|chunk| chunk.entrypoint_hits > 0) {
        for chunk in chunks {
            if chunk.entrypoint_hits > 0 {
                chunk.role = DistChunkRole::Boot;
                chunk.role_confidence = DistConfidence::High;
            } else {
                chunk.role = DistChunkRole::Feature;
                chunk.role_confidence = DistConfidence::Medium;
            }
        }
        return;
    }

    let max_sources = chunks
        .iter()
        .map(|chunk| chunk.matched_files.len())
        .max()
        .unwrap_or(0);
    for chunk in chunks {
        if max_sources > 0 && chunk.matched_files.len() == max_sources {
            chunk.role = DistChunkRole::Boot;
            chunk.role_confidence = DistConfidence::Medium;
        } else {
            chunk.role = DistChunkRole::Feature;
            chunk.role_confidence = DistConfidence::Low;
        }
    }
}

fn add_importer(map: &mut HashMap<String, Vec<String>>, target: &str, importer: &str) {
    let entry = map.entry(target.to_string()).or_default();
    if !entry.iter().any(|existing| existing == importer) {
        entry.push(importer.to_string());
        entry.sort();
    }
}

fn resolve_import_targets(
    index: &FileIndex,
    resolved_path: Option<&str>,
    source: &str,
    source_raw: &str,
) -> Vec<String> {
    let mut targets = Vec::new();
    if let Some(resolved) = resolved_path {
        for target in index.lookup(resolved) {
            push_unique(&mut targets, target);
        }
    }
    if targets.is_empty() {
        for target in index.lookup(source) {
            push_unique(&mut targets, target);
        }
    }
    if targets.is_empty() {
        for target in index.lookup(source_raw) {
            push_unique(&mut targets, target);
        }
    }
    targets
}

fn build_import_evidence(analyses: &[FileAnalysis], index: &FileIndex) -> ImportEvidence {
    let mut evidence = ImportEvidence::default();

    for analysis in analyses {
        if is_test_like_analysis(analysis) {
            continue;
        }

        for import in &analysis.imports {
            let targets = resolve_import_targets(
                index,
                import.resolved_path.as_deref(),
                &import.source,
                &import.source_raw,
            );
            match import.kind {
                ImportKind::Dynamic => {
                    for target in targets {
                        add_importer(&mut evidence.dynamic_importers, &target, &analysis.path);
                    }
                }
                ImportKind::Static | ImportKind::SideEffect => {
                    for target in targets {
                        add_importer(&mut evidence.static_importers, &target, &analysis.path);
                    }
                }
                ImportKind::Type => {}
            }
        }

        for dynamic_import in &analysis.dynamic_imports {
            for target in index.lookup(dynamic_import) {
                add_importer(&mut evidence.dynamic_importers, &target, &analysis.path);
            }
        }
    }

    evidence
}

fn is_test_like_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_lowercase();
    normalized.contains("/__tests__/")
        || normalized.contains("/tests/")
        || normalized.contains("/test-utils/")
        || normalized.contains("/__mocks__/")
        || normalized.ends_with(".test.ts")
        || normalized.ends_with(".test.tsx")
        || normalized.ends_with(".test.js")
        || normalized.ends_with(".test.jsx")
        || normalized.ends_with(".spec.ts")
        || normalized.ends_with(".spec.tsx")
        || normalized.ends_with(".spec.js")
        || normalized.ends_with(".spec.jsx")
}

fn is_test_like_analysis(analysis: &FileAnalysis) -> bool {
    analysis.is_test || is_test_like_path(&analysis.path)
}

fn is_probable_barrel_file(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let file_name = normalized.rsplit('/').next().unwrap_or(&normalized);
    matches!(
        file_name,
        "index.ts" | "index.tsx" | "index.js" | "index.jsx" | "index.mjs" | "index.cjs"
    )
}

fn is_barrel_reexport(analysis: &FileAnalysis, export: &ExportSymbol) -> bool {
    export.kind == "reexport" && is_probable_barrel_file(&analysis.path)
}

fn is_runtime_dist_eligible(analysis: &FileAnalysis, export: &ExportSymbol) -> bool {
    !is_test_like_analysis(analysis)
        && !matches!(export.kind.as_str(), "type" | "interface")
        && !is_barrel_reexport(analysis, export)
}

fn has_nearby_line(lines: &HashSet<usize>, line: usize) -> bool {
    let start = line.saturating_sub(1);
    let end = line.saturating_add(1);
    (start..=end).any(|candidate| lines.contains(&candidate))
}

fn is_probable_entrypoint_file(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let file_name = normalized.rsplit('/').next().unwrap_or(&normalized);
    matches!(
        file_name,
        "index.ts"
            | "index.tsx"
            | "index.js"
            | "index.jsx"
            | "main.ts"
            | "main.tsx"
            | "main.js"
            | "main.jsx"
            | "app.ts"
            | "app.tsx"
            | "App.tsx"
    )
}

fn evaluate_export_presence(
    chunk: &ChunkCoverage,
    file: &str,
    export_name: &str,
    export_line: Option<usize>,
) -> ExportChunkPresence {
    if !chunk.matched_files.contains(file) {
        return ExportChunkPresence {
            maybe_present: false,
            reliable_present: false,
            reliable_absent: true,
        };
    }

    let line_hit = export_line.is_some_and(|line| {
        chunk
            .file_line_hits
            .get(file)
            .is_some_and(|lines| has_nearby_line(lines, line))
    });
    let symbol_hit = chunk
        .file_symbol_hits
        .get(file)
        .is_some_and(|symbols| symbols.contains(export_name));
    let reliable_present = line_hit || symbol_hit;
    let export_level_hits =
        chunk.file_line_hits.contains_key(file) || chunk.file_symbol_hits.contains_key(file);
    let reliable_absent = !reliable_present && export_level_hits && export_line.is_some();

    ExportChunkPresence {
        maybe_present: !reliable_absent,
        reliable_present,
        reliable_absent,
    }
}

fn determine_analysis_level(chunks: &[ChunkCoverage]) -> DistAnalysisLevel {
    let has_file = chunks
        .iter()
        .any(|chunk| matches!(chunk.analysis_level, DistAnalysisLevel::File));
    let has_line = chunks
        .iter()
        .any(|chunk| matches!(chunk.analysis_level, DistAnalysisLevel::Line));
    let has_symbol = chunks
        .iter()
        .any(|chunk| matches!(chunk.analysis_level, DistAnalysisLevel::Symbol));

    match (has_symbol, has_line, has_file) {
        (true, false, false) => DistAnalysisLevel::Symbol,
        (false, true, false) => DistAnalysisLevel::Line,
        (false, false, true) => DistAnalysisLevel::File,
        _ => DistAnalysisLevel::Mixed,
    }
}

fn summarize_chunks(chunks: &[ChunkCoverage]) -> Vec<DistChunkSummary> {
    chunks
        .iter()
        .map(|chunk| DistChunkSummary {
            path: path_to_string(&chunk.path),
            label: chunk.label.clone(),
            role: chunk.role,
            role_confidence: chunk.role_confidence,
            analysis_level: chunk.analysis_level,
            matched_sources: chunk.matched_files.len(),
            entrypoint_hits: chunk.entrypoint_hits,
        })
        .collect()
}

fn build_dead_bundle_export(candidate: &DistCandidate) -> DeadBundleExport {
    DeadBundleExport {
        file: candidate.file.clone(),
        line: candidate.line,
        name: candidate.name.clone(),
        kind: candidate.kind.clone(),
    }
}

fn classify_export_candidate(
    analysis: &FileAnalysis,
    export: &crate::types::ExportSymbol,
    chunks: &[ChunkCoverage],
    import_evidence: &ImportEvidence,
) -> Option<DistCandidate> {
    if !is_runtime_dist_eligible(analysis, export) {
        return None;
    }

    let mut seen_chunks = Vec::new();
    let mut boot_chunks = Vec::new();
    let mut feature_chunks = Vec::new();
    let mut reliable_present = 0usize;
    let mut reliable_absent = 0usize;
    let mut ambiguous_hits = 0usize;
    let mut low_role_signal = false;

    for chunk in chunks {
        let presence = evaluate_export_presence(chunk, &analysis.path, &export.name, export.line);
        if presence.reliable_present {
            reliable_present += 1;
        }
        if presence.reliable_absent {
            reliable_absent += 1;
        }
        if presence.maybe_present {
            seen_chunks.push(chunk.label.clone());
            if !presence.reliable_present {
                ambiguous_hits += 1;
            }
            if chunk.role_confidence == DistConfidence::Low {
                low_role_signal = true;
            }
            match chunk.role {
                DistChunkRole::Boot => boot_chunks.push(chunk.label.clone()),
                DistChunkRole::Feature => feature_chunks.push(chunk.label.clone()),
            }
        }
    }

    let dynamic_importers = import_evidence
        .dynamic_importers
        .get(&analysis.path)
        .cloned()
        .unwrap_or_default();
    let static_importers = import_evidence
        .static_importers
        .get(&analysis.path)
        .cloned()
        .unwrap_or_default();

    let is_dynamic_target = !dynamic_importers.is_empty();
    let has_static_runtime = !static_importers.is_empty();
    let is_entrypoint_file =
        !analysis.entry_points.is_empty() || is_probable_entrypoint_file(&analysis.path);
    let seen_count = seen_chunks.len();
    let boot_count = boot_chunks.len();
    let feature_count = feature_chunks.len();
    let multi_chunk = chunks.len() > 1;

    let (class_name, confidence, notes) = if seen_count == 0 {
        let note = if reliable_absent == chunks.len()
            && chunks
                .iter()
                .any(|chunk| chunk.matched_files.contains(&analysis.path))
        {
            "source file ships, but this export never shows up in the analyzed chunks".to_string()
        } else {
            "absent from every analyzed chunk".to_string()
        };
        (
            DistCandidateClass::DeadInAllChunks,
            DistConfidence::High,
            vec![note],
        )
    } else if multi_chunk
        && is_dynamic_target
        && boot_count > 0
        && (has_static_runtime || feature_count > 0)
    {
        let mut notes = vec![format!(
            "dynamic target still appears in boot chunk(s): {}",
            boot_chunks.join(", ")
        )];
        if has_static_runtime {
            notes.push(format!(
                "also statically imported by {}",
                static_importers.join(", ")
            ));
        }
        (
            DistCandidateClass::FakeLazy,
            if has_static_runtime {
                DistConfidence::High
            } else {
                DistConfidence::Medium
            },
            notes,
        )
    } else if multi_chunk && feature_count > 0 && boot_count == 0 {
        (
            DistCandidateClass::FeatureLocal,
            if reliable_present > 0 && !low_role_signal && ambiguous_hits == 0 {
                DistConfidence::High
            } else {
                DistConfidence::Medium
            },
            vec![format!(
                "seen only in feature chunk(s): {}",
                feature_chunks.join(", ")
            )],
        )
    } else if multi_chunk && boot_count > 0 && feature_count == 0 {
        if is_entrypoint_file {
            return None;
        }
        (
            DistCandidateClass::BootPathOnly,
            if reliable_present > 0 && !low_role_signal && ambiguous_hits == 0 {
                DistConfidence::High
            } else if ambiguous_hits > 0 {
                DistConfidence::Low
            } else {
                DistConfidence::Medium
            },
            vec![format!(
                "seen only in boot chunk(s): {}",
                boot_chunks.join(", ")
            )],
        )
    } else if ambiguous_hits > 0 && reliable_present == 0 {
        if is_entrypoint_file {
            return None;
        }
        let mut notes =
            vec!["bundle evidence is partial; verify before deleting or rewriting".to_string()];
        if is_dynamic_target {
            notes.push("module is dynamically imported somewhere in source".to_string());
        }
        (DistCandidateClass::VerifyFirst, DistConfidence::Low, notes)
    } else {
        return None;
    };

    Some(DistCandidate {
        file: analysis.path.clone(),
        line: export.line.unwrap_or(0),
        name: export.name.clone(),
        kind: export.kind.clone(),
        class_name,
        confidence,
        rank: class_name.priority() * 10 + confidence.weight(),
        seen_in_chunks: seen_count,
        boot_chunks: boot_count,
        feature_chunks: feature_count,
        chunk_names: seen_chunks,
        dynamic_importers,
        static_importers,
        notes,
    })
}

fn summarize_impacted_files(
    analyses: &[FileAnalysis],
    dead_exports: &[DeadBundleExport],
) -> Vec<DistFileImpact> {
    let total_by_file: HashMap<String, usize> = analyses
        .iter()
        .filter_map(|analysis| {
            let export_count = analysis
                .exports
                .iter()
                .filter(|export| is_runtime_dist_eligible(analysis, export))
                .count();
            (export_count > 0).then_some((analysis.path.clone(), export_count))
        })
        .collect();

    let mut dead_by_file: HashMap<String, usize> = HashMap::new();
    for dead in dead_exports {
        *dead_by_file.entry(dead.file.clone()).or_default() += 1;
    }

    let mut impacted: Vec<DistFileImpact> = dead_by_file
        .into_iter()
        .map(|(file, tree_shaken_exports)| {
            let source_exports = total_by_file
                .get(&file)
                .copied()
                .unwrap_or(tree_shaken_exports);
            let bundled_exports = source_exports.saturating_sub(tree_shaken_exports);
            let status = if bundled_exports == 0 {
                "fully-shaken"
            } else {
                "partially-shaken"
            };

            DistFileImpact {
                file,
                source_exports,
                bundled_exports,
                tree_shaken_exports,
                status: status.to_string(),
            }
        })
        .collect();

    impacted.sort_by(|a, b| {
        b.tree_shaken_exports
            .cmp(&a.tree_shaken_exports)
            .then(a.file.cmp(&b.file))
    });
    impacted
}

pub fn analyze_distribution_with_snapshot(
    source_map_paths: &[PathBuf],
    src_dir: &Path,
) -> Result<(DistResult, Snapshot), String> {
    if source_map_paths.is_empty() {
        return Err("At least one source map is required".to_string());
    }

    let snapshot = load_or_scan_src(src_dir)?;
    let discovered_maps = discover_source_maps(source_map_paths)?;
    let index = FileIndex::build(&snapshot.files);
    let entrypoints = collect_entrypoint_paths(&snapshot);
    let mut chunks = Vec::new();

    for source_map_path in &discovered_maps {
        chunks.push(build_chunk_coverage(
            source_map_path,
            &snapshot,
            &index,
            &entrypoints,
        )?);
    }

    if chunks.iter().all(|chunk| chunk.matched_files.is_empty()) {
        return Err(format!(
            "No source map sources matched files under {}",
            src_dir.display()
        ));
    }

    assign_chunk_roles(&mut chunks);
    let import_evidence = build_import_evidence(&snapshot.files, &index);
    let analysis_level = determine_analysis_level(&chunks);
    let mut candidates = Vec::new();

    for analysis in &snapshot.files {
        for export in &analysis.exports {
            if let Some(candidate) =
                classify_export_candidate(analysis, export, &chunks, &import_evidence)
            {
                candidates.push(candidate);
            }
        }
    }

    candidates.sort_by(|left, right| {
        right
            .rank
            .cmp(&left.rank)
            .then_with(|| right.confidence.cmp(&left.confidence))
            .then_with(|| left.file.cmp(&right.file))
            .then_with(|| left.name.cmp(&right.name))
    });

    let dead_exports: Vec<DeadBundleExport> = candidates
        .iter()
        .filter(|candidate| matches!(candidate.class_name, DistCandidateClass::DeadInAllChunks))
        .map(build_dead_bundle_export)
        .collect();

    let (source_exports, bundled_exports, reduction) =
        calculate_stats(&snapshot.files, &dead_exports);
    let tree_shaken_exports = dead_exports.len();
    let tree_shaken_pct = calculate_percent(source_exports, tree_shaken_exports);
    let coverage_pct = calculate_percent(source_exports, bundled_exports);
    let impacted_files = summarize_impacted_files(&snapshot.files, &dead_exports);
    let mut candidate_counts = BTreeMap::new();
    for candidate in &candidates {
        *candidate_counts
            .entry(candidate.class_name.as_str().to_string())
            .or_insert(0) += 1;
    }

    Ok((
        DistResult {
            src_dir: src_dir.display().to_string(),
            source_map_paths: discovered_maps
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            source_maps: discovered_maps.len(),
            source_exports,
            bundled_exports,
            dead_exports,
            reduction,
            symbol_level: matches!(analysis_level, DistAnalysisLevel::Symbol),
            analysis_level,
            tree_shaken_exports,
            tree_shaken_pct,
            coverage_pct,
            impacted_files,
            chunks: summarize_chunks(&chunks),
            candidate_counts,
            candidates,
        },
        snapshot,
    ))
}

pub fn analyze_distribution(
    source_map_paths: &[PathBuf],
    src_dir: &Path,
) -> Result<DistResult, String> {
    analyze_distribution_with_snapshot(source_map_paths, src_dir).map(|(result, _)| result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ExportSymbol;
    use serial_test::serial;
    use std::ffi::OsString;
    use std::process::Command;
    use tempfile::TempDir;

    const CACHE_ENV: &str = "LOCT_CACHE_DIR";

    #[derive(Debug)]
    struct EnvVarGuard {
        key: &'static str,
        original: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set_path(key: &'static str, value: &Path) -> Self {
            let guard = Self {
                key,
                original: std::env::var_os(key),
            };
            set_env_var(key, value.as_os_str());
            guard
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => set_env_var(self.key, value),
                None => remove_env_var(self.key),
            }
        }
    }

    fn set_env_var<K: AsRef<std::ffi::OsStr>, V: AsRef<std::ffi::OsStr>>(key: K, value: V) {
        unsafe {
            std::env::set_var(key, value);
        }
    }

    fn remove_env_var<K: AsRef<std::ffi::OsStr>>(key: K) {
        unsafe {
            std::env::remove_var(key);
        }
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap_or_else(|e| panic!("failed to run git {:?}: {e}", args));
        assert!(
            output.status.success(),
            "git {:?} failed.\nstdout: {}\nstderr: {}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn test_vlq_decode_simple() {
        assert_eq!(decode_vlq_value(&mut "A".chars()), Some(0));
        assert_eq!(decode_vlq_value(&mut "C".chars()), Some(1));
        assert_eq!(decode_vlq_value(&mut "D".chars()), Some(-1));
    }

    #[test]
    fn test_parse_mappings_simple() {
        let mappings = parse_mappings("AAAA");
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].gen_col, 0);
        assert_eq!(mappings[0].source_idx, Some(0));
    }

    #[test]
    fn test_parse_mappings_with_name() {
        let mappings = parse_mappings("AAAAA");
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].name_idx, Some(0));
    }

    #[test]
    fn test_parse_mappings_multiple_lines() {
        let mappings = parse_mappings("AAAA;AAAA");
        assert_eq!(mappings.len(), 2);
        assert_eq!(mappings[0].gen_line, 0);
        assert_eq!(mappings[1].gen_line, 1);
    }

    #[test]
    fn test_parse_source_map_basic() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let map_path = temp_dir.path().join("test.js.map");
        fs::write(
            &map_path,
            r#"{"version":3,"sources":["src/index.ts"],"names":["foo"],"mappings":"AAAA"}"#,
        )
        .expect("write source map fixture");
        let map = parse_source_map(&map_path).expect("parse source map");
        assert_eq!(map.version, 3);
        assert_eq!(map.sources.len(), 1);
    }

    #[test]
    fn test_calculate_stats() {
        let analyses = vec![FileAnalysis {
            path: "src/a.ts".to_string(),
            exports: vec![
                ExportSymbol {
                    name: "foo".to_string(),
                    kind: "function".to_string(),
                    export_type: "named".to_string(),
                    line: Some(10),
                    params: Vec::new(),

                    symbol_id: crate::types::SymbolIdV1::default(),
                },
                ExportSymbol {
                    name: "bar".to_string(),
                    kind: "const".to_string(),
                    export_type: "named".to_string(),
                    line: Some(20),
                    params: Vec::new(),

                    symbol_id: crate::types::SymbolIdV1::default(),
                },
            ],
            ..Default::default()
        }];
        let dead = vec![DeadBundleExport {
            file: "src/a.ts".to_string(),
            line: 20,
            name: "bar".to_string(),
            kind: "const".to_string(),
        }];
        let (total, bundled, reduction) = calculate_stats(&analyses, &dead);
        assert_eq!(total, 2);
        assert_eq!(bundled, 1);
        assert_eq!(reduction, "50%");
    }

    #[test]
    fn test_calculate_stats_ignores_type_only_exports() {
        let analyses = vec![FileAnalysis {
            path: "src/a.ts".to_string(),
            exports: vec![
                ExportSymbol {
                    name: "liveValue".to_string(),
                    kind: "const".to_string(),
                    export_type: "named".to_string(),
                    line: Some(10),
                    params: Vec::new(),

                    symbol_id: crate::types::SymbolIdV1::default(),
                },
                ExportSymbol {
                    name: "DeadType".to_string(),
                    kind: "type".to_string(),
                    export_type: "named".to_string(),
                    line: Some(20),
                    params: Vec::new(),

                    symbol_id: crate::types::SymbolIdV1::default(),
                },
                ExportSymbol {
                    name: "DeadShape".to_string(),
                    kind: "interface".to_string(),
                    export_type: "named".to_string(),
                    line: Some(30),
                    params: Vec::new(),

                    symbol_id: crate::types::SymbolIdV1::default(),
                },
            ],
            ..Default::default()
        }];
        let dead = vec![DeadBundleExport {
            file: "src/a.ts".to_string(),
            line: 10,
            name: "liveValue".to_string(),
            kind: "const".to_string(),
        }];
        let (total, bundled, reduction) = calculate_stats(&analyses, &dead);
        assert_eq!(total, 1);
        assert_eq!(bundled, 0);
        assert_eq!(reduction, "100%");
    }

    #[test]
    fn test_summarize_impacted_files() {
        let analyses = vec![
            FileAnalysis {
                path: "src/a.ts".to_string(),
                exports: vec![
                    ExportSymbol {
                        name: "foo".to_string(),
                        kind: "function".to_string(),
                        export_type: "named".to_string(),
                        line: Some(10),
                        params: Vec::new(),

                        symbol_id: crate::types::SymbolIdV1::default(),
                    },
                    ExportSymbol {
                        name: "bar".to_string(),
                        kind: "const".to_string(),
                        export_type: "named".to_string(),
                        line: Some(20),
                        params: Vec::new(),

                        symbol_id: crate::types::SymbolIdV1::default(),
                    },
                ],
                ..Default::default()
            },
            FileAnalysis {
                path: "src/b.ts".to_string(),
                exports: vec![ExportSymbol {
                    name: "baz".to_string(),
                    kind: "function".to_string(),
                    export_type: "named".to_string(),
                    line: Some(5),
                    params: Vec::new(),

                    symbol_id: crate::types::SymbolIdV1::default(),
                }],
                ..Default::default()
            },
        ];
        let dead = vec![
            DeadBundleExport {
                file: "src/a.ts".to_string(),
                line: 20,
                name: "bar".to_string(),
                kind: "const".to_string(),
            },
            DeadBundleExport {
                file: "src/b.ts".to_string(),
                line: 5,
                name: "baz".to_string(),
                kind: "function".to_string(),
            },
        ];

        let impacted = summarize_impacted_files(&analyses, &dead);
        assert_eq!(impacted.len(), 2);
        assert_eq!(impacted[0].file, "src/a.ts");
        assert_eq!(impacted[0].status, "partially-shaken");
        assert_eq!(impacted[1].file, "src/b.ts");
        assert_eq!(impacted[1].status, "fully-shaken");
    }

    #[test]
    fn dist_classify_export_candidate_skips_type_exports() {
        let analysis = FileAnalysis {
            path: "src/types.ts".to_string(),
            exports: vec![ExportSymbol {
                name: "WidgetConfig".to_string(),
                kind: "type".to_string(),
                export_type: "named".to_string(),
                line: Some(8),
                params: Vec::new(),

                symbol_id: crate::types::SymbolIdV1::default(),
            }],
            ..Default::default()
        };

        let candidate = classify_export_candidate(
            &analysis,
            &analysis.exports[0],
            &[],
            &ImportEvidence::default(),
        );

        assert!(candidate.is_none());
    }

    #[test]
    fn dist_classify_export_candidate_skips_test_file_exports() {
        let analysis = FileAnalysis {
            path: "src/__tests__/helper.test.ts".to_string(),
            is_test: true,
            exports: vec![ExportSymbol {
                name: "submitLoginAndExpectError".to_string(),
                kind: "function".to_string(),
                export_type: "named".to_string(),
                line: Some(12),
                params: Vec::new(),

                symbol_id: crate::types::SymbolIdV1::default(),
            }],
            ..Default::default()
        };

        let candidate = classify_export_candidate(
            &analysis,
            &analysis.exports[0],
            &[],
            &ImportEvidence::default(),
        );

        assert!(candidate.is_none());
    }

    #[test]
    fn dist_classify_export_candidate_skips_barrel_reexports() {
        let analysis = FileAnalysis {
            path: "src/components/auth/index.ts".to_string(),
            exports: vec![ExportSymbol {
                name: "AdminOnly".to_string(),
                kind: "reexport".to_string(),
                export_type: "named".to_string(),
                line: Some(4),
                params: Vec::new(),

                symbol_id: crate::types::SymbolIdV1::default(),
            }],
            ..Default::default()
        };

        let candidate = classify_export_candidate(
            &analysis,
            &analysis.exports[0],
            &[],
            &ImportEvidence::default(),
        );

        assert!(candidate.is_none());
    }

    #[test]
    fn dist_build_import_evidence_skips_test_importers() {
        let target = FileAnalysis {
            path: "src/target.ts".to_string(),
            ..Default::default()
        };

        let mut app_import =
            crate::types::ImportEntry::new("./target".to_string(), ImportKind::Static);
        app_import.resolved_path = Some("src/target.ts".to_string());
        let app = FileAnalysis {
            path: "src/app.ts".to_string(),
            imports: vec![app_import],
            ..Default::default()
        };

        let mut test_import =
            crate::types::ImportEntry::new("./target".to_string(), ImportKind::Static);
        test_import.resolved_path = Some("src/target.ts".to_string());
        let test_file = FileAnalysis {
            path: "src/__tests__/target.test.ts".to_string(),
            is_test: true,
            imports: vec![test_import],
            ..Default::default()
        };

        let analyses = vec![target, app, test_file];
        let index = FileIndex::build(&analyses);
        let evidence = build_import_evidence(&analyses, &index);

        assert_eq!(
            evidence.static_importers.get("src/target.ts"),
            Some(&vec!["src/app.ts".to_string()])
        );
    }

    #[test]
    fn dist_build_import_evidence_skips_test_utility_importers() {
        let target = FileAnalysis {
            path: "src/target.ts".to_string(),
            ..Default::default()
        };

        let mut test_utils_import =
            crate::types::ImportEntry::new("./target".to_string(), ImportKind::Static);
        test_utils_import.resolved_path = Some("src/target.ts".to_string());
        let test_utils = FileAnalysis {
            path: "src/test-utils/auth.tsx".to_string(),
            imports: vec![test_utils_import],
            ..Default::default()
        };

        let analyses = vec![target, test_utils];
        let index = FileIndex::build(&analyses);
        let evidence = build_import_evidence(&analyses, &index);

        assert!(!evidence.static_importers.contains_key("src/target.ts"));
    }

    #[test]
    #[serial]
    fn dist_load_or_scan_src_uses_exact_scope_snapshot() {
        let temp_dir = TempDir::new().expect("create repo temp dir");
        let cache_dir = TempDir::new().expect("create cache temp dir");
        let _cache_guard = EnvVarGuard::set_path(CACHE_ENV, cache_dir.path());

        let repo = temp_dir.path();
        let src_dir = repo.join("src");
        fs::create_dir_all(&src_dir).expect("create source dir");
        fs::write(repo.join("package.json"), "{\"name\":\"dist-test\"}")
            .expect("write package.json");
        fs::write(src_dir.join("mod.ts"), "export const value = 1;\n")
            .expect("write module fixture");

        run_git(repo, &["init"]);
        run_git(repo, &["config", "user.name", "Test User"]);
        run_git(repo, &["config", "user.email", "test@example.com"]);
        run_git(repo, &["add", "."]);
        run_git(repo, &["commit", "-m", "init"]);

        let snapshot = load_or_scan_src(&src_dir).expect("exact scope snapshot");
        let actual: Vec<_> = snapshot
            .metadata
            .roots
            .iter()
            .map(|root| {
                PathBuf::from(root)
                    .canonicalize()
                    .unwrap_or_else(|_| PathBuf::from(root))
            })
            .collect();
        let expected = vec![src_dir.canonicalize().expect("canonicalize src dir")];
        assert_eq!(actual, expected);
    }
}
