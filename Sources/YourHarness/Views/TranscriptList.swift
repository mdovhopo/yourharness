import AppKit
import HarnessCore
import SwiftUI

/// A thread's transcript as a list of its own, in AppKit, each row's content a small SwiftUI view
/// in a hosting view of its own.
///
/// It replaces a `LazyVStack` in a `ScrollView`, which was at the heart of the worst of what the
/// app did under load. Its row heights are estimates until measured and it moves your place to
/// suit them, which changes what it measures next, so it could go back and forth for good; with
/// hovering rows and scroll position requests feeding it, inside SwiftUI's update, frozen, until
/// AppKit ended the app. It also put every row's change through one graph with the window.
///
/// Here every step is finite and in plain sight: a row is measured when it comes near the screen,
/// once per width and content; your place is the row at the top of the screen, and the position
/// follows that row when heights above it change. Scrolling moves a layer, with nothing in
/// SwiftUI running, and a row's update touches that row.
@MainActor
final class TranscriptListView: NSView {
    /// What the controls over the transcript go by.
    let place = TranscriptPlace()

    private let thread: ThreadModel
    private let context: TranscriptRowContext
    private let scrollView = TranscriptScrollView()
    private let document = TranscriptDocumentView()

    private struct Slot {
        var row: TranscriptRow
        var height: CGFloat
        var y: CGFloat = 0
    }
    private var slots: [Slot] = []
    private var indexes: [String: Int] = [:]
    private var hosts: [String: TranscriptRowHost] = [:]
    private var rowsAreStale = true
    private var isUpdating = false
    /// Keeps the newest output in view as the transcript grows: while history loads, while a
    /// reply streams. Your own scrolling turns it off; coming back to the end turns it on.
    private var followsEnd = true
    /// Where a jump is headed: held for one update, while the rows around it are measured.
    private var target: (id: String, edge: Edge)?
    /// Waiting for more turns to come in.
    private var pendingJump: MessageJump.Direction?
    private var firstID: String?
    private var lastEntryID: String?
    private var placeWasRestored = false
    private var moreToMount = false
    private var hasShownRows = false

    private var wantsTyping = false
    private var typingIsDue = false
    private var typingWait: Task<Void, Never>?

    private var contentHeight: CGFloat = 0
    /// Rows shorter than the screen sit at its bottom.
    private var shortfall: CGFloat = 0
    var bottomInset: CGFloat = 0 {
        didSet { if bottomInset != oldValue { needsLayout = true } }
    }

    private static let topPadding: CGFloat = 24, bottomPadding: CGFloat = 24, rowSpacing: CGFloat = 18
    /// One screenful either side is kept made; past three it's let go.
    private static let unmountAfterScreens: CGFloat = 3

    override var isFlipped: Bool { true }

    init(thread: ThreadModel, context: TranscriptRowContext) {
        self.thread = thread
        self.context = context
        super.init(frame: .zero)
        context.list = self
        TranscriptWheelRouter.start()
        scrollView.drawsBackground = false
        scrollView.hasVerticalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.automaticallyAdjustsContentInsets = false
        scrollView.contentView.drawsBackground = false
        scrollView.contentView.postsBoundsChangedNotifications = true
        scrollView.documentView = document
        document.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scrollView)
        scrollView.onWillScrollUp = { [weak self] in self?.setFollowing(false) }
        scrollView.onDidScroll = { [weak self] up in self?.userScrolled(up: up) }
        NotificationCenter.default.addObserver(self, selector: #selector(boundsChanged), name: NSView.boundsDidChangeNotification, object: scrollView.contentView)
        NotificationCenter.default.addObserver(self, selector: #selector(scrollerMoved), name: NSScrollView.didLiveScrollNotification, object: scrollView)
    }

    required init?(coder: NSCoder) { fatalError() }

    private var clip: NSClipView { scrollView.contentView }
    private var topInset: CGFloat { scrollView.contentInsets.top }

    /// Parked, it does nothing, and catches up when it's back.
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard window != nil else { return }
        rowsAreStale = true
        needsLayout = true
        // Back after a while, what's on screen may all be new, and it should show whole.
        hasShownRows = false
        ScrollProbe.runIfRequested(on: self)
    }

