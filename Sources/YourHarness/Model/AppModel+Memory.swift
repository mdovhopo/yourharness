import AppKit
import HarnessCore
import SwiftUI
import UniformTypeIdentifiers

/// Memory: each project (workspace) has its own, kept in the database, given to its threads'
/// agents as an index in the prompt and as `mcp__memory__*` tools, explored in the Memory
/// view, and tended by two specialists, the importer and the reconciler.
extension AppModel {
    func memoryScope(for projectID: UUID) -> String {
        HarnessDatabase.memoryScope(project: projectID.uuidString)
    }

    /// Something changed `scope`'s memory (an agent, an import, an edit): what shows it reloads.
    func memoryChanged(_ scope: String) {
        memoryRevisions[scope, default: 0] += 1
    }

    func memoryRevision(for projectID: UUID) -> Int {
        memoryRevisions[memoryScope(for: projectID)] ?? 0
    }

    func memoryLibrary(for project: Project) -> MemoryLibrary {
        if let library = memoryLibraries[project.id] { return library }
        let library = MemoryLibrary(projectID: project.id, scope: memoryScope(for: project.id), database: store.database)
        memoryLibraries[project.id] = library
        return library
    }

    // MARK: - Settings

    /// The memory and its instructions are part of the launch: running sessions restart.
    func setMemoryEnabled(_ enabled: Bool) {
        memoryEnabled = enabled
        save()
        for thread in threads { thread.restartSession() }
    }

    func setClaudeCodeMemoryOff(_ off: Bool) {
        claudeCodeMemoryOff = off
        save()
        for thread in threads { thread.restartSession() }
    }

    // MARK: - Agents

    func memoryServer(for thread: ThreadModel, access: MemoryToolServer.Access, actor: HarnessDatabase.MemoryActor) -> MemoryToolServer {
        MemoryToolServer(database: store.database, scope: memoryScope(for: thread.projectID), access: access, actor: actor) { scope in
            Task { @MainActor in AppModel.shared.memoryChanged(scope) }
        }
    }

    /// What a thread's session is told about memory: the instructions and the index. Each
    /// launch is counted, with how much it carried.
    func memoryPrompt(for project: Project, thread: ThreadModel) async -> MemoryPrompt {
        let scope = memoryScope(for: project.id), database = store.database, name = project.name
        let actor = HarnessDatabase.MemoryActor("agent", threadID: thread.id.uuidString)
        let prompt = await Task.detached(priority: .userInitiated) {
            let prompt = MemoryPrompt.build(memories: database.memories(scope: scope, status: .active), workspace: name)
            database.noteMemoryInjection(scope: scope, count: prompt.listed, bytes: prompt.bytes, actor: actor)
            return prompt
        }.value
        memoryChanged(scope)
        return prompt
    }

    /// The importer and the reconciler read and ask, and change only memory: no edits, no
    /// commands, no MCP servers but memory's, nothing saved to Claude Code's history.
    func memorySpecialistOptions(_ options: ClaudeLaunchOptions, role: MemoryRole, thread: ThreadModel, project: Project) -> ClaudeLaunchOptions {
        var options = options
        options.permissionMode = "default"
        options.tools = ["Read", "Glob", "Grep", "AskUserQuestion"]
        options.allowedTools = ["mcp__memory", "Read", "Glob", "Grep"]
        options.withoutMCP = true
        options.persistsSession = false
        options.addDirectories = thread.extraDirectories
        options.appendSystemPrompt = MemoryPrompt.specialist(role, workspace: project.name)
        return options
    }

    private func memoryAgentKey(_ role: MemoryRole, _ projectID: UUID) -> String {
        "\(projectID.uuidString)-\(role.rawValue)"
    }

    /// The project's importer or reconciler: the one already open, or a new one.
    func memoryAgent(_ role: MemoryRole, projectID: UUID) -> ThreadModel {
        let key = memoryAgentKey(role, projectID)
        if let existing = memoryAgents[key] { return existing }
        let thread = ThreadModel(memoryRole: role, projectID: projectID, app: self)
        memoryAgents[key] = thread
        return thread
    }

    func existingMemoryAgent(_ role: MemoryRole, projectID: UUID) -> ThreadModel? {
        memoryAgents[memoryAgentKey(role, projectID)]
    }

