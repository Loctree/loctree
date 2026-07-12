//! VS2 Holographic Slice - Extract context for AI agents
//!
//! The slicer extracts a 3-layer context for a target file:
//! - Core: The target file itself (full source code)
//! - Deps: Files imported by target (signatures only by default)
//! - Consumers: Files that import target (shown by default; hide with --no-consumers)
//!
//! This implements the "scan once, slice many" philosophy for AI-oriented analysis.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, IsTerminal, Write as IoWrite};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::args::ParsedArgs;
use crate::snapshot::Snapshot;
use crate::types::FileAnalysis;

fn assemble_slice_target_path(input: &str) -> io::Result<PathBuf> {
    if input.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "slice target path is empty",
        ));
    }
    if input.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "slice target path contains NUL byte",
        ));
    }

    let mut out = PathBuf::new();
    let mut saw_any = false;
    for raw in input.split(['/', std::path::MAIN_SEPARATOR]) {
        if raw.is_empty() {
            if !saw_any {
                out.push(std::path::MAIN_SEPARATOR_STR);
                saw_any = true;
            }
            continue;
        }
        if raw == "." {
            continue;
        }
        if raw == ".." {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "slice target path contains '..' component",
            ));
        }
        out.push(raw);
        saw_any = true;
    }
    if !saw_any {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "slice target path is empty after sanitization",
        ));
    }
    Ok(out)
}

/// Configuration for slice operation
pub struct SliceConfig {
    /// Include consumer layer (files that import target)
    pub include_consumers: bool,
    /// Maximum depth for dependency traversal (default: 2)
    pub max_depth: usize,
}

impl Default for SliceConfig {
    fn default() -> Self {
        Self {
            include_consumers: true,
            max_depth: 2,
        }
    }
}

/// A file in the slice with its layer info
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SliceFile {
    /// File path relative to project root
    pub path: String,
    /// Layer: core, deps, or consumers
    pub layer: String,
    /// Lines of code
    pub loc: usize,
    /// Language (rust, typescript, etc.)
    pub language: String,
    /// File kind (code, test, config, doc, workflow, locale, resource, etc.)
    #[serde(default)]
    pub kind: String,
    /// Non-code resource membership, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_kind: Option<String>,
    /// Depth from target (0 = core, 1 = direct dep, etc.)
    pub depth: usize,
    /// True when this file is normally excluded by `.loctignore` and is only
    /// visible because the read was run with `--include-ignored`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignored: bool,
}

/// Symbol defined in the core layer of a slice/focus response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoreSymbol {
    /// Symbol name as indexed by Loctree.
    pub name: String,
    /// Symbol kind (function, struct, class, const, etc.).
    pub kind: String,
    /// Defining file, repo-relative.
    pub file: String,
    /// 1-based line number when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// True for exported symbols.
    #[serde(default)]
    pub exported: bool,
    /// Data provenance for this symbol row.
    pub authority: String,
}

/// Concrete follow-up command suggested by a slice/focus response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SuggestedNext {
    pub command: String,
    pub reason: String,
}

impl SliceFile {
    pub(crate) fn from_analysis(
        file: &crate::types::FileAnalysis,
        layer: &str,
        depth: usize,
    ) -> Self {
        Self {
            path: file.path.clone(),
            layer: layer.to_string(),
            loc: file.loc,
            language: file.language.clone(),
            kind: file.kind.clone(),
            resource_kind: file.resource_kind.clone(),
            depth,
            ignored: file.ignored,
        }
    }

    pub(crate) fn descriptor(&self) -> String {
        match &self.resource_kind {
            Some(resource) => format!("{}, {}", self.language, resource),
            None if !self.kind.is_empty() && self.kind != "code" => {
                format!("{}, {}", self.language, self.kind)
            }
            _ => self.language.clone(),
        }
    }

    /// Human-output suffix marking a file that is normally hidden by
    /// `.loctignore` and only surfaced via `--include-ignored`.
    pub(crate) fn ignored_tag(&self) -> &'static str {
        if self.ignored {
            " [ignored: .loctignore]"
        } else {
            ""
        }
    }
}

/// The complete slice result
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HolographicSlice {
    /// Target file that was sliced
    pub target: String,
    /// Core layer files (the target itself)
    pub core: Vec<SliceFile>,
    /// Dependencies layer files
    pub deps: Vec<SliceFile>,
    /// Consumer layer files (who imports target)
    pub consumers: Vec<SliceFile>,
    /// Files that use symbols defined in the target without importing it —
    /// the symbol-graph consumer layer (Wave C-1). Catches the Swift
    /// intra-module case where the import graph is structurally silent.
    /// Heuristic confidence: name+scope resolution, not compiler truth.
    #[serde(default)]
    pub symbol_consumers: Vec<SymbolConsumer>,
    /// Symbols defined in the core file, with file:line provenance.
    #[serde(default)]
    pub core_symbols: Vec<CoreSymbol>,
    /// Provenance labels represented in this response.
    #[serde(default)]
    pub authority_labels: Vec<String>,
    /// Concrete next Loctree commands an agent can run from this slice.
    #[serde(default)]
    pub suggested_next: Vec<SuggestedNext>,
    /// Command bridges involving the target
    pub command_bridges: Vec<String>,
    /// Event bridges involving the target
    pub event_bridges: Vec<String>,
    /// Statistics
    pub stats: SliceStats,
}

/// One symbol-graph consumer of the slice target.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymbolConsumer {
    /// Consumer file path relative to project root.
    pub path: String,
    /// Lines of code in the consumer, when the file is in the snapshot.
    #[serde(default)]
    pub loc: usize,
    /// Consumer language, when the file is in the snapshot.
    #[serde(default)]
    pub language: String,
    /// Target-defined symbol names this consumer touches.
    pub symbols: Vec<String>,
}

