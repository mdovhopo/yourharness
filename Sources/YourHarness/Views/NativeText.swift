import AppKit
import HarnessCore
import SwiftUI

/// Transcript text drawn by AppKit: selectable the way Mac text is (drag across
/// paragraphs, double-click a word, ⌘C, Look Up), links with the pointing hand, and sized
/// from a cache, so laying out a long thread doesn't lay out its text over and over.
struct NativeText: NSViewRepresentable {
    let text: NSAttributedString
    /// What the text is, for the size cache: the same key always means the same text.
    let key: NativeTextKey
    /// False for code: lines run on, and `size` is known without laying anything out.
    var wraps = true
    var size: CGSize?
    @Environment(\.openURL) private var openURL

    func makeNSView(context: Context) -> TranscriptTextView {
        let view = TranscriptTextView(wraps: wraps)
        view.open = { openURL($0) }
        view.textStorage?.setAttributedString(text)
        view.shownKey = key
        return view
    }

    func updateNSView(_ view: TranscriptTextView, context: Context) {
        view.open = { openURL($0) }
        guard view.shownKey != key else { return }
        view.shownKey = key
        view.textStorage?.setAttributedString(text)
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView: TranscriptTextView, context: Context) -> CGSize? {
        if let size { return size }
        let width = max(proposal.width ?? 600, 40)
        return CGSize(width: width, height: NativeTextMetrics.height(of: text, key: key, width: width))
    }
}

/// Identifies a piece of transcript text, styling included.
struct NativeTextKey: Hashable {
    var source: String
    var style: String
    var find: String = ""
}

/// Heights by text and width, kept while the window keeps its width.
@MainActor
enum NativeTextMetrics {
    private static var heights: [NativeTextKey: [Int: CGFloat]] = [:]
    private static let storage = NSTextStorage()
    private static let container: NSTextContainer = {
        let container = NSTextContainer(size: CGSize(width: 100, height: CGFloat.greatestFiniteMagnitude))
        container.lineFragmentPadding = 0
        return container
    }()
    private static let layout: NSLayoutManager = {
        let layout = NSLayoutManager()
        layout.addTextContainer(container)
        storage.addLayoutManager(layout)
        return layout
    }()

    static func height(of text: NSAttributedString, key: NativeTextKey, width: CGFloat) -> CGFloat {
        let column = Int(width.rounded(.down))
        if let known = heights[key]?[column] { return known }
        container.size = CGSize(width: CGFloat(column), height: .greatestFiniteMagnitude)
        storage.setAttributedString(text)
        layout.ensureLayout(for: container)
        let height = ceil(layout.usedRect(for: container).height)
        if heights.count > 4_000 { heights.removeAll(keepingCapacity: true) }
        heights[key, default: [:]][column] = height
        return height
    }
}

/// Read-only text that selects like any Mac text and hands links to the app.
final class TranscriptTextView: NSTextView {
    var open: ((URL) -> Void)?
    var shownKey: NativeTextKey?

    init(wraps: Bool) {
        let storage = NSTextStorage()
        let layout = NSLayoutManager()
        storage.addLayoutManager(layout)
        let container = NSTextContainer(size: CGSize(width: wraps ? 100 : CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude))
        container.lineFragmentPadding = 0
        container.widthTracksTextView = wraps
        layout.addTextContainer(container)
        super.init(frame: .zero, textContainer: container)
        isEditable = false
        isSelectable = true
        isRichText = true
        drawsBackground = false
        textContainerInset = .zero
        isVerticallyResizable = false
        isHorizontallyResizable = !wraps
        linkTextAttributes = [.foregroundColor: NSColor.linkColor, .cursor: NSCursor.pointingHand]
        setAccessibilityRole(.staticText)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override init(frame frameRect: NSRect, textContainer container: NSTextContainer?) {
        super.init(frame: frameRect, textContainer: container)
    }

    // Selectable by clicking, but not a stop when tabbing: the composer is where Tab goes,
    // and AppKit re-gathering every transcript text as a key view on each change is slow.
    override var canBecomeKeyView: Bool { false }

    // One selection at a time, as in a document: selecting elsewhere clears this one.
    override func resignFirstResponder() -> Bool {
        setSelectedRange(NSRange(location: selectedRange().location, length: 0))
        return super.resignFirstResponder()
    }

    override func clicked(onLink link: Any, at charIndex: Int) {
        if let url = link as? URL ?? (link as? String).flatMap(URL.init(string:)) {
            open?(url)
        } else {
            super.clicked(onLink: link, at: charIndex)
        }
    }
}

// MARK: - Building the text

/// Markdown prose (headings, paragraphs, lists, quotes) as one attributed string, in the
/// transcript's fonts and colors.
@MainActor
enum ProseText {
    static let bodySize = MarkdownMetrics.body
    static let blockSpacing: CGFloat = 12

