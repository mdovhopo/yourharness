import AppKit
import HarnessCore

/// `YH_FOLLOW_PROBE=<steps>`: plays a long turn into the selected thread, as Claude Code
/// writes it (stream-json lines through the real parser, no session), and checks the
/// transcript keeps the newest output in view without being touched: text streaming in,
/// tool calls, a big block arriving at once, long tool output, and the work folding away at
/// the end. Halfway, it scrolls up with the wheel (events handed to the window, nothing on
/// screen moves) and checks the transcript then stays where it was put, and that jumping
/// to the end follows again, and that it follows from the start (a transcript just shown
/// settles, which isn't scrolling). Prints each time the end went out of view on its own and
/// PASS/FAIL; `YH_FOLLOW_PROBE_QUIT=1` quits after.
@MainActor
enum FollowProbe {
    static func runIfRequested(app: AppModel) {
        guard let steps = ProcessInfo.processInfo.environment["YH_FOLLOW_PROBE"].flatMap(Int.init) else { return }
        Task { await run(app: app, steps: steps) }
    }

    private static func log(_ line: String) {
        FileHandle.standardError.write(Data("FOLLOW PROBE \(line)\n".utf8))
    }

    /// What the transcript reported last: how far its end is below the bottom of the screen.
    private static func gap(_ thread: ThreadModel) -> CGFloat? {
        guard let scroll = TranscriptDiagnostics.scroll[thread.id] else { return nil }
        return scroll.contentHeight - (scroll.offset + scroll.containerHeight + scroll.insetTop)
    }

