import Foundation

/// Tools the app serves to an agent itself, as an MCP server inside the app. Claude Code
/// reaches them over the stream-json control channel it already has with us (the SDK's
/// in-process servers), so there's no port, no token and nothing else can call them.
public protocol MCPToolServer: Sendable {
    /// What the agent sees: tools are `mcp__<name>__<tool>`.
    var name: String { get }
    var tools: [MCPTool] { get }
    /// `toolUseID` ties the call to its row in the transcript, when the CLI says.
    func call(_ tool: String, arguments: JSONValue, toolUseID: String?) async -> MCPToolResult
}

public struct MCPTool: Sendable, Equatable {
    public var name: String
    public var description: String
    /// A JSON Schema object.
    public var inputSchema: JSONValue
    /// Doesn't change anything: shown to the client as `readOnlyHint`.
    public var readOnly: Bool

    public init(name: String, description: String, inputSchema: JSONValue, readOnly: Bool = false) {
        self.name = name
        self.description = description
        self.inputSchema = inputSchema
        self.readOnly = readOnly
    }

    /// An object schema from its properties; `required` lists the ones that must be there.
    public static func schema(_ properties: [String: JSONValue], required: [String] = []) -> JSONValue {
        var schema: [String: JSONValue] = ["type": "object", "properties": .object(properties), "additionalProperties": false]
        if !required.isEmpty { schema["required"] = .array(required.map(JSONValue.string)) }
        return .object(schema)
    }
}

public struct MCPToolResult: Sendable, Equatable {
    public enum Content: Sendable, Equatable {
        case text(String)
        case image(Data, mimeType: String)
    }

    public var content: [Content]
    public var isError: Bool

    public init(_ content: [Content], isError: Bool = false) {
        self.content = content
        self.isError = isError
    }

    public static func text(_ text: String) -> MCPToolResult { MCPToolResult([.text(text)]) }
    public static func error(_ message: String) -> MCPToolResult { MCPToolResult([.text(message)], isError: true) }
}

/// The JSON-RPC side of one server: the handshake, the tool list and calls.
public struct MCPServerEndpoint: Sendable {
    public let server: any MCPToolServer

    public init(_ server: any MCPToolServer) {
        self.server = server
    }

    /// The reply to one message; nil for a notification, which gets none.
    public func handle(_ message: JSONValue) async -> JSONValue? {
        let id = message["id"]
        let method = message["method"]?.string ?? ""
        guard let id, id != .null else { return nil }
        switch method {
        case "initialize":
            return Self.reply(id, [
                "protocolVersion": message["params"]?["protocolVersion"] ?? "2025-06-18",
                "capabilities": ["tools": [:]],
                "serverInfo": ["name": .string(server.name), "version": "1.0"],
            ])
        case "ping":
            return Self.reply(id, [:])
        case "tools/list":
            return Self.reply(id, ["tools": .array(server.tools.map(Self.definition))])
        case "tools/call":
            let params = message["params"]
            guard let name = params?["name"]?.string else { return Self.failure(id, code: -32602, "Missing tool name") }
            guard server.tools.contains(where: { $0.name == name }) else { return Self.failure(id, code: -32602, "Unknown tool \(name)") }
            let toolUseID = params?["_meta"]?["claudecode/toolUseId"]?.string
            let result = await server.call(name, arguments: params?["arguments"] ?? [:], toolUseID: toolUseID)
            return Self.reply(id, Self.encode(result))
        default:
            return Self.failure(id, code: -32601, "Method not found: \(method)")
        }
    }

    static func definition(_ tool: MCPTool) -> JSONValue {
        [
            "name": .string(tool.name),
            "description": .string(tool.description),
            "inputSchema": tool.inputSchema,
            "annotations": ["readOnlyHint": .bool(tool.readOnly)],
        ]
    }

    static func encode(_ result: MCPToolResult) -> JSONValue {
        let content: [JSONValue] = result.content.map { part in
            switch part {
            case .text(let text): ["type": "text", "text": .string(text)]
            case .image(let data, let mimeType): ["type": "image", "data": .string(data.base64EncodedString()), "mimeType": .string(mimeType)]
            }
        }
        return ["content": .array(content), "isError": .bool(result.isError)]
    }

    private static func reply(_ id: JSONValue, _ result: JSONValue) -> JSONValue {
        ["jsonrpc": "2.0", "id": id, "result": result]
    }

    private static func failure(_ id: JSONValue, code: Int, _ message: String) -> JSONValue {
        ["jsonrpc": "2.0", "id": id, "error": ["code": .number(Double(code)), "message": .string(message)]]
    }
}
