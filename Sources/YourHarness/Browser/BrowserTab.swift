import AppKit
import WebKit

/// One page in a thread's browser: a `WKWebView`, what it logged, and the input the agent
/// sends it. Clicks and keys go in as real events, so the page sees what a person does.
@MainActor @Observable
final class BrowserTab: NSObject, Identifiable {
    struct ConsoleEntry: Identifiable {
        let id: Int
        let date: Date
        let level: String
        let text: String
        var isError: Bool { level == "error" }
    }

    struct NetworkEntry: Identifiable {
        let id: Int
        let date: Date
        let method: String
        let url: String
        let status: Int?
        let type: String
        let duration: Double?
        let failure: String?
        var failed: Bool { failure != nil || (status ?? 0) >= 400 }
    }

    /// An alert, confirm or prompt the page is waiting on.
    struct Dialog: Identifiable {
        enum Kind: String { case alert, confirm, prompt }
        let id = UUID()
        let kind: Kind
        let message: String
        let defaultText: String?
        let respond: (_ accept: Bool, _ text: String?) -> Void
    }

    /// Where the agent just acted, for the panel to ring it. In CSS pixels.
    struct Highlight: Equatable {
        let id = UUID()
        let rect: CGRect
        let label: String
    }

    /// The page's size in CSS pixels; the panel scales it to fit.
    struct Viewport: Equatable {
        var width: Double
        var height: Double
        /// Follows the panel's size instead.
        var fitsPanel = false

        /// Today's screens, in the points a page sees: a 14-inch MacBook Pro at its
        /// default scaling, a 1080p monitor, an 11-inch iPad Pro, an iPhone 17 Pro.
        static let laptop = Viewport(width: 1512, height: 982)
        static let desktop = Viewport(width: 1920, height: 1080)
        static let tablet = Viewport(width: 834, height: 1210)
        static let phone = Viewport(width: 402, height: 874)
    }

    /// Short and stable, so the agent can say which tab: 1, 2, 3...
    let id: Int
    let webView: WKWebView
    /// What panels and the parking window host: the web view at the page's CSS size,
    /// drawn scaled to fit.
    let pageView = FlippedView()
    private(set) var url: URL?
    private(set) var title = ""
    private(set) var isLoading = false
    private(set) var progress = 0.0
    private(set) var canGoBack = false
    private(set) var canGoForward = false
    private(set) var console: [ConsoleEntry] = []
    private(set) var network: [NetworkEntry] = []
    var dialog: Dialog?
    private(set) var highlight: Highlight?
    /// What the agent is doing right now, shown over the page.
    private(set) var agentActivity: String?
    var viewport = Viewport.laptop
    /// A full-page screenshot has the view grown to the page's height: the panel leaves it.
    var holdsLayout = false
    /// Opened by the agent (it may close it), or by you.
    let openedByAgent: Bool
    /// Reopened at launch and not used since: the agent's next `open` can take it over.
    var isRestored = false
    /// Files for the next file chooser the page opens: what the agent asked to upload.
    var pendingUpload: [URL]?
    /// The element picker is on; what's picked goes to the composer.
    private(set) var isPicking = false
    var onPick: ((PickedElement?) -> Void)?
    /// The page asked to open another (target=_blank, window.open).
    var onOpenWindow: ((WKWebViewConfiguration, URLRequest) -> WKWebView?)?
    var onClose: (() -> Void)?
    var onURLChange: (() -> Void)?

    /// The newest log entries the agent has been told about.
    var reportedConsole = 0
    var reportedNetwork = 0
    /// Refs restart above the last page's, so a stale one can't name a new element.
    var nextRef = 1
    /// Bumped on navigation and when you take control: refs from before are stale.
    private(set) var documentGeneration = 0

    @ObservationIgnored private var observations: [NSKeyValueObservation] = []
    @ObservationIgnored private var loadWaiters: [CheckedContinuation<Void, Never>] = []
    @ObservationIgnored private var sequence = 0
    @ObservationIgnored private var highlightClear: Task<Void, Never>?
    @ObservationIgnored private var activityClear: Task<Void, Never>?
    @ObservationIgnored private var lastNavigationError: String?
    @ObservationIgnored private var navigationsStarted = 0
    private static let logLimit = 2000

