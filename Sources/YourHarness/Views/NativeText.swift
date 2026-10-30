import AppKit
import HarnessCore
import SwiftUI

/// A code block's text drawn by AppKit: selectable the way Mac text is, lines run on, and
/// its size worked out from its lines, so laying out a long block costs nothing.
struct NativeText: NSViewRepresentable {
    let text: NSAttributedString
    /// What the text is: the same key always means the same text.
    let key: NativeTextKey
    let size: CGSize
    @Environment(\.openURL) private var openURL

    func makeNSView(context: Context) -> TranscriptTextView {
        let view = TranscriptTextView(wraps: false)
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
        size
    }
}

/// Identifies a piece of transcript text, styling included.
struct NativeTextKey: Hashable {
    var source: String
    var style: String
    var find: String = ""
}

/// Read-only text that selects like any Mac text and hands links to the app.
class TranscriptTextView: NSTextView {
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

// MARK: - Prose

/// Markdown prose (headings, paragraphs, lists, quotes, tables, rules) as one AppKit text:
/// selection runs across all of it, tables included, and ⌘C copies what you see. While a
/// reply streams in, only the blocks that changed are built again and replaced, so the rest
/// keeps its layout, and your selection.
struct RichText: NSViewRepresentable {
    let blocks: [MarkdownBlock]
    /// The last block is still streaming in.
    let isTail: Bool
    @Environment(\.issueLinker) private var issues
    @Environment(\.fileLinker) private var files
    @Environment(\.findHighlight) private var find
    @Environment(\.openURL) private var openURL

    func makeNSView(context: Context) -> RichTextView {
        let view = RichTextView()
        updateNSView(view, context: context)
        return view
    }

    func updateNSView(_ view: RichTextView, context: Context) {
        view.open = { openURL($0) }
        view.show(pieces, linker: TextLinkers(issues: issues, files: files))
    }


    func sizeThatFits(_ proposal: ProposedViewSize, nsView: RichTextView, context: Context) -> CGSize? {
        // Asked for its ideal size, it's as wide as it is now.
        let current = nsView.bounds.width > 0 ? nsView.bounds.width : 600
        let proposed = proposal.width.flatMap { $0.isFinite ? $0 : nil } ?? current
        let width = max(proposed, 40)
        return CGSize(width: width, height: nsView.height(for: width))
    }

    private var pieces: [ProsePiece] {
        let linkerKey = "\(issues.repos.count)|\(issues.openPullRequests.count)|\(files.revision)"
        return blocks.indices.map { index in
            let isLast = index == blocks.count - 1
            return ProsePiece(block: blocks[index], isFirst: index == 0, isLast: isLast, isTail: isTail && isLast,
                              linker: linkerKey, find: find)
        }
    }
}

/// One block of prose as it's drawn: where it sits changes its spacing, and the streaming
/// tail closes what's still open.
struct ProsePiece: Hashable {
    let block: MarkdownBlock
    let isFirst: Bool
    let isLast: Bool
    let isTail: Bool
    let linker: String
    let find: FindHighlight?
}

final class RichTextView: TranscriptTextView {
    private var pieces: [ProsePiece] = []
    /// Each piece's length in the text, to find where a changed one starts.
    private var lengths: [Int] = []
    private var measured: (width: CGFloat, height: CGFloat)?
    private var tableButtons: [TableCopyButton] = []

    init() {
        super.init(wraps: true)
    }

    func show(_ new: [ProsePiece], linker: TextLinkers) {
        guard new != pieces, let storage = textStorage else { return }
        var kept = 0
        while kept < min(pieces.count, new.count), pieces[kept] == new[kept] { kept += 1 }
        let start = lengths.prefix(kept).reduce(0, +)
        let replacement = NSMutableAttributedString()
        var newLengths = Array(lengths.prefix(kept))
        for piece in new[kept...] {
            let text = ProseText.text(for: piece, linker: linker)
            replacement.append(text)
            newLengths.append(text.length)
        }
        storage.beginEditing()
        storage.replaceCharacters(in: NSRange(location: start, length: storage.length - start), with: replacement)
        storage.endEditing()
        pieces = new
        lengths = newLengths
        measured = nil
        hasTables = new.contains { if case .table = $0.block { true } else { false } }
        needsLayout = true
    }

    private var hasTables = false

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        if hasTables { needsLayout = true }
    }

