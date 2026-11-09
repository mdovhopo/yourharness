// Measurements for test runs (docs/mobile.md, M3). `YH_PERF=1` counts frames that ran late
// (a hitch: more than 1.5 frames between two), the worst frame, and how long the main thread
// spent applying the Mac's changes, printed every 5 s as `PERF …` lines. Off by default: no
// display link runs unless asked.

import QuartzCore
import UIKit

@MainActor
final class Perf {
    static let shared = Perf()
    static let enabled = ProcessInfo.processInfo.environment["YH_PERF"] == "1"

    private var link: CADisplayLink?
    private var last: CFTimeInterval = 0
    private var frames = 0
    private var hitches = 0
    private var worstFrame: CFTimeInterval = 0
    private var applies: [CFTimeInterval] = []
    private var rowsApplied = 0
    private var started: CFTimeInterval = 0
    private var totals = (frames: 0, hitches: 0, worst: 0.0, applyWorst: 0.0)

    func start() {
        guard Perf.enabled, link == nil else { return }
        let link = CADisplayLink(target: self, selector: #selector(tick(_:)))
        link.add(to: .main, forMode: .common)
        self.link = link
        started = CACurrentMediaTime()
        Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { _ in
            MainActor.assumeIsolated { Perf.shared.report() }
        }
    }

    @objc private func tick(_ link: CADisplayLink) {
        defer { last = link.timestamp }
        guard last > 0 else { return }
        let interval = link.timestamp - last
        let expected = link.targetTimestamp - link.timestamp
        frames += 1
        worstFrame = max(worstFrame, interval)
        if interval > max(expected, 1.0 / 120) * 1.5 { hitches += 1 }
    }

    /// Times `work` as one application of changes from the Mac.
    func measure<T>(rows: Int, _ work: () -> T) -> T {
        guard Perf.enabled else { return work() }
        let t = CACurrentMediaTime()
        let result = work()
        applies.append(CACurrentMediaTime() - t)
        rowsApplied += rows
        return result
    }

    func log(_ line: String) {
        print(line)
        fflush(stdout)
    }

    private func report() {
        let sorted = applies.sorted()
        let worstApply = sorted.last ?? 0
        let p95 = sorted.isEmpty ? 0 : sorted[min(sorted.count - 1, Int(Double(sorted.count) * 0.95))]
        totals.frames += frames
        totals.hitches += hitches
        totals.worst = max(totals.worst, worstFrame)
        totals.applyWorst = max(totals.applyWorst, worstApply)
        log(String(format: "PERF %.0fs frames=%d hitches=%d worst=%.1fms applies=%d rows=%d apply_p95=%.1fms apply_worst=%.1fms",
                   CACurrentMediaTime() - started, frames, hitches, worstFrame * 1000, applies.count, rowsApplied, p95 * 1000, worstApply * 1000))
        log(String(format: "PERF total frames=%d hitches=%d (%.2f%%) worst=%.1fms apply_worst=%.1fms",
                   totals.frames, totals.hitches, totals.frames == 0 ? 0 : Double(totals.hitches) * 100 / Double(totals.frames),
                   totals.worst * 1000, totals.applyWorst * 1000))
        frames = 0
        hitches = 0
        worstFrame = 0
        applies = []
        rowsApplied = 0
    }
}
