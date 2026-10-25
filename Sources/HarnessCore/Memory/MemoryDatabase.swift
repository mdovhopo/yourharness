import Foundation

/// Memories in the database: kept per workspace (`project:<id>`), searchable, linked, with
/// every version and a log of what agents do with them. Writes happen right away, under the
/// write lock: a memory an agent just saved is there for its next search.
extension HarnessDatabase {
    // MARK: - Reading

    public func memory(_ id: String) -> Memory? {
        (try? reader.withLock { try Writer.readMemories($0, where: "id = ?1", [.text(id)], order: "id") })?.first
    }

    /// Memories in `scope` (all of them when nil), newest first.
    public func memories(scope: String? = nil, status: Memory.Status? = nil) -> [Memory] {
        (try? reader.withLock { connection in
            try Writer.readMemories(connection, where: "(?1 IS NULL OR scope = ?1) AND (?2 IS NULL OR status = ?2)",
                                    [.of(scope), .of(status?.rawValue)], order: "updated_at DESC")
        }) ?? []
    }

    /// Memories matching `query` in their title, description or body, best first.
    public func searchMemories(_ query: String, scope: String? = nil, limit: Int = 20) -> [Memory] {
        findMemories(query, scope: scope, statuses: nil, limit: limit).map(\.memory)
    }

    /// Memories matching `query`, best first (a title match counts most), with the match in
    /// context. `statuses` nil means any.
    public func findMemories(_ query: String, scope: String?, statuses: Set<Memory.Status>? = [.active], limit: Int = 20) -> [MemoryHit] {
        guard let match = Self.memoryMatchExpression(query) else { return [] }
        return (try? reader.withLock { connection in
            var ranked: [(String, String)] = []
            try connection.query("""
                SELECT m.id, snippet(memories_fts, 2, '[', ']', '…', 14)
                FROM memories_fts JOIN memories m ON m.rowid = memories_fts.rowid
                WHERE memories_fts MATCH ?1 AND (?2 IS NULL OR m.scope = ?2)
                ORDER BY bm25(memories_fts, 10.0, 4.0, 1.0) LIMIT ?3
                """, [.text(match), .of(scope), .int(Int64(limit * 2))]) { row in
                if let id = row.text(0) { ranked.append((id, row.text(1) ?? "")) }
            }
            let byID = Dictionary(try Writer.readMemories(connection, where: "id IN (\(ranked.map { "'\(Self.quoted($0.0))'" }.joined(separator: ",")))",
                                                          [], order: "id").map { ($0.id, $0) }) { first, _ in first }
            return ranked.compactMap { id, snippet in
                guard let memory = byID[id], statuses?.contains(memory.status) ?? true else { return nil }
                return MemoryHit(memory: memory, snippet: snippet)
            }.prefix(limit).map(\.self)
        }) ?? []
    }

    /// Any word may match (a memory search is a question, not a filter), the last as a prefix.
    static func memoryMatchExpression(_ query: String) -> String? {
        let words = query.split { !$0.isLetter && !$0.isNumber }.map(String.init).filter { $0.count > 1 || $0.first?.isNumber == true }
        guard !words.isEmpty else { return nil }
        return words.enumerated().map { index, word in
            "\"\(word)\"" + (index == words.count - 1 ? "*" : "")
        }.joined(separator: " OR ")
    }

    private static func quoted(_ id: String) -> String { id.replacingOccurrences(of: "'", with: "''") }

    /// The memory in `scope` whose ID is or starts with `prefix` (case doesn't matter), if
    /// exactly one does.
    public func resolveMemoryID(_ prefix: String, scope: String?) -> String? {
        let prefix = prefix.trimmingCharacters(in: .whitespaces).lowercased()
        guard prefix.count >= 4 else { return nil }
        var found: [String] = []
        try? reader.withLock { connection in
            try connection.query("SELECT id FROM memories WHERE lower(id) LIKE ?1 || '%' AND (?2 IS NULL OR scope = ?2) LIMIT 2",
                                 [.text(prefix), .of(scope)]) { if let id = $0.text(0) { found.append(id) } }
        }
        return found.count == 1 ? found[0] : nil
    }

