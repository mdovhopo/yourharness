import Foundation

/// Block-level markdown, the subset agents actually write: headings, paragraphs, lists
/// (nested, numbered, task), fenced code, quotes, tables and rules. Inline syntax is left
/// to `AttributedString(markdown:)` at render time.
public enum MarkdownBlock: Equatable, Sendable {
    case heading(level: Int, text: String)
    case paragraph(String)
    case list([MarkdownListItem])
    case code(language: String, text: String)
    case quote(String)
    case table(MarkdownTable)
    case rule
}

public struct MarkdownListItem: Equatable, Sendable {
    public enum Marker: Equatable, Sendable {
        case bullet
        case number(Int)
    }

    public var marker: Marker
    /// Nil for a plain item; otherwise whether the `- [x]` box is checked.
    public var task: Bool?
    public var text: String
    public var depth: Int
}

public struct MarkdownTable: Equatable, Sendable {
    public enum Alignment: Equatable, Sendable {
        case leading, center, trailing
    }

    public var header: [String]
    public var alignments: [Alignment]
    public var rows: [[String]]

    public var columnCount: Int { header.count }
}

public enum MarkdownParser {
    /// With `streaming`, the text is a prefix of a message still arriving: constructs that
    /// would render one way now and another a moment later (a table row before its
    /// separator, a lone `#`) are held back instead of flashing.
    public static func parse(_ source: String, streaming: Bool = false) -> [MarkdownBlock] {
        var lines = source.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        if streaming, let last = lines.last, !source.hasSuffix("\n"), isPendingLine(last) {
            lines.removeLast()
        }

        var blocks: [MarkdownBlock] = []
        var paragraph: [String] = []
        var index = 0

        func flushParagraph() {
            if !paragraph.isEmpty {
                blocks.append(.paragraph(paragraph.joined(separator: "\n")))
                paragraph.removeAll()
            }
        }

        while index < lines.count {
            let line = lines[index]
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            // Fenced code
            if let fence = fenceMarker(trimmed) {
                flushParagraph()
                let language = String(trimmed.dropFirst(fence.count)).trimmingCharacters(in: .whitespaces)
                var body: [String] = []
                index += 1
                while index < lines.count, !lines[index].trimmingCharacters(in: .whitespaces).hasPrefix(fence) {
                    body.append(lines[index])
                    index += 1
                }
                index += 1 // closing fence, or past the end while still streaming
                blocks.append(.code(language: language, text: body.joined(separator: "\n")))
                continue
            }

            if trimmed.isEmpty {
                flushParagraph()
                index += 1
                continue
            }

            if let heading = heading(trimmed) {
                flushParagraph()
                blocks.append(heading)
                index += 1
                continue
            }

            if isRule(trimmed) {
                flushParagraph()
                blocks.append(.rule)
                index += 1
                continue
            }

            // Tables need a header row and a separator row.
            if trimmed.hasPrefix("|") || (trimmed.contains("|") && index + 1 < lines.count && isTableSeparator(lines[index + 1])) {
                if index + 1 < lines.count, isTableSeparator(lines[index + 1]) {
                    flushParagraph()
                    let header = cells(trimmed)
                    let alignments = cells(lines[index + 1]).map(alignment)
                    var rows: [[String]] = []
                    index += 2
                    while index < lines.count {
                        let row = lines[index].trimmingCharacters(in: .whitespaces)
                        guard !row.isEmpty, row.contains("|") else { break }
                        rows.append(normalized(cells(row), count: header.count))
                        index += 1
                    }
                    blocks.append(.table(MarkdownTable(
                        header: header,
                        alignments: normalized(alignments, count: header.count, filler: .leading),
                        rows: rows
                    )))
                    continue
                }
                if streaming, lines[(index + 1)...].allSatisfy({ $0.trimmingCharacters(in: .whitespaces).isEmpty }) {
                    // A header row whose separator hasn't arrived yet.
                    index += 1
                    continue
                }
            }

            if trimmed.hasPrefix(">") {
                flushParagraph()
                var quote: [String] = []
                while index < lines.count {
                    let quoted = lines[index].trimmingCharacters(in: .whitespaces)
                    guard quoted.hasPrefix(">") else { break }
                    quote.append(String(quoted.dropFirst().drop { $0 == " " }))
                    index += 1
                }
                blocks.append(.quote(quote.joined(separator: "\n")))
                continue
            }

            if listItem(line) != nil {
                flushParagraph()
                var items: [MarkdownListItem] = []
                while index < lines.count {
                    let current = lines[index]
                    if let item = listItem(current) {
                        items.append(item)
                        index += 1
                        continue
                    }
                    let content = current.trimmingCharacters(in: .whitespaces)
                    if content.isEmpty {
                        // A blank line inside a list only continues it if another item follows.
                        var next = index + 1
                        while next < lines.count, lines[next].trimmingCharacters(in: .whitespaces).isEmpty { next += 1 }
                        if next < lines.count, listItem(lines[next]) != nil {
                            index = next
                            continue
                        }
                        break
                    }
                    // Continuation of the previous item, unless it starts something else.
                    let startsBlock = fenceMarker(content) != nil || heading(content) != nil || isRule(content)
                        || content.hasPrefix(">") || content.hasPrefix("|")
                    if startsBlock || items.isEmpty { break }
                    items[items.count - 1].text += "\n" + content
                    index += 1
                }
                blocks.append(.list(items))
                continue
            }

            paragraph.append(trimmed)
            index += 1
        }

        flushParagraph()
        return blocks
    }

