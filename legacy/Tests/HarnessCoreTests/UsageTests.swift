import Foundation
import Testing
@testable import HarnessCore

/// Token spend from Claude Code's session files, priced at API rates.
struct UsageTests {
    // MARK: - Pricing

    @Test func modelIDsFoldIntoFamilies() {
        #expect(ModelPricing.family(of: "claude-opus-4-5-20251101") == "claude-opus-4-5")
        #expect(ModelPricing.family(of: "us.anthropic.claude-opus-4-5-20251101-v1:0") == "claude-opus-4-5")
        #expect(ModelPricing.family(of: "claude-opus-4-5@20251101") == "claude-opus-4-5")
        #expect(ModelPricing.family(of: "claude-opus-5-5[1m]") == "claude-opus-5-5")
        #expect(ModelPricing.family(of: "claude-sonnet-4-20250514") == "claude-sonnet-4")
        #expect(ModelPricing.family(of: "claude-3-5-haiku-20241022") == "claude-3-5-haiku")
        #expect(ModelPricing.family(of: "gpt-9") == "gpt-9")
    }

    @Test func namesModelsForPeople() {
        #expect(ModelPricing.displayName("claude-opus-5-5") == "Opus 5.5")
        #expect(ModelPricing.displayName("claude-sonnet-4-5-20250929") == "Sonnet 4.5")
        #expect(ModelPricing.displayName("claude-opus-4-20250514") == "Opus 4")
        #expect(ModelPricing.displayName("claude-3-5-haiku-20241022") == "Haiku 3.5")
        #expect(ModelPricing.displayName("claude-fable-5-1") == "Fable 5.1")
        #expect(ModelPricing.displayName("some-other-model") == "some-other-model")
    }

    @Test func pricesEveryKindOfToken() throws {
        let tokens = TokenCounts(input: 1_000_000, output: 1_000_000, cacheRead: 1_000_000, cacheWrite5m: 1_000_000, cacheWrite1h: 1_000_000)
        // Opus 5.5: $4 in, $20 out, $5 / $8 cache writes, $0.20 cache reads.
        #expect(try #require(ModelPricing.cost(tokens, model: "claude-opus-5-5")).isClose(to: 37.2))
        // Sonnet 4.6: $3, $15, $3.75, $6, $0.30.
        #expect(try #require(ModelPricing.cost(tokens, model: "claude-sonnet-4-6")).isClose(to: 28.05))
        #expect(ModelPricing.cost(tokens, model: "claude-unknown-9") == nil)
        #expect(ModelPricing.cost(tokens, model: "<synthetic>") == nil)
    }

    @Test func haikuCostsMoreForLongPrompts() throws {
        let short = TokenCounts(input: 100_000, output: 1_000)
        let long = TokenCounts(input: 60_000, output: 1_000, cacheRead: 40_001)
        #expect(try #require(ModelPricing.cost(short, model: "claude-haiku-5-5")).isClose(to: 0.0105))
        // Cache reads count toward the prompt's length.
        #expect(try #require(ModelPricing.cost(long, model: "claude-haiku-5-5")).isClose(to: 0.03 + 0.0025 + 0.00200005))
    }

    @Test func fastModeDoublesOnlyWhereItExists() throws {
        let tokens = TokenCounts(input: 1_000_000, output: 1_000_000)
        #expect(try #require(ModelPricing.cost(tokens, model: "claude-opus-5-5", fast: true)).isClose(to: 48))
        #expect(try #require(ModelPricing.cost(tokens, model: "claude-opus-4-8", fast: true)).isClose(to: 60))
        #expect(try #require(ModelPricing.cost(tokens, model: "claude-sonnet-4-5", fast: true)).isClose(to: 18))
    }

    // MARK: - Reading session files

    private func fixture() throws -> Data {
        let url = try #require(Bundle.module.url(forResource: "session-usage", withExtension: "jsonl", subdirectory: "Fixtures"))
        return try Data(contentsOf: url)
    }

