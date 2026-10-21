import AppKit
import WebKit

/// A thread's browser: its tabs, who's driving, and where its pages live while no panel
/// shows them. Nothing loads until the agent opens a page or you type an address.
@MainActor @Observable
final class BrowserSession {
    private(set) var tabs: [BrowserTab] = []
    var selectedTabID: Int?
    /// You took over: the agent's actions fail until you give it back; it can still look.
    var userHasControl = false {
        didSet { if userHasControl != oldValue { for tab in tabs { tab.invalidateRefs() } } }
    }
    /// The element picker's result, for the composer.
    var onPick: ((PickedElement) -> Void)?
    /// Tabs opened or moved: their addresses are saved with the thread.
    var onChange: (() -> Void)?

    let projectID: UUID
    /// Where tabs were at the last quit, reopened when the browser is first used.
    private var restorable: [String]
    @ObservationIgnored private var nextTabID = 1
    @ObservationIgnored private var lastAgentUse: Date?
    @ObservationIgnored private var calmDown: Task<Void, Never>?
    @ObservationIgnored private var pendingSave: Task<Void, Never>?

    static let maxTabs = 8

    init(projectID: UUID, restoring urls: [String]) {
        self.projectID = projectID
        restorable = urls
    }

    var selectedTab: BrowserTab? {
        tabs.first { $0.id == selectedTabID } ?? tabs.last
    }

    /// What's saved with the thread: the open tabs, or what's still waiting to reopen.
    var savedURLs: [String] {
        tabs.isEmpty ? restorable : tabs.compactMap { $0.url?.absoluteString }.filter { !$0.hasPrefix("about:") }
    }

    var isEmpty: Bool { tabs.isEmpty && restorable.isEmpty }

    func tab(_ id: Int?) -> BrowserTab? {
        guard let id else { return selectedTab }
        return tabs.first { $0.id == id }
    }

    /// Reopens last session's tabs, once, on first use.
    func restoreIfNeeded() {
        guard tabs.isEmpty, !restorable.isEmpty else { return }
        let urls = restorable
        restorable = []
        for url in urls.prefix(Self.maxTabs) {
            let tab = newTab(url: url, byAgent: true)
            tab.isRestored = true
        }
    }

    @discardableResult
    func newTab(url: String? = nil, byAgent: Bool, configuration: WKWebViewConfiguration? = nil) -> BrowserTab {
        let configuration = configuration ?? BrowserProfiles.configuration(for: projectID)
        let messages = BrowserMessages()
        let controller = WKUserContentController()
        BrowserScripts.install(in: controller)
        controller.add(messages, contentWorld: .page, name: BrowserScripts.logHandler)
        controller.add(messages, contentWorld: BrowserScripts.world, name: "yourHarnessPick")
        configuration.userContentController = controller
        let tab = BrowserTab(id: nextTabID, configuration: configuration, openedByAgent: byAgent)
        nextTabID += 1
        messages.tab = tab
        tab.onOpenWindow = { [weak self] configuration, request in
            guard let self, self.tabs.count < Self.maxTabs else { return nil }
            let popup = self.newTab(byAgent: true, configuration: configuration)
            popup.log(["kind": "console", "level": "info", "text": "Opened by tab \(tab.id) (\(request.url?.absoluteString ?? "a new window"))"])
            return popup.webView
        }
        tab.onClose = { [weak self, weak tab] in
            if let self, let tab { self.close(tab) }
        }
        tab.onPick = { [weak self] picked in
            if let picked { self?.onPick?(picked) }
        }
        tabs.append(tab)
        selectedTabID = tab.id
        BrowserParking.shared.park(tab)
        if lastAgentUse != nil { WebKitSPI.setOcclusionDetection(tab.webView, enabled: false) }
        tab.onURLChange = { [weak self] in self?.urlChanged() }
        if let url { tab.load(url) }
        onChange?()
        return tab
    }

    func close(_ tab: BrowserTab) {
        tab.close()
        tabs.removeAll { $0 === tab }
        if selectedTabID == tab.id { selectedTabID = tabs.last?.id }
        onChange?()
    }

