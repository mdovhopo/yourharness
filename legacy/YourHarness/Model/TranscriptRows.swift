import Foundation

/// A row of the transcript as shown: an entry, or a finished turn's work folded into one line.
enum TranscriptRow: Identifiable, Equatable {
    case entry(TranscriptEntry)
    case work(TurnWork)
    /// One step of a turn's opened work.
    case workStep(TranscriptEntry, isLast: Bool)
    /// Above the turns shown: how many earlier ones there are.
    case earlier(Int)
    /// Claude is working and nothing is streaming in.
    case typing

    var id: String {
        switch self {
        case .entry(let entry): entry.id
        case .work(let work): work.id
        case .workStep(let entry, _): entry.id
        case .earlier: "earlier-turns"
        case .typing: "typing-indicator"
        }
    }

    var entry: TranscriptEntry? {
        if case .entry(let entry) = self { entry } else { nil }
    }

    static func == (a: Self, b: Self) -> Bool {
        switch (a, b) {
        case let (.entry(x), .entry(y)): x === y
        case let (.work(x), .work(y)): x == y
        case let (.workStep(x, xLast), .workStep(y, yLast)): x === y && xLast == yLast
        case let (.earlier(x), .earlier(y)): x == y
        case (.typing, .typing): true
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
    /// The files the turn changed, as git saw them; nil outside a repo or until it's known.
    var changedFiles: [String]? = nil

    var id: String { "work-" + turnID }

    static func == (a: Self, b: Self) -> Bool {
        a.turnID == b.turnID && a.isExpanded == b.isExpanded && a.endedAt == b.endedAt && a.changedFiles == b.changedFiles
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
    /// Claude's answer. The turn in progress shows what's recent, so you can follow along;
    /// messages you sent during a turn, and notices, always show.
    ///
    /// Only the last `shownTurns` turns: a long thread opens as fast as a short one, and
    /// scrolling to the top shows more.
    var transcriptDisplayRows: [TranscriptRow] {
        let revision = rowsRevision
        if let cached = displayRowsCache, cached.revision == revision { return cached.rows }
        let rows = computeDisplayRows()
        displayRowsCache = (revision, rows)
        return rows
    }

    private func computeDisplayRows() -> [TranscriptRow] {
        let rows = transcriptRows
        let turns = transcriptTurnStarts
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
            result.append(contentsOf: isCurrent ? currentTurnRows(turn) : Self.fold(turn, isCurrent: false, expanded: expandedTurns))
        }
        return result
    }

    /// The turn in progress: once it's long, what's older goes into its work row, as it will
    /// when the turn ends, so the rows on screen are the same few however long it runs. Hundreds
    /// of tool calls in, switching to a thread that made them all took seconds. In steps, not
    /// row by row, and not while you're reading back, so nothing moves under you.
    private func currentTurnRows(_ turn: ArraySlice<TranscriptEntry>) -> [TranscriptRow] {
        guard let first = turn.first else { return [] }
        var folded = runningFold?.turnID == first.id ? runningFold?.rows ?? 0 : 0
        if !isReadingBack, turn.count - folded > Self.runningRowsLimit { folded = turn.count - Self.runningRowsKept }
        runningFold = (first.id, folded)
        guard folded > 1 else { return turn.map(TranscriptRow.entry) }
        let older = turn.prefix(folded).dropFirst()
        let shown = older.filter { Self.alwaysShows($0) }
        let work = TurnWork(turnID: first.id, entries: older.filter { !Self.alwaysShows($0) }, startedAt: first.date, endedAt: nil,
                            isExpanded: expandedTurns.contains(first.id) || expandsAll, changedFiles: first.changedFiles)
        return [.entry(first), .work(work)] + Self.steps(of: work) + shown.map(TranscriptRow.entry) + turn.dropFirst(folded).map(TranscriptRow.entry)
    }

    private static let runningRowsLimit = ProcessInfo.processInfo.environment["YH_RUNNING_ROWS"].flatMap(Int.init) ?? 90
    private static var runningRowsKept: Int { runningRowsLimit * 4 / 9 }

    private static func alwaysShows(_ entry: TranscriptEntry) -> Bool {
        entry.kind == .user || entry.kind == .notice || entry.kind == .commandOutput
            // What Claude put in front of you to see.
            || entry.kind == .tool && entry.toolName == MediaTool.toolName && !entry.toolIsError
    }

    /// Where each turn starts in `rows`.
    static func turnStarts(in rows: [TranscriptEntry]) -> [Int] {
        rows.indices.filter { rows[$0].startsTurn }
    }

    /// When the turn started: the typing indicator shows how long it's been going.
    var turnStartedAt: Date? { entries.last(where: \.startsTurn)?.date }

    /// Shows enough earlier turns that `entryID` is among them.
    func showTurns(including entryID: String) {
        let rows = transcriptRows
        guard let position = rows.firstIndex(where: { $0.id == entryID }) else { return }
        let turns = transcriptTurnStarts
        let turn = turns.lastIndex(where: { $0 <= position }) ?? 0
        let needed = turns.count - turn
        if needed > shownTurns { shownTurns = needed }
    }

    private static func steps(of work: TurnWork) -> [TranscriptRow] {
        guard work.isExpanded else { return [] }
        return work.entries.enumerated().map { .workStep($1, isLast: $0 == work.entries.count - 1) }
    }

    private static func fold(_ turn: ArraySlice<TranscriptEntry>, isCurrent: Bool, expanded: Set<String>) -> [TranscriptRow] {
        guard let first = turn.first, !isCurrent else { return turn.map(TranscriptRow.entry) }
        let answer = turn.last { $0.kind == .assistant }
        let shows: (TranscriptEntry) -> Bool = { entry in
            entry === first || entry === answer || alwaysShows(entry)
        }
        let hidden = turn.filter { !shows($0) }
        guard !hidden.isEmpty else { return turn.map(TranscriptRow.entry) }
        // Development: `YH_EXPAND_WORK=1` opens every turn's work, for checking how it looks.
        let isExpanded = expanded.contains(first.id) || expandsAll
        let work = TurnWork(turnID: first.id, entries: hidden, startedAt: first.date, endedAt: first.turnEndedAt, isExpanded: isExpanded,
                            changedFiles: first.changedFiles)
        // Opened, the work shows as rows of their own, reading as one group: a turn can hold
        // hundreds of tool calls, and only the ones near the screen are made.
        var rows: [TranscriptRow] = [.entry(first), .work(work)] + steps(of: work)
        for entry in turn.dropFirst() where shows(entry) {
            rows.append(.entry(entry))
        }
        return rows
    }
}
