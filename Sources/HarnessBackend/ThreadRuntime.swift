import Foundation
import HarnessCore

/// One thread's `claude`: started on the first message (resuming the thread's session),
/// kept between turns, let go of after ten idle minutes and resumed on the next message.
/// Its events go through the backend's `ThreadStore`; changed rows are saved a moment later.
@MainActor
final class ThreadRuntime {
    let id: String
    private unowned let backend: Backend
    private var session: ClaudeSession?
    private var pump: Task<Void, Never>?
    private var generation = 0
    private(set) var running = false
    private var idleReap: Task<Void, Never>?
    private var saveScheduled = false
    /// Effort is a launch option: a change applies from the next start.
    private var startedEffort: String?

    static let idleLimit: Duration = .seconds(600)

    init(id: String, backend: Backend) {
        self.id = id
        self.backend = backend
    }

    var hasSession: Bool { session != nil }

    func send(_ text: String, now: Bool) async throws {
        idleReap?.cancel()
        backend.emit(backend.store.userMessage(text, thread: id), thread: id)
        setRunning(true)
        do {
            let session = try await ensureSession()
            try await session.send(text: text, id: UUID().uuidString, priority: now ? .now : nil)
        } catch {
            backend.emit(backend.store.notice("Couldn't send: \(error)", thread: id), thread: id)
            setRunning(false)
            throw error
        }
    }

    func interrupt() async { try? await session?.interrupt() }

    func setPermissionMode(_ mode: String) async { try? await session?.setPermissionMode(mode) }

    func setModel(_ model: String?) async { try? await session?.setModel(model) }

    func respond(_ request: PermissionRequest, _ decision: PermissionDecision) async {
        backend.emit(backend.store.answered(id), thread: id)
        backend.threadChanged(id)
        try? await session?.respond(to: request, with: decision)
    }

    /// Ends the process; the next message resumes the session.
    func end() {
        generation += 1
        pump?.cancel()
        if let session { Task { await session.terminate() } }
        session = nil
        if running {
            backend.emit(backend.store.apply(.processExited(code: 0, stderrTail: ""), thread: id), thread: id)
            setRunning(false)
        }
    }

    private func ensureSession() async throws -> ClaudeSession {
        let effort = backend.effort(of: id)
        if let session, startedEffort == effort { return session }
        if session != nil { end() }
        let (environment, executable) = try await backend.cli()
        let meta = backend.library.thread(id) ?? [:]
        let projectID = meta["projectID"] as? String ?? ""
        let folder = (meta["workingDirectory"] as? String).flatMap { FileManager.default.fileExists(atPath: $0) ? $0 : nil }
            ?? backend.library.folder(ofProject: projectID, dataDirectory: backend.dataDirectory)
        let mode = PermissionModes.cli(backend.library.project(projectID)?["permissionMode"] as? String)
        let options = ClaudeLaunchOptions(executable: executable, workingDirectory: URL(fileURLWithPath: folder, isDirectory: true),
                                          environment: environment, model: backend.model(of: id), permissionMode: mode,
                                          effort: effort, resumeSessionID: meta["sessionID"] as? String)
        let session = try ClaudeSession(options: options)
        try? await session.initialize()
        self.session = session
        startedEffort = effort
        generation += 1
        let generation = self.generation
        pump = Task { [weak self] in
            for await event in session.events {
                guard let self, self.generation == generation else { return }
                self.receive(event)
            }
            guard let self, self.generation == generation else { return }
            self.session = nil
        }
        return session
    }

    private func receive(_ event: AgentEvent) {
        switch event {
        case .sessionStarted(let info):
            backend.library.updateThread(id) { $0["sessionID"] = info.sessionID }
        case .catalog(let catalog):
            backend.noteCatalog(catalog)
        case .turnCompleted, .processExited:
            backend.emit(backend.store.apply(event, thread: id), thread: id)
            setRunning(false)
            scheduleIdleReap()
            return
        default:
            break
        }
        let patches = backend.store.apply(event, thread: id)
        guard !patches.isEmpty else { return }
        backend.emit(patches, thread: id)
        if case .permissionRequest = event { backend.threadChanged(id) }
        scheduleSave()
    }

    private func setRunning(_ value: Bool) {
        guard running != value else { return }
        running = value
        backend.library.updateThread(id) {
            $0["turnInProgress"] = value ? true : nil
            if value { $0["updatedAt"] = backend.library.now() }
        }
        backend.threadChanged(id)
        scheduleSave()
    }

    private func scheduleIdleReap() {
        idleReap?.cancel()
        idleReap = Task { [weak self] in
            try? await Task.sleep(for: Self.idleLimit)
            guard !Task.isCancelled, let self, !self.running, self.backend.store.pendingPermission(self.id) == nil else { return }
            self.end()
        }
    }

    // MARK: Saving

    private func scheduleSave() {
        guard !saveScheduled else { return }
        saveScheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
            MainActor.assumeIsolated { self?.saveNow() }
        }
    }

    /// Writes the rows that changed; the database does it off the main thread.
    func saveNow() {
        saveScheduled = false
        guard let uuid = UUID(uuidString: id), let total = backend.store.total(id) else { return }
        let changes = backend.store.takeChanges(id)
        guard !changes.isEmpty else { return }
        var snapshots: [Int: EntrySnapshot] = [:]
        for change in changes { snapshots[change.seq] = change.entry.snapshot(fullResult: change.fullResult) }
        backend.library.db.saveTranscriptChanges(uuid, count: total, changes: snapshots)
    }
}

/// The app's permission modes, as saved with a project, and what the CLI calls them.
enum PermissionModes {
    static let all = ["auto", "ask", "acceptEdits", "plan", "bypass"]

    static func cli(_ mode: String?) -> String? {
        switch mode ?? "auto" {
        case "ask": nil
        case "bypass": "bypassPermissions"
        case let other: other
        }
    }

    static func control(_ mode: String) -> String { cli(mode) ?? "default" }
}
