//! `#include` / `#import` / `@import` parsing for C-family sources.
//!
//! Line-regex based on purpose: preprocessor directives survive even when the
//! grammar chokes on the surrounding code, and they are line-oriented anyway.

use crate::symbols::{
    Confidence, LanguageId, SymbolEdge, SymbolEdgeKind, SymbolId, SymbolProvenance,
};
use crate::types::{ImportEntry, ImportKind, ImportResolutionKind};
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::Path;

// `#include <stdio.h>` / `#include "util.h"` / `#import <UIKit/UIKit.h>`
// Capture group 2 is the delimiter (`<` or `"`) so callers can tell a
// quote-include (resolvable relative to the including file) from an
// angle-include (system/framework header, left `Unknown`) apart — see
// [`resolve_quote_include`].
static RE_INCLUDE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"^\s*#\s*(include|import)\s*([<"])([^>"]+)[>"]"#).expect("valid include regex")
});

// ObjC modules: `@import UIKit;`
static RE_AT_IMPORT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*@import\s+([A-Za-z_][A-Za-z0-9_.]*)\s*;").expect("valid @import regex")
});

/// Extract include/import directives as [`ImportEntry`]s without resolving
/// them. Production goes through [`parse_includes_resolved`] (G7); this
/// stays as the path-less baseline the resolution tests contrast against.
#[cfg(test)]
pub(super) fn parse_includes(content: &str) -> Vec<ImportEntry> {
    parse_includes_inner(content, None)
}

/// Like [`parse_includes`], but also resolves quote-form includes
/// (`#include "util.h"`) relative to the including file's own directory.
///
/// Angle-bracket includes (`#include <stdio.h>`) and ObjC `@import Module;`
/// stay `Unknown` — they name a system/framework header or module, not a
/// path in this repo, so there is nothing sibling-relative to resolve.
///
/// This is lexical path resolution only (no filesystem `.exists()` probe):
/// the caller only has the including file's *relative* path, not the scan
/// root, so there is no root to join against for a real existence check.
/// `include_edges` still treats the result as `Confidence::Heuristic`.
pub(super) fn parse_includes_resolved(content: &str, including_relative: &str) -> Vec<ImportEntry> {
    parse_includes_inner(content, Some(including_relative))
}

fn parse_includes_inner(content: &str, including_relative: Option<&str>) -> Vec<ImportEntry> {
    let including_dir = including_relative.map(|rel| {
        Path::new(rel)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    });

    let mut imports: Vec<ImportEntry> = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let (path, is_quote) = if let Some(caps) = RE_INCLUDE.captures(line) {
            let delim = caps.get(2).map(|m| m.as_str()).unwrap_or("<");
            let Some(m) = caps.get(3) else { continue };
            (m.as_str(), delim == "\"")
        } else if let Some(caps) = RE_AT_IMPORT.captures(line) {
            let Some(m) = caps.get(1) else { continue };
            (m.as_str(), false)
        } else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() || imports.iter().any(|i| i.source == path) {
            continue;
        }
        let mut entry = ImportEntry::new(path.to_string(), ImportKind::Static);
        entry.line = Some(idx + 1);
        if is_quote
            && let Some(dir) = &including_dir
            && let Some(joined) = join_relative(dir, path)
        {
            entry.resolution = ImportResolutionKind::Local;
            entry.resolved_path = Some(joined);
        } else {
            entry.resolution = ImportResolutionKind::Unknown;
        }
        imports.push(entry);
    }
    imports
}

