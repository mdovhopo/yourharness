import SwiftUI
import WebKit

/// The right sidebar's Browser tab: the thread's pages, which the agent drives and you
/// watch, with the console and network under them.
struct BrowserPanel: View {
    @Environment(AppModel.self) private var app
    @Environment(\.openWindow) private var openWindow
    @Environment(\.dismissWindow) private var dismissWindow
    let thread: ThreadModel
    /// In its own window rather than the sidebar.
    var inWindow = false

    var body: some View {
        if let browser = thread.browser {
            content(browser)
        } else {
            Color.clear.onAppear { _ = thread.ensureBrowser() }
        }
    }

    private func content(_ browser: BrowserSession) -> some View {
        VStack(spacing: 0) {
            BrowserAddressBar(browser: browser, popOut: inWindow ? nil : { openWindow(id: "browser", value: thread.id) })
                .padding(.horizontal, 10)
                .padding(.top, 8)
                .padding(.bottom, 6)
            if browser.tabs.count > 1 {
                BrowserTabStrip(browser: browser)
                    .padding(.horizontal, 10)
                    .padding(.bottom, 6)
            }
            if browser.userHasControl {
                ControlBanner(browser: browser)
                    .padding(.horizontal, 10)
                    .padding(.bottom, 6)
            }
            if browser.isPoppedOut, !inWindow {
                ContentUnavailableView {
                    Label("In Its Own Window", systemImage: "macwindow")
                } description: {
                    Text("This thread's browser is open in a separate window. Claude keeps using it there.")
                } actions: {
                    Button("Show Window") { openWindow(id: "browser", value: thread.id) }
                        .buttonStyle(.glass)
                    Button("Bring It Back") { dismissWindow(id: "browser", value: thread.id) }
                        .buttonStyle(.glass)
                }
                .frame(maxHeight: .infinity)
            } else if let tab = browser.selectedTab {
                BrowserPageArea(tab: tab)
                    .id(tab.id)
            } else {
                BrowserEmptyState(thread: thread, browser: browser)
            }
        }
        .onAppear {
            browser.restoreIfNeeded()
            browser.viewerAppeared(window: inWindow)
        }
        .onDisappear { browser.viewerDisappeared(window: inWindow) }
    }
}

// MARK: - Address bar

private struct BrowserAddressBar: View {
    @Environment(AppModel.self) private var app
    let browser: BrowserSession
    /// Opens the browser in its own window; nil when it's already in one.
    let popOut: (() -> Void)?
    @State private var address = ""
    @FocusState private var editing: Bool

    private var tab: BrowserTab? { browser.selectedTab }

    var body: some View {
        HStack(spacing: 4) {
            Button("Back", systemImage: "chevron.backward") { tab?.webView.goBack() }
                .disabled(!(tab?.canGoBack ?? false))
                .help("Back")
            Button("Forward", systemImage: "chevron.forward") { tab?.webView.goForward() }
                .disabled(!(tab?.canGoForward ?? false))
                .help("Forward")
            addressField
            Button("Pick Element", systemImage: "scope") {
                guard let tab else { return }
                if tab.isPicking { tab.stopPicking() } else { tab.startPicking() }
            }
            .symbolVariant(tab?.isPicking == true ? .fill : .none)
            .foregroundStyle(tab?.isPicking == true ? AnyShapeStyle(.tint) : AnyShapeStyle(.primary))
            .disabled(tab == nil)
            .help("Pick an element to put in your message, with a picture of it. Esc cancels.")
            Button(browser.userHasControl ? "Give Back Control" : "Take Control",
                   systemImage: browser.userHasControl ? "hand.raised.fill" : "hand.raised") {
                browser.userHasControl.toggle()
            }
            .foregroundStyle(browser.userHasControl ? AnyShapeStyle(.orange) : AnyShapeStyle(.primary))
            .help(browser.userHasControl ? "Let Claude act in the browser again" : "Stop Claude from acting in the browser while you use it. It can still look.")
            BrowserMoreMenu(browser: browser, popOut: popOut)
        }
        .labelStyle(.iconOnly)
        .buttonStyle(.borderless)
        .padding(.horizontal, 6)
        .padding(.vertical, 4)
        .glassEffect(.regular, in: .capsule)
        .onAppear { address = tab?.url?.absoluteString ?? "" }
        .onChange(of: tab?.url) { if !editing { address = tab?.url?.absoluteString ?? "" } }
        .onChange(of: browser.selectedTabID) { address = tab?.url?.absoluteString ?? "" }
    }

