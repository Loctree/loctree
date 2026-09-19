//! Lightweight Swift (.swift) analyzer.
//!
//! Regex-based parser that extracts public declarations (`class`, `struct`, `enum`, `protocol`, `func`, `var`, `let`, `extension`),
//! `@import` / `import` statements, and symbol usages.

use std::collections::{HashMap, HashSet};

use once_cell::sync::Lazy;
use regex::Regex;

use crate::types::{
    ExportSymbol, FileAnalysis, ImportEntry, ImportKind, ImportResolutionKind, LocalSymbol,
    SymbolUsage,
};

// Public declarations:   public final class NAME / struct NAME / func NAME / protocol NAME / extension NAME
static RE_SWIFT_DECL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^\s*(?:@objc\s*(?:\([^)]+\)\s*)?)?(?:(?:public|internal|private|fileprivate|open|final|override|static|class|mutating|nonmutating|lazy|weak|unowned)\s+)*(class|struct|enum|protocol|extension|func|var|let)\s+([A-Za-z_][A-Za-z0-9_]*)",
    )
    .expect("valid swift decl regex")
});

// `import Foundation`, `@testable import MyApp`
static RE_IMPORT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*(?:@testable\s+)?import\s+(?:class\s+|struct\s+|enum\s+|protocol\s+|func\s+|var\s+|let\s+)?([A-Za-z0-9_.]+)")
        .expect("valid swift import regex")
});

// Regex to capture potential symbol usages (CamelCase or known patterns)
static RE_WORD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b([A-Z][A-Za-z0-9_]*|[a-z][A-Za-z0-9_]*)\b").expect("valid swift word regex")
});

// Type declaration with an inheritance clause:
//   `struct ContentView: View {`, `final class Suite: XCTestCase {`,
//   `extension ContentView: View {`, `struct S<T>: Widget where T: Sendable {`
static RE_SWIFT_CONFORMANCE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^\s*(?:@[A-Za-z_][A-Za-z0-9_]*(?:\([^)]*\))?\s+)*(?:(?:public|internal|private|fileprivate|open|final|indirect)\s+)*(?:class|struct|enum|actor|extension)\s+([A-Za-z_][A-Za-z0-9_]*)(?:<[^>]*>)?\s*:\s*([^{]+)",
    )
    .expect("valid swift conformance regex")
});

pub fn analyze_swift_file(content: &str, relative: String) -> FileAnalysis {
    let mut analysis = FileAnalysis::new(relative);
    analysis.imports = parse_imports(content);
    analysis.exports = parse_exports(content);
    analysis.symbol_usages = parse_symbol_usages(content, &analysis.exports);
    analysis.local_uses = parse_local_uses(content, &analysis.exports);
    apply_runtime_dispatch_signals(content, &mut analysis);
    apply_framework_conformance_credits(content, &mut analysis);
    credit_uniffi_generated_glue(content, &mut analysis);
    analysis
}

/// Swift surfaces enumerated by tooling rather than referenced by identifier:
/// Xcode discovers `PreviewProvider` conformances and the XCTest runner
/// discovers `XCTestCase` subclasses at build/run time. Their declarations
/// carry zero in-code references while being fully live.
const SWIFT_ENUMERATED_CONFORMANCES: &[&str] = &["PreviewProvider", "XCTestCase"];

/// Protocols whose `body` requirement is invoked exclusively through SwiftUI
/// dispatch — `var body` on a conforming type is never read by identifier.
const SWIFT_BODY_REQUIREMENT_PROTOCOLS: &[&str] = &[
    "App",
    "Scene",
    "View",
    "Widget",
    "WidgetBundle",
    "ViewModifier",
    "Commands",
    "ToolbarContent",
    "Shape",
    "InsettableShape",
    "Gesture",
    "PreviewModifier",
];

/// AppKit/UIKit (and peers) delegate/datasource protocols whose requirement
/// methods are invoked by the framework dynamically — never by identifier.
///
/// W9-B / loctree-fail.md (2026-07-23): W9-A credited SwiftUI `body`/`previews`
/// and XCTest, but left NSToolbarDelegate / NSTableView* methods as dead-HIGH
/// (e.g. `toolbarDefaultItemIdentifiers` in vibecrafted). Conformance in the
/// same file gates the credit so a free-standing helper with the same name
/// stays a dead candidate.
///
/// Method names are the Swift base name the regex export parser captures
/// (`func numberOfRows(in:)` → `numberOfRows`). Shared overloads collapse to
/// one base (`func tableView(...)` for many NSTableView* requirements).
const SWIFT_PROTOCOL_DISPATCH_REQUIREMENTS: &[(&str, &[&str])] = &[
    (
        "NSToolbarDelegate",
        &[
            "toolbar",
            "toolbarDefaultItemIdentifiers",
            "toolbarAllowedItemIdentifiers",
            "toolbarSelectableItemIdentifiers",
            "toolbarWillAddItem",
            "toolbarDidRemoveItem",
        ],
    ),
    (
        "NSTableViewDataSource",
        &["numberOfRows", "tableView", "acceptDrop", "validateDrop"],
    ),
    (
        "NSTableViewDelegate",
        &[
            "tableView",
            "tableViewSelectionDidChange",
            "tableViewSelectionIsChanging",
            "tableViewColumnDidMove",
            "tableViewColumnDidResize",
            "selectionShouldChange",
        ],
    ),
    (
        "NSOutlineViewDataSource",
        &["outlineView", "acceptDrop", "validateDrop"],
    ),
    (
        "NSOutlineViewDelegate",
        &["outlineView", "outlineViewSelectionDidChange"],
    ),
    (
        "NSCollectionViewDataSource",
        &["numberOfSections", "collectionView"],
    ),
    ("NSCollectionViewDelegate", &["collectionView"]),
    (
        "NSMenuDelegate",
        &[
            "menu",
            "menuNeedsUpdate",
            "menuWillOpen",
            "menuDidClose",
            "numberOfItemsInMenu",
            "menuHasKeyEquivalent",
        ],
    ),
    (
        "NSWindowDelegate",
        &[
            "windowWillClose",
            "windowDidResize",
            "windowShouldClose",
            "windowDidBecomeKey",
            "windowDidResignKey",
            "windowDidBecomeMain",
            "windowDidResignMain",
            "windowWillMiniaturize",
            "windowDidMiniaturize",
            "windowDidDeminiaturize",
            "windowDidEnterFullScreen",
            "windowDidExitFullScreen",
            "windowWillReturnFieldEditor",
            "window",
        ],
    ),
    (
        "NSTextViewDelegate",
        &[
            "textDidChange",
            "textDidBeginEditing",
            "textDidEndEditing",
            "textView",
            "textShouldBeginEditing",
            "textShouldEndEditing",
        ],
    ),
    (
        "NSTextFieldDelegate",
        &[
            "controlTextDidChange",
            "controlTextDidBeginEditing",
            "controlTextDidEndEditing",
            "control",
        ],
    ),
    (
        "NSApplicationDelegate",
        // Overlaps SWIFT_FRAMEWORK_DISPATCH_METHODS for name-only credit;
        // protocol gate still helps methods not yet on that curated list.
        &[
            "applicationDidFinishLaunching",
            "applicationWillFinishLaunching",
            "applicationWillTerminate",
            "applicationShouldTerminateAfterLastWindowClosed",
            "applicationShouldTerminate",
            "applicationSupportsSecureRestorableState",
            "applicationDidBecomeActive",
            "applicationWillBecomeActive",
            "applicationDidResignActive",
            "applicationWillResignActive",
            "applicationDidHide",
            "applicationDidUnhide",
            "applicationShouldHandleReopen",
            "applicationDockMenu",
            "applicationOpenUntitledFile",
            "applicationShouldOpenUntitledFile",
            "applicationDidChangeScreenParameters",
            "application",
        ],
    ),
    (
        "UIApplicationDelegate",
        &[
            "application",
            "applicationDidFinishLaunching",
            "applicationWillTerminate",
            "applicationDidBecomeActive",
            "applicationWillResignActive",
            "applicationDidEnterBackground",
            "applicationWillEnterForeground",
        ],
    ),
    ("UITableViewDataSource", &["numberOfSections", "tableView"]),
    ("UITableViewDelegate", &["tableView", "scrollViewDidScroll"]),
    (
        "UICollectionViewDataSource",
        &["numberOfSections", "collectionView"],
    ),
    (
        "UICollectionViewDelegate",
        &["collectionView", "scrollViewDidScroll"],
    ),
    (
        "UIScrollViewDelegate",
        &[
            "scrollViewDidScroll",
            "scrollViewWillBeginDragging",
            "scrollViewDidEndDragging",
            "scrollViewDidEndDecelerating",
            "scrollViewDidZoom",
            "viewForZooming",
        ],
    ),
    (
        "UITextFieldDelegate",
        &[
            "textField",
            "textFieldShouldBeginEditing",
            "textFieldDidBeginEditing",
            "textFieldShouldEndEditing",
            "textFieldDidEndEditing",
            "textFieldShouldReturn",
            "textFieldShouldClear",
        ],
    ),
    (
        "UITextViewDelegate",
        &[
            "textView",
            "textViewShouldBeginEditing",
            "textViewDidBeginEditing",
            "textViewShouldEndEditing",
            "textViewDidEndEditing",
            "textViewDidChange",
            "textViewDidChangeSelection",
        ],
    ),
    ("WKNavigationDelegate", &["webView"]),
    ("WKUIDelegate", &["webView"]),
    ("WKScriptMessageHandler", &["userContentController"]),
    (
        "NSOpenSavePanelDelegate",
        &["panel", "panelSelectionDidChange"],
    ),
    ("NSSharingServicePickerDelegate", &["sharingServicePicker"]),
    (
        "NSGestureRecognizerDelegate",
        &["gestureRecognizer", "gestureRecognizerShouldBegin"],
    ),
];