/// Lexically join `spec` onto `dir` and collapse `.`/`..` segments, without
/// touching the filesystem. `dir` and the result are `/`-separated
/// repo-relative paths (matching `FileAnalysis::path` conventions), even on
/// Windows — this never sees a real filesystem path, only spec text.
///
/// Returns `None` when the spec climbs ABOVE the repo root (`..` on an empty
/// stack): a target outside the scanned universe is not a `Local` fact
/// (audit N10) — the old code silently dropped the excess `..` and produced
/// a wrong in-repo path.
fn join_relative(dir: &str, spec: &str) -> Option<String> {
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

/// Synthesize file-level `Includes` / `ImportsModule` edges from the import
/// surface. Both endpoints are file/module pseudo-symbols (`kind = "file"` /
/// `"module"`); Wave C-1 resolves them against real targets.
pub(super) fn include_edges(
    relative: &str,
    imports: &[ImportEntry],
    lang: LanguageId,
) -> Vec<SymbolEdge> {
    let from = SymbolId::from_parts(relative, "file", relative, 0);
    imports
        .iter()
        .map(|entry| {
            // Header-ish targets are textual includes; bare module names
            // (Swift `import Foundation`, ObjC `@import UIKit;`) are module
            // imports.
            let is_header = entry.source.contains('/') || entry.source.contains('.');
            let (kind, to_kind) = if lang == LanguageId::Swift || !is_header {
                (SymbolEdgeKind::ImportsModule, "module")
            } else {
                (SymbolEdgeKind::Includes, "file")
            };
            // G7: when the import surface resolved this quote-include to a
            // sibling path, the symbol-edge target must agree with
            // `ImportEntry.resolved_path` rather than the raw header text —
            // otherwise the two layers name the same include differently.
            let target = entry.resolved_path.as_deref().unwrap_or(&entry.source);
            SymbolEdge {
                from: from.clone(),
                to: SymbolId::from_parts(target, to_kind, target, 0),
                kind,
                provenance: SymbolProvenance::TreeSitter,
                confidence: Confidence::Heuristic,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_include_and_import_forms() {
        let src = r#"
#include <stdio.h>
#include "util.h"
#import <UIKit/UIKit.h>
#import "EditorModel.h"
@import CoreData;
"#;
        let imports = parse_includes(src);
        let sources: Vec<&str> = imports.iter().map(|i| i.source.as_str()).collect();
        assert_eq!(
            sources,
            vec![
                "stdio.h",
                "util.h",
                "UIKit/UIKit.h",
                "EditorModel.h",
                "CoreData"
            ]
        );
        assert_eq!(imports[0].line, Some(2));
    }

    #[test]
    fn edges_split_includes_vs_modules() {
        let imports = parse_includes("#include \"util.h\"\n@import CoreData;\n");
        let edges = include_edges("src/a.m", &imports, LanguageId::ObjC);
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].kind, SymbolEdgeKind::Includes);
        assert_eq!(edges[1].kind, SymbolEdgeKind::ImportsModule);
        assert!(edges.iter().all(|e| e.confidence == Confidence::Heuristic));
    }

    #[test]
    fn g7_quote_include_resolves_relative_to_including_file() {
        // main.c sits next to util.h (same directory, "src/").
        let src = "#include \"util.h\"\n#include <stdio.h>\n";
        let imports = parse_includes_resolved(src, "src/main.c");

        let quote = imports
            .iter()
            .find(|i| i.source == "util.h")
            .expect("quote-include present");
        assert_eq!(
            quote.resolved_path.as_deref(),
            Some("src/util.h"),
            "quote-include must resolve relative to the including file's dir: {quote:?}"
        );
        assert_eq!(quote.resolution, ImportResolutionKind::Local);

        let angle = imports
            .iter()
            .find(|i| i.source == "stdio.h")
            .expect("angle-include present");
        assert_eq!(
            angle.resolved_path, None,
            "angle-include must stay unresolved: {angle:?}"
        );
        assert_eq!(angle.resolution, ImportResolutionKind::Unknown);
    }

    #[test]
    fn g7_quote_include_climbs_parent_dir_with_dotdot() {
        let src = "#include \"../shared/util.h\"\n";
        let imports = parse_includes_resolved(src, "src/nested/main.c");
        let entry = &imports[0];
        assert_eq!(entry.resolved_path.as_deref(), Some("src/shared/util.h"));
    }

    #[test]
    fn g7_parse_includes_unresolving_shape_is_unchanged() {
        // The production call site (c_family_syntax/mod.rs) still calls the
        // 1-arg `parse_includes`, with no including-file path in scope.
        // It must keep behaving exactly as before this fix.
        let imports = parse_includes("#include \"util.h\"\n");
        assert_eq!(imports[0].resolved_path, None);
        assert_eq!(imports[0].resolution, ImportResolutionKind::Unknown);
    }

    #[test]
    fn g7_include_edge_target_agrees_with_resolved_path() {
        let imports = parse_includes_resolved("#include \"util.h\"\n", "src/a.m");
        let edges = include_edges("src/a.m", &imports, LanguageId::ObjC);
        // SymbolId::from_parts encodes as "file::kind::name::hash"; the
        // resolved sibling path ("src/util.h") must be the `name` segment,
        // not the raw header text ("util.h"), so both layers agree.
        assert_eq!(
            edges[0].to.as_str().split("::").nth(2),
            Some("src/util.h"),
            "edge target must carry the resolved path: {:?}",
            edges[0].to
        );
    }
    /// N10 (audit): `..` climbing above the repo root is not a resolvable
    /// local include — the old code dropped the excess `..` and fabricated
    /// an in-repo path.
    #[test]
    fn n10_root_escape_include_stays_unknown() {
        let imports = parse_includes_resolved("#include \"../../outside.h\"\n", "src/a.c");
        let entry = imports
            .iter()
            .find(|i| i.source == "../../outside.h")
            .expect("entry");
        assert_eq!(entry.resolution, ImportResolutionKind::Unknown);
        assert!(entry.resolved_path.is_none());
    }
}
