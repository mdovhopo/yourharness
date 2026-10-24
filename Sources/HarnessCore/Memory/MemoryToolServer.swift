import Foundation

/// `mcp__memory__*`: a workspace's memory as tools, served from inside the app. Every call
/// is recorded (what was searched and found, what was read), which is what the analytics
/// are made of. Thread agents search, read and save; the importer and the reconciler also
/// list, merge and archive.
public struct MemoryToolServer: MCPToolServer {
    public enum Access: Sendable {
        /// Search and read: a side chat.
        case readOnly
        /// A thread's agent.
        case agent
        /// The importer and the reconciler.
        case specialist
    }

    public let name = "memory"
    public let database: HarnessDatabase
    public let scope: String
    public let access: Access
    public let actor: HarnessDatabase.MemoryActor
    /// Called after a call changed something in `scope`, from whatever thread it ran on.
    public let onChange: @Sendable (String) -> Void

    public static let maxTitle = 120
    public static let maxDescription = 240
    public static let maxBody = 6_000

    public init(database: HarnessDatabase, scope: String, access: Access, actor: HarnessDatabase.MemoryActor,
                onChange: @escaping @Sendable (String) -> Void = { _ in }) {
        self.database = database
        self.scope = scope
        self.access = access
        self.actor = actor
        self.onChange = onChange
    }

    /// Tool names as the agent sees them, for summaries in the transcript.
    public static let toolPrefix = "mcp__memory__"

    public var tools: [MCPTool] {
        var tools = [Self.search, Self.read]
        if access != .readOnly { tools += [Self.save, Self.update, Self.forget, Self.link] }
        if access == .specialist { tools += [Self.list, Self.stats, Self.merge, Self.unlink, Self.restore, Self.pin] }
        return tools
    }

    public func call(_ tool: String, arguments: JSONValue, toolUseID: String?) async -> MCPToolResult {
        switch tool {
        case "search": return search(arguments)
        case "read": return read(arguments)
        case "save": return save(arguments, toolUseID: toolUseID)
        case "update": return update(arguments)
        case "forget": return forget(arguments)
        case "link": return link(arguments)
        case "unlink": return unlink(arguments)
        case "list": return list(arguments)
        case "stats": return stats()
        case "merge": return merge(arguments)
        case "restore": return restore(arguments)
        case "pin": return pin(arguments)
        default: return .error("No tool named \(tool).")
        }
    }

    // MARK: - Definitions

    private static let kinds: JSONValue = ["type": "string", "enum": .array(MemoryKind.allCases.map { .string($0.rawValue) }),
                                           "description": "user, feedback, project or reference."]
    private static let ids: JSONValue = ["type": "array", "items": ["type": "string"], "minItems": 1,
                                         "description": "Memory IDs (the 8-character ones in the index are enough)."]

    static let search = MCPTool(
        name: "search",
        description: """
        Search this workspace's memory: words in titles, descriptions and bodies, best match first. Use it when \
        something you should know about isn't in the memory index, or to check before saving a new memory.
        """,
        inputSchema: MCPTool.schema([
            "query": ["type": "string", "description": "What to look for, in a few words."],
            "limit": ["type": "integer", "minimum": 1, "maximum": 30, "description": "At most this many (10 by default)."],
            "include_archived": ["type": "boolean", "description": "Also archived memories."],
        ], required: ["query"]),
        readOnly: true)

    static let read = MCPTool(
        name: "read",
        description: "Read memories in full: their body, kind, links and when they were last updated.",
        inputSchema: MCPTool.schema(["ids": ids], required: ["ids"]),
        readOnly: true)

    static let save = MCPTool(
        name: "save",
        description: """
        Save a new memory for every later session in this workspace: one fact, preference, decision or pointer. \
        Check the index or search first; if one already covers it, update that one instead.
        """,
        inputSchema: MCPTool.schema([
            "title": ["type": "string", "description": "A few words, e.g. \"Release only on request\"."],
            "description": ["type": "string", "description": "One line on what it says and when it matters: the index shows it."],
            "body": ["type": "string", "description": "The memory itself, short and self-contained. For feedback and project memories, add **Why:** and **How to apply:** lines."],
            "kind": kinds,
            "links": ["type": "array", "items": ["type": "string"], "description": "IDs of related memories."],
        ], required: ["title", "description", "body", "kind"]))