/// Look up curated framework-dispatched requirement method names for a
/// protocol base name (`AppKit.NSToolbarDelegate` already stripped to
/// `NSToolbarDelegate` by the caller).
fn protocol_dispatch_requirement_methods(proto: &str) -> Option<&'static [&'static str]> {
    SWIFT_PROTOCOL_DISPATCH_REQUIREMENTS
        .iter()
        .find(|(name, _)| *name == proto)
        .map(|(_, methods)| *methods)
}

/// XCTest methods invoked by the runner, never by identifier.
fn is_xctest_dispatch_method(name: &str) -> bool {
    name.starts_with("test")
        || matches!(
            name,
            "setUp" | "setUpWithError" | "tearDown" | "tearDownWithError" | "invokeTest"
        )
}

/// True when a stripped line looks like a new declaration / `where` clause /
/// brace body rather than a multi-line inheritance-clause continuation
/// (`NSToolbarDelegate,` / `NSTableViewDataSource`).
fn looks_like_swift_decl_start(trimmed: &str) -> bool {
    if trimmed.is_empty() || trimmed == "{" || trimmed.starts_with('}') {
        return true;
    }
    if trimmed.starts_with('@') || trimmed.starts_with("where ") {
        return true;
    }
    let first = trimmed.split_whitespace().next().unwrap_or("");
    matches!(
        first,
        "import"
            | "class"
            | "struct"
            | "enum"
            | "protocol"
            | "extension"
            | "actor"
            | "func"
            | "var"
            | "let"
            | "init"
            | "deinit"
            | "subscript"
            | "typealias"
            | "associatedtype"
            | "public"
            | "internal"
            | "private"
            | "fileprivate"
            | "open"
            | "final"
            | "static"
            | "override"
            | "required"
            | "convenience"
            | "lazy"
            | "weak"
            | "unowned"
            | "mutating"
            | "nonmutating"
            | "indirect"
            | "case"
    )
}

/// Walk file lines and yield `(type_name, inheritance_clause)` pairs, folding
/// multi-line clauses such as:
///
/// ```swift
/// final class MainWindowController: NSWindowController,
///     NSToolbarDelegate, NSTableViewDataSource
/// {
/// ```
///
/// Continuation lines are absorbed until `{`, a `where` clause, or a real
/// declaration start. Line-oriented only — matching the rest of the analyzer.
fn iter_swift_conformance_clauses(content: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = content.lines().collect();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = strip_line_comment(lines[i]);
        let Some(caps) = RE_SWIFT_CONFORMANCE.captures(line) else {
            i += 1;
            continue;
        };
        let type_name = caps
            .get(1)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let mut clause = caps
            .get(2)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();

        // Inheritance still open when the opening brace is not on this line
        // (common for long AppKit delegate lists).
        if !line.contains('{') {
            let mut j = i + 1;
            while j < lines.len() {
                let cont = strip_line_comment(lines[j]).trim();
                if cont.is_empty() {
                    j += 1;
                    continue;
                }
                if cont.starts_with('{') {
                    j += 1;
                    break;
                }
                if looks_like_swift_decl_start(cont) {
                    break;
                }
                let piece = cont.trim_end_matches('{').trim().trim_end_matches(',');
                if !piece.is_empty() {
                    if !clause.is_empty() && !clause.ends_with(',') {
                        clause.push(',');
                    }
                    clause.push_str(piece);
                }
                j += 1;
                if cont.contains('{') {
                    break;
                }
            }
            i = j;
        } else {
            i += 1;
        }

        // Drop a trailing `where` constraint tail: `View where T: Equatable`.
        if let Some(idx) = clause.find(" where ") {
            clause.truncate(idx);
        } else if let Some(idx) = clause.find("\twhere ") {
            clause.truncate(idx);
        }

        out.push((type_name, clause));
    }
    out
}

/// Stored on `FileAnalysis::local_symbols` so a later snapshot-wide pass can
/// credit witnesses whose protocol is declared in a different file.
const SWIFT_PROTOCOL_REQUIREMENT_KIND: &str = "swift.protocol.requirement";
const SWIFT_PROTOCOL_PARENT_KIND: &str = "swift.protocol.parent";
const SWIFT_PROTOCOL_CONFORMANCE_KIND: &str = "swift.protocol.conformance";

struct ProtocolIndex {
    requirements: HashMap<String, Vec<String>>,
    parents: HashMap<String, Vec<String>>,
}

fn protocol_base_name(piece: &str) -> Option<&str> {
    let head = piece.split_whitespace().next()?;
    let proto = head.split('<').next().unwrap_or(head);
    let proto = proto.rsplit('.').next().unwrap_or(proto);
    (!proto.is_empty()).then_some(proto)
}

fn parse_protocol_parent_list(clause: &str) -> Vec<String> {
    let mut clause = clause.trim().to_string();
    if let Some(idx) = clause.find(" where ") {
        clause.truncate(idx);
    } else if let Some(idx) = clause.find("\twhere ") {
        clause.truncate(idx);
    }
    clause
        .split(',')
        .filter_map(protocol_base_name)
        .map(str::to_string)
        .collect()
}

fn brace_delta(text: &str) -> i32 {
    let mut depth = 0i32;
    for ch in text.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
    }
    depth
}

