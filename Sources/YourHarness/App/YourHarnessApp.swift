import AppKit
import SwiftUI

@main
struct YourHarnessApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var app = AppModel.shared

    var body: some Scene {
        Window("YourHarness", id: "main") {
            RootView()
                .environment(app)
        }
        .defaultSize(width: 1200, height: 800)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("New Thread") { app.newThreadInCurrentProject() }
                    .keyboardShortcut("n")
                Button("Add Project…") { app.presentAddProject() }
                    .keyboardShortcut("o")
            }
            CommandMenu("Thread") {
                Button("Previous Thread") {
                    withAnimation(SidebarMetrics.selectionSpring) { app.selectAdjacentThread(-1) }
                }
                .keyboardShortcut(.upArrow, modifiers: [.command, .option])
                Button("Next Thread") {
                    withAnimation(SidebarMetrics.selectionSpring) { app.selectAdjacentThread(1) }
                }
                .keyboardShortcut(.downArrow, modifiers: [.command, .option])
                Divider()
                Button("Stop") { app.selectedThread?.stop() }
                    .keyboardShortcut(".")
                    .disabled(app.selectedThread?.phase != .running)
            }
            CommandMenu("Debug") {
                Button("Open Stress Transcript (10,000 entries)") { app.openStressThread() }
                Button("Simulate Streaming in Current Thread") {
                    if let thread = app.selectedThread { StressTest.simulateStreaming(into: thread) }
                }
                .disabled(app.selectedThread == nil)
            }
        }

        Settings {
            SettingsView()
                .environment(app)
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        // Needed when running the bare executable (`swift run`) rather than the .app bundle.
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()
        // Development: stream the markdown sample into the selected thread, for checking
        // streaming without clicking through the Debug menu.
        if ProcessInfo.processInfo.environment["YH_SIMULATE_STREAMING"] == "1" {
            MainActor.assumeIsolated {
                if let thread = AppModel.shared.selectedThread {
                    thread.loadTranscriptIfNeeded()
                    StressTest.simulateStreaming(into: thread)
                }
            }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    func applicationWillTerminate(_ notification: Notification) {
        MainActor.assumeIsolated { AppModel.shared.shutdown() }
    }
}