    /// Ends the conversation; the next one starts fresh.
    func closeMemoryAgent(_ role: MemoryRole, projectID: UUID) {
        memoryAgents.removeValue(forKey: memoryAgentKey(role, projectID))?.endSession()
    }

    /// Starts an import of `sources` (files and folders) in the project's importer.
    func startMemoryImport(projectID: UUID, sources: [String], note: String?) {
        let agent = memoryAgent(.importer, projectID: projectID)
        let folders = sources.map { path -> String in
            var isDirectory: ObjCBool = false
            FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory)
            return isDirectory.boolValue ? path : (path as NSString).deletingLastPathComponent
        }
        let added = Set(folders).subtracting(agent.extraDirectories)
        if !added.isEmpty {
            agent.extraDirectories += added.sorted()
            // Folders are given at launch.
            agent.restartSession()
        }
        agent.send(MemoryPrompt.importRequest(sources: sources, note: note))
    }

    func startMemoryReview(projectID: UUID, note: String?) {
        let agent = memoryAgent(.reconciler, projectID: projectID)
        var request = MemoryPrompt.reconcileRequest
        if let note, !note.isEmpty { request += "\n\n" + note }
        agent.send(request)
    }

    // MARK: - Sources to import

    /// Claude Code's own memory files for the project's folder, if it has any.
    func claudeCodeMemoryFiles(for project: Project) -> [URL] {
        ClaudeCodeMemory.files(for: project.path, configDirectory: ClaudeSessionArchive.configDirectory(environment: environment))
    }

    func claudeCodeMemoryFolder(for project: Project) -> URL {
        ClaudeCodeMemory.directory(for: project.path, configDirectory: ClaudeSessionArchive.configDirectory(environment: environment))
    }

    /// Instruction files other tools keep in the project: what an import usually starts from.
    func instructionFiles(in project: Project) -> [URL] {
        guard project.url.standardizedFileURL.path != NSHomeDirectory() else { return [] }
        let names = ["CLAUDE.md", "CLAUDE.local.md", "AGENTS.md", ".cursorrules", ".windsurfrules", "GEMINI.md",
                     ".github/copilot-instructions.md", ".cursor/rules"]
        return names.map { project.url.appending(path: $0) }.filter { FileManager.default.fileExists(atPath: $0.path) }
    }

    /// Pasted text to import, kept as a file the importer can read.
    func savePastedMemorySource(_ text: String) -> String? {
        let folder = store.root.appending(path: "memory-imports")
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let url = folder.appending(path: "pasted-\(Int(Date.now.timeIntervalSince1970)).md")
        return (try? text.write(to: url, atomically: true, encoding: .utf8)) != nil ? url.path : nil
    }

    // MARK: - Files

    enum MemoryExportFormat {
        /// One file that brings everything back.
        case json
        /// A folder in Claude Code's memory format.
        case markdown
    }

    func exportMemory(of project: Project, as format: MemoryExportFormat) {
        let memories = store.database.memories(scope: memoryScope(for: project.id))
        let stamp = Date.now.formatted(.iso8601.year().month().day())
        switch format {
        case .json:
            let panel = NSSavePanel()
            panel.allowedContentTypes = [.json]
            panel.nameFieldStringValue = "\(project.name) Memory \(stamp).json"
            panel.message = "Everything in \(project.name)'s memory, to import into YourHarness again."
            guard panel.runModal() == .OK, let url = panel.url else { return }
            do {
                try writeMemoryExport(memories, of: project, as: .json, to: url)
                noteExport(project, count: memories.count, to: url)
            } catch {
                presentMemoryError("Couldn't export the memory", error.localizedDescription)
            }
        case .markdown:
            let panel = NSSavePanel()
            panel.nameFieldStringValue = "\(project.name) Memory \(stamp)"
            panel.message = "A folder with a Markdown file a memory and a MEMORY.md index, as Claude Code keeps its memory."
            guard panel.runModal() == .OK, let url = panel.url else { return }
            do {
                try writeMemoryExport(memories, of: project, as: .markdown, to: url)
                noteExport(project, count: memories.count, to: url)
            } catch {
                presentMemoryError("Couldn't export the memory", error.localizedDescription)
            }
        }
    }

    func writeMemoryExport(_ memories: [HarnessDatabase.Memory], of project: Project, as format: MemoryExportFormat, to url: URL) throws {
        switch format {
        case .json:
            try MemoryArchive.json(memories, workspace: project.name, path: project.path).write(to: url, options: .atomic)
        case .markdown:
            try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
            for (name, text) in MemoryArchive.markdownFiles(memories, workspace: project.name) {
                try text.write(to: url.appending(path: name), atomically: true, encoding: .utf8)
            }
        }
    }

    private func noteExport(_ project: Project, count: Int, to url: URL) {
        let scope = memoryScope(for: project.id)
        store.database.recordMemoryEvent(scope: scope, kind: .exported, actor: .user, count: count, detail: url.lastPathComponent)
        memoryChanged(scope)
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }

    /// Imports a YourHarness export, or Markdown memory files (Claude Code's or an export's),
    /// with no agent. Anything else is offered to the importer.
    func presentImportMemoryFiles(into project: Project, openImporter: @escaping ([String]) -> Void) {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        panel.prompt = "Import"
        panel.message = "Choose a YourHarness memory export (.json), or Markdown memory files or their folder."
        guard panel.runModal() == .OK, !panel.urls.isEmpty else { return }
        importMemoryFiles(panel.urls, into: project, openImporter: openImporter)
    }

    func importMemoryFiles(_ urls: [URL], into project: Project, openImporter: (([String]) -> Void)? = nil) {
        let scope = memoryScope(for: project.id)
        var incoming: [HarnessDatabase.Memory] = [], unstructured: [String] = [], problems: [String] = []
        for url in urls {
            var isDirectory: ObjCBool = false
            FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory)
            if isDirectory.boolValue {
                let found = MemoryArchive.memories(fromMarkdown: MemoryArchive.readMarkdownFolder(url), scope: scope)
                if found.isEmpty { unstructured.append(url.path) } else { incoming += found }
            } else if url.pathExtension.lowercased() == "json" {
                do {
                    incoming += try MemoryArchive.memories(fromJSON: Data(contentsOf: url), scope: scope)
                } catch let error as MemoryArchive.ReadError {
                    if error == .notAnExport { unstructured.append(url.path) } else { problems.append("\(url.lastPathComponent): \(error)") }
                } catch {
                    problems.append("\(url.lastPathComponent): \(error.localizedDescription)")
                }
            } else if url.pathExtension.lowercased() == "md", let text = try? String(contentsOf: url, encoding: .utf8) {
                let found = MemoryArchive.memories(fromMarkdown: [url.lastPathComponent: text], scope: scope)
                if found.isEmpty { unstructured.append(url.path) } else { incoming += found }
            } else {
                unstructured.append(url.path)
            }
        }
        if !incoming.isEmpty {
            let report = MemoryArchive.merge(incoming, into: scope, database: store.database, actor: .user)
            memoryChanged(scope)
            importReport = MemoryImportReport(projectName: project.name, report: report, unstructured: unstructured, problems: problems)
        } else if !problems.isEmpty {
            presentMemoryError("Couldn't import", problems.joined(separator: "\n"))
        }
        if !unstructured.isEmpty, incoming.isEmpty, let openImporter {
            // Not memory files: that's what the importer is for.
            openImporter(unstructured)
        }
    }

    func presentMemoryError(_ title: String, _ message: String) {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = message
        alert.runModal()
    }
}

