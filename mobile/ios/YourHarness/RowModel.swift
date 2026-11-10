// A transcript row ready to draw: made off the main thread as a message is decoded, so the
// main thread only styles and lays out. Markdown is parsed here, once per copy of a row.

import Foundation

struct RowModel: Sendable {
    let row: Row
    let content: RowContent
    var seq: Int { row.seq }
    var rev: Int { row.rev }

    init(_ row: Row) {
        self.row = row
        content = RowContent(row)
    }
}

enum RowContent: Sendable {
    case user(String, attachments: [String])
    case reply([MDBlock])
    case thinking(String)
    case tool(ToolSummary)
    /// A sub-agent's call (`Agent`, `Task`): a card with its run.
    case agent(ToolSummary, SubagentInfo?)
    /// Pictures Claude showed you (`show_media`): their paths on the Mac.
    case media(ToolSummary, pictures: [String])
    case notice(String, style: String?)
    case other(kind: String, text: String)

    init(_ row: Row) {
        switch row.kind {
        case "user": self = .user(row.text, attachments: row.attachments ?? [])
        case "assistant": self = .reply(Markdown.blocks(row.text))
        case "thinking": self = .thinking(row.text)
        case "tool" where row.subagent != nil || ["Agent", "Task"].contains(row.toolName ?? ""):
            self = .agent(ToolSummary(row), row.subagent)
        case "tool" where row.toolName == Pictures.mediaTool:
            self = .media(ToolSummary(row), pictures: Pictures.shown(row.toolInput))
        case "tool": self = .tool(ToolSummary(row))
        case "notice": self = .notice(row.text, style: row.noticeStyle)
        default: self = .other(kind: row.kind, text: row.text)
        }
    }
}

/// A tool call as one line: what it did, to what.
struct ToolSummary: Sendable {
    let name: String
    let symbol: String
    let target: String?
    let failed: Bool
    let running: Bool

    init(_ row: Row) {
        let tool = row.toolName ?? "Tool"
        failed = row.toolIsError == true
        running = row.toolResult == nil && row.toolResultLength == nil && row.toolIsError == nil
        name = ToolSummary.displayName(tool)
        symbol = switch tool {
        case "Bash", "BashOutput", "KillShell": "terminal"
        case "Read": "doc.text"
        case "Edit", "MultiEdit", "Write", "NotebookEdit": "pencil"
        case "Grep", "Glob", "ToolSearch": "magnifyingglass"
        case "WebFetch", "WebSearch": "globe"
        case "Agent", "Task": "person.2"
        case "TodoWrite", "TaskCreate", "TaskUpdate", "TaskList": "checklist"
        default: tool.hasPrefix("mcp__browser__") ? "safari" : "wrench.and.screwdriver"
        }
        target = ToolSummary.target(tool: tool, input: row.toolInput ?? "")
    }

    /// `mcp__browser__click` as "browser · click".
    static func displayName(_ tool: String) -> String {
        let parts = tool.split(separator: "__", omittingEmptySubsequences: true)
        guard tool.hasPrefix("mcp__"), parts.count >= 3 else { return tool }
        return "\(parts[1]) · \(parts[2...].joined(separator: "_"))"
    }

    /// The input field that says what the call was about. The input may be cut short on the
    /// wire, so fields are found by pattern rather than by parsing it whole.
    static func target(tool: String, input: String) -> String? {
        for key in ["file_path", "notebook_path", "path", "command", "pattern", "url", "query", "description", "selector", "prompt"] {
            guard let value = field(key, in: input), !value.isEmpty else { continue }
            switch key {
            case "file_path", "notebook_path", "path":
                let parts = value.split(separator: "/")
                return parts.suffix(2).joined(separator: "/")
            case "command", "prompt", "description":
                let line = value.split(separator: "\n", maxSplits: 1).first.map(String.init) ?? value
                return String(line.prefix(120))
            default:
                return String(value.prefix(120))
            }
        }
        return nil
    }

    static func field(_ key: String, in json: String) -> String? {
        guard let keyRange = json.range(of: "\"\(key)\"") else { return nil }
        var i = keyRange.upperBound
        while i < json.endIndex, json[i] == " " || json[i] == ":" { i = json.index(after: i) }
        guard i < json.endIndex, json[i] == "\"" else { return nil }
        i = json.index(after: i)
        var out = ""
        var escaped = false
        while i < json.endIndex {
            let c = json[i]
            if escaped {
                out.append(c == "n" ? "\n" : c == "t" ? "\t" : c)
                escaped = false
            } else if c == "\\" {
                escaped = true
            } else if c == "\"" {
                break
            } else {
                out.append(c)
            }
            i = json.index(after: i)
        }
        return out
    }
}

// MARK: - Markdown

enum MDBlock: Sendable {
    /// Paragraphs, headings, list items and quotes in a row: one text view.
    case text([MDParagraph])
    case code(language: String?, text: String)
    case rule
}

