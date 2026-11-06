import AppKit
import Darwin
import WebKit

/// The host's half of a performance run (`YH_PERF=<results folder>`). The page drives the
/// run (web/ui/src/probe.ts) and measures its own frames; this side sends it real events, as
/// the window server would, reads memory and CPU of every process that draws it, says when a
/// run can't be trusted, and writes the results.
@MainActor
final class HostProbe {
    let folder: String
    weak var bridge: Bridge?
    let window: NSWindow
    let webView: WKWebView
    private let launched = Date()
    private var phases: [[String: Any]] = []
    private var clicks: [[String: Any]] = []
    private var lastCPU: [Int32: Double] = [:]
    private var lastCPUAt = Date()

    init(folder: String, bridge: Bridge, window: NSWindow, webView: WKWebView) {
        self.folder = folder
        self.bridge = bridge
        self.window = window
        self.webView = webView
        try? FileManager.default.createDirectory(atPath: folder, withIntermediateDirectories: true)
    }

    func windowReady() {
        guard let screen = window.screen ?? NSScreen.main else { return }
        if env["YH_PERF_FRONT"] == "1" {
            // As you use it: in front, filling the screen, above everything.
            window.setFrame(screen.visibleFrame, display: true)
            window.level = .floating
            window.collectionBehavior.formUnion([.canJoinAllSpaces, .fullScreenAuxiliary])
            NSApp.activate()
            window.makeKeyAndOrderFront(nil)
        } else {
            // Below the screen, where it can't take your focus or be seen: the same as the
            // native app's probe without YH_PERF_FRONT.
            let size = NSSize(width: 1440, height: 900)
            window.setFrame(NSRect(x: screen.frame.minX, y: screen.frame.minY - size.height - 200, width: size.width, height: size.height), display: true)
            window.orderFrontRegardless()
        }
        log("PERF window \(Int(window.frame.width))x\(Int(window.frame.height)), screen \(screen.maximumFramesPerSecond) Hz, front \(env["YH_PERF_FRONT"] == "1")")
    }

    func handle(_ method: String, _ p: [String: Any]) async throws -> Any? {
        switch method {
        case "config":
            return ["seconds": Double(env["YH_PERF_SECONDS"] ?? "") ?? 30,
                    "phases": env["YH_PERF_PHASES"] ?? "",
                    "screenHz": (window.screen ?? NSScreen.main)?.maximumFramesPerSecond ?? 60,
                    "rate": env["YH_PERF_RATE"] ?? "1",
                    "front": env["YH_PERF_FRONT"] == "1"]
        case "now":
            return Self.epochMs()
        case "wheel":
            return wheel(x: p["x"] as? Double ?? 600, y: p["y"] as? Double ?? 400, dy: Int32(p["dy"] as? Double ?? 0))
        case "click":
            return click(x: p["x"] as? Double ?? 0, y: p["y"] as? Double ?? 0)
        case "move":
            move(x: p["x"] as? Double ?? 0, y: p["y"] as? Double ?? 0)
            return true
        case "key":
            return key(p["s"] as? String ?? "")
        case "sample":
            return sample()
        case "phase":
            var result = p
            result["valid"] = isValid()
            phases.append(result)
            write()
            return true
        case "clicks":
            clicks.append(contentsOf: p["list"] as? [[String: Any]] ?? [])
            write()
            return true
        case "log":
            log("PERF " + (p["s"] as? String ?? ""))
            return true
        case "done":
            write()
            log("PERF DONE")
            if env["YH_PERF_QUIT"] == "1" {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { NSApp.terminate(nil) }
            }
            return true
        default:
            throw HostError("unknown probe method \(method)")
        }
    }

    static func epochMs() -> Double { Date().timeIntervalSince1970 * 1000 }

    // MARK: Events

    private func windowPoint(x: Double, y: Double) -> NSPoint {
        let point = webView.isFlipped ? NSPoint(x: x, y: y) : NSPoint(x: x, y: webView.bounds.height - y)
        return webView.convert(point, to: nil)
    }

    /// In CG's global coordinates (top left of the main display), where events live.
    private func globalPoint(_ windowPoint: NSPoint) -> CGPoint {
        let screen = window.convertPoint(toScreen: windowPoint)
        let height = NSScreen.screens.first?.frame.height ?? 0
        return CGPoint(x: screen.x, y: height - screen.y)
    }

