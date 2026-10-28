import Foundation

/// Tokens of one kind or another, as the API counts them.
public struct TokenCounts: Sendable, Equatable, Hashable {
    public var input = 0
    public var output = 0
    public var cacheRead = 0
    /// Written to the 5-minute cache.
    public var cacheWrite5m = 0
    /// Written to the 1-hour cache, which costs more.
    public var cacheWrite1h = 0

    public init(input: Int = 0, output: Int = 0, cacheRead: Int = 0, cacheWrite5m: Int = 0, cacheWrite1h: Int = 0) {
        self.input = input
        self.output = output
        self.cacheRead = cacheRead
        self.cacheWrite5m = cacheWrite5m
        self.cacheWrite1h = cacheWrite1h
    }

    public var cacheWrite: Int { cacheWrite5m + cacheWrite1h }
    /// Everything the model read: what a request's prompt length is measured by.
    public var prompt: Int { input + cacheRead + cacheWrite }
    public var total: Int { prompt + output }
    public var isEmpty: Bool { total == 0 }

    public static func + (lhs: TokenCounts, rhs: TokenCounts) -> TokenCounts {
        TokenCounts(input: lhs.input + rhs.input, output: lhs.output + rhs.output,
                    cacheRead: lhs.cacheRead + rhs.cacheRead,
                    cacheWrite5m: lhs.cacheWrite5m + rhs.cacheWrite5m,
                    cacheWrite1h: lhs.cacheWrite1h + rhs.cacheWrite1h)
    }

    public static func += (lhs: inout TokenCounts, rhs: TokenCounts) { lhs = lhs + rhs }
}

/// What a model costs at API prices, in dollars per million tokens.
public struct ModelPrice: Sendable, Equatable {
    public var input: Double
    public var output: Double
    public var cacheWrite5m: Double
    public var cacheWrite1h: Double
    public var cacheRead: Double

    public init(input: Double, output: Double, cacheWrite5m: Double, cacheWrite1h: Double, cacheRead: Double) {
        self.input = input
        self.output = output
        self.cacheWrite5m = cacheWrite5m
        self.cacheWrite1h = cacheWrite1h
        self.cacheRead = cacheRead
    }

    public func cost(_ tokens: TokenCounts) -> Double {
        (Double(tokens.input) * input
            + Double(tokens.output) * output
            + Double(tokens.cacheWrite5m) * cacheWrite5m
            + Double(tokens.cacheWrite1h) * cacheWrite1h
            + Double(tokens.cacheRead) * cacheRead) / 1_000_000
    }

    func scaled(_ factor: Double) -> ModelPrice {
        ModelPrice(input: input * factor, output: output * factor, cacheWrite5m: cacheWrite5m * factor,
                   cacheWrite1h: cacheWrite1h * factor, cacheRead: cacheRead * factor)
    }
}

/// Anthropic's API prices for every Claude model, in one place, so a subscription's usage
/// can be put in dollars: what the same tokens would cost on the API, not what you pay.
///
/// Source: https://platform.claude.com/docs/en/about-claude/pricing (model pricing, fast
/// mode and long context pricing), checked 2026-10-28. Update `checked` with the table.
public enum ModelPricing {
    public static let source = URL(string: "https://platform.claude.com/docs/en/about-claude/pricing")!
    public static let checked = "2026-10-28"

    public struct Entry: Sendable {
        public var price: ModelPrice
        /// Prices for a request whose prompt is longer than `longPromptThreshold` (Haiku 5.5).
        public var longPrompt: ModelPrice?
        /// Fast mode's multiplier, for the models that have it.
        public var fast: Double?
    }

    /// Haiku 5.5 charges more for a request whose prompt (input plus cache reads and writes)
    /// is over this many tokens.
    public static let longPromptThreshold = 100_000

    private static func price(_ input: Double, _ output: Double, write5m: Double, write1h: Double, read: Double) -> ModelPrice {
        ModelPrice(input: input, output: output, cacheWrite5m: write5m, cacheWrite1h: write1h, cacheRead: read)
    }

