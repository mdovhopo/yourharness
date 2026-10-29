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
                    ProjectHeader(project: project, selection: selection)
                        .padding(.top, project.id == app.projects.first?.id ? 2 : 12)
                        .padding(.bottom, 2)

                    if !app.isCollapsed(project) {
                        ForEach(app.threads(in: project)) { thread in
                            ThreadRow(thread: thread, selection: selection)
                                .transition(.opacity.combined(with: .move(edge: .top)))
                        }
                        let settled = app.settledThreads(in: project)
                        if !settled.isEmpty {
                            SettledShelfToggle(project: project, count: settled.count)
                            if app.expandedShelves.contains(project.id) {
                                ForEach(settled) { thread in
                                    ThreadRow(thread: thread, selection: selection)
                                        .transition(.opacity.combined(with: .move(edge: .top)))
                                }
                            }
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
            VStack(spacing: 6) {
                NewThreadButton()
                SearchButton()
            }
            .padding(.horizontal, 10)
            .padding(.top, 4)
            .padding(.bottom, 10)
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                if let notice = app.undoNotice {
                    UndoToast(notice: notice)
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
                UsageFooter()
            }
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
        app.selectAdjacentThread(offset)
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

/// Opens ⌘K: search threads, memories and tasks everywhere.
private struct SearchButton: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        Button {
            app.spotlight.open(app)
        } label: {
            HStack(spacing: 9) {
                Image(systemName: "magnifyingglass")
                    .font(.body.weight(.semibold))
                    .foregroundStyle(.secondary)
                Text("Search")
                    .foregroundStyle(.secondary)
                Spacer()
                KeyCap("⌘K")
            }
        }
        .buttonStyle(SidebarActionStyle())
        .help("Search threads, what was said in them, memories and tasks, and jump there (⌘K)")
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
    let selection: Namespace.ID
    @State private var hovering = false

    private var collapsed: Bool { app.isCollapsed(project) }
    private var threadCount: Int { app.threads(in: project).count }
    private var hasActivity: Bool {
        app.threads(in: project).contains { $0.isWorking || $0.hasUnread || !$0.pendingPermissions.isEmpty }
    }
    private var workspace: WorkspaceModel? { app.workspace(for: project) }
    private var isSelected: Bool { app.overviewProjectID == project.id }
    private var reviewCount: Int {
        workspace?.pullRequests.count(where: \.reviewRequestedFromMe) ?? 0
    }

    var body: some View {
        HStack(spacing: SidebarMetrics.gap) {
            Button {
                withAnimation(.snappy(duration: 0.25)) { app.setCollapsed(!collapsed, for: project) }
            } label: {
                Image(systemName: "chevron.right")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundStyle(.tertiary)
                    .rotationEffect(.degrees(collapsed ? 0 : 90))
                    .frame(width: SidebarMetrics.chevron, height: 18)
                    .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .help(collapsed ? "Show threads" : "Hide threads")

            HStack(spacing: SidebarMetrics.gap) {
                Image(systemName: app.isHome(project) ? "house.fill" : ProjectIcon.symbol(for: workspace?.layout?.kind))
                    .font(.callout)
                    .foregroundStyle(app.isHome(project) ? AnyShapeStyle(Color.claude) : AnyShapeStyle(.secondary))
                    .frame(width: SidebarMetrics.icon)
                    .contentTransition(.symbolEffect(.replace))
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
                Spacer(minLength: 4)
                if reviewCount > 0 {
                    Label("\(reviewCount)", systemImage: "eye")
                        .font(.caption2.weight(.semibold).monospacedDigit())
                        .labelStyle(.titleAndIcon)
                        .foregroundStyle(Color.accentColor)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(Color.accentColor.opacity(0.14), in: .capsule)
                        .help("\(reviewCount) pull request\(reviewCount == 1 ? "" : "s") waiting for your review")
                        .transition(.scale.combined(with: .opacity))
                }
            }
            .contentShape(.rect)
            .onTapGesture {
                app.showProject(project)
            }

            if hovering || app.memoryProjectID == project.id {
                Button("\(project.name) Memory", systemImage: "brain") { app.showMemory(of: project) }
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .font(.callout.weight(.medium))
                    .foregroundStyle(app.memoryProjectID == project.id ? AnyShapeStyle(Color.claude) : AnyShapeStyle(.secondary))
                    .help("What agents remember in \(project.name) (⇧⌘M)")
                    .transition(.opacity)
            }

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
        .background {
            if isSelected {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(Color.primary.opacity(0.1))
                    .matchedGeometryEffect(id: "selection", in: selection)
            } else if hovering, !app.isHome(project) {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(Color.primary.opacity(0.045))
            }
        }
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.15), value: hovering)
        .animation(.snappy, value: reviewCount)
        .help(app.isHome(project) ? "\(project.path): its memory" : "Pull requests, repos and memory of \(project.name)")
        .contextMenu {
            Button("New Thread", systemImage: "square.and.pencil") { app.newThread(in: project) }
            if !app.isHome(project) {
                Button("Repos & Pull Requests", systemImage: "arrow.triangle.pull") { app.showOverview(of: project) }
            }
            Button("Memory", systemImage: "brain") { app.showMemory(of: project) }
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
    @State private var renaming = false

    private var isSelected: Bool { app.visibleThreadID == thread.id }

    var body: some View {
        HStack(spacing: SidebarMetrics.gap) {
            // Status sits under the project icon; the title lines up with the project name.
            ThreadStatus(thread: thread)
                .frame(width: SidebarMetrics.icon, height: 14)
                .padding(.leading, SidebarMetrics.chevron + SidebarMetrics.gap)
            if renaming {
                RenameField(thread: thread, isPresented: $renaming)
            } else {
                Text(thread.title)
                    .fontWeight(thread.hasUnread ? .semibold : .regular)
                    .foregroundStyle(thread.isSettled ? .secondary : .primary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    // A placeholder until the generated title arrives.
                    .opacity(thread.isGeneratingTitle ? 0.55 : 1)
                    .contentTransition(.opacity)
                    .accessibilityValue(thread.isGeneratingTitle ? "Generating title" : "")
            }
            Spacer(minLength: 6)
            if hovering && !renaming {
                RowActions(thread: thread, renaming: $renaming)
                    .transition(.opacity)
            } else if thread.isPinned {
                Image(systemName: "pin.fill")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(.tertiary)
                    .rotationEffect(.degrees(40))
                    .transition(.opacity)
            }
            TimelineView(.everyMinute) { context in
                Text(ShortAge.string(from: thread.updatedAt, to: context.date))
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
            // Instant, as a native sidebar's selection is.
            app.selectedThreadID = thread.id
        }
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .animation(.snappy, value: thread.hasUnread)
        .animation(.snappy, value: thread.isPinned)
        .animation(.easeInOut(duration: 0.25), value: thread.title)
        .contextMenu { ThreadMenuItems(thread: thread, renaming: $renaming) }
    }
}

/// Everything you can do to a thread, shared by its context menu and its "…" button.
private struct ThreadMenuItems: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    @Binding var renaming: Bool

    var body: some View {
        if !thread.isSettled {
            Button(thread.isPinned ? "Unpin" : "Pin", systemImage: thread.isPinned ? "pin.slash" : "pin") {
                withAnimation(.snappy) { app.togglePinned(thread) }
            }
        }
        Button("Rename…", systemImage: "pencil") { renaming = true }
        Button(thread.isGeneratingTitle ? "Regenerating Title…" : "Regenerate Title", systemImage: "sparkles") {
            app.generateTitle(for: thread, from: .conversation)
        }
        .disabled(thread.isGeneratingTitle || app.titleGeneration.provider == nil || thread.sessionID == nil && thread.entries.isEmpty)
        Divider()
        Button(thread.isSettled ? "Un-settle" : "Settle", systemImage: thread.isSettled ? "arrow.uturn.backward.circle" : "checkmark.circle") {
            withAnimation(.snappy) { app.toggleSettled(thread) }
        }
        Toggle(isOn: Binding(get: { thread.autoSettle }, set: { app.setAutoSettle($0, thread) })) {
            Label("Settle Automatically When Idle", systemImage: "clock.arrow.circlepath")
        }
        Divider()
        Button("Copy Session ID", systemImage: "doc.on.doc") {
            if let session = thread.sessionID { Pasteboard.copy(session) }
        }
        .disabled(thread.sessionID == nil)
        .help("Claude Code's session ID, for `claude --resume`")
        Button("Copy Thread ID", systemImage: "number") { Pasteboard.copy(thread.id.uuidString) }
        Divider()
        Button("Delete Thread", systemImage: "trash", role: .destructive) {
            withAnimation(.snappy) { app.deleteThread(thread) }
        }
        .help("Undo brings it back (⌘Z)")
    }
}

/// Edits a thread's title in place. Return saves, Escape cancels, clicking away saves.
private struct RenameField: View {
    let thread: ThreadModel
    @Binding var isPresented: Bool
    @State private var text = ""
    @FocusState private var focused: Bool

    var body: some View {
        TextField("Title", text: $text)
            .textFieldStyle(.plain)
            .focused($focused)
            .onSubmit(commit)
            .onExitCommand { isPresented = false }
            .onChange(of: focused) { _, isFocused in
                if !isFocused { commit() }
            }
            .onAppear {
                text = thread.title
                focused = true
            }
    }

    private func commit() {
        guard isPresented else { return }
        thread.rename(text)
        isPresented = false
    }
}

/// Pin, settle and a "…" menu, shown while the pointer is over a row.
private struct RowActions: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    @Binding var renaming: Bool

    var body: some View {
        HStack(spacing: 8) {
            if !thread.isSettled {
                Button(thread.isPinned ? "Unpin" : "Pin", systemImage: thread.isPinned ? "pin.slash" : "pin") {
                    withAnimation(.snappy) { app.togglePinned(thread) }
                }
                .help(thread.isPinned ? "Unpin" : "Pin to the top")
            }
            Button(thread.isSettled ? "Un-settle" : "Settle", systemImage: thread.isSettled ? "arrow.uturn.backward.circle" : "checkmark.circle") {
                withAnimation(.snappy) { app.toggleSettled(thread) }
            }
            .help(thread.isSettled ? "Move back to active work" : "Settle: move finished work to the shelf and stop its session")
            Menu {
                ThreadMenuItems(thread: thread, renaming: $renaming)
            } label: {
                Image(systemName: "ellipsis")
            }
            .menuIndicator(.hidden)
            .menuStyle(.button)
            .fixedSize()
            .help("More")
        }
        .labelStyle(.iconOnly)
        .buttonStyle(.borderless)
        .font(.system(size: 11, weight: .medium))
        .foregroundStyle(.secondary)
    }
}

/// "Settled · 3" at the end of a project's threads; opens the shelf inline.
private struct SettledShelfToggle: View {
    @Environment(AppModel.self) private var app
    let project: Project
    let count: Int
    @State private var hovering = false

    private var expanded: Bool { app.expandedShelves.contains(project.id) }

    var body: some View {
        Button {
            withAnimation(.snappy(duration: 0.25)) { app.setShelfExpanded(!expanded, for: project) }
        } label: {
            HStack(spacing: SidebarMetrics.gap) {
                Image(systemName: "checkmark.circle")
                    .font(.system(size: 10, weight: .medium))
                    .frame(width: SidebarMetrics.icon)
                    .padding(.leading, SidebarMetrics.chevron + SidebarMetrics.gap)
                Text("Settled")
                Text("\(count)")
                    .monospacedDigit()
                    .foregroundStyle(.tertiary)
                Image(systemName: "chevron.right")
                    .font(.system(size: 8, weight: .bold))
                    .rotationEffect(.degrees(expanded ? 90 : 0))
                    .opacity(hovering || expanded ? 1 : 0)
                Spacer()
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            .padding(.horizontal, SidebarMetrics.rowPadding - 4)
            .padding(.vertical, 4)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .help(expanded ? "Hide settled threads" : "Show settled threads")
    }
}

/// Working: a turning Claude mark. Waiting on you: a pulsing hand. Waiting on a job
/// Claude started: an hourglass. Unread: a blue dot.
private struct ThreadStatus: View {
    let thread: ThreadModel

    private enum State: Equatable {
        case idle, running, needsApproval, waitingOnJobs, unread
    }

    private var state: State {
        if !thread.pendingPermissions.isEmpty { return .needsApproval }
        if thread.isWorking { return .running }
        if thread.isWaitingOnJobs { return .waitingOnJobs }
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
            case .waitingOnJobs:
                Image(systemName: "hourglass")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .transition(.scale.combined(with: .opacity))
            case .unread:
                Circle()
                    .fill(Color.accentColor)
                    .frame(width: 7, height: 7)
                    .transition(.scale.combined(with: .opacity))
            case .idle:
                Circle()
                    .fill(.tertiary)
                    .frame(width: 6, height: 6)
            }
        }
        .animation(.spring(response: 0.3, dampingFraction: 0.75), value: state)
        .help(helpText)
    }

    private var helpText: String {
        switch state {
        case .running: "Working…"
        case .needsApproval: "Waiting for your approval"
        case .waitingOnJobs: thread.waitingJobs.count == 1 ? "Claude is waiting on a background job" : "Claude is waiting on \(thread.waitingJobs.count) background jobs"
        case .unread: "New reply"
        case .idle: ""
        }
    }
}

// MARK: - Footer

/// Usage limits, this app's memory, a refresh, and Settings. Details live in tooltips;
/// clicking the box opens the usage view.
private struct UsageFooter: View {
    @Environment(AppModel.self) private var app

    /// Always show these two, even before the first reading, so the footer never jumps.
    private static let primaryWindows = ["five_hour", "seven_day"]

    private var windows: [(name: String, window: RateLimitInfo.Window?)] {
        let reported = app.rateLimit?.windows ?? []
        let extra = reported.filter { !Self.primaryWindows.contains($0.name) }
        return Self.primaryWindows.map { name in (name, reported.first { $0.name == name }) }
            + extra.map { ($0.name, $0) }
    }

    @State private var isHovered = false

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            UpdateBanner()
            TimelineView(.everyMinute) { context in
                VStack(spacing: 7) {
                    ForEach(windows, id: \.name) { item in
                        LimitMeter(name: item.name, window: item.window, now: context.date)
                    }
                }
            }
            .accessibilityElement(children: .combine)
            .accessibilityAddTraits(.isButton)
            .accessibilityHint("Shows usage")
            .accessibilityAction { app.tokenUsage.open() }
            HStack(spacing: 10) {
                TimelineView(.periodic(from: .now, by: 2)) { _ in
                    Label(MemoryFootprint.formatted, systemImage: "memorychip")
                        .font(.caption2.monospacedDigit())
                        .foregroundStyle(.tertiary)
                        .contentTransition(.numericText())
                        .help("This app's memory use")
                }
                VersionButton()
                Spacer()
                RefreshLimitsButton()
                SettingsLink {
                    Image(systemName: "gearshape")
                }
                .buttonStyle(.borderless)
                .help("Settings (⌘,)")
            }
            .font(.callout)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        // Its buttons keep their clicks; anywhere else opens the usage view.
        .contentShape(.rect(cornerRadius: 14))
        .onTapGesture { app.tokenUsage.open() }
        .onHover { isHovered = $0 }
        .background(.fill.quaternary.opacity(isHovered ? 1 : 0), in: .rect(cornerRadius: 14))
        .animation(.easeOut(duration: 0.12), value: isHovered)
        .animation(.smooth(duration: 0.35), value: app.updater.phase.bannerKind)
        .animation(.smooth(duration: 0.35), value: app.updater.announcesUpToDate)
        .animation(.smooth(duration: 0.35), value: app.updater.updatedFrom)
        .glassEffect(.regular, in: .rect(cornerRadius: 14))
        .padding(10)
    }
}

/// "Settled “Fix login” · Undo", for a few seconds after a thread action.
private struct UndoToast: View {
    @Environment(AppModel.self) private var app
    let notice: AppModel.UndoNotice

    var body: some View {
        HStack(spacing: 8) {
            Text(notice.message)
                .font(.callout)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 4)
            Button("Undo") { notice.undo() }
                .buttonStyle(.borderless)
                .fontWeight(.semibold)
                .help("Undo (⌘Z)")
            Button("Dismiss", systemImage: "xmark") { app.dismissUndoNotice() }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
                .font(.caption)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .glassEffect(.regular, in: .capsule)
        .padding(.horizontal, 10)
        .padding(.top, 4)
    }
}

private struct RefreshLimitsButton: View {
    @Environment(AppModel.self) private var app

    private var help: String {
        if let error = app.limitsRefreshError { return "Couldn't refresh: \(error)" }
        var text = "Refresh usage limits (sends Claude a tiny Haiku request)"
        if let updated = app.rateLimitUpdatedAt {
            text += "\nUpdated \(updated.formatted(.relative(presentation: .named)))"
        }
        return text
    }

    var body: some View {
        Button {
            Task { await app.refreshLimits() }
        } label: {
            Image(systemName: app.limitsRefreshError == nil ? "arrow.clockwise" : "exclamationmark.arrow.trianglehead.counterclockwise")
                .foregroundStyle(app.limitsRefreshError == nil ? AnyShapeStyle(.secondary) : AnyShapeStyle(Color.orange))
                .symbolEffect(.rotate, options: .repeat(.continuous), isActive: app.isRefreshingLimits)
                .contentTransition(.symbolEffect(.replace))
        }
        .buttonStyle(.borderless)
        .disabled(app.isRefreshingLimits)
        .help(help)
    }
}

/// `5h ━━━━━━──────── 6%  2h 14m`: used, and how long until it resets.
struct LimitMeter: View {
    let name: String
    let window: RateLimitInfo.Window?
    let now: Date

    /// A saved reading from before the window reset says nothing about now.
    private var hasReset: Bool { window?.resetsAt.map { $0 <= now } ?? false }
    private var utilization: Double? {
        guard let window else { return nil }
        return hasReset ? 0 : min(max(window.utilization, 0), 1)
    }

    private var label: String {
        switch name {
        case "five_hour": "5h"
        case "seven_day": "7d"
        case "seven_day_opus": "Opus"
        case "seven_day_sonnet": "Sonnet"
        default: String(name.prefix(4)).capitalized
        }
    }

    private var title: String { Self.title(name) }

    static func title(_ name: String) -> String {
        switch name {
        case "five_hour": "5-hour limit"
        case "seven_day": "Weekly limit"
        case "seven_day_opus": "Weekly Opus limit"
        case "seven_day_sonnet": "Weekly Sonnet limit"
        default: name.replacing("_", with: " ").capitalized
        }
    }

    private var tint: Color { Self.tint(utilization ?? 0) }

    static func tint(_ utilization: Double) -> Color {
        switch utilization {
        case ..<0.7: .accentColor
        case ..<0.9: .orange
        default: .red
        }
    }

    /// Here rather than on the box: a tooltip there would be every button's in it too.
    private var help: String { reading + "\nClick for usage (⇧⌘U)" }

    private var reading: String {
        guard let window else { return "\(title): no reading yet. Refresh to check." }
        if hasReset { return "\(title): reset since the last reading. Refresh to update." }
        guard let resetsAt = window.resetsAt else { return title }
        return "\(title): resets \(resetsAt.formatted(.relative(presentation: .named))), \(resetsAt.formatted(date: .abbreviated, time: .shortened))"
    }

    /// Time left until the reset, short: "42m", "2h 14m", "3d 4h".
    static func countdown(to date: Date, from now: Date) -> String {
        let minutes = max(Int(date.timeIntervalSince(now) / 60), 0)
        let days = minutes / 1440, hours = minutes % 1440 / 60, rest = minutes % 60
        if days > 0 { return hours > 0 ? "\(days)d \(hours)h" : "\(days)d" }
        if hours > 0 { return rest > 0 ? "\(hours)h \(rest)m" : "\(hours)h" }
        return minutes > 0 ? "\(minutes)m" : "now"
    }

    var body: some View {
        HStack(spacing: 8) {
            Text(label)
                .font(.caption2.weight(.semibold))
                .foregroundStyle(.secondary)
                .frame(width: 36, alignment: .leading)
            GeometryReader { proxy in
                ZStack(alignment: .leading) {
                    Capsule().fill(.fill.tertiary)
                    if let utilization {
                        Capsule()
                            .fill(tint)
                            .frame(width: max(proxy.size.width * utilization, 4))
                    }
                }
            }
            .frame(height: 4)
            Text(utilization.map { $0.formatted(.percent.precision(.fractionLength(0))) } ?? "–")
                .font(.caption2.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 30, alignment: .trailing)
                .contentTransition(.numericText())
            // A fixed column, so the bars stay the same length whatever the countdown says.
            Text(window?.resetsAt.flatMap { hasReset ? nil : Self.countdown(to: $0, from: now) } ?? "")
                .font(.caption2.monospacedDigit())
                .foregroundStyle(.tertiary)
                .lineLimit(1)
                .frame(width: 40, alignment: .trailing)
                .contentTransition(.numericText())
        }
        .contentShape(.rect)
        .help(help)
        .animation(.smooth(duration: 0.4), value: utilization)
    }
}

enum ShortAge {
    /// Compact and relative: "now", "5m", "3h", "2d", "3w", "1mo", "2y".
    static func string(from date: Date, to now: Date) -> String {
        let seconds = max(now.timeIntervalSince(date), 0)
        let minutes = seconds / 60, hours = minutes / 60, days = hours / 24
        switch seconds {
        case ..<60: return "now"
        case ..<3_600: return "\(Int(minutes))m"
        case ..<86_400: return "\(Int(hours))h"
        case ..<(7 * 86_400): return "\(Int(days))d"
        case ..<(30 * 86_400): return "\(Int(days / 7))w"
        case ..<(365 * 86_400): return "\(max(Int(days / 30), 1))mo"
        default: return "\(Int(days / 365))y"
        }
    }
}
