import Foundation
import HarnessCore

/// What goes to the page: plain JSON objects with short, stable keys (docs/protocol.md). Kept
/// free of Swift-only shapes, so another backend can speak it unchanged.
public typealias Wire = [String: Any]

enum WireLimits {
    /// Tool text the page holds per row: it shows a head, folded.
    static let toolText = 4000
    static let liveResult = 1500

    static func head(_ text: String?, _ limit: Int = toolText) -> String? {
        guard let text else { return nil }
        return text.utf16.count > limit ? String(text.prefix(limit)) : text
    }
}

extension ThreadStore.Entry {
    var wire: Wire {
        var o: Wire = ["id": id, "k": kind == .commandOutput ? "command" : kind.rawValue, "t": text]
        if let toolName { o["n"] = toolName }
        if let toolInput { o["i"] = WireLimits.head(toolInput) }
        if let toolResult { o["r"] = WireLimits.head(toolResult, WireLimits.liveResult) }
        if let resultLength { o["rl"] = resultLength }
        if isError { o["e"] = 1 }
        if live { o["live"] = 1 }
        if let date { o["d"] = date.timeIntervalSince1970 * 1000 }
        if let subagent { o["sa"] = ["d": subagent.description, "s": subagent.status, "c": subagent.steps] }
        return o
    }

    /// As the database keeps it, with the whole tool result when it was cut.
    func snapshot(fullResult: String?) -> EntrySnapshot {
        var s = EntrySnapshot(id: id, kind: kind, text: text, toolName: toolName, toolInput: toolInput,
                              toolResult: fullResult ?? toolResult, toolIsError: kind == .tool ? isError : nil, date: date)
        if let subagent {
            s.subagent = SubagentSnapshot(description: subagent.description, agentType: nil, prompt: "", isBackground: nil,
                                          status: SubagentStatus(rawValue: subagent.status) ?? .completed, toolUses: subagent.steps,
                                          tokens: nil, durationMs: nil, report: nil, startedAt: date ?? Date(), entries: [])
        }
        return s
    }
}

extension EntrySnapshot {
    var wire: Wire {
        var o: Wire = ["id": id, "k": kind == .commandOutput ? "command" : kind.rawValue, "t": text]
        if let toolName { o["n"] = toolName }
        if let toolInput { o["i"] = WireLimits.head(toolInput) }
        if let toolResult { o["r"] = WireLimits.head(toolResult) }
        if let length = toolResultLength ?? toolResult.map(\.count) { o["rl"] = length }
        if toolIsError == true { o["e"] = 1 }
        if let date { o["d"] = date.timeIntervalSince1970 * 1000 }
        if let subagent { o["sa"] = ["d": subagent.description, "s": subagent.status.rawValue, "c": subagent.toolUses ?? 0] }
        return o
    }
}

extension PermissionRequest {
    var wire: Wire {
        var o: Wire = ["id": requestID, "n": toolName, "d": description ?? "", "i": WireLimits.head(inputJSON, 20_000) ?? ""]
        if toolName == AgentQuestion.toolName, let questions = AgentQuestion.parse(inputJSON: inputJSON) {
            o["questions"] = questions.map { q in
                ["q": q.question, "h": q.header ?? "", "multi": q.multiSelect,
                 "options": q.options.map { ["label": $0.label, "d": $0.description ?? ""] }] as Wire
            }
        }
        if toolName == AgentPlan.toolName, let plan = AgentPlan.text(inputJSON: inputJSON) { o["plan"] = plan }
        return o
    }
}
