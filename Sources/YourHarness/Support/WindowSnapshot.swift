import AppKit

/// Development: `YH_SNAPSHOT=<file.png>` draws the main window into a PNG a few seconds after
/// launch (and `YH_SNAPSHOT_DELAY=<seconds>`), whether or not it's on screen: for checking
/// how things look when the screen can't be captured. A sheet open on it is drawn too, into
/// `<file>-sheet.png`. Off unless set.
@MainActor
enum WindowSnapshot {
    static func runIfRequested() {
        let environment = ProcessInfo.processInfo.environment
        guard let path = environment["YH_SNAPSHOT"], !path.isEmpty else { return }
        let delay = environment["YH_SNAPSHOT_DELAY"].flatMap(Double.init) ?? 5
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { write(to: path) }
    }

    /// Draws the main window into a PNG now.
    @discardableResult
    static func write(to path: String) -> Bool {
        // `YH_SNAPSHOT_WINDOW=<title>` draws the window with that title instead (a memory agent's, say).
        let title = ProcessInfo.processInfo.environment["YH_SNAPSHOT_WINDOW"]
        guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil && $0.frame.width > 600
                && (title == nil || $0.title.contains(title!)) }),
              let view = window.contentView?.superview ?? window.contentView
        else {
            FileHandle.standardError.write(Data("snapshot: no window\n".utf8))
            return false
        }
        let written = draw(view, to: path)
        if let sheet = window.attachedSheet, let view = sheet.contentView?.superview ?? sheet.contentView {
            draw(view, to: (path as NSString).deletingPathExtension + "-sheet.png")
        }
        return written
    }

    @discardableResult
    private static func draw(_ view: NSView, to path: String) -> Bool {
        guard let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return false }
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let written = (try? bitmap.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))) != nil
        FileHandle.standardError.write(Data("snapshot: \(written ? "wrote" : "couldn't write") \(path)\n".utf8))
        return written
    }
}
