//! Lightweight Makefile / GNUmakefile analyzer.
//!
//! Regex-based parser that extracts targets, variable definitions, and
//! `include` directives. Prerequisite edges are captured as part of each
//! target's exports metadata so the dependency graph can see intra-file
//! target-to-target references.

use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;

use crate::types::{ExportSymbol, FileAnalysis, ImportEntry, ImportKind, ImportResolutionKind};

// Target line:  name: prerequisites   (exclude `:=` assignments)
// We capture the target name and pre-req tail; must NOT match `FOO := bar`.
// Leading `.` permitted for special targets (`.PHONY`, `.DEFAULT`, ...).
static RE_TARGET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(\.?[A-Za-z_][-A-Za-z0-9_.]*)\s*:\s*([^=].*)?$")
        .expect("valid makefile target regex")
});

// Variable definitions:   FOO = bar | FOO := bar | FOO ?= bar | FOO += bar
static RE_VAR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^([A-Z_][A-Z0-9_]*)\s*(\?=|:=|\+=|=)").expect("valid makefile variable regex")
});

// Include directive:   include path  |  -include path  |  sinclude path
static RE_INCLUDE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*(?:-include|sinclude|include)\s+(.+)$").expect("valid makefile include regex")
});

/// Detect whether a filename is a Makefile (GNU-make family).
pub fn is_makefile_name(filename: &str) -> bool {
    matches!(
        filename,
        "Makefile" | "makefile" | "GNUmakefile" | "BSDmakefile"
    ) || filename.ends_with(".mk")
        || filename.ends_with(".make")
}

/// Analyze a Makefile with regex-based structural extraction.
pub fn analyze_makefile(content: &str, relative: String) -> FileAnalysis {
    let mut analysis = FileAnalysis::new(relative);
    analysis.imports = parse_includes(content);
    analysis.imports.extend(parse_recipe_script_refs(content));
    analysis.exports = parse_targets_and_vars(content);
    analysis
}

/// Script extensions a recipe line can dispatch to as an explicit path fact.
/// Deliberately excludes `.mk`/`.make` (those are `include` territory) and
/// compiled-language sources (a recipe mentions `main.c` to build it, not to
/// dispatch into it).
const RECIPE_SCRIPT_EXTS: &[&str] = &[
    ".py", ".sh", ".bash", ".zsh", ".js", ".mjs", ".cjs", ".ts", ".rb", ".pl",
];

/// True when a Makefile import entry's source is a recipe script-dispatch
/// candidate (vs an `include` directive). The discriminator is textual on
/// purpose: recipe refs keep `ImportKind::Static`, because `Dynamic` would
/// put the script — and everything it transitively imports — behind the
/// dead-code dynamic-reachability fence (React.lazy semantics), silencing
/// symbol-level dead reporting across whole packages dispatched by `make`.
pub fn is_recipe_script_source(source: &str) -> bool {
    RECIPE_SCRIPT_EXTS
        .iter()
        .any(|ext| source.len() > ext.len() && source.ends_with(ext))
}

/// H8 (post-audit): extract explicit script paths from recipe lines.
///
/// `focus tools/` used to see the Makefile as a consumer only through the
/// bare-name call synthesis that audit N02 removed as fabrication. The honest
/// replacement is textual: a tab-indented recipe line that spells out
/// `tools/probe.py` names a concrete file. Tokens carrying unexpanded
/// variables (`$(PY)`, `${DIR}/x.sh`) are skipped — they are not path facts.
/// Existence is NOT checked here (no filesystem access in this parser);
/// `root_scan` verifies each candidate against the scanned universe before
/// granting `Local` resolution and a graph edge (N10 parity).
fn parse_recipe_script_refs(content: &str) -> Vec<ImportEntry> {
    let mut refs: Vec<ImportEntry> = Vec::new();
    for (idx, raw) in content.lines().enumerate() {
        if !raw.starts_with('\t') {
            continue;
        }
        for token in raw.split_whitespace() {
            let cleaned =
                token.trim_matches(|c| matches!(c, '"' | '\'' | '(' | ')' | ';' | '&' | ','));
            if cleaned.contains('$') || cleaned.contains('#') {
                continue;
            }
            let cleaned = cleaned.strip_prefix("./").unwrap_or(cleaned);
            if cleaned.is_empty() || cleaned.starts_with('-') || cleaned.starts_with('/') {
                continue;
            }
            if !RECIPE_SCRIPT_EXTS
                .iter()
                .any(|ext| cleaned.len() > ext.len() && cleaned.ends_with(ext))
            {
                continue;
            }
            if refs.iter().any(|i| i.source == cleaned) {
                continue;
            }
            let mut entry = ImportEntry::new(cleaned.to_string(), ImportKind::Static);
            entry.line = Some(idx + 1);
            entry.resolution = ImportResolutionKind::Unknown;
            refs.push(entry);
        }
    }
    refs
}