    // Resized, it has no constraints to tell AppKit to ask.
    override func setFrameSize(_ newSize: NSSize) {
        let changed = newSize != frame.size
        super.setFrameSize(newSize)
        guard changed else { return }
        scrollView.frame = bounds
        needsLayout = true
    }

    override func layout() {
        super.layout()
        update()
    }

    // MARK: - Updating

    @objc private func boundsChanged() {
        guard !isUpdating else { return }
        update()
    }

    private func rowsChanged() {
        rowsAreStale = true
        needsLayout = true
    }

    /// Everything goes through here: rows that changed, new sizes, scrolling. It settles by
    /// itself: each round can only measure rows not measured yet.
    private func update() {
        guard !isUpdating, bounds.width > 0, window != nil else { return }
        isUpdating = true
        defer { isUpdating = false }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }

        let anchor = rowAtTop()
        let insets = NSEdgeInsets(top: safeAreaInsets.top, left: 0, bottom: bottomInset, right: 0)
        if scrollView.contentInsets.top != insets.top || scrollView.contentInsets.bottom != insets.bottom {
            scrollView.contentInsets = insets
        }
        if rowsAreStale { takeRows() }

        var rounds = 0
        var changed = true
        while changed, rounds < 16 {
            arrange()
            scroll(keeping: anchor)
            changed = mountRowsInView()
            rounds += 1
        }
        target = nil
        giveRowsInViewTheirWidth()
        placeRows()
        letGoOfFarRows()
        report()
        if moreToMount {
            moreToMount = false
            DispatchQueue.main.async { [weak self] in self?.needsLayout = true }
        }
    }

    private func takeRows() {
        rowsAreStale = false
        // Back where you were, the first time there are rows to go back to.
        let restoring = !placeWasRestored && !thread.hasNoEntries
        if restoring, let anchor = thread.scrollAnchor { thread.showTurns(including: anchor) }

        let box = WeakList(list: self)
        let (rows, typing, first, last) = withObservationTracking {
            (thread.transcriptDisplayRows,
             // Claude is working and nothing is streaming in.
             thread.phase == .running && thread.pendingPermissions.isEmpty && !(thread.entries.last?.isStreaming ?? false),
             thread.entries.first?.id,
             thread.entries.last)
        } onChange: {
            MainActor.assumeIsolated { box.list?.rowsChanged() }
        }

        if restoring {
            placeWasRestored = true
            if let anchor = thread.scrollAnchor, rows.contains(where: { $0.id == anchor }) {
                followsEnd = false
                target = (anchor, .bottom)
            }
        }
        // Sending a message always takes you to it.
        if let last, last.id != lastEntryID {
            if lastEntryID != nil, last.kind == .user { followsEnd = true }
            lastEntryID = last.id
        }
        noteTyping(typing, afterMessage: last?.kind == .user)

        var shown = rows
        if wantsTyping, typingIsDue { shown.append(.typing) }
        let firstChanged = first != firstID
        firstID = first

        var new: [Slot] = []
        new.reserveCapacity(shown.count)
        var newIndexes: [String: Int] = [:]
        newIndexes.reserveCapacity(shown.count)
        for row in shown {
            let id = row.id
            newIndexes[id] = new.count
            if let old = indexes[id] {
                var slot = slots[old]
                if slot.row != row || firstChanged {
                    slot.row = row
                    if let host = hosts[id] { host.rootView = root(row, width: host.rootView.width) }
                }
                new.append(slot)
            } else {
                new.append(Slot(row: row, height: Self.estimate(row, width: bounds.width)))
            }
        }
        for (id, host) in hosts where newIndexes[id] == nil {
            hosts[id] = nil
            host.removeFromSuperview()
        }
        slots = new
        indexes = newIndexes
        if let jump = pendingJump {
            pendingJump = nil
            self.jump(jump)
        }
    }

    /// Between two of its steps (text ending, a tool call starting) that's true for a moment
    /// only, so there it waits a little before showing: a row flashing in and out moves
    /// everything above it. Right after your message it shows at once.
    private func noteTyping(_ wanted: Bool, afterMessage: Bool) {
        guard wanted != wantsTyping else { return }
        wantsTyping = wanted
        typingWait?.cancel()
        typingIsDue = wanted && afterMessage
        guard wanted, !afterMessage else { return }
        typingWait = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled, let self else { return }
            self.typingIsDue = true
            self.rowsChanged()
        }
    }

    private func arrange() {
        var y = Self.topPadding
        for index in slots.indices {
            slots[index].y = y
            y += slots[index].height
            if slots[index].height > 0 { y += gap(after: index) }
        }
        let total = (slots.isEmpty ? 0 : y) + Self.bottomPadding
        let viewport = viewportHeight
        contentHeight = total
        shortfall = max(viewport - total, 0)
        let height = max(total, viewport)
        if document.frame.size != NSSize(width: bounds.width, height: height) {
            document.frame = NSRect(x: 0, y: 0, width: bounds.width, height: height)
        }
    }

    /// A turn's opened work reads as one group.
    private func gap(after index: Int) -> CGFloat {
        guard index + 1 < slots.count else { return 0 }
        if case .workStep = slots[index + 1].row { return 0 }
        return Self.rowSpacing
    }

    private var viewportHeight: CGFloat {
        clip.bounds.height - scrollView.contentInsets.top - scrollView.contentInsets.bottom
    }

    private var endY: CGFloat {
        max(-topInset, document.frame.height + scrollView.contentInsets.bottom - clip.bounds.height)
    }

    /// The row at the top of the screen, and how far its top is from there.
    private func rowAtTop() -> (id: String, index: Int, offset: CGFloat)? {
        guard !slots.isEmpty else { return nil }
        let top = clip.bounds.origin.y + topInset
        let index = firstRow(endingBelow: top)
        return (slots[index].row.id, index, slots[index].y + shortfall - top)
    }

    /// By bisection: the first row whose bottom is below `y`.
    private func firstRow(endingBelow y: CGFloat) -> Int {
        var low = 0, high = slots.count - 1
        while low < high {
            let middle = (low + high) / 2
            if slots[middle].y + shortfall + slots[middle].height <= y { low = middle + 1 } else { high = middle }
        }
        return low
    }

    private func scroll(keeping anchor: (id: String, index: Int, offset: CGFloat)?) {
        var y = clip.bounds.origin.y
        if let target, let index = indexes[target.id] {
            let row = slots[index]
            switch target.edge {
            case .top: y = row.y + shortfall - topInset - 12
            case .bottom: y = row.y + shortfall + row.height - clip.bounds.height + scrollView.contentInsets.bottom
            case .leading, .trailing: y = row.y + shortfall - topInset - (viewportHeight - row.height) / 2
            }
        } else if followsEnd {
            y = endY
        } else if let anchor {
            // Gone (folded away): the row now where it was.
            let index = indexes[anchor.id] ?? min(anchor.index, slots.count - 1)
            if slots.indices.contains(index) { y = slots[index].y + shortfall - anchor.offset - topInset }
        }
        y = min(max(y, -topInset), endY)
        guard abs(y - clip.bounds.origin.y) > 0.25 else { return }
        clip.setBoundsOrigin(NSPoint(x: 0, y: y))
        scrollView.reflectScrolledClipView(clip)
    }

    /// What's on screen is made now, however long it takes; what's a screenful away a little at a
    /// time, so a click doesn't wait for rows you can't see. True if a row wasn't the height
    /// it was thought to be.
    private func mountRowsInView() -> Bool {
        guard !slots.isEmpty else { return false }
        let top = clip.bounds.origin.y, bottom = top + clip.bounds.height, screen = clip.bounds.height
        var changed = false
        let started = ContinuousClock.now
        // The first screenful in one go: a thread shows whole. After that, rows flying past don't
        // hold the frame up: it goes out, and the rest are made after it.
        let allowed: Duration = hasShownRows ? .milliseconds(24) : .milliseconds(200)
        // From the end when that's where you are, so that what's at the end shows true first.
        let inView = firstRow(endingBelow: top)...max(firstRow(endingBelow: top), lastRow(startingAbove: bottom))
        for index in followsEnd ? Array(inView.reversed()) : Array(inView) where hosts[slots[index].row.id] == nil {
            if ContinuousClock.now - started > allowed {
                moreToMount = true
                return changed
            }
            if mount(index) { changed = true }
        }
        hasShownRows = true
        if changed { return true }
        let near = firstRow(endingBelow: top - screen)...max(firstRow(endingBelow: top - screen), lastRow(startingAbove: bottom + screen))
        for index in near.reversed() where hosts[slots[index].row.id] == nil {
            if ContinuousClock.now - started > .milliseconds(4) {
                moreToMount = true
                break
            }
            if mount(index) { changed = true }
        }
        return changed
    }

    private func lastRow(startingAbove y: CGFloat) -> Int {
        var low = 0, high = slots.count - 1
        while low < high {
            let middle = (low + high + 1) / 2
            if slots[middle].y + shortfall < y { low = middle } else { high = middle - 1 }
        }
        return low
    }

    /// For test runs.
    static var rowsMade = 0
    static var secondsMaking: CFTimeInterval = 0

    /// True if it wasn't the height it was thought to be.
    private func mount(_ index: Int) -> Bool {
        let started = CACurrentMediaTime()
        defer {
            Self.rowsMade += 1
            Self.secondsMaking += CACurrentMediaTime() - started
        }
        let row = slots[index].row
        let host = TranscriptRowHost(rootView: root(row, width: bounds.width))
        // Where it is on the screen is nothing to it: the list keeps rows clear of what's over them.
        // Otherwise, scrolled under the toolbar, a row would be pushed down inside itself.
        host.safeAreaRegions = []
        host.sizingOptions = [.intrinsicContentSize]
        hosts[row.id] = host
        let height = ceil(host.intrinsicContentSize.height)
        host.sizingOptions = []
        host.frame = NSRect(x: 0, y: slots[index].y + shortfall, width: bounds.width, height: height)
        // Frames only: nothing here for the window's constraint engine to work out.
        host.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(host)
        guard abs(height - slots[index].height) > 0.5 else { return false }
        slots[index].height = height
        return true
    }

    private func root(_ row: TranscriptRow, width: CGFloat) -> TranscriptRowRoot {
        TranscriptRowRoot(row: row, isFirst: row.id == firstID, width: width, thread: thread, context: context)
    }

    /// The list's width changed: rows on screen go with it, each saying its new height when it's
    /// laid out. The rest wait until it has settled: dragging the window's edge, rewrapping
    /// every row made, off screen too, took a quarter of a second a step.
    private func giveRowsInViewTheirWidth() {
        let width = bounds.width
        var behind = false
        for (id, host) in hosts where host.rootView.width != width {
            guard let index = indexes[id] else { continue }
            let minY = slots[index].y + shortfall
            if minY < clip.bounds.maxY, minY + slots[index].height > clip.bounds.minY {
                host.rootView = root(slots[index].row, width: width)
            } else {
                behind = true
            }
        }
        widthSettling?.cancel()
        widthSettling = nil
        guard behind else { return }
        widthSettling = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled, let self else { return }
            self.widthSettling = nil
            self.giveEveryRowItsWidth()
        }
    }

    private func giveEveryRowItsWidth() {
        for (id, host) in hosts where host.rootView.width != bounds.width {
            guard let index = indexes[id] else { continue }
            host.rootView = root(slots[index].row, width: bounds.width)
        }
        needsLayout = true
    }
    private var widthSettling: Task<Void, Never>?

    /// A first guess, before a row comes near the screen. It only sets how long the transcript
    /// looks to the scroll bar: your place is held by a row.
    private static func estimate(_ row: TranscriptRow, width: CGFloat) -> CGFloat {
        let charactersPerLine = max(min(width - 56, 820) / 7.2, 20)
        func lines(_ text: String) -> CGFloat {
            var count: CGFloat = 1
            var run = 0
            for byte in text.utf8.prefix(20_000) {
                run += 1
                if byte == 10 || CGFloat(run) > charactersPerLine {
                    count += 1
                    run = 0
                }
            }
            return count
        }
        switch row {
        case .entry(let entry), .workStep(let entry, _):
            switch entry.kind {
            case .user: return lines(entry.text) * 20 + 46
            case .assistant: return lines(entry.text) * 23
            case .tool: return 40
            case .thinking: return 18
            case .notice: return 18
            case .commandOutput: return lines(entry.text) * 20 + 40
            }
        case .work: return 36
        case .earlier: return 30
        case .typing: return 18
        }
    }

    private func placeRows() {
        for (id, host) in hosts {
            guard let index = indexes[id] else { continue }
            let frame = NSRect(x: 0, y: slots[index].y + shortfall, width: host.rootView.width, height: slots[index].height)
            if host.frame != frame { host.frame = frame }
        }
    }

    /// A few at a time: taking one down takes a moment.
    private func letGoOfFarRows() {
        let reach = clip.bounds.height * Self.unmountAfterScreens
        let top = clip.bounds.origin.y - reach, bottom = clip.bounds.maxY + reach
        var gone = 0
        for (id, host) in hosts where gone < 4 {
            guard let index = indexes[id] else { continue }
            let y = slots[index].y + shortfall
            guard y + slots[index].height < top || y > bottom else { continue }
            // You've selected text in it, or it's where you're typing.
            if let responder = window?.firstResponder as? NSView, responder.isDescendant(of: host) { continue }
            hosts[id] = nil
            host.removeFromSuperview()
            gone += 1
        }
    }

    private func report() {
        let y = clip.bounds.origin.y
        let height = clip.bounds.height
        let atBottom = endY - y <= 40
        let atTop = y + topInset <= 40
        let short = contentHeight <= viewportHeight + 0.5
        var above = false, earlier = false
        if let first = slots.first {
            if case .earlier = first.row { earlier = true }
            let top = firstRow(endingBelow: y + topInset)
            above = earlier || slots[..<top].contains { $0.row.entry?.kind == .user }
        }
        if place.isAtBottom != atBottom { place.isAtBottom = atBottom }
        if place.isAtTop != atTop { place.isAtTop = atTop }
        if place.isShort != short { place.isShort = short }
        if place.hasMessageAbove != above { place.hasMessageAbove = above }
        if place.hasEarlierTurns != earlier { place.hasEarlierTurns = earlier }
        thread.isReadingBack = !followsEnd

        var visible: [String] = [], touched: [String] = []
        if !slots.isEmpty {
            let top = y + topInset, bottom = y + height - scrollView.contentInsets.bottom
            for index in firstRow(endingBelow: top)..<slots.count {
                let slot = slots[index], minY = slot.y + shortfall, maxY = minY + slot.height
                if minY >= bottom { break }
                touched.append(slot.row.id)
                let seen = min(maxY, bottom) - max(minY, top)
                if slot.height > 0, seen / slot.height >= 0.3 { visible.append(slot.row.id) }
            }
        }
        var diagnostics = TranscriptDiagnostics.Scroll()
        diagnostics.offset = y
        diagnostics.contentHeight = document.frame.height + topInset + scrollView.contentInsets.bottom
        diagnostics.containerHeight = height
        diagnostics.insetTop = topInset
        diagnostics.insetBottom = scrollView.contentInsets.bottom
        diagnostics.visibleRows = visible
        diagnostics.touchedRows = touched
        diagnostics.followsBottom = followsEnd
        diagnostics.isShort = short
        TranscriptDiagnostics.scroll[thread.id] = diagnostics
        if SwitchProbe.isOn {
            SwitchProbe.geometry[thread.id] = (y, diagnostics.contentHeight, height)
            SwitchProbe.visibleRows[thread.id] = touched.count
        }
        if ScrollProbe.isOn {
            ScrollProbe.positions = Dictionary(uniqueKeysWithValues: touched.compactMap { id in indexes[id].map { (id, slots[$0].y + shortfall - y) } })
        }
    }

    // MARK: - You, scrolling

    /// Up and away from the end stops following; coming back to it starts again. A nudge that
    /// didn't get anywhere (bouncing back to the end) doesn't count, which shows once it has
    /// settled: a wheel event moves the list a moment after it's handled.
    private func userScrolled(up: Bool) {
        settling?.cancel()
        guard up else {
            if endY - clip.bounds.origin.y <= 30 { setFollowing(true) }
            return
        }
        settling = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled, let self, self.endY - self.clip.bounds.origin.y <= 4 else { return }
            self.setFollowing(true)
        }
    }
    private var settling: Task<Void, Never>?

    /// The scroll bar, dragged: no wheel events for that.
    @objc private func scrollerMoved() {
        guard NSEvent.pressedMouseButtons != 0 else { return }
        let gap = endY - clip.bounds.origin.y
        if gap > 40 { setFollowing(false) } else if gap <= 30 { setFollowing(true) }
    }

    private func setFollowing(_ following: Bool) {
        guard followsEnd != following else { return }
        if FollowLog.isOn { FollowLog.write("following \(following ? "on" : "off")") }
        followsEnd = following
        thread.isReadingBack = !following
    }

    // MARK: - Going places

    /// To your previous message above the top of the screen, or your next one below it
    /// (past the last, to the end).
    func jump(_ direction: MessageJump.Direction) {
        guard !slots.isEmpty else { return }
        arrange()
        let top = firstRow(endingBelow: clip.bounds.origin.y + topInset)
        let isMessage: (Slot) -> Bool = { $0.row.entry?.kind == .user }
        switch direction {
        case .previous:
            setFollowing(false)
            // Past the turns shown: show more, then go on from there.
            if !slots[..<top].contains(where: isMessage), case .earlier = slots[0].row {
                pendingJump = .previous
                thread.showEarlierTurns()
                return
            }
            go(to: (slots[..<top].last(where: isMessage) ?? slots[0]).row.id, edge: .top, animated: true)
        case .next:
            if top + 1 < slots.count, let next = slots[(top + 1)...].first(where: isMessage) {
                setFollowing(false)
                go(to: next.row.id, edge: .top, animated: true)
            } else {
                setFollowing(true)
                needsLayout = true
            }
        case .top:
            // A long way: straight there, as ⌘↑ goes to the top of a document. Scrolled
            // through, the rows in between would each be made on the way.
            setFollowing(false)
            if case .earlier = slots[0].row {
                pendingJump = .top
                thread.showAllTurns()
                return
            }
            go(to: slots[0].row.id, edge: .top, animated: false)
        case .bottom:
            setFollowing(true)
            needsLayout = true
        }
    }

    /// To a row that may not be among the rows yet: a match in a folded turn, opened just now.
    func show(rowID: String) {
        setFollowing(false)
        target = (rowID, .leading)
        needsLayout = true
    }

    private func go(to id: String, edge: Edge, animated: Bool) {
        let start = clip.bounds.origin.y
        target = (id, edge)
        update()
        let end = clip.bounds.origin.y
        // Rows around it are made by now, and if it's near the rows between are too.
        guard animated, !isUpdating, abs(end - start) > 1, abs(end - start) < clip.bounds.height * 2 else { return }
        isUpdating = true
        clip.setBoundsOrigin(NSPoint(x: 0, y: start))
        isUpdating = false
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.25
            context.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
            clip.animator().setBoundsOrigin(NSPoint(x: 0, y: end))
        }
    }

    /// For test runs: rows whose height in the list isn't what SwiftUI makes of them now.
    func rowsOffTheirHeight() -> [String] {
        guard window != nil, bounds.width > 0 else { return [] }
        return hosts.compactMap { id, host in
            guard let index = indexes[id] else { return "\(id): made, not in the list" }
            if abs(host.frame.width - bounds.width) > 0.5, widthSettling == nil { return "\(id.prefix(8)): \(Int(host.frame.width)) wide in a list of \(Int(bounds.width))" }
            host.sizingOptions = [.intrinsicContentSize]
            let real = ceil(host.intrinsicContentSize.height)
            host.sizingOptions = []
            return abs(real - slots[index].height) > 1.5 || abs(host.frame.height - slots[index].height) > 1.5
                ? "\(id.prefix(8)): list \(Int(slots[index].height)), frame \(Int(host.frame.height)), real \(Int(real))" : nil
        }
    }

    /// For the scroll probe: as a scroll by that much would.
    func scrollForProbe(by points: CGFloat) {
        clip.setBoundsOrigin(NSPoint(x: 0, y: min(max(clip.bounds.origin.y - points, -topInset), endY)))
        scrollView.reflectScrolledClipView(clip)
    }

    func goToTopForProbe() {
        setFollowing(false)
        target = (slots.first?.row.id ?? "", .top)
        update()
    }

    var probeOffset: CGFloat { clip.bounds.origin.y }
    var probeViewport: CGFloat { clip.bounds.height }
    var probeTopInset: CGFloat { topInset }

    /// Where you are, for a transcript made anew later (this one let go of).
    func rememberPlace() {
        guard !followsEnd, !place.isAtBottom else {
            thread.scrollAnchor = nil
            // Left at the end: next time it opens with just the latest turns again.
            thread.shownTurns = ThreadModel.turnsPerPage
            return
        }
        let bottom = clip.bounds.maxY - scrollView.contentInsets.bottom
        thread.scrollAnchor = slots.isEmpty ? nil : slots[lastRow(startingAbove: bottom)].row.id
    }

    fileprivate func row(_ id: String, measured height: CGFloat) {
        guard hosts[id] != nil, let index = indexes[id], abs(slots[index].height - ceil(height)) > 0.5 else { return }
        slots[index].height = ceil(height)
        needsLayout = true
    }

    private struct WeakList: @unchecked Sendable {
        weak var list: TranscriptListView?
    }
}

