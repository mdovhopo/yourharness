import Foundation

/// A model option as the CLI reports it in its `initialize` response.
public struct ClaudeModel: Sendable, Equatable, Codable, Identifiable {
    public var id: String { value }
    /// What to pass to `--model` / `set_model`: an alias (`opus`) or a full ID.
    public var value: String
    public var resolvedModel: String?
    public var displayName: String
    public var description: String?
    public var supportedEffortLevels: [String]
    /// Whether `--permission-mode auto` works with this model. The CLI quietly falls back
    /// to asking when it doesn't. Nil in catalogs cached before the CLI reported it.
    public var supportsAutoMode: Bool?

    /// Aliases that track the latest model, as opposed to pinned versions.
    public var isAlias: Bool { !value.hasPrefix("claude-") }

    init?(json: [String: Any]) {
        guard let value = json["value"] as? String, let displayName = json["displayName"] as? String else { return nil }
        self.value = value
        self.resolvedModel = json["resolvedModel"] as? String
        self.displayName = displayName
        self.description = json["description"] as? String
        self.supportedEffortLevels = json["supportedEffortLevels"] as? [String] ?? []
        // The CLI omits the key for models without auto mode.
        self.supportsAutoMode = json["supportsAutoMode"] as? Bool ?? false
    }
}

/// A slash command or skill, as the CLI lists it in its `initialize` response.
public struct SlashCommand: Sendable, Equatable, Codable, Identifiable {
    public var id: String { name }
    public var name: String
    /// Ends with where it comes from for custom ones: "(project)", "(user)".
    public var description: String
    /// e.g. "<name>", "[optional instructions]".
    public var argumentHint: String
    public var aliases: [String]
    /// Part of Claude Code itself, as opposed to your commands and skills.
    public var isBuiltin: Bool
    /// For an MCP server's prompt: the server, as Claude Code names it.
    public var mcpServer: String?

    public init(name: String, description: String, argumentHint: String = "", aliases: [String] = [], isBuiltin: Bool = false,
                mcpServer: String? = nil) {
        self.name = name
        self.description = description
        self.argumentHint = argumentHint
        self.aliases = aliases
        self.isBuiltin = isBuiltin
        self.mcpServer = mcpServer
    }

    init?(json: [String: Any]) {
        guard var name = json["name"] as? String, !name.isEmpty else { return nil }
        // An MCP prompt is listed as "server:prompt (MCP)" but typed as /mcp__server__prompt.
        let prompt = Self.mcpPrompt(listedAs: name)
        if let prompt { name = "mcp__\(Self.normalized(prompt.server))__\(Self.normalized(prompt.prompt))" }
        self.init(name: name, description: json["description"] as? String ?? "",
                  argumentHint: json["argumentHint"] as? String ?? "", aliases: json["aliases"] as? [String] ?? [],
                  isBuiltin: json["builtin"] as? Bool ?? false, mcpServer: prompt?.server)
    }

    /// "github:review (MCP)" → ("github", "review").
    static func mcpPrompt(listedAs name: String) -> (server: String, prompt: String)? {
        guard name.hasSuffix(" (MCP)") else { return nil }
        let bare = name.dropLast(" (MCP)".count)
        guard let colon = bare.lastIndex(of: ":"), colon > bare.startIndex, bare.index(after: colon) < bare.endIndex else { return nil }
        return (String(bare[..<colon]), String(bare[bare.index(after: colon)...]))
    }