/// Join a recipe-relative script path against the Makefile's directory,
/// resolving `.`/`..` textually. `None` when the path climbs above the repo
/// root — such a target is outside the scanned universe (N10 parity).
pub fn join_recipe_path(dir: &str, spec: &str) -> Option<String> {
    let mut parts: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split(['/', '\\']).filter(|s| !s.is_empty()).collect()
    };
    for seg in spec.split(['/', '\\']) {
        match seg {
            "" | "." => continue,
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

fn parse_includes(content: &str) -> Vec<ImportEntry> {
    let mut imports: Vec<ImportEntry> = Vec::new();
    for (idx, raw) in content.lines().enumerate() {
        // Strip full-line comments; but keep leading tab-indented recipe lines.
        let line = strip_line_comment(raw);
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(caps) = RE_INCLUDE.captures(trimmed)
            && let Some(m) = caps.get(1)
        {
            let paths_str = m.as_str().trim();
            // `include a.mk b.mk` — split on whitespace
            for path in paths_str.split_whitespace() {
                let cleaned = path.trim_matches(|c| c == '"' || c == '\'');
                if cleaned.is_empty() {
                    continue;
                }
                if imports.iter().any(|i| i.source == cleaned) {
                    continue;
                }
                let mut entry = ImportEntry::new(cleaned.to_string(), ImportKind::Static);
                entry.line = Some(idx + 1);
                entry.resolution = ImportResolutionKind::Unknown;
                imports.push(entry);
            }
        }
    }
    imports
}

fn parse_targets_and_vars(content: &str) -> Vec<ExportSymbol> {
    let mut out: Vec<ExportSymbol> = Vec::new();
    for (idx, raw) in content.lines().enumerate() {
        // Recipe lines start with tab — skip, they're command bodies.
        if raw.starts_with('\t') {
            continue;
        }
        let line = strip_line_comment(raw);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Variable definition check runs first because `FOO := bar` would
        // also satisfy part of the target regex without the `[^=]` guard.
        if let Some(caps) = RE_VAR.captures(trimmed)
            && let Some(m) = caps.get(1)
        {
            let name = m.as_str().to_string();
            if !out.iter().any(|e| e.name == name && e.kind == "var") {
                out.push(ExportSymbol::new(name, "var", "named", Some(idx + 1)));
            }
            continue;
        }

        if let Some(caps) = RE_TARGET.captures(trimmed)
            && let Some(m) = caps.get(1)
        {
            let name = m.as_str().to_string();
            // Reject purely-uppercase all-caps names that look like vars we
            // failed to match because of unusual spacing — be generous: allow
            // them as both var-if-rhs-has-=, but the `[^=]` guard in RE_TARGET
            // already strips most of that risk.
            if matches!(
                name.as_str(),
                ".PHONY" | ".DEFAULT" | ".SUFFIXES" | ".IGNORE"
            ) {
                // Still useful context but not a real build target — record as
                // special kind so dead-target passes can ignore.
                if !out
                    .iter()
                    .any(|e| e.name == name && e.kind == "special_target")
                {
                    out.push(ExportSymbol::new(
                        name,
                        "special_target",
                        "named",
                        Some(idx + 1),
                    ));
                }
                continue;
            }
            if !out.iter().any(|e| e.name == name && e.kind == "target") {
                out.push(ExportSymbol::new(name, "target", "named", Some(idx + 1)));
            }
        }
    }
    out
}

fn strip_line_comment(line: &str) -> &str {
    // Makefile comments start with `#`; but we preserve recipe lines (caller
    // guards via `starts_with('\t')`). No quote tracking needed — Makefiles
    // don't have string escapes on directive lines.
    match line.find('#') {
        Some(pos) => &line[..pos],
        None => line,
    }
}

/// Resolve a Makefile `include` path relative to the file's directory.
pub fn resolve_makefile_include(spec: &str, file_path: &Path, _root: &Path) -> Option<String> {
    let parent = file_path.parent()?;
    let candidate: PathBuf = if Path::new(spec).is_absolute() {
        PathBuf::from(spec)
    } else {
        parent.join(spec)
    };
    if candidate.exists() {
        let canon = candidate.canonicalize().unwrap_or(candidate);
        return Some(canon.to_string_lossy().to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_makefile_names() {
        assert!(is_makefile_name("Makefile"));
        assert!(is_makefile_name("makefile"));
        assert!(is_makefile_name("GNUmakefile"));
        assert!(is_makefile_name("common.mk"));
        assert!(is_makefile_name("rules.make"));
        assert!(!is_makefile_name("main.rs"));
        assert!(!is_makefile_name("Dockerfile"));
    }

    #[test]
    fn parses_targets() {
        let src = r#"
all: build test

build: src/main.rs
	cargo build

test:
	cargo test

.PHONY: all build test clean

clean:
	rm -rf target
"#;
        let analysis = analyze_makefile(src, "Makefile".to_string());
        let targets: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "target")
            .map(|e| e.name.clone())
            .collect();
        assert!(targets.contains(&"all".to_string()));
        assert!(targets.contains(&"build".to_string()));
        assert!(targets.contains(&"test".to_string()));
        assert!(targets.contains(&"clean".to_string()));
        // .PHONY captured as special_target, not var
        let special: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "special_target")
            .map(|e| e.name.clone())
            .collect();
        assert!(special.contains(&".PHONY".to_string()));
    }

    #[test]
    fn parses_variables() {
        let src = r#"
VERSION := 1.0.0
NAME ?= mytool
SOURCES = main.rs lib.rs
FLAGS += -O3
lowercase = not-caps   # lowercase should be skipped by RE_VAR anchor
"#;
        let analysis = analyze_makefile(src, "Makefile".to_string());
        let vars: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "var")
            .map(|e| e.name.clone())
            .collect();
        assert!(vars.contains(&"VERSION".to_string()));
        assert!(vars.contains(&"NAME".to_string()));
        assert!(vars.contains(&"SOURCES".to_string()));
        assert!(vars.contains(&"FLAGS".to_string()));
        assert!(!vars.contains(&"lowercase".to_string()));
    }

    #[test]
    fn parses_includes() {
        let src = r#"
include common.mk
-include optional.mk
sinclude config.mk
include a.mk b.mk
# include skipped.mk   <- commented
"#;
        let analysis = analyze_makefile(src, "Makefile".to_string());
        let incs: Vec<_> = analysis.imports.iter().map(|i| i.source.clone()).collect();
        assert!(incs.contains(&"common.mk".to_string()));
        assert!(incs.contains(&"optional.mk".to_string()));
        assert!(incs.contains(&"config.mk".to_string()));
        assert!(incs.contains(&"a.mk".to_string()));
        assert!(incs.contains(&"b.mk".to_string()));
        assert!(!incs.contains(&"skipped.mk".to_string()));
    }

    #[test]
    fn recipe_script_paths_become_dispatch_candidates() {
        let src = "PY := python3\n\nprobe:\n\t$(PY) tools/probe_runner.py --json out.json\n\nlint:\n\t./scripts/lint.sh --fix && echo done\n\nvars:\n\tpython3 $(TOOLS)/hidden.py\n\tbash ${DIR}/also_hidden.sh\n";
        let analysis = analyze_makefile(src, "Makefile".to_string());
        let recipe_refs: Vec<_> = analysis
            .imports
            .iter()
            .filter(|i| is_recipe_script_source(&i.source))
            .map(|i| i.source.clone())
            .collect();
        assert!(recipe_refs.contains(&"tools/probe_runner.py".to_string()));
        assert!(recipe_refs.contains(&"scripts/lint.sh".to_string()));
        // Unexpanded make/shell variables are not path facts.
        assert!(!recipe_refs.iter().any(|s| s.contains("hidden")));
        // Flags and non-script tokens never qualify.
        assert!(!recipe_refs.iter().any(|s| s.starts_with('-')));
        // Every candidate starts Unknown — root_scan grants Local only after
        // the existence check against the scanned universe.
        // Every candidate starts Unknown and STATIC: `Dynamic` would drag the
        // dispatched script and its whole transitive import closure behind
        // the dead-code dynamic-reachability fence.
        assert!(
            analysis
                .imports
                .iter()
                .filter(|i| is_recipe_script_source(&i.source))
                .all(|i| i.kind == ImportKind::Static
                    && i.resolution == ImportResolutionKind::Unknown
                    && i.resolved_path.is_none())
        );
    }

    #[test]
    fn join_recipe_path_resolves_dots_and_refuses_root_escape() {
        assert_eq!(
            join_recipe_path("sub", "tools/x.py"),
            Some("sub/tools/x.py".to_string())
        );
        assert_eq!(
            join_recipe_path("sub", "../tools/x.py"),
            Some("tools/x.py".to_string())
        );
        assert_eq!(join_recipe_path("", "./x.sh"), Some("x.sh".to_string()));
        assert_eq!(join_recipe_path("", "../escape.sh"), None);
    }

    #[test]
    fn var_and_target_distinguished() {
        // `FOO := bar` should parse as var, NOT target (regex guard `[^=]`)
        let src = "FOO := bar\nbuild: main\n\tcc main.c\n";
        let analysis = analyze_makefile(src, "Makefile".to_string());
        let vars: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "var")
            .map(|e| e.name.clone())
            .collect();
        let targets: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "target")
            .map(|e| e.name.clone())
            .collect();
        assert!(vars.contains(&"FOO".to_string()));
        assert!(!targets.contains(&"FOO".to_string()));
        assert!(targets.contains(&"build".to_string()));
    }
}