/// What the controls over the transcript go by.
@MainActor @Observable
final class TranscriptPlace {
    var isAtBottom = true
    var isAtTop = false
    var isShort = true
    /// One of your messages above the top of the screen, shown or not yet.
    var hasMessageAbove = false
    /// Turns not shown yet, above the first one that is.
    var hasEarlierTurns = false
}

/// What every row of a transcript gets from it.
@MainActor @Observable
final class TranscriptRowContext {
    @ObservationIgnored let app: AppModel
    @ObservationIgnored let openURL: OpenURLAction
    @ObservationIgnored weak var list: TranscriptListView?
    /// On screen, rather than parked.
    @ObservationIgnored var isShown = false
    var issueLinker = IssueLinker(repos: [], openPullRequests: [])
    var fileLinker = FileLinker.empty

    init(app: AppModel, thread: ThreadModel) {
        self.app = app
        // From the start: rows made without them would all be made again a moment later.
        issueLinker = app.issueLinker(for: thread)
        fileLinker = app.fileLinker(for: thread)
        // Links to this Mac (the app being built) open in the thread's browser; ⌘-click
        // for your default browser. Files open in the Files tab; ⌘-click in their app.
        openURL = OpenURLAction { [app, thread] url in
            if FileLinkAction.open(url, in: thread) { return .handled }
            guard app.browserTools, LocalServers.isLocal(url), !NSEvent.modifierFlags.contains(.command) else { return .systemAction }
            thread.openInBrowser(url)
            return .handled
        }
    }
}

