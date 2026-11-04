import Foundation
import Testing
@testable import HarnessCore

/// The fixture is a real capture (sanitized, on a scratch config): `initialize`, `mcp_status`,
/// a server turned off and on, a reconnect that fails, a sign-in started, a toggle of a server
/// that doesn't exist, then `/mcp` for the session's `init`. Servers: two copies of an echo
/// server (one from `.mcp.json`, one from `--mcp-config`) with a tool and a prompt, a missing
/// binary, and an HTTP server that needs signing in.
struct MCPServerParsingTests {
    private func events() throws -> [AgentEvent] {
        let url = try #require(Bundle.module.url(forResource: "mcp-servers", withExtension: "jsonl", subdirectory: "Fixtures"))
        var parser = ClaudeStreamParser()
        return try String(contentsOf: url, encoding: .utf8)
            .split(separator: "\n")
            .flatMap { parser.parse(line: Data($0.utf8)) }
    }

    private func reply(_ id: String, in events: [AgentEvent]) -> JSONValue? {
        events.lazy.compactMap { event -> JSONValue? in
            if case .controlReply(id, let response) = event { response } else { nil }
        }.first
    }

    @Test func statusListsEveryServerWithItsState() throws {
        let servers = MCPServerStatus.list(from: try #require(reply("yh-2", in: try events())))
        let byName = Dictionary(uniqueKeysWithValues: servers.map { ($0.name, $0) })

        let echo = try #require(byName["project-echo"])
        #expect(echo.state == .connected)
        #expect(echo.scope == "project")
        #expect(echo.transport == "stdio")
        #expect(echo.serverName == "echo" && echo.serverVersion == "1.0")
        #expect(echo.target == "python3 -I /path/to/echo_server.py")
        #expect(echo.tools.map(\.name) == ["echo"])

        let broken = try #require(byName["broken"])
        #expect(broken.state == .failed)
        #expect(broken.error?.contains("ENOENT") == true)
        #expect(broken.state.needsAttention)

        let remote = try #require(byName["remote-auth"])
        #expect(remote.state == .needsSignIn)
        #expect(remote.transport == "http")
        #expect(remote.target == "https://mcp.linear.app/mcp")
        #expect(remote.tools.isEmpty)
    }

    @Test func turningOffShowsAsDisabled() throws {
        let servers = MCPServerStatus.list(from: try #require(reply("yh-4", in: try events())))
        #expect(servers.first { $0.name == "flag-echo" }?.state == .disabled)
    }

    @Test func failuresComeBackAsErrors() throws {
        let events = try events()
        #expect(events.contains { if case .controlFailed("yh-6", let message) = $0 { message.contains("ENOENT") } else { false } })
        #expect(events.contains(.controlFailed(requestID: "yh-8", message: "Server not found: no-such-server")))
    }

    @Test func signInHandsBackABrowserPage() throws {
        let signIn = MCPSignIn(json: try #require(reply("yh-7", in: try events())))
        #expect(signIn.url?.host() == "mcp.linear.app")
        #expect(signIn.callbackExpected)
    }

    @Test func sessionStartSaysWhichServersAreUp() throws {
        let info = try #require(try events().lazy.compactMap { event -> SessionInfo? in
            if case .sessionStarted(let info) = event { info } else { nil }
        }.first)
        let servers = try #require(info.mcpServers)
        #expect(servers.map(\.name) == ["project-echo", "flag-echo", "broken", "remote-auth"])
        #expect(servers.map(\.state) == [.connected, .connected, .failed, .needsSignIn])
        #expect(servers.first?.scope == "project")
    }

    @Test func promptsArriveAsCommandsWhenServersConnect() throws {
        let lists = try events().compactMap { event -> [SlashCommand]? in
            if case .commandsChanged(let commands) = event { commands } else { nil }
        }
        let last = try #require(lists.last)
        let prompt = try #require(last.first { $0.mcpServer == "project-echo" })
        #expect(prompt.name == "mcp__project-echo__greet")
        #expect(prompt.displayName == "project-echo:greet")
        #expect(prompt.description == "Say hi")
        #expect(last.contains { $0.name == "mcp" && $0.mcpServer == nil })
    }

    @Test func promptNamesAreTypedTheWayClaudeCodeTakesThem() {
        let command = SlashCommand(json: ["name": "claude.ai Linear:create issue (MCP)", "description": "x"])
        #expect(command?.name == "mcp__claude_ai_Linear__create_issue")
        #expect(command?.mcpServer == "claude.ai Linear")
        #expect(SlashCommand.mcpPrompt(listedAs: "review (MCP)") == nil)
        #expect(SlashCommand.mcpPrompt(listedAs: "github:review") == nil)
        #expect(SlashCommand(json: ["name": "compact"])?.displayName == "compact")
    }

    @Test func unknownStatesAreKept() {
        let server = MCPServerStatus(json: ["name": "x", "status": "sleeping"])
        #expect(server?.state == .other("sleeping"))
        #expect(MCPServerStatus(json: ["status": "connected"]) == nil)
    }
}

/// Drives the real `claude` CLI on a scratch config folder, so turning servers off never
/// touches yours. No message is sent: none of this uses your subscription.
/// Opt in with: YH_INTEGRATION=1 swift test --filter Integration
@Suite(.enabled(if: ProcessInfo.processInfo.environment["YH_INTEGRATION"] == "1"), .serialized)
struct MCPServerIntegrationTests {
    private static let echoServer = #"""
    import sys, json
    for line in sys.stdin:
        m = json.loads(line)
        if "id" not in m: continue
        method = m.get("method")
        if method == "initialize":
            r = {"protocolVersion": m["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "echo", "version": "1.0"}}
        elif method == "tools/list":
            r = {"tools": [{"name": "echo", "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": True}}]}
        else:
            r = {}
        print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
    """#

    @Test func statusTogglesAndErrors() async throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("yh-mcp-\(UUID().uuidString)")
        let work = folder.appendingPathComponent("work")
        try FileManager.default.createDirectory(at: work, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let script = folder.appendingPathComponent("echo.py")
        try Self.echoServer.write(to: script, atomically: true, encoding: .utf8)
        let config: JSONValue = ["mcpServers": [
            "echo": ["command": "python3", "args": ["-I", .string(script.path)]],
            "broken": ["command": "/nonexistent/bin/server"],
        ]]

        var environment = await ShellEnvironment.load()
        environment.removeValue(forKey: "CLAUDECODE")
        environment["CLAUDE_CONFIG_DIR"] = folder.appendingPathComponent("config").path
        var options = ClaudeLaunchOptions(executable: try #require(ShellEnvironment.which("claude", in: environment)),
                                          workingDirectory: work, environment: environment, model: "haiku")
        options.mcpConfigs = [config.json]
        options.persistsSession = false
        let session = try ClaudeSession(options: options)
        try await session.initialize()

        // Servers connect after the process starts.
        var servers: [MCPServerStatus] = []
        for _ in 0..<40 {
            servers = try await session.mcpStatus()
            if servers.first(where: { $0.name == "echo" })?.state == .connected { break }
            try await Task.sleep(for: .milliseconds(250))
        }
        let echo = try #require(servers.first { $0.name == "echo" })
        #expect(echo.state == .connected)
        #expect(echo.tools.map(\.name) == ["echo"])
        #expect(servers.first { $0.name == "broken" }?.state == .failed)

        try await session.setMCPServer("echo", enabled: false)
        #expect(try await session.mcpStatus().first { $0.name == "echo" }?.state == .disabled)
        try await session.setMCPServer("echo", enabled: true)

        await #expect(throws: ClaudeControlError.self) { try await session.reconnectMCPServer("broken") }
        await #expect(throws: ClaudeControlError.failed("Server not found: nope")) {
            try await session.setMCPServer("nope", enabled: false)
        }

        await session.terminate()
        await #expect(throws: (any Error).self) { _ = try await session.mcpStatus() }
    }
}
