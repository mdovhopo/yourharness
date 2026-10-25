import SwiftUI

/// A project's page, from clicking its name: its pull requests and repos, or its memory,
/// switched in the middle of the toolbar. Home has no workspace, so its page is its memory.
struct ProjectPage: View {
    @Environment(AppModel.self) private var app
    let project: Project

    private var workspace: WorkspaceModel? { app.workspace(for: project) }
    private var tab: AppModel.ProjectTab { workspace == nil ? .memory : app.projectTab }

    var body: some View {
        Group {
            switch tab {
            case .workspace:
                if let workspace { WorkspaceView(project: project, workspace: workspace) }
            case .memory:
                MemoryView(project: project, library: app.memoryLibrary(for: project))
            }
        }
        .navigationTitle(project.name)
        .navigationSubtitle(tab.label)
        .toolbar {
            if workspace != nil {
                ToolbarItem(placement: .principal) {
                    Picker("Page", selection: Binding(get: { app.projectTab }, set: { app.showProject(project, tab: $0) })) {
                        ForEach(AppModel.ProjectTab.allCases) { tab in
                            Label(tab.label, systemImage: tab.symbol).tag(tab)
                        }
                    }
                    .pickerStyle(.segmented)
                    .labelStyle(.titleAndIcon)
                    .labelsHidden()
                    .help("Switch between \(project.name)'s pull requests and its memory")
                }
            }
        }
    }
}
