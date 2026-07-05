import Foundation
import class AppState.DocumentStore

public final class WorkspaceCacheStore {
    public static let shared = WorkspaceCacheStore()
    private var records: [String: Any] = [:]

    // Usage of IndexDatabase as consumer
    public var db: IndexDatabase?

    public func updateWorkspaceSearch() {
        let store = DocumentStore(id: "123")
        self.db?.searchDocumentRecords()
    }
}