    private var addressField: some View {
        HStack(spacing: 4) {
            if let tab, tab.url?.scheme == "https", !editing {
                Image(systemName: "lock.fill")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            TextField("Address or search", text: $address)
                .textFieldStyle(.plain)
                .focused($editing)
                .onSubmit(go)
                .onKeyPress(.escape) {
                    address = tab?.url?.absoluteString ?? ""
                    editing = false
                    return .handled
                }
            if let tab {
                Button(tab.isLoading ? "Stop" : "Reload", systemImage: tab.isLoading ? "xmark" : "arrow.clockwise") {
                    if tab.isLoading { tab.webView.stopLoading() } else { tab.webView.reload() }
                }
                .font(.caption.weight(.semibold))
                .help(tab.isLoading ? "Stop loading" : "Reload (⌘R in the page)")
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(.quaternary.opacity(0.6), in: .capsule)
        .overlay(alignment: .bottom) {
            if let tab, tab.isLoading {
                GeometryReader { proxy in
                    Capsule()
                        .fill(.tint)
                        .frame(width: max(8, proxy.size.width * tab.progress), height: 2)
                        .animation(.smooth, value: tab.progress)
                }
                .frame(height: 2)
                .padding(.horizontal, 10)
            }
        }
    }

    private func go() {
        let text = address
        editing = false
        if let tab { tab.load(text) } else { browser.newTab(url: text, byAgent: false) }
    }
}

private struct BrowserMoreMenu: View {
    let browser: BrowserSession
    let popOut: (() -> Void)?
    @State private var confirmingClear = false
    @State private var importingCookies = false

    private var tab: BrowserTab? { browser.selectedTab }

    var body: some View {
        Menu {
            if let popOut {
                Button("Open in Window", systemImage: "macwindow.badge.plus", action: popOut)
                Divider()
            }
            Button("New Tab", systemImage: "plus") { browser.newTab(byAgent: false) }
                .disabled(browser.tabs.count >= BrowserSession.maxTabs)
            if let tab {
                Button("Close Tab", systemImage: "xmark") { browser.close(tab) }
                Divider()
                Picker("Page Size", selection: Binding(get: { ViewportChoice(tab.viewport) }, set: { tab.resize(to: $0.viewport) })) {
                    ForEach(ViewportChoice.allCases) { choice in
                        Text(choice.label).tag(choice)
                    }
                }
                Picker("Appearance", selection: Binding(get: { tab.colorSchemeName ?? "auto" }, set: { tab.setColorScheme($0) })) {
                    Text("Same as the App").tag("auto")
                    Text("Light").tag("light")
                    Text("Dark").tag("dark")
                }
                Divider()
                Button("Open in Default Browser", systemImage: "safari") {
                    if let url = tab.url { NSWorkspace.shared.open(url) }
                }
                .disabled(tab.url == nil)
                Button("Copy Address", systemImage: "doc.on.doc") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(tab.url?.absoluteString ?? "", forType: .string)
                }
                .disabled(tab.url == nil)
                Button("Show Web Inspector", systemImage: "wrench.and.screwdriver") { WebInspector.show(tab.webView) }
            }
            Divider()
            Button("Import Cookies…", systemImage: "square.and.arrow.down") { importingCookies = true }
            Button("Clear Browsing Data…", systemImage: "trash") { confirmingClear = true }
        } label: {
            Label("More", systemImage: "ellipsis")
        }
        .menuIndicator(.hidden)
        .fixedSize()
        .help("More")
        .sheet(isPresented: $importingCookies) { CookieImportSheet(browser: browser) }
        .confirmationDialog("Clear this project's browsing data?", isPresented: $confirmingClear) {
            Button("Clear", role: .destructive) { Task { await browser.clearBrowsingData() } }
        } message: {
            Text("Cookies, logins and site storage for this project's browser are removed. Other projects and Safari keep theirs.")
        }
    }
}

/// The page sizes the menu offers.
private enum ViewportChoice: String, CaseIterable, Identifiable {
    case fit, laptop, desktop, tablet, phone, custom
    var id: String { rawValue }

    init(_ viewport: BrowserTab.Viewport) {
        if viewport.fitsPanel { self = .fit }
        else if viewport == .laptop { self = .laptop }
        else if viewport == .desktop { self = .desktop }
        else if viewport == .tablet { self = .tablet }
        else if viewport == .phone { self = .phone }
        else { self = .custom }
    }

    var label: String {
        switch self {
        case .fit: "Fit the Sidebar"
        case .laptop: "Laptop (1512 × 982)"
        case .desktop: "Desktop (1920 × 1080)"
        case .tablet: "Tablet (834 × 1210)"
        case .phone: "Phone (402 × 874)"
        case .custom: "Set by Claude"
        }
    }

    var viewport: BrowserTab.Viewport {
        switch self {
        case .fit: BrowserTab.Viewport(width: BrowserTab.Viewport.laptop.width, height: BrowserTab.Viewport.laptop.height, fitsPanel: true)
        case .laptop, .custom: .laptop
        case .desktop: .desktop
        case .tablet: .tablet
        case .phone: .phone
        }
    }
}

enum WebInspector {
    /// Safari's Web Inspector for the page, through WebKit's inspector object; without it,
    /// the Develop menu in Safari can still attach (the page is inspectable).
    @MainActor static func show(_ webView: WKWebView) {
        let selector = NSSelectorFromString("_inspector")
        guard webView.responds(to: selector),
              let inspector = webView.perform(selector)?.takeUnretainedValue() as? NSObject,
              inspector.responds(to: NSSelectorFromString("show"))
        else {
            let alert = NSAlert()
            alert.messageText = "Open the Web Inspector from Safari"
            alert.informativeText = "In Safari, turn on Settings → Advanced → Show features for web developers, then choose Develop → this Mac → YourHarness → the page."
            alert.runModal()
            return
        }
        inspector.perform(NSSelectorFromString("show"))
    }
}

// MARK: - Tabs and control

private struct BrowserTabStrip: View {
    let browser: BrowserSession

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 4) {
                ForEach(browser.tabs) { tab in
                    let selected = tab.id == browser.selectedTab?.id
                    HStack(spacing: 4) {
                        if tab.isLoading {
                            ProgressView().controlSize(.mini)
                        }
                        Text(tab.displayTitle)
                            .lineLimit(1)
                            .frame(maxWidth: 140)
                        Button("Close", systemImage: "xmark") { browser.close(tab) }
                            .labelStyle(.iconOnly)
                            .buttonStyle(.borderless)
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(.secondary)
                    }
                    .font(.caption)
                    .padding(.leading, 8)
                    .padding(.trailing, 4)
                    .padding(.vertical, 3)
                    .background(selected ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: .capsule)
                    .contentShape(.capsule)
                    .onTapGesture { browser.selectedTabID = tab.id }
                    .help(tab.url?.absoluteString ?? "")
                }
            }
        }
        .scrollIndicators(.hidden)
    }
}

