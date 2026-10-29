import Foundation

/// A normalized event emitted by an agent session. Adapters (Claude today, others later)
/// translate their wire protocol into these, and the UI only ever consumes these.
public enum AgentEvent: Sendable, Equatable {
    case sessionStarted(SessionInfo)
    case status(String)
    case blockStarted(BlockRef, BlockKind)
    case textDelta(BlockRef, String)
    case thinkingDelta(BlockRef, String)
    case toolInputDelta(BlockRef, String)
    /// A complete assistant message. Authoritative for tool inputs; text may already
    /// have arrived as deltas.
    case assistantMessage(messageID: String, blocks: [ContentBlock], parentToolUseID: String?)
    case toolResults([ToolResult], parentToolUseID: String?)
    /// A sub-agent (the Agent tool) started, moved on, or ended. Its own messages arrive as
    /// `assistantMessage`/`toolResults` whose parent is the Agent call.
    case subagentStarted(SubagentStart)
    case subagentProgress(toolUseID: String, activity: String?, usage: SubagentUsage)
    case subagentFinished(SubagentEnd)
    /// A job Claude left running: a command in the background, a foreground one that's been
    /// going a while, or a watch (Monitor). Claude is woken when one ends or a watch fires,
    /// in a turn of its own, after the one that started it may have ended.
    case jobStarted(JobStart)
    case jobFinished(JobEnd)
    /// What a watch saw: woken by it, Claude reads it next.
    case jobEvent(taskID: String, text: String)
    /// Every job still running, sent whenever that changes: the list to trust over the
    /// starts and ends.
    case jobsRunning([String])
    case permissionRequest(PermissionRequest)
    /// What a built-in command printed (`/context`, `/usage`, ...): markdown, not a reply.
    case commandOutput(String)
    /// The conversation was summarized to free context (`/compact`, or automatically).
    case compacted(Compaction)
    /// `/clear`: the CLI starts a new session with empty context.
    case conversationReset
    /// `/rename`.
    case titleChanged(String)
    /// A message you sent was queued, picked up by Claude, finished, or dropped.
    case messageLifecycle(id: String, state: MessageState)
    case rateLimit(RateLimitInfo)
    /// Tokens in the context window after the latest top-level assistant message.
    case contextUsage(tokens: Int)
    /// One API response's tokens, top-level or a sub-agent's. Sent more than once for a
    /// response as it grows (same key): the largest counts wait for the last.
    case messageUsage(MessageUsage)
    /// Reply to `initialize`: models, effort levels, CLI version.
    case catalog(ClaudeCatalog)
    case controlFailed(requestID: String, message: String)
    /// For a server the app serves (`MCPToolServer`). The session answers these itself;
    /// they never reach the UI.
    case mcpMessage(requestID: String, server: String, message: JSONValue)
    case turnCompleted(TurnResult)
    case processExited(code: Int32, stderrTail: String)
}

/// Identifies a content block within a streamed assistant message.
public struct BlockRef: Hashable, Sendable {
    public var messageID: String
    public var index: Int
    public var parentToolUseID: String?

    public init(messageID: String, index: Int, parentToolUseID: String? = nil) {
        self.messageID = messageID
        self.index = index
        self.parentToolUseID = parentToolUseID
    }
}

public enum BlockKind: Sendable, Equatable {
    case text
    case thinking
    case toolUse(id: String, name: String)
}

public enum ContentBlock: Sendable, Equatable {
    case text(String)
    case thinking(String)
    case toolUse(id: String, name: String, inputJSON: String)
}

public struct SessionInfo: Sendable, Equatable {
    public var sessionID: String
    public var model: String?
    public var cwd: String?
    public var permissionMode: String?
    public var cliVersion: String?
    /// What the agent can run itself with its Skill tool: skills, bundled ones included,
    /// as opposed to commands only the CLI runs (`/compact`).
    public var skills: [String]? = nil
}

public struct ToolResult: Sendable, Equatable {
    public var toolUseID: String
    public var content: String
    public var isError: Bool
}

public struct SubagentStart: Sendable, Equatable {
    /// The Agent tool call that started it.
    public var toolUseID: String
    /// What `stop_task` takes.
    public var taskID: String
    public var description: String
    /// e.g. "general-purpose", "Explore".
    public var agentType: String?
    public var prompt: String?
    /// Runs on while the main agent carries on; the turn can end before it does.
    public var isBackground: Bool
}

public struct SubagentUsage: Sendable, Equatable {
    public var tokens: Int?
    public var toolUses: Int?
    public var durationMs: Int?

    init(_ json: Any?) {
        let json = json as? [String: Any] ?? [:]
        tokens = json["total_tokens"] as? Int
        toolUses = json["tool_uses"] as? Int
        durationMs = json["duration_ms"] as? Int
    }
}

public struct SubagentEnd: Sendable, Equatable {
    public var toolUseID: String
    /// "completed", "failed", "stopped", ...
    public var status: String
    /// Its final report.
    public var summary: String?
    public var usage: SubagentUsage
}

public struct JobStart: Sendable, Equatable {
    /// What `stop_task` takes.
    public var taskID: String
    /// The tool call that started it (Bash, Monitor).
    public var toolUseID: String?
    public var description: String
    /// Started in the background; otherwise the turn waits on it.
    public var isBackground: Bool
}

public struct JobEnd: Sendable, Equatable {
    public var taskID: String
    public var toolUseID: String?
    /// "completed", "failed", "killed", "stopped", ...
    public var status: String
    /// e.g. `Background command "Build" failed with exit code 1`.
    public var summary: String?
    /// Everything it printed.
    public var outputFile: String?

    public var exitCode: Int? {
        guard let summary, let range = summary.range(of: "exit code ") else { return nil }
        return Int(summary[range.upperBound...].prefix { $0.isNumber || $0 == "-" })
    }
}

public enum MessageState: String, Sendable, Equatable {
    case queued, started, completed, cancelled
}

/// When a message sent during a turn reaches Claude.
public enum MessagePriority: String, Sendable {
    /// Stop the current turn for it.
    case now
    /// After the current step: steering. What the CLI does by default.
    case next
    /// After the turn ends.
    case later
}

public struct Compaction: Sendable, Equatable {
    /// "manual" or "auto".
    public var trigger: String
    public var tokensBefore: Int?
    public var tokensAfter: Int?
}

public struct PermissionRequest: Sendable, Equatable, Identifiable {
    public var id: String { requestID }
    public var requestID: String
    public var toolName: String
    public var description: String?
    /// The raw tool input, echoed back as `updatedInput` when allowing.
    public var inputJSON: String
}

public struct RateLimitInfo: Sendable, Equatable, Codable {
    public struct Window: Sendable, Equatable, Codable, Identifiable {
        public var id: String { name }
        /// e.g. "five_hour", "seven_day"
        public var name: String
        /// 0...1
        public var utilization: Double
        public var resetsAt: Date?
    }

    public var status: String
    public var windows: [Window]
}

public struct TurnResult: Sendable, Equatable {
    public var isError: Bool
    public var subtype: String
    public var durationMs: Int?
    public var costUSD: Double?
    public var resultText: String?
    /// Context window size per model used this turn, keyed by resolved model ID.
    public var contextWindows: [String: Int] = [:]
}
