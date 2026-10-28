import Foundation
import Synchronization

/// Token spend by day and model, for the usage view.
public struct UsageReport: Sendable, Equatable {
    public struct Spend: Sendable, Equatable {
        public var tokens = TokenCounts()
        /// At API prices; tokens of models with no price aren't in it.
        public var cost = 0.0
        /// Tokens of models the pricing table doesn't know.
        public var unpricedTokens = 0
        public var messages = 0

        public init() {}

        public static func += (lhs: inout Spend, rhs: Spend) {
            lhs.tokens += rhs.tokens
            lhs.cost += rhs.cost
            lhs.unpricedTokens += rhs.unpricedTokens
            lhs.messages += rhs.messages
        }
    }

    public struct Day: Sendable, Equatable, Identifiable {
        /// Its start, in the calendar the report was made with.
        public var date: Date
        /// Keyed by model family (`ModelPricing.family(of:)`).
        public var models: [String: Spend] = [:]
        public var total = Spend()
        public var id: Date { date }

        /// What most of the day went to: by cost, then by tokens.
        public var topModel: String? {
            models.max { ($0.value.cost, $0.value.tokens.total) < ($1.value.cost, $1.value.tokens.total) }?.key
        }
    }

    public struct ModelTotal: Sendable, Equatable, Identifiable {
        public var model: String
        public var spend: Spend
        public var id: String { model }
    }

    public var start: Date
    /// The last day's start.
    public var end: Date
    /// Every day in the range, the oldest first, with or without usage.
    public var days: [Day]
    /// The models used in the range, the costliest first.
    public var models: [ModelTotal]
    public var total: Spend
    /// Every model ever seen, the newest first (by the day it was first used, then by cost):
    /// colors follow a model whatever the range, and the models in use now get the first ones.
    public var allModels: [String]

    /// The day `date` falls on, if it's in the range: worked out from the date, not looked
    /// for, so a chart can ask for every bar.
    public func day(containing date: Date, calendar: Calendar = .current) -> Day? {
        let start = calendar.startOfDay(for: date)
        guard let offset = calendar.dateComponents([.day], from: self.start, to: start).day,
              days.indices.contains(offset), days[offset].date == start
        else { return nil }
        return days[offset]
    }

    public static let empty = UsageReport(start: .distantPast, end: .distantPast, days: [], models: [], total: Spend(), allModels: [])
}

/// What Claude Code's session files say was spent, indexed in a database of its own
/// (`usage.sqlite` in the data folder) so a year of history is summed in a moment, plus
/// what the app's own sessions that leave no file behind (side chats, memory agents) spent.
///
/// Files are read once and then only from where they were left: Claude Code appends to them.
/// Every response is kept by its message and request IDs, so one copied into a forked or
/// resumed session's file counts once, and history stays after Claude Code deletes old
/// sessions (after 30 days by default).
public final class UsageStore: Sendable {
    public let url: URL
    private let writer: Mutex<SQLiteConnection?>
    private let reader: Mutex<SQLiteConnection?>
    /// `UsageReport.allModels`, until something new is written.
    private let order = Mutex<[String]?>(nil)

    /// Where a record came from.
    enum Source: Int64 {
        case sessionFile = 0
        case app = 1
    }

    public init(directory: URL) {
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        url = directory.appending(path: "usage.sqlite")
        var connections = Self.open(url)
        if connections == nil {
            // An index of other files: if it can't be read, it's built again.
            for suffix in ["", "-wal", "-shm"] { try? FileManager.default.removeItem(at: URL(fileURLWithPath: url.path + suffix)) }
            connections = Self.open(url)
        }
        writer = Mutex(connections?.writer)
        reader = Mutex(connections?.reader)
    }

    static let schemaVersion = 1

    private static func bucket(_ row: String) -> String { "CAST(\(row).at / 900 AS INTEGER)" }

    private static func long(_ row: String) -> String {
        "(\(row).input + \(row).cache_read + \(row).cache_5m + \(row).cache_1h > \(ModelPricing.longPromptThreshold))"
    }