private struct ControlBanner: View {
    let browser: BrowserSession

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "hand.raised.fill")
                .foregroundStyle(.orange)
            Text("You have control. Claude can look but not act.")
                .font(.callout)
            Spacer(minLength: 4)
            Button("Give Back") { browser.userHasControl = false }
                .buttonStyle(.glass)
                .controlSize(.small)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .card(radius: Theme.innerRadius + 4)
    }
}

// MARK: - The page

private struct BrowserPageArea: View {
    let tab: BrowserTab
    @State private var showsLogs = true

    var body: some View {
        GeometryReader { proxy in
            let fitted = BrowserPageLayout(viewport: tab.viewport, in: CGSize(width: proxy.size.width, height: proxy.size.height))
            // A fixed-size page fills the width; what's left below is for the logs.
            let pageHeight = tab.viewport.fitsPanel
                ? (showsLogs ? max(proxy.size.height - 220, proxy.size.height * 0.55) : proxy.size.height - 32)
                : min(fitted.frame.height, proxy.size.height - 32)
            VStack(spacing: 0) {
                BrowserPage(tab: tab)
                    .frame(height: max(pageHeight, 120))
                Divider()
                BrowserLogs(tab: tab, expanded: $showsLogs)
                    .frame(maxHeight: .infinity, alignment: .top)
            }
        }
    }
}