    @Test func readsUsageFromAssistantLinesOnly() throws {
        let data = try fixture()
        let read = SessionUsageReader.read(data)
        // Two lines of one Opus response, Haiku, Sonnet; no user line, synthetic notice,
        // broken line, or the last line that has no newline yet.
        #expect(read.records.map(\.model) == ["claude-opus-5-5", "claude-opus-5-5", "claude-haiku-5-5", "claude-sonnet-4-5-20250929"])
        let partial = try #require(String(data: data, encoding: .utf8)?.split(separator: "\n").last)
        #expect(read.consumed == data.count - partial.utf8.count)

        let opus = read.records[1]
        #expect(opus.key == read.records[0].key)
        // The message's own usage, not the tool input's.
        #expect(opus.tokens == TokenCounts(input: 10, output: 420, cacheRead: 30000, cacheWrite5m: 0, cacheWrite1h: 2000))
        #expect(opus.date == Date(timeIntervalSince1970: 1_793_091_609.5))
        // An offset in the timestamp.
        #expect(read.records[2].date == Date(timeIntervalSince1970: 1_793_093_400))
        // No split by cache lifetime: all 5-minute writes.
        #expect(read.records[3].tokens.cacheWrite5m == 500)
        #expect(read.records[3].fast)
    }

    @Test func parsesTimestamps() {
        #expect(SessionUsageReader.parseTimestamp("1970-01-01T00:00:00Z") == Date(timeIntervalSince1970: 0))
        #expect(SessionUsageReader.parseTimestamp("2000-02-29T12:00:00.250Z") == Date(timeIntervalSince1970: 951_825_600.25))
        #expect(SessionUsageReader.parseTimestamp("2026-10-27T11:30:00-01:30") == Date(timeIntervalSince1970: 1_793_106_000))
        #expect(SessionUsageReader.parseTimestamp("yesterday") == nil)
        #expect(SessionUsageReader.parseTimestamp("2026-13-01T00:00:00Z") == nil)
    }

    // MARK: - The index

    private struct Scratch {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("usage-\(UUID().uuidString)")
        var config: URL { root.appendingPathComponent("config") }
        var sessions: URL { config.appendingPathComponent("projects/-tmp-project") }
        var data: URL { root.appendingPathComponent("data") }

        init() throws {
            try FileManager.default.createDirectory(at: sessions, withIntermediateDirectories: true)
        }

        func remove() { try? FileManager.default.removeItem(at: root) }
    }

