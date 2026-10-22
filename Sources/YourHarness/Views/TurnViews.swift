import HarnessCore
import SwiftUI

/// "10:42", "Yesterday 10:42", "Oct 20, 10:42": when a message was sent, small under it.
struct MessageTime: View {
    let date: Date

    var body: some View {
        Text(Self.format(date))
            .font(.caption2.monospacedDigit())
            .foregroundStyle(.tertiary)
            .help(date.formatted(date: .complete, time: .standard))
    }

    static func format(_ date: Date, now: Date = .now) -> String {
        let calendar = Calendar.current
        let time = date.formatted(date: .omitted, time: .shortened)
        if calendar.isDate(date, inSameDayAs: now) { return time }
        if let yesterday = calendar.date(byAdding: .day, value: -1, to: now), calendar.isDate(date, inSameDayAs: yesterday) {
            return "Yesterday \(time)"
        }
        let sameYear = calendar.component(.year, from: date) == calendar.component(.year, from: now)
        let day = date.formatted(sameYear ? .dateTime.month(.abbreviated).day() : .dateTime.month(.abbreviated).day().year())
        return "\(day), \(time)"
    }
}

/// "45s", "2m 14s", "1h 3m".
enum TurnDuration {
    static func format(_ seconds: TimeInterval) -> String {
        let total = Int(seconds.rounded())
        if total < 60 { return "\(max(total, 1))s" }
        if total < 3600 { return "\(total / 60)m \(total % 60)s" }
        return "\(total / 3600)h \((total % 3600) / 60)m"
    }
}

/// A finished turn's work in one line, between your message and Claude's answer:
/// "Worked for 2m 14s · 9 tool calls · edited 3 files". Click to see it all.
struct WorkSummaryRow: View {
    let work: TurnWork
    let toggle: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: toggle) {
            HStack(spacing: 7) {
                Image(systemName: "chevron.right")
                    .font(.caption2.weight(.bold))
                    .rotationEffect(.degrees(work.isExpanded ? 90 : 0))
                Text(work.duration.map { "Worked for \(TurnDuration.format($0))" } ?? "Worked")
                    .fontWeight(.medium)
                if !details.isEmpty {
                    Text(details)
                        .foregroundStyle(.tertiary)
                }
                Rectangle()
                    .fill(.separator)
                    .frame(height: 1)
                    .frame(maxWidth: .infinity)
                    .padding(.leading, 4)
            }
            .font(.callout)
            .foregroundStyle(hovering ? .primary : .secondary)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(work.isExpanded ? "Hide Claude's work" : "Show what Claude did: thinking, tool calls and sub-agents")
    }

    /// Counted when the row shows, not for every turn of the thread.
    private var details: String {
        var tools = 0, agents = 0
        var files = Set<String>()
        for entry in work.entries where entry.kind == .tool {
            if entry.subagent != nil || SubagentReport.toolNames.contains(entry.toolName) {
                agents += 1
            } else {
                tools += 1
            }
            if Self.editTools.contains(entry.toolName), let path = Self.filePath(entry.toolInput) { files.insert(path) }
        }
        var parts: [String] = []
        if tools > 0 { parts.append(tools == 1 ? "1 tool call" : "\(tools) tool calls") }
        if !files.isEmpty { parts.append(files.count == 1 ? "edited 1 file" : "edited \(files.count) files") }
        if agents > 0 { parts.append(agents == 1 ? "1 agent" : "\(agents) agents") }
        return parts.isEmpty ? "" : "· " + parts.joined(separator: " · ")
    }

    private static let editTools: Set<String> = ["Edit", "MultiEdit", "Write", "NotebookEdit"]

    private static func filePath(_ inputJSON: String) -> String? {
        guard let data = inputJSON.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        return (object["file_path"] ?? object["notebook_path"]) as? String
    }
}

/// Up to your previous message, down to your next one (or the end). ⌘[ and ⌘] do the same.
struct MessageJumpControls: View {
    let thread: ThreadModel

    var body: some View {
        VStack(spacing: 0) {
            Button("Previous Message", systemImage: "chevron.up") { thread.jumpToMessage(.previous) }
                .help("Your previous message (⌘[)")
            Divider().frame(width: 18)
            Button("Next Message", systemImage: "chevron.down") { thread.jumpToMessage(.next) }
                .help("Your next message, or the end (⌘])")
        }
        .labelStyle(.iconOnly)
        .buttonStyle(.borderless)
        .font(.callout.weight(.semibold))
        .foregroundStyle(.secondary)
        .padding(.vertical, 6)
        .padding(.horizontal, 7)
        .glassEffect(.regular, in: .capsule)
    }
}
