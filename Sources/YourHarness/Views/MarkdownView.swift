import AppKit
import HarnessCore
import SwiftUI

enum MarkdownMetrics {
    static let body: CGFloat = 14
    static let small: CGFloat = 13
}

extension EnvironmentValues {
    /// Turns `#12` and friends into PR/issue links for the thread being shown.
    @Entry var issueLinker = IssueLinker(repos: [], openPullRequests: [])
}

/// Assistant text. While a reply streams in, text is revealed at a steady pace, a word at
/// a time, instead of in network-sized bursts; when the stream ends it catches up quickly.
struct StreamingMarkdown: View {
    let entry: TranscriptEntry
    /// Set while revealing; nil shows the entry's full text.
    @State private var shown: String?

    var body: some View {
        // `??` only reads `entry.text` when not revealing, so deltas don't re-render this
        // view directly: the reveal loop paces them.
        MarkdownView(text: shown ?? entry.text, streaming: shown != nil)
            .task(id: entry.id) { await reveal() }
    }

    private func reveal() async {
        guard entry.isStreaming else { return }
        // Start from what's already there: a row re-created mid-stream shouldn't replay.
        var count = entry.text.count
        shown = entry.text
        var last = ContinuousClock.now
        while !Task.isCancelled {
            // 30 Hz reads as smooth for word-sized steps and halves the layout work of 60.
            try? await Task.sleep(for: .milliseconds(33))
            let full = entry.text
            let target = full.count
            let now = ContinuousClock.now
            let elapsed = (now - last) / .seconds(1)
            last = now

            if count < target {
                let streaming = entry.isStreaming
                let backlog = Double(target - count)
                // Fast enough to never fall far behind, slow enough to read as typing.
                let speed = max(streaming ? 80 : 1_200, backlog / (streaming ? 0.35 : 0.12))
                let next = Self.wordBoundary(in: full, after: min(target, count + max(1, Int(speed * elapsed))), limit: target)
                count = next
                shown = String(full.prefix(count))
            } else if !entry.isStreaming {
                break
            }
        }
        shown = nil
    }

    /// Extends a cut point to the end of the current word, so lines don't rewrap mid-word.
    private static func wordBoundary(in text: String, after offset: Int, limit: Int) -> Int {
        guard offset < limit else { return limit }
        var index = text.index(text.startIndex, offsetBy: offset)
        var position = offset
        while position < limit, position - offset < 16, !text[index].isWhitespace {
            index = text.index(after: index)
            position += 1
        }
        return position
    }
}

struct MarkdownView: View {
    let text: String
    var streaming = false

    var body: some View {
        let blocks = MarkdownParser.parse(text, streaming: streaming)
        VStack(alignment: .leading, spacing: 12) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { index, block in
                BlockView(block: block, isTail: streaming && index == blocks.count - 1)
                    .equatable()
            }
        }
        // A touch larger than the 13pt system body: this is the text you read all day.
        .font(.system(size: MarkdownMetrics.body))
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Equatable on its content, so while streaming only the last block re-renders.
private struct BlockView: View, Equatable {
    let block: MarkdownBlock
    let isTail: Bool

    nonisolated static func == (lhs: BlockView, rhs: BlockView) -> Bool {
        lhs.block == rhs.block && lhs.isTail == rhs.isTail
    }

    var body: some View {
        switch block {
        case .heading(let level, let text):
            InlineText(text, isTail: isTail)
                .font(Self.headingFont(level))
                .padding(.top, level <= 2 ? 8 : 2)
                .accessibilityAddTraits(.isHeader)
        case .paragraph(let text):
            InlineText(text, isTail: isTail)
                .lineSpacing(3)
        case .list(let items):
            ListBlock(items: items, isTail: isTail)
        case .code(let language, let text):
            if language.lowercased() == "mermaid" {
                if isTail {
                    // Still streaming: the fence may not be closed yet.
                    DiagramPlaceholder()
                } else {
                    MermaidBlockView(source: text)
                }
            } else {
                CodeBlockView(language: language, code: text)
            }
        case .quote(let text):
            HStack(alignment: .top, spacing: 12) {
                Capsule()
                    .fill(.tint.opacity(0.45))
                    .frame(width: 3)
                InlineText(text, isTail: isTail)
                    .foregroundStyle(.secondary)
                    .lineSpacing(3)
            }
            .fixedSize(horizontal: false, vertical: true)
        case .table(let table):
            TableBlock(table: table)
        case .rule:
            Divider()
                .padding(.vertical, 4)
        }
    }

