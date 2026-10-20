import HarnessCore
import SwiftUI

struct ThreadView: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    @State private var dropTargeted = false
    @State private var visibleHeight: CGFloat = 0

    private var showsTypingIndicator: Bool {
        thread.phase == .running && thread.pendingPermissions.isEmpty && !(thread.entries.last?.isStreaming ?? false)
    }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) {
                ForEach(thread.entries) { entry in
                    EntryView(entry: entry)
                        .transition(.asymmetric(
                            insertion: .opacity.combined(with: .offset(y: 12)),
                            removal: .opacity
                        ))
                }
                if showsTypingIndicator {
                    TypingIndicator()
                        .transition(.opacity.combined(with: .scale(scale: 0.96, anchor: .leading)))
                }
            }
            // Keyed on count, so streaming text doesn't animate, only new rows.
            .animation(.smooth(duration: 0.32), value: thread.entries.count)
            .animation(.smooth(duration: 0.25), value: showsTypingIndicator)
            .frame(maxWidth: 820)
            .frame(maxWidth: .infinity)
            .padding(.horizontal, 28)
            .padding(.vertical, 24)
            .frame(minHeight: visibleHeight, alignment: .bottom)
        }
        .defaultScrollAnchor(.bottom)
        .defaultScrollAnchor(.bottom, for: .sizeChanges)
        // At least as tall as what's visible, messages at the bottom: a short thread would
        // otherwise sit on a tall top inset, and the toolbar's edge effect tints all of it.
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
        .environment(\.issueLinker, app.issueLinker(for: thread))
        .navigationTitle(thread.title)
        .navigationSubtitle(subtitle)
        .task { thread.loadTranscriptIfNeeded() }
    }

    private var subtitle: String {
        let project = app.project(id: thread.projectID)?.name ?? ""
        guard let model = thread.activeModelName else { return project }
        return "\(project) · \(model)"
    }
}

private struct TypingIndicator: View {
    @State private var dimmed = false

    var body: some View {
        HStack(spacing: 8) {
            SpinningClaudeMark(size: 14)
            Text("Working…")
                .foregroundStyle(.secondary)
                .opacity(dimmed ? 0.45 : 1)
                .animation(.easeInOut(duration: 1.1).repeatForever(), value: dimmed)
        }
        .font(.callout)
        .onAppear { dimmed = true }
    }
}

/// A question, a plan or a plain permission prompt, depending on the tool asking.
private struct RequestCard: View {
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
