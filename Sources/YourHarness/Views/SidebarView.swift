import HarnessCore
import SwiftUI

struct SidebarView: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        @Bindable var app = app
        List(selection: $app.selectedThreadID) {
            NewThreadButton()
                .listRowInsets(EdgeInsets(top: 2, leading: 0, bottom: 10, trailing: 0))
                .listRowSeparator(.hidden)
                .selectionDisabled()

            ForEach(app.projects) { project in
                Section {
                    if !app.isCollapsed(project) {
                        ForEach(app.threads(in: project)) { thread in
                            ThreadRow(thread: thread)
                                .tag(thread.id)
                                .transition(.opacity.combined(with: .move(edge: .top)))
                                .contextMenu {
                                    Button("Delete Thread", systemImage: "trash", role: .destructive) {
                                        withAnimation(.snappy) { app.deleteThread(thread) }
                                    }
                                }
                        }
                    }
                } header: {
                    ProjectHeader(project: project)
                }
            }
        }
        .listStyle(.sidebar)
        // Inserts, deletes, collapses and recency reorders all glide instead of jumping.
        .animation(.snappy(duration: 0.28), value: app.sidebarOrder)
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

// MARK: - New thread

private struct NewThreadButton: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        Button {
            app.newThreadInCurrentProject()
        } label: {
            HStack(spacing: 9) {
                Image(systemName: "square.and.pencil")
                    .font(.body.weight(.semibold))
                    .foregroundStyle(Color.claude)
                Text("New Thread")
                    .font(.body.weight(.medium))
                Spacer()
                KeyCap("⌘N")
            }
        }
        .buttonStyle(SidebarActionStyle())
    }
}

/// Soft fill that brightens on hover and dips slightly when pressed.
private struct SidebarActionStyle: ButtonStyle {
    @State private var hovering = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background {
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .fill(.fill.opacity(configuration.isPressed ? 0.9 : hovering ? 0.7 : 0.45))
            }
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .contentShape(.rect)
            .onHover { hovering = $0 }
            .animation(.easeOut(duration: 0.15), value: hovering)
            .animation(.spring(response: 0.25, dampingFraction: 0.7), value: configuration.isPressed)
    }
}

private struct KeyCap: View {
    let text: String

    init(_ text: String) {
        self.text = text
    }

    var body: some View {
        Text(text)
            .font(.caption2.weight(.medium))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 5)
            .padding(.vertical, 1.5)
            .background(.background.opacity(0.6), in: .rect(cornerRadius: 4, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 4, style: .continuous)
                    .strokeBorder(.separator)
            }
    }
}

// MARK: - Projects

private struct ProjectHeader: View {
    @Environment(AppModel.self) private var app
    let project: Project
    @State private var hovering = false

    private var collapsed: Bool { app.isCollapsed(project) }
    private var threadCount: Int { app.threads(in: project).count }
    private var hasActivity: Bool {
        app.threads(in: project).contains { $0.phase == .running || $0.hasUnread || !$0.pendingPermissions.isEmpty }
    }

    var body: some View {
        HStack(spacing: 6) {
            Button {
                withAnimation(.snappy(duration: 0.25)) { app.setCollapsed(!collapsed, for: project) }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .bold))
                        .foregroundStyle(.tertiary)
                        .rotationEffect(.degrees(collapsed ? 0 : 90))
                        .frame(width: 10)
                    Image(systemName: app.isHome(project) ? "house.fill" : "folder.fill")
                        .font(.callout)
                        .foregroundStyle(app.isHome(project) ? AnyShapeStyle(Color.claude) : AnyShapeStyle(.secondary))
                    Text(project.name)
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                    if collapsed {
                        if hasActivity {
                            Circle()
                                .fill(Color.accentColor)
                                .frame(width: 6, height: 6)
                                .transition(.scale.combined(with: .opacity))
                        } else if threadCount > 0 {
                            Text("\(threadCount)")
                                .font(.caption2.weight(.semibold).monospacedDigit())
                                .foregroundStyle(.secondary)
                                .padding(.horizontal, 5)
                                .background(.fill.tertiary, in: .capsule)
                                .transition(.scale.combined(with: .opacity))
                        }
                    }
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)

