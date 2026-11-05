import AppKit
import WebKit

let env = ProcessInfo.processInfo.environment

func log(_ line: String) {
    FileHandle.standardError.write(Data((line + "\n").utf8))
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    var window: NSWindow!
    var webView: WKWebView!
    var bridge: Bridge!
    var probe: HostProbe?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.mainMenu = Self.makeMenu()

        let config = WKWebViewConfiguration()
        config.setURLSchemeHandler(AppScheme.shared, forURLScheme: AppScheme.scheme)
        config.suppressesIncrementalRendering = false
        let controller = WKUserContentController()
        config.userContentController = controller
        // A probe run reads the page's own numbers; nothing else may slow it.
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")

        // WebKit draws pages at 60 Hz even on a 120 Hz screen unless told otherwise
        // (`YHW_60HZ=1` keeps its default, for comparing).
        if env["YHW_60HZ"] != "1" { Self.setFeature("PreferPageRenderingUpdatesNear60FPSEnabled", false, in: config.preferences) }

        let webView = WKWebView(frame: .zero, configuration: config)
        webView.isInspectable = true
        // A window parked below the screen still draws every frame, as the native app's probe expects.
        if env["YHW_PERF"] != nil || env["YHW_BACKGROUND"] == "1" { webView.setValue(false, forKey: "windowOcclusionDetectionEnabled") }
        self.webView = webView

        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1280, height: 820),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                              backing: .buffered, defer: false)
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.title = "YourHarness Web"
        window.minSize = NSSize(width: 640, height: 420)
        window.delegate = self
        window.setFrameAutosaveName("web-main")
        if window.frame.origin == .zero { window.center() }

        let content = NSView()
        content.addSubview(webView)
        webView.translatesAutoresizingMaskIntoConstraints = false
        let strip = DragStrip()
        content.addSubview(strip)
        strip.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            webView.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            webView.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            webView.topAnchor.constraint(equalTo: content.topAnchor),
            webView.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            // Over the page's own title bar, so the window moves and zooms as any other does.
            strip.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 80),
            strip.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            strip.topAnchor.constraint(equalTo: content.topAnchor),
            strip.heightAnchor.constraint(equalToConstant: 36),
        ])
        window.contentView = content
        self.window = window

        bridge = Bridge(webView: webView, window: window)
        controller.addScriptMessageHandler(bridge, contentWorld: .page, name: "host")
        if let folder = env["YHW_PERF"] {
            probe = HostProbe(folder: folder, bridge: bridge, window: window, webView: webView)
            bridge.probe = probe
        }

        webView.load(URLRequest(url: URL(string: "\(AppScheme.scheme)://app/index.html")!))
        if env["YHW_BACKGROUND"] == "1" {
            window.orderBack(nil)
        } else {
            window.makeKeyAndOrderFront(nil)
            NSApp.activate()
        }
        probe?.windowReady()
        if let file = env["YHW_SNAPSHOT"] { snapshot(to: file, after: Double(env["YHW_SNAPSHOT_DELAY"] ?? "") ?? 4) }
    }

    /// `YHW_SNAPSHOT=<file.png>`: the page drawn into a PNG, for looking without capturing the screen.
    func snapshot(to file: String, after delay: Double) {
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [self] in
            webView.takeSnapshot(with: nil) { image, error in
                guard let image, let tiff = image.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
                      let png = rep.representation(using: .png, properties: [:]) else { return log("snapshot failed: \(String(describing: error))") }
                try? png.write(to: URL(fileURLWithPath: file))
                log("SNAPSHOT \(file)")
                if env["YHW_SNAPSHOT_QUIT"] == "1" { NSApp.terminate(nil) }
            }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func applicationWillTerminate(_ notification: Notification) {
        bridge?.live.stopAll()
    }

    static func makeMenu() -> NSMenu {
        let main = NSMenu()
        func submenu(_ title: String, _ items: [NSMenuItem]) {
            let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
            let menu = NSMenu(title: title)
            items.forEach(menu.addItem)
            item.submenu = menu
            main.addItem(item)
        }
        func item(_ title: String, _ action: Selector?, _ key: String, _ modifiers: NSEvent.ModifierFlags = .command) -> NSMenuItem {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
            item.keyEquivalentModifierMask = modifiers
            return item
        }
        submenu("YourHarness Web", [
            item("Hide", #selector(NSApplication.hide(_:)), "h"),
            .separator(),
            item("Quit", #selector(NSApplication.terminate(_:)), "q"),
        ])
        // The page's text fields only get ⌘C, ⌘V, ⌘Z and the rest through these.
        submenu("Edit", [
            item("Undo", Selector(("undo:")), "z"),
            item("Redo", Selector(("redo:")), "z", [.command, .shift]),
            .separator(),
            item("Cut", #selector(NSText.cut(_:)), "x"),
            item("Copy", #selector(NSText.copy(_:)), "c"),
            item("Paste", #selector(NSText.paste(_:)), "v"),
            item("Select All", #selector(NSText.selectAll(_:)), "a"),
        ])
        submenu("View", [
            item("Reload", #selector(AppDelegate.reloadPage), "r"),
            item("Enter Full Screen", #selector(NSWindow.toggleFullScreen(_:)), "f", [.command, .control]),
        ])
        submenu("Window", [
            item("Minimize", #selector(NSWindow.performMiniaturize(_:)), "m"),
            item("Close", #selector(NSWindow.performClose(_:)), "w"),
        ])
        return main
    }

    @objc func reloadPage() { webView.reload() }

    /// Turns one of WebKit's feature flags on or off (WKPreferences' `_features` SPI).
    static func setFeature(_ key: String, _ enabled: Bool, in preferences: WKPreferences) {
        let features = (WKPreferences.self as AnyObject).perform(NSSelectorFromString("_features"))?.takeUnretainedValue() as? [NSObject] ?? []
        guard let feature = features.first(where: { $0.value(forKey: "key") as? String == key }) else {
            return log("no WebKit feature \(key)")
        }
        typealias Setter = @convention(c) (AnyObject, Selector, Bool, AnyObject) -> Void
        let selector = NSSelectorFromString("_setEnabled:forFeature:")
        guard let method = class_getInstanceMethod(WKPreferences.self, selector) else { return log("no _setEnabled:forFeature:") }
        unsafeBitCast(method_getImplementation(method), to: Setter.self)(preferences, selector, enabled, feature)
    }
}

/// The strip along the top edge that moves the window: the page can't say "this is title bar".
final class DragStrip: NSView {
    override var mouseDownCanMoveWindow: Bool { true }
    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 { window?.performZoom(nil) } else { window?.performDrag(with: event) }
    }
}
