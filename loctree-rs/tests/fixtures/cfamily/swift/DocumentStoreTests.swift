// Cross-file consumer of DocumentStore — proves a Swift symbol has
// references/consumers even when there is NO import edge (same module).
// Exercises: References + Call edges to DocumentStore, and the observe half
// of the NotificationCenter pair (NotificationObserve).

import Foundation
import XCTest

final class DocumentStoreTests: XCTestCase {
    func testPersistEmitsNotification() {
        // Reference + Call edges into DocumentStore (defined in DocumentStore.swift).
        let store = DocumentStore()

        // NotificationObserve: pairs with the emit in DocumentStore.persist.
        let token = NotificationCenter.default.addObserver(
            forName: .vcDocumentChanged,
            object: nil,
            queue: nil
        ) { _ in }

        store.persist("hello")
        XCTAssertEqual(store.contents, "hello")
        NotificationCenter.default.removeObserver(token)
    }
}
