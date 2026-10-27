import Foundation
import HarnessCore

/// Development: `YH_THREADS_PROBE=<steps.json>` runs thread tool calls at launch, as an agent
/// would make them, and prints each result to stderr. The file is a list of
/// `{"as": "<thread id>", "tool": "list_threads", "args": {...}}`: `as` picks the calling
/// thread, whose project decides what it reaches (the selected thread when left out);
/// `{"sleep": seconds}` waits. Point `CLAUDE_CONFIG_DIR` at a scratch folder for the Claude
/// Code sessions. Off unless set.
@MainActor
enum ThreadToolsProbe {
    static func runIfRequested(app: AppModel) {
        guard let path = ProcessInfo.processInfo.environment["YH_THREADS_PROBE"], !path.isEmpty,
              let data = FileManager.default.contents(atPath: path),
              let steps = JSONValue(json: String(decoding: data, as: UTF8.self))?.array
        else { return }
        Task {
            try? await Task.sleep(for: .seconds(1))
            for (index, step) in steps.enumerated() {
                if let seconds = step["sleep"]?.double {
                    try? await Task.sleep(for: .seconds(seconds))
                    continue
                }
                guard let tool = step["tool"]?.string else { continue }
                let caller = step["as"]?.string.flatMap { id in app.threads.first { $0.id.uuidString.lowercased() == id.lowercased() } }
                    ?? app.selectedThread
                guard let caller else {
                    log("== \(index) no thread to call as")
                    continue
                }
                let result = await ThreadTools(thread: caller, app: app).call(tool, step["args"] ?? [:])
                let text = result.content.compactMap { if case .text(let text) = $0 { text } else { nil } }.joined(separator: "\n")
                log("== \(index) \(tool) \(step["args"]?.json ?? "{}") as “\(caller.title)”\(result.isError ? " ERROR" : "")\n\(text)")
            }
            log("== done; selected: “\(app.selectedThread?.title ?? "nothing")”")
        }
    }

    private static func log(_ text: String) {
        FileHandle.standardError.write(Data((text + "\n").utf8))
    }
}
