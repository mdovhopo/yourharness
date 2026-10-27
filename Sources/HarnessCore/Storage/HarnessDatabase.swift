import CryptoKit
import Foundation
import Synchronization

/// Everything the app keeps, in one SQLite file (`harness.sqlite`): settings and caches,
/// projects, threads, transcripts, big tool payloads, a full-text index over every
/// transcript, and memories.
///
/// Built to grow: saves write only the rows that changed, on a background queue, and
/// coalesce while one is in flight; big tool inputs and results are stored once, compressed,
/// and a big result is loaded only as its head. Every transcript row write takes the next
/// revision, so indexers (the full-text index today, memory later) follow a change feed
/// from a bookmark instead of rereading history.
///
/// The first launch moves the old JSON files in: the state right away, each transcript the
/// first time it's opened or soon after in the background. The files are left as they were.
public final class HarnessDatabase: Sendable {
    // MARK: - Rows

    /// The app's state, split so a change to one thing writes one row.
    public struct StateRows: Sendable, Equatable {
        /// Each top-level setting or cache as JSON, by its key.
        public var values: [String: Data] = [:]
        /// In order.
        public var projects: [ProjectRow] = []
        /// In order.
        public var threads: [ThreadRow] = []

        public init(values: [String: Data] = [:], projects: [ProjectRow] = [], threads: [ThreadRow] = []) {
            self.values = values
            self.projects = projects
            self.threads = threads
        }
    }

    public struct ProjectRow: Sendable, Equatable {
        public var id: String
        public var json: Data

        public init(id: String, json: Data) {
            self.id = id
            self.json = json
        }
    }

    /// A thread's metadata as JSON, plus what queries need to find it by.
    public struct ThreadRow: Sendable, Equatable {
        public var id: String
        public var projectID: String
        public var updatedAt: Date?
        public var sessionID: String?
        public var json: Data

        public init(id: String, projectID: String, updatedAt: Date?, sessionID: String?, json: Data) {
            self.id = id
            self.projectID = projectID
            self.updatedAt = updatedAt
            self.sessionID = sessionID
            self.json = json
        }
    }

    /// A transcript row written since some revision: what an indexer reads.
    public struct EntryChange: Sendable, Equatable {
        public var rowID: Int
        public var threadID: String
        public var seq: Int
        public var entryID: String
        public var kind: EntryKind
        public var text: String
        public var toolName: String?
        /// The tool input's first few kilobytes.
        public var toolInput: String?
        /// The tool result's first few kilobytes.
        public var toolResult: String?
        public var rev: Int
    }

    public struct SearchHit: Sendable, Equatable {
        public var threadID: String
        public var seq: Int
        public var entryID: String
        public var kind: EntryKind
        /// The match in context, with `[` `]` around the matched words.
        public var snippet: String
    }

    // MARK: - Tuning

    /// Tool inputs, results and sub-agent runs bigger than this (UTF-8 bytes) go in `blobs`,
    /// compressed and stored once however many rows point at them.
    public static let blobThreshold = 32 * 1024
    /// How much of a big tool result is kept in the row and loaded with the transcript:
    /// as much as the transcript shows.
    public static let resultHeadLength = 20_000
    /// Results kept whole in memory however big: a sub-agent's report is read from its result.
    public static let wholeResultTools = SubagentReport.toolNames
    static let schemaVersion = 3
    private static let indexBatch = 500

    // MARK: - Setup

    public let url: URL
    /// Set when the database couldn't be read and was moved aside for a fresh one.
    public let recoveredFrom: URL?

    let writer: Mutex<Writer>
    let reader: Mutex<SQLiteConnection>
    private let pending = Mutex(Pending())
    /// Old transcripts still to move in, by thread ID.
    private let legacyPending: Mutex<Set<String>>
    private let legacy: LegacyStore?
    private let queue = DispatchQueue(label: "yourharness.database", qos: .utility)

    /// Opens (or creates) `harness.sqlite` in `directory`. With `importLegacy`, the old JSON
    /// files in that folder are moved in the first time.
    public init(directory: URL, importLegacy: Bool = true) {
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appending(path: "harness.sqlite")
        let legacy = importLegacy ? LegacyStore(root: directory) : nil
        var recoveredFrom: URL?
        var opened: (Writer, SQLiteConnection)
        do {
            opened = try Self.open(url, legacy: legacy)
        } catch {
            // Unreadable: kept aside for a look, and the app starts over from the old files.
            let aside = directory.appending(path: "harness.sqlite.unreadable-\(Int(Date.now.timeIntervalSince1970))")
            FileHandle.standardError.write(Data("Database didn't open (\(error)); moved to \(aside.lastPathComponent)\n".utf8))
            try? FileManager.default.moveItem(at: url, to: aside)
            for suffix in ["-wal", "-shm"] {
                try? FileManager.default.removeItem(at: URL(fileURLWithPath: url.path + suffix))
            }
            recoveredFrom = aside
            do {
                opened = try Self.open(url, legacy: legacy)
            } catch {
                // Nowhere to write: a throwaway file, so the app still runs.
                let temporary = FileManager.default.temporaryDirectory.appending(path: "yourharness-\(UUID().uuidString).sqlite")
                FileHandle.standardError.write(Data("Database still didn't open (\(error)); using \(temporary.path)\n".utf8))
                opened = try! Self.open(temporary, legacy: nil)
            }
        }
        self.url = url
        self.recoveredFrom = recoveredFrom
        self.legacy = legacy
        legacyPending = Mutex(opened.0.unimportedLegacyTranscripts())
        writer = Mutex(opened.0)
        reader = Mutex(opened.1)
        // Launch first; old transcripts and the index catch up in the background.
        queue.asyncAfter(deadline: .now() + 3) { [weak self] in
            self?.importNextLegacyTranscript()
            self?.indexBatch()
            self?.collectGarbage()
            self?.pruneMemoryEvents()
        }
    }