    static let update = MCPTool(
        name: "update",
        description: "Change a memory: give only what changes. The old version is kept.",
        inputSchema: MCPTool.schema([
            "id": ["type": "string"],
            "title": ["type": "string"],
            "description": ["type": "string"],
            "body": ["type": "string"],
            "kind": kinds,
        ], required: ["id"]))

    static let forget = MCPTool(
        name: "forget",
        description: "Archive a memory that's wrong or no longer true: agents stop getting it. It can be restored.",
        inputSchema: MCPTool.schema([
            "id": ["type": "string"],
            "reason": ["type": "string", "description": "Why, in a few words."],
        ], required: ["id"]))

    static let link = MCPTool(
        name: "link",
        description: "Link two related memories, so reading one points at the other.",
        inputSchema: MCPTool.schema([
            "from": ["type": "string"], "to": ["type": "string"],
            "relation": ["type": "string", "description": "Optional: how they relate, e.g. \"refines\", \"see also\", \"contradicts\"."],
        ], required: ["from", "to"]))

    static let unlink = MCPTool(
        name: "unlink",
        description: "Remove the link between two memories.",
        inputSchema: MCPTool.schema(["from": ["type": "string"], "to": ["type": "string"]], required: ["from", "to"]))

    static let list = MCPTool(
        name: "list",
        description: "List the workspace's memories: ID, kind, title, description, size, reads and dates.",
        inputSchema: MCPTool.schema([
            "kind": kinds,
            "status": ["type": "string", "enum": ["active", "proposed", "archived", "all"], "description": "active by default."],
        ]),
        readOnly: true)

    static let stats = MCPTool(
        name: "stats",
        description: "How the workspace's memory is used: sizes, reads, memories never read, likely duplicates, searches that found nothing.",
        inputSchema: MCPTool.schema([:]),
        readOnly: true)

    static let merge = MCPTool(
        name: "merge",
        description: """
        Merge memories that say the same thing into one: it keeps the first ID, every link, source and read count; \
        the others are archived and point at it.
        """,
        inputSchema: MCPTool.schema([
            "ids": ids,
            "title": ["type": "string"], "description": ["type": "string"], "body": ["type": "string"], "kind": kinds,
        ], required: ["ids", "title", "description", "body", "kind"]))

    static let restore = MCPTool(
        name: "restore",
        description: "Bring an archived memory back into use.",
        inputSchema: MCPTool.schema(["id": ["type": "string"]], required: ["id"]))

    static let pin = MCPTool(
        name: "pin",
        description: "Pin a memory so every session gets it in full, not only its index line (for the few that always matter), or unpin it.",
        inputSchema: MCPTool.schema(["id": ["type": "string"], "pinned": ["type": "boolean"]], required: ["id", "pinned"]))

    // MARK: - Calls

    private func search(_ arguments: JSONValue) -> MCPToolResult {
        guard let query = arguments["query"]?.string?.trimmingCharacters(in: .whitespacesAndNewlines), !query.isEmpty else {
            return .error("Say what to search for in `query`.")
        }
        let limit = min(max(arguments["limit"]?.int ?? 10, 1), 30)
        let statuses: Set<HarnessDatabase.Memory.Status> = arguments["include_archived"]?.bool == true ? [.active, .archived] : [.active]
        let hits = database.findMemories(query, scope: scope, statuses: statuses, limit: limit)
        database.noteMemorySearch(scope: scope, query: query, hits: hits.map(\.memory.id), actor: actor)
        onChange(scope)
        guard !hits.isEmpty else { return .text("No memory matches \"\(query)\".") }
        let lines = hits.map { hit in
            var line = "- \(hit.memory.shortID) [\(hit.memory.kind)] \(hit.memory.title)"
            if !hit.memory.description.isEmpty { line += " — \(hit.memory.description)" }
            if hit.memory.status != .active { line += " (\(hit.memory.status.rawValue))" }
            let snippet = hit.snippet.replacingOccurrences(of: "\n", with: " ")
            if !snippet.isEmpty, !hit.memory.description.contains(snippet) { line += "\n  \(snippet)" }
            return line
        }
        return .text("\(hits.count) match\(hits.count == 1 ? "" : "es") for \"\(query)\":\n" + lines.joined(separator: "\n")
                     + "\nRead the ones that matter with `read`.")
    }

