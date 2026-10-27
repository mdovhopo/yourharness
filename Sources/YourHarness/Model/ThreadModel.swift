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
    private(set) var entries: [TranscriptEntry] = [] {
        didSet { rowsChanged() }
    }
    private(set) var phase = Phase.idle {
        didSet { if phase != oldValue { rowsChanged() } }
    }
    private(set) var pendingPermissions: [PermissionRequest] = [] {
        didSet {
            guard pendingPermissions.isEmpty != oldValue.isEmpty else { return }
            app.waitingChanged()
            rowsChanged()
        }
    }
    /// Sub-agents still at work. Background ones outlive the turn that started them.
    private(set) var liveSubagents: [SubagentRun] = []
    /// The sub-agent the inspector follows; nil follows the latest.
    var inspectedSubagentID: String?
    /// A reply finished while you were looking at another thread.
    private(set) var hasUnread = false
    /// Finished turns you've opened to see Claude's work, by their first message's ID.
    var expandedTurns: Set<String> = [] {
        didSet { rowsChanged() }
    }
    /// How many of the latest turns the transcript shows; scrolling to the top adds more.
    var shownTurns = ThreadModel.turnsPerPage {
        didSet { if shownTurns != oldValue { rowsChanged() } }
    }
    /// Bumped whenever the rows the transcript shows may have changed. They're worked out
    /// again on the next read, once, however many times the view asks.
    private(set) var rowsRevision = 0
    @ObservationIgnored var transcriptRowsCache: (revision: Int, rows: [TranscriptEntry], turnStarts: [Int])?
    @ObservationIgnored var displayRowsCache: (revision: Int, rows: [TranscriptRow])?

    /// For changes the rows depend on that aren't to the list itself: a turn's end, a result.
    func rowsChanged() {
        rowsRevision &+= 1
    }
    static let turnsPerPage = ProcessInfo.processInfo.environment["YH_TURNS_PER_PAGE"].flatMap(Int.init) ?? 40

    func showEarlierTurns() {
        shownTurns += Self.turnsPerPage
    }

    /// Every turn, for going to the very top.
    func showAllTurns() {
        shownTurns = max(shownTurns, transcriptTurnStarts.count)
    }
    /// Where you were when you last left the thread: the row at the bottom of the screen;
    /// nil when you were at the end, which is where it opens then.
    var scrollAnchor: String?
    /// Asked for from the menu or the transcript's buttons; the transcript scrolls.
    private(set) var messageJump: MessageJump?
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
    /// Images waiting to go with the next message.
    var draftImages: [ImageAttachment] = []
    /// A generated title is on its way; the current one is a placeholder.
    private(set) var isGeneratingTitle = false
    /// What the agent is doing when it isn't writing, e.g. compacting the conversation.
    private(set) var activity: String?
    /// Sent while Claude works, waiting for its current step to finish. Each joins the
    /// transcript where Claude picks it up.
    private(set) var queuedMessages: [QueuedMessage] = []

    struct QueuedMessage: Identifiable {
        let id: String
        let text: String
        let images: [ImageAttachment]
        let attachments: [String]
        /// You asked for it to go now (Claude is being stopped), or to take it back.
        var pending: Pending?

        enum Pending { case sendingNow, withdrawing }
    }
    /// ⌘F in this thread.
    let find = ThreadFind()
    /// Claude's checklist from its task tools, as it stands now.
    private(set) var plan = TaskPlan()
    /// The plan as of each run of task tool calls, keyed by the run's last call.
    private(set) var planSteps: [String: TaskPlan] = [:]
    /// Task tool calls followed by another: their run shows once, at its end.
    private(set) var foldedPlanCalls: Set<String> = [] {
        didSet { rowsChanged() }
    }
    /// The newest run, shown as the full checklist.
    private(set) var latestPlanStep: String?
    /// `/btw`: questions on the side, in a copy of this conversation. Shown in the right
    /// sidebar; nothing asked there reaches this thread.
    private(set) var sideChat: ThreadModel?
    /// A thread's `/btw` side chat: not saved, read-only tools, never reaped.
    let isSideChat: Bool
    /// The memory importer or reconciler, in its own window: not a thread of the project.
    let memoryRole: MemoryRole?
    /// For a memory specialist: folders it may read besides the project (what it imports).
    var extraDirectories: [String] = []
    /// Tasks in memory this thread has worked on, for its toolbar; loaded by the thread view.
    var tasks: [HarnessDatabase.Memory] = []
    /// For the importer: sources picked elsewhere (a file that wasn't memory), waiting in its window.
    var pendingImportSources: [String] = []
    /// A side chat or a memory specialist: it works beside the project's threads, not in them.
    var isAside: Bool { isSideChat || memoryRole != nil }
    /// For a side chat: the session it forks from when it starts. Nil starts fresh.
    let forksSession: String?
    /// For a side chat: the thread it's on the side of. It can look at its browser.
    @ObservationIgnored private(set) weak var parentThread: ThreadModel?
    /// The thread's browser, made when it's first used.
    private(set) var browser: BrowserSession?
    /// Folders its tool calls touched, newest last: the repos and worktrees it works in.
    private(set) var footprint: [String] = []
    /// False for a thread saved before footprints were kept, until it's worked out.
    @ObservationIgnored private var footprintKnown = true
    /// PR and MR links that came up in the thread, oldest first.
    private(set) var pullRequestLinks: [String] = []
    @ObservationIgnored private var linksKnown = true
    /// Folders it changed something in: a branch there is its own work, and so is the branch's PR.
    private(set) var writtenFootprint: [String] = []
    /// PRs it opened, merged, edited, reviewed or commented on; the rest were only mentioned.
    private(set) var workedPullRequestLinks: [String] = []
    /// False for a thread saved before work was told from mentions, until it's worked out.
    @ObservationIgnored private var workKnown = true
    /// Where `claude` runs, when it isn't the project folder: a worktree, say.
    private(set) var workingDirectory: String?
    /// Ports that processes this thread started are listening on: its dev servers.
    private(set) var listeningPorts: [Int] = []
    @ObservationIgnored private var serverScan: Task<Void, Never>?
    /// Tabs open at the last quit, until the browser is made.
    @ObservationIgnored private var savedBrowserTabs: [String] = []

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
    /// By Agent tool call ID. Unlike `tools`, kept across turns: background sub-agents
    /// report after the turn that started them has ended.
    @ObservationIgnored private var subagents: [String: SubagentRun] = [:]
    @ObservationIgnored private var idleReaper: Task<Void, Never>?
    /// The title request whose answer still counts; renaming or a newer request voids it.
    @ObservationIgnored private var titleRequest: UUID?
    /// The first message didn't say what this is about; retitle once a turn has.
    @ObservationIgnored private var titleNeedsRefinement = false
    /// You sent `/rename`; the CLI's title change that follows is yours to apply.
    @ObservationIgnored private var renameRequested = false
    /// The turn is being stopped for a message of yours, which says so itself; the
    /// stopped turn gets no "Interrupted" notice.
    @ObservationIgnored private var interruptingForMessage = false
    /// Messages to restore into the composer once their withdrawal is confirmed.
    @ObservationIgnored private var restoreOnWithdraw: Set<String> = []
    /// The app closed while Claude was working on this, without saving that it stopped.
    @ObservationIgnored private var wasCutOff = false
    /// The running `claude` process, so quitting can stop it on the spot.
    @ObservationIgnored private var processID: Int32?
    /// A save during a turn, a moment after the latest change.
    @ObservationIgnored private var pendingSave: Task<Void, Never>?

    /// Idle `claude` processes cost hundreds of MB each; drop them and `--resume` later.
    private static let idleTimeout = Duration.seconds(10 * 60)

    init(projectID: UUID, app: AppModel, isEphemeral: Bool = false) {
        id = UUID()
        self.projectID = projectID
        self.app = app
        self.isEphemeral = isEphemeral
        isSideChat = false
        memoryRole = nil
        forksSession = nil
        title = Self.untitled
        createdAt = .now
        updatedAt = .now
        transcriptLoaded = true
    }

    /// A side chat for `parent`, starting from its conversation as it is now.
    init(sideChatOf parent: ThreadModel, app: AppModel) {
        id = UUID()
        projectID = parent.projectID
        self.app = app
        isEphemeral = true
        isSideChat = true
        memoryRole = nil
        forksSession = parent.sessionID
        parentThread = parent
        // Its session lives with the folder the thread's runs in.
        workingDirectory = parent.workingDirectory
        title = "Side Chat"
        model = parent.model
        effort = parent.effort
        createdAt = .now
        updatedAt = .now
        transcriptLoaded = true
    }

    /// The memory importer or reconciler for `projectID`'s memory.
    init(memoryRole role: MemoryRole, projectID: UUID, app: AppModel) {
        id = UUID()
        self.projectID = projectID
        self.app = app
        isEphemeral = true
        isSideChat = false
        memoryRole = role
        forksSession = nil
        title = role.title
        createdAt = .now
        updatedAt = .now
        transcriptLoaded = true
    }

    init(meta: ThreadMeta, app: AppModel) {
        isSideChat = false
        memoryRole = nil
        forksSession = nil
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
        wasCutOff = meta.turnInProgress == true
        draft = meta.draft ?? ""
        savedBrowserTabs = meta.browserTabs ?? []
        footprint = meta.footprint ?? []
        footprintKnown = meta.footprint != nil
        pullRequestLinks = meta.pullRequests ?? []
        linksKnown = meta.pullRequests != nil
        writtenFootprint = meta.writtenFootprint ?? []
        workedPullRequestLinks = meta.workedPullRequests ?? []
        workKnown = meta.writtenFootprint != nil && meta.workedPullRequests != nil
        workingDirectory = meta.workingDirectory
        transcriptLoaded = false
    }

    var meta: ThreadMeta {
        ThreadMeta(id: id, projectID: projectID, title: title, createdAt: createdAt,
                   updatedAt: updatedAt, sessionID: sessionID, model: model, effort: effort,
                   contextTokens: contextTokens, contextWindow: contextWindow,
                   isPinned: isPinned ? true : nil, settledAt: settledAt,
                   autoSettle: autoSettle ? nil : false, reactivatedAt: reactivatedAt,
                   turnInProgress: phase == .running || wasCutOff ? true : nil,
                   draft: draft.isEmpty ? nil : draft,
                   browserTabs: Self.nilIfEmpty(browser?.savedURLs ?? savedBrowserTabs),
                   footprint: footprintKnown ? footprint : nil,
                   workingDirectory: workingDirectory,
                   pullRequests: linksKnown ? pullRequestLinks : nil,
                   writtenFootprint: workKnown ? writtenFootprint : nil,
                   workedPullRequests: workKnown ? workedPullRequestLinks : nil)
    }

    private static func nilIfEmpty(_ urls: [String]) -> [String]? {
        urls.isEmpty ? nil : urls
    }

    var hasLiveSession: Bool { sessionStart != nil }

    /// A turn is running, or a sub-agent is still at work after one.
    var isWorking: Bool { phase == .running || !liveSubagents.isEmpty }

    /// Every sub-agent in the transcript, oldest first.
    var subagentRuns: [SubagentRun] { entries.compactMap(\.subagent) }

    var effectiveModel: String? { model ?? app.defaults.model }
    var effectiveEffort: String? { effort ?? app.defaults.effort }

    /// Context window for the running model, falling back to what we've seen before.
    var effectiveContextWindow: Int? {
        contextWindow
            ?? app.contextWindow(forModel: activeModelName)
            ?? app.contextWindow(forModel: app.claudeModel(for: effectiveModel)?.resolvedModel)
    }

    /// Right now, on this thread: for sending, steering and the like, which need it.
    func loadTranscriptIfNeeded() {
        guard !transcriptLoaded else { return }
        transcriptLoad?.cancel()
        transcriptLoad = nil
        apply(app.store.loadTranscript(id))
    }

    /// Read and decoded off the main thread, for showing the thread: a big transcript is
    /// megabytes of JSON, and switching to it shouldn't wait for that.
    func loadTranscriptSoon() async {
        guard !transcriptLoaded else { return }
        if transcriptLoad == nil {
            let store = app.store, id = id
            transcriptLoad = Task { [weak self] in
                let loaded = await Task.detached(priority: .userInitiated) { store.loadTranscript(id) }.value
                guard let self, !Task.isCancelled, !self.transcriptLoaded else { return }
                self.apply(loaded)
            }
        }
        await transcriptLoad?.value
    }

    @ObservationIgnored private var transcriptLoad: Task<Void, Never>?

    /// The transcript for another thread's agent to read: what's in memory when it's loaded
    /// (newer than what's saved while Claude works), else read from the database without
    /// loading it here.
    func transcriptForReading() async -> [EntrySnapshot] {
        if transcriptLoaded { return entries.map(\.snapshot) }
        let store = app.store, id = id
        return await Task.detached(priority: .userInitiated) { store.loadTranscript(id) }.value
    }

    private func apply(_ snapshots: [EntrySnapshot]) {
        transcriptLoaded = true
        let loaded = snapshots
            .filter { !($0.kind == .thinking && $0.text.isEmpty) }
            .map(TranscriptEntry.init(snapshot:))
        // History should just be there; only new messages animate in.
        var transaction = Transaction()
        transaction.disablesAnimations = true
        withTransaction(transaction) { entries = loaded }
        rebuildPlan()
        recoverFromSessionHistory()
    }

    /// What ⌘F looks through: each row's words, and for a tool call its input (the
    /// command, the file), which is usually what you're after.
    var findableText: [(id: String, text: String)] {
        transcriptRows.compactMap { entry in
            switch entry.kind {
            case .user, .assistant, .notice, .commandOutput: (entry.id, entry.text)
            case .tool: (entry.id, entry.toolName + " " + entry.toolInput)
            case .thinking: nil
            }
        }
    }

    /// What the transcript shows: each run of task tool calls as one plan row.
    var transcriptRows: [TranscriptEntry] {
        cachedTranscriptRows.rows
    }

    /// Where each turn starts in `transcriptRows`.
    var transcriptTurnStarts: [Int] {
        cachedTranscriptRows.turnStarts
    }

    private var cachedTranscriptRows: (revision: Int, rows: [TranscriptEntry], turnStarts: [Int]) {
        let revision = rowsRevision
        if let cached = transcriptRowsCache, cached.revision == revision { return cached }
        let rows = foldedPlanCalls.isEmpty ? entries : entries.filter { !foldedPlanCalls.contains($0.id) }
        let cached = (revision, rows, Self.turnStarts(in: rows))
        transcriptRowsCache = cached
        return cached
    }

    /// Replays the task tool calls. Only when one arrives or gets its result, so a
    /// streaming reply never pays for it.
    private func rebuildPlan() {
        var plan = TaskPlan()
        var steps: [String: TaskPlan] = [:]
        var folded: Set<String> = []
        var previous: String?
        for entry in entries {
            guard entry.kind == .tool, TaskPlan.isPlanTool(name: entry.toolName, inputJSON: entry.toolInput) else {
                previous = nil
                continue
            }
            plan.apply(toolName: entry.toolName, inputJSON: entry.toolInput, result: entry.toolResult, toolUseID: entry.id)
            if let previous {
                folded.insert(previous)
                steps[previous] = nil
            }
            steps[entry.id] = plan
            previous = entry.id
        }
        let latest = entries.last { steps[$0.id] != nil }?.id
        if plan != self.plan { self.plan = plan }
        if steps != planSteps { planSteps = steps }
        if folded != foldedPlanCalls { foldedPlanCalls = folded }
        if latest != latestPlanStep { latestPlanStep = latest }
    }

    /// Brings in what the session's own history has and the transcript doesn't: the end
    /// of a turn the app closed during, or what was said in `claude --resume` elsewhere.
    /// After a crash it always looks; otherwise only if the session file changed since.
    private func recoverFromSessionHistory() {
        guard !isEphemeral else { return }
        let cutOff = wasCutOff
        guard let sessionID else {
            if cutOff { finishRecovery(SessionBackfill(), cutOff: true) }
            return
        }
        let known = entries.map(\.archiveItem)
        let count = entries.count
        let savedAt = cutOff ? nil : app.store.transcriptSavedAt(id)
        Task {
            let backfill = await app.sessionBackfill(sessionID: sessionID, known: known, changedSince: savedAt)
            // You've carried on meanwhile: what's on screen is what counts.
            guard entries.count == count, phase == .idle, sessionStart == nil else { return }
            if backfill != nil || cutOff { finishRecovery(backfill ?? SessionBackfill(), cutOff: cutOff) }
        }
    }

    private func finishRecovery(_ backfill: SessionBackfill, cutOff: Bool) {
        wasCutOff = false
        var transaction = Transaction()
        transaction.disablesAnimations = true
        withTransaction(transaction) {
            for (index, item) in backfill.completed where entries.indices.contains(index) {
                entries[index].complete(with: item)
            }
            // Messages sent elsewhere; a tool result written as the app quit needs no word.
            if !cutOff, backfill.missing.contains(where: { if case .user = $0 { true } else { false } }) {
                entries.append(.notice("Continued outside YourHarness"))
            }
            entries.append(contentsOf: backfill.missing.map(TranscriptEntry.init(item:)))
            rowsChanged()
            rebuildPlan()
            if cutOff {
                entries.append(.notice(backfill.isEmpty
                    ? "YourHarness closed while Claude was working, so this turn stopped here."
                    : "YourHarness closed while Claude was working. What Claude did until then was recovered from its session history."))
            }
        }
        if cutOff || !backfill.isEmpty { persist() }
    }

    func markRead() {
        hasUnread = false
    }

    // MARK: - Actions

    /// While Claude works, a message steers by default: it waits for the current step and
    /// joins the turn. `interrupting` stops Claude so it goes in right away.
    func send(_ text: String, images: [ImageAttachment] = [], interrupting: Bool = false) {
        var text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty || !images.isEmpty else { return }
        noteLinks(in: text)
        if images.isEmpty, let command = SlashCommand.parse(text) {
            if isSideChat, command.name == "btw" {
                // Already on the side: the question is all that's left.
                text = command.arguments
                guard !text.isEmpty else { return }
            } else if handleLocally(command) {
                return
            }
            if command.name == "rename" { renameRequested = true }
        }
        loadTranscriptIfNeeded()
        // Picking a settled thread back up brings it back to the list, and new activity
        // lifts the hold an un-settle puts on automatic settling.
        if isSettled { app.setSettled(false, self) }
        reactivatedAt = nil
        let isFirstMessage = title == Self.untitled && !entries.contains { $0.kind == .user }
        let subject = text.isEmpty ? (images.count == 1 ? "Attached image" : "Attached images") : text
        if isFirstMessage { title = ThreadTitlePrompt.seed(from: subject) }
        let messageID = UUID().uuidString.lowercased()
        let attachments = isEphemeral ? [] : images.compactMap { app.store.saveAttachment($0, thread: id) }
        let working = phase == .running && sessionStart != nil
        if working, !interrupting {
            // Steering: shown above the composer until Claude picks it up.
            queuedMessages.append(QueuedMessage(id: messageID, text: text, images: images, attachments: attachments))
        } else {
            let entry = TranscriptEntry(id: messageID, kind: .user, text: text)
            entry.attachments = attachments
            if working { entry.delivery = .interrupted }
            entries.append(entry)
        }
        if isFirstMessage { app.generateTitle(for: self, from: .firstMessage(subject)) }
        phase = .running
        idleReaper?.cancel()
        touch()
        // Saved before Claude sees it: your message survives whatever happens next.
        persist()

        let stopFirst = working && interrupting
        if stopFirst { interruptingForMessage = true }
        let forAgent = isSideChat ? text : app.messageForAgent(text, projectID: projectID)
        Task {
            do {
                let session = try await ensureSession()
                if stopFirst { try await session.interrupt() }
                try await session.send(text: forAgent, images: images, id: messageID, priority: stopFirst ? .now : nil)
            } catch {
                fail(error.localizedDescription)
            }
        }
    }

    /// A queued message, right away: Claude is stopped, and the CLI hands it the message.
    func sendNow(_ message: QueuedMessage) {
        guard let index = queuedMessages.firstIndex(where: { $0.id == message.id }), queuedMessages[index].pending == nil,
              let start = sessionStart else { return }
        queuedMessages[index].pending = .sendingNow
        interruptingForMessage = true
        Task { try? await start.value.interrupt() }
    }

    /// Takes a queued message back; with `toComposer`, its text and images return there.
    func withdraw(_ message: QueuedMessage, toComposer: Bool) {
        guard let index = queuedMessages.firstIndex(where: { $0.id == message.id }), queuedMessages[index].pending == nil else { return }
        guard let start = sessionStart else {
            queuedMessages.remove(at: index)
            if toComposer { restore(message) }
            return
        }
        queuedMessages[index].pending = .withdrawing
        if toComposer { restoreOnWithdraw.insert(message.id) }
        Task { try? await start.value.cancelQueuedMessage(message.id) }
    }

    private func restore(_ message: QueuedMessage) {
        draft = draft.isEmpty ? message.text : draft + "\n" + message.text
        draftImages += message.images
    }

    /// Commands the app does itself, so its own state stays true: `/btw` opens the side
    /// chat, `/model` and `/effort` go through the same pickers as the composer's chips.
    private func handleLocally(_ command: (name: String, arguments: String)) -> Bool {
        switch command.name {
        case "btw":
            askAside(command.arguments)
            return true
        case "model" where !command.arguments.isEmpty:
            guard let model = app.claudeModels.first(where: { $0.value.caseInsensitiveCompare(command.arguments) == .orderedSame }) else { return false }
            setModel(model.value == "default" ? nil : model.value)
            entries.append(.notice("Model set to \(app.modelDisplayName(for: model.value))"))
            return true
        case "effort" where !command.arguments.isEmpty:
            guard let level = EffortLevel(rawValue: command.arguments.lowercased()) else { return false }
            setEffort(level.rawValue)
            entries.append(.notice("Reasoning effort set to \(level.label)"))
            return true
        default:
            return false
        }
    }

    // MARK: - Side chat

    /// Opens the side chat in the sidebar and asks it `question`, if there is one. It
    /// starts from a copy of this conversation as it is now.
    func askAside(_ question: String) {
        let chat = sideChat ?? startSideChat()
        app.showInspector(.sideChat)
        if !question.isEmpty { chat.send(question) }
    }

    private func startSideChat() -> ThreadModel {
        let chat = ThreadModel(sideChatOf: self, app: app)
        sideChat = chat
        return chat
    }

    /// Ends the side chat; the next `/btw` starts a new copy.
    func closeSideChat() {
        sideChat?.endSession()
        sideChat = nil
    }

    func stopSubagent(_ run: SubagentRun) {
        guard run.isRunning, let taskID = run.taskID, let start = sessionStart else { return }
        Task { try? await start.value.stopTask(taskID) }
    }

    /// Stops Claude. Anything queued is taken back into the composer first, so stopping
    /// never sends it.
    func stop() {
        guard phase == .running, let start = sessionStart else { return }
        for message in queuedMessages where message.pending == nil { withdraw(message, toComposer: true) }
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
        respond(to: request, with: allow ? .allow() : .deny(message: "The user denied this action."))
    }

    /// Answers the agent's questions; your choices go back as the tool's result.
    func answer(_ request: PermissionRequest, with answers: [String: [String]]) {
        respond(to: request, with: .allow(updatedInputJSON: AgentQuestion.answeredInput(inputJSON: request.inputJSON, answers: answers)))
    }

    /// Approving a plan leaves plan mode for `mode`, for this thread's whole project, as
    /// the permission chip shows. Rejecting it sends your feedback and keeps planning.
    func decidePlan(_ request: PermissionRequest, approve mode: PermissionMode?, feedback: String) {
        let feedback = feedback.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let mode else {
            respond(to: request, with: .deny(message: feedback.isEmpty
                ? "The user wants to keep planning. Ask what to change."
                : "The user wants changes to the plan before you start: \(feedback)"))
            return
        }
        respond(to: request, with: .allow(mode: mode.controlValue))
        app.setPermissionMode(mode, for: projectID)
        if !feedback.isEmpty { send(feedback) }
    }

    func respond(to request: PermissionRequest, with decision: PermissionDecision) {
        pendingPermissions.removeAll { $0.id == request.id }
        guard let start = sessionStart else { return }
        Task {
            do {
                try await start.value.respond(to: request, with: decision)
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
        if isWorking { restartWhenIdle = true } else { endSession() }
    }

    func endSession() {
        guard let start = sessionStart else { return }
        sessionStart = nil
        sessionGeneration += 1
        idleReaper?.cancel()
        // Its exit event is ignored from here on, so end its sub-agents now.
        if !liveSubagents.isEmpty {
            for run in liveSubagents { run.abandon() }
            liveSubagents.removeAll()
            persist()
        }
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
            let session = try ClaudeSession(options: try await app.launchOptions(for: self), servers: app.mcpServers(for: self))
            processID = session.processIdentifier
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
                self.receive(event, generation: generation)
            }
        }
    }

    /// Events held while no window can be seen (see `WindowPresence`).
    @ObservationIgnored private var heldEvents: [(event: AgentEvent, generation: Int)] = []

    /// Applies an event now or, while no window can be seen, holds it to apply with the rest
    /// in a batch. What needs you or ends the turn applies at once, after what came before it.
    private func receive(_ event: AgentEvent, generation: Int) {
        if WindowPresence.shared.isVisible || !event.canWait {
            applyHeld()
            apply(event, generation: generation)
        } else {
            if heldEvents.isEmpty { WindowPresence.shared.whenSeen { [weak self] in self?.applyHeld() } }
            heldEvents.append((event, generation))
        }
    }

    private func applyHeld() {
        guard !heldEvents.isEmpty else { return }
        let held = heldEvents
        heldEvents = []
        for item in held { apply(item.event, generation: item.generation) }
    }

    // MARK: - Reducer

    private func apply(_ event: AgentEvent, generation: Int) {
        // Saved as it goes, so quitting or a crash loses a moment, not the turn.
        defer { if phase == .running { persistSoon() } }
        // Everything but the exit event is harmless from a superseded session.
        switch event {
        case .sessionStarted(let info):
            receivedInit = true
            sessionID = info.sessionID
            activeModelName = info.model
            if let skills = info.skills, !isAside { app.noteSkills(skills, projectID: projectID) }
            app.save()

        case .status(let status):
            if status == "requesting" { phase = .running }
            activity = status == "compacting" ? "Compacting the conversation…" : nil

        case .commandOutput(let text):
            entries.append(TranscriptEntry(kind: .commandOutput, text: text))

        case .compacted(let compaction):
            activity = nil
            var notice = compaction.trigger == "manual" ? "Conversation compacted" : "Conversation compacted automatically"
            if let before = compaction.tokensBefore, let after = compaction.tokensAfter {
                notice += " · \(Self.tokens(before)) → \(Self.tokens(after)) tokens"
            }
            entries.append(.notice(notice))
            if let after = compaction.tokensAfter { contextTokens = after }

        case .conversationReset:
            // The CLI moves to a new session; its init event brings the new ID.
            entries.append(.notice("Context cleared. Claude starts fresh from here."))
            contextTokens = nil

        case .messageLifecycle(let messageID, let state):
            guard let index = queuedMessages.firstIndex(where: { $0.id == messageID }) else { return }
            let message = queuedMessages[index]
            switch state {
            case .started, .completed:
                // Claude has it now: into the transcript, where it entered the conversation.
                queuedMessages.remove(at: index)
                let entry = TranscriptEntry(id: message.id, kind: .user, text: message.text)
                entry.attachments = message.attachments
                // Picked up after the turn ended, it simply came next: nothing was steered.
                entry.delivery = message.pending == .sendingNow ? .interrupted : phase == .running ? .steered : .normal
                entries.append(entry)
                restoreOnWithdraw.remove(message.id)
            case .cancelled:
                queuedMessages.remove(at: index)
                if restoreOnWithdraw.remove(message.id) != nil { restore(message) }
            case .queued:
                break
            }

        case .titleChanged(let newTitle):
            // Only for your /rename: a title the CLI picks itself doesn't replace ours.
            guard renameRequested, !isAside else { return }
            renameRequested = false
            rename(newTitle)

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
            // A plan row from its first moment, not a tool row that turns into one.
            if case .toolUse(_, let name) = kind, TaskPlan.toolNames.contains(name) { rebuildPlan() }

        case .textDelta(let ref, let text):
            blocks[ref]?.text += text

        case .thinkingDelta(let ref, let text):
            guard let entry = blocks[ref], !text.isEmpty else { return }
            if entry.text.isEmpty { entries.append(entry) }
            entry.text += text

        case .toolInputDelta(let ref, let json):
            blocks[ref]?.toolInput += json

        case .assistantMessage(let messageID, let content, let parent):
            noteFootprint(content)
            for case .text(let text) in content { noteLinks(in: text) }
            if let parent {
                subagent(for: parent)?.append(content)
                return
            }
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
            if content.contains(where: { if case .toolUse(_, let name, let input) = $0 { TaskPlan.isPlanTool(name: name, inputJSON: input) } else { false } }) {
                rebuildPlan()
            }

        case .toolResults(let results, let parent):
            if let parent {
                // A sub-agent's work is the thread's work: PRs it opens or merges, folders
                // it writes in, links it comes across.
                for entry in subagent(for: parent)?.apply(results) ?? [] {
                    noteLinks(in: entry.toolResult ?? "")
                    addFootprint(ThreadFootprint.folders(toolName: entry.toolName, inputJSON: entry.toolInput, workingDirectory: effectiveDirectory))
                    guard !entry.toolIsError else { continue }
                    noteWork(toolName: entry.toolName, inputJSON: entry.toolInput, result: entry.toolResult ?? "")
                    if entry.toolName == "Bash", Self.touchesPullRequests(entry.toolInput) { app.pullRequestActivity(in: self) }
                }
                return
            }
            var plansChanged = false, ranCommand = false
            for result in results {
                guard let entry = tools[result.toolUseID] else { continue }
                entry.toolResult = result.content
                entry.toolIsError = result.isError
                entry.isStreaming = false
                if TaskPlan.isPlanTool(name: entry.toolName, inputJSON: entry.toolInput) { plansChanged = true }
                if entry.toolName == "Bash" {
                    ranCommand = true
                    // Merging, opening or pushing to a PR: its state is about to change.
                    if Self.touchesPullRequests(entry.toolInput) { app.pullRequestActivity(in: self) }
                }
                noteLinks(in: result.content)
                if !result.isError { noteWork(toolName: entry.toolName, inputJSON: entry.toolInput, result: result.content) }
                if entry.toolName == "EnterWorktree", !result.isError {
                    addFootprint(ThreadFootprint.folders(inResult: result.content))
                }
            }
            // A result can change how its turn folds (an error, a picture to show).
            rowsChanged()
            if plansChanged { rebuildPlan() }
            if ranCommand { refreshLocalServers() }

        case .subagentStarted(let start):
            subagent(for: start.toolUseID)?.start(start)

        case .subagentProgress(let id, let activity, let usage):
            subagent(for: id)?.update(activity: activity, usage: usage)

        case .subagentFinished(let end):
            guard let run = subagent(for: end.toolUseID) else { return }
            run.finish(end)
            liveSubagents.removeAll { $0 === run }
            // A background one finishing after its turn: keep what it did.
            if phase == .idle {
                persist()
                if liveSubagents.isEmpty { scheduleIdleReap() }
            }

        case .permissionRequest(let request):
            pendingPermissions.append(request)
            app.notify(.needsYou, about: self, Self.describe(request))

        case .rateLimit(let info):
            app.updateRateLimit(info)

        case .contextUsage(let tokens):
            contextTokens = tokens

        case .catalog(let catalog):
            app.updateCatalog(catalog, projectID: isAside ? nil : projectID)

        case .controlFailed, .mcpMessage:
            break

        case .turnCompleted(let result):
            activity = nil
            if !isAside { Task { await app.syncPullRequests(for: self, force: true) } }
            app.rememberContextWindows(result.contextWindows)
            if let model = activeModelName, let window = result.contextWindows[model] {
                contextWindow = window
            } else if let window = result.contextWindows.values.max() {
                contextWindow = window
            }
            if result.isError, interruptingForMessage {
                // Stopped for your message, which carries that on itself.
                interruptingForMessage = false
            } else if result.isError {
                if interruptRequested {
                    entries.append(.notice("Interrupted"))
                } else {
                    let message = result.resultText ?? "Claude stopped with an error (\(result.subtype))."
                    entries.append(.notice(message, style: .error))
                }
            }
            // You stopped it yourself: you're there, nothing to tell.
            if !interruptRequested, !interruptingForMessage {
                app.notify(.finished, about: self, result.isError ? (result.resultText ?? "Claude stopped with an error.") : lastReply)
            }
            interruptRequested = false
            settle()
            if !isAside { app.turnFinished(in: projectID) }
            if titleNeedsRefinement, !result.isError {
                titleNeedsRefinement = false
                app.generateTitle(for: self, from: .conversation)
            }
            scheduleIdleReap()
            if restartWhenIdle, liveSubagents.isEmpty {
                restartWhenIdle = false
                endSession()
            }

        case .processExited(let code, let stderr):
            guard generation == sessionGeneration else { return }
            sessionStart = nil
            let wasRunning = phase == .running
            for run in liveSubagents { run.abandon() }
            liveSubagents.removeAll()
            // Whatever was still waiting never reached Claude: back to the composer.
            for message in queuedMessages { restore(message) }
            queuedMessages.removeAll()
            restoreOnWithdraw.removeAll()
            interruptingForMessage = false
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

    /// What Claude is waiting for, in a notification.
    private static func describe(_ request: PermissionRequest) -> String {
        if request.toolName == AgentQuestion.toolName {
            let question = AgentQuestion.parse(inputJSON: request.inputJSON)?.first?.question
            return question.map { "Claude asks: \($0)" } ?? "Claude has a question for you."
        }
        if request.toolName == AgentPlan.toolName { return "Claude has a plan ready for you to review." }
        let presentation = ToolPresentation(name: request.toolName, inputJSON: request.inputJSON)
        let detail = presentation.detail.isEmpty ? "" : ": \(presentation.detail)"
        return "Allow \(presentation.verb.lowercased())\(detail)?"
    }

    /// The turn's last reply, as a notification's line or two.
    private var lastReply: String {
        let turn = entries.reversed().prefix { $0.kind != .user }
        guard let reply = turn.first(where: { $0.kind == .assistant && !$0.text.isEmpty })?.text else { return "Claude finished." }
        let plain = reply.replacingOccurrences(of: "[*_`#>]+", with: "", options: .regularExpression)
            .split(whereSeparator: \.isNewline).map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
            .joined(separator: " ")
        return plain.count > 180 ? String(plain.prefix(180)) + "…" : plain
    }

    /// Marks the turn finished: nothing is streaming, nothing is awaiting approval.
    private func settle() {
        if phase == .running, app.visibleThreadID != id { hasUnread = true }
        if phase == .running { noteTurnEnded() }
        phase = .idle
        pendingPermissions.removeAll()
        for entry in blocks.values where entry.isStreaming { entry.isStreaming = false }
        for entry in tools.values where entry.isStreaming { entry.isStreaming = false }
        blocks.removeAll()
        streamedMessageIDs.removeAll()
        // A foreground sub-agent can't outlive its turn; one still marked running was cut off.
        for run in liveSubagents where !run.isBackground { run.abandon() }
        liveSubagents.removeAll { !$0.isRunning }
        touch()
        persist()
    }

    /// How long the turn took goes on the message that started it.
    private func noteTurnEnded() {
        guard let start = entries.last(where: \.startsTurn), start.turnEndedAt == nil else { return }
        start.turnEndedAt = .now
        rowsChanged()
    }

    func jumpToMessage(_ direction: MessageJump.Direction) {
        messageJump = MessageJump(direction: direction)
    }

    /// Opens the turn that holds `entryID`, if it's folded away.
    func reveal(_ entryID: String) {
        showTurns(including: entryID)
        for case .work(let work) in transcriptDisplayRows where !work.isExpanded && work.entries.contains(where: { $0.id == entryID }) {
            expandedTurns.insert(work.turnID)
        }
    }

    /// The run for an Agent tool call, made on first sight of its tool call.
    private func subagent(for toolUseID: String) -> SubagentRun? {
        if let run = subagents[toolUseID] { return run }
        guard let entry = tools[toolUseID], SubagentReport.toolNames.contains(entry.toolName) else { return nil }
        let run = SubagentRun(toolCall: entry)
        entry.subagent = run
        subagents[toolUseID] = run
        liveSubagents.append(run)
        return run
    }

    private func fail(_ message: String) {
        if phase == .running { noteTurnEnded() }
        phase = .idle
        entries.append(.notice(message, style: .error))
        persist()
    }

    static func tokens(_ count: Int) -> String {
        count < 1000 ? "\(count)" : "\((Double(count) / 1000).formatted(.number.precision(.fractionLength(count < 10_000 ? 1 : 0))))k"
    }

    private func scheduleIdleReap() {
        idleReaper?.cancel()
        // A side chat or a memory specialist can't be resumed: it isn't saved. It lives while it's open.
        if isAside { return }
        idleReaper = Task { [weak self] in
            try? await Task.sleep(for: Self.idleTimeout)
            guard !Task.isCancelled, let self, !self.isWorking else { return }
            self.endSession()
        }
    }

    private func touch() {
        updatedAt = .now
    }

    private func persist() {
        pendingSave?.cancel()
        pendingSave = nil
        guard !isEphemeral else { return }
        // Only a transcript that's in memory: one that isn't would be saved as empty.
        if transcriptLoaded { app.store.saveTranscript(id, entries: entries.map(\.snapshot)) }
        app.save()
    }

    /// Lets go of the transcript to save memory; opening the thread reads it back. Only a
    /// thread that's idle, with nothing waiting to be saved or sent.
    func unloadTranscript() {
        guard transcriptLoaded, !isEphemeral, !isSideChat, !isWorking, pendingSave == nil,
              pendingPermissions.isEmpty, queuedMessages.isEmpty, sideChat == nil else { return }
        transcriptLoad?.cancel()
        transcriptLoad = nil
        transcriptLoaded = false
        entries.removeAll()
        expandedTurns.removeAll()
        rebuildPlan()
        app.store.forgetTranscript(id)
    }

    /// Saved now, not in a moment: an export reads the database.
    func saveForExport() {
        persist()
        sideChat?.saveForExport()
    }

    /// At most one save every couple of seconds while Claude streams.
    private func persistSoon() {
        guard !isEphemeral, pendingSave == nil else { return }
        pendingSave = Task { [weak self] in
            try? await Task.sleep(for: .seconds(2))
            guard !Task.isCancelled, let self else { return }
            self.persist()
        }
    }

    /// Quitting: a running turn is stopped and says so, what was queued goes back to the
    /// composer, and anything unsaved is saved.
    func prepareForQuit() {
        if phase == .running {
            for message in queuedMessages { restore(message) }
            queuedMessages.removeAll()
            entries.append(.notice("Stopped when YourHarness quit"))
            settle()
        } else if pendingSave != nil {
            persist()
        }
        // Stopped now: the app is about to exit, and an orphaned `claude` would carry on
        // with the turn. Asking the session to end needs a moment there isn't.
        if let processID, sessionStart != nil { kill(processID, SIGTERM) }
        sideChat?.prepareForQuit()
    }

    // MARK: - Footprint

    /// The folder `claude` runs in.
    var effectiveDirectory: String {
        workingDirectory ?? app.project(id: projectID)?.path ?? NSHomeDirectory()
    }

    /// Runs in `path` from the next session on (a worktree, say); nil goes back to the project.
    func setWorkingDirectory(_ path: String?) {
        guard path != workingDirectory else { return }
        workingDirectory = path
        // Claude keeps sessions per folder: one from elsewhere can't be resumed here.
        if sessionID != nil { entries.append(.notice("Now working in \((path ?? app.project(id: projectID)?.path ?? "").replacingOccurrences(of: NSHomeDirectory(), with: "~")). The next message starts a new Claude session there.")) }
        sessionID = nil
        endSession()
        if let path { addFootprint([path]) }
        persist()
    }

    private func noteFootprint(_ content: [ContentBlock]) {
        let cwd = effectiveDirectory
        var found: [String] = []
        for case .toolUse(_, let name, let input) in content {
            found += ThreadFootprint.folders(toolName: name, inputJSON: input, workingDirectory: cwd)
        }
        addFootprint(found)
    }

    private func addFootprint(_ folders: [String]) {
        let merged = ThreadFootprint.merge(folders, into: footprint)
        guard merged != footprint else { return }
        let new = Set(merged).subtracting(footprint)
        footprint = merged
        // Only when it reached somewhere new: the same folders again change nothing.
        if !new.isEmpty { app.footprintChanged(self) }
    }

    /// For a thread saved before footprints were kept: read once from its saved history,
    /// off the main thread.
    func computeFootprintIfNeeded() {
        guard !footprintKnown || !linksKnown || !workKnown, !isEphemeral else { return }
        let needsFootprint = !footprintKnown, needsLinks = !linksKnown, needsWork = !workKnown
        footprintKnown = true
        linksKnown = true
        workKnown = true
        let store = app.store, id = id, cwd = effectiveDirectory
        Task { [weak self] in
            let (folders, links, written, activities) = await Task.detached(priority: .utility) { () -> ([String], [String], [String], [PullRequestActivity]) in
                var found: [String] = [], links: [String] = [], written: [String] = [], activities: [PullRequestActivity] = []
                func walk(_ entries: [EntrySnapshot]) {
                    for entry in entries {
                        // PR links can be anywhere: your messages, replies, tool output.
                        for text in [entry.text, entry.toolResult ?? ""] where !text.isEmpty {
                            links = Self.mergeLinks(PullRequestLink.find(in: text).map(\.url.absoluteString), into: links)
                        }
                        guard entry.kind == .tool else { continue }
                        if entry.toolIsError != true {
                            written = ThreadFootprint.merge(ThreadFootprint.writtenFolders(toolName: entry.toolName ?? "", inputJSON: entry.toolInput ?? "", workingDirectory: cwd), into: written)
                            if let activity = PullRequestActivity.of(toolName: entry.toolName ?? "", inputJSON: entry.toolInput ?? "", result: entry.toolResult ?? "") {
                                activities.append(activity)
                            }
                        }
                        found = ThreadFootprint.merge(ThreadFootprint.folders(toolName: entry.toolName ?? "", inputJSON: entry.toolInput ?? "", workingDirectory: cwd), into: found)
                        if entry.toolName == "EnterWorktree", let result = entry.toolResult {
                            found = ThreadFootprint.merge(ThreadFootprint.folders(inResult: result), into: found)
                        }
                        if let subagent = entry.subagent { walk(subagent.entries) }
                    }
                }
                store.scanTranscript(id) { walk($0) }
                return (found, links, written, activities)
            }.value
            guard let self else { return }
            if needsFootprint {
                self.footprint = ThreadFootprint.merge(self.footprint, into: folders)
                self.app.footprintChanged(self)
            }
            if needsLinks { self.pullRequestLinks = Self.mergeLinks(self.pullRequestLinks, into: links) }
            if needsWork {
                self.writtenFootprint = ThreadFootprint.merge(self.writtenFootprint, into: written)
                let worked = activities.flatMap { self.app.links(for: $0, in: self) }
                self.workedPullRequestLinks = Self.mergeLinks(self.workedPullRequestLinks, into: worked)
                self.pullRequestLinks = Self.mergeLinks(worked, into: self.pullRequestLinks)
            }
            self.app.save()
        }
    }

    /// Notes PR and MR links in some text from the thread.
    private func noteLinks(in text: String) {
        let found = PullRequestLink.find(in: text).map(\.url.absoluteString)
        guard !found.isEmpty else { return }
        let merged = Self.mergeLinks(found, into: pullRequestLinks)
        if merged != pullRequestLinks { pullRequestLinks = merged }
    }

    /// PRs found another way (through a branch it works on): kept with the thread.
    func addPullRequestLinks(_ urls: [String]) {
        let merged = Self.mergeLinks(urls, into: pullRequestLinks)
        guard merged != pullRequestLinks else { return }
        pullRequestLinks = merged
        persist()
    }

    /// PRs it worked on: linked, and marked as its own work.
    func addWorkedPullRequestLinks(_ urls: [String]) {
        guard !urls.isEmpty else { return }
        let links = Self.mergeLinks(urls, into: pullRequestLinks), worked = Self.mergeLinks(urls, into: workedPullRequestLinks)
        guard links != pullRequestLinks || worked != workedPullRequestLinks else { return }
        pullRequestLinks = links
        workedPullRequestLinks = worked
        persist()
    }

    /// A command that merges, opens or pushes to a PR: its state is about to change.
    private static func touchesPullRequests(_ inputJSON: String) -> Bool {
        let command = (try? JSONSerialization.jsonObject(with: Data(inputJSON.utf8)) as? [String: Any])?["command"] as? String ?? ""
        return ["gh pr", "gh stack", "glab mr", "git push", "gh api"].contains(where: command.contains)
    }

    /// What one finished tool call changed: folders it wrote in, PRs it acted on.
    private func noteWork(toolName: String, inputJSON: String, result: String) {
        let written = ThreadFootprint.writtenFolders(toolName: toolName, inputJSON: inputJSON, workingDirectory: effectiveDirectory)
        if !written.isEmpty {
            let merged = ThreadFootprint.merge(written, into: writtenFootprint)
            if merged != writtenFootprint { writtenFootprint = merged }
        }
        if let activity = PullRequestActivity.of(toolName: toolName, inputJSON: inputJSON, result: result) {
            addWorkedPullRequestLinks(app.links(for: activity, in: self))
        }
    }

    nonisolated static func mergeLinks(_ new: [String], into existing: [String]) -> [String] {
        Array((existing + new.filter { !existing.contains($0) }).suffix(100))
    }

    // MARK: - Browser

    /// The browser, made on first use: by the agent, or by you opening the panel.
    func ensureBrowser() -> BrowserSession {
        if let browser { return browser }
        let browser = BrowserSession(projectID: projectID, restoring: savedBrowserTabs)
        browser.onChange = { [weak self] in self?.app.save() }
        browser.onPick = { [weak self] picked in self?.attachPicked(picked) }
        self.browser = browser
        return browser
    }

    /// Looks for servers under this thread's `claude` after a command: one started in the
    /// background takes a moment to listen, so it looks again a little later.
    func refreshLocalServers() {
        guard let processID, sessionStart != nil else { return }
        serverScan?.cancel()
        serverScan = Task { [weak self] in
            for delay in [1.5, 5.0] {
                try? await Task.sleep(for: .seconds(delay))
                guard !Task.isCancelled else { return }
                let ports = await LocalServers.listeningPorts(under: processID)
                guard !Task.isCancelled, let self else { return }
                if ports != self.listeningPorts { self.listeningPorts = ports }
            }
        }
    }

    /// A link to this Mac, clicked in the thread: it opens in the thread's browser, in the
    /// tab already on that server if there is one.
    func openInBrowser(_ url: URL) {
        let browser = ensureBrowser()
        browser.restoreIfNeeded()
        if let tab = browser.tabs.first(where: { $0.url?.host() == url.host() && $0.url?.port == url.port }) {
            browser.selectedTabID = tab.id
            tab.load(url.absoluteString)
        } else {
            browser.newTab(url: url.absoluteString, byAgent: false)
        }
        app.showInspector(.browser)
    }

    func closeBrowser() {
        if let browser { savedBrowserTabs = browser.savedURLs }
        browser?.closeAll()
        browser = nil
    }

    /// What you picked in the browser goes to the composer: where it is, and a picture.
    private func attachPicked(_ picked: PickedElement) {
        let separator = draft.isEmpty || draft.hasSuffix("\n") ? "" : "\n"
        draft += separator + picked.promptText + "\n"
        if let image = picked.image.flatMap(ImageAttachment.prepare) {
            draftImages.append(image)
        }
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

private extension AgentEvent {
    /// Whether it can wait for a batch while no window can be seen: the stream of a turn can,
    /// what needs you, ends the turn or answers a request can't.
    var canWait: Bool {
        switch self {
        case .status, .blockStarted, .textDelta, .thinkingDelta, .toolInputDelta, .assistantMessage, .toolResults,
             .subagentStarted, .subagentProgress, .subagentFinished, .commandOutput, .rateLimit, .contextUsage:
            true
        default:
            false
        }
    }
}
