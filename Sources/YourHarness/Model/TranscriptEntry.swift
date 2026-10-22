import Foundation
import HarnessCore

/// One row in a thread's transcript. A class, so a streaming delta only invalidates the
/// row it touches, not the whole list.
@MainActor @Observable
final class TranscriptEntry: Identifiable {
    enum Kind: String, Codable {
        /// `commandOutput`: what a built-in command printed, e.g. `/context`.
        case user, assistant, thinking, tool, notice, commandOutput
    }

    enum NoticeStyle: String, Codable {
        case info, error
    }

    let id: String
    let kind: Kind
    var text: String
    var isStreaming: Bool
    /// When it happened; nil for rows saved before times were kept, or taken from a
    /// session's history.
    var date: Date?
    /// On the message that starts a turn: when Claude finished it.
    var turnEndedAt: Date?

    // Tool calls
    var toolName = ""
    /// JSON; partial while the model is still streaming the input.
    var toolInput = ""
    var toolResult: String?
    var toolIsError = false
    /// For an Agent tool call: the sub-agent it started.
    var subagent: SubagentRun?

    var noticeStyle = NoticeStyle.info
    /// Images sent with a user message: file names in the store's attachments folder.
    var attachments: [String] = []
    /// How a message you sent while Claude was working got to it.
    var delivery = Delivery.normal

    enum Delivery: String, Codable {
        case normal
        /// Waited for Claude's current step, then joined the turn: steering.
        case steered
        /// Claude was stopped so this went in right away.
        case interrupted
    }

    init(id: String = UUID().uuidString, kind: Kind, text: String = "", isStreaming: Bool = false, date: Date? = .now) {
        self.id = id
        self.kind = kind
        self.text = text
        self.isStreaming = isStreaming
        self.date = date
    }

    /// A message you sent that starts a turn, rather than one that joined Claude's work
    /// (an interruption stops the turn, so it starts the next).
    var startsTurn: Bool { kind == .user && delivery != .steered }

    static func notice(_ text: String, style: NoticeStyle = .info) -> TranscriptEntry {
        let entry = TranscriptEntry(kind: .notice, text: text)
        entry.noticeStyle = style
        return entry
    }
}

// MARK: - Persistence

struct EntrySnapshot: Codable, Sendable {
    var id: String
    var kind: TranscriptEntry.Kind
    var text: String
    var toolName: String?
    var toolInput: String?
    var toolResult: String?
    var toolIsError: Bool?
    var noticeStyle: TranscriptEntry.NoticeStyle?
    var attachments: [String]?
    var subagent: SubagentSnapshot?
    var delivery: TranscriptEntry.Delivery?
    var date: Date?
    var turnEndedAt: Date?
}

extension TranscriptEntry {
    var snapshot: EntrySnapshot {
        EntrySnapshot(
            id: id,
            kind: kind,
            text: text,
            toolName: kind == .tool ? toolName : nil,
            toolInput: kind == .tool ? toolInput : nil,
            toolResult: toolResult,
            toolIsError: toolIsError ? true : nil,
            noticeStyle: kind == .notice ? noticeStyle : nil,
            attachments: attachments.isEmpty ? nil : attachments,
            subagent: subagent?.snapshot,
            delivery: delivery == .normal ? nil : delivery,
            date: date,
            turnEndedAt: turnEndedAt
        )
    }

    convenience init(snapshot: EntrySnapshot) {
        self.init(id: snapshot.id, kind: snapshot.kind, text: snapshot.text, date: snapshot.date)
        turnEndedAt = snapshot.turnEndedAt
        toolName = snapshot.toolName ?? ""
        toolInput = snapshot.toolInput ?? ""
        toolResult = snapshot.toolResult
        toolIsError = snapshot.toolIsError ?? false
        noticeStyle = snapshot.noticeStyle ?? .info
        attachments = snapshot.attachments ?? []
        subagent = snapshot.subagent.map { SubagentRun(id: snapshot.id, snapshot: $0) }
        delivery = snapshot.delivery ?? .normal
    }
}

extension TranscriptEntry {
    /// As the session's own history records it, to line the two up.
    var archiveItem: ClaudeSessionArchive.Item {
        switch kind {
        case .user: .user(text.isEmpty ? "[Image]" : text)
        case .assistant: .assistant(text)
        case .thinking: .thinking(text)
        case .tool: .tool(id: id, name: toolName, inputJSON: toolInput, result: toolResult, isError: toolIsError)
        case .notice, .commandOutput: .notice(text)
        }
    }

    /// Fills in what was cut off: the rest of a reply, a tool call's result.
    func complete(with item: ClaudeSessionArchive.Item) {
        switch item {
        case .assistant(let text), .thinking(let text): self.text = text
        case .tool(_, _, _, let result, let isError):
            toolResult = result
            toolIsError = isError
        case .user, .notice: break
        }
    }

    /// A message from a resumed Claude Code session.
    convenience init(item: ClaudeSessionArchive.Item) {
        switch item {
        case .user(let text):
            self.init(kind: .user, text: text, date: nil)
        case .assistant(let text):
            self.init(kind: .assistant, text: text, date: nil)
        case .thinking(let text):
            self.init(kind: .thinking, text: text, date: nil)
        case .tool(let id, let name, let input, let result, let isError):
            self.init(id: id, kind: .tool, date: nil)
            toolName = name
            toolInput = input
            toolResult = result
            toolIsError = isError
        case .notice(let text):
            self.init(kind: .notice, text: text, date: nil)
        }
    }
}
