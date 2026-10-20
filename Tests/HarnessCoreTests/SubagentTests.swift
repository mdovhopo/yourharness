import Foundation
import Testing
@testable import HarnessCore

/// Fixtures are real captures (sanitized) of Haiku handing a line count to a sub-agent.
struct SubagentParsingTests {
    private func events(fromFixture name: String) throws -> [AgentEvent] {
        let url = try #require(Bundle.module.url(forResource: name, withExtension: "jsonl", subdirectory: "Fixtures"))
        var parser = ClaudeStreamParser()
        return try String(contentsOf: url, encoding: .utf8)
            .split(separator: "\n")
            .flatMap { parser.parse(line: Data($0.utf8)) }
    }

    private func agentCallID(_ events: [AgentEvent]) throws -> String {
        try #require(events.lazy.compactMap { event -> String? in
            if case .blockStarted(_, .toolUse(let id, "Agent")) = event { return id }
            return nil
        }.first)
    }

    @Test func backgroundSubagentOutlivesItsTurn() throws {
        let events = try events(fromFixture: "subagent-background")
        let agent = try agentCallID(events)

        let start = try #require(events.lazy.compactMap { event -> SubagentStart? in
            if case .subagentStarted(let start) = event { return start }
            return nil
        }.first)
        #expect(start.toolUseID == agent)
        #expect(start.isBackground)
        #expect(start.agentType == "general-purpose")
        #expect(start.description == "Count lines in a.txt and b.txt")
        #expect(!start.taskID.isEmpty)
        #expect(start.prompt?.isEmpty == false)

        // The main turn ends while the sub-agent is still working, then a second turn
        // starts on its own to report back.
        let firstResult = try #require(events.firstIndex { if case .turnCompleted = $0 { true } else { false } })
        let finished = try #require(events.firstIndex { if case .subagentFinished = $0 { true } else { false } })
        #expect(firstResult < finished)
        #expect(events.count { if case .turnCompleted = $0 { true } else { false } } == 2)

        let progress = events.compactMap { event -> String? in
            if case .subagentProgress(agent, let activity, _) = event { return activity }
            return nil
        }
        #expect(progress == ["Reading a.txt", "Reading b.txt"])

        let ownTools = events.flatMap { event -> [String] in
            guard case .assistantMessage(_, let blocks, agent) = event else { return [] }
            return blocks.compactMap { if case .toolUse(_, let name, _) = $0 { name } else { nil } }
        }
        #expect(ownTools == ["Read", "Read"])
        let ownResults = events.flatMap { event -> [ToolResult] in
            if case .toolResults(let results, agent) = event { return results }
            return []
        }
        #expect(ownResults.count == 2)

        guard case .subagentFinished(let end) = events[finished] else { return }
        #expect(end.toolUseID == agent)
        #expect(end.status == "completed")
        #expect(end.summary?.contains("a.txt") == true)
        #expect(end.usage.toolUses == 2)
        #expect((end.usage.tokens ?? 0) > 0)
    }

    @Test func foregroundSubagentEndsBeforeItsToolResult() throws {
        let events = try events(fromFixture: "subagent-foreground")
        let agent = try agentCallID(events)
        let started = try #require(events.firstIndex { if case .subagentStarted(let s) = $0 { s.toolUseID == agent && !s.isBackground } else { false } })
        let finished = try #require(events.firstIndex { if case .subagentFinished(let e) = $0 { e.toolUseID == agent } else { false } })
        let result = try #require(events.firstIndex { event in
            guard case .toolResults(let results, nil) = event else { return false }
            return results.contains { $0.toolUseID == agent }
        })
        #expect(started < finished && finished < result)
        #expect(events.count { if case .turnCompleted = $0 { true } else { false } } == 1)

        guard case .toolResults(let results, nil) = events[result] else { return }
        let report = try #require(SubagentReport.text(fromToolResult: results[0].content))
        #expect(report.hasPrefix("Both files were readable."))
        #expect(!report.contains("hand-back"))
    }

    @Test func otherBackgroundTasksAreNotSubagents() {
        var parser = ClaudeStreamParser()
        let line = #"{"type":"system","subtype":"task_started","task_id":"b1","tool_use_id":"toolu_1","task_type":"local_bash","description":"npm run dev"}"#
        #expect(parser.parse(line: Data(line.utf8)).isEmpty)
    }
}

struct SubagentReportTests {
    @Test func unwrapsTheHandBackFrame() {
        let result = "[Subagent hand-back] The text below is the final report of a subagent. The report follows:\n  **Done.**\n  \n  - one\n    - nested"
        #expect(SubagentReport.text(fromToolResult: result) == "**Done.**\n\n- one\n  - nested")
    }

    @Test func theLaunchPlaceholderIsNoReport() {
        #expect(SubagentReport.text(fromToolResult: "Async agent launched successfully. (This tool result is internal…)") == nil)
    }

    @Test func plainResultsPassThrough() {
        #expect(SubagentReport.text(fromToolResult: "3 lines") == "3 lines")
    }
}

/// The real CLI: stopping a sub-agent mid-task. On Haiku.
@Suite(.enabled(if: ProcessInfo.processInfo.environment["YH_INTEGRATION"] == "1"))
struct SubagentIntegrationTests {
    @Test func stopsASubagent() async throws {
        var environment = await ShellEnvironment.load()
        environment.removeValue(forKey: "CLAUDECODE")
        let executable = try #require(ShellEnvironment.which("claude", in: environment))
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("yh-subagent-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let session = try ClaudeSession(options: ClaudeLaunchOptions(
            executable: executable, workingDirectory: folder, environment: environment,
            model: "haiku", permissionMode: "bypassPermissions"
        ))
        try await session.send(text: "Use the Agent tool (general-purpose) to run the shell command `sleep 120` with Bash and then report \"slept\". Don't do it yourself.")

        var end: SubagentEnd?
        for await event in session.events {
            switch event {
            case .subagentStarted(let start):
                try await Task.sleep(for: .seconds(4))
                try await session.stopTask(start.taskID)
            case .subagentFinished(let finished):
                end = finished
            default:
                continue
            }
            if end != nil { break }
        }
        await session.terminate()
        let status = try #require(end?.status)
        #expect(status != "completed", "stopped well before the sleep could finish")
    }
}
