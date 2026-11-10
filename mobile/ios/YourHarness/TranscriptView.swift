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
    @State private var draft = ProcessInfo.processInfo.environment["YH_PHONE_DRAFT"] ?? ""
    @State private var images: [OutgoingImage] = []
    @State private var composerHeight: CGFloat = 0
    /// `YH_PHONE_QUEUE`'s message went.
    @State private var queued = false
    @State private var askedAside = false
    @State private var showsAside = ProcessInfo.processInfo.environment["YH_PHONE_ASIDE"] != nil
    @Environment(\.horizontalSizeClass) private var sizeClass
    /// The browser beside the thread (iPad), live while it shows.
    @State private var showsBrowserPane = false
    /// A page Claude opened since the thread opened, offered above the composer (iPhone).
    @State private var offeredPage: BrowserSummary?
    /// When the thread opened (seconds since 1970): pages the agent opened before aren't news.
    @State private var openedAt = Date().timeIntervalSince1970
    private static let env = ProcessInfo.processInfo.environment
    private static var openedPull = false
    private static var openedBrowser = false

    /// The thread's browser: beside the thread on an iPad, pushed over it on a phone.
    private func showBrowser() {
        offeredPage = nil
        if sizeClass == .regular { withAnimation { showsBrowserPane.toggle() } } else { store.openBrowser = threadID }
    }

    /// "Claude opened localhost:3000": Show opens the browser; nothing streams until then.
    private func pageCard(_ page: BrowserSummary) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "globe").foregroundStyle(.tint)
            VStack(alignment: .leading, spacing: 1) {
                Text("Claude opened a page").font(.subheadline.weight(.semibold))
                Text(page.title.flatMap { $0.isEmpty ? nil : $0 } ?? BrowserScreen.host(page.url ?? "")).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer(minLength: 4)
            Button("Show") { showBrowser() }.buttonStyle(.glassProminent)
            Button("Dismiss", systemImage: "xmark") { withAnimation { offeredPage = nil } }.labelStyle(.iconOnly).buttonStyle(.glass)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .glassEffect(in: .rect(cornerRadius: 22))
        .padding(.horizontal, 12)
        .transition(.move(edge: .bottom).combined(with: .opacity))
        // Gone by itself after a while: the globe still has it.
        .task(id: page.openedAt) {
            try? await Task.sleep(for: .seconds(20))
            if offeredPage?.openedAt == page.openedAt { withAnimation { offeredPage = nil } }
        }
    }

    /// A link to a page on the Mac itself (`localhost`): opened in the thread's browser there and
    /// shown here, when the Mac streams it; else nil, and it opens as any link.
    private var openLocal: ((URL) -> Void)? {
        guard store.offersBrowser else { return nil }
        return { url in
            store.browserInput(.open(url.absoluteString), in: threadID)
            showBrowser()
        }
    }

    var body: some View {
        // Something always there, so it appears (and opens the thread) wherever it's placed:
        // an empty view never appears in a split view's detail.
        ZStack {
            Color.clear
            if let model {
                // Under the composer, so rows scroll beneath its glass; its height is the list's inset.
                TranscriptRepresentable(model: model, store: store, bottomInset: composerHeight, onPull: { store.openPull = $0 }, onLocal: openLocal).ignoresSafeArea(.container, edges: .bottom)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 8) {
                if let page = offeredPage { pageCard(page) }
                ComposerBar(threadID: threadID, draft: $draft, images: $images, showsAside: $showsAside)
            }
            .onGeometryChange(for: CGFloat.self, of: { $0.size.height }) { composerHeight = $0 }
        }
        // Claude opened a page while you're here: beside the thread on an iPad, offered on a phone.
        .onChange(of: store.browserSummaries[threadID]?.openedAt) { _, at in
            guard let at, at > openedAt, let summary = store.browserSummaries[threadID] else { return }
            openedAt = at
            Perf.shared.log("BROWSER agent opened \(summary.url ?? "")")
            if sizeClass == .regular { withAnimation { showsBrowserPane = true } } else { withAnimation { offeredPage = summary } }
        }
        .inspector(isPresented: $showsBrowserPane) {
            // Made only while it shows: a hidden one would still watch, and stream.
            if showsBrowserPane {
                BrowserScreen(threadID: threadID, onClose: { showsBrowserPane = false })
                    .inspectorColumnWidth(min: 340, ideal: 520, max: 900)
            }
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
        // Test hooks, doing what the buttons do (docs/mobile.md lists them).
        .task(id: store.canAct) {
            guard store.canAct, let text = Self.env["YH_PHONE_SEND"] else { return }
            try? await Task.sleep(for: .seconds(2))
            let pictures = Self.env["YH_PHONE_PHOTO"] == "1" ? [TestPicture.make()] : []
            Perf.shared.log("PHONE send: \(text)\(pictures.isEmpty ? "" : " +1 picture")")
            store.send(text, to: threadID, images: pictures)
        }
        .task(id: store.canControl) {
            guard store.canControl else { return }
            try? await Task.sleep(for: .seconds(1))
            // `YH_PHONE_SET=model=haiku,effort=low,mode=ask`
            for part in (Self.env["YH_PHONE_SET"] ?? "").split(separator: ",") {
                let kv = part.split(separator: "=", maxSplits: 1).map(String.init)
                guard kv.count == 2 else { continue }
                Perf.shared.log("PHONE set \(kv[0]) \(kv[1])")
                switch kv[0] {
                case "model": store.setModel(kv[1], of: threadID)
                case "effort": store.setEffort(kv[1], of: threadID)
                case "mode": store.setMode(kv[1], of: threadID)
                default: break
                }
            }
            // `YH_PHONE_WHOLE=<seq>`: a row's whole tool output asked for, as its sheet's button does.
            if let seq = Self.env["YH_PHONE_WHOLE"].flatMap(Int.init) {
                store.askWhole(thread: threadID, seq: seq)
                for _ in 0..<50 {
                    if let w = store.whole["\(threadID)#\(seq)"] {
                        Perf.shared.log("PHONE whole \(seq): input \(w.input?.count ?? 0), result \(w.result?.count ?? 0) characters")
                        break
                    }
                    try? await Task.sleep(for: .milliseconds(100))
                }
            }
            // `YH_PHONE_THREAD_ACTIONS=pin,rename:Title,settle,unsettle,delete`, a second and a half apart.
            for step in (Self.env["YH_PHONE_THREAD_ACTIONS"] ?? "").split(separator: ",").map(String.init) {
                try? await Task.sleep(for: .seconds(1.5))
                Perf.shared.log("PHONE thread \(step)")
                switch step {
                case "pin": store.pin(threadID, true)
                case "unpin": store.pin(threadID, false)
                case "settle": store.settle(threadID, true)
                case "unsettle": store.settle(threadID, false)
                case "delete": store.delete(threadID)
                case let s where s.hasPrefix("rename:"): store.rename(threadID, to: String(s.dropFirst(7)))
                default: break
                }
            }
        }
        // `YH_PHONE_QUEUE=<text>`: sent while Claude works; `YH_PHONE_QUEUE_ACTION=now|remove` then.
        .task(id: store.live?.running) {
            guard store.live?.running == true, let text = Self.env["YH_PHONE_QUEUE"], !queued else { return }
            queued = true
            try? await Task.sleep(for: .seconds(2))
            Perf.shared.log("PHONE queue: \(text)")
            store.send(text, to: threadID)
        }
        .task(id: store.live?.queue.first?.id) {
            guard let q = store.live?.queue.first, let action = Self.env["YH_PHONE_QUEUE_ACTION"] else { return }
            try? await Task.sleep(for: .seconds(2))
            guard store.live?.queue.first?.id == q.id else { return }
            Perf.shared.log("PHONE queued \(action)")
            if action == "now" { store.sendNow(q, in: threadID) } else { store.withdraw(q, in: threadID) }
        }
        // `YH_PHONE_STOP=<s>`: Stop pressed that long after the thread starts working.
        .task(id: store.live?.running) {
            guard store.live?.running == true, let after = Self.env["YH_PHONE_STOP"].flatMap(Double.init) else { return }
            try? await Task.sleep(for: .seconds(after))
            guard store.live?.running == true else { return }
            Perf.shared.log("PHONE stop")
            store.stop(threadID)
        }
        // A request waiting: `YH_PHONE_ANSWER=allow|deny` for a tool; `YH_PHONE_CHOOSE=<label>[,<label>]`
        // answers Claude's question (`YH_PHONE_ANSWER=deny` skips it); `YH_PHONE_PLAN=approve:<mode>|keep:<feedback>`
        // for a plan; each `YH_PHONE_ANSWER_DELAY` s (3) after it shows.
        .task(id: store.live?.requests.first?.id) {
            guard let request = store.live?.requests.first else { return }
            Perf.shared.log("PHONE card: \(request.tool) · \(request.description.prefix(60))")
            try? await Task.sleep(for: .seconds(Self.env["YH_PHONE_ANSWER_DELAY"].flatMap(Double.init) ?? 3))
            guard store.live?.requests.first?.id == request.id else { return }
            switch request.tool {
            case "AskUserQuestion":
                let questions = AskedQuestion.parse(request.input)
                if let labels = Self.env["YH_PHONE_CHOOSE"]?.split(separator: ",").map({ $0.trimmingCharacters(in: .whitespaces) }) {
                    var chosen: [Int: Set<Int>] = [:]
                    var written: [Int: String] = [:]
                    for q in questions {
                        let picks = q.options.indices.filter { labels.contains(q.options[$0].label) }
                        if picks.isEmpty { written[q.id] = labels.first ?? "" } else { chosen[q.id] = Set(q.multi ? picks : [picks[0]]) }
                    }
                    let answers = AskedQuestion.answers(questions, chosen: chosen, written: written)
                    Perf.shared.log("PHONE answer \(answers.values.sorted().joined(separator: "; "))")
                    store.answer(request, answers: answers, in: threadID)
                } else if Self.env["YH_PHONE_ANSWER"] == "deny" {
                    Perf.shared.log("PHONE skip question")
                    store.skip(request, in: threadID)
                }
            case "ExitPlanMode":
                guard let plan = Self.env["YH_PHONE_PLAN"] else { return }
                let parts = plan.split(separator: ":", maxSplits: 1).map(String.init)
                Perf.shared.log("PHONE plan \(plan)")
                if parts.first == "approve" { store.approvePlan(request, mode: parts.count > 1 ? parts[1] : "auto", feedback: "", in: threadID) }
                else { store.keepPlanning(request, feedback: parts.count > 1 ? parts[1] : "", in: threadID) }
            default:
                guard let answer = Self.env["YH_PHONE_ANSWER"] else { return }
                Perf.shared.log("PHONE \(answer) \(request.tool)")
                store.respond(request, allow: answer == "allow", in: threadID)
            }
        }
        // `YH_PHONE_MCP=accept|decline`: an MCP server's question answered.
        .task(id: store.live?.questions.first?.id) {
            guard let q = store.live?.questions.first, let answer = Self.env["YH_PHONE_MCP"] else { return }
            try? await Task.sleep(for: .seconds(2))
            Perf.shared.log("PHONE mcp \(answer) \(q.server)")
            store.answerQuestion(q, accept: answer == "accept", in: threadID)
        }
        .navigationTitle(store.threads[threadID]?.title ?? "Thread")
        // The project, then the model and effort it runs with.
        .navigationSubtitle([store.threads[threadID].flatMap(store.projectName), store.offersControl ? ThreadSettings.summary(store.threads[threadID], models: store.models) : nil].compactMap { $0 }.joined(separator: " · "))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if store.offersBrowser {
                ToolbarItem(placement: .topBarTrailing) {
                    // A dot when the thread's browser has pages open.
                Button { showBrowser() } label: {
                    Image(systemName: "globe")
                        .overlay(alignment: .topTrailing) {
                            if (store.browserSummaries[threadID]?.tabs ?? 0) > 0 {
                                Circle().fill(.tint).frame(width: 8, height: 8).offset(x: 3, y: -2)
                            }
                        }
                }
                .accessibilityLabel((store.browserSummaries[threadID]?.tabs ?? 0) > 0 ? "Browser, pages open" : "Browser")
                }
            }
            if store.serverProtocol >= 5 {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Side Chat", systemImage: "bubble.left.and.text.bubble.right") { showsAside = true }
                }
            }
            if store.offersControl {
                ToolbarItem(placement: .topBarTrailing) { ThreadSettings(threadID: threadID) }
            }
            if store.threads[threadID]?.isRunning == true {
                ToolbarItem(placement: .topBarTrailing) {
                    Image(systemName: "circle.fill").font(.system(size: 8)).foregroundStyle(.green)
                        .symbolEffect(.pulse, options: .repeating)
                        .accessibilityLabel("Working")
                }
            }
        }
        .sheet(isPresented: $showsAside) { AsideSheet(threadID: threadID) }
        // A PR's link: its review page, pushed over the thread (which stays watched beneath).
        .navigationDestination(item: Bindable(store).openPull) { PullScreen(link: $0) }
        // The thread's browser on the Mac, streamed: pushed over the thread (watched beneath).
        .navigationDestination(isPresented: Binding(get: { store.openBrowser == threadID }, set: { if !$0, store.openBrowser == threadID { store.openBrowser = nil } })) {
            BrowserScreen(threadID: threadID)
        }
        // `YH_PHONE_OPEN_BROWSER=1`: the thread's browser pushed, as its button does.
        .task(id: store.offersBrowser) {
            guard store.offersBrowser, Self.env["YH_PHONE_OPEN_BROWSER"] == "1", !Self.openedBrowser else { return }
            Self.openedBrowser = true
            try? await Task.sleep(for: .seconds(1))
            Perf.shared.log("OPEN browser")
            store.openBrowser = threadID
        }
        // `YH_PHONE_ASIDE=<question>`: asked in the side chat, which is open.
        .task(id: store.canAct) {
            guard store.canAct, let question = Self.env["YH_PHONE_ASIDE"], !askedAside else { return }
            askedAside = true
            try? await Task.sleep(for: .seconds(2))
            Perf.shared.log("PHONE aside: \(question)")
            store.askAside(question, in: threadID)
        }
        // `YH_PHONE_OPEN_PULL=<url>`: that PR's page pushed over the thread, as tapping its link does.
        .task(id: store.offersPullReview) {
            guard store.offersPullReview, let pull = Self.env["YH_PHONE_OPEN_PULL"].flatMap(PullLink.init(_:)), !Self.openedPull else { return }
            Self.openedPull = true
            try? await Task.sleep(for: .seconds(1))
            Perf.shared.log("OPEN pull \(pull.url)")
            store.openPull = pull
        }
        .onAppear { model = store.open(threadID) }
        // A PR's page pushed over it keeps the thread open; leaving the thread closes it (`path`).
        .onDisappear { if let model, store.openPull == nil, store.openBrowser == nil { store.close(model) } }
    }
}

