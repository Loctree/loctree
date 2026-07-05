// Minimal Swift fixture for symbol_graph Wave B extraction.
// Exercises: type definition, protocol conformance, intra-module reference
// (no `import` between same-module files), and a NotificationCenter
// post/observe pair keyed on a `Notification.Name` constant.

import Foundation

/// Notification.Name constant — the literal both the emit and the observe
/// sites pair on (NotificationEmit ↔ NotificationObserve).
extension Notification.Name {
    static let vcDocumentChanged = Notification.Name("vcDocumentChanged")
}

/// Storage abstraction the store conforms to (Conforms edge).
protocol DocumentPersisting {
    func persist(_ text: String)
}

/// Primary type defined here (Defines edge; `find --where-symbol DocumentStore`
/// must resolve to this declaration in Wave B).
final class DocumentStore: DocumentPersisting {
    private(set) var contents: String = ""

    /// Conformance implementation (Implements/Overrides surface).
    func persist(_ text: String) {
        contents = text
        // NotificationEmit: post keyed on `.vcDocumentChanged`.
        NotificationCenter.default.post(name: .vcDocumentChanged, object: self)
    }
}
