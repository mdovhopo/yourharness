import Foundation
import Testing
@testable import HarnessCore

struct ClaudeUsageProbeTests {
    /// Shape of a real probe run (`claude -p ... --output-format stream-json`), trimmed.
    private let stream = """
    {"type":"system","subtype":"init","session_id":"s","model":"claude-haiku-4-5","tools":[]}
    {"type":"assistant","message":{"id":"m","role":"assistant","content":[{"type":"text","text":"pong"}]},"session_id":"s"}
    {"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1792448400,"rateLimitType":"five_hour","unifiedWindows":{"five_hour":{"utilization":0.06,"resetsAt":1792448400},"seven_day":{"utilization":0.1,"resetsAt":1792958400}}},"session_id":"s"}
    {"type":"result","subtype":"success","is_error":false,"result":"pong","session_id":"s"}
    """

    @Test func readsLimitsFromAProbeRun() throws {
        let info = try #require(ClaudeUsageProbe.parse(stream))
        let byName = Dictionary(uniqueKeysWithValues: info.windows.map { ($0.name, $0) })
        #expect(byName["five_hour"]?.utilization == 0.06)
        #expect(byName["seven_day"]?.utilization == 0.1)
    }

    @Test func noEventMeansNoReading() {
        #expect(ClaudeUsageProbe.parse(#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in"}"#) == nil)
    }

    @Test(.enabled(if: ProcessInfo.processInfo.environment["YH_INTEGRATION"] == "1"))
    func realProbeReportsLimits() async throws {
        var environment = ProcessInfo.processInfo.environment
        for key in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SSE_PORT"] { environment.removeValue(forKey: key) }
        let claude = try #require(ShellEnvironment.which("claude", in: environment))
        let started = ContinuousClock.now
        let result = await ClaudeUsageProbe.run(executable: claude, environment: environment)
        print("usage probe: \(result) in \(ContinuousClock.now - started)")
        let info = try result.get()
        #expect(!info.windows.isEmpty)
    }
}
