# C-family symbol_graph fixtures

Minimal Swift + Objective-C fixtures for the `symbol_graph` layer
(`loctree-rs/src/symbols/`). Wave A ships them as **documented inputs**; the
files are read as text by loctree, never compiled — SourceKit/clang diagnostics
in an editor are expected and irrelevant.

Each fixture deliberately exercises a slice of the `SymbolEdgeKind` /
`OccurrenceRole` surface so Wave B (tree-sitter extraction) and Wave C
(usage/deep edges) have concrete acceptance targets.

## `swift/` — intra-module symbols + NotificationCenter pair

| File | Constructs | Edge / role targets |
|---|---|---|
| `DocumentStore.swift` | `class DocumentStore`, `protocol DocumentPersisting`, `extension Notification.Name { .vcDocumentChanged }`, `.post(name:)` | `Defines`, `Conforms`, `Implements`, `NotificationEmit` |
| `DocumentStoreTests.swift` | constructs + calls `DocumentStore`, `addObserver(forName: .vcDocumentChanged)` | `References`, `Calls`, `NotificationObserve` |

Key property: the two files are in the **same module**, so there is **no
`import` edge between them**. A correct symbol graph still links
`DocumentStoreTests` → `DocumentStore` (proving symbol-level consumers exist
where the file-level `import_graph` reports zero). The notification pair links
on the `.vcDocumentChanged` literal name.

## `objc/` — `.h`↔`.m` split + selector + NSNotificationCenter

| File | Constructs | Edge / role targets |
|---|---|---|
| `EditorViewController.h` | `@interface`, `@property`, `#import <Foundation/Foundation.h>`, `- (IBAction)saveDocument:` | `Declares`, `Includes`, `IBActionBinding` |
| `EditorViewController.m` | `@implementation`, `#import "EditorViewController.h"`, `@selector(handleDocumentChanged:)`, `addTarget`/`addObserver`, `postNotificationName:` | `Defines`, `Includes`, `SelectorMessage`, `NotificationObserve`, `NotificationEmit` |

Key property: `@interface` (in `.h`) **declares** what `@implementation` (in
`.m`) **defines** — the canonical ObjC declare/implement split. The `@selector`
target-action wiring is the `SelectorMessage` heuristic surface, and the
`NSNotificationCenter` post/observe pair mirrors the Swift one on the
`VCDocumentChanged` literal.

## Related (not owned by this fixture set)

`cfamily/Pensieve/` is a separate Swift fixture backing the `import_graph` /
`query where-symbol` path in `tests/swift_extraction.rs`. It includes
package-internal `final class` declarations plus a SwiftUI `Commands` menu with
`Button("Close") { closeActiveDocument() }`, because older Loctree builds missed
that production-shaped surface. The `swift/` and `objc/` dirs here are the
symbol_graph-specific minimal cases. See also `cfamily/scip/` for deep-mode
indexing fixtures.
