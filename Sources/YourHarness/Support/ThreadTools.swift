import Foundation
import HarnessCore

/// `mcp__threads__…`: an agent looks through the app's threads (list, read, search), tidies
/// them (rename, settle, pin) and brings in Claude Code sessions, the way you would from the
/// sidebar and the Resume sheet. It can't delete anything. A thread in Home reaches every
/// project; a thread anywhere else only its own project's threads, and only sessions that ran
/// in that project's folder.
struct ThreadToolServer: MCPToolServer {
    let name = "threads"
    let tools = ThreadTools.definitions
    let threads: ThreadTools

    func call(_ tool: String, arguments: JSONValue, toolUseID: String?) async -> MCPToolResult {
        await threads.call(tool, arguments)
    }
}

@MainActor
final class ThreadTools {
    /// The thread whose agent calls: its project decides what it reaches.
    private weak var caller: ThreadModel?
    private unowned let app: AppModel

    init(thread: ThreadModel, app: AppModel) {
        caller = thread
        self.app = app
    }

    nonisolated static let definitions: [MCPTool] = {
        let threadID: JSONValue = ["type": "string", "description": "The thread's id from list_threads or search_threads (its first 8 characters are enough)."]
        return [
            MCPTool(
                name: "list_threads",
                description: "List the user's threads in YourHarness that you can reach, most recent first, with their id, project, state and when they last changed.",
                inputSchema: MCPTool.schema([
                    "query": ["type": "string", "description": "Only threads whose title has all these words."],
                    "project": ["type": "string", "description": "Only this project's threads (name or id). From Home only."],
                    "include_settled": ["type": "boolean", "description": "Include settled threads (finished work on the shelf). Default false."],
                    "limit": ["type": "integer", "minimum": 1, "maximum": 200, "description": "Default 50."],
                ]),
                readOnly: true),
            MCPTool(
                name: "read_thread",
                description: "Read a thread's conversation: messages, Claude's replies and its tool calls, numbered. Without offset, the last entries.",
                inputSchema: MCPTool.schema([
                    "thread_id": threadID,
                    "offset": ["type": "integer", "minimum": 0, "description": "The entry number to start at (from a search hit, say)."],
                    "limit": ["type": "integer", "minimum": 1, "maximum": 100, "description": "How many entries. Default 30."],
                ], required: ["thread_id"]),
                readOnly: true),
            MCPTool(
                name: "search_threads",
                description: "Search what was said in the threads you can reach (messages, replies, commands). Each hit gives the thread and entry number for read_thread.",
                inputSchema: MCPTool.schema([
                    "query": ["type": "string", "description": "Words to find; the last one may be a prefix."],
                    "limit": ["type": "integer", "minimum": 1, "maximum": 50, "description": "Default 20."],
                ], required: ["query"]),
                readOnly: true),
            MCPTool(
                name: "rename_thread",
                description: "Rename a thread, as the user would in the sidebar.",
                inputSchema: MCPTool.schema([
                    "thread_id": threadID,
                    "title": ["type": "string", "description": "The new title: short, says what the thread is about."],
                ], required: ["thread_id", "title"])),
            MCPTool(
                name: "settle_thread",
                description: "Settle a thread (move finished work to its project's Settled shelf; the conversation stays and can be picked up again), or bring a settled one back.",
                inputSchema: MCPTool.schema([
                    "thread_id": threadID,
                    "settled": ["type": "boolean", "description": "False brings it back. Default true."],
                ], required: ["thread_id"])),
            MCPTool(
                name: "pin_thread",
                description: "Pin a thread to the top of its project in the sidebar, or unpin it.",
                inputSchema: MCPTool.schema([
                    "thread_id": threadID,
                    "pinned": ["type": "boolean", "description": "False unpins. Default true."],
                ], required: ["thread_id"])),
            MCPTool(
                name: "list_claude_code_sessions",
                description: "List sessions from Claude Code's own history (claude in a terminal or an editor) that you can import, most recent first, with the folder each ran in.",
                inputSchema: MCPTool.schema([
                    "query": ["type": "string", "description": "Only sessions whose title or first message has all these words."],
                    "include_imported": ["type": "boolean", "description": "Include sessions that already have a thread. Default false."],
                    "limit": ["type": "integer", "minimum": 1, "maximum": 50, "description": "Default 20."],
                ]),
                readOnly: true),
            MCPTool(
                name: "import_claude_code_session",
                description: "Import a Claude Code session as a thread: its history shows in the thread, which continues the session (claude --resume) when the user sends a message. The user isn't taken to it.",
                inputSchema: MCPTool.schema([
                    "session_id": ["type": "string", "description": "From list_claude_code_sessions."],
                    "project": ["type": "string", "description": "From Home only: the project to put it in (name or id). Default: the project for the folder it ran in, else Home."],
                ], required: ["session_id"])),
        ]
    }()

