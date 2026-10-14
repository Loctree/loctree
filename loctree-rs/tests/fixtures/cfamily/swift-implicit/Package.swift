// swift-tools-version:5.9
// Implicit-edge regression fixture (loctree-feedback.md 2026-07-25, blinksh/blink).
//
// Module layout matters: the whole defect exists because Swift files in ONE
// target see each other without `import`. AgentKit is that single module
// (Agent/Consumer/Unrelated, zero imports between them). SharedA and SharedB
// are SEPARATE targets that both declare `SharedThing` — valid Swift, and
// exactly the cross-target duplication loctree cannot attribute from a flat
// file scan, so no implicit edge may be guessed for that name.

import PackageDescription

let package = Package(
    name: "ImplicitFixture",
    targets: [
        .target(name: "AgentKit"),
        .target(name: "SharedA"),
        .target(name: "SharedB"),
        .target(name: "ClientKit", dependencies: ["SharedA"]),
    ]
)