/// How a page of `viewport` CSS pixels sits in `size` points: scaled down to fit the
/// width (never up), at the top.
struct BrowserPageLayout {
    let zoom: Double
    let frame: CGRect

    init(viewport: BrowserTab.Viewport, in size: CGSize) {
        guard size.width > 1, size.height > 1 else {
            zoom = 1
            frame = .zero
            return
        }
        if viewport.fitsPanel {
            zoom = 1
            frame = CGRect(origin: .zero, size: size)
        } else {
            zoom = min(size.width / viewport.width, 1)
            let width = viewport.width * zoom
            frame = CGRect(x: (size.width - width) / 2, y: 0, width: width, height: viewport.height * zoom)
        }
    }
}

private struct BrowserPage: View {
    let tab: BrowserTab

    var body: some View {
        GeometryReader { proxy in
            let layout = BrowserPageLayout(viewport: tab.viewport, in: proxy.size)
            ZStack(alignment: .topLeading) {
                BrowserWebView(tab: tab, viewport: tab.viewport)
                if let highlight = tab.highlight {
                    let rect = highlight.rect
                    RoundedRectangle(cornerRadius: 4)
                        .stroke(.tint, lineWidth: 2)
                        .background(.tint.opacity(0.12), in: .rect(cornerRadius: 4))
                        .frame(width: max(rect.width * layout.zoom, 10), height: max(rect.height * layout.zoom, 10))
                        .offset(x: layout.frame.minX + rect.minX * layout.zoom, y: layout.frame.minY + rect.minY * layout.zoom)
                        .allowsHitTesting(false)
                        .transition(.opacity)
                        .id(highlight.id)
                }
            }
            .overlay(alignment: .bottom) {
                if let activity = tab.agentActivity {
                    Label(activity, systemImage: "sparkle")
                        .font(.caption.weight(.medium))
                        .padding(.horizontal, 10)
                        .padding(.vertical, 5)
                        .glassEffect(.regular, in: .capsule)
                        .padding(8)
                        .allowsHitTesting(false)
                        .transition(.opacity)
                }
            }
            .overlay {
                if let dialog = tab.dialog {
                    DialogCard(dialog: dialog) { accept, text in
                        tab.dialog = nil
                        dialog.respond(accept, text)
                    }
                    .padding(16)
                }
            }
            .animation(.easeOut(duration: 0.2), value: tab.highlight)
            .animation(.easeOut(duration: 0.2), value: tab.agentActivity)
        }
        .clipped()
    }
}

private struct DialogCard: View {
    let dialog: BrowserTab.Dialog
    let respond: (Bool, String?) -> Void
    @State private var text = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("The page says")
                .font(.caption)
                .foregroundStyle(.secondary)
            Text(dialog.message)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            if dialog.kind == .prompt {
                TextField("", text: $text)
                    .textFieldStyle(.roundedBorder)
                    .onAppear { text = dialog.defaultText ?? "" }
            }
            HStack {
                Spacer()
                if dialog.kind != .alert {
                    Button("Cancel") { respond(false, nil) }
                        .buttonStyle(.glass)
                }
                Button("OK") { respond(true, text) }
                    .buttonStyle(.glassProminent)
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(14)
        .frame(maxWidth: 360)
        .glassEffect(.regular, in: .rect(cornerRadius: Theme.floatingRadius))
    }
}

/// Hosts the tab's web view while it's on screen; when it leaves, the page goes back to
/// waiting off screen instead of being torn down.
private struct BrowserWebView: NSViewRepresentable {
    let tab: BrowserTab
    let viewport: BrowserTab.Viewport

    func makeNSView(context: Context) -> BrowserHostView { BrowserHostView() }