    // MARK: - Pieces

    static func fenceMarker(_ trimmed: String) -> String? {
        if trimmed.hasPrefix("```") { return "```" }
        if trimmed.hasPrefix("~~~") { return "~~~" }
        return nil
    }

    static func heading(_ trimmed: String) -> MarkdownBlock? {
        let hashes = trimmed.prefix { $0 == "#" }.count
        guard (1...6).contains(hashes), trimmed.dropFirst(hashes).first == " " else { return nil }
        var text = trimmed.dropFirst(hashes + 1).trimmingCharacters(in: .whitespaces)
        // Closing hashes ("## Title ##") are decoration.
        while text.hasSuffix("#") { text.removeLast() }
        return .heading(level: hashes, text: text.trimmingCharacters(in: .whitespaces))
    }

    static func isRule(_ trimmed: String) -> Bool {
        let compact = trimmed.filter { $0 != " " }
        guard compact.count >= 3, let first = compact.first, "-*_".contains(first) else { return false }
        return compact.allSatisfy { $0 == first }
    }

    static func listItem(_ line: String) -> MarkdownListItem? {
        let indent = line.prefix { $0 == " " || $0 == "\t" }.reduce(0) { $0 + ($1 == "\t" ? 4 : 1) }
        let rest = line.drop { $0 == " " || $0 == "\t" }
        var marker: MarkdownListItem.Marker
        var text: Substring
        if let first = rest.first, "-*+".contains(first), rest.dropFirst().first == " " {
            marker = .bullet
            text = rest.dropFirst(2)
        } else {
            let digits = rest.prefix(while: \.isNumber)
            let after = rest.dropFirst(digits.count)
            guard !digits.isEmpty, digits.count <= 9, let number = Int(digits),
                  after.first == "." || after.first == ")", after.dropFirst().first == " "
            else { return nil }
            marker = .number(number)
            text = after.dropFirst(2)
        }
        var task: Bool?
        if text.hasPrefix("[ ] ") {
            task = false
            text = text.dropFirst(4)
        } else if text.hasPrefix("[x] ") || text.hasPrefix("[X] ") {
            task = true
            text = text.dropFirst(4)
        }
        return MarkdownListItem(marker: marker, task: task, text: String(text), depth: min(indent / 2, 6))
    }

    static func isTableSeparator(_ line: String) -> Bool {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        guard trimmed.contains("-"), trimmed.contains("|") || trimmed.hasPrefix(":") || trimmed.hasPrefix("-") else { return false }
        let parts = cells(trimmed)
        return !parts.isEmpty && parts.allSatisfy { cell in
            let core = cell.trimmingCharacters(in: CharacterSet(charactersIn: ":"))
            return !core.isEmpty && core.allSatisfy { $0 == "-" }
        }
    }