    /// Every save of a memory, oldest first.
    public func memoryVersions(_ id: String) -> [MemoryVersion] {
        var versions: [MemoryVersion] = []
        try? reader.withLock { connection in
            try connection.query("""
                SELECT version, kind, title, description, body, status, saved_at, actor, thread_id, task_state, task_steps
                FROM memory_versions WHERE memory_id = ? ORDER BY version
                """, [.text(id)]) { row in
                versions.append(MemoryVersion(memoryID: id, version: row.int(0) ?? 0, kind: row.text(1) ?? "project",
                                              title: row.text(2) ?? "", description: row.text(3) ?? "", body: row.text(4) ?? "",
                                              status: row.text(5).flatMap(Memory.Status.init) ?? .active,
                                              savedAt: row.date(6) ?? .now, actor: row.text(7), threadID: row.text(8),
                                              taskState: row.text(9).flatMap(TaskState.init), steps: Writer.decodeSteps(row.text(10))))
            }
        }
        return versions
    }

    /// What happened in `scope`'s memory, newest first; `memoryID` keeps to one memory.
    public func memoryEvents(scope: String?, memoryID: String? = nil, kinds: Set<MemoryEvent.Kind>? = nil,
                             since: Date? = nil, limit: Int = 500) -> [MemoryEvent] {
        var events: [MemoryEvent] = []
        let kindList = kinds.map { $0.map { "'\($0.rawValue)'" }.joined(separator: ",") }
        try? reader.withLock { connection in
            try connection.query("""
                SELECT id, scope, memory_id, title, kind, actor, thread_id, at, query, count, detail FROM memory_events
                WHERE (?1 IS NULL OR scope = ?1) AND (?2 IS NULL OR memory_id = ?2) AND (?3 IS NULL OR at >= ?3)
                \(kindList.map { "AND kind IN (\($0))" } ?? "")
                ORDER BY at DESC, id DESC LIMIT ?4
                """, [.of(scope), .of(memoryID), .of(since), .int(Int64(limit))]) { row in
                guard let kind = row.text(4).flatMap(MemoryEvent.Kind.init(rawValue:)) else { return }
                events.append(MemoryEvent(id: row.int(0) ?? 0, scope: row.text(1) ?? "", memoryID: row.text(2), title: row.text(3),
                                          kind: kind, actor: row.text(5), threadID: row.text(6), at: row.date(7) ?? .now,
                                          query: row.text(8), count: row.int(9), detail: row.text(10)))
            }
        }
        return events
    }

    /// Tasks a thread has worked on (it saved or updated them), not archived, newest first.
    public func tasks(forThread threadID: String) -> [Memory] {
        (try? reader.withLock { connection in
            try Writer.readMemories(connection, where: """
                task_state IS NOT NULL AND status != 'archived'
                AND id IN (SELECT memory_id FROM memory_sources WHERE thread_id = ?1)
                """, [.text(threadID)], order: "updated_at DESC")
        }) ?? []
    }

    /// How many active memories each scope has.
    public func memoryCounts() -> [String: Int] {
        var counts: [String: Int] = [:]
        try? reader.withLock { connection in
            try connection.query("SELECT scope, count(*) FROM memories WHERE status = 'active' GROUP BY scope") { row in
                if let scope = row.text(0) { counts[scope] = row.int(1) ?? 0 }
            }
        }
        return counts
    }

    // MARK: - Writing

    /// Saves a memory, new or changed, with its links and sources. A change to what it says
    /// (or its kind or status) keeps a new version; every save is in the activity log.
    public func saveMemory(_ memory: Memory, actor: MemoryActor = .app) {
        try? writer.withLock { writer in
            try writer.connection.transaction { try writer.saveMemory(memory, actor: actor) }
        }
    }

    /// Several at once, in one transaction: an import.
    public func saveMemories(_ memories: [Memory], actor: MemoryActor = .app) {
        try? writer.withLock { writer in
            try writer.connection.transaction {
                // Every memory first, so links between them have both ends.
                for memory in memories { try writer.saveMemory(memory, actor: actor, links: false) }
                for memory in memories { try writer.replaceLinks(memory.id, memory.links) }
            }
        }
    }