pub(crate) fn collect_core_symbols(file: &FileAnalysis) -> Vec<CoreSymbol> {
    let mut symbols = Vec::new();
    for export in &file.exports {
        symbols.push(CoreSymbol {
            name: export.name.clone(),
            kind: export.kind.clone(),
            file: file.path.clone(),
            line: export.line,
            exported: true,
            authority: "LoctreeDerived".to_string(),
        });
    }
    for local in &file.local_symbols {
        symbols.push(CoreSymbol {
            name: local.name.clone(),
            kind: local.kind.clone(),
            file: file.path.clone(),
            line: local.line,
            exported: local.is_exported,
            authority: "LoctreeDerived".to_string(),
        });
    }
    symbols.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.name.cmp(&b.name))
    });
    symbols
}

pub(crate) fn authority_labels(has_semantic_guess: bool) -> Vec<String> {
    let mut labels = vec!["RepoVerified".to_string(), "LoctreeDerived".to_string()];
    if has_semantic_guess {
        labels.push("SemanticGuess".to_string());
    }
    labels
}

pub(crate) fn suggested_next_for_symbols(
    target_command: String,
    symbols: &[CoreSymbol],
) -> Vec<SuggestedNext> {
    let mut steps = Vec::new();
    if let Some(symbol) = symbols.first() {
        let quoted = shell_quote(&symbol.name);
        steps.push(SuggestedNext {
            command: format!("loct occurrences {quoted} --json"),
            reason: format!("verify literal/reference sites for {}", symbol.name),
        });
        steps.push(SuggestedNext {
            command: format!("loct body {quoted} --json"),
            reason: format!("inspect the defining body for {}", symbol.name),
        });
        steps.push(SuggestedNext {
            command: format!("loct find --literal {quoted} --json"),
            reason: "confirm exact identifier-boundary parity before editing".to_string(),
        });
    }
    steps.push(SuggestedNext {
        command: target_command,
        reason: "inspect blast radius before modifying this surface".to_string(),
    });
    steps
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Statistics about the slice.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SliceStats {
    /// Number of core files (usually 1).
    pub core_files: usize,
    /// Total LOC in core layer.
    pub core_loc: usize,
    /// Number of dependency files.
    pub deps_files: usize,
    /// Total LOC in deps layer.
    pub deps_loc: usize,
    /// Number of consumer files.
    pub consumers_files: usize,
    /// Total LOC in consumers layer.
    pub consumers_loc: usize,
    /// Total files across all layers.
    pub total_files: usize,
    /// Total LOC across all layers.
    pub total_loc: usize,
}

/// Strip common extensions from a path for matching
fn strip_extension(path: &str) -> &str {
    // Common extensions that may be omitted in imports
    const EXTENSIONS: &[&str] = &[
        ".tsx", ".ts", ".jsx", ".js", ".mjs", ".cjs", ".rs", ".py", ".css", ".scss", ".sass",
    ];
    for ext in EXTENSIONS {
        if let Some(stripped) = path.strip_suffix(ext) {
            return stripped;
        }
    }
    path
}

impl HolographicSlice {
    /// Create a slice from a file path using snapshot data
    pub fn from_path(snapshot: &Snapshot, target_path: &str, config: &SliceConfig) -> Option<Self> {
        // Normalize target path (handles absolute paths, ./ prefix, backslashes)
        let normalized = snapshot.normalize_path(target_path);

        // An empty or whitespace-only target can never name a real file. Without
        // this short-circuit the suffix matcher below runs `path.ends_with("")`,
        // which is true for EVERY file: the request collapses into the
        // "Ambiguous slice target" branch and spams the whole repo file list.
        // Observed via `loct context --full --markdown`, which composes an empty
        // default target. Absent input is "no match", not "ambiguous" — return
        // None quietly instead of warning.
        if normalized.trim().is_empty() {
            return None;
        }

        // Build adjacency maps from snapshot edges
        // Note: edges may have paths with or without extensions, so we build
        // maps with both forms for flexible lookup
        let mut imports: HashMap<String, Vec<String>> = HashMap::new();
        let mut imported_by: HashMap<String, Vec<String>> = HashMap::new();

        for edge in &snapshot.edges {
            // Store with original key
            imports
                .entry(edge.from.clone())
                .or_default()
                .push(edge.to.clone());
            imported_by
                .entry(edge.to.clone())
                .or_default()
                .push(edge.from.clone());

            // Also store with stripped extension key for matching
            let from_stripped = strip_extension(&edge.from);
            let to_stripped = strip_extension(&edge.to);
            if from_stripped != edge.from {
                imports
                    .entry(from_stripped.to_string())
                    .or_default()
                    .push(edge.to.clone());
            }
            if to_stripped != edge.to {
                imported_by
                    .entry(to_stripped.to_string())
                    .or_default()
                    .push(edge.from.clone());
            }
        }

        // Find the target file in snapshot
        // Priority: exact match > ends_with match
        // Warn if multiple matches found
        let matches: Vec<_> = snapshot
            .files
            .iter()
            .filter(|f| {
                let path_normalized = f.path.trim_start_matches("./").replace('\\', "/");
                path_normalized == normalized
                    || path_normalized.ends_with(&normalized)
                    || normalized.ends_with(&path_normalized)
            })
            .collect();

        if matches.is_empty() {
            return None;
        }

        // Prefer exact repo-relative match. If the request only matched by
        // suffix and more than one file qualifies, refuse to guess: callers
        // must provide a repo-relative path to avoid cross-crate basename
        // collisions such as `src/lib.rs`.
        let exact_match = matches
            .iter()
            .find(|f| {
                let path_normalized = f.path.trim_start_matches("./").replace('\\', "/");
                path_normalized == normalized
            })
            .copied();
        let target_file = match exact_match {
            Some(file) => file,
            None if matches.len() == 1 => matches[0],
            None => {
                eprintln!(
                    "[loctree][warn] Ambiguous slice target '{}': {}. Provide a repo-relative path.",
                    target_path,
                    matches
                        .iter()
                        .map(|f| f.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                return None;
            }
        };

        let target_path_norm = target_file.path.clone();
        // Also create stripped version for edge lookup
        let target_stripped = strip_extension(&target_path_norm).to_string();

        let mut slice = Self {
            target: target_file.path.clone(),
            core: Vec::new(),
            deps: Vec::new(),
            consumers: Vec::new(),
            symbol_consumers: Vec::new(),
            core_symbols: Vec::new(),
            authority_labels: Vec::new(),
            suggested_next: Vec::new(),
            command_bridges: Vec::new(),
            event_bridges: Vec::new(),
            stats: SliceStats {
                core_files: 0,
                core_loc: 0,
                deps_files: 0,
                deps_loc: 0,
                consumers_files: 0,
                consumers_loc: 0,
                total_files: 0,
                total_loc: 0,
            },
        };

        // Layer 1: Core - the target file itself
        slice
            .core
            .push(SliceFile::from_analysis(target_file, "core", 0));
        slice.core_symbols = collect_core_symbols(target_file);

        // Layer 2: Deps - files imported by target (BFS)
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();

        visited.insert(target_path_norm.clone());
        visited.insert(target_stripped.clone());

        // Try lookup with both full path and stripped path
        let direct_deps: Vec<String> = imports
            .get(&target_path_norm)
            .into_iter()
            .chain(imports.get(&target_stripped))
            .flatten()
            .cloned()
            .collect();

        for dep in direct_deps {
            let dep_stripped = strip_extension(&dep).to_string();
            if !visited.contains(&dep) && !visited.contains(&dep_stripped) {
                queue.push_back((dep.clone(), 1));
                visited.insert(dep);
                visited.insert(dep_stripped);
            }
        }

        while let Some((path, depth)) = queue.pop_front() {
            if depth > config.max_depth {
                continue;
            }

            // Find matching file in snapshot (try exact match first, then stripped)
            let file = snapshot
                .files
                .iter()
                .find(|f| f.path == path || strip_extension(&f.path) == path);

            if let Some(file) = file {
                slice
                    .deps
                    .push(SliceFile::from_analysis(file, "deps", depth));
            }

            // Go deeper for transitive deps
            if depth < config.max_depth {
                let path_stripped = strip_extension(&path).to_string();
                let transitive: Vec<String> = imports
                    .get(&path)
                    .into_iter()
                    .chain(imports.get(&path_stripped))
                    .flatten()
                    .cloned()
                    .collect();

                for dep in transitive {
                    let dep_stripped = strip_extension(&dep).to_string();
                    if !visited.contains(&dep) && !visited.contains(&dep_stripped) {
                        queue.push_back((dep.clone(), depth + 1));
                        visited.insert(dep);
                        visited.insert(dep_stripped);
                    }
                }
            }
        }

        // Layer 3: Consumers - files that import target
        // For barrel files (index.ts), we need to transitively find consumers through re-export chains
        if config.include_consumers {
            let mut all_consumers = HashSet::new();
            let mut to_visit: VecDeque<String> = VecDeque::new();
            let mut visited_for_consumers = HashSet::new();
            let is_target_consumer = |consumer_path: &str| {
                consumer_path == target_path_norm.as_str()
                    || strip_extension(consumer_path) == target_stripped.as_str()
            };

            // Start with direct consumers
            let direct_consumers: Vec<String> = imported_by
                .get(&target_path_norm)
                .into_iter()
                .chain(imported_by.get(&target_stripped))
                .flatten()
                .cloned()
                .collect();

            for consumer in direct_consumers {
                if is_target_consumer(&consumer) {
                    continue;
                }
                all_consumers.insert(consumer.clone());
                to_visit.push_back(consumer);
            }

            // Transitively follow through barrel files
            // If A imports barrel B, and B re-exports target, then A is a consumer of target
            while let Some(current) = to_visit.pop_front() {
                if visited_for_consumers.contains(&current) {
                    continue;
                }
                visited_for_consumers.insert(current.clone());

                // Check if current file is a barrel that re-exports the target
                let current_file = snapshot
                    .files
                    .iter()
                    .find(|f| f.path == current || strip_extension(&f.path) == current);

                let is_barrel = current_file
                    .map(|f| !f.reexports.is_empty())
                    .unwrap_or(false);

                if is_barrel {
                    // Find consumers of this barrel and add them
                    let current_stripped = strip_extension(&current).to_string();
                    let barrel_consumers: Vec<String> = imported_by
                        .get(&current)
                        .into_iter()
                        .chain(imported_by.get(&current_stripped))
                        .flatten()
                        .cloned()
                        .collect();

                    for consumer in barrel_consumers {
                        if is_target_consumer(&consumer) {
                            continue;
                        }
                        if all_consumers.insert(consumer.clone()) {
                            to_visit.push_back(consumer);
                        }
                    }
                }
            }

            // Convert consumer paths to SliceFile objects
            for consumer_path in all_consumers {
                if is_target_consumer(&consumer_path) {
                    continue;
                }
                let file = snapshot
                    .files
                    .iter()
                    .find(|f| f.path == consumer_path || strip_extension(&f.path) == consumer_path);

                if let Some(file) = file {
                    // Avoid duplicates (shouldn't happen with HashSet, but safety check)
                    if !slice.consumers.iter().any(|c| c.path == file.path) {
                        slice
                            .consumers
                            .push(SliceFile::from_analysis(file, "consumers", 1));
                    }
                }
            }
        }

        // Symbol-graph consumer layer (Wave C-1): files using symbols defined
        // in the target without importing it. Composed unconditionally — this
        // is the only consumer surface for same-module Swift, where
        // `include_consumers` over import edges finds nothing.
        if let Some(graph) = &snapshot.symbol_graph {
            let target_as_path = PathBuf::from(&target_path_norm);
            for (consumer_path, symbols) in graph.file_symbol_consumers(&target_as_path) {
                let file = snapshot
                    .files
                    .iter()
                    .find(|f| f.path == consumer_path || strip_extension(&f.path) == consumer_path);
                slice.symbol_consumers.push(SymbolConsumer {
                    path: consumer_path,
                    loc: file.map(|f| f.loc).unwrap_or_default(),
                    language: file.map(|f| f.language.clone()).unwrap_or_default(),
                    symbols,
                });
            }
        }
        slice.authority_labels = authority_labels(!slice.symbol_consumers.is_empty());
        slice.suggested_next = suggested_next_for_symbols(
            format!("loct impact {}", shell_quote(&slice.target)),
            &slice.core_symbols,
        );

        // Collect command bridges involving this file
        for bridge in &snapshot.command_bridges {
            let involves_target = bridge
                .frontend_calls
                .iter()
                .any(|(f, _)| f == &target_path_norm || strip_extension(f) == target_stripped)
                || bridge
                    .backend_handler
                    .as_ref()
                    .map(|(f, _)| f == &target_path_norm || strip_extension(f) == target_stripped)
                    .unwrap_or(false);
            if involves_target {
                slice.command_bridges.push(bridge.name.clone());
            }
        }

        // Collect event bridges involving this file
        for bridge in &snapshot.event_bridges {
            let involves_target =
                bridge.emits.iter().any(|(f, _, _)| {
                    f == &target_path_norm || strip_extension(f) == target_stripped
                }) || bridge
                    .listens
                    .iter()
                    .any(|(f, _)| f == &target_path_norm || strip_extension(f) == target_stripped);
            if involves_target {
                slice.event_bridges.push(bridge.name.clone());
            }
        }

        // Calculate stats
        slice.stats.core_files = slice.core.len();
        slice.stats.core_loc = slice.core.iter().map(|f| f.loc).sum();
        slice.stats.deps_files = slice.deps.len();
        slice.stats.deps_loc = slice.deps.iter().map(|f| f.loc).sum();
        slice.stats.consumers_files = slice.consumers.len();
        slice.stats.consumers_loc = slice.consumers.iter().map(|f| f.loc).sum();
        slice.stats.total_files =
            slice.stats.core_files + slice.stats.deps_files + slice.stats.consumers_files;
        slice.stats.total_loc =
            slice.stats.core_loc + slice.stats.deps_loc + slice.stats.consumers_loc;

        // Sort deps by depth, then by path
        slice
            .deps
            .sort_by(|a, b| a.depth.cmp(&b.depth).then(a.path.cmp(&b.path)));
        slice.consumers.sort_by(|a, b| a.path.cmp(&b.path));

        Some(slice)
    }

    /// Print slice in human-readable format
    pub fn print(&self) {
        const DISPLAY_LIMIT: usize = 25;

        println!("Slice for: {}", self.target);
        println!();

        println!(
            "Core ({} files, {} LOC):",
            self.stats.core_files, self.stats.core_loc
        );
        for f in &self.core {
            println!(
                "  {} ({} LOC, {}){}",
                f.path,
                f.loc,
                f.descriptor(),
                f.ignored_tag()
            );
        }

        if !self.core_symbols.is_empty() {
            println!("\nCore symbols ({}):", self.core_symbols.len());
            for symbol in self.core_symbols.iter().take(DISPLAY_LIMIT) {
                let line = symbol
                    .line
                    .map(|line| line.to_string())
                    .unwrap_or_else(|| "?".to_string());
                println!(
                    "  {}:{} {} {} [{}]",
                    symbol.file, line, symbol.kind, symbol.name, symbol.authority
                );
            }
            if self.core_symbols.len() > DISPLAY_LIMIT {
                println!(
                    "  ... and {} more (use --json for full list)",
                    self.core_symbols.len() - DISPLAY_LIMIT
                );
            }
        }

        println!(
            "\nDeps ({} files, {} LOC):",
            self.stats.deps_files, self.stats.deps_loc
        );

        for (i, f) in self.deps.iter().enumerate() {
            if i >= DISPLAY_LIMIT {
                println!(
                    "  ... and {} more (use --json for full list)",
                    self.deps.len() - DISPLAY_LIMIT
                );
                break;
            }
            let indent = "  ".repeat(f.depth);
            println!(
                "{}[d{}] {} ({} LOC, {}){}",
                indent,
                f.depth,
                f.path,
                f.loc,
                f.descriptor(),
                f.ignored_tag()
            );
        }

        if !self.consumers.is_empty() {
            println!(
                "\nConsumers ({} files, {} LOC):",
                self.stats.consumers_files, self.stats.consumers_loc
            );

            for (i, f) in self.consumers.iter().enumerate() {
                if i >= DISPLAY_LIMIT {
                    println!(
                        "  ... and {} more (use --json for full list)",
                        self.consumers.len() - DISPLAY_LIMIT
                    );
                    break;
                }
                println!(
                    "  {} ({} LOC, {}){}",
                    f.path,
                    f.loc,
                    f.descriptor(),
                    f.ignored_tag()
                );
            }
        }

        if !self.symbol_consumers.is_empty() {
            println!(
                "\nSymbol consumers ({} files, heuristic):",
                self.symbol_consumers.len()
            );
            for (i, c) in self.symbol_consumers.iter().enumerate() {
                if i >= DISPLAY_LIMIT {
                    println!(
                        "  ... and {} more (use --json for full list)",
                        self.symbol_consumers.len() - DISPLAY_LIMIT
                    );
                    break;
                }
                println!(
                    "  {} ({} LOC, {}) via {}",
                    c.path,
                    c.loc,
                    if c.language.is_empty() {
                        "?"
                    } else {
                        c.language.as_str()
                    },
                    c.symbols.join(", ")
                );
            }
        }

        if !self.command_bridges.is_empty() {
            println!("\nCommand bridges: {}", self.command_bridges.join(", "));
        }

        if !self.event_bridges.is_empty() {
            println!("Event bridges: {}", self.event_bridges.join(", "));
        }

        if !self.suggested_next.is_empty() {
            println!("\nsuggested next:");
            for step in &self.suggested_next {
                println!("  {}  # {}", step.command, step.reason);
            }
        }

        println!(
            "\nTotal: {} files, {} LOC",
            self.stats.total_files, self.stats.total_loc
        );
    }

    /// Output as JSON
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "target": self.target,
            "core": self.core,
            "deps": self.deps,
            "consumers": self.consumers,
            "symbolConsumers": self.symbol_consumers,
            "coreSymbols": self.core_symbols,
            "authorityLabels": self.authority_labels,
            "suggestedNext": self.suggested_next,
            "commandBridges": self.command_bridges,
            "eventBridges": self.event_bridges,
            "stats": self.stats,
        })
    }
}

