import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 220, ideal: 270, max: 380)
        } detail: {
            ZStack {
                if let project = app.overviewProject, let workspace = app.workspace(for: project) {
                    WorkspaceView(project: project, workspace: workspace)
                        .id(project.id)
                        .transition(.opacity)
                } else if let thread = app.selectedThread {
                    ThreadView(thread: thread)
                        .id(thread.id)
                        .transition(.opacity)
                } else {
                    EmptyDetailView()
                        .transition(.opacity)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            // One surface under every view, so switching crossfades content, not the canvas.
            .canvasBackground()
            // A quick crossfade when switching threads, not a hard cut.
            .animation(.easeInOut(duration: 0.16), value: app.selectedThreadID)
            .animation(.easeInOut(duration: 0.16), value: app.overviewProjectID)
            .inspector(isPresented: Binding(get: { app.showsInspector }, set: { app.setShowsInspector($0) })) {
                Group {
                    if app.overviewProject == nil, let thread = app.selectedThread {
                        SubagentInspector(thread: thread)
                            .environment(\.issueLinker, app.issueLinker(for: thread))
                    } else {
                        ContentUnavailableView("No Thread Selected", systemImage: "person.2")
                    }
                }
                .inspectorColumnWidth(min: 300, ideal: 380, max: 640)
                .toolbar {
                    Button("Sub-agents", systemImage: "sidebar.trailing") { app.setShowsInspector(!app.showsInspector) }
                        .help(app.showsInspector ? "Hide sub-agents (⌥⌘0)" : "Show sub-agents (⌥⌘0)")
                }
            }
        }
        .sheet(isPresented: Bindable(app).isResumingSession) {
            ResumeSessionSheet()
        }
        .onAppear {
            // Development: open the resume sheet filled in, for checking it without clicking.
            if ProcessInfo.processInfo.environment["YH_RESUME_SESSION"] != nil { app.isResumingSession = true }
            // Development: open Settings, for checking it without a shortcut.
            if ProcessInfo.processInfo.environment["YH_OPEN_SETTINGS"] != nil { openSettings() }
            // Development: open a Mermaid file in the diagram window, for checking it without clicking.
            if let path = ProcessInfo.processInfo.environment["YH_OPEN_DIAGRAM"],
               let source = try? String(contentsOfFile: path, encoding: .utf8) {
                openWindow(id: "diagram", value: source)
            }
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