    /// The text's height at a width, from this view's own layout: what's measured is what's drawn.
    func height(for width: CGFloat) -> CGFloat {
        let width = width.rounded(.down)
        if let measured, measured.width == width { return measured.height }
        let isFinal = !(pieces.last?.isTail ?? false)
        if isFinal, let known = ProseHeights.height(of: pieces, width: width) {
            measured = (width, known)
            return known
        }
        guard let container = textContainer, let layout = layoutManager, let storage = textStorage else { return 0 }
        let height: CGFloat
        if bounds.width == 0 || bounds.width.rounded(.down) == width {
            // The width it has, or is about to get: its own layout, which drawing then uses.
            container.size = CGSize(width: width, height: .greatestFiniteMagnitude)
            layout.ensureLayout(for: container)
            height = ceil(layout.usedRect(for: container).height)
        } else {
            // Another width (a probe, a resize on the way): measured aside, so the text
            // stays laid out at the width it's drawn at.
            return ProseMeasure.height(of: storage, width: width)
        }
        measured = (width, height)
        if isFinal { ProseHeights.store(height, of: pieces, width: width) }
        return height
    }

    // MARK: Drawing what text doesn't: table cards, quote bars, rules

    override func drawBackground(in rect: NSRect) {
        super.drawBackground(in: rect)
        guard let storage = textStorage, let layout = layoutManager, let container = textContainer, storage.length > 0 else { return }
        let all = NSRange(location: 0, length: storage.length)
        let origin = textContainerOrigin

        storage.enumerateAttribute(.proseQuote, in: all) { value, range, _ in
            guard value != nil else { return }
            let bounds = usedRect(of: range, layout: layout, container: container).offsetBy(dx: origin.x, dy: origin.y)
            guard bounds.intersects(rect) else { return }
            NSColor.controlAccentColor.withAlphaComponent(0.45).setFill()
            NSBezierPath(roundedRect: CGRect(x: origin.x, y: bounds.minY, width: 3, height: bounds.height), xRadius: 1.5, yRadius: 1.5).fill()
        }

        storage.enumerateAttribute(.proseRule, in: all) { value, range, _ in
            guard value != nil else { return }
            let bounds = usedRect(of: range, layout: layout, container: container).offsetBy(dx: origin.x, dy: origin.y)
            guard bounds.intersects(rect.insetBy(dx: 0, dy: -4)) else { return }
            NSColor.separatorColor.setFill()
            CGRect(x: origin.x, y: bounds.midY.rounded(.down), width: container.size.width, height: 1).fill()
        }

        for table in tables() {
            let card = table.bounds.offsetBy(dx: origin.x, dy: origin.y)
            guard card.intersects(rect) else { continue }
            let shape = NSBezierPath(roundedRect: card.insetBy(dx: 0.5, dy: 0.5), xRadius: Theme.cardRadius, yRadius: Theme.cardRadius)
            NSColor.quaternarySystemFill.setFill()
            shape.fill()
            NSGraphicsContext.saveGraphicsState()
            shape.addClip()
            NSColor.quaternarySystemFill.setFill()
            CGRect(x: card.minX, y: card.minY, width: card.width, height: table.rowTops[1, default: card.maxY - origin.y] + origin.y - card.minY).fill()
            NSColor.separatorColor.setFill()
            for (row, top) in table.rowTops where row > 0 {
                CGRect(x: card.minX, y: (top + origin.y).rounded(.down), width: card.width, height: 1).fill()
            }
            NSGraphicsContext.restoreGraphicsState()
            NSColor.separatorColor.withAlphaComponent(0.6).setStroke()
            shape.lineWidth = 1
            shape.stroke()
        }
    }

    private func usedRect(of range: NSRange, layout: NSLayoutManager, container: NSTextContainer) -> CGRect {
        let glyphs = layout.glyphRange(forCharacterRange: range, actualCharacterRange: nil)
        var bounds = CGRect.null
        layout.enumerateLineFragments(forGlyphRange: glyphs) { _, used, _, _, _ in bounds = bounds.union(used) }
        return bounds.isNull ? .zero : bounds
    }

