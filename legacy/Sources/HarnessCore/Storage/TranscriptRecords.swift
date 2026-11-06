import Foundation

/// What a transcript row is, as saved. The app's live rows are made from these and back.
public enum EntryKind: String, Codable, Sendable {
    /// `commandOutput`: what a built-in command printed, e.g. `/context`.
    case user, assistant, thinking, tool, notice, commandOutput
}

public enum NoticeStyle: String, Codable, Sendable {
    case info, error
}

/// How a message you sent while Claude was working got to it.
public enum MessageDelivery: String, Codable, Sendable {
    case normal
    /// Waited for Claude's current step, then joined the turn: steering.
    case steered
    /// Claude was stopped so this went in right away.
    case interrupted
}

public enum SubagentStatus: String, Codable, Sendable {
    case running, completed, failed, stopped
}

/// One transcript row as saved.
public struct EntrySnapshot: Codable, Sendable, Equatable {
    public var id: String
    public var kind: EntryKind
    public var text: String
    public var toolName: String?
    public var toolInput: String?
    /// The whole result, or for a big one only its head: see `toolResultRef`.
    public var toolResult: String?
    public var toolIsError: Bool?
    public var noticeStyle: NoticeStyle?
    public var attachments: [String]?
    public var subagent: SubagentSnapshot?
    public var delivery: MessageDelivery?
    public var date: Date?
    public var turnEndedAt: Date?
    /// Set when `toolResult` is only the head of a big result: the whole one stays in the
    /// database under this key, so it isn't held in memory.
    public var toolResultRef: String?
    /// The whole result's length in characters, when `toolResult` is its head.
    public var toolResultLength: Int?
    /// On the message that started a turn: the files the turn changed, as git saw them.
    public var changedFiles: [String]?

    public init(id: String, kind: EntryKind, text: String, toolName: String? = nil, toolInput: String? = nil,
                toolResult: String? = nil, toolIsError: Bool? = nil, noticeStyle: NoticeStyle? = nil,
                attachments: [String]? = nil, subagent: SubagentSnapshot? = nil, delivery: MessageDelivery? = nil,
                date: Date? = nil, turnEndedAt: Date? = nil, toolResultRef: String? = nil, toolResultLength: Int? = nil,
                changedFiles: [String]? = nil) {
        self.id = id
        self.kind = kind
        self.text = text
        self.toolName = toolName
        self.toolInput = toolInput
        self.toolResult = toolResult
        self.toolIsError = toolIsError
        self.noticeStyle = noticeStyle
        self.attachments = attachments
        self.subagent = subagent
        self.delivery = delivery
        self.date = date
        self.turnEndedAt = turnEndedAt
        self.toolResultRef = toolResultRef
        self.toolResultLength = toolResultLength
        self.changedFiles = changedFiles
    }
}

/// A sub-agent's run as saved, its own transcript included.
public struct SubagentSnapshot: Codable, Sendable, Equatable {
    public var description: String
    public var agentType: String?
    public var prompt: String
    public var isBackground: Bool?
    public var status: SubagentStatus
    public var toolUses: Int?
    public var tokens: Int?
    public var durationMs: Int?
    public var report: String?
    public var startedAt: Date
    public var entries: [EntrySnapshot]

    public init(description: String, agentType: String?, prompt: String, isBackground: Bool?, status: SubagentStatus,
                toolUses: Int?, tokens: Int?, durationMs: Int?, report: String?, startedAt: Date, entries: [EntrySnapshot]) {
        self.description = description
        self.agentType = agentType
        self.prompt = prompt
        self.isBackground = isBackground
        self.status = status
        self.toolUses = toolUses
        self.tokens = tokens
        self.durationMs = durationMs
        self.report = report
        self.startedAt = startedAt
        self.entries = entries
    }
}

/// An element that may not decode: nil instead of failing the whole array.
public struct Lossy<Value: Decodable>: Decodable {
    public let value: Value?

    public init(from decoder: Decoder) throws {
        value = try? Value(from: decoder)
    }
}
