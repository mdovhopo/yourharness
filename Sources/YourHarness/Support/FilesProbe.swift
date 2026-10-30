import AppKit
import HarnessCore

/// Development, for the selected thread's Files tab:
/// - `YH_OPEN_FILE=<path[:line[-end]]>` (relative to the thread's folder, or absolute inside it)
///   opens a file at launch, as clicking a link to it does.
/// - `YH_FILES_PROBE=<n>` opens n of the thread's files one after another as links do (a line
///   in each), then its biggest text file, scrolls through that, and closes them all. It prints
///   how long each took to show, the main thread's CPU and its longest stall (a watchdog on
///   another thread), and the memory footprint before and after. PASS is every stall under
///   50 ms. `YH_FILES_PROBE_QUIT=1` quits after; `YH_FILES_PROBE_WAIT=<seconds>` waits before
///   starting, for attaching Time Profiler; `YH_FILES_PROBE_INPUT=1` also sends real wheel, mouse
///   and typing events (the file, the tree, the divider, resizing, Go to File, a minified file)
///   and prints each one's main thread CPU a step against a frame.
@MainActor
enum FilesProbe {
    static let isOn = ProcessInfo.processInfo.environment["YH_FILES_PROBE"] != nil
    private static var waiting: (path: String, continuation: CheckedContinuation<Bool, Never>)?

    /// From the viewer, each time a file's content is on screen.
    static func shown(_ file: LoadedFile) {
        guard isOn, let waiting, waiting.path == file.path else { return }
        self.waiting = nil
        waiting.continuation.resume(returning: true)
    }

    static func runIfRequested(app: AppModel) {
        let environment = ProcessInfo.processInfo.environment
        if let request = environment["YH_OPEN_FILE"], !request.isEmpty {
            Task {
                try? await Task.sleep(for: .seconds(1))
                guard let thread = app.selectedThread else { return log("YH_OPEN_FILE: no thread selected") }
                let (path, line, end) = parse(request)
                thread.openFile(atPath: path, line: line, endLine: end, keep: true)
            }
        }
        guard isOn else { return }
        let count = environment["YH_FILES_PROBE"].flatMap(Int.init) ?? 20
        Task { await run(app: app, count: count) }
    }

    private static func parse(_ request: String) -> (String, Int?, Int?) {
        let parts = request.split(separator: ":", omittingEmptySubsequences: false)
        guard parts.count >= 2, let last = parts.last else { return (request, nil, nil) }
        let lines = last.split(separator: "-").compactMap { Int($0) }
        guard !lines.isEmpty else { return (request, nil, nil) }
        return (parts.dropLast().joined(separator: ":"), lines.first, lines.count > 1 ? lines[1] : nil)
    }

    private static func log(_ line: String) {
        FileHandle.standardError.write(Data("FILES PROBE \(line)\n".utf8))
    }

