import AppKit
import ApplicationServices
import ScreenCaptureKit

let dir = URL(fileURLWithPath: "/tmp/yh-cu-spike")
func append(_ s: String, _ name: String) {
    let line = s + "\n"
    let url = dir.appendingPathComponent(name)
    if let h = try? FileHandle(forWritingTo: url) { h.seekToEndOfFile(); h.write(line.data(using: .utf8)!); try? h.close() }
    else { try? line.write(to: url, atomically: true, encoding: .utf8) }
}
func say(_ s: String) { append(s, "results.log") }
func nap(_ s: Double) async { try? await Task.sleep(for: .seconds(s)) }

// MARK: - Custom-drawn target: no accessibility, every event logged

final class TargetView: NSView {
    var tick = 0
    override var acceptsFirstResponder: Bool { true }
    override func draw(_ r: NSRect) {
        tick += 1
        NSColor(hue: CGFloat(tick % 120) / 120, saturation: 0.6, brightness: 0.9, alpha: 1).setFill()
        bounds.fill()
    }
    override func mouseDown(with e: NSEvent) {
        let p = convert(e.locationInWindow, from: nil)
        append("mouse \(Int(p.x)) \(Int(bounds.height - p.y)) key=\(window?.isKeyWindow ?? false)", "target.log")
    }
    override func keyDown(with e: NSEvent) { append("key \(e.characters ?? "?")", "target.log") }
    override func scrollWheel(with e: NSEvent) { append("scroll \(e.scrollingDeltaY)", "target.log") }
}
var targetWindow: NSWindow?
func runTarget() {
    NSApplication.shared.setActivationPolicy(.regular)
    let w = NSWindow(contentRect: NSRect(x: 80, y: 80, width: 600, height: 400), styleMask: [.titled, .miniaturizable], backing: .buffered, defer: false)
    w.title = "YH Spike Target"
    let v = TargetView(frame: w.contentView!.bounds)
    w.contentView = v
    w.makeFirstResponder(v)
    w.orderBack(nil)
    targetWindow = w
    Timer.scheduledTimer(withTimeInterval: 1.0 / 60, repeats: true) { _ in v.needsDisplay = true }
}

// MARK: - Helpers