    /// Where each table is, from its cells' laid-out blocks, in text container coordinates.
    private func tables() -> [TableFrame] {
        guard hasTables, let storage = textStorage, let layout = layoutManager else { return [] }
        var frames: [ObjectIdentifier: TableFrame] = [:]
        var order: [ObjectIdentifier] = []
        storage.enumerateAttribute(.proseTableCell, in: NSRange(location: 0, length: storage.length)) { value, range, _ in
            guard let cell = value as? TableCellMark else { return }
            let glyphs = layout.glyphRange(forCharacterRange: range, actualCharacterRange: nil)
            let rect = layout.boundsRect(for: cell.block, glyphRange: glyphs)
            guard !rect.isEmpty else { return }
            let id = ObjectIdentifier(cell.block.table)
            if frames[id] == nil { order.append(id) }
            var frame = frames[id] ?? TableFrame(table: cell.table, bounds: rect, rowTops: [:], range: range)
            frame.bounds = frame.bounds.union(rect)
            frame.rowTops[cell.row] = min(frame.rowTops[cell.row] ?? .greatestFiniteMagnitude, rect.minY)
            frame.range = NSUnionRange(frame.range, range)
            frames[id] = frame
        }
        return order.compactMap { frames[$0] }
    }

    private struct TableFrame {
        let table: MarkdownTable
        var bounds: CGRect
        var rowTops: [Int: CGFloat]
        var range: NSRange
    }

    // MARK: Table copy buttons

    override func layout() {
        super.layout()
        guard hasTables || !tableButtons.isEmpty else { return }
        if let container = textContainer { layoutManager?.ensureLayout(for: container) }
        let frames = tables()
        while tableButtons.count > frames.count { tableButtons.removeLast().removeFromSuperview() }
        while tableButtons.count < frames.count {
            let button = TableCopyButton()
            button.isHidden = true
            addSubview(button)
            tableButtons.append(button)
        }
        let origin = textContainerOrigin
        for (button, frame) in zip(tableButtons, frames) {
            let header = (frame.rowTops[1] ?? frame.bounds.maxY) - frame.bounds.minY
            button.frame = CGRect(x: frame.bounds.maxX + origin.x - 30, y: frame.bounds.minY + origin.y + (header - 24) / 2, width: 24, height: 24)
            button.table = frame.table
            button.rich = textStorage?.attributedSubstring(from: frame.range)
        }
        updateHover(at: window.map { convert($0.mouseLocationOutsideOfEventStream, from: nil) })
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if !trackingAreas.contains(where: { $0.owner === self && $0.options.contains(.inVisibleRect) && $0.options.contains(.mouseEnteredAndExited) }) {
            addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInActiveApp, .inVisibleRect], owner: self))
        }
    }

    override func mouseMoved(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        updateHover(at: point)
        if tableButtons.contains(where: { !$0.isHidden && $0.frame.contains(point) }) {
            NSCursor.arrow.set()
        } else {
            super.mouseMoved(with: event)
        }
    }

    override func mouseExited(with event: NSEvent) {
        super.mouseExited(with: event)
        updateHover(at: nil)
    }

    /// The copy button shows on the table under the pointer, as on code blocks.
    private func updateHover(at point: CGPoint?) {
        guard !tableButtons.isEmpty else { return }
        let origin = textContainerOrigin
        let hovered = point.flatMap { point in
            tables().firstIndex { $0.bounds.offsetBy(dx: origin.x, dy: origin.y).contains(point) }
        }
        for (index, button) in tableButtons.enumerated() {
            button.isHidden = index != hovered && !button.showsCopied
        }
    }

    // MARK: Copying

    /// What you see, as plain text: table cells separated by tabs, a row to a line, list
    /// markers followed by a space. Rich text (tables as tables) goes along as AppKit writes it.
    override func writeSelection(to pboard: NSPasteboard, types: [NSPasteboard.PasteboardType]) -> Bool {
        guard super.writeSelection(to: pboard, types: types) else { return false }
        // AppKit may ask by the old name.
        if types.contains(.string) || types.contains(NSPasteboard.PasteboardType("NSStringPboardType")), let storage = textStorage {
            let ranges = selectedRanges.map(\.rangeValue).filter { $0.length > 0 }
            pboard.setString(ranges.map { ProseText.plainText(storage, in: $0) }.joined(separator: "\n"), forType: .string)
        }
        return true
    }
}

/// Lays text out at a width other than the one it's shown at.
@MainActor
private enum ProseMeasure {
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