    private static func run(app: AppModel, count: Int) async {
        // `YH_FILES_PROBE_WAIT=<seconds>` (default 2): time to attach Time Profiler.
        try? await Task.sleep(for: .seconds(ProcessInfo.processInfo.environment["YH_FILES_PROBE_WAIT"].flatMap(Double.init) ?? 2))
        guard let thread = app.selectedThread, let root = thread.fileRoot else { return log("FAIL: no thread in a project folder selected") }
        guard let index = await app.fileIndex(root: root) else { return log("FAIL: couldn't list \(root.path)") }
        // Sizes off the main thread: the probe shouldn't stall what it measures.
        let candidates = await Task.detached { () -> [(path: String, size: Int)] in
            index.paths.filter { !$0.hasSuffix("/") }.compactMap { path in
                let size = (try? root.appending(path: path).resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0
                return size > 0 ? (path, size) : nil
            }
        }.value
        guard !candidates.isEmpty else { return log("FAIL: no files in \(root.path)") }
        let step = max(1, candidates.count / max(count, 1))
        var picked = stride(from: 0, to: candidates.count, by: step).prefix(count).map { candidates[$0].path }
        let text = Set(["swift", "ts", "js", "py", "md", "json", "txt", "c", "h", "go", "rs", "sql", "yaml", "yml", "html", "css"])
        let biggest = candidates.filter { text.contains(($0.path as NSString).pathExtension.lowercased()) }.max { $0.size < $1.size }
        if let biggest { picked.append(biggest.path) }

        let before = MemoryFootprint.current()
        log("ready: pid \(ProcessInfo.processInfo.processIdentifier), \(candidates.count) files, opening \(picked.count), biggest \(biggest.map { "\($0.path) (\($0.size.formatted(.byteCount(style: .file))))" } ?? "none"), footprint \(before.formatted(.byteCount(style: .memory)))")
        var watchdog = Watchdog()
        var stalls: [String] = []
        var stall = 0.0
        func phase(_ name: String) async {
            let longest = await watchdog.stop()
            stall = max(stall, longest)
            stalls.append(String(format: "%@ %.0f ms", name, longest))
            watchdog = Watchdog()
        }
        let cpuStart = Watchdog.threadCPU()
        // For comparison: the first show of another of the sidebar's tabs.
        if ProcessInfo.processInfo.environment["YH_FILES_PROBE_BASELINE"] == "1" {
            app.showInspector(.subagents)
            try? await Task.sleep(for: .milliseconds(300))
            await phase("baseline: Sub-agents tab")
            app.showInspector(.sideChat)
            try? await Task.sleep(for: .milliseconds(300))
            await phase("baseline: Side Chat tab")
            app.showInspector(.files)
            try? await Task.sleep(for: .milliseconds(500))
            await phase("Files tab, nothing open")
        }
        var times: [Double] = []
        var failed = 0
        for path in picked {
            let started = ContinuousClock.now
            let shown = await withCheckedContinuation { continuation in
                waiting = (path, continuation)
                thread.openFile(atPath: root.appending(path: path).path, line: 30, endLine: 34)
                Task {
                    try? await Task.sleep(for: .seconds(5))
                    if let waiting, waiting.path == path {
                        self.waiting = nil
                        waiting.continuation.resume(returning: false)
                    }
                }
            }
            let elapsed = (ContinuousClock.now - started) / .milliseconds(1)
            if !shown { failed += 1 }
            times.append(elapsed)
            log(String(format: "%@ %@ in %.0f ms", shown ? "shown" : "NOT SHOWN", path, elapsed))
            try? await Task.sleep(for: .milliseconds(50))
            await phase("open " + (path as NSString).lastPathComponent)
        }

        // What's on screen, for telling a blank viewer or tree.
        if let view = fileScrollView() {
            log("viewer \(view.frame.integral), text \(view.textView.frame.integral), \(view.textView.textStorage?.length ?? -1) characters, window \(view.window != nil)")
        } else {
            log("viewer: none")
        }
        if let tree: NSOutlineView = findView() {
            log("tree \(tree.enclosingScrollView?.frame.integral ?? .zero), outline \(tree.frame.integral), \(tree.numberOfRows) rows, column \(tree.tableColumns.first?.width ?? -1)")
        } else {
            log("tree: none")
        }
        // Through the biggest file, a screen at a time, then a jump to its end and back.
        var scrolls = 0
        if let view = fileScrollView() {
            let height = view.textView.frame.height
            let page = view.scrollView.contentView.bounds.height
            for index in 0..<60 {
                let y = index == 30 ? max(0, height - page) : Double(index % 30) * page
                view.scroll(to: NSPoint(x: 0, y: y))
                scrolls += 1
                try? await Task.sleep(for: .milliseconds(16))
            }
        }
        await phase("scroll")
        if ProcessInfo.processInfo.environment["YH_FILES_PROBE_INPUT"] == "1", let view = fileScrollView(), let window = view.window {
            // Real events handed to the window, as a mouse and keyboard send them: nothing on
            // screen moves. Each step's main thread CPU against a frame, and the stall.
            func act(_ name: String, steps: Int, _ step: (Int) -> Void) async {
                var worst = 0.0
                let start = Watchdog.threadCPU()
                for index in 0..<steps {
                    let before = Watchdog.threadCPU()
                    step(index)
                    try? await Task.sleep(for: .milliseconds(16))
                    worst = max(worst, Watchdog.threadCPU() - before)
                }
                try? await Task.sleep(for: .milliseconds(100))
                await phase(name)
                log(String(format: "input %@: %d steps, %.1f ms a step, at most %.1f, stall %@", name, steps, (Watchdog.threadCPU() - start) / Double(steps), worst, stalls.last ?? ""))
            }
            func wheel(_ pixels: Int, over target: NSView, sideways: Bool = false) {
                let point = target.window?.convertPoint(toScreen: target.convert(CGPoint(x: target.bounds.midX, y: target.bounds.midY), to: nil)) ?? .zero
                guard let event = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2, wheel1: sideways ? 0 : Int32(pixels), wheel2: sideways ? Int32(pixels) : 0, wheel3: 0) else { return }
                event.location = CGPoint(x: point.x, y: (NSScreen.screens.first?.frame.height ?? 0) - point.y)
                if let event = NSEvent(cgEvent: event) { window.sendEvent(event) }
            }
            func mouse(_ type: NSEvent.EventType, at point: NSPoint) {
                if let event = NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                                                  context: nil, eventNumber: 0, clickCount: 1, pressure: type == .mouseMoved ? 0 : 1) { window.sendEvent(event) }
            }
            window.acceptsMouseMovedEvents = true
            func resize(_ name: String) async {
                let original = window.frame
                await act("resizing the window, " + name, steps: 60) { index in
                    var size = original.size
                    size.width -= Double(index < 30 ? index : 60 - index) * 4
                    window.setFrame(NSRect(origin: original.origin, size: size), display: true)
                }
                window.setFrame(original, display: true)
                try? await Task.sleep(for: .milliseconds(200))
            }
            // Open the biggest folder in the tree, and come back to the big file.
            let current = thread.files?.selectedPath
            let folders = Dictionary(grouping: candidates, by: { ($0.path as NSString).deletingLastPathComponent })
            if let folder = folders.max(by: { $0.value.count < $1.value.count })?.value.first?.path {
                thread.openFile(atPath: root.appending(path: folder).path)
                try? await Task.sleep(for: .milliseconds(800))
            }
            if let current { thread.openFile(atPath: root.appending(path: current).path) }
            try? await Task.sleep(for: .milliseconds(500))
            await act("wheel over a big file", steps: 150) { wheel($0 < 75 ? -40 : 40, over: view.scrollView) }
            await act("wheel sideways over it", steps: 60) { wheel($0 < 30 ? -30 : 30, over: view.scrollView, sideways: true) }
            if let tree: NSOutlineView = findView(), let scroller = tree.enclosingScrollView {
                log("tree has \(tree.numberOfRows) rows")
                await act("wheel over the tree", steps: 150) { wheel($0 < 75 ? -40 : 40, over: scroller) }
                let frame = scroller.convert(scroller.bounds, to: nil)
                await act("pointer over the tree", steps: 100) { mouse(.mouseMoved, at: NSPoint(x: frame.midX, y: frame.minY + frame.height * Double($0 % 50) / 50)) }
                // What a drag does: synthetic events don't reach its gesture.
                await act("dragging the divider", steps: 100) { UserDefaults.standard.set(220 + Double($0 < 50 ? $0 : 100 - $0) * 2, forKey: "filesTreeHeight") }
                UserDefaults.standard.removeObject(forKey: "filesTreeHeight")
            }
            // Between tabs, timed until the file shows, and whether a scrolled file comes back
            // where it was. Through the model: synthetic clicks don't reach SwiftUI's tap gestures.
            if let files = thread.files, let big = biggest?.path {
                for path in files.tabs.map(\.path) { files.close(path) }
                let small = candidates.filter { $0.size < 2_000 }.prefix(2).map(\.path)
                for path in small + [big] {
                    thread.openFile(atPath: root.appending(path: path).path, keep: true)
                    try? await Task.sleep(for: .milliseconds(300))
                }
                func timed(_ path: String, _ action: () -> Void) async -> Double? {
                    let started = ContinuousClock.now
                    var timeout: Task<Void, Never>?
                    let shown = await withCheckedContinuation { continuation in
                        waiting = (path, continuation)
                        action()
                        timeout = Task {
                            try? await Task.sleep(for: .seconds(2))
                            if !Task.isCancelled, let waiting, waiting.path == path {
                                self.waiting = nil
                                waiting.continuation.resume(returning: false)
                            }
                        }
                    }
                    timeout?.cancel()
                    return shown ? (ContinuousClock.now - started) / .milliseconds(1) : nil
                }
                func format(_ times: [Double?]) -> String {
                    times.map { $0.map { String(format: "%.0f", $0) } ?? "none" }.joined(separator: ", ") + " ms"
                }
                if small.count == 2 {
                    var direct: [Double?] = []
                    for index in 0..<6 {
                        let path = small[index % 2]
                        if files.selectedPath == path { files.select(small[(index + 1) % 2]); try? await Task.sleep(for: .milliseconds(200)) }
                        direct.append(await timed(path) { files.select(path) })
                        try? await Task.sleep(for: .milliseconds(200))
                    }
                    log("switch small files, no click: \(format(direct))")
                    var large: [Double?] = []
                    for index in 0..<6 {
                        let path = index % 2 == 0 ? big : small[0]
                        large.append(await timed(path) { files.select(path) })
                        try? await Task.sleep(for: .milliseconds(200))
                    }
                    log("switch to and from the big file, no click: \(format(large))")
                    await phase("switching tabs")
                    log("switching stall \(stalls.last ?? "")")
                    files.select(big)
                    try? await Task.sleep(for: .milliseconds(300))
                    if let view = fileScrollView() {
                        view.scroll(to: NSPoint(x: 0, y: 500_000))
                        try? await Task.sleep(for: .milliseconds(200))
                        let before = view.scrollView.contentView.bounds.origin.y
                        files.select(small[0])
                        try? await Task.sleep(for: .milliseconds(300))
                        files.select(big)
                        try? await Task.sleep(for: .milliseconds(500))
                        log("scrolled to \(Int(before)), back at \(Int(view.scrollView.contentView.bounds.origin.y))")
                    }
                }
            }
            await resize("big file")
            if let field: NSTextField = findView(where: { $0.placeholderString == "Go to File" }) {
                window.makeFirstResponder(field)
                try? await Task.sleep(for: .milliseconds(100))
                await act("typing in Go to File", steps: 12) { index in
                    (window.firstResponder as? NSText)?.insertText(String("catalogcatal".dropFirst(index).prefix(1)))
                }
                (window.firstResponder as? NSText)?.string = ""
                window.makeFirstResponder(nil)
                thread.files?.selectedPath.map { thread.files?.select($0) }
            }
            // A minified file: one enormous line.
            if let minified = candidates.first(where: { $0.path.hasSuffix(".min.js") }) {
                let started = ContinuousClock.now
                let shown = await withCheckedContinuation { continuation in
                    waiting = (minified.path, continuation)
                    thread.openFile(atPath: root.appending(path: minified.path).path)
                    Task {
                        try? await Task.sleep(for: .seconds(10))
                        if let waiting, waiting.path == minified.path {
                            self.waiting = nil
                            waiting.continuation.resume(returning: false)
                        }
                    }
                }
                log(String(format: "%@ %@ (%@) in %.0f ms", shown ? "shown" : "NOT SHOWN", minified.path, minified.size.formatted(.byteCount(style: .file)), (ContinuousClock.now - started) / .milliseconds(1)))
                try? await Task.sleep(for: .milliseconds(300))
                await phase("open minified")
                if let view = fileScrollView() {
                    await act("wheel over minified", steps: 60) { wheel($0 < 30 ? -30 : 30, over: view.scrollView, sideways: true) }
                }
            }
            if let small = candidates.filter({ $0.size < 2_000 }).first(where: { $0.path.hasSuffix(".ts") }) {
                thread.openFile(atPath: root.appending(path: small.path).path)
                try? await Task.sleep(for: .milliseconds(400))
                await resize("small file")
            }
            app.showInspector(.sideChat)
            try? await Task.sleep(for: .milliseconds(400))
            await resize("Side Chat tab")
            app.setShowsInspector(false)
            try? await Task.sleep(for: .milliseconds(400))
            await resize("sidebar closed")
            app.showInspector(.files)
            try? await Task.sleep(for: .milliseconds(400))
        }
        let opened = MemoryFootprint.current()
        thread.files?.tabs.map(\.path).forEach { thread.files?.close($0) }
        try? await Task.sleep(for: .seconds(1))
        await phase("close")
        let cpu = Watchdog.threadCPU() - cpuStart
        let after = MemoryFootprint.current()
        let ok = stall < 50 && failed == 0
        let sorted = times.sorted()
        log(String(format: "\n%@  opened %d files (%d not shown): median %.0f ms, slowest %.0f ms; %d scroll steps; main thread busy %.0f ms; longest stall %.0f ms (%@)\nfootprint %@ before, %@ with them open, %@ after closing\n%@",
                   ok ? "PASS" : "FAIL", picked.count, failed, sorted[sorted.count / 2], sorted.last ?? 0, scrolls, cpu, stall, stalls.joined(separator: ", "),
                   before.formatted(.byteCount(style: .memory)), opened.formatted(.byteCount(style: .memory)), after.formatted(.byteCount(style: .memory)),
                   ok ? "PASS" : "FAIL"))
        if ProcessInfo.processInfo.environment["YH_FILES_PROBE_QUIT"] == "1" { NSApp.terminate(nil) }
    }

    private static func fileScrollView() -> FileScrollView? {
        findView()
    }

    private static func findView<V: NSView>(where matches: @escaping (V) -> Bool = { _ in true }) -> V? {
        func find(_ view: NSView) -> V? {
            if let found = view as? V, matches(found) { return found }
            for subview in view.subviews { if let found = find(subview) { return found } }
            return nil
        }
        return NSApp.windows.lazy.compactMap { $0.contentView.flatMap(find) }.first
    }
}
