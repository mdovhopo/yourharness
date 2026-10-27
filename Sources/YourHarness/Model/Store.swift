import AppKit
import HarnessCore
import Synchronization

enum PermissionMode: String, Codable, CaseIterable, Identifiable {
    case auto, ask, acceptEdits, plan, bypass

    /// What new projects start with unless Settings says otherwise.
    static let standard = PermissionMode.auto

    var id: String { rawValue }

    var cliValue: String? {
        switch self {
        case .auto: "auto"
        case .ask: nil
        case .acceptEdits: "acceptEdits"
        case .plan: "plan"
        case .bypass: "bypassPermissions"
        }
    }

    /// For the live `set_permission_mode` control request, which has no "unset".
    var controlValue: String { cliValue ?? "default" }

    var shortLabel: String {
        switch self {
        case .auto: "Auto"
        case .ask: "Ask"
        case .acceptEdits: "Accept Edits"
        case .plan: "Plan"
        case .bypass: "Bypass"
        }
    }

    var detail: String {
        switch self {
        case .auto: "Claude runs what looks safe and asks when something looks risky."
        case .ask: "Claude asks before editing files or running commands."
        case .acceptEdits: "File edits go through. Commands still ask."
        case .plan: "Claude explores and plans, but changes nothing."
        case .bypass: "Everything runs without asking. Use with care."
        }
    }

    var label: String {
        switch self {
        case .auto: "Auto"
        case .ask: "Ask Before Changes"
        case .acceptEdits: "Auto-Accept Edits"
        case .plan: "Plan Only"
        case .bypass: "Bypass Permissions"
        }
    }

    var symbol: String {
        switch self {
        case .auto: "checkmark.shield"
        case .ask: "hand.raised"
        case .acceptEdits: "pencil.and.outline"
        case .plan: "list.bullet.clipboard"
        case .bypass: "exclamationmark.shield"
        }
    }
}

struct Project: Codable, Identifiable, Hashable {
    var id = UUID()
    var name: String
    var path: String
    var permissionMode = PermissionMode.standard

    var url: URL { URL(fileURLWithPath: path, isDirectory: true) }
}

struct ThreadMeta: Codable {
    var id: UUID
    var projectID: UUID
    var title: String
    var createdAt: Date
    var updatedAt: Date
    var sessionID: String?
    var model: String?
    // Optionals decode as absent from older files, so these are safe to add.
    var effort: String?
    var contextTokens: Int?
    var contextWindow: Int?
    var isPinned: Bool?
    /// When the thread moved to the Settled shelf; nil while it's active.
    var settledAt: Date?
    /// False keeps the thread out of automatic settling.
    var autoSettle: Bool?
    /// Set by un-settling: the thread sorts as fresh, and isn't auto-settled again until
    /// you send something new.
    var reactivatedAt: Date?
    /// You named it (or asked Claude to): generated titles don't replace it until you ask
    /// for one with Regenerate Title.
    var titleChosen: Bool?
    /// Claude was working when this was saved. Still set at launch, the app closed
    /// mid-turn without saying so (a crash), and the turn is marked as cut off.
    var turnInProgress: Bool?
    /// What you'd typed and not sent.
    var draft: String?
    /// The browser's open tabs, reopened when it's next used.
    var browserTabs: [String]?
    /// Folders the thread's tool calls touched, newest last: which repos and worktrees it
    /// works in. Nil until worked out (threads from before this was kept).
    var footprint: [String]?
    /// Runs here instead of the project folder: a worktree, say.
    var workingDirectory: String?
    /// PR and MR links that came up in the thread. Nil until worked out.
    var pullRequests: [String]?
    /// Folders the thread changed something in (edits, commits, pushes), newest last.
    /// Nil until worked out.
    var writtenFootprint: [String]?
    /// The PRs it worked on, as opposed to ones it only mentioned. Nil until worked out.
    var workedPullRequests: [String]?
}

