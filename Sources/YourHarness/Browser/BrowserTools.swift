import AppKit
import HarnessCore
import WebKit

/// The browser as MCP tools (`mcp__browser__*`), served from inside the app over the
/// CLI's control channel. Sendable glue: the work happens on the main actor.
struct BrowserToolServer: MCPToolServer {
    let name = "browser"
    let tools: [MCPTool]
    let browser: BrowserTools

    @MainActor init(browser: BrowserTools) {
        self.browser = browser
        tools = browser.readOnly ? BrowserTools.definitions.filter(\.readOnly) : BrowserTools.definitions
    }

    func call(_ tool: String, arguments: JSONValue, toolUseID: String?) async -> MCPToolResult {
        await browser.call(tool, arguments, toolUseID: toolUseID)
    }
}

@MainActor
final class BrowserTools {
    /// The thread whose browser this drives. A side chat looks at its thread's browser.
    private weak var thread: ThreadModel?
    private unowned let app: AppModel
    /// The side chat's: it can look, not act, and leaves the agent's bookkeeping alone.
    let readOnly: Bool

    /// Big tool results get moved to a file by Claude Code, and the refs with them.
    static let outlineBudget = 18_000

    init(thread: ThreadModel, app: AppModel, readOnly: Bool) {
        self.thread = thread
        self.app = app
        self.readOnly = readOnly
    }

    func call(_ tool: String, _ args: JSONValue, toolUseID: String?) async -> MCPToolResult {
        guard let thread else { return .error("This thread is gone.") }
        if readOnly, !(Self.definitions.first { $0.name == tool }?.readOnly ?? false) {
            return .error("The side chat can look at the browser, not use it.")
        }
        let session: BrowserSession
        if readOnly {
            guard let existing = thread.browser, !existing.tabs.isEmpty else {
                return .error("The thread's browser has no pages open.")
            }
            session = existing
        } else {
            session = thread.ensureBrowser()
            session.restoreIfNeeded()
            session.agentUsed()
        }
        do {
            return try await perform(tool, args, session: session, toolUseID: toolUseID)
        } catch let error as BrowserError {
            return .error(error.description)
        } catch {
            return .error(error.localizedDescription)
        }
    }

    // MARK: - Tools

