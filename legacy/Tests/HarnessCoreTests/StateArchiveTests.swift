import Foundation
import Testing
@testable import HarnessCore

/// Exporting the whole state to one file and importing it back over another.
struct StateArchiveTests {
    private let scratch: URL

    init() throws {
        scratch = FileManager.default.temporaryDirectory.appending(path: "StateArchiveTests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
    }

    @Test func importBringsBackExactlyWhatWasExported() throws {
        let source = try folder("source", project: "Mine", thread: "T1", says: "Why is it slow?", memory: "Uses Haiku in tests")
        try write("one image", to: source.appending(path: "attachments/T1-a.png"))
        let file = scratch.appending(path: "state.yhstate")
        let cookies = ["P1": [ImportedCookie(domain: ".example.com", name: "session", value: "abc", isSecure: true)]]
        let defaults = try PropertyListSerialization.data(fromPropertyList: ["NSWindow Frame main": "0 0 800 600"], format: .binary, options: 0)
        do {
            let database = HarnessDatabase(directory: source)
            let written = try StateArchive.export(database, dataFolder: source, extras: .init(defaults: defaults, cookies: cookies),
                                                  manifest: manifest(), to: file)
            #expect(written.contents.projects == ["Mine"])
            #expect(written.contents.threads == 1 && written.contents.entries == 1 && written.contents.memories == 1)
            #expect(written.contents.attachments == 1 && written.contents.browserCookies == 1 && written.contents.hasDefaults)
            #expect(written.schemaVersion == Schema.steps.count)
        }

        // Another state, with things in its folder that aren't the app's.
        let target = try folder("target", project: "Theirs", thread: "T9", says: "Something else", memory: "Another memory")
        try write("theirs", to: target.appending(path: "attachments/T9-b.png"))
        try write("work in progress", to: target.appending(path: "worktrees/repo/branch/file.swift"))
        try write("log", to: target.appending(path: "dev.log"))

        let opened = try StateArchive.open(file)
        #expect(opened.manifest.app.version == "1.2.3")
        #expect(opened.extras == .init(defaults: defaults, cookies: cookies))
        try StateArchive.stage(opened, in: target)
        let applied = try #require(StateArchive.applyPending(in: target))
        #expect(applied.manifest.contents.projects == ["Mine"])
        #expect(applied.extras.cookies == cookies)
        applied.finish()

        let database = HarnessDatabase(directory: target)
        #expect(database.loadState() == HarnessDatabase(directory: source).loadState())
        #expect(database.loadTranscript(threadID).map(\.text) == ["Why is it slow?"])
        #expect(database.memories().map(\.title) == ["Uses Haiku in tests"])
        let attachments = try FileManager.default.contentsOfDirectory(atPath: target.appending(path: "attachments").path)
        #expect(attachments == ["T1-a.png"])
        // Left alone: a worktree with work in it, and a log.
        #expect(FileManager.default.fileExists(atPath: target.appending(path: "worktrees/repo/branch/file.swift").path))
        #expect(FileManager.default.fileExists(atPath: target.appending(path: "dev.log").path))
        #expect(!FileManager.default.fileExists(atPath: target.appending(path: StateArchive.pendingFolderName).path))
        #expect(StateArchive.applyPending(in: target) == nil)
    }

    @Test func usageTravelsWithTheStateAndReplacesTheTargets() throws {
        let now = Date.now
        func usage(_ key: Int64, _ model: String) -> MessageUsage {
            MessageUsage(key: key, date: now.addingTimeInterval(-60), model: model, tokens: TokenCounts(input: 100, output: 10))
        }
        let source = try folder("source", project: "Mine", thread: "T1", says: "Hi", memory: "Mine")
        let file = scratch.appending(path: "state.yhstate")
        do {
            // Still open, with what it wrote in its WAL: the snapshot has it all the same.
            let store = UsageStore(directory: source)
            store.record([usage(1, "claude-opus-5-5"), usage(2, "claude-opus-5-5")])
            try StateArchive.export(HarnessDatabase(directory: source), usage: store, dataFolder: source, extras: .init(),
                                    manifest: manifest(), to: file)
        }
        let target = try folder("target", project: "Theirs", thread: "T9", says: "Else", memory: "Theirs")
        do {
            let store = UsageStore(directory: target)
            store.record([usage(3, "claude-haiku-5-5")])
            #expect(FileManager.default.fileExists(atPath: target.appending(path: "usage.sqlite-wal").path))
        }
        try StateArchive.stage(try StateArchive.open(file), in: target)
        try #require(StateArchive.applyPending(in: target)).finish()

        let report = UsageStore(directory: target).report(days: 1, now: now)
        #expect(report.total.messages == 2)
        #expect(report.models.map(\.model) == ["claude-opus-5-5"])
    }

    @Test func aSwapCutOffPartwayFinishesAtTheNextLaunch() throws {
        let source = try folder("source", project: "Mine", thread: "T1", says: "Imported", memory: "Imported memory")
        try write("image", to: source.appending(path: "attachments/T1-a.png"))
        let file = scratch.appending(path: "state.yhstate")
        do { try StateArchive.export(HarnessDatabase(directory: source), dataFolder: source, extras: .init(), manifest: manifest(), to: file) }
        let target = try folder("target", project: "Theirs", thread: "T9", says: "Old", memory: "Old memory")
        try StateArchive.stage(try StateArchive.open(file), in: target)

        // As if the app died after moving the old state aside and the new database in.
        let pending = target.appending(path: StateArchive.pendingFolderName)
        let replaced = pending.appending(path: "replaced")
        try FileManager.default.createDirectory(at: replaced, withIntermediateDirectories: true)
        for name in try FileManager.default.contentsOfDirectory(atPath: target.path) where name != StateArchive.pendingFolderName {
            try FileManager.default.moveItem(at: target.appending(path: name), to: replaced.appending(path: name))
        }
        try Data().write(to: pending.appending(path: "cleared"))
        try FileManager.default.moveItem(at: pending.appending(path: "data/harness.sqlite"), to: target.appending(path: "harness.sqlite"))

        let applied = try #require(StateArchive.applyPending(in: target))
        applied.finish()
        let database = HarnessDatabase(directory: target)
        #expect(database.loadTranscript(threadID).map(\.text) == ["Imported"])
        #expect(database.memories().map(\.title) == ["Imported memory"])
        #expect(FileManager.default.fileExists(atPath: target.appending(path: "attachments/T1-a.png").path))
    }

    @Test func refusesWhatItCantImport() throws {
        let text = scratch.appending(path: "notes.yhstate")
        try write("not a zip", to: text)
        #expect(throws: StateArchive.Problem.self) { try StateArchive.open(text) }

        // A newer app's format: refused, not half-imported.
        let newer = scratch.appending(path: "newer")
        try write(#"{"format": "yourharness-state", "formatVersion": 99}"#, to: newer.appending(path: "manifest.json"))
        let zip = scratch.appending(path: "newer.yhstate")
        let ditto = try Process.run(URL(fileURLWithPath: "/usr/bin/ditto"), arguments: ["-c", "-k", newer.path, zip.path])
        ditto.waitUntilExit()
        do {
            _ = try StateArchive.open(zip)
            Issue.record("opened a newer format")
        } catch let problem as StateArchive.Problem {
            #expect(problem.description.contains("newer YourHarness"))
        }
    }

    // MARK: - Helpers

    private let threadID = UUID()

    /// A data folder with one project, one thread that said `says`, and one memory.
    private func folder(_ name: String, project: String, thread: String, says text: String, memory: String) throws -> URL {
        let folder = scratch.appending(path: name, directoryHint: .isDirectory)
        let database = HarnessDatabase(directory: folder)
        let json = { (object: Any) in try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]) }
        database.saveState(.init(values: ["autoSettleDays": try json(["days": 5])],
                                 projects: [.init(id: "P1", json: try json(["id": "P1", "name": project, "path": "/tmp/\(name)"]))],
                                 threads: [.init(id: threadID.uuidString, projectID: "P1", updatedAt: Date(timeIntervalSince1970: 1_700_000_000),
                                                 sessionID: nil, json: try json(["id": threadID.uuidString, "title": thread]))]))
        database.saveTranscript(threadID, entries: [EntrySnapshot(id: "1", kind: .user, text: text, date: Date(timeIntervalSince1970: 1_700_000_000))])
        database.saveMemory(HarnessDatabase.Memory(scope: "project:P1", kind: "project", title: memory, description: "", body: memory))
        database.flush()
        return folder
    }

    private func manifest() -> StateArchive.Manifest {
        StateArchive.Manifest(app: .init(name: "YourHarness", bundleID: "test", version: "1.2.3", build: "7", commit: "abc1234"),
                              system: .current, tools: [.init(name: "claude", path: "/bin/claude", version: "2.0.0")])
    }

    private func write(_ text: String, to url: URL) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(text.utf8).write(to: url)
    }
}