struct PersistedState: Codable {
    var projects: [Project] = []
    var threads: [ThreadMeta] = []
    var selectedThreadID: UUID?
    /// A project overview that was open at quit, shown again at launch.
    var overviewProjectID: UUID?
    /// The always-present project threads start in when you haven't picked a folder.
    var homeProjectID: UUID?
    /// User-chosen executable paths, keyed by `AgentCLI.id`.
    var cliPathOverrides: [String: String] = [:]
    var defaults = ThreadDefaults()
    /// Last known catalog, so pickers are populated instantly at launch.
    var claudeModels: [ClaudeModel] = []
    /// Context window sizes seen so far, keyed by resolved model ID.
    var contextWindows: [String: Int] = [:]
    var rateLimit: RateLimitInfo?
    var rateLimitUpdatedAt: Date?
    var collapsedProjectIDs: Set<UUID> = []
    /// Tell agents they can answer with diagrams. On unless turned off.
    var diagramInstructions = true
    /// Idle threads move to the Settled shelf after this many days; 0 turns it off.
    var autoSettleDays = 3
    /// Who names new threads. Claude Code's Haiku unless changed.
    var titleGeneration = TitleGenerationSettings()
    /// Your rules for titles in place of the built-in ones (`ThreadTitlePrompt.defaultRules`).
    var titlePrompt: String?
    /// A thread's agent may have its title rewritten when the subject changes. On unless
    /// turned off.
    var agentUpdatesTitles = true
    /// The right sidebar was open at quit.
    var showsInspector = false
    /// Each project's slash commands and skills as the CLI last listed them, keyed by
    /// project ID, so the "/" menu fills instantly.
    var slashCommands: [String: [SlashCommand]] = [:]
    /// Each project's skills as a session there last listed them: what the agent can run
    /// partway through a message.
    var skills: [String: [String]] = [:]
    var appearance = Appearance.system
    /// Tell you when a turn finishes or Claude needs you. On unless turned off.
    var notifications = true
    /// Give Claude its task tools, so long turns come with a checklist. On unless turned off.
    var planChecklist = true
    /// Give Claude the browser built into the app. On unless turned off.
    var browserTools = true
    /// Open the sidebar on the browser when Claude opens a page in the thread you're on.
    var browserAutoShow = true
    /// How much each repo is used, by its path: for sorting repos by use.
    var repoUsage: [String: RepoUsage] = [:]
    /// Repos whose worktrees are shown in the repos view, by path; the rest are folded.
    var expandedWorktrees: Set<String> = []
    /// A project's memory that was open at quit, shown again at launch.
    var memoryProjectID: UUID?
    /// Give agents the workspace's memory. On unless turned off.
    var memoryEnabled = true
    /// While it's on, Claude Code's own memory files are off, so there's one memory.
    var claudeCodeMemoryOff = true
    /// The model and effort the Importer and Reconciler start with; nil is the default for threads.
    var memoryAgentModel: String?
    var memoryAgentEffort: String?
    /// When each project's Claude Code memory was last brought in (or said to be in), by project ID.
    var claudeMemorySyncedAt: [String: Date] = [:]
    /// Projects whose "two memories" nudge you put off.
    var claudeMemoryNudgeDismissed: Set<UUID> = []
    /// Removed projects' IDs by folder: adding the folder again brings its memory back.
    var removedProjectIDs: [String: UUID] = [:]

    init() {}