    private static func open(_ url: URL, legacy: LegacyStore?) throws -> (Writer, SQLiteConnection) {
        let connection = try SQLiteConnection(url: url)
        try connection.execute("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")
        try Schema.migrate(connection)
        let writer = try Writer(connection: connection, legacy: legacy)
        let reader = try SQLiteConnection(url: url)
        try reader.execute("PRAGMA query_only = 1; PRAGMA foreign_keys = ON;")
        return (writer, reader)
    }

    // MARK: - State

    /// Settings, projects and threads as last saved, or as moved in from `state.json`.
    public func loadState() -> StateRows {
        if let state = pending.withLock({ $0.state ?? $0.inFlightState }) { return state }
        return (try? reader.withLock { try Writer.readState($0) }) ?? StateRows()
    }

    /// Writes the rows that changed since the last save, in the background. Saves made
    /// while one is being written collapse into the latest.
    public func saveState(_ state: StateRows) {
        pending.withLock { $0.state = state }
        scheduleDrain()
    }

    // MARK: - Transcripts

    /// A thread's transcript, oldest first. Big tool results come as their head, with
    /// `toolResultRef` set (see `fullToolResult`).
    public func loadTranscript(_ id: UUID) -> [EntrySnapshot] {
        let key = id.uuidString
        if let waiting = pending.withLock({ $0.deletes.contains(key) ? [] : $0.transcripts[key] ?? $0.inFlightTranscripts[key] }) {
            return waiting
        }
        importLegacyTranscriptNow(key)
        return (try? reader.withLock { try Writer.readEntries($0, thread: key) }) ?? []
    }

    /// The transcript in pages of `pageSize`, for going through a long one without holding
    /// all of it.
    public func scanTranscript(_ id: UUID, pageSize: Int = 500, _ body: ([EntrySnapshot]) -> Void) {
        let key = id.uuidString
        if let waiting = pending.withLock({ $0.transcripts[key] ?? $0.inFlightTranscripts[key] }) {
            stride(from: 0, to: waiting.count, by: pageSize).forEach { body(Array(waiting[$0..<min($0 + pageSize, waiting.count)])) }
            return
        }
        importLegacyTranscriptNow(key)
        var from = 0
        while true {
            let page = (try? reader.withLock { try Writer.readEntries($0, thread: key, from: from, limit: pageSize) }) ?? []
            if page.isEmpty { return }
            body(page)
            from += page.count
            if page.count < pageSize { return }
        }
    }

    /// Writes the rows that changed since the last save, in the background. A thread's
    /// saves made while one is being written collapse into the latest.
    public func saveTranscript(_ id: UUID, entries: [EntrySnapshot]) {
        pending.withLock {
            $0.transcripts[id.uuidString] = entries
            $0.deletes.remove(id.uuidString)
        }
        scheduleDrain()
    }

    /// The transcript was let go of in memory: so is what the last save compared against.
    public func forgetTranscript(_ id: UUID) {
        pending.withLock { _ = $0.forgets.insert(id.uuidString) }
        scheduleDrain()
    }

    /// Removes a transcript, its index entries and blobs only it used, and the old JSON file.
    public func deleteTranscript(_ id: UUID) {
        pending.withLock {
            $0.transcripts[id.uuidString] = nil
            $0.deletes.insert(id.uuidString)
        }
        scheduleDrain()
    }

    /// When the transcript was last written.
    public func transcriptSavedAt(_ id: UUID) -> Date? {
        let key = id.uuidString
        if legacyPending.withLock({ $0.contains(key) }) { return legacy?.transcriptModified(key) }
        return try? reader.withLock { try $0.scalar("SELECT saved_at FROM transcripts WHERE thread_id = ?", [.text(key)]) }
            .flatMap { if case .double(let value) = $0 { Date(timeIntervalSince1970: value) } else { nil } }
    }

    /// A big tool result in full, by its `toolResultRef`.
    public func fullToolResult(_ ref: String) -> String? {
        try? reader.withLock { try Writer.blobString($0, ref) }
    }

    /// Waits until everything saved so far is written; at quit, so nothing is cut off.
    public func flush() {
        queue.sync { drain() }
    }

    /// Everything as one compact file, as of now: what's waiting is written first, and
    /// nothing else is written while it's copied. For exporting the state.
    public func snapshot(to file: URL) throws {
        try queue.sync {
            drain()
            try writer.withLock { try $0.connection.run("VACUUM INTO ?", [.text(file.path)]) }
        }
    }

    // MARK: - Change feed and search

    /// Transcript rows written after `rev`, oldest change first: for an indexer to follow.
    public func changes(after rev: Int, limit: Int = 500) -> [EntryChange] {
        (try? reader.withLock { try Writer.changes($0, after: rev, limit: limit) }) ?? []
    }

    /// How far an indexer has got in the change feed.
    public func bookmark(_ indexer: String) -> Int {
        (try? reader.withLock { try $0.scalar("SELECT rev FROM index_state WHERE indexer = ?", [.text(indexer)]) })?.int ?? 0
    }

    public func setBookmark(_ indexer: String, rev: Int) {
        try? writer.withLock { try $0.setBookmark(indexer, rev: rev) }
    }

    /// The newest revision written.
    public var currentRev: Int { writer.withLock { $0.rev } }