    private func read(_ arguments: JSONValue) -> MCPToolResult {
        let requested = Self.idList(arguments["ids"] ?? arguments["id"])
        guard !requested.isEmpty else { return .error("Give the memory IDs in `ids`.") }
        var found: [HarnessDatabase.Memory] = [], missing: [String] = []
        for id in requested.prefix(20) {
            if let resolved = database.resolveMemoryID(id, scope: scope), let memory = database.memory(resolved) {
                found.append(memory)
            } else {
                missing.append(id)
            }
        }
        database.noteMemoryRecall(found.map(\.id), actor: actor)
        if !found.isEmpty { onChange(scope) }
        guard !found.isEmpty else { return .error("No memory with ID \(missing.joined(separator: ", ")) here.") }
        let titles = Dictionary(database.memories(scope: scope).map { ($0.id, $0) }) { first, _ in first }
        var text = found.map { memory in
            var block = "## \(memory.title)\nid: \(memory.shortID) · kind: \(memory.kind) · updated \(Self.day(memory.updatedAt))"
            if memory.status != .active { block += " · \(memory.status.rawValue)" }
            if let merged = memory.mergedInto { block += " · merged into \(String(merged.prefix(8)))" }
            if !memory.description.isEmpty { block += "\n\(memory.description)" }
            block += "\n\n\(memory.body)"
            let links = memory.links.compactMap { link in
                titles[link.to].map { "\($0.shortID) \($0.title)" + (link.relation.map { " (\($0))" } ?? "") }
            }
            if !links.isEmpty { block += "\n\nLinked: " + links.joined(separator: "; ") }
            return block
        }.joined(separator: "\n\n")
        if !missing.isEmpty { text += "\n\nNot found: \(missing.joined(separator: ", "))." }
        return .text(text)
    }

    private func save(_ arguments: JSONValue, toolUseID: String?) -> MCPToolResult {
        guard let title = clean(arguments["title"]), let body = clean(arguments["body"]) else {
            return .error("A memory needs a `title` and a `body`.")
        }
        let description = clean(arguments["description"]) ?? ""
        guard let kind = arguments["kind"]?.string.flatMap(MemoryKind.init(rawValue:)) else {
            return .error("`kind` must be one of: user, feedback, project, reference.")
        }
        if let problem = Self.problem(title: title, description: description, body: body) { return .error(problem) }
        let existing = database.memories(scope: scope, status: .active)
        if let same = existing.first(where: { $0.title.caseInsensitiveCompare(title) == .orderedSame }) {
            return .error("Memory \(same.shortID) already has this title. Update it with `update` instead of saving another.")
        }
        var memory = HarnessDatabase.Memory(scope: scope, kind: kind.rawValue, title: title, description: description, body: body,
                                            origin: Self.origin(actor))
        if let threadID = actor.threadID, let toolUseID { memory.sources = [.init(threadID: threadID, entryID: toolUseID)] }
        memory.links = Self.idList(arguments["links"]).compactMap { database.resolveMemoryID($0, scope: scope) }.map { .init(to: $0) }
        database.saveMemory(memory, actor: actor)
        onChange(scope)
        var text = "Saved as \(memory.shortID)."
        let similar = database.findMemories(title, scope: scope, limit: 4).filter { $0.memory.id != memory.id }
        if !similar.isEmpty {
            text += " Similar memories: " + similar.map { "\($0.memory.shortID) \($0.memory.title)" }.joined(separator: "; ")
                + ". If one says the same thing, merge them by updating it and forgetting this one."
        }
        return .text(text)
    }

    private func update(_ arguments: JSONValue) -> MCPToolResult {
        guard let id = resolve(arguments["id"]), var memory = database.memory(id) else { return notFound(arguments["id"]) }
        var changed = false
        if let title = clean(arguments["title"]), title != memory.title { memory.title = title; changed = true }
        if let description = arguments["description"]?.string.map({ $0.trimmingCharacters(in: .whitespacesAndNewlines) }),
           description != memory.description { memory.description = description; changed = true }
        if let body = clean(arguments["body"]), body != memory.body { memory.body = body; changed = true }
        if let raw = arguments["kind"]?.string {
            guard let kind = MemoryKind(rawValue: raw) else { return .error("`kind` must be one of: user, feedback, project, reference.") }
            if kind.rawValue != memory.kind { memory.kind = kind.rawValue; changed = true }
        }
        guard changed else { return .text("Nothing to change in \(memory.shortID).") }
        if let problem = Self.problem(title: memory.title, description: memory.description, body: memory.body) { return .error(problem) }
        memory.updatedAt = .now
        database.saveMemory(memory, actor: actor)
        onChange(scope)
        return .text("Updated \(memory.shortID).")
    }