    func call(_ tool: String, _ args: JSONValue) async -> MCPToolResult {
        guard let caller, let scope = scope(of: caller) else { return .error("This thread is gone.") }
        do {
            switch tool {
            case "list_threads": return try list(args, scope: scope)
            case "read_thread": return try await read(args, scope: scope)
            case "search_threads": return try await search(args, scope: scope)
            case "rename_thread": return try rename(args, scope: scope)
            case "settle_thread": return try settle(args, scope: scope)
            case "pin_thread": return try pin(args, scope: scope)
            case "list_claude_code_sessions": return await sessions(args, scope: scope)
            case "import_claude_code_session": return try await importSession(args, scope: scope)
            default: return .error("No tool named \(tool).")
            }
        } catch {
            return .error(error.message)
        }
    }

    // MARK: - Reach

    /// What a thread reaches: everything from Home, else its own project.
    enum Scope {
        case everything
        case project(Project)

        func contains(_ thread: ThreadModel) -> Bool {
            switch self {
            case .everything: true
            case .project(let project): thread.projectID == project.id
            }
        }
    }

    private func scope(of thread: ThreadModel) -> Scope? {
        guard let project = app.project(id: thread.projectID) else { return nil }
        return app.isHome(project) ? .everything : .project(project)
    }

    struct Failure: Error {
        var message: String
    }

    /// Threads the agent may see: yours, not side chats or memory agents.
    private func reachable(_ scope: Scope) -> [ThreadModel] {
        app.threads.filter { !$0.isAside && scope.contains($0) }
    }

    /// A thread by id or the start of one. One outside the scope reads as not there.
    private func thread(_ args: JSONValue, scope: Scope) throws(Failure) -> ThreadModel {
        let given = (args["thread_id"]?.string ?? "").trimmingCharacters(in: .whitespaces).lowercased()
        guard given.count >= 6 else { throw Failure(message: "Give thread_id: the id from list_threads (8 characters are enough).") }
        let matches = reachable(scope).filter { $0.id.uuidString.lowercased().hasPrefix(given) }
        guard let match = matches.first else { throw Failure(message: "No thread \(given) that you can reach. list_threads shows them.") }
        guard matches.count == 1 else { throw Failure(message: "\(given) matches \(matches.count) threads; give more of the id.") }
        return match
    }

    /// A project named in the arguments, from Home; elsewhere only the thread's own.
    private func project(_ args: JSONValue, scope: Scope) throws(Failure) -> Project? {
        guard let given = args["project"]?.string?.trimmingCharacters(in: .whitespaces), !given.isEmpty else { return nil }
        let found = app.projects.first { $0.id.uuidString.lowercased().hasPrefix(given.lowercased()) && given.count >= 6 }
            ?? app.projects.first { $0.name.localizedCaseInsensitiveCompare(given) == .orderedSame }
        guard let found else { throw Failure(message: "No project \(given). Projects: \(app.projects.map(\.name).joined(separator: ", ")).") }
        if case .project(let own) = scope, own.id != found.id {
            throw Failure(message: "From a thread in \(own.name) you can only reach \(own.name). Work across projects from a thread in Home.")
        }
        return found
    }

    // MARK: - Looking

    private func list(_ args: JSONValue, scope: Scope) throws(Failure) -> MCPToolResult {
        let project = try project(args, scope: scope)
        let words = Self.words(args["query"]?.string)
        let includeSettled = args["include_settled"]?.bool ?? false
        let limit = max(1, min(args["limit"]?.int ?? 50, 200))
        let found = reachable(scope)
            .filter { project == nil || $0.projectID == project?.id }
            .filter { includeSettled || !$0.isSettled }
            .filter { Self.matches($0.title, words) }
            .sorted { $0.sortDate > $1.sortDate }
        guard !found.isEmpty else { return .text("No threads match.") }
        var lines = [(project.map { "\($0.name):" } ?? header(scope)) + " \(found.count) thread\(found.count == 1 ? "" : "s")\(found.count > limit ? ", the \(limit) most recent" : ""):"]
        lines += found.prefix(limit).map { describe($0) }
        return .text(lines.joined(separator: "\n"))
    }