    /// Transcript rows matching `query` (words, the last one as a prefix), best first.
    /// `projectID` keeps to that project's threads.
    public func search(_ query: String, projectID: String? = nil, limit: Int = 50) -> [SearchHit] {
        guard let match = Self.matchExpression(query) else { return [] }
        return (try? reader.withLock { connection in
            var hits: [SearchHit] = []
            try connection.query("""
                SELECT e.thread_id, e.seq, e.id, e.kind, snippet(entries_fts, 0, '[', ']', '…', 12)
                FROM entries_fts JOIN entries e ON e.rowid = entries_fts.rowid
                WHERE entries_fts MATCH ?1
                  AND (?2 IS NULL OR e.thread_id IN (SELECT id FROM threads WHERE project_id = ?2))
                ORDER BY bm25(entries_fts) LIMIT ?3
                """, [.text(match), .of(projectID), .int(Int64(limit))]) { row in
                guard let kind = row.text(3).flatMap(EntryKind.init(rawValue:)) else { return }
                hits.append(SearchHit(threadID: row.text(0) ?? "", seq: row.int(1) ?? 0, entryID: row.text(2) ?? "",
                                      kind: kind, snippet: row.text(4) ?? ""))
            }
            return hits
        }) ?? []
    }

    /// Indexes everything written so far, right now: for tests and probes. The app leaves
    /// it to the background.
    public func catchUpIndex() {
        queue.sync {
            drain()
            while (try? writer.withLock({ try $0.indexBatch(limit: Self.indexBatch) })) == true {}
        }
    }

    /// Moves every old transcript in, right now: for tests. The app does it in the background.
    public func importAllLegacyTranscripts() {
        for key in legacyPending.withLock({ Array($0) }) { importLegacyTranscriptNow(key) }
    }

    /// FTS5 syntax for what was typed: each word quoted (so punctuation can't break the
    /// query), all of them required, the last as a prefix.
    static func matchExpression(_ query: String) -> String? {
        let words = query.split { !$0.isLetter && !$0.isNumber }.map(String.init)
        guard !words.isEmpty else { return nil }
        return words.enumerated().map { index, word in
            "\"\(word)\"" + (index == words.count - 1 ? "*" : "")
        }.joined(separator: " ")
    }

    // MARK: - Writing

    private struct Pending {
        var state: StateRows?
        var transcripts: [String: [EntrySnapshot]] = [:]
        var deletes: Set<String> = []
        var forgets: Set<String> = []
        /// Being written: still what a load should see until it's committed.
        var inFlightState: StateRows?
        var inFlightTranscripts: [String: [EntrySnapshot]] = [:]
        var drainScheduled = false
        var indexScheduled = false
    }

    private func scheduleDrain() {
        let schedule = pending.withLock { pending in
            defer { pending.drainScheduled = true }
            return !pending.drainScheduled
        }
        if schedule { queue.async { [weak self] in self?.drain() } }
    }

    /// Writes what's waiting, in one transaction. Runs on `queue`.
    private func drain() {
        let (state, transcripts, deletes, forgets) = pending.withLock { pending in
            defer {
                pending.inFlightState = pending.state
                pending.inFlightTranscripts = pending.transcripts
                pending.state = nil
                pending.transcripts = [:]
                pending.deletes = []
                pending.forgets = []
                pending.drainScheduled = false
            }
            return (pending.state, pending.transcripts, pending.deletes, pending.forgets)
        }
        guard state != nil || !transcripts.isEmpty || !deletes.isEmpty || !forgets.isEmpty else { return }
        for key in Set(transcripts.keys).subtracting(deletes) { importLegacyTranscriptNow(key, onQueue: true) }
        var wroteEntries = false
        do {
            try writer.withLock { writer in
                try writer.connection.transaction {
                    if let state { try writer.writeState(state) }
                    for (key, entries) in transcripts where !deletes.contains(key) {
                        if try writer.writeTranscript(key, entries) { wroteEntries = true }
                    }
                    for key in deletes { try writer.deleteTranscript(key) }
                }
                for key in forgets.union(deletes) { writer.baselines[key] = nil }
                writer.collectReplacedBlobs()
            }
            for key in deletes {
                legacy?.deleteTranscript(key)
                legacyPending.withLock { _ = $0.remove(key) }
            }
        } catch {
            FileHandle.standardError.write(Data("Database write failed: \(error)\n".utf8))
            // Kept for the next try, unless something newer has come in meanwhile.
            pending.withLock { pending in
                if pending.state == nil { pending.state = state }
                for (key, entries) in transcripts where pending.transcripts[key] == nil { pending.transcripts[key] = entries }
                pending.deletes.formUnion(deletes)
            }
            queue.asyncAfter(deadline: .now() + 2) { [weak self] in self?.drain() }
        }
        pending.withLock { pending in
            pending.inFlightState = nil
            pending.inFlightTranscripts = [:]
        }
        if wroteEntries || !deletes.isEmpty { scheduleIndexing() }
    }

    private func scheduleIndexing() {
        let schedule = pending.withLock { pending in
            defer { pending.indexScheduled = true }
            return !pending.indexScheduled
        }
        // A moment after the writes stop: a streaming reply rewrites its row every save.
        if schedule { queue.asyncAfter(deadline: .now() + 3) { [weak self] in self?.indexBatch() } }
    }

    /// One batch of the full-text index, then the next after anything else waiting.
    private func indexBatch() {
        pending.withLock { $0.indexScheduled = false }
        let more = (try? writer.withLock { try $0.indexBatch(limit: Self.indexBatch) }) ?? false
        if more { queue.async { [weak self] in self?.indexBatch() } }
    }

    private func collectGarbage() {
        try? writer.withLock { try $0.collectGarbage() }
    }

    // MARK: - Old files

