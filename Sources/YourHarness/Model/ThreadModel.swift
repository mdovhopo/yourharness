import Foundation
import SwiftUI
import HarnessCore

/// A conversation with one agent session, plus the reducer that turns `AgentEvent`s
/// into transcript entries.
@MainActor @Observable
final class ThreadModel: Identifiable {
    enum Phase {
        case idle, running
    }

    static let untitled = ThreadTitlePrompt.fallback

    let id: UUID
    let projectID: UUID
    let createdAt: Date
    let isEphemeral: Bool
    var title: String
    private(set) var updatedAt: Date
    private(set) var sessionID: String?
    /// CLI model alias (`opus`, `sonnet`, ...); nil follows the app default.
    private(set) var model: String?
    /// Reasoning effort; nil follows the app default.
    private(set) var effort: String?
    /// Resolved model name reported by the running session.
    private(set) var activeModelName: String?
    /// Tokens currently in context, from the latest assistant message.
    private(set) var contextTokens: Int?
    private(set) var contextWindow: Int?
    private(set) var entries: [TranscriptEntry] = []
    private(set) var phase = Phase.idle
    private(set) var pendingPermissions: [PermissionRequest] = []
    /// A reply finished while you were looking at another thread.
    private(set) var hasUnread = false
    /// Pinned threads sit at the top of their project.
    var isPinned = false
    /// Settled threads move to their project's Settled shelf; history stays.
    var settledAt: Date?
    var autoSettle = true
    var reactivatedAt: Date?

    var isSettled: Bool { settledAt != nil }
    /// Where the thread sorts among active ones: its last activity, or when it was brought back.
    var sortDate: Date { max(updatedAt, reactivatedAt ?? .distantPast) }
    var draft = ""
    /// A generated title is on its way; the current one is a placeholder.
    private(set) var isGeneratingTitle = false

    @ObservationIgnored private unowned let app: AppModel
    @ObservationIgnored private var sessionStart: Task<ClaudeSession, Error>?
    /// Bumped whenever we drop a session on purpose, so its exit event is ignored.
    @ObservationIgnored private var sessionGeneration = 0
    @ObservationIgnored private var receivedInit = false
    @ObservationIgnored private var interruptRequested = false
    @ObservationIgnored private var restartWhenIdle = false
    @ObservationIgnored private var transcriptLoaded: Bool
    @ObservationIgnored private var blocks: [BlockRef: TranscriptEntry] = [:]
    @ObservationIgnored private var tools: [String: TranscriptEntry] = [:]
    @ObservationIgnored private var streamedMessageIDs: Set<String> = []
    @ObservationIgnored private var idleReaper: Task<Void, Never>?
    /// The title request whose answer still counts; renaming or a newer request voids it.
    @ObservationIgnored private var titleRequest: UUID?
    /// The first message didn't say what this is about; retitle once a turn has.
    @ObservationIgnored private var titleNeedsRefinement = false

    /// Idle `claude` processes cost hundreds of MB each; drop them and `--resume` later.
    private static let idleTimeout = Duration.seconds(10 * 60)

    init(projectID: UUID, app: AppModel, isEphemeral: Bool = false) {
        id = UUID()
        self.projectID = projectID
        self.app = app
        self.isEphemeral = isEphemeral
        title = Self.untitled
        createdAt = .now
        updatedAt = .now
        transcriptLoaded = true
    }

    init(meta: ThreadMeta, app: AppModel) {
        id = meta.id
        projectID = meta.projectID
        self.app = app
        isEphemeral = false
        title = meta.title
        createdAt = meta.createdAt
        updatedAt = meta.updatedAt
        sessionID = meta.sessionID
        model = meta.model
        effort = meta.effort
        contextTokens = meta.contextTokens
        contextWindow = meta.contextWindow
        isPinned = meta.isPinned ?? false
        settledAt = meta.settledAt
        autoSettle = meta.autoSettle ?? true
        reactivatedAt = meta.reactivatedAt
        transcriptLoaded = false
    }

