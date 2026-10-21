import HarnessCore
import SwiftUI

/// The right sidebar for a thread: its side chat and its sub-agents. The tabs sit in the
/// sidebar's part of the window toolbar, so the panel itself is all content.
struct ThreadInspector: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel?

    var body: some View {
        Group {
            if let thread {
                switch app.inspectorPanel {
                case .sideChat: SideChatPanel(thread: thread)
                case .subagents: SubagentInspector(thread: thread)
                }
            } else {
                ContentUnavailableView("No Thread Selected", systemImage: "sidebar.trailing")
            }
        }
        .environment(\.issueLinker, thread.map(app.issueLinker(for:)) ?? IssueLinker(repos: [], openPullRequests: []))
        .toolbar {
            if thread != nil {
                ToolbarItem {
                    Picker("Panel", selection: Bindable(app).inspectorPanel) {
                        ForEach(AppModel.InspectorPanel.allCases) { panel in
                            Text(panel.label).tag(panel)
                        }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                }
            }
            ToolbarSpacer(.flexible)
            if app.inspectorPanel == .sideChat, let thread, thread.sideChat != nil {
                ToolbarItem {
                    Button("New Side Chat", systemImage: "square.and.pencil", action: thread.closeSideChat)
                        .help("End this side chat. The next question starts from a fresh copy of the thread.")
                }
            }
            ToolbarItem {
                Button("Sidebar", systemImage: "sidebar.trailing") { app.setShowsInspector(!app.showsInspector) }
                    .help(app.showsInspector ? "Hide the side chat and sub-agents (⌥⌘0)" : "Show the side chat and sub-agents (⌥⌘0)")
            }
        }
    }
}

/// `/btw`: ask on the side. A copy of the thread's conversation answers; it reads files,
/// asks before running anything, can't edit, and nothing said here is added to the thread.
private struct SideChatPanel: View {
    let thread: ThreadModel

    var body: some View {
        if let chat = thread.sideChat {
            SideChatConversation(chat: chat)
                .id(chat.id)
        } else {
            VStack(spacing: 0) {
                ContentUnavailableView {
                    Label("Ask on the Side", systemImage: "bubble.left.and.text.bubble.right")
                } description: {
                    Text("A copy of this conversation answers here, so the thread isn't interrupted and nothing is added to it. It can't edit files, and asks before running commands. Type **/btw** and a question in the composer, or ask below.")
                }
                .frame(maxHeight: .infinity)
                SideChatComposer(placeholder: "Ask on the side…", isRunning: false, stop: {}) { question in
                    thread.askAside(question)
                }
                .padding(12)
            }
        }
    }
}

private struct SideChatConversation: View {
    let chat: ThreadModel

    private var showsTypingIndicator: Bool {
        chat.phase == .running && chat.pendingPermissions.isEmpty && !(chat.entries.last?.isStreaming ?? false)
    }

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    ForEach(chat.entries) { entry in
                        EntryView(entry: entry)
                    }
                    if showsTypingIndicator {
                        TypingIndicator(text: chat.activity ?? "Thinking…")
                    }
                }
                .padding(14)
            }
            .defaultScrollAnchor(.bottom)
            .defaultScrollAnchor(.bottom, for: .sizeChanges)

            VStack(spacing: 8) {
                ForEach(chat.pendingPermissions) { request in
                    RequestCard(request: request, thread: chat)
                }
                SideChatComposer(placeholder: "Ask a follow-up…", isRunning: chat.phase == .running, stop: chat.stop) { question in
                    chat.send(question)
                }
            }
            .padding(12)
        }
    }
}

private struct SideChatComposer: View {
    let placeholder: String
    let isRunning: Bool
    let stop: () -> Void
    let send: (String) -> Void
    @State private var text = ""

    private var canSend: Bool { !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    var body: some View {
        HStack(alignment: .bottom, spacing: 6) {
            TextField(placeholder, text: $text, axis: .vertical)
                .textFieldStyle(.plain)
                .lineLimit(1...6)
                .onSubmit(submit)
                .padding(.vertical, 4)
            if isRunning {
                Button("Stop", systemImage: "stop.fill", action: stop)
                    .labelStyle(.iconOnly)
                    .buttonStyle(.glass)
                    .buttonBorderShape(.circle)
                    .controlSize(.small)
            }
            Button("Send", systemImage: "arrow.up", action: submit)
                .labelStyle(.iconOnly)
                .fontWeight(.semibold)
                .buttonStyle(.glassProminent)
                .buttonBorderShape(.circle)
                .tint(.claude)
                .controlSize(.small)
                .disabled(!canSend)
        }
        .padding(.leading, 12)
        .padding(.trailing, 6)
        .padding(.vertical, 6)
        .glassEffect(.regular, in: .rect(cornerRadius: Theme.floatingRadius))
        // No focus grab: /btw comes from the thread's composer, and you carry on typing there.
    }

    private func submit() {
        guard canSend else { return }
        send(text)
        text = ""
    }
}
