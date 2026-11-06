import AppKit
import HarnessCore
import ImageIO
import SwiftUI

/// A file as the viewer shows it, built off the main thread: its text highlighted and ready
/// to swap in whole, or a picture, or what it is when it's neither.
final class LoadedFile: @unchecked Sendable {
    /// Relative to the thread's folder.
    let path: String
    let snapshot: FileSnapshot
    /// Made on a background thread, then only touched on the main one once it's handed over.
    let storage: NSTextStorage?
    let image: CGImage?

    private init(path: String, snapshot: FileSnapshot, storage: NSTextStorage?, image: CGImage?) {
        self.path = path
        self.snapshot = snapshot
        self.storage = storage
        self.image = image
    }

    nonisolated(unsafe) static let font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)

    /// How much text it holds, for keeping recent files within a budget.
    nonisolated var textLength: Int { snapshot.text.utf8.count }

    /// Whether the file on disk is still what was read: call off the main thread.
    nonisolated func isCurrent(at url: URL) -> Bool {
        // A URL keeps what it was told: ask the disk.
        var url = url
        url.removeAllCachedResourceValues()
        let values = try? url.resourceValues(forKeys: [.fileSizeKey, .contentModificationDateKey])
        return values?.contentModificationDate == snapshot.modified && Int64(values?.fileSize ?? -1) == snapshot.size
    }

    /// Reads and builds it: call off the main thread.
    nonisolated static func load(_ url: URL, path: String) -> LoadedFile {
        let snapshot = FileReader.read(url)
        switch snapshot.kind {
        case .text:
            return LoadedFile(path: path, snapshot: snapshot, storage: text(snapshot), image: nil)
        case .image:
            return LoadedFile(path: path, snapshot: snapshot, storage: nil, image: image(url))
        default:
            return LoadedFile(path: path, snapshot: snapshot, storage: nil, image: nil)
        }
    }

    private nonisolated static func text(_ snapshot: FileSnapshot) -> NSTextStorage {
        let style = NSMutableParagraphStyle()
        style.lineSpacing = 1
        // Tabs four spaces wide, as editors default to.
        style.tabStops = []
        style.defaultTabInterval = 4 * ("M" as NSString).size(withAttributes: [.font: font]).width
        let storage = NSTextStorage(string: snapshot.text, attributes: [.font: font, .foregroundColor: NSColor.labelColor, .paragraphStyle: style])
        let highlights = !snapshot.language.isEmpty && snapshot.text.utf8.count <= FileReader.highlightLimit
            && snapshot.lineCount <= FileReader.highlightLineLimit
        guard highlights else { return storage }
        let text = snapshot.text
        storage.beginEditing()
        for token in SyntaxHighlighter.tokens(in: text, language: snapshot.language) {
            guard let color = SyntaxPalette.nsColor(token.kind) else { continue }
            storage.addAttribute(.foregroundColor, value: color, range: NSRange(token.range, in: text))
        }
        storage.endEditing()
        return storage
    }

    /// Big pictures come in at a size worth showing, not their full resolution.
    private nonisolated static func image(_ url: URL) -> CGImage? {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: 3_000,
        ]
        return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
    }
}

/// The files shown last, ready to swap back in: going between tabs doesn't wait for the disk.
/// Gone with the view, so with the thread; bounded, so a thread's tabs stay cheap.
final class RecentFiles {
    private var files: [LoadedFile] = []
    private let limit: Int
    private let textLimit: Int

    init(limit: Int = 4, textLimit: Int = 32 * 1024 * 1024) {
        self.limit = limit
        self.textLimit = textLimit
    }

    subscript(path: String) -> LoadedFile? { files.first { $0.path == path } }

    /// Newest first; what's past the limits goes, oldest first, but never the newest.
    func keep(_ file: LoadedFile) {
        files.removeAll { $0.path == file.path }
        files.insert(file, at: 0)
        while files.count > 1, files.count > limit || files.reduce(0, { $0 + $1.textLength }) > textLimit {
            files.removeLast()
        }
    }

    /// Closed tabs aren't worth keeping.
    func keepOnly(_ paths: Set<String>) {
        files.removeAll { !paths.contains($0.path) }
    }
}

/// The selected tab's file: loaded when it's picked, and again whenever it changes on disk
/// (Claude editing it while you read), keeping your place.
struct FileDocumentView: View {
    let files: FilesModel
    let path: String
    @AppStorage("filesWrapLines") private var wraps = false
    @State private var loaded: LoadedFile?
    @State private var recent = RecentFiles()
    /// Goes up each time the file changes on disk.
    @State private var version = 0