    private func perform(_ tool: String, _ args: JSONValue, session: BrowserSession, toolUseID: String?) async throws -> MCPToolResult {
        switch tool {
        case "status":
            return .text(status(session))

        case "open":
            try checkControl(session)
            guard let address = args["url"]?.string, let url = BrowserTab.url(from: address) else { throw BrowserError("Give a url to open.") }
            let tab: BrowserTab
            if args["new_tab"]?.bool != true, let current = session.selectedTab {
                tab = current
                tab.load(url.absoluteString)
            } else {
                guard session.tabs.count < BrowserSession.maxTabs else {
                    throw BrowserError("\(BrowserSession.maxTabs) tabs are open, the most there can be. Close one first.")
                }
                tab = session.newTab(url: url.absoluteString, byAgent: true)
            }
            session.selectedTabID = tab.id
            if args["show"]?.bool != false { app.revealBrowser(for: thread) }
            try await waitForNavigation(tab, args)
            return result(tab, session, "Opened \(tab.shortURL) in tab \(tab.id)", showPage: true)

        case "navigate":
            try checkControl(session)
            let tab = try page(session, args)
            try checkDialog(tab)
            let before = tab.url
            if let address = args["url"]?.string {
                tab.load(address)
            } else {
                switch args["action"]?.string {
                case "back":
                    guard tab.webView.canGoBack else { throw BrowserError("There's no page to go back to.") }
                    tab.webView.goBack()
                case "forward":
                    guard tab.webView.canGoForward else { throw BrowserError("There's no page to go forward to.") }
                    tab.webView.goForward()
                case "reload":
                    tab.webView.reload()
                default:
                    throw BrowserError("Give a url, or an action: back, forward or reload.")
                }
            }
            try await waitForNavigation(tab, args)
            let verb = args["action"]?.string.map { $0 == "reload" ? "Reloaded" : "Went \($0)" } ?? "Went"
            return result(tab, session, "\(verb) to \(tab.shortURL)", showPage: before != tab.url || args["action"]?.string == "reload")

        case "snapshot":
            let tab = try page(session, args)
            return .text(try await snapshot(tab, session, selector: args["selector"]?.string))

        case "screenshot":
            let tab = try page(session, args)
            var rect: CGRect?
            if Self.hasTarget(args) {
                let point = try await locate(tab, args)
                rect = point.rect.insetBy(dx: -8, dy: -8)
            }
            let data: Data
            if args["full_page"]?.bool == true, rect == nil {
                data = try await fullPage(tab)
            } else {
                data = try await BrowserScreenshot.capture(tab, cssRect: rect)
            }
            if let toolUseID, !readOnly { app.saveBrowserScreenshot(data, thread: thread?.id, toolUseID: toolUseID) }
            let size = NSBitmapImageRep(data: data).map { "\($0.pixelsWide)×\($0.pixelsHigh)" } ?? ""
            var text = "Screenshot of tab \(tab.id), \(tab.shortURL) (\(size))."
            if let note = news(tab) { text += "\n" + note }
            return MCPToolResult([.image(data, mimeType: "image/jpeg"), .text(text)])

        case "click":
            try checkControl(session)
            let tab = try page(session, args)
            try checkDialog(tab)
            let button = args["button"]?.string ?? "left"
            let count = max(1, min(args["count"]?.int ?? 1, 3))
            let modifiers = Self.modifiers(args["modifiers"])
            let described: String
            if let x = args["x"]?.double, let y = args["y"]?.double, !Self.hasTarget(args) {
                tab.show("Click", at: CGRect(x: x - 12, y: y - 12, width: 24, height: 24))
                tab.click(at: CGPoint(x: x, y: y), button: button, count: count, modifiers: modifiers)
                described = "at \(Int(x)),\(Int(y))"
            } else {
                let point = try await locate(tab, args)
                if let blocker = point.blockedBy, args["force"]?.bool != true {
                    throw BrowserError("\(point.describe) is covered by \(blocker) there. Close or scroll that away first, or pass force: true to click it with a script.")
                }
                tab.show("Click", at: point.rect)
                if point.blockedBy != nil {
                    _ = try await tab.helper("return __yh.click(t)", ["t": Self.target(args)])
                } else {
                    tab.click(at: point.point, button: button, count: count, modifiers: modifiers)
                }
                described = point.describe
            }
            await tab.settleAfterAction()
            let verb = count == 2 ? "Double-clicked" : button == "right" ? "Right-clicked" : "Clicked"
            return result(tab, session, "\(verb) \(described)")

        case "hover":
            try checkControl(session)
            let tab = try page(session, args)
            let point = try await locate(tab, args)
            tab.show("Hover", at: point.rect)
            tab.hover(at: point.point)
            await tab.settleAfterAction(timeout: .seconds(2))
            return result(tab, session, "Hovered over \(point.describe)")

        case "type":
            try checkControl(session)
            let tab = try page(session, args)
            try checkDialog(tab)
            guard let text = args["text"]?.string else { throw BrowserError("Give the text to type.") }
            var described = "the focused element"
            if Self.hasTarget(args) {
                let point = try await locate(tab, args)
                tab.show("Type", at: point.rect)
                described = point.describe
            }
            described = try await tab.helper("return __yh.prepareTyping(t, c)", ["t": Self.target(args), "c": args["clear"]?.bool ?? true]) as? String ?? described
            if args["clear"]?.bool ?? true, text.isEmpty {
                _ = try await tab.helper("__yh.clear(t)", ["t": Self.target(args)])
            } else {
                tab.insert(text)
            }
            if args["submit"]?.bool == true, let enter = BrowserKey("Enter") { await tab.press(enter) }
            await tab.settleAfterAction()
            // A password isn't repeated back: only its length, and the transcript masks it.
            let secret = (try? await tab.helper("return __yh.isSecret(t)", ["t": Self.target(args)]) as? Bool) ?? false
            var line = secret ? "Typed \(text.count) characters into \(described)" : "Typed \(Self.quoted(text, limit: 60)) into \(described)"
            if !secret, let value = try? await tab.helper("return __yh.value(t)", ["t": Self.target(args)]) as? String, value != text, !value.isEmpty {
                line += "; it now holds \(Self.quoted(value, limit: 80))"
            }
            if args["submit"]?.bool == true { line += ", and pressed Enter" }
            return result(tab, session, line)

        case "press":
            try checkControl(session)
            let tab = try page(session, args)
            try checkDialog(tab)
            guard let spec = args["key"]?.string, let key = BrowserKey(spec) else {
                throw BrowserError("Unknown key. Use names like Enter, Tab, Escape, Backspace, ArrowDown, PageDown, a, or combinations like Shift+Tab and Meta+A.")
            }
            if Self.hasTarget(args) {
                let point = try await locate(tab, args)
                _ = try await tab.helper("__yh.prepareTyping(t, false)", ["t": Self.target(args)])
                tab.show("Press \(spec)", at: point.rect)
            } else {
                tab.show("Press \(spec)")
            }
            let count = max(1, min(args["count"]?.int ?? 1, 50))
            for _ in 0..<count { await tab.press(key) }
            await tab.settleAfterAction()
            return result(tab, session, "Pressed \(spec)\(count > 1 ? " ×\(count)" : "")")

        case "select":
            try checkControl(session)
            let tab = try page(session, args)
            let values = args["values"]?.array?.compactMap(\.string) ?? args["value"]?.string.map { [$0] } ?? []
            guard !values.isEmpty else { throw BrowserError("Give the option to select, as values.") }
            let point = try await locate(tab, args)
            tab.show("Select", at: point.rect)
            let picked = try await tab.helper("return __yh.select(t, v)", ["t": Self.target(args), "v": values]) as? [String] ?? values
            await tab.settleAfterAction()
            return result(tab, session, "Selected \(picked.map { Self.quoted($0, limit: 40) }.joined(separator: ", ")) in \(point.describe)")

        case "scroll":
            try checkControl(session)
            let tab = try page(session, args)
            var options: [String: Any] = [:]
            if Self.hasTarget(args) { options["target"] = Self.target(args) }
            if let direction = args["direction"]?.string { options["direction"] = direction }
            if let amount = args["amount"]?.double { options["amount"] = amount }
            if let to = args["to"]?.string { options["to"] = to }
            let line = try await tab.helper("return __yh.scroll(o)", ["o": options]) as? String ?? "Scrolled"
            tab.show("Scroll")
            await tab.settleAfterAction(timeout: .seconds(2))
            return result(tab, session, line)

        case "drag":
            try checkControl(session)
            let tab = try page(session, args)
            guard let from = args["from"], let to = args["to"] else { throw BrowserError("Give from and to, each a ref or selector.") }
            let start = try await locate(tab, from), end = try await locate(tab, to)
            tab.show("Drag", at: start.rect)
            await tab.drag(from: start.point, to: end.point)
            await tab.settleAfterAction()
            return result(tab, session, "Dragged \(start.describe) to \(end.describe)")

        case "wait_for":
            let tab = try page(session, args)
            let timeout = min(max(args["timeout"]?.double ?? 10, 0.1), 60)
            let started = Date()
            if let fragment = args["url"]?.string {
                while !(tab.url?.absoluteString.contains(fragment) ?? false) {
                    guard Date().timeIntervalSince(started) < timeout else {
                        throw BrowserError("Timed out after \(Int(timeout)) s waiting for the URL to contain \(fragment); it's \(tab.shortURL).")
                    }
                    try await Task.sleep(for: .milliseconds(100))
                }
                await tab.waitForLoad(timeout: .seconds(max(1, timeout - Date().timeIntervalSince(started))))
            }
            var options: [String: Any] = ["timeout": (timeout - Date().timeIntervalSince(started)) * 1000]
            for key in ["text", "selector", "gone"] { if let value = args[key]?.string { options[key] = value } }
            if options.count > 1 { _ = try await tab.helper("return await __yh.waitFor(o)", ["o": options]) }
            let waited = String(format: "%.1f", Date().timeIntervalSince(started))
            return result(tab, session, "Found it after \(waited) s")

        case "console":
            let tab = try page(session, args)
            return .text(consoleLog(tab, args))

        case "network":
            let tab = try page(session, args)
            return .text(networkLog(tab, args))

        case "evaluate":
            try checkControl(session)
            let tab = try page(session, args)
            try checkDialog(tab)
            guard let source = args["expression"]?.string else { throw BrowserError("Give the JavaScript to run, as expression.") }
            tab.show("Run JavaScript")
            var value = try await tab.evaluate(source)
            if value.count > 20_000 { value = String(value.prefix(20_000)) + "… (\(value.count - 20_000) more characters; return less)" }
            var text = value
            if let note = news(tab) { text += "\n\n" + note }
            return .text(text)

        case "resize":
            try checkControl(session)
            let tab = try page(session, args)
            var viewport = tab.viewport
            switch args["preset"]?.string {
            case "laptop": viewport = .laptop
            case "desktop": viewport = .desktop
            case "tablet": viewport = .tablet
            case "phone": viewport = .phone
            case "fit": viewport.fitsPanel = true
            default: break
            }
            if let width = args["width"]?.double { viewport.width = min(max(width, 200), 3840); viewport.fitsPanel = false }
            if let height = args["height"]?.double { viewport.height = min(max(height, 200), 2400); viewport.fitsPanel = false }
            tab.resize(to: viewport)
            if let scheme = args["color_scheme"]?.string { tab.setColorScheme(scheme) }
            await tab.settleAfterAction(timeout: .seconds(2))
            let size = await tab.cssViewport()
            return result(tab, session, "Viewport is \(Int(size.width))×\(Int(size.height))\(tab.colorSchemeName.map { ", \($0)" } ?? "")")

        case "dialog":
            try checkControl(session)
            let tab = try page(session, args)
            guard let dialog = tab.dialog else { throw BrowserError("No dialog is open.") }
            let accept = args["accept"]?.bool ?? true
            tab.dialog = nil
            dialog.respond(accept, args["text"]?.string)
            await tab.settleAfterAction()
            return result(tab, session, "\(accept ? "Accepted" : "Dismissed") the \(dialog.kind.rawValue) \(Self.quoted(dialog.message, limit: 80))")

        case "upload":
            try checkControl(session)
            let tab = try page(session, args)
            let paths = args["paths"]?.array?.compactMap(\.string) ?? []
            guard !paths.isEmpty else { throw BrowserError("Give absolute file paths, as paths.") }
            let files = paths.map { URL(fileURLWithPath: ($0 as NSString).expandingTildeInPath) }
            if let missing = files.first(where: { !FileManager.default.fileExists(atPath: $0.path) }) {
                throw BrowserError("No file at \(missing.path).")
            }
            tab.pendingUpload = files
            let point = try await locate(tab, args)
            tab.show("Upload", at: point.rect)
            tab.click(at: point.point)
            await tab.settleAfterAction()
            if tab.pendingUpload != nil {
                tab.pendingUpload = nil
                throw BrowserError("Clicking \(point.describe) didn't open a file chooser. Target the <input type=file> or the button that opens it.")
            }
            return result(tab, session, "Uploaded \(files.map(\.lastPathComponent).joined(separator: ", ")) with \(point.describe)")

        case "close":
            try checkControl(session)
            let tab = try page(session, args)
            session.close(tab)
            return .text("Closed tab \(tab.id). " + (session.tabs.isEmpty ? "No tabs are open." : "Tab \(session.selectedTab?.id ?? 0) is current."))

        default:
            throw BrowserError("Unknown tool \(tool).")
        }
    }

