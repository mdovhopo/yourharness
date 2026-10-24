import Foundation

/// How a workspace's memory is doing: how much there is, how old, how linked, and whether
/// agents actually use it. What the Insights view draws and the reconciler starts from.
public struct MemoryStats: Sendable, Equatable {
    public struct Summary: Sendable, Equatable, Identifiable {
        public var id: String
        public var title: String
        public var kind: String
        public var bytes: Int
        public var recallCount: Int
        public var lastRecalledAt: Date?
        public var createdAt: Date
        public var updatedAt: Date
    }

    public struct Day: Sendable, Equatable, Identifiable {
        public var day: Date
        public var created = 0
        public var updated = 0
        public var recalled = 0
        public var searches = 0
        public var searchHits = 0
        public var sessions = 0
        /// Active memories at the end of the day.
        public var total = 0
        public var id: Date { day }
    }

    public struct Bucket: Sendable, Equatable, Identifiable {
        public var label: String
        public var count: Int
        public var id: String { label }
    }

    public var active = 0
    public var proposed = 0
    public var archived = 0
    /// What the active memories say, in UTF-8 bytes.
    public var bytes = 0
    public var largest: [Summary] = []
    public var byKind: [String: Int] = [:]
    public var byOrigin: [String: Int] = [:]
    public var pinned = 0
    public var links = 0
    /// Active memories with no link either way.
    public var orphans = 0
    /// Reads, ever.
    public var recalls = 0
    public var recallsInWindow = 0
    /// Active memories read at least once in the window.
    public var recalledInWindow = 0
    public var topRecalled: [Summary] = []
    /// Active, at least a week old, and never read.
    public var neverRecalled: [Summary] = []
    public var searches = 0
    public var searchesWithHits = 0
    /// Queries that found nothing: what agents looked for and memory didn't have.
    public var missedQueries: [String] = []
    public var sessions = 0
    /// Bytes of memory each session started with, on average over the window.
    public var averageInjectedBytes = 0
    public var ageBuckets: [Bucket] = []
    public var days: [Day] = []
    /// Titles that look like the same memory twice (shared words), for the reconciler.
    public var likelyDuplicates: [[Summary]] = []
    public var oldest: Date?
    public var newestUpdate: Date?
    /// The window the counts cover, in days.
    public var window = 30

    public var averageBytes: Int { active == 0 ? 0 : bytes / active }
    /// A rough token count: about four bytes a token for English prose.
    public static func tokens(_ bytes: Int) -> Int { (bytes + 3) / 4 }
    public var searchHitRate: Double? { searches == 0 ? nil : Double(searchesWithHits) / Double(searches) }
    /// The share of active memories read in the window.
    public var recallRate: Double? { active == 0 ? nil : Double(recalledInWindow) / Double(active) }
}

extension HarnessDatabase {
    /// Counts for `scope` as of `now`, over the last `window` days.
    public func memoryStats(scope: String, now: Date = .now, window: Int = 30) -> MemoryStats {
        let memories = memories(scope: scope)
        let since = now.addingTimeInterval(-Double(window) * 86_400)
        let events = memoryEvents(scope: scope, since: since, limit: 100_000)
        return MemoryStats.compute(memories: memories, events: events, now: now, window: window)
    }
}

