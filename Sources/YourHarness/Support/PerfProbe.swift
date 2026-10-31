import AppKit
import Darwin
import QuartzCore
import os

/// `YH_PERF_PROBE=<folder>`: runs the app through the heaviest use there is, phase by phase,
/// and records frames, CPU and memory: `<folder>/phases.json` per phase, `<folder>/seconds.csv`
/// per second. For `scripts/perf-run.sh`, which sets up the state and the fake `claude`.
/// `YH_PERF_PHASES=<a,b>` runs only those; `YH_PERF_SECONDS=<n>` sets each phase's length;
/// `YH_PERF_WAIT=<seconds>` holds before each phase and prints its name, for Time Profiler.
///
/// Frames are counted from a display link on the main thread, asking for the screen's top
/// rate: a vsync with no callback is a frame the main thread wasn't there for. That's what
/// the app can miss; the window server's own drops don't show here.
///
/// Macs don't draw a covered window, so results would flatter: the window floats, letting
/// clicks through, almost entirely below the screen. Your pointer isn't over it, so hovering
/// isn't really part of that run. `YH_PERF_FRONT=1` is the true test, and takes over the Mac
/// while it runs: in front, filling the screen, moving the real pointer over the transcript.
@MainActor
final class PerfProbe: NSObject {
    private static var probe: PerfProbe?

    static func runIfRequested(app: AppModel) {
        guard let folder = ProcessInfo.processInfo.environment["YH_PERF_PROBE"] else { return }
        let probe = PerfProbe(app: app, folder: folder)
        self.probe = probe
        Task { await probe.run() }
    }

    private let app: AppModel
    private let folder: String
    private let env = ProcessInfo.processInfo.environment
    private var link: CADisplayLink?
    private var lastTimestamp: CFTimeInterval = 0
    private var period: CFTimeInterval = 1.0 / 120
    private var frames = FrameStats()
    private var second = FrameStats()
    private var phaseName = "launch"
    private var results: [[String: Any]] = []
    private var csv = "elapsed,phase,fps,worst_ms,main_cpu_pct,process_cpu_pct,footprint_mb,threads\n"
    private let started = CACurrentMediaTime()
    private var frameWaiters: [CheckedContinuation<CFTimeInterval, Never>] = []
    /// How long each kind of click took to show on screen, in milliseconds.
    private var clicks: [String: [Double]] = [:]

    private init(app: AppModel, folder: String) {
        self.app = app
        self.folder = folder
    }

    struct FrameStats {
        var vsyncs = 0, missed = 0
        var worst: CFTimeInterval = 0
        var longFrames: [CFTimeInterval] = []
        var hitches = 0, stalls100 = 0, stalls250 = 0, stalls1000 = 0

        mutating func add(delta: CFTimeInterval, period: CFTimeInterval) {
            let spent = max(1, Int((delta / period).rounded()))
            vsyncs += spent
            missed += spent - 1
            worst = max(worst, delta)
            if spent > 1 { hitches += 1; longFrames.append(delta) }
            if delta > 0.1 { stalls100 += 1 }
            if delta > 0.25 { stalls250 += 1 }
            if delta > 1 { stalls1000 += 1 }
        }
    }

    private static let lastFrame = OSAllocatedUnfairLock(initialState: CACurrentMediaTime())

    /// Off the main thread: when frames stop for 3 s, samples the process, so a freeze says
    /// where it is. Up to five a run.
    private func watchForFreezes() {
        let folder = folder, pid = ProcessInfo.processInfo.processIdentifier
        Thread.detachNewThread {
            var taken = 0
            while taken < 5 {
                Thread.sleep(forTimeInterval: 0.5)
                guard CACurrentMediaTime() - Self.lastFrame.withLock({ $0 }) > 3 else { continue }
                taken += 1
                let sample = Process()
                sample.executableURL = URL(fileURLWithPath: "/usr/bin/sample")
                sample.arguments = ["\(pid)", "3", "-file", "\(folder)/freeze-\(taken).sample"]
                try? sample.run()
                sample.waitUntilExit()
                FileHandle.standardError.write(Data("PERF freeze sampled: freeze-\(taken).sample\n".utf8))
                Thread.sleep(forTimeInterval: 5)
            }
        }
    }