    // Every field is optional on decode, so adding fields never wipes existing state.
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        projects = try container.decodeIfPresent([Project].self, forKey: .projects) ?? []
        threads = try container.decodeIfPresent([ThreadMeta].self, forKey: .threads) ?? []
        selectedThreadID = try container.decodeIfPresent(UUID.self, forKey: .selectedThreadID)
        overviewProjectID = try container.decodeIfPresent(UUID.self, forKey: .overviewProjectID)
        homeProjectID = try container.decodeIfPresent(UUID.self, forKey: .homeProjectID)
        cliPathOverrides = try container.decodeIfPresent([String: String].self, forKey: .cliPathOverrides) ?? [:]
        defaults = try container.decodeIfPresent(ThreadDefaults.self, forKey: .defaults) ?? ThreadDefaults()
        claudeModels = try container.decodeIfPresent([ClaudeModel].self, forKey: .claudeModels) ?? []
        contextWindows = try container.decodeIfPresent([String: Int].self, forKey: .contextWindows) ?? [:]
        rateLimit = try container.decodeIfPresent(RateLimitInfo.self, forKey: .rateLimit)
        rateLimitUpdatedAt = try container.decodeIfPresent(Date.self, forKey: .rateLimitUpdatedAt)
        collapsedProjectIDs = try container.decodeIfPresent(Set<UUID>.self, forKey: .collapsedProjectIDs) ?? []
        diagramInstructions = try container.decodeIfPresent(Bool.self, forKey: .diagramInstructions) ?? true
        autoSettleDays = try container.decodeIfPresent(Int.self, forKey: .autoSettleDays) ?? 3
        titleGeneration = try container.decodeIfPresent(TitleGenerationSettings.self, forKey: .titleGeneration) ?? TitleGenerationSettings()
        titlePrompt = try container.decodeIfPresent(String.self, forKey: .titlePrompt)
        agentUpdatesTitles = try container.decodeIfPresent(Bool.self, forKey: .agentUpdatesTitles) ?? true
        showsInspector = try container.decodeIfPresent(Bool.self, forKey: .showsInspector) ?? false
        slashCommands = try container.decodeIfPresent([String: [SlashCommand]].self, forKey: .slashCommands) ?? [:]
        skills = try container.decodeIfPresent([String: [String]].self, forKey: .skills) ?? [:]
        appearance = try container.decodeIfPresent(Appearance.self, forKey: .appearance) ?? .system
        notifications = try container.decodeIfPresent(Bool.self, forKey: .notifications) ?? true
        planChecklist = try container.decodeIfPresent(Bool.self, forKey: .planChecklist) ?? true
        browserTools = try container.decodeIfPresent(Bool.self, forKey: .browserTools) ?? true
        browserAutoShow = try container.decodeIfPresent(Bool.self, forKey: .browserAutoShow) ?? true
        repoUsage = try container.decodeIfPresent([String: RepoUsage].self, forKey: .repoUsage) ?? [:]
        expandedWorktrees = try container.decodeIfPresent(Set<String>.self, forKey: .expandedWorktrees) ?? []
        memoryProjectID = try container.decodeIfPresent(UUID.self, forKey: .memoryProjectID)
        memoryEnabled = try container.decodeIfPresent(Bool.self, forKey: .memoryEnabled) ?? true
        claudeCodeMemoryOff = try container.decodeIfPresent(Bool.self, forKey: .claudeCodeMemoryOff) ?? true
        removedProjectIDs = try container.decodeIfPresent([String: UUID].self, forKey: .removedProjectIDs) ?? [:]
        memoryAgentModel = try container.decodeIfPresent(String.self, forKey: .memoryAgentModel)
        claudeMemorySyncedAt = try container.decodeIfPresent([String: Date].self, forKey: .claudeMemorySyncedAt) ?? [:]
        claudeMemoryNudgeDismissed = try container.decodeIfPresent(Set<UUID>.self, forKey: .claudeMemoryNudgeDismissed) ?? []
        memoryAgentEffort = try container.decodeIfPresent(String.self, forKey: .memoryAgentEffort)
    }
}

/// The threads that have worked in a repo, in its main checkout or any of its worktrees.
/// Kept when a thread is deleted: it's a record of use, not a list of what's there.
struct RepoUsage: Codable, Equatable {
    var threads: Set<UUID> = []
    var lastUsed: Date?

    var count: Int { threads.count }
}

/// What new threads and projects start with. Nil means "let the CLI decide".
struct ThreadDefaults: Codable, Equatable {
    var model: String?
    var effort: String?
    var permissionMode: PermissionMode?
}

/// Light, dark, or whatever macOS is set to.
enum Appearance: String, Codable, CaseIterable, Identifiable {
    case system, light, dark

    var id: String { rawValue }

    var label: String {
        switch self {
        case .system: "System"
        case .light: "Light"
        case .dark: "Dark"
        }
    }

    /// Nil follows the system, live.
    var nsAppearance: NSAppearance? {
        switch self {
        case .system: nil
        case .light: NSAppearance(named: .aqua)
        case .dark: NSAppearance(named: .darkAqua)
        }
    }
}

enum EffortLevel: String, CaseIterable, Identifiable {
    case low, medium, high, xhigh, max

    var id: String { rawValue }

    var label: String {
        switch self {
        case .low: "Low"
        case .medium: "Medium"
        case .high: "High"
        case .xhigh: "Extra High"
        case .max: "Max"
        }
    }

    var detail: String {
        switch self {
        case .low: "Quickest replies, light reasoning"
        case .medium: "Balanced speed and depth"
        case .high: "Thinks things through"
        case .xhigh: "Deep reasoning for hard problems"
        case .max: "Everything it's got. Slowest."
        }
    }

