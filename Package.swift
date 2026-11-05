// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "YourHarness",
    platforms: [.macOS(.v26)],
    products: [
        .executable(name: "YourHarness", targets: ["YourHarness"]),
        .executable(name: "YourHarnessWeb", targets: ["YourHarnessWeb"]),
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
        // The web rendering experiment (web/README.md): a thin AppKit window holding one
        // WKWebView, over the same HarnessCore.
        .executableTarget(
            name: "YourHarnessWeb",
            dependencies: ["HarnessCore"],
            path: "web/host"
        ),
        .testTarget(
            name: "HarnessCoreTests",
            dependencies: ["HarnessCore"],
            resources: [.copy("Fixtures")]
        ),
        // The app's own model: what it saves, and moving in what older versions saved.
        .testTarget(
            name: "YourHarnessTests",
            dependencies: ["YourHarness", "HarnessCore"]
        ),
    ]
)