    func updateNSView(_ view: BrowserHostView, context: Context) {
        view.show(tab)
        view.needsLayout = true
    }

    static func dismantleNSView(_ view: BrowserHostView, coordinator: ()) {
        view.release()
    }
}

final class BrowserHostView: NSView {
    private var tab: BrowserTab?

    override var isFlipped: Bool { true }

    func show(_ tab: BrowserTab) {
        guard self.tab !== tab else { return }
        release()
        self.tab = tab
        addSubview(tab.pageView)
        needsLayout = true
    }

    func release() {
        if let tab, tab.pageView.superview === self { BrowserParking.shared.park(tab) }
        tab = nil
    }

    override func layout() {
        super.layout()
        guard let tab, !tab.holdsLayout else { return }
        let layout = BrowserPageLayout(viewport: tab.viewport, in: bounds.size)
        guard layout.frame.width > 1 else { return }
        tab.place(layout.frame, css: CGSize(width: layout.frame.width / layout.zoom, height: layout.frame.height / layout.zoom))
    }
}

// MARK: - Console and network

private struct BrowserLogs: View {
    let tab: BrowserTab
    @Binding var expanded: Bool
    @State private var kind = Kind.console
    @State private var filter = ""
    @State private var errorsOnly = false

    enum Kind: String, CaseIterable, Identifiable {
        case console, network
        var id: String { rawValue }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Button(expanded ? "Hide" : "Show", systemImage: "chevron.right") { withAnimation(.snappy(duration: 0.2)) { expanded.toggle() } }
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .rotationEffect(.degrees(expanded ? 90 : 0))
                Picker("Log", selection: $kind) {
                    Text(consoleLabel).tag(Kind.console)
                    Text(networkLabel).tag(Kind.network)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                Spacer(minLength: 4)
                if expanded {
                    Toggle(isOn: $errorsOnly) {
                        Label("Errors Only", systemImage: "exclamationmark.triangle")
                    }
                    .toggleStyle(.button)
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .help(kind == .console ? "Errors and warnings only" : "Failed requests only")
                    Button("Clear", systemImage: "trash") { tab.clearLogs() }
                        .labelStyle(.iconOnly)
                        .buttonStyle(.borderless)
                        .help("Clear the console and network logs")
                }
            }
            .font(.callout)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            if expanded {
                TextField("Filter", text: $filter)
                    .textFieldStyle(.roundedBorder)
                    .controlSize(.small)
                    .padding(.horizontal, 10)
                    .padding(.bottom, 6)
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        switch kind {
                        case .console:
                            ForEach(consoleEntries) { ConsoleRow(entry: $0) }
                        case .network:
                            ForEach(networkEntries) { NetworkRow(entry: $0) }
                        }
                    }
                    .padding(.bottom, 8)
                }
                // New lines at the bottom, like a console; a short list still starts at the top.
                .defaultScrollAnchor(.top, for: .alignment)
                .defaultScrollAnchor(.bottom, for: .initialOffset)
                .defaultScrollAnchor(.bottom, for: .sizeChanges)
                .overlay {
                    if (kind == .console ? consoleEntries.isEmpty : networkEntries.isEmpty) {
                        Text(kind == .console ? "No console messages" : "No requests")
                            .font(.callout)
                            .foregroundStyle(.tertiary)
                    }
                }
            }
        }
    }

    private var consoleLabel: String {
        let errors = tab.console.count { $0.isError }
        return errors > 0 ? "Console (\(errors) ⚠︎)" : "Console"
    }

    private var networkLabel: String {
        let failed = tab.network.count { $0.failed }
        return failed > 0 ? "Network (\(failed) ⚠︎)" : "Network"
    }

    private var consoleEntries: [BrowserTab.ConsoleEntry] {
        tab.console.suffix(500).filter { entry in
            (!errorsOnly || entry.isError || entry.level == "warn")
                && (filter.isEmpty || entry.text.localizedCaseInsensitiveContains(filter))
        }
    }

    private var networkEntries: [BrowserTab.NetworkEntry] {
        tab.network.suffix(500).filter { entry in
            (!errorsOnly || entry.failed) && (filter.isEmpty || entry.url.localizedCaseInsensitiveContains(filter))
        }
    }
}

