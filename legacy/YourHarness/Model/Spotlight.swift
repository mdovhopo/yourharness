import HarnessCore
import SwiftUI

/// ⌘K: one search over everything (threads by title, what was said in them, memories and
/// tasks in every project, projects, and what the app can do), to jump anywhere from the
/// keyboard. Titles and actions match as you type; transcripts and memories come from the
/// database's full-text indexes, off the main thread.
@MainActor @Observable
final class Spotlight {
    struct Result: Identifiable {
        enum Section: Int, CaseIterable {
            case recent, threads, tasks, memories, projects, conversations, actions

            var title: String {
                switch self {
                case .recent: "Recent Threads"
                case .threads: "Threads"
                case .tasks: "Tasks"
                case .memories: "Memories"
                case .projects: "Projects"
                case .conversations: "In Conversations"
                case .actions: "Actions"
                }
            }
        }

        let id: String
        let section: Section
        let symbol: String
        var tint: Color?
        let title: String
        let subtitle: String
        var snippet: AttributedString?
        var shortcut: String?
        let perform: @MainActor (AppModel, SpotlightEnvironment) -> Void
    }

    var isOpen = false
    var query = ""
    private(set) var results: [Result] = []
    var selection = 0
    @ObservationIgnored private var search: Task<Void, Never>?

    func toggle(_ app: AppModel) {
        if isOpen { close() } else { open(app) }
    }

    func open(_ app: AppModel, query: String = "") {
        self.query = query
        isOpen = true
        refresh(app)
    }

    func close() {
        isOpen = false
        search?.cancel()
    }

    func move(_ offset: Int) {
        guard !results.isEmpty else { return }
        selection = min(max(selection + offset, 0), results.count - 1)
    }

    func perform(_ result: Result?, app: AppModel, environment: SpotlightEnvironment) {
        guard let result else { return }
        close()
        result.perform(app, environment)
    }

    /// What matches as typed comes at once; what the indexes find follows a moment later.
    func refresh(_ app: AppModel) {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        let immediate = Self.immediate(query, app: app)
        results = immediate
        selection = 0
        search?.cancel()
        guard query.count >= 2 else { return }
        let database = app.store.database
        search = Task {
            try? await Task.sleep(for: .milliseconds(90))
            guard !Task.isCancelled else { return }
            let (hits, memories) = await Task.detached(priority: .userInitiated) {
                (database.search(query, limit: 80), database.findMemories(query, scope: nil, statuses: [.active], limit: 16))
            }.value
            guard !Task.isCancelled, self.query.trimmingCharacters(in: .whitespacesAndNewlines) == query else { return }
            let indexed = Self.indexed(hits: hits, memories: memories, app: app, shownThreads: Set(immediate.map(\.id)))
            results = Self.ordered(immediate + indexed)
        }
    }

    // MARK: - Matching

    private static func words(_ query: String) -> [String] {
        query.lowercased().split(whereSeparator: \.isWhitespace).map(String.init)
    }

    private static func matches(_ text: String, _ words: [String]) -> Bool {
        let text = text.lowercased()
        return words.allSatisfy(text.contains)
    }

    private static func ordered(_ results: [Result]) -> [Result] {
        results.enumerated().sorted { a, b in
            a.element.section.rawValue != b.element.section.rawValue ? a.element.section.rawValue < b.element.section.rawValue : a.offset < b.offset
        }.map(\.element)
    }

    private static func immediate(_ query: String, app: AppModel) -> [Result] {
        let words = words(query)
        var results: [Result] = []
        let threads = app.threads.filter { !$0.isEphemeral }
        if words.isEmpty {
            for thread in threads.sorted(by: { $0.sortDate > $1.sortDate }).prefix(6) {
                results.append(threadResult(thread, section: .recent, app: app))
            }
        } else {
            let found = threads.filter { matches($0.title, words) }.sorted { $0.sortDate > $1.sortDate }.prefix(8)
            results += found.map { threadResult($0, section: .threads, app: app) }
            for project in app.projects where matches(project.name, words) {
                results.append(Result(id: "project-\(project.id)", section: .projects, symbol: app.isHome(project) ? "house" : "folder",
                                      title: project.name, subtitle: project.path.replacingOccurrences(of: NSHomeDirectory(), with: "~")) { app, _ in
                    app.showProject(project)
                })
            }
        }
        results += actions(app).filter { words.isEmpty || matches($0.title, words) }
        return ordered(results)
    }

    private static func threadResult(_ thread: ThreadModel, section: Result.Section, app: AppModel) -> Result {
        let project = app.project(id: thread.projectID)?.name ?? ""
        let when = thread.updatedAt.formatted(.relative(presentation: .named))
        return Result(id: "thread-\(thread.id)", section: section, symbol: thread.isSettled ? "tray" : "bubble.left.and.text.bubble.right",
                      title: thread.title, subtitle: "\(project) · \(when)" + (thread.isSettled ? " · Settled" : "")) { app, _ in
            app.selectedThreadID = thread.id
        }
    }