    init(id: Int, configuration: WKWebViewConfiguration, openedByAgent: Bool) {
        self.id = id
        self.openedByAgent = openedByAgent
        webView = WKWebView(frame: .zero, configuration: configuration)
        super.init()
        pageView.addSubview(webView)
        place(CGRect(x: 0, y: 0, width: Viewport.laptop.width / 2, height: Viewport.laptop.height / 2),
              css: CGSize(width: Viewport.laptop.width, height: Viewport.laptop.height))
        webView.isInspectable = true
        webView.allowsBackForwardNavigationGestures = true
        webView.navigationDelegate = self
        webView.uiDelegate = self
        observations = [
            webView.observe(\.url) { [weak self] web, _ in
                MainActor.assumeIsolated {
                    self?.url = web.url
                    self?.onURLChange?()
                }
            },
            webView.observe(\.title) { [weak self] web, _ in MainActor.assumeIsolated { self?.title = web.title ?? "" } },
            webView.observe(\.isLoading) { [weak self] web, _ in MainActor.assumeIsolated { self?.loadingChanged(web.isLoading) } },
            webView.observe(\.estimatedProgress) { [weak self] web, _ in MainActor.assumeIsolated { self?.progress = web.estimatedProgress } },
            webView.observe(\.canGoBack) { [weak self] web, _ in MainActor.assumeIsolated { self?.canGoBack = web.canGoBack } },
            webView.observe(\.canGoForward) { [weak self] web, _ in MainActor.assumeIsolated { self?.canGoForward = web.canGoForward } },
        ]
    }

    var displayTitle: String {
        if !title.isEmpty { return title }
        return url.map { $0.host() ?? $0.absoluteString } ?? "New Tab"
    }

    /// For the agent: `localhost:3000/login`.
    var shortURL: String {
        guard let url else { return "about:blank" }
        return Self.short(url)
    }

    static func short(_ url: URL) -> String {
        var text = url.absoluteString
        for prefix in ["https://", "http://"] where text.hasPrefix(prefix) { text.removeFirst(prefix.count) }
        if text.hasSuffix("/") { text.removeLast() }
        return text
    }

    func close() {
        dialog?.respond(false, nil)
        dialog = nil
        stopPicking()
        webView.stopLoading()
        pageView.removeFromSuperview()
        for waiter in loadWaiters { waiter.resume() }
        loadWaiters.removeAll()
    }

    // MARK: - Loading

    /// Starts loading `text` as typed in a URL field: a URL, a host, or `localhost:3000`.
    func load(_ text: String) {
        guard let url = Self.url(from: text) else { return }
        isRestored = false
        webView.load(URLRequest(url: url))
    }

