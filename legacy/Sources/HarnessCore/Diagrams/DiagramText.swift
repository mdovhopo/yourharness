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

/// An sRGB color from Mermaid styling (`#f96`, `#ff9966cc`, `rgb(…)`, `red`).
public struct DiagramColor: Sendable, Equatable {
    public var red: Double
    public var green: Double
    public var blue: Double
    public var alpha: Double

    public init(red: Double, green: Double, blue: Double, alpha: Double = 1) {
        self.red = red
        self.green = green
        self.blue = blue
        self.alpha = alpha
    }

    static let named: [String: (Double, Double, Double)] = [
        "red": (0.90, 0.28, 0.30), "orange": (0.96, 0.58, 0.20), "yellow": (0.95, 0.80, 0.20),
        "green": (0.20, 0.70, 0.40), "blue": (0.25, 0.50, 0.95), "purple": (0.60, 0.40, 0.90),
        "pink": (0.95, 0.45, 0.65), "teal": (0.20, 0.65, 0.70), "gray": (0.55, 0.55, 0.58),
        "grey": (0.55, 0.55, 0.58), "black": (0, 0, 0), "white": (1, 1, 1),
    ]

    public static func parse(_ raw: String) -> DiagramColor? {
        let text = raw.trimmingCharacters(in: .whitespaces).lowercased()
        if let (r, g, b) = named[text] { return DiagramColor(red: r, green: g, blue: b) }
        if text.hasPrefix("#") {
            var hex = String(text.dropFirst())
            if hex.count == 3 || hex.count == 4 { hex = hex.map { "\($0)\($0)" }.joined() }
            guard hex.count == 6 || hex.count == 8, let value = UInt64(hex, radix: 16) else { return nil }
            let hasAlpha = hex.count == 8
            let shift: UInt64 = hasAlpha ? 8 : 0
            return DiagramColor(
                red: Double((value >> (16 + shift)) & 0xFF) / 255,
                green: Double((value >> (8 + shift)) & 0xFF) / 255,
                blue: Double((value >> shift) & 0xFF) / 255,
                alpha: hasAlpha ? Double(value & 0xFF) / 255 : 1
            )
        }
        if text.hasPrefix("rgb"), let open = text.firstIndex(of: "("), let close = text.lastIndex(of: ")") {
            let parts = text[text.index(after: open)..<close].split(separator: ",").compactMap { Double($0.trimmingCharacters(in: .whitespaces)) }
            guard parts.count >= 3 else { return nil }
            return DiagramColor(red: parts[0] / 255, green: parts[1] / 255, blue: parts[2] / 255, alpha: parts.count > 3 ? parts[3] : 1)
        }
        return nil
    }
}

/// The subset of Mermaid node styling we honor: fill, outline color, dashes and width.
/// Text color is ignored on purpose so labels stay readable in light and dark mode.
public struct ShapeStyle: Sendable, Equatable {
    public var fill: DiagramColor?
    public var stroke: DiagramColor?
    public var dashed = false
    public var strokeWidth: Double?

    public init() {}

    public var isEmpty: Bool { self == ShapeStyle() }

    /// `fill:#f96,stroke:#333,stroke-width:2px,stroke-dasharray: 5 5`
    public static func parse(_ declarations: String) -> ShapeStyle {
        var style = ShapeStyle()
        // Commas separate declarations, except inside `rgb(…)`.
        var parts: [String] = [""]
        var depth = 0
        for character in declarations {
            if character == "(" { depth += 1 }
            if character == ")" { depth = max(0, depth - 1) }
            if character == ",", depth == 0 { parts.append("") } else { parts[parts.count - 1].append(character) }
        }
        for declaration in parts {
            let parts = declaration.split(separator: ":", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
            guard parts.count == 2 else { continue }
            switch parts[0].lowercased() {
            case "fill": style.fill = DiagramColor.parse(parts[1])
            case "stroke": style.stroke = DiagramColor.parse(parts[1])
            case "stroke-width": style.strokeWidth = Double(parts[1].lowercased().replacingOccurrences(of: "px", with: "").trimmingCharacters(in: .whitespaces))
            case "stroke-dasharray": style.dashed = !["0", "none", ""].contains(parts[1].lowercased())
            default: break
            }
        }
        return style
    }

    /// `other` wins where it says something.
    public func merged(with other: ShapeStyle) -> ShapeStyle {
        var result = self
        if let fill = other.fill { result.fill = fill }
        if let stroke = other.stroke { result.stroke = stroke }
        if other.dashed { result.dashed = true }
        if let width = other.strokeWidth { result.strokeWidth = width }
        return result
    }
}
