import Foundation

// Cross-file referenced (used by view.swift) — must NOT be dead.
struct DocumentSession {
    var documentTitle: String
    var isDirty: Bool

    func parse() -> String {
        // same-file use of a sibling declaration
        return normalize(documentTitle)
    }
}

// Referenced only within THIS file (by parse()) — must NOT be dead.
func normalize(_ s: String) -> String {
    return s.trimmingCharacters(in: .whitespaces)
}

// Referenced nowhere in the snapshot — this is the ONLY genuine orphan.
struct AbandonedScratchModel {
    var note: String
}
