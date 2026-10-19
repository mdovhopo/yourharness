import SwiftUI

struct EntryView: View {
    let entry: TranscriptEntry

    var body: some View {
        switch entry.kind {
        case .user:
            UserMessageView(text: entry.text)
        case .assistant:
            StreamingMarkdown(entry: entry)
        case .thinking:
            ThinkingView(entry: entry)
        case .tool:
            ToolCallView(entry: entry)
        case .notice:
            NoticeView(text: entry.text, style: entry.noticeStyle)
        }
    }
}

private struct UserMessageView: View {
    let text: String

    var body: some View {
        HStack {
            Spacer(minLength: 80)
            Text(text)
                .textSelection(.enabled)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .background(.tint.opacity(0.14), in: .rect(cornerRadius: 16))
        }
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
