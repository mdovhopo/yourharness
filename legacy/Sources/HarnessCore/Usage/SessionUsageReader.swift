import Foundation

/// One API response's usage: which model, when, and its tokens.
public struct MessageUsage: Sendable, Equatable {
    /// Unique per response: the message and request IDs hashed, so a response written into
    /// more than one line or file (one line per content block, a forked or resumed session's
    /// copied history) counts once.
    public var key: Int64
    public var date: Date
    public var model: String
    public var tokens: TokenCounts
    /// Fast mode, which costs more.
    public var fast: Bool

    public init(key: Int64, date: Date, model: String, tokens: TokenCounts, fast: Bool = false) {
        self.key = key
        self.date = date
        self.model = model
        self.tokens = tokens
        self.fast = fast
    }

    /// From a `usage` object as the API sends it, or nil when there's nothing in it.
    public init?(messageID: String, requestID: String?, model: String, usage: [String: Any], date: Date) {
        func int(_ key: String, in object: [String: Any] = usage) -> Int { (object[key] as? NSNumber)?.intValue ?? 0 }
        let creation = usage["cache_creation"] as? [String: Any]
        let tokens = Self.tokens(input: int("input_tokens"), output: int("output_tokens"),
                                 cacheRead: int("cache_read_input_tokens"), cacheWrite: int("cache_creation_input_tokens"),
                                 write5m: creation.map { int("ephemeral_5m_input_tokens", in: $0) },
                                 write1h: creation.map { int("ephemeral_1h_input_tokens", in: $0) })
        guard !tokens.isEmpty, Self.isRealModel(model) else { return nil }
        self.init(key: Self.key(messageID: messageID, requestID: requestID), date: date, model: model,
                  tokens: tokens, fast: usage["speed"] as? String == "fast")
    }

    /// Without the split by cache lifetime (older CLIs), every cache write is the 5-minute kind.
    static func tokens(input: Int, output: Int, cacheRead: Int, cacheWrite: Int, write5m: Int?, write1h: Int?) -> TokenCounts {
        var counts = TokenCounts(input: input, output: output, cacheRead: cacheRead, cacheWrite5m: cacheWrite)
        if let write1h, write5m != nil || write1h > 0 {
            counts.cacheWrite1h = min(write1h, cacheWrite)
            counts.cacheWrite5m = cacheWrite - counts.cacheWrite1h
        }
        return counts
    }

    /// Claude Code writes its own notices (API errors, interruptions) as `<synthetic>` replies.
    static func isRealModel(_ model: String) -> Bool {
        !model.isEmpty && !model.hasPrefix("<")
    }

    /// The same response read twice: the larger count of each kind.
    func merging(_ other: MessageUsage) -> MessageUsage {
        var merged = self
        merged.tokens = TokenCounts(input: max(tokens.input, other.tokens.input), output: max(tokens.output, other.tokens.output),
                                    cacheRead: max(tokens.cacheRead, other.tokens.cacheRead),
                                    cacheWrite5m: max(tokens.cacheWrite5m, other.tokens.cacheWrite5m),
                                    cacheWrite1h: max(tokens.cacheWrite1h, other.tokens.cacheWrite1h))
        return merged
    }

    public static func key(messageID: String, requestID: String?) -> Int64 {
        stableHash("\(messageID):\(requestID ?? "")")
    }

    /// FNV-1a: the same on every launch, unlike `Hasher`.
    static func stableHash(_ text: String) -> Int64 {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in text.utf8 {
            hash ^= UInt64(byte)
            hash = hash &* 0x0000_0100_0000_01b3
        }
        return Int64(bitPattern: hash)
    }
}

/// Reads the usage Claude Code records with every reply in its session files
/// (`<config>/projects/<folder>/<session>.jsonl`, and sub-agents' files beside them).
/// Only lines that mention `usage` are decoded, and only the few fields needed.
public enum SessionUsageReader {
    /// The usage in `data`'s complete lines, and how many bytes those lines took: a line
    /// still being written (no newline yet) is left for next time.
    public static func read(_ data: Data, source: String = "") -> (records: [MessageUsage], consumed: Int) {
        var records: [MessageUsage] = []
        var consumed = 0
        let decoder = JSONDecoder()
        data.withUnsafeBytes { (buffer: UnsafeRawBufferPointer) in
            guard let base = buffer.baseAddress?.assumingMemoryBound(to: UInt8.self) else { return }
            let count = buffer.count
            var start = 0
            while start < count {
                guard let newline = memchr(base + start, 0x0A, count - start) else { break }
                let end = base.distance(to: newline.assumingMemoryBound(to: UInt8.self))
                let length = end - start
                if length > 0, contains(base + start, length, usageKey) {
                    let line = Data(bytesNoCopy: UnsafeMutableRawPointer(mutating: base + start), count: length, deallocator: .none)
                    if let record = record(line, decoder: decoder, fallbackKey: "\(source):\(start)") {
                        records.append(record)
                    }
                }
                start = end + 1
                consumed = start
            }
        }
        return (records, consumed)
    }