struct TranscriptRepresentable: UIViewControllerRepresentable {
    let model: TranscriptModel
    /// For a row's sheet: its whole output is asked of the Mac.
    let store: Store
    var bottomInset: CGFloat = 0
    /// A PR's link tapped: its review page (protocol 6), instead of the browser.
    var onPull: ((PullLink) -> Void)?
    /// A `localhost` link tapped: the Mac's browser (protocol 7), instead of the phone's.
    var onLocal: ((URL) -> Void)?

    func makeUIViewController(context: Context) -> TranscriptViewController {
        let controller = TranscriptViewController(model: model)
        controller.store = store
        controller.onPull = onPull
        controller.onLocal = onLocal
        return controller
    }

    func updateUIViewController(_ controller: TranscriptViewController, context: Context) {
        controller.setBottomInset(bottomInset)
        controller.onPull = onPull
        controller.onLocal = onLocal
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
    private var openedRow = false
    var onPull: ((PullLink) -> Void)?
    var onLocal: ((URL) -> Void)?

    /// A link tapped: a PR's opens its review page when the Mac can read it, a page on the Mac
    /// (`localhost`) its browser, the rest the phone's browser.
    func openLink(_ url: URL) {
        Perf.shared.log("OPEN link \(url.absoluteString)")
        if let onLocal, LocalLink.isLocal(url) {
            onLocal(url)
        } else if let onPull, store?.offersPullReview == true, let pull = PullLink(url) {
            onPull(pull)
        } else {
            UIApplication.shared.open(url)
        }
    }
    fileprivate var view_: UICollectionView { collectionView }
    fileprivate var transcript: TranscriptModel { model }

    weak var store: Store?

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
        collectionView.allowsSelection = true
        collectionView.keyboardDismissMode = .interactive
        collectionView.register(RowCell.self, forCellWithReuseIdentifier: "row")
        view.addSubview(collectionView)
        model.onUpdate = { [weak self] update in self?.enqueue(update) }
        NotificationCenter.default.addObserver(self, selector: #selector(diagramReady(_:)), name: DiagramCache.ready, object: nil)
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
                // `YH_OPEN_ROW=<tool name|thinking>`: the newest such row opened, as a tap does.
                if let wanted = ProcessInfo.processInfo.environment["YH_OPEN_ROW"], !openedRow,
                   let row = model.rows.last(where: { $0.row.toolName == wanted || $0.row.kind == wanted })?.row {
                    openedRow = true
                    Perf.shared.log("OPEN row \(row.seq) \(wanted)")
                    openDetail(row)
                }
                // `YH_OPEN_PICTURE=1`: the newest row's pictures opened, as tapping one does.
                if ProcessInfo.processInfo.environment["YH_OPEN_PICTURE"] == "1", !openedRow,
                   let row = model.rows.last(where: { m in
                       if case .user(_, let a) = m.content { return !Pictures.yours(a).isEmpty }
                       if case .media(_, let p) = m.content { return !p.isEmpty }
                       return false
                   }) {
                    openedRow = true
                    Perf.shared.log("OPEN pictures \(row.seq)")
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in self?.openPictures(seq: row.seq, index: 0) }
                }
                // `YH_OPEN_LINK=1`: the last link on screen found where it's drawn and opened, as a tap there does.
                if ProcessInfo.processInfo.environment["YH_OPEN_LINK"] == "1", !openedRow {
                    openedRow = true
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
                        guard let self else { return }
                        let cells = collectionView.indexPathsForVisibleItems.sorted().reversed().compactMap { self.collectionView.cellForItem(at: $0) as? RowCell }
                        guard let cell = cells.first(where: { $0.lastLinkPoint() != nil }), let point = cell.lastLinkPoint(),
                              let url = cell.link(at: point) else { return Perf.shared.log("OPEN link none") }
                        self.openLink(url)
                    }
                }
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
        case .remeasure(let seqs):
            let rows = seqs.compactMap { model.row($0) }
            measureInBackground(rows) { [weak self] measured in self?.replace(measured) }
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
        // Nothing shown yet (a thread opened empty): its rows start where the model's do.
        let first = layouts.first?.seq ?? model.firstSeq
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

