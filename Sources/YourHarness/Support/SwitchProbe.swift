import AppKit
import SwiftUI

/// Development: `YH_SWITCH_PROBE=<rounds>` switches through the sidebar's threads that many
/// rounds at launch, the way clicking a row does, and prints to stderr how long each switch
/// took: until the thread's view appeared, and until the main thread was free again (its
/// first layout done). `YH_SWITCH_ANIMATED=0` switches without the sidebar's animation.
/// Off unless set.
@MainActor
enum SwitchProbe {
    private static var appeared: [UUID: CheckedContinuation<Void, Never>] = [:]
    static let isOn = ProcessInfo.processInfo.environment["YH_SWITCH_PROBE"] != nil
        || ProcessInfo.processInfo.environment["YH_BLANK_WATCH"] == "1"
    /// From each transcript while the probe runs: rows at least partly in view, and its
    /// scroll offset, content and viewport heights. For telling a blank screen.
    static var visibleRows: [UUID: Int] = [:]
    static var geometry: [UUID: (offset: CGFloat, content: CGFloat, container: CGFloat)] = [:]
    private static var blanks = 0

    /// After a switch has settled: the thread has rows but none is in view, or another of
    /// the app's windows is on screen over the main one. Either is the empty main area.
    static func checkBlank(_ thread: ThreadModel, label: String) {
        let rows = thread.transcriptDisplayRows.count
        let visible = visibleRows[thread.id] ?? -1
        var problems: [String] = []
        if rows > 0, visible <= 0 {
            let g = geometry[thread.id]
            problems.append("no rows in view (\(rows) rows, \(visible) visible, offset \(Int(g?.offset ?? -1)) content \(Int(g?.content ?? -1)) viewport \(Int(g?.container ?? -1)))")
        }
        // What's drawn, whatever layout says: rows can be in view and still not show.
        let ink = conversationInk()
        FileHandle.standardError.write(Data(String(format: "check %@ ink %.4f, %d rows, %d in view\n", label, ink ?? -1, rows, visible).utf8))
        if rows > 0, let ink, ink < 0.002 {
            problems.append(String(format: "nothing drawn in the conversation (%.4f of it inked, %d rows, %d in view)", ink, rows, visible))
        }
        if let main = NSApp.windows.first(where: { $0.frameAutosaveName == "main" }) {
            for window in NSApp.windows where window !== main && window.isVisible && window.occlusionState.contains(.visible)
                && window.frame.intersects(main.frame) && !(window is NSPanel && window.frame.minX < -10_000)
                && !String(describing: type(of: window)).contains("Toolbar") {
                problems.append("window over it: \(type(of: window)) “\(window.title)” at \(window.frame)")
            }
        }
        guard !problems.isEmpty else { return }
        blanks += 1
        FileHandle.standardError.write(Data("BLANK \(label) “\(thread.title)”: \(problems.joined(separator: "; "))\n".utf8))
        if let folder = ProcessInfo.processInfo.environment["YH_SWITCH_PROBE_SHOTS"], blanks <= 5 {
            _ = WindowSnapshot.write(to: "\(folder)/blank-\(blanks).png")
        }
    }

    /// From the thread view, when it's on screen.
    static func threadAppeared(_ id: UUID) {
        appeared.removeValue(forKey: id)?.resume()
    }