    static func url(from text: String) -> URL? {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return nil }
        if let url = URL(string: text), let scheme = url.scheme?.lowercased(),
           ["http", "https", "file", "about", "data"].contains(scheme) { return url }
        let local = text.hasPrefix("localhost") || text.hasPrefix("127.0.0.1") || text.hasPrefix("0.0.0.0") || text.hasPrefix("[::1]")
        if !text.contains(" "), text.contains(".") || text.contains(":") || local {
            return URL(string: (local ? "http://" : "https://") + text)
        }
        var search = URLComponents(string: "https://duckduckgo.com/")!
        search.queryItems = [URLQueryItem(name: "q", value: text)]
        return search.url
    }

    /// Waits for the page to finish loading, or the timeout; true if it did.
    @discardableResult
    func waitForLoad(timeout: Duration) async -> Bool {
        guard webView.isLoading else { return true }
        let timer = Task { [weak self] in
            try? await Task.sleep(for: timeout)
            guard !Task.isCancelled else { return }
            self?.releaseWaiters()
        }
        await withCheckedContinuation { loadWaiters.append($0) }
        timer.cancel()
        return !webView.isLoading
    }

    /// After an action: if it started a navigation, wait for that to land.
    func settleAfterAction(timeout: Duration = .seconds(10)) async {
        let before = navigationsStarted
        try? await Task.sleep(for: .milliseconds(150))
        if navigationsStarted != before || webView.isLoading { await waitForLoad(timeout: timeout) }
        // Two frames, so what it changed is on screen for a screenshot.
        _ = try? await untilDialog(timeout: .seconds(2)) { [webView] in
            _ = try await webView.callAsyncJavaScript("await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))", contentWorld: BrowserScripts.world)
        }
    }

    /// A page's scripts stop while an alert, confirm or prompt waits for an answer, so
    /// nothing may wait on them past one opening, or past `timeout`.
    func untilDialog(timeout: Duration, _ operation: @escaping @MainActor () async throws -> Void) async throws {
        if let dialog { throw BrowserError.waiting(on: dialog) }
        let outcome = Outcome()
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            outcome.continuation = continuation
            Task { @MainActor in
                do {
                    try await operation()
                    outcome.finish(nil)
                } catch {
                    outcome.finish(error)
                }
            }
            Task { @MainActor [weak self] in
                let deadline = ContinuousClock.now + timeout
                while ContinuousClock.now < deadline, !outcome.isDone {
                    if let dialog = self?.dialog { return outcome.finish(BrowserError.waiting(on: dialog)) }
                    try? await Task.sleep(for: .milliseconds(40))
                }
                outcome.finish(BrowserError("The page didn't respond within \(timeout.components.seconds) s: it may be busy or frozen."))
            }
        }
        if let error = outcome.error { throw error }
    }

    /// Why the last navigation failed, once.
    func takeNavigationError() -> String? {
        defer { lastNavigationError = nil }
        return lastNavigationError
    }

    private func loadingChanged(_ loading: Bool) {
        isLoading = loading
        if !loading { releaseWaiters() }
    }

    private func releaseWaiters() {
        let waiters = loadWaiters
        loadWaiters.removeAll()
        for waiter in waiters { waiter.resume() }
    }

    // MARK: - Logs

    func log(_ message: [String: Any]) {
        sequence += 1
        let date = Date()
        switch message["kind"] as? String {
        case "console":
            console.append(ConsoleEntry(id: sequence, date: date, level: message["level"] as? String ?? "log", text: message["text"] as? String ?? ""))
            if console.count > Self.logLimit { console.removeFirst(console.count - Self.logLimit) }
        case "request":
            network.append(NetworkEntry(
                id: sequence, date: date, method: message["method"] as? String ?? "GET", url: message["url"] as? String ?? "",
                status: (message["status"] as? NSNumber)?.intValue, type: message["type"] as? String ?? "other",
                duration: (message["ms"] as? NSNumber)?.doubleValue, failure: message["failure"] as? String))
            if network.count > Self.logLimit { network.removeFirst(network.count - Self.logLimit) }
        default:
            break
        }
    }

    func clearLogs() {
        console.removeAll()
        network.removeAll()
    }

    private func logDocument(_ url: URL?, status: Int?, failure: String?) {
        log(["kind": "request", "method": "GET", "url": url?.absoluteString ?? "", "type": "document",
             "status": status as Any, "failure": failure as Any])
    }

    // MARK: - Showing the agent at work

    func show(_ activity: String, at rect: CGRect? = nil) {
        agentActivity = activity
        if let rect { highlight = Highlight(rect: rect, label: activity) }
        highlightClear?.cancel()
        highlightClear = Task { [weak self] in
            try? await Task.sleep(for: .seconds(1.4))
            guard !Task.isCancelled else { return }
            self?.highlight = nil
            self?.agentActivity = nil
        }
    }

    // MARK: - Scripts

    /// Runs one of our helpers (`__yh.<name>`) in our own world.
    func helper(_ body: String, _ arguments: [String: Any] = [:], timeout: Duration = .seconds(65)) async throws -> Any? {
        let box = ResultBox()
        try await untilDialog(timeout: timeout) { [webView] in
            do {
                box.value = try await webView.callAsyncJavaScript(body, arguments: arguments, contentWorld: BrowserScripts.world)
            } catch {
                throw BrowserError(script: error)
            }
        }
        return box.value
    }

    /// The agent's own JavaScript, in the page's world.
    func evaluate(_ source: String) async throws -> String {
        // An expression, or a function body with its own `return`: told apart in our world,
        // since a syntax error in the page's would show up in its console.
        let isExpression = (try? await helper("return __yh.isExpression(s)", ["s": source], timeout: .seconds(5))) as? Bool ?? true
        let box = ResultBox()
        try await untilDialog(timeout: .seconds(30)) { [weak self] in
            box.value = try await self?.evaluateNow(source, isExpression: isExpression)
        }
        return box.value as? String ?? "undefined"
    }

    private func evaluateNow(_ source: String, isExpression: Bool) async throws -> String {
        // An expression first; if that isn't valid, a function body with its own `return`.
        // The wrapper's names are ones a page won't use: the code runs in their scope, so a
        // plain `seen` here hid the page's own `seen`.
        let wrapped = { (body: String) in
            """
            const __yhValue = await (async () => { \(body) })();
            const __yhSeen = new WeakSet();
            return JSON.stringify(__yhValue, (key, v) => {
              if (typeof Node !== 'undefined' && v instanceof Node) return v.nodeType === 1 ? v.outerHTML.slice(0, 500) : String(v.textContent).slice(0, 500);
              if (v instanceof Error) return String(v);
              if (typeof v === 'function') return 'ƒ ' + (v.name || 'anonymous');
              if (typeof v === 'bigint') return String(v);
              if (v && typeof v === 'object') { if (__yhSeen.has(v)) return '[circular]'; __yhSeen.add(v); }
              return v;
            }) ?? 'undefined';
            """
        }
        do {
            let body = isExpression ? "return (\n\(source)\n);" : source
            let result = try await webView.callAsyncJavaScript(wrapped(body), contentWorld: .page)
            return result as? String ?? "undefined"
        } catch {
            throw BrowserError(script: error)
        }
    }

    // MARK: - Input

    /// A click at a point in CSS pixels, as real mouse events.
    func click(at point: CGPoint, button: String = "left", count: Int = 1, modifiers: NSEvent.ModifierFlags = []) {
        withoutTakingFocus {
            let location = windowPoint(point)
            movePointer(to: location, modifiers: modifiers)
            for click in 1...max(count, 1) {
                switch button {
                case "right":
                    SyntheticMouse.holding(2) { webView.rightMouseDown(with: mouseEvent(.rightMouseDown, at: location, clicks: click, modifiers: modifiers)) }
                    SyntheticMouse.holding(0) { webView.rightMouseUp(with: mouseEvent(.rightMouseUp, at: location, clicks: click, modifiers: modifiers)) }
                default:
                    SyntheticMouse.holding(1) { webView.mouseDown(with: mouseEvent(.leftMouseDown, at: location, clicks: click, modifiers: modifiers)) }
                    SyntheticMouse.holding(0) { webView.mouseUp(with: mouseEvent(.leftMouseUp, at: location, clicks: click, modifiers: modifiers)) }
                }
            }
        }
    }

    func hover(at point: CGPoint) {
        movePointer(to: windowPoint(point))
    }

    /// The pointer to a window point, as the page sees a mouse move: `:hover`, mouseover,
    /// mousemove, no button down. WebKit hit-tests a plain move only in the key window (in
    /// any other it just updates scroll bars), and these pages are rarely in it: parked off
    /// screen, or in a window you've left. A move with the left button held is hit-tested
    /// anywhere, and the page still sees no button, since WebKit reads the buttons from the
    /// real mouse; with no mouse down before it, nothing is selected or dragged.
    private func movePointer(to location: NSPoint, modifiers: NSEvent.ModifierFlags = []) {
        SyntheticMouse.holding(0) {
            webView.mouseDragged(with: NSEvent.mouseEvent(
                with: .leftMouseDragged, location: location, modifierFlags: modifiers, timestamp: ProcessInfo.processInfo.systemUptime,
                windowNumber: webView.window?.windowNumber ?? 0, context: nil, eventNumber: 0, clickCount: 0, pressure: 0)!)
        }
    }

    enum DragOutcome {
        case mouse
        /// An HTML drag and drop: whether something under the pointer took the drop.
        case html(started: Bool, dropped: Bool, target: String)
    }

    /// A drag with the mouse, in CSS pixels. From a `draggable` element it's an HTML drag and
    /// drop, played as the page's drag events: WebKit hands that to an AppKit drag session,
    /// which follows the real mouse, so mouse events alone never drop anything.
    func drag(from start: CGPoint, to end: CGPoint) async -> DragOutcome {
        // In the page's world, so its handlers see this drag's dropEffect and effectAllowed.
        if let html = try? await webView.callAsyncJavaScript(Self.htmlDrag, arguments: ["from": [start.x, start.y], "to": [end.x, end.y]], contentWorld: .page) as? [String: Any] {
            movePointer(to: windowPoint(end))
            return .html(started: html["started"] as? Bool ?? false, dropped: html["dropped"] as? Bool ?? false, target: html["target"] as? String ?? "")
        }
        let from = windowPoint(start), to = windowPoint(end)
        withoutTakingFocus {
            movePointer(to: from)
            SyntheticMouse.holding(1) { webView.mouseDown(with: mouseEvent(.leftMouseDown, at: from)) }
        }
        for step in 1...12 {
            let t = Double(step) / 12
            let point = CGPoint(x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t)
            SyntheticMouse.holding(1) { webView.mouseDragged(with: mouseEvent(.leftMouseDragged, at: point)) }
            try? await Task.sleep(for: .milliseconds(16))
        }
        withoutTakingFocus { SyntheticMouse.holding(0) { webView.mouseUp(with: mouseEvent(.leftMouseUp, at: to)) } }
        return .mouse
    }

    /// Null unless the drag starts on a `draggable` element; then the events a browser fires
    /// for it, in order, and whether the drop was taken (the last dragover was cancelled).
    private static let htmlDrag = """
        const [x0, y0] = from, [x1, y1] = to;
        const source = document.elementFromPoint(x0, y0)?.closest('[draggable="true"]');
        if (!source) return null;
        const data = new DataTransfer();
        // A DataTransfer made by script ignores these outside a real drag; here they work as in one.
        let dropEffect = 'none', effectAllowed = 'uninitialized';
        Object.defineProperty(data, 'dropEffect', { configurable: true, get: () => dropEffect,
            set: (value) => { if (['none', 'copy', 'link', 'move'].includes(value)) dropEffect = value; } });
        Object.defineProperty(data, 'effectAllowed', { configurable: true, get: () => effectAllowed,
            set: (value) => { if (['none', 'copy', 'copyLink', 'copyMove', 'link', 'linkMove', 'move', 'all', 'uninitialized'].includes(value)) effectAllowed = value; } });
        const init = (x, y, extra) => ({ bubbles: true, cancelable: true, composed: true, view: window,
            clientX: x, clientY: y, screenX: screenX + x, screenY: screenY + y, ...extra });
        const fire = (target, type, x, y, extra = {}) => target.dispatchEvent(
            type.startsWith('pointer') ? new PointerEvent(type, init(x, y, { pointerId: 1, pointerType: 'mouse', isPrimary: true, ...extra }))
            : type.startsWith('mouse') ? new MouseEvent(type, init(x, y, extra))
            : new DragEvent(type, init(x, y, { dataTransfer: data, ...extra })));
        // What a browser starts each dragenter and dragover with, from effectAllowed; the page may change it.
        const effect = () => ({ none: 'none', copy: 'copy', copyLink: 'copy', copyMove: 'copy', all: 'copy', link: 'link', linkMove: 'link' })[data.effectAllowed] ?? 'move';
        const describe = (el) => el.id ? '#' + el.id : el.localName + (el.classList.length ? '.' + [...el.classList].join('.') : '');
        fire(source, 'pointerdown', x0, y0, { button: 0, buttons: 1 });
        fire(source, 'mousedown', x0, y0, { button: 0, buttons: 1 });
        if (!fire(source, 'dragstart', x0, y0)) return { started: false, dropped: false, target: describe(source) };
        fire(source, 'pointercancel', x0, y0);
        let over = null, accepted = false;
        for (let step = 1; step <= 12; step++) {
            const x = x0 + (x1 - x0) * step / 12, y = y0 + (y1 - y0) * step / 12;
            fire(source, 'drag', x, y);
            const under = document.elementFromPoint(x, y) ?? document.documentElement;
            if (under !== over) {
                data.dropEffect = effect();
                fire(under, 'dragenter', x, y);
                if (over) fire(over, 'dragleave', x, y);
                over = under;
            }
            data.dropEffect = effect();
            accepted = !fire(under, 'dragover', x, y) && data.dropEffect !== 'none';
            await new Promise((done) => setTimeout(done, 16));
        }
        if (accepted) fire(over, 'drop', x1, y1);
        // dragend says what happened: the drop's effect, or none.
        if (!accepted) data.dropEffect = 'none';
        fire(source, 'dragend', x1, y1);
        return { started: true, dropped: accepted, target: describe(over) };
        """

    /// Text into whatever has focus in the page, as input events (no key presses), so it
    /// doesn't need the keyboard focus you might be typing with.
    func insert(_ text: String) {
        webView.insertText(text)
    }

    /// A key press, with modifiers: real `keydown`/`keyup`.
    func press(_ key: BrowserKey) async {
        withoutTakingFocus {
            let down = keyEvent(.keyDown, key)
            if !key.modifiers.isEmpty, webView.performKeyEquivalent(with: down) { return }
            webView.keyDown(with: down)
            webView.keyUp(with: keyEvent(.keyUp, key))
        }
        // In a Mac app ⌘A, ⌘C, ⌘X and ⌘V are menu commands, which WebKit runs only for the
        // view with the keyboard focus, and that may be your composer. Done in the page
        // instead, with a clipboard of the browser's own: the agent never overwrites yours.
        switch key.editingShortcut {
        case "a":
            _ = try? await helper("__yh.selectAll()")
        case "c", "x":
            if let text = try? await helper("return __yh.selectedText()") as? String, !text.isEmpty {
                Self.clipboard = text
                if key.editingShortcut == "x" { insert("") }
            }
        case "v":
            if let text = Self.clipboard { insert(text) }
        default:
            break
        }
    }

    /// What ⌘C copied in a page, for ⌘V: the agent's, not the system clipboard.
    private static var clipboard: String?

    /// WebKit makes the page the window's first responder when it's clicked, which would
    /// pull the keyboard out of the composer you're typing in. The agent's input mustn't.
    private func withoutTakingFocus(_ body: () -> Void) {
        let alreadyFocused = webView.window?.firstResponder === webView
        if !alreadyFocused { WebKitSPI.suppressFirstResponderChanges(webView, true) }
        body()
        guard !alreadyFocused else { return }
        // Off again once the page has handled them, so your own clicks focus it as usual.
        Task { [weak self] in
            guard let self else { return }
            try? await self.untilDialog(timeout: .seconds(2)) { [webView] in _ = try await webView.evaluateJavaScript("0") }
            WebKitSPI.suppressFirstResponderChanges(self.webView, false)
        }
    }

    /// A point in CSS pixels where the window has it. A hundredth of a pixel in: WebKit
    /// truncates the page point it converts back, and scaled, 250 came back 249.9999, so
    /// clicks landed a pixel up and left.
    private func windowPoint(_ point: CGPoint) -> NSPoint {
        let x = point.x + 0.01, y = point.y + 0.01
        let local = NSPoint(x: x, y: webView.isFlipped ? y : webView.bounds.height - y)
        return webView.convert(local, to: nil)
    }

    private func mouseEvent(_ type: NSEvent.EventType, at location: NSPoint, clicks: Int = 1, modifiers: NSEvent.ModifierFlags = []) -> NSEvent {
        NSEvent.mouseEvent(
            with: type, location: location, modifierFlags: modifiers, timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: webView.window?.windowNumber ?? 0, context: nil, eventNumber: 0, clickCount: clicks,
            pressure: type == .leftMouseUp || type == .rightMouseUp || type == .mouseMoved ? 0 : 1)!
    }

    private func keyEvent(_ type: NSEvent.EventType, _ key: BrowserKey) -> NSEvent {
        NSEvent.keyEvent(
            with: type, location: .zero, modifierFlags: key.modifiers, timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: webView.window?.windowNumber ?? 0, context: nil, characters: key.characters,
            charactersIgnoringModifiers: key.charactersIgnoringModifiers, isARepeat: false, keyCode: key.keyCode)!
    }

    // MARK: - Viewport

    func resize(to viewport: Viewport) {
        self.viewport = viewport
        // Shown in a panel, the panel lays it out; parked, it keeps the zoom it had.
        if pageView.window === BrowserParking.shared.windowForChecks || pageView.superview == nil {
            let scale = displayScale
            place(CGRect(origin: pageView.frame.origin, size: CGSize(width: viewport.width * scale, height: viewport.height * scale)),
                  css: CGSize(width: viewport.width, height: viewport.height))
        } else {
            pageView.superview?.needsLayout = true
        }
    }

    /// What the page really has, CSS pixels: the panel may be fitting it.
    func cssViewport() async -> CGSize {
        webView.frame.size
    }

    /// The web view is `css` points, so a CSS pixel is a point, as in Safari, and the page
    /// view around it draws that scaled into `frame`. Page zoom would shrink the page too,
    /// but WebKit then holds text to its 9-point minimum on screen (at half size, every font
    /// under 18px came out at 18px) and the page sees a device pixel ratio of 1.
    func place(_ frame: CGRect, css: CGSize) {
        pageView.frame = frame
        if pageView.bounds.size != css { pageView.setBoundsSize(css) }
        if webView.frame != CGRect(origin: .zero, size: css) { webView.frame = CGRect(origin: .zero, size: css) }
    }

    /// Screen points per CSS pixel.
    var displayScale: Double {
        pageView.bounds.width > 0 ? pageView.frame.width / pageView.bounds.width : 1
    }

    /// Light or dark for the page's `prefers-color-scheme`; "auto" follows the app.
    func setColorScheme(_ scheme: String) {
        switch scheme {
        case "light": webView.appearance = NSAppearance(named: .aqua)
        case "dark": webView.appearance = NSAppearance(named: .darkAqua)
        default: webView.appearance = nil
        }
        colorSchemeName = webView.appearance == nil ? nil : scheme
    }

    private(set) var colorSchemeName: String?

    // MARK: - Picking

    func startPicking() {
        isPicking = true
        Task { _ = try? await helper("__yh.startPicking()") }
    }

    func stopPicking() {
        guard isPicking else { return }
        isPicking = false
        Task { _ = try? await helper("__yh.stopPicking()") }
    }

    func picked(_ info: [String: Any]?) {
        isPicking = false
        guard let info else { return onPick?(nil) ?? () }
        let rect = (info["rect"] as? [Double]).flatMap { $0.count == 4 ? CGRect(x: $0[0], y: $0[1], width: $0[2], height: $0[3]) : nil }
        Task {
            var image: Data?
            if let rect { image = try? await BrowserScreenshot.capture(self, cssRect: rect.insetBy(dx: -6, dy: -6), maxWidth: 1200) }
            onPick?(PickedElement(
                describe: info["describe"] as? String ?? "element", selector: info["selector"] as? String ?? "",
                html: info["html"] as? String ?? "", url: url?.absoluteString ?? "", image: image))
        }
    }

    fileprivate func newDocument() {
        documentGeneration += 1
    }

    func invalidateRefs() {
        documentGeneration += 1
        Task { _ = try? await helper("__yh.reset(start)", ["start": nextRef]) }
    }
}

