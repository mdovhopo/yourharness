import Foundation
import Synchronization

public struct ClaudeLaunchOptions: Sendable {
    public var executable: URL
    public var workingDirectory: URL
    public var environment: [String: String]
    public var model: String?
    public var permissionMode: String?
    /// Reasoning effort: low, medium, high, xhigh, max. Launch-time only.
    public var effort: String?
    /// Resume an existing Claude session (survives app restarts and idle reaping).
    public var resumeSessionID: String?
    /// Extra instructions after Claude Code's own system prompt, e.g. what this app renders.
    public var appendSystemPrompt: String?

    public init(
        executable: URL,
        workingDirectory: URL,
        environment: [String: String],
        model: String? = nil,
        permissionMode: String? = nil,
        effort: String? = nil,
        resumeSessionID: String? = nil,
        appendSystemPrompt: String? = nil
    ) {
        self.executable = executable
        self.workingDirectory = workingDirectory
        self.environment = environment
        self.model = model
        self.permissionMode = permissionMode
        self.effort = effort
        self.resumeSessionID = resumeSessionID
        self.appendSystemPrompt = appendSystemPrompt
    }

    var arguments: [String] {
        var args = [
            "-p",
            "--input-format", "stream-json",
            "--output-format", "stream-json",
            "--verbose",
            "--include-partial-messages",
            // Route permission prompts to us as `control_request`s instead of denying.
            "--permission-prompt-tool", "stdio",
        ]
        if let model { args += ["--model", model] }
        if let permissionMode { args += ["--permission-mode", permissionMode] }
        if let appendSystemPrompt { args += ["--append-system-prompt", appendSystemPrompt] }
        if let effort { args += ["--effort", effort] }
        if let resumeSessionID { args += ["--resume", resumeSessionID] }
        return args
    }
}

/// One long-lived `claude -p` process speaking stream-json over stdin/stdout.
/// The process stays alive between turns, so follow-up messages skip CLI startup.
public actor ClaudeSession {
    public nonisolated let events: AsyncStream<AgentEvent>

    private let process: Process
    private let stdin: FileHandle
    private var requestCounter = 0

    public init(options: ClaudeLaunchOptions) throws {
        let process = Process()
        process.executableURL = options.executable
        process.arguments = options.arguments
        process.currentDirectoryURL = options.workingDirectory
        process.environment = options.environment

        let stdinPipe = Pipe(), stdoutPipe = Pipe(), stderrPipe = Pipe()
        process.standardInput = stdinPipe
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe

        let (events, continuation) = AsyncStream.makeStream(of: AgentEvent.self, bufferingPolicy: .unbounded)
        let reader = OutputReader(continuation: continuation)

        stdoutPipe.fileHandleForReading.readabilityHandler = { handle in
            let chunk = handle.availableData
            if chunk.isEmpty {
                handle.readabilityHandler = nil
                reader.stdoutClosed()
            } else {
                reader.consume(chunk)
            }
        }
        stderrPipe.fileHandleForReading.readabilityHandler = { handle in
            let chunk = handle.availableData
            if chunk.isEmpty {
                handle.readabilityHandler = nil
            } else {
                reader.appendStderr(chunk)
            }
        }
        process.terminationHandler = { process in
            reader.processExited(code: process.terminationStatus)
        }

        try process.run()
        self.process = process
        self.stdin = stdinPipe.fileHandleForWriting
        // Writing to a process that just died should throw, not kill the app with SIGPIPE.
        _ = fcntl(stdinPipe.fileHandleForWriting.fileDescriptor, F_SETNOSIGPIPE, 1)
        self.events = events
    }

    /// Images go before the text, which is how the model reads them best.
    public func send(text: String, images: [ImageAttachment] = []) throws {
        var content: [[String: Any]] = images.map(\.messageBlock)
        if !text.isEmpty || content.isEmpty { content.append(["type": "text", "text": text]) }
        try write([
            "type": "user",
            "message": ["role": "user", "content": content],
        ])
    }

    /// Stops the current turn. The process survives and accepts the next message.
    public func interrupt() throws {
        try control(["subtype": "interrupt"])
    }

    /// Stops one sub-agent; the turn, and any other sub-agents, carry on.
    public func stopTask(_ taskID: String) throws {
        try control(["subtype": "stop_task", "task_id": taskID])
    }

    /// Requests the catalog (models, effort levels); answered with a `.catalog` event.
    public func initialize() throws {
        try control(["subtype": "initialize"])
    }

    /// Switches model for the following turns, without restarting the process.
    public func setModel(_ model: String?) throws {
        try control(["subtype": "set_model", "model": model ?? "default"])
    }

    /// Takes effect immediately. Pass the CLI value ("default", "acceptEdits", ...).
    public func setPermissionMode(_ mode: String) throws {
        try control(["subtype": "set_permission_mode", "mode": mode])
    }

    private func control(_ request: [String: Any]) throws {
        requestCounter += 1
        try write([
            "type": "control_request",
            "request_id": "yh-\(requestCounter)",
            "request": request,
        ])
    }

    public func respond(to request: PermissionRequest, allow: Bool, denyMessage: String = "The user denied this action.") throws {
        try respond(to: request, with: allow ? .allow() : .deny(message: denyMessage))
    }

    public func respond(to request: PermissionRequest, with decision: PermissionDecision) throws {
        try write(Self.response(to: request, with: decision))
    }

    static func response(to request: PermissionRequest, with decision: PermissionDecision) -> [String: Any] {
        let body: [String: Any]
        switch decision {
        case .allow(let updatedInput, let mode):
            let json = updatedInput ?? request.inputJSON
            let input = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) ?? [String: Any]()
            var allow: [String: Any] = ["behavior": "allow", "updatedInput": input]
            if let mode {
                allow["updatedPermissions"] = [["type": "setMode", "mode": mode, "destination": "session"]]
            }
            body = allow
        case .deny(let message):
            body = ["behavior": "deny", "message": message]
        }
        return [
            "type": "control_response",
            "response": ["subtype": "success", "request_id": request.requestID, "response": body],
        ]
    }

    public func terminate() {
        try? stdin.close()
        if process.isRunning { process.terminate() }
    }

    public var isRunning: Bool { process.isRunning }

    private func write(_ object: [String: Any]) throws {
        var data = try JSONSerialization.data(withJSONObject: object)
        data.append(0x0A)
        try stdin.write(contentsOf: data)
    }
}

