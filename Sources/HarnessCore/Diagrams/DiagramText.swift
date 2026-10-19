import CoreGraphics
import Foundation

/// How the renderer measures text. Layout lives here, fonts live in the app, so the app
/// hands in a width function for its font and the line height that goes with it.
public struct TextMetrics: Sendable {
    public var lineHeight: Double
    public var width: @Sendable (String) -> Double

    public init(lineHeight: Double, width: @escaping @Sendable (String) -> Double) {
        self.lineHeight = lineHeight
        self.width = width
    }

    /// Fixed-width stand-in for tests: every character is `characterWidth` wide.
    public static func monospaced(characterWidth: Double = 7, lineHeight: Double = 16) -> TextMetrics {
        TextMetrics(lineHeight: lineHeight) { Double($0.count) * characterWidth }
    }
}

/// Text broken into lines that fit a width, plus the size it takes.
public struct WrappedText: Sendable, Equatable {
    public var lines: [String]
    public var size: CGSize

    public static let empty = WrappedText(lines: [], size: .zero)

    public var isEmpty: Bool { lines.isEmpty }

    /// Greedy word wrap. Explicit line breaks (`\n`, `<br>`) are kept; a single word longer
    /// than the width gets its own line rather than being split.
    public static func wrap(_ text: String, maxWidth: Double, metrics: TextMetrics) -> WrappedText {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return .empty }
        var lines: [String] = []
        for paragraph in text.components(separatedBy: "\n") {
            var current = ""
            for word in paragraph.split(separator: " ", omittingEmptySubsequences: true) {
                let candidate = current.isEmpty ? String(word) : current + " " + word
                if current.isEmpty || metrics.width(candidate) <= maxWidth {
                    current = candidate
                } else {
                    lines.append(current)
                    current = String(word)
                }
            }
            lines.append(current)
        }
        let width = lines.map(metrics.width).max() ?? 0
        return WrappedText(lines: lines, size: CGSize(width: width, height: Double(lines.count) * metrics.lineHeight))
    }
}

enum MermaidText {
    /// Mermaid label text to plain text: quotes, `<br>`, a few entities and simple tags.
    static func clean(_ raw: String) -> String {
        var text = raw.trimmingCharacters(in: .whitespaces)
        if text.count >= 2, text.hasPrefix("\""), text.hasSuffix("\"") {
            text = String(text.dropFirst().dropLast())
        }
        if text.count >= 2, text.hasPrefix("`"), text.hasSuffix("`") {
            text = String(text.dropFirst().dropLast())
        }
        text = text.replacingOccurrences(of: #"<br\s*/?>"#, with: "\n", options: [.regularExpression, .caseInsensitive])
        text = text.replacingOccurrences(of: #"</?(b|i|em|strong|u|code|small|span)[^>]*>"#, with: "", options: [.regularExpression, .caseInsensitive])
        for (entity, value) in ["#quot;": "\"", "&quot;": "\"", "&amp;": "&", "&lt;": "<", "&gt;": ">", "#35;": "#", "#59;": ";", "&nbsp;": " "] {
            text = text.replacingOccurrences(of: entity, with: value)
        }
        // Mermaid markdown strings: drop emphasis markers rather than show them.
        text = text.replacingOccurrences(of: "**", with: "")
        return text.trimmingCharacters(in: .whitespaces)
    }

    /// Statements, without comments and blank lines. Semicolons separate statements too,
    /// except inside quotes and brackets.
    static func statements(_ source: String) -> [String] {
        var result: [String] = []
        for rawLine in source.components(separatedBy: .newlines) {
            var line = rawLine
            if let comment = line.range(of: "%%") { line = String(line[..<comment.lowerBound]) }
            var current = ""
            var depth = 0
            var inQuote = false
            for character in line {
                switch character {
                case "\"": inQuote.toggle()
                case "[", "(", "{": if !inQuote { depth += 1 }
                case "]", ")", "}": if !inQuote { depth = max(0, depth - 1) }
                default: break
                }
                if character == ";", depth == 0, !inQuote {
                    result.append(current)
                    current = ""
                } else {
                    current.append(character)
                }
            }
            result.append(current)
        }
        return result.map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
    }
}

public struct MermaidError: Error, Equatable, Sendable, CustomStringConvertible {
    public var message: String

    public init(_ message: String) {
        self.message = message
    }

    public var description: String { message }
}