    static func height(of text: NSAttributedString, width: CGFloat) -> CGFloat {
        container.size = CGSize(width: width, height: .greatestFiniteMagnitude)
        storage.setAttributedString(text)
        layout.ensureLayout(for: container)
        let height = ceil(layout.usedRect(for: container).height)
        storage.setAttributedString(NSAttributedString())
        return height
    }
}

/// Heights of finished prose by width, so a row made again (scrolled back to, a thread
/// switched back to) is sized without laying its text out first.
@MainActor
private enum ProseHeights {
    private static var heights: [[ProsePiece]: [CGFloat: CGFloat]] = [:]

    static func height(of pieces: [ProsePiece], width: CGFloat) -> CGFloat? {
        heights[pieces]?[width]
    }

    static func store(_ height: CGFloat, of pieces: [ProsePiece], width: CGFloat) {
        if heights.count > 4_000 { heights.removeAll(keepingCapacity: true) }
        heights[pieces, default: [:]][width] = height
    }
}

/// Copies a table as markdown (and as tab-separated values and a rich table, for apps that
/// take those), and says so for a moment.
final class TableCopyButton: NSButton {
    var table: MarkdownTable?
    var rich: NSAttributedString?
    private(set) var showsCopied = false

    init() {
        super.init(frame: .zero)
        isBordered = false
        bezelStyle = .accessoryBarAction
        imagePosition = .imageOnly
        contentTintColor = .secondaryLabelColor
        symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 11, weight: .regular)
        setSymbol(copied: false)
        toolTip = "Copy table"
        target = self
        action = #selector(copyTable)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private func setSymbol(copied: Bool) {
        image = NSImage(systemSymbolName: copied ? "checkmark" : "doc.on.doc", accessibilityDescription: copied ? "Copied" : "Copy table")
    }

    @objc private func copyTable() {
        guard let table else { return }
        let board = NSPasteboard.general
        board.clearContents()
        board.setString(MarkdownTableText.markdown(table), forType: .string)
        board.setString(MarkdownTableText.tabSeparated(table), forType: .tabularText)
        if let rich, let rtf = rich.rtf(from: NSRange(location: 0, length: rich.length), documentAttributes: [:]) {
            board.setData(rtf, forType: .rtf)
        }
        showsCopied = true
        setSymbol(copied: true)
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(1.5))
            self?.showsCopied = false
            self?.setSymbol(copied: false)
        }
    }
}

/// A table back as text: markdown to paste where markdown goes, tab-separated for spreadsheets.
enum MarkdownTableText {
    static func markdown(_ table: MarkdownTable) -> String {
        let separator = table.alignments.map { alignment in
            switch alignment {
            case .leading: "---"
            case .center: ":---:"
            case .trailing: "---:"
            }
        }
        return ([table.header, separator] + table.rows)
            .map { "| " + $0.map { $0.replacingOccurrences(of: "|", with: "\\|") }.joined(separator: " | ") + " |" }
            .joined(separator: "\n")
    }

    static func tabSeparated(_ table: MarkdownTable) -> String {
        ([table.header] + table.rows).map { $0.joined(separator: "\t") }.joined(separator: "\n")
    }
}

extension NSAttributedString.Key {
    /// On a table cell's text: which cell, of which table.
    static let proseTableCell = NSAttributedString.Key("YHProseTableCell")
    /// On a quote's text: drawn with a bar beside it.
    static let proseQuote = NSAttributedString.Key("YHProseQuote")
    /// On a rule's placeholder: drawn as a line.
    static let proseRule = NSAttributedString.Key("YHProseRule")
    /// On the line of space under a table: left out when copied.
    static let proseSpacer = NSAttributedString.Key("YHProseSpacer")
    /// On the tab after a list marker: a space when copied.
    static let proseListTab = NSAttributedString.Key("YHProseListTab")
}

/// A table cell's place, carried in its text.
final class TableCellMark: NSObject {
    let block: NSTextTableBlock
    let table: MarkdownTable
    let row: Int
    let isLastInRow: Bool

    init(block: NSTextTableBlock, table: MarkdownTable, row: Int, isLastInRow: Bool) {
        self.block = block
        self.table = table
        self.row = row
        self.isLastInRow = isLastInRow
    }
}

// MARK: - Building the text