    private struct LoadKey: Equatable {
        let path: String
        let version: Int
    }

    var body: some View {
        Group {
            // A file seen lately shows in this very frame; another waits for its read, with the
            // last one still up rather than a blank.
            if let file = recent[path] ?? loaded {
                content(file)
            } else {
                Color.clear
            }
        }
        .task(id: LoadKey(path: path, version: version)) {
            let url = files.url(for: path)
            let path = path
            recent.keepOnly(Set(files.tabs.map(\.path)).union([path]))
            let known = recent[path]
            // Changed since it was read? (While it's on screen the watch below says so.)
            let isCurrent = await Task.detached(priority: .userInitiated) { known?.isCurrent(at: url) ?? false }.value
            guard !Task.isCancelled else { return }
            if let known, isCurrent {
                recent.keep(known)
                loaded = known
                FilesProbe.shown(known)
            } else {
                let file = await Task.detached(priority: .userInitiated) { LoadedFile.load(url, path: path) }.value
                guard !Task.isCancelled else { return }
                recent.keep(file)
                loaded = file
                FilesProbe.shown(file)
            }
            // Then wait for it to change, and load it again.
            guard await FileChangeWatch.next(url) else { return }
            try? await Task.sleep(for: .milliseconds(120))
            guard !Task.isCancelled else { return }
            version += 1
        }
    }

    @ViewBuilder
    private func content(_ file: LoadedFile) -> some View {
        let snapshot = file.snapshot
        switch snapshot.kind {
        case .text:
            VStack(spacing: 0) {
                if snapshot.hasCutLines {
                    Label("Lines over \(FileReader.lineLimit.formatted()) characters are cut", systemImage: "scissors")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 5)
                    Divider()
                }
                if snapshot.isTruncated {
                    Label("Showing the first \(ByteCountFormatter.string(fromByteCount: Int64(FileReader.textLimit), countStyle: .file)) of \(ByteCountFormatter.string(fromByteCount: snapshot.size, countStyle: .file))",
                          systemImage: "scissors")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 5)
                    Divider()
                }
                FileTextViewer(file: file, reveal: files.reveal, wraps: wraps, files: files)
            }
        case .image:
            if let image = file.image {
                ScrollView([.horizontal, .vertical]) {
                    Image(decorative: image, scale: 2)
                        .resizable()
                        .scaledToFit()
                        .frame(maxWidth: CGFloat(image.width) / 2, maxHeight: CGFloat(image.height) / 2)
                        .padding(12)
                }
            } else {
                unavailable("Can't Show This Picture", symbol: "photo", file: file)
            }
        case .binary:
            unavailable("Binary File", symbol: "doc.zipper", file: file)
        case .folder:
            unavailable("Folder", symbol: "folder", file: file)
        case .missing:
            ContentUnavailableView("File Not Found", systemImage: "questionmark.folder",
                                   description: Text("It may have been moved or deleted."))
        case .unreadable(let reason):
            ContentUnavailableView("Can't Read This File", systemImage: "exclamationmark.triangle", description: Text(reason))
        }
    }

    private func unavailable(_ title: String, symbol: String, file: LoadedFile) -> some View {
        ContentUnavailableView {
            Label(title, systemImage: symbol)
        } description: {
            Text(ByteCountFormatter.string(fromByteCount: file.snapshot.size, countStyle: .file))
        } actions: {
            Button("Open in Default App") { NSWorkspace.shared.open(files.url(for: file.path)) }
        }
    }
}

