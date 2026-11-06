import AppKit
@preconcurrency import Quartz

/// Images and videos in a thread, shown big in Quick Look: zoom, full screen, playback, and
/// the arrow keys go through the rest of the message's media.
///
/// Quick Look takes its items from a controller it finds in the responder chain, so this
/// puts itself in the main window's chain, right after the window, while it shows.
@MainActor
final class MediaPreview: NSResponder, QLPreviewPanelDataSource, QLPreviewPanelDelegate {
    static let shared = MediaPreview()

    // Quick Look asks for these from callbacks it doesn't mark as main actor, though it
    // calls them on the main thread.
    nonisolated(unsafe) private var urls: [URL] = []
    nonisolated(unsafe) private var startIndex = 0
    private var began = 0

    /// Shows `urls`, starting at `index`. Files that are gone are skipped.
    static func show(_ urls: [URL], at index: Int = 0) {
        let present = urls.enumerated().filter { FileManager.default.fileExists(atPath: $0.element.path) }
        guard !present.isEmpty else { return }
        let preview = shared
        preview.urls = present.map(\.element)
        preview.startIndex = present.firstIndex { $0.offset >= index } ?? 0
        guard let window = NSApp.mainWindow ?? NSApp.keyWindow ?? NSApp.windows.first(where: { $0.isVisible && $0.canBecomeMain }),
              let panel = QLPreviewPanel.shared() else { return }
        preview.attach(to: window)
        if panel.isVisible, panel.dataSource === preview {
            panel.reloadData()
            panel.currentPreviewItemIndex = preview.startIndex
        } else {
            panel.updateController()
            // With no key window (the app in the background) Quick Look finds no controller
            // in the chain: hand it the items directly.
            if panel.dataSource !== preview {
                panel.dataSource = preview
                panel.delegate = preview
                panel.reloadData()
                panel.currentPreviewItemIndex = preview.startIndex
            }
            panel.makeKeyAndOrderFront(nil)
        }
    }

    /// Development (`YH_PREVIEW_MEDIA`): the selected thread's last message with pictures or
    /// videos, and whether Quick Look came up.
    static func showLastSent(in app: AppModel) {
        // `YH_PREVIEW_MEDIA=draft`: what's waiting in the composer instead, as clicking it does.
        if ProcessInfo.processInfo.environment["YH_PREVIEW_MEDIA"] == "draft", let thread = app.selectedThread {
            thread.previewDraft(at: nil)
            Task {
                try? await Task.sleep(for: .seconds(1))
                let panel = QLPreviewPanel.shared()
                let line = "YH_PREVIEW_MEDIA draft: \(panel?.dataSource === shared && !shared.urls.isEmpty ? "PASS" : "FAIL"), \(shared.urls.count) items: "
                    + shared.urls.map(\.lastPathComponent).joined(separator: ", ") + "\n"
                FileHandle.standardError.write(Data(line.utf8))
            }
            return
        }
        let names = app.selectedThread?.entries.last { entry in
            entry.attachments.contains { UserMessageView.mediaKind($0) != nil }
        }?.attachments.filter { UserMessageView.mediaKind($0) != nil } ?? []
        guard !names.isEmpty else { return FileHandle.standardError.write(Data("YH_PREVIEW_MEDIA: no media in the selected thread\n".utf8)) }
        show(names.map(app.store.attachmentURL))
        Task {
            try? await Task.sleep(for: .seconds(1))
            let panel = QLPreviewPanel.shared()
            // Quick Look hides while the app isn't active, as a test run's isn't: what counts
            // is that it took the items.
            let line = "YH_PREVIEW_MEDIA: \(panel?.dataSource === shared ? "PASS" : "FAIL"), visible \(panel?.isVisible == true), item \(panel?.currentPreviewItemIndex ?? -1) of \(shared.urls.count), through the chain \(shared.began > 0): "
                + names.map { ($0 as NSString).lastPathComponent }.joined(separator: ", ") + "\n"
            FileHandle.standardError.write(Data(line.utf8))
        }
    }

    private func attach(to window: NSWindow) {
        var responder: NSResponder? = window
        while let next = responder?.nextResponder {
            if next === self { return }
            responder = next
        }
        nextResponder = window.nextResponder
        window.nextResponder = self
    }

    // MARK: QLPreviewPanelController

    override func acceptsPreviewPanelControl(_ panel: QLPreviewPanel!) -> Bool { !urls.isEmpty }

    override func beginPreviewPanelControl(_ panel: QLPreviewPanel!) {
        MainActor.assumeIsolated {
            began += 1
            panel.dataSource = self
            panel.delegate = self
            panel.currentPreviewItemIndex = startIndex
        }
    }

    override func endPreviewPanelControl(_ panel: QLPreviewPanel!) {
        MainActor.assumeIsolated {
            panel.dataSource = nil
            panel.delegate = nil
            urls = []
        }
    }

    // MARK: QLPreviewPanelDataSource

    nonisolated func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int {
        urls.count
    }

    nonisolated func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> (any QLPreviewItem)! {
        urls.indices.contains(index) ? urls[index] as NSURL : nil
    }
}
