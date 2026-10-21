import Foundation
import HarnessCore

/// Development: `YH_BROWSER_PROBE=<steps.json>` runs browser tool calls on the selected
/// thread at launch, as the agent would make them, and prints each result to stderr;
/// screenshots go next to the file as `<steps>.<n>.jpg`. The file is a list of
/// `{"tool": "click", "args": {...}}`; `{"sleep": seconds}` waits. Off unless set.
@MainActor
enum BrowserProbe {
    static func runIfRequested(app: AppModel) {
        guard let path = ProcessInfo.processInfo.environment["YH_BROWSER_PROBE"], !path.isEmpty,
              let data = FileManager.default.contents(atPath: path),
              let steps = JSONValue(json: String(decoding: data, as: UTF8.self))?.array,
              let thread = app.selectedThread
        else { return }
        let tools = BrowserTools(thread: thread, app: app, readOnly: false)
        Task {
            try? await Task.sleep(for: .seconds(1))
            for (index, step) in steps.enumerated() {
                if let seconds = step["sleep"]?.double {
                    try? await Task.sleep(for: .seconds(seconds))
                    continue
                }
                guard let tool = step["tool"]?.string else { continue }
                let started = Date()
                let result = await tools.call(tool, step["args"] ?? [:], toolUseID: "probe-\(index)")
                var lines = ["== \(index) \(tool) \(step["args"]?.json ?? "{}") (\(Int(Date().timeIntervalSince(started) * 1000)) ms)\(result.isError ? " ERROR" : "")"]
                for part in result.content {
                    switch part {
                    case .text(let text):
                        lines.append(text)
                    case .image(let image, _):
                        let file = "\(path).\(index).jpg"
                        try? image.write(to: URL(fileURLWithPath: file))
                        lines.append("[image \(image.count) bytes → \(file)]")
                    }
                }
                FileHandle.standardError.write(Data((lines.joined(separator: "\n") + "\n").utf8))
            }
            FileHandle.standardError.write(Data("== done\n".utf8))
        }
    }
}