fn collect_protocol_requirement(line: &str, reqs: &mut Vec<String>) {
    let Some(caps) = RE_SWIFT_DECL.captures(line) else {
        return;
    };
    let keyword = caps.get(1).map(|m| m.as_str()).unwrap_or("");
    if !matches!(keyword, "func" | "var" | "let") {
        return;
    }
    if let Some(name) = caps.get(2).map(|m| m.as_str())
        && !name.is_empty()
    {
        reqs.push(name.to_string());
    }
}

/// Discover protocol requirement names (and inheritance) from `protocol P { … }`
/// declarations in this file. Framework protocols that never appear in the
/// snapshot stay on the curated fallback lists.
fn index_swift_protocols(content: &str) -> ProtocolIndex {
    let lines: Vec<&str> = content.lines().collect();
    let mut requirements: HashMap<String, Vec<String>> = HashMap::new();
    let mut parents: HashMap<String, Vec<String>> = HashMap::new();
    let mut i = 0;
    while i < lines.len() {
        let line = strip_line_comment(lines[i]);
        let Some(caps) = RE_SWIFT_DECL.captures(line) else {
            i += 1;
            continue;
        };
        if caps.get(1).map(|m| m.as_str()) != Some("protocol") {
            i += 1;
            continue;
        }
        let Some(name_m) = caps.get(2) else {
            i += 1;
            continue;
        };
        let name = name_m.as_str().to_string();
        if name.is_empty() {
            i += 1;
            continue;
        }

        let mut tail = line[name_m.end()..].to_string();
        let mut j = i;
        while !tail.contains('{') && j + 1 < lines.len() {
            let next = strip_line_comment(lines[j + 1]);
            let trimmed = next.trim();
            if trimmed.is_empty() {
                j += 1;
                continue;
            }
            if looks_like_swift_decl_start(trimmed) && !trimmed.starts_with('{') {
                break;
            }
            tail.push(' ');
            tail.push_str(trimmed);
            j += 1;
        }

        if let Some(colon) = tail.find(':') {
            let after = &tail[colon + 1..];
            let until = after.find('{').unwrap_or(after.len());
            let pars = parse_protocol_parent_list(&after[..until]);
            if !pars.is_empty() {
                parents.insert(name.clone(), pars);
            }
        }

        let mut reqs: Vec<String> = Vec::new();
        if let Some(brace_at) = tail.find('{') {
            let mut depth = 1i32;
            let remainder = &tail[brace_at + 1..];
            if depth == 1 {
                collect_protocol_requirement(remainder, &mut reqs);
            }
            depth += brace_delta(remainder);
            j += 1;
            while depth > 0 && j < lines.len() {
                let body_line = strip_line_comment(lines[j]);
                if depth == 1 {
                    collect_protocol_requirement(body_line, &mut reqs);
                }
                depth += brace_delta(body_line);
                j += 1;
            }
            i = j;
        } else {
            i += 1;
        }

        if !reqs.is_empty() {
            let mut seen = HashSet::new();
            reqs.retain(|r| seen.insert(r.clone()));
            requirements.insert(name, reqs);
        }
    }
    ProtocolIndex {
        requirements,
        parents,
    }
}

fn requirement_names_for(
    proto: &str,
    index: &ProtocolIndex,
    visiting: &mut HashSet<String>,
) -> HashSet<String> {
    if !visiting.insert(proto.to_string()) {
        return HashSet::new();
    }
    let mut out = HashSet::new();
    if let Some(reqs) = index.requirements.get(proto) {
        out.extend(reqs.iter().cloned());
    }
    if let Some(methods) = protocol_dispatch_requirement_methods(proto) {
        out.extend(methods.iter().map(|m| (*m).to_string()));
    }
    if SWIFT_BODY_REQUIREMENT_PROTOCOLS.contains(&proto) {
        out.insert("body".to_string());
    }
    if let Some(pars) = index.parents.get(proto) {
        for parent in pars {
            out.extend(requirement_names_for(parent, index, visiting));
        }
    }
    visiting.remove(proto);
    out
}

fn attach_protocol_index(
    analysis: &mut FileAnalysis,
    index: &ProtocolIndex,
    conformances: &[String],
) {
    for (proto, reqs) in &index.requirements {
        for req in reqs {
            analysis.local_symbols.push(LocalSymbol {
                name: req.clone(),
                kind: SWIFT_PROTOCOL_REQUIREMENT_KIND.to_string(),
                line: None,
                context: proto.clone(),
                is_exported: false,
            });
        }
    }
    for (proto, pars) in &index.parents {
        for parent in pars {
            analysis.local_symbols.push(LocalSymbol {
                name: parent.clone(),
                kind: SWIFT_PROTOCOL_PARENT_KIND.to_string(),
                line: None,
                context: proto.clone(),
                is_exported: false,
            });
        }
    }
    for proto in conformances {
        analysis.local_symbols.push(LocalSymbol {
            name: proto.clone(),
            kind: SWIFT_PROTOCOL_CONFORMANCE_KIND.to_string(),
            line: None,
            context: String::new(),
            is_exported: false,
        });
    }
}

/// Snapshot-wide protocol-witness credits: `(file path → requirement names)`.
///
/// Same-file credit already lands in `local_uses`. This pass covers the
/// remaining G-SWIFT-DEAD-FP shape: protocol `P` declared in one file,
/// `struct S: P` / `extension S: P` implementing `P`'s requirements in
/// another. Curated AppKit/SwiftUI lists remain a fallback for protocols
/// that never appear in the snapshot.
pub(crate) fn protocol_witness_credits(
    analyses: &[FileAnalysis],
) -> HashMap<String, HashSet<String>> {
    let mut requirements: HashMap<String, Vec<String>> = HashMap::new();
    let mut parents: HashMap<String, Vec<String>> = HashMap::new();
    for analysis in analyses {
        if !analysis.path.ends_with(".swift") {
            continue;
        }
        for sym in &analysis.local_symbols {
            match sym.kind.as_str() {
                SWIFT_PROTOCOL_REQUIREMENT_KIND if !sym.context.is_empty() => {
                    requirements
                        .entry(sym.context.clone())
                        .or_default()
                        .push(sym.name.clone());
                }
                SWIFT_PROTOCOL_PARENT_KIND if !sym.context.is_empty() => {
                    parents
                        .entry(sym.context.clone())
                        .or_default()
                        .push(sym.name.clone());
                }
                _ => {}
            }
        }
    }
    let index = ProtocolIndex {
        requirements,
        parents,
    };

    let mut credits: HashMap<String, HashSet<String>> = HashMap::new();
    for analysis in analyses {
        if !analysis.path.ends_with(".swift") {
            continue;
        }
        let mut methods = HashSet::new();
        for sym in &analysis.local_symbols {
            if sym.kind == SWIFT_PROTOCOL_CONFORMANCE_KIND {
                let mut visiting = HashSet::new();
                methods.extend(requirement_names_for(&sym.name, &index, &mut visiting));
            }
        }
        if methods.is_empty() {
            continue;
        }
        let credited: HashSet<String> = analysis
            .exports
            .iter()
            .filter(|exp| {
                matches!(exp.kind.as_str(), "func" | "var" | "let") && methods.contains(&exp.name)
            })
            .map(|exp| exp.name.clone())
            .collect();
        if !credited.is_empty() {
            credits.insert(analysis.path.clone(), credited);
        }
    }

    // A protocol requirement declaration is live API once any snapshot file
    // conforms to it — otherwise `protocol P { func f() }` looks unused
    // even while `struct S: P { func f() }` is credited as a witness.
    let conformed: HashSet<String> = analyses
        .iter()
        .flat_map(|analysis| {
            analysis
                .local_symbols
                .iter()
                .filter(|sym| sym.kind == SWIFT_PROTOCOL_CONFORMANCE_KIND)
                .map(|sym| sym.name.clone())
        })
        .collect();
    for analysis in analyses {
        if !analysis.path.ends_with(".swift") {
            continue;
        }
        let mut methods = HashSet::new();
        let mut visiting_protos: HashSet<String> = HashSet::new();
        for sym in &analysis.local_symbols {
            if sym.kind == SWIFT_PROTOCOL_REQUIREMENT_KIND
                && conformed.contains(&sym.context)
                && visiting_protos.insert(sym.context.clone())
            {
                let mut visiting = HashSet::new();
                methods.extend(requirement_names_for(&sym.context, &index, &mut visiting));
            }
        }
        if methods.is_empty() {
            continue;
        }
        let extra: HashSet<String> = analysis
            .exports
            .iter()
            .filter(|exp| {
                matches!(exp.kind.as_str(), "func" | "var" | "let") && methods.contains(&exp.name)
            })
            .map(|exp| exp.name.clone())
            .collect();
        if !extra.is_empty() {
            credits
                .entry(analysis.path.clone())
                .or_default()
                .extend(extra);
        }
    }
    credits
}