    var meta: ThreadMeta {
        ThreadMeta(id: id, projectID: projectID, title: title, createdAt: createdAt,
                   updatedAt: updatedAt, sessionID: sessionID, model: model, effort: effort,
                   contextTokens: contextTokens, contextWindow: contextWindow,
                   isPinned: isPinned ? true : nil, settledAt: settledAt,
                   autoSettle: autoSettle ? nil : false, reactivatedAt: reactivatedAt)
    }

    var hasLiveSession: Bool { sessionStart != nil }

    var effectiveModel: String? { model ?? app.defaults.model }
    var effectiveEffort: String? { effort ?? app.defaults.effort }

    /// Context window for the running model, falling back to what we've seen before.
    var effectiveContextWindow: Int? {
        contextWindow
            ?? app.contextWindow(forModel: activeModelName)
            ?? app.contextWindow(forModel: app.claudeModel(for: effectiveModel)?.resolvedModel)
    }

    func loadTranscriptIfNeeded() {
        guard !transcriptLoaded else { return }
        transcriptLoaded = true
        let loaded = app.store.loadTranscript(id)
            .filter { !($0.kind == .thinking && $0.text.isEmpty) }
            .map(TranscriptEntry.init(snapshot:))
        // History should just be there; only new messages animate in.
        var transaction = Transaction()
        transaction.disablesAnimations = true
        withTransaction(transaction) { entries = loaded }
    }

    func markRead() {
        hasUnread = false
    }

    // MARK: - Actions

    func send(_ text: String) {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        loadTranscriptIfNeeded()
        // Picking a settled thread back up brings it back to the list, and new activity
        // lifts the hold an un-settle puts on automatic settling.
        if isSettled { app.setSettled(false, self) }
        reactivatedAt = nil
        let isFirstMessage = title == Self.untitled && !entries.contains { $0.kind == .user }
        if isFirstMessage { title = ThreadTitlePrompt.seed(from: text) }
        entries.append(TranscriptEntry(kind: .user, text: text))
        if isFirstMessage { app.generateTitle(for: self, from: .firstMessage(text)) }
        phase = .running
        idleReaper?.cancel()
        touch()

        Task {
            do {
                let session = try await ensureSession()
                try await session.send(text: text)
            } catch {
                fail(error.localizedDescription)
            }
        }
    }

    func stop() {
        guard phase == .running, let start = sessionStart else { return }
        interruptRequested = true
        pendingPermissions.removeAll()
        Task {
            do {
                try await start.value.interrupt()
            } catch {
                fail(error.localizedDescription)
            }
        }
    }

    func respond(to request: PermissionRequest, allow: Bool) {
        pendingPermissions.removeAll { $0.id == request.id }
        guard let start = sessionStart else { return }
        Task {
            do {
                try await start.value.respond(to: request, allow: allow)
            } catch {
                fail(error.localizedDescription)
            }
        }
    }

    func setModel(_ model: String?) {
        guard model != self.model else { return }
        self.model = model
        app.save()
        applyModel()
    }

    /// Pushes the effective model into a live session; the next turn uses it.
    func applyModel() {
        contextWindow = nil
        guard let start = sessionStart else { return }
        let model = effectiveModel
        Task { try? await start.value.setModel(model) }
    }

    func setEffort(_ effort: String?) {
        guard effort != self.effort else { return }
        self.effort = effort
        app.save()
        // Effort is launch-only, so resume the conversation in a fresh process.
        restartSession()
    }

    func applyPermissionMode(_ mode: PermissionMode) {
        guard let start = sessionStart else { return }
        Task { try? await start.value.setPermissionMode(mode.controlValue) }
    }

    /// Restart the process (keeping the conversation) so new launch options take effect.
    func restartSession() {
        if phase == .idle { endSession() } else { restartWhenIdle = true }
    }

    func endSession() {
        guard let start = sessionStart else { return }
        sessionStart = nil
        sessionGeneration += 1
        idleReaper?.cancel()
        Task {
            if let session = try? await start.value { await session.terminate() }
        }
    }

    // MARK: - Session

