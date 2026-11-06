// The reference: layout.html in a plain WKWebView whose frame is exactly the viewport, unscaled,
// as Safari draws it. Writes reference.json (what the page measures) and reference-<size>.png.
// Usage: reference <pages dir> <out dir>
import AppKit
import WebKit

let pages = CommandLine.arguments[1], out = CommandLine.arguments[2]
let sizes = try! JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: pages + "/sizes.json"))) as! [String: [Double]]
let metrics = try! String(contentsOfFile: pages + "/metrics.js", encoding: .utf8)

final class Loader: NSObject, WKNavigationDelegate {
    var done: CheckedContinuation<Void, Never>?
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { done?.resume(); done = nil }
}

@MainActor func run() async {
    var results: [String: Any] = [:]
    for (name, size) in sizes {
        let (width, height) = (size[0], size[1])
        let window = NSWindow(contentRect: NSRect(x: -30_000, y: -30_000, width: width, height: height), styleMask: [.borderless], backing: .buffered, defer: false)
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        let web = WKWebView(frame: NSRect(x: 0, y: 0, width: width, height: height), configuration: configuration)
        window.contentView = web
        window.orderFrontRegardless()
        let loader = Loader()
        web.navigationDelegate = loader
        await withCheckedContinuation { done in
            loader.done = done
            web.loadFileURL(URL(fileURLWithPath: pages + "/layout.html"), allowingReadAccessTo: URL(fileURLWithPath: pages))
        }
        try? await Task.sleep(for: .milliseconds(600))
        let json = try! await web.evaluateJavaScript(metrics) as! String
        results[name] = try! JSONSerialization.jsonObject(with: Data(json.utf8))
        let snapshot = WKSnapshotConfiguration()
        snapshot.rect = CGRect(x: 0, y: 0, width: width, height: height)
        snapshot.afterScreenUpdates = true
        let image = try! await web.takeSnapshot(configuration: snapshot)
        try! NSBitmapImageRep(data: image.tiffRepresentation!)!.representation(using: .png, properties: [:])!
            .write(to: URL(fileURLWithPath: "\(out)/reference-\(name).png"))
        window.orderOut(nil)
    }
    try! JSONSerialization.data(withJSONObject: results, options: [.prettyPrinted, .sortedKeys]).write(to: URL(fileURLWithPath: out + "/reference.json"))
    exit(0)
}

NSApplication.shared.setActivationPolicy(.accessory)
Task { await run() }
NSApplication.shared.run()
