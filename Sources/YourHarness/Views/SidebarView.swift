import HarnessCore
import SwiftUI

/// Hand-built rather than a `List`: the system selection highlight can't animate, and a
/// pill that glides between threads is the point.
struct SidebarView: View {
    @Environment(AppModel.self) private var app
    @Namespace private var selection
    @FocusState private var focused: Bool

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 1) {
                ForEach(app.projects) { project in
                    ProjectHeader(project: project)
                        .padding(.top, project.id == app.projects.first?.id ? 2 : 12)
                        .padding(.bottom, 2)

                    if !app.isCollapsed(project) {
                        ForEach(app.threads(in: project)) { thread in
                            ThreadRow(thread: thread, selection: selection)
                                .transition(.opacity.combined(with: .move(edge: .top)))
                        }
                    }
                }
            }
            .padding(.horizontal, 10)
            .padding(.bottom, 8)
            // Inserts, deletes, collapses and recency reorders all glide instead of jumping.
            .animation(.snappy(duration: 0.28), value: app.sidebarOrder)
        }
        .scrollIndicators(.never)
        .focusable()
        .focused($focused)
        .focusEffectDisabled()
        .onKeyPress(.upArrow) { move(-1) }
        .onKeyPress(.downArrow) { move(1) }
        .safeAreaInset(edge: .top, spacing: 0) {
            NewThreadButton()
                .padding(.horizontal, 10)
                .padding(.top, 4)
                .padding(.bottom, 10)
        }
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

    private func move(_ offset: Int) -> KeyPress.Result {
        withAnimation(SidebarMetrics.selectionSpring) { app.selectAdjacentThread(offset) }
        return .handled
    }
}

enum SidebarMetrics {
    /// Fixed columns so thread titles line up under project names:
    /// [chevron][gap][icon][gap][name] over [indent][status][gap][title].
    static let chevron: CGFloat = 10
    static let icon: CGFloat = 18
    static let gap: CGFloat = 6
    static let rowPadding: CGFloat = 8
    static let selectionSpring = Animation.spring(response: 0.32, dampingFraction: 0.82)
}

// MARK: - New thread

private struct NewThreadButton: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        Button {
            withAnimation(SidebarMetrics.selectionSpring) { app.newThreadInCurrentProject() }
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
        HStack(spacing: SidebarMetrics.gap) {
            Button {
                withAnimation(.snappy(duration: 0.25)) { app.setCollapsed(!collapsed, for: project) }
            } label: {
                HStack(spacing: SidebarMetrics.gap) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .bold))
                        .foregroundStyle(.tertiary)
                        .rotationEffect(.degrees(collapsed ? 0 : 90))
                        .frame(width: SidebarMetrics.chevron)
                    Image(systemName: app.isHome(project) ? "house.fill" : "folder.fill")
                        .font(.callout)
                        .foregroundStyle(app.isHome(project) ? AnyShapeStyle(Color.claude) : AnyShapeStyle(.secondary))
                        .frame(width: SidebarMetrics.icon)
                    Text(project.name)
                        .font(.callout.weight(.semibold))
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
                withAnimation(SidebarMetrics.selectionSpring) { _ = app.newThread(in: project) }
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .font(.callout.weight(.medium))
            .foregroundStyle(hovering ? .secondary : .tertiary)
            .help("New thread in \(project.name)")
        }
        .padding(.horizontal, SidebarMetrics.rowPadding - 4)
        .padding(.vertical, 4)
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
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    let selection: Namespace.ID
    @State private var hovering = false

    private var isSelected: Bool { app.selectedThreadID == thread.id }

    var body: some View {
        HStack(spacing: SidebarMetrics.gap) {
            // Status sits under the project icon; the title lines up with the project name.
            ThreadStatus(thread: thread)
                .frame(width: SidebarMetrics.icon, height: 14)
                .padding(.leading, SidebarMetrics.chevron + SidebarMetrics.gap)
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
        .padding(.horizontal, SidebarMetrics.rowPadding - 4)
        .padding(.vertical, 6)
        .background {
            if isSelected {
                // One pill, shared across rows: it slides to wherever the selection goes.
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(Color.primary.opacity(0.1))
                    .matchedGeometryEffect(id: "selection", in: selection)
            } else if hovering {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(Color.primary.opacity(0.045))
            }
        }
        .contentShape(.rect)
        .onTapGesture {
            withAnimation(SidebarMetrics.selectionSpring) { app.selectedThreadID = thread.id }
        }
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .animation(.snappy, value: thread.hasUnread)
        .contextMenu {
            Button("Delete Thread", systemImage: "trash", role: .destructive) {
                withAnimation(.snappy) { app.deleteThread(thread) }
            }
        }
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
