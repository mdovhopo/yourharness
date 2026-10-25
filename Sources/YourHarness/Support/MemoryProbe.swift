import Foundation
import HarnessCore

/// Development: drives memory at launch, for checking it without clicking.
/// - `YH_OPEN_MEMORY=1` shows the selected project's memory, on `YH_MEMORY_TAB` (memories,
///   insights, activity, graph) if set.
/// - `YH_MEMORY_SEED=<n>` adds n made-up memories with links, reads and searches, for
///   looking at the views and their speed at scale.
/// - `YH_MEMORY_IMPORT=<file or folder>` imports an export or Markdown memory files.
/// - `YH_MEMORY_EXPORT=<folder>` exports the memory there, as `memory.json` and `markdown/`.
/// - `YH_MEMORY_AGENT=importer|reconciler` (RootView) opens a specialist's window;
///   `YH_MEMORY_AGENT_START=1` sets it going (the importer on `YH_MEMORY_IMPORT_SOURCES`,
///   paths split by `:`), and `YH_MEMORY_AGENT_MODEL` picks its model.
@MainActor
enum MemoryProbe {
    static func runIfRequested(app: AppModel) {
        let environment = ProcessInfo.processInfo.environment
        let project = app.selectedProject
        let scope = app.memoryScope(for: project.id)
        if let count = environment["YH_MEMORY_SEED"].flatMap(Int.init), count > 0 {
            let started = Date.now
            seed(count, scope: scope, database: app.store.database)
            FileHandle.standardError.write(Data("memory: seeded \(count) in \(Int(Date.now.timeIntervalSince(started) * 1000)) ms\n".utf8))
            app.memoryChanged(scope)
        }
        if let path = environment["YH_MEMORY_IMPORT"], !path.isEmpty {
            app.importMemoryFiles([URL(fileURLWithPath: path)], into: project)
            let count = app.store.database.memories(scope: scope).count
            FileHandle.standardError.write(Data("memory: \(count) memories in \(project.name) after import\n".utf8))
        }
        if let path = environment["YH_MEMORY_EXPORT"], !path.isEmpty {
            let folder = URL(fileURLWithPath: path), memories = app.store.database.memories(scope: scope)
            do {
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                try app.writeMemoryExport(memories, of: project, as: .json, to: folder.appending(path: "memory.json"))
                try app.writeMemoryExport(memories, of: project, as: .markdown, to: folder.appending(path: "markdown"))
                FileHandle.standardError.write(Data("memory: exported \(memories.count) to \(path)\n".utf8))
            } catch {
                FileHandle.standardError.write(Data("memory: export failed: \(error)\n".utf8))
            }
        }
        if environment["YH_OPEN_MEMORY"] == "1" {
            app.showMemory(of: project)
            if let tab = environment["YH_MEMORY_TAB"].flatMap(MemoryLibrary.Tab.init(rawValue:)) {
                app.memoryLibrary(for: project).tab = tab
            }
        }
        if let role = environment["YH_MEMORY_AGENT"].flatMap(MemoryRole.init(rawValue:)) {
            let agent = app.memoryAgent(role, projectID: project.id)
            if let model = environment["YH_MEMORY_AGENT_MODEL"], !model.isEmpty { agent.setModel(model) }
            if environment["YH_MEMORY_AGENT_START"] == "1" {
                switch role {
                case .importer:
                    let sources = (environment["YH_MEMORY_IMPORT_SOURCES"] ?? "").split(separator: ":").map(String.init)
                    app.startMemoryImport(projectID: project.id, sources: sources, note: nil)
                case .reconciler:
                    app.startMemoryReview(projectID: project.id, note: environment["YH_MEMORY_AGENT_NOTE"])
                }
            }
        }
    }

    private static func seed(_ count: Int, scope: String, database: HarnessDatabase) {
        let topics = ["deploy", "release", "tests", "database", "auth", "billing", "design", "API", "CI", "docs", "iOS", "search"]
        let kinds = MemoryKind.allCases
        var memories: [HarnessDatabase.Memory] = []
        for index in 0..<count {
            let topic = topics[index % topics.count]
            let age = Double((index * 7919) % 90) * 86_400
            var memory = HarnessDatabase.Memory(
                scope: scope, kind: kinds[(index / 3) % kinds.count].rawValue,
                title: "\(topic.capitalized) note \(index / topics.count + 1)",
                description: "What to know about \(topic) (\(index))",
                body: "A made-up memory about \(topic), number \(index).\n**Why:** seeded for testing.\n**How to apply:** ignore it.",
                origin: index % 5 == 0 ? "user" : "agent",
                createdAt: .now.addingTimeInterval(-age), updatedAt: .now.addingTimeInterval(-age / 2),
                recallCount: (index * 31) % 7 == 0 ? 0 : (index * 13) % 9)
            if index > 0, index % 3 != 0 { memory.links = [.init(to: memories[(index * 17) % index].id)] }
            memories.append(memory)
        }
        for (index, state) in [HarnessDatabase.TaskState.active, .blocked, .todo, .done].enumerated() {
            memories.append(HarnessDatabase.Memory(
                scope: scope, kind: MemoryKind.task.rawValue, title: ["Ship dark mode", "Move billing to Stripe", "Write API docs", "Fix login loop"][index],
                description: "A made-up task", body: "", origin: "agent", taskState: state,
                steps: [.init("Plan it", done: true), .init("Build it", done: index != 2 && index != 0), .init("Test it", done: index == 3)]))
        }
        database.saveMemories(memories, actor: .app)
        let actor = HarnessDatabase.MemoryActor("agent")
        for index in stride(from: 0, to: count, by: 4) {
            database.noteMemoryRecall([memories[index].id], actor: actor)
            database.noteMemorySearch(scope: scope, query: topics[index % topics.count], hits: [memories[index].id], actor: actor)
        }
        database.noteMemorySearch(scope: scope, query: "kubernetes staging", hits: [], actor: actor)
        database.noteMemoryInjection(scope: scope, count: min(count, 120), bytes: 6_400, actor: actor)
    }
}
