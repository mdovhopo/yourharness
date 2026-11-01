import HarnessCore
import SwiftUI

/// A thread: its transcript, and around it what stays from thread to thread (the composer,
/// the toolbar, the title). The transcript lives in `TranscriptDeck`, so switching swaps it
/// rather than rebuilding the window's content, as Mail does.
struct ThreadView: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    @State private var dropTargeted = false
    /// The thread's whole height, and the composer's with what sits on it: a new thread's
    /// composer waits in the middle.
    @State private var areaHeight: CGFloat = 0
    @State private var composerHeight: CGFloat = 0
    /// The thread it was last updated for: going to another isn't a change to animate. The
    /// prompt of a new thread faded out over the rows of the one you went to.
    @State private var settledThread: UUID?

    /// Nothing said yet: the composer sits in the middle, asking what to work on, and goes
    /// down to its place when you send.
    private var isFresh: Bool {
        thread.sessionID == nil && thread.hasNoEntries && thread.queuedMessages.isEmpty && thread.phase != .running
    }

    var body: some View {
        // Under the toolbar and the composer: the transcript scrolls beneath them.
        TranscriptDeck(thread: thread, bottomInset: composerHeight)
            .ignoresSafeArea(edges: .vertical)
            .onChange(of: thread.id, initial: true) {
                let id = thread.id
                settledThread = id
                DispatchQueue.main.async { SwitchProbe.threadAppeared(id) }
            }
            .safeAreaInset(edge: .bottom, spacing: 0) {
                VStack(spacing: 10) {
                    if isFresh {
                        StartPrompt(thread: thread)
                            .transition(.opacity.combined(with: .scale(scale: 0.97, anchor: .bottom)))
                    }
                    ForEach(thread.pendingPermissions) { request in
                        RequestCard(request: request, thread: thread)
                            .transition(.move(edge: .bottom).combined(with: .opacity))
                    }
                    ComposerView(thread: thread)
                }
                .frame(maxWidth: 820)
                .padding(.horizontal, 20)
                .padding(.bottom, 16)
                .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { composerHeight = $0 }
                // The same composer, lifted to the middle: it keeps its focus and draft on the way down.
                .offset(y: isFresh ? -max((areaHeight - composerHeight) / 2, 0) : 0)
                .animation(.snappy, value: thread.pendingPermissions)
                .animation(settledThread == thread.id ? .spring(response: 0.55, dampingFraction: 0.86) : nil, value: isFresh)
            }
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { areaHeight = $0 }
            .onDrop(of: [.fileURL, .image], isTargeted: $dropTargeted) { providers in
                Task { thread.attach(await AttachmentIntake.sources(from: providers)) }
                return true
            }
            .overlay {
                if dropTargeted {
                    RoundedRectangle(cornerRadius: 16)
                        .strokeBorder(.tint, style: StrokeStyle(lineWidth: 2, dash: [6, 4]))
                        .background(.tint.opacity(0.05), in: .rect(cornerRadius: 16))
                        .overlay { Label("Drop files to attach", systemImage: "paperclip").font(.title3) }
                        .padding(8)
                        .allowsHitTesting(false)
                        .transition(.opacity)
                }
            }
            .animation(.easeOut(duration: 0.15), value: dropTargeted)
            .environment(thread)
            // Links to this Mac (the app being built) open in the thread's browser; ⌘-click
            // for your default browser. Files open in the Files tab; ⌘-click in their app.
            .environment(\.openURL, OpenURLAction { url in
                if FileLinkAction.open(url, in: thread) { return .handled }
                guard app.browserTools, LocalServers.isLocal(url), !NSEvent.modifierFlags.contains(.command) else { return .systemAction }
                thread.openInBrowser(url)
                return .handled
            })
            // Lists the thread's files once, off the main thread, for linking paths.
            .task(id: thread.fileRoot?.path) { await app.loadFileLinker(for: thread) }
            .navigationTitle(thread.title)
            .navigationSubtitle(subtitle)
            .task(id: "\(thread.id)-\(app.memoryRevision(for: thread.projectID))") { await ThreadTasksButton.load(thread, app: app) }
            .modifier(SelectedThreadToolbar())
    }

    private var subtitle: String {
        var parts = [app.project(id: thread.projectID)?.name ?? ""]
        // Running in a worktree: which one.
        if let folder = thread.workingDirectory { parts.append((folder as NSString).lastPathComponent) }
        if let model = thread.activeModelName { parts.append(model) }
        return parts.joined(separator: " · ")
    }
}

/// A toolbar that isn't made anew with the thread: a change to `.toolbar`'s content rebuilds
/// its items and lays the window's toolbar out again, a good part of a switch, and in a layout
/// pass that asks for another pass. Each item finds the thread itself.
private struct SelectedThreadToolbar: ViewModifier {
    func body(content: Content) -> some View {
        content.toolbar {
            ToolbarItem { SelectedThread { WorktreesItem(thread: $0) } }
            ToolbarItem { SelectedThread { ThreadTasksButton(thread: $0) } }
            ToolbarItem { SelectedThread { ThreadPullRequestsButton(thread: $0) } }
        }
    }
}