            Spacer(minLength: 4)

            Button("New Thread in \(project.name)", systemImage: "plus") {
                app.newThread(in: project)
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .font(.callout.weight(.medium))
            .foregroundStyle(hovering ? .secondary : .tertiary)
            .help("New thread in \(project.name)")
        }
        .padding(.trailing, 6)
        .padding(.vertical, 2)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.15), value: hovering)
        .help(project.path)
        .contextMenu {
            Button("New Thread", systemImage: "square.and.pencil") { app.newThread(in: project) }
            Button("Show in Finder", systemImage: "folder") { app.revealInFinder(project) }
            if app.isHome(project) {
                Button("Change Default Folder…", systemImage: "house") { app.presentChooseHomeFolder() }
            } else {
                Divider()
                Button("Remove Project", systemImage: "minus.circle", role: .destructive) {
                    withAnimation(.snappy) { app.removeProject(project) }
                }
            }
        }
    }
}

// MARK: - Threads

private struct ThreadRow: View {
    let thread: ThreadModel

    var body: some View {
        HStack(spacing: 8) {
            ThreadStatus(thread: thread)
                .frame(width: 12, height: 12)
            Text(thread.title)
                .fontWeight(thread.hasUnread ? .semibold : .regular)
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 6)
            TimelineView(.everyMinute) { context in
                Text(Self.shortAge(from: thread.updatedAt, to: context.date))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .contentTransition(.numericText())
            }
        }
        .padding(.vertical, 3)
        .padding(.leading, 6)
        .animation(.snappy, value: thread.hasUnread)
    }

    /// Compact like Mail: "now", "53m", "2h", "Yesterday", "Mon", "Oct 3".
    static func shortAge(from date: Date, to now: Date) -> String {
        let seconds = now.timeIntervalSince(date)
        let calendar = Calendar.current
        switch seconds {
        case ..<60: return "now"
        case ..<3_600: return "\(Int(seconds / 60))m"
        case ..<86_400 where calendar.isDate(date, inSameDayAs: now): return "\(Int(seconds / 3_600))h"
        default:
            if calendar.isDateInYesterday(date) { return "Yesterday" }
            if seconds < 6 * 86_400 { return date.formatted(.dateTime.weekday(.abbreviated)) }
            return date.formatted(.dateTime.month(.abbreviated).day())
        }
    }
}

/// Working: a turning Claude mark. Waiting on you: a pulsing hand. Unread: a blue dot.
private struct ThreadStatus: View {
    let thread: ThreadModel

    private enum State: Equatable {
        case idle, running, needsApproval, unread
    }

    private var state: State {
        if !thread.pendingPermissions.isEmpty { return .needsApproval }
        if thread.phase == .running { return .running }
        if thread.hasUnread { return .unread }
        return .idle
    }

    var body: some View {
        ZStack {
            switch state {
            case .running:
                SpinningClaudeMark()
                    .transition(.scale.combined(with: .opacity))
            case .needsApproval:
                Image(systemName: "hand.raised.fill")
                    .font(.system(size: 10))
                    .foregroundStyle(.orange)
                    .symbolEffect(.pulse)
                    .transition(.scale.combined(with: .opacity))
            case .unread:
                Circle()
                    .fill(Color.accentColor)
                    .frame(width: 7, height: 7)
                    .transition(.scale.combined(with: .opacity))
            case .idle:
                Color.clear
            }
        }
        .animation(.spring(response: 0.3, dampingFraction: 0.75), value: state)
        .help(helpText)
    }

    private var helpText: String {
        switch state {
        case .running: "Working…"
        case .needsApproval: "Waiting for your approval"
        case .unread: "New reply"
        case .idle: ""
        }
    }
}

// MARK: - Footer

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
                .transition(.opacity)
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
                        .contentTransition(.numericText())
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
        .animation(.smooth, value: app.rateLimit)
    }
}
