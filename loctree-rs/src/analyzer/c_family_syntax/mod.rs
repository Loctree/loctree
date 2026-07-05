//! C-family (Swift / Objective-C / C / C++) analyzer — Wave B tree-sitter
//! symbol extraction.
//!
//! Dispatched from `scan.rs` for `.swift/.m/.mm/.c/.cc/.cpp/.cxx/.h/.hpp`.
//! Produces a regular [`FileAnalysis`] (imports/exports/local symbols, so the
//! existing import-graph and query surfaces keep working) **plus** a per-file
//! [`SymbolGraph`] fragment stored in `FileAnalysis::symbol_fragment`. The
//! snapshot builder merges fragments into `Snapshot::symbol_graph`.
//!
//! Authority discipline (Wave A contract, `crate::symbols`): every node is
//! `SymbolProvenance::TreeSitter`, every occurrence/edge is
//! `Confidence::Heuristic`. Tier 1 never claims `Precise`.

mod includes;
mod symbols;
mod usages;

use crate::symbols::{
    FileSymbolSummary, LanguageId, SymbolEngineRun, SymbolGraph, SymbolProvenance,
};
use crate::types::{FileAnalysis, LocalSymbol};

/// Entry point used by `scan.rs::analyze_file` for all C-family extensions.
pub fn analyze_c_family_file(content: &str, relative: String, ext: &str) -> FileAnalysis {
    let lang = detect_language_id(ext, content);

    // Swift keeps its established regex analyzer for imports/exports so the
    // pre-Wave-B behavior (import graph, dead exports, twins) is unchanged.
    let mut analysis = match lang {
        LanguageId::Swift => crate::analyzer::swift::analyze_swift_file(content, relative.clone()),
        _ => {
            let mut base = FileAnalysis::new(relative.clone());
            base.imports = includes::parse_includes(content);
            base
        }
    };

    // tree-sitter-objc derails on bare declaration macros
    // (NS_ASSUME_NONNULL_BEGIN, NS_SWIFT_NAME(...), API_AVAILABLE(...)):
    // the whole @interface collapses into an ERROR declaration. Erase such
    // lines with spaces — byte offsets and line numbers stay intact.
    let source: std::borrow::Cow<'_, str> = match lang {
        LanguageId::ObjC | LanguageId::ObjCpp => {
            std::borrow::Cow::Owned(erase_bare_macro_lines(content))
        }
        _ => std::borrow::Cow::Borrowed(content),
    };

    let extraction = symbols::extract(&source, &relative, lang);

    // Non-Swift definitions surface as local symbols (not exports): C has no
    // export concept and routing them through `exports` would flood the
    // dead-export/twins machinery with false positives.
    if lang != LanguageId::Swift {
        for node in &extraction.nodes {
            analysis.local_symbols.push(LocalSymbol {
                name: node.name.clone(),
                kind: symbols::kind_label(&node.kind),
                line: node.range.map(|r| r.start_line),
                context: node.signature.clone().unwrap_or_default(),
                is_exported: false,
            });
        }
    }

    let mut fragment = SymbolGraph::new();
    fragment.edges = includes::include_edges(&relative, &analysis.imports, lang);
    fragment.occurrences = extraction.occurrences;

    if let Some(tree) = &extraction.tree {
        let defined: std::collections::HashMap<String, crate::symbols::SymbolId> = extraction
            .nodes
            .iter()
            .map(|n| (n.name.clone(), n.id.clone()))
            .collect();
        fragment.occurrences.extend(usages::collect_usages(
            tree,
            &source,
            &relative,
            &defined,
            &extraction.name_ranges,
        ));
    }

    if !extraction.nodes.is_empty() {
        fragment.file_projection.push(FileSymbolSummary {
            file: std::path::PathBuf::from(&relative),
            defined: extraction.nodes.iter().map(|n| n.id.clone()).collect(),
            referenced: Vec::new(),
        });
    }
    fragment.symbols = extraction.nodes;

    if !fragment.is_empty() {
        fragment.engines.push(SymbolEngineRun {
            engine: SymbolProvenance::TreeSitter,
            symbol_count: fragment.symbols.len(),
            occurrence_count: fragment.occurrences.len(),
            tool_version: None,
        });
        analysis.symbol_fragment = Some(fragment);
    }

    analysis
}