/// A row, at the list's width, as tall as it needs.
struct TranscriptRowRoot: View {
    let row: TranscriptRow
    let isFirst: Bool
    let width: CGFloat
    let thread: ThreadModel
    let context: TranscriptRowContext

    var body: some View {
        TranscriptRowView(row: row, thread: thread, isFirst: isFirst)
            .frame(maxWidth: 820, alignment: .leading)
            .frame(maxWidth: .infinity)
            .padding(.horizontal, 28)
            .frame(width: width)
            .fixedSize(horizontal: false, vertical: true)
            // Its real height, from its layout, whatever made it change: text streaming in, a
            // tool call opened, the width.
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height in
                context.list?.row(row.id, measured: height)
            }
            // Before the list hears, what's grown hangs down rather than out both ways.
            .frame(maxHeight: .infinity, alignment: .top)
            .environment(\.issueLinker, context.issueLinker)
            .environment(\.fileLinker, context.fileLinker)
            .environment(\.transcriptRows, context)
            .environment(\.openURL, context.openURL)
            .environment(thread)
            .environment(context.app)
    }
}

typealias TranscriptRowHost = NSHostingView<TranscriptRowRoot>

final class TranscriptDocumentView: NSView {
    override var isFlipped: Bool { true }
}

final class TranscriptScrollView: NSScrollView {
    /// Before a wheel or trackpad event moves it up, as the move itself would otherwise be
    /// answered by going back to the end; and after one has moved it.
    var onWillScrollUp: (() -> Void)?
    var onDidScroll: ((_ up: Bool) -> Void)?

