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
            UserMessageView(text: entry.text, attachments: entry.attachments, delivery: entry.delivery, date: entry.date)
        case .assistant:
            AssistantMessageView(entry: entry)
        case .thinking:
            ThinkingView(entry: entry)
        case .tool where thread?.planSteps[entry.id] != nil:
            PlanStepView(plan: thread?.planSteps[entry.id] ?? TaskPlan(), isLatest: thread?.latestPlanStep == entry.id)
        case .tool where entry.toolName == MediaTool.toolName:
            SharedMediaView(entry: entry)
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

/// Claude's reply, with Copy under it while the pointer is over it.
struct AssistantMessageView: View {
    let entry: TranscriptEntry
    @State private var hovering = false

    var body: some View {
        StreamingMarkdown(entry: entry)
            // In the gap below, so showing it doesn't move anything.
            .overlay(alignment: .bottomLeading) {
                if hovering, !entry.isStreaming {
                    CopyMessageButton(text: entry.text)
                        .offset(y: 20)
                        .transition(.opacity)
                }
            }
            .onHover { hovering = $0 }
            .animation(.easeOut(duration: 0.12), value: hovering)
    }
}

struct UserMessageView: View {
    @Environment(AppModel.self) private var app
    @Environment(ThreadModel.self) private var thread: ThreadModel?
    @Environment(\.findHighlight) private var findHighlight
    let text: String
    let attachments: [String]
    var delivery = TranscriptEntry.Delivery.normal
    var date: Date?
    @State private var hovering = false

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
                    // Tinted and outlined, so your messages stand out when scrolling back.
                    message
                        .textSelection(.enabled)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 10)
                        .background(.tint.opacity(0.17), in: .rect(cornerRadius: 16))
                        .overlay(RoundedRectangle(cornerRadius: 16).strokeBorder(.tint.opacity(0.35), lineWidth: 1))
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
                HStack(spacing: 6) {
                    if hovering, !text.isEmpty {
                        CopyMessageButton(text: text)
                            .font(.caption)
                            .transition(.opacity)
                    }
                    if let date {
                        MessageTime(date: date)
                    }
                }
            }
        }
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
    }
}

extension UserMessageView {
    /// A command reads as one: its name in monospace, then what you passed it.
    var message: Text {
        guard let command = SlashCommand.parse(text) else { return Text(chained(AttributedString(text).highlighted(findHighlight), in: text)) }
        let name = Text("/\(command.name)").font(.body.monospaced().weight(.semibold))
        return command.arguments.isEmpty ? name : Text("\(name) \(chained(AttributedString(command.arguments), in: command.arguments))")
    }

    /// Skills chained later in the message (`then /review`) read as commands too.
    private func chained(_ attributed: AttributedString, in text: String) -> AttributedString {
        guard let thread, text.contains("/") else { return attributed }
        let known = Set(app.chainableCommands(for: thread.projectID).flatMap { [$0.name] + $0.aliases })
        var result = attributed
        for name in SlashCommand.chainedCommands(in: " " + text, known: known) {
            var search = result.startIndex
            while let range = result[search...].range(of: "/" + name) {
                let before = range.lowerBound == result.startIndex ? " " : result.characters[result.characters.index(before: range.lowerBound)]
                let after = range.upperBound == result.endIndex ? " " : result.characters[range.upperBound]
                if before.isWhitespace, !(after.isLetter || after.isNumber || after == "-" || after == ":" || after == "/") {
                    result[range].font = .body.monospaced().weight(.semibold)
                }
                search = range.upperBound
            }
        }
        return result
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
    @Environment(\.findHighlight) private var findHighlight

    var body: some View {
        HStack {
            Spacer()
            Label { Text(AttributedString(text).highlighted(findHighlight)) } icon: {
                Image(systemName: style == .error ? "exclamationmark.triangle.fill" : "info.circle")
            }
                .font(.callout)
                .foregroundStyle(style == .error ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                .textSelection(.enabled)
            Spacer()
        }
    }
}
