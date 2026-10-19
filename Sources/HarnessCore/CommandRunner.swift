import Foundation
import Synchronization

public struct CommandOutput: Sendable, Equatable {
    public var status: Int32
    public var stdout: String
    public var stderr: String
    public var timedOut: Bool
    public var launchFailed: Bool

    public var succeeded: Bool { status == 0 && !timedOut && !launchFailed }
}

/// Runs short-lived commands (`--version`, `auth status`) with a hard timeout.
///
/// Completes on process exit or timeout, never on pipe EOF: wrapper scripts often leave a
/// grandchild holding the pipe open, and waiting for EOF would hang forever.
public enum CommandRunner {
    public static func run(
        _ executable: URL,
        arguments: [String],
        environment: [String: String]? = nil,
        timeout: TimeInterval = 8,
        outputLimit: Int = 256 * 1024
    ) async -> CommandOutput {
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        if let environment { process.environment = environment }
        process.currentDirectoryURL = FileManager.default.homeDirectoryForCurrentUser
        process.standardInput = FileHandle.nullDevice
        let stdout = Pipe(), stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr

        let collector = Collector(limit: outputLimit)
        stdout.fileHandleForReading.readabilityHandler = { handle in
            let chunk = handle.availableData
            if chunk.isEmpty { handle.readabilityHandler = nil } else { collector.append(chunk, isStderr: false) }
        }
        stderr.fileHandleForReading.readabilityHandler = { handle in
            let chunk = handle.availableData
            if chunk.isEmpty { handle.readabilityHandler = nil } else { collector.append(chunk, isStderr: true) }
        }
        let detach: @Sendable () -> Void = {
            stdout.fileHandleForReading.readabilityHandler = nil
            stderr.fileHandleForReading.readabilityHandler = nil
        }

        return await withCheckedContinuation { continuation in
            collector.setContinuation(continuation)
            process.terminationHandler = { process in
                let status = process.terminationStatus
                // Let the readers drain what the child wrote right before exiting.
                DispatchQueue.global().asyncAfter(deadline: .now() + 0.05) {
                    if collector.finish(status: status, timedOut: false) { detach() }
                }
            }
            do {
                try process.run()
            } catch {
                collector.finish(status: -1, timedOut: false, launchFailed: true, message: error.localizedDescription)
                detach()
                return
            }
            let pid = process.processIdentifier
            DispatchQueue.global().asyncAfter(deadline: .now() + timeout) {
                if collector.finish(status: -1, timedOut: true) {
                    kill(pid, SIGKILL)
                    detach()
                }
            }
        }
    }

    private final class Collector: Sendable {
        private struct State {
            var stdout = Data()
            var stderr = Data()
            var continuation: CheckedContinuation<CommandOutput, Never>?
            var finished = false
        }

        private let state = Mutex(State())
        private let limit: Int

        init(limit: Int) {
            self.limit = limit
        }

        func setContinuation(_ continuation: CheckedContinuation<CommandOutput, Never>) {
            state.withLock { $0.continuation = continuation }
        }

        func append(_ data: Data, isStderr: Bool) {
            state.withLock { state in
                if isStderr {
                    if state.stderr.count < limit { state.stderr.append(data) }
                } else {
                    if state.stdout.count < limit { state.stdout.append(data) }
                }
            }
        }

        /// Returns true for the caller that actually finished; later calls are no-ops.
        @discardableResult
        func finish(status: Int32, timedOut: Bool, launchFailed: Bool = false, message: String? = nil) -> Bool {
            let result: (CheckedContinuation<CommandOutput, Never>, CommandOutput)? = state.withLock { state in
                guard !state.finished, let continuation = state.continuation else { return nil }
                state.finished = true
                state.continuation = nil
                let output = CommandOutput(
                    status: status,
                    stdout: String(decoding: state.stdout, as: UTF8.self),
                    stderr: message ?? String(decoding: state.stderr, as: UTF8.self),
                    timedOut: timedOut,
                    launchFailed: launchFailed
                )
                return (continuation, output)
            }
            guard let (continuation, output) = result else { return false }
            continuation.resume(returning: output)
            return true
        }
    }
}
