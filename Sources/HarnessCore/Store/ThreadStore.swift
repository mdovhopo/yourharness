import Foundation

/// A thread's rows while Claude writes them, owned here rather than by whoever draws them:
/// `AgentEvent`s go in, and what changed comes out as `ThreadPatch`es, for a view to mirror.
/// One reducer, so saving, search and the tools agents call see the same rows the screen does.
public final class ThreadStore {
    public struct Entry: Sendable, Equatable {
        public var id: String
        public var kind: EntryKind
        public var text: String = ""
        public var toolName: String?
        public var toolInput: String?
        public var toolResult: String?
        public var resultLength: Int?
        public var isError = false
        /// Still being written.
        public var live = false
        public var subagent: Subagent?

        public struct Subagent: Sendable, Equatable {
            public var description: String
            public var status: String
            public var steps: Int
        }
    }

    public enum Field: String, Sendable { case text, input }

    public enum ThreadPatch: Sendable, Equatable {
        /// A new row at the end.
        case add(thread: String, entry: Entry)
        /// Streamed text at the end of a row's text or tool input.
        case append(thread: String, id: String, field: Field, text: String)
        /// A row changed otherwise: its whole new state.
        case update(thread: String, entry: Entry)
        case running(thread: String, Bool)
        case permission(thread: String, PermissionRequest?)
        case session(thread: String, id: String)
    }

    private final class Thread {
        var entries: [Entry] = []
        var index: [String: Int] = [:]
        /// Streaming blocks: message ID and index to row ID.
        var blocks: [String: String] = [:]
        var permission: PermissionRequest?
    }

    private var threads: [String: Thread] = [:]
    /// Results longer than this keep their head: they're shown folded, and there are hundreds a turn.
    public var resultLimit: Int
    public var inputLimit: Int
    private var counter = 0

    public init(resultLimit: Int = 1500, inputLimit: Int = 4000) {
        self.resultLimit = resultLimit
        self.inputLimit = inputLimit
    }

    /// The rows held for a thread, newest last.
    public func entries(_ thread: String) -> [Entry] {
        threads[thread]?.entries ?? []
    }

    public func pendingPermission(_ thread: String) -> PermissionRequest? {
        threads[thread]?.permission
    }

    private func state(_ id: String) -> Thread {
        if let thread = threads[id] { return thread }
        let thread = Thread()
        threads[id] = thread
        return thread
    }

    private func nextID(_ prefix: String) -> String {
        counter += 1
        return "\(prefix)\(counter)"
    }

    private func head(_ text: String, _ limit: Int) -> String {
        text.utf16.count > limit ? String(text.prefix(limit)) : text
    }

    private func add(_ entry: Entry, to t: Thread, thread: String) -> ThreadPatch {
        t.index[entry.id] = t.entries.count
        t.entries.append(entry)
        return .add(thread: thread, entry: entry)
    }

    private func update(_ id: String, in t: Thread, thread: String, _ change: (inout Entry) -> Void) -> ThreadPatch? {
        guard let i = t.index[id] else { return nil }
        let before = t.entries[i]
        change(&t.entries[i])
        return t.entries[i] == before ? nil : .update(thread: thread, entry: t.entries[i])
    }

    /// Your message, as sent.
    public func userMessage(_ text: String, thread: String) -> [ThreadPatch] {
        let t = state(thread)
        return [add(Entry(id: nextID("u"), kind: .user, text: text), to: t, thread: thread), .running(thread: thread, true)]
    }

    public func notice(_ text: String, thread: String) -> [ThreadPatch] {
        [add(Entry(id: nextID("n"), kind: .notice, text: text), to: state(thread), thread: thread)]
    }

    /// Forgets a permission request once it's answered.
    public func answered(_ thread: String) -> [ThreadPatch] {
        guard let t = threads[thread], t.permission != nil else { return [] }
        t.permission = nil
        return [.permission(thread: thread, nil)]
    }

