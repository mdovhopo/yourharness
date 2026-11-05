import Foundation
import HarnessCore

/// One transcript row as the page gets it: short keys, long tool text cut to a head (the page
/// shows a head anyway, and the full text would only cost memory on both sides).
struct WireEntry: Encodable {
    var id: String
    var k: String
    var t: String
    var n: String? = nil
    var i: String? = nil
    var r: String? = nil
    var rl: Int? = nil
    var e: Int? = nil
    var d: Double? = nil
    var sa: Subagent? = nil

    struct Subagent: Encodable {
        var d: String
        var s: String
        var c: Int?
    }

    static let toolTextLimit = 4000
    /// Streamed tool output: what a folded step shows when opened. Hundreds of results a turn
    /// at full length would be most of the page's memory.
    static let liveResultLimit = 1500

    static func head(_ text: String?, limit: Int = toolTextLimit) -> String? {
        guard let text else { return nil }
        return text.utf16.count > limit ? String(text.prefix(limit)) : text
    }

    init(id: String, k: String, t: String) {
        self.id = id
        self.k = k
        self.t = t
    }

    init(_ snapshot: EntrySnapshot) {
        id = snapshot.id
        k = snapshot.kind == .commandOutput ? "command" : snapshot.kind.rawValue
        t = snapshot.text
        n = snapshot.toolName
        i = Self.head(snapshot.toolInput)
        r = Self.head(snapshot.toolResult)
        rl = snapshot.toolResultLength ?? snapshot.toolResult.map(\.count)
        e = snapshot.toolIsError == true ? 1 : nil
        d = snapshot.date.map { $0.timeIntervalSince1970 * 1000 }
        sa = snapshot.subagent.map { Subagent(d: $0.description, s: $0.status.rawValue, c: $0.toolUses) }
    }
}

struct ProjectMeta: Encodable {
    var id: String
    var name: String
    var path: String
    var source: String
}

struct ThreadMeta: Encodable {
    var id: String
    var projectId: String
    var title: String
    var updatedAt: Double
    var sessionId: String?
    var source: String
}

/// What there is to show: the harness's own database (a copy: the extreme preset for load
/// tests) and Claude Code's history in `~/.claude/projects`, read-only.
final class Library: @unchecked Sendable {
    private let db: HarnessDatabase?
    private let claudeConfig: URL?
    private let lock = NSLock()
    private(set) var projects: [ProjectMeta] = []
    private(set) var threads: [ThreadMeta] = []
    private var sessionFiles: [String: URL] = [:]
    private var projectPaths: [String: String] = [:]
    /// Claude Code sessions read whole (they're one file), the last few kept.
    private var archives: [(String, [WireEntry])] = []

    init() {
        if let dir = env["YHW_DB_DIR"] {
            db = HarnessDatabase(directory: URL(fileURLWithPath: dir, isDirectory: true), importLegacy: false)
        } else {
            db = nil
        }
        if env["YHW_NO_CLAUDE"] == "1" {
            claudeConfig = nil
        } else if let dir = env["YHW_CLAUDE_CONFIG"] {
            claudeConfig = URL(fileURLWithPath: (dir as NSString).expandingTildeInPath, isDirectory: true)
        } else {
            claudeConfig = ClaudeSessionArchive.configDirectory(environment: env)
        }
    }

    func projectPath(_ id: String) -> String? {
        lock.withLock { projectPaths[id] }
    }

    func scan() {
        var projects: [ProjectMeta] = [], threads: [ThreadMeta] = [], files: [String: URL] = [:]
        let started = Date()
        if let db {
            let state = db.loadState()
            for row in state.projects {
                guard let json = try? JSONSerialization.jsonObject(with: row.json) as? [String: Any] else { continue }
                projects.append(ProjectMeta(id: row.id, name: json["name"] as? String ?? "Project",
                                            path: json["path"] as? String ?? "", source: "db"))
            }
            for row in state.threads {
                guard let json = try? JSONSerialization.jsonObject(with: row.json) as? [String: Any] else { continue }
                threads.append(ThreadMeta(id: row.id, projectId: row.projectID, title: json["title"] as? String ?? "Thread",
                                          updatedAt: (row.updatedAt ?? .distantPast).timeIntervalSince1970 * 1000,
                                          sessionId: row.sessionID, source: "db"))
            }
        }
        // `YHW_PROJECT=<folder>`: a project of its own, for trying live turns somewhere throwaway.
        if let path = env["YHW_PROJECT"] {
            projects.insert(ProjectMeta(id: "dir:" + path, name: (path as NSString).lastPathComponent, path: path, source: "dir"), at: 0)
        }
        if let claudeConfig {
            let limit = Int(env["YHW_CLAUDE_LIMIT"] ?? "") ?? 400
            var byPath: [String: Int] = [:]
            for file in ClaudeSessionArchive.sessionFiles(configDirectory: claudeConfig).prefix(limit) {
                let peek = Self.peek(file.url)
                guard let title = peek.title else { continue }
                let path = peek.cwd ?? file.folderName
                let projectID = "cc:" + path
                if byPath[path] == nil {
                    byPath[path] = projects.count
                    projects.append(ProjectMeta(id: projectID, name: (path as NSString).lastPathComponent, path: path, source: "claude"))
                }
                let id = "cc:" + file.sessionID
                files[id] = file.url
                threads.append(ThreadMeta(id: id, projectId: projectID, title: title,
                                          updatedAt: file.modified.timeIntervalSince1970 * 1000, sessionId: file.sessionID, source: "claude"))
            }
        }
        lock.withLock {
            self.projects = projects
            self.threads = threads
            sessionFiles = files
            projectPaths = Dictionary(projects.map { ($0.id, $0.path) }) { a, _ in a }
        }
        log(String(format: "library: %d projects, %d threads in %.2f s", projects.count, threads.count, Date().timeIntervalSince(started)))
    }

