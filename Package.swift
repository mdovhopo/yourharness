// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "YourHarness",
    platforms: [.macOS(.v26)],
    products: [
        .executable(name: "YourHarness", targets: ["YourHarness"]),
        .library(name: "HarnessCore", targets: ["HarnessCore"]),
    ],
    targets: [
        // Agent-agnostic core: process supervision, CLI protocol adapters, event model.
        // No SwiftUI/AppKit imports here, so it stays testable and reusable.
        .target(name: "HarnessCore"),
        .executableTarget(
            name: "YourHarness",
            dependencies: ["HarnessCore"]
        ),
        .testTarget(
            name: "HarnessCoreTests",
            dependencies: ["HarnessCore"],
            resources: [.copy("Fixtures")]
        ),
    ]
)
