import Foundation

/// A row of the transcript as shown: an entry, or a finished turn's work folded into one line.
enum TranscriptRow: Identifiable, Equatable {
    case entry(TranscriptEntry)
    case work(TurnWork)
    /// Above the turns shown: how many earlier ones there are.
    case earlier(Int)

    var id: String {
        switch self {
        case .entry(let entry): entry.id
        case .work(let work): work.id
        case .earlier: "earlier-turns"
        }
    }

    var entry: TranscriptEntry? {
        if case .entry(let entry) = self { entry } else { nil }
    }

    static func == (a: Self, b: Self) -> Bool {
        switch (a, b) {
        case let (.entry(x), .entry(y)): x === y
        case let (.work(x), .work(y)): x == y
        case let (.earlier(x), .earlier(y)): x == y
        default: false
        }
    }
}

/// What Claude did on the way to its answer in one finished turn: thinking, tool calls,
/// sub-agents, and what it said in between.
struct TurnWork: Equatable {
    /// The ID of the message that started the turn.
    let turnID: String
    let entries: [TranscriptEntry]
    let startedAt: Date?
    let endedAt: Date?
    let isExpanded: Bool

    var id: String { "work-" + turnID }

    static func == (a: Self, b: Self) -> Bool {
        a.turnID == b.turnID && a.isExpanded == b.isExpanded && a.endedAt == b.endedAt
            && a.entries.count == b.entries.count && a.entries.last === b.entries.last
    }

    var duration: TimeInterval? {
        guard let startedAt, let endedAt else { return nil }
        return max(endedAt.timeIntervalSince(startedAt), 0)
    }
}

/// "Previous message", "next message", or the thread's very top or end, once per request.
struct MessageJump: Equatable {
    enum Direction { case previous, next, top, bottom }
    let id = UUID()
    let direction: Direction
}

private let expandsAll = ProcessInfo.processInfo.environment["YH_EXPAND_WORK"] != nil

extension ThreadModel {
    /// The transcript with each finished turn's work folded between your message and
    /// Claude's answer. The turn in progress shows everything, so you can follow along;
    /// messages you sent during a turn, and notices, always show.
    ///
    /// Only the last `shownTurns` turns: a long thread opens as fast as a short one, and
    /// scrolling to the top shows more.
    var transcriptDisplayRows: [TranscriptRow] {
        let rows = transcriptRows
        let turns = Self.turnStarts(in: rows)
        let hidden = max(turns.count - shownTurns, 0)
        var result: [TranscriptRow] = []
        var index: Int
        if hidden > 0 {
            result.append(.earlier(hidden))
            index = turns[hidden]
        } else {
            // Anything before the first message you sent (a resumed session's notice) shows as is.
            index = 0
            while index < rows.count, !rows[index].startsTurn {
                result.append(.entry(rows[index]))
                index += 1
            }
        }
        while index < rows.count {
            let start = index
            index += 1
            while index < rows.count, !rows[index].startsTurn { index += 1 }
            let turn = rows[start..<index]
            let isCurrent = index == rows.count && (phase == .running || !pendingPermissions.isEmpty)
            result.append(contentsOf: Self.fold(turn, isCurrent: isCurrent, expanded: expandedTurns))
        }
        return result
    }

    /// Where each turn starts in `rows`.
    static func turnStarts(in rows: [TranscriptEntry]) -> [Int] {
        rows.indices.filter { rows[$0].startsTurn }
    }

    /// Shows enough earlier turns that `entryID` is among them.
    func showTurns(including entryID: String) {
        let rows = transcriptRows
        guard let position = rows.firstIndex(where: { $0.id == entryID }) else { return }
        let turns = Self.turnStarts(in: rows)
        let turn = turns.lastIndex(where: { $0 <= position }) ?? 0
        let needed = turns.count - turn
        if needed > shownTurns { shownTurns = needed }
    }

    private static func fold(_ turn: ArraySlice<TranscriptEntry>, isCurrent: Bool, expanded: Set<String>) -> [TranscriptRow] {
        guard let first = turn.first, !isCurrent else { return turn.map(TranscriptRow.entry) }
        let answer = turn.last { $0.kind == .assistant }
        let shows: (TranscriptEntry) -> Bool = { entry in
            entry === first || entry === answer || entry.kind == .user || entry.kind == .notice || entry.kind == .commandOutput
                // What Claude put in front of you to see.
                || entry.kind == .tool && entry.toolName == MediaTool.toolName && !entry.toolIsError
        }
        let hidden = turn.filter { !shows($0) }
        guard !hidden.isEmpty else { return turn.map(TranscriptRow.entry) }
        // Development: `YH_EXPAND_WORK=1` opens every turn's work, for checking how it looks.
        let isExpanded = expanded.contains(first.id) || expandsAll
        let work = TurnWork(turnID: first.id, entries: hidden, startedAt: first.date, endedAt: first.turnEndedAt, isExpanded: isExpanded)
        // Opened, the work shows inside its own row, as one group.
        var rows: [TranscriptRow] = [.entry(first), .work(work)]
        for entry in turn.dropFirst() where shows(entry) {
            rows.append(.entry(entry))
        }
        return rows
    }
}
