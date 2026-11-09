// The transcript: a UICollectionView, not a SwiftUI list (on the Mac, a lazy list pinned to
// the bottom stuttered under streaming). Rows are measured once per copy (RowLayout), a page
// of earlier ones off the main thread, and placed by TranscriptLayout from those heights: no
// self-sizing cells, no Auto Layout. It follows the bottom while you're there and reads earlier
// pages well before you reach the top.

import SwiftUI
import UIKit

struct TranscriptScreen: View {
    let threadID: String
    @Environment(Store.self) private var store
    @State private var model: TranscriptModel?
    @State private var draft = ""
    @State private var composerHeight: CGFloat = 0
    private static let env = ProcessInfo.processInfo.environment

    var body: some View {
        Group {
            if let model {
                // Under the composer, so rows scroll beneath its glass; its height is the list's inset.
                TranscriptRepresentable(model: model, bottomInset: composerHeight).ignoresSafeArea(.container, edges: .bottom)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            ComposerBar(threadID: threadID, draft: $draft)
                .onGeometryChange(for: CGFloat.self, of: { $0.size.height }) { composerHeight = $0 }
        }
        .overlay(alignment: .top) {
            if let error = store.actionError {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .font(.footnote)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                    .glassEffect(in: .capsule)
                    .padding(.top, 6)
                    .onTapGesture { store.actionError = nil }
                    .task(id: error) {
                        try? await Task.sleep(for: .seconds(5))
                        if store.actionError == error { store.actionError = nil }
                    }
            }
        }
        // Test hooks, doing what the buttons do: `YH_PHONE_SEND=<text>` once the thread is open,
        // `YH_PHONE_ANSWER=allow|deny` to each request `YH_PHONE_ANSWER_DELAY` s (3) after it shows.
        .task(id: store.canAct) {
            guard store.canAct, let text = Self.env["YH_PHONE_SEND"] else { return }
            try? await Task.sleep(for: .seconds(2))
            Perf.shared.log("PHONE send: \(text)")
            store.send(text, to: threadID)
        }
        .task(id: store.live?.requests.first?.id) {
            guard let request = store.live?.requests.first, let answer = Self.env["YH_PHONE_ANSWER"] else { return }
            Perf.shared.log("PHONE card: \(request.tool) · \(request.description)")
            try? await Task.sleep(for: .seconds(Self.env["YH_PHONE_ANSWER_DELAY"].flatMap(Double.init) ?? 3))
            guard store.live?.requests.first?.id == request.id else { return }
            Perf.shared.log("PHONE \(answer) \(request.tool)")
            store.respond(request, allow: answer == "allow", in: threadID)
        }
        .navigationTitle(store.threads[threadID]?.title ?? "Thread")
        .navigationSubtitle(store.threads[threadID].flatMap(store.projectName) ?? "")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if store.threads[threadID]?.isRunning == true {
                ToolbarItem(placement: .topBarTrailing) {
                    Image(systemName: "circle.fill").font(.system(size: 8)).foregroundStyle(.green)
                        .symbolEffect(.pulse, options: .repeating)
                        .accessibilityLabel("Working")
                }
            }
        }
        .onAppear { model = store.open(threadID) }
        .onDisappear { if let model { store.close(model) } }
    }
}

struct TranscriptRepresentable: UIViewControllerRepresentable {
    let model: TranscriptModel
    var bottomInset: CGFloat = 0

    func makeUIViewController(context: Context) -> TranscriptViewController {
        TranscriptViewController(model: model)
    }

    func updateUIViewController(_ controller: TranscriptViewController, context: Context) {
        controller.setBottomInset(bottomInset)
    }
}

final class TranscriptViewController: UIViewController, UICollectionViewDataSource, UICollectionViewDelegate {
    private let model: TranscriptModel
    private var collectionView: UICollectionView!
    private let layout = TranscriptLayout()
    /// What's shown: consecutive `seq`s, each with its measured layout.
    private var layouts: [RowLayout] = []
    private var fonts = Fonts(.large)
    private var width: CGFloat = 0
    private var queue: [TranscriptModel.Update] = []
    private var busy = false
    private var followsBottom = true
    private var probe: ScrollProbe?
    fileprivate var view_: UICollectionView { collectionView }
    fileprivate var transcript: TranscriptModel { model }

