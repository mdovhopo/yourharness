import AppKit
import SwiftUI

/// Block-level markdown for assistant messages. Inline syntax (bold, code, links) goes
/// through `AttributedString`; blocks are parsed here because Foundation only does inline.
struct MarkdownView: View {
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(MarkdownBlock.parse(text).enumerated()), id: \.offset) { _, block in
                BlockView(block: block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

enum MarkdownBlock: Equatable {
    case heading(level: Int, text: String)
    case paragraph(String)
    case listItem(marker: String, text: String, indent: Int)
    case code(language: String, text: String)
    case quote(String)
    case rule

    static func parse(_ source: String) -> [MarkdownBlock] {
        var blocks: [MarkdownBlock] = []
        var paragraph: [Substring] = []
        var code: (language: String, lines: [Substring])?

        func flushParagraph() {
            if !paragraph.isEmpty {
                blocks.append(.paragraph(paragraph.joined(separator: "\n")))
                paragraph.removeAll()
            }
        }

        for line in source.split(separator: "\n", omittingEmptySubsequences: false) {
            let trimmed = line.drop { $0 == " " }

            if var open = code {
                if trimmed.hasPrefix("```") {
                    blocks.append(.code(language: open.language, text: open.lines.joined(separator: "\n")))
                    code = nil
                } else {
                    open.lines.append(line)
                    code = open
                }
                continue
            }

            if trimmed.hasPrefix("```") {
                flushParagraph()
                code = (String(trimmed.dropFirst(3)).trimmingCharacters(in: .whitespaces), [])
            } else if trimmed.isEmpty {
                flushParagraph()
            } else if let heading = Self.heading(trimmed) {
                flushParagraph()
                blocks.append(heading)
            } else if let item = Self.listItem(line) {
                flushParagraph()
                blocks.append(item)
            } else if trimmed.hasPrefix(">") {
                flushParagraph()
                let text = String(trimmed.dropFirst().drop { $0 == " " })
                if case .quote(let previous) = blocks.last {
                    blocks[blocks.count - 1] = .quote(previous + "\n" + text)
                } else {
                    blocks.append(.quote(text))
                }
            } else if trimmed.allSatisfy({ $0 == "-" || $0 == "*" || $0 == "_" }) && trimmed.count >= 3 {
                flushParagraph()
                blocks.append(.rule)
            } else {
                paragraph.append(trimmed)
            }
        }

        flushParagraph()
        // An unterminated fence is a code block still streaming in.
        if let open = code {
            blocks.append(.code(language: open.language, text: open.lines.joined(separator: "\n")))
        }
        return blocks
    }

    private static func heading(_ line: Substring) -> MarkdownBlock? {
        let hashes = line.prefix { $0 == "#" }.count
        guard (1...6).contains(hashes), line.dropFirst(hashes).first == " " else { return nil }
        return .heading(level: hashes, text: String(line.dropFirst(hashes + 1)))
    }

    private static func listItem(_ line: Substring) -> MarkdownBlock? {
        let indent = line.prefix { $0 == " " }.count
        let rest = line.dropFirst(indent)
        if let first = rest.first, "-*+".contains(first), rest.dropFirst().first == " " {
            return .listItem(marker: "•", text: String(rest.dropFirst(2)), indent: indent / 2)
        }
        let digits = rest.prefix(while: \.isNumber)
        if !digits.isEmpty, rest.dropFirst(digits.count).hasPrefix(". ") {
            return .listItem(marker: digits + ".", text: String(rest.dropFirst(digits.count + 2)), indent: indent / 2)
        }
        return nil
    }
}

private struct BlockView: View {
    let block: MarkdownBlock

    var body: some View {
        switch block {
        case .heading(let level, let text):
            InlineText(text)
                .font(level == 1 ? .title2.bold() : level == 2 ? .title3.bold() : .headline)
                .padding(.top, 4)
        case .paragraph(let text):
            InlineText(text)
                .lineSpacing(2)
        case .listItem(let marker, let text, let indent):
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(marker)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
                InlineText(text)
            }
            .padding(.leading, CGFloat(indent) * 18 + 4)
        case .code(let language, let text):
            CodeBlockView(language: language, code: text)
        case .quote(let text):
            HStack(spacing: 10) {
                Capsule()
                    .fill(.tint.opacity(0.5))
                    .frame(width: 3)
                InlineText(text)
                    .foregroundStyle(.secondary)
            }
        case .rule:
            Divider()
        }
    }
}

private struct InlineText: View {
    let source: String

    init(_ source: String) {
        self.source = source
    }

    var body: some View {
        Text(Self.attributed(source))
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
    }

    private static func attributed(_ source: String) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        return (try? AttributedString(markdown: source, options: options)) ?? AttributedString(source)
    }
}

private struct CodeBlockView: View {
    let language: String
    let code: String
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(language.isEmpty ? "code" : language)
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
                Spacer()
                Button(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(code, forType: .string)
                    copied = true
                    Task {
                        try? await Task.sleep(for: .seconds(1.5))
                        copied = false
                    }
                }
                .labelStyle(.titleAndIcon)
                .buttonStyle(.borderless)
                .font(.caption)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)

            ScrollView(.horizontal) {
                Text(code)
                    .font(.system(.callout, design: .monospaced))
                    .textSelection(.enabled)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 10)
            }
        }
        .background(.quaternary.opacity(0.45), in: .rect(cornerRadius: 10))
    }
}
