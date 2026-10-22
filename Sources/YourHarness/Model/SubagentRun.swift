import Foundation
import HarnessCore

/// A sub-agent started by an Agent tool call: what it was asked, what it's doing, its own
/// transcript, and its report. Hangs off that tool call's transcript entry.
@MainActor @Observable
final class SubagentRun: Identifiable {
    enum Status: String, Codable {
        case running, completed, failed, stopped
    }

    /// The Agent tool call's ID.
    let id: String
    var taskID: String?
    var description: String
    var agentType: String?
    var prompt: String
    var isBackground = false
    var status = Status.running
    /// What it's doing right now, e.g. "Reading a.txt".
    var activity: String?
    var toolUses: Int?
    var tokens: Int?
    var durationMs: Int?
    var report: String?
    let startedAt: Date
    private(set) var entries: [TranscriptEntry] = []

    @ObservationIgnored private var tools: [String: TranscriptEntry] = [:]

    var isRunning: Bool { status == .running }

    init(id: String, description: String, agentType: String? = nil, prompt: String = "", startedAt: Date = .now) {
        self.id = id
        self.description = description
        self.agentType = agentType
        self.prompt = prompt
        self.startedAt = startedAt
    }

    /// Fills in from the Agent tool's input when no start event said otherwise.
    convenience init(toolCall entry: TranscriptEntry) {
        let input = (try? JSONSerialization.jsonObject(with: Data(entry.toolInput.utf8))) as? [String: Any] ?? [:]
        self.init(id: entry.id, description: input["description"] as? String ?? "",
                  agentType: input["subagent_type"] as? String, prompt: input["prompt"] as? String ?? "")
        isBackground = input["run_in_background"] as? Bool ?? false
    }

    func start(_ start: SubagentStart) {
        taskID = start.taskID
        if !start.description.isEmpty { description = start.description }
        agentType = start.agentType ?? agentType
        if let prompt = start.prompt, !prompt.isEmpty { self.prompt = prompt }
        isBackground = start.isBackground
    }

    func update(activity: String?, usage: SubagentUsage) {
        if let activity, !activity.isEmpty { self.activity = activity }
        apply(usage)
    }

    func finish(_ end: SubagentEnd) {
        status = switch end.status {
        case "completed": .completed
        case "failed", "error": .failed
        default: .stopped
        }
        if let summary = end.summary, !summary.isEmpty { report = summary }
        apply(end.usage)
        activity = nil
        settleTools()
    }

    /// The process went away, or the turn it belonged to was interrupted.
    func abandon() {
        guard isRunning else { return }
        status = .stopped
        activity = nil
        settleTools()
    }

    private func apply(_ usage: SubagentUsage) {
        toolUses = usage.toolUses ?? toolUses
        tokens = usage.tokens ?? tokens
        durationMs = usage.durationMs ?? durationMs
    }

    private func settleTools() {
        for entry in tools.values where entry.isStreaming { entry.isStreaming = false }
        tools.removeAll()
    }

    // MARK: - Its transcript

    /// Sub-agent messages arrive whole, never as deltas.
    func append(_ blocks: [ContentBlock]) {
        for block in blocks {
            switch block {
            case .text(let text) where !text.isEmpty:
                entries.append(TranscriptEntry(kind: .assistant, text: text))
            case .thinking(let text) where !text.isEmpty:
                entries.append(TranscriptEntry(kind: .thinking, text: text))
            case .toolUse(let id, let name, let input):
                let entry = TranscriptEntry(id: id, kind: .tool, isStreaming: true)
                entry.toolName = name
                entry.toolInput = input
                tools[id] = entry
                entries.append(entry)
            case .text, .thinking:
                break
            }
        }
    }

    /// The calls these results finished, for the thread to see what they did.
    @discardableResult
    func apply(_ results: [ToolResult]) -> [TranscriptEntry] {
        var finished: [TranscriptEntry] = []
        for result in results {
            guard let entry = tools.removeValue(forKey: result.toolUseID) else { continue }
            entry.toolResult = result.content
            entry.toolIsError = result.isError
            entry.isStreaming = false
            finished.append(entry)
        }
        return finished
    }
}

// MARK: - Persistence

struct SubagentSnapshot: Codable, Sendable {
    var description: String
    var agentType: String?
    var prompt: String
    var isBackground: Bool?
    var status: SubagentRun.Status
    var toolUses: Int?
    var tokens: Int?
    var durationMs: Int?
    var report: String?
    var startedAt: Date
    var entries: [EntrySnapshot]
}

extension SubagentRun {
    var snapshot: SubagentSnapshot {
        SubagentSnapshot(description: description, agentType: agentType, prompt: prompt,
                         isBackground: isBackground ? true : nil, status: status, toolUses: toolUses,
                         tokens: tokens, durationMs: durationMs, report: report, startedAt: startedAt,
                         entries: entries.map(\.snapshot))
    }

    convenience init(id: String, snapshot: SubagentSnapshot) {
        self.init(id: id, description: snapshot.description, agentType: snapshot.agentType,
                  prompt: snapshot.prompt, startedAt: snapshot.startedAt)
        isBackground = snapshot.isBackground ?? false
        // Whatever was still running ended with the process it ran in.
        status = snapshot.status == .running ? .stopped : snapshot.status
        toolUses = snapshot.toolUses
        tokens = snapshot.tokens
        durationMs = snapshot.durationMs
        report = snapshot.report
        entries = snapshot.entries.map(TranscriptEntry.init(snapshot:))
    }
}
