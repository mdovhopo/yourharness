import HarnessCore
import SwiftUI

/// A thread: its transcript, and around it what stays from thread to thread (the composer,
/// the toolbar, the title). Only the transcript is made anew when you switch, so switching
/// swaps rows rather than rebuilding the window's content, as Mail does.
struct ThreadView: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    @State private var dropTargeted = false
    /// The threads you were in last, newest first: their transcripts stay made (hidden), so
    /// going back to one shows it at once, where you were.
    @State private var recent: [UUID] = []
    private static let keptTranscripts = 4
    /// The thread's whole height, and the composer's with what sits on it: a new thread's
    /// composer waits in the middle.
    @State private var areaHeight: CGFloat = 0
    @State private var composerHeight: CGFloat = 0

    /// Nothing said yet: the composer sits in the middle, asking what to work on, and goes
    /// down to its place when you send.
    private var isFresh: Bool {
        thread.sessionID == nil && thread.entries.isEmpty && thread.queuedMessages.isEmpty && thread.phase != .running
    }

    private var keptThreads: [ThreadModel] {
        let ids = recent.contains(thread.id) ? recent : [thread.id] + recent
        return ids.compactMap { id in id == thread.id ? thread : app.threads.first { $0.id == id } }
    }

    var body: some View {
        ZStack {
            ForEach(keptThreads) { kept in
                let shown = kept.id == thread.id
                TranscriptView(thread: kept)
                    .environment(\.issueLinker, app.issueLinker(for: kept))
                    .environment(kept)
                    .opacity(shown ? 1 : 0)
                    .allowsHitTesting(shown)
                    .accessibilityHidden(!shown)
                    .zIndex(shown ? 1 : 0)
            }
        }
            .onChange(of: thread.id, initial: true) {
                recent = Array(([thread.id] + recent.filter { $0 != thread.id }).prefix(Self.keptTranscripts))
                let id = thread.id
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
                .animation(.spring(response: 0.55, dampingFraction: 0.86), value: isFresh)
            }
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { areaHeight = $0 }
            .onDrop(of: [.fileURL, .image], isTargeted: $dropTargeted) { providers in
                Task { thread.attach(await ImageIntake.sources(from: providers)) }
                return true
            }
            .overlay {
                if dropTargeted {
                    RoundedRectangle(cornerRadius: 16)
                        .strokeBorder(.tint, style: StrokeStyle(lineWidth: 2, dash: [6, 4]))
                        .background(.tint.opacity(0.05), in: .rect(cornerRadius: 16))
                        .overlay { Label("Drop images to attach", systemImage: "photo.badge.plus").font(.title3) }
                        .padding(8)
                        .allowsHitTesting(false)
                        .transition(.opacity)
                }
            }
            .animation(.easeOut(duration: 0.15), value: dropTargeted)
            .environment(thread)
            // Links to this Mac (the app being built) open in the thread's browser; ⌘-click
            // for your default browser.
            .environment(\.openURL, OpenURLAction { url in
                guard app.browserTools, LocalServers.isLocal(url), !NSEvent.modifierFlags.contains(.command) else { return .systemAction }
                thread.openInBrowser(url)
                return .handled
            })
            .navigationTitle(thread.title)
            .navigationSubtitle(subtitle)
            .toolbar {
                if let project = app.project(id: thread.projectID), let workspace = app.workspace(for: project) {
                    ToolbarItem {
                        ThreadWorktreesButton(thread: thread, project: project, workspace: workspace)
                    }
                }
                ToolbarItem {
                    ThreadPullRequestsButton(thread: thread)
                }
            }
    }

    private var subtitle: String {
        var parts = [app.project(id: thread.projectID)?.name ?? ""]
        // Running in a worktree: which one.
        if let folder = thread.workingDirectory { parts.append((folder as NSString).lastPathComponent) }
        if let model = thread.activeModelName { parts.append(model) }
        return parts.joined(separator: " · ")
    }
}

