import Foundation

/// A coding-agent CLI the harness knows how to find. Only `isSupported` ones can run
/// threads today; the rest are detected so they're ready when their adapters land.
public struct AgentCLI: Sendable, Hashable, Identifiable {
    public let id: String
    public let name: String
    public let executableNames: [String]
    public let isSupported: Bool
    public let installURL: URL

    public static let claude = AgentCLI(
        id: "claude", name: "Claude Code", executableNames: ["claude"], isSupported: true,
        installURL: URL(string: "https://docs.claude.com/en/docs/claude-code/setup")!
    )
    public static let codex = AgentCLI(
        id: "codex", name: "Codex", executableNames: ["codex"], isSupported: false,
        installURL: URL(string: "https://github.com/openai/codex")!
    )
    public static let cursor = AgentCLI(
        id: "cursor", name: "Cursor Agent", executableNames: ["cursor-agent"], isSupported: false,
        installURL: URL(string: "https://cursor.com/cli")!
    )
    public static let gemini = AgentCLI(
        id: "gemini", name: "Gemini CLI", executableNames: ["gemini"], isSupported: false,
        installURL: URL(string: "https://github.com/google-gemini/gemini-cli")!
    )
    public static let opencode = AgentCLI(
        id: "opencode", name: "OpenCode", executableNames: ["opencode"], isSupported: false,
        installURL: URL(string: "https://opencode.ai")!
    )

    public static let all: [AgentCLI] = [.claude, .codex, .cursor, .gemini, .opencode]
}

public struct CLIStatus: Sendable, Equatable {
    public enum State: Sendable, Equatable {
        case notFound
        /// Found, but `--version` failed. Usually a broken install or wrong architecture.
        case broken(reason: String)
        case ready(version: String)
    }

    public var path: URL?
    public var state: State

    public var isReady: Bool {
        if case .ready = state { return true }
        return false
    }
}

public enum CLIDiscovery {
    /// Locates `cli` (or uses `override`) and checks that it actually runs.
    public static func probe(_ cli: AgentCLI, environment: [String: String], override: URL? = nil) async -> CLIStatus {
        let path = override ?? cli.executableNames.lazy
            .compactMap { ShellEnvironment.which($0, in: environment) }
            .first
        guard let path else { return CLIStatus(path: nil, state: .notFound) }

        let output = await CommandRunner.run(path, arguments: ["--version"], environment: environment)
        if output.succeeded, let version = firstLine(output.stdout) {
            return CLIStatus(path: path, state: .ready(version: version))
        }
        let reason = output.timedOut
            ? "Didn't respond to --version within 8 seconds."
            : firstLine(output.stderr) ?? firstLine(output.stdout) ?? "Exited with code \(output.status)."
        return CLIStatus(path: path, state: .broken(reason: reason))
    }

    static func firstLine(_ text: String) -> String? {
        text.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .first { !$0.isEmpty }
    }
}

/// Output of `claude auth status --json`.
public struct ClaudeAccount: Sendable, Equatable, Decodable {
    public var loggedIn: Bool
    public var authMethod: String?
    public var apiProvider: String?
    public var email: String?
    public var orgName: String?
    public var subscriptionType: String?

    public static func load(executable: URL, environment: [String: String]) async -> ClaudeAccount? {
        let output = await CommandRunner.run(executable, arguments: ["auth", "status", "--json"], environment: environment)
        return try? JSONDecoder().decode(ClaudeAccount.self, from: Data(output.stdout.utf8))
    }

    public var planName: String? {
        switch subscriptionType?.lowercased() {
        case nil, "": nil
        case "max": "Claude Max"
        case "pro": "Claude Pro"
        case "team": "Claude Team"
        case "enterprise": "Claude Enterprise"
        case let other?: other.capitalized
        }
    }

    public var signInMethod: String? {
        switch authMethod {
        case nil: nil
        case "claude.ai": "Claude subscription"
        case "api_key", "apiKey": "API key"
        case let other?: other
        }
    }
}