    /// One old transcript per turn of the queue, so saves get in between.
    private func importNextLegacyTranscript() {
        guard let key = legacyPending.withLock({ $0.first }) else { return }
        importLegacyTranscriptNow(key, onQueue: true)
        queue.async { [weak self] in self?.importNextLegacyTranscript() }
    }

    private func importLegacyTranscriptNow(_ key: String, onQueue: Bool = false) {
        guard let legacy, legacyPending.withLock({ $0.contains(key) }) else { return }
        let work = {
            guard self.legacyPending.withLock({ $0.contains(key) }) else { return }
            do {
                try self.writer.withLock { try $0.importLegacyTranscript(key, from: legacy) }
                self.legacyPending.withLock { _ = $0.remove(key) }
            } catch {
                FileHandle.standardError.write(Data("Couldn't move transcript \(key) into the database: \(error)\n".utf8))
            }
        }
        if onQueue { work() } else { queue.sync(execute: work) }
        scheduleIndexing()
    }
}

// MARK: - Schema

enum Schema {
    /// Each step takes the schema from one version to the next. Append, never edit.
    static let steps: [String] = [
        // 1: the first database.
        """
        CREATE TABLE settings (
            key TEXT PRIMARY KEY,
            value BLOB NOT NULL
        ) WITHOUT ROWID;

        CREATE TABLE projects (
            id TEXT PRIMARY KEY,
            position INTEGER NOT NULL,
            json BLOB NOT NULL
        ) WITHOUT ROWID;

        CREATE TABLE threads (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            updated_at REAL,
            session_id TEXT,
            json BLOB NOT NULL
        );
        CREATE INDEX threads_project ON threads(project_id);
        CREATE INDEX threads_session ON threads(session_id) WHERE session_id IS NOT NULL;

        CREATE TABLE entries (
            thread_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            id TEXT NOT NULL,
            kind TEXT NOT NULL,
            text TEXT NOT NULL,
            tool_name TEXT,
            tool_input TEXT,
            tool_input_blob TEXT,
            tool_result TEXT,
            tool_result_blob TEXT,
            tool_result_length INTEGER,
            tool_is_error INTEGER,
            notice_style TEXT,
            attachments TEXT,
            delivery TEXT,
            date REAL,
            turn_ended_at REAL,
            subagent BLOB,
            subagent_blob TEXT,
            rev INTEGER NOT NULL,
            UNIQUE (thread_id, seq)
        );
        CREATE INDEX entries_rev ON entries(rev);
        CREATE INDEX entries_entry ON entries(thread_id, id);
        CREATE INDEX entries_input_blob ON entries(tool_input_blob) WHERE tool_input_blob IS NOT NULL;
        CREATE INDEX entries_result_blob ON entries(tool_result_blob) WHERE tool_result_blob IS NOT NULL;
        CREATE INDEX entries_subagent_blob ON entries(subagent_blob) WHERE subagent_blob IS NOT NULL;

        CREATE TABLE transcripts (
            thread_id TEXT PRIMARY KEY,
            saved_at REAL NOT NULL,
            entry_count INTEGER NOT NULL
        ) WITHOUT ROWID;

        CREATE TABLE blobs (
            hash TEXT PRIMARY KEY,
            size INTEGER NOT NULL,
            codec INTEGER NOT NULL,
            data BLOB NOT NULL
        ) WITHOUT ROWID;

        CREATE VIRTUAL TABLE entries_fts USING fts5(body, tokenize = 'unicode61 remove_diacritics 2');

        CREATE TABLE index_state (
            indexer TEXT PRIMARY KEY,
            rev INTEGER NOT NULL,
            updated_at REAL
        ) WITHOUT ROWID;

        CREATE TABLE legacy_imports (
            name TEXT PRIMARY KEY,
            imported_at REAL NOT NULL
        ) WITHOUT ROWID;

        CREATE TABLE memories (
            id TEXT PRIMARY KEY,
            scope TEXT NOT NULL,
            kind TEXT NOT NULL,
            title TEXT NOT NULL,
            body TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at REAL NOT NULL,
            updated_at REAL NOT NULL,
            rev INTEGER NOT NULL
        );
        CREATE INDEX memories_scope ON memories(scope, status);

        CREATE TABLE memory_sources (
            memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            thread_id TEXT NOT NULL,
            entry_id TEXT NOT NULL,
            quote TEXT,
            PRIMARY KEY (memory_id, thread_id, entry_id)
        ) WITHOUT ROWID;
        CREATE INDEX memory_sources_thread ON memory_sources(thread_id);

        CREATE VIRTUAL TABLE memories_fts USING fts5(title, body, content = 'memories', content_rowid = 'rowid',
                                                     tokenize = 'unicode61 remove_diacritics 2');
        CREATE TRIGGER memories_fts_insert AFTER INSERT ON memories BEGIN
            INSERT INTO memories_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
        END;
        CREATE TRIGGER memories_fts_delete AFTER DELETE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, title, body) VALUES ('delete', old.rowid, old.title, old.body);
        END;
        CREATE TRIGGER memories_fts_update AFTER UPDATE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, title, body) VALUES ('delete', old.rowid, old.title, old.body);
            INSERT INTO memories_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
        END;
        """,
        // 2: memory you can explore: a one-line description, pins, where each came from,
        // links between memories, every version, and what agents do with them.
        """
        ALTER TABLE memories ADD COLUMN description TEXT NOT NULL DEFAULT '';
        ALTER TABLE memories ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE memories ADD COLUMN origin TEXT;
        ALTER TABLE memories ADD COLUMN recall_count INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE memories ADD COLUMN surfaced_count INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE memories ADD COLUMN last_recalled_at REAL;
        ALTER TABLE memories ADD COLUMN merged_into TEXT;

        CREATE TABLE memory_links (
            from_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            to_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            relation TEXT,
            created_at REAL NOT NULL,
            PRIMARY KEY (from_id, to_id)
        ) WITHOUT ROWID;
        CREATE INDEX memory_links_to ON memory_links(to_id);

        CREATE TABLE memory_versions (
            memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            version INTEGER NOT NULL,
            kind TEXT NOT NULL,
            title TEXT NOT NULL,
            description TEXT NOT NULL,
            body TEXT NOT NULL,
            status TEXT NOT NULL,
            saved_at REAL NOT NULL,
            actor TEXT,
            thread_id TEXT,
            PRIMARY KEY (memory_id, version)
        ) WITHOUT ROWID;

        CREATE TABLE memory_events (
            id INTEGER PRIMARY KEY,
            scope TEXT NOT NULL,
            memory_id TEXT,
            title TEXT,
            kind TEXT NOT NULL,
            actor TEXT,
            thread_id TEXT,
            at REAL NOT NULL,
            query TEXT,
            count INTEGER,
            detail TEXT
        );
        CREATE INDEX memory_events_scope ON memory_events(scope, at);
        CREATE INDEX memory_events_memory ON memory_events(memory_id, at) WHERE memory_id IS NOT NULL;

        DROP TRIGGER memories_fts_insert;
        DROP TRIGGER memories_fts_delete;
        DROP TRIGGER memories_fts_update;
        DROP TABLE memories_fts;
        CREATE VIRTUAL TABLE memories_fts USING fts5(title, description, body, content = 'memories', content_rowid = 'rowid',
                                                     tokenize = 'unicode61 remove_diacritics 2');
        CREATE TRIGGER memories_fts_insert AFTER INSERT ON memories BEGIN
            INSERT INTO memories_fts(rowid, title, description, body) VALUES (new.rowid, new.title, new.description, new.body);
        END;
        CREATE TRIGGER memories_fts_delete AFTER DELETE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, title, description, body)
            VALUES ('delete', old.rowid, old.title, old.description, old.body);
        END;
        CREATE TRIGGER memories_fts_update AFTER UPDATE OF title, description, body ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, title, description, body)
            VALUES ('delete', old.rowid, old.title, old.description, old.body);
            INSERT INTO memories_fts(rowid, title, description, body) VALUES (new.rowid, new.title, new.description, new.body);
        END;
        INSERT INTO memories_fts(memories_fts) VALUES ('rebuild');
        """,
        // 3: tasks, the user's asks and how far they've got.
        """
        ALTER TABLE memories ADD COLUMN task_state TEXT;
        ALTER TABLE memories ADD COLUMN task_steps TEXT;
        ALTER TABLE memory_versions ADD COLUMN task_state TEXT;
        ALTER TABLE memory_versions ADD COLUMN task_steps TEXT;
        CREATE INDEX memories_tasks ON memories(scope, task_state) WHERE task_state IS NOT NULL;
        """,
    ]

