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

    public init(name: String, description: String, argumentHint: String = "", aliases: [String] = [], isBuiltin: Bool = false) {
        self.name = name
        self.description = description
        self.argumentHint = argumentHint
        self.aliases = aliases
        self.isBuiltin = isBuiltin
    }

    init?(json: [String: Any]) {
        guard let name = json["name"] as? String, !name.isEmpty else { return nil }
        self.init(name: name, description: json["description"] as? String ?? "",
                  argumentHint: json["argumentHint"] as? String ?? "", aliases: json["aliases"] as? [String] ?? [],
                  isBuiltin: json["builtin"] as? Bool ?? false)
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