/// Replace lines consisting solely of a bare UPPER_SNAKE macro invocation
/// (optionally with a parenthesized argument list) with spaces of the same
/// length, preserving every byte offset and line number for the parser.
fn erase_bare_macro_lines(content: &str) -> String {
    content
        .lines()
        .map(|line| {
            let t = line.trim();
            let body = t.trim_end_matches(['(', ')', ';']);
            let is_bare_macro = t.len() >= 2
                && t.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                && !body.is_empty()
                && body
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if is_bare_macro {
                " ".repeat(line.len())
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Map a dispatch extension (plus content sniffing for ambiguous `.h`) to the
/// schema [`LanguageId`].
fn detect_language_id(ext: &str, content: &str) -> LanguageId {
    match ext {
        "swift" => LanguageId::Swift,
        "m" => LanguageId::ObjC,
        "mm" => LanguageId::ObjCpp,
        "c" => LanguageId::C,
        "cc" | "cpp" | "cxx" | "hpp" => LanguageId::Cpp,
        // `.h` is ambiguous: ObjC headers carry @-directives, C++ headers
        // carry classes/namespaces/templates; otherwise treat as C.
        _ => {
            if content.contains("@interface")
                || content.contains("@protocol")
                || content.contains("@implementation")
                || content.contains("#import")
            {
                LanguageId::ObjC
            } else if content.contains("namespace ")
                || content.contains("template<")
                || content.contains("template <")
                || content.contains("class ")
            {
                LanguageId::Cpp
            } else {
                LanguageId::C
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbols::{Confidence, SymbolKind, SymbolProvenance};

    #[test]
    fn swift_symbols_populate_fragment() {
        let src = r#"
import Foundation

public struct WorkspaceSubstrate {
    let id: String
    func materialize() -> Bool { return true }
}

protocol Searchable {}
class EditorController {}

final class WorkspaceMetadataStore {
    func closeActiveDocument() {}
}

final class FolderManager {
    func openResolvedWorkspace() {}
    func rebuildWorkspace() {}
    func scanChildren() {}
}

struct DocumentCommands: Commands {
    let store: WorkspaceMetadataStore

    var body: some Commands {
        CommandMenu("File") {
            Button("Close") {
                store.closeActiveDocument()
            }
        }
    }
}
"#;
        let analysis = analyze_c_family_file(src, "Sources/App/Substrate.swift".into(), "swift");
        let frag = analysis.symbol_fragment.expect("swift fragment");
        let names: Vec<&str> = frag.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"WorkspaceSubstrate"), "got {names:?}");
        assert!(names.contains(&"Searchable"), "got {names:?}");
        assert!(names.contains(&"EditorController"), "got {names:?}");
        assert!(names.contains(&"WorkspaceMetadataStore"), "got {names:?}");
        assert!(names.contains(&"FolderManager"), "got {names:?}");
        assert!(names.contains(&"DocumentCommands"), "got {names:?}");
        assert!(names.contains(&"materialize"), "got {names:?}");
        assert!(names.contains(&"closeActiveDocument"), "got {names:?}");
        assert!(names.contains(&"openResolvedWorkspace"), "got {names:?}");
        assert!(names.contains(&"rebuildWorkspace"), "got {names:?}");
        assert!(names.contains(&"scanChildren"), "got {names:?}");
        assert!(names.contains(&"Close"), "got {names:?}");
        assert!(
            frag.occurrences
                .iter()
                .any(|o| o.role == crate::symbols::OccurrenceRole::Call
                    && frag
                        .symbols
                        .iter()
                        .any(|s| s.id == o.symbol_id && s.name == "closeActiveDocument")),
            "expected a Call occurrence for closeActiveDocument()"
        );
        assert!(
            frag.symbols
                .iter()
                .all(|s| s.provenance == SymbolProvenance::TreeSitter)
        );
        assert!(
            frag.occurrences
                .iter()
                .all(|o| o.confidence == Confidence::Heuristic)
        );
        // Swift keeps the regex-based exports surface intact.
        assert!(
            analysis
                .exports
                .iter()
                .any(|e| e.name == "WorkspaceSubstrate")
        );
    }

    #[test]
    fn objc_interface_and_methods_extracted() {
        let src = r#"
#import <UIKit/UIKit.h>
#import "EditorModel.h"

@interface EditorViewController : UIViewController
@property (nonatomic, strong) NSString *title;
- (void)reloadDocument;
@end

@implementation EditorViewController
- (void)reloadDocument {
}
@end
"#;
        let analysis = analyze_c_family_file(src, "legacy/EditorViewController.m".into(), "m");
        let frag = analysis.symbol_fragment.expect("objc fragment");
        let names: Vec<&str> = frag.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"EditorViewController"), "got {names:?}");
        assert!(names.contains(&"title"), "got {names:?}");
        assert!(names.contains(&"reloadDocument"), "got {names:?}");
        assert!(
            !names.contains(&"nonatomic"),
            "property attributes are not symbol names: {names:?}"
        );
        assert!(
            !names.contains(&"IBAction"),
            "return-type macros are not method names: {names:?}"
        );
        // #import lines land in the import surface and as Includes edges.
        assert!(analysis.imports.iter().any(|i| i.source == "UIKit/UIKit.h"));
        assert!(analysis.imports.iter().any(|i| i.source == "EditorModel.h"));
        assert!(
            frag.edges
                .iter()
                .any(|e| e.kind == crate::symbols::SymbolEdgeKind::Includes)
        );
    }

    #[test]
    fn c_functions_structs_typedefs_extracted() {
        let src = r#"
#include <stdio.h>
#include "util.h"

typedef struct Point { int x; int y; } Point;

struct Buffer { char *data; };

static int clamp(int v) { return v; }

int main(void) {
    return clamp(0);
}
"#;
        let analysis = analyze_c_family_file(src, "src/main.c".into(), "c");
        let frag = analysis.symbol_fragment.expect("c fragment");
        let names: Vec<&str> = frag.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"main"), "got {names:?}");
        assert!(names.contains(&"clamp"), "got {names:?}");
        assert!(names.contains(&"Buffer"), "got {names:?}");
        // local refs/calls land as heuristic occurrences
        assert!(
            frag.occurrences
                .iter()
                .any(|o| o.role == crate::symbols::OccurrenceRole::Call),
            "expected a Call occurrence for clamp()"
        );
        // C defs surface as local symbols, not exports.
        assert!(analysis.local_symbols.iter().any(|l| l.name == "main"));
        assert!(analysis.exports.is_empty());
    }

    #[test]
    fn cpp_classes_and_namespaces_extracted() {
        let src = r#"
#include <vector>

namespace editor {

class Document {
public:
    void save();
};

void Document::save() {}

}  // namespace editor
"#;
        let analysis = analyze_c_family_file(src, "src/document.cpp".into(), "cpp");
        let frag = analysis.symbol_fragment.expect("cpp fragment");
        let names: Vec<(&str, &SymbolKind)> = frag
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), &s.kind))
            .collect();
        assert!(
            names
                .iter()
                .any(|(n, k)| *n == "Document" && **k == SymbolKind::Class),
            "got {names:?}"
        );
        assert!(
            names
                .iter()
                .any(|(n, k)| *n == "editor" && **k == SymbolKind::Namespace),
            "got {names:?}"
        );
    }

    #[test]
    fn objc_header_with_nullability_macros() {
        // Real-world ObjC header shape: NS_ASSUME_NONNULL_* macros around a
        // bare @interface (legacy/MarkdownEditor fixture).
        let src = r#"
#import <Cocoa/Cocoa.h>

NS_ASSUME_NONNULL_BEGIN

@interface EditorViewController : NSViewController

@end

NS_ASSUME_NONNULL_END
"#;
        let analysis = analyze_c_family_file(src, "Sources/EditorViewController.h".into(), "h");
        let frag = analysis.symbol_fragment.expect("objc header fragment");
        let names: Vec<&str> = frag.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"EditorViewController"),
            "expected @interface symbol from header, got {names:?}"
        );
    }

    #[test]
    fn ambiguous_header_detection() {
        assert_eq!(
            detect_language_id("h", "@interface Foo : NSObject\n@end"),
            LanguageId::ObjC
        );
        assert_eq!(
            detect_language_id("h", "namespace foo { class Bar; }"),
            LanguageId::Cpp
        );
        assert_eq!(
            detect_language_id("h", "int add(int a, int b);"),
            LanguageId::C
        );
    }
}