    private func ensureSession() async throws -> ClaudeSession {
        if let start = sessionStart { return try await start.value }
        sessionGeneration += 1
        receivedInit = false
        let generation = sessionGeneration
        let start = Task { () throws -> ClaudeSession in
            let session = try ClaudeSession(options: try await app.launchOptions(for: self))
            listen(to: session, generation: generation)
            // Keeps the model catalog fresh; costs nothing.
            try? await session.initialize()
            return session
        }
        sessionStart = start
        do {
            return try await start.value
        } catch {
            if sessionGeneration == generation { sessionStart = nil }
            throw error
        }
    }

    private func listen(to session: ClaudeSession, generation: Int) {
        Task { [weak self] in
            for await event in session.events {
                guard let self else { break }
                self.apply(event, generation: generation)
            }
        }
    }

    // MARK: - Reducer

    private func apply(_ event: AgentEvent, generation: Int) {
        // Everything but the exit event is harmless from a superseded session.
        switch event {
        case .sessionStarted(let info):
            receivedInit = true
            sessionID = info.sessionID
            activeModelName = info.model
            app.save()

        case .status(let status):
            if status == "requesting" { phase = .running }

        case .blockStarted(let ref, let kind):
            // Subagent internals (non-nil parent) get a nested view later; skip for now.
            guard ref.parentToolUseID == nil else { return }
            phase = .running
            streamedMessageIDs.insert(ref.messageID)
            let entry: TranscriptEntry
            switch kind {
            case .text:
                entry = TranscriptEntry(kind: .assistant, isStreaming: true)
            case .thinking:
                // Often redacted (signature only, no text). Only show it once text arrives.
                blocks[ref] = TranscriptEntry(kind: .thinking, isStreaming: true)
                return
            case .toolUse(let id, let name):
                entry = TranscriptEntry(id: id, kind: .tool, isStreaming: true)
                entry.toolName = name
                tools[id] = entry
            }
            blocks[ref] = entry
            entries.append(entry)

        case .textDelta(let ref, let text):
            blocks[ref]?.text += text

        case .thinkingDelta(let ref, let text):
            guard let entry = blocks[ref], !text.isEmpty else { return }
            if entry.text.isEmpty { entries.append(entry) }
            entry.text += text

        case .toolInputDelta(let ref, let json):
            blocks[ref]?.toolInput += json

        case .assistantMessage(let messageID, let content, let parent):
            guard parent == nil else { return }
            let streamed = streamedMessageIDs.contains(messageID)
            for block in content {
                switch block {
                case .toolUse(let id, let name, let input):
                    if let entry = tools[id] {
                        entry.toolInput = input
                    } else {
                        let entry = TranscriptEntry(id: id, kind: .tool, isStreaming: true)
                        entry.toolName = name
                        entry.toolInput = input
                        tools[id] = entry
                        entries.append(entry)
                    }
                case .text(let text) where !streamed:
                    entries.append(TranscriptEntry(kind: .assistant, text: text))
                case .thinking(let text) where !streamed && !text.isEmpty:
                    entries.append(TranscriptEntry(kind: .thinking, text: text))
                case .text, .thinking:
                    break
                }
            }
            for (ref, entry) in blocks where ref.messageID == messageID && entry.kind != .tool {
                entry.isStreaming = false
            }

        case .toolResults(let results, let parent):
            guard parent == nil else { return }
            for result in results {
                guard let entry = tools[result.toolUseID] else { continue }
                entry.toolResult = result.content
                entry.toolIsError = result.isError
                entry.isStreaming = false
            }

        case .permissionRequest(let request):
            pendingPermissions.append(request)

        case .rateLimit(let info):
            app.updateRateLimit(info)

        case .contextUsage(let tokens):
            contextTokens = tokens

        case .catalog(let catalog):
            app.updateCatalog(catalog)

        case .controlFailed:
            break

        case .turnCompleted(let result):
            app.rememberContextWindows(result.contextWindows)
            if let model = activeModelName, let window = result.contextWindows[model] {
                contextWindow = window
            } else if let window = result.contextWindows.values.max() {
                contextWindow = window
            }
            if result.isError {
                if interruptRequested {
                    entries.append(.notice("Interrupted"))
                } else {
                    let message = result.resultText ?? "Claude stopped with an error (\(result.subtype))."
                    entries.append(.notice(message, style: .error))
                }
            }
            interruptRequested = false
            settle()
            app.turnFinished(in: projectID)
            if titleNeedsRefinement, !result.isError {
                titleNeedsRefinement = false
                app.generateTitle(for: self, from: .conversation)
            }
            scheduleIdleReap()
            if restartWhenIdle {
                restartWhenIdle = false
                endSession()
            }

        case .processExited(let code, let stderr):
            guard generation == sessionGeneration else { return }
            sessionStart = nil
            let wasRunning = phase == .running
            settle()
            if !receivedInit, sessionID != nil, code != 0 {
                sessionID = nil
                entries.append(.notice("Couldn't resume the previous session. Your next message starts a new one.", style: .error))
            } else if wasRunning || code != 0 {
                let detail = stderr.split(separator: "\n").suffix(3).joined(separator: "\n")
                entries.append(.notice("Claude exited (code \(code)). \(detail)", style: .error))
            }
        }
    }

