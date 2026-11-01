import AppKit
import HarnessCore
import SwiftUI

/// The transcripts of the threads you've been in, each with a hosting view of its own, kept made:
/// going back to one swaps a view, instead of making hundreds of views again, and it's as you
/// left it. Parked out of the window, a transcript does nothing, however much its thread does;
/// it catches up when it's shown.
///
/// A hosting view of its own also keeps a transcript's updates to itself: in the window's own,
/// every change went through the whole window's graph, toolbar included, and the toolbar asks
/// for another layout pass when its preferences change.
struct TranscriptDeck: NSViewRepresentable {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    let bottomInset: CGFloat

    // There's one for the app: it outlives this view, so going to a project's page and back
    // keeps every transcript made.
    func makeNSView(context: Context) -> TranscriptDeckView { app.transcriptDeck }

    func updateNSView(_ deck: TranscriptDeckView, context: Context) {
        deck.show(thread, bottomInset: bottomInset, app: app)
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView: TranscriptDeckView, context: Context) -> CGSize? {
        proposal.replacingUnspecifiedDimensions()
    }
}

/// A transcript and what it hears from the deck.
@MainActor @Observable
final class TranscriptCard {
    /// On screen, rather than parked for coming back to.
    var isShown = false {
        didSet { rows.isShown = isShown }
    }
    /// The composer's height with what's on it: the transcript scrolls under it.
    var bottomInset: CGFloat = 0 {
        didSet { list.bottomInset = bottomInset }
    }
    @ObservationIgnored let rows: TranscriptRowContext
    @ObservationIgnored let list: TranscriptListView

    init(thread: ThreadModel, app: AppModel) {
        rows = TranscriptRowContext(app: app, thread: thread)
        list = TranscriptListView(thread: thread, context: rows)
    }
}

final class TranscriptDeckView: NSView {
    @MainActor private final class Entry {
        let thread: ThreadModel
        let card: TranscriptCard
        let host: NSHostingView<TranscriptCardRoot>

        init(thread: ThreadModel, app: AppModel) {
            self.thread = thread
            card = TranscriptCard(thread: thread, app: app)
            host = NSHostingView(rootView: TranscriptCardRoot(thread: thread, card: card, app: app))
            host.sizingOptions = []
        }
    }

    private var entries: [UUID: Entry] = [:]
    /// Newest first.
    private var recent: [UUID] = []
    private var current: Entry?
    /// Made, not yet shown: see `show`.
    private var waiting: Entry?
    private var bottomInset: CGFloat = 0

    /// Enough for every thread you'd work in at once. What keeps memory in check is the transcripts
    /// being let go of (`AppModel.trimLoadedTranscripts`), which takes their cards with them.
    private static let kept = ProcessInfo.processInfo.environment["YH_KEPT_TRANSCRIPTS"].flatMap(Int.init) ?? 24

    override var isFlipped: Bool { true }

    override func layout() {
        super.layout()
        current?.host.frame = bounds
    }

    func show(_ thread: ThreadModel, bottomInset: CGFloat, app: AppModel) {
        if bottomInset != self.bottomInset {
            self.bottomInset = bottomInset
            current?.card.bottomInset = bottomInset
        }
        guard (waiting ?? current)?.thread !== thread else { return }
        recent = [thread.id] + recent.filter { $0 != thread.id }
        waiting = nil
        if let entry = entries[thread.id] {
            swap(to: entry)
        } else {
            let entry = Entry(thread: thread, app: app)
            entries[thread.id] = entry
            if current == nil || thread.entries.isEmpty {
                swap(to: entry)
            } else {
                // Rows to make, which takes a moment: what you clicked shows first (the sidebar,
                // the title, the composer), with this thread's rows right after, so the click is
                // answered at once.
                waiting = entry
                DispatchQueue.main.async { [weak self, weak entry] in
                    guard let self, let entry, self.waiting === entry else { return }
                    self.waiting = nil
                    self.swap(to: entry)
                }
            }
        }
        for id in recent.dropFirst(Self.kept) { forget(id) }
    }

    /// For test runs.
    var currentList: TranscriptListView? { current?.card.list }

    /// Its transcript has been let go of, or it's too long since you were there.
    func forget(_ id: UUID) {
        guard let entry = entries[id], entry !== current, entry !== waiting else { return }
        entry.card.list.rememberPlace()
        entries[id] = nil
        recent.removeAll { $0 == id }
        // Taking down a graph takes a moment: not during the click that led here.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { _ = entry }
    }

    private func swap(to entry: Entry) {
        // Back to the one still showing, before the one you'd gone to was shown.
        guard entry !== current else { return }
        let previous = current
        let hadFocus = previous.map { (window?.firstResponder as? NSView)?.isDescendant(of: $0.host) ?? false } ?? false
        entry.card.bottomInset = bottomInset
        entry.card.isShown = true
        entry.host.frame = bounds
        addSubview(entry.host)
        current = entry
        guard let previous else { return }
        previous.card.list.rememberPlace()
        previous.card.isShown = false
        previous.host.removeFromSuperview()
        // The text you'd selected there went with it: the keyboard goes back to the composer.
        if hadFocus { AppModel.shared.composerFocusRequests += 1 }
    }
}

/// What a transcript gets from the window, which a hosting view doesn't inherit.
struct TranscriptCardRoot: View {
    let thread: ThreadModel
    let card: TranscriptCard
    let app: AppModel

    var body: some View {
        TranscriptView(thread: thread, card: card)
            // For what floats over it.
            .safeAreaPadding(.bottom, card.bottomInset)
            .environment(thread)
            .environment(app)
    }
}