private struct SelectedThread<Content: View>: View {
    @Environment(AppModel.self) private var app
    @ViewBuilder let content: (ThreadModel) -> Content

    var body: some View {
        if let thread = app.selectedThread { content(thread) }
    }
}

private struct WorktreesItem: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel

    var body: some View {
        if let project = app.project(id: thread.projectID), let workspace = app.workspace(for: project) {
            ThreadWorktreesButton(thread: thread, project: project, workspace: workspace)
        }
    }
}

/// The transcript of one thread: the list, and what floats over it.
struct TranscriptView: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    /// Where it's shown: on screen, or parked for coming back to.
    let card: TranscriptCard
    @State private var hasBeenShown = false
    @State private var showsLoading = false

    private var list: TranscriptListView { card.list }
    private var place: TranscriptPlace { card.list.place }

    var body: some View {
        TranscriptListHost(list: list)
            // Under the toolbar and the composer: the list keeps its rows clear of them.
            .ignoresSafeArea(edges: .vertical)
            .task {
                await thread.loadTranscriptSoon()
                thread.computeFootprintIfNeeded()
            }
            .task(id: card.isShown) {
                guard card.isShown, hasBeenShown else {
                    hasBeenShown = hasBeenShown || card.isShown
                    return
                }
                // Parked meanwhile, its transcript may have been let go (only the last
                // few threads you looked at stay in memory).
                await thread.loadTranscriptSoon()
            }
            .onChange(of: app.issueLinker(for: thread)) { _, linker in card.rows.issueLinker = linker }
            .onChange(of: app.fileLinker(for: thread)) { _, linker in card.rows.fileLinker = linker }
            // ⌘F: searched a moment after you stop typing, and again as the thread grows.
            .task(id: FindKey(query: thread.find.isOpen ? thread.find.query : "", rows: thread.find.isOpen ? thread.entries.count : 0)) {
                let query = thread.find.query.trimmingCharacters(in: .whitespaces)
                guard thread.find.isOpen, !query.isEmpty else { return }
                try? await Task.sleep(for: .milliseconds(120))
                guard !Task.isCancelled else { return }
                let found = await ThreadFind.search(query, in: thread.findableText)
                guard !Task.isCancelled else { return }
                thread.find.update(found)
            }
            .onChange(of: thread.find.current) { _, id in
                guard let id else { return }
                // A match inside a folded turn: open it first, then go there.
                thread.reveal(id)
                list.show(rowID: id)
            }
            .onChange(of: thread.messageJump) { _, jump in
                if let jump { list.jump(jump.direction) }
            }
            .overlay {
                if showsLoading, thread.hasNoEntries {
                    ProgressView("Loading conversation…")
                        .controlSize(.small)
                        .foregroundStyle(.secondary)
                        .transition(.opacity)
                }
            }
            // Only when it takes a moment: a thread that opens at once shouldn't flash this.
            .task(id: thread.isLoadingTranscript && thread.hasNoEntries) {
                guard thread.isLoadingTranscript, thread.hasNoEntries else {
                    withAnimation(.easeOut(duration: 0.12)) { showsLoading = false }
                    return
                }
                try? await Task.sleep(for: .milliseconds(120))
                guard !Task.isCancelled else { return }
                withAnimation(.easeOut(duration: 0.12)) { showsLoading = true }
            }
            .overlay(alignment: .bottomTrailing) {
                if !place.isShort {
                    MessageJumpControls(thread: thread, canGoTop: !place.isAtTop || place.hasEarlierTurns, canGoPrevious: place.hasMessageAbove, canGoNext: !place.isAtBottom)
                        .padding(.trailing, 14)
                        .padding(.bottom, 10)
                        .transition(.opacity)
                }
            }
            .overlay(alignment: .top) {
                if thread.find.isOpen {
                    FindBar(find: thread.find)
                        .padding(.top, 10)
                        .padding(.horizontal, 20)
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
            }
            .animation(.snappy(duration: 0.2), value: thread.find.isOpen)
    }
}

/// Over a new thread's composer: where the work happens.
private struct StartPrompt: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel

    var body: some View {
        let project = app.project(id: thread.projectID)
        VStack(spacing: 6) {
            Text(project.map { app.isHome($0) } ?? true ? "What are we working on?" : "What are we working on in \(project?.name ?? "")?")
                .font(.title2.weight(.semibold))
                .multilineTextAlignment(.center)
            Text(thread.effectiveDirectory.replacingOccurrences(of: NSHomeDirectory(), with: "~"))
                .font(.callout.monospaced())
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .padding(.bottom, 14)
        .frame(maxWidth: .infinity)
    }
}

/// At the top of a long thread: earlier turns come in as you scroll up to it.
private struct EarlierTurnsRow: View {
    let count: Int
    let load: () -> Void

