import AppKit
import WebKit
import HarnessBackend

/// The page and the host talk in JSON: the page calls `window.webkit.messageHandlers.host
/// .postMessage({method, params})` and gets a promise of the reply; the host hands it events
/// with `__yh.recv([...])`, at most once a display frame and never while the last batch is
/// still being taken, so a busy page gets bigger batches, not a queue.
@MainActor
final class Bridge: NSObject, WKScriptMessageHandlerWithReply {
    let webView: WKWebView
    let window: NSWindow
    let backend: Backend
    let outbox = Outbox()
    weak var probe: HostProbe?
    private var link: CADisplayLink?
    private var delivering = false

    init(webView: WKWebView, window: NSWindow) {
        self.webView = webView
        self.window = window
        backend = Backend(dataDirectory: Self.dataDirectory())
        AppScheme.shared.attachments = Self.dataDirectory().appendingPathComponent("attachments", isDirectory: true)
        super.init()
        backend.sink = { [weak self] patch in self?.outbox.push(patch) }
        // `YH_PROJECT=<folder>`: that folder as a project, for trying things somewhere throwaway.
        if let path = env["YH_PROJECT"] { backend.library.addProject(path: path) }
        outbox.onPending = { [weak self] in self?.link?.isPaused = false }
        let link = webView.displayLink(target: self, selector: #selector(tick))
        link.add(to: .main, forMode: .common)
        link.isPaused = true
        self.link = link
    }

    /// `YH_DATA_DIR`, else a folder of its own until the switch: never the frozen app's data.
    static func dataDirectory() -> URL {
        if let dir = env["YH_DATA_DIR"] { return URL(fileURLWithPath: dir, isDirectory: true) }
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("YourHarness Next", isDirectory: true)
    }

    @objc private func tick() {
        guard !delivering else { return }
        guard let data = outbox.drain() else {
            link?.isPaused = true
            return
        }
        delivering = true
        if env["YH_EVENT_LOG"] == "1" { log("EVENTS " + String(decoding: data, as: UTF8.self).prefix(4000)) }
        let script = "__yh.recv(" + String(decoding: data, as: UTF8.self) + ")"
        webView.evaluateJavaScript(script) { [weak self] _, error in
            if let error { log("recv failed: \(error)") }
            self?.delivering = false
        }
    }

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage,
                               replyHandler: @escaping @MainActor @Sendable (Any?, String?) -> Void) {
        guard let body = message.body as? [String: Any], let method = body["method"] as? String else {
            return replyHandler(nil, "bad message")
        }
        let params = body["params"] as? [String: Any] ?? [:]
        Task { @MainActor in
            do {
                let result = try await self.handle(method, params)
                // Big answers go as JSON text: the page parses it faster than WebKit converts objects.
                if let object = result as? [String: Any], let data = try? JSONSerialization.data(withJSONObject: object) {
                    replyHandler(String(decoding: data, as: UTF8.self), nil)
                } else {
                    replyHandler(result, nil)
                }
            } catch {
                replyHandler(nil, "\(error)")
            }
        }
    }

    private func handle(_ method: String, _ p: [String: Any]) async throws -> Any? {
        switch method {
        case "log":
            log(p["s"] as? String ?? "")
            return true
        case "quit":
            NSApp.terminate(nil)
            return true
        case "menu":
            return NativeMenu.show(items: p["items"] as? [[String: Any]] ?? [], at: NSPoint(x: p["x"] as? Double ?? 0, y: p["y"] as? Double ?? 0), in: webView)
        case "confirm":
            let alert = NSAlert()
            alert.messageText = p["title"] as? String ?? ""
            alert.informativeText = p["message"] as? String ?? ""
            let ok = alert.addButton(withTitle: p["ok"] as? String ?? "OK")
            ok.hasDestructiveAction = true
            alert.addButton(withTitle: "Cancel")
            return await alert.beginSheetModal(for: window) == .alertFirstButtonReturn
        case "config":
            // Development: `YH_SELECT=<title>` opens a thread at launch; `YH_SEND=<text>` sends it
            // from a new thread in the first project (`YH_MODEL`, `YH_MODE`), `YH_ANSWER=allow|deny`
            // answers what Claude asks, `YH_QUIT=1` quits after; `YH_FLAGS` adds classes.
            return ["select": env["YH_SELECT"] ?? "", "send": env["YH_SEND"] ?? "", "model": env["YH_MODEL"] ?? "",
                    "mode": env["YH_MODE"] ?? "", "answer": env["YH_ANSWER"] ?? "", "quit": env["YH_QUIT"] == "1",
                    "flags": env["YH_FLAGS"] ?? "", "diagAfter": env["YH_DIAG_AFTER"] ?? "", "project": env["YH_PROJECT_NAME"] ?? "",
                    // `YH_SEND_IMAGE=<file>`: sent with `YH_SEND`.
                    "image": env["YH_SEND_IMAGE"].flatMap { try? Data(contentsOf: URL(fileURLWithPath: $0)).base64EncodedString() } ?? ""]
        default:
            if method.hasPrefix("probe."), let probe {
                return try await probe.handle(String(method.dropFirst(6)), p)
            }
            return try await backend.handle(method, p)
        }
    }
}

/// A real macOS menu where the page asked for one; answers the chosen item's id.
@MainActor
enum NativeMenu {
    private final class Target: NSObject {
        var chosen: String?
        @objc func pick(_ sender: NSMenuItem) { chosen = sender.representedObject as? String }
    }

    static func show(items: [[String: Any]], at point: NSPoint, in view: NSView) -> String? {
        let menu = NSMenu()
        menu.autoenablesItems = false
        let target = Target()
        for item in items {
            guard let id = item["id"] as? String, id != "-" else { menu.addItem(.separator()); continue }
            let entry = NSMenuItem(title: item["title"] as? String ?? id, action: #selector(Target.pick(_:)), keyEquivalent: "")
            entry.target = target
            entry.representedObject = id
            if item["destructive"] as? Bool == true {
                entry.attributedTitle = NSAttributedString(string: entry.title, attributes: [.foregroundColor: NSColor.systemRed])
            }
            menu.addItem(entry)
        }
        let location = view.isFlipped ? point : NSPoint(x: point.x, y: view.bounds.height - point.y)
        menu.popUp(positioning: nil, at: location, in: view)
        return target.chosen
    }
}