/// Credit framework-reached declarations the identifier scan cannot see
/// (LCT-G01 class G false-dead family, W9-A + W9-B + W3-05):
/// - a type conforming to [`SWIFT_ENUMERATED_CONFORMANCES`] plus its
///   requirement members (`previews`, `test*`/lifecycle methods),
/// - `var body` when the file declares a conformance to a
///   [`SWIFT_BODY_REQUIREMENT_PROTOCOLS`] protocol (directly or via
///   `extension`),
/// - `func`/`var`/`let` members matching requirements of protocols
///   declared in the snapshot (W3-05) **or** curated AppKit/UIKit
///   delegate/datasource lists ([`SWIFT_PROTOCOL_DISPATCH_REQUIREMENTS`])
///   as fallback when the protocol is outside the snapshot.
///
/// Gating is file-level, matching the analyzer's line-oriented shape: a
/// `body`/`previews`/delegate-requirement declaration is only credited when
/// the same file carries the triggering conformance, so unrelated symbols of
/// the same name in plain files stay detectable. Multi-line inheritance
/// clauses are folded (W9-B follow-up) so real AppKit controller headers are
/// not half-blind. Crediting can only REMOVE a dead flag, never add one.
fn apply_framework_conformance_credits(content: &str, analysis: &mut FileAnalysis) {
    let index = index_swift_protocols(content);
    let mut credited: Vec<String> = Vec::new();
    let mut has_body_protocol = false;
    let mut has_preview = false;
    let mut has_xctest = false;
    let mut protocol_dispatch_methods: HashSet<String> = HashSet::new();
    let mut conformance_protos: Vec<String> = Vec::new();
    let mut seen_conformances: HashSet<String> = HashSet::new();

    for (type_name, clause) in iter_swift_conformance_clauses(content) {
        for piece in clause.split(',') {
            // `View where T: Equatable` → `View`; `SwiftUI.App` → `App`.
            let Some(proto) = protocol_base_name(piece) else {
                continue;
            };
            if seen_conformances.insert(proto.to_string()) {
                conformance_protos.push(proto.to_string());
            }
            if SWIFT_ENUMERATED_CONFORMANCES.contains(&proto) {
                if proto == "PreviewProvider" {
                    has_preview = true;
                }
                if proto == "XCTestCase" {
                    has_xctest = true;
                }
                if !type_name.is_empty() {
                    credited.push(type_name.clone());
                }
            }
            if SWIFT_BODY_REQUIREMENT_PROTOCOLS.contains(&proto) {
                has_body_protocol = true;
            }
            let mut visiting = HashSet::new();
            protocol_dispatch_methods.extend(requirement_names_for(proto, &index, &mut visiting));
        }
    }

    attach_protocol_index(analysis, &index, &conformance_protos);

    if !(has_body_protocol || has_preview || has_xctest || !protocol_dispatch_methods.is_empty()) {
        return;
    }

    for exp in &analysis.exports {
        let name = exp.name.as_str();
        let dispatched = match exp.kind.as_str() {
            "var" | "let" => {
                (has_body_protocol && name == "body")
                    || (has_preview && name == "previews")
                    || protocol_dispatch_methods.contains(name)
            }
            "func" => {
                (has_xctest && is_xctest_dispatch_method(name))
                    || protocol_dispatch_methods.contains(name)
            }
            _ => false,
        };
        if dispatched {
            credited.push(name.to_string());
        }
    }

    for name in credited {
        if !analysis.local_uses.iter().any(|u| u == &name) {
            analysis.local_uses.push(name);
        }
    }
}

/// A UniFFI-generated Swift bridge file is saturated with `FfiConverter*` glue;
/// a hand-written file almost never carries more than one or two. Two
/// independent corroborating signals (this density AND the autogenerated header)
/// keep the detection precise so a hand-written `*_ffi.swift` is not fenced.
const UNIFFI_FFICONVERTER_DENSITY_THRESHOLD: usize = 4;

/// Recognize a UniFFI-generated Swift bridge file and credit ALL of its exports
/// as `local_uses` so they are not flagged HIGH-confidence dead.
///
/// loctree-fail.md (2026-06-26): `loct dead --full` flagged ~50 symbols in
/// `*_ffi.swift` (`FfiConverterType*_lift/_lower`, `UNIFFI_CALLBACK_*`,
/// `uniffiTraitInterface*`, …) as HIGH dead. These are machine-written FFI glue
/// whose ONLY consumers live across the FFI boundary (C / Rust) — exactly the
/// `pub extern "C" fn` blind spot, on the Swift side. They have 0 in-Swift
/// references yet are 100% live.
///
/// Detection is deliberately narrow (the prior hak warned against overreaching
/// generated-file detection): the canonical UniFFI autogenerated header OR a
/// high `FfiConverter` density. Crediting can only REMOVE a dead flag, never add
/// one, and only fires on a recognized generated bridge, so genuine dead code in
/// a hand-written file stays detectable.
fn credit_uniffi_generated_glue(content: &str, analysis: &mut FileAnalysis) {
    if !is_uniffi_generated_bridge(content) {
        return;
    }
    let existing: std::collections::HashSet<&str> =
        analysis.local_uses.iter().map(|u| u.as_str()).collect();
    let credited: Vec<String> = analysis
        .exports
        .iter()
        .map(|e| e.name.clone())
        .filter(|name| !existing.contains(name.as_str()))
        .collect();
    // Dedup credited names (an export may appear twice with different kinds).
    let mut seen = std::collections::HashSet::new();
    for name in credited {
        if seen.insert(name.clone()) {
            analysis.local_uses.push(name);
        }
    }
}

/// True when the content looks like a UniFFI-generated Swift binding:
/// 1. the autogenerated header (`autogenerated` + `hand-written` in the head), or
/// 2. `FfiConverter` density at/above [`UNIFFI_FFICONVERTER_DENSITY_THRESHOLD`].
fn is_uniffi_generated_bridge(content: &str) -> bool {
    let head: String = content
        .lines()
        .take(12)
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    let has_autogen_header = head.contains("autogenerated") && head.contains("hand-written");
    if has_autogen_header {
        return true;
    }
    content.matches("FfiConverter").count() >= UNIFFI_FFICONVERTER_DENSITY_THRESHOLD
}