    static func build(_ blocks: [MarkdownBlock], tail: Bool, linker: IssueLinker, find: FindHighlight?) -> NSAttributedString {
        let result = NSMutableAttributedString()
        for (index, block) in blocks.enumerated() {
            let isTail = tail && index == blocks.count - 1
            let last = index == blocks.count - 1
            switch block {
            case .heading(let level, let text):
                let style = paragraph(spacingBefore: index == 0 ? 0 : (level <= 2 ? 8 : 2), after: last ? 0 : blockSpacing - 4)
                result.append(inline(text, tail: isTail, linker: linker, font: headingFont(level), paragraph: style))
            case .paragraph(let text):
                result.append(inline(text, tail: isTail, linker: linker, font: .systemFont(ofSize: bodySize), paragraph: paragraph(after: last ? 0 : blockSpacing)))
            case .quote(let text):
                let style = paragraph(after: last ? 0 : blockSpacing, indent: 14)
                result.append(inline(text, tail: isTail, linker: linker, font: .systemFont(ofSize: bodySize), paragraph: style, color: .secondaryLabelColor))
            case .list(let items):
                for (number, item) in items.enumerated() {
                    let lastItem = number == items.count - 1
                    let indent = CGFloat(item.depth) * 20
                    let style = paragraph(after: lastItem ? (last ? 0 : blockSpacing) : 6, indent: indent + 22, firstLine: indent, tab: indent + 22)
                    let marker: String = switch (item.task, item.marker) {
                    case (true?, _): "☑"
                    case (false?, _): "☐"
                    case (nil, .bullet): ["•", "◦", "▪︎"][item.depth % 3]
                    case (nil, .number(let n)): "\(n)."
                    }
                    let line = NSMutableAttributedString(string: marker + "\t", attributes: [
                        .font: NSFont.monospacedDigitSystemFont(ofSize: bodySize, weight: .regular),
                        .foregroundColor: NSColor.secondaryLabelColor, .paragraphStyle: style,
                    ])
                    let done = item.task == true
                    line.append(inline(item.text, tail: isTail && lastItem, linker: linker, font: .systemFont(ofSize: bodySize), paragraph: style,
                                       color: done ? .secondaryLabelColor : .labelColor, strike: done))
                    result.append(line)
                    if !lastItem { result.append(NSAttributedString(string: "\n", attributes: [.paragraphStyle: style])) }
                }
            case .code, .table, .rule:
                continue
            }
            if !last { result.append(NSAttributedString(string: "\n")) }
        }
        if let find { highlight(find, in: result) }
        return result
    }

    private static func paragraph(spacingBefore: CGFloat = 0, after: CGFloat, indent: CGFloat = 0, firstLine: CGFloat? = nil, tab: CGFloat? = nil) -> NSParagraphStyle {
        let style = NSMutableParagraphStyle()
        style.lineSpacing = 3
        style.paragraphSpacingBefore = spacingBefore
        style.paragraphSpacing = after
        style.headIndent = indent
        style.firstLineHeadIndent = firstLine ?? indent
        if let tab { style.tabStops = [NSTextTab(textAlignment: .left, location: tab)] }
        return style
    }

    private static func headingFont(_ level: Int) -> NSFont {
        switch level {
        case 1: .systemFont(ofSize: 22, weight: .semibold)
        case 2: .systemFont(ofSize: 18, weight: .semibold)
        case 3: .systemFont(ofSize: 15.5, weight: .semibold)
        default: .systemFont(ofSize: bodySize, weight: .semibold)
        }
    }