    // MARK: - Pieces

    private func page(_ session: BrowserSession, _ args: JSONValue) throws -> BrowserTab {
        if let id = args["tab"]?.int {
            guard let tab = session.tab(id) else { throw BrowserError("There's no tab \(id). Open tabs: \(session.tabs.map { String($0.id) }.joined(separator: ", ")).") }
            return tab
        }
        guard let tab = session.selectedTab else { throw BrowserError("No page is open. Use open with a URL first.") }
        return tab
    }

    private func checkControl(_ session: BrowserSession) throws {
        if session.userHasControl {
            throw BrowserError("The user has taken control of the browser. You can still look (status, snapshot, screenshot, console, network); wait for them to give it back before acting.")
        }
    }

    private func checkDialog(_ tab: BrowserTab) throws {
        if let dialog = tab.dialog {
            throw BrowserError("The page is waiting on a \(dialog.kind.rawValue): \(Self.quoted(dialog.message, limit: 200)). Answer it with dialog first.")
        }
    }

    private func waitForNavigation(_ tab: BrowserTab, _ args: JSONValue) async throws {
        let until = args["wait_until"]?.string ?? "load"
        guard until != "none" else { return }
        try? await Task.sleep(for: .milliseconds(50))
        let finished = await tab.waitForLoad(timeout: .seconds(min(max(args["timeout"]?.double ?? 30, 1), 60)))
        if let error = tab.takeNavigationError() { throw BrowserError("Couldn't load \(tab.shortURL): \(error)") }
        if !finished { tab.log(["kind": "console", "level": "info", "text": "Still loading after the timeout."]) }
        await tab.settleAfterAction(timeout: .seconds(2))
    }