/// AppKit/UIKit lifecycle methods invoked by the framework, never "called by
/// identifier" in user code. They conform to NSApplicationDelegate /
/// UIApplicationDelegate / scene protocols, so an import-graph dead scan sees
/// 0 references and (before this) flagged them HIGH-confidence dead
/// (loctree-fail.md, 2026-06-16). Curated, not exhaustive — `override` and
/// `@objc` cover the rest.
const SWIFT_FRAMEWORK_DISPATCH_METHODS: &[&str] = &[
    // NSApplicationDelegate
    "applicationDidFinishLaunching",
    "applicationWillFinishLaunching",
    "applicationWillTerminate",
    "applicationShouldTerminateAfterLastWindowClosed",
    "applicationShouldTerminate",
    "applicationSupportsSecureRestorableState",
    "applicationDidBecomeActive",
    "applicationWillBecomeActive",
    "applicationDidResignActive",
    "applicationWillResignActive",
    "applicationDidHide",
    "applicationDidUnhide",
    "applicationShouldHandleReopen",
    "applicationDockMenu",
    "applicationOpenUntitledFile",
    "applicationShouldOpenUntitledFile",
    "applicationDidChangeScreenParameters",
    // UIApplicationDelegate / scene lifecycle (method base name)
    "application",
    "sceneDidDisconnect",
    "sceneWillEnterForeground",
    "sceneDidEnterBackground",
    "sceneWillResignActive",
    "sceneDidBecomeActive",
    // NSObject KVO / NSWindowDelegate (common)
    "observeValue",
    "windowWillClose",
    "windowDidResize",
    "windowShouldClose",
];

/// Detect Swift runtime reachability the import graph cannot see:
/// 1. Entry points — `@main` / `@NSApplicationMain` / `@UIApplicationMain`
///    attributes, or a top-level `NSApplicationMain(` / `UIApplicationMain(`
///    call — recorded in `entry_points` so the dead pipeline fences the file.
/// 2. Framework-dispatched methods — `override func`, `@objc func`, and known
///    AppKit/UIKit lifecycle methods — credited into `local_uses` so they are
///    not false dead. Crediting can only REMOVE a dead flag, never add one, so
///    genuine dead code stays detectable.
fn apply_runtime_dispatch_signals(content: &str, analysis: &mut FileAnalysis) {
    let mut is_entry = false;
    // `@objc` can sit on the line above the `func`; carry it forward one decl.
    let mut pending_objc = false;

    for raw_line in content.lines() {
        let line = strip_line_comment(raw_line);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Entry-point attributes / top-level executable bootstrap.
        if trimmed.starts_with("@main")
            || trimmed.starts_with("@NSApplicationMain")
            || trimmed.starts_with("@UIApplicationMain")
            || trimmed.contains("NSApplicationMain(")
            || trimmed.contains("UIApplicationMain(")
        {
            is_entry = true;
        }

        if trimmed.starts_with("@objc") {
            pending_objc = true;
        }

        // Framework-dispatched method crediting.
        if let Some(caps) = RE_SWIFT_DECL.captures(line) {
            let is_func = caps.get(1).map(|m| m.as_str()) == Some("func");
            if is_func && let Some(name) = caps.get(2).map(|m| m.as_str()) {
                let has_override = trimmed.contains("override ");
                let has_objc = trimmed.contains("@objc") || pending_objc;
                let is_lifecycle = SWIFT_FRAMEWORK_DISPATCH_METHODS.contains(&name);
                if (has_override || has_objc || is_lifecycle)
                    && !analysis.local_uses.iter().any(|u| u == name)
                {
                    analysis.local_uses.push(name.to_string());
                }
            }
            // A declaration consumes any pending attribute carry.
            pending_objc = false;
        }
    }

    if is_entry && !analysis.entry_points.iter().any(|e| e == "swift-main") {
        analysis.entry_points.push("swift-main".to_string());
    }
}

/// Same-file uses of this file's OWN declarations. `parse_symbol_usages`
/// deliberately drops own-export names, so without this a Swift symbol read
/// only within its defining file (a property used by a sibling method, a
/// file-private helper) looks unused. We credit an export name that appears as
/// an identifier on any line OTHER than its declaration line — the declaration
/// occurrence itself never counts as a use. Dead detection consumes this via
/// `local_uses` exactly like the Go/Dart analyzers do.
fn parse_local_uses(content: &str, exports: &[ExportSymbol]) -> Vec<String> {
    use std::collections::{HashMap, HashSet};

    if exports.is_empty() {
        return Vec::new();
    }

    // export name -> the line(s) it is declared on (1-based); used to skip the
    // declaration occurrence so a never-referenced symbol stays a candidate.
    let mut decl_lines: HashMap<&str, HashSet<usize>> = HashMap::new();
    for e in exports {
        if let Some(line) = e.line {
            decl_lines.entry(e.name.as_str()).or_default().insert(line);
        }
    }
    let export_names: HashSet<&str> = exports.iter().map(|e| e.name.as_str()).collect();

    let mut used: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (idx, line) in content.lines().enumerate() {
        let effective = strip_line_comment(line);
        if effective.trim().is_empty() {
            continue;
        }
        let lineno = idx + 1;
        for caps in RE_WORD.captures_iter(effective) {
            let Some(m) = caps.get(1) else { continue };
            let word = m.as_str();
            if !export_names.contains(word) {
                continue;
            }
            // Skip the declaration line for this exact symbol.
            if decl_lines
                .get(word)
                .is_some_and(|lines| lines.contains(&lineno))
            {
                continue;
            }
            if seen.insert(word.to_string()) {
                used.push(word.to_string());
            }
        }
    }
    used
}

fn parse_imports(content: &str) -> Vec<ImportEntry> {
    let mut imports: Vec<ImportEntry> = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let effective = strip_line_comment(line);
        if let Some(caps) = RE_IMPORT.captures(effective)
            && let Some(m) = caps.get(1)
        {
            let path = m.as_str().trim();
            if path.is_empty() {
                continue;
            }
            if imports.iter().any(|i| i.source == path) {
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

fn parse_exports(content: &str) -> Vec<ExportSymbol> {
    let mut out: Vec<ExportSymbol> = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let effective = strip_line_comment(line);
        if let Some(caps) = RE_SWIFT_DECL.captures(effective) {
            let keyword = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            if !out.iter().any(|e| e.name == name && e.kind == keyword) {
                out.push(ExportSymbol::new(name, keyword, "named", Some(idx + 1)));
            }
        }
    }
    out
}

fn parse_symbol_usages(content: &str, exports: &[ExportSymbol]) -> Vec<SymbolUsage> {
    let mut out: Vec<SymbolUsage> = Vec::new();
    let export_names: std::collections::HashSet<&str> =
        exports.iter().map(|e| e.name.as_str()).collect();

    for (idx, line) in content.lines().enumerate() {
        let effective = strip_line_comment(line);
        if effective.trim().is_empty() {
            continue;
        }
        for caps in RE_WORD.captures_iter(effective) {
            if let Some(m) = caps.get(1) {
                let word = m.as_str();
                // Avoid self-references (exports) or basic keywords
                if word.is_empty() || is_swift_keyword(word) || export_names.contains(word) {
                    continue;
                }
                // Cap to a reasonable number to avoid noise
                if out.len() >= 1500 {
                    return out;
                }
                out.push(SymbolUsage {
                    name: word.to_string(),
                    line: idx + 1,
                    context: effective.trim().to_string(),
                });
            }
        }
    }
    // Deduplicate symbol usages
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.line.cmp(&b.line)));
    out.dedup_by(|a, b| a.name == b.name && a.line == b.line);
    out
}