    private static func run(app: AppModel, steps: Int) async {
        guard let thread = app.selectedThread else { return log("FAIL: no thread selected") }
        await thread.loadTranscriptSoon()
        // History above, so the thread is longer than the screen when the turn folds away.
        if thread.entries.isEmpty { thread.seed(StressTest.entries(count: 60)) }
        try? await Task.sleep(for: .seconds(1.5))
        log("pid \(ProcessInfo.processInfo.processIdentifier), \(steps) steps")

        var parser = ClaudeStreamParser()
        var failures: [String] = []
        var step = 0
        /// While true, the end must be in view; out of it for longer than an animation, it
        /// went away on its own.
        var expectsEnd = true
        let start = Date.now
        let watch = Task { @MainActor in
            var awaySince: Date?
            var reported = false
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(50))
                guard expectsEnd, let gap = gap(thread) else { awaySince = nil; continue }
                if gap <= 40 {
                    awaySince = nil
                    reported = false
                } else if let since = awaySince {
                    if !reported, Date.now.timeIntervalSince(since) > 0.8 {
                        reported = true
                        let line = String(format: "%.1fs step %d: the end went out of view (%.0f pt below), following %@",
                                          since.timeIntervalSince(start), step, gap,
                                          TranscriptDiagnostics.scroll[thread.id]?.followsBottom == true ? "on" : "off")
                        log(line)
                        failures.append(line)
                    }
                } else {
                    awaySince = .now
                }
            }
        }

        func play(_ object: [String: Any]) {
            guard let line = try? JSONSerialization.data(withJSONObject: object) else { return }
            for event in parser.parse(line: line) { thread.applyForProbe(event) }
        }
        func pause(_ milliseconds: Int) async { try? await Task.sleep(for: .milliseconds(milliseconds)) }

        // Nobody has scrolled: shown at the end, it must still be following.
        if TranscriptDiagnostics.scroll[thread.id]?.followsBottom != true {
            failures.append("opened at the end, it wasn't following")
            log("FAIL: opened at the end, it wasn't following")
        }
        thread.beginProbeTurn("Follow probe: work through \(steps) steps and report.")
        play(["type": "system", "subtype": "init", "session_id": "follow-probe", "model": "claude-haiku-5-5"])
        await pause(400)

        var message = 0
        func reply(_ text: String, chunks: Int) async {
            message += 1
            let id = "msg_probe_\(message)"
            play(["type": "system", "subtype": "status", "status": "requesting"])
            play(["type": "stream_event", "api_message_id": id, "event": ["type": "message_start", "message": ["id": id, "model": "claude-haiku-5-5"]]])
            play(["type": "stream_event", "api_message_id": id, "event": ["type": "content_block_start", "index": 0, "content_block": ["type": "text", "text": ""]]])
            let size = max(1, text.count / chunks)
            var rest = Substring(text)
            while !rest.isEmpty {
                let piece = rest.prefix(size)
                rest = rest.dropFirst(piece.count)
                play(["type": "stream_event", "api_message_id": id, "event": ["type": "content_block_delta", "index": 0, "delta": ["type": "text_delta", "text": String(piece)]]])
                if !rest.isEmpty { await pause(Int.random(in: 30...90)) }
            }
            play(["type": "assistant", "message": ["id": id, "model": "claude-haiku-5-5", "content": [["type": "text", "text": text]]]])
            play(["type": "stream_event", "api_message_id": id, "event": ["type": "content_block_stop", "index": 0]])
        }
        func tool(_ name: String, input: [String: Any], output: String) async {
            message += 1
            let id = "msg_probe_\(message)", toolID = "toolu_probe_\(message)"
            let json = String(decoding: (try? JSONSerialization.data(withJSONObject: input)) ?? Data(), as: UTF8.self)
            play(["type": "stream_event", "api_message_id": id, "event": ["type": "message_start", "message": ["id": id, "model": "claude-haiku-5-5"]]])
            play(["type": "stream_event", "api_message_id": id, "event": ["type": "content_block_start", "index": 0, "content_block": ["type": "tool_use", "id": toolID, "name": name, "input": [:]]]])
            play(["type": "stream_event", "api_message_id": id, "event": ["type": "content_block_delta", "index": 0, "delta": ["type": "input_json_delta", "partial_json": json]]])
            play(["type": "assistant", "message": ["id": id, "model": "claude-haiku-5-5", "content": [["type": "tool_use", "id": toolID, "name": name, "input": input]]]])
            play(["type": "stream_event", "api_message_id": id, "event": ["type": "content_block_stop", "index": 0]])
            await pause(Int.random(in: 150...500))
            play(["type": "user", "message": ["role": "user", "content": [["type": "tool_result", "tool_use_id": toolID, "content": output, "is_error": false]]]])
        }

        func work(_ count: Int) async {
            for _ in 0..<count {
                step += 1
                switch step % 7 {
                case 3:
                    // A long answer arriving in one piece.
                    await reply(longText(step), chunks: 1)
                case 5:
                    await reply(longText(step), chunks: 24)
                default:
                    await reply("Step \(step): checking the next part of the change before moving on.", chunks: 6)
                }
                await pause(200)
                switch step % 4 {
                case 0:
                    await tool("Bash", input: ["command": "swift test --filter Step\(step)", "description": "Run the tests"],
                               output: (0..<300).map { "Test case \($0) of step \(step) passed (0.0\($0 % 10)s)" }.joined(separator: "\n"))
                case 2:
                    await tool("Edit", input: ["file_path": "/tmp/project/Sources/Step\(step).swift", "old_string": "let a = 1\nlet b = 2",
                                               "new_string": "let a = 1\nlet b = 3\nlet c = a + b"],
                               output: "The file has been updated.")
                default:
                    await tool("Read", input: ["file_path": "/tmp/project/Sources/Step\(step).swift"],
                               output: (0..<40).map { "\($0)\tlet value\($0) = \($0)" }.joined(separator: "\n"))
                }
                await pause(250)
            }
        }

        await work(steps / 2)

        // Scrolled up by hand: it must stay there while the turn goes on.
        let window = NSApp.windows.first { $0.frameAutosaveName == "main" }
        if window == nil { log("no main window: \(NSApp.windows.map { "\(type(of: $0)) “\($0.frameAutosaveName)” \($0.isVisible)" })") }
        if let window, let content = window.contentView {
            expectsEnd = false
            await pause(300)
            let point = window.convertPoint(toScreen: NSPoint(x: content.bounds.midX + 120, y: content.bounds.midY))
            for _ in 0..<12 {
                if let cg = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: 60, wheel2: 0, wheel3: 0) {
                    cg.location = CGPoint(x: point.x, y: (NSScreen.screens.first?.frame.height ?? 0) - point.y)
                    if let event = NSEvent(cgEvent: cg) { window.sendEvent(event) }
                }
                await pause(16)
            }
            await pause(500)
            let left = gap(thread) ?? 0
            let anchor = TranscriptDiagnostics.scroll[thread.id]?.visibleRows.first
            await work(3)
            await pause(600)
            let after = gap(thread) ?? 0
            let still = TranscriptDiagnostics.scroll[thread.id]?.visibleRows.contains { $0 == anchor } ?? false
            if left <= 40 {
                failures.append("the wheel didn't scroll the transcript up")
                log("FAIL: the wheel didn't scroll the transcript up (\(Int(left)) pt from the end)")
            } else if after <= 40 || !still {
                failures.append("scrolled up, it was pulled away from where it was put")
                log("FAIL: scrolled up \(Int(left)) pt from the end, then pulled to \(Int(after)) pt (row still in view: \(still))")
            } else {
                log(String(format: "scrolled up %.0f pt from the end: stayed put while the turn went on (now %.0f pt)", left, after))
            }
            thread.jumpToMessage(.bottom)
            await pause(700)
            expectsEnd = true
        }

        await work(steps - steps / 2)
        await reply("All \(step) steps are done.\n\n" + longText(step + 1), chunks: 20)
        play(["type": "result", "subtype": "success", "is_error": false, "duration_ms": Int(Date.now.timeIntervalSince(start) * 1000)])
        await pause(2500)
        watch.cancel()

        // The turn folded away: its answer at the bottom, in view.
        let rows = thread.transcriptDisplayRows
        let last = rows.last?.id
        let scroll = TranscriptDiagnostics.scroll[thread.id]
        let endGap = gap(thread) ?? .infinity
        let lastInView = last.map { scroll?.touchedRows.contains($0) ?? false } ?? false
        if endGap > 40 || !lastInView {
            let line = "after the turn ended the answer isn't at the bottom: \(Int(endGap)) pt below, last row in view: \(lastInView)"
                + " (rows end \(rows.suffix(3).map(\.id)), in view \(scroll?.touchedRows ?? []))"
            failures.append(line)
            log("FAIL: " + line)
        }
        log(failures.isEmpty ? "PASS: followed \(step) steps and the end of the turn" : "FAIL: \(failures.count) problem(s)")
        if ProcessInfo.processInfo.environment["YH_FOLLOW_PROBE_QUIT"] == "1" { NSApp.terminate(nil) }
    }

    private static func longText(_ step: Int) -> String {
        """
        ## Step \(step)

        Here's what changed and why. The reducer stays **pure**, and the session layer does the rest:

        1. Parse each line into an `AgentEvent`
        2. Apply events to the transcript
        3. Persist when the turn settles

        ```swift
        func apply(_ event: AgentEvent) {
            switch event {
            case .textDelta(let ref, let text):
                blocks[ref]?.text += text
            default:
                break
            }
        }
        ```

        > Deltas only invalidate the row they touch, so a long thread stays quick.

        | Part | Before | After |
        | --- | --- | --- |
        | Parse | 4.1 ms | 1.2 ms |
        | Apply | 2.0 ms | 0.4 ms |
        """
    }
}

/// `YH_FOLLOW_LOG=1`: every scroll report of the open transcript, and what it made of it
/// (following on or off), for telling why it stopped following.
enum FollowLog {
    static let isOn = ProcessInfo.processInfo.environment["YH_FOLLOW_LOG"] == "1"
    private static let start = Date.now

    static func write(_ line: String) {
        FileHandle.standardError.write(Data(String(format: "FOLLOW %.3f %@\n", Date.now.timeIntervalSince(start), line).utf8))
    }
}
