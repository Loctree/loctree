import Foundation

// Cross-file references into model.swift: DocumentSession, documentTitle, parse.
struct DocumentView {
    let session: DocumentSession

    func render() -> String {
        let title = session.documentTitle
        return title + session.parse()
    }
}
