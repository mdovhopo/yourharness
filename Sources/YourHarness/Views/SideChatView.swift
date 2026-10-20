import HarnessCore
import SwiftUI

/// The right sidebar for a thread: its side chat and its sub-agents.
struct ThreadInspector: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel

    var body: some View {
        VStack(spacing: 0) {
            Picker("Panel", selection: Bindable(app).inspectorPanel) {
                ForEach(AppModel.InspectorPanel.allCases) { panel in
                    Text(panel.label).tag(panel)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            Divider()
            switch app.inspectorPanel {
            case .sideChat: SideChatPanel(thread: thread)
            case .subagents: SubagentInspector(thread: thread)
            }
        }
    }
}

/// `/btw`: ask on the side. A copy of the thread's conversation answers; it can read files
/// but not change them, and nothing said here is added to the thread.
private struct SideChatPanel: View {
    let thread: ThreadModel

    var body: some View {
        if let chat = thread.sideChat {
            SideChatConversation(chat: chat, close: thread.closeSideChat)
                .id(chat.id)
        } else {
            VStack(spacing: 0) {
                ContentUnavailableView {
                    Label("Ask on the Side", systemImage: "bubble.left.and.text.bubble.right")
                } description: {
                    Text("A copy of this conversation answers here, so the thread isn't interrupted and nothing is added to it. Type **/btw** and a question in the composer, or ask below.")
                }
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
    let close: () -> Void

    private var showsTypingIndicator: Bool {
        chat.phase == .running && chat.pendingPermissions.isEmpty && !(chat.entries.last?.isStreaming ?? false)
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Label("Copy of this conversation · read-only", systemImage: "eye")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Spacer(minLength: 4)
                Button("End Side Chat", systemImage: "xmark", action: close)
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .foregroundStyle(.secondary)
                    .help("End the side chat. The next /btw starts a fresh copy of the thread.")
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 8)

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
