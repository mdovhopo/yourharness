// Demo recordings (`YH_PHONE_DEMO=<steps as JSON>`, scripts/demo/README.md): the phone acts out a
// script while `simctl io recordVideo` films it, as the Mac app's `YH_DEMO` does over there. Each
// step prints `DEMO <seconds since 1970> step <n>: …` so the edit can line both films up.
//
//   [{"wait_connected": 20}, {"open": "heatmap"}, {"type": "Make the cells smaller", "cps": 14},
//    {"send": true}, {"wait": 6}, {"back": true}, {"settings": true}, {"tab": "mac"}]

import Foundation
import Observation

@Observable @MainActor
final class Demo {
    static let shared = Demo()

    /// What the open thread's composer should hold, typed a letter at a time.
    var draft: String?
    /// Bumped to press Send in the open thread's composer.
    var sendTick = 0
    /// The phone's Settings sheet, open or not.
    var settingsOpen: Bool?

    private var started = false

    func start(fleet: Fleet) {
        guard !started, let json = ProcessInfo.processInfo.environment["YH_PHONE_DEMO"]?.data(using: .utf8),
              let steps = try? JSONSerialization.jsonObject(with: json) as? [[String: Any]] else { return }
        started = true
        Task { @MainActor in
            for (n, step) in steps.enumerated() {
                print(String(format: "DEMO %.3f step %d: %@", Date().timeIntervalSince1970, n, String(describing: step)))
                await run(step, fleet: fleet)
            }
            print(String(format: "DEMO %.3f done", Date().timeIntervalSince1970))
        }
    }

    private func run(_ step: [String: Any], fleet: Fleet) async {
        let store = fleet.current
        if let s = step["wait"] as? Double {
            try? await Task.sleep(for: .seconds(s))
        } else if let max = step["wait_connected"] as? Double {
            let until = Date().addingTimeInterval(max)
            while Date() < until, store.state != .connected || store.sections.isEmpty {
                try? await Task.sleep(for: .milliseconds(200))
            }
        } else if let title = (step["open"] as? String)?.lowercased() {
            let all = store.sections.flatMap { $0.live + $0.settled }
            if let t = all.first(where: { $0.title.lowercased().contains(title) }) {
                draft = nil
                store.path = [t.id]
            } else {
                print("DEMO no thread \(title)")
            }
        } else if step["back"] != nil {
            store.path = []
        } else if let text = step["type"] as? String {
            let cps = step["cps"] as? Double ?? 14
            var typed = ""
            for (i, ch) in text.enumerated() {
                typed.append(ch)
                draft = typed
                // A thumb's rhythm: a little uneven, a beat after a space.
                let jitter = Double((i * 7919) % 13) / 13 * 0.5 + 0.75
                try? await Task.sleep(for: .seconds(jitter * (ch == " " ? 1.4 : 1) / cps))
            }
        } else if step["send"] != nil {
            sendTick += 1
        } else if let open = step["settings"] as? Bool {
            settingsOpen = open
        } else if let tab = step["tab"] as? String {
            switch tab {
            case "threads": fleet.tab = .threads
            case "usage": fleet.tab = .usage
            case "mac": fleet.tab = .mac
            default: print("DEMO no tab \(tab)")
            }
        } else {
            print("DEMO unknown step \(step)")
        }
        if let after = step["after"] as? Double {
            try? await Task.sleep(for: .seconds(after))
        }
    }
}