private struct ConsoleRow: View {
    let entry: BrowserTab.ConsoleEntry

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: symbol)
                .foregroundStyle(tint)
                .font(.caption)
                .frame(width: 14)
            Text(entry.text)
                .font(.caption.monospaced())
                .foregroundStyle(entry.isError ? AnyShapeStyle(.red) : AnyShapeStyle(.primary))
                .lineLimit(6)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 3)
        .background(entry.isError ? Color.red.opacity(0.06) : entry.level == "warn" ? Color.yellow.opacity(0.07) : .clear)
    }

    private var symbol: String {
        switch entry.level {
        case "error": "xmark.octagon.fill"
        case "warn": "exclamationmark.triangle.fill"
        case "info": "info.circle"
        case "debug": "ant"
        default: "chevron.right"
        }
    }

    private var tint: AnyShapeStyle {
        switch entry.level {
        case "error": AnyShapeStyle(.red)
        case "warn": AnyShapeStyle(.yellow)
        default: AnyShapeStyle(.tertiary)
        }
    }
}

private struct NetworkRow: View {
    let entry: BrowserTab.NetworkEntry

    var body: some View {
        HStack(spacing: 6) {
            Text(entry.method)
                .font(.caption2.monospaced().weight(.semibold))
                .foregroundStyle(.secondary)
                .frame(width: 38, alignment: .leading)
            Text(entry.status.map(String.init) ?? (entry.failure != nil ? "failed" : "–"))
                .font(.caption.monospacedDigit())
                .foregroundStyle(entry.failed ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                .frame(width: 40, alignment: .leading)
            Text(path)
                .font(.caption.monospaced())
                .foregroundStyle(entry.failed ? AnyShapeStyle(.red) : AnyShapeStyle(.primary))
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text(entry.type)
                .font(.caption2)
                .foregroundStyle(.tertiary)
            if let duration = entry.duration {
                Text("\(Int(duration)) ms")
                    .font(.caption2.monospacedDigit())
                    .foregroundStyle(.tertiary)
                    .frame(width: 52, alignment: .trailing)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 3)
        .help(entry.failure.map { "\(entry.url)\n\($0)" } ?? entry.url)
        .textSelection(.enabled)
    }

    private var path: String {
        guard let url = URL(string: entry.url), let host = url.host() else { return entry.url }
        return host + url.path() + (url.query().map { "?" + $0 } ?? "")
    }
}

// MARK: - Empty

private struct BrowserEmptyState: View {
    let thread: ThreadModel
    let browser: BrowserSession

    var body: some View {
        let servers = LocalServers.found(in: thread)
        VStack(spacing: 16) {
            ContentUnavailableView {
                Label("Browser", systemImage: "safari")
            } description: {
                Text("Claude opens pages here to test what it builds, and you watch each step. You can type an address above too.")
            }
            // Not fixedSize: measured at the window's narrowest, the text wraps a word a line, and
            // its height (2,000 pt and more) became the window's minimum, pushing it off screen.
            if !servers.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Started in this thread")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    ForEach(servers, id: \.self) { server in
                        Button {
                            browser.newTab(url: server, byAgent: false)
                        } label: {
                            Label(BrowserTab.short(URL(string: server) ?? URL(filePath: "/")), systemImage: "server.rack")
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .buttonStyle(.glass)
                    }
                }
                .frame(maxWidth: 280)
            }
        }
        .padding()
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .task { thread.refreshLocalServers() }
    }
}

/// A thread's browser in its own window, for a bigger look than the sidebar gives.
struct BrowserWindow: View {
    @Environment(AppModel.self) private var app
    let threadID: UUID

    var body: some View {
        if let thread = app.threads.first(where: { $0.id == threadID }) {
            BrowserPanel(thread: thread, inWindow: true)
                .canvasBackground()
                .navigationTitle(thread.browser?.selectedTab?.displayTitle ?? "Browser")
                .navigationSubtitle(thread.title)
        } else {
            ContentUnavailableView("Thread Closed", systemImage: "safari")
        }
    }
}
