import AppKit

/// Whether any of the app's windows can be seen. AppKit keeps laying out and drawing a window
/// nobody can see (covered, minimized, on another Space, the app hidden, the display asleep)
/// as busily as a visible one, so while none can be seen, streaming threads hold their events
/// and apply them together once a second (`whenSeen`), and what animates stops.
@MainActor @Observable
final class WindowPresence {
    static let shared = WindowPresence()

    /// True until AppKit says otherwise.
    private(set) var isVisible = true

    @ObservationIgnored private var waiting: [() -> Void] = []
    @ObservationIgnored private var flushTask: Task<Void, Never>?
    @ObservationIgnored private var observers: [NSObjectProtocol] = []

    /// Follows the windows from now on.
    func start() {
        guard observers.isEmpty else { return }
        let names: [Notification.Name] = [NSWindow.didChangeOcclusionStateNotification,
                                          NSApplication.didHideNotification, NSApplication.didUnhideNotification]
        observers = names.map { name in
            NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { _ in
                MainActor.assumeIsolated { WindowPresence.shared.update() }
            }
        }
    }

    private func update() {
        let visible = NSApp.windows.contains { $0.isVisible && $0.occlusionState.contains(.visible) }
        guard visible != isVisible else { return }
        isVisible = visible
        if visible { flush() }
    }

    /// Runs `apply` once a window can be seen again, or within a second, together with
    /// everything else waiting: one update of the UI rather than one per event.
    func whenSeen(_ apply: @escaping () -> Void) {
        waiting.append(apply)
        guard flushTask == nil else { return }
        flushTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(1))
            self?.flush()
        }
    }

    /// Applies everything waiting now: a window can be seen, or the app is quitting.
    func flush() {
        flushTask?.cancel()
        flushTask = nil
        let batch = waiting
        waiting = []
        for apply in batch { apply() }
    }
}