func window(_ pid: pid_t, wait: Double = 15) async -> SCWindow? {
    let end = Date().addingTimeInterval(wait)
    while Date() < end {
        if let c = try? await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false),
           let w = c.windows.filter({ $0.owningApplication?.processID == pid && $0.windowLayer == 0 && $0.frame.width > 150 && $0.frame.height > 150 }).max(by: { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height }) { return w }
        await nap(0.5)
    }
    return nil
}
func state() -> String {
    "front=\(NSWorkspace.shared.frontmostApplication?.localizedName ?? "?") pointer=\(CGEvent(source: nil)?.location ?? .zero)"
}
func click(_ pid: pid_t, _ p: CGPoint, windowID: CGWindowID?) {
    for type in [CGEventType.leftMouseDown, .leftMouseUp] {
        guard let e = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: p, mouseButton: .left) else { continue }
        e.setIntegerValueField(.mouseEventClickState, value: 1)
        if let windowID {
            e.setIntegerValueField(CGEventField(rawValue: 91)!, value: Int64(windowID))
            e.setIntegerValueField(CGEventField(rawValue: 92)!, value: Int64(windowID))
        }
        e.postToPid(pid)
    }
}
func typeKeys(_ pid: pid_t, _ text: String) async {
    for ch in text.utf16 {
        var c = ch
        for down in [true, false] {
            guard let e = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down) else { continue }
            e.keyboardSetUnicodeString(stringLength: 1, unicodeString: &c)
            e.postToPid(pid)
        }
        await nap(0.02)
    }
}
func attr(_ e: AXUIElement, _ a: String) -> AnyObject? {
    var v: AnyObject?
    return AXUIElementCopyAttributeValue(e, a as CFString, &v) == .success ? v : nil
}
func walk(_ e: AXUIElement, _ out: inout [AXUIElement], depth: Int = 0) {
    guard out.count < 6000, depth < 40 else { return }
    out.append(e)
    for c in (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] { walk(c, &out, depth: depth + 1) }
}
func center(_ e: AXUIElement) -> CGPoint? {
    guard let p = attr(e, kAXPositionAttribute), let s = attr(e, kAXSizeAttribute) else { return nil }
    var pos = CGPoint.zero, size = CGSize.zero
    AXValueGetValue(p as! AXValue, .cgPoint, &pos); AXValueGetValue(s as! AXValue, .cgSize, &size)
    return CGPoint(x: pos.x + size.width / 2, y: pos.y + size.height / 2)
}
func role(_ e: AXUIElement) -> String { attr(e, kAXRoleAttribute) as? String ?? "" }
func shot(_ w: SCWindow, _ name: String) async -> [UInt8]? {
    let c = SCStreamConfiguration()
    let scale = 2
    c.width = Int(w.frame.width) * scale; c.height = Int(w.frame.height) * scale
    do {
        let img = try await SCScreenshotManager.captureImage(contentFilter: SCContentFilter(desktopIndependentWindow: w), configuration: c)
        let rep = NSBitmapImageRep(cgImage: img)
        try rep.representation(using: .png, properties: [:])?.write(to: dir.appendingPathComponent(name + ".png"))
        guard let d = rep.bitmapData else { return nil }
        return Array(UnsafeBufferPointer(start: d, count: rep.bytesPerRow * rep.pixelsHigh))
    } catch { say("  shot \(name) failed: \(error)"); return nil }
}
func changed(_ a: [UInt8]?, _ b: [UInt8]?) -> String {
    guard let a, let b, a.count == b.count else { return "n/a" }
    var n = 0; for i in stride(from: 0, to: a.count, by: 4) where a[i] != b[i] { n += 1 }
    return String(format: "%.3f%%", Double(n) / Double(a.count / 4) * 100)
}
func launch(_ path: String, args: [String] = [], open url: URL? = nil) async -> NSRunningApplication? {
    let c = NSWorkspace.OpenConfiguration()
    c.activates = false; c.hides = false; c.addsToRecentItems = false; c.createsNewApplicationInstance = true
    c.arguments = args
    let app = URL(fileURLWithPath: path)
    do {
        if let url { return try await NSWorkspace.shared.open([url], withApplicationAt: app, configuration: c) }
        return try await NSWorkspace.shared.openApplication(at: app, configuration: c)
    } catch { say("  launch \(path) failed: \(error)"); return nil }
}
func cpuSeconds() -> Double {
    var u = rusage(); getrusage(RUSAGE_SELF, &u)
    return Double(u.ru_utime.tv_sec + u.ru_stime.tv_sec) + Double(u.ru_utime.tv_usec + u.ru_stime.tv_usec) / 1e6
}
func footprintMB() -> Double {
    var info = task_vm_info_data_t(); var n = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
    _ = withUnsafeMutablePointer(to: &info) { $0.withMemoryRebound(to: integer_t.self, capacity: Int(n)) { task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &n) } }
    return Double(info.phys_footprint) / 1_048_576
}
final class Counter: NSObject, SCStreamOutput, @unchecked Sendable {
    let lock = NSLock(); var complete = 0
    func stream(_ s: SCStream, didOutputSampleBuffer b: CMSampleBuffer, of t: SCStreamOutputType) {
        guard let a = CMSampleBufferGetSampleAttachmentsArray(b, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
              let raw = a.first?[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete else { return }
        lock.withLock { complete += 1 }
    }
}
func measure(_ w: SCWindow, streams: Int, seconds: Double) async -> String {
    let counter = Counter()
    var live: [SCStream] = []
    let c = SCStreamConfiguration()
    c.width = Int(w.frame.width) * 2; c.height = Int(w.frame.height) * 2
    c.minimumFrameInterval = CMTime(value: 1, timescale: 30); c.queueDepth = 3
    do {
        for _ in 0..<streams {
            let s = SCStream(filter: SCContentFilter(desktopIndependentWindow: w), configuration: c, delegate: nil)
            try s.addStreamOutput(counter, type: .screen, sampleHandlerQueue: .global())
            try await s.startCapture()
            live.append(s)
        }
    } catch { return "start failed: \(error)" }
    let cpu0 = cpuSeconds()
    await nap(seconds)
    let cpu = (cpuSeconds() - cpu0) / seconds * 100
    for s in live { try? await s.stopCapture() }
    return String(format: "%.1f fps per stream, %.1f%% cpu, footprint %.0f MB", Double(counter.complete) / seconds / Double(streams), cpu, footprintMB())
}

// MARK: - Tests

func testTarget() async {
    say("== custom-drawn target")
    let lg = dir.appendingPathComponent("target.log"); try? FileManager.default.removeItem(at: lg)
    let host = Bundle.main.bundleURL
    let c = NSWorkspace.OpenConfiguration(); c.activates = false; c.createsNewApplicationInstance = true; c.arguments = ["target"]
    guard let app = try? await NSWorkspace.shared.openApplication(at: host, configuration: c), let w = await window(app.processIdentifier) else { say("  no target window"); return }
    await nap(1)
    let pid = app.processIdentifier
    say("  window frame \(w.frame) onScreen=\(w.isOnScreen) \(state())")
    func events() -> String { ((try? String(contentsOf: lg, encoding: .utf8)) ?? "").replacingOccurrences(of: "\n", with: " | ") }
    // Content is below a 28 pt-ish title bar; aim at image-space (100, 100) in window points.
    let aim = CGPoint(x: w.frame.minX + 100, y: w.frame.minY + 100)
    click(pid, aim, windowID: nil); await nap(0.5)
    say("  plain postToPid click: [\(events())] \(state())")
    try? FileManager.default.removeItem(at: lg)
    click(pid, aim, windowID: w.windowID); await nap(0.5)
    say("  click with window fields: [\(events())] \(state())")
    try? FileManager.default.removeItem(at: lg)
    await typeKeys(pid, "hi"); await nap(0.5)
    say("  keys: [\(events())] \(state())")
    try? FileManager.default.removeItem(at: lg)
    if let e = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: -30, wheel2: 0, wheel3: 0) { e.location = aim; e.postToPid(pid) }
    await nap(0.5)
    say("  scroll: [\(events())] \(state())")
    say("  stream 1x: \(await measure(w, streams: 1, seconds: 8))")
    say("  stream 4x: \(await measure(w, streams: 4, seconds: 8))")
    _ = await shot(w, "target")
    let ax = AXUIElementCreateApplication(pid)
    if let wins = attr(ax, kAXWindowsAttribute) as? [AXUIElement], let win = wins.first {
        AXUIElementSetAttributeValue(win, kAXMinimizedAttribute as CFString, kCFBooleanTrue)
        await nap(2)
        say("  minimized, stream: \(await measure(w, streams: 1, seconds: 4))")
        say("  minimized, shot bytes: \((await shot(w, "target-min"))?.count ?? -1)")
    }
    app.forceTerminate()
}

func testCalculator() async {
    say("== Calculator (native, AX)")
    guard let app = await launch("/System/Applications/Calculator.app"), let w = await window(app.processIdentifier) else { say("  no window"); return }
    await nap(1)
    let pid = app.processIdentifier, ax = AXUIElementCreateApplication(pid)
    var all: [AXUIElement] = []; walk(ax, &all)
    let buttons = all.filter { role($0) == kAXButtonRole }
    func name(_ e: AXUIElement) -> String { (attr(e, kAXDescriptionAttribute) as? String) ?? (attr(e, kAXTitleAttribute) as? String) ?? "" }
    say("  elements \(all.count), buttons \(buttons.count): \(buttons.prefix(8).map(name)) \(state())")
    func display() -> String {
        var a: [AXUIElement] = []; walk(ax, &a)
        return a.filter { role($0) == kAXStaticTextRole }.compactMap { attr($0, kAXValueAttribute) as? String }.joined(separator: "/")
    }
    if let seven = buttons.first(where: { name($0) == "7" }) {
        let r = AXUIElementPerformAction(seven, kAXPressAction as CFString)
        await nap(0.5)
        say("  AXPress 7: \(r.rawValue) display=\(display()) \(state())")
    } else { say("  no button 7") }
    if let eight = buttons.first(where: { name($0) == "8" }), let p = center(eight) {
        click(pid, p, windowID: nil); await nap(0.5)
        say("  plain click 8: display=\(display()) \(state())")
        click(pid, p, windowID: w.windowID); await nap(0.5)
        say("  fields click 8: display=\(display()) \(state())")
    }
    await typeKeys(pid, "9"); await nap(0.5)
    say("  key 9: display=\(display()) \(state())")
    _ = await shot(w, "calculator")
    app.forceTerminate()
}

func testTextEdit() async {
    say("== TextEdit (native text, scratch file)")
    let file = dir.appendingPathComponent("scratch.txt")
    try? "start\n".write(to: file, atomically: true, encoding: .utf8)
    guard let app = await launch("/System/Applications/TextEdit.app", args: ["-ApplePersistenceIgnoreState", "YES"], open: file), await window(app.processIdentifier) != nil else { say("  no window"); return }
    await nap(1)
    let ax = AXUIElementCreateApplication(app.processIdentifier)
    var all: [AXUIElement] = []; walk(ax, &all)
    guard let area = all.first(where: { role($0) == kAXTextAreaRole }) else { say("  no text area, \(all.count) elements"); app.forceTerminate(); return }
    let set = AXUIElementSetAttributeValue(area, kAXValueAttribute as CFString, "set by ax" as CFString)
    await nap(0.3)
    say("  AX set value: \(set.rawValue) -> \(attr(area, kAXValueAttribute) as? String ?? "nil") \(state())")
    await typeKeys(app.processIdentifier, "typed"); await nap(0.5)
    say("  keys -> \(attr(area, kAXValueAttribute) as? String ?? "nil") \(state())")
    app.forceTerminate()
}

func testVSCode() async {
    say("== VS Code (Electron, throwaway profile)")
    let profile = dir.appendingPathComponent("vscode-profile"), file = dir.appendingPathComponent("vscode-scratch.txt")
    try? "start\n".write(to: file, atomically: true, encoding: .utf8)
    let exe = "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"
    _ = exe
    guard let app = await launch("/Applications/Visual Studio Code.app", args: ["--user-data-dir", profile.path, "--extensions-dir", profile.appendingPathComponent("ext").path, "--disable-workspace-trust", "--skip-welcome", "--skip-release-notes", "--disable-extensions", file.path]), let w = await window(app.processIdentifier, wait: 30) else { say("  no window"); return }
    await nap(8)
    let pid = app.processIdentifier, ax = AXUIElementCreateApplication(pid)
    var before: [AXUIElement] = []; walk(ax, &before)
    AXUIElementSetAttributeValue(ax, "AXManualAccessibility" as CFString, kCFBooleanTrue)
    await nap(3)
    var all: [AXUIElement] = []; walk(ax, &all)
    say("  elements \(before.count) -> \(all.count) with AXManualAccessibility, buttons \(all.filter { role($0) == kAXButtonRole }.count) \(state())")
    let area = all.first { role($0) == kAXTextAreaRole }
    say("  text area: \(area != nil) value=\((area.flatMap { attr($0, kAXValueAttribute) as? String })?.prefix(40) ?? "nil")")
    let a = await shot(w, "vscode-before")
    if let p = area.flatMap(center) { click(pid, p, windowID: w.windowID); await nap(0.5) }
    await typeKeys(pid, "typedbypid"); await nap(1.5)
    let b = await shot(w, "vscode-after")
    say("  keys: pixels changed \(changed(a, b)) value=\((area.flatMap { attr($0, kAXValueAttribute) as? String })?.prefix(40) ?? "nil") \(state())")
    app.forceTerminate()
}

typealias PostToPid = @convention(c) (pid_t, CGEvent) -> Void
typealias SetWinLoc = @convention(c) (CGEvent, CGPoint) -> Void
let RTLD_DEF = UnsafeMutableRawPointer(bitPattern: -2)
let skylight = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW)
let slPost = dlsym(skylight, "SLEventPostToPid").map { unsafeBitCast($0, to: PostToPid.self) }
let setWinLoc = dlsym(RTLD_DEF, "CGEventSetWindowLocation").map { unsafeBitCast($0, to: SetWinLoc.self) }