fn strip_line_comment(line: &str) -> &str {
    let mut in_str = false;
    let bytes = line.as_bytes();
    let mut idx = 0;
    while idx + 1 < bytes.len() {
        let ch = bytes[idx] as char;
        match ch {
            '\\' => {
                idx += 2;
                continue;
            }
            '"' => in_str = !in_str,
            '/' if !in_str && bytes[idx + 1] == b'/' => {
                return &line[..idx];
            }
            _ => {}
        }
        idx += 1;
    }
    line
}

/// Type names a bare Swift identifier resolves to in the standard library,
/// Foundation, concurrency runtime, or SwiftUI — BEFORE any same-module type
/// of the same name. Repos routinely declare nested `enum Error`, `struct
/// State`, etc.; a bare cross-file usage of such a name can only mean the
/// stdlib/framework symbol (the nested one needs `Outer.Error` qualification),
/// so an `implicit_symbol` edge on these names is categorically noise.
/// Evidence: blinksh/blink — nested `enum Error` in `SSHDefaultAgent.swift`
/// turned 75 unrelated files into "importers" (loctree-fail.md 2026-07-25).
pub fn is_swift_shadowed_stdlib_name(word: &str) -> bool {
    matches!(
        word,
        // Core language / stdlib
        "Error"
            | "Result"
            | "Optional"
            | "Any"
            | "AnyObject"
            | "Void"
            | "Never"
            | "String"
            | "Substring"
            | "Character"
            | "Bool"
            | "Int"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Float"
            | "Double"
            | "CGFloat"
            | "Array"
            | "Dictionary"
            | "Set"
            // Common conformance protocols
            | "Codable"
            | "Encodable"
            | "Decodable"
            | "Equatable"
            | "Hashable"
            | "Comparable"
            | "Identifiable"
            | "Sendable"
            | "CaseIterable"
            | "RawRepresentable"
            | "CustomStringConvertible"
            // Foundation
            | "Data"
            | "Date"
            | "Calendar"
            | "TimeZone"
            | "Locale"
            | "URL"
            | "URLRequest"
            | "UUID"
            | "Decimal"
            | "IndexPath"
            | "IndexSet"
            | "Notification"
            | "NotificationCenter"
            | "Bundle"
            | "FileManager"
            | "UserDefaults"
            | "JSONEncoder"
            | "JSONDecoder"
            | "Timer"
            | "Thread"
            | "DispatchQueue"
            | "OperationQueue"
            // Concurrency
            | "Task"
            | "MainActor"
            // SwiftUI ubiquitous surfaces
            | "State"
            | "Binding"
            | "Published"
            | "ObservedObject"
            | "ObservableObject"
            | "EnvironmentObject"
            | "Environment"
            | "View"
            | "Text"
            | "Image"
            | "Color"
            | "Font"
            | "Button"
            | "Label"
    )
}

