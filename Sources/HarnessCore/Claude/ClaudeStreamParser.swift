import Foundation

/// Translates Claude Code's `--output-format stream-json` lines into `AgentEvent`s.
///
/// Unknown line types are ignored on purpose: the CLI adds event types regularly and the
/// harness must keep working when it does.
public struct ClaudeStreamParser: Sendable {
    /// Fallback for stream events that arrive without `api_message_id`.
    private var currentMessageID = ""

    public init() {}

    public mutating func parse(line: Data) -> [AgentEvent] {
        guard let object = try? JSONSerialization.jsonObject(with: line) as? [String: Any],
              let type = object["type"] as? String
        else { return [] }

        let parent = object["parent_tool_use_id"] as? String

        switch type {
        case "system":
            return parseSystem(object)
        case "stream_event":
            guard let event = object["event"] as? [String: Any] else { return [] }
            return parseStreamEvent(event, messageID: object["api_message_id"] as? String, parent: parent)
        case "assistant":
            guard let message = object["message"] as? [String: Any],
                  let id = message["id"] as? String
            else { return [] }
            let blocks = (message["content"] as? [[String: Any]] ?? []).compactMap(Self.contentBlock)
            return [.assistantMessage(messageID: id, blocks: blocks, parentToolUseID: parent)]
        case "user":
            guard let message = object["message"] as? [String: Any],
                  let content = message["content"] as? [[String: Any]]
            else { return [] }
            let results = content.compactMap(Self.toolResult)
            return results.isEmpty ? [] : [.toolResults(results, parentToolUseID: parent)]
        case "control_request":
            return parseControlRequest(object)
        case "control_response":
            return parseControlResponse(object)
        case "rate_limit_event":
            guard let info = object["rate_limit_info"] as? [String: Any] else { return [] }
            return [.rateLimit(Self.rateLimit(info))]
        case "result":
            let modelUsage = object["modelUsage"] as? [String: [String: Any]] ?? [:]
            return [.turnCompleted(TurnResult(
                isError: object["is_error"] as? Bool ?? false,
                subtype: object["subtype"] as? String ?? "",
                durationMs: object["duration_ms"] as? Int,
                costUSD: object["total_cost_usd"] as? Double,
                resultText: object["result"] as? String,
                contextWindows: modelUsage.compactMapValues { $0["contextWindow"] as? Int }
            ))]
        default:
            return []
        }
    }

    // MARK: - Line types

    private func parseSystem(_ object: [String: Any]) -> [AgentEvent] {
        switch object["subtype"] as? String {
        case "init":
            guard let sessionID = object["session_id"] as? String else { return [] }
            return [.sessionStarted(SessionInfo(
                sessionID: sessionID,
                model: object["model"] as? String,
                cwd: object["cwd"] as? String,
                permissionMode: object["permissionMode"] as? String,
                cliVersion: object["claude_code_version"] as? String
            ))]
        case "status":
            return (object["status"] as? String).map { [.status($0)] } ?? []
        default:
            return []
        }
    }

    private mutating func parseStreamEvent(_ event: [String: Any], messageID: String?, parent: String?) -> [AgentEvent] {
        switch event["type"] as? String {
        case "message_start":
            if let id = (event["message"] as? [String: Any])?["id"] as? String {
                currentMessageID = id
            }
            return []
        case "content_block_start":
            guard let index = event["index"] as? Int,
                  let block = event["content_block"] as? [String: Any]
            else { return [] }
            let ref = BlockRef(messageID: messageID ?? currentMessageID, index: index, parentToolUseID: parent)
            switch block["type"] as? String {
            case "text":
                return [.blockStarted(ref, .text)]
            case "thinking":
                return [.blockStarted(ref, .thinking)]
            case "tool_use":
                guard let id = block["id"] as? String, let name = block["name"] as? String else { return [] }
                return [.blockStarted(ref, .toolUse(id: id, name: name))]
            default:
                return []
            }
        case "content_block_delta":
            guard let index = event["index"] as? Int,
                  let delta = event["delta"] as? [String: Any]
            else { return [] }
            let ref = BlockRef(messageID: messageID ?? currentMessageID, index: index, parentToolUseID: parent)
            switch delta["type"] as? String {
            case "text_delta":
                return (delta["text"] as? String).map { [.textDelta(ref, $0)] } ?? []
            case "thinking_delta":
                return (delta["thinking"] as? String).map { [.thinkingDelta(ref, $0)] } ?? []
            case "input_json_delta":
                guard let partial = delta["partial_json"] as? String, !partial.isEmpty else { return [] }
                return [.toolInputDelta(ref, partial)]
            default:
                return []
            }
        case "message_delta":
            // Final usage for the message: everything the model saw plus what it wrote.
            guard parent == nil, let usage = event["usage"] as? [String: Any] else { return [] }
            let tokens = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens"]
                .compactMap { usage[$0] as? Int }
                .reduce(0, +)
            return tokens > 0 ? [.contextUsage(tokens: tokens)] : []
        default:
            return []
        }
    }

