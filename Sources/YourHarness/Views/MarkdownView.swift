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
    /// Turns paths into links to the thread's files.
    @Entry var fileLinker = FileLinker.empty
}

/// What text is linked with: PR and issue references, and the thread's files.
struct TextLinkers {
    var issues: IssueLinker
    var files: FileLinker
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
        let text = shown ?? entry.text
        MarkdownView(text: text, streaming: shown != nil)
            // Text grows in place. When a row is added in the same moment, the transcript
            // animates the change; text animated to its new height draws clipped meanwhile.
            .animation(nil, value: text)
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
        let groups = MarkdownGroup.make(blocks)
        VStack(alignment: .leading, spacing: 12) {
            ForEach(groups) { group in
                // Prose runs, tables included, are one piece of text, so selection runs across them.
                let isTail = streaming && group.id == groups.last?.id
                if let prose = group.prose {
                    RichText(blocks: prose, isTail: isTail)
                } else if let block = group.block {
                    BlockView(block: block, isTail: isTail)
                        .equatable()
                }
            }
        }
        // A touch larger than the 13pt system body: this is the text you read all day.
        .font(.system(size: MarkdownMetrics.body))
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A message's blocks, with the prose next to each other gathered into one run. Code and
/// diagrams stand on their own: code scrolls sideways.
struct MarkdownGroup: Identifiable {
    let id: Int
    var prose: [MarkdownBlock]?
    var block: MarkdownBlock?

    static func make(_ blocks: [MarkdownBlock]) -> [MarkdownGroup] {
        var groups: [MarkdownGroup] = []
        for block in blocks {
            switch block {
            case .heading, .paragraph, .list, .quote, .table, .rule:
                if groups.last?.prose != nil { groups[groups.count - 1].prose?.append(block) }
                else { groups.append(MarkdownGroup(id: groups.count, prose: [block])) }
            case .code:
                groups.append(MarkdownGroup(id: groups.count, block: block))
            }
        }
        return groups
    }
}

/// Inline markdown via `AttributedString`, plus bare URLs as links, `#12`-style
/// references linked to their PR or issue, and paths to the thread's files.
@MainActor
enum InlineMarkdown {
    static func render(_ source: String, linker: TextLinkers) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        var text = (try? AttributedString(markdown: source, options: options)) ?? AttributedString(source)

        let plain = String(text.characters)
        let mayLinkFiles = !linker.files.isEmpty && FileLinker.mightMention(plain)
        guard mayLinkFiles || plain.contains("://") || plain.contains("#") || plain.contains("!") else { return text }

        func link(_ offsets: Range<Int>, to url: URL, inCode: Bool = false) {
            let characters = text.characters
            guard offsets.upperBound <= characters.count else { return }
            let start = characters.index(characters.startIndex, offsetBy: offsets.lowerBound)
            let end = characters.index(start, offsetBy: offsets.count)
            let range = start..<end
            // Leave links the author already made alone, and code spans, except for paths:
            // `Sources/App.swift:12` is how files get named.
            guard !text[range].runs.contains(where: { $0.link != nil || (!inCode && $0.inlinePresentationIntent?.contains(.code) == true) }) else { return }
            text[range].link = url
        }

        for (offsets, url) in bareURLs(in: plain) {
            link(offsets, to: url)
        }
        for reference in IssueReference.find(in: plain) {
            if let url = linker.issues.url(for: reference, context: plain) {
                link(reference.range, to: url)
            }
        }
        if mayLinkFiles {
            for reference in linker.files.references(in: plain) {
                if let url = FileLink.url(path: linker.files.absolute(reference.path), line: reference.line, endLine: reference.endLine) {
                    link(reference.range, to: url, inCode: true)
                }
            }
        }
        return text
    }

    /// File links in plain text, like your own messages.
    static func linkFiles(_ text: inout AttributedString, files: FileLinker) {
        guard !files.isEmpty else { return }
        let plain = String(text.characters)
        for reference in files.references(in: plain) {
            guard let url = FileLink.url(path: files.absolute(reference.path), line: reference.line, endLine: reference.endLine) else { continue }
            let characters = text.characters
            guard reference.range.upperBound <= characters.count else { continue }
            let start = characters.index(characters.startIndex, offsetBy: reference.range.lowerBound)
            let end = characters.index(start, offsetBy: reference.range.count)
            text[start..<end].link = url
        }
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

/// A code block or a diagram. Equatable on its content, so while streaming only the last
/// one re-renders.
private struct BlockView: View, Equatable {
    let block: MarkdownBlock
    let isTail: Bool

    nonisolated static func == (lhs: BlockView, rhs: BlockView) -> Bool {
        lhs.block == rhs.block && lhs.isTail == rhs.isTail
    }

    var body: some View {
        if case .code(let language, let text) = block {
            if language.lowercased() == "mermaid" {
                if isTail {
                    // Still streaming: the fence may not be closed yet.
                    DiagramPlaceholder()
                } else {
                    MermaidBlockView(source: text)
                }
            } else {
                CodeBlockView(language: language, code: text, streaming: isTail)
            }
        }
    }
}

struct CodeBlockView: View {
    let language: String
    let code: String
    /// The reply is still streaming this block in.
    var streaming = false
    @State private var copied = false
    @State private var hovering = false
    @Environment(\.findHighlight) private var findHighlight

    var body: some View {
        ZStack(alignment: .topTrailing) {
            ScrollView(.horizontal) {
                // AppKit text, its size from its lines: measuring a long block costs nothing.
                let built = CodeText.build(code, language: language, streaming: streaming, find: findHighlight)
                NativeText(text: built.text, key: built.key, size: built.size)
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
