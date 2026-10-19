import Foundation

/// A Claude Code session read back from the CLI's own history
/// (`~/.claude/projects/<folder>/<session-id>.jsonl`), so a thread can pick it up and
/// continue with `--resume`. Read-only: the file is never written.
public struct ClaudeSessionArchive: Sendable, Equatable {
    public enum Item: Sendable, Equatable {
        case user(String)
        case assistant(String)
        case thinking(String)
        case tool(id: String, name: String, inputJSON: String, result: String?, isError: Bool)
        case notice(String)
    }

    public var sessionID: String
    /// The folder the session ran in; `--resume` only finds it from there.
    public var cwd: String?
    /// Your `/rename`, else Claude Code's own generated title.
    public var title: String?
    /// The title is one you gave the session with `/rename`.
    public var hasCustomTitle = false
    public var model: String?
    public var startedAt: Date?
    public var updatedAt: Date?
    public var items: [Item]

    /// The first thing you asked; names a session that has no title.
    public var firstMessage: String? {
        items.lazy.compactMap { if case .user(let text) = $0 { text } else { nil } }.first
    }

    public var messageCount: Int {
        items.count(where: { if case .user = $0 { true } else { false } })
    }

    // MARK: - Finding

    /// Pulls a session ID out of whatever was pasted: the bare ID, `claude --resume <id>`,
    /// or a path to the `.jsonl` file.
    public static func sessionID(in text: String) -> String? {
        let pattern = #"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"#
        guard let range = text.range(of: pattern, options: .regularExpression) else { return nil }
        return text[range].lowercased()
    }