    private static let usageKey = Array(#""usage""#.utf8)

    private static func contains(_ bytes: UnsafePointer<UInt8>, _ length: Int, _ needle: [UInt8]) -> Bool {
        needle.withUnsafeBufferPointer { needle in
            memmem(bytes, length, needle.baseAddress, needle.count) != nil
        }
    }

    private struct Line: Decodable {
        var type: String?
        var timestamp: String?
        var requestId: String?
        var message: Message?

        struct Message: Decodable {
            var id: String?
            var model: String?
            var usage: Usage?
        }

        struct Usage: Decodable {
            var input_tokens: Int?
            var output_tokens: Int?
            var cache_read_input_tokens: Int?
            var cache_creation_input_tokens: Int?
            var cache_creation: CacheCreation?
            var speed: String?
        }

        struct CacheCreation: Decodable {
            var ephemeral_5m_input_tokens: Int?
            var ephemeral_1h_input_tokens: Int?
        }
    }

    static func record(_ line: Data, decoder: JSONDecoder, fallbackKey: String) -> MessageUsage? {
        // A field of a new type mustn't lose the whole line, so each is optional; a line
        // that doesn't decode at all is skipped.
        guard let decoded = try? decoder.decode(Line.self, from: line),
              decoded.type == "assistant",
              let message = decoded.message, let usage = message.usage,
              let model = message.model, MessageUsage.isRealModel(model),
              let date = decoded.timestamp.flatMap(parseTimestamp)
        else { return nil }
        let tokens = MessageUsage.tokens(
            input: usage.input_tokens ?? 0, output: usage.output_tokens ?? 0,
            cacheRead: usage.cache_read_input_tokens ?? 0, cacheWrite: usage.cache_creation_input_tokens ?? 0,
            write5m: usage.cache_creation?.ephemeral_5m_input_tokens, write1h: usage.cache_creation?.ephemeral_1h_input_tokens)
        guard !tokens.isEmpty else { return nil }
        let key = message.id.map { MessageUsage.key(messageID: $0, requestID: decoded.requestId) }
            ?? MessageUsage.stableHash(fallbackKey)
        return MessageUsage(key: key, date: date, model: model, tokens: tokens, fast: usage.speed == "fast")
    }

    /// `2026-10-28T09:41:07.123Z`, and offsets like `+02:00`, without a formatter: a year of
    /// history is a million of these.
    static func parseTimestamp(_ text: String) -> Date? {
        let bytes = Array(text.utf8)
        guard bytes.count >= 19, bytes[4] == UInt8(ascii: "-"), bytes[7] == UInt8(ascii: "-"),
              bytes[10] == UInt8(ascii: "T") || bytes[10] == UInt8(ascii: " ")
        else { return nil }
        func number(_ from: Int, _ length: Int) -> Int? {
            var value = 0
            for index in from..<(from + length) {
                let digit = Int(bytes[index]) - 48
                guard (0...9).contains(digit) else { return nil }
                value = value * 10 + digit
            }
            return value
        }
        guard let year = number(0, 4), let month = number(5, 2), let day = number(8, 2),
              let hour = number(11, 2), let minute = number(14, 2), let second = number(17, 2),
              (1...12).contains(month), (1...31).contains(day)
        else { return nil }
        var index = 19
        var fraction = 0.0
        if index < bytes.count, bytes[index] == UInt8(ascii: ".") {
            index += 1
            var scale = 0.1
            while index < bytes.count {
                let digit = Int(bytes[index]) - 48
                guard (0...9).contains(digit) else { break }
                fraction += Double(digit) * scale
                scale /= 10
                index += 1
            }
        }
        var offset = 0
        if index < bytes.count, bytes[index] == UInt8(ascii: "+") || bytes[index] == UInt8(ascii: "-"), bytes.count >= index + 6,
           let hours = number(index + 1, 2), let minutes = number(index + 4, 2) {
            offset = (hours * 3600 + minutes * 60) * (bytes[index] == UInt8(ascii: "-") ? -1 : 1)
        }
        // Days from 1970-01-01 to the date (Howard Hinnant's days_from_civil).
        let y = month <= 2 ? year - 1 : year
        let era = (y >= 0 ? y : y - 399) / 400
        let yearOfEra = y - era * 400
        let dayOfYear = (153 * (month + (month > 2 ? -3 : 9)) + 2) / 5 + day - 1
        let dayOfEra = yearOfEra * 365 + yearOfEra / 4 - yearOfEra / 100 + dayOfYear
        let days = era * 146_097 + dayOfEra - 719_468
        let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset
        return Date(timeIntervalSince1970: Double(seconds) + fraction)
    }
}