    private struct Located {
        let point: CGPoint
        let rect: CGRect
        let describe: String
        let blockedBy: String?
    }

    private func locate(_ tab: BrowserTab, _ args: JSONValue) async throws -> Located {
        guard Self.hasTarget(args) else { throw BrowserError("Say which element: a ref from the latest snapshot, a CSS selector, or its text.") }
        let info = try await tab.helper("return __yh.point(t)", ["t": Self.target(args)]) as? [String: Any] ?? [:]
        let rect = (info["rect"] as? [Double]).map { CGRect(x: $0[0], y: $0[1], width: $0[2], height: $0[3]) } ?? .zero
        return Located(point: CGPoint(x: info["x"] as? Double ?? rect.midX, y: info["y"] as? Double ?? rect.midY), rect: rect,
                       describe: info["describe"] as? String ?? "the element", blockedBy: info["blockedBy"] as? String)
    }

    private static func hasTarget(_ args: JSONValue) -> Bool {
        args["ref"]?.string != nil || args["selector"]?.string != nil || args["label"]?.string != nil
    }

    private static func target(_ args: JSONValue) -> [String: Any] {
        var target: [String: Any] = [:]
        if let ref = args["ref"]?.string { target["ref"] = ref }
        if let selector = args["selector"]?.string { target["selector"] = selector }
        if let label = args["label"]?.string { target["text"] = label }
        return target
    }