    /// Keyed by the model's ID without its date (see `family(of:)`).
    static let table: [String: Entry] = {
        let fable51 = price(10, 50, write5m: 12.5, write1h: 20, read: 0.25)
        let fable5 = price(10, 50, write5m: 12.5, write1h: 20, read: 1)
        let opus5 = price(5, 25, write5m: 6.25, write1h: 10, read: 0.5)
        let opus41 = price(15, 75, write5m: 18.75, write1h: 30, read: 1.5)
        let sonnet4 = price(3, 15, write5m: 3.75, write1h: 6, read: 0.3)
        return [
            "claude-fable-5-1": Entry(price: fable51),
            "claude-mythos-5-1": Entry(price: fable51),
            "claude-fable-5": Entry(price: fable5),
            "claude-mythos-5": Entry(price: fable5),
            // Fast mode: $8 / $40 on Opus 5.5, $10 / $50 on Opus 5 and 4.8; caching on top.
            "claude-opus-5-5": Entry(price: price(4, 20, write5m: 5, write1h: 8, read: 0.2), fast: 2),
            "claude-opus-5": Entry(price: opus5, fast: 2),
            "claude-opus-4-8": Entry(price: opus5, fast: 2),
            "claude-opus-4-7": Entry(price: opus5),
            "claude-opus-4-6": Entry(price: opus5),
            "claude-opus-4-5": Entry(price: opus5),
            "claude-opus-4-1": Entry(price: opus41),
            "claude-opus-4": Entry(price: opus41),
            "claude-sonnet-5-5": Entry(price: price(2, 10, write5m: 2.5, write1h: 4, read: 0.1)),
            "claude-sonnet-5": Entry(price: price(2, 10, write5m: 2.5, write1h: 4, read: 0.2)),
            "claude-sonnet-4-6": Entry(price: sonnet4),
            "claude-sonnet-4-5": Entry(price: sonnet4),
            "claude-sonnet-4": Entry(price: sonnet4),
            "claude-haiku-5-5": Entry(price: price(0.1, 0.5, write5m: 0.125, write1h: 0.2, read: 0.01),
                                      longPrompt: price(0.5, 2.5, write5m: 0.625, write1h: 1, read: 0.05)),
            "claude-haiku-4-5": Entry(price: price(1, 5, write5m: 1.25, write1h: 2, read: 0.1)),
            "claude-3-5-haiku": Entry(price: price(0.8, 4, write5m: 1, write1h: 1.6, read: 0.08)),
        ]
    }()

    /// A model ID as one family: `claude-opus-4-5-20251101`, `us.anthropic.claude-opus-4-5-20251101-v1:0`,
    /// `claude-opus-4-5@20251101` and `claude-opus-4-5[1m]` are all `claude-opus-4-5`.
    public static func family(of model: String) -> String {
        var id = model.lowercased().trimmingCharacters(in: .whitespaces)
        if let start = id.range(of: "claude-") { id = String(id[start.lowerBound...]) }
        for separator in ["[", "@", ":"] {
            if let cut = id.firstIndex(of: Character(separator)) { id = String(id[..<cut]) }
        }
        if id.hasSuffix("-v1") || id.hasSuffix("-v2") { id.removeLast(3) }
        // A trailing date: -20251101.
        if let dash = id.lastIndex(of: "-") {
            let tail = id[id.index(after: dash)...]
            if tail.count == 8, tail.allSatisfy(\.isNumber) { id = String(id[..<dash]) }
        }
        return id
    }

    public static func entry(for model: String) -> Entry? {
        table[family(of: model)]
    }

    /// What one request cost at API prices; nil for a model that isn't in the table.
    public static func cost(_ tokens: TokenCounts, model: String, fast: Bool = false) -> Double? {
        guard let entry = entry(for: model) else { return nil }
        var price = entry.price
        if let long = entry.longPrompt, tokens.prompt > longPromptThreshold { price = long }
        if fast, let factor = entry.fast { price = price.scaled(factor) }
        return price.cost(tokens)
    }

    /// "Opus 5.5", "Haiku 3.5", "Sonnet 4": for people.
    public static func displayName(_ model: String) -> String {
        let family = family(of: model)
        guard family.hasPrefix("claude-") else { return model }
        let parts = family.dropFirst("claude-".count).split(separator: "-").map(String.init)
        let names = parts.filter { $0.first?.isLetter == true }
        let numbers = parts.filter { $0.allSatisfy(\.isNumber) }
        guard !names.isEmpty else { return model }
        let name = names.map(\.capitalized).joined(separator: " ")
        return numbers.isEmpty ? name : "\(name) \(numbers.joined(separator: "."))"
    }
}
