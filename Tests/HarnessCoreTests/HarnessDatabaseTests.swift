import Foundation
import Testing
@testable import HarnessCore

struct HarnessDatabaseTests {
    private let directory: URL

    init() throws {
        directory = FileManager.default.temporaryDirectory.appending(path: "HarnessDatabaseTests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    private func open() -> HarnessDatabase {
        HarnessDatabase(directory: directory)
    }

    /// A second look at the file, as the app's next launch would get.
    private func inspect<T>(_ body: (SQLiteConnection) throws -> T) throws -> T {
        try body(SQLiteConnection(url: directory.appending(path: "harness.sqlite")))
    }

    private func count(_ table: String) throws -> Int {
        try inspect { try $0.scalar("SELECT count(*) FROM \(table)")?.int ?? -1 }
    }

    private func json(_ object: Any) throws -> Data {
        try JSONSerialization.data(withJSONObject: object, options: [.fragmentsAllowed, .sortedKeys])
    }

    private func entry(_ id: String, _ kind: EntryKind = .assistant, _ text: String = "") -> EntrySnapshot {
        EntrySnapshot(id: id, kind: kind, text: text, date: Date(timeIntervalSince1970: 1_700_000_000))
    }

    private func state(threads: [String] = ["T1"]) throws -> HarnessDatabase.StateRows {
        HarnessDatabase.StateRows(
            values: ["autoSettleDays": try json(5), "defaults": try json(["model": "haiku"])],
            projects: [.init(id: "P1", json: try json(["id": "P1", "name": "One", "path": "/tmp/one"]))],
            threads: try threads.map { .init(id: $0, projectID: "P1", updatedAt: Date(timeIntervalSince1970: 1_700_000_000),
                                             sessionID: nil, json: try json(["id": $0, "projectID": "P1", "title": $0])) })
    }

    // MARK: - State

    @Test func stateSurvivesAReopen() throws {
        let saved = try state(threads: ["T1", "T2"])
        do {
            let database = open()
            database.saveState(saved)
            database.flush()
        }
        #expect(open().loadState() == saved)
    }

    @Test func stateDropsWhatIsGone() throws {
        let database = open()
        database.saveState(try state(threads: ["T1", "T2"]))
        var smaller = try state(threads: ["T2"])
        smaller.values["autoSettleDays"] = nil
        database.saveState(smaller)
        database.flush()
        #expect(open().loadState() == smaller)
        #expect(try count("threads") == 1)
        #expect(try count("settings") == 1)
    }

    @Test func aSaveIsSeenBeforeItIsWritten() throws {
        let database = open()
        let saved = try state()
        database.saveState(saved)
        #expect(database.loadState() == saved)
    }

    // MARK: - Transcripts

    @Test func transcriptRoundTrip() throws {
        let thread = UUID()
        var user = entry("u1", .user, "Fix the build")
        user.attachments = ["a.png"]
        user.delivery = .steered
        user.turnEndedAt = Date(timeIntervalSince1970: 1_700_000_100)
        var tool = entry("t1", .tool)
        tool.toolName = "Bash"
        tool.toolInput = #"{"command":"swift build"}"#
        tool.toolResult = "Build complete"
        tool.toolIsError = true
        var notice = entry("n1", .notice, "Interrupted")
        notice.noticeStyle = .error
        var agent = entry("a1", .tool)
        agent.toolName = "Agent"
        agent.toolInput = "{}"
        agent.subagent = SubagentSnapshot(description: "Look around", agentType: "Explore", prompt: "Find it", isBackground: true,
                                          status: .completed, toolUses: 3, tokens: 100, durationMs: 2000, report: "Found",
                                          startedAt: Date(timeIntervalSince1970: 1_700_000_050), entries: [entry("s1", .assistant, "Inside")])
        let entries = [user, entry("r1", .assistant, "On it"), tool, notice, agent]
        do {
            let database = open()
            database.saveTranscript(thread, entries: entries)
            database.flush()
        }
        #expect(open().loadTranscript(thread) == entries)
    }

    @Test func unchangedRowsAreNotRewritten() throws {
        let thread = UUID()
        let database = open()
        var entries = [entry("u1", .user, "Hi"), entry("r1", .assistant, "Hel")]
        database.saveTranscript(thread, entries: entries)
        database.flush()
        let before = database.currentRev
        database.saveTranscript(thread, entries: entries)
        database.flush()
        #expect(database.currentRev == before)

        // Streaming: only the row that grew is written.
        entries[1].text = "Hello"
        database.saveTranscript(thread, entries: entries)
        database.flush()
        #expect(database.currentRev == before + 1)
        #expect(try inspect { try $0.scalar("SELECT seq FROM entries WHERE rev = ?", [.int(Int64(before + 1))])?.int } == 1)
    }

    @Test func aFreshDatabaseComparesWithWhatIsOnDisk() throws {
        let thread = UUID()
        let entries = (0..<20).map { entry("e\($0)", .assistant, "Row \($0)") }
        do {
            let database = open()
            database.saveTranscript(thread, entries: entries)
            database.flush()
        }
        // The next launch loads it and saves it unchanged plus one row: one write.
        let database = open()
        let rev = database.currentRev
        database.saveTranscript(thread, entries: database.loadTranscript(thread) + [entry("new", .user, "More")])
        database.flush()
        #expect(database.currentRev == rev + 1)
    }

    @Test func shorterTranscriptDropsTheRest() throws {
        let thread = UUID()
        let database = open()
        database.saveTranscript(thread, entries: (0..<5).map { entry("e\($0)") })
        database.saveTranscript(thread, entries: (0..<3).map { entry("e\($0)") })
        database.flush()
        #expect(open().loadTranscript(thread).map(\.id) == ["e0", "e1", "e2"])
    }

    @Test func savesCollapseIntoTheLatest() throws {
        let thread = UUID()
        let database = open()
        for count in 1...50 { database.saveTranscript(thread, entries: (0..<count).map { entry("e\($0)") }) }
        #expect(database.loadTranscript(thread).count == 50)
        database.flush()
        #expect(open().loadTranscript(thread).count == 50)
    }

    @Test func bigResultsLoadAsTheirHead() throws {
        let thread = UUID()
        let big = String(repeating: "line of output\n", count: 10_000)
        var tool = entry("t1", .tool)
        tool.toolName = "Read"
        tool.toolInput = #"{"file_path":"/a"}"#
        tool.toolResult = big
        let database = open()
        database.saveTranscript(thread, entries: [tool])
        database.flush()

        let loaded = try #require(open().loadTranscript(thread).first)
        #expect(loaded.toolResult == String(big.prefix(HarnessDatabase.resultHeadLength)))
        #expect(loaded.toolResultLength == big.count)
        let ref = try #require(loaded.toolResultRef)
        #expect(database.fullToolResult(ref) == big)
        // Compressed: repetitive output takes a fraction of its size.
        #expect(try inspect { try $0.scalar("SELECT length(data) FROM blobs")?.int ?? .max } < big.utf8.count / 10)

        // Saving the loaded rows back keeps the whole result.
        let reopened = open()
        let rev = reopened.currentRev
        reopened.saveTranscript(thread, entries: [loaded])
        reopened.flush()
        #expect(reopened.currentRev == rev)
        var renamed = loaded
        renamed.text = "changed"
        reopened.saveTranscript(thread, entries: [renamed])
        reopened.flush()
        #expect(reopened.fullToolResult(ref) == big)
        #expect(try count("blobs") == 1)
    }

    @Test func subagentResultsStayWhole() throws {
        let thread = UUID()
        var agent = entry("a1", .tool)
        agent.toolName = "Agent"
        agent.toolResult = String(repeating: "report ", count: 10_000)
        let database = open()
        database.saveTranscript(thread, entries: [agent])
        database.flush()
        let loaded = try #require(open().loadTranscript(thread).first)
        #expect(loaded.toolResult == agent.toolResult)
        #expect(loaded.toolResultRef == nil)
    }

    @Test func bigInputsAndSubagentsRoundTrip() throws {
        let thread = UUID()
        var write = entry("w1", .tool)
        write.toolName = "Write"
        write.toolInput = #"{"content":""# + String(repeating: "x", count: 100_000) + #""}"#
        var agent = entry("a1", .tool)
        agent.toolName = "Agent"
        agent.subagent = SubagentSnapshot(description: "d", agentType: nil, prompt: "p", isBackground: nil, status: .completed,
                                          toolUses: nil, tokens: nil, durationMs: nil, report: nil, startedAt: .now,
                                          entries: (0..<200).map { entry("s\($0)", .assistant, String(repeating: "y", count: 500)) })
        let database = open()
        database.saveTranscript(thread, entries: [write, agent])
        database.flush()
        #expect(open().loadTranscript(thread) == [write, agent])
        #expect(try count("blobs") == 2)
    }

    @Test func replacedBlobsAreCollected() throws {
        let thread = UUID()
        let database = open()
        var tool = entry("w1", .tool)
        tool.toolName = "Write"
        // A big input streaming in: each save stores a longer one.
        for length in [40_000, 60_000, 80_000] {
            tool.toolInput = String(repeating: "z", count: length)
            database.saveTranscript(thread, entries: [tool])
            database.flush()
        }
        #expect(try count("blobs") == 1)
    }

    @Test func deletingATranscriptRemovesEverything() throws {
        let thread = UUID(), other = UUID()
        var tool = entry("t1", .tool)
        tool.toolName = "Read"
        tool.toolResult = String(repeating: "q", count: 50_000)
        let database = open()
        database.saveTranscript(thread, entries: [entry("u1", .user, "searchable"), tool])
        database.saveTranscript(other, entries: [entry("u1", .user, "kept")])
        database.catchUpIndex()
        database.deleteTranscript(thread)
        database.flush()
        #expect(database.loadTranscript(thread).isEmpty)
        #expect(database.loadTranscript(other).count == 1)
        #expect(try count("blobs") == 0)
        #expect(database.search("searchable").isEmpty)
        #expect(database.transcriptSavedAt(thread) == nil)
    }

    @Test func pagedScanSeesEveryRow() throws {
        let thread = UUID()
        let database = open()
        database.saveTranscript(thread, entries: (0..<1234).map { entry("e\($0)") })
        database.flush()
        var pages: [Int] = []
        database.scanTranscript(thread, pageSize: 500) { pages.append($0.count) }
        #expect(pages == [500, 500, 234])
    }

    // MARK: - Change feed and search

    @Test func searchFindsWordsAcrossThreads() throws {
        let a = UUID(), b = UUID()
        let database = open()
        var tool = entry("t1", .tool)
        tool.toolName = "Bash"
        tool.toolInput = #"{"command":"xcodebuild -scheme Harness"}"#
        database.saveTranscript(a, entries: [entry("u1", .user, "The migration is crashing on launch"), tool])
        database.saveTranscript(b, entries: [entry("u1", .user, "Write the release notes"), entry("r1", .assistant, "Migrations done")])
        database.catchUpIndex()

        #expect(Set(database.search("migrat").map(\.threadID)) == [a.uuidString, b.uuidString])
        #expect(database.search("crashing launch").map(\.entryID) == ["u1"])
        #expect(database.search("xcodebuild").first?.kind == .tool)
        #expect(database.search("crashing").first?.snippet.contains("[crashing]") == true)
        #expect(database.search("\"; DROP TABLE entries; --").isEmpty)
        #expect(database.search("   ").isEmpty)
    }

    @Test func searchFollowsEdits() throws {
        let thread = UUID()
        let database = open()
        database.saveTranscript(thread, entries: [entry("r1", .assistant, "draft wording")])
        database.catchUpIndex()
        database.saveTranscript(thread, entries: [entry("r1", .assistant, "final phrasing")])
        database.catchUpIndex()
        #expect(database.search("draft").isEmpty)
        #expect(database.search("phrasing").count == 1)
    }

    @Test func searchKeepsToAProject() throws {
        let database = open()
        var rows = try state(threads: [])
        let one = UUID(), two = UUID()
        rows.threads = [
            .init(id: one.uuidString, projectID: "P1", updatedAt: nil, sessionID: nil, json: try json(["id": one.uuidString])),
            .init(id: two.uuidString, projectID: "P2", updatedAt: nil, sessionID: nil, json: try json(["id": two.uuidString])),
        ]
        database.saveState(rows)
        database.saveTranscript(one, entries: [entry("u1", .user, "shared word")])
        database.saveTranscript(two, entries: [entry("u1", .user, "shared word")])
        database.catchUpIndex()
        #expect(database.search("shared").count == 2)
        #expect(database.search("shared", projectID: "P2").map(\.threadID) == [two.uuidString])
    }

    @Test func changeFeedFollowsABookmark() throws {
        let thread = UUID()
        let database = open()
        database.saveTranscript(thread, entries: [entry("a", .user, "one"), entry("b", .assistant, "two")])
        database.flush()
        let first = database.changes(after: database.bookmark("memory"))
        #expect(first.map(\.entryID) == ["a", "b"])
        database.setBookmark("memory", rev: first.last!.rev)

        database.saveTranscript(thread, entries: [entry("a", .user, "one"), entry("b", .assistant, "two!"), entry("c", .user, "three")])
        database.flush()
        #expect(database.changes(after: database.bookmark("memory")).map(\.entryID) == ["b", "c"])
        // Bookmarks are saved.
        #expect(open().bookmark("memory") == first.last!.rev)
    }

    @Test func matchExpressionQuotesEveryWord() {
        #expect(HarnessDatabase.matchExpression("foo-bar baz") == #""foo" "bar" "baz"*"#)
        #expect(HarnessDatabase.matchExpression("\"") == nil)
    }

    // MARK: - Memories

    @Test func memoriesAreKeptSearchedAndSourced() throws {
        let database = open()
        let day = Date(timeIntervalSince1970: 1_790_000_000)
        let memory = HarnessDatabase.Memory(scope: "project:P1", kind: "preference", title: "Release flow",
                                            body: "Never cut a release without being asked", createdAt: day, updatedAt: day,
                                            sources: [.init(threadID: "T1", entryID: "u1", quote: "no version release please")])
        database.saveMemory(memory)
        database.saveMemory(.init(title: "Editor", body: "Prefers tabs"))

        #expect(database.memories(scope: "project:P1") == [memory])
        #expect(database.memories().count == 2)
        #expect(database.searchMemories("release").map(\.id) == [memory.id])
        #expect(database.searchMemories("tabs", scope: "project:P1").isEmpty)

        var edited = memory
        edited.body = "Releases only on request"
        edited.status = .archived
        database.saveMemory(edited)
        #expect(database.searchMemories("never").isEmpty)
        #expect(database.memories(status: .archived).first?.body == "Releases only on request")

        database.deleteMemory(memory.id)
        #expect(database.memories(scope: "project:P1").isEmpty)
        #expect(try count("memory_sources") == 0)
    }

    // MARK: - Moving in from the JSON files

    private func writeLegacy(threads: [String: [[String: Any]]], extraThreads: [String] = []) throws {
        var threadMetas: [[String: Any]] = []
        for id in threads.keys.sorted() + extraThreads {
            threadMetas.append(["id": id, "projectID": "P1", "title": "Thread \(id.prefix(4))",
                                "createdAt": "2026-10-20T10:00:00Z", "updatedAt": "2026-10-21T10:00:00Z", "sessionID": "s-\(id)"])
        }
        let state: [String: Any] = [
            "projects": [["id": "P1", "name": "One", "path": "/tmp/one", "permissionMode": "auto"]],
            "threads": threadMetas,
            "selectedThreadID": threads.keys.sorted().first ?? "",
            "autoSettleDays": 7,
            "slashCommands": ["P1": [["name": "review"]]],
        ]
        try json(state).write(to: directory.appending(path: "state.json"))
        let folder = directory.appending(path: "threads")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        for (id, entries) in threads {
            try JSONSerialization.data(withJSONObject: entries).write(to: folder.appending(path: "\(id).json"))
        }
    }

    @Test func movesTheOldFilesIn() throws {
        let thread = UUID().uuidString
        try writeLegacy(threads: [thread: [
            ["id": "u1", "kind": "user", "text": "Hello", "date": "2026-10-20T10:00:00Z", "attachments": ["x.png"]],
            ["id": "t1", "kind": "tool", "text": "", "toolName": "Read", "toolInput": "{}", "toolResult": String(repeating: "r", count: 40_000)],
            ["id": "z1", "kind": "somethingNewer", "text": "?"],
            ["id": "a1", "kind": "tool", "text": "", "toolName": "Agent",
             "subagent": ["description": "d", "prompt": "p", "status": "completed", "startedAt": "2026-10-20T10:01:00Z",
                          "entries": [["id": "s1", "kind": "assistant", "text": "inner"]]]],
        ]])
        let stateBefore = try Data(contentsOf: directory.appending(path: "state.json"))
        let legacyFile = directory.appending(path: "threads/\(thread).json")
        let fileDate = try #require(try FileManager.default.attributesOfItem(atPath: legacyFile.path)[.modificationDate] as? Date)

        let database = open()
        let state = database.loadState()
        #expect(state.projects.map(\.id) == ["P1"])
        #expect(state.threads.map(\.id) == [thread])
        #expect(state.threads.first?.sessionID == "s-\(thread)")
        #expect(state.values["autoSettleDays"] == (try json(7)))
        #expect(state.values["slashCommands"] != nil)
        #expect(state.values["projects"] == nil)
        // Before it's moved in, the file's date stands for when it was saved.
        #expect(database.transcriptSavedAt(UUID(uuidString: thread)!) == fileDate)
        func savedAt() -> TimeInterval { database.transcriptSavedAt(UUID(uuidString: thread)!)?.timeIntervalSince(fileDate) ?? .infinity }

        let entries = database.loadTranscript(UUID(uuidString: thread)!)
        // The entry it can't read is the only one lost.
        #expect(entries.map(\.id) == ["u1", "t1", "a1"])
        #expect(entries[0].attachments == ["x.png"])
        #expect(entries[0].date == (try Date.ISO8601FormatStyle().parse("2026-10-20T10:00:00Z")))
        #expect(entries[1].toolResultRef != nil)
        #expect(entries[2].subagent?.entries.first?.text == "inner")
        // And after, too.
        #expect(abs(savedAt()) < 0.001)

        // The files are left exactly as they were.
        #expect(try Data(contentsOf: directory.appending(path: "state.json")) == stateBefore)
        #expect(FileManager.default.fileExists(atPath: legacyFile.path))
    }

    @Test func movesInOnlyOnce() throws {
        let thread = UUID().uuidString
        try writeLegacy(threads: [thread: [["id": "u1", "kind": "user", "text": "Original"]]])
        do {
            let database = open()
            let id = UUID(uuidString: thread)!
            var entries = database.loadTranscript(id)
            entries.append(EntrySnapshot(id: "r1", kind: .assistant, text: "Newer"))
            database.saveTranscript(id, entries: entries)
            var state = database.loadState()
            state.values["autoSettleDays"] = try json(1)
            database.saveState(state)
            database.flush()
        }
        // An old app writing the files again doesn't undo what's in the database.
        try writeLegacy(threads: [thread: [["id": "other", "kind": "user", "text": "Stale"]]])
        let database = open()
        database.importAllLegacyTranscripts()
        #expect(database.loadTranscript(UUID(uuidString: thread)!).map(\.text) == ["Original", "Newer"])
        #expect(database.loadState().values["autoSettleDays"] == (try json(1)))
    }

    @Test func movesTheRestInTheBackground() throws {
        let threads = (0..<5).map { _ in UUID().uuidString }
        try writeLegacy(threads: Dictionary(uniqueKeysWithValues: threads.map { ($0, [["id": "u1", "kind": "user", "text": "Hi \($0)"]]) }))
        // A transcript whose thread is gone stays where it is.
        let orphan = UUID().uuidString
        try JSONSerialization.data(withJSONObject: [["id": "u1", "kind": "user", "text": "orphan"]])
            .write(to: directory.appending(path: "threads/\(orphan).json"))

        let database = open()
        database.importAllLegacyTranscripts()
        #expect(try count("entries") == 5)
        #expect(try count("transcripts") == 5)
        database.catchUpIndex()
        #expect(database.search(threads[2]).map(\.threadID) == [threads[2]])
        #expect(database.loadTranscript(UUID(uuidString: orphan)!).isEmpty)
    }

    @Test func savingBeforeOpeningKeepsTheOldTranscript() throws {
        let thread = UUID().uuidString
        try writeLegacy(threads: [thread: [["id": "u1", "kind": "user", "text": "Old"]]])
        let database = open()
        // Something saves before the transcript was ever read: it's moved in first, so the
        // save extends it rather than replacing it.
        database.saveTranscript(UUID(uuidString: thread)!, entries: [EntrySnapshot(id: "u1", kind: .user, text: "Old"),
                                                                     EntrySnapshot(id: "r1", kind: .assistant, text: "New")])
        database.flush()
        #expect(open().loadTranscript(UUID(uuidString: thread)!).map(\.text) == ["Old", "New"])
    }

    @Test func freshInstallStartsEmpty() throws {
        let database = open()
        #expect(database.loadState() == HarnessDatabase.StateRows())
        // A state written by hand after that (a development run's) is still picked up.
        try writeLegacy(threads: [UUID().uuidString: []])
        #expect(open().loadState().threads.count == 1)
    }

    @Test func anUnreadableDatabaseStartsOverFromTheFiles() throws {
        let thread = UUID().uuidString
        try writeLegacy(threads: [thread: [["id": "u1", "kind": "user", "text": "Recovered"]]])
        try Data("this is not a database, just junk bytes that go on for a while".utf8)
            .write(to: directory.appending(path: "harness.sqlite"))
        let database = open()
        #expect(database.recoveredFrom != nil)
        #expect(database.loadState().threads.map(\.id) == [thread])
        #expect(database.loadTranscript(UUID(uuidString: thread)!).map(\.text) == ["Recovered"])
    }

    @Test func aNewerSchemaStillOpens() throws {
        let thread = UUID()
        do {
            let database = open()
            database.saveTranscript(thread, entries: [entry("u1", .user, "kept")])
            database.flush()
        }
        try inspect { try $0.execute("PRAGMA user_version = 99; CREATE TABLE future (x);") }
        let database = open()
        #expect(database.recoveredFrom == nil)
        #expect(database.loadTranscript(thread).map(\.text) == ["kept"])
    }
}
