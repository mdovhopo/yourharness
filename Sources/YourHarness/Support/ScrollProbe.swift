import AppKit

/// `YH_SCROLL_PROBE=<points>`: scrolls the open thread up (negative: down) in trackpad-sized
/// steps and prints how often a row on screen moved by more or less than the step, the jump
/// count. `YH_SCROLL_PROBE=log` prints the scroll geometry instead.
@MainActor
enum ScrollProbe {
    private static let setting = ProcessInfo.processInfo.environment["YH_SCROLL_PROBE"]
    static let isOn = setting != nil
    /// Where each row on screen is, for comparing between steps.
    static var positions: [String: CGFloat] = [:]
    private static var ran = false

    static func runIfRequested(on list: TranscriptListView) {
        guard let setting, !ran else { return }
        ran = true
        if setting == "log" {
            Task {
                while true {
                    try? await Task.sleep(for: .milliseconds(100))
                    FileHandle.standardError.write(Data("offset \(Int(list.probeOffset)) viewport \(Int(list.probeViewport))\n".utf8))
                }
            }
        } else if let step = Double(setting) {
            Task { await run(list, step: CGFloat(step)) }
        }
    }

    /// Up from the bottom for a positive step, down from the top for a negative one.
    private static func run(_ list: TranscriptListView, step: CGFloat) async {
        try? await Task.sleep(for: .seconds(2))
        if step < 0 {
            list.goToTopForProbe()
            try? await Task.sleep(for: .seconds(1))
        }
        var steps = 0
        var jumps: [Int] = []
        while steps < 2000 {
            let start = list.probeOffset, before = positions
            list.scrollForProbe(by: step)
            try? await Task.sleep(for: .milliseconds(80))
            let moved = start - list.probeOffset
            guard abs(moved) > 0.5 else { break }
            steps += 1
            // The last step up stops short at the top.
            if list.probeOffset <= -list.probeTopInset + 0.5 { continue }
            // What you'd see: a row on screen should move by exactly the step.
            for (id, y) in positions {
                guard let old = before[id], (0...list.probeViewport).contains(old), (0...list.probeViewport).contains(y) else { continue }
                let off = y - old - step
                if abs(off) > 2 { jumps.append(Int(off.rounded())) }
            }
        }
        let worst = jumps.sorted { abs($0) > abs($1) }.prefix(5)
        FileHandle.standardError.write(Data("scroll probe: \(steps) steps, \(jumps.count) jumps, worst \(Array(worst))\n".utf8))
    }
}