    public func setMemoryStatus(_ id: String, _ status: Memory.Status, actor: MemoryActor = .user, reason: String? = nil) {
        try? writer.withLock { writer in
            try writer.connection.transaction {
                guard var memory = try Writer.readMemories(writer.connection, where: "id = ?1", [.text(id)], order: "id").first,
                      memory.status != status else { return }
                let restoring = memory.status == .archived
                memory.status = status
                memory.updatedAt = .now
                try writer.saveMemory(memory, actor: actor, event: status == .archived ? .archived : restoring ? .restored : .updated,
                                      detail: reason)
            }
        }
    }

    public func setMemoryPinned(_ id: String, _ pinned: Bool, actor: MemoryActor = .user) {
        try? writer.withLock { writer in
            try writer.connection.transaction {
                guard let memory = try Writer.readMemories(writer.connection, where: "id = ?1", [.text(id)], order: "id").first,
                      memory.pinned != pinned else { return }
                try writer.connection.run("UPDATE memories SET pinned = ? WHERE id = ?", [.of(pinned), .text(id)])
                try writer.logEvent(scope: memory.scope, memory: memory, kind: pinned ? .pinned : .unpinned, actor: actor)
            }
        }
    }

    /// Gone for good, with its versions and links; the log keeps its title.
    public func deleteMemory(_ id: String, actor: MemoryActor = .user) {
        try? writer.withLock { writer in
            try writer.connection.transaction {
                if let memory = try Writer.readMemories(writer.connection, where: "id = ?1", [.text(id)], order: "id").first {
                    try writer.logEvent(scope: memory.scope, memory: memory, kind: .deleted, actor: actor)
                }
                try writer.connection.run("DELETE FROM memories WHERE id = ?", [.text(id)])
            }
        }
    }

    public func linkMemories(_ from: String, _ to: String, relation: String? = nil, actor: MemoryActor = .app) {
        guard from != to else { return }
        try? writer.withLock { writer in
            try writer.connection.transaction {
                guard let memory = try Writer.readMemories(writer.connection, where: "id = ?1", [.text(from)], order: "id").first,
                      try writer.connection.scalar("SELECT 1 FROM memories WHERE id = ?", [.text(to)]) != nil else { return }
                try writer.connection.run("""
                    INSERT INTO memory_links(from_id, to_id, relation, created_at) VALUES (?, ?, ?, ?)
                    ON CONFLICT(from_id, to_id) DO UPDATE SET relation = excluded.relation
                    """, [.text(from), .text(to), .of(relation), .of(Date.now)])
                try writer.logEvent(scope: memory.scope, memory: memory, kind: .linked, actor: actor, detail: to)
            }
        }
    }

    public func unlinkMemories(_ from: String, _ to: String, actor: MemoryActor = .app) {
        try? writer.withLock { writer in
            try writer.connection.transaction {
                guard let memory = try Writer.readMemories(writer.connection, where: "id = ?1", [.text(from)], order: "id").first else { return }
                try writer.connection.run("DELETE FROM memory_links WHERE (from_id = ?1 AND to_id = ?2) OR (from_id = ?2 AND to_id = ?1)",
                                          [.text(from), .text(to)])
                try writer.logEvent(scope: memory.scope, memory: memory, kind: .unlinked, actor: actor, detail: to)
            }
        }
    }