    private static func modifiers(_ value: JSONValue?) -> NSEvent.ModifierFlags {
        var flags: NSEvent.ModifierFlags = []
        for name in value?.array?.compactMap(\.string) ?? [] {
            switch name.lowercased() {
            case "meta", "cmd", "command": flags.insert(.command)
            case "shift": flags.insert(.shift)
            case "alt", "option": flags.insert(.option)
            case "control", "ctrl": flags.insert(.control)
            default: break
            }
        }
        return flags
    }

    /// What every action says: what it did, where the page is if that changed, and what
    /// went wrong in the page since the agent last heard.
    private func result(_ tab: BrowserTab, _ session: BrowserSession, _ line: String, showPage: Bool = false) -> MCPToolResult {
        tab.isRestored = false
        var lines = [line + "."]
        if showPage || tab.url != lastURL[tab.id] || tab.title != lastTitle[tab.id] {
            lines.append("Page: \(tab.shortURL)\(tab.title.isEmpty ? "" : " — \(Self.quoted(tab.title, limit: 80))")\(tab.isLoading ? " (still loading)" : "")")
        }
        lastURL[tab.id] = tab.url
        lastTitle[tab.id] = tab.title
        if let dialog = tab.dialog {
            lines.append("The page opened a \(dialog.kind.rawValue): \(Self.quoted(dialog.message, limit: 200)). Answer it with dialog.")
        }
        if let note = news(tab) { lines.append(note) }
        return .text(lines.joined(separator: "\n"))
    }

    private var lastURL: [Int: URL?] = [:]
    private var lastTitle: [Int: String] = [:]

    /// New console errors and failed requests since the agent last heard about them.
    private func news(_ tab: BrowserTab) -> String? {
        guard !readOnly else { return nil }
        let errors = tab.console.filter { $0.id > tab.reportedConsole && $0.isError }
        let failed = tab.network.filter { $0.id > tab.reportedNetwork && $0.failed }
        tab.reportedConsole = max(tab.reportedConsole, tab.console.last?.id ?? 0)
        tab.reportedNetwork = max(tab.reportedNetwork, tab.network.last?.id ?? 0)
        guard !errors.isEmpty || !failed.isEmpty else { return nil }
        var parts: [String] = []
        if !errors.isEmpty { parts.append("\(errors.count) console error\(errors.count == 1 ? "" : "s")") }
        if !failed.isEmpty { parts.append("\(failed.count) failed request\(failed.count == 1 ? "" : "s")") }
        var lines = ["New in the page: \(parts.joined(separator: ", ")) (see console, network)."]
        for error in errors.prefix(2) { lines.append("  " + Self.oneLine(error.text, limit: 240)) }
        for request in failed.prefix(2) { lines.append("  \(request.method) \(Self.oneLine(request.url, limit: 160)) → \(request.status.map(String.init) ?? request.failure ?? "failed")") }
        return lines.joined(separator: "\n")
    }