    init(model: TranscriptModel) {
        self.model = model
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        fonts = Fonts(traitCollection.preferredContentSizeCategory)
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: layout)
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .systemBackground
        collectionView.dataSource = self
        collectionView.delegate = self
        collectionView.allowsSelection = false
        collectionView.keyboardDismissMode = .interactive
        collectionView.register(RowCell.self, forCellWithReuseIdentifier: "row")
        view.addSubview(collectionView)
        model.onUpdate = { [weak self] update in self?.enqueue(update) }
        if let seconds = ProcessInfo.processInfo.environment["YH_SCROLL_PROBE"].flatMap(Double.init) {
            probe = ScrollProbe(seconds: seconds, controller: self)
        }
        // Text size changed: every row measures again.
        registerForTraitChanges([UITraitPreferredContentSizeCategory.self]) { (self: Self, _: UITraitCollection) in
            self.fonts = Fonts(self.traitCollection.preferredContentSizeCategory)
            self.enqueue(.reload)
        }
    }

    /// The composer's height: rows end above it, and scroll beneath it.
    func setBottomInset(_ inset: CGFloat) {
        guard isViewLoaded, collectionView.contentInset.bottom != inset else { return }
        collectionView.contentInset.bottom = inset
        collectionView.verticalScrollIndicatorInsets.bottom = inset
        if followsBottom, !layouts.isEmpty { scrollToBottom() }
    }

    override func viewSafeAreaInsetsDidChange() {
        super.viewSafeAreaInsetsDidChange()
        if followsBottom, !layouts.isEmpty { scrollToBottom() }
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        let w = TranscriptLayout.contentWidth(for: collectionView.bounds.width)
        guard w > 0, w != width else { return }
        let first = width == 0
        width = w
        if first, !model.loaded { return }
        enqueue(.reload)
    }

    // MARK: Updates, in order; a page is measured in the background before it's shown.

    private func enqueue(_ update: TranscriptModel.Update) {
        if case .reload = update { queue.removeAll() }
        queue.append(update)
        process()
    }

    private func process() {
        guard !busy, width > 0, !queue.isEmpty else { return }
        let update = queue.removeFirst()
        switch update {
        case .reload:
            guard model.loaded else { return process() }
            measureInBackground(model.rows) { [weak self] measured in
                guard let self else { return }
                layouts = measured
                reloadShowing()
                followsBottom = true
                scrollToBottom()
                Perf.shared.log("SHOWN \(measured.count) rows")
            }
        case .prepended:
            guard let first = layouts.first?.seq else { return process() }
            let earlier = model.rows.prefix { $0.seq < first }
            measureInBackground(Array(earlier)) { [weak self] measured in
                guard let self, !measured.isEmpty, measured.last?.seq == (layouts.first?.seq ?? 0) - 1 else { return }
                let offset = collectionView.contentOffset.y
                let added = measured.reduce(0) { $0 + $1.height }
                layouts.insert(contentsOf: measured, at: 0)
                reloadShowing()
                collectionView.contentOffset.y = offset + added
            }
        case .changed(let appended, let changed):
            // Measured in the background as well: a long reply growing costs the main thread nothing.
            let rows = (changed + appended).compactMap { model.row($0) }
            measureInBackground(rows) { [weak self] measured in self?.applyChanges(measured) }
        }
    }

    private func measureInBackground(_ rows: [RowModel], then apply: @escaping @MainActor ([RowLayout]) -> Void) {
        busy = true
        let (width, fonts) = (width, fonts)
        Task.detached(priority: .userInitiated) {
            let measured = rows.map { RowLayout.make($0, width: width, fonts: fonts) }
            await MainActor.run { [weak self] in
                guard let self else { return }
                busy = false
                // Measured for a width or text size that's gone: the reload queued behind does it again.
                if width == self.width, fonts.category == self.fonts.category {
                    Perf.shared.measure(rows: measured.count) { apply(measured) }
                }
                process()
            }
        }
    }

    /// Rows that grew or arrived while streaming: only they are touched, new ones inserted,
    /// changed ones reconfigured where they are.
    private func applyChanges(_ measured: [RowLayout]) {
        guard let first = layouts.first?.seq else { return }
        let follow = followsBottom
        var heightsChanged = false
        var reconfigure: [IndexPath] = []
        var inserted: [IndexPath] = []
        for new in measured.sorted(by: { $0.seq < $1.seq }) {
            let i = new.seq - first
            if i < layouts.count {
                guard new.rev > layouts[i].rev else { continue }
                heightsChanged = heightsChanged || new.height != layouts[i].height
                layouts[i] = new
                reconfigure.append(IndexPath(item: i, section: 0))
            } else if i == layouts.count {
                layouts.append(new)
                inserted.append(IndexPath(item: i, section: 0))
            }
        }
        if heightsChanged || !inserted.isEmpty {
            layout.heights = layouts.map(\.height)
            UIView.performWithoutAnimation {
                if inserted.isEmpty {
                    layout.invalidateLayout()
                } else {
                    collectionView.performBatchUpdates { collectionView.insertItems(at: inserted) }
                }
            }
        }
        for path in reconfigure {
            (collectionView.cellForItem(at: path) as? RowCell)?.configure(layouts[path.item])
        }
        if follow { scrollToBottom() }
    }

    private func reloadShowing() {
        layout.heights = layouts.map(\.height)
        collectionView.reloadData()
        collectionView.layoutIfNeeded()
    }

    private func scrollToBottom() {
        collectionView.layoutIfNeeded()
        let inset = collectionView.adjustedContentInset
        let bottom = collectionView.contentSize.height + inset.bottom - collectionView.bounds.height
        collectionView.contentOffset.y = max(-inset.top, bottom)
    }

    // MARK: Data source

    func collectionView(_ collectionView: UICollectionView, numberOfItemsInSection section: Int) -> Int {
        layouts.count
    }

    func collectionView(_ collectionView: UICollectionView, cellForItemAt indexPath: IndexPath) -> UICollectionViewCell {
        let cell = collectionView.dequeueReusableCell(withReuseIdentifier: "row", for: indexPath) as! RowCell
        cell.configure(layouts[indexPath.item])
        return cell
    }

    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        guard scrollView.isTracking || scrollView.isDecelerating || probe?.running == true else { return }
        let inset = scrollView.adjustedContentInset
        let distance = scrollView.contentSize.height + inset.bottom - scrollView.bounds.height - scrollView.contentOffset.y
        followsBottom = distance < 80
        // Ask for the page before this one while a few screens of rows are still above.
        if scrollView.contentOffset.y + inset.top < scrollView.bounds.height * 4 { model.loadEarlier() }
    }
}