/// Auto-create snapshot if it doesn't exist, or prompt in interactive mode.
///
/// Snapshot creation is a thin call into the freshness authority
/// (`crate::snapshot::acquire_snapshot`): missing snapshots are created with
/// the same unified file universe as `loct`.
fn ensure_snapshot(root: &Path, _parsed: &ParsedArgs) -> io::Result<bool> {
    let snapshot_path = crate::snapshot::Snapshot::snapshot_path(root);
    let create = |print_summary: bool| -> io::Result<()> {
        crate::snapshot::acquire_snapshot(
            &[root.to_path_buf()],
            crate::snapshot::SnapshotReusePolicy::TrustExisting,
            &crate::snapshot::AcquireOptions {
                print_scan_summary: print_summary,
                ..Default::default()
            },
        )
        .map(|_| ())
    };

    if !std::io::stdin().is_terminal() {
        // Non-interactive: auto-create snapshot silently
        create(false)?;
        eprintln!();
        return Ok(true);
    }

    eprintln!("No snapshot found at {}", snapshot_path.display());
    eprintln!("Run `loctree` first to create a snapshot.");
    eprintln!();
    eprint!("Create snapshot now? [Y/n] ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    if input.trim().is_empty() || input.trim().to_lowercase() == "y" {
        create(true)?;
        eprintln!();
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Run slice command
pub fn run_slice(
    root: &Path,
    target: &str,
    include_consumers: bool,
    json_output: bool,
    parsed: &ParsedArgs,
) -> io::Result<()> {
    // Search upward for .loctree/ directory (like git finds .git/)
    let effective_root = Snapshot::find_loctree_root(root)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|cwd| Snapshot::find_loctree_root(&cwd))
        })
        .unwrap_or_else(|| root.to_path_buf());

    // Force rescan if --rescan flag is set (for uncommitted files)
    if parsed.slice_rescan {
        if !std::io::stdin().is_terminal() {
            eprintln!("[loct] Rescanning for new files...");
        }
        crate::snapshot::acquire_snapshot(
            std::slice::from_ref(&effective_root),
            crate::snapshot::SnapshotReusePolicy::TrustExisting,
            &crate::snapshot::AcquireOptions {
                fresh: true,
                quiet: true,
                print_scan_summary: true,
                ..Default::default()
            },
        )?;
    } else if !Snapshot::exists(&effective_root) {
        // Check if snapshot exists, prompt to create if not
        if ensure_snapshot(&effective_root, parsed)? {
            // Snapshot was created, continue
        } else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "No snapshot found. Run `loctree` first to create one.",
            ));
        }
    }

    let snapshot = crate::snapshot::acquire_snapshot(
        std::slice::from_ref(&effective_root),
        crate::snapshot::SnapshotReusePolicy::Strict,
        &crate::snapshot::AcquireOptions {
            include_ignored: parsed.include_ignored,
            ..Default::default()
        },
    )?;

    let config = SliceConfig {
        include_consumers,
        max_depth: 2,
    };

    let slice = match HolographicSlice::from_path(&snapshot, target, &config) {
        Some(s) => s,
        None => {
            eprintln!();
            eprintln!("[ERR] Target file '{}' not found in snapshot.", target);
            eprintln!();
            let target_path = assemble_slice_target_path(target)
                .map(|raw| {
                    if raw.is_absolute() {
                        raw
                    } else {
                        effective_root.join(raw)
                    }
                })
                .ok();
            if let Some(target_path) = target_path
                && target_path.exists()
                && let Ok(sanitized) =
                    crate::fs_utils::SanitizedPath::within(&effective_root, &target_path)
                && let Some(note) =
                    crate::fs_utils::explain_ignore_for_path(&effective_root, sanitized.as_path())
            {
                eprintln!("   Detected exclusion: {note}");
                eprintln!();
            }
            eprintln!("   Possible causes:");
            eprintln!("   - File path is incorrect or uses wrong case");
            eprintln!("   - File was added after last snapshot (run `loctree` to update)");
            eprintln!("   - File is excluded by .gitignore or .loctignore");
            eprintln!();
            std::process::exit(1);
        }
    };

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&slice.to_json())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
        );
    } else {
        slice.print();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{EventBridge, GraphEdge, Snapshot, SnapshotMetadata};
    use crate::types::FileAnalysis;

    fn create_test_snapshot() -> Snapshot {
        Snapshot {
            metadata: SnapshotMetadata {
                schema_version: crate::snapshot::SNAPSHOT_SCHEMA_VERSION.to_string(),
                generated_at: "2025-01-01T00:00:00Z".to_string(),
                roots: vec!["/test".to_string()],
                languages: ["rust".to_string()].into_iter().collect(),
                file_count: 4,
                total_loc: 400,
                scan_duration_ms: 100,
                resolver_config: None,
                manifest_summary: Vec::new(),
                entrypoints: Vec::new(),
                entrypoint_drift: crate::snapshot::EntrypointDriftSummary::default(),
                git_repo: None,
                git_owner_repo: None,
                git_branch: None,
                git_commit: None,
                git_scan_id: None,
            },
            files: vec![
                FileAnalysis {
                    path: "src/main.rs".to_string(),
                    loc: 100,
                    language: "rust".to_string(),
                    ..FileAnalysis::new("src/main.rs".to_string())
                },
                FileAnalysis {
                    path: "src/lib.rs".to_string(),
                    loc: 150,
                    language: "rust".to_string(),
                    ..FileAnalysis::new("src/lib.rs".to_string())
                },
                FileAnalysis {
                    path: "src/utils.rs".to_string(),
                    loc: 80,
                    language: "rust".to_string(),
                    ..FileAnalysis::new("src/utils.rs".to_string())
                },
                FileAnalysis {
                    path: "src/tests.rs".to_string(),
                    loc: 70,
                    language: "rust".to_string(),
                    ..FileAnalysis::new("src/tests.rs".to_string())
                },
            ],
            edges: vec![
                GraphEdge {
                    from: "src/main.rs".to_string(),
                    to: "src/lib.rs".to_string(),
                    label: "import".to_string(),
                },
                GraphEdge {
                    from: "src/lib.rs".to_string(),
                    to: "src/utils.rs".to_string(),
                    label: "import".to_string(),
                },
                GraphEdge {
                    from: "src/tests.rs".to_string(),
                    to: "src/lib.rs".to_string(),
                    label: "import".to_string(),
                },
            ],
            export_index: Default::default(),
            command_bridges: vec![],
            event_bridges: vec![EventBridge {
                name: "test_event".to_string(),
                emits: vec![("src/lib.rs".to_string(), 10, "emit".to_string())],
                listens: vec![("src/main.rs".to_string(), 20)],
                is_fe_sync: false,
                same_file_sync: false,
            }],
            barrels: vec![],
            semantic_facts: None,
            symbol_graph: None,
        }
    }

    #[test]
    fn test_slice_core_only() {
        let snapshot = create_test_snapshot();
        let config = SliceConfig::default();

        let slice = HolographicSlice::from_path(&snapshot, "src/lib.rs", &config)
            .expect("slice src/lib.rs");

        assert_eq!(slice.target, "src/lib.rs");
        assert_eq!(slice.core.len(), 1);
        assert_eq!(slice.core[0].path, "src/lib.rs");
        assert_eq!(slice.stats.core_loc, 150);
    }

    #[test]
    fn test_slice_with_deps() {
        let snapshot = create_test_snapshot();
        let config = SliceConfig::default();

        let slice = HolographicSlice::from_path(&snapshot, "src/lib.rs", &config)
            .expect("slice src/lib.rs");

        // lib.rs imports utils.rs
        assert_eq!(slice.deps.len(), 1);
        assert_eq!(slice.deps[0].path, "src/utils.rs");
        assert_eq!(slice.deps[0].depth, 1);
    }

    #[test]
    fn test_slice_with_consumers() {
        let snapshot = create_test_snapshot();
        let config = SliceConfig {
            include_consumers: true,
            ..Default::default()
        };

        let slice = HolographicSlice::from_path(&snapshot, "src/lib.rs", &config)
            .expect("slice src/lib.rs with consumers");

        // lib.rs is imported by main.rs and tests.rs
        assert_eq!(slice.consumers.len(), 2);
        let consumer_paths: Vec<_> = slice.consumers.iter().map(|f| f.path.as_str()).collect();
        assert!(consumer_paths.contains(&"src/main.rs"));
        assert!(consumer_paths.contains(&"src/tests.rs"));
    }

    #[test]
    fn test_slice_transitive_deps() {
        let snapshot = create_test_snapshot();
        let config = SliceConfig {
            include_consumers: false,
            max_depth: 2,
        };

        let slice = HolographicSlice::from_path(&snapshot, "src/main.rs", &config)
            .expect("slice src/main.rs with transitive deps");

        // main.rs -> lib.rs (depth 1) -> utils.rs (depth 2)
        assert_eq!(slice.deps.len(), 2);
        let dep_paths: Vec<_> = slice.deps.iter().map(|f| f.path.as_str()).collect();
        assert!(dep_paths.contains(&"src/lib.rs"));
        assert!(dep_paths.contains(&"src/utils.rs"));
    }

    #[test]
    fn test_slice_event_bridges() {
        let snapshot = create_test_snapshot();
        let config = SliceConfig::default();

        let slice = HolographicSlice::from_path(&snapshot, "src/lib.rs", &config)
            .expect("slice src/lib.rs");

        // lib.rs emits test_event
        assert_eq!(slice.event_bridges.len(), 1);
        assert_eq!(slice.event_bridges[0], "test_event");
    }

    #[test]
    fn slice_json_exposes_core_symbols_authority_and_suggested_next() {
        let mut snapshot = create_test_snapshot();
        let lib = snapshot
            .files
            .iter_mut()
            .find(|file| file.path == "src/lib.rs")
            .expect("fixture lib.rs");
        lib.exports.push(crate::types::ExportSymbol {
            name: "load_patient".to_string(),
            kind: "function".to_string(),
            export_type: "named".to_string(),
            line: Some(12),
            params: Vec::new(),
            symbol_id: Default::default(),
        });
        lib.local_symbols.push(crate::types::LocalSymbol {
            name: "PatientCache".to_string(),
            kind: "struct".to_string(),
            line: Some(24),
            context: "struct PatientCache;".to_string(),
            is_exported: false,
        });

        let slice = HolographicSlice::from_path(&snapshot, "src/lib.rs", &SliceConfig::default())
            .expect("slice src/lib.rs");
        let json = slice.to_json();

        let symbols = json["coreSymbols"].as_array().expect("coreSymbols array");
        assert!(
            symbols.iter().any(|symbol| {
                symbol["name"] == "load_patient"
                    && symbol["file"] == "src/lib.rs"
                    && symbol["line"] == 12
                    && symbol["authority"] == "LoctreeDerived"
            }),
            "slice should expose exported core symbols with file:line authority: {json}"
        );
        assert!(
            symbols.iter().any(|symbol| {
                symbol["name"] == "PatientCache"
                    && symbol["file"] == "src/lib.rs"
                    && symbol["line"] == 24
            }),
            "slice should expose local core symbols too: {json}"
        );
        assert_eq!(
            json["authorityLabels"]
                .as_array()
                .expect("authorityLabels")
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>(),
            vec!["RepoVerified", "LoctreeDerived"]
        );
        assert!(
            json["suggestedNext"]
                .as_array()
                .expect("suggestedNext")
                .iter()
                .any(|step| step["command"] == "loct occurrences 'load_patient' --json"),
            "slice should suggest concrete next Loctree moves: {json}"
        );
    }

    #[test]
    fn test_slice_not_found() {
        let snapshot = create_test_snapshot();
        let config = SliceConfig::default();

        let slice = HolographicSlice::from_path(&snapshot, "nonexistent.rs", &config);
        assert!(slice.is_none());
    }

    #[test]
    fn test_slice_refuses_ambiguous_suffix_target() {
        let mut snapshot = create_test_snapshot();
        snapshot.files.push(FileAnalysis {
            path: "crates/nested/src/lib.rs".to_string(),
            loc: 55,
            language: "rust".to_string(),
            ..FileAnalysis::new("crates/nested/src/lib.rs".to_string())
        });
        let config = SliceConfig::default();

        let ambiguous = HolographicSlice::from_path(&snapshot, "lib.rs", &config);
        assert!(
            ambiguous.is_none(),
            "bare basename must not guess between duplicate lib.rs files"
        );

        let exact = HolographicSlice::from_path(&snapshot, "src/lib.rs", &config)
            .expect("exact repo-relative path should still resolve");
        assert_eq!(exact.target, "src/lib.rs");
    }

    #[test]
    fn test_slice_empty_target_is_absent_not_ambiguous() {
        // An empty / whitespace target must resolve to None WITHOUT tripping the
        // "Ambiguous slice target" branch. Regression for the 2026-06-26
        // loctree-feedback report: `loct context --full --markdown` composed an empty
        // default target, `path.ends_with("")` matched every file, and the slicer
        // spammed the whole repo list as an ambiguity warning.
        let snapshot = create_test_snapshot();
        let config = SliceConfig::default();

        for target in ["", "   ", "\t", "./"] {
            assert!(
                HolographicSlice::from_path(&snapshot, target, &config).is_none(),
                "empty/whitespace target {target:?} must be a quiet None, not ambiguous"
            );
        }
    }

    #[test]
    fn test_slice_stats() {
        let snapshot = create_test_snapshot();
        let config = SliceConfig {
            include_consumers: true,
            max_depth: 1,
        };

        let slice = HolographicSlice::from_path(&snapshot, "src/lib.rs", &config)
            .expect("slice src/lib.rs for stats");

        assert_eq!(slice.stats.core_files, 1);
        assert_eq!(slice.stats.core_loc, 150); // lib.rs
        assert_eq!(slice.stats.deps_files, 1); // utils.rs
        assert_eq!(slice.stats.deps_loc, 80);
        assert_eq!(slice.stats.consumers_files, 2); // main.rs, tests.rs
        assert_eq!(slice.stats.consumers_loc, 170); // 100 + 70
        assert_eq!(slice.stats.total_files, 4);
        assert_eq!(slice.stats.total_loc, 400);
    }

    #[test]
    fn test_slice_config_default() {
        let config = SliceConfig::default();
        assert!(config.include_consumers);
        assert_eq!(config.max_depth, 2);
    }

    #[test]
    fn test_slice_file_fields() {
        let file = SliceFile {
            path: "src/main.rs".to_string(),
            layer: "core".to_string(),
            loc: 100,
            language: "rust".to_string(),
            kind: "code".to_string(),
            resource_kind: None,
            depth: 0,
            ignored: false,
        };
        assert_eq!(file.path, "src/main.rs");
        assert_eq!(file.layer, "core");
        assert_eq!(file.loc, 100);
        assert_eq!(file.language, "rust");
        assert_eq!(file.depth, 0);
    }

    #[test]
    fn test_slice_stats_default() {
        let stats = SliceStats {
            core_files: 0,
            core_loc: 0,
            deps_files: 0,
            deps_loc: 0,
            consumers_files: 0,
            consumers_loc: 0,
            total_files: 0,
            total_loc: 0,
        };
        assert_eq!(stats.total_files, 0);
        assert_eq!(stats.total_loc, 0);
    }

    #[test]
    fn test_slice_depth_limit() {
        let snapshot = create_test_snapshot();
        let config = SliceConfig {
            include_consumers: false,
            max_depth: 1, // Only direct deps
        };

        let slice = HolographicSlice::from_path(&snapshot, "src/main.rs", &config)
            .expect("slice src/main.rs with depth 1");

        // main.rs -> lib.rs (depth 1), but utils.rs (depth 2) should be excluded
        assert_eq!(slice.deps.len(), 1);
        assert_eq!(slice.deps[0].path, "src/lib.rs");
    }

    #[test]
    fn test_slice_no_deps() {
        let snapshot = create_test_snapshot();
        let config = SliceConfig::default();

        // utils.rs has no outgoing edges, so no deps
        let slice = HolographicSlice::from_path(&snapshot, "src/utils.rs", &config)
            .expect("slice src/utils.rs");

        assert!(slice.deps.is_empty());
    }

    #[test]
    fn test_slice_command_bridges_empty() {
        let snapshot = create_test_snapshot();
        let config = SliceConfig::default();

        let slice = HolographicSlice::from_path(&snapshot, "src/utils.rs", &config)
            .expect("slice src/utils.rs");

        // No command bridges in this test snapshot
        assert!(slice.command_bridges.is_empty());
    }

    #[test]
    fn test_slice_serde_roundtrip() {
        let slice = HolographicSlice {
            target: "src/main.rs".to_string(),
            core: vec![SliceFile {
                path: "src/main.rs".to_string(),
                layer: "core".to_string(),
                loc: 100,
                language: "rust".to_string(),
                kind: "code".to_string(),
                resource_kind: None,
                depth: 0,
                ignored: false,
            }],
            deps: vec![],
            consumers: vec![],
            symbol_consumers: vec![],
            core_symbols: vec![],
            authority_labels: vec![],
            suggested_next: vec![],
            command_bridges: vec![],
            event_bridges: vec![],
            stats: SliceStats {
                core_files: 1,
                core_loc: 100,
                deps_files: 0,
                deps_loc: 0,
                consumers_files: 0,
                consumers_loc: 0,
                total_files: 1,
                total_loc: 100,
            },
        };

        let json = serde_json::to_string(&slice).expect("serialize");
        let deser: HolographicSlice = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deser.target, "src/main.rs");
        assert_eq!(deser.core.len(), 1);
        assert_eq!(deser.stats.core_loc, 100);
    }

    #[test]
    fn test_slice_consumers_excludes_self_reference() {
        let snapshot = Snapshot {
            metadata: SnapshotMetadata {
                schema_version: crate::snapshot::SNAPSHOT_SCHEMA_VERSION.to_string(),
                generated_at: "2025-01-01T00:00:00Z".to_string(),
                roots: vec!["/test".to_string()],
                languages: ["rust".to_string()].into_iter().collect(),
                file_count: 2,
                total_loc: 150,
                scan_duration_ms: 100,
                resolver_config: None,
                manifest_summary: Vec::new(),
                entrypoints: Vec::new(),
                entrypoint_drift: crate::snapshot::EntrypointDriftSummary::default(),
                git_repo: None,
                git_owner_repo: None,
                git_branch: None,
                git_commit: None,
                git_scan_id: None,
            },
            files: vec![
                FileAnalysis {
                    path: "src/codex.rs".to_string(),
                    loc: 100,
                    language: "rust".to_string(),
                    ..FileAnalysis::new("src/codex.rs".to_string())
                },
                FileAnalysis {
                    path: "src/user.rs".to_string(),
                    loc: 50,
                    language: "rust".to_string(),
                    ..FileAnalysis::new("src/user.rs".to_string())
                },
            ],
            edges: vec![
                GraphEdge {
                    from: "src/codex.rs".to_string(),
                    to: "src/codex.rs".to_string(),
                    label: "import".to_string(),
                },
                GraphEdge {
                    from: "src/user.rs".to_string(),
                    to: "src/codex.rs".to_string(),
                    label: "import".to_string(),
                },
            ],
            export_index: Default::default(),
            command_bridges: vec![],
            event_bridges: vec![],
            barrels: vec![],
            semantic_facts: None,
            symbol_graph: None,
        };

        let config = SliceConfig {
            include_consumers: true,
            ..Default::default()
        };

        let slice = HolographicSlice::from_path(&snapshot, "src/codex.rs", &config)
            .expect("slice src/codex.rs with consumers");

        assert!(
            slice.deps.is_empty(),
            "target self-import should not appear in deps"
        );
        let consumer_paths: Vec<_> = slice.consumers.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(slice.consumers.len(), 1);
        assert!(consumer_paths.contains(&"src/user.rs"));
        assert!(
            !consumer_paths.contains(&"src/codex.rs"),
            "target file must not appear as its own consumer"
        );
    }

    #[test]
    fn test_slice_consumers_through_barrel() {
        use crate::types::{FileAnalysis, ReexportEntry, ReexportKind};

        // Create a test snapshot with barrel file re-export chain:
        // Component.tsx -> features/index.ts (barrel) -> App.tsx
        let snapshot = Snapshot {
            metadata: SnapshotMetadata {
                schema_version: crate::snapshot::SNAPSHOT_SCHEMA_VERSION.to_string(),
                generated_at: "2025-01-01T00:00:00Z".to_string(),
                roots: vec!["/test".to_string()],
                languages: ["typescript".to_string()].into_iter().collect(),
                file_count: 3,
                total_loc: 300,
                scan_duration_ms: 100,
                resolver_config: None,
                manifest_summary: Vec::new(),
                entrypoints: Vec::new(),
                entrypoint_drift: crate::snapshot::EntrypointDriftSummary::default(),
                git_repo: None,
                git_owner_repo: None,
                git_branch: None,
                git_commit: None,
                git_scan_id: None,
            },
            files: vec![
                FileAnalysis {
                    path: "src/Component.tsx".to_string(),
                    loc: 100,
                    language: "typescript".to_string(),
                    ..FileAnalysis::new("src/Component.tsx".to_string())
                },
                {
                    let mut barrel = FileAnalysis {
                        path: "src/features/index.ts".to_string(),
                        loc: 10,
                        language: "typescript".to_string(),
                        ..FileAnalysis::new("src/features/index.ts".to_string())
                    };
                    barrel.reexports.push(ReexportEntry {
                        source: "../Component".to_string(),
                        kind: ReexportKind::Named(vec![(
                            "MyComponent".to_string(),
                            "MyComponent".to_string(),
                        )]),
                        resolved: Some("src/Component.tsx".to_string()),
                    });
                    barrel
                },
                FileAnalysis {
                    path: "src/App.tsx".to_string(),
                    loc: 150,
                    language: "typescript".to_string(),
                    ..FileAnalysis::new("src/App.tsx".to_string())
                },
            ],
            edges: vec![
                // App.tsx imports from barrel
                GraphEdge {
                    from: "src/App.tsx".to_string(),
                    to: "src/features/index.ts".to_string(),
                    label: "import".to_string(),
                },
                // Barrel re-exports Component
                GraphEdge {
                    from: "src/features/index.ts".to_string(),
                    to: "src/Component.tsx".to_string(),
                    label: "reexport".to_string(),
                },
            ],
            export_index: Default::default(),
            command_bridges: vec![],
            event_bridges: vec![],
            barrels: vec![],
            semantic_facts: None,
            symbol_graph: None,
        };

        let config = SliceConfig {
            include_consumers: true,
            max_depth: 2,
        };

        let slice = HolographicSlice::from_path(&snapshot, "src/Component.tsx", &config)
            .expect("slice Component.tsx with consumers through barrel");

        // CRITICAL TEST: App.tsx should show up as a consumer of Component.tsx
        // even though it imports through the barrel file
        assert_eq!(
            slice.consumers.len(),
            2,
            "Should have both barrel and App.tsx as consumers"
        );
        let consumer_paths: Vec<_> = slice.consumers.iter().map(|f| f.path.as_str()).collect();
        assert!(
            consumer_paths.contains(&"src/App.tsx"),
            "App.tsx should be a consumer (imports through barrel)"
        );
        assert!(
            consumer_paths.contains(&"src/features/index.ts"),
            "Barrel should be a consumer (directly re-exports)"
        );
    }
}
