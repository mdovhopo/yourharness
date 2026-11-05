import Foundation
import HarnessCore

/// What happened since the last frame, for the page. Streamed text for the same block
/// arrives as many small deltas; they're joined here, so a frame carries one per block.
@MainActor
final class Outbox {
    private enum Item {
        case delta(thread: String, message: String, index: Int, parent: String?, kind: String, text: String)
        case event([String: Any])
    }
    private var items: [Item] = []
    var isEmpty: Bool { items.isEmpty }
    /// Called when the first item since the last flush arrives.
    var onPending: (() -> Void)?

    func delta(thread: String, ref: BlockRef, kind: String, text: String) {
        let wasEmpty = items.isEmpty
        if case .delta(let t, let m, let i, let p, let k, let s)? = items.last,
           t == thread, m == ref.messageID, i == ref.index, k == kind {
            items[items.count - 1] = .delta(thread: t, message: m, index: i, parent: p, kind: k, text: s + text)
        } else {
            items.append(.delta(thread: thread, message: ref.messageID, index: ref.index, parent: ref.parentToolUseID, kind: kind, text: text))
        }
        if wasEmpty { onPending?() }
    }

    func event(_ event: [String: Any]) {
        let wasEmpty = items.isEmpty
        items.append(.event(event))
        if wasEmpty { onPending?() }
    }

    func drain() -> Data? {
        guard !items.isEmpty else { return nil }
        let objects: [[String: Any]] = items.map {
            switch $0 {
            case .delta(let thread, let message, let index, let parent, let kind, let text):
                var object: [String: Any] = ["t": "d", "th": thread, "m": message, "i": index, "k": kind, "s": text]
                if let parent { object["p"] = parent }
                return object
            case .event(let event): return event
            }
        }
        items.removeAll(keepingCapacity: true)
        return try? JSONSerialization.data(withJSONObject: objects)
    }
}

/// The `claude` processes, one per thread that's been talked to, and their events as the
/// page's wire format.
@MainActor
final class LiveThreads {
    private struct Live {
        var session: ClaudeSession
        var pump: Task<Void, Never>
        var permissions: [String: PermissionRequest] = [:]
    }

    private var live: [String: Live] = [:]
    private let outbox: Outbox
    private var environment: [String: String]?
    private var executable: URL?

    init(outbox: Outbox) {
        self.outbox = outbox
    }

    var runningCount: Int { live.count }

    private func prepare() async throws -> ([String: String], URL) {
        if let environment, let executable { return (environment, executable) }
        let environment = await ShellEnvironment.load()
        let status = await CLIDiscovery.probe(.claude, environment: environment)
        guard let path = status.path else { throw HostError("claude isn't on PATH") }
        self.environment = environment
        executable = path
        return (environment, path)
    }

    func send(thread: String, text: String, cwd: String, sessionID: String?, model: String?, mode: String?) async throws {
        if live[thread] == nil {
            let (environment, executable) = try await prepare()
            var options = ClaudeLaunchOptions(executable: executable, workingDirectory: URL(fileURLWithPath: cwd, isDirectory: true),
                                              environment: environment, model: model, permissionMode: mode ?? "default",
                                              resumeSessionID: sessionID)
            options.persistsSession = true
            let session = try ClaudeSession(options: options)
            let pump = Task { [weak self] in
                for await event in session.events {
                    self?.handle(event, thread: thread)
                }
                self?.ended(thread)
            }
            live[thread] = Live(session: session, pump: pump)
        }
        try await live[thread]?.session.send(text: text)
    }

    func interrupt(thread: String) async {
        try? await live[thread]?.session.interrupt()
    }

    func setMode(thread: String, mode: String) async {
        try? await live[thread]?.session.setPermissionMode(mode)
    }

    func setModel(thread: String, model: String) async {
        try? await live[thread]?.session.setModel(model)
    }

