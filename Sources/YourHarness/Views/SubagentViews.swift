import HarnessCore
import SwiftUI

/// An Agent tool call in the transcript: what the sub-agent is doing while it works, and
/// its report once it's done. The right sidebar follows it step by step.
struct SubagentCallView: View {
    @Environment(AppModel.self) private var app
    let entry: TranscriptEntry
    @State private var expanded = false

    /// Live, or from history. Older transcripts only kept the tool call.
    private var run: SubagentRun? { entry.subagent }

    private var title: String {
        if let run, !run.description.isEmpty { return run.description }
        return ToolPresentation(name: entry.toolName, inputJSON: entry.toolInput).detail
    }

    private var report: String? {
        run?.report ?? entry.toolResult.flatMap(SubagentReport.text(fromToolResult:))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.snappy) { expanded.toggle() }
            } label: {
                header
            }
            .buttonStyle(.plain)

            if let run, run.isRunning, !run.entries.isEmpty {
                Divider().padding(.horizontal, 12)
                RecentSteps(run: run)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .transition(.opacity)
            }

            if expanded, let report, !report.isEmpty {
                Divider()
                MarkdownView(text: report)
                    .textSelection(.enabled)
                    .padding(12)
            }
        }
        .background(.quaternary.opacity(0.35), in: .rect(cornerRadius: 12))
        .overlay {
            RoundedRectangle(cornerRadius: 12)
                .strokeBorder(borderStyle, lineWidth: run?.isRunning == true ? 1.5 : 1)
        }
        .animation(.snappy, value: run?.isRunning)
    }

    private var borderStyle: AnyShapeStyle {
        switch run?.status {
        case .running: AnyShapeStyle(.tint.opacity(0.45))
        case .failed: AnyShapeStyle(.red.opacity(0.5))
        default: entry.toolIsError ? AnyShapeStyle(.red.opacity(0.5)) : AnyShapeStyle(.separator.opacity(0.6))
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            SubagentStatusIcon(status: run?.status ?? (entry.isStreaming ? .running : (entry.toolIsError ? .failed : .completed)))
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(title)
                        .fontWeight(.medium)
                        .lineLimit(1)
                    if let type = run?.agentType {
                        AgentTypeChip(type: type)
                    }
                }
                if let run {
                    SubagentStats(run: run)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 8)
            if let run {
                Button("Follow in Sidebar", systemImage: "sidebar.trailing") { app.inspect(run) }
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .foregroundStyle(.secondary)
                    .help("Follow this sub-agent in the sidebar (⌥⌘0)")
            }
            if report?.isEmpty == false {
                Image(systemName: "chevron.right")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.tertiary)
                    .rotationEffect(.degrees(expanded ? 90 : 0))
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .contentShape(.rect)
    }
}

/// "Reading a.txt · 2 tools · 0:07" while it works; "2 tools · 16.8k tokens · 7s" after.
struct SubagentStats: View {
    let run: SubagentRun

    var body: some View {
        HStack(spacing: 0) {
            Text(leading)
            if run.isRunning {
                Text(" · ")
                Text(run.startedAt, style: .timer)
                    .monospacedDigit()
            }
        }
        .lineLimit(1)
    }

    private var leading: String {
        var parts: [String] = []
        switch run.status {
        case .running: parts.append(run.activity ?? (run.isBackground ? "Working in the background" : "Working"))
        case .completed: break
        case .failed: parts.append("Failed")
        case .stopped: parts.append("Stopped")
        }
        if let tools = run.toolUses, tools > 0 { parts.append(tools == 1 ? "1 tool" : "\(tools) tools") }
        if !run.isRunning {
            if let tokens = run.tokens { parts.append(Self.tokens(tokens)) }
            if let ms = run.durationMs { parts.append(Self.duration(ms)) }
        }
        return parts.isEmpty ? "Done" : parts.joined(separator: " · ")
    }

    static func tokens(_ count: Int) -> String {
        count < 1000 ? "\(count) tokens" : "\((Double(count) / 1000).formatted(.number.precision(.fractionLength(count < 10_000 ? 1 : 0))))k tokens"
    }

    static func duration(_ ms: Int) -> String {
        let seconds = ms / 1000
        return seconds < 60 ? "\(max(seconds, 1))s" : "\(seconds / 60)m \(seconds % 60)s"
    }
}

struct SubagentStatusIcon: View {
    let status: SubagentRun.Status

    var body: some View {
        switch status {
        case .running:
            ProgressView().controlSize(.small)
        case .completed:
            Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
        case .failed:
            Image(systemName: "xmark.octagon.fill").foregroundStyle(.red)
        case .stopped:
            Image(systemName: "stop.circle").foregroundStyle(.secondary)
        }
    }
}

private struct AgentTypeChip: View {
    let type: String

    var body: some View {
        Text(type)
            .font(.caption.weight(.medium))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 6)
            .padding(.vertical, 1)
            .background(.quaternary, in: .capsule)
            .lineLimit(1)
            .fixedSize()
    }
}

/// The last few tool calls, one line each, so a working sub-agent visibly moves.
private struct RecentSteps: View {
    let run: SubagentRun

    var body: some View {
        let steps = run.entries.filter { $0.kind == .tool }.suffix(3)
        VStack(alignment: .leading, spacing: 5) {
            ForEach(steps) { step in
                StepRow(entry: step)
            }
        }
    }
}

private struct StepRow: View {
    let entry: TranscriptEntry

