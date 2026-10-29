import Foundation
import Testing
@testable import HarnessCore

/// A real capture (sanitized), on Haiku: a background command printing a metric every 5
/// seconds, a Monitor watching its output, and Claude woken for each sample in turns of its
/// own after the first ended.
struct BackgroundJobTests {
    private func fixtureEvents() throws -> [AgentEvent] {
        let url = try #require(Bundle.module.url(forResource: "background-job-monitor", withExtension: "jsonl", subdirectory: "Fixtures"))
        var parser = ClaudeStreamParser()
        return try String(contentsOf: url, encoding: .utf8)
            .split(separator: "\n")
            .flatMap { parser.parse(line: Data($0.utf8)) }
    }

    @Test func jobsStartAndEndByTask() throws {
        let events = try fixtureEvents()
        let starts = events.compactMap { if case .jobStarted(let start) = $0 { start } else { nil } }
        #expect(starts.map(\.taskID) == ["b7xkqnlhn", "bfyo780nc"])
        #expect(starts.allSatisfy { $0.isBackground })
        #expect(starts[0].description == "Print 1-minute load average 8 times, 5s apart")
        #expect(starts[1].toolUseID == "toolu_01FSdVchqmhDs3n4mJL95ytu")

        let ends = events.compactMap { if case .jobFinished(let end) = $0 { end } else { nil } }
        #expect(ends.map(\.taskID) == ["b7xkqnlhn", "bfyo780nc"])
        #expect(ends[0].status == "failed")
        #expect(ends[0].exitCode == 1)
        #expect(ends[0].outputFile?.hasSuffix("/tasks/b7xkqnlhn.output") == true)
        #expect(ends[1].status == "completed")
        // Neither is taken for a sub-agent.
        #expect(!events.contains { if case .subagentFinished = $0 { true } else { false } })
    }

    @Test func theRunningListFollowsEveryChange() throws {
        let lists = try fixtureEvents().compactMap { if case .jobsRunning(let ids) = $0 { ids } else { nil } }
        #expect(lists == [["b7xkqnlhn"], ["b7xkqnlhn", "bfyo780nc"], ["bfyo780nc"], []])
    }

    @Test func whatTheWatchSawIsAnEventNotAMessage() throws {
        let events = try fixtureEvents()
        let seen = events.compactMap { if case .jobEvent(let id, let text) = $0 { "\(id): \(text)" } else { nil } }
        #expect(seen.first == "bfyo780nc: sample 1: 2.58\nsample 2: 2.69")
        #expect(seen.last == "bfyo780nc: sample 8: 3.05")
        // Claude woken by it: turns of its own, with no message of yours.
        let turns = events.filter { if case .turnCompleted = $0 { true } else { false } }
        #expect(turns.count > 3)
    }

    @Test func exitCodeComesFromTheSummary() {
        let end = { (summary: String) in JobEnd(taskID: "b", status: "failed", summary: summary).exitCode }
        #expect(end(#"Background command "Build" failed with exit code 127"#) == 127)
        #expect(end(#"Background command "x" completed (exit code 0)"#) == 0)
        #expect(end(#"Monitor "x" stream ended"#) == nil)
    }
}