/// What the element picker puts in the composer.
struct PickedElement: Equatable {
    let describe: String
    let selector: String
    let html: String
    let url: String
    let image: Data?

    /// How it reads in the message: enough for the agent to find it again.
    var promptText: String {
        """
        [Picked in the browser at \(url): \(describe), selector `\(selector)`]
        ```html
        \(html)
        ```
        """
    }
}

/// Settles once: whichever of the work, a dialog or the deadline comes first.
@MainActor
private final class Outcome {
    var continuation: CheckedContinuation<Void, Never>?
    private(set) var error: Error?
    private(set) var isDone = false

    func finish(_ error: Error?) {
        guard !isDone else { return }
        isDone = true
        self.error = error
        continuation?.resume()
        continuation = nil
    }
}

@MainActor
private final class ResultBox {
    var value: Any?
}

struct BrowserError: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = description
    }

    static func waiting(on dialog: BrowserTab.Dialog) -> BrowserError {
        BrowserError("The page is waiting on a \(dialog.kind.rawValue): \"\(dialog.message.prefix(200))\". Answer it with dialog first.")
    }

    /// A script's exception as the agent should read it.
    init(script error: Error) {
        let nsError = error as NSError
        var message = nsError.userInfo["WKJavaScriptExceptionMessage"] as? String ?? nsError.localizedDescription
        if message.hasPrefix("Error: ") { message.removeFirst("Error: ".count) }
        if message.hasPrefix("stale:") {
            message = "Ref \(message.dropFirst("stale:".count)) is from an older snapshot: the page changed. Take a fresh snapshot."
        } else if message.contains("__yh") {
            message = "The page isn't ready for that yet (it's loading, or it's not a web page)."
        }
        description = message
    }
}

