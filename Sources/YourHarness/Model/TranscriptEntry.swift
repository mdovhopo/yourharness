import Foundation
import HarnessCore

/// One row in a thread's transcript. A class, so a streaming delta only invalidates the
/// row it touches, not the whole list.
@MainActor @Observable
final class TranscriptEntry: Identifiable {
    typealias Kind = EntryKind
    typealias NoticeStyle = HarnessCore.NoticeStyle
    typealias Delivery = MessageDelivery

    let id: String
    let kind: Kind
    var text: String
    var isStreaming: Bool
    // Set before the entry joins a transcript and never after, so not observed: the
    // transcript reads them for every entry each time it lays out its rows, and observing
    // 2,000 entries' worth of reads cost a switch to a long thread about 200 ms.

    /// When it happened; nil for rows saved before times were kept, or taken from a
    /// session's history.
    @ObservationIgnored var date: Date?
    /// On the message that starts a turn: when Claude finished it.
    var turnEndedAt: Date?
    /// On the message that starts a turn: the files it changed, as git saw them, however
    /// they were changed (absolute paths). Nil outside a repo, or before it's worked out.
    var changedFiles: [String]?

    // Tool calls
    @ObservationIgnored var toolName = ""
    /// JSON; partial while the model is still streaming the input.
    var toolInput = ""
    var toolResult: String?
    /// For a big result loaded as its head: where the whole one is kept, and its length.
    @ObservationIgnored var toolResultRef: String?
    @ObservationIgnored var toolResultLength: Int?
    var toolIsError = false
    /// For an Agent tool call: the sub-agent it started.
    var subagent: SubagentRun?
    /// For a command or watch that went on running: the job, while the session lasts.
    var job: BackgroundJob?

    var noticeStyle = NoticeStyle.info
    /// Images sent with a user message: file names in the store's attachments folder.
    var attachments: [String] = []
    /// How a message you sent while Claude was working got to it.
    @ObservationIgnored var delivery = Delivery.normal

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
            turnEndedAt: turnEndedAt,
            toolResultRef: toolResultRef,
            toolResultLength: toolResultLength,
            changedFiles: changedFiles
        )
    }

    convenience init(snapshot: EntrySnapshot) {
        self.init(id: snapshot.id, kind: snapshot.kind, text: snapshot.text, date: snapshot.date)
        turnEndedAt = snapshot.turnEndedAt
        changedFiles = snapshot.changedFiles
        toolName = snapshot.toolName ?? ""
        toolInput = snapshot.toolInput ?? ""
        toolResult = snapshot.toolResult
        toolResultRef = snapshot.toolResultRef
        toolResultLength = snapshot.toolResultLength
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
            toolResultRef = nil
            toolResultLength = nil
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