    /// Inline markdown (bold, italic, code, links, `#12`) in a font, as AppKit attributes.
    private static func inline(_ source: String, tail: Bool, linker: IssueLinker, font: NSFont, paragraph: NSParagraphStyle,
                               color: NSColor = .labelColor, strike: Bool = false) -> NSAttributedString {
        let rendered = InlineMarkdown.render(tail ? MarkdownInline.closingUnterminated(source) : source, linker: linker)
        let result = NSMutableAttributedString()
        for run in rendered.runs {
            let piece = String(rendered.characters[run.range])
            var attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color, .paragraphStyle: paragraph]
            let intent = run.inlinePresentationIntent ?? []
            var traits: NSFontDescriptor.SymbolicTraits = []
            if intent.contains(.stronglyEmphasized) { traits.insert(.bold) }
            if intent.contains(.emphasized) { traits.insert(.italic) }
            if intent.contains(.code) {
                attributes[.font] = NSFont.monospacedSystemFont(ofSize: font.pointSize - 1, weight: traits.contains(.bold) ? .semibold : .regular)
                attributes[.backgroundColor] = Self.codeBackground
            } else if !traits.isEmpty {
                attributes[.font] = NSFont(descriptor: font.fontDescriptor.withSymbolicTraits(font.fontDescriptor.symbolicTraits.union(traits)), size: font.pointSize) ?? font
            }
            if intent.contains(.strikethrough) || strike { attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
            if let link = run.link { attributes[.link] = link }
            result.append(NSAttributedString(string: piece, attributes: attributes))
        }
        return result
    }

    static let codeBackground = NSColor(name: nil) { appearance in
        appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? NSColor(white: 1, alpha: 0.09) : NSColor(white: 0, alpha: 0.06)
    }

    static func highlight(_ find: FindHighlight, in text: NSMutableAttributedString) {
        guard !find.query.isEmpty else { return }
        let plain = text.string as NSString
        let color = find.isCurrent ? NSColor.systemOrange.withAlphaComponent(0.6) : NSColor.systemYellow.withAlphaComponent(0.35)
        var search = NSRange(location: 0, length: plain.length)
        while true {
            let found = plain.range(of: find.query, options: [.caseInsensitive, .diacriticInsensitive], range: search)
            guard found.location != NSNotFound else { break }
            text.addAttribute(.backgroundColor, value: color, range: found)
            search = NSRange(location: found.upperBound, length: plain.length - found.upperBound)
        }
    }
}

/// A code block's text, highlighted, and its size worked out from its lines: a monospaced
/// font makes that arithmetic.
@MainActor
enum CodeText {
    static let font = NSFont.monospacedSystemFont(ofSize: MarkdownMetrics.small, weight: .regular)
    static let lineSpacing: CGFloat = 2
    private static var cache: [NativeTextKey: (text: NSAttributedString, size: CGSize)] = [:]

    static func build(_ code: String, language: String, streaming: Bool, find: FindHighlight?) -> (text: NSAttributedString, size: CGSize, key: NativeTextKey) {
        let key = NativeTextKey(source: code, style: "code:" + language, find: find.map { "\($0.query)|\($0.isCurrent)" } ?? "")
        if let cached = cache[key] { return (cached.text, cached.size, key) }
        let style = NSMutableParagraphStyle()
        style.lineSpacing = lineSpacing
        let text = NSMutableAttributedString(string: code, attributes: [.font: font, .foregroundColor: NSColor.labelColor, .paragraphStyle: style])
        let lines = code.split(separator: "\n", omittingEmptySubsequences: false)
        // Highlighting a long block on every streamed frame costs more than it shows.
        let highlights = SyntaxHighlighter.highlights(language) && code.utf8.count <= 200_000 && lines.count <= 2_000
            && !(streaming && lines.count > 400)
        if highlights {
            for token in SyntaxHighlighter.tokens(in: code, language: language) {
                guard let color = SyntaxPalette.nsColor(token.kind) else { continue }
                text.addAttribute(.foregroundColor, value: color, range: NSRange(token.range, in: code))
            }
        }
        if let find { ProseText.highlight(find, in: text) }
        let advance = ("M" as NSString).size(withAttributes: [.font: font]).width
        let lineHeight = ceil(font.ascender - font.descender + font.leading)
        let longest = lines.map { $0.count }.max() ?? 0
        let size = CGSize(width: ceil(CGFloat(longest) * advance) + 2,
                          height: CGFloat(lines.count) * lineHeight + CGFloat(max(lines.count - 1, 0)) * lineSpacing)
        if !streaming {
            if cache.count > 2_000 { cache.removeAll(keepingCapacity: true) }
            cache[key] = (text, size)
        }
        return (text, size, key)
    }
}