    /// Wheel events over the transcript, as a trackpad sends them: up for positive points.
    static func scrollUp(points: Int) async {
        guard let window = NSApp.windows.first(where: { $0.frameAutosaveName == "main" }) else { return }
        let location = CGPoint(x: window.frame.minX + window.frame.width * 0.6,
                               y: (NSScreen.screens.first?.frame.height ?? 0) - (window.frame.minY + window.frame.height * 0.5))
        let step = points >= 0 ? 40 : -40
        for _ in 0..<max(1, abs(points) / 40) {
            guard let cg = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: Int32(step), wheel2: 0, wheel3: 0) else { continue }
            cg.location = location
            if let event = NSEvent(cgEvent: cg) { window.sendEvent(event) }
            try? await Task.sleep(for: .milliseconds(20))
        }
    }

    /// How much of the conversation area differs from its background: the main window drawn,
    /// the middle of the detail column between the toolbar and the composer.
    static func conversationInk() -> Double? {
        guard let window = NSApp.windows.first(where: { $0.frameAutosaveName == "main" }),
              let view = window.contentView?.superview ?? window.contentView,
              let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)
        else { return nil }
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let width = bitmap.pixelsWide, height = bitmap.pixelsHigh
        func luminance(_ x: Int, _ y: Int) -> CGFloat? {
            guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) else { return nil }
            return 0.3 * color.redComponent + 0.59 * color.greenComponent + 0.11 * color.blueComponent
        }
        // The canvas, right of the transcript column.
        guard let base = luminance(Int(Double(width) * 0.97), height / 2) else { return nil }
        var inked = 0, total = 0
        for y in stride(from: Int(Double(height) * 0.12), to: Int(Double(height) * 0.85), by: 2) {
            for x in stride(from: Int(Double(width) * 0.3), to: Int(Double(width) * 0.9), by: 2) {
                total += 1
                // Text, not the faint fill of a card.
                if let value = luminance(x, y), abs(value - base) > 0.3 { inked += 1 }
            }
        }
        return total == 0 ? nil : Double(inked) / Double(total)
    }

    /// `YH_BLANK_WATCH=1`: four times a second, whether the open thread has rows but none
    /// in view; prints when that starts (after half a second of it) and how long it lasted.
    private static func watch(app: AppModel) {
        Task {
            var since: ContinuousClock.Instant?
            var reported = false
            var tick = 0
            var lastInk = 1.0
            while true {
                try? await Task.sleep(for: .milliseconds(250))
                guard let thread = app.selectedThread, app.overviewProjectID == nil else { continue }
                let rows = thread.transcriptDisplayRows.count, visible = visibleRows[thread.id] ?? -1
                let g = geometry[thread.id]
                let state = "“\(thread.title)” \(thread.phase == .running ? "working" : "idle"), \(rows) rows, \(visible) in view, ink \(String(format: "%.4f", lastInk)), offset \(Int(g?.offset ?? -1)) content \(Int(g?.content ?? -1)) viewport \(Int(g?.container ?? -1))"
                tick += 1
                if tick % 4 == 0, rows > 0, let ink = conversationInk() { lastInk = ink }
                if rows > 0, visible <= 0 || lastInk < 0.002 {
                    let start = since ?? .now
                    since = start
                    if !reported, ContinuousClock.now - start > .milliseconds(500) {
                        reported = true
                        FileHandle.standardError.write(Data("BLANK watch: no rows in view: \(state)\n".utf8))
                        if let folder = ProcessInfo.processInfo.environment["YH_SWITCH_PROBE_SHOTS"] {
                            blanks += 1
                            _ = WindowSnapshot.write(to: "\(folder)/watch-\(blanks).png")
                        }
                    }
                } else if let start = since {
                    if reported { FileHandle.standardError.write(Data("BLANK watch: rows back after \(ContinuousClock.now - start): \(state)\n".utf8)) }
                    since = nil
                    reported = false
                }
            }
        }
    }

    static func runIfRequested(app: AppModel) {
        if ProcessInfo.processInfo.environment["YH_BLANK_WATCH"] == "1" { watch(app: app) }
        // `YH_BLANK_SCROLL=<seconds>`: then scrolls the open thread up a screenful with
        // trackpad-like wheel events, as reading back while Claude works does.
        if let delay = ProcessInfo.processInfo.environment["YH_BLANK_SCROLL"].flatMap(Double.init) {
            Task {
                try? await Task.sleep(for: .seconds(delay))
                await scrollUp(points: 600)
                FileHandle.standardError.write(Data("scrolled up\n".utf8))
            }
        }
        // `YH_FULL_SCREEN=1`: the main window goes full screen at launch.
        if ProcessInfo.processInfo.environment["YH_FULL_SCREEN"] == "1" {
            Task {
                try? await Task.sleep(for: .seconds(1))
                NSApp.windows.first { $0.frameAutosaveName == "main" }?.toggleFullScreen(nil)
            }
        }
        guard let rounds = ProcessInfo.processInfo.environment["YH_SWITCH_PROBE"].flatMap(Int.init) else { return }
        Task {
            try? await Task.sleep(for: .seconds(3))
            if let window = NSApp.windows.first(where: { $0.frameAutosaveName == "main" }) {
                // Top-left screen coordinates, as `screencapture -R` takes them.
                let top = (NSScreen.screens.first?.frame.height ?? 0) - window.frame.maxY
                FileHandle.standardError.write(Data("window id \(window.windowNumber) at \(Int(window.frame.minX)),\(Int(top)) size \(Int(window.frame.width))x\(Int(window.frame.height))\n".utf8))
            }
            // `YH_SWITCH_PROBE_THREADS=<n>` goes round the first n only, as going back and
            // forth between a few threads does.
            let limit = ProcessInfo.processInfo.environment["YH_SWITCH_PROBE_THREADS"].flatMap(Int.init) ?? .max
            let ids = Array(app.visibleThreadIDs.prefix(limit))
            var lines: [String] = [], firsts: [Double] = [], agains: [Double] = []
            for round in 0..<rounds {
                for id in ids where id != app.selectedThreadID {
                    let clock = ContinuousClock(), start = clock.now, cpuStart = mainThreadCPU()
                    await withCheckedContinuation { continuation in
                        appeared[id] = continuation
                        if ProcessInfo.processInfo.environment["YH_SWITCH_ANIMATED"] == "0" {
                            app.selectedThreadID = id
                        } else {
                            withAnimation(SidebarMetrics.selectionSpring) { app.selectedThreadID = id }
                        }
                    }
                    let shown = clock.now - start
                    // Free again: the first layout and drawing are done.
                    await withCheckedContinuation { continuation in
                        DispatchQueue.main.async { DispatchQueue.main.async { continuation.resume() } }
                    }
                    let idle = clock.now - start
                    let cpu = mainThreadCPU() - cpuStart
                    let ms = { (d: Duration) in Double(d.components.attoseconds) / 1e15 + Double(d.components.seconds) * 1000 }
                    if round == 0 { firsts.append(cpu) } else { agains.append(cpu) }
                    let shownThread = app.threads.first { $0.id == id }
                    let title = (shownThread?.title ?? "?") + " · rows \(shownThread?.transcriptDisplayRows.count ?? 0), turns shown \(shownThread?.shownTurns ?? 0)"
                    let line = String(format: "round %d  shown %6.0f ms  idle %6.0f ms  main thread busy %5.0f ms  %@", round + 1, ms(shown), ms(idle), cpu, title)
                    lines.append(line)
                    // As it goes, so a profiler can be started on the round you want.
                    FileHandle.standardError.write(Data(("switch " + line + "\n").utf8))
                    try? await Task.sleep(for: .milliseconds(300))
                    if let shownThread { checkBlank(shownThread, label: "round \(round + 1)") }
                    // `YH_SWITCH_PROBE_HOLD=<ms>`: stays put that long, for a screenshot from outside.
                    if let hold = ProcessInfo.processInfo.environment["YH_SWITCH_PROBE_HOLD"].flatMap(Int.init) {
                        try? await Task.sleep(for: .milliseconds(hold))
                    }
                    // `YH_SWITCH_PROBE_SCROLL=<points>`: read back that far before leaving, as
                    // you do in a long thread; the next visit goes back there.
                    if let points = ProcessInfo.processInfo.environment["YH_SWITCH_PROBE_SCROLL"].flatMap(Int.init) {
                        await scrollUp(points: points * (round % 2 == 0 ? 1 : 2))
                        try? await Task.sleep(for: .milliseconds(400))
                    }
                }
            }
            let summary = { (name: String, values: [Double]) -> String in
                guard !values.isEmpty else { return "" }
                let sorted = values.sorted()
                return String(format: "%@: median %.0f ms, worst %.0f ms", name, sorted[sorted.count / 2], sorted.last!)
            }
            // Busy time is what to go by: a window behind others is drawn late, so wall time
            // in a background run is mostly waiting.
            let report = lines + [summary("first visit, main thread busy", firsts), summary("visited before, main thread busy", agains),
                                  "blank after a switch: \(blanks) of \(lines.count)"]
            FileHandle.standardError.write(Data(("SWITCH PROBE\n" + report.joined(separator: "\n") + "\n").utf8))
        }
    }
}

/// CPU time the calling thread has used, in milliseconds.
private func mainThreadCPU() -> Double {
    var time = timespec()
    clock_gettime(CLOCK_THREAD_CPUTIME_ID, &time)
    return Double(time.tv_sec) * 1000 + Double(time.tv_nsec) / 1_000_000
}
