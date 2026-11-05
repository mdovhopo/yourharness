import AppKit
import WebKit

/// The page and the host talk in JSON: the page calls `window.webkit.messageHandlers.host
/// .postMessage({method, params})` and gets a promise of the reply; the host hands it events
/// with `__yh.recv([...])`, at most once a display frame and never while the last batch is
/// still being taken, so a busy page gets bigger batches, not a queue.
@MainActor
final class Bridge: NSObject, WKScriptMessageHandlerWithReply {
    let webView: WKWebView
    let window: NSWindow
    let library = Library()
    let outbox = Outbox()
    lazy var live = LiveThreads(outbox: outbox)
    weak var probe: HostProbe?
    private var link: CADisplayLink?
    private var delivering = false

    init(webView: WKWebView, window: NSWindow) {
        self.webView = webView
        self.window = window
        super.init()
        let library = self.library
        AppScheme.shared.api = { name, query in
            switch name {
            case "library":
                if library.projects.isEmpty && library.threads.isEmpty { library.scan() }
                return library.listingJSON()
            case "page":
                return library.pageJSON(thread: query["thread"] ?? "", before: query["before"].flatMap(Int.init),
                                        limit: query["limit"].flatMap(Int.init) ?? 200)
            default:
                return nil
            }
        }
        outbox.onPending = { [weak self] in self?.link?.isPaused = false }
        let link = webView.displayLink(target: self, selector: #selector(tick))
        link.add(to: .main, forMode: .common)
        link.isPaused = true
        self.link = link
    }

    @objc private func tick() {
        guard !delivering else { return }
        guard let data = outbox.drain() else {
            link?.isPaused = true
            return
        }
        delivering = true
        if env["YHW_EVENT_LOG"] == "1" { log("EVENTS " + String(decoding: data, as: UTF8.self).prefix(4000)) }
        let script = "__yh.recv(" + String(decoding: data, as: UTF8.self) + ")"
        webView.evaluateJavaScript(script) { [weak self] _, error in
            if let error { log("recv failed: \(error)") }
            self?.delivering = false
        }
    }

    func push(_ event: [String: Any]) {
        outbox.event(event)
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
                replyHandler(result, nil)
            } catch {
                replyHandler(nil, "\(error)")
            }
        }
    }

    private func handle(_ method: String, _ p: [String: Any]) async throws -> Any? {
        let thread = p["th"] as? String ?? ""
        switch method {
        case "send":
            let cwd = (p["cwd"] as? String).flatMap { $0.isEmpty ? nil : $0 }
                ?? (p["projectId"] as? String).flatMap(library.projectPath) ?? FileManager.default.homeDirectoryForCurrentUser.path
            try await live.send(thread: thread, text: p["text"] as? String ?? "", cwd: cwd,
                                sessionID: (p["sid"] as? String).flatMap { $0.isEmpty ? nil : $0 },
                                model: (p["model"] as? String).flatMap { $0.isEmpty ? nil : $0 },
                                mode: p["mode"] as? String)
            return true
        case "interrupt":
            await live.interrupt(thread: thread)
            return true
        case "respond":
            await live.respond(thread: thread, requestID: p["id"] as? String ?? "", allow: p["allow"] as? Bool ?? false)
            return true
        case "setMode":
            await live.setMode(thread: thread, mode: p["mode"] as? String ?? "default")
            return true
        case "setModel":
            await live.setModel(thread: thread, model: p["model"] as? String ?? "default")
            return true
        case "quit":
            NSApp.terminate(nil)
            return true
        case "log":
            log(p["s"] as? String ?? "")
            return true
        case "config":
            // `YHW_SELECT=<title>`: opens that thread at launch. `YHW_EXPAND=1`: unfolds work.
            // `YHW_SEND=<text>`: sends it from a new thread in the first project (`YHW_MODEL`,
            // `YHW_MODE`), and `YHW_ANSWER=allow|deny` answers what Claude asks; both logged.
            return ["select": env["YHW_SELECT"] ?? "", "expand": env["YHW_EXPAND"] == "1",
                    "send": env["YHW_SEND"] ?? "", "model": env["YHW_MODEL"] ?? "", "mode": env["YHW_MODE"] ?? "",
                    "answer": env["YHW_ANSWER"] ?? "", "diagAfter": env["YHW_DIAG_AFTER"] ?? "",
                    // `YHW_FLAGS=a,b`: classes on the page's root, for turning one thing off to measure it.
                    "flags": env["YHW_FLAGS"] ?? "",
                    // `YHW_QUIT=1`: quits once the `YHW_SEND` turn is done.
                    "quit": env["YHW_QUIT"] == "1"]
        default:
            if method.hasPrefix("probe."), let probe {
                return try await probe.handle(String(method.dropFirst(6)), p)
            }
            throw HostError("unknown method \(method)")
        }
    }
}