// MARK: - Delegates

extension BrowserTab: WKNavigationDelegate, WKUIDelegate {
    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        navigationsStarted += 1
        lastNavigationError = nil
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        newDocument()
        Task { _ = try? await helper("__yh.reset(start)", ["start": nextRef]) }
    }

    func webView(_ webView: WKWebView, decidePolicyFor navigationResponse: WKNavigationResponse) async -> WKNavigationResponsePolicy {
        if navigationResponse.isForMainFrame {
            logDocument(navigationResponse.response.url, status: (navigationResponse.response as? HTTPURLResponse)?.statusCode, failure: nil)
        }
        return navigationResponse.canShowMIMEType ? .allow : .download
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        navigationFailed(error)
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        navigationFailed(error)
    }

    private func navigationFailed(_ error: Error) {
        let nsError = error as NSError
        // Replaced by another navigation: not a failure.
        guard nsError.code != NSURLErrorCancelled else { return }
        lastNavigationError = nsError.localizedDescription
        logDocument(nsError.userInfo[NSURLErrorFailingURLErrorKey] as? URL, status: nil, failure: nsError.localizedDescription)
        log(["kind": "console", "level": "error", "text": "Navigation failed: \(nsError.localizedDescription)"])
    }

    func webView(_ webView: WKWebView, navigationResponse: WKNavigationResponse, didBecome download: WKDownload) {
        log(["kind": "console", "level": "info", "text": "Download of \(navigationResponse.response.url?.absoluteString ?? "a file") skipped: the browser doesn't download files."])
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        log(["kind": "console", "level": "error", "text": "The page crashed (its web content process ended). Reloading."])
        webView.reload()
    }

    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        onOpenWindow?(configuration, navigationAction.request)
    }

    func webViewDidClose(_ webView: WKWebView) {
        onClose?()
    }

    func webView(_ webView: WKWebView, runJavaScriptAlertPanelWithMessage message: String,
                 initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping @MainActor () -> Void) {
        dialog = Dialog(kind: .alert, message: message, defaultText: nil) { _, _ in completionHandler() }
    }

    func webView(_ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String,
                 initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping @MainActor (Bool) -> Void) {
        dialog = Dialog(kind: .confirm, message: message, defaultText: nil) { accept, _ in completionHandler(accept) }
    }

    func webView(_ webView: WKWebView, runJavaScriptTextInputPanelWithPrompt prompt: String, defaultText: String?,
                 initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping @MainActor (String?) -> Void) {
        dialog = Dialog(kind: .prompt, message: prompt, defaultText: defaultText) { accept, text in completionHandler(accept ? text ?? defaultText ?? "" : nil) }
    }

    func webView(_ webView: WKWebView, runOpenPanelWith parameters: WKOpenPanelParameters,
                 initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping @MainActor ([URL]?) -> Void) {
        if let files = pendingUpload {
            pendingUpload = nil
            completionHandler(parameters.allowsMultipleSelection ? files : Array(files.prefix(1)))
            return
        }
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = parameters.allowsMultipleSelection
        panel.canChooseDirectories = parameters.allowsDirectories
        panel.canChooseFiles = true
        if let window = webView.window, window.isVisible, window.frame.intersects(NSScreen.main?.frame ?? .zero) {
            panel.beginSheetModal(for: window) { response in completionHandler(response == .OK ? panel.urls : nil) }
        } else {
            completionHandler(panel.runModal() == .OK ? panel.urls : nil)
        }
    }
}

