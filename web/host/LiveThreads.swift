import Foundation
import HarnessCore

/// What changed since the last frame, for the page. Streamed text for the same row arrives
/// as many small appends; they're joined here, so a frame carries one per row.
@MainActor
final class Outbox {
    private enum Item {
        case append(thread: String, id: String, field: String, text: String)
        case event([String: Any])
    }
    private var items: [Item] = []
    var isEmpty: Bool { items.isEmpty }
    /// Called when the first item since the last flush arrives.
    var onPending: (() -> Void)?

    func append(thread: String, id: String, field: String, text: String) {
        let wasEmpty = items.isEmpty
        if case .append(let t, let i, let f, let s)? = items.last, t == thread, i == id, f == field {
            items[items.count - 1] = .append(thread: t, id: i, field: f, text: s + text)
        } else {
            items.append(.append(thread: thread, id: id, field: field, text: text))
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
            case .append(let thread, let id, let field, let text): ["t": "app", "th": thread, "id": id, "f": field, "s": text]
            case .event(let event): event
            }
        }
        items.removeAll(keepingCapacity: true)
        return try? JSONSerialization.data(withJSONObject: objects)
    }
}

extension ThreadStore.Entry {
    /// The page's row format (WireEntry in web/ui/src/host.ts), plus `live`.
    var wire: [String: Any] {
        var object: [String: Any] = ["id": id, "k": kind == .commandOutput ? "command" : kind.rawValue, "t": text]
        if let toolName { object["n"] = toolName }
        if let toolInput { object["i"] = toolInput }
        if let toolResult { object["r"] = toolResult }
        if let resultLength { object["rl"] = resultLength }
        if isError { object["e"] = 1 }
        if live { object["live"] = 1 }
        if let subagent { object["sa"] = ["d": subagent.description, "s": subagent.status, "c": subagent.steps] }
        return object
    }
}

/// The `claude` processes, one per thread that's been talked to. Their events go through
/// the `ThreadStore`, which owns the rows; the page only gets what changed.
@MainActor
final class LiveThreads {
    private struct Live {
        var session: ClaudeSession
        var pump: Task<Void, Never>
    }

    private var live: [String: Live] = [:]
    private let outbox: Outbox
    let store = ThreadStore(resultLimit: WireEntry.liveResultLimit, inputLimit: WireEntry.toolTextLimit)
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
        emit(store.userMessage(text, thread: thread))
        do {
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
        } catch {
            emit(store.notice("Couldn't send: \(error)", thread: thread))
            emit([.running(thread: thread, false)])
            throw error
        }
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
        guard let request = store.pendingPermission(thread), request.requestID == requestID else { return }
        emit(store.answered(thread))
        try? await live[thread]?.session.respond(to: request, allow: allow)
    }

    func stopAll() {
        for (_, entry) in live { kill(entry.session.processIdentifier, SIGTERM) }
    }

    private func ended(_ thread: String) {
        live[thread] = nil
    }

    private func handle(_ event: AgentEvent, thread: String) {
        emit(store.apply(event, thread: thread))
    }

    private func emit(_ patches: [ThreadStore.ThreadPatch]) {
        for patch in patches {
            switch patch {
            case .add(let thread, let entry):
                outbox.event(["t": "add", "th": thread, "e": entry.wire])
            case .append(let thread, let id, let field, let text):
                outbox.append(thread: thread, id: id, field: field == .text ? "t" : "i", text: text)
            case .update(let thread, let entry):
                outbox.event(["t": "upd", "th": thread, "e": entry.wire])
            case .running(let thread, let running):
                outbox.event(["t": "run", "th": thread, "v": running])
            case .permission(let thread, let request):
                if let request {
                    outbox.event(["t": "perm", "th": thread, "id": request.requestID, "n": request.toolName,
                                  "d": request.description ?? "", "i": WireEntry.head(request.inputJSON) ?? ""])
                } else {
                    outbox.event(["t": "perm", "th": thread])
                }
            case .session(let thread, let id):
                outbox.event(["t": "session", "th": thread, "sid": id])
            }
        }
    }
}

struct HostError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}