    var symbol: String {
        switch self {
        case .low: "gauge.with.dots.needle.0percent"
        case .medium: "gauge.with.dots.needle.33percent"
        case .high: "gauge.with.dots.needle.50percent"
        case .xhigh: "gauge.with.dots.needle.67percent"
        case .max: "gauge.with.dots.needle.100percent"
        }
    }
}

/// Where the app keeps things: everything but files in one database (`HarnessDatabase`,
/// `harness.sqlite`) under Application Support, and images as files beside it. Thread
/// metadata loads at launch; transcripts load when a thread is first opened, so startup cost
/// doesn't grow with history. The JSON files of earlier versions move in on first launch.
final class Store: Sendable {
    let root: URL
    /// A development run against `YH_DATA_DIR`: it must never wander into your real folders.
    let isScratch: Bool
    let database: HarnessDatabase
    /// The state Import State brought in at this launch, with what's left to apply.
    let importedState: StateArchive.Applied?
    /// Attachments and screenshots, and turning the state into rows.
    private let queue = DispatchQueue(label: "yourharness.store", qos: .utility)
    /// Rows this version can't read (a newer one's), saved back as they were so they aren't lost.
    private let unreadable = Mutex<(projects: [HarnessDatabase.ProjectRow], threads: [HarnessDatabase.ThreadRow])>(([], []))

    init(root override: URL? = nil) {
        if let override {
            root = override
            isScratch = true
        } else if let override = ProcessInfo.processInfo.environment["YH_DATA_DIR"], !override.isEmpty {
            // Development: run against a throwaway state instead of your real one.
            root = URL(fileURLWithPath: override, isDirectory: true)
            isScratch = true
        } else {
            isScratch = false
            let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            root = base.appending(path: "YourHarness", directoryHint: .isDirectory)
        }
        // Development: start from a state file, without asking. Only ever a scratch state.
        if isScratch, let path = ProcessInfo.processInfo.environment["YH_STATE_IMPORT"], !path.isEmpty {
            do {
                try StateArchive.stage(try StateArchive.open(URL(fileURLWithPath: path)), in: root)
            } catch {
                FileHandle.standardError.write(Data("YH_STATE_IMPORT didn't import: \((error as? StateArchive.Problem)?.description ?? "\(error)")\n".utf8))
            }
        }
        // Before anything is read: an import replaces it all.
        importedState = StateArchive.applyPending(in: root)
        if let defaults = importedState?.extras.defaults { StateTransfer.applyDefaults(defaults) }
        database = HarnessDatabase(directory: root)
        try? FileManager.default.createDirectory(at: attachmentsDirectory, withIntermediateDirectories: true)
    }

    /// Images you sent, one file each, named `<thread id>-<image id>.<ext>`.
    private var attachmentsDirectory: URL { root.appending(path: "attachments", directoryHint: .isDirectory) }

    func attachmentURL(_ name: String) -> URL { attachmentsDirectory.appending(path: name) }

    /// A browser screenshot the agent took, kept for its row in the transcript. Named like
    /// attachments, so deleting the thread deletes it.
    func browserScreenshotURL(thread: UUID, toolUseID: String) -> URL {
        attachmentURL("\(thread.uuidString)-browser-\(toolUseID).jpg")
    }

    func saveBrowserScreenshot(_ data: Data, thread: UUID, toolUseID: String) {
        let url = browserScreenshotURL(thread: thread, toolUseID: toolUseID)
        queue.async { try? data.write(to: url, options: .atomic) }
    }

    /// Written right away: the message showing it is on screen a moment later.
    func saveAttachment(_ image: ImageAttachment, thread: UUID) -> String? {
        let name = "\(thread.uuidString)-\(image.id.uuidString).\(image.fileExtension)"
        do {
            try image.data.write(to: attachmentURL(name), options: .atomic)
            return name
        } catch {
            return nil
        }
    }

    func deleteAttachments(thread: UUID) {
        let directory = attachmentsDirectory
        let prefix = thread.uuidString + "-"
        queue.async {
            let names = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
            for name in names where name.hasPrefix(prefix) {
                try? FileManager.default.removeItem(at: directory.appending(path: name))
            }
        }
    }

    func revealInFinder() {
        NSWorkspace.shared.activateFileViewerSelecting([root])
    }

    // MARK: - State

