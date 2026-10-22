import Foundation

/// A row of the transcript as shown: an entry, or a finished turn's work folded into one line.
enum TranscriptRow: Identifiable, Equatable {
    case entry(TranscriptEntry)
    case work(TurnWork)

    var id: String {
        switch self {
        case .entry(let entry): entry.id
        case .work(let work): work.id
        }
    }

    var entry: TranscriptEntry? {
        if case .entry(let entry) = self { entry } else { nil }
    }

    static func == (a: Self, b: Self) -> Bool {
        switch (a, b) {
        case let (.entry(x), .entry(y)): x === y
        case let (.work(x), .work(y)): x == y
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

/// "Previous message" or "next message", once per request.
struct MessageJump: Equatable {
    enum Direction { case previous, next }
    let id = UUID()
    let direction: Direction
}

extension ThreadModel {
    /// The transcript with each finished turn's work folded between your message and
    /// Claude's answer. The turn in progress shows everything, so you can follow along;
    /// messages you sent during a turn, and notices, always show.
    var transcriptDisplayRows: [TranscriptRow] {
        let rows = transcriptRows
        var result: [TranscriptRow] = []
        result.reserveCapacity(rows.count)
        var index = 0
        // Anything before the first message you sent (a resumed session's notice) shows as is.
        while index < rows.count, !rows[index].startsTurn {
            result.append(.entry(rows[index]))
            index += 1
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

    private static func fold(_ turn: ArraySlice<TranscriptEntry>, isCurrent: Bool, expanded: Set<String>) -> [TranscriptRow] {
        guard let first = turn.first, !isCurrent else { return turn.map(TranscriptRow.entry) }
        let answer = turn.last { $0.kind == .assistant }
        let shows: (TranscriptEntry) -> Bool = { entry in
            entry === first || entry === answer || entry.kind == .user || entry.kind == .notice || entry.kind == .commandOutput
        }
        let hidden = turn.filter { !shows($0) }
        guard !hidden.isEmpty else { return turn.map(TranscriptRow.entry) }
        let isExpanded = expanded.contains(first.id)
        let work = TurnWork(turnID: first.id, entries: hidden, startedAt: first.date, endedAt: first.turnEndedAt, isExpanded: isExpanded)
        var rows: [TranscriptRow] = [.entry(first), .work(work)]
        for entry in turn.dropFirst() where isExpanded || shows(entry) {
            rows.append(.entry(entry))
        }
        return rows
    }
}
