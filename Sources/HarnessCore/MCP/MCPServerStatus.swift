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

/// An MCP server asking you something mid-call: a few fields to fill in (`form`), or a page
/// to visit (`url`), such as signing in again before something sensitive. Claude Code waits
/// for the answer, so the tool call does too.
public struct MCPElicitation: Sendable, Equatable, Identifiable {
    public enum Mode: Sendable, Equatable { case form, url }

    /// A field of a form, from its JSON Schema.
    public struct Field: Sendable, Equatable, Identifiable {
        public enum Kind: Sendable, Equatable {
            case text
            case number(integer: Bool)
            case boolean
            /// Values and what to show for each.
            case choice([(value: String, label: String)])

            public static func == (a: Kind, b: Kind) -> Bool {
                switch (a, b) {
                case (.text, .text), (.boolean, .boolean): true
                case (.number(let x), .number(let y)): x == y
                case (.choice(let x), .choice(let y)): x.map(\.value) == y.map(\.value) && x.map(\.label) == y.map(\.label)
                default: false
                }
            }
        }

        public var id: String { name }
        public var name: String
        public var title: String
        public var description: String?
        public var kind: Kind
        public var required: Bool
        public var defaultValue: JSONValue?
    }

    /// The control request's, which the answer goes back under.
    public var id: String
    public var server: String
    public var message: String
    public var mode: Mode
    public var url: URL?
    /// The server's own ID for a `url` one, which says when it's done.
    public var elicitationID: String?
    public var fields: [Field]
    public var title: String?

    init?(requestID: String, request: [String: Any]) {
        guard let server = request["mcp_server_name"] as? String else { return nil }
        id = requestID
        self.server = server
        message = request["message"] as? String ?? ""
        mode = request["mode"] as? String == "url" ? .url : .form
        url = (request["url"] as? String).flatMap(URL.init(string:))
        elicitationID = request["elicitation_id"] as? String
        title = request["title"] as? String
        fields = Self.fields(schema: JSONValue(request["requested_schema"]), order: request["property_order"] as? [String])
    }

    static func fields(schema: JSONValue, order: [String]?) -> [Field] {
        guard case .object(let properties)? = schema["properties"] else { return [] }
        let required = Set(schema["required"]?.array?.compactMap(\.string) ?? [])
        let names = (order ?? []).filter { properties[$0] != nil } + properties.keys.sorted().filter { !(order ?? []).contains($0) }
        return names.compactMap { name in
            guard let property = properties[name] else { return nil }
            let kind: Field.Kind
            if let values = property["enum"]?.array?.compactMap(\.string) {
                let labels = property["enumNames"]?.array?.compactMap(\.string) ?? []
                kind = .choice(values.enumerated().map { ($1, $0 < labels.count ? labels[$0] : $1) })
            } else if let options = property["oneOf"]?.array ?? property["anyOf"]?.array,
                      case let choices = options.compactMap({ option -> (String, String)? in
                          guard let value = option["const"]?.string else { return nil }
                          return (value, option["title"]?.string ?? value)
                      }), !choices.isEmpty {
                kind = .choice(choices.map { (value: $0.0, label: $0.1) })
            } else {
                switch property["type"]?.string {
                case "boolean": kind = .boolean
                case "integer": kind = .number(integer: true)
                case "number": kind = .number(integer: false)
                default: kind = .text
                }
            }
            return Field(name: name, title: property["title"]?.string ?? name, description: property["description"]?.string,
                         kind: kind, required: required.contains(name), defaultValue: property["default"])
        }
    }
}