    private func forget(_ arguments: JSONValue) -> MCPToolResult {
        guard let id = resolve(arguments["id"]), let memory = database.memory(id) else { return notFound(arguments["id"]) }
        database.setMemoryStatus(id, .archived, actor: actor, reason: arguments["reason"]?.string)
        onChange(scope)
        return .text("Archived \(memory.shortID) \(memory.title). Agents no longer get it; it can be restored.")
    }

    private func restore(_ arguments: JSONValue) -> MCPToolResult {
        guard let id = resolve(arguments["id"]), let memory = database.memory(id) else { return notFound(arguments["id"]) }
        database.setMemoryStatus(id, .active, actor: actor)
        onChange(scope)
        return .text("Restored \(memory.shortID) \(memory.title).")
    }

    private func pin(_ arguments: JSONValue) -> MCPToolResult {
        guard let id = resolve(arguments["id"]), let memory = database.memory(id) else { return notFound(arguments["id"]) }
        let pinned = arguments["pinned"]?.bool ?? true
        database.setMemoryPinned(id, pinned, actor: actor)
        onChange(scope)
        return .text("\(pinned ? "Pinned" : "Unpinned") \(memory.shortID) \(memory.title).")
    }

    private func link(_ arguments: JSONValue) -> MCPToolResult {
        guard let from = resolve(arguments["from"]) else { return notFound(arguments["from"]) }
        guard let to = resolve(arguments["to"]) else { return notFound(arguments["to"]) }
        guard from != to else { return .error("A memory can't link to itself.") }
        database.linkMemories(from, to, relation: arguments["relation"]?.string, actor: actor)
        onChange(scope)
        return .text("Linked \(from.prefix(8).lowercased()) → \(to.prefix(8).lowercased()).")
    }

    private func unlink(_ arguments: JSONValue) -> MCPToolResult {
        guard let from = resolve(arguments["from"]) else { return notFound(arguments["from"]) }
        guard let to = resolve(arguments["to"]) else { return notFound(arguments["to"]) }
        database.unlinkMemories(from, to, actor: actor)
        onChange(scope)
        return .text("Unlinked.")
    }

    private func list(_ arguments: JSONValue) -> MCPToolResult {
        let status = arguments["status"]?.string ?? "active"
        let kind = arguments["kind"]?.string
        let memories = database.memories(scope: scope, status: status == "all" ? nil : HarnessDatabase.Memory.Status(rawValue: status))
            .filter { kind == nil || $0.kind == kind }
        guard !memories.isEmpty else { return .text("No memories here\(status == "active" ? "" : " (\(status))") yet.") }
        let lines = memories.sorted { ($0.kind, $0.title) < ($1.kind, $1.title) }.map { memory in
            var line = "- \(memory.shortID) [\(memory.kind)] \(memory.title)"
            if !memory.description.isEmpty { line += " — \(memory.description)" }
            line += " (\(memory.size) B, read \(memory.recallCount)×, updated \(Self.day(memory.updatedAt))"
            if memory.pinned { line += ", pinned" }
            if memory.status != .active { line += ", \(memory.status.rawValue)" }
            if !memory.links.isEmpty { line += ", links \(memory.links.map { String($0.to.prefix(8)).lowercased() }.joined(separator: " "))" }
            return line + ")"
        }
        return .text("\(memories.count) memories:\n" + lines.joined(separator: "\n"))
    }

    private func stats() -> MCPToolResult {
        let stats = database.memoryStats(scope: scope)
        var lines = [
            "Active: \(stats.active), archived: \(stats.archived), pinned: \(stats.pinned).",
            "Size: \(stats.bytes) bytes (about \(MemoryStats.tokens(stats.bytes)) tokens), \(stats.averageBytes) on average.",
            "Kinds: " + stats.byKind.sorted { $0.key < $1.key }.map { "\($0.key) \($0.value)" }.joined(separator: ", ") + ".",
            "Links: \(stats.links); memories with none: \(stats.orphans).",
            "Last \(stats.window) days: \(stats.sessions) sessions, \(stats.recallsInWindow) reads of \(stats.recalledInWindow) memories, "
                + "\(stats.searches) searches (\(stats.searchesWithHits) found something).",
        ]
        if !stats.largest.isEmpty {
            lines.append("Largest: " + stats.largest.map { "\(String($0.id.prefix(8)).lowercased()) \($0.title) (\($0.bytes) B)" }.joined(separator: "; "))
        }
        if !stats.neverRecalled.isEmpty {
            lines.append("Never read, older than a week: " + stats.neverRecalled.prefix(30)
                .map { "\(String($0.id.prefix(8)).lowercased()) \($0.title)" }.joined(separator: "; "))
        }
        for group in stats.likelyDuplicates {
            lines.append("Possible duplicates: " + group.map { "\(String($0.id.prefix(8)).lowercased()) \($0.title)" }.joined(separator: " | "))
        }
        if !stats.missedQueries.isEmpty {
            lines.append("Searches that found nothing: " + stats.missedQueries.map { "\"\($0)\"" }.joined(separator: ", "))
        }
        return .text(lines.joined(separator: "\n"))
    }