    override func scrollWheel(with event: NSEvent) {
        let delta = event.scrollingDeltaY
        if delta > 0 { onWillScrollUp?() }
        super.scrollWheel(with: event)
        if delta != 0 { onDidScroll?(delta > 0) }
    }
}

/// The list, in the transcript's SwiftUI.
struct TranscriptListHost: NSViewRepresentable {
    let list: TranscriptListView

    func makeNSView(context: Context) -> TranscriptListView { list }
    func updateNSView(_ list: TranscriptListView, context: Context) {}

    func sizeThatFits(_ proposal: ProposedViewSize, nsView: TranscriptListView, context: Context) -> CGSize? {
        proposal.replacingUnspecifiedDimensions()
    }
}

/// Scrolling over something in a row that scrolls too. A code block goes sideways, and left to
/// itself it keeps every wheel event: with the pointer over one, the transcript stopped moving.
/// So at the start of each gesture: a row's scroll view gets it only if it can use it; if it
/// can't, the whole gesture goes to the list.
@MainActor
enum TranscriptWheelRouter {
    private static var monitor: Any?
    /// Where the gesture in progress is going, when that's the list.
    private static weak var gestureTarget: TranscriptScrollView?

    static func start() {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { event in
            nonisolated(unsafe) let event = event
            let goesOn = MainActor.assumeIsolated { handle(event, in: event.window) != nil }
            return goesOn ? event : nil
        }
    }

