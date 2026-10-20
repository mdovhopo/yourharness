import Foundation
import HarnessCore

/// One row in a thread's transcript. A class, so a streaming delta only invalidates the
/// row it touches, not the whole list.
@MainActor @Observable
final class TranscriptEntry: Identifiable {
    enum Kind: String, Codable {
        case user, assistant, thinking, tool, notice
    }

    enum NoticeStyle: String, Codable {
        case info, error
    }

    let id: String
    let kind: Kind
    var text: String
    var isStreaming: Bool

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

    init(id: String = UUID().uuidString, kind: Kind, text: String = "", isStreaming: Bool = false) {
        self.id = id
        self.kind = kind
        self.text = text
        self.isStreaming = isStreaming
    }

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
            subagent: subagent?.snapshot
        )
    }

    convenience init(snapshot: EntrySnapshot) {
        self.init(id: snapshot.id, kind: snapshot.kind, text: snapshot.text)
        toolName = snapshot.toolName ?? ""
        toolInput = snapshot.toolInput ?? ""
        toolResult = snapshot.toolResult
        toolIsError = snapshot.toolIsError ?? false
        noticeStyle = snapshot.noticeStyle ?? .info
        attachments = snapshot.attachments ?? []
        subagent = snapshot.subagent.map { SubagentRun(id: snapshot.id, snapshot: $0) }
    }
}

extension TranscriptEntry {
    /// A message from a resumed Claude Code session.
    convenience init(item: ClaudeSessionArchive.Item) {
        switch item {
        case .user(let text):
            self.init(kind: .user, text: text)
        case .assistant(let text):
            self.init(kind: .assistant, text: text)
        case .thinking(let text):
            self.init(kind: .thinking, text: text)
        case .tool(let id, let name, let input, let result, let isError):
            self.init(id: id, kind: .tool)
            toolName = name
            toolInput = input
            toolResult = result
            toolIsError = isError
        case .notice(let text):
            self.init(kind: .notice, text: text)
        }
    }
}