struct MDParagraph: Sendable {
    enum Kind: Sendable, Equatable {
        case body
        case heading(Int)
        case item(marker: String, depth: Int)
        case quote
    }

    let kind: Kind
    /// Inline styles as Foundation's markdown parser leaves them (`inlinePresentationIntent`, `link`).
    var text: AttributedString
}

enum Markdown {
    static func blocks(_ source: String) -> [MDBlock] {
        let options = AttributedString.MarkdownParsingOptions(
            allowsExtendedAttributes: false, interpretedSyntax: .full, failurePolicy: .returnPartiallyParsedIfPossible)
        guard let parsed = try? AttributedString(markdown: source, options: options) else {
            return [.text([MDParagraph(kind: .body, text: AttributedString(source))])]
        }
        var blocks: [MDBlock] = []
        var paragraphs: [MDParagraph] = []
        var lastTableRow: Int?
        func flush() {
            if !paragraphs.isEmpty { blocks.append(.text(paragraphs)) }
            paragraphs = []
        }
        for (intent, range) in parsed.runs[\.presentationIntent] {
            let text = AttributedString(parsed[range])
            guard let intent else {
                paragraphs.append(MDParagraph(kind: .body, text: text))
                continue
            }
            let kinds = intent.components.map(\.kind)
            if let code = kinds.first(where: { if case .codeBlock = $0 { true } else { false } }), case .codeBlock(let language) = code {
                flush()
                var body = String(text.characters)
                while body.hasSuffix("\n") { body.removeLast() }
                blocks.append(.code(language: language, text: body))
                continue
            }
            if kinds.contains(.thematicBreak) {
                flush()
                blocks.append(.rule)
                continue
            }
            // A table: each row one line, its cells joined.
            if let row = intent.components.first(where: { if case .tableRow = $0.kind { true } else if case .tableHeaderRow = $0.kind { true } else { false } }) {
                if lastTableRow == row.identity, var last = paragraphs.popLast() {
                    last.text += AttributedString("  ·  ") + text
                    paragraphs.append(last)
                } else {
                    paragraphs.append(MDParagraph(kind: .body, text: text))
                }
                lastTableRow = row.identity
                continue
            }
            lastTableRow = nil
            var kind = MDParagraph.Kind.body
            let depth = kinds.filter { $0 == .orderedList || $0 == .unorderedList }.count
            if depth > 0, let item = kinds.first(where: { if case .listItem = $0 { true } else { false } }), case .listItem(let ordinal) = item {
                let ordered = kinds.first(where: { $0 == .orderedList || $0 == .unorderedList }) == .orderedList
                kind = .item(marker: ordered ? "\(ordinal)." : "•", depth: depth)
            } else if case .header(let level) = kinds.first {
                kind = .heading(level)
            } else if kinds.contains(.blockQuote) {
                kind = .quote
            }
            paragraphs.append(MDParagraph(kind: kind, text: text))
        }
        flush()
        return blocks.map { block in
            guard case .text(let paragraphs) = block else { return block }
            return .text(paragraphs.map { MDParagraph(kind: $0.kind, text: linkingBareURLs($0.text)) })
        }
    }

    private static let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)

    /// Web addresses written out without markdown's brackets made links too, as the Mac shows
    /// them; never inside inline code.
    static func linkingBareURLs(_ text: AttributedString) -> AttributedString {
        guard let detector else { return text }
        let plain = String(text.characters)
        guard plain.contains("://") || plain.contains("www.") else { return text }
        var out = text
        for match in detector.matches(in: plain, range: NSRange(plain.startIndex..., in: plain)) {
            guard let url = match.url, ["http", "https"].contains(url.scheme?.lowercased() ?? ""),
                  let range = Range<AttributedString.Index>(match.range, in: out),
                  !out[range].runs.contains(where: { $0.link != nil || $0.inlinePresentationIntent?.contains(.code) == true })
            else { continue }
            out[range].link = url
        }
        return out
    }
}

/// Which of a row's files are pictures the Mac sends (`media`): the same rule as its server's.
enum Pictures {
    static let mediaTool = "mcp__harness__show_media"
    static func isPicture(_ name: String) -> Bool {
        ["png", "jpg", "jpeg", "gif", "webp", "heic"].contains(name.split(separator: ".").last.map { $0.lowercased() } ?? "")
    }
    /// Your attachments that are pictures kept in the app's attachments folder.
    static func yours(_ attachments: [String]) -> [String] {
        attachments.filter { isPicture($0) && !$0.contains("/") && !$0.contains("..") }
    }
    /// What `show_media` showed: its `paths`, then `path`, the pictures among them.
    static func shown(_ input: String?) -> [String] {
        guard let object = input.flatMap({ try? JSONSerialization.jsonObject(with: Data($0.utf8)) }) as? [String: Any] else { return [] }
        var paths = (object["paths"] as? [String]) ?? []
        if let p = object["path"] as? String { paths.append(p) }
        return paths.filter(isPicture)
    }
}
