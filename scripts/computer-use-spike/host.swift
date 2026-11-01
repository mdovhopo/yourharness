import AppKit
import ApplicationServices
import CoreGraphics

// Stable host: holds the macOS permissions. Never rebuilt, so edits to the plugin don't prompt again.
let dir = URL(fileURLWithPath: "/tmp/yh-cu-spike")
func say(_ s: String) {
    let line = s + "\n"
    let url = dir.appendingPathComponent("host.log")
    if let h = try? FileHandle(forWritingTo: url) { h.seekToEndOfFile(); h.write(line.data(using: .utf8)!); try? h.close() }
    else { try? line.write(to: url, atomically: true, encoding: .utf8) }
}
let mode = CommandLine.arguments.dropFirst().first ?? "request"
let app = NSApplication.shared
if mode == "request" {
    app.setActivationPolicy(.accessory)
    let opts = ["AXTrustedCheckOptionPrompt": true] as CFDictionary
    say("ax=\(AXIsProcessTrustedWithOptions(opts)) screen=\(CGPreflightScreenCaptureAccess())")
    _ = CGRequestScreenCaptureAccess()
    DispatchQueue.global().async {
        for _ in 0..<600 {
            if AXIsProcessTrusted() && CGPreflightScreenCaptureAccess() { say("GRANTED"); exit(0) }
            sleep(1)
        }
        say("TIMEOUT ax=\(AXIsProcessTrusted()) screen=\(CGPreflightScreenCaptureAccess())")
        exit(1)
    }
} else {
    guard let handle = dlopen(dir.appendingPathComponent("spike.dylib").path, RTLD_NOW) else {
        say("dlopen failed: \(String(cString: dlerror()))"); exit(2)
    }
    typealias Entry = @convention(c) (UnsafePointer<CChar>) -> Void
    let entry = unsafeBitCast(dlsym(handle, "spike_main"), to: Entry.self)
    mode.withCString { entry($0) }
}
app.run()