/// Waits for a file to change on disk. A save that replaces the file (most editors, and
/// Claude's writes) ends the watch too; the caller watches the new one after reading it.
enum FileChangeWatch {
    /// True once it changed; false if cancelled first.
    static func next(_ url: URL) async -> Bool {
        let watched = url.path
        let descriptor = open(watched, O_EVTONLY)
        // Not there (yet): watch its folder for it to appear.
        let target = descriptor >= 0 ? descriptor : open(url.deletingLastPathComponent().path, O_EVTONLY)
        guard target >= 0 else { return false }
        let source = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: target, eventMask: [.write, .extend, .delete, .rename, .attrib, .revoke], queue: .main)
        source.setCancelHandler { close(target) }
        let box = ResumeOnce()
        return await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                box.set(continuation)
                source.setEventHandler {
                    // A folder changes for every file in it: only its file turning up counts.
                    if descriptor < 0, !FileManager.default.fileExists(atPath: watched) { return }
                    box.resume(true)
                    source.cancel()
                }
                source.resume()
            }
        } onCancel: {
            box.resume(false)
            source.cancel()
        }
    }

    /// Resumes a continuation once, from whichever comes first: the change or the cancel.
    private final class ResumeOnce: @unchecked Sendable {
        private let lock = NSLock()
        private var continuation: CheckedContinuation<Bool, Never>?
        private var result: Bool?

        func set(_ continuation: CheckedContinuation<Bool, Never>) {
            lock.lock()
            if let result {
                lock.unlock()
                continuation.resume(returning: result)
                return
            }
            self.continuation = continuation
            lock.unlock()
        }

        func resume(_ value: Bool) {
            lock.lock()
            guard result == nil else { return lock.unlock() }
            result = value
            let continuation = self.continuation
            self.continuation = nil
            lock.unlock()
            continuation?.resume(returning: value)
        }
    }
}

// MARK: - Text

/// A file's text in AppKit's text system, laid out only where it's looked at: a file of a
/// hundred thousand lines opens as fast as a short one.
struct FileTextViewer: NSViewRepresentable {
    let file: LoadedFile
    let reveal: FilesModel.Reveal?
    let wraps: Bool
    let files: FilesModel

    func makeNSView(context: Context) -> FileScrollView {
        FileScrollView()
    }

    func updateNSView(_ view: FileScrollView, context: Context) {
        view.onLeave = { [files] path, origin in files.scrollPositions[path] = origin }
        view.setWraps(wraps)
        if view.shown !== file {
            view.show(file, restoring: files.scrollPositions[file.path])
        }
        if let reveal, reveal.path == file.path, reveal.serial != view.revealSerial {
            view.revealSerial = reveal.serial
            view.reveal(reveal)
        }
    }

    static func dismantleNSView(_ view: FileScrollView, coordinator: ()) {
        view.leave()
    }
}

/// The line numbers and the scrolling text beside them. The numbers are a view of their own
/// rather than a ruler, so they stay put when the text scrolls sideways.
final class FileScrollView: NSView {
    let textView: FileTextView
    let scrollView = NSScrollView()
    private let gutter: LineNumberGutter
    private(set) var shown: LoadedFile?
    var revealSerial = -1
    var onLeave: ((String, CGPoint) -> Void)?
    private var wraps = false
    /// The lines last revealed, kept marked through reloads and going between tabs.
    private var markedLines: (line: Int, endLine: Int)? {
        didSet { if let path = shown?.path { markedByPath[path] = markedLines.map { ClosedRange(uncheckedBounds: ($0.line, $0.endLine)) } } }
    }
    private var markedByPath: [String: ClosedRange<Int>] = [:]

    private var contentView: NSClipView { scrollView.contentView }

    init() {
        let storage = NSTextStorage()
        let layout = NSLayoutManager()
        // Only what's on screen is laid out; the rest is estimated until it's scrolled to.
        layout.allowsNonContiguousLayout = true
        storage.addLayoutManager(layout)
        let container = NSTextContainer(size: CGSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude))
        container.widthTracksTextView = false
        container.lineFragmentPadding = 6
        layout.addTextContainer(container)
        textView = FileTextView(frame: .zero, textContainer: container)
        gutter = LineNumberGutter(textView: textView)
        super.init(frame: .zero)

        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = false
        textView.importsGraphics = false
        textView.allowsUndo = false
        textView.drawsBackground = false
        textView.usesFindBar = true
        textView.isIncrementalSearchingEnabled = true
        textView.isAutomaticLinkDetectionEnabled = false
        textView.textContainerInset = NSSize(width: 2, height: 8)
        textView.minSize = .zero
        textView.maxSize = CGSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = true
        textView.autoresizingMask = [.width, .height]
        textView.setAccessibilityLabel("File contents")

        scrollView.drawsBackground = false
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.documentView = textView
        clipsToBounds = true
        addSubview(gutter)
        addSubview(scrollView)
        contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(self, selector: #selector(scrolled), name: NSView.boundsDidChangeNotification, object: contentView)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    override func layout() {
        super.layout()
        let width = gutter.thickness
        gutter.frame = CGRect(x: 0, y: 0, width: width, height: bounds.height)
        scrollView.frame = CGRect(x: width, y: 0, width: max(bounds.width - width, 0), height: bounds.height)
        if wraps { fitWrappedWidth() }
    }

