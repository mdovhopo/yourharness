import AppKit
import HarnessCore
import SwiftUI

struct EntryView: View {
    let entry: TranscriptEntry
    /// For plan rows: the thread they're part of.
    @Environment(ThreadModel.self) private var thread: ThreadModel?

    var body: some View {
        switch entry.kind {
        case .user:
            UserMessageView(text: entry.text, attachments: entry.attachments, delivery: entry.delivery)
        case .assistant:
            StreamingMarkdown(entry: entry)
        case .thinking:
            ThinkingView(entry: entry)
        case .tool where thread?.planSteps[entry.id] != nil:
            PlanStepView(plan: thread?.planSteps[entry.id] ?? TaskPlan(), isLatest: thread?.latestPlanStep == entry.id)
        case .tool where SubagentReport.toolNames.contains(entry.toolName):
            SubagentCallView(entry: entry)
        case .tool:
            ToolCallView(entry: entry)
        case .notice:
            NoticeView(text: entry.text, style: entry.noticeStyle)
        case .commandOutput:
            CommandOutputView(text: entry.text)
        }
    }
}

struct UserMessageView: View {
    @Environment(AppModel.self) private var app
    let text: String
    let attachments: [String]
    var delivery = TranscriptEntry.Delivery.normal

    var body: some View {
        HStack {
            Spacer(minLength: 80)
            VStack(alignment: .trailing, spacing: 6) {
                if !attachments.isEmpty {
                    HStack(spacing: 6) {
                        ForEach(attachments, id: \.self) { name in
                            let url = app.store.attachmentURL(name)
                            Button {
                                NSWorkspace.shared.open(url)
                            } label: {
                                AttachmentThumbnail(content: .file(url), maxSize: CGSize(width: 220, height: 160))
                            }
                            .buttonStyle(.plain)
                            .help("Open in Preview")
                        }
                    }
                }
                if !text.isEmpty {
                    message
                        .textSelection(.enabled)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 10)
                        .background(.tint.opacity(0.14), in: .rect(cornerRadius: 16))
                }
                switch delivery {
                case .steered:
                    Label("Steered · Claude read this after its step", systemImage: "arrow.turn.down.right")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                case .interrupted:
                    Label("Interrupted Claude to send this", systemImage: "bolt.fill")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                case .normal:
                    EmptyView()
                }
            }
        }
    }
}

extension UserMessageView {
    /// A command reads as one: its name in monospace, then what you passed it.
    var message: Text {
        guard let command = SlashCommand.parse(text) else { return Text(text) }
        let name = Text("/\(command.name)").font(.body.monospaced().weight(.semibold))
        return command.arguments.isEmpty ? name : Text("\(name) \(command.arguments)")
    }
}

/// What a built-in command printed, like `/context`'s usage table.
private struct CommandOutputView: View {
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Command output", systemImage: "terminal")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            MarkdownView(text: text)
                .textSelection(.enabled)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }
}

private struct ThinkingView: View {
    let entry: TranscriptEntry
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                withAnimation(.snappy) { expanded.toggle() }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "brain")
                    Text(entry.isStreaming ? "Thinking…" : "Thought")
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                }
                .foregroundStyle(.secondary)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)

            if expanded {
                Text(entry.text)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
                    .padding(.leading, 22)
            }
        }
    }
}

private struct NoticeView: View {
    let text: String
    let style: TranscriptEntry.NoticeStyle

    var body: some View {
        HStack {
            Spacer()
            Label(text, systemImage: style == .error ? "exclamationmark.triangle.fill" : "info.circle")
                .font(.callout)
                .foregroundStyle(style == .error ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                .textSelection(.enabled)
            Spacer()
        }
    }
}