    /// `$CLAUDE_CONFIG_DIR` when set, else `~/.claude`.
    public static func configDirectory(environment: [String: String]) -> URL {
        if let custom = environment["CLAUDE_CONFIG_DIR"], !custom.isEmpty {
            return URL(fileURLWithPath: (custom as NSString).expandingTildeInPath, isDirectory: true)
        }
        return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".claude", isDirectory: true)
    }

    /// The session's file, looked up by name in each project folder; nothing else is read.
    public static func locate(sessionID: String, configDirectory: URL) -> URL? {
        let projects = configDirectory.appendingPathComponent("projects", isDirectory: true)
        guard let folders = try? FileManager.default.contentsOfDirectory(
            at: projects, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles]) else { return nil }
        return folders.lazy
            .map { $0.appendingPathComponent("\(sessionID).jsonl") }
            .first { FileManager.default.fileExists(atPath: $0.path) }
    }

    // MARK: - Reading

    public enum LoadError: Error, Equatable {
        case unreadable
        case empty
    }

    public static func load(from url: URL, sessionID: String) throws(LoadError) -> ClaudeSessionArchive {
        guard let data = try? Data(contentsOf: url, options: .mappedIfSafe) else { throw .unreadable }
        let archive = parse(data, sessionID: sessionID)
        guard !archive.items.isEmpty else { throw .empty }
        return archive
    }

    private struct Record {
        var line: Int
        var object: [String: Any]
        var uuid: String
        var parent: String?
        var type: String
        var isCompactBoundary: Bool
        var isToolResultOnly: Bool
    }

    /// Rebuilds the conversation you'd see on `--resume`.
    ///
    /// The file is a tree: every message names its parent, rewinds leave dead branches,
    /// and a compaction starts a fresh root that points back to where it cut. Walking up
    /// from the newest message (jumping back across compactions) gives the live branch.
    /// Results of parallel tool calls hang off that branch as siblings, so they're kept too.
    static func parse(_ data: Data, sessionID: String) -> ClaudeSessionArchive {
        var archive = ClaudeSessionArchive(sessionID: sessionID, items: [])
        var records: [String: Record] = [:]
        var lastMessage: String?
        var customTitle: String?
        var aiTitle: String?

        var lineNumber = 0
        for line in data.split(separator: UInt8(ascii: "\n"), omittingEmptySubsequences: true) {
            lineNumber += 1
            guard let object = try? JSONSerialization.jsonObject(with: line) as? [String: Any],
                  let type = object["type"] as? String else { continue }
            switch type {
            case "custom-title":
                customTitle = object["customTitle"] as? String ?? customTitle
            case "ai-title":
                aiTitle = object["aiTitle"] as? String ?? aiTitle
            case "summary":
                aiTitle = aiTitle ?? object["summary"] as? String
            default:
                // Attachments and the like carry no conversation, but the chain runs through them.
                guard object["isSidechain"] as? Bool != true, let uuid = object["uuid"] as? String else { continue }
                let isBoundary = type == "system" && object["subtype"] as? String == "compact_boundary"
                let parent = object["parentUuid"] as? String ?? (isBoundary ? object["logicalParentUuid"] as? String : nil)
                var toolResultOnly = false
                if type == "user", let blocks = (object["message"] as? [String: Any])?["content"] as? [[String: Any]] {
                    toolResultOnly = !blocks.isEmpty && blocks.allSatisfy { $0["type"] as? String == "tool_result" }
                }
                records[uuid] = Record(line: lineNumber, object: object, uuid: uuid, parent: parent, type: type,
                                       isCompactBoundary: isBoundary, isToolResultOnly: toolResultOnly)
                if type == "user" || type == "assistant" { lastMessage = uuid }
            }
        }
        archive.title = (customTitle ?? aiTitle).flatMap { $0.isEmpty ? nil : $0 }
        archive.hasCustomTitle = customTitle?.isEmpty == false

        var live = Set<String>()
        var cursor = lastMessage
        while let uuid = cursor, !live.contains(uuid), let record = records[uuid] {
            live.insert(uuid)
            cursor = record.parent
        }
        let kept = records.values
            .filter { live.contains($0.uuid) || ($0.isToolResultOnly && $0.parent.map(live.contains) == true) }
            .sorted { $0.line < $1.line }

        var toolIndex: [String: Int] = [:]
        for record in kept {
            let object = record.object
            if let cwd = object["cwd"] as? String { archive.cwd = cwd }
            if let date = (object["timestamp"] as? String).flatMap(Self.date) {
                archive.startedAt = archive.startedAt ?? date
                archive.updatedAt = date
            }
            if record.isCompactBoundary {
                archive.items.append(.notice("Conversation compacted"))
                continue
            }
            guard record.type == "user" || record.type == "assistant",
                  let message = object["message"] as? [String: Any] else { continue }

            if record.type == "assistant" {
                if let model = message["model"] as? String, model != "<synthetic>" { archive.model = model }
                for block in message["content"] as? [[String: Any]] ?? [] {
                    switch ClaudeStreamParser.contentBlock(block) {
                    case .text(let text)?:
                        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
                        if !text.isEmpty { archive.items.append(.assistant(text)) }
                    case .thinking(let text)?:
                        // Usually redacted to a signature; only real text is worth showing.
                        if !text.isEmpty { archive.items.append(.thinking(text)) }
                    case .toolUse(let id, let name, let input)?:
                        toolIndex[id] = archive.items.count
                        archive.items.append(.tool(id: id, name: name, inputJSON: input, result: nil, isError: false))
                    case nil:
                        continue
                    }
                }
                continue
            }

            // User messages: what you typed, tool results, and CLI bookkeeping to skip.
            guard record.type == "user", object["isMeta"] as? Bool != true,
                  object["isCompactSummary"] as? Bool != true else { continue }
            var texts: [String] = []
            var hasImage = false
            switch message["content"] {
            case let string as String:
                texts.append(string)
            case let blocks as [[String: Any]]:
                for block in blocks {
                    switch block["type"] as? String {
                    case "text":
                        if let text = block["text"] as? String { texts.append(text) }
                    case "image":
                        hasImage = true
                    case "tool_result":
                        guard let result = ClaudeStreamParser.toolResult(block), let index = toolIndex[result.toolUseID],
                              case .tool(let id, let name, let input, _, _) = archive.items[index] else { continue }
                        archive.items[index] = .tool(id: id, name: name, inputJSON: input, result: result.content, isError: result.isError)
                    default:
                        continue
                    }
                }
            default:
                continue
            }
            let text = texts.map(Self.visibleText).filter { !$0.isEmpty }.joined(separator: "\n\n")
            if text.hasPrefix("[Request interrupted by user") {
                archive.items.append(.notice("Interrupted"))
            } else if let command = Self.slashCommand(in: text) {
                archive.items.append(.notice(command))
            } else if !text.isEmpty || hasImage {
                archive.items.append(.user(text.isEmpty ? "[Image]" : text))
            }
        }
        return archive
    }

    /// Strips what the CLI wraps around a prompt (`<system-reminder>`, local command output).
    private static func visibleText(_ text: String) -> String {
        var text = text
        for tag in ["system-reminder", "local-command-stdout", "local-command-stderr", "local-command-caveat", "bash-stdout", "bash-stderr"] {
            text = text.replacingOccurrences(of: "<\(tag)>[\\s\\S]*?</\(tag)>", with: "", options: .regularExpression)
        }
        if let range = text.range(of: "<bash-input>([\\s\\S]*?)</bash-input>", options: .regularExpression) {
            let command = text[range].dropFirst("<bash-input>".count).dropLast("</bash-input>".count)
            text.replaceSubrange(range, with: "! " + command)
        }
        return text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// "/compact" for a slash command you ran in the terminal.
    private static func slashCommand(in text: String) -> String? {
        guard let name = tagContents("command-name", in: text) else { return nil }
        let arguments = tagContents("command-args", in: text) ?? ""
        return arguments.isEmpty ? name : "\(name) \(arguments)"
    }

    private static func tagContents(_ tag: String, in text: String) -> String? {
        guard let start = text.range(of: "<\(tag)>"), let end = text.range(of: "</\(tag)>", range: start.upperBound..<text.endIndex) else { return nil }
        return text[start.upperBound..<end.lowerBound].trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static func date(_ string: String) -> Date? {
        (try? Date.ISO8601FormatStyle(includingFractionalSeconds: true).parse(string))
            ?? (try? Date.ISO8601FormatStyle().parse(string))
    }
}