    /// Rows measured again with nothing new in them (a diagram drawn): put in place whatever
    /// their copy, heights and cells updated.
    private func replace(_ measured: [RowLayout]) {
        guard let first = layouts.first?.seq else { return }
        var reconfigure: [IndexPath] = []
        var heightsChanged = false
        for new in measured {
            let i = new.seq - first
            guard layouts.indices.contains(i), layouts[i].seq == new.seq else { continue }
            heightsChanged = heightsChanged || new.height != layouts[i].height
            layouts[i] = new
            reconfigure.append(IndexPath(item: i, section: 0))
        }
        if heightsChanged {
            layout.heights = layouts.map(\.height)
            UIView.performWithoutAnimation { layout.invalidateLayout() }
        }
        for path in reconfigure {
            (collectionView.cellForItem(at: path) as? RowCell)?.configure(layouts[path.item])
        }
        if followsBottom { scrollToBottom() }
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
        let seq = layouts[indexPath.item].seq
        cell.thread = model.threadID
        cell.onPicture = { [weak self] index in self?.openPictures(seq: seq, index: index) }
        cell.onDiagram = { [weak self] key in self?.openDiagram(key) }
        cell.onLink = { [weak self] url in self?.openLink(url) }
        cell.configure(layouts[indexPath.item])
        return cell
    }

