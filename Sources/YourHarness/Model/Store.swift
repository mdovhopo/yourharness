import AppKit
import HarnessCore

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
    /// The right sidebar was open at quit.
    var showsInspector = false
    /// Each project's slash commands and skills as the CLI last listed them, keyed by
    /// project ID, so the "/" menu fills instantly.
    var slashCommands: [String: [SlashCommand]] = [:]
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
    /// Repos whose worktrees are folded in the repos view, by path.
    var collapsedWorktrees: Set<String> = []

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
        showsInspector = try container.decodeIfPresent(Bool.self, forKey: .showsInspector) ?? false
        slashCommands = try container.decodeIfPresent([String: [SlashCommand]].self, forKey: .slashCommands) ?? [:]
        appearance = try container.decodeIfPresent(Appearance.self, forKey: .appearance) ?? .system
        notifications = try container.decodeIfPresent(Bool.self, forKey: .notifications) ?? true
        planChecklist = try container.decodeIfPresent(Bool.self, forKey: .planChecklist) ?? true
        browserTools = try container.decodeIfPresent(Bool.self, forKey: .browserTools) ?? true
        browserAutoShow = try container.decodeIfPresent(Bool.self, forKey: .browserAutoShow) ?? true
        repoUsage = try container.decodeIfPresent([String: RepoUsage].self, forKey: .repoUsage) ?? [:]
        collapsedWorktrees = try container.decodeIfPresent(Set<String>.self, forKey: .collapsedWorktrees) ?? []
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

/// JSON files under Application Support. Thread metadata loads at launch; transcripts
/// load lazily when a thread is first opened, so startup cost doesn't grow with history.
struct Store: Sendable {
    let root: URL
    /// A development run against `YH_DATA_DIR`: it must never wander into your real folders.
    let isScratch: Bool
    private let queue = DispatchQueue(label: "yourharness.store", qos: .utility)

    init() {
        if let override = ProcessInfo.processInfo.environment["YH_DATA_DIR"], !override.isEmpty {
            // Development: run against a throwaway state instead of your real one.
            root = URL(fileURLWithPath: override, isDirectory: true)
            isScratch = true
        } else {
            isScratch = false
            let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            root = base.appending(path: "YourHarness", directoryHint: .isDirectory)
        }
        try? FileManager.default.createDirectory(at: threadsDirectory, withIntermediateDirectories: true)
        try? FileManager.default.createDirectory(at: attachmentsDirectory, withIntermediateDirectories: true)
    }

    private var stateURL: URL { root.appending(path: "state.json") }
    private var threadsDirectory: URL { root.appending(path: "threads", directoryHint: .isDirectory) }
    private func transcriptURL(_ id: UUID) -> URL { threadsDirectory.appending(path: "\(id.uuidString).json") }
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

    func loadState() -> PersistedState {
        guard let data = try? Data(contentsOf: stateURL) else { return PersistedState() }
        do {
            return try Self.decoder.decode(PersistedState.self, from: data)
        } catch {
            // Said out loud in a development run, where a bad hand-written state would
            // otherwise start fresh without a word.
            if isScratch { FileHandle.standardError.write(Data("state.json didn't load: \(error)\n".utf8)) }
            return PersistedState()
        }
    }

    func saveState(_ state: PersistedState) {
        write(state, to: stateURL)
    }

    func revealInFinder() {
        NSWorkspace.shared.activateFileViewerSelecting([root])
    }

    func loadTranscript(_ id: UUID) -> [EntrySnapshot] {
        guard let data = try? Data(contentsOf: transcriptURL(id)) else { return [] }
        return (try? Self.decoder.decode([EntrySnapshot].self, from: data)) ?? []
    }

    func saveTranscript(_ id: UUID, entries: [EntrySnapshot]) {
        write(entries, to: transcriptURL(id))
    }

    /// When the transcript was last written, to tell whether the session went on without it.
    func transcriptModified(_ id: UUID) -> Date? {
        try? FileManager.default.attributesOfItem(atPath: transcriptURL(id).path)[.modificationDate] as? Date
    }

    /// Waits for every write so far; at quit, so none is cut off.
    func flush() {
        queue.sync {}
    }

    func deleteTranscript(_ id: UUID) {
        let url = transcriptURL(id)
        queue.async { try? FileManager.default.removeItem(at: url) }
    }

    /// Encodes and writes on a serial background queue, so writes stay ordered and a long
    /// transcript saved mid-turn costs the main thread nothing.
    private func write(_ value: some Encodable & Sendable, to url: URL) {
        queue.async {
            guard let data = try? Self.encoder.encode(value) else { return }
            try? data.write(to: url, options: .atomic)
        }
    }

    private static let encoder: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        return encoder
    }()

    private static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }()
}
