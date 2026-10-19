import Foundation
import Testing
@testable import HarnessCore

/// Fixtures are real `claude -p --output-format stream-json` captures (sanitized).
struct ClaudeStreamParserTests {
    private func events(fromFixture name: String) throws -> [AgentEvent] {
        let url = try #require(Bundle.module.url(forResource: name, withExtension: "jsonl", subdirectory: "Fixtures"))
        var parser = ClaudeStreamParser()
        return try String(contentsOf: url, encoding: .utf8)
            .split(separator: "\n")
            .flatMap { parser.parse(line: Data($0.utf8)) }
    }

    @Test func toolTurnProducesSessionToolCallAndResult() throws {
        let events = try events(fromFixture: "tool-turn")

        guard case .sessionStarted(let info) = try #require(events.first) else {
            Issue.record("first event should be sessionStarted, got \(events.first!)")
            return
        }
        #expect(!info.sessionID.isEmpty)
        #expect(info.cwd == "/tmp/project")

        let toolStart = events.compactMap { event -> (String, String)? in
            if case .blockStarted(_, .toolUse(let id, let name)) = event { return (id, name) }
            return nil
        }
        #expect(toolStart.count == 1)
        #expect(toolStart.first?.1 == "Bash")

        let streamedInput = events.compactMap { event -> String? in
            if case .toolInputDelta(_, let json) = event { return json }
            return nil
        }.joined()
        let input = try #require(try JSONSerialization.jsonObject(with: Data(streamedInput.utf8)) as? [String: Any])
        #expect(input["command"] as? String == "echo hi")

        let results = events.flatMap { event -> [ToolResult] in
            if case .toolResults(let results, nil) = event { return results }
            return []
        }
        #expect(results == [ToolResult(toolUseID: toolStart[0].0, content: "hi", isError: false)])

        let text = events.compactMap { event -> String? in
            if case .textDelta(_, let text) = event { return text }
            return nil
        }.joined()
        #expect(text.hasSuffix("done"))

        guard case .turnCompleted(let result) = try #require(events.last) else {
            Issue.record("last event should be turnCompleted")
            return
        }
        #expect(!result.isError)
        #expect(result.subtype == "success")
    }

    @Test func deltasCarryTheirMessageID() throws {
        let events = try events(fromFixture: "tool-turn")
        let refs = events.compactMap { event -> BlockRef? in
            switch event {
            case .blockStarted(let ref, _), .textDelta(let ref, _), .toolInputDelta(let ref, _): ref
            default: nil
            }
        }
        #expect(!refs.isEmpty)
        #expect(refs.allSatisfy { $0.messageID.hasPrefix("msg_") })
        // Two assistant messages: the tool call, then the final reply.
        #expect(Set(refs.map(\.messageID)).count == 2)
    }

    @Test func rateLimitEventExposesUsageWindows() throws {
        let events = try events(fromFixture: "tool-turn")
        let info = try #require(events.compactMap { event -> RateLimitInfo? in
            if case .rateLimit(let info) = event { return info }
            return nil
        }.first)
        #expect(info.status == "allowed")
        #expect(info.windows.map(\.name) == ["five_hour", "seven_day"])
        #expect(info.windows.allSatisfy { (0...1).contains($0.utilization) && $0.resetsAt != nil })
    }

    @Test func permissionRequestIsParsed() throws {
        let events = try events(fromFixture: "permission-turn")
        let request = try #require(events.compactMap { event -> PermissionRequest? in
            if case .permissionRequest(let request) = event { return request }
            return nil
        }.first)
        #expect(request.toolName == "Bash")
        #expect(!request.requestID.isEmpty)
        let input = try #require(try JSONSerialization.jsonObject(with: Data(request.inputJSON.utf8)) as? [String: Any])
        #expect(input["command"] as? String == "touch probe_file.txt")
    }

    @Test func unknownAndMalformedLinesAreIgnored() {
        var parser = ClaudeStreamParser()
        #expect(parser.parse(line: Data("not json".utf8)).isEmpty)
        #expect(parser.parse(line: Data(#"{"type":"some_future_event","x":1}"#.utf8)).isEmpty)
        #expect(parser.parse(line: Data(#"{"no_type":true}"#.utf8)).isEmpty)
    }

    @Test func toolResultWithContentPartsIsFlattened() {
        var parser = ClaudeStreamParser()
        let line = #"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}],"is_error":true}]}}"#
        #expect(parser.parse(line: Data(line.utf8)) == [
            .toolResults([ToolResult(toolUseID: "t1", content: "a\nb", isError: true)], parentToolUseID: nil),
        ])
    }

    @Test func contextUsageAndWindowAreReported() throws {
        let events = try events(fromFixture: "tool-turn")
        let usages = events.compactMap { event -> Int? in
            if case .contextUsage(let tokens) = event { return tokens }
            return nil
        }
        // One per assistant message; the second includes the first's context plus the tool result.
        #expect(usages.count == 2)
        #expect(usages[0] == 2 + 10_363 + 11_229 + 77)
        #expect(usages[1] > usages[0])

        guard case .turnCompleted(let result) = try #require(events.last) else {
            Issue.record("last event should be turnCompleted")
            return
        }
        #expect(result.contextWindows == ["claude-haiku-5-5": 1_000_000])
    }

    @Test func initializeResponseBecomesCatalog() {
        var parser = ClaudeStreamParser()
        let line = #"{"type":"control_response","response":{"subtype":"success","request_id":"yh-1","response":{"claude_code_version":"2.1.302","models":[{"value":"default","resolvedModel":"claude-opus-5-5","displayName":"Default (recommended)","description":"Opus 5.5","supportedEffortLevels":["low","high"],"supportsAutoMode":true},{"value":"claude-opus-5","displayName":"Opus 5"},{"bogus":true}]}}}"#
        guard case .catalog(let catalog) = parser.parse(line: Data(line.utf8)).first else {
            Issue.record("expected a catalog")
            return
        }
        #expect(catalog.cliVersion == "2.1.302")
        #expect(catalog.models.map(\.value) == ["default", "claude-opus-5"])
        #expect(catalog.models[0].supportedEffortLevels == ["low", "high"])
        #expect(catalog.models[0].isAlias)
        #expect(!catalog.models[1].isAlias)
        // Models without auto mode simply omit the key.
        #expect(catalog.models[0].supportsAutoMode == true)
        #expect(catalog.models[1].supportsAutoMode == false)
    }

    @Test func controlErrorsAreSurfaced() {
        var parser = ClaudeStreamParser()
        let line = #"{"type":"control_response","response":{"subtype":"error","request_id":"yh-3","error":"Unsupported control request subtype: set_effort"}}"#
        #expect(parser.parse(line: Data(line.utf8)) == [
            .controlFailed(requestID: "yh-3", message: "Unsupported control request subtype: set_effort"),
        ])
        // Plain acks (set_model, interrupt) carry nothing the UI needs.
        let ack = #"{"type":"control_response","response":{"subtype":"success","request_id":"yh-2"}}"#
        #expect(parser.parse(line: Data(ack.utf8)).isEmpty)
    }

    @Test func shellEnvironmentParsesAfterMarker() {
        let raw = "motd noise\n__YOURHARNESS_ENV__PATH=/usr/bin:/bin\0HOME=/Users/x\0EMPTY=\0"
        let env = ShellEnvironment.parse(Data(raw.utf8))
        #expect(env == ["PATH": "/usr/bin:/bin", "HOME": "/Users/x", "EMPTY": ""])
    }
}