    private static func indexed(hits: [HarnessDatabase.SearchHit], memories: [HarnessDatabase.MemoryHit], app: AppModel,
                                shownThreads: Set<String>) -> [Result] {
        var results: [Result] = []
        let projectsByScope = Dictionary(app.projects.map { (app.memoryScope(for: $0.id), $0) }) { first, _ in first }
        for hit in memories {
            guard let project = projectsByScope[hit.memory.scope] else { continue }
            let memory = hit.memory
            var subtitle = project.name
            if memory.isTask {
                subtitle += " · " + (memory.taskState?.label ?? "In Progress")
                if !memory.steps.isEmpty { subtitle += " · \(memory.stepsDone) of \(memory.steps.count) steps" }
            } else {
                subtitle += " · " + MemoryKind.of(memory.kind).label
            }
            let kind = MemoryKind.of(memory.kind)
            results.append(Result(id: "memory-\(memory.id)", section: memory.isTask ? .tasks : .memories, symbol: kind.symbol, tint: kind.color,
                                  title: memory.title, subtitle: subtitle,
                                  snippet: memory.description.isEmpty ? snippet(hit.snippet) : AttributedString(memory.description)) { app, _ in
                app.showMemory(of: project)
                let library = app.memoryLibrary(for: project)
                library.kindFilter = nil
                library.showsArchived = false
                library.searchText = ""
                library.tab = .memories
                library.selection = memory.id
            })
        }
        // One hit a thread, what was said before what a tool was given, and none for a thread
        // already found by its title.
        var seen = Set(shownThreads.compactMap { $0.hasPrefix("thread-") ? String($0.dropFirst(7)) : nil })
        let threads = Dictionary(app.threads.map { ($0.id.uuidString, $0) }) { first, _ in first }
        let ranked = hits.filter { $0.kind != .tool } + hits.filter { $0.kind == .tool }
        for hit in ranked where seen.insert(hit.threadID).inserted {
            guard let thread = threads[hit.threadID], results.count(where: { $0.section == .conversations }) < 8 else { continue }
            var result = threadResult(thread, section: .conversations, app: app)
            let word = firstMatch(hit.snippet)
            result = Result(id: "hit-\(thread.id)", section: .conversations, symbol: hit.kind == .user ? "person.crop.circle" : hit.kind == .tool ? "wrench.and.screwdriver" : "text.bubble",
                            title: result.title, subtitle: result.subtitle, snippet: snippet(hit.snippet)) { app, _ in
                app.selectedThreadID = thread.id
                thread.reveal(hit.entryID)
                // Find, on the word that matched: it highlights it and scrolls there.
                if let word {
                    thread.find.open()
                    thread.find.query = word
                }
            }
            results.append(result)
        }
        return results
    }

    private static func actions(_ app: AppModel) -> [Result] {
        let project = app.overviewProject ?? app.selectedProject
        return [
            Result(id: "action-new-thread", section: .actions, symbol: "square.and.pencil", title: "New Thread",
                   subtitle: "In \(project.name)", shortcut: "⌘N") { app, _ in _ = app.newThread(in: project) },
            Result(id: "action-memory", section: .actions, symbol: "brain", title: "Open Memory",
                   subtitle: project.name, shortcut: "⇧⌘M") { app, _ in app.showMemory(of: project) },
            Result(id: "action-tasks", section: .actions, symbol: "checklist", title: "Open Tasks",
                   subtitle: project.name) { app, _ in
                app.showMemory(of: project)
                app.memoryLibrary(for: project).tab = .tasks
            },
            Result(id: "action-import", section: .actions, symbol: "square.and.arrow.down", title: "Import Memory…",
                   subtitle: "The Importer, for \(project.name)") { _, environment in
                environment.openWindow(MemoryAgentKey(projectID: project.id, role: .importer))
            },
            Result(id: "action-reconcile", section: .actions, symbol: "wand.and.sparkles", title: "Reconcile Memory…",
                   subtitle: "The Reconciler, for \(project.name)") { _, environment in
                environment.openWindow(MemoryAgentKey(projectID: project.id, role: .reconciler))
            },
            Result(id: "action-add-project", section: .actions, symbol: "folder.badge.plus", title: "Add Project…",
                   subtitle: "Pick a folder to work in", shortcut: "⌘O") { app, _ in app.presentAddProject() },
            Result(id: "action-resume", section: .actions, symbol: "arrow.uturn.forward", title: "Resume Claude Code Session…",
                   subtitle: "Continue a session started elsewhere", shortcut: "⇧⌘O") { app, _ in app.isResumingSession = true },
            Result(id: "action-settings", section: .actions, symbol: "gearshape", title: "Settings…",
                   subtitle: "Agents, memory, browser, notifications", shortcut: "⌘,") { _, environment in environment.openSettings() },
        ]
    }

    /// The database marks matches with `[` `]`: bold them.
    static func snippet(_ marked: String) -> AttributedString {
        var result = AttributedString()
        var bold = false, run = ""
        func flush() {
            var part = AttributedString(run)
            if bold { part.inlinePresentationIntent = .stronglyEmphasized }
            result += part
            run = ""
        }
        for character in marked.replacingOccurrences(of: "\n", with: " ") {
            if character == "[" { flush(); bold = true } else if character == "]" { flush(); bold = false } else { run.append(character) }
        }
        flush()
        return result
    }

    private static func firstMatch(_ marked: String) -> String? {
        guard let open = marked.firstIndex(of: "["), let close = marked[open...].firstIndex(of: "]") else { return nil }
        let word = String(marked[marked.index(after: open)..<close])
        return word.isEmpty ? nil : word
    }
}

/// What results need from the window: other windows and Settings.
struct SpotlightEnvironment {
    let openWindow: (MemoryAgentKey) -> Void
    let openSettings: () -> Void
}