    func respond(thread: String, requestID: String, allow: Bool) async {
        guard let request = live[thread]?.permissions.removeValue(forKey: requestID) else { return }
        try? await live[thread]?.session.respond(to: request, allow: allow)
    }

    func stopAll() {
        for (_, entry) in live { kill(entry.session.processIdentifier, SIGTERM) }
    }

    private func ended(_ thread: String) {
        live[thread] = nil
    }

    private func handle(_ event: AgentEvent, thread: String) {
        switch event {
        case .sessionStarted(let info):
            outbox.event(["t": "session", "th": thread, "sid": info.sessionID, "model": info.model ?? ""])
        case .status(let status):
            outbox.event(["t": "status", "th": thread, "s": status])
        case .blockStarted(let ref, let kind):
            var object: [String: Any] = ["t": "b", "th": thread, "m": ref.messageID, "i": ref.index]
            if let parent = ref.parentToolUseID { object["p"] = parent }
            switch kind {
            case .text: object["k"] = "text"
            case .thinking: object["k"] = "thinking"
            case .toolUse(let id, let name): object["k"] = "tool"; object["id"] = id; object["n"] = name
            }
            outbox.event(object)
        case .textDelta(let ref, let text): outbox.delta(thread: thread, ref: ref, kind: "text", text: text)
        case .thinkingDelta(let ref, let text): outbox.delta(thread: thread, ref: ref, kind: "thinking", text: text)
        case .toolInputDelta(let ref, let text): outbox.delta(thread: thread, ref: ref, kind: "input", text: text)
        case .assistantMessage(let messageID, let blocks, let parent):
            let wire: [[String: Any]] = blocks.map {
                switch $0 {
                case .text(let text): ["k": "text", "s": text]
                case .thinking(let text): ["k": "thinking", "s": text]
                case .toolUse(let id, let name, let input): ["k": "tool", "id": id, "n": name, "s": WireEntry.head(input) ?? ""]
                }
            }
            var object: [String: Any] = ["t": "am", "th": thread, "m": messageID, "b": wire]
            if let parent { object["p"] = parent }
            outbox.event(object)
        case .toolResults(let results, let parent):
            let wire: [[String: Any]] = results.map {
                ["id": $0.toolUseID, "s": WireEntry.head($0.content, limit: WireEntry.liveResultLimit) ?? "", "l": $0.content.count, "e": $0.isError ? 1 : 0]
            }
            var object: [String: Any] = ["t": "tr", "th": thread, "r": wire]
            if let parent { object["p"] = parent }
            outbox.event(object)
        case .subagentStarted(let start):
            outbox.event(["t": "sa", "th": thread, "id": start.toolUseID, "d": start.description])
        case .subagentFinished(let end):
            outbox.event(["t": "sae", "th": thread, "id": end.toolUseID, "s": end.status, "c": end.usage.toolUses ?? 0])
        case .permissionRequest(let request):
            live[thread]?.permissions[request.requestID] = request
            outbox.event(["t": "perm", "th": thread, "id": request.requestID, "n": request.toolName,
                          "d": request.description ?? "", "i": WireEntry.head(request.inputJSON) ?? ""])
        case .compacted(let compaction):
            outbox.event(["t": "notice", "th": thread, "s": "Conversation compacted (\(compaction.trigger))"])
        case .commandOutput(let text):
            outbox.event(["t": "command", "th": thread, "s": text])
        case .contextUsage(let tokens):
            outbox.event(["t": "ctx", "th": thread, "n": tokens])
        case .turnCompleted(let result):
            outbox.event(["t": "done", "th": thread, "err": result.isError ? 1 : 0, "ms": result.durationMs ?? 0,
                          "s": result.isError ? (result.resultText ?? result.subtype) : ""])
        case .processExited(let code, let tail):
            outbox.event(["t": "exit", "th": thread, "code": Int(code), "s": code == 0 ? "" : String(tail.suffix(600))])
        default:
            break
        }
    }
}

struct HostError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}