    private func parseControlResponse(_ object: [String: Any]) -> [AgentEvent] {
        guard let response = object["response"] as? [String: Any],
              let requestID = response["request_id"] as? String
        else { return [] }
        if response["subtype"] as? String == "error" {
            return [.controlFailed(requestID: requestID, message: response["error"] as? String ?? "Unknown error")]
        }
        guard let payload = response["response"] as? [String: Any],
              let models = payload["models"] as? [[String: Any]]
        else { return [] }
        return [.catalog(ClaudeCatalog(
            models: models.compactMap(ClaudeModel.init(json:)),
            cliVersion: payload["claude_code_version"] as? String
        ))]
    }

    private func parseControlRequest(_ object: [String: Any]) -> [AgentEvent] {
        guard let requestID = object["request_id"] as? String,
              let request = object["request"] as? [String: Any],
              request["subtype"] as? String == "can_use_tool",
              let toolName = request["tool_name"] as? String
        else { return [] }
        return [.permissionRequest(PermissionRequest(
            requestID: requestID,
            toolName: toolName,
            description: request["description"] as? String,
            inputJSON: Self.jsonString(request["input"] ?? [String: Any]())
        ))]
    }

    // MARK: - Helpers

    static func contentBlock(_ block: [String: Any]) -> ContentBlock? {
        switch block["type"] as? String {
        case "text":
            return (block["text"] as? String).map(ContentBlock.text)
        case "thinking":
            return (block["thinking"] as? String).map(ContentBlock.thinking)
        case "tool_use":
            guard let id = block["id"] as? String, let name = block["name"] as? String else { return nil }
            return .toolUse(id: id, name: name, inputJSON: jsonString(block["input"] ?? [String: Any]()))
        default:
            return nil
        }
    }

    static func toolResult(_ block: [String: Any]) -> ToolResult? {
        guard block["type"] as? String == "tool_result",
              let id = block["tool_use_id"] as? String
        else { return nil }
        let text: String
        switch block["content"] {
        case let string as String:
            text = string
        case let parts as [[String: Any]]:
            text = parts.compactMap { $0["text"] as? String }.joined(separator: "\n")
        default:
            text = ""
        }
        return ToolResult(toolUseID: id, content: text, isError: block["is_error"] as? Bool ?? false)
    }

    private static func rateLimit(_ info: [String: Any]) -> RateLimitInfo {
        let windows = (info["unifiedWindows"] as? [String: [String: Any]] ?? [:])
            .map { name, window in
                RateLimitInfo.Window(
                    name: name,
                    utilization: window["utilization"] as? Double ?? 0,
                    resetsAt: (window["resetsAt"] as? Double).map(Date.init(timeIntervalSince1970:))
                )
            }
            .sorted { $0.name < $1.name }
        return RateLimitInfo(status: info["status"] as? String ?? "unknown", windows: windows)
    }

    static func jsonString(_ value: Any) -> String {
        guard JSONSerialization.isValidJSONObject(value),
              let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
        else { return "{}" }
        return String(decoding: data, as: UTF8.self)
    }
}