    private func status(_ session: BrowserSession) -> String {
        guard !session.tabs.isEmpty else { return "No pages are open. Use open with a URL." }
        var lines = ["Tabs:"]
        for tab in session.tabs {
            let current = tab.id == session.selectedTab?.id ? "*" : " "
            var line = "\(current) \(tab.id)  \(tab.shortURL)"
            if !tab.title.isEmpty { line += " — \(Self.quoted(tab.title, limit: 80))" }
            if tab.isLoading { line += " (loading \(Int(tab.progress * 100))%)" }
            if tab.isRestored { line += " (reopened from last session)" }
            lines.append(line)
            if let dialog = tab.dialog { lines.append("    waiting on a \(dialog.kind.rawValue): \(Self.quoted(dialog.message, limit: 120))") }
        }
        if let tab = session.selectedTab {
            let viewport = tab.viewport.fitsPanel ? "fits the panel" : "\(Int(tab.viewport.width))×\(Int(tab.viewport.height))"
            lines.append("Viewport: \(viewport)\(tab.colorSchemeName.map { ", \($0)" } ?? "").")
            let errors = tab.console.count { $0.isError }, failed = tab.network.count { $0.failed }
            lines.append("Logged: \(Self.count(tab.console.count, "console message")) (\(Self.count(errors, "error"))), \(Self.count(tab.network.count, "request")) (\(failed) failed).")
        }
        lines.append(session.userHasControl
            ? "The user has taken control: you can look, but actions fail until they give it back."
            : "You can act; the user watches in the app and may take over.")
        return lines.joined(separator: "\n")
    }

    private func snapshot(_ tab: BrowserTab, _ session: BrowserSession, selector: String?) async throws -> String {
        if tab.isLoading { await tab.waitForLoad(timeout: .seconds(10)) }
        var options: [String: Any] = ["budget": Self.outlineBudget, "keepRefs": readOnly]
        if let selector { options["selector"] = selector }
        guard let info = try await tab.helper("return __yh.snapshot(o)", ["o": options]) as? [String: Any] else {
            throw BrowserError("Couldn't read the page.")
        }
        if let next = info["nextRef"] as? Int { tab.nextRef = max(tab.nextRef, next) }
        let viewport = info["viewport"] as? [Int] ?? [], scroll = info["scroll"] as? [Int] ?? [], size = info["size"] as? [Int] ?? []
        var lines = ["Tab \(tab.id): \(tab.shortURL)\((info["title"] as? String).map { $0.isEmpty ? "" : " — \(Self.quoted($0, limit: 100))" } ?? "")"]
        if viewport.count == 2, scroll.count == 2, size.count == 2 {
            lines.append("Viewport \(viewport[0])×\(viewport[1]), scrolled to \(scroll[0]),\(scroll[1]) of \(size[0])×\(size[1]).")
        }
        if let dialog = tab.dialog { lines.append("A \(dialog.kind.rawValue) is open: \(Self.quoted(dialog.message, limit: 200)). Answer it with dialog.") }
        if session.userHasControl { lines.append("The user has control: you can read, not act.") }
        lines.append("")
        let outline = info["outline"] as? String ?? ""
        lines.append(outline.isEmpty ? "(The page shows nothing readable.)" : outline)
        if let dropped = info["dropped"] as? Int, dropped > 0 {
            lines.append("")
            lines.append("(\(dropped) lines left out to keep this short, text and off-screen parts first. Snapshot a part with selector, or scroll and snapshot again.)")
        }
        if let note = news(tab) { lines.append(""); lines.append(note) }
        return lines.joined(separator: "\n")
    }