/// Splits stdout into lines, parses them, and finishes the stream only once both stdout
/// has drained and the process has exited, so no trailing events are lost.
private final class OutputReader: Sendable {
    private struct State {
        var buffer = Data()
        var parser = ClaudeStreamParser()
        var stderrTail = Data()
        var stdoutOpen = true
        var exitCode: Int32?
    }

    private let state = Mutex(State())
    private let continuation: AsyncStream<AgentEvent>.Continuation
    private static let stderrLimit = 8 * 1024

    init(continuation: AsyncStream<AgentEvent>.Continuation) {
        self.continuation = continuation
    }

    func consume(_ chunk: Data) {
        let events = state.withLock { state -> [AgentEvent] in
            state.buffer.append(chunk)
            var events: [AgentEvent] = []
            while let newline = state.buffer.firstIndex(of: 0x0A) {
                let line = state.buffer[state.buffer.startIndex..<newline]
                if !line.isEmpty { events += state.parser.parse(line: Data(line)) }
                state.buffer.removeSubrange(state.buffer.startIndex...newline)
            }
            return events
        }
        for event in events { continuation.yield(event) }
    }

    func appendStderr(_ chunk: Data) {
        state.withLock { state in
            state.stderrTail.append(chunk)
            if state.stderrTail.count > Self.stderrLimit {
                state.stderrTail = state.stderrTail.suffix(Self.stderrLimit)
            }
        }
    }

    func stdoutClosed() {
        state.withLock { $0.stdoutOpen = false }
        finishIfDone()
    }

    func processExited(code: Int32) {
        state.withLock { $0.exitCode = code }
        finishIfDone()
    }

    private func finishIfDone() {
        let exit: (Int32, String)? = state.withLock { state in
            guard !state.stdoutOpen, let code = state.exitCode else { return nil }
            return (code, String(decoding: state.stderrTail, as: UTF8.self))
        }
        guard let (code, stderr) = exit else { return }
        continuation.yield(.processExited(code: code, stderrTail: stderr))
        continuation.finish()
    }
}