    private var utc: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        return calendar
    }

    private let now = Date(timeIntervalSince1970: 1_793_188_800) // 2026-10-28 12:00 UTC

    @Test func sumsByDayAndModel() throws {
        let scratch = try Scratch()
        defer { scratch.remove() }
        try fixture().write(to: scratch.sessions.appendingPathComponent("aaaaaaaa-0000-4000-8000-000000000000.jsonl"))
        let store = UsageStore(directory: scratch.data)
        let result = store.refresh(configDirectory: scratch.config)
        #expect(result.filesRead == 1)

        let report = store.report(days: 7, now: now, calendar: utc)
        #expect(report.days.count == 7)
        #expect(report.days.first?.date == utc.date(from: DateComponents(year: 2026, month: 10, day: 22)))
        let yesterday = try #require(report.days.first { $0.date == utc.date(from: DateComponents(year: 2026, month: 10, day: 27)) })
        #expect(Set(yesterday.models.keys) == ["claude-opus-5-5", "claude-haiku-5-5"])
        #expect(yesterday.models["claude-opus-5-5"]?.tokens.output == 420)
        #expect(try #require(yesterday.models["claude-opus-5-5"]?.cost).isClose(to: 0.03044))
        // Over 100K tokens of prompt: Haiku's higher prices.
        #expect(try #require(yesterday.models["claude-haiku-5-5"]?.cost).isClose(to: 0.062))
        #expect(yesterday.topModel == "claude-haiku-5-5")
        let today = try #require(report.days.last)
        #expect(today.models.keys.sorted() == ["claude-sonnet-4-5"])
        #expect(report.total.messages == 3)
        #expect(report.total.cost.isClose(to: 0.03044 + 0.062 + 0.006375))
        #expect(report.models.map(\.model) == ["claude-haiku-5-5", "claude-opus-5-5", "claude-sonnet-4-5"])
        // Colors go to the newest models first: Sonnet was first used today.
        #expect(report.allModels == ["claude-sonnet-4-5", "claude-haiku-5-5", "claude-opus-5-5"])

        // A day is found from its date: any time in it, and nothing outside the range.
        #expect(report.day(containing: yesterday.date.addingTimeInterval(23 * 3600), calendar: utc)?.date == yesterday.date)
        #expect(report.day(containing: now, calendar: utc)?.date == today.date)
        #expect(report.day(containing: now.addingTimeInterval(-30 * 86_400), calendar: utc) == nil)
        #expect(report.day(containing: now.addingTimeInterval(86_400), calendar: utc) == nil)

        // A shorter range leaves the older days out.
        #expect(store.report(days: 1, now: now, calendar: utc).total.messages == 1)
    }

    @Test func readsOnlyWhatsNewAndCountsCopiesOnce() throws {
        let scratch = try Scratch()
        defer { scratch.remove() }
        let file = scratch.sessions.appendingPathComponent("aaaaaaaa-0000-4000-8000-000000000000.jsonl")
        try fixture().write(to: file)
        let store = UsageStore(directory: scratch.data)
        store.refresh(configDirectory: scratch.config)
        #expect(store.refresh(configDirectory: scratch.config).filesTotal == 0)

        // The last line is finished.
        let handle = try FileHandle(forWritingTo: file)
        try handle.seekToEnd()
        try handle.write(contentsOf: Data("}\n".utf8))
        try handle.close()
        let appended = store.refresh(configDirectory: scratch.config)
        #expect(appended.filesRead == 1)
        #expect(appended.records == 1)
        #expect(store.report(days: 7, now: now, calendar: utc).total.messages == 4)

        // A fork's file carries the history it came from; a sub-agent's sits in a folder.
        let fork = scratch.sessions.appendingPathComponent("bbbbbbbb-0000-4000-8000-000000000000.jsonl")
        try FileManager.default.copyItem(at: file, to: fork)
        let subagents = scratch.sessions.appendingPathComponent("aaaaaaaa-0000-4000-8000-000000000000/subagents")
        try FileManager.default.createDirectory(at: subagents, withIntermediateDirectories: true)
        try FileManager.default.copyItem(at: file, to: subagents.appendingPathComponent("agent-1.jsonl"))
        #expect(store.refresh(configDirectory: scratch.config).filesRead == 2)
        #expect(store.report(days: 7, now: now, calendar: utc).total.messages == 4)

        // Claude Code deleting old sessions doesn't take their history with them.
        try FileManager.default.removeItem(at: scratch.config)
        store.refresh(configDirectory: scratch.config)
        #expect(store.report(days: 7, now: now, calendar: utc).total.messages == 4)
    }

    @Test func keepsTheAppsOwnSessions() throws {
        let scratch = try Scratch()
        defer { scratch.remove() }
        let store = UsageStore(directory: scratch.data)
        let date = now.addingTimeInterval(-60)
        let first = MessageUsage(key: 1, date: date, model: "claude-opus-5-5", tokens: TokenCounts(input: 100, output: 10))
        var later = first
        later.tokens.output = 50
        store.record([first, later])
        store.record([MessageUsage(key: 2, date: date, model: "claude-mystery-1", tokens: TokenCounts(input: 7))])
        let report = store.report(days: 1, now: now, calendar: utc)
        #expect(report.total.messages == 2)
        #expect(report.total.tokens.output == 50)
        #expect(report.total.unpricedTokens == 7)
        #expect(report.allModels.first == "claude-opus-5-5")

        // Still there when opened again.
        #expect(UsageStore(directory: scratch.data).report(days: 1, now: now, calendar: utc).total.messages == 2)
    }

    // MARK: - Streams

    @Test func streamsReportEachResponsesUsage() {
        var parser = ClaudeStreamParser()
        let lines = [
            #"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_1","model":"claude-haiku-5-5","usage":{"input_tokens":5}}}}"#,
            #"{"type":"assistant","message":{"id":"msg_1","model":"claude-haiku-5-5","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":5,"cache_read_input_tokens":100,"output_tokens":1}}}"#,
            #"{"type":"stream_event","event":{"type":"message_delta","usage":{"input_tokens":5,"cache_read_input_tokens":100,"output_tokens":12}}}"#,
        ]
        let usages = lines.flatMap { parser.parse(line: Data($0.utf8)) }.compactMap { event -> MessageUsage? in
            if case .messageUsage(let usage) = event { usage } else { nil }
        }
        #expect(usages.count == 2)
        #expect(Set(usages.map(\.key)).count == 1)
        #expect(usages.map(\.tokens.output) == [1, 12])
        #expect(usages.allSatisfy { $0.model == "claude-haiku-5-5" && $0.tokens.cacheRead == 100 })
    }
}

private extension Double {
    func isClose(to other: Double) -> Bool { abs(self - other) < 1e-9 }
}