    // MARK: Opening and copying rows

    /// A tool call or thinking opens in a sheet: what it did, an edit as a diff.
    func collectionView(_ collectionView: UICollectionView, shouldSelectItemAt indexPath: IndexPath) -> Bool {
        guard let row = model.row(layouts[indexPath.item].seq)?.row else { return false }
        return row.kind == "tool" || (row.kind == "thinking" && !row.text.isEmpty)
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: false)
        guard let row = model.row(layouts[indexPath.item].seq)?.row else { return }
        openDetail(row)
    }

    /// A row's pictures full screen, from the one tapped.
    fileprivate func openPictures(seq: Int, index: Int) {
        guard let row = model.row(seq) else { return }
        let count: Int = switch row.content {
        case .user(_, let attachments): Pictures.yours(attachments).count
        case .media(_, let pictures): pictures.count
        default: 0
        }
        guard count > 0 else { return }
        let viewer = UIHostingController(rootView: PictureViewer(thread: model.threadID, seq: seq, count: count, page: min(index, count - 1)))
        viewer.modalPresentationStyle = .fullScreen
        present(viewer, animated: true)
    }

    /// A diagram at its own size, to zoom and scroll.
    fileprivate func openDiagram(_ key: String) {
        guard let drawing = DiagramCache.shared.drawing(key) else { return }
        let nav = UINavigationController(rootViewController: DiagramViewer(drawing: drawing))
        nav.modalPresentationStyle = .fullScreen
        present(nav, animated: true)
        Perf.shared.log("OPEN diagram \(drawing.kind) \(Int(drawing.width))x\(Int(drawing.height))")
    }

    /// A diagram drawn by the Mac: the rows it's in measured again.
    @objc private func diagramReady(_ note: Notification) {
        guard let key = note.object as? String else { return }
        let seqs = layouts.filter { $0.diagramKeys.contains(key) }.map(\.seq)
        if !seqs.isEmpty { enqueue(.remeasure(seqs)) }
        // `YH_OPEN_DIAGRAM=1`: the newest diagram opened once drawn, as a tap does.
        if ProcessInfo.processInfo.environment["YH_OPEN_DIAGRAM"] == "1", !openedRow, layouts.last(where: { !$0.diagramKeys.isEmpty })?.diagramKeys.last == key {
            openedRow = true
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in self?.openDiagram(key) }
        }
    }

    fileprivate func openDetail(_ row: Row) {
        if case .agent? = model.row(row.seq)?.content, let store {
            let sheet = UIHostingController(rootView: SubagentSheet(thread: model.threadID, seq: row.seq, store: store))
            if let s = sheet.sheetPresentationController {
                s.detents = [.medium(), .large()]
                s.prefersGrabberVisible = true
            }
            present(sheet, animated: true)
            return
        }
        if case .media(_, let pictures)? = model.row(row.seq)?.content, !pictures.isEmpty {
            return openPictures(seq: row.seq, index: 0)
        }
        let sheet = UIHostingController(rootView: RowDetail(row: row, threadID: model.threadID, store: store))
        if let s = sheet.sheetPresentationController {
            s.detents = [.medium(), .large()]
            s.prefersGrabberVisible = true
        }
        present(sheet, animated: true)
        UISelectionFeedbackGenerator().selectionChanged()
    }

    /// Your messages and Claude's replies copy with a long press.
    func collectionView(_ collectionView: UICollectionView, contextMenuConfigurationForItemsAt indexPaths: [IndexPath], point: CGPoint) -> UIContextMenuConfiguration? {
        guard let path = indexPaths.first, let row = model.row(layouts[path.item].seq)?.row, !row.text.isEmpty,
              row.kind == "user" || row.kind == "assistant" else { return nil }
        return UIContextMenuConfiguration(actionProvider: { _ in
            UIMenu(children: [UIAction(title: "Copy", image: UIImage(systemName: "doc.on.doc")) { _ in
                UIPasteboard.general.string = row.text
            }])
        })
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
final class RowCell: UICollectionViewCell, UIGestureRecognizerDelegate {
    private var labels: [UILabel] = []
    private var bubbles: [UIView] = []
    private var codes: [CodeBlockView] = []
    private var rules: [UIView] = []
    private var icons: [UIImageView] = []
    private var cards: [UIView] = []
    private var pictures: [PictureView] = []
    private var diagrams: [DiagramView] = []
    /// The thread its row is in, for its pictures.
    var thread = ""
    /// A picture tapped (its place among the row's), a diagram tapped (its key).
    var onPicture: ((Int) -> Void)?
    var onDiagram: ((String) -> Void)?
    var onLink: ((URL) -> Void)?

    override init(frame: CGRect) {
        super.init(frame: frame)
        isAccessibilityElement = true
        // Labels don't follow their links: a tap on one opens it (only there, so a tap elsewhere
        // still opens the row).
        let tap = UITapGestureRecognizer(target: self, action: #selector(tapped(_:)))
        tap.delegate = self
        contentView.addGestureRecognizer(tap)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func gestureRecognizerShouldBegin(_ recognizer: UIGestureRecognizer) -> Bool {
        guard recognizer.view === contentView else { return super.gestureRecognizerShouldBegin(recognizer) }
        return link(at: recognizer.location(in: contentView)) != nil
    }

    @objc private func tapped(_ recognizer: UITapGestureRecognizer) {
        guard let url = link(at: recognizer.location(in: contentView)) else { return }
        if let onLink { return onLink(url) }
        Perf.shared.log("OPEN link \(url.absoluteString)")
        UIApplication.shared.open(url)
    }

    /// The web address under a point in the cell, if a link's words are there.
    func link(at point: CGPoint) -> URL? {
        for l in labels where !l.isHidden && l.frame.insetBy(dx: -4, dy: -4).contains(point) {
            guard let text = l.attributedText, let (storage, manager, container) = Self.textLayout(l) else { continue }
            defer { withExtendedLifetime(storage) {} }
            let p = CGPoint(x: point.x - l.frame.minX, y: point.y - l.frame.minY)
            let glyph = manager.glyphIndex(for: p, in: container)
            guard manager.boundingRect(forGlyphRange: NSRange(location: glyph, length: 1), in: container).insetBy(dx: -4, dy: -4).contains(p) else { continue }
            let value = text.attribute(.link, at: manager.characterIndexForGlyph(at: glyph), effectiveRange: nil)
            let url = (value as? URL) ?? (value as? String).flatMap(URL.init(string:))
            if let url, ["http", "https", "mailto"].contains(url.scheme?.lowercased() ?? "") { return url }
        }
        return nil
    }

    /// Where the cell's last link is drawn (`YH_OPEN_LINK`), in the cell.
    func lastLinkPoint() -> CGPoint? {
        for l in labels.reversed() where !l.isHidden {
            guard let text = l.attributedText, let (storage, manager, container) = Self.textLayout(l) else { continue }
            defer { withExtendedLifetime(storage) {} }
            var found: NSRange?
            text.enumerateAttribute(.link, in: NSRange(location: 0, length: text.length)) { value, range, _ in
                if value != nil { found = range }
            }
            guard let found else { continue }
            let glyphs = manager.glyphRange(forCharacterRange: NSRange(location: found.location, length: 1), actualCharacterRange: nil)
            let r = manager.boundingRect(forGlyphRange: glyphs, in: container)
            return CGPoint(x: l.frame.minX + r.midX, y: l.frame.minY + r.midY)
        }
        return nil
    }

    /// A label's text laid out as it draws it, to find what's under a point. The storage comes
    /// along: the layout manager doesn't keep it.
    private static func textLayout(_ l: UILabel) -> (NSTextStorage, NSLayoutManager, NSTextContainer)? {
        guard let text = l.attributedText, text.length > 0 else { return nil }
        let storage = NSTextStorage(attributedString: text)
        let manager = NSLayoutManager()
        let container = NSTextContainer(size: l.bounds.size)
        container.lineFragmentPadding = 0
        container.maximumNumberOfLines = l.numberOfLines
        container.lineBreakMode = l.lineBreakMode
        manager.addTextContainer(container)
        storage.addLayoutManager(manager)
        return (storage, manager, container)
    }

    func configure(_ layout: RowLayout) {
        var used = (labels: 0, bubbles: 0, codes: 0, rules: 0, icons: 0, cards: 0, pictures: 0, diagrams: 0)
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
            case .card(let frame):
                let c = take(&cards, &used.cards) { () -> UIView in
                    let c = UIView()
                    c.backgroundColor = .secondarySystemBackground
                    c.layer.cornerRadius = 16
                    c.layer.cornerCurve = .continuous
                    c.layer.borderWidth = 1 / UIScreen.main.scale
                    c.layer.borderColor = UIColor.separator.cgColor
                    return c
                }
                c.frame = frame
                contentView.sendSubviewToBack(c)
            case .picture(let index, let frame):
                let v = take(&pictures, &used.pictures) { PictureView(frame: .zero) }
                v.frame = frame
                v.load(thread: thread, seq: layout.seq, index: index)
                v.onTap = { [weak self] in self?.onPicture?(index) }
            case .diagram(let key, let frame):
                let v = take(&diagrams, &used.diagrams) { DiagramView() }
                v.frame = frame
                v.key = key
                v.drawing = DiagramCache.shared.drawing(key)
                v.onTap = { [weak self] in self?.onDiagram?(key) }
            }
        }
        hide(cards, from: used.cards)
        hide(pictures, from: used.pictures)
        hide(diagrams, from: used.diagrams)
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

/// A picture made up for test runs (`YH_PHONE_PHOTO=1`): the Simulator's library isn't there to
/// pick from. Big type, so a model can read it.
enum TestPicture {
    static func make() -> OutgoingImage {
        let size = CGSize(width: 600, height: 300)
        let image = UIGraphicsImageRenderer(size: size).image { ctx in
            UIColor.systemTeal.setFill()
            ctx.fill(CGRect(origin: .zero, size: size))
            let text = "BLUE 42" as NSString
            text.draw(at: CGPoint(x: 90, y: 90), withAttributes: [.font: UIFont.boldSystemFont(ofSize: 110), .foregroundColor: UIColor.white])
        }
        return OutgoingImage(mediaType: "image/jpeg", data: image.jpegData(compressionQuality: 0.9)!)
    }
}
