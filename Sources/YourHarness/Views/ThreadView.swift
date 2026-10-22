import HarnessCore
import SwiftUI

struct ThreadView: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    @State private var dropTargeted = false
    @State private var visibleHeight: CGFloat = 0
    /// Shorter than the screen: only then does the transcript get a minimum height.
    @State private var isShort = true
    /// Where you are: the row at the bottom edge, not a pixel offset.
    @State private var position = ScrollPosition(idType: String.self, edge: .bottom)
    /// Keep the newest output in view as the transcript grows: while history loads, while
    /// a reply streams. Your own scrolling turns it off; stopping at the bottom turns it
    /// back on.
    @State private var followsBottom = true
    @State private var isAtBottom = true
    /// Rows on screen now, for jumping between your messages from where you are.
    @State private var visibleRowIDs: [String] = []
    private static let typingID = "typing-indicator"

    private var showsTypingIndicator: Bool {
        thread.phase == .running && thread.pendingPermissions.isEmpty && !(thread.entries.last?.isStreaming ?? false)
    }

    /// The row the conversation ends with right now.
    private var lastRowID: String? {
        showsTypingIndicator ? Self.typingID : thread.entries.last?.id
    }

    var body: some View {
        transcript
            .environment(\.issueLinker, app.issueLinker(for: thread))
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
            .task {
                thread.loadTranscriptIfNeeded()
                thread.computeFootprintIfNeeded()
            }
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
            if !isShort, thread.entries.contains(where: { $0.kind == .user }) {
                MessageJumpControls(thread: thread)
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
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 10) {
                ForEach(thread.pendingPermissions) { request in
                    RequestCard(request: request, thread: thread)
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
                ComposerView(thread: thread)
            }
            .frame(maxWidth: 820)
            .padding(.horizontal, 20)
            .padding(.bottom, 16)
            .animation(.snappy, value: thread.pendingPermissions)
        }
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

    private var subtitle: String {
        var parts = [app.project(id: thread.projectID)?.name ?? ""]
        // Running in a worktree: which one.
        if let folder = thread.workingDirectory { parts.append((folder as NSString).lastPathComponent) }
        if let model = thread.activeModelName { parts.append(model) }
        return parts.joined(separator: " · ")
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