    /// A pixel scroll, as a trackpad sends: through the window, which finds what's under it.
    private func wheel(x: Double, y: Double, dy: Int32) -> Double {
        let point = windowPoint(x: x, y: y)
        guard let cg = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: dy, wheel2: 0, wheel3: 0) else { return 0 }
        cg.location = globalPoint(point)
        cg.setIntegerValueField(.mouseEventWindowUnderMousePointer, value: Int64(window.windowNumber))
        cg.setIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent, value: Int64(window.windowNumber))
        guard let event = NSEvent(cgEvent: cg) else { return 0 }
        let sent = Self.epochMs()
        if event.window === window {
            window.sendEvent(event)
        } else {
            webView.scrollWheel(with: event)
        }
        return sent
    }

    private func mouse(_ type: NSEvent.EventType, _ point: NSPoint, clickCount: Int = 1) -> NSEvent? {
        NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                           windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: clickCount, pressure: type == .leftMouseDown ? 1 : 0)
    }

    private var clicksLogged = 0
    private func click(x: Double, y: Double) -> Double {
        let point = windowPoint(x: x, y: y)
        if clicksLogged < 3 {
            clicksLogged += 1
            let hit = window.contentView?.hitTest(window.contentView!.convert(point, from: nil))
            log("PERF click at \(point) window \(window.frame) screen \(String(describing: window.screen?.frame)) visible \(window.isVisible) key \(window.isKeyWindow) hit \(hit.map { String(describing: type(of: $0)) } ?? "nil")")
        }
        let sent = Self.epochMs()
        if let down = mouse(.leftMouseDown, point) { window.sendEvent(down) }
        if let up = mouse(.leftMouseUp, point) { window.sendEvent(up) }
        return sent
    }

    private func move(x: Double, y: Double) {
        window.acceptsMouseMovedEvents = true
        if let event = mouse(.mouseMoved, windowPoint(x: x, y: y), clickCount: 0) { window.sendEvent(event) }
    }

    private func key(_ text: String) -> Double {
        let sent = Self.epochMs()
        for character in text {
            let string = String(character)
            let code: UInt16 = string == "\r" ? 36 : 0
            for type in [NSEvent.EventType.keyDown, .keyUp] {
                if let event = NSEvent.keyEvent(with: type, location: .zero, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                                windowNumber: window.windowNumber, context: nil, characters: string,
                                                charactersIgnoringModifiers: string, isARepeat: false, keyCode: code) {
                    window.sendEvent(event)
                }
            }
        }
        return sent
    }

    // MARK: Memory and CPU

    /// Every process drawing this window: the app, its web content process, and WebKit's
    /// GPU and networking processes started since launch.
    private func processes() -> [(String, Int32)] {
        var list: [(String, Int32)] = [("host", getpid())]
        if let web = webView.value(forKey: "_webProcessIdentifier") as? Int32, web > 0 { list.append(("web", web)) }
        let count = proc_listallpids(nil, 0)
        var pids = [Int32](repeating: 0, count: Int(count) + 64)
        let found = proc_listallpids(&pids, Int32(pids.count * MemoryLayout<Int32>.size))
        let since = launched.timeIntervalSince1970 - 2
        for pid in pids.prefix(Int(found)) where pid > 0 {
            var info = proc_bsdinfo()
            guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, Int32(MemoryLayout<proc_bsdinfo>.size)) > 0,
                  Double(info.pbi_start_tvsec) >= since, info.pbi_uid == getuid() else { continue }
            let name = withUnsafeBytes(of: info.pbi_name) { String(decoding: $0.prefix { $0 != 0 }, as: UTF8.self) }
            if name.contains("WebKit.GPU") { list.append(("gpu", pid)) }
            else if name.contains("WebKit.Networking") { list.append(("net", pid)) }
        }
        return list
    }

    private func usage(_ pid: Int32) -> rusage_info_v4? {
        var info = rusage_info_v4()
        let result = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { proc_pid_rusage(pid, RUSAGE_INFO_V4, $0) }
        }
        return result == 0 ? info : nil
    }

    private static let timebase: Double = {
        var info = mach_timebase_info_data_t()
        mach_timebase_info(&info)
        return Double(info.numer) / Double(info.denom)
    }()

    /// Footprint in MB and CPU % since the last sample, per process and in all.
    private func sample() -> [String: Any] {
        let now = Date()
        let elapsed = max(0.001, now.timeIntervalSince(lastCPUAt))
        var result: [String: Any] = [:]
        var totalMB = 0.0, totalCPU = 0.0
        for (name, pid) in processes() {
            guard let info = usage(pid) else { continue }
            let mb = Double(info.ri_phys_footprint) / 1_048_576
            let cpuSeconds = Double(info.ri_user_time + info.ri_system_time) * Self.timebase / 1e9
            let cpu = lastCPU[pid].map { (cpuSeconds - $0) / elapsed * 100 } ?? 0
            lastCPU[pid] = cpuSeconds
            result[name + "MB"] = ((result[name + "MB"] as? Double) ?? 0) + mb
            result[name + "CPU"] = ((result[name + "CPU"] as? Double) ?? 0) + cpu
            totalMB += mb
            totalCPU += cpu
        }
        lastCPUAt = now
        result["totalMB"] = totalMB
        result["totalCPU"] = totalCPU
        result["valid"] = isValid()
        return result
    }

    // MARK: Validity

    /// A locked screen or a sleeping display stops frames with nothing wrong in the app.
    private func isValid() -> Bool {
        let locked = (CGSessionCopyCurrentDictionary() as? [String: Any])?["CGSSessionScreenIsLocked"] as? Bool ?? false
        let asleep = CGDisplayIsAsleep(CGMainDisplayID()) != 0
        if locked || asleep { log("PERF INVALID locked=\(locked) displayAsleep=\(asleep)") }
        return !locked && !asleep
    }

    private func write() {
        func save(_ object: Any, _ name: String) {
            guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys]) else { return }
            try? data.write(to: URL(fileURLWithPath: folder).appendingPathComponent(name))
        }
        save(phases, "phases.json")
        save(clicks, "clicks.json")
    }
}
