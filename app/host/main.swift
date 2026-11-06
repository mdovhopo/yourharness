import AppKit

// The web rendering experiment's host: one window, one WKWebView, HarnessCore underneath.
// Everything drawn is web (web/ui); this side reads history, runs `claude`, and hands events
// to the page in one batch a frame. See web/README.md.

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = AppDelegate()
app.delegate = delegate
app.run()
