import AppKit
import HarnessCore

/// Debug → Capture Transcript State: what the open thread's transcript looks like to the app
/// right now (where it's scrolled, which rows are laid out and in view, the window), and the
/// window drawn, into the data folder's `diagnostics/`. For the transcript showing nothing
/// until you scroll, which can't be made to happen on demand: captured while it's empty, it
/// tells whether the scroll position was past the rows, the rows weren't laid out there, or
/// they were and didn't draw.
@MainActor
enum TranscriptDiagnostics {
    /// Kept up by each transcript as it scrolls and its rows come and go: a few assignments,
    /// nothing worked out.
    struct Scroll {
        var offset: CGFloat = 0
        var contentHeight: CGFloat = 0
        var containerHeight: CGFloat = 0
        var insetTop: CGFloat = 0
        var insetBottom: CGFloat = 0
        /// At least 30% in view, as the transcript counts them for jumping between messages.
        var visibleRows: [String] = []
        /// Any part laid out in view.
        var touchedRows: [String] = []
        var followsBottom = true
        var isShort = true
        var updated = Date.now
    }

    static var scroll: [UUID: Scroll] = [:]

    static func capture(app: AppModel, reveal: Bool = true) {
        let stamp = Int(Date.now.timeIntervalSince1970)
        let folder = app.store.root.appending(path: "diagnostics", directoryHint: .isDirectory)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let base = folder.appending(path: "transcript-\(stamp)")

        var report: [String: Any] = [
            "capturedAt": ISO8601DateFormatter().string(from: .now),
            "app": [StateArchive.AppInfo.current.version ?? "?", StateArchive.AppInfo.current.build ?? "?", StateArchive.AppInfo.current.commit ?? "?"],
            "windowsSeen": WindowPresence.shared.isVisible,
        ]
        if let window = NSApp.windows.first(where: { $0.frameAutosaveName == "main" }) {
            report["window"] = [
                "frame": NSStringFromRect(window.frame),
                "fullScreen": window.styleMask.contains(.fullScreen),
                "zoomed": window.isZoomed,
                "visible": window.occlusionState.contains(.visible),
                "screen": window.screen.map { NSStringFromRect($0.frame) } ?? "none",
                "backingScale": window.backingScaleFactor,
            ]
            report["otherWindowsOverIt"] = NSApp.windows
                .filter { $0 !== window && $0.isVisible && $0.frame.intersects(window.frame) }
                .map { "\(type(of: $0)) “\($0.title)” \(NSStringFromRect($0.frame)) level \($0.level.rawValue)" }
        }
        if let thread = app.selectedThread {
            let rows = thread.transcriptDisplayRows
            let scroll = scroll[thread.id]
            let visible = Set(scroll?.visibleRows ?? []), touched = Set(scroll?.touchedRows ?? [])
            report["thread"] = [
                "id": thread.id.uuidString,
                "phase": "\(thread.phase)",
                "entries": thread.entries.count,
                "rows": rows.count,
                "shownTurns": thread.shownTurns,
                "scrollAnchor": thread.scrollAnchor ?? NSNull(),
            ] as [String: Any]
            if let scroll {
                report["scroll"] = [
                    "offset": scroll.offset, "contentHeight": scroll.contentHeight, "containerHeight": scroll.containerHeight,
                    "insets": [scroll.insetTop, scroll.insetBottom],
                    "pastTheEnd": scroll.offset + scroll.containerHeight + scroll.insetTop > scroll.contentHeight + 1,
                    "followsBottom": scroll.followsBottom, "isShort": scroll.isShort,
                    "secondsSinceUpdate": Date.now.timeIntervalSince(scroll.updated),
                ] as [String: Any]
            } else {
                report["scroll"] = "never reported"
            }
            // Every row, marked by whether it's in view; kinds and sizes, never what was said.
            report["rowList"] = rows.enumerated().map { index, row in
                let mark = visible.contains(row.id) ? "visible" : touched.contains(row.id) ? "partly" : "-"
                let kind = row.entry.map { "\($0.kind)" } ?? "\(row)".prefix(while: { $0 != "(" }).description
                let length = row.entry?.text.count ?? 0
                return "\(index) \(mark) \(kind) \(length) chars"
            }
        }
        let json = (try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])) ?? Data()
        try? json.write(to: base.appendingPathExtension("json"))
        WindowSnapshot.write(to: base.appendingPathExtension("png").path)
        if reveal { NSWorkspace.shared.activateFileViewerSelecting([base.appendingPathExtension("json")]) }
        FileHandle.standardError.write(Data("captured \(base.path).json\n".utf8))
    }

    /// A transcript found blank (its rows laid out somewhere else than in view) goes to its end:
    /// what it looked like is captured first, quietly and once a launch, being the blank
    /// transcript caught in the act.
    private static var caughtLostPlace = false

    static func lostPlace(app: AppModel) {
        FileHandle.standardError.write(Data("transcript lost its rows in view: going to the end\n".utf8))
        guard !caughtLostPlace else { return }
        caughtLostPlace = true
        capture(app: app, reveal: false)
    }

    /// Development: `YH_CAPTURE_TRANSCRIPT=<seconds>` captures that long after launch, without
    /// showing it in Finder.
    static func runIfRequested(app: AppModel) {
        guard let delay = ProcessInfo.processInfo.environment["YH_CAPTURE_TRANSCRIPT"].flatMap(Double.init) else { return }
        Task {
            try? await Task.sleep(for: .seconds(delay))
            capture(app: app, reveal: false)
        }
    }
}