    var body: some View {
        let presentation = ToolPresentation(name: entry.toolName, inputJSON: entry.toolInput)
        HStack(spacing: 8) {
            Image(systemName: presentation.symbol)
                .foregroundStyle(.secondary)
                .frame(width: 16)
            Text(presentation.verb)
            Text(presentation.detail)
                .font(.callout.monospaced())
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 4)
            if entry.isStreaming {
                ProgressView().controlSize(.mini)
            } else if entry.toolIsError {
                Image(systemName: "xmark.circle.fill").foregroundStyle(.red)
            }
        }
        .font(.callout)
    }
}

// MARK: - Right sidebar

/// The thread's sub-agents: pick one, follow what it does, stop it.
struct SubagentInspector: View {
    let thread: ThreadModel

    var body: some View {
        let runs = thread.subagentRuns
        Group {
            if runs.isEmpty {
                ContentUnavailableView {
                    Label("No Sub-agents", systemImage: "person.2")
                } description: {
                    Text("When Claude hands work to a sub-agent, you can follow it here step by step.")
                }
            } else {
                let selected = selection(in: runs)
                VStack(spacing: 0) {
                    if runs.count > 1 {
                        RunList(runs: runs, selected: selected.id) { thread.inspectedSubagentID = $0.id }
                        Divider()
                    }
                    SubagentDetail(run: selected, stop: { thread.stopSubagent(selected) })
                        .id(selected.id)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// Your pick, else the newest one still working, else the newest.
    private func selection(in runs: [SubagentRun]) -> SubagentRun {
        if let id = thread.inspectedSubagentID, let run = runs.first(where: { $0.id == id }) { return run }
        return runs.last(where: \.isRunning) ?? runs[runs.count - 1]
    }
}

private struct RunList: View {
    let runs: [SubagentRun]
    let selected: String
    let select: (SubagentRun) -> Void

    var body: some View {
        ScrollView {
            VStack(spacing: 2) {
                ForEach(runs.reversed()) { run in
                    Button { select(run) } label: {
                        HStack(spacing: 8) {
                            SubagentStatusIcon(status: run.status)
                                .frame(width: 16)
                            Text(run.description.isEmpty ? "Sub-agent" : run.description)
                                .lineLimit(1)
                            Spacer(minLength: 4)
                            if let type = run.agentType {
                                Text(type)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                            }
                        }
                        .padding(.horizontal, 8)
                        .padding(.vertical, 5)
                        .background(run.id == selected ? AnyShapeStyle(.tint.opacity(0.15)) : AnyShapeStyle(.clear),
                                    in: .rect(cornerRadius: 7))
                        .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(8)
        }
        .scrollBounceBehavior(.basedOnSize)
        .frame(maxHeight: 168)
        .fixedSize(horizontal: false, vertical: true)
    }
}

private struct SubagentDetail: View {
    let run: SubagentRun
    let stop: () -> Void

    /// Its final message is the report, shown once, below.
    private var steps: [TranscriptEntry] {
        guard let report = run.report, let last = run.entries.last, last.kind == .assistant,
              last.text.trimmingCharacters(in: .whitespacesAndNewlines) == report.trimmingCharacters(in: .whitespacesAndNewlines)
        else { return run.entries }
        return Array(run.entries.dropLast())
    }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 14) {
                header
                if !run.prompt.isEmpty {
                    PromptBox(prompt: run.prompt)
                }
                ForEach(steps) { entry in
                    EntryView(entry: entry)
                }
                if run.isRunning {
                    HStack(spacing: 8) {
                        SpinningClaudeMark(size: 13)
                        Text(run.activity ?? "Working…")
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    .font(.callout)
                } else if let report = run.report, !report.isEmpty {
                    VStack(alignment: .leading, spacing: 8) {
                        Label("Report", systemImage: "text.document")
                            .font(.headline)
                        MarkdownView(text: report)
                            .textSelection(.enabled)
                    }
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.tint.opacity(0.06), in: .rect(cornerRadius: 12))
                }
            }
            .padding(16)
            .animation(.smooth(duration: 0.25), value: run.entries.count)
        }
        .defaultScrollAnchor(.bottom, for: .sizeChanges)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(run.description.isEmpty ? "Sub-agent" : run.description)
                    .font(.title3.weight(.semibold))
                    .lineLimit(3)
                Spacer(minLength: 8)
                if run.isRunning, run.taskID != nil {
                    Button("Stop", systemImage: "stop.fill", action: stop)
                        .buttonStyle(.glass)
                        .controlSize(.small)
                        .help("Stop this sub-agent. The thread carries on.")
                }
            }
            HStack(spacing: 6) {
                SubagentStatusIcon(status: run.status)
                    .controlSize(.mini)
                SubagentStats(run: run)
            }
            .font(.callout)
            .foregroundStyle(.secondary)
            let kind = [run.agentType, run.isBackground ? "in the background" : nil].compactMap(\.self)
            if !kind.isEmpty {
                Text(kind.joined(separator: " · "))
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        }
    }
}

/// The instructions the main agent wrote for it; long, so folded by default.
private struct PromptBox: View {
    let prompt: String
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                withAnimation(.snappy) { expanded.toggle() }
            } label: {
                HStack(spacing: 6) {
                    Text("Task")
                        .font(.subheadline.weight(.semibold))
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                    Spacer()
                }
                .foregroundStyle(.secondary)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            Text(prompt)
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(expanded ? nil : 3)
                .textSelection(.enabled)
        }
        .padding(10)
        .background(.quaternary.opacity(0.35), in: .rect(cornerRadius: 10))
    }
}