/// Markdown prose, a block at a time, as attributed strings in the transcript's fonts and colors.
@MainActor
enum ProseText {
    static let bodySize = MarkdownMetrics.body
    static let blockSpacing: CGFloat = 12
    private static var cache: [ProsePiece: NSAttributedString] = [:]

    /// A block's text, ending in the line break to the next one. Built once (tables each
    /// time they're shown): a streaming reply rebuilds only its tail.
    static func text(for piece: ProsePiece, linker: TextLinkers) -> NSAttributedString {
        if let known = cache[piece] { return known }
        let text = build(piece, linker: linker)
        // A table's text carries its AppKit table: two tables sharing one would lay out as one.
        let isTable = if case .table = piece.block { true } else { false }
        if !piece.isTail, !isTable {
            if cache.count > 3_000 { cache.removeAll(keepingCapacity: true) }
            cache[piece] = text
        }
        return text
    }

    private static func build(_ piece: ProsePiece, linker: TextLinkers) -> NSAttributedString {
        let result = NSMutableAttributedString()
        let after = piece.isLast ? 0 : blockSpacing
        let tail = piece.isTail
        switch piece.block {
        case .heading(let level, let text):
            let style = paragraph(spacingBefore: piece.isFirst ? 0 : (level <= 2 ? 8 : 2), after: piece.isLast ? 0 : blockSpacing - 4)
            result.append(inline(text, tail: tail, linker: linker, font: headingFont(level), paragraph: style))
        case .paragraph(let text):
            result.append(inline(text, tail: tail, linker: linker, font: .systemFont(ofSize: bodySize), paragraph: paragraph(after: after)))
        case .quote(let text):
            let quote = NSMutableAttributedString(attributedString: inline(text, tail: tail, linker: linker, font: .systemFont(ofSize: bodySize),
                                                                           paragraph: paragraph(after: after, indent: 15), color: .secondaryLabelColor))
            quote.addAttribute(.proseQuote, value: true, range: NSRange(location: 0, length: quote.length))
            result.append(quote)
        case .list(let items):
            for (number, item) in items.enumerated() {
                let lastItem = number == items.count - 1
                let indent = CGFloat(item.depth) * 20
                let style = paragraph(after: lastItem ? after : 6, indent: indent + 22, firstLine: indent, tab: indent + 22)
                let marker: String = switch (item.task, item.marker) {
                case (true?, _): "☑"
                case (false?, _): "☐"
                case (nil, .bullet): ["•", "◦", "▪︎"][item.depth % 3]
                case (nil, .number(let n)): "\(n)."
                }
                let markerAttributes: [NSAttributedString.Key: Any] = [
                    .font: NSFont.monospacedDigitSystemFont(ofSize: bodySize, weight: .regular),
                    .foregroundColor: NSColor.secondaryLabelColor, .paragraphStyle: style,
                ]
                let line = NSMutableAttributedString(string: marker, attributes: markerAttributes)
                var tab = markerAttributes
                tab[.proseListTab] = true
                line.append(NSAttributedString(string: "\t", attributes: tab))
                let done = item.task == true
                line.append(inline(item.text, tail: tail && lastItem, linker: linker, font: .systemFont(ofSize: bodySize), paragraph: style,
                                   color: done ? .secondaryLabelColor : .labelColor, strike: done))
                if !lastItem { line.append(NSAttributedString(string: "\n", attributes: [.paragraphStyle: style])) }
                result.append(line)
            }
        case .table(let table):
            result.append(self.table(table, linker: linker, after: after))
        case .rule:
            let style = paragraph(spacingBefore: piece.isFirst ? 0 : 4, after: piece.isLast ? 0 : blockSpacing + 4)
            result.append(NSAttributedString(string: " ", attributes: [.font: NSFont.systemFont(ofSize: 6), .paragraphStyle: style, .proseRule: true]))
        case .code:
            break
        }
        if let find = piece.find { highlight(find, in: result) }
        // The break to the next block, in this one's style: a line break takes its line's font.
        if !piece.isLast, result.length > 0 {
            var attributes = result.attributes(at: result.length - 1, effectiveRange: nil)
            attributes[.proseTableCell] = nil
            attributes[.proseRule] = nil
            attributes[.link] = nil
            attributes[.backgroundColor] = nil
            result.append(NSAttributedString(string: "\n", attributes: attributes))
        }
        return result
    }