/// The scrolling transcript of one thread, with where you are in it.
private struct TranscriptView: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel

    init(thread: ThreadModel) {
        self.thread = thread
        // Where you were, from the first frame: no landing at the end and jumping back.
        _position = State(initialValue: thread.scrollAnchor.map { ScrollPosition(id: $0, anchor: .bottom) }
            ?? ScrollPosition(idType: String.self, edge: .bottom))
        _followsBottom = State(initialValue: thread.scrollAnchor == nil)
    }
    @State private var visibleHeight: CGFloat = 0
    /// Shorter than the screen: only then does the transcript get a minimum height.
    @State private var isShort = true
    /// Where you are: the row at the bottom edge, not a pixel offset.
    @State private var position: ScrollPosition
    /// Keep the newest output in view as the transcript grows: while history loads, while
    /// a reply streams. Your own scrolling turns it off; stopping at the bottom turns it
    /// back on.
    @State private var followsBottom = true
    @State private var isAtBottom = true
    /// Rows on screen now, for jumping between your messages from where you are.
    @State private var visibleRowIDs: [String] = []
    private static let typingID = "typing-indicator"

    /// Claude is working and nothing is streaming in. Between two of its steps (text ending,
    /// a tool call starting) that's true for a moment only, so there it waits a little
    /// before showing: a row flashing in and out moves everything above it.
    private var wantsTypingIndicator: Bool {
        thread.phase == .running && thread.pendingPermissions.isEmpty && !(thread.entries.last?.isStreaming ?? false)
    }
    @State private var typingIndicatorDue = false

    private var showsTypingIndicator: Bool { wantsTypingIndicator && typingIndicatorDue }

    /// The row the conversation ends with right now, as shown: a finished turn's last
    /// entry may be folded away.
    private var lastRowID: String? {
        showsTypingIndicator ? Self.typingID : thread.transcriptDisplayRows.last?.id
    }

    var body: some View {
        transcript
            .task(id: wantsTypingIndicator) {
                typingIndicatorDue = false
                guard wantsTypingIndicator else { return }
                // Right after your message it shows at once.
                if thread.entries.last?.kind != .user {
                    try? await Task.sleep(for: .milliseconds(400))
                    guard !Task.isCancelled else { return }
                }
                typingIndicatorDue = true
            }
            .task {
                await thread.loadTranscriptSoon()
                thread.computeFootprintIfNeeded()
                await restorePlace()
            }
            .onDisappear { rememberPlace() }
    }

    private var transcript: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) { rows }
                .scrollTargetLayout()
                // Keyed on count, so streaming text doesn't animate, only new rows.
                .animation(.smooth(duration: 0.32), value: thread.entries.count)
                .animation(.smooth(duration: 0.25), value: showsTypingIndicator)
                .frame(maxWidth: 820)
                .frame(maxWidth: .infinity)
                .padding(.horizontal, 28)
                .padding(.vertical, 24)
                .frame(minHeight: isShort ? visibleHeight : nil, alignment: .bottom)
        }
        // Your place is a row, not an offset. The lazy stack keeps revising its height
        // estimates for rows off screen; held by offset, the view jumped every time, and
        // pinning to the bottom pulled you back down. Held by the row at the bottom edge,
        // that row stays put. Following the conversation is keeping the last row there.
        .scrollPosition($position, anchor: .bottom)
        .defaultScrollAnchor(.bottom, for: .initialOffset)
        .onScrollGeometryChange(for: ScrollState.self) { geometry in
            // The reported content height counts the toolbar and composer insets; at the
            // very bottom, offset + container + top inset equals it (measured both ways).
            ScrollState(offset: geometry.contentOffset.y, contentHeight: geometry.contentSize.height,
                        containerHeight: geometry.containerSize.height,
                        isAtBottom: geometry.contentOffset.y + geometry.containerSize.height + geometry.contentInsets.top >= geometry.contentSize.height - 40)
        } action: { old, new in
            isAtBottom = new.isAtBottom
            isShort = new.contentHeight <= new.containerHeight + 0.5
            // Moved while the content stayed the same size: that's scrolling, by any means
            // (a scroll bar drag may report no phase), so follow only if it ended at the bottom.
            if new.contentHeight == old.contentHeight, new.offset != old.offset { followsBottom = new.isAtBottom }
        }
        .onScrollPhaseChange { old, phase in
            if phase != .idle {
                followsBottom = false
            } else if old != .idle {
                followsBottom = isAtBottom
            }
        }
        // A new row: show it if you're following. Sending a message always takes you to it.
        .onChange(of: lastRowID) { _, id in
            guard let id else { return }
            if thread.entries.last?.id == id, thread.entries.last?.kind == .user { followsBottom = true }
            if followsBottom { position.scrollTo(id: id, anchor: .bottom) }
        }
        .modifier(ScrollProbe(position: $position))
        // ⌘F: searched a moment after you stop typing, and again as the thread grows.
        .task(id: FindKey(query: thread.find.isOpen ? thread.find.query : "", rows: thread.entries.count)) {
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
            followsBottom = false
            // A match inside a folded turn: open it first, then go there.
            thread.reveal(id)
            Task { @MainActor in
                withAnimation(.smooth(duration: 0.25)) { position.scrollTo(id: id, anchor: .center) }
            }
        }
        .onScrollTargetVisibilityChange(idType: String.self, threshold: 0.3) { visibleRowIDs = $0 }
        .onChange(of: thread.messageJump) { _, jump in
            if let jump { self.jump(jump.direction) }
        }
        .overlay(alignment: .bottomTrailing) {
            if !isShort {
                MessageJumpControls(thread: thread, canGoPrevious: hasMessageAbove, canGoNext: !isAtBottom)
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
        // At least as tall as the scroll view, messages at the bottom: a short thread
        // would otherwise sit on a tall top inset, and the toolbar's edge effect tints all of it.
        .onGeometryChange(for: CGFloat.self) { proxy in
            proxy.size.height
        } action: { height in
            visibleHeight = height
        }
    }

    @ViewBuilder private var rows: some View {
        let firstID = thread.entries.first?.id
        ForEach(thread.transcriptDisplayRows) { row in
            TranscriptRowView(row: row, thread: thread, isFirst: row.id == firstID)
                .modifier(ScrollProbe.Row(id: row.id))
            .transition(.asymmetric(
                insertion: .opacity.combined(with: .offset(y: 12)),
                removal: .opacity
            ))
        }
        if showsTypingIndicator {
            TypingIndicator(text: thread.activity ?? "Working…", since: thread.entries.last(where: \.startsTurn)?.date)
                .id(Self.typingID)
                .transition(.opacity.combined(with: .scale(scale: 0.96, anchor: .leading)))
        }
    }

    /// Back where you were in this thread, or at the end if that's where you were. The
    /// position starts there; this only settles it once a transcript that loaded after the
    /// first frame is in.
    private func restorePlace() async {
        if let anchor = thread.scrollAnchor { thread.showTurns(including: anchor) }
        if let anchor = thread.scrollAnchor, thread.transcriptDisplayRows.contains(where: { $0.id == anchor }) {
            followsBottom = false
            position.scrollTo(id: anchor, anchor: .bottom)
        } else {
            followsBottom = true
            if let last = lastRowID { position.scrollTo(id: last, anchor: .bottom) }
        }
    }

    private func rememberPlace() {
        guard !isAtBottom, !followsBottom else {
            thread.scrollAnchor = nil
            // Left at the end: next time it opens with just the latest turns again.
            thread.shownTurns = ThreadModel.turnsPerPage
            return
        }
        let order = Dictionary(thread.transcriptDisplayRows.enumerated().map { ($1.id, $0) }, uniquingKeysWith: { first, _ in first })
        thread.scrollAnchor = visibleRowIDs.max { (order[$0] ?? -1) < (order[$1] ?? -1) }
    }

    /// One of your messages above the top of the screen, shown or not yet.
    private var hasMessageAbove: Bool {
        let rows = thread.transcriptDisplayRows
        let visible = Set(visibleRowIDs)
        guard let top = rows.firstIndex(where: { visible.contains($0.id) }) else { return false }
        return rows[..<top].contains { row in
            if case .earlier = row { return true }
            return row.entry?.kind == .user
        }
    }

    /// To your previous message above the top of the screen, or your next one below it
    /// (past the last, to the end).
    private func jump(_ direction: MessageJump.Direction) {
        let rows = thread.transcriptDisplayRows
        guard !rows.isEmpty else { return }
        let index = Dictionary(rows.enumerated().map { ($1.id, $0) }, uniquingKeysWith: { first, _ in first })
        let top = visibleRowIDs.compactMap { index[$0] }.min() ?? rows.count - 1
        let isMessage: (TranscriptRow) -> Bool = { $0.entry?.kind == .user }
        switch direction {
        case .previous:
            followsBottom = false
            // Past the turns shown: show more, then go on from there.
            if !rows[..<top].contains(where: isMessage), case .earlier = rows.first {
                thread.showEarlierTurns()
                Task { @MainActor in jump(.previous) }
                return
            }
            let target = rows[..<top].last(where: isMessage) ?? rows[0]
            withAnimation(.smooth(duration: 0.3)) { position.scrollTo(id: target.id, anchor: .top) }
        case .next:
            if top + 1 < rows.count, let target = rows[(top + 1)...].first(where: isMessage) {
                followsBottom = false
                withAnimation(.smooth(duration: 0.3)) { position.scrollTo(id: target.id, anchor: .top) }
            } else if let last = lastRowID {
                followsBottom = true
                withAnimation(.smooth(duration: 0.3)) { position.scrollTo(id: last, anchor: .bottom) }
            }
        }
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

private struct TranscriptRowView: View {
    let row: TranscriptRow
    let thread: ThreadModel
    let isFirst: Bool

    var body: some View {
        // In a stack, so the row is one view whichever case it is: the lazy stack can't
        // tell that about a bare switch, and builds every row to find out (10,000 rows: 130 MB more).
        VStack(alignment: .leading, spacing: 0) {
            switch row {
            case .entry(let entry):
                EntryView(entry: entry)
                    .environment(\.findHighlight, thread.find.highlight(for: entry.id))
                    // A little more room above each turn, so they read as separate.
                    .padding(.top, entry.startsTurn && !isFirst ? 12 : 0)
            case .work(let work):
                WorkSummaryRow(work: work) {
                    withAnimation(.smooth(duration: 0.3)) {
                        if work.isExpanded { thread.expandedTurns.remove(work.turnID) } else { thread.expandedTurns.insert(work.turnID) }
                    }
                }
            case .earlier(let count):
                EarlierTurnsRow(count: count) { thread.showEarlierTurns() }
            }
        }
    }
}

private struct FindKey: Equatable {
    var query: String
    var rows: Int
}

private struct ScrollState: Equatable {
    var offset: CGFloat
    var contentHeight: CGFloat
    var containerHeight: CGFloat
    var isAtBottom: Bool
}

struct TypingIndicator: View {
    var text = "Working…"
    /// When the turn started: shows how long it's been going.
    var since: Date?
    @State private var dimmed = false

    var body: some View {
        HStack(spacing: 8) {
            SpinningClaudeMark(size: 14)
            Text(text)
                .foregroundStyle(.secondary)
                .opacity(dimmed ? 0.45 : 1)
                .animation(.easeInOut(duration: 1.1).repeatForever(), value: dimmed)
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
        .onAppear { dimmed = true }
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
