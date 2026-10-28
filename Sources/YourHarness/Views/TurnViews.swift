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

/// A finished turn's work, folded into one row between your message and Claude's answer:
/// "Worked for 2m 14s · 9 tool calls · edited 3 files". Opened, it shows every step as one
/// group: thinking, tool calls, sub-agents, and what Claude said along the way.
struct WorkSummaryRow: View {
    let work: TurnWork
    let toggle: () -> Void
    @State private var hovering = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button(action: toggle) {
                HStack(spacing: 8) {
                    Image(systemName: "hammer")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Text(work.duration.map { "Worked for \(TurnDuration.format($0))" } ?? "Worked")
                        .fontWeight(.medium)
                    if !details.isEmpty {
                        Text(details)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 8)
                    Text(work.isExpanded ? "Hide work" : "Show work")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .opacity(hovering || work.isExpanded ? 1 : 0)
                    Image(systemName: "chevron.right")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .rotationEffect(.degrees(work.isExpanded ? 90 : 0))
                }
                .font(.callout)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .onHover { hovering = $0 }
            .help(work.isExpanded ? "Hide Claude's work" : "Show what Claude did: thinking, tool calls and sub-agents")

            if work.isExpanded {
                Divider()
                VStack(alignment: .leading, spacing: 14) {
                    ForEach(work.entries) { entry in
                        EntryView(entry: entry)
                    }
                }
                .padding(.leading, 12)
                .overlay(alignment: .leading) {
                    Rectangle().fill(.separator).frame(width: 2)
                }
                .padding(12)
                .transition(.opacity)
            }
        }
        .background(hovering && !work.isExpanded ? AnyShapeStyle(.primary.opacity(0.06)) : AnyShapeStyle(.primary.opacity(0.035)),
                    in: .rect(cornerRadius: Theme.innerRadius))
        .overlay(RoundedRectangle(cornerRadius: Theme.innerRadius).strokeBorder(.separator.opacity(0.6)))
        .animation(.easeOut(duration: 0.12), value: hovering)
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
            // As git names it, symlinks resolved (`/tmp` is `/private/tmp`), so a file isn't counted twice.
            if Self.editTools.contains(entry.toolName), let path = Self.filePath(entry.toolInput) {
                let folder = (path as NSString).deletingLastPathComponent
                files.insert((WorkspaceModel.realPath(folder) as NSString).appendingPathComponent((path as NSString).lastPathComponent))
            }
        }
        // What git saw change, however it was changed: shell commands and scripts too.
        files.formUnion(work.changedFiles ?? [])
        var parts: [String] = []
        if tools > 0 { parts.append(tools == 1 ? "1 tool call" : "\(tools) tool calls") }
        if !files.isEmpty { parts.append(files.count == 1 ? "edited 1 file" : "edited \(files.count) files") }
        if agents > 0 { parts.append(agents == 1 ? "1 agent" : "\(agents) agents") }
        return parts.joined(separator: " · ")
    }

    private static let editTools: Set<String> = ["Edit", "MultiEdit", "Write", "NotebookEdit"]

    private static func filePath(_ inputJSON: String) -> String? {
        guard let data = inputJSON.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        return (object["file_path"] ?? object["notebook_path"]) as? String
    }
}

/// To the top of the thread, up to your previous message, down to your next one (or the
/// end), and to the end. ⇧⌘[, ⌘[, ⌘] and ⇧⌘] do the same.
struct MessageJumpControls: View {
    let thread: ThreadModel
    /// Anything above the screen; a message of yours above it; past the bottom of the
    /// conversation. Each arrow shows only when it has somewhere to go.
    let canGoTop: Bool
    let canGoPrevious: Bool
    let canGoNext: Bool

    var body: some View {
        VStack(spacing: 10) {
            if canGoTop {
                Button("Top of Thread", systemImage: "chevron.up.2") { thread.jumpToMessage(.top) }
                    .help("The top of the thread (⇧⌘[)")
                    .transition(.scale(scale: 0.6).combined(with: .opacity))
            }
            if canGoPrevious {
                Button("Previous Message", systemImage: "chevron.up") { thread.jumpToMessage(.previous) }
                    .help("Your previous message (⌘[)")
                    .transition(.scale(scale: 0.6).combined(with: .opacity))
            }
            if canGoNext {
                Button("Next Message", systemImage: "chevron.down") { thread.jumpToMessage(.next) }
                    .help("Your next message, or the end (⌘])")
                    .transition(.scale(scale: 0.6).combined(with: .opacity))
                Button("End of Thread", systemImage: "chevron.down.2") { thread.jumpToMessage(.bottom) }
                    .help("The end of the thread (⇧⌘])")
                    .transition(.scale(scale: 0.6).combined(with: .opacity))
            }
        }
        .labelStyle(.iconOnly)
        .buttonStyle(JumpButtonStyle())
        .animation(.snappy(duration: 0.2), value: canGoTop)
        .animation(.snappy(duration: 0.2), value: canGoPrevious)
        .animation(.snappy(duration: 0.2), value: canGoNext)
    }
}

/// A small round button floating over the transcript: solid, a hairline edge, a soft shadow.
private struct JumpButtonStyle: ButtonStyle {
    @State private var hovering = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 15, weight: .medium))
            .foregroundStyle(.secondary)
            .frame(width: 34, height: 34)
            .background {
                Circle()
                    .fill(Color(nsColor: .controlBackgroundColor))
                    .overlay(Circle().fill(.fill.opacity(configuration.isPressed ? 0.8 : hovering ? 0.45 : 0)))
                    .overlay(Circle().strokeBorder(.separator, lineWidth: 0.5))
                    .shadow(color: .black.opacity(0.12), radius: 3, y: 1)
            }
            .scaleEffect(configuration.isPressed ? 0.94 : 1)
            .contentShape(.circle)
            .onHover { hovering = $0 }
            .animation(.easeOut(duration: 0.15), value: hovering)
            .animation(.spring(response: 0.25, dampingFraction: 0.7), value: configuration.isPressed)
    }
}

/// Copies a message's text, and says so for a moment.
struct CopyMessageButton: View {
    let text: String
    @State private var copied = false

    var body: some View {
        Button(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc") {
            Pasteboard.copy(text)
            copied = true
            Task {
                try? await Task.sleep(for: .seconds(1.5))
                copied = false
            }
        }
        .labelStyle(.iconOnly)
        .buttonStyle(.borderless)
        .foregroundStyle(.secondary)
        .contentTransition(.symbolEffect(.replace))
        .help("Copy message")
    }
}
