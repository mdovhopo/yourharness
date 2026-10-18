import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 220, ideal: 270, max: 380)
        } detail: {
            if let thread = app.selectedThread {
                ThreadView(thread: thread)
                    .id(thread.id)
            } else {
                EmptyDetailView()
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
