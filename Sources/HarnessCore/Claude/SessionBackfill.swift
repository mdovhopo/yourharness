import Foundation

/// What a session's own history has that a saved transcript lacks: the app quit or crashed
/// mid-turn (the CLI writes as it goes; the transcript was saved a moment earlier), or the
/// session went on elsewhere with `claude --resume`.
public struct SessionBackfill: Equatable, Sendable {
    /// Messages the transcript has only the start of, by their position in `known`, with
    /// the whole message: a reply cut off mid-stream, a tool call that has no result yet.
    public var completed: [Int: ClaudeSessionArchive.Item] = [:]
    /// What came after the transcript's last message, in order.
    public var missing: [ClaudeSessionArchive.Item] = []

    public var isEmpty: Bool { completed.isEmpty && missing.isEmpty }

    public init(completed: [Int: ClaudeSessionArchive.Item] = [:], missing: [ClaudeSessionArchive.Item] = []) {
        self.completed = completed
        self.missing = missing
    }

    /// Lines `known` (the transcript, oldest first) up with `archive` and returns what's
    /// left over. It anchors on the newest tool call or message you sent that both have,
    /// then walks forward; where they disagree it returns nothing rather than guess.
    public init(known: [ClaudeSessionArchive.Item], archive: ClaudeSessionArchive) {
        let items = archive.items
        let ours = known.indices.filter { Self.isComparable(known[$0]) }
        let theirs = items.indices.filter { Self.isComparable(items[$0]) }

        var toolAt: [String: Int] = [:], userAt: [String: Int] = [:]
        for (position, index) in theirs.enumerated() {
            switch items[index] {
            case .tool(let id, _, _, _, _): toolAt[id] = position
            case .user(let text): userAt[text] = position
            default: break
            }
        }
        var anchor: (ours: Int, theirs: Int)?
        for position in ours.indices.reversed() {
            switch known[ours[position]] {
            case .tool(let id, _, _, _, _): anchor = toolAt[id].map { (position, $0) }
            case .user(let text): anchor = userAt[text].map { (position, $0) }
            default: continue
            }
            if anchor != nil { break }
        }
        guard let anchor else { return }

        var result = SessionBackfill()
        var mine = anchor.ours, session = anchor.theirs
        // The anchor itself may be a tool call that was still running.
        while mine < ours.count {
            guard session < theirs.count else { return }
            switch Self.compare(known[ours[mine]], items[theirs[session]]) {
            case .same: break
            case .completes(let whole): result.completed[ours[mine]] = whole
            case .different: return
            }
            mine += 1
            session += 1
        }
        let lastMatched = theirs[session - 1]
        result.missing = Array(items[(lastMatched + 1)...])
        self = result
    }

    /// Replies, tool calls and what you typed; not notices or thinking, which each side
    /// records its own way, nor slash commands, which the CLI keeps as notices.
    private static func isComparable(_ item: ClaudeSessionArchive.Item) -> Bool {
        switch item {
        case .user(let text): !text.hasPrefix("/")
        case .assistant, .tool: true
        case .thinking, .notice: false
        }
    }

    private enum Comparison { case same, completes(ClaudeSessionArchive.Item), different }

    private static func compare(_ mine: ClaudeSessionArchive.Item, _ theirs: ClaudeSessionArchive.Item) -> Comparison {
        switch (mine, theirs) {
        case (.user(let a), .user(let b)):
            return a == b ? .same : .different
        case (.assistant(let a), .assistant(let b)):
            let a = a.trimmingCharacters(in: .whitespacesAndNewlines)
            if a == b { return .same }
            return b.hasPrefix(a) ? .completes(theirs) : .different
        case (.tool(let a, _, _, let resultA, _), .tool(let b, _, _, let resultB, _)):
            guard a == b else { return .different }
            return resultA == nil && resultB != nil ? .completes(theirs) : .same
        default:
            return .different
        }
    }
}