    static func migrate(_ connection: SQLiteConnection) throws {
        let version = try connection.scalar("PRAGMA user_version")?.int ?? 0
        guard version < steps.count else {
            if version > steps.count {
                // A newer app's database: what it added is left alone, and the rest still works.
                FileHandle.standardError.write(Data("Database schema \(version) is newer than this app's \(steps.count)\n".utf8))
            }
            return
        }
        try connection.transaction {
            for step in steps[version...] { try connection.execute(step) }
            try connection.execute("PRAGMA user_version = \(steps.count)")
        }
    }
}

// MARK: - Writer

/// The write connection and what it remembers between saves. Only used under its mutex.
final class Writer: @unchecked Sendable {
    let connection: SQLiteConnection
    let legacy: LegacyStore?
    /// The last revision handed out.
    var rev: Int
    /// What each transcript's last save wrote, to compare the next one against. Shares its
    /// strings with the app's rows, so it costs little; dropped when the app lets go.
    var baselines: [String: [EntrySnapshot]] = [:]
    private var stateBaseline: HarnessDatabase.StateRows
    /// Blobs that rows stopped pointing at in the last write, to delete if nothing else does.
    private var replacedBlobs: Set<String> = []

    init(connection: SQLiteConnection, legacy: LegacyStore?) throws {
        self.connection = connection
        self.legacy = legacy
        rev = try connection.scalar("SELECT max(coalesce((SELECT max(rev) FROM entries), 0), coalesce((SELECT max(rev) FROM memories), 0))")?.int ?? 0
        stateBaseline = HarnessDatabase.StateRows()
        try importLegacyState()
        stateBaseline = try Self.readState(connection)
    }

    func nextRev() -> Int {
        rev += 1
        return rev
    }

    // MARK: State

    static func readState(_ connection: SQLiteConnection) throws -> HarnessDatabase.StateRows {
        var state = HarnessDatabase.StateRows()
        try connection.query("SELECT key, value FROM settings") { row in
            if let key = row.text(0), let value = row.data(1) { state.values[key] = value }
        }
        try connection.query("SELECT id, json FROM projects ORDER BY position") { row in
            if let id = row.text(0), let json = row.data(1) { state.projects.append(.init(id: id, json: json)) }
        }
        try connection.query("SELECT id, project_id, updated_at, session_id, json FROM threads ORDER BY position") { row in
            guard let id = row.text(0), let project = row.text(1), let json = row.data(4) else { return }
            state.threads.append(.init(id: id, projectID: project, updatedAt: row.date(2), sessionID: row.text(3), json: json))
        }
        return state
    }

