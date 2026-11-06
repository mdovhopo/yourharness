// swift-tools-version: 6.2
import PackageDescription

// The app is a thin AppKit window holding one WKWebView (app/host) that draws everything
// with the UI in app/ui. Under it, HarnessBackend owns what the app knows (threads, rows,
// sessions) and HarnessCore drives the CLIs. The SwiftUI app this replaced is frozen in
// legacy/ (tag swift-app-frozen) and isn't built. See docs/architecture.md.
let package = Package(
    name: "YourHarness",
    platforms: [.macOS(.v26)],
    products: [
        .executable(name: "YourHarness", targets: ["YourHarness"]),
        .library(name: "HarnessCore", targets: ["HarnessCore"]),
        .library(name: "HarnessBackend", targets: ["HarnessBackend"]),
    ],
    targets: [
        // Agent-agnostic core: process supervision, CLI protocol adapters, event model, storage.
        // No SwiftUI/AppKit imports here, so it stays testable and reusable.
        .target(name: "HarnessCore"),
        // The app's state and services, with no UI: what the page mirrors. Speaks only the
        // protocol in docs/protocol.md, so it can be replaced (Rust is the plan) without the UI.
        .target(name: "HarnessBackend", dependencies: ["HarnessCore"]),
        .executableTarget(
            name: "YourHarness",
            dependencies: ["HarnessCore", "HarnessBackend"],
            path: "app/host"
        ),
        .testTarget(
            name: "HarnessCoreTests",
            dependencies: ["HarnessCore"],
            resources: [.copy("Fixtures")]
        ),
        .testTarget(
            name: "HarnessBackendTests",
            dependencies: ["HarnessBackend", "HarnessCore"]
        ),
    ]
)