    func loadState() -> PersistedState {
        let rows = database.loadState()
        var state = decodeSettings(rows.values)
        var unreadableProjects: [HarnessDatabase.ProjectRow] = [], unreadableThreads: [HarnessDatabase.ThreadRow] = []
        for row in rows.projects {
            if let project = try? Self.decoder.decode(Project.self, from: row.json) { state.projects.append(project) } else { unreadableProjects.append(row) }
        }
        for row in rows.threads {
            if let thread = try? Self.decoder.decode(ThreadMeta.self, from: row.json) { state.threads.append(thread) } else { unreadableThreads.append(row) }
        }
        if !unreadableProjects.isEmpty || !unreadableThreads.isEmpty {
            FileHandle.standardError.write(Data("Kept \(unreadableProjects.count) projects and \(unreadableThreads.count) threads this version can't read\n".utf8))
        }
        unreadable.withLock { $0 = (unreadableProjects, unreadableThreads) }
        return state
    }

    /// Settings come one row each. One a newer version changed so this one can't read it
    /// costs that setting, not the rest.
    private func decodeSettings(_ values: [String: Data]) -> PersistedState {
        var object: [String: Any] = [:]
        for (key, value) in values {
            object[key] = try? JSONSerialization.jsonObject(with: value, options: .fragmentsAllowed)
        }
        func decode(_ object: [String: Any]) -> PersistedState? {
            guard let data = try? JSONSerialization.data(withJSONObject: object) else { return nil }
            return try? Self.decoder.decode(PersistedState.self, from: data)
        }
        if let state = decode(object) { return state }
        var readable: [String: Any] = [:]
        for key in object.keys.sorted() {
            readable[key] = object[key]
            if decode(readable) == nil {
                readable[key] = nil
                // Said out loud in a development run, where a bad hand-written state would
                // otherwise start fresh without a word.
                if isScratch { FileHandle.standardError.write(Data("Setting \(key) didn't load\n".utf8)) }
            }
        }
        return decode(readable) ?? PersistedState()
    }

    /// Turned into rows and written in the background; only what changed is written.
    func saveState(_ state: PersistedState) {
        let database = database, unreadable = unreadable.withLock { $0 }
        queue.async {
            var settings = state
            settings.projects = []
            settings.threads = []
            var rows = HarnessDatabase.StateRows()
            if let object = (try? Self.encoder.encode(settings)).flatMap({ try? JSONSerialization.jsonObject(with: $0) }) as? [String: Any] {
                for (key, value) in object where key != "projects" && key != "threads" {
                    rows.values[key] = try? JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed, .sortedKeys, .withoutEscapingSlashes])
                }
            }
            rows.projects = state.projects.compactMap { project in
                (try? Self.encoder.encode(project)).map { .init(id: project.id.uuidString, json: $0) }
            } + unreadable.projects
            rows.threads = state.threads.compactMap { thread in
                (try? Self.encoder.encode(thread)).map {
                    .init(id: thread.id.uuidString, projectID: thread.projectID.uuidString, updatedAt: thread.updatedAt,
                          sessionID: thread.sessionID, json: $0)
                }
            } + unreadable.threads
            database.saveState(rows)
        }
    }

    // MARK: - Transcripts

    func loadTranscript(_ id: UUID) -> [EntrySnapshot] {
        database.loadTranscript(id)
    }

    /// Only the rows that changed are written, in the background.
    func saveTranscript(_ id: UUID, entries: [EntrySnapshot]) {
        database.saveTranscript(id, entries: entries)
    }

    /// The transcript was let go of in memory.
    func forgetTranscript(_ id: UUID) {
        database.forgetTranscript(id)
    }

    /// The transcript a page at a time, for going through a long one off the main thread.
    func scanTranscript(_ id: UUID, _ body: ([EntrySnapshot]) -> Void) {
        database.scanTranscript(id, body)
    }

    /// When the transcript was last written, to tell whether the session went on without it.
    func transcriptSavedAt(_ id: UUID) -> Date? {
        database.transcriptSavedAt(id)
    }

    func deleteTranscript(_ id: UUID) {
        database.deleteTranscript(id)
    }

    /// Waits for every write so far; at quit, so none is cut off.
    func flush() {
        queue.sync {}
        database.flush()
    }

    private static let encoder: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        // The same value always makes the same bytes, so an unchanged row isn't rewritten.
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return encoder
    }()

    private static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }()
}
