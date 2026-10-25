import Foundation
import Testing
@testable import HarnessCore

struct MemoryTests {
    private let directory: URL
    private let scope = "project:P1"

    init() throws {
        directory = FileManager.default.temporaryDirectory.appending(path: "MemoryTests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    private func open() -> HarnessDatabase {
        HarnessDatabase(directory: directory, importLegacy: false)
    }

    private func memory(_ title: String, _ body: String = "Body", kind: MemoryKind = .project, description: String = "") -> HarnessDatabase.Memory {
        HarnessDatabase.Memory(scope: scope, kind: kind.rawValue, title: title, description: description, body: body)
    }

    private func call(_ server: MemoryToolServer, _ tool: String, _ arguments: JSONValue, toolUseID: String? = nil) async -> MCPToolResult {
        await server.call(tool, arguments: arguments, toolUseID: toolUseID)
    }

    private func text(_ result: MCPToolResult) -> String {
        result.content.compactMap { if case .text(let text) = $0 { text } else { nil } }.joined()
    }

    // MARK: - Database

    @Test func aFirstVersionDatabaseKeepsItsMemories() throws {
        let url = directory.appending(path: "harness.sqlite")
        do {
            let connection = try SQLiteConnection(url: url)
            try connection.execute(Schema.steps[0])
            try connection.execute("PRAGMA user_version = 1")
            try connection.run("""
                INSERT INTO memories(id, scope, kind, title, body, status, created_at, updated_at, rev)
                VALUES ('m1', 'project:P1', 'fact', 'Old one', 'From before', 'active', 1, 2, 1)
                """)
        }
        let database = open()
        let old = try #require(database.memory("m1"))
        #expect(old.title == "Old one")
        #expect(old.description == "")
        #expect(old.recallCount == 0)
        #expect(database.findMemories("before", scope: scope).map(\.memory.id) == ["m1"])
    }

    @Test func savesKeepVersionsAndALog() throws {
        let database = open()
        var saved = memory("Release flow", "Only on request", kind: .feedback)
        database.saveMemory(saved, actor: .init("agent", threadID: "T1"))
        saved.body = "Only when asked, and never on Fridays"
        database.saveMemory(saved, actor: .user)
        // Saving it unchanged isn't a new version.
        database.saveMemory(saved, actor: .user)

        let versions = database.memoryVersions(saved.id)
        #expect(versions.map(\.body) == ["Only on request", "Only when asked, and never on Fridays"])
        #expect(versions.map(\.actor) == ["agent", "user"])
        #expect(versions.first?.threadID == "T1")
        #expect(database.memoryEvents(scope: scope).map(\.kind) == [.updated, .created])

        database.restoreMemoryVersion(saved.id, version: 1)
        #expect(database.memory(saved.id)?.body == "Only on request")
        #expect(database.memoryVersions(saved.id).count == 3)
    }

    @Test func linksMergesAndDeletes() throws {
        let database = open()
        let a = memory("Tabs over spaces"), b = memory("Indent with tabs"), c = memory("Lint config")
        database.saveMemories([a, b, c])
        database.linkMemories(c.id, b.id, relation: "see also")
        database.linkMemories(a.id, c.id)
        #expect(database.memory(c.id)?.links == [.init(to: b.id, relation: "see also")])

        let merged = try #require(database.mergeMemories([a.id, b.id], into: memory("Tabs", "Indent with tabs, always")))
        #expect(merged.id == a.id)
        #expect(merged.body == "Indent with tabs, always")
        #expect(merged.links.map(\.to) == [c.id])
        let old = try #require(database.memory(b.id))
        #expect(old.status == .archived)
        #expect(old.mergedInto == a.id)
        // The link into the merged-away memory now points at the merged one.
        #expect(database.memory(c.id)?.links.map(\.to) == [a.id])

        database.deleteMemory(c.id)
        #expect(database.memory(a.id)?.links == [])
        #expect(database.memoryEvents(scope: scope, kinds: [.deleted]).first?.title == "Lint config")
    }

    @Test func recallAndSearchAreCounted() throws {
        let database = open()
        let deploy = memory("Deploys go through Fly", "flyctl deploy from main", kind: .reference)
        database.saveMemory(deploy)
        database.saveMemory(memory("Tests need Docker"))
        let actor = HarnessDatabase.MemoryActor("agent", threadID: "T1")
        let hits = database.findMemories("deploy", scope: scope)
        #expect(hits.map(\.memory.id) == [deploy.id])
        database.noteMemorySearch(scope: scope, query: "deploy", hits: hits.map(\.memory.id), actor: actor)
        database.noteMemorySearch(scope: scope, query: "kubernetes", hits: [], actor: actor)
        database.noteMemoryRecall([deploy.id], actor: actor)
        database.noteMemoryInjection(scope: scope, count: 2, bytes: 900, actor: actor)

        let read = try #require(database.memory(deploy.id))
        #expect(read.recallCount == 1)
        #expect(read.surfacedCount == 1)
        #expect(read.lastRecalledAt != nil)

        let stats = database.memoryStats(scope: scope)
        #expect(stats.active == 2)
        #expect(stats.searches == 2)
        #expect(stats.searchesWithHits == 1)
        #expect(stats.missedQueries == ["kubernetes"])
        #expect(stats.recallsInWindow == 1)
        #expect(stats.recalledInWindow == 1)
        #expect(stats.sessions == 1)
        #expect(stats.averageInjectedBytes == 900)
        #expect(stats.days.count == 30)
        #expect(stats.days.last?.recalled == 1)
        #expect(stats.days.last?.total == 2)
        #expect(stats.byKind == ["reference": 1, "project": 1])
    }

    @Test func searchFindsAnyWordAndPrefersTitles() throws {
        let database = open()
        let inBody = memory("Database", "Postgres runs in Docker for tests")
        let inTitle = memory("Docker setup", "Compose file at the root")
        database.saveMemories([inBody, inTitle])
        #expect(database.findMemories("docker", scope: scope).map(\.memory.id) == [inTitle.id, inBody.id])
        // A question finds what shares any of its words.
        #expect(database.findMemories("how do I run postgres?", scope: scope).map(\.memory.id) == [inBody.id])
        #expect(database.findMemories("docker", scope: "project:other").isEmpty)
    }

    @Test func likelyDuplicatesAndNeverRead() {
        let old = Date.now.addingTimeInterval(-30 * 86_400)
        var a = memory("Release only on request"), b = memory("Releases on request only"), c = memory("Prefers dark mode")
        a.createdAt = old; b.createdAt = old; c.createdAt = old
        c.recallCount = 3
        let stats = MemoryStats.compute(memories: [a, b, c], events: [], now: .now, window: 30)
        #expect(stats.likelyDuplicates.map { $0.map(\.id) } == [[a.id, b.id]])
        #expect(stats.neverRecalled.map(\.id) == [a.id, b.id])
        #expect(stats.topRecalled.map(\.id) == [c.id])
        #expect(stats.ageBuckets.first { $0.label == "1–3 months" }?.count == 3)
    }

    // MARK: - Prompt

    @Test func theIndexStaysInBudget() {
        var memories = (0..<300).map { memory("Memory number \($0)", String(repeating: "x", count: 200), description: "What memory \($0) says") }
        memories[5].pinned = true
        memories[5].body = "Always use the staging cluster"
        memories[7].status = .archived
        let prompt = MemoryPrompt.build(memories: memories, workspace: "acme")
        #expect(prompt.text.contains("Always use the staging cluster"))
        #expect(!prompt.text.contains("Memory number 7 "))
        #expect(prompt.unlisted > 0)
        #expect(prompt.listed + prompt.unlisted == 299)
        #expect(prompt.bytes < MemoryPrompt.indexBudget + MemoryPrompt.pinnedBudget + 4_000)
        #expect(prompt.text.contains("more not listed here"))

        let empty = MemoryPrompt.build(memories: [], workspace: "acme")
        #expect(empty.text.contains("The memory is empty so far."))
    }

    // MARK: - Tools

    @Test func agentsSaveSearchReadAndForget() async throws {
        let database = open()
        let changes = Counter()
        let server = MemoryToolServer(database: database, scope: scope, access: .agent, actor: .init("agent", threadID: "T1"),
                                      onChange: { _ in changes.bump() })
        #expect(server.tools.map(\.name) == ["search", "read", "save", "update", "forget", "link", "track"])

        let saved = await call(server, "save", ["title": "Use pnpm", "description": "Package manager for the web app",
                                                "body": "Run pnpm, never npm.\n**Why:** lockfile.", "kind": "feedback"], toolUseID: "toolu_9")
        #expect(!saved.isError)
        let memory = try #require(database.memories(scope: scope).first)
        #expect(text(saved).contains(memory.shortID))
        #expect(memory.origin == "agent")
        #expect(memory.sources == [.init(threadID: "T1", entryID: "toolu_9")])

        let duplicate = await call(server, "save", ["title": "use PNPM", "description": "", "body": "x", "kind": "feedback"])
        #expect(duplicate.isError)
        let badKind = await call(server, "save", ["title": "Other", "description": "", "body": "x", "kind": "fact"])
        #expect(badKind.isError)
        let secret = await call(server, "save", ["title": "Key", "description": "", "body": "token ghp_abcdefghijklmnopqrstuvwxyz0123456789AB", "kind": "reference"])
        #expect(secret.isError)

        let found = await call(server, "search", ["query": "package manager"])
        #expect(text(found).contains("Use pnpm"))
        let read = await call(server, "read", ["ids": [.string(memory.shortID)]])
        #expect(text(read).contains("never npm"))
        #expect(database.memory(memory.id)?.recallCount == 1)

        let updated = await call(server, "update", ["id": .string(memory.shortID), "body": "Run pnpm 9."])
        #expect(!updated.isError)
        #expect(database.memory(memory.id)?.body == "Run pnpm 9.")
        let forgotten = await call(server, "forget", ["id": .string(memory.shortID), "reason": "moved to bun"])
        #expect(!forgotten.isError)
        #expect(database.memory(memory.id)?.status == .archived)
        #expect(text(await call(server, "search", ["query": "pnpm"])).contains("No memory"))
        #expect(changes.value >= 5)
    }

    @Test func specialistsListMergeAndPin() async throws {
        let database = open()
        let a = memory("Release only on request", kind: .feedback), b = memory("Releases on request only", kind: .feedback)
        database.saveMemories([a, b])
        let server = MemoryToolServer(database: database, scope: scope, access: .specialist, actor: .init("reconciler"))
        #expect(server.tools.contains { $0.name == "merge" })
        #expect(text(await call(server, "stats", [:])).contains("Possible duplicates"))
        #expect(text(await call(server, "list", [:])).contains("2 memories"))
        let merged = await call(server, "merge", ["ids": [.string(a.shortID), .string(b.shortID)], "title": "Release only when asked",
                                                  "description": "Never cut a release unasked", "body": "Ask first.", "kind": "feedback"])
        #expect(!merged.isError)
        #expect(database.memories(scope: scope, status: .active).map(\.title) == ["Release only when asked"])
        #expect(!(await call(server, "pin", ["id": .string(a.shortID), "pinned": true])).isError)
        #expect(database.memory(a.id)?.pinned == true)
        let readOnly = MemoryToolServer(database: database, scope: scope, access: .readOnly, actor: .init("agent"))
        #expect(readOnly.tools.map(\.name) == ["search", "read"])
    }

    @Test func toolsStayInTheirWorkspace() async {
        let database = open()
        let elsewhere = HarnessDatabase.Memory(scope: "project:P2", title: "Secret plan", body: "Elsewhere")
        database.saveMemory(elsewhere)
        let server = MemoryToolServer(database: database, scope: scope, access: .agent, actor: .init("agent"))
        #expect((await call(server, "read", ["ids": [.string(elsewhere.shortID)]])).isError)
        #expect(text(await call(server, "search", ["query": "plan"])).contains("No memory"))
    }

    // MARK: - Tasks

    @Test func tasksTrackProgressAcrossSessions() async throws {
        let database = open()
        let first = MemoryToolServer(database: database, scope: scope, access: .agent, actor: .init("agent", threadID: "T1"))
        let started = await call(first, "track", ["title": "Move billing to Stripe Checkout", "description": "Replace the custom form",
                                                  "steps": [["text": "Spike Checkout"], ["text": "Webhooks"], ["text": "Remove old form"]]],
                                 toolUseID: "toolu_1")
        #expect(!started.isError)
        #expect(text(started).contains("0 of 3 steps done. Next: Spike Checkout"))
        let task = try #require(database.memories(scope: scope).first)
        #expect(task.isTask)
        #expect(task.taskState == .active)

        // Another thread picks it up by title and ticks a step.
        let second = MemoryToolServer(database: database, scope: scope, access: .agent, actor: .init("agent", threadID: "T2"))
        let ticked = await call(second, "track", ["title": "move billing to stripe checkout", "note": "Checkout session works in test mode",
                                                  "steps": [["text": "Spike Checkout", "done": true], ["text": "Webhooks"], ["text": "Remove old form"]]])
        #expect(text(ticked).contains("1 of 3 steps done. Next: Webhooks"))
        let updated = try #require(database.memory(task.id))
        #expect(database.memories(scope: scope).count == 1)
        #expect(updated.stepsDone == 1)
        #expect(updated.body.contains("Checkout session works in test mode"))
        #expect(Set(updated.sources.map(\.threadID)) == ["T1", "T2"])
        #expect(database.tasks(forThread: "T2").map(\.id) == [task.id])
        #expect(database.memoryEvents(scope: scope).first?.kind == .progressed)
        #expect(database.memoryEvents(scope: scope).first?.detail == "active 1/3")

        let blocked = await call(second, "track", ["id": .string(task.shortID), "state": "blocked", "note": "Needs the Stripe key"])
        #expect(!blocked.isError)
        #expect(database.memory(task.id)?.taskState == .blocked)
        #expect((await call(second, "save", ["title": "x", "description": "", "body": "y", "kind": "task"])).isError)
        #expect(text(await call(second, "read", ["ids": [.string(task.shortID)]])).contains("- [x] Spike Checkout"))

        // Restoring an old version brings its progress back.
        database.restoreMemoryVersion(task.id, version: 1)
        #expect(database.memory(task.id)?.stepsDone == 0)
        #expect(database.memory(task.id)?.taskState == .active)

        let stats = database.memoryStats(scope: scope)
        #expect(stats.tasks == ["active": 1])
    }

    @Test func theIndexLeadsWithOpenTasks() {
        var doing = memory("Ship dark mode")
        doing.kind = MemoryKind.task.rawValue
        doing.taskState = .active
        doing.steps = [.init("Colors", done: true), .init("Settings toggle")]
        var finished = memory("Fix login")
        finished.kind = MemoryKind.task.rawValue
        finished.taskState = .done
        var stale = finished
        stale.id = UUID().uuidString
        stale.title = "Old cleanup"
        stale.updatedAt = .now.addingTimeInterval(-30 * 86_400)
        let prompt = MemoryPrompt.build(memories: [doing, finished, stale, memory("Uses pnpm")], workspace: "acme")
        #expect(prompt.text.contains("In Progress:\n- \(doing.shortID) Ship dark mode — 1 of 2 steps done; next: Settings toggle"))
        #expect(prompt.text.contains("Done lately:\n- \(finished.shortID) Fix login (done"))
        #expect(!prompt.text.contains("Old cleanup"))
        #expect(prompt.text.range(of: "## Tasks")!.lowerBound < prompt.text.range(of: "## Memory index")!.lowerBound)
    }

    @Test func tasksSurviveExportAndImport() throws {
        var task = memory("Ship dark mode")
        task.kind = MemoryKind.task.rawValue
        task.taskState = .blocked
        task.steps = [.init("Colors", done: true), .init("Settings toggle")]
        let json = try MemoryArchive.json([task], workspace: "acme", path: nil)
        #expect(try MemoryArchive.memories(fromJSON: json, scope: scope).first?.steps == task.steps)
        let back = MemoryArchive.memories(fromMarkdown: MemoryArchive.markdownFiles([task], workspace: "acme"), scope: scope).first
        #expect(back?.taskState == .blocked)
        #expect(back?.steps == task.steps)
        #expect(back?.body == task.body)
    }

    // MARK: - Files

    @Test func jsonRoundTrips() throws {
        let database = open()
        var a = memory("One", "First", kind: .user, description: "the first")
        let b = memory("Two", "Second")
        a.links = [.init(to: b.id, relation: "see also")]
        a.pinned = true
        database.saveMemories([a, b])
        let data = try MemoryArchive.json(database.memories(scope: scope), workspace: "acme", path: "/tmp/acme")

        let back = try MemoryArchive.memories(fromJSON: data, scope: "project:P2")
        #expect(Set(back.map(\.title)) == ["One", "Two"])
        #expect(back.first { $0.title == "One" }?.links == a.links)
        #expect(back.first { $0.title == "One" }?.pinned == true)
        #expect(throws: MemoryArchive.ReadError.notAnExport) { try MemoryArchive.memories(fromJSON: Data("{}".utf8), scope: scope) }

        // Into another workspace: the IDs are taken, so they're new, and links follow.
        let report = MemoryArchive.merge(back, into: "project:P2", database: database, actor: .user)
        #expect(report.added == 2)
        let copied = database.memories(scope: "project:P2")
        let one = try #require(copied.first { $0.title == "One" })
        #expect(one.id != a.id)
        #expect(one.links.map(\.to) == [copied.first { $0.title == "Two" }!.id])
        // Again: nothing new.
        #expect(MemoryArchive.merge(back, into: "project:P2", database: database, actor: .user).unchanged == 2)
    }

    @Test func markdownRoundTripsAndReadsClaudeCode() throws {
        var a = memory("Push after features", "Commit and push to main.\n**Why:** asked.", kind: .feedback, description: "Push to main, no asking")
        let b = memory("Vision: native app", "Swift and SwiftUI.")
        a.links = [.init(to: b.id)]
        let files = MemoryArchive.markdownFiles([a, b], workspace: "acme")
        #expect(files["MEMORY.md"]?.contains("[Push after features](push-after-features.md)") == true)
        let back = MemoryArchive.memories(fromMarkdown: files, scope: scope)
        let pushed = try #require(back.first { $0.id == a.id })
        #expect(pushed.title == "Push after features")
        #expect(pushed.kind == "feedback")
        #expect(pushed.body == a.body)
        #expect(pushed.description == a.description)
        #expect(pushed.links.map(\.to) == [b.id])

        // As Claude Code writes them.
        let claude = [
            "MEMORY.md": "- [Never touch mcpont](never-touch.md) — never read it\n",
            "never-touch.md": "---\nname: never-touch\ndescription: \"Never read, inspect or reference ~/dev/x\"\nmetadata:\n  type: feedback\n---\n\nDon't. See [[vision]].\n",
            "vision.md": "---\nname: vision\ndescription: The vision\nmetadata:\n  node_type: memory\n  type: project\n---\n\nNative.\n",
            "notes.md": "Just notes, no frontmatter.\n",
        ]
        let imported = MemoryArchive.memories(fromMarkdown: claude, scope: scope)
        #expect(imported.count == 2)
        let never = try #require(imported.first { $0.kind == "feedback" })
        #expect(never.title == "Never touch mcpont")
        #expect(never.description == "Never read, inspect or reference ~/dev/x")
        #expect(never.links.map(\.to) == [imported.first { $0.kind == "project" }!.id])
        #expect(imported.first { $0.kind == "project" }?.title == "Vision")
    }

    @Test func claudeCodeMemoryTurnsOffPerFolder() throws {
        let config = directory.appending(path: "config"), project = directory.appending(path: "project")
        try FileManager.default.createDirectory(at: project.appending(path: ".git/info"), withIntermediateDirectories: true)
        #expect(ClaudeCodeMemory.setting(for: project.path, configDirectory: config).isOff == false)

        // Other settings in the file stay.
        let local = ClaudeCodeMemory.localSettings(for: project.path)
        try FileManager.default.createDirectory(at: local.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(#"{"permissions":{"allow":["Bash(ls)"]}}"#.utf8).write(to: local)
        try ClaudeCodeMemory.turnOff(in: local)
        let written = try JSONSerialization.jsonObject(with: Data(contentsOf: local)) as? [String: Any]
        #expect(written?["autoMemoryEnabled"] as? Bool == false)
        #expect(written?["permissions"] != nil)
        #expect(ClaudeCodeMemory.setting(for: project.path, configDirectory: config).offHere)

        ClaudeCodeMemory.excludeFromGit(project.path)
        ClaudeCodeMemory.excludeFromGit(project.path)
        #expect(try String(contentsOf: project.appending(path: ".git/info/exclude"), encoding: .utf8) == ".claude/settings.local.json\n")

        // Off for you everywhere, unless a folder turns it back on.
        let elsewhere = directory.appending(path: "elsewhere")
        try ClaudeCodeMemory.turnOff(in: config.appending(path: "settings.json"))
        #expect(ClaudeCodeMemory.setting(for: elsewhere.path, configDirectory: config).isOff)
        try FileManager.default.createDirectory(at: elsewhere.appending(path: ".claude"), withIntermediateDirectories: true)
        try Data(#"{"autoMemoryEnabled":true}"#.utf8).write(to: elsewhere.appending(path: ".claude/settings.json"))
        #expect(!ClaudeCodeMemory.setting(for: elsewhere.path, configDirectory: config).isOff)

        // A file that isn't JSON is left alone.
        try Data("not json".utf8).write(to: local)
        #expect(throws: ClaudeCodeMemory.WriteError.self) { try ClaudeCodeMemory.turnOff(in: local) }
        #expect(try String(contentsOf: local, encoding: .utf8) == "not json")
    }

    @Test func claudeCodeMemoryNotYetImported() throws {
        let config = directory.appending(path: "config"), folder = directory.appending(path: "work").path
        let memoryFolder = ClaudeCodeMemory.directory(for: folder, configDirectory: config)
        try FileManager.default.createDirectory(at: memoryFolder, withIntermediateDirectories: true)
        let file = { (name: String, body: String) in
            "---\nname: \(name)\ndescription: d\nmetadata:\n  type: feedback\n---\n\n\(body)\n"
        }
        try file("short-answers", "Keep it short.").write(to: memoryFolder.appending(path: "short-answers.md"), atomically: true, encoding: .utf8)
        try file("deploys", "Fly from main.").write(to: memoryFolder.appending(path: "deploys.md"), atomically: true, encoding: .utf8)
        let have = [memory("Short answers", "Keep it short.")]
        #expect(ClaudeCodeMemory.unimported(for: folder, configDirectory: config, memories: have, since: nil).map(\.lastPathComponent) == ["deploys.md"])
        #expect(ClaudeCodeMemory.unimported(for: folder, configDirectory: config, memories: have, since: .now.addingTimeInterval(60)).isEmpty)
        // Titled by the index, as an import titles it.
        try "- [Deploys go through Fly](deploys.md) — x\n".write(to: memoryFolder.appending(path: "MEMORY.md"), atomically: true, encoding: .utf8)
        var archived = memory("Deploys go through Fly", "Rewritten since")
        archived.status = .archived
        #expect(ClaudeCodeMemory.unimported(for: folder, configDirectory: config, memories: have + [archived], since: nil).isEmpty)
        // An import doesn't bring back one you archived.
        let database = open()
        database.saveMemory(archived)
        let report = MemoryArchive.merge([memory("Deploys go through Fly", "Fly from main.")], into: scope, database: database, actor: .user)
        #expect(report.unchanged == 1)
        #expect(database.memories(scope: scope).count == 1)
    }

    @Test func claudeCodeNamesFoldersAfterTheirRealPath() throws {
        let folder = URL(fileURLWithPath: "/tmp").appending(path: "yh-memory-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let name = ClaudeCodeMemory.directory(for: folder.path, configDirectory: URL(fileURLWithPath: "/c")).deletingLastPathComponent().lastPathComponent
        #expect(name.hasPrefix("-private-tmp-yh-memory-"))
    }

    @Test func claudeCodeNamesFoldersAfterTheirPath() {
        let config = URL(fileURLWithPath: "/Users/me/.claude")
        #expect(ClaudeCodeMemory.directory(for: "/Users/me/dev/tricky_lake.app", configDirectory: config).path
                == "/Users/me/.claude/projects/-Users-me-dev-tricky-lake-app/memory")
    }
}

private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    var value: Int { lock.withLock { count } }
    func bump() { lock.withLock { count += 1 } }
}