extension MemoryStats {
    static func compute(memories: [HarnessDatabase.Memory], events: [HarnessDatabase.MemoryEvent], now: Date, window: Int) -> MemoryStats {
        var stats = MemoryStats()
        stats.window = window
        let active = memories.filter { $0.status == .active }
        stats.active = active.count
        stats.proposed = memories.count { $0.status == .proposed }
        stats.archived = memories.count { $0.status == .archived }
        stats.bytes = active.reduce(0) { $0 + $1.size }
        stats.pinned = active.count(where: \.pinned)
        func summary(_ memory: HarnessDatabase.Memory) -> Summary {
            Summary(id: memory.id, title: memory.title, kind: memory.kind, bytes: memory.size, recallCount: memory.recallCount,
                    lastRecalledAt: memory.lastRecalledAt, createdAt: memory.createdAt, updatedAt: memory.updatedAt)
        }
        stats.largest = active.sorted { $0.size > $1.size }.prefix(5).map(summary)
        for memory in active {
            stats.byKind[MemoryKind.of(memory.kind).rawValue, default: 0] += 1
            stats.byOrigin[memory.origin ?? "agent", default: 0] += 1
        }

        let activeIDs = Set(active.map(\.id))
        var linked = Set<String>()
        for memory in active {
            for link in memory.links where activeIDs.contains(link.to) {
                stats.links += 1
                linked.insert(memory.id)
                linked.insert(link.to)
            }
        }
        stats.orphans = active.count { !linked.contains($0.id) }

        stats.recalls = memories.reduce(0) { $0 + $1.recallCount }
        stats.topRecalled = active.filter { $0.recallCount > 0 }.sorted {
            $0.recallCount != $1.recallCount ? $0.recallCount > $1.recallCount : ($0.lastRecalledAt ?? .distantPast) > ($1.lastRecalledAt ?? .distantPast)
        }.prefix(8).map(summary)
        let weekAgo = now.addingTimeInterval(-7 * 86_400)
        stats.neverRecalled = active.filter { $0.recallCount == 0 && $0.createdAt < weekAgo && !$0.pinned }
            .sorted { $0.createdAt < $1.createdAt }.map(summary)

        var recalledIDs = Set<String>(), injectedBytes = 0, missed: [String] = []
        for event in events {
            switch event.kind {
            case .recalled:
                stats.recallsInWindow += 1
                if let id = event.memoryID { recalledIDs.insert(id) }
            case .searched:
                stats.searches += 1
                if (event.count ?? 0) > 0 {
                    stats.searchesWithHits += 1
                } else if let query = event.query, !missed.contains(query) {
                    missed.append(query)
                }
            case .injected:
                stats.sessions += 1
                injectedBytes += event.detail.flatMap(Int.init) ?? 0
            default:
                break
            }
        }
        stats.recalledInWindow = recalledIDs.intersection(activeIDs).count
        stats.missedQueries = Array(missed.prefix(12))
        stats.averageInjectedBytes = stats.sessions == 0 ? 0 : injectedBytes / stats.sessions

        let ages: [(String, TimeInterval)] = [("Today", 86_400), ("This week", 7 * 86_400), ("This month", 30 * 86_400),
                                              ("1–3 months", 91 * 86_400), ("Older", .infinity)]
        var counts = Array(repeating: 0, count: ages.count)
        for memory in active {
            let age = now.timeIntervalSince(memory.createdAt)
            counts[ages.firstIndex { age < $0.1 } ?? ages.count - 1] += 1
        }
        stats.ageBuckets = zip(ages, counts).map { Bucket(label: $0.0, count: $1) }
        stats.oldest = active.map(\.createdAt).min()
        stats.newestUpdate = active.map(\.updatedAt).max()

        stats.days = days(memories: memories, events: events, now: now, window: window)
        stats.likelyDuplicates = duplicates(active).map { $0.map(summary) }
        return stats
    }

    private static func days(memories: [HarnessDatabase.Memory], events: [HarnessDatabase.MemoryEvent], now: Date, window: Int) -> [Day] {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = .current
        let today = calendar.startOfDay(for: now)
        var days: [Date: Day] = [:]
        let starts = (0..<window).compactMap { calendar.date(byAdding: .day, value: -$0, to: today) }
        for start in starts { days[start] = Day(day: start) }
        for event in events {
            let start = calendar.startOfDay(for: event.at)
            guard days[start] != nil else { continue }
            switch event.kind {
            case .created, .imported: days[start]!.created += 1
            case .updated, .merged: days[start]!.updated += 1
            case .recalled: days[start]!.recalled += 1
            case .searched:
                days[start]!.searches += 1
                if (event.count ?? 0) > 0 { days[start]!.searchHits += 1 }
            case .injected: days[start]!.sessions += 1
            default: break
            }
        }
        // How many were active at each day's end: everything that exists now and was made by
        // then, less what's archived (an archive's date is its last update).
        for start in starts {
            guard let end = calendar.date(byAdding: .day, value: 1, to: start) else { continue }
            days[start]!.total = memories.count { memory in
                memory.createdAt < end && (memory.status != .archived || memory.updatedAt >= end)
            }
        }
        return starts.reversed().compactMap { days[$0] }
    }

    /// Groups of memories whose titles share most of their words.
    static func duplicates(_ memories: [HarnessDatabase.Memory]) -> [[HarnessDatabase.Memory]] {
        let words = memories.map { memory in
            Set(memory.title.lowercased().split { !$0.isLetter && !$0.isNumber }.map(String.init).filter { $0.count > 2 && !stopWords.contains($0) })
        }
        var groups: [[Int]] = [], grouped = Set<Int>()
        for i in memories.indices where !grouped.contains(i) && !words[i].isEmpty {
            var group = [i]
            for j in memories.indices where j > i && !grouped.contains(j) && !words[j].isEmpty {
                let shared = words[i].intersection(words[j]).count
                let smaller = min(words[i].count, words[j].count)
                if shared >= 2 || smaller == 1 && shared == 1 && words[i] == words[j] { group.append(j) }
                else if Double(shared) / Double(smaller) >= 0.75, shared >= 1, smaller >= 2 { group.append(j) }
            }
            if group.count > 1 {
                groups.append(group)
                grouped.formUnion(group)
            }
        }
        return groups.map { $0.map { memories[$0] } }
    }

    private static let stopWords: Set<String> = ["the", "and", "for", "with", "use", "uses", "how", "what", "when", "not", "never",
                                                 "always", "about", "from", "into", "that", "this", "are", "you", "your"]
}
