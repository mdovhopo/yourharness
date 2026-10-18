import Foundation

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

    var noticeStyle = NoticeStyle.info

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
            noticeStyle: kind == .notice ? noticeStyle : nil
        )
    }

    convenience init(snapshot: EntrySnapshot) {
        self.init(id: snapshot.id, kind: snapshot.kind, text: snapshot.text)
        toolName = snapshot.toolName ?? ""
        toolInput = snapshot.toolInput ?? ""
        toolResult = snapshot.toolResult
        toolIsError = snapshot.toolIsError ?? false
        noticeStyle = snapshot.noticeStyle ?? .info
    }
}
