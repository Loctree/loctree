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

// `#include <stdio.h>` / `#include "util.h"` / `#import <UIKit/UIKit.h>`
static RE_INCLUDE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"^\s*#\s*(include|import)\s*[<"]([^>"]+)[>"]"#).expect("valid include regex")
});

// ObjC modules: `@import UIKit;`
static RE_AT_IMPORT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*@import\s+([A-Za-z_][A-Za-z0-9_.]*)\s*;").expect("valid @import regex")
});

/// Extract include/import directives as [`ImportEntry`]s for the regular
/// import surface.
pub(super) fn parse_includes(content: &str) -> Vec<ImportEntry> {
    let mut imports: Vec<ImportEntry> = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let captured = RE_INCLUDE
            .captures(line)
            .and_then(|c| c.get(2))
            .or_else(|| RE_AT_IMPORT.captures(line).and_then(|c| c.get(1)));
        if let Some(m) = captured {
            let path = m.as_str().trim();
            if path.is_empty() || imports.iter().any(|i| i.source == path) {
                continue;
            }
            let mut entry = ImportEntry::new(path.to_string(), ImportKind::Static);
            entry.line = Some(idx + 1);
            entry.resolution = ImportResolutionKind::Unknown;
            imports.push(entry);
        }
    }
    imports
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
            SymbolEdge {
                from: from.clone(),
                to: SymbolId::from_parts(&entry.source, to_kind, &entry.source, 0),
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
}