    private func merge(_ arguments: JSONValue) -> MCPToolResult {
        var seen = Set<String>()
        let ids = Self.idList(arguments["ids"]).compactMap { database.resolveMemoryID($0, scope: scope) }.filter { seen.insert($0).inserted }
        guard ids.count >= 2 else { return .error("Give at least two memory IDs from this workspace in `ids`.") }
        guard let title = clean(arguments["title"]), let body = clean(arguments["body"]),
              let kind = arguments["kind"]?.string.flatMap(MemoryKind.init(rawValue:)) else {
            return .error("A merge needs the merged `title`, `description`, `body` and `kind`.")
        }
        let description = clean(arguments["description"]) ?? ""
        if let problem = Self.problem(title: title, description: description, body: body) { return .error(problem) }
        let merged = HarnessDatabase.Memory(scope: scope, kind: kind.rawValue, title: title, description: description, body: body)
        guard let result = database.mergeMemories(ids, into: merged, actor: actor) else {
            return .error("Couldn't merge those memories.")
        }
        onChange(scope)
        return .text("Merged into \(result.shortID) \(result.title); \(ids.count - 1) archived.")
    }

    // MARK: - Helpers

    private func resolve(_ value: JSONValue?) -> String? {
        value?.string.flatMap { database.resolveMemoryID($0, scope: scope) }
    }

    private func notFound(_ value: JSONValue?) -> MCPToolResult {
        .error("No memory with ID \(value?.string ?? "?") in this workspace. IDs are in the index, or use `search`.")
    }

    private func clean(_ value: JSONValue?) -> String? {
        guard let text = value?.string?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else { return nil }
        return text
    }

    static func idList(_ value: JSONValue?) -> [String] {
        if let array = value?.array { return array.compactMap(\.string) }
        // Some models send a list as one string.
        if let string = value?.string { return string.split { ",; \n[]\"".contains($0) }.map(String.init) }
        return []
    }

    static func origin(_ actor: HarnessDatabase.MemoryActor) -> String {
        switch actor.kind {
        case "importer": "import"
        case "reconciler": "reconciler"
        case "user": "user"
        default: "agent"
        }
    }

    /// Why a memory can't be saved as written, if it can't.
    static func problem(title: String, description: String, body: String) -> String? {
        if title.count > maxTitle { return "Keep the title under \(maxTitle) characters." }
        if description.count > maxDescription { return "Keep the description to one line (under \(maxDescription) characters)." }
        if body.count > maxBody { return "That's too long for one memory (\(body.count) characters; at most \(maxBody)). Split it into smaller ones." }
        if containsSecret(title + "\n" + description + "\n" + body) { return "That looks like it contains a secret (a key, token or password). Memories never keep secrets." }
        return nil
    }

    private static let secretPatterns = [
        #"sk-(ant-)?[A-Za-z0-9_\-]{20,}"#, #"gh[pousr]_[A-Za-z0-9]{30,}"#, #"github_pat_[A-Za-z0-9_]{30,}"#,
        #"AKIA[0-9A-Z]{16}"#, #"xox[baprs]-[A-Za-z0-9\-]{10,}"#, #"-----BEGIN [A-Z ]*PRIVATE KEY-----"#,
        #"AIza[0-9A-Za-z\-_]{35}"#,
    ].compactMap { try? NSRegularExpression(pattern: $0) }

    static func containsSecret(_ text: String) -> Bool {
        let range = NSRange(text.startIndex..., in: text)
        return secretPatterns.contains { $0.firstMatch(in: text, range: range) != nil }
    }

    private static func day(_ date: Date) -> String {
        date.formatted(.iso8601.year().month().day())
    }
}
