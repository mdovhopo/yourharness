import Foundation
import Testing
@testable import HarnessCore

/// Drives the real `claude` CLI (uses a little of your subscription, on Haiku).
/// Opt in with: YH_INTEGRATION=1 swift test --filter Integration
@Suite(.enabled(if: ProcessInfo.processInfo.environment["YH_INTEGRATION"] == "1"), .serialized)
struct ClaudeSessionIntegrationTests {
    private func makeSession() async throws -> ClaudeSession {
        var environment = await ShellEnvironment.load()
        environment.removeValue(forKey: "CLAUDECODE")
        let executable = try #require(ShellEnvironment.which("claude", in: environment))
        return try ClaudeSession(options: ClaudeLaunchOptions(
            executable: executable,
            workingDirectory: FileManager.default.temporaryDirectory,
            environment: environment,
            model: "haiku"
        ))
    }

    /// Collects events until `stop` returns true for one of them.
    private func collect(_ session: ClaudeSession, until stop: @escaping (AgentEvent) -> Bool) async -> [AgentEvent] {
        var events: [AgentEvent] = []
        for await event in session.events {
            events.append(event)
            if stop(event) { break }
        }
        return events
    }

    @Test func twoTurnsInOneProcessThenCleanExit() async throws {
        let session = try await makeSession()
        try await session.send(text: "Reply with exactly: one")

        var turns = 0
        var texts: [String] = []
        var current = ""
        var exit: Int32?
        for await event in session.events {
            switch event {
            case .textDelta(_, let text):
                current += text
            case .turnCompleted:
                turns += 1
                texts.append(current)
                current = ""
                if turns == 1 {
                    try await session.send(text: "Reply with exactly: two")
                } else {
                    await session.terminate()
                }
            case .processExited(let code, _):
                exit = code
            default:
                break
            }
        }
        #expect(turns == 2)
        #expect(texts.first?.contains("one") == true)
        #expect(texts.last?.contains("two") == true)
        #expect(exit != nil, "stream must end with processExited after the process stops")
    }

    @Test func catalogListsModelsWithoutSendingAMessage() async throws {
        var environment = await ShellEnvironment.load()
        environment.removeValue(forKey: "CLAUDECODE")
        let executable = try #require(ShellEnvironment.which("claude", in: environment))
        let catalog = try #require(await ClaudeCatalog.load(executable: executable, environment: environment))
        #expect(catalog.models.contains { $0.value == "default" })
        #expect(catalog.models.contains { $0.value == "opus" })
        #expect(catalog.models.allSatisfy { !$0.displayName.isEmpty })
    }

    @Test func shellEnvironmentLoadsWithinTimeouts() async {
        let clock = ContinuousClock()
        let start = clock.now
        let environment = await ShellEnvironment.load()
        #expect(clock.now - start < .seconds(8))
        #expect(environment["PATH"] != nil)
        #expect(environment["HOME"] != nil)
        #expect(ShellEnvironment.which("claude", in: environment) != nil)
        // Run from inside Claude Code, the test process has these; the agents must not.
        for key in ShellEnvironment.sessionVariables {
            #expect(environment[key] == nil, "\(key) leaked into the agent environment")
        }
    }

    @Test func discoveryOfEveryCLIFinishesWithinTimeout() async throws {
        let environment = await ShellEnvironment.load()
        let clock = ContinuousClock()
        for cli in AgentCLI.all {
            let start = clock.now
            let status = await CLIDiscovery.probe(cli, environment: environment)
            let elapsed = clock.now - start
            print("probe \(cli.id): \(status.state) in \(elapsed)")
            #expect(elapsed < .seconds(10), "\(cli.id) took \(elapsed)")
        }
    }

    @Test func liveModelSwitchAppliesToNextTurn() async throws {
        let session = try await makeSession()
        try await session.setModel("sonnet")
        try await session.send(text: "Reply with exactly: ok")
        var model: String?
        for await event in session.events {
            if case .sessionStarted(let info) = event { model = info.model }
            if case .turnCompleted = event { break }
        }
        #expect(model?.contains("sonnet") == true)
        await session.terminate()
    }

    @Test func interruptStopsTurnAndKeepsProcessAlive() async throws {
        let session = try await makeSession()
        try await session.send(text: "Write a 600-word essay about lighthouses.")

        var interrupted = false
        let events = await collect(session) { event in
            if case .textDelta = event, !interrupted {
                interrupted = true
                Task { try? await session.interrupt() }
            }
            if case .turnCompleted = event { return true }
            return false
        }
        guard case .turnCompleted(let result) = events.last else {
            Issue.record("expected turnCompleted after interrupt")
            return
        }
        #expect(result.isError)
        #expect(await session.isRunning)
        await session.terminate()
    }
}

/// An in-app tool, called by the real CLI over the control channel.
@Suite(.enabled(if: ProcessInfo.processInfo.environment["YH_INTEGRATION"] == "1"))
struct MCPIntegrationTests {
    private struct Secret: MCPToolServer {
        let name = "yh"
        var tools: [MCPTool] { [MCPTool(name: "secret_word", description: "Returns today's secret word.", inputSchema: MCPTool.schema([:]), readOnly: true)] }
        func call(_ tool: String, arguments: JSONValue, toolUseID: String?) async -> MCPToolResult { .text("pelican") }
    }

    @Test func claudeCallsAnInAppTool() async throws {
        var environment = await ShellEnvironment.load()
        environment.removeValue(forKey: "CLAUDECODE")
        let executable = try #require(ShellEnvironment.which("claude", in: environment))
        var options = ClaudeLaunchOptions(executable: executable, workingDirectory: FileManager.default.temporaryDirectory,
                                          environment: environment, model: "haiku")
        options.tools = []
        options.withoutMCP = true
        options.persistsSession = false
        options.allowedTools = ["mcp__yh"]
        let session = try ClaudeSession(options: options, servers: [Secret()])
        try await session.initialize()
        try await session.send(text: "Call the secret_word tool, then reply with only the word it returned.")
        var reply = "", called = false
        for await event in session.events {
            switch event {
            case .assistantMessage(_, let blocks, _):
                for case .toolUse(_, let name, _) in blocks where name == "mcp__yh__secret_word" { called = true }
            case .textDelta(_, let text): reply += text
            case .permissionRequest: Issue.record("Allowed tools shouldn't ask")
            case .turnCompleted: await session.terminate()
            default: break
            }
        }
        #expect(called)
        #expect(reply.lowercased().contains("pelican"))
    }
}