    /// A native text table: its cells wrap and select as text. The card behind it is drawn by the view.
    private static func table(_ table: MarkdownTable, linker: TextLinkers, after: CGFloat) -> NSAttributedString {
        let result = NSMutableAttributedString()
        let textTable = NSTextTable()
        textTable.numberOfColumns = table.columnCount
        textTable.layoutAlgorithm = .automaticLayoutAlgorithm
        textTable.collapsesBorders = true
        textTable.hidesEmptyCells = false
        textTable.setContentWidth(100, type: .percentageValueType)
        let rows = [table.header] + table.rows
        let size = MarkdownMetrics.small
        for (row, cells) in rows.enumerated() {
            for column in 0..<table.columnCount {
                let block = NSTextTableBlock(table: textTable, startingRow: row, rowSpan: 1, startingColumn: column, columnSpan: 1)
                block.setWidth(12, type: .absoluteValueType, for: .padding, edge: .minX)
                // The header's last cell leaves room for the copy button.
                block.setWidth(row == 0 && column == table.columnCount - 1 ? 36 : 12, type: .absoluteValueType, for: .padding, edge: .maxX)
                block.setWidth(7, type: .absoluteValueType, for: .padding, edge: .minY)
                block.setWidth(7, type: .absoluteValueType, for: .padding, edge: .maxY)
                block.verticalAlignment = .topAlignment
                let style = NSMutableParagraphStyle()
                style.textBlocks = [block]
                style.lineSpacing = 2
                style.alignment = switch table.alignments[column] {
                case .leading: .natural
                case .center: .center
                case .trailing: .right
                }
                let font = NSFont.systemFont(ofSize: size, weight: row == 0 ? .semibold : .regular)
                let cell = NSMutableAttributedString(attributedString: inline(column < cells.count ? cells[column] : "", tail: false, linker: linker,
                                                                              font: font, paragraph: style))
                let isLastCell = row == rows.count - 1 && column == table.columnCount - 1
                // Every cell is a paragraph; the table's last one ends where the next block's break goes.
                if !isLastCell { cell.append(NSAttributedString(string: "\n", attributes: [.font: font, .paragraphStyle: style])) }
                if cell.length == 0 { cell.append(NSAttributedString(string: "\u{200B}", attributes: [.font: font, .paragraphStyle: style])) }
                let mark = TableCellMark(block: block, table: table, row: row, isLastInRow: column == table.columnCount - 1)
                cell.addAttribute(.proseTableCell, value: mark, range: NSRange(location: 0, length: cell.length))
                result.append(cell)
            }
        }
        // The space below: AppKit doesn't keep a table's margin or the next paragraph's
        // spacing, so a line of that height, which copying leaves out.
        if after > 0 {
            let style = NSMutableParagraphStyle()
            style.minimumLineHeight = after
            style.maximumLineHeight = after
            let last = result.attributes(at: result.length - 1, effectiveRange: nil)
            result.append(NSAttributedString(string: "\n", attributes: last))
            result.append(NSAttributedString(string: "\u{200B}", attributes: [.font: NSFont.systemFont(ofSize: 1), .paragraphStyle: style, .proseSpacer: true]))
        }
        return result
    }

    /// The text of a selection as you see it: cells separated by tabs, rules as `---`.
    static func plainText(_ text: NSAttributedString, in range: NSRange) -> String {
        let plain = text.string as NSString
        var result = ""
        text.enumerateAttributes(in: range) { attributes, run, _ in
            var piece = plain.substring(with: run).replacingOccurrences(of: "\u{2028}", with: "\n")
            if attributes[.proseSpacer] != nil {
                piece = ""
            } else if attributes[.proseRule] != nil {
                piece = "---"
            } else if attributes[.proseListTab] != nil {
                piece = " "
            } else if let cell = attributes[.proseTableCell] as? TableCellMark {
                piece = piece.replacingOccurrences(of: "\u{200B}", with: "")
                if !cell.isLastInRow, piece.hasSuffix("\n") { piece = String(piece.dropLast()) + "\t" }
            }
            result += piece
        }
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
    private static func inline(_ source: String, tail: Bool, linker: TextLinkers, font: NSFont, paragraph: NSParagraphStyle,
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
        // A line break inside a block keeps to its paragraph: its indent, its spacing.
        result.mutableString.replaceOccurrences(of: "\n", with: "\u{2028}", range: NSRange(location: 0, length: result.length))
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
