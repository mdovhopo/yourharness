import Foundation
import HarnessCore

/// A turn cut off by a quit or crash (`turnInProgress` still set at launch): what Claude did
/// until then is in its session file, which the CLI writes as it goes. Lined up with the
/// saved rows (`SessionBackfill`), the rest is filled in and saved, with a note saying so.
enum Recovery {
    static func item(_ s: EntrySnapshot) -> ClaudeSessionArchive.Item {
        switch s.kind {
        case .user: .user(s.text.isEmpty ? "[Image]" : s.text)
        case .assistant: .assistant(s.text)
        case .thinking: .thinking(s.text)
        case .tool: .tool(id: s.id, name: s.toolName ?? "", inputJSON: s.toolInput ?? "", result: s.toolResult, isError: s.toolIsError ?? false)
        case .notice, .commandOutput: .notice(s.text)
        }
    }

    static func snapshot(_ item: ClaudeSessionArchive.Item, id: String) -> EntrySnapshot {
        switch item {
        case .user(let text): EntrySnapshot(id: id, kind: .user, text: text, date: Date())
        case .assistant(let text): EntrySnapshot(id: id, kind: .assistant, text: text, date: Date())
        case .thinking(let text): EntrySnapshot(id: id, kind: .thinking, text: text, date: Date())
        case .notice(let text): EntrySnapshot(id: id, kind: .notice, text: text, date: Date())
        case .tool(let toolID, let name, let input, let result, let isError):
            EntrySnapshot(id: toolID.isEmpty ? id : toolID, kind: .tool, text: "", toolName: name, toolInput: input,
                          toolResult: result, toolIsError: isError, date: Date())
        }
    }

    /// Off the main thread: the rows to change (by row number) and how many there are after.
    static func plan(db: HarnessDatabase, thread: UUID, sessionID: String?, environment: [String: String]) async -> (changes: [Int: EntrySnapshot], count: Int) {
        await Task.detached(priority: .utility) {
            var rows = db.loadTranscript(thread)
            var changes: [Int: EntrySnapshot] = [:]
            var backfill = SessionBackfill()
            let config = ClaudeSessionArchive.configDirectory(environment: environment)
            if let sessionID, let url = ClaudeSessionArchive.locate(sessionID: sessionID, configDirectory: config),
               let archive = try? ClaudeSessionArchive.load(from: url, sessionID: sessionID) {
                backfill = SessionBackfill(known: rows.map(item), archive: archive)
            }
            for (index, item) in backfill.completed where rows.indices.contains(index) {
                var row = snapshot(item, id: rows[index].id)
                row.date = rows[index].date
                rows[index] = row
                changes[index] = row
            }
            for item in backfill.missing {
                changes[rows.count] = snapshot(item, id: "r\(rows.count)")
                rows.append(changes[rows.count]!)
            }
            let note = backfill.isEmpty
                ? "YourHarness closed while Claude was working, so this turn stopped here."
                : "YourHarness closed while Claude was working. What Claude did until then was recovered from its session history."
            changes[rows.count] = EntrySnapshot(id: "r\(rows.count)", kind: .notice, text: note, date: Date())
            return (changes, rows.count + 1)
        }.value
    }
}
