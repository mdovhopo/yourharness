import AppKit
import HarnessCore
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
            CommandGroup(after: .appInfo) {
                Button("Check for Updates…") { app.updater.checkNow() }
                    .disabled(!app.updater.isEnabled)
            }
            CommandGroup(replacing: .newItem) {
                Button("New Thread") { app.newThreadInCurrentProject() }
                    .keyboardShortcut("n")
                Button("Add Project…") { app.presentAddProject() }
                    .keyboardShortcut("o")
                Divider()
                Button("Resume Claude Code Session…") { app.isResumingSession = true }
                    .keyboardShortcut("o", modifiers: [.command, .shift])
                Divider()
                Button("Export State…") { app.stateTransfer.presentExport(app: app) }
                    .disabled(app.stateTransfer.busy != nil)
                Button("Import State…") { app.stateTransfer.presentImport(app: app) }
                    .disabled(app.stateTransfer.busy != nil || app.stateTransfer.pending != nil)
            }
            CommandGroup(after: .textEditing) {
                Divider()
                Button("Search Everything…") { app.spotlight.toggle(app) }
                    .keyboardShortcut("k")
                Button("Find in Thread…") { app.selectedThread?.find.open() }
                    .keyboardShortcut("f")
                    .disabled(app.visibleThreadID == nil)
                Button("Find Next") { app.selectedThread?.find.step(1) }
                    .keyboardShortcut("g")
                    .disabled(app.selectedThread?.find.matches.isEmpty ?? true)
                Button("Find Previous") { app.selectedThread?.find.step(-1) }
                    .keyboardShortcut("g", modifiers: [.command, .shift])
                    .disabled(app.selectedThread?.find.matches.isEmpty ?? true)
            }
            CommandGroup(after: .sidebar) {
                Button("Memory") { app.showMemory(of: app.memoryProject ?? app.overviewProject ?? app.selectedProject) }
                    .keyboardShortcut("m", modifiers: [.command, .shift])
                Divider()
                Button(app.showsInspector ? "Hide Right Sidebar" : "Show Right Sidebar") { app.setShowsInspector(!app.showsInspector) }
                    .keyboardShortcut("0", modifiers: [.command, .option])
                    .disabled(!app.inspectorHasThread)
            }
            CommandMenu("Thread") {
                Button("Previous Thread") {
                    app.selectAdjacentThread(-1)
                }
                .keyboardShortcut(.upArrow, modifiers: [.command, .option])
                Button("Next Thread") {
                    app.selectAdjacentThread(1)
                }
                .keyboardShortcut(.downArrow, modifiers: [.command, .option])
                Divider()
                Button("Previous Message") { app.selectedThread?.jumpToMessage(.previous) }
                    .keyboardShortcut("[")
                    .disabled(app.visibleThreadID == nil)
                Button("Next Message") { app.selectedThread?.jumpToMessage(.next) }
                    .keyboardShortcut("]")
                    .disabled(app.visibleThreadID == nil)
                Button("Top of Thread") { app.selectedThread?.jumpToMessage(.top) }
                    .keyboardShortcut("[", modifiers: [.command, .shift])
                    .disabled(app.visibleThreadID == nil)
                Button("End of Thread") { app.selectedThread?.jumpToMessage(.bottom) }
                    .keyboardShortcut("]", modifiers: [.command, .shift])
                    .disabled(app.visibleThreadID == nil)
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

        WindowGroup("Browser", id: "browser", for: UUID.self) { $threadID in
            if let threadID {
                BrowserWindow(threadID: threadID)
                    .environment(app)
            }
        }
        .defaultSize(width: 1320, height: 960)
        .restorationBehavior(.disabled)

        WindowGroup("Memory Agent", id: "memory-agent", for: MemoryAgentKey.self) { $key in
            if let key {
                MemoryAgentWindow(key: key)
                    .environment(app)
            }
        }
        .defaultSize(width: 1040, height: 760)
        .restorationBehavior(.disabled)

        WindowGroup("Diagram", id: "diagram", for: String.self) { $source in
            if let source {
                DiagramWindow(source: source)
            }
        }
        .defaultSize(width: 1100, height: 800)
        .restorationBehavior(.disabled)

        Settings {
            SettingsView()
                .environment(app)
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        MainActor.assumeIsolated {
            AppModel.shared.applyAppearance()
            WindowPresence.shared.start()
        }
        // Needed when running the bare executable (`swift run`) rather than the .app bundle.
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()
        // Development: stream the markdown sample into the selected thread, for checking
        // streaming without clicking through the Debug menu.
        if let message = ProcessInfo.processInfo.environment["YH_SEND_MESSAGE"], !message.isEmpty {
            MainActor.assumeIsolated {
                AppModel.shared.newThreadInCurrentProject()
                let images = (ProcessInfo.processInfo.environment["YH_SEND_IMAGES"] ?? "")
                    .split(separator: ":").compactMap { ImageAttachment.prepare(contentsOf: URL(fileURLWithPath: String($0))) }
                AppModel.shared.selectedThread?.send(message, images: images)
            }
        } else if let paths = ProcessInfo.processInfo.environment["YH_SEND_IMAGES"], !paths.isEmpty {
            // Without a message: attach to the selected thread's composer, as a drop does.
            MainActor.assumeIsolated {
                guard let thread = AppModel.shared.selectedThread else { return }
                thread.loadTranscriptIfNeeded()
                thread.attach(paths.split(separator: ":").map { .file(URL(fileURLWithPath: String($0))) })
            }
        }
        MainActor.assumeIsolated {
            AppModel.shared.updater.start()
            CookieImport.importIfRequested(app: AppModel.shared)
            BrowserProbe.runIfRequested(app: AppModel.shared)
            ThreadToolsProbe.runIfRequested(app: AppModel.shared)
            StateTransfer.runIfRequested(app: AppModel.shared)
            SwitchProbe.runIfRequested(app: AppModel.shared)
            NavigationProbe.runIfRequested(app: AppModel.shared)
            SearchProbe.runIfRequested(app: AppModel.shared)
            MemoryProbe.runIfRequested(app: AppModel.shared)
            StackProbe.runIfRequested(app: AppModel.shared)
            WindowSnapshot.runIfRequested()
            if let panel = ProcessInfo.processInfo.environment["YH_INSPECTOR_PANEL"].flatMap(AppModel.InspectorPanel.init(rawValue:)) {
                AppModel.shared.showInspector(panel)
            }
        }
        if ProcessInfo.processInfo.environment["YH_SIMULATE_STREAMING"] == "1" {
            MainActor.assumeIsolated {
                if let thread = AppModel.shared.selectedThread {
                    thread.loadTranscriptIfNeeded()
                    StressTest.simulateStreaming(into: thread)
                }
            }
        }
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        MainActor.assumeIsolated { AppModel.shared.didBecomeActive() }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    func applicationWillTerminate(_ notification: Notification) {
        MainActor.assumeIsolated {
            // Into the threads before they're saved.
            WindowPresence.shared.flush()
            AppModel.shared.shutdown()
        }
    }
}
