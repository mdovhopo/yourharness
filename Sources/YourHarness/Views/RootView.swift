import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings
    @Environment(\.undoManager) private var undoManager
    /// The window is first laid out at its default size, before it grows to the saved one.
    /// Opening the right sidebar in that pass can leave no room for the left one, which
    /// AppKit then collapses while its toggle still says it's shown; so it opens once the
    /// window is on screen.
    @State private var isOnScreen = false

    var body: some View {
        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 220, ideal: 270, max: 380)
        } detail: {
            ZStack {
                // Switching is instant, as in Mail or Messages: no crossfade to wait through.
                if let project = app.overviewProject, let workspace = app.workspace(for: project) {
                    WorkspaceView(project: project, workspace: workspace)
                        .id(project.id)
                } else if let thread = app.selectedThread {
                    ThreadView(thread: thread)
                        .id(thread.id)
                } else {
                    EmptyDetailView()
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            // One surface under every view, so switching swaps content, not the canvas.
            .canvasBackground()
            .transaction(value: app.selectedThreadID) { $0.animation = nil }
            .transaction(value: app.overviewProjectID) { $0.animation = nil }
            .inspector(isPresented: Binding(get: { isOnScreen && app.showsInspector }, set: { app.setShowsInspector($0) })) {
                ThreadInspector(thread: app.overviewProject == nil ? app.selectedThread : nil)
                    .inspectorColumnWidth(min: 300, ideal: 380, max: 960)
            }
        }
        .sheet(isPresented: Bindable(app).isResumingSession) {
            ResumeSessionSheet()
        }
        .onChange(of: undoManager, initial: true) { app.undoManager = undoManager }
        .onAppear {
            isOnScreen = true
            fitOnScreen()
            // Development: open the resume sheet filled in, for checking it without clicking.
            if ProcessInfo.processInfo.environment["YH_RESUME_SESSION"] != nil { app.isResumingSession = true }
            // Development: open Settings, for checking it without a shortcut.
            if ProcessInfo.processInfo.environment["YH_OPEN_SETTINGS"] != nil { openSettings() }
            // Development: open the selected thread's browser in its own window.
            if ProcessInfo.processInfo.environment["YH_OPEN_BROWSER_WINDOW"] != nil, let thread = app.selectedThread {
                openWindow(id: "browser", value: thread.id)
            }
            // Development: open a Mermaid file in the diagram window, for checking it without clicking.
            if let path = ProcessInfo.processInfo.environment["YH_OPEN_DIAGRAM"],
               let source = try? String(contentsOfFile: path, encoding: .utf8) {
                openWindow(id: "diagram", value: source)
            }
        }
    }
}

/// A window saved taller or wider than its screen (0.0.2 and 0.0.3 could grow one past the
/// bottom, from the Browser tab) comes back fitted to it.
@MainActor
private func fitOnScreen() {
    DispatchQueue.main.async {
        for window in NSApp.windows where window.isVisible && !window.styleMask.contains(.fullScreen) {
            guard let visible = (window.screen ?? NSScreen.main)?.visibleFrame else { continue }
            let frame = window.frame
            guard frame.height > visible.height + 1 || frame.width > visible.width + 1 else { continue }
            let size = NSSize(width: min(frame.width, visible.width), height: min(frame.height, visible.height))
            let origin = NSPoint(x: min(max(frame.minX, visible.minX), visible.maxX - size.width),
                                 y: min(max(frame.minY, visible.minY), visible.maxY - size.height))
            window.setFrame(NSRect(origin: origin, size: size), display: true)
        }
    }
}

private struct EmptyDetailView: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        if app.projects.isEmpty {
            ContentUnavailableView {
                Label("Add Your First Project", systemImage: "folder.badge.plus")
            } description: {
                Text("Pick a folder to work in. Claude runs there, using your own Claude Code login.")
            } actions: {
                Button("Add Project…") { app.presentAddProject() }
                    .buttonStyle(.glassProminent)
                    .controlSize(.large)
            }
        } else {
            ContentUnavailableView {
                Label("No Thread Selected", systemImage: "bubble.left.and.text.bubble.right")
            } description: {
                Text("Choose a thread in the sidebar, or start a new one with ⌘N.")
            } actions: {
                Button("New Thread") { app.newThreadInCurrentProject() }
                    .buttonStyle(.glass)
            }
        }
    }
}