    private func read(_ args: JSONValue, scope: Scope) async throws(Failure) -> MCPToolResult {
        let thread = try thread(args, scope: scope)
        let entries = await thread.transcriptForReading()
        let limit = max(1, min(args["limit"]?.int ?? 30, 100))
        let start = args["offset"]?.int.map { max(0, min($0, entries.count)) } ?? max(0, entries.count - limit)
        let end = min(entries.count, start + limit)
        var lines = [describe(thread), "Entries \(start)–\(max(start, end - 1)) of \(entries.count)" + (start > 0 ? " (offset to read earlier ones)" : "") + ":"]
        var budget = 60_000
        for index in start..<end {
            let line = Self.render(entries[index], index: index)
            budget -= line.count
            if budget < 0 {
                lines.append("… stopped at entry \(index) to keep this short; read on with offset \(index).")
                break
            }
            lines.append(line)
        }
        return .text(lines.joined(separator: "\n"))
    }

    private func search(_ args: JSONValue, scope: Scope) async throws(Failure) -> MCPToolResult {
        guard let query = args["query"]?.string, !query.trimmingCharacters(in: .whitespaces).isEmpty else {
            throw Failure(message: "Give query: the words to find.")
        }
        let limit = max(1, min(args["limit"]?.int ?? 20, 50))
        let database = app.store.database
        let projectID: String? = if case .project(let project) = scope { project.id.uuidString } else { nil }
        // More than asked for: hits in threads it can't reach are dropped.
        let hits = await Task.detached(priority: .userInitiated) { database.search(query, projectID: projectID, limit: limit * 3) }.value
        let byID = Dictionary(reachable(scope).map { ($0.id.uuidString, $0) }) { first, _ in first }
        let kept = hits.compactMap { hit in byID[hit.threadID].map { (hit, $0) } }.prefix(limit)
        guard !kept.isEmpty else { return .text("Nothing found for “\(query)”.") }
        var lines: [String] = []
        var lastThread: UUID?
        for (hit, thread) in kept {
            if thread.id != lastThread {
                lines.append(describe(thread))
                lastThread = thread.id
            }
            lines.append("    entry \(hit.seq) (\(hit.kind.rawValue)): \(hit.snippet.replacingOccurrences(of: "\n", with: " "))")
        }
        return .text(lines.joined(separator: "\n"))
    }

    // MARK: - Tidying