    public func apply(_ event: AgentEvent, thread: String) -> [ThreadPatch] {
        let t = state(thread)
        switch event {
        case .sessionStarted(let info):
            return [.session(thread: thread, id: info.sessionID)]

        case .blockStarted(let ref, let kind):
            if let parent = ref.parentToolUseID { return subagentStep(parent, in: t, thread: thread) }
            let key = "\(ref.messageID):\(ref.index)"
            guard t.blocks[key] == nil else { return [] }
            var entry: Entry
            switch kind {
            case .text: entry = Entry(id: key, kind: .assistant)
            case .thinking: entry = Entry(id: key, kind: .thinking)
            case .toolUse(let id, let name): entry = Entry(id: id, kind: .tool); entry.toolName = name
            }
            entry.live = true
            t.blocks[key] = entry.id
            return [add(entry, to: t, thread: thread)]

        case .textDelta(let ref, let text), .thinkingDelta(let ref, let text), .toolInputDelta(let ref, let text):
            guard ref.parentToolUseID == nil else { return [] }
            let key = "\(ref.messageID):\(ref.index)"
            var patches: [ThreadPatch] = []
            if t.blocks[key] == nil {
                var kind = EntryKind.assistant
                if case .thinkingDelta = event { kind = .thinking }
                var entry = Entry(id: key, kind: kind)
                entry.live = true
                t.blocks[key] = key
                patches.append(add(entry, to: t, thread: thread))
            }
            guard let id = t.blocks[key], let i = t.index[id] else { return patches }
            if case .toolInputDelta = event {
                t.entries[i].toolInput = (t.entries[i].toolInput ?? "") + text
                patches.append(.append(thread: thread, id: id, field: .input, text: text))
            } else {
                t.entries[i].text += text
                patches.append(.append(thread: thread, id: id, field: .text, text: text))
            }
            return patches

        case .assistantMessage(let messageID, let blocks, let parent):
            if let parent {
                return blocks.reduce(into: []) { patches, block in
                    if case .toolUse = block { patches += subagentStep(parent, in: t, thread: thread) }
                }
            }
            var patches: [ThreadPatch] = []
            for (index, block) in blocks.enumerated() {
                let key = "\(messageID):\(index)"
                var id = t.blocks[key]
                if id == nil, case .toolUse(let toolID, _, _) = block, t.index[toolID] != nil { id = toolID }
                if id == nil {
                    let entry: Entry
                    switch block {
                    case .text: entry = Entry(id: key, kind: .assistant)
                    case .thinking: entry = Entry(id: key, kind: .thinking)
                    case .toolUse(let toolID, _, _): entry = Entry(id: toolID, kind: .tool)
                    }
                    t.blocks[key] = entry.id
                    patches.append(add(entry, to: t, thread: thread))
                    id = entry.id
                }
                guard let id else { continue }
                let change = update(id, in: t, thread: thread) { entry in
                    switch block {
                    case .text(let text), .thinking(let text): entry.text = text
                    case .toolUse(_, let name, let input): entry.toolName = name; entry.toolInput = head(input, inputLimit)
                    }
                    entry.live = false
                }
                if let change { patches.append(change) }
            }
            return patches

        case .toolResults(let results, let parent):
            guard parent == nil else { return [] }
            return results.compactMap { result in
                update(result.toolUseID, in: t, thread: thread) { entry in
                    entry.toolResult = head(result.content, resultLimit)
                    entry.resultLength = result.content.count
                    entry.isError = result.isError
                    entry.live = false
                }
            }

        case .subagentStarted(let start):
            return update(start.toolUseID, in: t, thread: thread) {
                $0.subagent = Entry.Subagent(description: start.description, status: "running", steps: 0)
            }.map { [$0] } ?? []

        case .subagentFinished(let end):
            return update(end.toolUseID, in: t, thread: thread) {
                $0.subagent = Entry.Subagent(description: $0.subagent?.description ?? "", status: end.status,
                                             steps: end.usage.toolUses ?? $0.subagent?.steps ?? 0)
            }.map { [$0] } ?? []

        case .permissionRequest(let request):
            t.permission = request
            return [.permission(thread: thread, request)]

        case .compacted(let compaction):
            return notice("Conversation compacted (\(compaction.trigger))", thread: thread)

        case .commandOutput(let text):
            return [add(Entry(id: nextID("c"), kind: .commandOutput, text: text), to: t, thread: thread)]

        case .turnCompleted(let result):
            var patches = endTurn(t, thread: thread)
            if result.isError { patches += notice(result.resultText ?? result.subtype, thread: thread) }
            return patches

        case .processExited(let code, let tail):
            var patches = endTurn(t, thread: thread)
            if code != 0 { patches += notice("claude exited (\(code)) \(tail.suffix(600))", thread: thread) }
            return patches

        default:
            return []
        }
    }

    private func subagentStep(_ parent: String, in t: Thread, thread: String) -> [ThreadPatch] {
        update(parent, in: t, thread: thread) {
            var sub = $0.subagent ?? Entry.Subagent(description: "Sub-agent", status: "running", steps: 0)
            sub.steps += 1
            $0.subagent = sub
        }.map { [$0] } ?? []
    }

    private func endTurn(_ t: Thread, thread: String) -> [ThreadPatch] {
        var patches: [ThreadPatch] = []
        for id in Set(t.blocks.values) {
            if let change = update(id, in: t, thread: thread, { $0.live = false }) { patches.append(change) }
        }
        t.blocks.removeAll()
        if t.permission != nil {
            t.permission = nil
            patches.append(.permission(thread: thread, nil))
        }
        patches.append(.running(thread: thread, false))
        return patches
    }
}