/// `YH_SCROLL_PROBE=<s>`: once the transcript has loaded, scrolls it up at a fling's speed for
/// that long, reading earlier pages as a finger would, then prints how far it got.
@MainActor
private final class ScrollProbe {
    private weak var controller: TranscriptViewController?
    private var link: CADisplayLink?
    private let seconds: Double
    private var started: CFTimeInterval = 0
    private var startSeq = 0
    private(set) var running = false

    init(seconds: Double, controller: TranscriptViewController) {
        self.seconds = seconds
        self.controller = controller
        Timer.scheduledTimer(withTimeInterval: 3, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.start() }
        }
    }

    private func start() {
        guard let c = controller else { return }
        startSeq = c.transcript.firstSeq
        Perf.shared.log("SCROLL probe start: rows \(c.transcript.firstSeq)..<\(c.transcript.count)")
        running = true
        started = CACurrentMediaTime()
        let link = CADisplayLink(target: self, selector: #selector(step(_:)))
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    @objc private func step(_ link: CADisplayLink) {
        guard let c = controller else { return link.invalidate() }
        let view = c.view_
        let elapsed = CACurrentMediaTime() - started
        if elapsed > seconds {
            link.invalidate()
            running = false
            let t = c.transcript
            Perf.shared.log("SCROLL probe done: from row \(startSeq) up to row \(t.firstSeq) of \(t.count), \(t.rows.count) rows held, offset \(Int(view.contentOffset.y)) of \(Int(view.contentSize.height))")
            return
        }
        // About 4000 pt/s, a hard fling.
        let dy = 4000 * (link.targetTimestamp - link.timestamp)
        view.contentOffset.y = max(-view.adjustedContentInset.top, view.contentOffset.y - dy)
    }
}

// MARK: - The cell

/// Any row: its pieces placed at the frames its layout measured. Views are kept and reused
/// by kind, so a row streaming in only changes text and frames.
final class RowCell: UICollectionViewCell {
    private var labels: [UILabel] = []
    private var bubbles: [UIView] = []
    private var codes: [CodeBlockView] = []
    private var rules: [UIView] = []
    private var icons: [UIImageView] = []

    override init(frame: CGRect) {
        super.init(frame: frame)
        isAccessibilityElement = true
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ layout: RowLayout) {
        var used = (labels: 0, bubbles: 0, codes: 0, rules: 0, icons: 0)
        for part in layout.parts {
            switch part {
            case .text(let text, let frame, let lines):
                let l = take(&labels, &used.labels) { () -> UILabel in
                    let l = UILabel()
                    l.lineBreakMode = .byTruncatingTail
                    return l
                }
                l.numberOfLines = lines
                l.attributedText = text
                l.frame = frame
            case .bubble(let frame):
                let b = take(&bubbles, &used.bubbles) { () -> UIView in
                    let b = UIView()
                    b.backgroundColor = .secondarySystemFill
                    b.layer.cornerRadius = 18
                    b.layer.cornerCurve = .continuous
                    return b
                }
                b.frame = frame
                contentView.sendSubviewToBack(b)
            case .code(let header, let code, let frame, let codeSize):
                let v = take(&codes, &used.codes) { CodeBlockView() }
                v.frame = frame
                v.configure(header: header, code: code, codeSize: codeSize)
            case .rule(let frame):
                let r = take(&rules, &used.rules) { () -> UIView in
                    let r = UIView()
                    r.backgroundColor = .separator
                    return r
                }
                r.frame = frame
            case .icon(let symbol, let tint, let frame):
                let i = take(&icons, &used.icons) { () -> UIImageView in
                    let i = UIImageView()
                    i.contentMode = .center
                    return i
                }
                i.preferredSymbolConfiguration = .init(font: .preferredFont(forTextStyle: .footnote))
                i.image = UIImage(systemName: symbol)
                i.tintColor = tint
                i.frame = frame
            }
        }
        hide(labels, from: used.labels)
        hide(bubbles, from: used.bubbles)
        hide(codes, from: used.codes)
        hide(rules, from: used.rules)
        hide(icons, from: used.icons)
        accessibilityLabel = layout.accessibility
    }

    private func take<V: UIView>(_ pool: inout [V], _ used: inout Int, make: () -> V) -> V {
        if used == pool.count {
            let v = make()
            contentView.addSubview(v)
            pool.append(v)
        }
        let v = pool[used]
        v.isHidden = false
        used += 1
        return v
    }

    private func hide(_ views: [UIView], from: Int) {
        for v in views.dropFirst(from) { v.isHidden = true }
    }
}

final class CodeBlockView: UIView {
    private let scroll = UIScrollView()
    private let header = UILabel()
    private let code = UILabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = .secondarySystemBackground
        layer.cornerRadius = 10
        layer.cornerCurve = .continuous
        clipsToBounds = true
        code.numberOfLines = 0
        scroll.showsHorizontalScrollIndicator = false
        scroll.addSubview(code)
        addSubview(header)
        addSubview(scroll)
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(header: NSAttributedString, code: NSAttributedString, codeSize: CGSize) {
        let p = RowLayout.codePadding
        self.header.attributedText = header
        self.header.frame = CGRect(x: p.left, y: p.top, width: bounds.width - p.left - p.right, height: ceil(header.size().height))
        let top = self.header.frame.maxY + 4
        scroll.frame = CGRect(x: 0, y: top, width: bounds.width, height: codeSize.height)
        self.code.attributedText = code
        self.code.frame = CGRect(x: p.left, y: 0, width: codeSize.width, height: codeSize.height)
        scroll.contentSize = CGSize(width: codeSize.width + p.left + p.right, height: codeSize.height)
        scroll.contentOffset = .zero
    }
}