fn is_swift_keyword(word: &str) -> bool {
    matches!(
        word,
        "import"
            | "struct"
            | "class"
            | "enum"
            | "protocol"
            | "extension"
            | "func"
            | "var"
            | "let"
            | "public"
            | "internal"
            | "private"
            | "fileprivate"
            | "open"
            | "final"
            | "override"
            | "static"
            | "mutating"
            | "nonmutating"
            | "lazy"
            | "weak"
            | "unowned"
            | "if"
            | "else"
            | "guard"
            | "switch"
            | "case"
            | "default"
            | "for"
            | "in"
            | "while"
            | "repeat"
            | "do"
            | "catch"
            | "throw"
            | "throws"
            | "rethrows"
            | "try"
            | "return"
            | "break"
            | "continue"
            | "fallthrough"
            | "defer"
            | "true"
            | "false"
            | "nil"
            | "self"
            | "super"
            | "init"
            | "deinit"
            | "subscript"
            | "typealias"
            | "associatedtype"
            | "String"
            | "Int"
            | "Bool"
            | "Double"
            | "Float"
            | "Optional"
            | "Array"
            | "Dictionary"
            | "Set"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_swift_decls() {
        let src = r#"
import Foundation

public final class WorkspaceCacheStore {
    let id: String
    private var data: [String: Any]
    
    init() {}
}

struct DocumentRecord {}
protocol Searchable {}
extension WorkspaceCacheStore: Searchable {}
"#;
        let analysis = analyze_swift_file(src, "main.swift".to_string());

        let classes: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "class")
            .map(|e| e.name.clone())
            .collect();
        assert!(classes.contains(&"WorkspaceCacheStore".to_string()));

        let structs: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "struct")
            .map(|e| e.name.clone())
            .collect();
        assert!(structs.contains(&"DocumentRecord".to_string()));

        let extensions: Vec<_> = analysis
            .exports
            .iter()
            .filter(|e| e.kind == "extension")
            .map(|e| e.name.clone())
            .collect();
        assert!(extensions.contains(&"WorkspaceCacheStore".to_string()));
    }

    #[test]
    fn marks_nsapplicationmain_file_as_entry_point() {
        // loctree-fail.md (2026-06-16): main.swift drives the app via
        // NSApplicationMain; it is a runtime entry point, not dead.
        let src = "import AppKit\n\nlet delegate = AppDelegate()\nNSApplication.shared.delegate = delegate\n_ = NSApplicationMain(CommandLine.argc, CommandLine.unsafeArgv)\n";
        let analysis = analyze_swift_file(src, "main.swift".to_string());
        assert!(
            !analysis.entry_points.is_empty(),
            "a file calling NSApplicationMain must be a runtime entry point"
        );
    }

    #[test]
    fn marks_main_attribute_file_as_entry_point() {
        let src = "import SwiftUI\n\n@main\nstruct MyApp: App {\n    var body: some Scene { WindowGroup {} }\n}\n";
        let analysis = analyze_swift_file(src, "MyApp.swift".to_string());
        assert!(
            !analysis.entry_points.is_empty(),
            "a @main type must mark its file as an entry point"
        );
    }

    #[test]
    fn credits_appkit_lifecycle_and_override_methods_as_used() {
        // loctree-fail.md (2026-06-16): NSApplicationDelegate protocol methods
        // and `override`/`@objc` self-dispatched helpers are framework-invoked,
        // never "called by identifier" — they were FALSE HIGH-confidence dead.
        // Crediting them as local_uses removes the FP (can only ADD a use,
        // never mask genuine dead code).
        let src = "import AppKit\n\n@MainActor\nclass AppDelegate: NSObject, NSApplicationDelegate {\n    func applicationDidFinishLaunching(_ notification: Notification) {}\n    func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool { true }\n    override func observeValue(forKeyPath keyPath: String?) {}\n    @objc func handleClick() {}\n    private func internalHelper() {}\n}\n";
        let analysis = analyze_swift_file(src, "AppDelegate.swift".to_string());

        for invoked in [
            "applicationDidFinishLaunching",
            "applicationSupportsSecureRestorableState",
            "observeValue",
            "handleClick",
        ] {
            assert!(
                analysis.local_uses.iter().any(|u| u == invoked),
                "framework-dispatched method `{invoked}` must be credited as used"
            );
        }
        // A plain private helper with no dispatch signal is NOT auto-credited
        // (so genuine dead code is still detectable).
        assert!(
            !analysis.local_uses.iter().any(|u| u == "internalHelper"),
            "plain private helper must not be force-credited"
        );
    }

    #[test]
    fn credits_uniffi_generated_glue_via_header() {
        // loctree-fail.md (2026-06-26): UniFFI-generated bridge glue is reached
        // only across the FFI boundary (C/Rust), has 0 in-Swift references, and
        // was flagged HIGH-confidence dead. The autogenerated header marks the
        // whole file as generated → every export is credited as used.
        let src = "// This file was autogenerated by some hand-written code.\n// Trust me, you don't want to mess with it!\nimport Foundation\n\npublic func uniffiTraitInterfaceCallWithError() {}\npublic let UNIFFI_CALLBACK_SUCCESS = 0\npublic struct FfiConverterTypeFoo {}\n";
        let analysis = analyze_swift_file(src, "vibecrafted_shell_ffi.swift".to_string());
        for credited in [
            "uniffiTraitInterfaceCallWithError",
            "UNIFFI_CALLBACK_SUCCESS",
            "FfiConverterTypeFoo",
        ] {
            assert!(
                analysis.local_uses.iter().any(|u| u == credited),
                "UniFFI generated symbol `{credited}` must be credited as used"
            );
        }
    }

    #[test]
    fn credits_uniffi_generated_glue_via_ffi_converter_density() {
        // Header stripped, but FfiConverter density alone is a precise UniFFI
        // signature (hand-written code virtually never reaches the threshold).
        let src = "import Foundation\n\npublic struct FfiConverterUInt8 {}\npublic struct FfiConverterString {}\npublic struct FfiConverterData {}\npublic struct FfiConverterBool {}\npublic func lift() {}\n";
        let analysis = analyze_swift_file(src, "bindings.swift".to_string());
        assert!(
            analysis.local_uses.iter().any(|u| u == "lift"),
            "a high-FfiConverter-density file must credit its exports as used"
        );
    }

    #[test]
    fn does_not_credit_hand_written_ffi_named_file() {
        // A hand-written file that merely sits at *_ffi.swift, with no UniFFI
        // header and no FfiConverter density, must NOT be fenced — genuine dead
        // code stays detectable (no overreaching generated-file detection).
        let src = "import Foundation\n\npublic func myHandWrittenHelper() {}\npublic struct FfiBridge {}\n";
        let analysis = analyze_swift_file(src, "custom_ffi.swift".to_string());
        assert!(
            !analysis
                .local_uses
                .iter()
                .any(|u| u == "myHandWrittenHelper"),
            "hand-written *_ffi.swift must not be force-credited as generated"
        );
    }

    #[test]
    fn credits_preview_provider_type_and_previews_requirement() {
        // LCT-G01 family (W9-A): Xcode enumerates PreviewProvider
        // conformances; neither the type nor its `previews` requirement is
        // ever referenced by identifier, yet both are live.
        let src = "import SwiftUI\n\nstruct ContentView_Previews: PreviewProvider {\n    static var previews: some View {\n        ContentView()\n    }\n}\n";
        let analysis = analyze_swift_file(src, "Previews.swift".to_string());
        for credited in ["ContentView_Previews", "previews"] {
            assert!(
                analysis.local_uses.iter().any(|u| u == credited),
                "preview surface `{credited}` must be credited as framework-reached"
            );
        }
    }

    #[test]
    fn credits_body_requirement_on_swiftui_conformance() {
        // `var body` on a View/App/Scene conformer is invoked only through
        // SwiftUI dispatch — never by identifier.
        let src = "import SwiftUI\n\nstruct ContentView: View {\n    var body: some View {\n        Text(\"hello\")\n    }\n}\n";
        let analysis = analyze_swift_file(src, "ContentView.swift".to_string());
        assert!(
            analysis.local_uses.iter().any(|u| u == "body"),
            "protocol-requirement `body` must be credited as framework-dispatched"
        );
        // The conforming type itself is NOT auto-credited: a genuinely
        // unreferenced View must stay a dead candidate.
        assert!(
            !analysis.local_uses.iter().any(|u| u == "ContentView"),
            "a View conformer must not be force-credited by conformance alone"
        );
    }

    #[test]
    fn does_not_credit_body_without_framework_conformance() {
        // A plain type with a `body` property has no SwiftUI dispatch —
        // genuine dead code stays detectable.
        let src = "import Foundation\n\nstruct Message {\n    var body: String\n}\n";
        let analysis = analyze_swift_file(src, "Message.swift".to_string());
        assert!(
            !analysis.local_uses.iter().any(|u| u == "body"),
            "`body` outside a framework conformance must not be credited"
        );
    }

    #[test]
    fn credits_xctest_subclass_and_runner_dispatched_methods() {
        // The XCTest runner discovers XCTestCase subclasses and invokes
        // test*/lifecycle methods reflectively.
        let src = "import XCTest\n\nfinal class FixtureTests: XCTestCase {\n    override func setUpWithError() throws {}\n    func testDormantFeature() {}\n    func helperNotATest() -> Int { 7 }\n}\n";
        let analysis = analyze_swift_file(src, "FixtureTests.swift".to_string());
        for credited in ["FixtureTests", "testDormantFeature", "setUpWithError"] {
            assert!(
                analysis.local_uses.iter().any(|u| u == credited),
                "XCTest surface `{credited}` must be credited as runner-reached"
            );
        }
        assert!(
            !analysis.local_uses.iter().any(|u| u == "helperNotATest"),
            "a non-test helper in a test class must not be force-credited"
        );
    }

    #[test]
    fn credits_body_via_extension_conformance() {
        // Conformance declared through an extension still makes `body` a
        // framework-dispatched requirement in this file.
        let src = "import SwiftUI\n\nstruct Panel {\n    var body: some View {\n        Text(\"panel\")\n    }\n}\n\nextension Panel: View {}\n";
        let analysis = analyze_swift_file(src, "Panel.swift".to_string());
        assert!(
            analysis.local_uses.iter().any(|u| u == "body"),
            "`body` must be credited when conformance arrives via extension"
        );
    }

    #[test]
    fn credits_appkit_toolbar_and_table_delegate_requirements() {
        // W9-B / loctree-fail.md (2026-07-23): NSToolbarDelegate /
        // NSTableViewDataSource requirements are AppKit-dispatched with zero
        // identifier references. Without protocol-gated credit they land as
        // dead-HIGH (vibecrafted MainWindowController).
        let src = r#"
import AppKit

final class MainWindowController: NSWindowController, NSToolbarDelegate, NSTableViewDataSource, NSTableViewDelegate {
    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] { [] }
    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] { [] }
    func numberOfRows(in tableView: NSTableView) -> Int { 0 }
    func tableViewSelectionDidChange(_ notification: Notification) {}
    private func layoutHelper() {}
}
"#;
        let analysis = analyze_swift_file(src, "MainWindowController.swift".to_string());
        for credited in [
            "toolbarDefaultItemIdentifiers",
            "toolbarAllowedItemIdentifiers",
            "numberOfRows",
            "tableViewSelectionDidChange",
        ] {
            assert!(
                analysis.local_uses.iter().any(|u| u == credited),
                "AppKit delegate requirement `{credited}` must be credited as framework-dispatched"
            );
        }
        assert!(
            !analysis.local_uses.iter().any(|u| u == "layoutHelper"),
            "a plain helper on a delegate type must not be force-credited"
        );
        // Conformance alone must not credit the type (same rule as View).
        assert!(
            !analysis
                .local_uses
                .iter()
                .any(|u| u == "MainWindowController"),
            "a delegate conformer must not be force-credited by conformance alone"
        );
    }

    #[test]
    fn credits_delegate_requirements_via_extension_conformance() {
        let src = r#"
import AppKit

final class PaletteController: NSObject {
    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] { [] }
    func orphanCallback() {}
}