    private func rename(_ args: JSONValue, scope: Scope) throws(Failure) -> MCPToolResult {
        let thread = try thread(args, scope: scope)
        let title = (args["title"]?.string ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty else { throw Failure(message: "Give title: the new name.") }
        guard title.count <= 120 else { throw Failure(message: "That title is \(title.count) characters; keep it under 120.") }
        let old = thread.title
        thread.rename(title)
        return .text("Renamed “\(old)” to “\(thread.title)”.")
    }

    private func settle(_ args: JSONValue, scope: Scope) throws(Failure) -> MCPToolResult {
        let thread = try thread(args, scope: scope)
        let settled = args["settled"]?.bool ?? true
        guard thread.isSettled != settled else { return .text("“\(thread.title)” is already \(settled ? "settled" : "active").") }
        app.setSettled(settled, thread)
        return .text(settled ? "Settled “\(thread.title)”: it's on the Settled shelf of \(projectName(thread)), and comes back when the user sends it a message."
                             : "Brought “\(thread.title)” back to the active threads of \(projectName(thread)).")
    }

    private func pin(_ args: JSONValue, scope: Scope) throws(Failure) -> MCPToolResult {
        let thread = try thread(args, scope: scope)
        let pinned = args["pinned"]?.bool ?? true
        guard thread.isPinned != pinned else { return .text("“\(thread.title)” is already \(pinned ? "pinned" : "unpinned").") }
        guard !(pinned && thread.isSettled) else { throw Failure(message: "“\(thread.title)” is settled; bring it back with settle_thread first.") }
        app.setPinned(pinned, thread)
        return .text("\(pinned ? "Pinned" : "Unpinned") “\(thread.title)”.")
    }

    // MARK: - Claude Code sessions

    private func sessions(_ args: JSONValue, scope: Scope) async -> MCPToolResult {
        let words = Self.words(args["query"]?.string)
        let includeImported = args["include_imported"]?.bool ?? false
        let limit = max(1, min(args["limit"]?.int ?? 20, 50))
        let config = await app.claudeConfigDirectory()
        let imported = Set(app.threads.compactMap { $0.sessionID?.lowercased() })
        let folder: String? = if case .project(let project) = scope { project.path } else { nil }
        let found = await Task.detached(priority: .userInitiated) {
            Self.findSessions(config: config, inside: folder, words: words, skipping: includeImported ? [] : imported, limit: limit)
        }.value
        guard !found.isEmpty else {
            return .text(folder.map { "No Claude Code sessions to import from \($0)." } ?? "No Claude Code sessions to import.")
        }
        var lines = ["\(found.count) Claude Code session\(found.count == 1 ? "" : "s"), most recent first:"]
        for archive in found {
            var line = "- \(archive.sessionID) · “\(Self.oneLine(archive.title ?? archive.firstMessage ?? "Untitled", max: 90))”"
            line += " · \(archive.messageCount) message\(archive.messageCount == 1 ? "" : "s")"
            if let updated = archive.updatedAt { line += " · \(Self.ago(updated))" }
            line += " · in \(archive.cwd ?? "an unknown folder")"
            if let thread = app.thread(forSession: archive.sessionID) {
                line += scope.contains(thread) ? " · already thread \(shortID(thread))" : " · already imported"
            }
            lines.append(line)
        }
        return .text(lines.joined(separator: "\n"))
    }

    /// The newest sessions that ran in `folder` (or anywhere, when nil) and match `words`,
    /// read one at a time until there are enough.
    nonisolated static func findSessions(config: URL, inside folder: String?, words: [String], skipping: Set<String>, limit: Int) -> [ClaudeSessionArchive] {
        var found: [ClaudeSessionArchive] = []
        var read = 0
        for file in ClaudeSessionArchive.sessionFiles(configDirectory: config) {
            guard found.count < limit, read < 300 else { break }
            guard !skipping.contains(file.sessionID) else { continue }
            if let folder, !ClaudeSessionArchive.folderName(file.folderName, mayBeInside: folder) { continue }
            read += 1
            guard let archive = try? ClaudeSessionArchive.load(from: file.url, sessionID: file.sessionID) else { continue }
            if let folder, !isInside(archive.cwd, folder) { continue }
            guard matches([archive.title, archive.firstMessage].compactMap(\.self).joined(separator: " "), words) else { continue }
            found.append(archive)
        }
        return found
    }

    /// `path` is `folder` or somewhere inside it.
    nonisolated static func isInside(_ path: String?, _ folder: String) -> Bool {
        guard let path else { return false }
        let inner = URL(fileURLWithPath: path).standardizedFileURL.path
        let outer = URL(fileURLWithPath: folder).standardizedFileURL.path
        return inner == outer || inner.hasPrefix(outer.hasSuffix("/") ? outer : outer + "/")
    }

    private func importSession(_ args: JSONValue, scope: Scope) async throws(Failure) -> MCPToolResult {
        guard let sessionID = args["session_id"]?.string.flatMap(ClaudeSessionArchive.sessionID(in:)) else {
            throw Failure(message: "Give session_id: one from list_claude_code_sessions.")
        }
        let chosen = try project(args, scope: scope)
        if let existing = app.thread(forSession: sessionID) {
            guard scope.contains(existing) else { throw Failure(message: "That session is already a thread, in a project you can't reach from here.") }
            return .text("Already imported: \(describe(existing, listed: false))")
        }
        let archive: ClaudeSessionArchive
        switch await app.loadClaudeSession(sessionID) {
        case .success(let loaded): archive = loaded
        case .failure(let error): throw Failure(message: error.errorDescription ?? "Couldn't read that session.")
        }
        let destination: Project
        switch scope {
        case .project(let own):
            guard Self.isInside(archive.cwd, own.path) else {
                throw Failure(message: "That session ran in \(archive.cwd ?? "an unknown folder"), outside \(own.name) (\(own.path)). Import it from a thread in Home.")
            }
            destination = own
        case .everything:
            // Never a new project: the one for its folder, else Home, where it still runs in its folder.
            destination = chosen ?? app.suggestedProject(forSessionFolder: archive.cwd) ?? app.homeProject
        }
        do {
            let thread = try app.resumeClaudeSession(archive, into: destination, opens: false)
            let messages = archive.messageCount == 1 ? "1 message is" : "\(archive.messageCount) messages are"
            return .text("Imported as \(describe(thread, listed: false))\n\(messages) in the thread; it continues the session when the user sends it a message.")
        } catch {
            throw Failure(message: error.errorDescription ?? "Couldn't import that session.")
        }
    }

    // MARK: - Text

    private func header(_ scope: Scope) -> String {
        switch scope {
        case .everything: "All projects:"
        case .project(let project): "\(project.name):"
        }
    }

    private func projectName(_ thread: ThreadModel) -> String {
        app.project(id: thread.projectID)?.name ?? "its project"
    }

    /// One line: id, title, project and state.
    /// `listed` starts it as an item of a list.
    private func describe(_ thread: ThreadModel, listed: Bool = true) -> String {
        var states: [String] = []
        if thread === caller { states.append("this thread") }
        if thread.isWorking { states.append("working") }
        if !thread.pendingPermissions.isEmpty { states.append("waiting for the user") }
        if thread.hasUnread { states.append("unread") }
        if thread.isPinned { states.append("pinned") }
        if thread.isSettled { states.append("settled") }
        if thread.sessionID == nil && thread.updatedAt == thread.createdAt { states.append("empty") }
        var line = (listed ? "- " : "") + "\(shortID(thread)) · “\(Self.oneLine(thread.title, max: 100))” · \(projectName(thread))"
        if !states.isEmpty { line += " · " + states.joined(separator: ", ") }
        return line + " · \(Self.ago(thread.sortDate))"
    }

    /// The first 8 characters of its id, or all of it when another thread starts the same.
    private func shortID(_ thread: ThreadModel) -> String {
        let id = thread.id.uuidString.lowercased()
        let short = String(id.prefix(8))
        let shared = app.threads.contains { $0 !== thread && $0.id.uuidString.lowercased().hasPrefix(short) }
        return shared ? id : short
    }

    /// An entry as a line or a few: who said it and what, tools in brief.
    nonisolated static func render(_ entry: EntrySnapshot, index: Int) -> String {
        let when = entry.date.map { " " + $0.formatted(date: .abbreviated, time: .shortened) } ?? ""
        switch entry.kind {
        case .user:
            return "[\(index)] user\(when): \(clip(entry.text, 4000))"
        case .assistant:
            return "[\(index)] claude\(when): \(clip(entry.text, 4000))"
        case .thinking:
            return "[\(index)] (thinking, \(entry.text.count) characters)"
        case .tool:
            var line = "[\(index)] tool \(entry.toolName ?? "?"): \(oneLine(entry.toolInput ?? "", max: 300))"
            if let result = entry.toolResult {
                line += "\n      → \(entry.toolIsError == true ? "error: " : "")\(oneLine(result, max: 400))"
            }
            return line
        case .notice:
            return "[\(index)] notice: \(oneLine(entry.text, max: 400))"
        case .commandOutput:
            return "[\(index)] command output: \(clip(entry.text, 1500))"
        }
    }

    nonisolated private static func clip(_ text: String, _ max: Int) -> String {
        text.count <= max ? text : text.prefix(max) + "… (\(text.count - max) more characters)"
    }

    nonisolated private static func oneLine(_ text: String, max: Int) -> String {
        let flat = text.split(whereSeparator: \.isNewline).joined(separator: " ")
        return flat.count <= max ? flat : flat.prefix(max) + "…"
    }

    nonisolated private static func ago(_ date: Date) -> String {
        date.formatted(.relative(presentation: .named))
    }

    nonisolated static func words(_ query: String?) -> [String] {
        (query ?? "").lowercased().split(whereSeparator: \.isWhitespace).map(String.init)
    }

    nonisolated static func matches(_ text: String, _ words: [String]) -> Bool {
        let text = text.lowercased()
        return words.allSatisfy(text.contains)
    }
}