    /// The event if it should go on its way as usual, nil if it's been dealt with.
    static func handle(_ event: NSEvent, in window: NSWindow?) -> NSEvent? {
        let isMouseWheel = event.phase.isEmpty && event.momentumPhase.isEmpty
        if isMouseWheel || event.phase == .began {
            gestureTarget = target(of: event, in: window)
        } else if event.phase == .mayBegin {
            gestureTarget = nil
        }
        guard let gestureTarget else { return event }
        gestureTarget.scrollWheel(with: event)
        return nil
    }

    private static func target(of event: NSEvent, in window: NSWindow?) -> TranscriptScrollView? {
        var view = window?.contentView?.hitTest(event.locationInWindow)
        var inner: NSScrollView?
        while let current = view {
            if let list = current as? TranscriptScrollView {
                guard let inner else { return nil }
                return canUse(event, inner) ? nil : list
            }
            if inner == nil, let scroller = current as? NSScrollView { inner = scroller }
            view = current.superview
        }
        return nil
    }

    private static func canUse(_ event: NSEvent, _ scroller: NSScrollView) -> Bool {
        guard let document = scroller.documentView else { return false }
        let clip = scroller.contentView.bounds, size = document.frame.size
        if abs(event.scrollingDeltaX) > abs(event.scrollingDeltaY) { return size.width > clip.width + 1 }
        // Upward from the top, or past the end, is for the list.
        return event.scrollingDeltaY > 0 ? clip.minY > 1 : clip.maxY < size.height - 1
    }
}
