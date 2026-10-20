import Foundation
import Testing
@testable import HarnessCore

/// Fixtures are real captures (sanitized) of a second message sent during `sleep 8`:
/// steering (no priority), and a queued message when the turn is then interrupted.
struct SteeringTests {
    private func events(fromFixture name: String) throws -> [AgentEvent] {
        let url = try #require(Bundle.module.url(forResource: name, withExtension: "jsonl", subdirectory: "Fixtures"))
        var parser = ClaudeStreamParser()
        return try String(contentsOf: url, encoding: .utf8)
            .split(separator: "\n")
            .flatMap { parser.parse(line: Data($0.utf8)) }
    }

    private func lifecycle(_ events: [AgentEvent]) -> [(id: String, state: MessageState, index: Int)] {
        events.enumerated().compactMap { index, event in
            if case .messageLifecycle(let id, let state) = event { return (id, state, index) }
            return nil
        }
    }

    @Test func aSteeringMessageJoinsTheRunningTurn() throws {
        let events = try events(fromFixture: "steering")
        let states = lifecycle(events)
        let ids = Array(Set(states.map(\.id)))
        #expect(ids.count == 2)
        // The second message: queued during the turn, picked up before its only result.
        let second = try #require(states.first { $0.state == .queued && $0.index > states[0].index && $0.id != states[0].id }?.id)
        let started = try #require(states.first { $0.id == second && $0.state == .started }?.index)
        let results = events.indices.filter { if case .turnCompleted = events[$0] { true } else { false } }
        #expect(results.count == 1, "steering doesn't start a second turn")
        #expect(started < results[0])
        // After the running tool's result: between steps, not mid-tool.
        let firstToolResult = try #require(events.firstIndex { if case .toolResults(_, nil) = $0 { true } else { false } })
        #expect(started > firstToolResult)
    }

    @Test func interruptingHandsOverWhatsQueued() throws {
        let events = try events(fromFixture: "interrupt-with-queued")
        let states = lifecycle(events)
        let first = try #require(states.first?.id)
        let queued = try #require(states.first { $0.id != first }?.id)
        let interrupted = try #require(events.firstIndex { if case .turnCompleted(let result) = $0 { result.isError } else { false } })
        #expect(states.contains { $0.id == first && $0.state == .cancelled })
        let started = try #require(states.first { $0.id == queued && $0.state == .started }?.index)
        #expect(started > interrupted, "the queued message starts right after the stopped turn")
    }

    @Test func messagesCarryTheirIDAndPriority() throws {
        let message = ClaudeSession.userMessage(text: "hi", id: "m-1", priority: .now)
        #expect(message["uuid"] as? String == "m-1")
        #expect(message["priority"] as? String == "now")
        let plain = ClaudeSession.userMessage(text: "hi")
        #expect(plain["uuid"] == nil && plain["priority"] == nil)
    }

    @Test func lifecycleLines() {
        var parser = ClaudeStreamParser()
        let line = #"{"type":"command_lifecycle","command_uuid":"m-1","state":"started","uuid":"x","session_id":"s"}"#
        #expect(parser.parse(line: Data(line.utf8)) == [.messageLifecycle(id: "m-1", state: .started)])
        let unknown = #"{"type":"command_lifecycle","command_uuid":"m-1","state":"teleported"}"#
        #expect(parser.parse(line: Data(unknown.utf8)).isEmpty)
    }
}

/// The real CLI on Haiku: a message sent during a slow tool call steers the turn, and one
/// sent with an interrupt goes in right away.
@Suite(.enabled(if: ProcessInfo.processInfo.environment["YH_INTEGRATION"] == "1"), .serialized)
struct SteeringIntegrationTests {
    private func session() async throws -> ClaudeSession {
        var environment = await ShellEnvironment.load()
        environment.removeValue(forKey: "CLAUDECODE")
        let executable = try #require(ShellEnvironment.which("claude", in: environment))
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("yh-steer-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var options = ClaudeLaunchOptions(executable: executable, workingDirectory: folder, environment: environment,
                                          model: "haiku", permissionMode: "bypassPermissions")
        options.persistsSession = false
        return try ClaudeSession(options: options)
    }

    /// Sends the second message once the first tool call starts; returns when the CLI says
    /// it started, how many turns ended before that, and the text Claude wrote.
    private func run(interrupting: Bool) async throws -> (turnsBefore: Int, startedAfter: Duration, text: String) {
        let session = try await session()
        try await session.send(text: "Run `sleep 6` with Bash, then reply with just DONE.", id: "first")
        let clock = ContinuousClock()
        var sentAt: ContinuousClock.Instant?
        var startedAfter: Duration?
        var turns = 0
        var text = ""
        for await event in session.events {
            switch event {
            case .blockStarted(_, .toolUse) where sentAt == nil:
                if interrupting { try await session.interrupt() }
                try await session.send(text: "Also say the word KIWI.", id: "second", priority: interrupting ? .now : nil)
                sentAt = clock.now
            case .messageLifecycle("second", .started):
                startedAfter = sentAt.map { clock.now - $0 }
            case .textDelta(_, let delta):
                text += delta
            case .turnCompleted:
                turns += 1
            default:
                break
            }
            if startedAfter != nil, case .turnCompleted = event, !interrupting || turns >= 2 { break }
        }
        await session.terminate()
        return (turns, try #require(startedAfter), text)
    }

    @Test func steeringJoinsTheTurn() async throws {
        let result = try await run(interrupting: false)
        #expect(result.startedAfter > .seconds(3), "it waited for the sleep to finish")
        #expect(result.text.uppercased().contains("KIWI"))
    }

    @Test func interruptingSendsRightAway() async throws {
        let result = try await run(interrupting: true)
        #expect(result.startedAfter < .seconds(3), "it didn't wait for the sleep")
        #expect(result.text.uppercased().contains("KIWI"))
    }
}