/// What a direct import did, shown once.
struct MemoryImportReport: Identifiable {
    let id = UUID()
    let projectName: String
    let report: MemoryArchive.ImportReport
    /// Files that weren't memory files, left for the importer.
    let unstructured: [String]
    let problems: [String]
}

/// One project's memory as the Memory view shows it: loaded off the main thread, and again
/// whenever it changes.
@MainActor @Observable
final class MemoryLibrary {
    enum Tab: String, CaseIterable, Identifiable {
        case memories, insights, activity, graph
        var id: String { rawValue }
        var label: String {
            switch self {
            case .memories: "Memories"
            case .insights: "Insights"
            case .activity: "Activity"
            case .graph: "Graph"
            }
        }
    }

    typealias Memory = HarnessDatabase.Memory

    let projectID: UUID
    let scope: String
    @ObservationIgnored let database: HarnessDatabase

    private(set) var memories: [Memory] = [] {
        didSet { byID = Dictionary(memories.map { ($0.id, $0) }) { first, _ in first } }
    }
    /// The same, by ID: rows look up their links and titles here.
    private(set) var byID: [String: Memory] = [:]
    private(set) var stats: MemoryStats?
    private(set) var events: [HarnessDatabase.MemoryEvent] = []
    private(set) var isLoaded = false
    @ObservationIgnored private var loadedRevision: Int?
    @ObservationIgnored private var loading: Task<Void, Never>?

