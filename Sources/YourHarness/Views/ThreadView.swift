import HarnessCore
import SwiftUI

struct ThreadView: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    @State private var dropTargeted = false
    @State private var visibleHeight: CGFloat = 0
    /// Keep the newest output in view as the transcript grows: while history loads, while
    /// a reply streams. Your own scrolling turns it off; stopping at the bottom turns it
    /// back on.
    @State private var followsBottom = true
    @State private var isAtBottom = true
    private static let bottomID = "transcript-bottom"

    private var showsTypingIndicator: Bool {
        thread.phase == .running && thread.pendingPermissions.isEmpty && !(thread.entries.last?.isStreaming ?? false)
    }

    var body: some View {
        ScrollViewReader { proxy in
            transcript(proxy)
        }
        .environment(\.issueLinker, app.issueLinker(for: thread))
        .navigationTitle(thread.title)
        .navigationSubtitle(subtitle)
        .task { thread.loadTranscriptIfNeeded() }
    }

    private func transcript(_ proxy: ScrollViewProxy) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) { rows }
            // Keyed on count, so streaming text doesn't animate, only new rows.
            .animation(.smooth(duration: 0.32), value: thread.entries.count)
            .animation(.smooth(duration: 0.25), value: showsTypingIndicator)
            .frame(maxWidth: 820)
            .frame(maxWidth: .infinity)
            .padding(.horizontal, 28)
            .padding(.vertical, 24)
            .frame(minHeight: visibleHeight, alignment: .bottom)
            .overlay(alignment: .bottom) { Color.clear.frame(height: 1).id(Self.bottomID) }
        }
        .defaultScrollAnchor(.bottom, for: .initialOffset)
        .defaultScrollAnchor(.bottom, for: .alignment)
        // Pinned to the bottom as the transcript grows, but only while following: always on,
        // it pulled you back down whenever any row's height changed while you read further
        // up. (Scrolling to the end by hand as rows arrived could instead leave the lazy
        // stack drawing nothing when a very tall row came in.)
        .defaultScrollAnchor(followsBottom ? .bottom : nil, for: .sizeChanges)
        .onScrollGeometryChange(for: Bool.self) { geometry in
            // The reported content height counts the toolbar and composer insets; at the
            // very bottom, offset + container + top inset equals it (measured both ways).
            geometry.contentOffset.y + geometry.containerSize.height + geometry.contentInsets.top >= geometry.contentSize.height - 40
        } action: { _, atBottom in
            isAtBottom = atBottom
        }
        // Only scrolling decides, never a size change: while you scroll, nothing pulls you
        // back down; once you stop, the bottom is followed again only if that's where you are.
        .onScrollPhaseChange { old, phase in
            if phase != .idle {
                followsBottom = false
            } else if old != .idle {
                followsBottom = isAtBottom
            }
        }
        // Sending a message takes you to it, wherever you'd scrolled.
        .onChange(of: thread.entries.last?.id) {
            guard thread.entries.last?.kind == .user else { return }
            followsBottom = true
            proxy.scrollTo(Self.bottomID, anchor: .bottom)
        }
        .modifier(ScrollProbe())
        // At least as tall as what's visible between the toolbar and the composer, messages
        // at the bottom: a short thread would otherwise sit on a tall top inset, and the
        // toolbar's edge effect tints all of it. No taller, or an empty thread would already
        // count as scrolled away from the bottom when its history loads.
        .onGeometryChange(for: CGFloat.self) { proxy in
            proxy.size.height - proxy.safeAreaInsets.top - proxy.safeAreaInsets.bottom
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
        ForEach(thread.entries) { entry in
            EntryView(entry: entry)
                .modifier(ScrollProbe.Row(id: entry.id))
                .transition(.asymmetric(
                    insertion: .opacity.combined(with: .offset(y: 12)),
                    removal: .opacity
                ))
        }
        if showsTypingIndicator {
            TypingIndicator(text: thread.activity ?? "Working…")
                .transition(.opacity.combined(with: .scale(scale: 0.96, anchor: .leading)))
        }
    }

    private var subtitle: String {
        let project = app.project(id: thread.projectID)?.name ?? ""
        guard let model = thread.activeModelName else { return project }
        return "\(project) · \(model)"
    }
}

struct TypingIndicator: View {
    var text = "Working…"
    @State private var dimmed = false

    var body: some View {
        HStack(spacing: 8) {
            SpinningClaudeMark(size: 14)
            Text(text)
                .foregroundStyle(.secondary)
                .opacity(dimmed ? 0.45 : 1)
                .animation(.easeInOut(duration: 1.1).repeatForever(), value: dimmed)
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

    var body: some View {
        let presentation = ToolPresentation(name: request.toolName, inputJSON: request.inputJSON)
        HStack(alignment: .top, spacing: 12) {
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
                if let description = request.description, description != presentation.detail {
                    Text(description)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 12)
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
        .padding(14)
        .glassEffect(.regular.tint(.orange.opacity(0.12)), in: .rect(cornerRadius: Theme.floatingRadius))
    }
}