    private static func addToBucket(_ row: String) -> String {
        """
        INSERT INTO buckets VALUES (\(bucket(row)), \(row).model, \(row).fast, \(long(row)), \(row).input, \(row).output,
            \(row).cache_read, \(row).cache_5m, \(row).cache_1h, 1)
        ON CONFLICT DO UPDATE SET input = input + excluded.input, output = output + excluded.output,
            cache_read = cache_read + excluded.cache_read, cache_5m = cache_5m + excluded.cache_5m,
            cache_1h = cache_1h + excluded.cache_1h, messages = messages + 1;
        """
    }

    private static func open(_ url: URL) -> (writer: SQLiteConnection, reader: SQLiteConnection)? {
        do {
            let writer = try SQLiteConnection(url: url)
            try writer.execute("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
            let version = try writer.scalar("PRAGMA user_version")?.int ?? 0
            if version < schemaVersion {
                try writer.execute("""
                CREATE TABLE IF NOT EXISTS files (
                    path TEXT PRIMARY KEY,
                    size INTEGER NOT NULL,
                    modified REAL NOT NULL,
                    offset INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS messages (
                    key INTEGER PRIMARY KEY,
                    at REAL NOT NULL,
                    model TEXT NOT NULL,
                    input INTEGER NOT NULL,
                    output INTEGER NOT NULL,
                    cache_read INTEGER NOT NULL,
                    cache_5m INTEGER NOT NULL,
                    cache_1h INTEGER NOT NULL,
                    fast INTEGER NOT NULL DEFAULT 0,
                    source INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX IF NOT EXISTS messages_at ON messages(at);
                -- What reports read: messages summed by fifteen minutes and by how they're
                -- priced, kept up to date as they're written, so a year is a few thousand rows.
                CREATE TABLE IF NOT EXISTS buckets (
                    bucket INTEGER NOT NULL,
                    model TEXT NOT NULL,
                    fast INTEGER NOT NULL,
                    long INTEGER NOT NULL,
                    input INTEGER NOT NULL,
                    output INTEGER NOT NULL,
                    cache_read INTEGER NOT NULL,
                    cache_5m INTEGER NOT NULL,
                    cache_1h INTEGER NOT NULL,
                    messages INTEGER NOT NULL,
                    PRIMARY KEY (bucket, model, fast, long)
                ) WITHOUT ROWID;
                CREATE TRIGGER IF NOT EXISTS messages_added AFTER INSERT ON messages BEGIN
                    \(addToBucket("NEW"))
                END;
                CREATE TRIGGER IF NOT EXISTS messages_changed AFTER UPDATE ON messages BEGIN
                    UPDATE buckets SET input = input - OLD.input, output = output - OLD.output,
                        cache_read = cache_read - OLD.cache_read, cache_5m = cache_5m - OLD.cache_5m,
                        cache_1h = cache_1h - OLD.cache_1h, messages = messages - 1
                    WHERE bucket = \(bucket("OLD")) AND model = OLD.model AND fast = OLD.fast AND long = \(long("OLD"));
                    \(addToBucket("NEW"))
                END;
                PRAGMA user_version = \(schemaVersion);
                """)
            }
            let reader = try SQLiteConnection(url: url)
            try reader.execute("PRAGMA query_only = 1;")
            return (writer, reader)
        } catch {
            FileHandle.standardError.write(Data("Usage index didn't open (\(error))\n".utf8))
            return nil
        }
    }

    /// A copy as of now, for Export State: consistent, and with nothing left in a WAL.
    public func snapshot(to file: URL) throws {
        try writer.withLock { connection in
            guard let connection else { return }
            try connection.run("VACUUM INTO ?", [.text(file.path)])
        }
    }

    // MARK: - Recording

    /// What the app's own sessions spent that no session file records.
    public func record(_ usages: [MessageUsage]) {
        guard !usages.isEmpty else { return }
        writer.withLock { connection in
            guard let connection else { return }
            _ = try? connection.transaction {
                for usage in usages { try Self.upsert(usage, source: .app, connection) }
            }
        }
        order.withLock { $0 = nil }
    }

    /// A response is written once per content block, the later ones with more output.
    private static func upsert(_ usage: MessageUsage, source: Source, _ connection: SQLiteConnection) throws {
        let tokens = usage.tokens
        try connection.run("""
        INSERT INTO messages (key, at, model, input, output, cache_read, cache_5m, cache_1h, fast, source)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(key) DO UPDATE SET
            input = max(input, excluded.input), output = max(output, excluded.output),
            cache_read = max(cache_read, excluded.cache_read),
            cache_5m = max(cache_5m, excluded.cache_5m), cache_1h = max(cache_1h, excluded.cache_1h)
        """, [.int(usage.key), .double(usage.date.timeIntervalSince1970), .text(usage.model),
              .of(tokens.input), .of(tokens.output), .of(tokens.cacheRead), .of(tokens.cacheWrite5m),
              .of(tokens.cacheWrite1h), .of(usage.fast), .int(source.rawValue)])
    }

    // MARK: - Reading session files

    public struct RefreshResult: Sendable, Equatable {
        public var filesRead = 0
        public var filesTotal = 0
        public var records = 0
    }

    /// Reads what's new in Claude Code's session files under `configDirectory/projects`,
    /// the most recently changed first. `progress` gets (files read, files to read) now and then.
    @discardableResult
    public func refresh(configDirectory: URL, progress: (@Sendable (Int, Int) -> Void)? = nil) -> RefreshResult {
        let projects = configDirectory.appendingPathComponent("projects", isDirectory: true)
        let files = Self.sessionFiles(in: projects)
        return writer.withLock { connection -> RefreshResult in
            guard let connection else { return RefreshResult() }
            var known: [String: (size: Int, modified: Double, offset: Int)] = [:]
            try? connection.query("SELECT path, size, modified, offset FROM files") { row in
                if let path = row.text(0) { known[path] = (row.int(1) ?? 0, row.double(2) ?? 0, row.int(3) ?? 0) }
            }
            let changed = files.filter { file in
                guard let seen = known[file.path] else { return true }
                return seen.size != file.size || seen.modified != file.modified
            }
            var result = RefreshResult(filesTotal: changed.count)
            guard !changed.isEmpty else { return result }
            // Room for the index while it's written, given back after.
            try? connection.execute("PRAGMA cache_size = -65536")
            defer { try? connection.execute("PRAGMA cache_size = -2000; PRAGMA shrink_memory;") }
            progress?(0, changed.count)
            var lastReport = Date.now
            // Read a batch of files at once, a core each, then write it in one transaction.
            var inTransaction = false
            var lastCommit = Date.now
            let batchSize = max(ProcessInfo.processInfo.activeProcessorCount * 4, 16)
            for batchStart in stride(from: 0, to: changed.count, by: batchSize) {
                let batch = Array(changed[batchStart..<min(batchStart + batchSize, changed.count)])
                let starts = batch.map { file in
                    // Shorter than where it was left: not the same file any more.
                    known[file.path].map { $0.offset <= file.size ? $0.offset : 0 } ?? 0
                }
                let reads = Mutex([(records: [MessageUsage], consumed: Int)](repeating: ([], 0), count: batch.count))
                DispatchQueue.concurrentPerform(iterations: batch.count) { index in
                    let read = Self.readFile(batch[index].url, from: starts[index], size: batch[index].size)
                    reads.withLock { $0[index] = read }
                }
                let results = reads.withLock { $0 }
                // A response's lines and its copies in forks, merged before they're written.
                var merged: [Int64: MessageUsage] = [:]
                for read in results {
                    for record in read.records {
                        merged[record.key] = merged[record.key].map { $0.merging(record) } ?? record
                    }
                }
                // One transaction for a second's worth of batches: each commit costs a
                // checkpoint, and committing every batch took most of a first read's time.
                if !inTransaction { inTransaction = (try? connection.execute("BEGIN IMMEDIATE")) != nil }
                do {
                    // In key order, so each write lands near the last.
                    for record in merged.values.sorted(by: { $0.key < $1.key }) { try Self.upsert(record, source: .sessionFile, connection) }
                    for (index, file) in batch.enumerated() {
                        try connection.run("INSERT OR REPLACE INTO files (path, size, modified, offset) VALUES (?, ?, ?, ?)",
                                           [.text(file.path), .of(file.size), .double(file.modified), .of(starts[index] + results[index].consumed)])
                    }
                } catch {
                    // Nothing of this stretch is kept; its files are read again next time.
                    try? connection.execute("ROLLBACK")
                    inTransaction = false
                    lastCommit = .now
                    continue
                }
                if inTransaction, Date.now.timeIntervalSince(lastCommit) > 1 {
                    try? connection.execute("COMMIT")
                    inTransaction = false
                    lastCommit = .now
                }
                result.filesRead += batch.count
                result.records += merged.count
                if let progress, Date.now.timeIntervalSince(lastReport) > 0.25 {
                    lastReport = .now
                    progress(result.filesRead, result.filesTotal)
                }
            }
            if inTransaction { try? connection.execute("COMMIT") }
            progress?(result.filesRead, result.filesTotal)
            if result.records > 0 { order.withLock { $0 = nil } }
            return result
        }
    }

    struct SessionFile {
        var url: URL
        var path: String
        var size: Int
        var modified: Double
    }

    /// Every `.jsonl` under `projects`, sub-agents' included, the newest first.
    static func sessionFiles(in projects: URL) -> [SessionFile] {
        let keys: [URLResourceKey] = [.isRegularFileKey, .fileSizeKey, .contentModificationDateKey]
        guard let enumerator = FileManager.default.enumerator(at: projects, includingPropertiesForKeys: keys,
                                                              options: [.skipsHiddenFiles, .skipsPackageDescendants])
        else { return [] }
        var files: [SessionFile] = []
        for case let url as URL in enumerator where url.pathExtension == "jsonl" {
            guard let values = try? url.resourceValues(forKeys: Set(keys)), values.isRegularFile == true else { continue }
            files.append(SessionFile(url: url, path: url.path, size: values.fileSize ?? 0,
                                     modified: values.contentModificationDate?.timeIntervalSince1970 ?? 0))
        }
        return files.sorted { $0.modified > $1.modified }
    }

    /// From `offset` to the last complete line: in one read, or a few megabytes at a time
    /// for a very long file.
    static func readFile(_ url: URL, from offset: Int, size: Int) -> (records: [MessageUsage], consumed: Int) {
        guard size > offset, let handle = try? FileHandle(forReadingFrom: url) else { return ([], 0) }
        defer { try? handle.close() }
        do { try handle.seek(toOffset: UInt64(offset)) } catch { return ([], 0) }
        var records: [MessageUsage] = []
        var consumed = 0
        var carry = Data()
        var remaining = size - offset
        while true {
            // Sized to what's there: a buffer of a fixed size is zeroed for every file.
            let chunk = autoreleasepool { (try? handle.read(upToCount: min(max(remaining, 1 << 16), 32 << 20))) ?? Data() }
            if chunk.isEmpty { break }
            remaining -= chunk.count
            let buffer = carry.isEmpty ? chunk : carry + chunk
            let read = SessionUsageReader.read(buffer, source: "\(url.path):\(offset + consumed)")
            records += read.records
            consumed += read.consumed
            carry = read.consumed < buffer.count ? buffer.subdata(in: (buffer.startIndex + read.consumed)..<buffer.endIndex) : Data()
        }
        return (records, consumed)
    }

    // MARK: - Reports

    /// The last `days` days, today included.
    public func report(days: Int, now: Date = .now, calendar: Calendar = .current) -> UsageReport {
        let today = calendar.startOfDay(for: now)
        guard let start = calendar.date(byAdding: .day, value: -(max(days, 1) - 1), to: today),
              let end = calendar.date(byAdding: .day, value: 1, to: today)
        else { return .empty }
        var dayIndex: [Date: Int] = [:]
        var dayList: [UsageReport.Day] = []
        var cursor = start
        while cursor < end {
            dayIndex[cursor] = dayList.count
            dayList.append(UsageReport.Day(date: cursor))
            guard let next = calendar.date(byAdding: .day, value: 1, to: cursor) else { break }
            cursor = next
        }
        var models: [String: UsageReport.Spend] = [:]
        var total = UsageReport.Spend()
        let (groups, allModels) = reader.withLock { connection -> ([Group: Sum], [String]) in
            guard let connection else { return ([:], []) }
            let groups = Self.sum(connection, from: start.timeIntervalSince1970, to: end.timeIntervalSince1970, bucket: 900)
            if let order = order.withLock({ $0 }) { return (groups, order) }
            let order = Self.modelOrder(Self.sum(connection, from: nil, to: nil, bucket: nil))
            self.order.withLock { $0 = order }
            return (groups, order)
        }
        // Fifteen-minute buckets, put into days here: SQLite knows nothing of the calendar,
        // and every time zone's offset is a multiple of fifteen minutes.
        var bucketDays: [Int: Int?] = [:]
        for (group, sum) in groups {
            let index = bucketDays[group.bucket] ?? {
                let index = dayIndex[calendar.startOfDay(for: Date(timeIntervalSince1970: Double(group.bucket) * 900))]
                bucketDays[group.bucket] = index
                return index
            }()
            guard let index else { continue }
            let spend = Self.spend(sum, group)
            let family = ModelPricing.family(of: group.model)
            dayList[index].models[family, default: .init()] += spend
            dayList[index].total += spend
            models[family, default: .init()] += spend
            total += spend
        }
        let ranked = models.map { UsageReport.ModelTotal(model: $0.key, spend: $0.value) }
            .sorted { ($0.spend.cost, $0.spend.tokens.total, $1.model) > ($1.spend.cost, $1.spend.tokens.total, $0.model) }
        return UsageReport(start: start, end: today, days: dayList, models: ranked, total: total, allModels: allModels)
    }

    /// Responses priced alike: the same model, fast or not, and prompt over the long-prompt
    /// threshold or not, in the same stretch of time.
    private struct Group: Hashable {
        var bucket: Int
        var model: String
        var fast: Bool
        var long: Bool
    }

    private struct Sum {
        var tokens = TokenCounts()
        var messages = 0
        var firstAt = Double.infinity
    }

    /// The buckets from `from` to `to` (all of them when nil), merged into buckets `bucket`
    /// seconds long (one for all, when nil).
    private static func sum(_ connection: SQLiteConnection, from: Double?, to: Double?, bucket: Double?) -> [Group: Sum] {
        var groups: [Group: Sum] = [:]
        let low = from.map { Int64(($0 / 900).rounded(.down)) } ?? .min
        let high = to.map { Int64(($0 / 900).rounded(.up)) } ?? .max
        try? connection.query("""
        SELECT bucket, model, fast, long, input, output, cache_read, cache_5m, cache_1h, messages
        FROM buckets WHERE bucket >= ? AND bucket < ? AND messages > 0
        """, [.int(low), .int(high)]) { row in
            guard let stored = row.int(0), let model = row.text(1) else { return }
            let at = Double(stored) * 900
            if let from, at < from { return }
            if let to, at >= to { return }
            let group = Group(bucket: bucket.map { Int((at / $0).rounded(.down)) } ?? 0, model: model,
                              fast: row.int(2) == 1, long: row.int(3) == 1)
            var sum = groups[group] ?? Sum()
            sum.tokens += TokenCounts(input: row.int(4) ?? 0, output: row.int(5) ?? 0, cacheRead: row.int(6) ?? 0,
                                      cacheWrite5m: row.int(7) ?? 0, cacheWrite1h: row.int(8) ?? 0)
            sum.messages += row.int(9) ?? 0
            sum.firstAt = min(sum.firstAt, at)
            groups[group] = sum
        }
        return groups
    }

    /// Model families, the newest first (the day each was first used, then all-time cost).
    private static func modelOrder(_ groups: [Group: Sum]) -> [String] {
        var seen: [String: (firstDay: Int, cost: Double)] = [:]
        for (group, sum) in groups {
            let family = ModelPricing.family(of: group.model), spend = Self.spend(sum, group)
            var entry = seen[family] ?? (Int(sum.firstAt / 86_400), 0)
            entry.firstDay = min(entry.firstDay, Int(sum.firstAt / 86_400))
            // A model with no price goes by its tokens, after the priced ones.
            entry.cost += spend.cost + Double(spend.unpricedTokens) / 1e12
            seen[family] = entry
        }
        return seen.sorted { ($0.value.firstDay, $0.value.cost, $1.key) > ($1.value.firstDay, $1.value.cost, $0.key) }.map(\.key)
    }

    private static func spend(_ sum: Sum, _ group: Group) -> UsageReport.Spend {
        var spend = UsageReport.Spend()
        spend.tokens = sum.tokens
        spend.messages = sum.messages
        if let entry = ModelPricing.entry(for: group.model) {
            var price = (group.long ? entry.longPrompt : nil) ?? entry.price
            if group.fast, let factor = entry.fast { price = price.scaled(factor) }
            spend.cost = price.cost(spend.tokens)
        } else {
            spend.unpricedTokens = spend.tokens.total
        }
        return spend
    }
}