    var tab = Tab.memories
    var selection: String?
    var searchText = ""
    var kindFilter: MemoryKind?
    var showsArchived = false

    init(projectID: UUID, scope: String, database: HarnessDatabase) {
        self.projectID = projectID
        self.scope = scope
        self.database = database
    }

    func refresh(revision: Int) {
        guard loadedRevision != revision else { return }
        loadedRevision = revision
        let database = database, scope = scope
        loading?.cancel()
        loading = Task {
            let (memories, stats, events) = await Task.detached(priority: .userInitiated) {
                (database.memories(scope: scope), database.memoryStats(scope: scope), database.memoryEvents(scope: scope, limit: 400))
            }.value
            guard !Task.isCancelled else { return }
            self.memories = memories
            self.stats = stats
            self.events = events
            self.isLoaded = true
            if let selection, !memories.contains(where: { $0.id == selection }) { self.selection = nil }
        }
    }

    /// What the list shows: the filters, then a search over what memories say.
    var visibleMemories: [Memory] {
        let words = searchText.lowercased().split(separator: " ").map(String.init)
        return memories.filter { memory in
            guard showsArchived ? memory.status != .active : memory.status == .active else { return false }
            if let kindFilter, MemoryKind.of(memory.kind) != kindFilter { return false }
            guard !words.isEmpty else { return true }
            let text = (memory.title + " " + memory.description + " " + memory.body).lowercased()
            return words.allSatisfy(text.contains)
        }.sorted { a, b in
            a.pinned != b.pinned ? a.pinned : a.updatedAt > b.updatedAt
        }
    }

    /// Memories that link to `id`.
    func backlinks(to id: String) -> [Memory] {
        memories.filter { $0.links.contains { $0.to == id } }
    }

    func versions(of id: String) -> [HarnessDatabase.MemoryVersion] {
        database.memoryVersions(id)
    }

    // MARK: - Changes you make

    private func write(_ body: @escaping @Sendable (HarnessDatabase) -> Void) {
        let database = database, scope = scope
        Task {
            await Task.detached { body(database) }.value
            AppModel.shared.memoryChanged(scope)
        }
    }

    func save(_ memory: Memory) {
        var memory = memory
        memory.updatedAt = .now
        // Shown at once; the database follows.
        if let index = memories.firstIndex(where: { $0.id == memory.id }) { memories[index] = memory } else { memories.insert(memory, at: 0) }
        let saved = memory
        write { $0.saveMemory(saved, actor: .user) }
    }

    @discardableResult
    func newMemory() -> Memory {
        let memory = Memory(scope: scope, kind: (kindFilter ?? .project).rawValue, title: "New Memory", body: "", origin: "user")
        showsArchived = false
        searchText = ""
        save(memory)
        selection = memory.id
        return memory
    }

    func setStatus(_ status: Memory.Status, _ id: String) {
        write { $0.setMemoryStatus(id, status, actor: .user) }
    }

    func setPinned(_ pinned: Bool, _ id: String) {
        if let index = memories.firstIndex(where: { $0.id == id }) { memories[index].pinned = pinned }
        write { $0.setMemoryPinned(id, pinned, actor: .user) }
    }

    func delete(_ id: String) {
        memories.removeAll { $0.id == id }
        if selection == id { selection = nil }
        write { $0.deleteMemory(id, actor: .user) }
    }

    func link(_ from: String, to: String) {
        write { $0.linkMemories(from, to, actor: .user) }
    }

    func unlink(_ from: String, _ to: String) {
        write { $0.unlinkMemories(from, to, actor: .user) }
    }

    func restore(_ id: String, version: Int) {
        write { $0.restoreMemoryVersion(id, version: version, actor: .user) }
    }
}