    /// Marks the turn finished: nothing is streaming, nothing is awaiting approval.
    private func settle() {
        if phase == .running, app.visibleThreadID != id { hasUnread = true }
        phase = .idle
        pendingPermissions.removeAll()
        for entry in blocks.values where entry.isStreaming { entry.isStreaming = false }
        for entry in tools.values where entry.isStreaming { entry.isStreaming = false }
        blocks.removeAll()
        streamedMessageIDs.removeAll()
        touch()
        persist()
    }

    private func fail(_ message: String) {
        phase = .idle
        entries.append(.notice(message, style: .error))
        persist()
    }

    private func scheduleIdleReap() {
        idleReaper?.cancel()
        idleReaper = Task { [weak self] in
            try? await Task.sleep(for: Self.idleTimeout)
            guard !Task.isCancelled, let self, self.phase == .idle else { return }
            self.endSession()
        }
    }

    private func touch() {
        updatedAt = .now
    }

    private func persist() {
        guard !isEphemeral else { return }
        app.store.saveTranscript(id, entries: entries.map(\.snapshot))
        app.save()
    }

    // MARK: - Title

    func rename(_ newTitle: String) {
        let newTitle = newTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !newTitle.isEmpty else { return }
        // Your name wins over anything still being generated.
        titleRequest = nil
        titleNeedsRefinement = false
        isGeneratingTitle = false
        guard newTitle != title else { return }
        title = newTitle
        app.save()
    }

    /// User and assistant messages, oldest first; what a regenerated title reads.
    var titleMessages: [ThreadTitlePrompt.Message] {
        loadTranscriptIfNeeded()
        return entries.compactMap { entry in
            switch entry.kind {
            case .user: ThreadTitlePrompt.Message(role: .user, text: entry.text)
            case .assistant where !entry.isStreaming: ThreadTitlePrompt.Message(role: .assistant, text: entry.text)
            default: nil
            }
        }
    }

    func beginTitleGeneration() -> UUID {
        let request = UUID()
        titleRequest = request
        isGeneratingTitle = true
        return request
    }

    /// Applies a generated title unless you renamed the thread or asked again meanwhile.
    func finishTitleGeneration(_ request: UUID, with generated: GeneratedTitle?, regenerating: Bool) {
        guard titleRequest == request else { return }
        titleRequest = nil
        isGeneratingTitle = false
        guard let generated, generated.title != ThreadTitlePrompt.fallback, generated.title != title else { return }
        title = generated.title
        if !regenerating, generated.needsRefinement { titleNeedsRefinement = true }
        app.save()
    }

    /// History brought in from elsewhere (a resumed Claude Code session), shown as is.
    func importTranscript(_ imported: [TranscriptEntry]) {
        transcriptLoaded = true
        seed(imported)
        persist()
    }

    // MARK: - Debug

    /// Seeds a transcript directly; used by the stress test.
    func seed(_ seeded: [TranscriptEntry]) {
        var transaction = Transaction()
        transaction.disablesAnimations = true
        withTransaction(transaction) { entries = seeded }
    }

    func appendForStress(_ entry: TranscriptEntry) {
        entries.append(entry)
    }
}