    func listingJSON() -> Data {
        struct Listing: Encodable { var projects: [ProjectMeta]; var threads: [ThreadMeta] }
        let listing = lock.withLock { Listing(projects: projects, threads: threads) }
        return (try? JSONEncoder().encode(listing)) ?? Data("{}".utf8)
    }

    /// Rows `from..<before` of a thread, the newest `limit` before `before` (or the end).
    func pageJSON(thread: String, before: Int?, limit: Int) -> Data {
        struct Page: Encodable { var total: Int; var from: Int; var entries: [WireEntry] }
        var page = Page(total: 0, from: 0, entries: [])
        if thread.hasPrefix("cc:") {
            let entries = archive(thread)
            let end = min(before ?? entries.count, entries.count)
            let from = max(0, end - limit)
            page = Page(total: entries.count, from: from, entries: Array(entries[from..<end]))
        } else if let db, let id = UUID(uuidString: thread) {
            let t0 = Date()
            let total = db.transcriptCount(id)
            let t1 = Date()
            let end = min(before ?? total, total)
            let from = max(0, end - limit)
            let rows = db.loadTranscriptPage(id, from: from, limit: end - from)
            let t2 = Date()
            page = Page(total: total, from: from, entries: rows.map(WireEntry.init))
            if env["YHW_TIMING"] == "1" {
                log(String(format: "page %@: count %.0f ms, rows %.0f ms, wire %.0f ms", thread, t1.timeIntervalSince(t0) * 1000, t2.timeIntervalSince(t1) * 1000, Date().timeIntervalSince(t2) * 1000))
            }
        }
        return (try? JSONEncoder().encode(page)) ?? Data("{}".utf8)
    }

    private func archive(_ thread: String) -> [WireEntry] {
        if let cached = lock.withLock({ archives.first { $0.0 == thread }?.1 }) { return cached }
        guard let url = lock.withLock({ sessionFiles[thread] }),
              let archive = try? ClaudeSessionArchive.load(from: url, sessionID: String(thread.dropFirst(3))) else { return [] }
        var entries: [WireEntry] = []
        entries.reserveCapacity(archive.items.count)
        for (index, item) in archive.items.enumerated() {
            let id = "h\(index)"
            switch item {
            case .user(let text): entries.append(WireEntry(id: id, k: "user", t: text))
            case .assistant(let text): entries.append(WireEntry(id: id, k: "assistant", t: text))
            case .thinking(let text): entries.append(WireEntry(id: id, k: "thinking", t: text))
            case .notice(let text): entries.append(WireEntry(id: id, k: "notice", t: text))
            case .tool(let toolID, let name, let input, let result, let isError):
                var entry = WireEntry(id: toolID.isEmpty ? id : toolID, k: "tool", t: "")
                entry.n = name
                entry.i = WireEntry.head(input)
                entry.r = WireEntry.head(result)
                entry.rl = result?.count
                entry.e = isError ? 1 : nil
                entries.append(entry)
            }
        }
        lock.withLock {
            archives.removeAll { $0.0 == thread }
            archives.append((thread, entries))
            if archives.count > 6 { archives.removeFirst() }
        }
        return entries
    }

    /// The folder and first real message of a session, from the start of its file only.
    static func peek(_ url: URL) -> (cwd: String?, title: String?) {
        guard let handle = try? FileHandle(forReadingFrom: url) else { return (nil, nil) }
        defer { try? handle.close() }
        let data = (try? handle.read(upToCount: 256 * 1024)) ?? Data()
        var cwd: String?, title: String?
        for line in data.split(separator: UInt8(ascii: "\n")) {
            guard let object = try? JSONSerialization.jsonObject(with: line) as? [String: Any] else { continue }
            if cwd == nil, let value = object["cwd"] as? String { cwd = value }
            if title == nil, object["type"] as? String == "user", object["isMeta"] as? Bool != true,
               let message = object["message"] as? [String: Any] {
                var text: String?
                if let string = message["content"] as? String { text = string }
                else if let parts = message["content"] as? [[String: Any]] {
                    text = parts.first { $0["type"] as? String == "text" }?["text"] as? String
                }
                if let text, !text.hasPrefix("<"), !text.hasPrefix("Caveat:"), !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    title = String(text.split(separator: "\n").first ?? "").prefix(80).description
                }
            }
            if cwd != nil, title != nil { break }
        }
        return (cwd, title)
    }
}