    /// Splits a table row on unescaped pipes outside code spans.
    static func cells(_ row: String) -> [String] {
        var trimmed = row.trimmingCharacters(in: .whitespaces)
        if trimmed.hasPrefix("|") { trimmed.removeFirst() }
        if trimmed.hasSuffix("|"), !trimmed.hasSuffix("\\|") { trimmed.removeLast() }
        var cells: [String] = []
        var current = ""
        var inCode = false
        var escaped = false
        for character in trimmed {
            if escaped {
                current.append(character)
                escaped = false
            } else if character == "\\" {
                escaped = true
            } else if character == "`" {
                inCode.toggle()
                current.append(character)
            } else if character == "|", !inCode {
                cells.append(current.trimmingCharacters(in: .whitespaces))
                current = ""
            } else {
                current.append(character)
            }
        }
        cells.append(current.trimmingCharacters(in: .whitespaces))
        return cells
    }

    static func alignment(_ separatorCell: String) -> MarkdownTable.Alignment {
        switch (separatorCell.hasPrefix(":"), separatorCell.hasSuffix(":")) {
        case (true, true): .center
        case (false, true): .trailing
        default: .leading
        }
    }

    static func normalized<T>(_ values: [T], count: Int, filler: T) -> [T] {
        values.count >= count ? Array(values.prefix(count)) : values + Array(repeating: filler, count: count - values.count)
    }

    static func normalized(_ values: [String], count: Int) -> [String] {
        normalized(values, count: count, filler: "")
    }

    /// A partial last line that is about to become something else.
    static func isPendingLine(_ line: String) -> Bool {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        if trimmed.isEmpty { return false }
        if trimmed.hasPrefix("|") { return true }
        if trimmed.allSatisfy({ $0 == "#" }) { return true }
        if trimmed.allSatisfy({ $0 == "`" || $0 == "~" }) { return true }
        if ["-", "*", "+"].contains(trimmed) { return true }
        if trimmed.allSatisfy({ $0.isNumber || $0 == "." || $0 == ")" }) && trimmed.count <= 4 { return true }
        return false
    }
}

/// Inline fixes for text that is still streaming in.
public enum MarkdownInline {
    /// Closes or drops syntax that's been opened but not finished yet, so `**bold` renders
    /// bold right away instead of showing raw asterisks that snap into place later.
    public static func closingUnterminated(_ text: String) -> String {
        var text = text

        // A link still being typed: show its label until the URL completes.
        if let range = text.range(of: #"!?\[([^\[\]\n]*)\]\([^)\s]*$"#, options: .regularExpression) {
            let label = text[range].drop { $0 == "!" }.dropFirst().prefix { $0 != "]" }
            text.replaceSubrange(range, with: label)
        } else if let range = text.range(of: #"!?\[([^\[\]\n]*)$"#, options: .regularExpression) {
            text.replaceSubrange(range, with: text[range].drop { $0 == "!" }.dropFirst())
        }

        let backticks = text.filter { $0 == "`" }.count
        if backticks % 2 == 1 {
            if text.hasSuffix("`") { text.removeLast() } else { text += "`" }
        }

        // Count emphasis outside code spans only.
        let outsideCode = text.split(separator: "`", omittingEmptySubsequences: false)
            .enumerated().filter { $0.offset % 2 == 0 }.map(\.element).joined()
        for marker in ["**", "~~"] {
            if outsideCode.components(separatedBy: marker).count % 2 == 0 {
                if text.hasSuffix(marker) { text.removeLast(marker.count) } else { text += marker }
            }
        }
        let singles = outsideCode.replacingOccurrences(of: "**", with: "").filter { $0 == "*" }.count
        if singles % 2 == 1 {
            if text.hasSuffix("*"), !text.hasSuffix("**") { text.removeLast() } else { text += "*" }
        }
        return text
    }
}
