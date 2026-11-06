import AppKit
import Darwin
import QuartzCore
import SwiftUI
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
    private var csv = "elapsed,phase,fps,worst_ms,main_cpu_pct,process_cpu_pct,footprint_mb,threads,window_visible,app_active\n"
    private let started = CACurrentMediaTime()
    private var frameWaiters: [CheckedContinuation<CFTimeInterval, Never>] = []
    /// How long each kind of click took to show on screen, in milliseconds.
    private var clicks: [String: [Double]] = [:]
    private var clickLog = "phase,seconds_in,click,ms,main_cpu_ms,answered_ms,rows_made,rows_ms\n"
    private var phaseStart: CFTimeInterval = 0

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
        // `YH_PERF_FREEZE_AFTER=<seconds>`: sooner, to land inside stalls too short for the default.
        let freezeAfter = env["YH_PERF_FREEZE_AFTER"].flatMap(Double.init) ?? 3
        Thread.detachNewThread {
            var taken = 0
            while taken < (freezeAfter < 3 ? 25 : 5) {
                Thread.sleep(forTimeInterval: freezeAfter < 3 ? 0.1 : 0.5)
                guard CACurrentMediaTime() - Self.lastFrame.withLock({ $0 }) > freezeAfter else { continue }
                taken += 1
                let sample = Process()
                sample.executableURL = URL(fileURLWithPath: "/usr/bin/sample")
                sample.arguments = ["\(pid)", freezeAfter < 3 ? "1" : "3", "-file", "\(folder)/freeze-\(taken).sample"]
                try? sample.run()
                sample.waitUntilExit()
                FileHandle.standardError.write(Data("PERF freeze sampled: freeze-\(taken).sample\n".utf8))
                Thread.sleep(forTimeInterval: freezeAfter < 3 ? 0.5 : 5)
            }
        }
    }

    /// The longest the main thread took to answer since the last frame, and all phase, whatever
    /// the frames did: frames can stop with the main thread free, which is another problem.
    private static let pingWorst = OSAllocatedUnfairLock(initialState: CFTimeInterval(0))
    private static let pingPhase = OSAllocatedUnfairLock(initialState: [CFTimeInterval]())

    private func pingMainThread() {
        Thread.detachNewThread {
            while true {
                let sent = CACurrentMediaTime()
                DispatchQueue.main.async {
                    let latency = CACurrentMediaTime() - sent
                    Self.pingWorst.withLock { $0 = max($0, latency) }
                    Self.pingPhase.withLock { $0.append(latency) }
                }
                Thread.sleep(forTimeInterval: 0.05)
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

    private static func secondsSinceLaunch() -> Double {
        var process = kinfo_proc(), size = MemoryLayout<kinfo_proc>.stride
        var name = [CTL_KERN, KERN_PROC, KERN_PROC_PID, getpid()]
        guard sysctl(&name, 4, &process, &size, nil, 0) == 0 else { return 0 }
        let started = process.kp_proc.p_starttime
        return Date().timeIntervalSince1970 - (Double(started.tv_sec) + Double(started.tv_usec) / 1_000_000)
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
            if delta > 0.25 {
                log(String(format: "stall %.0f ms in or after %@ (main thread answered within %.0f ms)", delta * 1000, phaseName,
                           Self.pingWorst.withLock { $0 } * 1000))
            }
            Self.pingWorst.withLock { $0 = 0 }
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

    /// `YH_ROW_CHECK=1`: every half a second, that rows are the height they say, which is what
    /// keeps them from drawing over each other. Being a moment behind is fine, a while isn't.
    private func checkRowHeights() {
        guard env["YH_ROW_CHECK"] == "1" else { return }
        Task { @MainActor in
            var lastBad: Set<String> = []
            var reported = 0
            while true {
                try? await Task.sleep(for: .milliseconds(500))
                let bad = app.transcriptDeck.currentList?.rowsOffTheirHeight() ?? []
                let still = bad.filter { lastBad.contains($0) }
                if !still.isEmpty, reported < 40 {
                    reported += 1
                    log("FAIL: rows off their height for half a second: \(still.prefix(6))")
                }
                lastBad = Set(bad)
            }
        }
    }

    /// Frames stop by themselves when the window can't be seen, with the app idle: says so, so
    /// that a hand on the Mac or another Space isn't taken for a freeze.
    private func watchWhatsOutsideTheApp() {
        let center = NotificationCenter.default, workspace = NSWorkspace.shared.notificationCenter
        center.addObserver(forName: NSWindow.didChangeOcclusionStateNotification, object: mainWindow, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.log("OUTSIDE window visible: \(self?.mainWindow?.occlusionState.contains(.visible) == true)") }
        }
        center.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.log("OUTSIDE app no longer active") }
        }
        workspace.addObserver(forName: NSWorkspace.activeSpaceDidChangeNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.log("OUTSIDE the Space changed") }
        }
        workspace.addObserver(forName: NSWorkspace.screensDidSleepNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.log("OUTSIDE the screen slept") }
        }
        workspace.addObserver(forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main) { [weak self] note in
            let name = (note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication)?.localizedName ?? "?"
            MainActor.assumeIsolated { self?.log("OUTSIDE \(name) came to the front") }
        }
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
            // On whichever Space you're on, full screen or not: on another it can't be seen.
            window.collectionBehavior.formUnion([.canJoinAllSpaces, .fullScreenAuxiliary])
            NSApp.activate()
            window.makeKeyAndOrderFront(nil)
            window.orderFrontRegardless()
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

    /// Straight to the scroll view of the transcript: the window is below the screen, where an
    /// event's location wouldn't find it.
    private func wheel(_ points: Int32) {
        guard let content = mainWindow?.contentView else { return }
        guard let cg = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: points, wheel2: 0, wheel3: 0),
              let event = NSEvent(cgEvent: cg) else { return }
        var view = content.hitTest(NSPoint(x: content.bounds.midX + 120, y: content.bounds.midY))
        // Past a code block's own scroll view, which is for going sideways.
        while let current = view, !(current is TranscriptScrollView) { view = current.superview }
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

    /// `YH_PERF_SHOTS=1`: the first time each thing is on screen, a drawing of the window, for
    /// checking how it looks. Costs frames: not for runs that measure.
    private var shots: Set<String> = []
    private var switches = 0
    private func shot(_ name: String) {
        guard env["YH_PERF_SHOTS"] == "1", shots.insert(name).inserted else { return }
        WindowSnapshot.write(to: "\(folder)/\(name).png")
    }

    @discardableResult private func nextFrame() async -> CFTimeInterval {
        await withCheckedContinuation { frameWaiters.append($0) }
    }

    /// Does what a click does, and times it until it has been on screen for a frame: the main
    /// thread free again after it, which is what a click feels like.
    private func click(_ label: String, _ action: () -> Void) async {
        let start = CACurrentMediaTime(), cpu = Self.threadCPU()
        let rowsBefore = TranscriptListView.rowsMade, mountBefore = TranscriptListView.secondsMaking
        action()
        // The first frame answers the click; rows being made can follow it.
        let answered = await nextFrame()
        let shown = await nextFrame()
        let ms = (shown - start) * 1000
        clicks[label, default: []].append(ms)
        clicks[label + " (answered)", default: []].append((answered - start) * 1000)
        let used = (Self.threadCPU() - cpu) * 1000
        clickLog += String(format: "%@,%.1f,%@,%.0f,%.0f,%.0f,%d,%.0f\n", phaseName, start - phaseStart, label, ms, used, (answered - start) * 1000,
                           TranscriptListView.rowsMade - rowsBefore, (TranscriptListView.secondsMaking - mountBefore) * 1000)
        if ms > 100 { log(String(format: "slow click %@: %.0f ms, %.0f ms of it main thread, %.1f s into %@", label, ms, used, start - phaseStart, phaseName)) }
    }

    /// What it costs for a while after: a click that shows at once and then stalls is slow too.
    /// Only for clicks that say so: the frames it waits for count as the pause between clicks.
    private func aftermath(_ label: String, for seconds: Double = 0.7) async {
        let end = CACurrentMediaTime() + seconds
        var last = await nextFrame(), worst: CFTimeInterval = 0
        while last < end {
            let next = await nextFrame()
            worst = max(worst, next - last)
            last = next
        }
        clicks[label + " (worst frame after)", default: []].append(worst * 1000)
    }

    /// A key typed into whatever has the focus, as the keyboard sends it.
    private func typed(_ character: Character) -> Bool {
        guard let window = mainWindow else { return false }
        for type in [NSEvent.EventType.keyDown, .keyUp] {
            guard let event = NSEvent.keyEvent(with: type, location: .zero, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                               windowNumber: window.windowNumber, context: nil, characters: String(character),
                                               charactersIgnoringModifiers: String(character), isARepeat: false, keyCode: 0) else { return false }
            window.sendEvent(event)
        }
        return true
    }

    private func switchTo(_ thread: ThreadModel) async {
        let wasLoaded = !thread.entries.isEmpty
        await click(wasLoaded ? "switch-loaded" : "switch-unloaded") { app.selectedThreadID = thread.id }
        if switches < 3 {
            switches += 1
            try? await Task.sleep(for: .milliseconds(300))
            shot("switched-\(switches)")
        }
        guard !wasLoaded else { return }
        try? await Task.sleep(for: .milliseconds(150))
        shot("loading")
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
                shot("expanded")
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

    /// Everything else you do in a day, in turn, for what `clicking` doesn't reach: typing, tabs,
    /// search, jumping, resizing, the sidebar. `YH_PERF_ONLY=<a,b>` runs only those labels' groups.
    private func everything(among list: [ThreadModel], until end: CFTimeInterval) async {
        let pause: Duration = .milliseconds(Int(env["YH_PERF_PAUSE"] ?? "") ?? 500)
        let only = env["YH_PERF_ONLY"].map { Set($0.split(separator: ",").map(String.init)) }
        func wants(_ group: String) -> Bool { only?.contains(group) ?? true }
        var index = 0
        let ordinary = app.threads.first { !$0.title.hasPrefix("Huge") }
        while CACurrentMediaTime() < end, !list.isEmpty {
            index = (index + 1) % list.count
            let thread = list[index]
            await switchTo(thread)
            try? await Task.sleep(for: pause)

            if wants("type") {
                // Whatever the keyboard reaches, it must be the composer.
                var typedText = ""
                for character in "fix the parser" {
                    typedText.append(character)
                    await click("type-key") { _ = typed(character) }
                    try? await Task.sleep(for: .milliseconds(40))
                }
                if thread.draft != typedText { log("FAIL: typed \"\(typedText)\", the composer has \"\(thread.draft.prefix(40))\"") }
                await click("clear-draft") { thread.draft = "" }
                try? await Task.sleep(for: pause)
            }
            if wants("tabs") {
                for panel in AppModel.InspectorPanel.allCases where panel != .browser {
                    await click("tab-\(panel.rawValue)") { app.showInspector(panel) }
                    await aftermath("tab-\(panel.rawValue)")
                }
                // And switching with it open, as you'd have it.
                await switchTo(list[(index + 1) % list.count])
                try? await Task.sleep(for: pause)
                await switchTo(thread)
                try? await Task.sleep(for: pause)
                await click("inspector-close") { app.setShowsInspector(false) }
                try? await Task.sleep(for: pause)
            }
            if wants("spotlight") {
                await click("spotlight-open") { app.spotlight.open(app) }
                try? await Task.sleep(for: pause)
                await click("spotlight-type") { app.spotlight.query = "timezone" }
                await aftermath("spotlight-type", for: 1.2)
                await click("spotlight-close") { app.spotlight.close() }
                try? await Task.sleep(for: pause)
            }
            if wants("find") {
                await click("find-open") { thread.find.open() }
                try? await Task.sleep(for: pause)
                await click("find-type") { thread.find.query = "migration" }
                await aftermath("find-type", for: 1.2)
                await click("find-close") { thread.find.close() }
                try? await Task.sleep(for: pause)
            }
            if wants("jump") {
                await click("jump-previous") { thread.jumpToMessage(.previous) }
                await aftermath("jump-previous")
                await click("jump-top") { thread.jumpToMessage(.top) }
                await aftermath("jump-top", for: 1.5)
                await click("jump-bottom") { thread.jumpToMessage(.bottom) }
                await aftermath("jump-bottom", for: 1.5)
            }
            if wants("bigtool"), case .work(let work)? = thread.transcriptDisplayRows.last(where: { if case .work = $0 { true } else { false } }) {
                // The biggest output there is, opened.
                await click("expand-work") { thread.setWork(work, expanded: true) }
                await aftermath("expand-work")
                if let big = work.entries.filter({ $0.kind == .tool }).max(by: { ($0.toolResult?.utf8.count ?? 0) < ($1.toolResult?.utf8.count ?? 0) }) {
                    app.transcriptDeck.currentList?.show(rowID: big.id)
                    try? await Task.sleep(for: pause)
                    await click("open-big-tool") { big.isOpen = true }
                    await aftermath("open-big-tool")
                    await click("close-big-tool") { big.isOpen = false }
                    await aftermath("close-big-tool")
                }
                await click("collapse-work") { thread.setWork(work, expanded: false) }
                await aftermath("collapse-work")
                await click("jump-bottom") { thread.jumpToMessage(.bottom) }
                try? await Task.sleep(for: pause)
            }
            if wants("sidebar"), let ordinary, let project = app.project(id: ordinary.projectID) {
                await click("pin") { withAnimation(.snappy) { app.togglePinned(ordinary) } }
                await aftermath("pin")
                await click("unpin") { withAnimation(.snappy) { app.togglePinned(ordinary) } }
                await aftermath("unpin")
                await click("settle") { withAnimation(.snappy) { app.toggleSettled(ordinary) } }
                await aftermath("settle")
                await click("unsettle") { withAnimation(.snappy) { app.toggleSettled(ordinary) } }
                await aftermath("unsettle")
                await click("collapse") { withAnimation(.snappy(duration: 0.25)) { app.setCollapsed(true, for: project) } }
                await aftermath("collapse")
                await click("uncollapse") { withAnimation(.snappy(duration: 0.25)) { app.setCollapsed(false, for: project) } }
                await aftermath("uncollapse")
                app.selectedThreadID = thread.id
                try? await Task.sleep(for: pause)
            }
            if wants("pages"), let project = app.project(id: thread.projectID) {
                await click("memory-page") { app.showMemory(of: project) }
                await aftermath("memory-page")
                await click("back-to-thread") { app.selectedThreadID = thread.id }
                await aftermath("back-to-thread")
                await click("usage-open") { app.tokenUsage.open() }
                await aftermath("usage-open", for: 1.2)
                await click("usage-close") { app.tokenUsage.close() }
                await aftermath("usage-close")
            }
            if wants("resize"), let window = mainWindow {
                let frame = window.frame
                // A live resize: every frame a little narrower, then back.
                for step in Array(1...15) + Array((0...14).reversed()) {
                    await click("resize-step") {
                        window.setFrame(NSRect(x: frame.minX, y: frame.minY, width: frame.width - CGFloat(step) * 12, height: frame.height), display: true)
                    }
                }
                try? await Task.sleep(for: pause)
                if let split = Self.splitView(in: window.contentView), split.arrangedSubviews.count >= 2 {
                    let left = split.arrangedSubviews[0].frame.width
                    for step in Array(1...10) + Array((0...9).reversed()) {
                        await click("sidebar-drag-step") { split.setPosition(left + CGFloat(step) * 8, ofDividerAt: 0) }
                    }
                }
                try? await Task.sleep(for: pause)
            }
        }
    }

    /// Anything, in any order, as fast as a hand could: what a list of clicks doesn't think of.
    /// Each action is logged before it's done, so a crash says what led to it, and
    /// `YH_PERF_CHAOS_SEED=<n>` plays a run again.
    private func chaos(among list: [ThreadModel], until end: CFTimeInterval) async {
        let seed = env["YH_PERF_CHAOS_SEED"].flatMap(UInt64.init) ?? UInt64(Date().timeIntervalSince1970)
        var random = ChaosRandom(state: seed | 1)
        log("chaos seed \(seed)")
        let ordinary = app.threads.filter { !$0.title.hasPrefix("Huge") }
        let actions: [(String, Int, (ThreadModel) async -> Void)] = [
            ("switch", 12, { [self] _ in
                var pick = ChaosRandom(state: random.next())
                await switchTo(list[Int(pick.next() % UInt64(list.count))])
            }),
            ("switch-ordinary", 2, { [self] _ in
                if let other = ordinary.randomElement() { await switchTo(other) }
            }),
            ("jump-top", 3, { [self] thread in await click("jump-top") { thread.jumpToMessage(.top) } }),
            ("jump-bottom", 3, { [self] thread in await click("jump-bottom") { thread.jumpToMessage(.bottom) } }),
            ("jump-previous", 4, { [self] thread in await click("jump-previous") { thread.jumpToMessage(.previous) } }),
            ("jump-next", 3, { [self] thread in await click("jump-next") { thread.jumpToMessage(.next) } }),
            ("wheel-up", 6, { [self] _ in
                for _ in 0..<30 { wheel(60); try? await Task.sleep(for: .milliseconds(8)) }
            }),
            ("wheel-down", 4, { [self] _ in
                for _ in 0..<30 { wheel(-60); try? await Task.sleep(for: .milliseconds(8)) }
            }),
            ("wheel-fling", 2, { [self] _ in
                for _ in 0..<40 { wheel(400); try? await Task.sleep(for: .milliseconds(8)) }
            }),
            ("expand-work", 4, { [self] thread in
                if case .work(let work)? = thread.transcriptDisplayRows.last(where: { if case .work = $0 { true } else { false } }) {
                    await click(work.isExpanded ? "collapse-work" : "expand-work") { thread.setWork(work, expanded: !work.isExpanded) }
                }
            }),
            ("earlier-turns", 1, { [self] thread in await click("earlier-turns") { thread.showEarlierTurns() } }),
            ("inspector", 3, { [self] _ in let shows = app.showsInspector; await click("inspector") { app.setShowsInspector(!shows) } }),
            ("tab", 3, { [self] _ in
                let panels = AppModel.InspectorPanel.allCases.filter { $0 != .browser }
                if let panel = panels.randomElement() { await click("tab-\(panel.rawValue)") { app.showInspector(panel) } }
            }),
            ("spotlight", 2, { [self] _ in
                await click("spotlight-open") { app.spotlight.open(app) }
                try? await Task.sleep(for: .milliseconds(120))
                await click("spotlight-type") { app.spotlight.query = "timezone" }
                try? await Task.sleep(for: .milliseconds(200))
                await click("spotlight-close") { app.spotlight.close() }
            }),
            ("find", 2, { [self] thread in
                await click("find-open") { thread.find.open() }
                await click("find-type") { thread.find.query = "migration" }
                try? await Task.sleep(for: .milliseconds(250))
                await click("find-next") { thread.find.step(1) }
                try? await Task.sleep(for: .milliseconds(150))
                await click("find-close") { thread.find.close() }
            }),
            ("type", 3, { [self] thread in
                for character in "go on" { await click("type-key") { _ = typed(character) }; try? await Task.sleep(for: .milliseconds(30)) }
                thread.draft = ""
            }),
            ("new-thread", 2, { [self] _ in await click("new-thread") { app.newThreadInCurrentProject() } }),
            ("project-page", 2, { [self] thread in
                if let project = app.project(id: thread.projectID) { await click("project-page") { app.showMemory(of: project) } }
                try? await Task.sleep(for: .milliseconds(150))
                await click("back-to-thread") { app.selectedThreadID = thread.id }
            }),
            ("usage", 1, { [self] _ in
                await click("usage-open") { app.tokenUsage.open() }
                try? await Task.sleep(for: .milliseconds(200))
                await click("usage-close") { app.tokenUsage.close() }
            }),
            ("sidebar-order", 2, { [self] _ in
                guard let other = ordinary.randomElement(), let project = app.project(id: other.projectID) else { return }
                await click("pin") { withAnimation(.snappy) { app.togglePinned(other) } }
                await click("settle") { withAnimation(.snappy) { app.toggleSettled(other) } }
                await click("collapse") { withAnimation(.snappy(duration: 0.25)) { app.setCollapsed(true, for: project) } }
                try? await Task.sleep(for: .milliseconds(100))
                await click("uncollapse") { withAnimation(.snappy(duration: 0.25)) { app.setCollapsed(false, for: project) } }
                await click("unsettle") { withAnimation(.snappy) { app.toggleSettled(other) } }
                await click("unpin") { withAnimation(.snappy) { if other.isPinned { app.togglePinned(other) } } }
            }),
            ("resize", 2, { [self] _ in
                guard let window = mainWindow else { return }
                let frame = window.frame
                for step in [1, 3, 5, 7, 5, 3, 1, 0] {
                    await click("resize-step") { window.setFrame(NSRect(x: frame.minX, y: frame.minY, width: frame.width - CGFloat(step) * 30, height: frame.height), display: true) }
                }
            }),
            ("stop", 1, { [self] thread in await click("stop") { thread.stop() } }),
            ("send", 2, { [self] thread in await click("send") { thread.send("PERF seconds=8 rate=1") } }),
        ]
        let total = actions.reduce(0) { $0 + $1.1 }
        var count = 0
        while CACurrentMediaTime() < end {
            var roll = Int(random.next() % UInt64(total))
            guard let action = actions.first(where: { roll -= $0.1; return roll < 0 }) else { continue }
            guard let thread = app.selectedThread ?? list.first else { continue }
            count += 1
            log("chaos \(count) \(action.0) in \(thread.title)")
            if app.visibleThreadID == nil { app.selectedThreadID = thread.id }
            await action.2(thread)
            try? await Task.sleep(for: .milliseconds(Int(random.next() % 400)))
        }
        log("chaos done, \(count) actions")
    }

    private struct ChaosRandom: RandomNumberGenerator {
        var state: UInt64
        mutating func next() -> UInt64 {
            state ^= state << 13
            state ^= state >> 7
            state ^= state << 17
            return state
        }
    }

    private static func splitView(in view: NSView?) -> NSSplitView? {
        guard let view else { return nil }
        if let split = view as? NSSplitView { return split }
        for child in view.subviews { if let found = splitView(in: child) { return found } }
        return nil
    }

    /// Starts turns and waits until they're all really streaming: a send waits for its history to
    /// load, tens of seconds with twenty huge threads, and a phase timed from here would mostly
    /// measure that. Returns when the phase should end.
    private func startTurns(_ count: Int, length: Double) async -> CFTimeInterval {
        let started = threads.prefix(count).filter { $0.phase != .running }
        let marks = Dictionary(uniqueKeysWithValues: started.map { ($0.id, $0.entries.count) })
        let begin = CACurrentMediaTime()
        for thread in started {
            thread.send("PERF seconds=\(Int(length)) rate=\(env["YH_PERF_RATE"] ?? "1")\(env["YH_PERF_NO_SUBAGENTS"] == "1" ? " nosubagents" : "")")
        }
        // Loaded, your message and something from Claude after it.
        func streaming(_ thread: ThreadModel) -> Bool {
            guard let last = thread.entries.last, thread.entries.count > (marks[thread.id] ?? 0) else { return false }
            return thread.phase == .running && last.kind != .user
        }
        while !started.allSatisfy(streaming), CACurrentMediaTime() - begin < 120 {
            try? await Task.sleep(for: .milliseconds(200))
        }
        log(String(format: "%d turns streaming after %.1f s", started.count, CACurrentMediaTime() - begin))
        return CACurrentMediaTime() + length
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
        log(String(format: "launch: window after %.2f s", Self.secondsSinceLaunch()))
        guard startFrames() else { return log("FAIL: no window") }
        watchWhatsOutsideTheApp()
        checkRowHeights()
        watchForFreezes()
        pingMainThread()
        keepPointerOverTranscript()
        let length = Double(env["YH_PERF_SECONDS"] ?? "") ?? 30
        let only = env["YH_PERF_PHASES"].map { Set($0.split(separator: ",").map(String.init)) }
        let huge = threads
        guard let first = huge.first else { return log("FAIL: no threads named Huge") }
        log("pid \(ProcessInfo.processInfo.processIdentifier), \(huge.count) huge threads, screen \(mainWindow?.screen?.maximumFramesPerSecond ?? 0) Hz, low power \(ProcessInfo.processInfo.isLowPowerModeEnabled)")
        app.selectedThreadID = first.id
        await first.loadTranscriptSoon()
        await nextFrame()
        log(String(format: "launch: first huge transcript shown after %.2f s (%d rows)", Self.secondsSinceLaunch(), first.entries.count))

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
            ("switch-all-idle", { end in
                // Every thread loaded, nothing running: what a switch itself costs.
                for thread in huge { await thread.loadTranscriptSoon() }
                await self.switching(among: huge, until: CACurrentMediaTime() + length)
            }),
            ("stream-1", { _ in
                let end = await self.startTurns(1, length: length)
                try? await Task.sleep(for: .seconds(end - CACurrentMediaTime()))
            }),
            ("stream-1-scroll", { _ in
                let end = await self.startTurns(1, length: length)
                await self.scrolling(until: end)
            }),
            ("stream-4", { _ in
                let end = await self.startTurns(4, length: length)
                try? await Task.sleep(for: .seconds(end - CACurrentMediaTime()))
            }),
            ("stream-8", { _ in
                let end = await self.startTurns(8, length: length)
                try? await Task.sleep(for: .seconds(end - CACurrentMediaTime()))
            }),
            ("stream-all", { _ in
                let end = await self.startTurns(huge.count, length: length)
                try? await Task.sleep(for: .seconds(end - CACurrentMediaTime()))
            }),
            ("stream-all-scroll", { _ in
                let end = await self.startTurns(huge.count, length: length)
                await self.scrolling(until: end)
            }),
            ("stream-all-switch", { _ in
                let end = await self.startTurns(huge.count, length: length)
                await self.switching(among: huge, until: end)
            }),
            ("stream-all-clicks", { _ in
                let end = await self.startTurns(huge.count, length: length)
                await self.clicking(among: huge, until: end)
            }),
            ("idle-everything", { end in
                await self.everything(among: huge, until: end)
            }),
            ("stream-all-everything", { _ in
                let end = await self.startTurns(huge.count, length: length)
                await self.everything(among: huge, until: end)
            }),
            ("idle-chaos", { end in
                await self.chaos(among: huge, until: end)
            }),
            ("stream-all-chaos", { _ in
                let end = await self.startTurns(huge.count, length: length)
                await self.chaos(among: huge, until: end)
            }),
            ("stream-all-elsewhere", { _ in
                // All of it in threads you aren't looking at.
                let other = self.app.threads.first { !$0.title.hasPrefix("Huge") }
                self.app.selectedThreadID = other?.id
                let end = await self.startTurns(huge.count, length: length)
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
        FileManager.default.createFile(atPath: "\(folder)/clicks.csv", contents: Data(clickLog.utf8))
        FileManager.default.createFile(atPath: "\(folder)/clicks.json", contents: try? JSONSerialization.data(withJSONObject: clickReport, options: [.prettyPrinted, .sortedKeys]))
        log("DONE \(results.count) phases")
        if env["YH_PERF_QUIT"] == "1" { NSApp.terminate(nil) }
    }

    private func record(_ name: String, seconds: Double, _ action: @escaping (CFTimeInterval) async -> Void) async {
        phaseName = name
        phaseStart = CACurrentMediaTime()
        Self.pingPhase.withLock { $0 = [] }
        if env["YH_PERF_FRONT"] == "1" {
            NSApp.activate()
            mainWindow?.makeKeyAndOrderFront(nil)
            mainWindow?.orderFrontRegardless()
        }
        log("start \(name)")
        frames = FrameStats()
        second = FrameStats()
        let wallStart = CACurrentMediaTime()
        let mainStart = Self.threadCPU(), processStart = Self.processCPU(), memoryStart = Self.footprintMB()
        var peak = memoryStart
        var lastWall = wallStart, lastMain = mainStart, lastProcess = processStart
        var hiddenSeconds = 0

        let sampler = Task { @MainActor in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                let now = CACurrentMediaTime(), main = Self.threadCPU(), process = Self.processCPU(), memory = Self.footprintMB()
                peak = max(peak, memory)
                let wall = max(0.001, now - lastWall)
                let visible = self.mainWindow?.occlusionState.contains(.visible) ?? false
                if !visible { hiddenSeconds += 1 }
                let row = String(format: "%.1f,%@,%.0f,%.1f,%.0f,%.0f,%.0f,%d,%d,%d\n", now - self.started, name, Double(self.second.vsyncs - self.second.missed) / wall,
                                 self.second.worst * 1000, (main - lastMain) / wall * 100, (process - lastProcess) / wall * 100, memory, Self.threadCount(),
                                 visible ? 1 : 0, NSApp.isActive ? 1 : 0)
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
        let pings = Self.pingPhase.withLock { $0 }.sorted()
        let pingMedian = pings.isEmpty ? 0 : pings[pings.count / 2], pingWorst = pings.last ?? 0
        log(String(format: "main thread answers %@: median %.0f ms, worst %.0f ms, %d over 100 ms, %d over 250 ms, %d over 1 s, of %d",
                   name, pingMedian * 1000, pingWorst * 1000, pings.count { $0 > 0.1 }, pings.count { $0 > 0.25 }, pings.count { $0 > 1 }, pings.count))
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
        // A window that can't be seen (the display off, another Space) gets few frames, with the
        // main thread idle and clicks seeming to take a second: it says nothing about the app.
        if hiddenSeconds > 0 { log("INVALID \(name): the window couldn't be seen for \(hiddenSeconds) s of \(Int(wall)) s, so its numbers don't count") }
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