    func closeAll() {
        for tab in tabs { tab.close() }
        tabs.removeAll()
        selectedTabID = nil
        calmDown?.cancel()
        pendingSave?.cancel()
        pendingSave = nil
    }

    /// While the agent is using the browser, its pages run at full speed even where you
    /// can't see them; a couple of minutes after its last call, WebKit may throttle them.
    func agentUsed() {
        if lastAgentUse == nil {
            for tab in tabs { WebKitSPI.setOcclusionDetection(tab.webView, enabled: false) }
        }
        lastAgentUse = .now
        calmDown?.cancel()
        calmDown = Task { [weak self] in
            try? await Task.sleep(for: .seconds(120))
            guard !Task.isCancelled, let self else { return }
            self.lastAgentUse = nil
            for tab in self.tabs { WebKitSPI.setOcclusionDetection(tab.webView, enabled: true) }
        }
    }

    /// Saves the thread a moment after a tab goes somewhere new: redirects come in bursts.
    private func urlChanged() {
        guard pendingSave == nil else { return }
        pendingSave = Task { [weak self] in
            try? await Task.sleep(for: .seconds(1))
            guard !Task.isCancelled, let self else { return }
            self.pendingSave = nil
            self.onChange?()
        }
    }

    func clearBrowsingData() async {
        let store = BrowserProfiles.dataStore(for: projectID)
        await store.removeData(ofTypes: WKWebsiteDataStore.allWebsiteDataTypes(), modifiedSince: .distantPast)
        for tab in tabs { tab.webView.reload() }
    }
}

/// Cookies and storage per project: logins survive restarts and stay out of Safari and
/// your other projects.
enum BrowserProfiles {
    @MainActor private static var stores: [UUID: WKWebsiteDataStore] = [:]

    @MainActor static func dataStore(for projectID: UUID) -> WKWebsiteDataStore {
        if let store = stores[projectID] { return store }
        let store = WKWebsiteDataStore(forIdentifier: projectID)
        stores[projectID] = store
        return store
    }

    @MainActor static func configuration(for projectID: UUID) -> WKWebViewConfiguration {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = dataStore(for: projectID)
        configuration.preferences.isElementFullscreenEnabled = true
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = true
        configuration.mediaTypesRequiringUserActionForPlayback = []
        return configuration
    }
}

/// Where pages wait while no panel shows them: a window off screen that never takes
/// focus. In a window, WebKit keeps laying them out, running their timers and drawing
/// screenshots; out of one, it suspends them. A panel, so it doesn't count as a window
/// that keeps the app open after you close the last real one.
@MainActor
final class BrowserParking {
    static let shared = BrowserParking()
    private lazy var window: NSWindow = {
        let window = NSPanel(contentRect: NSRect(x: -30_000, y: -30_000, width: 2600, height: 2000),
                             styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        // Panels hide when the app goes to the background; this one's pages must not.
        window.hidesOnDeactivate = false
        window.isReleasedWhenClosed = false
        window.ignoresMouseEvents = true
        window.hasShadow = false
        window.isExcludedFromWindowsMenu = true
        window.collectionBehavior = [.transient, .ignoresCycle, .stationary, .fullScreenNone]
        window.contentView = FlippedView()
        window.orderFrontRegardless()
        return window
    }()

    /// The parking window, to tell whether a page is parked.
    var windowForChecks: NSWindow { window }

    /// Keeps its size and zoom, so the page doesn't relayout because you closed the panel.
    func park(_ tab: BrowserTab) {
        let web = tab.webView
        guard let content = window.contentView else { return }
        if web.superview !== content {
            web.removeFromSuperview()
            content.addSubview(web)
        }
        let size = web.frame.size == .zero
            ? CGSize(width: tab.viewport.width * web.pageZoom, height: tab.viewport.height * web.pageZoom)
            : web.frame.size
        web.frame = NSRect(origin: .zero, size: size)
    }
}

final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}
