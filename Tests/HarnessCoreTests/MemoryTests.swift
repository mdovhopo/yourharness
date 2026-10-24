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
        #expect(server.tools.map(\.name) == ["search", "read", "save", "update", "forget", "link"])

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