    /// Folds `ids` into one memory, `merged`: saved under the first ID, with every link and
    /// source of the others. The others are archived, pointing at it.
    @discardableResult
    public func mergeMemories(_ ids: [String], into merged: Memory, actor: MemoryActor = .app) -> Memory? {
        guard let target = ids.first else { return nil }
        return try? writer.withLock { writer in
            try writer.connection.transaction { () -> Memory? in
                let existing = try Writer.readMemories(writer.connection, where: "id IN (\(ids.map { "'\(Self.quoted($0))'" }.joined(separator: ",")))",
                                                       [], order: "created_at")
                guard let first = existing.first(where: { $0.id == target }) else { return nil }
                var result = merged
                result.id = target
                result.scope = first.scope
                result.createdAt = existing.map(\.createdAt).min() ?? first.createdAt
                result.updatedAt = .now
                result.status = .active
                result.mergedInto = nil
                result.recallCount = existing.reduce(0) { $0 + $1.recallCount }
                result.surfacedCount = existing.reduce(0) { $0 + $1.surfacedCount }
                result.lastRecalledAt = existing.compactMap(\.lastRecalledAt).max()
                result.pinned = existing.contains(where: \.pinned) || merged.pinned
                result.origin = first.origin
                result.sources = Array(Set(existing.flatMap(\.sources) + merged.sources)).sorted { $0.entryID < $1.entryID }
                let others = Set(ids.dropFirst())
                var links = merged.links
                for link in existing.flatMap(\.links) where link.to != target && !others.contains(link.to) && !links.contains(where: { $0.to == link.to }) {
                    links.append(link)
                }
                result.links = links
                try writer.saveMemory(result, actor: actor, event: .merged, detail: others.sorted().joined(separator: ","))
                for var other in existing where other.id != target {
                    // Links into the old ones now point at the merged memory.
                    try writer.connection.run("""
                        INSERT OR IGNORE INTO memory_links(from_id, to_id, relation, created_at)
                        SELECT from_id, ?1, relation, created_at FROM memory_links WHERE to_id = ?2 AND from_id != ?1
                        """, [.text(target), .text(other.id)])
                    try writer.connection.run("DELETE FROM memory_links WHERE to_id = ?1 OR from_id = ?1", [.text(other.id)])
                    other.status = .archived
                    other.mergedInto = target
                    other.updatedAt = .now
                    other.links = []
                    try writer.saveMemory(other, actor: actor, event: .archived, detail: "merged into \(target)")
                }
                return try Writer.readMemories(writer.connection, where: "id = ?1", [.text(target)], order: "id").first
            }
        }
    }

    /// Puts a memory back as it was at `version` (a new version, so that's undoable too).
    public func restoreMemoryVersion(_ id: String, version: Int, actor: MemoryActor = .user) {
        guard var memory = memory(id), let old = memoryVersions(id).first(where: { $0.version == version }) else { return }
        memory.kind = old.kind
        memory.title = old.title
        memory.description = old.description
        memory.body = old.body
        memory.status = old.status
        if memory.isTask {
            memory.taskState = old.taskState
            memory.steps = old.steps
        }
        memory.updatedAt = .now
        saveMemory(memory, actor: actor)
    }

    // MARK: - What agents do with them

    /// Agents read these memories in full.
    public func noteMemoryRecall(_ ids: [String], actor: MemoryActor) {
        guard !ids.isEmpty else { return }
        try? writer.withLock { writer in
            try writer.connection.transaction {
                for id in ids {
                    guard let memory = try Writer.readMemories(writer.connection, where: "id = ?1", [.text(id)], order: "id", details: false).first
                    else { continue }
                    try writer.connection.run("UPDATE memories SET recall_count = recall_count + 1, last_recalled_at = ? WHERE id = ?",
                                              [.of(Date.now), .text(id)])
                    try writer.logEvent(scope: memory.scope, memory: memory, kind: .recalled, actor: actor)
                }
            }
        }
    }

    /// An agent searched `scope`'s memory for `query` and got `hits`.
    public func noteMemorySearch(scope: String, query: String, hits: [String], actor: MemoryActor) {
        try? writer.withLock { writer in
            try writer.connection.transaction {
                for id in hits {
                    try writer.connection.run("UPDATE memories SET surfaced_count = surfaced_count + 1 WHERE id = ?", [.text(id)])
                }
                try writer.logEvent(scope: scope, memory: nil, kind: .searched, actor: actor, query: query, count: hits.count,
                                    detail: hits.joined(separator: ","))
            }
        }
    }

    /// A session started with `count` memories in its prompt, `bytes` long.
    public func noteMemoryInjection(scope: String, count: Int, bytes: Int, actor: MemoryActor) {
        recordMemoryEvent(scope: scope, kind: .injected, actor: actor, count: count, detail: "\(bytes)")
    }

