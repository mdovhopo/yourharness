import Foundation
import HarnessCore

/// Something Claude left running: a command in the background (a build, a dev server, a
/// loop sampling a metric), a foreground one that's taking a while, or a watch (Monitor).
/// Hangs off the tool call that started it. Lives with the `claude` process: never saved.
@MainActor @Observable
final class BackgroundJob: Identifiable {
    enum Status: Equatable {
        case running, completed, failed, stopped
    }

    /// The task's ID, what `stop_task` takes.
    let id: String
    let toolUseID: String?
    var description: String
    /// Started in the background: the turn didn't wait for it.
    let isBackground: Bool
    /// A Monitor watch, which wakes Claude with what it sees, rather than a command.
    let isWatch: Bool
    /// The message of yours it started under: while it's the latest, Claude is waiting on it.
    let turnID: String?
    let startedAt: Date
    private(set) var status = Status.running
    private(set) var endedAt: Date?
    private(set) var exitCode: Int?
    /// Everything it printed, as the CLI writes it.
    var outputFile: String?
    /// What a watch last saw, or the command's last line of output.
    var lastLine: String?

    var isRunning: Bool { status == .running }

    init(_ start: JobStart, toolName: String?, turnID: String?, startedAt: Date = .now) {
        id = start.taskID
        toolUseID = start.toolUseID
        description = start.description
        isBackground = start.isBackground
        isWatch = toolName == "Monitor"
        self.turnID = turnID
        self.startedAt = startedAt
    }

    func finish(_ end: JobEnd) {
        guard isRunning else { return }
        status = switch end.status {
        case "completed": end.exitCode.map { $0 == 0 ? .completed : .failed } ?? .completed
        case "failed", "error": .failed
        default: .stopped
        }
        exitCode = end.exitCode
        endedAt = .now
        if let file = end.outputFile { outputFile = file }
    }

    /// Its tool call came back (a foreground one), or the process it ran in went away.
    func finish(failed: Bool) {
        guard isRunning else { return }
        status = failed ? .failed : .completed
        endedAt = .now
    }

    func abandon() {
        guard isRunning else { return }
        status = .stopped
        endedAt = .now
    }

    /// "finished", "failed with exit code 1", "was stopped".
    var outcome: String {
        switch status {
        case .running: "is running"
        case .completed: isWatch ? "ended" : "finished"
        case .failed: exitCode.map { "failed with exit code \($0)" } ?? "failed"
        case .stopped: "was stopped"
        }
    }

    /// The last few KB of what it printed, read off the main thread. The CLI ends a
    /// command's file with `[exited with code 0]`, which says nothing the status doesn't.
    nonisolated static func tail(of path: String, bytes: Int = 8_192) async -> String? {
        await Task.detached(priority: .utility) {
            guard let handle = FileHandle(forReadingAtPath: path) else { return nil }
            defer { try? handle.close() }
            let size = (try? handle.seekToEnd()) ?? 0
            try? handle.seek(toOffset: size > UInt64(bytes) ? size - UInt64(bytes) : 0)
            guard let data = try? handle.readToEnd() else { return nil }
            var text = String(decoding: data, as: UTF8.self)
            if size > UInt64(bytes), let newline = text.firstIndex(of: "\n") { text = String(text[text.index(after: newline)...]) }
            let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
                .filter { !$0.hasPrefix("[exited with code ") }
            return lines.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
        }.value
    }

    /// Where the CLI writes a job's output, from what it said about another job of the
    /// same session ("Output is being written to: …/tasks/b1.output."): a foreground
    /// command's own path is only told once it ends.
    static func outputFile(inResult result: String) -> String? {
        guard let start = result.range(of: "Output is being written to: ") else { return nil }
        let rest = result[start.upperBound...]
        guard let end = rest.range(of: ".output") else { return nil }
        return String(rest[..<end.upperBound])
    }
}
