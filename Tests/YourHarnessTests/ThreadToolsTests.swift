import Foundation
import HarnessCore
import Testing
@testable import YourHarness

/// What the thread tools reach and how they show it.
struct ThreadToolsTests {
    @Test func insideAFolderMeansItOrBelowIt() {
        #expect(ThreadTools.isInside("/Users/me/dev/work", "/Users/me/dev/work"))
        #expect(ThreadTools.isInside("/Users/me/dev/work/api/Sources", "/Users/me/dev/work/"))
        #expect(ThreadTools.isInside("/Users/me/dev/other/../work/api", "/Users/me/dev/work"))
        #expect(!ThreadTools.isInside("/Users/me/dev/workshop", "/Users/me/dev/work"))
        #expect(!ThreadTools.isInside("/Users/me/dev", "/Users/me/dev/work"))
        #expect(!ThreadTools.isInside(nil, "/Users/me/dev/work"))
    }

    @Test func findsSessionsInAFolderNewestFirst() throws {
        let config = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: config) }
        let work = "/tmp/yh-tools/work"
        let api = try session(config, cwd: "\(work)/api", says: "Fix the login flow", age: 60)
        let root = try session(config, cwd: work, says: "Plan the release", age: 120)
        let elsewhere = try session(config, cwd: "/tmp/yh-tools/workshop", says: "Fix the printer", age: 10)

        let inWork = ThreadTools.findSessions(config: config, inside: work, words: [], skipping: [], limit: 10)
        #expect(inWork.map(\.sessionID) == [api, root])

        let everywhere = ThreadTools.findSessions(config: config, inside: nil, words: [], skipping: [], limit: 10)
        #expect(everywhere.map(\.sessionID) == [elsewhere, api, root])

        let fixes = ThreadTools.findSessions(config: config, inside: nil, words: ["fix"], skipping: [], limit: 10)
        #expect(fixes.map(\.sessionID) == [elsewhere, api])

        let notImported = ThreadTools.findSessions(config: config, inside: nil, words: [], skipping: [elsewhere], limit: 1)
        #expect(notImported.map(\.sessionID) == [api])
    }

    @Test func showsEntriesBriefly() {
        let user = EntrySnapshot(id: "1", kind: .user, text: "Why is it slow?")
        #expect(ThreadTools.render(user, index: 3) == "[3] user: Why is it slow?")
        let tool = EntrySnapshot(id: "2", kind: .tool, text: "", toolName: "Bash", toolInput: #"{"command":"swift test"}"#,
                                 toolResult: "line one\nline two", toolIsError: true)
        #expect(ThreadTools.render(tool, index: 4) == "[4] tool Bash: {\"command\":\"swift test\"}\n      → error: line one line two")
        let long = EntrySnapshot(id: "3", kind: .assistant, text: String(repeating: "a", count: 4010))
        #expect(ThreadTools.render(long, index: 5).hasSuffix("… (10 more characters)"))
    }

    /// A session file as Claude Code writes one: a question and its answer.
    private func session(_ config: URL, cwd: String, says text: String, age: TimeInterval) throws -> String {
        let id = UUID().uuidString.lowercased()
        let folder = config.appendingPathComponent("projects").appendingPathComponent(ClaudeSessionArchive.folderName(for: cwd))
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let lines = [
            #"{"parentUuid":null,"isSidechain":false,"type":"user","message":{"role":"user","content":"\#(text)"},"uuid":"u1","timestamp":"2026-10-01T10:00:00.000Z","cwd":"\#(cwd)","sessionId":"\#(id)"}"#,
            #"{"parentUuid":"u1","isSidechain":false,"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"On it."}]},"uuid":"a1","timestamp":"2026-10-01T10:00:01.000Z","cwd":"\#(cwd)","sessionId":"\#(id)"}"#,
        ]
        let file = folder.appendingPathComponent("\(id).jsonl")
        try Data(lines.joined(separator: "\n").utf8).write(to: file)
        try FileManager.default.setAttributes([.modificationDate: Date(timeIntervalSinceNow: -age)], ofItemAtPath: file.path)
        return id
    }
}