    public func recordMemoryEvent(scope: String, kind: MemoryEvent.Kind, memory: Memory? = nil, actor: MemoryActor,
                                  query: String? = nil, count: Int? = nil, detail: String? = nil) {
        try? writer.withLock { writer in
            try writer.logEvent(scope: scope, memory: memory, kind: kind, actor: actor, query: query, count: count, detail: detail)
        }
    }

    /// The log is for learning how memory gets used, not an archive: a year of it is plenty.
    func pruneMemoryEvents(olderThan days: Double = 365) {
        try? writer.withLock { writer in
            try writer.connection.run("DELETE FROM memory_events WHERE at < ?", [.of(Date.now.addingTimeInterval(-days * 86_400))])
        }
    }
}

// MARK: - Writer

extension Writer {
    func saveMemory(_ memory: HarnessDatabase.Memory, actor: HarnessDatabase.MemoryActor, links: Bool = true,
                    event: HarnessDatabase.MemoryEvent.Kind? = nil, detail: String? = nil) throws {
        let old = try Self.readMemories(connection, where: "id = ?1", [.text(memory.id)], order: "id", details: false).first
        try connection.run("""
            INSERT INTO memories(id, scope, kind, title, body, status, created_at, updated_at, rev, description, pinned, origin,
                                 recall_count, surfaced_count, last_recalled_at, merged_into, task_state, task_steps)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET scope = excluded.scope, kind = excluded.kind, title = excluded.title,
                body = excluded.body, status = excluded.status, updated_at = excluded.updated_at, rev = excluded.rev,
                description = excluded.description, pinned = excluded.pinned, origin = coalesce(memories.origin, excluded.origin),
                recall_count = excluded.recall_count, surfaced_count = excluded.surfaced_count,
                last_recalled_at = excluded.last_recalled_at, merged_into = excluded.merged_into,
                task_state = excluded.task_state, task_steps = excluded.task_steps
            """, [.text(memory.id), .text(memory.scope), .text(memory.kind), .text(memory.title), .text(memory.body),
                  .text(memory.status.rawValue), .of(memory.createdAt), .of(memory.updatedAt), .int(Int64(nextRev())),
                  .text(memory.description), .of(memory.pinned), .of(memory.origin), .of(memory.recallCount),
                  .of(memory.surfacedCount), .of(memory.lastRecalledAt), .of(memory.mergedInto),
                  .of(memory.taskState?.rawValue), .of(Self.encodeSteps(memory.steps))])
        try connection.run("DELETE FROM memory_sources WHERE memory_id = ?", [.text(memory.id)])
        for source in memory.sources {
            try connection.run("INSERT OR REPLACE INTO memory_sources(memory_id, thread_id, entry_id, quote) VALUES (?, ?, ?, ?)",
                               [.text(memory.id), .text(source.threadID), .text(source.entryID), .of(source.quote)])
        }
        if links { try replaceLinks(memory.id, memory.links) }

        // Ticking a step, moving a task along or adding to its log is progress, not a rewrite.
        let changed = old.map {
            $0.title != memory.title || $0.description != memory.description || ($0.body != memory.body && !memory.isTask)
                || $0.kind != memory.kind || $0.status != memory.status
        } ?? true
        let progressed = old.map {
            $0.taskState != memory.taskState || $0.steps != memory.steps || ($0.body != memory.body && memory.isTask)
        } ?? false
        if changed || progressed {
            let version = (try connection.scalar("SELECT max(version) FROM memory_versions WHERE memory_id = ?", [.text(memory.id)])?.int ?? 0) + 1
            try connection.run("""
                INSERT INTO memory_versions(memory_id, version, kind, title, description, body, status, saved_at, actor, thread_id,
                                            task_state, task_steps)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """, [.text(memory.id), .int(Int64(version)), .text(memory.kind), .text(memory.title), .text(memory.description),
                      .text(memory.body), .text(memory.status.rawValue), .of(Date.now), .text(actor.kind), .of(actor.threadID),
                      .of(memory.taskState?.rawValue), .of(Self.encodeSteps(memory.steps))])
        }
        if changed || progressed || event != nil {
            let kind = event ?? (old == nil ? .created : changed ? .updated : .progressed)
            let progress = memory.taskState.map { state in
                "\(state.rawValue)" + (memory.steps.isEmpty ? "" : " \(memory.stepsDone)/\(memory.steps.count)")
            }
            try logEvent(scope: memory.scope, memory: memory, kind: kind, actor: actor, detail: detail ?? (kind == .progressed ? progress : nil))
        }
    }

