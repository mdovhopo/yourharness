import Foundation
import Testing
@testable import HarnessCore

private struct Reverser: MCPToolServer {
    let name = "yh"
    var tools: [MCPTool] {
        [MCPTool(name: "reverse", description: "Reverses text.", inputSchema: MCPTool.schema(["text": ["type": "string"]], required: ["text"]), readOnly: true)]
    }

    func call(_ tool: String, arguments: JSONValue, toolUseID: String?) async -> MCPToolResult {
        guard let text = arguments["text"]?.string else { return .error("No text") }
        return .text(String(text.reversed()) + (toolUseID.map { " (\($0))" } ?? ""))
    }
}

struct MCPToolServerTests {
    private let endpoint = MCPServerEndpoint(Reverser())

    @Test func handshakeAndToolList() async throws {
        let initialize = try #require(await endpoint.handle(["jsonrpc": "2.0", "id": 0, "method": "initialize", "params": ["protocolVersion": "2025-06-18"]]))
        #expect(initialize["result"]?["protocolVersion"] == "2025-06-18")
        #expect(initialize["result"]?["serverInfo"]?["name"] == "yh")
        #expect(await endpoint.handle(["jsonrpc": "2.0", "method": "notifications/initialized"]) == nil)
        let list = try #require(await endpoint.handle(["jsonrpc": "2.0", "id": 1, "method": "tools/list"]))
        let tool = try #require(list["result"]?["tools"]?.array?.first)
        #expect(tool["name"] == "reverse")
        #expect(tool["inputSchema"]?["required"] == ["text"])
        #expect(tool["annotations"]?["readOnlyHint"] == true)
    }

    @Test func callsCarryTheirToolUse() async throws {
        let reply = try #require(await endpoint.handle([
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": ["name": "reverse", "arguments": ["text": "abc"], "_meta": ["claudecode/toolUseId": "toolu_1"]],
        ]))
        #expect(reply["id"] == 2)
        #expect(reply["result"]?["content"]?.array?.first?["text"] == "cba (toolu_1)")
        #expect(reply["result"]?["isError"] == false)
        let unknown = try #require(await endpoint.handle(["jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": ["name": "nope"]]))
        #expect(unknown["error"]?["code"] == -32602)
    }

    @Test func imagesAreBase64() {
        let encoded = MCPServerEndpoint.encode(MCPToolResult([.image(Data([1, 2, 3]), mimeType: "image/png")]))
        #expect(encoded["content"]?.array?.first?["data"] == "AQID")
        #expect(encoded["content"]?.array?.first?["mimeType"] == "image/png")
    }

    @Test func parserRoutesMCPMessages() {
        var parser = ClaudeStreamParser()
        let line = #"{"type":"control_request","request_id":"r1","request":{"subtype":"mcp_message","server_name":"yh","message":{"jsonrpc":"2.0","id":4,"method":"tools/list"}}}"#
        #expect(parser.parse(line: Data(line.utf8)) == [.mcpMessage(requestID: "r1", server: "yh", message: ["jsonrpc": "2.0", "id": 4, "method": "tools/list"])])
    }

    @Test func jsonValueKeepsBooleansApart() {
        let value = JSONValue(json: #"{"a":true,"b":1,"c":[1.5,null,"x"]}"#)
        #expect(value == ["a": true, "b": 1, "c": [1.5, nil, "x"]])
        #expect(JSONValue(json: value?.json ?? "") == value)
    }

    @Test func allowedToolsGoOnTheCommandLine() {
        var options = ClaudeLaunchOptions(executable: URL(fileURLWithPath: "/bin/claude"), workingDirectory: URL(fileURLWithPath: "/"), environment: [:])
        options.allowedTools = ["mcp__browser"]
        let args = options.arguments
        #expect(args.firstIndex(of: "--allowedTools").map { args[$0 + 1] } == "mcp__browser")
    }
}
