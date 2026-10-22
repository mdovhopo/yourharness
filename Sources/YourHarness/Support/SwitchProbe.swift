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

    /// From the thread view, when it's on screen.
    static func threadAppeared(_ id: UUID) {
        appeared.removeValue(forKey: id)?.resume()
    }

    static func runIfRequested(app: AppModel) {
        guard let rounds = ProcessInfo.processInfo.environment["YH_SWITCH_PROBE"].flatMap(Int.init) else { return }
        Task {
            try? await Task.sleep(for: .seconds(3))
            let ids = app.visibleThreadIDs
            var lines: [String] = [], firsts: [Double] = [], agains: [Double] = []
            for round in 0..<rounds {
                for id in ids where id != app.selectedThreadID {
                    let clock = ContinuousClock(), start = clock.now
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
                    let ms = { (d: Duration) in Double(d.components.attoseconds) / 1e15 + Double(d.components.seconds) * 1000 }
                    if round == 0 { firsts.append(ms(idle)) } else { agains.append(ms(idle)) }
                    let title = app.threads.first { $0.id == id }?.title ?? "?"
                    lines.append(String(format: "round %d  shown %6.0f ms  idle %6.0f ms  %@", round + 1, ms(shown), ms(idle), title))
                    try? await Task.sleep(for: .milliseconds(300))
                }
            }
            let summary = { (name: String, values: [Double]) -> String in
                guard !values.isEmpty else { return "" }
                let sorted = values.sorted()
                return String(format: "%@: median %.0f ms, worst %.0f ms", name, sorted[sorted.count / 2], sorted.last!)
            }
            let report = lines + [summary("first visit", firsts), summary("visited before", agains)]
            FileHandle.standardError.write(Data(("SWITCH PROBE\n" + report.joined(separator: "\n") + "\n").utf8))
        }
    }
}