    func replaceLinks(_ id: String, _ links: [HarnessDatabase.MemoryLink]) throws {
        try connection.run("DELETE FROM memory_links WHERE from_id = ?", [.text(id)])
        for link in links where link.to != id {
            // A link to a memory that isn't there (yet, or any more) is dropped.
            try connection.run("""
                INSERT OR IGNORE INTO memory_links(from_id, to_id, relation, created_at)
                SELECT ?, id, ?, ? FROM memories WHERE id = ?
                """, [.text(id), .of(link.relation), .of(Date.now), .text(link.to)])
        }
    }

    func logEvent(scope: String, memory: HarnessDatabase.Memory?, kind: HarnessDatabase.MemoryEvent.Kind,
                  actor: HarnessDatabase.MemoryActor, query: String? = nil, count: Int? = nil, detail: String? = nil) throws {
        try connection.run("""
            INSERT INTO memory_events(scope, memory_id, title, kind, actor, thread_id, at, query, count, detail)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """, [.text(scope), .of(memory?.id), .of(memory?.title), .text(kind.rawValue), .text(actor.kind), .of(actor.threadID),
                  .of(Date.now), .of(query), .of(count), .of(detail)])
    }

    static func encodeSteps(_ steps: [HarnessDatabase.TaskStep]) -> String? {
        steps.isEmpty ? nil : (try? JSONEncoder().encode(steps)).map { String(decoding: $0, as: UTF8.self) }
    }

    static func decodeSteps(_ json: String?) -> [HarnessDatabase.TaskStep] {
        json.flatMap { try? JSONDecoder().decode([HarnessDatabase.TaskStep].self, from: Data($0.utf8)) } ?? []
    }

    static func readMemories(_ connection: SQLiteConnection, where condition: String, _ values: [SQLiteValue],
                             order: String, limit: Int = -1, details: Bool = true) throws -> [HarnessDatabase.Memory] {
        var memories: [HarnessDatabase.Memory] = []
        try connection.query("""
            SELECT id, scope, kind, title, body, status, created_at, updated_at, description, pinned, origin, recall_count,
                   surfaced_count, last_recalled_at, merged_into, task_state, task_steps
            FROM memories WHERE \(condition) ORDER BY \(order) LIMIT \(limit)
            """, values) { row in
            guard let id = row.text(0) else { return }
            memories.append(.init(id: id, scope: row.text(1) ?? "global", kind: row.text(2) ?? "project", title: row.text(3) ?? "",
                                  description: row.text(8) ?? "", body: row.text(4) ?? "",
                                  status: row.text(5).flatMap(HarnessDatabase.Memory.Status.init) ?? .active,
                                  pinned: (row.int(9) ?? 0) != 0, origin: row.text(10),
                                  createdAt: row.date(6) ?? .now, updatedAt: row.date(7) ?? .now,
                                  recallCount: row.int(11) ?? 0, surfacedCount: row.int(12) ?? 0,
                                  lastRecalledAt: row.date(13), mergedInto: row.text(14),
                                  taskState: row.text(15).flatMap(HarnessDatabase.TaskState.init), steps: decodeSteps(row.text(16))))
        }
        guard details, !memories.isEmpty else { return memories }
        for index in memories.indices {
            let id = memories[index].id
            try connection.query("SELECT thread_id, entry_id, quote FROM memory_sources WHERE memory_id = ?", [.text(id)]) { row in
                memories[index].sources.append(.init(threadID: row.text(0) ?? "", entryID: row.text(1) ?? "", quote: row.text(2)))
            }
            try connection.query("SELECT to_id, relation FROM memory_links WHERE from_id = ? ORDER BY created_at, to_id", [.text(id)]) { row in
                if let to = row.text(0) { memories[index].links.append(.init(to: to, relation: row.text(1))) }
            }
        }
        return memories
    }
}