    /// How Claude Code puts a name into a tool's or command's: anything but letters, digits,
    /// `_` and `-` becomes `_` ("claude.ai Linear" → "claude_ai_Linear").
    static func normalized(_ name: String) -> String {
        String(name.map { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "_" || $0 == "-") ? $0 : "_" })
    }

    /// What the menu shows: "server:prompt" for an MCP prompt, as Claude Code lists it; the
    /// name otherwise.
    public var displayName: String {
        guard let mcpServer else { return name }
        let prefix = "mcp__\(Self.normalized(mcpServer))__"
        return name.hasPrefix(prefix) ? "\(mcpServer):\(name.dropFirst(prefix.count))" : name
    }

    /// "/name the rest" → ("name", "the rest"); nil when the text isn't a command.
    public static func parse(_ text: String) -> (name: String, arguments: String)? {
        guard text.hasPrefix("/"), text.count > 1 else { return nil }
        let body = text.dropFirst()
        let name = body.prefix { !$0.isWhitespace }
        guard !name.isEmpty, !name.contains("/") else { return nil }
        return (String(name), body.dropFirst(name.count).trimmingCharacters(in: .whitespacesAndNewlines))
    }

    /// Only mean something in the terminal UI.
    public static let terminalOnly: Set<String> = ["color", "focus", "heapdump"]

    /// The `/name` being typed at the end of the text: at its start or after a space, with
    /// nothing after it yet. `start` is where its slash is.
    public static func trailingCommand(in text: String) -> (start: String.Index, query: String)? {
        guard let slash = text.lastIndex(of: "/") else { return nil }
        if slash > text.startIndex, !text[text.index(before: slash)].isWhitespace { return nil }
        let query = text[text.index(after: slash)...]
        guard !query.contains(where: { $0.isWhitespace || $0 == "/" }) else { return nil }
        return (slash, String(query))
    }

    /// `/name` words after the start of a message that name one of `known`, in order, once
    /// each: skills to run along the way. The command a message starts with is Claude Code's
    /// to run, so it isn't one of them.
    public static func chainedCommands(in text: String, known: Set<String>) -> [String] {
        var found: [String] = []
        var index = text.startIndex
        while let slash = text[index...].firstIndex(of: "/") {
            index = text.index(after: slash)
            // A word of its own, not the start of the message, not part of a path or `code`.
            guard slash > text.startIndex, text[text.index(before: slash)].isWhitespace else { continue }
            let name = text[index...].prefix { !$0.isWhitespace && !",.;!?)\"'`".contains($0) }
            guard !name.isEmpty, !name.contains("/") else { continue }
            let after = text.index(index, offsetBy: name.count)
            if after < text.endIndex, text[after] == "/" { continue }
            let key = String(name)
            if known.contains(key), !found.contains(key) { found.append(key) }
        }
        return found
    }

    /// What's added to a message that chains skills, so the agent runs each where it comes up.
    public static func chainNote(for names: [String]) -> String? {
        guard !names.isEmpty else { return nil }
        let list = names.map { "/\($0)" }
        let joined = list.count == 1 ? list[0] : list.dropLast().joined(separator: ", ") + " and " + list.last!
        return names.count == 1
            ? "(Run the \(joined) skill where it comes up above, with the Skill tool.)"
            : "(Run the \(joined) skills where they come up above, in that order, with the Skill tool.)"
    }
}

public struct ClaudeCatalog: Sendable, Equatable {
    public var models: [ClaudeModel]
    public var cliVersion: String?
    /// Built-ins, plus the commands and skills of the folder the CLI runs in.
    public var commands: [SlashCommand] = []

    /// Asks a throwaway CLI process for its catalog. No message is sent, so this makes
    /// no API request and uses none of your subscription.
    /// Commands depend on `workingDirectory`: a project's own commands and skills.
    public static func load(executable: URL, environment: [String: String], workingDirectory: URL? = nil,
                            timeout: Duration = .seconds(20)) async -> ClaudeCatalog? {
        var options = ClaudeLaunchOptions(
            executable: executable,
            workingDirectory: workingDirectory ?? FileManager.default.homeDirectoryForCurrentUser,
            environment: environment
        )
        options.persistsSession = false
        guard let session = try? ClaudeSession(options: options) else { return nil }
        try? await session.initialize()

        let catalog = await withTaskGroup(of: ClaudeCatalog?.self) { group in
            group.addTask {
                for await event in session.events {
                    if case .catalog(let catalog) = event { return catalog }
                }
                return nil
            }
            group.addTask {
                try? await Task.sleep(for: timeout)
                return nil
            }
            let first = await group.next() ?? nil
            group.cancelAll()
            return first
        }
        await session.terminate()
        return catalog
    }
}
