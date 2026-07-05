import SwiftUI

final class WorkspaceMetadataStore {
    func closeActiveDocument() {}
}

final class FolderManager {
    func openResolvedWorkspace() {}
    func rebuildWorkspace() {}
    func scanChildren() {}
}

struct DocumentCommands: Commands {
    let metadataStore: WorkspaceMetadataStore

    var body: some Commands {
        CommandMenu("File") {
            Button("Close") {
                metadataStore.closeActiveDocument()
            }
        }
    }
}