/// `how`: hid = HID-state source with a move first; sl = SkyLight post; slwin = SkyLight plus window fields and location.
func variantClick(_ pid: pid_t, _ p: CGPoint, _ w: SCWindow, how: String) async {
    let src = CGEventSource(stateID: how == "hid" ? .hidSystemState : .privateState)
    func make(_ t: CGEventType) -> CGEvent? {
        guard let e = CGEvent(mouseEventSource: src, mouseType: t, mouseCursorPosition: p, mouseButton: .left) else { return nil }
        e.setIntegerValueField(.mouseEventClickState, value: 1)
        e.setIntegerValueField(.mouseEventButtonNumber, value: 0)
        if how != "hid" {
            e.setIntegerValueField(CGEventField(rawValue: 91)!, value: Int64(w.windowID))
            e.setIntegerValueField(CGEventField(rawValue: 92)!, value: Int64(w.windowID))
        }
        if how == "slwin" { setWinLoc?(e, CGPoint(x: p.x - w.frame.minX, y: p.y - w.frame.minY)) }
        return e
    }
    for t in [CGEventType.mouseMoved, .leftMouseDown, .leftMouseUp] {
        guard let e = make(t) else { continue }
        if how == "hid" { e.postToPid(pid) } else { slPost?(pid, e) }
        await nap(0.05)
    }
}
func run2() async {
    let prior = NSWorkspace.shared.frontmostApplication
    say("START2 slPost=\(slPost != nil) setWinLoc=\(setWinLoc != nil) \(state())")
    let lg = dir.appendingPathComponent("target.log"); try? FileManager.default.removeItem(at: lg)
    let c = NSWorkspace.OpenConfiguration(); c.activates = false; c.createsNewApplicationInstance = true; c.arguments = ["target"]
    if let app = try? await NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: c), let w = await window(app.processIdentifier) {
        await nap(1)
        let aim = CGPoint(x: w.frame.minX + 100, y: w.frame.minY + 100)
        for how in ["hid", "sl", "slwin"] {
            try? FileManager.default.removeItem(at: lg)
            await variantClick(app.processIdentifier, aim, w, how: how); await nap(0.5)
            say("  target \(how): [\(((try? String(contentsOf: lg, encoding: .utf8)) ?? "").replacingOccurrences(of: "\n", with: " | "))] \(state())")
        }
        // Control: the same event with the target frontmost, to tell a test bug from background delivery failing.
        app.activate(); await nap(0.4)
        try? FileManager.default.removeItem(at: lg)
        click(app.processIdentifier, aim, windowID: nil); await nap(0.4)
        say("  CONTROL target frontmost, plain click: [\(((try? String(contentsOf: lg, encoding: .utf8)) ?? "").replacingOccurrences(of: "\n", with: " | "))] \(state())")
        prior?.activate(); await nap(0.3)
        try? FileManager.default.removeItem(at: lg)
        if let e = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: -30, wheel2: 0, wheel3: 0) { e.location = aim; e.setIntegerValueField(CGEventField(rawValue: 91)!, value: Int64(w.windowID)); slPost?(app.processIdentifier, e) }
        await nap(0.5)
        say("  target sl scroll: [\(((try? String(contentsOf: lg, encoding: .utf8)) ?? "").replacingOccurrences(of: "\n", with: " | "))]")
        app.forceTerminate()
    }
    if let app = await launch("/System/Applications/Calculator.app"), let w = await window(app.processIdentifier) {
        await nap(1)
        let pid = app.processIdentifier, ax = AXUIElementCreateApplication(pid)
        var all: [AXUIElement] = []; walk(ax, &all)
        func name(_ e: AXUIElement) -> String { (attr(e, kAXDescriptionAttribute) as? String) ?? "" }
        func display() -> String { var a: [AXUIElement] = []; walk(ax, &a); return a.filter { role($0) == kAXStaticTextRole }.compactMap { attr($0, kAXValueAttribute) as? String }.joined(separator: "/") }
        if let b = all.first(where: { role($0) == kAXButtonRole && name($0) == "8" }), let p = center(b) {
            for how in ["hid", "sl", "slwin"] {
                await variantClick(pid, p, w, how: how); await nap(0.5)
                say("  calculator \(how) click 8: display=\(display()) \(state())")
            }
        }
        app.forceTerminate()
    }
    let profile = dir.appendingPathComponent("vscode-profile"), file = dir.appendingPathComponent("vscode-scratch.txt")
    try? "start\n".write(to: file, atomically: true, encoding: .utf8)
    if let app = await launch("/Applications/Visual Studio Code.app", args: ["--user-data-dir", profile.path, "--extensions-dir", profile.appendingPathComponent("ext").path, "--disable-workspace-trust", "--skip-welcome", "--skip-release-notes", "--disable-extensions", file.path]), let w = await window(app.processIdentifier, wait: 30) {
        await nap(8)
        let w = await window(app.processIdentifier) ?? w
        say("  vscode after launch \(state()) frame \(w.frame)")
        prior?.activate(); await nap(1)
        say("  vscode front restored \(state())")
        let pid = app.processIdentifier, ax = AXUIElementCreateApplication(pid)
        AXUIElementSetAttributeValue(ax, "AXManualAccessibility" as CFString, kCFBooleanTrue); await nap(3)
        var all: [AXUIElement] = []; walk(ax, &all)
        let a = await shot(w, "vscode2-0")
        await typeKeys(pid, "keysonly"); await nap(1.5)
        let b = await shot(w, "vscode2-1")
        say("  keys, no click: pixels changed \(changed(a, b)) \(state())")
        let area = all.first { role($0) == kAXTextAreaRole }
        if let area { let f = AXUIElementSetAttributeValue(area, kAXFocusedAttribute as CFString, kCFBooleanTrue); say("  AX focus text area: \(f.rawValue) \(state())") }
        await nap(0.5)
        await typeKeys(pid, "afterfocus"); await nap(1.5)
        let c2 = await shot(w, "vscode2-2")
        say("  keys after AX focus: pixels changed \(changed(b, c2)) \(state())")
        if let btn = all.first(where: { role($0) == kAXButtonRole && !((attr($0, kAXDescriptionAttribute) as? String) ?? "").isEmpty }) {
            let r = AXUIElementPerformAction(btn, kAXPressAction as CFString); await nap(1.5)
            let d = await shot(w, "vscode2-3")
            say("  AXPress '\(attr(btn, kAXDescriptionAttribute) as? String ?? "?")': \(r.rawValue) pixels changed \(changed(c2, d)) \(state())")
        }
        if let p = area.flatMap(center) {
            let e0 = await shot(w, "vscode2-4")
            await variantClick(pid, CGPoint(x: p.x, y: p.y + 60), w, how: "slwin"); await nap(1)
            let e1 = await shot(w, "vscode2-5")
            say("  slwin click: pixels changed \(changed(e0, e1)) \(state())")
        }
        app.forceTerminate()
    }
    prior?.activate()
    say("END \(state())")
    exit(0)
}

@_cdecl("spike_main")
public func spike_main(_ m: UnsafePointer<CChar>) {
    if String(cString: m) == "target" { runTarget(); return }
    if String(cString: m) == "check" {
        typealias Resp = @convention(c) (pid_t) -> pid_t
        let sym = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "responsibility_get_pid_responsible_for_pid")
        let me = getpid()
        let r = sym.map { unsafeBitCast($0, to: Resp.self)(me) } ?? -1
        say("CHECK pid=\(me) responsible=\(r) (\(NSRunningApplication(processIdentifier: r)?.localizedName ?? "?")) ppid=\(getppid()) ax=\(AXIsProcessTrusted()) screen=\(CGPreflightScreenCaptureAccess())")
        exit(0)
    }
    NSApplication.shared.setActivationPolicy(.accessory)
    if String(cString: m) == "run2" { Task { await run2() }; return }
    Task {
        say("START \(Date()) ax=\(AXIsProcessTrusted()) screen=\(CGPreflightScreenCaptureAccess()) \(state())")
        await testTarget()
        await testCalculator()
        await testTextEdit()
        await testVSCode()
        say("END \(state())")
        exit(0)
    }
}