/// Messages from the page: console lines and requests from the page's world, picks from ours.
final class BrowserMessages: NSObject, WKScriptMessageHandler {
    weak var tab: BrowserTab?

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        MainActor.assumeIsolated {
            guard let tab else { return }
            if message.name == BrowserScripts.logHandler, let body = message.body as? [String: Any] {
                tab.log(body)
            } else if message.name == "yourHarnessPick" {
                tab.picked(message.body as? [String: Any])
            }
        }
    }
}

/// WebKit switches with no public API, each looked up first so a WebKit without one
/// just behaves as before.
enum WebKitSPI {
    /// Off, a page in a window that's hidden behind others, or off screen, isn't treated as
    /// hidden: its timers and animations run at full speed while the agent works.
    static func setOcclusionDetection(_ webView: WKWebView, enabled: Bool) {
        call(webView, "_setWindowOcclusionDetectionEnabled:", enabled)
    }

    static func suppressFirstResponderChanges(_ webView: WKWebView, _ suppress: Bool) {
        call(webView, "_setShouldSuppressFirstResponderChanges:", suppress)
    }

    /// Lays the page out at `size` whatever the view's size, or at the view's size again
    /// (nil). Viewport units and media queries follow the layout size.
    static func fixLayoutSize(_ webView: WKWebView, to size: CGSize?) {
        let setSize = NSSelectorFromString("_setFixedLayoutSize:"), setMode = NSSelectorFromString("_setLayoutMode:")
        guard webView.responds(to: setSize), webView.responds(to: setMode) else { return }
        if let size {
            typealias SizeSetter = @convention(c) (AnyObject, Selector, CGSize) -> Void
            unsafeBitCast(webView.method(for: setSize), to: SizeSetter.self)(webView, setSize, size)
        }
        // _WKLayoutModeViewSize, _WKLayoutModeFixedSize.
        typealias ModeSetter = @convention(c) (AnyObject, Selector, Int) -> Void
        unsafeBitCast(webView.method(for: setMode), to: ModeSetter.self)(webView, setMode, size == nil ? 0 : 1)
    }

    private static func call(_ object: NSObject, _ name: String, _ value: Bool) {
        let selector = NSSelectorFromString(name)
        guard object.responds(to: selector) else { return }
        typealias Setter = @convention(c) (AnyObject, Selector, Bool) -> Void
        unsafeBitCast(object.method(for: selector), to: Setter.self)(object, selector, value)
    }
}
