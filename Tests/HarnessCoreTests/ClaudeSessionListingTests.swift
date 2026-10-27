import Foundation
import Testing
@testable import HarnessCore

/// Finding sessions in Claude Code's history without reading them.
struct ClaudeSessionListingTests {
    @Test func namesFoldersAsClaudeCodeDoes() {
        #expect(ClaudeSessionArchive.folderName(for: "/Users/me/dev/my.app") == "-Users-me-dev-my-app")
        #expect(ClaudeSessionArchive.folderName(for: "/Users/me/Мой проект") == "-Users-me-----------")
    }

    @Test func narrowsToAFolderAndWhatsInside() {
        let work = "/Users/me/dev/work"
        #expect(ClaudeSessionArchive.folderName("-Users-me-dev-work", mayBeInside: work))
        #expect(ClaudeSessionArchive.folderName("-Users-me-dev-work-api", mayBeInside: work))
        #expect(!ClaudeSessionArchive.folderName("-Users-me-dev-workshop", mayBeInside: work))
        #expect(!ClaudeSessionArchive.folderName("-Users-me-dev", mayBeInside: work))
        // Claude Code shortens very long names, so those can't be ruled out by name.
        #expect(ClaudeSessionArchive.folderName(String(repeating: "x", count: 210), mayBeInside: work))
    }

    @Test func listsSessionFilesNewestFirst() throws {
        let config = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: config) }
        let older = "11111111-1111-4111-8111-111111111111", newer = "22222222-2222-4222-8222-222222222222"
        let a = config.appendingPathComponent("projects/-tmp-a"), b = config.appendingPathComponent("projects/-tmp-b")
        try FileManager.default.createDirectory(at: a, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: b, withIntermediateDirectories: true)
        try write(a.appendingPathComponent("\(older).jsonl"), modified: Date(timeIntervalSinceNow: -3600))
        try write(b.appendingPathComponent("\(newer).jsonl"), modified: .now)
        // Not sessions: a subagent's side file and anything else.
        try write(b.appendingPathComponent("agent-3f2a.jsonl"), modified: .now)
        try write(b.appendingPathComponent("notes.txt"), modified: .now)

        let files = ClaudeSessionArchive.sessionFiles(configDirectory: config)
        #expect(files.map(\.sessionID) == [newer, older])
        #expect(files.map(\.folderName) == ["-tmp-b", "-tmp-a"])
    }

    @Test func noHistoryIsNoSessions() {
        let missing = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        #expect(ClaudeSessionArchive.sessionFiles(configDirectory: missing).isEmpty)
    }

    private func write(_ url: URL, modified: Date) throws {
        try Data("{}\n".utf8).write(to: url)
        try FileManager.default.setAttributes([.modificationDate: modified], ofItemAtPath: url.path)
    }
}
