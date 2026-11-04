import Foundation

/// An MCP server as Claude Code reports it: in `mcp_status` replies (everything) and in a
/// session's `init` (name, state and where it's from). See docs/mcp.md.
public struct MCPServerStatus: Sendable, Equatable, Identifiable {
    public enum State: Sendable, Equatable {
        case connected
        case needsSignIn
        case failed
        case disabled
        case connecting
        /// A state this app doesn't know yet, as the CLI spells it.
        case other(String)

        init(_ raw: String) {
            switch raw {
            case "connected": self = .connected
            case "needs-auth": self = .needsSignIn
            case "failed": self = .failed
            case "disabled": self = .disabled
            case "pending", "connecting": self = .connecting
            default: self = .other(raw)
            }
        }

        /// It's waiting on you: a sign-in, or a fix before it starts.
        public var needsAttention: Bool { self == .needsSignIn || self == .failed }
    }

    public struct Tool: Sendable, Equatable, Hashable {
        public var name: String
        /// The server says it only reads.
        public var readOnly: Bool
        /// The server says it can't be undone.
        public var destructive: Bool
    }

    public var id: String { name }
    /// As Claude Code knows it: what the `mcp_*` requests take.
    public var name: String
    public var state: State
    /// `project` (.mcp.json), `local`, `user`, `claudeai`, `dynamic` (--mcp-config, or added
    /// for the session), as the CLI spells it.
    public var scope: String?
    /// `stdio`, `http`, `sse`, `ws`, `claudeai-proxy`.
    public var transport: String?
    /// Why it failed.
    public var error: String?
    public var serverName: String?
    public var serverVersion: String?
    /// The command and its arguments, or the URL. Never the environment or headers, which
    /// can hold secrets.
    public var target: String?
    public var tools: [Tool]

    public init(name: String, state: State, scope: String? = nil, transport: String? = nil, error: String? = nil,
                serverName: String? = nil, serverVersion: String? = nil, target: String? = nil, tools: [Tool] = []) {
        self.name = name
        self.state = state
        self.scope = scope
        self.transport = transport
        self.error = error
        self.serverName = serverName
        self.serverVersion = serverVersion
        self.target = target
        self.tools = tools
    }

    init?(json: JSONValue) {
        guard let name = json["name"]?.string, !name.isEmpty else { return nil }
        let config = json["config"]
        self.init(
            name: name,
            state: State(json["status"]?.string ?? ""),
            scope: json["scope"]?.string ?? json["source"]?.string,
            transport: json["transport"]?.string ?? config?["type"]?.string,
            error: json["error"]?.string,
            serverName: json["serverInfo"]?["name"]?.string,
            serverVersion: json["serverInfo"]?["version"]?.string,
            target: config.flatMap(Self.target),
            tools: (json["tools"]?.array ?? []).compactMap { tool in
                guard let name = tool["name"]?.string else { return nil }
                let annotations = tool["annotations"]
                return Tool(name: name, readOnly: annotations?["readOnly"]?.bool ?? false,
                            destructive: annotations?["destructive"]?.bool ?? false)
            }
        )
    }

    private static func target(_ config: JSONValue) -> String? {
        if let url = config["url"]?.string { return url }
        guard let command = config["command"]?.string else { return nil }
        let arguments = (config["args"]?.array ?? []).compactMap(\.string)
        return ([command] + arguments).joined(separator: " ")
    }

    /// The servers in an `mcp_status` reply.
    public static func list(from reply: JSONValue) -> [MCPServerStatus] {
        (reply["mcpServers"]?.array ?? []).compactMap(MCPServerStatus.init(json:))
    }
}

/// What `mcp_authenticate` hands back: a page to open in the browser. Claude Code listens
/// for the browser coming back and connects the server itself.
public struct MCPSignIn: Sendable, Equatable {
    /// Nil when it's already signed in, or signed in without the browser.
    public var url: URL?
    /// Claude Code waits for the browser on localhost; false means paste the address back.
    public var callbackExpected: Bool

    init(json: JSONValue) {
        url = json["authUrl"]?.string.flatMap(URL.init(string:))
        callbackExpected = json["callbackExpected"]?.bool ?? true
    }
}

/// A control request Claude Code answered with an error, or didn't answer.
public enum ClaudeControlError: Error, Equatable, CustomStringConvertible {
    case failed(String)
    case timedOut
    /// The process went away before answering.
    case ended

    public var description: String {
        switch self {
        case .failed(let message): message
        case .timedOut: "Claude Code didn't answer in time."
        case .ended: "The Claude Code session ended."
        }
    }
}
