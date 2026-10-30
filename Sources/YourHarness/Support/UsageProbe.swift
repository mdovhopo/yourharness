import AppKit
import HarnessCore
import Synchronization

/// `YH_USAGE_PROBE=1` (with `YH_OPEN_USAGE=1`): once the usage view has its report, moves
/// the pointer across the stacked chart and then scrolls over it, as a mouse does (mouse-moved
/// and wheel events, handed to the window, so nothing on screen moves and nothing asks for
/// permission), and prints how long the main thread was held: its CPU per step and the
/// longest it didn't answer, as a watchdog on another thread saw it. PASS is under 100 ms.
/// It waits `YH_USAGE_PROBE_WAIT` seconds (default 3) before starting, for attaching
/// Time Profiler.
@MainActor
enum UsageProbe {
    static let isOn = ProcessInfo.processInfo.environment["YH_USAGE_PROBE"] != nil
    /// The stacked chart's frame, in the window's top-left coordinates.
    static var chartFrame: CGRect?
    private static var hovers: [WeakHover] = []

    private struct WeakHover {
        weak var hover: AnyObject?
        let day: () -> Date?
    }

    static func register<Hover: AnyObject>(_ hover: Hover, day: @escaping (Hover) -> Date?) {
        guard isOn else { return }
        hovers.append(WeakHover(hover: hover) { [weak hover] in hover.flatMap(day) })
    }

    private static var hoveredDays: Set<Date> {
        Set(hovers.compactMap { $0.day() })
    }

    static func runIfRequested(app: AppModel) {
        guard isOn else { return }
        Task { await run(app: app) }
    }

    private static func log(_ line: String) {
        FileHandle.standardError.write(Data("USAGE PROBE \(line)\n".utf8))
    }

    private static func run(app: AppModel) async {
        let usage = app.tokenUsage
        for _ in 0..<300 {
            if usage.isOpen, !usage.isReading, usage.report.total.messages > 0, chartFrame != nil { break }
            try? await Task.sleep(for: .milliseconds(100))
        }
        guard let frame = chartFrame, let window = NSApp.windows.first(where: { $0.frameAutosaveName == "main" }),
              let content = window.contentView
        else { return log("FAIL: the usage view's chart never showed") }
        let wait = ProcessInfo.processInfo.environment["YH_USAGE_PROBE_WAIT"].flatMap(Double.init) ?? 3
        log("ready: pid \(ProcessInfo.processInfo.processIdentifier), \(usage.range.rawValue), \(usage.report.days.count) days, chart \(frame.integral); starting in \(wait)s")
        try? await Task.sleep(for: .seconds(wait))

        func windowPoint(_ x: CGFloat, _ y: CGFloat) -> NSPoint {
            NSPoint(x: x, y: content.bounds.height - y)
        }
        func move(to point: NSPoint) {
            guard let event = NSEvent.mouseEvent(with: .mouseMoved, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                                 windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 0, pressure: 0)
            else { return }
            window.sendEvent(event)
        }
        func scroll(by step: Int, at point: NSPoint) {
            let screen = window.convertPoint(toScreen: point)
            guard let cg = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: Int32(step), wheel2: 0, wheel3: 0) else { return }
            cg.location = CGPoint(x: screen.x, y: (NSScreen.screens.first?.frame.height ?? 0) - screen.y)
            if let event = NSEvent(cgEvent: cg) { window.sendEvent(event) }
        }

        // Mouse-moved events reach hover tracking only in a window that takes them.
        window.acceptsMouseMovedEvents = true
        var results: [String] = []
        var passed = true
        func phase(_ name: String, steps: Int, _ step: (Int) -> Void) async {
            let watchdog = Watchdog()
            var days = Set<Date>()
            var busiest = 0.0
            let start = mainThreadCPU()
            for index in 0..<steps {
                let before = mainThreadCPU()
                step(index)
                // A frame, so what the step changed is drawn before the next.
                try? await Task.sleep(for: .milliseconds(16))
                busiest = max(busiest, mainThreadCPU() - before)
                days.formUnion(hoveredDays)
                if ProcessInfo.processInfo.environment["YH_USAGE_PROBE_VERBOSE"] != nil, index % 20 == 0 {
                    log("\(name) step \(index): \(hoveredDays.map { $0.formatted(date: .abbreviated, time: .omitted) })")
                }
            }
            try? await Task.sleep(for: .milliseconds(200))
            let stall = await watchdog.stop()
            let total = mainThreadCPU() - start
            let ok = stall < 100
            passed = passed && ok
            results.append(String(format: "%@  %@: %d steps, %d days hovered, main thread busy %.0f ms (%.1f ms a step, at most %.1f), longest stall %.0f ms",
                                  ok ? "PASS" : "FAIL", name, steps, days.count, total, total / Double(steps), busiest, stall))
        }

        // Across the chart and back, a few pixels a step.
        let steps = 160
        await phase("hover across the chart", steps: steps) { index in
            let t = Double(index < steps / 2 ? index : steps - index) / Double(steps / 2)
            move(to: windowPoint(frame.minX + 50 + (frame.width - 70) * t, frame.midY))
        }
        // The pointer stays over the chart while the view scrolls under it: what's under it
        // changes, as with a trackpad.
        let point = windowPoint(frame.midX, frame.midY)
        await phase("scroll over the chart", steps: 120) { index in
            scroll(by: index < 60 ? -30 : 30, at: point)
            move(to: windowPoint(frame.minX + 50 + (frame.width - 70) * Double(index % 60) / 60, frame.midY))
        }
        log("\n" + results.joined(separator: "\n") + "\n" + (passed ? "PASS" : "FAIL"))
    }
}

/// Pings the main thread from another every few milliseconds and keeps the longest it took
/// to answer. The probes' measure of a stall.
final class Watchdog: Sendable {
    private let state = Mutex((running: true, done: false, longest: 0.0))

    init() {
        let thread = Thread { [self] in
            while state.withLock({ $0.running }) {
                let answered = DispatchSemaphore(value: 0)
                let sent = DispatchTime.now()
                DispatchQueue.main.async { answered.signal() }
                answered.wait()
                let waited = Double(DispatchTime.now().uptimeNanoseconds - sent.uptimeNanoseconds) / 1_000_000
                state.withLock { $0.longest = max($0.longest, waited) }
                Thread.sleep(forTimeInterval: 0.004)
            }
            state.withLock { $0.done = true }
        }
        thread.qualityOfService = .userInteractive
        thread.start()
    }

    /// The longest wait, in milliseconds. Waits without holding the main thread, which the
    /// last ping may be waiting for.
    func stop() async -> Double {
        state.withLock { $0.running = false }
        while !state.withLock({ $0.done }) { try? await Task.sleep(for: .milliseconds(5)) }
        return state.withLock { $0.longest }
    }

    /// CPU time the calling thread has used, in milliseconds.
    static func threadCPU() -> Double {
        mainThreadCPU()
    }
}

/// CPU time the calling thread has used, in milliseconds.
private func mainThreadCPU() -> Double {
    var time = timespec()
    clock_gettime(CLOCK_THREAD_CPUTIME_ID, &time)
    return Double(time.tv_sec) * 1000 + Double(time.tv_nsec) / 1_000_000
}
