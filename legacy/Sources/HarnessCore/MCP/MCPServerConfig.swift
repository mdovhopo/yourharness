import Foundation

/// A server to add, as Claude Code's `claude mcp add-json` takes it. See docs/mcp.md.
public enum MCPServerConfig: Sendable, Equatable {
    /// A server on the web. Signing in, if it needs it, happens after.
    case url(URL, transport: String = "http", headers: [String: String] = [:], clientID: String? = nil, callbackPort: Int? = nil)
    /// A program on this Mac.
    case command(String, arguments: [String] = [], environment: [String: String] = [:])

    public var json: JSONValue {
        switch self {
        case .url(let url, let transport, let headers, let clientID, let callbackPort):
            var config: [String: JSONValue] = ["type": .string(transport), "url": .string(url.absoluteString)]
            if !headers.isEmpty { config["headers"] = .object(headers.mapValues(JSONValue.string)) }
            var oauth: [String: JSONValue] = [:]
            if let clientID, !clientID.isEmpty { oauth["clientId"] = .string(clientID) }
            if let callbackPort { oauth["callbackPort"] = .number(Double(callbackPort)) }
            if !oauth.isEmpty { config["oauth"] = .object(oauth) }
            return .object(config)
        case .command(let command, let arguments, let environment):
            var config: [String: JSONValue] = ["type": "stdio", "command": .string(command), "args": .array(arguments.map(JSONValue.string))]
            if !environment.isEmpty { config["env"] = .object(environment.mapValues(JSONValue.string)) }
            return .object(config)
        }
    }

    /// A name from a URL's host: "mcp.linear.app" → "linear".
    public static func suggestedName(for url: URL) -> String {
        let parts = (url.host() ?? "").split(separator: ".").map(String.init)
        let generic: Set = ["mcp", "api", "www", "app", "server", "com", "io", "dev", "ai", "net", "org", "co"]
        return parts.first { !generic.contains($0.lowercased()) } ?? parts.first ?? "server"
    }

    /// A name from a command: `npx -y @modelcontextprotocol/server-github` → "github".
    public static func suggestedName(forCommand line: String) -> String {
        let words = splitCommandLine(line).filter { !$0.hasPrefix("-") }
        let package = words.dropFirst(["npx", "uvx", "bunx", "pnpx", "docker", "node", "python", "python3", "deno"].contains(words.first ?? "") ? 1 : 0).first ?? ""
        var name = String(package.split(separator: "/").last ?? "")
        if let at = name.firstIndex(of: "@"), at != name.startIndex { name = String(name[..<at]) }
        for prefix in ["mcp-server-", "server-", "mcp-"] where name.hasPrefix(prefix) { name.removeFirst(prefix.count) }
        for suffix in ["-mcp-server", "-mcp", ".py", ".js"] where name.hasSuffix(suffix) { name.removeLast(suffix.count) }
        return name.isEmpty ? "server" : name
    }

    /// Splits a command line as a shell would for simple cases: spaces, and quotes around a
    /// word with spaces in it.
    public static func splitCommandLine(_ line: String) -> [String] {
        var words: [String] = []
        var current = ""
        var quote: Character?
        var inWord = false
        for character in line {
            if let open = quote {
                if character == open { quote = nil } else { current.append(character) }
            } else if character == "\"" || character == "'" {
                quote = character
                inWord = true
            } else if character.isWhitespace {
                if inWord { words.append(current); current = ""; inWord = false }
            } else {
                current.append(character)
                inWord = true
            }
        }
        if inWord { words.append(current) }
        return words
    }

    /// Servers from a pasted `{"mcpServers": {…}}`, a single `{"name": {…}}`, or one server's
    /// own config (named `fallbackName`).
    public static func servers(fromJSON text: String, fallbackName: String) -> [String: JSONValue]? {
        guard let value = JSONValue(json: text), case .object(let object) = value else { return nil }
        if case .object(let servers)? = object["mcpServers"] { return servers }
        if object["command"] != nil || object["url"] != nil { return [fallbackName: value] }
        let named = object.filter { if case .object(let config) = $0.value { config["command"] != nil || config["url"] != nil } else { false } }
        return named.isEmpty ? nil : named
    }
}

/// Where Claude Code keeps a server it adds: `claude mcp add --scope`.
public enum MCPScope: String, Sendable, CaseIterable {
    /// All your projects (`~/.claude.json`).
    case user
    /// This project, just you (`~/.claude.json`, under the folder).
    case local
    /// This project, for everyone: its `.mcp.json`, to commit.
    case project
}

/// Adds and removes servers through Claude Code's own commands, so the terminal sees them
/// too, in the folder they're for.
public enum ClaudeMCPConfig {
    public static func add(name: String, config: JSONValue, scope: MCPScope, clientSecret: String? = nil,
                           executable: URL, environment: [String: String], folder: URL) async -> Result<Void, MCPConfigError> {
        var arguments = ["mcp", "add-json", "--scope", scope.rawValue, name, config.json]
        var environment = environment
        if let clientSecret, !clientSecret.isEmpty {
            arguments.insert("--client-secret", at: 2)
            environment["MCP_CLIENT_SECRET"] = clientSecret
        }
        return await run(arguments, executable: executable, environment: environment, folder: folder)
    }

    public static func remove(name: String, scope: MCPScope?, executable: URL, environment: [String: String], folder: URL) async -> Result<Void, MCPConfigError> {
        var arguments = ["mcp", "remove", name]
        if let scope { arguments += ["--scope", scope.rawValue] }
        return await run(arguments, executable: executable, environment: environment, folder: folder)
    }

    private static func run(_ arguments: [String], executable: URL, environment: [String: String], folder: URL) async -> Result<Void, MCPConfigError> {
        let output = await CommandRunner.run(executable, arguments: arguments, environment: environment, timeout: 20, currentDirectory: folder)
        if output.succeeded { return .success(()) }
        if output.timedOut { return .failure(MCPConfigError(message: "Claude Code didn't answer in time.")) }
        let message = [output.stderr, output.stdout].map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.first { !$0.isEmpty }
        return .failure(MCPConfigError(message: message ?? "Claude Code exited with code \(output.status)."))
    }

    /// The servers Claude Desktop has, from its config file, if it's installed.
    public static func claudeDesktopServers(home: URL = FileManager.default.homeDirectoryForCurrentUser) -> [String: JSONValue] {
        let file = home.appendingPathComponent("Library/Application Support/Claude/claude_desktop_config.json")
        guard let data = try? Data(contentsOf: file), let text = String(data: data, encoding: .utf8),
              let value = JSONValue(json: text), case .object(let servers)? = value["mcpServers"] else { return [:] }
        return servers
    }
}

public struct MCPConfigError: Error, Equatable, CustomStringConvertible {
    public var message: String
    public var description: String { message }
}
