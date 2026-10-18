import HarnessCore
import SwiftUI

struct SidebarView: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        @Bindable var app = app
        List(selection: $app.selectedThreadID) {
            NewThreadRow()

            ForEach(app.projects) { project in
                Section {
                    ForEach(app.threads(in: project)) { thread in
                        ThreadRow(thread: thread)
                            .tag(thread.id)
                            .contextMenu {
                                Button("Delete Thread", role: .destructive) { app.deleteThread(thread) }
                            }
                    }
                } header: {
                    ProjectHeader(project: project)
                }
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            UsageFooter()
        }
        .toolbar {
            ToolbarItemGroup {
                Button("Add Project", systemImage: "folder.badge.plus") { app.presentAddProject() }
                    .help("Add Project (⌘O)")
                Button("New Thread", systemImage: "square.and.pencil") { app.newThreadInCurrentProject() }
                    .help("New Thread (⌘N)")
            }
        }
    }
}

/// A big, obvious entry point at the top of the sidebar, like Mail's or Notes' compose.
private struct NewThreadRow: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        Button {
            app.newThreadInCurrentProject()
        } label: {
            HStack(spacing: 8) {
                Image(systemName: "square.and.pencil")
                    .fontWeight(.medium)
                Text("New Thread")
                    .fontWeight(.medium)
                Spacer()
                Text("⌘N")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 10))
        .listRowInsets(EdgeInsets(top: 4, leading: 4, bottom: 8, trailing: 4))
        .selectionDisabled()
    }
}

private struct ProjectHeader: View {
    @Environment(AppModel.self) private var app
    let project: Project

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: app.isHome(project) ? "house" : "folder")
            Text(project.name)
                .lineLimit(1)
            Spacer()
            Button("New Thread", systemImage: "plus") { app.newThread(in: project) }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
                .help("New thread in \(project.name)")
        }
        .help(project.path)
        .contextMenu {
            Button("New Thread") { app.newThread(in: project) }
            Button("Show in Finder") { app.revealInFinder(project) }
            if app.isHome(project) {
                Button("Change Default Folder…") { app.presentChooseHomeFolder() }
            } else {
                Divider()
                Button("Remove Project", role: .destructive) { app.removeProject(project) }
            }
        }
    }
}

private struct ThreadRow: View {
    let thread: ThreadModel

    var body: some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(thread.title)
                    .lineLimit(1)
                Text(thread.updatedAt, format: .relative(presentation: .named))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
            if !thread.pendingPermissions.isEmpty {
                Image(systemName: "hand.raised.fill")
                    .foregroundStyle(.orange)
                    .help("Waiting for your approval")
            } else if thread.phase == .running {
                ProgressView()
                    .controlSize(.small)
            }
        }
        .padding(.vertical, 2)
    }
}

/// Subscription usage from the CLI's `rate_limit_event`, this app's own memory, and a way
/// into Settings.
private struct UsageFooter: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let rateLimit = app.rateLimit, !rateLimit.windows.isEmpty {
                // Re-evaluate every minute so "resets in" and reset detection stay current.
                TimelineView(.everyMinute) { context in
                    VStack(alignment: .leading, spacing: 10) {
                        ForEach(rateLimit.windows) { window in
                            UsageBar(window: window, now: context.date)
                        }
                        if let updated = app.rateLimitUpdatedAt {
                            Text("Updated \(updated, format: .relative(presentation: .named))")
                                .font(.caption2)
                                .foregroundStyle(.tertiary)
                        }
                    }
                }
            } else {
                Text("Usage limits appear after your first message.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            HStack {
                TimelineView(.periodic(from: .now, by: 2)) { _ in
                    Label("App memory \(MemoryFootprint.formatted)", systemImage: "memorychip")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
                Spacer()
                SettingsLink {
                    Image(systemName: "gearshape")
                }
                .buttonStyle(.borderless)
                .help("Settings (⌘,)")
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .glassEffect(.regular, in: .rect(cornerRadius: 14))
        .padding(10)
    }
}