    /// The whole page: the view grows to the page's height for the capture, then back.
    private func fullPage(_ tab: BrowserTab) async throws -> Data {
        let height = try await tab.helper("return Math.min(document.documentElement.scrollHeight, 12000)") as? Double ?? 0
        let viewport = await tab.cssViewport()
        guard height > viewport.height + 1 else { return try await BrowserScreenshot.capture(tab) }
        let web = tab.webView
        let original = tab.pageView.frame, originalCSS = web.frame.size
        let scroll = try await tab.helper("const s = document.scrollingElement; const y = s.scrollTop; s.scrollTop = 0; return y") as? Double ?? 0
        tab.holdsLayout = true
        tab.place(CGRect(origin: original.origin, size: CGSize(width: original.width, height: height * tab.displayScale)),
                  css: CGSize(width: originalCSS.width, height: height))
        defer {
            tab.holdsLayout = false
            tab.place(original, css: originalCSS)
            Task { _ = try? await tab.helper("document.scrollingElement.scrollTop = y", ["y": scroll]) }
        }
        try? await Task.sleep(for: .milliseconds(120))
        await tab.settleAfterAction(timeout: .seconds(1))
        return try await BrowserScreenshot.capture(tab, cssRect: CGRect(x: 0, y: 0, width: viewport.width, height: height), maxWidth: min(viewport.width, 1280))
    }

    private func consoleLog(_ tab: BrowserTab, _ args: JSONValue) -> String {
        let level = args["level"]?.string ?? "all"
        let all = args["all"]?.bool == true || readOnly
        let limit = min(max(args["limit"]?.int ?? 100, 1), 500)
        var entries = tab.console.filter { all || $0.id > consoleRead[tab.id, default: 0] }
        switch level {
        case "error": entries = entries.filter(\.isError)
        case "warning": entries = entries.filter { $0.isError || $0.level == "warn" }
        default: break
        }
        if !readOnly { consoleRead[tab.id] = tab.console.last?.id ?? 0 }
        guard !entries.isEmpty else { return all ? "The console is empty." : "Nothing new in the console since you last looked." }
        let shown = entries.suffix(limit)
        var lines = shown.map { "[\($0.level)] \(Self.clip($0.text, limit: 2000))" }
        if entries.count > shown.count { lines.insert("(\(entries.count - shown.count) older messages not shown)", at: 0) }
        return lines.joined(separator: "\n")
    }

    private func networkLog(_ tab: BrowserTab, _ args: JSONValue) -> String {
        let all = args["all"]?.bool == true || readOnly
        let limit = min(max(args["limit"]?.int ?? 100, 1), 500)
        var entries = tab.network.filter { all || $0.id > networkRead[tab.id, default: 0] }
        if args["failed_only"]?.bool == true { entries = entries.filter(\.failed) }
        if let filter = args["filter"]?.string, !filter.isEmpty { entries = entries.filter { $0.url.localizedCaseInsensitiveContains(filter) } }
        if !readOnly { networkRead[tab.id] = tab.network.last?.id ?? 0 }
        guard !entries.isEmpty else { return all ? "No requests logged." : "No new requests since you last looked." }
        let shown = entries.suffix(limit)
        var lines = shown.map { entry in
            var line = "\(entry.method) \(Self.clip(entry.url, limit: 300)) → \(entry.status.map(String.init) ?? entry.failure ?? "…")"
            line += " [\(entry.type)]"
            if let ms = entry.duration { line += " \(Int(ms)) ms" }
            if let failure = entry.failure, entry.status != nil { line += " \(failure)" }
            return line
        }
        if entries.count > shown.count { lines.insert("(\(entries.count - shown.count) older requests not shown)", at: 0) }
        return lines.joined(separator: "\n")
    }

    private var consoleRead: [Int: Int] = [:]
    private var networkRead: [Int: Int] = [:]

    private static func count(_ n: Int, _ noun: String) -> String {
        "\(n) \(noun)\(n == 1 ? "" : "s")"
    }

    private static func quoted(_ text: String, limit: Int) -> String {
        "\"" + oneLine(text, limit: limit) + "\""
    }

    private static func oneLine(_ text: String, limit: Int) -> String {
        clip(text.split(whereSeparator: \.isNewline).joined(separator: " "), limit: limit)
    }

    private static func clip(_ text: String, limit: Int) -> String {
        text.count > limit ? String(text.prefix(limit - 1)) + "…" : text
    }
}