    @objc private func scrolled() {
        gutter.needsDisplay = true
    }

    func setWraps(_ wraps: Bool) {
        guard wraps != self.wraps, let container = textView.textContainer else { return }
        self.wraps = wraps
        scrollView.hasHorizontalScroller = !wraps
        textView.isHorizontallyResizable = !wraps
        container.widthTracksTextView = wraps
        if wraps {
            fitWrappedWidth()
        } else {
            container.size = CGSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
        }
        gutter.needsDisplay = true
    }

    private func fitWrappedWidth() {
        let width = scrollView.contentSize.width
        guard width > 0, textView.frame.width != width else { return }
        textView.frame.size.width = width
        textView.textContainer?.size = CGSize(width: width - 2 * textView.textContainerInset.width, height: .greatestFiniteMagnitude)
    }

    /// Swaps the file's text in whole: one handover, no attribute work on this thread.
    func show(_ file: LoadedFile, restoring origin: CGPoint?) {
        let samePath = shown?.path == file.path
        let previous = contentView.bounds.origin
        if let shown, !samePath { onLeave?(shown.path, previous) }
        shown = file
        if !samePath { markedLines = markedByPath[file.path].map { ($0.lowerBound, $0.upperBound) } }
        guard let storage = file.storage, let layout = textView.layoutManager else { return }
        textView.setSelectedRange(NSRange(location: 0, length: 0))
        layout.replaceTextStorage(storage)
        gutter.lineStarts = file.snapshot.lineStarts
        if let markedLines {
            textView.marked = file.snapshot.range(ofLines: markedLines.line, through: markedLines.endLine)
            gutter.marked = markedLines.line...markedLines.endLine
        } else {
            textView.marked = nil
            gutter.marked = nil
        }
        needsLayout = true
        // A reload keeps your place; another file goes back to where you left it.
        scroll(to: samePath ? previous : (origin ?? .zero))
    }

    func reveal(_ reveal: FilesModel.Reveal) {
        guard let file = shown, file.storage != nil, let layout = textView.layoutManager, let container = textView.textContainer else { return }
        let snapshot = file.snapshot
        var range: NSRange?
        if let text = reveal.text, !text.isEmpty {
            let found = (textView.string as NSString).range(of: text)
            if found.location != NSNotFound {
                let first = snapshot.line(at: found.location)
                let last = snapshot.line(at: max(found.location, NSMaxRange(found) - 1))
                markedLines = (first, last)
                range = snapshot.range(ofLines: first, through: last)
            }
        }
        if range == nil, let line = reveal.line {
            let end = max(reveal.endLine ?? line, line)
            markedLines = (line, end)
            range = snapshot.range(ofLines: line, through: end)
        }
        guard let range else {
            markedLines = nil
            textView.marked = nil
            gutter.marked = nil
            return
        }
        textView.marked = range
        gutter.marked = markedLines.map { $0.line...$0.endLine }
        // With what's above it too: laid out later, as it scrolls into view, it would move the line.
        let above = snapshot.range(ofLines: max((markedLines?.line ?? 1) - 80, 1)).location
        layout.ensureLayout(forCharacterRange: NSRange(location: above, length: NSMaxRange(range) - above))
        let glyphs = layout.glyphRange(forCharacterRange: range, actualCharacterRange: nil)
        let rect = layout.boundingRect(forGlyphRange: glyphs, in: container).offsetBy(dx: 0, dy: textView.textContainerOrigin.y)
        // A third of the way down, so what leads up to it shows too.
        let visible = contentView.bounds.height
        scroll(to: CGPoint(x: 0, y: rect.height > visible * 0.66 ? rect.minY - 8 : rect.minY - visible * 0.3))
        textView.setSelectedRange(NSRange(location: range.location, length: 0))
    }

    func scroll(to point: CGPoint) {
        let maxY = max(0, textView.frame.height - contentView.bounds.height)
        contentView.scroll(to: CGPoint(x: max(0, point.x), y: min(max(0, point.y), maxY)))
        scrollView.reflectScrolledClipView(contentView)
        gutter.needsDisplay = true
    }

    /// Going away: where it was scrolled to, for coming back.
    func leave() {
        if let shown { onLeave?(shown.path, contentView.bounds.origin) }
        NotificationCenter.default.removeObserver(self)
    }
}

