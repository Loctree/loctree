// Implicit-edge regression fixture (loctree-feedback.md 2026-07-25, blinksh/blink).
// Top-level type plus a NESTED `enum Error` — the Swift analyzer flattens the
// nested declaration into file-level exports, which used to turn every file
// using the stdlib `Error` protocol into an "importer" of this file.

import Foundation

final class DefaultAgent {
    static let instance = DefaultAgent()

    enum Error: Swift.Error {
        case missingKey
    }

    func addKey() throws {
        throw Error.missingKey
    }
}