    func writeState(_ state: HarnessDatabase.StateRows) throws {
        let old = stateBaseline
        for (key, value) in state.values where old.values[key] != value {
            try connection.run("INSERT INTO settings(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                               [.text(key), .blob(value)])
        }
        for key in old.values.keys where state.values[key] == nil {
            try connection.run("DELETE FROM settings WHERE key = ?", [.text(key)])
        }

        let oldProjects = Dictionary(old.projects.enumerated().map { ($1.id, ($0, $1)) }) { first, _ in first }
        for (position, project) in state.projects.enumerated() {
            if let old = oldProjects[project.id], old.0 == position, old.1 == project { continue }
            try connection.run("""
                INSERT INTO projects(id, position, json) VALUES (?, ?, ?)
                ON CONFLICT(id) DO UPDATE SET position = excluded.position, json = excluded.json
                """, [.text(project.id), .int(Int64(position)), .blob(project.json)])
        }
        let projectIDs = Set(state.projects.map(\.id))
        for project in old.projects where !projectIDs.contains(project.id) {
            try connection.run("DELETE FROM projects WHERE id = ?", [.text(project.id)])
        }

        let oldThreads = Dictionary(old.threads.enumerated().map { ($1.id, ($0, $1)) }) { first, _ in first }
        for (position, thread) in state.threads.enumerated() {
            if let old = oldThreads[thread.id], old.0 == position, old.1 == thread { continue }
            try connection.run("""
                INSERT INTO threads(id, project_id, position, updated_at, session_id, json) VALUES (?, ?, ?, ?, ?, ?)
                ON CONFLICT(id) DO UPDATE SET project_id = excluded.project_id, position = excluded.position,
                    updated_at = excluded.updated_at, session_id = excluded.session_id, json = excluded.json
                """, [.text(thread.id), .text(thread.projectID), .int(Int64(position)), .of(thread.updatedAt),
                      .of(thread.sessionID), .blob(thread.json)])
        }
        let threadIDs = Set(state.threads.map(\.id))
        for thread in old.threads where !threadIDs.contains(thread.id) {
            // The transcript stays until it's deleted: an undone delete brings the thread back.
            try connection.run("DELETE FROM threads WHERE id = ?", [.text(thread.id)])
        }
        stateBaseline = state
    }

    // MARK: Transcripts

    /// Writes the rows that differ from the last save. True if any did.
    func writeTranscript(_ thread: String, _ entries: [EntrySnapshot]) throws -> Bool {
        let baseline = try baselines[thread] ?? Self.readEntries(connection, thread: thread)
        var wrote = false
        for (seq, entry) in entries.enumerated() where seq >= baseline.count || baseline[seq] != entry {
            try upsert(thread, seq: seq, entry)
            wrote = true
        }
        if baseline.count > entries.count {
            try truncate(thread, from: entries.count)
            wrote = true
        }
        try connection.run("""
            INSERT INTO transcripts(thread_id, saved_at, entry_count) VALUES (?, ?, ?)
            ON CONFLICT(thread_id) DO UPDATE SET saved_at = excluded.saved_at, entry_count = excluded.entry_count
            """, [.text(thread), .of(Date.now), .int(Int64(entries.count))])
        baselines[thread] = entries
        return wrote
    }

    private func truncate(_ thread: String, from seq: Int) throws {
        try noteBlobs(thread, where: "seq >= ?", .int(Int64(seq)))
        try connection.run("DELETE FROM entries_fts WHERE rowid IN (SELECT rowid FROM entries WHERE thread_id = ? AND seq >= ?)",
                           [.text(thread), .int(Int64(seq))])
        try connection.run("DELETE FROM entries WHERE thread_id = ? AND seq >= ?", [.text(thread), .int(Int64(seq))])
    }

    func deleteTranscript(_ thread: String) throws {
        try truncate(thread, from: 0)
        try connection.run("DELETE FROM transcripts WHERE thread_id = ?", [.text(thread)])
        try markImported(thread)
    }

    private func upsert(_ thread: String, seq: Int, _ entry: EntrySnapshot) throws {
        try noteBlobs(thread, where: "seq = ?", .int(Int64(seq)))

        var input: String? = entry.toolInput, inputBlob: String?
        if let toolInput = entry.toolInput, toolInput.utf8.count > HarnessDatabase.blobThreshold {
            inputBlob = try storeBlob(Data(toolInput.utf8))
            input = nil
        }

        var result = entry.toolResult, resultBlob = entry.toolResultRef, resultLength = entry.toolResultLength
        if resultBlob == nil, let toolResult = entry.toolResult, toolResult.utf8.count > HarnessDatabase.blobThreshold {
            resultBlob = try storeBlob(Data(toolResult.utf8))
            resultLength = toolResult.count
            result = String(toolResult.prefix(HarnessDatabase.resultHeadLength))
        }

        var subagent: Data?, subagentBlob: String?
        if let run = entry.subagent {
            let json = try Self.encoder.encode(run)
            if json.count > HarnessDatabase.blobThreshold { subagentBlob = try storeBlob(json) } else { subagent = json }
        }

        let attachments = try entry.attachments.map { String(decoding: try JSONEncoder().encode($0), as: UTF8.self) }
        try connection.run("""
            INSERT INTO entries(thread_id, seq, id, kind, text, tool_name, tool_input, tool_input_blob, tool_result,
                                tool_result_blob, tool_result_length, tool_is_error, notice_style, attachments, delivery,
                                date, turn_ended_at, subagent, subagent_blob, rev)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(thread_id, seq) DO UPDATE SET id = excluded.id, kind = excluded.kind, text = excluded.text,
                tool_name = excluded.tool_name, tool_input = excluded.tool_input, tool_input_blob = excluded.tool_input_blob,
                tool_result = excluded.tool_result, tool_result_blob = excluded.tool_result_blob,
                tool_result_length = excluded.tool_result_length, tool_is_error = excluded.tool_is_error,
                notice_style = excluded.notice_style, attachments = excluded.attachments, delivery = excluded.delivery,
                date = excluded.date, turn_ended_at = excluded.turn_ended_at, subagent = excluded.subagent,
                subagent_blob = excluded.subagent_blob, rev = excluded.rev
            """, [
                .text(thread), .int(Int64(seq)), .text(entry.id), .text(entry.kind.rawValue), .text(entry.text),
                .of(entry.toolName), .of(input), .of(inputBlob), .of(result), .of(resultBlob), .of(resultLength),
                .of(entry.toolIsError), .of(entry.noticeStyle?.rawValue), .of(attachments), .of(entry.delivery?.rawValue),
                .of(entry.date), .of(entry.turnEndedAt), .of(subagent), .of(subagentBlob), .int(Int64(nextRev())),
            ])
    }

    static func readEntries(_ connection: SQLiteConnection, thread: String, from: Int = 0, limit: Int = -1) throws -> [EntrySnapshot] {
        var entries: [EntrySnapshot] = []
        try connection.query("""
            SELECT id, kind, text, tool_name, tool_input, tool_input_blob, tool_result, tool_result_blob, tool_result_length,
                   tool_is_error, notice_style, attachments, delivery, date, turn_ended_at, subagent, subagent_blob
            FROM entries WHERE thread_id = ? AND seq >= ? ORDER BY seq LIMIT ?
            """, [.text(thread), .int(Int64(from)), .int(Int64(limit))]) { row in
            // A kind from a newer app is skipped, not misread.
            guard let id = row.text(0), let kind = row.text(1).flatMap(EntryKind.init(rawValue:)) else { return }
            var entry = EntrySnapshot(id: id, kind: kind, text: row.text(2) ?? "")
            entry.toolName = row.text(3)
            entry.toolInput = try row.text(5).map { try blobString(connection, $0) ?? "" } ?? row.text(4)
            entry.toolResult = row.text(6)
            if let ref = row.text(7) {
                if let name = entry.toolName, HarnessDatabase.wholeResultTools.contains(name) {
                    entry.toolResult = try blobString(connection, ref) ?? entry.toolResult
                } else {
                    entry.toolResultRef = ref
                    entry.toolResultLength = row.int(8)
                }
            }
            entry.toolIsError = row.int(9).map { $0 != 0 }
            entry.noticeStyle = row.text(10).flatMap(NoticeStyle.init(rawValue:))
            entry.attachments = row.text(11).flatMap { try? JSONDecoder().decode([String].self, from: Data($0.utf8)) }
            entry.delivery = row.text(12).flatMap(MessageDelivery.init(rawValue:))
            entry.date = row.date(13)
            entry.turnEndedAt = row.date(14)
            if let json = try row.data(15) ?? row.text(16).flatMap({ try blob(connection, $0) }) {
                entry.subagent = try? decoder.decode(SubagentSnapshot.self, from: json)
            }
            entries.append(entry)
        }
        return entries
    }

    // MARK: Blobs

    private func storeBlob(_ data: Data) throws -> String {
        let hash = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        let compressed = try? (data as NSData).compressed(using: .lzfse) as Data
        let (codec, stored) = compressed.map { $0.count < data.count ? (1, $0) : (0, data) } ?? (0, data)
        try connection.run("INSERT OR IGNORE INTO blobs(hash, size, codec, data) VALUES (?, ?, ?, ?)",
                           [.text(hash), .int(Int64(data.count)), .int(Int64(codec)), .blob(stored)])
        replacedBlobs.remove(hash)
        return hash
    }

    static func blob(_ connection: SQLiteConnection, _ hash: String) throws -> Data? {
        var result: Data?
        try connection.query("SELECT codec, data FROM blobs WHERE hash = ?", [.text(hash)]) { row in
            guard let data = row.data(1) else { return }
            result = row.int(0) == 1 ? try? (data as NSData).decompressed(using: .lzfse) as Data : data
        }
        return result
    }

    static func blobString(_ connection: SQLiteConnection, _ hash: String) throws -> String? {
        try blob(connection, hash).map { String(decoding: $0, as: UTF8.self) }
    }

    /// Notes the blobs of rows about to be rewritten or deleted.
    private func noteBlobs(_ thread: String, where condition: String, _ value: SQLiteValue) throws {
        try connection.query("""
            SELECT tool_input_blob, tool_result_blob, subagent_blob FROM entries
            WHERE thread_id = ? AND \(condition)
              AND (tool_input_blob IS NOT NULL OR tool_result_blob IS NOT NULL OR subagent_blob IS NOT NULL)
            """, [.text(thread), value]) { row in
            for column: Int32 in 0...2 { if let hash = row.text(column) { replacedBlobs.insert(hash) } }
        }
    }

    private static let unreferenced = """
        NOT EXISTS (SELECT 1 FROM entries WHERE tool_input_blob = blobs.hash)
        AND NOT EXISTS (SELECT 1 FROM entries WHERE tool_result_blob = blobs.hash)
        AND NOT EXISTS (SELECT 1 FROM entries WHERE subagent_blob = blobs.hash)
        """

    /// Deletes the blobs the last write stopped using, unless another row still does.
    func collectReplacedBlobs() {
        guard !replacedBlobs.isEmpty else { return }
        for hash in replacedBlobs {
            try? connection.run("DELETE FROM blobs WHERE hash = ? AND \(Self.unreferenced)", [.text(hash)])
        }
        replacedBlobs.removeAll()
    }

    /// Every blob no row points at: left by a crash between a write and its cleanup.
    func collectGarbage() throws {
        try connection.run("DELETE FROM blobs WHERE \(Self.unreferenced)")
    }

    // MARK: Index

    static func changes(_ connection: SQLiteConnection, after rev: Int, limit: Int) throws -> [HarnessDatabase.EntryChange] {
        var changes: [HarnessDatabase.EntryChange] = []
        try connection.query("""
            SELECT rowid, thread_id, seq, id, kind, text, tool_name, substr(tool_input, 1, 4096), tool_input_blob,
                   substr(tool_result, 1, 4096), rev
            FROM entries WHERE rev > ? ORDER BY rev LIMIT ?
            """, [.int(Int64(rev)), .int(Int64(limit))]) { row in
            guard let kind = row.text(4).flatMap(EntryKind.init(rawValue:)) else { return }
            var input = row.text(7)
            if input == nil, let hash = row.text(8) {
                input = try blobString(connection, hash).map { String($0.prefix(4096)) }
            }
            changes.append(.init(rowID: row.int(0) ?? 0, threadID: row.text(1) ?? "", seq: row.int(2) ?? 0,
                                 entryID: row.text(3) ?? "", kind: kind, text: row.text(5) ?? "", toolName: row.text(6),
                                 toolInput: input, toolResult: row.text(9), rev: row.int(10) ?? 0))
        }
        return changes
    }

    func setBookmark(_ indexer: String, rev: Int) throws {
        try connection.run("""
            INSERT INTO index_state(indexer, rev, updated_at) VALUES (?, ?, ?)
            ON CONFLICT(indexer) DO UPDATE SET rev = excluded.rev, updated_at = excluded.updated_at
            """, [.text(indexer), .int(Int64(rev)), .of(Date.now)])
    }

    /// Brings the full-text index up to date with one batch of the change feed. True if
    /// there's more to do.
    func indexBatch(limit: Int) throws -> Bool {
        let from = try connection.scalar("SELECT rev FROM index_state WHERE indexer = 'fts'")?.int ?? 0
        let changes = try Self.changes(connection, after: from, limit: limit)
        guard let last = changes.last else { return false }
        try connection.transaction {
            for change in changes {
                try connection.run("DELETE FROM entries_fts WHERE rowid = ?", [.int(Int64(change.rowID))])
                guard let body = Self.searchableText(change), !body.isEmpty else { continue }
                try connection.run("INSERT INTO entries_fts(rowid, body) VALUES (?, ?)", [.int(Int64(change.rowID)), .text(body)])
            }
            try setBookmark("fts", rev: last.rev)
        }
        return changes.count == limit
    }

    /// What a search looks through: what was said, and for a tool call what it was given
    /// (the command, the file) and the start of what came back.
    private static func searchableText(_ change: HarnessDatabase.EntryChange) -> String? {
        switch change.kind {
        case .user, .assistant, .notice, .commandOutput:
            change.text
        case .tool:
            [change.toolName, change.toolInput, change.toolResult.map { String($0.prefix(2048)) }]
                .compactMap { $0 }.joined(separator: "\n")
        case .thinking:
            nil
        }
    }

    // MARK: Old files

    private func isImported(_ name: String) throws -> Bool {
        try connection.scalar("SELECT 1 FROM legacy_imports WHERE name = ?", [.text(name)]) != nil
    }

    private func markImported(_ name: String) throws {
        try connection.run("INSERT OR IGNORE INTO legacy_imports(name, imported_at) VALUES (?, ?)", [.text(name), .of(Date.now)])
    }

    /// `state.json`, the first time there is one.
    private func importLegacyState() throws {
        guard let legacy, try !isImported("state") else { return }
        let state: HarnessDatabase.StateRows?
        do {
            state = try legacy.state()
        } catch {
            // Kept for a later version to try; this launch starts without it.
            FileHandle.standardError.write(Data("state.json didn't load: \(error)\n".utf8))
            return
        }
        guard let state else { return }
        try connection.transaction {
            try writeState(state)
            try markImported("state")
        }
    }

    /// Old transcripts of threads that still exist, not yet moved in.
    func unimportedLegacyTranscripts() -> Set<String> {
        guard let legacy else { return [] }
        var threads = Set<String>(), imported = Set<String>()
        try? connection.query("SELECT id FROM threads") { if let id = $0.text(0) { threads.insert(id) } }
        try? connection.query("SELECT name FROM legacy_imports") { if let name = $0.text(0) { imported.insert(name) } }
        return Set(legacy.transcriptIDs()).intersection(threads).subtracting(imported)
    }

    func importLegacyTranscript(_ thread: String, from legacy: LegacyStore) throws {
        guard try !isImported(thread) else { return }
        let entries = legacy.transcript(thread) ?? []
        try connection.transaction {
            // Rows already there were written since: they're newer than the file.
            let existing = try connection.scalar("SELECT count(*) FROM entries WHERE thread_id = ?", [.text(thread)])?.int ?? 0
            if existing == 0 {
                for (seq, entry) in entries.enumerated() { try upsert(thread, seq: seq, entry) }
                // When the file was written, so a session that went on elsewhere since is still noticed.
                try connection.run("INSERT OR REPLACE INTO transcripts(thread_id, saved_at, entry_count) VALUES (?, ?, ?)",
                                   [.text(thread), .of(legacy.transcriptModified(thread) ?? .now), .int(Int64(entries.count))])
            }
            try markImported(thread)
        }
        baselines[thread] = nil
    }

    // Exact dates: a sub-agent's run compares equal after a round trip.
    private static let encoder = JSONEncoder()
    private static let decoder = JSONDecoder()
}
