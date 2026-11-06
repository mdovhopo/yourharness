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

        // Added through Claude Code's own command, the running session finds it by name.
        let executable = try #require(ShellEnvironment.which("claude", in: environment))
        let added = await ClaudeMCPConfig.add(name: "late", config: MCPServerConfig.command("python3", arguments: ["-I", script.path]).json,
                                              scope: .user, executable: executable, environment: environment, folder: work)
        if case .failure(let error) = added { Issue.record("add failed: \(error)") }
        try await session.connectNewMCPServer("late")
        #expect(try await session.mcpStatus().contains { $0.name == "late" })
        if case .failure(let error) = await ClaudeMCPConfig.remove(name: "late", scope: .user, executable: executable, environment: environment, folder: work) {
            Issue.record("remove failed: \(error)")
        }
        if case .success = await ClaudeMCPConfig.remove(name: "late", scope: .user, executable: executable, environment: environment, folder: work) {
            Issue.record("removing it twice should fail")
        }

        await session.terminate()
        await #expect(throws: (any Error).self) { _ = try await session.mcpStatus() }
    }
}

/// A real capture (sanitized): a server's tool asking for a form, then for a page visit, as
/// Claude Code forwards them, plus the note that the page visit is done.
struct MCPElicitationParsingTests {
    private func events() throws -> [AgentEvent] {
        let url = try #require(Bundle.module.url(forResource: "mcp-elicitation", withExtension: "jsonl", subdirectory: "Fixtures"))
        var parser = ClaudeStreamParser()
        return try String(contentsOf: url, encoding: .utf8)
            .split(separator: "\n")
            .flatMap { parser.parse(line: Data($0.utf8)) }
    }

    private func elicitations() throws -> [MCPElicitation] {
        try events().compactMap { if case .elicitation(let e) = $0 { e } else { nil } }
    }

    @Test func formFieldsComeInTheServersOrder() throws {
        let form = try #require(try elicitations().first)
        #expect(form.mode == .form)
        #expect(form.server == "elicit")
        #expect(form.message == "Where should this deploy go?")
        #expect(form.fields.map(\.name) == ["environment", "replicas", "notify", "note"])

        let environment = form.fields[0]
        #expect(environment.required)
        guard case .choice(let options) = environment.kind else { Issue.record("not a choice"); return }
        #expect(options.map(\.value) == ["staging", "production"])
        #expect(options.map(\.label) == ["Staging", "Production"])

        #expect(form.fields[1].kind == .number(integer: true))
        #expect(form.fields[1].defaultValue == 2)
        #expect(!form.fields[1].required)
        #expect(form.fields[2].kind == .boolean)
        #expect(form.fields[2].defaultValue == true)
        #expect(form.fields[3].kind == .text)
        #expect(form.fields[3].description == "Anything to add")
    }

    @Test func pageVisitsCarryTheirLink() throws {
        let page = try #require(try elicitations().last)
        #expect(page.mode == .url)
        #expect(page.url?.host() == "example.com")
        #expect(page.elicitationID == "el-1")
        #expect(page.fields.isEmpty)
    }

    @Test func serversSayWhenAPageVisitIsDone() throws {
        #expect(try events().contains(.elicitationComplete(server: "elicit", elicitationID: "el-1")))
    }

    @Test func choicesCanBeOneOfWithTitles() {
        let fields = MCPElicitation.fields(schema: ["properties": ["size": ["oneOf": [
            ["const": "s", "title": "Small"], ["const": "l", "title": "Large"],
        ]]]], order: nil)
        guard case .choice(let options) = fields.first?.kind else { Issue.record("not a choice"); return }
        #expect(options.map(\.label) == ["Small", "Large"])
    }
}

struct MCPServerConfigTests {
    @Test func urlServersCarryHeadersAndOAuth() {
        let config = MCPServerConfig.url(URL(string: "https://mcp.acme.com/mcp")!, headers: ["X-Team": "a"], clientID: "abc", callbackPort: 8123).json
        #expect(config["type"] == "http")
        #expect(config["url"] == "https://mcp.acme.com/mcp")
        #expect(config["headers"]?["X-Team"] == "a")
        #expect(config["oauth"]?["clientId"] == "abc")
        #expect(config["oauth"]?["callbackPort"] == 8123)
        #expect(MCPServerConfig.url(URL(string: "https://x.dev/mcp")!).json["oauth"] == nil)
    }

    @Test func commandsSplitLikeAShell() {
        #expect(MCPServerConfig.splitCommandLine(#"npx -y "@scope/server name" --root '/a b'"#) == ["npx", "-y", "@scope/server name", "--root", "/a b"])
        let config = MCPServerConfig.command("npx", arguments: ["-y", "pkg"], environment: ["TOKEN": "t"]).json
        #expect(config["command"] == "npx")
        #expect(config["args"] == ["-y", "pkg"])
        #expect(config["env"]?["TOKEN"] == "t")
    }

    @Test func namesComeFromTheURLOrCommand() {
        #expect(MCPServerConfig.suggestedName(for: URL(string: "https://mcp.linear.app/mcp")!) == "linear")
        #expect(MCPServerConfig.suggestedName(for: URL(string: "https://api.githubcopilot.com/mcp/")!) == "githubcopilot")
        #expect(MCPServerConfig.suggestedName(forCommand: "npx -y @modelcontextprotocol/server-github") == "github")
        #expect(MCPServerConfig.suggestedName(forCommand: "uvx mcp-server-postgres postgresql://x") == "postgres")
        #expect(MCPServerConfig.suggestedName(forCommand: "npx @playwright/mcp@latest") == "mcp")
    }

    @Test func pastedJSONComesInEveryShape() {
        let block = MCPServerConfig.servers(fromJSON: #"{"mcpServers": {"a": {"command": "x"}, "b": {"url": "https://y"}}}"#, fallbackName: "z")
        #expect(block?.keys.sorted() == ["a", "b"])
        #expect(MCPServerConfig.servers(fromJSON: #"{"command": "x", "args": []}"#, fallbackName: "z")?.keys.sorted() == ["z"])
        #expect(MCPServerConfig.servers(fromJSON: #"{"a": {"type": "http", "url": "https://y"}}"#, fallbackName: "z")?.keys.sorted() == ["a"])
        #expect(MCPServerConfig.servers(fromJSON: "not json", fallbackName: "z") == nil)
        #expect(MCPServerConfig.servers(fromJSON: #"{"a": 1}"#, fallbackName: "z") == nil)
    }

    @Test func claudeDesktopServersAreRead() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent("yh-desktop-\(UUID().uuidString)")
        let folder = home.appendingPathComponent("Library/Application Support/Claude")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: home) }
        try #"{"mcpServers": {"files": {"command": "npx", "args": ["fs"]}}}"#.write(to: folder.appendingPathComponent("claude_desktop_config.json"), atomically: true, encoding: .utf8)
        #expect(ClaudeMCPConfig.claudeDesktopServers(home: home).keys.sorted() == ["files"])
        #expect(ClaudeMCPConfig.claudeDesktopServers(home: folder).isEmpty)
    }
}
