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
///   starting, for attaching Time Profiler.
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

    private static func findView<V: NSView>() -> V? {
        func find(_ view: NSView) -> V? {
            if let found = view as? V { return found }
            for subview in view.subviews { if let found = find(subview) { return found } }
            return nil
        }
        return NSApp.windows.lazy.compactMap { $0.contentView.flatMap(find) }.first
    }
}