final class FileTextView: NSTextView {
    /// Lines a link pointed at, drawn behind the text across the whole width.
    var marked: NSRange? {
        didSet { if marked != oldValue { needsDisplay = true } }
    }

    // ⌘F finds in the file when it's the one you're in; the thread's own find otherwise.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if window?.firstResponder === self, event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command,
           event.charactersIgnoringModifiers == "f" {
            let item = NSMenuItem()
            item.tag = NSTextFinder.Action.showFindInterface.rawValue
            performTextFinderAction(item)
            return true
        }
        return super.performKeyEquivalent(with: event)
    }

    override func drawBackground(in rect: NSRect) {
        super.drawBackground(in: rect)
        guard let marked, let layout = layoutManager, let container = textContainer, NSMaxRange(marked) <= (textStorage?.length ?? 0) else { return }
        let glyphs = layout.glyphRange(forCharacterRange: marked, actualCharacterRange: nil)
        var bounds = layout.boundingRect(forGlyphRange: glyphs, in: container)
        bounds.origin.y += textContainerOrigin.y
        bounds.origin.x = 0
        bounds.size.width = self.bounds.width
        guard bounds.intersects(rect) else { return }
        NSColor.controlAccentColor.withAlphaComponent(0.13).setFill()
        bounds.fill()
    }
}

/// Line numbers beside the text, drawn only for the lines on screen.
final class LineNumberGutter: NSView {
    private weak var textView: NSTextView?
    var lineStarts: [Int] = [0] {
        didSet {
            let digits = max(3, String(lineStarts.count).count)
            let width = ceil(CGFloat(digits) * Self.advance + 12)
            if width != thickness {
                thickness = width
                superview?.needsLayout = true
            }
            needsDisplay = true
        }
    }
    private(set) var thickness = ceil(3 * LineNumberGutter.advance + 12)
    var marked: ClosedRange<Int>? {
        didSet { if marked != oldValue { needsDisplay = true } }
    }

    private static let font = NSFont.monospacedDigitSystemFont(ofSize: 10.5, weight: .regular)
    private static let advance = ("8" as NSString).size(withAttributes: [.font: font]).width
    private static let lineHeight = ("8" as NSString).size(withAttributes: [.font: font]).height

    init(textView: NSTextView) {
        self.textView = textView
        super.init(frame: .zero)
        clipsToBounds = true
        setAccessibilityElement(false)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        guard let textView, let layout = textView.layoutManager, let container = textView.textContainer,
              let length = textView.textStorage?.length else { return }
        let visible = textView.visibleRect
        let glyphs = layout.glyphRange(forBoundingRectWithoutAdditionalLayout: visible, in: container)
        let characters = layout.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
        let origin = textView.textContainerOrigin
        let normal: [NSAttributedString.Key: Any] = [.font: Self.font, .foregroundColor: NSColor.tertiaryLabelColor]
        let strong: [NSAttributedString.Key: Any] = [.font: Self.font, .foregroundColor: NSColor.controlAccentColor]
        var line = lineIndex(at: characters.location)
        let end = NSMaxRange(characters)
        while line < lineStarts.count, lineStarts[line] <= end {
            let start = lineStarts[line]
            let fragment: CGRect
            if length == 0 {
                fragment = layout.extraLineFragmentRect
            } else if start >= length {
                break
            } else {
                fragment = layout.lineFragmentRect(forGlyphAt: layout.glyphIndexForCharacter(at: start), effectiveRange: nil, withoutAdditionalLayout: true)
            }
            // The text's coordinates to this view's: both flipped, scrolled by the clip view.
            let y = fragment.minY + origin.y - visible.minY
            if y > bounds.maxY { break }
            if y + fragment.height >= dirtyRect.minY {
                // Digits are all one width: nothing to measure, on every step of a scroll.
                let number = String(line + 1) as NSString
                let attributes = marked?.contains(line + 1) == true ? strong : normal
                let width = CGFloat(number.length) * Self.advance
                number.draw(at: NSPoint(x: bounds.maxX - 2 - width, y: y + (fragment.height - Self.lineHeight) / 2), withAttributes: attributes)
            }
            line += 1
        }
    }

    /// The index of the line holding `offset`, by binary search.
    private func lineIndex(at offset: Int) -> Int {
        var low = 0, high = lineStarts.count - 1
        while low < high {
            let mid = (low + high + 1) / 2
            if lineStarts[mid] <= offset { low = mid } else { high = mid - 1 }
        }
        return low
    }
}