    private static func headingFont(_ level: Int) -> Font {
        switch level {
        case 1: .system(size: 22, weight: .semibold)
        case 2: .system(size: 18, weight: .semibold)
        case 3: .system(size: 15.5, weight: .semibold)
        default: .system(size: MarkdownMetrics.body, weight: .semibold)
        }
    }
}

private struct ListBlock: View {
    let items: [MarkdownListItem]
    let isTail: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Marker(item: item)
                    InlineText(item.text, isTail: isTail && index == items.count - 1)
                        .lineSpacing(3)
                        .strikethrough(item.task == true, color: .secondary)
                        .foregroundStyle(item.task == true ? AnyShapeStyle(.secondary) : AnyShapeStyle(.primary))
                }
                .padding(.leading, CGFloat(item.depth) * 20)
            }
        }
    }

    private struct Marker: View {
        let item: MarkdownListItem

        var body: some View {
            Group {
                if let done = item.task {
                    Image(systemName: done ? "checkmark.square.fill" : "square")
                        .foregroundStyle(done ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
                } else {
                    switch item.marker {
                    case .bullet:
                        Text(["•", "◦", "▪︎"][item.depth % 3])
                            .foregroundStyle(.secondary)
                    case .number(let number):
                        Text("\(number).")
                            .monospacedDigit()
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .frame(minWidth: 14, alignment: .trailing)
        }
    }
}

private struct TableBlock: View {
    let table: MarkdownTable

    var body: some View {
        Grid(alignment: .leading, horizontalSpacing: 0, verticalSpacing: 0) {
            GridRow {
                ForEach(0..<table.columnCount, id: \.self) { column in
                    cell(table.header[column], column: column)
                        .font(.system(size: MarkdownMetrics.small, weight: .semibold))
                        .background(.fill.quaternary)
                        .gridColumnAlignment(alignment(column))
                }
            }
            ForEach(Array(table.rows.enumerated()), id: \.offset) { _, row in
                Divider()
                GridRow {
                    ForEach(0..<table.columnCount, id: \.self) { column in
                        cell(row[column], column: column)
                            .font(.system(size: MarkdownMetrics.small))
                    }
                }
            }
        }
        .clipShape(.rect(cornerRadius: Theme.cardRadius, style: .continuous))
        .card()
        .fixedSize(horizontal: false, vertical: true)
    }

    private func cell(_ text: String, column: Int) -> some View {
        InlineText(text)
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: frameAlignment(column))
    }

    private func alignment(_ column: Int) -> HorizontalAlignment {
        switch table.alignments[column] {
        case .leading: .leading
        case .center: .center
        case .trailing: .trailing
        }
    }

    private func frameAlignment(_ column: Int) -> Alignment {
        switch table.alignments[column] {
        case .leading: .topLeading
        case .center: .top
        case .trailing: .topTrailing
        }
    }
}

/// Inline markdown via `AttributedString`, plus subtle backgrounds on code spans, bare
/// URLs as links, and `#12`-style references linked to their PR or issue.
/// Text with links that shows the pointing hand over them, as links do everywhere else
/// on the Mac. Selectable text doesn't: its links keep the text cursor, and it's drawn
/// without custom renderers. So an invisible, non-selectable twin with the same text in
/// the same frame lays out identically, notes where its link pieces land, and draws
/// nothing; hovering checks those frames.
private struct LinkedText: View {
    let text: AttributedString
    @State private var linkFrames = LinkFrames()
    @State private var overLink = false

    var body: some View {
        Text(text)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
            .background {
                Self.tagged(text)
                    .textRenderer(LinkFrameRenderer(frames: linkFrames))
                    .fixedSize(horizontal: false, vertical: true)
                    .allowsHitTesting(false)
                    .accessibilityHidden(true)
            }
            .onContinuousHover { phase in
                let isOver = switch phase {
                case .active(let location): linkFrames.rects.contains { $0.contains(location) }
                case .ended: false
                }
                if isOver != overLink { overLink = isOver }
            }
            .pointerStyle(overLink ? .link : nil)
    }

    /// One Text, with each link run carrying a marker the renderer can see.
    private static func tagged(_ text: AttributedString) -> Text {
        text.runs.reduce(Text(verbatim: "")) { result, run in
            let piece = Text(AttributedString(text[run.range]))
            return run.link == nil ? Text("\(result)\(piece)") : Text("\(result)\(piece.customAttribute(LinkMarker()))")
        }
    }
}

private struct LinkMarker: TextAttribute {}

/// Written while drawing, read while hovering; never observed, so drawing doesn't loop.
private final class LinkFrames {
    var rects: [CGRect] = []
}

/// Draws nothing: it only notes where the link pieces are.
private struct LinkFrameRenderer: TextRenderer {
    let frames: LinkFrames

    func draw(layout: Text.Layout, in context: inout GraphicsContext) {
        var rects: [CGRect] = []
        for line in layout {
            for run in line where run[LinkMarker.self] != nil {
                rects.append(run.typographicBounds.rect)
            }
        }
        frames.rects = rects
    }
}

private struct InlineText: View {
    let source: String
    var isTail = false
    @Environment(\.issueLinker) private var linker

    init(_ source: String, isTail: Bool = false) {
        self.source = source
        self.isTail = isTail
    }

    var body: some View {
        let rendered = Self.render(isTail ? MarkdownInline.closingUnterminated(source) : source, linker: linker)
        if rendered.runs.contains(where: { $0.link != nil }) {
            LinkedText(text: rendered)
        } else {
            Text(rendered)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    static func render(_ source: String, linker: IssueLinker) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        var text = (try? AttributedString(markdown: source, options: options)) ?? AttributedString(source)

        for run in text.runs where run.inlinePresentationIntent?.contains(.code) == true {
            text[run.range].backgroundColor = Color.primary.opacity(0.07)
        }

        let plain = String(text.characters)
        guard plain.contains("://") || plain.contains("#") || plain.contains("!") else { return text }

        func link(_ offsets: Range<Int>, to url: URL) {
            let characters = text.characters
            guard offsets.upperBound <= characters.count else { return }
            let start = characters.index(characters.startIndex, offsetBy: offsets.lowerBound)
            let end = characters.index(start, offsetBy: offsets.count)
            let range = start..<end
            // Leave code spans and links the author already made alone.
            guard !text[range].runs.contains(where: { $0.link != nil || $0.inlinePresentationIntent?.contains(.code) == true }) else { return }
            text[range].link = url
        }

        for (offsets, url) in bareURLs(in: plain) {
            link(offsets, to: url)
        }
        for reference in IssueReference.find(in: plain) {
            if let url = linker.url(for: reference, context: plain) {
                link(reference.range, to: url)
            }
        }
        return text
    }

    private static let urlPattern = try? NSRegularExpression(pattern: #"https?://[^\s<>()\[\]`]+[^\s<>()\[\]`.,;:!?'"]"#)

    private static func bareURLs(in text: String) -> [(Range<Int>, URL)] {
        guard let pattern = urlPattern, text.contains("://") else { return [] }
        let ns = text as NSString
        return pattern.matches(in: text, range: NSRange(location: 0, length: ns.length)).compactMap { match in
            guard let range = Range(match.range, in: text), let url = URL(string: String(text[range])) else { return nil }
            let lower = text.distance(from: text.startIndex, to: range.lowerBound)
            return (lower..<(lower + text[range].count), url)
        }
    }
}

struct CodeBlockView: View {
    let language: String
    let code: String
    @State private var copied = false
    @State private var hovering = false

    var body: some View {
        ZStack(alignment: .topTrailing) {
            ScrollView(.horizontal) {
                Text(code)
                    .font(.system(size: MarkdownMetrics.small, design: .monospaced))
                    .lineSpacing(2)
                    .textSelection(.enabled)
                    .padding(.horizontal, 14)
                    .padding(.top, language.isEmpty ? 12 : 28)
                    .padding(.bottom, 12)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .scrollIndicators(.never)

            HStack(spacing: 8) {
                if !language.isEmpty {
                    Text(language)
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.tertiary)
                }
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
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .font(.caption)
                .opacity(hovering || copied ? 1 : 0)
                .help("Copy code")
            }
            .padding(.horizontal, 12)
            .padding(.top, 8)
        }
        .card()
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
    }
}