    private func log(_ line: String) {
        FileHandle.standardError.write(Data("PERF \(line)\n".utf8))
    }

    // MARK: Readings

    private static func footprintMB() -> Double {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
        let result = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count) }
        }
        return result == KERN_SUCCESS ? Double(info.phys_footprint) / 1_048_576 : 0
    }

    private static func processCPU() -> CFTimeInterval {
        var usage = rusage()
        getrusage(RUSAGE_SELF, &usage)
        func seconds(_ t: timeval) -> Double { Double(t.tv_sec) + Double(t.tv_usec) / 1_000_000 }
        return seconds(usage.ru_utime) + seconds(usage.ru_stime)
    }

    /// Called on the main thread: this thread's CPU time so far.
    private static func threadCPU() -> CFTimeInterval {
        var info = thread_basic_info()
        var count = mach_msg_type_number_t(MemoryLayout<thread_basic_info>.size / MemoryLayout<natural_t>.size)
        let port = pthread_mach_thread_np(pthread_self())
        let result = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { thread_info(port, thread_flavor_t(THREAD_BASIC_INFO), $0, &count) }
        }
        guard result == KERN_SUCCESS else { return 0 }
        return Double(info.user_time.seconds + info.system_time.seconds)
            + Double(info.user_time.microseconds + info.system_time.microseconds) / 1_000_000
    }

    private static func threadCount() -> Int {
        var list: thread_act_array_t?
        var count: mach_msg_type_number_t = 0
        guard task_threads(mach_task_self_, &list, &count) == KERN_SUCCESS, let list else { return 0 }
        vm_deallocate(mach_task_self_, vm_address_t(UInt(bitPattern: list)), vm_size_t(Int(count) * MemoryLayout<thread_t>.size))
        return Int(count)
    }

    // MARK: Frames

    @objc private func tick(_ link: CADisplayLink) {
        if link.duration > 0 { period = link.duration }
        if lastTimestamp > 0 {
            let delta = link.timestamp - lastTimestamp
            frames.add(delta: delta, period: period)
            // Wherever it falls: a freeze can run past the end of its phase.
            if delta > 0.25 { log(String(format: "stall %.0f ms in or after %@", delta * 1000, phaseName)) }
            second.add(delta: delta, period: period)
        }
        lastTimestamp = link.timestamp
        let waiting = frameWaiters
        frameWaiters = []
        // The frame's own time: resuming goes through the main actor's queue, which can be seconds
        // behind in a burst while frames and clicks aren't.
        let now = CACurrentMediaTime()
        for waiter in waiting { waiter.resume(returning: now) }
        Self.lastFrame.withLock { $0 = CACurrentMediaTime() }
    }

    private var mainWindow: NSWindow? { NSApp.windows.first { $0.frameAutosaveName == "main" } }

    private func startFrames() -> Bool {
        guard let window = mainWindow, let view = window.contentView else { return false }
        if env["YH_PERF_FRONT"] != "1" {
            window.level = .floating
            window.alphaValue = 0.03
            window.ignoresMouseEvents = true
            // Past the bottom of the screen but for a strip, so it counts as on screen and is
            // drawn, without being over what you're doing: nearly transparent still showed.
            if let screen = window.screen ?? NSScreen.main {
                window.setFrameOrigin(NSPoint(x: screen.frame.minX, y: screen.frame.minY - window.frame.height + 2))
            }
            window.orderFrontRegardless()
        } else {
            // As you use it: in front, filling the screen, the real pointer over it.
            if let screen = window.screen ?? NSScreen.main { window.setFrame(screen.visibleFrame, display: true) }
            // Above everything: a window covered by another isn't drawn, and that flatters the
            // numbers (occlusionState can still say it's visible).
            window.level = .floating
            NSApp.activate()
            window.makeKeyAndOrderFront(nil)
        }
        let link = view.displayLink(target: self, selector: #selector(tick))
        let top = Float(window.screen?.maximumFramesPerSecond ?? 120)
        link.preferredFrameRateRange = CAFrameRateRange(minimum: top, maximum: top, preferred: top)
        link.add(to: .main, forMode: .common)
        self.link = link
        return true
    }

    // MARK: Actions

    private var threads: [ThreadModel] {
        app.threads.filter { $0.title.hasPrefix("Huge") }.sorted { $0.title < $1.title }
    }

    /// Straight to the scroll view under the middle of the transcript: the window is below
    /// the screen, where an event's location wouldn't find it.
    private func wheel(_ points: Int32) {
        guard let content = mainWindow?.contentView else { return }
        guard let cg = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: points, wheel2: 0, wheel3: 0),
              let event = NSEvent(cgEvent: cg) else { return }
        var view = content.hitTest(NSPoint(x: content.bounds.midX + 120, y: content.bounds.midY))
        while let current = view, !(current is NSScrollView) { view = current.superview }
        view?.scrollWheel(with: event)
    }

    /// A pointer resting over the transcript, moved now and then: content moving under it is
    /// a big part of what a real session does. `YH_PERF_POINTER=0` leaves it out.
    private func keepPointerOverTranscript() {
        guard env["YH_PERF_POINTER"] != "0", let window = mainWindow, let content = window.contentView else { return }
        window.acceptsMouseMovedEvents = true
        let front = env["YH_PERF_FRONT"] == "1"
        Task { @MainActor in
            while true {
                // Anywhere in the window: the sidebar's rows have hover too.
                let point = NSPoint(x: CGFloat.random(in: 20...(content.bounds.width - 20)), y: CGFloat.random(in: 150...(content.bounds.height - 100)))
                if let event = NSEvent.mouseEvent(with: .mouseMoved, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                                  windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 0, pressure: 0) {
                    window.sendEvent(event)
                }
                if front {
                    let screen = window.convertPoint(toScreen: point)
                    CGWarpMouseCursorPosition(CGPoint(x: screen.x, y: (NSScreen.screens.first?.frame.height ?? 0) - screen.y))
                }
                try? await Task.sleep(for: .seconds(front ? 2 : 0.5))
            }
        }
    }

    /// Scrolls for as long as the phase: long pulls up and back, as you do reading back.
    private func scrolling(until end: CFTimeInterval, speed: Int32 = 40) async {
        var direction: Int32 = 1, count = 0
        let id = app.selectedThreadID
        var lowest = id.flatMap { TranscriptDiagnostics.scroll[$0]?.offset } ?? 0, highest = lowest
        defer {
            // Scrolling that doesn't arrive would look like a perfect result.
            if highest - lowest < 100 { log("FAIL: scrolling moved the transcript only \(Int(highest - lowest)) pt") }
            else { log("scrolled across \(Int(highest - lowest)) pt") }
        }
        while CACurrentMediaTime() < end {
            wheel(speed * direction)
            count += 1
            if let id, let offset = TranscriptDiagnostics.scroll[id]?.offset {
                lowest = min(lowest, offset)
                highest = max(highest, offset)
            }
            if count % 200 == 0 { direction = -direction }
            try? await Task.sleep(for: .milliseconds(8))
        }
    }

    private func switching(among list: [ThreadModel], until end: CFTimeInterval, every seconds: Double = 1.5) async {
        var index = 0
        while CACurrentMediaTime() < end, !list.isEmpty {
            index = (index + 1) % list.count
            await switchTo(list[index])
            try? await Task.sleep(for: .seconds(seconds))
        }
    }

    @discardableResult private func nextFrame() async -> CFTimeInterval {
        await withCheckedContinuation { frameWaiters.append($0) }
    }

    /// Does what a click does, and times it until it has been on screen for a frame: the main
    /// thread free again after it, which is what a click feels like.
    private func click(_ label: String, _ action: () -> Void) async {
        let start = CACurrentMediaTime()
        action()
        await nextFrame()
        let shown = await nextFrame()
        let ms = (shown - start) * 1000
        clicks[label, default: []].append(ms)
        if ms > 100 { log(String(format: "slow click %@: %.0f ms", label, ms)) }
    }

    private func switchTo(_ thread: ThreadModel) async {
        let wasLoaded = !thread.entries.isEmpty
        await click(wasLoaded ? "switch-loaded" : "switch-unloaded") { app.selectedThreadID = thread.id }
        guard !wasLoaded else { return }
        // Then its history reading in, under a loading indicator.
        let start = CACurrentMediaTime()
        while thread.entries.isEmpty, CACurrentMediaTime() - start < 20 { await nextFrame() }
        clicks["history-shown", default: []].append((CACurrentMediaTime() - start) * 1000)
    }

    /// The things you click most, in turn, while everything's working.
    private func clicking(among list: [ThreadModel], until end: CFTimeInterval) async {
        var index = 0
        let pause: Duration = .milliseconds(700)
        while CACurrentMediaTime() < end, !list.isEmpty {
            index = (index + 1) % list.count
            let thread = list[index]
            await switchTo(thread)
            try? await Task.sleep(for: pause)
            let worked = thread.transcriptDisplayRows.last { if case .work = $0 { true } else { false } }
            if case .work(let work)? = worked {
                await click("expand-work") { thread.setWork(work, expanded: true) }
                try? await Task.sleep(for: pause)
                await click("collapse-work") { thread.setWork(work, expanded: false) }
                try? await Task.sleep(for: pause)
            }
            let shows = app.showsInspector
            await click("inspector") { app.setShowsInspector(!shows) }
            try? await Task.sleep(for: pause)
            await click("inspector") { app.setShowsInspector(shows) }
            try? await Task.sleep(for: pause)
            if let project = app.project(id: thread.projectID) {
                await click("project-page") { app.showOverview(of: project) }
                try? await Task.sleep(for: pause)
            }
            await click("new-thread") { app.newThreadInCurrentProject() }
            try? await Task.sleep(for: pause)
        }
    }

    private func startTurns(_ count: Int, seconds: Double) {
        for thread in threads.prefix(count) where thread.phase != .running {
            thread.send("PERF seconds=\(Int(seconds)) rate=\(env["YH_PERF_RATE"] ?? "1")")
        }
    }

    private func waitForTurnsToEnd() async {
        let limit = CACurrentMediaTime() + 60
        while threads.contains(where: { $0.phase == .running }), CACurrentMediaTime() < limit {
            try? await Task.sleep(for: .milliseconds(250))
        }
    }

    // MARK: The run

    private func run() async {
        try? FileManager.default.createDirectory(atPath: folder, withIntermediateDirectories: true)
        log("waiting for the main window")
        // The window comes up with the app.
        var waited = 0
        while mainWindow == nil {
            try? await Task.sleep(for: .milliseconds(50))
            waited += 1
            // SwiftUI sometimes doesn't name it. Named here, the rest of the probe (and the checks
            // it uses) can find it.
            if waited % 100 == 0, let window = NSApp.windows.first(where: { $0.isVisible && $0.canBecomeMain && !($0 is NSPanel) }) {
                window.setFrameAutosaveName("main")
                log("named the unnamed main window after \(waited / 20) s")
            }
        }
        guard startFrames() else { return log("FAIL: no window") }
        watchForFreezes()
        keepPointerOverTranscript()
        let length = Double(env["YH_PERF_SECONDS"] ?? "") ?? 30
        let only = env["YH_PERF_PHASES"].map { Set($0.split(separator: ",").map(String.init)) }
        let huge = threads
        guard let first = huge.first else { return log("FAIL: no threads named Huge") }
        log("pid \(ProcessInfo.processInfo.processIdentifier), \(huge.count) huge threads, screen \(mainWindow?.screen?.maximumFramesPerSecond ?? 0) Hz, low power \(ProcessInfo.processInfo.isLowPowerModeEnabled)")
        app.selectedThreadID = first.id

        let phases: [(String, (CFTimeInterval) async -> Void)] = [
            ("open-huge", { _ in
                await first.loadTranscriptSoon()
                try? await Task.sleep(for: .seconds(5))
            }),
            ("idle-huge", { end in try? await Task.sleep(for: .seconds(end - CACurrentMediaTime())) }),
            ("scroll-reading", { end in await self.scrolling(until: end, speed: 10) }),
            ("scroll-huge", { end in await self.scrolling(until: end) }),
            ("scroll-huge-fast", { end in await self.scrolling(until: end, speed: 200) }),
            ("switch-huge", { end in await self.switching(among: Array(huge.prefix(4)), until: end) }),
            ("stream-1", { end in
                self.startTurns(1, seconds: end - CACurrentMediaTime())
                try? await Task.sleep(for: .seconds(end - CACurrentMediaTime()))
            }),
            ("stream-1-scroll", { end in
                self.startTurns(1, seconds: end - CACurrentMediaTime())
                await self.scrolling(until: end)
            }),
            ("stream-4", { end in
                self.startTurns(4, seconds: end - CACurrentMediaTime())
                try? await Task.sleep(for: .seconds(end - CACurrentMediaTime()))
            }),
            ("stream-8", { end in
                self.startTurns(8, seconds: end - CACurrentMediaTime())
                try? await Task.sleep(for: .seconds(end - CACurrentMediaTime()))
            }),
            ("stream-all", { end in
                self.startTurns(huge.count, seconds: end - CACurrentMediaTime())
                try? await Task.sleep(for: .seconds(end - CACurrentMediaTime()))
            }),
            ("stream-all-scroll", { end in
                self.startTurns(huge.count, seconds: end - CACurrentMediaTime())
                await self.scrolling(until: end)
            }),
            ("stream-all-switch", { end in
                self.startTurns(huge.count, seconds: end - CACurrentMediaTime())
                await self.switching(among: huge, until: end)
            }),
            ("stream-all-clicks", { end in
                self.startTurns(huge.count, seconds: end - CACurrentMediaTime())
                await self.clicking(among: huge, until: end)
            }),
            ("stream-all-elsewhere", { end in
                // All of it in threads you aren't looking at.
                let other = self.app.threads.first { !$0.title.hasPrefix("Huge") }
                self.startTurns(huge.count, seconds: end - CACurrentMediaTime())
                self.app.selectedThreadID = other?.id
                try? await Task.sleep(for: .seconds(end - CACurrentMediaTime()))
                self.app.selectedThreadID = first.id
            }),
            ("settled", { end in
                try? await Task.sleep(for: .seconds(end - CACurrentMediaTime()))
            }),
        ]

        for (name, action) in phases where only?.contains(name) ?? true {
            if let wait = env["YH_PERF_WAIT"].flatMap(Double.init) {
                log("about to run \(name), waiting \(Int(wait)) s")
                try? await Task.sleep(for: .seconds(wait))
            }
            await record(name, seconds: length, action)
            // A turn runs on past its phase by its last reply, and ending folds its work away.
            if threads.contains(where: { $0.phase == .running }) {
                await record("\(name)-end", seconds: 0) { _ in await self.waitForTurnsToEnd() }
            }
            app.selectedThreadID = first.id
            try? await Task.sleep(for: .seconds(2))
        }

        let summary = try? JSONSerialization.data(withJSONObject: results, options: [.prettyPrinted, .sortedKeys])
        FileManager.default.createFile(atPath: "\(folder)/phases.json", contents: summary)
        FileManager.default.createFile(atPath: "\(folder)/seconds.csv", contents: Data(csv.utf8))
        var clickReport: [String: Any] = [:]
        for (label, times) in clicks.sorted(by: { $0.key < $1.key }) {
            let sorted = times.sorted()
            let median = sorted[sorted.count / 2], worst = sorted.last ?? 0
            clickReport[label] = ["count": sorted.count, "medianMs": median, "worstMs": worst]
            log(String(format: "click %@: %d times, median %.0f ms, worst %.0f ms", label, sorted.count, median, worst))
        }
        FileManager.default.createFile(atPath: "\(folder)/clicks.json", contents: try? JSONSerialization.data(withJSONObject: clickReport, options: [.prettyPrinted, .sortedKeys]))
        log("DONE \(results.count) phases")
        if env["YH_PERF_QUIT"] == "1" { NSApp.terminate(nil) }
    }

    private func record(_ name: String, seconds: Double, _ action: @escaping (CFTimeInterval) async -> Void) async {
        phaseName = name
        if env["YH_PERF_FRONT"] == "1" {
            NSApp.activate()
            mainWindow?.makeKeyAndOrderFront(nil)
        }
        log("start \(name)")
        frames = FrameStats()
        second = FrameStats()
        let wallStart = CACurrentMediaTime()
        let mainStart = Self.threadCPU(), processStart = Self.processCPU(), memoryStart = Self.footprintMB()
        var peak = memoryStart
        var lastWall = wallStart, lastMain = mainStart, lastProcess = processStart

        let sampler = Task { @MainActor in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                let now = CACurrentMediaTime(), main = Self.threadCPU(), process = Self.processCPU(), memory = Self.footprintMB()
                peak = max(peak, memory)
                let wall = max(0.001, now - lastWall)
                let row = String(format: "%.1f,%@,%.0f,%.1f,%.0f,%.0f,%.0f,%d\n", now - self.started, name, Double(self.second.vsyncs - self.second.missed) / wall,
                                 self.second.worst * 1000, (main - lastMain) / wall * 100, (process - lastProcess) / wall * 100, memory, Self.threadCount())
                self.csv += row
                self.second = FrameStats()
                lastWall = now; lastMain = main; lastProcess = process
            }
        }
        let end = wallStart + seconds
        await action(end)
        // A frame after it all, so a freeze at the end counts here.
        try? await Task.sleep(for: .milliseconds(100))
        sampler.cancel()

        let wall = max(0.001, CACurrentMediaTime() - wallStart)
        // After the numbers' window, since drawing the window to look costs frames: nothing
        // drawn in the conversation is the blank transcript, whatever layout says.
        let selected = app.selectedThread
        let rows = selected?.transcriptDisplayRows.count ?? 0
        let ink = SwitchProbe.conversationInk() ?? -1
        // That was the probe drawing the window, not a stall of the app's.
        lastTimestamp = 0
        if rows > 0, ink >= 0, ink < 0.002 {
            log(String(format: "BLANK at the end of %@: %d rows, %.4f inked", name, rows, ink))
        }
        let drawn = frames.vsyncs - frames.missed
        let result: [String: Any] = [
            "phase": name,
            "seconds": wall,
            "screenHz": 1 / period,
            "fps": Double(drawn) / wall,
            "framesMissedPct": frames.vsyncs == 0 ? 0 : Double(frames.missed) / Double(frames.vsyncs) * 100,
            "hitches": frames.hitches,
            "worstFrameMs": frames.worst * 1000,
            "stallsOver100ms": frames.stalls100,
            "stallsOver250ms": frames.stalls250,
            "stallsOver1s": frames.stalls1000,
            "mainThreadCpuPct": (Self.threadCPU() - mainStart) / wall * 100,
            "processCpuPct": (Self.processCPU() - processStart) / wall * 100,
            "footprintStartMB": memoryStart,
            "footprintEndMB": Self.footprintMB(),
            "footprintPeakMB": peak,
            "threads": Self.threadCount(),
            "transcriptRows": rows,
            "conversationInk": ink,
            "windowVisible": mainWindow?.occlusionState.contains(.visible) ?? false,
            "appActive": NSApp.isActive,
        ]
        results.append(result)
        log(String(format: "end %@: %.0f fps of %.0f, %.1f%% missed, worst %.0f ms, %d stalls >100ms, %d >250ms, %d >1s, main %.0f%%, process %.0f%%, %.0f MB (peak %.0f)",
                   name, Double(drawn) / wall, 1 / period, result["framesMissedPct"] as? Double ?? 0, frames.worst * 1000,
                   frames.stalls100, frames.stalls250, frames.stalls1000, result["mainThreadCpuPct"] as? Double ?? 0,
                   result["processCpuPct"] as? Double ?? 0, Self.footprintMB(), peak))
        // Saved as it goes: a crash in a later phase shouldn't lose the earlier ones.
        let partial = try? JSONSerialization.data(withJSONObject: results, options: [.prettyPrinted, .sortedKeys])
        FileManager.default.createFile(atPath: "\(folder)/phases.json", contents: partial)
        FileManager.default.createFile(atPath: "\(folder)/seconds.csv", contents: Data(csv.utf8))
    }
}
