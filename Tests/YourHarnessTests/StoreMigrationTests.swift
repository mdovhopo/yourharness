import Foundation
import HarnessCore
import Testing
@testable import YourHarness

/// Upgrading from the JSON files: everything an earlier version saved comes back the same.
struct StoreMigrationTests {
    private let root: URL

    init() throws {
        root = FileManager.default.temporaryDirectory.appending(path: "StoreMigrationTests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root.appending(path: "threads"), withIntermediateDirectories: true)
    }

    /// What versions up to 0.0.6 wrote: `JSONEncoder` with ISO 8601 dates, nothing else set.
    private func writeLegacy(_ state: PersistedState, transcripts: [UUID: [EntrySnapshot]]) throws {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        try encoder.encode(state).write(to: root.appending(path: "state.json"))
        for (id, entries) in transcripts {
            try encoder.encode(entries).write(to: root.appending(path: "threads/\(id.uuidString).json"))
        }
    }

    /// The same value, compared as JSON with its keys sorted.
    private func same(_ a: some Encodable, _ b: some Encodable) throws -> Bool {
        let encoder = JSONEncoder()
        encoder.outputFormatting = .sortedKeys
        encoder.dateEncodingStrategy = .iso8601
        return try encoder.encode(a) == encoder.encode(b)
    }

    private static let day = try! Date.ISO8601FormatStyle().parse("2026-10-20T10:00:00Z")

    private func sampleState() -> (PersistedState, [UUID: [EntrySnapshot]]) {
        let home = Project(name: "Home", path: "/tmp/home")
        let work = Project(name: "Work", path: "/tmp/work", permissionMode: .acceptEdits)
        var pinned = ThreadMeta(id: UUID(), projectID: work.id, title: "Pinned work", createdAt: Self.day, updatedAt: Self.day)
        pinned.sessionID = "8f0c2c4e-0000-4000-8000-000000000001"
        pinned.model = "haiku"
        pinned.isPinned = true
        pinned.draft = "half a thought"
        pinned.footprint = ["/tmp/work/repo"]
        pinned.pullRequests = ["https://github.com/o/r/pull/1"]
        pinned.writtenFootprint = ["/tmp/work/repo"]
        pinned.workedPullRequests = []
        pinned.browserTabs = ["http://localhost:3000/"]
        var settled = ThreadMeta(id: UUID(), projectID: home.id, title: "Old one", createdAt: Self.day, updatedAt: Self.day)
        settled.settledAt = Self.day
        settled.autoSettle = false

        var state = PersistedState()
        state.projects = [home, work]
        state.threads = [pinned, settled]
        state.homeProjectID = home.id
        state.selectedThreadID = pinned.id
        state.defaults = ThreadDefaults(model: "haiku", effort: "low", permissionMode: .plan)
        state.appearance = .dark
        state.autoSettleDays = 7
        state.notifications = false
        state.collapsedProjectIDs = [home.id]
        state.cliPathOverrides = ["claude": "/opt/claude"]
        state.contextWindows = ["claude-haiku": 200_000]
        state.repoUsage = ["/tmp/work/repo": RepoUsage(threads: [pinned.id], lastUsed: Self.day)]
        state.showsInspector = true

        var tool = EntrySnapshot(id: "toolu_1", kind: .tool, text: "", toolName: "Read",
                                 toolInput: #"{"file_path":"/tmp/work/repo/a.swift"}"#,
                                 toolResult: String(repeating: "let x = 1\n", count: 5_000), date: Self.day)
        tool.toolIsError = nil
        let transcript = [
            EntrySnapshot(id: "u1", kind: .user, text: "Read a.swift", attachments: ["\(pinned.id.uuidString)-img.png"], date: Self.day,
                          turnEndedAt: Self.day),
            tool,
            EntrySnapshot(id: "r1", kind: .assistant, text: "It sets x.", date: Self.day),
            EntrySnapshot(id: "n1", kind: .notice, text: "Stopped", noticeStyle: .error),
        ]
        return (state, [pinned.id: transcript, settled.id: [EntrySnapshot(id: "u1", kind: .user, text: "Long ago")]])
    }

    @Test func upgradeKeepsEverything() throws {
        let (state, transcripts) = sampleState()
        try writeLegacy(state, transcripts: transcripts)

        let store = Store(root: root)
        let loaded = store.loadState()
        #expect(try same(loaded.projects, state.projects))
        #expect(try same(loaded.threads, state.threads))
        #expect(loaded.selectedThreadID == state.selectedThreadID)
        #expect(loaded.homeProjectID == state.homeProjectID)
        #expect(loaded.defaults == state.defaults)
        #expect(loaded.appearance == .dark)
        #expect(loaded.autoSettleDays == 7)
        #expect(loaded.notifications == false)
        #expect(loaded.collapsedProjectIDs == state.collapsedProjectIDs)
        #expect(loaded.cliPathOverrides == state.cliPathOverrides)
        #expect(loaded.contextWindows == state.contextWindows)
        #expect(loaded.repoUsage == state.repoUsage)
        #expect(loaded.showsInspector)

        for (id, entries) in transcripts {
            let back = store.loadTranscript(id)
            #expect(back.map(\.id) == entries.map(\.id))
            for (old, new) in zip(entries, back) {
                #expect(new.text == old.text)
                #expect(new.date == old.date)
                #expect(new.attachments == old.attachments)
                #expect(new.noticeStyle == old.noticeStyle)
                #expect(new.turnEndedAt == old.turnEndedAt)
                if new.toolResultRef != nil {
                    #expect(store.database.fullToolResult(new.toolResultRef!) == old.toolResult)
                } else {
                    #expect(new.toolResult == old.toolResult)
                }
            }
        }
    }

    @Test func savedStateComesBackAfterARelaunch() throws {
        let (state, transcripts) = sampleState()
        try writeLegacy(state, transcripts: transcripts)
        do {
            let store = Store(root: root)
            var changed = store.loadState()
            changed.threads[0].title = "Renamed"
            changed.threads.removeLast()
            changed.appearance = .light
            changed.selectedThreadID = nil
            store.saveState(changed)
            store.flush()
        }
        let loaded = Store(root: root).loadState()
        #expect(loaded.threads.map(\.title) == ["Renamed"])
        #expect(loaded.appearance == .light)
        #expect(loaded.selectedThreadID == nil)
        #expect(loaded.autoSettleDays == 7)
    }

    @Test func whatThisVersionCantReadIsKept() throws {
        let (state, transcripts) = sampleState()
        try writeLegacy(state, transcripts: transcripts)
        // A newer version's thread (no title) and a setting it changed the type of.
        var object = try #require(try JSONSerialization.jsonObject(with: Data(contentsOf: root.appending(path: "state.json"))) as? [String: Any])
        var threads = try #require(object["threads"] as? [[String: Any]])
        threads.append(["id": UUID().uuidString, "projectID": UUID().uuidString, "createdAt": "2026-10-20T10:00:00Z",
                        "updatedAt": "2026-10-20T10:00:00Z", "kindOfThread": "future"])
        object["threads"] = threads
        object["autoSettleDays"] = "seven"
        try JSONSerialization.data(withJSONObject: object).write(to: root.appending(path: "state.json"))

        do {
            let store = Store(root: root)
            let loaded = store.loadState()
            #expect(loaded.threads.count == 2)
            // The one setting is lost, not the rest.
            #expect(loaded.autoSettleDays == 3)
            #expect(loaded.appearance == .dark)
            store.saveState(loaded)
            store.flush()
        }
        // Saving didn't drop the thread it couldn't read.
        #expect(Store(root: root).database.loadState().threads.count == 3)
    }

    @Test func freshInstallHasNothing() {
        let loaded = Store(root: root).loadState()
        #expect(loaded.projects.isEmpty)
        #expect(loaded.threads.isEmpty)
        #expect(loaded.autoSettleDays == 3)
    }
}

/// Moves a real data folder in (a copy of it) and checks every thread and transcript came
/// through: `YH_MIGRATION_SOURCE=<data folder> swift test --filter RealDataMigration`.
@Suite(.enabled(if: ProcessInfo.processInfo.environment["YH_MIGRATION_SOURCE"] != nil))
struct RealDataMigrationTests {
    @Test func everythingComesThrough() throws {
        let source = URL(fileURLWithPath: ProcessInfo.processInfo.environment["YH_MIGRATION_SOURCE"]!)
        let copy = FileManager.default.temporaryDirectory.appending(path: "RealDataMigration-\(UUID().uuidString)")
        try FileManager.default.copyItem(at: source, to: copy)
        defer { try? FileManager.default.removeItem(at: copy) }
        try? FileManager.default.removeItem(at: copy.appending(path: "harness.sqlite"))

        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        let legacy = try decoder.decode(PersistedState.self, from: Data(contentsOf: copy.appending(path: "state.json")))

        let start = Date.now
        let store = Store(root: copy)
        let state = store.loadState()
        print("Opened in \(Int(Date.now.timeIntervalSince(start) * 1000)) ms: \(state.projects.count) projects, \(state.threads.count) threads")
        #expect(state.projects.map(\.id) == legacy.projects.map(\.id))
        #expect(state.threads.map(\.id) == legacy.threads.map(\.id))
        #expect(state.threads.map(\.title) == legacy.threads.map(\.title))
        #expect(state.threads.map(\.sessionID) == legacy.threads.map(\.sessionID))

        var rows = 0, heads = 0
        for thread in legacy.threads {
            let file = copy.appending(path: "threads/\(thread.id.uuidString).json")
            guard let data = try? Data(contentsOf: file) else { continue }
            let old = (try? decoder.decode([Lossy<EntrySnapshot>].self, from: data))?.compactMap(\.value) ?? []
            let new = store.loadTranscript(thread.id)
            #expect(new.map(\.id) == old.map(\.id), "thread \(thread.id)")
            #expect(new.map(\.text) == old.map(\.text), "thread \(thread.id)")
            for (a, b) in zip(old, new) {
                let result = b.toolResultRef.flatMap(store.database.fullToolResult) ?? b.toolResult
                #expect(result == a.toolResult, "entry \(a.id)")
                #expect(b.toolInput == a.toolInput, "entry \(a.id)")
                #expect(b.subagent?.entries.count == a.subagent?.entries.count, "entry \(a.id)")
                if b.toolResultRef != nil { heads += 1 }
            }
            rows += new.count
        }
        store.database.catchUpIndex()
        print("Moved \(rows) rows in (\(heads) big results kept as heads); indexed through rev \(store.database.bookmark("fts"))")
        let size = (try? FileManager.default.attributesOfItem(atPath: copy.appending(path: "harness.sqlite").path)[.size] as? Int) ?? 0
        print("Database: \(size / 1024) KB")
    }
}