    var body: some View {
        Button(action: load) {
            Label(count == 1 ? "1 earlier turn" : "\(count) earlier turns", systemImage: "arrow.up")
                .font(.callout)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 6)
        }
        .buttonStyle(.borderless)
        // Reaching it is asking for more; what's on screen stays put as they're added above.
        .onAppear { load() }
    }
}

struct TranscriptRowView: View {
    let row: TranscriptRow
    let thread: ThreadModel
    let isFirst: Bool

    var body: some View {
        switch row {
        case .entry(let entry):
            EntryView(entry: entry)
                .environment(\.findHighlight, thread.find.highlight(for: entry.id))
                // A little more room above each turn, so they read as separate.
                .padding(.top, entry.startsTurn && !isFirst ? 12 : 0)
        case .work(let work):
            WorkSummaryRow(work: work) { thread.setWork(work, expanded: !work.isExpanded) }
        case .workStep(let entry, let isLast):
            WorkStepRow(isLast: isLast) {
                EntryView(entry: entry)
                    .environment(\.findHighlight, thread.find.highlight(for: entry.id))
            }
        case .earlier(let count):
            EarlierTurnsRow(count: count) { thread.showEarlierTurns() }
        case .typing:
            TypingIndicator(text: thread.activity ?? "Working…", since: thread.turnStartedAt)
        }
    }
}

private struct FindKey: Equatable {
    var query: String
    var rows: Int
}

struct TypingIndicator: View {
    var text = "Working…"
    /// When the turn started: shows how long it's been going.
    var since: Date?

    var body: some View {
        HStack(spacing: 8) {
            SpinningClaudeMark(size: 14)
            Text(text)
                .foregroundStyle(.secondary)
                .pulsing(low: 0.45, period: 2.2)
            if let since {
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    Text(TurnDuration.format(context.date.timeIntervalSince(since)))
                        .monospacedDigit()
                        .foregroundStyle(.tertiary)
                        .contentTransition(.numericText())
                }
            }
        }
        .font(.callout)
    }
}

/// A question, a plan or a plain permission prompt, depending on the tool asking.
struct RequestCard: View {
    let request: PermissionRequest
    let thread: ThreadModel

    var body: some View {
        if request.toolName == AgentQuestion.toolName, let questions = AgentQuestion.parse(inputJSON: request.inputJSON) {
            QuestionCard(questions: questions) { answers in
                thread.answer(request, with: answers)
            } skip: {
                thread.respond(to: request, with: .deny(message: "The user chose not to answer. Continue with your best judgment, or ask in chat."))
            }
        } else if request.toolName == AgentPlan.toolName, let plan = AgentPlan.text(inputJSON: request.inputJSON) {
            PlanCard(plan: plan, thread: thread) { mode, feedback in
                thread.decidePlan(request, approve: mode, feedback: feedback)
            }
        } else {
            PermissionCard(request: request) { allow in
                thread.respond(to: request, allow: allow)
            }
        }
    }
}

struct PermissionCard: View {
    let request: PermissionRequest
    let decide: (Bool) -> Void

    /// Narrow, as in the side chat: the buttons go under the request instead of beside it.
    @State private var isNarrow = false

    var body: some View {
        let layout = isNarrow ? AnyLayout(VStackLayout(alignment: .trailing, spacing: 10)) : AnyLayout(HStackLayout(alignment: .top, spacing: 12))
        layout {
            summary.frame(maxWidth: .infinity, alignment: .leading)
            buttons
        }
        .padding(14)
        .onGeometryChange(for: Bool.self) { $0.size.width < 440 } action: { isNarrow = $0 }
        .glassEffect(.regular.tint(.orange.opacity(0.12)), in: .rect(cornerRadius: Theme.floatingRadius))
    }

    private var summary: some View {
        let presentation = ToolPresentation(name: self.request.toolName, inputJSON: self.request.inputJSON)
        return HStack(alignment: .top, spacing: 12) {
            Image(systemName: presentation.symbol)
                .font(.title3)
                .foregroundStyle(.orange)
                .frame(width: 24)
            VStack(alignment: .leading, spacing: 4) {
                Text("Allow \(presentation.verb)?")
                    .font(.headline)
                if !presentation.detail.isEmpty {
                    Text(presentation.detail)
                        .font(.callout.monospaced())
                        .lineLimit(4)
                        .textSelection(.enabled)
                }
                if let description = self.request.description, description != presentation.detail {
                    Text(description)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    private var buttons: some View {
        HStack(spacing: 8) {
            Button("Deny") { decide(false) }
                .keyboardShortcut(.delete, modifiers: .command)
                .help("Deny (⌘⌫)")
            Button("Allow") { decide(true) }
                .keyboardShortcut(.return, modifiers: .command)
                .buttonStyle(.glassProminent)
                .help("Allow (⌘↩)")
        }
        .controlSize(.large)
    }
}
