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

public struct ClaudeCatalog: Sendable, Equatable {
    public var models: [ClaudeModel]
    public var cliVersion: String?

    /// Asks a throwaway CLI process for its catalog. No message is sent, so this makes
    /// no API request and uses none of your subscription.
    public static func load(executable: URL, environment: [String: String], timeout: Duration = .seconds(20)) async -> ClaudeCatalog? {
        let options = ClaudeLaunchOptions(
            executable: executable,
            workingDirectory: FileManager.default.homeDirectoryForCurrentUser,
            environment: environment
        )
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
