import Foundation

public struct DocumentStore {
    public let id: String
}

public protocol Searchable {
    func searchDocumentRecords()
}

public class IndexDatabase: Searchable {
    public var temp: IndexDatabase?
    
    public init() {}
    public func searchDocumentRecords() {}
}