extension PaletteController: NSToolbarDelegate {}
"#;
        let analysis = analyze_swift_file(src, "PaletteController.swift".to_string());
        assert!(
            analysis
                .local_uses
                .iter()
                .any(|u| u == "toolbarDefaultItemIdentifiers"),
            "delegate requirement must be credited when conformance arrives via extension"
        );
        assert!(
            !analysis.local_uses.iter().any(|u| u == "orphanCallback"),
            "non-requirement methods must stay uncredited without other signals"
        );
    }

    #[test]
    fn does_not_credit_delegate_method_names_without_protocol() {
        // Same method names outside a curated protocol conformance are not
        // framework-dispatched and must stay dead candidates.
        let src = r#"
import Foundation

struct ToolbarCatalog {
    func toolbarDefaultItemIdentifiers() -> [String] { [] }
    func numberOfRows() -> Int { 0 }
}
"#;
        let analysis = analyze_swift_file(src, "ToolbarCatalog.swift".to_string());
        for name in ["toolbarDefaultItemIdentifiers", "numberOfRows"] {
            assert!(
                !analysis.local_uses.iter().any(|u| u == name),
                "`{name}` without AppKit delegate conformance must not be credited"
            );
        }
    }

    #[test]
    fn credits_delegate_requirements_across_multiline_conformance() {
        // Real AppKit controllers often wrap long conformance lists. The
        // trailing protocols (NSTableViewDelegate here) must still gate credit.
        let src = r#"
import AppKit

final class MainWindowController: NSWindowController,
    NSToolbarDelegate,
    NSTableViewDataSource,
    NSTableViewDelegate
{
    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] { [] }
    func numberOfRows(in tableView: NSTableView) -> Int { 0 }
    func tableViewSelectionDidChange(_ notification: Notification) {}
    private func layoutHelper() {}
}
"#;
        let analysis = analyze_swift_file(src, "MainWindowController.swift".to_string());
        for credited in [
            "toolbarDefaultItemIdentifiers",
            "numberOfRows",
            "tableViewSelectionDidChange",
        ] {
            assert!(
                analysis.local_uses.iter().any(|u| u == credited),
                "multi-line conformance must still credit `{credited}`"
            );
        }
        assert!(
            !analysis.local_uses.iter().any(|u| u == "layoutHelper"),
            "helpers must stay uncredited under multi-line conformance"
        );
    }

    #[test]
    fn parses_swift_imports() {
        let src = r#"
import Foundation
@testable import MyApp
import struct Module.MyStruct
"#;
        let analysis = analyze_swift_file(src, "main.swift".to_string());
        let imports: Vec<_> = analysis.imports.iter().map(|i| i.source.clone()).collect();
        assert!(imports.contains(&"Foundation".to_string()));
        assert!(imports.contains(&"MyApp".to_string()));
        assert!(imports.contains(&"Module.MyStruct".to_string()));
    }

    #[test]
    fn w3_05_swift_protocol_witness_not_dead() {
        // Custom protocol is not on the curated AppKit/SwiftUI lists.
        // Witness lives in a different file so same-file identifier
        // coincidence (parse_local_uses seeing the impl as a "use" of the
        // requirement name) cannot mask a missing conformance credit.
        let proto = analyze_swift_file(
            r#"
protocol PayloadMatching {
    func matchesPayload(_ raw: String) -> Bool
    var glyph: String { get }
}
"#,
            "Sources/App/PayloadMatching.swift".to_string(),
        );
        let impls = analyze_swift_file(
            r#"
struct CarrierGuard: PayloadMatching {
    func matchesPayload(_ raw: String) -> Bool { true }
    var glyph: String { "x" }
    func unusedHelper() {}
}
"#,
            "Sources/App/CarrierGuard.swift".to_string(),
        );
        let ext = analyze_swift_file(
            r#"
struct PanelWitness {
    func unusedPanel() {}
}
extension PanelWitness: PayloadMatching {
    func matchesPayload(_ raw: String) -> Bool { true }
    var glyph: String { "y" }
}
"#,
            "Sources/App/PanelWitness.swift".to_string(),
        );

        let files = [proto, impls, ext];
        let credits = protocol_witness_credits(&files);
        let credited = credits
            .get("Sources/App/CarrierGuard.swift")
            .expect("conformance file must receive discovered-requirement credits");
        assert!(
            credited.contains("matchesPayload"),
            "custom protocol func requirement must be credited from the snapshot declaration"
        );
        assert!(
            credited.contains("glyph"),
            "custom protocol property requirement must be credited from the snapshot declaration"
        );
        assert!(
            !credited.contains("unusedHelper"),
            "non-requirement helpers must not ride the witness credit"
        );
        let ext_credited = credits
            .get("Sources/App/PanelWitness.swift")
            .expect("extension conformance file must receive discovered-requirement credits");
        assert!(
            ext_credited.contains("matchesPayload"),
            "extension X: P must credit P's func requirement"
        );
        assert!(
            ext_credited.contains("glyph"),
            "extension X: P must credit P's property requirement"
        );
        assert!(
            !ext_credited.contains("unusedPanel"),
            "non-requirement helpers on the type must not ride extension witness credit"
        );

        let dead = crate::analyzer::dead_parrots::find_dead_exports(
            &files,
            false,
            None,
            crate::analyzer::dead_parrots::DeadFilterConfig::default(),
        );
        let dead_of = |file: &str, symbol: &str| {
            dead.iter()
                .any(|d| d.file.contains(file) && d.symbol == symbol)
        };
        assert!(
            !dead_of("CarrierGuard.swift", "matchesPayload"),
            "custom protocol witness matchesPayload must not be dead; got {dead:?}"
        );
        assert!(
            !dead_of("CarrierGuard.swift", "glyph"),
            "custom protocol requirement property glyph must not be dead; got {dead:?}"
        );
        assert!(
            !dead_of("PayloadMatching.swift", "matchesPayload"),
            "protocol requirement matchesPayload is live API once a snapshot type conforms; got {dead:?}"
        );
        assert!(
            dead_of("CarrierGuard.swift", "unusedHelper"),
            "non-requirement helper must stay a dead candidate; got {dead:?}"
        );
        assert!(
            !dead_of("PanelWitness.swift", "matchesPayload"),
            "extension-conformance witness matchesPayload must not be dead; got {dead:?}"
        );
        assert!(
            !dead_of("PanelWitness.swift", "glyph"),
            "extension-conformance requirement property glyph must not be dead; got {dead:?}"
        );
        assert!(
            dead_of("PanelWitness.swift", "unusedPanel"),
            "non-requirement helper on extension-conforming type must stay dead; got {dead:?}"
        );
    }

    #[test]
    fn w3_05_swift_same_module_call_not_dead() {
        let callee = analyze_swift_file(
            "func runtimePackMatchesCarrier(_ pack: String) -> Bool { true }\nfunc unusedScratch() {}\n",
            "Sources/App/RuntimePackMenuPolicy.swift".to_string(),
        );
        let caller = analyze_swift_file(
            "func bootPolicy() {\n    _ = runtimePackMatchesCarrier(\"x\")\n}\n",
            "Sources/App/ProductUpdatePolicy.swift".to_string(),
        );
        let dead = crate::analyzer::dead_parrots::find_dead_exports(
            &[callee, caller],
            false,
            None,
            crate::analyzer::dead_parrots::DeadFilterConfig::default(),
        );
        let names: Vec<&str> = dead.iter().map(|d| d.symbol.as_str()).collect();
        assert!(
            !names.contains(&"runtimePackMatchesCarrier"),
            "same-module Swift call must not be dead; got {names:?}"
        );
        assert!(
            names.contains(&"unusedScratch"),
            "unreferenced sibling must stay dead; got {names:?}"
        );
    }
}
